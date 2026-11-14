//! The settings window: sidebar, the gadget gallery, each gadget's page, Combos and General.
use crate::app::{self, app, later, State};
use crate::core::catalog::{self, SNAP_GROUPS};
use crate::core::clock::{self, ClockHours};
use crate::core::combos::{self, Combo};
use crate::core::layout::Rect;
use crate::core::settings::*;
use crate::core::shortcut::{self, Shortcut};
use crate::gfx::*;
use crate::panel::{controls, status_color, status_text};
use crate::ui::*;
use crate::window::{UiWindow, WindowCtx};
use crate::{monitor, snap, trayorg};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const PRODUCT: &str = "NachoCombo for Windows";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page { Home, Combos, General, Gadget(ModuleId) }

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab { Overview, Settings, Shortcuts }

#[derive(Default)]
pub struct SettingsWindow {
    win: Option<isize>,
    pub page: Option<Page>,
    pub tab: Option<Tab>,
    pub combo_pick: Option<String>,
    pub editing: Option<SavedCombo>,
    pub capture_for: Option<String>,
    pub shortcut_issue: Option<(String, String)>,
    pub confirm_reset: bool,
    pub awake_draft: String,
}

impl SettingsWindow {
    pub fn is_open(&self) -> bool { self.win.is_some() }
    pub fn invalidate(&self) { if let Some(h) = self.win { unsafe { let _ = InvalidateRect(Some(HWND(h as *mut _)), None, false); } } }
}

fn page_key(p: Page) -> String { match p { Page::Home => "home".into(), Page::Combos => "combos".into(), Page::General => "general".into(), Page::Gadget(m) => m.key().into() } }
fn page_from(k: &str) -> Option<Page> { match k { "home" => Some(Page::Home), "combos" => Some(Page::Combos), "general" => Some(Page::General), k => ModuleId::from_key(k).map(Page::Gadget) } }

pub fn go(p: Page) {
    let a = app();
    let s = &mut a.settings_window;
    s.page = Some(p);
    s.capture_for = None;
    s.shortcut_issue = None;
    // A gadget that is on opens on Settings, one that is off on Overview.
    if let Page::Gadget(m) = p { s.tab = Some(if a.settings.is_on(m) { Tab::Settings } else { Tab::Overview }); }
    a.settings.last_page = Some(page_key(p));
    a.save();
    a.settings_window.invalidate();
}

pub fn open(m: Option<ModuleId>) {
    let a = app();
    if !a.settings.welcomed { a.settings.welcomed = true; a.save(); }
    let start = m.map(Page::Gadget).or_else(|| a.settings.last_page.as_deref().and_then(page_from)).unwrap_or(if a.settings.enabled.values().any(|v| *v) { Page::Home } else { Page::Combos });
    if let Some(h) = a.settings_window.win {
        go(start);
        unsafe { let h = HWND(h as *mut _); let _ = ShowWindow(h, SW_RESTORE); let _ = SetForegroundWindow(h); }
        return;
    }
    let (w, h) = a.settings.window_size.unwrap_or((980.0, 700.0));
    let win = UiWindow::create(PRODUCT, WS_OVERLAPPEDWINDOW, WINDOW_EX_STYLE(0), CW_USEDEFAULT, CW_USEDEFAULT, w.max(760.0), h.max(520.0), Box::new(draw));
    let hw = win.hwnd;
    win.on_close = Some(Box::new(move || {
        let a = app();
        let mut r = RECT::default();
        unsafe { let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hw, &mut r); }
        let s = crate::window::dpi_scale(hw);
        a.settings.window_size = Some((r.right as f64 / s, r.bottom as f64 / s));
        a.save();
        a.settings_window.win = None;
    }));
    win.set_msg(Box::new(|_, msg, wp, lp| {
        // While a key field listens, every key combination goes to it, including ones with Alt.
        if (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN) && app().settings_window.capture_for.is_some() { capture_key(wp.0 as u32); let _ = lp; return Some(LRESULT(0)); }
        if msg == WM_GETMINMAXINFO {
            let m = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
            m.ptMinTrackSize = POINT { x: 760, y: 520 };
        }
        None
    }));
    a.settings_window.win = Some(hw.0 as isize);
    go(start);
    win.show();
}

pub fn open_combos_new() {
    open(None);
    go(Page::Combos);
    let s = &app().settings;
    app().settings_window.editing = Some(new_draft(s, None));
}

