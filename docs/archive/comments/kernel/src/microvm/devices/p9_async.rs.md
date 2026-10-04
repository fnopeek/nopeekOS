# `kernel/src/microvm/devices/p9_async.rs` @ 5e0102684

## L1-20 · `use alloc::collections::{BTreeMap, VecDeque};`

```
//! Asynchronous 9p write persistence.
//!
//! The synchronous 9p Twrite path persisted each streamed chunk INLINE on the
//! vCPU exit handler — freezing the whole guest vCPU (no RX pump, no guest TCP
//! softirqs) for the put/encrypt and, on close, the commit. On a 1 Gbit
//! download-to-disk that collapsed throughput ~8× (measured: speedtest-to-RAM
//! 460 Mbit vs file-to-disk 56 Mbit): the guest couldn't ACK while it was
//! blocked waiting for each chunk's Rwrite, so the sender never ramped.
//!
//! This module moves npkFS persistence onto a worker fiber on ANOTHER core. The
//! vCPU enqueues a chunk and replies LATER (deferred Rwrite via the device's
//! in-flight table), so it never freezes — the guest kernel keeps running TCP,
//! ACKs flow, the sender ramps. Durability is preserved: the reply is posted
//! only AFTER the worker actually persisted, and `Finish` runs the full 4-phase
//! commit (FLUSH/FUA) before its reply. Nothing is ack'd before it is durable.
//!
//! Concurrency is race-free by ownership split: the worker touches ONLY the FS
//! + these two queues; the vCPU owns the virtqueue, guest memory, and fid
//! table. Data crosses as owned `Vec<u8>` through the queues — no shared device
//! state across cores.
```

## L30-32 · `pub const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;`

```
/// Backpressure ceiling: bytes buffered in PENDING but not yet persisted. When
/// exceeded, the vCPU stops pulling the 9p avail-ring, so the guest's
/// outstanding 9p tags self-limit (flow control) — bounded host RAM, no drops.
```

## L35-36 · `enum Op {`

```
/// What the worker should do for one queued op. `key` (the fid) identifies the
/// per-file StreamingWriter the worker owns.
```

## L38 · `Start { path: String, data: Vec<u8> },`

```
/// First op of a streamed file: create the writer + write the buffered prefix.
```

## L40 · `Write { data: Vec<u8> },`

```
/// Append the next sequential chunk.
```

## L42 · `Finish,`

```
/// Flush the final chunk + write manifest + commit (durable), drop the writer.
```

## L50-53 · `reply: bool,`

```
/// Whether the vCPU is waiting for a deferred reply (true) or already acked
/// this Twrite (false — fast path). The worker only emits a `Done` when
/// `reply`, so an already-acked write whose tag the guest later reuses can't
/// be mis-matched in `in_flight`.
```

## L57 · `pub struct Done {`

```
/// A completed op the vCPU must turn into a deferred R-message.
```

## L60 · `pub result: i32,`

```
/// ≥0: success (byte count for writes, 0 for finish). <0: -errno.
```

## L85 · `pub fn poll_done() -> Option<Done> { DONE.lock().pop_front() }`

```
/// vCPU side: drain one completion (build its deferred reply in the device).
```

## L88-92 · `const WORKER_STACK_BYTES: usize = 1024 * 1024;`

```
/// Persist worker stack. npkFS writes (AES + B-tree COW) and especially the
/// commit at `finish()` (journal + bitmap + superblock) run a deep call chain —
/// fine on the main kernel stack but it overflows the default 128 KiB fiber
/// stack, which has no guard page → silent memory smash → worker death → the
/// guest hangs on its next 9p write. 1 MiB is generous headroom.
```

## L95-96 · `pub fn start_worker(core: usize) {`

```
/// Spawn the persist worker on `core` (chosen load-aware, never Core 0).
/// Idempotent within a VM session: a second call while one runs is a no-op.
```

## L98 · `if WORKER_RUNNING.swap(true, Ordering::AcqRel) { return; } // already running`

```
// already running
```

## L103-107 · `pub fn stop_worker() {`

```
/// Stop the worker at VM teardown and WAIT (bounded) for it to exit, so it
/// drops any in-flight writers — which balances the npkFS stream gc-guard
/// (`ACTIVE_STREAMS`) and leaves a clean slate for the next launch. The worker
/// runs on its own core, so this brief spin on the teardown core doesn't block
/// it from exiting. Incomplete downloads are abandoned (the VM is closing).
```

## L117-122 · `const HOT_TICKS: u64 = 30;`

```
/// Ticks (100 Hz → 10 ms each) to stay HOT after the last persisted chunk
/// before parking. The synchronous per-chunk 9p round-trip means the guest
/// waits for each Rwrite; if the worker parks between chunks it wakes only on
/// the ~10 ms per-core timer, gating throughput at ~1 chunk/10 ms. Staying hot
/// (cooperative `yield_ready`, not a parking `yield_sleep`) during a transfer
/// keeps the round-trip in the µs range. ~300 ms hot window covers gaps.
```

## L129-130 · `if STOP.load(Ordering::Acquire) {`

```
// Check teardown FIRST so close is snappy (abandon any queued writes;
// dropping `writers` balances the stream gc-guard).
```

## L154-155 · `if reply {`

```
// Only deferred ops are awaited by the vCPU; fast-path-acked
// writes (reply=false) must NOT post a Done (tag could be reused).
```

## L158 · `crate::microvm::cpu::kick_bsp_net_irq();`

```
// The BSP posts the reply and raises the IRQ; wake it.
```

## L162-163 · `crate::smp::fiber::yield_ready();`

```
// Cooperative yield: lets a co-located vCPU (shared core) run,
// returns immediately when alone so we grab the next chunk fast.
```

## L167-168 · `if crate::interrupts::ticks().wrapping_sub(last_work) < HOT_TICKS {`

```
// Stay hot right after a transfer (low round-trip latency); park
// only once genuinely idle so we don't burn the core forever.
```

## L192 · `Err(_) => -EIO, // w dropped here → stream_end`

```
// w dropped here → stream_end
```

## L201 · `None => 0, // never promoted to streaming → nothing to finish`

```
// never promoted to streaming → nothing to finish
```

