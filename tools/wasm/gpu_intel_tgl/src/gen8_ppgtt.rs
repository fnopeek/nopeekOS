//! gt/gen8_ppgtt.c, gt/intel_ppgtt.c and the scratch-page part of
//! gt/intel_gtt.c (Linux 6.18.55): the 4-level PPGTT on Tiger Lake, as
//! `i915_ppgtt_create` leaves it: scratch pages for every level and a
//! top-level directory pointing at scratch; contexts carry its top-level
//! address (`ASSIGN_CTX_PML4`). Binding first takes every table the range
//! can need into a stash (`i915_vm_alloc_pt_stash`), so that filling the
//! tree (`gen8_ppgtt_alloc`) cannot fail halfway, then writes 4K PTEs
//! (`gen8_ppgtt_insert_pte`); the 64K/2M path of `gen8_ppgtt_insert_huge`
//! is not ported, so contiguous objects are mapped in 4K pages. Unbinding
//! (`gen8_ppgtt_clear`) points the PTEs back at scratch and releases tables
//! whose use count drops to zero. A released table page goes back to the
//! kernel once no engine can reach it through its TLB: it waits for the
//! next full invalidation (`release_retired`, after
//! `intel_gt_invalidate_tlb_full`); a stashed page the walk did not take
//! never reached the card and goes back at once.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::drm_mm::drm_mm;
use crate::i915_drv::drm_i915_private;
use crate::i915_vma::{drm_i915_gem_object, i915_gem_object_create_internal};
use crate::regs::*;

const ENOMEM: i32 = 12;
/// Entries in a page-table page (`I915_PDES`).
const PDES: usize = 512;
/// `vm->top` for a 4-level table.
const TOP: usize = 3;
/// `vm->pd_shift`: the bytes one page table maps, as a shift.
const PD_SHIFT: u32 = 21;

/// `struct i915_page_table`; with `entry`, `struct i915_page_directory`.
pub struct i915_page_directory {
    pub obj: drm_i915_gem_object,
    /// `pt->used`: PTEs in use in a page table, entries set in a directory.
    pub used: u32,
    /// `pd->entry`; None for a page table.
    pub entry: Option<Box<[Option<Box<i915_page_directory>>; PDES]>>,
}

impl i915_page_directory {
    fn new(obj: drm_i915_gem_object, directory: bool) -> Box<i915_page_directory> {
        let entry = directory.then(|| Box::new([const { None }; PDES]));
        Box::new(i915_page_directory { obj, used: 0, entry })
    }

    /// `pd->entry[idx]`.
    fn child(&self, idx: usize) -> Option<&i915_page_directory> {
        self.entry.as_ref()?[idx].as_deref()
    }

    /// Tables at and below this one.
    fn count(&self) -> u32 {
        1 + self.entry.iter().flat_map(|e| e.iter().flatten()).map(|c| c.count()).sum::<u32>()
    }
}

/// Table pages taken out of a tree, waiting for a TLB invalidation.
static RETIRED: spin::Mutex<Vec<drm_i915_gem_object>> = spin::Mutex::new(Vec::new());

/// `alloc_pt` / `alloc_pd`.
fn alloc_pt() -> Result<drm_i915_gem_object, i32> {
    alloc_pt_dma(I915_GTT_PAGE_SIZE_4K)
}

/// `free_px`: the page leaves the tree; it goes back to the kernel after
/// the next full TLB invalidation.
fn free_px(obj: drm_i915_gem_object) {
    RETIRED.lock().push(obj);
}

/// A page the card never reached goes back now.
fn free_unused(obj: drm_i915_gem_object) {
    if let Some(d) = obj.map() {
        d.free();
    }
}

/// After a full TLB invalidation: every retired table page goes back.
pub fn release_retired() {
    for obj in RETIRED.lock().drain(..) {
        free_unused(obj);
    }
}

/// Table pages waiting for a TLB invalidation.
pub fn retired_tables() -> usize {
    RETIRED.lock().len()
}

/// `struct i915_vm_pt_stash`: the tables a bind may need, taken before the
/// tree is touched; [0] page tables, [1] directories.
struct i915_vm_pt_stash {
    pt: [Vec<drm_i915_gem_object>; 2],
}

/// `pd_count`: tables of `1 << shift` bytes a range of `size` bytes can
/// touch, misalignment included.
fn pd_count(size: u64, shift: u32) -> u64 {
    (size + 2 * ((1u64 << shift) - 1)) >> shift
}

