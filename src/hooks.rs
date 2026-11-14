//! The low-level mouse hook, on its own thread so a busy UI thread never delays the pointer. It only runs while a gadget
//! needs it: Window Snap's Ctrl+Alt-drag and File Shelf's drag detection.
use crate::app::{app, State, WM_ANYDRAG, WM_SHAKE};
use crate::core::settings::*;
use crate::core::shelf::Shake;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering::*};
use std::sync::Mutex;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static MOTION: AtomicBool = AtomicBool::new(false);
static SHAKE: AtomicBool = AtomicBool::new(false);
static ANY_DRAG: AtomicBool = AtomicBool::new(false);
static PEEK: AtomicBool = AtomicBool::new(false);
static REVERSE: AtomicBool = AtomicBool::new(false);
static FLIPS: AtomicU32 = AtomicU32::new(0);
/// Marks the wheel events this app sends itself, so its own hook lets them through.
const MARKER: usize = 0x4E43_4D54;
static MAIN: AtomicIsize = AtomicIsize::new(0);
static THREAD: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
struct Motion { hwnd: isize, resize: bool, start: POINT, frame: RECT, corner: (bool, bool) }

struct HookState { motion: Option<Motion>, left_down: bool, shake: Shake, drag_posted: bool, last_check: u32 }
static STATE: Mutex<Option<HookState>> = Mutex::new(None);

pub fn wheel_flips() -> u32 { FLIPS.load(Relaxed) }

pub fn active() -> bool { THREAD.load(Relaxed) != 0 }

/// Starts or stops the hook to match what the running gadgets need.
pub fn sync() {
    let a = app();
    let o = &a.settings.options;
    let motion = a.state(ModuleId::WindowSnap) == State::Running && o.window_motion;
    let shelf_on = a.state(ModuleId::FileShelf) == State::Running;
    let shake = shelf_on && o.shelf_trigger == ShelfTrigger::Shake;
    let any = shelf_on && o.shelf_trigger == ShelfTrigger::AnyDrag;
    let peek = shelf_on && o.explorer_preview;
    let reverse = a.state(ModuleId::MouseTune) == State::Running && o.mouse_reverse;
    REVERSE.store(reverse, Relaxed);
    PEEK.store(peek, Relaxed);
    MOTION.store(motion, Relaxed);
    SHAKE.store(shake, Relaxed);
    ANY_DRAG.store(any, Relaxed);
    MAIN.store(a.main.0 as isize, Relaxed);
    let need = motion || shake || any || peek || reverse;
    let running = THREAD.load(Relaxed);
    if need && running == 0 {
        std::thread::spawn(thread_main);
        // The thread id is needed to stop it again; it publishes it within a moment.
        while THREAD.load(Acquire) == 0 { std::thread::yield_now(); }
    } else if !need && running != 0 {
        unsafe { let _ = PostThreadMessageW(running, WM_QUIT, WPARAM(0), LPARAM(0)); }
        THREAD.store(0, Release);
    }
}

fn thread_main() {
    unsafe {
        *STATE.lock().unwrap() = Some(HookState { motion: None, left_down: false, shake: Shake::new(40), drag_posted: false, last_check: 0 });
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0);
        let keys = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), None, 0);
        // Forces the thread's message queue into existence before the id is published.
        let mut m = MSG::default();
        let _ = PeekMessageW(&mut m, None, 0, 0, PM_NOREMOVE);
        THREAD.store(windows::Win32::System::Threading::GetCurrentThreadId(), Release);
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        if let Ok(h) = hook { let _ = UnhookWindowsHookEx(h); }
        if let Ok(h) = keys { let _ = UnhookWindowsHookEx(h); }
    }
}

