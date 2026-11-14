//! App Switch: a window switcher on a shortcut you hold and let go of. Which windows are listed and how the selection
//! moves is `core::appswitch`; this file finds the windows, draws the list and brings the chosen one to the front.
//! It holds nothing while it is idle: the shortcuts are the app's usual hotkeys (released with the gadget), the
//! overlay, its timer and the temporary Esc keys exist only while the list is open, and no keyboard hook is used.
use crate::app::{app, later, State, TIMER_APPSWITCH};
use crate::core::appswitch::{self as logic, Item, RawWindow};
use crate::core::layout::Rect;
use crate::core::settings::ModuleId;
use crate::core::shortcut::*;
use crate::gfx::*;
use crate::ui::*;
use crate::window::*;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Hotkey ids for Esc while the list is open; the app's own shortcuts count up from 1, the probe uses 9000.
pub const CANCEL_ID: i32 = 9101;
const ROW: f64 = 40.0;
const WIDTH: f64 = 460.0;
const PAD: f64 = 10.0;

#[derive(Default)]
pub struct AppSwitch {
    window: Option<isize>,
    items: Vec<Item>,
    selected: usize,
    /// First row shown, when the list is taller than the screen.
    top: usize,
    /// The modifiers of the shortcut that opened the list: letting go of them switches.
    mods: u32,
    cancel_keys: Vec<i32>,
    pub switches: u32,
    pub last_listed: usize,
}

impl AppSwitch {
    pub fn visible(&self) -> bool { self.window.is_some() }
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "switcherVisible": self.visible(), "switcherWindows": self.items.len(), "selected": self.selected, "switchCount": self.switches,
            "lastListed": self.last_listed, "cancelKeys": self.cancel_keys.len(), "timer": self.visible() })
    }
}

pub fn start() -> std::result::Result<(), String> { Ok(()) }

/// The OFF contract: the list, its timer and the Esc keys go; the shortcuts go with `App::sync_hotkeys`.
pub fn stop() { close(); }

// MARK: finding the windows

struct Collect { list: Vec<RawWindow> }

unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
    let c = &mut *(lp.0 as *mut Collect);
    if let Some(w) = describe(hwnd) { c.list.push(w); }
    BOOL(1)
}

fn exe_name(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 520];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        if !ok { return String::new(); }
        let path = String::from_utf16_lossy(&buf[..n as usize]);
        let file = path.rsplit(['\\', '/']).next().unwrap_or("");
        let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
        let mut chars = stem.chars();
        chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
    }
}

fn describe(hwnd: HWND) -> Option<RawWindow> {
    unsafe {
        let visible = IsWindowVisible(hwnd).as_bool();
        let minimized = IsIconic(hwnd).as_bool();
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let mut title = [0u16; 512];
        let n = GetWindowTextW(hwnd, &mut title);
        let mut class = [0u16; 128];
        let cn = GetClassNameW(hwnd, &mut class);
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        Some(RawWindow {
            id: hwnd.0 as isize, pid, app: String::new(),
            title: String::from_utf16_lossy(&title[..n.max(0) as usize]),
            class: String::from_utf16_lossy(&class[..cn.max(0) as usize]),
            width: r.right - r.left, height: r.bottom - r.top, visible: visible || minimized, cloaked: cloaked != 0,
            tool_window: ex & WS_EX_TOOLWINDOW.0 != 0, app_window: ex & WS_EX_APPWINDOW.0 != 0,
            owned: GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.0.is_null()), minimized,
        })
    }
}

/// Every window the switcher would list, front to back.
pub fn windows_now() -> Vec<Item> {
    let mut c = Collect { list: vec![] };
    unsafe { let _ = EnumWindows(Some(each), LPARAM(&mut c as *mut _ as isize)); }
    let own = std::process::id();
    // Looking up the program's name is the slow part, so only the windows that will be listed pay for it.
    let mut items = logic::list(&c.list, own);
    let mut names: std::collections::BTreeMap<u32, String> = Default::default();
    for i in &mut items { i.app = names.entry(i.pid).or_insert_with(|| exe_name(i.pid)).clone(); if i.app.is_empty() { i.app = i.title.clone(); } }
    items
}

// MARK: bringing a window forward

