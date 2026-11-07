//! Clipboard History: listens for copies, keeps them in memory (pins on disk) and pastes them back on request.
use crate::app::{app, later, State};
use crate::capture::copy_text;
use crate::core::layout::Rect;
use crate::core::settings::ModuleId;
use crate::gfx::*;
use crate::ui::*;
use crate::window::{UiWindow, WindowCtx};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIB: u32 = 8;
const MAX_IMAGE: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Content { Text(String), Files(Vec<PathBuf>), Image { dib: Vec<u8>, w: u32, h: u32 } }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip { pub content: Content, pub source: String, #[serde(default)] pub pinned: bool }

impl Clip {
    pub fn title(&self) -> String {
        match &self.content {
            Content::Text(t) => t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect(),
            Content::Files(f) => if f.len() == 1 { f[0].file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default() } else { format!("{} files", f.len()) },
            Content::Image { w, h, .. } => format!("Image {w} × {h}"),
        }
    }
    fn matches(&self, q: &str) -> bool {
        if q.is_empty() { return true; }
        let q = q.to_lowercase();
        match &self.content {
            Content::Text(t) => t.to_lowercase().contains(&q),
            Content::Files(f) => f.iter().any(|p| p.to_string_lossy().to_lowercase().contains(&q)),
            Content::Image { .. } => "image".contains(&q),
        }
    }
}

#[derive(Default)]
pub struct ClipModule {
    pub items: Vec<Clip>,
    pub listening: bool,
    pub recorded: u32,
    pub skipped_private: u32,
    window: Option<isize>,
    /// The window that was in front before the list opened: pastes go there.
    target: Option<isize>,
    query: String,
    selected: usize,
    /// Our own write to the clipboard, which must not be recorded again.
    ignore_next: bool,
}

impl ClipModule {
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "items": self.items.len(), "pins": self.items.iter().filter(|c| c.pinned).count(), "listening": self.listening, "recorded": self.recorded, "skippedPrivate": self.skipped_private, "open": self.window.is_some() })
    }
    pub fn is_open(&self) -> bool { self.window.is_some() }
}

fn pins_path() -> PathBuf { app().dir.join("clipboard-pins.json") }

fn save_pins() {
    let pins: Vec<&Clip> = app().clipboard.items.iter().filter(|c| c.pinned).collect();
    let _ = std::fs::write(pins_path(), serde_json::to_vec(&pins).unwrap_or_default());
}

