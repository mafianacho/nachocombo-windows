//! NachoCombo MCP in the app: the server's lifetime (Settings ▸ MCP switch), its API keys, and the approval card on the
//! pet's chat. The protocol, the checks and the socket are `core::mcp` (tested on Linux); this file connects them to
//! the gadgets (the connector's executor, `pet_exec`), to the pet (cards) and to the settings.
//!
//! - The port is open only while Settings ▸ MCP ▸ the server switch is on, on 127.0.0.1. Switching it off (or the
//!   port, or quitting) closes the listening socket, every open connection and the server's threads.
//! - What the connection threads read (settings, which gadgets run, "always allow") is a snapshot the UI thread
//!   refreshes on every change ([`sync`]); they never touch the app's state.
//! - The port is bound with `SO_EXCLUSIVEADDRUSE` ([`exclusive_listener`]): another program cannot bind the same
//!   address alongside and take connections (and keys) meant for Nacho Combo. A port that is taken fails the switch.
//! - `computer-use-keys.json` gets a protected DACL for the current user only before any hash is written into it
//!   ([`owner_only`]); if that fails, nothing is written.
//! - An approval waits on the pet's chat as a local card (`core::pet::mcp_card`), never on nacho-server. Without Nacho
//!   Pet the call is refused (`approval_unavailable`); unanswered for 60 s it is refused (`approval_timeout`).
use crate::app::{app, State};
use crate::core::mcp::keys::{IssuedKey, KeyStore};
use crate::core::mcp::server::McpServer;
use crate::core::mcp::service::{CallLogEntry, McpApproval, McpApprovalRequest, McpApproving, McpService, McpSources};
use crate::core::mcp::settings::McpSettings;
use crate::core::pet::mcp_card::McpCards;
use crate::core::pet::transcript::PetChatItem;
use crate::core::settings::ModuleId;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};

type Done = Box<dyn FnOnce(McpApproval) + Send>;

struct Pending { request: McpApprovalRequest, done: Option<Done>, answering: bool }

#[derive(Default)]
struct Snapshot { settings: McpSettings, running: BTreeSet<ModuleId> }

#[derive(Default)]
struct Runtime {
    server: Option<McpServer>,
    service: Option<Arc<McpService>>,
    port: u16,
    problem: Option<String>,
}

static SNAP: Mutex<Option<Snapshot>> = Mutex::new(None);
static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);
static KEYS: OnceLock<Arc<KeyStore>> = OnceLock::new();
/// The key made a moment ago, shown once on Settings ▸ MCP until the user closes it or leaves the page.
static ISSUED: Mutex<Option<IssuedKey>> = Mutex::new(None);

pub fn keys() -> &'static Arc<KeyStore> {
    KEYS.get_or_init(|| {
        Arc::new(KeyStore::new(Some(KeyStore::path_in(&app().dir)), Arc::new(crate::pet_win::os_random), crate::core::mcp::limits::system_clock())
            .with_protection(Arc::new(owner_only)))
    })
}

// MARK: lifetime (UI thread)

/// Brings the server in line with the settings and refreshes what the connection threads read. Called on every change
/// (`App::changed`) and at start.
pub fn sync() {
    let a = app();
    let settings = a.settings.computer_use.clone();
    let running: BTreeSet<ModuleId> = ModuleId::ALL.into_iter().filter(|m| a.state(*m) == State::Running).collect();
    *SNAP.lock().unwrap() = Some(Snapshot { settings: settings.clone(), running });
    let want = settings.server_enabled.then_some(settings.port);
    let mut guard = RUNTIME.lock().unwrap();
    let rt = guard.get_or_insert_with(Runtime::default);
    let current = rt.server.as_ref().map(|_| rt.port);
    if current == want { return; }
    if current.is_some() { close(rt); }
    rt.problem = None;
    let Some(port) = want else { return };
    // A port another program holds is tried again on the next change.
    let service = Arc::new(service());
    match McpServer::start_with(service.clone(), port, exclusive_listener) {
        Ok(s) => { rt.server = Some(s); rt.service = Some(service); rt.port = port; }
        Err(e) => rt.problem = Some(e),
    }
}

