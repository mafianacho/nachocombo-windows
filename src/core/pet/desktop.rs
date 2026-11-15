//! Nacho's desktop actions beyond one window: apps, several windows at once, system controls and saved workspaces.
//! Port of `PetDesktop.swift` (plus `SavedWindow` of the macOS `GadgetLogic.swift`, which this needs). Everything here
//! is plain data and rules; the Win32 shell does the window and process work.
//!
//! Windows changes: an app's id is its exe name (`notepad.exe`) where macOS uses a bundle id (the field keeps the
//! macOS name `bundle_id` so the two ports read alike); `.exe` is folded away like `.app`; the protected apps are the
//! Windows shell processes; display numbering and saved frames use top-left screen coordinates.
use super::commands::{PetCommandOutcome, PetCommands, PetGadget};
use crate::core::layout::{Rect, SnapAction};
use crate::core::settings::ModuleId;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

// Apps

/// One app Nacho may name: running now and/or installed (Start menu).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetAppEntry {
    /// Windows: the exe name (`msedge.exe`), compared without case. macOS: the bundle id.
    pub bundle_id: String,
    /// Names the user may say: the shortcut's (localized, e.g. "계산기"), FileDescription, the exe name.
    pub names: Vec<String>,
    pub running: bool,
}

impl PetAppEntry {
    pub fn new(bundle_id: &str, names: &[&str], running: bool) -> Self {
        Self { bundle_id: bundle_id.into(), names: names.iter().map(|s| s.to_string()).collect(), running }
    }
    pub fn display_name(&self) -> &str { self.names.first().map_or(&self.bundle_id, |n| n) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PetAppMatch {
    Found(PetAppEntry),
    Ambiguous(Vec<PetAppEntry>),
    None { suggestions: Vec<String> },
}

pub struct PetAppMatcher;

impl PetAppMatcher {
    /// "Google Chrome.app" / "chrome.exe" → "googlechrome" / "chrome": case, spaces, the `.app` / `.exe` suffix and
    /// punctuation do not matter.
    pub fn fold(s: &str) -> String {
        let mut t = s.trim().to_lowercase();
        for suffix in [".app", ".exe"] { if let Some(x) = t.strip_suffix(suffix) { t = x.to_string(); break; } }
        t.chars().filter(|c| c.is_alphanumeric()).collect()
    }

    /// Exact match on a name or the id first (running apps win a tie), then a single partial match (2+ characters).
    /// Several partial matches are ambiguous; nothing gives up to five suggestions.
    pub fn match_app(query: &str, entries: &[PetAppEntry]) -> PetAppMatch {
        let q = Self::fold(query);
        let raw_q = query.trim().to_lowercase();
        if q.is_empty() { return PetAppMatch::None { suggestions: vec![] }; }
        let unique = Self::dedupe(entries);
        let pick = |list: Vec<&PetAppEntry>, cap: usize| -> Option<PetAppMatch> {
            match list.len() {
                0 => None,
                1 => Some(PetAppMatch::Found(list[0].clone())),
                _ => {
                    let running: Vec<&&PetAppEntry> = list.iter().filter(|e| e.running).collect();
                    if running.len() == 1 { return Some(PetAppMatch::Found((*running[0]).clone())); }
                    Some(PetAppMatch::Ambiguous(list.into_iter().take(cap).cloned().collect()))
                }
            }
        };
        let exact: Vec<&PetAppEntry> = unique.iter().filter(|e| e.bundle_id.to_lowercase() == raw_q || e.names.iter().any(|n| Self::fold(n) == q)).collect();
        if let Some(m) = pick(exact, usize::MAX) { return m; }
        if q.chars().count() < 2 { return PetAppMatch::None { suggestions: vec![] }; }
        let partial: Vec<&PetAppEntry> = unique.iter().filter(|e| e.names.iter().any(|n| Self::fold(n).contains(&q))).collect();
        if let Some(m) = pick(partial, 8) { return m; }
        // A word in common (e.g. "chrome canary" vs "Google Chrome Canary") still helps the model ask back.
        let words: Vec<String> = query.to_lowercase().split(' ').map(Self::fold).filter(|w| w.chars().count() >= 2).collect();
        let near = unique.iter().filter(|e| words.iter().any(|w| e.names.iter().any(|n| Self::fold(n).contains(w.as_str()))));
        PetAppMatch::None { suggestions: near.take(5).map(|e| e.display_name().to_string()).collect() }
    }

    /// One entry per id (without case): names merged, running if any copy runs.
    pub fn dedupe(entries: &[PetAppEntry]) -> Vec<PetAppEntry> {
        let mut out: Vec<PetAppEntry> = Vec::new();
        for e in entries {
            if let Some(existing) = out.iter_mut().find(|x| x.bundle_id.to_lowercase() == e.bundle_id.to_lowercase()) {
                for n in &e.names { if !existing.names.contains(n) { existing.names.push(n.clone()); } }
                existing.running |= e.running;
            } else {
                out.push(e.clone());
            }
        }
        out
    }

    /// Apps Nacho never hides or quits: this app and the processes that keep the Windows desktop working (macOS: this
    /// app, Finder, Dock, loginwindow, SystemUIServer, Control Center, Notification Center, WindowManager).
    pub const PROTECTED_BUNDLE_IDS: [&'static str; 9] = [
        "nachocombo.exe", "explorer.exe", "dwm.exe", "winlogon.exe", "csrss.exe", "sihost.exe",
        "shellexperiencehost.exe", "startmenuexperiencehost.exe", "searchhost.exe",
    ];

    pub fn is_protected(bundle_id: &str) -> bool { Self::PROTECTED_BUNDLE_IDS.iter().any(|p| p.eq_ignore_ascii_case(bundle_id)) }
}

// Several windows

/// One window Nacho places in a multi-window layout. `display` is 1-based in left-to-right order (as
/// `combo_list_windows` lists them); `window` is 1-based front to back among the app's windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetWindowPlacement {
    pub app: String,
    pub position: SnapAction,
    pub display: Option<i64>,
    pub window: i64,
}

pub struct PetDisplays;

impl PetDisplays {
    /// Screens in the order Nacho numbers them: left to right, then top to bottom (same as next / previous display).
    /// Top-left screen coordinates (macOS compares the Cocoa `maxY`, bottom-left origin).
    pub fn ordered(screens: &[Rect]) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..screens.len()).collect();
        idx.sort_by(|&a, &b| {
            let (sa, sb) = (screens[a], screens[b]);
            if sa.x != sb.x { sa.x.total_cmp(&sb.x) } else { sa.y.total_cmp(&sb.y) }
        });
        idx
    }

    /// Index into `screens` for Nacho's 1-based display number, or None when there is no such display.
    pub fn index_for_number(number: i64, screens: &[Rect]) -> Option<usize> {
        let order = Self::ordered(screens);
        if number < 1 || number as usize > order.len() { return None; }
        Some(order[number as usize - 1])
    }

    /// Nacho's 1-based number for a screen index.
    pub fn number_for_index(index: usize, screens: &[Rect]) -> Option<i64> {
        Self::ordered(screens).iter().position(|&i| i == index).map(|p| p as i64 + 1)
    }
}

