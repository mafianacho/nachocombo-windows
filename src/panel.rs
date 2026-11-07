//! The quick panel that opens from the nacho box: the tool list, each tool's controls and the combo screen.
use crate::app::{app, later, State};
use crate::core::catalog;
use crate::core::combos;
use crate::core::layout::{Rect, SnapAction};
use crate::core::settings::*;
use crate::gfx::*;
use crate::ui::*;
use crate::window::{UiWindow, WindowCtx};
use crate::{capture, dim, monitor, monitor_panels, pages, shelf, snap, trayorg};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Route { List, Tool(ModuleId), Combos }

#[derive(Default)]
pub struct Panel { win: Option<isize>, pub route: Option<Route>, pub combo_pick: Option<String> }

impl Panel {
    pub fn is_open(&self) -> bool { self.win.is_some() }
    pub fn invalidate(&self) { if let Some(h) = self.win { unsafe { let _ = InvalidateRect(Some(HWND(h as *mut _)), None, false); } } }
    pub fn hwnd(&self) -> Option<HWND> { self.win.map(|h| HWND(h as *mut _)) }
}

pub fn toggle_list() { if app().panel.is_open() { close(); } else { open(Route::List); } }
pub fn open_tool(m: ModuleId) { open(Route::Tool(m)); }

pub fn close() { if let Some(h) = app().panel.win.take() { unsafe { let _ = DestroyWindow(HWND(h as *mut _)); } } }

pub fn open(route: Route) {
    let p = &mut app().panel;
    p.route = Some(route);
    if let Some(h) = p.hwnd() { p.invalidate(); unsafe { let _ = SetForegroundWindow(h); } return; }
    let win = UiWindow::create("Nacho Combo", WS_POPUP, WS_EX_TOOLWINDOW | WS_EX_TOPMOST, 0, 0, 320.0, 520.0, Box::new(draw));
    win.close_on_deactivate = true;
    win.on_close = Some(Box::new(|| { app().panel.win = None; }));
    let h = win.hwnd;
    app().panel.win = Some(h.0 as isize);
    monitor_panels::place_near_tray(h);
    win.show();
}

pub fn status_color(th: &Theme, s: &State, on: bool) -> Color {
    match s { State::Running => th.guac, State::Failed(_) => th.salsa, State::Off if on => th.nacho_deep, State::Off => STATUS_OFF }
}

pub fn status_text(s: &State) -> String { match s { State::Running => "Running".into(), State::Off => "Off".into(), State::Failed(e) => format!("Error: {e}") } }

fn draw(ui: &mut Ui, ctx: &mut WindowCtx) {
    let th = ui.th;
    let route = app().panel.route.unwrap_or(Route::List);
    for k in ui.st.keys.clone() {
        if k.vk == 0x1B { if route == Route::List { ctx.close = true; } else { app().panel.route = Some(Route::List); ui.st.again = true; } }
    }
    let head = Rect::new(0.0, 0.0, ui.w, 48.0);
    ui.c.fill(head, NACHO);
    if route == Route::List {
        ui.c.logo(Rect::new(10.0, 8.0, 32.0, 32.0));
        ui.c.line_text("NACHO COMBO", sign(18), Rect::new(46.0, 0.0, 200.0, 48.0), ON_NACHO, Align::Left);
    } else {
        let back = Rect::new(8.0, 10.0, 110.0, 28.0);
        let id = ui.id("back");
        let r = ui.hit(id, back, true);
        if r.hover { ui.c.round(back, 6.0, ON_NACHO.alpha(0.12)); }
        ui.c.line_text("‹  All Tools", semi(13), back.inset(8.0, 0.0), ON_NACHO, Align::Left);
        if r.clicked { app().panel.route = Some(Route::List); ui.st.again = true; }
    }
    let gear = Rect::new(ui.w - 40.0, 10.0, 28.0, 28.0);
    let gid = ui.id("gear");
    let g = ui.hit(gid, gear, true);
    if g.hover { ui.c.round(gear, 6.0, ON_NACHO.alpha(0.12)); }
    ui.c.glyph(0xE713, 14, gear, ON_NACHO);
    if g.clicked {
        ctx.close = true;
        let m = if let Route::Tool(m) = route { Some(m) } else { None };
        later(move || pages::open(m));
    }
    let view = Rect::new(0.0, 48.0, ui.w, ui.h - 48.0);
    match route {
        Route::List => ui.scroll("list", view, |ui, y| list(ui, ctx, y)),
        Route::Tool(m) => ui.scroll(&format!("tool{}", m.key()), view, |ui, y| {
            let y = tool_header(ui, m, y + 12.0);
            controls(ui, m, 14.0, y, ui.w - 28.0) + 12.0
        }),
        Route::Combos => ui.scroll("combos", view, |ui, y| combo_screen(ui, y)),
    }
    let _ = th;
}

