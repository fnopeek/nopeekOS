//! gt/intel_gt_irq.c (Linux 6.18.55): the GT's interrupt registers and
//! handler, gen11 layout.

use crate::i915_drv::drm_i915_private;
use crate::intel_engine::has_engine;
use crate::intel_gt::intel_gt;
use crate::linux::Line;
use crate::regs::*;

const NUM_CLASSES: usize = MAX_ENGINE_CLASS as usize + 1;
const NUM_INSTANCES: usize = MAX_ENGINE_INSTANCE as usize + 1;

/// The GT part of the interrupt state.
#[derive(Clone, Copy, Default)]
pub struct intel_gt_irq_state {
    /// `gt->engine_class[class][instance] != NULL`, as an instance mask per
    /// class, taken when the interrupts are installed.
    pub engine_present: [u16; NUM_CLASSES],
    /// `intel_engine_cs_irq(engine, iir)`: the engine interrupts that came
    /// in, ORed per class and instance, until
    /// `intel_gt_requests::intel_engines_cs_irq` hands them to the engines.
    pub engine_iir: [[u16; NUM_INSTANCES]; NUM_CLASSES],
    /// `gen11_rps_irq_handler`: PM interrupts that came in for RPS, which
    /// is not ported yet.
    pub pm_iir: u16,
}

/// `intel_uc_wants_guc` / `intel_uc_wants_guc_submission`: Tiger Lake
/// runs without GuC (`enable_guc` resolves to 0 for it).
const WANTS_GUC: bool = false;

/// `HAS_HECI_GSC`.
const HAS_HECI_GSC: bool = TGL_HAS_HECI_PXP != 0 || TGL_HAS_HECI_GSCFI != 0;

/// `CCS_MASK(gt) != 0`.
fn has_ccs(gt: &intel_gt) -> bool {
    [CCS0, CCS1, CCS2, CCS3].iter().any(|&e| has_engine(gt.info.engine_mask, e))
}

/// Records which engines exist, for the identity handler.
pub fn gen11_gt_irq_init_engines(i915: &mut drm_i915_private, gt: &intel_gt) {
    let mut present = [0u16; NUM_CLASSES];
    for e in gt.engine.iter().flatten() {
        if (e.class as usize) < NUM_CLASSES && (e.instance as usize) < NUM_INSTANCES {
            present[e.class as usize] |= 1 << e.instance;
        }
    }
    i915.irq.gt.engine_present = present;
}

/// `gen11_gt_engine_identity`.
fn gen11_gt_engine_identity(i915: &mut drm_i915_private, bank: u32, bit: u32) -> u32 {
    let u = &i915.uncore;
    u.__raw_uncore_write32(GEN11_IIR_REG_SELECTOR(bank), 1 << bit);

    // Specs do not say how long to spin; ~100 us.
    let t0 = crate::linux::now_us();
    let mut ident;
    loop {
        ident = u.__raw_uncore_read32(GEN11_INTR_IDENTITY_REG(bank));
        if ident & (GEN11_INTR_DATA_VALID as u32) != 0 || crate::linux::now_us() - t0 > 100 {
            break;
        }
    }

    if ident & (GEN11_INTR_DATA_VALID as u32) == 0 {
        Line::new().s("error: INTR_IDENTITY_REG").dec(bank as u64).s(":").dec(bit as u64).s(" ")
            .hex(ident).s(" not valid!").say();
        return 0;
    }

    u.__raw_uncore_write32(GEN11_INTR_IDENTITY_REG(bank), GEN11_INTR_DATA_VALID as u32);

    ident
}

