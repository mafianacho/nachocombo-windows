//! The pet's brain, shared by the app and the dev CLI: login → `/ws/chat` → uploads → `/ws/connector`. Port of
//! `PetSession.swift`.
//!
//! Nothing connects at launch: the first open of the chat (or a send) connects, so starting the PC does not log in
//! anywhere or ping the phone with "connector connected". A stored login is read at construction (no network), so a
//! restart is signed in without a new login. All state changes run on one worker thread (the Swift `queue`): public
//! methods post a job and return; `on_change` is called on that thread after every change (the shell hops to its UI
//! thread). [`PetSession::snapshot`] reads a copy that is kept up to date and never blocks on the worker.
use super::auth::{AuthProblem, Clock, OIDCConfig, PetAuth, PetAuthBrowser, RandomBytesSource};
use super::chat_client::{ChatClient, ChatClientOptions, MakeTransport, OutgoingText, Schedule, SendRefusal};
use super::chat_state::{ChatState, ConnectionPhase, ErrorText};
use super::connector_client::{ConnectorBinding, ConnectorPhase, PetCommandExecuting, PetConnectorClient};
use super::connectors::{ConnectorError, ConnectorsAPI, OverviewCompletion};
use super::http::PetHTTPClient;
use super::media::{MediaProblem, PetAttachment, PetMedia, PetOutgoingFile};
use super::secrets::{PetSecretLookup, PetSecretStore};
use super::selection::{PetSelectionCompletion, PetSelectionSending, PetSelectionSent};
use super::server_api::{PetAPIError, PetServerAPI, PetServerCalls, PetTaskStart};
use super::server_models::{PetClientConfig, PetTask};
use super::settings::{PetSettings, ServerEndpoint};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum PetAuthPhase {
    #[default]
    SignedOut,
    SigningIn,
    SignedIn,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetPendingFile {
    pub id: u64,
    pub file: PetOutgoingFile,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PetSessionState {
    pub auth: PetAuthPhase,
    pub chat: ChatState,
    pub connector: ConnectorPhase,
    pub pending: Vec<PetPendingFile>,
    pub uploading: bool,
    /// Last message for the user that is not part of a turn (Korean).
    pub notice: Option<String>,
    /// Unix seconds.
    pub last_interaction: f64,
    /// `GET /client/config` (dictation on/off and limits, service status). None until loaded.
    pub config: Option<PetClientConfig>,
    pub loading_history: bool,
    pub dictating: bool,
    /// Requests from the selection bar by the turn's client message id (chip, 다시, 보낸 내용 보기). Memory only.
    pub selection_turns: BTreeMap<String, PetSelectionSent>,
    /// Turns the user spoke (hold the pet to talk), by client message id: their answer's first sentence may be read.
    pub voice_turns: BTreeSet<String>,
    /// Text sent while Nacho was answering: waits here and goes out when the answer ends (one slot; more text is appended).
    pub queued_text: Option<String>,
    /// A queued text the automatic send could not deliver: the window moves it back into the input field.
    pub returned_text: Option<String>,
}

impl Default for PetSessionState {
    fn default() -> Self {
        Self {
            auth: PetAuthPhase::SignedOut,
            chat: ChatState::default(),
            connector: ConnectorPhase::Off,
            pending: vec![],
            uploading: false,
            notice: None,
            last_interaction: 0.0,
            config: None,
            loading_history: false,
            dictating: false,
            selection_turns: BTreeMap::new(),
            voice_turns: BTreeSet::new(),
            queued_text: None,
            returned_text: None,
        }
    }
}

pub type SessionChange = Arc<dyn Fn(&PetSessionState) + Send + Sync>;
/// Korean refusal text, or None when it went out.
pub type RefusalCompletion = Box<dyn FnOnce(Option<String>) + Send>;

type Job = Box<dyn FnOnce(&mut Inner) + Send>;

/// Posts jobs to the session's worker thread. Held weakly by everything the session hands out, so dropping the
/// session ends the worker.
#[derive(Clone)]
struct Poster(Weak<Mutex<mpsc::Sender<Job>>>);

impl Poster {
    fn post(&self, job: impl FnOnce(&mut Inner) + Send + 'static) {
        if let Some(tx) = self.0.upgrade() { let _ = tx.lock().unwrap().send(Box::new(job)); }
    }
}

/// What the session is built from. `executor` is held weakly (the shell owns it).
pub struct PetSessionDeps {
    pub http: Arc<dyn PetHTTPClient>,
    pub secrets: Arc<dyn PetSecretStore>,
    pub browser: Option<Arc<dyn PetAuthBrowser>>,
    pub make_transport: MakeTransport,
    pub executor: Option<Weak<dyn PetCommandExecuting>>,
    /// OS randomness (PKCE).
    pub random: RandomBytesSource,
    pub now: Clock,
    pub schedule: Schedule,
    /// The binding label the server shows for this device (`ConnectorBinding::LABEL_WINDOWS`).
    pub connector_label: String,
}

struct Inner {
    post: Poster,
    state: PetSessionState,
    mirror: Arc<Mutex<PetSessionState>>,
    on_change: Option<SessionChange>,
    settings: PetSettings,
    deps: PetSessionDeps,
    auth: PetAuth,
    /// Bumped whenever `auth` is replaced: callbacks of an older auth are dropped.
    auth_gen: u64,
    chat: Option<Arc<ChatClient>>,
    chat_gen: u64,
    connector: Option<Arc<PetConnectorClient>>,
    connector_gen: u64,
    connector_agent: Option<String>,
    connector_reissued: bool,
    connector_capabilities: Option<Vec<Value>>,
    refresh_tried: bool,
    next_pending_id: u64,
    opened: bool,
    /// Tasks whose [시작]/[취소] request is out: a second press sends nothing.
    task_actions_in_flight: BTreeSet<String>,
    /// Plan-note tasks already looked up with `GET /tasks/{id}` (once each).
    task_lookups: BTreeSet<String>,
}

pub struct PetSession {
    tx: Arc<Mutex<mpsc::Sender<Job>>>,
    mirror: Arc<Mutex<PetSessionState>>,
    worker: std::thread::ThreadId,
}

impl PetSession {
    /// Seconds the server keeps a forgotten memory restorable (spec/memory.tsp: 10 s).
    pub const RESTORE_WINDOW: f64 = 10.0;

    pub fn new(settings: PetSettings, deps: PetSessionDeps) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let tx = Arc::new(Mutex::new(tx));
        let post = Poster(Arc::downgrade(&tx));
        let auth = PetAuth::new(settings.oidc(), &settings.account_scope(), deps.http.clone(), deps.secrets.clone(), deps.random.clone(), deps.now.clone());
        let mut state = PetSessionState { last_interaction: (deps.now)(), ..Default::default() };
        Inner::restore_auth_phase(&auth, &mut state);
        let mirror = Arc::new(Mutex::new(state.clone()));
        let mut inner = Inner {
            post,
            state,
            mirror: mirror.clone(),
            on_change: None,
            settings,
            deps,
            auth,
            auth_gen: 0,
            chat: None,
            chat_gen: 0,
            connector: None,
            connector_gen: 0,
            connector_agent: None,
            connector_reissued: false,
            connector_capabilities: None,
            refresh_tried: false,
            next_pending_id: 1,
            opened: false,
            task_actions_in_flight: BTreeSet::new(),
            task_lookups: BTreeSet::new(),
        };
        let handle = std::thread::Builder::new()
            .name("nacho-pet-session".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() { job(&mut inner); }
                inner.drop_clients(false);
            })
            .expect("session thread");
        Self { tx, mirror, worker: handle.thread().id() }
    }

    fn post(&self, job: impl FnOnce(&mut Inner) + Send + 'static) { let _ = self.tx.lock().unwrap().send(Box::new(job)); }

    /// Runs `job` on the worker and waits for its result. Must not be called from `on_change` (the worker thread).
    fn sync<R: Send + 'static>(&self, job: impl FnOnce(&mut Inner) -> R + Send + 'static) -> R {
        assert_ne!(std::thread::current().id(), self.worker, "PetSession::sync called on the session thread");
        let (rtx, rrx) = mpsc::channel();
        self.post(move |i| { let _ = rtx.send(job(i)); });
        rrx.recv().expect("session thread ended")
    }

    /// Waits until queued work ran (tests, the CLI).
    pub fn flush(&self) { self.sync(|_| ()) }

    /// The latest state (never blocks on the worker; safe from `on_change`).
    pub fn snapshot(&self) -> PetSessionState { self.mirror.lock().unwrap().clone() }

    pub fn settings(&self) -> PetSettings { self.sync(|i| i.settings.clone()) }

    /// Called on the session thread after every change.
    pub fn set_on_change(&self, f: Option<SessionChange>) { self.post(move |i| i.on_change = f) }

    /// The capabilities the connector declares (W3: what this PC really runs). None = the core's full list.
    pub fn set_connector_capabilities(&self, caps: Option<Vec<Value>>) {
        self.post(move |i| {
            i.connector_capabilities = caps.clone();
            if let (Some(c), Some(caps)) = (&i.connector, caps) { c.declare(caps); }
        })
    }

    // MARK: lifecycle

    /// The chat was opened: connect if signed in (once; later calls only refresh the interaction time).
    pub fn open(&self) {
        self.post(|i| {
            i.state.last_interaction = (i.deps.now)();
            i.opened = true;
            i.retry_refused_login();
            if i.state.auth == PetAuthPhase::SignedIn && i.chat.is_none() { i.connect_chat(); }
            i.publish();
        })
    }

    pub fn touch(&self) {
        self.post(|i| {
            i.state.last_interaction = (i.deps.now)();
            i.publish();
        })
    }

    pub fn reconnect(&self) {
        self.post(|i| {
            i.refresh_tried = false;
            i.drop_clients(false);
            i.retry_refused_login();
            if i.state.auth == PetAuthPhase::SignedIn {
                i.connect_chat();
            } else if i.auth.secrets_unavailable() {
                i.state.notice = Some(AuthProblem::SecretsUnavailable.korean_message().into());
            } else {
                i.state.notice = Some(AuthProblem::SignedOut.korean_message().into());
            }
            i.publish();
        })
    }

    /// Everything off: sockets closed, nothing scheduled. Tokens stay (signing out is separate).
    pub fn shutdown(&self) {
        self.sync(|i| {
            i.opened = false;
            i.drop_clients(false);
            i.state.pending.clear();
            i.state.queued_text = None;
            i.state.selection_turns.clear();
            i.state.uploading = false;
            i.publish();
        })
    }

    /// New server or realm: other tokens apply, so everything restarts.
    pub fn apply_settings(&self, new: PetSettings) {
        self.post(move |i| {
            let scope_changed = new.account_scope() != i.settings.account_scope();
            i.settings = new.clone();
            if scope_changed {
                i.drop_clients(false);
                i.auth = PetAuth::new(new.oidc(), &new.account_scope(), i.deps.http.clone(), i.deps.secrets.clone(), i.deps.random.clone(), i.deps.now.clone());
                i.auth_gen += 1;
                i.state = PetSessionState { last_interaction: (i.deps.now)(), ..Default::default() };
                Inner::restore_auth_phase(&i.auth, &mut i.state);
                if i.opened && i.state.auth == PetAuthPhase::SignedIn { i.connect_chat(); }
            } else if !new.allow_combo_control {
                i.drop_connector();
            } else if let (Some(agent), ConnectionPhase::Connected) = (i.state.chat.agent_id.clone(), i.state.chat.phase) {
                i.ensure_connector(&agent);
            }
            i.publish();
        })
    }

    // MARK: login

    pub fn sign_in(&self) { self.post(|i| i.sign_in()) }

    pub fn sign_out(&self) { self.post(|i| i.sign_out()) }

    /// A fresh access token (None when signed out) for a one-off REST call such as `GET /api/v1/me`.
    pub fn with_access_token(&self, body: impl FnOnce(Option<String>) + Send + 'static) {
        self.post(move |i| i.auth.valid_access_token(Box::new(move |r| body(r.ok()))))
    }

    // MARK: attachments

    /// Adds a file for the next message. Returns why it was refused (Korean), or None.
    pub fn attach(&self, file: PetOutgoingFile) -> Option<String> {
        self.sync(move |i| {
            if i.state.pending.len() >= PetMedia::MAX_ATTACHMENTS { return Some(MediaProblem::TooMany.korean_message()); }
            i.state.pending.push(PetPendingFile { id: i.next_pending_id, file });
            i.next_pending_id += 1;
            i.state.last_interaction = (i.deps.now)();
            i.state.notice = None;
            i.publish();
            None
        })
    }

    pub fn remove_pending(&self, id: u64) {
        self.post(move |i| {
            i.state.pending.retain(|p| p.id != id);
            i.publish();
        })
    }

    // MARK: send / stop

    /// Uploads the waiting files, then sends one message. `completion` gets the refusal text (Korean) or None. `voice`:
    /// the text came from hold-to-talk (the turn is remembered in `voice_turns`).
    pub fn send(&self, text: &str, voice: bool, completion: Option<RefusalCompletion>) {
        let text = text.to_string();
        self.post(move |i| i.send(text, voice, completion))
    }

    /// Send while Nacho is answering: the text waits (never stops the answer) and goes out when the turn ends. Not busy:
    /// a plain `send`.
    pub fn send_or_queue(&self, text: &str, completion: Option<RefusalCompletion>) {
        let text = text.to_string();
        self.post(move |i| {
            if i.state.auth != PetAuthPhase::SignedIn || !i.state.chat.is_busy() { return i.send(text, false, completion); }
            i.state.last_interaction = (i.deps.now)();
            if let Err(p) = OutgoingText::validate(&text, !i.state.pending.is_empty()) {
                let m = p.korean_message();
                i.state.notice = Some(m.clone());
                i.publish();
                if let Some(c) = completion { c(Some(m)) }
                return;
            }
            i.state.queued_text = match i.state.queued_text.take() {
                Some(held) if !held.is_empty() => Some(if text.is_empty() { held } else { format!("{held}\n{text}") }),
                _ => Some(text),
            };
            i.state.notice = None;
            i.publish();
            if let Some(c) = completion { c(None) }
        })
    }

    /// Drops the waiting text: it is not sent.
    pub fn cancel_queued(&self) {
        self.post(|i| {
            if i.state.queued_text.take().is_some() { i.publish(); }
        })
    }

    /// The window took back the text the automatic send could not deliver.
    pub fn clear_returned(&self) {
        self.post(|i| {
            if i.state.returned_text.take().is_some() { i.publish(); }
        })
    }

    pub fn stop(&self) {
        self.post(|i| {
            if let Some(c) = &i.chat { c.stop(); }
        })
    }

    /// A selection-bar request: uploads only its context file (files waiting in the composer stay there) and sends the
    /// instruction. The turn is remembered for its chip and the answer's actions.
    pub fn send_selection(&self, sent: PetSelectionSent, completion: RefusalCompletion) { self.post(move |i| i.send_selection(sent, completion)) }

    /// Commands Nacho ran on this PC (diagnostics).
    pub fn handled_commands(&self) -> Vec<String> { self.sync(|i| i.connector.as_ref().map(|c| c.handled()).unwrap_or_default()) }

    // MARK: connections screen

    pub fn load_connectors(&self, completion: OverviewCompletion) {
        self.post(move |i| {
            let Some(endpoint) = i.settings.endpoint() else { return completion(Err(ConnectorError::BadResponse)) };
            let http = i.deps.http.clone();
            i.auth.valid_access_token(Box::new(move |tok| match tok {
                Err(_) => completion(Err(ConnectorError::Unauthorized)),
                Ok(token) => ConnectorsAPI::new(endpoint, http, Arc::new(move || Some(token.clone()))).load(completion),
            }));
        })
    }

    // MARK: server features besides the socket

    /// "이전 대화 더 보기": the page before the oldest message shown.
    pub fn load_older_history(&self) {
        self.post(|i| {
            let Some(room) = i.state.chat.room_id.clone().filter(|r| !r.is_empty()) else { return };
            if !i.state.chat.has_more_history { return; }
            let oldest = i.state.chat.history.iter().filter_map(|m| m.seq).min().or_else(|| i.state.chat.turns.iter().filter_map(|t| t.user_seq).min());
            i.load_history(room, oldest);
        })
    }

    pub fn refresh_usage(&self) { self.post(|i| i.refresh_usage()) }

    /// Server dictation: recorded audio → text for the input field (not sent by itself). Free; the server limits it per
    /// minute/day. `completion` (on the session thread): the text, or the Korean reason.
    pub fn dictate(&self, audio: Vec<u8>, mime: &str, file_name: &str, duration_sec: Option<f64>, completion: Box<dyn FnOnce(Result<String, PetAPIError>) + Send>) {
        let (mime, file_name) = (mime.to_string(), file_name.to_string());
        self.post(move |i| {
            // Hold-to-talk can come before the chat was ever opened: read the server's dictation settings first.
            match (i.state.config.is_none(), i.settings.endpoint()) {
                (true, Some(endpoint)) => {
                    let post = i.post.clone();
                    PetServerCalls::new(endpoint, i.deps.http.clone(), None).client_config(Box::new(move |r| {
                        post.post(move |i| {
                            if let Ok(c) = r { i.state.config = Some(c); }
                            i.dictate_now(audio, mime, file_name, duration_sec, completion);
                        })
                    }));
                }
                _ => i.dictate_now(audio, mime, file_name, duration_sec, completion),
            }
        })
    }

    /// "잊어줘" on a "기억했어요" line: forgets its memories (undo possible for 10 seconds).
    pub fn forget_memory(&self, update_id: &str) {
        let update_id = update_id.to_string();
        self.post(move |i| i.forget_memory(update_id))
    }

    pub fn can_undo_forget(&self, update_id: &str) -> bool {
        let update_id = update_id.to_string();
        self.sync(move |i| {
            let at = i.state.chat.memory_notes.iter().find(|n| n.update_id == update_id).and_then(|n| n.forgotten_at);
            at.is_some_and(|at| (i.deps.now)() - at < Self::RESTORE_WINDOW)
        })
    }

    pub fn undo_forget(&self, update_id: &str) {
        let update_id = update_id.to_string();
        self.post(move |i| i.undo_forget(update_id))
    }

    /// "기억이 틀렸어요" under an answer: a correction about the memories that answer used (or general feedback).
    pub fn send_memory_feedback(&self, turn_id: &str, text: &str, completion: Option<RefusalCompletion>) {
        let (turn_id, text) = (turn_id.to_string(), text.to_string());
        self.post(move |i| i.send_memory_feedback(turn_id, text, completion))
    }

    /// Answers an approval / choice card: approve | deny | choose (+ options) | dismiss, optionally with an allow rule.
    pub fn answer_card(&self, id: &str, action: &str, option_ids: Vec<String>, allow_rule_id: Option<String>, completion: Option<RefusalCompletion>) {
        let (id, action) = (id.to_string(), action.to_string());
        self.post(move |i| i.answer_card(id, action, option_ids, allow_rule_id, completion))
    }

    /// [시작] on a plan card: `POST /tasks/{id}/start {expected_estimate_id}` with the estimate shown on the card.
    /// `completion`: None when started, else the one line shown (also kept under the card).
    pub fn start_task(&self, id: &str, completion: Option<RefusalCompletion>) {
        let id = id.to_string();
        self.post(move |i| i.start_task(id, completion))
    }

    /// [취소] on a plan card: `POST /tasks/{id}/cancel`.
    pub fn cancel_task(&self, id: &str, completion: Option<RefusalCompletion>) {
        let id = id.to_string();
        self.post(move |i| i.cancel_task(id, completion))
    }
}

