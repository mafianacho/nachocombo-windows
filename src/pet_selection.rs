//! The selection bar on Windows (Mac: `PetSelectionBar.swift`, `NachoPetModule+Selection.swift`): text selected in any
//! other app -> a small bar next to it -> the pet's chat. What is read, what is sent and when the bar may show is decided
//! in `core::pet::selection` and `core::pet::selection_watch` (tested on any OS); this file only does the Windows part:
//! the mouse and key hooks that say when a selection may have ended, the UI Automation read (with the copy fallback and
//! the clipboard given back), the bar window and the menu.
//!
//! Everything runs only while the pet is on and the bar is switched on; switching either off ends the hooks, drops the
//! bar and the waiting timer. Nothing here logs the selected text.
use crate::app::{app, later};
use crate::core::layout::Rect;
use crate::core::pet::clock;
use crate::core::pet::design_tokens::pet_tokens::{radius as rad, size as tok};
use crate::core::pet::transcript::PetText;
use crate::core::pet::selection::*;
use crate::core::pet::selection_watch::*;
use crate::gfx::*;
use crate::i18n::t;
use crate::pet_theme;
use crate::pet_ui::{self as pet, Event};
use crate::ui::*;
use crate::window::{UiWindow, WindowCtx};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::*};
use std::sync::{Arc, Weak};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const T_SELECT: usize = 4;

// ---------- state ----------

/// What the bar keeps between frames. Lives in `PetUi`, so it ends with the pet.
#[derive(Default)]
pub struct SelState {
    pub flow: Option<Arc<PetSelectionFlow>>,
    /// Keeps the session link alive (the flow holds it weakly).
    sender: Option<Arc<dyn PetSelectionSending>>,
    /// Turn ids whose 보낸 내용 보기 is open.
    pub expanded: BTreeSet<String>,
    watcher: SelectionWatcher,
    bar: Option<isize>,
    bar_mode: Option<bool>,
    anchor: Option<Rect>,
    /// Bumped by every dismissal and every new look, so a slow read that finishes late is dropped.
    generation: u64,
    reading: bool,
    /// The bar is being closed by the app itself (not by a click elsewhere): its close handler must not dismiss the flow.
    closing: bool,
    timer_on: bool,
    hooks_on: bool,
    question: String,
}

fn st() -> &'static mut SelState { &mut app().pet.sel }

fn settings() -> PetSelectionBarSettings { app().pet.settings.selection_bar.clone() }

/// Sends through whichever session is current (the session is replaced when the server changes).
struct ToSession;

impl PetSelectionSending for ToSession {
    fn send_selection(&self, sent: PetSelectionSent, completion: PetSelectionCompletion) {
        match pet::session() {
            Some(s) => s.send_selection(sent, completion),
            None => completion(Some("보낼 수 없어요.".into())),
        }
    }
}

// ---------- start, stop ----------

/// With the pet.
pub fn start() {
    let s = st();
    let sender: Arc<dyn PetSelectionSending> = Arc::new(ToSession);
    let flow = Arc::new(PetSelectionFlow::new(
        Some(Arc::downgrade(&sender) as Weak<dyn PetSelectionSending>),
        crate::capture::copy_text,
        |url| pet::push(Event::OpenUrl(url.to_string())),
    ));
    flow.set_on_change(|| pet::push(Event::SelChanged));
    s.sender = Some(sender);
    s.flow = Some(flow);
    refresh();
}

/// With the pet: nothing stays behind.
pub fn stop() {
    let s = st();
    stop_hooks();
    kill_timer();
    close_bar();
    s.generation += 1;
    s.flow = None;
    s.sender = None;
}

/// Hooks only while the pet is on and the bar is switched on (the same rule the Mac's global monitors follow).
pub fn refresh() {
    let s = st();
    let wanted = pet::is_running() && s.flow.is_some() && app().pet.settings.selection_bar.enabled;
    if wanted { start_hooks(); } else {
        stop_hooks();
        kill_timer();
        s.generation += 1;
        if let Some(f) = &s.flow { f.dismiss(); }
        close_bar();
    }
}

/// The user switched the bar on (the first-use card or Settings).
pub fn switched_on() { refresh(); }

// ---------- hooks ----------

static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
static HOOKS_RUN: AtomicBool = AtomicBool::new(false);

fn start_hooks() {
    if st().hooks_on { return; }
    st().hooks_on = true;
    HOOKS_RUN.store(true, Relaxed);
    HOOK_THREAD.store(0, Release);
    std::thread::spawn(hook_thread);
    // The id is needed to stop the thread again; it publishes it within a moment.
    let t0 = std::time::Instant::now();
    while HOOK_THREAD.load(Acquire) == 0 && t0.elapsed().as_millis() < 2000 { std::thread::yield_now(); }
}

fn stop_hooks() {
    let s = st();
    if !s.hooks_on { return; }
    s.hooks_on = false;
    HOOKS_RUN.store(false, Relaxed);
    let id = HOOK_THREAD.swap(0, AcqRel);
    if id != 0 { unsafe { let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0)); } }
}

fn hook_thread() {
    unsafe {
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0);
        let keys = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), None, 0);
        let mut m = MSG::default();
        let _ = PeekMessageW(&mut m, None, 0, 0, PM_NOREMOVE);
        HOOK_THREAD.store(GetCurrentThreadId(), Release);
        // Stopped before the thread got going.
        if !HOOKS_RUN.load(Relaxed) { PostQuitMessage(0); }
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        if let Ok(h) = mouse { let _ = UnhookWindowsHookEx(h); }
        if let Ok(h) = keys { let _ = UnhookWindowsHookEx(h); }
    }
}

fn key_down(k: VIRTUAL_KEY) -> bool { unsafe { GetAsyncKeyState(k.0 as i32) < 0 } }

