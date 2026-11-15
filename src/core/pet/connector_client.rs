//! `/ws/connector` client. Port of `PetConnectorClient.swift`: Nacho Combo registers its capabilities, the server
//! forwards tool calls as `request` frames, the shell runs them ([`PetCommandExecuting`]) and the client answers with
//! `response` (same `id`).
//!
//! Binding token: `POST /api/v1/agents/{agent_id}/connector-binding {kind, label}` → `{token:"nct_…"}`, shown only
//! once; kept in the secret store. One connector session per Nacho: a newer one replaces ours (`error{code:"replaced"}`)
//! and we do not fight back (no automatic reconnect after a replacement).
//!
//! Client platform: the macOS app sends no platform field in `hello` (`{type, token, capabilities}` only); the device
//! is named by the binding's `label`, which macOS fixes to "Nacho Combo (Mac)". Here the label is a parameter (the
//! Windows shell passes [`ConnectorBinding::LABEL_WINDOWS`]); `kind` stays `nacho_combo` (the server's connector kind,
//! not a platform). The hello frame keeps the macOS shape so a strict server never sees an unknown field.
//!
//! Not ported (macOS 0.10.2): the gadget tool registry and its "always allow" trust (re-declaration with a ping
//! barrier, `approval_revoked` refusals) and the registry tools beyond the 16 `PetCommands`. What stays: the shell
//! decides what hello declares ([`PetConnectorClient::with_capabilities`], [`PetConnectorClient::declare`]) and a
//! request for anything not declared is refused before it reaches the shell.
use super::chat_client::{MakeTransport, Schedule, TransportEvent, WebSocketTransport};
use super::commands::{PetCommand, PetCommandOutcome, PetCommandProblem, PetCommands};
use super::http::PetHTTPRequest;
use super::json::{self, JsonExt};
use super::media::PetIDs;
use super::reconnect::{ReconnectPolicy, TransportFailure};
use super::settings::ServerEndpoint;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectorPhase {
    Off,
    Connecting,
    Ready,
    Reconnecting { attempt: u32 },
    /// Another connector (e.g. a second PC) took over this Nacho's connector slot.
    Replaced,
    Failed(String),
}

impl ConnectorPhase {
    pub fn korean_label(&self) -> String {
        match self {
            Self::Off => "꺼짐".into(),
            Self::Connecting => "연결 중".into(),
            Self::Ready => "연결됨".into(),
            Self::Reconnecting { .. } => "다시 연결 중".into(),
            Self::Replaced => "다른 기기가 연결해 쉬는 중".into(),
            Self::Failed(m) => m.clone(),
        }
    }
}

pub struct ConnectorBinding;

impl ConnectorBinding {
    pub const KIND: &'static str = "nacho_combo";
    /// What the macOS app sends as `label`.
    pub const LABEL_MAC: &'static str = "Nacho Combo (Mac)";
    /// What the Windows shell should send as `label`.
    pub const LABEL_WINDOWS: &'static str = "Nacho Combo (Windows)";

    /// None for an agent id that is not a UUID (it goes into the path).
    pub fn request(endpoint: &ServerEndpoint, bearer: Option<&str>, agent_id: &str, label: &str) -> Option<PetHTTPRequest> {
        if !PetIDs::is_uuid(agent_id) { return None; }
        Some(PetHTTPRequest::post_json(endpoint.rest(&format!("/api/v1/agents/{}/connector-binding", agent_id.to_lowercase())), bearer,
                                       &json!({"kind": Self::KIND, "label": label})))
    }

    /// `nct_` + lower-case hex. Anything else is not stored.
    pub fn token(data: &[u8]) -> Option<String> {
        let t = json::object(data)?.str("token")?;
        let ok = t.starts_with("nct_") && t.chars().count() <= 200 && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        ok.then_some(t)
    }

    pub fn secret_key(scope: &str, agent_id: &str) -> String { format!("connector|{scope}|{}", agent_id.to_lowercase()) }
    /// Which Nacho the stored binding belongs to, so signing out can delete it even when the chat is not connected.
    pub fn agent_key(scope: &str) -> String { format!("connector-agent|{scope}") }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectorIncoming {
    Ready { agent_id: String },
    Request { id: String, capability: String, params_json: String },
    Error { code: String },
    Pong,
    Other,
}

pub struct ConnectorFrames;

impl ConnectorFrames {
    pub const PING: &'static str = r#"{"type":"ping"}"#;

