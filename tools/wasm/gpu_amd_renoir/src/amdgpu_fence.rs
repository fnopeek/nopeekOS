//! amdgpu/amdgpu_fence.c (Linux 6.18.55): the hardware fences a ring
//! signals through its writeback slot and its interrupt.
//!
//! A hardware fence is a `dma_fence` (dma_fence.rs) on the ring's context;
//! the fence driver's slots hold the outstanding ones until
//! `amdgpu_fence_process` signals them, from the interrupt or the fallback
//! timer (a deadline `workqueue::run_pending` checks).

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ring::amdgpu_irq_src_id;
use crate::dma_fence::{self, dma_fence_ops, Fence};
use crate::linux::Line;
use crate::regs::*;
use core::cell::Cell;

/// The `adev` field a fence interrupt source is.
pub fn irq_src_name(src: amdgpu_irq_src_id) -> &'static str {
    match src {
        amdgpu_irq_src_id::gfx_eop => "gfx.eop_irq",
        amdgpu_irq_src_id::kiq => "gfx.kiq.irq",
        amdgpu_irq_src_id::sdma_trap => "sdma.trap_irq",
        amdgpu_irq_src_id::vcn(_) => "vcn.inst[0].irq",
        amdgpu_irq_src_id::jpeg => "jpeg.inst[0].irq",
    }
}

/// `amdgpu_fence_read`.
fn amdgpu_fence_read(adev: &amdgpu_device, r: usize) -> u32 {
    let drv = &adev.rings[r].fence_drv;
    match (drv.cpu_addr, adev.wb.wb) {
        (Some(offs), Some(k)) => crate::amdgpu_gart::kptr_r32(adev, k, offs as u64 * 4),
        _ => drv.last_seq,
    }
}

/// `amdgpu_fence_need_ring_interrupt_restore`.
fn amdgpu_fence_need_ring_interrupt_restore(adev: &amdgpu_device, r: usize) -> bool {
    let is_gfx_power_domain = match adev.rings[r].funcs.type_ {
        // SDMA 5.x+ is part of GFX power domain so it's covered by GFXOFF
        t if t == AMDGPU_RING_TYPE_SDMA => adev.ip_version(SDMA0_HWIP, 0) >= 5 << 24,
        t if t == AMDGPU_RING_TYPE_GFX || t == AMDGPU_RING_TYPE_COMPUTE || t == AMDGPU_RING_TYPE_KIQ
            || t == AMDGPU_RING_TYPE_MES => true,
        _ => false,
    };
    !(adev.in_s0ix && is_gfx_power_domain)
}

/// `amdgpu_fence_driver_hw_init`.
pub fn amdgpu_fence_driver_hw_init(adev: &mut amdgpu_device) {
    for r in 0..adev.rings.len() {
        let drv = &adev.rings[r].fence_drv;
        if !drv.initialized {
            continue;
        }
        // enable the interrupt
        if let Some(src) = drv.irq_src {
            let t = drv.irq_type;
            if amdgpu_fence_need_ring_interrupt_restore(adev, r) {
                crate::amdgpu_irq::amdgpu_irq_get(adev, irq_src_name(src), t);
            }
        }
    }
}

/// `amdgpu_fence_schedule_fallback`: AMDGPU_FENCE_JIFFIES_TIMEOUT is half a
/// second.
fn amdgpu_fence_schedule_fallback(adev: &mut amdgpu_device, r: usize) {
    adev.rings[r].fence_drv.fallback_deadline_us = Some(crate::linux::now_us() + 500_000);
}

/// `amdgpu_fence_enable_signaling` (and `amdgpu_job_fence_enable_signaling`,
/// the same for a job's fence on its scheduler's ring).
pub fn amdgpu_fence_enable_signaling(adev: &mut amdgpu_device, r: usize) -> bool {
    if adev.rings[r].fence_drv.fallback_deadline_us.is_none() {
        amdgpu_fence_schedule_fallback(adev, r);
    }
    true
}

/// `amdgpu_fence_fallback`.
pub fn amdgpu_fence_fallback(adev: &mut amdgpu_device, r: usize) {
    if amdgpu_fence_process(adev, r) {
        let name = adev.rings[r].name.clone();
        Line::new().s("Fence fallback timer expired on ring ").s(&name).say();
    }
}

/// `amdgpu_fence_emit`: the ring's next sequence number, written by the GPU
/// with an interrupt, as a fence on the ring's context. With `job` it is
/// that job's `hw_fence` (`amdgpu_job_fence_ops`), made here as Linux
/// initializes the one embedded in the job; `af_context` is the
/// `amdgpu_fence`'s own context field.
pub fn amdgpu_fence_emit(adev: &mut amdgpu_device, r: usize, job: Option<u64>, af_context: u64, flags: u32)
                         -> Result<Fence, i32> {
    adev.rings[r].fence_drv.sync_seq = adev.rings[r].fence_drv.sync_seq.wrapping_add(1);
    let seq = adev.rings[r].fence_drv.sync_seq;
    let context = adev.fence_context + adev.rings[r].idx as u64;
    let fence = dma_fence::dma_fence_init(
        dma_fence_ops::amdgpu_fence { ring: r, wptr: Cell::new(0), context: af_context, job: job.is_some() },
        context, seq as u64);
    if let Some(j) = job {
        adev.jobs.get_mut(&j).unwrap().hw_fence = Some(fence.clone());
    }
    let addr = adev.rings[r].fence_drv.gpu_addr;
    crate::amdgpu_ring::amdgpu_ring_emit_fence(adev, r, addr, seq as u64, flags | AMDGPU_FENCE_FLAG_INT);
    amdgpu_fence_save_wptr(adev, &fence);
    let slot = (seq & adev.rings[r].fence_drv.num_fences_mask) as usize;
    if let Some(old) = adev.rings[r].fence_drv.fences[slot].clone() {
        let r2 = dma_fence::dma_fence_wait(adev, &old);
        if r2 != 0 {
            return Err(r2 as i32);
        }
    }
    adev.rings[r].fence_drv.fences[slot] = Some(fence.clone());
    Ok(fence)
}

