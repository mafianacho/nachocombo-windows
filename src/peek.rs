//! Space in File Explorer previews the selected file, like Quick Look. Part of File Shelf.
use std::path::PathBuf;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Ole::IOleWindow;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn class_of(h: HWND) -> String {
    let mut name = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut name) } as usize;
    String::from_utf16_lossy(&name[..n])
}

/// Space should preview only while the file list itself has the focus: not while renaming or typing in the address
/// or search box, which are edit controls.
pub fn explorer_list_focused() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if class_of(fg) != "CabinetWClass" { return false; }
        let tid = GetWindowThreadProcessId(fg, None);
        let mut gui = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        if GetGUIThreadInfo(tid, &mut gui).is_err() { return false; }
        class_of(gui.hwndFocus) == "DirectUIHWND"
    }
}

/// The file selected in the File Explorer window in front. Every open Explorer window is asked which window it is.
pub fn selected_in_front() -> Option<PathBuf> { find_selected().ok() }

/// Each step says where it stopped, for the debug channel.
fn find_selected() -> std::result::Result<PathBuf, String> {
    unsafe {
        let fg = GetForegroundWindow();
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).map_err(|e| format!("ShellWindows {e}"))?;
        let count = windows.Count().unwrap_or(0);
        let mut seen = vec![];
        for i in 0..count {
            let Ok(disp) = windows.Item(&VARIANT::from(i)) else { seen.push("item".to_string()); continue };
            let Ok(sp) = disp.cast::<IServiceProvider>() else { seen.push("sp".into()); continue };
            let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { seen.push("browser".into()); continue };
            let Ok(top) = browser.cast::<IOleWindow>().and_then(|w| w.GetWindow()) else { seen.push("hwnd".into()); continue };
            // Explorer windows with tabs nest the browser window inside the top-level one.
            if top != fg && GetAncestor(top, GA_ROOT) != fg { seen.push(format!("other {:?}", top.0)); continue; }
            let view = browser.QueryActiveShellView().map_err(|e| format!("view {e}"))?;
            let folder: IFolderView = view.cast().map_err(|e| format!("folderview {e}"))?;
            let items: IShellItemArray = folder.Items(SVGIO_SELECTION).map_err(|e| format!("selection {e}"))?;
            let item = items.GetItemAt(0).map_err(|e| format!("item0 {e}"))?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH).map_err(|e| format!("path {e}"))?;
            let path = name.to_string().map_err(|e| e.to_string());
            CoTaskMemFree(Some(name.0 as *const _));
            return path.map(PathBuf::from);
        }
        Err(format!("{count} windows, fg {:?}: {}", fg.0, seen.join(", ")))
    }
}

thread_local! { pub static LAST: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) }; }

pub fn on_space() {
    let found = find_selected();
    LAST.with(|l| *l.borrow_mut() = format!("{found:?}"));
    if let Ok(p) = found { crate::shelf::preview_file(p); }
}
