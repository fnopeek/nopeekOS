//! The UI scale factor.
//!
//! Apps write logical px; layout and raster work in physical px. Every
//! logical length crosses into physical through `len` (or `font`), so the
//! factor lives in exactly one place.
//!
//! The factor is in 1/120 units, after Wayland `fractional-scale-v1`:
//! 120 = 1.0, 150 = 1.25, 180 = 1.5, 240 = 2.0.

use core::sync::atomic::{AtomicU32, Ordering};

pub const UNIT: u32 = 120;
pub const MIN: u32 = UNIT;
pub const MAX: u32 = 3 * UNIT;

static FACTOR: AtomicU32 = AtomicU32::new(UNIT);

pub fn get() -> u32 { FACTOR.load(Ordering::Relaxed) }

pub fn set(units: u32) { FACTOR.store(units.clamp(MIN, MAX), Ordering::Relaxed); }

/// Default for a screen `width` px wide: 2.0 for 4K-class panels, else 1.0.
/// Same threshold as `gui::font::scale_for`.
pub fn default_for(width: u32) -> u32 {
    if width > 2560 { 2 * UNIT } else { UNIT }
}

/// Parse a `shade.scale` value such as `1.25`, rounded to the 1/120 grid.
/// `None` for anything that is not a number in range.
pub fn parse(s: &str) -> Option<u32> {
    let v: f32 = s.trim().parse().ok()?;
    if !v.is_finite() { return None; }
    let units = (v * UNIT as f32 + 0.5) as u32;
    (MIN..=MAX).contains(&units).then_some(units)
}

/// Boot-time factor: `shade.scale` when set and valid, else the default
/// for the screen width.
pub fn init(screen_w: u32) {
    let units = crate::config::get("shade.scale")
        .and_then(|s| parse(&s))
        .unwrap_or_else(|| default_for(screen_w));
    set(units);
}

/// Logical length → physical px, rounded. Since the factor is at least
/// 1.0, a non-zero length never rounds to zero: a 1 px hairline stays
/// visible at every factor.
pub fn len(v: u32) -> u32 {
    ((v as u64 * get() as u64 + (UNIT / 2) as u64) / UNIT as u64).min(u32::MAX as u64) as u32
}

/// `len` for a signed offset (round half away from zero).
pub fn len_i(v: i32) -> i32 {
    let m = len(v.unsigned_abs()).min(i32::MAX as u32) as i32;
    if v < 0 { -m } else { m }
}

/// `len` for the u8 fields of the wire format (radii, border widths).
pub fn len_u8(v: u8) -> u8 { len(v as u32).min(u8::MAX as u32) as u8 }

/// A logical font size → the physical px it is shaped and drawn at.
pub fn font(px: u16) -> u16 { len(px as u32).clamp(1, u16::MAX as u32) as u16 }

/// Physical px → logical, rounded. For decisions that must not change with
/// the factor (density classes) and for sizes reported back to apps.
pub fn logical(v: u32) -> u32 {
    ((v as u64 * UNIT as u64 + (get() / 2) as u64) / get() as u64) as u32
}
