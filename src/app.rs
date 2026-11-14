//! The running app: settings, gadget lifecycle, shortcuts and the hidden main window that receives everything global.
use crate::core::catalog;
use crate::core::combos::{self, Combo, Snapshot};
use crate::core::layout::SnapAction;
use crate::core::settings::*;
use crate::core::shortcut::*;
use crate::{appswitch, awake, capture, clipboard, devices, dim, hooks, monitor, panel, pages, shelf, snap, tray, trayorg};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const MAIN_CLASS: PCWSTR = w!("NachoComboMain");
pub const WM_LATER: u32 = WM_APP + 1;
pub const WM_TRAY: u32 = WM_APP + 2;
pub const WM_SHAKE: u32 = WM_APP + 3;
pub const WM_ANYDRAG: u32 = WM_APP + 4;
pub const WM_WAKE: u32 = WM_APP + 5;
pub const WM_OCR_DONE: u32 = WM_APP + 6;
pub const WM_PUBLIC_IP: u32 = WM_APP + 7;
pub const WM_DEVICES: u32 = WM_APP + 8;
pub const WM_PEEK: u32 = WM_APP + 9;

pub const TIMER_MONITOR: usize = 1;
pub const TIMER_TRAY: usize = 2;
pub const TIMER_DIM: usize = 3;
pub const TIMER_FADE: usize = 4;
pub const TIMER_DRAG: usize = 5;
pub const TIMER_COMBO_UNDO: usize = 6;
pub const TIMER_AWAKE: usize = 7;
pub const TIMER_PASTE: usize = 8;
pub const TIMER_THEME: usize = 9;
pub const TIMER_LAYOUT: usize = 10;
pub const TIMER_ONTOP: usize = 11;
pub const TIMER_APPSWITCH: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub enum State { Off, Running, Failed(String) }

pub struct App {
    pub settings: AppSettings,
    pub dir: PathBuf,
    pub states: BTreeMap<ModuleId, State>,
    pub main: HWND,
    pub debug_ctl: bool,
    later: Vec<Box<dyn FnOnce()>>,
    pub hotkey_ids: Vec<String>,
    /// Shortcuts another app holds: the action id.
    pub hotkey_clashes: BTreeSet<String>,
    pub tray: tray::Tray,
    pub snap: snap::Snap,
    pub dim: dim::Dim,
    pub organizer: trayorg::Organizer,
    pub shelf: shelf::ShelfModule,
    pub capture: capture::Capture,
    pub monitor: monitor::Monitor,
    pub awake: awake::Awake,
    pub clipboard: clipboard::ClipModule,
    pub devices: devices::Devices,
    pub appswitch: appswitch::AppSwitch,
    pub settings_window: pages::SettingsWindow,
    pub panel: panel::Panel,
    pub undo: Option<(Snapshot, String)>,
}

thread_local! { static APP: std::cell::Cell<*mut App> = const { std::cell::Cell::new(std::ptr::null_mut()) }; }

/// shortcut: one mutable App reached from window procedures on the UI thread, because Windows re-enters them while
/// a handler runs and a RefCell would panic. Keep it single-threaded and never hold the reference across a modal loop.
pub fn app() -> &'static mut App { APP.with(|a| unsafe { &mut *a.get() }) }

/// Runs after the current message: anything that shows UI, starts a drag or changes the window being painted.
pub fn later(f: impl FnOnce() + 'static) {
    let a = app();
    a.later.push(Box::new(f));
    unsafe { let _ = PostMessageW(Some(a.main), WM_LATER, WPARAM(0), LPARAM(0)); }
}

pub fn open_url(url: &str) {
    unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL); }
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

pub fn login_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, w!("NachoCombo"), RRF_RT_REG_SZ, None, None, None).is_ok() }
}

