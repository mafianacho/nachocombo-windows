//! Where the chat bubble sits next to the pet and how big it is. Platform free so the rules (follow the pet, flip below
//! it at the top of the screen, stay on the pet's screen, size limits) are testable on any OS; the shell's chat window
//! only applies the result. Port of `PetChatLayout.swift`.
//!
//! Coordinates: the math is kept exactly in the Swift (AppKit) convention, **y grows upward** ("above the pet" = larger
//! y, `max_y` = top edge). Windows screen coordinates grow downward. The shell converts at the edges only:
//! - into the layout: [`PetRect::from_y_down`] (`left, top, width, height` of a Win32 `RECT`, e.g. the pet window and
//!   the monitor's work area from `GetMonitorInfo`) — it mirrors y about 0 (`y = -(top + height)`), which keeps every
//!   distance and above/below relation, also across monitors, as long as pet and screen are converted the same way;
//! - out of it: [`PetChatPlacement::window_top_y_down`] gives the window's Win32 `top` (`x` is unchanged);
//! - while resizing: a Win32 mouse delta `dy` must be negated before [`PetChatLayout::resized`] (here +dy = up).
//!
//! `tail_on_top` keeps its visual meaning (the tail is on the window's top edge, pointing up at the pet) on both.
use super::design_tokens::pet_tokens::{radius, size, space};
use serde::{Deserialize, Serialize};

/// The chat bubble's body (without the tail) in points. Saved in the settings (`chatSize: {width, height}`) when the
/// user resizes it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PetChatSize {
    pub width: f64,
    pub height: f64,
}

impl PetChatSize {
    pub const fn new(width: f64, height: f64) -> Self { Self { width, height } }
}

/// A rectangle in screen points with the origin at the bottom left (AppKit convention; see the module doc for
/// Windows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PetRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl PetRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self { Self { x, y, width, height } }
    /// From a y-down (Windows) rectangle: mirrored about y = 0, so `max_y()` is `-top` and `min_y()` is `-bottom`.
    pub fn from_y_down(left: f64, top: f64, width: f64, height: f64) -> Self { Self::new(left, -(top + height), width, height) }
    /// The y-down (Windows) top of this rectangle (inverse of [`PetRect::from_y_down`]).
    pub fn top_y_down(&self) -> f64 { -self.max_y() }
    pub fn min_x(&self) -> f64 { self.x }
    pub fn max_x(&self) -> f64 { self.x + self.width }
    pub fn min_y(&self) -> f64 { self.y }
    pub fn max_y(&self) -> f64 { self.y + self.height }
    pub fn mid_x(&self) -> f64 { self.x + self.width / 2.0 }
}

/// The chat window's place for one pet position: window origin (bottom left, the tail included), which side the tail
/// is on, and the tail's centre measured from the window's right edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PetChatPlacement {
    pub x: f64,
    pub y: f64,
    /// The chat is below the pet (not enough room above it), so the tail points up.
    pub tail_on_top: bool,
    pub tail_from_right: f64,
    /// The body height shown here: the chosen height, or less when it fits neither above nor below the pet.
    pub body_height: f64,
}

impl PetChatPlacement {
    /// The window's height (body plus tail).
    pub fn window_height(&self) -> f64 { self.body_height + PetChatLayout::TAIL }
    /// The window's top edge in y-down (Windows) coordinates, for rectangles made with [`PetRect::from_y_down`].
    pub fn window_top_y_down(&self) -> f64 { -(self.y + self.window_height()) }
}

/// The parts of the chat's edge the user can drag to resize it. The right edge is lined up with the pet and the tail
/// side touches the pet, so only the free sides move: the left edge, the edge away from the pet, and their corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PetChatEdge { Left, Top, Bottom, TopLeft, BottomLeft }

impl PetChatEdge {
    pub const ALL: [PetChatEdge; 5] = [Self::Left, Self::Top, Self::Bottom, Self::TopLeft, Self::BottomLeft];

    pub fn raw(self) -> &'static str {
        match self { Self::Left => "left", Self::Top => "top", Self::Bottom => "bottom", Self::TopLeft => "topLeft", Self::BottomLeft => "bottomLeft" }
    }
    pub fn from_raw(s: &str) -> Option<Self> { Self::ALL.into_iter().find(|e| e.raw() == s) }

    fn moves_left(self) -> bool { matches!(self, Self::Left | Self::TopLeft | Self::BottomLeft) }
    fn moves_top(self) -> bool { matches!(self, Self::Top | Self::TopLeft) }
    fn moves_bottom(self) -> bool { matches!(self, Self::Bottom | Self::BottomLeft) }
}

