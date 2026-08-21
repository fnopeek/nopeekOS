//! Token → concrete BGRA color.
//!
//! Two curated palettes (DARK + LIGHT) with fixed surface/border/text
//! values. Only the Accent and AccentMuted tokens derive from the
//! wallpaper's extracted theme. The mode is picked from the
//! `theme` config key (`dark` | `light` | `auto`). `auto` uses the
//! wallpaper's background luminance to decide.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, Ordering};

use super::abi::{Palette, Token};

struct ThemePalette {
    page:             u32,
    surface:          u32,
    surface_elevated: u32,
    surface_muted:    u32,
    surface_hover:    u32,
    border:           u32,
    on_surface:       u32,
    on_surface_muted: u32,
    on_surface_faint: u32,
    success:          u32,
    warning:          u32,
    danger:           u32,
}

const DARK: ThemePalette = ThemePalette {
    page:             0xFF131517,
    surface:          0xFF17191B,
    surface_elevated: 0xFF1C1F21,
    surface_muted:    0xFF1E2124,
    surface_hover:    0xFF262A2E,
    border:           0xFF2C3034,
    on_surface:       0xFFE6E8EA,
    on_surface_muted: 0xFFA2A8AE,
    on_surface_faint: 0xFF6F767C,
    success:          0xFF8FBF9F,
    warning:          0xFFE0B877,
    danger:           0xFFE07B7B,
};

const LIGHT: ThemePalette = ThemePalette {
    page:             0xFFFFFFFF,
    surface:          0xFFFAF9F7,
    surface_elevated: 0xFFF0F0EC,
    surface_muted:    0xFFF1F0EC,
    surface_hover:    0xFFE4E3DE,
    border:           0xFFDEDCD6,
    on_surface:       0xFF1B1D1F,
    on_surface_muted: 0xFF5C6166,
    on_surface_faint: 0xFF8A9096,
    success:          0xFF4D8A63,
    warning:          0xFFB07A28,
    danger:           0xFFC04C4C,
};

/// Named accent presets from the design. `accent = auto` (the default)
/// keeps deriving the accent from the wallpaper instead.
const ACCENT_PRESETS: [(&str, u32); 4] = [
    ("rose",  0xFFE39BAB),
    ("sage",  0xFF8FBF9F),
    ("blue",  0xFF9FB8E0),
    ("amber", 0xFFE0B877),
];

const DEFAULT_ACCENT: u32 = 0xFFE39BAB;


// ── Code schemes (syntax colours) ─────────────────────────────────────
//
// Values taken verbatim from VSCodium's built-in theme JSONs, resolved
// the way TextMate resolves scopes (most specific scope wins). A scheme
// supplies ONLY these nine colours — the canvas stays `Page` and plain
// text stays `OnSurface`. Importing a scheme's own background too would
// fight the glass surfaces, and a dark scheme picked under a light theme
// would then paint dark-on-white. Preference and canvas are two separate
// things; this knob only moves the preference.

struct CodeScheme {
    name:     &'static str,
    /// True if the scheme was authored for a light canvas. Only used to
    /// warn on an obvious mismatch — an explicit choice is still honoured.
    light:    bool,
    keyword:  u32,
    control:  u32,
    string:   u32,
    comment:  u32,
    number:   u32,
    function: u32,
    typ:      u32,
    variable: u32,
    constant: u32,
}

