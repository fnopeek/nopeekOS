//! amdgpu/amdgpu_vm_pt.c (Linux 6.18.55): the page table tree of a VM.
//!
//! A directory's entries are bases (`amdgpu_vm.rs`) made with the
//! directory's buffer and named by key in its `entries`; a cursor holds
//! keys where Linux holds pointers into those arrays.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_object as obj;
use crate::amdgpu_vm::{self, base, base_mut};
use crate::regs::*;
use alloc::vec::Vec;

/// `struct amdgpu_vm_pt_cursor`.
#[derive(Clone, Copy)]
pub struct amdgpu_vm_pt_cursor {
    pub pfn: u64,
    pub parent: Option<usize>,
    pub entry: Option<usize>,
    pub level: u32,
}

/// `amdgpu_vm_pt_level_shift`.
pub fn amdgpu_vm_pt_level_shift(adev: &amdgpu_device, level: u32) -> u32 {
    match level {
        AMDGPU_VM_PDB2 | AMDGPU_VM_PDB1 | AMDGPU_VM_PDB0 => 9 * (AMDGPU_VM_PDB0 - level) + adev.vm_manager.block_size,
        AMDGPU_VM_PTB => 0,
        _ => !0,
    }
}

/// `amdgpu_vm_pt_num_entries`.
pub fn amdgpu_vm_pt_num_entries(adev: &amdgpu_device, level: u32) -> u32 {
    let shift = amdgpu_vm_pt_level_shift(adev, adev.vm_manager.root_level);
    if level == adev.vm_manager.root_level {
        // For the root directory
        (crate::linux::align(adev.vm_manager.max_pfn, 1 << shift) >> shift) as u32
    } else if level != AMDGPU_VM_PTB {
        // Everything in between
        512
    } else {
        // For the page tables on the leaves
        1 << adev.vm_manager.block_size
    }
}

/// `amdgpu_vm_pt_entries_mask`.
fn amdgpu_vm_pt_entries_mask(adev: &amdgpu_device, level: u32) -> u32 {
    if level <= adev.vm_manager.root_level {
        0xffffffff
    } else if level != AMDGPU_VM_PTB {
        0x1ff
    } else {
        (1 << adev.vm_manager.block_size) - 1
    }
}

/// `amdgpu_vm_pt_size`.
fn amdgpu_vm_pt_size(adev: &amdgpu_device, level: u32) -> u64 {
    crate::linux::align(amdgpu_vm_pt_num_entries(adev, level) as u64 * 8, AMDGPU_GPU_PAGE_SIZE as u64)
}

/// `amdgpu_vm_pt_parent`.
fn amdgpu_vm_pt_parent(adev: &amdgpu_device, pt: usize) -> Option<usize> {
    let parent = obj::bo(adev, base(adev, pt).bo?).parent?;
    obj::bo(adev, parent).vm_bo
}

/// `amdgpu_vm_pt_start`.
fn amdgpu_vm_pt_start(adev: &amdgpu_device, vm: usize, start: u64) -> amdgpu_vm_pt_cursor {
    amdgpu_vm_pt_cursor { pfn: start, parent: None, entry: Some(adev.vm(vm).root), level: adev.vm_manager.root_level }
}

/// `amdgpu_vm_pt_descendant`.
pub fn amdgpu_vm_pt_descendant(adev: &amdgpu_device, c: &mut amdgpu_vm_pt_cursor) -> bool {
    let Some(e) = c.entry else { return false };
    let Some(h) = base(adev, e).bo else { return false };
    if c.level == AMDGPU_VM_PTB {
        return false;
    }
    let mask = amdgpu_vm_pt_entries_mask(adev, c.level);
    let shift = amdgpu_vm_pt_level_shift(adev, c.level);
    c.level += 1;
    let idx = ((c.pfn >> shift) as u32 & mask) as usize;
    c.parent = c.entry;
    c.entry = Some(obj::bo(adev, h).entries[idx]);
    true
}