pub fn set_login(on: bool) {
    unsafe {
        let mut k = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut k).is_err() { return; }
        if on {
            let exe = std::env::current_exe().unwrap_or_default();
            let v: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
            let _ = RegSetValueExW(k, w!("NachoCombo"), None, REG_SZ, Some(std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2)));
        } else {
            let _ = RegDeleteValueW(k, w!("NachoCombo"));
        }
        let _ = RegCloseKey(k);
    }
}

impl App {
    pub fn save(&self) { let _ = self.settings.save(&self.dir.join("settings.json")); }
    pub fn state(&self, id: ModuleId) -> State { self.states.get(&id).cloned().unwrap_or(State::Off) }

    pub fn update(&mut self, f: impl FnOnce(&mut AppSettings)) {
        let before = self.settings.clone();
        f(&mut self.settings);
        if before != self.settings { self.save(); self.changed(); }
    }

    /// Something shown may have changed: repaint what is open.
    pub fn changed(&mut self) {
        self.settings_window.invalidate();
        self.panel.invalidate();
        self.tray.refresh_main();
        self.write_state();
    }

    pub fn set_enabled(&mut self, id: ModuleId, on: bool) {
        self.settings.set_on(id, on);
        self.save();
        if on { self.start(id) } else { self.stop(id) }
        self.sync_hotkeys();
        self.changed();
    }

    pub fn start(&mut self, id: ModuleId) {
        if self.state(id) == State::Running { return; }
        let r = match id {
            ModuleId::TrayOrganizer => trayorg::start(),
            ModuleId::SystemMonitor => monitor::start(),
            ModuleId::WindowSnap => snap::start(),
            ModuleId::FileShelf => shelf::start(),
            ModuleId::CaptureFloat => capture::start(),
            ModuleId::FocusDim => dim::start(),
            ModuleId::KeepAwake => awake::start(),
            ModuleId::Clipboard => clipboard::start(),
            ModuleId::DeviceControl => devices::start(),
            ModuleId::DiskMap => crate::diskmap::start(),
            ModuleId::AppSwitch => appswitch::start(),
        };
        self.states.insert(id, match r { Ok(()) => State::Running, Err(e) => State::Failed(e) });
        hooks::sync();
        tray::resync_order();
    }

    /// The OFF contract: everything the gadget holds is released here.
    pub fn stop(&mut self, id: ModuleId) {
        match id {
            ModuleId::TrayOrganizer => trayorg::stop(),
            ModuleId::SystemMonitor => monitor::stop(),
            ModuleId::WindowSnap => snap::stop(),
            ModuleId::FileShelf => shelf::stop(),
            ModuleId::CaptureFloat => capture::stop(),
            ModuleId::FocusDim => dim::stop(),
            ModuleId::KeepAwake => awake::stop(),
            ModuleId::Clipboard => clipboard::stop(),
            ModuleId::DeviceControl => devices::stop(),
            ModuleId::DiskMap => crate::diskmap::stop(),
            ModuleId::AppSwitch => appswitch::stop(),
        }
        self.states.insert(id, State::Off);
        hooks::sync();
    }

    pub fn shutdown(&mut self) {
        for id in ModuleId::ALL { self.stop(id); }
        self.unregister_hotkeys();
        self.tray.remove_all();
    }

    fn unregister_hotkeys(&mut self) {
        for i in 0..self.hotkey_ids.len() { unsafe { let _ = UnregisterHotKey(Some(self.main), i as i32 + 1); } }
        self.hotkey_ids.clear();
    }

