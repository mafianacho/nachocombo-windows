//! The NachoCombo MCP endpoint without any networking (port of `ComputerUseService.swift`): `handle` turns one parsed
//! HTTP request into one HTTP response. Order of checks, every step fails closed:
//! running (503) → path `/mcp` (404) → peer is loopback (403, before the key check) → Host / Origin (403) → client
//! locked out (429) → Bearer key (401) → per-client rate → per-key rate (429) → POST (405) → JSON-RPC → gadget's MCP
//! switch → **approval in Nacho Combo** (Windows) → the executor the connector uses (60 s limit).
//!
//! Windows differs from macOS in one step: a tool that the connector would declare with `"approval": "required"`
//! (destructive always; content_out / input_inject unless the gadget is "always allowed") is not run for an MCP
//! client until the user allows it on the pet's approval card. macOS runs MCP calls under the switches alone.
//!
//! Computer Use (experimental, W5d) sits on the same endpoint, as on macOS: its tools (`computer_<name>`, the old
//! `<name>` accepted) are listed only while Computer Use and their tool group are on and the emergency stop has not
//! fired. A Computer Use call goes: emergency stop → Computer Use on → group switch → closed argument check →
//! **approval card, every call** (Windows; "always allow" never lifts it) → emergency stop again → the backend, one
//! call at a time.
//!
//! The call log keeps the name the client used, the key's label, the time and the outcome — never arguments or results.
use crate::core::computer::tools::{ComputerTool, ComputerTools};
use crate::core::computer::{ComputerBackend, EmergencyStop, ToolResult};
use super::http::{HttpRequest, HttpResponse};
use super::jsonrpc::{cut, JsonRpc, RpcError, RpcInbound, RpcParsed};
use super::keys::{KeyRecord, KeyStore};
use super::limits::{AuthFailureLimiter, Clock, RequestRateLimiter};
use super::policy::NetworkPolicy;
use super::settings::McpSettings;
use super::tools::{McpTool, McpTools};
use crate::core::pet::commands::{PetCommandOutcome, PetCommands};
use crate::core::pet::connector_client::PetCommandExecuting;
use crate::core::pet::gadget_guide::{PetGadgetGuides, PetGuideExit};
use crate::core::pet::gadget_tools::{PetGadgetTools, PetToolRisk};
use crate::core::settings::ModuleId;
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallOutcome { Ok, Error, Denied }

impl CallOutcome {
    pub fn raw_value(self) -> &'static str { match self { Self::Ok => "ok", Self::Error => "error", Self::Denied => "denied" } }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CallLogEntry {
    pub time: f64,
    pub tool: String,
    pub key_label: String,
    pub outcome: CallOutcome,
    pub high_risk: bool,
}

/// The last 200 calls, in memory only.
#[derive(Default)]
pub struct CallLog { entries: Mutex<VecDeque<CallLogEntry>> }

impl CallLog {
    pub const CAPACITY: usize = 200;
    pub fn entries(&self) -> Vec<CallLogEntry> { self.entries.lock().unwrap().iter().cloned().collect() }
    pub fn clear(&self) { self.entries.lock().unwrap().clear(); }
    fn append(&self, e: CallLogEntry) {
        let mut q = self.entries.lock().unwrap();
        q.push_back(e);
        while q.len() > Self::CAPACITY { q.pop_front(); }
    }
}

/// One call waiting for the user (what the pet's approval card shows). No arguments: the card names the tool, the
/// gadget and the key, never what would be read or changed.
#[derive(Clone, Debug, PartialEq)]
pub struct McpApprovalRequest {
    pub id: u64,
    /// MCP name the client used.
    pub tool: String,
    pub connector: String,
    pub gadget: String,
    pub risk: PetToolRisk,
    pub key_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpApproval {
    Allowed,
    Denied,
    /// No card can be shown (Nacho Pet is off or Nacho Combo is closing).
    Unavailable,
}

/// The shell shows the approval card (Windows: the pet's chat) and calls `done` once. `cancel` takes a card away that
/// nobody answered in time.
pub trait McpApproving: Send + Sync {
    fn ask(&self, request: McpApprovalRequest, done: Box<dyn FnOnce(McpApproval) + Send>);
    fn cancel(&self, id: u64);
}

/// Where the service reads what can change while it runs.
#[derive(Clone)]
pub struct McpSources {
    pub settings: Arc<dyn Fn() -> McpSettings + Send + Sync>,
    /// Gadgets set to "always allow" (`gadget-trust.json`).
    pub always_allowed: Arc<dyn Fn() -> BTreeSet<String> + Send + Sync>,
    /// Whether a gadget's module runs right now.
    pub module_on: Arc<dyn Fn(ModuleId) -> bool + Send + Sync>,
}

/// What Computer Use needs from the shell.
#[derive(Clone)]
pub struct ComputerParts {
    pub backend: Arc<dyn ComputerBackend>,
    pub stop: Arc<EmergencyStop>,
    /// Err(reason) while Computer Use cannot be served (the emergency stop shortcut could not be registered).
    pub ready: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}

pub struct McpService {
    pub call_log: Arc<CallLog>,
    computer: Option<ComputerParts>,
    execution: Mutex<()>,
    sources: McpSources,
    keys: Arc<KeyStore>,
    executor: Arc<dyn PetCommandExecuting>,
    approver: Arc<dyn McpApproving>,
    tools: Vec<McpTool>,
    failures: AuthFailureLimiter,
    requests: RequestRateLimiter,
    client_requests: RequestRateLimiter,
    now: Clock,
    accepting: AtomicBool,
    next_approval: AtomicU64,
    auth_failures: AtomicU64,
    tool_calls: AtomicU64,
    pub run_timeout: Duration,
    pub approval_timeout: Duration,
}

impl McpService {
    pub const PROTOCOL_VERSIONS: [&'static str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
    pub const ENDPOINT: &'static str = "/mcp";
    pub const SERVER_NAME: &'static str = "NachoCombo MCP";
    pub const INSTRUCTIONS: &'static str = "NachoCombo MCP: tools of the Nacho Combo gadgets on this Windows PC. Only the tools of the gadgets the user turned on for MCP are listed. \
        Tools that close apps or read content (such as clipboard text) wait until the user allows the call in Nacho Combo; a declined call answers ok:false with reason approval_denied. \
        Computer Use (experimental, computer_* tools) is listed only for the tool groups the user turned on, and every Computer Use call waits for the user's approval. \
        Window positions and sizes are screen pixels.";

    pub fn new(sources: McpSources, keys: Arc<KeyStore>, executor: Arc<dyn PetCommandExecuting>, approver: Arc<dyn McpApproving>, now: Clock) -> Self {
        Self {
            call_log: Arc::new(CallLog::default()),
            computer: None,
            execution: Mutex::new(()),
            sources,
            keys,
            executor,
            approver,
            tools: McpTools::all(),
            failures: AuthFailureLimiter::new(now.clone()),
            requests: RequestRateLimiter::new(now.clone()),
            client_requests: RequestRateLimiter::new(now.clone()),
            now,
            accepting: AtomicBool::new(true),
            next_approval: AtomicU64::new(1),
            auth_failures: AtomicU64::new(0),
            tool_calls: AtomicU64::new(0),
            run_timeout: Duration::from_secs(60),
            approval_timeout: Duration::from_secs(60),
        }
    }

    /// Replaces the request limits (tests).
    pub fn with_limits(mut self, failures: AuthFailureLimiter, requests: RequestRateLimiter, client_requests: RequestRateLimiter) -> Self {
        self.failures = failures;
        self.requests = requests;
        self.client_requests = client_requests;
        self
    }

    /// Adds Computer Use (its tools still need its switch and their group switches).
    pub fn with_computer(mut self, parts: ComputerParts) -> Self { self.computer = Some(parts); self }

    pub fn tools(&self) -> &[McpTool] { &self.tools }

    fn stopped(&self) -> bool { self.computer.as_ref().is_some_and(|c| c.stop.is_active()) }

    /// The Computer Use tools `tools/list` offers right now.
    pub fn offered_computer(&self, settings: &McpSettings) -> Vec<&'static ComputerTool> {
        let Some(c) = &self.computer else { return vec![] };
        if c.stop.is_active() || (c.ready)().is_err() { return vec![]; }
        let groups = settings.served_groups();
        ComputerTools::ALL.iter().filter(|t| t.is_available() && groups.contains(&t.group)).collect()
    }

