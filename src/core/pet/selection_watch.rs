//! The selection bar's Windows-only decisions, platform-free so they are tested on any OS: when to look (the mouse and
//! key stream the shell's hooks report), where the bar goes, what the focused place may be read (password fields,
//! windows of an elevated app) and the copy fallback that gives the clipboard back.
//!
//! The Mac does the first part with global event monitors (`PetSelectionWatcher`) and the second with `NSPanel`
//! placement; the rules are the same, only the events differ (a Windows hook reports no click count and also reports
//! the modifier keys themselves).
use super::selection::{PetSelectionBarSettings, PetSelectionDelay, PetSelectionGate, PetSelectionProbe};
use crate::core::layout::Rect;

// ---------- when to look ----------

/// What the low-level hooks report. Positions are screen pixels; `ms` is a monotonic clock in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WatchEvent {
    LeftDown { x: i32, y: i32, ms: u64 },
    LeftUp { x: i32, y: i32, ms: u64, ctrl: bool },
    RightDown,
    Wheel,
    KeyDown { vk: u32 },
    KeyUp { vk: u32, shift: bool, ctrl: bool },
}

/// What the shell does about one event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WatchAction {
    None,
    /// A click, scroll or key outside the bar: cancel the wait, and close the bar if it shows.
    Dismiss,
    /// A selection may have just ended: look after this many seconds (a new `Check` replaces the one waiting).
    Check { after_secs: f64 },
}

/// Movement of more than this many pixels between press and release is a drag (the Mac uses 3 points).
pub const DRAG_PIXELS: f64 = 3.0;
/// Two releases this close in time (the system's double-click time) and place are a double click.
pub const DOUBLE_CLICK_PIXELS: f64 = 4.0;

pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;

pub fn is_arrow(vk: u32) -> bool { (VK_LEFT..=VK_DOWN).contains(&vk) }

/// Shift, Ctrl, Alt and the Windows keys: a Windows hook reports them as keys of their own, the Mac's monitors do not.
pub fn is_modifier(vk: u32) -> bool { matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B | 0x5C) }

#[derive(Clone, Debug, Default)]
pub struct SelectionWatcher {
    down: Option<(i32, i32)>,
    last_up: Option<(i32, i32, u64)>,
}

impl SelectionWatcher {
    /// `double_click_ms` is the system's double-click time.
    pub fn handle(&mut self, e: WatchEvent, settings: &PetSelectionBarSettings, double_click_ms: u64) -> WatchAction {
        match e {
            WatchEvent::LeftDown { x, y, .. } => {
                self.down = Some((x, y));
                WatchAction::Dismiss
            }
            WatchEvent::RightDown | WatchEvent::Wheel => WatchAction::Dismiss,
            WatchEvent::KeyDown { vk } => {
                if is_modifier(vk) || is_arrow(vk) { WatchAction::None } else { WatchAction::Dismiss }
            }
            WatchEvent::LeftUp { x, y, ms, ctrl } => {
                let moved = self.down.take().is_some_and(|(dx, dy)| ((x - dx) as f64).hypot((y - dy) as f64) > DRAG_PIXELS);
                let double = self.last_up.is_some_and(|(px, py, pm)| {
                    ms.saturating_sub(pm) <= double_click_ms && ((x - px) as f64).hypot((y - py) as f64) <= DOUBLE_CLICK_PIXELS
                });
                // The release that completed a double click does not start a triple click's "double".
                self.last_up = if double { None } else { Some((x, y, ms)) };
                if !(moved || double) { return WatchAction::None; }
                Self::schedule(settings, ctrl)
            }
            WatchEvent::KeyUp { vk, shift, ctrl } => {
                if shift && is_arrow(vk) { Self::schedule(settings, ctrl) } else { WatchAction::None }
            }
        }
    }

    fn schedule(settings: &PetSelectionBarSettings, ctrl: bool) -> WatchAction {
        if settings.delay == PetSelectionDelay::CommandOnly && !ctrl { return WatchAction::None; }
        WatchAction::Check { after_secs: settings.delay.seconds() }
    }
}

// ---------- where the bar goes ----------

pub struct SelectionBarPlacement;

