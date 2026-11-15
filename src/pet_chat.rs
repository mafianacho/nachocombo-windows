//! The chat bubble of Nacho Pet: a window that follows the pet, with a tail pointing at it. What it shows comes from the
//! core (`PetTranscript` items, `PetSessionState`); this file lays the items out and paints them.
use crate::app::{app, later};
use crate::core::layout::Rect;
use crate::core::pet::card_rules::PetCardKey;
use crate::core::pet::chat_layout::{PetChatEdge, PetChatLayout, PetChatSize, PetRect};
use crate::core::pet::composer::{plan_send, send_button, DigitEdit, SendButton, SendPlan};
use crate::core::pet::design_tokens::pet_tokens::{radius as rad, size as tok, space};
use crate::core::pet::markdown::PetMarkdown;
use crate::core::pet::session::PetAuthPhase;
use crate::core::pet::transcript::*;
use crate::gfx::*;
use crate::pet_theme::{self, Pal};
use crate::pet_ui as pet;
use crate::ui::*;
use crate::window::{UiWindow, WindowCtx};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

const HEADER: f64 = tok::HEADER_HEIGHT;
const TAIL: f64 = tok::CHAT_TAIL;
const TAIL_HALF: f64 = 10.0;
const EDGE: f64 = 6.0;

fn hwnd() -> Option<HWND> { app().pet.chat.hwnd.map(|h| HWND(h as *mut _)) }

pub fn invalidate() { if let Some(h) = hwnd() { unsafe { let _ = InvalidateRect(Some(h), None, false); } } }

pub fn close() { if let Some(h) = hwnd() { unsafe { let _ = DestroyWindow(h); } } app().pet.chat.hwnd = None; }

pub fn open() {
    if let Some(h) = hwnd() {
        follow();
        unsafe { let _ = ShowWindow(h, SW_SHOW); let _ = SetForegroundWindow(h); }
        invalidate();
        return;
    }
    let win = UiWindow::create("Nacho", WS_POPUP, WS_EX_TOOLWINDOW | WS_EX_TOPMOST, 0, 0, tok::CHAT_WIDTH, tok::CHAT_HEIGHT + TAIL, Box::new(draw));
    win.set_msg(Box::new(on_message));
    win.on_close = Some(Box::new(|| { let c = &mut app().pet.chat; c.hwnd = None; c.sig = 0; pet::escape_recording(); }));
    let h = win.hwnd;
    let c = &mut app().pet.chat;
    c.hwnd = Some(h.0 as isize);
    c.sig = 0;
    c.pinned = true;
    c.taken_back = None;
    follow();
    win.show();
    if let Some(s) = pet::session() { s.touch(); s.open(); }
    // The cursor starts in the input field.
    win.st.focus = Some(id_of("input"));
}

// ---------- where it is ----------

struct Geometry { x: f64, y: f64, w: f64, h: f64, tail_on_top: bool, tail_from_right: f64, scale: f64 }

fn work_area_of(h: HWND) -> Option<Rect> {
    unsafe {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() { return None; }
        let r = mi.rcWork;
        Some(Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64))
    }
}

fn geometry(body_override: Option<PetChatSize>) -> Option<Geometry> {
    let pet_rect = pet::pet_rect()?;
    let pet_hwnd = app().pet.pet_hwnd()?;
    let scale = crate::window::dpi_scale(pet_hwnd);
    let work = work_area_of(pet_hwnd)?;
    let d = |r: Rect| PetRect::from_y_down(r.x / scale, r.y / scale, r.w / scale, r.h / scale);
    let (pet_d, screen_d) = (d(pet_rect), d(work));
    let wanted = body_override.or(app().pet.settings.chat_size);
    let body = PetChatLayout::clamp(wanted, Some(screen_d));
    let p = PetChatLayout::place(body, pet_d, screen_d);
    Some(Geometry { x: p.x * scale, y: p.window_top_y_down() * scale, w: body.width * scale, h: p.window_height() * scale, tail_on_top: p.tail_on_top, tail_from_right: p.tail_from_right, scale })
}

/// Puts the chat next to the pet (above it, or below when there is no room), inside the pet's monitor.
pub fn follow() {
    let Some(h) = hwnd() else { return };
    let Some(g) = geometry(None) else { return };
    let c = &mut app().pet.chat;
    c.tail_on_top = g.tail_on_top;
    c.tail_from_right = g.tail_from_right;
    unsafe { let _ = SetWindowPos(h, Some(HWND_TOPMOST), g.x.round() as i32, g.y.round() as i32, g.w.round() as i32, g.h.round() as i32, SWP_NOACTIVATE); }
    set_region(h, g.w.round(), g.h.round(), g.scale);
    invalidate();
}

pub fn resize_to_settings() { follow(); }

/// The window's shape: a rounded body and the tail.
fn set_region(h: HWND, w: f64, hh: f64, scale: f64) {
    let c = &app().pet.chat;
    let (r, tail) = (rad::WINDOW * scale, TAIL * scale);
    let (top, bottom) = if c.tail_on_top { (tail, hh) } else { (0.0, hh - tail) };
    let tip_x = (w - c.tail_from_right * scale).clamp(TAIL_HALF * scale + 1.0, w - TAIL_HALF * scale - 1.0);
    let half = TAIL_HALF * scale;
    unsafe {
        let body = CreateRoundRectRgn(0, top as i32, w as i32 + 1, bottom as i32 + 1, (2.0 * r) as i32, (2.0 * r) as i32);
        let pts = if c.tail_on_top {
            [POINT { x: (tip_x - half) as i32, y: top as i32 + 1 }, POINT { x: (tip_x + half) as i32, y: top as i32 + 1 }, POINT { x: tip_x as i32, y: 0 }]
        } else {
            [POINT { x: (tip_x - half) as i32, y: bottom as i32 - 1 }, POINT { x: (tip_x + half) as i32, y: bottom as i32 - 1 }, POINT { x: tip_x as i32, y: hh as i32 }]
        };
        let tri = CreatePolygonRgn(&pts, ALTERNATE);
        CombineRgn(Some(body), Some(body), Some(tri), RGN_OR);
        let _ = DeleteObject(tri.into());
        SetWindowRgn(h, Some(body), true);
    }
}