    /// After this every request is refused with 503 (the server is being switched off).
    pub fn stop(&self) { self.accepting.store(false, Ordering::SeqCst); }

    /// Counters only: no keys, no content.
    pub fn diagnostics(&self) -> Value {
        json!({"accepting": self.accepting.load(Ordering::SeqCst), "emergencyStopped": self.stopped(), "authFailures": self.auth_failures.load(Ordering::SeqCst),
               "toolCalls": self.tool_calls.load(Ordering::SeqCst), "activeKeys": self.keys.records().len()})
    }

    /// The tools `tools/list` offers right now: the gadget's MCP switch is on and, for a tool that needs its module,
    /// the gadget itself is on.
    pub fn offered(&self, settings: &McpSettings) -> Vec<&McpTool> {
        // Nothing at all while the emergency stop holds (macOS: the server is closed then).
        if self.stopped() { return vec![]; }
        self.tools.iter().filter(|t| settings.is_gadget_enabled(&t.gadget) && self.module_ready(t)).collect()
    }

    fn module_ready(&self, t: &McpTool) -> bool { t.requires.is_none_or(|m| (self.sources.module_on)(m)) }

    // MARK: HTTP

    /// `client` is the peer address of the connection as text.
    pub fn handle(&self, request: &HttpRequest, client: &str) -> HttpResponse {
        let client = NetworkPolicy::normalize_client(client);
        if !self.accepting.load(Ordering::SeqCst) { return HttpResponse::error(503, "service stopped"); }
        if request.path != Self::ENDPOINT { return HttpResponse::error(404, "not found"); }
        // Outsiders learn nothing: no key check, no failure bookkeeping.
        if !NetworkPolicy::is_loopback(&client) { return HttpResponse::error(403, "forbidden client address"); }
        let host_ok = request.header("host").is_some_and(NetworkPolicy::is_host_allowed);
        let origin_ok = request.header("origin").is_none_or(NetworkPolicy::is_origin_allowed);
        if !host_ok || !origin_ok { return HttpResponse::error(403, "forbidden host or origin"); }
        if let Some(wait) = self.failures.retry_after(&client) {
            return HttpResponse::error_with(429, "too many failed attempts", vec![("Retry-After".into(), (wait.ceil() as u64).to_string())]);
        }
        let Some(key) = Self::bearer(request).and_then(|t| self.keys.authenticate(t)) else {
            self.failures.record_failure(&client);
            self.auth_failures.fetch_add(1, Ordering::SeqCst);
            return HttpResponse::error_with(401, "unauthorized", vec![("WWW-Authenticate".into(), "Bearer".into())]);
        };
        self.failures.record_success(&client);
        if !self.client_requests.allow(&client) {
            return HttpResponse::error_with(429, "rate limit exceeded", vec![("Retry-After".into(), self.client_requests.retry_after_seconds().to_string())]);
        }
        if !self.requests.allow(&key.id) {
            return HttpResponse::error_with(429, "rate limit exceeded", vec![("Retry-After".into(), self.requests.retry_after_seconds().to_string())]);
        }
        if request.method != "POST" { return HttpResponse::error_with(405, "method not allowed", vec![("Allow".into(), "POST".into())]); }
        let settings = (self.sources.settings)();
        let body = |v: &Value| serde_json::to_vec(v).unwrap_or_default();
        match JsonRpc::parse(&request.body) {
            RpcParsed::Invalid { id, error } => HttpResponse::json(400, body(&JsonRpc::error(id.as_ref(), &error)), vec![]),
            RpcParsed::Single(m) => match self.respond(m, &key, &settings) {
                Some(reply) => HttpResponse::json(200, body(&reply), vec![]),
                None => HttpResponse::new(202),
            },
            RpcParsed::Batch(ms) => {
                let replies: Vec<Value> = ms.into_iter().filter_map(|m| self.respond(m, &key, &settings)).collect();
                if replies.is_empty() { HttpResponse::new(202) } else { HttpResponse::json(200, body(&Value::Array(replies)), vec![]) }
            }
        }
    }

