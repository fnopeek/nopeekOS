//! i915_gem.c (Linux 6.18.55): `i915_gem_init` and its teardown, the GT
//! side of the probe after the display readout.

use crate::i915_drv::drm_i915_private;
use crate::intel_ggtt::{i915_dsm, i915_ggtt};
use crate::intel_gt::intel_gt;

/// `i915_gem_init`.
pub fn i915_gem_init(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt,
                     dsm: Option<&mut i915_dsm>) -> Result<(), i32> {
    crate::linux::say("i915_gem_init: wopcm, PAT, GGTT, clock gating not ported");
    crate::intel_gt::intel_gt_init(i915, gt, ggtt, dsm)
}

/// `i915_gem_driver_remove` + `i915_gem_driver_release`: the GT is
/// quiesced before the card is given back.
pub fn i915_gem_driver_remove(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt) {
    crate::intel_gt::intel_gt_driver_remove(i915, gt, ggtt);
}