    /// The first frame: the binding token goes here, not in a header, so it never shows up in proxy logs.
    pub fn hello(token: &str, capabilities: &[Value]) -> String {
        json::encode(&json!({"type": "hello", "token": token, "capabilities": capabilities}))
    }

    /// Declares the capabilities again on an open connection.
    pub fn capabilities(declaration: &[Value]) -> String { json::encode(&json!({"type": "capabilities", "capabilities": declaration})) }

    pub fn response(id: &str, outcome: &PetCommandOutcome) -> String {
        json::encode(&json!({"type": "response", "id": id, "result": outcome.result_json()}))
    }

    pub fn response_error(id: &str, error: &str) -> String { json::encode(&json!({"type": "response", "id": id, "error": error})) }

    pub fn decode(text: &str) -> ConnectorIncoming {
        let Some(o) = json::object_str(text) else { return ConnectorIncoming::Other };
        match o.str("type").as_deref() {
            Some("ready") => ConnectorIncoming::Ready { agent_id: o.str("agent_id").unwrap_or_default() },
            Some("request") => {
                let id = o.str("id").filter(|i| !i.is_empty() && i.chars().count() <= 100);
                let (Some(id), Some(capability)) = (id, o.str("capability")) else { return ConnectorIncoming::Other };
                ConnectorIncoming::Request { id, capability, params_json: o.get("params").map_or_else(|| "{}".into(), json::encode) }
            }
            Some("error") => ConnectorIncoming::Error { code: o.str("code").unwrap_or_else(|| "unknown".into()) },
            Some("pong") => ConnectorIncoming::Pong,
            _ => ConnectorIncoming::Other,
        }
    }
}

/// Runs the commands on this PC (the Win32 shell; it hops to its UI thread as needed). `done` exactly once, any thread.
pub trait PetCommandExecuting: Send + Sync {
    fn execute(&self, command: PetCommand, done: Box<dyn FnOnce(PetCommandOutcome) + Send>);
}

pub type PhaseHandler = Arc<dyn Fn(ConnectorPhase) + Send + Sync>;
type Transport = Arc<dyn WebSocketTransport>;

struct State {
    phase: ConnectorPhase,
    handled: Vec<String>,
    transport: Option<Transport>,
    generation: u64,
    attempt: u32,
    wanted: bool,
    capabilities: Vec<Value>,
}

/// Work decided under the lock and done after it is released (transport calls, callbacks, timers).
enum Act {
    Send(Transport, String),
    Close(Transport),
    Connect(Transport, u64),
    Phase(ConnectorPhase),
    Unauthorized,
    Execute(Arc<dyn PetCommandExecuting>, PetCommand, String, String, u64),
    Later(f64, Box<dyn FnOnce() + Send>),
}

struct Inner {
    url: String,
    token: String,
    executor: Weak<dyn PetCommandExecuting>,
    make_transport: MakeTransport,
    policy: ReconnectPolicy,
    schedule: Schedule,
    jitter: Arc<dyn Fn() -> f64 + Send + Sync>,
    state: Mutex<State>,
    on_change: Mutex<Option<PhaseHandler>>,
    on_unauthorized: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

pub struct PetConnectorClient {
    inner: Arc<Inner>,
}

impl PetConnectorClient {
    /// `url` is `ServerEndpoint::connector_url()`; `token` the binding token. The executor is held weakly (a closing
    /// app answers "Nacho Combo is closing"). Declares `PetCommands::capabilities()` unless told otherwise.
    pub fn new(url: &str, token: &str, executor: &Arc<dyn PetCommandExecuting>, make_transport: MakeTransport, schedule: Schedule) -> Self {
        let state = State {
            phase: ConnectorPhase::Off, handled: Vec::new(), transport: None, generation: 0, attempt: 0, wanted: false,
            capabilities: PetCommands::capabilities(),
        };
        Self { inner: Arc::new(Inner {
            url: url.into(), token: token.into(), executor: Arc::downgrade(executor), make_transport,
            policy: ReconnectPolicy::default(), schedule, jitter: Arc::new(|| 1.0), state: Mutex::new(state),
            on_change: Mutex::new(None), on_unauthorized: Mutex::new(None),
        }) }
    }