/// Only positions, clicks and the kind of a key leave the hook: which letter was typed never does.
unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && HOOKS_RUN.load(Relaxed) {
        let info = &*(lp.0 as *const MSLLHOOKSTRUCT);
        // Events this app sends itself (the copy fallback) are not the user's.
        if info.flags & LLMHF_INJECTED == 0 {
            let ms = GetTickCount64();
            let ev = match wp.0 as u32 {
                WM_LBUTTONDOWN => Some(WatchEvent::LeftDown { x: info.pt.x, y: info.pt.y, ms }),
                WM_LBUTTONUP => Some(WatchEvent::LeftUp { x: info.pt.x, y: info.pt.y, ms, ctrl: key_down(VK_CONTROL) }),
                WM_RBUTTONDOWN => Some(WatchEvent::RightDown),
                WM_MOUSEWHEEL | WM_MOUSEHWHEEL => Some(WatchEvent::Wheel),
                _ => None,
            };
            if let Some(e) = ev { pet::push(Event::SelWatch(e)); }
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

unsafe extern "system" fn key_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && HOOKS_RUN.load(Relaxed) {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        if k.flags.0 & LLKHF_INJECTED.0 == 0 {
            // Arrows and modifiers keep their code; every other key is only "a key" (0).
            let vk = if is_arrow(k.vkCode) || is_modifier(k.vkCode) { k.vkCode } else { 0 };
            let ev = match wp.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => Some(WatchEvent::KeyDown { vk }),
                WM_KEYUP | WM_SYSKEYUP => Some(WatchEvent::KeyUp { vk, shift: key_down(VK_SHIFT), ctrl: key_down(VK_CONTROL) }),
                _ => None,
            };
            if let Some(e) = ev { pet::push(Event::SelWatch(e)); }
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

// ---------- when to look ----------

fn double_click_ms() -> u64 { unsafe { GetDoubleClickTime() as u64 } }

fn point_in_bar(x: i32, y: i32) -> bool {
    let Some(h) = st().bar else { return false };
    let mut r = RECT::default();
    unsafe { GetWindowRect(HWND(h as *mut _), &mut r).is_ok() && x >= r.left && x < r.right && y >= r.top && y < r.bottom }
}

/// An event from the hooks, on the UI thread.
pub fn on_watch(e: WatchEvent) {
    let s = st();
    let Some(flow) = s.flow.clone() else { return };
    let phase = flow.phase();
    // Clicks on the bar itself are the bar's.
    match e {
        WatchEvent::LeftDown { x, y, .. } | WatchEvent::LeftUp { x, y, .. } if point_in_bar(x, y) => return,
        _ => {}
    }
    // While the question is typed, the keys are the question's, not a sign that the selection is over.
    if phase == PetSelectionFlowPhase::Asking && matches!(e, WatchEvent::KeyDown { .. } | WatchEvent::KeyUp { .. } | WatchEvent::Wheel) { return; }
    match s.watcher.handle(e, &settings(), double_click_ms()) {
        WatchAction::None => {}
        WatchAction::Dismiss => {
            kill_timer();
            s.generation += 1;
            if matches!(phase, PetSelectionFlowPhase::Bar | PetSelectionFlowPhase::Asking) { flow.dismiss(); }
        }
        WatchAction::Check { after_secs } => {
            s.generation += 1;
            if after_secs <= 0.0 { kill_timer(); look(); } else { set_timer((after_secs * 1000.0) as u32); }
        }
    }
}

fn set_timer(ms: u32) {
    let Some(h) = app().pet.pet_hwnd() else { return };
    unsafe { SetTimer(Some(h), T_SELECT, ms, None); }
    st().timer_on = true;
}

fn kill_timer() {
    let s = st();
    if !s.timer_on { return; }
    s.timer_on = false;
    if let Some(h) = app().pet.pet_hwnd() { unsafe { let _ = KillTimer(Some(h), T_SELECT); } }
}

/// The waiting is over: look.
pub fn on_timer() {
    kill_timer();
    look();
}

/// Reads on a worker thread (another app may take a while to answer), then `Event::SelRead` comes back.
fn look() {
    let s = st();
    let Some(flow) = s.flow.clone() else { return };
    if flow.phase() == PetSelectionFlowPhase::Sending || s.reading { return; }
    s.reading = true;
    let (generation, settings) = (s.generation, settings());
    std::thread::spawn(move || {
        let result = read_selection(&settings);
        pet::push(Event::SelRead { generation, result: result.map(Box::new) });
    });
}

pub struct ReadOk { pub capture: PetSelectionCapture, pub anchor: Option<Rect>, pub pointer: (f64, f64) }

/// The read came back.
pub fn on_read(generation: u64, result: Option<Box<ReadOk>>) {
    let s = st();
    s.reading = false;
    let Some(flow) = s.flow.clone() else { return };
    // Something happened since the look began (a click, a key, the bar switched off): the answer is stale.
    if generation != s.generation { return; }
    match result {
        Some(r) => {
            s.anchor = r.anchor.or(Some(Rect::new(r.pointer.0, r.pointer.1, 0.0, 0.0)));
            flow.present(&r.capture);
        }
        None => { if matches!(flow.phase(), PetSelectionFlowPhase::Bar | PetSelectionFlowPhase::Asking) { flow.dismiss(); } }
    }
}

/// For `ctl`: shows the bar for a given text without reading any app (screenshots and tests).
pub fn present_text(text: &str, near: Option<(f64, f64)>) {
    let s = st();
    let Some(flow) = s.flow.clone() else { return };
    s.anchor = near.map(|p| Rect::new(p.0, p.1, 0.0, 0.0));
    flow.present(&PetSelectionCapture {
        text: text.into(), app_name: "Notepad".into(), bundle_id: "notepad.exe".into(), window_title: Some("notes.txt - Notepad".into()),
        before: Some("before ".into()), after: Some(" after".into()), ..Default::default()
    });
}

/// For `ctl`: closes the bar.
pub fn hide() { if let Some(f) = st().flow.clone() { f.dismiss(); } }

// ---------- reading ----------

struct ComGuard(bool);
impl ComGuard {
    fn init() -> Self { ComGuard(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() }) }
}
impl Drop for ComGuard {
    fn drop(&mut self) { if self.0 { unsafe { CoUninitialize(); } } }
}

fn exe_of(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        if !ok { return None; }
        String::from_utf16_lossy(&buf[..n as usize]).rsplit('\\').next().map(String::from)
    }
}

/// True when the process runs elevated, and also when Windows will not say (another user's, a service): the bar
/// stays out of both.
fn elevated_or_unknown(pid: u32) -> bool {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return true };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(h, windows::Win32::Security::TOKEN_QUERY, &mut token).is_ok();
        let _ = CloseHandle(h);
        if !opened { return true; }
        let mut e = windows::Win32::Security::TOKEN_ELEVATION::default();
        let mut n = 0u32;
        let ok = windows::Win32::Security::GetTokenInformation(token, windows::Win32::Security::TokenElevation, Some(&mut e as *mut _ as *mut _),
            std::mem::size_of_val(&e) as u32, &mut n).is_ok();
        let _ = CloseHandle(token);
        !ok || e.TokenIsElevated != 0
    }
}

