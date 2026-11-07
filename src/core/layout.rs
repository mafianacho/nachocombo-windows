//! Window layouts, in screen coordinates with a top-left origin (the Swift original is bottom-left).

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect { pub x: f64, pub y: f64, pub w: f64, pub h: f64 }

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self { Rect { x, y, w, h } }
    pub fn right(&self) -> f64 { self.x + self.w }
    pub fn bottom(&self) -> f64 { self.y + self.h }
    pub fn cx(&self) -> f64 { self.x + self.w / 2.0 }
    pub fn cy(&self) -> f64 { self.y + self.h / 2.0 }
    pub fn contains(&self, x: f64, y: f64) -> bool { x >= self.x && x < self.right() && y >= self.y && y < self.bottom() }
    pub fn inset(&self, dx: f64, dy: f64) -> Rect { Rect::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0)) }
    pub fn overlap_area(&self, o: &Rect) -> f64 {
        let w = self.right().min(o.right()) - self.x.max(o.x);
        let h = self.bottom().min(o.bottom()) - self.y.max(o.y);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SnapAction {
    Left, Right, CenterHalf, TopHalf, BottomHalf, TopLeft, TopRight, BottomLeft, BottomRight,
    FirstThird, CenterThird, LastThird, FirstTwoThirds, CenterTwoThirds, LastTwoThirds,
    Maximize, AlmostMaximize, MaximizeHeight, Center, Smaller, Larger, Restore, NextDisplay, PreviousDisplay,
}

use SnapAction::*;

impl SnapAction {
    pub const ALL: [SnapAction; 24] = [Left, Right, CenterHalf, TopHalf, BottomHalf, TopLeft, TopRight, BottomLeft, BottomRight,
        FirstThird, CenterThird, LastThird, FirstTwoThirds, CenterTwoThirds, LastTwoThirds,
        Maximize, AlmostMaximize, MaximizeHeight, Center, Smaller, Larger, Restore, NextDisplay, PreviousDisplay];

    pub fn id(self) -> &'static str {
        match self {
            Left => "left", Right => "right", CenterHalf => "center-half", TopHalf => "top-half", BottomHalf => "bottom-half",
            TopLeft => "top-left", TopRight => "top-right", BottomLeft => "bottom-left", BottomRight => "bottom-right",
            FirstThird => "first-third", CenterThird => "center-third", LastThird => "last-third",
            FirstTwoThirds => "first-two-thirds", CenterTwoThirds => "center-two-thirds", LastTwoThirds => "last-two-thirds",
            Maximize => "maximize", AlmostMaximize => "almost-maximize", MaximizeHeight => "maximize-height", Center => "center",
            Smaller => "smaller", Larger => "larger", Restore => "restore", NextDisplay => "next-display", PreviousDisplay => "previous-display",
        }
    }
    pub fn from_id(id: &str) -> Option<SnapAction> { Self::ALL.into_iter().find(|a| a.id() == id) }
    pub fn name(self) -> &'static str {
        match self {
            Left => "Left Half", Right => "Right Half", CenterHalf => "Center Half", TopHalf => "Top Half", BottomHalf => "Bottom Half",
            TopLeft => "Top Left", TopRight => "Top Right", BottomLeft => "Bottom Left", BottomRight => "Bottom Right",
            FirstThird => "First Third", CenterThird => "Center Third", LastThird => "Last Third",
            FirstTwoThirds => "First Two Thirds", CenterTwoThirds => "Center Two Thirds", LastTwoThirds => "Last Two Thirds",
            Maximize => "Maximize", AlmostMaximize => "Almost Maximize", MaximizeHeight => "Maximize Height", Center => "Center",
            Smaller => "Make Smaller", Larger => "Make Larger", Restore => "Restore", NextDisplay => "Next Display", PreviousDisplay => "Previous Display",
        }
    }
    pub fn cycles(self) -> bool { matches!(self, Left | Right | TopHalf | BottomHalf) }
}

