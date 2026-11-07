//! Focus Dim: a click-through black window per display, kept directly below the active window in the z-order.
use crate::app::{app, TIMER_DIM, TIMER_FADE};
use crate::core::layout::{self, Rect};
use crate::core::settings::*;
use crate::snap::monitors;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS: PCWSTR = w!("NachoComboDim");

struct Overlay { screen: Rect, current: HWND, spare: HWND, alpha_cur: f64, alpha_spare: f64 }

#[derive(Default)]
pub struct Dim {
    overlays: Vec<Overlay>,
    hooks: Vec<HWINEVENTHOOK>,
    pub paused: bool,
    anchor: isize,
    screen: Option<usize>,
    fade_start: Option<std::time::Instant>,
    fade_len: f64,
    pub refreshes: u32,
    pub reorders: u32,
}

impl Dim {
    pub fn diagnostics(&self) -> serde_json::Value {
        let visible = self.overlays.iter().filter(|o| unsafe { IsWindowVisible(o.current).as_bool() }).count();
        serde_json::json!({ "overlays": self.overlays.len(), "visible": visible, "eventHooks": self.hooks.len(), "paused": self.paused, "refreshes": self.refreshes, "reorders": self.reorders })
    }
}

unsafe fn make_window() -> Result<HWND> {
    let hinst = GetModuleHandleW(None)?;
    let h = CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, CLASS, w!("Focus Dim"), WS_POPUP, 0, 0, 1, 1, None, None, Some(hinst.into()), None)?;
    let _ = SetLayeredWindowAttributes(h, COLORREF(0), 0, LWA_ALPHA);
    // Windows' own "peek" and Alt-Tab must not treat the shade as a window.
    let exclude = BOOL(1);
    let _ = DwmSetWindowAttribute(h, DWMWA_EXCLUDED_FROM_PEEK, &exclude as *const _ as *const _, 4);
    Ok(h)
}

fn build() -> Result<()> {
    let d = &mut app().dim;
    unsafe {
        for (_, full, _) in monitors() {
            d.overlays.push(Overlay { screen: full, current: make_window()?, spare: make_window()?, alpha_cur: 0.0, alpha_spare: 0.0 });
        }
    }
    Ok(())
}

fn destroy_overlays() {
    for o in app().dim.overlays.drain(..) { unsafe { let _ = DestroyWindow(o.current); let _ = DestroyWindow(o.spare); } }
}

pub fn start() -> std::result::Result<(), String> {
    unsafe {
        let hinst = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(plain_proc), hInstance: hinst.into(), lpszClassName: CLASS,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0), ..Default::default() };
        RegisterClassExW(&wc);
    }
    build().map_err(|e| e.to_string())?;
    let a = app();
    a.dim.paused = false;
    unsafe {
        for (lo, hi) in [(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND), (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND), (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND)] {
            let h = SetWinEventHook(lo, hi, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT);
            if !h.is_invalid() { a.dim.hooks.push(h); }
        }
        // Catches what no event reports, such as another window pushed between the shade and the active window.
        SetTimer(Some(a.main), TIMER_DIM, 500, None);
    }
    refresh();
    Ok(())
}

pub fn stop() {
    let a = app();
    unsafe {
        let _ = KillTimer(Some(a.main), TIMER_DIM);
        let _ = KillTimer(Some(a.main), TIMER_FADE);
    }
    for h in a.dim.hooks.drain(..) { unsafe { let _ = UnhookWinEvent(h); } }
    destroy_overlays();
    let d = &mut app().dim;
    d.anchor = 0;
    d.screen = None;
    d.paused = false;
    d.fade_start = None;
}

unsafe extern "system" fn plain_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT { DefWindowProcW(h, m, w, l) }

unsafe extern "system" fn on_event(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) { refresh(); }

pub fn toggle_pause() {
    let d = &mut app().dim;
    if d.overlays.is_empty() { return; }
    d.paused = !d.paused;
    if d.paused { hide_all(); } else { refresh(); }
    app().changed();
}

pub fn options_changed() {
    let d = &mut app().dim;
    if d.overlays.is_empty() { return; }
    let target = app().settings.dim_intensity;
    let d = &mut app().dim;
    d.fade_start = None;
    for o in &mut d.overlays { if o.alpha_cur > 0.0 { o.alpha_cur = target; set_alpha(o.current, target); } }
    d.anchor = 0;
    refresh();
}

pub fn displays_changed() {
    if app().dim.overlays.is_empty() { return; }
    destroy_overlays();
    let _ = build();
    let d = &mut app().dim;
    d.anchor = 0;
    d.fade_start = None;
    refresh();
}

fn set_alpha(h: HWND, a: f64) { unsafe { let _ = SetLayeredWindowAttributes(h, COLORREF(0), (a.clamp(0.0, 1.0) * 255.0) as u8, LWA_ALPHA); } }

fn hide_all() {
    let d = &mut app().dim;
    d.fade_start = None;
    d.anchor = 0;
    d.screen = None;
    for o in &mut d.overlays {
        unsafe { let _ = ShowWindow(o.current, SW_HIDE); let _ = ShowWindow(o.spare, SW_HIDE); }
        (o.alpha_cur, o.alpha_spare) = (0.0, 0.0);
    }
}

fn class_of(h: HWND) -> String {
    let mut name = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut name) } as usize;
    String::from_utf16_lossy(&name[..n])
}

fn cloaked(h: HWND) -> bool {
    let mut v = 0u32;
    unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut _ as *mut _, 4).is_ok() && v != 0 }
}

