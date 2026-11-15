//! One HTTP request of the pet (REST to nacho-server, token calls to the login server). Bodies are built here so the
//! app, the dev CLI and the tests send exactly the same bytes. Port of `PetHTTP.swift`; the network itself is the
//! shell's job (WinHTTP on Windows, a plain socket client in the dev CLI) behind [`PetHTTPClient`].
use super::json::{self, JsonExt};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetHTTPRequest {
    pub method: String,
    /// Absolute `http(s)://` URL.
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
}

impl PetHTTPRequest {
    pub fn new(method: &str, url: impl Into<String>) -> Self {
        Self { method: method.into(), url: url.into(), headers: BTreeMap::new(), body: None }
    }

    fn with_bearer(mut h: BTreeMap<String, String>, bearer: Option<&str>) -> BTreeMap<String, String> {
        if let Some(b) = bearer.filter(|b| !b.is_empty()) { h.insert("Authorization".into(), format!("Bearer {b}")); }
        h
    }

    pub fn get(url: impl Into<String>, bearer: Option<&str>) -> Self {
        let h = BTreeMap::from([("Accept".to_string(), "application/json".to_string())]);
        Self { method: "GET".into(), url: url.into(), headers: Self::with_bearer(h, bearer), body: None }
    }

    /// JSON body with sorted keys.
    pub fn post_json(url: impl Into<String>, bearer: Option<&str>, body: &Value) -> Self {
        let h = BTreeMap::from([
            ("Accept".to_string(), "application/json".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ]);
        Self { method: "POST".into(), url: url.into(), headers: Self::with_bearer(h, bearer), body: Some(json::encode(body).into_bytes()) }
    }

    /// `application/x-www-form-urlencoded` (OAuth token endpoint). Keys are sorted for stable bytes.
    pub fn post_form(url: impl Into<String>, fields: &BTreeMap<String, String>) -> Self {
        let body = fields.iter().map(|(k, v)| format!("{}={}", Self::form_escape(k), Self::form_escape(v))).collect::<Vec<_>>().join("&");
        let h = BTreeMap::from([
            ("Accept".to_string(), "application/json".to_string()),
            ("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()),
        ]);
        Self { method: "POST".into(), url: url.into(), headers: h, body: Some(body.into_bytes()) }
    }

    /// RFC 3986 unreserved characters stay, everything else is percent-encoded (also `+`, `&`, `=`), UTF-8 bytes.
    pub fn form_escape(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { out.push(b as char) } else { out.push_str(&format!("%{b:02X}")) }
        }
        out
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn body_json(&self) -> Option<json::JsonObject> { self.body.as_deref().and_then(json::object) }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PetHTTPResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl PetHTTPResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self { Self { status, headers: BTreeMap::new(), body: body.into() } }
    pub fn with_header(mut self, k: &str, v: &str) -> Self { self.headers.insert(k.into(), v.into()); self }
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn is_success(&self) -> bool { (200..300).contains(&self.status) }
}

/// A request that never got an HTTP answer (no network, TLS, timeout). The text is for logs, never shown as is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetNetError(pub String);

impl std::fmt::Display for PetNetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

pub type HTTPResult = Result<PetHTTPResponse, PetNetError>;
pub type HTTPCompletion = Box<dyn FnOnce(HTTPResult) + Send>;

/// The network. Redirects are never followed (the OAuth callback is a custom scheme and must be read, not opened); no
/// cookies or cache on disk. `completion` may be called on any thread, exactly once.
pub trait PetHTTPClient: Send + Sync {
    fn send(&self, request: PetHTTPRequest, completion: HTTPCompletion);
}

/// Server error body `{"error":{"code","message"}}`, `{"code","message"}` or `{"error":"code"}` → code.
pub struct PetServerError;

impl PetServerError {
    pub fn code(data: &[u8]) -> Option<String> {
        let o = json::object(data)?;
        if let Some(c) = o.obj("error").and_then(|e| e.str("code")) { return Some(c); }
        if let Some(c) = o.str("code") { return Some(c); }
        o.str("error")
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod testing {
    //! A fake network for tests: records requests, answers from a queue (or holds them until `release`).
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub struct FakeHTTP {
        pub requests: Mutex<Vec<PetHTTPRequest>>,
        pub replies: Mutex<std::collections::VecDeque<HTTPResult>>,
        pub held: Mutex<Vec<(PetHTTPRequest, HTTPCompletion)>>,
        pub hold: std::sync::atomic::AtomicBool,
    }

    impl FakeHTTP {
        pub fn new() -> Arc<Self> { Arc::new(Self::default()) }
        pub fn reply(&self, r: HTTPResult) { self.replies.lock().unwrap().push_back(r); }
        pub fn reply_json(&self, status: u16, body: &str) { self.reply(Ok(PetHTTPResponse::new(status, body.as_bytes().to_vec()))); }
        pub fn sent(&self) -> Vec<PetHTTPRequest> { self.requests.lock().unwrap().clone() }
        /// Answers every held request with the next queued replies.
        pub fn release(&self) {
            let held: Vec<_> = std::mem::take(&mut *self.held.lock().unwrap());
            for (_, c) in held {
                let r = self.replies.lock().unwrap().pop_front().unwrap_or_else(|| Err(PetNetError("no reply".into())));
                c(r);
            }
        }
    }

    impl PetHTTPClient for FakeHTTP {
        fn send(&self, request: PetHTTPRequest, completion: HTTPCompletion) {
            self.requests.lock().unwrap().push(request.clone());
            if self.hold.load(std::sync::atomic::Ordering::SeqCst) {
                self.held.lock().unwrap().push((request, completion));
                return;
            }
            let r = self.replies.lock().unwrap().pop_front().unwrap_or_else(|| Err(PetNetError("no reply".into())));
            completion(r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding_escapes_reserved_characters() {
        assert_eq!(PetHTTPRequest::form_escape("a b+c&d=e/f~g"), "a%20b%2Bc%26d%3De%2Ff~g");
        assert_eq!(PetHTTPRequest::form_escape("나"), "%EB%82%98");
        let f = BTreeMap::from([("z".to_string(), "1".to_string()), ("a".to_string(), "x y".to_string())]);
        let r = PetHTTPRequest::post_form("https://e.test/t", &f);
        assert_eq!(String::from_utf8(r.body.clone().unwrap()).unwrap(), "a=x%20y&z=1");
        assert_eq!(r.header("content-type"), Some("application/x-www-form-urlencoded"));
    }

    #[test]
    fn bearer_and_json() {
        let r = PetHTTPRequest::post_json("https://e.test/x", Some("tok"), &serde_json::json!({"b":1,"a":"/"}));
        assert_eq!(r.header("authorization"), Some("Bearer tok"));
        assert_eq!(String::from_utf8(r.body.unwrap()).unwrap(), r#"{"a":"/","b":1}"#);
        assert_eq!(PetHTTPRequest::get("https://e.test", Some("")).header("Authorization"), None);
    }

    #[test]
    fn server_error_code() {
        assert_eq!(PetServerError::code(br#"{"error":{"code":"x","message":"m"}}"#).as_deref(), Some("x"));
        assert_eq!(PetServerError::code(br#"{"code":"y"}"#).as_deref(), Some("y"));
        assert_eq!(PetServerError::code(br#"{"error":"invalid_grant"}"#).as_deref(), Some("invalid_grant"));
        assert_eq!(PetServerError::code(b"nope"), None);
    }
}