fn new_draft(s: &AppSettings, from: Option<&Combo>) -> SavedCombo {
    let id = format!("custom-{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) & 0xffffff);
    match from {
        Some(c) => SavedCombo { id, name: combos::free_name(&format!("My {}", c.name), s), gadgets: c.gadgets.clone(), tuning: c.tuning.clone() },
        None => SavedCombo { id, name: combos::free_name("My Combo", s), gadgets: ModuleId::ALL.into_iter().filter(|m| s.is_on(*m)).collect(), tuning: Tuning::default() },
    }
}

/// The gadget's tile colours, from the Mac app's icons.
pub fn tile_colors(m: ModuleId) -> (Color, Color, Color) {
    let white = rgb(0xFFFFFF);
    match m {
        ModuleId::TrayOrganizer => (rgb(0xD6ECFF), rgb(0x7DB8F7), rgb(0x1B3A5C)),
        ModuleId::SystemMonitor => (rgb(0x4A4A52), rgb(0x1C1C21), rgb(0x7FCB72)),
        ModuleId::WindowSnap => (rgb(0x5EE0B0), rgb(0x119A74), white),
        ModuleId::FileShelf => (rgb(0xFFC860), rgb(0xF27A1A), white),
        ModuleId::CaptureFloat => (rgb(0xFF8FB1), rgb(0xD92A63), white),
        ModuleId::FocusDim => (rgb(0x8C86FF), rgb(0x4630C9), white),
        ModuleId::KeepAwake => (rgb(0xD9A47A), rgb(0x7A4626), white),
        ModuleId::Clipboard => (rgb(0x7FD3FF), rgb(0x1F7BE0), white),
        ModuleId::DeviceControl => (rgb(0x6FE7DD), rgb(0x0E8A93), white),
        ModuleId::DiskMap => (rgb(0x3F2E20), rgb(0x1E1610), NACHO),
        ModuleId::AppSwitch => (rgb(0xB9C4FF), rgb(0x3C4FC4), white),
        ModuleId::CalendarClock => (rgb(0xFF9F9A), rgb(0xD9382F), white),
    }
}

pub fn tile(c: &Canvas, m: ModuleId, x: f64, y: f64, s: f64) {
    let (a, b, ink) = tile_colors(m);
    let r = Rect::new(x, y, s, s);
    c.gradient_round(r, s * 0.24, a, b);
    gadget_icon(c, m, x + s * 0.2, y + s * 0.2, s * 0.6, ink);
}

// ---------- frame ----------

const SIDE: f64 = 236.0;

fn draw(ui: &mut Ui, ctx: &mut WindowCtx) {
    let th = ui.th;
    let a = app();
    let page = a.settings_window.page.unwrap_or(Page::Home);
    // Arrow keys move through the sidebar when nothing else takes them.
    if a.settings_window.capture_for.is_none() && a.settings_window.editing.is_none() {
        let list = sidebar_pages();
        for k in ui.st.keys.clone() {
            if (k.vk == 0x26 || k.vk == 0x28) && !k.ctrl {
                let i = list.iter().position(|p| *p == page).unwrap_or(0) as i32 + if k.vk == 0x28 { 1 } else { -1 };
                go(list[i.clamp(0, list.len() as i32 - 1) as usize]);
            }
        }
    }
    sidebar(ui, page);
    let view = Rect::new(SIDE, 0.0, ui.w - SIDE, ui.h);
    let key = page_key(page);
    ui.scroll(&key, view, |ui, y| {
        let x = SIDE + 40.0;
        let w = (ui.w - SIDE - 80.0).min(880.0);
        match page {
            Page::Home => home(ui, x, y + 28.0, w),
            Page::Combos => combos_page(ui, x, y + 28.0, w),
            Page::General => general(ui, ctx, x, y + 28.0, w),
            Page::Gadget(m) => gadget_page(ui, m, x, y + 28.0, w),
        }
    });
    let _ = th;
}

fn sidebar_pages() -> Vec<Page> {
    let mut v = vec![Page::Home, Page::Combos, Page::General];
    v.extend(ModuleId::ALL.into_iter().map(Page::Gadget));
    v
}

fn sidebar(ui: &mut Ui, page: Page) {
    let th = ui.th;
    let a = app();
    ui.c.fill(Rect::new(0.0, 0.0, SIDE, ui.h), th.paper2);
    ui.c.line(SIDE, 0.0, SIDE, ui.h, th.line, 1.0);
    let mut y = 16.0;
    ui.c.logo(Rect::new(14.0, y - 4.0, 36.0, 36.0));
    ui.c.line_text("NACHO COMBO", sign(17), Rect::new(54.0, y - 2.0, 170.0, 22.0), th.ink, Align::Left);
    ui.c.line_text("FOR WINDOWS", mono(10), Rect::new(54.0, y + 18.0, 170.0, 14.0), th.ink2, Align::Left);
    y += 52.0;
    let entry = |ui: &mut Ui, p: Page, label: &str, y: f64, dot: Option<Color>| {
        let r = Rect::new(10.0, y, SIDE - 20.0, 34.0);
        let id = ui.id(&format!("side-{}", page_key(p)));
        let resp = ui.hit(id, r, true);
        let sel = page == p;
        if sel { ui.c.round(r, 8.0, NACHO.alpha(0.4)); ui.c.stroke_round(r, 8.0, th.nacho_deep, 1.5); }
        else if resp.hover { ui.c.round(r, 8.0, th.ink.alpha(0.05)); }
        match p {
            Page::Gadget(m) => tile(ui.c, m, r.x + 8.0, r.y + 6.0, 22.0),
            _ => {
                let code = match p { Page::Home => 0xF0E2, Page::Combos => 0xE7BF, _ => 0xE713 };
                let t = Rect::new(r.x + 8.0, r.y + 6.0, 22.0, 22.0);
                ui.c.gradient_round(t, 5.0, rgb(0x3A2C1F), BOARD);
                ui.c.glyph(code, 12, t, NACHO);
            }
        }
        ui.c.line_text(label, if sel { semi(14) } else { medium(14) }, Rect::new(r.x + 40.0, r.y, r.w - 66.0, r.h), th.ink, Align::Left);
        if let Some(c) = dot { ui.status_dot(r.right() - 14.0, r.cy(), c); }
        if resp.clicked { later(move || go(p)); }
    };
    entry(ui, Page::Home, "All Gadgets", y, None);
    entry(ui, Page::Combos, "Combos", y + 36.0, None);
    entry(ui, Page::General, "General", y + 72.0, None);
    y += 124.0;
    ui.eyebrow("Gadgets", 24.0, y, 180.0, th.ink2);
    y += 24.0;
    for m in ModuleId::ALL {
        let st = a.state(m);
        let dot = match st { State::Running => Some(th.guac), State::Failed(_) => Some(th.salsa), State::Off => None };
        entry(ui, Page::Gadget(m), m.name(), y, dot);
        y += 36.0;
    }
}

fn heading(ui: &Ui, s: &str, x: f64, y: f64, w: f64) -> f64 {
    ui.c.line_text(s, bold(16), Rect::new(x, y, w, 24.0), ui.th.ink, Align::Left);
    y + 32.0
}

/// A card that grows with its rows; `body` draws from the y it gets and returns where it ended.
fn card(ui: &mut Ui, key: &str, x: f64, y: f64, w: f64, body: impl FnOnce(&mut Ui, f64) -> f64) -> f64 {
    let id = ui.id(key);
    let h = ui.st.content.get(&id).copied().unwrap_or(60.0);
    ui.card(Rect::new(x, y, w, h));
    let end = body(ui, y + 6.0) + 6.0;
    let nh = end - y;
    if (nh - h).abs() > 0.5 { ui.st.content.insert(id, nh); ui.st.again = true; }
    end
}

fn divider(ui: &Ui, x: f64, y: f64, w: f64) { ui.c.line(x + 18.0, y, x + w - 18.0, y, ui.th.line, 1.0); }

// ---------- home ----------

fn home(ui: &mut Ui, x: f64, y: f64, w: f64) -> f64 {
    let th = ui.th;
    let a = app();
    let lid = Rect::new(x, y, w, 120.0);
    ui.c.round(lid, 14.0, NACHO);
    ui.c.stroke_round(lid, 14.0, ON_NACHO, 1.5);
    ui.c.line_text("WHAT'S IN YOUR BOX", mono(11), Rect::new(x + 24.0, y + 18.0, 300.0, 16.0), ON_NACHO.alpha(0.75), Align::Left);
    let combo = combos::current(&a.settings);
    let title = combo.as_ref().map_or("A box of your own".to_string(), |c| format!("Your box: {}", c.name));
    ui.c.line_text(&title.to_uppercase(), sign(30), Rect::new(x + 24.0, y + 36.0, w - 260.0, 42.0), ON_NACHO, Align::Left);
    let running = ModuleId::ALL.iter().filter(|m| a.state(**m) == State::Running).count();
    let failed: Vec<ModuleId> = ModuleId::ALL.into_iter().filter(|m| matches!(a.state(*m), State::Failed(_))).collect();
    let mut line = format!("{running} running");
    if let Some(f) = failed.first() { line += &format!("  ·  {} needs you ›", f.name()); }
    ui.c.line_text(&line, medium(14), Rect::new(x + 24.0, y + 80.0, w - 260.0, 22.0), ON_NACHO, Align::Left);
    let bw = ui.button_width("Order a Combo", false);
    if ui.button("tocombos", "Order a Combo", lid.right() - bw - 24.0, y + 45.0, Btn::Board, false) { later(|| go(Page::Combos)); }
    let mut y = y + 144.0;
    let cols = if w > 700.0 { 3 } else { 2 };
    let gap = 14.0;
    let tw = (w - gap * (cols as f64 - 1.0)) / cols as f64;
    for (i, m) in ModuleId::ALL.into_iter().enumerate() {
        let r = Rect::new(x + (i % cols) as f64 * (tw + gap), y + (i / cols) as f64 * 132.0, tw, 118.0);
        let id = ui.id(&format!("tile{}", m.key()));
        let resp = ui.hit(id, r, true);
        ui.c.round(r, 12.0, if resp.hover { th.paper } else { th.paper2 });
        ui.c.stroke_round(r, 12.0, if resp.hover || resp.focused { th.nacho_deep } else { th.line }, if resp.focused { 2.0 } else { 1.0 });
        tile(ui.c, m, r.x + 16.0, r.y + 16.0, 40.0);
        let on = a.settings.is_on(m);
        let sw_x = r.right() - 56.0;
        if ui.switch(&format!("tsw{}", m.key()), sw_x, r.y + 20.0, on) { later(move || app().set_enabled(m, !on)); }
        ui.c.line_text(m.name(), semi(15), Rect::new(r.x + 16.0, r.y + 64.0, r.w - 32.0, 22.0), th.ink, Align::Left);
        let st = a.state(m);
        let slot = tile_status(m, &st);
        ui.status_dot(r.x + 21.0, r.y + 98.0, status_color(&th, &st, on));
        ui.c.line_text(&slot, sans(12), Rect::new(r.x + 32.0, r.y + 88.0, r.w - 48.0, 20.0), if matches!(st, State::Failed(_)) { th.salsa } else { th.ink2 }, Align::Left);
        if resp.clicked { later(move || go(Page::Gadget(m))); }
    }
    y += ModuleId::ALL.len().div_ceil(cols) as f64 * 132.0;
    y + 20.0
}

/// One status slot per tile: what matters most right now.
fn tile_status(m: ModuleId, st: &State) -> String {
    let a = app();
    match st {
        State::Off => "Off".into(),
        State::Failed(e) => e.clone(),
        State::Running => match m {
            ModuleId::SystemMonitor => { let v = &a.monitor.last; format!("CPU {:.0}% · Memory {:.0}%", v.cpu_user + v.cpu_system, v.mem_load) }
            ModuleId::FileShelf => format!("{} · {} item{}", shortcut_of("shelf.toggle"), a.shelf.count(), if a.shelf.count() == 1 { "" } else { "s" }),
            ModuleId::CaptureFloat => format!("{} · {} float{}", shortcut_of("capture.area"), a.capture.float_count(), if a.capture.float_count() == 1 { "" } else { "s" }),
            ModuleId::WindowSnap => format!("{} · Left half", shortcut_of("snap.left")),
            ModuleId::FocusDim => if a.dim.paused { "Paused".into() } else { format!("{:.0}% · {}", a.settings.dim_intensity * 100.0, shortcut_of("dim.toggle")) },
            ModuleId::TrayOrganizer => if a.settings.tray_collapsed { "Icons folded away".into() } else { "Icons shown".into() },
            ModuleId::KeepAwake => if a.awake.active() { a.awake.minutes_left().map_or("Awake".into(), |m| format!("Awake · {m} min left")) } else { "Sleeps as usual".into() },
            ModuleId::Clipboard => format!("{} · {} item{}", shortcut_of("clipboard.show"), a.clipboard.items.len(), if a.clipboard.items.len() == 1 { "" } else { "s" }),
            ModuleId::AppSwitch => format!("{} · Switch windows", shortcut_of("appswitch.next")),
            ModuleId::DiskMap => if crate::diskmap::is_open() { "Map open".into() } else { "Ready".into() },
            ModuleId::CalendarClock => a.clock.status(),
            ModuleId::DeviceControl => {
                let out = a.devices.outputs.iter().find(|d| d.default).map_or("No output".to_string(), |d| d.name.clone());
                if a.devices.mic_muted { format!("Mic muted · {out}") } else { out }
            }
        },
    }
}

fn shortcut_of(id: &str) -> String {
    let a = app();
    catalog::actions().into_iter().find(|x| x.id == id).and_then(|x| a.settings.shortcut_for(&x)).map_or("No shortcut".into(), |s| s.label())
}

// ---------- gadget page ----------

fn gadget_page(ui: &mut Ui, m: ModuleId, x: f64, y: f64, w: f64) -> f64 {
    let th = ui.th;
    let a = app();
    let info = catalog::info(m);
    tile(ui.c, m, x, y, 72.0);
    let tx = x + 92.0;
    ui.eyebrow(if matches!(m, ModuleId::WindowSnap | ModuleId::CaptureFloat | ModuleId::SystemMonitor) { "Main · Every day" } else { "Gadget" }, tx, y + 2.0, 300.0, th.guac);
    ui.c.line_text(m.name(), bold(24), Rect::new(tx, y + 18.0, w - 200.0, 32.0), th.ink, Align::Left);
    let th_ = ui.label(info.tagline, sans(14), tx, y + 54.0, w - 200.0, th.ink2);
    let st = a.state(m);
    let on = a.settings.is_on(m);
    ui.status_dot(tx + 5.0, y + 66.0 + th_, status_color(&th, &st, on));
    ui.c.line_text(&status_text(&st), medium(13), Rect::new(tx + 16.0, y + 56.0 + th_, w - 220.0, 20.0), th.ink2, Align::Left);
    if ui.switch(&format!("hsw{}", m.key()), x + w - 44.0, y + 6.0, on) { later(move || app().set_enabled(m, !on)); }
    let mut y = y + 96.0 + th_;

    // What to do: errors, clashes and notes, before everything else.
    let mut notes: Vec<(Color, String)> = vec![];
    if let State::Failed(e) = &st { notes.push((th.salsa, e.clone())); }
    for id in &a.hotkey_clashes {
        if let Some(act) = catalog::actions().into_iter().find(|x| &x.id == id && x.module == m) {
            notes.push((th.nacho_ink, format!("Another app already uses the shortcut for {}. Pick another one under Shortcuts.", act.title)));
        }
    }
    match m {
        ModuleId::WindowSnap => {
            if let Some(n) = &a.snap.notice { notes.push((th.nacho_ink, n.clone())); }
        }
        ModuleId::CaptureFloat => if let Some(n) = &a.capture.notice { notes.push((th.nacho_ink, n.clone())); },
        ModuleId::TrayOrganizer => if let Some(n) = &a.organizer.notice { notes.push((th.nacho_ink, n.clone())); },
        ModuleId::SystemMonitor => for msg in a.monitor.alerts.messages() { notes.push((th.salsa, msg)); },
        _ => {}
    }
    for (c, n) in notes {
        let h = ui.c.measure(&n, medium(13), w - 56.0).1 + 24.0;
        let r = Rect::new(x, y, w, h);
        ui.c.round(r, 10.0, c.alpha(0.1));
        ui.c.stroke_round(r, 10.0, c.alpha(0.5), 1.0);
        ui.c.glyph(0xE7BA, 14, Rect::new(x + 12.0, y + 12.0, 18.0, 18.0), c);
        ui.label(&n, medium(13), x + 38.0, y + 12.0, w - 56.0, c);
        y += h + 10.0;
    }
    y += 10.0;

    let tab = a.settings_window.tab.unwrap_or(Tab::Overview);
    let tabs: Vec<String> = ["Overview", "Settings", "Shortcuts"].iter().map(|s| s.to_string()).collect();
    let cur = match tab { Tab::Overview => 0, Tab::Settings => 1, Tab::Shortcuts => 2 };
    if let Some(i) = ui.segments("tabs", &tabs, Some(cur), Rect::new(x, y, 330.0, 34.0), false) {
        app().settings_window.tab = Some([Tab::Overview, Tab::Settings, Tab::Shortcuts][i]);
        app().settings_window.capture_for = None;
        ui.st.again = true;
    }
    y += 54.0;
    match tab {
        Tab::Overview => overview(ui, m, x, y, w),
        Tab::Settings => settings_tab(ui, m, x, y, w),
        Tab::Shortcuts => shortcuts_tab(ui, m, x, y, w),
    }
}

fn overview(ui: &mut Ui, m: ModuleId, x: f64, y: f64, w: f64) -> f64 {
    let th = ui.th;
    let info = catalog::info(m);
    let mut y = heading(ui, "Controls", x, y, w);
    y = card(ui, &format!("ctl{}", m.key()), x, y, w, |ui, y| controls(ui, m, x + 18.0, y + 12.0, w - 36.0) + 6.0) + 24.0;
    y = heading(ui, "Features", x, y, w);
    let col = (w - 16.0) / 2.0;
    let mut heights = [0.0f64; 2];
    let base = y;
    for (i, f) in info.features.iter().enumerate() {
        let c = i % 2;
        let fx = x + c as f64 * (col + 16.0);
        let fy = base + heights[c];
        ui.c.line_text(f.title, semi(14), Rect::new(fx, fy, col, 22.0), th.ink, Align::Left);
        let h = ui.label(f.detail, sans(13), fx, fy + 22.0, col, th.ink2);
        heights[c] += h + 36.0;
    }
    y = base + heights[0].max(heights[1]) + 8.0;
    if !info.tips.is_empty() {
        y = heading(ui, "Keys and Gestures", x, y, w);
        y = card(ui, &format!("tips{}", m.key()), x, y, w, |ui, y| {
            let mut y = y + 8.0;
            for t in info.tips {
                let mut cx = x + 18.0;
                if !t.keys.is_empty() { cx += ui.keycaps(&t.keys.split('+').map(String::from).collect::<Vec<_>>(), cx, y + 3.0) + 8.0; }
                if !t.gesture.is_empty() {
                    let gw = ui.c.measure(t.gesture, mono(11), 300.0).0 + 14.0;
                    let r = Rect::new(cx, y + 3.0, gw, 22.0);
                    ui.c.round(r, 5.0, th.paper);
                    ui.c.line_text(t.gesture, mono(11), r, th.ink, Align::Center);
                    cx += gw + 10.0;
                }
                ui.c.line_text(t.title, sans(13), Rect::new(cx.max(x + 220.0), y, w - 240.0, 28.0), th.ink, Align::Left);
                y += 32.0;
            }
            y + 4.0
        }) + 24.0;
    }
    y = heading(ui, "Good to Know", x, y, w);
    let mut lines: Vec<&str> = vec![info.note];
    lines.extend(info.limits.iter().copied());
    for l in lines {
        ui.c.circle(x + 5.0, y + 10.0, 2.5, th.ink2);
        y += ui.label(l, sans(13), x + 16.0, y, w - 16.0, th.ink2) + 10.0;
    }
    y + 30.0
}

fn option_row(ui: &mut Ui, label: &str, x: f64, y: f64, w: f64) -> f64 {
    ui.c.line_text(label, sans(14), Rect::new(x + 18.0, y, w * 0.5, 44.0), ui.th.ink, Align::Left);
    y
}

fn choice<T: Copy + PartialEq>(ui: &mut Ui, key: &str, label: &str, opts: &[(T, &str)], cur: T, x: f64, y: f64, w: f64) -> (Option<T>, f64) {
    option_row(ui, label, x, y, w);
    let names: Vec<String> = opts.iter().map(|o| o.1.to_string()).collect();
    let sw = (names.len() as f64 * 88.0).min(w * 0.55);
    let idx = opts.iter().position(|o| o.0 == cur);
    let r = ui.segments(key, &names, idx, Rect::new(x + w - 18.0 - sw, y + 7.0, sw, 30.0), false).map(|i| opts[i].0);
    (r, y + 44.0)
}

fn check(ui: &mut Ui, key: &str, label: &str, on: bool, x: f64, y: f64, w: f64) -> (bool, f64) {
    let hit = ui.checkbox(key, label, x + 18.0, y + 10.0, w - 36.0, on);
    (hit, y + 44.0)
}

fn settings_tab(ui: &mut Ui, m: ModuleId, x: f64, y: f64, w: f64) -> f64 {
    let th = ui.th;
    let a = app();
    let mut o = a.settings.options.clone();
    let before = o.clone();
    let mut y = heading(ui, "Options", x, y, w);
    y = card(ui, &format!("opt{}", m.key()), x, y, w, |ui, y| {
        let mut y = y;
        let mut sep = |ui: &Ui, y: f64| divider(ui, x, y, w);
        match m {
            ModuleId::TrayOrganizer => {
                let opts: Vec<(i32, String)> = Options::COLLAPSE_CHOICES.iter().map(|s| (*s, if *s == 0 { "Never".to_string() } else { format!("{s} s") })).collect();
                let refs: Vec<(i32, &str)> = opts.iter().map(|o| (o.0, o.1.as_str())).collect();
                let (r, ny) = choice(ui, "collapse", "Fold Away Again After", &refs, o.auto_collapse_seconds, x, y, w);
                if let Some(v) = r { o.auto_collapse_seconds = v; }
                y = ny;
                sep(ui, y);
                y += 12.0;
                ui.eyebrow("Always in view", x + 18.0, y, w, th.ink2);
                y += 24.0;
                if a.organizer.entries.is_empty() { trayorg::refresh_list(); }
                let entries = a.organizer.entries.clone();
                if entries.is_empty() { y += ui.label("Windows lists no tray icons yet.", sans(13), x + 18.0, y, w - 36.0, th.ink2) + 8.0; }
                for e in entries {
                    let keep = a.settings.tray_keep.iter().any(|k| k.eq_ignore_ascii_case(&e.path));
                    if ui.checkbox(&format!("keep{}", e.key), &e.name, x + 18.0, y, w - 36.0, keep) { let p = e.path.clone(); later(move || trayorg::set_keep(&p, !keep)); }
                    y += 30.0;
                }
                y += 6.0;
            }
            ModuleId::SystemMonitor => {
                ui.eyebrow("Tray items", x + 18.0, y + 12.0, w, th.ink2);
                y += 36.0;
                let mut items = a.settings.monitor_items.clone();
                let mut changed = false;
                let cw = (w - 36.0) / 3.0;
                for (i, it) in MonitorItem::ALL.into_iter().enumerate() {
                    let on = items.contains(&it);
                    if ui.checkbox(&format!("item{}", it.name()), it.name(), x + 18.0 + (i % 3) as f64 * cw, y + (i / 3) as f64 * 30.0, cw - 8.0, on) {
                        if on { items.retain(|x| *x != it); } else { items.push(it); }
                        changed = true;
                    }
                }
                if changed { let sorted: Vec<_> = MonitorItem::ALL.into_iter().filter(|i| items.contains(i)).collect(); later(move || { app().update(|s| s.monitor_items = sorted); monitor::sync_tray(); }); }
                y += 68.0;
                sep(ui, y);
                let opts: Vec<(i32, String)> = Options::CPU_CHOICES.iter().map(|s| (*s, if *s == 0 { "Off".to_string() } else { format!("{s}%") })).collect();
                let refs: Vec<(i32, &str)> = opts.iter().map(|o| (o.0, o.1.as_str())).collect();
                let (r, ny) = choice(ui, "cpualert", "CPU Alert At", &refs, o.cpu_alert_percent, x, y, w);
                if let Some(v) = r { o.cpu_alert_percent = v; }
                y = ny;
                sep(ui, y);
                let (h, ny) = check(ui, "notify", "Show a notification when an alert starts", o.alert_notifications, x, y, w);
                if h { o.alert_notifications = !o.alert_notifications; }
                y = ny;
                sep(ui, y);
                let (h, ny) = check(ui, "ipauto", "Look up the public IP by itself (asks cloudflare.com)", o.public_ip_auto, x, y, w);
                if h { o.public_ip_auto = !o.public_ip_auto; }
                y = ny;
            }
            ModuleId::WindowSnap => {
                let opts: Vec<(i32, String)> = Options::GAP_CHOICES.iter().map(|s| (*s, if *s == 0 { "None".to_string() } else { format!("{s}") })).collect();
                let refs: Vec<(i32, &str)> = opts.iter().map(|o| (o.0, o.1.as_str())).collect();
                let (r, ny) = choice(ui, "gap", "Gap Between Windows (px)", &refs, o.snap_gap, x, y, w);
                if let Some(v) = r { o.snap_gap = v; }
                y = ny;
                sep(ui, y);
                let (r, ny) = choice(ui, "repeat", "Repeating Left / Right", &[(SnapRepeat::CycleSizes, "Cycle ½ → ⅔ → ⅓"), (SnapRepeat::NextDisplay, "Next Display"), (SnapRepeat::Nothing, "Nothing")], o.snap_repeat, x, y, w);
                if let Some(v) = r { o.snap_repeat = v; }
                y = ny;
                sep(ui, y);
                let (h, ny) = check(ui, "drag", "Snap windows dragged to a screen edge or corner", o.snap_drag_to_edges, x, y, w);
                if h { o.snap_drag_to_edges = !o.snap_drag_to_edges; }
                y = ny;
                if o.snap_drag_to_edges && snap::edge_drags_left_to_windows() {
                    y += ui.label("Windows' own Snap is handling edge drags now. Turn off “Snap windows” in Settings ▸ System ▸ Multitasking to use these zones.", sans(13), x + 44.0, y - 6.0, w - 62.0, th.nacho_ink);
                    if ui.button("multitask", "Open Multitasking Settings", x + 44.0, y + 2.0, Btn::Secondary, true) { app::open_url("ms-settings:multitasking"); }
                    y += 40.0;
                }
                sep(ui, y);
                let (h, ny) = check(ui, "restore", "Restore the size when a snapped window is dragged away", o.snap_restore_on_drag, x, y, w);
                if h { o.snap_restore_on_drag = !o.snap_restore_on_drag; }
                y = ny;
                sep(ui, y);
                let (h, ny) = check(ui, "motion", "Ctrl+Alt-drag to move, Ctrl+Alt-right-drag to resize", o.window_motion, x, y, w);
                if h { o.window_motion = !o.window_motion; }
                y = ny;
                sep(ui, y);
                let (h, ny) = check(ui, "layouts", "Put windows back when the same displays connect again", o.snap_restore_layouts, x, y, w);
                if h { o.snap_restore_layouts = !o.snap_restore_layouts; }
                y = ny;
            }
            ModuleId::FileShelf => {
                let (r, ny) = choice(ui, "trigger", "Opens By Itself", &[(ShelfTrigger::Shake, "Shake While Dragging"), (ShelfTrigger::AnyDrag, "Any Drag"), (ShelfTrigger::ShortcutOnly, "Shortcut Only")], o.shelf_trigger, x, y, w);
                if let Some(v) = r { o.shelf_trigger = v; }
                y = ny;
                sep(ui, y);
                let (h, ny) = check(ui, "peek", "Space in File Explorer previews the selected file", o.explorer_preview, x, y, w);
                if h { o.explorer_preview = !o.explorer_preview; }
                y = ny;
            }
            ModuleId::CaptureFloat => {
                let (h, ny) = check(ui, "rounded", "Rounded corners on floats", o.capture_rounded_corners, x, y, w);
                if h { o.capture_rounded_corners = !o.capture_rounded_corners; }
                y = ny;
            }
            ModuleId::KeepAwake => {
                let (h, ny) = check(ui, "display", "Keep the display on too", o.awake_display, x, y, w);
                if h { o.awake_display = !o.awake_display; }
                y = ny;
                sep(ui, y);
                let opts: Vec<(i32, String)> = Options::GUARD_CHOICES.iter().map(|s| (*s, if *s == 0 { "Never".to_string() } else { format!("Below {s}%") })).collect();
                let refs: Vec<(i32, &str)> = opts.iter().map(|o| (o.0, o.1.as_str())).collect();
                let (r, ny) = choice(ui, "guard", "Sleep Again on Battery", &refs, o.awake_battery_guard, x, y, w);
                if let Some(v) = r { o.awake_battery_guard = v; }
                y = ny;
                sep(ui, y);
                ui.c.line_text("Stay awake while these programs run", sans(14), Rect::new(x + 18.0, y, w - 36.0, 40.0), th.ink, Align::Left);
                y += 40.0;
                for (i, name) in o.awake_apps.clone().iter().enumerate() {
                    ui.c.line_text(name, medium(13), Rect::new(x + 30.0, y, w - 160.0, 30.0), th.ink, Align::Left);
                    if ui.button(&format!("rm{i}"), "Remove", x + w - 100.0, y + 3.0, Btn::Secondary, true) { o.awake_apps.remove(i); }
                    y += 32.0;
                }
                let mut draft = a.settings_window.awake_draft.clone();
                ui.text_field("awakeapp", Rect::new(x + 30.0, y, 240.0, 30.0), &mut draft, 64);
                a.settings_window.awake_draft = draft.clone();
                let clean = draft.trim().to_string();
                if ui.button_rect("addapp", "Add Program", Rect::new(x + 280.0, y, 120.0, 30.0), Btn::Secondary, true, !clean.is_empty()) {
                    let name = if clean.to_lowercase().ends_with(".exe") { clean } else { format!("{clean}.exe") };
                    if !o.awake_apps.iter().any(|x| x.eq_ignore_ascii_case(&name)) { o.awake_apps.push(name); }
                    a.settings_window.awake_draft.clear();
                }
                y += 34.0;
                y += ui.label("The program's file name, as Task Manager ▸ Details shows it (for example zoom.exe).", sans(12), x + 30.0, y, w - 60.0, th.ink2) + 10.0;
            }
            ModuleId::Clipboard => {
                let opts: Vec<(i32, String)> = Options::CLIP_CHOICES.iter().map(|s| (*s, s.to_string())).collect();
                let refs: Vec<(i32, &str)> = opts.iter().map(|o| (o.0, o.1.as_str())).collect();
                let (r, ny) = choice(ui, "clipmax", "Copies to Keep", &refs, o.clip_max, x, y, w);
                if let Some(v) = r { o.clip_max = v; }
                y = ny;
                sep(ui, y);
                let pins = a.clipboard.items.iter().filter(|c| c.pinned).count();
                ui.c.line_text(&format!("{} in the history, {pins} pinned", a.clipboard.items.len()), sans(14), Rect::new(x + 18.0, y, w * 0.6, 46.0), th.ink, Align::Left);
                let bw = ui.button_width("Clear History", true);
                if ui.button("clearclips", "Clear History", x + w - 18.0 - bw, y + 11.0, Btn::Secondary, true) { a.clipboard.items.retain(|c| c.pinned); }
                y += 46.0;
            }
            ModuleId::AppSwitch => {
                y += ui.label("App Switch has no options of its own: change its shortcuts on the Shortcuts tab.", sans(13), x + 18.0, y + 12.0, w - 36.0, th.ink2) + 24.0;
            }
            ModuleId::CalendarClock => {
                let (h, ny) = check(ui, "cdate", "Show the date", o.clock_date, x, y, w);
                if h { o.clock_date = !o.clock_date; }
                y = ny;
                sep(ui, y);
                let (h, ny) = check(ui, "cweekday", "Show the weekday", o.clock_weekday, x, y, w);
                if h { o.clock_weekday = !o.clock_weekday; }
                y = ny;
                sep(ui, y);
                let (h, ny) = check(ui, "cseconds", "Show seconds", o.clock_seconds, x, y, w);
                if h { o.clock_seconds = !o.clock_seconds; }
                y = ny;
                sep(ui, y);
                let (r, ny) = choice(ui, "chours", "Hours", &[(ClockHours::System, "Like Windows"), (ClockHours::Twelve, "12-Hour"), (ClockHours::TwentyFour, "24-Hour")], o.clock_hours, x, y, w);
                if let Some(v) = r { o.clock_hours = v; }
                y = ny;
                sep(ui, y);
                ui.c.line_text("World clocks", sans(14), Rect::new(x + 18.0, y, w - 36.0, 40.0), th.ink, Align::Left);
                y += 40.0;
                let now = crate::clock::utc_now();
                for (i, id) in o.world_clocks.clone().iter().enumerate() {
                    ui.c.line_text(&clock::city(id), medium(13), Rect::new(x + 30.0, y, 200.0, 30.0), th.ink, Align::Left);
                    if let Some(off) = clock::zone_offset(id, now) { ui.c.line_text(&clock::offset_text(off - crate::clock::local_offset()), sans(12), Rect::new(x + 230.0, y, 100.0, 30.0), th.ink2, Align::Left); }
                    if ui.button(&format!("rmc{i}"), "Remove", x + w - 100.0, y + 3.0, Btn::Secondary, true) { o.world_clocks.remove(i); }
                    y += 32.0;
                }
                if o.world_clocks.len() < clock::MAX_WORLD_CLOCKS {
                    let mut draft = a.clock.city_query.clone();
                    ui.text_field("clockcity", Rect::new(x + 30.0, y, 240.0, 30.0), &mut draft, 40);
                    a.clock.city_query = draft.clone();
                    ui.c.line_text("Type a city, such as Tokyo", sans(12), Rect::new(x + 280.0, y, w - 300.0, 30.0), th.ink2, Align::Left);
                    y += 36.0;
                    for id in clock::search_cities(&draft, &o.world_clocks, 5) {
                        ui.c.line_text(&format!("{}  ·  {}", clock::city(id), id.split('/').next().unwrap_or("")), sans(13), Rect::new(x + 30.0, y, w - 160.0, 30.0), th.ink, Align::Left);
                        if ui.button(&format!("addc{id}"), "Add", x + w - 70.0, y + 3.0, Btn::Secondary, true) { o.world_clocks.push(id.to_string()); a.clock.city_query.clear(); }
                        y += 32.0;
                    }
                } else {
                    y += ui.label("That is the most the calendar shows. Remove one to add another.", sans(12), x + 30.0, y, w - 60.0, th.ink2) + 6.0;
                }
                y += 8.0;
            }
            ModuleId::DiskMap => {
                y += ui.label("Disk Map has no options: it scans when you open it.", sans(13), x + 18.0, y + 12.0, w - 36.0, th.ink2) + 24.0;
            }
            ModuleId::DeviceControl => {
                let outs = a.devices.outputs.clone();
                let ins = a.devices.inputs.clone();
                ui.eyebrow("Prefer these when they connect", x + 18.0, y + 12.0, w, th.ink2);
                y += 34.0;
                let mut pick = |ui: &mut Ui, key: &str, label: &str, list: &[crate::devices::Device], cur: &Option<String>, y: &mut f64| -> Option<Option<String>> {
                    ui.c.line_text(label, sans(14), Rect::new(x + 18.0, *y, 120.0, 34.0), th.ink, Align::Left);
                    let mut names: Vec<String> = vec!["None".into()];
                    names.extend(list.iter().map(|d| d.name.clone()));
                    let idx = cur.as_ref().and_then(|c| list.iter().position(|d| &d.id == c).map(|i| i + 1)).unwrap_or(0);
                    let r = ui.segments(key, &names, Some(idx), Rect::new(x + 140.0, *y + 2.0, w - 158.0, 30.0), false);
                    *y += 42.0;
                    r.map(|i| if i == 0 { None } else { Some(list[i - 1].id.clone()) })
                };
                if let Some(v) = pick(ui, "prefout", "Output", &outs, &o.preferred_output, &mut y) { o.preferred_output = v; }
                if let Some(v) = pick(ui, "prefin", "Microphone", &ins, &o.preferred_input, &mut y) { o.preferred_input = v; }
                sep(ui, y);
                let (h, ny) = check(ui, "themesched", "Switch light and dark mode on a schedule", o.theme_schedule, x, y, w);
                if h { o.theme_schedule = !o.theme_schedule; later(crate::devices::theme_tick); }
                y = ny;
                if o.theme_schedule {
                    let hours: Vec<String> = (0..24).step_by(1).map(|h| format!("{h:02}")).collect();
                    for (key, label, val) in [("lighth", "Light from", &mut o.light_minute), ("darkh", "Dark from", &mut o.dark_minute)] {
                        ui.c.line_text(label, sans(13), Rect::new(x + 44.0, y, 100.0, 32.0), th.ink, Align::Left);
                        let cur = (*val / 60) as usize;
                        if let Some(i) = ui.segments(key, &hours, Some(cur), Rect::new(x + 140.0, y + 2.0, w - 158.0, 28.0), false) { *val = i as i32 * 60; later(crate::devices::theme_tick); }
                        y += 38.0;
                    }
                    y += 4.0;
                }
            }
            ModuleId::FocusDim => {
                option_row(ui, &format!("Strength  {:.0}%", a.settings.dim_intensity * 100.0), x, y, w);
                if let Some(v) = ui.slider("str", Rect::new(x + w * 0.45, y + 10.0, w * 0.55 - 36.0, 24.0), a.settings.dim_intensity, 0.1, 0.9) {
                    a.settings.dim_intensity = (v * 20.0).round() / 20.0;
                    a.save();
                    crate::dim::options_changed();
                }
                y += 44.0;
                sep(ui, y);
                let (r, ny) = choice(ui, "scope", "Keep Bright", &[(DimScope::FrontWindow, "Front Window"), (DimScope::FrontApp, "Whole App")], o.dim_scope, x, y, w);
                if let Some(v) = r { o.dim_scope = v; }
                y = ny;
                sep(ui, y);
                let (h, ny) = check(ui, "others", "Dim the other displays too", o.dim_other_displays, x, y, w);
                if h { o.dim_other_displays = !o.dim_other_displays; }
                y = ny;
                sep(ui, y);
                let (r, ny) = choice(ui, "fade", "Fade", &[(0.0, "Off"), (0.25, "0.25 s"), (0.5, "0.5 s"), (1.0, "1 s")], o.dim_fade_seconds, x, y, w);
                if let Some(v) = r { o.dim_fade_seconds = v; }
                y = ny;
            }
        }
        y
    }) + 24.0;
    if o != before { a.settings.options = o; a.save(); later(|| app().apply_options()); }

    y = heading(ui, "Tray Access", x, y, w);
    y = card(ui, &format!("access{}", m.key()), x, y, w, |ui, y| {
        let pinned = a.settings.pinned_tools.contains(&m);
        ui.c.line_text("Pin to the Taskbar Corner", sans(14), Rect::new(x + 18.0, y + 6.0, w - 120.0, 22.0), th.ink, Align::Left);
        ui.c.line_text(&format!("A tray icon of its own opens {}'s controls directly.", m.name()), sans(12), Rect::new(x + 18.0, y + 28.0, w - 120.0, 18.0), th.ink2, Align::Left);
        if ui.switch(&format!("pin{}", m.key()), x + w - 62.0, y + 14.0, pinned) {
            later(move || { app().update(|s| { s.pinned_tools.retain(|p| *p != m); if !pinned { s.pinned_tools.push(m); } }); crate::tray::sync_pins(); });
        }
        let y = y + 56.0;
        divider(ui, x, y, w);
        ui.c.line_text("Shortcut to Open Its Controls", sans(14), Rect::new(x + 18.0, y, w * 0.5, 50.0), th.ink, Align::Left);
        key_field(ui, &format!("open.{}", m.key()), a.settings.tool_shortcuts.get(&m).copied(), None, x + w - 238.0, y + 10.0);
        y + 56.0
    }) + 24.0;
    y
}

// ---------- shortcuts ----------

fn shortcuts_tab(ui: &mut Ui, m: ModuleId, x: f64, y: f64, w: f64) -> f64 {
    let th = ui.th;
    let a = app();
    let acts: Vec<catalog::Action> = catalog::actions().into_iter().filter(|x| x.module == m).collect();
    let set = acts.iter().filter(|x| a.settings.shortcut_for(x).is_some()).count();
    let clashes = acts.iter().filter(|x| a.hotkey_clashes.contains(&x.id)).count();
    let mut summary = format!("{set} of {} set", acts.len());
    if clashes > 0 { summary += &format!(" · {clashes} held by another app"); }
    ui.c.line_text(&summary, medium(14), Rect::new(x, y, w - 200.0, 30.0), th.ink2, Align::Left);
    let bw = ui.button_width("Restore Defaults", false);
    if ui.button("defaults", "Restore Defaults", x + w - bw, y, Btn::Secondary, false) {
        later(move || { app().update(|s| for act in catalog::actions().into_iter().filter(|x| x.module == m) { s.action_shortcuts.remove(&act.id); }); app().sync_hotkeys(); });
    }
    let mut y = y + 44.0;
    if let Some((id, msg)) = a.settings_window.shortcut_issue.clone() {
        if acts.iter().any(|x| x.id == id) {
            y += ui.label(&msg, medium(13), x, y, w, th.salsa) + 12.0;
        }
    }
    let groups: Vec<(Option<&str>, Vec<catalog::Action>)> = if m == ModuleId::WindowSnap {
        SNAP_GROUPS.iter().map(|g| (Some(g.0), g.1.iter().filter_map(|s| acts.iter().find(|x| x.id == catalog::snap_id(*s)).cloned()).collect())).collect()
    } else { vec![(None, acts.clone())] };
    if acts.is_empty() { return y + ui.label("This gadget has no keyboard commands of its own. Its tray icon and the Open shortcut on Settings reach it.", sans(13), x, y, w, th.ink2) + 20.0; }
    for (title, list) in groups {
        if let Some(t) = title { y = heading(ui, t, x, y, w); }
        y = card(ui, &format!("sc{}{}", m.key(), title.unwrap_or("")), x, y, w, |ui, y| {
            let mut y = y;
            for (i, act) in list.iter().enumerate() {
                if i > 0 { divider(ui, x, y, w); }
                let clash = a.hotkey_clashes.contains(&act.id);
                ui.c.line_text(&act.title, sans(14), Rect::new(x + 18.0, y, w * 0.5, 46.0), if clash { th.nacho_ink } else { th.ink }, Align::Left);
                key_field(ui, &act.id, a.settings.shortcut_for(act), act.default, x + w - 238.0, y + 8.0);
                y += 46.0;
            }
            y
        }) + 20.0;
    }
    y + 10.0
}

/// A key cap field: click it and press a new combination. Backspace removes, ↺ brings back the default, Esc cancels.
fn key_field(ui: &mut Ui, id: &str, cur: Option<Shortcut>, default: Option<Shortcut>, x: f64, y: f64) {
    let th = ui.th;
    let listening = app().settings_window.capture_for.as_deref() == Some(id);
    let r = Rect::new(x, y, 190.0, 30.0);
    let hid = ui.id(&format!("kf{id}"));
    let resp = ui.hit(hid, r, true);
    ui.c.round(r, 7.0, if listening { NACHO.alpha(0.3) } else { th.paper });
    ui.c.stroke_round(r, 7.0, if listening { th.nacho_ink } else if resp.hover { th.ink2 } else { th.line }, if listening { 2.0 } else { 1.0 });
    if listening { ui.c.line_text("Press keys…", medium(13), r, th.nacho_ink, Align::Center); }
    else if let Some(s) = cur {
        let parts = s.parts();
        let width: f64 = parts.iter().map(|p| ui.c.measure(p, mono(11), 200.0).0.ceil().max(10.0) + 15.0).sum();
        ui.keycaps(&parts, r.cx() - width / 2.0, r.y + 4.0);
    } else { ui.c.line_text("None", sans(13), r, th.ink2, Align::Center); }
    if resp.hover { ui.st.cursor_hand = true; }
    if resp.clicked { app().settings_window.capture_for = Some(id.to_string()); app().settings_window.shortcut_issue = None; ui.st.capture_key = Some(hid); ui.st.again = true; }
    if !listening && ui.st.capture_key == Some(hid) { ui.st.capture_key = None; }
    if default.is_some() && cur != default {
        let rr = Rect::new(r.right() + 8.0, r.y + 3.0, 24.0, 24.0);
        let rid = ui.id(&format!("rs{id}"));
        let rresp = ui.hit(rid, rr, true);
        if rresp.hover { ui.c.round(rr, 5.0, th.line); }
        ui.c.glyph(0xE7A7, 12, rr, th.ink2);
        if rresp.clicked { let id = id.to_string(); later(move || { app().update(|s| { s.action_shortcuts.remove(&id); }); app().sync_hotkeys(); }); }
    }
}

fn capture_key(vk: u32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let a = app();
    let Some(id) = a.settings_window.capture_for.clone() else { return };
    let d = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) < 0 };
    if [0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5].contains(&vk) { return; }
    let mods = (if d(VK_MENU) { shortcut::ALT } else { 0 }) | (if d(VK_CONTROL) { shortcut::CTRL } else { 0 }) | (if d(VK_SHIFT) { shortcut::SHIFT } else { 0 }) | (if d(VK_LWIN) || d(VK_RWIN) { shortcut::WIN } else { 0 });
    let done = |a: &mut app::App| { a.settings_window.capture_for = None; a.settings_window.invalidate(); };
    if vk == 0x1B && mods == 0 { done(a); return; }
    let tool = id.strip_prefix("open.").and_then(ModuleId::from_key);
    if vk == 0x08 && mods == 0 {
        match tool { Some(m) => { a.settings.tool_shortcuts.remove(&m); } None => { a.settings.action_shortcuts.insert(id.clone(), None); } }
        a.save();
        a.sync_hotkeys();
        done(a);
        a.changed();
        return;
    }
    let s = Shortcut::new(vk, mods);
    if let Some(issue) = shortcut::validate(s, &a.settings.taken_shortcuts(&id)) {
        a.settings_window.shortcut_issue = Some((id, issue.message()));
        done(a);
        return;
    }
    // The old one is released first, so pressing the same keys again is not reported as "held by another app".
    let saved = (a.settings.tool_shortcuts.clone(), a.settings.action_shortcuts.clone());
    match tool { Some(m) => { a.settings.tool_shortcuts.remove(&m); } None => { a.settings.action_shortcuts.insert(id.clone(), None); } }
    a.sync_hotkeys();
    if !a.hotkey_free(s) {
        (a.settings.tool_shortcuts, a.settings.action_shortcuts) = saved;
        a.sync_hotkeys();
        a.settings_window.shortcut_issue = Some((id, format!("Another app already uses {}. The previous shortcut stays.", s.label())));
        done(a);
        return;
    }
    match tool { Some(m) => { a.settings.tool_shortcuts.insert(m, s); } None => { a.settings.action_shortcuts.insert(id, Some(s)); } }
    a.save();
    a.sync_hotkeys();
    done(a);
    a.changed();
}