/// Windows only lets the process that last got input take the front. Joining the input queue of the window that
/// has it now is the usual, documented way to be allowed.
fn raise(id: isize, minimized: bool) {
    unsafe {
        let h = HWND(id as *mut _);
        if !IsWindow(Some(h)).as_bool() { return; }
        if minimized || IsIconic(h).as_bool() { let _ = ShowWindow(h, SW_RESTORE); }
        let fg = GetForegroundWindow();
        let (mine, theirs) = (GetCurrentThreadId(), if fg.0.is_null() { 0 } else { GetWindowThreadProcessId(fg, None) });
        let joined = theirs != 0 && theirs != mine && AttachThreadInput(mine, theirs, true).as_bool();
        let _ = BringWindowToTop(h);
        let _ = SetForegroundWindow(h);
        if joined { let _ = AttachThreadInput(mine, theirs, false); }
    }
}

// MARK: the list

fn shortcut_mods() -> u32 {
    let a = app();
    crate::core::catalog::actions().iter().find(|x| x.id == "appswitch.next")
        .and_then(|x| a.settings.shortcut_for(x)).map_or(CTRL | ALT, |s| s.mods)
}

/// Opens the list on the first press and moves the selection on each press after that.
pub fn cycle(forward: bool) {
    let a = app();
    if a.state(ModuleId::AppSwitch) != State::Running { return; }
    if a.appswitch.visible() {
        let n = a.appswitch.items.len();
        a.appswitch.selected = logic::move_selection(a.appswitch.selected, if forward { 1 } else { -1 }, n);
        reveal();
        invalidate();
        return;
    }
    let items = windows_now();
    a.appswitch.last_listed = items.len();
    if items.is_empty() { a.write_state(); return; }
    a.appswitch.selected = logic::first_selection(forward, items.len());
    a.appswitch.items = items;
    a.appswitch.top = 0;
    a.appswitch.mods = shortcut_mods();
    open();
    reveal();
    // Esc cancels; with the modifiers still held it arrives with them.
    for (n, m) in logic::cancel_mods(a.appswitch.mods).into_iter().enumerate() {
        let id = CANCEL_ID + n as i32;
        if unsafe { RegisterHotKey(Some(a.main), id, HOT_KEY_MODIFIERS(m) | MOD_NOREPEAT, 0x1B) }.is_ok() { a.appswitch.cancel_keys.push(id); }
    }
    // Letting go of the modifiers switches. Their state is read from the keyboard directly, which needs no hook.
    unsafe { SetTimer(Some(a.main), TIMER_APPSWITCH, 30, None); }
    a.changed();
}

fn invalidate() { if let Some(h) = app().appswitch.window { unsafe { let _ = InvalidateRect(Some(HWND(h as *mut _)), None, false); } } }

fn held_mods() -> u32 {
    let down = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 };
    (if down(VK_MENU.0) { ALT } else { 0 }) | (if down(VK_CONTROL.0) { CTRL } else { 0 }) | (if down(VK_SHIFT.0) { SHIFT } else { 0 })
        | (if down(VK_LWIN.0) || down(VK_RWIN.0) { WIN } else { 0 })
}

/// Every 30 ms while the list is open.
pub fn on_timer() {
    let a = app();
    if !a.appswitch.visible() { stop_timer(); return; }
    if logic::released(held_mods(), a.appswitch.mods) { let i = a.appswitch.selected; commit(i); }
}

fn stop_timer() { let a = app(); unsafe { let _ = KillTimer(Some(a.main), TIMER_APPSWITCH); } }

pub fn cancel() { close(); app().changed(); }

pub fn commit(index: usize) {
    let a = app();
    let item = a.appswitch.items.get(index).cloned();
    close();
    if let Some(i) = item {
        a.appswitch.switches += 1;
        raise(i.id, i.minimized);
    }
    a.changed();
}

/// Closes the list and everything it held: the window, the timer and the Esc keys.
pub fn close() {
    let a = app();
    stop_timer();
    for id in std::mem::take(&mut a.appswitch.cancel_keys) { unsafe { let _ = UnregisterHotKey(Some(a.main), id); } }
    if let Some(h) = a.appswitch.window.take() { unsafe { let _ = DestroyWindow(HWND(h as *mut _)); } }
    a.appswitch.items.clear();
}

/// "Hide Other Windows": minimizes every window on screen that is not the front app's.
pub fn hide_others() {
    let a = app();
    if a.state(ModuleId::AppSwitch) != State::Running { return; }
    let front = unsafe { let mut pid = 0u32; GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)); pid };
    if front == 0 { return; }
    for id in logic::others_to_hide(&windows_now(), front) { unsafe { let _ = ShowWindowAsync(HWND(id as *mut _), SW_MINIMIZE); } }
}

// MARK: drawing

