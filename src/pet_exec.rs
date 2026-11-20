//! Runs what Nacho asks for on this PC (the connector's `request` frames), with the gadgets that already exist. The
//! connector client (`core::pet::connector_client`) has already checked the name against what was declared and the
//! parameters against closed lists; the decisions (which app a name means, where a window goes, what a list says) are
//! `core::pet::desktop_runner` and `core::pet::gadget_tools`. This file only reads windows, moves them, starts
//! programs and calls the gadgets, on the UI thread (requests arrive on the socket's thread and hop here).
//!
//! Privacy: lists carry numbers, kinds, sizes and app names, never window titles or clipboard text. The one action
//! that sends content (`combo_clipboard_get_item`) is declared with `"approval": "required"` unless the user set the
//! gadget to "always allow" (`gadget-trust.json`, `ctl trust clipboard on|off`).
//!
//! Nothing here runs until the pet's session is wired in (W2): the session takes [`WindowsPetExecutor`] and
//! [`declaration`], and calls it again through [`watch_declaration`] when a switch changes.
#![allow(dead_code)]
use crate::app::{app, State};
use crate::core::layout::{Rect, SnapAction};
use crate::core::pet::commands::{PetCommand, PetCommandOutcome, PetCommands, PetGadget};
use crate::core::pet::connector_client::PetCommandExecuting;
use crate::core::pet::desktop::{DesktopWorkspaceStore, PetAppEntry, PetAppMatcher, PetPartResult, PetPermissionHelp, PetSystemChange, SaveResult, WorkspaceApplyReport};
use crate::core::pet::desktop_runner::{exe_name, PetDesktopRunner, PetWindowInfo};
use crate::core::pet::gadget_tools::*;
use crate::core::pet::trust::GadgetTrust;
use crate::core::settings::ModuleId;
use crate::core::shelf::Kind;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_PET_EXEC: u32 = WM_APP + 10;

type Job = Box<dyn FnOnce() + Send>;
type Done = Box<dyn FnOnce(PetCommandOutcome) + Send>;

static MAIN: AtomicIsize = AtomicIsize::new(0);
static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());
static TRUST: OnceLock<Arc<GadgetTrust>> = OnceLock::new();

/// At start: the window that receives the hops, and the "always allow" switches read from the app's folder.
pub fn init(main: HWND, dir: &Path) {
    MAIN.store(main.0 as isize, Ordering::SeqCst);
    trust().attach(&dir.join(GadgetTrust::FILE_NAME));
}

/// The "always allow" switches of what this PC declares (no chat handoff yet: clipboard only).
pub fn trust() -> &'static Arc<GadgetTrust> { TRUST.get_or_init(|| Arc::new(GadgetTrust::for_windows(false))) }

/// What hello declares on Windows, with the current switches.
pub fn declaration() -> Vec<Value> {
    let t = trust();
    PetCommands::windows_declaration(&t.always_allowed(), &t.disabled(), false)
}

/// Calls `f` with the new declaration whenever a switch changes (the session passes it to the connector, which
/// declares it again behind a ping barrier).
pub fn watch_declaration(f: Arc<dyn Fn(Vec<Value>) + Send + Sync>) -> u64 { trust().observe(Arc::new(move || f(declaration()))) }

/// The UI thread drains the hops.
pub fn on_message() {
    let jobs = std::mem::take(&mut *JOBS.lock().unwrap());
    for j in jobs { j(); }
}

fn hop(job: Job) -> bool {
    let main = MAIN.load(Ordering::SeqCst);
    if main == 0 { return false; }
    JOBS.lock().unwrap().push(job);
    unsafe { PostMessageW(Some(HWND(main as *mut _)), WM_PET_EXEC, WPARAM(0), LPARAM(0)).is_ok() }
}

/// The executor the session holds (weakly).
pub struct WindowsPetExecutor;

