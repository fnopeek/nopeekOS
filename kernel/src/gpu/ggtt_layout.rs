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

#![allow(dead_code)]

// ── Partition boundaries (GGTT byte offsets) ──────────────────────────

/// Reserved scratch region at GGTT start. Unused in v1.
pub const GGTT_SCRATCH_BASE: u32 = 0x0000_0000;
pub const GGTT_SCRATCH_END:  u32 = 0x0100_0000;  // 16 MB

/// Framebuffer region (existing — set up by gpu::intel_xe during modeset).
/// 48 MB covers a 4K × 32bpp framebuffer + shadow pair.
pub const GGTT_FB_BASE: u32 = 0x0100_0000;
pub const GGTT_FB_END:  u32 = 0x0400_0000;  // 48 MB window

/// BCS infrastructure (existing — ring buffer, LRC, HWSP, test pages).
pub const GGTT_BCS_BASE: u32 = 0x0400_0000;
pub const GGTT_BCS_END:  u32 = 0x0500_0000;  // 16 MB

/// Glyph atlas region. Inter Variable rendered glyphs keyed by
/// (glyph_id, size, weight). Populated by `gui/text.rs` (P10.1) and
/// migrated into GGTT in P10.4.
pub const GGTT_GLYPH_BASE: u32 = 0x0500_0000;
pub const GGTT_GLYPH_END:  u32 = 0x0600_0000;  // 16 MB

/// Icon atlas region. Phosphor subset, pre-rasterized at build time,
/// uploaded at boot (P10.9). Alpha-only, 5 size variants.
pub const GGTT_ICON_BASE: u32 = 0x0600_0000;
pub const GGTT_ICON_END:  u32 = 0x0700_0000;  // 16 MB

/// Tile + composition-layer slab. Primary consumer of GGTT space.
/// ~916 MB upper bound; the allocator (P10.4) carves this into fixed
/// buckets with LRU eviction.
pub const GGTT_SLAB_BASE: u32 = 0x0700_0000;
pub const GGTT_SLAB_END:  u32 = 0x4000_0000;  // 1 GB — conservative ceiling

// ── Slab bucket sizes ─────────────────────────────────────────────────

/// Slab bucket sizes (bytes), indexed by `BucketKind as usize`.
/// **Primary bucket is 1 MB (tiles).**
///
/// Off-screen tiles evict first; composition layers evict last. Eviction
/// kicks in when slab residency exceeds 80 %.
pub const BUCKET_SIZES: [usize; 7] = [
    1 * 1024,           //  0: 1 KB  — legacy reserved, not used in tile model
    4 * 1024,           //  1: 4 KB  — small comp layers (hover-pill buttons)
    16 * 1024,          //  2: 16 KB — mid comp layers (tooltip, small popover)
    64 * 1024,          //  3: 64 KB — larger comp layers (dropdown menu)
    256 * 1024,         //  4: 256 KB — small Canvas, large popover/menu
    1 * 1024 * 1024,    //  5: 1 MB  — **PRIMARY** tiles + small Canvas
    4 * 1024 * 1024,    //  6: 4 MB  — large Canvas (up to 1024×1024 logical)
];

/// Symbolic index into `BUCKET_SIZES`. Use these, never a raw index.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BucketKind {
    Reserved1K   = 0,
    CompSmall4K  = 1,
    CompMid16K   = 2,
    CompLarge64K = 3,
    Small256K    = 4,
    /// Primary bucket — tiles (512×512 BGRA = exactly 1 MB).
    Tile1M       = 5,
    Canvas4M     = 6,
    // Appended only.
}

impl BucketKind {
    pub const fn size(self) -> usize {
        BUCKET_SIZES[self as usize]
    }
}

/// Eviction threshold — free old entries when residency exceeds this
/// fraction of the slab region.
pub const EVICT_WATERMARK_PCT: u32 = 80;

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

pub const BUCKET_REGION_BYTES: [usize; 7] = [
    0,                    // 0: Reserved1K   — not used
    4 * 1024 * 1024,      // 1: CompSmall4K  — 4 MB / 1024 slots
    8 * 1024 * 1024,      // 2: CompMid16K   — 8 MB / 512 slots
    16 * 1024 * 1024,     // 3: CompLarge64K — 16 MB / 256 slots
    32 * 1024 * 1024,     // 4: Small256K    — 32 MB / 128 slots
    768 * 1024 * 1024,    // 5: Tile1M       — **768 MB / 768 slots (primary)**
    64 * 1024 * 1024,     // 6: Canvas4M     — 64 MB / 16 slots
];

/// Start offset of each bucket's region inside the slab. Computed
/// at compile-time from cumulative `BUCKET_REGION_BYTES`.
pub const BUCKET_BASES: [u32; 7] = {
    let mut out = [0u32; 7];
    out[0] = GGTT_SLAB_BASE;
    let mut i = 1;
    while i < 7 {
        out[i] = out[i - 1] + BUCKET_REGION_BYTES[i - 1] as u32;
        i += 1;
    }
    out
};

/// Number of slots in each bucket region.
pub const BUCKET_SLOT_COUNTS: [u32; 7] = {
    let mut out = [0u32; 7];
    let mut i = 0;
    while i < 7 {
        out[i] = if BUCKET_SIZES[i] == 0 {
            0
        } else {
            (BUCKET_REGION_BYTES[i] / BUCKET_SIZES[i]) as u32
        };
        i += 1;
    }
    out
};

const _: () = {
    // Sum of all bucket regions must fit in the slab.
    let mut total: u64 = 0;
    let mut i = 0;
    while i < 7 {
        total += BUCKET_REGION_BYTES[i] as u64;
        i += 1;
    }
    assert!(total <= (GGTT_SLAB_END - GGTT_SLAB_BASE) as u64);

    // Region count at slot resolution — slots integer-divisible.
    let mut j = 1;  // skip j=0 (Reserved1K has region 0)
    while j < 7 {
        assert!(BUCKET_REGION_BYTES[j] % BUCKET_SIZES[j] == 0);
        j += 1;
    }

    // Primary bucket is the one with the largest region — Tile1M.
    assert!(BUCKET_REGION_BYTES[BucketKind::Tile1M as usize]
            >= BUCKET_REGION_BYTES[BucketKind::Canvas4M as usize]);
};

// ── Compile-time invariants ───────────────────────────────────────────

const _: () = {
    // Partitions are non-overlapping and monotonic.
    assert!(GGTT_SCRATCH_END == GGTT_FB_BASE);
    assert!(GGTT_FB_END      == GGTT_BCS_BASE);
    assert!(GGTT_BCS_END     == GGTT_GLYPH_BASE);
    assert!(GGTT_GLYPH_END   == GGTT_ICON_BASE);
    assert!(GGTT_ICON_END    == GGTT_SLAB_BASE);
    assert!(GGTT_SLAB_BASE   <  GGTT_SLAB_END);

    // Primary tile bucket matches the 512×512 BGRA32 tile size.
    assert!(BUCKET_SIZES[BucketKind::Tile1M as usize] == 1024 * 1024);

    // Bucket sizes strictly increasing (free-list lookup assumes this).
    let mut i = 1;
    while i < BUCKET_SIZES.len() {
        assert!(BUCKET_SIZES[i] > BUCKET_SIZES[i - 1]);
        i += 1;
    }
};