// ---------- combos ----------

fn combos_page(ui: &mut Ui, x: f64, y: f64, w: f64) -> f64 {
    let a = app();
    ui.c.line_text("COMBOS", mono(11), Rect::new(x, y, 200.0, 16.0), ui.th.ink2, Align::Left);
    ui.c.line_text("WHAT'S YOUR ORDER?", sign(40), Rect::new(x, y + 18.0, w, 54.0), ui.th.ink, Align::Left);
    let mut y = y + 78.0;
    y += ui.label("Pick a combo to switch on a few gadgets together, or make your own. Before you order, it shows what changes in your box. Every gadget stays its own switch afterwards.", sans(14), x, y, w, ui.th.ink2) + 20.0;
    let board_top = y;
    let id = ui.id("board");
    let bh = ui.st.content.get(&id).copied().unwrap_or(600.0);
    ui.c.round(Rect::new(x, y, w, bh), 18.0, BOARD);
    ui.c.stroke_round(Rect::new(x, y, w, bh), 18.0, BOARD_LINE, 1.5);
    let inner = x + 20.0;
    let iw = w - 40.0;
    let all = combos::all(&a.settings);
    let cur = combos::current(&a.settings).map(|c| c.id);
    if a.settings_window.combo_pick.is_none() { a.settings_window.combo_pick = cur.clone().or(Some("desk".into())); }
    let pick = a.settings_window.combo_pick.clone().unwrap_or_default();
    let cw = ((iw - 4.0 * 10.0) / 5.0).min(150.0);
    let mut y = y + 20.0;
    let chips = |ui: &mut Ui, list: &[Combo], y: f64| -> f64 {
        for (i, c) in list.iter().enumerate() {
            let r = Rect::new(inner + (i % 5) as f64 * (cw + 10.0), y + (i / 5) as f64 * 118.0, cw, 108.0);
            let id = ui.id(&format!("chip{}", c.id));
            let resp = ui.hit(id, r, true);
            let sel = c.id == pick;
            ui.c.round(r, 12.0, if sel { BOARD_PICK } else { BOARD2 });
            ui.c.stroke_round(r, 12.0, if sel { NACHO } else if resp.hover { BOARD_MUTE } else { BOARD_LINE }, if sel { 2.5 } else { 1.0 });
            ui.c.line_text(&format!("NO.{}", c.number), sign(26), Rect::new(r.x + 12.0, r.y + 10.0, r.w - 24.0, 34.0), NACHO, Align::Left);
            ui.c.line_text(&c.name.to_uppercase(), sign(16), Rect::new(r.x + 12.0, r.y + 46.0, r.w - 24.0, 24.0), BOARD_INK, Align::Left);
            let tag = if cur.as_ref() == Some(&c.id) { "In your box".to_string() } else { format!("{} gadget{}", c.gadgets.len(), if c.gadgets.len() == 1 { "" } else { "s" }) };
            ui.c.line_text(&tag, sans(12), Rect::new(r.x + 12.0, r.y + 74.0, r.w - 24.0, 18.0), if cur.as_ref() == Some(&c.id) { BOARD_GUAC } else { BOARD_MUTE }, Align::Left);
            if resp.clicked { app().settings_window.combo_pick = Some(c.id.clone()); app().settings_window.editing = None; ui.st.again = true; }
        }
        y + list.len().div_ceil(5).max(1) as f64 * 118.0
    };
    let presets: Vec<Combo> = all.iter().filter(|c| !c.custom).cloned().collect();
    let custom: Vec<Combo> = all.iter().filter(|c| c.custom).cloned().collect();
    y = chips(ui, &presets, y) + 8.0;
    ui.c.line_text("YOUR COMBOS", mono(11), Rect::new(inner, y, 200.0, 16.0), BOARD_MUTE, Align::Left);
    y += 24.0;
    let top = y;
    y = chips(ui, &custom, y);
    if custom.len() < combos::MAX_CUSTOM {
        let i = custom.len();
        let r = Rect::new(inner + (i % 5) as f64 * (cw + 10.0), top + (i / 5) as f64 * 118.0, cw, 108.0);
        let id = ui.id("newcombo");
        let resp = ui.hit(id, r, true);
        ui.c.round(r, 12.0, if resp.hover { BOARD2 } else { BOARD });
        ui.c.stroke_round(r, 12.0, BOARD_LINE, 1.5);
        ui.c.line_text("+", sign(22), Rect::new(r.x + 12.0, r.y + 10.0, 30.0, 30.0), NACHO, Align::Left);
        ui.c.line_text("NEW COMBO", sign(16), Rect::new(r.x + 12.0, r.y + 46.0, r.w - 24.0, 24.0), BOARD_INK, Align::Left);
        ui.c.line_text("From your box", sans(12), Rect::new(r.x + 12.0, r.y + 74.0, r.w - 24.0, 18.0), BOARD_MUTE, Align::Left);
        if resp.clicked { let d = new_draft(&a.settings, None); app().settings_window.editing = Some(d); ui.st.again = true; }
        y = top + (i + 1).div_ceil(5) as f64 * 118.0;
    }
    y += 14.0;
    y = if let Some(mut draft) = a.settings_window.editing.clone() {
        let end = editor(ui, &mut draft, inner, y, iw);
        if app().settings_window.editing.is_some() { app().settings_window.editing = Some(draft); }
        end
    } else if let Some(c) = all.iter().find(|c| c.id == pick).cloned() { detail(ui, &c, inner, y, iw) } else { y };
    let end = y + 20.0;
    if ((end - board_top) - bh).abs() > 0.5 { ui.st.content.insert(id, end - board_top); ui.st.again = true; }
    let y = end + 16.0;
    y + ui.label("A combo switches gadgets and a few settings; it never changes your shortcuts. You can switch any gadget on or off afterwards, and undo an order.", sans(12), x, y, w, ui.th.ink2) + 30.0
}

