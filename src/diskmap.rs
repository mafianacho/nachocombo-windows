//! Disk Map: what fills a disk, as blocks. A background thread reads one fixed drive; the window shows one folder at a
//! time, goes in and out, and moves an item to the Recycle Bin only after a confirmation drawn in the window.
use crate::app::{app, later};
use crate::core::format;
use crate::core::layout::Rect;
use crate::gfx::*;
use crate::ui::*;
use crate::window::{UiWindow, WindowCtx};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Instant;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Posted to `app().main` by the scan: `wp` 0 is progress, 1 a finished scan with a `Box<Done>` in `lp`.
pub const WM_DISKMAP: u32 = WM_APP + 20;
/// Files below this are summed per folder: a full drive has millions of them and none is worth a block.
const BIG_FILE: u64 = 1 << 20;
/// Blocks drawn per folder; the rest are summed into one.
const MAX_BLOCKS: usize = 300;
/// How long the map is kept after the window closes, so reopening it does not read the disk again.
const KEEP_MS: u32 = 600_000;
const SIDE: f64 = 290.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind { Folder, Locked, File, Smaller }

/// One folder, big file or "Smaller files" sum. The root is index 0 and is its own parent.
struct Node { name: Box<str>, size: u64, parent: u32, kids: Vec<u32>, kind: Kind, changed: u64 }

/// What the scan shares with the UI thread while it runs.
#[derive(Default)]
struct Progress { cancel: AtomicBool, files: AtomicU64, bytes: AtomicU64, locked: AtomicU64 }

struct Done { gen: u32, tree: Vec<Node>, secs: f64 }

#[derive(Clone)]
struct Summary { drive: String, files: u64, bytes: u64, locked: u64, secs: f64 }

/// A block on the map: a node, or one of the parts of the drive that are not in the tree.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Item { Node(u32), Free, Hidden, Rest(u32) }

struct Confirm { node: u32, name: String, path: String, size: u64 }

#[derive(Default)]
struct DiskMap {
    window: Option<isize>,
    drive: String,
    drives: Vec<String>,
    tree: Vec<Node>,
    scan: Option<Arc<Progress>>,
    gen: u32,
    view: u32,
    sel: Option<Item>,
    confirm: Option<Confirm>,
    /// Free and total bytes of the drive.
    space: (u64, u64),
    summary: Option<Summary>,
    note: String,
    guard: Option<Guard>,
    forget_timer: usize,
    scans: u32,
    recycled: u32,
}

thread_local! { static DM: RefCell<DiskMap> = RefCell::new(DiskMap::default()); }

/// Never call anything that can re-enter the window (DestroyWindow, menus, shell dialogs) inside `with`.
fn with<R>(f: impl FnOnce(&mut DiskMap) -> R) -> R { DM.with(|d| f(&mut d.borrow_mut())) }

// ---------- lifecycle ----------

pub fn start() -> std::result::Result<(), String> { Ok(()) }

/// Cancels a running scan, closes the window and lets go of the map.
pub fn stop() {
    let h = with(|d| {
        cancel(d);
        if d.forget_timer != 0 { unsafe { let _ = KillTimer(None, d.forget_timer); } }
        let h = d.window.take();
        *d = DiskMap { scans: d.scans, recycled: d.recycled, ..Default::default() };
        h
    });
    if let Some(h) = h { unsafe { let _ = DestroyWindow(HWND(h as *mut _)); } }
}

pub fn is_open() -> bool { with(|d| d.window.is_some()) }

pub fn diagnostics() -> serde_json::Value {
    with(|d| {
        let p = d.scan.as_ref().map(|p| (p.files.load(Relaxed), p.bytes.load(Relaxed)));
        serde_json::json!({
            "open": d.window.is_some(), "drive": d.drive, "scanning": d.scan.is_some(), "progressFiles": p.map(|p| p.0), "progressBytes": p.map(|p| p.1),
            "nodes": d.tree.len(), "view": if d.tree.is_empty() { String::new() } else { d.path_of(d.view) }, "free": d.space.0, "total": d.space.1,
            "lastScan": d.summary.as_ref().map(|s| serde_json::json!({ "drive": s.drive, "files": s.files, "bytes": s.bytes, "unreadable": s.locked, "seconds": s.secs })),
            "scans": d.scans, "recycled": d.recycled, "confirming": d.confirm.is_some(),
        })
    })
}

/// Opens or focuses the window, and starts a scan when there is no map yet.
pub fn open() {
    if let Some(h) = with(|d| d.window) {
        unsafe { let h = HWND(h as *mut _); let _ = ShowWindow(h, SW_RESTORE); let _ = SetForegroundWindow(h); }
        return;
    }
    let win = UiWindow::create("Disk Map", WS_OVERLAPPEDWINDOW, WINDOW_EX_STYLE(0), CW_USEDEFAULT, CW_USEDEFAULT, 1000.0, 660.0, Box::new(draw));
    win.on_close = Some(Box::new(|| {
        with(|d| {
            d.window = None;
            d.confirm = None;
            if !d.tree.is_empty() || d.scan.is_some() { d.forget_timer = unsafe { SetTimer(None, 0, KEEP_MS, Some(forget)) }; }
        });
        app().changed();
    }));
    let h = win.hwnd;
    with(|d| {
        d.window = Some(h.0 as isize);
        if d.forget_timer != 0 { unsafe { let _ = KillTimer(None, d.forget_timer); } d.forget_timer = 0; }
        if d.drive.is_empty() { d.drive = system_drive(); }
        d.drives = fixed_drives();
        if d.tree.is_empty() && d.scan.is_none() { start_scan(d); } else { d.space = space(&d.drive); }
    });
    win.show();
    app().changed();
}

/// Ten minutes after the window closed, the map goes; the last scan's summary stays.
unsafe extern "system" fn forget(_: HWND, _: u32, id: usize, _: u32) {
    let _ = KillTimer(None, id);
    with(|d| if d.window.is_none() { cancel(d); d.tree = Vec::new(); d.view = 0; d.sel = None; d.forget_timer = 0; });
}

pub fn on_message(wp: WPARAM, lp: LPARAM) {
    if wp.0 == 1 {
        let done = unsafe { Box::from_raw(lp.0 as *mut Done) };
        let Done { gen, tree, secs } = *done;
        with(|d| {
            let Some(p) = d.scan.as_ref() else { return };
            if gen != d.gen || p.cancel.load(Relaxed) { return; }
            d.summary = Some(Summary { drive: d.drive.clone(), files: p.files.load(Relaxed), bytes: tree[0].size, locked: p.locked.load(Relaxed), secs });
            d.tree = tree;
            d.scan = None;
            d.view = 0;
            d.sel = None;
            d.confirm = None;
            d.space = space(&d.drive);
        });
        app().changed();
    } else {
        app().panel.invalidate();
    }
    repaint();
}

