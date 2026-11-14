//! A small immediate-mode UI over the canvas. A window repaints only on input or when its data changes, so an idle
//! window costs nothing; widgets are plain function calls that return what the user did.
use crate::core::layout::Rect;
use crate::gfx::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

pub type Id = u64;

pub fn id_of(s: &str) -> Id {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Key { pub vk: u32, pub ctrl: bool, pub shift: bool, pub alt: bool, pub win: bool }

/// What survives between frames.
#[derive(Default)]
pub struct UiState {
    pub mouse: (f64, f64),
    pub inside: bool,
    pub down: bool,
    pub click: Option<(f64, f64)>,
    pub right_click: Option<(f64, f64)>,
    pub double_click: bool,
    pub wheel: f64,
    pub hot: Option<Id>,
    pub active: Option<Id>,
    pub focus: Option<Id>,
    /// The focused widget is shown with a ring only after keyboard use.
    pub keyboard_used: bool,
    pub keys: Vec<Key>,
    pub chars: Vec<char>,
    pub activate: bool,
    pub tab: i32,
    pub scroll: HashMap<Id, f64>,
    pub content: HashMap<Id, f64>,
    pub hits: Vec<(Id, Rect)>,
    pub focusables: Vec<Id>,
    pub again: bool,
    pub cursor_hand: bool,
    pub cursor_text: bool,
    pub capture_key: Option<Id>,
}

impl UiState {
    /// Which widget is under the pointer, from the last frame.
    pub fn hit_at(&self, x: f64, y: f64) -> Option<Id> { self.hits.iter().rev().find(|h| h.1.contains(x, y)).map(|h| h.0) }
}

pub struct Ui<'a> {
    pub c: &'a Canvas,
    pub st: &'a mut UiState,
    pub th: Theme,
    pub w: f64,
    pub h: f64,
    clips: Vec<Rect>,
    new_hits: Vec<(Id, Rect)>,
    new_focus: Vec<Id>,
    scope: u64,
}

#[derive(Clone, Copy, Default)]
pub struct Resp { pub hover: bool, pub pressed: bool, pub clicked: bool, pub right: bool, pub focused: bool }

#[derive(Clone, Copy, PartialEq)]
pub enum Btn { Primary, Attention, Secondary, Board }

impl<'a> Ui<'a> {
    pub fn new(c: &'a Canvas, st: &'a mut UiState, th: Theme, w: f64, h: f64) -> Self {
        st.cursor_hand = false;
        st.cursor_text = false;
        Ui { c, st, th, w, h, clips: vec![], new_hits: vec![], new_focus: vec![], scope: 0 }
    }

    /// Ends the frame: hit regions go live, Tab moves the focus, and leftovers of this frame's input are dropped.
    pub fn finish(mut self) {
        let hits = std::mem::take(&mut self.new_hits);
        let focus = std::mem::take(&mut self.new_focus);
        if self.st.tab != 0 && !focus.is_empty() {
            let pos = self.st.focus.and_then(|f| focus.iter().position(|x| *x == f));
            let n = focus.len() as i32;
            let next = match pos { Some(p) => (p as i32 + self.st.tab + n) % n, None => if self.st.tab > 0 { 0 } else { n - 1 } };
            self.st.focus = Some(focus[next as usize]);
            self.st.again = true;
        }
        self.st.tab = 0;
        self.st.hot = self.st.inside.then(|| hits.iter().rev().find(|h| h.1.contains(self.st.mouse.0, self.st.mouse.1)).map(|h| h.0)).flatten();
        self.st.hits = hits;
        self.st.focusables = focus;
        self.st.click = None;
        self.st.right_click = None;
        self.st.double_click = false;
        self.st.activate = false;
        self.st.wheel = 0.0;
        self.st.keys.clear();
        self.st.chars.clear();
    }

    pub fn id(&self, s: &str) -> Id { id_of(s) ^ self.scope.wrapping_mul(0x9E3779B97F4A7C15) }
    pub fn in_scope<R>(&mut self, key: u64, f: impl FnOnce(&mut Self) -> R) -> R {
        let old = self.scope;
        self.scope = old.wrapping_mul(31).wrapping_add(key + 1);
        let r = f(self);
        self.scope = old;
        r
    }

    fn visible(&self, x: f64, y: f64) -> bool { self.clips.last().is_none_or(|c| c.contains(x, y)) }