pub fn start() -> std::result::Result<(), String> {
    let a = app();
    let pins: Vec<Clip> = std::fs::read(pins_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    a.clipboard.items = pins;
    unsafe { AddClipboardFormatListener(a.main).map_err(|e| format!("Windows refused to report copies: {e}"))?; }
    a.clipboard.listening = true;
    Ok(())
}

pub fn stop() {
    let a = app();
    close();
    if a.clipboard.listening { unsafe { let _ = RemoveClipboardFormatListener(a.main); } }
    a.clipboard = ClipModule::default();
}

unsafe fn format_id(name: PCWSTR) -> u32 { RegisterClipboardFormatW(name) }

unsafe fn global(fmt: u32) -> Option<Vec<u8>> {
    let h = GetClipboardData(fmt).ok()?;
    let g = HGLOBAL(h.0);
    let p = GlobalLock(g) as *const u8;
    if p.is_null() { return None; }
    let n = GlobalSize(g);
    let v = std::slice::from_raw_parts(p, n).to_vec();
    let _ = GlobalUnlock(g);
    Some(v)
}

fn wide(b: &[u8]) -> String {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
    String::from_utf16_lossy(&w)
}

fn source_app() -> String {
    unsafe {
        let h = GetClipboardOwner().unwrap_or_default();
        let mut pid = 0;
        GetWindowThreadProcessId(h, Some(&mut pid));
        crate::monitor_data::process_name(pid).unwrap_or_default()
    }
}

/// A copy happened somewhere. Private copies (password managers set these formats) are never read.
pub fn on_update() {
    let a = app();
    if a.state(ModuleId::Clipboard) != State::Running { return; }
    if std::mem::take(&mut a.clipboard.ignore_next) { return; }
    let clip = unsafe {
        let private = IsClipboardFormatAvailable(format_id(w!("ExcludeClipboardContentFromMonitorProcessing"))).is_ok()
            || IsClipboardFormatAvailable(format_id(w!("Clipboard Viewer Ignore"))).is_ok()
            || (IsClipboardFormatAvailable(format_id(w!("CanIncludeInClipboardHistory"))).is_ok() && {
                let ok = OpenClipboard(Some(a.main)).is_ok();
                let v = if ok { global(format_id(w!("CanIncludeInClipboardHistory"))) } else { None };
                if ok { let _ = CloseClipboard(); }
                v.is_some_and(|b| b.len() >= 4 && u32::from_le_bytes([b[0], b[1], b[2], b[3]]) == 0)
            });
        if private { a.clipboard.skipped_private += 1; return; }
        let mut tries = 0;
        while OpenClipboard(Some(a.main)).is_err() {
            tries += 1;
            if tries > 5 { return; }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let content = if IsClipboardFormatAvailable(CF_HDROP).is_ok() {
            GetClipboardData(CF_HDROP).ok().map(|h| {
                let drop = HDROP(h.0);
                let n = DragQueryFileW(drop, 0xFFFFFFFF, None);
                Content::Files((0..n).map(|i| { let mut b = vec![0u16; 32768]; let l = DragQueryFileW(drop, i, Some(&mut b)) as usize; PathBuf::from(String::from_utf16_lossy(&b[..l])) }).collect())
            })
        } else if IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok() {
            global(CF_UNICODETEXT).map(|b| Content::Text(wide(&b))).filter(|c| !matches!(c, Content::Text(t) if t.trim().is_empty()))
        } else if IsClipboardFormatAvailable(CF_DIB).is_ok() {
            global(CF_DIB).filter(|d| d.len() <= MAX_IMAGE && d.len() >= 40).map(|dib| {
                let w = i32::from_le_bytes([dib[4], dib[5], dib[6], dib[7]]).unsigned_abs();
                let h = i32::from_le_bytes([dib[8], dib[9], dib[10], dib[11]]).unsigned_abs();
                Content::Image { dib, w, h }
            })
        } else { None };
        let _ = CloseClipboard();
        content.map(|c| Clip { content: c, source: source_app(), pinned: false })
    };
    let Some(clip) = clip else { return };
    let m = &mut a.clipboard;
    // A repeat moves to the top instead of appearing twice.
    if let Some(i) = m.items.iter().position(|x| x.content == clip.content) {
        let old = m.items.remove(i);
        m.items.insert(0, Clip { pinned: old.pinned, ..clip });
    } else { m.items.insert(0, clip); }
    m.recorded += 1;
    let max = a.settings.options.clip_max.max(10) as usize;
    let mut unpinned = 0;
    m.items.retain(|c| { if c.pinned { return true; } unpinned += 1; unpinned <= max });
    let open = m.window;
    a.changed();
    if let Some(h) = open { unsafe { let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(HWND(h as *mut _)), None, false); } }
}

fn visible() -> Vec<usize> {
    let m = &app().clipboard;
    let mut v: Vec<usize> = (0..m.items.len()).filter(|i| m.items[*i].matches(&m.query)).collect();
    v.sort_by_key(|i| !m.items[*i].pinned);
    v
}

pub fn show() {
    let a = app();
    if a.state(ModuleId::Clipboard) != State::Running { return; }
    if let Some(h) = a.clipboard.window { unsafe { let _ = SetForegroundWindow(HWND(h as *mut _)); } return; }
    let fg = unsafe { GetForegroundWindow() };
    a.clipboard.target = Some(fg.0 as isize);
    a.clipboard.query.clear();
    a.clipboard.selected = 0;
    let mut thumbs: std::collections::HashMap<usize, Option<ID2D1Bitmap>> = Default::default();
    let win = UiWindow::create("Clipboard History", WS_POPUP, WS_EX_TOOLWINDOW | WS_EX_TOPMOST, 0, 0, 380.0, 460.0, Box::new(move |ui, ctx| draw(ui, ctx, &mut thumbs)));
    win.close_on_deactivate = true;
    win.on_close = Some(Box::new(|| { app().clipboard.window = None; }));
    let h = win.hwnd;
    a.clipboard.window = Some(h.0 as isize);
    unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let mut mi = windows::Win32::Graphics::Gdi::MONITORINFO { cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32, ..Default::default() };
        let _ = windows::Win32::Graphics::Gdi::GetMonitorInfoW(windows::Win32::Graphics::Gdi::MonitorFromPoint(p, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST), &mut mi);
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let (w, hh) = (r.right - r.left, r.bottom - r.top);
        let x = p.x.min(mi.rcWork.right - w).max(mi.rcWork.left);
        let y = p.y.min(mi.rcWork.bottom - hh).max(mi.rcWork.top);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE);
    }
    win.show();
}

