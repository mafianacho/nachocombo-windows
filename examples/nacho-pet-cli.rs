//! nacho-pet-cli: a headless client on the same `PetSession` as the app (dev/test only; driven by
//! `scripts/pet-roundtrip.sh` against `tools/mock-server`). It never talks to production: `--server` must be a loopback
//! `http://` URL, and its plain-socket HTTP client refuses anything else.
//!
//!   nacho-pet-cli login     --server http://127.0.0.1:8791 --store DIR
//!   nacho-pet-cli restart   --server … --store DIR            (a new session over the same store: signed in, no login)
//!   nacho-pet-cli chat      --server … --store DIR "안녕 나초"
//!   nacho-pet-cli ask-image --server … --store DIR capture.png "이거 뭐야?"     (Capture Float → pet)
//!   nacho-pet-cli send-file --server … --store DIR notes.md "요약해 줘"        (File Shelf → pet)
//!   nacho-pet-cli command   --server … --store DIR "창 왼쪽으로 붙여 줘"       (connector: window / gadget)
//!   nacho-pet-cli stop|queued|usage|history|recover|search|task|plan-start|plan-cancel|plan-change|remember|dictate|logout
//! Secrets go to 0600 files in DIR (0700 on Unix); they are never printed.
use nachocombo::core::pet::auth::{AuthProblem, PetAuthBrowser};
use nachocombo::core::pet::chat_client::{thread_schedule, MakeTransport, WebSocketTransport};
use nachocombo::core::pet::chat_state::{ConnectionPhase, TurnStatus};
use nachocombo::core::pet::frames::ActivityKind;
use nachocombo::core::pet::commands::{PetCommand, PetCommandOutcome, PetGadget};
use nachocombo::core::pet::connector_client::{ConnectorBinding, ConnectorPhase, PetCommandExecuting};
use nachocombo::core::pet::crypto;
use nachocombo::core::pet::http::{HTTPCompletion, PetHTTPClient, PetHTTPRequest, PetHTTPResponse, PetNetError};
use nachocombo::core::pet::media::PetMedia;
use nachocombo::core::pet::secrets::{PetSecretBundle, PetSecretLookup, PetSecretStore, PetSecretUnavailable};
use nachocombo::core::pet::session::{PetAuthPhase, PetSession, PetSessionDeps, PetSessionState};
use nachocombo::core::pet::settings::{PetServerMode, PetSettings, ServerEndpoint};
use nachocombo::core::pet::socket_transport::SocketTransport;
use nachocombo::core::settings::ModuleId;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

fn stamp(s: &str) {
    let t = nachocombo::core::pet::clock::now();
    println!("[{t:.3}] {s}");
    let _ = std::io::stdout().flush();
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}

// ---------- plain-socket HTTP (loopback http:// only) ----------

struct LoopbackHTTP;

fn perform(req: &PetHTTPRequest) -> Result<PetHTTPResponse, PetNetError> {
    let err = |m: String| PetNetError(m);
    let rest = req.url.strip_prefix("http://").ok_or_else(|| err("dev CLI speaks plain http to loopback only".into()))?;
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let path = if path.is_empty() { "/" } else { path };
    let host = authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority);
    if !ServerEndpoint::is_loopback(host) { return Err(err("not a loopback host".into())); }
    let addr = if authority.contains(':') { authority.to_string() } else { format!("{authority}:80") };
    let mut s = std::net::TcpStream::connect(addr).map_err(|e| err(e.to_string()))?;
    s.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let body = req.body.clone().unwrap_or_default();
    let mut head = format!("{} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nContent-Length: {}\r\n", req.method, body.len());
    for (k, v) in &req.headers { head.push_str(&format!("{k}: {v}\r\n")); }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(&body)).map_err(|e| err(e.to_string()))?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| err(e.to_string()))?;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| err("bad response".into()))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut body = raw[split + 4..].to_vec();
    let mut lines = head.lines();
    let status: u16 = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|c| c.parse().ok()).ok_or_else(|| err("bad status".into()))?;
    let mut headers = BTreeMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') { headers.insert(k.trim().to_string(), v.trim().to_string()); }
    }
    if headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked")) { body = dechunk(&body); }
    Ok(PetHTTPResponse { status, headers, body })
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = b.windows(2).position(|w| w == b"\r\n") {
        let n = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        if n == 0 || b.len() < i + 2 + n { break; }
        out.extend_from_slice(&b[i + 2..i + 2 + n]);
        b = &b[(i + 4 + n).min(b.len())..];
    }
    out
}

