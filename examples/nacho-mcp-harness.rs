//! nacho-mcp-harness: the app's NachoCombo MCP server (`core::mcp`: the same service, socket and checks) with stand-in
//! gadgets, for a client round trip without Windows (dev/test only; `scripts/mcp-roundtrip.sh`). It binds 127.0.0.1
//! only, like the app. The approval card is printed instead of drawn and answered by `--approve`.
//!
//!   nacho-mcp-harness --dir DIR [--port N] [--gadgets clipboard,dim,appswitch] [--approve allow|deny|none]
//!
//! It makes one key labelled `roundtrip`, writes it to DIR/key.txt (test only; the app shows a key once and keeps
//! only its hash), prints `listening <url>` and serves until stdin closes, then stops the server and prints `closed`.
use nachocombo::core::mcp::keys::KeyStore;
use nachocombo::core::mcp::limits::system_clock;
use nachocombo::core::mcp::server::McpServer;
use nachocombo::core::mcp::service::{McpApproval, McpApprovalRequest, McpApproving, McpService, McpSources};
use nachocombo::core::mcp::settings::McpSettings;
use nachocombo::core::pet::commands::{PetCommand, PetCommandOutcome};
use nachocombo::core::pet::connector_client::PetCommandExecuting;
use nachocombo::core::pet::gadget_tools::{PetClipKind, PetClipRow, PetDataTools, PetDimState};
use nachocombo::core::pet::mcp_card::McpCards;
use std::collections::BTreeSet;
use std::io::Read;
use std::sync::Arc;

/// Stand-in gadgets: a two-item clipboard history, Focus Dim at 60 %, an app list that closes nothing.
struct Gadgets { clips: Vec<PetClipRow> }

impl PetCommandExecuting for Gadgets {
    fn execute(&self, command: PetCommand, done: Box<dyn FnOnce(PetCommandOutcome) + Send>) {
        println!("RUN {command:?}");
        let keys: Vec<&[u8]> = self.clips.iter().map(|c| c.key.as_slice()).collect();
        done(match command {
            PetCommand::ClipboardList => PetDataTools::clipboard_list(&self.clips),
            PetCommand::ClipboardGetItem(r) => match PetDataTools::pick(&r, &keys, "clipboard item") {
                Ok(i) => PetDataTools::clipboard_content(i + 1, &self.clips[i]),
                Err(o) => o,
            },
            PetCommand::DimStatus => PetDimState { paused: false, strength: 0.6, front_app: false, other_displays: true }.outcome(None),
            PetCommand::QuitApp(name) => PetCommandOutcome::new(true, format!("Asked {name} to close (stand-in: nothing was closed).")),
            other => PetCommandOutcome::new(true, format!("{other:?} (stand-in)")),
        })
    }
}

/// Prints the card the pet would show and answers it.
struct Console { answer: Option<McpApproval> }

impl McpApproving for Console {
    fn ask(&self, request: McpApprovalRequest, done: Box<dyn FnOnce(McpApproval) + Send>) {
        let card = McpCards::item(&request, false);
        println!("CARD {} | {} | {} | rows: {}", card.id, card.title, card.summary.join(" / "),
            card.options.iter().map(|o| format!("{} -> {}", o.label, o.link.clone().unwrap_or_default())).collect::<Vec<_>>().join(", "));
        match self.answer {
            Some(a) => { println!("ANSWER {} {a:?}", card.id); done(a) }
            None => println!("ANSWER {} (nobody answers)", card.id),
        }
    }
    fn cancel(&self, id: u64) { println!("CANCEL {}", McpCards::card_id(id)); }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let dir = std::path::PathBuf::from(opt("--dir").expect("--dir DIR"));
    let port: u16 = opt("--port").and_then(|p| p.parse().ok()).unwrap_or(0);
    let answer = match opt("--approve").as_deref() { Some("deny") => Some(McpApproval::Denied), Some("none") => None, _ => Some(McpApproval::Allowed) };
    let mut settings = McpSettings { server_enabled: true, ..Default::default() };
    for g in opt("--gadgets").unwrap_or_else(|| "clipboard,dim,appswitch".into()).split(',').filter(|g| !g.is_empty()) { settings.set_gadget_enabled(g, true); }

    std::fs::create_dir_all(&dir).expect("dir");
    let random: Arc<dyn Fn(usize) -> Vec<u8> + Send + Sync> = Arc::new(|n| {
        let mut b = vec![0u8; n];
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).expect("random bytes");
        b
    });
    let keys = Arc::new(KeyStore::new(Some(KeyStore::path_in(&dir)), random, system_clock()));
    let key = keys.create("roundtrip").expect("key");
    std::fs::write(dir.join("key.txt"), &key.token).expect("key file");

    let clips = vec![
        PetClipRow { key: b"one".to_vec(), kind: PetClipKind::Text, text: "meeting at 3pm in room B".into(), bytes: 0, pinned: false },
        PetClipRow { key: b"two".to_vec(), kind: PetClipKind::Link, text: "https://example.com/".into(), bytes: 0, pinned: true },
    ];
    let s = settings.clone();
    let sources = McpSources {
        settings: Arc::new(move || s.clone()),
        always_allowed: Arc::new(BTreeSet::new),
        module_on: Arc::new(|_| true),
    };
    let mut service = McpService::new(sources, keys, Arc::new(Gadgets { clips }), Arc::new(Console { answer }), system_clock());
    service.approval_timeout = std::time::Duration::from_secs(3);
    let mut server = McpServer::start(Arc::new(service), port).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1) });
    println!("listening {}", server.url());
    let mut sink = String::new();
    let _ = std::io::stdin().read_to_string(&mut sink);
    server.stop();
    println!("closed");
}