    /// A clickable region. `focusable` regions take part in Tab and react to Space / Enter.
    pub fn hit(&mut self, id: Id, r: Rect, focusable: bool) -> Resp {
        let shown = match self.clips.last() { Some(c) => Rect::new(r.x.max(c.x), r.y.max(c.y), (r.right().min(c.right()) - r.x.max(c.x)).max(0.0), (r.bottom().min(c.bottom()) - r.y.max(c.y)).max(0.0)), None => r };
        self.new_hits.push((id, shown));
        if focusable { self.new_focus.push(id); }
        let focused = focusable && self.st.focus == Some(id);
        let hover = self.st.hot == Some(id);
        let mut clicked = self.st.click.is_some_and(|p| shown.contains(p.0, p.1) && self.visible(p.0, p.1) && self.st.hit_at(p.0, p.1) == Some(id));
        if clicked && focusable { self.st.focus = Some(id); }
        if focused && self.st.activate { clicked = true; }
        let right = self.st.right_click.is_some_and(|p| shown.contains(p.0, p.1) && self.st.hit_at(p.0, p.1) == Some(id));
        Resp { hover, pressed: hover && self.st.down, clicked, right, focused }
    }

    fn focus_ring(&self, r: Rect, rad: f64, on: bool) {
        if on && self.st.keyboard_used { self.c.stroke_round(r.inset(-2.0, -2.0), rad + 2.0, self.th.nacho_ink, 2.0); }
    }

    pub fn label(&self, s: &str, f: Font, x: f64, y: f64, w: f64, c: Color) -> f64 {
        let (_, h) = self.c.measure(s, f, w);
        self.c.text(s, f, Rect::new(x, y, w, h + 1.0), c, Align::Left);
        h
    }
    pub fn eyebrow(&self, s: &str, x: f64, y: f64, w: f64, c: Color) {
        self.c.line_text(&crate::i18n::t(s).to_uppercase(), mono(11), Rect::new(x, y, w, 16.0), c, Align::Left);
    }

    pub fn button_width(&self, s: &str, small: bool) -> f64 {
        let f = if small { semi(12) } else { semi(13) };
        self.c.measure(s, f, 600.0).0.ceil() + if small { 20.0 } else { 28.0 }
    }

    /// Returns whether it was clicked; draws at (x, y) and takes `button_width`.
    pub fn button(&mut self, key: &str, s: &str, x: f64, y: f64, style: Btn, small: bool) -> bool {
        let w = self.button_width(s, small);
        let h = if small { 24.0 } else { 30.0 };
        self.button_rect(key, s, Rect::new(x, y, w, h), style, small, true)
    }

    pub fn button_rect(&mut self, key: &str, s: &str, r: Rect, style: Btn, small: bool, enabled: bool) -> bool {
        let id = self.id(key);
        let resp = if enabled { self.hit(id, r, true) } else { Resp::default() };
        let (fill, stroke, text) = match style {
            Btn::Primary => (NACHO, ON_NACHO, ON_NACHO),
            Btn::Attention => (self.th.nacho_deep, self.th.nacho_deep, ON_NACHO),
            Btn::Secondary => (self.th.paper, self.th.line, self.th.ink),
            Btn::Board => (BOARD2, BOARD_LINE, BOARD_INK),
        };
        let mut fill = if resp.pressed { fill.mix(Color(0.0, 0.0, 0.0, 1.0), 0.15) } else if resp.hover { fill.mix(self.th.ink, 0.06) } else { fill };
        let mut text = text;
        if !enabled { fill = fill.alpha(0.4); text = text.alpha(0.4); }
        self.c.round(r, 8.0, fill);
        self.c.stroke_round(r, 8.0, if enabled { stroke } else { stroke.alpha(0.4) }, if style == Btn::Primary { 1.5 } else { 1.0 });
        self.c.line_text(s, if small { semi(12) } else { semi(13) }, r, text, Align::Center);
        self.focus_ring(r, 8.0, resp.focused);
        if resp.hover { self.st.cursor_hand = true; }
        resp.clicked
    }

    /// A switch; returns true when flipped.
    pub fn switch(&mut self, key: &str, x: f64, y: f64, on: bool) -> bool {
        let r = Rect::new(x, y, 40.0, 22.0);
        let id = self.id(key);
        let resp = self.hit(id, r, true);
        let th = self.th;
        if on {
            self.c.round(r, 11.0, th.guac);
            self.c.circle(x + 29.0, y + 11.0, 8.0, Color(1.0, 1.0, 1.0, 1.0));
        } else {
            self.c.round(r, 11.0, th.paper);
            self.c.stroke_round(r, 11.0, th.ink2, 1.5);
            self.c.circle(x + 11.0, y + 11.0, 6.0, th.ink2);
        }
        self.focus_ring(r, 11.0, resp.focused);
        if resp.hover { self.st.cursor_hand = true; }
        resp.clicked
    }