/// Every scheme `set code.scheme <name>` accepts. `auto` (the default)
/// picks `dark-plus` or `light-plus` from the active theme.
const CODE_SCHEMES: [CodeScheme; 8] = [
    CodeScheme { name: "dark-plus", light: false,
        keyword: 0xFF569CD6, control: 0xFFC586C0, string:   0xFFCE9178,
        comment: 0xFF6A9955, number:  0xFFB5CEA8, function: 0xFFDCDCAA,
        typ:     0xFF4EC9B0, variable: 0xFF9CDCFE, constant: 0xFF569CD6 },
    CodeScheme { name: "light-plus", light: true,
        keyword: 0xFF0000FF, control: 0xFFAF00DB, string:   0xFFA31515,
        comment: 0xFF008000, number:  0xFF098658, function: 0xFF795E26,
        typ:     0xFF267F99, variable: 0xFF001080, constant: 0xFF0000FF },
    CodeScheme { name: "monokai", light: false,
        keyword: 0xFF66D9EF, control: 0xFFF92672, string:   0xFFE6DB74,
        comment: 0xFF88846F, number:  0xFFAE81FF, function: 0xFFA6E22E,
        typ:     0xFFA6E22E, variable: 0xFFF8F8F2, constant: 0xFFAE81FF },
    CodeScheme { name: "solarized-dark", light: false,
        keyword: 0xFF93A1A1, control: 0xFF859900, string:   0xFF2AA198,
        comment: 0xFF586E75, number:  0xFFD33682, function: 0xFF268BD2,
        typ:     0xFFCB4B16, variable: 0xFF93A1A1, constant: 0xFFB58900 },
    CodeScheme { name: "solarized-light", light: true,
        keyword: 0xFF586E75, control: 0xFF859900, string:   0xFF2AA198,
        comment: 0xFF93A1A1, number:  0xFFD33682, function: 0xFF268BD2,
        typ:     0xFFCB4B16, variable: 0xFF93A1A1, constant: 0xFFB58900 },
    CodeScheme { name: "abyss", light: false,
        keyword: 0xFF9966B8, control: 0xFF225588, string:   0xFF22AA44,
        comment: 0xFF384887, number:  0xFFF280D0, function: 0xFFDDBB88,
        typ:     0xFFFFEEBB, variable: 0xFF6688CC, constant: 0xFFF280D0 },
    CodeScheme { name: "kimbie-dark", light: false,
        keyword: 0xFF98676A, control: 0xFF98676A, string:   0xFF889B4A,
        comment: 0xFFA57A4C, number:  0xFFF79A32, function: 0xFF8AB1B0,
        typ:     0xFFF06431, variable: 0xFFDC3958, constant: 0xFFF79A32 },
    CodeScheme { name: "quiet-light", light: true,
        keyword: 0xFF7A3E9D, control: 0xFF4B69C6, string:   0xFF448C27,
        comment: 0xFFAAAAAA, number:  0xFF9C5D27, function: 0xFFAA3731,
        typ:     0xFF7A3E9D, variable: 0xFF7A3E9D, constant: 0xFF9C5D27 },
];

/// Scheme names, for `set code.scheme` and its error message.
pub fn code_scheme_names() -> &'static [&'static str] {
    const NAMES: [&str; 8] = [
        "dark-plus", "light-plus", "monokai", "solarized-dark",
        "solarized-light", "abyss", "kimbie-dark", "quiet-light",
    ];
    &NAMES
}

/// Is `name` a scheme we know? `auto` counts.
pub fn code_scheme_exists(name: &str) -> bool {
    name == "auto" || CODE_SCHEMES.iter().any(|s| s.name == name)
}

/// Does `name` target a light canvas? `None` for unknown / `auto`.
pub fn code_scheme_is_light(name: &str) -> Option<bool> {
    CODE_SCHEMES.iter().find(|s| s.name == name).map(|s| s.light)
}

fn code_scheme() -> &'static CodeScheme {
    let want = crate::config::get("code.scheme").unwrap_or_default();
    let want = want.trim();
    if !want.is_empty() && want != "auto" {
        if let Some(s) = CODE_SCHEMES.iter().find(|s| s.name == want) {
            return s;
        }
    }
    // auto — follow the theme.
    let idx = if is_light_theme() { 1 } else { 0 };
    &CODE_SCHEMES[idx]
}

pub fn current() -> Palette {
    let mut colors = [0u32; super::abi::PALETTE_SLOTS];
    for (i, slot) in colors.iter_mut().enumerate() {
        *slot = resolve(token_at(i));
    }
    Palette { colors }
}

