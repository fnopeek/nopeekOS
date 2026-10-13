//! gt/intel_reset.c (Linux 6.18.55): the full GT reset `sanitize_gpu`
//! runs, on Tiger Lake (`gen8_reset_engines`, Gen11+ domains).
//!
//! The MTL GSC workaround (Wa_14015076503) and DG2's per-engine pre-reset
//! do not apply.

use crate::i915_drv::drm_i915_private;
use crate::intel_gt::{intel_engine_cs, intel_gt};
use crate::linux::{self, Line};
use crate::regs::*;

/// `ALL_ENGINES`.
pub const ALL_ENGINES: u64 = !0;

/// `_MASKED_BIT_ENABLE`.
const fn masked_bit_enable(a: u32) -> u32 {
    (a << 16) | a
}

/// `_MASKED_BIT_DISABLE`.
const fn masked_bit_disable(a: u32) -> u32 {
    a << 16
}

/// The engines of `gt` in `mask`, by id (`for_each_engine_masked`).
fn engines_masked(gt: &intel_gt, mask: u64) -> impl Iterator<Item = intel_engine_cs> + '_ {
    gt.engine.iter().flatten().copied().filter(move |e| e.mask & mask != 0)
}

/// `gen6_hw_domain_reset`: twice below IP 12.70, then 50 us to settle.
fn gen6_hw_domain_reset(i915: &mut drm_i915_private, hw_domain_mask: u32) -> Result<(), ()> {
    let mut loops = 2;
    let mut err;
    loop {
        i915.uncore.intel_uncore_write_fw(GEN6_GDRST, hw_domain_mask);
        err = i915.uncore.__intel_wait_for_register_fw(GEN6_GDRST, hw_domain_mask, 0, 2000, 0, None);
        loops -= 1;
        if err.is_err() || loops == 0 {
            break;
        }
    }
    if err.is_err() {
        Line::new().s("wait for ").hex(hw_domain_mask).s(" engines reset failed").say();
    }
    linux::udelay(50);
    err
}

/// `struct sfc_lock_data`.
struct sfc_lock_data {
    lock_reg: u32,
    ack_reg: u32,
    usage_reg: u32,
    lock_bit: u32,
    ack_bit: u32,
    usage_bit: u32,
    reset_bit: u32,
}

fn get_sfc_forced_lock_data(engine: &intel_engine_cs) -> sfc_lock_data {
    if engine.class == VIDEO_ENHANCEMENT_CLASS {
        sfc_lock_data {
            lock_reg: GEN11_VECS_SFC_FORCED_LOCK(engine.mmio_base),
            lock_bit: GEN11_VECS_SFC_FORCED_LOCK_BIT,
            ack_reg: GEN11_VECS_SFC_LOCK_ACK(engine.mmio_base),
            ack_bit: GEN11_VECS_SFC_LOCK_ACK_BIT,
            usage_reg: GEN11_VECS_SFC_USAGE(engine.mmio_base),
            usage_bit: GEN11_VECS_SFC_USAGE_BIT,
            reset_bit: GEN11_VECS_SFC_RESET_BIT(engine.instance),
        }
    } else {
        if engine.class != VIDEO_DECODE_CLASS {
            Line::new().s("warning: missing case engine class ").dec(engine.class as u64).say();
        }
        sfc_lock_data {
            lock_reg: GEN11_VCS_SFC_FORCED_LOCK(engine.mmio_base),
            lock_bit: GEN11_VCS_SFC_FORCED_LOCK_BIT,
            ack_reg: GEN11_VCS_SFC_LOCK_STATUS(engine.mmio_base),
            ack_bit: GEN11_VCS_SFC_LOCK_ACK_BIT,
            usage_reg: GEN11_VCS_SFC_LOCK_STATUS(engine.mmio_base),
            usage_bit: GEN11_VCS_SFC_USAGE_BIT,
            reset_bit: GEN11_VCS_SFC_RESET_BIT(engine.instance),
        }
    }
}

/// `find_sfc_paired_vecs_engine`.
fn find_sfc_paired_vecs_engine(gt: &intel_gt, engine: &intel_engine_cs) -> Option<intel_engine_cs> {
    gt.engine[_VECS(engine.instance / 2) as usize]
}

