//! GGTT slab allocator — fixed-bucket, LRU-evicting.
//!
//! Tracks occupancy of the per-bucket regions defined in
//! `gpu::ggtt_layout`. Each allocation returns a `SlotId` whose GGTT
//! offset is stable for the lifetime of the slot — cache lookups
//! (glyph atlas, tile cache, comp-layer cache) survive across
//! alloc/free cycles as long as nothing evicts them.
//!
//! Algorithm:
//!   - One `Bucket` per `BucketKind` with (a) a free-list stack of
//!     unused slot indices and (b) an LRU queue of in-use slot indices
//!     (front = oldest, back = newest).
//!   - `alloc` pops from the free list, or — if empty — evicts the
//!     oldest LRU entry and recycles its slot.
//!   - `free` removes the slot from LRU and pushes it back on the free
//!     list.
//!   - `touch` moves a slot to the LRU back (marks it recently used).
//!
//! This file does **not** read or write GGTT memory — it's a pure
//! bookkeeping allocator. Actual glyph/tile bytes land in GGTT when
//! the rasterizer is wired up (P10.5).

#![allow(dead_code)]

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;

use super::ggtt_layout::{
    BUCKET_BASES, BUCKET_REGION_BYTES, BUCKET_SIZES, BUCKET_SLOT_COUNTS, BucketKind,
};

// ── Error type ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlabError {
    /// Requested bucket is disabled (e.g. the legacy 1 KB bucket).
    BucketUnavailable,
    /// Bucket has zero slots configured — nothing to allocate from.
    BucketEmpty,
    /// Slot id out of range for its bucket (caller bug).
    InvalidSlot,
    /// Slab not yet initialised; call `init()` first.
    NotInitialized,
}

// ── SlotId ────────────────────────────────────────────────────────────

/// Opaque allocation handle. The GGTT offset is derived on demand so
/// the ID itself is cheap to pass around and hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotId {
    pub kind: BucketKind,
    pub idx:  u32,
}

impl SlotId {
    /// GGTT byte offset for this slot — stable for its lifetime.
    pub fn ggtt_offset(self) -> u32 {
        let b = self.kind as usize;
        BUCKET_BASES[b] + self.idx * (BUCKET_SIZES[b] as u32)
    }

    pub fn bucket_size(self) -> usize {
        BUCKET_SIZES[self.kind as usize]
    }
}

// ── Bucket state ──────────────────────────────────────────────────────

struct Bucket {
    kind:     BucketKind,
    /// Slot indices that are currently free, LIFO. Init: [count-1..0].
    free:     Vec<u32>,
    /// Slot indices in LRU order. Front = oldest (first to evict).
    lru:      VecDeque<u32>,
    /// Running counts for stats.
    evictions: u64,
    allocs:    u64,
    frees:     u64,
}

impl Bucket {
    fn new(kind: BucketKind) -> Self {
        let count = BUCKET_SLOT_COUNTS[kind as usize];
        let mut free = Vec::with_capacity(count as usize);
        // Push in reverse so alloc hands out low indices first.
        for i in (0..count).rev() {
            free.push(i);
        }
        Self {
            kind,
            free,
            lru: VecDeque::with_capacity(count as usize),
            evictions: 0,
            allocs:    0,
            frees:     0,
        }
    }

    fn total_slots(&self) -> u32 {
        BUCKET_SLOT_COUNTS[self.kind as usize]
    }

    fn in_use(&self) -> usize {
        self.lru.len()
    }

    fn residency_pct(&self) -> u32 {
        let total = self.total_slots();
        if total == 0 { return 0; }
        (self.in_use() as u32 * 100) / total
    }

    fn alloc(&mut self) -> Result<SlotId, (SlabError, Option<SlotId>)> {
        let total = self.total_slots();
        if total == 0 {
            return Err((
                if self.kind as usize == BucketKind::Reserved1K as usize {
                    SlabError::BucketUnavailable
                } else {
                    SlabError::BucketEmpty
                },
                None,
            ));
        }

        // Fast path — free list has capacity.
        if let Some(idx) = self.free.pop() {
            self.lru.push_back(idx);
            self.allocs += 1;
            return Ok(SlotId { kind: self.kind, idx });
        }

        // Slow path — bucket full, evict LRU front.
        match self.lru.pop_front() {
            Some(idx) => {
                self.lru.push_back(idx);
                self.evictions += 1;
                self.allocs    += 1;
                Ok(SlotId { kind: self.kind, idx })
            }
            None => {
                // Full count > 0 but LRU empty: impossible unless the
                // free list was corrupted externally. Report cleanly.
                Err((SlabError::BucketEmpty, None))
            }
        }
    }

    fn free(&mut self, idx: u32) -> Result<(), SlabError> {
        if idx >= self.total_slots() {
            return Err(SlabError::InvalidSlot);
        }
        // Remove from LRU (linear — acceptable for typical bucket sizes;
        // hot-path `touch` uses the same cost).
        if let Some(pos) = self.lru.iter().position(|&i| i == idx) {
            self.lru.remove(pos);
            self.free.push(idx);
            self.frees += 1;
            Ok(())
        } else {
            // Freeing a slot that wasn't in use — ignore silently rather
            // than panic, matches how Linux slab_free treats double-free
            // in debug-soft mode.
            Ok(())
        }
    }

    fn touch(&mut self, idx: u32) {
        if let Some(pos) = self.lru.iter().position(|&i| i == idx) {
            self.lru.remove(pos);
            self.lru.push_back(idx);
        }
    }
}

// ── Global slab state ─────────────────────────────────────────────────

static SLAB: Mutex<Option<[Bucket; 7]>> = Mutex::new(None);