    /// Registers the shortcuts of running gadgets. One that another app holds is reported on the gadget's page.
    pub fn sync_hotkeys(&mut self) {
        self.unregister_hotkeys();
        self.hotkey_clashes.clear();
        let mut want: Vec<(String, Shortcut)> = vec![];
        for a in catalog::actions() {
            if self.state(a.module) == State::Running {
                if let Some(s) = self.settings.shortcut_for(&a) { want.push((a.id.clone(), s)); }
            }
        }
        for (m, s) in &self.settings.tool_shortcuts { want.push((format!("open.{}", m.key()), *s)); }
        // App shortcuts wait while one is being recorded, so the keys reach the settings window.
        let recording = self.settings_window.capture_for.as_deref().is_some_and(|c| c.starts_with("app:"));
        if self.state(ModuleId::AppSwitch) == State::Running && !recording {
            for h in &self.settings.options.app_shortcuts { if let Some(s) = h.shortcut { want.push((crate::core::appswitch::app_id(&h.path), s)); } }
        }
        for (id, s) in want {
            let hid = self.hotkey_ids.len() as i32 + 1;
            let ok = unsafe { RegisterHotKey(Some(self.main), hid, HOT_KEY_MODIFIERS(s.mods) | MOD_NOREPEAT, s.key) }.is_ok();
            if !ok { self.hotkey_clashes.insert(id.clone()); }
            self.hotkey_ids.push(id);
        }
    }

    /// Would `s` register right now? Tried for real, because only Windows knows what other apps hold.
    pub fn hotkey_free(&self, s: Shortcut) -> bool {
        unsafe {
            if RegisterHotKey(Some(self.main), 9000, HOT_KEY_MODIFIERS(s.mods), s.key).is_ok() {
                let _ = UnregisterHotKey(Some(self.main), 9000);
                return true;
            }
        }
        false
    }

    pub fn run_action(&mut self, id: &str) {
        if let Some(a) = id.strip_prefix("snap.") { if let Some(act) = SnapAction::from_id(a) { snap::run(act); } return; }
        if id.starts_with("app:") {
            if let Some(h) = self.settings.options.app_shortcuts.iter().find(|h| crate::core::appswitch::app_id(&h.path) == id).cloned() { appswitch::open_or_toggle(&h.path); }
            return;
        }
        if let Some(m) = id.strip_prefix("open.") { if let Some(m) = ModuleId::from_key(m) { panel::open_tool(m); } return; }
        if let Some(m) = id.strip_prefix("monitor.") {
            if let Some(i) = MonitorItem::ALL.into_iter().find(|i| i.name().to_lowercase() == m) { monitor::open_panel(i); }
            return;
        }
        match id {
            "tray.toggle" => trayorg::toggle(),
            "shelf.toggle" => shelf::toggle(),
            "shelf.paste" => shelf::paste_clipboard(),
            "capture.area" => capture::select(capture::Mode::Area),
            "capture.window" => capture::select(capture::Mode::Window),
            "capture.screen" => capture::capture_screen(),
            "capture.previous" => capture::capture_previous(),
            "capture.text" => capture::select(capture::Mode::Text),
            "capture.lock" => capture::toggle_lock(),
            "capture.hide" => capture::toggle_hide(),
            "capture.closeAll" => capture::close_all(),
            "capture.color" => capture::select(capture::Mode::Color),
            "capture.live" => capture::select(capture::Mode::Live),
            "dim.toggle" => dim::toggle_pause(),
            "awake.toggle" => if self.state(ModuleId::KeepAwake) == State::Running { awake::toggle() },
            "clipboard.show" => clipboard::show(),
            "clipboard.plain" => clipboard::paste_last_plain(),
            "device.micMute" => devices::toggle_mic(),
            "device.nextOutput" => devices::next_output(),
            "device.brighter" => devices::step_all(true),
            "device.dimmer" => devices::step_all(false),
            "diskmap.open" => if self.state(ModuleId::DiskMap) == State::Running { crate::diskmap::open() },
            "device.theme" => if self.state(ModuleId::DeviceControl) == State::Running { devices::toggle_theme() },
            "window.onTop" => snap::toggle_on_top(),
            "appswitch.next" => appswitch::cycle(true),
            "appswitch.previous" => appswitch::cycle(false),
            "appswitch.hideOthers" => appswitch::hide_others(),
            _ => {}
        }
    }