fn window_title(h: HWND) -> Option<String> {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) } as usize;
    if n == 0 { None } else { Some(String::from_utf16_lossy(&buf[..n.min(buf.len())])) }
}

const BROWSERS: [&str; 8] = ["chrome.exe", "msedge.exe", "firefox.exe", "brave.exe", "vivaldi.exe", "opera.exe", "arc.exe", "iexplore.exe"];

fn read_selection(settings: &PetSelectionBarSettings) -> Option<ReadOk> {
    let _com = ComGuard::init();
    unsafe {
        let fg = GetForegroundWindow();
        let mut fg_pid = 0u32;
        if !fg.0.is_null() { GetWindowThreadProcessId(fg, Some(&mut fg_pid)); }
        let uia: Option<IUIAutomation> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
        let focused = uia.as_ref().and_then(|u| u.GetFocusedElement().ok());
        // What the focused element is, before any text of it is touched. The app that owns it, not a frame around it
        // (apps from the Store sit inside one), decides the exe.
        let pid = focused.as_ref().and_then(|e| e.CurrentProcessId().ok()).map(|p| p as u32).filter(|p| *p != 0).unwrap_or(fg_pid);
        let mut facts = FocusFacts::default();
        if pid != 0 {
            facts.exe = exe_of(pid);
            facts.elevated_or_unknown = elevated_or_unknown(pid);
        }
        if let Some(e) = &focused {
            facts.is_password = e.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false);
            facts.control_type = e.CurrentControlType().ok().map(|c| format!("{}", c.0));
        } else {
            // No answer from UI Automation: not knowing whether it is a password field means not reading it.
            facts.secure_desktop = fg.0.is_null();
        }
        let probe = facts.probe();
        let exe = facts.exe.clone().unwrap_or_default();
        let out = PetSelectionReader::read(&probe, settings, clock::now(), || {
            let mut anchor = None;
            let mut cap = PetSelectionCapture {
                app_name: exe.trim_end_matches(".exe").to_string(),
                bundle_id: exe.clone(),
                window_title: window_title(fg),
                ..Default::default()
            };
            let via_pattern = focused.as_ref().and_then(|e| read_pattern(e, &mut cap, &mut anchor));
            match via_pattern {
                Some(true) => {}
                // The element has text but nothing is selected: nothing to show.
                Some(false) => return None,
                // No text pattern here: the same thing the Mac cannot do, by copying, with the clipboard given back.
                None => cap.text = copy_fallback()?,
            }
            if BROWSERS.iter().any(|b| exe.eq_ignore_ascii_case(b)) {
                if let (Some(u), Some(e)) = (&uia, &focused) { browser_page(u, e, &mut cap); }
            }
            READ_ANCHOR.with(|a| *a.borrow_mut() = anchor);
            Some(cap)
        });
        let capture = out.capture?;
        let anchor = READ_ANCHOR.with(|a| a.borrow_mut().take());
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        Some(ReadOk { capture, anchor, pointer: (p.x as f64, p.y as f64) })
    }
}

thread_local! { static READ_ANCHOR: std::cell::RefCell<Option<Rect>> = const { std::cell::RefCell::new(None) }; }

/// Reads the selection through the Text pattern. `None`: the element has no text pattern. `Some(false)`: it has one and
/// nothing is selected. `Some(true)`: `cap.text`, the text around it and `anchor` are filled.
unsafe fn read_pattern(el: &IUIAutomationElement, cap: &mut PetSelectionCapture, anchor: &mut Option<Rect>) -> Option<bool> {
    let pat: IUIAutomationTextPattern = el.GetCurrentPattern(UIA_TextPatternId).ok()?.cast().ok()?;
    let ranges = pat.GetSelection().ok()?;
    if ranges.Length().ok()? < 1 { return Some(false); }
    let sel = ranges.GetElement(0).ok()?;
    let text = sel.GetText(200_000).ok()?.to_string();
    if text.is_empty() { return Some(false); }
    cap.text = text;
    let around = PetSelectionLimits::AROUND as i32;
    if let Ok(b) = sel.Clone() {
        if b.MoveEndpointByRange(TextPatternRangeEndpoint_End, &sel, TextPatternRangeEndpoint_Start).is_ok()
            && b.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -around).is_ok()
        {
            cap.before = b.GetText(around).ok().map(|s| s.to_string());
        }
    }
    if let Ok(a) = sel.Clone() {
        if a.MoveEndpointByRange(TextPatternRangeEndpoint_Start, &sel, TextPatternRangeEndpoint_End).is_ok()
            && a.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, around).is_ok()
        {
            cap.after = a.GetText(around).ok().map(|s| s.to_string());
        }
    }
    *anchor = bounds_of(&sel);
    Some(true)
}

