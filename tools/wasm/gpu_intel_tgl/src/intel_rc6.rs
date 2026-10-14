//! gt/intel_rc6.c (Linux 6.18.55): render C-state 6 on Tiger Lake
//! (gen11 programming, no GuC RC).
//!
//! `rpm_get`/`rpm_put` hold the device's runtime-PM reference while RC6 is
//! off; runtime PM is the kernel's, so only the bookkeeping is kept.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::intel_gt;
use crate::linux::Line;
use crate::regs::*;

/// `enum intel_rc6_res_type`.
pub const INTEL_RC6_RES_RC6_LOCKED: usize = 0;
pub const INTEL_RC6_RES_RC6: usize = 1;
const INTEL_RC6_RES_MAX: usize = 4;

/// `struct intel_rc6`.
#[derive(Clone, Copy, Default)]
pub struct intel_rc6 {
    pub res_reg: [u32; INTEL_RC6_RES_MAX],
    pub ctl_enable: u32,
    pub supported: bool,
    pub enabled: bool,
    pub manual: bool,
    pub wakeref: bool,
}

/// `rpm_get`.
fn rpm_get(rc6: &mut intel_rc6) {
    rc6.wakeref = true;
}

/// `rpm_put`.
fn rpm_put(rc6: &mut intel_rc6) {
    rc6.wakeref = false;
}

/// `gen11_rc6_enable`, without GuC RC: thresholds, power-gating
/// hysteresis, the RC_CONTROL value for unpark, and the power-gating
/// enables (render, media, sampler, and HCP/MFX per present VDBOX).
fn gen11_rc6_enable(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    let u = &mut i915.uncore;

    // 2b: Program RC6 thresholds.
    u.intel_uncore_write_fw(GEN6_RC6_WAKE_RATE_LIMIT, 54 << 16 | 85);
    u.intel_uncore_write_fw(GEN10_MEDIA_WAKE_RATE_LIMIT, 150);

    u.intel_uncore_write_fw(GEN6_RC_EVALUATION_INTERVAL, 125000);
    u.intel_uncore_write_fw(GEN6_RC_IDLE_HYSTERSIS, 25);
    for engine in gt.engine.iter().flatten() {
        u.intel_uncore_write_fw(RING_MAX_IDLE(engine.mmio_base), 10);
    }

    u.intel_uncore_write_fw(GUC_MAX_IDLE_COUNT, 0xA);

    u.intel_uncore_write_fw(GEN6_RC_SLEEP, 0);

    u.intel_uncore_write_fw(GEN6_RC6_THRESHOLD, 50000);

    // 2c: Program Coarse Power Gating Policies. 60 * 1280 ns, above the
    // interrupt-service latency of a CS interrupt plus a context switch.
    u.intel_uncore_write_fw(GEN9_MEDIA_PG_IDLE_HYSTERESIS, 60);
    u.intel_uncore_write_fw(GEN9_RENDER_PG_IDLE_HYSTERESIS, 60);

    // 3a: Enable RC6. intel_guc_rc_enable fails without GuC, so the
    // hardware timers drive RC6 entry.
    gt.rc6.ctl_enable = GEN6_RC_CTL_HW_ENABLE as u32 | GEN6_RC_CTL_RC6_ENABLE | GEN6_RC_CTL_EI_MODE(1);

    let mut pg_enable = GEN9_RENDER_PG_ENABLE | GEN9_MEDIA_PG_ENABLE | GEN11_MEDIA_SAMPLER_PG_ENABLE;

    // GRAPHICS_VER >= 12 and not DG1.
    for i in 0..I915_MAX_VCS {
        if crate::intel_engine::has_engine(gt.info.engine_mask, _VCS(i)) {
            pg_enable |= VDN_HCP_POWERGATE_ENABLE(i) | VDN_MFX_POWERGATE_ENABLE(i);
        }
    }

    u.intel_uncore_write_fw(GEN9_PG_ENABLE, pg_enable);
}

/// `rc6_supported`: HAS_RC6, no vGPU, no mock GT; the BXT, MTL and media
/// A-step checks are for other platforms.
fn rc6_supported() -> bool {
    TGL_HAS_RC6 != 0
}

/// `pctx_corrupted`: NEEDS_RC6_CTX_CORRUPTION_WA is GRAPHICS_VER 9 only.
fn pctx_corrupted() -> bool {
    false
}

/// `rc6_res_reg_init` for the render GT.
fn rc6_res_reg_init(rc6: &mut intel_rc6) {
    rc6.res_reg = [0; INTEL_RC6_RES_MAX];
    rc6.res_reg[INTEL_RC6_RES_RC6_LOCKED] = GEN6_GT_GFX_RC6_LOCKED;
    rc6.res_reg[INTEL_RC6_RES_RC6] = GEN6_GT_GFX_RC6;
}

