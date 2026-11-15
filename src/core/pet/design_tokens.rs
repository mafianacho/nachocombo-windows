//! Nacho Pet design tokens: the values of `assets/pet/design-tokens.json` (a copy of the macOS app's `design/tokens.json`,
//! the single source of the design) as Rust constants. Port of the generated `PetDesignTokens.swift`; the test below
//! checks every constant against the JSON, so edit the JSON and this file together. Points (= CSS px at 1x; on Windows
//! device-independent pixels, multiply by the monitor's DPI scale), milliseconds, colors as "#RRGGBB" / "#RRGGBBAA".
//!
//! Layout: Swift's nested `PetTokens.Size.chatWidth` is not valid Rust, so the namespace is the module
//! [`pet_tokens`] with one submodule per group and SCREAMING_CASE constants: `pet_tokens::size::CHAT_WIDTH`,
//! `pet_tokens::motion::BLINK`, `pet_tokens::LIGHT.accent`. `font::FAMILY` is the macOS CSS font list (kept so the
//! values match the design); the Windows shell picks its own DirectWrite family (Segoe UI / Malgun Gothic).

pub mod pet_tokens {
    use std::collections::BTreeMap;

    /// Sizes in points.
    pub mod size {
        pub const PET: f64 = 112.0;
        pub const PET_ART_INSET: f64 = 6.0;
        pub const PET_SCREEN_MARGIN: f64 = 24.0;
        pub const PET_HOVER_LIFT: f64 = 3.0;
        pub const PET_NAME_TAG_HEIGHT: f64 = 22.0;
        pub const CHAT_WIDTH: f64 = 360.0;
        pub const CHAT_HEIGHT: f64 = 520.0;
        pub const CHAT_GAP_TO_PET: f64 = 4.0;
        pub const CHAT_TAIL: f64 = 12.0;
        pub const HEADER_HEIGHT: f64 = 44.0;
        pub const AVATAR: f64 = 26.0;
        pub const STATUS_DOT: f64 = 7.0;
        pub const CONTROL: f64 = 28.0;
        pub const COMPOSER_HEIGHT: f64 = 36.0;
        pub const SEND_BUTTON: f64 = 28.0;
        pub const ICON: f64 = 16.0;
        pub const BUBBLE_MAX_WIDTH: f64 = 272.0;
        pub const CHIP_HEIGHT: f64 = 24.0;
        pub const THUMBNAIL: f64 = 40.0;
        pub const LINE_ICON: f64 = 13.0;
        pub const SEND_ICON: f64 = 14.0;
        pub const SIGNIN_ART: f64 = 96.0;
        pub const CHIP_MAX_WIDTH: f64 = 170.0;
        pub const BADGE: f64 = 20.0;
        pub const BADGE_ICON: f64 = 12.0;
        pub const SELECT_BAR_HEIGHT: f64 = 34.0;
        pub const SELECT_BAR_ICON: f64 = 14.0;
        pub const SELECT_BAR_OFFSET: f64 = 8.0;
        pub const SELECT_ASK_WIDTH: f64 = 400.0;
        pub const CONTEXT_ROW_ICON: f64 = 14.0;
    }

    /// Spacing in points.
    pub mod space {
        pub const XXS: f64 = 2.0;
        pub const XS: f64 = 4.0;
        pub const S: f64 = 8.0;
        pub const M: f64 = 12.0;
        pub const L: f64 = 16.0;
        pub const XL: f64 = 24.0;
    }

    /// Corner radii in points.
    pub mod radius {
        pub const WINDOW: f64 = 14.0;
        pub const BUBBLE: f64 = 14.0;
        pub const BUBBLE_TAIL: f64 = 4.0;
        pub const CARD: f64 = 10.0;
        pub const CONTROL: f64 = 8.0;
        pub const CHIP: f64 = 12.0;
        pub const THUMBNAIL: f64 = 6.0;
        pub const SELECT_BAR: f64 = 10.0;
    }

    /// Font family (CSS list) and sizes in points, weights, line height.
    pub mod font {
        pub const FAMILY: &str = "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Apple SD Gothic Neo\", \"Noto Sans CJK KR\", \"Noto Sans KR\", sans-serif";
        pub const TITLE: f64 = 15.0;
        pub const BODY: f64 = 13.0;
        pub const CAPTION: f64 = 11.0;
        pub const WEIGHT_REGULAR: f64 = 400.0;
        pub const WEIGHT_MEDIUM: f64 = 500.0;
        pub const WEIGHT_SEMIBOLD: f64 = 600.0;
        pub const LINE_HEIGHT: f64 = 1.4;
    }