/// The union of the rectangles the selection covers, in screen pixels.
unsafe fn bounds_of(range: &IUIAutomationTextRange) -> Option<Rect> {
    use windows::Win32::System::Ole::*;
    let psa = range.GetBoundingRectangles().ok()?;
    if psa.is_null() { return None; }
    let lo = SafeArrayGetLBound(psa, 1).ok()?;
    let hi = SafeArrayGetUBound(psa, 1).ok()?;
    let n = (hi - lo + 1).max(0) as usize;
    let mut out = None;
    if n >= 4 {
        let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
        if SafeArrayAccessData(psa, &mut data).is_ok() && !data.is_null() {
            let v = std::slice::from_raw_parts(data as *const f64, n);
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for r in v.chunks(4).filter(|c| c.len() == 4) {
                x0 = x0.min(r[0]); y0 = y0.min(r[1]); x1 = x1.max(r[0] + r[2]); y1 = y1.max(r[1] + r[3]);
            }
            if x1 >= x0 && y1 >= y0 { out = Some(Rect::new(x0, y0, x1 - x0, y1 - y0)); }
            let _ = SafeArrayUnaccessData(psa);
        }
    }
    let _ = SafeArrayDestroy(psa);
    out
}

/// A browser's page address and title: the document element above the focus.
unsafe fn browser_page(uia: &IUIAutomation, focused: &IUIAutomationElement, cap: &mut PetSelectionCapture) {
    let Ok(walker) = uia.ControlViewWalker() else { return };
    let mut cur = focused.clone();
    for _ in 0..40 {
        if cur.CurrentControlType().map(|c| c == UIA_DocumentControlTypeId).unwrap_or(false) {
            let value: Option<IUIAutomationValuePattern> = cur.GetCurrentPattern(UIA_ValuePatternId).ok().and_then(|p| p.cast().ok());
            cap.url = value.and_then(|v| v.CurrentValue().ok()).map(|s| s.to_string()).filter(|u| u.starts_with("http"));
            cap.page_title = cur.CurrentName().ok().map(|s| s.to_string());
            return;
        }
        match walker.GetParentElement(&cur) { Ok(p) => cur = p, Err(_) => return }
    }
}

// ---------- the copy fallback ----------

/// Copies the selection with Ctrl+C and gives the clipboard back (the core decides the order, tested with a fake).
/// Not while Shift, Alt or Windows are held: Ctrl+Shift+C is not "copy" in every app.
fn copy_fallback() -> Option<String> {
    for k in [VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN] { if key_down(k) { return None; } }
    // The clipboard history does not record the copy and the giving back as copies of the user.
    crate::clipboard::quiet_for(2500);
    let text = read_by_copy(&mut WinClipboard);
    crate::clipboard::quiet_for(400);
    text
}

type Format = (u32, Vec<u8>);

struct WinClipboard;

const MAX_KEEP: usize = 64 * 1024 * 1024;
const CF_BITMAP: u32 = 2;
const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
const CF_UNICODETEXT: u32 = 13;

fn open_clipboard() -> bool {
    // The app that just copied may still hold it for a moment.
    for _ in 0..40 {
        if unsafe { OpenClipboard(None).is_ok() } { return true; }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

impl SelectionClipboard for WinClipboard {
    type Snapshot = Vec<Format>;

    fn sequence(&self) -> u32 { unsafe { GetClipboardSequenceNumber() } }

    fn snapshot(&mut self) -> Option<Vec<Format>> {
        if !open_clipboard() { return None; }
        let mut out: Vec<Format> = vec![];
        let mut total = 0usize;
        let mut ok = true;
        unsafe {
            let mut fmt = 0u32;
            let (mut has_dib, mut has_bitmap, mut has_other_gdi) = (false, false, false);
            loop {
                fmt = EnumClipboardFormats(fmt);
                if fmt == 0 { break; }
                match fmt {
                    CF_BITMAP => { has_bitmap = true; continue; }
                    CF_DIB | CF_DIBV5 => has_dib = true,
                    // Handles that are not memory (metafiles, palettes) and the private ranges cannot be copied.
                    3 | 9 | 14 | 0x80..=0x8F | 0x200..=0x3FF => { has_other_gdi = true; continue; }
                    _ => {}
                }
                let Ok(h) = GetClipboardData(fmt) else { continue };
                let g = HGLOBAL(h.0);
                let size = GlobalSize(g);
                if size == 0 { continue; }
                total += size;
                if total > MAX_KEEP { ok = false; break; }
                let p = GlobalLock(g) as *const u8;
                if p.is_null() { continue; }
                out.push((fmt, std::slice::from_raw_parts(p, size).to_vec()));
                let _ = GlobalUnlock(g);
            }
            // A bitmap with no device-independent copy, or a metafile, cannot be given back: nothing is touched.
            if has_other_gdi || (has_bitmap && !has_dib) { ok = false; }
            let _ = CloseClipboard();
        }
        if ok { Some(out) } else { None }
    }

    fn text(&mut self) -> Option<String> {
        if !open_clipboard() { return None; }
        let r = unsafe {
            (|| {
                let h = GetClipboardData(CF_UNICODETEXT).ok()?;
                let g = HGLOBAL(h.0);
                let p = GlobalLock(g) as *const u16;
                if p.is_null() { return None; }
                let max = GlobalSize(g) / 2;
                let mut n = 0;
                while n < max && *p.add(n) != 0 { n += 1; }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                let _ = GlobalUnlock(g);
                Some(s)
            })()
        };
        unsafe { let _ = CloseClipboard(); }
        r
    }

    fn restore(&mut self, s: Vec<Format>) {
        if !open_clipboard() { return; }
        unsafe {
            let _ = EmptyClipboard();
            for (fmt, bytes) in &s {
                let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else { continue };
                let p = GlobalLock(mem) as *mut u8;
                if p.is_null() { let _ = GlobalFree(Some(mem)); continue; }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
                let _ = GlobalUnlock(mem);
                if SetClipboardData(*fmt, Some(HANDLE(mem.0))).is_err() { let _ = GlobalFree(Some(mem)); }
            }
            // What was given back is not a new copy: Windows' clipboard history and sync skip it.
            if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, 4) {
                let p = GlobalLock(mem) as *mut u32;
                if !p.is_null() {
                    *p = 1;
                    let _ = GlobalUnlock(mem);
                    let f = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
                    if SetClipboardData(f, Some(HANDLE(mem.0))).is_err() { let _ = GlobalFree(Some(mem)); }
                } else { let _ = GlobalFree(Some(mem)); }
            }
            let _ = CloseClipboard();
        }
    }

    fn press_copy(&mut self) {
        unsafe {
            let key = |vk: VIRTUAL_KEY, up: bool| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT {
                wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } } };
            // A Ctrl the user still holds (Only while holding Ctrl) stays theirs.
            let held = key_down(VK_CONTROL);
            let mut seq = vec![];
            if !held { seq.push(key(VK_CONTROL, false)); }
            seq.push(key(VK_C, false));
            seq.push(key(VK_C, true));
            if !held { seq.push(key(VK_CONTROL, true)); }
            SendInput(&seq, std::mem::size_of::<INPUT>() as i32);
        }
    }

    fn pause(&mut self, ms: u32) { std::thread::sleep(std::time::Duration::from_millis(ms as u64)); }
}