/// `amdgpu_fence_save_wptr`.
pub fn amdgpu_fence_save_wptr(adev: &amdgpu_device, f: &Fence) {
    if let dma_fence_ops::amdgpu_fence { ring, wptr, .. } = &f.ops {
        wptr.set(adev.rings[*ring].wptr);
    }
}

/// `amdgpu_fence_emit_polling`.
pub fn amdgpu_fence_emit_polling(adev: &mut amdgpu_device, r: usize, timeout: i64) -> Result<u32, i32> {
    adev.rings[r].fence_drv.sync_seq = adev.rings[r].fence_drv.sync_seq.wrapping_add(1);
    let seq = adev.rings[r].fence_drv.sync_seq;
    let mask = adev.rings[r].fence_drv.num_fences_mask;
    if amdgpu_fence_wait_polling(adev, r, seq.wrapping_sub(mask), timeout) < 1 {
        return Err(-110);
    }
    let addr = adev.rings[r].fence_drv.gpu_addr;
    crate::amdgpu_ring::amdgpu_ring_emit_fence(adev, r, addr, seq as u64, 0);
    Ok(seq)
}

/// `amdgpu_fence_process`.
pub fn amdgpu_fence_process(adev: &mut amdgpu_device, r: usize) -> bool {
    let mut last_seq = adev.rings[r].fence_drv.last_seq;
    let mut seq = amdgpu_fence_read(adev, r);
    adev.rings[r].fence_drv.last_seq = seq;

    let pending = adev.rings[r].fence_drv.fallback_deadline_us.take().is_some();
    if pending && seq != adev.rings[r].fence_drv.sync_seq {
        amdgpu_fence_schedule_fallback(adev, r);
    }
    if seq == last_seq {
        return false;
    }
    let mask = adev.rings[r].fence_drv.num_fences_mask;
    last_seq &= mask;
    seq &= mask;
    loop {
        last_seq = (last_seq + 1) & mask;
        // There is always exactly one thread signaling this fence slot
        let fence = adev.rings[r].fence_drv.fences[last_seq as usize].take();
        if let Some(fence) = fence {
            if let dma_fence_ops::amdgpu_fence { wptr, .. } = &fence.ops {
                adev.rings[r].fence_drv.signalled_wptr = wptr.get();
            }
            dma_fence::dma_fence_signal(adev, &fence);
        }
        if last_seq == seq {
            break;
        }
    }
    true
}

/// `amdgpu_fence_wait_polling`.
pub fn amdgpu_fence_wait_polling(adev: &amdgpu_device, r: usize, wait_seq: u32, mut timeout: i64) -> i64 {
    while (wait_seq.wrapping_sub(amdgpu_fence_read(adev, r)) as i32) > 0 && timeout > 0 {
        crate::linux::udelay(2);
        timeout -= 2;
    }
    timeout.max(0)
}

/// `amdgpu_fence_wait_empty`.
fn amdgpu_fence_wait_empty(adev: &mut amdgpu_device, r: usize) -> i32 {
    let drv = &adev.rings[r].fence_drv;
    let seq = drv.sync_seq;
    if seq == 0 {
        return 0;
    }
    let Some(fence) = drv.fences[(seq & drv.num_fences_mask) as usize].clone() else { return 0 };
    dma_fence::dma_fence_wait(adev, &fence) as i32
}

/// `amdgpu_fence_driver_force_completion`.
pub fn amdgpu_fence_driver_force_completion(adev: &mut amdgpu_device, r: usize, timedout: Option<&Fence>) {
    for f in adev.rings[r].fence_drv.fences.iter().flatten() {
        if !f.test_flag(dma_fence::DMA_FENCE_FLAG_SIGNALED_BIT) {
            let e = if timedout.is_some_and(|t| alloc::rc::Rc::ptr_eq(t, f)) { -62 } else { -125 };
            dma_fence::dma_fence_set_error(f, e);
        }
    }
    // amdgpu_fence_write
    let drv = &adev.rings[r].fence_drv;
    if let (Some(offs), Some(k)) = (drv.cpu_addr, adev.wb.wb) {
        crate::amdgpu_gart::kptr_w32(adev, k, offs as u64 * 4, drv.sync_seq);
    }
    amdgpu_fence_process(adev, r);
}

/// `amdgpu_fence_driver_hw_fini`.
pub fn amdgpu_fence_driver_hw_fini(adev: &mut amdgpu_device) {
    for r in 0..adev.rings.len() {
        if !adev.rings[r].fence_drv.initialized {
            continue;
        }
        // You can't wait for HW to signal if it's gone
        let ret = amdgpu_fence_wait_empty(adev, r);
        // no need to trigger GPU reset as we are unloading
        if ret != 0 {
            amdgpu_fence_driver_force_completion(adev, r, None);
        }
        let drv = &adev.rings[r].fence_drv;
        if let Some(src) = drv.irq_src {
            let t = drv.irq_type;
            if amdgpu_fence_need_ring_interrupt_restore(adev, r) {
                crate::amdgpu_irq::amdgpu_irq_put(adev, irq_src_name(src), t);
            }
        }
        adev.rings[r].fence_drv.fallback_deadline_us = None;
    }
}
