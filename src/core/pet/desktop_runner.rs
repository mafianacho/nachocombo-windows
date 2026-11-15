//! The decisions of the Windows desktop actions (macOS `PetDesktopActions.swift`), kept out of the Win32 shell so they
//! are tested on any OS: which app a name means, what the app and window lists say (never window titles), where each
//! window of a layout goes, what a workspace saves and what applying it does. The shell only reads windows, moves
//! them and starts programs.
//!
//! Windows changes: an app is its program (`bundle_id` = the exe's file name, compared without case); workspaces keep
//! the full exe path there so a missing app can be started again (the macOS file keeps bundle ids, so the two files do
//! not mix); display and window frames use top-left screen coordinates.
use super::commands::PetCommandOutcome;
use super::desktop::{DesktopWorkspace, DesktopWorkspaceApp, PetAppEntry, PetAppMatch, PetAppMatcher, PetDisplays, PetWindowPlacement, SavedWindow};
use crate::core::layout::{self, Rect};
use crate::core::settings::ModuleId;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// One top-level window of another app, front to back (the shell's `EnumWindows` order, own windows left out).
#[derive(Clone, Debug, PartialEq)]
pub struct PetWindowInfo {
    pub id: isize,
    /// The program's full path (`C:\Windows\notepad.exe`).
    pub path: String,
    /// The name people know it by (file description, or the exe name).
    pub app: String,
    pub frame: Rect,
    pub minimized: bool,
}

impl PetWindowInfo {
    /// `notepad.exe`
    pub fn exe(&self) -> String { exe_name(&self.path) }
}

/// The file name of a path, lower case (`C:\Windows\Notepad.EXE` → `notepad.exe`).
pub fn exe_name(path: &str) -> String { path.rsplit(['\\', '/']).next().unwrap_or(path).trim().to_lowercase() }

pub fn same_exe(a: &str, b: &str) -> bool { !a.trim().is_empty() && exe_name(a) == exe_name(b) }

/// An app found by name, or the answer to give instead.
pub type PetLookup = Result<PetAppEntry, PetCommandOutcome>;

/// What a layout does: windows to move (id → target frame) and what could not be placed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetLayoutPlan {
    pub moves: Vec<(isize, Rect)>,
    pub missing: Vec<String>,
}

/// What applying a workspace does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetApplyPlan {
    /// Saved apps with no window open now: start them (full path).
    pub open: Vec<DesktopWorkspaceApp>,
    /// Open windows to put back (id → saved frame, pulled onto a display that is there).
    pub moves: Vec<(isize, Rect)>,
    /// Saved windows whose app has fewer windows open now.
    pub unmatched: usize,
}

pub struct PetDesktopRunner;

impl PetDesktopRunner {
    /// At most this many windows in `combo_list_windows` (macOS: 60).
    pub const MAX_LISTED_WINDOWS: usize = 60;
    pub const MAX_INSTALLED: usize = 300;

    /// One entry per running program, front first: names = its display name and the exe name.
    pub fn running_entries(windows: &[PetWindowInfo]) -> Vec<PetAppEntry> {
        let mut out: Vec<PetAppEntry> = Vec::new();
        for w in windows {
            let exe = w.exe();
            if exe.is_empty() || out.iter().any(|e| e.bundle_id == exe) { continue; }
            let stem = exe.trim_end_matches(".exe").to_string();
            let mut names = vec![w.app.clone()];
            if !names.iter().any(|n| PetAppMatcher::fold(n) == PetAppMatcher::fold(&stem)) { names.push(stem); }
            names.retain(|n| !n.trim().is_empty());
            out.push(PetAppEntry { bundle_id: exe, names, running: true });
        }
        out
    }