    /// Shadows: offsets and blur in points, opacity 0…1.
    pub mod shadow {
        pub const WINDOW_Y: f64 = 10.0;
        pub const WINDOW_BLUR: f64 = 30.0;
        pub const PET_Y: f64 = 4.0;
        pub const PET_BLUR: f64 = 10.0;
        pub const PET_OPACITY: f64 = 0.22;
    }

    /// Opacities 0…1.
    pub mod opacity {
        pub const PAST: f64 = 0.78;
        pub const GONE: f64 = 0.7;
        pub const SEPARATOR_TEXT: f64 = 0.5;
    }

    /// Durations in milliseconds.
    pub mod motion {
        pub const FAST: f64 = 120.0;
        pub const BASE: f64 = 200.0;
        pub const SLOW: f64 = 320.0;
        pub const PET_BREATH: f64 = 2400.0;
        pub const CARET_BLINK: f64 = 900.0;
        pub const THINKING_DOT: f64 = 1200.0;
        pub const HAPPY_HOLD: f64 = 4000.0;
        pub const TIRED_AFTER: f64 = 600000.0;
        pub const SLEEP_AFTER: f64 = 1800000.0;
        pub const TRANSITION: f64 = 200.0;
        pub const BLINK_MIN: f64 = 2000.0;
        pub const BLINK_MAX: f64 = 5200.0;
        pub const BLINK: f64 = 130.0;
        pub const LISTEN_PERIOD: f64 = 900.0;
        pub const THINK_PERIOD: f64 = 1800.0;
        pub const SPEAK_PERIOD: f64 = 520.0;
        pub const HAPPY_HOP: f64 = 420.0;
        pub const DROWSE_PERIOD: f64 = 4200.0;
        pub const DROWSE_CLOSED: f64 = 1400.0;
        pub const PRESS: f64 = 110.0;
        pub const LAND: f64 = 280.0;
        pub const FRAME_INTERVAL: f64 = 33.0;
        pub const SELECT_BAR_DELAY: f64 = 300.0;
        pub const SELECT_BAR_FADE: f64 = 120.0;
    }

    /// Motion amounts: points, degrees or scale factors.
    pub mod motion_amount {
        pub const TRANSITION_SCALE: f64 = 0.04;
        pub const BREATH_LIFT: f64 = 1.5;
        pub const BREATH_SCALE: f64 = 0.02;
        pub const LISTEN_LIFT: f64 = 2.0;
        pub const LISTEN_TILT: f64 = 2.0;
        pub const THINK_TILT: f64 = 3.0;
        pub const THINK_LIFT: f64 = 0.8;
        pub const SPEAK_LIFT: f64 = 1.2;
        pub const SPEAK_SCALE: f64 = 0.012;
        pub const HAPPY_HOP_HEIGHT: f64 = 9.0;
        pub const TIRED_BREATH_SLOW: f64 = 1.6;
        pub const SLEEP_BREATH_SLOW: f64 = 2.2;
        pub const PRESS_SCALE: f64 = 0.94;
        pub const DRAG_TILT_PER_SPEED: f64 = 0.012;
        pub const DRAG_TILT_MAX: f64 = 8.0;
        pub const LAND_SQUASH: f64 = 0.08;
    }

    /// CSS `cubic-bezier(…)` easing curves (see `motion::PetCurve`).
    pub mod curve {
        pub const STANDARD: &str = "cubic-bezier(0.2, 0, 0, 1)";
        pub const EMPHASIZED: &str = "cubic-bezier(0.3, 0, 0, 1.3)";
        pub const EXIT: &str = "cubic-bezier(0.4, 0, 1, 1)";
    }