impl PetCommandExecuting for WindowsPetExecutor {
    fn execute(&self, command: PetCommand, done: Done) {
        let slot: Arc<Mutex<Option<Done>>> = Arc::new(Mutex::new(Some(done)));
        let s = slot.clone();
        let posted = hop(Box::new(move || {
            let outcome = run(command);
            if let Some(d) = s.lock().unwrap().take() { d(outcome) }
        }));
        if !posted {
            if let Some(d) = slot.lock().unwrap().take() { d(PetCommandOutcome::new(false, "Nacho Combo is closing")) }
        }
    }
}

// MARK: running one command (UI thread)

fn running(m: ModuleId) -> bool { app().state(m) == State::Running }

pub fn run(command: PetCommand) -> PetCommandOutcome {
    if let Some(m) = PetGadgetTools::required_module(&command) {
        if !running(m) { return PetCommandOutcome::gadget_off(PetGadgetTools::gadget_of_module(m).unwrap_or("combo")); }
    }
    use PetCommand::*;
    match command {
        ArrangeWindow(action, name) => arrange(action, name.as_deref()),
        SetGadget(m, on) => set_gadget(m, on),
        ListGadgets => { let mut o = PetCommandOutcome::new(true, gadgets_text()); o.gadgets = Some(gadget_states()); o }
        ListApps { installed } => {
            let list = installed.then(installed_apps);
            PetDesktopRunner::list_apps(&windows(), list.as_deref())
        }
        OpenApp(name) => open_app(&name),
        FocusApp(name) => with_running(&name, |e, mine| {
            let Some(w) = mine.first() else { return PetCommandOutcome::new(false, format!("{} has no window to show.", e.display_name())) };
            crate::appswitch::raise(w.id, w.minimized);
            PetCommandOutcome::new(true, format!("Switched to {}.", e.display_name()))
        }),
        HideApp(name) => with_running(&name, |e, mine| {
            if PetAppMatcher::is_protected(&e.bundle_id) { return PetCommandOutcome::new(false, format!("{} is never hidden by Nacho.", e.display_name())); }
            for w in mine { unsafe { let _ = ShowWindowAsync(HWND(w.id as *mut _), SW_MINIMIZE); } }
            PetCommandOutcome::new(true, format!("Minimized every window of {} (it keeps running).", e.display_name()))
        }),
        QuitApp(name) => with_running(&name, |e, mine| {
            if PetAppMatcher::is_protected(&e.bundle_id) { return PetCommandOutcome::new(false, format!("{} is never quit by Nacho.", e.display_name())); }
            // A normal close: the app may ask to save first. Never forced.
            for w in mine { unsafe { let _ = PostMessageW(Some(HWND(w.id as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)); } }
            PetCommandOutcome::new(true, format!("Asked {} to close; it may ask the user to save first.", e.display_name()))
        }),
        ListWindows => PetDesktopRunner::list_windows(&screens().0, &windows()),
        LayoutWindows(items) => {
            let (full, works) = screens();
            let plan = PetDesktopRunner::plan_layout(&items, &windows(), &full, &works, gap());
            let refused = plan.moves.iter().filter(|(id, r)| !crate::snap::place(HWND(*id as *mut _), *r)).count();
            PetDesktopRunner::layout_outcome(&plan, refused)
        }
        SetSystem(change) => set_system(change),
        SetDimStrength(p) => {
            app().update(|s| s.dim_intensity = p as f64 / 100.0);
            crate::dim::options_changed();
            let note = if running(ModuleId::FocusDim) { "" } else { " Focus Dim is off; the strength is stored for when it is on." };
            PetCommandOutcome::new(true, format!("Focus Dim strength is {p}%.{note}"))
        }
        ClearShelf => PetCommandOutcome::unavailable(PetCommands::CLEAR_SHELF), // not declared on Windows (deletes)
        SaveWorkspace(name) => save_workspace(&name),
        ApplyWorkspace(name) => apply_workspace(&name),
        ListWorkspaces => list_workspaces(),
        DeviceStatus => device_state().outcome(),
        SetVolume { volume, muted } => {
            if let Some(v) = volume { crate::devices::set_volume(v as f32 / 100.0); }
            let mute_ok = muted.is_none_or(crate::devices::set_muted);
            crate::devices::refresh();
            let mut o = device_state().outcome();
            if !mute_ok { o.ok = false; o.message = format!("The sound output could not be muted. {}", o.message); }
            o
        }
        SetMicMuted(mute) => {
            crate::devices::set_mic_muted(mute);
            let o = device_state().outcome();
            let ok = app().devices.mic_muted == mute;
            PetCommandOutcome { ok, message: if ok { o.message } else { format!("The microphone did not change. {}", o.message) }, ..o }
        }
        DimStatus => dim_state().outcome(None),
        DimPause | DimResume => {
            let pause = matches!(command, DimPause);
            if app().dim.paused != pause { crate::dim::toggle_pause(); }
            dim_state().outcome(Some(if pause { "Paused." } else { "Dimming again." }))
        }
        ShelfList => PetDataTools::shelf_list(&shelf_rows()),
        ShelfGive(_) | CaptureAsk(_) => PetCommandOutcome::unavailable("combo_shelf_give / combo_capture_ask"), // needs the pet chat window
        ClipboardList => PetDataTools::clipboard_list(&clip_rows()),
        ClipboardGetItem(r) => {
            let rows = clip_rows();
            let keys: Vec<&[u8]> = rows.iter().map(|r| r.key.as_slice()).collect();
            match PetDataTools::pick(&r, &keys, "clipboard item") { Ok(i) => PetDataTools::clipboard_content(i + 1, &rows[i]), Err(o) => o }
        }
        ClipboardCopyAgain(r) => {
            let rows = clip_rows();
            let keys: Vec<&[u8]> = rows.iter().map(|r| r.key.as_slice()).collect();
            match PetDataTools::pick(&r, &keys, "clipboard item") {
                Ok(i) if crate::clipboard::copy_again(i) => PetDataTools::clipboard_copied_again(i + 1, rows[i].kind),
                Ok(i) => PetCommandOutcome::new(false, format!("Clipboard item {} is gone. List again.", i + 1)),
                Err(o) => o,
            }
        }
        CaptureListPinned => {
            let sizes: Vec<(u32, u32)> = app().capture.float_windows().iter().map(|h| unsafe {
                let mut r = RECT::default();
                let _ = GetClientRect(HWND(*h as *mut _), &mut r);
                ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32)
            }).collect();
            PetDataTools::pinned_captures(&sizes)
        }
        AwakeStatus => awake_state().outcome(None),
        AwakeStart { minutes } => {
            crate::awake::set(true, (minutes > 0).then_some(minutes as u64));
            awake_state().outcome(Some("Started."))
        }
        AwakeStop => {
            crate::awake::set(false, None);
            awake_state().outcome(Some("Stopped."))
        }
    }
}

