//! Capture Float: freezes the screen, lets the user pick an area or a window, and keeps the result floating on top.
use crate::app::{app, later, State, WM_OCR_DONE};
use crate::core::layout::Rect;
use crate::core::settings::ModuleId;
use crate::gfx::*;
use crate::shelf;
use crate::ui::*;
use crate::window::{apply_chrome, dpi_scale, UiWindow};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::core::BOOL;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::CF_DIB;
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode { Area, Window, Text, Color, Live }

pub struct Image { pub px: Vec<u8>, pub w: u32, pub h: u32 }

#[derive(Default)]
pub struct Capture {
    floats: Vec<isize>,
    pub locked: bool,
    pub hidden: bool,
    previous: Option<RECT>,
    selecting: Option<isize>,
    pub taken: u32,
    pub notice: Option<String>,
}

impl Capture {
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "floats": self.floats.len(), "locked": self.locked, "hidden": self.hidden, "taken": self.taken, "selecting": self.selecting.is_some(), "notice": self.notice })
    }
    pub fn float_count(&self) -> usize { self.floats.len() }
    pub fn has_previous(&self) -> bool { self.previous.is_some() }
}

pub fn start() -> std::result::Result<(), String> { let c = &mut app().capture; c.locked = false; c.hidden = false; Ok(()) }

pub fn stop() {
    close_all();
    let c = &mut app().capture;
    if let Some(id) = c.selecting.take() { unsafe { let _ = DestroyWindow(HWND(id as *mut _)); } }
    c.previous = None;
    let dir = app().dir.join("Captures");
    let _ = std::fs::remove_dir_all(dir);
}

fn running() -> bool { app().state(ModuleId::CaptureFloat) == State::Running }

fn virtual_screen() -> RECT {
    unsafe {
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        RECT { left: x, top: y, right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN), bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN) }
    }
}

/// Copies a memory DC's bitmap out as opaque top-down BGRA.
unsafe fn read_bits(dc: HDC, bmp: HBITMAP, w: i32, h: i32) -> Vec<u8> {
    let mut bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
    let mut px = vec![0u8; (w * h * 4) as usize];
    GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
    for p in px.chunks_exact_mut(4) { p[3] = 255; }
    px
}

/// What is on screen in `r`, in physical pixels.
pub fn grab(r: RECT) -> Option<Image> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 { return None; }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY | CAPTUREBLT).is_ok();
        SelectObject(mem, old);
        let px = ok.then(|| read_bits(mem, bmp, w, h));
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        px.map(|px| Image { px, w: w as u32, h: h as u32 })
    }
}

/// A window's own picture, even when partly covered; the visible part of the screen when the app refuses.
fn grab_window(h: HWND) -> Option<Image> {
    unsafe {
        let mut o = RECT::default();
        GetWindowRect(h, &mut o).ok()?;
        let mut v = RECT::default();
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut v as *mut _ as *mut _, 16).is_err() { v = o; }
        let (ow, oh) = (o.right - o.left, o.bottom - o.top);
        if ow <= 0 || oh <= 0 { return None; }
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, ow, oh);
        let old = SelectObject(mem, bmp.into());
        let ok = PrintWindow(h, mem, PRINT_WINDOW_FLAGS(2)).as_bool(); // PW_RENDERFULLCONTENT
        SelectObject(mem, old);
        let full = ok.then(|| read_bits(mem, bmp, ow, oh));
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        let Some(full) = full else { return grab(v) };
        // Crops away the invisible resize border.
        let (cx, cy, cw, ch) = ((v.left - o.left).max(0), (v.top - o.top).max(0), (v.right - v.left).min(ow), (v.bottom - v.top).min(oh));
        let mut px = Vec::with_capacity((cw * ch * 4) as usize);
        for y in cy..(cy + ch).min(oh) {
            let start = ((y * ow + cx) * 4) as usize;
            px.extend_from_slice(&full[start..start + (cw.min(ow - cx) * 4) as usize]);
        }
        Some(Image { px, w: cw.min(ow - cx) as u32, h: (ch.min(oh - cy)) as u32 })
    }
}

// ---------- selection ----------

struct Sel { img: Image, origin: RECT, mode: Mode, start: Option<(f64, f64)>, windows: Vec<(HWND, Rect)>, bitmap: Option<ID2D1Bitmap>, done: bool }

/// Visible top-level windows, front to back, in physical pixels.
fn window_list(own: HWND) -> Vec<(HWND, Rect)> {
    let mut v = vec![];
    unsafe {
        let mut cur = GetTopWindow(None).unwrap_or_default();
        while !cur.0.is_null() {
            if cur != own && IsWindowVisible(cur).as_bool() && !IsIconic(cur).as_bool() {
                let mut cloak = 0u32;
                let _ = DwmGetWindowAttribute(cur, DWMWA_CLOAKED, &mut cloak as *mut _ as *mut _, 4);
                let ex = GetWindowLongW(cur, GWL_EXSTYLE) as u32;
                let mut r = RECT::default();
                if cloak == 0 && ex & WS_EX_TRANSPARENT.0 == 0 && ex & WS_EX_NOACTIVATE.0 == 0 && DwmGetWindowAttribute(cur, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, 16).is_ok() && r.right - r.left > 40 && r.bottom - r.top > 40 {
                    v.push((cur, Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64)));
                }
            }
            cur = GetWindow(cur, GW_HWNDNEXT).unwrap_or_default();
        }
    }
    v
}