// ---------- window messages (resizing by the edges) ----------

fn on_message(h: HWND, msg: u32, _wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let scale = crate::window::dpi_scale(h);
    match msg {
        WM_NCHITTEST => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
            let mut r = RECT::default();
            unsafe { let _ = GetWindowRect(h, &mut r); }
            let (px, py) = ((x - r.left) as f64, (y - r.top) as f64);
            let (w, hh) = ((r.right - r.left) as f64, (r.bottom - r.top) as f64);
            let e = EDGE * scale;
            let tail_on_top = app().pet.chat.tail_on_top;
            let edges = PetChatLayout::resizable_edges(tail_on_top);
            let (left, top, bottom) = (px < e, py < e, py > hh - e);
            let _ = w;
            let edge = match (left, top, bottom) {
                (true, true, _) => PetChatEdge::TopLeft,
                (true, _, true) => PetChatEdge::BottomLeft,
                (true, _, _) => PetChatEdge::Left,
                (_, true, _) => PetChatEdge::Top,
                (_, _, true) => PetChatEdge::Bottom,
                _ => return None,
            };
            if !edges.contains(&edge) { return None; }
            Some(LRESULT(match edge { PetChatEdge::Left => HTLEFT, PetChatEdge::Top => HTTOP, PetChatEdge::Bottom => HTBOTTOM, PetChatEdge::TopLeft => HTTOPLEFT, PetChatEdge::BottomLeft => HTBOTTOMLEFT } as isize))
        }
        WM_GETMINMAXINFO => {
            let mm = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
            let min = PetChatLayout::MINIMUM_SIZE;
            mm.ptMinTrackSize = POINT { x: (min.width * scale) as i32, y: ((min.height + TAIL) * scale) as i32 };
            if let (Some(pr), Some(ph)) = (pet::pet_rect(), app().pet.pet_hwnd()) {
                if let Some(work) = work_area_of(ph) {
                    let d = |r: Rect| PetRect::from_y_down(r.x / scale, r.y / scale, r.w / scale, r.h / scale);
                    let lim = PetChatLayout::drag_limit(d(pr), d(work), app().pet.chat.tail_on_top);
                    mm.ptMaxTrackSize = POINT { x: (lim.width * scale) as i32, y: ((lim.height + TAIL) * scale) as i32 };
                }
            }
            Some(LRESULT(0))
        }
        WM_SIZE => {
            let mut r = RECT::default();
            unsafe { let _ = GetWindowRect(h, &mut r); }
            set_region(h, (r.right - r.left) as f64, (r.bottom - r.top) as f64, scale);
            None
        }
        WM_EXITSIZEMOVE => {
            let mut r = RECT::default();
            unsafe { let _ = GetWindowRect(h, &mut r); }
            let body = PetChatSize::new(((r.right - r.left) as f64 / scale).round(), ((r.bottom - r.top) as f64 / scale - TAIL).round());
            pet::update_settings(|s| s.chat_size = PetChatLayout::saved(body));
            later(follow);
            Some(LRESULT(0))
        }
        WM_IME_STARTCOMPOSITION => {
            use windows::Win32::UI::Input::Ime::*;
            let (cx, cy) = CARET.with(|c| c.get());
            unsafe {
                let ctx = ImmGetContext(h);
                let form = COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: POINT { x: (cx * scale) as i32, y: (cy * scale) as i32 }, ..Default::default() };
                let _ = ImmSetCompositionWindow(ctx, &form);
                let _ = ImmReleaseContext(h, ctx);
            }
            None
        }
        _ => None,
    }
}

thread_local! { static CARET: std::cell::Cell<(f64, f64)> = const { std::cell::Cell::new((0.0, 0.0)) }; }

// ---------- drawing ----------

fn kind_color(pal: &Pal, k: PetChatLineKind) -> Color {
    match k { PetChatLineKind::Memory | PetChatLineKind::Notice => pal.secondary, PetChatLineKind::Danger => pal.danger, _ => pal.tertiary }
}

fn glyph_for(symbol: &str) -> u32 {
    match symbol {
        s if s.contains("brain") => 0xE9CE,
        s if s.contains("wrench") || s.contains("gear") => 0xE90F,
        s if s.contains("info") => 0xE946,
        s if s.contains("exclamation") || s.contains("warning") => 0xE7BA,
        s if s.contains("bell") => 0xEA8F,
        s if s.contains("clock") || s.contains("hourglass") => 0xE916,
        s if s.contains("doc") || s.contains("file") => 0xE8A5,
        s if s.contains("photo") || s.contains("image") => 0xEB9F,
        _ => 0xE946,
    }
}

fn spans_of(parts: &[PetTextPart], pal: &Pal, base: Color) -> Vec<Span> {
    parts.iter().map(|p| Span {
        text: p.text.clone(),
        color: Some(match p.style { PetTextStyle::Plain => base, PetTextStyle::Secondary => base.alpha(0.5), PetTextStyle::Ok => pal.success, PetTextStyle::Failed => pal.danger }),
        link: p.link.clone(),
        ..Default::default()
    }).collect()
}

fn markdown_spans(text: &str, pal: &Pal, color: Color) -> Vec<Span> {
    PetMarkdown::spans(text).into_iter().map(|s| Span { text: s.text, bold: s.bold, italic: s.italic, code: s.code, color: Some(if s.link.is_some() { pal.link } else { color }), link: s.link }).collect()
}

/// Rich text at (x, y); returns its height and reports clicks on its links.
fn rich(ui: &mut Ui, spans: &[Span], f: Font, x: f64, y: f64, w: f64, base: Color, clicked: &mut Vec<String>) -> (f64, f64) {
    let Some(l) = ui.c.rich_layout(spans, f, w, base) else { return (0.0, 0.0) };
    let links = ui.c.draw_rich(&l, x, y, base);
    for (i, (r, url)) in links.iter().enumerate() {
        let resp = ui.hit(ui.id(&format!("lk{i}")), *r, false);
        if resp.hover { ui.st.cursor_hand = true; }
        if resp.clicked { clicked.push(url.clone()); }
    }
    (l.w, l.h)
}

