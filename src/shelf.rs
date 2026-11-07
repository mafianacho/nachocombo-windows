//! File Shelf: a floating window that takes OLE drops and hands them back out. Dragging out always copies.
use crate::app::{app, later, State};
use crate::capture;
use crate::core::layout::Rect;
use crate::core::settings::*;
use crate::core::shelf::*;
use crate::gfx::*;
use crate::ui::*;
use crate::window::UiWindow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::StructuredStorage::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::*;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CF_UNICODETEXT: u16 = 13;
const CF_HDROP: u16 = 15;
const CF_DIB_: u16 = 8;

#[derive(Default)]
pub struct ShelfModule {
    pub shelf: Shelf,
    window: Option<isize>,
    preview: Option<isize>,
    pub opened: u32,
    pub dropped: u32,
    press: Option<(f64, f64, Option<usize>)>,
}

impl ShelfModule {
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "items": self.shelf.items.len(), "open": self.window.is_some(), "preview": self.preview.is_some(), "peek": crate::peek::LAST.with(|l| l.borrow().clone()), "opened": self.opened, "dropped": self.dropped, "titles": self.shelf.items.iter().map(|i| i.title.clone()).collect::<Vec<_>>() })
    }
    pub fn is_open(&self) -> bool { self.window.is_some() }
    pub fn count(&self) -> usize { self.shelf.items.len() }
}

pub fn start() -> std::result::Result<(), String> { Ok(()) }

/// Turning the gadget off deletes only the shelf's own copies.
pub fn stop() {
    close();
    let m = &mut app().shelf;
    for p in m.shelf.all_own_copies() { let _ = std::fs::remove_file(p); }
    m.shelf = Shelf::default();
    let _ = std::fs::remove_dir_all(app().dir.join("Shelf"));
}

fn running() -> bool { app().state(ModuleId::FileShelf) == State::Running }

pub fn toggle() { if app().shelf.window.is_some() { close(); } else { open_at_cursor(true); } }
pub fn show() { open_at_cursor(true); }
pub fn on_shake() { open_at_cursor(false); }
pub fn on_any_drag() { open_at_cursor(false); }

pub fn close() {
    let m = &mut app().shelf;
    if let Some(id) = m.preview.take() { unsafe { let _ = DestroyWindow(HWND(id as *mut _)); } }
    if let Some(id) = m.window.take() {
        unsafe {
            let h = HWND(id as *mut _);
            let _ = RevokeDragDrop(h);
            let _ = DestroyWindow(h);
        }
    }
    app().changed();
}

fn open_at_cursor(activate: bool) {
    if !running() { return; }
    let m = &mut app().shelf;
    if let Some(id) = m.window { if activate { unsafe { let _ = SetForegroundWindow(HWND(id as *mut _)); } } return; }
    m.opened += 1;
    let mut p = POINT::default();
    unsafe { let _ = GetCursorPos(&mut p); }
    let mut thumbs: HashMap<PathBuf, Option<ID2D1Bitmap>> = HashMap::new();
    let win = UiWindow::create("File Shelf", WS_POPUP | WS_CLIPCHILDREN, WS_EX_TOOLWINDOW | WS_EX_TOPMOST, p.x + 24, p.y - 60, 300.0, 380.0, Box::new(move |ui, ctx| draw(ui, ctx, &mut thumbs)));
    let h = win.hwnd;
    win.clear = Some(crate::window::theme().paper2);
    app().shelf.window = Some(h.0 as isize);
    win.on_close = Some(Box::new(|| { app().shelf.window = None; }));
    win.set_msg(Box::new(|hw, msg, _wp, lp| unsafe {
        if msg == WM_LBUTTONDOWN {
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f64 / crate::window::dpi_scale(hw);
            let x = (lp.0 & 0xFFFF) as i16 as f64 / crate::window::dpi_scale(hw);
            // The title strip moves the shelf, except over its close button.
            if y < 40.0 && x < 240.0 {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                let _ = SendMessageW(hw, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), Some(lp));
                return Some(LRESULT(0));
            }
        }
        None
    }));
    unsafe {
        let target: IDropTarget = Dropper.into();
        let _ = RegisterDragDrop(h, &target);
        // Keeps the shelf on screen even at the edge of a display.
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST), &mut mi);
        let (w, hh) = (r.right - r.left, r.bottom - r.top);
        let x = r.left.min(mi.rcWork.right - w).max(mi.rcWork.left);
        let y = r.top.min(mi.rcWork.bottom - hh).max(mi.rcWork.top);
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        let _ = ShowWindow(h, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE });
        if activate { let _ = SetForegroundWindow(h); }
    }
    app().changed();
}

