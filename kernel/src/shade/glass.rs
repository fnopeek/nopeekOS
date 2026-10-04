//! Glass parameters — computed from the wallpaper and the theme.
//!
//! Glass (loop, dock, bar) is a fill blended over the blurred wallpaper.
//! How much blur, how much fill, and where the fill must hold the text
//! legible depend on the picture: a calm dark image wants a light touch, a
//! bright busy one wants more of everything. One fixed set per theme was
//! right for one wallpaper at a time. So the wallpaper is measured once
//! when it is set (`background::compute_blur`), and every value below is
//! derived from that measurement. A key set with `set` wins over the
//! derived value; `unset` returns it to automatic.

use spin::Mutex;

use crate::gui::render::GlassInk;
use crate::shade::widgets::palette;

/// What the wallpaper looks like, measured on its 1/4-size copy.
#[derive(Clone, Copy)]
pub struct Stats {
    /// Luma percentiles of the BLURRED image (what glass sits on).
    pub p10: u32,
    pub p50: u32,
    pub p90: u32,
    /// Mean luma standard deviation inside 4x4 blocks of the SHARP image —
    /// how much fine detail would compete with text.
    pub detail: u32,
    /// Mean colour of the blurred image (0xRRGGBB).
    pub mean_rgb: u32,
}

static STATS: Mutex<Option<Stats>> = Mutex::new(None);

pub fn set_stats(s: Option<Stats>) { *STATS.lock() = s; }
pub fn stats() -> Option<Stats> { *STATS.lock() }

/// The values glass is drawn with.
#[derive(Clone, Copy)]
pub struct Params {
    /// Box radius on the 1/4-size image; 0 = no blur.
    pub blur: usize,
    /// Fill weight of the loop glass, 0..256.
    pub loop_opacity: u32,
    /// Fill weight of dock and bar, 0..255.
    pub chrome_opacity: u32,
    pub ink: GlassInk,
}

/// Contrast the glass must keep to the theme's text colour, ×10
/// (60 = 6:1 — between WCAG AA 4.5 and AAA 7 for body text).
const CONTRAST_X10: u64 = 60;
/// Wallpaper colour mixed into the fill (×256): ~10 % ties the glass to
/// the picture; text keeps its neutral colour.
const TINT_WEIGHT: u32 = 26;
/// How far past the contrast bound the median of the picture is taken:
/// the bound is the least legible glass, the margin makes it comfortable.
const MARGIN: u32 = 20;
/// Fill weight bounds (×256), the same for both themes — light and dark
/// glass are meant to feel alike and differ in colour, not in weight.
const FILL_MIN: u32 = 160;
const FILL_MAX: u32 = 208;

/// Measurement used before any wallpaper is set (flat grey background).
const NO_WALLPAPER: Stats = Stats { p10: 24, p50: 24, p90: 24, detail: 0, mean_rgb: 0x181820 };

/// Blur radius for a measured detail level: the busier the picture, the
/// more it has to be calmed before text can sit on it.
pub fn auto_blur(detail: u32) -> usize {
    // Florian's set measures 5-7 here and still carries fine grain that
    // fights text, so the scale starts at 2 (~10 px), not 1.
    (2 + detail / 4).clamp(2, 5) as usize
}

pub fn blur_radius(detail: u32) -> usize {
    key("shade.blur").map(|v| v.min(8) as usize).unwrap_or_else(|| auto_blur(detail))
}

pub fn params() -> Params {
    let light = palette::is_light_theme();
    let st = stats().unwrap_or(NO_WALLPAPER);
    let text = palette::resolve(crate::shade::widgets::abi::Token::OnSurface);
    let lt = lin(luma(text));

    // Contrast bound, from the text colour.
    let (floor, ceil) = match bounds(light, lt) {
        (f, _) if light => (key("shade.light_floor").map(|v| v.min(255)).unwrap_or(f), 255),
        (_, c) => (0, key("shade.dark_ceil").map(|v| v.min(255)).unwrap_or(c)),
    };

    // Fill weight, ONE value for both themes — light and dark glass should
    // feel alike. Each theme needs just enough fill to take the MEDIAN of
    // the picture a margin past its contrast bound (the bound then holds the
    // remaining dark or bright patches per pixel); both use the larger need.
    let need = fill_need(true, st.p50).max(fill_need(false, st.p50));
    let loop_auto = need.clamp(FILL_MIN, FILL_MAX);
    let loop_opacity = key("shade.opacity").map(|v| v.min(256)).unwrap_or(loop_auto);
    let chrome_auto = (loop_opacity * 255 / 256 + 30).min(255);
    let chrome_opacity = key("shade.chrome_opacity").map(|v| v.min(255)).unwrap_or(chrome_auto);

    let tint_w = key("shade.glass_tint").map(|pct| pct.min(40) * 256 / 100).unwrap_or(TINT_WEIGHT);

    Params {
        blur: blur_radius(st.detail),
        loop_opacity,
        chrome_opacity,
        ink: GlassInk { floor, ceil, tint: st.mean_rgb, tint_w },
    }
}