// MARK: gadgets

fn gadget_states() -> BTreeMap<String, String> {
    ModuleId::ALL.iter().map(|m| (PetGadget::Module(*m).raw_value(), match app().state(*m) {
        State::Running => "on".to_string(),
        State::Off => "off".to_string(),
        State::Failed(_) => "failed".to_string(),
    })).collect()
}

fn gadgets_text() -> String {
    let on: Vec<&str> = ModuleId::ALL.iter().filter(|m| running(**m)).map(|m| m.name()).collect();
    if on.is_empty() { "No gadget is on.".into() } else { format!("{} gadget{} on: {}.", on.len(), if on.len() == 1 { "" } else { "s" }, on.join(", ")) }
}

fn set_gadget(m: ModuleId, on: bool) -> PetCommandOutcome {
    app().set_enabled(m, on);
    let mut o = match (on, app().state(m)) {
        (true, State::Running) => PetCommandOutcome::new(true, format!("{} is now on.", m.name())),
        (false, State::Off) => PetCommandOutcome::new(true, format!("{} is now off.", m.name())),
        (_, State::Failed(e)) => PetCommandOutcome::new(false, format!("{} could not start: {e}", m.name())),
        (_, s) => PetCommandOutcome::new(false, format!("{} is {:?}.", m.name(), s)),
    };
    o.gadgets = Some(gadget_states());
    o
}