pub fn select(mode: Mode) {
    if !running() || app().capture.selecting.is_some() { return; }
    let vs = virtual_screen();
    let Some(img) = grab(vs) else { return };
    let mut sel = Sel { img, origin: vs, mode, start: None, windows: vec![], bitmap: None, done: false };
    let win = UiWindow::create("Capture", WS_POPUP, WS_EX_TOPMOST | WS_EX_TOOLWINDOW, vs.left, vs.top, 10.0, 10.0, Box::new(move |ui, ctx| {
        let scale = ctx.scale;
        if sel.windows.is_empty() { sel.windows = window_list(ctx.hwnd); }
        if sel.bitmap.is_none() { sel.bitmap = ui.c.make_bitmap(&sel.img.px, sel.img.w, sel.img.h); }
        let full = Rect::new(0.0, 0.0, ui.w, ui.h);
        if let Some(b) = &sel.bitmap { ui.c.bitmap(b, full, 1.0); }
        let shade = Color(0.0, 0.0, 0.0, 0.35);
        for k in ui.st.keys.clone() {
            if k.vk == 0x1B { ctx.close = true; }
            if k.vk == 0x20 && matches!(sel.mode, Mode::Area | Mode::Window) {
                sel.mode = if sel.mode == Mode::Window { Mode::Area } else { Mode::Window };
                sel.start = None;
            }
        }
        ui.st.activate = false;
        let m = ui.st.mouse;
        let phys = |p: (f64, f64)| (p.0 * scale + sel.origin.left as f64, p.1 * scale + sel.origin.top as f64);
        let accent = NACHO;
        if sel.mode == Mode::Color {
            // A loupe of 15 × 15 screen pixels under the pointer, and the colour of the middle one.
            let (px, py) = phys(m);
            let (ix, iy) = (px as i64 - sel.origin.left as i64, py as i64 - sel.origin.top as i64);
            let at = |x: i64, y: i64| -> Option<(u8, u8, u8)> {
                if x < 0 || y < 0 || x >= sel.img.w as i64 || y >= sel.img.h as i64 { return None; }
                let o = ((y as u32 * sel.img.w + x as u32) * 4) as usize;
                Some((sel.img.px[o + 2], sel.img.px[o + 1], sel.img.px[o]))
            };
            let cell = 8.0;
            let size = 15.0 * cell;
            let lx = if m.0 + 24.0 + size > ui.w { m.0 - 24.0 - size } else { m.0 + 24.0 };
            let ly = if m.1 + 24.0 + size + 30.0 > ui.h { m.1 - 24.0 - size - 30.0 } else { m.1 + 24.0 };
            let lr = Rect::new(lx, ly, size, size);
            ui.c.fill(lr.inset(-3.0, -3.0), BOARD);
            for dy in -7..=7 {
                for dx in -7..=7 {
                    if let Some((r, g, b)) = at(ix + dx, iy + dy) {
                        ui.c.fill(Rect::new(lx + (dx + 7) as f64 * cell, ly + (dy + 7) as f64 * cell, cell, cell), Color(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0));
                    }
                }
            }
            ui.c.stroke_round(Rect::new(lx + 7.0 * cell, ly + 7.0 * cell, cell, cell), 0.0, NACHO, 2.0);
            let hex = at(ix, iy).map(|(r, g, b)| format!("#{r:02X}{g:02X}{b:02X}"));
            if let Some(h) = &hex {
                let tr = Rect::new(lx - 3.0, ly + size + 3.0, size + 6.0, 28.0);
                ui.c.fill(tr, BOARD);
                ui.c.line_text(h, mono(13), tr, BOARD_INK, Align::Center);
            }
            if ui.st.click.is_some() {
                if let Some(h) = hex { ctx.close = true; sel.done = true; later(move || { copy_text(&h); app().tray.balloon("Pick Color", &format!("Copied {h}")); }); }
            }
            hint(ui, "Click to copy the colour  ·  Esc: cancel");
        } else if sel.mode == Mode::Window {
            let (px, py) = phys(m);
            let hit = sel.windows.iter().find(|w| w.1.contains(px, py)).copied();
            let hole = hit.map(|w| Rect::new((w.1.x - sel.origin.left as f64) / scale, (w.1.y - sel.origin.top as f64) / scale, w.1.w / scale, w.1.h / scale));
            shade_except(ui.c, full, hole, shade);
            if let Some(h) = hole { ui.c.stroke_round(h, 0.0, accent, 3.0); }
            if ui.st.click.is_some() {
                if let Some(w) = hit { let h = w.0; sel.done = true; ctx.close = true; later(move || { if let Some(i) = grab_window(h) { new_float(i, None); } }); }
            }
            hint(ui, "Click a window  ·  Space: drag an area  ·  Esc: cancel");
        } else {
            if ui.st.down && sel.start.is_none() { sel.start = Some(m); }
            let area = sel.start.map(|s| Rect::new(s.0.min(m.0), s.1.min(m.1), (s.0 - m.0).abs(), (s.1 - m.1).abs()));
            shade_except(ui.c, full, area, shade);
            if let Some(a) = area {
                ui.c.stroke_round(a, 0.0, accent, 2.0);
                let label = format!("{} × {}", (a.w * scale) as i32, (a.h * scale) as i32);
                let lw = ui.c.measure(&label, mono(12), 200.0).0 + 14.0;
                let lr = Rect::new(a.x, (a.y - 26.0).max(2.0), lw, 22.0);
                ui.c.round(lr, 5.0, BOARD);
                ui.c.line_text(&label, mono(12), lr, BOARD_INK, Align::Center);
                if ui.st.click.is_some() && !ui.st.down {
                    sel.start = None;
                    if a.w * scale >= 4.0 && a.h * scale >= 4.0 {
                        let (x0, y0) = phys((a.x, a.y));
                        let r = RECT { left: x0 as i32, top: y0 as i32, right: (x0 + a.w * scale) as i32, bottom: (y0 + a.h * scale) as i32 };
                        let i = crop(&sel.img, &sel.origin, r);
                        let mode = sel.mode;
                        ctx.close = true;
                        sel.done = true;
                        later(move || {
                            app().capture.previous = Some(r);
                            match mode { Mode::Text => read_text(i), Mode::Live => live_float(r), _ => new_float(i, Some(r)) }
                        });
                    }
                }
            } else {
                hint(ui, match sel.mode { Mode::Text => "Drag over the text  ·  Esc: cancel", Mode::Live => "Drag over the part of a window to keep live  ·  Esc: cancel", _ => "Drag an area  ·  Space: pick a window  ·  Esc: cancel" });
            }
        }
    }));
    let h = win.hwnd;
    app().capture.selecting = Some(h.0 as isize);
    win.on_close = Some(Box::new(|| { app().capture.selecting = None; }));
    unsafe {
        let none = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &none as *const _ as *const _, 4);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), vs.left, vs.top, vs.right - vs.left, vs.bottom - vs.top, SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(h);
        SetCursor(LoadCursorW(None, IDC_CROSS).ok());
    }
    win.st.inside = true;
    let mut p = POINT::default();
    unsafe { let _ = GetCursorPos(&mut p); }
    let s = dpi_scale(h);
    win.st.mouse = ((p.x - vs.left) as f64 / s, (p.y - vs.top) as f64 / s);
}

