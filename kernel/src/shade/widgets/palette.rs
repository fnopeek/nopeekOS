//! Token → concrete BGRA color.
//!
//! Two curated palettes (DARK + LIGHT) with fixed surface/border/text
//! values. Only the Accent and AccentMuted tokens derive from the
//! wallpaper's extracted theme. The mode is picked from the
//! `theme` config key (`dark` | `light` | `auto`). `auto` uses the
//! wallpaper's background luminance to decide.

#![allow(dead_code)]

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
    crate::config::get("shade.chrome_opacity")
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|v| v.min(255))
        .unwrap_or(dflt)
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

fn token_at(idx: usize) -> Token {
    match idx {
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
        _  => Token::Surface,
    }
}

pub fn scale_alpha(alpha: u8, opacity: u8) -> u8 {
    ((alpha as u16 * opacity as u16) / 255) as u8
}

pub fn with_opacity(color: u32, opacity: u8) -> u32 {
    let a = (color >> 24) as u8;
    let new_a = scale_alpha(a, opacity);
    (color & 0x00FF_FFFF) | ((new_a as u32) << 24)
}
