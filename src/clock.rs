//! Calendar Clock: a taskbar-corner icon that shows today's date and whose tooltip is the clock; a click opens a month
//! calendar with world clocks. The logic that decides what to show is in `core::clock`.
use crate::app::{app, later, State, TIMER_CLOCK};
use crate::core::clock::*;
use crate::core::layout::Rect;
use crate::core::settings::ModuleId;
use crate::gfx::*;
use crate::tray::{icon_from_pixels, icon_size, taskbar_is_light};
use crate::ui::*;
use crate::window::{UiWindow, WindowCtx};
use windows::Win32::Foundation::*;
use windows::Win32::Globalization::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const UID: u32 = 32;
const W: f64 = 300.0;
const PAD: f64 = 14.0;
const ROW: f64 = 32.0;

#[derive(Default)]
pub struct Clock {
    popup: Option<isize>,
    /// What the tooltip says now; the icon is only touched when it changes.
    title: String,
    /// The day the icon was drawn for.
    icon_day: Option<u32>,
    /// The month the calendar shows, and the day picked in it.
    view: (i32, u32),
    selected: (i32, u32, u32),
    pub city_query: String,
    closed_at: Option<std::time::Instant>,
}

impl Clock {
    pub fn popup_hwnd(&self) -> Option<isize> { self.popup }
    /// The first line of the tooltip: what the clock reads now.
    pub fn status(&self) -> String { self.title.lines().next().unwrap_or_default().to_string() }
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "title": self.title, "popup": self.popup.is_some(), "iconDay": self.icon_day, "view": format!("{}-{:02}", self.view.0, self.view.1) })
    }
}

/// Local time, UTC as seconds since 1970, and how far local is ahead of UTC.
struct Now { local: Civil, utc: i64, offset: i64 }

fn now() -> Now {
    let (l, u) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let civil = |t: &windows::Win32::Foundation::SYSTEMTIME| Civil { year: t.wYear as i32, month: t.wMonth as u32, day: t.wDay as u32, hour: t.wHour as u32, minute: t.wMinute as u32, second: t.wSecond as u32 };
    let (local, utc) = (civil(&l), civil(&u).to_unix());
    // The two reads are a moment apart: round to the minute.
    let offset = (((local.to_unix() - utc) as f64) / 60.0).round() as i64 * 60;
    Now { local, utc, offset }
}

/// UTC now as seconds since 1970, for the settings page's offsets.
pub fn utc_now() -> i64 { now().utc }

/// How far local time is ahead of UTC, in seconds.
pub fn local_offset() -> i64 { now().offset }

fn locale_number(kind: u32) -> Option<u32> {
    let mut buf = [0u16; 8];
    let n = unsafe { GetLocaleInfoEx(windows::core::PCWSTR::null(), kind, Some(&mut buf)) };
    if n <= 1 { return None; }
    String::from_utf16_lossy(&buf[..n as usize - 1]).trim().parse().ok()
}

/// Whether Windows itself shows a 24-hour clock.
fn system_24() -> bool { locale_number(LOCALE_ITIME) == Some(1) }

/// The weekday the week starts on in this PC's region (0 = Sunday).
fn first_weekday() -> u32 { first_weekday_from_windows(locale_number(LOCALE_IFIRSTDAYOFWEEK).unwrap_or(6)) }

fn lang() -> String { crate::i18n::language() }

/// The clock line and the tooltip: the clock, then the world clocks.
fn texts(n: &Now) -> (String, String) {
    let o = &app().settings.options;
    let l = lang();
    let sys = system_24();
    let clock = clock_text(&l, &n.local, o.clock_date, o.clock_weekday, o.clock_seconds, o.clock_hours, sys);
    let mut tip = clock.clone();
    for id in o.world_clocks.iter().take(3) {
        if let Some(off) = zone_offset(id, n.utc) {
            let there = Civil::from_unix(n.utc + off);
            tip += &format!("\n{} {}", city(id), time_text(&l, &there, false, o.clock_hours.is_24(sys)));
        }
    }
    (clock, tip)
}

pub fn start() -> std::result::Result<(), String> {
    let a = app();
    unsafe { SetTimer(Some(a.main), TIMER_CLOCK, 1000, None); }
    a.clock = Clock::default();
    refresh();
    Ok(())
}

pub fn stop() {
    let a = app();
    unsafe { let _ = KillTimer(Some(a.main), TIMER_CLOCK); }
    close_popup();
    a.clock = Clock::default();
    a.tray.remove(UID);
}

/// Picks up changed options right away.
pub fn options_changed() {
    if app().state(ModuleId::CalendarClock) != State::Running { return; }
    // The calendar's height depends on the world clocks, so it opens again on the next click.
    close_popup();
    app().clock.title.clear();
    refresh();
}

/// The taskbar changed colour: the icon is drawn again.
pub fn redraw_tray() {
    if app().state(ModuleId::CalendarClock) != State::Running { return; }
    app().clock.icon_day = None;
    refresh();
}

