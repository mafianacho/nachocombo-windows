//! Nacho Pet on Windows: the character on the desktop, the session that talks to the Nacho server, the login, hold to
//! talk and the dictation. The chat window is `pet_chat`; the logic that decides things is in `core::pet`.
//!
//! Everything runs on the UI thread. The session and the recorder work on their own threads and hand results over
//! through [`push`] (a queue and one posted message), so no window code ever runs on another thread.
use crate::app::{app, later, WM_PET};
use crate::core::layout::Rect;
use crate::core::pet::auth::PetAuthBrowser;
use crate::core::pet::card_flow::{CardStep, PetCardFlow};
use crate::core::pet::chat_client::{thread_schedule, MakeTransport, WebSocketTransport};
use crate::core::pet::clock;
use crate::core::pet::composer::DigitHold;
use crate::core::pet::connector_client::{ConnectorBinding, PetCommandExecuting};
use crate::core::pet::design_tokens::pet_tokens::size as tok;
use crate::core::pet::dictation::{DictationDenial, DictationState};
use crate::core::pet::face::PetFace;
use crate::core::pet::login_browser::{self, PetCallbackBrowser};
use crate::core::pet::motion::{PetAnimationGate, PetMotion};
use crate::core::pet::placement;
use crate::core::pet::secrets::PetSecretBundle;
use crate::core::pet::server_api::PetAPIError;
use crate::core::pet::session::{PetAuthPhase, PetSession, PetSessionDeps, PetSessionState};
use crate::core::pet::settings::{PetPosition, PetSettings, PetSettingsStore};
use crate::core::pet::transcript::PetTextPart;
use crate::core::pet::voice::{HoldEvent, PetHoldGesture, PetVoiceText};
use crate::core::pet::wav;
use crate::core::settings::ModuleId;
use crate::gfx::*;
use crate::pet_art;
use crate::pet_audio::{RecordEnd, Recorder};
use crate::pet_win::{CredSecretStore, WinHttpClient, WinHttpWebSocket};
use crate::ui::*;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// `WM_COPYDATA` tag of a login redirect handed from the copy of the exe that Windows started for the link.
pub const CALLBACK_TAG: usize = 0x4E50_4341;

const PET_CLASS: PCWSTR = w!("NachoPetWindow");
const T_FRAME: usize = 1;
const T_SLOW: usize = 2;
const T_FLASH: usize = 3;

// ---------- events from other threads ----------

pub enum Event {
    Session(Box<PetSessionState>),
    Callback(String),
    OpenUrl(String),
    Recorded { end: RecordEnd, hold: bool },
    Dictated { hold: bool, result: std::result::Result<String, PetAPIError>, seconds: f64 },
    SendResult { text: String, voice: bool, attempt: u32, refusal: Option<String> },
    /// Time to try sending a spoken or chosen text again.
    Retry { text: String, voice: bool, attempt: u32 },
    CardAnswered { card_id: String, refusal: Option<String> },
    Flash(String),
    /// The "always allow" switches changed: the connector declares what this PC can do again.
    Capabilities(Vec<serde_json::Value>),
    /// The server took (or refused) the message the field held.
    SendDone(Option<String>),
    ArtChanged,
}

static QUEUE: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static MAIN: AtomicIsize = AtomicIsize::new(0);

/// Hands an event to the UI thread. Safe from any thread.
pub fn push(e: Event) {
    QUEUE.lock().unwrap().push(e);
    let h = MAIN.load(Ordering::Relaxed);
    if h != 0 { unsafe { let _ = PostMessageW(Some(HWND(h as *mut _)), WM_PET, WPARAM(0), LPARAM(0)); } }
}

// ---------- state ----------

#[derive(Clone, Copy, PartialEq)]
enum RecMode { Composer, Hold }

/// What the chat window keeps between frames.
#[derive(Default)]
pub struct ChatView {
    pub hwnd: Option<isize>,
    pub input: String,
    pub cursor: usize,
    pub flash: Option<(Vec<PetTextPart>, std::time::Instant)>,
    pub sig: u64,
    pub taken_back: Option<String>,
    /// The next message is the correction of a memory ("기억이 틀렸어요") for this turn.
    pub memory_feedback: Option<String>,
    pub tail_on_top: bool,
    pub tail_from_right: f64,
    pub sending: bool,
    /// The log follows its end while new content is still being measured (a card is taller than one frame knows).
    pub stick: bool,
    pub last_content: f64,
    /// The user is at the end of the log (until the wheel moves it away): new content then keeps the end in view.
    pub pinned: bool,
    pub last_view_h: f64,
}

#[derive(Default)]
pub struct PetUi {
    started: bool,
    session: Option<Arc<PetSession>>,
    store: Option<PetSettingsStore>,
    pub settings: PetSettings,
    pub state: PetSessionState,
    browser: Option<Arc<PetCallbackBrowser>>,
    pub flow: PetCardFlow,
    pub digits: DigitHold,
    pub dictation: DictationState,
    pub chat: ChatView,
    pet: Option<PetWin>,
    motion: PetMotion,
    gesture: PetHoldGesture,
    gate: PetAnimationGate,
    recorder: Option<Recorder>,
    /// The session holds the executor weakly: it lives here.
    executor: Option<Arc<dyn PetCommandExecuting>>,
    trust_watch: Option<u64>,
    rec_mode: Option<(RecMode, String)>,
    drag_grab: Option<(i32, i32)>,
    last_x: i32,
}

impl PetUi {
    pub fn pet_hwnd(&self) -> Option<HWND> { self.pet.as_ref().map(|w| w.hwnd) }
}

struct PetWin {
    hwnd: HWND,
    surface: Option<Surface>,
    timer_on: bool,
}

