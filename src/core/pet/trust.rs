//! "Always allow" per gadget. Port of `GadgetTrust.swift` (the macOS rule): a content_out tool (the PC's content
//! leaves it: clipboard text, a capture, a file) is declared with `"approval": "required"`, so nacho-server shows an
//! approval card every time, unless the user switched its gadget to "always allow". Default off; destructive tools
//! always ask whatever the switch says.
//!
//! Stored in `gadget-trust.json` next to `settings.json`, in the macOS shape (`{"version":1,"alwaysAllowed":["clipboard"]}`,
//! macOS gadget ids), so a later port of the full macOS registry (W5) reads the same file. Every change notifies the
//! observers; the shell then declares its capabilities again (the connector sends a `capabilities` frame and a ping
//! barrier, see `connector_client.rs`).
//!
//! `disabled` is the extension point for switching single actions off on the connection tab (as on macOS): the
//! connector names in it are neither declared nor run. Memory only, as on macOS.
use super::gadget_tools::PetGadgetTools;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type TrustObserver = Arc<dyn Fn() + Send + Sync>;

/// One switch row for the settings screen: "이 가젯은 항상 허용".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustRow {
    /// macOS gadget id (`clipboard`).
    pub gadget: String,
    pub korean_name: &'static str,
    pub display_name: &'static str,
    pub on: bool,
}

#[derive(Default)]
struct State {
    allowed: BTreeSet<String>,
    disabled: BTreeSet<String>,
    path: Option<PathBuf>,
    observers: Vec<(u64, TrustObserver)>,
    next: u64,
}

pub struct GadgetTrust {
    trustable: BTreeSet<String>,
    state: Mutex<State>,
}

impl GadgetTrust {
    pub const FILE_NAME: &'static str = "gadget-trust.json";
    /// The switch and its note on the settings screen (English keys of `assets/i18n/<lang>.json`).
    pub const LABEL: &'static str = "Always allow this gadget";
    pub const NOTE: &'static str = "Nacho may send this gadget's content (such as clipboard text) without an approval card each time. Off by default.";
    pub const DESTRUCTIVE_NOTE: &'static str = "Quitting an app always asks first.";

    /// `trustable`: the gadgets that may be switched (those with a content_out tool the shell declares,
    /// [`PetGadgetTools::trustable`]). In memory until [`GadgetTrust::attach`], so tests never read the user's file.
    pub fn new(trustable: BTreeSet<String>) -> Self { Self { trustable, state: Mutex::new(State::default()) } }

    /// For the Windows declaration ([`PetGadgetTools::WINDOWS`], plus the chat handoff tools when the shell has them).
    pub fn for_windows(chat_handoff: bool) -> Self {
        let mut names: Vec<&str> = PetGadgetTools::WINDOWS.to_vec();
        if chat_handoff { names.extend(PetGadgetTools::WITH_CHAT_HANDOFF); }
        Self::new(PetGadgetTools::trustable(&names))
    }

    pub fn trustable(&self) -> &BTreeSet<String> { &self.trustable }

    /// Starts keeping the switches in `path` (read now, written on every change). A missing or broken file is all off.
    pub fn attach(&self, path: &Path) {
        let loaded = Self::decode(&std::fs::read(path).unwrap_or_default(), &self.trustable);
        let changed = {
            let mut s = self.state.lock().unwrap();
            s.path = Some(path.to_path_buf());
            let changed = s.allowed != loaded;
            s.allowed = loaded;
            changed
        };
        if changed { self.notify(); }
    }

    /// The gadgets whose content_out tools are declared without an approval card.
    pub fn always_allowed(&self) -> BTreeSet<String> { self.state.lock().unwrap().allowed.clone() }

    pub fn is_always_allowed(&self, gadget: &str) -> bool { self.state.lock().unwrap().allowed.contains(gadget) }

