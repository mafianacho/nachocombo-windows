//! Nacho Pet motion as a pure function of time, from the original pictures only (no new art): crossfade with a small
//! scale pop between states, breathing while idle, random blinks, a different small loop while listening, thinking and
//! answering, a hop when done, dozing off when tired, press, drag tilt and landing. Twin of the design mockup's
//! `motion.js` (same rules, same tokens); the test replays the web model's poses from `fixtures/pet-motion-vectors.json`.
//! Times in milliseconds on any monotonic clock. Port of `PetMotion.swift`.
use super::design_tokens::pet_tokens::{curve, motion as m, motion_amount as a};
use super::face::PetFace;
use std::f64::consts::PI;
use std::sync::LazyLock;

/// The pet's pose at a moment: which picture shows (and the one fading out), and how the character is moved.
/// dx/dy in points (+dy = up; the shell flips it for a y-down canvas), scale around the bottom centre, rotation in
/// degrees (+ = clockwise).
#[derive(Clone, Debug, PartialEq)]
pub struct PetPose {
    pub asset: String,
    pub previous: Option<String>,
    /// Opacity of `asset` over `previous` (1 when there is no crossfade).
    pub mix: f64,
    pub dx: f64,
    pub dy: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotation: f64,
}

impl PetPose {
    /// A still pose of `asset` (no crossfade, no movement).
    pub fn new(asset: &str, previous: Option<&str>) -> Self {
        Self { asset: asset.into(), previous: previous.map(Into::into), mix: 1.0, dx: 0.0, dy: 0.0, scale_x: 1.0, scale_y: 1.0, rotation: 0.0 }
    }
}

static STANDARD: LazyLock<PetCurve> = LazyLock::new(|| PetCurve::new(curve::STANDARD));
static EMPHASIZED: LazyLock<PetCurve> = LazyLock::new(|| PetCurve::new(curve::EMPHASIZED));

#[derive(Clone, Debug, PartialEq)]
pub struct PetMotion {
    face: PetFace,
    since: f64,
    pub reduce_motion: bool,
    from: Option<String>,
    seed: u32,
    pressed_at: Option<f64>,
    released_at: Option<f64>,
    press_depth: f64,
    tilt_from: f64,
    tilt_to: f64,
    tilt_since: f64,
    landed_at: Option<f64>,
}

impl Default for PetMotion {
    fn default() -> Self { Self::new(PetFace::Idle, 0.0, false, 1) }
}

impl PetMotion {
    /// Swift defaults: `face: .idle, at: 0, reduceMotion: false, seed: 1`.
    pub fn new(face: PetFace, t: f64, reduce_motion: bool, seed: u32) -> Self {
        Self {
            face, since: t, reduce_motion, from: None, seed, pressed_at: None, released_at: None, press_depth: 0.0,
            tilt_from: 0.0, tilt_to: 0.0, tilt_since: t, landed_at: None,
        }
    }

    pub fn face(&self) -> PetFace { self.face }
    pub fn since(&self) -> f64 { self.since }