// System controls

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PetSystemChange {
    /// 0...100 each; None = leave as it is.
    pub volume: Option<i64>,
    pub brightness: Option<i64>,
    pub do_not_disturb: Option<bool>,
}

impl PetSystemChange {
    pub fn is_empty(&self) -> bool { self.volume.is_none() && self.brightness.is_none() && self.do_not_disturb.is_none() }
}

/// Result of one part of a system change: the model hears each part (a PC may set the volume but not the brightness).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetPartResult {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

impl PetPartResult {
    pub fn new(name: &str, ok: bool, message: &str) -> Self { Self { name: name.into(), ok, message: message.into() } }

    pub fn summary(parts: &[PetPartResult]) -> PetCommandOutcome {
        let ok = !parts.is_empty() && parts.iter().all(|p| p.ok);
        let text: Vec<String> = parts.iter().map(|p| format!("{}: {} — {}", p.name, if p.ok { "done" } else { "not done" }, p.message)).collect();
        let mut o = PetCommandOutcome::new(ok, text.join(" "));
        let results: Vec<Value> = parts.iter().map(|p| json!({"name": p.name, "ok": p.ok, "message": p.message})).collect();
        o.details = json!({"results": results}).as_object().cloned();
        o
    }
}

// Workspaces

/// One saved window (macOS `SavedWindow` of the Window Snap layouts). Windows: top-left screen coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWindow {
    /// Windows: the exe name.
    #[serde(rename = "bundleID")]
    pub bundle_id: String,
    #[serde(rename = "appName")]
    pub app_name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Which of the app's windows it was, front to back.
    pub order: i64,
}