/// `i915_vm_free_pt_stash`: what the bind did not use goes back.
fn i915_vm_free_pt_stash(stash: &mut i915_vm_pt_stash) {
    for l in stash.pt.iter_mut() {
        for obj in l.drain(..) {
            free_unused(obj);
        }
    }
}

/// `i915_vm_alloc_pt_stash`.
fn i915_vm_alloc_pt_stash(size: u64) -> Result<i915_vm_pt_stash, i32> {
    let mut stash = i915_vm_pt_stash { pt: [Vec::new(), Vec::new()] };
    let mut shift = PD_SHIFT;
    for n in 0..TOP {
        if n > 0 {
            shift += GEN8_PDES.trailing_zeros();
        }
        for _ in 0..pd_count(size, shift) {
            match alloc_pt() {
                Ok(obj) => stash.pt[(n > 0) as usize].push(obj),
                Err(e) => {
                    i915_vm_free_pt_stash(&mut stash);
                    return Err(e);
                }
            }
        }
    }
    Ok(stash)
}

/// `set_pd_entry`: link and count.
fn set_pd_entry(pd: &mut i915_page_directory, idx: usize, to: Box<i915_page_directory>) -> Result<(), i32> {
    write_pd_entry(&pd.obj, idx, gen8_pde_encode(to.obj.phys, I915_CACHE_LLC))?;
    pd.entry.as_mut().ok_or(-EINVAL)?[idx] = Some(to);
    pd.used += 1;
    Ok(())
}

/// `clear_pd_entry`: entry `idx` of `pd` back at `scratch`, the link and the
/// count dropped; the table is returned.
fn clear_pd_entry(pd: &mut i915_page_directory, idx: usize, scratch: u64) -> Result<Box<i915_page_directory>, i32> {
    write_pd_entry(&pd.obj, idx, scratch)?;
    let t = pd.entry.as_mut().and_then(|e| e[idx].take()).ok_or(-EINVAL)?;
    pd.used -= 1;
    Ok(t)
}

/// `__gen8_ppgtt_cleanup`: the table and everything below it to the pool.
fn __gen8_ppgtt_cleanup(pd: Box<i915_page_directory>) {
    let pd = *pd;
    if let Some(entry) = pd.entry {
        for child in entry.into_iter().flatten() {
            __gen8_ppgtt_cleanup(child);
        }
    }
    free_px(pd.obj);
}

/// `__gen8_ppgtt_alloc`: walk `[*start, end)` (in pages) below `pd` at level
/// `lvl`, linking tables from the stash where there are none. Fails only
/// if a write to a table page the module owns is refused.
fn __gen8_ppgtt_alloc(pd: &mut i915_page_directory, scratch: &[u64; TOP + 1], stash: &mut i915_vm_pt_stash,
                      start: &mut u64, end: u64, lvl: usize) -> Result<(), i32> {
    let (mut idx, mut len) = gen8_pd_range(*start, end, lvl);
    let lvl = lvl - 1;
    loop {
        if pd.child(idx).is_none() {
            // pd_count bounds what the walk can take.
            let obj = stash.pt[(lvl > 0) as usize].pop().ok_or(-ENOMEM)?;
            if let Err(e) = fill_page_dma(&obj, scratch[lvl], PDES) {
                stash.pt[(lvl > 0) as usize].push(obj);
                return Err(e);
            }
            let t = i915_page_directory::new(obj, lvl > 0);
            set_pd_entry(pd, idx, t)?;
        }
        let pt = pd.entry.as_mut().ok_or(-EINVAL)?[idx].as_mut().ok_or(-EINVAL)?;
        if lvl > 0 {
            pt.used += 1;
            let r = __gen8_ppgtt_alloc(pt, scratch, stash, start, end, lvl);
            pt.used -= 1;
            r?;
        } else {
            let count = gen8_pt_count(*start, end);
            pt.used += count as u32;
            *start += count;
        }
        idx += 1;
        len -= 1;
        if len == 0 {
            return Ok(());
        }
    }
}

