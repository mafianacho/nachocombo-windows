//! A debug control channel: `NachoCombo ctl <command…>` drives a copy started with `--debug-ctl` and prints its state.
use crate::app::{app, MAIN_CLASS};
use crate::core::layout::SnapAction;
use crate::core::settings::*;
use crate::core::shortcut::Shortcut;
use crate::{capture, dim, monitor, monitor_panels, pages, panel, shelf, snap, trayorg};
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::*;

/// The client side: sends the command, waits for it and prints `state.json`.
pub fn client(args: &[String]) -> i32 {
    unsafe {
        let Ok(h) = FindWindowW(MAIN_CLASS, None) else { eprintln!("Nacho Combo is not running"); return 1 };
        let text = args.join("\u{1f}");
        let cd = COPYDATASTRUCT { dwData: 1, cbData: text.len() as u32, lpData: text.as_ptr() as *mut _ };
        let mut result = 0usize;
        let sent = SendMessageTimeoutW(h, WM_COPYDATA, WPARAM(0), LPARAM(&cd as *const _ as isize), SMTO_NORMAL, 20_000, Some(&mut result));
        if sent.0 == 0 || result == 0 { eprintln!("Nacho Combo did not accept the command (start it with --debug-ctl)"); return 1; }
        let dir = std::env::var_os("NACHOCOMBO_DIR").map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("NachoCombo"))).unwrap_or_default();
        if let Ok(s) = std::fs::read_to_string(dir.join("state.json")) { println!("{s}"); }
    }
    0
}

fn arg(a: &[String], i: usize) -> &str { a.get(i).map(String::as_str).unwrap_or("") }

pub fn run(a: &[String]) {
    let x = app();
    match arg(a, 0) {
        "set" => if let Some(m) = ModuleId::from_key(arg(a, 1)) { x.set_enabled(m, arg(a, 2) == "on"); },
        "snap" => if let Some(s) = SnapAction::from_id(arg(a, 1)) { snap::run(s); },
        "action" => x.run_action(arg(a, 1)),
        "option" => {
            let mut v = serde_json::to_value(&x.settings.options).unwrap_or_default();
            let raw = arg(a, 2);
            let parsed = serde_json::from_str::<serde_json::Value>(raw).unwrap_or(serde_json::Value::String(raw.into()));
            v[arg(a, 1)] = parsed;
            if let Ok(o) = serde_json::from_value(v) { x.settings.options = o; x.save(); x.apply_options(); }
        }
        "actionshortcut" => {
            let id = arg(a, 1).to_string();
            let s = match arg(a, 2) { "clear" => None, "reset" => { x.update(|s| { s.action_shortcuts.remove(&id); }); x.sync_hotkeys(); return; } k => Some(Shortcut::new(k.parse().unwrap_or(0), arg(a, 3).parse().unwrap_or(0))) };
            x.update(|st| { st.action_shortcuts.insert(id, s); });
            x.sync_hotkeys();
        }
        "dim" => match arg(a, 1) {
            "intensity" => { x.settings.dim_intensity = clamp_dim(arg(a, 2).parse().unwrap_or(0.5)); x.save(); dim::options_changed(); }
            "pause" => dim::toggle_pause(),
            _ => dim::refresh(),
        },
        "tray" => match arg(a, 1) { "collapse" => trayorg::set_collapsed(true), "expand" => trayorg::set_collapsed(false), "keep" => trayorg::set_keep(arg(a, 2), arg(a, 3) != "off"), _ => trayorg::refresh_list() },
        "shelf" => match arg(a, 1) {
            "show" => shelf::show(), "hide" => shelf::close(), "addfile" => shelf::add_path(arg(a, 2)), "addtext" => shelf::add_plain_text(&a[2..].join(" ")),
            "addlink" => shelf::add_url(arg(a, 2)), "paste" => shelf::paste_clipboard(), _ => {}
        },
        "capture" => match arg(a, 1) {
            "select" => capture::select(capture::Mode::Area), "window" => capture::select(capture::Mode::Window), "screen" => capture::capture_screen(), "previous" => capture::capture_previous(),
            "lock" => capture::toggle_lock(), "hide" => capture::toggle_hide(), "closeall" => capture::close_all(),
            "live" => { let n = |i| arg(a, i).parse::<i32>().unwrap_or(0); capture::live_float(RECT { left: n(2), top: n(3), right: n(2) + n(4), bottom: n(3) + n(5) }); }
            "rect" => { let n = |i| arg(a, i).parse::<i32>().unwrap_or(0); capture::capture_rect(RECT { left: n(2), top: n(3), right: n(2) + n(4), bottom: n(3) + n(5) }); }
            _ => {}
        },
        "monitor" => match arg(a, 1) {
            "show" => if let Some(i) = MonitorItem::ALL.into_iter().find(|i| i.name().eq_ignore_ascii_case(arg(a, 2))) { monitor_panels::open(i); },
            "hide" => monitor::close_panel(),
            "items" => { let items: Vec<MonitorItem> = arg(a, 2).split(',').filter_map(|n| MonitorItem::ALL.into_iter().find(|i| i.name().eq_ignore_ascii_case(n))).collect(); x.update(|s| s.monitor_items = items); monitor::sync_tray(); }
            "publicip" => monitor::request_public_ip(),
            _ => {}
        },
        "pin" => if let Some(m) = ModuleId::from_key(arg(a, 1)) { let on = arg(a, 2) == "on"; x.update(|s| { s.pinned_tools.retain(|p| *p != m); if on { s.pinned_tools.push(m); } }); crate::tray::sync_pins(); },
        "panel" => match arg(a, 1) {
            "list" => panel::open(panel::Route::List), "combos" => panel::open(panel::Route::Combos), "close" => panel::close(),
            "tool" => if let Some(m) = ModuleId::from_key(arg(a, 2)) { panel::open_tool(m); },
            _ => {}
        },
        "gallery" | "page" => pages::ctl(&a[1..]),
        "combo" => match arg(a, 1) {
            "order" => if let Some(c) = crate::core::combos::all(&x.settings).into_iter().find(|c| c.id == arg(a, 2)) { x.order(&c); },
            "undo" => x.undo_order(),
            _ => {}
        },
        "snapshot" => pages::snapshot(arg(a, 1), std::path::Path::new(arg(a, 2))),
        "diskmap" => if arg(a, 1) == "open" { crate::app::later(crate::diskmap::open) },
        "appswitch" => match arg(a, 1) { "next" => crate::appswitch::cycle(true), "previous" => crate::appswitch::cycle(false), "commit" => { let i = x.appswitch.diagnostics()["selected"].as_u64().unwrap_or(0) as usize; crate::appswitch::commit(i) }, "cancel" => crate::appswitch::cancel(), "hide" => crate::appswitch::hide_others(), _ => {} },
        "peek" => crate::app::later(crate::peek::on_space),
        "quit" => crate::app::quit(),
        _ => {}
    }
}
