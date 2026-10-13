//! display/intel_vrr.c (Linux 6.18.55): the VRR readout of display version
//! 12 (no CMRR, no adaptive sync SDP, no forced VRR timing generator).

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::intel_display::REG_FIELD_GET;
use crate::regs::*;

/// `intel_vrr_get_config`.
pub fn intel_vrr_get_config(i915: &mut drm_i915_private, c: usize) {
    let cpu_transcoder = i915.display.crtcs[c].state.cpu_transcoder;
    let trans_vrr_ctl = intel_de_read(i915, TRANS_VRR_CTL(cpu_transcoder));

    if trans_vrr_ctl & VRR_CTL_PIPELINE_FULL_OVERRIDE != 0 {
        i915.display.crtcs[c].state.vrr.pipeline_full =
            REG_FIELD_GET(VRR_CTL_PIPELINE_FULL_MASK, trans_vrr_ctl) as u8;
    }

    if trans_vrr_ctl & VRR_CTL_FLIP_LINE_EN != 0 {
        let flipline = intel_de_read(i915, TRANS_VRR_FLIPLINE(cpu_transcoder)) + 1;
        let vmax = intel_de_read(i915, TRANS_VRR_VMAX(cpu_transcoder)) + 1;
        let vmin = intel_de_read(i915, TRANS_VRR_VMIN(cpu_transcoder)) + 1;
        let vrr = &mut i915.display.crtcs[c].state.vrr;
        vrr.flipline = flipline as u16;
        vrr.vmax = vmax as u16;
        vrr.vmin = vmin as u16;
    }

    let state = &mut i915.display.crtcs[c].state;
    state.vrr.enable = trans_vrr_ctl & VRR_CTL_VRR_ENABLE != 0;

    if state.vrr.enable {
        state.mode_flags |= I915_MODE_FLAG_VRR;
    }
}