// ---------- the bar ----------

struct BarLayout { buttons: Vec<(PetSelectionAction, Rect)>, divider: f64, ask: Rect, more: Rect, w: f64, h: f64 }

const PAD: f64 = 4.0;
const BTN_H: f64 = tok::SELECT_BAR_HEIGHT - 6.0;
const ICON: f64 = tok::SELECT_BAR_ICON;

fn label_w(s: &str, f: Font) -> f64 { gfx().measure(s, f, 10_000.0).0.ceil() }

fn layout(buttons: &[PetSelectionAction]) -> BarLayout {
    let y = (tok::SELECT_BAR_HEIGHT - BTN_H) / 2.0;
    let mut x = PAD;
    let mut out = vec![];
    for a in buttons {
        let w = 8.0 + ICON + 4.0 + label_w(a.title(), sans(12)) + 8.0;
        out.push((*a, Rect::new(x, y, w, BTN_H)));
        x += w + 1.0;
    }
    let divider = x + 2.0;
    x = divider + 1.0 + 3.0;
    let aw = 8.0 + 18.0 + 4.0 + label_w(PetSelectionAction::Ask.title(), semi(12)) + 8.0;
    let ask = Rect::new(x, y, aw, BTN_H);
    x += aw + 1.0;
    let more = Rect::new(x, y, 28.0, BTN_H);
    BarLayout { buttons: out, divider, ask, more, w: more.right() + PAD, h: tok::SELECT_BAR_HEIGHT }
}

/// The Segoe Fluent icon of an action (the mockup's icon names are the Mac's SF Symbols).
fn action_glyph(a: PetSelectionAction) -> u32 {
    match a {
        PetSelectionAction::Copy => 0xE8C8,
        PetSelectionAction::Search => 0xE721,
        PetSelectionAction::Translate => 0xE8C1,
        PetSelectionAction::Summarize => 0xE8E4,
        PetSelectionAction::Explain => 0xEA80,
        PetSelectionAction::Ask => 0xE8BD,
    }
}

fn close_bar() {
    let s = st();
    s.bar_mode = None;
    s.closing = true;
    if let Some(h) = s.bar.take() { unsafe { let _ = DestroyWindow(HWND(h as *mut _)); } }
    s.closing = false;
}

fn work_area_at(x: i32, y: i32) -> Rect {
    unsafe {
        let m = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(m, &mut mi).as_bool() {
            let r = mi.rcWork;
            return Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64);
        }
    }
    Rect::new(0.0, 0.0, 1280.0, 800.0)
}

/// Shows the bar (or the question field) above the selection, below it when there is no room.
fn show_bar(ask: bool) {
    let s = st();
    let Some(flow) = s.flow.clone() else { return };
    close_bar();
    let visible = settings().visible_buttons();
    let (w, h) = if ask { (tok::SELECT_ASK_WIDTH, tok::SELECT_BAR_HEIGHT) } else { let l = layout(&visible); (l.w, l.h) };
    if ask { s.question = flow.question(); }
    let anchor = s.anchor;
    let mut cur = POINT::default();
    unsafe { let _ = GetCursorPos(&mut cur); }
    let at = anchor.map(|a| (a.x as i32, a.y as i32)).unwrap_or((cur.x, cur.y));
    let mut ex = WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
    // The bar never takes the focus from the app the text is in; only the question field does.
    if !ask { ex |= WS_EX_NOACTIVATE; }
    let win = UiWindow::create("Nacho selection", WS_POPUP, ex, at.0, at.1, w, h, Box::new(draw_bar));
    win.clear = Some(pet_theme::current().window);
    win.close_on_deactivate = ask;
    win.on_close = Some(Box::new(|| {
        let s = st();
        if s.closing { return; }
        s.bar = None;
        s.bar_mode = None;
        // The question field lost the focus (another window was clicked): the same as a click elsewhere.
        if let Some(f) = s.flow.clone() { if matches!(f.phase(), PetSelectionFlowPhase::Bar | PetSelectionFlowPhase::Asking) { later(move || f.dismiss()); } }
    }));
    let hwnd = win.hwnd;
    let scale = crate::window::dpi_scale(hwnd);
    let work = work_area_at(at.0, at.1);
    let anchor_px = anchor.or(Some(Rect::new(cur.x as f64, cur.y as f64, 0.0, 0.0)));
    let (pw, ph) = ((w * scale).ceil(), (h * scale).ceil());
    let (x, y) = SelectionBarPlacement::origin(anchor_px, (cur.x as f64, cur.y as f64), (pw, ph), work, tok::SELECT_BAR_OFFSET * scale);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x as i32, y as i32, pw as i32, ph as i32, SWP_NOACTIVATE);
        let r = (rad::SELECT_BAR * scale) as i32 * 2;
        let region = CreateRoundRectRgn(0, 0, pw as i32 + 1, ph as i32 + 1, r, r);
        SetWindowRgn(hwnd, Some(region), true);
        if ask { win.show(); } else { let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE); }
    }
    s.bar = Some(hwnd.0 as isize);
    s.bar_mode = Some(ask);
}