    fn configure(&mut self) -> &mut Inner { Arc::get_mut(&mut self.inner).expect("configure the connector client before connect") }

    pub fn with_policy(mut self, policy: ReconnectPolicy) -> Self { self.configure().policy = policy; self }
    /// 0...1, new for every reconnect (the shell passes OS randomness; the default 1.0 means no jitter).
    pub fn with_jitter(mut self, jitter: Arc<dyn Fn() -> f64 + Send + Sync>) -> Self { self.configure().jitter = jitter; self }
    /// What hello declares (W3 chooses the Windows set from `PetCommands::capabilities_with`).
    pub fn with_capabilities(self, capabilities: Vec<Value>) -> Self { self.inner.state.lock().unwrap().capabilities = capabilities; self }

    pub fn set_on_change(&self, h: Option<PhaseHandler>) { *self.inner.on_change.lock().unwrap() = h; }
    /// The server refused our binding token (revoked or unknown): the session issues a new one.
    pub fn set_on_unauthorized(&self, h: Option<Arc<dyn Fn() + Send + Sync>>) { *self.inner.on_unauthorized.lock().unwrap() = h; }

    pub fn phase(&self) -> ConnectorPhase { self.inner.state.lock().unwrap().phase.clone() }
    /// Command log for diagnostics: capability + ok/failed/refused (never parameters' free text).
    pub fn handled(&self) -> Vec<String> { self.inner.state.lock().unwrap().handled.clone() }

    pub fn connect(&self) {
        let mut acts = Vec::new();
        {
            let mut s = self.inner.state.lock().unwrap();
            s.wanted = true;
            s.attempt = 0;
            Inner::open(&self.inner, &mut s, &mut acts);
        }
        Inner::run(&self.inner, acts);
    }

    pub fn disconnect(&self) {
        let mut acts = Vec::new();
        {
            let mut s = self.inner.state.lock().unwrap();
            s.wanted = false;
            s.generation += 1;
            if let Some(t) = s.transport.take() { acts.push(Act::Close(t)); }
            Inner::set(&mut s, ConnectorPhase::Off, &mut acts);
        }
        Inner::run(&self.inner, acts);
    }

    /// Changes what is declared; an open connection hears it at once (`capabilities` frame), a later one in hello.
    pub fn declare(&self, capabilities: Vec<Value>) {
        let mut acts = Vec::new();
        {
            let mut s = self.inner.state.lock().unwrap();
            s.capabilities = capabilities;
            if let (true, ConnectorPhase::Ready, Some(t)) = (s.wanted, &s.phase, &s.transport) {
                acts.push(Act::Send(t.clone(), ConnectorFrames::capabilities(&s.capabilities)));
            }
        }
        Inner::run(&self.inner, acts);
    }
}

impl Drop for PetConnectorClient {
    fn drop(&mut self) {
        // Nothing may keep a socket of a client nobody holds.
        let t = { let mut s = self.inner.state.lock().unwrap(); s.wanted = false; s.generation += 1; s.transport.take() };
        if let Some(t) = t { t.close(); }
    }
}

impl Inner {
    fn set(s: &mut State, p: ConnectorPhase, acts: &mut Vec<Act>) {
        s.phase = p.clone();
        acts.push(Act::Phase(p));
    }

    fn open(this: &Arc<Inner>, s: &mut State, acts: &mut Vec<Act>) {
        s.generation += 1;
        if let Some(t) = s.transport.take() { acts.push(Act::Close(t)); }
        let t: Transport = Arc::from((this.make_transport)());
        s.transport = Some(t.clone());
        let p = if s.attempt == 0 { ConnectorPhase::Connecting } else { ConnectorPhase::Reconnecting { attempt: s.attempt } };
        Self::set(s, p, acts);
        acts.push(Act::Connect(t, s.generation));
    }

