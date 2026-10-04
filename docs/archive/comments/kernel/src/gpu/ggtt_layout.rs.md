# `kernel/src/gpu/ggtt_layout.rs` @ 5e0102684

## L1-13 · `#![allow(dead_code)]`

```
//! GGTT partition map — frozen at P10.0.
//!
//! The Global Graphics Translation Table is divided into named regions.
//! These numerical addresses are part of the ABI: every cached GGTT
//! pointer (glyph atlas entry, icon atlas entry, tile handle, comp-layer
//! handle) is stable across kernel versions as long as these constants
//! stay put.
//!
//! Moving a partition boundary later invalidates every persisted GGTT
//! offset and forces a full cache rebuild. Do not do this without a
//! wire-version bump.
//!
//! P10.0 scope: constants only. Slab allocator (P10.4) reads from here.
```

## L17 · `pub const GGTT_SCRATCH_BASE: u32 = 0x0000_0000;`

```
// ── Partition boundaries (GGTT byte offsets) ──────────────────────────
```

## L19 · `pub const GGTT_SCRATCH_BASE: u32 = 0x0000_0000;`

```
/// Reserved scratch region at GGTT start. Unused in v1.
```

## L21 · `pub const GGTT_SCRATCH_END:  u32 = 0x0100_0000;  // 16 MB`

```
// 16 MB
```

## L23-24 · `pub const GGTT_FB_BASE: u32 = 0x0100_0000;`

```
/// Framebuffer region (existing — set up by gpu::intel_xe during modeset).
/// 48 MB covers a 4K × 32bpp framebuffer + shadow pair.
```

## L26 · `pub const GGTT_FB_END:  u32 = 0x0400_0000;  // 48 MB window`

```
// 48 MB window
```

## L28 · `pub const GGTT_BCS_BASE: u32 = 0x0400_0000;`

```
/// BCS infrastructure (existing — ring buffer, LRC, HWSP, test pages).
```

## L30 · `pub const GGTT_BCS_END:  u32 = 0x0500_0000;  // 16 MB`

```
// 16 MB
```

## L32-34 · `pub const GGTT_GLYPH_BASE: u32 = 0x0500_0000;`

```
/// Glyph atlas region. Inter Variable rendered glyphs keyed by
/// (glyph_id, size, weight). Populated by `gui/text.rs` (P10.1) and
/// migrated into GGTT in P10.4.
```

## L36 · `pub const GGTT_GLYPH_END:  u32 = 0x0600_0000;  // 16 MB`

```
// 16 MB
```

## L38-39 · `pub const GGTT_ICON_BASE: u32 = 0x0600_0000;`

```
/// Icon atlas region. Phosphor subset, pre-rasterized at build time,
/// uploaded at boot (P10.9). Alpha-only, 5 size variants.
```

## L41 · `pub const GGTT_ICON_END:  u32 = 0x0700_0000;  // 16 MB`

```
// 16 MB
```

## L43-45 · `pub const GGTT_SLAB_BASE: u32 = 0x0700_0000;`

```
/// Tile + composition-layer slab. Primary consumer of GGTT space.
/// ~916 MB upper bound; the allocator (P10.4) carves this into fixed
/// buckets with LRU eviction.
```

## L47 · `pub const GGTT_SLAB_END:  u32 = 0x4000_0000;  // 1 GB — conservative ceiling`

```
// 1 GB — conservative ceiling
```

## L49 · `pub const BUCKET_SIZES: [usize; 7] = [`

```
// ── Slab bucket sizes ─────────────────────────────────────────────────
```

## L51-55 · `pub const BUCKET_SIZES: [usize; 7] = [`

```
/// Slab bucket sizes (bytes), indexed by `BucketKind as usize`.
/// **Primary bucket is 1 MB (tiles).**
///
/// Off-screen tiles evict first; composition layers evict last. Eviction
/// kicks in when slab residency exceeds 80 %.
```

## L57 · `1 * 1024,           //  0: 1 KB  — legacy reserved, not used in tile model`

```
//  0: 1 KB  — legacy reserved, not used in tile model
```

## L58 · `4 * 1024,           //  1: 4 KB  — small comp layers (hover-pill buttons)`

```
//  1: 4 KB  — small comp layers (hover-pill buttons)
```

## L59 · `16 * 1024,          //  2: 16 KB — mid comp layers (tooltip, small popover)`

```
//  2: 16 KB — mid comp layers (tooltip, small popover)
```

## L60 · `64 * 1024,          //  3: 64 KB — larger comp layers (dropdown menu)`

```
//  3: 64 KB — larger comp layers (dropdown menu)
```

## L61 · `256 * 1024,         //  4: 256 KB — small Canvas, large popover/menu`

