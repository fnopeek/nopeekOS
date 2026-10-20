//! amdgpu/amdgpu_vram_mgr.c (Linux 6.18.55): VRAM as buddy blocks.
//!
//! With `trace` on, every `new` and `del` prints one line, which
//! oracle/main_vram.c replays through Linux's own `amdgpu_vram_mgr_new`
//! and buddy.c: the request on the left of `->`, the outcome on the right.

use crate::gpu_buddy::*;
use crate::regs::*;
use crate::ttm::{ttm_caching, ttm_place, ttm_resource, ttm_resource_init, ttm_resource_manager, ttm_resource_priv};
use alloc::vec::Vec;

const PAGE_SHIFT: u32 = 12;
const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;

/// `struct amdgpu_vram_mgr`.
pub struct amdgpu_vram_mgr {
    pub manager: ttm_resource_manager,
    pub mm: gpu_buddy,
    pub default_page_size: u64,
    pub vis_usage: u64,
    /// Print every new/del for the oracle.
    pub trace: bool,
    /// The number the next allocation gets in the trace.
    next_id: u32,
}

/// What `amdgpu_vram_mgr_new` reads of the buffer and the device.
pub struct vram_request {
    pub size: u64,
    pub page_alignment: u32,
    pub bo_type: u32,
    pub bo_flags: u64,
    pub mc_vram_size: u64,
    pub visible_vram_size: u64,
}

/// `amdgpu_vram_mgr_block_start`.
pub fn amdgpu_vram_mgr_block_start(mm: &gpu_buddy, block: usize) -> u64 {
    mm.block_offset(block)
}

/// `amdgpu_vram_mgr_block_size`.
pub fn amdgpu_vram_mgr_block_size(mm: &gpu_buddy, block: usize) -> u64 {
    PAGE_SIZE << mm.block_order(block)
}

/// `amdgpu_vram_mgr_vis_size`.
fn amdgpu_vram_mgr_vis_size(visible_vram_size: u64, mm: &gpu_buddy, block: usize) -> u64 {
    let start = amdgpu_vram_mgr_block_start(mm, block);
    let end = start + amdgpu_vram_mgr_block_size(mm, block);
    if start >= visible_vram_size {
        return 0;
    }
    end.min(visible_vram_size) - start
}

/// `amdgpu_vram_mgr_bo_visible_size`: the CPU-visible bytes of buffer `h`.
pub fn amdgpu_vram_mgr_bo_visible_size(adev: &crate::amdgpu_device::amdgpu_device, h: usize) -> u64 {
    let size = crate::amdgpu_object::amdgpu_bo_size(adev, h);
    // amdgpu_gmc_vram_full_visible
    if adev.gmc.real_vram_size == adev.gmc.visible_vram_size {
        return size;
    }
    let Some(res) = crate::amdgpu_object::bo(adev, h).tbo.resource.as_ref() else { return 0 };
    if res.start >= adev.gmc.visible_vram_size >> PAGE_SHIFT {
        return 0;
    }
    let (Some(mgr), ttm_resource_priv::vram { blocks, .. }) = (adev.mman.vram_mgr.as_ref(), &res.priv_) else { return 0 };
    blocks.iter().map(|&b| amdgpu_vram_mgr_vis_size(adev.gmc.visible_vram_size, &mgr.mm, b)).sum()
}

/// `amdgpu_is_vram_mgr_blocks_contiguous`.
fn amdgpu_is_vram_mgr_blocks_contiguous(mm: &gpu_buddy, blocks: &[usize]) -> bool {
    if blocks.is_empty() {
        return false;
    }
    blocks.windows(2).all(|w| {
        amdgpu_vram_mgr_block_start(mm, w[0]) + amdgpu_vram_mgr_block_size(mm, w[0]) == amdgpu_vram_mgr_block_start(mm, w[1])
    })
}

/// `amdgpu_vram_mgr_blocks_size`.
fn amdgpu_vram_mgr_blocks_size(mm: &gpu_buddy, blocks: &[usize]) -> u64 {
    blocks.iter().map(|&b| amdgpu_vram_mgr_block_size(mm, b)).sum()
}

impl amdgpu_vram_mgr {
    /// `amdgpu_vram_mgr_init`.
    pub fn init(real_vram_size: u64) -> Result<Self, i32> {
        let mut manager = ttm_resource_manager::default();
        manager.init(real_vram_size);
        let mm = gpu_buddy::init(manager.size, PAGE_SIZE)?;
        manager.use_type = true;
        Ok(amdgpu_vram_mgr { manager, mm, default_page_size: PAGE_SIZE, vis_usage: 0, trace: false, next_id: 0 })
    }

    /// Ends the line `l` with the resource and prints one line per block.
    fn trace_line(&self, mut l: crate::linux::Line, res: &ttm_resource) {
        let empty = Vec::new();
        let (id, blocks) = match &res.priv_ {
            ttm_resource_priv::vram { id, blocks, .. } => (*id, blocks),
            _ => (0, &empty),
        };
        l.s(" id ").dec(id as u64).s(" start ").dec(res.start).s(" placement ").hex(res.placement).s(" blocks ")
            .dec(blocks.len() as u64).say();
        for &b in blocks {
            crate::linux::Line::new().s("  block ").hex64(self.mm.block_offset(b)).s("+")
                .dec(amdgpu_vram_mgr_block_size(&self.mm, b) >> PAGE_SHIFT).say();
        }
    }