/// `amdgpu_vm_pt_sibling`.
fn amdgpu_vm_pt_sibling(adev: &amdgpu_device, c: &mut amdgpu_vm_pt_cursor) -> bool {
    let Some(p) = c.parent else { return false };
    let shift = amdgpu_vm_pt_level_shift(adev, c.level - 1);
    let num_entries = amdgpu_vm_pt_num_entries(adev, c.level - 1) as usize;
    let parent = base(adev, p).bo.unwrap();
    let idx = base(adev, c.entry.unwrap()).pt_index;
    if idx == num_entries - 1 {
        return false;
    }
    c.pfn += 1 << shift;
    c.pfn &= !((1u64 << shift) - 1);
    c.entry = Some(obj::bo(adev, parent).entries[idx + 1]);
    true
}

/// `amdgpu_vm_pt_ancestor`.
pub fn amdgpu_vm_pt_ancestor(adev: &amdgpu_device, c: &mut amdgpu_vm_pt_cursor) -> bool {
    let Some(p) = c.parent else { return false };
    c.level -= 1;
    c.entry = Some(p);
    c.parent = amdgpu_vm_pt_parent(adev, p);
    true
}

/// `amdgpu_vm_pt_next`.
fn amdgpu_vm_pt_next(adev: &amdgpu_device, c: &mut amdgpu_vm_pt_cursor) {
    // First try a newborn child
    if amdgpu_vm_pt_descendant(adev, c) {
        return;
    }
    // If that didn't worked try to find a sibling
    while !amdgpu_vm_pt_sibling(adev, c) {
        // No sibling, go to our parents and grandparents
        if !amdgpu_vm_pt_ancestor(adev, c) {
            c.pfn = !0;
            return;
        }
    }
}

/// `amdgpu_vm_pt_first_dfs`.
fn amdgpu_vm_pt_first_dfs(adev: &amdgpu_device, vm: usize, start: Option<&amdgpu_vm_pt_cursor>)
                          -> amdgpu_vm_pt_cursor {
    let mut c = match start {
        Some(s) => *s,
        None => amdgpu_vm_pt_start(adev, vm, 0),
    };
    while amdgpu_vm_pt_descendant(adev, &mut c) {}
    c
}

/// `amdgpu_vm_pt_continue_dfs`.
fn amdgpu_vm_pt_continue_dfs(start: Option<&amdgpu_vm_pt_cursor>, entry: Option<usize>) -> bool {
    entry.is_some() && start.is_none_or(|s| entry != s.entry)
}

/// `amdgpu_vm_pt_next_dfs`.
fn amdgpu_vm_pt_next_dfs(adev: &amdgpu_device, c: &mut amdgpu_vm_pt_cursor) {
    if c.entry.is_none() {
        return;
    }
    if c.parent.is_none() {
        c.entry = None;
    } else if amdgpu_vm_pt_sibling(adev, c) {
        while amdgpu_vm_pt_descendant(adev, c) {}
    } else {
        amdgpu_vm_pt_ancestor(adev, c);
    }
}

/// `for_each_amdgpu_vm_pt_dfs_safe`: the entries in the order the loop
/// visits them, each read before the cursor moves past it.
fn for_each_dfs_safe(adev: &amdgpu_device, vm: usize, start: Option<&amdgpu_vm_pt_cursor>) -> Vec<usize> {
    let mut out = Vec::new();
    let mut c = amdgpu_vm_pt_first_dfs(adev, vm, start);
    let mut entry = c.entry;
    amdgpu_vm_pt_next_dfs(adev, &mut c);
    while amdgpu_vm_pt_continue_dfs(start, entry) {
        out.push(entry.unwrap());
        entry = c.entry;
        amdgpu_vm_pt_next_dfs(adev, &mut c);
    }
    out
}

/// The directory level of a page table buffer, counted from the root.
fn pt_level(adev: &amdgpu_device, h: usize) -> u32 {
    let mut level = adev.vm_manager.root_level;
    if let Some(p) = obj::bo(adev, h).parent {
        level += 1;
        let mut parent = p;
        while let Some(grandparent) = obj::bo(adev, parent).parent {
            level += 1;
            parent = grandparent;
        }
    }
    level
}