struct Cx<'a> { pal: &'a Pal, w: f64, clicked: Vec<String>, input_fill: Option<String> }

fn bubble_max(w: f64) -> f64 {
    let design_log = tok::CHAT_WIDTH - 2.0 * space::M;
    tok::BUBBLE_MAX_WIDTH.max((w * tok::BUBBLE_MAX_WIDTH / design_log).floor())
}

fn draw_item(ui: &mut Ui, cx: &mut Cx, item: &PetChatItem, x: f64, y: f64) -> f64 {
    let (pal, w) = (cx.pal, cx.w);
    match item {
        PetChatItem::Older { loading } => {
            let label = crate::i18n::t(if *loading { "Loading…" } else { "Show earlier messages" }).into_owned();
            let bw = ui.c.measure(&label, medium(12), 300.0).0 + 24.0;
            let r = Rect::new(x + (w - bw) / 2.0, y, bw, tok::CHIP_HEIGHT);
            let resp = ui.hit(ui.id("older"), r, false);
            ui.c.round(r, rad::CHIP, if resp.hover && !*loading { pal.surface.mix(pal.text, 0.06) } else { pal.surface });
            ui.c.line_text(&label, medium(12), r, pal.secondary, Align::Center);
            if resp.hover && !*loading { ui.st.cursor_hand = true; }
            if resp.clicked && !*loading { cx.clicked.push(PetLinks::url("older", &[])); }
            tok::CHIP_HEIGHT
        }
        PetChatItem::Hello { title, examples } => {
            let (_, th) = ui.c.measure(title, semi(15), w);
            ui.c.text(title, semi(15), Rect::new(x, y + 10.0, w, th + 2.0), pal.text, Align::Center);
            let mut yy = y + 10.0 + th + 12.0;
            for (i, ex) in examples.iter().enumerate() {
                let bw = (ui.c.measure(ex, medium(12), w).0 + 28.0).min(w);
                let r = Rect::new(x + (w - bw) / 2.0, yy, bw, tok::CHIP_HEIGHT + 4.0);
                let resp = ui.hit(ui.id(&format!("ex{i}")), r, false);
                ui.c.round(r, rad::CHIP + 2.0, if resp.hover { pal.accent_soft } else { pal.surface });
                ui.c.line_text(ex, medium(12), r, pal.text, Align::Center);
                if resp.hover { ui.st.cursor_hand = true; }
                if resp.clicked { cx.input_fill = Some(ex.clone()); }
                yy += tok::CHIP_HEIGHT + 10.0;
            }
            yy - y
        }
        PetChatItem::Mine { text, attachments, past } => bubble(ui, cx, &[Span::plain(text)], true, *past, false, attachments, x, y),
        PetChatItem::Nacho { text, past, streaming } => {
            let spans = markdown_spans(text, pal, pal.nacho_text);
            bubble(ui, cx, &spans, false, *past, *streaming, &[], x, y)
        }
        PetChatItem::Line(l) => line(ui, cx, l, x, y),
        PetChatItem::Files { files, mine } => files_row(ui, cx, files, *mine, x, y),
        PetChatItem::Card(c) => {
            let (h, link) = crate::pet_cards::draw_card(ui, pal, c, x, y, w);
            if let Some(l) = link { cx.clicked.push(l); }
            h
        }
        PetChatItem::Note(n) => {
            let (_, h) = ui.c.measure(n, sans(11), w);
            ui.c.text(n, sans(11), Rect::new(x, y, w, h + 1.0), pal.tertiary, Align::Center);
            h
        }
        PetChatItem::Divider(d) => {
            let tw = ui.c.measure(d, sans(11), w - 60.0).0 + 16.0;
            ui.c.line(x, y + 8.0, x + (w - tw) / 2.0, y + 8.0, pal.separator, 1.0);
            ui.c.line(x + (w + tw) / 2.0, y + 8.0, x + w, y + 8.0, pal.separator, 1.0);
            ui.c.line_text(d, sans(11), Rect::new(x + (w - tw) / 2.0, y, tw, 16.0), pal.tertiary, Align::Center);
            16.0
        }
        // The selection bar's pieces (`pet_selection_ui`).
        PetChatItem::RequestChip(text) => crate::pet_selection_ui::chip(ui, pal, text, x, y, w),
        PetChatItem::AnswerActions { turn_id, showing_sent } => crate::pet_selection_ui::answer_actions(ui, pal, turn_id, *showing_sent, x, y, &mut cx.clicked),
        PetChatItem::SentContext(rows) => crate::pet_selection_ui::sent_context(ui, pal, rows, x, y, w),
        PetChatItem::SelectionPreview(p) => crate::pet_selection_ui::preview(ui, pal, p, x, y, w, &mut cx.clicked),
        PetChatItem::SelectionIntro => crate::pet_selection_ui::intro(ui, pal, x, y, w, &mut cx.clicked),
    }
}

