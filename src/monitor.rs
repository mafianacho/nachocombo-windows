//! System Monitor: samples every 2 s while on, keeps 24 h of history in memory, draws small numbers into the tray.
use crate::app::{app, later, State, TIMER_MONITOR, WM_PUBLIC_IP};
use crate::core::alerts::{self, Alerts};
use crate::core::format;
use crate::core::series::Series;
use crate::core::settings::*;
use crate::gfx::*;
use crate::monitor_data::*;
use crate::monitor_panels;
use crate::tray::{self, icon_from_pixels, icon_size, UID_MONITOR};
use crate::core::layout::Rect;
use std::collections::HashMap;
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Default)]
pub struct History {
    pub cpu_user: Series, pub cpu_system: Series, pub cpu_total: Series, pub mem: Series, pub commit: Series, pub read: Series, pub write: Series,
    pub down: Series, pub up: Series, pub gpu: Series, pub temp: Series, pub battery: Series, pub queue: Series,
}

#[derive(Default)]
pub struct Monitor {
    pub sampler: Option<Sampler>,
    pub last: Sample,
    pub history: History,
    pub alerts: Alerts,
    pub processes: Vec<Proc>,
    pub adapters: Vec<Adapter>,
    pub public_ip: Option<String>,
    pub public_ip_state: Option<String>,
    pub ranges: HashMap<String, u32>,
    pub total_read: f64,
    pub total_write: f64,
    pub ticks: u64,
    /// Fixed sample numbers for pictures: the data in `last` is then not live.
    pub sample_mode: bool,
    pub open: Option<MonitorItem>,
    pub panel: Option<isize>,
    shown: Vec<MonitorItem>,
    last_text: HashMap<MonitorItem, String>,
}

impl Monitor {
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "sampling": self.sampler.is_some(), "ticks": self.ticks, "trayItems": self.shown.len(), "alerts": self.alerts.messages(), "panel": self.open.map(|o| o.name()), "cpu": self.last.cpu_user + self.last.cpu_system, "memLoad": self.last.mem_load, "cores": self.last.cores.len(), "gpu": self.last.gpu, "temps": self.last.temps.len(), "battery": self.last.battery.as_ref().map(|b| b.percent), "volumes": self.last.volumes.len() })
    }
    pub fn range(&self, key: &str) -> u32 { self.ranges.get(key).copied().unwrap_or(600) }
}

pub fn start() -> std::result::Result<(), String> {
    let a = app();
    a.monitor.sampler = Some(Sampler::new());
    a.monitor.alerts = Alerts::new(a.settings.options.cpu_alert_percent as f64);
    unsafe { SetTimer(Some(a.main), TIMER_MONITOR, 2000, None); }
    tick();
    if a.settings.options.public_ip_auto { request_public_ip(); }
    Ok(())
}

pub fn stop() {
    let a = app();
    unsafe { let _ = KillTimer(Some(a.main), TIMER_MONITOR); }
    close_panel();
    let m = &mut a.monitor;
    m.sampler = None;
    m.history = History::default();
    m.alerts = Alerts::default();
    m.processes.clear();
    m.public_ip = None;
    (m.total_read, m.total_write) = (0.0, 0.0);
    for i in std::mem::take(&mut m.shown) { a.tray.remove(UID_MONITOR + index(i)); }
    m.last_text.clear();
}

pub fn index(i: MonitorItem) -> u32 { MonitorItem::ALL.iter().position(|x| *x == i).unwrap_or(0) as u32 }