/// The window behind which the shade goes: the active one, or the backmost visible window of its app.
fn anchor_for(front: HWND, scope: DimScope) -> HWND {
    if scope == DimScope::FrontWindow { return front; }
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(front, Some(&mut pid));
        let (mut best, mut cur) = (front, front);
        loop {
            let Ok(next) = GetWindow(cur, GW_HWNDNEXT) else { return best };
            cur = next;
            let mut p = 0;
            GetWindowThreadProcessId(cur, Some(&mut p));
            if p == pid && IsWindowVisible(cur).as_bool() && !IsIconic(cur).as_bool() && !cloaked(cur) { best = cur; }
        }
    }
}

fn reduced_motion() -> bool {
    let mut on = BOOL(1);
    unsafe { let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    !on.as_bool()
}

pub fn refresh() {
    let a = app();
    if a.dim.overlays.is_empty() || a.dim.paused { return; }
    a.dim.refreshes += 1;
    let front = unsafe { GetForegroundWindow() };
    if front.0.is_null() { hide_all(); return; }
    let class = class_of(front);
    if class == "Progman" || class == "WorkerW" { hide_all(); return; }
    // The taskbar, Start, Alt-Tab and tray menus come and go: the shade stays as it is behind them.
    const SHELL: [&str; 9] = ["Shell_TrayWnd", "Shell_SecondaryTrayWnd", "NotifyIconOverflowWindow", "TopLevelWindowForOverflowXamlIsland", "XamlExplorerHostIslandWindow", "Windows.UI.Core.CoreWindow", "#32768", "ForegroundStaging", "MultitaskingViewFrame"];
    if SHELL.contains(&class.as_str()) || class == "NachoComboDim" { return; }
    unsafe { if !IsWindowVisible(front).as_bool() || IsIconic(front).as_bool() || cloaked(front) { hide_all(); return; } }
    let opts = a.settings.options.clone();
    let target = a.settings.dim_intensity;
    let anchor = anchor_for(front, opts.dim_scope);
    let mut fr = RECT::default();
    unsafe { let _ = GetWindowRect(front, &mut fr); }
    let screens: Vec<Rect> = a.dim.overlays.iter().map(|o| o.screen).collect();
    let screen = layout::screen_index(&Rect::new(fr.left as f64, fr.top as f64, (fr.right - fr.left) as f64, (fr.bottom - fr.top) as f64), &screens);
    let d = &mut a.dim;
    let id = anchor.0 as isize;
    let changed = id != d.anchor;
    let wanted = |i: usize| opts.dim_other_displays || Some(i) == screen;
    let in_place = unsafe {
        let next = GetWindow(anchor, GW_HWNDNEXT).unwrap_or_default();
        d.overlays.iter().enumerate().all(|(i, o)| !wanted(i) || IsWindowVisible(o.current).as_bool())
            && d.overlays.iter().any(|o| o.current == next || o.spare == next)
    };
    if !changed && in_place && screen == d.screen { return; }
    d.anchor = id;
    d.screen = screen;
    d.reorders += 1;
    let fade = if opts.dim_fade_seconds > 0.0 && changed && !reduced_motion() { opts.dim_fade_seconds } else { 0.0 };
    let anchor_top = unsafe { GetWindowLongW(anchor, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0 };
    for (i, o) in d.overlays.iter_mut().enumerate() {
        unsafe {
            if !wanted(i) {
                let _ = ShowWindow(o.current, SW_HIDE);
                let _ = ShowWindow(o.spare, SW_HIDE);
                (o.alpha_cur, o.alpha_spare) = (0.0, 0.0);
                continue;
            }
            if fade > 0.0 && IsWindowVisible(o.current).as_bool() {
                // Cross-fade: the spare appears below the new window while the old shade fades out.
                std::mem::swap(&mut o.current, &mut o.spare);
                std::mem::swap(&mut o.alpha_cur, &mut o.alpha_spare);
            }
            let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;
            // Leaving the topmost band needs an explicit step: an insert-after alone does not do it.
            if !anchor_top { let _ = SetWindowPos(o.current, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
            let _ = SetWindowPos(o.current, Some(anchor), o.screen.x as i32, o.screen.y as i32, o.screen.w as i32, o.screen.h as i32, flags);
            if fade == 0.0 {
                let _ = ShowWindow(o.spare, SW_HIDE);
                (o.alpha_cur, o.alpha_spare) = (target, 0.0);
                set_alpha(o.current, target);
            }
        }
    }
    if fade > 0.0 {
        d.fade_start = Some(std::time::Instant::now());
        d.fade_len = fade;
        unsafe { SetTimer(Some(a.main), TIMER_FADE, 16, None); }
    }
}

pub fn fade_tick() {
    let a = app();
    let target = a.settings.dim_intensity;
    let d = &mut a.dim;
    let Some(start) = d.fade_start else { unsafe { let _ = KillTimer(Some(a.main), TIMER_FADE); } return };
    let t = (start.elapsed().as_secs_f64() / d.fade_len.max(0.01)).min(1.0);
    for o in &mut d.overlays {
        unsafe {
            if !IsWindowVisible(o.current).as_bool() { continue; }
            o.alpha_cur = target * t;
            set_alpha(o.current, o.alpha_cur);
            set_alpha(o.spare, o.alpha_spare * (1.0 - t));
            if t >= 1.0 { o.alpha_spare = 0.0; let _ = ShowWindow(o.spare, SW_HIDE); }
        }
    }
    if t >= 1.0 { d.fade_start = None; unsafe { let _ = KillTimer(Some(a.main), TIMER_FADE); } }
}