impl SavedWindow {
    pub fn new(bundle_id: &str, app_name: &str, frame: Rect, order: i64) -> Self {
        Self { bundle_id: bundle_id.into(), app_name: app_name.into(), x: frame.x, y: frame.y, width: frame.w, height: frame.h, order }
    }
    pub fn frame(&self) -> Rect { Rect::new(self.x, self.y, self.width, self.height) }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopWorkspaceApp {
    /// Windows: the exe name.
    #[serde(rename = "bundleID")]
    pub bundle_id: String,
    pub name: String,
}

/// A named desktop: which apps run, where their windows are, which gadgets are on. Saved by Nacho on request and put
/// back later; putting it back opens missing apps but never quits anything.
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopWorkspace {
    pub name: String,
    /// Unix seconds.
    pub saved_at: f64,
    /// Signature of the displays it was saved on (frames on a missing display are pulled onto one that is there).
    pub displays: String,
    pub apps: Vec<DesktopWorkspaceApp>,
    pub windows: Vec<SavedWindow>,
    /// Gadget raw value → on. Only gadgets Nacho may switch are applied.
    pub gadgets: BTreeMap<String, bool>,
}

/// `2027-01-15T08:00:00Z` (whole seconds, UTC).
pub fn iso8601(t: f64) -> String {
    let s = t.floor() as i64;
    let (days, rem) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    let (y, m, d) = crate::core::pet::clock::civil(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

impl DesktopWorkspace {
    /// Lenient: a field this version does not know is ignored, a missing or broken field is empty (files written by
    /// later versions). Only `name` is required.
    pub fn from_json(v: &Value) -> Option<DesktopWorkspace> {
        let o = v.as_object()?;
        fn list<T: serde::de::DeserializeOwned + Default>(o: &serde_json::Map<String, Value>, k: &str) -> T {
            o.get(k).cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
        }
        Some(DesktopWorkspace {
            name: o.get("name")?.as_str()?.to_string(),
            saved_at: o.get("savedAt").and_then(Value::as_str).and_then(crate::core::pet::clock::parse_rfc3339).unwrap_or(0.0),
            displays: o.get("displays").and_then(Value::as_str).unwrap_or("").to_string(),
            apps: list(o, "apps"),
            windows: list(o, "windows"),
            gadgets: list(o, "gadgets"),
        })
    }

    pub fn to_json(&self) -> Value {
        json!({"name": self.name, "savedAt": iso8601(self.saved_at), "displays": self.displays, "apps": self.apps,
               "windows": self.windows, "gadgets": self.gadgets})
    }

    /// Gadgets with a known id that Nacho may switch (a renamed or removed gadget in an old file is skipped).
    pub fn gadget_switches(&self) -> Vec<(ModuleId, bool)> {
        let toggleable = PetCommands::toggleable();
        let mut v: Vec<(String, ModuleId, bool)> = self.gadgets.iter().filter_map(|(k, on)| match PetGadget::from_raw(k)? {
            PetGadget::Module(m) if toggleable.contains(&m) => Some((k.clone(), m, *on)),
            _ => None,
        }).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v.into_iter().map(|(_, m, on)| (m, on)).collect()
    }

    fn gadgets_on(&self) -> usize { self.gadgets.values().filter(|on| **on).count() }

    pub fn summary(&self) -> String {
        let (a, w, g) = (self.apps.len(), self.windows.len(), self.gadgets_on());
        let s = |n: usize| if n == 1 { "" } else { "s" };
        format!("{a} app{}, {w} window{}, {g} gadget{} on", s(a), s(w), s(g))
    }

    pub fn korean_summary(&self) -> String {
        format!("앱 {}개 · 창 {}개 · 가젯 {}개 켜짐", self.apps.len(), self.windows.len(), self.gadgets_on())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceNameProblem { Empty, TooLong, BadCharacters }

impl WorkspaceNameProblem {
    pub fn message(&self) -> String {
        match self {
            Self::Empty => "name must not be empty".into(),
            Self::TooLong => format!("name must be at most {} characters", DesktopWorkspaceStore::MAX_NAME_LENGTH),
            Self::BadCharacters => "name must be printable text on one line".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveResult { Added, Replaced, Full }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopWorkspaceStore {
    pub workspaces: Vec<DesktopWorkspace>,
}

impl DesktopWorkspaceStore {
    pub const MAX_WORKSPACES: usize = 20;
    pub const MAX_NAME_LENGTH: usize = 40;
    pub const FILE_VERSION: i64 = 1;

    /// Trimmed, 1...40 characters, no control characters or line breaks.
    pub fn valid_name(raw: &str) -> Result<String, WorkspaceNameProblem> {
        let t = raw.trim();
        if t.is_empty() { return Err(WorkspaceNameProblem::Empty); }
        if t.chars().count() > Self::MAX_NAME_LENGTH { return Err(WorkspaceNameProblem::TooLong); }
        let bad = |c: char| { let v = c as u32; v < 0x20 || v == 0x7f || (0x80..=0x9f).contains(&v) || v == 0x2028 || v == 0x2029 };
        if t.chars().any(bad) { return Err(WorkspaceNameProblem::BadCharacters); }
        Ok(t.to_string())
    }

    /// Names compare without case and spacing: "업무 모드" and "업무모드" are the same workspace.
    pub fn key(name: &str) -> String { name.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect() }

    pub fn workspace(&self, named: &str) -> Option<&DesktopWorkspace> {
        let k = Self::key(named);
        self.workspaces.iter().find(|w| Self::key(&w.name) == k)
    }

    /// Same name replaces (newest first); a new name when the list is full is refused, never evicting one silently.
    pub fn save(&mut self, workspace: DesktopWorkspace) -> SaveResult {
        let k = Self::key(&workspace.name);
        if let Some(i) = self.workspaces.iter().position(|w| Self::key(&w.name) == k) {
            self.workspaces.remove(i);
            self.workspaces.insert(0, workspace);
            return SaveResult::Replaced;
        }
        if self.workspaces.len() >= Self::MAX_WORKSPACES { return SaveResult::Full; }
        self.workspaces.insert(0, workspace);
        SaveResult::Added
    }

    pub fn remove(&mut self, named: &str) -> bool {
        let k = Self::key(named);
        let before = self.workspaces.len();
        self.workspaces.retain(|w| Self::key(&w.name) != k);
        self.workspaces.len() != before
    }

    /// Entries that fail to decode (hand-edited, truncated) are dropped one by one, not the whole file. None when the
    /// file is not a JSON object with a `workspaces` list.
    pub fn decode(data: &[u8]) -> Option<DesktopWorkspaceStore> {
        let v: Value = serde_json::from_slice(data).ok()?;
        let list = v.as_object()?.get("workspaces")?.as_array()?;
        Some(DesktopWorkspaceStore { workspaces: list.iter().filter_map(DesktopWorkspace::from_json).collect() })
    }

    /// Pretty JSON with sorted keys.
    pub fn encoded(&self) -> Vec<u8> {
        let v = json!({"version": Self::FILE_VERSION, "workspaces": self.workspaces.iter().map(DesktopWorkspace::to_json).collect::<Vec<_>>()});
        serde_json::to_vec_pretty(&v).unwrap_or_default()
    }

    pub fn load(path: &Path) -> DesktopWorkspaceStore {
        std::fs::read(path).ok().and_then(|d| Self::decode(&d)).unwrap_or_default()
    }

    /// Writes next to the file and renames it over (never a half-written file).
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, self.encoded())?;
        std::fs::rename(&tmp, path)
    }
}

/// How the parts of applying a workspace went, in one message for the model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceApplyReport {
    pub name: String,
    pub opened_apps: Vec<String>,
    pub missing_apps: Vec<String>,
    pub failed_apps: Vec<String>,
    pub placed_windows: usize,
    pub saved_windows: usize,
    /// Windows were skipped because Nacho Combo may not move them (Windows: they belong to apps running as
    /// administrator; macOS: no Accessibility permission).
    pub windows_need_permission: bool,
    pub gadgets_changed: Vec<String>,
    pub gadgets_refused: Vec<String>,
}

impl WorkspaceApplyReport {
    pub fn new(name: &str) -> Self { Self { name: name.into(), ..Default::default() } }

    pub fn ok(&self) -> bool {
        self.missing_apps.is_empty() && self.failed_apps.is_empty() && !self.windows_need_permission && self.gadgets_refused.is_empty()
    }

    pub fn outcome(&self) -> PetCommandOutcome {
        let mut parts = vec![format!("Workspace \"{}\":", self.name)];
        parts.push(if self.opened_apps.is_empty() { "no app had to be opened (apps already open were left running).".into() }
                   else { format!("opened {} (apps already open were left running).", self.opened_apps.join(", ")) });
        // Windows wording: "this PC" (macOS: "this Mac").
        if !self.missing_apps.is_empty() { parts.push(format!("Not installed on this PC: {}.", self.missing_apps.join(", "))); }
        if !self.failed_apps.is_empty() { parts.push(format!("Could not open: {}.", self.failed_apps.join(", "))); }
        if self.windows_need_permission {
            parts.push(format!("Windows were not moved: {}", PetPermissionHelp::ELEVATED_WINDOWS));
        } else {
            parts.push(format!("Placed {} of {} saved windows.", self.placed_windows, self.saved_windows));
        }
        if !self.gadgets_changed.is_empty() { parts.push(format!("Gadgets: {}.", self.gadgets_changed.join(", "))); }
        if !self.gadgets_refused.is_empty() { parts.push(format!("Not switched: {}.", self.gadgets_refused.join("; "))); }
        let mut o = PetCommandOutcome::new(self.ok(), parts.join(" "));
        o.details = json!({"opened": self.opened_apps, "missing": self.missing_apps, "placed": self.placed_windows, "saved_windows": self.saved_windows})
            .as_object().cloned();
        o
    }
}

/// How to get past what blocks an action, said to the model so it can tell the user.
pub struct PetPermissionHelp;

impl PetPermissionHelp {
    /// Windows: a normal app may not move the windows of an app running as administrator (UIPI).
    pub const ELEVATED_WINDOWS: &'static str = "some windows belong to apps running as administrator, and Windows does not let \
        Nacho Combo move them. Close and reopen those apps normally, or run Nacho Combo as administrator. \
        (관리자 권한으로 실행 중인 앱의 창은 옮길 수 없어요. 그 앱을 일반 권한으로 다시 열어 주세요.)";
    /// macOS only (kept for reference by the shared wording): the Accessibility permission.
    pub const ACCESSIBILITY: &'static str = "Nacho Combo has no Accessibility permission. To allow it: open System Settings \
        (System Preferences on macOS 12) > Privacy & Security > Accessibility and switch on Nacho Combo, \
        or switch on the Window Snap gadget in Nacho Combo, which asks for it. \
        (손쉬운 사용 권한: 시스템 설정 > 개인정보 보호 및 보안 > 손쉬운 사용에서 Nacho Combo를 켜 주세요.)";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps() -> Vec<PetAppEntry> {
        vec![
            PetAppEntry::new("calc.exe", &["계산기", "Calculator"], false),
            PetAppEntry::new("msedge.exe", &["Edge"], true),
            PetAppEntry::new("chrome.exe", &["Google Chrome"], false),
            PetAppEntry::new("chrome_canary.exe", &["Google Chrome Canary"], false),
            PetAppEntry::new("winword.exe", &["Microsoft Word"], false),
            PetAppEntry::new("excel.exe", &["Microsoft Excel"], false),
            PetAppEntry::new("notepad.exe", &["메모장", "Notepad"], true),
            // The same app found twice (running + in the Start menu) is one app.
            PetAppEntry::new("NOTEPAD.EXE", &["Notepad"], false),
        ]
    }

    fn found(q: &str) -> Option<String> {
        match PetAppMatcher::match_app(q, &apps()) { PetAppMatch::Found(e) => Some(e.bundle_id), _ => None }
    }

    #[test]
    fn matches_korean_english_and_bundle_names() {
        assert_eq!(found("계산기").as_deref(), Some("calc.exe"));
        assert_eq!(found("calculator").as_deref(), Some("calc.exe"));
        assert_eq!(found("Calculator.exe").as_deref(), Some("calc.exe"));
        assert_eq!(found(" EDGE ").as_deref(), Some("msedge.exe"));
        assert_eq!(found("MSEdge.exe").as_deref(), Some("msedge.exe"), "the exe name");
        assert_eq!(found("메모장").as_deref(), Some("notepad.exe"));
        assert_eq!(found("google chrome").as_deref(), Some("chrome.exe")); // exact beats the Canary partial match
        assert_eq!(found("googlechrome").as_deref(), Some("chrome.exe"));
        assert_eq!(found("word").as_deref(), Some("winword.exe")); // single partial match
    }

    #[test]
    fn ambiguous_and_unknown_names() {
        let PetAppMatch::Ambiguous(list) = PetAppMatcher::match_app("Microsoft", &apps()) else { panic!("not ambiguous") };
        let mut ids: Vec<String> = list.into_iter().map(|e| e.bundle_id).collect();
        ids.sort();
        assert_eq!(ids, vec!["excel.exe", "winword.exe"]);
        assert!(matches!(PetAppMatcher::match_app("chrome", &apps()), PetAppMatch::Ambiguous(_)), "chrome must be ambiguous");
        assert_eq!(PetAppMatcher::match_app("Photoshop", &apps()), PetAppMatch::None { suggestions: vec![] });
        assert_eq!(PetAppMatcher::match_app("", &apps()), PetAppMatch::None { suggestions: vec![] });
        assert_eq!(PetAppMatcher::match_app("a", &apps()), PetAppMatch::None { suggestions: vec![] }); // one letter never partially matches
        let PetAppMatch::None { mut suggestions } = PetAppMatcher::match_app("Microsoft Paint", &apps()) else { panic!() };
        suggestions.sort();
        assert_eq!(suggestions, vec!["Microsoft Excel", "Microsoft Word"]);
    }

    #[test]
    fn duplicates_merge_and_running_wins() {
        let merged = PetAppMatcher::dedupe(&apps());
        assert_eq!(merged.iter().filter(|e| e.bundle_id.to_lowercase() == "notepad.exe").count(), 1);
        assert!(merged.iter().find(|e| e.bundle_id == "notepad.exe").unwrap().running);
        let two = vec![PetAppEntry::new("a\\notes.exe", &["Notes"], false), PetAppEntry::new("b\\notes.exe", &["Notes"], true)];
        assert_eq!(PetAppMatcher::match_app("notes", &two), PetAppMatch::Found(two[1].clone()));
    }

    #[test]
    fn protected_apps() {
        assert!(PetAppMatcher::is_protected("Explorer.EXE"));
        assert!(PetAppMatcher::is_protected("nachocombo.exe"));
        assert!(!PetAppMatcher::is_protected("msedge.exe"));
    }

    #[test]
    fn numbers_displays_left_to_right() {
        // Primary first in the system's order, an external display on the left.
        let screens = [Rect::new(0.0, 0.0, 1512.0, 982.0), Rect::new(-1920.0, 0.0, 1920.0, 1080.0), Rect::new(1512.0, 0.0, 1080.0, 1920.0)];
        assert_eq!(PetDisplays::index_for_number(1, &screens), Some(1));
        assert_eq!(PetDisplays::index_for_number(2, &screens), Some(0));
        assert_eq!(PetDisplays::index_for_number(3, &screens), Some(2));
        assert_eq!(PetDisplays::index_for_number(4, &screens), None);
        assert_eq!(PetDisplays::index_for_number(0, &screens), None);
        assert_eq!(PetDisplays::number_for_index(0, &screens), Some(2));
        // Stacked displays: the upper one (smaller y, top-left origin) first.
        let stacked = [Rect::new(0.0, 0.0, 1920.0, 1080.0), Rect::new(0.0, -1080.0, 1920.0, 1080.0)];
        assert_eq!(PetDisplays::index_for_number(1, &stacked), Some(1));
    }

    fn sample(name: &str) -> DesktopWorkspace {
        DesktopWorkspace {
            name: name.into(), saved_at: 1_800_000_000.0, displays: "1512x982+0+0".into(),
            apps: vec![DesktopWorkspaceApp { bundle_id: "msedge.exe".into(), name: "Edge".into() },
                       DesktopWorkspaceApp { bundle_id: "notepad.exe".into(), name: "메모장".into() }],
            windows: vec![SavedWindow::new("msedge.exe", "Edge", Rect::new(0.0, 0.0, 756.0, 957.0), 0),
                          SavedWindow::new("notepad.exe", "메모장", Rect::new(756.0, 0.0, 756.0, 957.0), 0)],
            gadgets: BTreeMap::from([("focusDim".into(), true), ("keepAwake".into(), false)]),
        }
    }

    #[test]
    fn round_trip_through_the_file() {
        let mut store = DesktopWorkspaceStore::default();
        assert_eq!(store.save(sample("업무 모드")), SaveResult::Added);
        let data = store.encoded();
        let text = String::from_utf8(data.clone()).unwrap();
        assert!(text.contains("\"version\": 1"));
        assert!(text.contains("2027-01-15T08:00:00Z"));
        assert_eq!(DesktopWorkspaceStore::decode(&data), Some(store.clone()));
        let dir = std::env::temp_dir().join(format!("ws-{}", crate::core::pet::media::PetIDs::new_uuid()));
        let path = dir.join("workspaces.json");
        store.write(&path).unwrap();
        assert_eq!(DesktopWorkspaceStore::load(&path), store);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_older_and_newer_files_leniently() {
        let json = r#"{"version": 2, "future": {"x": 1}, "workspaces": [
          {"name": "집중", "savedAt": "2026-11-13T00:00:00Z", "apps": [{"bundleID": "notepad.exe", "name": "메모장"}],
           "colorTheme": "dark"},
          {"name": 42},
          {"name": "빈 공간"}
        ]}"#;
        let store = DesktopWorkspaceStore::decode(json.as_bytes()).unwrap();
        assert_eq!(store.workspaces.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), vec!["집중", "빈 공간"]);
        assert_eq!(store.workspaces[0].apps.len(), 1);
        assert_eq!(store.workspaces[0].saved_at, 1_794_528_000.0);
        assert_eq!(store.workspaces[0].windows, vec![]);
        assert_eq!(store.workspaces[1].gadgets, BTreeMap::new());
        assert_eq!(DesktopWorkspaceStore::decode(b"not json"), None);
        let missing = std::env::temp_dir().join(format!("none-{}.json", crate::core::pet::media::PetIDs::new_uuid()));
        assert_eq!(DesktopWorkspaceStore::load(&missing), DesktopWorkspaceStore::default());
    }

    #[test]
    fn names_replace_remove_and_limit() {
        let mut store = DesktopWorkspaceStore::default();
        assert_eq!(store.save(sample("업무 모드")), SaveResult::Added);
        assert_eq!(store.save(sample("업무모드")), SaveResult::Replaced); // same name without the space
        assert_eq!(store.workspaces.len(), 1);
        assert_eq!(store.workspaces[0].name, "업무모드");
        assert!(store.workspace("업무 모드").is_some());
        for i in 1..DesktopWorkspaceStore::MAX_WORKSPACES { assert_eq!(store.save(sample(&format!("ws {i}"))), SaveResult::Added); }
        assert_eq!(store.save(sample("one too many")), SaveResult::Full);
        assert_eq!(store.workspaces.len(), DesktopWorkspaceStore::MAX_WORKSPACES);
        assert_eq!(store.save(sample("WS 3")), SaveResult::Replaced); // replacing still works when full
        assert_eq!(store.workspaces[0].name, "WS 3");
        assert!(store.remove("ws3"));
        assert!(!store.remove("ws3"));
        assert_eq!(DesktopWorkspaceStore::valid_name("  집중 "), Ok("집중".into()));
        assert_eq!(DesktopWorkspaceStore::valid_name(" "), Err(WorkspaceNameProblem::Empty));
        assert_eq!(DesktopWorkspaceStore::valid_name(&"가".repeat(41)), Err(WorkspaceNameProblem::TooLong));
        assert_eq!(DesktopWorkspaceStore::valid_name("a\u{7}b"), Err(WorkspaceNameProblem::BadCharacters));
        assert_eq!(DesktopWorkspaceStore::valid_name("a\u{2028}b"), Err(WorkspaceNameProblem::BadCharacters));
    }

    #[test]
    fn only_switchable_gadgets_are_applied() {
        let mut w = sample("업무 모드");
        w.gadgets = BTreeMap::from([("focusDim".into(), true), ("computerUse".into(), true), ("nachoPet".into(), false),
                                    ("removedGadget".into(), true), ("keepAwake".into(), false)]);
        assert_eq!(w.gadget_switches(), vec![(ModuleId::FocusDim, true), (ModuleId::KeepAwake, false)]);
        assert_eq!(w.korean_summary(), "앱 2개 · 창 2개 · 가젯 3개 켜짐");
        assert_eq!(w.summary(), "2 apps, 2 windows, 3 gadgets on");
    }

    #[test]
    fn apply_report() {
        let mut r = WorkspaceApplyReport::new("업무 모드");
        r.opened_apps = vec!["메모장".into()];
        r.placed_windows = 2;
        r.saved_windows = 2;
        r.gadgets_changed = vec!["Focus Dim on".into()];
        assert!(r.ok());
        assert!(r.outcome().message.contains("opened 메모장 (apps already open were left running)"));
        r.windows_need_permission = true;
        assert!(!r.ok());
        assert!(r.outcome().message.contains("administrator"));
        assert!(r.outcome().message.contains("관리자 권한"));
        let parts = [PetPartResult::new("volume", true, "set to 40%"), PetPartResult::new("dnd", false, "notifications could not be switched")];
        let o = PetPartResult::summary(&parts);
        assert!(!o.ok);
        assert_eq!(o.result_json()["results"].as_array().map(|a| a.len()), Some(2));
    }
}