fn repaint() {
    if let Some(h) = with(|d| d.window) { unsafe { let _ = InvalidateRect(Some(HWND(h as *mut _)), None, false); } }
}

fn cancel(d: &mut DiskMap) {
    if let Some(p) = d.scan.take() { p.cancel.store(true, Relaxed); }
    d.gen += 1;
}

fn start_scan(d: &mut DiskMap) {
    cancel(d);
    d.scans += 1;
    d.note.clear();
    d.space = space(&d.drive);
    let p = Arc::new(Progress::default());
    let (root, gen, main, q) = (d.drive.clone(), d.gen, app().main.0 as isize, p.clone());
    // Deep folder trees recurse deeply, hence the larger stack.
    let spawned = std::thread::Builder::new().name("disk-map".into()).stack_size(16 << 20).spawn(move || scan(root, gen, q, main));
    if spawned.is_ok() { d.scan = Some(p); } else { d.note = "The scan could not start.".into(); }
}

// ---------- drives ----------

fn system_drive() -> String { format!("{}\\", std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())) }

fn fixed_drives() -> Vec<String> {
    const DRIVE_FIXED: u32 = 3;
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| format!("{}:\\", (b'A' + i) as char))
        .filter(|r| unsafe { GetDriveTypeW(&HSTRING::from(r.as_str())) } == DRIVE_FIXED).collect()
}

/// Free and total bytes.
fn space(root: &str) -> (u64, u64) {
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    unsafe { let _ = GetDiskFreeSpaceExW(&HSTRING::from(root), Some(&mut avail), Some(&mut total), Some(&mut free)); }
    (free, total)
}

// ---------- the scan ----------

struct Walker { tree: Vec<Node>, p: Arc<Progress>, main: isize, last: Instant }

fn scan(root: String, gen: u32, p: Arc<Progress>, main: isize) {
    let t0 = Instant::now();
    let mut w = Walker { tree: vec![Node { name: root.as_str().into(), size: 0, parent: 0, kids: vec![], kind: Kind::Folder, changed: 0 }], p, main, last: t0 };
    // The long-path prefix keeps folders deeper than 260 characters readable.
    let mut path: Vec<u16> = format!(r"\\?\{root}").encode_utf16().collect();
    w.dir(&mut path, 0);
    w.tree.shrink_to_fit();
    let done = Box::into_raw(Box::new(Done { gen, tree: w.tree, secs: t0.elapsed().as_secs_f64() }));
    unsafe {
        if PostMessageW(Some(HWND(main as *mut _)), WM_DISKMAP, WPARAM(1), LPARAM(done as isize)).is_err() { drop(Box::from_raw(done)); }
    }
}

impl Walker {
    fn push(&mut self, name: Box<str>, size: u64, parent: u32, kind: Kind, changed: u64) -> u32 {
        self.tree.push(Node { name, size, parent, kids: vec![], kind, changed });
        self.tree.len() as u32 - 1
    }

    fn tick(&mut self) {
        if self.last.elapsed().as_millis() < 150 { return; }
        self.last = Instant::now();
        unsafe { let _ = PostMessageW(Some(HWND(self.main as *mut _)), WM_DISKMAP, WPARAM(0), LPARAM(0)); }
    }

    /// Reads the folder at `path` (ending in a backslash) into node `me`; returns its size.
    fn dir(&mut self, path: &mut Vec<u16>, me: u32) -> u64 {
        const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
        let base = path.len();
        path.extend_from_slice(&[b'*' as u16, 0]);
        let mut fd = WIN32_FIND_DATAW::default();
        let h = unsafe { FindFirstFileExW(PCWSTR(path.as_ptr()), FindExInfoBasic, &mut fd as *mut _ as *mut _, FindExSearchNameMatch, None, FIND_FIRST_EX_LARGE_FETCH) };
        path.truncate(base);
        let Ok(h) = h else {
            // Not readable is a fact about the folder, not an error of the scan.
            self.tree[me as usize].kind = Kind::Locked;
            self.p.locked.fetch_add(1, Relaxed);
            return 0;
        };
        let (mut kids, mut dirs, mut small, mut total) = (vec![], vec![], 0u64, 0u64);
        loop {
            if self.p.cancel.load(Relaxed) { break; }
            let n = fd.cFileName.iter().position(|c| *c == 0).unwrap_or(fd.cFileName.len());
            let name = &fd.cFileName[..n];
            let a = fd.dwFileAttributes;
            let changed = (fd.ftLastWriteTime.dwHighDateTime as u64) << 32 | fd.ftLastWriteTime.dwLowDateTime as u64;
            if name == [46] || name == [46, 46] {
            } else if a & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
                // Junctions, links and mounted volumes lead to space counted elsewhere, or back up the tree.
                if a & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 { dirs.push((String::from_utf16_lossy(name), changed)); }
            } else if a & (FILE_ATTRIBUTE_OFFLINE.0 | RECALL_ON_DATA_ACCESS) == 0 {
                // Cloud placeholders are left out: their size is not on this disk.
                let size = (fd.nFileSizeHigh as u64) << 32 | fd.nFileSizeLow as u64;
                total += size;
                let count = self.p.files.fetch_add(1, Relaxed);
                self.p.bytes.fetch_add(size, Relaxed);
                if size >= BIG_FILE { kids.push(self.push(String::from_utf16_lossy(name).into(), size, me, Kind::File, changed)); } else { small += size; }
                if count % 4096 == 0 { self.tick(); }
            }
            if unsafe { FindNextFileW(h, &mut fd) }.is_err() { break; }
        }
        unsafe { let _ = FindClose(h); }
        self.tick();
        // Subfolders after the handle is closed, so a deep tree keeps one search open at a time.
        for (name, changed) in dirs {
            if self.p.cancel.load(Relaxed) { break; }
            path.extend(name.encode_utf16());
            path.push(b'\\' as u16);
            let i = self.push(name.into(), 0, me, Kind::Folder, changed);
            let size = self.dir(path, i);
            path.truncate(base);
            self.tree[i as usize].size = size;
            total += size;
            kids.push(i);
        }
        if small > 0 { kids.push(self.push("Smaller files".into(), small, me, Kind::Smaller, 0)); }
        kids.sort_by_key(|k| std::cmp::Reverse(self.tree[*k as usize].size));
        kids.shrink_to_fit();
        let n = &mut self.tree[me as usize];
        n.kids = kids;
        n.size = total;
        total
    }
}

// ---------- the map's model ----------

