#![cfg_attr(not(test), windows_subsystem = "windows")]
#![allow(clippy::too_many_arguments)]
mod app;
mod appswitch;
mod awake;
mod capture;
mod clipboard;
mod core;
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
mod pages;
mod panel;
mod peek;
mod shelf;
mod snap;
mod tray;
mod trayorg;
mod ui;
mod window;

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
    unsafe {
        let mutex = CreateMutexW(None, true, w!("Local\\NachoComboSingleInstance"));
        // Opening the app again brings the running copy's window back.
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(app::MAIN_CLASS, None) { let _ = PostMessageW(Some(h), app::WM_WAKE, WPARAM(0), LPARAM(0)); }
            return;
        }
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