pub fn status_text() -> String {
    let p = &app().pet;
    if !p.started { return "Off".into(); }
    match &p.state.auth {
        PetAuthPhase::SignedOut => "Click Nacho to sign in".into(),
        PetAuthPhase::SigningIn => "Signing in…".into(),
        PetAuthPhase::Failed(_) => "Sign-in failed".into(),
        PetAuthPhase::SignedIn => {
            use crate::core::pet::chat_state::ConnectionPhase::*;
            match p.state.chat.phase { Connected => "Connected".into(), Connecting | Reconnecting { .. } => "Connecting…".into(), AuthFailed => "Sign in again".into(), Disconnected => "Signed in".into() }
        }
    }
}

fn now_secs() -> f64 { clock::now() }

fn now_ms() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

pub fn session() -> Option<Arc<PetSession>> { app().pet.session.clone() }

pub fn is_running() -> bool { app().pet.started }

// ---------- start / stop ----------

fn exe_path() -> String { std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default() }

/// The `nachogadget:` link for this user only (HKCU), written when the login button is pressed.
fn register_protocol() {
    for (key, name, value) in login_browser::protocol_registration(&exe_path()) {
        unsafe {
            let mut k = HKEY::default();
            if RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(&key), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut k, None).is_err() { continue; }
            let v: Vec<u16> = value.encode_utf16().chain([0]).collect();
            let n = name.map(HSTRING::from);
            let _ = RegSetValueExW(k, n.as_ref().map(|n| PCWSTR(n.as_ptr())).unwrap_or(PCWSTR::null()), None, REG_SZ, Some(std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2)));
            let _ = RegCloseKey(k);
        }
    }
}

/// Removes the link again when it still points at this exe (another install's link is not ours to remove).
fn unregister_protocol() {
    let key = HSTRING::from("Software\\Classes\\nachogadget\\shell\\open\\command");
    let mut buf = [0u16; 1024];
    let mut n = (buf.len() * 2) as u32;
    let ours = unsafe { RegGetValueW(HKEY_CURRENT_USER, &key, PCWSTR::null(), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut n)).is_ok() }
        && String::from_utf16_lossy(&buf[..(n as usize / 2).saturating_sub(1)]).contains(&exe_path());
    if ours { unsafe { let _ = RegDeleteTreeW(HKEY_CURRENT_USER, w!("Software\\Classes\\nachogadget")); } }
}

fn make_session(settings: PetSettings, browser: Arc<PetCallbackBrowser>, executor: &Arc<dyn PetCommandExecuting>) -> Arc<PetSession> {
    let secrets = Arc::new(PetSecretBundle::new(Arc::new(CredSecretStore::default())));
    let make_transport: MakeTransport = Arc::new(|| Box::new(WinHttpWebSocket::default()) as Box<dyn WebSocketTransport>);
    let s = Arc::new(PetSession::new(settings, PetSessionDeps {
        http: Arc::new(WinHttpClient::default()),
        secrets,
        browser: Some(browser as Arc<dyn PetAuthBrowser>),
        make_transport,
        executor: Some(Arc::downgrade(executor)),
        random: Arc::new(crate::pet_win::os_random),
        now: Arc::new(clock::now),
        schedule: thread_schedule(),
        connector_label: ConnectorBinding::LABEL_WINDOWS.into(),
    }));
    s.set_on_change(Some(Arc::new(|st| push(Event::Session(Box::new(st.clone()))))));
    // What this PC really runs (W3), declared when the connector opens and again whenever a switch changes.
    s.set_connector_capabilities(Some(crate::pet_exec::declaration()));
    s
}

pub fn start() -> std::result::Result<(), String> {
    let a = app();
    if a.pet.started { return Ok(()); }
    MAIN.store(a.main.0 as isize, Ordering::Relaxed);
    clock::set_local_offset(crate::pet_win::local_utc_offset());
    let store = PetSettingsStore::new(a.dir.join("pet.json"));
    let settings = store.load();
    let browser = Arc::new(PetCallbackBrowser::new(
        Box::new(register_protocol),
        Box::new(|url| push(Event::OpenUrl(url.to_string()))),
        Box::new(clock::now),
    ));
    let executor: Arc<dyn PetCommandExecuting> = Arc::new(crate::pet_exec::WindowsPetExecutor);
    let session = make_session(settings.clone(), browser.clone(), &executor);
    let p = &mut a.pet;
    *p = PetUi { started: true, state: session.snapshot(), settings, store: Some(store), browser: Some(browser), session: Some(session), executor: Some(executor), ..Default::default() };
    p.trust_watch = Some(crate::pet_exec::watch_declaration(Arc::new(|caps| push(Event::Capabilities(caps)))));
    p.motion = PetMotion::new(PetFace::Idle, now_ms(), reduce_motion(), (now_ms() as u32) | 1);
    p.gesture = PetHoldGesture::default();
    p.gate.window_visible = true;
    register_pet_class();
    let hwnd = create_pet_window()?;
    p.pet = Some(PetWin { hwnd, surface: None, timer_on: false });
    place_pet_initially();
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), T_SLOW, 5000, None);
    }
    refresh_face();
    start_art();
    Ok(())
}

pub fn stop() {
    let a = app();
    if !a.pet.started { return; }
    let p = &mut a.pet;
    if let Some(r) = p.recorder.take() { r.cancel(); }
    if let Some(b) = &p.browser { b.cancel(); }
    p.dictation.stop();
    close_chat();
    if let Some(w) = p.pet.take() { unsafe { let _ = KillTimer(Some(w.hwnd), T_FRAME); let _ = KillTimer(Some(w.hwnd), T_SLOW); let _ = DestroyWindow(w.hwnd); } }
    // Closes the sockets; the stored login stays for the next time the pet is on.
    if let Some(s) = p.session.take() { s.shutdown(); }
    pet_art::stop_server_art();
    if let Some(t) = p.trust_watch.take() { crate::pet_exec::trust().remove_observer(t); }
    p.executor = None;
    p.started = false;
    p.browser = None;
    QUEUE.lock().unwrap().clear();
    unregister_protocol();
}