/// Once a second: the tooltip and icon change only when what they say changes.
pub fn tick() {
    if app().state(ModuleId::CalendarClock) != State::Running { return; }
    refresh();
    if let Some(h) = app().clock.popup { unsafe { let _ = InvalidateRect(Some(HWND(h as *mut _)), None, false); } }
}

fn refresh() {
    let a = app();
    let n = now();
    let (_, tip) = texts(&n);
    let day = n.local.day;
    if a.clock.title == tip && a.clock.icon_day == Some(day) { return; }
    a.clock.title = tip.clone();
    a.clock.icon_day = Some(day);
    a.tray.set(UID, draw_icon(day), &tip, true);
}

/// A calendar page with today's date, in the taskbar's colour.
fn draw_icon(day: u32) -> HICON {
    let n = icon_size();
    let s = n as f64;
    let ink = if taskbar_is_light() { rgb(0x24180D) } else { rgb(0xFFFFFF) };
    let px = render_offscreen(n as u32, n as u32, 96.0, None, |c| {
        let body = Rect::new(s * 0.08, s * 0.1, s * 0.84, s * 0.82);
        c.stroke_round(body, s * 0.12, ink, (s * 0.08).max(1.0));
        let band = Rect::new(body.x, body.y, body.w, s * 0.26);
        c.round(band, s * 0.12, NACHO);
        c.fill(Rect::new(band.x, band.y + band.h * 0.5, band.w, band.h * 0.5), NACHO);
        c.line_text(&day.to_string(), bold(((s * 0.46).round() as u32).max(7)), Rect::new(body.x, band.bottom(), body.w, body.bottom() - band.bottom()), ink, Align::Center);
    }).unwrap_or_default();
    icon_from_pixels(&px, n, n)
}

// ---------- the calendar ----------

pub fn close_popup() {
    if let Some(h) = app().clock.popup.take() { unsafe { let _ = DestroyWindow(HWND(h as *mut _)); } }
}

pub fn toggle_popup() {
    let a = app();
    if a.state(ModuleId::CalendarClock) != State::Running { return; }
    if a.clock.popup.is_some() { close_popup(); return; }
    // Clicking the icon first takes the focus from the open calendar, which closes it: that click is not a new open.
    if a.clock.closed_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(300)) { return; }
    open_popup();
}

/// The shortcut and the tray menu: opens the calendar if it is not open.
pub fn show() {
    if app().state(ModuleId::CalendarClock) != State::Running { return; }
    if app().clock.popup.is_none() { open_popup(); }
}

fn height(worlds: usize) -> f64 {
    let grid = 44.0 + 40.0 + 26.0 + 6.0 * ROW + 8.0 + 52.0;
    let world = if worlds > 0 { 14.0 + worlds as f64 * 30.0 } else { 0.0 };
    grid + world + 14.0 + 40.0
}

fn open_popup() {
    let a = app();
    let n = now();
    a.clock.view = (n.local.year, n.local.month);
    a.clock.selected = (n.local.year, n.local.month, n.local.day);
    let h = height(a.settings.options.world_clocks.len());
    let win = UiWindow::create("Calendar Clock", WS_POPUP, WS_EX_TOOLWINDOW | WS_EX_TOPMOST, 0, 0, W, h, Box::new(draw));
    win.close_on_deactivate = true;
    win.on_close = Some(Box::new(|| { let c = &mut app().clock; c.popup = None; c.closed_at = Some(std::time::Instant::now()); }));
    let hw = win.hwnd;
    a.clock.popup = Some(hw.0 as isize);
    place(hw);
    win.show();
}

