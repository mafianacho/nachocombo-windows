//! Widget Board: one window with a tile per gadget (the macOS "bento"). What a tile says is decided in `core::bento`;
//! this file reads the running gadgets, lays the tiles out and draws them. The board starts nothing by itself: editing it
//! never turns a gadget on or off, and a tile's switch is the user's own click.
use crate::app::{app, later, State, TIMER_BOARD};
use crate::core::bento::*;
use crate::core::catalog;
use crate::core::layout::Rect;
use crate::core::settings::ModuleId;
use crate::gfx::*;
use crate::ui::*;
use crate::window::{UiWindow, WindowCtx};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::*;

const PAD: f64 = 20.0;
const GAP: f64 = 12.0;
const MIN_CELL: f64 = 150.0;
const CELL_H: f64 = 150.0;
const HEAD: f64 = 64.0;

#[derive(Default)]
pub struct Board {
    win: Option<isize>,
    /// Edit mode: tiles can be dragged, resized and hidden.
    pub edit: bool,
    /// The tile being dragged.
    drag: Option<ModuleId>,
}

impl Board {
    pub fn is_open(&self) -> bool { self.win.is_some() }
    pub fn diagnostics(&self) -> serde_json::Value {
        let l = app().settings.board_layout();
        serde_json::json!({ "open": self.win.is_some(), "edit": self.edit, "visible": l.visible().len(), "hidden": l.hidden_modules().len(), "saved": app().settings.bento.is_some() })
    }
    pub fn invalidate(&self) { if let Some(h) = self.win { unsafe { let _ = InvalidateRect(Some(HWND(h as *mut _)), None, false); } } }
}

pub fn start() -> std::result::Result<(), String> { Ok(()) }

/// The OFF contract: the window and its refresh timer go.
pub fn stop() {
    let a = app();
    unsafe { let _ = KillTimer(Some(a.main), TIMER_BOARD); }
    if let Some(h) = a.board.win.take() { unsafe { let _ = DestroyWindow(HWND(h as *mut _)); } }
    a.board = Board::default();
}

/// Once a second while the window is open: the numbers on the tiles move.
pub fn tick() {
    let a = app();
    if a.board.win.is_none() { unsafe { let _ = KillTimer(Some(a.main), TIMER_BOARD); } return; }
    a.board.invalidate();
}

pub fn show() {
    let a = app();
    if a.state(ModuleId::WidgetBoard) != State::Running { return; }
    if let Some(h) = a.board.win {
        unsafe { let h = HWND(h as *mut _); let _ = ShowWindow(h, SW_RESTORE); let _ = SetForegroundWindow(h); }
        return;
    }
    let win = UiWindow::create("Widget Board", WS_OVERLAPPEDWINDOW, WINDOW_EX_STYLE(0), CW_USEDEFAULT, CW_USEDEFAULT, 1000.0, 640.0, Box::new(draw));
    win.on_close = Some(Box::new(|| {
        let a = app();
        unsafe { let _ = KillTimer(Some(a.main), TIMER_BOARD); }
        a.board.win = None;
        a.board.edit = false;
        a.board.drag = None;
    }));
    win.set_msg(Box::new(|_, msg, _, lp| {
        if msg == WM_GETMINMAXINFO {
            let m = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
            m.ptMinTrackSize = POINT { x: 420, y: 360 };
        }
        None
    }));
    a.board.win = Some(win.hwnd.0 as isize);
    unsafe { SetTimer(Some(a.main), TIMER_BOARD, 1000, None); }
    win.show();
}

/// Saves a change of the board. Never touches which gadgets are on.
fn edit(f: impl FnOnce(&mut BentoLayout)) {
    app().update(|s| { let mut b = s.board_layout(); f(&mut b); s.bento = Some(b); });
}

// ---------- what each tile reads ----------

fn now_secs() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

/// The system drive's used and total bytes, readable whether or not Disk Map is on.
fn system_disk() -> Option<(u64, u64)> {
    use windows::core::w;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(w!("C:\\"), Some(&mut free), Some(&mut total), None).ok()?; }
    (total > 0).then(|| (total.saturating_sub(free), total))
}