impl SelectionBarPlacement {
    /// The bar's top-left, in screen pixels (y down): above the selection, below it when there is no room above, kept
    /// inside the work area. Without a selection rectangle the pointer stands in for it.
    pub fn origin(anchor: Option<Rect>, pointer: (f64, f64), size: (f64, f64), work: Rect, gap: f64) -> (f64, f64) {
        let a = anchor.unwrap_or(Rect::new(pointer.0, pointer.1, 0.0, 0.0));
        let (w, h) = size;
        let mut y = a.y - gap - h;
        if y < work.y { y = a.y + a.h + gap; }
        let y = y.min(work.y + work.h - h).max(work.y);
        let lo = work.x + gap;
        let hi = (work.x + work.w - w - gap).max(lo);
        (a.x.clamp(lo, hi), y)
    }
}

// ---------- what may be read ----------

/// Facts about the focused place the shell collects before it reads any text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FocusFacts {
    /// The foreground app's exe name.
    pub exe: Option<String>,
    /// UI Automation's `IsPassword` of the focused element.
    pub is_password: bool,
    /// The app runs elevated (as administrator), or Windows would not say: a program that is not elevated must not read
    /// into it, and the bar must not float over it.
    pub elevated_or_unknown: bool,
    /// The secure desktop (UAC, Ctrl+Alt+Del) or a locked session has the input.
    pub secure_desktop: bool,
    pub control_type: Option<String>,
}

impl FocusFacts {
    /// The probe the core gate decides on. A password field takes the role the gate looks for, an elevated or unknown
    /// window and the secure desktop count as secure input: nothing is read there.
    pub fn probe(&self) -> PetSelectionProbe {
        PetSelectionProbe {
            bundle_id: self.exe.clone(),
            role: if self.is_password { Some(PetSelectionGate::SECURE_ROLE.to_string()) } else { self.control_type.clone() },
            subrole: None,
            secure_input: self.elevated_or_unknown || self.secure_desktop,
            accessibility_trusted: true,
        }
    }
}

// ---------- the copy fallback ----------

/// The clipboard as the fallback needs it. `Snapshot` holds everything that can be given back.
pub trait SelectionClipboard {
    type Snapshot;
    /// Changes whenever anything is put on the clipboard (`GetClipboardSequenceNumber`).
    fn sequence(&self) -> u32;
    /// `None` when the clipboard cannot be kept whole (busy, or a format that cannot be copied): then nothing is done.
    fn snapshot(&mut self) -> Option<Self::Snapshot>;
    fn text(&mut self) -> Option<String>;
    /// Puts the snapshot back (an empty clipboard stays empty).
    fn restore(&mut self, s: Self::Snapshot);
    /// Sends Ctrl+C to the focused app.
    fn press_copy(&mut self);
    fn pause(&mut self, ms: u32);
}

/// How long the app gets to answer Ctrl+C.
pub const COPY_WAIT_STEPS: u32 = 40;
pub const COPY_WAIT_MS: u32 = 25;