fn close(rt: &mut Runtime) {
    if let Some(mut s) = rt.server.take() { s.stop(); }
    rt.service = None;
    // Calls still waiting for a card cannot be answered any more.
    let gone: Vec<Pending> = std::mem::take(&mut *PENDING.lock().unwrap());
    for mut p in gone { if let Some(d) = p.done.take() { d(McpApproval::Denied) } }
    crate::pet_ui::invalidate_chat();
}

/// At quit: the port closes with the app.
pub fn shutdown() { if let Some(rt) = RUNTIME.lock().unwrap().as_mut() { close(rt); } }

fn service() -> McpService {
    let sources = McpSources {
        settings: Arc::new(|| SNAP.lock().unwrap().as_ref().map(|s| s.settings.clone()).unwrap_or_default()),
        always_allowed: Arc::new(|| crate::pet_exec::trust().always_allowed()),
        module_on: Arc::new(|m| SNAP.lock().unwrap().as_ref().is_some_and(|s| s.running.contains(&m))),
    };
    McpService::new(sources, keys().clone(), Arc::new(crate::pet_exec::WindowsPetExecutor), Arc::new(PetApprover), crate::core::mcp::limits::system_clock())
}

/// `127.0.0.1:<port>` with `SO_EXCLUSIVEADDRUSE` set before the bind, so no other socket can share the address
/// (Windows otherwise lets a second socket bind it with `SO_REUSEADDR`). The socket is not inherited by child processes.
fn exclusive_listener(port: u16) -> std::io::Result<std::net::TcpListener> {
    use std::os::windows::io::FromRawSocket;
    use windows::Win32::Networking::WinSock::*;
    unsafe {
        let mut data = WSADATA::default();
        if WSAStartup(0x0202, &mut data) != 0 { return Err(std::io::Error::last_os_error()); }
        let s = WSASocketW(AF_INET.0 as i32, SOCK_STREAM.0, IPPROTO_TCP.0, None, 0, WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT)
            .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
        let fail = |s: SOCKET| { let e = std::io::Error::from_raw_os_error(WSAGetLastError().0); closesocket(s); e };
        let on: i32 = 1;
        if setsockopt(s, SOL_SOCKET, SO_EXCLUSIVEADDRUSE, Some(&on.to_ne_bytes())) != 0 { return Err(fail(s)); }
        let addr = SOCKADDR_IN {
            sin_family: AF_INET,
            sin_port: port.to_be(),
            sin_addr: IN_ADDR { S_un: IN_ADDR_0 { S_addr: u32::from_ne_bytes([127, 0, 0, 1]) } },
            sin_zero: [0; 8],
        };
        if bind(s, &addr as *const SOCKADDR_IN as *const SOCKADDR, std::mem::size_of::<SOCKADDR_IN>() as i32) != 0 { return Err(fail(s)); }
        if listen(s, 64) != 0 { return Err(fail(s)); }
        Ok(std::net::TcpListener::from_raw_socket(s.0 as u64))
    }
}

/// A protected DACL with one entry: full access for the current user. Nothing is inherited from the folder.
fn owner_only(path: &std::path::Path) -> std::io::Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::*;
    use windows::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let os = |e: windows::core::Error| std::io::Error::other(e.message());
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(os)?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        // u64 storage keeps the TOKEN_USER aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), len, &mut len);
        let _ = CloseHandle(token);
        got.map_err(os)?;
        let sid = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
        // ACL header + one ACCESS_ALLOWED_ACE (its SidStart u32 overlaps the SID).
        let acl_len = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>() + GetLengthSid(sid) as usize - 4;
        let mut acl_buf = vec![0u64; acl_len.div_ceil(8)];
        let acl = acl_buf.as_mut_ptr() as *mut ACL;
        InitializeAcl(acl, acl_len as u32, ACL_REVISION).map_err(os)?;
        AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS.0, sid).map_err(os)?;
        let mut sd = SECURITY_DESCRIPTOR::default();
        let psd = PSECURITY_DESCRIPTOR(&mut sd as *mut _ as *mut _);
        InitializeSecurityDescriptor(psd, 1).map_err(os)?;
        SetSecurityDescriptorDacl(psd, true, Some(acl), false).map_err(os)?;
        if !SetFileSecurityW(&HSTRING::from(path.as_os_str()), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, psd).as_bool() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

