//! The settings file. Every field defaults on its own, so an older or partly broken file keeps everything else.
use super::clock::ClockHours;
use super::mouse::{Precision, Saved};
use super::shortcut::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModuleId { TrayOrganizer, SystemMonitor, WindowSnap, FileShelf, CaptureFloat, FocusDim, KeepAwake, Clipboard, DeviceControl, DiskMap, AppSwitch, CalendarClock, MouseTune }

impl ModuleId {
    pub const ALL: [ModuleId; 13] = [ModuleId::TrayOrganizer, ModuleId::SystemMonitor, ModuleId::WindowSnap, ModuleId::FileShelf, ModuleId::CaptureFloat, ModuleId::FocusDim,
        ModuleId::KeepAwake, ModuleId::Clipboard, ModuleId::DeviceControl, ModuleId::DiskMap, ModuleId::AppSwitch, ModuleId::CalendarClock, ModuleId::MouseTune];
    pub fn name(self) -> &'static str {
        match self {
            ModuleId::TrayOrganizer => "Tray Organizer", ModuleId::SystemMonitor => "System Monitor", ModuleId::WindowSnap => "Window Snap",
            ModuleId::FileShelf => "File Shelf", ModuleId::CaptureFloat => "Capture Float", ModuleId::FocusDim => "Focus Dim",
            ModuleId::KeepAwake => "Keep Awake", ModuleId::Clipboard => "Clipboard History", ModuleId::DeviceControl => "Device Control", ModuleId::DiskMap => "Disk Map", ModuleId::AppSwitch => "App Switch",
            ModuleId::CalendarClock => "Calendar Clock", ModuleId::MouseTune => "Mouse Tune",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            ModuleId::TrayOrganizer => "tray", ModuleId::SystemMonitor => "monitor", ModuleId::WindowSnap => "snap",
            ModuleId::FileShelf => "shelf", ModuleId::CaptureFloat => "capture", ModuleId::FocusDim => "dim",
            ModuleId::KeepAwake => "awake", ModuleId::Clipboard => "clipboard", ModuleId::DeviceControl => "devices", ModuleId::DiskMap => "diskmap", ModuleId::AppSwitch => "appswitch",
            ModuleId::CalendarClock => "clock", ModuleId::MouseTune => "mouse",
        }
    }
    pub fn from_key(k: &str) -> Option<ModuleId> { Self::ALL.into_iter().find(|m| m.key() == k) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MonitorItem { Cpu, Memory, Disk, Network, Sensors, Battery }

impl MonitorItem {
    pub const ALL: [MonitorItem; 6] = [MonitorItem::Cpu, MonitorItem::Memory, MonitorItem::Disk, MonitorItem::Network, MonitorItem::Sensors, MonitorItem::Battery];
    pub fn name(self) -> &'static str {
        match self { MonitorItem::Cpu => "CPU", MonitorItem::Memory => "Memory", MonitorItem::Disk => "Disk", MonitorItem::Network => "Network", MonitorItem::Sensors => "Sensors", MonitorItem::Battery => "Battery" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShelfTrigger { Shake, AnyDrag, ShortcutOnly }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DimScope { FrontWindow, FrontApp }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SnapRepeat { CycleSizes, NextDisplay, Nothing }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchWindow { Always, WhenNeeded, Never }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Options {
    pub auto_collapse_seconds: i32,
    pub snap_gap: i32,
    pub snap_repeat: SnapRepeat,
    pub snap_drag_to_edges: bool,
    pub snap_restore_on_drag: bool,
    pub window_motion: bool,
    pub cpu_alert_percent: i32,
    pub alert_notifications: bool,
    pub shelf_trigger: ShelfTrigger,
    pub capture_rounded_corners: bool,
    pub dim_scope: DimScope,
    pub dim_other_displays: bool,
    pub dim_fade_seconds: f64,
    pub public_ip_auto: bool,
    /// Keep Awake: the display stays on too.
    pub awake_display: bool,
    /// Keep Awake turns itself on while one of these programs runs (file names such as "zoom.exe").
    pub awake_apps: Vec<String>,
    /// Lets the PC sleep again below this charge on battery (0 = never).
    pub awake_battery_guard: i32,
    /// How many copies Clipboard History keeps (pins come on top).
    pub clip_max: i32,
    /// Sound devices to switch to as soon as they connect (their endpoint ids).
    pub preferred_output: Option<String>,
    pub preferred_input: Option<String>,
    /// Switches Windows between light and dark mode at two times of day (minutes after midnight).
    pub theme_schedule: bool,
    pub light_minute: i32,
    pub dark_minute: i32,
    /// Window Snap: puts windows back where they were when the same set of displays comes back.
    pub snap_restore_layouts: bool,
    /// File Shelf: Space in File Explorer previews the selected file.
    pub explorer_preview: bool,
    /// Calendar Clock: what the clock shows, and the cities of the world clocks (time zone ids such as "Asia/Tokyo").
    pub clock_date: bool,
    pub clock_weekday: bool,
    pub clock_seconds: bool,
    pub clock_hours: ClockHours,
    pub world_clocks: Vec<String>,
    /// Mouse Tune: a wheel mouse scrolls the other way (precision touchpads keep their own direction).
    pub mouse_reverse: bool,
    /// Mouse Tune: lines per wheel notch, as a factor of the user's own setting.
    pub mouse_speed: f64,
    /// Mouse Tune: pointer speed 1 to 20; 0 leaves it as it is.
    pub mouse_pointer: i32,
    pub mouse_precision: Precision,
}

impl Default for Options {
    fn default() -> Self {
        Options { auto_collapse_seconds: 10, snap_gap: 0, snap_repeat: SnapRepeat::CycleSizes, snap_drag_to_edges: true, snap_restore_on_drag: true,
            window_motion: true, cpu_alert_percent: 90, alert_notifications: false, shelf_trigger: ShelfTrigger::Shake, capture_rounded_corners: true,
            dim_scope: DimScope::FrontWindow, dim_other_displays: true, dim_fade_seconds: 0.25, public_ip_auto: false,
            awake_display: true, awake_apps: vec![], awake_battery_guard: 0, clip_max: 100, preferred_output: None, preferred_input: None,
            theme_schedule: false, light_minute: 7 * 60, dark_minute: 19 * 60, snap_restore_layouts: true, explorer_preview: true,
            clock_date: true, clock_weekday: true, clock_seconds: false, clock_hours: ClockHours::System, world_clocks: vec![],
            mouse_reverse: true, mouse_speed: 1.0, mouse_pointer: 0, mouse_precision: Precision::Keep }
    }
}

impl Options {
    pub const COLLAPSE_CHOICES: [i32; 6] = [0, 5, 10, 15, 30, 60];
    pub const GAP_CHOICES: [i32; 6] = [0, 4, 8, 12, 16, 24];
    pub const FADE_CHOICES: [f64; 4] = [0.0, 0.25, 0.5, 1.0];
    pub const CPU_CHOICES: [i32; 5] = [0, 70, 80, 90, 95];
    pub const GUARD_CHOICES: [i32; 4] = [0, 10, 20, 30];
    pub const CLIP_CHOICES: [i32; 4] = [50, 100, 200, 500];
    fn clamped(mut self) -> Self {
        self.auto_collapse_seconds = self.auto_collapse_seconds.clamp(0, 3600);
        self.snap_gap = self.snap_gap.clamp(0, 64);
        self.cpu_alert_percent = self.cpu_alert_percent.clamp(0, 100);
        self.awake_battery_guard = self.awake_battery_guard.clamp(0, 90);
        self.clip_max = self.clip_max.clamp(10, 1000);
        self.light_minute = self.light_minute.rem_euclid(1440);
        self.dark_minute = self.dark_minute.rem_euclid(1440);
        self.world_clocks.truncate(super::clock::MAX_WORLD_CLOCKS);
        self.mouse_speed = if self.mouse_speed.is_finite() { self.mouse_speed.clamp(0.25, 4.0) } else { 1.0 };
        self.mouse_pointer = self.mouse_pointer.clamp(0, 20);
        self.dim_fade_seconds = if self.dim_fade_seconds.is_finite() { self.dim_fade_seconds.clamp(0.0, 5.0) } else { 0.25 };
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Tuning {
    pub dim_intensity: Option<f64>,
    pub snap_gap: Option<i32>,
    pub auto_collapse_seconds: Option<i32>,
    pub dim_other_displays: Option<bool>,
    pub monitor_items: Option<Vec<MonitorItem>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedCombo { pub id: String, pub name: String, pub gadgets: Vec<ModuleId>, #[serde(default)] pub tuning: Tuning }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub enabled: BTreeMap<ModuleId, bool>,
    pub dim_intensity: f64,
    pub options: Options,
    pub monitor_items: Vec<MonitorItem>,
    pub pinned_tools: Vec<ModuleId>,
    pub tool_shortcuts: BTreeMap<ModuleId, Shortcut>,
    /// A user's choice per action id; `None` means the shortcut was removed.
    pub action_shortcuts: BTreeMap<String, Option<Shortcut>>,
    pub launch_window: LaunchWindow,
    pub attention_dot: bool,
    pub combo: Option<String>,
    pub custom_combos: Vec<SavedCombo>,
    pub welcomed: bool,
    /// Icons the Tray Organizer keeps in view (their executable paths).
    pub tray_keep: Vec<String>,
    /// Each icon's own setting before the organizer changed it, to put back when it stops.
    pub tray_original: BTreeMap<String, u32>,
    pub tray_collapsed: bool,
    pub window_size: Option<(f64, f64)>,
    pub last_page: Option<String>,
    /// "en", "ko", "ja" or "zh"; None follows Windows.
    pub language: Option<String>,
    /// Mouse Tune: the user's own system values and the ones it set, kept so they come back even after a crash.
    pub mouse_saved: Option<Saved>,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings { enabled: BTreeMap::new(), dim_intensity: 0.5, options: Options::default(),
            monitor_items: vec![MonitorItem::Cpu, MonitorItem::Memory, MonitorItem::Network], pinned_tools: vec![], tool_shortcuts: BTreeMap::new(),
            action_shortcuts: BTreeMap::new(), launch_window: LaunchWindow::WhenNeeded, attention_dot: true, combo: None, custom_combos: vec![], welcomed: false,
            tray_keep: vec![], tray_original: BTreeMap::new(), tray_collapsed: false, window_size: None, last_page: None, language: None, mouse_saved: None }
    }
}

/// Whether light mode is due at `now` (minutes after midnight) for a light..dark day, also when dark comes before light.
pub fn light_due(now: i32, light: i32, dark: i32) -> bool {
    if light == dark { return true; }
    if light < dark { now >= light && now < dark } else { now >= light || now < dark }
}

pub fn clamp_dim(v: f64) -> f64 { if v.is_finite() { v.clamp(0.1, 0.9) } else { 0.5 } }

impl AppSettings {
    pub fn is_on(&self, id: ModuleId) -> bool { self.enabled.get(&id).copied().unwrap_or(false) }
    pub fn set_on(&mut self, id: ModuleId, on: bool) { self.enabled.insert(id, on); }

    /// A missing or broken file gives the defaults; a broken file is kept beside it, never overwritten silently.
    pub fn load(path: &Path) -> AppSettings {
        let Ok(text) = std::fs::read_to_string(path) else { return AppSettings::default() };
        match serde_json::from_str::<AppSettings>(&text) {
            Ok(mut s) => { s.options = s.options.clamped(); s.dim_intensity = clamp_dim(s.dim_intensity); s }
            Err(_) => { let _ = std::fs::copy(path, path.with_extension("broken.json")); AppSettings::default() }
        }
    }

    /// Written to a side file first, so a crash mid-write cannot lose the settings.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn old_or_partial_files_keep_the_rest() {
        let s: AppSettings = serde_json::from_str(r#"{"dimIntensity":0.7,"options":{"snapGap":8},"unknown":1}"#).unwrap();
        assert_eq!(s.dim_intensity, 0.7);
        assert_eq!(s.options.snap_gap, 8);
        assert!(s.options.window_motion);
        assert_eq!(s.launch_window, LaunchWindow::WhenNeeded);
    }
    #[test] fn a_settings_file_from_before_calendar_clock_still_loads() {
        let s: AppSettings = serde_json::from_str(r#"{"enabled":{"focusDim":true,"keepAwake":false},"options":{"snapGap":8,"awakeApps":["zoom.exe"]}}"#).unwrap();
        assert!(s.is_on(ModuleId::FocusDim));
        assert!(!s.is_on(ModuleId::CalendarClock), "a new gadget is off until it is turned on");
        assert_eq!(s.options.snap_gap, 8);
        assert_eq!(s.options.awake_apps, vec!["zoom.exe".to_string()]);
        let o = s.options;
        assert!(o.clock_date && o.clock_weekday && !o.clock_seconds);
        assert_eq!(o.clock_hours, ClockHours::System);
        assert!(o.world_clocks.is_empty());
    }
    #[test] fn clock_options_roundtrip_and_survive_a_bad_value() {
        let s: AppSettings = serde_json::from_str(r#"{"enabled":{"calendarClock":true},"options":{"clockHours":"twentyFour","clockSeconds":true,"worldClocks":["Asia/Tokyo"]}}"#).unwrap();
        assert!(s.is_on(ModuleId::CalendarClock));
        assert_eq!((s.options.clock_hours, s.options.clock_seconds, s.options.world_clocks.clone()), (ClockHours::TwentyFour, true, vec!["Asia/Tokyo".to_string()]));
        let again: AppSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(again, s);
        // A value this version does not know falls back instead of throwing the whole file away.
        let odd: AppSettings = serde_json::from_str(r#"{"dimIntensity":0.7,"options":{"clockHours":"bogus"}}"#).unwrap();
        assert_eq!(odd.dim_intensity, 0.7);
        assert_eq!(odd.options.clock_hours, ClockHours::Unknown);
    }
    #[test] fn too_many_world_clocks_are_trimmed_on_load() {
        let dir = std::env::temp_dir().join(format!("nacho-clocks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.json");
        std::fs::write(&path, r#"{"options":{"worldClocks":["UTC","UTC","UTC","UTC","UTC","UTC","UTC","UTC"]}}"#).unwrap();
        assert_eq!(AppSettings::load(&path).options.world_clocks.len(), super::super::clock::MAX_WORLD_CLOCKS);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test] fn roundtrip_and_broken_file() {
        let dir = std::env::temp_dir().join(format!("nacho-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mut s = AppSettings::default();
        s.set_on(ModuleId::FocusDim, true);
        s.action_shortcuts.insert("snap.left".into(), None);
        s.tool_shortcuts.insert(ModuleId::FileShelf, Shortcut::new(65, CTRL | ALT));
        s.save(&path).unwrap();
        assert_eq!(AppSettings::load(&path), s);
        std::fs::write(&path, "{nope").unwrap();
        assert_eq!(AppSettings::load(&path), AppSettings::default());
        assert!(dir.join("settings.broken.json").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test] fn theme_schedule_handles_both_orders() {
        assert!(light_due(8 * 60, 7 * 60, 19 * 60));
        assert!(!light_due(20 * 60, 7 * 60, 19 * 60));
        assert!(!light_due(6 * 60, 7 * 60, 19 * 60));
        // Night shift: light from 22:00 to 06:00.
        assert!(light_due(23 * 60, 22 * 60, 6 * 60));
        assert!(light_due(60, 22 * 60, 6 * 60));
        assert!(!light_due(12 * 60, 22 * 60, 6 * 60));
    }
    #[test] fn mouse_tune_is_off_and_leaves_the_system_alone_until_asked() {
        let s: AppSettings = serde_json::from_str(r#"{"enabled":{"focusDim":true},"options":{"snapGap":8}}"#).unwrap();
        assert!(!s.is_on(ModuleId::MouseTune));
        assert!(s.is_on(ModuleId::FocusDim), "an older file keeps the user's choices");
        assert_eq!((s.options.mouse_speed, s.options.mouse_pointer, s.options.mouse_precision), (1.0, 0, Precision::Keep));
        assert_eq!(s.mouse_saved, None);
        assert_eq!(ModuleId::from_key("mouse"), Some(ModuleId::MouseTune));
    }
    #[test] fn mouse_values_are_clamped_and_saved_values_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("nacho-mouse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.json");
        std::fs::write(&path, r#"{"options":{"mouseSpeed":99,"mousePointer":-4}}"#).unwrap();
        let mut s = AppSettings::load(&path);
        assert_eq!((s.options.mouse_speed, s.options.mouse_pointer), (4.0, 0));
        let sys = crate::core::mouse::Sys { pointer_speed: 10, wheel_lines: 3, precision: true };
        s.mouse_saved = Some(Saved { original: sys, applied: crate::core::mouse::Sys { wheel_lines: 6, ..sys } });
        s.save(&path).unwrap();
        assert_eq!(AppSettings::load(&path).mouse_saved, s.mouse_saved);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test] fn out_of_range_values_are_clamped() {
        let dir = std::env::temp_dir().join(format!("nacho-clamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.json");
        std::fs::write(&path, r#"{"dimIntensity":5,"options":{"snapGap":999,"cpuAlertPercent":-3}}"#).unwrap();
        let s = AppSettings::load(&path);
        assert_eq!((s.dim_intensity, s.options.snap_gap, s.options.cpu_alert_percent), (0.9, 64, 0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