fn gadget_tray(ui: &mut Ui, key: &str, on: &[ModuleId], x: f64, y: f64, w: f64, editable: bool) -> (Option<ModuleId>, f64) {
    let cols = 6usize.min(ModuleId::ALL.len());
    let cw = (w - (cols as f64 - 1.0) * 10.0) / cols as f64;
    let mut hit = None;
    for (i, m) in ModuleId::ALL.into_iter().enumerate() {
        let r = Rect::new(x + (i % cols) as f64 * (cw + 10.0), y + (i / cols) as f64 * 104.0, cw, 94.0);
        let lit = on.contains(&m);
        let resp = if editable { let id = ui.id(&format!("{key}{}", m.key())); ui.hit(id, r, true) } else { Resp::default() };
        if lit { ui.c.round(r, 12.0, BOARD2); ui.c.stroke_round(r, 12.0, NACHO, 2.0); }
        else { ui.c.stroke_round(r, 12.0, if resp.hover { BOARD_MUTE } else { BOARD_LINE }, 1.0); }
        if lit { tile(ui.c, m, r.cx() - 16.0, r.y + 14.0, 32.0); } else {
            let (a, b, ink) = tile_colors(m);
            ui.c.gradient_round(Rect::new(r.cx() - 16.0, r.y + 14.0, 32.0, 32.0), 8.0, a.alpha(0.35), b.alpha(0.35));
            gadget_icon(ui.c, m, r.cx() - 10.0, r.y + 20.0, 20.0, ink.alpha(0.5));
        }
        ui.c.text(m.name(), semi(12), Rect::new(r.x + 6.0, r.y + 54.0, r.w - 12.0, 36.0), if lit { BOARD_INK } else { BOARD_MUTE }, Align::Center);
        if resp.clicked { hit = Some(m); }
    }
    (hit, y + ModuleId::ALL.len().div_ceil(cols) as f64 * 104.0)
}