/// Space alone, while a File Explorer file list has the focus, opens the preview instead of reaching Explorer.
unsafe extern "system" fn key_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && PEEK.load(Relaxed) && wp.0 as u32 == WM_KEYDOWN {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        let plain = !key_down(VK_CONTROL) && !key_down(VK_MENU) && !key_down(VK_SHIFT) && !key_down(VK_LWIN) && !key_down(VK_RWIN);
        if k.vkCode == 0x20 && plain && k.flags.0 & LLKHF_INJECTED.0 == 0 && crate::peek::explorer_list_focused() {
            let _ = PostMessageW(Some(HWND(MAIN.load(Relaxed) as *mut _)), crate::app::WM_PEEK, WPARAM(0), LPARAM(0));
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

fn key_down(k: VIRTUAL_KEY) -> bool { unsafe { GetAsyncKeyState(k.0 as i32) < 0 } }

/// Windows must stay out of Ctrl+Alt-drag: the shell, and anything that is not a normal top-level window.
unsafe fn movable_at(p: POINT) -> Option<HWND> {
    let h = GetAncestor(WindowFromPoint(p), GA_ROOT);
    if h.0.is_null() { return None; }
    let mut name = [0u16; 64];
    let n = GetClassNameW(h, &mut name) as usize;
    let class = String::from_utf16_lossy(&name[..n]);
    if ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&class.as_str()) { return None; }
    if IsZoomed(h).as_bool() { return None; }
    Some(h)
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code < 0 { return CallNextHookEx(None, code, wp, lp); }
    let info = &*(lp.0 as *const MSLLHOOKSTRUCT);
    let msg = wp.0 as u32;
    if (msg == WM_MOUSEWHEEL || msg == WM_MOUSEHWHEEL) && REVERSE.load(Relaxed) && info.dwExtraInfo != MARKER {
        let delta = (info.mouseData >> 16) as i16 as i32;
        if crate::core::mouse::is_notch(delta) && send_wheel(msg == WM_MOUSEHWHEEL, crate::core::mouse::wheel_delta(delta, true)) {
            FLIPS.fetch_add(1, Relaxed);
            return LRESULT(1);
        }
    }
    // Injected events pass through untouched except for tests of the gadget itself, which inject on purpose.
    let mut guard = STATE.lock().unwrap();
    let Some(st) = guard.as_mut() else { return CallNextHookEx(None, code, wp, lp) };
    let mut swallow = false;

    match msg {
        WM_LBUTTONDOWN | WM_RBUTTONDOWN if MOTION.load(Relaxed) && key_down(VK_CONTROL) && key_down(VK_MENU) && !key_down(VK_LWIN) => {
            if let Some(h) = movable_at(info.pt) {
                let mut r = RECT::default();
                let _ = GetWindowRect(h, &mut r);
                let corner = (info.pt.x > (r.left + r.right) / 2, info.pt.y > (r.top + r.bottom) / 2);
                st.motion = Some(Motion { hwnd: h.0 as isize, resize: msg == WM_RBUTTONDOWN, start: info.pt, frame: r, corner });
                swallow = true;
            }
        }
        WM_MOUSEMOVE if st.motion.is_some() => {
            let m = st.motion.unwrap();
            let (dx, dy) = (info.pt.x - m.start.x, info.pt.y - m.start.y);
            let f = m.frame;
            let (mut l, mut t, mut r, mut b) = (f.left, f.top, f.right, f.bottom);
            if m.resize {
                if m.corner.0 { r += dx } else { l += dx }
                if m.corner.1 { b += dy } else { t += dy }
                if r - l < 120 { if m.corner.0 { r = l + 120 } else { l = r - 120 } }
                if b - t < 60 { if m.corner.1 { b = t + 60 } else { t = b - 60 } }
            } else {
                (l, r, t, b) = (l + dx, r + dx, t + dy, b + dy);
            }
            let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | if m.resize { SET_WINDOW_POS_FLAGS(0) } else { SWP_NOSIZE };
            let _ = SetWindowPos(HWND(m.hwnd as *mut _), None, l, t, r - l, b - t, flags);
        }
        WM_LBUTTONUP | WM_RBUTTONUP if st.motion.is_some() => {
            st.motion = None;
            swallow = true;
        }
        _ => {}
    }

    if !swallow {
        match msg {
            WM_LBUTTONDOWN => { st.left_down = true; st.shake.reset(); st.drag_posted = false; }
            WM_LBUTTONUP => st.left_down = false,
            WM_MOUSEMOVE if st.left_down => {
                let main = HWND(MAIN.load(Relaxed) as *mut _);
                if SHAKE.load(Relaxed) && st.shake.feed(info.pt.x, info.time) && drag_image_visible() {
                    let _ = PostMessageW(Some(main), WM_SHAKE, WPARAM(0), LPARAM(0));
                }
                if ANY_DRAG.load(Relaxed) && !st.drag_posted && info.time.wrapping_sub(st.last_check) > 100 {
                    st.last_check = info.time;
                    if drag_image_visible() {
                        st.drag_posted = true;
                        let _ = PostMessageW(Some(main), WM_ANYDRAG, WPARAM(0), LPARAM(0));
                    }
                }
            }
            _ => {}
        }
    }
    drop(guard);
    if swallow { return LRESULT(1); }
    CallNextHookEx(None, code, wp, lp)
}

/// The wheel step again, the other way round, marked as ours. False when Windows did not take it, so the original goes on.
unsafe fn send_wheel(horizontal: bool, delta: i32) -> bool {
    let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT {
        dx: 0, dy: 0, mouseData: delta as u32, dwFlags: if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL }, time: 0, dwExtraInfo: MARKER } } };
    SendInput(&[input], std::mem::size_of::<INPUT>() as i32) == 1
}

/// Windows has no "a drag is in progress" signal, but its drag helper shows this window during shell-style drags.
unsafe fn drag_image_visible() -> bool {
    FindWindowW(w!("SysDragImage"), None).is_ok_and(|h| IsWindowVisible(h).as_bool())
}