    /// Pictures with an eyes-shut twin of the same pose, for blinks.
    pub fn blink_asset(face: PetFace) -> Option<&'static str> {
        match face {
            PetFace::Idle => Some("eyes-closed"),
            PetFace::Listening => Some("calling-eyeless"),
            _ => None,
        }
    }

    pub fn set_face(&mut self, next: PetFace, t: f64) {
        if next == self.face { return; }
        self.from = Some(self.asset(t));
        self.face = next;
        self.since = t;
    }

    pub fn press(&mut self, t: f64) {
        self.pressed_at = Some(t);
        self.released_at = None;
    }

    pub fn release(&mut self, t: f64) {
        self.press_depth = self.press_amount(t);
        self.pressed_at = None;
        self.released_at = Some(t);
    }

    /// While dragging: horizontal speed in points per second leans the character into the move.
    pub fn drag(&mut self, velocity_x: f64, t: f64) {
        self.tilt_from = self.tilt(t);
        self.tilt_to = (velocity_x * a::DRAG_TILT_PER_SPEED).clamp(-a::DRAG_TILT_MAX, a::DRAG_TILT_MAX);
        self.tilt_since = t;
    }

    /// Let go after a drag: straightens up and lands with a small squash.
    pub fn drop(&mut self, t: f64) {
        self.tilt_from = self.tilt(t);
        self.tilt_to = 0.0;
        self.tilt_since = t;
        self.landed_at = Some(t);
    }

    /// The picture without the crossfade: blinks and dozing swap to an eyes-shut twin.
    pub fn asset(&self, t: f64) -> String {
        let base = self.face.asset_name();
        if self.reduce_motion { return base.into(); }
        let age = t - self.since;
        if let Some(shut) = Self::blink_asset(self.face) {
            let mut b = self.since;
            let mut n: u32 = 0;
            loop {
                b += m::BLINK_MIN + Self::random(self.seed, n) * (m::BLINK_MAX - m::BLINK_MIN);
                if b > t { break; }
                if t < b + m::BLINK { return shut.into(); }
                n += 1;
            }
        }
        if self.face == PetFace::Tired && age % m::DROWSE_PERIOD >= m::DROWSE_PERIOD - m::DROWSE_CLOSED { return "eyes-closed".into(); }
        base.into()
    }

    /// Frames are needed while anything moves; with reduced motion only during a crossfade.
    pub fn is_animating(&self, t: f64) -> bool { !self.reduce_motion || (self.from.is_some() && t - self.since < m::TRANSITION) }

    pub fn pose(&self, t: f64) -> PetPose {
        let age = t - self.since;
        let mut p = PetPose::new(&self.asset(t), None);
        if let Some(f) = self.from.as_ref().filter(|_| age < m::TRANSITION) {
            let k = Self::clamp01(age / m::TRANSITION);
            p.previous = Some(f.clone());
            p.mix = STANDARD.y(k);
            if !self.reduce_motion {
                let pop = a::TRANSITION_SCALE * (PI * k).sin();
                p.scale_x += pop;
                p.scale_y += pop;
            }
        }
        if self.reduce_motion { return Self::rounded(p); }
        let breath = |p: &mut PetPose, slow: f64| {
            let w = Self::wave(age / (m::PET_BREATH * slow));
            p.dy += a::BREATH_LIFT * w;
            p.scale_y *= 1.0 + a::BREATH_SCALE * w;
            p.scale_x *= 1.0 - a::BREATH_SCALE * 0.4 * w;
        };
        match self.face {
            PetFace::Idle | PetFace::Waiting => breath(&mut p, 1.0),
            PetFace::Listening => {
                p.dy += a::LISTEN_LIFT * Self::wave(age / m::LISTEN_PERIOD);
                p.rotation += a::LISTEN_TILT * (PI * age / m::LISTEN_PERIOD).sin();
            }
            PetFace::Thinking => {
                p.rotation += a::THINK_TILT * (2.0 * PI * age / m::THINK_PERIOD).sin();
                p.dy += a::THINK_LIFT * Self::wave(age / m::THINK_PERIOD);
            }
            PetFace::Speaking => {
                let w = Self::wave(age / m::SPEAK_PERIOD);
                p.dy += a::SPEAK_LIFT * w;
                p.scale_y *= 1.0 + a::SPEAK_SCALE * w;
                p.scale_x *= 1.0 - a::SPEAK_SCALE * 0.5 * w;
            }
            PetFace::Happy => {
                if age < m::HAPPY_HOP {
                    let k = age / m::HAPPY_HOP;
                    p.dy += a::HAPPY_HOP_HEIGHT * 4.0 * k * (1.0 - k);
                } else if age < m::HAPPY_HOP + m::LAND {
                    let d = 1.0 - STANDARD.y((age - m::HAPPY_HOP) / m::LAND);
                    p.scale_y *= 1.0 - a::LAND_SQUASH * d;
                    p.scale_x *= 1.0 + a::LAND_SQUASH * d;
                } else {
                    breath(&mut p, 1.0);
                }
            }
            PetFace::Tired => {
                breath(&mut p, a::TIRED_BREATH_SLOW);
                let (c, start) = (age % m::DROWSE_PERIOD, m::DROWSE_PERIOD - m::DROWSE_CLOSED);
                if c >= start {
                    let nod = (PI * (c - start) / m::DROWSE_CLOSED).sin();
                    p.dy -= 1.5 * nod;
                    p.rotation += 2.0 * nod;
                }
            }
            PetFace::Sleeping => breath(&mut p, a::SLEEP_BREATH_SLOW),
        }
        let press = self.press_amount(t);
        if press != 0.0 {
            let s = 1.0 - (1.0 - a::PRESS_SCALE) * press;
            p.scale_x *= s;
            p.scale_y *= s;
        }
        p.rotation += self.tilt(t);
        if let Some(l) = self.landed_at.filter(|l| t - l < m::LAND) {
            let d = 1.0 - STANDARD.y((t - l) / m::LAND);
            p.scale_y *= 1.0 - a::LAND_SQUASH * d;
            p.scale_x *= 1.0 + a::LAND_SQUASH * d;
        }
        Self::rounded(p)
    }

    fn tilt(&self, t: f64) -> f64 {
        let k = 1.0 - (-(t - self.tilt_since) / 80.0).exp();
        self.tilt_from + (self.tilt_to - self.tilt_from) * Self::clamp01(k)
    }

    fn press_amount(&self, t: f64) -> f64 {
        if let Some(p) = self.pressed_at { return STANDARD.y(Self::clamp01((t - p) / m::PRESS)); }
        if let Some(r) = self.released_at { return self.press_depth * (1.0 - EMPHASIZED.y(Self::clamp01((t - r) / (m::PRESS * 2.0)))); }
        0.0
    }

    // helpers shared with motion.js

    /// lowbias32 hash → [0, 1); same numbers as motion.js `rand`.
    pub fn random(seed: u32, n: u32) -> f64 {
        let mut x = seed ^ n.wrapping_add(1).wrapping_mul(0x9E37_79B9);
        x ^= x >> 16;
        x = x.wrapping_mul(0x7FEB_352D);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846C_A68B);
        x ^= x >> 16;
        x as f64 / 4_294_967_296.0
    }

    fn clamp01(v: f64) -> f64 { v.clamp(0.0, 1.0) }
    fn wave(phase: f64) -> f64 { 0.5 - 0.5 * (2.0 * PI * phase).cos() }

    fn rounded(mut p: PetPose) -> PetPose {
        let r = |v: f64| {
            let x = (v * 1e4).round() / 1e4;
            if x == 0.0 { 0.0 } else { x }
        };
        p.mix = r(p.mix);
        p.dx = r(p.dx);
        p.dy = r(p.dy);
        p.scale_x = r(p.scale_x);
        p.scale_y = r(p.scale_y);
        p.rotation = r(p.rotation);
        p
    }
}