    /// false (and nothing changes) for a gadget without a trustable tool, or when nothing changed.
    pub fn set_always_allowed(&self, gadget: &str, on: bool) -> bool {
        if on && !self.trustable.contains(gadget) { return false; }
        let changed = {
            let mut s = self.state.lock().unwrap();
            let changed = if on { s.allowed.insert(gadget.to_string()) } else { s.allowed.remove(gadget) };
            if changed {
                if let Some(p) = s.path.clone() {
                    // A switch that cannot be stored stays as set for this run only; nothing else depends on the file.
                    let _ = Self::write(&p, &s.allowed);
                }
            }
            changed
        };
        if changed { self.notify(); }
        changed
    }

    /// Connector names the user switched off.
    pub fn disabled(&self) -> BTreeSet<String> { self.state.lock().unwrap().disabled.clone() }

    pub fn set_disabled(&self, names: BTreeSet<String>) {
        let changed = {
            let mut s = self.state.lock().unwrap();
            let changed = s.disabled != names;
            s.disabled = names;
            changed
        };
        if changed { self.notify(); }
    }

    /// The switch rows, in a fixed order (clipboard, shelf, capture, then any other trustable gadget).
    pub fn rows(&self) -> Vec<TrustRow> {
        let allowed = self.always_allowed();
        let order = ["clipboard", "shelf", "capture"];
        let mut gadgets: Vec<&String> = self.trustable.iter().collect();
        gadgets.sort_by_key(|g| (order.iter().position(|o| o == g).unwrap_or(order.len()), g.to_string()));
        gadgets.into_iter().map(|g| TrustRow {
            gadget: g.clone(),
            korean_name: PetGadgetTools::korean_name(g),
            display_name: PetGadgetTools::display_name(g),
            on: allowed.contains(g),
        }).collect()
    }

    /// Called (on the thread that changed the switch) after `always_allowed` or `disabled` changed.
    pub fn observe(&self, f: TrustObserver) -> u64 {
        let mut s = self.state.lock().unwrap();
        s.next += 1;
        let token = s.next;
        s.observers.push((token, f));
        token
    }

    pub fn remove_observer(&self, token: u64) { self.state.lock().unwrap().observers.retain(|(t, _)| *t != token); }

    fn notify(&self) {
        let calls: Vec<TrustObserver> = self.state.lock().unwrap().observers.iter().map(|(_, f)| f.clone()).collect();
        for f in calls { f(); }
    }

    // file

    /// Lenient: unknown or untrustable ids are dropped, a broken file is all off.
    pub fn decode(data: &[u8], trustable: &BTreeSet<String>) -> BTreeSet<String> {
        let Ok(v) = serde_json::from_slice::<Value>(data) else { return BTreeSet::new() };
        let Some(list) = v.get("alwaysAllowed").and_then(Value::as_array) else { return BTreeSet::new() };
        list.iter().filter_map(Value::as_str).filter(|g| trustable.contains(*g)).map(str::to_string).collect()
    }

    pub fn encode(allowed: &BTreeSet<String>) -> Vec<u8> {
        serde_json::to_vec(&json!({"version": 1, "alwaysAllowed": allowed.iter().collect::<Vec<_>>()})).unwrap_or_default()
    }