fn hint(ui: &mut Ui, s: &str) {
    let w = ui.c.measure(s, medium(14), 800.0).0 + 28.0;
    let r = Rect::new((ui.w - w) / 2.0, 40.0, w, 36.0);
    ui.c.round(r, 8.0, BOARD);
    ui.c.line_text(s, medium(14), r, BOARD_INK, Align::Center);
}

fn shade_except(c: &Canvas, full: Rect, hole: Option<Rect>, col: Color) {
    let Some(h) = hole else { c.fill(full, col); return };
    c.fill(Rect::new(0.0, 0.0, full.w, h.y), col);
    c.fill(Rect::new(0.0, h.bottom(), full.w, full.h - h.bottom()), col);
    c.fill(Rect::new(0.0, h.y, h.x, h.h), col);
    c.fill(Rect::new(h.right(), h.y, full.w - h.right(), h.h), col);
}

fn crop(img: &Image, origin: &RECT, r: RECT) -> Image {
    let x0 = (r.left - origin.left).clamp(0, img.w as i32) as usize;
    let y0 = (r.top - origin.top).clamp(0, img.h as i32) as usize;
    let x1 = (r.right - origin.left).clamp(0, img.w as i32) as usize;
    let y1 = (r.bottom - origin.top).clamp(0, img.h as i32) as usize;
    let mut px = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        let s = y * img.w as usize * 4;
        px.extend_from_slice(&img.px[s + x0 * 4..s + x1 * 4]);
    }
    Image { px, w: (x1 - x0) as u32, h: (y1 - y0) as u32 }
}

pub fn capture_screen() {
    if !running() { return; }
    let (mut p, mut mi) = (POINT::default(), MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() });
    unsafe {
        let _ = GetCursorPos(&mut p);
        let _ = GetMonitorInfoW(MonitorFromPoint(p, MONITOR_DEFAULTTOPRIMARY), &mut mi);
    }
    if let Some(i) = grab(mi.rcMonitor) { new_float(i, None); }
}

pub fn capture_previous() {
    if !running() { return; }
    if let Some(r) = app().capture.previous { if let Some(i) = grab(r) { new_float(i, Some(r)); } }
}

/// For `ctl`: captures a rectangle in physical pixels directly.
pub fn capture_rect(r: RECT) { if let Some(i) = grab(r) { app().capture.previous = Some(r); new_float(i, Some(r)); } }

// ---------- floats ----------

struct FloatState { img: Image, bitmap: Option<ID2D1Bitmap>, aspect: f64, opacity: f64, pinned: bool, draw: Draw, drawing: std::rc::Rc<std::cell::Cell<bool>> }

#[derive(Clone, Copy, PartialEq)]
enum Tool { Pen, Arrow, Box, Text, Mosaic }

#[derive(Clone)]
enum Shape { Pen(Vec<(f64, f64)>), Arrow((f64, f64), (f64, f64)), Box((f64, f64), (f64, f64)), Text((f64, f64), String) }

/// A drawn mark, in the picture's own pixels so it stays put when the float is resized.
#[derive(Clone)]
struct Mark { shape: Shape, color: Color }

enum Undo { Mark, Pixels(Vec<u8>) }

struct Draw { tool: Tool, color: usize, marks: Vec<Mark>, undo: Vec<Undo>, active: bool, mosaic_from: Option<(f64, f64)>, typing: bool }

impl Default for Draw { fn default() -> Self { Draw { tool: Tool::Box, color: 0, marks: vec![], undo: vec![], active: false, mosaic_from: None, typing: false } } }

const INKS: [Color; 6] = [rgb(0xE5484D), NACHO, rgb(0x3FA34D), rgb(0x2F7DE1), rgb(0x111111), rgb(0xFFFFFF)];

fn line_w(img_w: f64) -> f64 { (img_w / 250.0).clamp(3.0, 10.0) }

/// Draws the marks, mapping picture pixels by `s` (picture → view).
fn draw_marks(c: &Canvas, marks: &[Mark], s: f64, img_w: f64) {
    let w = line_w(img_w) * s;
    let p = |q: (f64, f64)| (q.0 * s, q.1 * s);
    for m in marks {
        match &m.shape {
            Shape::Pen(pts) => { let v: Vec<(f64, f64)> = pts.iter().map(|q| p(*q)).collect(); c.chart(&v, 0.0, m.color, w, None); }
            Shape::Box(a, b) => { let (a, b) = (p(*a), p(*b)); c.stroke_round(Rect::new(a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs(), (a.1 - b.1).abs()), w, m.color, w); }
            Shape::Arrow(a, b) => {
                let (a, b) = (p(*a), p(*b));
                c.line(a.0, a.1, b.0, b.1, m.color, w);
                let ang = (b.1 - a.1).atan2(b.0 - a.0);
                let len = w * 4.5;
                let tip = |d: f64| (b.0 - len * (ang + d).cos(), b.1 - len * (ang + d).sin());
                c.polygon(&[b, tip(0.45), tip(-0.45)], m.color);
            }
            Shape::Text(at, t) => {
                let size = ((line_w(img_w) * 6.0) * s).max(10.0) as u32;
                let (x, y) = p(*at);
                c.text(t, bold(size), Rect::new(x, y, 4000.0, size as f64 * 1.6), m.color, Align::Left);
            }
        }
    }
}