    /// Orders a combo: tunes its options, switches off what it leaves out, switches on what it needs. `Undo` puts it back.
    pub fn order(&mut self, c: &Combo) {
        self.undo = Some((combos::snapshot(&self.settings), c.name.clone()));
        let plan = combos::plan(c, &self.settings);
        c.tuning.apply(&mut self.settings);
        self.settings.combo = Some(c.id.clone());
        self.save();
        for id in plan.turns_off { self.set_enabled(id, false); }
        for id in plan.turns_on { self.set_enabled(id, true); }
        self.apply_options();
        unsafe { SetTimer(Some(self.main), TIMER_COMBO_UNDO, 300_000, None); }
    }

    pub fn undo_order(&mut self) {
        let Some((snap, _)) = self.undo.take() else { return };
        self.settings.options = snap.options;
        self.settings.dim_intensity = snap.dim;
        self.settings.monitor_items = snap.monitor;
        self.settings.combo = snap.combo;
        self.save();
        for (id, on) in snap.enabled { if self.settings.is_on(id) != on { self.set_enabled(id, on); } }
        self.apply_options();
    }

    /// Lets running gadgets pick up changed options.
    pub fn apply_options(&mut self) {
        dim::options_changed();
        monitor::options_changed();
        hooks::sync();
        self.changed();
    }

