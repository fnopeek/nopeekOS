//! Off-vCPU GPU backend for the microvm (vhost/virtio-gpu-style).
//!
//! Mirrors `net_backend`: moves the virtio-gpu device OUT of `VmShared` so a
//! dedicated GPU worker fiber can drain the controlq + do the ~8 MB framebuffer
//! copy + `write_frame` on ITS OWN core, while the vCPU only notes the controlq
//! doorbell (a cheap exit). The inline copy on the vCPU exit was the browser's
//! net-throttle root: cage renders the UI → constant TRANSFER_TO_HOST_2D + FLUSH
//! → 8 MB/frame copy on the same vCPU that services the net → bufferbloat + the
//! ugly framerate-throttle workaround. Off-vCPU removes the contention entirely.
//!
//! Stage 1 (this commit) is behavior-neutral: the device is still serviced inline
//! from the vCPU exit handlers, they just reach it through this lock instead of
//! `sh.pci.virtio_gpu`. Stage 2 wires the doorbell-defer + the worker fiber.
//!
//! Why out of VmShared: the GPU copy reads guest pages (`guest_mem::active()`,
//! already `&self`) and writes the compositor surface (global `shade::surface`).
//! Only the device STATE needed to move out so the worker can hold it across
//! cores without aliasing the vCPU's `&mut VmShared`. Lock order: a vCPU takes
//! `VM_BIG_LOCK` then this lock; the worker takes ONLY this lock — no cycle.

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};
use super::virtio_gpu_pci::{VirtioGpu, BAR0_BASE};

static GPU: Mutex<VirtioGpu> = Mutex::new(VirtioGpu::new());

/// Acquire the GPU device. The vCPU takes this only AFTER `VM_BIG_LOCK`; the
/// worker takes ONLY this — lock order is acyclic.
pub fn lock() -> MutexGuard<'static, VirtioGpu> { GPU.lock() }

/// Reset the device on VM teardown/start (alongside `net_backend::reset`).
pub fn reset() { *GPU.lock() = VirtioGpu::new(); }

/// Lock-free BAR0 range check (const base) — the vCPU NPF dispatch tests this on
/// every MMIO exit, so keep it off the device lock (mirror of net_backend).
#[inline]
pub fn bar0_in_range(gpa: u64) -> bool {
    gpa >= BAR0_BASE && gpa < BAR0_BASE + 0x4000
}

// ── Stage 2 scaffolding (inert in Stage 1): controlq doorbell defer ──
/// Pending controlq notify from the vCPU. 0xFFFF = none. On the doorbell the vCPU
/// sets the qidx here (instead of servicing inline) + wakes the worker's core;
/// the GPU worker drains it on its own core.
static GPU_KICK: AtomicU16 = AtomicU16::new(0xFFFF);
static WORKER_CORE: AtomicUsize = AtomicUsize::new(usize::MAX);
static FULL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Compile-time gate for the off-vCPU GPU worker (clean OTA rollback).
pub const FULL_GPU_BACKEND: bool = true;
#[inline]
pub fn full_active() -> bool { FULL_ACTIVE.load(Ordering::Acquire) }
pub fn set_full_active(on: bool) { FULL_ACTIVE.store(on, Ordering::Release); }
pub fn set_worker_core(core: usize) { WORKER_CORE.store(core, Ordering::Release); }

/// vCPU: the guest notified the controlq (`qidx`). Defer to the worker and, on the
/// empty→set edge, wake its core out of HLT (coalesced like the net TX kick).
pub fn note_gpu_kick(qidx: u16) {
    if GPU_KICK.swap(qidx, Ordering::AcqRel) == 0xFFFF {
        let c = WORKER_CORE.load(Ordering::Acquire);
        if c != usize::MAX { crate::smp::kick_host_core(c); }
    }
}

/// Worker: take the pending controlq qidx (clears it). `Some(q)` ⇒ service it.
pub fn take_gpu_kick() -> Option<u16> {
    let q = GPU_KICK.swap(0xFFFF, Ordering::AcqRel);
    if q == 0xFFFF { None } else { Some(q) }
}