    pub fn checkbox(&mut self, key: &str, s: &str, x: f64, y: f64, w: f64, on: bool) -> bool {
        let r = Rect::new(x, y, w, 24.0);
        let id = self.id(key);
        let resp = self.hit(id, r, true);
        let b = Rect::new(x, y + 3.0, 18.0, 18.0);
        let th = self.th;
        if on { self.c.round(b, 4.0, th.guac); self.c.glyph(0xE73E, 12, b, Color(1.0, 1.0, 1.0, 1.0)); }
        else { self.c.stroke_round(b, 4.0, if resp.hover { th.ink } else { th.ink2 }, 1.5); }
        self.c.line_text(s, sans(13), Rect::new(x + 26.0, y, w - 26.0, 24.0), th.ink, Align::Left);
        self.focus_ring(b, 4.0, resp.focused);
        if resp.hover { self.st.cursor_hand = true; }
        resp.clicked
    }

    /// One of a few choices, as a row of segments. Returns the index clicked.
    pub fn segments(&mut self, key: &str, opts: &[String], cur: Option<usize>, r: Rect, board: bool) -> Option<usize> {
        let (bg, line, ink, mute) = if board { (BOARD2, BOARD_LINE, BOARD_INK, BOARD_MUTE) } else { (self.th.paper2, self.th.line, self.th.ink, self.th.ink2) };
        self.c.round(r, 7.0, bg);
        self.c.stroke_round(r, 7.0, line, 1.0);
        let w = r.w / opts.len().max(1) as f64;
        let mut res = None;
        for (i, o) in opts.iter().enumerate() {
            let cell = Rect::new(r.x + w * i as f64, r.y, w, r.h);
            let id = self.id(&format!("{key}#{i}"));
            let resp = self.hit(id, cell, true);
            let sel = cur == Some(i);
            if sel { self.c.round(cell.inset(2.0, 2.0), 5.0, NACHO); }
            else if resp.hover { self.c.round(cell.inset(2.0, 2.0), 5.0, ink.alpha(0.08)); }
            self.c.line_text(o, semi(12), cell.inset(4.0, 0.0), if sel { ON_NACHO } else if resp.hover { ink } else { mute }, Align::Center);
            self.focus_ring(cell.inset(2.0, 2.0), 5.0, resp.focused);
            if resp.hover { self.st.cursor_hand = true; }
            if resp.clicked { res = Some(i); }
        }
        res
    }

    /// Returns the new value while dragged or nudged with the arrow keys.
    pub fn slider(&mut self, key: &str, r: Rect, v: f64, min: f64, max: f64) -> Option<f64> {
        let id = self.id(key);
        let resp = self.hit(id, r, true);
        let th = self.th;
        let t = ((v - min) / (max - min)).clamp(0.0, 1.0);
        let cy = r.cy();
        self.c.round(Rect::new(r.x, cy - 2.0, r.w, 4.0), 2.0, th.line);
        self.c.round(Rect::new(r.x, cy - 2.0, r.w * t, 4.0), 2.0, th.nacho_deep);
        self.c.circle(r.x + r.w * t, cy, 8.0, NACHO);
        self.c.ring(r.x + r.w * t, cy, 8.0, ON_NACHO, 1.5);
        self.focus_ring(Rect::new(r.x + r.w * t - 8.0, cy - 8.0, 16.0, 16.0), 8.0, resp.focused);
        if resp.hover { self.st.cursor_hand = true; }
        if resp.hover && self.st.down { self.st.active = Some(id); }
        let mut out = None;
        if self.st.active == Some(id) {
            if self.st.down {
                let nv = min + ((self.st.mouse.0 - r.x) / r.w).clamp(0.0, 1.0) * (max - min);
                if (nv - v).abs() > 1e-6 { out = Some(nv); }
            } else { self.st.active = None; }
        }
        if resp.focused {
            let step = (max - min) / 20.0;
            for k in self.st.keys.clone() {
                if k.vk == 0x25 { out = Some((v - step).max(min)); }
                if k.vk == 0x27 { out = Some((v + step).min(max)); }
            }
        }
        out
    }

    pub fn text_field(&mut self, key: &str, r: Rect, value: &mut String, max: usize) -> bool {
        let id = self.id(key);
        let resp = self.hit(id, r, true);
        let th = self.th;
        let mut changed = false;
        if resp.focused {
            for ch in self.st.chars.clone() {
                if ch == '\u{8}' { value.pop(); changed = true; }
                else if !ch.is_control() && value.chars().count() < max { value.push(ch); changed = true; }
            }
        }
        self.c.round(r, 6.0, th.paper);
        self.c.stroke_round(r, 6.0, if resp.focused { th.nacho_ink } else { th.line }, if resp.focused { 2.0 } else { 1.0 });
        let shown = if resp.focused { format!("{value}|") } else { value.clone() };
        self.c.line_text(&shown, sans(13), r.inset(9.0, 0.0), th.ink, Align::Left);
        if resp.hover { self.st.cursor_text = true; }
        changed
    }