fn draw_bar(ui: &mut Ui, ctx: &mut WindowCtx) {
    let pal = pet_theme::current();
    let s = st();
    let Some(flow) = s.flow.clone() else { ctx.close = true; return };
    let (w, h) = (ui.w, ui.h);
    ui.c.fill(Rect::new(0.0, 0.0, w, h), pal.window);
    ui.c.stroke_round(Rect::new(0.5, 0.5, w - 1.0, h - 1.0), rad::SELECT_BAR, pal.edge, 1.0);
    let keys = ui.st.keys.clone();
    for k in &keys { if k.vk == 0x1B { later({ let f = flow.clone(); move || f.dismiss() }); } }
    match flow.phase() {
        PetSelectionFlowPhase::Bar => draw_buttons(ui, &pal, &flow),
        PetSelectionFlowPhase::Asking => draw_ask(ui, &pal, &flow, &keys),
        _ => {}
    }
}

fn draw_buttons(ui: &mut Ui, pal: &crate::pet_theme::Pal, flow: &Arc<PetSelectionFlow>) {
    let l = layout(&settings().visible_buttons());
    let mut act: Option<PetSelectionAction> = None;
    for (a, r) in &l.buttons {
        let resp = ui.hit(ui.id(a.raw()), *r, true);
        if resp.hover { ui.c.round(*r, rad::SELECT_BAR - 3.0, if resp.pressed { pal.surface.mix(pal.text, 0.1) } else { pal.surface }); ui.st.cursor_hand = true; }
        ui.c.glyph(action_glyph(*a), ICON as u32, Rect::new(r.x + 8.0, r.y, ICON, r.h), pal.secondary);
        ui.c.line_text(a.title(), sans(12), Rect::new(r.x + 8.0 + ICON + 4.0, r.y, r.w, r.h), pal.text, Align::Left);
        if resp.clicked { act = Some(*a); }
    }
    ui.c.line(l.divider, (l.h - 18.0) / 2.0, l.divider, (l.h + 18.0) / 2.0, pal.separator, 1.0);
    let resp = ui.hit(ui.id("ask"), l.ask, true);
    if resp.hover { ui.c.round(l.ask, rad::SELECT_BAR - 3.0, pal.surface); ui.st.cursor_hand = true; }
    ui.c.image(&crate::pet_art::key_of("idle"), || crate::pet_art::image("idle").map(|p| (*p).clone()), Rect::new(l.ask.x + 8.0, l.ask.cy() - 9.0, 18.0, 18.0), 1.0);
    ui.c.line_text(PetSelectionAction::Ask.title(), semi(12), Rect::new(l.ask.x + 8.0 + 18.0 + 4.0, l.ask.y, l.ask.w, l.ask.h), pal.accent, Align::Left);
    if resp.clicked { act = Some(PetSelectionAction::Ask); }
    let resp = ui.hit(ui.id("more"), l.more, true);
    if resp.hover { ui.c.round(l.more, rad::SELECT_BAR - 3.0, pal.surface); ui.st.cursor_hand = true; }
    ui.c.glyph(0xE712, 14, l.more, pal.secondary);
    if resp.clicked { later(show_menu); }
    if let Some(a) = act {
        let f = flow.clone();
        later(move || { f.choose(a, &settings()); });
    }
}

fn draw_ask(ui: &mut Ui, pal: &crate::pet_theme::Pal, flow: &Arc<PetSelectionFlow>, keys: &[Key]) {
    let s = st();
    let (w, h) = (ui.w, ui.h);
    ui.c.image(&crate::pet_art::key_of("idle"), || crate::pet_art::image("idle").map(|p| (*p).clone()), Rect::new(8.0, h / 2.0 - 10.0, 20.0, 20.0), 1.0);
    let send_r = Rect::new(w - PAD - 26.0, (h - BTN_H) / 2.0, 26.0, BTN_H);
    let hint = t("↩ OK · esc");
    let hint_w = label_w(&hint, sans(11));
    let field = Rect::new(36.0, 4.0, send_r.x - 8.0 - hint_w - 8.0 - 36.0, h - 8.0);
    ui.c.round(field, rad::SELECT_BAR - 3.0, pal.raised);
    ui.c.stroke_round(field, rad::SELECT_BAR - 3.0, pal.accent, 1.0);
    // Typing: characters, Backspace, Ctrl+V; Return asks for the preview.
    let mut submit = false;
    for ch in std::mem::take(&mut ui.st.chars) {
        if ch == '\u{8}' { s.question.pop(); }
        else if !ch.is_control() && s.question.chars().count() < 2000 { s.question.push(ch); }
    }
    for k in keys {
        if k.vk == 0x0D { submit = true; }
        if k.vk == 0x56 && k.ctrl {
            if let Some(c) = crate::pet_chat::clipboard_text() {
                for ch in c.chars().map(|c| if c == '\r' || c == '\n' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control()) { if s.question.chars().count() < 2000 { s.question.push(ch); } }
            }
        }
    }
    ui.c.clip(field.inset(8.0, 0.0));
    if s.question.is_empty() {
        ui.c.line_text(&t("Ask about the selected text…"), sans(12), field.inset(8.0, 0.0), pal.tertiary, Align::Left);
        ui.c.line(field.x + 8.0, field.cy() - 8.0, field.x + 8.0, field.cy() + 8.0, pal.text, 1.0);
    } else {
        let tw = label_w(&s.question, sans(12));
        let room = field.w - 16.0 - 2.0;
        let shift = (tw - room).max(0.0);
        ui.c.line_text(&s.question, sans(12), Rect::new(field.x + 8.0 - shift, field.y, 100_000.0, field.h), pal.text, Align::Left);
        ui.c.line(field.x + 8.0 - shift + tw + 1.0, field.cy() - 8.0, field.x + 8.0 - shift + tw + 1.0, field.cy() + 8.0, pal.text, 1.0);
    }
    ui.c.unclip();
    ui.c.line_text(&hint, sans(11), Rect::new(send_r.x - 8.0 - hint_w, 0.0, hint_w, h), pal.secondary, Align::Left);
    let resp = ui.hit(ui.id("send"), send_r, true);
    if resp.hover { ui.c.round(send_r, rad::SELECT_BAR - 3.0, pal.surface); ui.st.cursor_hand = true; }
    ui.c.glyph(0xE74A, 12, send_r, pal.secondary);
    if resp.clicked { submit = true; }
    if submit {
        let (f, q) = (flow.clone(), s.question.clone());
        // An empty question stays in the field: nothing happens until something is typed.
        later(move || { f.submit_question(&q); });
    }
    ui.st.again = true;
}