impl PetHTTPClient for LoopbackHTTP {
    fn send(&self, request: PetHTTPRequest, completion: HTTPCompletion) { std::thread::spawn(move || completion(perform(&request))); }
}

// ---------- 0600 file store ----------

/// Files named by the SHA-256 of the key in one directory (0700, files 0600 on Unix). The app uses Credential Manager.
struct FileSecretStore {
    dir: PathBuf,
}

impl FileSecretStore {
    fn new(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { dir })
    }
    fn file(&self, key: &str) -> PathBuf { self.dir.join(crypto::hex(&crypto::sha256(key.as_bytes()))) }
}

impl PetSecretStore for FileSecretStore {
    fn lookup(&self, key: &str) -> PetSecretLookup {
        let f = self.file(key);
        if !f.exists() { return PetSecretLookup::Missing; }
        match std::fs::read(f) { Ok(d) => PetSecretLookup::Found(d), Err(_) => PetSecretLookup::Unavailable(-1) }
    }
    fn write(&self, key: &str, value: &[u8]) -> Result<(), PetSecretUnavailable> {
        let f = self.file(key);
        std::fs::write(&f, value).map_err(|_| PetSecretUnavailable { status: -1 })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).map_err(|_| PetSecretUnavailable { status: -1 })?;
        }
        Ok(())
    }
    fn delete(&self, key: &str) { let _ = std::fs::remove_file(self.file(key)); }
}

// ---------- browser without a browser ----------

/// GET the login page and read the redirect (the mock login page approves at once). Redirects are not followed, so the
/// custom-scheme callback is only read.
struct HTTPBrowser;

impl PetAuthBrowser for HTTPBrowser {
    fn authorize(&self, url: &str, callback_scheme: &str, completion: Box<dyn FnOnce(Result<String, AuthProblem>) + Send>) {
        let path = url.split_once("://").map(|(_, r)| r).and_then(|r| r.find('/').map(|i| &r[i..])).unwrap_or("/");
        let path = path.split('?').next().unwrap_or(path);
        stamp(&format!("browser GET {path} (client_id, redirect_uri, code_challenge S256, state)"));
        let scheme = format!("{callback_scheme}://");
        LoopbackHTTP.send(PetHTTPRequest::new("GET", url), Box::new(move |r| match r {
            Ok(resp) if resp.status == 302 && resp.header("Location").is_some_and(|l| l.starts_with(&scheme)) => {
                let loc = resp.header("Location").unwrap().to_string();
                let shown = loc.split('?').next().unwrap_or("");
                stamp(&format!("browser redirected to {shown} (code + state)"));
                completion(Ok(loc))
            }
            Ok(resp) => {
                stamp(&format!("browser: login page answered HTTP {}", resp.status));
                completion(Err(AuthProblem::Network))
            }
            Err(_) => completion(Err(AuthProblem::Network)),
        }));
    }
}

// ---------- the PC, simulated ----------

/// Plays the PC: keeps gadget switches in memory and "moves" a window. Prints what Nacho asked for.
#[derive(Default)]
struct SimulatedPC {
    on: Mutex<BTreeSet<String>>,
}