/// `gen11_lock_sfc`.
fn gen11_lock_sfc(i915: &mut drm_i915_private, gt: &intel_gt, engine: &intel_engine_cs,
                  reset_mask: &mut u32, unlock_mask: &mut u64) -> Result<(), ()> {
    let vdbox_sfc_access = gt.info.vdbox_sfc_access as u8;
    match engine.class {
        VIDEO_DECODE_CLASS if (1u8 << engine.instance) & vdbox_sfc_access == 0 => return Ok(()),
        VIDEO_DECODE_CLASS | VIDEO_ENHANCEMENT_CLASS => {}
        _ => return Ok(()),
    }
    let mut sfc_lock = get_sfc_forced_lock_data(engine);
    let mut lock_to_other = false;

    if i915.uncore.intel_uncore_read_fw(sfc_lock.usage_reg) & sfc_lock.usage_bit == 0 {
        if engine.class != VIDEO_DECODE_CLASS {
            return Ok(());
        }
        // Wa_14010733141 (GRAPHICS_VER 12): VCS-HCP may hold the SFC.
        if i915.uncore.intel_uncore_read_fw(GEN12_HCP_SFC_LOCK_STATUS(engine.mmio_base))
            & GEN12_HCP_SFC_USAGE_BIT == 0 {
            return Ok(());
        }
        let Some(paired_vecs) = find_sfc_paired_vecs_engine(gt, engine) else { return Ok(()) };
        sfc_lock = get_sfc_forced_lock_data(&paired_vecs);
        lock_to_other = true;
        *unlock_mask |= paired_vecs.mask;
    } else {
        *unlock_mask |= engine.mask;
    }

    i915.uncore.intel_uncore_rmw_fw(sfc_lock.lock_reg, 0, sfc_lock.lock_bit);
    let ret = i915.uncore.__intel_wait_for_register_fw(sfc_lock.ack_reg, sfc_lock.ack_bit,
                                                       sfc_lock.ack_bit, 1000, 0, None);
    let lock_obtained = i915.uncore.intel_uncore_read_fw(sfc_lock.usage_reg) & sfc_lock.usage_bit != 0;
    if lock_obtained == lock_to_other {
        return Ok(());
    }
    if ret.is_err() {
        return ret;
    }
    *reset_mask |= sfc_lock.reset_bit;
    Ok(())
}

/// `gen11_unlock_sfc`.
fn gen11_unlock_sfc(i915: &mut drm_i915_private, gt: &intel_gt, engine: &intel_engine_cs) {
    let vdbox_sfc_access = gt.info.vdbox_sfc_access as u8;
    if engine.class != VIDEO_DECODE_CLASS && engine.class != VIDEO_ENHANCEMENT_CLASS {
        return;
    }
    if engine.class == VIDEO_DECODE_CLASS && (1u8 << engine.instance) & vdbox_sfc_access == 0 {
        return;
    }
    let sfc_lock = get_sfc_forced_lock_data(engine);
    i915.uncore.intel_uncore_rmw_fw(sfc_lock.lock_reg, sfc_lock.lock_bit, 0);
}

/// `__gen11_reset_engines`.
fn __gen11_reset_engines(i915: &mut drm_i915_private, gt: &intel_gt, engine_mask: u64) -> Result<(), ()> {
    let mut unlock_mask = 0u64;
    let mut ret = Ok(());
    let reset_mask;
    if engine_mask == ALL_ENGINES {
        reset_mask = GEN11_GRDOM_FULL;
        ret = gen6_hw_domain_reset(i915, reset_mask);
    } else {
        let mut m = 0u32;
        let mut failed = false;
        for engine in engines_masked(gt, engine_mask).collect::<heapless_vec::Engines>().iter() {
            m |= engine.reset_domain;
            if let Err(e) = gen11_lock_sfc(i915, gt, engine, &mut m, &mut unlock_mask) {
                ret = Err(e);
                failed = true;
                break;
            }
        }
        if !failed {
            ret = gen6_hw_domain_reset(i915, m);
        }
    }
    for engine in engines_masked(gt, unlock_mask).collect::<heapless_vec::Engines>().iter() {
        gen11_unlock_sfc(i915, gt, engine);
    }
    ret
}

