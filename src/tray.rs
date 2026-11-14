//! Notification area icons: the nacho box, pinned tools and System Monitor's live numbers.
use crate::app::{self, app, later, State, WM_TRAY};
use crate::core::combos;
use crate::core::settings::*;
use crate::gfx::*;
use crate::{monitor, pages, panel};
use std::collections::BTreeMap;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const UID_MAIN: u32 = 1;
pub const UID_PIN: u32 = 10;
pub const UID_MONITOR: u32 = 40;

struct Entry { icon: HICON, tip: String, owned: bool }

#[derive(Default)]
pub struct Tray { entries: BTreeMap<u32, Entry>, dot_shown: Option<bool> }

pub fn taskbar_created() -> u32 {
    use std::sync::OnceLock;
    static M: OnceLock<u32> = OnceLock::new();
    *M.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

fn fill_tip(buf: &mut [u16], s: &str) {
    for (i, c) in s.encode_utf16().take(buf.len() - 1).enumerate() { buf[i] = c; }
}

fn nid(uid: u32) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: app().main, uID: uid, uCallbackMessage: WM_TRAY, ..Default::default() }
}

impl Tray {
    pub fn count(&self) -> usize { self.entries.len() }

    fn push(&self, uid: u32, e: &Entry, msg: NOTIFY_ICON_MESSAGE) -> bool {
        let mut d = nid(uid);
        d.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
        d.hIcon = e.icon;
        fill_tip(&mut d.szTip, &crate::i18n::t(&e.tip));
        unsafe {
            let ok = Shell_NotifyIconW(msg, &d).as_bool();
            if msg == NIM_ADD {
                d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
            }
            ok
        }
    }

    /// Adds or updates an icon. An owned icon is destroyed when replaced.
    pub fn set(&mut self, uid: u32, icon: HICON, tip: &str, owned: bool) {
        let new = Entry { icon, tip: tip.to_string(), owned };
        let msg = if self.entries.contains_key(&uid) { NIM_MODIFY } else { NIM_ADD };
        self.push(uid, &new, msg);
        if let Some(old) = self.entries.insert(uid, new) { if old.owned { unsafe { let _ = DestroyIcon(old.icon); } } }
    }

    pub fn remove(&mut self, uid: u32) {
        if let Some(e) = self.entries.remove(&uid) {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid(uid));
                if e.owned { let _ = DestroyIcon(e.icon); }
            }
        }
    }

    pub fn remove_all(&mut self) { for uid in self.entries.keys().copied().collect::<Vec<_>>() { self.remove(uid); } }

    /// Explorer restarted: every icon has to be added again.
    pub fn recreate(&mut self) { for (uid, e) in &self.entries { self.push(*uid, e, NIM_ADD); } }

    pub fn balloon(&self, title: &str, text: &str) {
        let mut d = nid(UID_MAIN);
        d.uFlags = NIF_INFO;
        fill_tip(&mut d.szInfoTitle, &crate::i18n::t(title));
        fill_tip(&mut d.szInfo, &crate::i18n::t(text));
        d.dwInfoFlags = NIIF_INFO;
        unsafe { let _ = Shell_NotifyIconW(NIM_MODIFY, &d); }
    }

    /// The icon's place on screen, so a panel can open beside it.
    pub fn icon_rect(&self, uid: u32) -> Option<RECT> {
        let id = NOTIFYICONIDENTIFIER { cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32, hWnd: app().main, uID: uid, ..Default::default() };
        unsafe { Shell_NotifyIconGetRect(&id).ok() }
    }

    /// Redraws the box when the attention dot changes.
    pub fn refresh_main(&mut self) {
        let a = app();
        let waiting = a.settings.attention_dot && ModuleId::ALL.iter().any(|m| matches!(a.state(*m), State::Failed(_)));
        if self.dot_shown == Some(waiting) || !self.entries.contains_key(&UID_MAIN) { return; }
        self.dot_shown = Some(waiting);
        self.set(UID_MAIN, main_icon(waiting), &main_tip(), true);
        sync_pins();
    }
}

fn main_tip() -> String {
    match combos::current(&app().settings) { Some(c) => format!("Nacho Combo: {}", c.name), None => "Nacho Combo".to_string() }
}

pub fn icon_size() -> i32 { unsafe { GetSystemMetricsForDpi(SM_CXSMICON, windows::Win32::UI::HiDpi::GetDpiForSystem()) } }