pub fn tick() {
    let a = app();
    let m = &mut a.monitor;
    let Some(s) = m.sampler.as_mut() else { return };
    let v = s.sample();
    m.ticks += 1;
    let h = &mut m.history;
    h.cpu_user.push(v.cpu_user);
    h.cpu_system.push(v.cpu_system);
    h.cpu_total.push(v.cpu_user + v.cpu_system);
    h.mem.push(v.mem_load);
    h.commit.push(v.commit_used / v.commit_limit.max(1.0) * 100.0);
    h.read.push(v.disk_read);
    h.write.push(v.disk_write);
    h.down.push(v.net_down);
    h.up.push(v.net_up);
    h.queue.push(v.queue.unwrap_or(0.0));
    if let Some(g) = v.gpu { h.gpu.push(g); }
    if let Some(t) = hottest(&v) { h.temp.push(t); }
    if let Some(b) = &v.battery { h.battery.push(b.percent); }
    m.total_read += v.disk_read * 2.0;
    m.total_write += v.disk_write * 2.0;
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let disk = v.volumes.iter().find(|d| d.root.starts_with(&system)).map(|d| (1.0 - d.free / d.total.max(1.0)) * 100.0);
    m.alerts.cpu_threshold = a.settings.options.cpu_alert_percent as f64;
    let started = m.alerts.update(alerts::Input { cpu: Some(v.cpu_user + v.cpu_system), pressure: alerts::pressure_level(v.mem_load), disk_used: disk, temperature: hottest(&v), battery_percent: v.battery.as_ref().map(|b| b.percent), on_ac: v.battery.as_ref().is_some_and(|b| b.on_ac) });
    m.last = v;
    if a.settings.options.alert_notifications {
        for k in started { let text = a.monitor.alerts.message(k); a.tray.balloon("System Monitor", &text); }
    }
    if a.monitor.panel.is_some() {
        let cores = a.monitor.last.cores.len();
        if let Some(s) = a.monitor.sampler.as_mut() { a.monitor.processes = s.processes(cores); }
        if a.monitor.open == Some(MonitorItem::Network) && a.monitor.ticks % 5 == 0 { a.monitor.adapters = adapters(); }
        monitor_panels::invalidate();
    }
    sync_tray();
    if a.settings_window.is_open() { a.settings_window.invalidate(); }
    if a.panel.is_open() { a.panel.invalidate(); }
}

pub fn hottest(v: &Sample) -> Option<f64> { v.temps.iter().map(|t| t.1).fold(None, |m, t| Some(m.map_or(t, |x: f64| x.max(t)))) }

/// One tray icon per chosen metric. Each redraws only when its text changes.
pub fn sync_tray() {
    let a = app();
    let want: Vec<MonitorItem> = a.settings.monitor_items.iter().copied().filter(|i| *i != MonitorItem::Battery || a.monitor.last.battery.is_some()).collect();
    for i in a.monitor.shown.clone() { if !want.contains(&i) { a.tray.remove(UID_MONITOR + index(i)); a.monitor.last_text.remove(&i); } }
    a.monitor.shown = want.clone();
    for i in want {
        let (top, bottom, frac, tip) = tray_face(i, &a.monitor.last);
        let key = format!("{top}|{bottom}|{:.0}", frac.unwrap_or(0.0) * 16.0);
        if a.monitor.last_text.get(&i) == Some(&key) { continue; }
        a.monitor.last_text.insert(i, key);
        a.tray.set(UID_MONITOR + index(i), draw_icon(&top, &bottom, frac), &tip, true);
    }
}

/// (first line, second line, fill level, tooltip)
fn tray_face(i: MonitorItem, v: &Sample) -> (String, String, Option<f64>, String) {
    match i {
        MonitorItem::Cpu => { let c = v.cpu_user + v.cpu_system; (format!("{c:.0}"), String::new(), Some(c / 100.0), format!("CPU {c:.0}%")) }
        MonitorItem::Memory => (format!("{:.0}", v.mem_load), String::new(), Some(v.mem_load / 100.0), format!("Memory {:.0}%  ({} of {})", v.mem_load, format::bytes(v.mem_used), format::bytes(v.mem_total))),
        MonitorItem::Disk => (format::compact_rate(v.disk_read), format::compact_rate(v.disk_write), None, format!("Disk  read {}  write {}", format::rate(v.disk_read), format::rate(v.disk_write))),
        MonitorItem::Network => (format::compact_rate(v.net_down), format::compact_rate(v.net_up), None, format!("Network  ↓ {}  ↑ {}", format::rate(v.net_down), format::rate(v.net_up))),
        MonitorItem::Sensors => match hottest(v) {
            Some(t) => (format!("{t:.0}°"), String::new(), None, format!("Hottest sensor {t:.0} °C")),
            None => match v.gpu { Some(g) => (format!("{g:.0}"), "GPU".into(), None, format!("GPU {g:.0}%. This PC's firmware reports no temperature.")), None => ("–".into(), String::new(), None, "No sensor readings".into()) },
        },
        MonitorItem::Battery => { let b = v.battery.clone().unwrap_or_default(); (format!("{:.0}", b.percent), String::new(), Some(b.percent / 100.0), format!("Battery {:.0}%  {}", b.percent, b.state())) }
    }
}

