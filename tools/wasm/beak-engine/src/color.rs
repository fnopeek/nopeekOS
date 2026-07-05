//! color.rs — CSS `<color>` parsing.
//!
//! Owns ALL colour syntax: hex (#rgb/#rgba/#rrggbb/#rrggbbaa), the functional
//! forms rgb()/rgba()/hsl()/hsla() (comma- and space/slash-separated, int and
//! %), and the full CSS Color Module Level 4 named-colour set. Host-testable,
//! no OS in the loop.
//!
//! Alpha is parsed but DROPPED: there is no compositing context yet, so the
//! 4/8-digit hex forms and the `a` channel of rgba()/hsla()/slash-alpha are
//! accepted (never a parse failure) and the opaque RGB is returned.
//!
//! `no_std`-safe: only `core`/`alloc`. No libm — the float helpers avoid
//! `round`/`floor`/`abs`/`rem_euclid` (std-only) and use casts + `%` + a
//! hand-rolled `fabs`.

use crate::layout::Rgb;
use alloc::vec::Vec;

/// Parse a CSS `<color>` to an opaque `Rgb`. `None` means "keep the inherited
/// value" — the caller relies on this for `currentcolor`/`inherit`/`transparent`
/// and for unparseable input (forward-compatible, like a browser).
pub fn parse_color(v: &str) -> Option<Rgb> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    if let Some(hex) = v.strip_prefix('#') {
        return parse_hex(hex);
    }
    let lower = v.to_ascii_lowercase();
    let l = lower.as_str();
    // Functional notation. `rgba(` never matches `rgb(` because the `(` is part
    // of the probe, so declaration order here is irrelevant.
    if let Some(inner) = fn_body(l, "rgba") {
        return parse_rgb(inner);
    }
    if let Some(inner) = fn_body(l, "rgb") {
        return parse_rgb(inner);
    }
    if let Some(inner) = fn_body(l, "hsla") {
        return parse_hsl(inner);
    }
    if let Some(inner) = fn_body(l, "hsl") {
        return parse_hsl(inner);
    }
    // CSS Color 4 functional forms.
    if let Some(inner) = fn_body(l, "hwb") {
        return parse_hwb(inner);
    }
    if let Some(inner) = fn_body(l, "oklch") {
        return parse_lch(inner, true);
    }
    if let Some(inner) = fn_body(l, "oklab") {
        return parse_lab(inner, true);
    }
    if let Some(inner) = fn_body(l, "lch") {
        return parse_lch(inner, false);
    }
    if let Some(inner) = fn_body(l, "lab") {
        return parse_lab(inner, false);
    }
    if let Some(inner) = fn_body(l, "color") {
        return parse_color_fn(inner);
    }
    // Named colours. `transparent`/`currentcolor`/`inherit` are absent from the
    // table → fall through to `None` (= keep inherited), as the contract wants.
    named_color(l)
}

// ── hex ──────────────────────────────────────────────────────────────────

fn parse_hex(hex: &str) -> Option<Rgb> {
    let b = hex.as_bytes();
    match hex.len() {
        // #rgb — each nibble ×17 (0x0..0xF → 0x00..0xFF).
        3 => Some(Rgb(
            hex_digit(b[0])? * 17,
            hex_digit(b[1])? * 17,
            hex_digit(b[2])? * 17,
        )),
        // #rgba — parse the alpha nibble (validate), then drop it.
        4 => {
            let r = hex_digit(b[0])? * 17;
            let g = hex_digit(b[1])? * 17;
            let bl = hex_digit(b[2])? * 17;
            hex_digit(b[3])?; // alpha, dropped
            Some(Rgb(r, g, bl))
        }
        // #rrggbb
        6 => Some(Rgb(hex2(b, 0)?, hex2(b, 2)?, hex2(b, 4)?)),
        // #rrggbbaa — parse the alpha byte (validate), then drop it.
        8 => {
            let r = hex2(b, 0)?;
            let g = hex2(b, 2)?;
            let bl = hex2(b, 4)?;
            hex2(b, 6)?; // alpha, dropped
            Some(Rgb(r, g, bl))
        }
        _ => None,
    }
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex2(b: &[u8], i: usize) -> Option<u8> {
    Some(hex_digit(b[i])? * 16 + hex_digit(b[i + 1])?)
}

// ── functional notation helpers ──────────────────────────────────────────

/// Strip `name(` … `)`, returning the inner argument string, or `None` if `s`
/// is not that function call.
fn fn_body<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    s.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')
}

/// Split channel tokens on commas and/or ASCII whitespace, dropping empties.
fn tokens(s: &str) -> Vec<&str> {
    s.split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|t| !t.is_empty())
        .collect()
}

