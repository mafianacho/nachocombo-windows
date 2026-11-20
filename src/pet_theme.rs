//! The pet's colours (`assets/pet/design-tokens.json`, the one design shared with the Mac app) as canvas colours, for
//! the light and the dark Windows theme.
use crate::core::pet::color_hex::PetColorHex;
use crate::core::pet::design_tokens::pet_tokens::{self, Palette};
use crate::gfx::Color;

#[derive(Clone, Copy)]
pub struct Pal {
    pub window: Color,
    pub surface: Color,
    pub raised: Color,
    pub separator: Color,
    pub edge: Color,
    pub text: Color,
    pub secondary: Color,
    pub tertiary: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub on_accent: Color,
    pub mine_bubble: Color,
    pub mine_text: Color,
    pub nacho_bubble: Color,
    pub nacho_text: Color,
    pub link: Color,
    pub success: Color,
    pub warning: Color,
    pub warning_soft: Color,
    pub danger: Color,
    pub on_danger: Color,
}

fn c(hex: &str) -> Color {
    PetColorHex::parse(hex).map(|v| Color(v.r as f32, v.g as f32, v.b as f32, v.a as f32)).unwrap_or(Color(1.0, 0.0, 1.0, 1.0))
}

fn from(p: &Palette) -> Pal {
    // The window is drawn opaque: Windows has no vibrancy behind it.
    let window = { let w = c(p.window); Color(w.0, w.1, w.2, 1.0) };
    Pal {
        window, surface: c(p.surface), raised: c(p.surface_raised), separator: c(p.separator), edge: c(p.edge),
        text: c(p.text), secondary: c(p.text_secondary), tertiary: c(p.text_tertiary), accent: c(p.accent), accent_soft: c(p.accent_soft),
        on_accent: c(p.on_accent), mine_bubble: c(p.mine_bubble), mine_text: c(p.mine_text), nacho_bubble: c(p.nacho_bubble),
        nacho_text: c(p.nacho_text), link: c(p.link), success: c(p.success), warning: c(p.warning), warning_soft: c(p.warning_soft),
        danger: c(p.danger), on_danger: c(p.on_danger),
    }
}

pub fn palette(dark: bool) -> Pal { if dark { from(&pet_tokens::DARK) } else { from(&pet_tokens::LIGHT) } }

pub fn current() -> Pal { palette(crate::window::is_dark()) }