/// `amdgpu_vm_pt_clear`: every entry of a new table invalid.
pub fn amdgpu_vm_pt_clear(adev: &mut amdgpu_device, vm: usize, vmbo: usize, immediate: bool) -> i32 {
    let level = pt_level(adev, vmbo);
    let entries = (obj::amdgpu_bo_size(adev, vmbo) / 8) as u32;
    let placement = obj::bo(adev, vmbo).placements.clone();
    let r = crate::amdgpu_ttm::ttm_bo_validate(adev, vmbo, &placement);
    if r != 0 {
        return r;
    }
    let r = crate::amdgpu_vm_sdma::amdgpu_vm_sdma_map_table(adev, vmbo);
    if r != 0 {
        return r;
    }
    let mut params = amdgpu_vm::amdgpu_vm_update_params::new(vm, immediate);
    let r = crate::amdgpu_vm_sdma::amdgpu_vm_sdma_prepare(adev, &mut params, None);
    if r != 0 {
        return r;
    }
    let addr = 0u64;
    let mut value = 0u64;
    let mut flags = 0u64;
    if adev.asic_type >= CHIP_VEGA10 {
        if level != AMDGPU_VM_PTB {
            // Handle leaf PDEs as PTEs
            flags |= AMDGPU_PDE_PTE;
            crate::gmc_v9_0::gmc_v9_0_get_vm_pde(adev, level as i32, &mut value, &mut flags);
        } else {
            // Workaround for fault priority problem on GMC9
            flags = AMDGPU_PTE_EXECUTABLE as u64;
        }
    }
    let r = crate::amdgpu_vm_sdma::amdgpu_vm_sdma_update(adev, &mut params, vmbo, addr, 0, entries, value as u32,
                                                         flags);
    if r != 0 {
        return r;
    }
    match crate::amdgpu_vm_sdma::amdgpu_vm_sdma_commit(adev, &mut params, false) {
        Ok(_) => 0,
        Err(r) => r,
    }
}

/// `amdgpu_vm_pt_create`: a table for `level`, in VRAM, sharing the root's
/// reservation object, with a base per entry if it is a directory.
pub fn amdgpu_vm_pt_create(adev: &mut amdgpu_device, vm: usize, level: u32, _immediate: bool) -> Result<usize, i32> {
    let mut flags = (AMDGPU_GEM_CREATE_VRAM_CONTIGUOUS | AMDGPU_GEM_CREATE_CPU_GTT_USWC) as u64;
    if adev.vm(vm).use_cpu_for_update {
        flags |= AMDGPU_GEM_CREATE_CPU_ACCESS_REQUIRED as u64;
    }
    let domain = if !adev.gmc.is_app_apu { AMDGPU_GEM_DOMAIN_VRAM } else { AMDGPU_GEM_DOMAIN_GTT };
    let root = base(adev, adev.vm(vm).root).bo;
    let bp = obj::amdgpu_bo_param {
        size: amdgpu_vm_pt_size(adev, level),
        byte_align: AMDGPU_GPU_PAGE_SIZE as u64,
        domain,
        flags,
        type_: ttm_bo_type_kernel,
        resv: root,
        ..Default::default()
    };
    let h = obj::amdgpu_bo_create(adev, &bp)?;
    let num_entries = if level < AMDGPU_VM_PTB { amdgpu_vm_pt_num_entries(adev, level) as usize } else { 0 };
    let entries: Vec<usize> = (0..num_entries).map(|i| amdgpu_vm::amdgpu_vm_bo_base_alloc(adev, vm, i)).collect();
    obj::bo_mut(adev, h).entries = entries;
    Ok(h)
}