    /// One theme's colors.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Palette {
        pub desktop: &'static str,
        pub window: &'static str,
        pub surface: &'static str,
        pub surface_raised: &'static str,
        pub separator: &'static str,
        pub edge: &'static str,
        pub text: &'static str,
        pub text_secondary: &'static str,
        pub text_tertiary: &'static str,
        pub accent: &'static str,
        pub accent_soft: &'static str,
        pub on_accent: &'static str,
        pub mine_bubble: &'static str,
        pub mine_text: &'static str,
        pub nacho_bubble: &'static str,
        pub nacho_text: &'static str,
        pub link: &'static str,
        pub success: &'static str,
        pub warning: &'static str,
        pub warning_soft: &'static str,
        pub danger: &'static str,
        pub danger_soft: &'static str,
        pub on_danger: &'static str,
        pub info: &'static str,
        pub focus_ring: &'static str,
        pub tooltip: &'static str,
        pub on_tooltip: &'static str,
        pub shadow: &'static str,
        pub knob: &'static str,
    }

    pub const LIGHT: Palette = Palette {
        desktop: "#C9CED6",
        window: "#FBFAF8F2",
        surface: "#F1EEEA",
        surface_raised: "#FFFFFF",
        separator: "#1F1A151F",
        edge: "#1F1A1529",
        text: "#1F1B17",
        text_secondary: "#5F5750",
        text_tertiary: "#6F665E",
        accent: "#C2530A",
        accent_soft: "#FCE9DB",
        on_accent: "#FFFFFF",
        mine_bubble: "#FCE3D0",
        mine_text: "#4E2205",
        nacho_bubble: "#F1EEEA",
        nacho_text: "#1F1B17",
        link: "#A9480A",
        success: "#2F8A4C",
        warning: "#A15C00",
        warning_soft: "#FFF1D6",
        danger: "#B8342B",
        danger_soft: "#FBE5E2",
        on_danger: "#FFFFFF",
        info: "#2C6FBF",
        focus_ring: "#C2530A40",
        tooltip: "#2A2521F0",
        on_tooltip: "#FFFFFF",
        shadow: "#00000030",
        knob: "#FFFFFF",
    };

    pub const DARK: Palette = Palette {
        desktop: "#2B2F36",
        window: "#252220F2",
        surface: "#322E2B",
        surface_raised: "#3B3632",
        separator: "#FFFFFF1A",
        edge: "#FFFFFF2E",
        text: "#F4EFE9",
        text_secondary: "#C2B8AE",
        text_tertiary: "#ABA196",
        accent: "#FF8A3D",
        accent_soft: "#4A2E1C",
        on_accent: "#1F1206",
        mine_bubble: "#4A2E1C",
        mine_text: "#FFE2CC",
        nacho_bubble: "#322E2B",
        nacho_text: "#F4EFE9",
        link: "#FFAD72",
        success: "#63C483",
        warning: "#F2B45A",
        warning_soft: "#3D3020",
        danger: "#FF7A6E",
        danger_soft: "#45231F",
        on_danger: "#2A0D0A",
        info: "#7FB1F2",
        focus_ring: "#FF8A3D4D",
        tooltip: "#F4EFE9F0",
        on_tooltip: "#1F1B17",
        shadow: "#00000073",
        knob: "#FFFFFF",
    };

    /// Every token as "group.key" (colors "color.<theme>.key") → its value as written in the JSON, built from the
    /// constants above (so a hand edit of a constant shows up as a mismatch in the test).
    pub fn flat() -> BTreeMap<String, String> {
        [
        ("size.pet", text(size::PET)),
        ("size.petArtInset", text(size::PET_ART_INSET)),
        ("size.petScreenMargin", text(size::PET_SCREEN_MARGIN)),
        ("size.petHoverLift", text(size::PET_HOVER_LIFT)),
        ("size.petNameTagHeight", text(size::PET_NAME_TAG_HEIGHT)),
        ("size.chatWidth", text(size::CHAT_WIDTH)),
        ("size.chatHeight", text(size::CHAT_HEIGHT)),
        ("size.chatGapToPet", text(size::CHAT_GAP_TO_PET)),
        ("size.chatTail", text(size::CHAT_TAIL)),
        ("size.headerHeight", text(size::HEADER_HEIGHT)),
        ("size.avatar", text(size::AVATAR)),
        ("size.statusDot", text(size::STATUS_DOT)),
        ("size.control", text(size::CONTROL)),
        ("size.composerHeight", text(size::COMPOSER_HEIGHT)),
        ("size.sendButton", text(size::SEND_BUTTON)),
        ("size.icon", text(size::ICON)),
        ("size.bubbleMaxWidth", text(size::BUBBLE_MAX_WIDTH)),
        ("size.chipHeight", text(size::CHIP_HEIGHT)),
        ("size.thumbnail", text(size::THUMBNAIL)),
        ("size.lineIcon", text(size::LINE_ICON)),
        ("size.sendIcon", text(size::SEND_ICON)),
        ("size.signinArt", text(size::SIGNIN_ART)),
        ("size.chipMaxWidth", text(size::CHIP_MAX_WIDTH)),
        ("size.badge", text(size::BADGE)),
        ("size.badgeIcon", text(size::BADGE_ICON)),
        ("size.selectBarHeight", text(size::SELECT_BAR_HEIGHT)),
        ("size.selectBarIcon", text(size::SELECT_BAR_ICON)),
        ("size.selectBarOffset", text(size::SELECT_BAR_OFFSET)),
        ("size.selectAskWidth", text(size::SELECT_ASK_WIDTH)),
        ("size.contextRowIcon", text(size::CONTEXT_ROW_ICON)),
        ("space.xxs", text(space::XXS)),
        ("space.xs", text(space::XS)),
        ("space.s", text(space::S)),
        ("space.m", text(space::M)),
        ("space.l", text(space::L)),
        ("space.xl", text(space::XL)),
        ("radius.window", text(radius::WINDOW)),
        ("radius.bubble", text(radius::BUBBLE)),
        ("radius.bubbleTail", text(radius::BUBBLE_TAIL)),
        ("radius.card", text(radius::CARD)),
        ("radius.control", text(radius::CONTROL)),
        ("radius.chip", text(radius::CHIP)),
        ("radius.thumbnail", text(radius::THUMBNAIL)),
        ("radius.selectBar", text(radius::SELECT_BAR)),
        ("font.family", font::FAMILY.to_string()),
        ("font.title", text(font::TITLE)),
        ("font.body", text(font::BODY)),
        ("font.caption", text(font::CAPTION)),
        ("font.weightRegular", text(font::WEIGHT_REGULAR)),
        ("font.weightMedium", text(font::WEIGHT_MEDIUM)),
        ("font.weightSemibold", text(font::WEIGHT_SEMIBOLD)),
        ("font.lineHeight", text(font::LINE_HEIGHT)),
        ("shadow.windowY", text(shadow::WINDOW_Y)),
        ("shadow.windowBlur", text(shadow::WINDOW_BLUR)),
        ("shadow.petY", text(shadow::PET_Y)),
        ("shadow.petBlur", text(shadow::PET_BLUR)),
        ("shadow.petOpacity", text(shadow::PET_OPACITY)),
        ("opacity.past", text(opacity::PAST)),
        ("opacity.gone", text(opacity::GONE)),
        ("opacity.separatorText", text(opacity::SEPARATOR_TEXT)),
        ("motion.fast", text(motion::FAST)),
        ("motion.base", text(motion::BASE)),
        ("motion.slow", text(motion::SLOW)),
        ("motion.petBreath", text(motion::PET_BREATH)),
        ("motion.caretBlink", text(motion::CARET_BLINK)),
        ("motion.thinkingDot", text(motion::THINKING_DOT)),
        ("motion.happyHold", text(motion::HAPPY_HOLD)),
        ("motion.tiredAfter", text(motion::TIRED_AFTER)),
        ("motion.sleepAfter", text(motion::SLEEP_AFTER)),
        ("motion.transition", text(motion::TRANSITION)),
        ("motion.blinkMin", text(motion::BLINK_MIN)),
        ("motion.blinkMax", text(motion::BLINK_MAX)),
        ("motion.blink", text(motion::BLINK)),
        ("motion.listenPeriod", text(motion::LISTEN_PERIOD)),
        ("motion.thinkPeriod", text(motion::THINK_PERIOD)),
        ("motion.speakPeriod", text(motion::SPEAK_PERIOD)),
        ("motion.happyHop", text(motion::HAPPY_HOP)),
        ("motion.drowsePeriod", text(motion::DROWSE_PERIOD)),
        ("motion.drowseClosed", text(motion::DROWSE_CLOSED)),
        ("motion.press", text(motion::PRESS)),
        ("motion.land", text(motion::LAND)),
        ("motion.frameInterval", text(motion::FRAME_INTERVAL)),
        ("motion.selectBarDelay", text(motion::SELECT_BAR_DELAY)),
        ("motion.selectBarFade", text(motion::SELECT_BAR_FADE)),
        ("motionAmount.transitionScale", text(motion_amount::TRANSITION_SCALE)),
        ("motionAmount.breathLift", text(motion_amount::BREATH_LIFT)),
        ("motionAmount.breathScale", text(motion_amount::BREATH_SCALE)),
        ("motionAmount.listenLift", text(motion_amount::LISTEN_LIFT)),
        ("motionAmount.listenTilt", text(motion_amount::LISTEN_TILT)),
        ("motionAmount.thinkTilt", text(motion_amount::THINK_TILT)),
        ("motionAmount.thinkLift", text(motion_amount::THINK_LIFT)),
        ("motionAmount.speakLift", text(motion_amount::SPEAK_LIFT)),
        ("motionAmount.speakScale", text(motion_amount::SPEAK_SCALE)),
        ("motionAmount.happyHopHeight", text(motion_amount::HAPPY_HOP_HEIGHT)),
        ("motionAmount.tiredBreathSlow", text(motion_amount::TIRED_BREATH_SLOW)),
        ("motionAmount.sleepBreathSlow", text(motion_amount::SLEEP_BREATH_SLOW)),
        ("motionAmount.pressScale", text(motion_amount::PRESS_SCALE)),
        ("motionAmount.dragTiltPerSpeed", text(motion_amount::DRAG_TILT_PER_SPEED)),
        ("motionAmount.dragTiltMax", text(motion_amount::DRAG_TILT_MAX)),
        ("motionAmount.landSquash", text(motion_amount::LAND_SQUASH)),
        ("curve.standard", curve::STANDARD.to_string()),
        ("curve.emphasized", curve::EMPHASIZED.to_string()),
        ("curve.exit", curve::EXIT.to_string()),
        ("color.light.desktop", LIGHT.desktop.to_string()),
        ("color.light.window", LIGHT.window.to_string()),
        ("color.light.surface", LIGHT.surface.to_string()),
        ("color.light.surfaceRaised", LIGHT.surface_raised.to_string()),
        ("color.light.separator", LIGHT.separator.to_string()),
        ("color.light.edge", LIGHT.edge.to_string()),
        ("color.light.text", LIGHT.text.to_string()),
        ("color.light.textSecondary", LIGHT.text_secondary.to_string()),
        ("color.light.textTertiary", LIGHT.text_tertiary.to_string()),
        ("color.light.accent", LIGHT.accent.to_string()),
        ("color.light.accentSoft", LIGHT.accent_soft.to_string()),
        ("color.light.onAccent", LIGHT.on_accent.to_string()),
        ("color.light.mineBubble", LIGHT.mine_bubble.to_string()),
        ("color.light.mineText", LIGHT.mine_text.to_string()),
        ("color.light.nachoBubble", LIGHT.nacho_bubble.to_string()),
        ("color.light.nachoText", LIGHT.nacho_text.to_string()),
        ("color.light.link", LIGHT.link.to_string()),
        ("color.light.success", LIGHT.success.to_string()),
        ("color.light.warning", LIGHT.warning.to_string()),
        ("color.light.warningSoft", LIGHT.warning_soft.to_string()),
        ("color.light.danger", LIGHT.danger.to_string()),
        ("color.light.dangerSoft", LIGHT.danger_soft.to_string()),
        ("color.light.onDanger", LIGHT.on_danger.to_string()),
        ("color.light.info", LIGHT.info.to_string()),
        ("color.light.focusRing", LIGHT.focus_ring.to_string()),
        ("color.light.tooltip", LIGHT.tooltip.to_string()),
        ("color.light.onTooltip", LIGHT.on_tooltip.to_string()),
        ("color.light.shadow", LIGHT.shadow.to_string()),
        ("color.light.knob", LIGHT.knob.to_string()),
        ("color.dark.desktop", DARK.desktop.to_string()),
        ("color.dark.window", DARK.window.to_string()),
        ("color.dark.surface", DARK.surface.to_string()),
        ("color.dark.surfaceRaised", DARK.surface_raised.to_string()),
        ("color.dark.separator", DARK.separator.to_string()),
        ("color.dark.edge", DARK.edge.to_string()),
        ("color.dark.text", DARK.text.to_string()),
        ("color.dark.textSecondary", DARK.text_secondary.to_string()),
        ("color.dark.textTertiary", DARK.text_tertiary.to_string()),
        ("color.dark.accent", DARK.accent.to_string()),
        ("color.dark.accentSoft", DARK.accent_soft.to_string()),
        ("color.dark.onAccent", DARK.on_accent.to_string()),
        ("color.dark.mineBubble", DARK.mine_bubble.to_string()),
        ("color.dark.mineText", DARK.mine_text.to_string()),
        ("color.dark.nachoBubble", DARK.nacho_bubble.to_string()),
        ("color.dark.nachoText", DARK.nacho_text.to_string()),
        ("color.dark.link", DARK.link.to_string()),
        ("color.dark.success", DARK.success.to_string()),
        ("color.dark.warning", DARK.warning.to_string()),
        ("color.dark.warningSoft", DARK.warning_soft.to_string()),
        ("color.dark.danger", DARK.danger.to_string()),
        ("color.dark.dangerSoft", DARK.danger_soft.to_string()),
        ("color.dark.onDanger", DARK.on_danger.to_string()),
        ("color.dark.info", DARK.info.to_string()),
        ("color.dark.focusRing", DARK.focus_ring.to_string()),
        ("color.dark.tooltip", DARK.tooltip.to_string()),
        ("color.dark.onTooltip", DARK.on_tooltip.to_string()),
        ("color.dark.shadow", DARK.shadow.to_string()),
        ("color.dark.knob", DARK.knob.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    /// 112 → "112", 1.4 → "1.4" (the way the JSON writes numbers).
    pub fn text(v: f64) -> String {
        if v.round() == v && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
    }
}

#[cfg(test)]
mod tests {
    use super::super::color_hex::PetColorHex;
    use super::pet_tokens::{self, Palette, DARK, LIGHT};
    use serde_json::Value;
    use std::collections::BTreeMap;

    const JSON: &str = include_str!("../../../assets/pet/design-tokens.json");

    /// The JSON flattened the same way as [`pet_tokens::flat`].
    fn flat_json() -> BTreeMap<String, String> {
        let Value::Object(o) = serde_json::from_str::<Value>(JSON).expect("tokens json") else { panic!("not an object") };
        let mut out = BTreeMap::new();
        for (group, value) in o.iter().filter(|(g, _)| !g.starts_with('$')) {
            let g = value.as_object().unwrap_or_else(|| panic!("{group}"));
            for (k, v) in g {
                if group == "color" {
                    for (ck, cv) in v.as_object().unwrap_or_else(|| panic!("{k}")) {
                        out.insert(format!("color.{k}.{ck}"), cv.as_str().unwrap_or_else(|| panic!("{k}.{ck}")).to_string());
                    }
                } else if let Some(s) = v.as_str() {
                    out.insert(format!("{group}.{k}"), s.to_string());
                } else {
                    out.insert(format!("{group}.{k}"), pet_tokens::text(v.as_f64().unwrap_or_else(|| panic!("{group}.{k}"))));
                }
            }
        }
        out
    }

    #[test]
    fn swift_constants_equal_tokens_json() {
        let json = flat_json();
        let flat = pet_tokens::flat();
        assert_eq!(json.len(), flat.len());
        for (k, v) in &json { assert_eq!(flat.get(k), Some(v), "{k}"); }
        assert_eq!(pet_tokens::size::PET, 112.0);
        assert_eq!(Some(&LIGHT.accent.to_string()), json.get("color.light.accent"));
        assert_eq!(Some(&DARK.window.to_string()), json.get("color.dark.window"));
        assert_eq!(pet_tokens::text(1.4), "1.4");
        assert_eq!(pet_tokens::text(600000.0), "600000");
    }

    #[test]
    fn colors_are_valid_hex_and_text_is_readable() {
        for (key, value) in pet_tokens::flat().iter().filter(|(k, _)| k.starts_with("color.")) {
            assert!(PetColorHex::parse(value).is_some(), "{key}={value}");
        }
        // WCAG 2.x contrast for text on the surfaces it sits on (4.5:1 normal text).
        let themes: [(&str, Palette); 2] = [("light", LIGHT), ("dark", DARK)];
        for (name, p) in themes {
            let pairs = [
                ("text/surface", p.text, p.surface), ("textSecondary/surface", p.text_secondary, p.surface),
                ("textTertiary/surfaceRaised", p.text_tertiary, p.surface_raised), ("mineText/mineBubble", p.mine_text, p.mine_bubble),
                ("nachoText/nachoBubble", p.nacho_text, p.nacho_bubble), ("link/surface", p.link, p.surface),
                ("onAccent/accent", p.on_accent, p.accent), ("onTooltip/tooltip", p.on_tooltip, p.tooltip),
                ("warning/warningSoft", p.warning, p.warning_soft), ("danger/dangerSoft", p.danger, p.danger_soft),
                ("onDanger/danger", p.on_danger, p.danger),
            ];
            for (label, fg, bg) in pairs {
                let r = PetColorHex::contrast(fg, bg, p.window);
                assert!(r >= 4.5, "{name} {label} = {r:.2}");
            }
        }
    }
}