fn draw_icon(top: &str, bottom: &str, frac: Option<f64>) -> HICON {
    let n = icon_size();
    let s = n as f64;
    let ink = if tray::taskbar_is_light() { rgb(0x24180D) } else { rgb(0xFFFFFF) };
    let px = render_offscreen(n as u32, n as u32, 96.0, None, |c| {
        if let Some(f) = frac {
            let bar = (s * 0.14).max(2.0);
            c.fill(Rect::new(0.0, s - bar, s, bar), ink.alpha(0.25));
            c.fill(Rect::new(0.0, s - bar, s * f.clamp(0.0, 1.0), bar), if f > 0.9 { rgb(0xF38069) } else { NACHO });
            fit(c, top, Rect::new(0.0, 0.0, s, s - bar), ink);
        } else if bottom.is_empty() {
            fit(c, top, Rect::new(0.0, 0.0, s, s), ink);
        } else {
            fit(c, top, Rect::new(0.0, 0.0, s, s / 2.0), ink);
            fit(c, bottom, Rect::new(0.0, s / 2.0, s, s / 2.0), ink);
        }
    }).unwrap_or_default();
    icon_from_pixels(&px, n, n)
}

/// The largest size at which `t` fits.
fn fit(c: &Canvas, t: &str, r: Rect, ink: Color) {
    let mut size = (r.h * 0.95) as u32;
    while size > 6 && c.measure(t, bold(size), 500.0).0 > r.w + 1.0 { size -= 1; }
    c.line_text(t, bold(size), r, ink, Align::Center);
}

pub fn redraw_tray() { app().monitor.last_text.clear(); if app().monitor.sampler.is_some() { sync_tray(); } }

pub fn options_changed() {
    let a = app();
    if a.state(ModuleId::SystemMonitor) == State::Running { sync_tray(); }
}

pub fn request_public_ip() {
    let a = app();
    a.monitor.public_ip_state = Some("Looking up…".into());
    let main = a.main.0 as isize;
    std::thread::spawn(move || {
        let r = lookup_public_ip().unwrap_or_default();
        let b = Box::into_raw(Box::new(r));
        unsafe { let _ = PostMessageW(Some(HWND(main as *mut _)), WM_PUBLIC_IP, WPARAM(0), LPARAM(b as isize)); }
    });
    a.changed();
}

pub fn on_public_ip(p: *mut String) {
    let s = unsafe { *Box::from_raw(p) };
    let m = &mut app().monitor;
    if s.is_empty() { m.public_ip_state = Some("Could not be looked up".into()); } else { m.public_ip = Some(s); m.public_ip_state = None; }
    monitor_panels::invalidate();
    app().changed();
}

pub fn open_panel(i: MonitorItem) {
    if app().state(ModuleId::SystemMonitor) != State::Running { return; }
    monitor_panels::open(i);
}

pub fn toggle_panel(i: MonitorItem) {
    if app().monitor.open == Some(i) && app().monitor.panel.is_some() { close_panel(); } else { later(move || open_panel(i)); }
}

pub fn close_panel() {
    let m = &mut app().monitor;
    if let Some(id) = m.panel.take() { unsafe { let _ = DestroyWindow(HWND(id as *mut _)); } }
    m.open = None;
}
