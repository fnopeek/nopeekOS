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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