    fn run(this: &Arc<Inner>, acts: Vec<Act>) {
        for a in acts {
            match a {
                Act::Send(t, text) => t.send(&text),
                Act::Close(t) => t.close(),
                Act::Connect(t, gen) => {
                    let weak = Arc::downgrade(this);
                    // The token goes in the first frame (`hello`), not in a header.
                    t.connect(&this.url, None, Arc::new(move |ev| { if let Some(i) = weak.upgrade() { Inner::handle(&i, ev, gen) } }));
                }
                Act::Phase(p) => { let h = this.on_change.lock().unwrap().clone(); if let Some(h) = h { h(p) } }
                Act::Unauthorized => { let h = this.on_unauthorized.lock().unwrap().clone(); if let Some(h) = h { h() } }
                Act::Execute(ex, command, id, capability, gen) => {
                    let weak = Arc::downgrade(this);
                    ex.execute(command, Box::new(move |outcome| {
                        let Some(i) = weak.upgrade() else { return };
                        let mut acts = Vec::new();
                        {
                            let mut s = i.state.lock().unwrap();
                            s.handled.push(format!("{capability}:{}", if outcome.ok { "ok" } else { "failed" }));
                            // The socket changed: the server already timed this call out.
                            if let (true, Some(t)) = (gen == s.generation, &s.transport) {
                                acts.push(Act::Send(t.clone(), ConnectorFrames::response(&id, &outcome)));
                            }
                        }
                        Inner::run(&i, acts);
                    }));
                }
                Act::Later(delay, work) => (this.schedule)(delay, work),
            }
        }
    }

    fn handle(this: &Arc<Inner>, ev: TransportEvent, gen: u64) {
        let mut acts = Vec::new();
        {
            let mut s = this.state.lock().unwrap();
            if gen != s.generation { return; }
            match ev {
                TransportEvent::Opened => {
                    if let Some(t) = &s.transport { acts.push(Act::Send(t.clone(), ConnectorFrames::hello(&this.token, &s.capabilities))); }
                }
                TransportEvent::Text(text) => match ConnectorFrames::decode(&text) {
                    ConnectorIncoming::Ready { .. } => {
                        s.attempt = 0;
                        Self::set(&mut s, ConnectorPhase::Ready, &mut acts);
                        Self::schedule_ping(this, gen, &mut acts);
                    }
                    ConnectorIncoming::Request { id, capability, params_json } => Self::request(this, &mut s, &id, &capability, &params_json, gen, &mut acts),
                    ConnectorIncoming::Error { code } => {
                        if code == "replaced" {
                            s.wanted = false;
                            Self::set(&mut s, ConnectorPhase::Replaced, &mut acts);
                        } else if ["unauthorized", "not_found", "forbidden"].contains(&code.as_str()) {
                            s.wanted = false;
                            Self::set(&mut s, ConnectorPhase::Failed("커넥터 연결이 거절됐어요.".into()), &mut acts);
                            acts.push(Act::Unauthorized);
                        }
                    }
                    ConnectorIncoming::Pong | ConnectorIncoming::Other => {}
                },
                TransportEvent::Closed(failure) => Self::closed(this, &mut s, failure, &mut acts),
            }
        }
        Self::run(this, acts);
    }

    fn closed(this: &Arc<Inner>, s: &mut State, failure: Option<TransportFailure>, acts: &mut Vec<Act>) {
        s.transport = None;
        if !s.wanted {
            if !matches!(s.phase, ConnectorPhase::Replaced | ConnectorPhase::Failed(_)) { Self::set(s, ConnectorPhase::Off, acts); } // keep the reason on screen
            return;
        }
        s.attempt += 1;
        let f = failure.unwrap_or_else(|| TransportFailure::new(None, "closed"));
        if !this.policy.should_retry(s.attempt, &f) {
            s.wanted = false;
            return Self::set(s, ConnectorPhase::Failed("커넥터에 연결하지 못했어요.".into()), acts);
        }
        Self::set(s, ConnectorPhase::Reconnecting { attempt: s.attempt }, acts);
        let retry_gen = s.generation;
        let weak = Arc::downgrade(this);
        acts.push(Act::Later(this.policy.delay(s.attempt, (this.jitter)()), Box::new(move || {
            let Some(i) = weak.upgrade() else { return };
            let mut acts = Vec::new();
            {
                let mut s = i.state.lock().unwrap();
                if !s.wanted || retry_gen != s.generation { return; }
                Inner::open(&i, &mut s, &mut acts);
            }
            Inner::run(&i, acts);
        })));
    }

