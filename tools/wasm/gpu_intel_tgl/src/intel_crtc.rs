//! display/intel_crtc.c (Linux 6.18.55): one crtc per pipe with its planes
//! (`intel_crtc_init`), and display/intel_modeset_setup.c's plane state
//! readout. FIFO underrun reporting, DRRS, CRC and the DRM crtc object are
//! not part of the hardware state and are not kept.

use crate::i915_drv::drm_i915_private;
use crate::intel_display;
use crate::intel_display_types::*;
use crate::linux::Line;
use crate::regs::*;

/// `drm_plane_index` of a new plane: planes are numbered in creation order.
fn add_plane(i915: &mut drm_i915_private, plane: intel_plane) -> Result<(), i32> {
    let n = i915.display.num_planes;
    if n >= MAX_PLANES {
        return Err(-(ENOMEM as i32));
    }
    i915.display.planes[n] = plane;
    i915.display.num_planes = n + 1;
    Ok(())
}

/// `intel_crtc_init` (display version 9+, not GMCH). The crtc index equals
/// the pipe, as Linux asserts.
pub fn intel_crtc_init(i915: &mut drm_i915_private, pipe: u32) -> Result<(), i32> {
    let mut crtc = intel_crtc {
        pipe,
        num_scalers: i915.display.runtime.num_scalers[pipe as usize],
        ..Default::default()
    };

    let primary = crate::skl_universal_plane::skl_universal_plane_create(i915, pipe, PLANE_1);
    crtc.plane_ids_mask |= 1 << primary.id;
    add_plane(i915, primary)?;

    for sprite in 0..i915.display.runtime.num_sprites[pipe as usize] {
        let plane = crate::skl_universal_plane::skl_universal_plane_create(i915, pipe, PLANE_2 + sprite);
        crtc.plane_ids_mask |= 1 << plane.id;
        add_plane(i915, plane)?;
    }

    let cursor = crate::intel_cursor::intel_cursor_plane_create(pipe);
    crtc.plane_ids_mask |= 1 << cursor.id;
    add_plane(i915, cursor)?;

    i915.display.crtcs[pipe as usize] = crtc;
    i915.display.crtc_valid[pipe as usize] = true;
    Ok(())
}

/// `intel_crtc_state_reset` (the HSW workaround pipe is not kept).
pub fn intel_crtc_state_reset(state: &mut intel_crtc_state) {
    *state = intel_crtc_state::default();
    state.cpu_transcoder = INVALID_TRANSCODER as u32;
    state.master_transcoder = INVALID_TRANSCODER as u32;
    state.scaler_state.scaler_id = -1;
    state.mst_master_transcoder = INVALID_TRANSCODER as u32;
}

/// The crtc part of `intel_modeset_readout_hw_state` (intel_modeset_setup.c):
/// each crtc's pipe configuration, then the planes. The encoder, DPLL and
/// connector parts that follow are another port's.
pub fn intel_modeset_readout_crtcs(i915: &mut drm_i915_private) {
    for c in 0..I915_MAX_PIPES as usize {
        if !i915.display.crtc_valid[c] {
            continue;
        }
        intel_crtc_state_reset(&mut i915.display.crtcs[c].state);

        intel_display::intel_crtc_get_pipe_config(i915, c);

        let state = &mut i915.display.crtcs[c].state;
        state.hw.enable = state.hw.active;
        Line::new().s("pipe ").dec(c as u64).s(": hw state readout: ")
            .s(if state.hw.active { "enabled" } else { "disabled" }).say();
    }

    readout_plane_state(i915);
}

/// `intel_crtc_is_joiner_secondary`.
pub fn intel_crtc_is_joiner_secondary(state: &intel_crtc_state, pipe: u32) -> bool {
    // joiner_primary_pipe: the lowest pipe of the joined set
    state.joiner_pipes != 0 && (state.joiner_pipes | (1 << pipe)).trailing_zeros() != pipe
}

/// `intel_crtc_copy_hw_to_uapi_state` (intel_modeset_setup.c), the enable
/// and active flags; the mode and LUT blobs are DRM objects.
pub fn intel_crtc_copy_hw_to_uapi_state(i915: &mut drm_i915_private, c: usize) {
    let pipe = i915.display.crtcs[c].pipe;
    let state = &mut i915.display.crtcs[c].state;
    if intel_crtc_is_joiner_secondary(state, pipe) {
        return;
    }
    state.uapi_enable = state.hw.enable;
    state.uapi_active = state.hw.active;
}

/// The primary plane (`crtc->base.primary`) of `pipe`.
pub fn primary_plane(i915: &drm_i915_private, pipe: u32) -> Option<usize> {
    (0..i915.display.num_planes).find(|&p| i915.display.planes[p].pipe == pipe && i915.display.planes[p].id == PLANE_1)
}

/// `plane->get_hw_state`.
pub fn plane_get_hw_state(i915: &mut drm_i915_private, p: usize, pipe: &mut u32) -> bool {
    let plane = i915.display.planes[p];
    match plane.kind {
        plane_kind::skl_universal => crate::skl_universal_plane::skl_plane_get_hw_state(i915, &plane, pipe),
        plane_kind::i9xx_cursor => crate::intel_cursor::i9xx_cursor_get_hw_state(i915, &plane, pipe),
    }
}

/// `readout_plane_state` (intel_modeset_setup.c).
pub fn readout_plane_state(i915: &mut drm_i915_private) {
    for p in 0..i915.display.num_planes {
        let mut pipe = PIPE_A;
        let visible = plane_get_hw_state(i915, p, &mut pipe);

        let Some(c) = intel_display::intel_crtc_for_pipe(i915, pipe) else {
            continue;
        };
        intel_display::intel_set_plane_visible(i915, c, p, visible);

        let plane = i915.display.planes[p];
        if visible {
            Line::new().s("plane ").dec(plane.id as u64 + 1).s(" pipe ").dec(pipe as u64)
                .s(": hw state readout: enabled").say();
        }
    }

    for c in 0..I915_MAX_PIPES as usize {
        if i915.display.crtc_valid[c] {
            intel_display::intel_plane_fixup_bitmasks(i915, c);
        }
    }
}