fn reduce_motion() -> bool {
    let mut on = BOOL(1);
    unsafe { let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    !on.as_bool()
}

fn start_art() {
    let a = app();
    let Some(_) = a.pet.session.as_ref() else { return };
    let settings = a.pet.settings.clone();
    let dir = a.dir.join("CharacterAssets");
    pet_art::start_server_art(dir, Arc::new(WinHttpClient { timeout_ms: 15_000 }), Box::new(move || settings.endpoint()), Box::new(|changed| if changed { push(Event::ArtChanged) }));
}

// ---------- events ----------

/// `WM_PET`: everything other threads handed over.
pub fn on_message() {
    let events: Vec<Event> = std::mem::take(&mut *QUEUE.lock().unwrap());
    for e in events {
        if !app().pet.started { return; }
        match e {
            Event::Session(st) => on_session(*st),
            Event::Callback(url) => { if let Some(b) = app().pet.browser.clone() { b.deliver(&url); } }
            Event::OpenUrl(url) => crate::app::open_url(&url),
            Event::Recorded { end, hold } => on_recorded(end, hold),
            Event::Dictated { hold, result, seconds } => on_dictated(hold, result, seconds),
            Event::SendResult { text, voice, attempt, refusal } => on_send_result(text, voice, attempt, refusal),
            Event::Retry { text, voice, attempt } => send_when_ready(text, voice, attempt),
            Event::CardAnswered { card_id, refusal } => card_answered(&card_id, refusal),
            Event::Flash(t) => flash_text(&t),
            Event::Capabilities(caps) => { if let Some(s) = session() { s.set_connector_capabilities(Some(caps)); } }
            Event::SendDone(r) => {
                let c = &mut app().pet.chat;
                c.sending = false;
                match r { None => { c.input.clear(); c.cursor = 0; } Some(r) => flash_text(&r) }
                invalidate_chat();
            }
            Event::ArtChanged => { pet_art::art_changed(); render_pet(); invalidate_chat(); }
        }
    }
}

/// The login redirect from a second copy of the exe (`WM_COPYDATA`).
pub fn deliver_callback(url: &str) {
    if app().pet.started { push(Event::Callback(url.to_string())); }
}

fn on_session(st: PetSessionState) {
    let p = &mut app().pet;
    let was = p.state.auth.clone();
    p.state = st;
    if was == PetAuthPhase::SigningIn && p.state.auth == PetAuthPhase::SignedIn { open_chat(); }
    refresh_face();
    invalidate_chat();
    crate::app::app().changed();
}

pub fn face() -> PetFace {
    let p = &app().pet;
    PetFace::derive(p.dictation.is_listening() || p.state.dictating, &p.state.chat, now_secs(), p.state.last_interaction)
}

fn refresh_face() {
    let p = &mut app().pet;
    let f = face();
    if f != p.motion.face() { p.motion.set_face(f, now_ms()); }
    request_frames();
}

// ---------- settings ----------

pub fn update_settings(change: impl FnOnce(&mut PetSettings)) {
    let p = &mut app().pet;
    let mut next = p.settings.clone();
    change(&mut next);
    let next = PetSettings::sanitized(next);
    if next == p.settings { return; }
    let server_changed = next.account_scope() != p.settings.account_scope();
    p.settings = next.clone();
    if let Some(s) = &p.store { let _ = s.save(&next); }
    if server_changed {
        if let (Some(old), Some(b), Some(e)) = (p.session.take(), p.browser.clone(), p.executor.clone()) {
            old.shutdown();
            let s = make_session(next, b, &e);
            p.state = s.snapshot();
            p.session = Some(s);
        }
    } else if let Some(s) = &p.session { s.apply_settings(next); }
}

// ---------- screens ----------

/// Work areas of every monitor in virtual-screen pixels, and the primary's.
fn screens() -> (Vec<Rect>, Rect) {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let v = &mut *(lp.0 as *mut Vec<(Rect, bool)>);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(m, &mut mi).as_bool() {
            let r = mi.rcWork;
            v.push((Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64), mi.dwFlags & 1 != 0));
        }
        TRUE
    }
    let mut v: Vec<(Rect, bool)> = vec![];
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut v as *mut _ as isize)); }
    let primary = v.iter().find(|m| m.1).or(v.first()).map(|m| m.0).unwrap_or(Rect::new(0.0, 0.0, 1280.0, 720.0));
    (v.into_iter().map(|m| m.0).collect(), primary)
}

fn pet_px(hwnd: HWND) -> f64 { (tok::PET * crate::window::dpi_scale(hwnd)).round() }

fn place_pet_initially() {
    let p = &mut app().pet;
    let Some(w) = &p.pet else { return };
    let (all, primary) = screens();
    let size = pet_px(w.hwnd);
    let scale = crate::window::dpi_scale(w.hwnd);
    let saved = p.settings.pet_position.map(|v| (v.x, v.y));
    let o = placement::initial_origin(saved, size, &all, primary, tok::PET_SCREEN_MARGIN * scale);
    unsafe { let _ = SetWindowPos(w.hwnd, Some(HWND_TOPMOST), o.0 as i32, o.1 as i32, size as i32, size as i32, SWP_NOACTIVATE); }
    render_pet();
}

pub fn reset_position() {
    update_settings(|s| s.pet_position = None);
    place_pet_initially();
    follow_pet();
}

/// The pet's window rectangle in pixels.
pub fn pet_rect() -> Option<Rect> {
    let w = app().pet.pet.as_ref()?;
    let mut r = RECT::default();
    unsafe { GetWindowRect(w.hwnd, &mut r).ok()?; }
    Some(Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64))
}

// ---------- the pet window ----------

fn register_pet_class() {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(pet_proc), hInstance: hinst.into(), hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(), lpszClassName: PET_CLASS, ..Default::default() };
        RegisterClassExW(&wc);
    }
}

fn create_pet_window() -> std::result::Result<HWND, String> {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_ACCEPTFILES, PET_CLASS, w!("Nacho"), WS_POPUP, 0, 0, 112, 112, None, None, Some(hinst.into()), None)
            .map_err(|e| format!("window: {e}"))
    }
}

