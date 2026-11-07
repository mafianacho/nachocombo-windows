//! A top-level window whose content is drawn by the immediate-mode UI.
use crate::gfx::*;
use crate::ui::*;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const CLASS: PCWSTR = w!("NachoComboWindow");
const WM_MOUSELEAVE: u32 = 0x02A3;

pub type DrawFn = Box<dyn FnMut(&mut Ui, &mut WindowCtx)>;
/// Handles a raw message first; `Some` ends it there.
pub type MsgFn = Box<dyn FnMut(HWND, u32, WPARAM, LPARAM) -> Option<LRESULT>>;

/// What a draw function can ask of its window.
pub struct WindowCtx { pub hwnd: HWND, pub close: bool, pub scale: f64 }

pub struct UiWindow {
    pub hwnd: HWND,
    pub st: UiState,
    draw: DrawFn,
    msg: Option<MsgFn>,
    rt: Option<ID2D1HwndRenderTarget>,
    canvas: Option<Canvas>,
    pub clear: Option<Color>,
    /// A popup closes when it loses the focus.
    pub close_on_deactivate: bool,
    pub on_close: Option<Box<dyn FnMut()>>,
}

pub fn register_class() {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, style: CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW, lpfnWndProc: Some(proc), hInstance: hinst.into(),
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(1 as _)).unwrap_or_default(), hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), lpszClassName: CLASS, ..Default::default() };
        RegisterClassExW(&wc);
    }
}

pub fn dpi_scale(hwnd: HWND) -> f64 { unsafe { GetDpiForWindow(hwnd) as f64 / 96.0 } }

pub fn is_dark() -> bool {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut v = 1u32;
        let mut n = 4u32;
        let r = RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("AppsUseLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as *mut _), Some(&mut n));
        r.is_ok() && v == 0
    }
}

pub fn theme() -> Theme { if is_dark() { DARK } else { LIGHT } }

/// Follows the Windows theme on the title bar and border.
pub fn apply_chrome(hwnd: HWND) {
    unsafe {
        let th = theme();
        let dark = BOOL(th.dark as i32);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
        let col = |c: Color| (c.0 * 255.0) as u32 | ((c.1 * 255.0) as u32) << 8 | ((c.2 * 255.0) as u32) << 16;
        let cap = col(th.paper);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &cap as *const _ as *const _, 4);
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
    }
}

impl UiWindow {
    /// `w`, `h` in device-independent pixels; the window owns itself until it is destroyed.
    pub fn create(title: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, x: i32, y: i32, w: f64, h: f64, draw: DrawFn) -> &'static mut UiWindow {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let win = Box::leak(Box::new(UiWindow { hwnd: HWND::default(), st: UiState::default(), draw, msg: None, rt: None, canvas: None, clear: None, close_on_deactivate: false, on_close: None }));
            let t = HSTRING::from(title);
            let hwnd = CreateWindowExW(ex, CLASS, &t, style, x, y, 100, 100, None, None, Some(hinst.into()), Some(win as *mut _ as *const _)).expect("CreateWindowExW");
            win.hwnd = hwnd;
            apply_chrome(hwnd);
            let s = dpi_scale(hwnd);
            let mut rc = RECT { left: 0, top: 0, right: (w * s) as i32, bottom: (h * s) as i32 };
            let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(&mut rc, style, false, ex, GetDpiForWindow(hwnd));
            let _ = SetWindowPos(hwnd, None, 0, 0, rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            win
        }
    }

    pub fn set_msg(&mut self, f: MsgFn) { self.msg = Some(f); }
    pub fn invalidate(&self) { unsafe { let _ = InvalidateRect(Some(self.hwnd), None, false); } }
    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }
    pub fn close(&self) { unsafe { let _ = DestroyWindow(self.hwnd); } }

    fn ensure_target(&mut self) -> Result<()> {
        if self.rt.is_some() { return Ok(()); }
        unsafe {
            let mut rc = RECT::default();
            GetClientRect(self.hwnd, &mut rc)?;
            let dpi = GetDpiForWindow(self.hwnd) as f32;
            let props = D2D1_RENDER_TARGET_PROPERTIES { r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT, pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE }, dpiX: dpi, dpiY: dpi, usage: D2D1_RENDER_TARGET_USAGE_NONE, minLevel: D2D1_FEATURE_LEVEL_DEFAULT };
            let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd: self.hwnd, pixelSize: D2D_SIZE_U { width: rc.right as u32, height: rc.bottom as u32 }, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
            let rt = gfx().d2d.CreateHwndRenderTarget(&props, &hp)?;
            self.canvas = Some(Canvas::new(rt.cast()?)?);
            self.rt = Some(rt);
        }
        Ok(())
    }

    fn paint(&mut self) {
        if self.ensure_target().is_err() { return; }
        let hwnd = self.hwnd;
        let scale = dpi_scale(hwnd);
        let mut rc = RECT::default();
        unsafe { let _ = GetClientRect(hwnd, &mut rc); }
        let (w, h) = (rc.right as f64 / scale, rc.bottom as f64 / scale);
        let mut ctx = WindowCtx { hwnd, close: false, scale };
        let th = theme();
        let rt = self.rt.clone().unwrap();
        let canvas = self.canvas.take().unwrap();
        unsafe { rt.BeginDraw(); }
        canvas.clear(self.clear.unwrap_or(th.paper));
        {
            let mut ui = Ui::new(&canvas, &mut self.st, th, w, h);
            (self.draw)(&mut ui, &mut ctx);
            ui.finish();
        }
        let end = unsafe { rt.EndDraw(None, None) };
        self.canvas = Some(canvas);
        if let Err(e) = end {
            if e.code() == D2DERR_RECREATE_TARGET { self.rt = None; self.canvas = None; self.invalidate(); }
        }
        if ctx.close { self.close(); return; }
        if std::mem::take(&mut self.st.again) { self.invalidate(); }
    }

    /// The window's current picture, for `ctl` snapshots.
    pub fn snapshot(&mut self, path: &std::path::Path) -> Result<()> {
        let hwnd = self.hwnd;
        let scale = dpi_scale(hwnd);
        let mut rc = RECT::default();
        unsafe { GetClientRect(hwnd, &mut rc)?; }
        let (w, h) = (rc.right as f64 / scale, rc.bottom as f64 / scale);
        let th = theme();
        let bg = self.clear.unwrap_or(th.paper);
        let mut ctx = WindowCtx { hwnd, close: false, scale };
        let (draw, st) = (&mut self.draw, &mut self.st);
        // Twice, so that scroll areas know their height.
        for _ in 0..2 { render_offscreen(rc.right as u32, rc.bottom as u32, 96.0 * scale as f32, Some(bg), |c| { let mut ui = Ui::new(c, st, th, w, h); draw(&mut ui, &mut ctx); ui.finish(); })?; }
        let px = render_offscreen(rc.right as u32, rc.bottom as u32, 96.0 * scale as f32, Some(bg), |c| { let mut ui = Ui::new(c, st, th, w, h); draw(&mut ui, &mut ctx); ui.finish(); })?;
        save_png(path, &px, rc.right as u32, rc.bottom as u32)
    }
}