fn glyph_for(k: &Kind) -> u32 { match k { Kind::File => 0xE8A5, Kind::Image => 0xEB9F, Kind::Link(_) => 0xE71B, Kind::Text(_) => 0xE8A4 } }

fn draw(ui: &mut Ui, ctx: &mut crate::window::WindowCtx, thumbs: &mut HashMap<PathBuf, Option<ID2D1Bitmap>>) {
    let th = ui.th;
    let m = &mut app().shelf;
    let h = ctx.hwnd;
    ui.c.fill(Rect::new(0.0, 0.0, ui.w, 40.0), NACHO);
    ui.c.line_text("FILE SHELF", sign(16), Rect::new(14.0, 0.0, 150.0, 40.0), ON_NACHO, Align::Left);
    let close_r = Rect::new(ui.w - 36.0, 6.0, 28.0, 28.0);
    let cid = ui.id("close");
    let cr = ui.hit(cid, close_r, false);
    if cr.hover { ui.c.round(close_r, 6.0, ON_NACHO.alpha(0.15)); }
    ui.c.glyph(0xE711, 12, close_r, ON_NACHO);
    if cr.clicked { later(close); return; }

    let count = m.shelf.items.len();
    let list = Rect::new(0.0, 40.0, ui.w, ui.h - 40.0 - 44.0);
    if count == 0 {
        ui.c.stroke_round(list.inset(14.0, 14.0), 10.0, th.line, 1.5);
        ui.c.glyph(0xE896, 28, Rect::new(0.0, list.cy() - 46.0, ui.w, 40.0), th.ink2);
        ui.c.text("Drop files, images, links or text here", medium(13), Rect::new(30.0, list.cy() - 2.0, ui.w - 60.0, 44.0), th.ink2, Align::Center);
    }
    let mut clicked_row = None;
    let mut remove = None;
    ui.scroll("rows", list, |ui, y0| {
        let mut y = y0 + 8.0;
        for i in 0..count {
            let it = m.shelf.items[i].clone();
            let r = Rect::new(10.0, y, ui.w - 20.0, 52.0);
            let id = ui.id(&format!("row{i}"));
            let resp = ui.hit(id, r, false);
            let sel = m.shelf.selected == Some(i);
            ui.c.round(r, 8.0, if sel { NACHO.alpha(0.35) } else if resp.hover { th.paper } else { th.paper2 });
            let tr = Rect::new(r.x + 6.0, r.y + 6.0, 40.0, 40.0);
            let bmp = thumbs.entry(it.path.clone()).or_insert_with(|| thumbnail(&it.path, 96).and_then(|(px, w, h)| ui.c.make_bitmap(&px, w, h)));
            match bmp { Some(b) => ui.c.bitmap(b, tr, 1.0), None => ui.c.glyph(glyph_for(&it.kind), 22, tr, th.ink2) }
            ui.c.line_text(&it.title, medium(13), Rect::new(r.x + 54.0, r.y + 7.0, r.w - 86.0, 20.0), th.ink, Align::Left);
            let sub = match &it.kind { Kind::File => it.path.parent().map(|p| p.display().to_string()).unwrap_or_default(), Kind::Image => "Image".to_string(), Kind::Link(u) => u.clone(), Kind::Text(_) => "Text".to_string() };
            ui.c.line_text(&sub, sans(11), Rect::new(r.x + 54.0, r.y + 27.0, r.w - 86.0, 18.0), th.ink2, Align::Left);
            if resp.hover {
                let xr = Rect::new(r.right() - 28.0, r.y + 14.0, 24.0, 24.0);
                let xid = ui.id(&format!("x{i}"));
                let xresp = ui.hit(xid, xr, false);
                ui.c.round(xr, 5.0, if xresp.hover { th.line } else { th.paper2 });
                ui.c.glyph(0xE711, 10, xr, th.ink2);
                if xresp.clicked { remove = Some(i); }
            }
            if resp.pressed && app().shelf.press.is_none() { app().shelf.press = Some((ui.st.mouse.0, ui.st.mouse.1, Some(i))); }
            if resp.clicked { clicked_row = Some(i); }
            if ui.st.double_click && r.contains(ui.st.mouse.0, ui.st.mouse.1) { let p = it.path.clone(); later(move || crate::app::open_url(&p.to_string_lossy())); }
            if resp.right { let idx = i; later(move || row_menu(idx)); }
            y += 56.0;
        }
        y + 4.0
    });
    if let Some(i) = remove { m.shelf.remove(i); app().changed(); ui.st.again = true; }
    else if let Some(i) = clicked_row { m.shelf.selected = Some(i); }

    // The stack handle at the bottom drags everything at once.
    let foot = Rect::new(10.0, ui.h - 40.0, ui.w - 20.0, 32.0);
    if count > 0 {
        let sid = ui.id("stack");
        let sr = ui.hit(sid, foot, false);
        ui.c.round(foot, 7.0, if sr.hover { BOARD2 } else { BOARD });
        ui.c.line_text(&format!("{} item{}  ·  drag all", count, if count == 1 { "" } else { "s" }), semi(12), foot, BOARD_INK, Align::Center);
        if sr.pressed && app().shelf.press.is_none() { app().shelf.press = Some((ui.st.mouse.0, ui.st.mouse.1, None)); }
    } else {
        ui.c.line_text("Ctrl+V adds the clipboard", sans(12), foot, th.ink2, Align::Center);
    }

    // A press that travels becomes a drag out.
    if let Some((x, y, which)) = app().shelf.press {
        if !ui.st.down { app().shelf.press = None; }
        else if (ui.st.mouse.0 - x).abs() + (ui.st.mouse.1 - y).abs() > 5.0 {
            app().shelf.press = None;
            ui.st.down = false;
            unsafe { let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture(); }
            later(move || drag_items(h, which));
        }
    }

    for k in ui.st.keys.clone() {
        let m = &mut app().shelf;
        match (k.vk, k.ctrl) {
            (0x20, false) => { if let Some(i) = m.shelf.selected { later(move || toggle_preview(i)); } }
            (0x2E, _) | (0x08, _) => { if let Some(i) = m.shelf.selected { m.shelf.remove(i); app().changed(); } }
            (0x5A, true) => { m.shelf.undo(); app().changed(); }
            (0x56, true) => later(paste_clipboard),
            (0x43, true) => { if let Some(i) = m.shelf.selected { later(move || copy_item(i)); } }
            (0x1B, _) => later(close),
            (0x28, _) => { let n = m.shelf.items.len(); if n > 0 { m.shelf.selected = Some(m.shelf.selected.map_or(0, |i| (i + 1).min(n - 1))); } }
            (0x26, _) => { m.shelf.selected = m.shelf.selected.map(|i| i.saturating_sub(1)); }
            (0x0D, _) => { if let Some(it) = m.shelf.selected.and_then(|i| m.shelf.items.get(i)) { let p = it.path.clone(); later(move || crate::app::open_url(&p.to_string_lossy())); } }
            _ => {}
        }
    }
    ui.st.activate = false;
}

