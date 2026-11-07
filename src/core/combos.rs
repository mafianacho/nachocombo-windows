//! Combos: a set of gadgets switched on together with a few options tuned.
use super::settings::*;
use ModuleId::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Combo { pub id: String, pub number: usize, pub name: String, pub line: String, pub who: String, pub gadgets: Vec<ModuleId>, pub tuning: Tuning, pub custom: bool }

pub const MAX_CUSTOM: usize = 5;
pub const MAX_NAME: usize = 24;

fn preset(id: &str, number: usize, name: &str, line: &str, who: &str, gadgets: &[ModuleId], tuning: Tuning) -> Combo {
    Combo { id: id.into(), number, name: name.into(), line: line.into(), who: who.into(), gadgets: gadgets.to_vec(), tuning, custom: false }
}

pub fn presets() -> Vec<Combo> {
    vec![
        preset("starter", 1, "Starter", "A first taste of the box.", "For trying it out: three gadgets that stay out of your way.", &[TrayOrganizer, FileShelf, FocusDim], Tuning { dim_intensity: Some(0.4), ..Default::default() }),
        preset("desk", 2, "Desk", "Windows in place, files in reach, references on top.", "For documents and design work that moves between windows and files.", &[WindowSnap, FileShelf, CaptureFloat], Tuning { snap_gap: Some(8), ..Default::default() }),
        preset("builder", 3, "Builder", "For long builds and a busy tray.", "For developers who keep builds and terminals running for hours.", &[SystemMonitor, WindowSnap, TrayOrganizer], Tuning { monitor_items: Some(vec![MonitorItem::Cpu, MonitorItem::Memory, MonitorItem::Network]), ..Default::default() }),
        preset("focus", 4, "Focus", "One window, a quiet tray.", "For writing or studying in one window for a long time.", &[FocusDim, TrayOrganizer], Tuning { dim_intensity: Some(0.6), auto_collapse_seconds: Some(5), dim_other_displays: Some(true), ..Default::default() }),
        preset("meeting", 5, "Meeting", "Ready for the call before it starts.", "For calls: the right headset, a mute key, and a PC that stays awake.", &[DeviceControl, KeepAwake, Clipboard], Tuning::default()),
        preset("presenter", 6, "Presenter", "A clean screen to share.", "For sharing your screen: a quiet tray, one bright window, no sleep.", &[TrayOrganizer, FocusDim, KeepAwake], Tuning { dim_intensity: Some(0.6), ..Default::default() }),
        preset("full", 7, "Full Combo", "Everything in the box.", "For anyone who installs all of these on every new PC.", &ModuleId::ALL, Tuning::default()),
    ]
}

pub fn all(s: &AppSettings) -> Vec<Combo> {
    let mut v = presets();
    let first = v.len() + 1;
    for (i, c) in s.custom_combos.iter().enumerate() {
        v.push(Combo { id: c.id.clone(), number: first + i, name: c.name.clone(), line: "Your own mix.".into(), who: "Made by you. Change it any time with Edit.".into(), gadgets: c.gadgets.clone(), tuning: c.tuning.clone(), custom: true });
    }
    v
}

/// The combo whose gadgets are exactly the ones on; the one ordered last wins a tie.
pub fn current(s: &AppSettings) -> Option<Combo> {
    let on: Vec<ModuleId> = ModuleId::ALL.into_iter().filter(|m| s.is_on(*m)).collect();
    if on.is_empty() { return None; }
    let same = |c: &Combo| { let mut g = c.gadgets.clone(); g.sort(); let mut o = on.clone(); o.sort(); g == o };
    let matching: Vec<Combo> = all(s).into_iter().filter(same).collect();
    matching.iter().find(|c| Some(&c.id) == s.combo.as_ref()).or(matching.first()).cloned()
}