fn detail(ui: &mut Ui, c: &Combo, x: f64, y: f64, w: f64) -> f64 {
    let a = app();
    let card = Rect::new(x, y, w, 10.0);
    let _ = card;
    ui.c.line_text(&c.name.to_uppercase(), sign(36), Rect::new(x, y, w, 48.0), NACHO, Align::Left);
    let mut y = y + 52.0;
    ui.c.line_text(&c.line, semi(16), Rect::new(x, y, w, 24.0), BOARD_INK, Align::Left);
    y += 26.0;
    y += ui.label(&c.who, sans(14), x, y, w, BOARD_MUTE) + 16.0;
    let (_, ny) = gadget_tray(ui, "show", &c.gadgets, x, y, w, false);
    y = ny + 10.0;
    ui.c.line_text("WHAT CHANGES IN YOUR BOX", mono(12), Rect::new(x, y, w, 18.0), BOARD_MUTE, Align::Left);
    y += 28.0;
    let plan = combos::plan(c, &a.settings);
    let names = |v: &[ModuleId]| v.iter().map(|m| m.name()).collect::<Vec<_>>().join(", ");
    let mut rows: Vec<(&str, Color, String)> = vec![];
    if !plan.turns_on.is_empty() { rows.push(("TURNS ON", NACHO, names(&plan.turns_on))); }
    if !plan.stays_on.is_empty() { rows.push(("STAYS ON", BOARD_MUTE, names(&plan.stays_on))); }
    if !plan.turns_off.is_empty() { rows.push(("TURNS OFF", BOARD_SALSA, names(&plan.turns_off))); }
    for ch in &plan.changes { rows.push(("SETTING", BOARD_MUTE, ch.clone())); }
    if rows.is_empty() { rows.push(("", BOARD_MUTE, "Nothing: this is your box already.".into())); }
    for (k, col, v) in rows {
        ui.c.line_text(k, mono(12), Rect::new(x, y, 110.0, 24.0), col, Align::Left);
        y += ui.label(&v, sans(14), x + 120.0, y + 2.0, w - 120.0, BOARD_INK).max(22.0) + 6.0;
    }
    y += 14.0;
    let no_change = plan.turns_on.is_empty() && plan.turns_off.is_empty() && plan.changes.is_empty();
    let label = format!("Order {}", c.name);
    let bw = ui.button_width(&label, false);
    let cc = c.clone();
    if ui.button_rect("order", &label, Rect::new(x, y, bw, 32.0), Btn::Primary, false, !no_change) { later(move || app().order(&cc)); }
    let mut bx = x + bw + 12.0;
    if let Some((_, name)) = &a.undo {
        let l = format!("Undo {name}");
        let uw = ui.button_width(&l, false);
        if ui.button("undo", &l, bx, y + 1.0, Btn::Board, false) { later(|| app().undo_order()); }
        bx += uw + 12.0;
    }
    let _ = bx;
    y += 48.0;
    ui.c.line(x, y, x + w, y, BOARD_LINE, 1.0);
    y += 12.0;
    let own = if c.custom { "Edit This Combo".to_string() } else { format!("Make Your Own from {}", c.name) };
    let ow = ui.button_width(&own, false);
    let cc = c.clone();
    if ui.button("own", &own, x, y, Btn::Board, false) {
        let d = if cc.custom { SavedCombo { id: cc.id.clone(), name: cc.name.clone(), gadgets: cc.gadgets.clone(), tuning: cc.tuning.clone() } } else { new_draft(&app().settings, Some(&cc)) };
        app().settings_window.editing = Some(d);
        ui.st.again = true;
    }
    if c.custom && ui.button("delete", "Delete", x + ow + 12.0, y, Btn::Board, false) {
        let id = c.id.clone();
        later(move || { app().update(|s| { s.custom_combos.retain(|x| x.id != id); if s.combo.as_deref() == Some(&id) { s.combo = None; } }); app().settings_window.combo_pick = None; });
    }
    y + 40.0
}