impl DiskMap {
    fn path_of(&self, i: u32) -> String {
        let mut parts = vec![];
        let mut n = i;
        while n != 0 { parts.push(&*self.tree[n as usize].name); n = self.tree[n as usize].parent; }
        let mut s = self.tree.first().map_or(self.drive.clone(), |r| r.name.to_string());
        for (k, p) in parts.iter().rev().enumerate() { if k > 0 { s.push('\\'); } s.push_str(p); }
        s
    }

    /// Used space the scan did not see: system files, closed folders, changes since.
    fn hidden(&self) -> u64 { self.space.1.saturating_sub(self.space.0).saturating_sub(self.tree.first().map_or(0, |r| r.size)) }

    /// The blocks of a folder, biggest first. The drive root also shows its free and hidden space.
    fn items(&self, view: u32) -> Vec<(Item, u64)> {
        let Some(node) = self.tree.get(view as usize) else { return vec![] };
        let mut v: Vec<(Item, u64)> = node.kids.iter().take(MAX_BLOCKS).map(|k| (Item::Node(*k), self.tree[*k as usize].size)).collect();
        if node.kids.len() > MAX_BLOCKS {
            let rest: u64 = node.kids[MAX_BLOCKS..].iter().map(|k| self.tree[*k as usize].size).sum();
            v.push((Item::Rest((node.kids.len() - MAX_BLOCKS) as u32), rest));
        }
        if view == 0 { v.push((Item::Free, self.space.0)); v.push((Item::Hidden, self.hidden())); }
        v.retain(|i| i.1 > 0);
        v.sort_by_key(|i| std::cmp::Reverse(i.1));
        v
    }

    fn size_of(&self, it: Item) -> u64 {
        match it {
            Item::Node(n) => self.tree[n as usize].size,
            Item::Free => self.space.0,
            Item::Hidden => self.hidden(),
            Item::Rest(_) => self.items(self.view).iter().find(|i| i.0 == it).map_or(0, |i| i.1),
        }
    }

    fn guard(&mut self) -> &Guard { self.guard.get_or_insert_with(Guard::system) }

    /// Where an item lives on disk, and whether it may go to the Recycle Bin. "Smaller files" points at its folder.
    fn target(&mut self, it: Item) -> Option<(u32, String, bool, bool)> {
        let Item::Node(n) = it else { return None };
        let node = &self.tree[n as usize];
        if node.kind == Kind::Smaller { let p = node.parent; return Some((p, self.path_of(p), true, false)); }
        let folder = node.kind != Kind::File;
        let path = self.path_of(n);
        let ok = self.guard().allows(&path);
        Some((n, path, folder, ok))
    }

    fn up(&mut self) {
        if self.view == 0 || self.tree.is_empty() { return; }
        self.sel = Some(Item::Node(self.view));
        self.view = self.tree[self.view as usize].parent;
    }

    fn go_inside(&mut self, it: Item) {
        if let Item::Node(n) = it {
            let node = &self.tree[n as usize];
            if node.kind == Kind::Folder && !node.kids.is_empty() { self.view = n; self.sel = None; }
        }
    }

    /// Takes a removed item out of the tree: its size leaves every folder above it.
    fn remove(&mut self, n: u32) {
        if n == 0 { return; }
        let size = self.tree[n as usize].size;
        let mut p = self.tree[n as usize].parent;
        self.tree[p as usize].kids.retain(|k| *k != n);
        loop {
            self.tree[p as usize].size = self.tree[p as usize].size.saturating_sub(size);
            let mut kids = std::mem::take(&mut self.tree[p as usize].kids);
            kids.sort_by_key(|k| std::cmp::Reverse(self.tree[*k as usize].size));
            self.tree[p as usize].kids = kids;
            if p == 0 { break; }
            p = self.tree[p as usize].parent;
        }
        if self.sel == Some(Item::Node(n)) { self.sel = None; }
    }
}

// ---------- what is never offered ----------

/// The paths never offered for removal, lowercased without a trailing backslash.
pub struct Guard { keep: Vec<String>, inside: Vec<String>, users: String, profile: String }

