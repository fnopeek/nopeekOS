//! Off-vCPU GPU backend for the microvm (vhost/virtio-gpu-style).
//!
//! Mirrors `net_backend`: moves the virtio-gpu device out of `VmShared` so a
//! dedicated GPU worker fiber can drain the controlq + do the framebuffer
//! copy + `write_frame` on its own core, while the vCPU only notes the
//! controlq doorbell (a cheap exit). Inline, the per-frame copy runs on the
//! same vCPU that services the network and competes with it.
//!
//! Why out of VmShared: the GPU copy reads guest pages (`guest_mem::active()`,
//! already `&self`) and writes the compositor surface (global `shade::surface`).
//! Only the device state needed to move out so the worker can hold it across
//! cores without waiting for the vCPUs' device lock. Lock order: a vCPU takes
//! `VmShared::dev` then this lock; the worker takes only this lock — no cycle.

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use spin::{Mutex, MutexGuard};
use super::virtio_gpu_pci::{VirtioGpu, BAR0_BASE};

static GPU: Mutex<VirtioGpu> = Mutex::new(VirtioGpu::new());

/// Acquire the GPU device. The vCPU takes this only after `VmShared::dev`;
/// the worker takes only this — lock order is acyclic.
pub fn lock() -> MutexGuard<'static, VirtioGpu> { GPU.lock() }

/// Reset the device on VM teardown/start (alongside `net_backend::reset`).
pub fn reset() {
    *GPU.lock() = VirtioGpu::new();
    RESUME_TSC.store(0, Ordering::Release);
    D4_PENDING.store(false, Ordering::Release);
}

/// Guest GPU traffic for `cores`: controlq commands by kind, cursorq
/// commands, and the bytes copied on the TRANSFER and FLUSH paths.
pub const STAT_BUCKETS: usize = 7;
pub const STAT_LABELS: [&str; STAT_BUCKETS] =
    ["transfer", "flush", "scanout", "other", "cursor", "xfer-KB", "flush-KB"];
pub const STAT_TRANSFER: usize = 0;
pub const STAT_FLUSH: usize = 1;
pub const STAT_SCANOUT: usize = 2;
pub const STAT_OTHER: usize = 3;
pub const STAT_CURSOR: usize = 4;
pub const STAT_XFER_KB: usize = 5;
pub const STAT_FLUSH_KB: usize = 6;
static STATS: [core::sync::atomic::AtomicU64; STAT_BUCKETS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; STAT_BUCKETS];
pub fn note(kind: usize, n: u64) {
    STATS[kind].fetch_add(n, Ordering::Relaxed);
}
pub fn stats_snapshot() -> [u64; STAT_BUCKETS] {
    core::array::from_fn(|i| STATS[i].load(Ordering::Relaxed))
}