fn editor(ui: &mut Ui, d: &mut SavedCombo, x: f64, y: f64, w: f64) -> f64 {
    ui.c.line_text("YOUR OWN COMBO", mono(12), Rect::new(x, y, w, 18.0), BOARD_MUTE, Align::Left);
    let mut y = y + 26.0;
    ui.text_field("name", Rect::new(x, y, 320.0, 34.0), &mut d.name, combos::MAX_NAME);
    y += 50.0;
    ui.c.line_text("Tap the gadgets to put them in or take them out.", sans(13), Rect::new(x, y, w, 20.0), BOARD_MUTE, Align::Left);
    y += 28.0;
    let (hit, ny) = gadget_tray(ui, "edit", &d.gadgets.clone(), x, y, w, true);
    if let Some(m) = hit { if d.gadgets.contains(&m) { d.gadgets.retain(|g| *g != m); } else { d.gadgets.push(m); } ui.st.again = true; }
    y = ny + 10.0;
    ui.c.line_text("THE SETTINGS IT BRINGS", mono(12), Rect::new(x, y, w, 18.0), BOARD_MUTE, Align::Left);
    y += 28.0;
    let keep = "Keep as it is".to_string();
    if d.gadgets.contains(&ModuleId::WindowSnap) {
        ui.c.line_text("Gap between windows", sans(13), Rect::new(x, y, 220.0, 30.0), BOARD_INK, Align::Left);
        let opts: Vec<String> = std::iter::once(keep.clone()).chain(Options::GAP_CHOICES.iter().map(|g| if *g == 0 { "None".into() } else { format!("{g} px") })).collect();
        let cur = d.tuning.snap_gap.and_then(|g| Options::GAP_CHOICES.iter().position(|x| *x == g).map(|i| i + 1)).unwrap_or(0);
        if let Some(i) = ui.segments("tgap", &opts, Some(cur), Rect::new(x + 230.0, y, (w - 230.0).min(560.0), 30.0), true) { d.tuning.snap_gap = (i > 0).then(|| Options::GAP_CHOICES[i - 1]); }
        y += 40.0;
    }
    if d.gadgets.contains(&ModuleId::FocusDim) {
        ui.c.line_text("Dim strength", sans(13), Rect::new(x, y, 220.0, 30.0), BOARD_INK, Align::Left);
        let vals = [0.3, 0.4, 0.5, 0.6, 0.7];
        let opts: Vec<String> = std::iter::once(keep.clone()).chain(vals.iter().map(|v| format!("{:.0}%", v * 100.0))).collect();
        let cur = d.tuning.dim_intensity.and_then(|g| vals.iter().position(|x| (*x - g).abs() < 0.01).map(|i| i + 1)).unwrap_or(0);
        if let Some(i) = ui.segments("tdim", &opts, Some(cur), Rect::new(x + 230.0, y, (w - 230.0).min(520.0), 30.0), true) { d.tuning.dim_intensity = (i > 0).then(|| vals[i - 1]); }
        y += 40.0;
    }
    if d.gadgets.contains(&ModuleId::TrayOrganizer) {
        ui.c.line_text("Fold away again after", sans(13), Rect::new(x, y, 220.0, 30.0), BOARD_INK, Align::Left);
        let opts: Vec<String> = std::iter::once(keep.clone()).chain(Options::COLLAPSE_CHOICES.iter().map(|g| if *g == 0 { "Never".into() } else { format!("{g} s") })).collect();
        let cur = d.tuning.auto_collapse_seconds.and_then(|g| Options::COLLAPSE_CHOICES.iter().position(|x| *x == g).map(|i| i + 1)).unwrap_or(0);
        if let Some(i) = ui.segments("tcol", &opts, Some(cur), Rect::new(x + 230.0, y, (w - 230.0).min(560.0), 30.0), true) { d.tuning.auto_collapse_seconds = (i > 0).then(|| Options::COLLAPSE_CHOICES[i - 1]); }
        y += 40.0;
    }
    y += 10.0;
    let ok = !d.gadgets.is_empty();
    if ui.button_rect("save", "Save Combo", Rect::new(x, y, 130.0, 32.0), Btn::Primary, false, ok) {
        let c = d.clone();
        later(move || { let a = app(); let mut s = a.settings.clone(); if combos::save(&mut s, c.clone()) { a.update(|x| *x = s); a.settings_window.combo_pick = Some(c.id); } a.settings_window.editing = None; a.settings_window.invalidate(); });
    }
    if ui.button("cancel", "Cancel", x + 142.0, y, Btn::Board, false) { app().settings_window.editing = None; ui.st.again = true; }
    if !ok { ui.c.line_text("A combo needs at least one gadget.", sans(12), Rect::new(x + 240.0, y, 300.0, 32.0), BOARD_SALSA, Align::Left); }
    y + 44.0
}