fn row_menu(i: usize) {
    let Some(it) = app().shelf.shelf.items.get(i).cloned() else { return };
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, 1, &HSTRING::from(crate::i18n::t("Open").as_ref()));
        let _ = AppendMenuW(menu, MF_STRING, 2, &HSTRING::from(crate::i18n::t("Preview\tSpace").as_ref()));
        let _ = AppendMenuW(menu, MF_STRING, 3, &HSTRING::from(crate::i18n::t("Show in File Explorer").as_ref()));
        let _ = AppendMenuW(menu, MF_STRING, 4, &HSTRING::from(crate::i18n::t("Copy\tCtrl+C").as_ref()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, 5, &HSTRING::from(crate::i18n::t("Remove\tDel").as_ref()));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let owner = app().shelf.window.map_or(app().main, |w| HWND(w as *mut _));
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, owner, None).0;
        let _ = DestroyMenu(menu);
        match cmd {
            1 => crate::app::open_url(&it.path.to_string_lossy()),
            2 => toggle_preview(i),
            3 => { let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", it.path.display())).spawn(); }
            4 => copy_item(i),
            5 => { app().shelf.shelf.remove(i); app().changed(); }
            _ => {}
        }
    }
}

/// A picture of any file, through the same handlers File Explorer uses. Top-down BGRA.
fn thumbnail(path: &Path, size: i32) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let item: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
        let bmp = item.GetImage(SIZE { cx: size, cy: size }, SIIGBF_BIGGERSIZEOK).ok()?;
        let mut info = BITMAP::default();
        GetObjectW(bmp.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut _));
        let (w, h) = (info.bmWidth, info.bmHeight.abs());
        let dc = CreateCompatibleDC(None);
        let mut bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
        let mut px = vec![0u8; (w * h * 4) as usize];
        GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        let _ = DeleteObject(bmp.into());
        // Opaque pictures come back with alpha 0: treat "all zero" as opaque.
        if px.chunks_exact(4).all(|p| p[3] == 0) { for p in px.chunks_exact_mut(4) { p[3] = 255; } }
        Some((px, w as u32, h as u32))
    }
}