/// `rgb()`/`rgba()`. Comma- OR space-separated, modern `r g b / a` slash-alpha,
/// channels as int (0–255) or percentage (0–100%). Alpha parsed then dropped.
fn parse_rgb(inner: &str) -> Option<Rgb> {
    let (channels, has_slash_alpha) = match inner.split_once('/') {
        Some((c, _a)) => (c, true), // `_a` = alpha, dropped
        None => (inner, false),
    };
    let t = tokens(channels);
    // 3 channels; a 4th is the legacy comma-form alpha (only when no slash).
    let ok = if has_slash_alpha {
        t.len() == 3
    } else {
        t.len() == 3 || t.len() == 4
    };
    if !ok {
        return None;
    }
    Some(Rgb(channel_255(t[0])?, channel_255(t[1])?, channel_255(t[2])?))
}

/// `hsl()`/`hsla()`. Same separator rules; hue in degrees, s/l as percentages.
/// Alpha parsed then dropped.
fn parse_hsl(inner: &str) -> Option<Rgb> {
    let (channels, has_slash_alpha) = match inner.split_once('/') {
        Some((c, _a)) => (c, true),
        None => (inner, false),
    };
    let t = tokens(channels);
    let ok = if has_slash_alpha {
        t.len() == 3
    } else {
        t.len() == 3 || t.len() == 4
    };
    if !ok {
        return None;
    }
    let h = parse_hue(t[0])?;
    let s = unit_pct(t[1])?;
    let l = unit_pct(t[2])?;
    Some(hsl_to_rgb(h, s, l))
}

/// An rgb() channel: `0..255` int/float, or `0%..100%` → `0..255`. Clamped.
fn channel_255(tok: &str) -> Option<u8> {
    let tok = tok.trim();
    if let Some(p) = tok.strip_suffix('%') {
        let pct: f32 = p.trim().parse().ok()?;
        Some(round_u8(clamp(pct, 0.0, 100.0) / 100.0 * 255.0))
    } else {
        let n: f32 = tok.parse().ok()?;
        Some(round_u8(n))
    }
}

/// A hue token: bare number or `<n>deg`, wrapped into `[0, 360)`.
fn parse_hue(tok: &str) -> Option<f32> {
    let tok = tok.trim();
    let tok = tok.strip_suffix("deg").unwrap_or(tok);
    let h: f32 = tok.trim().parse().ok()?;
    let h = h % 360.0;
    Some(if h < 0.0 { h + 360.0 } else { h })
}

/// A required percentage (`s`/`l`) → unit interval `[0, 1]`.
fn unit_pct(tok: &str) -> Option<f32> {
    let tok = tok.trim();
    let p = tok.strip_suffix('%')?;
    let v: f32 = p.trim().parse().ok()?;
    Some(clamp(v, 0.0, 100.0) / 100.0)
}

// ── float helpers (no libm) ──────────────────────────────────────────────

fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    x.max(lo).min(hi)
}

fn fabs(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

/// Round a non-negative-ish float to a `u8`, clamping to `0..=255`.
fn round_u8(f: f32) -> u8 {
    // Clamp first, then round-half-up; the saturating f32→u8 cast handles the
    // 255.5 edge (truncates to 255). No `round()` (std-only) needed.
    (clamp(f, 0.0, 255.0) + 0.5) as u8
}

/// HSL → RGB (CSS Color 4 algorithm). `h` in `[0,360)`, `s`/`l` in `[0,1]`.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgb {
    let c = (1.0 - fabs(2.0 * l - 1.0)) * s;
    let hp = h / 60.0; // in [0, 6)
    let x = c * (1.0 - fabs((hp % 2.0) - 1.0));
    let m = l - c / 2.0;
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x), // 5 (and clamp for any float edge)
    };
    Rgb(
        round_u8((r1 + m) * 255.0),
        round_u8((g1 + m) * 255.0),
        round_u8((b1 + m) * 255.0),
    )
}

// ── CSS Color 4: hwb / lab / lch / oklab / oklch / color() ─────────────────
//
// Each functional colour is converted to linear sRGB, gamma-encoded, and
// simple-clipped into gamut. f32 throughout — the oracle's ±20/255 tolerance
// is far looser than f32 rounding. Matrices/constants are the CSS Color 4
// "Sample code for color conversions" values (drafts.csswg.org/css-color-4).

/// Split off an optional `/ alpha` tail (dropped — no compositing yet).
fn split_slash(inner: &str) -> &str {
    match inner.split_once('/') {
        Some((c, _a)) => c,
        None => inner,
    }
}

/// `hwb(H W B)` — hue + whiteness + blackness. W/B as `%` or fraction.
fn parse_hwb(inner: &str) -> Option<Rgb> {
    let t = tokens(split_slash(inner));
    if t.len() != 3 {
        return None;
    }
    let h = if t[0] == "none" { 0.0 } else { parse_hue(t[0])? };
    let w = frac(t[1])?;
    let bk = frac(t[2])?;
    if w + bk >= 1.0 {
        let g = round_u8(w / (w + bk) * 255.0);
        return Some(Rgb(g, g, g));
    }
    // Tint/shade a pure-hue colour: c·(1−W−B) + W.
    let base = hsl_to_rgb(h, 1.0, 0.5);
    let mix = |c: u8| round_u8(((c as f32 / 255.0) * (1.0 - w - bk) + w) * 255.0);
    Some(Rgb(mix(base.0), mix(base.1), mix(base.2)))
}

