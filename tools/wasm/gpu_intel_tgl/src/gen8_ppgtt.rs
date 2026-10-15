//! gt/gen8_ppgtt.c, gt/intel_ppgtt.c and the scratch-page part of
//! gt/intel_gtt.c (Linux 6.18.55): the 4-level PPGTT of the kernel's
//! address space (`gt->vm`) on Tiger Lake, as `i915_ppgtt_create` leaves it:
//! scratch pages for every level and a top-level directory pointing at
//! scratch; contexts carry its top-level address (`ASSIGN_CTX_PML4`).
//! Binding allocates the page-table tree (`gen8_ppgtt_alloc`) and writes
//! 4K PTEs (`gen8_ppgtt_insert_pte`); the 64K/2M path of
//! `gen8_ppgtt_insert_huge` is not ported, so contiguous objects are
//! mapped in 4K pages. Unbinding (`gen8_ppgtt_clear`) points the PTEs back
//! at scratch and releases tables whose use count drops to zero; their
//! pages go to a stash the next allocation takes from, since DMA pages
//! cannot be given back to the kernel.

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
/// module has no heap that frees, so they sit in a fixed table. 2048 page
/// tables map 4 GB in 4K pages.
const MAX_PT: usize = 2048;
/// Released table pages kept for reuse; as many as there can be tables, so
/// none is lost when a client's address space is emptied.
const MAX_STASH: usize = MAX_PT;

/// A page-table page below the top directory (`struct i915_page_table` /
/// `i915_page_directory`): `entry[idx]` of the table at `parent`.
#[derive(Clone, Copy)]
pub struct pt_node {
    pub parent: u64,
    pub idx: u16,
    pub obj: drm_i915_gem_object,
    /// `pt->used`: PTEs in use in a page table, entries set in a directory.
    pub used: u32,
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
    /// Released table pages (`free_px`), taken by `alloc_pt_dma` first.
    pub stash: [Option<drm_i915_gem_object>; MAX_STASH],
}

impl i915_ppgtt {
    /// `px_dma(ppgtt->pd)`.
    pub fn pd_dma(&self) -> u64 {
        self.pd.phys
    }

    /// `pd->entry[idx]` of the table at `parent`.
    fn entry(&self, parent: u64, idx: usize) -> Option<drm_i915_gem_object> {
        self.slot(parent, idx).and_then(|i| self.pts[i]).map(|n| n.obj)
    }

    /// The slot in `pts` of `pd->entry[idx]` of the table at `parent`.
    fn slot(&self, parent: u64, idx: usize) -> Option<usize> {
        self.pts.iter().position(|n| n.is_some_and(|n| n.parent == parent && n.idx as usize == idx))
    }

    /// `px_used(pd)` of the table at `phys`; the top directory is not counted.
    fn used_add(&mut self, phys: u64, delta: i32) {
        if let Some(n) = self.pts.iter_mut().flatten().find(|n| n.obj.phys == phys) {
            n.used = n.used.wrapping_add(delta as u32);
        }
    }

    /// `alloc_pt_dma`, from the stash first.
    fn alloc_pt(&mut self) -> Result<drm_i915_gem_object, i32> {
        match self.stash.iter_mut().find(|x| x.is_some()) {
            Some(x) => Ok(x.take().unwrap()),
            None => alloc_pt_dma(I915_GTT_PAGE_SIZE_4K),
        }
    }

    /// `free_px`: the page goes to the stash (lost when it is full).
    fn free_px(&mut self, obj: drm_i915_gem_object) {
        if let Some(x) = self.stash.iter_mut().find(|x| x.is_none()) {
            *x = Some(obj);
        }
    }

    /// `set_pd_entry`: link and count.
    fn set_pd_entry(&mut self, pd: &drm_i915_gem_object, idx: usize, to: &drm_i915_gem_object) -> Result<(), i32> {
        set_pd_entry(pd, idx, to)?;
        self.used_add(pd.phys, 1);
        Ok(())
    }

