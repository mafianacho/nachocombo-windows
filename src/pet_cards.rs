//! How a card is drawn in the chat: approval, choice, suggestions, rule proposal and the plan card of a task. What a
//! card says and does comes from the core (`PetCardItem`, `PetCardFlow`); this only paints it and reports the click.
//! One drawer for every place that shows a card (the chat today, the desktop-actions work next), so the cards look and
//! answer the same everywhere.
use crate::core::layout::Rect;
use crate::core::pet::card_rules::{PetCardRowRole, PetCardStyle};
use crate::core::pet::transcript::{PetCardItem, PetCardItemOption};
use crate::gfx::*;
use crate::pet_theme::Pal;
use crate::ui::*;

const PAD: f64 = 12.0;

/// Segoe icon code point for the card's SF-symbol-like key.
fn glyph_of(symbol: &str) -> u32 {
    match symbol {
        s if s.contains("shield") || s.contains("lock") => 0xE72E,
        s if s.contains("checklist") || s.contains("list") => 0xE762,
        s if s.contains("calendar") => 0xE787,
        s if s.contains("clock") || s.contains("timer") => 0xE916,
        s if s.contains("bolt") || s.contains("play") => 0xE768,
        s if s.contains("exclamation") || s.contains("warning") => 0xE7BA,
        s if s.contains("lightbulb") || s.contains("sparkle") => 0xE82F,
        _ => 0xE946,
    }
}

/// Draws `item` at (x, y) with width `w`. Returns its height and the link of what was clicked (the shell acts on it).
pub fn draw_card(ui: &mut Ui, pal: &Pal, item: &PetCardItem, x: f64, y: f64, w: f64) -> (f64, Option<String>) {
    let inner = w - 2.0 * PAD;
    let body = sans(13);
    let cap = sans(11);
    let mut clicked: Option<String> = None;
    // Measure first: the box is drawn under its content.
    let (tw, th) = ui.c.measure(&item.title, semi(13), inner - 24.0);
    let mut h = PAD + th.max(18.0) + 4.0;
    let _ = tw;
    if let Some(d) = &item.detail { h += ui.c.measure(d, cap, inner).1 + 4.0; }
    for s in &item.summary { h += ui.c.measure(s, cap, inner - 10.0).1 + 2.0; }
    let all_options: Vec<&PetCardItemOption> = item.options.iter().chain(item.confirm.iter()).collect();
    for o in &all_options { h += row_height(ui, o, inner) + 6.0; }
    if let Some(s) = &item.status { h += ui.c.measure(s, cap, inner).1 + 4.0; }
    if let Some(s) = &item.hint { h += ui.c.measure(s, cap, inner).1 + 4.0; }
    if item.retry.is_some() { h += 24.0; }
    h += PAD - 2.0;

    let r = Rect::new(x, y, w, h);
    ui.c.round(r, 10.0, pal.raised);
    let border = if item.style == PetCardStyle::Approval && item.status.as_deref().is_none_or(|s| item.status_waits || s.is_empty()) { pal.warning } else { pal.edge };
    ui.c.stroke_round(r, 10.0, border, 1.0);
    let mut cy = y + PAD;
    ui.c.glyph(glyph_of(&item.symbol), 14, Rect::new(x + PAD, cy, 18.0, 18.0), if item.style == PetCardStyle::Approval { pal.warning } else { pal.accent });
    ui.c.text(&item.title, semi(13), Rect::new(x + PAD + 24.0, cy, inner - 24.0, th + 2.0), pal.text, Align::Left);
    cy += th.max(18.0) + 4.0;
    if let Some(d) = &item.detail {
        let (_, dh) = ui.c.measure(d, cap, inner);
        ui.c.text(d, cap, Rect::new(x + PAD, cy, inner, dh + 1.0), pal.secondary, Align::Left);
        cy += dh + 4.0;
    }
    for s in &item.summary {
        let (_, sh) = ui.c.measure(s, cap, inner - 10.0);
        ui.c.text("·", cap, Rect::new(x + PAD, cy, 8.0, sh), pal.tertiary, Align::Left);
        ui.c.text(s, cap, Rect::new(x + PAD + 10.0, cy, inner - 10.0, sh + 1.0), pal.secondary, Align::Left);
        cy += sh + 2.0;
    }
    for (i, o) in all_options.iter().enumerate() {
        let rh = row_height(ui, o, inner);
        let rect = Rect::new(x + PAD, cy, inner, rh);
        if let Some(link) = draw_option(ui, pal, &format!("{}:{i}", item.id), o, rect, body, cap) { clicked = Some(link); }
        cy += rh + 6.0;
    }
    if let Some(s) = &item.status {
        let (_, sh) = ui.c.measure(s, cap, inner);
        ui.c.text(s, cap, Rect::new(x + PAD, cy, inner, sh + 1.0), if item.status_waits { pal.secondary } else { pal.warning }, Align::Left);
        cy += sh + 4.0;
    }
    if let Some(s) = &item.hint {
        let (_, sh) = ui.c.measure(s, cap, inner);
        ui.c.text(s, cap, Rect::new(x + PAD, cy, inner, sh + 1.0), pal.tertiary, Align::Left);
        cy += sh + 4.0;
    }
    if let Some(link) = &item.retry {
        let rect = Rect::new(x + PAD, cy, 90.0, 22.0);
        let resp = ui.hit(ui.id(&format!("retry:{}", item.id)), rect, false);
        ui.c.line_text(&crate::i18n::t("Try again"), medium(12), rect, pal.link, Align::Left);
        if resp.hover { ui.st.cursor_hand = true; }
        if resp.clicked { clicked = Some(link.clone()); }
    }
    (h, clicked)
}

