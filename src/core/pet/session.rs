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
        done(completion, msg);
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
                        done(completion, None)
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
        done(completion, notice);
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