/// `__gen8_ppgtt_clear`.
fn __gen8_ppgtt_clear(pd: &mut i915_page_directory, scratch: &[u64; TOP + 1], mut start: u64, end: u64, lvl: usize)
                      -> Result<u64, i32> {
    let scratch_lvl = scratch[lvl];
    let (mut idx, mut len) = gen8_pd_range(start, end, lvl);
    let lvl = lvl - 1;
    loop {
        let pt = pd.entry.as_mut().ok_or(-EINVAL)?[idx].as_mut().ok_or(-EINVAL)?;
        let used = pt.used;
        pt.used += 1;
        if used >> gen8_pd_shift(1) != 0 && gen8_pd_contains(start, end, lvl) {
            let t = clear_pd_entry(pd, idx, scratch_lvl)?;
            __gen8_ppgtt_cleanup(t);
            start += (PDES as u64) << gen8_pd_shift(lvl);
        } else {
            if lvl > 0 {
                start = __gen8_ppgtt_clear(pt, scratch, start, end, lvl)?;
            } else {
                let count = gen8_pt_count(start, end);
                let pte = gen8_pd_index(start, 0);
                let mut buf = [0u8; 4096];
                for k in 0..count as usize {
                    buf[k * 8..k * 8 + 8].copy_from_slice(&scratch[0].to_le_bytes());
                }
                let d = pt.obj.map().ok_or(-ENOMEM)?;
                if !d.write((pte * 8) as u32, &buf[..count as usize * 8]) {
                    return Err(-ENOMEM);
                }
                pt.used -= count as u32;
                start += count;
            }
            // release_pd_entry
            let pt = pd.entry.as_mut().ok_or(-EINVAL)?[idx].as_mut().ok_or(-EINVAL)?;
            if pt.used != 1 {
                pt.used -= 1;
            } else {
                pt.used = 0;
                let t = clear_pd_entry(pd, idx, scratch_lvl)?;
                __gen8_ppgtt_cleanup(t);
            }
        }
        idx += 1;
        len -= 1;
        if len == 0 {
            return Ok(start);
        }
    }
}

/// `struct i915_ppgtt`.
pub struct i915_ppgtt {
    /// `ppgtt->pd`, the top-level directory (PML4) and the tree below it.
    pub pd: Box<i915_page_directory>,
    /// `vm->scratch[0..=top]` and their encodings.
    pub scratch: [drm_i915_gem_object; TOP + 1],
    pub scratch_encode: [u64; TOP + 1],
    /// `vm->total`.
    pub total: u64,
    /// `vm->mm`.
    pub mm: drm_mm,
}

impl i915_ppgtt {
    /// `gen8_ppgtt_cleanup`: the tree and the scratch pages go back after
    /// the next full TLB invalidation.
    pub fn cleanup(self) {
        __gen8_ppgtt_cleanup(self.pd);
        for obj in self.scratch {
            free_px(obj);
        }
    }

    /// `px_dma(ppgtt->pd)`.
    pub fn pd_dma(&self) -> u64 {
        self.pd.obj.phys
    }

    /// Tables below the top directory.
    pub fn tables(&self) -> u32 {
        self.pd.count() - 1
    }

    /// The page table holding PTE `idx`.
    fn pt_for(&self, idx: u64) -> Option<drm_i915_gem_object> {
        let pdp = self.pd.child(gen8_pd_index(idx, 3))?;
        let pd = pdp.child(gen8_pd_index(idx, 2))?;
        pd.child(gen8_pd_index(idx, 1)).map(|pt| pt.obj)
    }

