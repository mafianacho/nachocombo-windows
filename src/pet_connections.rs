//! The 연결 tab of the pet's chat bubble: the list (도구 / 커넥터 / 가젯) and the detail of one row. What it says is
//! decided in `core/pet/connections.rs`; this file draws it and carries out the switches:
//!   gadget switch → the app's own module switch (`App::set_enabled`);
//!   "항상 허용" and a single action's switch → `pet_exec::trust()` (every change declares the capabilities again);
//!   tool / service switch, scope, disconnect → the server calls of `ConnectionsAPI`.
use crate::app::{app, later};
use crate::core::layout::Rect;
use crate::core::pet::connections::*;
use crate::core::pet::connectors::{ConnectorOverview, ToolRow};
use crate::gfx::*;
use crate::pet_theme::Pal;
use crate::pet_ui as pet;
use crate::ui::*;
use std::collections::BTreeSet;

/// The two tabs of the bubble.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum ChatTab { #[default] Talk, Connections }

/// A question the user must answer before something is switched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm { AlwaysAllow(&'static str), Disconnect(String) }

#[derive(Default)]
pub struct ConnState {
    pub overview: Option<ConnectorOverview>,
    pub loading: bool,
    /// The server list could not be read (the gadget section still shows).
    pub error: Option<String>,
    pub requested: bool,
    pub detail: Option<ConnectionsRef>,
    pub confirm: Option<Confirm>,
    /// A server call is on its way: the switches that call the server wait for it.
    pub busy: bool,
}

/// What came back from the server (delivered on the UI thread through `pet_ui::Event::Connections`).
pub enum Done {
    Overview(Result<ConnectorOverview, String>),
    /// A switch, a scope or a disconnect finished. The updated tool row comes with a tool switch.
    Changed(Result<Option<ToolRow>, ConnectionsFailure>),
}

pub fn on_done(d: Done) {
    let c = &mut app().pet.chat.conn;
    match d {
        Done::Overview(Ok(o)) => { c.overview = Some(o); c.error = None; c.loading = false; }
        Done::Overview(Err(e)) => { c.error = Some(e); c.loading = false; }
        Done::Changed(r) => {
            c.busy = false;
            match r {
                Ok(Some(row)) => {
                    if let Some(o) = c.overview.as_mut() { if let Some(t) = o.tools.iter_mut().find(|t| t.id == row.id) { t.user_enabled = row.user_enabled; t.locked = row.locked; } }
                }
                // The row changed on the server (its version too): read the list again.
                Ok(None) => { c.confirm = None; load(); }
                Err(f) => { pet::flash_text(f.korean_message()); if f.needs_reload() { load(); } }
            }
        }
    }
    crate::pet_chat::invalidate();
}

/// Reads the server list again.
pub fn load() {
    let c = &mut app().pet.chat.conn;
    c.requested = true;
    c.loading = true;
    let Some(s) = pet::session() else { c.loading = false; return };
    s.load_connectors(Box::new(|r| pet::push(pet::Event::Connections(Done::Overview(r.map_err(|e| e.korean_message()))))));
}

/// The tab was opened: read the list once.
pub fn opened() { if !app().pet.chat.conn.requested { load(); } }

fn call(f: impl FnOnce(ConnectionsAPI, ConnectionsCompletion<Option<ToolRow>>) + Send + 'static) {
    let c = &mut app().pet.chat.conn;
    if c.busy { return; }
    let Some(s) = pet::session() else { return };
    c.busy = true;
    s.with_connections_api(Box::new(move |r| match r {
        Err(e) => pet::push(pet::Event::Connections(Done::Changed(Err(e)))),
        Ok(api) => f(api, Box::new(|r| pet::push(pet::Event::Connections(Done::Changed(r))))),
    }));
}

fn set_tool(id: String, on: bool) { call(move |api, done| api.set_tool_enabled(&id, on, Box::new(move |r| done(r.map(Some))))); }
fn set_connection(id: String, on: bool, version: i64) { call(move |api, done| api.set_connection_enabled(&id, on, version, Box::new(move |r| done(r.map(|_| None))))); }
fn set_scope(id: String, level: String, version: i64) { call(move |api, done| api.set_scope_level(&id, &level, version, Box::new(move |r| done(r.map(|_| None))))); }
fn disconnect(id: String) { call(move |api, done| api.disconnect(&id, Box::new(move |r| done(r.map(|_| None))))); }

