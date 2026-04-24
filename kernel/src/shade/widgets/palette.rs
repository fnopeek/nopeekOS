//! Token → concrete BGRA color resolver.
//!
//! Widget trees declare colors as tokens (`Token::Accent` etc.); the
//! rasterizer resolves them to the current palette at raster time. For
//! P10.5 we ship hardcoded defaults that look like a modern dark UI —
//! full theme integration (`gui/theme.rs` → palette slots, wallpaper-
//! extracted accent) lands in a follow-up patch.
//!
//! Format: packed as 0xAARRGGBB (alpha in the high byte, BGRA bytes in
//! memory on little-endian). Matches `framebuffer::blit_rect` and
//! `gui/render.rs` pixel conventions.

#![allow(dead_code)]

use super::abi::{Palette, Token};

/// Build a `Palette` struct populated with the active theme colors.
/// Called once per frame — cheap (just 16 u32 copies).
pub fn current() -> Palette {
    let mut colors = [0u32; 16];
    for i in 0..16 {
        colors[i] = resolve(token_at(i));
    }
    Palette { colors }
}

/// Resolve a single token to its BGRA color right now. When the
/// wallpaper-extracted theme is active, surface + accent tokens pull
/// from the live palette so widgets follow the system look; otherwise
/// falls back to the hardcoded v1 defaults.
pub fn resolve(token: Token) -> u32 {
    if crate::theme::is_active() {
        if let Some(c) = from_live_theme(token) {
            return c;
        }
    }
    fallback(token)
}

/// Map a token to the live theme palette (16-color extracted from
/// the current wallpaper). Returns None for tokens the theme doesn't
/// drive — those fall back to hardcoded values.
///
/// Theme colors are stored as 0x00RRGGBB; we promote to 0xFFRRGGBB
/// (opaque) here. Surface uses palette[0] (darkest), Accent uses
/// `background::accent_color()` (the dominant dominant-hue slot),
/// Border uses palette[8] (bright variant of bg). Lightness-adjusted
/// muted / elevated variants are derived via lerp.
fn from_live_theme(token: Token) -> Option<u32> {
    let surface = crate::theme::bg_color() | 0xFF00_0000;
    let accent  = crate::gui::background::accent_color() | 0xFF00_0000;
    let border  = (crate::theme::border_gradient().0) | 0xFF00_0000;

    let surface_light = luminance(surface) > 128;
    let accent_light  = luminance(accent) > 128;
    let (on_surface, on_surface_muted) = if surface_light {
        (0xFF1A1A20u32, 0xFF505060u32)
    } else {
        (0xFFE0E0E8, 0xFF8A8A96)
    };
    let on_accent = if accent_light { 0xFF1A1A20 } else { 0xFFFFFFFF };

    Some(match token {
        Token::Surface         => surface,
        Token::SurfaceElevated => if surface_light { darken(surface, 0x0A) } else { lighten(surface, 0x10) },
        Token::SurfaceMuted    => if surface_light { darken(surface, 0x04) } else { lighten(surface, 0x06) },

        Token::Accent          => accent,
        Token::AccentMuted     => blend(surface, accent, 36),

        Token::Border          => border,

        Token::OnSurface       => on_surface,
        Token::OnSurfaceMuted  => on_surface_muted,
        Token::OnAccent        => on_accent,
        Token::Success         => 0xFF4CAF50,
        Token::Warning         => 0xFFFFB300,
        Token::Danger          => 0xFFE74C3C,

        _ => return None,
    })
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

/// Hardcoded fallback palette — applied when no theme is active
/// (early boot, headless tests) or for tokens the theme doesn't drive.
fn fallback(token: Token) -> u32 {
    match token {
        Token::Surface         => 0xFF1E1E24,
        Token::SurfaceElevated => 0xFF2A2A32,
        Token::SurfaceMuted    => 0xFF252530,

        Token::OnSurface       => 0xFFE0E0E8,
        Token::OnSurfaceMuted  => 0xFF8A8A96,
        Token::OnAccent        => 0xFFFFFFFF,

        Token::Accent          => 0xFF7B50A0,
        Token::AccentMuted     => 0xFF5A3780,

        Token::Border          => 0xFF3A3A45,
        Token::Success         => 0xFF4CAF50,
        Token::Warning         => 0xFFFFB300,
        Token::Danger          => 0xFFE74C3C,

        _ => 0xFFFF00FF,
    }
}

/// Shift each RGB channel up by `delta` (saturating at 0xFF). Keeps
/// alpha channel intact.
fn lighten(color: u32, delta: u8) -> u32 {
    let a =  color & 0xFF00_0000;
    let r = ((color >> 16) & 0xFF).saturating_add(delta as u32).min(0xFF);
    let g = ((color >> 8)  & 0xFF).saturating_add(delta as u32).min(0xFF);
    let b = ( color        & 0xFF).saturating_add(delta as u32).min(0xFF);
    a | (r << 16) | (g << 8) | b
}

/// Shift each RGB channel down by `delta` (saturating at 0).
fn darken(color: u32, delta: u8) -> u32 {
    let a =  color & 0xFF00_0000;
    let r = ((color >> 16) & 0xFF).saturating_sub(delta as u32);
    let g = ((color >> 8)  & 0xFF).saturating_sub(delta as u32);
    let b = ( color        & 0xFF).saturating_sub(delta as u32);
    a | (r << 16) | (g << 8) | b
}

/// Token at slot `idx` in the `Palette.colors` array. Mirrors the
/// enum's discriminant order.
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
        _  => Token::Surface,  // unused slots fall back to surface
    }
}

/// Blend an 8-bit alpha with an opacity modifier (0..=255).
pub fn scale_alpha(alpha: u8, opacity: u8) -> u8 {
    ((alpha as u16 * opacity as u16) / 255) as u8
}

/// Premultiply a BGRA color's alpha channel by `opacity` (0..=255).
pub fn with_opacity(color: u32, opacity: u8) -> u32 {
    let a = (color >> 24) as u8;
    let new_a = scale_alpha(a, opacity);
    (color & 0x00FF_FFFF) | ((new_a as u32) << 24)
}