impl PetCommandExecuting for SimulatedPC {
    fn execute(&self, command: PetCommand, done: Box<dyn FnOnce(PetCommandOutcome) + Send>) {
        match command {
            PetCommand::ArrangeWindow(action, app) => {
                stamp(&format!("pc: arrange window {} app={}", action.id(), app.as_deref().unwrap_or("front")));
                done(PetCommandOutcome::new(true, format!("Moved the front window to {}.", action.name())))
            }
            PetCommand::SetGadget(id, value) => {
                let raw = PetGadget::Module(id).raw_value();
                if value { self.on.lock().unwrap().insert(raw.clone()); } else { self.on.lock().unwrap().remove(&raw); }
                stamp(&format!("pc: gadget {raw} -> {}", if value { "on" } else { "off" }));
                done(PetCommandOutcome::new(true, format!("{} is now {}.", id.name(), if value { "on" } else { "off" })))
            }
            PetCommand::ListGadgets => {
                stamp("pc: list gadgets");
                let on = self.on.lock().unwrap().clone();
                let g: BTreeMap<String, String> = ModuleId::ALL.iter().map(|m| {
                    let raw = PetGadget::Module(*m).raw_value();
                    let state = if on.contains(&raw) { "on" } else { "off" };
                    (raw, state.to_string())
                }).collect();
                let mut out = PetCommandOutcome::new(true, format!("{} gadgets on.", on.len()));
                out.gadgets = Some(g);
                done(out)
            }
            other => {
                // The desktop actions (apps, several windows, system, workspaces) only print what Nacho asked for.
                stamp(&format!("pc: {other:?}"));
                done(PetCommandOutcome::new(true, format!("Simulated: {other:?}")))
            }
        }
    }
}

/// Dev randomness (the CLI talks only to the loopback mock). The app takes PKCE randomness from BCryptGenRandom.
fn dev_random(n: usize) -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher};
    let mut out = Vec::with_capacity(n + 8);
    let mut i = 0u64;
    while out.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.extend_from_slice(&h.finish().to_le_bytes());
        i += 1;
    }
    out.truncate(n);
    out
}

fn auth_name(a: &PetAuthPhase) -> String {
    match a {
        PetAuthPhase::SignedOut => "signedOut".into(),
        PetAuthPhase::SigningIn => "signingIn".into(),
        PetAuthPhase::SignedIn => "signedIn".into(),
        PetAuthPhase::Failed(m) => format!("failed({m})"),
    }
}