pub struct PetChatLayout;

impl PetChatLayout {
    /// The confirmed design's bubble (360 × 520).
    pub const DEFAULT_SIZE: PetChatSize = PetChatSize::new(size::CHAT_WIDTH, size::CHAT_HEIGHT);
    /// Smallest body that still shows the header (avatar, name, status, tabs), a few log lines and the composer.
    pub const MINIMUM_SIZE: PetChatSize = PetChatSize::new(320.0, 420.0);
    /// Room kept between the chat and the screen's edges.
    pub const SCREEN_MARGIN: f64 = space::S;
    pub(crate) const TAIL: f64 = size::CHAT_TAIL;
    const GAP: f64 = size::CHAT_GAP_TO_PET;
    const CORNER: f64 = radius::WINDOW;

    /// The window's size for a body size: the tail adds to the height.
    pub fn window_size(body: PetChatSize) -> PetChatSize { PetChatSize::new(body.width, body.height + Self::TAIL) }

    /// The largest body that fits in a screen's visible frame with the tail and the margins (never below the minimum).
    pub fn maximum_size(screen: PetRect) -> PetChatSize {
        PetChatSize::new(
            Self::MINIMUM_SIZE.width.max(screen.width - 2.0 * Self::SCREEN_MARGIN),
            Self::MINIMUM_SIZE.height.max(screen.height - 2.0 * Self::SCREEN_MARGIN - Self::TAIL),
        )
    }

    /// The saved size made safe: a missing, hand-edited or broken value falls back, and the body fits the screen it is
    /// shown on. Takes `PetChatSize` or `Option<PetChatSize>` (the setting is optional: `None` = the default).
    pub fn clamp(size: impl Into<Option<PetChatSize>>, screen: Option<PetRect>) -> PetChatSize {
        let s = match size.into() {
            Some(s) if s.width.is_finite() && s.height.is_finite() => s,
            _ => return Self::clamp(Self::DEFAULT_SIZE, screen),
        };
        let (mut w, mut h) = (Self::MINIMUM_SIZE.width.max(s.width), Self::MINIMUM_SIZE.height.max(s.height));
        if let Some(screen) = screen {
            let most = Self::maximum_size(screen);
            w = w.min(most.width);
            h = h.min(most.height);
        }
        PetChatSize::new(w.round(), h.round())
    }

    /// Above the pet with the tail on its head, right edges lined up; below the pet when it does not fit above; always
    /// inside the pet's screen. The same rule for opening the chat, following a dragged pet and resizing. A (resized)
    /// chat taller than the room on both sides goes to the side with more room and is shown only as tall as that room
    /// (never below the minimum), so it does not cover the pet; the chosen size is kept.
    pub fn place(body: PetChatSize, pet: PetRect, screen: PetRect) -> PetChatPlacement {
        let (m, gap, tail) = (Self::SCREEN_MARGIN, Self::GAP, Self::TAIL);
        let mut size = Self::window_size(body);
        let mut x = pet.max_x() - size.width;
        let mut y = pet.max_y() + gap;
        let mut below = y + size.height > screen.max_y();
        if below {
            let room_below = pet.min_y() - gap - (screen.min_y() + m);
            if size.height > room_below {
                let room_above = screen.max_y() - m - (pet.max_y() + gap);
                below = room_below > room_above;
                let room = if below { room_below } else { room_above };
                size.height = Self::window_size(Self::MINIMUM_SIZE).height.max(size.height.min(room));
            }
        }
        if below { y = pet.min_y() - size.height - gap; }
        x = x.max(screen.min_x() + m).min(screen.max_x() - size.width - m);
        y = y.max(screen.min_y() + m).min(screen.max_y() - size.height - m);
        let tail_from_right = (x + size.width - pet.mid_x()).max(Self::CORNER + tail).min(size.width - Self::CORNER - tail);
        PetChatPlacement { x, y, tail_on_top: below, tail_from_right, body_height: size.height - tail }
    }