fn bubble(ui: &mut Ui, cx: &mut Cx, spans: &[Span], mine: bool, past: bool, streaming: bool, attachments: &[String], x: f64, y: f64) -> f64 {
    let pal = cx.pal;
    let (fill, ink) = if mine { (pal.mine_bubble, pal.mine_text) } else { (pal.nacho_bubble, pal.nacho_text) };
    let fade = if past { 0.78 } else { 1.0 };
    let max = bubble_max(cx.w);
    let inner = max - 2.0 * space::M;
    let mut spans = spans.to_vec();
    if streaming { spans.push(Span { text: if spans.iter().all(|s| s.text.is_empty()) { "…".into() } else { " ▍".into() }, color: Some(pal.accent), ..Default::default() }); }
    let Some(l) = ui.c.rich_layout(&spans, sans(13), inner, ink.alpha(fade)) else { return 0.0 };
    let mut chips_h = 0.0;
    if !attachments.is_empty() { chips_h = tok::CHIP_HEIGHT + space::XS; }
    let (bw, bh) = ((l.w + 2.0 * space::M).max(24.0).min(max), l.h + 2.0 * space::S + chips_h);
    let bx = if mine { x + cx.w - bw } else { x };
    let r = Rect::new(bx, y, bw, bh);
    ui.c.round(r, rad::BUBBLE, fill.alpha(fade));
    // The corner at the speaker's side is squarer, like the Mac bubble.
    let sq = Rect::new(if mine { r.right() - rad::BUBBLE } else { r.x }, r.bottom() - rad::BUBBLE, rad::BUBBLE, rad::BUBBLE);
    ui.c.round(sq, rad::BUBBLE_TAIL, fill.alpha(fade));
    ui.c.fill(Rect::new(sq.x + if mine { 0.0 } else { rad::BUBBLE_TAIL }, sq.y, sq.w - rad::BUBBLE_TAIL, sq.h - rad::BUBBLE_TAIL), fill.alpha(fade));
    let (_, _) = rich(ui, &spans, sans(13), bx + space::M, y + space::S, inner, ink.alpha(fade), &mut cx.clicked);
    if !attachments.is_empty() {
        let mut ax = bx + space::M;
        for name in attachments {
            let cw = (ui.c.measure(name, sans(11), 200.0).0 + 22.0).min(tok::CHIP_MAX_WIDTH);
            let cr = Rect::new(ax, y + space::S + l.h + space::XS, cw, tok::CHIP_HEIGHT);
            ui.c.round(cr, rad::CHIP, pal.raised.alpha(0.7));
            ui.c.line_text(name, sans(11), cr.inset(8.0, 0.0), ink, Align::Left);
            ax += cw + space::XS;
        }
    }
    bh
}

fn line(ui: &mut Ui, cx: &mut Cx, l: &PetChatLine, x: f64, y: f64) -> f64 {
    let pal = cx.pal;
    let color = kind_color(pal, l.kind);
    let mut tx = x;
    let mut tw = cx.w;
    if let Some(s) = &l.symbol {
        ui.c.glyph(glyph_for(s), 11, Rect::new(x, y, tok::LINE_ICON, 16.0), color);
        tx += tok::LINE_ICON + space::XS;
        tw -= tok::LINE_ICON + space::XS;
    }
    let spans = spans_of(&l.parts, pal, color);
    let Some(lay) = ui.c.rich_layout(&spans, sans(11), tw, color) else { return 0.0 };
    if l.align_trailing || l.kind == PetChatLineKind::Usage { tx = x + cx.w - lay.w; }
    let links = ui.c.draw_rich(&lay, tx, y, color);
    for (i, (r, url)) in links.iter().enumerate() {
        let resp = ui.hit(ui.id(&format!("ll{i}")), *r, false);
        if resp.hover { ui.st.cursor_hand = true; }
        if resp.clicked { cx.clicked.push(url.clone()); }
    }
    lay.h.max(16.0)
}

fn files_row(ui: &mut Ui, cx: &mut Cx, files: &[PetFileChip], mine: bool, x: f64, y: f64) -> f64 {
    let pal = cx.pal;
    let mut chips: Vec<(f64, &PetFileChip, String)> = vec![];
    for f in files {
        let mut label = f.name.clone();
        if f.by_nacho { label += &format!(" · {}", crate::i18n::t("made by Nacho")); }
        if f.deleted { label += &format!(" · {}", crate::i18n::t("deleted")); }
        let w = (ui.c.measure(&label, medium(11), 400.0).0 + 34.0).min(tok::CHIP_MAX_WIDTH + 60.0);
        chips.push((w, f, label));
    }
    let (mut cx_x, mut yy) = (0.0, y);
    let mut rows: Vec<Vec<usize>> = vec![vec![]];
    for (i, c) in chips.iter().enumerate() {
        if cx_x + c.0 > cx.w && !rows.last().unwrap().is_empty() { rows.push(vec![]); cx_x = 0.0; }
        rows.last_mut().unwrap().push(i);
        cx_x += c.0 + space::XS;
    }
    for row in rows {
        let total: f64 = row.iter().map(|i| chips[*i].0 + space::XS).sum::<f64>() - space::XS;
        let mut rx = if mine { x + cx.w - total } else { x };
        for i in row {
            let (w, f, label) = (&chips[i].0, chips[i].1, &chips[i].2);
            let r = Rect::new(rx, yy, *w, tok::CHIP_HEIGHT);
            let resp = ui.hit(ui.id(&format!("file{i}")), r, false);
            let fade = if f.deleted { 0.7 } else { 1.0 };
            ui.c.round(r, rad::CHIP, pal.raised.alpha(if mine { 0.7 } else { 1.0 }));
            ui.c.stroke_round(r, rad::CHIP, pal.separator, 1.0);
            ui.c.glyph(if f.is_image { 0xEB9F } else { 0xE8A5 }, 11, Rect::new(r.x + 6.0, r.y, 16.0, r.h), pal.secondary.alpha(fade));
            ui.c.line_text(label, medium(11), Rect::new(r.x + 24.0, r.y, r.w - 30.0, r.h), if f.url.is_some() { pal.link } else { pal.secondary }.alpha(fade), Align::Left);
            if let (true, Some(u)) = (resp.hover, &f.url) { ui.st.cursor_hand = true; let _ = u; }
            if resp.clicked { if let Some(u) = &f.url { cx.clicked.push(u.clone()); } }
            rx += w + space::XS;
        }
        yy += tok::CHIP_HEIGHT + space::XS;
    }
    yy - y - space::XS
}

pub fn clipboard_text() -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        OpenClipboard(None).ok()?;
        let r = (|| {
            let h = GetClipboardData(13).ok()?; // CF_UNICODETEXT
            let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
            if p.is_null() { return None; }
            let mut n = 0;
            while *p.add(n) != 0 && n < 100_000 { n += 1; }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(HGLOBAL(h.0));
            Some(s)
        })();
        let _ = CloseClipboard();
        r
    }
}