    /// The app a name means (macOS `lookup`): running apps first; with `running_only` an installed but closed app
    /// answers "use combo_open_app".
    pub fn lookup(name: &str, running: &[PetAppEntry], installed: &[PetAppEntry], running_only: bool) -> PetLookup {
        let mut entries = running.to_vec();
        if !running_only { entries.extend(installed.iter().cloned()); }
        match PetAppMatcher::match_app(name, &entries) {
            PetAppMatch::Found(e) => Ok(e),
            PetAppMatch::Ambiguous(list) => {
                let names: Vec<String> = list.iter().map(|e| format!("{} ({})", e.display_name(), e.bundle_id)).collect();
                let mut o = PetCommandOutcome::new(false, format!("More than one app matches \"{name}\": {}. Ask the user which one.", names.join(", ")));
                o.details = json!({"candidates": list.iter().map(|e| e.display_name()).collect::<Vec<_>>()}).as_object().cloned();
                Err(o)
            }
            PetAppMatch::None { suggestions } => {
                if running_only {
                    if let PetAppMatch::Found(e) = PetAppMatcher::match_app(name, installed) {
                        return Err(PetCommandOutcome::new(false, format!("{} is not running. Use combo_open_app to open it.", e.display_name())));
                    }
                }
                let mut text = format!("No {}app on this PC is named \"{name}\".", if running_only { "running " } else { "" });
                if !suggestions.is_empty() { text += &format!(" Similar: {}.", suggestions.join(", ")); }
                let mut o = PetCommandOutcome::new(false, text);
                o.details = json!({"suggestions": suggestions}).as_object().cloned();
                Err(o)
            }
        }
    }

    /// Running apps (name, exe, front, minimized = every window minimized) and, when asked, the installed names.
    pub fn list_apps(windows: &[PetWindowInfo], installed: Option<&[PetAppEntry]>) -> PetCommandOutcome {
        let running = Self::running_entries(windows);
        let apps: Vec<Value> = running.iter().enumerate().map(|(n, e)| {
            let all_min = windows.iter().filter(|w| w.exe() == e.bundle_id).all(|w| w.minimized);
            json!({"name": e.display_name(), "bundle_id": e.bundle_id, "front": n == 0, "hidden": all_min})
        }).collect();
        let names: Vec<&str> = running.iter().map(|e| e.display_name()).collect();
        let mut text = format!("{} app{} running: {}.", apps.len(), if apps.len() == 1 { " is" } else { "s are" }, names.join(", "));
        let mut d = json!({"running": apps});
        if let Some(list) = installed {
            let mut names: Vec<String> = PetAppMatcher::dedupe(list).iter().map(|e| e.display_name().to_string()).collect();
            names.sort();
            names.dedup();
            text += &format!(" {} apps are installed{}.", names.len(), if names.len() > Self::MAX_INSTALLED { " (first 300 listed)" } else { "" });
            d["installed"] = json!(names.into_iter().take(Self::MAX_INSTALLED).collect::<Vec<_>>());
        }
        let mut o = PetCommandOutcome::new(true, text);
        o.details = d.as_object().cloned();
        o
    }

