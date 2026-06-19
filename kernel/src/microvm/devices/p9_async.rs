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

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

const EIO: i32 = 5;

/// Backpressure ceiling: bytes buffered in PENDING but not yet persisted. When
/// exceeded, the vCPU stops pulling the 9p avail-ring, so the guest's
/// outstanding 9p tags self-limit (flow control) — bounded host RAM, no drops.
pub const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;

/// What the worker should do for one queued op. `key` (the fid) identifies the
/// per-file StreamingWriter the worker owns.
enum Op {
    /// First op of a streamed file: create the writer + write the buffered prefix.
    Start { path: String, data: Vec<u8> },
    /// Append the next sequential chunk.
    Write { data: Vec<u8> },
    /// Flush the final chunk + write manifest + commit (durable), drop the writer.
    Finish,
}

struct Pending {
    tag: u16,
    key: u64,
    op: Op,
    /// Whether the vCPU is waiting for a deferred reply (true) or already acked
    /// this Twrite (false — fast path). The worker only emits a `Done` when
    /// `reply`, so an already-acked write whose tag the guest later reuses can't
    /// be mis-matched in `in_flight`.
    reply: bool,
}

/// A completed op the vCPU must turn into a deferred R-message.
pub struct Done {
    pub tag: u16,
    /// ≥0: success (byte count for writes, 0 for finish). <0: -errno.
    pub result: i32,
}

static PENDING: Mutex<VecDeque<Pending>> = Mutex::new(VecDeque::new());
static DONE: Mutex<VecDeque<Done>> = Mutex::new(VecDeque::new());
static PENDING_BYTES: AtomicUsize = AtomicUsize::new(0);
static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

pub fn pending_bytes() -> usize { PENDING_BYTES.load(Ordering::Acquire) }
pub fn is_full() -> bool { pending_bytes() >= MAX_PENDING_BYTES }

pub fn enqueue_start(tag: u16, key: u64, path: String, data: Vec<u8>, reply: bool) {
    PENDING_BYTES.fetch_add(data.len(), Ordering::AcqRel);
    PENDING.lock().push_back(Pending { tag, key, op: Op::Start { path, data }, reply });
}
pub fn enqueue_write(tag: u16, key: u64, data: Vec<u8>, reply: bool) {
    PENDING_BYTES.fetch_add(data.len(), Ordering::AcqRel);
    PENDING.lock().push_back(Pending { tag, key, op: Op::Write { data }, reply });
}
pub fn enqueue_finish(tag: u16, key: u64) {
    PENDING.lock().push_back(Pending { tag, key, op: Op::Finish, reply: true });
}

/// vCPU side: drain one completion (build its deferred reply in the device).
pub fn poll_done() -> Option<Done> { DONE.lock().pop_front() }

/// Persist worker stack. npkFS writes (AES + B-tree COW) and especially the
/// commit at `finish()` (journal + bitmap + superblock) run a deep call chain —
/// fine on the main kernel stack but it overflows the default 128 KiB fiber
/// stack, which has no guard page → silent memory smash → worker death → the
/// guest hangs on its next 9p write. 1 MiB is generous headroom.
const WORKER_STACK_BYTES: usize = 1024 * 1024;

/// Spawn the persist worker on `core` (chosen load-aware, never Core 0).
/// Idempotent within a VM session: a second call while one runs is a no-op.
pub fn start_worker(core: usize) {
    if WORKER_RUNNING.swap(true, Ordering::AcqRel) { return; } // already running
    STOP.store(false, Ordering::Release);
    crate::smp::fiber::admit_with_stack(core, worker_entry, 0, WORKER_STACK_BYTES);
}

/// Stop the worker at VM teardown and WAIT (bounded) for it to exit, so it
/// drops any in-flight writers — which balances the npkFS stream gc-guard
/// (`ACTIVE_STREAMS`) and leaves a clean slate for the next launch. The worker
/// runs on its own core, so this brief spin on the teardown core doesn't block
/// it from exiting. Incomplete downloads are abandoned (the VM is closing).
pub fn stop_worker() {
    if !WORKER_RUNNING.load(Ordering::Acquire) { return; }
    STOP.store(true, Ordering::Release);
    for _ in 0..50_000_000u64 {
        if !WORKER_RUNNING.load(Ordering::Acquire) { break; }
        core::hint::spin_loop();
    }
}

