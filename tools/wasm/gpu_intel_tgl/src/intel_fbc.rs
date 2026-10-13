//! display/intel_fbc.c (Linux 6.18.55): `intel_fbc_sanitize`. Display
//! version 7+ uses `ivb_fbc_funcs`, whose `is_active` and `deactivate`
//! are the ILK ones; there is no dirty-rect FBC before Xe3.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::linux::Line;
use crate::regs::*;

/// `ilk_fbc_is_active`.
fn ilk_fbc_is_active(i915: &mut drm_i915_private, fbc_id: u32) -> bool {
    intel_de_read(i915, ILK_DPFC_CONTROL(fbc_id)) & DPFC_CTL_EN != 0
}

/// `ilk_fbc_deactivate`.
fn ilk_fbc_deactivate(i915: &mut drm_i915_private, fbc_id: u32) {
    // Disable compression
    let mut dpfc_ctl = intel_de_read(i915, ILK_DPFC_CONTROL(fbc_id));
    if dpfc_ctl & DPFC_CTL_EN != 0 {
        dpfc_ctl &= !DPFC_CTL_EN;
        intel_de_write(i915, ILK_DPFC_CONTROL(fbc_id), dpfc_ctl);
    }
}

/// `intel_fbc_sanitize`: FBC starts disabled, since nothing is known about
/// where in stolen memory it may be writing. The FBC instances are the
/// ones `intel_fbc_init` creates, one per bit of the runtime `fbc_mask`.
pub fn intel_fbc_sanitize(i915: &mut drm_i915_private) {
    for fbc_id in 0..I915_MAX_FBCS {
        if i915.display.runtime.fbc_mask & (1 << fbc_id) == 0 {
            continue;
        }
        if ilk_fbc_is_active(i915, fbc_id) {
            Line::new().s("FBC ").dec(fbc_id as u64).s(" was active, deactivating").say();
            ilk_fbc_deactivate(i915, fbc_id);
        }
    }
}