impl PetSelectionSending for PetSession {
    fn send_selection(&self, sent: PetSelectionSent, completion: PetSelectionCompletion) { PetSession::send_selection(self, sent, completion) }
}

fn done(c: Option<RefusalCompletion>, v: Option<String>) {
    if let Some(c) = c { c(v) }
}

impl Inner {
    /// Answers `c` after the work already queued on the session thread. A change made through the chat client (a sent
    /// turn, a card or task update) reaches `state` as a posted job, so answering at once would let a caller read a
    /// `snapshot()` without it (the Swift `snapshot` was a `queue.sync` read and waited for that job).
    fn done_after_queued(&self, c: Option<RefusalCompletion>, v: Option<String>) {
        if let Some(c) = c { self.post.post(move |_| c(v)); }
    }

    fn publish(&mut self) {
        *self.mirror.lock().unwrap() = self.state.clone();
        if let Some(f) = self.on_change.clone() { f(&self.state); }
    }

    /// The phase of the stored login. A refused credential store is not "signed out": it shows why and keeps the login.
    fn restore_auth_phase(auth: &PetAuth, state: &mut PetSessionState) {
        if auth.is_signed_in() {
            state.auth = PetAuthPhase::SignedIn;
        } else if auth.secrets_unavailable() {
            let m = AuthProblem::SecretsUnavailable.korean_message().to_string();
            state.auth = PetAuthPhase::Failed(m.clone());
            state.notice = Some(m);
        } else {
            state.auth = PetAuthPhase::SignedOut;
        }
    }

    /// The next access after a refused store read asks again. True when the stored login is back.
    fn retry_refused_login(&mut self) -> bool {
        if !self.auth.secrets_unavailable() || !self.auth.retry_stored_tokens() { return false; }
        self.state.auth = PetAuthPhase::SignedIn;
        self.state.notice = None;
        self.refresh_tried = false;
        true
    }

    fn drop_connector(&mut self) {
        if let Some(c) = self.connector.take() { c.disconnect(); }
        self.connector_gen += 1;
        self.connector_agent = None;
        self.state.connector = ConnectorPhase::Off;
    }

    fn drop_clients(&mut self, keep_chat: bool) {
        self.drop_connector();
        if !keep_chat {
            if let Some(c) = self.chat.take() {
                c.set_on_change(None);
                c.disconnect();
            }
            self.chat_gen += 1;
            self.state.chat = ChatState::default();
            self.task_actions_in_flight.clear();
            self.task_lookups.clear();
        }
    }

    // MARK: login

    fn sign_in(&mut self) {
        if self.state.auth == PetAuthPhase::SigningIn { return; }
        if self.retry_refused_login() {
            // The credential store handed out the stored login this time: no login page.
            self.opened = true;
            if self.chat.is_none() { self.connect_chat(); }
            return self.publish();
        }
        let Some(browser) = self.deps.browser.clone() else {
            self.state.notice = Some(AuthProblem::NotConfigured.korean_message().into());
            return self.publish();
        };
        match self.auth.begin() {
            Err(p) => {
                self.state.auth = PetAuthPhase::Failed(p.korean_message().into());
                self.publish();
            }
            Ok(pending) => {
                self.state.auth = PetAuthPhase::SigningIn;
                self.state.notice = None;
                self.publish();
                let (post, gen, auth) = (self.post.clone(), self.auth_gen, self.auth.clone());
                browser.authorize(&pending.url.clone(), OIDCConfig::CALLBACK_SCHEME, Box::new(move |result| {
                    post.clone().post(move |i| {
                        if i.auth_gen != gen { return; } // settings changed meanwhile
                        match result {
                            Err(p) => i.sign_in_failed(p),
                            Ok(url) => auth.complete(&url, &pending, Box::new(move |r| {
                                post.post(move |i| {
                                    if i.auth_gen != gen { return; }
                                    match r {
                                        Ok(()) => {
                                            i.state.auth = PetAuthPhase::SignedIn;
                                            i.state.notice = None;
                                            i.refresh_tried = false;
                                            i.opened = true;
                                            i.connect_chat();
                                            i.publish();
                                        }
                                        Err(p) => i.sign_in_failed(p),
                                    }
                                })
                            })),
                        }
                    })
                }));
            }
        }
    }

    fn sign_in_failed(&mut self, p: AuthProblem) {
        self.state.auth = if p == AuthProblem::Canceled { PetAuthPhase::SignedOut } else { PetAuthPhase::Failed(p.korean_message().into()) };
        self.state.notice = Some(p.korean_message().into());
        self.publish();
    }

    fn sign_out(&mut self) {
        let scope = self.settings.account_scope();
        let stored = self.deps.secrets.read(&ConnectorBinding::agent_key(&scope)).and_then(|d| String::from_utf8(d).ok());
        let agents: BTreeSet<String> = [self.connector_agent.clone(), self.state.chat.agent_id.clone(), stored].into_iter().flatten().collect();
        for agent in agents { self.deps.secrets.delete(&ConnectorBinding::secret_key(&scope, &agent)); }
        self.deps.secrets.delete(&ConnectorBinding::agent_key(&scope));
        self.drop_clients(false);
        self.state.pending.clear();
        self.state.queued_text = None;
        self.state.selection_turns.clear();
        self.state.auth = PetAuthPhase::SignedOut;
        self.state.notice = Some("로그아웃했어요.".into());
        self.auth.sign_out(Box::new(|| {}));
        self.publish();
    }

    // MARK: chat

    fn connect_chat(&mut self) {
        let Some(endpoint) = self.settings.endpoint() else {
            self.state.notice = Some("서버 주소가 올바르지 않아요. 설정에서 확인해 주세요.".into());
            return;
        };
        if let Some(c) = self.chat.take() {
            c.set_on_change(None);
            c.disconnect();
        }
        self.chat_gen += 1;
        let (post, gen, chat_gen) = (self.post.clone(), self.auth_gen, self.chat_gen);
        self.auth.valid_access_token(Box::new(move |result| {
            post.post(move |i| {
                if i.auth_gen != gen || i.chat_gen != chat_gen || i.chat.is_some() { return; }
                match result {
                    Ok(_) => {
                        let auth = i.auth.clone();
                        let c = Arc::new(ChatClient::new(
                            endpoint,
                            Arc::new(move || auth.current_access_token()),
                            i.deps.make_transport.clone(),
                            i.deps.schedule.clone(),
                            ChatClientOptions::default(),
                        ));
                        let post = i.post.clone();
                        c.set_on_change(Some(Arc::new(move |st: &ChatState| {
                            let st = st.clone();
                            post.post(move |i| {
                                if i.chat_gen == chat_gen && i.chat.is_some() { i.chat_changed(st); }
                            })
                        })));
                        i.chat = Some(c.clone());
                        c.connect();
                    }
                    Err(AuthProblem::Network) => {
                        i.state.notice = Some(AuthProblem::Network.korean_message().into());
                        i.publish();
                    }
                    Err(AuthProblem::SecretsUnavailable) => {
                        let m = AuthProblem::SecretsUnavailable.korean_message().to_string();
                        i.state.auth = PetAuthPhase::Failed(m.clone());
                        i.state.notice = Some(m);
                        i.publish();
                    }
                    Err(p) => {
                        i.state.auth = PetAuthPhase::SignedOut;
                        i.state.notice = Some(if p == AuthProblem::SignedOut { p.korean_message().into() } else { AuthProblem::Rejected(String::new()).korean_message().into() });
                        i.publish();
                    }
                }
            })
        }));
    }

    fn chat_changed(&mut self, st: ChatState) {
        let was_done = self.state.chat.last_finished_at;
        let was_connected = self.state.chat.phase == ConnectionPhase::Connected;
        let was_busy = self.state.chat.is_busy();
        let (busy, phase, finished, agent) = (st.is_busy(), st.phase, st.last_finished_at, st.agent_id.clone());
        self.state.chat = st;
        if was_busy && !busy { self.flush_queued(); }
        if phase == ConnectionPhase::Connected && !was_connected { self.after_connected(); }
        if finished != was_done && finished.is_some() { self.mark_read(); }
        if finished != was_done { self.state.last_interaction = (self.deps.now)(); }
        match phase {
            ConnectionPhase::Connected => {
                self.refresh_tried = false;
                if let Some(a) = agent { self.ensure_connector(&a); }
            }
            ConnectionPhase::AuthFailed => {
                self.drop_connector();
                if !self.refresh_tried {
                    // The token may have expired between the check and the upgrade: one refresh, then one new connection.
                    self.refresh_tried = true;
                    let (post, gen) = (self.post.clone(), self.auth_gen);
                    self.auth.force_refresh(Box::new(move |r| {
                        post.post(move |i| {
                            if i.auth_gen != gen { return; }
                            match r {
                                Ok(_) => {
                                    if let Some(c) = i.chat.take() {
                                        c.set_on_change(None);
                                        c.disconnect();
                                    }
                                    i.connect_chat();
                                }
                                Err(AuthProblem::Network) => i.state.notice = Some(AuthProblem::Network.korean_message().into()),
                                Err(_) => {
                                    i.state.auth = PetAuthPhase::SignedOut;
                                    i.state.notice = Some(AuthProblem::Rejected(String::new()).korean_message().into());
                                }
                            }
                            i.publish();
                        })
                    }));
                }
            }
            _ => {}
        }
        self.look_up_plan_tasks();
        self.publish();
    }

