//! App Switch: which windows the switcher lists and in what order, how the selection moves, and when letting go of
//! the modifiers switches. Ported from `AppSwitchModule.swift` (`SwitchableWindow.list`, `select`, `checkHold`).
//! No Windows calls here: the shell hands in what it found and acts on what comes back.
use super::settings::AppHotKey;
use super::shortcut::*;

/// The switcher shows at most this many windows.
pub const MAX_WINDOWS: usize = 24;
/// Smaller windows are not real windows (tooltips, helper surfaces).
pub const MIN_WIDTH: i32 = 80;
pub const MIN_HEIGHT: i32 = 50;

/// One top-level window as the shell found it, in the order Windows lists them (front to back).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawWindow {
    pub id: isize,
    pub pid: u32,
    pub app: String,
    pub title: String,
    pub class: String,
    pub width: i32,
    pub height: i32,
    pub visible: bool,
    /// Hidden by the shell on purpose: another virtual desktop, or a suspended Store app.
    pub cloaked: bool,
    pub tool_window: bool,
    /// Has `WS_EX_APPWINDOW`: shown on the taskbar whatever else it says.
    pub app_window: bool,
    pub owned: bool,
    pub minimized: bool,
}

/// A window the switcher lists.
#[derive(Clone, Debug, PartialEq)]
pub struct Item { pub id: isize, pub pid: u32, pub app: String, pub title: String, pub minimized: bool }

/// The desktop, the taskbar and the like: windows that are never something to switch to.
fn is_shell_class(class: &str) -> bool {
    matches!(class, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "NotifyIconOverflowWindow" | "TopLevelWindowForOverflowXamlIsland"
        | "XamlExplorerHostIslandWindow" | "Windows.UI.Core.CoreWindow")
}

/// Whether Windows would show this window on the taskbar, which is what Alt+Tab lists too.
fn switchable(w: &RawWindow, own_pid: u32) -> bool {
    if w.pid == own_pid || !w.visible || w.cloaked || is_shell_class(&w.class) { return false; }
    // A tool window or an owned window (a dialog) belongs to its app's main window, unless it asks for its own button.
    if (w.tool_window || w.owned) && !w.app_window { return false; }
    // A minimized window has no size worth checking; its title is what identifies it.
    if !w.minimized && (w.width < MIN_WIDTH || w.height < MIN_HEIGHT) { return false; }
    // A window with no title at all is a helper surface the user never sees.
    !w.title.trim().is_empty()
}

/// The windows to list: on-screen ones front to back, then minimized ones, at most `MAX_WINDOWS`.
pub fn list(raw: &[RawWindow], own_pid: u32) -> Vec<Item> {
    let mut v = list_all(raw, own_pid);
    v.truncate(MAX_WINDOWS);
    v
}

/// Every switchable window, uncapped: what "Hide Other Windows" and the app shortcuts look through.
/// A window listed twice stays once; an empty app name falls back to the title.
pub fn list_all(raw: &[RawWindow], own_pid: u32) -> Vec<Item> {
    let mut seen = std::collections::BTreeSet::new();
    let mut shown = vec![];
    let mut minimized = vec![];
    for w in raw.iter().filter(|w| switchable(w, own_pid)) {
        if !seen.insert(w.id) { continue; }
        let app = if w.app.trim().is_empty() { w.title.trim().to_string() } else { w.app.trim().to_string() };
        let item = Item { id: w.id, pid: w.pid, app, title: w.title.trim().to_string(), minimized: w.minimized };
        if w.minimized { minimized.push(item) } else { shown.push(item) }
    }
    shown.extend(minimized);
    shown
}

/// Where the selection starts: the window behind the front one when going forward, the last one when going backward.
pub fn first_selection(forward: bool, len: usize) -> usize {
    if len == 0 { 0 } else if forward { 1.min(len - 1) } else { len - 1 }
}

/// Moves the selection by `step`, round the ends.
pub fn move_selection(current: usize, step: i32, len: usize) -> usize {
    if len == 0 { return 0; }
    (current as i64 + step as i64).rem_euclid(len as i64) as usize
}