fn awake_state() -> PetAwakeState {
    let a = &app().awake;
    PetAwakeState {
        manual: a.manual,
        seconds_left: a.until.map(|u| u.saturating_duration_since(std::time::Instant::now()).as_secs()),
        by_app: a.by_app.clone(),
        guarded: a.guarded,
    }
}

fn dim_state() -> PetDimState { PetDimState { paused: app().dim.paused, strength: app().settings.dim_intensity } }

fn device_state() -> PetDeviceState {
    let d = &app().devices;
    PetDeviceState {
        output: d.outputs.iter().find(|x| x.default).map(|x| x.name.clone()),
        volume: Some(d.volume as f64),
        muted: d.muted,
        input: d.inputs.iter().find(|x| x.default).map(|x| x.name.clone()),
        mic_muted: d.mic_muted,
    }
}

fn set_system(change: PetSystemChange) -> PetCommandOutcome {
    let mut parts = vec![];
    if let Some(v) = change.volume {
        crate::devices::set_volume(v as f32 / 100.0);
        parts.push(PetPartResult::new("volume", true, &format!("set to {v}%")));
    }
    if let Some(b) = change.brightness {
        let read_here = app().devices.monitors.is_empty();
        if read_here { crate::devices::read_monitors(); }
        let n = app().devices.monitors.len();
        for i in 0..n {
            let m = &app().devices.monitors[i];
            let v = m.min + ((m.max - m.min) as f64 * b as f64 / 100.0).round() as u32;
            crate::devices::set_brightness(i, v);
        }
        if read_here && !running(ModuleId::DeviceControl) { crate::devices::release_monitors(); }
        parts.push(if n > 0 { PetPartResult::new("brightness", true, &format!("set to {b}% on {n} display{}", if n == 1 { "" } else { "s" })) }
                   else { PetPartResult::new("brightness", false, "no display of this PC lets Nacho Combo change its brightness (DDC/CI monitors only; laptop panels are not supported yet)") });
    }
    if change.do_not_disturb.is_some() {
        parts.push(PetPartResult::new("dnd", false, "Windows has no public way for apps to switch Do Not Disturb; the user can switch it in Notifications settings"));
    }
    app().changed();
    PetPartResult::summary(&parts)
}

// MARK: windows and apps

fn screens() -> (Vec<Rect>, Vec<Rect>) {
    let m = crate::snap::monitors();
    (m.iter().map(|x| x.1).collect(), m.iter().map(|x| x.2).collect())
}

fn gap() -> f64 { app().settings.options.snap_gap as f64 }

/// Other apps' switchable windows, front to back, with their program and visible frame.
fn windows() -> Vec<PetWindowInfo> {
    let mut paths: BTreeMap<u32, String> = BTreeMap::new();
    crate::appswitch::windows_all().into_iter().filter_map(|i| {
        let path = paths.entry(i.pid).or_insert_with(|| crate::appswitch::exe_path(i.pid)).clone();
        if path.is_empty() { return None; }
        let frame = crate::snap::visible_frame(HWND(i.id as *mut _))?;
        Some(PetWindowInfo { id: i.id, path, app: i.app, frame, minimized: i.minimized })
    }).collect()
}