fn norm(s: &str) -> String { s.strip_prefix(r"\\?\").unwrap_or(s).trim_end_matches('\\').to_lowercase() }
fn under(p: &str, d: &str) -> bool { p == d || (p.len() > d.len() && p.starts_with(d) && p.as_bytes()[d.len()] == b'\\') }

impl Guard {
    /// `keep` are folders that may not go themselves (nor anything that holds them); everything inside Windows stays.
    fn new(windows: &str, keep: &[String], profile: &str) -> Guard {
        let profile = norm(profile);
        let mut k: Vec<String> = keep.iter().map(|s| norm(s)).filter(|s| !s.is_empty()).collect();
        if !profile.is_empty() {
            k.push(profile.clone());
            for f in ["desktop", "documents", "downloads", "pictures", "music", "videos", "appdata"] { k.push(format!("{profile}\\{f}")); }
        }
        let users = profile.rsplit_once('\\').map(|(u, _)| u.to_string()).unwrap_or_default();
        Guard { keep: k, inside: [norm(windows)].into_iter().filter(|s| !s.is_empty()).collect(), users, profile }
    }

    fn system() -> Guard {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let mut keep: Vec<String> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData"].iter().map(|k| env(k)).collect();
        // Known folders can be moved (to OneDrive, another drive): their real places count too.
        for id in [FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures, FOLDERID_Music, FOLDERID_Videos, FOLDERID_RoamingAppData, FOLDERID_LocalAppData] {
            unsafe {
                if let Ok(p) = SHGetKnownFolderPath(&id, KNOWN_FOLDER_FLAG(0), None) {
                    keep.push(p.to_string().unwrap_or_default());
                    CoTaskMemFree(Some(p.0 as *const _));
                }
            }
        }
        Guard::new(&env("SystemRoot"), &keep, &env("USERPROFILE"))
    }

    pub fn allows(&self, path: &str) -> bool {
        let p = norm(path);
        // A drive root, or a path that is not absolute.
        if p.len() <= 2 || !p.contains('\\') { return false; }
        if let Some(first) = p.split('\\').nth(1) { if first == "$recycle.bin" || first == "system volume information" { return false; } }
        if self.keep.iter().any(|k| under(k, &p)) { return false; }
        if self.inside.iter().any(|d| under(&p, d)) { return false; }
        if !self.users.is_empty() && under(&p, &self.users) && !under(&p, &self.profile) { return false; }
        true
    }
}

// ---------- layout ----------

/// Squarified treemap (Bruls, Huizing, van Wijk): one rectangle per size, filling `r` with areas in proportion.
/// Sizes biggest first give the squarest blocks; zero sizes get empty rectangles.
pub fn squarify(sizes: &[f64], r: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::new(r.x, r.y, 0.0, 0.0); sizes.len()];
    let idx: Vec<usize> = (0..sizes.len()).filter(|i| sizes[*i] > 0.0).collect();
    let total: f64 = idx.iter().map(|i| sizes[*i]).sum();
    if total <= 0.0 || r.w <= 0.0 || r.h <= 0.0 { return out; }
    let k = r.w * r.h / total;
    let mut rest = r;
    let mut i = 0;
    while i < idx.len() {
        let side = rest.w.min(rest.h);
        let worst = |sum: f64, lo: f64, hi: f64| (side * side * hi / (sum * sum)).max(sum * sum / (side * side * lo));
        let a0 = sizes[idx[i]] * k;
        let (mut j, mut sum, mut lo, mut hi) = (i + 1, a0, a0, a0);
        while j < idx.len() {
            let a = sizes[idx[j]] * k;
            if worst(sum + a, lo.min(a), hi.max(a)) > worst(sum, lo, hi) { break; }
            sum += a; lo = lo.min(a); hi = hi.max(a); j += 1;
        }
        let row = &idx[i..j];
        let last_row = j == idx.len();
        if rest.w >= rest.h {
            // A column down the left side; the last one takes what is left so rounding leaves no gap.
            let cw = if last_row { rest.w } else { sum / rest.h };
            let mut y = rest.y;
            for (n, &t) in row.iter().enumerate() {
                let h = if n + 1 == row.len() { rest.bottom() - y } else { sizes[t] * k / cw };
                out[t] = Rect::new(rest.x, y, cw, h);
                y += h;
            }
            rest = Rect::new(rest.x + cw, rest.y, rest.w - cw, rest.h);
        } else {
            let rh = if last_row { rest.h } else { sum / rest.w };
            let mut x = rest.x;
            for (n, &t) in row.iter().enumerate() {
                let w = if n + 1 == row.len() { rest.right() - x } else { sizes[t] * k / rh };
                out[t] = Rect::new(x, rest.y, w, rh);
                x += w;
            }
            rest = Rect::new(rest.x, rest.y + rh, rest.w, rest.h - rh);
        }
        i = j;
    }
    out
}

/// The ramp through the box, from DiskMapRenderer.swift: maize, habanero, salsa, pickled onion, blue corn, jalapeño, guac.
const RAMP: [Color; 9] = [rgb(0xFFC43D), rgb(0xFF9440), rgb(0xF0654A), rgb(0xD86CC6), rgb(0xA98BF0), rgb(0x6F8FF4), rgb(0x35B996), rgb(0x6FC765), rgb(0xB7D65A)];
const SMALLER: Color = rgb(0x4A3A2D);

fn ramp(t: f64) -> Color {
    let p = t.clamp(0.0, 1.0) * (RAMP.len() - 1) as f64;
    let i = (p as usize).min(RAMP.len() - 2);
    RAMP[i].mix(RAMP[i + 1], (p - i as f64) as f32)
}

fn ink_on(c: Color) -> Color { if 0.299 * c.0 + 0.587 * c.1 + 0.114 * c.2 > 0.56 { ON_NACHO } else { BOARD_INK } }

/// Each block's stretch of the ramp: the share of the view it covers, so a big folder spans many tones.
fn stretches(sizes: &[u64]) -> Vec<(f64, f64)> {
    let total = sizes.iter().sum::<u64>().max(1) as f64;
    let mut at = 0.0;
    sizes.iter().map(|s| { let a = at; at += *s as f64 / total; (a, at) }).collect()
}

// ---------- text ----------

fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { out.push(','); } out.push(c); }
    out
}

fn share(a: u64, of: u64) -> String {
    if of == 0 { return "—".into(); }
    let v = a as f64 / of as f64 * 100.0;
    if v > 0.0 && v < 1.0 { "under 1%".into() } else { format::percent(v) }
}

/// A file time (100 ns since 1601) as "3 months ago".
fn ago(ft: u64) -> String {
    let t = (ft / 10_000_000) as i64 - 11_644_473_600;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let days = (now - t).max(0) / 86_400;
    match days { 0 => "today".into(), 1 => "yesterday".into(), 2..=59 => format!("{days} days ago"), 60..=729 => format!("{} months ago", days / 30), _ => format!("{} years ago", days / 365) }
}

// ---------- actions ----------

fn reveal(path: &str, folder_itself: bool) {
    let arg = if folder_itself { path.to_string() } else { format!("/select,{path}") };
    let _ = std::process::Command::new("explorer.exe").arg(arg).spawn();
}

fn ask_recycle(d: &mut DiskMap, it: Item) {
    let Some((n, path, _, true)) = d.target(it) else { return };
    let name = d.tree[n as usize].name.to_string();
    d.confirm = Some(Confirm { node: n, name, size: d.tree[n as usize].size, path });
}

/// Moves one item to the Recycle Bin after the confirmation; checked again right before it goes.
fn recycle(n: u32, path: String) {
    let ok = with(|d| d.tree.get(n as usize).is_some() && d.path_of(n) == path && d.guard().allows(&path));
    if !ok || std::fs::symlink_metadata(&path).is_err() {
        with(|d| d.note = "It is no longer there, so nothing was moved. Rescan to see the disk as it is now.".into());
        repaint();
        return;
    }
    let owner = with(|d| d.window).map_or(app().main, |h| HWND(h as *mut _));
    let from: Vec<u16> = path.encode_utf16().chain([0, 0]).collect();
    // FOF_WANTNUKEWARNING: Windows asks before anything too big for the Recycle Bin would be deleted for good.
    let mut op = SHFILEOPSTRUCTW { hwnd: owner, wFunc: FO_DELETE, pFrom: PCWSTR(from.as_ptr()), fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING).0 as u16, ..Default::default() };
    let r = unsafe { SHFileOperationW(&mut op) };
    let gone = std::fs::symlink_metadata(&path).is_err();
    with(|d| {
        let name = path.rsplit('\\').next().unwrap_or(&path).to_string();
        if gone {
            d.remove(n);
            d.recycled += 1;
            d.note = format!("{name} is in the Recycle Bin. The space comes back when you empty it.");
        } else if op.fAnyOperationsAborted.as_bool() || r == 0 {
            d.note = "Nothing was moved.".into();
        } else {
            d.note = format!("Windows could not move {name} (error {r}).");
        }
    });
    app().changed();
    repaint();
}