    fn bearer(request: &HttpRequest) -> Option<&str> {
        let h = request.header("authorization")?.trim();
        let (scheme, token) = h.split_once(' ')?;
        let token = token.trim();
        (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
    }

    // MARK: JSON-RPC

    /// None = nothing to send back (notification or client reply).
    fn respond(&self, message: RpcInbound, key: &KeyRecord, settings: &McpSettings) -> Option<Value> {
        let (id, method, params) = match message {
            RpcInbound::Response | RpcInbound::Notification { .. } => return None,
            RpcInbound::Invalid { id, error } => return Some(JsonRpc::error(id.as_ref(), &error)),
            RpcInbound::Request { id, method, params } => (id, method, params),
        };
        let ok = |r: Value| Some(JsonRpc::result(&id, r));
        match method.as_str() {
            "initialize" => {
                let asked = params.as_ref().and_then(|p| p["protocolVersion"].as_str());
                let version = asked.filter(|v| Self::PROTOCOL_VERSIONS.contains(v)).unwrap_or(Self::PROTOCOL_VERSIONS[0]);
                ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false, "subscribe": false}, "prompts": {"listChanged": false}},
                    "serverInfo": {"name": Self::SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
                    "instructions": Self::INSTRUCTIONS,
                }))
            }
            "ping" => ok(json!({})),
            "tools/list" => {
                let mut tools: Vec<Value> = self.offered(settings).into_iter().map(McpTool::json).collect();
                tools.extend(self.offered_computer(settings).into_iter().map(ComputerTool::json));
                ok(json!({"tools": tools}))
            }
            "tools/call" => Some(self.call_tool(&id, params.as_ref(), key, settings)),
            // The gadgets' guides (`core::pet::gadget_guide`), with MCP tool names; prompts are not written for Windows.
            "resources/list" => ok(json!({"resources": PetGadgetGuides::GADGETS.iter().filter(|g| PetGadgetGuides::guide(g).is_some()).map(|g| {
                let display = if *g == "combo" { "Nacho Combo" } else { PetGadgetTools::display_name(g) };
                json!({"uri": Self::guide_uri(g), "name": format!("{g}_guide"), "title": format!("{display} guide"),
                       "description": format!("How to use the {display} tools: what they do, order of calls, examples, limits."), "mimeType": "text/markdown"})
            }).collect::<Vec<_>>()})),
            "resources/templates/list" => ok(json!({"resourceTemplates": []})),
            "resources/read" => {
                let Some(uri) = params.as_ref().and_then(|p| p["uri"].as_str()) else { return Some(JsonRpc::error(Some(&id), &RpcError::invalid_params("uri is required"))) };
                let on_mcp: Vec<_> = PetGadgetTools::declared(true).into_iter().filter(|t| t.on_mcp()).collect();
                let text = PetGadgetGuides::GADGETS.iter().find(|g| Self::guide_uri(g) == uri).and_then(|g| PetGadgetGuides::render(g, PetGuideExit::Mcp, &on_mcp));
                match text {
                    Some(t) => ok(json!({"contents": [{"uri": uri, "mimeType": "text/markdown", "text": t}]})),
                    None => Some(JsonRpc::error(Some(&id), &RpcError::new(RpcError::RESOURCE_NOT_FOUND, format!("Resource not found: {}", cut(uri, 100))))),
                }
            }
            "prompts/list" => ok(json!({"prompts": []})),
            "prompts/get" => {
                let Some(name) = params.as_ref().and_then(|p| p["name"].as_str()) else { return Some(JsonRpc::error(Some(&id), &RpcError::invalid_params("name is required"))) };
                Some(JsonRpc::error(Some(&id), &RpcError::invalid_params(format!("Unknown prompt: {}", cut(name, 64)))))
            }
            other => Some(JsonRpc::error(Some(&id), &RpcError::method_not_found(other))),
        }
    }

    pub fn guide_uri(gadget: &str) -> String { format!("nachocombo://gadgets/{gadget}/guide") }

    fn tool_result(outcome: &PetCommandOutcome) -> Value {
        let text = serde_json::to_string(&Value::Object(outcome.result_json())).unwrap_or_else(|_| "{}".into());
        json!({"content": [{"type": "text", "text": text}], "isError": !outcome.ok})
    }

    fn refused(reason: &str, message: &str, hint: &str, guide_ko: &str) -> PetCommandOutcome {
        PetCommandOutcome::refused(reason, message, hint, guide_ko, json!({}))
    }

    fn call_tool(&self, id: &Value, params: Option<&Value>, key: &KeyRecord, settings: &McpSettings) -> Value {
        let err = |e: RpcError| JsonRpc::error(Some(id), &e);
        let Some(name) = params.and_then(|p| p["name"].as_str()) else { return err(RpcError::invalid_params("name is required")) };
        let arguments = match params.and_then(|p| p.get("arguments")) {
            None | Some(Value::Null) => None,
            Some(v @ Value::Object(_)) => Some(v.clone()),
            Some(_) => return err(RpcError::invalid_params("arguments must be an object")),
        };
        let Some(tool) = McpTools::find(name, &self.tools).cloned() else {
            return match ComputerTools::find(name) {
                Some(t) => self.call_computer(id, name, t, arguments.as_ref(), key, settings),
                None => err(RpcError::invalid_params(format!("Unknown tool: {}", cut(name, 64)))),
            };
        };
        self.tool_calls.fetch_add(1, Ordering::SeqCst);
        let record = |outcome: CallOutcome| self.call_log.append(CallLogEntry {
            time: (self.now)(), tool: cut(name, 64), key_label: key.label.clone(), outcome, high_risk: tool.risk == PetToolRisk::Destructive,
        });
        let answer = |outcome: CallOutcome, o: PetCommandOutcome| { record(outcome); JsonRpc::result(id, Self::tool_result(&o)) };

        if self.stopped() { return answer(CallOutcome::Denied, Self::emergency_refusal()); }
        if !settings.is_gadget_enabled(&tool.gadget) {
            let gadget = PetGadgetTools::display_name(&tool.gadget);
            return answer(CallOutcome::Denied, Self::refused("mcp_gadget_off",
                &format!("'{}' tool is turned off. Turn on the {gadget} gadget for NachoCombo MCP in Nacho Combo (Settings ▸ MCP) on this PC.", cut(name, 64)),
                "ask the user to turn the gadget on in Settings ▸ MCP; do not retry until then", "설정 ▸ MCP에서 이 가젯을 켜야 해요"));
        }
        // The gadget itself is off: no card for a call that cannot run.
        if !self.module_ready(&tool) { return answer(CallOutcome::Denied, PetCommandOutcome::gadget_off(&tool.gadget)); }
        // The same closed parameter checks as Nacho's requests.
        let command = match PetCommands::parse_with(&tool.connector, arguments.as_ref(), true) {
            Ok(c) => c,
            Err(p) => return answer(CallOutcome::Error, PetCommandOutcome::new(false, p.message())),
        };
        if McpTools::needs_approval(&tool, &(self.sources.always_allowed)()) {
            match self.ask(&tool, name, key) {
                Some(McpApproval::Allowed) => {}
                Some(McpApproval::Denied) => return answer(CallOutcome::Denied, Self::refused("approval_denied",
                    "The user declined this call in Nacho Combo.", "do not retry unless the user asks again", "사용자가 나초콤보에서 거절했어요")),
                Some(McpApproval::Unavailable) => return answer(CallOutcome::Denied, Self::refused("approval_unavailable",
                    "This tool needs the user's approval in Nacho Combo, and Nacho Pet is not on to ask.",
                    "ask the user to turn on Nacho Pet, then call again", "이 도구는 나초 펫 승인 카드가 있어야 해요. 나초 펫을 켜 주세요")),
                None => return answer(CallOutcome::Denied, Self::refused("approval_timeout",
                    "Nobody answered the approval card in Nacho Combo in time.", "call again when the user is at the PC", "승인 카드에 답이 없어 취소했어요")),
            }
        }
        let (tx, rx) = mpsc::channel();
        self.executor.execute(command, Box::new(move |o| { let _ = tx.send(o); }));
        match rx.recv_timeout(self.run_timeout) {
            Ok(o) => answer(if o.ok { CallOutcome::Ok } else { CallOutcome::Error }, o),
            Err(_) => answer(CallOutcome::Error, Self::refused("timeout", "The tool did not finish in 60 seconds.", "check the state before calling again", "60초 안에 끝나지 않았어요")),
        }
    }

    fn emergency_refusal() -> PetCommandOutcome {
        Self::refused("emergency_stop", "Emergency stop is active. The user has to turn Computer Use and the MCP server back on in Nacho Combo.",
            "do not retry", "비상 정지가 켜져 있어요")
    }

    /// A Computer Use call (see the module comment for the order).
    fn call_computer(&self, id: &Value, name: &str, tool: &'static ComputerTool, arguments: Option<&Value>, key: &KeyRecord, settings: &McpSettings) -> Value {
        self.tool_calls.fetch_add(1, Ordering::SeqCst);
        let record = |outcome: CallOutcome| self.call_log.append(CallLogEntry {
            time: (self.now)(), tool: cut(name, 64), key_label: key.label.clone(), outcome, high_risk: tool.high_risk(),
        });
        let refuse = |o: PetCommandOutcome| { record(CallOutcome::Denied); JsonRpc::result(id, Self::tool_result(&o)) };
        let Some(c) = self.computer.clone() else {
            return refuse(Self::refused("computer_off", "Computer Use is not part of this Nacho Combo.", "do not retry", "컴퓨터 사용이 없어요"));
        };
        if c.stop.is_active() { return refuse(Self::emergency_refusal()); }
        if !settings.computer_enabled {
            return refuse(Self::refused("computer_off", "Computer Use is off. The user turns it on in Nacho Combo (Settings ▸ MCP ▸ Computer Use) on this PC.",
                "ask the user; do not retry until then", "설정 ▸ MCP에서 컴퓨터 사용을 켜야 해요"));
        }
        if let Err(why) = (c.ready)() { return refuse(Self::refused("computer_unavailable", &why, "ask the user to check Settings ▸ MCP", "컴퓨터 사용을 지금 쓸 수 없어요")); }
        if !tool.is_available() {
            return refuse(Self::refused("not_available", &format!("'{}' is not available on Windows.", cut(name, 64)), "use another tool", "Windows에는 없는 도구예요"));
        }
        if !settings.is_group_enabled(tool.group) {
            return refuse(Self::refused("mcp_group_off",
                &format!("'{}' tool is turned off. Turn on the {} tool group in Nacho Combo (Settings ▸ MCP ▸ Computer Use) on this PC.", cut(name, 64), tool.group.display_name()),
                "ask the user to turn the group on; do not retry until then", "설정 ▸ MCP에서 이 도구 묶음을 켜야 해요"));
        }
        let args = match ComputerTools::check_arguments(tool.name, arguments) {
            Ok(a) => a,
            Err(e) => { record(CallOutcome::Error); return JsonRpc::result(id, ToolResult::error(format!("bad parameters: {e}")).json()); }
        };
        let generation = c.stop.generation();
        let request = |rid| McpApprovalRequest { id: rid, tool: cut(name, 64), connector: tool.connector_name(), gadget: "computer".into(), risk: tool.risk, key_label: key.label.clone() };
        match self.ask_request(request) {
            Some(McpApproval::Allowed) => {}
            Some(McpApproval::Denied) => return refuse(Self::refused("approval_denied", "The user declined this call in Nacho Combo.", "do not retry unless the user asks again", "사용자가 나초콤보에서 거절했어요")),
            Some(McpApproval::Unavailable) => return refuse(Self::refused("approval_unavailable",
                "Computer Use calls need the user's approval in Nacho Combo, and Nacho Pet is not on to ask.", "ask the user to turn on Nacho Pet, then call again", "이 도구는 나초 펫 승인 카드가 있어야 해요. 나초 펫을 켜 주세요")),
            None => return refuse(Self::refused("approval_timeout", "Nobody answered the approval card in Nacho Combo in time.", "call again when the user is at the PC", "승인 카드에 답이 없어 취소했어요")),
        }
        // One call at a time; the stop may have fired while the card waited or another call ran.
        let _turn = self.execution.lock().unwrap_or_else(|e| e.into_inner());
        if c.stop.is_active() || c.stop.generation() != generation { return refuse(Self::emergency_refusal()); }
        let result = c.backend.execute(tool.name, &args);
        record(if result.is_error { CallOutcome::Error } else { CallOutcome::Ok });
        JsonRpc::result(id, result.json())
    }

    /// None: nobody answered in time (the card is taken away).
    fn ask(&self, tool: &McpTool, name: &str, key: &KeyRecord) -> Option<McpApproval> {
        self.ask_request(|id| McpApprovalRequest { id, tool: cut(name, 64), connector: tool.connector.clone(), gadget: tool.gadget.clone(), risk: tool.risk, key_label: key.label.clone() })
    }

    fn ask_request(&self, make: impl FnOnce(u64) -> McpApprovalRequest) -> Option<McpApproval> {
        let id = self.next_approval.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        let request = make(id);
        self.approver.ask(request, Box::new(move |a| { let _ = tx.send(a); }));
        match rx.recv_timeout(self.approval_timeout) {
            Ok(a) => Some(a),
            Err(_) => { self.approver.cancel(id); None }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::core::mcp::keys::tests::counting;
    use crate::core::mcp::limits::tests::FakeClock;
    use crate::core::pet::commands::PetCommand;
    use std::collections::BTreeMap;

    /// Runs nothing for real: answers with the command's debug name and keeps what it ran.
    #[derive(Default)]
    pub struct FakeExecutor { pub ran: Mutex<Vec<String>> }
    impl PetCommandExecuting for FakeExecutor {
        fn execute(&self, command: PetCommand, done: Box<dyn FnOnce(PetCommandOutcome) + Send>) {
            let name = format!("{command:?}");
            self.ran.lock().unwrap().push(name.clone());
            let mut o = PetCommandOutcome::new(true, "done");
            o.details = Some(serde_json::from_value(json!({"ran": name})).unwrap());
            done(o)
        }
    }

    /// Answers every card with a fixed choice (None: never answers).
    pub struct FakeApprover { pub answer: Mutex<Option<McpApproval>>, pub asked: Mutex<Vec<McpApprovalRequest>>, pub cancelled: Mutex<Vec<u64>> }
    impl FakeApprover {
        pub fn new(answer: Option<McpApproval>) -> Self { Self { answer: Mutex::new(answer), asked: Mutex::new(vec![]), cancelled: Mutex::new(vec![]) } }
    }
    impl McpApproving for FakeApprover {
        fn ask(&self, request: McpApprovalRequest, done: Box<dyn FnOnce(McpApproval) + Send>) {
            self.asked.lock().unwrap().push(request);
            if let Some(a) = *self.answer.lock().unwrap() { done(a) }
        }
        fn cancel(&self, id: u64) { self.cancelled.lock().unwrap().push(id); }
    }

    pub struct Rig {
        pub service: McpService,
        pub settings: Arc<Mutex<McpSettings>>,
        pub trust: Arc<Mutex<BTreeSet<String>>>,
        /// Modules that run (all, unless a test switches one off).
        pub modules: Arc<Mutex<BTreeSet<ModuleId>>>,
        pub executor: Arc<FakeExecutor>,
        pub approver: Arc<FakeApprover>,
        pub token: String,
        pub clock: FakeClock,
    }

    pub fn rig(answer: Option<McpApproval>) -> Rig {
        let clock = FakeClock::new(1_800_000_000.0);
        let keys = Arc::new(KeyStore::new(None, counting(), clock.clock()));
        let token = keys.create("test client").unwrap().token;
        let settings = Arc::new(Mutex::new(McpSettings { server_enabled: true, ..Default::default() }));
        let trust = Arc::new(Mutex::new(BTreeSet::new()));
        let executor = Arc::new(FakeExecutor::default());
        let approver = Arc::new(FakeApprover::new(answer));
        let (s, t) = (settings.clone(), trust.clone());
        let modules: Arc<Mutex<BTreeSet<ModuleId>>> = Arc::new(Mutex::new(ModuleId::ALL.into_iter().collect()));
        let m = modules.clone();
        let sources = McpSources { settings: Arc::new(move || s.lock().unwrap().clone()), always_allowed: Arc::new(move || t.lock().unwrap().clone()),
            module_on: Arc::new(move |id| m.lock().unwrap().contains(&id)) };
        let mut service = McpService::new(sources, keys, executor.clone(), approver.clone(), clock.clock());
        service.approval_timeout = Duration::from_millis(50);
        Rig { service, settings, trust, modules, executor, approver, token, clock }
    }

    impl Rig {
        pub fn enable(&self, gadget: &str) { self.settings.lock().unwrap().set_gadget_enabled(gadget, true); }
        pub fn post(&self, body: Value) -> HttpResponse { self.send("POST", "/mcp", &[("Host", "127.0.0.1:47653"), ("Authorization", &format!("Bearer {}", self.token))], body, "127.0.0.1") }
        pub fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Value, client: &str) -> HttpResponse {
            let headers: BTreeMap<String, String> = headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
            let request = HttpRequest { method: method.into(), path: path.into(), headers, body: serde_json::to_vec(&body).unwrap() };
            self.service.handle(&request, client)
        }
        pub fn rpc(&self, method: &str, params: Value) -> Value {
            let r = self.post(json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}));
            assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
            serde_json::from_slice(&r.body).unwrap()
        }
        /// tools/call → the JSON inside the text content.
        pub fn call(&self, name: &str, arguments: Value) -> (Value, bool) {
            let v = self.rpc("tools/call", json!({"name": name, "arguments": arguments}));
            let r = &v["result"];
            (serde_json::from_str(r["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{v}"))).unwrap(), r["isError"].as_bool().unwrap())
        }
        pub fn listed(&self) -> Vec<String> {
            self.rpc("tools/list", json!({}))["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
        }
    }

    #[test]
    fn initialize_names_the_server_and_picks_a_known_version() {
        let r = rig(None);
        let v = r.rpc("initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
        assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(v["result"]["serverInfo"]["name"], "NachoCombo MCP");
        assert!(v["result"]["capabilities"]["tools"].is_object());
        let v = r.rpc("initialize", json!({"protocolVersion": "1999-01-01"}));
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r.rpc("ping", json!(null))["result"], json!({}));
    }

    #[test]
    fn requests_are_checked_in_order_and_fail_closed() {
        let r = rig(None);
        let auth = format!("Bearer {}", r.token);
        let ok_headers = [("Host", "127.0.0.1:47653"), ("Authorization", auth.as_str())];
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
        assert_eq!(r.send("POST", "/other", &ok_headers, body.clone(), "127.0.0.1").status, 404);
        // Not loopback: refused before the key check (no 401 hint, no failure counted).
        assert_eq!(r.send("POST", "/mcp", &ok_headers, body.clone(), "192.168.0.5").status, 403);
        assert_eq!(r.send("POST", "/mcp", &[("Host", "evil.example"), ("Authorization", auth.as_str())], body.clone(), "127.0.0.1").status, 403);
        assert_eq!(r.send("POST", "/mcp", &[("Host", "localhost"), ("Origin", "https://evil.example"), ("Authorization", auth.as_str())], body.clone(), "127.0.0.1").status, 403);
        assert_eq!(r.send("POST", "/mcp", &[("Host", "localhost")], body.clone(), "127.0.0.1").status, 401);
        let wrong = r.send("POST", "/mcp", &[("Host", "localhost"), ("Authorization", "Bearer nccu_nope")], body.clone(), "127.0.0.1");
        assert_eq!(wrong.status, 401);
        assert_eq!(wrong.header("WWW-Authenticate"), Some("Bearer"));
        assert_eq!(r.send("GET", "/mcp", &ok_headers, body.clone(), "127.0.0.1").status, 405);
        assert_eq!(r.send("POST", "/mcp", &ok_headers, body.clone(), "::1").status, 200);
        assert_eq!(r.service.diagnostics()["authFailures"], 2);
        r.service.stop();
        assert_eq!(r.send("POST", "/mcp", &ok_headers, body, "127.0.0.1").status, 503);
    }

    #[test]
    fn five_wrong_keys_lock_the_client_out_even_with_the_right_key() {
        let r = rig(None);
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
        for _ in 0..5 { assert_eq!(r.send("POST", "/mcp", &[("Host", "localhost"), ("Authorization", "Bearer nccu_x")], body.clone(), "127.0.0.1").status, 401); }
        let locked = r.post(body.clone());
        assert_eq!(locked.status, 429);
        assert_eq!(locked.header("Retry-After"), Some("60"));
        r.clock.advance(61.0);
        assert_eq!(r.post(body).status, 200);
    }

    #[test]
    fn per_client_and_per_key_rate_limits() {
        let mut r = rig(None);
        let c = r.clock.clock();
        r.service = r.service.with_limits(AuthFailureLimiter::new(c.clone()), RequestRateLimiter::with(3, 60.0, c.clone()), RequestRateLimiter::with(100, 60.0, c));
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
        for _ in 0..3 { assert_eq!(r.post(body.clone()).status, 200); }
        let limited = r.post(body);
        assert_eq!(limited.status, 429);
        assert_eq!(limited.header("Retry-After"), Some("60"));
    }

    #[test]
    fn json_rpc_errors_batches_and_notifications() {
        let r = rig(None);
        let bad = r.post(json!("not an object"));
        assert_eq!(bad.status, 400);
        let v: Value = serde_json::from_slice(&r.post(json!({"jsonrpc": "2.0", "id": 1, "method": "nope"})).body).unwrap();
        assert_eq!(v["error"]["code"], -32601);
        assert_eq!(r.post(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).status, 202);
        let batch: Value = serde_json::from_slice(&r.post(json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}, {"jsonrpc": "2.0", "method": "x"}, {"jsonrpc": "2.0", "id": 2, "method": "ping"}])).body).unwrap();
        assert_eq!(batch.as_array().unwrap().len(), 2);
        let v = r.rpc("resources/read", json!({"uri": "nachocombo://gadgets/nope/guide"}));
        assert_eq!(v["error"]["code"], -32002);
        assert_eq!(r.rpc("resources/read", json!({}))["error"]["code"], -32602);
        assert_eq!(r.rpc("prompts/list", json!({}))["result"]["prompts"], json!([]));
        assert_eq!(r.rpc("prompts/get", json!({"name": "window_side_by_side"}))["error"]["code"], -32602);
    }

    #[test]
    fn guides_are_resources_with_mcp_tool_names() {
        let r = rig(None);
        let list = r.rpc("resources/list", json!({}));
        let uris: Vec<&str> = list["result"]["resources"].as_array().unwrap().iter().map(|g| g["uri"].as_str().unwrap()).collect();
        assert!(uris.contains(&"nachocombo://gadgets/window/guide") && uris.contains(&"nachocombo://gadgets/clipboard/guide"), "{uris:?}");
        assert!(!uris.iter().any(|u| u.contains("/computer/")));
        let v = r.rpc("resources/read", json!({"uri": "nachocombo://gadgets/window/guide"}));
        let c = &v["result"]["contents"][0];
        assert_eq!(c["mimeType"], "text/markdown");
        let text = c["text"].as_str().unwrap();
        assert!(text.contains("`window_place`") && !text.contains("`combo_window_place`"), "MCP names: {text}");
    }

    #[test]
    fn tools_list_shows_only_gadgets_switched_on_for_mcp() {
        let r = rig(None);
        assert!(r.listed().is_empty(), "every gadget switch is off by default");
        r.enable("dim");
        let listed = r.listed();
        assert_eq!(listed, vec!["dim_set_strength", "dim_status", "dim_pause", "dim_resume"]);
        r.enable("clipboard");
        assert!(r.listed().contains(&"clipboard_get_item".to_string()));
        r.settings.lock().unwrap().set_gadget_enabled("dim", false);
        assert!(!r.listed().iter().any(|n| n.starts_with("dim_")));
        let tools = &r.rpc("tools/list", json!({}))["result"]["tools"];
        assert!(tools[0]["description"].as_str().unwrap().len() > 10);
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn tools_of_a_gadget_that_is_off_are_not_listed_and_answer_gadget_off() {
        let r = rig(Some(McpApproval::Allowed));
        r.enable("clipboard");
        r.enable("dim");
        r.modules.lock().unwrap().remove(&ModuleId::Clipboard);
        r.modules.lock().unwrap().remove(&ModuleId::FocusDim);
        let listed = r.listed();
        assert!(!listed.iter().any(|n| n.starts_with("clipboard_")), "{listed:?}");
        // The first contract works with the gadget off, as on Nacho's side.
        assert_eq!(listed, vec!["dim_set_strength"]);
        let (v, is_error) = r.call("clipboard_get_item", json!({"index": 1}));
        assert!(is_error);
        assert_eq!(v["reason"], "gadget_off");
        assert!(r.approver.asked.lock().unwrap().is_empty(), "no card for a call that cannot run");
        assert!(r.executor.ran.lock().unwrap().is_empty());
    }

    #[test]
    fn a_switched_off_gadget_refuses_the_call_and_unknown_tools_are_errors() {
        let r = rig(None);
        let (v, is_error) = r.call("dim_status", json!({}));
        assert!(is_error);
        assert_eq!(v["reason"], "mcp_gadget_off");
        assert!(r.executor.ran.lock().unwrap().is_empty());
        let e = r.rpc("tools/call", json!({"name": "shelf_clear"}));
        assert_eq!(e["error"]["code"], -32602, "Windows declares no deleting tool, on either exit");
        let e = r.rpc("tools/call", json!({"name": "dim_status", "arguments": [1]}));
        assert_eq!(e["error"]["code"], -32602);
        assert_eq!(r.service.call_log.entries()[0].outcome, CallOutcome::Denied);
    }

    #[test]
    fn read_and_change_tools_run_at_once_with_closed_parameters() {
        let r = rig(None);
        r.enable("dim");
        let (v, is_error) = r.call("dim_status", json!({}));
        assert!(!is_error);
        assert_eq!(v["ok"], true);
        assert_eq!(v["ran"], "DimStatus");
        let (v, is_error) = r.call("dim_set_strength", json!({"percent": 40, "extra": 1}));
        assert!(is_error, "{v}");
        assert!(v["message"].as_str().unwrap().contains("bad parameters"));
        assert!(r.approver.asked.lock().unwrap().is_empty(), "no card for read / change");
    }

    #[test]
    fn content_out_and_destructive_wait_for_the_card() {
        let r = rig(Some(McpApproval::Allowed));
        r.enable("clipboard");
        r.enable("appswitch");
        let (v, is_error) = r.call("clipboard_get_item", json!({"index": 1}));
        assert!(!is_error, "{v}");
        let asked = r.approver.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].tool, "clipboard_get_item");
        assert_eq!(asked[0].gadget, "clipboard");
        assert_eq!(asked[0].key_label, "test client");
        // Always allow lifts content_out, never destructive.
        r.trust.lock().unwrap().insert("clipboard".into());
        r.trust.lock().unwrap().insert("appswitch".into());
        r.call("clipboard_get_item", json!({"index": 1}));
        assert_eq!(r.approver.asked.lock().unwrap().len(), 1);
        r.call("appswitch_quit", json!({"app": "Notepad"}));
        assert_eq!(r.approver.asked.lock().unwrap().len(), 2);
        assert_eq!(r.executor.ran.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_declined_missing_or_unanswered_card_runs_nothing() {
        for (answer, reason) in [(Some(McpApproval::Denied), "approval_denied"), (Some(McpApproval::Unavailable), "approval_unavailable"), (None, "approval_timeout")] {
            let r = rig(answer);
            r.enable("clipboard");
            let (v, is_error) = r.call("clipboard_get_item", json!({"index": 1}));
            assert!(is_error);
            assert_eq!(v["reason"], reason);
            assert!(v["hint"].is_string() && v["guide_ko"].is_string());
            assert!(r.executor.ran.lock().unwrap().is_empty(), "{reason}");
            assert_eq!(r.service.call_log.entries().last().unwrap().outcome, CallOutcome::Denied);
            if answer.is_none() { assert_eq!(r.approver.cancelled.lock().unwrap().len(), 1, "the card is taken away"); }
        }
    }

    #[test]
    fn the_call_log_keeps_names_never_arguments() {
        let r = rig(Some(McpApproval::Allowed));
        r.enable("clipboard");
        r.call("clipboard_get_item", json!({"index": 1, "id": "secret12"}));
        let e = r.service.call_log.entries();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].tool, "clipboard_get_item");
        assert_eq!(e[0].key_label, "test client");
        assert!(!format!("{e:?}").contains("secret12"));
        assert!(!e[0].high_risk);
        r.enable("appswitch");
        r.call("appswitch_quit", json!({"app": "x"}));
        assert!(r.service.call_log.entries()[1].high_risk);
    }
}