fn row_height(ui: &Ui, o: &PetCardItemOption, w: f64) -> f64 {
    let label_w = w - 16.0 - if o.number > 0 || o.tickable { 26.0 } else { 0.0 };
    let (_, lh) = ui.c.measure(&o.label, medium(13), label_w);
    let dh = o.detail.as_ref().map_or(0.0, |d| ui.c.measure(d, sans(11), label_w).1 + 1.0);
    (lh + dh + 12.0).max(30.0)
}

fn draw_option(ui: &mut Ui, pal: &Pal, key: &str, o: &PetCardItemOption, r: Rect, body: Font, cap: Font) -> Option<String> {
    let enabled = o.link.is_some();
    let resp = ui.hit(ui.id(key), r, false);
    let (fill, line, ink) = match (o.role, o.chosen || o.marked) {
        (PetCardRowRole::Primary, _) => (Some(pal.accent), None, pal.on_accent),
        (PetCardRowRole::Quiet, _) => (None, None, pal.secondary),
        (_, true) => (Some(pal.accent_soft), Some(pal.accent), pal.text),
        _ => (Some(pal.surface), Some(pal.edge), pal.text),
    };
    let hot = enabled && resp.hover;
    if let Some(f) = fill { ui.c.round(r, 8.0, if hot { f.mix(pal.text, 0.06) } else { f }); }
    else if hot { ui.c.round(r, 8.0, pal.surface); }
    if let Some(l) = line { ui.c.stroke_round(r, 8.0, l, if o.marked { 2.0 } else { 1.0 }); }
    if resp.focused && ui.st.keyboard_used { ui.c.stroke_round(r.inset(-2.0, -2.0), 10.0, pal.accent, 2.0); }
    let mut tx = r.x + 8.0;
    if o.tickable {
        let b = Rect::new(r.x + 8.0, r.cy() - 8.0, 16.0, 16.0);
        ui.c.stroke_round(b, 4.0, if o.marked { pal.accent } else { pal.tertiary }, 1.5);
        if o.marked { ui.c.round(b.inset(3.0, 3.0), 2.0, pal.accent); }
        tx += 26.0;
    } else if o.number > 0 {
        ui.c.line_text(&o.number.to_string(), mono(11), Rect::new(r.x + 8.0, r.y, 16.0, r.h.min(30.0)), pal.tertiary, Align::Center);
        tx += 26.0;
    }
    let tw = r.right() - 8.0 - tx;
    let align = if o.role == PetCardRowRole::Primary || o.role == PetCardRowRole::Quiet { if o.number > 0 { Align::Left } else { Align::Center } } else { Align::Left };
    let (_, lh) = ui.c.measure(&o.label, if o.role == PetCardRowRole::Primary { semi(13) } else { medium(13) }, tw);
    let text_h = lh + o.detail.as_ref().map_or(0.0, |d| ui.c.measure(d, cap, tw).1 + 1.0);
    let top = r.y + (r.h - text_h) / 2.0;
    let ink = if enabled || o.chosen { ink } else { ink.alpha(0.55) };
    ui.c.text(&o.label, if o.role == PetCardRowRole::Primary { semi(13) } else { let _ = body; medium(13) }, Rect::new(tx, top, tw, lh + 1.0), ink, align);
    if let Some(d) = &o.detail { ui.c.text(d, cap, Rect::new(tx, top + lh + 1.0, tw, r.h), if o.role == PetCardRowRole::Primary { pal.on_accent.alpha(0.85) } else { pal.secondary }, align); }
    if o.chosen { ui.c.glyph(0xE73E, 12, Rect::new(r.right() - 24.0, r.cy() - 8.0, 16.0, 16.0), pal.accent); }
    if resp.hover && enabled { ui.st.cursor_hand = true; }
    if resp.clicked { o.link.clone() } else { None }
}