/// Reads the selection by copying it, then gives the clipboard back exactly as it was. Nothing is sent when the
/// clipboard cannot be kept; when the app copies nothing the clipboard was never touched.
pub fn read_by_copy<C: SelectionClipboard>(c: &mut C) -> Option<String> {
    let keep = c.snapshot()?;
    let before = c.sequence();
    c.press_copy();
    let mut changed = false;
    for _ in 0..COPY_WAIT_STEPS {
        c.pause(COPY_WAIT_MS);
        if c.sequence() != before { changed = true; break; }
    }
    // One more look: a slow app may have copied just as the wait ended. The clipboard is given back in that case too.
    if !changed && c.sequence() == before { return None; }
    let text = c.text();
    c.restore(keep);
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> PetSelectionBarSettings { PetSelectionBarSettings { enabled: true, ..Default::default() } }
    const DBL: u64 = 500;

    fn drag(w: &mut SelectionWatcher, s: &PetSelectionBarSettings) -> WatchAction {
        w.handle(WatchEvent::LeftDown { x: 100, y: 100, ms: 0 }, s, DBL);
        w.handle(WatchEvent::LeftUp { x: 180, y: 104, ms: 300, ctrl: false }, s, DBL)
    }

    #[test]
    fn a_drag_asks_for_a_look_after_the_delay() {
        let mut w = SelectionWatcher::default();
        assert_eq!(drag(&mut w, &on()), WatchAction::Check { after_secs: 0.3 });
        let s = PetSelectionBarSettings { delay: PetSelectionDelay::Long, ..on() };
        assert_eq!(drag(&mut SelectionWatcher::default(), &s), WatchAction::Check { after_secs: 0.6 });
        let s = PetSelectionBarSettings { delay: PetSelectionDelay::Immediate, ..on() };
        assert_eq!(drag(&mut SelectionWatcher::default(), &s), WatchAction::Check { after_secs: 0.0 });
    }

    #[test]
    fn a_click_without_movement_looks_at_nothing() {
        let mut w = SelectionWatcher::default();
        w.handle(WatchEvent::LeftDown { x: 10, y: 10, ms: 0 }, &on(), DBL);
        // 3 pixels is still a click.
        assert_eq!(w.handle(WatchEvent::LeftUp { x: 12, y: 12, ms: 80, ctrl: false }, &on(), DBL), WatchAction::None);
    }

    #[test]
    fn two_quick_clicks_in_one_place_select_a_word() {
        let mut w = SelectionWatcher::default();
        let s = on();
        for (ms, expect) in [(0, WatchAction::None), (200, WatchAction::Check { after_secs: 0.3 })] {
            w.handle(WatchEvent::LeftDown { x: 50, y: 50, ms }, &s, DBL);
            assert_eq!(w.handle(WatchEvent::LeftUp { x: 50, y: 50, ms: ms + 40, ctrl: false }, &s, DBL), expect);
        }
        // The third click is a new first click, not another "double".
        w.handle(WatchEvent::LeftDown { x: 50, y: 50, ms: 400 }, &s, DBL);
        assert_eq!(w.handle(WatchEvent::LeftUp { x: 50, y: 50, ms: 440, ctrl: false }, &s, DBL), WatchAction::None);
    }

    #[test]
    fn slow_or_distant_clicks_are_not_a_double_click() {
        let mut w = SelectionWatcher::default();
        let s = on();
        w.handle(WatchEvent::LeftDown { x: 50, y: 50, ms: 0 }, &s, DBL);
        w.handle(WatchEvent::LeftUp { x: 50, y: 50, ms: 40, ctrl: false }, &s, DBL);
        w.handle(WatchEvent::LeftDown { x: 50, y: 50, ms: 900 }, &s, DBL);
        assert_eq!(w.handle(WatchEvent::LeftUp { x: 50, y: 50, ms: 940, ctrl: false }, &s, DBL), WatchAction::None);
        w.handle(WatchEvent::LeftDown { x: 300, y: 50, ms: 1000 }, &s, DBL);
        assert_eq!(w.handle(WatchEvent::LeftUp { x: 300, y: 50, ms: 1040, ctrl: false }, &s, DBL), WatchAction::None);
    }

    #[test]
    fn only_while_holding_ctrl_waits_for_ctrl() {
        let s = PetSelectionBarSettings { delay: PetSelectionDelay::CommandOnly, ..on() };
        let mut w = SelectionWatcher::default();
        w.handle(WatchEvent::LeftDown { x: 0, y: 0, ms: 0 }, &s, DBL);
        assert_eq!(w.handle(WatchEvent::LeftUp { x: 90, y: 0, ms: 50, ctrl: false }, &s, DBL), WatchAction::None);
        w.handle(WatchEvent::LeftDown { x: 0, y: 0, ms: 1000 }, &s, DBL);
        assert_eq!(w.handle(WatchEvent::LeftUp { x: 90, y: 0, ms: 1050, ctrl: true }, &s, DBL), WatchAction::Check { after_secs: 0.0 });
    }

    #[test]
    fn shift_arrow_selects_and_looks_when_the_key_comes_up() {
        let mut w = SelectionWatcher::default();
        let s = on();
        // Holding Shift and pressing an arrow does not close the bar; letting the arrow go looks.
        assert_eq!(w.handle(WatchEvent::KeyDown { vk: 0x10 }, &s, DBL), WatchAction::None);
        assert_eq!(w.handle(WatchEvent::KeyDown { vk: VK_RIGHT }, &s, DBL), WatchAction::None);
        assert_eq!(w.handle(WatchEvent::KeyUp { vk: VK_RIGHT, shift: true, ctrl: false }, &s, DBL), WatchAction::Check { after_secs: 0.3 });
        assert_eq!(w.handle(WatchEvent::KeyUp { vk: VK_RIGHT, shift: false, ctrl: false }, &s, DBL), WatchAction::None);
        assert_eq!(w.handle(WatchEvent::KeyUp { vk: 0x41, shift: true, ctrl: false }, &s, DBL), WatchAction::None);
    }

    #[test]
    fn typing_scrolling_and_clicking_elsewhere_close_the_bar() {
        let mut w = SelectionWatcher::default();
        let s = on();
        assert_eq!(w.handle(WatchEvent::KeyDown { vk: 0x41 }, &s, DBL), WatchAction::Dismiss);
        assert_eq!(w.handle(WatchEvent::KeyDown { vk: 0x0D }, &s, DBL), WatchAction::Dismiss);
        assert_eq!(w.handle(WatchEvent::Wheel, &s, DBL), WatchAction::Dismiss);
        assert_eq!(w.handle(WatchEvent::RightDown, &s, DBL), WatchAction::Dismiss);
        assert_eq!(w.handle(WatchEvent::LeftDown { x: 1, y: 1, ms: 0 }, &s, DBL), WatchAction::Dismiss);
        // Pressing only a modifier is not typing.
        for vk in [0x10, 0x11, 0x12, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C] {
            assert_eq!(w.handle(WatchEvent::KeyDown { vk }, &s, DBL), WatchAction::None, "{vk:#x}");
        }
    }

    // ---- placement

    const WORK: Rect = Rect::new(0.0, 0.0, 1920.0, 1040.0);

    #[test]
    fn the_bar_sits_above_the_selection() {
        let (x, y) = SelectionBarPlacement::origin(Some(Rect::new(400.0, 300.0, 120.0, 20.0)), (0.0, 0.0), (300.0, 34.0), WORK, 8.0);
        assert_eq!((x, y), (400.0, 300.0 - 8.0 - 34.0));
    }

    #[test]
    fn the_bar_drops_below_when_there_is_no_room_above() {
        let (_, y) = SelectionBarPlacement::origin(Some(Rect::new(400.0, 20.0, 120.0, 20.0)), (0.0, 0.0), (300.0, 34.0), WORK, 8.0);
        assert_eq!(y, 20.0 + 20.0 + 8.0);
    }

    #[test]
    fn the_bar_stays_inside_the_work_area() {
        let (x, _) = SelectionBarPlacement::origin(Some(Rect::new(1900.0, 300.0, 10.0, 20.0)), (0.0, 0.0), (300.0, 34.0), WORK, 8.0);
        assert_eq!(x, 1920.0 - 300.0 - 8.0);
        let (x, _) = SelectionBarPlacement::origin(Some(Rect::new(-50.0, 300.0, 10.0, 20.0)), (0.0, 0.0), (300.0, 34.0), WORK, 8.0);
        assert_eq!(x, 8.0);
        // A selection at the very bottom with no room above or below stays on screen.
        let (_, y) = SelectionBarPlacement::origin(Some(Rect::new(10.0, 1030.0, 10.0, 10.0)), (0.0, 0.0), (300.0, 34.0), Rect::new(0.0, 0.0, 800.0, 1040.0), 8.0);
        assert!(y + 34.0 <= 1040.0);
    }

    #[test]
    fn a_work_area_on_the_second_monitor_is_respected() {
        let second = Rect::new(1920.0, 0.0, 1280.0, 720.0);
        let (x, y) = SelectionBarPlacement::origin(Some(Rect::new(1930.0, 10.0, 40.0, 20.0)), (0.0, 0.0), (300.0, 34.0), second, 8.0);
        assert_eq!(x, 1930.0);
        assert_eq!(y, 38.0);
    }

    #[test]
    fn without_a_selection_rectangle_the_pointer_is_the_anchor() {
        let (x, y) = SelectionBarPlacement::origin(None, (600.0, 500.0), (300.0, 34.0), WORK, 8.0);
        assert_eq!((x, y), (600.0, 500.0 - 8.0 - 34.0));
    }

    // ---- what may be read

    fn facts(exe: &str) -> FocusFacts { FocusFacts { exe: Some(exe.into()), control_type: Some("Document".into()), ..Default::default() } }

    #[test]
    fn a_password_field_is_never_read() {
        let mut f = facts("chrome.exe");
        f.is_password = true;
        assert_eq!(PetSelectionGate::before_reading(&f.probe(), &on(), 0.0), Some(super::super::selection::PetSelectionGateReason::SecureField));
    }

    #[test]
    fn an_elevated_or_unknown_window_is_never_read() {
        let mut f = facts("regedit.exe");
        f.elevated_or_unknown = true;
        assert_eq!(PetSelectionGate::before_reading(&f.probe(), &on(), 0.0), Some(super::super::selection::PetSelectionGateReason::SecureInput));
        let mut f = facts("notepad.exe");
        f.secure_desktop = true;
        assert_eq!(PetSelectionGate::before_reading(&f.probe(), &on(), 0.0), Some(super::super::selection::PetSelectionGateReason::SecureInput));
    }

    #[test]
    fn an_ordinary_window_may_be_read_and_the_bar_off_reads_nothing() {
        assert_eq!(PetSelectionGate::before_reading(&facts("notepad.exe").probe(), &on(), 0.0), None);
        assert!(PetSelectionGate::before_reading(&facts("notepad.exe").probe(), &PetSelectionBarSettings::default(), 0.0).is_some());
        // Nobody in front (the desktop): no exe, no reading.
        assert!(PetSelectionGate::before_reading(&FocusFacts::default().probe(), &on(), 0.0).is_some());
    }

    #[test]
    fn terminals_and_the_apps_own_windows_are_left_alone() {
        assert!(PetSelectionGate::before_reading(&facts("WindowsTerminal.exe").probe(), &on(), 0.0).is_some());
        assert!(PetSelectionGate::before_reading(&facts("nachocombo.exe").probe(), &on(), 0.0).is_some());
    }

    // ---- the copy fallback

    #[derive(Default)]
    struct Fake {
        board: Option<String>,
        seq: u32,
        /// What Ctrl+C puts there (None: the app copies nothing).
        copies: Option<String>,
        pressed: u32,
        /// The app answers Ctrl+C only after this many pauses.
        slow: u32,
        waited: u32,
        armed: bool,
        refuses_snapshot: bool,
        restored: u32,
    }

    impl SelectionClipboard for Fake {
        type Snapshot = Option<String>;
        fn sequence(&self) -> u32 { self.seq }
        fn snapshot(&mut self) -> Option<Self::Snapshot> { if self.refuses_snapshot { None } else { Some(self.board.clone()) } }
        fn text(&mut self) -> Option<String> { self.board.clone() }
        fn restore(&mut self, s: Self::Snapshot) { self.board = s; self.seq += 1; self.restored += 1; }
        fn press_copy(&mut self) {
            self.pressed += 1;
            if self.slow == 0 { if let Some(t) = self.copies.clone() { self.board = Some(t); self.seq += 1; } } else { self.armed = true; }
        }
        fn pause(&mut self, _ms: u32) {
            self.waited += 1;
            if self.armed && self.waited >= self.slow {
                self.armed = false;
                if let Some(t) = self.copies.clone() { self.board = Some(t); self.seq += 1; }
            }
        }
    }

    #[test]
    fn the_copy_fallback_gives_the_clipboard_back() {
        let mut c = Fake { board: Some("my own copy".into()), copies: Some("the selection".into()), ..Default::default() };
        assert_eq!(read_by_copy(&mut c).as_deref(), Some("the selection"));
        assert_eq!(c.board.as_deref(), Some("my own copy"));
        assert_eq!(c.restored, 1);
    }

    #[test]
    fn a_slow_app_is_waited_for_and_the_clipboard_still_comes_back() {
        let mut c = Fake { board: Some("mine".into()), copies: Some("the selection".into()), slow: 30, ..Default::default() };
        assert_eq!(read_by_copy(&mut c).as_deref(), Some("the selection"));
        assert_eq!(c.board.as_deref(), Some("mine"));
        // An app slower than the wait copies nothing in time: nothing is read, and nothing was touched.
        let mut c = Fake { board: Some("mine".into()), copies: Some("x".into()), slow: 90, ..Default::default() };
        assert_eq!(read_by_copy(&mut c), None);
        assert_eq!(c.board.as_deref(), Some("mine"));
    }

    #[test]
    fn an_empty_clipboard_is_empty_again() {
        let mut c = Fake { board: None, copies: Some("the selection".into()), ..Default::default() };
        assert_eq!(read_by_copy(&mut c).as_deref(), Some("the selection"));
        assert_eq!(c.board, None);
    }

    #[test]
    fn an_app_that_copies_nothing_leaves_the_clipboard_untouched() {
        let mut c = Fake { board: Some("mine".into()), copies: None, ..Default::default() };
        assert_eq!(read_by_copy(&mut c), None);
        assert_eq!(c.board.as_deref(), Some("mine"));
        assert_eq!(c.restored, 0);
        assert_eq!(c.seq, 0);
    }

    #[test]
    fn nothing_is_pressed_when_the_clipboard_cannot_be_kept() {
        let mut c = Fake { board: Some("mine".into()), copies: Some("x".into()), refuses_snapshot: true, ..Default::default() };
        assert_eq!(read_by_copy(&mut c), None);
        assert_eq!(c.pressed, 0);
        assert_eq!(c.board.as_deref(), Some("mine"));
    }
}
