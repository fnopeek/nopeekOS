//! display/intel_fb.c and drm_fourcc.c (Linux 6.18.55): tile geometry of
//! the main colour plane on Tiger Lake (display version 12, graphics 12)
//! and the bytes per pixel of a format.

use crate::linux::Line;
use crate::regs::*;
use crate::tables;

/// `struct drm_format_info`, the members used here.
#[derive(Clone, Copy)]
pub struct drm_format_info {
    pub format: u32,
    pub cpp0: u32,
}

/// `drm_format_info(format)->cpp[0]`.
pub fn drm_format_info_cpp(format: u32) -> Option<u32> {
    tables::formats.iter().find(|f| f.format == format).map(|f| f.cpp0)
}

/// `intel_tile_size`: 4 KiB from display version 3 on.
pub fn intel_tile_size() -> u32 {
    4096
}

/// `intel_tile_width_bytes` for colour plane 0 (never a CCS aux or
/// clear-colour plane). `HAS_128_BYTE_Y_TILING` holds from graphics
/// version 3 on, except i915G/GM.
pub fn intel_tile_width_bytes(modifier: u64, cpp: u32) -> u32 {
    match modifier {
        DRM_FORMAT_MOD_LINEAR => intel_tile_size(),
        I915_FORMAT_MOD_X_TILED => 512,
        I915_FORMAT_MOD_Y_TILED_CCS
        | I915_FORMAT_MOD_Y_TILED_GEN12_RC_CCS
        | I915_FORMAT_MOD_Y_TILED_GEN12_MC_CCS
        | I915_FORMAT_MOD_Y_TILED => 128,
        I915_FORMAT_MOD_Yf_TILED_CCS | I915_FORMAT_MOD_Yf_TILED => match cpp {
            1 => 64,
            2 | 4 => 128,
            8 | 16 => 256,
            _ => {
                Line::new().s("MISSING_CASE cpp ").dec(cpp as u64).say();
                cpp
            }
        },
        _ => {
            Line::new().s("MISSING_CASE modifier ").hex((modifier >> 32) as u32).s("_")
                .hex(modifier as u32).say();
            cpp
        }
    }
}

/// `intel_tile_height`.
pub fn intel_tile_height(modifier: u64, cpp: u32) -> u32 {
    intel_tile_size() / intel_tile_width_bytes(modifier, cpp)
}

/// `intel_fb_align_height`.
pub fn intel_fb_align_height(modifier: u64, cpp: u32, height: u32) -> u32 {
    let tile_height = intel_tile_height(modifier, cpp);
    height.div_ceil(tile_height) * tile_height
}
