//! drivers/gpu/drm/ttm (Linux 6.18.55): the buffer object core as far as
//! amdgpu's kernel buffers take it. Kernel buffers are created, pinned and
//! freed; nothing is evicted, swapped or moved between placements once
//! placed, so the LRU lists and the eviction walk are not here.

use alloc::vec::Vec;

/// `struct ttm_place`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ttm_place {
    pub fpfn: u32,
    pub lpfn: u32,
    pub mem_type: u32,
    pub flags: u32,
}

/// `enum ttm_caching`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum ttm_caching {
    #[default]
    ttm_uncached,
    ttm_write_combined,
    ttm_cached,
}

/// What a resource manager keeps for one allocation: the `container_of`
/// of `struct ttm_resource` in each manager.
#[derive(Clone, Debug, Default)]
pub enum ttm_resource_priv {
    /// `struct amdgpu_vram_mgr_resource`: the buddy blocks and their flags.
    vram { blocks: Vec<usize>, flags: u64, id: u32 },
    /// `struct ttm_range_mgr_node` / `struct amdgpu_gtt_node`: one drm_mm node.
    range { node: crate::drm_mm::drm_mm_node },
    /// A placement that holds no space of its own (system, an unbound TT).
    #[default]
    none,
}

/// `struct ttm_resource`.
#[derive(Clone, Debug, Default)]
pub struct ttm_resource {
    /// In pages.
    pub start: u64,
    pub size: u64,
    pub mem_type: u32,
    pub placement: u32,
    pub caching: ttm_caching,
    pub priv_: ttm_resource_priv,
}

/// `ttm_resource_init`: the resource for `size` bytes in `place`. The
/// manager's usage is counted by the caller, which holds the manager.
pub fn ttm_resource_init(size: u64, place: &ttm_place) -> ttm_resource {
    ttm_resource {
        start: 0,
        size,
        mem_type: place.mem_type,
        placement: place.flags,
        caching: ttm_caching::ttm_cached,
        priv_: ttm_resource_priv::none,
    }
}

/// `struct ttm_resource_manager`, the part every manager shares.
#[derive(Default, Clone, Copy)]
pub struct ttm_resource_manager {
    pub use_type: bool,
    pub use_tt: bool,
    pub size: u64,
    pub usage: u64,
}

impl ttm_resource_manager {
    /// `ttm_resource_manager_init`.
    pub fn init(&mut self, size: u64) {
        self.size = size;
        self.usage = 0;
    }
}