fn with_running(name: &str, f: impl FnOnce(&PetAppEntry, Vec<&PetWindowInfo>) -> PetCommandOutcome) -> PetCommandOutcome {
    let ws = windows();
    let running = PetDesktopRunner::running_entries(&ws);
    // The installed list (Start menu, slow) only to tell "not running, open it" apart from "no such app".
    let found = PetDesktopRunner::lookup(name, &running, &[], true).or_else(|_| PetDesktopRunner::lookup(name, &running, &installed_apps(), true));
    match found {
        Err(o) => o,
        Ok(e) => { let mine = PetDesktopRunner::windows_of(&e.bundle_id, &ws); f(&e, mine) }
    }
}

fn arrange(action: SnapAction, name: Option<&str>) -> PetCommandOutcome {
    let ws = windows();
    let target = match name {
        None => ws.first().cloned(),
        Some(n) => match PetDesktopRunner::lookup(n, &PetDesktopRunner::running_entries(&ws), &[], true) {
            Err(o) => return o,
            Ok(e) => PetDesktopRunner::windows_of(&e.bundle_id, &ws).first().map(|w| (*w).clone()),
        },
    };
    let Some(w) = target else { return PetCommandOutcome::new(false, "There is no window to move.") };
    app().snap.notice = None;
    crate::snap::run_on(HWND(w.id as *mut _), action);
    if app().snap.notice.is_some() { return PetCommandOutcome::new(false, PetPermissionHelp::ELEVATED_WINDOWS); }
    PetCommandOutcome::new(true, format!("Moved {}'s window to {}.", w.app, action.name()))
}

fn start_program(path: &str) -> bool {
    let r = unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &HSTRING::from(path), None, None, SW_SHOWNORMAL) };
    r.0 as isize > 32
}

fn open_app(name: &str) -> PetCommandOutcome {
    let ws = windows();
    let running = PetDesktopRunner::running_entries(&ws);
    let installed = installed_apps();
    let e = match PetDesktopRunner::lookup(name, &running, &installed, false) { Ok(e) => e, Err(o) => return o };
    if let Some(w) = PetDesktopRunner::windows_of(&e.bundle_id, &ws).first() {
        crate::appswitch::raise(w.id, w.minimized);
        return PetCommandOutcome::new(true, format!("{} was already open; switched to it.", e.display_name()));
    }
    let Some(link) = installed_links().into_iter().find(|(_, exe)| exe_name(exe) == e.bundle_id).map(|(l, _)| l) else {
        return PetCommandOutcome::new(false, format!("{} is not installed any more.", e.display_name()));
    };
    if start_program(&link.to_string_lossy()) { PetCommandOutcome::new(true, format!("Opened {}.", e.display_name())) }
    else { PetCommandOutcome::new(false, format!("{} did not open.", e.display_name())) }
}

/// Start menu shortcuts (this user's and all users'), with the program each one starts.
fn installed_links() -> Vec<(PathBuf, String)> {
    let mut roots = vec![];
    for var in ["APPDATA", "ProgramData"] {
        if let Some(d) = std::env::var_os(var) { roots.push(PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs")); }
    }
    let mut out = vec![];
    fn walk(dir: &Path, depth: u32, out: &mut Vec<(PathBuf, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { if depth < 3 { walk(&p, depth + 1, out); } continue; }
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) { continue; }
            if let Some(target) = link_target(&p).filter(|t| t.to_lowercase().ends_with(".exe")) { out.push((p, target)); }
            if out.len() >= 2000 { return; }
        }
    }
    for r in roots { walk(&r, 0, &mut out); }
    out
}

fn link_target(lnk: &Path) -> Option<String> {
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ).ok()?;
        let mut buf = [0u16; 1040];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let s = String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(0)]);
        (!s.is_empty()).then_some(s)
    }
}