    /// Changes the chat state: through the chat client when there is one (its state is the source), else directly.
    fn update_chat(&mut self, body: impl FnOnce(&mut ChatState)) {
        match &self.chat {
            Some(c) => c.update(body),
            None => {
                body(&mut self.state.chat);
                self.publish();
            }
        }
    }

    // MARK: send

    /// The connected chat and server, or the refusal (and a connection attempt when there is no chat).
    fn ready_to_send(&mut self) -> Result<(Arc<ChatClient>, ServerEndpoint), String> {
        if self.state.auth != PetAuthPhase::SignedIn { return Err(AuthProblem::SignedOut.korean_message().into()); }
        let ready = match (&self.chat, self.state.chat.phase, self.settings.endpoint()) {
            (Some(c), ConnectionPhase::Connected, Some(e)) => Some((c.clone(), e)),
            _ => None,
        };
        let Some(r) = ready else {
            if self.chat.is_none() { self.connect_chat(); }
            return Err(SendRefusal::NotConnected.korean_message());
        };
        if self.state.uploading { return Err("파일을 올리는 중이에요. 잠시만요.".into()); }
        if self.state.chat.is_busy() { return Err(SendRefusal::Busy.korean_message()); }
        Ok(r)
    }

    fn finish_send(&mut self, completion: Option<RefusalCompletion>, msg: Option<String>) {
        if let Some(m) = &msg { self.state.notice = Some(m.clone()); }
        self.publish();
        self.done_after_queued(completion, msg);
    }

    fn send(&mut self, text: String, voice: bool, completion: Option<RefusalCompletion>) {
        self.state.last_interaction = (self.deps.now)();
        let (chat, endpoint) = match self.ready_to_send() {
            Ok(r) => r,
            Err(m) => return self.finish_send(completion, Some(m)),
        };
        let files = self.state.pending.clone();
        if let Err(p) = OutgoingText::validate(&text, !files.is_empty()) { return self.finish_send(completion, Some(p.korean_message())); }
        if files.is_empty() {
            match chat.send(&text, &[]) {
                Err(r) => self.finish_send(completion, Some(r.korean_message())),
                Ok(id) => {
                    if voice { self.state.voice_turns.insert(id); }
                    self.state.notice = None;
                    self.finish_send(completion, None);
                }
            }
            return;
        }
        self.state.uploading = true;
        self.publish();
        let chat_gen = self.chat_gen;
        self.upload(files.iter().map(|p| p.file.clone()).collect(), endpoint, vec![], Box::new(move |i, result| {
            i.state.uploading = false;
            match result {
                Err(p) => {
                    if p == MediaProblem::Unauthorized { i.state.auth = PetAuthPhase::SignedOut; }
                    i.finish_send(completion, Some(p.korean_message()))
                }
                Ok(attachments) => {
                    if i.chat_gen != chat_gen { return i.finish_send(completion, Some(SendRefusal::NotConnected.korean_message())); }
                    match chat.send(&text, &attachments) {
                        Err(r) => i.finish_send(completion, Some(r.korean_message())),
                        Ok(id) => {
                            if voice { i.state.voice_turns.insert(id); }
                            let sent: BTreeSet<u64> = files.iter().map(|p| p.id).collect();
                            i.state.pending.retain(|p| !sent.contains(&p.id));
                            i.state.notice = None;
                            i.finish_send(completion, None)
                        }
                    }
                }
            }
        }));
    }

    fn send_selection(&mut self, sent: PetSelectionSent, completion: RefusalCompletion) {
        self.state.last_interaction = (self.deps.now)();
        let completion = Some(completion);
        let (chat, endpoint) = match self.ready_to_send() {
            Ok(r) => r,
            Err(m) => return self.finish_send(completion, Some(m)),
        };
        self.state.uploading = true;
        self.publish();
        let chat_gen = self.chat_gen;
        self.upload(vec![sent.file.clone()], endpoint, vec![], Box::new(move |i, result| {
            i.state.uploading = false;
            match result {
                Err(p) => {
                    if p == MediaProblem::Unauthorized { i.state.auth = PetAuthPhase::SignedOut; }
                    i.finish_send(completion, Some(p.korean_message()))
                }
                Ok(attachments) => {
                    if i.chat_gen != chat_gen { return i.finish_send(completion, Some(SendRefusal::NotConnected.korean_message())); }
                    match chat.send(&sent.instruction, &attachments) {
                        Err(r) => i.finish_send(completion, Some(r.korean_message())),
                        Ok(id) => {
                            i.state.selection_turns.insert(id, sent);
                            i.state.notice = None;
                            i.finish_send(completion, None)
                        }
                    }
                }
            }
        }));
    }

    /// One file after the other (on the session thread), each with a token that is fresh at that moment.
    #[allow(clippy::type_complexity)]
    fn upload(&mut self, files: Vec<PetOutgoingFile>, endpoint: ServerEndpoint, done_so_far: Vec<PetAttachment>,
              completion: Box<dyn FnOnce(&mut Inner, Result<Vec<PetAttachment>, MediaProblem>) + Send>) {
        let mut files = files;
        if files.is_empty() { return completion(self, Ok(done_so_far)); }
        let file = files.remove(0);
        let (post, http) = (self.post.clone(), self.deps.http.clone());
        self.auth.valid_access_token(Box::new(move |tok| {
            let Ok(token) = tok else { return post.post(move |i| completion(i, Err(MediaProblem::Unauthorized))) };
            let target = endpoint.clone();
            PetMedia::upload(&file, &target, Some(&token), http.as_ref(), Box::new(move |r| {
                post.post(move |i| match r {
                    Err(p) => completion(i, Err(p)),
                    Ok(a) => {
                        let mut all = done_so_far;
                        all.push(a);
                        i.upload(files, endpoint, all, completion)
                    }
                })
            }));
        }));
    }

    /// The answer ended (done, error or stopped): the waiting text goes out now. A refusal hands it back to the user.
    fn flush_queued(&mut self) {
        let Some(text) = self.state.queued_text.take() else { return };
        let post = self.post.clone();
        let back = text.clone();
        self.send(text, false, Some(Box::new(move |refusal| {
            let Some(msg) = refusal else { return };
            post.post(move |i| {
                i.state.returned_text = Some(back);
                i.state.notice = Some(format!("대기하던 글을 보내지 못했어요. 입력창에 돌려놨어요. {msg}"));
                i.publish();
            })
        })));
    }

    // MARK: connector (window arrangement, gadget switches)

    fn ensure_connector(&mut self, agent_id: &str) {
        if !self.settings.allow_combo_control { return; }
        let Some(executor) = self.deps.executor.as_ref().and_then(Weak::upgrade) else { return };
        let Some(endpoint) = self.settings.endpoint() else { return };
        if self.connector_agent.as_deref() == Some(agent_id) { return; } // running, being set up, or given up until the next reconnect
        if self.state.connector == ConnectorPhase::Replaced { return; } // another device holds the slot; the user can reconnect
        if let Some(c) = self.connector.take() { c.disconnect(); }
        self.connector_gen += 1;
        self.connector_agent = Some(agent_id.to_string());
        let scope = self.settings.account_scope();
        let key = ConnectorBinding::secret_key(&scope, agent_id);
        match self.deps.secrets.lookup(&key) {
            PetSecretLookup::Found(data) => {
                if let Some(token) = String::from_utf8(data).ok().filter(|t| !t.is_empty()) {
                    return self.start_connector(&token, &endpoint, executor, agent_id);
                }
            }
            PetSecretLookup::Unavailable(_) => {
                // The stored binding is there but the store refused it: a new one would replace it on the server for
                // nothing. Tried again on the next reconnect.
                self.state.connector = ConnectorPhase::Failed("자격 증명 관리자에서 커넥터 연결 정보를 읽지 못했어요. 다시 연결하면 다시 시도해요.".into());
                return;
            }
            PetSecretLookup::Missing => {}
        }
        self.state.connector = ConnectorPhase::Connecting;
        let (post, gen, connector_gen, http, label) = (self.post.clone(), self.auth_gen, self.connector_gen, self.deps.http.clone(), self.deps.connector_label.clone());
        let agent = agent_id.to_string();
        self.auth.valid_access_token(Box::new(move |tok| {
            let req = tok.ok().and_then(|b| ConnectorBinding::request(&endpoint, Some(&b), &agent, &label));
            let Some(req) = req else {
                return post.post(|i| {
                    i.state.connector = ConnectorPhase::Failed("커넥터 연결을 만들지 못했어요.".into());
                    i.publish();
                });
            };
            http.send(req, Box::new(move |r| {
                post.post(move |i| {
                    if i.auth_gen != gen || i.connector_gen != connector_gen { return; }
                    let token = r.ok().filter(|resp| resp.is_success()).and_then(|resp| ConnectorBinding::token(&resp.body));
                    let Some(token) = token else {
                        i.state.connector = ConnectorPhase::Failed("커넥터 연결을 만들지 못했어요.".into());
                        return i.publish();
                    };
                    let scope = i.settings.account_scope();
                    let _ = i.deps.secrets.write(&ConnectorBinding::secret_key(&scope, &agent), token.as_bytes());
                    let _ = i.deps.secrets.write(&ConnectorBinding::agent_key(&scope), agent.as_bytes());
                    if let Some(executor) = i.deps.executor.as_ref().and_then(Weak::upgrade) { i.start_connector(&token, &endpoint, executor, &agent); }
                    i.publish();
                })
            }));
        }));
    }

    fn start_connector(&mut self, token: &str, endpoint: &ServerEndpoint, executor: Arc<dyn PetCommandExecuting>, agent_id: &str) {
        let mut c = PetConnectorClient::new(&endpoint.connector_url(), token, &executor, self.deps.make_transport.clone(), self.deps.schedule.clone());
        if let Some(caps) = self.connector_capabilities.clone() { c = c.with_capabilities(caps); }
        let c = Arc::new(c);
        self.connector_gen += 1;
        let gen = self.connector_gen;
        let post = self.post.clone();
        c.set_on_change(Some(Arc::new(move |phase: ConnectorPhase| {
            post.post(move |i| {
                if i.connector_gen != gen { return; }
                i.state.connector = phase;
                i.publish();
            })
        })));
        let (post, agent) = (self.post.clone(), agent_id.to_string());
        c.set_on_unauthorized(Some(Arc::new(move || {
            let agent = agent.clone();
            post.post(move |i| {
                if i.connector_gen != gen { return; }
                i.deps.secrets.delete(&ConnectorBinding::secret_key(&i.settings.account_scope(), &agent));
                if let Some(c) = i.connector.take() { c.disconnect(); }
                i.connector_gen += 1;
                i.connector_agent = None;
                if i.connector_reissued { return; }
                i.connector_reissued = true;
                i.ensure_connector(&agent);
            })
        })));
        self.connector = Some(c.clone());
        c.connect();
    }

    // MARK: server features besides the socket (history, recovery, usage, dictation, memory, cards)

    /// Runs `body` on the session thread with calls carrying a fresh token (None when signed out or the auth changed).
    fn with_calls(&mut self, body: impl FnOnce(&mut Inner, Option<PetServerCalls>) + Send + 'static) {
        let Some(endpoint) = self.settings.endpoint() else { return body(self, None) };
        let (post, gen, http) = (self.post.clone(), self.auth_gen, self.deps.http.clone());
        self.auth.valid_access_token(Box::new(move |tok| {
            post.post(move |i| {
                let calls = match tok {
                    Ok(t) if i.auth_gen == gen => Some(PetServerCalls::new(endpoint, http, Some(t))),
                    _ => None,
                };
                body(i, calls)
            })
        }));
    }

    fn failed(&mut self, e: &PetAPIError) {
        if e.is_unauthorized() { self.state.auth = PetAuthPhase::SignedOut; }
        self.state.notice = Some(e.text.clone());
        self.publish();
    }

    /// The socket is (again) usable: load the record once, then fill gaps from the delta; usage and server config.
    fn after_connected(&mut self) {
        if self.state.config.is_none() { self.load_client_config(); }
        self.refresh_usage();
        let Some(room) = self.state.chat.room_id.clone().filter(|r| !r.is_empty()) else { return };
        if !self.state.chat.history_loaded {
            self.load_history(room, None);
        } else {
            let since = self.state.chat.last_seq();
            self.with_calls(move |i, calls| {
                let Some(calls) = calls else { return };
                let post = i.post.clone();
                calls.delta(&room.clone(), since, Box::new(move |r| {
                    post.post(move |i| {
                        let (Ok(d), Some(chat)) = (r, i.chat.clone()) else { return };
                        chat.update(|s| {
                            if s.room_id.as_deref() == Some(room.as_str()) { s.recover(d.messages); }
                        });
                    })
                }));
            });
        }
    }

    fn load_history(&mut self, room: String, before_seq: Option<i64>) {
        if self.state.loading_history { return; }
        self.state.loading_history = true;
        self.publish();
        self.with_calls(move |i, calls| {
            let Some(calls) = calls else {
                i.state.loading_history = false;
                return i.publish();
            };
            let post = i.post.clone();
            calls.history(&room.clone(), before_seq, Box::new(move |r| {
                post.post(move |i| {
                    i.state.loading_history = false;
                    match r {
                        Err(e) => i.failed(&e),
                        Ok(page) => {
                            if let Some(c) = i.chat.clone() {
                                c.update(|s| {
                                    if s.room_id.as_deref() == Some(room.as_str()) { s.merge_history(page.messages, page.has_more, before_seq.is_some()); }
                                });
                            }
                            i.publish();
                        }
                    }
                })
            }));
        });
    }

    /// The read mark follows what the pet showed, so the phone app does not count it as unread.
    fn mark_read(&mut self) {
        if let Some(c) = &self.chat { c.mark_read(self.state.chat.last_seq()); }
    }

    fn refresh_usage(&mut self) {
        if self.state.auth != PetAuthPhase::SignedIn { return; }
        self.with_calls(|i, calls| {
            let Some(calls) = calls else { return };
            let post = i.post.clone();
            calls.usage(Box::new(move |r| {
                post.post(move |i| {
                    let Ok(u) = r else { return };
                    if let Some(c) = i.chat.clone() {
                        let u2 = u.clone();
                        c.update(move |s| s.usage = Some(u2));
                    }
                    i.state.chat.usage = Some(u);
                    i.publish();
                })
            }));
        });
    }