/// An off-screen picture of the pet kept between frames (the picture cache lives in its canvas), copied into the
/// layered window's per-pixel-alpha bitmap.
struct Surface {
    w: u32,
    h: u32,
    bmp: IWICBitmap,
    canvas: Canvas,
    dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
}

impl Surface {
    fn new(w: u32, h: u32, dpi: f32) -> windows::core::Result<Surface> {
        unsafe {
            let g = gfx();
            let bmp = g.wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES { r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT, pixelFormat: D2D1_PIXEL_FORMAT { format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: dpi, dpiY: dpi, usage: D2D1_RENDER_TARGET_USAGE_NONE, minLevel: D2D1_FEATURE_LEVEL_DEFAULT };
            let rt = g.d2d.CreateWicBitmapRenderTarget(&bmp, &props)?;
            let canvas = Canvas::new(rt.cast()?)?;
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w as i32, biHeight: -(h as i32), biPlanes: 1, biBitCount: 32, biCompression: 0, ..Default::default() }, ..Default::default() };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            ReleaseDC(None, screen);
            let old = SelectObject(dc, dib.into());
            Ok(Surface { w, h, bmp, canvas, dc, dib, old, bits: bits as *mut u8 })
        }
    }

    fn draw(&self, f: impl FnOnce(&Canvas)) -> windows::core::Result<()> {
        unsafe {
            let rt = &self.canvas.rt;
            rt.BeginDraw();
            rt.Clear(Some(&Color(0.0, 0.0, 0.0, 0.0).d2d()));
            f(&self.canvas);
            rt.EndDraw(None, None)?;
            let buf = std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize);
            self.bmp.CopyPixels(std::ptr::null(), self.w * 4, buf)?;
        }
        Ok(())
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.dib.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Draws the pet now and pushes it to the window.
pub fn render_pet() {
    let p = &mut app().pet;
    let Some(win) = p.pet.as_mut() else { return };
    let hwnd = win.hwnd;
    let scale = crate::window::dpi_scale(hwnd);
    let px = (tok::PET * scale).round() as u32;
    if win.surface.as_ref().is_none_or(|s| s.w != px) { win.surface = Surface::new(px, px, (96.0 * scale) as f32).ok(); }
    let Some(surface) = win.surface.as_ref() else { return };
    let t = now_ms();
    let pose = p.motion.pose(t);
    let (progress, recording) = (p.gesture.progress(now_secs()), p.gesture.is_recording() || p.recorder.is_some());
    let level = p.recorder.as_ref().map_or(0.0, |r| r.level());
    let _ = surface.draw(|c| {
        let s = tok::PET;
        let art = Rect::new(tok::PET_ART_INSET, tok::PET_ART_INSET, s - 2.0 * tok::PET_ART_INSET, s - 2.0 * tok::PET_ART_INSET);
        // Shadow under the feet.
        c.transformed(s / 2.0, s - 8.0, 0.0, 0.0, 1.0, 0.22, 0.0, || c.circle(s / 2.0, s - 8.0, 30.0, Color(0.0, 0.0, 0.0, 0.22)));
        c.transformed(s / 2.0, art.bottom(), pose.dx, -pose.dy, pose.scale_x, pose.scale_y, pose.rotation, || {
            if let Some(prev) = &pose.previous { draw_art(c, prev, art, 1.0); }
            draw_art(c, &pose.asset, art, pose.mix as f32);
        });
        let ring = Rect::new(4.0, 4.0, s - 8.0, s - 8.0);
        if recording {
            c.ring(ring.cx(), ring.cy(), ring.w / 2.0, Color(0.85, 0.2, 0.15, 0.95), 4.0);
            c.ring(ring.cx(), ring.cy(), ring.w / 2.0 - 5.0, Color(0.85, 0.2, 0.15, 0.35 + 0.65 * level as f32), 2.0 + 4.0 * level);
        } else if progress > 0.0 {
            c.ring(ring.cx(), ring.cy(), ring.w / 2.0, Color(0.0, 0.0, 0.0, 0.12), 4.0);
            c.arc(ring.cx(), ring.cy(), ring.w / 2.0, progress, crate::pet_theme::current().accent, 4.0);
        }
    });
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        let (pos, size, src) = (POINT { x: r.left, y: r.top }, SIZE { cx: px as i32, cy: px as i32 }, POINT { x: 0, y: 0 });
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let _ = UpdateLayeredWindow(hwnd, None, Some(&pos), Some(&size), Some(surface.dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
    }
}

fn draw_art(c: &Canvas, name: &str, r: Rect, opacity: f32) {
    if opacity <= 0.0 { return; }
    let key = pet_art::key_of(name);
    c.image(&key, || pet_art::image(name).map(|p| (*p).clone()), r, opacity);
}

/// Runs the frame timer while the pet moves; stops it when nothing does (a still pet costs nothing).
fn request_frames() {
    let p = &mut app().pet;
    let Some(win) = p.pet.as_mut() else { return };
    let t = now_ms();
    p.gate.interacting = p.gesture.phase() != crate::core::pet::voice::HoldPhase::Idle || p.recorder.is_some();
    let want = p.gate.needs_frames(&p.motion, t);
    if want && !win.timer_on {
        let busy = p.motion.face() != PetFace::Idle && p.motion.face() != PetFace::Sleeping && p.motion.face() != PetFace::Tired;
        let ms = if p.gate.interacting || busy { 33 } else { 66 };
        unsafe { SetTimer(Some(win.hwnd), T_FRAME, ms, None); }
        win.timer_on = true;
    } else if !want && win.timer_on {
        unsafe { let _ = KillTimer(Some(win.hwnd), T_FRAME); }
        win.timer_on = false;
    }
    render_pet_if_visible();
}

fn render_pet_if_visible() { if app().pet.gate.allows_motion() || app().pet.pet.as_ref().is_some_and(|w| w.surface.is_none()) { render_pet(); } }

/// Another program covers the whole monitor the pet is on: no frames are drawn for it.
fn full_screen_app_in_front() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() { return false; }
        let Some(pw) = app().pet.pet.as_ref().map(|w| w.hwnd) else { return false };
        if fg == pw || fg == GetDesktopWindow() || fg == GetShellWindow() { return false; }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(fg, &mut cls) as usize;
        let cls = String::from_utf16_lossy(&cls[..n]);
        if cls == "Progman" || cls == "WorkerW" || cls == "Shell_TrayWnd" { return false; }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() { return false; }
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromWindow(pw, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() { return false; }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom && MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) == MonitorFromWindow(pw, MONITOR_DEFAULTTONEAREST)
    }
}

fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe { let _ = GetCursorPos(&mut p); }
    (p.x, p.y)
}

unsafe extern "system" fn pet_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    if !a.pet.started { return DefWindowProcW(hwnd, msg, wp, lp); }
    match msg {
        WM_LBUTTONDOWN => {
            let (x, y) = cursor();
            let p = &mut a.pet;
            let _ = SetCapture(hwnd);
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            p.drag_grab = Some((x - r.left, y - r.top));
            p.last_x = x;
            voice_input_started();
            let talk = can_hold_to_talk();
            p.gesture.down(now_secs(), x as f64, y as f64, talk);
            p.motion.press(now_ms());
            request_frames();
            LRESULT(0)
        }
        WM_MOUSEMOVE if GetCapture() == hwnd => {
            let (x, y) = cursor();
            let p = &mut a.pet;
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let inside = x >= r.left && x < r.right && y >= r.top && y < r.bottom;
            let ev = p.gesture.move_to(now_secs(), x as f64, y as f64, inside);
            handle_hold(ev);
            if matches!(p.gesture.phase(), crate::core::pet::voice::HoldPhase::Dragging) {
                if let Some((gx, gy)) = p.drag_grab {
                    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x - gx, y - gy, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
                    p.motion.drag((x - p.last_x) as f64, now_ms());
                    p.last_x = x;
                    follow_pet();
                    render_pet();
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // The gesture ends first: releasing the capture sends WM_CAPTURECHANGED, which must find it finished.
            let ev = a.pet.gesture.up(now_secs());
            let _ = ReleaseCapture();
            a.pet.motion.release(now_ms());
            handle_hold(ev);
            request_frames();
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            // Capture taken away (a dialog opened): a recording or a drag ends the safe way.
            if a.pet.gesture.is_recording() { let ev = a.pet.gesture.escape(); handle_hold(ev); } else { a.pet.gesture.escape(); }
            LRESULT(0)
        }
        WM_RBUTTONUP => { show_menu(); LRESULT(0) }
        WM_DROPFILES => {
            let drop = HDROP(wp.0 as *mut _);
            let n = DragQueryFileW(drop, u32::MAX, None);
            let mut files = vec![];
            for i in 0..n.min(20) {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                files.push(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len])));
            }
            DragFinish(drop);
            later(move || for f in files { attach_file(&f); });
            LRESULT(0)
        }
        WM_TIMER => {
            match wp.0 {
                T_FRAME => {
                    let ev = a.pet.gesture.tick(now_secs());
                    handle_hold(ev);
                    request_frames();
                    if a.pet.gate.allows_motion() { render_pet(); }
                }
                T_SLOW => {
                    a.pet.gate.full_screen_app_in_front = full_screen_app_in_front();
                    if let Some(b) = a.pet.browser.clone() { if b.expire() { /* the session was told */ } }
                    refresh_face();
                }
                T_FLASH => { let _ = KillTimer(Some(hwnd), T_FLASH); invalidate_chat(); }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            let (all, _) = screens();
            let scale = crate::window::dpi_scale(hwnd);
            let size = pet_px(hwnd);
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let o = placement::keep_on_screen((r.left as f64, r.top as f64), size, &all);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), o.0 as i32, o.1 as i32, size as i32, size as i32, SWP_NOACTIVATE);
            let _ = scale;
            render_pet();
            follow_pet();
            LRESULT(0)
        }
        WM_SETCURSOR => { SetCursor(LoadCursorW(None, IDC_HAND).ok()); LRESULT(1) }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn handle_hold(ev: HoldEvent) {
    match ev {
        HoldEvent::None => {}
        HoldEvent::Click => { toggle_chat(); if let Some(s) = session() { s.touch(); } }
        HoldEvent::DragStarted => { app().pet.motion.drag(0.0, now_ms()); }
        HoldEvent::DragEnded => finish_drag(),
        HoldEvent::StartRecording => begin_hold_recording(),
        HoldEvent::FinishRecording { seconds } => finish_hold_recording(seconds),
        HoldEvent::CancelRecording => cancel_hold_recording(PetVoiceText::CANCELED),
    }
}

fn finish_drag() {
    let p = &mut app().pet;
    let Some(w) = &p.pet else { return };
    let hwnd = w.hwnd;
    let (all, _) = screens();
    let size = pet_px(hwnd);
    let mut r = RECT::default();
    unsafe { let _ = GetWindowRect(hwnd, &mut r); }
    let o = placement::keep_on_screen((r.left as f64, r.top as f64), size, &all);
    unsafe { let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), o.0 as i32, o.1 as i32, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE); }
    p.motion.drop(now_ms());
    update_settings(|s| s.pet_position = Some(PetPosition { x: o.0, y: o.1 }));
    follow_pet();
    request_frames();
}

// ---------- menu ----------

pub fn show_menu_at_cursor() { show_menu() }