/// `gen11_other_irq_handler`. No media GT on Tiger Lake.
fn gen11_other_irq_handler(i915: &mut drm_i915_private, instance: u32, iir: u16) {
    if instance == OTHER_GUC_INSTANCE {
        // guc_irq_handler: the GuC's interrupts are never enabled here.
        return;
    }
    if instance == OTHER_GTPM_INSTANCE {
        i915.irq.gt.pm_iir |= iir;
        return;
    }
    if instance == OTHER_KCR_INSTANCE {
        Line::new().s("KCR (PXP) interrupt ").hex(iir as u32).s(": intel_pxp_irq_handler not ported").say();
        return;
    }
    if instance == OTHER_GSC_INSTANCE {
        Line::new().s("GSC interrupt ").hex(iir as u32).s(": intel_gsc_irq_handler not ported").say();
        return;
    }
    if instance == OTHER_GSC_HECI_2_INSTANCE {
        Line::new().s("GSC HECI2 interrupt ").hex(iir as u32).s(": intel_gsc_proxy_irq_handler not ported").say();
        return;
    }
    Line::new().s("warning: unhandled other interrupt instance=").hex(instance).s(", iir=").hex(iir as u32).say();
}

/// `gen11_gt_identity_handler`.
fn gen11_gt_identity_handler(i915: &mut drm_i915_private, identity: u32) {
    let class = crate::intel_gt::reg_field_get(GEN11_INTR_ENGINE_CLASS_MASK, identity);
    let instance = crate::intel_gt::reg_field_get(GEN11_INTR_ENGINE_INSTANCE_MASK, identity);
    let intr = GEN11_INTR_ENGINE_INTR(identity) as u16;

    if intr == 0 {
        return;
    }

    if class <= MAX_ENGINE_CLASS && instance <= MAX_ENGINE_INSTANCE {
        let st = &mut i915.irq.gt;
        if st.engine_present[class as usize] & (1 << instance) != 0 {
            st.engine_iir[class as usize][instance as usize] |= intr;
            return;
        }
    }

    if class == OTHER_CLASS {
        return gen11_other_irq_handler(i915, instance, intr);
    }

    Line::new().s("warning: unknown interrupt class=").hex(class).s(", instance=").hex(instance)
        .s(", intr=").hex(intr as u32).say();
}

/// `gen11_gt_bank_handler`.
fn gen11_gt_bank_handler(i915: &mut drm_i915_private, bank: u32) {
    let intr_dw = i915.uncore.__raw_uncore_read32(GEN11_GT_INTR_DW(bank));

    for bit in 0..32 {
        if intr_dw & (1 << bit) == 0 {
            continue;
        }
        let ident = gen11_gt_engine_identity(i915, bank, bit);
        gen11_gt_identity_handler(i915, ident);
    }

    // Clear must be after shared has been served for engine.
    i915.uncore.__raw_uncore_write32(GEN11_GT_INTR_DW(bank), intr_dw);
}

/// `gen11_gt_irq_handler`.
pub fn gen11_gt_irq_handler(i915: &mut drm_i915_private, master_ctl: u32) {
    for bank in 0..2 {
        if master_ctl & GEN11_GT_DW_IRQ(bank) != 0 {
            gen11_gt_bank_handler(i915, bank);
        }
    }
}

/// `gen11_gt_reset_one_iir`.
pub fn gen11_gt_reset_one_iir(i915: &mut drm_i915_private, bank: u32, bit: u32) -> bool {
    let dw = i915.uncore.__raw_uncore_read32(GEN11_GT_INTR_DW(bank));
    if dw & (1 << bit) != 0 {
        // DW_IIR bits cannot be cleared without first servicing the
        // selector and shared IIR registers.
        gen11_gt_engine_identity(i915, bank, bit);

        // Reading GT INT DW locked it; clear our bit to release it.
        i915.uncore.__raw_uncore_write32(GEN11_GT_INTR_DW(bank), 1 << bit);

        return true;
    }
    false
}