pub const CYCLE: [f64; 3] = [1.0 / 2.0, 2.0 / 3.0, 1.0 / 3.0];
const RESIZE_STEP: f64 = 30.0;
const TOLERANCE: f64 = 4.0;

pub fn matches(a: &Rect, b: &Rect) -> bool {
    (a.x - b.x).abs() <= TOLERANCE && (a.y - b.y).abs() <= TOLERANCE && (a.w - b.w).abs() <= TOLERANCE && (a.h - b.h).abs() <= TOLERANCE
}

/// `current` drives center / maximize height / smaller / larger and, with `cycle`, the 1/2 → 2/3 → 1/3 sequence.
pub fn target(action: SnapAction, v: Rect, current: Option<Rect>, gap: f64, cycle: bool) -> Rect {
    if action.cycles() && cycle {
        if let Some(cur) = current {
            for (i, f) in CYCLE.iter().enumerate() {
                if matches(&cur, &apply_gap(base(action, v, *f), v, gap, true)) {
                    return apply_gap(base(action, v, CYCLE[(i + 1) % 3]), v, gap, true);
                }
            }
        }
    }
    match action {
        Center => {
            let (cw, ch) = current.map_or((v.w / 2.0, v.h / 2.0), |c| (c.w, c.h));
            let (w, h) = (cw.min(v.w), ch.min(v.h));
            Rect::new((v.cx() - w / 2.0).round(), (v.cy() - h / 2.0).round(), w, h)
        }
        MaximizeHeight => {
            let f = current.unwrap_or(v);
            apply_gap(Rect::new(f.x, v.y, f.w.min(v.w), v.h), v, gap, false)
        }
        Smaller | Larger => current.map_or(v, |f| resized(f, v, action == Larger)),
        Restore | NextDisplay | PreviousDisplay => current.unwrap_or(v),
        _ => apply_gap(base(action, v, 0.5), v, gap, true),
    }
}

fn base(a: SnapAction, v: Rect, fraction: f64) -> Rect {
    let portrait = v.h > v.w;
    // The epsilon keeps 1/6 + 2/3 from flooring to one pixel short of 5/6.
    let columns = |start: f64, len: f64| {
        if portrait {
            let (y0, y1) = ((v.h * start + 1e-6).floor(), (v.h * (start + len) + 1e-6).floor());
            Rect::new(v.x, v.y + y0, v.w, y1 - y0)
        } else {
            let (x0, x1) = ((v.w * start + 1e-6).floor(), (v.w * (start + len) + 1e-6).floor());
            Rect::new(v.x + x0, v.y, x1 - x0, v.h)
        }
    };
    let (half_w, half_h) = ((v.w / 2.0).floor(), (v.h / 2.0).floor());
    // The second part of a split is the remainder of its complement, so the two meet exactly.
    let (part_w, part_h) = ((v.w * fraction).floor(), (v.h * fraction).floor());
    let rest_w = v.w - (v.w * (1.0 - fraction)).floor();
    let rest_h = v.h - (v.h * (1.0 - fraction)).floor();
    match a {
        Left => Rect::new(v.x, v.y, part_w, v.h),
        Right => Rect::new(v.right() - rest_w, v.y, rest_w, v.h),
        TopHalf => Rect::new(v.x, v.y, v.w, part_h),
        BottomHalf => Rect::new(v.x, v.bottom() - rest_h, v.w, rest_h),
        CenterHalf => Rect::new(v.x + (v.w / 4.0).floor(), v.y, half_w, v.h),
        TopLeft => Rect::new(v.x, v.y, half_w, half_h),
        TopRight => Rect::new(v.x + half_w, v.y, v.w - half_w, half_h),
        BottomLeft => Rect::new(v.x, v.y + half_h, half_w, v.h - half_h),
        BottomRight => Rect::new(v.x + half_w, v.y + half_h, v.w - half_w, v.h - half_h),
        FirstThird => columns(0.0, 1.0 / 3.0),
        CenterThird => columns(1.0 / 3.0, 1.0 / 3.0),
        LastThird => columns(2.0 / 3.0, 1.0 / 3.0),
        FirstTwoThirds => columns(0.0, 2.0 / 3.0),
        CenterTwoThirds => columns(1.0 / 6.0, 2.0 / 3.0),
        LastTwoThirds => columns(1.0 / 3.0, 2.0 / 3.0),
        AlmostMaximize => {
            let (w, h) = ((v.w * 0.9).round(), (v.h * 0.9).round());
            Rect::new((v.cx() - w / 2.0).round(), (v.cy() - h / 2.0).round(), w, h)
        }
        _ => v,
    }
}

