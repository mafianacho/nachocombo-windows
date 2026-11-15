//! "#RRGGBB" / "#RRGGBBAA" design-token colors (`assets/pet/design-tokens.json`) as sRGB components, plus WCAG
//! contrast. Port of `PetColorHex.swift`.

/// sRGB components 0…1, straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RGBA {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

pub struct PetColorHex;

impl PetColorHex {
    pub fn parse(hex: &str) -> Option<RGBA> {
        let digits = hex.strip_prefix('#')?;
        let n = hex.chars().count();
        if n != 7 && n != 9 { return None; }
        let v = u64::from_str_radix(digits, 16).ok()?;
        let full = if n == 7 { (v << 8) | 0xFF } else { v };
        let c = |shift: u32| ((full >> shift) & 0xFF) as f64 / 255.0;
        Some(RGBA { r: c(24), g: c(16), b: c(8), a: c(0) })
    }

    /// `top` over `bottom` (straight alpha); the result is opaque when `bottom` is.
    pub fn over(top: RGBA, bottom: RGBA) -> RGBA {
        let a = top.a + bottom.a * (1.0 - top.a);
        if a <= 0.0 { return RGBA { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }; }
        let c = |t: f64, b: f64| (t * top.a + b * bottom.a * (1.0 - top.a)) / a;
        RGBA { r: c(top.r, bottom.r), g: c(top.g, bottom.g), b: c(top.b, bottom.b), a }
    }

    pub fn luminance(c: RGBA) -> f64 {
        let lin = |v: f64| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    /// Contrast of `fg` drawn on `bg`, where `bg` itself lies on `backdrop` (made opaque). 1…21; 0 for a bad color.
    pub fn contrast(fg: &str, bg: &str, backdrop: &str) -> f64 {
        let (Some(mut base), Some(b), Some(f)) = (Self::parse(backdrop), Self::parse(bg), Self::parse(fg)) else { return 0.0 };
        base.a = 1.0;
        let back = Self::over(b, base);
        let front = Self::over(f, back);
        let (l1, l2) = (Self::luminance(front), Self::luminance(back));
        (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Not a Swift test file of its own (PetDesignTokenTests covers it); the edges the token test does not reach.
    #[test]
    fn parse_and_contrast_edges() {
        assert_eq!(PetColorHex::parse("#FF000080"), Some(RGBA { r: 1.0, g: 0.0, b: 0.0, a: 128.0 / 255.0 }));
        assert_eq!(PetColorHex::parse("#00FF00").map(|c| c.a), Some(1.0));
        for bad in ["FF0000", "#FF00", "#GG0000", "#FF0000F", ""] { assert_eq!(PetColorHex::parse(bad), None, "{bad}"); }
        assert!((PetColorHex::contrast("#000000", "#FFFFFF", "#FFFFFF") - 21.0).abs() < 1e-9);
        assert_eq!(PetColorHex::contrast("#000000", "nope", "#FFFFFF"), 0.0);
    }
}
