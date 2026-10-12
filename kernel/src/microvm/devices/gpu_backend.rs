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

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, AtomicUsize, Ordering};
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
    MSIX.reset(super::msix::wanted());
    reset_vectors();
    ISR.store(0, Ordering::Release);
    GPU_KICK.store(0, Ordering::Release);
}

// ── Interrupts: MSI-X (config, controlq, cursorq) or INTx line 9 ──

pub const MSIX_VECTORS: usize = 3;
static MSIX: super::msix::Msix<MSIX_VECTORS> = super::msix::Msix::new();
pub fn msix_offered() -> bool { MSIX.offered() }
pub fn msix_msgctl() -> u16 { MSIX.msgctl() }
pub fn msix_set_msgctl(v: u16) { MSIX.set_msgctl(v) }
pub fn msix_mmio(off: u32, write: Option<u32>) -> Option<u32> { MSIX.mmio(off, write) }
pub fn msix_report() -> Option<([u64; 2], alloc::string::String)> { MSIX.report() }

/// virtio `msix_config` and `queue_msix_vector` (controlq 0, cursorq 1), in
/// atomics so any core signals without the device lock; 0xFFFF = none.
static CONFIG_VECTOR: AtomicU16 = AtomicU16::new(0xFFFF);
static QUEUE_VECTOR: [AtomicU16; 2] = [const { AtomicU16::new(0xFFFF) }; 2];
pub fn set_config_vector(v: u16) { CONFIG_VECTOR.store(v, Ordering::Release); }
pub fn set_queue_vector(q: u16, v: u16) {
    if let Some(x) = QUEUE_VECTOR.get(q as usize) { x.store(v, Ordering::Release); }
}
pub fn reset_vectors() {
    CONFIG_VECTOR.store(0xFFFF, Ordering::Release);
    for q in QUEUE_VECTOR.iter() { q.store(0xFFFF, Ordering::Release); }
}

/// Queue `q` completed. `false` = MSI-X is off, the caller raises IRQ 9.
pub fn notify_queue(q: u16) -> bool {
    MSIX.notify(QUEUE_VECTOR.get(q as usize).map_or(0xFFFF, |x| x.load(Ordering::Acquire)))
}
/// The configuration changed. `false` = MSI-X is off, the caller raises IRQ 9.
pub fn notify_config() -> bool {
    MSIX.notify(CONFIG_VECTOR.load(Ordering::Acquire))
}

/// virtio ISR status (read-to-clear): bit 0 queue, bit 1 configuration. An
/// atomic so the guest's ISR read needs no device lock.
static ISR: AtomicU8 = AtomicU8::new(0);
pub fn raise_isr(bits: u8) { ISR.fetch_or(bits, Ordering::AcqRel); }
pub fn take_isr() -> u8 { ISR.swap(0, Ordering::AcqRel) }

/// The BSP at vblank: a paused controlq runs its next frame — on the worker
/// as a doorbell, else here. True = raise IRQ 9.
pub fn vblank(now: u64, gm: &super::guest_mem::GuestMem) -> bool {
    if !take_resume(now) { return false; }
    if full_active() {
        note_gpu_kick(0);
        return false;
    }
    lock().service_queues(0, gm) && !notify_queue(0)
}

/// Windows came or went (`display_tick`). True = raise IRQ 9.
pub fn display_tick_intx() -> bool {
    display_tick() && !notify_config()
}

