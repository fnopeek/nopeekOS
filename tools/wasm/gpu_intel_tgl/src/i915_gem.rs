//! i915_gem.c (Linux 6.18.55): `i915_gem_init` and its teardown, the GT
//! side of the probe after the display readout.

use crate::i915_drv::drm_i915_private;
use crate::intel_ggtt::{i915_dsm, i915_ggtt};
use crate::intel_gt::intel_gt;
use crate::linux::Line;
use crate::regs::*;

/// `i915_gem_init`.
pub fn i915_gem_init(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt,
                     dsm: Option<&mut i915_dsm>) -> Result<(), i32> {
    // intel_wopcm_init_early runs from intel_gt_common_init_early in Linux;
    // gt.wopcm is first used here.
    crate::intel_wopcm::intel_wopcm_init_early(&mut gt.wopcm);

    // intel_uc_fetch_firmwares: enable_guc defaults to 0 on Tiger Lake
    // (uc_expand_default_options), so the uC ops are uc_ops_off, which has
    // no init_fw.
    crate::intel_wopcm::intel_wopcm_init(&mut gt.wopcm);
    crate::intel_gtt::setup_private_pat(i915);
    let mut pat = Line::new();
    pat.s("PAT_INDEX 0-7:");
    for i in 0..8 {
        pat.s(" ").hex(i915.uncore.intel_uncore_read(GEN12_PAT_INDEX(i)));
    }
    pat.say();

    crate::intel_ggtt::i915_init_ggtt(ggtt, &gt.wopcm)?;
    let mut nodes = Line::new();
    nodes.s("GGTT pin bias ").hex(ggtt.pin_bias as u32).s(", nodes:");
    for n in ggtt.vm_mm.nodes() {
        nodes.s(" [").hex(n.start as u32).s(" + ").hex(n.size as u32).s("]");
    }
    nodes.say();

    // Clock gating, display and GT workarounds alike: nop on TGL.
    crate::intel_clock_gating::intel_clock_gating_init(i915);

    if let Err(e) = crate::intel_gt::intel_gt_init(i915, gt, ggtt, dsm) {
        // Linux marks the GT wedged on -EIO and keeps KMS alive; the
        // display here is the firmware's and stays untouched either way.
        Line::new().s("intel_gt_init failed: ").dec((-e) as u64).say();
        return Err(e);
    }

    // intel_engines_driver_register is user-facing engine bookkeeping
    // (class B).
    Ok(())
}

/// `i915_gem_driver_remove` + `i915_gem_driver_release`: the GT is
/// quiesced before the card is given back.
pub fn i915_gem_driver_remove(i915: &mut drm_i915_private, gt: &mut intel_gt, ggtt: &mut i915_ggtt) {
    crate::intel_gt::intel_gt_driver_remove(i915, gt, ggtt);
}
