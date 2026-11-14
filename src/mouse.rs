//! Mouse Tune: pointer speed, scroll lines and pointer precision through the per-user system settings, and the wheel
//! direction through the mouse hook (`hooks.rs`). The user's own values are saved before the first change and put back
//! when the gadget stops, also after a crash. The rules live in `core::mouse`.
use crate::app::app;
use crate::core::mouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Applied and restored for this session only: nothing is written to the user's profile, so even a killed app is
/// undone by signing out.
const NOW: SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS = SPIF_SENDCHANGE;

fn read() -> Sys {
    let mut speed = 10i32;
    let mut lines = 3u32;
    let mut accel = [0i32; 3];
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETMOUSESPEED, 0, Some(&mut speed as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let _ = SystemParametersInfoW(SPI_GETMOUSE, 0, Some(accel.as_mut_ptr() as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    // "One screen per notch" is stored as u32::MAX: it reads as -1 here.
    Sys { pointer_speed: speed.clamp(1, 20), wheel_lines: lines as i32, precision: accel[2] != 0 }
}

fn write(now: Sys, old: Sys) {
    unsafe {
        if now.pointer_speed != old.pointer_speed {
            let _ = SystemParametersInfoW(SPI_SETMOUSESPEED, 0, Some(now.pointer_speed as usize as *mut _), NOW);
        }
        if now.wheel_lines != old.wheel_lines {
            let _ = SystemParametersInfoW(SPI_SETWHEELSCROLLLINES, now.wheel_lines as u32, None, NOW);
        }
        if now.precision != old.precision {
            // Keeps the thresholds Windows has; only the on/off value changes.
            let mut accel = [0i32; 3];
            let _ = SystemParametersInfoW(SPI_GETMOUSE, 0, Some(accel.as_mut_ptr() as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
            if now.precision { if accel[2] == 0 { accel = [6, 10, 1]; } } else { accel[2] = 0; }
            let _ = SystemParametersInfoW(SPI_SETMOUSE, 0, Some(accel.as_mut_ptr() as *mut _), NOW);
        }
    }
}

pub fn start() -> Result<(), String> {
    apply();
    Ok(())
}

/// Brings the system values in line with the options, counting from the user's own values.
fn apply() {
    let a = app();
    let o = &a.settings.options;
    let current = read();
    let base = baseline(a.settings.mouse_saved.as_ref(), current);
    let want = target(base, o.mouse_speed, o.mouse_pointer, o.mouse_precision);
    write(want, current);
    a.settings.mouse_saved = Some(Saved { original: base, applied: want });
    a.save();
}

pub fn options_changed() {
    if app().state(crate::core::settings::ModuleId::MouseTune) == crate::app::State::Running { apply(); }
}

/// Puts the user's values back and forgets them.
pub fn stop() {
    restore_saved();
}

fn restore_saved() {
    let a = app();
    let Some(saved) = a.settings.mouse_saved.take() else { return };
    let current = read();
    write(restore(&saved, current), current);
    a.save();
}

/// At launch: values left behind by a run that did not stop cleanly while the gadget is now off.
pub fn recover() {
    if !app().settings.is_on(crate::core::settings::ModuleId::MouseTune) { restore_saved(); }
}

pub fn toggle_reverse() {
    let a = app();
    if a.state(crate::core::settings::ModuleId::MouseTune) != crate::app::State::Running { return; }
    a.update(|s| s.options.mouse_reverse = !s.options.mouse_reverse);
    crate::hooks::sync();
}

pub fn diagnostics() -> serde_json::Value {
    let a = app();
    let s = read();
    serde_json::json!({
        "reverse": a.settings.options.mouse_reverse, "pointerSpeed": s.pointer_speed, "wheelLines": s.wheel_lines, "precision": s.precision,
        "saved": a.settings.mouse_saved.is_some(), "wheelFlips": crate::hooks::wheel_flips(),
    })
}