/// Full gap on a side at the screen edge and half on an inner side, so neighbours end up one gap apart.
pub fn apply_gap(r: Rect, v: Rect, gap: f64, horizontal: bool) -> Rect {
    if gap <= 0.0 { return r; }
    let inset = |edge: bool| if edge { gap } else { gap / 2.0 };
    let l = if horizontal { inset((r.x - v.x).abs() < 1.0) } else { 0.0 };
    let rr = if horizontal { inset((r.right() - v.right()).abs() < 1.0) } else { 0.0 };
    let t = inset((r.y - v.y).abs() < 1.0);
    let b = inset((r.bottom() - v.bottom()).abs() < 1.0);
    Rect::new(r.x + l, r.y + t, (r.w - l - rr).max(1.0), (r.h - t - b).max(1.0))
}

pub fn resized(f: Rect, v: Rect, grow: bool) -> Rect {
    let step = if grow { RESIZE_STEP } else { -RESIZE_STEP };
    let (min_w, min_h) = ((v.w / 4.0).round(), (v.h / 4.0).round());
    let mut w = v.w.min(min_w.max(f.w + 2.0 * step));
    let mut h = v.h.min(min_h.max(f.h + 2.0 * step));
    if !grow { w = w.min(f.w); h = h.min(f.h); }
    let x = (f.cx() - w / 2.0).max(v.x).min(v.right() - w);
    let y = (f.cy() - h / 2.0).max(v.y).min(v.bottom() - h);
    Rect::new(x.round(), y.round(), w, h)
}

/// Same place and proportions on another screen.
pub fn move_to(f: Rect, src: Rect, dst: Rect) -> Rect {
    if src.w <= 0.0 || src.h <= 0.0 { return f; }
    let (sx, sy) = (dst.w / src.w, dst.h / src.h);
    let (w, h) = (dst.w.min((f.w * sx).round()), dst.h.min((f.h * sy).round()));
    let x = (dst.x + ((f.x - src.x) * sx).round()).max(dst.x).min(dst.right() - w);
    let y = (dst.y + ((f.y - src.y) * sy).round()).max(dst.y).min(dst.bottom() - h);
    Rect::new(x, y, w, h)
}

/// Screen after (or before) `index`, left to right then top to bottom, wrapping around.
pub fn adjacent_screen(index: usize, screens: &[Rect], forward: bool) -> Option<usize> {
    if screens.len() < 2 || index >= screens.len() { return None; }
    let mut order: Vec<usize> = (0..screens.len()).collect();
    order.sort_by(|a, b| screens[*a].x.total_cmp(&screens[*b].x).then(screens[*a].y.total_cmp(&screens[*b].y)));
    let pos = order.iter().position(|i| *i == index)?;
    Some(order[(pos + if forward { 1 } else { order.len() - 1 }) % order.len()])
}

/// The screen sharing the most area with `w`; the nearest centre when none does.
pub fn screen_index(w: &Rect, screens: &[Rect]) -> Option<usize> {
    let best = screens.iter().enumerate().max_by(|a, b| a.1.overlap_area(w).total_cmp(&b.1.overlap_area(w)))?;
    if best.1.overlap_area(w) > 0.0 { return Some(best.0); }
    let d = |s: &Rect| (s.cx() - w.cx()).hypot(s.cy() - w.cy());
    screens.iter().enumerate().min_by(|a, b| d(a.1).total_cmp(&d(b.1))).map(|x| x.0)
}

