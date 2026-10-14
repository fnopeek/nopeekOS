//! i915_irq.c (Linux 6.18.55): interrupt install, reset, postinstall, the
//! top-level handler and uninstall, on Tiger Lake (gen11 layout).
//!
//! `request_irq` is the device's MSI through `npk_irq_register`. The
//! module is single-threaded, so the handler does not run on its own: it
//! runs inside `intel_irq_wait`, whenever the driver waits for an event.

use crate::i915_drv::drm_i915_private;
use crate::intel_display_irq::{self, intel_display_irq_state};
use crate::intel_gt::intel_gt;
use crate::intel_gt_irq::{self, intel_gt_irq_state};
use crate::intel_uncore::intel_uncore;
use crate::linux::Line;
use crate::regs::*;

/// Interrupt state of the device (`i915->irq_*`, `display->irq`).
#[derive(Clone, Copy, Default)]
pub struct i915_irq_state {
    /// The MSI vector `npk_irq_register` gave out; 0 before install.
    pub vector: i32,
    pub gt: intel_gt_irq_state,
    pub display: intel_display_irq_state,
}

/// `gen2_irq_reset`.
pub fn gen2_irq_reset(uncore: &mut intel_uncore, imr: u32, ier: u32, iir: u32) {
    uncore.intel_uncore_write(imr, 0xffffffff);
    let _ = uncore.intel_uncore_read_notrace(imr);

    uncore.intel_uncore_write(ier, 0);

    // IIR can queue up two events.
    uncore.intel_uncore_write(iir, 0xffffffff);
    let _ = uncore.intel_uncore_read_notrace(iir);
    uncore.intel_uncore_write(iir, 0xffffffff);
    let _ = uncore.intel_uncore_read_notrace(iir);
}

/// `gen2_assert_iir_is_zero`.
pub fn gen2_assert_iir_is_zero(uncore: &mut intel_uncore, reg: u32) {
    let val = uncore.intel_uncore_read(reg);
    if val == 0 {
        return;
    }
    Line::new().s("warning: Interrupt register ").hex(reg).s(" is not zero: ").hex(val).say();
    uncore.intel_uncore_write(reg, 0xffffffff);
    let _ = uncore.intel_uncore_read_notrace(reg);
    uncore.intel_uncore_write(reg, 0xffffffff);
    let _ = uncore.intel_uncore_read_notrace(reg);
}

/// `gen2_irq_init`.
pub fn gen2_irq_init(uncore: &mut intel_uncore, imr: u32, ier: u32, iir: u32, imr_val: u32, ier_val: u32) {
    gen2_assert_iir_is_zero(uncore, iir);

    uncore.intel_uncore_write(ier, ier_val);
    uncore.intel_uncore_write(imr, imr_val);
    let _ = uncore.intel_uncore_read_notrace(imr);
}

/// `gen11_master_intr_disable`: with the master off, the level
/// indications sampled now are cleared by the acks; new ones raise a new
/// interrupt once the master is back on.
fn gen11_master_intr_disable(uncore: &intel_uncore) -> u32 {
    uncore.__raw_uncore_write32(GEN11_GFX_MSTR_IRQ, 0);
    uncore.__raw_uncore_read32(GEN11_GFX_MSTR_IRQ)
}

/// `gen11_master_intr_enable`.
fn gen11_master_intr_enable(uncore: &intel_uncore) {
    uncore.__raw_uncore_write32(GEN11_GFX_MSTR_IRQ, GEN11_MASTER_IRQ as u32);
}

/// `gen11_irq_handler`: true for IRQ_HANDLED.
fn gen11_irq_handler(i915: &mut drm_i915_private) -> bool {
    if !i915.irqs_enabled {
        return false;
    }

    let master_ctl = gen11_master_intr_disable(&i915.uncore);
    if master_ctl == 0 {
        gen11_master_intr_enable(&i915.uncore);
        return false;
    }

    // Find, queue, then clear each source.
    intel_gt_irq::gen11_gt_irq_handler(i915, master_ctl);

    if master_ctl & GEN11_DISPLAY_IRQ != 0 {
        intel_display_irq::gen11_display_irq_handler(i915);
    }

    let gu_misc_iir = intel_display_irq::gen11_gu_misc_irq_ack(i915, master_ctl);

    gen11_master_intr_enable(&i915.uncore);

    intel_display_irq::gen11_gu_misc_irq_handler(i915, gu_misc_iir);

    true
}