pub fn ink() -> GlassInk { params().ink }

/// Fill weight (×256) that takes the picture's median `m` a margin past the
/// contrast bound of one theme: m + a·(fill − m) = target.
fn fill_need(light: bool, m: u32) -> u32 {
    let (text, surface) = palette::theme_text_and_surface(light);
    let (floor, ceil) = bounds(light, lin(luma(text)));
    let target = if light { (floor + MARGIN).min(250) } else { ceil.saturating_sub(MARGIN) };
    if (light && m >= target) || (!light && m <= target) { return 0; }
    target.abs_diff(m) * 256 / luma(surface).abs_diff(m).max(1)
}

/// Least (light) / most (dark) luma glass may have under text of linear
/// luminance `lt` for the contrast ratio. Gamma ~2: L = v² / 255².
fn bounds(light: bool, lt: u64) -> (u32, u32) {
    if light {
        let need = (CONTRAST_X10 * (lt + 3251) / 10).saturating_sub(3251);
        (isqrt(need.min(65025)) as u32, 255)
    } else {
        let max = ((lt + 3251) * 10 / CONTRAST_X10).saturating_sub(3251);
        (0, isqrt(max.min(65025)) as u32)
    }
}

/// `shade glass`: what was measured and what it led to.
pub fn report() {
    use crate::kprintln;
    let p = params();
    match stats() {
        Some(s) => kprintln!("  wallpaper  luma p10 {} · p50 {} · p90 {} · detail {} · colour #{:06x}",
            s.p10, s.p50, s.p90, s.detail, s.mean_rgb),
        None => kprintln!("  wallpaper  none (flat background)"),
    }
    let src = |k: &str| if key(k).is_some() { "set" } else { "auto" };
    kprintln!("  theme      {}", if palette::is_light_theme() { "light" } else { "dark" });
    kprintln!("  blur       {:>3}  ({})    shade.blur", p.blur, src("shade.blur"));
    kprintln!("  loop       {:>3}  ({})    shade.opacity        0-256", p.loop_opacity, src("shade.opacity"));
    kprintln!("  dock/bar   {:>3}  ({})    shade.chrome_opacity 0-255", p.chrome_opacity, src("shade.chrome_opacity"));
    if palette::is_light_theme() {
        kprintln!("  floor      {:>3}  ({})    shade.light_floor    least brightness under text",
            p.ink.floor, src("shade.light_floor"));
    } else {
        kprintln!("  ceiling    {:>3}  ({})    shade.dark_ceil      most brightness under text",
            p.ink.ceil, src("shade.dark_ceil"));
    }
    kprintln!("  tint       {:>3}% ({})    shade.glass_tint     wallpaper colour in the glass",
        p.ink.tint_w * 100 / 256, src("shade.glass_tint"));
    kprintln!("  `unset <key>` returns a value to auto.");
}

fn key(k: &str) -> Option<u32> {
    crate::config::get(k).and_then(|s| s.trim().parse::<u32>().ok())
}

fn luma(c: u32) -> u32 {
    (((c >> 16) & 0xFF) * 299 + ((c >> 8) & 0xFF) * 587 + (c & 0xFF) * 114) / 1000
}

/// Linear light ×65025 with a gamma of 2 — close enough to sRGB for a bound.
fn lin(v: u32) -> u64 { (v as u64) * (v as u64) }

fn isqrt(v: u64) -> u64 {
    if v < 2 { return v; }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x { x = y; y = (x + v / x) / 2; }
    x
}