/// Blocks of 12 × 12 picture pixels averaged in place: the region cannot be read back from the result.
fn mosaic(img: &mut Image, a: (f64, f64), b: (f64, f64)) {
    let (x0, y0) = (a.0.min(b.0).max(0.0) as u32, a.1.min(b.1).max(0.0) as u32);
    let (x1, y1) = ((a.0.max(b.0) as u32).min(img.w), (a.1.max(b.1) as u32).min(img.h));
    let n = 12;
    let mut by = y0;
    while by < y1 {
        let mut bx = x0;
        while bx < x1 {
            let (ex, ey) = ((bx + n).min(x1), (by + n).min(y1));
            let mut sum = [0u32; 3];
            let cnt = ((ex - bx) * (ey - by)).max(1);
            for y in by..ey { for x in bx..ex { let o = ((y * img.w + x) * 4) as usize; for k in 0..3 { sum[k] += img.px[o + k] as u32; } } }
            for y in by..ey { for x in bx..ex { let o = ((y * img.w + x) * 4) as usize; for k in 0..3 { img.px[o + k] = (sum[k] / cnt) as u8; } } }
            bx += n;
        }
        by += n;
    }
}

/// Burns the marks into the picture, so copying, saving and dragging carry them.
fn flatten(st: &mut FloatState) {
    if st.draw.marks.is_empty() { return; }
    let (w, h) = (st.img.w, st.img.h);
    let marks = st.draw.marks.clone();
    let src = clone_image(&st.img);
    if let Ok(px) = render_offscreen(w, h, 96.0, None, |c| {
        if let Some(b) = c.make_bitmap(&src.px, w, h) { c.bitmap(&b, Rect::new(0.0, 0.0, w as f64, h as f64), 1.0); }
        draw_marks(c, &marks, 1.0, w as f64);
    }) { st.img.px = px; }
    st.draw.marks.clear();
    st.draw.undo.clear();
    st.bitmap = None;
}

fn float_ids() -> Vec<HWND> { app().capture.floats.iter().map(|i| HWND(*i as *mut _)).collect() }

fn set_click_through(h: HWND, on: bool) {
    unsafe {
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        let v = if on { ex | WS_EX_TRANSPARENT.0 } else { ex & !WS_EX_TRANSPARENT.0 };
        SetWindowLongW(h, GWL_EXSTYLE, v as i32);
    }
}

fn set_opacity(h: HWND, o: f64) { unsafe { let _ = SetLayeredWindowAttributes(h, COLORREF(0), (o.clamp(0.1, 1.0) * 255.0) as u8, LWA_ALPHA); } }

pub fn new_float(img: Image, place: Option<RECT>) {
    if img.w == 0 || img.h == 0 { return; }
    let a = app();
    a.capture.taken += 1;
    let (pw, ph) = (img.w as i32, img.h as i32);
    let aspect = pw as f64 / ph as f64;
    let mut p = POINT::default();
    unsafe { let _ = GetCursorPos(&mut p); }
    let (x, y) = place.map_or((p.x - pw / 2, p.y - ph / 2), |r| (r.left, r.top));
    let drawing = std::rc::Rc::new(std::cell::Cell::new(false));
    let drawing_msg = drawing.clone();
    let mut st = FloatState { aspect, img, bitmap: None, opacity: 1.0, pinned: true, draw: Draw::default(), drawing };
    let win = UiWindow::create("Capture Float", WS_POPUP, WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW, x, y, 10.0, 10.0, Box::new(move |ui, ctx| {
        let h = ctx.hwnd;
        if st.bitmap.is_none() { st.bitmap = ui.c.make_bitmap(&st.img.px, st.img.w, st.img.h); }
        let full = Rect::new(0.0, 0.0, ui.w, ui.h);
        if let Some(b) = &st.bitmap { ui.c.bitmap(b, full, 1.0); }
        let s = ui.w / st.img.w as f64;
        draw_marks(ui.c, &st.draw.marks, s, st.img.w as f64);
        if st.drawing.get() { draw_mode(ui, &mut st, s); }
        let ctrl = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x11) < 0 };
        if ui.st.wheel != 0.0 && !ctrl {
            st.opacity = (st.opacity + ui.st.wheel / 60.0 * 0.05).clamp(0.1, 1.0);
            set_opacity(h, st.opacity);
        }
        if ui.st.wheel != 0.0 && ctrl { resize_by(h, 1.0 + ui.st.wheel / 60.0 * 0.05, st.aspect); }
        for k in ui.st.keys.clone() {
            match (k.vk, k.ctrl) {
                (0x1B, _) if st.drawing.get() => { st.drawing.set(false); st.draw.typing = false; flatten(&mut st); }
                (0x1B, _) => ctx.close = true,
                (0x43, true) if !st.draw.typing => { flatten(&mut st); copy_image(&st.img) }
                (0x53, true) => { flatten(&mut st); save_as(&st.img) }
                (0x5A, true) => undo_mark(&mut st),
                (0x45, true) => { st.drawing.set(!st.drawing.get()); if !st.drawing.get() { flatten(&mut st); } }
                _ => {}
            }
        }
        if ui.st.double_click && !st.drawing.get() { ctx.close = true; }
        let over = ui.st.inside;
        if over && ui.w >= 90.0 && ui.h >= 40.0 {
            let mut bx = ui.w - 30.0;
            let button = |ui: &mut Ui, key: &str, x: f64, code: u32| {
                let r = Rect::new(x, 6.0, 24.0, 24.0);
                let id = ui.id(key);
                let resp = ui.hit(id, r, false);
                ui.c.round(r, 6.0, if resp.hover { BOARD_LINE } else { BOARD.alpha(0.85) });
                ui.c.glyph(code, 12, r, BOARD_INK);
                resp.clicked
            };
            if button(ui, "close", bx, 0xE711) { ctx.close = true; }
            bx -= 28.0;
            if button(ui, "pin", bx, if st.pinned { 0xE718 } else { 0xE77A }) {
                st.pinned = !st.pinned;
                unsafe { let _ = SetWindowPos(h, Some(if st.pinned { HWND_TOPMOST } else { HWND_NOTOPMOST }), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
            }
            bx -= 28.0;
            if button(ui, "text", bx, 0xE8C8) { let i = clone_image(&st.img); later(move || read_text(i)); }
            bx -= 28.0;
            if button(ui, "draw", bx, 0xE70F) { st.drawing.set(!st.drawing.get()); if !st.drawing.get() { flatten(&mut st); } }
            bx -= 28.0;
            // The share handle: dragging it carries the picture into another app.
            let r = Rect::new(bx, 6.0, 24.0, 24.0);
            let id = ui.id("share");
            let resp = ui.hit(id, r, false);
            ui.c.round(r, 6.0, if resp.hover { BOARD_LINE } else { BOARD.alpha(0.85) });
            ui.c.glyph(0xE72D, 12, r, BOARD_INK);
            if resp.pressed { flatten(&mut st); let i = clone_image(&st.img); ui.st.down = false; later(move || drag_out(h, i)); }
            // The resize grip.
            ui.c.polygon(&[(ui.w, ui.h), (ui.w - 14.0, ui.h), (ui.w, ui.h - 14.0)], NACHO);
        }
        if ui.st.right_click.is_some() && !st.drawing.get() {
            flatten(&mut st);
            let i = clone_image(&st.img);
            let pinned = st.pinned;
            later(move || float_menu(h, i, pinned));
        }
    }));
    let h = win.hwnd;
    a.capture.floats.push(h.0 as isize);
    let rounded = a.settings.options.capture_rounded_corners;
    win.on_close = Some(Box::new(move || { let f = &mut app().capture.floats; f.retain(|x| *x != h.0 as isize); app().changed(); }));
    win.set_msg(Box::new(move |hw, msg, _wp, lp| unsafe { if drawing_msg.get() && msg == WM_LBUTTONDOWN && !over_buttons(hw, lp) { return None; } float_msg(hw, msg, lp, aspect) }));
    unsafe {
        if !rounded {
            let none = DWMWCP_DONOTROUND;
            let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &none as *const _ as *const _, 4);
        } else { apply_chrome(h); }
        set_opacity(h, 1.0);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, pw, ph, SWP_SHOWWINDOW);
        if a.capture.locked { set_click_through(h, true); }
        if a.capture.hidden { let _ = ShowWindow(h, SW_HIDE); }
        let _ = SetForegroundWindow(h);
    }
    a.changed();
}