// ---------- the bar's ⋯ menu ----------

fn show_menu() {
    let s = st();
    let Some(flow) = s.flow.clone() else { return };
    let ctx = flow.context();
    let exe = ctx.as_ref().map(|c| c.bundle_id.clone()).unwrap_or_default();
    let name = ctx.as_ref().map(|c| c.app_name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| t("this app").into_owned());
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let never = t(&format!("Never show in {name}")).into_owned();
        let _ = AppendMenuW(menu, if exe.is_empty() { MF_STRING | MF_GRAYED } else { MF_STRING }, 1, &HSTRING::from(never));
        let _ = AppendMenuW(menu, MF_STRING, 2, &HSTRING::from(t("Pause for 1 hour").as_ref()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, 3, &HSTRING::from(t("Selection bar settings…").as_ref()));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let owner = app().main;
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, owner, None).0 as usize;
        let _ = DestroyMenu(menu);
        later(move || match cmd {
            1 => {
                pet::update_settings(|s| s.selection_bar.excluded_apps.push(PetExcludedApp::new(&exe, &name)));
                if let Some(f) = st().flow.clone() { f.dismiss(); }
            }
            2 => {
                pet::update_settings(|s| s.selection_bar.snoozed_until = Some(clock::now() + 3600.0));
                if let Some(f) = st().flow.clone() { f.dismiss(); }
            }
            3 => {
                if let Some(f) = st().flow.clone() { f.dismiss(); }
                crate::pages::open(Some(crate::core::settings::ModuleId::NachoPet));
            }
            _ => {}
        });
    }
}

// ---------- the flow reaches the screen ----------

/// The flow changed (any thread asked, the UI thread does it).
pub fn on_changed() {
    let s = st();
    let Some(flow) = s.flow.clone() else { return };
    match flow.phase() {
        PetSelectionFlowPhase::Bar => show_bar(false),
        PetSelectionFlowPhase::Asking => { if s.bar_mode != Some(true) { show_bar(true); } }
        PetSelectionFlowPhase::Preview | PetSelectionFlowPhase::Sending => {
            close_bar();
            if app().pet.chat.hwnd.is_some() { pet::invalidate_chat(); } else { pet::open_chat(); }
        }
        PetSelectionFlowPhase::Idle => { close_bar(); }
    }
    pet::invalidate_chat();
}

// ---------- the chat's selection links ----------

/// `nachopet://sel…` links from the chat. False when the link is not the bar's.
pub fn on_link(action: &str, q: &std::collections::HashMap<String, String>) -> bool {
    let s = st();
    let Some(flow) = s.flow.clone() else { return false };
    match action {
        "selrm" => { if let Some(f) = q.get("f").and_then(|f| PetSelectionField::from_raw(f)) { flow.toggle(f); } }
        "selsend" => flow.confirm(Some(Box::new(|r| if let Some(r) = r { pet::push(Event::Flash(r)); }))),
        "selcancel" => flow.dismiss(),
        "selfull" => { if let Some(c) = flow.context() { later(move || show_full(&c)); } }
        "selcopy" => {
            if let Some(turn) = q.get("turn").and_then(|id| app().pet.state.chat.turns.iter().find(|t| &t.client_msg_id == id)) {
                crate::capture::copy_text(&turn.assistant_text);
                pet::flash_text("답을 복사했어요.");
            }
        }
        "selagain" => {
            if let Some(sent) = q.get("turn").and_then(|id| app().pet.state.selection_turns.get(id)).cloned() {
                flow.again(sent, Some(Box::new(|r| if let Some(r) = r { pet::push(Event::Flash(r)); })));
            }
        }
        "selshow" => {
            if let Some(id) = q.get("turn") {
                if !s.expanded.remove(id) { s.expanded.insert(id.clone()); }
                pet::invalidate_chat();
            }
        }
        "selintro" => {
            if q.get("a").map(String::as_str) == Some("on") {
                pet::update_settings(|s| { s.selection_bar.enabled = true; s.selection_bar.intro_answered = true; s.selection_bar.snoozed_until = None; });
                switched_on();
            } else {
                pet::update_settings(|s| s.selection_bar.intro_answered = true);
            }
            pet::invalidate_chat();
        }
        _ => return false,
    }
    true
}

/// 전체 보기: the selected text as it will be sent (after the limit).
fn show_full(c: &PetSelectionContext) {
    let title = format!("고른 글 · {}자", PetText::number(c.total_count as i64));
    let note = if c.truncated() { format!("너무 길어서 앞 {}자만 보내요.\n\n", PetText::number(PetSelectionLimits::SELECTION as i64)) } else { String::new() };
    let body = format!("{note}{}", c.selection.replace('\n', "\r\n"));
    unsafe { MessageBoxW(Some(app().main), &HSTRING::from(body), &HSTRING::from(title), MB_OK | MB_ICONINFORMATION); }
}

pub fn chat_state() -> crate::core::pet::transcript::PetSelectionChatState {
    let s = &app().pet.sel;
    crate::core::pet::transcript::PetSelectionChatState {
        preview: s.flow.as_ref().and_then(|f| f.preview()),
        intro: app().pet.settings.selection_bar.shows_intro(),
        expanded: s.expanded.clone(),
    }
}

/// Phases and counts only.
pub fn diagnostics() -> serde_json::Value {
    let s = &app().pet.sel;
    serde_json::json!({ "hooks": s.hooks_on, "bar": s.bar.is_some(), "flow": s.flow.as_ref().map(|f| f.diagnostics()), "enabled": app().pet.settings.selection_bar.enabled })
}

// ---------- settings (Nacho Pet's page) ----------

