//! Where the pet's window may be on a PC with several monitors. Screens are the monitors' work areas (taskbar excluded)
//! in virtual-screen pixels, y down. Port of `PetWindow.initialOrigin` plus the keep-on-screen rule Windows needs
//! (a monitor can be unplugged or the display layout can change while the app runs).
use crate::core::layout::Rect;

/// The first place: the saved one when the pet's centre is still on a connected monitor, otherwise the bottom-right
/// corner of the primary monitor. Returns the window's top-left.
pub fn initial_origin(saved: Option<(f64, f64)>, pet: f64, screens: &[Rect], primary: Rect, margin: f64) -> (f64, f64) {
    if let Some((x, y)) = saved {
        let c = (x + pet / 2.0, y + pet / 2.0);
        if screens.iter().any(|s| s.contains(c.0, c.1)) { return (x, y); }
    }
    (primary.right() - pet - margin, primary.bottom() - pet - margin)
}

/// The monitor a point belongs to: the one that contains it, otherwise the nearest.
pub fn screen_for(point: (f64, f64), screens: &[Rect]) -> Option<Rect> {
    if let Some(s) = screens.iter().find(|s| s.contains(point.0, point.1)) { return Some(*s); }
    screens.iter().copied().min_by(|a, b| distance(point, a).total_cmp(&distance(point, b)))
}

fn distance(p: (f64, f64), r: &Rect) -> f64 {
    let dx = (r.x - p.0).max(0.0).max(p.0 - r.right());
    let dy = (r.y - p.1).max(0.0).max(p.1 - r.bottom());
    dx * dx + dy * dy
}

/// After a drag or a display change: the whole pet stays inside the work area of the monitor it is mostly on.
pub fn keep_on_screen(origin: (f64, f64), pet: f64, screens: &[Rect]) -> (f64, f64) {
    let Some(s) = screen_for((origin.0 + pet / 2.0, origin.1 + pet / 2.0), screens) else { return origin };
    let x = origin.0.min(s.right() - pet).max(s.x);
    let y = origin.1.min(s.bottom() - pet).max(s.y);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Rect = Rect::new(0.0, 0.0, 1920.0, 1040.0);
    const B: Rect = Rect::new(1920.0, 0.0, 1280.0, 1000.0);
    const LEFT_OF_A: Rect = Rect::new(-1280.0, 0.0, 1280.0, 1000.0);

    #[test]
    fn first_launch_is_the_primary_corner_and_a_saved_place_on_a_connected_monitor_is_kept() {
        assert_eq!(initial_origin(None, 112.0, &[A, B], A, 24.0), (1920.0 - 112.0 - 24.0, 1040.0 - 112.0 - 24.0));
        assert_eq!(initial_origin(Some((2500.0, 300.0)), 112.0, &[A, B], A, 24.0), (2500.0, 300.0), "on the second monitor");
    }

    #[test]
    fn a_saved_place_on_an_unplugged_monitor_goes_back_to_the_primary_corner() {
        let o = initial_origin(Some((-900.0, 100.0)), 112.0, &[A], A, 24.0);
        assert_eq!(o, (1784.0, 904.0));
        assert!(A.contains(o.0 + 56.0, o.1 + 56.0));
        // Saved on a monitor to the left that is still there.
        assert_eq!(initial_origin(Some((-900.0, 100.0)), 112.0, &[LEFT_OF_A, A], A, 24.0), (-900.0, 100.0));
    }

    #[test]
    fn the_pet_is_pulled_fully_inside_the_monitor_it_is_mostly_on() {
        assert_eq!(keep_on_screen((1900.0, 500.0), 112.0, &[A, B]), (1920.0, 500.0), "centre 1956 is on B, so B's left edge");
    }

    #[test]
    fn keep_on_screen_handles_each_edge_and_a_gap_between_monitors() {
        assert_eq!(keep_on_screen((10.0, 10.0), 112.0, &[A]), (10.0, 10.0));
        assert_eq!(keep_on_screen((-60.0, -60.0), 112.0, &[A]), (0.0, 0.0));
        assert_eq!(keep_on_screen((1900.0, 1000.0), 112.0, &[A]), (1808.0, 928.0));
        assert_eq!(keep_on_screen((5000.0, 5000.0), 112.0, &[A, B]), (3200.0 - 112.0, 1000.0 - 112.0), "far away: the nearest monitor");
        assert_eq!(keep_on_screen((5.0, 5.0), 112.0, &[]), (5.0, 5.0), "no monitors known: unchanged");
    }
}