/// Installed apps by the shortcut's name (localized, e.g. "계산기") and the exe name; uninstallers left out.
fn installed_apps() -> Vec<PetAppEntry> {
    installed_links().into_iter().filter_map(|(lnk, exe)| {
        let name = lnk.file_stem()?.to_string_lossy().to_string();
        let lower = name.to_lowercase();
        // i18n-exempt:begin (matches an installer link name, not drawn)
        if lower.contains("uninstall") || name.contains("제거") { return None; }
        // i18n-exempt:end
        let id = exe_name(&exe);
        let stem = id.trim_end_matches(".exe").to_string();
        Some(PetAppEntry { bundle_id: id, names: vec![name, stem], running: false })
    }).collect()
}

// MARK: workspaces

fn workspaces_path() -> PathBuf { app().dir.join("workspaces.json") }

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) }

fn save_workspace(name: &str) -> PetCommandOutcome {
    let gadgets: Vec<(ModuleId, bool)> = PetCommands::toggleable().into_iter().map(|m| (m, running(m))).collect();
    let ws = PetDesktopRunner::workspace(name, now(), &windows(), &screens().0, &gadgets);
    let mut store = DesktopWorkspaceStore::load(&workspaces_path());
    let summary = ws.summary();
    let saved = store.save(ws.clone());
    if saved == SaveResult::Full {
        return PetCommandOutcome::new(false, format!("{} workspaces are saved already; ask the user which one to replace (save with its name).", DesktopWorkspaceStore::MAX_WORKSPACES));
    }
    if store.write(&workspaces_path()).is_err() { return PetCommandOutcome::new(false, "The workspace could not be written to disk."); }
    let mut o = PetCommandOutcome::new(true, format!("{} workspace \"{}\": {summary}.", if saved == SaveResult::Replaced { "Replaced" } else { "Saved" }, ws.name));
    o.details = json!({"workspace": {"name": ws.name, "apps": ws.apps.iter().map(|a| a.name.clone()).collect::<Vec<_>>(), "windows": ws.windows.len()}}).as_object().cloned();
    o
}

fn apply_workspace(name: &str) -> PetCommandOutcome {
    let store = DesktopWorkspaceStore::load(&workspaces_path());
    let Some(ws) = store.workspace(name).cloned() else {
        let names: Vec<String> = store.workspaces.iter().map(|w| w.name.clone()).collect();
        return PetCommandOutcome::new(false, format!("No workspace is named \"{name}\". Saved: {}.", if names.is_empty() { "none".into() } else { names.join(", ") }));
    };
    let mut report = WorkspaceApplyReport::new(&ws.name);
    let (full, _) = screens();
    let plan = PetDesktopRunner::plan_apply(&ws, &windows(), &full);
    for a in &plan.open {
        if !Path::new(&a.bundle_id).is_file() { report.missing_apps.push(a.name.clone()); }
        else if start_program(&a.bundle_id) { report.opened_apps.push(a.name.clone()); }
        else { report.failed_apps.push(a.name.clone()); }
    }
    report.saved_windows = ws.windows.len();
    let refused = plan.moves.iter().filter(|(id, r)| !crate::snap::place(HWND(*id as *mut _), *r)).count();
    report.placed_windows = plan.moves.len() - refused;
    report.windows_need_permission = refused > 0;
    for (m, on) in ws.gadget_switches() {
        if running(m) == on { continue; }
        app().set_enabled(m, on);
        if (app().state(m) == State::Running) == on { report.gadgets_changed.push(format!("{} {}", m.name(), if on { "on" } else { "off" })); }
        else { report.gadgets_refused.push(format!("{} did not switch", m.name())); }
    }
    report.outcome()
}

fn list_workspaces() -> PetCommandOutcome {
    let store = DesktopWorkspaceStore::load(&workspaces_path());
    if store.workspaces.is_empty() {
        let mut o = PetCommandOutcome::new(true, "No workspace is saved yet.");
        o.details = json!({"workspaces": []}).as_object().cloned();
        return o;
    }
    let list: Vec<Value> = store.workspaces.iter().map(|w| json!({"name": w.name, "apps": w.apps.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
        "windows": w.windows.len(), "saved_at": crate::core::pet::desktop::iso8601(w.saved_at)})).collect();
    let text: Vec<String> = store.workspaces.iter().map(|w| format!("\"{}\" ({})", w.name, w.summary())).collect();
    let mut o = PetCommandOutcome::new(true, format!("{} saved workspace{}: {}.", list.len(), if list.len() == 1 { "" } else { "s" }, text.join(", ")));
    o.details = json!({"workspaces": list}).as_object().cloned();
    o
}