fn shortcut_label(id: &str) -> Option<String> {
    catalog::actions().into_iter().find(|x| x.id == id).and_then(|x| app().settings.shortcut_for(&x)).map(|s| s.label())
}

fn body_of(m: ModuleId) -> BentoBody {
    let a = app();
    let running = a.state(m) == State::Running;
    match m {
        ModuleId::SystemMonitor => {
            let v = &a.monitor.last;
            let measured = a.monitor.ticks > 0;
            let disk = system_disk().map(|(u, t)| u as f64 / t as f64);
            build::monitor(running, measured.then_some(v.cpu_user + v.cpu_system), measured.then_some(v.mem_load), disk, measured.then_some(v.net_down), measured.then_some(v.net_up))
        }
        ModuleId::WindowSnap => {
            let acts = catalog::actions();
            let snaps: Vec<_> = acts.iter().filter(|x| x.module == ModuleId::WindowSnap).filter_map(|x| a.settings.shortcut_for(x).map(|s| (x.id.clone(), s.label()))).collect();
            let left = snaps.iter().find(|s| s.0 == "snap.left").map(|s| s.1.clone());
            build::window_snap(running, left.as_deref(), snaps.len().saturating_sub(left.is_some() as usize))
        }
        ModuleId::KeepAwake => {
            let holds = a.awake.active();
            let until = a.awake.minutes_left().map(|_| now_secs() + a.awake.until.map_or(0.0, |u| u.saturating_duration_since(std::time::Instant::now()).as_secs_f64()));
            build::keep_awake(running, holds, a.awake.manual, until, None, a.awake.by_app.as_ref().map(|n| format!("Awake while {n} runs")).as_deref(), 0, now_secs())
        }
        ModuleId::Clipboard => {
            use crate::clipboard::Content;
            let items: Vec<(RowKind, String, Option<f64>)> = a.clipboard.items.iter().take(8).map(|c| {
                let kind = match &c.content { Content::Image { .. } => RowKind::Image, Content::Files(_) => RowKind::File, Content::Text(t) if t.starts_with("http://") || t.starts_with("https://") => RowKind::Link, Content::Text(_) => RowKind::Text };
                (kind, c.title(), None)
            }).collect();
            build::clipboard(running, &items, now_secs(), 8)
        }
        ModuleId::CaptureFloat => build::capture(running, a.capture.float_count(), shortcut_label("capture.area").as_deref()),
        ModuleId::FocusDim => build::focus_dim(running, a.settings.dim_intensity, a.dim.paused),
        ModuleId::DeviceControl => {
            let d = &a.devices;
            let brightness = d.monitors.first().map(|m| ((m.brightness.saturating_sub(m.min)) * 100 / (m.max - m.min).max(1)) as i32);
            build::devices(running, (!d.outputs.is_empty()).then_some(d.volume as f64), brightness, d.outputs.iter().find(|o| o.default).map(|o| o.name.as_str()))
        }
        ModuleId::CalendarClock => { let (wd, day) = crate::clock::today(); build::calendar(running, wd, day, &crate::clock::world_lines(2)) }
        ModuleId::DiskMap => build::disk(system_disk().map(|d| d.0), system_disk().map(|d| d.1), &[]),
        ModuleId::FileShelf => {
            use crate::core::shelf::Kind;
            let items: Vec<(RowKind, String)> = a.shelf.shelf.items.iter().map(|i| (match i.kind { Kind::Image => RowKind::Image, Kind::Link(_) => RowKind::Link, Kind::Text(_) => RowKind::Text, Kind::File => RowKind::File }, i.title.clone())).collect();
            build::shelf(running, &items)
        }
        ModuleId::TrayOrganizer => build::tray(running, a.settings.tray_collapsed, a.organizer.notice.is_some()),
        ModuleId::AppSwitch => build::app_switch(running, shortcut_label("appswitch.next").as_deref(), a.settings.options.app_shortcuts.iter().filter(|h| h.shortcut.is_some()).count()),
        ModuleId::MouseTune => build::mouse(a.settings.options.mouse_speed, a.settings.options.mouse_reverse),
        ModuleId::WidgetBoard => BentoBody::Basic { line: String::new() },
    }
}