    fn load_client_config(&mut self) {
        let Some(endpoint) = self.settings.endpoint() else { return };
        let post = self.post.clone();
        PetServerCalls::new(endpoint, self.deps.http.clone(), None).client_config(Box::new(move |r| {
            post.post(move |i| {
                let Ok(c) = r else { return };
                if let Some(n) = c.service_notice() { i.state.notice = Some(n); }
                i.state.config = Some(c);
                i.publish();
            })
        }));
    }

    fn dictate_now(&mut self, audio: Vec<u8>, mime: String, file_name: String, duration_sec: Option<f64>, completion: Box<dyn FnOnce(Result<String, PetAPIError>) + Send>) {
        let config = self.state.config.clone().unwrap_or_else(PetClientConfig::fallback);
        let refuse = |text: String| PetAPIError { status: 0, code: Some("refused".into()), text };
        let too_long = format!("녹음이 너무 길어요. {}초 안으로 말해 주세요.", config.dictation_max_sec);
        if !config.dictation_enabled { return completion(Err(refuse("이 서버는 받아쓰기를 지원하지 않아요.".into()))); }
        if audio.is_empty() { return completion(Err(refuse("녹음된 소리가 없어요.".into()))); }
        if audio.len() as i64 > config.dictation_max_bytes { return completion(Err(refuse(too_long))); }
        if duration_sec.is_some_and(|d| d > config.dictation_max_sec as f64 + 1.0) { return completion(Err(refuse(too_long))); }
        let base = mime.split(';').next().unwrap_or(&mime).trim().to_string();
        if !PetServerAPI::DICTATION_TYPES.contains(&base.as_str()) {
            return completion(Err(refuse(ErrorText::user_message("unsupported_media_type", None, ""))));
        }
        if self.state.dictating { return completion(Err(refuse("받아쓰는 중이에요.".into()))); }
        self.state.dictating = true;
        self.publish();
        self.with_calls(move |i, calls| {
            let Some(calls) = calls else {
                i.state.dictating = false;
                i.publish();
                return completion(Err(PetAPIError { status: 401, code: Some("unauthorized".into()), text: AuthProblem::SignedOut.korean_message().into() }));
            };
            let post = i.post.clone();
            calls.dictate(&audio, &mime, &file_name, "ko", duration_sec, None, Box::new(move |r| {
                post.post(move |i| {
                    i.state.dictating = false;
                    if matches!(&r, Err(e) if e.is_unauthorized()) { i.state.auth = PetAuthPhase::SignedOut; }
                    i.publish();
                    completion(r)
                })
            }));
        });
    }

    // MARK: memory

    fn forget_memory(&mut self, update_id: String) {
        let Some(note) = self.state.chat.memory_notes.iter().find(|n| n.update_id == update_id && n.forgotten_at.is_none()).cloned() else { return };
        let at = (self.deps.now)();
        let id = update_id.clone();
        self.update_chat(move |s| {
            if let Some(n) = s.memory_notes.iter_mut().find(|n| n.update_id == id) {
                n.forgotten_at = Some(at);
                n.action = "forgotten".into();
            }
        });
        self.with_calls(move |i, calls| {
            let Some(calls) = calls else { return };
            let left = Arc::new(Mutex::new((note.items.len(), Vec::<PetAPIError>::new())));
            for item in &note.items {
                let (post, left, note, update_id) = (i.post.clone(), left.clone(), note.clone(), update_id.clone());
                calls.forget(&item.memory_id, &item.kind, Box::new(move |r| {
                    post.post(move |i| {
                        let mut g = left.lock().unwrap();
                        if let Err(e) = r { if e.code.as_deref() != Some("not_found") { g.1.push(e); } }
                        g.0 -= 1;
                        if g.0 > 0 { return; }
                        let Some(e) = g.1.first().cloned() else { return };
                        drop(g);
                        i.update_chat(|s| {
                            if let Some(n) = s.memory_notes.iter_mut().find(|n| n.update_id == update_id) {
                                n.forgotten_at = None;
                                n.action = note.action.clone();
                            }
                        });
                        i.failed(&e);
                    })
                }));
            }
        });
    }

    fn undo_forget(&mut self, update_id: String) {
        let Some(note) = self.state.chat.memory_notes.iter().find(|n| n.update_id == update_id).cloned() else { return };
        let Some(at) = note.forgotten_at else { return };
        if (self.deps.now)() - at >= PetSession::RESTORE_WINDOW {
            self.state.notice = Some(ErrorText::user_message("restore_expired", None, ""));
            return self.publish();
        }
        self.with_calls(move |i, calls| {
            let Some(calls) = calls else { return };
            let left = Arc::new(Mutex::new((note.items.len(), Vec::<PetAPIError>::new())));
            for item in &note.items {
                let (post, left, update_id) = (i.post.clone(), left.clone(), update_id.clone());
                calls.restore(&item.memory_id, Box::new(move |r| {
                    post.post(move |i| {
                        let mut g = left.lock().unwrap();
                        if let Err(e) = r { g.1.push(e); }
                        g.0 -= 1;
                        if g.0 > 0 { return; }
                        if let Some(e) = g.1.first().cloned() {
                            drop(g);
                            return i.failed(&e);
                        }
                        drop(g);
                        i.update_chat(|s| {
                            if let Some(n) = s.memory_notes.iter_mut().find(|n| n.update_id == update_id) {
                                n.forgotten_at = None;
                                n.action = "created".into();
                            }
                        });
                    })
                }));
            }
        });
    }

    fn send_memory_feedback(&mut self, turn_id: String, text: String, completion: Option<RefusalCompletion>) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() || trimmed.chars().count() > 2000 { return done(completion, Some("고칠 내용을 2000자 안으로 써 주세요.".into())); }
        let ids = self.state.chat.turns.iter().find(|t| t.client_msg_id == turn_id).map(|t| t.used_memory_ids.clone()).unwrap_or_default();
        let (room, agent) = (self.state.chat.room_id.clone(), self.state.chat.agent_id.clone());
        // FNV-1a of the text: the same correction sent twice is one feedback on the server.
        let hash = trimmed.bytes().fold(2_166_136_261u32, |h, b| (h ^ b as u32).wrapping_mul(16_777_619));
        let key = format!("pet-fb-{turn_id}-{hash:x}");
        self.with_calls(move |i, calls| {
            let Some(calls) = calls else { return done(completion, Some(AuthProblem::SignedOut.korean_message().into())) };
            let kind = if ids.is_empty() { "preference" } else { "correction" };
            let post = i.post.clone();
            calls.feedback(&trimmed, kind, &ids, room.as_deref(), agent.as_deref(), &key, Box::new(move |r| {
                post.post(move |i| match r {
                    Ok(()) => {
                        i.state.notice = Some("알려 줘서 고마워요. 기억을 고칠게요.".into());
                        i.publish();
                        done(completion, None)
                    }
                    Err(e) => {
                        i.failed(&e);
                        done(completion, Some(e.text))
                    }
                })
            }));
        });
    }

    // MARK: cards

    fn answer_card(&mut self, id: String, action: String, option_ids: Vec<String>, allow_rule_id: Option<String>, completion: Option<RefusalCompletion>) {
        let Some(card) = self.state.chat.card(&id).cloned() else { return done(completion, Some("이 요청을 찾지 못했어요.".into())) };
        if !card.is_pending() { return done(completion, Some(card.status_text())); }
        self.with_calls(move |i, calls| {
            let Some(calls) = calls else { return done(completion, Some(AuthProblem::SignedOut.korean_message().into())) };
            let calls = Arc::new(calls);
            let (post, calls2) = (i.post.clone(), calls.clone());
            calls.answer(&card, &action, &option_ids, allow_rule_id.as_deref(), Box::new(move |r| {
                post.clone().post(move |i| match r {
                    Ok(updated) => {
                        i.update_chat(|s| s.upsert_card(updated));
                        i.done_after_queued(completion, None)
                    }
                    Err(e) => {
                        // Closed or changed meanwhile: show the server's current card.
                        if ["already_answered", "expired", "superseded", "version_conflict"].contains(&e.code.as_deref().unwrap_or("")) {
                            calls2.card(&id, Box::new(move |rr| {
                                if let Ok(c) = rr { post.post(move |i| i.update_chat(|s| s.upsert_card(c))); }
                            }));
                        }
                        i.failed(&e);
                        done(completion, Some(e.text))
                    }
                })
            }));
        });
    }

    // MARK: tasks the server holds until the user starts them (plan notes)

    fn start_task(&mut self, id: String, completion: Option<RefusalCompletion>) {
        let Some(completion) = self.begin_task_action(&id, "start", completion) else { return };
        let expected = self.state.chat.tasks.get(&id).and_then(|t| t.estimate_id.clone());
        self.with_calls(move |i, calls| {
            let Some(calls) = calls else { return i.end_task_action(&id, None, Some(AuthProblem::SignedOut.korean_message().into()), completion) };
            let post = i.post.clone();
            calls.start_task(&id.clone(), expected.as_deref(), Box::new(move |r| {
                post.post(move |i| match r {
                    Ok(PetTaskStart::Started(t)) => i.end_task_action(&id, Some(t), None, completion),
                    Ok(PetTaskStart::EstimateChanged(t)) => {
                        let est = t.estimate_text().map(|e| format!(" ({e})")).unwrap_or_default();
                        let text = format!("예상 사용량이 바뀌었어요{est}. 확인하고 다시 눌러 주세요.");
                        i.end_task_action(&id, Some(t), Some(text), completion)
                    }
                    Err(e) => {
                        if e.is_unauthorized() { i.state.auth = PetAuthPhase::SignedOut; }
                        i.end_task_action(&id, None, Some(task_refusal(&e)), completion)
                    }
                })
            }));
        });
    }

    fn cancel_task(&mut self, id: String, completion: Option<RefusalCompletion>) {
        let Some(completion) = self.begin_task_action(&id, "cancel", completion) else { return };
        self.with_calls(move |i, calls| {
            let Some(calls) = calls else { return i.end_task_action(&id, None, Some(AuthProblem::SignedOut.korean_message().into()), completion) };
            let post = i.post.clone();
            calls.cancel_task(&id.clone(), Box::new(move |r| {
                post.post(move |i| match r {
                    Ok(t) => i.end_task_action(&id, Some(t), None, completion),
                    Err(e) => {
                        if e.is_unauthorized() { i.state.auth = PetAuthPhase::SignedOut; }
                        i.end_task_action(&id, None, Some(task_refusal(&e)), completion)
                    }
                })
            }));
        });
    }

    /// None (and nothing sent) while a start/cancel of this task is out; otherwise the completion back.
    fn begin_task_action(&mut self, id: &str, action: &str, completion: Option<RefusalCompletion>) -> Option<Option<RefusalCompletion>> {
        if id.is_empty() {
            done(completion, Some("이 작업을 찾지 못했어요.".into()));
            return None;
        }
        if self.task_actions_in_flight.contains(id) {
            done(completion, Some("처리하고 있어요. 잠시만요.".into()));
            return None;
        }
        self.task_actions_in_flight.insert(id.to_string());
        self.state.last_interaction = (self.deps.now)();
        let (id, action) = (id.to_string(), action.to_string());
        self.update_chat(move |s| {
            s.task_actions.insert(id.clone(), action);
            s.task_notices.remove(&id);
        });
        Some(completion)
    }

    fn end_task_action(&mut self, id: &str, task: Option<PetTask>, notice: Option<String>, completion: Option<RefusalCompletion>) {
        self.task_actions_in_flight.remove(id);
        let (id2, n) = (id.to_string(), notice.clone());
        self.update_chat(move |s| {
            s.task_actions.remove(&id2);
            if let Some(t) = task { s.apply_task(t, false); }
            match n { Some(n) => { s.task_notices.insert(id2, n); } None => { s.task_notices.remove(&id2); } }
        });
        self.done_after_queued(completion, notice);
    }

    /// Plan notes in the record whose task this chat has not heard of (e.g. loaded with the history): read each once, so
    /// the card shows its buttons only while the task is really waiting to be started.
    fn look_up_plan_tasks(&mut self) {
        if self.state.auth != PetAuthPhase::SignedIn { return; }
        let ids: Vec<String> = self
            .state
            .chat
            .history
            .iter()
            .filter(|m| m.note_kind.as_deref() == Some("plan"))
            .filter_map(|m| m.task_id.clone())
            .filter(|t| !self.state.chat.tasks.contains_key(t) && !self.task_lookups.contains(t))
            .collect();
        let start = ids.len().saturating_sub(10);
        for id in ids.into_iter().skip(start) {
            self.task_lookups.insert(id.clone());
            self.with_calls(move |i, calls| {
                let Some(calls) = calls else { return };
                let post = i.post.clone();
                calls.task(&id, Box::new(move |r| {
                    if let Ok(t) = r { post.post(move |i| i.update_chat(|s| s.apply_task(t, false))); }
                }));
            });
        }
    }
}

/// The refusal of a start/cancel as one Korean line (`not_found` is about the task here, not an attachment).
fn task_refusal(e: &PetAPIError) -> String {
    if e.code.as_deref() == Some("not_found") { "이 작업을 찾지 못했어요.".into() } else { e.text.clone() }
}

