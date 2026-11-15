//! The selection bar's pieces in the chat (Mac: `PetSelectionChatViews.swift`): the preview card before anything is sent,
//! the chip under the sent request, the actions under the answer, what was sent, and the first-use card. Like the other
//! cards they only paint; a click is reported as a `nachopet://sel…` link and `pet_selection::on_link` acts on it.
use crate::core::layout::Rect;
use crate::core::pet::design_tokens::pet_tokens::{radius as rad, size as tok, space};
use crate::core::pet::selection::*;
use crate::core::pet::transcript::PetLinks;
use crate::gfx::*;
use crate::i18n::t;
use crate::pet_theme::Pal;
use crate::ui::*;

const PAD: f64 = space::M;
const ROW_ICON: f64 = tok::CONTEXT_ROW_ICON;

/// Segoe Fluent icon for the row's Mac symbol name.
fn glyph(symbol: &str) -> u32 {
    match symbol {
        "quote.opening" => 0xE8BD,
        "macwindow" => 0xE737,
        "text.alignleft" => 0xE8E4,
        "globe" => 0xE774,
        "text.insert" => 0xE8E9,
        "character.book.closed" => 0xE8C1,
        "doc.on.doc" => 0xE8C8,
        "magnifyingglass" => 0xE721,
        "character.bubble" => 0xE8C1,
        "lightbulb" => 0xEA80,
        "doc.text" => 0xE8A5,
        "arrow.clockwise" => 0xE72C,
        "sparkles" => 0xE82F,
        _ => 0xE946,
    }
}

fn link(action: &str, q: &[(&str, &str)]) -> String { PetLinks::url(action, q) }

/// A small pill button; returns whether it was clicked. `fill`: None draws only the outline.
fn pill(ui: &mut Ui, key: &str, label: &str, r: Rect, fill: Option<Color>, ink: Color, edge: Option<Color>, enabled: bool) -> bool {
    let resp = if enabled { ui.hit(ui.id(key), r, true) } else { Resp::default() };
    if let Some(f) = fill { ui.c.round(r, rad::CONTROL, if resp.hover { f.mix(ink, 0.08) } else { f }); }
    if let Some(e) = edge { ui.c.stroke_round(r, rad::CONTROL, e, 1.0); }
    ui.c.line_text(label, semi(12), r, ink, Align::Center);
    if resp.hover && enabled { ui.st.cursor_hand = true; }
    resp.clicked
}

fn pill_width(ui: &Ui, label: &str) -> f64 { ui.c.measure(label, semi(12), 600.0).0.ceil() + 2.0 * PAD }

fn card_frame(ui: &Ui, pal: &Pal, r: Rect) {
    ui.c.round(r, rad::CARD, pal.raised);
    ui.c.stroke_round(r, rad::CARD, pal.separator, 1.0);
}

/// The one-line head of a card: accent icon and the title.
fn head(ui: &Ui, pal: &Pal, symbol: &str, title: &str, x: f64, y: f64, w: f64) -> f64 {
    let (_, h) = ui.c.measure(title, medium(13), w - 24.0);
    ui.c.glyph(glyph(symbol), 14, Rect::new(x, y, 18.0, 18.0), pal.accent);
    ui.c.text(title, medium(13), Rect::new(x + 24.0, y, w - 24.0, h + 1.0), pal.text, Align::Left);
    h.max(18.0)
}

fn row_height(ui: &Ui, r: &PetSelectionRow, inner: f64, editable: bool) -> f64 {
    if r.field == PetSelectionField::Selection {
        let tw = inner - ROW_ICON - space::S - if editable { 28.0 } else { 0.0 };
        let (_, qh) = ui.c.measure(&r.value, sans(11), tw);
        let line = ui.c.measure("가", sans(11), 100.0).1;
        let mut h = 16.0 + qh.min(2.0 * line);
        if r.warning.is_some() { h += 16.0; }
        h
    } else { 20.0 }
}