/// When the pet may draw frames: only while it is on screen, the displays are awake, no full-screen app is in front,
/// and something moves (or the user is touching it). Otherwise the frame timer stops: no CPU while idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PetAnimationGate {
    pub window_visible: bool,
    pub displays_asleep: bool,
    pub full_screen_app_in_front: bool,
    pub interacting: bool,
}

impl Default for PetAnimationGate {
    fn default() -> Self { Self { window_visible: true, displays_asleep: false, full_screen_app_in_front: false, interacting: false } }
}

impl PetAnimationGate {
    pub fn new() -> Self { Self::default() }
    pub fn allows_motion(&self) -> bool { self.window_visible && !self.displays_asleep && !self.full_screen_app_in_front }
    pub fn needs_frames(&self, motion: &PetMotion, t: f64) -> bool { self.allows_motion() && (motion.is_animating(t) || self.interacting) }
}

/// A CSS `cubic-bezier(x1, y1, x2, y2)` easing curve (the tokens' `curve` group), solved the same way as motion.js.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PetCurve {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl PetCurve {
    /// Panics when `spec` does not hold four numbers (the specs are design-token constants; Swift `precondition`).
    pub fn new(spec: &str) -> Self { Self::parse(spec).unwrap_or_else(|| panic!("cubic-bezier needs four numbers: {spec}")) }

    /// `None` unless `spec` has four comma separated values inside `( … )`; a value that is not a number reads as 0.
    pub fn parse(spec: &str) -> Option<Self> {
        let inner = spec.split_once('(')?.1;
        let inner = inner.split(')').next().unwrap_or("");
        let v: Vec<f64> = inner.split(',').map(|s| s.trim().parse().unwrap_or(0.0)).collect();
        if v.len() != 4 { return None; }
        Some(Self { x1: v[0], y1: v[1], x2: v[2], y2: v[3] })
    }

    pub fn y(&self, x: f64) -> f64 {
        if x <= 0.0 { return 0.0; }
        if x >= 1.0 { return 1.0; }
        let bx = |u: f64| 3.0 * self.x1 * u * (1.0 - u) * (1.0 - u) + 3.0 * self.x2 * u * u * (1.0 - u) + u * u * u;
        let by = |u: f64| 3.0 * self.y1 * u * (1.0 - u) * (1.0 - u) + 3.0 * self.y2 * u * u * (1.0 - u) + u * u * u;
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if bx(mid) < x { lo = mid } else { hi = mid }
        }
        by((lo + hi) / 2.0)
    }
}

