//! The detail panels that open from System Monitor's tray icons.
use crate::app::{app, later};
use crate::core::format;
use crate::core::layout::Rect;
use crate::core::series::{range_name, Series, RANGES};
use crate::core::settings::*;
use crate::gfx::*;
use crate::monitor;
use crate::monitor_data::*;
use crate::ui::*;
use crate::window::{UiWindow, WindowCtx};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const W: f64 = 340.0;

pub fn invalidate() {
    if let Some(id) = app().monitor.panel { unsafe { let _ = InvalidateRect(Some(HWND(id as *mut _)), None, false); } }
}

pub fn open(item: MonitorItem) {
    monitor::close_panel();
    let a = app();
    a.monitor.open = Some(item);
    a.monitor.adapters = adapters();
    let cores = a.monitor.last.cores.len();
    if let Some(s) = a.monitor.sampler.as_mut() { a.monitor.processes = s.processes(cores); }
    let h = 560.0;
    let win = UiWindow::create(item.name(), WS_POPUP, WS_EX_TOOLWINDOW | WS_EX_TOPMOST, 0, 0, W, h, Box::new(move |ui, ctx| draw(ui, ctx, item)));
    win.close_on_deactivate = true;
    win.on_close = Some(Box::new(|| { let m = &mut app().monitor; m.panel = None; m.open = None; }));
    let hw = win.hwnd;
    a.monitor.panel = Some(hw.0 as isize);
    place_near_tray(hw);
    win.show();
}

/// Above the taskbar, under the pointer's corner of the screen, never off it.
pub fn place_near_tray(h: HWND) {
    unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST), &mut mi);
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let (w, hh) = (r.right - r.left, r.bottom - r.top);
        let wk = mi.rcWork;
        let m = (8.0 * crate::window::dpi_scale(h)) as i32;
        let x = (p.x - w / 2).clamp(wk.left + m, wk.right - w - m);
        let y = if p.y > (wk.top + wk.bottom) / 2 { wk.bottom - hh - m } else { wk.top + m };
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

fn draw(ui: &mut Ui, ctx: &mut WindowCtx, item: MonitorItem) {
    for k in ui.st.keys.clone() { if k.vk == 0x1B { ctx.close = true; } }
    let th = ui.th;
    ui.c.fill(Rect::new(0.0, 0.0, ui.w, 44.0), th.paper2);
    ui.c.line_text(&item.name().to_uppercase(), sign(18), Rect::new(16.0, 0.0, 200.0, 44.0), th.ink, Align::Left);
    let settings = Rect::new(ui.w - 40.0, 8.0, 28.0, 28.0);
    let id = ui.id("gear");
    let r = ui.hit(id, settings, false);
    if r.hover { ui.c.round(settings, 6.0, th.line); }
    ui.c.glyph(0xE713, 14, settings, th.ink2);
    if r.clicked { ctx.close = true; later(|| crate::pages::open(Some(ModuleId::SystemMonitor))); }
    let view = Rect::new(0.0, 44.0, ui.w, ui.h - 44.0);
    ui.scroll("body", view, |ui, y| match item {
        MonitorItem::Cpu => cpu(ui, y),
        MonitorItem::Memory => memory(ui, y),
        MonitorItem::Disk => disk(ui, y),
        MonitorItem::Network => network(ui, y),
        MonitorItem::Sensors => sensors(ui, y),
        MonitorItem::Battery => battery(ui, y),
    });
}

const PAD: f64 = 12.0;