/// One `.row2`. Returns the link of what was clicked.
fn context_row(ui: &mut Ui, pal: &Pal, r: &PetSelectionRow, x: f64, y: f64, inner: f64, editable: bool) -> (f64, Option<String>) {
    let h = row_height(ui, r, inner, editable);
    let off = r.removed;
    let mut clicked = None;
    ui.c.glyph(glyph(r.symbol), ROW_ICON as u32, Rect::new(x, y + 1.0, ROW_ICON, 16.0), pal.tertiary);
    let tx = x + ROW_ICON + space::S;
    let right = if editable { if off { ui.c.measure(&t("Put back"), medium(11), 200.0).0 + 8.0 } else { 20.0 } } else { 0.0 };
    let tw = inner - ROW_ICON - space::S - if editable { right + space::S } else { 0.0 };
    let label_ink = if off { pal.tertiary } else { pal.secondary };
    if r.field == PetSelectionField::Selection {
        ui.c.line_text(&r.label, medium(11), Rect::new(tx, y, tw, 16.0), label_ink, Align::Left);
        let mut cy = y + 16.0;
        if editable && !off {
            let lw = ui.c.measure(&r.label, medium(11), 400.0).0 + 8.0;
            let rect = Rect::new(tx + lw, y, 70.0, 16.0);
            let resp = ui.hit(ui.id("selfull"), rect, false);
            ui.c.line_text(&t("View all"), medium(11), rect, pal.link, Align::Left);
            if resp.hover { ui.st.cursor_hand = true; }
            if resp.clicked { clicked = Some(link("selfull", &[])); }
        }
        if let Some(wn) = &r.warning {
            ui.c.glyph(0xE7BA, 11, Rect::new(tx, cy + 2.0, 14.0, 14.0), pal.warning);
            ui.c.line_text(wn, medium(11), Rect::new(tx + 18.0, cy, tw - 18.0, 16.0), pal.warning, Align::Left);
            cy += 16.0;
        }
        let line = ui.c.measure("가", sans(11), 100.0).1;
        let (_, qh) = ui.c.measure(&r.value, sans(11), tw);
        let qr = Rect::new(tx, cy, tw, qh.min(2.0 * line) + 1.0);
        ui.c.clip(qr);
        ui.c.text(&r.value, sans(11), Rect::new(tx, cy, tw, qh + 1.0), if off { pal.tertiary } else { pal.text }, Align::Left);
        ui.c.unclip();
    } else {
        ui.c.line_text(&r.label, medium(11), Rect::new(tx, y, 44.0, 20.0), label_ink, Align::Left);
        let vx = tx + 48.0;
        let vw = (tw - 48.0).max(10.0);
        let vtext = ui.c.measure(&r.value, sans(11), 10_000.0).0.min(vw);
        ui.c.line_text(&r.value, sans(11), Rect::new(vx, y, vw, 20.0), if off { pal.tertiary } else { pal.text }, Align::Left);
        if off { ui.c.line(vx, y + 10.0, vx + vtext, y + 10.0, pal.tertiary, 1.0); }
        let tail = if off { Some(t("Left out").into_owned()) } else { r.note.clone() };
        if let Some(n) = tail {
            let nx = vx + vtext + space::S;
            if nx < tx + tw { ui.c.line_text(&n, sans(11), Rect::new(nx, y, tx + tw - nx, 20.0), if off { pal.secondary } else { pal.tertiary }, Align::Left); }
        }
    }
    if editable {
        let key = format!("selrm{}", r.field.raw());
        if off {
            let rect = Rect::new(x + inner - right, y, right, 20.0);
            let resp = ui.hit(ui.id(&key), rect, true);
            ui.c.line_text(&t("Put back"), medium(11), rect, pal.link, Align::Right);
            if resp.hover { ui.st.cursor_hand = true; }
            if resp.clicked { clicked = Some(link("selrm", &[("f", r.field.raw())])); }
        } else {
            let rect = Rect::new(x + inner - 20.0, y, 20.0, 20.0);
            let resp = ui.hit(ui.id(&key), rect, true);
            ui.c.circle(rect.cx(), rect.cy(), 10.0, if resp.hover { pal.surface.mix(pal.text, 0.1) } else { pal.surface });
            ui.c.glyph(0xE711, 9, rect, pal.secondary);
            if resp.hover { ui.st.cursor_hand = true; }
            if resp.clicked { clicked = Some(link("selrm", &[("f", r.field.raw())])); }
        }
    }
    (h, clicked)
}

fn rows_height(ui: &Ui, rows: &[PetSelectionRow], inner: f64, editable: bool) -> f64 {
    rows.iter().map(|r| row_height(ui, r, inner, editable) + space::XS).sum()
}