pub fn is_open() -> bool { RUNTIME.lock().unwrap().as_ref().is_some_and(|r| r.server.is_some()) }

pub fn url() -> String { format!("http://127.0.0.1:{}/mcp", app().settings.computer_use.port) }

pub fn problem() -> Option<String> { RUNTIME.lock().unwrap().as_ref().and_then(|r| r.problem.clone()) }

pub fn call_log() -> Vec<CallLogEntry> {
    RUNTIME.lock().unwrap().as_ref().and_then(|r| r.service.as_ref().map(|s| s.call_log.entries())).unwrap_or_default()
}

pub fn set_server(on: bool) { app().update(|s| s.computer_use.server_enabled = on); sync(); }

pub fn set_gadget(gadget: &str, on: bool) { app().update(|s| s.computer_use.set_gadget_enabled(gadget, on)); sync(); }

// MARK: keys (UI thread)

pub fn create_key(label: &str) -> Result<(), String> {
    let k = keys().create(label).map_err(|e| e.message().to_string())?;
    *ISSUED.lock().unwrap() = Some(k);
    app().changed();
    Ok(())
}

pub fn revoke_key(id: &str) { keys().revoke(id); app().changed(); }

pub fn issued() -> Option<IssuedKey> { ISSUED.lock().unwrap().clone() }

/// Forgets the key shown once (Done, or leaving the page).
pub fn forget_issued() { *ISSUED.lock().unwrap() = None; }

/// Copies the new key; the clipboard is cleared after 60 seconds if it still holds it (macOS does the same).
pub fn copy_issued() {
    let Some(k) = issued() else { return };
    crate::capture::copy_text(&k.token);
    let token = k.token;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        crate::pet_exec::hop(Box::new(move || {
            if crate::pet_chat::clipboard_text().as_deref() == Some(token.as_str()) { clear_clipboard(); }
        }));
    });
}

fn clear_clipboard() {
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard};
    unsafe { if OpenClipboard(Some(app().main)).is_ok() { let _ = EmptyClipboard(); let _ = CloseClipboard(); } }
}

/// The client setup shown on Settings ▸ MCP (the key stays a placeholder).
pub fn client_example() -> String {
    format!("{{\n  \"mcpServers\": {{\n    \"nachocombo\": {{\n      \"type\": \"http\",\n      \"url\": \"{}\",\n      \"headers\": {{ \"Authorization\": \"Bearer <your key>\" }}\n    }}\n  }}\n}}", url())
}

// MARK: approval cards

/// Shows the card on the pet's chat (UI thread) or refuses at once when the pet is not there.
struct PetApprover;

impl McpApproving for PetApprover {
    fn ask(&self, request: McpApprovalRequest, done: Done) {
        let slot = Arc::new(Mutex::new(Some((request, done))));
        let s = slot.clone();
        let posted = crate::pet_exec::hop(Box::new(move || {
            let Some((request, done)) = s.lock().unwrap().take() else { return };
            if !crate::pet_ui::is_running() { return done(McpApproval::Unavailable); }
            PENDING.lock().unwrap().push(Pending { request, done: Some(done), answering: false });
            crate::pet_ui::open_chat();
            crate::pet_ui::invalidate_chat();
        }));
        if !posted { if let Some((_, d)) = slot.lock().unwrap().take() { d(McpApproval::Unavailable) } }
    }

    fn cancel(&self, id: u64) {
        PENDING.lock().unwrap().retain(|p| p.request.id != id);
        crate::pet_exec::hop(Box::new(crate::pet_ui::invalidate_chat));
    }
}