/// Computer Use on the MCP endpoint (W5d): listing by switch and group, the emergency stop, every call approved,
/// one call at a time, closed arguments and the macOS names.
#[cfg(test)]
mod computer_tests {
    use super::tests::{rig, Rig};
    use super::*;
    use crate::core::computer::tools::ToolGroup;
    use crate::core::computer::ToolContent;
    use serde_json::Map;
    use std::sync::atomic::AtomicBool;

    /// Records what ran; `screen_capture` answers a picture.
    #[derive(Default)]
    struct FakeComputer { ran: Mutex<Vec<String>> }
    impl ComputerBackend for FakeComputer {
        fn execute(&self, tool: &str, arguments: &Map<String, Value>) -> ToolResult {
            self.ran.lock().unwrap().push(format!("{tool} {}", Value::Object(arguments.clone())));
            if tool == "screen_capture" { return ToolResult { content: vec![ToolContent::Image { base64: "iVBORw0KGgo=".into(), mime: "image/png".into() }], is_error: false }; }
            ToolResult::object(json!({"ok": true, "tool": tool}))
        }
    }

    struct CRig { r: Rig, backend: Arc<FakeComputer>, stop: Arc<EmergencyStop>, ready: Arc<Mutex<Result<(), String>>> }

    fn crig(answer: Option<McpApproval>) -> CRig {
        let mut r = rig(answer);
        let backend = Arc::new(FakeComputer::default());
        let stop = Arc::new(EmergencyStop::default());
        let ready = Arc::new(Mutex::new(Ok(())));
        let rd = ready.clone();
        let parts = ComputerParts { backend: backend.clone(), stop: stop.clone(), ready: Arc::new(move || rd.lock().unwrap().clone()) };
        let s = std::mem::replace(&mut r.service, rig(None).service);
        r.service = s.with_computer(parts);
        CRig { r, backend, stop, ready }
    }