fn menu(it: Item) {
    let Some((_, path, folder, can)) = with(|d| d.target(it)) else { return };
    let smaller = with(|d| matches!(it, Item::Node(m) if d.tree[m as usize].kind == Kind::Smaller));
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(m, MF_STRING, 1, if folder { w!("Open Folder") } else { w!("Open") });
        let _ = AppendMenuW(m, MF_STRING, 2, w!("Show in File Explorer"));
        let _ = AppendMenuW(m, MF_STRING, 3, w!("Copy Path"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(m, if can { MF_STRING } else { MF_STRING | MF_GRAYED }, 4, w!("Move to Recycle Bin…\tDel"));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let owner = with(|d| d.window).map_or(app().main, |h| HWND(h as *mut _));
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, owner, None).0;
        let _ = DestroyMenu(m);
        match cmd {
            1 => crate::app::open_url(&path),
            2 => reveal(&path, smaller),
            3 => crate::capture::copy_text(&path),
            4 => { with(|d| ask_recycle(d, it)); repaint(); }
            _ => {}
        }
    }
}

// ---------- drawing ----------

fn draw(ui: &mut Ui, _ctx: &mut WindowCtx) { with(|d| draw_map(ui, d)); }

fn draw_map(ui: &mut Ui, d: &mut DiskMap) {
    let th = ui.th;
    let (w, h) = (ui.w, ui.h);
    let modal = d.confirm.is_some();
    ui.eyebrow("Disk Map", 20.0, 14.0, 200.0, th.nacho_ink);

    // Drive choice and Rescan, top right.
    let label = if d.scan.is_some() { "Stop Scan" } else { "Rescan" };
    let mut rx = w - 20.0 - ui.button_width(label, false);
    if ui.button("rescan", label, rx, 14.0, Btn::Secondary, false) { if d.scan.is_some() { cancel(d); } else { start_scan(d); } }
    if d.drives.len() > 1 {
        let sw = 56.0 * d.drives.len() as f64;
        rx -= sw + 10.0;
        let opts: Vec<String> = d.drives.iter().map(|r| r.trim_end_matches('\\').to_string()).collect();
        let cur = d.drives.iter().position(|r| *r == d.drive);
        if let Some(i) = ui.segments("drive", &opts, cur, Rect::new(rx, 14.0, sw, 30.0), false) {
            if Some(i) != cur { d.drive = d.drives[i].clone(); d.tree = Vec::new(); d.view = 0; d.sel = None; start_scan(d); }
        }
    }

    // The free space in large type.
    let (free, total) = d.space;
    let big = format!("{} FREE", format::bytes(free as f64));
    let bw = ui.c.measure(&big, sign(34), 800.0).0.ceil();
    ui.c.line_text(&big, sign(34), Rect::new(20.0, 32.0, bw + 4.0, 48.0), th.ink, Align::Left);
    ui.c.line_text(&format!("of {} on {}", format::bytes(total as f64), d.drive.trim_end_matches('\\')), medium(15), Rect::new(28.0 + bw, 44.0, 300.0, 30.0), th.ink2, Align::Left);
    let status = match (&d.scan, &d.summary) {
        (Some(p), _) => format!("Scanning {} — {} files, {} so far", d.drive, count(p.files.load(Relaxed)), format::bytes(p.bytes.load(Relaxed) as f64)),
        (None, Some(s)) => {
            let locked = if s.locked > 0 { format!(" · {} folders not readable", count(s.locked)) } else { String::new() };
            format!("{} files, {} read in {:.0} s{locked}", count(s.files), format::bytes(s.bytes as f64), s.secs.max(1.0))
        }
        _ => String::new(),
    };
    ui.c.line_text(&status, sans(12), Rect::new(20.0, 80.0, w - 40.0, 20.0), th.ink2, Align::Left);

    let map = Rect::new(20.0, 144.0, (w - SIDE - 56.0).max(100.0), (h - 164.0).max(60.0));
    if d.tree.is_empty() {
        let r = Rect::new(20.0, 144.0, w - 40.0, map.h);
        ui.card(r);
        let msg = if d.scan.is_some() { format!("Reading {} …", d.drive) } else { "Press Rescan to read this drive.".into() };
        ui.c.line_text(&msg, semi(15), Rect::new(r.x, r.cy() - 30.0, r.w, 28.0), th.ink, Align::Center);
        ui.c.line_text("Names and sizes are read on this PC and nothing leaves it. Links and junctions are not followed.", sans(12), Rect::new(r.x, r.cy(), r.w, 22.0), th.ink2, Align::Center);
        return;
    }
    if d.view as usize >= d.tree.len() { d.view = 0; }

    // Breadcrumb: back, then every folder from the drive down.
    let back = Rect::new(20.0, 108.0, 28.0, 28.0);
    let bid = ui.id("back");
    let br = ui.hit(bid, back, false);
    let can_up = d.view != 0;
    if br.hover && can_up { ui.c.round(back, 6.0, th.line); ui.st.cursor_hand = true; }
    ui.c.glyph(0xE72B, 13, back, if can_up { th.ink } else { th.ink2.alpha(0.4) });
    if br.clicked && !modal { d.up(); }
    let mut chain = vec![d.view];
    while *chain.last().unwrap() != 0 { let p = d.tree[*chain.last().unwrap() as usize].parent; chain.push(p); }
    chain.reverse();
    let mut x = 56.0;
    ui.c.clip(Rect::new(56.0, 104.0, map.w - 36.0, 36.0));
    for (k, n) in chain.iter().enumerate() {
        let name = if *n == 0 { d.drive.trim_end_matches('\\').to_string() } else { d.tree[*n as usize].name.to_string() };
        let tw = ui.c.measure(&name, semi(13), 400.0).0.ceil();
        let r = Rect::new(x, 108.0, tw + 12.0, 28.0);
        let id = ui.id(&format!("crumb{n}"));
        let resp = ui.hit(id, r, false);
        if resp.hover { ui.c.round(r, 6.0, th.line); ui.st.cursor_hand = true; }
        ui.c.line_text(&name, semi(13), r, if k + 1 == chain.len() { th.ink } else { th.ink2 }, Align::Center);
        if resp.clicked && !modal { d.view = *n; d.sel = None; }
        x += r.w;
        if k + 1 < chain.len() { ui.c.glyph(0xE76C, 9, Rect::new(x, 108.0, 14.0, 28.0), th.ink2); x += 14.0; }
    }
    ui.c.unclip();

    // The blocks.
    let items = d.items(d.view);
    let rects = squarify(&items.iter().map(|i| i.1 as f64).collect::<Vec<_>>(), map);
    let tones = stretches(&items.iter().map(|i| if matches!(i.0, Item::Node(_) | Item::Rest(_)) { i.1 } else { 0 }).collect::<Vec<_>>());
    let mouse = ui.st.mouse;
    let mut go = None;
    for (k, (&(it, size), r)) in items.iter().zip(&rects).enumerate() {
        let r = r.inset(1.0, 1.0);
        if r.w < 1.0 || r.h < 1.0 { continue; }
        let id = ui.id(&format!("{it:?}"));
        let resp = ui.hit(id, r, false);
        let (fill, ink) = match it {
            Item::Free => (th.guac.alpha(0.18), th.guac),
            Item::Hidden => (th.ink2.alpha(0.16), th.ink2),
            Item::Rest(_) => (SMALLER, BOARD_INK),
            Item::Node(n) => match d.tree[n as usize].kind {
                Kind::Smaller => (SMALLER, BOARD_INK),
                Kind::Locked => (BOARD_MUTE, ON_NACHO),
                _ => { let c = ramp((tones[k].0 + tones[k].1) / 2.0); (c, ink_on(c)) }
            },
        };
        ui.c.round(r, 3.0, if resp.hover && !modal { fill.mix(Color(1.0, 1.0, 1.0, fill.3), 0.18) } else { fill });
        if matches!(it, Item::Free | Item::Hidden) { ui.c.stroke_round(r, 3.0, ink.alpha(0.6), 1.0); }
        let mut header = false;
        if let Item::Node(n) = it {
            let node = &d.tree[n as usize];
            if node.kind == Kind::Folder && !node.kids.is_empty() && r.w > 80.0 && r.h > 64.0 {
                inner(ui, d, n, Rect::new(r.x + 3.0, r.y + 22.0, r.w - 6.0, r.h - 25.0), tones[k]);
                header = true;
            }
        }
        if r.w > 44.0 && r.h > 18.0 {
            let name = match it {
                Item::Node(n) => d.tree[n as usize].name.to_string(),
                Item::Free => "Free".into(),
                Item::Hidden => "Out of sight".into(),
                Item::Rest(c) => format!("{} more", count(c as u64)),
            };
            let sz = format::bytes(size as f64);
            if header || r.h < 40.0 {
                let sw = if r.w > 140.0 { ui.c.measure(&sz, mono(11), 200.0).0 + 8.0 } else { 0.0 };
                ui.c.line_text(&name, semi(12), Rect::new(r.x + 6.0, r.y + 2.0, r.w - 12.0 - sw, 18.0), ink, Align::Left);
                if sw > 0.0 { ui.c.line_text(&sz, mono(11), Rect::new(r.right() - sw - 6.0, r.y + 2.0, sw, 18.0), ink, Align::Right); }
            } else {
                ui.c.line_text(&name, semi(12), Rect::new(r.x + 6.0, r.y + 4.0, r.w - 12.0, 18.0), ink, Align::Left);
                let f = if r.w > 120.0 && r.h > 70.0 { sign(22) } else { mono(11) };
                ui.c.line_text(&sz, f, Rect::new(r.x + 6.0, r.y + 22.0, r.w - 12.0, if f == mono(11) { 16.0 } else { 30.0 }), ink, Align::Left);
            }
        }
        if d.sel == Some(it) { ui.c.stroke_round(r.inset(-1.0, -1.0), 4.0, th.ink, 2.5); }
        if modal { continue; }
        if resp.hover { ui.st.cursor_hand = true; }
        if resp.clicked { d.sel = Some(it); d.note.clear(); }
        if resp.right { d.sel = Some(it); later(move || menu(it)); }
        if ui.st.double_click && r.contains(mouse.0, mouse.1) { go = Some(it); }
    }
    if let Some(it) = go { d.go_inside(it); }

    side(ui, d, Rect::new(w - SIDE - 20.0, 144.0, SIDE, map.h));
    if modal { confirm(ui, d); return; }

    for k in ui.st.keys.clone() {
        match k.vk {
            0x08 => d.up(),
            0x25 if k.alt => d.up(),
            0x0D => if let Some(s) = d.sel { d.go_inside(s); },
            0x2E => if let Some(s) = d.sel { ask_recycle(d, s); },
            0x1B => d.sel = None,
            0x74 => start_scan(d),
            0x43 if k.ctrl => if let Some((_, p, _, _)) = d.sel.and_then(|s| d.target(s)) { later(move || crate::capture::copy_text(&p)); },
            _ => {}
        }
    }
}

