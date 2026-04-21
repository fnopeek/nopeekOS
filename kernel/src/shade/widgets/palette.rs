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

/// Resolve a single token to its BGRA color right now.
pub fn resolve(token: Token) -> u32 {
    // Hardcoded v1 palette — dark, modern, good contrast. Every value
    // stays stable under updates (token order is frozen ABI), so apps
    // can rely on "Surface is dark, Accent is purple" semantics.
    match token {
        Token::Surface         => 0xFF1E1E24,
        Token::SurfaceElevated => 0xFF2A2A32,
        Token::SurfaceMuted    => 0xFF252530,

        Token::OnSurface       => 0xFFE0E0E8,
        Token::OnSurfaceMuted  => 0xFF8A8A96,
        Token::OnAccent        => 0xFFFFFFFF,

        Token::Accent          => 0xFF7B50A0,   // nopeekOS purple
        Token::AccentMuted     => 0xFF5A3780,

        Token::Border          => 0xFF3A3A45,
        Token::Success         => 0xFF4CAF50,
        Token::Warning         => 0xFFFFB300,
        Token::Danger          => 0xFFE74C3C,

        _ => 0xFFFF00FF,  // loud magenta — new-token-not-in-resolver hint
    }
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