    impl CRig {
        fn on(&self, groups: &[ToolGroup]) {
            let mut s = self.r.settings.lock().unwrap();
            s.computer_enabled = true;
            for g in groups { s.set_group_enabled(*g, true); }
        }
        /// tools/call → (result object, isError).
        fn call(&self, name: &str, arguments: Value) -> Value { self.r.rpc("tools/call", json!({"name": name, "arguments": arguments}))["result"].clone() }
        fn text(v: &Value) -> Value { serde_json::from_str(v["content"][0]["text"].as_str().unwrap()).unwrap_or(Value::Null) }
        fn ran(&self) -> Vec<String> { self.backend.ran.lock().unwrap().clone() }
    }

    #[test]
    fn nothing_is_listed_until_computer_use_and_a_group_are_on() {
        let c = crig(Some(McpApproval::Allowed));
        assert!(!c.r.listed().iter().any(|n| n.starts_with("computer_")));
        c.r.settings.lock().unwrap().set_group_enabled(ToolGroup::Screen, true);
        assert!(!c.r.listed().iter().any(|n| n.starts_with("computer_")), "a group alone does nothing while Computer Use is off");
        c.on(&[ToolGroup::Screen]);
        let listed = c.r.listed();
        assert!(listed.contains(&"computer_screen_capture".to_string()) && listed.contains(&"computer_screen_list_displays".to_string()));
        assert!(!listed.contains(&"computer_mouse_click".to_string()), "Input is still off");
        // AppleScript is never listed, even if a settings file says on.
        c.r.settings.lock().unwrap().enabled_groups.insert("appleScript".into(), true);
        assert!(!c.r.listed().contains(&"computer_applescript_run".to_string()));
        // The shortcut could not be registered: Computer Use is not served.
        *c.ready.lock().unwrap() = Err("The emergency stop shortcut Ctrl+Alt+Win+. is taken by another app.".into());
        assert!(!c.r.listed().iter().any(|n| n.starts_with("computer_")));
        let v = c.call("computer_screen_capture", json!({"target": "display"}));
        assert_eq!(CRig::text(&v)["reason"], "computer_unavailable");
    }

