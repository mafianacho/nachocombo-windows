//! Windows pieces of Nacho Pet that the core cannot have: the network (WinHTTP for REST and `/ws/chat`,
//! `/ws/connector`), the token store (Credential Manager), OS randomness for PKCE and the local UTC offset. The pet UI
//! wires them into the core's session; until then nothing here runs (no network, no stored secrets at startup).
#![allow(dead_code)]
use crate::core::pet::chat_client::{TransportEvent, TransportHandler, WebSocketTransport};
use crate::core::pet::http::{HTTPCompletion, PetHTTPClient, PetHTTPRequest, PetHTTPResponse, PetNetError};
use crate::core::pet::reconnect::TransportFailure;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Credentials::*;

/// Random bytes from the OS (`BCryptGenRandom`, system RNG): PKCE verifier and `state`.
pub fn os_random(n: usize) -> Vec<u8> {
    use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    let mut buf = vec![0u8; n];
    let status = unsafe { BCryptGenRandom(None, &mut buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    assert!(status.is_ok(), "BCryptGenRandom failed"); // without randomness a login cannot be made safe
    buf
}

/// This PC's offset from UTC in seconds, for the core's clock (`clock::set_local_offset`).
pub fn local_utc_offset() -> i64 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tz = TIME_ZONE_INFORMATION::default();
    let mode = unsafe { GetTimeZoneInformation(&mut tz) };
    // 1 = standard time, 2 = daylight time (TIME_ZONE_ID_*); bias is minutes to add to local time to get UTC.
    let bias = tz.Bias + match mode { 1 => tz.StandardBias, 2 => tz.DaylightBias, _ => 0 };
    -(bias as i64) * 60
}

// ---------- token store ----------

/// Pet secrets in the Windows Credential Manager (generic credentials of this user, `NachoCombo/pet/<key>`), kept
/// across restarts and sign-ins on this PC (`CRED_PERSIST_LOCAL_MACHINE`, which is still per user). A credential
/// holds at most 2560 bytes, the pet's secret bundle can be larger, so a long value is split over `<key>`, `<key>#1`,
/// `<key>#2`… with the part count in the first item's comment. Other programs running as the same user can read
/// these, like the macOS login Keychain without an access list.
pub struct CredSecretStore {
    prefix: String,
}

const CRED_PART: usize = 2048;

impl Default for CredSecretStore {
    fn default() -> Self { Self { prefix: "NachoCombo/pet/".into() } }
}

/// What reading a credential gave: the bytes, nothing (not found) or an error code (anything else: never "signed out").
pub enum CredRead { Found(Vec<u8>, Option<String>), Missing, Failed(i32) }

impl CredSecretStore {
    fn target(&self, key: &str, part: usize) -> HSTRING {
        if part == 0 { HSTRING::from(format!("{}{key}", self.prefix)) } else { HSTRING::from(format!("{}{key}#{part}", self.prefix)) }
    }

    fn read_one(target: &HSTRING) -> CredRead {
        unsafe {
            let mut p: *mut CREDENTIALW = std::ptr::null_mut();
            match CredReadW(target, CRED_TYPE_GENERIC, None, &mut p) {
                Ok(()) => {
                    let c = &*p;
                    let data = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
                    let comment = if c.Comment.is_null() { None } else { c.Comment.to_string().ok() };
                    CredFree(p as *const _);
                    CredRead::Found(data, comment)
                }
                Err(e) if e.code() == windows::Win32::Foundation::ERROR_NOT_FOUND.to_hresult() => CredRead::Missing,
                Err(e) => CredRead::Failed(e.code().0),
            }
        }
    }

    fn write_one(target: &HSTRING, data: &[u8], comment: &str) -> Result<(), i32> {
        let mut comment: Vec<u16> = comment.encode_utf16().chain([0]).collect();
        let mut name: Vec<u16> = target.to_string().encode_utf16().chain([0]).collect();
        let mut user: Vec<u16> = "NachoCombo".encode_utf16().chain([0]).collect();
        let c = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(name.as_mut_ptr()),
            Comment: PWSTR(comment.as_mut_ptr()),
            CredentialBlobSize: data.len() as u32,
            CredentialBlob: data.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..Default::default()
        };
        unsafe { CredWriteW(&c, 0).map_err(|e| e.code().0) }
    }

    fn delete_one(target: &HSTRING) { unsafe { let _ = CredDeleteW(target, CRED_TYPE_GENERIC, None); } }

    /// The whole value; a part that is missing or unreadable makes the value unreadable (never half a token).
    pub fn read(&self, key: &str) -> CredRead {
        let (first, comment) = match Self::read_one(&self.target(key, 0)) {
            CredRead::Found(d, c) => (d, c),
            other => return other,
        };
        let parts = comment.as_deref().and_then(|c| c.strip_prefix("parts=")).and_then(|n| n.parse::<usize>().ok()).unwrap_or(1);
        let mut out = first;
        for i in 1..parts {
            match Self::read_one(&self.target(key, i)) {
                CredRead::Found(d, _) => out.extend_from_slice(&d),
                CredRead::Missing => return CredRead::Failed(-1),
                f => return f,
            }
        }
        CredRead::Found(out, None)
    }

    /// Writes the extra parts first and the first item last, so a reader never sees a part count that is not there yet.
    pub fn write(&self, key: &str, value: &[u8]) -> Result<(), i32> {
        let chunks: Vec<&[u8]> = if value.is_empty() { vec![&[][..]] } else { value.chunks(CRED_PART).collect() };
        let old_parts = match Self::read_one(&self.target(key, 0)) {
            CredRead::Found(_, Some(c)) => c.strip_prefix("parts=").and_then(|n| n.parse::<usize>().ok()).unwrap_or(1),
            _ => 1,
        };
        for (i, chunk) in chunks.iter().enumerate().skip(1) { Self::write_one(&self.target(key, i), chunk, "")?; }
        Self::write_one(&self.target(key, 0), chunks[0], &format!("parts={}", chunks.len()))?;
        for i in chunks.len()..old_parts { Self::delete_one(&self.target(key, i)); }
        Ok(())
    }

    pub fn delete(&self, key: &str) {
        let parts = match Self::read_one(&self.target(key, 0)) {
            CredRead::Found(_, Some(c)) => c.strip_prefix("parts=").and_then(|n| n.parse::<usize>().ok()).unwrap_or(1),
            _ => 1,
        };
        for i in (0..parts.max(1)).rev() { Self::delete_one(&self.target(key, i)); }
    }
}