/// `amdgpu_vm_pt_alloc`: the table under the cursor, made if missing.
fn amdgpu_vm_pt_alloc(adev: &mut amdgpu_device, vm: usize, c: &amdgpu_vm_pt_cursor, immediate: bool) -> i32 {
    let entry = c.entry.unwrap();
    if base(adev, entry).bo.is_some() {
        return 0;
    }
    let pt_bo = match amdgpu_vm_pt_create(adev, vm, c.level, immediate) {
        Ok(h) => h,
        Err(r) => return r,
    };
    // Keep a reference to the root directory to avoid freeing them up in
    // the wrong order.
    let parent = base(adev, c.parent.unwrap()).bo.unwrap();
    obj::bo_mut(adev, pt_bo).parent = Some(obj::amdgpu_bo_ref(adev, parent));
    amdgpu_vm::amdgpu_vm_bo_base_init(adev, entry, vm, Some(pt_bo));
    let r = amdgpu_vm_pt_clear(adev, vm, pt_bo, immediate);
    if r != 0 {
        // error_free_pt
        amdgpu_vm_pt_free(adev, entry);
        return r;
    }
    0
}

/// `amdgpu_vm_pt_free`.
fn amdgpu_vm_pt_free(adev: &mut amdgpu_device, entry: usize) {
    let Some(h) = base(adev, entry).bo else { return };
    obj::bo_mut(adev, h).vm_bo = None;
    amdgpu_vm::status_del(adev, entry);
    base_mut(adev, entry).bo = None;
    obj::amdgpu_bo_unref(adev, h);
}

/// `amdgpu_vm_pt_free_list`.
pub fn amdgpu_vm_pt_free_list(adev: &mut amdgpu_device, params: &mut amdgpu_vm::amdgpu_vm_update_params) {
    if params.tlb_flush_waitlist.is_empty() {
        return;
    }
    // unlocked updates are only allowed on the leaves
    if params.unlocked {
        crate::linux::Line::new().s("WARNING: page tables freed by an unlocked update").say();
    }
    for e in core::mem::take(&mut params.tlb_flush_waitlist) {
        amdgpu_vm_pt_free(adev, e);
    }
}

/// `amdgpu_vm_pt_add_list`: the tables under the cursor wait for the TLB
/// flush before they go.
fn amdgpu_vm_pt_add_list(adev: &mut amdgpu_device, params: &mut amdgpu_vm::amdgpu_vm_update_params,
                         c: &amdgpu_vm_pt_cursor) {
    let vm = params.vm;
    for e in for_each_dfs_safe(adev, vm, Some(c)) {
        if base(adev, e).bo.is_some() {
            amdgpu_vm::status_del(adev, e);
            params.tlb_flush_waitlist.insert(0, e);
        }
    }
    let e = c.entry.unwrap();
    amdgpu_vm::status_del(adev, e);
    params.tlb_flush_waitlist.insert(0, e);
}

/// `amdgpu_vm_pt_free_root`.
pub fn amdgpu_vm_pt_free_root(adev: &mut amdgpu_device, vm: usize) {
    for e in for_each_dfs_safe(adev, vm, None) {
        amdgpu_vm_pt_free(adev, e);
    }
}

/// `amdgpu_vm_pde_update`: the directory entry pointing at `entry`'s table.
pub fn amdgpu_vm_pde_update(adev: &mut amdgpu_device, params: &mut amdgpu_vm::amdgpu_vm_update_params, entry: usize)
                            -> i32 {
    let Some(parent) = amdgpu_vm_pt_parent(adev, entry) else {
        crate::linux::Line::new().s("WARNING: amdgpu_vm_pde_update without a parent").say();
        return -22;
    };
    let bo = base(adev, parent).bo.unwrap();
    let mut level = 0;
    let mut pbo = obj::bo(adev, bo).parent;
    while let Some(p) = pbo {
        level += 1;
        pbo = obj::bo(adev, p).parent;
    }
    level += adev.vm_manager.root_level;
    let (pt, flags) = crate::amdgpu_gmc::amdgpu_gmc_get_pde_for_bo(adev, base(adev, entry).bo.unwrap(), level as i32);
    let pde = base(adev, entry).pt_index as u64 * 8;
    crate::amdgpu_vm_sdma::amdgpu_vm_sdma_update(adev, params, bo, pde, pt, 1, 0, flags)
}