    #[test]
    fn every_call_waits_for_the_card_and_runs_once_allowed() {
        let c = crig(Some(McpApproval::Allowed));
        c.on(&[ToolGroup::Screen, ToolGroup::SystemInfo]);
        // Read-only tools ask too ("always allow" never lifts Computer Use).
        c.r.trust.lock().unwrap().insert("computer".into());
        let v = c.call("computer_screen_list_displays", json!({}));
        assert_eq!(v["isError"], false);
        let asked = c.r.approver.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!((asked[0].gadget.as_str(), asked[0].connector.as_str(), asked[0].risk), ("computer", "combo_computer_screen_list_displays", PetToolRisk::Read));
        let v = c.call("computer_screen_capture", json!({"target": "display", "maxWidth": 800}));
        assert_eq!(v["content"][0]["type"], "image");
        assert_eq!(v["content"][0]["mimeType"], "image/png");
        // The old Computer Use name works too.
        let v = c.call("process_list", json!(null));
        assert_eq!(v["isError"], false);
        assert_eq!(c.r.approver.asked.lock().unwrap().len(), 3);
        assert_eq!(c.ran(), vec!["screen_list_displays {}".to_string(), "screen_capture {\"maxWidth\":800,\"target\":\"display\"}".into(), "process_list {}".into()]);
        let log = c.r.service.call_log.entries();
        assert_eq!(log.iter().map(|e| e.tool.as_str()).collect::<Vec<_>>(), vec!["computer_screen_list_displays", "computer_screen_capture", "process_list"]);
        assert!(log.iter().all(|e| e.outcome == CallOutcome::Ok && !e.high_risk));
    }