/// Beside the clock's icon, or by the pointer when the shortcut opened it.
fn place(h: HWND) {
    let Some(r) = app().tray.icon_rect(UID) else { crate::monitor_panels::place_near_tray(h); return };
    unsafe {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mid = POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
        let _ = GetMonitorInfoW(MonitorFromPoint(mid, MONITOR_DEFAULTTONEAREST), &mut mi);
        let mut wr = RECT::default();
        let _ = GetWindowRect(h, &mut wr);
        let (w, hh) = (wr.right - wr.left, wr.bottom - wr.top);
        let wk = mi.rcWork;
        let m = (8.0 * crate::window::dpi_scale(h)) as i32;
        let x = (mid.x - w / 2).clamp(wk.left + m, (wk.right - w - m).max(wk.left + m));
        let y = if mid.y > (wk.top + wk.bottom) / 2 { wk.bottom - hh - m } else { wk.top + m };
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

fn draw(ui: &mut Ui, ctx: &mut WindowCtx) {
    for k in ui.st.keys.clone() { if k.vk == 0x1B { ctx.close = true; } }
    let th = ui.th;
    let a = app();
    let n = now();
    let l = lang();
    let today = (n.local.year, n.local.month, n.local.day);
    ui.c.fill(Rect::new(0.0, 0.0, ui.w, 44.0), th.paper2);
    ui.c.line_text("CALENDAR CLOCK", sign(18), Rect::new(16.0, 0.0, 220.0, 44.0), th.ink, Align::Left);

    // Month header.
    let (vy, vm) = a.clock.view;
    let mut y = 52.0;
    ui.c.line_text(&month_title(&l, vy, vm), bold(15), Rect::new(PAD, y, 130.0, 28.0), th.ink, Align::Left);
    let next_w = 30.0;
    let today_w = ui.button_width("Today", true);
    let nx = ui.w - PAD - next_w;
    if ui.button_rect("next", "›", Rect::new(nx, y + 2.0, next_w, 24.0), Btn::Secondary, true, true) { a.clock.view = shift_month(vy, vm, 1); ui.st.again = true; }
    let tx = nx - 4.0 - today_w;
    if ui.button_rect("today", "Today", Rect::new(tx, y + 2.0, today_w, 24.0), Btn::Secondary, true, true) { a.clock.view = (today.0, today.1); a.clock.selected = today; ui.st.again = true; }
    if ui.button_rect("prev", "‹", Rect::new(tx - 4.0 - next_w, y + 2.0, next_w, 24.0), Btn::Secondary, true, true) { a.clock.view = shift_month(vy, vm, -1); ui.st.again = true; }
    y += 40.0;

    // The grid: weekday names, then the weeks.
    let first = first_weekday();
    let cell = (ui.w - PAD * 2.0) / 7.0;
    for (i, wd) in week_order(first).into_iter().enumerate() {
        ui.c.line_text(&weekday_short(&l, wd), mono(11), Rect::new(PAD + i as f64 * cell, y, cell, 22.0), th.ink2, Align::Center);
    }
    y += 26.0;
    for (r, week) in month_weeks(vy, vm, first).iter().enumerate() {
        for (c, day) in week.iter().enumerate() {
            if *day == 0 { continue; }
            let rect = Rect::new(PAD + c as f64 * cell, y + r as f64 * ROW, cell, ROW);
            let date = (vy, vm, *day);
            let id = ui.id(&format!("day{day}"));
            let resp = ui.hit(id, rect, true);
            let is_today = date == today;
            let picked = date == a.clock.selected;
            let disc = Rect::new(rect.cx() - 13.0, rect.cy() - 13.0, 26.0, 26.0);
            if is_today { ui.c.round(disc, 13.0, NACHO); }
            else if picked { ui.c.round(disc, 13.0, th.nacho_deep.alpha(0.28)); }
            else if resp.hover { ui.c.round(disc, 13.0, th.ink.alpha(0.08)); }
            if resp.focused && ui.st.keyboard_used { ui.c.stroke_round(disc, 13.0, th.nacho_ink, 2.0); }
            ui.c.line_text(&day.to_string(), if is_today { semi(12) } else { sans(12) }, rect, if is_today { ON_NACHO } else { th.ink }, Align::Center);
            if resp.hover { ui.st.cursor_hand = true; }
            if resp.clicked { a.clock.selected = date; }
        }
    }
    y += 6.0 * ROW + 8.0;

    // The picked day.
    let (sy, sm, sd) = a.clock.selected;
    let picked = Civil { year: sy, month: sm, day: sd, hour: 0, minute: 0, second: 0 };
    ui.c.line_text(&day_title(&l, &picked), semi(13), Rect::new(PAD, y, ui.w - PAD * 2.0, 22.0), th.ink, Align::Left);
    ui.c.line_text(&relative_day(days_between(today, a.clock.selected)), sans(12), Rect::new(PAD, y + 22.0, ui.w - PAD * 2.0, 20.0), th.ink2, Align::Left);
    y += 52.0;

    // World clocks.
    let o = &a.settings.options;
    let sys = system_24();
    if !o.world_clocks.is_empty() {
        ui.c.line(PAD, y, ui.w - PAD, y, th.line, 1.0);
        y += 12.0;
        for id in &o.world_clocks {
            let Some(off) = zone_offset(id, n.utc) else { continue };
            let there = Civil::from_unix(n.utc + off);
            ui.c.line_text(&city(id), medium(13), Rect::new(PAD, y, 110.0, 26.0), th.ink, Align::Left);
            ui.c.line_text(&offset_text(off - n.offset), sans(11), Rect::new(PAD + 112.0, y, 70.0, 26.0), th.ink2, Align::Left);
            ui.c.line_text(&clock_text(&l, &there, false, true, false, o.clock_hours, sys), mono(12), Rect::new(ui.w - PAD - 110.0, y, 110.0, 26.0), th.ink, Align::Right);
            y += 30.0;
        }
        y += 2.0;
    }

    // Footer.
    ui.c.line(PAD, y + 4.0, ui.w - PAD, y + 4.0, th.line, 1.0);
    y += 12.0;
    if ui.button("datetime", "Date & Time Settings", PAD, y, Btn::Secondary, true) { ctx.close = true; later(|| crate::app::open_url("ms-settings:dateandtime")); }
    let sw = ui.button_width("Clock Settings…", true);
    if ui.button("settings", "Clock Settings…", ui.w - PAD - sw, y, Btn::Secondary, true) { ctx.close = true; later(|| crate::pages::open(Some(ModuleId::CalendarClock))); }
}