// ---------- HTTP ----------

struct Handle(*mut core::ffi::c_void);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) { if !self.0.is_null() { unsafe { let _ = WinHttpCloseHandle(self.0); } } }
}

/// `scheme://host[:port]/path?query` → (secure, host, port, object). Only http/https/ws/wss.
fn split_url(url: &str) -> Option<(bool, String, u16, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let secure = match scheme.to_ascii_lowercase().as_str() { "https" | "wss" => true, "http" | "ws" => false, _ => return None };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, object) = rest.split_at(end);
    let object = if object.is_empty() { "/".to_string() } else if object.starts_with('?') { format!("/{object}") } else { object.to_string() };
    let default_port = if secure { 443 } else { 80 };
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']')?;
        (h.to_string(), match after.strip_prefix(':') { Some(p) => p.parse().ok()?, None if after.is_empty() => default_port, None => return None })
    } else {
        match authority.rsplit_once(':') { Some((h, p)) => (h.to_string(), p.parse().ok()?), None => (authority.to_string(), default_port) }
    };
    Some((secure, host, port, object))
}

fn session(timeout_ms: i32) -> Result<Handle, PetNetError> {
    unsafe {
        let s = Handle(WinHttpOpen(&HSTRING::from(format!("NachoCombo/{}", env!("CARGO_PKG_VERSION"))), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if s.0.is_null() { return Err(PetNetError("WinHttpOpen failed".into())); }
        let _ = WinHttpSetTimeouts(s.0, timeout_ms, timeout_ms, timeout_ms, timeout_ms);
        Ok(s)
    }
}

/// Opens a request with redirects and cookies off (the login callback must be read, not followed).
fn open_request(s: &Handle, method: &str, url: &str) -> Result<(Handle, Handle), PetNetError> {
    let (secure, host, port, object) = split_url(url).ok_or_else(|| PetNetError("bad URL".into()))?;
    unsafe {
        let c = Handle(WinHttpConnect(s.0, &HSTRING::from(host), port, 0));
        if c.0.is_null() { return Err(PetNetError("WinHttpConnect failed".into())); }
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let r = Handle(WinHttpOpenRequest(c.0, &HSTRING::from(method), &HSTRING::from(object), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags));
        if r.0.is_null() { return Err(PetNetError("WinHttpOpenRequest failed".into())); }
        let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes();
        let _ = WinHttpSetOption(Some(r.0), WINHTTP_OPTION_REDIRECT_POLICY, Some(&never));
        let no_cookies = WINHTTP_DISABLE_COOKIES.to_ne_bytes();
        let _ = WinHttpSetOption(Some(r.0), WINHTTP_OPTION_DISABLE_FEATURE, Some(&no_cookies));
        Ok((c, r))
    }
}

fn add_headers(r: &Handle, headers: &BTreeMap<String, String>) {
    for (k, v) in headers {
        if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) { continue; } // never let a value inject a header
        let line: Vec<u16> = format!("{k}: {v}").encode_utf16().collect();
        unsafe { let _ = WinHttpAddRequestHeaders(r.0, &line, WINHTTP_ADDREQ_FLAG_ADD); }
    }
}

fn status_of(r: &Handle) -> u16 {
    let mut status = 0u32;
    let mut len = 4u32;
    unsafe {
        let _ = WinHttpQueryHeaders(r.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut status as *mut u32 as *mut _), &mut len, std::ptr::null_mut());
    }
    status as u16
}