    #[test]
    fn declined_unavailable_and_unanswered_cards_run_nothing() {
        for (answer, reason) in [(Some(McpApproval::Denied), "approval_denied"), (Some(McpApproval::Unavailable), "approval_unavailable"), (None, "approval_timeout")] {
            let c = crig(answer);
            c.on(&[ToolGroup::Input]);
            let v = c.call("computer_type_text", json!({"text": "hello"}));
            assert_eq!(v["isError"], true);
            assert_eq!(CRig::text(&v)["reason"], reason);
            assert!(c.ran().is_empty(), "{reason}");
            assert_eq!(c.r.service.call_log.entries()[0].outcome, CallOutcome::Denied);
        }
        let c = crig(None);
        c.on(&[ToolGroup::Input]);
        c.call("computer_mouse_move", json!({"x": 1, "y": 2}));
        assert_eq!(c.r.approver.cancelled.lock().unwrap().len(), 1, "the unanswered card is taken away");
    }

    #[test]
    fn switches_and_groups_are_checked_before_the_card() {
        let c = crig(Some(McpApproval::Allowed));
        let v = c.call("computer_mouse_click", json!({"x": 1, "y": 1}));
        assert_eq!(CRig::text(&v)["reason"], "computer_off");
        c.on(&[ToolGroup::Screen]);
        let v = c.call("computer_mouse_click", json!({"x": 1, "y": 1}));
        assert_eq!(CRig::text(&v)["reason"], "mcp_group_off");
        c.r.settings.lock().unwrap().enabled_groups.insert("appleScript".into(), true);
        let v = c.call("computer_applescript_run", json!({"script": "beep"}));
        assert_eq!(CRig::text(&v)["reason"], "not_available");
        assert!(c.r.approver.asked.lock().unwrap().is_empty(), "no card for a call that cannot run");
        assert!(c.ran().is_empty());
    }