/// What a command decides on each state change: keep waiting, or finish with an exit code.
type Step = Box<dyn FnMut(&PetSessionState, &Arc<PetSession>) -> Option<i32> + Send>;
/// What starts the command once `on_change` is set (sign in, open the chat).
type Start = Box<dyn FnOnce(&PetSession)>;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() { fail("usage: nacho-pet-cli login|restart|chat|ask-image|send-file|command|stop|queued|logout|history|recover|task|plan-start|plan-cancel|plan-change|remember|search|dictate|usage --server URL --store DIR …"); }
    let command: &'static str = Box::leak(args.remove(0).into_boxed_str());
    let (mut server, mut store_dir, mut rest) = (String::new(), String::new(), Vec::<String>::new());
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--server" if i + 1 < args.len() => { server = args[i + 1].clone(); i += 2; }
            "--store" if i + 1 < args.len() => { store_dir = args[i + 1].clone(); i += 2; }
            _ => { rest.push(args[i].clone()); i += 1; }
        }
    }
    let endpoint = match ServerEndpoint::parse(&server) {
        Ok(e) if e.base.starts_with("http://") && ServerEndpoint::is_loopback(e.base.trim_start_matches("http://").rsplit_once(':').map(|(h, _)| h).unwrap_or("")) => e,
        _ => fail("--server must be a loopback http URL (dev CLI; production is the app's job)"),
    };
    if store_dir.is_empty() { fail("--store DIR is required"); }
    let mut settings = PetSettings {
        server_mode: PetServerMode::Custom,
        custom_server_url: endpoint.base.clone(),
        custom_issuer_url: format!("{}/realms/nachotown", endpoint.base),
        ..Default::default()
    };
    if let Some(p) = settings.validation_problem() { fail(&p); }
    if command != "command" { settings.allow_combo_control = false; }
    let files = FileSecretStore::new(PathBuf::from(&store_dir)).unwrap_or_else(|e| fail(&format!("store: {e}")));
    let secrets: Arc<dyn PetSecretStore> = Arc::new(PetSecretBundle::new(Arc::new(files)));
    let pc: Arc<dyn PetCommandExecuting> = Arc::new(SimulatedPC::default());
    let make_transport: MakeTransport = Arc::new(|| Box::new(SocketTransport::new()) as Box<dyn WebSocketTransport>);
    let http: Arc<dyn PetHTTPClient> = Arc::new(LoopbackHTTP);
    let session = Arc::new(PetSession::new(settings, PetSessionDeps {
        http: http.clone(),
        secrets,
        browser: Some(Arc::new(HTTPBrowser)),
        make_transport,
        executor: Some(Arc::downgrade(&pc)),
        random: Arc::new(dev_random),
        now: Arc::new(nachocombo::core::pet::clock::now),
        schedule: thread_schedule(),
        connector_label: ConnectorBinding::LABEL_WINDOWS.into(),
    }));
    let (done_tx, done_rx) = mpsc::channel::<i32>();
    let finish = {
        let tx = Mutex::new(done_tx);
        Arc::new(move |code: i32| { let _ = tx.lock().unwrap().send(code); })
    };
    let signed_in = session.snapshot().auth == PetAuthPhase::SignedIn;
    let need_login = || { if !signed_in { stamp("not signed in (run login first)"); std::process::exit(1) } };

    let mut start: Option<Start> = None;
    let step: Option<Step> = match command {
        "login" => {
            let (http, endpoint, finish2) = (http.clone(), endpoint.clone(), finish.clone());
            let mut asked = false;
            start = Some(Box::new(|s: &PetSession| s.sign_in()));
            Some(Box::new(move |st, s| {
                match &st.auth {
                    PetAuthPhase::SignedIn if !asked => {
                        asked = true;
                        stamp("auth signedIn (tokens stored, 0600)");
                        // Same check the server lane uses: a signed-in token reads /api/v1/me.
                        let (http, endpoint, finish) = (http.clone(), endpoint.clone(), finish2.clone());
                        s.with_access_token(move |tok| {
                            http.send(PetHTTPRequest::get(endpoint.rest("/api/v1/me"), tok.as_deref()), Box::new(move |r| {
                                let status = r.map(|r| r.status).unwrap_or(0);
                                stamp(&format!("GET /api/v1/me -> {status}"));
                                finish(if status == 200 { 0 } else { 1 });
                            }));
                        });
                        None
                    }
                    PetAuthPhase::Failed(m) => { stamp(&format!("auth failed: {m}")); Some(1) }
                    _ => None,
                }
            }))
        }
        "restart" => {
            // A second launch: the session was built over the store the login wrote; no browser, no sign_in call.
            if signed_in { stamp("auth signedIn (restored)"); }
            let finish = finish.clone();
            session.with_access_token(move |tok| { stamp(&format!("access token {}", if tok.is_some() { "usable" } else { "missing" })); finish(if tok.is_some() && signed_in { 0 } else { 1 }) });
            None
        }
        "logout" => {
            need_login();
            session.sign_out();
            session.flush();
            std::thread::sleep(Duration::from_millis(500));
            let st = session.snapshot();
            stamp(&format!("auth {} notice={}", auth_name(&st.auth), st.notice.as_deref().unwrap_or("-")));
            std::process::exit(if st.auth == PetAuthPhase::SignedOut { 0 } else { 1 });
        }
        "chat" | "ask-image" | "send-file" | "command" | "stop" | "queued" => {
            let mut text = rest.join(" ");
            let mut later: Option<String> = None;
            if command == "ask-image" || command == "send-file" {
                let Some(path) = rest.first() else { fail("file path missing") };
                text = rest[1..].join(" ");
                let data = std::fs::read(path).unwrap_or_else(|_| fail(&format!("cannot read {path}")));
                match PetMedia::prepare(path, data, None) {
                    Err(p) => { stamp(&format!("refused file: {}", p.korean_message())); std::process::exit(1) }
                    Ok(file) => {
                        stamp(&format!("attach {} mime={} bytes={} note={}", file.name, file.mime, file.data.len(), file.readability().korean_note()));
                        if let Some(r) = session.attach(file) { stamp(&format!("refused: {r}")); std::process::exit(1) }
                    }
                }
            }
            if command == "queued" {
                text = rest.first().cloned().unwrap_or_else(|| "/long".into());
                later = Some(rest.get(1).cloned().unwrap_or_else(|| "이어서 하나 더".into()));
            }
            need_login();
            let (mut sent, mut stopped, mut queued, mut printed, mut last_phase, mut last_conn) = (false, false, false, 0usize, String::new(), String::new());
            let mut first_turn = 0usize;
            let finish = finish.clone();
            start = Some(Box::new(|s: &PetSession| s.open()));
            Some(Box::new(move |st, s| {
                let phase = format!("{:?}", st.chat.phase);
                if phase != last_phase { stamp(&format!("phase {phase}")); last_phase = phase; }
                let conn = format!("{:?}", st.connector);
                if conn != last_conn { stamp(&format!("connector {conn}")); last_conn = conn; }
                if st.auth != PetAuthPhase::SignedIn { stamp(&format!("auth {} {}", auth_name(&st.auth), st.notice.as_deref().unwrap_or(""))); return Some(1); }
                let ready = st.chat.phase == ConnectionPhase::Connected && (command != "command" || st.connector == ConnectorPhase::Ready);
                if ready && !sent {
                    sent = true;
                    first_turn = st.chat.turns.len();
                    let (text2, n, finish) = (text.clone(), st.pending.len(), finish.clone());
                    s.send(&text2.clone(), false, Some(Box::new(move |refusal| match refusal {
                        Some(r) => { stamp(&format!("refused: {r}")); finish(1) }
                        None => stamp(&format!("sent text={text2} attachments={n}")),
                    })));
                }
                if st.uploading { stamp(&format!("uploading {} file(s)", st.pending.len())); }
                if !sent || st.chat.turns.len() <= first_turn { return None; }
                let t = &st.chat.turns[first_turn];
                let chars: Vec<char> = t.assistant_text.chars().collect();
                if chars.len() > printed {
                    stamp(&format!("delta {:?}", chars[printed..].iter().collect::<String>()));
                    printed = chars.len();
                }
                if st.chat.activity == ActivityKind::Tool { if let Some(tool) = &st.chat.active_tool { stamp(&format!("activity tool {tool}")); } }
                if command == "stop" && !stopped && printed >= 24 {
                    stopped = true;
                    stamp("stop requested");
                    s.stop();
                }
                if command == "queued" && !queued && printed >= 8 && st.chat.is_busy() {
                    queued = true;
                    let l = later.clone().unwrap();
                    s.send_or_queue(&l, Some(Box::new(|r| if let Some(r) = r { stamp(&format!("queue refused: {r}")) })));
                    return None;
                }
                if command == "queued" && queued {
                    if let Some(q) = &st.queued_text { if !stopped { stopped = true; stamp(&format!("queued text={q:?}")); s.stop(); } }
                    if let Some(next) = st.chat.turns.get(first_turn + 1) {
                        if next.status == TurnStatus::Done && st.queued_text.is_none() {
                            stamp(&format!("auto-sent {:?} answer={:?}", next.user_text, next.assistant_text));
                            return Some(0);
                        }
                    }
                    return None;
                }
                match &t.status {
                    TurnStatus::Done => { stamp(&format!("done text={:?} attachments={:?}", t.assistant_text, t.attachment_names)); Some(0) }
                    TurnStatus::Stopped => { stamp(&format!("stopped partial={:?}", t.assistant_text)); Some(0) }
                    TurnStatus::Failed(m) => { stamp(&format!("failed: {m}")); Some(1) }
                    TurnStatus::Interrupted => { stamp("interrupted"); Some(1) }
                    _ => None,
                }
            }))
        }
        "history" | "usage" | "dictate" => {
            need_login();
            let (mut asked_older, mut dictated) = (false, false);
            let finish = finish.clone();
            start = Some(Box::new(|s: &PetSession| s.open()));
            Some(Box::new(move |st, s| {
                if st.chat.phase != ConnectionPhase::Connected { return None; }
                let config = st.config.clone()?;
                match command {
                    "usage" => {
                        let u = st.chat.usage.clone()?;
                        stamp(&format!("config dictation={} max_sec={} media_max={} service={}", config.dictation_enabled, config.dictation_max_sec, config.media_max_bytes, config.service_state));
                        stamp(&format!("usage {}", u.summary()));
                        Some(0)
                    }
                    "dictate" => {
                        if dictated { return None; }
                        dictated = true;
                        if rest.len() < 2 { fail("dictate <file> <mime>") }
                        let audio = std::fs::read(&rest[0]).unwrap_or_else(|_| fail("cannot read the audio file"));
                        let name = std::path::Path::new(&rest[0]).file_name().and_then(|n| n.to_str()).unwrap_or("voice.m4a").to_string();
                        let finish = finish.clone();
                        s.dictate(audio, &rest[1], &name, Some(2.0), Box::new(move |r| match r {
                            Ok(t) => { stamp(&format!("dictation text={t:?}")); finish(0) }
                            Err(e) => { stamp(&format!("dictation refused code={} status={} text={}", e.code.as_deref().unwrap_or("-"), e.status, e.text)); finish(2) }
                        }));
                        None
                    }
                    _ => {
                        if !st.chat.history_loaded || st.loading_history { return None; }
                        if !asked_older {
                            asked_older = true;
                            for m in &st.chat.history { stamp(&format!("history seq={} role={} type={} text={:?}", m.seq.unwrap_or(-1), m.role, m.content_type, m.text)); }
                            stamp(&format!("history count={} more={}", st.chat.history.len(), st.chat.has_more_history));
                            if st.chat.has_more_history { s.load_older_history(); return None; }
                            return Some(0);
                        }
                        stamp(&format!("older count={} more={} first_seq={}", st.chat.history.len(), st.chat.has_more_history, st.chat.history.first().and_then(|m| m.seq).unwrap_or(-1)));
                        Some(0)
                    }
                }
            }))
        }
        "recover" | "task" | "remember" | "search" | "plan-start" | "plan-cancel" | "plan-change" => {
            need_login();
            let tail = |dflt: &str| if rest.is_empty() { dflt.to_string() } else { rest.join(" ") };
            let text = match command {
                "recover" => "/drop".to_string(),
                "task" => format!("/task {}", tail("내일 저녁 파스타집 예약해 줘")),
                "remember" => format!("/remember {}", tail("고양이를 키워요")),
                "plan-start" | "plan-cancel" => format!("/plan {}", tail("가게에 전화해서 예약해 줘")),
                "plan-change" => format!("/plan-change {}", tail("가게에 전화해서 예약해 줘")),
                _ => format!("/search {}", tail("강남 파스타")),
            };
            let (mut sent, mut answered, mut step, mut saw_interrupted, mut pressed) = (false, false, 0, false, 0);
            let mut forget_at = 0.0;
            let mut first_turn = 0usize;
            let finish = finish.clone();
            start = Some(Box::new(|s: &PetSession| s.open()));
            Some(Box::new(move |st, s| {
                if st.chat.phase != ConnectionPhase::Connected { return None; }
                if !sent {
                    sent = true;
                    first_turn = st.chat.turns.len();
                    let (t2, finish) = (text.clone(), finish.clone());
                    s.send(&text, false, Some(Box::new(move |r| match r { Some(r) => { stamp(&format!("refused: {r}")); finish(1) } None => stamp(&format!("sent {t2}")) })));
                    return None;
                }
                let t = st.chat.turns.get(first_turn)?;
                if t.status == TurnStatus::Interrupted && !saw_interrupted { saw_interrupted = true; stamp("interrupted (socket dropped)"); }
                match command {
                    "recover" => {
                        if t.status == TurnStatus::Done && saw_interrupted {
                            stamp(&format!("recovered from delta text={:?} userSeq={} answerSeq={}", t.assistant_text, t.user_seq.unwrap_or(-1), t.answer_seq.unwrap_or(-1)));
                            return Some(0);
                        }
                        None
                    }
                    "search" => {
                        if t.status != TurnStatus::Done { return None; }
                        let tools: Vec<String> = t.tools.iter().map(|x| format!("{}:{}", x.name, if !x.finished { "running" } else if x.is_error { "error" } else { "ok" })).collect();
                        stamp(&format!("tools {}", tools.join(",")));
                        stamp(&format!("progress {:?}", t.progress));
                        stamp(&format!("done text={:?}", t.assistant_text));
                        Some(if t.tools.is_empty() || t.progress.is_empty() { 1 } else { 0 })
                    }
                    "task" => {
                        if let Some(card) = st.chat.pending_cards().first().filter(|c| c.answerable_here()) {
                            if !answered {
                                answered = true;
                                stamp(&format!("task {} status={} progress={:?}", t.task_id.as_deref().unwrap_or("-"), st.chat.task.as_ref().map(|x| x.status_text()).unwrap_or("-".into()), t.progress));
                                let labels: Vec<&str> = card.options.iter().map(|o| o.label.as_str()).collect();
                                stamp(&format!("card {} {:?} options={labels:?} -> choose {}", card.kind, card.title, card.options[0].label));
                                s.answer_card(&card.id, "choose", vec![card.options[0].id.clone()], None, Some(Box::new(|r| stamp(&format!("answer {}", r.as_deref().unwrap_or("ok"))))));
                            }
                        }
                        if answered && st.chat.task.as_ref().is_some_and(|x| x.state == "done") && !t.assistant_text.is_empty() {
                            stamp(&format!("card now {}", st.chat.cards.first().map(|c| c.status_text()).unwrap_or("-".into())));
                            stamp(&format!("task done answer={:?} history={}", t.assistant_text, st.chat.history.len()));
                            return Some(0);
                        }
                        None
                    }
                    "plan-start" | "plan-cancel" | "plan-change" => {
                        let id = t.task_id.clone()?;
                        let task = st.chat.tasks.get(&id)?;
                        if st.chat.task_actions.contains_key(&id) { return None; }
                        if task.state == "planned" && pressed == 0 && st.chat.plan_note(&id).is_some() {
                            pressed = 1;
                            stamp(&format!("plan {} {}", task.status_text(), task.estimate_text().unwrap_or_default()));
                            let finish2 = finish.clone();
                            let done_line = Box::new(move |r: Option<String>| if let Some(r) = r { stamp(&format!("notice {r}")); let _ = &finish2; });
                            if command == "plan-cancel" { s.cancel_task(&id, Some(done_line)) } else { s.start_task(&id, Some(done_line)) }
                            return None;
                        }
                        if command == "plan-change" && pressed == 1 && task.state == "planned" && st.chat.task_notices.contains_key(&id) {
                            pressed = 2;
                            s.start_task(&id, Some(Box::new(|r| if let Some(r) = r { stamp(&format!("notice {r}")) })));
                            return None;
                        }
                        match task.state.as_str() {
                            "running" => { stamp(&format!("started state={} step={}", task.state, task.status_text())); Some(0) }
                            "canceled" => { stamp(&format!("canceled state={}", task.state)); Some(0) }
                            _ => None,
                        }
                    }
                    _ => {
                        // remember: memory line → forget → undo (10 s) → feedback on the answer that used it
                        if t.status != TurnStatus::Done { return None; }
                        let note = st.chat.memory_notes.first()?;
                        let now = nachocombo::core::pet::clock::now();
                        match step {
                            0 => {
                                step = 1;
                                stamp(&format!("memory {} used={}", note.text(), t.used_memory_ids.len()));
                                forget_at = now;
                                s.forget_memory(&note.update_id);
                            }
                            1 if note.action == "forgotten" && note.forgotten_at.is_some() => {
                                step = 2;
                                stamp(&format!("forgotten; undo possible={}", now - note.forgotten_at.unwrap() < PetSession::RESTORE_WINDOW));
                                s.undo_forget(&note.update_id);
                            }
                            2 if note.action == "created" && note.forgotten_at.is_none() => {
                                step = 3;
                                stamp(&format!("restored after {:.1}s: {}", now - forget_at, note.text()));
                                let finish = finish.clone();
                                s.send_memory_feedback(&t.client_msg_id, "이제 고양이 두 마리예요", Some(Box::new(move |r| {
                                    stamp(&format!("feedback {}", r.as_deref().unwrap_or("ok")));
                                    finish(if r.is_none() { 0 } else { 1 })
                                })));
                            }
                            _ => {}
                        }
                        None
                    }
                }
            }))
        }
        _ => fail(&format!("unknown command {command}")),
    };

    if let Some(step) = step {
        let step = Arc::new(Mutex::new((step, false)));
        let (weak, finish) = (Arc::downgrade(&session), finish.clone());
        session.set_on_change(Some(Arc::new(move |st| {
            let Some(s) = weak.upgrade() else { return };
            let mut g = step.lock().unwrap();
            if g.1 { return; } // finished: later changes are not reported again
            if let Some(code) = (g.0)(st, &s) {
                g.1 = true;
                finish(code);
            }
        })));
    }
    if let Some(start) = start { start(&session); }
    let code = done_rx.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|_| { stamp("timeout"); 1 });
    if command == "command" { stamp(&format!("commands handled: {}", session.handled_commands().join(", "))); }
    session.set_on_change(None);
    session.shutdown();
    std::process::exit(code);
}