pub fn close() { if let Some(h) = app().clipboard.window.take() { unsafe { let _ = DestroyWindow(HWND(h as *mut _)); } } }

fn draw(ui: &mut Ui, ctx: &mut WindowCtx, thumbs: &mut std::collections::HashMap<usize, Option<ID2D1Bitmap>>) {
    let th = ui.th;
    let m = &mut app().clipboard;
    let vis = visible();
    if m.selected >= vis.len().max(1) { m.selected = vis.len().saturating_sub(1); }
    // Typing goes to the search field; the list is driven by the keys.
    for c in ui.st.chars.clone() {
        if c == '\u{8}' { m.query.pop(); } else if !c.is_control() { m.query.push(c); }
        m.selected = 0;
        thumbs.clear();
    }
    ui.st.chars.clear();
    for k in ui.st.keys.clone() {
        match (k.vk, k.ctrl, k.shift) {
            (0x1B, _, _) => ctx.close = true,
            (0x28, _, _) => m.selected = (m.selected + 1).min(vis.len().saturating_sub(1)),
            (0x26, _, _) => m.selected = m.selected.saturating_sub(1),
            (0x0D, _, shift) => if let Some(i) = vis.get(m.selected).copied() { ctx.close = true; later(move || paste(i, shift)); },
            (0x31..=0x39, true, _) => if let Some(i) = vis.get((k.vk - 0x31) as usize).copied() { ctx.close = true; later(move || paste(i, false)); },
            (0x50, true, _) => if let Some(i) = vis.get(m.selected).copied() { m.items[i].pinned = !m.items[i].pinned; later(save_pins); },
            (0x53, true, _) => if let Some(i) = vis.get(m.selected).copied() { later(move || to_shelf(i)); },
            (0x2E, _, _) => if let Some(i) = vis.get(m.selected).copied() { let was = m.items.remove(i).pinned; if was { later(save_pins); } thumbs.clear(); },
            _ => {}
        }
    }
    ui.st.activate = false;
    ui.st.tab = 0;
    ui.c.fill(Rect::new(0.0, 0.0, ui.w, 92.0), th.paper2);
    ui.c.line_text("CLIPBOARD", sign(18), Rect::new(16.0, 8.0, 200.0, 30.0), th.ink, Align::Left);
    ui.c.line_text(&format!("{} item{}", m.items.len(), if m.items.len() == 1 { "" } else { "s" }), mono(10), Rect::new(ui.w - 140.0, 8.0, 124.0, 30.0), th.ink2, Align::Right);
    let sr = Rect::new(12.0, 44.0, ui.w - 24.0, 36.0);
    ui.c.round(sr, 8.0, th.paper);
    ui.c.stroke_round(sr, 8.0, th.nacho_ink, 1.5);
    ui.c.glyph(0xE721, 13, Rect::new(sr.x + 6.0, sr.y, 26.0, sr.h), th.ink2);
    let q = if m.query.is_empty() { "Type to search".to_string() } else { format!("{}|", m.query) };
    ui.c.line_text(&q, sans(14), Rect::new(sr.x + 34.0, sr.y, sr.w - 40.0, sr.h), if m.query.is_empty() { th.ink2 } else { th.ink }, Align::Left);
    let list = Rect::new(0.0, 92.0, ui.w, ui.h - 92.0 - 30.0);
    if vis.is_empty() { ui.c.text(if m.items.is_empty() { "Copy something and it shows up here." } else { "Nothing matches." }, medium(13), Rect::new(20.0, list.y + 30.0, ui.w - 40.0, 40.0), th.ink2, Align::Center); }
    let sel = m.selected;
    let mut clicked = None;
    ui.scroll("clips", list, |ui, y0| {
        let mut y = y0 + 6.0;
        for (n, i) in vis.iter().enumerate() {
            let clip = app().clipboard.items[*i].clone();
            let r = Rect::new(8.0, y, ui.w - 16.0, 46.0);
            let id = ui.id(&format!("clip{i}"));
            let resp = ui.hit(id, r, false);
            ui.c.round(r, 8.0, if n == sel { NACHO.alpha(0.35) } else if resp.hover { th.paper2 } else { th.paper });
            let icon = Rect::new(r.x + 8.0, r.y + 7.0, 32.0, 32.0);
            match &clip.content {
                Content::Image { dib, .. } => {
                    let b = thumbs.entry(*i).or_insert_with(|| dib_pixels(dib).and_then(|(px, w, h)| ui.c.make_bitmap(&px, w, h)));
                    if let Some(b) = b { ui.c.bitmap(b, icon, 1.0); }
                }
                Content::Files(_) => ui.c.glyph(0xE8A5, 18, icon, th.ink2),
                Content::Text(_) => ui.c.glyph(0xE8A4, 18, icon, th.ink2),
            }
            ui.c.line_text(&clip.title(), medium(13), Rect::new(r.x + 48.0, r.y + 5.0, r.w - 96.0, 20.0), th.ink, Align::Left);
            let sub = if clip.source.is_empty() { String::new() } else { clip.source.clone() };
            ui.c.line_text(&sub, sans(11), Rect::new(r.x + 48.0, r.y + 24.0, r.w - 96.0, 16.0), th.ink2, Align::Left);
            if n < 9 { ui.c.line_text(&format!("Ctrl+{}", n + 1), mono(10), Rect::new(r.right() - 56.0, r.y, 48.0, r.h), th.ink2, Align::Right); }
            if clip.pinned { ui.c.glyph(0xE718, 11, Rect::new(r.right() - 74.0, r.y, 16.0, r.h), th.nacho_ink); }
            if resp.clicked { clicked = Some(*i); }
            y += 50.0;
        }
        y + 4.0
    });
    if let Some(i) = clicked { ctx.close = true; later(move || paste(i, false)); }
    ui.c.line_text("Enter paste  ·  Shift+Enter plain  ·  Ctrl+P pin  ·  Del remove", sans(11), Rect::new(0.0, ui.h - 28.0, ui.w, 24.0), th.ink2, Align::Center);
}