#[cfg(test)]
mod tests {
    //! PetMotion must move the pet exactly like the design mockup's motion.js: the fixture holds the web model's poses
    //! for the same timelines.
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeSet;

    const FIXTURE: &str = include_str!("fixtures/pet-motion-vectors.json");

    fn fixture() -> serde_json::Map<String, Value> {
        match serde_json::from_str(FIXTURE).expect("fixture") { Value::Object(o) => o, _ => panic!("not an object") }
    }

    #[test]
    fn swift_poses_match_the_web_model() {
        let all = fixture();
        assert_eq!(all.keys().map(String::as_str).collect::<BTreeSet<_>>(), BTreeSet::from(["states", "touch", "reduced"]));
        let mut compared = 0;
        for (name, tl) in &all {
            let events = tl["events"].as_array().expect("events");
            let samples = tl["samples"].as_array().expect("samples");
            let first = PetFace::from_raw(events[0][2].as_str().unwrap_or("")).expect("first face");
            let mut mo = PetMotion::new(first, 0.0, tl["reduceMotion"].as_bool().unwrap_or(false), tl["seed"].as_u64().unwrap_or(1) as u32);
            let mut next = 1;
            for s in samples {
                let t = s["t"].as_f64().expect("t");
                while next < events.len() {
                    let ev = events[next].as_array().expect("event");
                    let Some(at) = ev[0].as_f64().filter(|at| *at <= t) else { break };
                    match ev[1].as_str() {
                        Some("face") => mo.set_face(PetFace::from_raw(ev[2].as_str().unwrap_or("")).expect("face"), at),
                        Some("press") => mo.press(at),
                        Some("release") => mo.release(at),
                        Some("drag") => mo.drag(ev[2].as_f64().expect("velocity"), at),
                        Some("drop") => mo.drop(at),
                        op => panic!("unknown event {op:?}"),
                    }
                    next += 1;
                }
                let p = mo.pose(t);
                let label = format!("{name} @{}ms", t as i64);
                assert_eq!(Some(p.asset.as_str()), s["asset"].as_str(), "{label}");
                assert_eq!(p.previous.as_deref(), s["previous"].as_str(), "{label}");
                for (key, value) in [("mix", p.mix), ("dx", p.dx), ("dy", p.dy), ("scaleX", p.scale_x), ("scaleY", p.scale_y), ("rotation", p.rotation)] {
                    let want = s[key].as_f64().unwrap_or_else(|| panic!("{label} {key}"));
                    assert!((value - want).abs() <= 0.001, "{label} {key}: {value} != {want}");
                }
                assert_eq!(Some(mo.is_animating(t)), s["animating"].as_bool(), "{label}");
                compared += 1;
            }
        }
        assert!(compared > 500);
    }