/// A chart card with its range picker. Each series is (values, colour); `max` None scales to the data.
fn chart_card(ui: &mut Ui, key: &str, title: &str, value: &str, series: &[(&Series, Color)], max: Option<f64>, y: f64) -> f64 {
    let th = ui.th;
    let w = ui.w - PAD * 2.0;
    let card = Rect::new(PAD, y, w, 168.0);
    ui.card(card);
    ui.eyebrow(title, card.x + 12.0, y + 10.0, w * 0.5, th.ink2);
    ui.c.line_text(value, semi(13), Rect::new(card.x + 12.0, y + 10.0, w - 24.0, 16.0), th.ink, Align::Right);
    let range = app().monitor.range(key);
    let plot = Rect::new(card.x + 12.0, y + 32.0, w - 24.0, 92.0);
    draw_chart(ui, plot, series, range, max);
    let opts: Vec<String> = RANGES.iter().map(|r| range_name(*r).to_string()).collect();
    let cur = RANGES.iter().position(|r| *r == range);
    if let Some(i) = ui.segments(&format!("{key}-range"), &opts, cur, Rect::new(card.x + 12.0, y + 132.0, w - 24.0, 26.0), false) {
        app().monitor.ranges.insert(key.to_string(), RANGES[i]);
        ui.st.again = true;
    }
    y + 176.0
}

pub fn draw_chart(ui: &mut Ui, plot: Rect, series: &[(&Series, Color)], range: u32, max: Option<f64>) {
    let th = ui.th;
    ui.c.round(plot, 4.0, th.paper);
    for f in [0.25, 0.5, 0.75] { ui.c.line(plot.x, plot.y + plot.h * f, plot.right(), plot.y + plot.h * f, th.line, 1.0); }
    let vals: Vec<(Vec<f64>, Color)> = series.iter().map(|(s, c)| (s.values(range), *c)).collect();
    let top = max.unwrap_or_else(|| vals.iter().flat_map(|v| v.0.iter().copied()).fold(1.0, f64::max) * 1.1);
    let cap = (range / if range <= 3600 { 2 } else { 60 }).max(1) as f64;
    for (v, c) in &vals {
        if v.len() < 2 { continue; }
        let pts: Vec<(f64, f64)> = v.iter().enumerate().map(|(i, x)| {
            let px = plot.right() - (v.len() - 1 - i) as f64 / (cap - 1.0).max(1.0) * plot.w;
            (px, plot.bottom() - (x / top).clamp(0.0, 1.0) * plot.h)
        }).collect();
        ui.c.chart(&pts, plot.bottom(), *c, 1.5, Some(c.alpha(0.18)));
    }
}

fn row(ui: &mut Ui, label: &str, value: &str, y: f64) -> f64 {
    let th = ui.th;
    ui.c.line_text(label, sans(13), Rect::new(PAD + 12.0, y, ui.w * 0.55, 24.0), th.ink2, Align::Left);
    ui.c.line_text(value, medium(13), Rect::new(ui.w * 0.35, y, ui.w * 0.65 - PAD - 12.0, 24.0), th.ink, Align::Right);
    y + 24.0
}

fn rows_card(ui: &mut Ui, title: &str, rows: &[(String, String)], y: f64) -> f64 {
    let h = 34.0 + rows.len() as f64 * 24.0 + 8.0;
    ui.card(Rect::new(PAD, y, ui.w - PAD * 2.0, h));
    ui.eyebrow(title, PAD + 12.0, y + 10.0, ui.w - 48.0, ui.th.ink2);
    let mut ry = y + 32.0;
    for (l, v) in rows { ry = row(ui, l, v, ry); }
    y + h + 8.0
}

fn process_card(ui: &mut Ui, title: &str, f: impl Fn(&Proc) -> f64, text: impl Fn(&Proc) -> String, y: f64) -> f64 {
    let mut v: Vec<Proc> = app().monitor.processes.clone();
    v.sort_by(|a, b| f(b).total_cmp(&f(a)));
    let rows: Vec<(String, String)> = v.iter().take(6).map(|p| (p.name.clone(), text(p))).collect();
    rows_card(ui, title, &rows, y)
}

fn ring(ui: &Ui, cx: f64, cy: f64, r: f64, frac: f64, c: Color, label: &str, caption: &str) {
    ui.c.ring(cx, cy, r, ui.th.line, 7.0);
    ui.c.arc(cx, cy, r, frac, c, 7.0);
    ui.c.line_text(label, semi(14), Rect::new(cx - r, cy - 12.0, r * 2.0, 24.0), ui.th.ink, Align::Center);
    ui.c.line_text(caption, sans(11), Rect::new(cx - r - 16.0, cy + r + 7.0, r * 2.0 + 32.0, 16.0), ui.th.ink2, Align::Center);
}