/// `gen11_gt_irq_reset`.
pub fn gen11_gt_irq_reset(i915: &mut drm_i915_private, gt: &intel_gt) {
    let m = gt.info.engine_mask;
    let u = &mut i915.uncore;

    // Disable RCS, BCS, VCS and VECS class engines.
    u.intel_uncore_write(GEN11_RENDER_COPY_INTR_ENABLE, 0);
    u.intel_uncore_write(GEN11_VCS_VECS_INTR_ENABLE, 0);
    if has_ccs(gt) {
        u.intel_uncore_write(GEN12_CCS_RSVD_INTR_ENABLE, 0);
    }
    if HAS_HECI_GSC || has_engine(m, GSC0) {
        u.intel_uncore_write(GEN11_GUNIT_CSME_INTR_ENABLE, 0);
    }

    // Restore masks irqs on RCS, BCS, VCS and VECS engines.
    u.intel_uncore_write(GEN11_RCS0_RSVD_INTR_MASK, !0);
    u.intel_uncore_write(GEN11_BCS_RSVD_INTR_MASK, !0);
    if has_engine(m, BCS1) || has_engine(m, BCS2) {
        u.intel_uncore_write(XEHPC_BCS1_BCS2_INTR_MASK, !0);
    }
    if has_engine(m, BCS3) || has_engine(m, BCS4) {
        u.intel_uncore_write(XEHPC_BCS3_BCS4_INTR_MASK, !0);
    }
    if has_engine(m, BCS5) || has_engine(m, BCS6) {
        u.intel_uncore_write(XEHPC_BCS5_BCS6_INTR_MASK, !0);
    }
    if has_engine(m, BCS7) || has_engine(m, BCS8) {
        u.intel_uncore_write(XEHPC_BCS7_BCS8_INTR_MASK, !0);
    }
    u.intel_uncore_write(GEN11_VCS0_VCS1_INTR_MASK, !0);
    u.intel_uncore_write(GEN11_VCS2_VCS3_INTR_MASK, !0);
    if has_engine(m, VCS4) || has_engine(m, VCS5) {
        u.intel_uncore_write(GEN12_VCS4_VCS5_INTR_MASK, !0);
    }
    if has_engine(m, VCS6) || has_engine(m, VCS7) {
        u.intel_uncore_write(GEN12_VCS6_VCS7_INTR_MASK, !0);
    }
    u.intel_uncore_write(GEN11_VECS0_VECS1_INTR_MASK, !0);
    if has_engine(m, VECS2) || has_engine(m, VECS3) {
        u.intel_uncore_write(GEN12_VECS2_VECS3_INTR_MASK, !0);
    }
    if has_engine(m, CCS0) || has_engine(m, CCS1) {
        u.intel_uncore_write(GEN12_CCS0_CCS1_INTR_MASK, !0);
    }
    if has_engine(m, CCS2) || has_engine(m, CCS3) {
        u.intel_uncore_write(GEN12_CCS2_CCS3_INTR_MASK, !0);
    }
    if HAS_HECI_GSC || has_engine(m, GSC0) {
        u.intel_uncore_write(GEN11_GUNIT_CSME_INTR_MASK, !0);
    }

    u.intel_uncore_write(GEN11_GPM_WGBOXPERF_INTR_ENABLE, 0);
    u.intel_uncore_write(GEN11_GPM_WGBOXPERF_INTR_MASK, !0);
    u.intel_uncore_write(GEN11_GUC_SG_INTR_ENABLE, 0);
    u.intel_uncore_write(GEN11_GUC_SG_INTR_MASK, !0);

    u.intel_uncore_write(GEN11_CRYPTO_RSVD_INTR_ENABLE, 0);
    u.intel_uncore_write(GEN11_CRYPTO_RSVD_INTR_MASK, !0);
}

/// `REG_FIELD_PREP(mask, value)`.
fn reg_field_prep(mask: u32, value: u32) -> u32 {
    (value << mask.trailing_zeros()) & mask
}