```
//  4: 256 KB — small Canvas, large popover/menu
```

## L62 · `1 * 1024 * 1024,    //  5: 1 MB  — **PRIMARY** tiles + small Canvas`

```
//  5: 1 MB  — **PRIMARY** tiles + small Canvas
```

## L63 · `4 * 1024 * 1024,    //  6: 4 MB  — large Canvas (up to 1024×1024 logical)`

```
//  6: 4 MB  — large Canvas (up to 1024×1024 logical)
```

## L66 · `#[repr(u8)]`

```
/// Symbolic index into `BUCKET_SIZES`. Use these, never a raw index.
```

## L76 · `Tile1M       = 5,`

```
/// Primary bucket — tiles (512×512 BGRA = exactly 1 MB).
```

## L79 · `}`

```
// Appended only.
```

## L88-89 · `pub const EVICT_WATERMARK_PCT: u32 = 80;`

```
/// Eviction threshold — free old entries when residency exceeds this
/// fraction of the slab region.
```

## L92-106 · `pub const BUCKET_REGION_BYTES: [usize; 7] = [`

```
// ── Per-bucket region layout inside the slab ──────────────────────────
//
// Each bucket gets a contiguous slice of GGTT address space sized to
// realistic peak demand. Sum stays inside GGTT_SLAB_BASE..END with a
// ~20 MB headroom.
//
// Slot count is derived as `region_bytes / bucket_size`. Slot `idx` of
// bucket `b` lives at `BUCKET_BASES[b] + idx * BUCKET_SIZES[b]`. These
// offsets are stable — once an entry is cached at a GGTT offset by
// some consumer (glyph atlas, tile cache), LRU eviction may re-use
// the slot but the address never moves.
//
// Ordering matches `BucketKind`. The 1 KB bucket is the legacy
// placeholder from P10.0; it gets zero bytes here so allocs for it
// fail fast and the tile model stays unambiguous.
```

## L109 · `0,                    // 0: Reserved1K   — not used`

```
// 0: Reserved1K   — not used
```

## L110 · `4 * 1024 * 1024,      // 1: CompSmall4K  — 4 MB / 1024 slots`

```
// 1: CompSmall4K  — 4 MB / 1024 slots
```

## L111 · `8 * 1024 * 1024,      // 2: CompMid16K   — 8 MB / 512 slots`

```
// 2: CompMid16K   — 8 MB / 512 slots
```

## L112 · `16 * 1024 * 1024,     // 3: CompLarge64K — 16 MB / 256 slots`

```
// 3: CompLarge64K — 16 MB / 256 slots
```

## L113 · `32 * 1024 * 1024,     // 4: Small256K    — 32 MB / 128 slots`

```
// 4: Small256K    — 32 MB / 128 slots
```

## L114 · `768 * 1024 * 1024,    // 5: Tile1M       — **768 MB / 768 slots (primary)**`

```
// 5: Tile1M       — **768 MB / 768 slots (primary)**
```

## L115 · `64 * 1024 * 1024,     // 6: Canvas4M     — 64 MB / 16 slots`

```
// 6: Canvas4M     — 64 MB / 16 slots
```

## L118-119 · `pub const BUCKET_BASES: [u32; 7] = {`

```
/// Start offset of each bucket's region inside the slab. Computed
/// at compile-time from cumulative `BUCKET_REGION_BYTES`.
```

## L131 · `pub const BUCKET_SLOT_COUNTS: [u32; 7] = {`

```
/// Number of slots in each bucket region.
```

## L147 · `let mut total: u64 = 0;`

```
// Sum of all bucket regions must fit in the slab.
```

## L156 · `let mut j = 1;  // skip j=0 (Reserved1K has region 0)`

```
// Region count at slot resolution — slots integer-divisible.
```

## L157 · `let mut j = 1;  // skip j=0 (Reserved1K has region 0)`

```
// skip j=0 (Reserved1K has region 0)
```

## L163 · `assert!(BUCKET_REGION_BYTES[BucketKind::Tile1M as usize]`

```
// Primary bucket is the one with the largest region — Tile1M.
```

## L168 · `const _: () = {`

```
// ── Compile-time invariants ───────────────────────────────────────────
```

## L171 · `assert!(GGTT_SCRATCH_END == GGTT_FB_BASE);`

```
// Partitions are non-overlapping and monotonic.
```

## L179 · `assert!(BUCKET_SIZES[BucketKind::Tile1M as usize] == 1024 * 1024);`

```
// Primary tile bucket matches the 512×512 BGRA32 tile size.
```

## L182 · `let mut i = 1;`

```
// Bucket sizes strictly increasing (free-list lookup assumes this).
```