    /// A vertically scrolling area. `body` draws from the y it is given and returns the y it ended at.
    pub fn scroll(&mut self, key: &str, view: Rect, body: impl FnOnce(&mut Self, f64) -> f64) {
        let id = self.id(key);
        let content = self.st.content.get(&id).copied().unwrap_or(0.0);
        let mut s = self.st.scroll.get(&id).copied().unwrap_or(0.0);
        if self.st.wheel != 0.0 && view.contains(self.st.mouse.0, self.st.mouse.1) { s -= self.st.wheel; self.st.again = true; }
        s = s.clamp(0.0, (content - view.h).max(0.0));
        self.st.scroll.insert(id, s);
        self.c.clip(view);
        self.clips.push(view);
        let end = body(self, view.y - s);
        self.clips.pop();
        self.c.unclip();
        let total = end + s - view.y;
        if (total - content).abs() > 0.5 { self.st.content.insert(id, total); self.st.again = true; }
        if total > view.h {
            let bar = (view.h * view.h / total).max(24.0);
            let y = view.y + s / (total - view.h) * (view.h - bar);
            self.c.round(Rect::new(view.right() - 6.0, y, 4.0, bar), 2.0, self.th.ink2.alpha(0.4));
        }
    }

    pub fn card(&self, r: Rect) { self.c.round(r, 10.0, self.th.paper2); self.c.stroke_round(r, 10.0, self.th.line, 1.0); }

    /// Key caps in a row; returns the width used.
    pub fn keycaps(&self, parts: &[String], x: f64, y: f64) -> f64 {
        let mut cx = x;
        for p in parts {
            let w = self.c.measure(p, mono(11), 200.0).0.ceil() + 12.0;
            let r = Rect::new(cx, y, w.max(22.0), 22.0);
            self.c.round(r, 5.0, self.th.paper);
            self.c.stroke_round(r, 5.0, self.th.ink2.alpha(0.6), 1.0);
            self.c.line_text(p, mono(11), r, self.th.ink, Align::Center);
            cx += r.w + 3.0;
        }
        cx - x
    }

    pub fn status_dot(&self, x: f64, y: f64, c: Color) { self.c.circle(x, y, 4.5, c); }
}

/// The nacho box: the app's mark, drawn so it stays sharp at any size. `s` is the side of the square at (x, y).
pub fn draw_box(c: &Canvas, x: f64, y: f64, s: f64) {
    let u = s / 16.0;
    c.polygon(&[(x + 3.0 * u, y + 6.0 * u), (x + 13.0 * u, y + 6.0 * u), (x + 12.0 * u, y + 15.0 * u), (x + 4.0 * u, y + 15.0 * u)], NACHO);
    c.polygon(&[(x + 3.0 * u, y + 6.0 * u), (x + 8.0 * u, y + 1.0 * u), (x + 13.0 * u, y + 6.0 * u)], rgb(0xE48A0C));
    c.circle(x + 6.3 * u, y + 9.3 * u, 0.8 * u, ON_NACHO);
    c.circle(x + 9.7 * u, y + 9.3 * u, 0.8 * u, ON_NACHO);
    c.arc(x + 8.0 * u, y + 10.8 * u, 1.6 * u, 0.0, ON_NACHO, 0.8 * u);
    c.line(x + 6.6 * u, y + 11.6 * u, x + 9.4 * u, y + 11.6 * u, ON_NACHO, 0.8 * u);
}

/// Each gadget's mark, drawn with plain shapes in `c` inside the square at (x, y).
pub fn gadget_icon(cv: &Canvas, id: crate::core::settings::ModuleId, x: f64, y: f64, s: f64, c: Color) {
    use crate::core::settings::ModuleId::*;
    let code = match id {
        TrayOrganizer => 0xE8FD,  // bulleted list
        SystemMonitor => 0xE9D9,  // diagnostic
        WindowSnap => 0xF57C,     // tiles-like, see below
        FileShelf => 0xE8B7,      // folder
        CaptureFloat => 0xE722,   // camera
        FocusDim => 0xE706,       // brightness
        KeepAwake => 0xEC32,      // coffee cup
        Clipboard => 0xE77F,      // paste
        DeviceControl => 0xE767,  // volume
        DiskMap => 0xEDA2,        // hard drive
        AppSwitch => 0xE7C4,      // task view
        CalendarClock => 0xE787,  // calendar
    };
    if id == WindowSnap {
        // A split window: the font's snap glyphs differ between Windows versions, so it is drawn.
        let u = s / 16.0;
        let w = Rect::new(x + u, y + 2.0 * u, 14.0 * u, 12.0 * u);
        cv.stroke_round(w, 1.5 * u, c, 1.3 * u);
        cv.round(Rect::new(w.x + 1.6 * u, w.y + 1.6 * u, 4.8 * u, 8.8 * u), 0.8 * u, c);
        return;
    }
    cv.glyph(code, (s * 0.92) as u32, Rect::new(x, y, s, s), c);
}