fn cpu(ui: &mut Ui, y: f64) -> f64 {
    let th = ui.th;
    let m = &app().monitor;
    let v = m.last.clone();
    let total = v.cpu_user + v.cpu_system;
    let mut y = chart_card(ui, "cpu", "History", &format!("{total:.0}%  (user {:.0} · system {:.0})", v.cpu_user, v.cpu_system), &[(&m.history.cpu_total, th.salsa), (&m.history.cpu_user, rgb(0x4A90D9))], Some(100.0), y + 8.0);
    if !v.cores.is_empty() {
        let per_row = 8usize.min(v.cores.len());
        let rows = v.cores.len().div_ceil(per_row);
        let h = 36.0 + rows as f64 * 40.0;
        ui.card(Rect::new(PAD, y, ui.w - PAD * 2.0, h));
        let has_e = v.efficiency.iter().any(|e| *e);
        ui.eyebrow(if has_e { "Cores · efficiency pink · performance blue" } else { "Cores" }, PAD + 12.0, y + 10.0, ui.w - 48.0, th.ink2);
        let cw = (ui.w - PAD * 2.0 - 24.0) / per_row as f64;
        for (i, c) in v.cores.iter().enumerate() {
            let x = PAD + 12.0 + cw * (i % per_row) as f64;
            let cy = y + 32.0 + (i / per_row) as f64 * 40.0;
            let bar = Rect::new(x + 2.0, cy, cw - 6.0, 34.0);
            ui.c.round(bar, 3.0, th.paper);
            let fh = bar.h * (c / 100.0).clamp(0.0, 1.0);
            let col = if v.efficiency.get(i).copied().unwrap_or(false) { rgb(0xE86CA0) } else { rgb(0x4A90D9) };
            ui.c.round(Rect::new(bar.x, bar.bottom() - fh, bar.w, fh), 3.0, col);
        }
        y += h + 8.0;
    }
    y = process_card(ui, "Top Processes", |p| p.cpu, |p| format!("{:.1}%", p.cpu), y);
    let mut info = vec![("Uptime".to_string(), format::uptime(v.uptime))];
    info.push(("Processor queue (no load average on Windows)".to_string(), v.queue.map_or("–".into(), |q| format!("{q:.0}"))));
    info.push(("Clock".to_string(), v.mhz.map_or("–".into(), |f| format!("{:.2} GHz", f / 1000.0))));
    info.push(("GPU".to_string(), v.gpu.map_or("–".into(), |g| format!("{g:.0}%"))));
    info.push(("GPU memory".to_string(), v.gpu_mem.map_or("–".into(), format::bytes)));
    rows_card(ui, "System", &info, y)
}

fn memory(ui: &mut Ui, y: f64) -> f64 {
    let th = ui.th;
    let m = &app().monitor;
    let v = m.last.clone();
    let y0 = y + 8.0;
    ui.card(Rect::new(PAD, y0, ui.w - PAD * 2.0, 130.0));
    let col = if v.mem_load >= 95.0 { th.salsa } else if v.mem_load >= 88.0 { th.nacho_deep } else { th.guac };
    ring(ui, ui.w * 0.28, y0 + 54.0, 34.0, v.mem_load / 100.0, col, &format!("{:.0}%", v.mem_load), "Used");
    let commit = v.commit_used / v.commit_limit.max(1.0);
    ring(ui, ui.w * 0.72, y0 + 54.0, 34.0, commit, rgb(0x4A90D9), &format!("{:.0}%", commit * 100.0), "Committed");
    let y = chart_card(ui, "memory", "History", &format!("{} of {}", format::bytes(v.mem_used), format::bytes(v.mem_total)), &[(&m.history.mem, col)], Some(100.0), y0 + 138.0);
    let y = process_card(ui, "Top Processes", |p| p.mem, |p| format::bytes(p.mem), y);
    rows_card(ui, "Details", &[
        ("Cached".to_string(), format::bytes(v.mem_cache)),
        ("Kernel".to_string(), format::bytes(v.mem_kernel)),
        ("Page ins".to_string(), v.page_in.map_or("–".into(), |p| format!("{p:.0}/s"))),
        ("Page outs".to_string(), v.page_out.map_or("–".into(), |p| format!("{p:.0}/s"))),
        ("Page file used".to_string(), format::bytes((v.commit_used - v.mem_used).max(0.0))),
    ], y)
}