/// `amdgpu_vm_pte_update_noretry_flags`.
fn amdgpu_vm_pte_update_noretry_flags(adev: &amdgpu_device, flags: &mut u64) {
    if *flags & AMDGPU_VM_NORETRY_FLAGS == AMDGPU_VM_NORETRY_FLAGS {
        *flags &= !AMDGPU_VM_NORETRY_FLAGS;
        *flags |= adev.gmc.noretry_flags;
    }
}

/// `amdgpu_vm_pte_update_flags`: no NUMA override on a single-node APU.
fn amdgpu_vm_pte_update_flags(adev: &mut amdgpu_device, params: &mut amdgpu_vm::amdgpu_vm_update_params, pt: usize,
                              level: u32, pe: u64, mut addr: u64, count: u32, incr: u32, mut flags: u64) {
    if level != AMDGPU_VM_PTB {
        flags |= AMDGPU_PDE_PTE;
        crate::gmc_v9_0::gmc_v9_0_get_vm_pde(adev, level as i32, &mut addr, &mut flags);
    } else if adev.asic_type >= CHIP_VEGA10 && flags & AMDGPU_PTE_VALID as u64 == 0
        && flags & AMDGPU_PTE_PRT == 0 {
        flags |= AMDGPU_PTE_EXECUTABLE as u64;
    }
    // Update the noretry flags of leaf PTEs only
    if level == AMDGPU_VM_PTB {
        amdgpu_vm_pte_update_noretry_flags(adev, &mut flags);
    }
    crate::amdgpu_vm_sdma::amdgpu_vm_sdma_update(adev, params, pt, pe, addr, count, incr, flags);
}

/// `amdgpu_vm_pte_fragment`: the biggest fragment at `start`, and where it
/// ends.
fn amdgpu_vm_pte_fragment(adev: &amdgpu_device, params: &amdgpu_vm::amdgpu_vm_update_params, start: u64, end: u64)
                          -> (u32, u64) {
    // The MC L1 TLB supports variable sized pages, based on a fragment
    // field in the PTE.
    let max_frag = if adev.asic_type < CHIP_VEGA10 { adev.vm_manager.fragment_size } else { 31 };
    // system pages are non continuously
    if params.pages_addr.is_some() {
        return (0, end);
    }
    // This intentionally wraps around if no bit is set; ffs() takes an int
    let ffs = if start as u32 == 0 { 0 } else { (start as u32).trailing_zeros() + 1 };
    let fls = 64 - (end - start).leading_zeros();
    let frag = ffs.wrapping_sub(1).min(fls.wrapping_sub(1));
    if frag >= max_frag {
        (max_frag, end & !((1u64 << max_frag) - 1))
    } else {
        (frag, start + (1u64 << frag))
    }
}