/// When a vblank-paused controlq may run again (`VirtioGpu::paused_until`),
/// 0 = not paused. Lock-free, so the vCPU asks on every loop without the
/// device lock, and its timer deadline includes it.
static RESUME_TSC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(super) fn set_resume_tsc(t: u64) { RESUME_TSC.store(t, Ordering::Release); }
pub fn resume_tsc() -> Option<u64> {
    match RESUME_TSC.load(Ordering::Acquire) { 0 => None, t => Some(t) }
}
/// The pause is over: clear it and tell the caller to serve the controlq.
pub fn take_resume(now: u64) -> bool {
    let t = RESUME_TSC.load(Ordering::Acquire);
    t != 0 && now >= t
        && RESUME_TSC.compare_exchange(t, 0, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

/// Mirror of `VirtioGpu::d4_disconnecting`, so the vCPU's per-exit look at
/// the resize state needs no device lock. Written under the lock.
static D4_PENDING: AtomicBool = AtomicBool::new(false);
pub(super) fn set_d4_pending(on: bool) { D4_PENDING.store(on, Ordering::Release); }
#[inline]
pub fn d4_pending() -> bool { D4_PENDING.load(Ordering::Acquire) }

/// Lock-free BAR0 range check (const base) — the vCPU NPF dispatch tests this on
/// every MMIO exit, so keep it off the device lock (mirror of net_backend).
#[inline]
pub fn bar0_in_range(gpa: u64) -> bool {
    gpa >= BAR0_BASE && gpa < BAR0_BASE + 0x4000
}

// ── When guest frames reach the window ──
//
// A launch shows the window's own background until the app has painted:
// the guest console and the compositor's empty output are black, and an
// app starting looks like a black window with a console behind it. The
// first frame after launch that is mostly not black is the app; from then
// on every frame shows. If none comes within `REVEAL_TIMEOUT_MS` (a failed
// start, an app that is black) the frames show anyway, so the failure is
// visible. Once the app has exited, the compositor's black teardown frames
// are dropped and the window keeps the app's last frame until it closes.

const PRESENT_HIDDEN: u8 = 0;
const PRESENT_SHOWN: u8 = 1;
const PRESENT_CLOSING: u8 = 2;
static PRESENT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(PRESENT_SHOWN);
static LAUNCH_TSC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// How long a launch may stay hidden without a lit frame.
const REVEAL_TIMEOUT_MS: u64 = 8000;
/// Share of sampled pixels, in percent, that makes a frame the app's.
pub const LIT_PCT: usize = 50;
/// Sample every n-th pixel of every n-th row.
const SAMPLE_STEP: usize = 4;

/// Percent of sampled pixels (BGRX, `width` per row) that are not black.
pub fn lit_percent(pixels: &[u8], width: u32, height: u32) -> usize {
    let stride = width as usize * 4;
    if stride == 0 {
        return 0;
    }
    let (mut lit, mut total) = (0usize, 0usize);
    for row in pixels.chunks_exact(stride).take(height as usize).step_by(SAMPLE_STEP) {
        for px in row.chunks_exact(4).step_by(SAMPLE_STEP) {
            total += 1;
            if px[0] | px[1] | px[2] != 0 {
                lit += 1;
            }
        }
    }
    if total == 0 { 0 } else { lit * 100 / total }
}

/// A launch begins: hide guest frames until the app paints.
pub fn present_reset() {
    LAUNCH_TSC.store(crate::interrupts::rdtsc(), Ordering::Relaxed);
    PRESENT.store(PRESENT_HIDDEN, Ordering::Release);
}

/// The app has exited: keep the last frame, drop the teardown.
pub fn present_closing() {
    PRESENT.store(PRESENT_CLOSING, Ordering::Release);
}

/// Whether this flushed frame goes to the window.
pub fn should_present(pixels: &[u8], width: u32, height: u32) -> bool {
    match PRESENT.load(Ordering::Acquire) {
        PRESENT_SHOWN => true,
        PRESENT_CLOSING => false,
        _ => {
            let hz = crate::interrupts::tsc_freq().max(1);
            let waited_ms = crate::interrupts::rdtsc()
                .saturating_sub(LAUNCH_TSC.load(Ordering::Relaxed)) * 1000 / hz;
            if waited_ms >= REVEAL_TIMEOUT_MS || lit_percent(pixels, width, height) >= LIT_PCT {
                PRESENT.store(PRESENT_SHOWN, Ordering::Release);
                true
            } else {
                false
            }
        }
    }
}

// ── Off-vCPU worker: controlq doorbell defer ──
/// Pending controlq notify from the vCPU. 0xFFFF = none. On the doorbell the vCPU
/// sets the qidx here (instead of servicing inline) + wakes the worker's core;
/// the GPU worker drains it on its own core.
static GPU_KICK: AtomicU16 = AtomicU16::new(0xFFFF);
static WORKER_CORE: AtomicUsize = AtomicUsize::new(usize::MAX);
static FULL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Compile-time gate for the off-vCPU GPU worker. Off: the off-vCPU GPU's
/// async IRQ9 delivery to the guest is unreliable (cage's virtio-gpu driver
/// can stall waiting for a completion that never arrives), so the GPU stays
/// inline on the vCPU (synchronous deliver_irq).
pub const FULL_GPU_BACKEND: bool = false;
#[inline]
pub fn full_active() -> bool { FULL_ACTIVE.load(Ordering::Acquire) }
pub fn set_full_active(on: bool) { FULL_ACTIVE.store(on, Ordering::Release); }
pub fn set_worker_core(core: usize) { WORKER_CORE.store(core, Ordering::Release); }

/// Core the GPU worker runs on, if one is up.
pub fn worker_core() -> Option<usize> {
    match WORKER_CORE.load(Ordering::Acquire) {
        usize::MAX => None,
        c => Some(c),
    }
}

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

// ── The off-vCPU GPU worker fiber ──
static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// service_queues does the framebuffer copy + write_frame; give it a roomy
/// fiber stack (the default 128 KiB has no guard page).
const WORKER_STACK_BYTES: usize = 256 * 1024;

/// Spawn the GPU worker on its own reserved `core`. Idempotent per VM session.
/// Only when `FULL_GPU_BACKEND` + a core was reserved (see `mod::guest_vcpus`).
pub fn start_worker(core: usize) {
    if WORKER_RUNNING.swap(true, Ordering::AcqRel) { return; }
    STOP.store(false, Ordering::Release);
    set_worker_core(core);
    set_full_active(true);
    crate::smp::fiber::admit_with_stack(core, worker_entry, 0, WORKER_STACK_BYTES);
}

/// Stop the worker at VM teardown and wait (bounded) for it to exit. False
/// if it is still running: then guest memory must not be freed under it.
#[must_use]
pub fn stop_worker() -> bool {
    if !WORKER_RUNNING.load(Ordering::Acquire) { return true; }
    set_full_active(false);
    STOP.store(true, Ordering::Release);
    // Out of its park now rather than at the next timeout.
    let core = WORKER_CORE.load(Ordering::Acquire);
    if core != usize::MAX { crate::smp::kick_host_core(core); }
    for _ in 0..50_000_000u64 {
        if !WORKER_RUNNING.load(Ordering::Acquire) { break; }
        core::hint::spin_loop();
    }
    let stopped = !WORKER_RUNNING.load(Ordering::Acquire);
    GPU_KICK.store(0xFFFF, Ordering::Release);
    GPU_IRQ_PENDING.store(false, Ordering::Release);
    WORKER_CORE.store(usize::MAX, Ordering::Release);
    stopped
}

fn worker_entry(_arg: u64) {
    loop {
        if STOP.load(Ordering::Acquire) {
            WORKER_RUNNING.store(false, Ordering::Release);
            return;
        }
        // Drain any deferred controlq notify: do the heavy copy + write_frame on
        // this core, off the vCPU. Raise IRQ9 + wake the BSP to inject it.
        if let Some(qidx) = take_gpu_kick() {
            if let Some(gm) = crate::microvm::devices::guest_mem::active() {
                let advanced = GPU.lock().service_queues(qidx, gm);
                if advanced {
                    raise_irq();
                    crate::microvm::cpu::kick_bsp_net_irq();
                }
            }
        }
        // Park until the next doorbell: `note_gpu_kick` → `kick_host_core` bumps
        // this core's net-kick generation + IPIs it, so `kick_wait` resumes us
        // event-driven (2 ms safety re-check on a quiet display).
        crate::smp::fiber::kick_wait(2);
    }
}