fn gadget_states() -> Vec<GadgetSwitchState> {
    use crate::app::State;
    GadgetSwitchState::listed().into_iter().map(|(module, gadget)| GadgetSwitchState {
        module, gadget,
        phase: match app().state(module) { State::Running => GadgetPhase::On, State::Off => GadgetPhase::Off, State::Failed(_) => GadgetPhase::Failed },
    }).collect()
}

enum Act {
    Open(ConnectionsRef),
    Back,
    Refresh,
    Gadget(&'static str, bool),
    AlwaysAllowAsk(&'static str),
    AlwaysAllow(&'static str, bool),
    Action(String, bool),
    Tool(String, bool),
    Connector(String, bool, i64),
    Scope(String, String, i64),
    AskDisconnect(String),
    Disconnect(String),
    Cancel,
}

fn tone_color(pal: &Pal, t: ConnectionsTone) -> Color {
    match t { ConnectionsTone::Normal => pal.secondary, ConnectionsTone::Attention => pal.warning, ConnectionsTone::Off => pal.tertiary }
}

/// A switch in the pet's colours. Returns true when clicked.
fn switch(ui: &mut Ui, pal: &Pal, key: &str, x: f64, y: f64, on: bool, enabled: bool) -> bool {
    let r = Rect::new(x, y, 38.0, 22.0);
    let resp = if enabled { ui.hit(ui.id(key), r, true) } else { Resp::default() };
    let (track, knob) = if on { (pal.accent, pal.on_accent) } else { (pal.surface, pal.secondary) };
    let a = if enabled { 1.0 } else { 0.45 };
    ui.c.round(r, 11.0, track.alpha(a));
    if !on { ui.c.stroke_round(r, 11.0, pal.edge.alpha(a), 1.0); }
    ui.c.circle(if on { r.right() - 11.0 } else { r.x + 11.0 }, r.cy(), 7.0, knob.alpha(a));
    if resp.hover && enabled { ui.st.cursor_hand = true; }
    if resp.focused && ui.st.keyboard_used { ui.c.stroke_round(r.inset(-2.0, -2.0), 13.0, pal.accent, 2.0); }
    resp.clicked
}

fn para(ui: &Ui, s: &str, f: Font, x: f64, y: f64, w: f64, c: Color) -> f64 { ui.label(&crate::i18n::t(s), f, x, y, w, c) }

/// The control at the right end of a row or a detail: a switch, a lock, or nothing.
fn control(ui: &mut Ui, pal: &Pal, key: &str, c: &ConnectionsSwitch, right: f64, cy: f64, busy_blocks: bool) -> Option<bool> {
    match c {
        ConnectionsSwitch::On | ConnectionsSwitch::Off => {
            let on = *c == ConnectionsSwitch::On;
            switch(ui, pal, key, right - 38.0, cy - 11.0, on, !busy_blocks).then_some(!on)
        }
        ConnectionsSwitch::Locked => { ui.c.glyph(0xE72E, 14, Rect::new(right - 38.0, cy - 11.0, 38.0, 22.0), pal.secondary); None }
        ConnectionsSwitch::Fixed(_) => None,
    }
}

pub fn draw(ui: &mut Ui, pal: &Pal, view: Rect) {
    use crate::i18n::t;
    let c = &app().pet.chat.conn;
    let (overview, loading, error, detail_ref, confirm, busy) = (c.overview.clone(), c.loading, c.error.clone(), c.detail.clone(), c.confirm.clone(), c.busy);
    let trust = crate::pet_exec::trust();
    let (disabled, always, trustable) = (trust.disabled(), trust.always_allowed(), trust.trustable().clone());
    let gadgets = gadget_states();
    let names = declared_names(false);
    let allow = app().pet.settings.allow_combo_control;
    let input = ConnectionsInput { overview: overview.as_ref(), gadgets: &gadgets, disabled: &disabled, always_allowed: &always, trustable: &trustable, names: &names, allow_combo_control: allow };
    let detail_data = detail_ref.as_ref().and_then(|r| detail(r, &input));
    let version_of = |id: &str| overview.as_ref().and_then(|o| o.connections.iter().find(|c| c.id == id)).map(|c| c.version).unwrap_or(0);
    let mut acts: Vec<Act> = vec![];
    let m = 16.0;
    let w = view.w;

    ui.scroll("connections", view, |ui, y0| {
        let mut y = y0 + 8.0;
        match (&detail_ref, &detail_data) {
            (Some(_), Some(d)) => y = draw_detail(ui, pal, d, &confirm, busy, &version_of, w, y, &mut acts),
            _ => {
                if detail_ref.is_some() { acts.push(Act::Back); }
                // Top line: a note when Nacho may not use this PC, and the refresh button.
                let r = Rect::new(w - m - 28.0, y, 28.0, 28.0);
                let resp = ui.hit(ui.id("conn-refresh"), r, true);
                if resp.hover { ui.c.round(r, 14.0, pal.surface); ui.st.cursor_hand = true; }
                ui.c.glyph(0xE72C, 13, r, if loading { pal.tertiary } else { pal.secondary });
                if resp.clicked && !loading { acts.push(Act::Refresh); }
                let state = if loading { t("불러오는 중…") } else if let Some(e) = &error { t(e) } else { std::borrow::Cow::Borrowed("") };
                if !state.is_empty() { ui.c.line_text(&state, sans(11), Rect::new(m, y, w - 2.0 * m - 36.0, 28.0), if error.is_some() { pal.warning } else { pal.secondary }, Align::Left); }
                y += 34.0;
                if !allow {
                    let n = t("나초가 이 PC를 쓰지 못하게 꺼 두었어요. 설정에서 다시 켤 수 있어요.");
                    let (_, nh) = ui.c.measure(&n, sans(11), w - 2.0 * m - 16.0);
                    ui.c.round(Rect::new(m, y, w - 2.0 * m, nh + 16.0), 8.0, pal.warning_soft);
                    ui.c.text(&n, sans(11), Rect::new(m + 8.0, y + 8.0, w - 2.0 * m - 16.0, nh + 2.0), pal.text, Align::Left);
                    y += nh + 24.0;
                }
                for sec in sections(&input, "") {
                    ui.c.line_text(&format!("{}  {}", t(sec.section.title()), sec.rows.len()), semi(12), Rect::new(m, y, w - 2.0 * m, 20.0), pal.secondary, Align::Left);
                    y += 24.0;
                    if sec.rows.is_empty() {
                        y += para(ui, sec.section.empty_text(), sans(12), m, y, w - 2.0 * m, pal.tertiary) + 14.0;
                        continue;
                    }
                    for (i, row) in sec.rows.iter().enumerate() {
                        let sid = (sec.section as u64) * 10_000 + i as u64;
                        y += ui.in_scope(sid, |ui| draw_row(ui, pal, row, busy, w, y, &version_of, &mut acts));
                    }
                    y += 8.0;
                }
            }
        }
        y + 12.0
    });

    for a in acts { apply(a, &gadgets, &disabled); }
}

fn draw_row(ui: &mut Ui, pal: &Pal, row: &ConnectionsRow, busy: bool, w: f64, y: f64, version_of: &dyn Fn(&str) -> i64, acts: &mut Vec<Act>) -> f64 {
    use crate::i18n::t;
    let (m, h) = (16.0, 52.0);
    let r = Rect::new(8.0, y, w - 16.0, h);
    let open = ui.hit(ui.id("open"), r, true);
    if open.hover { ui.c.round(r, 8.0, pal.surface); ui.st.cursor_hand = true; }
    let has_switch = !matches!(row.control, ConnectionsSwitch::Fixed(_));
    let tw = w - 2.0 * m - if has_switch { 52.0 } else { 20.0 };
    ui.c.line_text(&row.title, semi(13), Rect::new(m, y + 7.0, tw, 20.0), pal.text, Align::Left);
    ui.c.line_text(&t(&row.status), sans(11), Rect::new(m, y + 27.0, tw, 18.0), tone_color(pal, row.tone), Align::Left);
    let mut toggled = None;
    if has_switch { toggled = control(ui, pal, "sw", &row.control, w - m, y + h / 2.0, busy); } else { ui.c.glyph(0xE76C, 11, Rect::new(w - m - 14.0, y, 14.0, h), pal.tertiary); }
    ui.c.line(m, y + h, w - m, y + h, pal.separator, 1.0);
    match (&row.target, toggled) {
        (target, None) => if open.clicked { acts.push(Act::Open(target.clone())); },
        (ConnectionsRef::Gadget(g), Some(on)) => acts.push(Act::Gadget(g, on)),
        (ConnectionsRef::Tool(id), Some(on)) => acts.push(Act::Tool(id.clone(), on)),
        (ConnectionsRef::Connector(id), Some(on)) => acts.push(Act::Connector(id.clone(), on, version_of(id))),
    }
    h
}

#[allow(clippy::too_many_arguments)]
fn draw_detail(ui: &mut Ui, pal: &Pal, d: &ConnectionsDetail, confirm: &Option<Confirm>, busy: bool, version_of: &dyn Fn(&str) -> i64, w: f64, y0: f64, acts: &mut Vec<Act>) -> f64 {
    use crate::i18n::t;
    let m = 16.0;
    let cw = w - 2.0 * m;
    let mut y = y0;
    // back
    let back = Rect::new(m - 6.0, y, 70.0, 28.0);
    let resp = ui.hit(ui.id("conn-back"), back, true);
    if resp.hover { ui.c.round(back, 8.0, pal.surface); ui.st.cursor_hand = true; }
    ui.c.glyph(0xE72B, 12, Rect::new(back.x, y, 22.0, 28.0), pal.secondary);
    ui.c.line_text(&t("연결"), medium(12), Rect::new(back.x + 22.0, y, 48.0, 28.0), pal.secondary, Align::Left);
    if resp.clicked { acts.push(Act::Back); }
    y += 36.0;
    // title, status, the switch
    let sw = !matches!(d.control, ConnectionsSwitch::Fixed(_));
    let tw = cw - if sw { 50.0 } else { 0.0 };
    ui.c.line_text(&d.title, semi(16), Rect::new(m, y, tw, 24.0), pal.text, Align::Left);
    ui.c.line_text(&t(&d.status), sans(12), Rect::new(m, y + 25.0, tw, 18.0), tone_color(pal, d.tone), Align::Left);
    if let Some(on) = control(ui, pal, "main-sw", &d.control, w - m, y + 22.0, busy) {
        match &d.target {
            ConnectionsRef::Gadget(g) => acts.push(Act::Gadget(g, on)),
            ConnectionsRef::Tool(id) => acts.push(Act::Tool(id.clone(), on)),
            ConnectionsRef::Connector(id) => acts.push(Act::Connector(id.clone(), on, version_of(id))),
        }
    }
    y += 52.0;
    y += para(ui, &d.summary, sans(12), m, y, cw, pal.text) + 10.0;
    if let Some(n) = &d.notice {
        let n = t(n);
        let (_, nh) = ui.c.measure(&n, sans(11), cw - 16.0);
        ui.c.round(Rect::new(m, y, cw, nh + 16.0), 8.0, pal.warning_soft);
        ui.c.text(&n, sans(11), Rect::new(m + 8.0, y + 8.0, cw - 16.0, nh + 2.0), pal.text, Align::Left);
        y += nh + 26.0;
    }

    // "이 가젯은 항상 허용"
    if let Some(a) = &d.always_allow {
        let note = t(a.note);
        let (_, nh) = ui.c.measure(&note, sans(11), cw - 60.0);
        let card = Rect::new(m, y, cw, 40.0 + nh);
        ui.c.round(card, 10.0, pal.surface);
        ui.c.line_text(&t("이 가젯은 항상 허용"), semi(13), Rect::new(m + 12.0, y + 9.0, cw - 70.0, 20.0), pal.text, Align::Left);
        ui.c.text(&note, sans(11), Rect::new(m + 12.0, y + 30.0, cw - 60.0, nh + 2.0), pal.secondary, Align::Left);
        if switch(ui, pal, "always", card.right() - 50.0, y + 9.0, a.on, true) { acts.push(if a.on { Act::AlwaysAllow(a.gadget, false) } else { Act::AlwaysAllowAsk(a.gadget) }); }
        y += card.h + 8.0;
        if matches!(confirm, Some(Confirm::AlwaysAllow(g)) if *g == a.gadget) {
            y = confirm_box(ui, pal, None, &a.confirm_text, "항상 허용", false, m, cw, y, acts, Act::AlwaysAllow(a.gadget, true));
        }
        y += 6.0;
    }

    // abilities
    if !d.abilities_title.is_empty() {
        ui.c.line_text(&t(d.abilities_title), semi(12), Rect::new(m, y, cw, 20.0), pal.secondary, Align::Left);
        y += 24.0;
        if d.abilities.is_empty() { y += para(ui, d.empty_abilities, sans(12), m, y, cw, pal.tertiary) + 8.0; }
        for (i, a) in d.abilities.iter().enumerate() {
            y += ui.in_scope(i as u64, |ui| {
                let tw = cw - if a.on.is_some() { 50.0 } else { 0.0 };
                let title = t(&a.title);
                let (_, th) = ui.c.measure(&title, sans(12), tw);
                let risk = a.risk_text.map(t);
                let rh = risk.as_ref().map(|r| ui.c.measure(r, sans(11), tw).1 + 2.0).unwrap_or(0.0);
                let note = a.note.as_deref().filter(|n| !n.is_empty()).map(|n| t(n).into_owned());
                let nh = note.as_ref().map(|r| ui.c.measure(r, sans(11), tw).1 + 2.0).unwrap_or(0.0);
                let h = (th + rh + nh + 14.0).max(34.0);
                ui.c.text(&title, sans(12), Rect::new(m, y + 7.0, tw, th + 1.0), if a.on == Some(false) { pal.tertiary } else { pal.text }, Align::Left);
                let mut ty = y + 7.0 + th;
                if let Some(r) = &risk { ui.c.text(r, sans(11), Rect::new(m, ty + 1.0, tw, rh), if a.risk == Some(crate::core::pet::gadget_tools::PetToolRisk::Destructive) { pal.danger } else { pal.warning }, Align::Left); ty += rh; }
                if let Some(n) = &note { ui.c.text(n, sans(11), Rect::new(m, ty + 1.0, tw, nh), pal.secondary, Align::Left); }
                if let Some(on) = a.on { if switch(ui, pal, "act", w - m - 38.0, y + (h - 22.0) / 2.0, on, true) { acts.push(Act::Action(a.id.clone(), !on)); } }
                ui.c.line(m, y + h, w - m, y + h, pal.separator, 1.0);
                h
            });
        }
        y += 10.0;
    }

    // facts
    for f in &d.facts {
        ui.c.line_text(&t(f.label), sans(11), Rect::new(m, y, 96.0, 20.0), pal.secondary, Align::Left);
        y += para(ui, &f.value, sans(12), m + 100.0, y, cw - 100.0, pal.text).max(20.0) + 4.0;
    }

    // scope
    if let Some(sc) = &d.scope {
        y += 6.0;
        ui.c.line_text(&t("권한 범위"), semi(12), Rect::new(m, y, cw, 20.0), pal.secondary, Align::Left);
        y += 24.0;
        let opts: Vec<String> = sc.levels.iter().map(|(_, title)| t(title).into_owned()).collect();
        let cur = sc.levels.iter().position(|(id, _)| *id == sc.selected);
        if let Some(i) = ui.segments("scope", &opts, cur, Rect::new(m, y, cw, 30.0), false) {
            let level = sc.levels[i].0;
            if level != sc.selected { if let ConnectionsRef::Connector(id) = &d.target { acts.push(Act::Scope(id.clone(), level.to_string(), version_of(id))); } }
        }
        y += 40.0;
    }

    // disconnect
    if d.can_disconnect {
        if let ConnectionsRef::Connector(id) = &d.target {
            y += 8.0;
            let asking = matches!(confirm, Some(Confirm::Disconnect(c)) if c == id);
            if asking {
                let title = format!("{} 연결을 끊을까요?", d.title);
                y = confirm_box(ui, pal, Some(&title), "나초가 이 서비스를 더 이상 쓸 수 없어요. 되돌릴 수 없어요. 다시 쓰려면 처음부터 다시 연결해야 해요.", "연결 해제", true, m, cw, y, acts, Act::Disconnect(id.clone()));
            } else {
                let label = t("연결 해제");
                let bw = ui.button_width(&label, true);
                if ui.button_rect("disc", &label, Rect::new(m, y, bw, 30.0), Btn::Secondary, true, !busy) { acts.push(Act::AskDisconnect(id.clone())); }
                y += 36.0;
            }
        }
    }
    y
}

#[allow(clippy::too_many_arguments)]
fn confirm_box(ui: &mut Ui, pal: &Pal, title: Option<&str>, text: &str, ok_label: &str, danger: bool, m: f64, cw: f64, y: f64, acts: &mut Vec<Act>, ok: Act) -> f64 {
    use crate::i18n::t;
    let text = t(text);
    let (_, th) = ui.c.measure(&text, sans(11), cw - 24.0);
    let head = if title.is_some() { 24.0 } else { 0.0 };
    let h = th + 62.0 + head;
    ui.c.round(Rect::new(m, y, cw, h), 10.0, pal.warning_soft);
    if let Some(tt) = title { ui.c.line_text(&t(tt), semi(13), Rect::new(m + 12.0, y + 8.0, cw - 24.0, 20.0), pal.text, Align::Left); }
    ui.c.text(&text, sans(11), Rect::new(m + 12.0, y + 10.0 + head, cw - 24.0, th + 2.0), pal.text, Align::Left);
    let by = y + th + 20.0 + head;
    let okl = t(ok_label);
    let okw = ui.button_width(&okl, true);
    if ui.button_rect("confirm-ok", &okl, Rect::new(m + 12.0, by, okw, 30.0), if danger { Btn::Attention } else { Btn::Primary }, true, true) { acts.push(ok); }
    let cl = t("취소");
    if ui.button_rect("confirm-cancel", &cl, Rect::new(m + 12.0 + okw + 8.0, by, ui.button_width(&cl, true), 30.0), Btn::Secondary, true, true) { acts.push(Act::Cancel); }
    y + h + 8.0
}

fn apply(a: Act, gadgets: &[GadgetSwitchState], disabled: &BTreeSet<String>) {
    let c = &mut app().pet.chat.conn;
    match a {
        Act::Open(r) => { c.detail = Some(r); c.confirm = None; }
        Act::Back => { c.detail = None; c.confirm = None; }
        Act::Cancel => c.confirm = None,
        Act::Refresh => load(),
        Act::Gadget(g, on) => {
            if let Some(s) = gadgets.iter().find(|s| s.gadget == g) {
                let m = s.module;
                later(move || { app().set_enabled(m, on); crate::pet_chat::invalidate(); });
            }
        }
        Act::AlwaysAllowAsk(g) => c.confirm = Some(Confirm::AlwaysAllow(g)),
        Act::AlwaysAllow(g, on) => { c.confirm = None; crate::pet_exec::trust().set_always_allowed(g, on); }
        Act::Action(id, on) => {
            let mut set = disabled.clone();
            if on { set.remove(&id); set.retain(|n| !crate::core::pet::commands::PetCommands::connector_names(&id).contains(&n.as_str())); } else { set.insert(id); }
            crate::pet_exec::trust().set_disabled(set);
        }
        Act::Tool(id, on) => set_tool(id, on),
        Act::Connector(id, on, v) => set_connection(id, on, v),
        Act::Scope(id, level, v) => set_scope(id, level, v),
        Act::AskDisconnect(id) => c.confirm = Some(Confirm::Disconnect(id)),
        Act::Disconnect(id) => { c.confirm = None; disconnect(id); }
    }
    crate::pet_chat::invalidate();
}