/// Opacity (0..255) of floating chrome — the bar card and the dock tray.
///
/// The design paints panels as a FLAT colour and gets its glass look from
/// a backdrop blur, which we don't have. Rendering them see-through
/// instead lets a busy wallpaper's texture read straight through the
/// panel and destroys exactly that flat-chrome impression — on the
/// design's smooth gradient the same value looks fine, on marble it does
/// not. Hence a high default plus a live knob: `set shade.chrome_opacity
/// <0..255>` (255 = flat, ~180 = clearly see-through). Light mode stays
/// lower because a near-white panel washes out faster.
pub fn chrome_opacity() -> u32 {
    let dflt = if is_light_theme() { 205 } else { 235 };
    opacity_key("shade.chrome_opacity").unwrap_or(dflt)
}

fn opacity_key(key: &str) -> Option<u32> {
    crate::config::get(key)
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|v| v.min(255))
}

// The two panel windows, published by the compositor. The rasterizer runs
// while the SCENES lock is held and must not reach for the compositor
// lock, so the ids travel as plain atomics. 0 = no such panel.
static BAR_WINDOW:  AtomicU32 = AtomicU32::new(0);
static DOCK_WINDOW: AtomicU32 = AtomicU32::new(0);

pub fn set_bar_window(id: Option<u32>) {
    BAR_WINDOW.store(id.unwrap_or(0), Ordering::Relaxed);
}

pub fn set_dock_window(id: Option<u32>) {
    DOCK_WINDOW.store(id.unwrap_or(0), Ordering::Relaxed);
}

/// Drop both per-panel overrides. Called when `shade.chrome_opacity` is
/// set: the shared knob is the master, so setting it always moves both
/// panels again — otherwise a value tried once on the bar would silently
/// outrank every later shared setting, with no way back short of editing
/// the config blob. Returns true if anything was actually cleared.
pub fn clear_panel_opacity_overrides() -> bool {
    let bar  = crate::config::unset("shade.bar_opacity");
    let dock = crate::config::unset("shade.dock_opacity");
    bar || dock
}

/// Opacity for one panel window: its own knob if set, else the shared
/// `shade.chrome_opacity`, else the theme default. Lets the bar stay
/// legible while the dock reads more like glass (or the other way round).
/// Setting the shared knob clears these — see above.
pub fn panel_opacity(window_id: u32) -> u32 {
    let key = if window_id != 0 && BAR_WINDOW.load(Ordering::Relaxed) == window_id {
        "shade.bar_opacity"
    } else if window_id != 0 && DOCK_WINDOW.load(Ordering::Relaxed) == window_id {
        "shade.dock_opacity"
    } else {
        return chrome_opacity();
    };
    opacity_key(key).unwrap_or_else(chrome_opacity)
}

pub fn resolve(token: Token) -> u32 {
    let is_light = is_light_theme();
    let t = if is_light { &LIGHT } else { &DARK };

    match token {
        Token::Page            => glass(t.page, is_light),
        Token::Surface         => glass(t.surface, is_light),
        Token::SurfaceElevated => glass(t.surface_elevated, is_light),
        Token::SurfaceMuted    => glass(t.surface_muted, is_light),
        Token::SurfaceHover    => glass(t.surface_hover, is_light),
        Token::Border          => t.border,
        Token::OnSurface       => t.on_surface,
        Token::OnSurfaceMuted  => t.on_surface_muted,
        Token::OnSurfaceFaint  => t.on_surface_faint,
        Token::Success         => t.success,
        Token::Warning         => t.warning,
        Token::Danger          => t.danger,

        Token::Accent          => accent_adjusted(t.surface),
        Token::AccentMuted     => accent_over(t.surface, 38),
        Token::AccentRing      => accent_over(t.surface, 56),
        Token::AccentLine      => accent_over(t.surface, 115),
        Token::OnAccent        => on_accent(t.surface),

        // Code tokens come from the scheme, not the theme ramp.
        Token::CodeKeyword     => code_scheme().keyword,
        Token::CodeControl     => code_scheme().control,
        Token::CodeString      => code_scheme().string,
        Token::CodeComment     => code_scheme().comment,
        Token::CodeNumber      => code_scheme().number,
        Token::CodeFunction    => code_scheme().function,
        Token::CodeType        => code_scheme().typ,
        Token::CodeVariable    => code_scheme().variable,
        Token::CodeConstant    => code_scheme().constant,
    }
}