/// One level deeper inside a big folder's block, on the folder's own stretch of the ramp, a little darker.
fn inner(ui: &mut Ui, d: &DiskMap, n: u32, r: Rect, (t0, t1): (f64, f64)) {
    let node = &d.tree[n as usize];
    let kids: Vec<u32> = node.kids.iter().take(60).copied().collect();
    let sizes: Vec<u64> = kids.iter().map(|k| d.tree[*k as usize].size).collect();
    let rest = node.size.saturating_sub(sizes.iter().sum());
    let mut all: Vec<f64> = sizes.iter().map(|s| *s as f64).collect();
    all.push(rest as f64);
    let rects = squarify(&all, r);
    let st = stretches(&sizes);
    for (i, rr) in rects.iter().enumerate() {
        let rr = rr.inset(0.5, 0.5);
        if rr.w < 2.0 || rr.h < 2.0 { continue; }
        let smaller = i == kids.len() || d.tree[kids[i] as usize].kind == Kind::Smaller;
        let c = if smaller { SMALLER } else { ramp(t0 + (t1 - t0) * (st[i].0 + st[i].1) / 2.0).mix(Color(0.0, 0.0, 0.0, 1.0), 0.12) };
        ui.c.round(rr, 2.0, c);
        if i < kids.len() && rr.w > 60.0 && rr.h > 18.0 {
            ui.c.line_text(&d.tree[kids[i] as usize].name, sans(11), Rect::new(rr.x + 4.0, rr.y + 1.0, rr.w - 8.0, 16.0), ink_on(c).alpha(0.85), Align::Left);
        }
    }
}

fn wide_button(ui: &mut Ui, key: &str, label: &str, x: f64, y: &mut f64, w: f64, style: Btn, enabled: bool) -> bool {
    let hit = ui.button_rect(key, label, Rect::new(x, *y, w, 30.0), style, false, enabled);
    *y += 38.0;
    hit
}

