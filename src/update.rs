//! In-app updates from the project's GitHub Releases (General ▸ Updates), without an update server or keys of its own.
//!
//! Checking: `GET /repos/mafianacho/nachocombo-windows/releases/latest` (no sign-in) when the user presses Check Now,
//! and, only if they turned automatic checks on, at launch and then every 6 hours, day or week. Drafts, pre-releases
//! and versions this PC went back from are ignored; a failed check stays quiet unless the user asked.
//!
//! Installing, always the user's choice: a found update is announced once (tray balloon, General page). "Update…"
//! asks first; then the release zip is downloaded, checked against its listed size and `SHA256SUMS.txt`, the exe is
//! taken out and must say it is that version, and only then the running exe is renamed aside, the new one takes its
//! place and starts, and this copy quits. If the new exe cannot be started the old one gets its name back.
//!
//! Rolling back: the version before is kept (one copy) and "Go Back to …" puts it back. A new version that does not
//! get through its first launches twice is replaced by the previous one at the next launch (as on the Mac, this runs
//! inside the new version, so one that crashes before its first line cannot roll itself back; the release page keeps
//! every version). No administrator rights: a copy in a folder it cannot write to says so and links the release page.
use crate::app::{self, app, later};
use crate::core::update::{self, Asset, Decision, Version};
use crate::core::update_store::{ExeInfo, Fetch, LaunchAction, Notice, UpdateStore};
use std::path::{Path, PathBuf};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_UPDATE: u32 = WM_APP + 21;
/// A launch counts as healthy once it has run this long (or quits normally).
const HEALTHY_AFTER_MS: u32 = 20_000;
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    /// Found; waits for the user's click.
    Available { version: String, summary: String },
    Downloading(String),
    Restarting,
    Failed(String),
}

#[derive(Clone)]
struct Offer { version: Version, zip: Asset, sums: Asset, page: Option<String>, summary: String }

#[derive(Default)]
pub struct Updater {
    pub status: Status,
    offer: Option<Offer>,
    busy: bool,
    /// Set once this copy was replaced and is about to quit: it is not "healthy" and must not swap again.
    replaced: bool,
    /// Why this copy cannot replace itself, worked out once at launch.
    pub replace_problem: Option<String>,
    /// The version "Go Back" would restore.
    pub previous: Option<String>,
    pub last_check: Option<u64>,
    checks: u32,
}

enum Done { Checked { manual: bool, data: Option<Vec<u8>> }, Staged(std::result::Result<Version, String>) }

fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }
fn current() -> Version { Version::parse(VERSION).unwrap_or(Version { major: 0, minor: 0, patch: 0, pre: None }) }
fn target() -> PathBuf { std::env::current_exe().unwrap_or_default() }

/// `%LOCALAPPDATA%\NachoCombo\Updates` (big files stay out of the roaming profile); beside the settings when a test
/// gives the app its own folder.
fn store() -> UpdateStore {
    let a = app();
    let root = if std::env::var_os("NACHOCOMBO_DIR").is_some() { a.dir.join("Updates") } else {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("NachoCombo").join("Updates")).unwrap_or_else(|| a.dir.join("Updates"))
    };
    UpdateStore::new(root)
}

/// The feed: GitHub, or (for testing) `NACHOCOMBO_UPDATE_FEED` when it is a plain-http address on this PC.
fn feed() -> String {
    std::env::var("NACHOCOMBO_UPDATE_FEED").ok().filter(|f| update::feed_allowed(f)).unwrap_or_else(|| update::LATEST_URL.to_string())
}

// ---------- launch and quit ----------

/// Started by an update or a rollback: waits for the copy that started this one to quit, so the single-instance
/// check and the files beside the exe are free.
pub fn wait_for(pid: u32) {
    use windows::Win32::System::Threading::*;
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, 20_000);
            let _ = CloseHandle(h);
        }
    }
}

/// First thing at launch, before any gadget starts. Files the exe a swap left beside this one and rolls back a version
/// that failed its first launches (then this copy restarts: returns true, stop here).
pub fn at_launch() -> bool {
    let store = store();
    let cur = current();
    let t = target();
    let u = &mut app().updater;
    u.replace_problem = UpdateStore::replace_problem(&t);
    let action = if u.replace_problem.is_none() { store.launch_began(&cur, &t, &VersionResource) } else { LaunchAction::Normal };
    if let LaunchAction::RollBack { .. } = action {
        let before = store.ledger();
        match store.roll_back(&t, &cur, &VersionResource) {
            Ok(v) => {
                if restart(&t) { return true; }
                let _ = store.undo_swap(&t, &before, &v, true);
                u.status = Status::Failed(format!("{} could not be started, so {cur} stays.", t.display()));
            }
            Err(p) => u.status = Status::Failed(format!("Going back to the previous version failed: {p}")),
        }
    }
    let l = store.ledger();
    u.last_check = l.last_check;
    u.previous = store.previous().map(|p| p.version.to_string());
    unsafe { SetTimer(Some(app().main), app::TIMER_UPDATE_HEALTHY, HEALTHY_AFTER_MS, None); }
    false
}

