//! Mouse Tune's rules: how the wheel direction and the system values follow the options, and how the user's own
//! values come back. No Windows calls here; `src/mouse.rs` reads and writes the real values.
use serde::{Deserialize, Serialize};

/// One wheel notch, as Windows reports it. Smaller or odd steps come from precision touchpads and smooth wheels.
pub const NOTCH: i32 = 120;

/// What Windows keeps for pointer precision ("Enhance pointer precision").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Precision { #[default] Keep, On, Off }

pub const SPEED_CHOICES: [f64; 5] = [0.5, 1.0, 1.5, 2.0, 3.0];
/// 0 leaves the pointer speed as the user set it; Windows offers 1 to 20 and starts at 10.
pub const POINTER_CHOICES: [i32; 8] = [0, 4, 6, 8, 10, 12, 14, 16];

/// The three system values this gadget may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sys {
    /// SPI_GETMOUSESPEED, 1 to 20.
    pub pointer_speed: i32,
    /// SPI_GETWHEELSCROLLLINES: lines per notch. 0 or less means "one screen" (Windows stores it as u32::MAX).
    pub wheel_lines: i32,
    /// "Enhance pointer precision".
    pub precision: bool,
}

/// What was there before the gadget changed anything, and what the gadget set. Kept on disk so a crash or a kill
/// still gets the user's values back at the next start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved { pub original: Sys, pub applied: Sys }

/// Whether a wheel event is one plain notch of a wheel mouse (a multiple of 120). Precision touchpads send small
/// steps, so they keep their own direction.
pub fn is_notch(delta: i32) -> bool { delta != 0 && delta % NOTCH == 0 }

/// The wheel delta to deliver instead of `delta`.
pub fn wheel_delta(delta: i32, reverse: bool) -> i32 { if reverse { -delta } else { delta } }

/// Lines per notch for a speed factor. 1× and "one screen" are left exactly as they are.
pub fn scaled_lines(original: i32, speed: f64) -> i32 {
    if original <= 0 || !speed.is_finite() || speed == 1.0 { return original; }
    ((original as f64 * speed).round() as i32).clamp(1, 100)
}

pub fn clamp_pointer(v: i32) -> i32 { v.clamp(1, 20) }

/// The system values the options ask for, starting from the user's own `original`.
pub fn target(original: Sys, speed: f64, pointer: i32, precision: Precision) -> Sys {
    Sys {
        pointer_speed: if pointer == 0 { original.pointer_speed } else { clamp_pointer(pointer) },
        wheel_lines: scaled_lines(original.wheel_lines, speed),
        precision: match precision { Precision::Keep => original.precision, Precision::On => true, Precision::Off => false },
    }
}

/// The values to put back. A value the user changed meanwhile (it no longer equals what the gadget set) is theirs now
/// and stays.
pub fn restore(saved: &Saved, current: Sys) -> Sys {
    Sys {
        pointer_speed: if current.pointer_speed == saved.applied.pointer_speed { saved.original.pointer_speed } else { current.pointer_speed },
        wheel_lines: if current.wheel_lines == saved.applied.wheel_lines { saved.original.wheel_lines } else { current.wheel_lines },
        precision: if current.precision == saved.applied.precision { saved.original.precision } else { current.precision },
    }
}

/// The user's own values at start: the current ones, or, after a crash that left the gadget's values behind, the ones
/// saved before.
pub fn baseline(saved: Option<&Saved>, current: Sys) -> Sys {
    match saved { Some(s) => restore(s, current), None => current }
}

#[cfg(test)]
mod tests {
    use super::*;
    const USER: Sys = Sys { pointer_speed: 10, wheel_lines: 3, precision: true };

    #[test] fn only_whole_notches_count_as_a_wheel() {
        assert!(is_notch(120) && is_notch(-120) && is_notch(240));
        assert!(!is_notch(0) && !is_notch(30) && !is_notch(-7) && !is_notch(121));
    }
    #[test] fn reverse_flips_the_direction_and_nothing_else() {
        assert_eq!(wheel_delta(120, true), -120);
        assert_eq!(wheel_delta(-240, true), 240);
        assert_eq!(wheel_delta(120, false), 120);
    }
    #[test] fn scroll_speed_scales_the_lines_and_stays_in_range() {
        assert_eq!(scaled_lines(3, 1.0), 3);
        assert_eq!(scaled_lines(3, 2.0), 6);
        assert_eq!(scaled_lines(3, 0.5), 2);
        assert_eq!(scaled_lines(1, 0.5), 1, "never below one line");
        assert_eq!(scaled_lines(60, 3.0), 100);
        assert_eq!(scaled_lines(-1, 2.0), -1, "one screen per notch stays one screen");
        assert_eq!(scaled_lines(3, f64::NAN), 3);
    }
    #[test] fn defaults_change_nothing() {
        assert_eq!(target(USER, 1.0, 0, Precision::Keep), USER);
    }
    #[test] fn options_map_to_system_values() {
        let t = target(USER, 2.0, 14, Precision::Off);
        assert_eq!(t, Sys { pointer_speed: 14, wheel_lines: 6, precision: false });
        assert_eq!(target(USER, 1.0, 99, Precision::On).pointer_speed, 20);
        assert!(target(Sys { precision: false, ..USER }, 1.0, 0, Precision::On).precision);
    }
    #[test] fn switching_off_puts_the_users_values_back() {
        let applied = target(USER, 2.0, 14, Precision::Off);
        let saved = Saved { original: USER, applied };
        assert_eq!(restore(&saved, applied), USER);
    }
    #[test] fn a_value_the_user_changed_meanwhile_is_left_alone() {
        let applied = target(USER, 2.0, 14, Precision::Off);
        let saved = Saved { original: USER, applied };
        let now = Sys { pointer_speed: 7, ..applied };
        assert_eq!(restore(&saved, now), Sys { pointer_speed: 7, wheel_lines: 3, precision: true });
    }
    #[test] fn a_value_the_gadget_never_touched_is_not_overwritten() {
        // Pointer speed left as it is: the user may change it freely while the gadget runs.
        let applied = target(USER, 2.0, 0, Precision::Keep);
        let saved = Saved { original: USER, applied };
        let now = Sys { pointer_speed: 5, ..applied };
        assert_eq!(restore(&saved, now).pointer_speed, 5);
        assert_eq!(restore(&saved, now).wheel_lines, 3);
    }
    #[test] fn after_a_crash_the_saved_originals_are_the_baseline() {
        let applied = target(USER, 2.0, 14, Precision::Off);
        let saved = Saved { original: USER, applied };
        // The process died with the gadget's values still set: the next start must not mistake them for the user's.
        assert_eq!(baseline(Some(&saved), applied), USER);
        assert_eq!(baseline(None, USER), USER);
        // The user changed one value in Windows' settings before restarting: that one is their new baseline.
        let edited = Sys { wheel_lines: 5, ..applied };
        assert_eq!(baseline(Some(&saved), edited), Sys { wheel_lines: 5, ..USER });
    }
}
