//! Tile grid — the workhorse of widget rasterization.
//!
//! A window's content is rasterized into a **fixed grid of tiles** in the
//! GGTT slab. Tiles are geometry-driven, not widget-driven — they remain
//! stable across tree rebuilds. Off-screen tiles LRU-evict individually.
//!
//! P10.0 scope: constants + TileId struct. Coordinate math, dirty-set
//! scheduling, and per-tile raster tasks land in P10.5.

#![allow(dead_code)]

/// Tile edge length in **actual** pixels (not logical). BGRA32 → 1 MB per
/// tile, matching the slab's primary bucket. At 2× HiDPI this renders
/// content at 256×256 logical.
///
/// Frozen: moving this value later rewrites every cached tile pointer and
/// shifts every per-window tile-grid coordinate.
///
/// Rationale (see PHASE10_WIDGETS.md "Raster granularity"):
///   - 512×512 BGRA32 = 1 048 576 B = exact 1 MB slab bucket
///   - 4K window (3840×2160) = 8×5 = 40 tiles ≈ 40 MB residency
///   - Matches Blink/WebKit's post-2013 tile size class
pub const TILE_SIZE_PX: u32 = 512;

/// Bytes per tile at BGRA32. `TILE_SIZE_PX * TILE_SIZE_PX * 4`.
pub const TILE_BYTES: usize = (TILE_SIZE_PX as usize) * (TILE_SIZE_PX as usize) * 4;

/// Per-window tile identifier. `(window_id, tx, ty)` uniquely keys a tile
/// in the per-app cache. `tx` / `ty` are grid indices, not pixel coords —
/// pixel rect is `(tx * TILE_SIZE_PX, ty * TILE_SIZE_PX, TILE_SIZE_PX, TILE_SIZE_PX)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileId {
    pub window_id: u32,
    pub tx:        u16,
    pub ty:        u16,
}

impl TileId {
    pub const fn new(window_id: u32, tx: u16, ty: u16) -> Self {
        Self { window_id, tx, ty }
    }

    /// Top-left corner of this tile in window coordinates (px).
    pub const fn origin_px(self) -> (u32, u32) {
        (self.tx as u32 * TILE_SIZE_PX, self.ty as u32 * TILE_SIZE_PX)
    }
}

// Compile-time sanity: 1 MB bucket invariant.
const _: () = assert!(TILE_BYTES == 1024 * 1024);