/// `gen8_engine_reset_prepare`.
fn gen8_engine_reset_prepare(i915: &mut drm_i915_private, engine: &intel_engine_cs) -> Result<(), ()> {
    let reg = RING_RESET_CTL(engine.mmio_base);
    let mut ack = i915.uncore.intel_uncore_read_fw(reg);
    let (request, mask);
    if ack & RESET_CTL_CAT_ERROR != 0 {
        // Catastrophic error: bypass the ready-for-reset sequence (HAS#396813).
        request = RESET_CTL_CAT_ERROR;
        mask = RESET_CTL_CAT_ERROR;
        ack = 0;
    } else if ack & RESET_CTL_READY_TO_RESET == 0 {
        request = RESET_CTL_REQUEST_RESET;
        mask = RESET_CTL_READY_TO_RESET;
        ack = RESET_CTL_READY_TO_RESET;
    } else {
        return Ok(());
    }
    i915.uncore.intel_uncore_write_fw(reg, masked_bit_enable(request));
    let ret = i915.uncore.__intel_wait_for_register_fw(reg, mask, ack, 700, 0, None);
    if ret.is_err() {
        let now = i915.uncore.intel_uncore_read_fw(reg);
        Line::new().s(crate::intel_gt::intel_engine_class_repr(engine.class)).s("'")
            .dec(engine.instance as u64).s(" reset request timed out: request ").hex(request)
            .s(", RESET_CTL ").hex(now).say();
    }
    ret
}

/// `gen8_engine_reset_cancel`.
fn gen8_engine_reset_cancel(i915: &mut drm_i915_private, engine: &intel_engine_cs) {
    i915.uncore.intel_uncore_write_fw(RING_RESET_CTL(engine.mmio_base),
                                      masked_bit_disable(RESET_CTL_REQUEST_RESET));
}

/// `gen8_reset_engines`.
fn gen8_reset_engines(i915: &mut drm_i915_private, gt: &intel_gt, engine_mask: u64, retry: u32) -> Result<(), ()> {
    let reset_non_ready = retry >= 1;
    let engines = engines_masked(gt, engine_mask).collect::<heapless_vec::Engines>();
    let mut ret = Ok(());
    let mut skip = false;
    for engine in engines.iter() {
        ret = gen8_engine_reset_prepare(i915, engine);
        if ret.is_err() && !reset_non_ready {
            skip = true;
            break;
        }
    }
    if !skip {
        ret = __gen11_reset_engines(i915, gt, engine_mask);
    }
    for engine in engines.iter() {
        gen8_engine_reset_cancel(i915, engine);
    }
    ret
}

/// `__intel_gt_reset`.
fn __intel_gt_reset(i915: &mut drm_i915_private, gt: &intel_gt, engine_mask: u64) -> Result<(), ()> {
    let retries = if engine_mask == ALL_ENGINES { RESET_MAX_RETRIES } else { 1 };
    let mut ret = Err(());
    // If the power well sleeps during the reset, the request may be dropped.
    i915.uncore.intel_uncore_forcewake_get(FORCEWAKE_ALL);
    let mut retry = 0;
    while ret.is_err() && retry < retries {
        ret = gen8_reset_engines(i915, gt, engine_mask, retry);
        retry += 1;
    }
    i915.uncore.intel_uncore_forcewake_put(FORCEWAKE_ALL);
    ret
}

/// `intel_gt_reset_all_engines`.
pub fn intel_gt_reset_all_engines(i915: &mut drm_i915_private, gt: &intel_gt) -> Result<(), ()> {
    __intel_gt_reset(i915, gt, ALL_ENGINES)
}

/// `intel_gt_gpu_reset_clobbers_display`.
pub fn intel_gt_gpu_reset_clobbers_display() -> bool {
    TGL_GPU_RESET_CLOBBERS_DISPLAY != 0
}

/// A fixed-size list of engines, so the loops above can borrow `i915`
/// mutably while walking the GT's engines.
mod heapless_vec {
    use crate::intel_gt::intel_engine_cs;
    use crate::regs::I915_NUM_ENGINES;

    pub struct Engines {
        items: [Option<intel_engine_cs>; I915_NUM_ENGINES as usize],
        len: usize,
    }

    impl Engines {
        pub fn iter(&self) -> impl Iterator<Item = &intel_engine_cs> {
            self.items[..self.len].iter().flatten()
        }
    }

    impl FromIterator<intel_engine_cs> for Engines {
        fn from_iter<I: IntoIterator<Item = intel_engine_cs>>(it: I) -> Self {
            let mut e = Engines { items: [None; I915_NUM_ENGINES as usize], len: 0 };
            for x in it {
                e.items[e.len] = Some(x);
                e.len += 1;
            }
            e
        }
    }
}