    fn write(path: &Path, allowed: &BTreeSet<String>) -> std::io::Result<()> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, Self::encode(allowed))?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn trust() -> GadgetTrust { GadgetTrust::new(["capture", "clipboard", "shelf"].iter().map(|s| s.to_string()).collect()) }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nacho-trust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn default_off_and_only_trustable_gadgets_switch() {
        let t = trust();
        assert!(t.always_allowed().is_empty(), "always allow starts off");
        assert!(!t.set_always_allowed("awake", true), "a gadget without a content_out tool has no switch");
        assert!(!t.set_always_allowed("computer", true));
        assert!(t.set_always_allowed("clipboard", true));
        assert!(!t.set_always_allowed("clipboard", true), "no change, no notice");
        assert!(t.is_always_allowed("clipboard"));
        assert!(t.set_always_allowed("clipboard", false));
        assert!(t.always_allowed().is_empty());
    }

    #[test]
    fn windows_switches_follow_what_windows_declares() {
        // System Monitor and Disk Map send program names and paths, so Windows asks for them too (W5a).
        assert_eq!(GadgetTrust::for_windows(false).trustable(), &["clipboard", "disk", "monitor"].iter().map(|s| s.to_string()).collect());
        assert_eq!(GadgetTrust::for_windows(true).trustable().len(), 5);
        let rows = GadgetTrust::for_windows(true).rows();
        assert_eq!(rows.iter().map(|r| r.gadget.as_str()).collect::<Vec<_>>(), ["clipboard", "shelf", "capture", "disk", "monitor"]);
        assert_eq!(rows[3].korean_name, "디스크 지도");
        assert!(!GadgetTrust::for_windows(true).set_always_allowed("desktop", true), "desktop tidy asks every time");
        assert_eq!(rows[0].korean_name, "클립보드 기록");
        assert!(rows.iter().all(|r| !r.on));
    }

    #[test]
    fn stored_and_read_back_in_the_macos_shape() {
        let d = dir("store");
        let path = d.join(GadgetTrust::FILE_NAME);
        let t = trust();
        t.attach(&path);
        t.set_always_allowed("clipboard", true);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, r#"{"alwaysAllowed":["clipboard"],"version":1}"#);
        // A new run reads the same switch back.
        let again = trust();
        again.attach(&path);
        assert!(again.is_always_allowed("clipboard"));
        // Switched off in the settings screen: stored too.
        again.set_always_allowed("clipboard", false);
        let third = trust();
        third.attach(&path);
        assert!(third.always_allowed().is_empty(), "turning it off is stored");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn switch_text_is_in_every_language() {
        for (lang, file) in [("ko", include_str!("../../../assets/i18n/ko.json")), ("ja", include_str!("../../../assets/i18n/ja.json")),
                             ("zh", include_str!("../../../assets/i18n/zh.json"))] {
            let t: std::collections::HashMap<String, String> = serde_json::from_str(file).unwrap();
            for key in [GadgetTrust::LABEL, GadgetTrust::NOTE, GadgetTrust::DESTRUCTIVE_NOTE] {
                assert!(t.get(key).is_some_and(|v| !v.is_empty()), "{lang}: {key}");
            }
        }
    }

    #[test]
    fn lenient_file() {
        let ok: BTreeSet<String> = ["clipboard".to_string()].into();
        assert_eq!(GadgetTrust::decode(br#"{"version":1,"alwaysAllowed":["clipboard","computer","nope"]}"#, &ok), ok);
        assert!(GadgetTrust::decode(b"garbage", &ok).is_empty());
        assert!(GadgetTrust::decode(br#"{"alwaysAllowed":"clipboard"}"#, &ok).is_empty());
        assert!(GadgetTrust::decode(b"", &ok).is_empty());
        // The macOS app's file (written by JSONEncoder, keys in any order) reads the same.
        assert_eq!(GadgetTrust::decode(br#"{"alwaysAllowed":["clipboard"],"version":1}"#, &ok), ok);
    }

    #[test]
    fn observers_hear_every_change_and_can_leave() {
        let t = trust();
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let token = t.observe(Arc::new(move || { c.fetch_add(1, Ordering::SeqCst); }));
        t.set_always_allowed("clipboard", true);
        t.set_always_allowed("awake", true); // refused: no notice
        t.set_disabled(["combo_clipboard_get_item".to_string()].into());
        t.set_disabled(["combo_clipboard_get_item".to_string()].into()); // same: no notice
        assert_eq!(n.load(Ordering::SeqCst), 2);
        t.remove_observer(token);
        t.set_always_allowed("clipboard", false);
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }
}