/// A translucent glass-fill surface (Surface / SurfaceElevated / SurfaceMuted).
/// In light mode these are near-white and, blended over a bright wallpaper,
/// wash out / glare. Darken them proportionally to the wallpaper's overall
/// luminance so every glass surface (loop, dock, bar, widget apps) keeps a
/// steady readable tone regardless of how bright the background is. Dark mode
/// (dark wallpapers) never had the problem, so it's left untouched.
fn glass(color: u32, is_light: bool) -> u32 {
    if !is_light { return color; }
    let shift = light_glass_shift();
    if shift == 0 { return color; }
    darken(color, shift.min(255) as u8)
}

/// How many 0..255 steps to darken a light glass surface, driven by the
/// wallpaper's overall luminance. Only bright wallpapers (from ~140 up) darken,
/// ramping to full strength at pure white. Strength is tunable live via
/// `set shade.light_tint <0..100>` (0 = off, 100 = maximum) without a rebuild.
fn light_glass_shift() -> u32 {
    let strength = crate::config::get("shade.light_tint")
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(70)
        .min(100);
    if strength == 0 { return 0; }
    let l = crate::theme::avg_luminance() as u32; // 0..255
    let over = l.saturating_sub(140); // 0..115; only bright wallpapers darken
    (over * strength) / 100
}

pub fn is_light_theme() -> bool {
    let setting = crate::config::get("theme").unwrap_or_default();
    match setting.as_str() {
        "light" => true,
        "dark"  => false,
        _ => {
            // auto: wallpaper bg luminance decides. No wallpaper → dark.
            if crate::theme::is_active() {
                luminance(crate::theme::bg_color() | 0xFF00_0000) > 128
            } else {
                false
            }
        }
    }
}

/// The user's accent choice. `auto` (default) keeps deriving it from the
/// wallpaper; a preset name or a `#RRGGBB` literal pins it. Set live with
/// `set accent <rose|sage|blue|amber|auto|#RRGGBB>`.
pub fn accent_raw() -> u32 {
    let choice = crate::config::get("accent").unwrap_or_default();
    let choice = choice.trim();

    for (name, color) in ACCENT_PRESETS {
        if choice.eq_ignore_ascii_case(name) { return color; }
    }
    if let Some(hex) = choice.strip_prefix('#') {
        if let Ok(rgb) = u32::from_str_radix(hex, 16) {
            if hex.len() == 6 { return 0xFF00_0000 | rgb; }
        }
    }

    if crate::theme::is_active() {
        crate::gui::background::accent_color() | 0xFF00_0000
    } else {
        DEFAULT_ACCENT
    }
}

/// Names a `set accent …` value can take, for the shell's completion/help.
pub fn accent_preset_names() -> [&'static str; 4] {
    [ACCENT_PRESETS[0].0, ACCENT_PRESETS[1].0, ACCENT_PRESETS[2].0, ACCENT_PRESETS[3].0]
}

/// Accent adjusted for minimum contrast against the active surface.
/// Extracted wallpaper accents can be close in luminance to the chosen
/// theme surface (e.g. mid-grey wallpaper accent + LIGHT surface both
/// bright) — we darken/lighten to keep Accent readable.
fn accent_adjusted(surface: u32) -> u32 {
    let raw = accent_raw();
    let raw_lum = luminance(raw) as i32;
    let surf_lum = luminance(surface) as i32;
    if (raw_lum - surf_lum).abs() >= 80 { return raw; }
    if surf_lum > 128 { darken(raw, 0x60) } else { lighten(raw, 0x60) }
}

/// Accent pre-mixed over the surface at `weight`/255. The design writes
/// these as `rgba(accent, .15/.22/.45)`; the rasterizer ignores a token's
/// alpha byte (opacity comes from `bg_alpha`), so they are flattened here.
fn accent_over(surface: u32, weight: u32) -> u32 {
    blend(surface, accent_adjusted(surface), weight)
}