// ---------- general ----------

fn general(ui: &mut Ui, ctx: &mut WindowCtx, x: f64, y: f64, w: f64) -> f64 {
    let th = ui.th;
    let a = app();
    ui.c.line_text("General", bold(24), Rect::new(x, y, w, 32.0), th.ink, Align::Left);
    let mut y = y + 52.0;
    y = card(ui, "gen", x, y, w, |ui, y| {
        let login = app::login_enabled();
        ui.c.line_text("Open at Sign-In", sans(14), Rect::new(x + 18.0, y, w * 0.6, 46.0), th.ink, Align::Left);
        if ui.switch("login", x + w - 62.0, y + 12.0, login) { app::set_login(!login); ui.st.again = true; }
        let mut y = y + 46.0;
        divider(ui, x, y, w);
        let (r, ny) = choice(ui, "launch", "Show This Window at Launch", &[(LaunchWindow::Always, "Always"), (LaunchWindow::WhenNeeded, "When Something Needs You"), (LaunchWindow::Never, "Never")], a.settings.launch_window, x, y, w);
        if let Some(v) = r { a.update(|s| s.launch_window = v); }
        y = ny;
        divider(ui, x, y, w);
        let names: Vec<(Option<&str>, &str)> = std::iter::once((None, "Windows")).chain(crate::i18n::LANGS.iter().map(|l| (Some(l.0), l.1))).collect();
        let cur = names.iter().position(|n| n.0 == a.settings.language.as_deref()).unwrap_or(0);
        option_row(ui, "Language", x, y, w);
        let labels: Vec<String> = names.iter().map(|n| n.1.to_string()).collect();
        if let Some(i) = ui.segments("lang", &labels, Some(cur), Rect::new(x + w - 18.0 - 440.0, y + 7.0, 440.0, 30.0), false) {
            let choice = names[i].0.map(String::from);
            crate::i18n::set_language(choice.as_deref());
            a.update(|s| s.language = choice);
        }
        y += 44.0;
        divider(ui, x, y, w);
        let (hit, ny) = check(ui, "dot", "An orange dot on the tray icon when a gadget needs you", a.settings.attention_dot, x, y, w);
        if hit { a.update(|s| s.attention_dot = !s.attention_dot); }
        y = ny;
        divider(ui, x, y, w);
        ui.c.line_text("Settings File", sans(14), Rect::new(x + 18.0, y, w * 0.5, 46.0), th.ink, Align::Left);
        let bw = ui.button_width("Show in File Explorer", true);
        if ui.button("folder", "Show in File Explorer", x + w - 18.0 - bw, y + 11.0, Btn::Secondary, true) { let d = a.dir.clone(); later(move || app::open_url(&d.to_string_lossy())); }
        y + 46.0
    }) + 24.0;
    y = card(ui, "danger", x, y, w, |ui, y| {
        ui.c.line_text("Reset All Settings", sans(14), Rect::new(x + 18.0, y, w * 0.5, 46.0), th.ink, Align::Left);
        let confirm = a.settings_window.confirm_reset;
        let label = if confirm { "Really Reset Everything" } else { "Reset…" };
        let bw = ui.button_width(label, true);
        if ui.button("reset", label, x + w - 18.0 - bw, y + 11.0, if confirm { Btn::Attention } else { Btn::Secondary }, true) {
            if confirm {
                later(|| { let a = app(); for m in ModuleId::ALL { a.stop(m); } a.settings = AppSettings { welcomed: true, ..Default::default() }; a.save(); a.sync_hotkeys(); crate::tray::sync_pins(); a.settings_window.confirm_reset = false; a.changed(); });
            } else { app().settings_window.confirm_reset = true; ui.st.again = true; }
        }
        let y = y + 46.0;
        if confirm { return y + ui.label("Every gadget goes off and every choice goes back to its default. Shelf copies are deleted.", sans(12), x + 18.0, y, w - 36.0, th.salsa) + 10.0; }
        y
    }) + 24.0;
    ui.c.line_text(&format!("{PRODUCT} {}", env!("CARGO_PKG_VERSION")), medium(13), Rect::new(x, y, w, 20.0), th.ink2, Align::Left);
    y += 30.0;
    if ui.button("quitapp", "Quit NachoCombo", x, y, Btn::Secondary, false) { ctx.close = true; app::quit(); }
    y + 60.0
}