/// The notice line under the log: a flash, dictation problems, uploads, the session's last error.
fn notice_line() -> Option<PetChatItem> {
    let p = &app().pet;
    let st = &p.state;
    if let Some((parts, at)) = &p.chat.flash {
        if at.elapsed() < std::time::Duration::from_secs(7) { return Some(PetChatItem::Line(PetChatLine::new(PetChatLineKind::Notice, "info.circle", parts.clone()))); }
    }
    let text = if st.uploading { Some("파일 올리는 중…".to_string()) }
        else if st.dictating { Some("받아쓰는 중…".to_string()) }
        else { st.chat.notice.clone().or(st.notice.clone()).filter(|n| Some(n) != st.config.as_ref().and_then(|c| c.service_notice()).as_ref()) };
    text.map(|t| PetChatItem::Line(PetChatLine::new(PetChatLineKind::Notice, "info.circle", vec![PetTextPart::new(&t)])))
}

fn signature(items: &[PetChatItem]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    items.len().hash(&mut h);
    match items.last() { Some(PetChatItem::Nacho { text, .. }) => text.len().hash(&mut h), Some(PetChatItem::Mine { text, .. }) => text.len().hash(&mut h), Some(PetChatItem::Card(c)) => { c.id.hash(&mut h); c.status.hash(&mut h); c.options.len().hash(&mut h); } _ => {} }
    h.finish()
}