fn show_menu() {
    use crate::i18n::t;
    let p = &app().pet;
    let signed_in = p.state.auth == PetAuthPhase::SignedIn;
    let chat_open = p.chat.hwnd.is_some();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |id: usize, s: &str| { let _ = AppendMenuW(menu, MF_STRING, id, &HSTRING::from(t(s).as_ref())); };
        add(1, if chat_open { "Close Chat" } else { "Open Chat" });
        if signed_in { add(2, "Sign Out"); } else { add(3, "Sign In"); }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        add(4, "Reset Chat Size");
        add(5, "Reset Position");
        add(6, "Nacho Pet Settings…");
        add(7, "Hide Nacho");
        let (x, y) = cursor();
        let owner = app().main;
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, None, owner, None).0 as usize;
        let _ = DestroyMenu(menu);
        later(move || run_menu(cmd));
    }
}

pub fn run_menu(cmd: usize) {
    match cmd {
        1 => toggle_chat(),
        2 => sign_out(),
        3 => sign_in(),
        4 => reset_chat_size(),
        5 => reset_position(),
        6 => crate::pages::open(Some(ModuleId::NachoPet)),
        7 => app().set_enabled(ModuleId::NachoPet, false),
        _ => {}
    }
}

// ---------- chat ----------

pub fn open_chat() { crate::pet_chat::open(); }
pub fn close_chat() { crate::pet_chat::close(); }
pub fn toggle_chat() { if app().pet.chat.hwnd.is_some() { close_chat() } else { open_chat() } }
pub fn invalidate_chat() { crate::pet_chat::invalidate(); }
pub fn follow_pet() { crate::pet_chat::follow(); }
pub fn reset_chat_size() { update_settings(|s| s.chat_size = None); crate::pet_chat::resize_to_settings(); }

pub fn flash(parts: Vec<PetTextPart>) {
    let c = &mut app().pet.chat;
    c.flash = Some((parts, std::time::Instant::now()));
    c.stick = true;
    if let Some(w) = &app().pet.pet { unsafe { SetTimer(Some(w.hwnd), T_FLASH, 7000, None); } }
    invalidate_chat();
}

pub fn flash_text(s: &str) { flash(vec![PetTextPart::new(s)]); }

// ---------- sign in ----------

pub fn sign_in() { if let Some(s) = session() { s.sign_in(); } }
pub fn sign_out() { if let Some(s) = session() { s.sign_out(); } }

// ---------- sending ----------

pub fn voice_input_started() {}

pub fn send(text: &str, completion: impl FnOnce(Option<String>) + Send + 'static) {
    voice_input_started();
    cancel_composer_recording();
    if let Some(s) = session() { s.send(text, false, Some(Box::new(completion))); }
}

pub fn queue_send(text: &str, completion: impl FnOnce(Option<String>) + Send + 'static) {
    voice_input_started();
    cancel_composer_recording();
    if let Some(s) = session() { s.send_or_queue(text, Some(Box::new(completion))); }
}

/// Sends `text` once the chat is connected and Nacho is free (about 30 seconds at most); when it still cannot go, the
/// text waits in the input field so nothing said is lost.
pub fn send_when_ready(text: String, voice: bool, attempt: u32) {
    let Some(s) = session() else { return };
    let t = text.clone();
    s.send(&text, voice, Some(Box::new(move |refusal| push(Event::SendResult { text: t, voice, attempt, refusal }))));
}

fn on_send_result(text: String, voice: bool, attempt: u32, refusal: Option<String>) {
    let Some(r) = refusal else { invalidate_chat(); return };
    let retry = r == crate::core::pet::chat_client::SendRefusal::NotConnected.korean_message() || r == crate::core::pet::chat_client::SendRefusal::Busy.korean_message();
    if retry && attempt < 60 {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            push(Event::Retry { text, voice, attempt: attempt + 1 });
        });
        return;
    }
    open_chat();
    let c = &mut app().pet.chat;
    c.input = text;
    c.cursor = c.input.chars().count();
    flash_text(&r);
}

// ---------- cards ----------

/// Carries out what the card flow decided.
pub fn card_step(step: CardStep) {
    let CardStep::Send { card_id, answer } = step else { invalidate_chat(); return };
    let Some(s) = session() else { return };
    let id = card_id.clone();
    s.answer_card(&card_id, &answer.action, answer.option_ids.clone(), answer.allow_rule_id.clone(), Some(Box::new(move |refusal| push(Event::CardAnswered { card_id: id, refusal }))));
    invalidate_chat();
}

/// The server answered a card answer: forget it, or keep the refusal for "Try again", and send the chosen text on.
fn card_answered(card_id: &str, refusal: Option<String>) {
    let p = &mut app().pet;
    let answer = p.flow.local(card_id).last_answer.unwrap_or_default();
    if let Some(t) = p.flow.finished(card_id, &answer, refusal) { send_when_ready(t, false, 0); }
    invalidate_chat();
}

pub fn card_by_id(id: &str) -> Option<crate::core::pet::server_models::PetCard> { app().pet.state.chat.card(id).cloned() }

/// A `nachopet://` link of the transcript.
pub fn on_link(url: &str) {
    use crate::core::pet::auth::percent_decode;
    let Some(rest) = url.strip_prefix("nachopet://") else { crate::app::open_url(url); return };
    let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
    let q: std::collections::HashMap<String, String> = query.split('&').filter_map(|kv| kv.split_once('=')).filter_map(|(k, v)| Some((k.to_string(), percent_decode(v)?))).collect();
    let Some(s) = session() else { return };
    match action {
        "older" => s.load_older_history(),
        "cancelqueued" => s.cancel_queued(),
        "forget" => if let Some(u) = q.get("u") { s.forget_memory(u) },
        "undo" => if let Some(u) = q.get("u") { s.undo_forget(u) },
        "memfb" => if let Some(t) = q.get("turn") {
            app().pet.chat.memory_feedback = Some(t.clone());
            flash_text(&crate::i18n::t("Type what Nacho remembered wrong and press send."));
        },
        "card" => if let (Some(id), Some(a)) = (q.get("id"), q.get("action")) {
            s.answer_card(id, a, q.get("option").cloned().into_iter().collect(), None, Some(Box::new(|r| if let Some(r) = r { push(Event::Flash(r)) })));
        },
        "task" => if let (Some(id), Some(a)) = (q.get("id"), q.get("action")) {
            match a.as_str() { "start" => s.start_task(id, None), "cancel" => s.cancel_task(id, None), _ => {} }
        },
        "cardrow" => if let (Some(id), Some(n)) = (q.get("id"), q.get("n").and_then(|n| n.parse::<usize>().ok())) {
            if let Some(c) = card_by_id(id) { let step = app().pet.flow.row(&c, n); card_step(step); }
        },
        "cardconfirm" => if let Some(id) = q.get("id") { if let Some(c) = card_by_id(id) { let step = app().pet.flow.confirm(&c); card_step(step); } },
        "cardretry" => if let Some(id) = q.get("id") { if let Some(c) = card_by_id(id) { let step = app().pet.flow.retry(&c); card_step(step); } },
        "micsettings" => crate::app::open_url("ms-settings:privacy-microphone"),
        _ => {}
    }
}