// ---------- ctl ----------

pub fn ctl(a: &[String]) {
    let arg = |i: usize| a.get(i).map(String::as_str).unwrap_or("");
    match arg(0) {
        "open" => open(None),
        "page" => if let Some(p) = page_from(arg(1)) { open(None); go(p); },
        "select" => if let Some(m) = ModuleId::from_key(arg(1)) { open(Some(m)); },
        "tab" => { app().settings_window.tab = Some(match arg(1) { "settings" => Tab::Settings, "shortcuts" => Tab::Shortcuts, _ => Tab::Overview }); app().settings_window.invalidate(); }
        "close" => if let Some(h) = app().settings_window.win { unsafe { let _ = DestroyWindow(HWND(h as *mut _)); } },
        _ => {}
    }
}

/// `ctl snapshot settings|panel|monitor <file.png>`: the window's picture without capturing the screen.
pub fn snapshot(which: &str, path: &std::path::Path) {
    let a = app();
    let h = match which { "settings" => a.settings_window.win, "panel" => a.panel.hwnd().map(|h| h.0 as isize), "monitor" => a.monitor.panel, "clock" => a.clock.popup_hwnd(), _ => None };
    let Some(h) = h else { return };
    let ptr = unsafe { GetWindowLongPtrW(HWND(h as *mut _), GWLP_USERDATA) } as *mut UiWindow;
    if !ptr.is_null() { if let Err(e) = unsafe { &mut *ptr }.snapshot(path) { let _ = std::fs::write(a.dir.join("snapshot-error.txt"), format!("{e:?}")); } }
}