fn draw(ui: &mut Ui, ctx: &mut WindowCtx) {
    use crate::i18n::t;
    let pal = pet_theme::current();
    let (w, h) = (ui.w, ui.h);
    let a = app();
    let tail_on_top = a.pet.chat.tail_on_top;
    let body = Rect::new(0.0, if tail_on_top { TAIL } else { 0.0 }, w, h - TAIL);
    let session = pet::session();
    ui.c.fill(Rect::new(0.0, 0.0, w, h), pal.window);
    ui.c.stroke_round(body, rad::WINDOW, pal.edge, 1.0);
    let tip = (w - a.pet.chat.tail_from_right).clamp(TAIL_HALF + 1.0, w - TAIL_HALF - 1.0);
    let (b, tp) = if tail_on_top { (body.y, 0.0) } else { (body.bottom(), h) };
    ui.c.line(tip - TAIL_HALF, b, tip, tp, pal.edge, 1.0);
    ui.c.line(tip + TAIL_HALF, b, tip, tp, pal.edge, 1.0);

    let st = a.pet.state.clone();
    let signed_in = st.auth == PetAuthPhase::SignedIn;
    let busy = st.chat.is_busy();

    // A queued text the automatic send could not deliver comes back into the field.
    if let Some(back) = st.returned_text.clone() {
        if a.pet.chat.taken_back.as_deref() != Some(back.as_str()) {
            let c = &mut a.pet.chat;
            c.taken_back = Some(back.clone());
            let back = back.replace(['\r', '\n'], " ");
            c.input = if c.input.trim().is_empty() { back } else { format!("{back} {}", c.input) };
            c.cursor = c.input.chars().count();
            if let Some(s) = &session { s.clear_returned(); }
            pet::flash_text("대기하던 글을 보내지 못했어요. 입력창에 돌려놨어요.");
        }
    } else { a.pet.chat.taken_back = None; }

    // Keys that are not text.
    let keys = ui.st.keys.clone();
    let focus_input = ui.st.focus.is_none() || ui.st.focus == Some(ui.id("input"));
    let items = if signed_in {
        let undo = |u: &str| session.as_ref().and_then(|s| s.can_undo_forget(u).then_some(10));
        let mut v = PetTranscript::items(&st, &undo, &crate::pet_selection::chat_state(), a.pet.flow.locals(), crate::core::pet::clock::now());
        if let Some(n) = notice_line() { v.push(n); }
        // NachoCombo MCP calls waiting for the user (local cards, never sent to the server).
        v.extend(crate::mcp::approval_items());
        v
    } else { vec![] };
    let active_card = PetTranscript::active_card_id(&items).and_then(|id| pet::card_by_id(&id));
    for k in &keys {
        if k.vk == 0x1B {
            if pet::escape_recording() { continue; }
            if a.pet.chat.input.is_empty() { if let Some(c) = &active_card { let step = a.pet.flow.key(PetCardKey::Escape, c); if step != crate::core::pet::card_flow::CardStep::Pass { pet::card_step(step); continue; } } }
            ctx.close = true;
        }
    }

    // ---- header ----
    let head = Rect::new(0.0, body.y, w, HEADER);
    let av = Rect::new(space::M, head.y + (HEADER - tok::AVATAR) / 2.0, tok::AVATAR, tok::AVATAR);
    ui.c.circle(av.cx(), av.cy(), av.w / 2.0, pal.accent_soft);
    ui.c.image(&crate::pet_art::key_of("idle"), || crate::pet_art::image("idle").map(|p| (*p).clone()), av.inset(1.0, 1.0), 1.0);
    let hs = PetTranscript::header_status(&st, &crate::pet_selection::chat_state());
    let name = if st.chat.agent_name.is_empty() { "나초".to_string() } else { st.chat.agent_name.clone() };
    let right = 2.0 * 28.0 + 4.0 + space::M;
    ui.c.line_text(&name, semi(15), Rect::new(av.right() + space::S, head.y + 4.0, w - right - 50.0, 20.0), pal.text, Align::Left);
    let dot = match hs.dot { HeaderDot::Ok => pal.success, HeaderDot::Busy => pal.accent, HeaderDot::Wait | HeaderDot::Warn => pal.warning, HeaderDot::Off => pal.tertiary };
    ui.c.circle(av.right() + space::S + tok::STATUS_DOT / 2.0, head.y + 31.0, tok::STATUS_DOT / 2.0, dot);
    ui.c.line_text(&PetTranscript::header_line(&st, &crate::pet_selection::chat_state()), sans(11), Rect::new(av.right() + space::S + tok::STATUS_DOT + space::XS, head.y + 23.0, w - right - 66.0, 16.0), pal.secondary, Align::Left);
    let close_r = Rect::new(w - space::M - 28.0, head.y + (HEADER - 28.0) / 2.0, 28.0, 28.0);
    let more_r = Rect::new(close_r.x - 4.0 - 28.0, close_r.y, 28.0, 28.0);
    for (key, r, glyph) in [("close", close_r, 0xE711u32), ("more", more_r, 0xE712u32)] {
        let resp = ui.hit(ui.id(key), r, false);
        if resp.hover { ui.c.round(r, 14.0, pal.surface); ui.st.cursor_hand = true; }
        ui.c.glyph(glyph, 13, r, pal.secondary);
        if resp.clicked {
            if key == "close" { ctx.close = true; } else { later(menu_from_chat); }
        }
    }
    ui.c.line(0.0, head.bottom(), w, head.bottom(), pal.separator, 1.0);

    let mut y_top = head.bottom();
    if signed_in {
        if let Some(n) = st.config.as_ref().and_then(|c| c.service_notice()) {
            let (_, nh) = ui.c.measure(&n, sans(11), w - 2.0 * space::M);
            let r = Rect::new(0.0, y_top, w, nh + 12.0);
            ui.c.fill(r, pal.warning_soft);
            ui.c.text(&n, sans(11), Rect::new(space::M, y_top + 6.0, w - 2.0 * space::M, nh + 2.0), pal.text, Align::Left);
            y_top += r.h;
        }
    }

    if !signed_in {
        // An MCP client's call can wait for the user without a Nacho account: its cards come first.
        let cards = crate::mcp::approval_items();
        if !cards.is_empty() {
            let mut cx = Cx { pal: &pal, w: w - 2.0 * space::M, clicked: vec![], input_fill: None };
            ui.scroll("mcpcards", Rect::new(0.0, y_top, w, body.bottom() - y_top), |ui, y0| {
                let mut y = y0 + space::S;
                for (i, item) in cards.iter().enumerate() {
                    let hgt = ui.in_scope(i as u64, |ui| draw_item(ui, &mut cx, item, space::M, y));
                    if hgt > 0.0 { y += hgt + space::S; }
                }
                y + space::XS
            });
            for url in std::mem::take(&mut cx.clicked) { let u = url.clone(); later(move || pet::on_link(&u)); }
            return;
        }
        sign_in_view(ui, &pal, &st, Rect::new(0.0, y_top, w, body.bottom() - y_top));
        return;
    }

    // ---- composer (bottom) ----
    let comp_h = 12.0 + tok::COMPOSER_HEIGHT.max(tok::CONTROL) + 8.0;
    let mut bottom = body.bottom() - comp_h;
    // waiting files
    if !st.pending.is_empty() {
        let mut px = space::M;
        let py = bottom - 4.0 - tok::CHIP_HEIGHT;
        for f in &st.pending {
            let label = f.file.name.clone();
            let cw = (ui.c.measure(&label, sans(11), 300.0).0 + 54.0).min(tok::CHIP_MAX_WIDTH + 30.0);
            let r = Rect::new(px, py, cw, tok::CHIP_HEIGHT);
            ui.c.round(r, rad::CHIP, pal.surface);
            ui.c.glyph(if f.file.kind == "image" { 0xEB9F } else { 0xE8A5 }, 11, Rect::new(r.x + 6.0, r.y, 16.0, r.h), pal.secondary);
            ui.c.line_text(&label, sans(11), Rect::new(r.x + 24.0, r.y, r.w - 46.0, r.h), pal.text, Align::Left);
            let xr = Rect::new(r.right() - 20.0, r.y + 2.0, 18.0, r.h - 4.0);
            let resp = ui.hit(ui.id(&format!("rm{}", f.id)), xr, false);
            ui.c.glyph(0xE711, 9, xr, if resp.hover { pal.danger } else { pal.tertiary });
            if resp.hover { ui.st.cursor_hand = true; }
            if resp.clicked { if let Some(s) = &session { s.remove_pending(f.id); } }
            px += cw + space::XS;
            if px > w - 60.0 { break; }
        }
        bottom = py - 4.0;
    }

    // ---- the log ----
    let view = Rect::new(0.0, y_top, w, bottom - y_top);
    let log_id = ui.id("log");
    let sig = signature(&items);
    let content = ui.st.content.get(&log_id).copied().unwrap_or(0.0);
    {
        let c = &mut a.pet.chat;
        if sig != c.sig {
            if c.pinned || c.sig == 0 { c.stick = true; c.stick_frames = 4; }
            c.sig = sig;
        }
        if (view.h - c.last_view_h).abs() > 0.5 {
            // Files waiting or a notice took room from the log: the end stays in view for a user who was at the end.
            if c.pinned { c.stick = true; }
            c.last_view_h = view.h;
        }
        if c.stick {
            // Stay at the end until the content stops growing (a card is measured one frame later).
            ui.st.scroll.insert(log_id, 1.0e9);
            // A new item is measured a frame after it is first drawn: keep following for a few frames before judging.
            c.stick_frames = c.stick_frames.saturating_sub(1);
            if c.stick_frames == 0 && (content - c.last_content).abs() < 0.5 { c.stick = false; }
            ui.st.again = true;
        }
        c.last_content = content;
    }
    let mut cx = Cx { pal: &pal, w: w - 2.0 * space::M, clicked: vec![], input_fill: None };
    let wheel = ui.st.wheel != 0.0 && view.contains(ui.st.mouse.0, ui.st.mouse.1);
    ui.scroll("log", view, |ui, y0| {
        let mut y = y0 + space::S;
        for (i, item) in items.iter().enumerate() {
            let hgt = ui.in_scope(i as u64, |ui| draw_item(ui, &mut cx, item, space::M, y));
            if hgt > 0.0 { y += hgt + space::S; }
        }
        y + space::XS
    });
    if wheel {
        // The user moved the log: it follows new content only when they left it at the end.
        let (sc, ct) = (ui.st.scroll.get(&log_id).copied().unwrap_or(0.0), ui.st.content.get(&log_id).copied().unwrap_or(0.0));
        a.pet.chat.pinned = sc >= ct - view.h - 4.0;
        a.pet.chat.stick = false;
    }
    let _ = focus_input;

    // ---- composer ----
    let comp_y = body.bottom() - comp_h + 12.0;
    let row_h = tok::COMPOSER_HEIGHT;
    ui.c.line(0.0, body.bottom() - comp_h, w, body.bottom() - comp_h, pal.separator, 1.0);
    let attach_r = Rect::new(space::M, comp_y + (row_h - tok::CONTROL) / 2.0, tok::CONTROL, tok::CONTROL);
    let send_r = Rect::new(w - space::M - tok::SEND_BUTTON, comp_y + (row_h - tok::SEND_BUTTON) / 2.0, tok::SEND_BUTTON, tok::SEND_BUTTON);
    let mic_r = Rect::new(send_r.x - space::XS - tok::CONTROL, attach_r.y, tok::CONTROL, tok::CONTROL);
    let field_r = Rect::new(attach_r.right() + space::XS, comp_y, mic_r.x - space::XS - (attach_r.right() + space::XS), row_h);
    let listening = pet::is_recording();
    // attach
    let resp = ui.hit(ui.id("attach"), attach_r, false);
    if resp.hover { ui.c.round(attach_r, 14.0, pal.surface); ui.st.cursor_hand = true; }
    ui.c.glyph(0xE723, 14, attach_r, pal.secondary);
    if resp.clicked { later(pet::pick_files); }
    // mic
    let resp = ui.hit(ui.id("mic"), mic_r, false);
    if listening { ui.c.round(mic_r, 14.0, pal.danger); } else if resp.hover { ui.c.round(mic_r, 14.0, pal.surface); }
    ui.c.glyph(0xE720, 14, mic_r, if listening { pal.on_danger } else { pal.secondary });
    if resp.hover { ui.st.cursor_hand = true; }
    if resp.clicked { let existing = a.pet.chat.input.clone(); pet::toggle_dictation(&existing); }
    // the field
    let fid = ui.id("input");
    let resp = ui.hit(fid, field_r, true);
    if ui.st.focus.is_none() { ui.st.focus = Some(fid); }
    let focused = ui.st.focus == Some(fid);
    ui.c.round(field_r, field_r.h / 2.0, pal.surface);
    if focused { ui.c.stroke_round(field_r, field_r.h / 2.0, pal.accent, 1.5); }
    if resp.hover { ui.st.cursor_text = true; }
    let mut send_now: Option<bool> = None;
    if let Some(t) = cx.input_fill.take() { a.pet.chat.input = t; a.pet.chat.cursor = a.pet.chat.input.chars().count(); ui.st.focus = Some(fid); }
    if focused { edit_input(ui, &keys, &mut send_now); }
    // number keys that answer a card while the field is empty
    if focused {
        let c = &mut a.pet.chat;
        let before = c.input.clone();
        let edit = a.pet.digits.changed(&before, active_card.as_ref(), &mut a.pet.flow);
        match edit {
            DigitEdit::Keep => {}
            DigitEdit::Clear => { a.pet.chat.input.clear(); a.pet.chat.cursor = 0; ui.st.again = true; }
            DigitEdit::Replace(t) => { a.pet.chat.cursor = t.chars().count(); a.pet.chat.input = t; ui.st.again = true; }
        }
        if send_now == Some(true) && a.pet.chat.input.is_empty() {
            if let Some(c) = &active_card { let step = a.pet.flow.key(PetCardKey::Enter, c); if step != crate::core::pet::card_flow::CardStep::Pass { pet::card_step(step); send_now = None; } }
        }
    }
    let input = a.pet.chat.input.clone();
    let hint = if listening { t("Listening…") } else if a.pet.chat.memory_feedback.is_some() { t("What did Nacho remember wrong?") } else { t("Talk to Nacho…") };
    let inner = Rect::new(field_r.x + space::M, field_r.y, field_r.w - 2.0 * space::M, field_r.h);
    ui.c.clip(inner);
    if input.is_empty() { ui.c.line_text(&hint, sans(13), inner, if listening { pal.danger } else { pal.tertiary }, Align::Left); }
    else {
        let cur = a.pet.chat.cursor.min(input.chars().count());
        let prefix: String = input.chars().take(cur).collect();
        let pw = if prefix.is_empty() { 0.0 } else { ui.c.measure(&prefix, sans(13), 100000.0).0 };
        let total = ui.c.measure(&input, sans(13), 100000.0).0;
        let shift = if pw > inner.w - 4.0 { pw - inner.w + 4.0 } else { 0.0 };
        let _ = total;
        ui.c.line_text(&input, sans(13), Rect::new(inner.x - shift, inner.y, 100000.0, inner.h), pal.text, Align::Left);
        if focused { let cx_ = inner.x - shift + pw; ui.c.line(cx_, inner.cy() - 8.0, cx_, inner.cy() + 8.0, pal.text, 1.0); CARET.with(|c| c.set((cx_, inner.y))); }
    }
    if input.is_empty() && focused { ui.c.line(inner.x, inner.cy() - 8.0, inner.x, inner.cy() + 8.0, pal.text, 1.0); CARET.with(|c| c.set((inner.x, inner.y))); }
    ui.c.unclip();
    // send / stop
    let has_text = !input.trim().is_empty() || !st.pending.is_empty();
    let resp = ui.hit(ui.id("send"), send_r, false);
    let button = send_button(busy, has_text);
    let (fill, ink, glyph) = match button {
        SendButton::Stop => (pal.text, pal.window, 0xE71Au32),
        SendButton::Send { armed: true, .. } => (pal.accent, pal.on_accent, 0xE74Au32),
        SendButton::Send { armed: false, .. } => (pal.surface, pal.tertiary, 0xE74Au32),
    };
    ui.c.circle(send_r.cx(), send_r.cy(), send_r.w / 2.0, if resp.hover && !st.uploading { fill.mix(pal.text, 0.1) } else { fill });
    ui.c.glyph(glyph, 12, send_r, ink);
    if resp.hover { ui.st.cursor_hand = true; }
    if resp.clicked && !st.uploading { send_now = Some(false); }

    if let Some(from_return) = send_now {
        let text = a.pet.chat.input.clone();
        match plan_send(busy, &text, !st.pending.is_empty(), from_return) {
            SendPlan::Nothing => {}
            SendPlan::Stop => { if let Some(s) = &session { s.stop(); } }
            plan @ (SendPlan::Send | SendPlan::Queue) => {
                if let Some(turn) = a.pet.chat.memory_feedback.take() {
                    if let Some(s) = &session {
                        s.send_memory_feedback(&turn, &text, Some(Box::new(|r| pet::push(pet::Event::Flash(r.unwrap_or_else(|| "알려 줘서 고마워요. 기억을 고칠게요.".into()))))));
                        a.pet.chat.input.clear();
                        a.pet.chat.cursor = 0;
                    }
                } else if !a.pet.chat.sending {
                    a.pet.chat.sending = true;
                    let done = |r: Option<String>| pet::push(pet::Event::SendDone(r));
                    if plan == SendPlan::Queue { pet::queue_send(&text, done); } else { pet::send(&text, done); }
                }
            }
        }
        ui.st.again = true;
    }
    // Links of the transcript.
    for url in std::mem::take(&mut cx.clicked) { let u = url.clone(); later(move || pet::on_link(&u)); }
}