    fn request(this: &Arc<Inner>, s: &mut State, id: &str, capability: &str, params_json: &str, gen: u64, acts: &mut Vec<Act>) {
        let Some(t) = s.transport.clone() else { return };
        let declared: Vec<String> = s.capabilities.iter().filter_map(|c| c["name"].as_str().map(str::to_string)).collect();
        let short: String = capability.chars().take(64).collect();
        let refuse = |s: &mut State, acts: &mut Vec<Act>, p: PetCommandProblem| {
            s.handled.push(format!("{short}:refused"));
            acts.push(Act::Send(t.clone(), ConnectorFrames::response_error(id, &p.message())));
        };
        // Closed list: only what this client declared (a legacy name declared also admits its aliases).
        if let Some(legacy) = PetCommands::resolve(capability) {
            if !declared.iter().any(|d| d == legacy) {
                return refuse(s, acts, PetCommandProblem::NotAllowed(format!("{short} is not declared by this Nacho Combo")));
            }
        }
        let params: Option<Value> = serde_json::from_str(params_json).ok();
        let allow_quit = declared.iter().any(|d| d == PetCommands::QUIT_APP);
        match PetCommands::parse_with(capability, params.as_ref(), allow_quit) {
            Err(p) => refuse(s, acts, p),
            Ok(command) => match this.executor.upgrade() {
                None => acts.push(Act::Send(t.clone(), ConnectorFrames::response_error(id, "Nacho Combo is closing"))),
                Some(ex) => acts.push(Act::Execute(ex, command, id.into(), capability.into(), gen)),
            },
        }
    }