// MARK: data gadgets

fn clip_rows() -> Vec<PetClipRow> {
    use crate::clipboard::Content;
    app().clipboard.items.iter().map(|c| match &c.content {
        Content::Text(t) => {
            let link = t.trim().starts_with("http://") || t.trim().starts_with("https://");
            PetClipRow { key: t.as_bytes().to_vec(), kind: if link && !t.trim().contains(char::is_whitespace) { PetClipKind::Link } else { PetClipKind::Text },
                         text: t.clone(), bytes: 0, pinned: c.pinned }
        }
        Content::Files(f) => {
            let text = f.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>().join("\n");
            PetClipRow { key: text.as_bytes().to_vec(), kind: PetClipKind::Files, text, bytes: 0, pinned: c.pinned }
        }
        Content::Image { dib, .. } => PetClipRow { key: dib.clone(), kind: PetClipKind::Image, text: String::new(), bytes: dib.len(), pinned: c.pinned },
    }).collect()
}

fn shelf_rows() -> Vec<PetShelfRow> {
    app().shelf.shelf.items.iter().map(|i| {
        let (kind, size) = match &i.kind {
            Kind::File => (PetShelfKind::File, std::fs::metadata(&i.path).ok().map(|m| m.len())),
            Kind::Image => (PetShelfKind::Image, std::fs::metadata(&i.path).ok().map(|m| m.len())),
            Kind::Text(t) => (PetShelfKind::Text, Some(t.chars().count() as u64)),
            Kind::Link(u) => (PetShelfKind::Link, Some(u.chars().count() as u64)),
        };
        let name = i.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| i.title.clone());
        PetShelfRow { key: format!("{}|{}", i.path.display(), i.title).into_bytes(), name, kind, size }
    }).collect()
}

// MARK: debug control (`NachoCombo ctl pet …`, only with --debug-ctl)

/// `pet declaration` writes what hello would declare; `pet <capability> [json]` runs one request as the connector
/// would (refused unless declared) and writes the result. Both go to `pet-ctl.json` in the app's folder; clipboard text
/// is replaced by its length there, so the file never holds content.
pub fn ctl(a: &[String]) {
    let decl = declaration();
    let out = match a.first().map(String::as_str) {
        Some("declaration") | None => json!({"capabilities": decl, "always_allowed": trust().always_allowed()}),
        Some(name) => {
            let declared = decl.iter().any(|c| PetCommands::resolve(name).is_some_and(|l| c["name"] == l));
            let params: Option<Value> = a.get(1).and_then(|p| serde_json::from_str(p).ok());
            if !declared { json!({"error": format!("not allowed: {name} is not declared by this Nacho Combo")}) }
            else {
                let approval = decl.iter().find(|c| PetCommands::resolve(name).is_some_and(|l| c["name"] == l)).and_then(|c| c.get("approval")).cloned();
                match PetCommands::parse(name, params.as_ref()) {
                    Err(p) => json!({"error": p.message()}),
                    Ok(c) => {
                        let mut r = Value::Object(run(c).result_json());
                        if let Some(t) = r.get("text").and_then(Value::as_str).map(|t| t.chars().count()) { r["text"] = json!(format!("({t} characters, not written here)")); }
                        json!({"capability": name, "approval": approval, "result": r})
                    }
                }
            }
        }
    };
    let _ = std::fs::write(app().dir.join("pet-ctl.json"), serde_json::to_vec_pretty(&out).unwrap_or_default());
}
