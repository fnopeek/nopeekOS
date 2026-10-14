//! display/intel_display_irq.c (Linux 6.18.55): the display engine's
//! interrupt registers and handler, display version 12.

use crate::i915_drv::drm_i915_private;

/// `intel_crtc_wait_for_next_vblank`.
pub fn intel_crtc_wait_for_next_vblank(_i915: &mut drm_i915_private, pipe: u32) {
    crate::linux::Line::new().s("pipe ").dec(pipe as u64).s(": vblank wait not ported").say();
}