    #[test]
    fn the_preview_shows_every_move() {
        let all = fixture();
        let samples = all["states"]["samples"].as_array().expect("samples");
        let assets: BTreeSet<&str> = samples.iter().filter_map(|s| s["asset"].as_str()).collect();
        // 9 of the 10 originals: body.webp has no face, so it is not a pose.
        assert_eq!(assets, BTreeSet::from(["idle", "eyes-closed", "calling", "calling-eyeless", "thinking", "working", "happy", "tired"]));
        assert!(samples.iter().any(|s| s["previous"].is_string()), "crossfades");
        assert!(samples.iter().any(|s| s["dy"].as_f64().unwrap_or(0.0) > 8.0), "happy hop");
        assert!(samples.iter().any(|s| s["rotation"].as_f64().unwrap_or(0.0).abs() > 4.0), "drag tilt");
    }

    #[test]
    fn reduced_motion_only_crossfades_and_then_stops() {
        let mut mo = PetMotion::new(PetFace::Idle, 0.0, true, 5);
        assert!(!mo.is_animating(10_000.0), "no breathing loop: no frames, no CPU");
        assert_eq!(mo.pose(10_000.0), PetPose::new("idle", None));
        mo.set_face(PetFace::Thinking, 20_000.0);
        let mid = mo.pose(20_000.0 + m::TRANSITION / 2.0);
        assert_eq!(mid.previous.as_deref(), Some("idle"));
        assert_eq!(mid.scale_x, 1.0);
        assert!(mo.is_animating(20_100.0));
        assert!(!mo.is_animating(20_000.0 + m::TRANSITION));
        mo.set_face(PetFace::Tired, 30_000.0);
        assert_eq!(mo.asset(30_000.0 + m::DROWSE_PERIOD - 10.0), "tired", "no dozing swap either");
    }

    #[test]
    fn blinks_are_short_and_only_where_an_eyes_shut_twin_exists() {
        let mo = PetMotion::new(PetFace::Idle, 0.0, false, 5);
        let (mut shut, mut run, mut longest) = (0, 0, 0);
        for i in 0..=6000 {
            if mo.asset(i as f64 * 10.0) == "eyes-closed" {
                shut += 1;
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        assert!(shut > 0);
        assert!((longest * 10) as f64 <= m::BLINK + 10.0);
        for face in PetFace::ALL.into_iter().filter(|f| PetMotion::blink_asset(*f).is_none() && *f != PetFace::Tired) {
            let f = PetMotion::new(face, 0.0, false, 5);
            assert!((0..=600).all(|i| f.asset(i as f64 * 50.0) == face.asset_name()), "{face:?}");
        }
    }

    #[test]
    fn frames_stop_when_hidden_asleep_or_behind_a_full_screen_app() {
        let mo = PetMotion::new(PetFace::Idle, 0.0, false, 1);
        let mut gate = PetAnimationGate::new();
        assert!(gate.needs_frames(&mo, 1000.0), "breathing while idle on screen");
        let changes: [fn(&mut PetAnimationGate); 3] =
            [|g| g.window_visible = false, |g| g.displays_asleep = true, |g| g.full_screen_app_in_front = true];
        for change in changes {
            let mut g = PetAnimationGate::new();
            change(&mut g);
            assert!(!g.needs_frames(&mo, 1000.0));
            g.interacting = true;
            assert!(!g.needs_frames(&mo, 1000.0), "not even while touched");
        }
        let still = PetMotion::new(PetFace::Idle, 0.0, true, 1);
        assert!(!gate.needs_frames(&still, 1000.0), "reduced motion: idle costs no frames");
        gate.interacting = true;
        assert!(gate.needs_frames(&still, 1000.0));
    }

    #[test]
    fn curves_follow_css() {
        let linear = PetCurve::new("cubic-bezier(0, 0, 1, 1)");
        assert!((linear.y(0.3) - 0.3).abs() < 1e-6);
        let standard = PetCurve::new(curve::STANDARD);
        assert_eq!(standard.y(0.0), 0.0);
        assert_eq!(standard.y(1.0), 1.0);
        assert!(standard.y(0.5) > 0.5, "eases out");
        let emphasized = PetCurve::new(curve::EMPHASIZED);
        assert!((0..=100).map(|i| emphasized.y(i as f64 / 100.0)).fold(0.0, f64::max) > 1.0, "overshoots");
        assert_eq!(PetCurve::parse("ease-in"), None);
    }
}