fn toggle_preview(i: usize) {
    let m = &mut app().shelf;
    if let Some(id) = m.preview.take() { unsafe { let _ = DestroyWindow(HWND(id as *mut _)); } return; }
    let Some(it) = m.shelf.items.get(i).cloned() else { return };
    show_preview(it);
}

/// Space in File Explorer: the same preview, for any file.
pub fn preview_file(path: PathBuf) {
    if let Some(id) = app().shelf.preview.take() { unsafe { let _ = DestroyWindow(HWND(id as *mut _)); } }
    let title = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let is_text = path.extension().is_some_and(|e| ["txt", "md", "log", "json", "csv", "ini", "xml", "yaml", "yml", "toml", "rs", "py", "js", "ts", "c", "h", "cpp", "cs", "java", "go", "swift", "html", "css", "sql", "ps1", "bat", "sh"].contains(&e.to_string_lossy().to_lowercase().as_str()));
    // A text file shows its first lines; anything else the picture Windows has for it.
    let kind = if is_text { std::fs::read(&path).ok().map(|b| Kind::Text(String::from_utf8_lossy(&b[..b.len().min(64 * 1024)]).to_string())).unwrap_or(Kind::File) } else { Kind::File };
    show_preview(Item { path, kind, title, own_copy: false });
}