/// The modifiers whose release switches. Shift is left out: it only means "backward" and is often let go first.
/// A shortcut with nothing but Shift would never end, so all its modifiers count then.
pub fn hold_mods(shortcut_mods: u32) -> u32 {
    let held = shortcut_mods & ALL_MODS & !SHIFT;
    if held == 0 { shortcut_mods & ALL_MODS } else { held }
}

/// Letting go of every modifier the shortcut needs switches to the selected window.
pub fn released(held_now: u32, shortcut_mods: u32) -> bool { held_now & hold_mods(shortcut_mods) == 0 }

/// The modifier combinations Esc has to be caught with while the switcher is open: the ones still held, with Shift too, and none.
pub fn cancel_mods(shortcut_mods: u32) -> [u32; 3] {
    let m = shortcut_mods & ALL_MODS;
    [m, m | SHIFT, 0]
}

/// "Hide Other Apps": every window on screen that is not the front app's.
pub fn others_to_hide(items: &[Item], front_pid: u32) -> Vec<isize> {
    items.iter().filter(|i| !i.minimized && i.pid != front_pid).map(|i| i.id).collect()
}

// MARK: app shortcuts

/// The id a program's shortcut goes by in the hotkey table and in clash messages.
pub fn app_id(path: &str) -> String { format!("app:{}", path_key(path)) }
/// Paths compare without regard to case or slash direction, as Windows does.
pub fn path_key(path: &str) -> String { path.trim().replace('/', "\\").to_lowercase() }
pub fn same_path(a: &str, b: &str) -> bool { !a.trim().is_empty() && path_key(a) == path_key(b) }