fn list(ui: &mut Ui, ctx: &mut WindowCtx, y: f64) -> f64 {
    let th = ui.th;
    let a = app();
    let mut y = y + 8.0;
    let failed: Vec<ModuleId> = ModuleId::ALL.into_iter().filter(|m| matches!(a.state(*m), State::Failed(_))).collect();
    if let Some(m) = failed.first() {
        let r = Rect::new(10.0, y, ui.w - 20.0, 40.0);
        ui.c.round(r, 8.0, th.salsa.alpha(0.12));
        ui.c.line_text(&format!("{} could not start", m.name()), medium(12), r.inset(12.0, 0.0), th.salsa, Align::Left);
        let m = *m;
        let bw = ui.button_width("Details", true);
        if ui.button("details", "Details", r.right() - bw - 8.0, r.y + 8.0, Btn::Secondary, true) { ctx.close = true; later(move || pages::open(Some(m))); }
        y += 48.0;
    }
    let combo = combos::current(&a.settings).map_or("Your own mix".to_string(), |c| format!("No.{} {}", c.number, c.name));
    let r = Rect::new(10.0, y, ui.w - 20.0, 44.0);
    let id = ui.id("combo-row");
    let resp = ui.hit(id, r, true);
    ui.c.round(r, 8.0, if resp.hover { BOARD2 } else { BOARD });
    ui.c.line_text("COMBO", mono(11), Rect::new(r.x + 14.0, r.y, 70.0, r.h), BOARD_MUTE, Align::Left);
    ui.c.line_text(&combo, semi(13), Rect::new(r.x + 80.0, r.y, r.w - 110.0, r.h), BOARD_INK, Align::Left);
    ui.c.line_text("›", semi(16), Rect::new(r.right() - 24.0, r.y, 14.0, r.h), BOARD_INK, Align::Center);
    if resp.clicked { app().panel.route = Some(Route::Combos); ui.st.again = true; }
    y += 52.0;
    for m in ModuleId::ALL {
        let r = Rect::new(10.0, y, ui.w - 20.0, 40.0);
        let id = ui.id(m.key());
        let resp = ui.hit(id, r, true);
        if resp.hover || resp.focused { ui.c.round(r, 8.0, th.paper2); }
        let st = a.state(m);
        ui.status_dot(r.x + 12.0, r.cy(), status_color(&th, &st, a.settings.is_on(m)));
        crate::pages::tile(ui.c, m, r.x + 22.0, r.y + 9.0, 22.0);
        ui.c.line_text(m.name(), medium(13), Rect::new(r.x + 52.0, r.y, r.w - 140.0, r.h), th.ink, Align::Left);
        if let Some(s) = a.settings.tool_shortcuts.get(&m) { ui.c.line_text(&s.label(), mono(10), Rect::new(r.right() - 120.0, r.y, 96.0, r.h), th.ink2, Align::Right); }
        ui.c.line_text("›", semi(16), Rect::new(r.right() - 20.0, r.y, 12.0, r.h), th.ink2, Align::Center);
        if resp.clicked { app().panel.route = Some(Route::Tool(m)); ui.st.again = true; }
        y += 42.0;
    }
    y += 6.0;
    ui.c.line(14.0, y, ui.w - 14.0, y, th.line, 1.0);
    y += 10.0;
    if ui.button("settings", "Settings…", 14.0, y, Btn::Secondary, true) { ctx.close = true; later(|| pages::open(None)); }
    let qw = ui.button_width("Quit", true);
    if ui.button("quit", "Quit", ui.w - 14.0 - qw, y + 3.0, Btn::Secondary, true) { crate::app::quit(); }
    y + 44.0
}