/// `.card.ctx`: what will be sent, each row removable, Send / Cancel. Nothing has left the PC yet.
pub fn preview(ui: &mut Ui, pal: &Pal, p: &PetSelectionPreview, x: f64, y: f64, w: f64, clicked: &mut Vec<String>) -> f64 {
    let inner = w - 2.0 * PAD;
    let editable = !p.sending;
    let foot = t("Nothing goes to the server until you press Send.");
    let (_, foot_h) = ui.c.measure(&foot, sans(11), inner);
    let (_, head_h) = ui.c.measure(&p.title, medium(13), inner - 24.0);
    let h = PAD + head_h.max(18.0) + space::S + rows_height(ui, &p.rows, inner, editable) + space::XS + tok::CONTROL + space::XS + foot_h + PAD;
    card_frame(ui, pal, Rect::new(x, y, w, h));
    let mut cy = y + PAD;
    cy += head(ui, pal, p.action.symbol(), &p.title, x + PAD, cy, inner) + space::S;
    for r in &p.rows {
        let (rh, c) = context_row(ui, pal, r, x + PAD, cy, inner, editable);
        if let Some(c) = c { clicked.push(c); }
        cy += rh + space::XS;
    }
    cy += space::XS;
    let label = if p.sending { t("Sending…").into_owned() } else { t("Send").into_owned() };
    let sw = pill_width(ui, &label);
    let can = p.can_send;
    let send_r = Rect::new(x + PAD, cy, sw, tok::CONTROL);
    if pill(ui, "selsend", &label, send_r, Some(if can { pal.accent } else { pal.surface }), if can { pal.on_accent } else { pal.tertiary }, None, can) { clicked.push(link("selsend", &[])); }
    let cl = t("Cancel").into_owned();
    let cw = pill_width(ui, &cl);
    if pill(ui, "selcancel", &cl, Rect::new(send_r.right() + space::S, cy, cw, tok::CONTROL), Some(pal.surface), pal.text, Some(pal.separator), !p.sending) { clicked.push(link("selcancel", &[])); }
    cy += tok::CONTROL + space::XS;
    ui.c.text(&foot, sans(11), Rect::new(x + PAD, cy, inner, foot_h + 1.0), pal.tertiary, Align::Left);
    h
}

/// `.reqchip` under the sent request: which app, how much was selected, how many context rows.
pub fn chip(ui: &mut Ui, pal: &Pal, text: &str, x: f64, y: f64, w: f64) -> f64 {
    let tw = ui.c.measure(text, sans(11), 600.0).0.ceil() + 2.0 * space::S + 16.0;
    let cw = tw.min(w);
    let r = Rect::new(x + w - cw, y, cw, tok::CHIP_HEIGHT);
    ui.c.round(r, rad::CHIP, pal.mine_bubble);
    ui.c.glyph(glyph("quote.opening"), 11, Rect::new(r.x + space::S, r.y, 14.0, r.h), pal.mine_text.alpha(0.85));
    ui.c.line_text(text, sans(11), Rect::new(r.x + space::S + 18.0, r.y, r.w - space::S - 18.0 - 4.0, r.h), pal.mine_text.alpha(0.85), Align::Left);
    tok::CHIP_HEIGHT
}

/// `.answer-acts`: Copy · Again · View what was sent.
pub fn answer_actions(ui: &mut Ui, pal: &Pal, turn: &str, showing_sent: bool, x: f64, y: f64, clicked: &mut Vec<String>) -> f64 {
    let items = [
        ("selcopy", "doc.on.doc", t("Copy").into_owned()),
        ("selagain", "arrow.clockwise", t("Again").into_owned()),
        ("selshow", "doc.text", if showing_sent { t("Hide what was sent").into_owned() } else { t("Show what was sent").into_owned() }),
    ];
    let mut cx = x;
    for (action, symbol, label) in items {
        let lw = ui.c.measure(&label, sans(11), 400.0).0.ceil();
        let r = Rect::new(cx, y, lw + 2.0 * space::S + 16.0, 22.0);
        let resp = ui.hit(ui.id(&format!("{action}{turn}")), r, true);
        ui.c.round(r, rad::CHIP, if resp.hover { pal.surface.mix(pal.text, 0.06) } else { pal.surface });
        ui.c.stroke_round(r, rad::CHIP, pal.separator, 1.0);
        ui.c.glyph(glyph(symbol), 11, Rect::new(r.x + space::S, r.y, 14.0, r.h), pal.secondary);
        ui.c.line_text(&label, sans(11), Rect::new(r.x + space::S + 18.0, r.y, lw + 2.0, r.h), pal.secondary, Align::Left);
        if resp.hover { ui.st.cursor_hand = true; }
        if resp.clicked { clicked.push(link(action, &[("turn", turn)])); }
        cx += r.w + space::XS;
    }
    22.0
}