/// Drag to snap: where a window released at (x, y) lands on a screen whose full frame is `s`. Distances scale with DPI.
pub fn zone_action(x: f64, y: f64, s: Rect, scale: f64) -> Option<SnapAction> {
    let (edge, corner, band) = (5.0 * scale, 20.0 * scale, 145.0 * scale);
    if x < s.x - 1.0 || x > s.right() + 1.0 || y < s.y - 1.0 || y > s.bottom() + 1.0 { return None; }
    let (fl, fr, ft, fb) = (x - s.x, s.right() - x, y - s.y, s.bottom() - y);
    let (al, ar, at, ab) = (fl <= edge, fr <= edge, ft <= edge, fb <= edge);
    if !(al || ar || at || ab) { return None; }
    if (al && ft <= corner) || (at && fl <= corner) { return Some(TopLeft); }
    if (ar && ft <= corner) || (at && fr <= corner) { return Some(TopRight); }
    if (al && fb <= corner) || (ab && fl <= corner) { return Some(BottomLeft); }
    if (ar && fb <= corner) || (ab && fr <= corner) { return Some(BottomRight); }
    if al || ar {
        if ft <= corner + band { return Some(TopHalf); }
        if fb <= corner + band { return Some(BottomHalf); }
        return Some(if al { Left } else { Right });
    }
    if at { return Some(Maximize); }
    if s.h > s.w { return Some(BottomHalf); }
    let third = s.w / 3.0;
    Some(if fl < third { FirstThird } else if fl < 2.0 * third { CenterThird } else { LastThird })
}

#[cfg(test)]
mod tests {
    use super::*;
    const V: Rect = Rect::new(100.0, 50.0, 1001.0, 800.0);