/// After the tray icon exists: the once-only notice of an update or a rollback, then the schedule.
pub fn after_launch() {
    match store().take_notice(&current()) {
        Some(Notice::Updated { to }) => app().tray.balloon("NachoCombo was updated", &format!("Now running {to}.")),
        Some(Notice::RolledBack { from, to }) => app().tray.balloon("NachoCombo went back to the previous version", &format!("{from} did not start properly, so {to} is back.")),
        None => {}
    }
    unsafe { SetTimer(Some(app().main), app::TIMER_UPDATE_CHECK, 60 * 60 * 1000, None); }
    check_if_due();
}

pub fn mark_healthy() {
    unsafe { let _ = KillTimer(Some(app().main), app::TIMER_UPDATE_HEALTHY); }
    if !app().updater.replaced { store().launch_healthy(&current()); }
}

/// Quitting normally counts as a healthy launch.
pub fn on_quit() { mark_healthy(); }

/// Starts `exe` once this process is gone (it waits for this pid) and quits this copy.
fn restart(exe: &Path) -> bool {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--after-update").arg(std::process::id().to_string());
    if app().debug_ctl { cmd.arg("--debug-ctl"); }
    if cmd.spawn().is_err() { return false; }
    app().updater.replaced = true;
    true
}

// ---------- checking ----------

/// Every hour: checks when automatic checks are on and the chosen interval has passed.
pub fn check_if_due() {
    let a = app();
    let o = &a.settings.options;
    if o.update_check && update::check_due(a.updater.last_check, now(), o.update_interval_hours) { check(false); }
}

pub fn check(manual: bool) {
    let u = &mut app().updater;
    if u.busy || matches!(u.status, Status::Checking | Status::Downloading(_) | Status::Restarting) { return; }
    u.busy = true;
    u.checks += 1;
    if manual { u.status = Status::Checking; app().changed(); }
    let main = app().main.0 as isize;
    let url = feed();
    std::thread::spawn(move || {
        let data = get(&url, true);
        post(main, Done::Checked { manual, data });
    });
}

fn post(main: isize, d: Done) {
    let p = Box::into_raw(Box::new(d));
    unsafe { if PostMessageW(Some(HWND(main as *mut _)), WM_UPDATE, WPARAM(0), LPARAM(p as isize)).is_err() { drop(Box::from_raw(p)); } }
}

/// A worker finished (UI thread).
pub fn on_message(lp: LPARAM) {
    let d = unsafe { Box::from_raw(lp.0 as *mut Done) };
    let a = app();
    a.updater.busy = false;
    match *d {
        Done::Checked { manual, data } => finish_check(manual, data),
        Done::Staged(Ok(v)) => install(v),
        Done::Staged(Err(p)) => {
            let v = a.updater.offer.as_ref().map(|o| o.version.to_string()).unwrap_or_default();
            a.updater.status = Status::Failed(format!("Update {v} was not installed: {p} Nothing was changed."));
        }
    }
    a.changed();
}

fn finish_check(manual: bool, data: Option<Vec<u8>>) {
    let store = store();
    let mut ledger = store.ledger();
    ledger.last_check = Some(now());
    let u = &mut app().updater;
    u.last_check = ledger.last_check;
    let before = u.status.clone();
    let Some(release) = data.as_deref().and_then(update::Release::parse) else {
        store.save_ledger(&ledger);
        // Offline or GitHub unreachable: only a check the user asked for says so.
        u.status = if manual { Status::Failed("Could not reach GitHub to look for updates. Try again later.".into()) } else { before };
        return;
    };
    match update::decide(VERSION, &release, &ledger.skipped) {
        Decision::Available { version, zip, sums } => {
            let summary = update::summary(release.body.as_deref(), 4, 320);
            u.offer = Some(Offer { version: version.clone(), zip, sums, page: release.page.clone(), summary: summary.clone() });
            u.status = Status::Available { version: version.to_string(), summary };
            // Announced once per version, so a check every few hours does not repeat the balloon.
            if ledger.announced.as_deref() != Some(&version.to_string()) {
                ledger.announced = Some(version.to_string());
                app().tray.balloon("A NachoCombo update is available", &format!("Version {version} is ready to install. Open Settings ▸ General to update."));
            }
        }
        Decision::UpToDate => { u.offer = None; u.status = Status::UpToDate; }
        Decision::Ignored(_) => u.status = match before { Status::Available { .. } => before, _ if manual => Status::UpToDate, _ => before },
    }
    store.save_ledger(&ledger);
}

