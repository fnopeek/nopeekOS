//! display/intel_plane_initial.c (Linux 6.18.55): reading the firmware's
//! framebuffer off each active pipe (`intel_initial_plane_config`), up to
//! the description of the framebuffer. Wrapping it into a GEM object and a
//! GGTT VMA (`intel_find_initial_plane_obj`) is not part of this.

use crate::i915_drv::drm_i915_private;
use crate::regs::*;

/// The framebuffer the plane scans out (`plane_config->fb`).
#[derive(Clone, Copy, Default)]
pub struct intel_initial_fb {
    pub width: u32,
    pub height: u32,
    /// `fb->pitches[0]`, bytes.
    pub pitch: u32,
    /// DRM fourcc.
    pub format: u32,
    /// Bytes per pixel of colour plane 0.
    pub cpp: u32,
    pub modifier: u64,
}

/// `struct intel_initial_plane_config`.
#[derive(Clone, Copy, Default)]
pub struct intel_initial_plane_config {
    pub fb: Option<intel_initial_fb>,
    /// `PLANE_SURF`: the GGTT offset the plane scans out from.
    pub base: u32,
    /// Bytes: pitch times the tile-aligned height.
    pub size: u32,
    pub tiling: u32,
    pub rotation: u32,
}

/// `intel_initial_plane_config`, the readout per active crtc
/// (`get_initial_plane_config` is `skl_get_initial_plane_config` from
/// display version 9 on).
pub fn intel_initial_plane_config(i915: &mut drm_i915_private) -> [intel_initial_plane_config; I915_MAX_PIPES as usize] {
    let mut plane_configs = [intel_initial_plane_config::default(); I915_MAX_PIPES as usize];

    for c in 0..I915_MAX_PIPES as usize {
        if !i915.display.crtc_valid[c] || !i915.display.crtcs[c].state.uapi_active {
            continue;
        }
        let pipe = i915.display.crtcs[c].pipe as usize;
        crate::skl_universal_plane::skl_get_initial_plane_config(i915, c, &mut plane_configs[pipe]);
    }
    plane_configs
}