fn show_preview(it: Item) {
    let pic = thumbnail(&it.path, 720);
    let text = match &it.kind { Kind::Text(t) => Some(t.clone()), Kind::Link(u) => Some(u.clone()), _ => None };
    let (pw, ph) = pic.as_ref().map_or((480.0, 320.0), |p| (p.1 as f64, p.2 as f64 + 44.0));
    let (pw, ph) = (pw.clamp(320.0, 720.0), ph.clamp(160.0, 764.0));
    let mut bmp: Option<ID2D1Bitmap> = None;
    let title = it.title.clone();
    let win = UiWindow::create("Preview", WS_POPUP, WS_EX_TOOLWINDOW | WS_EX_TOPMOST, 300, 200, pw, ph, Box::new(move |ui, ctx| {
        ui.c.fill(Rect::new(0.0, 0.0, ui.w, 36.0), ui.th.paper2);
        ui.c.line_text(&title, semi(13), Rect::new(12.0, 0.0, ui.w - 24.0, 36.0), ui.th.ink, Align::Left);
        let body = Rect::new(0.0, 36.0, ui.w, ui.h - 36.0);
        if let Some(t) = &text { ui.c.text(t, sans(14), body.inset(14.0, 12.0), ui.th.ink, Align::Left); }
        else if let Some((px, w, h)) = &pic {
            if bmp.is_none() { bmp = ui.c.make_bitmap(px, *w, *h); }
            if let Some(b) = &bmp {
                let s = (body.w / *w as f64).min(body.h / *h as f64).min(1.0);
                let (dw, dh) = (*w as f64 * s, *h as f64 * s);
                ui.c.bitmap(b, Rect::new(body.cx() - dw / 2.0, body.cy() - dh / 2.0, dw, dh), 1.0);
            }
        }
        for k in ui.st.keys.clone() { if k.vk == 0x1B || k.vk == 0x20 { ctx.close = true; } }
    }));
    win.close_on_deactivate = true;
    win.on_close = Some(Box::new(|| { app().shelf.preview = None; }));
    let h = win.hwnd;
    app().shelf.preview = Some(h.0 as isize);
    unsafe {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = GetMonitorInfoW(MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST), &mut mi);
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let (w, hh) = (r.right - r.left, r.bottom - r.top);
        let _ = SetWindowPos(h, None, (mi.rcWork.left + mi.rcWork.right - w) / 2, (mi.rcWork.top + mi.rcWork.bottom - hh) / 2, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
    }
    win.show();
}

// ---------- taking things in ----------

fn own_dir() -> PathBuf {
    let d = app().dir.join("Shelf");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn unique(dir: &Path, name: &str) -> PathBuf {
    let clean: String = name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    let clean = if clean.trim().is_empty() { "Item".to_string() } else { clean };
    let first = dir.join(&clean);
    if !first.exists() { return first; }
    let p = Path::new(&clean);
    let (stem, ext) = (p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), p.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default());
    (2..).map(|n| dir.join(format!("{stem} {n}{ext}"))).find(|c| !c.exists()).unwrap()
}

fn add_file(path: PathBuf) -> bool {
    let title = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().to_string());
    app().shelf.shelf.add(Item { path, kind: Kind::File, title, own_copy: false })
}

fn add_text(text: &str) -> bool {
    let first: String = text.lines().next().unwrap_or("Text").chars().take(40).collect();
    let path = unique(&own_dir(), &format!("{first}.txt"));
    if std::fs::write(&path, text).is_err() { return false; }
    app().shelf.shelf.add(Item { path, kind: Kind::Text(text.to_string()), title: first, own_copy: true })
}

fn add_link(url: &str) -> bool {
    let name = url.split("//").nth(1).unwrap_or(url).split(['/', '?']).next().unwrap_or("Link").to_string();
    let path = unique(&own_dir(), &format!("{name}.url"));
    if std::fs::write(&path, format!("[InternetShortcut]\r\nURL={url}\r\n")).is_err() { return false; }
    app().shelf.shelf.add(Item { path, kind: Kind::Link(url.to_string()), title: name, own_copy: true })
}

fn add_image_bytes(png: &[u8]) -> bool {
    let path = unique(&own_dir(), "Image.png");
    if std::fs::write(&path, png).is_err() { return false; }
    app().shelf.shelf.add(Item { path, kind: Kind::Image, title: "Image.png".into(), own_copy: true })
}

fn format_etc(cf: u16, index: i32) -> FORMATETC {
    FORMATETC { cfFormat: cf, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: index, tymed: (TYMED_HGLOBAL.0 | TYMED_ISTREAM.0) as u32 }
}

fn registered(name: PCWSTR) -> u16 { unsafe { RegisterClipboardFormatW(name) as u16 } }

unsafe fn global_bytes(m: &STGMEDIUM) -> Vec<u8> {
    let g = m.u.hGlobal;
    let p = GlobalLock(g) as *const u8;
    if p.is_null() { return vec![]; }
    let v = std::slice::from_raw_parts(p, GlobalSize(g)).to_vec();
    let _ = GlobalUnlock(g);
    v
}