/// What a request sent (read only), opened from "Show what was sent".
pub fn sent_context(ui: &mut Ui, pal: &Pal, rows: &[PetSelectionRow], x: f64, y: f64, w: f64) -> f64 {
    let inner = w - 2.0 * PAD;
    let title = t("What was sent");
    let h = PAD + 18.0 + space::S + rows_height(ui, rows, inner, false) + PAD - space::XS;
    card_frame(ui, pal, Rect::new(x, y, w, h));
    let mut cy = y + PAD;
    cy += head(ui, pal, "doc.text", &title, x + PAD, cy, inner) + space::S;
    for r in rows {
        let (rh, _) = context_row(ui, pal, r, x + PAD, cy, inner, false);
        cy += rh + space::XS;
    }
    h
}

/// First-use card: what the bar does, Turn on / Later, and that nothing is read before it is switched on.
pub fn intro(ui: &mut Ui, pal: &Pal, x: f64, y: f64, w: f64, clicked: &mut Vec<String>) -> f64 {
    let inner = w - 2.0 * PAD;
    let title = t("New: the selection bar");
    let body = t("Select text in any app and a small bar appears. Press Translate, Summarize or Explain, or ask Nacho right away. You see what will be sent before it goes.");
    let note = t("Nothing is read until you turn it on, and password fields are never read.");
    let (_, bh) = ui.c.measure(&body, sans(11), inner);
    let (_, nh) = ui.c.measure(&note, sans(11), inner);
    let h = PAD + 18.0 + space::S + 34.0 + space::S + bh + space::S + tok::CONTROL + space::S + nh + PAD;
    card_frame(ui, pal, Rect::new(x, y, w, h));
    let mut cy = y + PAD;
    cy += head(ui, pal, "sparkles", &title, x + PAD, cy, inner) + space::S;
    // The bar in small: the icons only.
    let acts = [PetSelectionAction::Copy, PetSelectionAction::Search, PetSelectionAction::Translate, PetSelectionAction::Summarize, PetSelectionAction::Explain];
    let bw = 8.0 + acts.len() as f64 * 28.0 + 8.0 + 30.0;
    let bar = Rect::new(x + (w - bw) / 2.0, cy, bw, tok::SELECT_BAR_HEIGHT);
    ui.c.round(bar, rad::SELECT_BAR, pal.window);
    ui.c.stroke_round(bar, rad::SELECT_BAR, pal.edge, 1.0);
    for (i, a) in acts.iter().enumerate() {
        ui.c.glyph(glyph(a.symbol()), 14, Rect::new(bar.x + 4.0 + i as f64 * 28.0, bar.y, 28.0, bar.h), pal.secondary);
    }
    let nx = bar.x + 4.0 + acts.len() as f64 * 28.0 + 4.0;
    ui.c.line(nx, bar.cy() - 9.0, nx, bar.cy() + 9.0, pal.separator, 1.0);
    ui.c.image(&crate::pet_art::key_of("idle"), || crate::pet_art::image("idle").map(|p| (*p).clone()), Rect::new(nx + 8.0, bar.cy() - 9.0, 18.0, 18.0), 1.0);
    cy += 34.0 + space::S;
    ui.c.text(&body, sans(11), Rect::new(x + PAD, cy, inner, bh + 1.0), pal.secondary, Align::Left);
    cy += bh + space::S;
    let on = t("Turn on").into_owned();
    let ow = pill_width(ui, &on);
    if pill(ui, "selon", &on, Rect::new(x + PAD, cy, ow, tok::CONTROL), Some(pal.accent_soft), pal.text, Some(pal.accent), true) { clicked.push(link("selintro", &[("a", "on")])); }
    let later = t("Later").into_owned();
    let lw = pill_width(ui, &later);
    if pill(ui, "sellater", &later, Rect::new(x + PAD + ow + space::S, cy, lw, tok::CONTROL), Some(pal.surface), pal.text, Some(pal.separator), true) { clicked.push(link("selintro", &[("a", "later")])); }
    cy += tok::CONTROL + space::S;
    ui.c.text(&note, sans(11), Rect::new(x + PAD, cy, inner, nh + 1.0), pal.tertiary, Align::Left);
    h
}
