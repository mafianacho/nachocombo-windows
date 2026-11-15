#![cfg_attr(not(test), windows_subsystem = "windows")]
#![allow(clippy::too_many_arguments)]
mod app;
mod appswitch;
mod awake;
mod capture;
mod clock;
mod clipboard;
mod ctl;
mod devices;
mod dim;
mod diskmap;
mod gfx;
mod hooks;
mod i18n;
mod monitor;
mod monitor_data;
mod monitor_panels;
mod mouse;
mod pages;
mod panel;
mod peek;
mod pet_art;
mod pet_audio;
mod pet_cards;
mod pet_chat;
mod pet_theme;
mod pet_ui;
mod pet_win;
mod shelf;
mod snap;
mod tray;
mod trayorg;
mod ui;
mod update;
mod window;

use nachocombo::core;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("ctl") { std::process::exit(ctl::client(&args[1..])); }
    let dir = std::env::var_os("NACHOCOMBO_DIR").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("NachoCombo")))
        .unwrap_or_else(|| ".".into());
    let _ = std::fs::create_dir_all(&dir);
    // Started by an update or a rollback: the copy that started this one quits first.
    if let Some(pid) = args.iter().position(|a| a == "--after-update").and_then(|i| args.get(i + 1)).and_then(|p| p.parse::<u32>().ok()) { update::wait_for(pid); }
    unsafe {
        let mutex = CreateMutexW(None, true, w!("Local\\NachoComboSingleInstance"));
        // Opening the app again brings the running copy's window back.
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(app::MAIN_CLASS, None) {
                // Windows started this copy for the `nachogadget:` login link: it goes to the running one, which waits for it.
                if let Some(url) = core::pet::login_browser::callback_from_arguments(&args) {
                    let cd = windows::Win32::System::DataExchange::COPYDATASTRUCT { dwData: pet_ui::CALLBACK_TAG, cbData: url.len() as u32, lpData: url.as_ptr() as *mut _ };
                    let _ = SendMessageW(h, WM_COPYDATA, None, Some(LPARAM(&cd as *const _ as isize)));
                } else {
                    let _ = PostMessageW(Some(h), app::WM_WAKE, WPARAM(0), LPARAM(0));
                }
            }
            return;
        }
        // A login link with no running app has nothing waiting for it.
        if core::pet::login_browser::callback_from_arguments(&args).is_some() { return; }
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _ = windows::Win32::System::Ole::OleInitialize(None);
        if let Err(e) = gfx::init() {
            let msg = HSTRING::from(format!("Nacho Combo could not start its graphics: {e}"));
            MessageBoxW(None, &msg, w!("Nacho Combo"), MB_ICONERROR);
            return;
        }
        app::run(dir, args.iter().any(|a| a == "--debug-ctl"));
        windows::Win32::System::Ole::OleUninitialize();
        drop(mutex);
    }
}
