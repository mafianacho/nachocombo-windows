//! Window Snap: moves other apps' windows. Everything is computed on the visible frame (DWM bounds without the
//! invisible resize border), then converted back to the outer frame `SetWindowPos` wants.
use crate::app::{app, TIMER_DRAG};
use crate::core::layout::{self, Rect, SnapAction};
use crate::core::settings::SnapRepeat;
use crate::gfx::*;
use crate::window::{dpi_scale, UiWindow};
use std::collections::HashMap;
use windows::core::BOOL;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Default)]
pub struct Snap {
    hooks: Vec<HWINEVENTHOOK>,
    /// The frame a window had before its first snap, and the frame it was snapped to.
    restore: HashMap<isize, (Rect, Rect)>,
    pub commands: u32,
    pub last_action: Option<String>,
    pub notice: Option<String>,
    drag: Option<isize>,
    preview: Option<isize>,
    preview_action: Option<SnapAction>,
    /// Windows kept on top: the window, its yellow frame, and its location hook.
    on_top: Vec<(isize, isize, HWINEVENTHOOK)>,
    /// Where windows were on each set of displays, keyed by the displays' rectangles.
    layouts: HashMap<String, Vec<(isize, Rect)>>,
    /// The displays changed at this moment: windows go back once Windows has finished moving them.
    restore_due: Option<std::time::Instant>,
    pub layouts_restored: u32,
}

impl Snap {
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "eventHooks": self.hooks.len(), "commands": self.commands, "last": self.last_action, "remembered": self.restore.len(), "notice": self.notice, "onTop": self.on_top.len(), "layouts": self.layouts.len(), "layoutsRestored": self.layouts_restored })
    }
}

pub fn start() -> Result<(), String> {
    unsafe {
        let h = SetWinEventHook(EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        if h.is_invalid() { return Err("Windows refused to report window moves".into()); }
        app().snap.hooks.push(h);
        SetTimer(Some(app().main), crate::app::TIMER_LAYOUT, 10_000, None);
    }
    layout_tick();
    Ok(())
}

pub fn stop() {
    let s = &mut app().snap;
    for h in s.hooks.drain(..) { unsafe { let _ = UnhookWinEvent(h); } }
    end_drag_visuals();
    unsafe { let _ = KillTimer(Some(app().main), crate::app::TIMER_LAYOUT); }
    for (h, _, _) in app().snap.on_top.clone() { release_on_top(h); }
    let s = &mut app().snap;
    s.layouts.clear();
    s.restore_due = None;
    s.restore.clear();
    s.drag = None;
    s.notice = None;
}

fn rect(r: RECT) -> Rect { Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64) }

pub fn monitors() -> Vec<(HMONITOR, Rect, Rect)> {
    unsafe extern "system" fn cb(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let v = &mut *(data.0 as *mut Vec<(HMONITOR, Rect, Rect)>);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(h, &mut mi).as_bool() { v.push((h, rect(mi.rcMonitor), rect(mi.rcWork))); }
        BOOL(1)
    }
    let mut v = vec![];
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize)); }
    v
}

fn visible_frame(h: HWND) -> Option<Rect> {
    unsafe {
        let mut r = RECT::default();
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, 16).is_ok() { return Some(rect(r)); }
        GetWindowRect(h, &mut r).ok()?;
        Some(rect(r))
    }
}

/// Moves so the *visible* frame lands on `t`. Returns false when Windows refused (usually an elevated window).
fn place(h: HWND, t: Rect) -> bool {
    unsafe {
        if IsZoomed(h).as_bool() || IsIconic(h).as_bool() { let _ = ShowWindow(h, SW_RESTORE); }
        let (Some(vis), mut outer) = (visible_frame(h), RECT::default()) else { return false };
        if GetWindowRect(h, &mut outer).is_err() { return false; }
        let o = rect(outer);
        let (l, top, r, b) = (vis.x - o.x, vis.y - o.y, o.right() - vis.right(), o.bottom() - vis.bottom());
        // Twice across displays with different scaling: Windows rescales on the first move.
        for _ in 0..2 {
            if SetWindowPos(h, None, (t.x - l) as i32, (t.y - top) as i32, (t.w + l + r) as i32, (t.h + top + b) as i32, SWP_NOZORDER | SWP_NOACTIVATE).is_err() { return false; }
        }
        true
    }
}

fn snappable(h: HWND) -> bool {
    unsafe {
        if h.0.is_null() || !IsWindowVisible(h).as_bool() { return false; }
        let mut name = [0u16; 64];
        let n = GetClassNameW(h, &mut name) as usize;
        let class = String::from_utf16_lossy(&name[..n]);
        if ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&class.as_str()) { return false; }
        let style = GetWindowLongW(h, GWL_STYLE) as u32;
        style & WS_THICKFRAME.0 != 0 || style & WS_CAPTION.0 == WS_CAPTION.0
    }
}