/// "Code" for `C:\Apps\code.exe`: the file name without `.exe`, first letter up.
pub fn app_name(path: &str) -> String {
    let file = path.trim().rsplit(['\\', '/']).next().unwrap_or("");
    let stem = match file.rsplit_once('.') { Some((s, e)) if e.eq_ignore_ascii_case("exe") && !s.is_empty() => s, _ => file };
    let mut c = stem.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Adds a program to the list; false when it is not an .exe path or is there already.
pub fn add_app(list: &mut Vec<AppHotKey>, path: &str) -> bool {
    let path = path.trim();
    if !path.to_lowercase().ends_with(".exe") || list.iter().any(|h| same_path(&h.path, path)) { return false; }
    list.push(AppHotKey { path: path.into(), name: app_name(path), shortcut: None });
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppAction { Launch, Raise, Hide }

/// What an app's shortcut does: start it when it has no window, hide it when it is in front already (and the
/// option says so), otherwise bring its window forward.
pub fn app_action(has_window: bool, in_front: bool, hide_on_repeat: bool) -> AppAction {
    if !has_window { AppAction::Launch } else if in_front && hide_on_repeat { AppAction::Hide } else { AppAction::Raise }
}

/// The default shortcuts. Windows keeps Alt+Tab (and Ctrl+Alt+Tab) for its own switcher, so these use the
/// key above Tab: Ctrl+Alt+` forward, Ctrl+Alt+Shift+` backward, holding Ctrl+Alt.
pub const NEXT_DEFAULT: Shortcut = Shortcut::new(0xC0, CTRL | ALT);
pub const PREVIOUS_DEFAULT: Shortcut = Shortcut::new(0xC0, CTRL | ALT | SHIFT);

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: isize, pid: u32, app: &str, title: &str) -> RawWindow {
        RawWindow { id, pid, app: app.into(), title: title.into(), class: "Cls".into(), width: 800, height: 600, visible: true, ..Default::default() }
    }

    #[test] fn keeps_front_to_back_order_and_puts_minimized_last() {
        let mut min = w(2, 20, "Mail", "Inbox");
        min.minimized = true;
        let items = list(&[w(1, 10, "Code", "main.rs"), min, w(3, 30, "Web", "Docs")], 99);
        assert_eq!(items.iter().map(|i| i.id).collect::<Vec<_>>(), vec![1, 3, 2]);
        assert!(items[2].minimized && !items[0].minimized);
    }
    #[test] fn leaves_out_what_is_not_a_window_to_switch_to() {
        let own = w(1, 99, "NachoCombo", "Nacho Combo");
        let mut hidden = w(2, 10, "A", "hidden"); hidden.visible = false;
        let mut cloaked = w(3, 10, "A", "other desktop"); cloaked.cloaked = true;
        let mut tool = w(4, 10, "A", "palette"); tool.tool_window = true;
        let mut owned = w(5, 10, "A", "dialog"); owned.owned = true;
        let mut tiny = w(6, 10, "A", "tip"); tiny.width = 79;
        let mut short = w(7, 10, "A", "strip"); short.height = 49;
        let mut untitled = w(8, 10, "A", "  "); untitled.title = "  ".into();
        let mut shell = w(9, 10, "Explorer", "Program Manager"); shell.class = "Progman".into();
        let mut tray = w(10, 10, "Explorer", "x"); tray.class = "Shell_TrayWnd".into();
        let keep = w(11, 10, "A", "real");
        assert_eq!(list(&[own, hidden, cloaked, tool, owned, tiny, short, untitled, shell, tray, keep], 99).len(), 1);
    }
    #[test] fn a_tool_or_owned_window_that_asks_for_a_taskbar_button_stays() {
        let mut a = w(1, 10, "A", "tool"); a.tool_window = true; a.app_window = true;
        let mut b = w(2, 10, "A", "owned"); b.owned = true; b.app_window = true;
        assert_eq!(list(&[a, b], 99).len(), 2);
    }
    #[test] fn a_minimized_window_is_listed_whatever_size_windows_reports() {
        let mut m = w(1, 10, "A", "icon"); m.minimized = true; m.width = 160; m.height = 28;
        assert_eq!(list(&[m], 99).len(), 1);
    }
    #[test] fn lists_a_window_once_and_names_it_by_title_without_an_app() {
        let items = list(&[w(1, 10, "", "Notes"), w(1, 10, "", "Notes")], 99);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].app, "Notes");
    }
    #[test] fn caps_the_list() {
        let raw: Vec<RawWindow> = (0..40).map(|i| w(i + 1, 10, "A", "t")).collect();
        let items = list(&raw, 99);
        assert_eq!(items.len(), MAX_WINDOWS);
        assert_eq!(items[0].id, 1);
        // A minimized window never pushes an on-screen one out.
        let raw: Vec<RawWindow> = (0..30).map(|i| { let mut m = w(i + 1, 10, "A", "t"); m.minimized = i < 6; m }).collect();
        assert!(list(&raw, 99).iter().take(24).all(|i| !i.minimized));
    }
    #[test] fn selection_starts_behind_the_front_window_going_forward_and_at_the_end_going_back() {
        assert_eq!(first_selection(true, 5), 1);
        assert_eq!(first_selection(false, 5), 4);
        assert_eq!(first_selection(true, 1), 0);
        assert_eq!(first_selection(false, 1), 0);
        assert_eq!(first_selection(true, 0), 0);
    }
    #[test] fn selection_wraps_both_ways() {
        assert_eq!(move_selection(4, 1, 5), 0);
        assert_eq!(move_selection(0, -1, 5), 4);
        assert_eq!(move_selection(2, 1, 5), 3);
        assert_eq!(move_selection(0, -1, 1), 0);
        assert_eq!(move_selection(3, 1, 0), 0);
        assert_eq!(move_selection(1, -7, 5), 4);
    }
    #[test] fn letting_go_of_the_hold_keys_switches_and_shift_does_not_count() {
        let m = CTRL | ALT | SHIFT;
        assert!(!released(CTRL | ALT | SHIFT, m));
        assert!(!released(CTRL, m));
        assert!(!released(ALT | SHIFT, m));
        assert!(released(SHIFT, m));
        assert!(released(0, m));
        assert!(released(0, CTRL | ALT));
        // Only Shift in the shortcut: it is what has to be held.
        assert!(!released(SHIFT, SHIFT));
        assert!(released(0, SHIFT));
    }
    #[test] fn esc_is_caught_with_the_held_modifiers_shift_and_none() {
        assert_eq!(cancel_mods(CTRL | ALT), [CTRL | ALT, CTRL | ALT | SHIFT, 0]);
        assert_eq!(cancel_mods(CTRL | ALT | SHIFT), [CTRL | ALT | SHIFT, CTRL | ALT | SHIFT, 0]);
    }
    #[test] fn hide_others_spares_the_front_app_and_minimized_windows() {
        let items = vec![
            Item { id: 1, pid: 10, app: "A".into(), title: "a".into(), minimized: false },
            Item { id: 2, pid: 10, app: "A".into(), title: "a2".into(), minimized: false },
            Item { id: 3, pid: 20, app: "B".into(), title: "b".into(), minimized: false },
            Item { id: 4, pid: 30, app: "C".into(), title: "c".into(), minimized: true },
        ];
        assert_eq!(others_to_hide(&items, 10), vec![3]);
        assert!(others_to_hide(&[], 10).is_empty());
    }
    #[test] fn default_shortcuts_are_valid_and_leave_windows_own_switcher_alone() {
        assert_eq!(validate(NEXT_DEFAULT, &[]), None);
        assert_eq!(validate(PREVIOUS_DEFAULT, &[]), None);
        assert_ne!(NEXT_DEFAULT.key, 0x09);
        assert_eq!(NEXT_DEFAULT.mods & SHIFT, 0);
        assert_eq!(PREVIOUS_DEFAULT.mods, NEXT_DEFAULT.mods | SHIFT);
        // Alt+Tab itself is refused as a shortcut.
        assert!(matches!(validate(Shortcut::new(0x09, ALT), &[]), Some(Issue::NeedsMoreModifiers) | Some(Issue::Reserved(_))));
    }
    #[test] fn off_by_default_and_an_old_settings_file_still_loads() {
        use crate::core::settings::*;
        assert!(!AppSettings::default().is_on(ModuleId::AppSwitch));
        let old = r#"{"enabled":{"windowSnap":true,"clipboard":true},"pinnedTools":["windowSnap"]}"#;
        let s: AppSettings = serde_json::from_str(old).unwrap();
        assert!(s.is_on(ModuleId::WindowSnap) && s.is_on(ModuleId::Clipboard) && !s.is_on(ModuleId::AppSwitch));
        assert_eq!(ModuleId::from_key("appswitch"), Some(ModuleId::AppSwitch));
        let on: AppSettings = serde_json::from_str(r#"{"enabled":{"appSwitch":true}}"#).unwrap();
        assert!(on.is_on(ModuleId::AppSwitch));
    }
    #[test] fn the_actions_are_listed_with_the_defaults() {
        use crate::core::catalog::actions;
        let a = actions();
        let get = |id: &str| a.iter().find(|x| x.id == id).unwrap();
        assert_eq!(get("appswitch.next").default, Some(NEXT_DEFAULT));
        assert_eq!(get("appswitch.previous").default, Some(PREVIOUS_DEFAULT));
        assert_eq!(get("appswitch.hideOthers").default, None);
    }
    #[test] fn hide_others_reaches_windows_beyond_the_switcher_cap() {
        let raw: Vec<RawWindow> = (0..40).map(|i| w(i + 1, 20 + i as u32, "A", "t")).collect();
        assert_eq!(list(&raw, 99).len(), MAX_WINDOWS);
        let all = list_all(&raw, 99);
        assert_eq!(all.len(), 40);
        assert_eq!(others_to_hide(&all, 20).len(), 39);
    }
    #[test] fn app_names_and_paths() {
        assert_eq!(app_name(r"C:\Apps\code.exe"), "Code");
        assert_eq!(app_name("C:/Apps/Zoom.EXE"), "Zoom");
        assert_eq!(app_name("notes"), "Notes");
        assert!(same_path(r"C:\Apps\Code.exe", "c:/apps/code.exe"));
        assert!(!same_path("", ""));
        assert_eq!(app_id(r"C:\A\b.exe"), app_id("c:/a/B.exe"));
    }
    #[test] fn only_exe_paths_are_added_and_once() {
        let mut l = vec![];
        assert!(add_app(&mut l, r"C:\Apps\code.exe"));
        assert!(!add_app(&mut l, "c:/apps/CODE.exe"));
        assert!(!add_app(&mut l, r"C:\Apps\readme.txt"));
        assert!(!add_app(&mut l, "  "));
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].name, "Code");
        assert_eq!(l[0].shortcut, None);
    }
    #[test] fn app_shortcut_decides_launch_raise_or_hide() {
        assert_eq!(app_action(false, false, true), AppAction::Launch);
        assert_eq!(app_action(false, true, true), AppAction::Launch);
        assert_eq!(app_action(true, false, true), AppAction::Raise);
        assert_eq!(app_action(true, true, true), AppAction::Hide);
        assert_eq!(app_action(true, true, false), AppAction::Raise);
    }
    #[test] fn app_options_default_and_old_files_load() {
        use crate::core::settings::*;
        let d = Options::default();
        assert!(d.app_shortcuts.is_empty() && d.app_hide_on_repeat);
        let o: Options = serde_json::from_str(r#"{"snapGap":4}"#).unwrap();
        assert!(o.app_shortcuts.is_empty() && o.app_hide_on_repeat && o.snap_gap == 4);
        let mut s = AppSettings::default();
        s.options.app_shortcuts = vec![AppHotKey { path: r"C:\a.exe".into(), name: "A".into(), shortcut: Some(Shortcut::new(0x41, CTRL | ALT)) }, AppHotKey { path: r"C:\b.exe".into(), name: "B".into(), shortcut: None }];
        s.options.app_hide_on_repeat = false;
        let back: AppSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.options, s.options);
        // A broken entry keeps the rest of the file.
        let part: Options = serde_json::from_str(r#"{"appShortcuts":[{"path":"C:\\a.exe"}],"appHideOnRepeat":false}"#).unwrap();
        assert_eq!(part.app_shortcuts[0].name, "");
        assert!(!part.app_hide_on_repeat);
    }
    #[test] fn an_app_shortcut_clashes_with_actions_tools_and_other_apps_both_ways() {
        use crate::core::settings::*;
        let mut s = AppSettings::default();
        let me = r"C:\a.exe";
        s.options.app_shortcuts = vec![
            AppHotKey { path: me.into(), name: "A".into(), shortcut: None },
            AppHotKey { path: r"C:\b.exe".into(), name: "B".into(), shortcut: Some(Shortcut::new(0x42, CTRL | ALT)) }];
        // An app shortcut against another app's, an action's and a tool's.
        assert!(matches!(validate(Shortcut::new(0x42, CTRL | ALT), &s.taken_shortcuts(&app_id(me))), Some(Issue::Conflict(n)) if n.contains("B")));
        assert!(matches!(validate(Shortcut::new(0x56, CTRL | ALT), &s.taken_shortcuts(&app_id(me))), Some(Issue::Conflict(_)))); // Clipboard's Ctrl+Alt+V
        s.tool_shortcuts.insert(ModuleId::KeepAwake, Shortcut::new(0x4B, CTRL | ALT | WIN));
        assert!(matches!(validate(Shortcut::new(0x4B, CTRL | ALT | WIN), &s.taken_shortcuts(&app_id(me))), Some(Issue::Conflict(_))));
        // Rules of any shortcut: two modifiers, nothing Windows owns.
        assert_eq!(validate(Shortcut::new(0x41, CTRL), &s.taken_shortcuts(&app_id(me))), Some(Issue::NeedsMoreModifiers));
        assert!(matches!(validate(Shortcut::new(0x4C, WIN), &s.taken_shortcuts(&app_id(me))), Some(Issue::NeedsMoreModifiers) | Some(Issue::Reserved(_))));
        assert!(matches!(validate(Shortcut::new(0x4C, WIN | CTRL), &s.taken_shortcuts(&app_id(me))), None | Some(Issue::Reserved(_))));
        // Its own current shortcut is not a clash with itself.
        assert_eq!(validate(Shortcut::new(0x42, CTRL | ALT), &s.taken_shortcuts(&app_id(r"C:/B.exe"))), None);
        // And an action's new shortcut clashes with an app's.
        assert!(matches!(validate(Shortcut::new(0x42, CTRL | ALT), &s.taken_shortcuts("snap.left")), Some(Issue::Conflict(n)) if n.contains("App Switch")));
    }
}