/// `gen11_gt_irq_postinstall`. No GuC, so no GuC mask; GSC0 and the HECI
/// GSC are absent on Tiger Lake (the code keeps the checks).
pub fn gen11_gt_irq_postinstall(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let m = gt.info.engine_mask;
    let mut irqs = GT_RENDER_USER_INTERRUPT;
    let mut gsc_mask = 0u32;
    let mut heci_mask = 0u32;

    if !WANTS_GUC {
        irqs |= GT_CS_MASTER_ERROR_INTERRUPT | GT_CONTEXT_SWITCH_INTERRUPT | GT_WAIT_SEMAPHORE_INTERRUPT;
    }

    let dmask = irqs << 16 | irqs;
    let smask = irqs << 16;

    if has_engine(m, GSC0) {
        // The HECI2 interrupt is enabled through the GSC's register but
        // has its own mask.
        gsc_mask = irqs;
        heci_mask = GSC_IRQ_INTF(1);
    } else if HAS_HECI_GSC {
        gsc_mask = GSC_IRQ_INTF(0) | GSC_IRQ_INTF(1);
    }

    let u = &mut i915.uncore;

    // Enable RCS, BCS, VCS and VECS class interrupts.
    u.intel_uncore_write(GEN11_RENDER_COPY_INTR_ENABLE, dmask);
    u.intel_uncore_write(GEN11_VCS_VECS_INTR_ENABLE, dmask);
    if has_ccs(gt) {
        u.intel_uncore_write(GEN12_CCS_RSVD_INTR_ENABLE, smask);
    }
    if gsc_mask != 0 {
        u.intel_uncore_write(GEN11_GUNIT_CSME_INTR_ENABLE, gsc_mask | heci_mask);
    }

    // Unmask irqs on RCS, BCS, VCS and VECS engines.
    u.intel_uncore_write(GEN11_RCS0_RSVD_INTR_MASK, !smask);
    u.intel_uncore_write(GEN11_BCS_RSVD_INTR_MASK, !smask);
    if has_engine(m, BCS1) || has_engine(m, BCS2) {
        u.intel_uncore_write(XEHPC_BCS1_BCS2_INTR_MASK, !dmask);
    }
    if has_engine(m, BCS3) || has_engine(m, BCS4) {
        u.intel_uncore_write(XEHPC_BCS3_BCS4_INTR_MASK, !dmask);
    }
    if has_engine(m, BCS5) || has_engine(m, BCS6) {
        u.intel_uncore_write(XEHPC_BCS5_BCS6_INTR_MASK, !dmask);
    }
    if has_engine(m, BCS7) || has_engine(m, BCS8) {
        u.intel_uncore_write(XEHPC_BCS7_BCS8_INTR_MASK, !dmask);
    }
    u.intel_uncore_write(GEN11_VCS0_VCS1_INTR_MASK, !dmask);
    u.intel_uncore_write(GEN11_VCS2_VCS3_INTR_MASK, !dmask);
    if has_engine(m, VCS4) || has_engine(m, VCS5) {
        u.intel_uncore_write(GEN12_VCS4_VCS5_INTR_MASK, !dmask);
    }
    if has_engine(m, VCS6) || has_engine(m, VCS7) {
        u.intel_uncore_write(GEN12_VCS6_VCS7_INTR_MASK, !dmask);
    }
    u.intel_uncore_write(GEN11_VECS0_VECS1_INTR_MASK, !dmask);
    if has_engine(m, VECS2) || has_engine(m, VECS3) {
        u.intel_uncore_write(GEN12_VECS2_VECS3_INTR_MASK, !dmask);
    }
    if has_engine(m, CCS0) || has_engine(m, CCS1) {
        u.intel_uncore_write(GEN12_CCS0_CCS1_INTR_MASK, !dmask);
    }
    if has_engine(m, CCS2) || has_engine(m, CCS3) {
        u.intel_uncore_write(GEN12_CCS2_CCS3_INTR_MASK, !dmask);
    }
    if gsc_mask != 0 {
        u.intel_uncore_write(GEN11_GUNIT_CSME_INTR_MASK, !gsc_mask);
    }
    if heci_mask != 0 {
        u.intel_uncore_write(GEN12_HECI2_RSVD_INTR_MASK, !reg_field_prep(ENGINE1_MASK, heci_mask));
    }

    // RPS interrupts are enabled and disabled with RPS itself.
    gt.pm_ier = 0;
    gt.pm_imr = !gt.pm_ier;
    let u = &mut i915.uncore;
    u.intel_uncore_write(GEN11_GPM_WGBOXPERF_INTR_ENABLE, 0);
    u.intel_uncore_write(GEN11_GPM_WGBOXPERF_INTR_MASK, !0);
}