/// `__intel_rc6_disable`. `intel_guc_rc_disable` has nothing to take back.
fn __intel_rc6_disable(i915: &mut drm_i915_private) {
    let u = &mut i915.uncore;
    u.intel_uncore_forcewake_get(FORCEWAKE_ALL);
    u.intel_uncore_write_fw(GEN9_PG_ENABLE, 0);
    u.intel_uncore_write_fw(GEN6_RC_CONTROL, 0);
    u.intel_uncore_write_fw(GEN6_RC_STATE, 0);
    u.intel_uncore_forcewake_put(FORCEWAKE_ALL);
}

/// `intel_rc6_init`: runtime PM stays held until RC6 is enabled, and RC6
/// is disabled until then. No power context on TGL (VLV/CHV only).
pub fn intel_rc6_init(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    rpm_get(&mut gt.rc6);

    if !rc6_supported() {
        return;
    }

    rc6_res_reg_init(&mut gt.rc6);

    // Sanitize rc6, ensure it is disabled before we are ready.
    __intel_rc6_disable(i915);

    gt.rc6.supported = true;
}

/// `intel_rc6_sanitize`. The residency snapshot (`prev_hw_residency`)
/// belongs to the sysfs counters, which are not kept.
pub fn intel_rc6_sanitize(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if gt.rc6.enabled {
        rpm_get(&mut gt.rc6);
        gt.rc6.enabled = false;
    }

    if gt.rc6.supported {
        __intel_rc6_disable(i915);
    }
}

/// `intel_rc6_enable`.
pub fn intel_rc6_enable(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if !gt.rc6.supported {
        return;
    }

    i915.uncore.intel_uncore_forcewake_get(FORCEWAKE_ALL);

    gen11_rc6_enable(i915, gt);

    gt.rc6.manual = gt.rc6.ctl_enable & GEN6_RC_CTL_RC6_ENABLE != 0;

    i915.uncore.intel_uncore_forcewake_put(FORCEWAKE_ALL);

    if pctx_corrupted() {
        return;
    }

    // rc6 is ready, runtime-pm is go!
    rpm_put(&mut gt.rc6);
    gt.rc6.enabled = true;
}

/// `intel_rc6_unpark`: restore the hardware timers for automatic RC6 entry
/// while busy.
pub fn intel_rc6_unpark(i915: &mut drm_i915_private, gt: &intel_gt) {
    if !gt.rc6.enabled {
        return;
    }
    i915.uncore.intel_uncore_write_fw(GEN6_RC_CONTROL, gt.rc6.ctl_enable);
}

/// `intel_rc6_park`: turn off the hardware timers and go directly to RC6
/// (TGL has neither RC6p nor RC6pp, so the target is normal RC6).
pub fn intel_rc6_park(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if !gt.rc6.enabled {
        return;
    }

    if pctx_corrupted() {
        intel_rc6_disable(i915, gt);
        return;
    }

    if !gt.rc6.manual {
        return;
    }

    i915.uncore.intel_uncore_write_fw(GEN6_RC_CONTROL, GEN6_RC_CTL_RC6_ENABLE);

    let target: u32 = if TGL_HAS_RC6P != 0 { 0x5 } else { 0x4 };
    i915.uncore.intel_uncore_write_fw(GEN6_RC_STATE, target << RC_SW_TARGET_STATE_SHIFT);
}

/// `intel_rc6_disable`.
pub fn intel_rc6_disable(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    if !gt.rc6.enabled {
        return;
    }

    rpm_get(&mut gt.rc6);
    gt.rc6.enabled = false;

    __intel_rc6_disable(i915);
}

/// `intel_rc6_fini`. No power context to free on TGL; the BIOS C6 state
/// restore is MTL's.
pub fn intel_rc6_fini(i915: &mut drm_i915_private, gt: &mut intel_gt) {
    intel_rc6_disable(i915, gt);
    if gt.rc6.wakeref {
        rpm_put(&mut gt.rc6);
    }
}

/// RC_CONTROL, RC_STATE and the RC6 residency counter, for the log.
pub fn report(i915: &mut drm_i915_private, gt: &intel_gt, when: &str) {
    let u = &mut i915.uncore;
    let ctl = u.intel_uncore_read(GEN6_RC_CONTROL);
    let state = u.intel_uncore_read(GEN6_RC_STATE);
    let res = if gt.rc6.supported { u.intel_uncore_read(gt.rc6.res_reg[INTEL_RC6_RES_RC6]) } else { 0 };
    Line::new().s("RC6 ").s(when).s(": supported ").dec(gt.rc6.supported as u64).s(", enabled ")
        .dec(gt.rc6.enabled as u64).s(", RC_CONTROL ").hex(ctl).s(", RC_STATE ").hex(state)
        .s(", residency ").hex(res).say();
}