fn dot_of(m: ModuleId) -> BentoDot {
    match app().state(m) { State::Running => BentoDot::On, State::Failed(_) => BentoDot::Error, State::Off => BentoDot::Off }
}

// ---------- drawing ----------

fn tone_color(t: Tone, th: &Theme) -> Color { match t { Tone::Green => th.guac, Tone::Blue => rgb(0x4A90D9), Tone::Orange => rgb(0xF2A33A) } }

fn segment_color(i: usize, th: &Theme) -> Color { [NACHO, rgb(0x4A90D9), rgb(0xF2A33A), th.salsa, th.ink2][i.min(4)] }

fn bar(ui: &Ui, r: Rect, f: Option<f64>, c: Color) {
    ui.c.round(r, r.h / 2.0, ui.th.line);
    if let Some(f) = f { if f > 0.0 { ui.c.round(Rect::new(r.x, r.y, (r.w * f.clamp(0.0, 1.0)).max(r.h), r.h), r.h / 2.0, c); } }
}

fn ring(ui: &Ui, cx: f64, cy: f64, r: f64, f: Option<f64>, c: Color, centre: &str) {
    ui.c.ring(cx, cy, r, ui.th.line, 5.0);
    if let Some(f) = f { if f > 0.0 { ui.c.arc(cx, cy, r, f.clamp(0.0, 1.0), c, 5.0); } }
    ui.c.line_text(centre, semi(11), Rect::new(cx - r, cy - 10.0, r * 2.0, 20.0), ui.th.ink, Align::Center);
}

/// A caption with several sentences ("left · 2 h"): each is translated on its own, since a pattern cannot reach into a
/// number's unit.
fn say(line: &str) -> String { fmt::sentences(line).into_iter().map(|p| crate::i18n::t(p).into_owned()).collect::<Vec<_>>().join(" · ") }

