//! `/ws/chat` client. Port of `PetChatClient.swift`: the transport contract every WebSocket implementation follows
//! (WinHTTP on Windows, a plain socket in the dev CLI ([`super::socket_transport`]), a fake in tests), the outgoing text
//! rules, the send refusals and [`ChatClient`] (connect with Bearer, stream turns, stop, ping, reconnect with backoff).
//!
//! Threading model of the pet core (same for every client and the session): state lives behind `Arc<Mutex<…>>`;
//! transport handlers, HTTP completions and scheduled timers may run on any thread; a `generation` counter drops the
//! callbacks of an older connection; `on_change` is called after every change, never while a lock is held.
use super::chat_state::{ChatState, ConnectionPhase, ErrorText};
use super::frames::{ClientFrame, ServerEvent, ServerFrame};
use super::media::{PetAttachment, PetIDs, PetMedia};
use super::reconnect::{ReconnectPolicy, TransportFailure};
use super::settings::ServerEndpoint;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportEvent {
    Opened,
    Text(String),
    /// None = clean close.
    Closed(Option<TransportFailure>),
}

pub type TransportHandler = Arc<dyn Fn(TransportEvent) + Send + Sync>;

/// One WebSocket connection. A new transport object is made for every connection attempt.
pub trait WebSocketTransport: Send + Sync {
    /// `url` is `ws://` or `wss://`; `bearer` goes in `Authorization: Bearer …` of the upgrade request.
    fn connect(&self, url: &str, bearer: Option<&str>, handler: TransportHandler);
    fn send(&self, text: &str);
    fn close(&self);
}

pub type MakeTransport = Arc<dyn Fn() -> Box<dyn WebSocketTransport> + Send + Sync>;

/// Runs `work` after `delay` seconds (on any thread). Tests pass a manual scheduler.
pub type Schedule = Arc<dyn Fn(f64, Box<dyn FnOnce() + Send>) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutgoingProblem { Empty, TooLong }

impl OutgoingProblem {
    pub fn korean_message(&self) -> String {
        match self {
            Self::Empty => "보낼 내용을 입력해 주세요.".into(),
            Self::TooLong => format!("한 번에 보낼 수 있는 길이는 {}자까지예요.", OutgoingText::MAX_CODE_POINTS),
        }
    }
}

pub struct OutgoingText;

impl OutgoingText {
    /// Client-side limit (the server's limit is not documented).
    pub const MAX_CODE_POINTS: usize = 4000;