    /// `amdgpu_vram_mgr_new`.
    pub fn new(&mut self, r: &vram_request, place: &ttm_place) -> Result<ttm_resource, i32> {
        let result = self.new_inner(r, place);
        if self.trace {
            let mut l = crate::linux::Line::new();
            l.s("vram new ").dec(r.size >> PAGE_SHIFT).s(" align ").dec(r.page_alignment as u64).s(" type ")
                .dec(r.bo_type as u64).s(" flags ").hex(r.bo_flags as u32).s(" place ").dec(place.fpfn as u64).s("-")
                .dec(place.lpfn as u64).s(" ").hex(place.flags).s(" ->");
            match &result {
                Ok(res) => self.trace_line(l, res),
                Err(e) => {
                    l.s(" err ").dec((-e) as u64).say();
                }
            }
        }
        result
    }

    fn new_inner(&mut self, r: &vram_request, place: &ttm_place) -> Result<ttm_resource, i32> {
        let mut lpfn = (place.lpfn as u64) << PAGE_SHIFT;
        if lpfn == 0 || lpfn > self.manager.size {
            lpfn = self.manager.size;
        }
        let fpfn = (place.fpfn as u64) << PAGE_SHIFT;
        let mut max_bytes = r.mc_vram_size;
        if r.bo_type != ttm_bo_type_kernel {
            max_bytes -= AMDGPU_VM_RESERVED_VRAM as u64;
        }
        let contiguous = r.bo_flags & AMDGPU_GEM_CREATE_VRAM_CONTIGUOUS as u64 != 0;
        let mut pages_per_block: u64 = if contiguous {
            !0
        } else {
            // Not CONFIG_TRANSPARENT_HUGEPAGE: default to 2MB
            (2u64 << (20 - PAGE_SHIFT)).max(r.page_alignment as u64)
        };

        let mut res = ttm_resource_init(r.size, place);
        self.manager.usage += res.size;

        // bail out quickly if there's likely not enough VRAM for this BO
        if self.manager.usage > max_bytes {
            self.manager.usage -= res.size;
            return Err(-28);
        }

        let mut flags = 0u64;
        if place.flags & TTM_PL_FLAG_TOPDOWN != 0 {
            flags |= GPU_BUDDY_TOPDOWN_ALLOCATION;
        }
        if contiguous {
            flags |= GPU_BUDDY_CONTIGUOUS_ALLOCATION;
        }
        if r.bo_flags & AMDGPU_GEM_CREATE_VRAM_CLEARED as u64 != 0 {
            flags |= GPU_BUDDY_CLEAR_ALLOCATION;
        }
        if fpfn != 0 || lpfn != self.mm.size {
            // Allocate blocks in desired range
            flags |= GPU_BUDDY_RANGE_ALLOCATION;
        }
        // GFX12 DCC is not Renoir's: no adjust_dcc_size.
        let mut blocks: Vec<usize> = Vec::new();
        let mut remaining_size = res.size;
        while remaining_size != 0 {
            let mut min_block_size =
                if r.page_alignment != 0 { (r.page_alignment as u64) << PAGE_SHIFT } else { self.default_page_size };
            let size = remaining_size;
            if size >= pages_per_block << PAGE_SHIFT && size & ((pages_per_block << PAGE_SHIFT) - 1) == 0 {
                min_block_size = pages_per_block << PAGE_SHIFT;
            }
            let err = self.mm.alloc_blocks(fpfn, lpfn, size, min_block_size, &mut blocks, flags);
            if err == -28 && pages_per_block == !0 && place.flags & TTM_PL_FLAG_CONTIGUOUS == 0 {
                flags &= !GPU_BUDDY_CONTIGUOUS_ALLOCATION;
                pages_per_block = (2u64 << (20 - PAGE_SHIFT)).max(r.page_alignment as u64);
                continue;
            }
            if err != 0 {
                self.mm.free_list(&mut blocks, 0);
                self.manager.usage -= res.size;
                return Err(err);
            }
            remaining_size = remaining_size.saturating_sub(size);
        }

        res.start = 0;
        let size = amdgpu_vram_mgr_blocks_size(&self.mm, &blocks).max(res.size);
        let mut vis_usage = 0;
        for &b in &blocks {
            let mut start = (amdgpu_vram_mgr_block_start(&self.mm, b) + amdgpu_vram_mgr_block_size(&self.mm, b)) >> PAGE_SHIFT;
            let pfn_up = size.div_ceil(PAGE_SIZE);
            if start > pfn_up {
                start -= pfn_up;
            } else {
                start = 0;
            }
            res.start = res.start.max(start);
            vis_usage += amdgpu_vram_mgr_vis_size(r.visible_vram_size, &self.mm, b);
        }
        if amdgpu_is_vram_mgr_blocks_contiguous(&self.mm, &blocks) {
            res.placement |= TTM_PL_FLAG_CONTIGUOUS;
        }
        res.caching = ttm_caching::ttm_write_combined;
        self.vis_usage += vis_usage;
        res.priv_ = ttm_resource_priv::vram { blocks, flags, id: self.next_id };
        self.next_id += 1;
        Ok(res)
    }

    /// `amdgpu_vram_mgr_del`. No reservations are pending (they come from
    /// RAS bad pages), so `amdgpu_vram_mgr_do_reserve` has nothing to do.
    pub fn del(&mut self, res: &mut ttm_resource, visible_vram_size: u64) {
        if self.trace {
            let mut l = crate::linux::Line::new();
            l.s("vram del");
            self.trace_line(l, res);
        }
        if let ttm_resource_priv::vram { blocks, flags, .. } = &mut res.priv_ {
            let vis_usage: u64 = blocks.iter().map(|&b| amdgpu_vram_mgr_vis_size(visible_vram_size, &self.mm, b)).sum();
            let f = *flags;
            self.mm.free_list(blocks, f);
            self.vis_usage -= vis_usage;
        }
        self.manager.usage -= res.size;
        res.priv_ = ttm_resource_priv::none;
    }
}