fn clone_image(i: &Image) -> Image { Image { px: i.px.clone(), w: i.w, h: i.h } }

/// The row of buttons along the top, and the drawing toolbar along the bottom while drawing.
unsafe fn over_buttons(h: HWND, lp: LPARAM) -> bool {
    let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
    let s = dpi_scale(h);
    let mut rc = RECT::default();
    let _ = GetClientRect(h, &mut rc);
    (y < (32.0 * s) as i32 && x > rc.right - (150.0 * s) as i32) || y > rc.bottom - (44.0 * s) as i32
}

fn undo_mark(st: &mut FloatState) {
    match st.draw.undo.pop() {
        Some(Undo::Mark) => { st.draw.marks.pop(); }
        Some(Undo::Pixels(px)) => { st.img.px = px; st.bitmap = None; }
        None => {}
    }
}

/// Drawing on the float: the toolbar, and pointer drags that become marks.
fn draw_mode(ui: &mut Ui, st: &mut FloatState, s: f64) {
    let bar = Rect::new(0.0, ui.h - 40.0, ui.w, 40.0);
    ui.c.fill(bar, BOARD.alpha(0.92));
    let tools = [(Tool::Box, 0xE7FB), (Tool::Arrow, 0xE7EA), (Tool::Pen, 0xEE56), (Tool::Text, 0xE8D2), (Tool::Mosaic, 0xE7F4)];
    let mut x = 8.0;
    for (t, code) in tools {
        let r = Rect::new(x, bar.y + 6.0, 28.0, 28.0);
        let id = ui.id(&format!("tool{code}"));
        let resp = ui.hit(id, r, false);
        if st.draw.tool == t { ui.c.round(r, 6.0, NACHO); } else if resp.hover { ui.c.round(r, 6.0, BOARD_LINE); }
        ui.c.glyph(code, 13, r, if st.draw.tool == t { ON_NACHO } else { BOARD_INK });
        if resp.clicked { st.draw.tool = t; st.draw.typing = false; }
        x += 32.0;
    }
    x += 6.0;
    for (i, c) in INKS.iter().enumerate() {
        let r = Rect::new(x, bar.y + 10.0, 20.0, 20.0);
        let id = ui.id(&format!("ink{i}"));
        let resp = ui.hit(id, r, false);
        ui.c.circle(r.cx(), r.cy(), 9.0, *c);
        if st.draw.color == i || resp.hover { ui.c.ring(r.cx(), r.cy(), 11.0, NACHO, 2.0); }
        if resp.clicked { st.draw.color = i; }
        x += 24.0;
    }
    let undo = Rect::new(ui.w - 70.0, bar.y + 6.0, 28.0, 28.0);
    let uid = ui.id("undo");
    let ur = ui.hit(uid, undo, false);
    if ur.hover { ui.c.round(undo, 6.0, BOARD_LINE); }
    ui.c.glyph(0xE7A7, 13, undo, BOARD_INK);
    if ur.clicked { undo_mark(st); }
    let done = Rect::new(ui.w - 38.0, bar.y + 6.0, 30.0, 28.0);
    let did = ui.id("done");
    let dr = ui.hit(did, done, false);
    ui.c.round(done, 6.0, NACHO);
    ui.c.glyph(0xE73E, 13, done, ON_NACHO);
    if dr.clicked { st.drawing.set(false); st.draw.typing = false; flatten(st); return; }

    // Typing goes into the text mark being written.
    if st.draw.typing {
        if let Some(Mark { shape: Shape::Text(_, t), .. }) = st.draw.marks.last_mut() {
            for ch in ui.st.chars.clone() { if ch == '\u{8}' { t.pop(); } else if ch == '\r' { st.draw.typing = false; } else if !ch.is_control() { t.push(ch); } }
        }
        ui.st.chars.clear();
    }
    let m = (ui.st.mouse.0 / s, ui.st.mouse.1 / s);
    let in_pic = ui.st.mouse.1 < bar.y && ui.st.mouse.1 > 0.0;
    let color = INKS[st.draw.color];
    if ui.st.down && in_pic && !st.draw.active && st.draw.mosaic_from.is_none() {
        st.draw.typing = false;
        match st.draw.tool {
            Tool::Pen => { st.draw.marks.push(Mark { shape: Shape::Pen(vec![m]), color }); st.draw.undo.push(Undo::Mark); st.draw.active = true; }
            Tool::Arrow => { st.draw.marks.push(Mark { shape: Shape::Arrow(m, m), color }); st.draw.undo.push(Undo::Mark); st.draw.active = true; }
            Tool::Box => { st.draw.marks.push(Mark { shape: Shape::Box(m, m), color }); st.draw.undo.push(Undo::Mark); st.draw.active = true; }
            Tool::Mosaic => st.draw.mosaic_from = Some(m),
            Tool::Text => {}
        }
    } else if ui.st.down && st.draw.active {
        if let Some(mark) = st.draw.marks.last_mut() {
            match &mut mark.shape { Shape::Pen(p) => p.push(m), Shape::Arrow(_, b) | Shape::Box(_, b) => *b = m, _ => {} }
        }
    } else if !ui.st.down && st.draw.active { st.draw.active = false; }
    if let Some(a) = st.draw.mosaic_from {
        let r = Rect::new(a.0.min(m.0) * s, a.1.min(m.1) * s, (a.0 - m.0).abs() * s, (a.1 - m.1).abs() * s);
        ui.c.stroke_round(r, 0.0, NACHO, 2.0);
        if !ui.st.down {
            st.draw.mosaic_from = None;
            st.draw.undo.push(Undo::Pixels(st.img.px.clone()));
            mosaic(&mut st.img, a, m);
            st.bitmap = None;
        }
    }
    if st.draw.tool == Tool::Text && ui.st.click.is_some() && in_pic && !st.draw.typing {
        st.draw.marks.push(Mark { shape: Shape::Text(m, String::new()), color });
        st.draw.undo.push(Undo::Mark);
        st.draw.typing = true;
    }
    ui.st.again = ui.st.again || st.draw.active;
}