/// `lab(L a b)` / `oklab(L a b)`.
fn parse_lab(inner: &str, ok: bool) -> Option<Rgb> {
    let t = tokens(split_slash(inner));
    if t.len() != 3 {
        return None;
    }
    let l = lab_l(t[0], ok)?;
    let a = lab_ab(t[1], ok)?;
    let b = lab_ab(t[2], ok)?;
    Some(if ok { oklab_to_rgb(l, a, b) } else { lab_to_rgb(l, a, b) })
}

/// `lch(L C H)` / `oklch(L C H)` — polar form of lab/oklab.
fn parse_lch(inner: &str, ok: bool) -> Option<Rgb> {
    let t = tokens(split_slash(inner));
    if t.len() != 3 {
        return None;
    }
    let l = lab_l(t[0], ok)?;
    let c = lch_c(t[1], ok)?;
    let h = if t[2] == "none" { 0.0 } else { parse_hue(t[2])? };
    let rad = h * core::f32::consts::PI / 180.0;
    let (a, b) = (c * libm::cosf(rad), c * libm::sinf(rad));
    Some(if ok { oklab_to_rgb(l, a, b) } else { lab_to_rgb(l, a, b) })
}

/// `color(<space> c1 c2 c3 [/ a])` — predefined + xyz colour spaces.
fn parse_color_fn(inner: &str) -> Option<Rgb> {
    let t = tokens(split_slash(inner));
    if t.len() != 4 {
        return None;
    }
    let c = [num_or_pct(t[1])?, num_or_pct(t[2])?, num_or_pct(t[3])?];
    let lin = match t[0] {
        // `color(srgb …)` gives display (gamma) values directly.
        "srgb" => return Some(gamma_to_rgb(c)),
        "srgb-linear" => c,
        "display-p3" => {
            xyz_to_lin_srgb(mat(&P3_TO_XYZ_D65, [srgb_lin(c[0]), srgb_lin(c[1]), srgb_lin(c[2])]))
        }
        "a98-rgb" => {
            xyz_to_lin_srgb(mat(&A98_TO_XYZ_D65, [a98_lin(c[0]), a98_lin(c[1]), a98_lin(c[2])]))
        }
        "prophoto-rgb" => xyz_to_lin_srgb(d50_to_d65(mat(
            &PROPHOTO_TO_XYZ_D50,
            [prophoto_lin(c[0]), prophoto_lin(c[1]), prophoto_lin(c[2])],
        ))),
        "rec2020" => xyz_to_lin_srgb(mat(
            &REC2020_TO_XYZ_D65,
            [rec2020_lin(c[0]), rec2020_lin(c[1]), rec2020_lin(c[2])],
        )),
        "xyz" | "xyz-d65" => xyz_to_lin_srgb(c),
        "xyz-d50" => xyz_to_lin_srgb(d50_to_d65(c)),
        _ => return None,
    };
    Some(lin_srgb_to_rgb(lin))
}

// — channel parsers —

/// `%`→fraction, bare number as-is, `none`→0.
fn frac(tok: &str) -> Option<f32> {
    if tok == "none" {
        return Some(0.0);
    }
    match tok.strip_suffix('%') {
        Some(p) => Some(p.parse::<f32>().ok()? / 100.0),
        None => tok.parse().ok(),
    }
}

/// `color()` channel: `%`→fraction (100%=1.0), bare number as-is, `none`→0.
fn num_or_pct(tok: &str) -> Option<f32> {
    frac(tok)
}

/// lab/oklab lightness. lab: `%`=0..100 or number; oklab: `%`=0..1 or number.
fn lab_l(tok: &str, ok: bool) -> Option<f32> {
    if tok == "none" {
        return Some(0.0);
    }
    match tok.strip_suffix('%') {
        Some(p) => Some(p.parse::<f32>().ok()? / 100.0 * if ok { 1.0 } else { 100.0 }),
        None => tok.parse().ok(),
    }
}

/// lab/oklab a/b axis. `%` reference: lab ±125, oklab ±0.4.
fn lab_ab(tok: &str, ok: bool) -> Option<f32> {
    if tok == "none" {
        return Some(0.0);
    }
    match tok.strip_suffix('%') {
        Some(p) => Some(p.parse::<f32>().ok()? / 100.0 * if ok { 0.4 } else { 125.0 }),
        None => tok.parse().ok(),
    }
}

/// lch/oklch chroma (≥0). `%` reference: lch 150, oklch 0.4.
fn lch_c(tok: &str, ok: bool) -> Option<f32> {
    if tok == "none" {
        return Some(0.0);
    }
    let v = match tok.strip_suffix('%') {
        Some(p) => p.parse::<f32>().ok()? / 100.0 * if ok { 0.4 } else { 150.0 },
        None => tok.parse().ok()?,
    };
    Some(v.max(0.0))
}

// — colour-space conversions —

