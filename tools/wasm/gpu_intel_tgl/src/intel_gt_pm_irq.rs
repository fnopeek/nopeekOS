//! gt/intel_gt_pm_irq.c (Linux 6.18.55): the GT's power-management
//! interrupt enable and mask, gen11 layout (PM sits in the upper half of
//! the GPM/WGBOXPERF registers).

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::regs::*;

/// `write_pm_imr`.
fn write_pm_imr(i915: &mut drm_i915_private, gt: &intel_gt) {
    i915.uncore.intel_uncore_write(GEN11_GPM_WGBOXPERF_INTR_MASK, gt.pm_imr << 16);
}

/// `gen6_gt_pm_update_irq`.
fn gen6_gt_pm_update_irq(i915: &mut drm_i915_private, gt: &mut intel_gt, interrupt_mask: u32,
                         enabled_irq_mask: u32) {
    let mut new_val = gt.pm_imr;
    new_val &= !interrupt_mask;
    new_val |= !enabled_irq_mask & interrupt_mask;
    if new_val != gt.pm_imr {
        gt.pm_imr = new_val;
        write_pm_imr(i915, gt);
    }
}

/// `gen6_gt_pm_unmask_irq`.
pub fn gen6_gt_pm_unmask_irq(i915: &mut drm_i915_private, gt: &mut intel_gt, mask: u32) {
    gen6_gt_pm_update_irq(i915, gt, mask, mask);
}

/// `gen6_gt_pm_mask_irq`.
pub fn gen6_gt_pm_mask_irq(i915: &mut drm_i915_private, gt: &mut intel_gt, mask: u32) {
    gen6_gt_pm_update_irq(i915, gt, mask, 0);
}

/// `write_pm_ier`.
fn write_pm_ier(i915: &mut drm_i915_private, gt: &intel_gt) {
    i915.uncore.intel_uncore_write(GEN11_GPM_WGBOXPERF_INTR_ENABLE, gt.pm_ier << 16);
}

/// `gen6_gt_pm_enable_irq`.
pub fn gen6_gt_pm_enable_irq(i915: &mut drm_i915_private, gt: &mut intel_gt, enable_mask: u32) {
    gt.pm_ier |= enable_mask;
    write_pm_ier(i915, gt);
    gen6_gt_pm_unmask_irq(i915, gt, enable_mask);
}

/// `gen6_gt_pm_disable_irq`.
pub fn gen6_gt_pm_disable_irq(i915: &mut drm_i915_private, gt: &mut intel_gt, disable_mask: u32) {
    gt.pm_ier &= !disable_mask;
    gen6_gt_pm_mask_irq(i915, gt, disable_mask);
    write_pm_ier(i915, gt);
}