// ---------- user actions ----------

/// "Update…": asks first, then downloads, verifies, swaps and restarts.
pub fn confirm_and_install() {
    let u = &mut app().updater;
    let Some(offer) = u.offer.clone() else { return };
    if u.busy { return; }
    if let Some(p) = u.replace_problem.clone() { u.status = Status::Failed(p); app().changed(); return; }
    let t = |s: &str| crate::i18n::t(s).into_owned();
    let title = t(&format!("Restart NachoCombo to update to {}?", offer.version));
    let note = t("The download is checked against the release's SHA256SUMS.txt before anything changes. This version is kept, so you can go back to it in Settings ▸ General.");
    let body = if offer.summary.is_empty() { note } else { format!("{}\n\n{note}", offer.summary) };
    // A modal loop: no reference into the app is held across it.
    if !ask(&title, &body, &t("Update and Restart"), &t("Later")) { return; }
    let u = &mut app().updater;
    if u.busy { return; }
    u.status = Status::Downloading(format!("Downloading {}…", offer.version));
    u.busy = true;
    app().changed();
    let main = app().main.0 as isize;
    let root = store().root;
    let url = feed();
    std::thread::spawn(move || {
        let s = UpdateStore::new(root);
        let r = s.stage(&offer.version, &offer.zip, &offer.sums, &url, &WinHttp, &VersionResource).map(|c| c.version).map_err(|p| p.0);
        post(main, Done::Staged(r));
    });
}

/// The verified copy is staged: swap, start it, quit. Back to the old exe if the new one cannot start.
fn install(v: Version) {
    let store = store();
    let t = target();
    let before = store.ledger();
    let u = &mut app().updater;
    match store.apply_staged(&t, &current(), &VersionResource) {
        Ok(_) if restart(&t) => { u.status = Status::Restarting; app::quit(); }
        Ok(_) => {
            let back = store.undo_swap(&t, &before, &v, false);
            u.status = Status::Failed(match back { Ok(()) => format!("{v} was put in place but could not be started, so this copy was kept."), Err(p) => p.0 });
        }
        Err(p) => u.status = Status::Failed(format!("The update was not installed: {p} This copy was kept.")),
    }
}

/// "Go Back to …": asks first, then puts the kept copy back and restarts. The version left is not offered again.
pub fn confirm_rollback() {
    let u = &mut app().updater;
    let Some(prev) = u.previous.clone() else { return };
    if u.busy { return; }
    if let Some(p) = u.replace_problem.clone() { u.status = Status::Failed(p); app().changed(); return; }
    let t = |s: &str| crate::i18n::t(s).into_owned();
    let cur = current();
    let title = t(&format!("Go back to NachoCombo {prev}?"));
    let body = t(&format!("NachoCombo {cur} is removed and {prev} restarts in its place. Updates will not offer {cur} again; a newer version still comes."));
    if !ask(&title, &body, &t(&format!("Go Back to {prev}")), &t("Cancel")) { return; }
    let store = store();
    let target = target();
    let before = store.ledger();
    let result = store.roll_back(&target, &cur, &VersionResource);
    let u = &mut app().updater;
    match result {
        Ok(_) if restart(&target) => { app().updater.status = Status::Restarting; app::quit(); }
        Ok(v) => { let _ = store.undo_swap(&target, &before, &v, true); u.status = Status::Failed("The previous version could not be started. This copy was kept.".into()); }
        Err(p) => { u.status = Status::Failed(format!("{p} This copy was kept.")); u.previous = store.previous().map(|p| p.version.to_string()); }
    }
    app().changed();
}

pub fn release_page() -> String {
    app().updater.offer.as_ref().and_then(|o| o.page.clone()).unwrap_or_else(|| format!("https://github.com/{}/releases/latest", update::REPO))
}

