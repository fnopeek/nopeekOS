# `kernel/src/gpu/ggtt_slab.rs` @ 5e0102684

## L1-21 · `#![allow(dead_code)]`

```
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
```

## L33 · `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`

```
// ── Error type ────────────────────────────────────────────────────────
```

## L37 · `BucketUnavailable,`

```
/// Requested bucket is disabled (e.g. the legacy 1 KB bucket).
```

## L39 · `BucketEmpty,`

```
/// Bucket has zero slots configured — nothing to allocate from.
```

## L41 · `InvalidSlot,`

```
/// Slot id out of range for its bucket (caller bug).
```

## L43 · `NotInitialized,`

```
/// Slab not yet initialised; call `init()` first.
```

## L47 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]`

```
// ── SlotId ────────────────────────────────────────────────────────────
```

## L49-50 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]`

```
/// Opaque allocation handle. The GGTT offset is derived on demand so
/// the ID itself is cheap to pass around and hash.
```

## L58 · `pub fn ggtt_offset(self) -> u32 {`

```
/// GGTT byte offset for this slot — stable for its lifetime.
```

## L69 · `struct Bucket {`

```
// ── Bucket state ──────────────────────────────────────────────────────
```

## L73 · `free:     Vec<u32>,`

```
/// Slot indices that are currently free, LIFO. Init: [count-1..0].
```

## L75 · `lru:      VecDeque<u32>,`

```
/// Slot indices in LRU order. Front = oldest (first to evict).
```

## L77 · `evictions: u64,`

```
/// Running counts for stats.
```

## L87 · `for i in (0..count).rev() {`

```
// Push in reverse so alloc hands out low indices first.
```

## L128 · `if let Some(idx) = self.free.pop() {`

```
// Fast path — free list has capacity.
```

## L135 · `match self.lru.pop_front() {`

```
// Slow path — bucket full, evict LRU front.
```

## L144-145 · `Err((SlabError::BucketEmpty, None))`

```
// Full count > 0 but LRU empty: impossible unless the
// free list was corrupted externally. Report cleanly.
```

## L155-156 · `if let Some(pos) = self.lru.iter().position(|&i| i == idx) {`

```
// Remove from LRU (linear — acceptable for typical bucket sizes;
// hot-path `touch` uses the same cost).
```

## L163-165 · `Ok(())`

```
// Freeing a slot that wasn't in use — ignore silently rather
// than panic, matches how Linux slab_free treats double-free
// in debug-soft mode.
```

## L178 · `static SLAB: Mutex<Option<[Bucket; 7]>> = Mutex::new(None);`

```
// ── Global slab state ─────────────────────────────────────────────────
```

## L182-183 · `pub fn init() {`

```
/// Initialise the slab allocator. Safe to call once; subsequent calls
/// reset the state (used mainly for the self-test).
```

## L204 · `pub fn alloc(kind: BucketKind) -> Result<SlotId, SlabError> {`

```
/// Allocate a slot in the requested bucket. Evicts LRU on overflow.
```

## L211 · `pub fn free(id: SlotId) -> Result<(), SlabError> {`

```
/// Free a previously-allocated slot. Double-free is a no-op.
```

## L218 · `pub fn touch(id: SlotId) {`

```
/// Mark a slot as recently used — keeps it from being evicted next.
```

## L225 · `#[derive(Debug, Clone, Copy)]`

```
// ── Stats ─────────────────────────────────────────────────────────────
```

## L232 · `pub residency:  u32, // percent 0..=100`

```
// percent 0..=100
```

## L259 · `pub fn dump_stats() {`

```
/// Pretty-print stats to serial.
```

## L284 · `pub fn self_test() -> bool {`

```
// ── Self-test ─────────────────────────────────────────────────────────
```

## L286-287 · `pub fn self_test() -> bool {`

```
/// 1000 alloc/free cycles across realistic buckets. Verifies no leak,
/// LRU eviction kicks in, slot ids are stable. Called from intent.
```

## L291 · `let count = BUCKET_SLOT_COUNTS[BucketKind::Tile1M as usize];`

```
// Phase A: fill Tile1M to capacity + evict pass.
```

## L303 · `for id in ids.drain(..count as usize / 2) {`

```
// Now 16 evictions should have happened.
```

## L305 · `for id in ids.drain(..count as usize / 2) {`

```
// Phase B: free half, alloc half from a different bucket.
```

## L313 · `for _ in 0..256 {`

```
// Phase C: churn — 256 paired alloc/free cycles on 256K bucket.
```

## L320 · `for id in ids {`

```
// Phase D: all remaining Tile1M allocations freed.
```

## L325 · `let s = match stats() { Some(s) => s, None => return false };`

```
// Validate: every non-reserved bucket returned to zero in_use.
```