fn tool_header(ui: &mut Ui, m: ModuleId, y: f64) -> f64 {
    let th = ui.th;
    let a = app();
    crate::pages::tile(ui.c, m, 14.0, y, 32.0);
    ui.c.line_text(m.name(), bold(16), Rect::new(54.0, y, ui.w - 120.0, 20.0), th.ink, Align::Left);
    let st = a.state(m);
    ui.c.line_text(&status_text(&st), sans(12), Rect::new(54.0, y + 20.0, ui.w - 120.0, 16.0), status_color(&th, &st, a.settings.is_on(m)), Align::Left);
    let on = a.settings.is_on(m);
    if ui.switch(&format!("sw{}", m.key()), ui.w - 56.0, y + 6.0, on) { later(move || app().set_enabled(m, !on)); }
    y + 52.0
}

/// What a tool lets you do right away. Used by the quick panel and the gadget's page.
pub fn controls(ui: &mut Ui, m: ModuleId, x: f64, y: f64, w: f64) -> f64 {
    let th = ui.th;
    let a = app();
    if a.state(m) != State::Running {
        ui.label("Switch it on to use these controls.", sans(13), x, y, w, th.ink2);
        let on = a.settings.is_on(m);
        if !on && ui.button(&format!("on{}", m.key()), "Turn On", x, y + 26.0, Btn::Primary, false) { later(move || app().set_enabled(m, true)); }
        return y + 64.0;
    }
    let mut y = y;
    let row_x = std::cell::Cell::new(x);
    let btn = |ui: &mut Ui, key: &str, label: &str, y: &mut f64, style: Btn| -> bool {
        let bw = ui.button_width(label, false);
        if row_x.get() + bw > x + w { row_x.set(x); *y += 38.0; }
        let hit = ui.button(key, label, row_x.get(), *y, style, false);
        row_x.set(row_x.get() + bw + 8.0);
        hit
    };
    match m {
        ModuleId::TrayOrganizer => {
            let collapsed = a.settings.tray_collapsed;
            if btn(ui, "fold", if collapsed { "Show All Icons" } else { "Fold Away Icons" }, &mut y, Btn::Primary) { later(trayorg::toggle); }
            y += 46.0;
            let kept = a.settings.tray_keep.len();
            ui.label(&format!("{kept} icon{} always in view. Choose them on the gadget's page.", if kept == 1 { "" } else { "s" }), sans(12), x, y, w, th.ink2);
            y += 24.0;
        }
        ModuleId::SystemMonitor => {
            let v = &a.monitor.last;
            let rows = [
                ("CPU", format!("{:.0}%", v.cpu_user + v.cpu_system), MonitorItem::Cpu),
                ("Memory", format!("{:.0}%", v.mem_load), MonitorItem::Memory),
                ("Network", format!("↓ {}  ↑ {}", crate::core::format::rate(v.net_down), crate::core::format::rate(v.net_up)), MonitorItem::Network),
                ("Disk", format!("R {}  W {}", crate::core::format::rate(v.disk_read), crate::core::format::rate(v.disk_write)), MonitorItem::Disk),
            ];
            for (label, value, item) in rows {
                let r = Rect::new(x, y, w, 34.0);
                let id = ui.id(&format!("mon{label}"));
                let resp = ui.hit(id, r, true);
                ui.c.round(r, 7.0, if resp.hover { th.paper2 } else { th.paper });
                ui.c.line_text(label, sans(13), r.inset(10.0, 0.0), th.ink2, Align::Left);
                ui.c.line_text(&value, semi(13), r.inset(10.0, 0.0), th.ink, Align::Right);
                if resp.clicked { later(move || monitor::open_panel(item)); }
                y += 38.0;
            }
        }
        ModuleId::WindowSnap => {
            ui.eyebrow("Front window", x, y, w, th.ink2);
            y += 20.0;
            let cols = 4.0;
            let cell = (w - 6.0 * (cols - 1.0)) / cols;
            const GRID: [SnapAction; 12] = [SnapAction::Left, SnapAction::Right, SnapAction::TopHalf, SnapAction::BottomHalf, SnapAction::TopLeft, SnapAction::TopRight, SnapAction::BottomLeft, SnapAction::BottomRight, SnapAction::FirstThird, SnapAction::CenterThird, SnapAction::LastThird, SnapAction::Maximize];
            for (i, act) in GRID.iter().enumerate() {
                let r = Rect::new(x + (i % 4) as f64 * (cell + 6.0), y + (i / 4) as f64 * (cell * 0.66 + 6.0), cell, cell * 0.66);
                let id = ui.id(&format!("snap{i}"));
                let resp = ui.hit(id, r, true);
                ui.c.round(r, 6.0, if resp.hover { th.paper2 } else { th.paper });
                ui.c.stroke_round(r, 6.0, th.line, 1.0);
                let inner = r.inset(6.0, 6.0);
                let t = crate::core::layout::target(*act, Rect::new(0.0, 0.0, 1000.0, 600.0), None, 0.0, false);
                ui.c.round(Rect::new(inner.x + inner.w * t.x / 1000.0, inner.y + inner.h * t.y / 600.0, inner.w * t.w / 1000.0, inner.h * t.h / 600.0), 2.0, if resp.hover { th.nacho_deep } else { NACHO });
                if resp.clicked {
                    let act = *act;
                    // The panel itself is in front: snap the window that was in front before it.
                    later(move || { close(); snap::run(act); });
                }
            }
            y += 3.0 * (cell * 0.66 + 6.0) + 6.0;
        }
        ModuleId::FileShelf => {
            let open = a.shelf.is_open();
            if btn(ui, "shelf", if open { "Close Shelf" } else { "Open Shelf" }, &mut y, Btn::Primary) { later(shelf::toggle); }
            if btn(ui, "paste", "Add Clipboard", &mut y, Btn::Secondary) { later(shelf::paste_clipboard); }
            y += 46.0;
            ui.label(&format!("{} item{} on the shelf", a.shelf.count(), if a.shelf.count() == 1 { "" } else { "s" }), sans(12), x, y, w, th.ink2);
            y += 24.0;
        }
        ModuleId::CaptureFloat => {
            if btn(ui, "area", "Capture Area", &mut y, Btn::Primary) { later(|| { close(); capture::select(capture::Mode::Area); }); }
            if btn(ui, "window", "Window", &mut y, Btn::Secondary) { later(|| { close(); capture::select(capture::Mode::Window); }); }
            if btn(ui, "screen", "Screen", &mut y, Btn::Secondary) { later(|| { close(); capture::capture_screen(); }); }
            if a.capture.has_previous() && btn(ui, "prev", "Previous Area", &mut y, Btn::Secondary) { later(|| { close(); capture::capture_previous(); }); }
            if btn(ui, "text", "Copy Text", &mut y, Btn::Secondary) { later(|| { close(); capture::select(capture::Mode::Text); }); }
            if btn(ui, "color", "Pick Colour", &mut y, Btn::Secondary) { later(|| { close(); capture::select(capture::Mode::Color); }); }
            if btn(ui, "live", "Live Float", &mut y, Btn::Secondary) { later(|| { close(); capture::select(capture::Mode::Live); }); }
            y += 46.0;
            let n = a.capture.float_count();
            ui.label(&format!("{n} float{} on screen", if n == 1 { "" } else { "s" }), sans(12), x, y, w, th.ink2);
            y += 24.0;
            row_x.set(x);
            let locked = a.capture.locked;
            if btn(ui, "lock", if locked { "Unlock Floats" } else { "Lock Floats" }, &mut y, Btn::Secondary) { later(capture::toggle_lock); }
            if n > 0 && btn(ui, "closeall", "Close All", &mut y, Btn::Secondary) { later(capture::close_all); }
            y += 46.0;
        }
        ModuleId::DiskMap => { y = crate::diskmap::controls(ui, x, y, w); }
        ModuleId::KeepAwake => {
            let on = a.awake.manual;
            if btn(ui, "awake", if on { "Let It Sleep" } else { "Keep Awake" }, &mut y, Btn::Primary) { later(crate::awake::toggle); }
            y += 46.0;
            row_x.set(x);
            for (m, label) in crate::awake::DURATIONS.iter().take(4) { if btn(ui, label, label, &mut y, Btn::Secondary) { let m = *m; later(move || crate::awake::set(true, Some(m))); } }
            y += 46.0;
            let state = if let Some(n) = &a.awake.by_app { format!("Awake while {n} runs") } else if a.awake.active() { a.awake.minutes_left().map_or("Awake until you turn it off".into(), |m| format!("Awake for {m} more min")) } else if a.awake.guarded { "Battery low: sleeping as usual".into() } else { "Sleeps as usual".into() };
            ui.label(&state, sans(12), x, y, w, th.ink2);
            y += 24.0;
        }
        ModuleId::Clipboard => {
            if btn(ui, "clipshow", "Show History", &mut y, Btn::Primary) { later(|| { close(); crate::clipboard::show(); }); }
            y += 46.0;
            for (n, c) in a.clipboard.items.iter().take(5).enumerate() {
                let r = Rect::new(x, y, w, 32.0);
                let id = ui.id(&format!("pc{n}"));
                let resp = ui.hit(id, r, true);
                ui.c.round(r, 7.0, if resp.hover { th.paper2 } else { th.paper });
                ui.c.line_text(&c.title(), sans(12), r.inset(10.0, 0.0), th.ink, Align::Left);
                if resp.clicked { let fg = n; later(move || { close(); crate::clipboard::paste(fg, false); }); }
                y += 36.0;
            }
        }
        ModuleId::DeviceControl => {
            ui.eyebrow(&format!("Volume {:.0}%", a.devices.volume * 100.0), x, y, w, th.ink2);
            if let Some(v) = ui.slider("vol", Rect::new(x, y + 20.0, w, 26.0), a.devices.volume as f64, 0.0, 1.0) { crate::devices::set_volume(v as f32); }
            y += 56.0;
            ui.eyebrow("Output", x, y, w, th.ink2);
            y += 20.0;
            for d in a.devices.outputs.clone() {
                if ui.checkbox(&format!("out{}", d.id), &d.name, x, y, w, d.default) && !d.default { let id = d.id.clone(); later(move || crate::devices::set_default(&id)); }
                y += 28.0;
            }
            y += 8.0;
            row_x.set(x);
            let muted = a.devices.mic_muted;
            if btn(ui, "mic", if muted { "Unmute Microphone" } else { "Mute Microphone" }, &mut y, if muted { Btn::Attention } else { Btn::Secondary }) { later(crate::devices::toggle_mic); }
            y += 46.0;
            for (i, m) in a.devices.monitors.clone().iter().enumerate() {
                ui.eyebrow(&format!("{}  {}%", m.name, (m.brightness - m.min) * 100 / (m.max - m.min).max(1)), x, y, w, th.ink2);
                if let Some(v) = ui.slider(&format!("br{i}"), Rect::new(x, y + 20.0, w, 26.0), m.brightness as f64, m.min as f64, m.max as f64) { crate::devices::set_brightness(i, v.round() as u32); }
                y += 56.0;
            }
            if a.devices.monitors.is_empty() { ui.label("No display here accepts brightness over DDC/CI.", sans(12), x, y, w, th.ink2); y += 24.0; }
            if !a.devices.sessions.is_empty() {
                y += 6.0;
                ui.eyebrow("Volume per app", x, y, w, th.ink2);
                y += 22.0;
                for s in a.devices.sessions.clone() {
                    let mr = Rect::new(x, y + 2.0, 26.0, 26.0);
                    let id = ui.id(&format!("mute{}", s.name));
                    let resp = ui.hit(id, mr, true);
                    if resp.hover { ui.c.round(mr, 6.0, th.paper2); }
                    ui.c.glyph(if s.muted { 0xE74F } else { 0xE767 }, 12, mr, if s.muted { th.salsa } else { th.ink2 });
                    if resp.clicked { let n = s.name.clone(); later(move || { crate::devices::set_app_volume(&n, None, Some(!s.muted)); app().changed(); }); }
                    ui.c.line_text(&s.name, sans(12), Rect::new(x + 32.0, y, w * 0.4, 30.0), th.ink, Align::Left);
                    if let Some(v) = ui.slider(&format!("av{}", s.name), Rect::new(x + w * 0.45, y + 2.0, w * 0.55, 26.0), s.volume as f64, 0.0, 1.0) { crate::devices::set_app_volume(&s.name, Some(v as f32), None); }
                    y += 32.0;
                }
            }
            row_x.set(x);
            y += 6.0;
            if btn(ui, "theme", if crate::window::is_dark() { "Light Mode" } else { "Dark Mode" }, &mut y, Btn::Secondary) { later(crate::devices::toggle_theme); }
            y += 46.0;
            if let Some(n) = &a.devices.notice { y += ui.label(n, sans(12), x, y, w, th.salsa) + 6.0; }
        }
        ModuleId::FocusDim => {
            ui.eyebrow(&format!("Strength {:.0}%", a.settings.dim_intensity * 100.0), x, y, w, th.ink2);
            if let Some(v) = ui.slider("dimslider", Rect::new(x, y + 20.0, w, 26.0), a.settings.dim_intensity, 0.1, 0.9) {
                a.settings.dim_intensity = (v * 20.0).round() / 20.0;
                a.save();
                dim::options_changed();
                a.changed();
            }
            y += 56.0;
            let paused = a.dim.paused;
            if btn(ui, "pause", if paused { "Resume Dimming" } else { "Pause Dimming" }, &mut y, Btn::Secondary) { later(dim::toggle_pause); }
            y += 46.0;
        }
    }
    y
}