/// Ticks (100 Hz → 10 ms each) to stay HOT after the last persisted chunk
/// before parking. The synchronous per-chunk 9p round-trip means the guest
/// waits for each Rwrite; if the worker parks between chunks it wakes only on
/// the ~10 ms per-core timer, gating throughput at ~1 chunk/10 ms. Staying hot
/// (cooperative `yield_ready`, not a parking `yield_sleep`) during a transfer
/// keeps the round-trip in the µs range. ~300 ms hot window covers gaps.
const HOT_TICKS: u64 = 30;

fn worker_entry(_: u64) {
    let mut writers: BTreeMap<u64, crate::storage::npkfs::fs::StreamingWriter> = BTreeMap::new();
    let mut last_work = crate::interrupts::ticks();
    loop {
        // Check teardown FIRST so close is snappy (abandon any queued writes;
        // dropping `writers` balances the stream gc-guard).
        if STOP.load(Ordering::Acquire) {
            writers.clear();
            PENDING.lock().clear();
            DONE.lock().clear();
            PENDING_BYTES.store(0, Ordering::Release);
            WORKER_RUNNING.store(false, Ordering::Release);
            return;
        }
        let item = { PENDING.lock().pop_front() };
        match item {
            Some(p) => {
                let nbytes = match &p.op {
                    Op::Start { data, .. } | Op::Write { data } => data.len(),
                    Op::Finish => 0,
                };
                let tag = p.tag;
                let reply = p.reply;
                let result = apply(&mut writers, p.key, p.op);
                if result < 0 {
                    let n = DIAG.fetch_add(1, Ordering::Relaxed);
                    if n < 16 { crate::kprintln!("[p9-async] persist error (tag={} result={})", tag, result); }
                }
                if nbytes > 0 { PENDING_BYTES.fetch_sub(nbytes, Ordering::AcqRel); }
                // Only deferred ops are awaited by the vCPU; fast-path-acked
                // writes (reply=false) must NOT post a Done (tag could be reused).
                if reply { DONE.lock().push_back(Done { tag, result }); }
                last_work = crate::interrupts::ticks();
                // Cooperative yield: lets a co-located vCPU (shared core) run,
                // returns immediately when alone so we grab the next chunk fast.
                crate::smp::fiber::yield_ready();
            }
            None => {
                // Stay hot right after a transfer (low round-trip latency); park
                // only once genuinely idle so we don't burn the core forever.
                if crate::interrupts::ticks().wrapping_sub(last_work) < HOT_TICKS {
                    crate::smp::fiber::yield_ready();
                } else {
                    crate::smp::fiber::yield_sleep(5);
                }
            }
        }
    }
}

static DIAG: AtomicUsize = AtomicUsize::new(0);

fn apply(
    writers: &mut BTreeMap<u64, crate::storage::npkfs::fs::StreamingWriter>,
    key: u64,
    op: Op,
) -> i32 {
    use crate::storage::npkfs::fs;
    match op {
        Op::Start { path, data } => {
            let mut w = fs::open_streaming_write(&path);
            match w.write(&data) {
                Ok(_) => { let n = data.len() as i32; writers.insert(key, w); n }
                Err(_) => -EIO, // w dropped here → stream_end
            }
        }
        Op::Write { data } => match writers.get_mut(&key) {
            Some(w) => match w.write(&data) { Ok(_) => data.len() as i32, Err(_) => -EIO },
            None => -EIO,
        },
        Op::Finish => match writers.remove(&key) {
            Some(w) => match w.finish() { Ok(_) => 0, Err(_) => -EIO },
            None => 0, // never promoted to streaming → nothing to finish
        },
    }
}
