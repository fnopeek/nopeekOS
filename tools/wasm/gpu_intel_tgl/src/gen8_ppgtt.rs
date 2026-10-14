//! gt/gen8_ppgtt.c, gt/intel_ppgtt.c and the scratch-page part of
//! gt/intel_gtt.c (Linux 6.18.55): the 4-level PPGTT of the kernel's
//! address space (`gt->vm`) on Tiger Lake, as `i915_ppgtt_create` leaves it:
//! scratch pages for every level and a top-level directory pointing at
//! scratch; contexts carry its top-level address (`ASSIGN_CTX_PML4`).
//! Binding allocates the page-table tree (`gen8_ppgtt_alloc`) and writes
//! 4K PTEs (`gen8_ppgtt_insert_pte`); the 64K/2M path of
//! `gen8_ppgtt_insert_huge` is not ported, so contiguous objects are
//! mapped in 4K pages.

use crate::drm_mm::drm_mm;
use crate::i915_drv::drm_i915_private;
use crate::i915_vma::{drm_i915_gem_object, i915_gem_object_create_internal};
use crate::regs::*;

const ENOMEM: i32 = 12;
/// Entries in a page-table page (`I915_PDES`).
const PDES: usize = 512;
/// `vm->top` for a 4-level table.
const TOP: usize = 3;
/// Page-table pages below the top directory one address space holds; the
/// module has no heap that frees, so they sit in a fixed table.
const MAX_PT: usize = 192;

/// A page-table page below the top directory (`struct i915_page_table` /
/// `i915_page_directory`): `entry[idx]` of the table at `parent`.
#[derive(Clone, Copy)]
pub struct pt_node {
    pub parent: u64,
    pub idx: u16,
    pub obj: drm_i915_gem_object,
}
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
    /// `vm->mm`.
    pub mm: drm_mm,
    /// The allocated tree below `pd`.
    pub pts: [Option<pt_node>; MAX_PT],
}

impl i915_ppgtt {
    /// `px_dma(ppgtt->pd)`.
    pub fn pd_dma(&self) -> u64 {
        self.pd.phys
    }

    /// `pd->entry[idx]` of the table at `parent`.
    fn entry(&self, parent: u64, idx: usize) -> Option<drm_i915_gem_object> {
        self.pts.iter().flatten().find(|n| n.parent == parent && n.idx as usize == idx).map(|n| n.obj)
    }

    /// `__gen8_ppgtt_alloc`: walk `[*start, end)` (in pages) below the
    /// table `pd` at level `lvl`, filling missing tables with the scratch
    /// of the level below and linking them in (`set_pd_entry`).
    fn __gen8_ppgtt_alloc(&mut self, pd: drm_i915_gem_object, start: &mut u64, end: u64, lvl: usize) -> Result<(), i32> {
        let (mut idx, mut len) = gen8_pd_range(*start, end, lvl);
        let lvl = lvl - 1;
        loop {
            let pt = match self.entry(pd.phys, idx) {
                Some(pt) => pt,
                None => {
                    let slot = self.pts.iter().position(|n| n.is_none()).ok_or(-ENOMEM)?;
                    let pt = alloc_pt_dma(I915_GTT_PAGE_SIZE_4K)?;
                    fill_page_dma(&pt, self.scratch_encode[lvl], PDES)?;
                    self.pts[slot] = Some(pt_node { parent: pd.phys, idx: idx as u16, obj: pt });
                    set_pd_entry(&pd, idx, &pt)?;
                    pt
                }
            };
            if lvl > 0 {
                self.__gen8_ppgtt_alloc(pt, start, end, lvl)?;
            } else {
                *start += gen8_pt_count(*start, end);
            }
            idx += 1;
            len -= 1;
            if len == 0 {
                return Ok(());
            }
        }
    }

