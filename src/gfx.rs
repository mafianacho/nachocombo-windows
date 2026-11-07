//! Direct2D and DirectWrite behind a small canvas, plus the Nacho Combo design tokens.
use crate::core::layout::Rect;
use std::cell::RefCell;
use std::collections::HashMap;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

pub const fn rgb(v: u32) -> Color {
    Color(((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0, 1.0)
}

impl Color {
    pub fn alpha(self, a: f32) -> Color { Color(self.0, self.1, self.2, a) }
    pub fn mix(self, o: Color, t: f32) -> Color { Color(self.0 + (o.0 - self.0) * t, self.1 + (o.1 - self.1) * t, self.2 + (o.2 - self.2) * t, self.3 + (o.3 - self.3) * t) }
    fn d2d(self) -> D2D1_COLOR_F { D2D1_COLOR_F { r: self.0, g: self.1, b: self.2, a: self.3 } }
}

/// The design system's tokens, from `Sources/NachoCombo/Brand.swift`.
#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub paper: Color, pub paper2: Color, pub line: Color, pub ink: Color, pub ink2: Color,
    pub nacho_deep: Color, pub nacho_ink: Color, pub guac: Color, pub salsa: Color,
}
pub const NACHO: Color = rgb(0xFFC43D);
pub const ON_NACHO: Color = rgb(0x2A1A0B);
pub const BOARD: Color = rgb(0x1E1610);
pub const BOARD2: Color = rgb(0x2B2017);
pub const BOARD_LINE: Color = rgb(0x45352A);
pub const BOARD_INK: Color = rgb(0xFFF1D2);
pub const BOARD_MUTE: Color = rgb(0xBFA889);
pub const BOARD_PICK: Color = rgb(0x3A2A14);
pub const BOARD_GUAC: Color = rgb(0x7FCB72);
pub const BOARD_SALSA: Color = rgb(0xF38069);
pub const STATUS_OFF: Color = rgb(0x8E8E93);

pub const LIGHT: Theme = Theme { dark: false, paper: rgb(0xFFFFFF), paper2: rgb(0xFFF7E1), line: rgb(0xEBDDBF), ink: rgb(0x24180D), ink2: rgb(0x66553F),
    nacho_deep: rgb(0xE48A0C), nacho_ink: rgb(0x9A5A00), guac: rgb(0x377F33), salsa: rgb(0xBE3B24) };
pub const DARK: Theme = Theme { dark: true, paper: rgb(0x15100B), paper2: rgb(0x211810), line: rgb(0x3A2C1F), ink: rgb(0xF8EFDF), ink2: rgb(0xC7B59C),
    nacho_deep: rgb(0xF5A53A), nacho_ink: rgb(0xF5A53A), guac: rgb(0x7FCB72), salsa: rgb(0xF38069) };

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face { Sign, Sans, Mono, Icon }

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Font { pub face: Face, pub tenths: u32, pub weight: u16 }

pub const fn sans(size: u32) -> Font { Font { face: Face::Sans, tenths: size * 10, weight: 400 } }
pub const fn medium(size: u32) -> Font { Font { face: Face::Sans, tenths: size * 10, weight: 500 } }
pub const fn semi(size: u32) -> Font { Font { face: Face::Sans, tenths: size * 10, weight: 600 } }
pub const fn bold(size: u32) -> Font { Font { face: Face::Sans, tenths: size * 10, weight: 700 } }
pub const fn mono(size: u32) -> Font { Font { face: Face::Mono, tenths: size * 10, weight: 600 } }
pub const fn sign(size: u32) -> Font { Font { face: Face::Sign, tenths: size * 10, weight: 400 } }
pub const fn icon(size: u32) -> Font { Font { face: Face::Icon, tenths: size * 10, weight: 400 } }

#[derive(Clone, Copy, PartialEq)]
pub enum Align { Left, Center, Right }

pub struct Gfx {
    pub d2d: ID2D1Factory,
    pub dw: IDWriteFactory5,
    pub wic: IWICImagingFactory,
    fonts: Option<IDWriteFontCollection1>,
    icon_family: &'static str,
    formats: RefCell<HashMap<(Font, u8), IDWriteTextFormat>>,
}

thread_local! { static GFX: RefCell<Option<std::rc::Rc<Gfx>>> = const { RefCell::new(None) }; }

pub fn gfx() -> std::rc::Rc<Gfx> { GFX.with(|g| g.borrow().clone().expect("gfx::init was not called")) }

const FONT_FILES: [&[u8]; 7] = [
    include_bytes!("../assets/fonts/Anton-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Bold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-SemiBold.ttf"),
];

/// The bundled fonts, in memory only. Without them (older Windows) text falls back to the system font.
unsafe fn load_fonts(dw: &IDWriteFactory5) -> Result<IDWriteFontCollection1> {
    let loader = dw.CreateInMemoryFontFileLoader()?;
    dw.RegisterFontFileLoader(&loader)?;
    let builder: IDWriteFontSetBuilder1 = dw.CreateFontSetBuilder()?.cast()?;
    for bytes in FONT_FILES {
        let file = loader.CreateInMemoryFontFileReference(dw, bytes.as_ptr() as *const _, bytes.len() as u32, None)?;
        builder.AddFontFile(&file)?;
    }
    dw.CreateFontCollectionFromFontSet(&builder.CreateFontSet()?)
}

pub fn init() -> Result<()> {
    unsafe {
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dw: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let fonts = load_fonts(&dw).ok();
        // Segoe Fluent Icons ships with Windows 11; Windows 10 has the same code points in MDL2.
        let mut sys: Option<IDWriteFontCollection1> = None;
        dw.GetSystemFontCollection(false, &mut sys, false)?;
        let (mut idx, mut found) = (0u32, BOOL(0));
        if let Some(c) = &sys { let _ = c.FindFamilyName(w!("Segoe Fluent Icons"), &mut idx, &mut found); }
        let icon_family = if found.as_bool() { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" };
        let g = Gfx { d2d, dw, wic, fonts, icon_family, formats: RefCell::new(HashMap::new()) };
        GFX.with(|c| *c.borrow_mut() = Some(std::rc::Rc::new(g)));
    }
    Ok(())
}

impl Gfx {
    fn format(&self, f: Font, align: Align) -> Result<IDWriteTextFormat> {
        let key = (f, align as u8);
        if let Some(x) = self.formats.borrow().get(&key) { return Ok(x.clone()); }
        let (family, coll): (String, Option<IDWriteFontCollection>) = match f.face {
            Face::Sign => ("Anton".into(), self.fonts.as_ref().and_then(|c| c.cast().ok())),
            Face::Sans => ("IBM Plex Sans".into(), self.fonts.as_ref().and_then(|c| c.cast().ok())),
            Face::Mono => ("IBM Plex Mono".into(), self.fonts.as_ref().and_then(|c| c.cast().ok())),
            Face::Icon => (self.icon_family.into(), None),
        };
        let fam = HSTRING::from(family);
        let tf = unsafe {
            self.dw.CreateTextFormat(&fam, coll.as_ref(), DWRITE_FONT_WEIGHT(f.weight as i32), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, f.tenths as f32 / 10.0, w!("en-us"))?
        };
        unsafe {
            tf.SetTextAlignment(match align { Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING, Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER, Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING })?;
        }
        self.formats.borrow_mut().insert(key, tf.clone());
        Ok(tf)
    }

    fn layout(&self, s: &str, f: Font, align: Align, w: f32, h: f32) -> Result<IDWriteTextLayout> {
        let tf = self.format(f, align)?;
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe { self.dw.CreateTextLayout(&wide, &tf, w.max(1.0), h.max(1.0)) }
    }

    /// Width and height of `s` wrapped to `max_w`.
    pub fn measure(&self, s: &str, f: Font, max_w: f64) -> (f64, f64) {
        let s = &*tr(s, f);
        let Ok(l) = self.layout(s, f, Align::Left, max_w as f32, 10000.0) else { return (0.0, 0.0) };
        let mut m = DWRITE_TEXT_METRICS::default();
        if unsafe { l.GetMetrics(&mut m) }.is_err() { return (0.0, 0.0); }
        (m.widthIncludingTrailingWhitespace as f64, m.height as f64)
    }
}

fn tr(s: &str, f: Font) -> std::borrow::Cow<'_, str> {
    if matches!(f.face, Face::Sans | Face::Mono) { crate::i18n::t(s) } else { std::borrow::Cow::Borrowed(s) }
}

fn rf(r: Rect) -> D2D_RECT_F { D2D_RECT_F { left: r.x as f32, top: r.y as f32, right: r.right() as f32, bottom: r.bottom() as f32 } }
fn pt(x: f64, y: f64) -> windows_numerics::Vector2 { windows_numerics::Vector2 { X: x as f32, Y: y as f32 } }

pub struct Canvas {
    pub rt: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
    g: std::rc::Rc<Gfx>,
    logo: RefCell<Option<ID2D1Bitmap>>,
}

impl Canvas {
    pub fn new(rt: ID2D1RenderTarget) -> Result<Canvas> {
        let brush = unsafe { rt.CreateSolidColorBrush(&Color(0.0, 0.0, 0.0, 1.0).d2d(), None)? };
        Ok(Canvas { rt, brush, g: gfx(), logo: RefCell::new(None) })
    }
    fn b(&self, c: Color) -> &ID2D1SolidColorBrush { unsafe { self.brush.SetColor(&c.d2d()) }; &self.brush }
    pub fn clear(&self, c: Color) { unsafe { self.rt.Clear(Some(&c.d2d())) } }
    pub fn fill(&self, r: Rect, c: Color) { unsafe { self.rt.FillRectangle(&rf(r), self.b(c)) } }
    pub fn round(&self, r: Rect, rad: f64, c: Color) {
        unsafe { self.rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(r), radiusX: rad as f32, radiusY: rad as f32 }, self.b(c)) }
    }
    /// A rounded rectangle shaded from `top` to `bottom`, like the Mac app's gadget icons.
    pub fn gradient_round(&self, r: Rect, rad: f64, top: Color, bottom: Color) {
        unsafe {
            let stops = [D2D1_GRADIENT_STOP { position: 0.0, color: top.d2d() }, D2D1_GRADIENT_STOP { position: 1.0, color: bottom.d2d() }];
            let Ok(coll) = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { self.round(r, rad, top); return };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(r.x, r.y), endPoint: pt(r.x, r.bottom()) };
            let Ok(br) = self.rt.CreateLinearGradientBrush(&props, None, &coll) else { self.round(r, rad, top); return };
            self.rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(r), radiusX: rad as f32, radiusY: rad as f32 }, &br);
        }
    }
    pub fn stroke_round(&self, r: Rect, rad: f64, c: Color, w: f64) {
        unsafe { self.rt.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(r.inset(w / 2.0, w / 2.0)), radiusX: rad as f32, radiusY: rad as f32 }, self.b(c), w as f32, None) }
    }
    pub fn line(&self, x0: f64, y0: f64, x1: f64, y1: f64, c: Color, w: f64) {
        unsafe { self.rt.DrawLine(pt(x0, y0), pt(x1, y1), self.b(c), w as f32, None) }
    }
    pub fn circle(&self, cx: f64, cy: f64, r: f64, c: Color) {
        unsafe { self.rt.FillEllipse(&D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r as f32, radiusY: r as f32 }, self.b(c)) }
    }
    pub fn ring(&self, cx: f64, cy: f64, r: f64, c: Color, w: f64) {
        unsafe { self.rt.DrawEllipse(&D2D1_ELLIPSE { point: pt(cx, cy), radiusX: r as f32, radiusY: r as f32 }, self.b(c), w as f32, None) }
    }
    fn path(&self, build: impl FnOnce(&ID2D1GeometrySink)) -> Option<ID2D1PathGeometry> {
        unsafe {
            let p = self.g.d2d.CreatePathGeometry().ok()?;
            let sink = p.Open().ok()?;
            build(&sink);
            sink.Close().ok()?;
            Some(p)
        }
    }
    /// A clockwise arc from 12 o'clock; `frac` of a full turn.
    pub fn arc(&self, cx: f64, cy: f64, r: f64, frac: f64, c: Color, w: f64) {
        let frac = frac.clamp(0.0, 1.0);
        if frac <= 0.0 { return; }
        if frac >= 0.999 { self.ring(cx, cy, r, c, w); return; }
        let a = frac * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
        let geo = self.path(|s| unsafe {
            s.BeginFigure(pt(cx, cy - r), D2D1_FIGURE_BEGIN_HOLLOW);
            s.AddArc(&D2D1_ARC_SEGMENT { point: pt(cx + r * a.cos(), cy + r * a.sin()), size: D2D_SIZE_F { width: r as f32, height: r as f32 }, rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: if frac > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL } });
            s.EndFigure(D2D1_FIGURE_END_OPEN);
        });
        if let Some(g) = geo { unsafe { self.rt.DrawGeometry(&g, self.b(c), w as f32, None) } }
    }
    /// A line through `pts`; with `fill`, also the area down to `base`.
    pub fn chart(&self, pts: &[(f64, f64)], base: f64, c: Color, w: f64, fill: Option<Color>) {
        if pts.len() < 2 { return; }
        if let Some(f) = fill {
            let geo = self.path(|s| unsafe {
                s.BeginFigure(pt(pts[0].0, base), D2D1_FIGURE_BEGIN_FILLED);
                let v: Vec<_> = pts.iter().map(|p| pt(p.0, p.1)).collect();
                s.AddLines(&v);
                s.AddLine(pt(pts[pts.len() - 1].0, base));
                s.EndFigure(D2D1_FIGURE_END_CLOSED);
            });
            if let Some(g) = geo { unsafe { self.rt.FillGeometry(&g, self.b(f), None) } }
        }
        let geo = self.path(|s| unsafe {
            s.BeginFigure(pt(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_HOLLOW);
            let v: Vec<_> = pts[1..].iter().map(|p| pt(p.0, p.1)).collect();
            s.AddLines(&v);
            s.EndFigure(D2D1_FIGURE_END_OPEN);
        });
        if let Some(g) = geo { unsafe { self.rt.DrawGeometry(&g, self.b(c), w as f32, None) } }
    }
    pub fn polygon(&self, pts: &[(f64, f64)], c: Color) {
        if pts.len() < 3 { return; }
        let geo = self.path(|s| unsafe {
            s.BeginFigure(pt(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_FILLED);
            let v: Vec<_> = pts[1..].iter().map(|p| pt(p.0, p.1)).collect();
            s.AddLines(&v);
            s.EndFigure(D2D1_FIGURE_END_CLOSED);
        });
        if let Some(g) = geo { unsafe { self.rt.FillGeometry(&g, self.b(c), None) } }
    }
    pub fn text(&self, s: &str, f: Font, r: Rect, c: Color, a: Align) {
        let s = &*tr(s, f);
        let Ok(l) = self.g.layout(s, f, a, r.w as f32, r.h as f32) else { return };
        unsafe { self.rt.DrawTextLayout(pt(r.x, r.y), &l, self.b(c), D2D1_DRAW_TEXT_OPTIONS_CLIP) }
    }
    /// One line, cut with "…" when it does not fit, vertically centred in `r`.
    pub fn line_text(&self, s: &str, f: Font, r: Rect, c: Color, a: Align) {
        let s = &*tr(s, f);
        let Ok(l) = self.g.layout(s, f, a, r.w as f32, r.h as f32) else { return };
        unsafe {
            let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = l.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let tr = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            if let Ok(tf) = self.g.format(f, a) {
                if let Ok(sign) = self.g.dw.CreateEllipsisTrimmingSign(&tf) { let _ = l.SetTrimming(&tr, &sign); }
            }
            self.rt.DrawTextLayout(pt(r.x, r.y), &l, self.b(c), D2D1_DRAW_TEXT_OPTIONS_CLIP)
        }
    }
    pub fn glyph(&self, code: u32, size: u32, r: Rect, c: Color) {
        let s = char::from_u32(code).map(|c| c.to_string()).unwrap_or_default();
        self.line_text(&s, icon(size), r, c, Align::Center);
    }
    pub fn clip(&self, r: Rect) { unsafe { self.rt.PushAxisAlignedClip(&rf(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) } }
    pub fn unclip(&self) { unsafe { self.rt.PopAxisAlignedClip() } }
    pub fn bitmap(&self, b: &ID2D1Bitmap, r: Rect, opacity: f32) {
        unsafe { self.rt.DrawBitmap(b, Some(&rf(r)), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) }
    }
    pub fn measure(&self, s: &str, f: Font, max_w: f64) -> (f64, f64) { self.g.measure(s, f, max_w) }
    /// The app icon from the Mac app (`Resources/AppIcon.png`), uploaded once per render target.
    pub fn logo(&self, r: Rect) {
        if self.logo.borrow().is_none() {
            let b = logo_pixels().and_then(|(px, w, h)| self.make_bitmap(&px, w, h));
            *self.logo.borrow_mut() = b;
        }
        if let Some(b) = self.logo.borrow().as_ref() { self.bitmap(b, r, 1.0); }
    }
    /// Top-down BGRA pixels into a bitmap for this target.
    pub fn make_bitmap(&self, pixels: &[u8], w: u32, h: u32) -> Option<ID2D1Bitmap> {
        let props = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
        unsafe { self.rt.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(pixels.as_ptr() as *const _), w * 4, &props).ok() }
    }
}

const APP_PNG: &[u8] = include_bytes!("../assets/AppIcon.png");

thread_local! { static LOGO: RefCell<Option<(Vec<u8>, u32, u32)>> = const { RefCell::new(None) }; }

/// The icon scaled down once to 256 px: plenty for every place it is drawn, and a fraction of the 1024 px original.
pub fn logo_pixels() -> Option<(Vec<u8>, u32, u32)> {
    LOGO.with(|l| {
        if l.borrow().is_none() {
            let r = unsafe {
                let g = gfx();
                let stream = g.wic.CreateStream().ok()?;
                stream.InitializeFromMemory(APP_PNG).ok()?;
                let dec = g.wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
                let frame = dec.GetFrame(0).ok()?;
                let sc = g.wic.CreateBitmapScaler().ok()?;
                sc.Initialize(&frame, 256, 256, WICBitmapInterpolationModeHighQualityCubic).ok()?;
                let conv = g.wic.CreateFormatConverter().ok()?;
                conv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
                let mut px = vec![0u8; 256 * 256 * 4];
                conv.CopyPixels(std::ptr::null(), 256 * 4, &mut px).ok()?;
                Some((px, 256u32, 256u32))
            };
            *l.borrow_mut() = r;
        }
        l.borrow().clone()
    })
}

/// Decodes an image file or bytes through WIC to premultiplied top-down BGRA.
pub fn decode(source: &IWICBitmapDecoder) -> Result<(Vec<u8>, u32, u32)> {
    unsafe {
        let frame = source.GetFrame(0)?;
        let conv = gfx().wic.CreateFormatConverter()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let (mut w, mut h) = (0, 0);
        conv.GetSize(&mut w, &mut h)?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        conv.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
        Ok((px, w, h))
    }
}

pub fn decode_bytes(data: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    unsafe {
        let g = gfx();
        let stream = g.wic.CreateStream()?;
        stream.InitializeFromMemory(data)?;
        decode(&g.wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?)
    }
}

pub fn decode_file(path: &std::path::Path) -> Result<(Vec<u8>, u32, u32)> {
    unsafe {
        let name = HSTRING::from(path.as_os_str());
        decode(&gfx().wic.CreateDecoderFromFilename(&name, None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?)
    }
}

/// Top-down BGRA pixels to a PNG on disk.
pub fn save_png(path: &std::path::Path, px: &[u8], w: u32, h: u32) -> Result<()> {
    unsafe {
        let g = gfx();
        let stream = g.wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let enc = g.wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or(Error::from(E_FAIL))?;
        frame.Initialize(None)?;
        frame.SetSize(w, h)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WritePixels(h, w * 4, px)?;
        frame.Commit()?;
        enc.Commit()
    }
}

/// Draws `draw` off screen and returns top-down BGRA pixels: tray icons and `ctl` snapshots.
pub fn render_offscreen(w: u32, h: u32, dpi: f32, clear: Option<Color>, draw: impl FnOnce(&Canvas)) -> Result<Vec<u8>> {
    unsafe {
        let g = gfx();
        let bmp = g.wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES { r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT, pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: dpi, dpiY: dpi, usage: D2D1_RENDER_TARGET_USAGE_NONE, minLevel: D2D1_FEATURE_LEVEL_DEFAULT };
        let rt = g.d2d.CreateWicBitmapRenderTarget(&bmp, &props)?;
        let c = Canvas::new(rt.clone())?;
        rt.BeginDraw();
        rt.Clear(Some(&clear.unwrap_or(Color(0.0, 0.0, 0.0, 0.0)).d2d()));
        draw(&c);
        rt.EndDraw(None, None)?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        bmp.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
        Ok(px)
    }
}
