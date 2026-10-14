//! gt/intel_gtt.c (Linux 6.18.55): the private PAT of Tiger Lake.

use crate::i915_drv::drm_i915_private;
use crate::regs::*;

/// `tgl_setup_private_ppat`: TGL doesn't support LLC or AGE settings.
fn tgl_setup_private_ppat(i915: &mut drm_i915_private) {
    const PAT: [u32; 8] = [GEN8_PPAT_WB, GEN8_PPAT_WC, GEN8_PPAT_WT, GEN8_PPAT_UC,
                           GEN8_PPAT_WB, GEN8_PPAT_WB, GEN8_PPAT_WB, GEN8_PPAT_WB];
    for (i, v) in PAT.iter().enumerate() {
        i915.uncore.intel_uncore_write(GEN12_PAT_INDEX(i as u32), *v);
    }
}

/// `setup_private_pat`: the primary GT of graphics version 12.0.
pub fn setup_private_pat(i915: &mut drm_i915_private) {
    tgl_setup_private_ppat(i915);
}
