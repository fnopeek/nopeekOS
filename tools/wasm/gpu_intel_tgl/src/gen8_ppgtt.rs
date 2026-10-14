//! gt/gen8_ppgtt.c, gt/intel_ppgtt.c and the scratch-page part of
//! gt/intel_gtt.c (Linux 6.18.55): the 4-level PPGTT of the kernel's
//! address space (`gt->vm`) on Tiger Lake, as `i915_ppgtt_create` leaves it:
//! scratch pages for every level and a top-level directory pointing at
//! scratch. Nothing is bound into it here; contexts carry its top-level
//! address (`ASSIGN_CTX_PML4`).

use crate::i915_drv::drm_i915_private;
use crate::i915_vma::{drm_i915_gem_object, i915_gem_object_create_internal};
use crate::regs::*;

const ENOMEM: i32 = 12;
/// Entries in a page-table page (`I915_PDES`).
const PDES: usize = 512;
/// `vm->top` for a 4-level table.
const TOP: usize = 3;

/// `struct i915_ppgtt`.
#[derive(Clone, Copy)]
pub struct i915_ppgtt {
    /// `ppgtt->pd`, the top-level directory (PML4).
    pub pd: drm_i915_gem_object,
    /// `vm->scratch[0..=top]` and their encodings.
    pub scratch: [drm_i915_gem_object; TOP + 1],
    pub scratch_encode: [u64; TOP + 1],
    /// `vm->total`.
    pub total: u64,
}

impl i915_ppgtt {
    /// `px_dma(ppgtt->pd)`.
    pub fn pd_dma(&self) -> u64 {
        self.pd.phys
    }
}

/// `gen8_pde_encode`.
fn gen8_pde_encode(addr: u64, cache_level: u32) -> u64 {
    let mut pde = addr | GEN8_PAGE_PRESENT | GEN8_PAGE_RW;
    if cache_level != I915_CACHE_NONE {
        pde |= PPAT_CACHED_PDE as u64;
    } else {
        pde |= PPAT_UNCACHED;
    }
    pde
}

/// `gen12_pte_encode`.
fn gen12_pte_encode(addr: u64, pat_index: u32, flags: u32) -> u64 {
    let mut pte = addr | GEN8_PAGE_PRESENT | GEN8_PAGE_RW;
    if flags & PTE_READ_ONLY as u32 != 0 {
        pte &= !GEN8_PAGE_RW;
    }
    if flags & PTE_LM as u32 != 0 {
        pte |= GEN12_PPGTT_PTE_LM;
    }
    if pat_index & 1 != 0 {
        pte |= GEN12_PPGTT_PTE_PAT0;
    }
    if pat_index & 2 != 0 {
        pte |= GEN12_PPGTT_PTE_PAT1;
    }
    if pat_index & 4 != 0 {
        pte |= GEN12_PPGTT_PTE_PAT2;
    }
    if pat_index & 8 != 0 {
        pte |= MTL_PPGTT_PTE_PAT3;
    }
    pte
}

/// `fill_page_dma`: `count` entries of `val` at the start of the page.
fn fill_page_dma(obj: &drm_i915_gem_object, val: u64, count: usize) -> Result<(), i32> {
    let d = obj.map().ok_or(-ENOMEM)?;
    let mut page = [0u8; 4096];
    for i in 0..count.min(PDES) {
        page[i * 8..i * 8 + 8].copy_from_slice(&val.to_le_bytes());
    }
    if d.write(0, &page[..count.min(PDES) * 8]) { Ok(()) } else { Err(-ENOMEM) }
}

/// `alloc_pt_dma`: a page-table page in system memory.
fn alloc_pt_dma(size: u64) -> Result<drm_i915_gem_object, i32> {
    i915_gem_object_create_internal(size)
}

/// `setup_scratch_page`: 64K on a 4-level table when the platform has 64K
/// pages but not 64K-only tables, and only if it comes out contiguous and
/// aligned; otherwise 4K. The page stays cleared (no DRM_I915_DEBUG_GEM).
fn setup_scratch_page(i915: &drm_i915_private) -> Result<drm_i915_gem_object, i32> {
    let mut size = I915_GTT_PAGE_SIZE_4K;
    if i915.runtime.page_sizes as u64 & I915_GTT_PAGE_SIZE_64K != 0 && TGL_HAS_64K_PAGES == 0 {
        size = I915_GTT_PAGE_SIZE_64K;
    }
    loop {
        if let Ok(obj) = alloc_pt_dma(size) {
            if obj.phys & (size - 1) == 0 {
                return Ok(obj);
            }
        }
        if size == I915_GTT_PAGE_SIZE_4K {
            return Err(-ENOMEM);
        }
        size = I915_GTT_PAGE_SIZE_4K;
    }
}

/// `gen8_init_scratch`: no read-only support on GRAPHICS_VER 11/12, so no
/// sharing of another vm's scratch; each level points at the one below.
fn gen8_init_scratch(i915: &drm_i915_private) -> Result<([drm_i915_gem_object; TOP + 1], [u64; TOP + 1]), i32> {
    let s0 = setup_scratch_page(i915)?;
    let mut scratch = [s0; TOP + 1];
    let mut encode = [0u64; TOP + 1];
    // pte_flags = vm->has_read_only (false), not local memory.
    encode[0] = gen12_pte_encode(s0.phys, TGL_PAT_CACHE_NONE, 0);
    for i in 1..=TOP {
        let obj = alloc_pt_dma(I915_GTT_PAGE_SIZE_4K)?;
        fill_page_dma(&obj, encode[i - 1], PDES)?;
        encode[i] = gen8_pde_encode(obj.phys, I915_CACHE_NONE);
        scratch[i] = obj;
    }
    Ok((scratch, encode))
}

/// `gen8_pd_top_count`.
fn gen8_pd_top_count(total: u64) -> usize {
    let shift = GEN8_PTE_SHIFT + TOP as u32 * (GEN8_PDES as u32).trailing_zeros();
    ((total + (1u64 << shift) - 1) >> shift) as usize
}

/// `i915_ppgtt_create` → `gen8_ppgtt_create`: 48-bit (4-level), no vGPU,
/// Wa_16018031267 (12.55–12.71) does not apply.
pub fn i915_ppgtt_create(i915: &drm_i915_private) -> Result<i915_ppgtt, i32> {
    let total = 1u64 << i915.runtime.ppgtt_size;
    let (scratch, scratch_encode) = gen8_init_scratch(i915)?;

    // gen8_alloc_top_pd
    let count = gen8_pd_top_count(total);
    let pd = alloc_pt_dma(I915_GTT_PAGE_SIZE_4K)?;
    fill_page_dma(&pd, scratch_encode[TOP], count)?;

    Ok(i915_ppgtt { pd, scratch, scratch_encode, total })
}
