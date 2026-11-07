//! Tray Organizer: Windows 11 keeps a per-user "show this icon on the taskbar" switch for every tray icon under
//! `HKCU\Control Panel\NotifyIconSettings`. This gadget flips those switches and restores them when it stops.
use crate::app::{app, TIMER_TRAY};
use std::collections::BTreeMap;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const ROOT: PCWSTR = w!("Control Panel\\NotifyIconSettings");
/// `tray_original` value for an icon that had no switch value at all.
const ABSENT: u32 = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct TrayEntry { pub key: String, pub path: String, pub name: String, pub promoted: Option<u32> }

#[derive(Default)]
pub struct Organizer { pub entries: Vec<TrayEntry>, pub collapse_due: bool, pub changes: u32, pub notice: Option<String> }

impl Organizer {
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "entries": self.entries.len(), "changes": self.changes, "collapsed": app().settings.tray_collapsed, "autoCollapsePending": self.collapse_due })
    }
}

fn open(sub: &str, access: REG_SAM_FLAGS) -> Option<HKEY> {
    let path = HSTRING::from(if sub.is_empty() { "Control Panel\\NotifyIconSettings".to_string() } else { format!("Control Panel\\NotifyIconSettings\\{sub}") });
    let mut k = HKEY::default();
    unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &path, None, access, &mut k).is_ok().then_some(k) }
}

fn read_string(k: HKEY, name: PCWSTR) -> Option<String> {
    unsafe {
        let mut buf = [0u16; 1024];
        let mut n = (buf.len() * 2) as u32;
        RegGetValueW(k, None, name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut n)).ok().ok()?;
        Some(String::from_utf16_lossy(&buf[..(n as usize / 2).saturating_sub(1)]))
    }
}

fn read_dword(k: HKEY, name: PCWSTR) -> Option<u32> {
    unsafe {
        let (mut v, mut n) = (0u32, 4u32);
        RegGetValueW(k, None, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as *mut _), Some(&mut n)).ok().ok()?;
        Some(v)
    }
}

fn friendly(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let stem = file.rsplit_once('.').map_or(file, |x| x.0);
    stem.to_string()
}

/// None when Windows has no such list (Windows 10, or before 22H2).
pub fn read_entries() -> Option<Vec<TrayEntry>> {
    let root = open("", KEY_READ)?;
    let mut out = vec![];
    unsafe {
        for i in 0.. {
            let mut name = [0u16; 64];
            let mut n = name.len() as u32;
            if RegEnumKeyExW(root, i, Some(PWSTR(name.as_mut_ptr())), &mut n, None, None, None, None).is_err() { break; }
            let key = String::from_utf16_lossy(&name[..n as usize]);
            let Some(k) = open(&key, KEY_READ) else { continue };
            let path = read_string(k, w!("ExecutablePath")).unwrap_or_default();
            let promoted = read_dword(k, w!("IsPromoted"));
            let _ = RegCloseKey(k);
            if path.is_empty() { continue; }
            out.push(TrayEntry { key, name: friendly(&path), path, promoted });
        }
        let _ = RegCloseKey(root);
    }
    let _ = ROOT;
    out.sort_by_key(|a| a.name.to_lowercase());
    Some(out)
}

fn write_promoted(key: &str, value: Option<u32>) -> bool {
    let Some(k) = open(key, KEY_SET_VALUE) else { return false };
    let ok = unsafe {
        match value {
            Some(v) => RegSetValueExW(k, w!("IsPromoted"), None, REG_DWORD, Some(&v.to_le_bytes())).is_ok(),
            None => { let _ = RegDeleteValueW(k, w!("IsPromoted")); true }
        }
    };
    unsafe { let _ = RegCloseKey(k); }
    ok
}

/// Windows reads the switches when told the settings changed.
fn notify_shell() {
    unsafe {
        let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(w!("TraySettings").as_ptr() as isize), SMTO_ABORTIFHUNG, 200, None);
    }
}