/// Straight-alpha BGRA from premultiplied pixels, as a Windows icon.
pub fn icon_from_pixels(px: &[u8], w: i32, h: i32) -> HICON {
    unsafe {
        let bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0) else { return HICON::default() };
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        for (d, s) in dst.chunks_exact_mut(4).zip(px.chunks_exact(4)) {
            let a = s[3] as u32;
            if a > 0 && a < 255 {
                d[0] = (s[0] as u32 * 255 / a).min(255) as u8;
                d[1] = (s[1] as u32 * 255 / a).min(255) as u8;
                d[2] = (s[2] as u32 * 255 / a).min(255) as u8;
            } else { d[0] = s[0]; d[1] = s[1]; d[2] = s[2]; }
            d[3] = s[3];
        }
        let mask = CreateBitmap(w, h, 1, 1, None);
        let info = ICONINFO { fIcon: BOOL(1), hbmMask: mask, hbmColor: color, ..Default::default() };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

fn main_icon(dot: bool) -> HICON {
    let n = icon_size();
    let Some((px, w, h)) = logo_pixels() else { return HICON::default() };
    let out = render_offscreen(n as u32, n as u32, 96.0, None, |c| {
        if let Some(b) = c.make_bitmap(&px, w, h) { c.bitmap(&b, crate::core::layout::Rect::new(0.0, 0.0, n as f64, n as f64), 1.0); }
        if dot {
            let r = n as f64 * 0.22;
            c.circle(n as f64 - r, n as f64 - r, r, LIGHT.nacho_deep);
        }
    }).unwrap_or_default();
    icon_from_pixels(&out, n, n)
}

pub fn create_main() {
    let dot = app().settings.attention_dot && false;
    let tip = main_tip();
    app().tray.set(UID_MAIN, main_icon(dot), &tip, true);
    sync_pins();
}

/// A tool's own small icon, for pins.
fn tool_icon(id: ModuleId) -> HICON {
    let n = icon_size();
    let dark = !crate::window::is_dark();
    // The taskbar has its own colour: the mark follows the theme of the taskbar, not the app.
    let ink = if taskbar_is_light() && dark { rgb(0x24180D) } else if taskbar_is_light() { rgb(0x24180D) } else { rgb(0xFFFFFF) };
    let px = render_offscreen(n as u32, n as u32, 96.0, None, |c| crate::ui::gadget_icon(c, id, 0.0, 0.0, n as f64, ink)).unwrap_or_default();
    icon_from_pixels(&px, n, n)
}

pub fn taskbar_is_light() -> bool {
    use windows::Win32::System::Registry::*;
    unsafe {
        let (mut v, mut n) = (0u32, 4u32);
        RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("SystemUsesLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as *mut _), Some(&mut n)).is_ok() && v == 1
    }
}

pub fn sync_pins() {
    let a = app();
    for (i, id) in ModuleId::ALL.into_iter().enumerate() {
        let uid = UID_PIN + i as u32;
        let pinned = a.settings.pinned_tools.contains(&id);
        if pinned && !a.tray.entries.contains_key(&uid) { a.tray.set(uid, tool_icon(id), id.name(), true); }
        if !pinned { a.tray.remove(uid); }
    }
}

/// Kept for symmetry with the Mac app, where item order matters. On Windows it only makes sure the pins exist.
pub fn resync_order() { sync_pins(); }

pub fn on_message(wp: WPARAM, lp: LPARAM) {
    const NIN_SELECT: u32 = WM_USER;
    const NIN_KEYSELECT: u32 = WM_USER + 1;
    let event = (lp.0 & 0xFFFF) as u32;
    let uid = ((lp.0 >> 16) & 0xFFFF) as u32;
    let _ = wp;
    match event {
        NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONUP => {
            if uid == UID_MAIN { later(panel::toggle_list); }
            else if (UID_PIN..UID_PIN + ModuleId::ALL.len() as u32).contains(&uid) { let m = ModuleId::ALL[(uid - UID_PIN) as usize]; later(move || panel::open_tool(m)); }
            else if uid == crate::awake::UID { later(crate::awake::toggle); }
            else if uid == crate::clock::UID { later(crate::clock::toggle_popup); }
            else if uid == crate::devices::UID_MIC { later(crate::devices::toggle_mic); }
            else if uid >= UID_MONITOR { let i = (uid - UID_MONITOR) as usize; later(move || monitor::toggle_panel(MonitorItem::ALL[i])); }
        }
        WM_CONTEXTMENU if uid == crate::awake::UID => { later(crate::awake::menu); }
        WM_CONTEXTMENU => { later(show_menu); }
        _ => {}
    }
}

fn show_menu() {
    unsafe {
        let a = app();
        let Ok(menu) = CreatePopupMenu() else { return };
        let Ok(sub) = CreatePopupMenu() else { return };
        let all = combos::all(&a.settings);
        let cur = combos::current(&a.settings).map(|c| c.id);
        for (i, c) in all.iter().enumerate() {
            let flags = if cur.as_ref() == Some(&c.id) { MF_STRING | MF_CHECKED } else { MF_STRING };
            let _ = AppendMenuW(sub, flags, 100 + i, &HSTRING::from(format!("No.{} {}", c.number, c.name)));
        }
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, &HSTRING::from(crate::i18n::t("Combo").as_ref()));
        let _ = AppendMenuW(menu, MF_STRING, 1, &HSTRING::from(crate::i18n::t("Settings…").as_ref()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, 2, &HSTRING::from(crate::i18n::t("Quit Nacho Combo").as_ref()));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        // A tray menu only closes on an outside click when its owner is the foreground window.
        let _ = SetForegroundWindow(a.main);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, p.x, p.y, None, a.main, None).0 as usize;
        let _ = PostMessageW(Some(a.main), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match cmd {
            1 => pages::open(None),
            2 => app::quit(),
            n if n >= 100 => { if let Some(c) = all.get(n - 100) { app().order(c); } }
            _ => {}
        }
    }
}

/// The taskbar changed between light and dark: icons drawn for its colour are drawn again.
pub fn refresh_theme() {
    let a = app();
    for (i, id) in ModuleId::ALL.into_iter().enumerate() { if a.settings.pinned_tools.contains(&id) { a.tray.remove(UID_PIN + i as u32); } }
    sync_pins();
    crate::monitor::redraw_tray();
    crate::clock::redraw_tray();
}

pub fn exe_module() -> HINSTANCE { unsafe { GetModuleHandleW(None).unwrap_or_default().into() } }
