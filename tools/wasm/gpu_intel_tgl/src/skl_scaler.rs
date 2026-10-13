//! display/skl_scaler.c (Linux 6.18.55): the pipe scaler (panel fitter)
//! readout.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::intel_display::REG_FIELD_GET;
use crate::intel_display_types::drm_rect;
use crate::regs::*;

/// `skl_scaler_get_config`.
pub fn skl_scaler_get_config(i915: &mut drm_i915_private, c: usize) {
    let pipe = i915.display.crtcs[c].pipe;
    let num_scalers = i915.display.crtcs[c].num_scalers;
    let mut id = -1;

    // find scaler attached to this pipe
    for i in 0..num_scalers {
        let ctl = intel_de_read(i915, SKL_PS_CTRL(pipe, i));
        if ctl & (PS_SCALER_EN | PS_BINDING_MASK) != (PS_SCALER_EN | PS_BINDING_PIPE) {
            continue;
        }

        id = i as i32;
        let pos = intel_de_read(i915, SKL_PS_WIN_POS(pipe, i));
        let size = intel_de_read(i915, SKL_PS_WIN_SZ(pipe, i));

        let state = &mut i915.display.crtcs[c].state;
        state.pch_pfit.enabled = true;
        state.pch_pfit.dst = drm_rect::init(REG_FIELD_GET(PS_WIN_XPOS_MASK, pos) as i32,
                                            REG_FIELD_GET(PS_WIN_YPOS_MASK, pos) as i32,
                                            REG_FIELD_GET(PS_WIN_XSIZE_MASK, size) as i32,
                                            REG_FIELD_GET(PS_WIN_YSIZE_MASK, size) as i32);
        state.scaler_state.scalers[i as usize].in_use = true;
        break;
    }

    let scaler_state = &mut i915.display.crtcs[c].state.scaler_state;
    scaler_state.scaler_id = id;
    if id >= 0 {
        scaler_state.scaler_users |= 1 << SKL_CRTC_INDEX;
    } else {
        scaler_state.scaler_users &= !(1 << SKL_CRTC_INDEX);
    }
}