fn disk(ui: &mut Ui, y: f64) -> f64 {
    let th = ui.th;
    let m = &app().monitor;
    let v = m.last.clone();
    let mut y = y + 8.0;
    let h = 34.0 + v.volumes.len() as f64 * 44.0;
    ui.card(Rect::new(PAD, y, ui.w - PAD * 2.0, h));
    ui.eyebrow("Volumes", PAD + 12.0, y + 10.0, 200.0, th.ink2);
    let mut vy = y + 32.0;
    for d in &v.volumes {
        let used = 1.0 - d.free / d.total.max(1.0);
        let name = if d.label.is_empty() { d.root.clone() } else { format!("{} ({})", d.label, d.root.trim_end_matches('\\')) };
        ui.c.line_text(&name, medium(13), Rect::new(PAD + 12.0, vy, ui.w * 0.5, 20.0), th.ink, Align::Left);
        ui.c.line_text(&format!("{} free of {}", format::bytes(d.free), format::bytes(d.total)), sans(12), Rect::new(ui.w * 0.3, vy, ui.w * 0.7 - PAD - 12.0, 20.0), th.ink2, Align::Right);
        let bar = Rect::new(PAD + 12.0, vy + 22.0, ui.w - PAD * 2.0 - 24.0, 8.0);
        ui.c.round(bar, 4.0, th.line);
        ui.c.round(Rect::new(bar.x, bar.y, bar.w * used, bar.h), 4.0, if used >= 0.9 { th.salsa } else { NACHO });
        vy += 44.0;
    }
    y += h + 8.0;
    y = chart_card(ui, "disk", "Activity", &format!("↓ {}  ↑ {}", format::rate(v.disk_read), format::rate(v.disk_write)), &[(&m.history.read, rgb(0x4A90D9)), (&m.history.write, th.salsa)], None, y);
    y = rows_card(ui, "Since Nacho Combo Started", &[("Read".to_string(), format::bytes(m.total_read)), ("Written".to_string(), format::bytes(m.total_write))], y);
    process_card(ui, "Processes", |p| p.read + p.write, |p| format!("R {}  W {}", format::rate(p.read), format::rate(p.write)), y)
}

fn network(ui: &mut Ui, y: f64) -> f64 {
    let th = ui.th;
    let m = &app().monitor;
    let v = m.last.clone();
    let mut y = chart_card(ui, "network", "Traffic", &format!("↓ {}  ↑ {}", format::rate(v.net_down), format::rate(v.net_up)), &[(&m.history.down, rgb(0x4A90D9)), (&m.history.up, th.salsa)], None, y + 8.0);
    ui.card(Rect::new(PAD, y, ui.w - PAD * 2.0, 76.0));
    ui.eyebrow("Public IP", PAD + 12.0, y + 10.0, 200.0, th.ink2);
    let shown = m.public_ip.clone().or(m.public_ip_state.clone()).unwrap_or_else(|| "Not looked up".into());
    ui.c.line_text(&shown, medium(13), Rect::new(PAD + 12.0, y + 34.0, ui.w * 0.5, 30.0), th.ink, Align::Left);
    let lw = ui.button_width("Look Up", true);
    if ui.button("lookup", "Look Up", ui.w - PAD - 12.0 - lw, y + 37.0, Btn::Secondary, true) { later(monitor::request_public_ip); }
    y += 84.0;
    let rows: Vec<(String, String)> = m.adapters.iter().filter(|a| a.up).map(|a| (a.name.clone(), a.addresses.first().cloned().unwrap_or_else(|| "–".into()))).collect();
    y = rows_card(ui, "Interfaces", &rows, y);
    ui.label("Per-process traffic needs administrator rights on Windows, so it is not shown.", sans(12), PAD + 4.0, y, ui.w - PAD * 2.0 - 8.0, th.ink2);
    y + 40.0
}