    /// `clear_pd_entry`: point entry `idx` of `pd` (at level `lvl`) back at
    /// scratch, drop the link and the count.
    fn clear_pd_entry(&mut self, pd: &drm_i915_gem_object, idx: usize, lvl: usize) -> Result<(), i32> {
        let d = pd.map().ok_or(-ENOMEM)?;
        if !d.write((idx * 8) as u32, &self.scratch_encode[lvl].to_le_bytes()) {
            return Err(-ENOMEM);
        }
        if let Some(i) = self.slot(pd.phys, idx) {
            self.pts[i] = None;
        }
        self.used_add(pd.phys, -1);
        Ok(())
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
                    let pt = self.alloc_pt()?;
                    fill_page_dma(&pt, self.scratch_encode[lvl], PDES)?;
                    self.pts[slot] = Some(pt_node { parent: pd.phys, idx: idx as u16, obj: pt, used: 0 });
                    self.set_pd_entry(&pd, idx, &pt)?;
                    pt
                }
            };
            if lvl > 0 {
                self.used_add(pt.phys, 1);
                let r = self.__gen8_ppgtt_alloc(pt, start, end, lvl);
                self.used_add(pt.phys, -1);
                r?;
            } else {
                let count = gen8_pt_count(*start, end);
                self.used_add(pt.phys, count as i32);
                *start += count;
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

    /// `gen8_ppgtt_insert_pte` for a list of pages, as from a
    /// scatterlist: page `i` of `pages` at `start + i * 4K`. The tree must
    /// have been allocated.
    pub fn gen8_ppgtt_insert_pages(&mut self, start: u64, pages: &[u64], pat_index: u32, flags: u32) -> Result<(), i32> {
        let pte_encode = gen12_pte_encode(0, pat_index, flags);
        let mut idx = start >> GEN8_PTE_SHIFT;
        let mut at = 0usize;
        while at < pages.len() {
            let pdp = self.entry(self.pd.phys, gen8_pd_index(idx, 3)).ok_or(-EINVAL)?;
            let pd = self.entry(pdp.phys, gen8_pd_index(idx, 2)).ok_or(-EINVAL)?;
            let pt = self.entry(pd.phys, gen8_pd_index(idx, 1)).ok_or(-EINVAL)?;
            let first = gen8_pd_index(idx, 0);
            let n = (PDES - first).min(pages.len() - at);
            let mut buf = [0u8; 4096];
            for i in 0..n {
                buf[i * 8..i * 8 + 8].copy_from_slice(&(pte_encode | pages[at + i]).to_le_bytes());
            }
            let d = pt.map().ok_or(-ENOMEM)?;
            if !d.write((first * 8) as u32, &buf[..n * 8]) {
                return Err(-ENOMEM);
            }
            idx += n as u64;
            at += n;
        }
        Ok(())
    }

    /// `__gen8_ppgtt_cleanup`: release the table at `pd` (level `lvl`) and
    /// everything below it.
    fn __gen8_ppgtt_cleanup(&mut self, pd: drm_i915_gem_object, lvl: usize) {
        if lvl > 0 {
            for idx in 0..PDES {
                if let Some(i) = self.slot(pd.phys, idx) {
                    let child = self.pts[i].unwrap().obj;
                    self.pts[i] = None;
                    self.__gen8_ppgtt_cleanup(child, lvl - 1);
                }
            }
        }
        self.free_px(pd);
    }

    /// `release_pd_entry`: drop one use of `pt` (entry `idx` of `pd` at
    /// level `lvl`); true when it was the last and the entry is cleared.
    fn release_pd_entry(&mut self, pd: &drm_i915_gem_object, idx: usize, pt: &drm_i915_gem_object, lvl: usize)
                        -> Result<bool, i32> {
        let used = self.pts.iter().flatten().find(|n| n.obj.phys == pt.phys).map(|n| n.used).unwrap_or(0);
        if used != 1 {
            self.used_add(pt.phys, -1);
            return Ok(false);
        }
        self.used_add(pt.phys, -1);
        self.clear_pd_entry(pd, idx, lvl)?;
        Ok(true)
    }

    /// `__gen8_ppgtt_clear`.
    fn __gen8_ppgtt_clear(&mut self, pd: drm_i915_gem_object, mut start: u64, end: u64, lvl: usize)
                          -> Result<u64, i32> {
        let scratch_lvl = lvl;
        let (mut idx, mut len) = gen8_pd_range(start, end, lvl);
        let lvl = lvl - 1;
        loop {
            let Some(i) = self.slot(pd.phys, idx) else { return Err(-EINVAL) };
            let node = self.pts[i].unwrap();
            let pt = node.obj;
            self.pts[i].as_mut().unwrap().used += 1;
            if node.used >> gen8_pd_shift(1) != 0 && gen8_pd_contains(start, end, lvl) {
                self.clear_pd_entry(&pd, idx, scratch_lvl)?;
                self.__gen8_ppgtt_cleanup(pt, lvl);
                start += (PDES as u64) << gen8_pd_shift(lvl);
            } else {
                if lvl > 0 {
                    start = self.__gen8_ppgtt_clear(pt, start, end, lvl)?;
                } else {
                    let count = gen8_pt_count(start, end);
                    let pte = gen8_pd_index(start, 0);
                    let mut buf = [0u8; 4096];
                    for k in 0..count as usize {
                        buf[k * 8..k * 8 + 8].copy_from_slice(&self.scratch_encode[0].to_le_bytes());
                    }
                    let d = pt.map().ok_or(-ENOMEM)?;
                    if !d.write((pte * 8) as u32, &buf[..count as usize * 8]) {
                        return Err(-ENOMEM);
                    }
                    self.used_add(pt.phys, -(count as i32));
                    start += count;
                }
                if self.release_pd_entry(&pd, idx, &pt, scratch_lvl)? {
                    self.free_px(pt);
                }
            }
            idx += 1;
            len -= 1;
            if len == 0 {
                return Ok(start);
            }
        }
    }

    /// `gen8_ppgtt_clear`.
    pub fn gen8_ppgtt_clear(&mut self, start: u64, length: u64) -> Result<(), i32> {
        if length == 0 || start + length > self.total {
            return Err(-EINVAL);
        }
        let first = start >> GEN8_PTE_SHIFT;
        let end = first + (length >> GEN8_PTE_SHIFT);
        let pd = self.pd;
        self.__gen8_ppgtt_clear(pd, first, end, TOP).map(|_| ())
    }
}

