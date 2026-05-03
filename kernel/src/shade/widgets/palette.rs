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
    surface:          u32,
    surface_elevated: u32,
    surface_muted:    u32,
    border:           u32,
    on_surface:       u32,
    on_surface_muted: u32,
    success:          u32,
    warning:          u32,
    danger:           u32,
}

const DARK: ThemePalette = ThemePalette {
    surface:          0xFF1E1E24,
    surface_elevated: 0xFF2A2A32,
    surface_muted:    0xFF252530,
    border:           0xFF3A3A45,
    on_surface:       0xFFE0E0E8,
    on_surface_muted: 0xFF8A8A96,
    success:          0xFF4CAF50,
    warning:          0xFFFFB300,
    danger:           0xFFE74C3C,
};

const LIGHT: ThemePalette = ThemePalette {
    surface:          0xFFF5F5F7,
    surface_elevated: 0xFFFFFFFF,
    surface_muted:    0xFFEAEAEC,
    border:           0xFFD0D0D5,
    on_surface:       0xFF1A1A20,
    on_surface_muted: 0xFF606068,
    success:          0xFF2E7D32,
    warning:          0xFFE68900,
    danger:           0xFFD32F2F,
};

const DEFAULT_ACCENT: u32 = 0xFF7B50A0;

pub fn current() -> Palette {
    let mut colors = [0u32; 16];
    for i in 0..16 {
        colors[i] = resolve(token_at(i));
    }
    Palette { colors }
}

pub fn resolve(token: Token) -> u32 {
    let is_light = is_light_theme();
    let t = if is_light { &LIGHT } else { &DARK };

    match token {
        Token::Surface         => t.surface,
        Token::SurfaceElevated => t.surface_elevated,
        Token::SurfaceMuted    => t.surface_muted,
        Token::Border          => t.border,
        Token::OnSurface       => t.on_surface,
        Token::OnSurfaceMuted  => t.on_surface_muted,
        Token::Success         => t.success,
        Token::Warning         => t.warning,
        Token::Danger          => t.danger,

        Token::Accent          => accent_adjusted(t.surface),
        Token::AccentMuted     => accent_muted(t.surface),
        Token::OnAccent        => on_accent(t.surface),
    }
}

fn is_light_theme() -> bool {
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

fn accent_raw() -> u32 {
    if crate::theme::is_active() {
        crate::gui::background::accent_color() | 0xFF00_0000
    } else {
        DEFAULT_ACCENT
    }
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

/// Selected-row fill — anchored on a shifted surface so contrast is
/// guaranteed, then tinted towards accent.
fn accent_muted(surface: u32) -> u32 {
    let accent = accent_adjusted(surface);
    let base = if luminance(surface) > 128 {
        darken(surface, 0x18)
    } else {
        lighten(surface, 0x18)
    };
    blend(base, accent, 72)
}

fn on_accent(surface: u32) -> u32 {
    let accent = accent_adjusted(surface);
    if luminance(accent) > 128 { 0xFF1A1A20 } else { 0xFFFFFFFF }
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