/// `gen11_irq_reset`.
fn gen11_irq_reset(i915: &mut drm_i915_private, gt: &intel_gt) {
    gen11_master_intr_disable(&i915.uncore);

    intel_gt_irq::gen11_gt_irq_reset(i915, gt);
    intel_display_irq::gen11_display_irq_reset(i915);

    gen2_irq_reset(&mut i915.uncore, GEN11_GU_MISC_IMR, GEN11_GU_MISC_IER, GEN11_GU_MISC_IIR);
    gen2_irq_reset(&mut i915.uncore, GEN8_PCU_IMR, GEN8_PCU_IER, GEN8_PCU_IIR);
}

/// `gen11_irq_postinstall`.
fn gen11_irq_postinstall(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let gu_misc_masked = GEN11_GU_MISC_GSE;

    intel_gt_irq::gen11_gt_irq_postinstall(i915, gt);
    intel_display_irq::gen11_de_irq_postinstall(i915);

    gen2_irq_init(&mut i915.uncore, GEN11_GU_MISC_IMR, GEN11_GU_MISC_IER, GEN11_GU_MISC_IIR,
                  !gu_misc_masked, gu_misc_masked);

    gen11_master_intr_enable(&i915.uncore);
    let _ = i915.uncore.intel_uncore_read_notrace(GEN11_GFX_MSTR_IRQ);
}

/// `intel_irq_reset`: graphics version 12.0 is below 12.10, so gen11.
fn intel_irq_reset(i915: &mut drm_i915_private, gt: &intel_gt) {
    gen11_irq_reset(i915, gt);
}

/// `intel_irq_postinstall`.
fn intel_irq_postinstall(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    gen11_irq_postinstall(i915, gt);
}

/// `intel_irq_install`.
pub fn intel_irq_install(i915: &mut drm_i915_private, gt: &mut intel_gt) -> Result<(), i32> {
    const EINVAL: i32 = 22;

    // Some sources are enabled in postinstall, so interrupts count as
    // enabled before they are.
    i915.irqs_enabled = true;

    intel_irq_reset(i915, gt);

    // request_irq
    let vector = npk_sys::irq_register(0);
    if vector < 0 {
        i915.irqs_enabled = false;
        return Err(-EINVAL);
    }
    i915.irq.vector = vector;
    intel_gt_irq::gen11_gt_irq_init_engines(i915, gt);

    intel_irq_postinstall(i915, gt);

    Line::new().s("interrupts installed, MSI vector ").hex(vector as u32).say();
    Ok(())
}

/// `intel_irq_uninstall`. `free_irq`: the kernel keeps the vector with the
/// binding. `intel_hpd_cancel_work` has no work to cancel.
pub fn intel_irq_uninstall(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if !i915.irqs_enabled {
        crate::linux::say("warning: intel_irq_uninstall with interrupts not installed");
        return;
    }

    intel_irq_reset(i915, gt);

    i915.irqs_enabled = false;
}

/// Waits up to `timeout_ms` for the device's interrupt and runs the
/// handler; true if it handled something. A pending source raised before
/// the vector was armed is caught by running the handler once up front.
pub fn intel_irq_wait(i915: &mut drm_i915_private, timeout_ms: i32) -> bool {
    let vector = i915.irq.vector;
    if vector <= 0 {
        return false;
    }
    let since = npk_sys::irq_arm(vector);
    if gen11_irq_handler(i915) {
        return true;
    }
    if since < 0 {
        return false;
    }
    if npk_sys::irq_wait(vector, since, timeout_ms) == 1 {
        return gen11_irq_handler(i915);
    }
    false
}