fn headers_of(r: &Handle) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    unsafe {
        let mut len = 0u32;
        let _ = WinHttpQueryHeaders(r.0, WINHTTP_QUERY_RAW_HEADERS_CRLF, PCWSTR::null(), None, &mut len, std::ptr::null_mut());
        if len == 0 { return out; }
        let mut buf = vec![0u16; len as usize / 2 + 1];
        if WinHttpQueryHeaders(r.0, WINHTTP_QUERY_RAW_HEADERS_CRLF, PCWSTR::null(), Some(buf.as_mut_ptr() as *mut _), &mut len, std::ptr::null_mut()).is_ok() {
            let text = String::from_utf16_lossy(&buf[..len as usize / 2]);
            for line in text.lines().skip(1) {
                if let Some((k, v)) = line.split_once(':') { out.insert(k.trim().to_string(), v.trim().to_string()); }
            }
        }
    }
    out
}

fn perform(req: &PetHTTPRequest, timeout_ms: i32) -> Result<PetHTTPResponse, PetNetError> {
    let s = session(timeout_ms)?;
    let (_c, r) = open_request(&s, &req.method, &req.url)?;
    add_headers(&r, &req.headers);
    let body = req.body.as_deref().unwrap_or(&[]);
    unsafe {
        let ptr = if body.is_empty() { None } else { Some(body.as_ptr() as *const _) };
        WinHttpSendRequest(r.0, None, ptr, body.len() as u32, body.len() as u32, 0).map_err(|e| PetNetError(format!("send: {e}")))?;
        WinHttpReceiveResponse(r.0, std::ptr::null_mut()).map_err(|e| PetNetError(format!("receive: {e}")))?;
        let status = status_of(&r);
        let headers = headers_of(&r);
        let mut data = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let mut n = 0u32;
            WinHttpReadData(r.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut n).map_err(|e| PetNetError(format!("read: {e}")))?;
            if n == 0 { break; }
            data.extend_from_slice(&buf[..n as usize]);
            if data.len() > 64 * 1024 * 1024 { return Err(PetNetError("response too large".into())); }
        }
        Ok(PetHTTPResponse { status, headers, body: data })
    }
}

/// REST through WinHTTP, one worker thread per request (the pet makes a handful; the UI thread never waits).
pub struct WinHttpClient {
    pub timeout_ms: i32,
}

impl Default for WinHttpClient {
    fn default() -> Self { Self { timeout_ms: 30_000 } }
}

impl PetHTTPClient for WinHttpClient {
    fn send(&self, request: PetHTTPRequest, completion: HTTPCompletion) {
        let timeout = self.timeout_ms;
        std::thread::spawn(move || completion(perform(&request, timeout)));
    }
}

// ---------- WebSocket ----------

#[derive(Default)]
struct WsState {
    socket: Option<Arc<Handle>>,
    closed: bool,
}

/// `/ws/chat` and `/ws/connector` through WinHTTP's WebSocket API. One object per connection attempt (the core makes a
/// new one each time); a reader thread delivers `Opened`, every text message, then one `Closed`.
#[derive(Default)]
pub struct WinHttpWebSocket {
    state: Arc<Mutex<WsState>>,
}