/// The body of one tile inside `b`.
fn body(ui: &mut Ui, body: &BentoBody, b: Rect, size: BentoSize) {
    let th = ui.th;
    match body {
        BentoBody::Basic { line } => { ui.label(&say(line), sans(12), b.x, b.y, b.w, th.ink2); }
        BentoBody::Gauges { rings, side } => {
            let n = if size == BentoSize::Small { 2 } else { rings.len() };
            let r = ((b.h - 22.0) / 2.0).clamp(14.0, 30.0);
            let slot = r * 2.0 + 14.0;
            for (i, g) in rings.iter().take(n).enumerate() {
                let cx = b.x + r + 2.0 + i as f64 * slot;
                if cx + r > b.right() { break; }
                ring(ui, cx, b.y + r + 2.0, r, g.fraction, tone_color(g.tone, &th), &g.text());
                ui.c.line_text(&g.label, sans(10), Rect::new(cx - slot / 2.0, b.y + r * 2.0 + 6.0, slot, 16.0), th.ink2, Align::Center);
            }
            if size != BentoSize::Small {
                let sx = b.x + n as f64 * slot + 8.0;
                for (i, l) in side.iter().enumerate() {
                    let y = b.y + 4.0 + i as f64 * 22.0;
                    if sx + 40.0 > b.right() { break; }
                    ui.c.line_text(&l.label, if l.value.is_empty() { semi(11) } else { sans(11) }, Rect::new(sx, y, 70.0, 20.0), th.ink2, Align::Left);
                    ui.c.line_text(&l.value, mono(11), Rect::new(sx + 22.0, y, (b.right() - sx - 22.0).max(10.0), 20.0), th.ink, Align::Left);
                }
            }
        }
        BentoBody::Layouts { shapes, caption } => {
            let (w, h) = (34.0, 24.0);
            for (i, s) in shapes.iter().enumerate() {
                let x = b.x + i as f64 * (w + 8.0);
                if x + w > b.right() { break; }
                ui.c.stroke_round(Rect::new(x, b.y, w, h), 3.0, th.ink2, 1.2);
                ui.c.round(Rect::new(x + 2.0 + s.x * (w - 4.0), b.y + 2.0 + s.y * (h - 4.0), s.w * (w - 4.0), s.h * (h - 4.0)), 2.0, NACHO);
            }
            if let Some(c) = caption { ui.label(&say(c), sans(11), b.x, b.y + h + 8.0, b.w, th.ink2); }
        }
        BentoBody::Timer { fraction, big, caption } => {
            let r = 22.0;
            ring(ui, b.x + r + 2.0, b.y + r + 2.0, r, *fraction, th.nacho_deep, "");
            ui.c.line_text(big, bold(18), Rect::new(b.x + r * 2.0 + 14.0, b.y + 4.0, (b.w - r * 2.0 - 14.0).max(10.0), 36.0), th.ink, Align::Left);
            ui.label(&say(caption), sans(11), b.x, b.y + r * 2.0 + 10.0, b.w, th.ink2);
        }
        BentoBody::List { rows, empty } => {
            if rows.is_empty() { ui.label(empty, sans(12), b.x, b.y, b.w, th.ink2); return; }
            let fit = (b.h / 22.0).floor().max(1.0) as usize;
            let shown = match size { BentoSize::Small => 1, BentoSize::Wide => 3, BentoSize::Large => 8 }.min(fit);
            for (i, r) in rows.iter().take(shown).enumerate() {
                let y = b.y + i as f64 * 22.0;
                let dw = if r.detail.is_some() { 56.0 } else { 0.0 };
                ui.c.line_text(&r.text, sans(12), Rect::new(b.x, y, (b.w - dw).max(10.0), 20.0), th.ink, Align::Left);
                if let Some(d) = &r.detail { ui.c.line_text(d, mono(10), Rect::new(b.right() - dw, y, dw, 20.0), th.ink2, Align::Right); }
            }
        }
        BentoBody::Stack { count, caption } => {
            ui.c.line_text(&count.to_string(), bold(28), Rect::new(b.x, b.y, b.w, 38.0), th.ink, Align::Left);
            ui.label(&say(caption), sans(11), b.x, b.y + 42.0, b.w, th.ink2);
        }
        BentoBody::Level { fraction, low, high, caption } => {
            bar(ui, Rect::new(b.x, b.y + 6.0, b.w, 8.0), Some(*fraction), th.nacho_deep);
            ui.c.line_text(low, sans(10), Rect::new(b.x, b.y + 18.0, b.w / 2.0, 16.0), th.ink2, Align::Left);
            ui.c.line_text(high, sans(10), Rect::new(b.x + b.w / 2.0, b.y + 18.0, b.w / 2.0, 16.0), th.ink2, Align::Right);
            ui.label(&say(caption), sans(11), b.x, b.y + 40.0, b.w, th.ink2);
        }
        BentoBody::Sliders { sliders, rows } => {
            let mut y = b.y;
            for s in sliders {
                ui.c.line_text(&s.label, sans(11), Rect::new(b.x, y, b.w * 0.6, 18.0), th.ink2, Align::Left);
                ui.c.line_text(&s.value, mono(11), Rect::new(b.x + b.w * 0.6, y, b.w * 0.4, 18.0), th.ink, Align::Right);
                bar(ui, Rect::new(b.x, y + 21.0, b.w, 6.0), s.fraction, th.nacho_deep);
                y += 34.0;
            }
            for l in rows {
                if y + 18.0 > b.bottom() + 4.0 { break; }
                ui.c.line_text(&l.label, sans(11), Rect::new(b.x, y, b.w * 0.65, 18.0), th.ink, Align::Left);
                ui.c.line_text(&l.value, sans(11), Rect::new(b.x + b.w * 0.65, y, b.w * 0.35, 18.0), th.ink2, Align::Right);
                y += 20.0;
            }
        }
        BentoBody::Calendar { weekday, day, lines, note } => {
            ui.c.line_text(weekday, sans(11), Rect::new(b.x, b.y, b.w, 16.0), th.ink2, Align::Left);
            ui.c.line_text(day, bold(32), Rect::new(b.x, b.y + 14.0, 70.0, 40.0), th.nacho_ink, Align::Left);
            if size != BentoSize::Small {
                for (i, l) in lines.iter().enumerate() {
                    let y = b.y + 6.0 + i as f64 * 22.0;
                    ui.c.line_text(&l.label, sans(11), Rect::new(b.x + 80.0, y, (b.w - 80.0) * 0.5, 20.0), th.ink2, Align::Left);
                    ui.c.line_text(&l.value, mono(11), Rect::new(b.x + 80.0 + (b.w - 80.0) * 0.5, y, (b.w - 80.0) * 0.5, 20.0), th.ink, Align::Right);
                }
            }
            if let Some(n) = note { ui.label(n, sans(11), b.x, b.y + 56.0, b.w, th.ink2); }
        }
        BentoBody::Usage { big, caption, segments } => {
            ui.c.line_text(big, bold(20), Rect::new(b.x, b.y, b.w, 28.0), th.ink, Align::Left);
            ui.c.line_text(caption, sans(11), Rect::new(b.x, b.y + 28.0, b.w, 18.0), th.ink2, Align::Left);
            let r = Rect::new(b.x, b.y + 52.0, b.w, 10.0);
            ui.c.round(r, 5.0, th.line);
            let mut x = r.x;
            for (i, s) in segments.iter().enumerate() {
                let w = (r.w * s.fraction).min(r.right() - x);
                if w > 1.0 { ui.c.fill(Rect::new(x, r.y, w, r.h), segment_color(i, &th)); x += w; }
            }
            if size != BentoSize::Small {
                for (i, s) in segments.iter().enumerate().filter(|(_, s)| s.label != "Used") {
                    let y = b.y + 68.0 + i as f64 * 0.0;
                    let lx = b.x + i as f64 * (b.w / segments.len().max(1) as f64);
                    ui.c.circle(lx + 4.0, y + 8.0, 3.5, segment_color(i, &th));
                    ui.c.line_text(&s.label, sans(10), Rect::new(lx + 12.0, y, b.w / segments.len().max(1) as f64 - 14.0, 16.0), th.ink2, Align::Left);
                }
            }
        }
        BentoBody::Slots { rows, drop } => {
            let fit = (b.h / 22.0).floor().max(1.0) as usize;
            let shown = rows.len().min(if size == BentoSize::Small { 1 } else { fit.saturating_sub(1).max(1) });
            for (i, r) in rows.iter().take(shown).enumerate() { ui.c.line_text(&r.text, sans(12), Rect::new(b.x, b.y + i as f64 * 22.0, b.w, 20.0), th.ink, Align::Left); }
            let y = b.y + shown as f64 * 22.0;
            if y + 20.0 <= b.bottom() + 6.0 { ui.c.line_text(drop, sans(11), Rect::new(b.x, y, b.w, 20.0), th.ink2, Align::Left); }
        }
        BentoBody::Strip { caption } => { ui.label(caption, sans(12), b.x, b.y, b.w, th.ink2); }
    }
}