/// Ink on an Accent fill. Light mode always takes white — `accent_adjusted`
/// has already darkened the pastel accent against the bright surface, so
/// white carries. Dark mode takes a near-black tinted with the accent hue
/// (the design's per-preset `--accent-ink`), falling back to white if the
/// accent is itself dark.
fn on_accent(surface: u32) -> u32 {
    if is_light_theme() { return 0xFFFFFFFF; }
    let accent = accent_adjusted(surface);
    if luminance(accent) > 128 { blend(0xFF101010, accent, 24) } else { 0xFFFFFFFF }
}

fn luminance(c: u32) -> u32 {
    let r = (c >> 16) & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = c & 0xFF;
    (r * 299 + g * 587 + b * 114) / 1000
}

fn blend(base: u32, top: u32, weight: u32) -> u32 {
    let w = weight.min(255);
    let inv = 255 - w;
    let br = (base >> 16) & 0xFF;
    let bg = (base >> 8)  & 0xFF;
    let bb =  base        & 0xFF;
    let tr = (top  >> 16) & 0xFF;
    let tg = (top  >> 8)  & 0xFF;
    let tb =  top         & 0xFF;
    let r = (br * inv + tr * w) / 255;
    let g = (bg * inv + tg * w) / 255;
    let b = (bb * inv + tb * w) / 255;
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

fn lighten(color: u32, delta: u8) -> u32 {
    let a =  color & 0xFF00_0000;
    let r = ((color >> 16) & 0xFF).saturating_add(delta as u32).min(0xFF);
    let g = ((color >> 8)  & 0xFF).saturating_add(delta as u32).min(0xFF);
    let b = ( color        & 0xFF).saturating_add(delta as u32).min(0xFF);
    a | (r << 16) | (g << 8) | b
}

fn darken(color: u32, delta: u8) -> u32 {
    let a =  color & 0xFF00_0000;
    let r = ((color >> 16) & 0xFF).saturating_sub(delta as u32);
    let g = ((color >> 8)  & 0xFF).saturating_sub(delta as u32);
    let b = ( color        & 0xFF).saturating_sub(delta as u32);
    a | (r << 16) | (g << 8) | b
}

/// Wire id → token. The single table: `current()` fills the palette
/// through it and `npk_theme_token` answers apps through it, so an
/// appended token reaches the rasterizer and the SDK in one edit.
pub fn token_from_id(id: usize) -> Option<Token> {
    Some(match id {
        0  => Token::Surface,
        1  => Token::SurfaceElevated,
        2  => Token::SurfaceMuted,
        3  => Token::OnSurface,
        4  => Token::OnSurfaceMuted,
        5  => Token::OnAccent,
        6  => Token::Accent,
        7  => Token::AccentMuted,
        8  => Token::Border,
        9  => Token::Success,
        10 => Token::Warning,
        11 => Token::Danger,
        12 => Token::Page,
        13 => Token::SurfaceHover,
        14 => Token::OnSurfaceFaint,
        15 => Token::AccentRing,
        16 => Token::AccentLine,
        17 => Token::CodeKeyword,
        18 => Token::CodeControl,
        19 => Token::CodeString,
        20 => Token::CodeComment,
        21 => Token::CodeNumber,
        22 => Token::CodeFunction,
        23 => Token::CodeType,
        24 => Token::CodeVariable,
        25 => Token::CodeConstant,
        _  => return None,
    })
}

fn token_at(idx: usize) -> Token {
    token_from_id(idx).unwrap_or(Token::Surface)
}

pub fn scale_alpha(alpha: u8, opacity: u8) -> u8 {
    ((alpha as u16 * opacity as u16) / 255) as u8
}

pub fn with_opacity(color: u32, opacity: u8) -> u32 {
    let a = (color >> 24) as u8;
    let new_a = scale_alpha(a, opacity);
    (color & 0x00FF_FFFF) | ((new_a as u32) << 24)
}