    /// The edges that can be dragged for the current side of the pet (the tail side never moves).
    pub fn resizable_edges(tail_on_top: bool) -> Vec<PetChatEdge> {
        use PetChatEdge::*;
        if tail_on_top { vec![Left, Bottom, BottomLeft] } else { vec![Left, Top, TopLeft] }
    }

    /// The largest body while dragging an edge: as wide as the screen, and as tall as the room on the pet's current
    /// side, so the chat does not jump to the other side of the pet in the middle of a drag.
    pub fn drag_limit(pet: PetRect, screen: PetRect, tail_on_top: bool) -> PetChatSize {
        let most = Self::maximum_size(screen);
        let (m, gap, tail) = (Self::SCREEN_MARGIN, Self::GAP, Self::TAIL);
        let room = if tail_on_top { pet.min_y() - gap - tail - (screen.min_y() + m) } else { (screen.max_y() - m) - (pet.max_y() + gap) - tail };
        PetChatSize::new(most.width, most.height.min(Self::MINIMUM_SIZE.height.max(room)))
    }

    /// The body after dragging `edge` by (dx, dy) points (+dy = up; negate a Windows delta) from the size the drag
    /// started with, within the limits.
    pub fn resized(start: PetChatSize, edge: PetChatEdge, dx: f64, dy: f64, limit: PetChatSize) -> PetChatSize {
        let (mut w, mut h) = (start.width, start.height);
        if edge.moves_left() { w -= dx; }
        if edge.moves_top() { h += dy; }
        if edge.moves_bottom() { h -= dy; }
        let min = Self::MINIMUM_SIZE;
        w = w.max(min.width).min(min.width.max(limit.width));
        h = h.max(min.height).min(min.height.max(limit.height));
        PetChatSize::new(w.round(), h.round())
    }

    /// The size worth saving: `None` when it is the default, so a later design change of the default still applies.
    pub fn saved(size: PetChatSize) -> Option<PetChatSize> { if size == Self::DEFAULT_SIZE { None } else { Some(size) } }
}

#[cfg(test)]
mod tests {
    //! The chat bubble follows the pet, flips below it at the top of the screen, stays on the pet's screen, and keeps
    //! the size the user dragged it to.
    use super::*;
    use PetChatEdge::*;

    const SCREEN: PetRect = PetRect::new(0.0, 0.0, 1440.0, 875.0);
    const PET: f64 = size::PET;
    const DEF: PetChatSize = PetChatLayout::DEFAULT_SIZE;
    const TAIL: f64 = size::CHAT_TAIL;
    const GAP: f64 = size::CHAT_GAP_TO_PET;
    const MARGIN: f64 = PetChatLayout::SCREEN_MARGIN;

    fn pet_at(x: f64, y: f64) -> PetRect { PetRect::new(x, y, PET, PET) }

    // follow

    #[test]
    fn above_pet_right_edges_lined_up_tail_on_pet() {
        let p = pet_at(1200.0, 30.0);
        let at = PetChatLayout::place(DEF, p, SCREEN);
        assert_eq!(at.x, p.max_x() - DEF.width);
        assert_eq!(at.y, p.max_y() + GAP);
        assert!(!at.tail_on_top);
        // The tail's tip is over the pet's centre.
        assert_eq!(at.x + DEF.width - at.tail_from_right, p.mid_x());
    }

    #[test]
    fn follows_a_dragged_pet() {
        let before = PetChatLayout::place(DEF, pet_at(1200.0, 30.0), SCREEN);
        let moved = pet_at(700.0, 120.0);
        let after = PetChatLayout::place(DEF, moved, SCREEN);
        assert_eq!(after.x - before.x, 700.0 - 1200.0);
        assert_eq!(after.y - before.y, 120.0 - 30.0);
        assert_eq!(after.x + DEF.width - after.tail_from_right, moved.mid_x());
    }

    #[test]
    fn flips_below_the_pet_at_the_top_of_the_screen() {
        let p = pet_at(900.0, 700.0);
        let at = PetChatLayout::place(DEF, p, SCREEN);
        assert!(at.tail_on_top);
        assert_eq!(at.y, p.min_y() - (DEF.height + TAIL) - GAP);
        assert_eq!(at.x + DEF.width - at.tail_from_right, p.mid_x());
    }