/// One format's bytes, from either a memory block or a stream.
unsafe fn read_format(d: &IDataObject, cf: u16, index: i32) -> Option<Vec<u8>> {
    let mut m = d.GetData(&format_etc(cf, index)).ok()?;
    let out = if m.tymed == TYMED_HGLOBAL.0 as u32 { Some(global_bytes(&m)) }
    else if m.tymed == TYMED_ISTREAM.0 as u32 {
        let stream = (*m.u.pstm).clone().map(|s| s);
        stream.map(|s| {
            let mut v = Vec::new();
            let mut buf = [0u8; 65536];
            loop {
                let mut n = 0u32;
                if s.Read(buf.as_mut_ptr() as *mut _, buf.len() as u32, Some(&mut n)).is_err() || n == 0 { break; }
                v.extend_from_slice(&buf[..n as usize]);
            }
            v
        })
    } else { None };
    ReleaseStgMedium(&mut m);
    out
}

fn wide_string(bytes: &[u8]) -> String {
    let w: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
    String::from_utf16_lossy(&w)
}

fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 { return None; }
    let head = u32::from_le_bytes(dib[0..4].try_into().ok()?) as usize;
    let w = i32::from_le_bytes(dib[4..8].try_into().ok()?);
    let hraw = i32::from_le_bytes(dib[8..12].try_into().ok()?);
    let bits = u16::from_le_bytes(dib[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(dib[16..20].try_into().ok()?);
    let h = hraw.abs();
    if w <= 0 || h == 0 || !(bits == 32 || bits == 24) { return None; }
    let masks = if compression == 3 && head == 40 { 12 } else { 0 };
    let start = head + masks;
    let stride = ((w as usize * bits as usize + 31) / 32) * 4;
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
    let tmp = std::env::temp_dir().join(format!("nacho-dib-{}.png", std::process::id()));
    save_png(&tmp, &px, w as u32, h as u32).ok()?;
    let out = std::fs::read(&tmp).ok();
    let _ = std::fs::remove_file(tmp);
    out
}

/// Everything a drop or the clipboard can carry, richest first. Returns how many items were added.
pub fn ingest(d: &IDataObject) -> usize {
    let mut added = 0;
    unsafe {
        if let Ok(mut m) = d.GetData(&FORMATETC { cfFormat: CF_HDROP, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }) {
            let drop = HDROP(m.u.hGlobal.0);
            let n = DragQueryFileW(drop, 0xFFFFFFFF, None);
            for i in 0..n {
                let mut buf = vec![0u16; 32768];
                let len = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                if add_file(PathBuf::from(String::from_utf16_lossy(&buf[..len]))) { added += 1; }
            }
            ReleaseStgMedium(&mut m);
            if added > 0 { return added; }
        }
        // Attachments and pictures that only exist inside the source app (Outlook, browsers): "virtual files".
        let (fgd, contents) = (registered(w!("FileGroupDescriptorW")), registered(w!("FileContents")));
        if let Some(desc) = read_format(d, fgd, -1) {
            if desc.len() >= 4 {
                let count = u32::from_le_bytes(desc[0..4].try_into().unwrap_or_default()) as usize;
                const ITEM: usize = 592;
                for i in 0..count {
                    let off = 4 + i * ITEM;
                    if desc.len() < off + ITEM { break; }
                    let name = wide_string(&desc[off + 72..off + ITEM]);
                    if let Some(bytes) = read_format(d, contents, i as i32) {
                        let path = unique(&own_dir(), &name);
                        if std::fs::write(&path, bytes).is_ok() {
                            let title = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            if app().shelf.shelf.add(Item { path, kind: Kind::File, title, own_copy: true }) { added += 1; }
                        }
                    }
                }
                if added > 0 { return added; }
            }
        }
        if let Some(b) = read_format(d, registered(w!("PNG")), -1) { if add_image_bytes(&b) { return 1; } }
        if let Some(b) = read_format(d, registered(w!("UniformResourceLocatorW")), -1) {
            let u = wide_string(&b);
            if !u.is_empty() && add_link(&u) { return 1; }
        }
        if let Some(b) = read_format(d, CF_UNICODETEXT, -1) {
            let t = wide_string(&b);
            if !t.trim().is_empty() {
                let looks_link = (t.starts_with("http://") || t.starts_with("https://")) && !t.trim().contains(char::is_whitespace);
                if if looks_link { add_link(t.trim()) } else { add_text(&t) } { return 1; }
            }
        }
        if let Some(b) = read_format(d, CF_DIB_, -1) {
            if let Some(png) = dib_to_png(&b) { if add_image_bytes(&png) { return 1; } }
        }
    }
    added
}

pub fn paste_clipboard() {
    if !running() { return; }
    if app().shelf.window.is_none() { open_at_cursor(false); }
    unsafe { if let Ok(d) = OleGetClipboard() { ingest(&d); } }
    app().changed();
}

/// For `ctl` and tests.
pub fn add_path(p: &str) { add_file(PathBuf::from(p)); app().changed(); }
pub fn add_plain_text(t: &str) { add_text(t); app().changed(); }
pub fn add_url(u: &str) { add_link(u); app().changed(); }

#[implement(IDropTarget)]
struct Dropper;

#[allow(non_snake_case)]
impl IDropTarget_Impl for Dropper_Impl {
    fn DragEnter(&self, _: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> { unsafe { *effect = DROPEFFECT_COPY; } Ok(()) }
    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> { unsafe { *effect = DROPEFFECT_COPY; } Ok(()) }
    fn DragLeave(&self) -> Result<()> { Ok(()) }
    fn Drop(&self, data: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *effect = DROPEFFECT_COPY; }
        if let Some(d) = data.as_ref() {
            let m = &mut app().shelf;
            m.dropped += ingest(d) as u32;
            app().changed();
        }
        Ok(())
    }
}

// ---------- handing things out ----------

/// A shell data object for files, which Explorer and every app understand, with text alongside when there is any.
unsafe fn data_object(paths: &[PathBuf], text: Option<&str>) -> Option<IDataObject> {
    let mut pidls: Vec<*mut ITEMIDLIST> = vec![];
    for p in paths {
        let mut pidl = std::ptr::null_mut();
        if SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None).is_ok() { pidls.push(pidl); }
    }
    let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
    let d: Option<IDataObject> = SHCreateDataObject(None, Some(&raw), None).ok();
    for p in pidls { CoTaskMemFree(Some(p as *const _)); }
    let d = d?;
    if let Some(t) = text {
        let w: Vec<u16> = t.encode_utf16().chain([0]).collect();
        if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) {
            let q = GlobalLock(mem) as *mut u16;
            if !q.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), q, w.len());
                let _ = GlobalUnlock(mem);
                let fe = FORMATETC { cfFormat: CF_UNICODETEXT, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
                let med = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: mem }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
                let _ = d.SetData(&fe, &med, true);
            }
        }
    }
    Some(d)
}