// ---------- live region ----------

/// Floats a part of another window that keeps updating (a video, a dashboard), drawn by Windows itself.
pub fn live_float(r: RECT) {
    let (cx, cy) = ((r.left + r.right) as f64 / 2.0, (r.top + r.bottom) as f64 / 2.0);
    let Some(src) = window_list(HWND::default()).into_iter().find(|w| w.1.contains(cx, cy)).map(|w| w.0) else { return };
    let mut wr = RECT::default();
    unsafe { let _ = GetWindowRect(src, &mut wr); }
    let source = RECT { left: r.left - wr.left, top: r.top - wr.top, right: r.right - wr.left, bottom: r.bottom - wr.top };
    let (pw, ph) = (r.right - r.left, r.bottom - r.top);
    if pw < 8 || ph < 8 { return; }
    let thumb = std::rc::Rc::new(std::cell::Cell::new(0isize));
    let thumb_draw = thumb.clone();
    let mut title = [0u16; 128];
    let n = unsafe { GetWindowTextW(src, &mut title) } as usize;
    let name = String::from_utf16_lossy(&title[..n]);
    let mut opacity = 255u8;
    let win = UiWindow::create("Live Float", WS_POPUP, WS_EX_TOPMOST | WS_EX_TOOLWINDOW, r.left, r.top - 28, 10.0, 10.0, Box::new(move |ui, ctx| {
        ui.c.fill(Rect::new(0.0, 0.0, ui.w, 28.0), BOARD);
        ui.c.line_text("LIVE", mono(11), Rect::new(10.0, 0.0, 40.0, 28.0), BOARD_GUAC, Align::Left);
        ui.c.line_text(&name, sans(11), Rect::new(48.0, 0.0, ui.w - 84.0, 28.0), BOARD_MUTE, Align::Left);
        let cr = Rect::new(ui.w - 26.0, 4.0, 20.0, 20.0);
        let id = ui.id("close");
        let resp = ui.hit(id, cr, false);
        if resp.hover { ui.c.round(cr, 5.0, BOARD_LINE); }
        ui.c.glyph(0xE711, 10, cr, BOARD_INK);
        if resp.clicked { ctx.close = true; }
        for k in ui.st.keys.clone() { if k.vk == 0x1B { ctx.close = true; } }
        if ui.st.wheel != 0.0 {
            opacity = (opacity as f64 + ui.st.wheel / 60.0 * 12.0).clamp(40.0, 255.0) as u8;
            update_thumb(ctx.hwnd, thumb_draw.get(), source, opacity);
        }
    }));
    let h = win.hwnd;
    let aspect = pw as f64 / ph as f64;
    win.set_msg(Box::new(move |hw, msg, _wp, lp| unsafe {
        match msg {
            WM_LBUTTONDOWN => {
                let x = (lp.0 & 0xFFFF) as i16 as i32;
                let mut rc = RECT::default();
                let _ = GetClientRect(hw, &mut rc);
                if x < rc.right - (30.0 * dpi_scale(hw)) as i32 {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                    let _ = SendMessageW(hw, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), Some(lp));
                    return Some(LRESULT(0));
                }
                None
            }
            WM_NCHITTEST => {
                // The lower-right corner resizes; everything else is the window's own.
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                let mut wr = RECT::default();
                let _ = GetWindowRect(hw, &mut wr);
                if x > wr.right - 16 && y > wr.bottom - 16 { return Some(LRESULT(HTBOTTOMRIGHT as isize)); }
                None
            }
            WM_SIZING => {
                let r = &mut *(lp.0 as *mut RECT);
                let head = (28.0 * dpi_scale(hw)) as i32;
                r.bottom = r.top + head + ((r.right - r.left) as f64 / aspect) as i32;
                Some(LRESULT(1))
            }
            WM_SIZE => { update_thumb(hw, thumb.get(), source, 255); None }
            WM_DESTROY => { if thumb.get() != 0 { let _ = DwmUnregisterThumbnail(thumb.get()); } None }
            _ => None,
        }
    }));
    unsafe {
        let head = (28.0 * dpi_scale(h)) as i32;
        if let Ok(t) = DwmRegisterThumbnail(h, src) {
            let id = t;
            app().capture.taken += 1;
            let _ = SetWindowPos(h, Some(HWND_TOPMOST), r.left, r.top - head, pw, ph + head, SWP_SHOWWINDOW);
            // The id is only known now; the message handler reads it from the shared cell.
            let ptr = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut UiWindow;
            let _ = ptr;
            set_thumb_id(h, id);
            update_thumb(h, id, source, 255);
        } else { let _ = DestroyWindow(h); }
    }
    app().changed();
}