fn draw(ui: &mut Ui, ctx: &mut WindowCtx) {
    let th = ui.th;
    let a = app();
    for k in ui.st.keys.clone() { if k.vk == 0x1B { if a.board.edit { a.board.edit = false; ui.st.again = true; } else { ctx.close = true; } } }
    let edit_mode = a.board.edit;

    // Header.
    ui.c.fill(Rect::new(0.0, 0.0, ui.w, HEAD - 8.0), NACHO);
    ui.c.logo(Rect::new(PAD, 12.0, 32.0, 32.0));
    ui.c.line_text("WIDGET BOARD", sign(20), Rect::new(PAD + 42.0, 0.0, 300.0, HEAD - 8.0), ON_NACHO, Align::Left);
    let ew = ui.button_width(if edit_mode { "Done" } else { "Edit" }, true);
    if ui.button(if edit_mode { "boarddone" } else { "boardedit" }, if edit_mode { "Done" } else { "Edit" }, ui.w - PAD - ew, 14.0, Btn::Board, true) { a.board.edit = !a.board.edit; a.board.drag = None; ui.st.again = true; }

    let layout = a.settings.board_layout();
    let hidden = layout.hidden_modules();
    let footer = if edit_mode { 56.0 } else { 0.0 };
    let view = Rect::new(0.0, HEAD, ui.w, (ui.h - HEAD - footer).max(10.0));
    let cols = columns_for_width(ui.w - PAD * 2.0, MIN_CELL, GAP);
    let cw = (ui.w - PAD * 2.0 - GAP * (cols as f64 - 1.0)) / cols as f64;
    let visible: Vec<BentoItem> = layout.visible().into_iter().cloned().collect();
    let (places, rows) = pack(&visible, cols, &[]);
    let rect_of = |p: &Placement, oy: f64| Rect::new(PAD + p.column as f64 * (cw + GAP), oy + p.row as f64 * (CELL_H + GAP), p.columns as f64 * cw + (p.columns as f64 - 1.0) * GAP, p.rows as f64 * CELL_H + (p.rows as f64 - 1.0) * GAP);

    let mut drop_to: Option<(ModuleId, ModuleId)> = None;
    let mut over: Option<ModuleId> = None;
    ui.scroll("boardscroll", view, |ui, top| {
        if visible.is_empty() {
            ui.label(if hidden.is_empty() { "No tiles yet." } else { "All tiles are hidden. Press Edit to bring some back." }, sans(14), PAD, top + 20.0, ui.w - PAD * 2.0, th.ink2);
            return top + 60.0;
        }
        for p in &places {
            let Some(m) = ModuleId::from_key(&p.id) else { continue };
            let Some(item) = layout.item(m).cloned() else { continue };
            let r = rect_of(p, top + 4.0);
            let state = a.state(m);
            let on = state == State::Running;
            let dragging = a.board.drag == Some(m);
            let under = ui.st.inside && r.contains(ui.st.mouse.0, ui.st.mouse.1);
            if a.board.drag.is_some() && under && !dragging { over = Some(m); }
            ui.in_scope(m as u64, |ui| {
                // The tile itself: a click opens the gadget's page; in edit mode a press starts a drag.
                let id = ui.id("tile");
                let resp = ui.hit(id, r, true);
                ui.c.round(r, 12.0, if resp.hover && !edit_mode { th.paper2.mix(NACHO, 0.12) } else { th.paper2 });
                ui.c.stroke_round(r, 12.0, if dragging { th.nacho_ink } else if over == Some(m) { NACHO } else { th.line }, if dragging || over == Some(m) { 2.5 } else { 1.0 });
                if resp.focused && ui.st.keyboard_used { ui.c.stroke_round(r.inset(-2.0, -2.0), 14.0, th.nacho_ink, 2.0); }
                if resp.hover { ui.st.cursor_hand = true; }
                if edit_mode {
                    if resp.pressed && a.board.drag.is_none() { a.board.drag = Some(m); ui.st.again = true; }
                    if resp.focused {
                        for k in ui.st.keys.clone() {
                            match k.vk {
                                0x25 | 0x26 => { later(move || edit(|b| b.step(m, -1))); }
                                0x27 | 0x28 => { later(move || edit(|b| b.step(m, 1))); }
                                0x2E => { later(move || edit(|b| b.set_hidden(m, true))); }
                                _ => {}
                            }
                        }
                    }
                } else if resp.clicked { later(move || crate::pages::open(Some(m))); }

                // Header: the gadget's mark, its name and its state in words, and the switch.
                crate::pages::tile(ui.c, m, r.x + 12.0, r.y + 10.0, 26.0);
                let name = if m == ModuleId::TrayOrganizer && item.size == BentoSize::Small { "Tray" } else { m.name() };
                ui.c.line_text(name, semi(13), Rect::new(r.x + 46.0, r.y + 8.0, (r.w - 46.0 - 56.0).max(20.0), 18.0), th.ink, Align::Left);
                let dot = dot_of(m);
                let dc = match dot { BentoDot::On => th.guac, BentoDot::Error => th.salsa, BentoDot::Off => STATUS_OFF };
                ui.c.circle(r.x + 50.0, r.y + 33.0, 3.5, dc);
                ui.c.line_text(dot.text(), mono(10), Rect::new(r.x + 58.0, r.y + 25.0, (r.w - 58.0 - 8.0).max(20.0), 16.0), th.ink2, Align::Left);
                if !edit_mode && ui.switch("on", r.right() - 52.0, r.y + 10.0, on || state != State::Off) { later(move || app().set_enabled(m, !(app().state(m) != State::Off))); }

                // Body.
                let foot = if edit_mode { 34.0 } else { 0.0 };
                let b = Rect::new(r.x + 14.0, r.y + 50.0, r.w - 28.0, (r.h - 50.0 - 12.0 - foot).max(10.0));
                ui.c.clip(Rect::new(r.x, r.y + 44.0, r.w, r.h - 44.0 - foot));
                // Editing shows what the picked size holds; otherwise the tile is live.
                if edit_mode { ui.label(size_text(m, item.size), sans(11), b.x, b.y, b.w, th.ink2); } else { self::body(ui, &body_of(m), b, item.size); }
                ui.c.unclip();
                if let State::Failed(e) = &state { ui.label(e, sans(10), b.x, r.bottom() - 40.0 - foot, b.w, th.salsa); }

                // Edit controls.
                if edit_mode {
                    let y = r.bottom() - 32.0;
                    let opts: Vec<String> = BentoSize::ALL.iter().map(|s| s.name().chars().next().map(|c| c.to_string()).unwrap_or_default()).collect();
                    let cur = BentoSize::ALL.iter().position(|s| *s == item.size);
                    if let Some(i) = ui.segments("size", &opts, cur, Rect::new(r.x + 10.0, y, 84.0, 24.0), false) { let s = BentoSize::ALL[i]; later(move || edit(|b| b.set_size(m, s))); }
                    let sx = r.right() - 10.0 - 24.0 * 3.0 - 8.0;
                    if ui.button_rect("left", "‹", Rect::new(sx, y, 24.0, 24.0), Btn::Secondary, true, true) { later(move || edit(|b| b.step(m, -1))); }
                    if ui.button_rect("right", "›", Rect::new(sx + 28.0, y, 24.0, 24.0), Btn::Secondary, true, true) { later(move || edit(|b| b.step(m, 1))); }
                    if ui.button_rect("hide", "×", Rect::new(sx + 56.0, y, 24.0, 24.0), Btn::Secondary, true, true) { later(move || edit(|b| b.set_hidden(m, true))); }
                }
            });
        }
        let last = places.iter().map(|p| rect_of(p, top + 4.0).bottom()).fold(top, f64::max);
        let _ = rows;
        last + PAD
    });

    // A drag ends where the pointer is let go.
    if let Some(d) = a.board.drag {
        if !ui.st.down {
            if let Some(t) = over { drop_to = Some((d, t)); }
            a.board.drag = None;
            ui.st.again = true;
        }
    }
    if let Some((d, t)) = drop_to { later(move || edit(|b| b.drop_onto(d, t))); }

    // Edit mode: the hidden tiles wait here to come back.
    if edit_mode {
        let f = Rect::new(0.0, ui.h - footer, ui.w, footer);
        ui.c.fill(f, th.paper2);
        ui.c.line(0.0, f.y, ui.w, f.y, th.line, 1.0);
        if hidden.is_empty() {
            ui.c.line_text("Drag a tile onto another to move it. × hides a tile; hidden tiles come back from here.", sans(12), Rect::new(PAD, f.y, ui.w - PAD * 2.0, f.h), th.ink2, Align::Left);
        } else {
            ui.c.line_text("Hidden:", semi(12), Rect::new(PAD, f.y, 60.0, f.h), th.ink2, Align::Left);
            let mut x = PAD + 62.0;
            for m in hidden {
                let label = format!("+ {}", crate::i18n::t(m.name()));
                let w = ui.button_width(&label, true);
                if x + w > ui.w - PAD { break; }
                if ui.button(&format!("add{}", m.key()), &label, x, f.y + 13.0, Btn::Secondary, true) { later(move || edit(|b| b.set_hidden(m, false))); }
                x += w + 8.0;
            }
        }
    }
}