#[cfg(test)]
mod tests {
    //! Ports of the macOS tests that drive a running `PetSession` (PetCoreTests, PetServerTests, PetTaskStartTests,
    //! PetSecretStoreTests, PetBusySendTests, PetSelectionTests). Everything the session talks to is a fake that answers
    //! synchronously, so a few `flush()` rounds run every chain of work to its end: no sleeps. Timers go to a
    //! `ManualSchedule` and run only when a test fires them (the reconnect after a dropped socket).
    use super::*;
    use crate::core::pet::auth::query_items;
    use crate::core::pet::auth::testing::{fixed_random, path_of, PetFakeBrowser, PetStubHTTP, TOKEN_JSON};
    use crate::core::pet::chat_client::testing::{FakeNetwork, FakeTransport, ManualSchedule};
    use crate::core::pet::chat_client::TransportEvent;
    use crate::core::pet::chat_state::TurnStatus;
    use crate::core::pet::commands::{PetCommand, PetCommandOutcome};
    use crate::core::pet::http::{HTTPCompletion, PetHTTPRequest};
    use crate::core::pet::json::{self, JsonExt};
    use crate::core::pet::reconnect::TransportFailure;
    use crate::core::pet::secrets::testing::ScriptedStore;
    use crate::core::pet::secrets::{MemorySecretStore, PetSecretBundle};
    use crate::core::pet::selection::{PetSelectionAction, PetSelectionBarSettings, PetSelectionCapture, PetSelectionContext, PetSelectionFlow};
    use crate::core::pet::settings::PetServerMode;
    use crate::core::settings::ModuleId;
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    const AGENT: &str = "0193aaaa-0000-7000-8000-000000000002";
    const TOKEN_PATH: &str = "/realms/nachotown/protocol/openid-connect/token";
    const BINDING_PATH: &str = "/api/v1/agents/0193aaaa-0000-7000-8000-000000000002/connector-binding";
    const WAIT: Duration = Duration::from_secs(5);

    // MARK: fakes

    /// The stub network plus response headers by "METHOD path" (the history's `X-Has-More`).
    struct HeaderHTTP {
        stub: Arc<PetStubHTTP>,
        headers: Mutex<HashMap<String, Vec<(String, String)>>>,
    }

    impl PetHTTPClient for HeaderHTTP {
        fn send(&self, request: PetHTTPRequest, completion: HTTPCompletion) {
            let key = format!("{} {}", request.method, path_of(&request.url));
            let extra = self.headers.lock().unwrap().get(&key).cloned().unwrap_or_default();
            self.stub.send(request, Box::new(move |r| {
                completion(r.map(|mut resp| {
                    resp.headers.extend(extra);
                    resp
                }))
            }));
        }
    }

    /// Records commands and answers ok.
    #[derive(Default)]
    struct RecordingExecutor {
        commands: Mutex<Vec<PetCommand>>,
    }

    impl PetCommandExecuting for RecordingExecutor {
        fn execute(&self, command: PetCommand, done: Box<dyn FnOnce(PetCommandOutcome) + Send>) {
            self.commands.lock().unwrap().push(command);
            done(PetCommandOutcome::new(true, "done"))
        }
    }

    /// One test's world: network, secret store, login page, sockets, timers, the PC's commands and the clock.
    struct Env {
        http: Arc<PetStubHTTP>,
        client: Arc<HeaderHTTP>,
        secrets: Arc<MemorySecretStore>,
        browser: Arc<PetFakeBrowser>,
        net: Arc<FakeNetwork>,
        timers: Arc<ManualSchedule>,
        executor: Arc<RecordingExecutor>,
        now: Arc<Mutex<f64>>,
    }

    impl Env {
        fn new(start: f64) -> Self {
            let http = PetStubHTTP::new();
            http.on("POST", TOKEN_PATH, 200, TOKEN_JSON);
            http.on("POST", BINDING_PATH, 200, r#"{"token":"nct_feed","endpoint":"/ws/connector"}"#);
            let client = Arc::new(HeaderHTTP { stub: http.clone(), headers: Mutex::new(HashMap::new()) });
            Self {
                http,
                client,
                secrets: Arc::new(MemorySecretStore::new()),
                browser: Arc::new(PetFakeBrowser::default()),
                net: FakeNetwork::new(),
                timers: ManualSchedule::new(),
                executor: Arc::new(RecordingExecutor::default()),
                now: Arc::new(Mutex::new(start)),
            }
        }

        fn header(&self, method: &str, path: &str, k: &str, v: &str) {
            self.client.headers.lock().unwrap().insert(format!("{method} {path}"), vec![(k.into(), v.into())]);
        }

        fn advance(&self, by: f64) { *self.now.lock().unwrap() += by; }

        /// Mock server; `executor`: the PC runs commands (the connector needs one).
        fn session_with(&self, secrets: Arc<dyn PetSecretStore>, executor: bool, mutate: impl FnOnce(&mut PetSettings)) -> PetSession {
            let mut s = PetSettings { server_mode: PetServerMode::Mock, ..Default::default() };
            mutate(&mut s);
            let now = self.now.clone();
            let executor: Option<Weak<dyn PetCommandExecuting>> = if executor {
                let e: Arc<dyn PetCommandExecuting> = self.executor.clone();
                Some(Arc::downgrade(&e))
            } else {
                None
            };
            PetSession::new(s, PetSessionDeps {
                http: self.client.clone(),
                secrets,
                browser: Some(self.browser.clone() as Arc<dyn PetAuthBrowser>),
                make_transport: self.net.make(),
                executor,
                random: fixed_random(),
                now: Arc::new(move || *now.lock().unwrap()),
                schedule: self.timers.schedule(),
                connector_label: ConnectorBinding::LABEL_WINDOWS.into(),
            })
        }

        fn session(&self, mutate: impl FnOnce(&mut PetSettings)) -> PetSession { self.session_with(self.secrets.clone(), true, mutate) }

        /// Combo control off, no executor (the server tests).
        fn plain_session(&self) -> PetSession { self.session_with(Arc::new(MemorySecretStore::new()), false, |s| s.allow_combo_control = false) }

        fn sockets(&self, path: &str) -> Vec<Arc<FakeTransport>> {
            let all = self.net.transports.lock().unwrap().clone();
            all.into_iter().filter(|t| t.url.lock().unwrap().as_deref().is_some_and(|u| u.ends_with(path))).collect()
        }

        fn chats(&self) -> Vec<Arc<FakeTransport>> { self.sockets("/ws/chat") }
        fn chat(&self, i: usize) -> Arc<FakeTransport> { self.chats()[i].clone() }
        fn last_chat(&self) -> Arc<FakeTransport> { self.chats().last().cloned().expect("a chat socket") }
        fn connectors(&self) -> Vec<Arc<FakeTransport>> { self.sockets("/ws/connector") }
    }

    /// Every chain of posted work ends within a few rounds (the fakes answer at once); 50 is far above the deepest.
    fn settle(s: &PetSession) {
        for _ in 0..50 { s.flush(); }
    }

    fn bearer(t: &FakeTransport) -> Option<String> { t.bearer.lock().unwrap().clone() }
    fn closed(t: &FakeTransport) -> bool { t.closed.load(Ordering::SeqCst) }

    fn ready(room: &str) -> String {
        format!(r#"{{"type":"ready","user_id":"u","agent_id":"{AGENT}","agent_name":"나초","room_id":"{room}"}}"#)
    }

    /// A completion and where its answer arrives.
    fn reply<T: Send + 'static>() -> (Box<dyn FnOnce(T) + Send>, mpsc::Receiver<T>) {
        let (tx, rx) = mpsc::channel();
        (Box::new(move |v| { let _ = tx.send(v); }), rx)
    }

    fn frame_json(raw: &str) -> json::JsonObject { json::object_str(raw).unwrap_or_default() }

    fn attachment_ids(frame: &json::JsonObject) -> Vec<String> {
        frame.objs("attachments").iter().filter_map(|a| a.str("attachment_id")).collect()
    }

    fn query(r: &PetHTTPRequest, name: &str) -> Option<String> {
        let q = r.url.split_once('?').map(|(_, q)| q).unwrap_or("");
        query_items(q)?.into_iter().find(|(k, _)| k == name).and_then(|(_, v)| v)
    }

    fn png(name: &str, bytes: &[u8]) -> PetOutgoingFile { PetMedia::prepare(name, bytes.to_vec(), None).unwrap() }

    // MARK: PetSessionTests (PetCoreTests.swift)

    #[test]
    fn nothing_connects_before_the_chat_opens() {
        let env = Env::new(1_000.0);
        let s = env.session(|_| {});
        settle(&s);
        assert_eq!(s.snapshot().auth, PetAuthPhase::SignedOut);
        assert!(env.http.requests().is_empty());
        s.open();
        settle(&s);
        assert!(env.chats().is_empty(), "signed out: no socket");
    }

    #[test]
    fn sign_in_connects_chat_then_connector_and_runs_a_command() {
        let env = Env::new(1_000.0);
        let s = env.session(|_| {});
        s.sign_in();
        settle(&s);
        assert_eq!(env.browser.opened(), 1);
        assert!(env.browser.opened.lock().unwrap()[0].starts_with("http://127.0.0.1:8787/"), "the mock server's login page");
        assert_eq!(s.snapshot().auth, PetAuthPhase::SignedIn);
        assert_eq!(env.chats().len(), 1);
        assert_eq!(bearer(&env.chat(0)).as_deref(), Some("at-1"));
        env.chat(0).deliver(&ready("r"));
        settle(&s);
        assert_eq!(s.snapshot().chat.phase, ConnectionPhase::Connected);
        let bindings = env.http.sent("POST", BINDING_PATH);
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].header("Authorization"), Some("Bearer at-1"));
        let conn = env.connectors();
        assert_eq!(conn.len(), 1);
        conn[0].emit(TransportEvent::Opened);
        settle(&s);
        conn[0].deliver(r#"{"type":"ready","agent_id":"a","kind":"nacho_combo"}"#);
        settle(&s);
        assert_eq!(s.snapshot().connector, ConnectorPhase::Ready);
        conn[0].deliver(r#"{"type":"request","id":"q","capability":"combo_set_gadget","params":{"gadget":"keepAwake","on":true}}"#);
        settle(&s);
        assert_eq!(*env.executor.commands.lock().unwrap(), vec![PetCommand::SetGadget(ModuleId::KeepAwake, true)]);
        let key = ConnectorBinding::secret_key(&s.settings().account_scope(), AGENT);
        assert_eq!(env.secrets.read(&key), Some(b"nct_feed".to_vec()));

        // Next launch: the binding token is reused, no second POST.
        let again = env.session(|_| {});
        again.open();
        settle(&again);
        env.last_chat().deliver(&ready("r"));
        settle(&again);
        assert_eq!(env.http.sent("POST", BINDING_PATH).len(), 1);
        assert_eq!(env.connectors().len(), 2);
    }

    #[test]
    fn combo_control_off_means_no_connector() {
        let env = Env::new(1_000.0);
        let s = env.session(|s| s.allow_combo_control = false);
        s.sign_in();
        settle(&s);
        env.chat(0).deliver(&ready("r"));
        settle(&s);
        assert!(env.connectors().is_empty());
        assert!(env.http.sent("POST", BINDING_PATH).is_empty());
    }

    #[test]
    fn attach_uploads_then_sends_the_ids() {
        let env = Env::new(1_000.0);
        env.http.on("POST", "/api/v1/media/uploads", 201, r#"{"attachment_id":"0193aaaa-0000-7000-8000-0000000000a1","kind":"image","name":"c.png","mime":"image/png","bytes":3}"#);
        env.http.then("POST", "/api/v1/media/uploads", 201, r#"{"attachment_id":"0193aaaa-0000-7000-8000-0000000000a2","kind":"file","name":"n.md","mime":"text/markdown","bytes":2}"#);
        let s = env.session(|s| s.allow_combo_control = false);
        s.sign_in();
        settle(&s);
        let chat = env.chat(0);
        chat.deliver(&ready("r"));
        settle(&s);
        assert_eq!(s.attach(png("c.png", &[1, 2, 3])), None);
        assert_eq!(s.attach(png("n.md", b"hi")), None);
        assert_eq!(s.snapshot().pending.len(), 2);
        let (done, rx) = reply();
        s.send("이거 뭐야?", false, Some(done));
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), None);
        assert_eq!(env.http.sent("POST", "/api/v1/media/uploads").len(), 2);
        let frame = frame_json(&chat.last_sent().unwrap());
        assert_eq!(frame.str("text").as_deref(), Some("이거 뭐야?"));
        assert_eq!(attachment_ids(&frame), ["0193aaaa-0000-7000-8000-0000000000a1", "0193aaaa-0000-7000-8000-0000000000a2"]);
        let st = s.snapshot();
        assert!(st.pending.is_empty());
        assert_eq!(st.chat.turns.last().unwrap().attachment_names, ["c.png", "n.md"]);
        for _ in 0..PetMedia::MAX_ATTACHMENTS { s.attach(png("x.png", &[1])); }
        assert_eq!(s.attach(png("y.png", &[1])), Some(MediaProblem::TooMany.korean_message()));
    }

    #[test]
    fn failed_upload_keeps_the_files_and_sends_nothing() {
        let env = Env::new(1_000.0);
        env.http.on("POST", "/api/v1/media/uploads", 413, "{}");
        let s = env.session(|s| s.allow_combo_control = false);
        s.sign_in();
        settle(&s);
        let chat = env.chat(0);
        chat.deliver(&ready("r"));
        settle(&s);
        s.attach(png("c.png", &[1]));
        let (done, rx) = reply();
        s.send("봐줘", false, Some(done));
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Some(MediaProblem::TooLarge.korean_message()));
        assert_eq!(s.snapshot().pending.len(), 1);
        assert!(chat.sent().is_empty());
    }

