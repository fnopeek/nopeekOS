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

/// Guest GPU completion IRQ (line 9), raised by the worker after it advanced the
/// used-ring, folded into the BSP's `pending_irqs` on its next exit (mirror of the
/// net IRQ10 path). Lock-free so the worker needs no `VmShared` borrow.
static GPU_IRQ_PENDING: AtomicBool = AtomicBool::new(false);
#[inline]
pub fn raise_irq() { GPU_IRQ_PENDING.store(true, Ordering::Release); }
/// BSP: take the pending GPU IRQ (clears it). True ⇒ fold IRQ9 into pending_irqs.
#[inline]
pub fn take_irq() -> bool { GPU_IRQ_PENDING.swap(false, Ordering::AcqRel) }

// ── The off-vCPU GPU worker fiber (Stage 2) ──
static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// service_queues does the ~8 MB framebuffer copy + write_frame; give it a roomy
/// fiber stack (the default 128 KiB has no guard page).
const WORKER_STACK_BYTES: usize = 256 * 1024;

/// Spawn the GPU worker on its OWN reserved `core`. Idempotent per VM session.
/// Only when `FULL_GPU_BACKEND` + a core was reserved (see `mod::guest_vcpus`).
pub fn start_worker(core: usize) {
    if WORKER_RUNNING.swap(true, Ordering::AcqRel) { return; }
    STOP.store(false, Ordering::Release);
    set_worker_core(core);
    set_full_active(true);
    crate::smp::fiber::admit_with_stack(core, worker_entry, 0, WORKER_STACK_BYTES);
}

/// Stop the worker at VM teardown and wait (bounded) for it to exit.
pub fn stop_worker() {
    if !WORKER_RUNNING.load(Ordering::Acquire) { return; }
    set_full_active(false);
    STOP.store(true, Ordering::Release);
    for _ in 0..50_000_000u64 {
        if !WORKER_RUNNING.load(Ordering::Acquire) { break; }
        core::hint::spin_loop();
    }
    GPU_KICK.store(0xFFFF, Ordering::Release);
    GPU_IRQ_PENDING.store(false, Ordering::Release);
    WORKER_CORE.store(usize::MAX, Ordering::Release);
}

fn worker_entry(_arg: u64) {
    loop {
        if STOP.load(Ordering::Acquire) {
            WORKER_RUNNING.store(false, Ordering::Release);
            return;
        }
        // Drain any deferred controlq notify: do the heavy copy + write_frame on
        // THIS core, off the vCPU. Raise IRQ9 + wake the BSP to inject it.
        if let Some(qidx) = take_gpu_kick() {
            if let Some(gm) = crate::microvm::devices::guest_mem::active() {
                let advanced = GPU.lock().service_queues(qidx, gm);
                if advanced {
                    raise_irq();
                    crate::microvm::cpu::svm::kick_bsp_net_irq();
                }
            }
        }
        // Park until the next doorbell: `note_gpu_kick` → `kick_host_core` bumps
        // this core's net-kick generation + IPIs it, so `kick_wait` resumes us
        // event-driven (2 ms safety re-check on a quiet display).
        crate::smp::fiber::kick_wait(2);
    }
}