// ---------- files ----------

pub fn attach_file(path: &std::path::Path) {
    use crate::core::pet::media::PetMedia;
    let Ok(meta) = std::fs::metadata(path) else { open_chat(); flash_text("파일을 읽지 못했어요."); return };
    if !meta.is_file() { open_chat(); flash_text("폴더는 보낼 수 없어요. 파일을 골라 주세요."); return; }
    if meta.len() > PetMedia::MAX_UPLOAD_BYTES as u64 * 3 { open_chat(); flash_text(&crate::core::pet::media::MediaProblem::TooLarge.korean_message()); return; }
    let Ok(data) = std::fs::read(path) else { open_chat(); flash_text("파일을 읽지 못했어요."); return };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match PetMedia::prepare(&name, data, None) {
        Err(p) => { open_chat(); flash_text(&p.korean_message()); }
        Ok(file) => {
            let refusal = session().and_then(|s| s.attach(file));
            open_chat();
            if let Some(r) = refusal { flash_text(&r); }
        }
    }
}

pub fn pick_files() {
    use windows::Win32::UI::Controls::Dialogs::*;
    let mut buf = vec![0u16; 8192];
    let filter: Vec<u16> = "Pictures, PDF, text, mp4\0*.png;*.jpg;*.jpeg;*.gif;*.webp;*.pdf;*.txt;*.md;*.csv;*.mp4\0All files\0*.*\0\0".encode_utf16().collect();
    let owner = app().pet.chat.hwnd.map(|h| HWND(h as *mut _));
    let mut ofn = OPENFILENAMEW { lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32, hwndOwner: owner.unwrap_or_default(), lpstrFilter: PCWSTR(filter.as_ptr()), lpstrFile: PWSTR(buf.as_mut_ptr()), nMaxFile: buf.len() as u32,
        Flags: OFN_ALLOWMULTISELECT | OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR, ..Default::default() };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() { return; }
    let parts: Vec<String> = buf.split(|c| *c == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
    let files: Vec<std::path::PathBuf> = match parts.as_slice() {
        [] => vec![],
        [one] => vec![one.into()],
        [dir, names @ ..] => names.iter().map(|n| std::path::Path::new(dir).join(n)).collect(),
    };
    for f in files.into_iter().take(crate::core::pet::media::PetMedia::MAX_ATTACHMENTS) { attach_file(&f); }
}

// ---------- recording: the microphone button and hold to talk ----------

fn dictation_max_seconds() -> u32 { app().pet.state.config.as_ref().map_or(60, |c| c.dictation_max_sec.clamp(1, 600) as u32) }

fn can_hold_to_talk() -> bool { let p = &app().pet; p.state.auth == PetAuthPhase::SignedIn && p.recorder.is_none() && !p.dictation.is_listening() }

pub fn is_recording() -> bool { app().pet.recorder.is_some() }

fn start_recorder(mode: RecMode, existing: &str) {
    let p = &mut app().pet;
    p.dictation = DictationState::new();
    p.dictation.start(existing);
    p.dictation.permission_granted();
    let hold = mode == RecMode::Hold;
    p.rec_mode = Some((mode, existing.to_string()));
    p.recorder = Some(Recorder::start(dictation_max_seconds(), Box::new(move |end| push(Event::Recorded { end, hold }))));
    refresh_face();
    request_frames();
    invalidate_chat();
}

/// The microphone button of the chat: starts, or ends and hands the audio to the server.
pub fn toggle_dictation(existing: &str) {
    if let Some(r) = &app().pet.recorder { r.stop(); return; }
    if app().pet.state.config.as_ref().is_some_and(|c| !c.dictation_enabled) && app().pet.state.auth == PetAuthPhase::SignedIn {
        flash_text(PetVoiceText::UNAVAILABLE);
        return;
    }
    start_recorder(RecMode::Composer, existing);
}

pub fn cancel_composer_recording() {
    let p = &mut app().pet;
    if p.rec_mode.as_ref().is_some_and(|m| m.0 == RecMode::Composer) { if let Some(r) = p.recorder.take() { r.cancel(); } p.rec_mode = None; p.dictation.stop(); }
}

fn begin_hold_recording() {
    open_chat();
    start_recorder(RecMode::Hold, "");
}

fn finish_hold_recording(seconds: f64) {
    let p = &mut app().pet;
    if !p.rec_mode.as_ref().is_some_and(|m| m.0 == RecMode::Hold) { return; }
    if seconds < crate::core::pet::voice::PetVoice::MINIMUM_SECONDS { cancel_hold_recording(PetVoiceText::TOO_SHORT); return; }
    if let Some(r) = &p.recorder { r.stop(); }
}

fn cancel_hold_recording(message: &str) {
    let p = &mut app().pet;
    if !p.rec_mode.as_ref().is_some_and(|m| m.0 == RecMode::Hold) { return; }
    if let Some(r) = p.recorder.take() { r.cancel(); }
    p.rec_mode = None;
    p.dictation.stop();
    flash_text(message);
    refresh_face();
}

/// Esc while recording.
pub fn escape_recording() -> bool {
    let p = &mut app().pet;
    if p.rec_mode.is_none() { return false; }
    p.gesture.escape();
    if p.rec_mode.as_ref().is_some_and(|m| m.0 == RecMode::Hold) { cancel_hold_recording(PetVoiceText::CANCELED); } else { cancel_composer_recording(); }
    request_frames();
    invalidate_chat();
    true
}

/// For `ctl petui dictate <file.wav> [hold]`: a 16 kHz mono 16-bit WAV goes the way a recording does (the microphone is
/// the only thing skipped), so the dictation request can be tried on a PC without a microphone.
pub fn dictate_from_wav(path: &str, hold: bool) {
    let Ok(bytes) = std::fs::read(path) else { return };
    if bytes.len() < 44 { return; }
    let samples: Vec<i16> = bytes[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    app().pet.rec_mode = Some((if hold { RecMode::Hold } else { RecMode::Composer }, String::new()));
    on_recorded(RecordEnd::Finished(samples), hold);
}

fn on_recorded(end: RecordEnd, hold: bool) {
    let p = &mut app().pet;
    let mode = p.rec_mode.take();
    p.recorder = None;
    p.dictation.stop();
    p.gesture.escape();
    refresh_face();
    let existing = mode.map(|m| m.1).unwrap_or_default();
    match end {
        RecordEnd::Canceled => {}
        RecordEnd::Denied => {
            p.dictation.permission_denied(DictationDenial::Microphone);
            open_chat();
            flash(vec![PetTextPart::new(PetVoiceText::MIC_DENIED), PetTextPart::styled(" · ", crate::core::pet::transcript::PetTextStyle::Secondary),
                PetTextPart::link(PetVoiceText::OPEN_SETTINGS, crate::core::pet::transcript::PetLinks::url("micsettings", &[]))]);
        }
        RecordEnd::Finished(samples) => {
            if !wav::has_sound(&samples) { open_chat(); flash_text(PetVoiceText::SILENCE); return; }
            let seconds = wav::seconds(samples.len(), crate::pet_audio::SAMPLE_RATE);
            let audio = wav::encode_pcm16_mono(&samples, crate::pet_audio::SAMPLE_RATE);
            let max = dictation_max_seconds() as f64;
            let Some(s) = session() else { return };
            p.state.dictating = true;
            let _ = existing;
            s.dictate(audio, wav::MIME, wav::FILE_NAME, Some(seconds.min(max)), Box::new(move |result| push(Event::Dictated { hold, result, seconds })));
            invalidate_chat();
        }
    }
}

fn on_dictated(hold: bool, result: std::result::Result<String, PetAPIError>, _seconds: f64) {
    let p = &mut app().pet;
    p.dictation.stop();
    match result {
        Ok(text) => {
            let t = text.trim().to_string();
            if t.is_empty() { open_chat(); flash_text(PetVoiceText::SILENCE); }
            else if hold { send_when_ready(t, true, 0); }
            else {
                // Into the input field only; never sent by itself.
                let c = &mut p.chat;
                if !c.input.trim().is_empty() && !c.input.ends_with(' ') { c.input.push(' '); }
                c.input.push_str(&t);
                c.cursor = c.input.chars().count();
            }
        }
        Err(e) => {
            let text = if e.code.as_deref() == Some("refused") { e.text } else { PetVoiceText::dictation_failure(e.code.as_deref(), None, dictation_max_seconds() as i64) };
            open_chat();
            flash_text(&text);
        }
    }
    refresh_face();
    invalidate_chat();
}

// ---------- the settings page ----------

pub fn settings_controls(ui: &mut Ui, x: f64, y: f64, w: f64) -> f64 {
    use crate::i18n::t;
    let th = ui.th;
    let mut y = y + 12.0;
    let status = status_text();
    ui.c.line_text(&t(&status), medium(13), Rect::new(x + 18.0, y, w - 36.0, 24.0), th.ink2, Align::Left);
    y += 36.0;
    let running = is_running();
    let signed_in = app().pet.state.auth == PetAuthPhase::SignedIn;
    fn button(ui: &mut Ui, key: &str, label: &str, style: Btn, enabled: bool, bx: &mut f64, y: f64) -> bool {
        let label = crate::i18n::t(label).into_owned();
        let bw = ui.button_width(&label, true);
        let hit = ui.button_rect(key, &label, Rect::new(*bx, y, bw, 30.0), style, true, enabled);
        *bx += bw + 10.0;
        hit
    }
    let mut bx = x + 18.0;
    if button(ui, "petopen", "Open Chat", Btn::Primary, running, &mut bx, y) { later(open_chat); }
    if signed_in { if button(ui, "petout", "Sign Out", Btn::Secondary, running, &mut bx, y) { later(sign_out); } }
    else if button(ui, "petin", "Sign In", Btn::Secondary, running, &mut bx, y) { later(|| { open_chat(); sign_in(); }); }
    y += 46.0;
    bx = x + 18.0;
    if button(ui, "petpos", "Reset Position", Btn::Secondary, running, &mut bx, y) { later(reset_position); }
    if button(ui, "petsize", "Reset Chat Size", Btn::Secondary, running, &mut bx, y) { later(reset_chat_size); }
    y += 46.0;
    y += ui.label(&t("Nothing connects to the Nacho server until you open the chat. The login is stored in Windows Credential Manager, for your account only."), sans(12), x + 18.0, y, w - 36.0, th.ink2) + 14.0;
    y
}

pub fn diagnostics() -> serde_json::Value {
    let p = &app().pet;
    // Phases and counts only: never text, tokens or addresses of a login.
    serde_json::json!({ "started": p.started, "auth": format!("{:?}", p.state.auth).split('(').next(), "chat": format!("{:?}", p.state.chat.phase).split(' ').next(),
        "turns": p.state.chat.turns.len(), "chatOpen": p.chat.hwnd.is_some(), "recording": p.recorder.is_some(), "waitingForLogin": p.browser.as_ref().is_some_and(|b| b.is_waiting()) })
}
