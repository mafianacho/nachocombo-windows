//! Global shortcuts: Windows virtual-key codes plus RegisterHotKey modifier bits.
use serde::{Deserialize, Serialize};

pub const ALT: u32 = 1;
pub const CTRL: u32 = 2;
pub const SHIFT: u32 = 4;
pub const WIN: u32 = 8;
pub const ALL_MODS: u32 = ALT | CTRL | SHIFT | WIN;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shortcut { pub key: u32, pub mods: u32 }

impl Shortcut {
    pub const fn new(key: u32, mods: u32) -> Self { Shortcut { key, mods } }
    pub fn parts(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.mods & CTRL != 0 { v.push("Ctrl".to_string()); }
        if self.mods & ALT != 0 { v.push("Alt".to_string()); }
        if self.mods & SHIFT != 0 { v.push("Shift".to_string()); }
        if self.mods & WIN != 0 { v.push("Win".to_string()); }
        v.push(key_name(self.key).unwrap_or_else(|| format!("Key {}", self.key)));
        v
    }
    pub fn label(&self) -> String { self.parts().join("+") }
}

pub fn is_function_key(k: u32) -> bool { (0x70..=0x7B).contains(&k) }

pub fn key_name(k: u32) -> Option<String> {
    Some(match k {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(k)?.to_string(),
        0x70..=0x7B => format!("F{}", k - 0x6F),
        0x08 => "Backspace".into(), 0x09 => "Tab".into(), 0x0D => "Enter".into(), 0x1B => "Esc".into(), 0x20 => "Space".into(),
        0x21 => "PgUp".into(), 0x22 => "PgDn".into(), 0x23 => "End".into(), 0x24 => "Home".into(),
        0x25 => "←".into(), 0x26 => "↑".into(), 0x27 => "→".into(), 0x28 => "↓".into(), 0x2D => "Ins".into(), 0x2E => "Del".into(),
        0xBA => ";".into(), 0xBB => "=".into(), 0xBC => ",".into(), 0xBD => "-".into(), 0xBE => ".".into(), 0xBF => "/".into(),
        0xC0 => "`".into(), 0xDB => "[".into(), 0xDC => "\\".into(), 0xDD => "]".into(), 0xDE => "'".into(),
        _ => return None,
    })
}

const ESC: u32 = 0x1B;

#[derive(Clone, Debug, PartialEq)]
pub enum Issue { UnsupportedKey, NeedsMoreModifiers, Reserved(String), Conflict(String) }

impl Issue {
    pub fn message(&self) -> String {
        match self {
            Issue::UnsupportedKey => "This key cannot be used in a shortcut.".into(),
            Issue::NeedsMoreModifiers => "Use at least two modifier keys (one is enough with a function key), so it never clashes with typing.".into(),
            Issue::Reserved(n) => format!("Windows uses this combination ({n})."),
            Issue::Conflict(n) => format!("Already used by {n}."),
        }
    }
}

/// Combinations Windows itself owns. `RegisterHotKey` refuses most, and the rest would break a habit.
fn reserved(s: Shortcut) -> Option<&'static str> {
    let k = |c: char| c as u32;
    let list: [(u32, u32, &str); 20] = [
        (0x2E, CTRL | ALT, "Security screen"), (ESC, CTRL | SHIFT, "Task Manager"), (0x09, ALT, "Switch windows"), (0x73, ALT, "Close window"),
        (k('L'), WIN, "Lock"), (k('D'), WIN, "Desktop"), (k('E'), WIN, "File Explorer"), (k('S'), WIN | SHIFT, "Snipping Tool"),
        (k('V'), WIN, "Clipboard history"), (k('R'), WIN, "Run"), (k('I'), WIN, "Settings"), (k('A'), WIN, "Quick Settings"),
        (0x25, WIN, "Snap left"), (0x27, WIN, "Snap right"), (0x26, WIN, "Maximize"), (0x28, WIN, "Minimize"),
        (0x25, WIN | CTRL, "Previous desktop"), (0x27, WIN | CTRL, "Next desktop"), (0x25, WIN | SHIFT, "Move to display"), (0x27, WIN | SHIFT, "Move to display"),
    ];
    list.iter().find(|(key, m, _)| *key == s.key && *m == s.mods).map(|x| x.2)
}

/// `taken` holds every other shortcut in use, with the name to show if it clashes.
pub fn validate(s: Shortcut, taken: &[(String, Shortcut)]) -> Option<Issue> {
    let mods = s.mods & ALL_MODS;
    if key_name(s.key).is_none() || s.key == ESC { return Some(Issue::UnsupportedKey); }
    let count = mods.count_ones();
    if count < if is_function_key(s.key) { 1 } else { 2 } { return Some(Issue::NeedsMoreModifiers); }
    if let Some(n) = reserved(Shortcut::new(s.key, mods)) { return Some(Issue::Reserved(n.into())); }
    taken.iter().find(|(_, t)| t.key == s.key && t.mods & ALL_MODS == mods).map(|(n, _)| Issue::Conflict(n.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn labels() {
        assert_eq!(Shortcut::new(0x25, CTRL | ALT).label(), "Ctrl+Alt+←");
        assert_eq!(Shortcut::new(0x70, WIN).label(), "Win+F1");
    }
    #[test] fn policy() {
        let s = |k, m| Shortcut::new(k, m);
        assert_eq!(validate(s(0x41, CTRL), &[]), Some(Issue::NeedsMoreModifiers));
        assert_eq!(validate(s(0x70, CTRL), &[]), None);
        assert_eq!(validate(s(ESC, CTRL | ALT), &[]), Some(Issue::UnsupportedKey));
        assert!(matches!(validate(s(0x2E, CTRL | ALT), &[]), Some(Issue::Reserved(_))));
        assert!(matches!(validate(s(0x25, WIN | SHIFT), &[]), Some(Issue::Reserved(_))));
        let taken = [("Snap".to_string(), s(0x41, CTRL | ALT))];
        assert!(matches!(validate(s(0x41, CTRL | ALT), &taken), Some(Issue::Conflict(_))));
        assert_eq!(validate(s(0x42, CTRL | ALT), &taken), None);
    }
}