pub fn run(action: SnapAction) { run_on(unsafe { GetForegroundWindow() }, action); }

pub fn run_on(h: HWND, action: SnapAction) {
    let s = &mut app().snap;
    s.commands += 1;
    s.last_action = Some(action.id().into());
    s.notice = None;
    if !snappable(h) { return; }
    let opts = app().settings.options.clone();
    let mons = monitors();
    let Some(cur) = visible_frame(h) else { return };
    let works: Vec<Rect> = mons.iter().map(|m| m.2).collect();
    let Some(idx) = layout::screen_index(&cur, &works) else { return };
    let v = works[idx];
    let gap = opts.snap_gap as f64 * dpi_scale(h);
    let id = h.0 as isize;
    let zoomed = unsafe { IsZoomed(h).as_bool() };
    let cur_for_layout = if zoomed { None } else { Some(cur) };

    let target = match action {
        SnapAction::Restore => {
            let Some((before, _)) = s.restore.remove(&id) else { unsafe { if zoomed { let _ = ShowWindow(h, SW_RESTORE); } } return };
            if !place(h, before) { refused(); }
            return;
        }
        SnapAction::NextDisplay | SnapAction::PreviousDisplay => {
            let Some(dst) = layout::adjacent_screen(idx, &works, action == SnapAction::NextDisplay) else { return };
            if zoomed {
                unsafe { let _ = ShowWindow(h, SW_RESTORE); }
                let rf = visible_frame(h).unwrap_or(cur);
                place(h, layout::move_to(rf, v, works[dst]));
                unsafe { let _ = ShowWindow(h, SW_MAXIMIZE); }
                return;
            }
            layout::move_to(cur, v, works[dst])
        }
        // A real maximize keeps the window's own maximize button and Aero behaviour. With a gap it is a plain frame.
        SnapAction::Maximize if gap == 0.0 => {
            s.restore.entry(id).or_insert((cur, cur));
            unsafe { let _ = ShowWindow(h, SW_MAXIMIZE); }
            return;
        }
        a if a.cycles() => {
            let plain = layout::target(a, v, None, gap, false);
            let repeated = cur_for_layout.is_some_and(|c| layout::matches(&c, &plain));
            match opts.snap_repeat {
                SnapRepeat::CycleSizes => layout::target(a, v, cur_for_layout, gap, true),
                SnapRepeat::Nothing if repeated => return,
                SnapRepeat::NextDisplay if repeated && matches!(a, SnapAction::Left | SnapAction::Right) => {
                    let Some(dst) = layout::adjacent_screen(idx, &works, a == SnapAction::Right) else { return };
                    let opposite = if a == SnapAction::Left { SnapAction::Right } else { SnapAction::Left };
                    layout::target(opposite, works[dst], None, gap, false)
                }
                _ => plain,
            }
        }
        a => layout::target(a, v, cur_for_layout, gap, false),
    };
    let snap = !matches!(action, SnapAction::NextDisplay | SnapAction::PreviousDisplay);
    if snap { let e = s.restore.entry(id).or_insert((cur, target)); e.1 = target; }
    if !place(h, target) { refused(); }
}

fn refused() {
    let a = app();
    a.snap.notice = Some("Windows refused to move that window. It probably runs as administrator.".into());
    a.changed();
}

