//! gt/intel_tlb.c (Linux 6.18.55): the full GPU TLB invalidation by MMIO
//! (`mmio_invalidate_full`), done after PTEs are cleared and before the
//! pages behind them go back. Without the seqno barrier of
//! `intel_gt_invalidate_tlb_full`: the caller invalidates once per batch
//! of unbinds. The module is single-threaded, so the MCR and uncore locks
//! have nothing to serialise.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::linux::Line;
use crate::regs::*;

/// `GEN12_OA_TLB_INV_CR` (i915_perf_oa_regs.h:93).
const GEN12_OA_TLB_INV_CR: u32 = 0xceec;
const TLB_INVAL_TIMEOUT_US: u32 = 100;
const TLB_INVAL_TIMEOUT_MS: u32 = 4;

/// `intel_gt_invalidate_tlb_full`: every awake engine drops its TLB.
/// Table pages taken out of a tree go back once it is done; a parked or
/// wedged GT keeps no translation.
pub fn intel_gt_invalidate_tlb_full(i915: &mut drm_i915_private, gt: &intel_gt) {
    if crate::intel_gt::intel_gt_is_wedged(gt) || !crate::intel_gt_pm::intel_gt_pm_is_awake(gt) {
        crate::gen8_ppgtt::release_retired();
        return;
    }
    let mut done = true;
    let uncore = &mut i915.uncore;
    uncore.intel_uncore_forcewake_get(FORCEWAKE_ALL);
    let mut awake = 0u64;
    for (id, e) in gt.engine.iter().enumerate() {
        let Some(e) = e else { continue };
        if e.wakeref_count == 0 {
            continue;
        }
        uncore.intel_uncore_write_fw(e.tlb_inv.reg, e.tlb_inv.request);
        awake |= 1 << id;
    }
    // Wa_2207587034:tgl
    if awake != 0 {
        uncore.intel_uncore_write_fw(GEN12_OA_TLB_INV_CR, 1);
    }
    for (id, e) in gt.engine.iter().enumerate() {
        let Some(e) = e else { continue };
        if awake & (1 << id) == 0 {
            continue;
        }
        if uncore
            .__intel_wait_for_register_fw(e.tlb_inv.reg, e.tlb_inv.done, 0, TLB_INVAL_TIMEOUT_US,
                                          TLB_INVAL_TIMEOUT_MS, None)
            .is_err()
        {
            Line::new().s("engine ").dec(id as u64).s(" TLB invalidation did not complete in ")
                .dec(TLB_INVAL_TIMEOUT_MS as u64).s(" ms").say();
            done = false;
        }
    }
    uncore.intel_uncore_forcewake_put_delayed(FORCEWAKE_ALL);
    if done {
        crate::gen8_ppgtt::release_retired();
    }
}