/// Typing into the input field: text, the arrow keys, Delete, paste, Enter.
fn edit_input(ui: &mut Ui, keys: &[Key], send_now: &mut Option<bool>) {
    let c = &mut app().pet.chat;
    let mut chars: Vec<char> = c.input.chars().collect();
    let mut cur = c.cursor.min(chars.len());
    for ch in ui.st.chars.clone() {
        match ch {
            '\u{8}' => { if cur > 0 { chars.remove(cur - 1); cur -= 1; } }
            '\r' | '\n' => {}
            ch if !ch.is_control() && chars.len() < 4000 => { chars.insert(cur, ch); cur += 1; }
            _ => {}
        }
    }
    for k in keys {
        match k.vk {
            0x25 => cur = cur.saturating_sub(1),
            0x27 => cur = (cur + 1).min(chars.len()),
            0x24 => cur = 0,
            0x23 => cur = chars.len(),
            0x2E => { if cur < chars.len() { chars.remove(cur); } }
            0x0D => *send_now = Some(true),
            0x56 if k.ctrl => {
                if let Some(t) = clipboard_text() {
                    for ch in t.chars().map(|c| if c == '\r' || c == '\n' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control()) { if chars.len() < 4000 { chars.insert(cur, ch); cur += 1; } }
                }
            }
            _ => {}
        }
    }
    let s: String = chars.into_iter().collect();
    if s != c.input { ui.st.again = true; }
    c.input = s;
    c.cursor = cur;
}