fn dib_pixels(dib: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let head = u32::from_le_bytes(dib[0..4].try_into().ok()?) as usize;
    let w = i32::from_le_bytes(dib[4..8].try_into().ok()?);
    let hraw = i32::from_le_bytes(dib[8..12].try_into().ok()?);
    let bits = u16::from_le_bytes(dib[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(dib[16..20].try_into().ok()?);
    let h = hraw.abs();
    if w <= 0 || h <= 0 || !(bits == 32 || bits == 24) { return None; }
    let start = head + if compression == 3 && head == 40 { 12 } else { 0 };
    let stride = (w as usize * bits as usize).div_ceil(32) * 4;
    if dib.len() < start + stride * h as usize { return None; }
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h as usize {
        let row = if hraw > 0 { h as usize - 1 - y } else { y };
        let src = &dib[start + row * stride..];
        for x in 0..w as usize {
            let (s, d) = (x * (bits as usize / 8), (y * w as usize + x) * 4);
            px[d..d + 3].copy_from_slice(&src[s..s + 3]);
            px[d + 3] = 255;
        }
    }
    Some((px, w as u32, h as u32))
}

unsafe fn put(fmt: u32, bytes: &[u8]) {
    let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else { return };
    let p = GlobalLock(mem) as *mut u8;
    if p.is_null() { let _ = GlobalFree(Some(mem)); return; }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    let _ = GlobalUnlock(mem);
    if SetClipboardData(fmt, Some(HANDLE(mem.0))).is_err() { let _ = GlobalFree(Some(mem)); }
}

/// Puts an item back on the clipboard, without recording it as a new copy.
fn restore(c: &Clip, plain: bool) {
    let a = app();
    a.clipboard.ignore_next = true;
    match &c.content {
        Content::Text(t) => { if plain { copy_text(t) } else { copy_text(t) } }
        Content::Files(paths) => unsafe {
            if OpenClipboard(Some(a.main)).is_err() { return; }
            let _ = EmptyClipboard();
            // DROPFILES header followed by double-zero-terminated wide paths.
            let mut b: Vec<u8> = vec![];
            b.extend_from_slice(&20u32.to_le_bytes());
            b.extend_from_slice(&[0u8; 12]);
            b.extend_from_slice(&1u32.to_le_bytes());
            for p in paths { for u in p.to_string_lossy().encode_utf16().chain([0]) { b.extend_from_slice(&u.to_le_bytes()); } }
            b.extend_from_slice(&[0, 0]);
            put(CF_HDROP, &b);
            let _ = CloseClipboard();
        },
        Content::Image { dib, .. } => unsafe {
            if OpenClipboard(Some(a.main)).is_err() { return; }
            let _ = EmptyClipboard();
            put(CF_DIB, dib);
            let _ = CloseClipboard();
        },
    }
}

fn send_paste() {
    unsafe {
        let key = |vk: VIRTUAL_KEY, up: bool| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } } };
        // The shortcut's own Alt may still be held: release modifiers first so the target sees a plain Ctrl+V.
        let release = [key(VK_MENU, true), key(VK_SHIFT, true), key(VK_LWIN, true)];
        SendInput(&release, std::mem::size_of::<INPUT>() as i32);
        let v = [key(VK_CONTROL, false), key(VIRTUAL_KEY(0x56), false), key(VIRTUAL_KEY(0x56), true), key(VK_CONTROL, true)];
        SendInput(&v, std::mem::size_of::<INPUT>() as i32);
    }
}