    #[test]
    fn stays_on_the_pets_screen() {
        // Pet at the left edge: the chat cannot line up with its right edge, so it is pushed onto the screen and the
        // tail slides along the bottom edge to stay over the pet...
        let left = PetChatLayout::place(DEF, pet_at(0.0, 30.0), SCREEN);
        assert_eq!(left.x, SCREEN.min_x() + MARGIN);
        assert_eq!(left.x + DEF.width - left.tail_from_right, pet_at(0.0, 30.0).mid_x());
        // ...but never past the rounded corner (pet half off the screen).
        let off = PetChatLayout::place(DEF, pet_at(-100.0, 30.0), SCREEN);
        assert_eq!(off.tail_from_right, DEF.width - radius::WINDOW - TAIL);
        // A second screen to the right (another monitor): clamped to that screen, not the first one.
        let second = PetRect::new(1440.0, -200.0, 1920.0, 1080.0);
        let there = PetChatLayout::place(DEF, PetRect::new(3300.0, -180.0, PET, PET), second);
        assert!(there.x + DEF.width <= second.max_x() - MARGIN);
        assert!(there.x >= second.min_x() + MARGIN);
        assert!(there.y >= second.min_y() + MARGIN);
    }

    #[test]
    fn resized_chat_still_follows_and_points_at_the_pet() {
        let big = PetChatSize::new(640.0, 700.0);
        let p = pet_at(1200.0, 30.0);
        let at = PetChatLayout::place(big, p, SCREEN);
        assert_eq!(at.x, p.max_x() - big.width);
        assert!(!at.tail_on_top);
        assert_eq!(at.x + big.width - at.tail_from_right, p.mid_x());
        assert_eq!(at.body_height, big.height);
        // Too tall for either side of a pet in the middle: the side with more room (below), shown as tall as that
        // room, tail up, still pointing at it, and never covering the pet.
        let mid = pet_at(1000.0, 500.0);
        let flipped = PetChatLayout::place(big, mid, SCREEN);
        assert!(flipped.tail_on_top);
        assert_eq!(flipped.x + big.width - flipped.tail_from_right, mid.mid_x());
        assert_eq!(flipped.body_height, mid.min_y() - GAP - MARGIN - TAIL);
        assert!(flipped.y + flipped.body_height + TAIL <= mid.min_y());
        assert_eq!(flipped.y, SCREEN.min_y() + MARGIN);
        // More room above (pet low): above, shortened to the room above.
        let low = pet_at(1200.0, 30.0);
        let tall = PetChatSize::new(400.0, 860.0);
        let up = PetChatLayout::place(tall, low, SCREEN);
        assert!(!up.tail_on_top);
        assert_eq!(up.y, low.max_y() + GAP);
        assert_eq!(up.y + up.body_height + TAIL, SCREEN.max_y() - MARGIN);
        // A default chat that fits keeps the old rule exactly (no shortening).
        assert_eq!(PetChatLayout::place(DEF, pet_at(900.0, 700.0), SCREEN).body_height, DEF.height);
    }

    // size

    #[test]
    fn clamp_keeps_the_body_between_minimum_and_screen() {
        assert_eq!(PetChatLayout::clamp(None, Some(SCREEN)), DEF);
        assert_eq!(PetChatLayout::clamp(PetChatSize::new(10.0, 10.0), Some(SCREEN)), PetChatLayout::MINIMUM_SIZE);
        let huge = PetChatLayout::clamp(PetChatSize::new(9999.0, 9999.0), Some(SCREEN));
        assert_eq!(huge, PetChatSize::new(SCREEN.width - 2.0 * MARGIN, SCREEN.height - 2.0 * MARGIN - TAIL));
        assert_eq!(PetChatLayout::clamp(PetChatSize::new(f64::NAN, 500.0), Some(SCREEN)), DEF);
        assert_eq!(PetChatLayout::clamp(Some(PetChatSize::new(500.4, 600.6)), None), PetChatSize::new(500.0, 601.0));
        // The default fits the minimum: the design's size is a valid size.
        const { assert!(PetChatLayout::MINIMUM_SIZE.width <= DEF.width) };
        const { assert!(PetChatLayout::MINIMUM_SIZE.height <= DEF.height) };
    }

