//! Keep Awake: asks Windows to stay awake, by hand, for a while, or while chosen programs run.
use crate::app::{app, later, TIMER_AWAKE};
use crate::core::layout::Rect;
use crate::gfx::*;
use crate::tray::{icon_from_pixels, icon_size, taskbar_is_light};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Power::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const UID: u32 = 30;

#[derive(Default)]
pub struct Awake {
    /// Turned on by hand (or for a while).
    pub manual: bool,
    pub until: Option<std::time::Instant>,
    /// A chosen program is running.
    pub by_app: Option<String>,
    /// Battery guard let the PC sleep.
    pub guarded: bool,
    pub requests: u32,
    held: bool,
}

impl Awake {
    pub fn active(&self) -> bool { (self.manual || self.by_app.is_some()) && !self.guarded }
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "manual": self.manual, "byApp": self.by_app, "active": self.active(), "held": self.held, "minutesLeft": self.minutes_left(), "requests": self.requests })
    }
    pub fn minutes_left(&self) -> Option<u64> { self.until.map(|u| u.saturating_duration_since(std::time::Instant::now()).as_secs().div_ceil(60)) }
}

pub fn start() -> std::result::Result<(), String> {
    let a = app();
    unsafe { SetTimer(Some(a.main), TIMER_AWAKE, 15_000, None); }
    tick();
    Ok(())
}

pub fn stop() {
    let a = app();
    unsafe { let _ = KillTimer(Some(a.main), TIMER_AWAKE); }
    a.awake = Awake::default();
    apply();
    a.tray.remove(UID);
}

/// Turns it on (for `minutes`, or until turned off with None) or off.
pub fn set(on: bool, minutes: Option<u64>) {
    let a = app();
    a.awake.manual = on;
    a.awake.until = if on { minutes.map(|m| std::time::Instant::now() + std::time::Duration::from_secs(m * 60)) } else { None };
    tick();
}

pub fn toggle() { let on = app().awake.manual; set(!on, None); }

fn running_app(names: &[String]) -> Option<String> {
    if names.is_empty() { return None; }
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|c| *c == 0).unwrap_or(0)]);
                if let Some(hit) = names.iter().find(|w| w.eq_ignore_ascii_case(&n)) { found = Some(hit.clone()); break; }
                if Process32NextW(snap, &mut e).is_err() { break; }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// Checks the clock, the chosen programs and the battery, then holds or releases the request.
pub fn tick() {
    let a = app();
    if a.state(crate::core::settings::ModuleId::KeepAwake) != crate::app::State::Running { return; }
    if a.awake.until.is_some_and(|u| std::time::Instant::now() >= u) { a.awake.manual = false; a.awake.until = None; }
    a.awake.by_app = running_app(&a.settings.options.awake_apps);
    let guard = a.settings.options.awake_battery_guard as f64;
    let mut st = SYSTEM_POWER_STATUS::default();
    let low = unsafe { GetSystemPowerStatus(&mut st).is_ok() } && st.ACLineStatus == 0 && st.BatteryLifePercent != 255 && guard > 0.0 && (st.BatteryLifePercent as f64) < guard;
    a.awake.guarded = low;
    apply();
    draw_tray();
    a.changed();
}

/// The request is per thread: the UI thread holds it, which is the thread that lives as long as the app.
fn apply() {
    let a = app();
    let want = a.awake.active();
    let display = a.settings.options.awake_display;
    unsafe {
        if want {
            let mut f = ES_CONTINUOUS | ES_SYSTEM_REQUIRED;
            if display { f |= ES_DISPLAY_REQUIRED; }
            SetThreadExecutionState(f);
            if !a.awake.held { a.awake.requests += 1; }
        } else {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
    a.awake.held = want;
}

fn draw_tray() {
    let a = app();
    let on = a.awake.active();
    let n = icon_size();
    let s = n as f64;
    let ink = if taskbar_is_light() { rgb(0x24180D) } else { rgb(0xFFFFFF) };
    let px = render_offscreen(n as u32, n as u32, 96.0, None, |c| {
        let u = s / 16.0;
        let cup = Rect::new(2.0 * u, 6.0 * u, 9.0 * u, 8.0 * u);
        if on { c.round(cup, 1.5 * u, NACHO); } else { c.stroke_round(cup, 1.5 * u, ink, 1.4 * u); }
        c.ring(12.0 * u, 9.5 * u, 2.0 * u, if on { NACHO } else { ink }, 1.4 * u);
        if on { for i in 0..2 { let x = (4.5 + i as f64 * 3.0) * u; c.line(x, 1.5 * u, x, 4.5 * u, NACHO, 1.2 * u); } }
    }).unwrap_or_default();
    let tip = if !on { "Keep Awake: off".to_string() } else if let Some(app_name) = &a.awake.by_app { format!("Keep Awake: on while {app_name} runs") }
        else if let Some(m) = a.awake.minutes_left() { format!("Keep Awake: {m} min left") } else { "Keep Awake: on until you turn it off".to_string() };
    a.tray.set(UID, icon_from_pixels(&px, n, n), &tip, true);
}

pub const DURATIONS: [(u64, &str); 5] = [(15, "15 Minutes"), (30, "30 Minutes"), (60, "1 Hour"), (120, "2 Hours"), (300, "5 Hours")];

pub fn menu() {
    unsafe {
        let a = app();
        let Ok(m) = CreatePopupMenu() else { return };
        let on = a.awake.manual;
        let first = HSTRING::from(crate::i18n::t(if on { "Let It Sleep" } else { "Keep Awake Until Turned Off" }).as_ref());
        let _ = AppendMenuW(m, MF_STRING, 1, &first);
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        for (i, d) in DURATIONS.iter().enumerate() { let _ = AppendMenuW(m, MF_STRING, 10 + i, &HSTRING::from(crate::i18n::t(&format!("For {}", d.1)).as_ref())); }
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(m, MF_STRING | if a.settings.options.awake_display { MF_CHECKED } else { MF_UNCHECKED }, 2, &HSTRING::from(crate::i18n::t("Keep the Display On Too").as_ref()));
        let _ = AppendMenuW(m, MF_STRING, 3, &HSTRING::from(crate::i18n::t("Settings…").as_ref()));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(a.main);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, a.main, None).0 as usize;
        let _ = DestroyMenu(m);
        match cmd {
            1 => set(!on, None),
            2 => { a.update(|s| s.options.awake_display = !s.options.awake_display); tick(); }
            3 => later(|| crate::pages::open(Some(crate::core::settings::ModuleId::KeepAwake))),
            n if n >= 10 => if let Some(d) = DURATIONS.get(n - 10) { set(true, Some(d.0)); },
            _ => {}
        }
    }
}