fn upgrade(url: &str, bearer: Option<&str>) -> Result<(Handle, Handle, Handle), TransportFailure> {
    let fail = |m: &str| TransportFailure::new(None, m);
    let s = session(30_000).map_err(|e| fail(&e.0))?;
    let (c, r) = open_request(&s, "GET", url).map_err(|e| fail(&e.0))?;
    unsafe {
        WinHttpSetOption(Some(r.0), WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET, None).map_err(|_| fail("upgrade option"))?;
        let mut h = BTreeMap::new();
        if let Some(b) = bearer.filter(|b| !b.is_empty()) { h.insert("Authorization".to_string(), format!("Bearer {b}")); }
        add_headers(&r, &h);
        WinHttpSendRequest(r.0, None, None, 0, 0, 0).map_err(|e| fail(&format!("send: {e}")))?;
        WinHttpReceiveResponse(r.0, std::ptr::null_mut()).map_err(|e| fail(&format!("receive: {e}")))?;
        let status = status_of(&r);
        if status != 101 { return Err(TransportFailure::new(Some(status), format!("HTTP {status}"))); }
        let ws = Handle(WinHttpWebSocketCompleteUpgrade(r.0, None));
        if ws.0.is_null() { return Err(fail("complete upgrade")); }
        Ok((s, c, ws))
    }
}

impl WebSocketTransport for WinHttpWebSocket {
    fn connect(&self, url: &str, bearer: Option<&str>, handler: TransportHandler) {
        let (url, bearer, state) = (url.to_string(), bearer.map(str::to_string), self.state.clone());
        std::thread::spawn(move || {
            let (_s, _c, ws) = match upgrade(&url, bearer.as_deref()) {
                Ok(x) => x,
                Err(f) => return handler(TransportEvent::Closed(Some(f))),
            };
            let ws = Arc::new(ws);
            {
                let mut st = state.lock().unwrap();
                if st.closed { return; }
                st.socket = Some(ws.clone());
            }
            handler(TransportEvent::Opened);
            let mut msg = Vec::new();
            let mut buf = vec![0u8; 64 * 1024];
            let failure = loop {
                let mut n = 0u32;
                let mut kind = WINHTTP_WEB_SOCKET_BUFFER_TYPE(0);
                let err = unsafe { WinHttpWebSocketReceive(ws.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut n, &mut kind) };
                if err != 0 { break Some(TransportFailure::new(None, format!("receive error {err}"))); }
                msg.extend_from_slice(&buf[..n as usize]);
                if kind == WINHTTP_WEB_SOCKET_UTF8_FRAGMENT_BUFFER_TYPE { continue; }
                if kind == WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE {
                    handler(TransportEvent::Text(String::from_utf8_lossy(&msg).into_owned()));
                } else if kind == WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE {
                    break None;
                }
                msg.clear(); // binary frames are not part of the protocol: dropped
            };
            let was_closed = {
                let mut st = state.lock().unwrap();
                st.socket = None;
                std::mem::replace(&mut st.closed, true)
            };
            // A close we asked for is reported as a clean close.
            handler(TransportEvent::Closed(if was_closed { None } else { failure }));
        });
    }

    fn send(&self, text: &str) {
        let socket = self.state.lock().unwrap().socket.clone();
        if let Some(ws) = socket {
            unsafe { let _ = WinHttpWebSocketSend(ws.0, WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE, Some(text.as_bytes())); }
        }
    }

    fn close(&self) {
        let socket = {
            let mut st = self.state.lock().unwrap();
            st.closed = true;
            st.socket.clone()
        };
        if let Some(ws) = socket { unsafe { let _ = WinHttpWebSocketClose(ws.0, 1000, None, 0); } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_split_for_winhttp() {
        assert_eq!(split_url("https://nacho.example/api/v1/me?x=1"), Some((true, "nacho.example".into(), 443, "/api/v1/me?x=1".into())));
        assert_eq!(split_url("ws://127.0.0.1:8787/ws/chat"), Some((false, "127.0.0.1".into(), 8787, "/ws/chat".into())));
        assert_eq!(split_url("http://[::1]:8787"), Some((false, "::1".into(), 8787, "/".into())));
        assert_eq!(split_url("wss://h.example?q"), Some((true, "h.example".into(), 443, "/?q".into())));
        assert_eq!(split_url("http://[::1]"), Some((false, "::1".into(), 80, "/".into())));
        assert_eq!(split_url("ftp://x"), None);
    }
}