thread_local! { static THUMBS: std::cell::RefCell<std::collections::HashMap<isize, isize>> = Default::default(); }

fn set_thumb_id(h: HWND, id: isize) { THUMBS.with(|t| t.borrow_mut().insert(h.0 as isize, id)); }

fn update_thumb(h: HWND, id: isize, source: RECT, opacity: u8) {
    let id = if id != 0 { id } else { THUMBS.with(|t| t.borrow().get(&(h.0 as isize)).copied().unwrap_or(0)) };
    if id == 0 { return; }
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(h, &mut rc);
        let head = (28.0 * dpi_scale(h)) as i32;
        let props = DWM_THUMBNAIL_PROPERTIES { dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_VISIBLE | DWM_TNP_OPACITY,
            rcDestination: RECT { left: 0, top: head, right: rc.right, bottom: rc.bottom }, rcSource: source, opacity, fVisible: BOOL(1), fSourceClientAreaOnly: BOOL(0) };
        let _ = DwmUpdateThumbnailProperties(id, &props);
    }
}

unsafe fn float_msg(h: HWND, msg: u32, lp: LPARAM, aspect: f64) -> Option<LRESULT> {
    match msg {
        // An empty spot moves the float; the corner resizes it. Buttons keep their own clicks.
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
            let s = dpi_scale(h);
            let mut rc = RECT::default();
            let _ = GetClientRect(h, &mut rc);
            let grip = (14.0 * s) as i32;
            if x >= rc.right - grip && y >= rc.bottom - grip {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                let _ = SendMessageW(h, WM_NCLBUTTONDOWN, Some(WPARAM(HTBOTTOMRIGHT as usize)), Some(lp));
                return Some(LRESULT(0));
            }
            let over_button = over_buttons(h, lp);
            let _ = (x, y);
            if !over_button {
                let _ = SetForegroundWindow(h);
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                let _ = SendMessageW(h, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), Some(lp));
                return Some(LRESULT(0));
            }
            None
        }
        WM_SIZING => {
            let r = &mut *(lp.0 as *mut RECT);
            r.bottom = r.top + ((r.right - r.left) as f64 / aspect) as i32;
            Some(LRESULT(1))
        }
        WM_GETMINMAXINFO => {
            let m = &mut *(lp.0 as *mut MINMAXINFO);
            m.ptMinTrackSize = POINT { x: 40, y: 30 };
            None
        }
        _ => None,
    }
}

fn resize_by(h: HWND, f: f64, aspect: f64) {
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let w = (((r.right - r.left) as f64 * f) as i32).clamp(40, 8000);
        let _ = SetWindowPos(h, None, 0, 0, w, (w as f64 / aspect) as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn float_menu(h: HWND, img: Image, pinned: bool) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, 1, &HSTRING::from(crate::i18n::t("Copy\tCtrl+C").as_ref()));
        let _ = AppendMenuW(menu, MF_STRING, 2, &HSTRING::from(crate::i18n::t("Save as PNG…\tCtrl+S").as_ref()));
        let _ = AppendMenuW(menu, MF_STRING, 3, &HSTRING::from(crate::i18n::t("Copy Text").as_ref()));
        let pin = HSTRING::from(crate::i18n::t(if pinned { "Unpin" } else { "Pin Again" }).as_ref());
        let _ = AppendMenuW(menu, MF_STRING, 4, &pin);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, 5, &HSTRING::from(crate::i18n::t("Close\tEsc").as_ref()));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, h, None).0;
        let _ = DestroyMenu(menu);
        match cmd {
            1 => copy_image(&img),
            2 => save_as(&img),
            3 => read_text(img),
            4 => { let _ = SetWindowPos(h, Some(if pinned { HWND_NOTOPMOST } else { HWND_TOPMOST }), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
            5 => { let _ = DestroyWindow(h); }
            _ => {}
        }
    }
}