    fn schedule_ping(this: &Arc<Inner>, gen: u64, acts: &mut Vec<Act>) {
        let weak = Arc::downgrade(this);
        acts.push(Act::Later(30.0, Box::new(move || {
            let Some(i) = weak.upgrade() else { return };
            let mut acts = Vec::new();
            {
                let s = i.state.lock().unwrap();
                if gen != s.generation || s.phase != ConnectorPhase::Ready { return; }
                if let Some(t) = &s.transport { acts.push(Act::Send(t.clone(), ConnectorFrames::PING.into())); }
                Inner::schedule_ping(&i, gen, &mut acts);
            }
            Inner::run(&i, acts);
        })));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::layout::SnapAction;
    use crate::core::pet::auth::testing::PetStubHTTP;
    use crate::core::pet::chat_client::TransportHandler;

    #[derive(Default)]
    struct FakeTransport {
        url: Mutex<Option<String>>,
        bearer: Mutex<Option<String>>,
        sent: Mutex<Vec<String>>,
        closed: Mutex<bool>,
        handler: Mutex<Option<TransportHandler>>,
    }

    impl FakeTransport {
        fn fire(&self, ev: TransportEvent) { let h = self.handler.lock().unwrap().clone().unwrap(); h(ev) }
        fn deliver(&self, json: &str) { self.fire(TransportEvent::Text(json.into())) }
        fn sent(&self) -> Vec<String> { self.sent.lock().unwrap().clone() }
        fn last(&self) -> json::JsonObject { json::object_str(self.sent().last().unwrap()).unwrap() }
    }

    struct Handle(Arc<FakeTransport>);

    impl WebSocketTransport for Handle {
        fn connect(&self, url: &str, bearer: Option<&str>, handler: TransportHandler) {
            *self.0.url.lock().unwrap() = Some(url.into());
            *self.0.bearer.lock().unwrap() = bearer.map(str::to_string);
            *self.0.handler.lock().unwrap() = Some(handler);
        }
        fn send(&self, text: &str) { self.0.sent.lock().unwrap().push(text.into()) }
        fn close(&self) { *self.0.closed.lock().unwrap() = true }
    }

    /// Records commands and answers ok.
    #[derive(Default)]
    struct RecordingExecutor { commands: Mutex<Vec<PetCommand>> }

    impl PetCommandExecuting for RecordingExecutor {
        fn execute(&self, command: PetCommand, done: Box<dyn FnOnce(PetCommandOutcome) + Send>) {
            self.commands.lock().unwrap().push(command);
            done(PetCommandOutcome::new(true, "done"))
        }
    }

    type Scheduled = Arc<Mutex<Vec<(f64, Box<dyn FnOnce() + Send>)>>>;
    type Made = Arc<Mutex<Vec<Arc<FakeTransport>>>>;

    fn setup() -> (Made, MakeTransport, Scheduled, Schedule) {
        let made: Made = Arc::default();
        let m = made.clone();
        let make: MakeTransport = Arc::new(move || {
            let t = Arc::new(FakeTransport::default());
            m.lock().unwrap().push(t.clone());
            Box::new(Handle(t))
        });
        let scheduled: Scheduled = Arc::default();
        let s = scheduled.clone();
        let schedule: Schedule = Arc::new(move |d, w| s.lock().unwrap().push((d, w)));
        (made, make, scheduled, schedule)
    }

    #[test]
    fn hello_request_response_and_replacement() {
        let (made, make, _scheduled, schedule) = setup();
        let rec = Arc::new(RecordingExecutor::default());
        let ex: Arc<dyn PetCommandExecuting> = rec.clone();
        let c = PetConnectorClient::new("ws://127.0.0.1:8787/ws/connector", "nct_abc", &ex, make, schedule);
        c.connect();
        let t = made.lock().unwrap()[0].clone();
        assert_eq!(t.url.lock().unwrap().as_deref(), Some("ws://127.0.0.1:8787/ws/connector"));
        assert_eq!(*t.bearer.lock().unwrap(), None, "token only in the hello frame");
        t.fire(TransportEvent::Opened);
        let hello = json::object_str(&t.sent()[0]).unwrap();
        assert_eq!(hello.str("type").as_deref(), Some("hello"));
        assert_eq!(hello.str("token").as_deref(), Some("nct_abc"));
        assert_eq!(hello["capabilities"].as_array().map(|a| a.len()), Some(PetCommands::names().len()));
        t.deliver(r#"{"type":"ready","agent_id":"a","kind":"nacho_combo"}"#);
        assert_eq!(c.phase(), ConnectorPhase::Ready);
        t.deliver(r#"{"type":"request","id":"r1","capability":"combo_arrange_window","params":{"position":"right"}}"#);
        assert_eq!(*rec.commands.lock().unwrap(), vec![PetCommand::ArrangeWindow(SnapAction::Right, None)]);
        let resp = t.last();
        assert_eq!(resp.str("id").as_deref(), Some("r1"));
        assert_eq!(resp["result"]["ok"], true);
        t.deliver(r#"{"type":"request","id":"r2","capability":"combo_set_gadget","params":{"gadget":"computerUse","on":true}}"#);
        let refused = t.last();
        assert_eq!(refused.str("id").as_deref(), Some("r2"));
        assert!(refused.str("error").is_some());
        assert_eq!(rec.commands.lock().unwrap().len(), 1, "refused before reaching the app");
        assert_eq!(c.handled(), vec!["combo_arrange_window:ok", "combo_set_gadget:refused"]);
        t.deliver(r#"{"type":"error","code":"replaced","message":"another connector session took over"}"#);
        t.fire(TransportEvent::Closed(None));
        assert_eq!(c.phase(), ConnectorPhase::Replaced);
        assert_eq!(made.lock().unwrap().len(), 1, "no fight for the slot");
    }

    #[test]
    fn binding_request_and_token() {
        let ep = ServerEndpoint::parse("https://nacho.nachotown.com").unwrap();
        assert_eq!(ConnectorBinding::request(&ep, Some("t"), "../../admin", ConnectorBinding::LABEL_WINDOWS), None);
        let r = ConnectorBinding::request(&ep, Some("t"), "0193AAAA-0000-7000-8000-000000000002", ConnectorBinding::LABEL_WINDOWS).unwrap();
        assert_eq!(r.url, "https://nacho.nachotown.com/api/v1/agents/0193aaaa-0000-7000-8000-000000000002/connector-binding");
        let body = PetStubHTTP::json(&r);
        assert_eq!(body.str("kind").as_deref(), Some("nacho_combo"));
        assert_eq!(body.str("label").as_deref(), Some("Nacho Combo (Windows)"));
        assert_eq!(r.header("Authorization"), Some("Bearer t"));
        assert_eq!(ConnectorBinding::token(br#"{"token":"nct_0a1b","endpoint":"/ws/connector"}"#).as_deref(), Some("nct_0a1b"));
        assert_eq!(ConnectorBinding::token(br#"{"token":"Bearer x"}"#), None);
        assert_eq!(ConnectorBinding::secret_key("s", "0193AAAA-0000-7000-8000-000000000002"), "connector|s|0193aaaa-0000-7000-8000-000000000002");
    }

    #[test]
    fn hello_frame_carries_the_capabilities() {
        let hello = ConnectorFrames::hello("nct_abc", &PetCommands::capabilities());
        let o = json::object_str(&hello).unwrap();
        let caps = o["capabilities"].as_array().unwrap();
        let names: Vec<&str> = caps.iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, PetCommands::names());
        assert!(names.contains(&"combo_quit_app"));
        let required: Vec<&str> = caps.iter().filter(|c| c["approval"] == "required").map(|c| c["name"].as_str().unwrap()).collect();
        assert!(required.contains(&"combo_quit_app"));
        assert!(required.contains(&"combo_clear_shelf"));
        assert!(!required.contains(&"combo_list_apps"));
        assert_eq!(ConnectorFrames::decode(r#"{"type":"request","id":"x","capability":"c"}"#),
                   ConnectorIncoming::Request { id: "x".into(), capability: "c".into(), params_json: "{}".into() });
        assert_eq!(ConnectorFrames::decode(r#"{"type":"request","id":"","capability":"c"}"#), ConnectorIncoming::Other);
        assert_eq!(ConnectorFrames::decode("nope"), ConnectorIncoming::Other);
    }

    #[test]
    fn reconnects_with_backoff_refuses_undeclared_and_reports_a_revoked_binding() {
        let (made, make, scheduled, schedule) = setup();
        let rec = Arc::new(RecordingExecutor::default());
        let ex: Arc<dyn PetCommandExecuting> = rec.clone();
        let declared = PetCommands::capabilities_with(false, &Default::default()); // this shell does not quit apps
        let c = PetConnectorClient::new("ws://h/ws/connector", "nct_1", &ex, make, schedule)
            .with_policy(ReconnectPolicy { base_delay: 1.0, max_delay: 30.0, max_attempts: 2 }).with_capabilities(declared);
        let phases: Arc<Mutex<Vec<ConnectorPhase>>> = Arc::default();
        let p = phases.clone();
        c.set_on_change(Some(Arc::new(move |ph| p.lock().unwrap().push(ph))));
        let unauthorized = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let u = unauthorized.clone();
        c.set_on_unauthorized(Some(Arc::new(move || u.store(true, std::sync::atomic::Ordering::SeqCst))));
        c.connect();
        let t = made.lock().unwrap()[0].clone();
        t.fire(TransportEvent::Opened);
        t.deliver(r#"{"type":"ready","agent_id":"a"}"#);
        t.deliver(r#"{"type":"request","id":"q","capability":"combo_appswitch_quit","params":{"app":"Notepad"}}"#);
        assert!(t.last().str("error").unwrap().contains("not declared"));
        assert!(rec.commands.lock().unwrap().is_empty());
        // The ping timer is scheduled after ready.
        let ping = scheduled.lock().unwrap().remove(0);
        assert_eq!(ping.0, 30.0);
        (ping.1)();
        assert_eq!(t.sent().last().map(String::as_str), Some(ConnectorFrames::PING));

        t.fire(TransportEvent::Closed(Some(TransportFailure::new(None, "reset"))));
        assert_eq!(c.phase(), ConnectorPhase::Reconnecting { attempt: 1 });
        let retry = scheduled.lock().unwrap().pop().unwrap();
        assert_eq!(retry.0, 1.0);
        (retry.1)();
        assert_eq!(made.lock().unwrap().len(), 2);
        let t2 = made.lock().unwrap()[1].clone();
        t.fire(TransportEvent::Closed(None)); // the old socket's late event is ignored
        assert_eq!(c.phase(), ConnectorPhase::Reconnecting { attempt: 1 });
        t2.fire(TransportEvent::Opened);
        t2.deliver(r#"{"type":"error","code":"unauthorized"}"#);
        assert_eq!(c.phase(), ConnectorPhase::Failed("커넥터 연결이 거절됐어요.".into()));
        assert!(unauthorized.load(std::sync::atomic::Ordering::SeqCst));
        t2.fire(TransportEvent::Closed(None));
        assert_eq!(c.phase(), ConnectorPhase::Failed("커넥터 연결이 거절됐어요.".into()), "the reason stays on screen");
        assert_eq!(phases.lock().unwrap().first(), Some(&ConnectorPhase::Connecting));
        c.disconnect();
        assert_eq!(c.phase(), ConnectorPhase::Off);
    }
}