    #[test] fn halves_meet_without_gap_or_overlap() {
        let (l, r) = (target(Left, V, None, 0.0, false), target(Right, V, None, 0.0, false));
        assert_eq!(l.right(), r.x);
        assert_eq!(r.right(), V.right());
        assert_eq!(l.x, V.x);
    }
    #[test] fn top_half_is_on_top() {
        let t = target(TopHalf, V, None, 0.0, false);
        assert_eq!(t.y, V.y);
        assert_eq!(t.bottom(), target(BottomHalf, V, None, 0.0, false).y);
    }
    #[test] fn quarters_cover_the_screen() {
        let area: f64 = [TopLeft, TopRight, BottomLeft, BottomRight].iter().map(|a| { let r = target(*a, V, None, 0.0, false); r.w * r.h }).sum();
        assert_eq!(area, V.w * V.h);
        assert_eq!(target(TopLeft, V, None, 0.0, false).y, V.y);
        assert_eq!(target(BottomRight, V, None, 0.0, false).bottom(), V.bottom());
    }
    #[test] fn thirds_are_contiguous_and_stack_on_portrait() {
        let (a, b, c) = (target(FirstThird, V, None, 0.0, false), target(CenterThird, V, None, 0.0, false), target(LastThird, V, None, 0.0, false));
        assert_eq!(a.right(), b.x);
        assert_eq!(b.right(), c.x);
        assert!((c.right() - V.right()).abs() < 1.0);
        let p = Rect::new(0.0, 0.0, 600.0, 1200.0);
        let first = target(FirstThird, p, None, 0.0, false);
        assert_eq!((first.y, first.w, first.h), (0.0, 600.0, 400.0));
    }
    #[test] fn center_two_thirds_is_not_one_short() {
        let v = Rect::new(0.0, 0.0, 1200.0, 800.0);
        let r = target(CenterTwoThirds, v, None, 0.0, false);
        assert_eq!((r.x, r.right()), (200.0, 1000.0));
    }
    #[test] fn repeat_cycles_half_two_thirds_one_third() {
        let v = Rect::new(0.0, 0.0, 1200.0, 900.0);
        let a = target(Left, v, None, 0.0, true);
        assert_eq!(a.w, 600.0);
        let b = target(Left, v, Some(a), 0.0, true);
        assert_eq!(b.w, 800.0);
        let c = target(Left, v, Some(b), 0.0, true);
        assert_eq!(c.w, 400.0);
        assert_eq!(target(Left, v, Some(c), 0.0, true).w, 600.0);
    }
    #[test] fn cycling_works_with_gaps() {
        let v = Rect::new(0.0, 0.0, 1200.0, 900.0);
        let a = target(Right, v, None, 8.0, true);
        let b = target(Right, v, Some(a), 8.0, true);
        assert!(b.w > a.w);
    }
    #[test] fn gap_is_full_at_edges_half_between_neighbours() {
        let v = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let (l, r) = (target(Left, v, None, 8.0, false), target(Right, v, None, 8.0, false));
        assert_eq!((l.x, l.y), (8.0, 8.0));
        assert_eq!(r.x - l.right(), 8.0);
        assert_eq!(v.right() - r.right(), 8.0);
    }
    #[test] fn maximize_height_keeps_the_column() {
        let c = Rect::new(300.0, 200.0, 400.0, 100.0);
        let r = target(MaximizeHeight, V, Some(c), 0.0, false);
        assert_eq!((r.x, r.w, r.y, r.h), (300.0, 400.0, V.y, V.h));
    }
    #[test] fn smaller_stops_at_a_quarter_and_larger_stays_inside() {
        let v = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let mut f = Rect::new(100.0, 100.0, 300.0, 300.0);
        for _ in 0..20 { f = target(Smaller, v, Some(f), 0.0, false); }
        assert_eq!((f.w, f.h), (250.0, 200.0));
        for _ in 0..40 { f = target(Larger, v, Some(f), 0.0, false); }
        assert_eq!(f, v);
    }
    #[test] fn move_keeps_proportions_and_stays_inside() {
        let (a, b) = (Rect::new(0.0, 0.0, 1000.0, 1000.0), Rect::new(1000.0, -200.0, 2000.0, 500.0));
        let r = move_to(Rect::new(500.0, 500.0, 500.0, 500.0), a, b);
        assert_eq!(r, Rect::new(2000.0, 50.0, 1000.0, 250.0));
    }
    #[test] fn adjacent_screen_wraps() {
        let s = [Rect::new(1920.0, 0.0, 100.0, 100.0), Rect::new(0.0, 0.0, 100.0, 100.0), Rect::new(-500.0, 0.0, 100.0, 100.0)];
        assert_eq!(adjacent_screen(1, &s, true), Some(0));
        assert_eq!(adjacent_screen(0, &s, true), Some(2));
        assert_eq!(adjacent_screen(2, &s, false), Some(0));
        assert_eq!(adjacent_screen(0, &s[..1], true), None);
    }
    #[test] fn screen_index_uses_area_then_distance() {
        let s = [Rect::new(0.0, 0.0, 100.0, 100.0), Rect::new(100.0, 0.0, 100.0, 100.0)];
        assert_eq!(screen_index(&Rect::new(80.0, 0.0, 50.0, 50.0), &s), Some(1));
        assert_eq!(screen_index(&Rect::new(900.0, 0.0, 10.0, 10.0), &s), Some(1));
        assert_eq!(screen_index(&Rect::new(0.0, 0.0, 1.0, 1.0), &[]), None);
    }
    #[test] fn zones() {
        let s = Rect::new(0.0, 0.0, 1800.0, 1000.0);
        assert_eq!(zone_action(0.0, 0.0, s, 1.0), Some(TopLeft));
        assert_eq!(zone_action(1800.0, 1000.0, s, 1.0), Some(BottomRight));
        assert_eq!(zone_action(0.0, 500.0, s, 1.0), Some(Left));
        assert_eq!(zone_action(1800.0, 100.0, s, 1.0), Some(TopHalf));
        assert_eq!(zone_action(900.0, 0.0, s, 1.0), Some(Maximize));
        assert_eq!(zone_action(100.0, 1000.0, s, 1.0), Some(FirstThird));
        assert_eq!(zone_action(900.0, 1000.0, s, 1.0), Some(CenterThird));
        assert_eq!(zone_action(900.0, 500.0, s, 1.0), None);
    }
}