/// Puts a top-down BGRA picture on the clipboard as a bottom-up DIB.
pub fn copy_image(i: &Image) {
    unsafe {
        let head = std::mem::size_of::<BITMAPINFOHEADER>();
        let size = head + i.px.len();
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, size) else { return };
        let p = GlobalLock(mem) as *mut u8;
        if p.is_null() { let _ = GlobalFree(Some(mem)); return; }
        let bh = BITMAPINFOHEADER { biSize: head as u32, biWidth: i.w as i32, biHeight: i.h as i32, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
        std::ptr::copy_nonoverlapping(&bh as *const _ as *const u8, p, head);
        let row = i.w as usize * 4;
        for y in 0..i.h as usize {
            let src = (i.h as usize - 1 - y) * row;
            std::ptr::copy_nonoverlapping(i.px[src..].as_ptr(), p.add(head + y * row), row);
        }
        let _ = GlobalUnlock(mem);
        if OpenClipboard(Some(app().main)).is_ok() {
            let _ = EmptyClipboard();
            if SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(mem.0))).is_err() { let _ = GlobalFree(Some(mem)); }
            let _ = CloseClipboard();
        } else { let _ = GlobalFree(Some(mem)); }
    }
}

pub fn copy_text(s: &str) {
    unsafe {
        let w: Vec<u16> = s.encode_utf16().chain([0]).collect();
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) else { return };
        let p = GlobalLock(mem) as *mut u16;
        if p.is_null() { let _ = GlobalFree(Some(mem)); return; }
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
        let _ = GlobalUnlock(mem);
        if OpenClipboard(Some(app().main)).is_ok() {
            let _ = EmptyClipboard();
            const CF_UNICODETEXT: u32 = 13;
            if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).is_err() { let _ = GlobalFree(Some(mem)); }
            let _ = CloseClipboard();
        } else { let _ = GlobalFree(Some(mem)); }
    }
}

fn save_as(i: &Image) {
    unsafe {
        let mut file = [0u16; 1024];
        for (n, c) in "Capture.png".encode_utf16().enumerate() { file[n] = c; }
        let filter: Vec<u16> = "PNG image\0*.png\0\0".encode_utf16().collect();
        let mut o = OPENFILENAMEW { lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32, lpstrFilter: PCWSTR(filter.as_ptr()), lpstrFile: PWSTR(file.as_mut_ptr()), nMaxFile: file.len() as u32, lpstrDefExt: w!("png"), Flags: OFN_OVERWRITEPROMPT | OFN_NOCHANGEDIR, ..Default::default() };
        if !GetSaveFileNameW(&mut o).as_bool() { return; }
        let n = file.iter().position(|c| *c == 0).unwrap_or(0);
        let path = std::path::PathBuf::from(String::from_utf16_lossy(&file[..n]));
        if save_png(&path, &i.px, i.w, i.h).is_err() { app().tray.balloon("Capture Float", "The picture could not be saved there."); }
    }
}

fn drag_out(h: HWND, i: Image) {
    let dir = app().dir.join("Captures");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("Capture {}.png", app().capture.taken));
    if save_png(&path, &i.px, i.w, i.h).is_ok() { shelf::drag_paths(h, &[path], None); }
}

// ---------- text ----------

/// Recognition runs off the UI thread; QR codes are tried first because they are exact.
pub fn read_text(i: Image) {
    let Some(png) = encode_png(&i) else { return };
    let (w, h, gray) = (i.w as usize, i.h as usize, grayscale(&i));
    let main = app().main.0 as isize;
    std::thread::spawn(move || {
        let mut text = read_qr(w, h, &gray).unwrap_or_default();
        if text.is_empty() { text = ocr(&png).unwrap_or_default(); }
        let boxed = Box::into_raw(Box::new(text));
        unsafe { let _ = PostMessageW(Some(HWND(main as *mut _)), WM_OCR_DONE, WPARAM(0), LPARAM(boxed as isize)); }
    });
}

fn grayscale(i: &Image) -> Vec<u8> { i.px.chunks_exact(4).map(|p| ((p[0] as u32 * 29 + p[1] as u32 * 150 + p[2] as u32 * 77) >> 8) as u8).collect() }

fn read_qr(w: usize, h: usize, gray: &[u8]) -> Option<String> {
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| gray[y * w + x]);
    img.detect_grids().into_iter().find_map(|g| g.decode().ok().map(|x| x.1))
}

fn encode_png(i: &Image) -> Option<Vec<u8>> {
    let path = std::env::temp_dir().join(format!("nacho-ocr-{}.png", std::process::id()));
    save_png(&path, &i.px, i.w, i.h).ok()?;
    let bytes = std::fs::read(&path).ok();
    let _ = std::fs::remove_file(path);
    bytes
}

fn ocr(png: &[u8]) -> windows::core::Result<String> {
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(png)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    writer.DetachStream()?;
    stream.Seek(0)?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let bitmap = decoder.GetSoftwareBitmapAsync()?.join()?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let mut lines = vec![];
    for l in result.Lines()? { lines.push(l.Text()?.to_string()); }
    Ok(lines.join("\n"))
}

pub fn on_text_done(p: *mut String) {
    let text = unsafe { *Box::from_raw(p) };
    let a = app();
    if text.trim().is_empty() {
        a.capture.notice = Some("No text or QR code found. Windows reads the languages installed in Settings.".into());
        a.tray.balloon("Copy Text", "No text or QR code found.");
    } else {
        copy_text(&text);
        a.capture.notice = None;
        a.tray.balloon("Copy Text", &format!("Copied {} characters.", text.chars().count()));
    }
    if a.debug_ctl { let _ = std::fs::write(a.dir.join("last-text.txt"), &text); }
    a.changed();
}

pub fn toggle_lock() {
    let c = &mut app().capture;
    c.locked = !c.locked;
    let on = c.locked;
    for h in float_ids() { set_click_through(h, on); }
    app().changed();
}

pub fn toggle_hide() {
    let c = &mut app().capture;
    c.hidden = !c.hidden;
    let hide = c.hidden;
    for h in float_ids() { unsafe { let _ = ShowWindow(h, if hide { SW_HIDE } else { SW_SHOWNOACTIVATE }); } }
    app().changed();
}

pub fn close_all() { for h in float_ids() { unsafe { let _ = DestroyWindow(h); } } }
