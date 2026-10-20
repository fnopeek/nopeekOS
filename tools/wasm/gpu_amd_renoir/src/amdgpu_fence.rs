//! amdgpu/amdgpu_fence.c (Linux 6.18.55): the hardware fences a ring
//! signals through its writeback slot and its interrupt.
//!
//! A `dma_fence` here is its ring and sequence number; it is signaled once
//! `amdgpu_fence_process` has moved the ring's `last_seq` past it. Waiting
//! runs the interrupt handler (`amdgpu_irq_wait`), and the fallback timer
//! is a deadline checked by the wait.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ring::amdgpu_irq_src_id;
use crate::linux::Line;
use crate::regs::*;

/// A hardware fence: `(ring, seq)`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct dma_fence {
    pub ring: usize,
    pub seq: u32,
}

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

/// `amdgpu_fence_emit` for a fence of its own (no job): the ring's next
/// sequence number, written by the GPU with an interrupt.
pub fn amdgpu_fence_emit(adev: &mut amdgpu_device, r: usize, flags: u32) -> Result<dma_fence, i32> {
    adev.rings[r].fence_drv.sync_seq = adev.rings[r].fence_drv.sync_seq.wrapping_add(1);
    let seq = adev.rings[r].fence_drv.sync_seq;
    let addr = adev.rings[r].fence_drv.gpu_addr;
    crate::amdgpu_ring::amdgpu_ring_emit_fence(adev, r, addr, seq as u64, flags | AMDGPU_FENCE_FLAG_INT);
    let drv = &adev.rings[r].fence_drv;
    let slot = (seq & drv.num_fences_mask) as usize;
    if let Some(old) = drv.fences[slot] {
        // the slot still holds a fence: wait for it
        let r2 = dma_fence_wait(adev, dma_fence { ring: r, seq: old }, i64::MAX);
        if r2 < 0 {
            return Err(r2 as i32);
        }
    }
    adev.rings[r].fence_drv.fences[slot] = Some(seq);
    Ok(dma_fence { ring: r, seq })
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
    let drv = &mut adev.rings[r].fence_drv;
    last_seq &= drv.num_fences_mask;
    seq &= drv.num_fences_mask;
    loop {
        last_seq = (last_seq + 1) & drv.num_fences_mask;
        // There is always exactly one thread signaling this fence slot:
        // taking it out is dma_fence_signal.
        drv.fences[last_seq as usize] = None;
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

/// `dma_fence_is_signaled`.
pub fn dma_fence_is_signaled(adev: &amdgpu_device, f: dma_fence) -> bool {
    (adev.rings[f.ring].fence_drv.last_seq.wrapping_sub(f.seq) as i32) >= 0
}

/// `dma_fence_wait_timeout` (`timeout` in microseconds, `i64::MAX` for
/// none): enables signaling, which arms the fallback timer, then lets the
/// interrupt handler run until the fence is signaled. Returns the time
/// left, at least 1, or 0 on timeout.
pub fn dma_fence_wait_timeout(adev: &mut amdgpu_device, f: dma_fence, timeout: i64) -> i64 {
    if dma_fence_is_signaled(adev, f) {
        return timeout.max(1);
    }
    // amdgpu_fence_enable_signaling
    if adev.rings[f.ring].fence_drv.fallback_deadline_us.is_none() {
        amdgpu_fence_schedule_fallback(adev, f.ring);
    }
    let t0 = crate::linux::now_us();
    loop {
        let elapsed = (crate::linux::now_us() - t0) as i64;
        if dma_fence_is_signaled(adev, f) {
            return (timeout - elapsed).max(1);
        }
        if elapsed >= timeout {
            return 0;
        }
        let left_ms = ((timeout - elapsed) / 1000).clamp(1, 10) as i32;
        crate::amdgpu_irq::amdgpu_irq_wait(adev, left_ms);
        // amdgpu_fence_fallback
        if adev.rings[f.ring].fence_drv.fallback_deadline_us.is_some_and(|d| crate::linux::now_us() >= d) {
            adev.rings[f.ring].fence_drv.fallback_deadline_us = None;
            if amdgpu_fence_process(adev, f.ring) {
                Line::new().s("Fence fallback timer expired on ring ").s(&adev.rings[f.ring].name.clone()).say();
            }
        }
    }
}

/// `dma_fence_wait`: uninterruptible, no timeout.
pub fn dma_fence_wait(adev: &mut amdgpu_device, f: dma_fence, timeout: i64) -> i64 {
    if dma_fence_wait_timeout(adev, f, timeout) > 0 { 0 } else { -110 }
}

/// `amdgpu_fence_wait_empty`.
fn amdgpu_fence_wait_empty(adev: &mut amdgpu_device, r: usize) -> i32 {
    let drv = &adev.rings[r].fence_drv;
    let seq = drv.sync_seq;
    if seq == 0 {
        return 0;
    }
    let Some(s) = drv.fences[(seq & drv.num_fences_mask) as usize] else { return 0 };
    dma_fence_wait(adev, dma_fence { ring: r, seq: s }, i64::MAX) as i32
}

/// `amdgpu_fence_driver_force_completion`: write the last emitted
/// sequence and process.
fn amdgpu_fence_driver_force_completion(adev: &mut amdgpu_device, r: usize) {
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
        let ret = amdgpu_fence_wait_empty(adev, r);
        // no need to trigger GPU reset as we are unloading
        if ret != 0 {
            amdgpu_fence_driver_force_completion(adev, r);
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