fn windows_snap_on() -> bool {
    unsafe {
        let mut v = BOOL(0);
        SystemParametersInfoW(SPI_GETWINARRANGING, 0, Some(&mut v as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_ok() && v.as_bool()
    }
}

/// True while Windows' own Snap handles edge drags.
pub fn edge_drags_left_to_windows() -> bool { windows_snap_on() }

unsafe extern "system" fn on_event(_: HWINEVENTHOOK, ev: u32, h: HWND, obj: i32, _: i32, _: u32, _: u32) {
    if obj != OBJID_WINDOW.0 { return; }
    let a = app();
    if ev == EVENT_SYSTEM_MOVESIZESTART {
        a.snap.drag = Some(h.0 as isize);
        a.snap.preview_action = None;
        if a.settings.options.snap_drag_to_edges && !windows_snap_on() { let _ = SetTimer(Some(a.main), TIMER_DRAG, 30, None); }
    } else if ev == EVENT_SYSTEM_MOVESIZEEND {
        let _ = KillTimer(Some(a.main), TIMER_DRAG);
        let act = a.snap.preview_action.take();
        end_drag_visuals();
        a.snap.drag = None;
        if let Some(act) = act { run_on(h, act); return; }
        restore_after_drag(h);
    }
}

/// A snapped window dragged away gets its earlier size back, where it was dropped.
fn restore_after_drag(h: HWND) {
    let a = app();
    if !a.settings.options.snap_restore_on_drag { return; }
    let id = h.0 as isize;
    let Some((before, snapped)) = a.snap.restore.get(&id).copied() else { return };
    let Some(cur) = visible_frame(h) else { return };
    // A resize or a window that did not move is left alone.
    if (cur.w - snapped.w).abs() > 4.0 || (cur.h - snapped.h).abs() > 4.0 { a.snap.restore.remove(&id); return; }
    if (cur.x - snapped.x).abs() < 2.0 && (cur.y - snapped.y).abs() < 2.0 { return; }
    a.snap.restore.remove(&id);
    let mut p = POINT::default();
    unsafe { let _ = GetCursorPos(&mut p); }
    // Keeps the pointer over the title bar at the same share of the width.
    let share = ((p.x as f64 - cur.x) / cur.w.max(1.0)).clamp(0.0, 1.0);
    place(h, Rect::new(p.x as f64 - before.w * share, cur.y, before.w, before.h));
}

pub fn drag_tick() {
    let a = app();
    let Some(id) = a.snap.drag else { return };
    let h = HWND(id as *mut _);
    let mut p = POINT::default();
    unsafe { let _ = GetCursorPos(&mut p); }
    let mons = monitors();
    let scale = dpi_scale(h);
    let hit = mons.iter().find(|m| m.1.inset(-1.0, -1.0).contains(p.x as f64, p.y as f64));
    let act = hit.and_then(|m| layout::zone_action(p.x as f64, p.y as f64, m.1, scale));
    if act == a.snap.preview_action { return; }
    a.snap.preview_action = act;
    match (act, hit) {
        (Some(act), Some(m)) => {
            let gap = a.settings.options.snap_gap as f64 * scale;
            let t = layout::target(act, m.2, None, gap, false);
            show_preview(t);
        }
        _ => end_drag_visuals(),
    }
}

fn show_preview(t: Rect) {
    unsafe {
        let a = app();
        let h = match a.snap.preview {
            Some(id) => HWND(id as *mut _),
            None => {
                let win = UiWindow::create("Snap Preview", WS_POPUP, WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, 0, 0, 10.0, 10.0,
                    Box::new(|ui, _| { let r = Rect::new(0.0, 0.0, ui.w, ui.h); ui.c.clear(NACHO.mix(Color(1.0, 1.0, 1.0, 1.0), 0.4)); ui.c.stroke_round(r.inset(1.0, 1.0), 8.0, NACHO, 3.0); }));
                let _ = SetLayeredWindowAttributes(win.hwnd, COLORREF(0), 90, LWA_ALPHA);
                a.snap.preview = Some(win.hwnd.0 as isize);
                win.hwnd
            }
        };
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), t.x as i32, t.y as i32, t.w as i32, t.h as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

fn end_drag_visuals() {
    if let Some(id) = app().snap.preview.take() { unsafe { let _ = DestroyWindow(HWND(id as *mut _)); } }
}

// ---------- keep on top ----------

const FRAME_KEY: COLORREF = COLORREF(0x00FF00FF);

/// Win+Ctrl+T: the front window stays above all others until pressed again. A yellow frame shows which one.
pub fn toggle_on_top() {
    let h = unsafe { GetForegroundWindow() };
    if !snappable(h) { return; }
    let id = h.0 as isize;
    if app().snap.on_top.iter().any(|x| x.0 == id) { release_on_top(id); return; }
    unsafe { let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
    let win = UiWindow::create("On Top", WS_POPUP, WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, 0, 0, 10.0, 10.0,
        Box::new(|ui, _| { ui.c.clear(Color(1.0, 0.0, 1.0, 1.0)); ui.c.stroke_round(Rect::new(0.0, 0.0, ui.w, ui.h), 9.0, NACHO, 3.0); }));
    let frame = win.hwnd;
    win.clear = Some(Color(1.0, 0.0, 1.0, 1.0));
    unsafe {
        let _ = SetLayeredWindowAttributes(frame, FRAME_KEY, 0, LWA_COLORKEY);
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let hook = SetWinEventHook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE, None, Some(on_top_moved), pid, 0, WINEVENT_OUTOFCONTEXT);
        let hook2 = SetWinEventHook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY, None, Some(on_top_moved), pid, 0, WINEVENT_OUTOFCONTEXT);
        let _ = hook2;
        app().snap.on_top.push((id, frame.0 as isize, hook));
    }
    follow(id);
    app().changed();
}

fn release_on_top(id: isize) {
    let s = &mut app().snap;
    let Some(i) = s.on_top.iter().position(|x| x.0 == id) else { return };
    let (h, frame, hook) = s.on_top.remove(i);
    unsafe {
        let _ = UnhookWinEvent(hook);
        let _ = DestroyWindow(HWND(frame as *mut _));
        let h = HWND(h as *mut _);
        if IsWindow(Some(h)).as_bool() { let _ = SetWindowPos(h, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
    }
    app().changed();
}

/// Puts the frame around the window, or hides it while the window is minimized.
fn follow(id: isize) {
    let Some((_, frame, _)) = app().snap.on_top.iter().find(|x| x.0 == id).copied() else { return };
    let (h, f) = (HWND(id as *mut _), HWND(frame as *mut _));
    unsafe {
        if !IsWindow(Some(h)).as_bool() { release_on_top(id); return; }
        if IsIconic(h).as_bool() || !IsWindowVisible(h).as_bool() { let _ = ShowWindow(f, SW_HIDE); return; }
        let Some(v) = visible_frame(h) else { return };
        let pad = 3.0 * dpi_scale(h);
        let r = v.inset(-pad, -pad);
        let _ = SetWindowPos(f, Some(HWND_TOPMOST), r.x as i32, r.y as i32, r.w as i32, r.h as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

unsafe extern "system" fn on_top_moved(_: HWINEVENTHOOK, _: u32, h: HWND, obj: i32, _: i32, _: u32, _: u32) {
    if obj != OBJID_WINDOW.0 { return; }
    let id = h.0 as isize;
    if app().snap.on_top.iter().any(|x| x.0 == id) { follow(id); }
}

// ---------- layouts per set of displays ----------

fn display_key() -> String {
    let mut v: Vec<String> = monitors().iter().map(|m| format!("{},{},{},{}", m.1.x, m.1.y, m.1.w, m.1.h)).collect();
    v.sort();
    v.join(";")
}

/// Normal windows a user arranges: visible, not minimized or maximized, not tool windows.
fn arranged_windows() -> Vec<(isize, Rect)> {
    let mut out = vec![];
    unsafe {
        let mut cur = GetTopWindow(None).unwrap_or_default();
        while !cur.0.is_null() {
            let ex = GetWindowLongW(cur, GWL_EXSTYLE) as u32;
            if snappable(cur) && ex & WS_EX_TOOLWINDOW.0 == 0 && !IsIconic(cur).as_bool() && GetWindow(cur, GW_OWNER).is_err_or_null() {
                if let Some(r) = visible_frame(cur) { out.push((cur.0 as isize, r)); }
            }
            cur = GetWindow(cur, GW_HWNDNEXT).unwrap_or_default();
        }
    }
    out
}

trait NullOrErr { fn is_err_or_null(&self) -> bool; }
impl NullOrErr for windows::core::Result<HWND> { fn is_err_or_null(&self) -> bool { self.as_ref().map_or(true, |h| h.0.is_null()) } }

/// Every 10 s: remembers the arrangement for the current displays, or puts one back after a change.
pub fn layout_tick() {
    let a = app();
    if a.state(crate::core::settings::ModuleId::WindowSnap) != crate::app::State::Running || !a.settings.options.snap_restore_layouts { return; }
    if let Some(at) = a.snap.restore_due {
        if at.elapsed() < std::time::Duration::from_millis(2500) { return; }
        a.snap.restore_due = None;
        unsafe { SetTimer(Some(a.main), crate::app::TIMER_LAYOUT, 10_000, None); }
        if let Some(saved) = a.snap.layouts.get(&display_key()).cloned() {
            let mut moved = 0;
            for (id, r) in saved {
                let h = HWND(id as *mut _);
                unsafe { if !IsWindow(Some(h)).as_bool() || IsZoomed(h).as_bool() || IsIconic(h).as_bool() { continue; } }
                if visible_frame(h).is_some_and(|cur| !layout::matches(&cur, &r)) && place(h, r) { moved += 1; }
            }
            if moved > 0 { a.snap.layouts_restored += 1; a.tray.balloon("Window Snap", &format!("{moved} windows put back for these displays")); }
        }
        return;
    }
    let key = display_key();
    a.snap.layouts.insert(key, arranged_windows());
}

/// Windows moves windows itself right after a display change; ours wait until it has finished.
pub fn displays_changed() {
    let a = app();
    if a.state(crate::core::settings::ModuleId::WindowSnap) != crate::app::State::Running || !a.settings.options.snap_restore_layouts { return; }
    a.snap.restore_due = Some(std::time::Instant::now());
    unsafe { SetTimer(Some(a.main), crate::app::TIMER_LAYOUT, 1000, None); }
}