/// The cards for the chat's list (after the conversation).
pub fn approval_items() -> Vec<PetChatItem> {
    PENDING.lock().unwrap().iter().map(|p| PetChatItem::Card(McpCards::item(&p.request, p.answering))).collect()
}

/// `nachopet://mcpapprove?id=…&a=allow|deny`.
pub fn answer(id: u64, allow: bool) {
    let done = {
        let mut q = PENDING.lock().unwrap();
        let Some(i) = q.iter().position(|p| p.request.id == id) else { return };
        q.remove(i).done
    };
    if let Some(d) = done { d(if allow { McpApproval::Allowed } else { McpApproval::Denied }) }
    crate::pet_ui::invalidate_chat();
}

/// The pet went away: nobody can answer the cards.
pub fn pet_stopped() {
    let gone: Vec<Pending> = std::mem::take(&mut *PENDING.lock().unwrap());
    for mut p in gone { if let Some(d) = p.done.take() { d(McpApproval::Unavailable) } }
}

// MARK: state and debug control

/// For `state.json`: counters only (no keys, no arguments).
pub fn diagnostics() -> Value {
    let s = &app().settings.computer_use;
    let rt = RUNTIME.lock().unwrap();
    json!({
        "serverEnabled": s.server_enabled,
        "open": rt.as_ref().is_some_and(|r| r.server.is_some()),
        "address": rt.as_ref().and_then(|r| r.server.as_ref().map(|s| s.local_addr().to_string())),
        "connections": rt.as_ref().and_then(|r| r.server.as_ref().map(|s| s.open_connections())).unwrap_or(0),
        "problem": rt.as_ref().and_then(|r| r.problem.clone()),
        "gadgets": s.enabled_gadgets.iter().filter(|(_, on)| **on).map(|(g, _)| g.clone()).collect::<Vec<_>>(),
        "keys": keys().records().len(),
        "pendingApprovals": PENDING.lock().unwrap().len(),
        "service": rt.as_ref().and_then(|r| r.service.as_ref().map(|s| s.diagnostics())),
    })
}

/// `ctl mcp server on|off`, `ctl mcp gadget <id> on|off`, `ctl mcp key create <label>|revoke <id>`,
/// `ctl mcp approve <id> allow|deny`, `ctl mcp log`. Results go to `mcp-ctl.json` in the app's folder. Only with
/// `--debug-ctl`: `key create` writes the new key there once so a test client can use it (delete the file after).
pub fn ctl(a: &[String]) {
    let arg = |i: usize| a.get(i).map(String::as_str).unwrap_or("");
    let out = match arg(0) {
        "server" => { set_server(arg(1) == "on"); diagnostics() }
        "gadget" => { set_gadget(arg(1), arg(2) == "on"); diagnostics() }
        "key" => match arg(1) {
            "create" => match keys().create(&a[2..].join(" ")) {
                Ok(k) => { app().changed(); json!({"id": k.record.id, "label": k.record.label, "token": k.token}) }
                Err(e) => json!({"error": e.message()}),
            },
            "revoke" => json!({"revoked": keys().revoke(arg(2))}),
            _ => json!({"keys": keys().records().iter().map(|k| json!({"id": k.id, "label": k.label, "createdAt": k.created_at, "lastUsedAt": k.last_used_at})).collect::<Vec<_>>()}),
        },
        "approve" => { if let Ok(id) = arg(1).parse() { answer(id, arg(2) == "allow"); } diagnostics() }
        "log" => json!({"calls": call_log().iter().map(|e| json!({"time": e.time, "tool": e.tool, "key": e.key_label, "outcome": e.outcome.raw_value(), "highRisk": e.high_risk})).collect::<Vec<_>>()}),
        _ => diagnostics(),
    };
    let _ = std::fs::write(app().dir.join("mcp-ctl.json"), serde_json::to_vec_pretty(&out).unwrap_or_default());
}