    #[test]
    fn arguments_are_closed() {
        let c = crig(Some(McpApproval::Allowed));
        c.on(&[ToolGroup::Input]);
        let v = c.call("computer_mouse_click", json!({"x": 1, "y": 1, "button": "left", "extra": true}));
        assert_eq!(v["isError"], true);
        assert!(v["content"][0]["text"].as_str().unwrap().contains("unknown argument extra"));
        let v = c.call("computer_key_press", json!({}));
        assert!(v["content"][0]["text"].as_str().unwrap().contains("Missing required argument 'key'"));
        assert!(c.r.approver.asked.lock().unwrap().is_empty());
        assert!(c.ran().is_empty());
        let r = c.r.rpc("tools/call", json!({"name": "computer_teleport", "arguments": {}}));
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn the_emergency_stop_refuses_everything_and_lists_nothing() {
        let c = crig(Some(McpApproval::Allowed));
        c.on(&[ToolGroup::Screen, ToolGroup::Shell]);
        c.r.enable("dim");
        assert!(c.r.listed().contains(&"dim_status".to_string()));
        c.stop.fire();
        assert!(c.r.listed().is_empty(), "nothing is listed while the stop holds");
        for (name, args) in [("computer_shell_run", json!({"command": "Get-Date"})), ("dim_status", json!({}))] {
            let v = c.call(name, args);
            assert_eq!(CRig::text(&v)["reason"], "emergency_stop", "{name}");
        }
        assert!(c.ran().is_empty() && c.r.executor.ran.lock().unwrap().is_empty());
        assert!(c.r.approver.asked.lock().unwrap().is_empty());
        assert_eq!(c.r.service.diagnostics()["emergencyStopped"], true);
        c.stop.clear();
        assert_eq!(c.call("computer_screen_list_displays", json!({}))["isError"], false);
    }

    #[test]
    fn a_stop_while_the_card_waits_refuses_the_allowed_call() {
        // The approver allows only after the stop fired (the user pressed the shortcut while the card was open).
        struct LateApprover(Arc<EmergencyStop>);
        impl McpApproving for LateApprover {
            fn ask(&self, _: McpApprovalRequest, done: Box<dyn FnOnce(McpApproval) + Send>) { self.0.fire(); self.0.clear(); done(McpApproval::Allowed) }
            fn cancel(&self, _: u64) {}
        }
        let c = crig(None);
        c.on(&[ToolGroup::Input]);
        let rig2 = rig(None);
        let parts = ComputerParts { backend: c.backend.clone(), stop: c.stop.clone(), ready: Arc::new(|| Ok(())) };
        let sources = McpSources { settings: { let s = c.r.settings.clone(); Arc::new(move || s.lock().unwrap().clone()) }, always_allowed: Arc::new(BTreeSet::new),
                                   module_on: Arc::new(|_| true) };
        let keys = Arc::new(KeyStore::new(None, crate::core::mcp::keys::tests::counting(), c.r.clock.clock()));
        let token = keys.create("t").unwrap().token;
        let svc = McpService::new(sources, keys, rig2.executor.clone(), Arc::new(LateApprover(c.stop.clone())), c.r.clock.clock()).with_computer(parts);
        let body = serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "computer_type_text", "arguments": {"text": "x"}}})).unwrap();
        let headers = [("host".to_string(), "127.0.0.1:47653".to_string()), ("authorization".to_string(), format!("Bearer {token}"))].into_iter().collect();
        let resp = svc.handle(&HttpRequest { method: "POST".into(), path: "/mcp".into(), headers, body }, "127.0.0.1");
        let v: Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(CRig::text(&v["result"])["reason"], "emergency_stop");
        assert!(c.ran().is_empty(), "a stop between the card and the run wins");
    }

    #[test]
    fn high_risk_calls_are_marked_in_the_log() {
        let c = crig(Some(McpApproval::Allowed));
        c.on(&[ToolGroup::Shell, ToolGroup::FileAccess]);
        c.call("computer_shell_run", json!({"command": "Get-Date", "timeoutSeconds": 5}));
        c.call("computer_file_read", json!({"path": "C:\\Users\\me\\a.txt"}));
        let log = c.r.service.call_log.entries();
        assert!(log.iter().all(|e| e.high_risk), "{log:?}");
        // The log never holds the arguments.
        assert!(log.iter().all(|e| !e.tool.contains("Get-Date") && !e.tool.contains("a.txt")));
    }

    #[test]
    fn calls_run_one_at_a_time() {
        // A slow backend: two calls at once never overlap.
        struct Slow { inside: AtomicBool, overlapped: AtomicBool }
        impl ComputerBackend for Slow {
            fn execute(&self, _: &str, _: &Map<String, Value>) -> ToolResult {
                if self.inside.swap(true, Ordering::SeqCst) { self.overlapped.store(true, Ordering::SeqCst); }
                std::thread::sleep(Duration::from_millis(30));
                self.inside.store(false, Ordering::SeqCst);
                ToolResult::text("ok")
            }
        }
        let c = crig(Some(McpApproval::Allowed));
        c.on(&[ToolGroup::Input]);
        let slow = Arc::new(Slow { inside: AtomicBool::new(false), overlapped: AtomicBool::new(false) });
        let parts = ComputerParts { backend: slow.clone(), stop: Arc::new(EmergencyStop::default()), ready: Arc::new(|| Ok(())) };
        let CRig { mut r, .. } = c;
        let s = std::mem::replace(&mut r.service, rig(None).service);
        r.service = s.with_computer(parts);
        let r = Arc::new(r);
        let threads: Vec<_> = (0..3).map(|_| { let r = r.clone(); std::thread::spawn(move || r.rpc("tools/call", json!({"name": "computer_mouse_move", "arguments": {"x": 1, "y": 1}}))) }).collect();
        for t in threads { assert_eq!(t.join().unwrap()["result"]["isError"], false); }
        assert!(!slow.overlapped.load(Ordering::SeqCst));
    }

    #[test]
    fn computer_names_never_shadow_gadget_tools() {
        let gadget: BTreeSet<String> = McpTools::all().into_iter().map(|t| t.name).collect();
        for t in &ComputerTools::ALL {
            assert!(!gadget.contains(&t.mcp_name()) && !gadget.contains(t.name), "{} collides with a gadget tool", t.name);
        }
    }
}