    /// The windows of a running app (by exe name), front to back.
    pub fn windows_of<'a>(exe: &str, windows: &'a [PetWindowInfo]) -> Vec<&'a PetWindowInfo> {
        windows.iter().filter(|w| same_exe(&w.path, exe)).collect()
    }

    /// Displays (number left to right, size, main = the first one Windows lists) and windows (app, exe, window number
    /// within the app, display number, frame from that display's top-left corner). No titles: they are content.
    pub fn list_windows(screens: &[Rect], windows: &[PetWindowInfo]) -> PetCommandOutcome {
        let displays: Vec<Value> = PetDisplays::ordered(screens).iter().enumerate().map(|(n, &i)| {
            json!({"display": n + 1, "width": screens[i].w as i64, "height": screens[i].h as i64, "main": i == 0})
        }).collect();
        let mut order: BTreeMap<String, usize> = BTreeMap::new();
        let mut rows: Vec<Value> = Vec::new();
        for w in windows {
            let n = order.entry(w.exe()).or_insert(0);
            *n += 1;
            if rows.len() >= Self::MAX_LISTED_WINDOWS { continue; }
            let index = layout::screen_index(&w.frame, screens).unwrap_or(0);
            let s = screens.get(index).copied().unwrap_or(w.frame);
            rows.push(json!({"app": w.app, "bundle_id": w.exe(), "window": *n,
                             "display": PetDisplays::number_for_index(index, screens).unwrap_or(1),
                             "x": (w.frame.x - s.x) as i64, "y": (w.frame.y - s.y) as i64,
                             "width": w.frame.w as i64, "height": w.frame.h as i64, "minimized": w.minimized}));
        }
        let text = format!("{} display{}, {} window{} open.", displays.len(), if displays.len() == 1 { "" } else { "s" },
                           rows.len(), if rows.len() == 1 { "" } else { "s" });
        let mut o = PetCommandOutcome::new(true, text);
        o.details = json!({"displays": displays, "windows": rows}).as_object().cloned();
        o
    }

    /// Where each window of a layout goes. `screens` are the displays' full frames, `works` their work areas (same
    /// order); a window without `display` stays on the display it is on.
    pub fn plan_layout(items: &[PetWindowPlacement], windows: &[PetWindowInfo], screens: &[Rect], works: &[Rect], gap: f64) -> PetLayoutPlan {
        let running = Self::running_entries(windows);
        let mut plan = PetLayoutPlan::default();
        for item in items {
            let Ok(app) = Self::lookup(&item.app, &running, &[], true) else {
                plan.missing.push(format!("{} (not running)", item.app));
                continue;
            };
            let mine = Self::windows_of(&app.bundle_id, windows);
            let Some(w) = mine.get(item.window as usize - 1) else {
                plan.missing.push(format!("{} window {} (it has {})", app.display_name(), item.window, mine.len()));
                continue;
            };
            let index = match item.display {
                Some(d) => match PetDisplays::index_for_number(d, screens) {
                    Some(i) => i,
                    None => { plan.missing.push(format!("{} (there is no display {d})", app.display_name())); continue; }
                },
                None => layout::screen_index(&w.frame, works).unwrap_or(0),
            };
            let Some(work) = works.get(index) else { continue };
            plan.moves.push((w.id, layout::target(item.position, *work, None, gap, false)));
        }
        plan
    }

    pub fn layout_outcome(plan: &PetLayoutPlan, refused: usize) -> PetCommandOutcome {
        let placed = plan.moves.len() - refused.min(plan.moves.len());
        let mut parts = vec![format!("Placed {placed} window{}.", if placed == 1 { "" } else { "s" })];
        if !plan.missing.is_empty() { parts.push(format!("Not placed: {}.", plan.missing.join(", "))); }
        if refused > 0 { parts.push(format!("{refused} window{} could not be moved: {}", if refused == 1 { "" } else { "s" }, super::desktop::PetPermissionHelp::ELEVATED_WINDOWS)); }
        let mut o = PetCommandOutcome::new(plan.missing.is_empty() && refused == 0 && placed > 0, parts.join(" "));
        o.details = json!({"placed": placed, "missing": plan.missing}).as_object().cloned();
        o
    }

    /// `1920x1080@0,0;1280x1024@1920,0` in display order: a workspace saved on other displays is pulled onto these.
    pub fn display_signature(screens: &[Rect]) -> String {
        PetDisplays::ordered(screens).iter().map(|&i| { let s = screens[i]; format!("{}x{}@{},{}", s.w as i64, s.h as i64, s.x as i64, s.y as i64) })
            .collect::<Vec<_>>().join(";")
    }

    /// The workspace of what is open now: apps (full path), windows (path, order within the app, frame), gadgets.
    pub fn workspace(name: &str, saved_at: f64, windows: &[PetWindowInfo], screens: &[Rect], gadgets: &[(ModuleId, bool)]) -> DesktopWorkspace {
        let mut apps: Vec<DesktopWorkspaceApp> = Vec::new();
        let mut saved: Vec<SavedWindow> = Vec::new();
        let mut order: BTreeMap<String, i64> = BTreeMap::new();
        for w in windows.iter().filter(|w| !w.minimized) {
            if !apps.iter().any(|a| same_exe(&a.bundle_id, &w.path)) { apps.push(DesktopWorkspaceApp { bundle_id: w.path.clone(), name: w.app.clone() }); }
            let n = order.entry(w.exe()).or_insert(0);
            *n += 1;
            saved.push(SavedWindow::new(&w.path, &w.app, w.frame, *n));
        }
        let gadgets = gadgets.iter().filter_map(|(m, on)| {
            Some((super::commands::PetGadget::Module(*m).raw_value(), *on)).filter(|(k, _)| !k.is_empty())
        }).collect();
        DesktopWorkspace { name: name.into(), saved_at, displays: Self::display_signature(screens), apps, windows: saved, gadgets }
    }

    /// Opening what is missing and putting windows back; nothing is ever closed.
    pub fn plan_apply(ws: &DesktopWorkspace, windows: &[PetWindowInfo], screens: &[Rect]) -> PetApplyPlan {
        let mut plan = PetApplyPlan::default();
        for a in &ws.apps {
            if Self::windows_of(&a.bundle_id, windows).is_empty() { plan.open.push(a.clone()); }
        }
        for s in &ws.windows {
            let mine = Self::windows_of(&s.bundle_id, windows);
            let Some(w) = mine.get((s.order.max(1) - 1) as usize) else { plan.unmatched += 1; continue };
            let mut frame = s.frame();
            // Saved on a display that is not there now: onto the first display, inside it.
            if !screens.iter().any(|sc| sc.contains(frame.cx(), frame.cy())) {
                if let Some(dst) = screens.first() {
                    frame = Rect::new(dst.x + 40.0, dst.y + 40.0, frame.w.min(dst.w - 80.0).max(200.0), frame.h.min(dst.h - 80.0).max(150.0));
                }
            }
            plan.moves.push((w.id, frame));
        }
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::layout::SnapAction;
    use crate::core::pet::desktop::DesktopWorkspaceStore;

    fn w(id: isize, path: &str, app: &str, x: f64, y: f64) -> PetWindowInfo {
        PetWindowInfo { id, path: path.into(), app: app.into(), frame: Rect::new(x, y, 800.0, 600.0), minimized: false }
    }

    fn desk() -> Vec<PetWindowInfo> {
        vec![w(1, r"C:\Program Files\Microsoft\Edge\msedge.exe", "Microsoft Edge", 100.0, 100.0),
             w(2, r"C:\Windows\System32\notepad.exe", "메모장", 2000.0, 50.0),
             w(3, r"C:\Program Files\Microsoft\Edge\msedge.exe", "Microsoft Edge", 300.0, 200.0)]
    }

    fn screens() -> Vec<Rect> { vec![Rect::new(0.0, 0.0, 1920.0, 1080.0), Rect::new(1920.0, 0.0, 1280.0, 1024.0)] }
    fn works() -> Vec<Rect> { vec![Rect::new(0.0, 0.0, 1920.0, 1040.0), Rect::new(1920.0, 0.0, 1280.0, 984.0)] }

    #[test]
    fn running_apps_are_one_entry_per_program_front_first() {
        let e = PetDesktopRunner::running_entries(&desk());
        assert_eq!(e.iter().map(|e| e.bundle_id.as_str()).collect::<Vec<_>>(), ["msedge.exe", "notepad.exe"]);
        assert_eq!(e[1].names, ["메모장", "notepad"]);
        let o = PetDesktopRunner::list_apps(&desk(), None);
        let d = o.details.unwrap();
        assert_eq!(d["running"][0]["front"], true);
        assert_eq!(d["running"][1]["name"], "메모장");
        assert!(d.get("installed").is_none());
    }

    #[test]
    fn lookup_running_installed_and_problems() {
        let running = PetDesktopRunner::running_entries(&desk());
        let installed = vec![PetAppEntry::new("calc.exe", &["계산기", "Calculator"], false)];
        assert_eq!(PetDesktopRunner::lookup("notepad", &running, &installed, true).unwrap().bundle_id, "notepad.exe");
        assert_eq!(PetDesktopRunner::lookup("계산기", &running, &installed, false).unwrap().bundle_id, "calc.exe");
        let closed = PetDesktopRunner::lookup("계산기", &running, &installed, true).unwrap_err();
        assert!(closed.message.contains("combo_open_app"), "{}", closed.message);
        let none = PetDesktopRunner::lookup("포토샵", &running, &installed, false).unwrap_err();
        assert!(none.message.contains("this PC"));
    }

    #[test]
    fn window_list_has_numbers_and_frames_but_no_titles() {
        let o = PetDesktopRunner::list_windows(&screens(), &desk());
        let d = o.details.unwrap();
        assert_eq!(d["displays"].as_array().unwrap().len(), 2);
        let rows = d["windows"].as_array().unwrap();
        assert_eq!(rows[0]["window"], 1);
        assert_eq!(rows[2]["window"], 2, "second Edge window");
        assert_eq!(rows[1]["display"], 2);
        assert_eq!(rows[1]["x"], 80, "from the display's left edge");
        assert!(rows.iter().all(|r| r.get("title").is_none()));
    }

    #[test]
    fn layout_puts_each_window_on_its_display() {
        let items = vec![
            PetWindowPlacement { app: "Edge".into(), position: SnapAction::Left, display: None, window: 1 },
            PetWindowPlacement { app: "메모장".into(), position: SnapAction::Right, display: Some(1), window: 1 },
            PetWindowPlacement { app: "Edge".into(), position: SnapAction::Maximize, display: Some(2), window: 2 },
            PetWindowPlacement { app: "Slack".into(), position: SnapAction::Left, display: None, window: 1 },
            PetWindowPlacement { app: "notepad".into(), position: SnapAction::Left, display: None, window: 3 },
        ];
        let plan = PetDesktopRunner::plan_layout(&items, &desk(), &screens(), &works(), 0.0);
        assert_eq!(plan.moves, vec![(1, Rect::new(0.0, 0.0, 960.0, 1040.0)), (2, Rect::new(960.0, 0.0, 960.0, 1040.0)),
                                    (3, Rect::new(1920.0, 0.0, 1280.0, 984.0))]);
        assert_eq!(plan.missing, ["Slack (not running)", "메모장 window 3 (it has 1)"]);
        let o = PetDesktopRunner::layout_outcome(&plan, 1);
        assert!(!o.ok);
        assert_eq!(o.details.unwrap()["placed"], 2);
    }

    #[test]
    fn workspace_saves_paths_and_applies_without_closing() {
        let ws = PetDesktopRunner::workspace("업무 모드", 1_800_000_000.0, &desk(), &screens(), &[(ModuleId::FocusDim, true), (ModuleId::Clipboard, false)]);
        assert_eq!(ws.apps.len(), 2);
        assert_eq!(ws.apps[0].bundle_id, r"C:\Program Files\Microsoft\Edge\msedge.exe", "the path, so it can be started again");
        assert_eq!(ws.windows.iter().map(|s| s.order).collect::<Vec<_>>(), [1, 1, 2]);
        assert_eq!(ws.gadgets.get("focusDim"), Some(&true));
        assert_eq!(ws.displays, "1920x1080@0,0;1280x1024@1920,0");
        // Round trip through the store file.
        let mut store = DesktopWorkspaceStore::default();
        store.save(ws.clone());
        let back = DesktopWorkspaceStore::decode(&store.encoded()).unwrap();
        assert_eq!(back.workspaces[0].windows, ws.windows);
        // Only Edge's first window is open now, moved; notepad is closed.
        let now = vec![w(9, r"C:\Program Files\Microsoft\Edge\msedge.exe", "Microsoft Edge", 500.0, 500.0)];
        let plan = PetDesktopRunner::plan_apply(&ws, &now, &screens());
        assert_eq!(plan.open.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["메모장"]);
        assert_eq!(plan.moves, vec![(9, Rect::new(100.0, 100.0, 800.0, 600.0))]);
        assert_eq!(plan.unmatched, 2);
        // Saved on a display that is gone: pulled onto the first one.
        let one = vec![Rect::new(0.0, 0.0, 1920.0, 1080.0)];
        let now = vec![w(7, r"C:\Windows\System32\notepad.exe", "메모장", 0.0, 0.0)];
        let plan = PetDesktopRunner::plan_apply(&ws, &now, &one);
        let (_, f) = plan.moves[0];
        assert!(one[0].contains(f.cx(), f.cy()), "{f:?}");
    }

    #[test]
    fn exe_names() {
        assert_eq!(exe_name(r"C:\Windows\Notepad.EXE"), "notepad.exe");
        assert!(same_exe(r"C:\a\Chrome.exe", "chrome.exe"));
        assert!(!same_exe("", "chrome.exe"));
    }
}