const EINVAL: i32 = 22;

/// `gen8_pd_shift(lvl)`.
fn gen8_pd_shift(lvl: usize) -> u32 {
    lvl as u32 * GEN8_PDES.trailing_zeros()
}

/// `gen8_pd_contains`: `[start, end)` covers the whole table at `lvl`.
fn gen8_pd_contains(start: u64, end: u64, lvl: usize) -> bool {
    let mask = !0u64 << gen8_pd_shift(lvl + 1);
    (start ^ end) & mask != 0 && (start & !mask) == 0
}

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

    Ok(i915_ppgtt { pd, scratch, scratch_encode, total, mm: drm_mm::drm_mm_init(0, total), pts: [None; MAX_PT],
                    stash: [None; MAX_STASH] })
}

impl i915_ppgtt {
    /// Walks `va` the way the GPU does, from `top` (the PML4 address taken
    /// from a context image) down to the PTE, reading every entry back from
    /// memory, and logs each level. A diagnostic for a copy that never
    /// completes: it shows which level does not lead where the binding put it.
    pub fn walk_log(&self, top: u64, va: u64) {
        let mut l = crate::linux::Line::new();
        l.s("walk ").hex((va >> 32) as u32).s("_").hex(va as u32).s(": top ").hex((top >> 32) as u32).s("_")
            .hex(top as u32);
        if top != self.pd.phys {
            l.s(" (vm pd ").hex((self.pd.phys >> 32) as u32).s("_").hex(self.pd.phys as u32).s("!)");
        }
        let mut table = self.pd;
        let pfn = va >> GEN8_PTE_SHIFT;
        for lvl in (0..=TOP).rev() {
            let idx = gen8_pd_index(pfn, lvl);
            let Some(d) = table.map() else {
                l.s(" | L").dec(lvl as u64).s(" unmapped").say();
                return;
            };
            let mut raw = [0u8; 8];
            if !d.read((idx * 8) as u32, &mut raw) {
                l.s(" | L").dec(lvl as u64).s(" read failed").say();
                return;
            }
            let e = u64::from_le_bytes(raw);
            l.s(" | L").dec(lvl as u64).s("[").dec(idx as u64).s("]=").hex((e >> 32) as u32).s("_").hex(e as u32);
            if lvl == 0 {
                break;
            }
            match self.entry(table.phys, idx) {
                Some(next) => {
                    if e & !0xfff & ((1 << 48) - 1) != next.phys {
                        l.s(" (table at ").hex((next.phys >> 32) as u32).s("_").hex(next.phys as u32).s("!)");
                    }
                    table = next;
                }
                None => {
                    l.s(" (no table)").say();
                    return;
                }
            }
        }
        l.say();
    }
}