fn own_path(path: &str) -> bool {
    std::env::current_exe().is_ok_and(|e| e.to_string_lossy().eq_ignore_ascii_case(path))
}

pub fn start() -> std::result::Result<(), String> {
    let a = app();
    let Some(entries) = read_entries() else { return Err("This version of Windows has no tray icon list (Windows 11 22H2 or later is needed).".into()) };
    // Only the first start after a clean stop records the originals; after a crash the recorded ones still hold.
    if a.settings.tray_original.is_empty() {
        let mut orig = BTreeMap::new();
        for e in &entries { orig.insert(e.path.clone(), e.promoted.unwrap_or(ABSENT)); }
        a.settings.tray_original = orig;
        a.save();
    }
    a.organizer.entries = entries;
    a.organizer.notice = None;
    if a.settings.tray_collapsed { apply(true); }
    Ok(())
}

pub fn stop() {
    let a = app();
    unsafe { let _ = KillTimer(Some(a.main), TIMER_TRAY); }
    a.organizer.collapse_due = false;
    if a.settings.tray_original.is_empty() { return; }
    if let Some(entries) = read_entries() {
        for e in entries {
            if let Some(v) = a.settings.tray_original.get(&e.path) {
                write_promoted(&e.key, if *v == ABSENT { None } else { Some(*v) });
            }
        }
        notify_shell();
    }
    a.settings.tray_original.clear();
    a.settings.tray_collapsed = false;
    a.save();
    a.organizer.entries.clear();
}

/// Writes the switches for the collapsed or expanded state. Icons seen for the first time get recorded too.
fn apply(collapsed: bool) {
    let a = app();
    let Some(entries) = read_entries() else { return };
    let mut failed = false;
    for e in &entries {
        if !a.settings.tray_original.contains_key(&e.path) { a.settings.tray_original.insert(e.path.clone(), e.promoted.unwrap_or(ABSENT)); }
        let keep = a.settings.tray_keep.iter().any(|k| k.eq_ignore_ascii_case(&e.path)) || own_path(&e.path);
        let want = if collapsed && !keep { 0 } else { 1 };
        if e.promoted != Some(want) {
            failed |= !write_promoted(&e.key, Some(want));
            a.organizer.changes += 1;
        }
    }
    notify_shell();
    a.organizer.entries = read_entries().unwrap_or_default();
    a.organizer.notice = failed.then(|| "Windows refused to change some icons.".to_string());
    a.settings.tray_collapsed = collapsed;
    a.save();
}

pub fn set_collapsed(collapsed: bool) {
    let a = app();
    if a.state(crate::core::settings::ModuleId::TrayOrganizer) != crate::app::State::Running { return; }
    apply(collapsed);
    let secs = a.settings.options.auto_collapse_seconds;
    unsafe {
        let _ = KillTimer(Some(a.main), TIMER_TRAY);
        a.organizer.collapse_due = !collapsed && secs > 0;
        if a.organizer.collapse_due { SetTimer(Some(a.main), TIMER_TRAY, secs as u32 * 1000, None); }
    }
    a.changed();
}

pub fn toggle() { let c = app().settings.tray_collapsed; set_collapsed(!c); }

pub fn on_timer() { set_collapsed(true); }

pub fn set_keep(path: &str, keep: bool) {
    let a = app();
    a.settings.tray_keep.retain(|k| !k.eq_ignore_ascii_case(path));
    if keep { a.settings.tray_keep.push(path.to_string()); }
    a.save();
    if a.settings.tray_collapsed { apply(true); }
    a.changed();
}

pub fn refresh_list() { if let Some(e) = read_entries() { app().organizer.entries = e; } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn friendly_names() {
        assert_eq!(friendly(r"C:\Users\x\Discord.exe"), "Discord");
        assert_eq!(friendly(r"{6D80}\WindowsApps\ChatGPT.exe"), "ChatGPT");
        assert_eq!(friendly("noext"), "noext");
    }
}