/// A task dialog with two buttons; true for the first. Owned by the settings window when it is open.
fn ask(title: &str, body: &str, yes: &str, no: &str) -> bool {
    let w = |s: &str| HSTRING::from(s);
    let (wt, wtitle, wbody, wyes, wno) = (w("NachoCombo"), w(title), w(body), w(yes), w(no));
    let buttons = [TASKDIALOG_BUTTON { nButtonID: 100, pszButtonText: PCWSTR(wyes.as_ptr()) }, TASKDIALOG_BUTTON { nButtonID: 101, pszButtonText: PCWSTR(wno.as_ptr()) }];
    let owner = crate::pages::window().unwrap_or(app().main);
    let cfg = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        dwFlags: TDF_POSITION_RELATIVE_TO_WINDOW | TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: PCWSTR(wt.as_ptr()),
        pszMainInstruction: PCWSTR(wtitle.as_ptr()),
        pszContent: PCWSTR(wbody.as_ptr()),
        cButtons: 2,
        pButtons: buttons.as_ptr(),
        nDefaultButton: 100,
        ..Default::default()
    };
    let mut pressed = 0i32;
    unsafe { TaskDialogIndirect(&cfg, Some(&mut pressed), None, None).is_ok() && pressed == 100 }
}

pub fn diagnostics() -> serde_json::Value {
    let u = &app().updater;
    serde_json::json!({
        "status": format!("{:?}", u.status),
        "offer": u.offer.as_ref().map(|o| o.version.to_string()),
        "previous": u.previous,
        "replaceProblem": u.replace_problem,
        "lastCheck": u.last_check,
        "checks": u.checks,
        "feed": feed(),
    })
}

/// `ctl update check|install|rollback` (only with `--debug-ctl`): the same steps the buttons take, dialogs included.
pub fn ctl(a: &[String]) {
    match a.first().map(String::as_str) {
        Some("check") => check(true),
        Some("install") => later(confirm_and_install),
        Some("rollback") => later(confirm_rollback),
        _ => {}
    }
}

// ---------- Windows pieces ----------

/// The ProductVersion string of an exe's version resource (`build.rs` writes the Cargo version there).
struct VersionResource;
impl ExeInfo for VersionResource {
    fn version_of(&self, exe: &Path) -> Option<String> {
        use windows::Win32::Storage::FileSystem::*;
        let path = HSTRING::from(exe.as_os_str());
        unsafe {
            let n = GetFileVersionInfoSizeW(&path, None);
            if n == 0 { return None; }
            let mut buf = vec![0u8; n as usize];
            GetFileVersionInfoW(&path, None, n, buf.as_mut_ptr() as *mut _).ok()?;
            let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut len = 0u32;
            if !VerQueryValueW(buf.as_ptr() as *const _, w!("\\StringFileInfo\\040904B0\\ProductVersion"), &mut p, &mut len).as_bool() || p.is_null() || len == 0 { return None; }
            let s = std::slice::from_raw_parts(p as *const u16, len as usize);
            let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
            Some(String::from_utf16_lossy(&s[..end]))
        }
    }
}

struct WinHttp;
impl Fetch for WinHttp { fn fetch(&self, url: &str) -> Option<Vec<u8>> { get(url, false) } }

/// Blocking GET with WinHTTP (system proxy, TLS, redirects to GitHub's file host). The body only for status 200.
fn get(url: &str, api: bool) -> Option<Vec<u8>> {
    use windows::Win32::Networking::WinHttp::*;
    let u = update::parse_url(url)?;
    unsafe {
        let s = WinHttpOpen(&HSTRING::from(format!("NachoCombo/{VERSION}")), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if s.is_null() { return None; }
        let _ = WinHttpSetTimeouts(s, 15_000, 15_000, 30_000, 60_000);
        let c = WinHttpConnect(s, &HSTRING::from(u.host.as_str()), u.port, 0);
        let flags = if u.secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let r = if c.is_null() { std::ptr::null_mut() } else { WinHttpOpenRequest(c, w!("GET"), &HSTRING::from(u.path.as_str()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags) };
        let headers: Vec<u16> = if api { "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n" } else { "Accept: application/octet-stream\r\n" }.encode_utf16().collect();
        let mut body = None;
        if !r.is_null() && WinHttpSendRequest(r, Some(&headers), None, 0, 0, 0).is_ok() && WinHttpReceiveResponse(r, std::ptr::null_mut()).is_ok() {
            let mut status = 0u32;
            let mut len = 4u32;
            let _ = WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut status as *mut u32 as *mut _), &mut len, std::ptr::null_mut());
            if status == 200 {
                let mut data = Vec::new();
                let mut buf = vec![0u8; 64 * 1024];
                let ok = loop {
                    let mut n = 0u32;
                    if WinHttpReadData(r, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut n).is_err() { break false; }
                    if n == 0 { break true; }
                    data.extend_from_slice(&buf[..n as usize]);
                    if data.len() > update::MAX_DOWNLOAD { break false; }
                };
                if ok { body = Some(data); }
            }
        }
        for h in [r, c, s] { if !h.is_null() { let _ = WinHttpCloseHandle(h); } }
        body
    }
}