    /// `gen8_ppgtt_alloc`.
    pub fn gen8_ppgtt_alloc(&mut self, start: u64, length: u64) -> Result<(), i32> {
        let mut first = start >> GEN8_PTE_SHIFT;
        let end = first + (length >> GEN8_PTE_SHIFT);
        if length == 0 || start + length > self.total {
            return Err(-EINVAL);
        }
        let pd = self.pd;
        self.__gen8_ppgtt_alloc(pd, &mut first, end, TOP)
    }

    /// `gen8_ppgtt_insert` → `gen8_ppgtt_insert_pte` for one contiguous
    /// range of bus addresses: `size` bytes from `addr` at `start`. The
    /// tree must have been allocated. PTEs of one table go out in one
    /// write.
    pub fn gen8_ppgtt_insert(&mut self, start: u64, addr: u64, size: u64, pat_index: u32, flags: u32) -> Result<(), i32> {
        let pte_encode = gen12_pte_encode(0, pat_index, flags);
        let mut idx = start >> GEN8_PTE_SHIFT;
        let end = idx + (size >> GEN8_PTE_SHIFT);
        let mut dma = addr;
        while idx < end {
            let pdp = self.entry(self.pd.phys, gen8_pd_index(idx, 3)).ok_or(-EINVAL)?;
            let pd = self.entry(pdp.phys, gen8_pd_index(idx, 2)).ok_or(-EINVAL)?;
            let pt = self.entry(pd.phys, gen8_pd_index(idx, 1)).ok_or(-EINVAL)?;
            let first = gen8_pd_index(idx, 0);
            let n = (PDES - first).min((end - idx) as usize);
            let mut buf = [0u8; 4096];
            for i in 0..n {
                buf[i * 8..i * 8 + 8].copy_from_slice(&(pte_encode | dma).to_le_bytes());
                dma += I915_GTT_PAGE_SIZE;
            }
            let d = pt.map().ok_or(-ENOMEM)?;
            if !d.write((first * 8) as u32, &buf[..n * 8]) {
                return Err(-ENOMEM);
            }
            idx += n as u64;
        }
        Ok(())
    }
}

const EINVAL: i32 = 22;

/// `gen8_pd_index(i, lvl)`.
fn gen8_pd_index(i: u64, lvl: usize) -> usize {
    ((i >> (lvl as u32 * GEN8_PDES.trailing_zeros())) & (GEN8_PDES - 1)) as usize
}

/// `gen8_pd_range`: first index and count of entries at `lvl` that
/// `[start, end)` (in pages) touches.
fn gen8_pd_range(start: u64, end: u64, lvl: usize) -> (usize, usize) {
    let bits = GEN8_PDES.trailing_zeros();
    let shift = lvl as u32 * bits;
    let mask = !0u64 << ((lvl as u32 + 1) * bits);
    let end = end + (!mask >> bits);
    let idx = ((start >> shift) & (GEN8_PDES - 1)) as usize;
    if (start ^ end) & mask != 0 {
        (idx, PDES - idx)
    } else {
        (idx, ((end >> shift) & (GEN8_PDES - 1)) as usize - idx)
    }
}

/// `gen8_pt_count`.
fn gen8_pt_count(start: u64, end: u64) -> u64 {
    if (start ^ end) >> GEN8_PDES.trailing_zeros() != 0 {
        GEN8_PDES - (start & (GEN8_PDES - 1))
    } else {
        end - start
    }
}

/// `set_pd_entry`: the table's PDE is written cached (`I915_CACHE_LLC`).
fn set_pd_entry(pd: &drm_i915_gem_object, idx: usize, to: &drm_i915_gem_object) -> Result<(), i32> {
    let d = pd.map().ok_or(-ENOMEM)?;
    let pde = gen8_pde_encode(to.phys, I915_CACHE_LLC);
    if d.write((idx * 8) as u32, &pde.to_le_bytes()) { Ok(()) } else { Err(-ENOMEM) }
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

    Ok(i915_ppgtt { pd, scratch, scratch_encode, total, mm: drm_mm::drm_mm_init(0, total), pts: [None; MAX_PT] })
}