    /// Trims, rejects empty / over-long text, drops NUL. Length is counted in Unicode code points. `allow_empty`: a
    /// message that carries attachments may have no text (the server accepts attachment-only turns).
    pub fn validate(raw: &str, allow_empty: bool) -> Result<String, OutgoingProblem> {
        let t = raw.replace('\0', "");
        let t = t.trim();
        if t.is_empty() && !allow_empty { return Err(OutgoingProblem::Empty); }
        if t.chars().count() > Self::MAX_CODE_POINTS { return Err(OutgoingProblem::TooLong); }
        Ok(t.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendRefusal { InvalidText(OutgoingProblem), NotConnected, Busy, TooManyAttachments }

impl SendRefusal {
    pub fn korean_message(&self) -> String {
        match self {
            Self::InvalidText(p) => p.korean_message(),
            Self::TooManyAttachments => format!("한 번에 붙일 수 있는 파일은 {}개까지예요.", PetMedia::MAX_ATTACHMENTS),
            Self::NotConnected => "서버에 연결되어 있지 않아요.".into(),
            Self::Busy => "나초가 아직 답하고 있어요. 끝난 뒤에 보내 주세요.".into(),
        }
    }
}


// MARK: the client

/// Called after every change of the chat state (on any thread, never while the client holds its lock).
pub type ChatChange = Arc<dyn Fn(&ChatState) + Send + Sync>;
/// The current access token (read for every connection attempt, so a refreshed token is used on reconnect).
pub type TokenSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// A number in 0…1 for the reconnect jitter.
pub type Jitter = Arc<dyn Fn() -> f64 + Send + Sync>;

/// Jitter from std's per-process random hasher keys (not for secrets; spreads reconnects of many PCs apart).
pub fn default_jitter() -> Jitter {
    Arc::new(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        (h.finish() >> 11) as f64 / (1u64 << 53) as f64
    })
}

/// A [`Schedule`] that sleeps on a short-lived thread (dev CLI; the app uses its own timer).
pub fn thread_schedule() -> Schedule {
    Arc::new(|delay, work| {
        std::thread::spawn(move || {
            if delay > 0.0 && delay.is_finite() { std::thread::sleep(std::time::Duration::from_secs_f64(delay)); }
            work();
        });
    })
}

#[derive(Clone)]
pub struct ChatClientOptions {
    pub policy: ReconnectPolicy,
    /// Seconds between `{type:ping}` frames while connected.
    pub ping_interval: f64,
    pub jitter: Jitter,
}

impl Default for ChatClientOptions {
    fn default() -> Self { Self { policy: ReconnectPolicy::default(), ping_interval: 30.0, jitter: default_jitter() } }
}

struct Inner {
    state: ChatState,
    transport: Option<Arc<dyn WebSocketTransport>>,
    /// Invalidates callbacks of older connections and timers.
    generation: u64,
    attempt: u32,
    want_connected: bool,
    /// Bumped with every published snapshot, so a slower thread never publishes an older state over a newer one.
    revision: u64,
}

impl Inner {
    fn snapshot(&mut self) -> (u64, ChatState) {
        self.revision += 1;
        (self.revision, self.state.clone())
    }
}

struct Shared {
    endpoint: ServerEndpoint,
    token: TokenSource,
    make_transport: MakeTransport,
    schedule: Schedule,
    opts: ChatClientOptions,
    inner: Mutex<Inner>,
    /// Held around "change the state + send the frame" so frames leave in the order the state saw them. Only taken
    /// outside `inner`, and transport handlers never take it, so a transport that reports synchronously cannot deadlock.
    send_order: Mutex<()>,
    on_change: Mutex<Option<ChatChange>>,
    published: AtomicU64,
}

/// `/ws/chat` client: connect with Bearer, stream turns, stop, ping, reconnect with backoff. Thread-safe; every method
/// may be called from any thread. Dropping the client closes its connection.
pub struct ChatClient {
    shared: Arc<Shared>,
}

impl ChatClient {
    pub fn new(endpoint: ServerEndpoint, token: TokenSource, make_transport: MakeTransport, schedule: Schedule, opts: ChatClientOptions) -> Self {
        let inner = Inner {
            state: ChatState::new(),
            transport: None,
            generation: 0,
            attempt: 0,
            want_connected: false,
            revision: 0,
        };
        let shared = Shared {
            endpoint,
            token,
            make_transport,
            schedule,
            opts,
            inner: Mutex::new(inner),
            send_order: Mutex::new(()),
            on_change: Mutex::new(None),
            published: AtomicU64::new(0),
        };
        Self { shared: Arc::new(shared) }
    }

    /// A copy of the current state.
    pub fn state(&self) -> ChatState { self.shared.lock().state.clone() }

    pub fn set_on_change(&self, f: Option<ChatChange>) { *self.shared.on_change.lock().unwrap_or_else(|e| e.into_inner()) = f; }

    pub fn connect(&self) {
        {
            let mut i = self.shared.lock();
            i.want_connected = true;
            i.attempt = 0;
        }
        self.shared.open_connection();
    }

    pub fn disconnect(&self) {
        let (old, snap) = {
            let mut i = self.shared.lock();
            i.want_connected = false;
            i.generation += 1;
            let old = i.transport.take();
            i.state.connection_lost();
            i.state.phase = ConnectionPhase::Disconnected;
            (old, i.snapshot())
        };
        if let Some(t) = old { t.close(); }
        self.shared.publish(snap);
    }

    /// Validates and sends one user message. Returns the client_msg_id, or the reason it was refused. `attachments`
    /// must already be uploaded (`POST /api/v1/media/uploads`); at most `PetMedia::MAX_ATTACHMENTS`.
    pub fn send(&self, raw: &str, attachments: &[PetAttachment]) -> Result<String, SendRefusal> {
        if attachments.len() > PetMedia::MAX_ATTACHMENTS { return Err(SendRefusal::TooManyAttachments); }
        let text = OutgoingText::validate(raw, !attachments.is_empty()).map_err(SendRefusal::InvalidText)?;
        let _order = self.shared.send_order.lock().unwrap_or_else(|e| e.into_inner());
        let (t, frame, id, snap) = {
            let mut i = self.shared.lock();
            let t = match (&i.transport, i.state.phase) {
                (Some(t), ConnectionPhase::Connected) => t.clone(),
                _ => return Err(SendRefusal::NotConnected),
            };
            if i.state.is_busy() { return Err(SendRefusal::Busy); }
            let id = format!("g-{}", &PetIDs::new_uuid()[..12]);
            i.state.begin_turn(&text, &id, attachments.iter().map(|a| a.name.clone()).collect());
            let frame = ClientFrame::Message {
                text,
                room_id: i.state.room_id.clone(),
                client_msg_id: id.clone(),
                attachment_ids: attachments.iter().map(|a| a.attachment_id.clone()).collect(),
            };
            (t, frame.encode(), id, i.snapshot())
        };
        t.send(&frame);
        drop(_order);
        self.shared.publish(snap);
        Ok(id)
    }

    /// Changes the chat state from outside (room record, recovered answers, memory lines) and publishes it. `body` runs
    /// under the client's lock: it must not call back into the client.
    pub fn update(&self, body: impl FnOnce(&mut ChatState)) {
        let snap = {
            let mut i = self.shared.lock();
            body(&mut i.state);
            i.snapshot()
        };
        self.shared.publish(snap);
    }

    /// Read mark (`{type:read,room_id,seq}`), forward only; the server answers nothing unless it fails.
    pub fn mark_read(&self, seq: i64) {
        let _order = self.shared.send_order.lock().unwrap_or_else(|e| e.into_inner());
        let send = {
            let i = self.shared.lock();
            match (&i.transport, &i.state.room_id) {
                (Some(t), Some(room)) if i.state.phase == ConnectionPhase::Connected && !room.is_empty() && seq > 0 => {
                    Some((t.clone(), ClientFrame::Read { room_id: room.clone(), seq }.encode()))
                }
                _ => None,
            }
        };
        if let Some((t, f)) = send { t.send(&f); }
    }

    /// Asks the server to stop the running answer (only while a turn runs).
    pub fn stop(&self) {
        let _order = self.shared.send_order.lock().unwrap_or_else(|e| e.into_inner());
        let (t, frame, snap) = {
            let mut i = self.shared.lock();
            let (Some(t), Some(room)) = (i.transport.clone(), i.state.room_id.clone()) else { return };
            if !i.state.is_busy() { return; }
            i.state.mark_stop_requested();
            (t, ClientFrame::Stop { room_id: room }.encode(), i.snapshot())
        };
        t.send(&frame);
        drop(_order);
        self.shared.publish(snap);
    }
}

impl Drop for ChatClient {
    fn drop(&mut self) {
        let old = {
            let mut i = self.shared.lock();
            i.want_connected = false;
            i.generation += 1;
            i.transport.take()
        };
        if let Some(t) = old { t.close(); }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> { self.inner.lock().unwrap_or_else(|e| e.into_inner()) }

    fn publish(&self, (rev, state): (u64, ChatState)) {
        if self.published.fetch_max(rev, Ordering::SeqCst) >= rev { return; } // a newer state went out already
        let cb = self.on_change.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(cb) = cb { cb(&state); }
    }

    fn open_connection(self: &Arc<Self>) {
        let (gen, old, snap) = {
            let mut i = self.lock();
            i.generation += 1;
            let gen = i.generation;
            let old = i.transport.take();
            i.state.phase = if i.attempt == 0 { ConnectionPhase::Connecting } else { ConnectionPhase::Reconnecting { attempt: i.attempt } };
            (gen, old, i.snapshot())
        };
        if let Some(o) = old { o.close(); }
        let t: Arc<dyn WebSocketTransport> = Arc::from((self.make_transport)());
        {
            let mut i = self.lock();
            if i.generation != gen { return; } // disconnected meanwhile
            i.transport = Some(t.clone());
        }
        self.publish(snap);
        let weak: Weak<Shared> = Arc::downgrade(self);
        let handler: TransportHandler = Arc::new(move |ev| {
            if let Some(s) = weak.upgrade() { s.handle(ev, gen); }
        });
        let bearer = (self.token)();
        t.connect(&self.endpoint.web_socket_url(), bearer.as_deref(), handler);
    }

    fn handle(self: &Arc<Self>, ev: TransportEvent, gen: u64) {
        match ev {
            TransportEvent::Opened => {} // the `ready` frame marks the connection usable
            TransportEvent::Text(s) => {
                let Some(frame) = ServerFrame::decode(&s) else { return };
                let ready = matches!(frame.event, ServerEvent::Ready(_));
                let snap = {
                    let mut i = self.lock();
                    if gen != i.generation { return; }
                    if ready { i.attempt = 0; }
                    i.state.apply(&frame);
                    i.snapshot()
                };
                if ready { self.schedule_ping(gen); }
                self.publish(snap);
            }
            TransportEvent::Closed(failure) => {
                let jitter = (self.opts.jitter)();
                let mut retry = None;
                let snap = {
                    let mut i = self.lock();
                    if gen != i.generation { return; }
                    i.transport = None;
                    i.state.connection_lost();
                    if !i.want_connected {
                        i.state.phase = ConnectionPhase::Disconnected;
                    } else {
                        let f = failure.unwrap_or_else(|| TransportFailure::new(None, "연결이 닫혔어요"));
                        i.attempt += 1;
                        let attempt = i.attempt;
                        if matches!(f.http_status, Some(401) | Some(403)) {
                            i.state.phase = ConnectionPhase::AuthFailed;
                            i.state.notice = Some(ErrorText::user_message("unauthorized", None, ""));
                            i.want_connected = false;
                        } else if self.opts.policy.should_retry(attempt, &f) {
                            i.state.phase = ConnectionPhase::Reconnecting { attempt };
                            retry = Some((self.opts.policy.delay(attempt, jitter), i.generation));
                        } else {
                            i.state.phase = ConnectionPhase::Disconnected;
                            i.state.notice = Some("서버에 연결하지 못했어요. 설정에서 서버 주소를 확인해 주세요.".into());
                            i.want_connected = false;
                        }
                    }
                    i.snapshot()
                };
                if let Some((delay, retry_gen)) = retry {
                    let weak = Arc::downgrade(self);
                    (self.schedule)(delay, Box::new(move || {
                        let Some(s) = weak.upgrade() else { return };
                        let go = {
                            let i = s.lock();
                            i.want_connected && i.generation == retry_gen
                        };
                        if go { s.open_connection(); }
                    }));
                }
                self.publish(snap);
            }
        }
    }

    fn schedule_ping(self: &Arc<Self>, gen: u64) {
        let weak = Arc::downgrade(self);
        (self.schedule)(self.opts.ping_interval, Box::new(move || {
            let Some(s) = weak.upgrade() else { return };
            {
                let _order = s.send_order.lock().unwrap_or_else(|e| e.into_inner());
                let t = {
                    let i = s.lock();
                    if gen != i.generation || i.state.phase != ConnectionPhase::Connected { return; }
                    i.transport.clone()
                };
                if let Some(t) = t { t.send(&ClientFrame::Ping.encode()); }
            }
            s.schedule_ping(gen);
        }));
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod testing {
    //! A fake WebSocket and a manual timer for client and session tests.
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[derive(Default)]
    pub struct FakeTransport {
        pub url: Mutex<Option<String>>,
        pub bearer: Mutex<Option<String>>,
        pub sent: Mutex<Vec<String>>,
        pub closed: AtomicBool,
        pub handler: Mutex<Option<TransportHandler>>,
    }

    impl FakeTransport {
        /// Calls the handler like the network would (outside the fake's own locks).
        pub fn emit(&self, ev: TransportEvent) {
            let h = self.handler.lock().unwrap().clone();
            if let Some(h) = h { h(ev); }
        }
        pub fn deliver(&self, json: &str) { self.emit(TransportEvent::Text(json.into())); }
        pub fn drop_with(&self, f: Option<TransportFailure>) { self.emit(TransportEvent::Closed(f)); }
        pub fn sent(&self) -> Vec<String> { self.sent.lock().unwrap().clone() }
        pub fn last_sent(&self) -> Option<String> { self.sent.lock().unwrap().last().cloned() }
    }

    struct FakeRef(Arc<FakeTransport>);

    impl WebSocketTransport for FakeRef {
        fn connect(&self, url: &str, bearer: Option<&str>, handler: TransportHandler) {
            *self.0.url.lock().unwrap() = Some(url.into());
            *self.0.bearer.lock().unwrap() = bearer.map(Into::into);
            *self.0.handler.lock().unwrap() = Some(handler);
        }
        fn send(&self, text: &str) { self.0.sent.lock().unwrap().push(text.into()); }
        fn close(&self) { self.0.closed.store(true, Ordering::SeqCst); }
    }

    /// Every transport the client made, in order.
    #[derive(Default)]
    pub struct FakeNetwork {
        pub transports: Mutex<Vec<Arc<FakeTransport>>>,
    }

    impl FakeNetwork {
        pub fn new() -> Arc<Self> { Arc::new(Self::default()) }
        pub fn make(self: &Arc<Self>) -> MakeTransport {
            let net = self.clone();
            Arc::new(move || {
                let t = Arc::new(FakeTransport::default());
                net.transports.lock().unwrap().push(t.clone());
                Box::new(FakeRef(t)) as Box<dyn WebSocketTransport>
            })
        }
        pub fn count(&self) -> usize { self.transports.lock().unwrap().len() }
        pub fn get(&self, i: usize) -> Arc<FakeTransport> { self.transports.lock().unwrap()[i].clone() }
    }

    type Work = Box<dyn FnOnce() + Send>;

    /// Records scheduled work; the test fires it by hand.
    #[derive(Default)]
    pub struct ManualSchedule {
        pub list: Mutex<Vec<(f64, Option<Work>)>>,
    }

    impl ManualSchedule {
        pub fn new() -> Arc<Self> { Arc::new(Self::default()) }
        pub fn schedule(self: &Arc<Self>) -> Schedule {
            let me = self.clone();
            Arc::new(move |d, w| me.list.lock().unwrap().push((d, Some(w))))
        }
        pub fn len(&self) -> usize { self.list.lock().unwrap().len() }
        pub fn is_empty(&self) -> bool { self.len() == 0 }
        pub fn delay(&self, i: usize) -> f64 { self.list.lock().unwrap()[i].0 }
        pub fn last_delay(&self) -> Option<f64> { self.list.lock().unwrap().last().map(|e| e.0) }
        /// Runs entry `i` (once; outside the list's lock, so the work may schedule more).
        pub fn fire(&self, i: usize) {
            let w = self.list.lock().unwrap()[i].1.take();
            if let Some(w) = w { w(); }
        }
        pub fn fire_last(&self) {
            let n = self.len();
            self.fire(n - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{FakeNetwork, ManualSchedule};
    use super::*;
    use crate::core::pet::chat_state::TurnStatus;
    use crate::core::pet::frames::{ActivityKind, ReadyInfo};
    use crate::core::pet::json;

    #[test]
    fn outgoing_text_rules() {
        assert_eq!(OutgoingText::validate("  안녕\0 \n", false), Ok("안녕".into()));
        assert_eq!(OutgoingText::validate("  ", false), Err(OutgoingProblem::Empty));
        assert_eq!(OutgoingText::validate("  ", true), Ok("".into()));
        assert_eq!(OutgoingText::validate(&"가".repeat(4000), false).map(|s| s.chars().count()), Ok(4000));
        assert_eq!(OutgoingText::validate(&"가".repeat(4001), false), Err(OutgoingProblem::TooLong));
        assert_eq!(SendRefusal::TooManyAttachments.korean_message(), "한 번에 붙일 수 있는 파일은 4개까지예요.");
    }

    // MARK: PetFramesTests (PetChatTests.swift)

    #[test]
    fn decode_ready_and_unknown_fields() {
        let f = ServerFrame::decode(r#"{"type":"ready","user_id":"u","agent_id":"a","agent_name":"나초","room_id":"r","extra":1}"#);
        assert_eq!(
            f.map(|f| f.event),
            Some(ServerEvent::Ready(ReadyInfo { user_id: "u".into(), agent_id: "a".into(), agent_name: "나초".into(), room_id: "r".into() }))
        );
    }

    #[test]
    fn decode_stream_frames_carry_client_id() {
        let d = ServerFrame::decode(r#"{"type":"text_delta","room_id":"r","text":"좋아요, ","client_id":"c-1"}"#).unwrap();
        assert_eq!(d.event, ServerEvent::TextDelta("좋아요, ".into()));
        assert_eq!(d.client_id.as_deref(), Some("c-1"));
        let a = ServerFrame::decode(r#"{"type":"activity","room_id":"r","activity":{"kind":"tool","tool":"search_web"}}"#).unwrap();
        assert_eq!(a.event, ServerEvent::Activity { kind: ActivityKind::Tool, tool: Some("search_web".into()) });
        let done = ServerFrame::decode(r#"{"type":"done","room_id":"r","message_id":null,"stopped":true}"#).unwrap();
        assert_eq!(done.event, ServerEvent::Done { message_id: None, stopped: true });
    }

    #[test]
    fn unknown_type_and_garbage() {
        assert_eq!(ServerFrame::decode(r#"{"type":"card_update"}"#).unwrap().event, ServerEvent::Unknown { kind: "card_update".into() });
        assert!(ServerFrame::decode("not json").is_none());
        assert!(ServerFrame::decode(r#"{"no_type":1}"#).is_none());
        assert!(ServerFrame::decode("[1,2]").is_none());
        assert!(ServerFrame::decode(r#"{"type":5}"#).is_none());
    }

    #[test]
    fn unknown_activity_kind_falls_back_to_idle() {
        let a = ServerFrame::decode(r#"{"type":"activity","activity":{"kind":"dancing"}}"#).unwrap();
        assert_eq!(a.event, ServerEvent::Activity { kind: ActivityKind::Idle, tool: None });
    }

    #[test]
    fn encode_is_deterministic_json() {
        assert_eq!(ClientFrame::Ping.encode(), r#"{"type":"ping"}"#);
        assert_eq!(ClientFrame::Stop { room_id: "r".into() }.encode(), r#"{"room_id":"r","type":"stop"}"#);
        let m = ClientFrame::message("안녕 \"나초\"", Some("r"), "c-1").encode();
        let o = json::object_str(&m).unwrap();
        use crate::core::pet::json::JsonExt;
        assert_eq!(o.str("text").as_deref(), Some("안녕 \"나초\""));
        assert_eq!(o.str("client_msg_id").as_deref(), Some("c-1"));
        assert_eq!(o.str("client_id").as_deref(), Some("c-1"));
        assert_eq!(o.str("room_id").as_deref(), Some("r"));
    }

    // MARK: PetChatStateTests (PetChatTests.swift)

    fn frame(json: &str) -> ServerFrame { ServerFrame::decode(json).expect("frame") }

    fn connected() -> ChatState {
        let mut s = ChatState::new();
        s.apply(&frame(r#"{"type":"ready","user_id":"u","agent_id":"a","agent_name":"나초","room_id":"r"}"#));
        s
    }

    /// The PetFace asserts (speaking / happy / idle) are left out: the face is another module.
    #[test]
    fn streaming_turn_happy_path() {
        let mut s = connected();
        assert_eq!(s.phase, ConnectionPhase::Connected);
        s.begin_turn("안녕", "c-1", vec![]);
        assert_eq!(s.turns[0].status, TurnStatus::Sending);
        s.apply(&frame(r#"{"type":"turn_started","room_id":"r","turn_id":"t","duplicate":false,"client_id":"c-1"}"#));
        assert_eq!(s.turns[0].status, TurnStatus::Streaming);
        s.apply(&frame(r#"{"type":"text_delta","room_id":"r","text":"반가","client_id":"c-1"}"#));
        s.apply(&frame(r#"{"type":"text_delta","room_id":"r","text":"워요","client_id":"c-1"}"#));
        assert_eq!(s.turns[0].assistant_text, "반가워요");
        s.apply(&frame(r#"{"type":"message","room_id":"r","message":{"id":"m","role":"assistant","text":"반가워요!"},"client_id":"c-1"}"#));
        assert_eq!(s.turns[0].assistant_text, "반가워요!"); // committed message wins
        s.apply(&frame(r#"{"type":"done","room_id":"r","stopped":false,"client_id":"c-1"}"#));
        assert_eq!(s.turns[0].status, TurnStatus::Done);
        assert!(!s.is_busy());
        assert!(s.last_finished_at.is_some());
    }

    #[test]
    fn stop_keeps_partial_text() {
        let mut s = connected();
        s.begin_turn("길게", "c-1", vec![]);
        s.apply(&frame(r#"{"type":"text_delta","text":"문장 1. ","client_id":"c-1"}"#));
        s.mark_stop_requested();
        assert!(s.turns[0].stop_requested);
        s.apply(&frame(r#"{"type":"done","room_id":"r","stopped":true,"client_id":"c-1"}"#));
        assert_eq!(s.turns[0].status, TurnStatus::Stopped);
        assert_eq!(s.turns[0].assistant_text, "문장 1. ");
    }

    #[test]
    fn duplicate_turn_ends_without_new_text() {
        let mut s = connected();
        s.begin_turn("again", "c-1", vec![]);
        s.apply(&frame(r#"{"type":"turn_started","room_id":"r","duplicate":true,"client_id":"c-1"}"#));
        assert_eq!(s.turns[0].status, TurnStatus::Done);
    }

    #[test]
    fn turn_error_uses_korean_text() {
        let mut s = connected();
        s.begin_turn("x", "c-1", vec![]);
        s.apply(&frame(r#"{"type":"error","room_id":"r","code":"upstream_error","message":"external service openrouter: 500","client_id":"c-1"}"#));
        let TurnStatus::Failed(m) = &s.turns[0].status else { panic!("expected failed") };
        assert!(!m.contains("openrouter"));
        assert!(m.contains("다시 시도"));
    }

    #[test]
    fn pre_turn_rejection_becomes_notice_or_turn_failure() {
        let mut s = connected();
        s.apply(&frame(r#"{"type":"error","code":"bad_frame","message":"x"}"#)); // no room_id, no active turn
        assert!(s.notice.is_some());
        s.begin_turn("x", "c-2", vec![]);
        s.apply(&frame(r#"{"type":"error","code":"credits_exhausted","message":"x","client_id":"c-2"}"#));
        assert_eq!(s.turns[0].status, TurnStatus::Failed("사용량이 모두 소진됐어요.".into()));
    }

    #[test]
    fn other_sessions_turn_is_ignored() {
        let mut s = connected();
        s.begin_turn("mine", "c-1", vec![]);
        s.apply(&frame(r#"{"type":"text_delta","text":"not mine","client_id":"other"}"#));
        assert_eq!(s.turns[0].assistant_text, "");
    }

    #[test]
    fn connection_lost_interrupts_unfinished_turns_only() {
        let mut s = connected();
        s.begin_turn("a", "c-1", vec![]);
        s.apply(&frame(r#"{"type":"done","stopped":false,"client_id":"c-1"}"#));
        s.begin_turn("b", "c-2", vec![]);
        s.connection_lost();
        assert_eq!(s.turns[0].status, TurnStatus::Done);
        assert_eq!(s.turns[1].status, TurnStatus::Interrupted);
    }

    #[test]
    fn unauthorized_sets_auth_failed() {
        let mut s = ChatState::new();
        s.apply(&frame(r#"{"type":"error","code":"unauthorized","message":"x"}"#));
        assert_eq!(s.phase, ConnectionPhase::AuthFailed);
    }

    /// Only the state half: the PetFace asserts (listening / thinking, 8 faces) belong to the face module.
    #[test]
    fn pet_states() {
        let mut s = connected();
        s.begin_turn("q", "c-1", vec![]);
        assert!(s.is_busy());
        s.apply(&frame(r#"{"type":"activity","activity":{"kind":"tool","tool":"search_web"}}"#));
        assert_eq!(s.active_tool.as_deref(), Some("search_web"));
        assert_eq!(s.activity, ActivityKind::Tool);
    }

    #[test]
    fn outgoing_text_validation() {
        assert_eq!(OutgoingText::validate("  \n ", false), Err(OutgoingProblem::Empty));
        assert_eq!(OutgoingText::validate("  hi \0 ", false), Ok("hi".into()));
        let ok = "가".repeat(OutgoingText::MAX_CODE_POINTS);
        assert!(OutgoingText::validate(&ok, false).is_ok());
        assert_eq!(OutgoingText::validate(&format!("{ok}가"), false), Err(OutgoingProblem::TooLong));
        // astral characters count as one code point each
        let emoji = "😀".repeat(OutgoingText::MAX_CODE_POINTS);
        assert!(OutgoingText::validate(&emoji, false).is_ok());
        assert_eq!(OutgoingText::validate(&format!("{emoji}😀"), false), Err(OutgoingProblem::TooLong));
    }

    // MARK: PetReconnectPolicyTests (PetChatTests.swift)

    #[test]
    fn backoff_doubles_and_caps() {
        let p = ReconnectPolicy { base_delay: 1.0, max_delay: 30.0, ..Default::default() };
        assert_eq!(p.delay(1, 1.0), 1.0);
        assert_eq!(p.delay(2, 1.0), 2.0);
        assert_eq!(p.delay(5, 1.0), 16.0);
        assert_eq!(p.delay(6, 1.0), 30.0);
        assert_eq!(p.delay(99, 1.0), 30.0);
        assert_eq!(p.delay(3, 0.0), 2.0); // 50% floor
        assert_eq!(p.delay(3, 5.0), 4.0); // clamped
    }

    #[test]
    fn auth_failures_and_limit_stop_retrying() {
        let p = ReconnectPolicy { max_attempts: 3, ..Default::default() };
        assert!(!p.should_retry(1, &TransportFailure::new(Some(401), "")));
        assert!(!p.should_retry(1, &TransportFailure::new(Some(403), "")));
        assert!(p.should_retry(3, &TransportFailure::new(None, "net")));
        assert!(!p.should_retry(4, &TransportFailure::new(None, "net")));
    }

    // MARK: PetChatClientTests (PetChatTests.swift): fake transport + manual timers

    const READY: &str = r#"{"type":"ready","user_id":"u","agent_id":"a","agent_name":"나초","room_id":"r"}"#;

    fn make_client(max_attempts: u32) -> (ChatClient, Arc<FakeNetwork>, Arc<ManualSchedule>) {
        let net = FakeNetwork::new();
        let timers = ManualSchedule::new();
        let ep = ServerEndpoint::parse("http://127.0.0.1:8787").unwrap();
        let opts = ChatClientOptions {
            policy: ReconnectPolicy { base_delay: 1.0, max_delay: 30.0, max_attempts },
            jitter: Arc::new(|| 1.0),
            ..Default::default()
        };
        let c = ChatClient::new(ep, Arc::new(|| Some("mock-t".into())), net.make(), timers.schedule(), opts);
        (c, net, timers)
    }

    fn fail(msg: &str) -> Option<TransportFailure> { Some(TransportFailure::new(None, msg)) }

    #[test]
    fn connect_uses_ws_url_and_bearer_then_ready() {
        let (c, net, _) = make_client(3);
        let seen = Arc::new(Mutex::new(vec![]));
        let s2 = seen.clone();
        c.set_on_change(Some(Arc::new(move |st: &ChatState| s2.lock().unwrap().push(st.phase))));
        c.connect();
        assert_eq!(net.count(), 1);
        assert_eq!(net.get(0).url.lock().unwrap().as_deref(), Some("ws://127.0.0.1:8787/ws/chat"));
        assert_eq!(net.get(0).bearer.lock().unwrap().as_deref(), Some("mock-t"));
        assert_eq!(c.state().phase, ConnectionPhase::Connecting);
        net.get(0).deliver(READY);
        assert_eq!(c.state().phase, ConnectionPhase::Connected);
        assert_eq!(c.state().room_id.as_deref(), Some("r"));
        assert_eq!(*seen.lock().unwrap(), vec![ConnectionPhase::Connecting, ConnectionPhase::Connected]);
    }

    #[test]
    fn send_refusals() {
        let (c, net, _) = make_client(3);
        assert_eq!(c.send("hi", &[]), Err(SendRefusal::NotConnected));
        assert_eq!(c.send("  ", &[]), Err(SendRefusal::InvalidText(OutgoingProblem::Empty)));
        c.connect();
        net.get(0).deliver(READY);
        let id = c.send("안녕", &[]).expect("send");
        assert!(id.starts_with("g-"));
        assert_eq!(id.len(), 14);
        assert_eq!(c.send("again", &[]), Err(SendRefusal::Busy));
        let frame = net.get(0).last_sent().unwrap();
        assert!(frame.contains(&format!("\"client_msg_id\":\"{id}\"")));
        assert!(frame.contains("\"room_id\":\"r\""));
        let five = vec![PetAttachment::default(); 5];
        assert_eq!(c.send("x", &five), Err(SendRefusal::TooManyAttachments));
    }

    #[test]
    fn stop_sends_frame_only_while_busy() {
        let (c, net, _) = make_client(3);
        c.connect();
        net.get(0).deliver(READY);
        c.stop();
        assert!(net.get(0).sent().is_empty());
        let _ = c.send("긴 답", &[]);
        c.stop();
        assert_eq!(net.get(0).last_sent().as_deref(), Some(r#"{"room_id":"r","type":"stop"}"#));
        assert!(c.state().turns[0].stop_requested);
    }

    #[test]
    fn drop_mid_turn_reconnects_with_backoff_and_resets_on_ready() {
        let (c, net, timers) = make_client(3);
        c.connect();
        net.get(0).deliver(READY);
        let _ = c.send("q", &[]);
        net.get(0).drop_with(fail("reset"));
        assert_eq!(c.state().phase, ConnectionPhase::Reconnecting { attempt: 1 });
        assert_eq!(c.state().turns[0].status, TurnStatus::Interrupted);
        assert_eq!(timers.last_delay(), Some(1.0)); // (an earlier entry is the ping timer)
        timers.fire_last();
        assert_eq!(net.count(), 2);
        net.get(1).drop_with(fail("refused"));
        assert_eq!(c.state().phase, ConnectionPhase::Reconnecting { attempt: 2 });
        assert_eq!(timers.last_delay(), Some(2.0));
        timers.fire_last();
        net.get(2).deliver(READY);
        assert_eq!(c.state().phase, ConnectionPhase::Connected);
        // after a successful ready the counter restarts at 1
        net.get(2).drop_with(None);
        assert_eq!(c.state().phase, ConnectionPhase::Reconnecting { attempt: 1 });
    }

    #[test]
    fn stale_transport_events_are_ignored() {
        let (c, net, timers) = make_client(3);
        c.connect();
        net.get(0).drop_with(fail("x"));
        timers.fire(0);
        net.get(0).deliver(READY); // late frame from the dead connection
        assert_ne!(c.state().phase, ConnectionPhase::Connected);
    }

    #[test]
    fn http401_stops_retrying_and_shows_login_notice() {
        let (c, net, timers) = make_client(3);
        c.connect();
        net.get(0).drop_with(Some(TransportFailure::new(Some(401), "401")));
        assert_eq!(c.state().phase, ConnectionPhase::AuthFailed);
        assert_eq!(c.state().notice.as_deref(), Some("로그인이 필요해요."));
        assert!(timers.is_empty());
    }

    #[test]
    fn gives_up_after_max_attempts() {
        let (c, net, timers) = make_client(2);
        c.connect();
        for n in 0..3 {
            net.get(n).drop_with(fail("down"));
            if n < 2 { timers.fire_last(); }
        }
        assert_eq!(c.state().phase, ConnectionPhase::Disconnected);
        assert!(c.state().notice.is_some());
    }

    #[test]
    fn disconnect_closes_and_cancels_retry() {
        let (c, net, timers) = make_client(3);
        c.connect();
        net.get(0).drop_with(fail("x"));
        c.disconnect();
        timers.fire(0); // timer fires after the user disconnected
        assert_eq!(net.count(), 1);
        assert_eq!(c.state().phase, ConnectionPhase::Disconnected);
    }

    #[test]
    fn ping_scheduled_after_ready() {
        let (c, net, timers) = make_client(3);
        c.connect();
        net.get(0).deliver(READY);
        assert_eq!(timers.last_delay(), Some(30.0));
        timers.fire_last();
        assert_eq!(net.get(0).last_sent().as_deref(), Some(r#"{"type":"ping"}"#));
        assert_eq!(timers.last_delay(), Some(30.0), "the next ping is scheduled");
    }

    // Rust-only checks of the threading model.

    #[test]
    fn read_mark_update_and_drop_close() {
        let (c, net, _) = make_client(3);
        c.mark_read(5); // not connected: nothing
        c.connect();
        net.get(0).deliver(READY);
        c.mark_read(0);
        assert!(net.get(0).sent().is_empty());
        c.mark_read(7);
        assert_eq!(net.get(0).last_sent().as_deref(), Some(r#"{"room_id":"r","seq":7,"type":"read"}"#));
        c.update(|s| s.notice = Some("x".into()));
        assert_eq!(c.state().notice.as_deref(), Some("x"));
        let t = net.get(0);
        drop(c);
        assert!(t.closed.load(Ordering::SeqCst));
        t.deliver(READY); // the client is gone: nothing happens
    }

    #[test]
    fn on_change_may_call_back_into_the_client() {
        let (c, net, _) = make_client(3);
        let c = Arc::new(c);
        let weak = Arc::downgrade(&c);
        let phases = Arc::new(Mutex::new(vec![]));
        let p2 = phases.clone();
        c.set_on_change(Some(Arc::new(move |st: &ChatState| {
            if let Some(c) = weak.upgrade() { p2.lock().unwrap().push(c.state().phase); } // no deadlock
            let _ = st;
        })));
        c.connect();
        net.get(0).deliver(READY);
        assert_eq!(phases.lock().unwrap().last(), Some(&ConnectionPhase::Connected));
    }

    #[test]
    fn events_from_another_thread() {
        let (c, net, _) = make_client(3);
        c.connect();
        let t = net.get(0);
        std::thread::spawn(move || t.deliver(READY)).join().unwrap();
        assert_eq!(c.state().phase, ConnectionPhase::Connected);
    }
}
