//! amdgpu/amdgpu_res_cursor.h (Linux 6.18.55): walking the address ranges
//! a resource is made of.

use crate::amdgpu_device::amdgpu_device;
use crate::regs::*;
use crate::ttm::{ttm_resource, ttm_resource_priv};

/// `struct amdgpu_res_cursor`; `node` is an index into the resource's
/// blocks.
pub struct amdgpu_res_cursor {
    pub start: u64,
    pub size: u64,
    pub remaining: u64,
    node: Option<usize>,
    mem_type: u32,
}

fn block_range(adev: &amdgpu_device, res: &ttm_resource, i: usize) -> (u64, u64) {
    match &res.priv_ {
        ttm_resource_priv::vram { blocks, .. } => {
            let mm = &adev.mman.vram_mgr.as_ref().unwrap().mm;
            (crate::amdgpu_vram_mgr::amdgpu_vram_mgr_block_start(mm, blocks[i]),
             crate::amdgpu_vram_mgr::amdgpu_vram_mgr_block_size(mm, blocks[i]))
        }
        ttm_resource_priv::range { node } => (node.start << 12, node.size << 12),
        ttm_resource_priv::none => (0, 0),
    }
}

/// `amdgpu_res_first`: `size` bytes of `res` from byte `start`.
pub fn amdgpu_res_first(adev: &amdgpu_device, res: Option<&ttm_resource>, mut start: u64, size: u64)
                        -> amdgpu_res_cursor {
    let fallback = amdgpu_res_cursor { start, size, remaining: size, node: None, mem_type: TTM_PL_SYSTEM };
    let Some(res) = res else { return fallback };
    let blocks = match &res.priv_ {
        ttm_resource_priv::vram { blocks, .. } if res.mem_type == TTM_PL_VRAM => blocks.len(),
        ttm_resource_priv::range { .. }
            if res.mem_type == TTM_PL_TT || res.mem_type == AMDGPU_PL_DOORBELL
                || res.mem_type == AMDGPU_PL_MMIO_REMAP => 1,
        _ => return fallback,
    };
    if blocks == 0 {
        return fallback;
    }
    let mut i = 0;
    let (mut bstart, mut bsize) = block_range(adev, res, i);
    while start >= bsize {
        start -= bsize;
        if i + 1 < blocks {
            i += 1;
            (bstart, bsize) = block_range(adev, res, i);
        }
    }
    amdgpu_res_cursor { start: bstart + start, size: (bsize - start).min(size), remaining: size, node: Some(i),
                        mem_type: res.mem_type }
}

/// `amdgpu_res_next`.
pub fn amdgpu_res_next(adev: &amdgpu_device, res: Option<&ttm_resource>, cur: &mut amdgpu_res_cursor, size: u64) {
    cur.remaining -= size;
    if cur.remaining == 0 {
        return;
    }
    cur.size -= size;
    if cur.size != 0 {
        cur.start += size;
        return;
    }
    let (Some(res), Some(i)) = (res, cur.node) else { return };
    if cur.mem_type != TTM_PL_VRAM && cur.mem_type != TTM_PL_TT && cur.mem_type != AMDGPU_PL_DOORBELL
        && cur.mem_type != AMDGPU_PL_MMIO_REMAP {
        return;
    }
    let (bstart, bsize) = block_range(adev, res, i + 1);
    cur.node = Some(i + 1);
    cur.start = bstart;
    cur.size = bsize.min(cur.remaining);
}