/// The guest's hot BAR accesses without the device lock: the ISR read, the
/// MSI-X table, and, while the worker owns the queues, the doorbell.
pub fn mmio_fast(off: u32, write: Option<u32>) -> Option<u64> {
    use super::virtio_gpu_pci::{ISR_OFF, ISR_LEN, NOTIFY_OFF, NOTIFY_LEN, NOTIFY_OFF_MULTIPLIER};
    if write.is_none() && (ISR_OFF..ISR_OFF + ISR_LEN).contains(&off) {
        return Some(take_isr() as u64);
    }
    if let Some(v) = MSIX.mmio(off, write) {
        return Some(v as u64);
    }
    if write.is_some() && full_active() && (NOTIFY_OFF..NOTIFY_OFF + NOTIFY_LEN).contains(&off) {
        note_gpu_kick(((off - NOTIFY_OFF) / NOTIFY_OFF_MULTIPLIER) as u16);
        return Some(0);
    }
    None
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

/// The vCPU core's display work: tell the guest that windows came or went
/// (true: raise the GPU interrupt), and send the guest compositor the new
/// size of every window that changed. A window's size waits only while the
/// compositor is not yet listening.
pub fn display_tick() -> bool {
    use super::virtio_console_pci as console;
    use super::virtio_gpu_pci::MAX_OUTPUTS;
    let changed = crate::microvm::take_outputs_changed();
    if changed {
        lock().signal_outputs_changed();
    }
    if console::take_resend_sizes() {
        for k in 0..MAX_OUTPUTS {
            send_size(k);
        }
    } else if crate::shade::surface::any_display_dirty() && console::windows_ready() {
        for k in 0..MAX_OUTPUTS {
            let wid = crate::microvm::output_window(k);
            if wid != 0 && crate::shade::surface::take_display_dirty(wid) {
                send_size(k);
            }
        }
    }
    changed
}

/// `WIN_RESIZE` for output k: u8 output, u16 width, u16 height.
fn send_size(k: usize) {
    use super::virtio_console_pci as console;
    let Some((w, h)) = crate::shade::surface::tile_size(crate::microvm::output_window(k)) else { return };
    let (w, h) = ((w.min(u16::MAX as u32) as u16).to_le_bytes(), (h.min(u16::MAX as u32) as u16).to_le_bytes());
    let _ = console::send_windows(console::WIN_RESIZE, &[k as u8, w[0], w[1], h[0], h[1]]);
}

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

/// The app has painted: frames go to the windows.
pub fn presenting() -> bool {
    PRESENT.load(Ordering::Acquire) == PRESENT_SHOWN
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

// ── Off-vCPU worker: queue doorbell defer ──
/// Queues the vCPU noted a doorbell for, one bit per queue. On the doorbell the
/// vCPU sets the bit here (instead of servicing inline) + wakes the worker's
/// core; the GPU worker drains them on its own core.
static GPU_KICK: AtomicU8 = AtomicU8::new(0);
static WORKER_CORE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Wakes the worker wherever it runs; it may share its core with a vCPU.
static GPU_BELL: crate::smp::fiber::Doorbell = crate::smp::fiber::Doorbell::new();
static FULL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The worker owns the queues: it runs, and the guest took MSI-X, so the
/// worker signals completions itself. Under INTx the line would have to go
/// through the BSP's PIC and the ISR read, and the GPU stays inline.
#[inline]
pub fn full_active() -> bool { FULL_ACTIVE.load(Ordering::Acquire) && MSIX.active() }
pub fn set_full_active(on: bool) { FULL_ACTIVE.store(on, Ordering::Release); }
pub fn set_worker_core(core: usize) { WORKER_CORE.store(core, Ordering::Release); }

/// Core the GPU worker runs on, if one is up.
pub fn worker_core() -> Option<usize> {
    match WORKER_CORE.load(Ordering::Acquire) {
        usize::MAX => None,
        c => Some(c),
    }
}

/// vCPU: the guest notified queue `qidx`. Defer to the worker and, on the
/// empty→set edge, ring its doorbell (coalesced like the net TX kick).
pub fn note_gpu_kick(qidx: u16) {
    if qidx >= 2 { return; }
    if GPU_KICK.fetch_or(1 << qidx, Ordering::AcqRel) == 0 {
        GPU_BELL.ring();
    }
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

/// Spawn the GPU worker, first on `core`. Idempotent per VM session. Only
/// when the off-vCPU GPU path is on (see `cpu::reserve_gpu_core`).
pub fn start_worker(core: usize) {
    if WORKER_RUNNING.swap(true, Ordering::AcqRel) { return; }
    STOP.store(false, Ordering::Release);
    set_worker_core(core);
    set_full_active(true);
    // Movable: it is woken through `GPU_BELL`, and an idle core may take it
    // over while its first core is busy with something else.
    crate::smp::fiber::admit_movable_with_stack(core, worker_entry, 0, WORKER_STACK_BYTES);
}

/// Stop the worker at VM teardown and wait (bounded) for it to exit. False
/// if it is still running: then guest memory must not be freed under it.
#[must_use]
pub fn stop_worker() -> bool {
    if !WORKER_RUNNING.load(Ordering::Acquire) { return true; }
    set_full_active(false);
    STOP.store(true, Ordering::Release);
    // Out of its park now rather than at the next timeout.
    GPU_BELL.ring();
    // Yielding, not spinning: the worker may live on the caller's core.
    for _ in 0..50_000_000u64 {
        if !WORKER_RUNNING.load(Ordering::Acquire) { break; }
        crate::smp::fiber::spin_or_yield();
    }
    let stopped = !WORKER_RUNNING.load(Ordering::Acquire);
    GPU_KICK.store(0, Ordering::Release);
    GPU_IRQ_PENDING.store(false, Ordering::Release);
    WORKER_CORE.store(usize::MAX, Ordering::Release);
    stopped
}

fn worker_entry(_arg: u64) {
    GPU_BELL.bind();
    loop {
        if STOP.load(Ordering::Acquire) {
            GPU_BELL.unbind();
            WORKER_RUNNING.store(false, Ordering::Release);
            return;
        }
        // Drain the deferred doorbells: the heavy copy + write_frame run on
        // this core, off the vCPU, and the completion goes out as a message.
        let kicks = GPU_KICK.swap(0, Ordering::AcqRel);
        if kicks != 0 {
            if let Some(gm) = crate::microvm::devices::guest_mem::active() {
                for q in 0..2u16 {
                    if kicks & (1 << q) != 0
                        && GPU.lock().service_queues(q, gm)
                        && !notify_queue(q)
                    {
                        // The guest left MSI-X while the kick was in flight.
                        raise_irq();
                        crate::microvm::cpu::kick_bsp_net_irq();
                    }
                }
            }
        }
        // Park until the next doorbell (`note_gpu_kick`), with a 2 ms safety
        // re-check on a quiet display. A doorbell that rang since the swap
        // above ends the park at once.
        let deadline = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 500;
        crate::smp::fiber::park_on_bell(deadline, || {
            GPU_KICK.load(Ordering::Acquire) != 0 || STOP.load(Ordering::Acquire)
        });
        // It may have woken on another core (`fiber::steal_for`).
        set_worker_core(crate::smp::per_core::current_core_id());
    }
}