/// Starts a drag of these files from `h`; returns when it ends. Only copying is offered, so originals never move.
pub fn drag_paths(h: HWND, paths: &[PathBuf], text: Option<&str>) {
    unsafe {
        let Some(d) = data_object(paths, text) else { return };
        let _ = SHDoDragDrop(Some(h), &d, None, DROPEFFECT_COPY);
    }
}

fn drag_items(h: HWND, which: Option<usize>) {
    let m = &app().shelf;
    let items: Vec<Item> = match which { Some(i) => m.shelf.items.get(i).cloned().into_iter().collect(), None => m.shelf.items.clone() };
    if items.is_empty() { return; }
    let paths: Vec<PathBuf> = items.iter().map(|i| i.path.clone()).collect();
    let text = (items.len() == 1).then(|| match &items[0].kind { Kind::Text(t) => Some(t.clone()), Kind::Link(u) => Some(u.clone()), _ => None }).flatten();
    drag_paths(h, &paths, text.as_deref());
}

fn copy_item(i: usize) {
    let Some(it) = app().shelf.shelf.items.get(i).cloned() else { return };
    match &it.kind {
        Kind::Text(t) | Kind::Link(t) => capture::copy_text(t),
        _ => unsafe { if let Some(d) = data_object(&[it.path], None) { let _ = OleSetClipboard(&d); let _ = OleFlushClipboard(); } },
    }
}

#[allow(unused)]
fn _keep() { later(|| {}); }
