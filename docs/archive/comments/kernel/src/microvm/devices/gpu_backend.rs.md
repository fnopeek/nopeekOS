# `kernel/src/microvm/devices/gpu_backend.rs` @ 5e0102684

## L1-19 · `use core::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};`

```
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
```

## L27-28 · `pub fn lock() -> MutexGuard<'static, VirtioGpu> { GPU.lock() }`

```
/// Acquire the GPU device. The vCPU takes this only AFTER `VM_BIG_LOCK`; the
/// worker takes ONLY this — lock order is acyclic.
```

## L31 · `pub fn reset() {`

```
/// Reset the device on VM teardown/start (alongside `net_backend::reset`).
```

## L38-39 · `pub const STAT_BUCKETS: usize = 7;`

```
/// Guest GPU traffic for `cores`: controlq commands by kind, cursorq
/// commands, and the bytes copied on the TRANSFER and FLUSH paths.
```

## L59-61 · `static RESUME_TSC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
/// When a vblank-paused controlq may run again (`VirtioGpu::paused_until`),
/// 0 = not paused. Lock-free, so the vCPU asks on every loop without the
/// device lock, and its timer deadline includes it.
```

## L67 · `pub fn take_resume(now: u64) -> bool {`

```
/// The pause is over: clear it and tell the caller to serve the controlq.
```

## L74-75 · `static D4_PENDING: AtomicBool = AtomicBool::new(false);`

```
/// Mirror of `VirtioGpu::d4_disconnecting`, so the vCPU's per-exit look at
/// the resize state needs no device lock. Written under the lock.
```

## L81-82 · `#[inline]`

```
/// Lock-free BAR0 range check (const base) — the vCPU NPF dispatch tests this on
/// every MMIO exit, so keep it off the device lock (mirror of net_backend).
```

## L88-91 · `static GPU_KICK: AtomicU16 = AtomicU16::new(0xFFFF);`

```
// ── Stage 2 scaffolding (inert in Stage 1): controlq doorbell defer ──
/// Pending controlq notify from the vCPU. 0xFFFF = none. On the doorbell the vCPU
/// sets the qidx here (instead of servicing inline) + wakes the worker's core;
/// the GPU worker drains it on its own core.
```

## L96-100 · `pub const FULL_GPU_BACKEND: bool = false;`

```
/// Compile-time gate for the off-vCPU GPU worker (clean OTA rollback).
/// OFF: the off-vCPU GPU's async IRQ9 delivery to the guest is unreliable —
/// cage's virtio-gpu driver stalled waiting for a completion that never arrived
/// ("no new frames"), so the GPU stays INLINE on the vCPU (synchronous
/// deliver_irq) for now. Re-enable only once the worker→guest IRQ path is proven.
```

## L107 · `pub fn worker_core() -> Option<usize> {`

```
/// Core the GPU worker runs on, if one is up.
```

## L115-116 · `pub fn note_gpu_kick(qidx: u16) {`

```
/// vCPU: the guest notified the controlq (`qidx`). Defer to the worker and, on the
/// empty→set edge, wake its core out of HLT (coalesced like the net TX kick).
```

## L124 · `pub fn take_gpu_kick() -> Option<u16> {`

```
/// Worker: take the pending controlq qidx (clears it). `Some(q)` ⇒ service it.
```

## L130-132 · `static GPU_IRQ_PENDING: AtomicBool = AtomicBool::new(false);`

```
/// Guest GPU completion IRQ (line 9), raised by the worker after it advanced the
/// used-ring, folded into the BSP's `pending_irqs` on its next exit (mirror of the
/// net IRQ10 path). Lock-free so the worker needs no `VmShared` borrow.
```

## L136 · `#[inline]`

```
/// BSP: take the pending GPU IRQ (clears it). True ⇒ fold IRQ9 into pending_irqs.
```

## L140 · `static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);`

```
// ── The off-vCPU GPU worker fiber (Stage 2) ──
```

## L143-144 · `const WORKER_STACK_BYTES: usize = 256 * 1024;`

```
/// service_queues does the ~8 MB framebuffer copy + write_frame; give it a roomy
/// fiber stack (the default 128 KiB has no guard page).
```

## L147-148 · `pub fn start_worker(core: usize) {`

```
/// Spawn the GPU worker on its OWN reserved `core`. Idempotent per VM session.
/// Only when `FULL_GPU_BACKEND` + a core was reserved (see `mod::guest_vcpus`).
```

## L157 · `pub fn stop_worker() {`

```
/// Stop the worker at VM teardown and wait (bounded) for it to exit.
```

## L177-178 · `if let Some(qidx) = take_gpu_kick() {`

```
// Drain any deferred controlq notify: do the heavy copy + write_frame on
// THIS core, off the vCPU. Raise IRQ9 + wake the BSP to inject it.
```

## L188-190 · `crate::smp::fiber::kick_wait(2);`

```
// Park until the next doorbell: `note_gpu_kick` → `kick_host_core` bumps
// this core's net-kick generation + IPIs it, so `kick_wait` resumes us
// event-driven (2 ms safety re-check on a quiet display).
```