fn lab_to_rgb(l: f32, a: f32, b: f32) -> Rgb {
    const K: f32 = 24389.0 / 27.0;
    const E: f32 = 216.0 / 24389.0;
    let fy = (l + 16.0) / 116.0;
    let fx = a / 500.0 + fy;
    let fz = fy - b / 200.0;
    let xr = if fx * fx * fx > E { fx * fx * fx } else { (116.0 * fx - 16.0) / K };
    let yr = if l > K * E { fy * fy * fy } else { l / K };
    let zr = if fz * fz * fz > E { fz * fz * fz } else { (116.0 * fz - 16.0) / K };
    // Scale by the D50 whitepoint, adapt to D65, project to linear sRGB.
    let xyz_d50 = [xr * 0.9642956, yr, zr * 0.8251046];
    lin_srgb_to_rgb(xyz_to_lin_srgb(d50_to_d65(xyz_d50)))
}

fn oklab_to_rgb(l: f32, a: f32, b: f32) -> Rgb {
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;
    let (ll, mm, ss) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let lin = [
        4.0767416621 * ll - 3.3077115913 * mm + 0.2309699292 * ss,
        -1.2684380046 * ll + 2.6097574011 * mm - 0.3413193965 * ss,
        -0.0041960863 * ll - 0.7034186147 * mm + 1.7076147010 * ss,
    ];
    lin_srgb_to_rgb(lin)
}

/// 3×3 (row-major) × vec3.
fn mat(m: &[f32; 9], v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
        m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
    ]
}

fn xyz_to_lin_srgb(xyz: [f32; 3]) -> [f32; 3] {
    mat(&XYZ_D65_TO_LIN_SRGB, xyz)
}
fn d50_to_d65(xyz: [f32; 3]) -> [f32; 3] {
    mat(&BRADFORD_D50_TO_D65, xyz)
}

/// Linear sRGB → gamma sRGB → clipped `Rgb`.
fn lin_srgb_to_rgb(lin: [f32; 3]) -> Rgb {
    Rgb(
        round_u8(clamp(srgb_gamma(lin[0]), 0.0, 1.0) * 255.0),
        round_u8(clamp(srgb_gamma(lin[1]), 0.0, 1.0) * 255.0),
        round_u8(clamp(srgb_gamma(lin[2]), 0.0, 1.0) * 255.0),
    )
}

/// Already-gamma sRGB fractions → clipped `Rgb`.
fn gamma_to_rgb(c: [f32; 3]) -> Rgb {
    Rgb(
        round_u8(clamp(c[0], 0.0, 1.0) * 255.0),
        round_u8(clamp(c[1], 0.0, 1.0) * 255.0),
        round_u8(clamp(c[2], 0.0, 1.0) * 255.0),
    )
}

// — transfer functions (gamma ↔ linear) —

fn srgb_gamma(c: f32) -> f32 {
    let (a, s) = (fabs(c), if c < 0.0 { -1.0 } else { 1.0 });
    if a <= 0.0031308 { 12.92 * c } else { s * (1.055 * libm::powf(a, 1.0 / 2.4) - 0.055) }
}
fn srgb_lin(c: f32) -> f32 {
    let (a, s) = (fabs(c), if c < 0.0 { -1.0 } else { 1.0 });
    if a <= 0.04045 { c / 12.92 } else { s * libm::powf((a + 0.055) / 1.055, 2.4) }
}
fn a98_lin(c: f32) -> f32 {
    let (a, s) = (fabs(c), if c < 0.0 { -1.0 } else { 1.0 });
    s * libm::powf(a, 563.0 / 256.0)
}
fn prophoto_lin(c: f32) -> f32 {
    let (a, s) = (fabs(c), if c < 0.0 { -1.0 } else { 1.0 });
    if a <= 16.0 / 512.0 { c / 16.0 } else { s * libm::powf(a, 1.8) }
}
fn rec2020_lin(c: f32) -> f32 {
    const AL: f32 = 1.09929682680944;
    const BE: f32 = 0.018053968510807;
    let (a, s) = (fabs(c), if c < 0.0 { -1.0 } else { 1.0 });
    if a < BE * 4.5 { c / 4.5 } else { s * libm::powf((a + AL - 1.0) / AL, 1.0 / 0.45) }
}

// — matrices (linear space → XYZ, and XYZ → linear sRGB) —

