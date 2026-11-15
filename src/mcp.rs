//! NachoCombo MCP in the app: the server's lifetime (Settings ▸ MCP switch), its API keys, and the approval card on the
//! pet's chat. The protocol, the checks and the socket are `core::mcp` (tested on Linux); this file connects them to
//! the gadgets (the connector's executor, `pet_exec`), to the pet (cards) and to the settings.
//!
//! - The port is open only while Settings ▸ MCP ▸ the server switch is on, on 127.0.0.1. Switching it off (or the
//!   port, or quitting) closes the listening socket, every open connection and the server's threads.
//! - What the connection threads read (settings, which gadgets run, "always allow") is a snapshot the UI thread
//!   refreshes on every change ([`sync`]); they never touch the app's state.
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
        Arc::new(KeyStore::new(Some(KeyStore::path_in(&app().dir)), Arc::new(crate::pet_win::os_random), crate::core::mcp::limits::system_clock()))
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
    match McpServer::start(service.clone(), port) {
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