pub fn paste(i: usize, plain: bool) {
    let a = app();
    let Some(c) = a.clipboard.items.get(i).cloned() else { return };
    restore(&c, plain);
    // Moves the item to the top, as a new copy would.
    if !c.pinned { let it = a.clipboard.items.remove(i); a.clipboard.items.insert(0, it); }
    let target = a.clipboard.target.take().map(|h| HWND(h as *mut _));
    if let Some(t) = target {
        unsafe {
            let _ = SetForegroundWindow(t);
            // A short pause lets the window take the focus before the keys arrive.
            let main = a.main;
            let _ = SetTimer(Some(main), crate::app::TIMER_PASTE, 60, None);
        }
    }
}

pub fn on_paste_timer() {
    unsafe { let _ = KillTimer(Some(app().main), crate::app::TIMER_PASTE); }
    send_paste();
}

/// The shortcut: the last copy as plain text, straight into the front app.
pub fn paste_last_plain() {
    let a = app();
    if a.state(ModuleId::Clipboard) != State::Running { return; }
    let Some(i) = a.clipboard.items.iter().position(|c| matches!(c.content, Content::Text(_))) else { return };
    a.clipboard.target = Some(unsafe { GetForegroundWindow() }.0 as isize);
    paste(i, true);
}

fn to_shelf(i: usize) {
    let Some(c) = app().clipboard.items.get(i).cloned() else { return };
    if app().state(ModuleId::FileShelf) != State::Running { app().tray.balloon("Clipboard History", "Switch File Shelf on to send items to it."); return; }
    match c.content {
        Content::Text(t) => crate::shelf::add_plain_text(&t),
        Content::Files(f) => for p in f { crate::shelf::add_path(&p.to_string_lossy()); },
        Content::Image { .. } => { restore(&c, false); crate::shelf::paste_clipboard(); }
    }
    crate::shelf::show();
}