static XYZ_D65_TO_LIN_SRGB: [f32; 9] = [
    3.2409699419045226, -1.537383177570094, -0.4986107602930034,
    -0.9692436362808796, 1.8759675015077202, 0.04155505740717559,
    0.05563007969699366, -0.20397695888897652, 1.0569715142428786,
];
static BRADFORD_D50_TO_D65: [f32; 9] = [
    0.955473452704218, -0.0230985368742614, 0.0632593086610217,
    -0.0283697069632081, 1.0099954580058226, 0.0210413989669430,
    0.0123140016883199, -0.0205076964334779, 1.3303659366080753,
];
static P3_TO_XYZ_D65: [f32; 9] = [
    0.4865709486482162, 0.26566769316909306, 0.19821728523436247,
    0.2289745640697488, 0.6917385218365064, 0.079286914093745,
    0.0, 0.04511338185890264, 1.043944368900976,
];
static A98_TO_XYZ_D65: [f32; 9] = [
    0.5766690429101305, 0.1855582379065463, 0.1882286462349947,
    0.29734497525053605, 0.6273635662554661, 0.07529145849399788,
    0.02703136138641234, 0.07068885253582723, 0.9913375368376388,
];
static PROPHOTO_TO_XYZ_D50: [f32; 9] = [
    0.7977604896723027, 0.13518583717574031, 0.0313493495815248,
    0.2880711282292934, 0.7118432178101014, 0.00008565396060525902,
    0.0, 0.0, 0.8251046025104601,
];
static REC2020_TO_XYZ_D65: [f32; 9] = [
    0.6369580483012914, 0.14461690358620832, 0.16888097516417205,
    0.2627002120112671, 0.6779980715188708, 0.05930171646986196,
    0.0, 0.028072693049087428, 1.060985057710791,
];

// ── named colours ────────────────────────────────────────────────────────

fn named_color(name: &str) -> Option<Rgb> {
    for &(n, r, g, b) in NAMED.iter() {
        if n == name {
            return Some(Rgb(r, g, b));
        }
    }
    for &(n, r, g, b) in SYSTEM.iter() {
        if n == name {
            return Some(Rgb(r, g, b));
        }
    }
    None
}

/// CSS system colours (CSS Color 4 §system-colors) with light-theme values.
/// Deprecated keywords are aliased to their modern equivalent's value so the
/// WPT "deprecated-sameas" reftests (which compare a deprecated colour to its
/// modern target) render identically. Names are pre-lowercased for lookup.
static SYSTEM: &[(&str, u8, u8, u8)] = &[
    // modern
    ("canvas", 255, 255, 255),
    ("canvastext", 0, 0, 0),
    ("linktext", 0, 0, 238),
    ("visitedtext", 85, 26, 139),
    ("activetext", 255, 0, 0),
    ("buttonface", 240, 240, 240),
    ("buttontext", 0, 0, 0),
    ("buttonborder", 118, 118, 118),
    ("field", 255, 255, 255),
    ("fieldtext", 0, 0, 0),
    ("highlight", 0, 120, 215),
    ("highlighttext", 255, 255, 255),
    ("selecteditem", 0, 120, 215),
    ("selecteditemtext", 255, 255, 255),
    ("mark", 255, 255, 0),
    ("marktext", 0, 0, 0),
    ("graytext", 128, 128, 128),
    ("accentcolor", 0, 120, 215),
    ("accentcolortext", 255, 255, 255),
    // deprecated → aliased to a modern value (must match for -sameas reftests)
    ("activeborder", 118, 118, 118),      // ButtonBorder
    ("activecaption", 255, 255, 255),     // Canvas
    ("appworkspace", 255, 255, 255),      // Canvas
    ("background", 255, 255, 255),        // Canvas
    ("buttonhighlight", 240, 240, 240),   // ButtonFace
    ("buttonshadow", 240, 240, 240),      // ButtonFace
    ("captiontext", 0, 0, 0),             // CanvasText
    ("inactiveborder", 118, 118, 118),    // ButtonBorder
    ("inactivecaption", 255, 255, 255),   // Canvas
    ("inactivecaptiontext", 128, 128, 128), // GrayText
    ("infobackground", 255, 255, 255),    // Canvas
    ("infotext", 0, 0, 0),                // CanvasText
    ("menu", 255, 255, 255),              // Canvas
    ("menutext", 0, 0, 0),                // CanvasText
    ("scrollbar", 255, 255, 255),         // Canvas
    ("threeddarkshadow", 118, 118, 118),  // ButtonBorder
    ("threedface", 240, 240, 240),        // ButtonFace
    ("threedhighlight", 118, 118, 118),   // ButtonBorder
    ("threedlightshadow", 118, 118, 118), // ButtonBorder
    ("threedshadow", 118, 118, 118),      // ButtonBorder
    ("window", 255, 255, 255),            // Canvas
    ("windowframe", 118, 118, 118),       // ButtonBorder
    ("windowtext", 0, 0, 0),              // CanvasText
];