    #[test]
    fn expired_token_refreshes_once_and_reconnects() {
        let env = Env::new(1_000.0);
        let s = env.session(|s| s.allow_combo_control = false);
        s.sign_in();
        settle(&s);
        env.http.on("POST", TOKEN_PATH, 200, r#"{"access_token":"at-2","expires_in":300,"refresh_token":"rt-2"}"#);
        env.chat(0).drop_with(Some(TransportFailure::new(Some(401), "401")));
        settle(&s);
        let chats = env.chats();
        assert_eq!(chats.len(), 2);
        assert_eq!(bearer(&chats[1]).as_deref(), Some("at-2"));
        assert_eq!(s.snapshot().auth, PetAuthPhase::SignedIn);
        chats[1].drop_with(Some(TransportFailure::new(Some(401), "401")));
        settle(&s);
        assert_eq!(env.chats().len(), 2, "a second 401 in a row does not loop");
    }

    #[test]
    fn canceled_login_stays_signed_out_and_sign_out_forgets() {
        let env = Env::new(1_000.0);
        env.browser.cancel.store(true, Ordering::SeqCst);
        let s = env.session(|_| {});
        s.sign_in();
        settle(&s);
        assert_eq!(s.snapshot().auth, PetAuthPhase::SignedOut);
        assert_eq!(s.snapshot().notice.as_deref(), Some(AuthProblem::Canceled.korean_message()));
        env.browser.cancel.store(false, Ordering::SeqCst);
        s.sign_in();
        settle(&s);
        env.chat(0).deliver(&ready("r"));
        settle(&s);
        s.sign_out();
        settle(&s);
        assert_eq!(s.snapshot().auth, PetAuthPhase::SignedOut);
        assert!(closed(&env.chat(0)));
        assert!(env.secrets.keys().is_empty(), "tokens and the connector binding are gone: {:?}", env.secrets.keys());
    }

    #[test]
    fn sign_out_in_a_later_launch_also_forgets_the_connector_binding() {
        let env = Env::new(1_000.0);
        let s = env.session(|_| {});
        s.sign_in();
        settle(&s);
        env.chat(0).deliver(&ready("r"));
        settle(&s);
        assert!(env.secrets.read(&ConnectorBinding::secret_key(&s.settings().account_scope(), AGENT)).is_some());
        s.shutdown();
        let later = env.session(|_| {}); // a new launch: nothing connected, the agent id only in the store
        later.sign_out();
        settle(&later);
        assert!(env.secrets.keys().is_empty(), "{:?}", env.secrets.keys());
    }

    #[test]
    fn shutdown_closes_everything() {
        let env = Env::new(1_000.0);
        let s = env.session(|_| {});
        s.sign_in();
        settle(&s);
        env.chat(0).deliver(&ready("r"));
        settle(&s);
        assert!(!env.connectors().is_empty());
        s.shutdown();
        settle(&s);
        assert!(env.chats().iter().all(|t| closed(t)));
        assert!(env.connectors().iter().all(|t| closed(t)));
        assert_eq!(s.snapshot().connector, ConnectorPhase::Off);
        assert_eq!(s.snapshot().chat.phase, ConnectionPhase::Disconnected);
    }

    /// Not in the Swift suite: a restart over the app's store (bundle) is signed in without the login page.
    #[test]
    fn restart_signs_in_from_the_store_without_the_browser() {
        let env = Env::new(1_000.0);
        let backing: Arc<dyn PetSecretStore> = env.secrets.clone();
        let first = env.session_with(Arc::new(PetSecretBundle::new(backing.clone())), true, |_| {});
        first.sign_in();
        settle(&first);
        assert_eq!(first.snapshot().auth, PetAuthPhase::SignedIn);
        drop(first);
        let restarted = env.session_with(Arc::new(PetSecretBundle::new(backing)), true, |_| {});
        assert_eq!(restarted.snapshot().auth, PetAuthPhase::SignedIn, "restored at construction");
        restarted.open();
        settle(&restarted);
        assert_eq!(env.chats().len(), 2);
        assert_eq!(bearer(&env.last_chat()).as_deref(), Some("at-1"), "the stored token");
        assert_eq!(env.browser.opened(), 1, "the login page only for the first sign-in");
    }

    // MARK: PetServerSessionTests (PetServerTests.swift)

    const ROOM: &str = "0193aaaa-0000-7000-8000-0000000000r1";

    fn msg(id: &str, seq: i64, role: &str, text: &str, client: Option<&str>) -> String {
        let c = client.map(|c| format!(r#","client_msg_id":"{c}""#)).unwrap_or_default();
        let sender = if role == "user" { "user" } else { "agent" };
        format!(r#"{{"id":"{id}","room_id":"{ROOM}","seq":{seq},"role":"{role}","sender_kind":"{sender}","text":"{text}","content":{{"type":"text","text":"{text}"}},"used_memory_ids":[],"redactions":[],"attachments":[]{c}}}"#)
    }

    fn server_env() -> Env {
        let env = Env::new(1_800_000_000.0);
        env.http.on("GET", "/client/config", 200, r#"{"service_status":{"state":"ok"},"dictation":{"enabled":true,"max_sec":60,"max_bytes":2000000},"media_max_bytes":20971520}"#);
        env.http.on("GET", "/api/v1/me/usage", 200, r#"{"plan_id":"free","allowance":100,"remaining":70,"available_to_start":70,"limits":[]}"#);
        env.http.on("GET", &format!("/api/v1/rooms/{ROOM}/history"), 200, &format!("[{},{}]", msg("h1", 1, "user", "어제", Some("x")), msg("h2", 2, "assistant", "응", None)));
        env
    }

    fn connected(env: &Env) -> (PetSession, Arc<FakeTransport>) {
        let s = env.plain_session();
        s.sign_in();
        settle(&s);
        let chat = env.chat(0);
        chat.deliver(&ready(ROOM));
        settle(&s);
        (s, chat)
    }

    fn history_path() -> String { format!("/api/v1/rooms/{ROOM}/history") }

    #[test]
    fn connecting_loads_config_history_and_usage_then_recovers_from_the_delta() {
        let env = server_env();
        let (s, chat) = connected(&env);
        let config = env.http.sent("GET", "/client/config");
        assert_eq!(config.len(), 1);
        assert_eq!(config[0].header("Authorization"), None);
        assert_eq!(env.http.sent("GET", &history_path())[0].header("Authorization"), Some("Bearer at-1"));
        let st = s.snapshot();
        assert_eq!(st.chat.history.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["어제", "응"]);
        assert_eq!(st.chat.usage.as_ref().map(|u| u.remaining), Some(70));
        assert_eq!(st.config.as_ref().map(|c| c.dictation_enabled), Some(true));

        s.send("오늘은?", false, None);
        settle(&s);
        let id = s.snapshot().chat.turns[0].client_msg_id.clone();
        chat.deliver(&format!(r#"{{"type":"turn_started","room_id":"{ROOM}","client_id":"{id}","turn_id":"t","duplicate":false}}"#));
        chat.deliver(&format!(r#"{{"type":"message","room_id":"{ROOM}","client_id":"{id}","message":{}}}"#, msg("u3", 3, "user", "오늘은?", Some(&id))));
        settle(&s);
        // The socket drops; the server finishes the answer meanwhile.
        env.http.on("GET", &format!("/api/v1/rooms/{ROOM}/delta"), 200, &format!(
            r#"{{"room_id":"{ROOM}","messages":[{},{}],"last_seq":4,"read_seq":2,"has_more":false}}"#,
            msg("u3", 3, "user", "오늘은?", Some(&id)), msg("a3", 4, "assistant", "맑아요", None)));
        chat.drop_with(Some(TransportFailure::new(None, "drop")));
        settle(&s);
        assert_eq!(s.snapshot().chat.turns[0].status, TurnStatus::Interrupted);
        assert_eq!(env.chats().len(), 1, "the reconnect waits for its timer");
        assert!(env.timers.last_delay().is_some_and(|d| d <= 1.0), "first retry after at most a second");
        env.timers.fire_last(); // the backoff timer (Swift waited for it in real time)
        settle(&s);
        assert_eq!(env.chats().len(), 2, "reconnected");
        env.chat(1).deliver(&ready(ROOM));
        settle(&s);
        let delta = env.http.sent("GET", &format!("/api/v1/rooms/{ROOM}/delta"));
        assert_eq!(delta.len(), 1);
        assert_eq!(query(&delta[0], "since_seq").as_deref(), Some("3"));
        let st = s.snapshot();
        assert_eq!(st.chat.turns[0].status, TurnStatus::Done);
        assert_eq!(st.chat.turns[0].assistant_text, "맑아요");
        assert_eq!(env.http.sent("GET", &history_path()).len(), 1, "the record is not loaded twice");
        assert_eq!(env.http.sent("GET", "/client/config").len(), 1);
    }

    #[test]
    fn older_history_and_read_mark() {
        let env = server_env();
        env.http.on("GET", &history_path(), 200, &format!("[{}]", msg("h5", 5, "user", "최근", Some("x"))));
        let s = env.plain_session();
        s.sign_in();
        settle(&s);
        let chat = env.chat(0);
        // First page says there is more.
        env.header("GET", &history_path(), "X-Has-More", "true");
        chat.deliver(&ready(ROOM));
        settle(&s);
        assert!(s.snapshot().chat.has_more_history);
        env.http.on("GET", &history_path(), 200, &format!("[{}]", msg("h4", 4, "assistant", "예전", None)));
        env.header("GET", &history_path(), "X-Has-More", "false");
        s.load_older_history();
        settle(&s);
        let older = env.http.sent("GET", &history_path()).last().cloned().unwrap();
        assert_eq!(query(&older, "before_seq").as_deref(), Some("5"));
        assert_eq!(s.snapshot().chat.history.iter().map(|m| m.seq).collect::<Vec<_>>(), [Some(4), Some(5)]);
        assert!(!s.snapshot().chat.has_more_history);

        s.send("안녕", false, None);
        settle(&s);
        let id = s.snapshot().chat.turns[0].client_msg_id.clone();
        chat.deliver(&format!(r#"{{"type":"message","room_id":"{ROOM}","client_id":"{id}","message":{}}}"#, msg("a6", 7, "assistant", "안녕!", None)));
        chat.deliver(&format!(r#"{{"type":"done","room_id":"{ROOM}","client_id":"{id}","message_id":"a6","stopped":false}}"#));
        settle(&s);
        let sent = chat.sent();
        assert!(sent.iter().any(|f| f.contains(r#""type":"read""#) && f.contains(r#""seq":7"#)), "{}", sent.join("\n"));
    }

    type Dictated = Result<String, PetAPIError>;

    fn dictate(s: &PetSession, audio: &[u8], mime: &str, name: &str, sec: f64) -> Option<Dictated> {
        let (done, rx) = reply::<Dictated>();
        s.dictate(audio.to_vec(), mime, name, Some(sec), done);
        settle(s);
        rx.try_recv().ok()
    }

    #[test]
    fn dictation_goes_to_the_server_within_the_limits() {
        let env = server_env();
        env.http.on("POST", "/api/v1/dictation", 200, r#"{"text":"내일 일정 알려줘","language":"ko","duration_sec":2.5}"#);
        let (s, _) = connected(&env);
        assert_eq!(dictate(&s, &[1; 100], "audio/mp4", "d.m4a", 2.5), Some(Ok("내일 일정 알려줘".into())));
        assert_eq!(env.http.sent("POST", "/api/v1/dictation").len(), 1);
        assert!(!s.snapshot().dictating);

        // Refused here: too long, wrong type, empty. No request.
        for (audio, mime, sec) in [(vec![1u8; 10], "audio/mp4", 62.0), (vec![1], "video/mp4", 1.0), (vec![], "audio/mp4", 1.0)] {
            let got = dictate(&s, &audio, mime, "d", sec);
            assert!(matches!(got, Some(Err(_))), "must refuse {mime} {sec}: {got:?}");
        }
        assert_eq!(env.http.sent("POST", "/api/v1/dictation").len(), 1);

        // Server limit: 429 with the time it is allowed again.
        env.http.on("POST", "/api/v1/dictation", 429, r#"{"error":{"code":"too_many_requests","message":"x","reason":"2030-01-02T03:04:05Z"}}"#);
        let Some(Err(e)) = dictate(&s, &[1], "audio/webm;codecs=opus", "d.webm", 1.0) else { panic!("429") };
        assert_eq!(e.code.as_deref(), Some("too_many_requests"));
        assert!(e.text.contains("다시 해 주세요"), "{}", e.text);
    }

    #[test]
    fn dictation_off_on_this_server() {
        let env = server_env();
        env.http.on("GET", "/client/config", 200, r#"{"service_status":{"state":"ok"},"dictation":{"enabled":false,"max_sec":60,"max_bytes":2000000}}"#);
        let (s, _) = connected(&env);
        assert!(matches!(dictate(&s, &[1], "audio/mp4", "d", 1.0), Some(Err(_))), "off");
        assert!(env.http.sent("POST", "/api/v1/dictation").is_empty());
    }

    #[test]
    fn forget_undo_within_ten_seconds_and_feedback() {
        let env = server_env();
        env.http.on("DELETE", "/api/v1/memory/facts/f1", 204, "");
        env.http.on("POST", "/api/v1/memory/f1/restore", 200, r#"{"kind":"fact","item":{"id":"f1"}}"#);
        env.http.on("POST", "/api/v1/memory/feedback", 201, r#"{"id":"fb","kind":"correction","classified_by":"client","actions":[{"action":"updated","memory_id":"f1"}],"created_at":"2026-11-11T00:00:00Z"}"#);
        let (s, chat) = connected(&env);
        chat.deliver(&format!(r#"{{"type":"memory_update","room_id":"{ROOM}","update":{{"update_id":"u1","action":"created","items":[{{"memory_id":"f1","kind":"fact","preview":"매운 거 좋아함"}}]}}}}"#));
        settle(&s);
        s.forget_memory("u1");
        settle(&s);
        assert_eq!(env.http.sent("DELETE", "/api/v1/memory/facts/f1").len(), 1);
        assert_eq!(s.snapshot().chat.memory_notes[0].action, "forgotten");
        assert!(s.can_undo_forget("u1"));
        env.advance(4.0);
        s.undo_forget("u1");
        settle(&s);
        assert_eq!(env.http.sent("POST", "/api/v1/memory/f1/restore").len(), 1);
        assert_eq!(s.snapshot().chat.memory_notes[0].action, "created");
        assert_eq!(s.snapshot().chat.memory_notes[0].forgotten_at, None);

        s.forget_memory("u1");
        settle(&s);
        env.advance(11.0);
        assert!(!s.can_undo_forget("u1"));
        s.undo_forget("u1");
        settle(&s);
        assert_eq!(env.http.sent("POST", "/api/v1/memory/f1/restore").len(), 1, "too late: no call");
        assert_eq!(s.snapshot().notice, Some(ErrorText::user_message("restore_expired", None, "")));

        // "기억이 틀렸어요" on an answer that used f1.
        s.send("내 취향 알아?", false, None);
        settle(&s);
        let id = s.snapshot().chat.turns[0].client_msg_id.clone();
        let answer = msg("a1", 9, "assistant", "매운 거요", None).replace(r#""used_memory_ids":[]"#, r#""used_memory_ids":["f1"]"#);
        chat.deliver(&format!(r#"{{"type":"message","room_id":"{ROOM}","client_id":"{id}","message":{answer}}}"#));
        chat.deliver(&format!(r#"{{"type":"done","room_id":"{ROOM}","client_id":"{id}","message_id":"a1","stopped":false}}"#));
        settle(&s);
        let (done, rx) = reply();
        s.send_memory_feedback(&id, "  이제 매운 거 못 먹어  ", Some(done));
        settle(&s);
        assert_eq!(rx.try_recv(), Ok(None));
        let fb = env.http.sent("POST", "/api/v1/memory/feedback");
        assert_eq!(fb.len(), 1);
        let j = PetStubHTTP::json(&fb[0]);
        assert_eq!(j.str("text").as_deref(), Some("이제 매운 거 못 먹어"));
        assert_eq!(j.str("kind").as_deref(), Some("correction"));
        assert_eq!(j.strs("memory_ids"), ["f1"]);
        assert_eq!(j.str("room_id").as_deref(), Some(ROOM));
        assert!(fb[0].header("Idempotency-Key").is_some());
        let (done, rx) = reply();
        s.send_memory_feedback(&id, "   ", Some(done));
        settle(&s);
        assert!(matches!(rx.try_recv(), Ok(Some(_))), "empty text is refused");
        assert_eq!(env.http.sent("POST", "/api/v1/memory/feedback").len(), 1, "empty text: no call");
    }

    /// Hold-to-talk before the chat was ever ready: the server's dictation settings are read first; the text then goes
    /// out as a message marked as spoken (typed messages are not).
    #[test]
    fn spoken_turn_is_marked_and_dictation_reads_the_config_first() {
        let env = server_env();
        env.http.on("POST", "/api/v1/dictation", 200, r#"{"text":"내일 날씨 알려 줘","language":"ko","duration_sec":2.5}"#);
        let s = env.plain_session();
        s.sign_in();
        settle(&s);
        assert_eq!(s.snapshot().config, None, "no `ready` yet, so no config");
        let got = dictate(&s, &[1, 2, 3], "audio/mp4", "dictation.m4a", 2.5);
        assert_eq!(env.http.sent("GET", "/client/config").len(), 1);
        assert_eq!(env.http.sent("POST", "/api/v1/dictation").len(), 1);
        assert_eq!(got, Some(Ok("내일 날씨 알려 줘".into())));

        let chat = env.chat(0);
        chat.deliver(&ready(ROOM));
        settle(&s);
        s.send("내일 날씨 알려 줘", true, None);
        settle(&s);
        let st = s.snapshot();
        let spoken = st.chat.turns[0].client_msg_id.clone();
        assert_eq!(st.voice_turns, BTreeSet::from([spoken.clone()]));
        assert_eq!(st.chat.turns[0].user_text, "내일 날씨 알려 줘", "shown as the user's bubble");
        chat.deliver(&format!(r#"{{"type":"done","room_id":"{ROOM}","client_id":"{spoken}","message_id":"a1","stopped":false}}"#));
        settle(&s);
        s.send("고마워", false, None);
        settle(&s);
        assert_eq!(s.snapshot().chat.turns.len(), 2);
        assert_eq!(s.snapshot().voice_turns, BTreeSet::from([spoken]), "a typed message is not spoken");
    }

    #[test]
    fn choice_card_answered_here_and_conflicts_refresh() {
        let env = server_env();
        env.http.on("POST", "/api/v1/chat/requests/cd/answer", 200, r#"{"id":"cd","kind":"choice","status":"answered","answered_action":"choose","version":2,"payload":{"question":"어디?","options":[{"id":"a","label":"강남"}]}}"#);
        let (s, chat) = connected(&env);
        chat.deliver(&format!(r#"{{"type":"card_update","room_id":"{ROOM}","request":{{"id":"cd","kind":"choice","status":"pending","version":1,"payload":{{"question":"어디?","options":[{{"id":"a","label":"강남"}}]}}}}}}"#));
        settle(&s);
        assert_eq!(s.snapshot().chat.pending_cards().len(), 1);
        s.answer_card("cd", "choose", vec!["a".into()], None, None);
        settle(&s);
        assert_eq!(PetStubHTTP::json(&env.http.sent("POST", "/api/v1/chat/requests/cd/answer")[0]).int("expected_version"), Some(1));
        assert_eq!(s.snapshot().chat.cards[0].status, "answered");
        assert!(s.snapshot().chat.pending_cards().is_empty());

        // Someone answered on the phone first: 409 → the current card is read again.
        env.http.on("POST", "/api/v1/chat/requests/cd2/answer", 409, r#"{"error":{"code":"already_answered","message":"x"}}"#);
        env.http.on("GET", "/api/v1/chat/requests/cd2", 200, r#"{"id":"cd2","kind":"choice","status":"answered","answered_action":"choose","version":5,"payload":{"question":"q"}}"#);
        chat.deliver(&format!(r#"{{"type":"card_update","room_id":"{ROOM}","request":{{"id":"cd2","kind":"choice","status":"pending","version":4,"payload":{{"question":"q"}}}}}}"#));
        settle(&s);
        let (done, rx) = reply();
        s.answer_card("cd2", "choose", vec!["x".into()], None, Some(done));
        settle(&s);
        assert_eq!(rx.try_recv(), Ok(Some(ErrorText::user_message("already_answered", None, ""))));
        assert_eq!(s.snapshot().chat.cards.iter().find(|c| c.id == "cd2").map(|c| c.version), Some(5));
    }

    // MARK: PetTaskStartSessionTests (PetTaskStartTests.swift). The plan card itself is PetTranscript (another
    // module): its labels and header lines are left out; what the card reads from the session state is checked.

    const TASK_ROOM: &str = "0193aaaa-0000-7000-8000-0000000000r7";
    const TASK: &str = "0193aaaa-0000-7000-8000-00000000t001";

    fn task_json(state: &str, estimate_id: &str, min: i64, max: i64, version: i64) -> String {
        format!(r#"{{"id":"{TASK}","room_id":"{TASK_ROOM}","kind":"task","state":"{state}","title":"휴대폰 전화해줘","request_text":"x","plan_mode":"full","steps":[],"stalled":false,"credit":{{"estimate":{{"estimate_min":{min},"estimate_max":{max},"task_label":"전화"}},"estimate_id":"{estimate_id}","used":0,"cap":{max},"refund_status":"none"}},"accessed_items":[],"plan_blocks":[],"result_blocks":[],"estimate_id":"{estimate_id}","version":{version}}}"#)
    }

    fn task(state: &str, version: i64) -> String { task_json(state, "e1", 40, 60, version) }

    fn plan_note_json(seq: i64) -> String {
        let text = "휴대폰 전화해줘 · 약 40~60크레딧 · 확인 후 시작해 주세요";
        format!(r#"{{"id":"n{seq}","room_id":"{TASK_ROOM}","seq":{seq},"role":"system","sender_kind":"system","text":"{text}","content":{{"type":"system_note","kind":"plan","text":"{text}"}},"task_id":"{TASK}","used_memory_ids":[],"attachments":[]}}"#)
    }

    fn task_env(history: &str) -> Env {
        let env = Env::new(1_000.0);
        env.http.on("GET", "/client/config", 200, r#"{"service_status":{"state":"ok"},"dictation":{"enabled":false}}"#);
        env.http.on("GET", "/api/v1/me/usage", 200, r#"{"plan_id":"free","allowance":100,"remaining":70,"limits":[]}"#);
        env.http.on("GET", &format!("/api/v1/rooms/{TASK_ROOM}/history"), 200, history);
        env
    }

    fn start_path() -> String { format!("/api/v1/tasks/{TASK}/start") }
    fn cancel_path() -> String { format!("/api/v1/tasks/{TASK}/cancel") }

    /// Signed in, connected, and a turn the server turned into a planned task.
    fn planned(env: &Env) -> (PetSession, Arc<FakeTransport>) {
        let s = env.plain_session();
        s.sign_in();
        settle(&s);
        let chat = env.chat(0);
        chat.deliver(&ready(TASK_ROOM));
        settle(&s);
        s.send("휴대폰 전화해줘", false, None);
        settle(&s);
        let id = s.snapshot().chat.turns[0].client_msg_id.clone();
        chat.deliver(&format!(r#"{{"type":"turn_started","room_id":"{TASK_ROOM}","client_id":"{id}","turn_id":"t","task_id":"{TASK}","duplicate":false}}"#));
        chat.deliver(&format!(r#"{{"type":"task_update","room_id":"{TASK_ROOM}","task":{}}}"#, task("planned", 1)));
        chat.deliver(&format!(r#"{{"type":"message","room_id":"{TASK_ROOM}","message":{}}}"#, plan_note_json(5)));
        chat.deliver(&format!(r#"{{"type":"done","room_id":"{TASK_ROOM}","client_id":"{id}","message_id":null,"task_id":"{TASK}","stopped":false}}"#));
        settle(&s);
        assert_eq!(s.snapshot().chat.task.as_ref().map(|t| t.state.as_str()), Some("planned"));
        (s, chat)
    }

    fn task_state(s: &PetSession) -> Option<String> { s.snapshot().chat.task.map(|t| t.state) }

    #[test]
    fn start_sends_the_estimate_the_user_saw_and_the_task_runs() {
        let env = task_env("[]");
        env.http.on("POST", &start_path(), 200, &task("running", 2));
        let (s, _) = planned(&env);
        let (done, rx) = reply();
        s.start_task(TASK, Some(done));
        settle(&s);
        assert_eq!(rx.try_recv(), Ok(None));
        let sent = env.http.sent("POST", &start_path());
        assert_eq!(sent.len(), 1);
        assert_eq!(PetStubHTTP::json(&sent[0]).str("expected_estimate_id").as_deref(), Some("e1"));
        assert_eq!(sent[0].header("Authorization"), Some("Bearer at-1"));
        assert_eq!(task_state(&s).as_deref(), Some("running"));
        assert_eq!(s.snapshot().chat.task.unwrap().status_text(), "하고 있어요");
        assert!(s.snapshot().chat.task_actions.is_empty());
        assert!(s.snapshot().chat.task_notices.is_empty());
    }

    #[test]
    fn a_second_press_while_the_first_is_out_sends_nothing() {
        let env = task_env("[]");
        env.http.on("POST", &start_path(), 200, &task("running", 2));
        let (s, _) = planned(&env);
        env.http.hold.store(true, Ordering::SeqCst); // the first press stays out
        let (second, rx) = reply();
        s.start_task(TASK, None);
        s.start_task(TASK, Some(second));
        s.cancel_task(TASK, None);
        settle(&s);
        assert_eq!(s.snapshot().chat.task_actions.get(TASK).map(String::as_str), Some("start"), "the card shows the press");
        env.http.hold.store(false, Ordering::SeqCst);
        env.http.release();
        settle(&s);
        assert_eq!(env.http.sent("POST", &start_path()).len(), 1);
        assert!(env.http.sent("POST", &cancel_path()).is_empty());
        assert_eq!(rx.try_recv(), Ok(Some("처리하고 있어요. 잠시만요.".into())));
        // Answered: a new press goes out again (start is idempotent on the server: not planned → current task).
        s.start_task(TASK, None);
        settle(&s);
        assert_eq!(env.http.sent("POST", &start_path()).len(), 2);
    }

    #[test]
    fn changed_estimate_shows_the_new_one_and_the_next_press_sends_it() {
        let env = task_env("[]");
        env.http.on("POST", &start_path(), 409, &format!(r#"{{"error":{{"code":"estimate_changed","message":"x"}},"task":{}}}"#, task_json("planned", "e2", 70, 90, 2)));
        let (s, _) = planned(&env);
        let (done, rx) = reply();
        s.start_task(TASK, Some(done));
        settle(&s);
        let line = "예상 사용량이 바뀌었어요 (약 70~90크레딧). 확인하고 다시 눌러 주세요.";
        assert_eq!(rx.try_recv(), Ok(Some(line.into())));
        let st = s.snapshot();
        let t = st.chat.tasks.get(TASK).unwrap();
        assert_eq!(t.estimate_id.as_deref(), Some("e2"));
        assert_eq!(t.estimate_text().as_deref(), Some("약 70~90크레딧"));
        assert!(t.needs_start_confirm(), "buttons on again");
        assert_eq!(st.chat.task_notices.get(TASK).map(String::as_str), Some(line), "shown under the card");
        assert!(st.chat.task_actions.is_empty());

        env.http.on("POST", &start_path(), 200, &task_json("queued", "e2", 70, 90, 3));
        s.start_task(TASK, None);
        settle(&s);
        assert_eq!(PetStubHTTP::json(&env.http.sent("POST", &start_path())[1]).str("expected_estimate_id").as_deref(), Some("e2"));
        assert_eq!(s.snapshot().chat.tasks.get(TASK).map(|t| t.status_text()).as_deref(), Some("곧 시작해요"));
        assert_eq!(s.snapshot().chat.task_notices.get(TASK), None);
    }

    #[test]
    fn refused_start_is_one_line_and_the_buttons_stay() {
        let env = task_env("[]");
        env.http.on("POST", &start_path(), 402, r#"{"error":{"code":"task_not_startable","message":"x","reason":"credits_exhausted"}}"#);
        let (s, _) = planned(&env);
        let (done, rx) = reply();
        s.start_task(TASK, Some(done));
        settle(&s);
        let line = "지금은 일을 맡길 수 없어요. 크레딧이 모자라요.";
        assert_eq!(rx.try_recv(), Ok(Some(line.into())));
        assert_eq!(s.snapshot().chat.task_notices.get(TASK).map(String::as_str), Some(line));
        assert!(!line.contains('\n'));
        assert_eq!(task_state(&s).as_deref(), Some("planned"), "the buttons stay");
        assert!(s.snapshot().chat.task_actions.is_empty());

        // A task that is gone: about the task, not an attachment.
        env.http.on("POST", &start_path(), 404, r#"{"error":{"code":"not_found","message":"x"}}"#);
        let (done, rx) = reply();
        s.start_task(TASK, Some(done));
        settle(&s);
        assert_eq!(rx.try_recv(), Ok(Some("이 작업을 찾지 못했어요.".into())));
    }

    #[test]
    fn cancel_ends_the_task() {
        let env = task_env("[]");
        env.http.on("POST", &cancel_path(), 200, &task("canceled", 2).replace(r#""version":2"#, r#""version":2,"failure":{"code":"user_canceled"}"#));
        let (s, _) = planned(&env);
        s.cancel_task(TASK, None);
        settle(&s);
        assert_eq!(env.http.sent("POST", &cancel_path()).len(), 1);
        assert!(env.http.sent("POST", &start_path()).is_empty());
        assert_eq!(task_state(&s).as_deref(), Some("canceled"));
        assert_eq!(s.snapshot().chat.task.unwrap().status_text(), "취소했어요");
    }

    #[test]
    fn a_plan_note_from_the_record_is_looked_up_once_before_it_gets_buttons() {
        let env = task_env(&format!("[{}]", plan_note_json(2)));
        env.http.on("GET", &format!("/api/v1/tasks/{TASK}"), 200, &task("planned", 1));
        let s = env.plain_session();
        s.sign_in();
        settle(&s);
        let chat = env.chat(0);
        chat.deliver(&ready(TASK_ROOM));
        settle(&s);
        assert_eq!(env.http.sent("GET", &format!("/api/v1/tasks/{TASK}")).len(), 1);
        let t = s.snapshot().chat.tasks.get(TASK).cloned().expect("looked up");
        assert!(t.needs_start_confirm());
        assert_eq!(t.estimate_text().as_deref(), Some("약 40~60크레딧"));
        chat.deliver(&format!(r#"{{"type":"usage","room_id":"{TASK_ROOM}","remaining":69}}"#));
        settle(&s);
        assert_eq!(env.http.sent("GET", &format!("/api/v1/tasks/{TASK}")).len(), 1, "looked up once");
    }

    // MARK: PetKeychainRefusalTests, the session tests (PetSecretStoreTests.swift)

    fn refusal_env() -> (Env, Arc<ScriptedStore>) { (Env::new(1_000.0), ScriptedStore::new()) }

    /// Signs in through the login page, connects, shuts down: the store now holds tokens and the connector binding.
    fn signed_in_launch(env: &Env, secrets: Arc<dyn PetSecretStore>) {
        let s = env.session_with(secrets, true, |_| {});
        s.sign_in();
        settle(&s);
        assert_eq!(s.snapshot().auth, PetAuthPhase::SignedIn);
        env.last_chat().deliver(&ready("r"));
        settle(&s);
        s.shutdown();
    }

    #[test]
    fn refused_keychain_at_launch_keeps_the_login_and_restores_it_on_the_next_access() {
        let (env, kc) = refusal_env();
        signed_in_launch(&env, kc.clone());
        let opens = env.browser.opened();
        kc.refuse(Some(-128)); // next launch: the store refuses
        let s = env.session_with(kc.clone(), true, |_| {});
        settle(&s);
        let m = AuthProblem::SecretsUnavailable.korean_message().to_string();
        assert_eq!(s.snapshot().auth, PetAuthPhase::Failed(m.clone()), "not signed out");
        assert_eq!(s.snapshot().notice, Some(m));
        s.open();
        settle(&s);
        assert_ne!(s.snapshot().auth, PetAuthPhase::SignedOut);
        assert!(kc.deleted().is_empty(), "nothing deleted");
        assert!(!kc.accounts().is_empty(), "nothing deleted");
        assert_eq!(env.browser.opened(), opens, "no login page");

        kc.refuse(None); // allowed this time
        s.open();
        settle(&s);
        assert_eq!(s.snapshot().auth, PetAuthPhase::SignedIn);
        assert_eq!(s.snapshot().notice, None);
        assert_eq!(bearer(&env.last_chat()).as_deref(), Some("at-1"), "the stored login, restored");
        assert_eq!(env.browser.opened(), opens);
    }

    #[test]
    fn sign_in_while_refused_first_tries_the_stored_login() {
        let (env, kc) = refusal_env();
        signed_in_launch(&env, kc.clone());
        let opens = env.browser.opened();
        kc.refuse(Some(-128));
        let s = env.session_with(kc.clone(), true, |_| {});
        settle(&s);
        kc.refuse(None);
        s.sign_in();
        settle(&s);
        assert_eq!(s.snapshot().auth, PetAuthPhase::SignedIn);
        assert_eq!(env.browser.opened(), opens, "restored from the store, no login page");
    }

    #[test]
    fn a_refused_connector_secret_is_not_reissued_over_the_stored_one() {
        let (env, kc) = refusal_env();
        signed_in_launch(&env, kc.clone());
        let key = ConnectorBinding::secret_key(&PetSettings { server_mode: PetServerMode::Mock, ..Default::default() }.account_scope(), AGENT);
        let stored = kc.get(&key);
        assert!(stored.is_some());
        let posts = env.http.sent("POST", BINDING_PATH).len();
        kc.refused.lock().unwrap().insert(key.clone());
        let s = env.session_with(kc.clone(), true, |_| {});
        s.open();
        settle(&s);
        env.last_chat().deliver(&ready("r"));
        settle(&s);
        assert_eq!(env.http.sent("POST", BINDING_PATH).len(), posts, "no new binding");
        assert_eq!(kc.get(&key), stored);
        assert!(matches!(s.snapshot().connector, ConnectorPhase::Failed(_)), "connector: {:?}", s.snapshot().connector);
    }

    #[test]
    fn the_app_store_asks_the_keychain_once_per_launch() {
        let (env, kc) = refusal_env();
        let backing: Arc<dyn PetSecretStore> = kc.clone();
        signed_in_launch(&env, Arc::new(PetSecretBundle::new(backing.clone())));
        kc.reset_prompts();
        let s = env.session_with(Arc::new(PetSecretBundle::new(backing)), true, |_| {}); // restart
        s.open();
        settle(&s);
        env.last_chat().deliver(&ready("r"));
        settle(&s);
        assert_eq!(s.snapshot().auth, PetAuthPhase::SignedIn);
        assert_eq!(env.connectors().len(), 2, "connector binding reused");
        assert_eq!(kc.prompts(), 1, "tokens + connector binding: one store read");
    }

    // MARK: PetBusySendTests (PetBusySendTests.swift; testQueuedLinesInTheTranscript is PetTranscript, not here)

    struct Busy {
        env: Env,
        s: PetSession,
        chat: Arc<FakeTransport>,
    }

    impl Busy {
        fn new() -> Self {
            let env = Env::new(1_000.0);
            let s = env.session(|s| s.allow_combo_control = false);
            s.sign_in();
            settle(&s);
            let chat = env.chat(0);
            chat.deliver(&ready("r"));
            settle(&s);
            Self { env, s, chat }
        }

        fn frames(&self) -> Vec<json::JsonObject> { self.chat.sent().iter().map(|f| frame_json(f)).collect() }
        fn types(&self) -> Vec<String> { self.frames().iter().filter_map(|f| f.str("type")).collect() }
        fn message_texts(&self) -> Vec<String> {
            self.frames().iter().filter(|f| f.str("type").as_deref() == Some("message")).filter_map(|f| f.str("text")).collect()
        }

        /// One turn is running (the first message is out and the server started it).
        fn start_busy_turn(&self) {
            let (done, rx) = reply();
            self.s.send("첫 질문", false, Some(done));
            assert_eq!(rx.recv_timeout(WAIT).unwrap(), None);
            let id = self.frames().iter().rev().find(|f| f.str("type").as_deref() == Some("message")).and_then(|f| f.str("client_msg_id")).unwrap();
            self.chat.deliver(&format!(r#"{{"type":"turn_started","room_id":"r","turn_id":"t","duplicate":false,"client_id":"{id}"}}"#));
            settle(&self.s);
            assert!(self.s.snapshot().chat.is_busy());
        }

        fn finish_turn(&self, stopped: bool) {
            let id = self.s.snapshot().chat.turns.last().map(|t| t.client_msg_id.clone()).unwrap_or_default();
            self.chat.deliver(&format!(r#"{{"type":"done","room_id":"r","stopped":{stopped},"client_id":"{id}"}}"#));
            settle(&self.s);
        }

        fn queue(&self, text: &str) -> Option<String> {
            let (done, rx) = reply();
            self.s.send_or_queue(text, Some(done));
            let out = rx.recv_timeout(WAIT).unwrap();
            settle(&self.s);
            out
        }
    }

    #[test]
    fn text_while_busy_waits_and_never_stops_the_answer() {
        let b = Busy::new();
        b.start_busy_turn();
        assert_eq!(b.queue("추가로 이것도"), None);
        assert_eq!(b.s.snapshot().queued_text.as_deref(), Some("추가로 이것도"));
        assert!(!b.types().contains(&"stop".to_string()), "Enter with text must not stop the answer");
        assert_eq!(b.message_texts(), ["첫 질문"], "nothing is sent while the answer runs");
        assert!(b.s.snapshot().chat.is_busy());
    }

    #[test]
    fn queued_text_goes_out_when_the_answer_ends() {
        let b = Busy::new();
        b.start_busy_turn();
        assert_eq!(b.queue("추가로 이것도"), None);
        b.finish_turn(false);
        assert_eq!(b.message_texts(), ["첫 질문", "추가로 이것도"]);
        assert_eq!(b.s.snapshot().queued_text, None);
        assert_eq!(b.s.snapshot().chat.turns.last().unwrap().user_text, "추가로 이것도");
        assert!(!b.types().contains(&"stop".to_string()));
    }

    #[test]
    fn queued_text_also_goes_out_after_a_stopped_answer() {
        let b = Busy::new();
        b.start_busy_turn();
        assert_eq!(b.queue("멈춘 뒤에도 보내요"), None);
        b.s.stop();
        settle(&b.s);
        assert_eq!(b.types().iter().filter(|t| *t == "stop").count(), 1, "the stop button still stops");
        b.finish_turn(true);
        assert_eq!(b.message_texts().last().map(String::as_str), Some("멈춘 뒤에도 보내요"));
    }

    #[test]
    fn cancelled_queue_sends_nothing() {
        let b = Busy::new();
        b.start_busy_turn();
        assert_eq!(b.queue("보내지 말아요"), None);
        b.s.cancel_queued();
        settle(&b.s);
        assert_eq!(b.s.snapshot().queued_text, None);
        b.finish_turn(false);
        assert_eq!(b.message_texts(), ["첫 질문"]);
    }

    #[test]
    fn a_second_queued_text_joins_the_first_on_a_new_line() {
        let b = Busy::new();
        b.start_busy_turn();
        assert_eq!(b.queue("하나"), None);
        assert_eq!(b.queue("둘"), None);
        assert_eq!(b.s.snapshot().queued_text.as_deref(), Some("하나\n둘"));
        b.finish_turn(false);
        assert_eq!(b.message_texts(), ["첫 질문", "하나\n둘"]);
    }

    #[test]
    fn empty_text_is_not_queued() {
        let b = Busy::new();
        b.start_busy_turn();
        assert!(b.queue("   ").is_some());
        assert_eq!(b.s.snapshot().queued_text, None);
    }

    #[test]
    fn not_busy_sends_at_once() {
        let b = Busy::new();
        assert_eq!(b.queue("바로 보내요"), None);
        assert_eq!(b.message_texts(), ["바로 보내요"]);
        assert_eq!(b.s.snapshot().queued_text, None);
    }

    #[test]
    fn a_refused_automatic_send_gives_the_text_back() {
        let b = Busy::new();
        b.start_busy_turn();
        assert_eq!(b.queue("돌려받을 글"), None);
        // The connection drops while answering: the turn ends as interrupted and the automatic send is refused.
        b.chat.drop_with(None);
        settle(&b.s);
        let st = b.s.snapshot();
        assert_eq!(st.queued_text, None);
        assert_eq!(st.returned_text.as_deref(), Some("돌려받을 글"), "the text is not lost: the window puts it back in the input");
        assert!(!b.types().contains(&"stop".to_string()));
        b.s.clear_returned();
        settle(&b.s);
        assert_eq!(b.s.snapshot().returned_text, None);
        drop(b.env);
    }

    // MARK: PetSelectionSessionTests (PetSelectionTests.swift)

    const SECRET: &str = "SECRET-선택-글-7f3a";

    fn selection_capture() -> PetSelectionCapture {
        let title = "mafianacho/nachocombo: Small gadgets in one box".to_string();
        PetSelectionCapture {
            text: format!("Every gadget starts off. {SECRET}"),
            app_name: "Microsoft Edge".into(),
            bundle_id: "msedge.exe".into(), // the exe name on Windows (com.apple.Safari on the Mac)
            window_title: Some(title.clone()),
            url: Some("https://github.com/mafianacho/nachocombo?token=abc#top".into()),
            page_title: Some(title),
            before: Some("Nacho Combo puts the small tools".into()),
            after: Some("The Nacho character sits".into()),
            language: Some("en".into()),
        }
    }

    #[test]
    fn sends_only_the_context_file_and_the_instruction() {
        let env = Env::new(1_000.0);
        env.http.on("POST", "/api/v1/media/uploads", 201, r#"{"attachment_id":"0193aaaa-0000-7000-8000-0000000000b1","kind":"file","name":"선택 맥락.md","mime":"text/markdown","bytes":10}"#);
        let s = Arc::new(env.session(|s| s.allow_combo_control = false));
        s.sign_in();
        settle(&s);
        let chat = env.chat(0);
        chat.deliver(&ready("r"));
        settle(&s);
        // A picture waiting in the composer is not swept into the selection request.
        assert_eq!(s.attach(png("c.png", &[1, 2, 3])), None);

        let sender: Arc<dyn PetSelectionSending> = s.clone();
        let flow = PetSelectionFlow::new(Some(Arc::downgrade(&sender)), |_| {}, |_| {});
        flow.present(&selection_capture());
        flow.choose(PetSelectionAction::Translate, &PetSelectionBarSettings { enabled: true, ..Default::default() });
        assert!(env.http.sent("POST", "/api/v1/media/uploads").is_empty(), "the preview uploads nothing");
        let (done, rx) = reply();
        flow.confirm(Some(done));
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), None);
        let uploads = env.http.sent("POST", "/api/v1/media/uploads");
        assert_eq!(uploads.len(), 1);
        assert_eq!(PetStubHTTP::json(&uploads[0]).str("name").as_deref(), Some(PetSelectionContext::FILE_NAME));
        assert_eq!(PetStubHTTP::json(&uploads[0]).str("mime").as_deref(), Some("text/markdown"));
        let frame = frame_json(&chat.last_sent().unwrap());
        let text = frame.str("text").unwrap_or_default();
        assert_eq!(text, "번역해 줘 · 한국어로");
        assert!(!text.contains(SECRET));
        assert_eq!(frame.objs("attachments").len(), 1);
        let st = s.snapshot();
        assert_eq!(st.pending.len(), 1, "the composer's picture is still waiting");
        let turn = st.chat.turns.last().unwrap();
        assert_eq!(st.selection_turns.get(&turn.client_msg_id).map(|t| t.action), Some(PetSelectionAction::Translate));
        drop(sender);
    }
}