/// The card for the selected item, or the folder in view when nothing is selected.
fn side(ui: &mut Ui, d: &mut DiskMap, r: Rect) {
    let th = ui.th;
    ui.card(r);
    let (x, cw) = (r.x + 16.0, r.w - 32.0);
    let mut y = r.y + 16.0;
    let it = d.sel.unwrap_or(Item::Node(d.view));
    let size = d.size_of(it);
    let (kind, name, explain, changed, parent) = match it {
        Item::Free => ("Free space", "Free".to_string(), Some("Room left on this drive. What is in the Recycle Bin still counts as used until you empty it."), 0, None),
        Item::Hidden => ("Out of sight", "Used, not seen".to_string(), Some("Used space the scan could not see: system files, folders Windows keeps closed and changes since the scan."), 0, None),
        Item::Rest(c) => ("Summed up", format!("{} more items", count(c as u64)), Some("Items too small to draw here, summed up."), 0, Some(d.view)),
        Item::Node(n) => {
            let node = &d.tree[n as usize];
            let p = (n != 0).then_some(node.parent);
            match node.kind {
                Kind::Folder => (if n == 0 { "Drive" } else { "Folder" }, node.name.to_string(), None, node.changed, p),
                Kind::Locked => ("Folder · not readable", node.name.to_string(), Some("Windows did not let the scan read this folder, so it counts as empty here."), node.changed, p),
                Kind::File => ("File", node.name.to_string(), None, node.changed, p),
                Kind::Smaller => ("Summed up", "Smaller files".to_string(), Some("Files under 1 MB in this folder, summed up to keep the map light."), 0, p),
            }
        }
    };
    ui.eyebrow(kind, x, y, cw, th.nacho_ink);
    y += 22.0;
    y += ui.label(&name, semi(16), x, y, cw, th.ink) + 2.0;
    ui.c.line_text(&format::bytes(size as f64), sign(28), Rect::new(x, y, cw, 40.0), th.ink, Align::Left);
    y += 42.0;
    let of = match parent {
        Some(p) => format!("{} of {}", share(size, d.tree[p as usize].size), if p == 0 { d.drive.trim_end_matches('\\').to_string() } else { d.tree[p as usize].name.to_string() }),
        None => format!("{} of the drive", share(size, d.space.1)),
    };
    y += ui.label(&of, sans(12), x, y, cw, th.ink2) + 6.0;
    let target = d.target(it);
    if let Some((_, p, _, _)) = &target { y += ui.label(p, sans(12), x, y, cw, th.ink2) + 6.0; }
    if changed > 0 { y += ui.label(&format!("Changed {}", ago(changed)), sans(12), x, y, cw, th.ink2) + 6.0; }
    if let Some(e) = explain { y += ui.label(e, sans(12), x, y, cw, th.ink2) + 6.0; }
    y += 6.0;
    let modal = d.confirm.is_some();
    if let Some((_, path, folder, can)) = target {
        let smaller = matches!(it, Item::Node(n) if d.tree[n as usize].kind == Kind::Smaller);
        if wide_button(ui, "open", if folder { "Open Folder" } else { "Open" }, x, &mut y, cw, Btn::Secondary, !modal) { let p = path.clone(); later(move || crate::app::open_url(&p)); }
        if wide_button(ui, "reveal", "Show in File Explorer", x, &mut y, cw, Btn::Secondary, !modal) { let p = path.clone(); later(move || reveal(&p, smaller)); }
        if wide_button(ui, "copy", "Copy Path", x, &mut y, cw, Btn::Secondary, !modal) { let p = path.clone(); later(move || crate::capture::copy_text(&p)); }
        if !smaller {
            if wide_button(ui, "recycle", "Move to Recycle Bin…", x, &mut y, cw, Btn::Attention, can && !modal) { ask_recycle(d, it); }
            if !can { y += ui.label("Windows or your account needs this, so it is not offered for removal.", sans(12), x, y, cw, th.ink2) + 6.0; }
        }
    }
    if !d.note.is_empty() { ui.label(&d.note, medium(12), x, y + 4.0, cw, th.ink); }
}

/// The confirmation drawn over the window: what goes, where it is and what it adds up to.
fn confirm(ui: &mut Ui, d: &mut DiskMap) {
    let th = ui.th;
    let Some(c) = &d.confirm else { return };
    ui.c.fill(Rect::new(0.0, 0.0, ui.w, ui.h), Color(0.0, 0.0, 0.0, 0.35));
    // Everything under the dialog stops taking clicks.
    let block = ui.id("modal");
    ui.hit(block, Rect::new(0.0, 0.0, ui.w, ui.h), false);
    let (cw, chh) = (440.0, 214.0);
    let r = Rect::new((ui.w - cw) / 2.0, (ui.h - chh) / 2.0, cw, chh);
    ui.c.round(r, 12.0, th.paper);
    ui.c.stroke_round(r, 12.0, th.line, 1.0);
    let (x, w) = (r.x + 22.0, r.w - 44.0);
    let mut y = r.y + 20.0;
    ui.c.line_text("Move to the Recycle Bin?", semi(17), Rect::new(x, y, w, 26.0), th.ink, Align::Left);
    y += 34.0;
    ui.c.line_text(&format!("{}  ·  {}", c.name, format::bytes(c.size as f64)), medium(14), Rect::new(x, y, w, 22.0), th.ink, Align::Left);
    y += 24.0;
    y += ui.label(&c.path, sans(12), x, y, w, th.ink2) + 10.0;
    ui.label("You can put it back from the Recycle Bin. The space stays used until you empty it.", sans(12), x, y, w, th.ink2);
    let (node, path) = (c.node, c.path.clone());
    let by = r.bottom() - 50.0;
    let mw = ui.button_width("Move to Recycle Bin", false);
    let cancel_w = ui.button_width("Cancel", false);
    let go = ui.button("confirm.go", "Move to Recycle Bin", r.right() - 22.0 - mw, by, Btn::Attention, false);
    let no = ui.button("confirm.no", "Cancel", r.right() - 30.0 - mw - cancel_w, by, Btn::Secondary, false);
    if go { d.confirm = None; later(move || recycle(node, path)); }
    else if no || ui.st.keys.iter().any(|k| k.vk == 0x1B) { d.confirm = None; }
    ui.st.again |= go || no;
}

// ---------- quick panel ----------