/// The full CSS Color Module Level 4 extended colour keywords (148 entries,
/// incl. the `aqua`/`cyan`, `fuchsia`/`magenta`, `gray`/`grey` synonyms and
/// `rebeccapurple`). `transparent`/`currentcolor`/`inherit` are intentionally
/// absent → they resolve to `None` (keep inherited).
static NAMED: &[(&str, u8, u8, u8)] = &[
    ("aliceblue", 240, 248, 255),
    ("antiquewhite", 250, 235, 215),
    ("aqua", 0, 255, 255),
    ("aquamarine", 127, 255, 212),
    ("azure", 240, 255, 255),
    ("beige", 245, 245, 220),
    ("bisque", 255, 228, 196),
    ("black", 0, 0, 0),
    ("blanchedalmond", 255, 235, 205),
    ("blue", 0, 0, 255),
    ("blueviolet", 138, 43, 226),
    ("brown", 165, 42, 42),
    ("burlywood", 222, 184, 135),
    ("cadetblue", 95, 158, 160),
    ("chartreuse", 127, 255, 0),
    ("chocolate", 210, 105, 30),
    ("coral", 255, 127, 80),
    ("cornflowerblue", 100, 149, 237),
    ("cornsilk", 255, 248, 220),
    ("crimson", 220, 20, 60),
    ("cyan", 0, 255, 255),
    ("darkblue", 0, 0, 139),
    ("darkcyan", 0, 139, 139),
    ("darkgoldenrod", 184, 134, 11),
    ("darkgray", 169, 169, 169),
    ("darkgreen", 0, 100, 0),
    ("darkgrey", 169, 169, 169),
    ("darkkhaki", 189, 183, 107),
    ("darkmagenta", 139, 0, 139),
    ("darkolivegreen", 85, 107, 47),
    ("darkorange", 255, 140, 0),
    ("darkorchid", 153, 50, 204),
    ("darkred", 139, 0, 0),
    ("darksalmon", 233, 150, 122),
    ("darkseagreen", 143, 188, 143),
    ("darkslateblue", 72, 61, 139),
    ("darkslategray", 47, 79, 79),
    ("darkslategrey", 47, 79, 79),
    ("darkturquoise", 0, 206, 209),
    ("darkviolet", 148, 0, 211),
    ("deeppink", 255, 20, 147),
    ("deepskyblue", 0, 191, 255),
    ("dimgray", 105, 105, 105),
    ("dimgrey", 105, 105, 105),
    ("dodgerblue", 30, 144, 255),
    ("firebrick", 178, 34, 34),
    ("floralwhite", 255, 250, 240),
    ("forestgreen", 34, 139, 34),
    ("fuchsia", 255, 0, 255),
    ("gainsboro", 220, 220, 220),
    ("ghostwhite", 248, 248, 255),
    ("gold", 255, 215, 0),
    ("goldenrod", 218, 165, 32),
    ("gray", 128, 128, 128),
    ("green", 0, 128, 0),
    ("greenyellow", 173, 255, 47),
    ("grey", 128, 128, 128),
    ("honeydew", 240, 255, 240),
    ("hotpink", 255, 105, 180),
    ("indianred", 205, 92, 92),
    ("indigo", 75, 0, 130),
    ("ivory", 255, 255, 240),
    ("khaki", 240, 230, 140),
    ("lavender", 230, 230, 250),
    ("lavenderblush", 255, 240, 245),
    ("lawngreen", 124, 252, 0),
    ("lemonchiffon", 255, 250, 205),
    ("lightblue", 173, 216, 230),
    ("lightcoral", 240, 128, 128),
    ("lightcyan", 224, 255, 255),
    ("lightgoldenrodyellow", 250, 250, 210),
    ("lightgray", 211, 211, 211),
    ("lightgreen", 144, 238, 144),
    ("lightgrey", 211, 211, 211),
    ("lightpink", 255, 182, 193),
    ("lightsalmon", 255, 160, 122),
    ("lightseagreen", 32, 178, 170),
    ("lightskyblue", 135, 206, 250),
    ("lightslategray", 119, 136, 153),
    ("lightslategrey", 119, 136, 153),
    ("lightsteelblue", 176, 196, 222),
    ("lightyellow", 255, 255, 224),
    ("lime", 0, 255, 0),
    ("limegreen", 50, 205, 50),
    ("linen", 250, 240, 230),
    ("magenta", 255, 0, 255),
    ("maroon", 128, 0, 0),
    ("mediumaquamarine", 102, 205, 170),
    ("mediumblue", 0, 0, 205),
    ("mediumorchid", 186, 85, 211),
    ("mediumpurple", 147, 112, 219),
    ("mediumseagreen", 60, 179, 113),
    ("mediumslateblue", 123, 104, 238),
    ("mediumspringgreen", 0, 250, 154),
    ("mediumturquoise", 72, 209, 204),
    ("mediumvioletred", 199, 21, 133),
    ("midnightblue", 25, 25, 112),
    ("mintcream", 245, 255, 250),
    ("mistyrose", 255, 228, 225),
    ("moccasin", 255, 228, 181),
    ("navajowhite", 255, 222, 173),
    ("navy", 0, 0, 128),
    ("oldlace", 253, 245, 230),
    ("olive", 128, 128, 0),
    ("olivedrab", 107, 142, 35),
    ("orange", 255, 165, 0),
    ("orangered", 255, 69, 0),
    ("orchid", 218, 112, 214),
    ("palegoldenrod", 238, 232, 170),
    ("palegreen", 152, 251, 152),
    ("paleturquoise", 175, 238, 238),
    ("palevioletred", 219, 112, 147),
    ("papayawhip", 255, 239, 213),
    ("peachpuff", 255, 218, 185),
    ("peru", 205, 133, 63),
    ("pink", 255, 192, 203),
    ("plum", 221, 160, 221),
    ("powderblue", 176, 224, 230),
    ("purple", 128, 0, 128),
    ("rebeccapurple", 102, 51, 153),
    ("red", 255, 0, 0),
    ("rosybrown", 188, 143, 143),
    ("royalblue", 65, 105, 225),
    ("saddlebrown", 139, 69, 19),
    ("salmon", 250, 128, 114),
    ("sandybrown", 244, 164, 96),
    ("seagreen", 46, 139, 87),
    ("seashell", 255, 245, 238),
    ("sienna", 160, 82, 45),
    ("silver", 192, 192, 192),
    ("skyblue", 135, 206, 235),
    ("slateblue", 106, 90, 205),
    ("slategray", 112, 128, 144),
    ("slategrey", 112, 128, 144),
    ("snow", 255, 250, 250),
    ("springgreen", 0, 255, 127),
    ("steelblue", 70, 130, 180),
    ("tan", 210, 180, 140),
    ("teal", 0, 128, 128),
    ("thistle", 216, 191, 216),
    ("tomato", 255, 99, 71),
    ("turquoise", 64, 224, 208),
    ("violet", 238, 130, 238),
    ("wheat", 245, 222, 179),
    ("white", 255, 255, 255),
    ("whitesmoke", 245, 245, 245),
    ("yellow", 255, 255, 0),
    ("yellowgreen", 154, 205, 50),
];

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex3_expands_each_nibble_times_17() {
        assert_eq!(parse_color("#f00"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("#0f0"), Some(Rgb(0, 255, 0)));
        assert_eq!(parse_color("#00f"), Some(Rgb(0, 0, 255)));
        assert_eq!(parse_color("#abc"), Some(Rgb(0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn hex4_drops_alpha() {
        // #rgba — alpha nibble parsed and discarded, opaque RGB returned.
        assert_eq!(parse_color("#f008"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("#abcd"), Some(Rgb(0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn hex6_and_case_insensitive() {
        assert_eq!(parse_color("#ff0000"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("#00FF00"), Some(Rgb(0, 255, 0)));
        assert_eq!(parse_color("#6495ed"), Some(Rgb(100, 149, 237)));
    }

    #[test]
    fn hex8_drops_alpha() {
        assert_eq!(parse_color("#ff000080"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("#11223344"), Some(Rgb(0x11, 0x22, 0x33)));
    }

    #[test]
    fn bad_hex_is_none() {
        assert_eq!(parse_color("#gg0000"), None);
        assert_eq!(parse_color("#12345"), None); // length 5 invalid
        assert_eq!(parse_color("#"), None);
    }

    #[test]
    fn rgb_int_comma_and_space() {
        assert_eq!(parse_color("rgb(255,0,0)"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("rgb(0, 128, 255)"), Some(Rgb(0, 128, 255)));
        assert_eq!(parse_color("rgb(10 20 30)"), Some(Rgb(10, 20, 30)));
    }

    #[test]
    fn rgb_percent() {
        assert_eq!(parse_color("rgb(100%, 0%, 0%)"), Some(Rgb(255, 0, 0)));
        // 50% of 255 = 127.5 → rounds to 128 (matches browsers).
        assert_eq!(parse_color("rgb(50%,50%,50%)"), Some(Rgb(128, 128, 128)));
    }

    #[test]
    fn rgb_clamps_out_of_range() {
        assert_eq!(parse_color("rgb(300, -20, 999)"), Some(Rgb(255, 0, 255)));
        assert_eq!(parse_color("rgb(150%, 0%, 0%)"), Some(Rgb(255, 0, 0)));
    }

    #[test]
    fn rgba_and_slash_alpha_drop_alpha() {
        assert_eq!(parse_color("rgba(255,0,0,0.5)"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("rgba(0, 128, 255, 1)"), Some(Rgb(0, 128, 255)));
        assert_eq!(parse_color("rgb(255 0 0 / 0.5)"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("rgba(10 20 30 / 50%)"), Some(Rgb(10, 20, 30)));
    }

    #[test]
    fn hsl_known_conversions() {
        assert_eq!(parse_color("hsl(0, 100%, 50%)"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("hsl(120, 100%, 50%)"), Some(Rgb(0, 255, 0)));
        assert_eq!(parse_color("hsl(240, 100%, 50%)"), Some(Rgb(0, 0, 255)));
        // achromatic: any hue, 0 saturation → grey by lightness.
        assert_eq!(parse_color("hsl(0, 0%, 50%)"), Some(Rgb(128, 128, 128)));
        assert_eq!(parse_color("hsl(0, 0%, 0%)"), Some(Rgb(0, 0, 0)));
        assert_eq!(parse_color("hsl(0, 0%, 100%)"), Some(Rgb(255, 255, 255)));
    }

    #[test]
    fn hsl_space_and_deg_and_wrap() {
        assert_eq!(parse_color("hsl(120 100% 50%)"), Some(Rgb(0, 255, 0)));
        assert_eq!(parse_color("hsl(120deg 100% 50%)"), Some(Rgb(0, 255, 0)));
        // 480 wraps to 120.
        assert_eq!(parse_color("hsl(480, 100%, 50%)"), Some(Rgb(0, 255, 0)));
    }

    #[test]
    fn hsla_and_slash_alpha_drop_alpha() {
        assert_eq!(parse_color("hsla(0, 100%, 50%, 0.5)"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("hsl(0 100% 50% / 0.5)"), Some(Rgb(255, 0, 0)));
    }

    #[test]
    fn hsl_secondary_hues() {
        // cyan / magenta / yellow at full sat, mid lightness.
        assert_eq!(parse_color("hsl(180, 100%, 50%)"), Some(Rgb(0, 255, 255)));
        assert_eq!(parse_color("hsl(300, 100%, 50%)"), Some(Rgb(255, 0, 255)));
        assert_eq!(parse_color("hsl(60, 100%, 50%)"), Some(Rgb(255, 255, 0)));
    }

    #[test]
    fn named_colours() {
        assert_eq!(parse_color("red"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("RED"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("  Red  "), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("rebeccapurple"), Some(Rgb(102, 51, 153)));
        assert_eq!(parse_color("cornflowerblue"), Some(Rgb(100, 149, 237)));
        assert_eq!(parse_color("aqua"), parse_color("cyan"));
        assert_eq!(parse_color("gray"), parse_color("grey"));
        assert_eq!(parse_color("fuchsia"), parse_color("magenta"));
        assert_eq!(parse_color("yellowgreen"), Some(Rgb(154, 205, 50)));
    }

    #[test]
    fn keywords_that_keep_inherited_are_none() {
        assert_eq!(parse_color("transparent"), None);
        assert_eq!(parse_color("currentcolor"), None);
        assert_eq!(parse_color("currentColor"), None);
        assert_eq!(parse_color("inherit"), None);
    }

    #[test]
    fn invalid_input_is_none() {
        assert_eq!(parse_color(""), None);
        assert_eq!(parse_color("   "), None);
        assert_eq!(parse_color("notacolor"), None);
        assert_eq!(parse_color("rgb(1,2)"), None); // too few channels
        assert_eq!(parse_color("rgb(1,2,3,4,5)"), None); // too many
        assert_eq!(parse_color("hsl(0, 100, 50)"), None); // s/l need %
        assert_eq!(parse_color("rgb(a,b,c)"), None);
        assert_eq!(parse_color("hsl(x, 100%, 50%)"), None);
    }

    #[test]
    fn named_table_has_148_entries() {
        assert_eq!(NAMED.len(), 148);
    }

    // — CSS Color 4 —

    fn close(got: Rgb, want: Rgb, tol: i32) {
        let d = (got.0 as i32 - want.0 as i32)
            .abs()
            .max((got.1 as i32 - want.1 as i32).abs())
            .max((got.2 as i32 - want.2 as i32).abs());
        assert!(d <= tol, "got {got:?} want {want:?} (Δ={d})");
    }

    #[test]
    fn hwb_pure_and_gray() {
        close(parse_color("hwb(120 0% 0%)").unwrap(), Rgb(0, 255, 0), 1);
        close(parse_color("hwb(0 50% 50%)").unwrap(), Rgb(128, 128, 128), 2);
    }

    #[test]
    fn lab_lch_green() {
        // CSS Color 4 sample: lab(46.2775% -47.5621 48.5837) ≈ #008000.
        close(parse_color("lab(46.2775% -47.5621 48.5837)").unwrap(), Rgb(0, 128, 0), 3);
        close(parse_color("lch(46.2775% 68 134.39)").unwrap(), Rgb(0, 128, 0), 4);
    }

    #[test]
    fn oklab_oklch_red() {
        // oklch red ≈ #ff0000.
        close(parse_color("oklch(0.628 0.2577 29.23)").unwrap(), Rgb(255, 0, 0), 4);
        close(parse_color("oklab(0.628 0.2249 0.1258)").unwrap(), Rgb(255, 0, 0), 4);
    }

    #[test]
    fn color_fn_spaces() {
        close(parse_color("color(srgb 0 0.5 0)").unwrap(), Rgb(0, 128, 0), 1);
        close(parse_color("color(srgb-linear 1 1 1)").unwrap(), Rgb(255, 255, 255), 1);
        // display-p3 green is out of sRGB gamut → clips near pure green.
        close(parse_color("color(display-p3 0 1 0)").unwrap(), Rgb(0, 255, 0), 6);
        close(parse_color("color(xyz 0 0 0)").unwrap(), Rgb(0, 0, 0), 1);
    }

    #[test]
    fn system_colors_and_deprecated_aliases() {
        assert_eq!(parse_color("Canvas"), Some(Rgb(255, 255, 255)));
        assert_eq!(parse_color("CanvasText"), Some(Rgb(0, 0, 0)));
        // Deprecated keyword renders identically to its modern target.
        assert_eq!(parse_color("ActiveBorder"), parse_color("ButtonBorder"));
        assert_eq!(parse_color("WindowText"), parse_color("CanvasText"));
    }
}
