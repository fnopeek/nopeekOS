//! Backing store of a virtio-blk disk. An image stored in npkFS as a chunked
//! object is read one chunk at a time; anything else is held whole in RAM.
//!
//! A chunk arrives one of two ways: a prefetch worker on another core reads
//! the image in the background from the moment the disk opens, hottest
//! chunks first, and the guest's own request reads whatever the worker has
//! not reached yet. Which chunks the guest touched is stored next to the
//! image (`<path>.hot`) and decides the order of the next run.
//!
//! Saving writes only the chunks the guest changed and reuses the stored
//! blobs for the rest.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use spin::Mutex;

/// Chunk storage shared between the device and the prefetch worker.
struct Chunks {
    len: usize,
    /// Bytes per chunk; every chunk but the last has exactly this size.
    chunk: usize,
    /// Storage hash per chunk, or empty when the image did not come from a
    /// chunked object (then every chunk is resident from the start).
    hashes: Vec<[u8; 32]>,
    slots: Vec<Mutex<Option<Box<[u8]>>>>,
    /// Chunk read by the guest this run, whoever loaded it.
    touched: Vec<AtomicBool>,
    /// Set when a chunk failed to load; the disk then fails with IOERR
    /// instead of handing the guest zeros.
    failed: AtomicBool,
    loaded: AtomicUsize,
    prefetched: AtomicUsize,
}

impl Chunks {
    fn chunk_len(&self, i: usize) -> usize {
        (self.len - i * self.chunk).min(self.chunk)
    }

    fn fetch(&self, i: usize) -> Option<Box<[u8]>> {
        match crate::npkfs::read_chunk(&self.hashes[i]) {
            Ok(c) if c.len() == self.chunk_len(i) => Some(c.into_boxed_slice()),
            _ => {
                crate::kprintln!("[virtio-blk] chunk {} unreadable — disk fails from here", i);
                self.failed.store(true, Ordering::Release);
                None
            }
        }
    }

    /// Run `f` on chunk `i`, loading it first if nobody has yet.
    fn with<R>(&self, i: usize, f: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
        self.touched[i].store(true, Ordering::Relaxed);
        let mut slot = self.slots[i].lock();
        if slot.is_none() {
            if self.failed.load(Ordering::Acquire) {
                return None;
            }
            *slot = Some(self.fetch(i)?);
            self.loaded.fetch_add(1, Ordering::Relaxed);
        }
        slot.as_deref_mut().map(f)
    }