fn mods() -> Key {
    unsafe {
        let d = |k: VIRTUAL_KEY| GetKeyState(k.0 as i32) < 0;
        Key { vk: 0, ctrl: d(VK_CONTROL), shift: d(VK_SHIFT), alt: d(VK_MENU), win: d(VK_LWIN) || d(VK_RWIN) }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
        let w = (*cs).lpCreateParams as *mut UiWindow;
        (*w).hwnd = hwnd;
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut UiWindow;
    if ptr.is_null() { return DefWindowProcW(hwnd, msg, wp, lp); }
    let win = &mut *ptr;
    if let Some(f) = win.msg.as_mut() { if let Some(r) = f(hwnd, msg, wp, lp) { return r; } }
    let scale = dpi_scale(hwnd);
    let pos = |lp: LPARAM| ((lp.0 & 0xFFFF) as i16 as f64 / scale, ((lp.0 >> 16) & 0xFFFF) as i16 as f64 / scale);
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            win.paint();
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            if let Some(rt) = &win.rt {
                let _ = rt.Resize(&D2D_SIZE_U { width: (lp.0 & 0xFFFF) as u32, height: ((lp.0 >> 16) & 0xFFFF) as u32 });
            }
            win.invalidate();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            win.rt = None;
            win.canvas = None;
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            win.invalidate();
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => { apply_chrome(hwnd); win.invalidate(); LRESULT(0) }
        WM_MOUSEMOVE => {
            let p = pos(lp);
            if !win.st.inside {
                let mut t = TRACKMOUSEEVENT { cbSize: 24, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut t);
            }
            win.st.inside = true;
            win.st.mouse = p;
            win.st.keyboard_used = false;
            if win.st.hit_at(p.0, p.1) != win.st.hot || win.st.active.is_some() || win.st.down { win.st.hot = win.st.hit_at(p.0, p.1); win.invalidate(); }
            LRESULT(0)
        }
        WM_MOUSELEAVE => { win.st.inside = false; win.st.hot = None; win.invalidate(); LRESULT(0) }
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            let c = if win.st.cursor_hand { IDC_HAND } else if win.st.cursor_text { IDC_IBEAM } else { IDC_ARROW };
            SetCursor(LoadCursorW(None, c).ok());
            LRESULT(1)
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            win.st.down = true;
            win.st.mouse = pos(lp);
            win.st.double_click = msg == WM_LBUTTONDBLCLK;
            win.st.hot = win.st.hit_at(win.st.mouse.0, win.st.mouse.1);
            SetCapture(hwnd);
            if win.close_on_deactivate { let _ = SetFocus(Some(hwnd)); }
            win.invalidate();
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            win.st.down = false;
            win.st.mouse = pos(lp);
            win.st.click = Some(win.st.mouse);
            win.invalidate();
            LRESULT(0)
        }
        WM_RBUTTONUP => { win.st.right_click = Some(pos(lp)); win.invalidate(); LRESULT(0) }
        WM_MOUSEWHEEL => { win.st.wheel += ((wp.0 >> 16) & 0xFFFF) as i16 as f64 / 120.0 * 60.0; win.invalidate(); LRESULT(0) }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u32;
            let mut k = mods();
            k.vk = vk;
            win.st.keys.push(k);
            win.st.keyboard_used = true;
            if win.st.capture_key.is_none() {
                match vk {
                    0x09 => win.st.tab = if k.shift { -1 } else { 1 },
                    0x20 | 0x0D => win.st.activate = true,
                    _ => {}
                }
            }
            win.invalidate();
            if msg == WM_SYSKEYDOWN && vk != 0x12 && win.st.capture_key.is_none() { return DefWindowProcW(hwnd, msg, wp, lp); }
            LRESULT(0)
        }
        WM_SYSCHAR if win.st.capture_key.is_some() => LRESULT(0),
        WM_CHAR => {
            if let Some(c) = char::from_u32(wp.0 as u32) { win.st.chars.push(c); }
            win.invalidate();
            LRESULT(0)
        }
        WM_ACTIVATE if win.close_on_deactivate && (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => { let _ = DestroyWindow(hwnd); LRESULT(0) }
        WM_DESTROY => {
            if let Some(f) = win.on_close.as_mut() { f(); }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            // The box leaked in `create` ends with its window.
            drop(Box::from_raw(ptr));
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