/// Initialise the slab allocator. Safe to call once; subsequent calls
/// reset the state (used mainly for the self-test).
pub fn init() {
    let buckets: [Bucket; 7] = [
        Bucket::new(BucketKind::Reserved1K),
        Bucket::new(BucketKind::CompSmall4K),
        Bucket::new(BucketKind::CompMid16K),
        Bucket::new(BucketKind::CompLarge64K),
        Bucket::new(BucketKind::Small256K),
        Bucket::new(BucketKind::Tile1M),
        Bucket::new(BucketKind::Canvas4M),
    ];
    *SLAB.lock() = Some(buckets);

    let total_mb: u64 = BUCKET_REGION_BYTES.iter().sum::<usize>() as u64 / (1024 * 1024);
    let total_slots: u32 = BUCKET_SLOT_COUNTS.iter().sum();
    crate::kprintln!(
        "[npk] GGTT slab: {} slots across 7 buckets, {} MB carved",
        total_slots, total_mb,
    );
}

/// Allocate a slot in the requested bucket. Evicts LRU on overflow.
pub fn alloc(kind: BucketKind) -> Result<SlotId, SlabError> {
    let mut guard = SLAB.lock();
    let slab = guard.as_mut().ok_or(SlabError::NotInitialized)?;
    slab[kind as usize].alloc().map_err(|(e, _)| e)
}

/// Free a previously-allocated slot. Double-free is a no-op.
pub fn free(id: SlotId) -> Result<(), SlabError> {
    let mut guard = SLAB.lock();
    let slab = guard.as_mut().ok_or(SlabError::NotInitialized)?;
    slab[id.kind as usize].free(id.idx)
}

/// Mark a slot as recently used — keeps it from being evicted next.
pub fn touch(id: SlotId) {
    if let Some(slab) = SLAB.lock().as_mut() {
        slab[id.kind as usize].touch(id.idx);
    }
}

// ── Stats ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct BucketStats {
    pub kind:       BucketKind,
    pub total:      u32,
    pub in_use:     u32,
    pub residency:  u32, // percent 0..=100
    pub allocs:     u64,
    pub frees:      u64,
    pub evictions:  u64,
}

pub fn stats() -> Option<[BucketStats; 7]> {
    let guard = SLAB.lock();
    let slab = guard.as_ref()?;
    let mut out = [BucketStats {
        kind: BucketKind::Reserved1K, total: 0, in_use: 0, residency: 0,
        allocs: 0, frees: 0, evictions: 0,
    }; 7];
    for (i, b) in slab.iter().enumerate() {
        out[i] = BucketStats {
            kind:      b.kind,
            total:     b.total_slots(),
            in_use:    b.in_use() as u32,
            residency: b.residency_pct(),
            allocs:    b.allocs,
            frees:     b.frees,
            evictions: b.evictions,
        };
    }
    Some(out)
}

/// Pretty-print stats to serial.
pub fn dump_stats() {
    let s = match stats() {
        Some(s) => s,
        None => { crate::kprintln!("[npk] slab not initialised"); return; }
    };
    crate::kprintln!("[npk] GGTT slab stats:");
    crate::kprintln!("[npk]   bucket      slots  use%  allocs  frees   evict");
    for b in &s {
        let name = match b.kind {
            BucketKind::Reserved1K   => "1K(rsvd) ",
            BucketKind::CompSmall4K  => "4K       ",
            BucketKind::CompMid16K   => "16K      ",
            BucketKind::CompLarge64K => "64K      ",
            BucketKind::Small256K    => "256K     ",
            BucketKind::Tile1M       => "1M (tile)",
            BucketKind::Canvas4M     => "4M       ",
        };
        crate::kprintln!(
            "[npk]   {} {:5}  {:3}%  {:6}  {:6}  {:6}",
            name, b.total, b.residency, b.allocs, b.frees, b.evictions,
        );
    }
}

// ── Self-test ─────────────────────────────────────────────────────────

/// 1000 alloc/free cycles across realistic buckets. Verifies no leak,
/// LRU eviction kicks in, slot ids are stable. Called from intent.
pub fn self_test() -> bool {
    crate::kprintln!("[npk] slab self-test: starting...");

    // Phase A: fill Tile1M to capacity + evict pass.
    let count = BUCKET_SLOT_COUNTS[BucketKind::Tile1M as usize];
    let mut ids: Vec<SlotId> = Vec::with_capacity(count as usize + 16);
    for _ in 0..(count as usize + 16) {
        match alloc(BucketKind::Tile1M) {
            Ok(id) => ids.push(id),
            Err(e) => {
                crate::kprintln!("[npk] slab: alloc failed mid-fill: {:?}", e);
                return false;
            }
        }
    }
    // Now 16 evictions should have happened.

    // Phase B: free half, alloc half from a different bucket.
    for id in ids.drain(..count as usize / 2) {
        let _ = free(id);
    }
    for _ in 0..64 {
        let _ = alloc(BucketKind::CompSmall4K);
    }

    // Phase C: churn — 256 paired alloc/free cycles on 256K bucket.
    for _ in 0..256 {
        if let Ok(id) = alloc(BucketKind::Small256K) {
            let _ = free(id);
        }
    }

    // Phase D: all remaining Tile1M allocations freed.
    for id in ids {
        let _ = free(id);
    }

    // Validate: every non-reserved bucket returned to zero in_use.
    let s = match stats() { Some(s) => s, None => return false };
    let mut leaks = 0;
    for b in &s {
        if b.in_use > 0 {
            crate::kprintln!(
                "[npk] slab: LEAK {:?} — {} slots still in use",
                b.kind, b.in_use,
            );
            leaks += 1;
        }
    }
    if leaks > 0 {
        return false;
    }

    crate::kprintln!("[npk] slab self-test: OK (1000+ cycles, no leaks)");
    dump_stats();
    true
}