    /// `i915_vm_alloc_pt_stash`, `gen8_ppgtt_alloc` and
    /// `i915_vm_free_pt_stash`: the tree for `[start, start + length)`. A
    /// refusal leaves the tree as it was.
    pub fn gen8_ppgtt_alloc(&mut self, start: u64, length: u64) -> Result<(), i32> {
        if length == 0 || start + length > self.total {
            return Err(-EINVAL);
        }
        let mut stash = i915_vm_alloc_pt_stash(length)?;
        let first = start >> GEN8_PTE_SHIFT;
        let end = first + (length >> GEN8_PTE_SHIFT);
        let mut at = first;
        let scratch = self.scratch_encode;
        let r = __gen8_ppgtt_alloc(&mut self.pd, &scratch, &mut stash, &mut at, end, TOP);
        if r.is_err() && at > first {
            let _ = __gen8_ppgtt_clear(&mut self.pd, &scratch, first, at, TOP);
        }
        i915_vm_free_pt_stash(&mut stash);
        r
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
            let pt = self.pt_for(idx).ok_or(-EINVAL)?;
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
            let pt = self.pt_for(idx).ok_or(-EINVAL)?;
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

    /// `gen8_ppgtt_clear`.
    pub fn gen8_ppgtt_clear(&mut self, start: u64, length: u64) -> Result<(), i32> {
        if length == 0 || start + length > self.total {
            return Err(-EINVAL);
        }
        let first = start >> GEN8_PTE_SHIFT;
        let end = first + (length >> GEN8_PTE_SHIFT);
        let scratch = self.scratch_encode;
        __gen8_ppgtt_clear(&mut self.pd, &scratch, first, end, TOP).map(|_| ())
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

/// `write_dma_entry`: entry `idx` of the table page `pd`.
fn write_pd_entry(pd: &drm_i915_gem_object, idx: usize, encoded: u64) -> Result<(), i32> {
    let d = pd.map().ok_or(-ENOMEM)?;
    if d.write((idx * 8) as u32, &encoded.to_le_bytes()) { Ok(()) } else { Err(-ENOMEM) }
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

/// `POISON_FREE`, the scratch fill of a CONFIG_DRM_I915_DEBUG_GEM build:
/// a byte that is no longer this was written by the GPU through an
/// address nothing is bound at.
const SCRATCH_POISON: u8 = 0x6b;

/// `poison_scratch_page`, as with CONFIG_DRM_I915_DEBUG_GEM.
fn poison_scratch_page(obj: &drm_i915_gem_object) -> Result<(), i32> {
    let d = obj.map().ok_or(-ENOMEM)?;
    let page = [SCRATCH_POISON; 4096];
    let mut off = 0u64;
    while off < obj.size {
        if !d.write(off as u32, &page) {
            return Err(-ENOMEM);
        }
        off += 4096;
    }
    Ok(())
}

impl i915_ppgtt {
    /// The first byte of the scratch page that lost its poison, and its
    /// value; the page is poisoned again so that later writes show too.
    pub fn scratch_written(&self) -> Option<(u32, u8)> {
        let obj = &self.scratch[0];
        let d = obj.map()?;
        let mut page = [0u8; 4096];
        let mut off = 0u64;
        while off < obj.size {
            if !d.read(off as u32, &mut page) {
                return None;
            }
            if let Some(i) = page.iter().position(|&b| b != SCRATCH_POISON) {
                let _ = poison_scratch_page(obj);
                return Some((off as u32 + i as u32, page[i]));
            }
            off += 4096;
        }
        None
    }
}

/// `setup_scratch_page`: 64K on a 4-level table when the platform has 64K
/// pages but not 64K-only tables, and only if it comes out contiguous and
/// aligned; otherwise 4K. The page is poisoned as with
/// CONFIG_DRM_I915_DEBUG_GEM, so that stray GPU writes can be seen.
fn setup_scratch_page(i915: &drm_i915_private) -> Result<drm_i915_gem_object, i32> {
    let mut size = I915_GTT_PAGE_SIZE_4K;
    if i915.runtime.page_sizes as u64 & I915_GTT_PAGE_SIZE_64K != 0 && TGL_HAS_64K_PAGES == 0 {
        size = I915_GTT_PAGE_SIZE_64K;
    }
    loop {
        if let Ok(obj) = alloc_pt_dma(size) {
            if obj.phys & (size - 1) == 0 {
                poison_scratch_page(&obj)?;
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

    let mut pd = i915_page_directory::new(pd, true);
    // `atomic_inc(px_used(ppgtt->pd))`: the top directory stays.
    pd.used = 1;
    Ok(i915_ppgtt { pd, scratch, scratch_encode, total, mm: drm_mm::drm_mm_init(0, total) })
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
        if top != self.pd.obj.phys {
            l.s(" (vm pd ").hex((self.pd.obj.phys >> 32) as u32).s("_").hex(self.pd.obj.phys as u32).s("!)");
        }
        let mut table: &i915_page_directory = &self.pd;
        let pfn = va >> GEN8_PTE_SHIFT;
        for lvl in (0..=TOP).rev() {
            let idx = gen8_pd_index(pfn, lvl);
            let Some(d) = table.obj.map() else {
                l.s(" | L").dec(lvl as u64).s(" unmapped").dbg();
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
            match table.child(idx) {
                Some(next) => {
                    if e & !0xfff & ((1 << 48) - 1) != next.obj.phys {
                        l.s(" (table at ").hex((next.obj.phys >> 32) as u32).s("_").hex(next.obj.phys as u32).s("!)");
                    }
                    table = next;
                }
                None => {
                    l.s(" (no table)").dbg();
                    return;
                }
            }
        }
        l.dbg();
    }
}