fn sensors(ui: &mut Ui, y: f64) -> f64 {
    let th = ui.th;
    let m = &app().monitor;
    let v = m.last.clone();
    let mut y = y + 8.0;
    if m.history.temp.latest().is_some() {
        y = chart_card(ui, "temp", "Hottest Sensor", &monitor::hottest(&v).map_or("–".into(), |t| format!("{t:.0} °C")), &[(&m.history.temp, th.salsa)], Some(110.0), y);
    }
    if m.history.gpu.latest().is_some() {
        y = chart_card(ui, "gpu", "GPU", &v.gpu.map_or("–".into(), |g| format!("{g:.0}%")), &[(&m.history.gpu, rgb(0x4A90D9))], Some(100.0), y);
    }
    let mut rows: Vec<(String, String)> = v.temps.iter().map(|t| (t.0.clone(), format!("{:.0} °C", t.1))).collect();
    if rows.is_empty() { rows.push(("Temperature".to_string(), "–".to_string())); }
    rows.push(("CPU clock".to_string(), v.mhz.map_or("–".into(), |f| format!("{:.2} GHz", f / 1000.0))));
    rows.push(("Fans".to_string(), "–".to_string()));
    y = rows_card(ui, "Sensors", &rows, y);
    ui.label("Windows reports only what the firmware exposes. Fans and per-core temperatures need a kernel driver, which this app does not install.", sans(12), PAD + 4.0, y, ui.w - PAD * 2.0 - 8.0, th.ink2);
    y + 56.0
}

fn battery(ui: &mut Ui, y: f64) -> f64 {
    let th = ui.th;
    let m = &app().monitor;
    let Some(b) = m.last.battery.clone() else {
        ui.label("This PC has no battery.", sans(13), PAD + 4.0, y + 12.0, ui.w - 40.0, th.ink2);
        return y + 40.0;
    };
    let y0 = y + 8.0;
    ui.card(Rect::new(PAD, y0, ui.w - PAD * 2.0, 130.0));
    let col = if b.percent <= 10.0 { th.salsa } else if b.percent <= 25.0 { th.nacho_deep } else { th.guac };
    ring(ui, ui.w * 0.28, y0 + 54.0, 34.0, b.percent / 100.0, col, &format!("{:.0}%", b.percent), b.state());
    let health = b.health();
    ring(ui, ui.w * 0.72, y0 + 54.0, 34.0, health.unwrap_or(0.0) / 100.0, rgb(0x4A90D9), &health.map_or("–".into(), |h| format!("{h:.0}%")), "Health");
    let y = chart_card(ui, "battery", "Charge", &format!("{:.0}%", b.percent), &[(&m.history.battery, col)], Some(100.0), y0 + 138.0);
    let some_mw = |v: Option<u32>| v.map_or("–".to_string(), |x| format!("{:.1} Wh", x as f64 / 1000.0));
    rows_card(ui, "Details", &[
        ("Time remaining".to_string(), b.seconds_left.map_or("–".into(), |s| format::minutes(s / 60))),
        ("Power source".to_string(), if b.on_ac { "Plugged in".to_string() } else { "Battery".to_string() }),
        ("Power".to_string(), b.rate_mw.map_or("–".into(), |r| format!("{:+.1} W", r as f64 / 1000.0))),
        ("Voltage".to_string(), b.voltage_mv.map_or("–".into(), |v| format!("{:.2} V", v as f64 / 1000.0))),
        ("Cycles".to_string(), b.cycles.map_or("–".into(), |c| c.to_string())),
        ("Full charge".to_string(), some_mw(b.full)),
        ("Design capacity".to_string(), some_mw(b.design)),
    ], y)
}