/// The quick panel's block: the free space, a button to the map and the last scan.
pub fn controls(ui: &mut Ui, x: f64, y: f64, w: f64) -> f64 {
    let th = ui.th;
    let (drive, scanning, summary) = with(|d| (if d.drive.is_empty() { system_drive() } else { d.drive.clone() }, d.scan.as_ref().map(|p| p.files.load(Relaxed)), d.summary.clone()));
    let (free, total) = space(&drive);
    let mut y = y;
    y += ui.label(&format!("{} free of {} on {}", format::bytes(free as f64), format::bytes(total as f64), drive.trim_end_matches('\\')), semi(13), x, y, w, th.ink) + 8.0;
    if ui.button("diskmap.open", "Open Disk Map", x, y, Btn::Primary, false) { later(open); }
    y += 38.0;
    let line = match (scanning, summary) {
        (Some(n), _) => format!("Scanning… {} files so far.", count(n)),
        (None, Some(s)) => format!("Last scan of {}: {} files, {}.", s.drive.trim_end_matches('\\'), count(s.files), format::bytes(s.bytes as f64)),
        _ => "No scan yet. Opening the map starts one.".into(),
    };
    y += ui.label(&line, sans(12), x, y, w, th.ink2) + 4.0;
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(sizes: &[f64], r: Rect) {
        let out = squarify(sizes, r);
        let total: f64 = sizes.iter().sum();
        let eps = 1e-6 * r.w * r.h;
        let mut covered = 0.0;
        for (i, a) in out.iter().enumerate() {
            assert!(a.x >= r.x - 1e-9 && a.y >= r.y - 1e-9 && a.right() <= r.right() + 1e-6 && a.bottom() <= r.bottom() + 1e-6, "{i} outside");
            let want = sizes[i] / total * r.w * r.h;
            assert!((a.w * a.h - want).abs() < eps.max(want * 1e-6), "{i}: area {} want {want}", a.w * a.h);
            covered += a.w * a.h;
            for b in &out[i + 1..] { assert!(a.overlap_area(b) < eps, "overlap"); }
        }
        assert!((covered - r.w * r.h).abs() < eps);
    }

    #[test]
    fn squarify_covers_without_overlap() {
        check(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], Rect::new(0.0, 0.0, 600.0, 400.0));
        check(&[500.0, 120.0, 80.0, 33.0, 9.0, 1.0, 0.5], Rect::new(10.0, 20.0, 300.0, 700.0));
        check(&[1.0], Rect::new(0.0, 0.0, 50.0, 50.0));
        let many: Vec<f64> = (1..400).rev().map(|i| (i * i) as f64).collect();
        check(&many, Rect::new(0.0, 0.0, 900.0, 500.0));
        // Unsorted input still partitions the rectangle exactly.
        check(&[1.0, 9.0, 3.0, 27.0], Rect::new(0.0, 0.0, 200.0, 100.0));
    }

    #[test]
    fn squarify_zero_sizes_are_empty() {
        let out = squarify(&[3.0, 0.0, 1.0], Rect::new(0.0, 0.0, 40.0, 10.0));
        assert_eq!(out[1].w * out[1].h, 0.0);
        assert!((out[0].w * out[0].h - 300.0).abs() < 1e-9);
        assert!(squarify(&[0.0, 0.0], Rect::new(0.0, 0.0, 10.0, 10.0)).iter().all(|r| r.w * r.h == 0.0));
    }

    fn guard() -> Guard {
        let keep: Vec<String> = [r"C:\Program Files", r"C:\Program Files (x86)", r"C:\ProgramData", r"D:\Sync\Documents"].iter().map(|s| s.to_string()).collect();
        Guard::new(r"C:\Windows", &keep, r"C:\Users\me")
    }

    #[test]
    fn guard_refuses_protected() {
        let g = guard();
        for p in [r"C:\", "C:", r"D:\", r"C:\Windows", r"c:\WINDOWS\Temp", r"C:\Windows\System32\drivers\etc\hosts", r"C:\Program Files", r"C:\Program Files (x86)\",
            r"C:\ProgramData", r"C:\Users", r"C:\Users\me", r"C:\Users\me\Desktop", r"C:\Users\me\Documents", r"C:\Users\me\Downloads", r"C:\Users\me\Pictures",
            r"C:\Users\me\Music", r"C:\Users\me\Videos", r"C:\Users\me\AppData", r"C:\Users\bob", r"C:\Users\bob\Downloads\big.iso", r"C:\Users\Public", r"C:\Users\meow",
            r"D:\Sync", r"D:\Sync\Documents", r"\\?\C:\Windows", r"C:\$Recycle.Bin\S-1-5", r"C:\System Volume Information"] {
            assert!(!g.allows(p), "{p} should be refused");
        }
    }

    #[test]
    fn guard_allows_normal_items() {
        let g = guard();
        for p in [r"C:\Users\me\Downloads\old.iso", r"C:\Users\me\Documents\Projects", r"C:\Users\me\AppData\Local\Temp\cache", r"C:\Games\Big Game",
            r"C:\Program Files\Some App", r"D:\Sync\Documents\notes.txt", r"C:\WindowsApps-old"] {
            assert!(g.allows(p), "{p} should be allowed");
        }
    }

    #[test]
    fn scan_sums_folders_and_skips_junctions() {
        let root = std::env::temp_dir().join(format!("diskmap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::File::create(root.join("big.bin")).unwrap().set_len(3 << 20).unwrap();
        std::fs::write(root.join("a.txt"), [0u8; 100]).unwrap();
        std::fs::write(root.join("sub").join("b.txt"), [0u8; 50]).unwrap();
        // A junction back to the root would loop forever if it were followed.
        assert!(std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(root.join("loop")).arg(&root).output().unwrap().status.success());
        let name = format!("{}\\", root.display());
        let mut w = Walker { tree: vec![Node { name: name.as_str().into(), size: 0, parent: 0, kids: vec![], kind: Kind::Folder, changed: 0 }], p: Arc::new(Progress::default()), main: 0, last: Instant::now() };
        let mut path: Vec<u16> = format!(r"\\?\{name}").encode_utf16().collect();
        let total = w.dir(&mut path, 0);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(total, (3 << 20) + 150);
        let names: Vec<(&str, Kind, u64)> = w.tree[0].kids.iter().map(|k| { let n = &w.tree[*k as usize]; (&*n.name, n.kind, n.size) }).collect();
        assert_eq!(names, vec![("big.bin", Kind::File, 3 << 20), ("Smaller files", Kind::Smaller, 100), ("sub", Kind::Folder, 50)]);
        let d = DiskMap { tree: w.tree, ..Default::default() };
        assert_eq!(d.path_of(d.tree[0].kids[2]), format!("{}\\sub", root.display()));
    }

    #[test]
    fn remove_subtracts_upward() {
        let n = |name: &str, size, parent, kids: Vec<u32>, kind| Node { name: name.into(), size, parent, kids, kind, changed: 0 };
        let mut d = DiskMap { tree: vec![n("C:\\", 100, 0, vec![1, 2], Kind::Folder), n("a", 70, 0, vec![3], Kind::Folder), n("b", 30, 0, vec![], Kind::File), n("x", 70, 1, vec![], Kind::File)], ..Default::default() };
        d.remove(3);
        assert_eq!((d.tree[0].size, d.tree[1].size), (30, 0));
        assert_eq!(d.tree[0].kids, vec![2, 1]);
        assert!(d.tree[1].kids.is_empty());
    }

    #[test]
    fn text() {
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(count(12), "12");
        assert_eq!(share(1, 1000), "under 1%");
        assert_eq!(share(1, 4), "25%");
    }
}