fn combo_screen(ui: &mut Ui, y: f64) -> f64 {
    let th = ui.th;
    let a = app();
    let all = combos::all(&a.settings);
    let cur = combos::current(&a.settings).map(|c| c.id);
    let pick = a.panel.combo_pick.clone().or(cur.clone()).unwrap_or_else(|| "starter".into());
    let mut y = y + 10.0;
    for c in &all {
        let r = Rect::new(10.0, y, ui.w - 20.0, 38.0);
        let id = ui.id(&format!("c{}", c.id));
        let resp = ui.hit(id, r, true);
        let sel = c.id == pick;
        ui.c.round(r, 8.0, if sel { NACHO.alpha(0.35) } else if resp.hover { th.paper2 } else { th.paper });
        ui.c.line_text(&format!("No.{}", c.number), sign(14), Rect::new(r.x + 12.0, r.y, 48.0, r.h), th.nacho_ink, Align::Left);
        ui.c.line_text(&c.name, semi(13), Rect::new(r.x + 60.0, r.y, r.w - 120.0, r.h), th.ink, Align::Left);
        if cur.as_ref() == Some(&c.id) { ui.c.line_text("Now", mono(10), Rect::new(r.right() - 60.0, r.y, 50.0, r.h), th.guac, Align::Right); }
        if resp.clicked { app().panel.combo_pick = Some(c.id.clone()); ui.st.again = true; }
        y += 42.0;
    }
    let Some(c) = all.iter().find(|c| c.id == pick).cloned() else { return y };
    y += 6.0;
    let plan = combos::plan(&c, &a.settings);
    let mut lines = vec![];
    let names = |v: &[ModuleId]| v.iter().map(|m| m.name()).collect::<Vec<_>>().join(", ");
    if !plan.turns_on.is_empty() { lines.push(format!("Turns on: {}", names(&plan.turns_on))); }
    if !plan.turns_off.is_empty() { lines.push(format!("Turns off: {}", names(&plan.turns_off))); }
    lines.extend(plan.changes.iter().cloned());
    if lines.is_empty() { lines.push("This is your box already.".into()); }
    for l in &lines { y += ui.label(l, sans(12), 14.0, y, ui.w - 28.0, th.ink2) + 2.0; }
    y += 8.0;
    let no_change = plan.turns_on.is_empty() && plan.turns_off.is_empty() && plan.changes.is_empty();
    if ui.button_rect("order", "Order", Rect::new(14.0, y, 100.0, 30.0), Btn::Primary, false, !no_change) { later(move || app().order(&c)); }
    if a.undo.is_some() && ui.button("undo", "Undo", 124.0, y, Btn::Secondary, false) { later(|| app().undo_order()); }
    y += 40.0;
    if ui.button("own", "Make Your Own…", 14.0, y, Btn::Secondary, true) { later(|| { close(); pages::open_combos_new(); }); }
    let _ = catalog::actions;
    y + 40.0
}