    #[test]
    fn dragging_the_free_edges() {
        assert_eq!(PetChatLayout::resizable_edges(false), vec![Left, Top, TopLeft]);
        assert_eq!(PetChatLayout::resizable_edges(true), vec![Left, Bottom, BottomLeft]);
        let limit = PetChatSize::new(2000.0, 2000.0);
        // Left edge to the left = wider; top edge up = taller; bottom edge down = taller (y grows upward here).
        assert_eq!(PetChatLayout::resized(DEF, Left, -100.0, 50.0, limit), PetChatSize::new(460.0, 520.0));
        assert_eq!(PetChatLayout::resized(DEF, Top, -100.0, 80.0, limit), PetChatSize::new(360.0, 600.0));
        assert_eq!(PetChatLayout::resized(DEF, Bottom, 0.0, -80.0, limit), PetChatSize::new(360.0, 600.0));
        assert_eq!(PetChatLayout::resized(DEF, TopLeft, -40.0, -60.0, limit), PetChatSize::new(400.0, 460.0));
        assert_eq!(PetChatLayout::resized(DEF, BottomLeft, 500.0, 500.0, limit), PetChatLayout::MINIMUM_SIZE);
    }

    #[test]
    fn drag_stops_at_the_screen_on_the_pets_side() {
        let p = pet_at(1200.0, 30.0);
        let limit = PetChatLayout::drag_limit(p, SCREEN, false);
        assert_eq!(limit.height, SCREEN.max_y() - MARGIN - (p.max_y() + GAP) - TAIL);
        let grown = PetChatLayout::resized(DEF, TopLeft, -5000.0, 5000.0, limit);
        assert_eq!(grown.width, SCREEN.width - 2.0 * MARGIN);
        // At the limit the chat still fits above the pet: it does not flip in the middle of the drag.
        let at = PetChatLayout::place(grown, p, SCREEN);
        assert!(!at.tail_on_top);
        assert!(at.y + grown.height + TAIL <= SCREEN.max_y() - MARGIN);
        // Below the pet: the room under it.
        let high = pet_at(900.0, 700.0);
        assert_eq!(PetChatLayout::drag_limit(high, SCREEN, true).height, high.min_y() - GAP - TAIL - MARGIN);
        // Never below the minimum, even with no room.
        assert_eq!(PetChatLayout::drag_limit(pet_at(900.0, 760.0), SCREEN, false).height, PetChatLayout::MINIMUM_SIZE.height);
    }

    #[test]
    fn default_is_not_saved() {
        assert_eq!(PetChatLayout::saved(DEF), None);
        assert_eq!(PetChatLayout::saved(PetChatSize::new(500.0, 600.0)), Some(PetChatSize::new(500.0, 600.0)));
    }

    // Windows additions (not in Swift): the y-down conversion and the settings shape.

    #[test]
    fn y_down_rectangles_place_the_same_way() {
        // The same desk as SCREEN, described the Windows way (top-left origin): a pet near the bottom right.
        let screen = PetRect::from_y_down(0.0, 0.0, 1440.0, 875.0);
        let pet = PetRect::from_y_down(1200.0, 875.0 - 30.0 - PET, PET, PET);
        let at = PetChatLayout::place(DEF, pet, screen);
        assert!(!at.tail_on_top);
        // The window's bottom (top + height) is GAP above the pet's top.
        assert_eq!(at.window_top_y_down() + at.window_height(), pet.top_y_down() - GAP);
        assert_eq!(at.x, 1200.0 + PET - DEF.width);
        // A pet near the top of the screen: the chat goes below it (its top is GAP under the pet's bottom).
        let high = PetRect::from_y_down(900.0, 20.0, PET, PET);
        let below = PetChatLayout::place(DEF, high, screen);
        assert!(below.tail_on_top);
        assert_eq!(below.window_top_y_down(), 20.0 + PET + GAP);
        assert_eq!(PetRect::from_y_down(5.0, 7.0, 10.0, 20.0).top_y_down(), 7.0);
    }

    #[test]
    fn chat_size_serializes_as_width_and_height() {
        let s = PetChatSize::new(520.0, 640.0);
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"width":520.0,"height":640.0}"#);
        assert_eq!(serde_json::from_str::<PetChatSize>(r#"{"width":480,"height":600}"#).unwrap(), PetChatSize::new(480.0, 600.0));
        assert!(serde_json::from_str::<PetChatSize>(r#""big""#).is_err());
        assert_eq!(PetChatEdge::from_raw("bottomLeft"), Some(BottomLeft));
    }
}