fn rows_that_fit(work_height: f64) -> usize { (((work_height - 2.0 * PAD - 60.0) / ROW).floor() as usize).max(3) }

/// Keeps the selected row inside the part of the list that is shown.
fn reveal() {
    let a = &mut app().appswitch;
    let fit = a.items.len().min(rows_that_fit(visible_height()));
    if a.selected < a.top { a.top = a.selected; }
    if a.selected >= a.top + fit { a.top = a.selected + 1 - fit; }
}

fn visible_height() -> f64 {
    unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST), &mut mi);
        let s = app().appswitch.window.map_or(1.0, |h| dpi_scale(HWND(h as *mut _)));
        (mi.rcWork.bottom - mi.rcWork.top) as f64 / s
    }
}

fn open() {
    let a = app();
    let rows = a.appswitch.items.len().min(rows_that_fit(visible_height()));
    let h = rows as f64 * ROW + 2.0 * PAD + if a.appswitch.items.len() > rows { 22.0 } else { 0.0 };
    let win = UiWindow::create("Window Switcher", WS_POPUP, WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE, 0, 0, WIDTH, h, Box::new(draw));
    win.on_close = Some(Box::new(|| { app().appswitch.window = None; }));
    let hwnd = win.hwnd;
    a.appswitch.window = Some(hwnd.0 as isize);
    unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST), &mut mi);
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        let (w, hh) = (r.right - r.left, r.bottom - r.top);
        let work = mi.rcWork;
        let x = work.left + ((work.right - work.left) - w) / 2;
        let y = (work.top + ((work.bottom - work.top) - hh) / 2 - ((work.bottom - work.top) as f64 * 0.08) as i32).max(work.top);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        // Shown without taking the focus: the window you are in stays the one that has it until you let go.
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

/// A round badge with the app's first letter, in a colour that stays the same for the same app.
fn badge(ui: &Ui, app_name: &str, r: Rect) {
    const COLORS: [u32; 8] = [0xE8590C, 0x1C7ED6, 0x2F9E44, 0xAE3EC9, 0xC2255C, 0x0C8599, 0x5F3DC4, 0x9C5A1F];
    let hash = app_name.to_lowercase().bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    ui.c.round(r, r.h / 4.0, rgb(COLORS[hash as usize % COLORS.len()]));
    let letter: String = app_name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
    ui.c.line_text(&letter, medium(14), r, rgb(0xFFFFFF), Align::Center);
}

fn draw(ui: &mut Ui, _ctx: &mut WindowCtx) {
    let th = ui.th;
    let a = &app().appswitch;
    ui.c.fill(Rect::new(0.0, 0.0, ui.w, ui.h), th.paper);
    let fit = a.items.len().min(rows_that_fit(visible_height()));
    let mut y = PAD;
    let mut clicked = None;
    for (n, item) in a.items.iter().enumerate().skip(a.top).take(fit) {
        let r = Rect::new(PAD, y, ui.w - 2.0 * PAD, ROW);
        let resp = ui.hit(ui.id(&format!("sw{n}")), r, false);
        let selected = n == a.selected;
        if selected { ui.c.round(r.inset(0.0, 2.0), 8.0, NACHO); } else if resp.hover { ui.c.round(r.inset(0.0, 2.0), 8.0, th.paper2); }
        let ink = if selected { rgb(0x24180D) } else { th.ink };
        let sub = if selected { rgb(0x24180D).alpha(0.7) } else { th.ink2 };
        badge(ui, &item.app, Rect::new(r.x + 8.0, r.y + (ROW - 26.0) / 2.0, 26.0, 26.0));
        ui.c.line_text(&item.title, medium(13), Rect::new(r.x + 46.0, r.y + 3.0, r.w - 56.0, 20.0), ink, Align::Left);
        let app_line = if item.minimized { format!("{} · minimized", item.app) } else { item.app.clone() };
        ui.c.line_text(&app_line, sans(11), Rect::new(r.x + 46.0, r.y + 21.0, r.w - 56.0, 16.0), sub, Align::Left);
        if resp.clicked { clicked = Some(n); }
        ui.st.cursor_hand |= resp.hover;
        y += ROW;
    }
    if a.items.len() > fit {
        ui.c.line_text(&format!("{} of {}", a.selected + 1, a.items.len()), mono(10), Rect::new(0.0, ui.h - 24.0, ui.w, 20.0), th.ink2, Align::Center);
    }
    if let Some(i) = clicked { later(move || commit(i)); }
}