pub fn settings_controls(ui: &mut Ui, x: f64, y: f64, w: f64) -> f64 {
    let th = ui.th;
    let mut y = y;
    let mut b = settings();
    let before = b.clone();
    ui.c.line(x + 18.0, y, x + w - 18.0, y, th.line, 1.0);
    y += 14.0;
    ui.eyebrow("Selection bar", x + 18.0, y, w, th.ink2);
    y += 24.0;
    if ui.switch("selon", x + 18.0, y, b.enabled) { b.enabled = !b.enabled; if b.enabled { b.intro_answered = true; b.snoozed_until = None; } }
    ui.c.line_text(&t("Show a small bar when you select text in another app"), medium(13), Rect::new(x + 70.0, y, w - 90.0, 22.0), th.ink, Align::Left);
    y += 32.0;
    y += ui.label(&t("Only what you choose goes to Nacho, and only after you press Send. Off by default; password fields, administrator windows and password managers are never read."), sans(12), x + 18.0, y, w - 36.0, th.ink2) + 12.0;
    if b.enabled {
        if b.is_snoozed(clock::now()) {
            ui.c.line_text(&t("Paused for now"), medium(12), Rect::new(x + 18.0, y, 200.0, 24.0), th.ink2, Align::Left);
            if ui.button("selresume", "Resume", x + 18.0 + 210.0, y - 2.0, Btn::Secondary, true) { b.snoozed_until = None; }
            y += 34.0;
        }
        ui.eyebrow("When the bar shows", x + 18.0, y, w, th.ink2);
        y += 22.0;
        let delays: Vec<String> = PetSelectionDelay::ALL.iter().map(|d| t(d.display_name()).into_owned()).collect();
        let cur = PetSelectionDelay::ALL.iter().position(|d| *d == b.delay);
        if let Some(i) = ui.segments("seldelay", &delays, cur, Rect::new(x + 18.0, y, w - 36.0, 30.0), false) { b.delay = PetSelectionDelay::ALL[i]; }
        y += 44.0;
        ui.eyebrow("Search with", x + 18.0, y, w, th.ink2);
        y += 22.0;
        let engines: Vec<String> = PetSearchEngine::ALL.iter().map(|e| e.display_name().to_string()).collect();
        let cur = PetSearchEngine::ALL.iter().position(|e| *e == b.search_engine);
        if let Some(i) = ui.segments("selengine", &engines, cur, Rect::new(x + 18.0, y, w - 36.0, 30.0), false) { b.search_engine = PetSearchEngine::ALL[i]; }
        y += 44.0;
        ui.eyebrow("Buttons", x + 18.0, y, w, th.ink2);
        y += 22.0;
        let order = b.order.clone();
        for (i, a) in order.iter().enumerate() {
            let shown = !b.hidden.contains(a);
            if ui.checkbox(&format!("selbtn{}", a.raw()), &t(a.settings_title()), x + 18.0, y, w - 150.0, shown) {
                if shown { b.hidden.push(*a); } else { b.hidden.retain(|h| h != a); }
            }
            if i > 0 && ui.button(&format!("selup{}", a.raw()), &t("Up"), x + w - 130.0, y - 2.0, Btn::Secondary, true) { b.order.swap(i, i - 1); }
            if i + 1 < order.len() && ui.button(&format!("seldn{}", a.raw()), &t("Down"), x + w - 70.0, y - 2.0, Btn::Secondary, true) { b.order.swap(i, i + 1); }
            y += 32.0;
        }
        y += 6.0;
        ui.eyebrow("Terminals", x + 18.0, y, w, th.ink2);
        y += 22.0;
        for e in PetSelectionExclusions::BUILT_INS.iter().filter(|e| e.can_allow) {
            let allowed = b.allowed_built_ins.iter().any(|a| a == e.id);
            if ui.checkbox(&format!("selterm{}", e.id), &t(&format!("Allow in {}", e.name)), x + 18.0, y, w - 36.0, allowed) {
                if allowed { b.allowed_built_ins.retain(|a| a != e.id); } else { b.allowed_built_ins.push(e.id.to_string()); }
            }
            y += 30.0;
        }
        y += 6.0;
        ui.eyebrow("Never show in these apps", x + 18.0, y, w, th.ink2);
        y += 22.0;
        let apps = b.excluded_apps.clone();
        for (i, a) in apps.iter().enumerate() {
            ui.c.line_text(&format!("{} · {}", a.name, a.bundle_id), sans(13), Rect::new(x + 18.0, y, w - 130.0, 24.0), th.ink, Align::Left);
            if ui.button(&format!("selrm{i}"), &t("Remove"), x + w - 90.0, y - 3.0, Btn::Secondary, true) { b.excluded_apps.remove(i); }
            y += 32.0;
        }
        let draft = ADD_DRAFT.with(|d| d.borrow().clone());
        let mut text = draft.clone();
        ui.text_field("seladd", Rect::new(x + 18.0, y, w - 130.0, 30.0), &mut text, 80);
        if text != draft { ADD_DRAFT.with(|d| *d.borrow_mut() = text.clone()); }
        if text.is_empty() { ui.c.line_text(&t("Name of the program, e.g. Slack.exe"), sans(12), Rect::new(x + 18.0 + 9.0, y, w - 150.0, 30.0), th.ink2.alpha(0.6), Align::Left); }
        let valid = PetSelectionExclusions::is_valid_exe_name(text.trim());
        if ui.button_rect("seladdgo", &t("Add"), Rect::new(x + w - 90.0, y, 72.0, 30.0), Btn::Secondary, true, valid) && valid {
            let exe = text.trim().to_string();
            b.excluded_apps.push(PetExcludedApp::new(&exe, exe.trim_end_matches(".exe")));
            ADD_DRAFT.with(|d| d.borrow_mut().clear());
        }
        y += 44.0;
    }
    if b != before { let changed = b.clone(); later(move || { pet::update_settings(|s| s.selection_bar = changed); refresh(); }); }
    y
}

thread_local! { static ADD_DRAFT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) }; }