    /// Prefetch chunk `i` unless it is already there. The read and decrypt
    /// run without the slot lock, so the guest is never held up by it.
    fn prefetch(&self, i: usize) {
        if self.slots[i].lock().is_some() || self.failed.load(Ordering::Acquire) {
            return;
        }
        let Some(c) = self.fetch(i) else { return };
        let mut slot = self.slots[i].lock();
        if slot.is_none() {
            *slot = Some(c);
            self.loaded.fetch_add(1, Ordering::Relaxed);
            self.prefetched.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub struct BlkImage {
    chunks: Arc<Chunks>,
    dirty: Vec<bool>,
    /// npkFS object the image came from; `<path>.hot` holds the touched set.
    path: Option<&'static str>,
}

impl BlkImage {
    /// A resident image built from `bytes`.
    pub fn resident(bytes: Vec<u8>) -> Self {
        let len = bytes.len();
        Self {
            chunks: Arc::new(Chunks {
                len,
                chunk: len.max(1),
                hashes: Vec::new(),
                slots: alloc::vec![Mutex::new(Some(bytes.into_boxed_slice()))],
                touched: alloc::vec![AtomicBool::new(true)],
                failed: AtomicBool::new(false),
                loaded: AtomicUsize::new(1),
                prefetched: AtomicUsize::new(0),
            }),
            dirty: alloc::vec![true],
            path: None,
        }
    }

    /// Open `path` for chunk-wise reads and queue it for prefetch. `None` if
    /// it is missing, stored as one Blob, or its chunks are not uniformly
    /// sized; the caller then falls back to a whole read. `all` prefetches
    /// every chunk; otherwise only those touched on the last run.
    pub fn open_lazy(path: &'static str, all: bool) -> Option<Self> {
        let (total, hashes) = crate::npkfs::chunk_list(path).ok()??;
        let len = usize::try_from(total).ok()?;
        let first = crate::npkfs::read_chunk(hashes.first()?).ok()?;
        let chunk = first.len();
        if chunk == 0 || hashes.len() != len.div_ceil(chunk) {
            return None;
        }
        let n = hashes.len();
        let mut slots: Vec<Mutex<Option<Box<[u8]>>>> = (0..n).map(|_| Mutex::new(None)).collect();
        slots[0] = Mutex::new(Some(first.into_boxed_slice()));
        let chunks = Arc::new(Chunks {
            len,
            chunk,
            hashes,
            slots,
            touched: (0..n).map(|_| AtomicBool::new(false)).collect(),
            failed: AtomicBool::new(false),
            loaded: AtomicUsize::new(1),
            prefetched: AtomicUsize::new(0),
        });
        queue_prefetch(&chunks, prefetch_order(path, n, all));
        Some(Self {
            chunks,
            dirty: alloc::vec![false; n],
            path: Some(path),
        })
    }

    pub fn len(&self) -> usize {
        self.chunks.len
    }

    /// `(chunks touched by the guest, loaded by prefetch, total)` this run.
    pub fn residency(&self) -> (usize, usize, usize) {
        let c = &self.chunks;
        let touched = c.touched.iter().filter(|t| t.load(Ordering::Relaxed)).count();
        (touched, c.prefetched.load(Ordering::Relaxed), c.slots.len())
    }

    /// Hand `f` the bytes at `off`, at most `max` long and never across a
    /// chunk boundary; returns what `f` returns. `None` past the end or on
    /// a read failure.
    pub fn read_with<R>(&mut self, off: usize, max: usize, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        let c = &self.chunks;
        if off >= c.len {
            return None;
        }
        let i = off / c.chunk;
        let at = off - i * c.chunk;
        let n = max.min(c.chunk_len(i) - at);
        c.with(i, |b| f(&b[at..at + n]))
    }

    /// Like [`Self::read_with`] for writing; marks the chunk dirty.
    pub fn write_with<R>(&mut self, off: usize, max: usize, f: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
        let c = &self.chunks;
        if off >= c.len {
            return None;
        }
        let i = off / c.chunk;
        let at = off - i * c.chunk;
        let n = max.min(c.chunk_len(i) - at);
        let r = c.with(i, |b| f(&mut b[at..at + n]))?;
        self.dirty[i] = true;
        Some(r)
    }

    /// Store which chunks the guest touched, for the next run's prefetch.
    pub fn remember_hot(&self) {
        let Some(path) = self.path else { return };
        let c = &self.chunks;
        let mut bits = alloc::vec![0u8; c.slots.len().div_ceil(8)];
        for (i, t) in c.touched.iter().enumerate() {
            if t.load(Ordering::Relaxed) {
                bits[i / 8] |= 1 << (i % 8);
            }
        }
        let hot = alloc::format!("{}.hot", path);
        let _ = crate::npkfs::upsert(&hot, &bits, crate::security::capability::CAP_NULL);
    }

    /// Write the image to `path`. Clean chunks keep their stored blob when
    /// the chunk size matches the writer's; everything else is written out.
    /// Returns `(chunks written, chunks reused)`.
    pub fn save(&mut self, path: &str) -> Result<(usize, usize), crate::npkfs::FsError> {
        let c = &self.chunks;
        if c.failed.load(Ordering::Acquire) {
            return Err(crate::npkfs::FsError::Corrupt);
        }
        let reuse = !c.hashes.is_empty() && c.chunk == crate::npkfs::STREAMING_CHUNK_SIZE;
        let mut w = crate::npkfs::open_streaming_write(path)?;
        let (mut written, mut reused) = (0, 0);
        for i in 0..c.slots.len() {
            if reuse && !self.dirty[i] {
                w.append_stored_chunk(c.hashes[i], c.chunk_len(i))
                    .map_err(|_| crate::npkfs::FsError::Corrupt)?;
                reused += 1;
                continue;
            }
            c.with(i, |b| w.write(b))
                .ok_or(crate::npkfs::FsError::Corrupt)?
                .map_err(|_| crate::npkfs::FsError::Corrupt)?;
            written += 1;
        }
        w.finish().map_err(|_| crate::npkfs::FsError::Corrupt)?;
        Ok((written, reused))
    }
}

/// Chunks touched on the last run first, then (with `all`) the rest, each
/// in ascending order.
fn prefetch_order(path: &str, n: usize, all: bool) -> Vec<usize> {
    let hot = crate::npkfs::fetch(&alloc::format!("{}.hot", path))
        .ok()
        .map(|(b, _)| b)
        .filter(|b| b.len() == n.div_ceil(8));
    let is_hot = |i: usize| hot.as_ref().is_some_and(|b| b[i / 8] & (1 << (i % 8)) != 0);
    let mut order: Vec<usize> = (1..n).filter(|&i| is_hot(i)).collect();
    if all {
        order.extend((1..n).filter(|&i| !is_hot(i)));
    }
    order
}

// ── Prefetch worker ─────────────────────────────────────────────────────

/// Pending prefetch jobs: an image and the chunks still to read.
static QUEUE: Mutex<VecDeque<(Arc<Chunks>, VecDeque<usize>)>> = Mutex::new(VecDeque::new());
static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

/// npkFS reads run AES and the B-tree walk; the default fiber stack is too
/// tight for that chain, as for the 9p persist worker.
const WORKER_STACK_BYTES: usize = 1024 * 1024;

fn queue_prefetch(chunks: &Arc<Chunks>, order: Vec<usize>) {
    if order.is_empty() {
        return;
    }
    QUEUE.lock().push_back((chunks.clone(), order.into()));
    if !RUNNING.swap(true, Ordering::AcqRel) {
        STOP.store(false, Ordering::Release);
        let core = crate::microvm::cpu::place_worker(false);
        crate::smp::fiber::admit_with_stack(core, worker_entry, 0, WORKER_STACK_BYTES);
    }
}

/// Stop prefetching at VM teardown and wait (bounded) for the worker to
/// leave, so the next launch starts with an empty queue.
pub fn stop_worker() {
    if !RUNNING.load(Ordering::Acquire) {
        return;
    }
    STOP.store(true, Ordering::Release);
    for _ in 0..50_000_000u64 {
        if !RUNNING.load(Ordering::Acquire) {
            break;
        }
        core::hint::spin_loop();
    }
}

fn worker_entry(_: u64) {
    loop {
        drain();
        RUNNING.store(false, Ordering::Release);
        // A job queued while the worker was finishing saw it running and did
        // not spawn another one; pick it up instead of leaving it behind.
        if STOP.load(Ordering::Acquire)
            || QUEUE.lock().is_empty()
            || RUNNING.swap(true, Ordering::AcqRel)
        {
            return;
        }
    }
}

fn drain() {
    loop {
        if STOP.load(Ordering::Acquire) {
            QUEUE.lock().clear();
            return;
        }
        let next = {
            let mut q = QUEUE.lock();
            loop {
                let Some((chunks, order)) = q.front_mut() else { break None };
                // Only the queue holds it: the disk is gone.
                if Arc::strong_count(chunks) == 1 {
                    q.pop_front();
                    continue;
                }
                match order.pop_front() {
                    Some(i) => break Some((chunks.clone(), i)),
                    None => {
                        q.pop_front();
                    }
                }
            }
        };
        let Some((chunks, i)) = next else { return };
        chunks.prefetch(i);
        // Let peer fibers on this core take a turn between chunks.
        crate::smp::fiber::yield_ready();
    }
}