/// `amdgpu_vm_ptes_update`: the entries for `[start, end)` pointing at
/// `dst`, making the tables on the way.
pub fn amdgpu_vm_ptes_update(adev: &mut amdgpu_device, params: &mut amdgpu_vm::amdgpu_vm_update_params, start: u64,
                             end: u64, mut dst: u64, flags: u64) -> i32 {
    let mut frag_start = start;
    let (mut frag, mut frag_end) = amdgpu_vm_pte_fragment(adev, params, frag_start, end);
    // walk over the address space and update the PTs
    let mut cursor = amdgpu_vm_pt_start(adev, params.vm, start);
    while cursor.pfn < end {
        if !params.unlocked {
            // make sure that the page tables covering the address range
            // are actually allocated
            let r = amdgpu_vm_pt_alloc(adev, params.vm, &cursor, params.immediate);
            if r != 0 {
                return r;
            }
        }
        let mut shift = amdgpu_vm_pt_level_shift(adev, cursor.level);
        let parent_shift = amdgpu_vm_pt_level_shift(adev, cursor.level.wrapping_sub(1));
        if params.unlocked {
            // Unlocked updates are only allowed on the leaves
            if amdgpu_vm_pt_descendant(adev, &mut cursor) {
                continue;
            }
        } else if frag < shift {
            // We can't use this level when the fragment size is smaller
            // than the address shift. Go to the next child entry and try
            // again.
            if amdgpu_vm_pt_descendant(adev, &mut cursor) {
                continue;
            }
        } else if frag >= parent_shift {
            // If the fragment size is even larger than the parent shift we
            // should go up one level and check it again.
            if !amdgpu_vm_pt_ancestor(adev, &mut cursor) {
                return -22;
            }
            continue;
        }
        let mut pt = base(adev, cursor.entry.unwrap()).bo;
        if pt.is_none() {
            // We need all PDs and PTs for mapping something, ...
            if flags & AMDGPU_PTE_VALID as u64 != 0 {
                return -2;
            }
            // ...but unmapping something can happen at a higher level.
            if !amdgpu_vm_pt_ancestor(adev, &mut cursor) {
                return -22;
            }
            pt = base(adev, cursor.entry.unwrap()).bo;
            shift = parent_shift;
            frag_end = frag_end.max(crate::linux::align(frag_start + 1, 1u64 << shift));
        }
        let pt = pt.unwrap();
        // Looks good so far, calculate parameters for the update
        let incr = (AMDGPU_GPU_PAGE_SIZE as u64) << shift;
        let mask = amdgpu_vm_pt_entries_mask(adev, cursor.level);
        let mut pe_start = ((cursor.pfn >> shift) & mask as u64) * 8;
        let mut entry_end = if cursor.level < AMDGPU_VM_PTB && params.unlocked {
            // MASK the shift to the pointed entry
            1u64 << shift
        } else {
            (mask as u64 + 1) << shift
        };
        entry_end += cursor.pfn & !(entry_end - 1);
        entry_end = entry_end.min(end);
        loop {
            let upd_end = entry_end.min(frag_end);
            let nptes = (((upd_end - frag_start) >> shift) as u32).max(1);
            let upd_flags = flags | (frag as u64 & 0x1f) * AMDGPU_PTE_FRAG_1 as u64;
            amdgpu_vm_pte_update_flags(adev, params, pt, cursor.level, pe_start, dst, nptes, incr as u32, upd_flags);
            pe_start += nptes as u64 * 8;
            dst += nptes as u64 * incr;
            frag_start = upd_end;
            if frag_start >= frag_end {
                // figure out the next fragment
                (frag, frag_end) = amdgpu_vm_pte_fragment(adev, params, frag_start, end);
                if frag < shift {
                    break;
                }
            }
            if frag_start >= entry_end {
                break;
            }
        }
        if amdgpu_vm_pt_descendant(adev, &mut cursor) {
            // Free all child entries. Update the tables with the flags and
            // addresses and free up subsequent tables in the case of huge
            // pages or freed up areas. This is the maximum you can free,
            // because all other page tables are not completely covered by
            // the range and so potentially still in use.
            while cursor.pfn < frag_start {
                // Make sure previous mapping is freed
                if base(adev, cursor.entry.unwrap()).bo.is_some() {
                    params.needs_flush = true;
                    amdgpu_vm_pt_add_list(adev, params, &cursor);
                }
                amdgpu_vm_pt_next(adev, &mut cursor);
            }
        } else if frag >= shift {
            // or just move on to the next on the same level.
            amdgpu_vm_pt_next(adev, &mut cursor);
        }
    }
    0
}

/// `amdgpu_vm_pt_map_tables`.
pub fn amdgpu_vm_pt_map_tables(adev: &mut amdgpu_device, vm: usize) -> i32 {
    for e in for_each_dfs_safe(adev, vm, None) {
        if let Some(h) = base(adev, e).bo {
            let r = crate::amdgpu_vm_sdma::amdgpu_vm_sdma_map_table(adev, h);
            if r != 0 {
                return r;
            }
        }
    }
    0
}