impl Tuning {
    pub fn apply(&self, s: &mut AppSettings) {
        if let Some(v) = self.dim_intensity { s.dim_intensity = clamp_dim(v); }
        if let Some(v) = self.snap_gap { s.options.snap_gap = v; }
        if let Some(v) = self.auto_collapse_seconds { s.options.auto_collapse_seconds = v; }
        if let Some(v) = self.dim_other_displays { s.options.dim_other_displays = v; }
        if let Some(v) = &self.monitor_items { s.monitor_items = MonitorItem::ALL.into_iter().filter(|i| v.contains(i)).collect(); }
    }
    pub fn changes(&self, s: &AppSettings) -> Vec<String> {
        let mut l = vec![];
        let gap = |v: i32| if v == 0 { "None".to_string() } else { format!("{v} px") };
        let secs = |v: i32| if v == 0 { "Never".to_string() } else { format!("{v} s") };
        if let Some(v) = self.dim_intensity { if (s.dim_intensity - v).abs() > 0.001 { l.push(format!("Dim strength: {:.0}% → {:.0}%", s.dim_intensity * 100.0, v * 100.0)); } }
        if let Some(v) = self.snap_gap { if s.options.snap_gap != v { l.push(format!("Gap between windows: {} → {}", gap(s.options.snap_gap), gap(v))); } }
        if let Some(v) = self.auto_collapse_seconds { if s.options.auto_collapse_seconds != v { l.push(format!("Collapse again after: {} → {}", secs(s.options.auto_collapse_seconds), secs(v))); } }
        if let Some(v) = self.dim_other_displays { if s.options.dim_other_displays != v { l.push(if v { "Dim the other displays too".to_string() } else { "Dim only the display you work on".to_string() }); } }
        if let Some(v) = &self.monitor_items {
            let want: Vec<_> = MonitorItem::ALL.into_iter().filter(|i| v.contains(i)).collect();
            if s.monitor_items != want { l.push(format!("Tray items: {}", want.iter().map(|i| i.name()).collect::<Vec<_>>().join(", "))); }
        }
        l
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Plan { pub turns_on: Vec<ModuleId>, pub stays_on: Vec<ModuleId>, pub turns_off: Vec<ModuleId>, pub changes: Vec<String> }

pub fn plan(c: &Combo, s: &AppSettings) -> Plan {
    let on: Vec<ModuleId> = ModuleId::ALL.into_iter().filter(|m| s.is_on(*m)).collect();
    Plan { turns_on: c.gadgets.iter().copied().filter(|g| !on.contains(g)).collect(), stays_on: c.gadgets.iter().copied().filter(|g| on.contains(g)).collect(),
        turns_off: on.iter().copied().filter(|g| !c.gadgets.contains(g)).collect(), changes: c.tuning.changes(s) }
}

/// "My Combo", then "My Combo 2"…
pub fn free_name(base: &str, s: &AppSettings) -> String {
    let taken: Vec<String> = s.custom_combos.iter().map(|c| c.name.to_lowercase()).collect();
    if !taken.contains(&base.to_lowercase()) { return base.into(); }
    (2..).map(|n| format!("{base} {n}")).find(|n| !taken.contains(&n.to_lowercase())).unwrap()
}

pub fn clean_name(n: &str) -> String {
    let t = n.trim();
    if t.is_empty() { "My Combo".into() } else { t.chars().take(MAX_NAME).collect() }
}

/// A combo of one's own: replaces the one with the same id; refuses an empty one or a sixth.
pub fn save(s: &mut AppSettings, mut c: SavedCombo) -> bool {
    if c.gadgets.is_empty() { return false; }
    c.name = clean_name(&c.name);
    c.gadgets = ModuleId::ALL.into_iter().filter(|m| c.gadgets.contains(m)).collect();
    if let Some(i) = s.custom_combos.iter().position(|x| x.id == c.id) { s.custom_combos[i] = c; return true; }
    if s.custom_combos.len() >= MAX_CUSTOM { return false; }
    s.custom_combos.push(c);
    true
}

/// The box as it was before an order.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot { pub enabled: Vec<(ModuleId, bool)>, pub options: Options, pub dim: f64, pub monitor: Vec<MonitorItem>, pub combo: Option<String> }

pub fn snapshot(s: &AppSettings) -> Snapshot {
    Snapshot { enabled: ModuleId::ALL.into_iter().map(|m| (m, s.is_on(m))).collect(), options: s.options.clone(), dim: s.dim_intensity, monitor: s.monitor_items.clone(), combo: s.combo.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn with(on: &[ModuleId]) -> AppSettings { let mut s = AppSettings::default(); for m in on { s.set_on(*m, true); } s }
    #[test] fn current_matches_exact_set() {
        assert_eq!(current(&with(&[])), None);
        assert_eq!(current(&with(&[WindowSnap, FileShelf, CaptureFloat])).unwrap().id, "desk");
        assert_eq!(current(&with(&[WindowSnap])), None);
    }
    #[test] fn last_ordered_wins_a_tie() {
        let mut s = with(&[FocusDim]);
        s.custom_combos = vec![SavedCombo { id: "a".into(), name: "A".into(), gadgets: vec![FocusDim], tuning: Tuning::default() }, SavedCombo { id: "b".into(), name: "B".into(), gadgets: vec![FocusDim], tuning: Tuning::default() }];
        s.combo = Some("b".into());
        assert_eq!(current(&s).unwrap().id, "b");
    }
    #[test] fn plan_lists_what_changes() {
        let s = with(&[FocusDim, WindowSnap]);
        let desk = presets().into_iter().find(|c| c.id == "desk").unwrap();
        let p = plan(&desk, &s);
        assert_eq!(p.turns_on, vec![FileShelf, CaptureFloat]);
        assert_eq!(p.stays_on, vec![WindowSnap]);
        assert_eq!(p.turns_off, vec![FocusDim]);
        assert_eq!(p.changes, vec!["Gap between windows: None → 8 px"]);
    }
    #[test] fn tuning_applies() {
        let mut s = AppSettings::default();
        Tuning { dim_intensity: Some(0.6), monitor_items: Some(vec![MonitorItem::Network, MonitorItem::Cpu]), ..Default::default() }.apply(&mut s);
        assert_eq!(s.dim_intensity, 0.6);
        assert_eq!(s.monitor_items, vec![MonitorItem::Cpu, MonitorItem::Network]);
        assert!(Tuning::default().changes(&s).is_empty());
    }
    #[test] fn custom_limits_and_names() {
        let mut s = AppSettings::default();
        assert!(!save(&mut s, SavedCombo { id: "x".into(), name: "n".into(), gadgets: vec![], tuning: Tuning::default() }));
        for i in 0..MAX_CUSTOM { assert!(save(&mut s, SavedCombo { id: i.to_string(), name: "  ".into(), gadgets: vec![FocusDim], tuning: Tuning::default() })); }
        assert!(!save(&mut s, SavedCombo { id: "6".into(), name: "n".into(), gadgets: vec![FocusDim], tuning: Tuning::default() }));
        assert!(save(&mut s, SavedCombo { id: "0".into(), name: "x".repeat(40), gadgets: vec![FocusDim], tuning: Tuning::default() }));
        assert_eq!(s.custom_combos[0].name.len(), MAX_NAME);
        assert_eq!(s.custom_combos[1].name, "My Combo");
        assert_eq!(free_name("My Combo", &s), "My Combo 2");
        assert_eq!(all(&s)[7].number, 8);
    }
}