    pub fn write_state(&self) {
        let mut modules = serde_json::Map::new();
        for id in ModuleId::ALL {
            modules.insert(id.key().into(), serde_json::Value::String(match self.state(id) { State::Off => "off".into(), State::Running => "running".into(), State::Failed(e) => format!("failed: {e}") }));
        }
        let v = serde_json::json!({
            "modules": modules,
            "combo": combos::current(&self.settings).map(|c| c.id),
            "hotkeys": self.hotkey_ids.len(),
            "hotkeyClashes": self.hotkey_clashes.iter().collect::<Vec<_>>(),
            "trayIcons": self.tray.count(),
            "hooks": hooks::active(),
            "snap": self.snap.diagnostics(),
            "dim": self.dim.diagnostics(),
            "shelf": self.shelf.diagnostics(),
            "capture": self.capture.diagnostics(),
            "monitor": self.monitor.diagnostics(),
            "awake": self.awake.diagnostics(),
            "clipboard": self.clipboard.diagnostics(),
            "devices": self.devices.diagnostics(),
            "appswitch": self.appswitch.diagnostics(),
            "appShortcuts": self.hotkey_ids.iter().filter(|i| i.starts_with("app:")).count(),
            "appShortcutsFailed": self.hotkey_clashes.iter().filter(|i| i.starts_with("app:")).collect::<Vec<_>>(),
            "diskmap": crate::diskmap::diagnostics(),
            "organizer": self.organizer.diagnostics(),
            "settingsWindow": self.settings_window.is_open(),
            "panel": self.panel.is_open(),
        });
        let _ = std::fs::write(self.dir.join("state.json"), serde_json::to_vec_pretty(&v).unwrap_or_default());
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if APP.with(|a| a.get().is_null()) { return DefWindowProcW(hwnd, msg, wp, lp); }
    let a = app();
    match msg {
        WM_LATER => {
            for f in std::mem::take(&mut a.later) { f(); }
            LRESULT(0)
        }
        WM_HOTKEY => {
            if (appswitch::CANCEL_ID as usize..appswitch::CANCEL_ID as usize + 3).contains(&wp.0) { appswitch::cancel(); }
            else if let Some(id) = a.hotkey_ids.get(wp.0.wrapping_sub(1)).cloned() { a.run_action(&id); }
            LRESULT(0)
        }
        WM_TRAY => { tray::on_message(wp, lp); LRESULT(0) }
        WM_SHAKE => { shelf::on_shake(); LRESULT(0) }
        WM_ANYDRAG => { shelf::on_any_drag(); LRESULT(0) }
        WM_WAKE => { pages::open(None); LRESULT(0) }
        WM_OCR_DONE => { capture::on_text_done(lp.0 as *mut String); LRESULT(0) }
        WM_PUBLIC_IP => { monitor::on_public_ip(lp.0 as *mut String); LRESULT(0) }
        WM_DEVICES => { devices::on_change(wp.0 == 1); LRESULT(0) }
        WM_PEEK => { crate::peek::on_space(); LRESULT(0) }
        crate::diskmap::WM_DISKMAP => { crate::diskmap::on_message(wp, lp); LRESULT(0) }
        clipboard::WM_CLIPBOARDUPDATE => { clipboard::on_update(); LRESULT(0) }
        WM_TIMER => {
            match wp.0 {
                TIMER_MONITOR => monitor::tick(),
                TIMER_TRAY => trayorg::on_timer(),
                TIMER_DIM => dim::refresh(),
                TIMER_FADE => dim::fade_tick(),
                TIMER_DRAG => snap::drag_tick(),
                TIMER_AWAKE => awake::tick(),
                TIMER_PASTE => clipboard::on_paste_timer(),
                TIMER_THEME => devices::theme_tick(),
                TIMER_LAYOUT => snap::layout_tick(),
                TIMER_APPSWITCH => appswitch::on_timer(),
                TIMER_COMBO_UNDO => { let _ = KillTimer(Some(hwnd), TIMER_COMBO_UNDO); a.undo = None; a.changed(); }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => { dim::displays_changed(); snap::displays_changed(); a.changed(); DefWindowProcW(hwnd, msg, wp, lp) }
        WM_COPYDATA => {
            if !a.debug_ctl { return LRESULT(0); }
            let cd = &*(lp.0 as *const COPYDATASTRUCT);
            let bytes = std::slice::from_raw_parts(cd.lpData as *const u8, cd.cbData as usize);
            let text = String::from_utf8_lossy(bytes).to_string();
            crate::ctl::run(&text.split('\u{1f}').map(String::from).collect::<Vec<_>>());
            a.write_state();
            LRESULT(1)
        }
        WM_ENDSESSION | WM_DESTROY => { a.shutdown(); PostQuitMessage(0); LRESULT(0) }
        WM_CLOSE => { let _ = DestroyWindow(hwnd); LRESULT(0) }
        _ if msg == tray::taskbar_created() => { a.tray.recreate(); LRESULT(0) }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn quit() { later(|| unsafe { let _ = DestroyWindow(app().main); }); }

/// Starts the app and runs its message loop; returns when it quits.
pub fn run(dir: PathBuf, debug_ctl: bool) {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(main_proc), hInstance: hinst.into(), lpszClassName: MAIN_CLASS, ..Default::default() };
        RegisterClassExW(&wc);
        let main = CreateWindowExW(WS_EX_TOOLWINDOW, MAIN_CLASS, w!("Nacho Combo"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None).expect("main window");
        let settings = AppSettings::load(&dir.join("settings.json"));
        crate::i18n::set_language(settings.language.as_deref());
        let boxed = Box::leak(Box::new(App { settings, dir, states: BTreeMap::new(), main, debug_ctl, later: vec![], hotkey_ids: vec![], hotkey_clashes: BTreeSet::new(),
            tray: tray::Tray::default(), snap: Default::default(), dim: Default::default(), organizer: Default::default(), shelf: Default::default(), capture: Default::default(),
            monitor: Default::default(), awake: Default::default(), clipboard: Default::default(), devices: Default::default(), appswitch: Default::default(), settings_window: Default::default(), panel: Default::default(), undo: None }));
        APP.with(|c| c.set(boxed));
        let a = app();
        crate::window::register_class();
        tray::create_main();
        for id in ModuleId::ALL { if a.settings.is_on(id) { a.start(id); } }
        a.sync_hotkeys();
        a.write_state();
        let failed = ModuleId::ALL.iter().any(|m| matches!(a.state(*m), State::Failed(_)));
        let open = !a.settings.welcomed || match a.settings.launch_window { LaunchWindow::Always => true, LaunchWindow::WhenNeeded => failed, LaunchWindow::Never => false };
        if open { later(|| pages::open(None)); }
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}