fn sign_in_view(ui: &mut Ui, pal: &Pal, st: &crate::core::pet::session::PetSessionState, r: Rect) {
    use crate::i18n::t;
    let inner = r.w - 2.0 * space::XL;
    let hi = t("Hi, I'm Nacho.");
    let p = t("Sign in with your Nacho account to chat here. You can hand me captures and files, or ask me to arrange windows and switch gadgets on and off.");
    let small = t("Signing in happens in your browser. Nacho Combo never sees your password.");
    let (_, h1) = ui.c.measure(&hi, semi(15), inner);
    let (_, h2) = ui.c.measure(&p, sans(13), inner);
    let (_, h3) = ui.c.measure(&small, sans(11), inner);
    let signing = st.auth == PetAuthPhase::SigningIn;
    let err = if let PetAuthPhase::Failed(m) = &st.auth { Some(m.clone()) } else { None };
    let eh = err.as_ref().map_or(0.0, |e| ui.c.measure(e, sans(12), inner).1 + space::M);
    let total = tok::SIGNIN_ART + space::M + h1 + space::M + h2 + space::M + eh + tok::CONTROL + 4.0 + space::M + h3;
    let mut y = r.y + ((r.h - total) / 2.0).max(space::M);
    ui.c.image(&crate::pet_art::key_of("happy"), || crate::pet_art::image("happy").map(|p| (*p).clone()), Rect::new(r.cx() - tok::SIGNIN_ART / 2.0, y, tok::SIGNIN_ART, tok::SIGNIN_ART), 1.0);
    y += tok::SIGNIN_ART + space::M;
    ui.c.text(&hi, semi(15), Rect::new(r.x + space::XL, y, inner, h1 + 2.0), pal.text, Align::Center);
    y += h1 + space::M;
    ui.c.text(&p, sans(13), Rect::new(r.x + space::XL, y, inner, h2 + 2.0), pal.secondary, Align::Center);
    y += h2 + space::M;
    if let Some(e) = &err {
        let (_, eh) = ui.c.measure(e, sans(12), inner);
        ui.c.text(e, sans(12), Rect::new(r.x + space::XL, y, inner, eh + 2.0), pal.danger, Align::Center);
        y += eh + space::M;
    }
    let label = t(if signing { "Waiting for your browser…" } else { "Sign in with Nacho account" });
    let bw = (ui.c.measure(&label, semi(13), 400.0).0 + 2.0 * space::L + 18.0).min(inner);
    let br = Rect::new(r.cx() - bw / 2.0, y, bw, tok::CONTROL + 4.0);
    let resp = ui.hit(ui.id("signin"), br, false);
    let fill = if signing { pal.accent.alpha(0.5) } else if resp.hover { pal.accent.mix(pal.text, 0.1) } else { pal.accent };
    ui.c.round(br, rad::CONTROL, fill);
    ui.c.line_text(&label, semi(13), br, pal.on_accent, Align::Center);
    if resp.hover && !signing { ui.st.cursor_hand = true; }
    if resp.clicked && !signing { later(pet::sign_in); }
    y += br.h + space::M;
    ui.c.text(&small, sans(11), Rect::new(r.x + space::XL, y, inner, h3 + 2.0), pal.tertiary, Align::Center);
}

fn menu_from_chat() { pet::show_menu_at_cursor(); }

#[allow(dead_code)]
fn dpi_of(h: HWND) -> u32 { unsafe { GetDpiForWindow(h) } }
