//! amdgpu/amdgpu_ttm.c with amdgpu_gtt_mgr.c, amdgpu_preempt_mgr.c and
//! the TTM core paths a kernel buffer takes (ttm_bo.c, ttm_resource.c,
//! ttm_range_manager.c, ttm_tt.c), Linux 6.18.55. Nothing is evicted: a
//! kernel buffer is pinned, so the second pass of `ttm_bo_validate`
//! (`force_space`, which would evict) finds nothing to evict either.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_object::{bo, bo_mut};
use crate::drm_mm::drm_mm;
use crate::linux::Line;
use crate::regs::*;
use crate::ttm::*;

const PAGE_SHIFT: u32 = 12;
const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;
const ENOSPC: i32 = 28;

/// `struct amdgpu_gtt_mgr`.
pub struct amdgpu_gtt_mgr {
    pub manager: ttm_resource_manager,
    pub mm: drm_mm,
}

/// `amdgpu_gtt_mgr_init`.
pub fn amdgpu_gtt_mgr_init(adev: &mut amdgpu_device, gtt_size: u64) -> i32 {
    let mut manager = ttm_resource_manager { use_tt: true, ..Default::default() };
    manager.init(gtt_size);
    let start = (AMDGPU_GTT_MAX_TRANSFER_SIZE * AMDGPU_GTT_NUM_TRANSFER_WINDOWS) as u64;
    let size = (adev.gmc.gart_size >> PAGE_SHIFT) - start;
    manager.use_type = true;
    adev.mman.gtt_mgr = Some(amdgpu_gtt_mgr { manager, mm: drm_mm::init(start, size) });
    0
}

/// `ttm_range_man_init_nocheck`.
fn ttm_range_man_init(adev: &mut amdgpu_device, type_: u32, use_tt: bool, p_size: u64) -> i32 {
    let mut manager = ttm_resource_manager { use_tt, ..Default::default() };
    manager.init(p_size);
    manager.use_type = true;
    adev.mman.range_mgrs.push((type_, manager, drm_mm::init(0, p_size)));
    0
}

/// `amdgpu_ttm_init_on_chip`.
fn amdgpu_ttm_init_on_chip(adev: &mut amdgpu_device, type_: u32, size_in_page: u64) -> i32 {
    if size_in_page == 0 {
        return 0;
    }
    ttm_range_man_init(adev, type_, false, size_in_page)
}

/// `amdgpu_preempt_mgr_init`.
fn amdgpu_preempt_mgr_init(adev: &mut amdgpu_device) -> i32 {
    let mut man = ttm_resource_manager { use_tt: true, ..Default::default() };
    man.init(1 << 30);
    man.use_type = true;
    adev.mman.preempt_mgr = Some(man);
    0
}

/// `ttm_manager_type` + `ttm_resource_manager_used`.
fn man_used(adev: &amdgpu_device, mem_type: u32) -> Option<ttm_resource_manager> {
    let m = match mem_type {
        t if t == TTM_PL_SYSTEM => Some(adev.mman.sys_mgr),
        t if t == TTM_PL_VRAM => adev.mman.vram_mgr.as_ref().map(|m| m.manager),
        t if t == TTM_PL_TT => adev.mman.gtt_mgr.as_ref().map(|m| m.manager),
        t if t == AMDGPU_PL_PREEMPT => adev.mman.preempt_mgr,
        t => adev.mman.range_mgrs.iter().find(|r| r.0 == t).map(|r| r.1),
    };
    m.filter(|m| m.use_type)
}

/// `ttm_resource_alloc`: the manager's `alloc`.
fn ttm_resource_alloc(adev: &mut amdgpu_device, h: usize, place: &ttm_place) -> Result<ttm_resource, i32> {
    let (size, page_alignment, bo_type, bo_flags) = {
        let b = bo(adev, h);
        (b.tbo.size, b.tbo.page_alignment, b.tbo.type_, b.flags)
    };
    let num_pages = size.div_ceil(PAGE_SIZE);
    match place.mem_type {
        t if t == TTM_PL_SYSTEM => {
            // ttm_sys_man_alloc
            let res = ttm_resource_init(size, place);
            adev.mman.sys_mgr.usage += res.size;
            Ok(res)
        }
        t if t == TTM_PL_VRAM => {
            let r = crate::amdgpu_vram_mgr::vram_request {
                size,
                page_alignment,
                bo_type,
                bo_flags,
                mc_vram_size: adev.gmc.mc_vram_size,
                visible_vram_size: adev.gmc.visible_vram_size,
            };
            adev.mman.vram_mgr.as_mut().unwrap().new(&r, place)
        }
        t if t == TTM_PL_TT => {
            // amdgpu_gtt_mgr_new
            let mgr = adev.mman.gtt_mgr.as_mut().unwrap();
            let mut res = ttm_resource_init(size, place);
            mgr.manager.usage += res.size;
            if place.flags & TTM_PL_FLAG_TEMPORARY == 0 && mgr.manager.usage > mgr.manager.size {
                mgr.manager.usage -= res.size;
                return Err(-ENOSPC);
            }
            if place.lpfn != 0 {
                match mgr.mm.insert_node_in_range_best(num_pages, page_alignment as u64, 0, place.fpfn as u64,
                                                       place.lpfn as u64) {
                    Ok(node) => {
                        res.start = node.start;
                        res.priv_ = ttm_resource_priv::range { node };
                    }
                    Err(e) => {
                        mgr.manager.usage -= res.size;
                        return Err(e);
                    }
                }
            } else {
                res.start = AMDGPU_BO_INVALID_OFFSET;
            }
            Ok(res)
        }
        t if t == AMDGPU_PL_PREEMPT => {
            // amdgpu_preempt_mgr_new
            let man = adev.mman.preempt_mgr.as_mut().unwrap();
            let mut res = ttm_resource_init(size, place);
            man.usage += res.size;
            res.start = AMDGPU_BO_INVALID_OFFSET;
            Ok(res)
        }
        t => {
            // ttm_range_man_alloc: TOPDOWN would be DRM_MM_INSERT_HIGH, which
            // no on-chip placement asks for.
            let (_, man, mm) = adev.mman.range_mgrs.iter_mut().find(|r| r.0 == t).ok_or(-22)?;
            let lpfn = if place.lpfn == 0 { man.size } else { place.lpfn as u64 };
            let mut res = ttm_resource_init(size, place);
            man.usage += res.size;
            match mm.insert_node_in_range_best(num_pages, page_alignment as u64, 0, place.fpfn as u64, lpfn) {
                Ok(node) => {
                    res.start = node.start;
                    res.priv_ = ttm_resource_priv::range { node };
                    Ok(res)
                }
                Err(e) => {
                    man.usage -= res.size;
                    Err(e)
                }
            }
        }
    }
}

/// `ttm_resource_free` of a resource that is not the buffer's.
fn ttm_resource_free_res(adev: &mut amdgpu_device, mut res: ttm_resource) {
    match res.mem_type {
        t if t == TTM_PL_SYSTEM => adev.mman.sys_mgr.usage -= res.size,
        t if t == TTM_PL_VRAM => {
            let vis = adev.gmc.visible_vram_size;
            adev.mman.vram_mgr.as_mut().unwrap().del(&mut res, vis);
        }
        t if t == TTM_PL_TT => {
            // amdgpu_gtt_mgr_del
            let mgr = adev.mman.gtt_mgr.as_mut().unwrap();
            if let ttm_resource_priv::range { node } = &res.priv_ {
                mgr.mm.remove_node(node);
            }
            mgr.manager.usage -= res.size;
        }
        t if t == AMDGPU_PL_PREEMPT => adev.mman.preempt_mgr.as_mut().unwrap().usage -= res.size,
        t => {
            if let Some((_, man, mm)) = adev.mman.range_mgrs.iter_mut().find(|r| r.0 == t) {
                if let ttm_resource_priv::range { node } = &res.priv_ {
                    mm.remove_node(node);
                }
                man.usage -= res.size;
            }
        }
    }
}

/// `ttm_resource_free(bo, &bo->resource)`.
pub fn ttm_resource_free(adev: &mut amdgpu_device, h: usize) {
    if let Some(res) = bo_mut(adev, h).tbo.resource.take() {
        ttm_resource_free_res(adev, res);
    }
}

/// `amdgpu_vram_mgr_compatible`.
fn amdgpu_vram_mgr_compatible(adev: &amdgpu_device, res: &ttm_resource, place: &ttm_place) -> bool {
    let ttm_resource_priv::vram { blocks, .. } = &res.priv_ else { return true };
    let mm = &adev.mman.vram_mgr.as_ref().unwrap().mm;
    blocks.iter().all(|&b| {
        let fpfn = mm.block_offset(b) >> PAGE_SHIFT;
        let lpfn = fpfn + (crate::amdgpu_vram_mgr::amdgpu_vram_mgr_block_size(mm, b) >> PAGE_SHIFT);
        !(fpfn < place.fpfn as u64 || (place.lpfn != 0 && lpfn > place.lpfn as u64))
    })
}

/// `ttm_resource_compatible`. Only the VRAM manager has `compatible`.
fn ttm_resource_compatible(adev: &amdgpu_device, res: &ttm_resource, placement: &ttm_placement, evicting: bool)
                           -> bool {
    if res.placement & TTM_PL_FLAG_TEMPORARY != 0 {
        return false;
    }
    placement.iter().any(|place| {
        res.mem_type == place.mem_type
            && place.flags & (if evicting { TTM_PL_FLAG_DESIRED } else { TTM_PL_FLAG_FALLBACK }) == 0
            && !(place.flags & TTM_PL_FLAG_CONTIGUOUS != 0 && res.placement & TTM_PL_FLAG_CONTIGUOUS == 0)
            && (res.mem_type != TTM_PL_VRAM || amdgpu_vram_mgr_compatible(adev, res, place))
    })
}

/// `ttm_bo_alloc_resource`. Kernel buffers are pinned and nothing else
/// is placed, so `ttm_bo_evict_alloc` finds nothing to evict: -EBUSY.
fn ttm_bo_alloc_resource(adev: &mut amdgpu_device, h: usize, placement: &ttm_placement, force_space: bool)
                         -> Result<ttm_resource, i32> {
    for place in placement {
        if man_used(adev, place.mem_type).is_none() {
            continue;
        }
        if place.flags & (if force_space { TTM_PL_FLAG_DESIRED } else { TTM_PL_FLAG_FALLBACK }) != 0 {
            continue;
        }
        match ttm_resource_alloc(adev, h, place) {
            Ok(res) => return Ok(res),
            Err(e) if e != -ENOSPC => return Err(e),
            Err(_) => continue,
        }
    }
    Err(-ENOSPC)
}

/// `ttm_bo_mem_space`.
pub fn ttm_bo_mem_space(adev: &mut amdgpu_device, h: usize, placement: &ttm_placement) -> Result<ttm_resource, i32> {
    let mut force_space = false;
    loop {
        let r = ttm_bo_alloc_resource(adev, h, placement, force_space);
        force_space = !force_space;
        match r {
            Err(e) if e == -ENOSPC && force_space => continue,
            r => return r,
        }
    }
}

/// `amdgpu_ttm_tt_create` via `ttm_tt_create`.
fn ttm_tt_create(adev: &mut amdgpu_device, h: usize) -> i32 {
    let b = bo(adev, h);
    if b.tbo.ttm.is_some() {
        return 0;
    }
    let page_flags = match b.tbo.type_ {
        t if t == ttm_bo_type_device => 0,
        t if t == ttm_bo_type_kernel => 0,
        t if t == ttm_bo_type_sg => TTM_TT_FLAG_EXTERNAL,
        _ => {
            Line::new().s("Illegal buffer object type").say();
            return -22;
        }
    };
    let caching = if b.flags & AMDGPU_GEM_CREATE_CPU_GTT_USWC as u64 != 0 {
        ttm_caching::ttm_write_combined
    } else {
        ttm_caching::ttm_cached
    };
    let num_pages = crate::linux::align(b.tbo.size, PAGE_SIZE) >> PAGE_SHIFT;
    bo_mut(adev, h).tbo.ttm = Some(ttm_tt {
        num_pages,
        page_flags,
        caching,
        populated: false,
        dma: None,
        offset: AMDGPU_BO_INVALID_OFFSET,
        bound: false,
    });
    0
}

/// `ttm_bo_populate` with `amdgpu_ttm_tt_populate`: the pages from the
/// kernel (`ttm_pool_alloc`), one DMA buffer for this driver's own buffers,
/// single frames counted to the guest for a guest client's.
fn ttm_bo_populate(adev: &mut amdgpu_device, h: usize) -> i32 {
    let Some(tt) = bo(adev, h).tbo.ttm else { return 0 };
    if tt.populated || tt.page_flags & TTM_TT_FLAG_EXTERNAL != 0 {
        return 0;
    }
    if let Some(client) = bo(adev, h).client {
        let id = npk_sys::gpudev_tt_alloc(client as i32, tt.num_pages as i32);
        if id <= 0 {
            return -12;
        }
        let buf = npk_sys::gpudev_tt_buffer(id);
        let mut pages = alloc::vec::Vec::new();
        let mut list = [0u64; 64];
        while buf > 0 && (pages.len() as u64) < tt.num_pages {
            let want = ((tt.num_pages - pages.len() as u64) as usize).min(list.len());
            let n = npk_sys::gpudev_bo_pages(buf, pages.len() as i32, &mut list[..want]);
            if n <= 0 || n as usize > want {
                break;
            }
            pages.extend_from_slice(&list[..n as usize]);
        }
        if pages.len() as u64 != tt.num_pages {
            npk_sys::gpudev_tt_free(id);
            return -12;
        }
        let b = bo_mut(adev, h);
        b.pool = Some((id as u32, pages));
        b.tbo.ttm.as_mut().unwrap().populated = true;
        return 0;
    }
    let Some(d) = npk_sys::Dma::alloc(tt.num_pages as i32) else { return -12 };
    let t = bo_mut(adev, h).tbo.ttm.as_mut().unwrap();
    t.dma = Some((d.handle, d.phys));
    t.populated = true;
    0
}

/// `amdgpu_ttm_backend_bind`: a TT without a GART address has nothing
/// to bind yet (`amdgpu_ttm_alloc_gart` does it).
fn amdgpu_ttm_backend_bind(adev: &mut amdgpu_device, h: usize, mem: &ttm_resource) -> i32 {
    let Some(tt) = bo_mut(adev, h).tbo.ttm.as_mut() else { return -22 };
    if tt.bound {
        return 0;
    }
    if !matches!(mem.priv_, ttm_resource_priv::range { .. }) {
        tt.offset = AMDGPU_BO_INVALID_OFFSET;
        return 0;
    }
    tt.offset = mem.start << PAGE_SHIFT;
    let flags = amdgpu_ttm_tt_pte_flags(adev, Some(&tt_of(adev, h)), Some(mem));
    amdgpu_ttm_gart_bind(adev, h, flags);
    0
}

fn tt_of(adev: &amdgpu_device, h: usize) -> ttm_tt {
    bo(adev, h).tbo.ttm.unwrap()
}

/// `ttm_bo_handle_move_mem` with `amdgpu_bo_move` for a buffer that has
/// no resource yet, or one in SYSTEM going to TT (an imported buffer being
/// pinned): TT pages created and populated, then `move_null`.
fn ttm_bo_handle_move_mem(adev: &mut amdgpu_device, h: usize, mem: ttm_resource) -> Result<(), (i32, ttm_resource)> {
    let new_use_tt = man_used(adev, mem.mem_type).is_some_and(|m| m.use_tt);
    if new_use_tt {
        let r = ttm_tt_create(adev, h);
        if r != 0 {
            return Err((r, mem));
        }
        if mem.mem_type != TTM_PL_SYSTEM {
            let r = ttm_bo_populate(adev, h);
            if r != 0 {
                return Err((r, mem));
            }
        }
    }
    // amdgpu_bo_move
    if mem.mem_type == TTM_PL_TT || mem.mem_type == AMDGPU_PL_PREEMPT {
        let r = amdgpu_ttm_backend_bind(adev, h, &mem);
        if r != 0 {
            return Err((r, mem));
        }
    }
    let old_system = bo(adev, h).tbo.resource.as_ref().map(|r| r.mem_type == TTM_PL_SYSTEM);
    let new_tt = mem.mem_type == TTM_PL_TT || mem.mem_type == AMDGPU_PL_PREEMPT;
    if old_system.is_none() || old_system == Some(true) && new_tt {
        // ttm_bo_move_null
        ttm_resource_free(adev, h);
        bo_mut(adev, h).tbo.resource = Some(mem);
        return Ok(());
    }
    // A pinned kernel buffer is never moved between placements.
    Err((-22, mem))
}

/// `ttm_bo_validate`.
pub fn ttm_bo_validate(adev: &mut amdgpu_device, h: usize, placement: &ttm_placement) -> i32 {
    if placement.is_empty() {
        return -22;
    }
    let mut force_space = false;
    let mut ret;
    loop {
        // Check whether we need to move buffer.
        if let Some(res) = bo(adev, h).tbo.resource.as_ref() {
            if ttm_resource_compatible(adev, res, placement, force_space) {
                return 0;
            }
        }
        // Moving of pinned BOs is forbidden
        if bo(adev, h).tbo.pin_count != 0 {
            return -22;
        }
        let r = ttm_bo_alloc_resource(adev, h, placement, force_space);
        force_space = !force_space;
        match r {
            Err(e) if e == -ENOSPC => {
                ret = e;
            }
            Err(e) => return e,
            Ok(res) => {
                ret = 0;
                if let Err((e, res)) = ttm_bo_handle_move_mem(adev, h, res) {
                    ttm_resource_free_res(adev, res);
                    return e;
                }
            }
        }
        if !(ret != 0 && force_space) {
            break;
        }
    }
    // For backward compatibility with userspace
    if ret == -ENOSPC {
        return -12;
    }
    // We might need to add a TTM.
    if bo(adev, h).tbo.resource.as_ref().is_none_or(|r| r.mem_type == TTM_PL_SYSTEM) {
        return ttm_tt_create(adev, h);
    }
    0
}

/// `ttm_bo_init_reserved`.
pub fn ttm_bo_init_reserved(adev: &mut amdgpu_device, h: usize, type_: u32, alignment: u32) -> i32 {
    let b = bo_mut(adev, h);
    b.tbo.type_ = type_;
    b.tbo.page_alignment = alignment;
    b.tbo.pin_count = 0;
    let placement = b.placements.clone();
    ttm_bo_validate(adev, h, &placement)
}

/// `amdgpu_ttm_domain_start`.
pub fn amdgpu_ttm_domain_start(adev: &amdgpu_device, type_: u32) -> u64 {
    match type_ {
        t if t == TTM_PL_TT => adev.gmc.gart_start,
        t if t == TTM_PL_VRAM => adev.gmc.vram_start,
        _ => 0,
    }
}

/// `amdgpu_ttm_tt_pde_flags`.
pub fn amdgpu_ttm_tt_pde_flags(tt: Option<&ttm_tt>, mem: Option<&ttm_resource>) -> u64 {
    let mut flags = 0u64;
    if let Some(mem) = mem {
        if mem.mem_type != TTM_PL_SYSTEM {
            flags |= AMDGPU_PTE_VALID as u64;
        }
        if mem.mem_type == TTM_PL_TT || mem.mem_type == AMDGPU_PL_DOORBELL || mem.mem_type == AMDGPU_PL_PREEMPT
            || mem.mem_type == AMDGPU_PL_MMIO_REMAP {
            flags |= AMDGPU_PTE_SYSTEM as u64;
            if tt.is_some_and(|t| t.caching == ttm_caching::ttm_cached) {
                flags |= AMDGPU_PTE_SNOOPED as u64;
            }
        }
        if mem.mem_type == TTM_PL_VRAM && mem.caching == ttm_caching::ttm_cached {
            flags |= AMDGPU_PTE_SNOOPED as u64;
        }
    }
    flags
}

/// `amdgpu_ttm_tt_pte_flags`: kernel TTs are never read-only.
pub fn amdgpu_ttm_tt_pte_flags(adev: &amdgpu_device, tt: Option<&ttm_tt>, mem: Option<&ttm_resource>) -> u64 {
    let mut flags = amdgpu_ttm_tt_pde_flags(tt, mem);
    flags |= adev.gart.gart_pte_flags;
    flags |= AMDGPU_PTE_READABLE as u64;
    flags |= AMDGPU_PTE_WRITEABLE as u64;
    flags
}

/// `amdgpu_ttm_gart_bind`: no TMZ buffers and no gfx9 MQD among the
/// kernel's. An imported buffer's pages are its `sg` list, as
/// `amdgpu_ttm_backend_bind` copies them with
/// `drm_prime_sg_to_dma_addr_array`.
fn amdgpu_ttm_gart_bind(adev: &mut amdgpu_device, h: usize, flags: u64) {
    let tt = tt_of(adev, h);
    let addrs = crate::amdgpu_object::tt_pages(bo(adev, h));
    crate::amdgpu_gart::amdgpu_gart_bind(adev, tt.offset, tt.num_pages, &addrs, flags);
    bo_mut(adev, h).tbo.ttm.as_mut().unwrap().bound = true;
}

/// `amdgpu_ttm_alloc_gart`.
pub fn amdgpu_ttm_alloc_gart(adev: &mut amdgpu_device, h: usize) -> i32 {
    let res = bo(adev, h).tbo.resource.as_ref().unwrap();
    if res.start != AMDGPU_BO_INVALID_OFFSET {
        return 0;
    }
    let placement_flags = res.placement;
    // amdgpu_gmc_agp_addr: the AGP aperture is off (amdgpu_agp auto).
    // allocate GART space
    let place = ttm_place { fpfn: 0, lpfn: (adev.gmc.gart_size >> PAGE_SHIFT) as u32, mem_type: TTM_PL_TT,
                            flags: placement_flags };
    let tmp = match ttm_bo_mem_space(adev, h, &alloc::vec![place]) {
        Ok(t) => t,
        Err(r) => return r,
    };
    // compute PTE flags for this buffer object
    let tt = tt_of(adev, h);
    let flags = amdgpu_ttm_tt_pte_flags(adev, Some(&tt), Some(&tmp));
    // Bind pages
    bo_mut(adev, h).tbo.ttm.as_mut().unwrap().offset = tmp.start << PAGE_SHIFT;
    amdgpu_ttm_gart_bind(adev, h, flags);
    crate::amdgpu_gart::amdgpu_gart_invalidate_tlb(adev);
    ttm_resource_free(adev, h);
    bo_mut(adev, h).tbo.resource = Some(tmp);
    0
}

/// `amdgpu_gtt_mgr_recover`: bind every TT that has a GART address again.
pub fn amdgpu_gtt_mgr_recover(adev: &mut amdgpu_device) {
    let handles: alloc::vec::Vec<usize> = (0..adev.mman.bos.len()).filter(|&h| {
        adev.mman.bos[h].as_ref().is_some_and(|b| {
            b.tbo.resource.as_ref().is_some_and(|r| r.mem_type == TTM_PL_TT
                && matches!(r.priv_, ttm_resource_priv::range { .. }))
        })
    }).collect();
    for h in handles {
        // amdgpu_ttm_recover_gart
        let Some(tt) = bo(adev, h).tbo.ttm else { continue };
        let res = bo(adev, h).tbo.resource.clone();
        let flags = amdgpu_ttm_tt_pte_flags(adev, Some(&tt), res.as_ref());
        amdgpu_ttm_gart_bind(adev, h, flags);
    }
}

/// `amdgpu_ttm_set_buffer_funcs_status`: the buffer functions' entities on
/// their ring's scheduler, and the VRAM size TTM sees.
pub fn amdgpu_ttm_set_buffer_funcs_status(adev: &mut amdgpu_device, enable: bool) {
    if !adev.mman.initialized || adev.mman.buffer_funcs_enabled == enable || adev.gmc.is_app_apu {
        return;
    }
    if enable {
        let Some(ring) = adev.sdma.instance[0].ring else { return };
        let sched = [ring];
        match crate::drm_sched::drm_sched_entity_init(adev, crate::drm_sched::DRM_SCHED_PRIORITY_KERNEL, &sched, None) {
            Ok(e) => adev.mman.high_pr = Some(e),
            Err(r) => {
                Line::new().s("Failed setting up TTM BO move entity (").dec((-r) as u64).s(")").say();
                return;
            }
        }
        match crate::drm_sched::drm_sched_entity_init(adev, crate::drm_sched::DRM_SCHED_PRIORITY_NORMAL, &sched, None) {
            Ok(e) => adev.mman.low_pr = Some(e),
            Err(r) => {
                Line::new().s("Failed setting up TTM BO move entity (").dec((-r) as u64).s(")").say();
                if let Some(e) = adev.mman.high_pr.take() {
                    crate::drm_sched::drm_sched_entity_destroy(adev, e);
                }
                return;
            }
        }
    } else {
        if let Some(e) = adev.mman.high_pr.take() {
            crate::drm_sched::drm_sched_entity_destroy(adev, e);
        }
        if let Some(e) = adev.mman.low_pr.take() {
            crate::drm_sched::drm_sched_entity_destroy(adev, e);
        }
        // dma_fence_put(man->move): no move fence is kept here
    }
    let size = if enable { adev.gmc.real_vram_size } else { adev.gmc.visible_vram_size };
    if let Some(m) = adev.mman.vram_mgr.as_mut() {
        m.manager.size = size;
    }
    adev.mman.buffer_funcs_enabled = enable;
}

/// `amdgpu_ttm_fw_reserve_vram_init`.
fn amdgpu_ttm_fw_reserve_vram_init(adev: &mut amdgpu_device) -> i32 {
    let vram_size = adev.gmc.visible_vram_size;
    adev.mman.fw_vram_usage_reserved_bo = None;
    if adev.mman.fw_vram_usage_size == 0 || adev.mman.fw_vram_usage_size > vram_size {
        return 0;
    }
    let (start, size) = (adev.mman.fw_vram_usage_start_offset, adev.mman.fw_vram_usage_size);
    match crate::amdgpu_object::amdgpu_bo_create_kernel_at(adev, start, size, true) {
        Ok(h) => {
            adev.mman.fw_vram_usage_reserved_bo = h;
            0
        }
        Err(r) => r,
    }
}

/// `amdgpu_ttm_drv_reserve_vram_init`.
fn amdgpu_ttm_drv_reserve_vram_init(adev: &mut amdgpu_device) -> i32 {
    let vram_size = adev.gmc.visible_vram_size;
    adev.mman.drv_vram_usage_reserved_bo = None;
    if adev.mman.drv_vram_usage_size == 0 || adev.mman.drv_vram_usage_size > vram_size {
        return 0;
    }
    let (start, size) = (adev.mman.drv_vram_usage_start_offset, adev.mman.drv_vram_usage_size);
    match crate::amdgpu_object::amdgpu_bo_create_kernel_at(adev, start, size, true) {
        Ok(h) => {
            adev.mman.drv_vram_usage_reserved_bo = h;
            0
        }
        Err(r) => r,
    }
}

/// `amdgpu_ttm_reserve_tmr`: the top of VRAM for the discovery TMR.
/// Memory training (G6) is a dGPU's.
fn amdgpu_ttm_reserve_tmr(adev: &mut amdgpu_device) -> i32 {
    let mem_train_support = !adev.bios.is_empty()
        && crate::amdgpu_atomfirmware::amdgpu_atomfirmware_mem_training_supported(adev);
    if !mem_train_support {
        Line::new().s("memory training does not support!").dbg();
    }
    // Query reserved tmr size through atom firmwareinfo for
    // Sienna_Cichlid and onwards; otherwise the legacy discovery TMR.
    let mut reserve_size: u32 = 0;
    if !adev.bios.is_empty() {
        reserve_size = crate::amdgpu_atomfirmware::amdgpu_atomfirmware_get_fw_reserved_fb_size(adev) as u32;
    }
    if reserve_size == 0 {
        reserve_size = DISCOVERY_TMR_OFFSET;
    }
    if mem_train_support {
        Line::new().s("memory training: not ported").say();
        return -22;
    }
    if !adev.gmc.is_app_apu {
        let at = adev.gmc.real_vram_size - reserve_size as u64;
        match crate::amdgpu_object::amdgpu_bo_create_kernel_at(adev, at, reserve_size as u64, false) {
            Ok(h) => adev.mman.fw_reserved_memory = h,
            Err(r) => {
                Line::new().s("alloc tmr failed(").dec((-r) as u64).s(")!").say();
                return r;
            }
        }
    }
    0
}

/// `amdgpu_ttm_init`: the managers, the stolen memory mapped, the fixed
/// reservations, the kernel doorbells and the SDMA access page.
pub fn amdgpu_ttm_init(adev: &mut amdgpu_device) -> i32 {
    // ttm_device_init: the system manager is always there.
    adev.mman.sys_mgr = ttm_resource_manager { use_tt: true, use_type: true, ..Default::default() };
    // amdgpu_ttm_pools_init: not an app APU.
    adev.mman.initialized = true;

    if !adev.gmc.is_app_apu {
        // Initialize VRAM pool with all of VRAM divided into pages
        match crate::amdgpu_vram_mgr::amdgpu_vram_mgr::init(adev.gmc.real_vram_size) {
            Ok(mut m) => {
                m.trace = crate::linux::debug_enabled() || cfg!(not(target_arch = "wasm32"));
                adev.mman.vram_mgr = Some(m);
            }
            Err(r) => {
                Line::new().s("Failed initializing VRAM heap.").say();
                return r;
            }
        }
    }
    // Change the size here instead of the init above so only lpfn is affected
    amdgpu_ttm_set_buffer_funcs_status(adev, false);
    // ioremap_wc of the aperture: on an APU the stolen system memory.
    let pages = (adev.gmc.visible_vram_size >> PAGE_SHIFT) as i32;
    adev.mman.aper_base_kaddr = npk_sys::Mmio::map_phys(adev.gmc.aper_base, pages);
    if adev.mman.aper_base_kaddr.is_none() {
        Line::new().s("the stolen memory at ").hex64(adev.gmc.aper_base).s(" could not be mapped (")
            .dec(pages as u64).s(" pages)").say();
    }

    // The reserved vram for firmware must be pinned to the specified place
    // on the VRAM, so reserve it early.
    let r = amdgpu_ttm_fw_reserve_vram_init(adev);
    if r != 0 {
        return r;
    }
    // The reserved vram for driver must be pinned to the specified place
    // on the VRAM, so reserve it early.
    let r = amdgpu_ttm_drv_reserve_vram_init(adev);
    if r != 0 {
        return r;
    }
    // If IP discovery enabled, a block of memory should be reserved for IP
    // discovey.
    if !adev.mman.discovery_bin.is_empty() {
        let r = amdgpu_ttm_reserve_tmr(adev);
        if r != 0 {
            return r;
        }
    }
    // allocate memory as required for VGA: VGA emulation and pre-OS
    // scanout buffers, to avoid display artifacts while transitioning
    // between pre-OS and driver.
    if !adev.gmc.is_app_apu {
        let (vga, ext) = (adev.mman.stolen_vga_size, adev.mman.stolen_extended_size);
        match crate::amdgpu_object::amdgpu_bo_create_kernel_at(adev, 0, vga, false) {
            Ok(h) => adev.mman.stolen_vga_memory = h,
            Err(r) => return r,
        }
        match crate::amdgpu_object::amdgpu_bo_create_kernel_at(adev, vga, ext, false) {
            Ok(h) => adev.mman.stolen_extended_memory = h,
            Err(r) => return r,
        }
        let (off, size) = (adev.mman.stolen_reserved_offset, adev.mman.stolen_reserved_size);
        match crate::amdgpu_object::amdgpu_bo_create_kernel_at(adev, off, size, false) {
            Ok(h) => adev.mman.stolen_reserved_memory = h,
            Err(r) => return r,
        }
    }
    Line::new().s("amdgpu: ").dec(adev.gmc.real_vram_size / (1024 * 1024)).s("M of VRAM memory ready").say();

    // Compute GTT size, either based on TTM limit or whatever the user
    // passed on module init: ttm_global_init limits it to half of RAM.
    let total_ram = (npk_sys::sys_info(2).max(0) as u64) << 20;
    let mut gtt_size = ((total_ram >> PAGE_SHIFT) / 2) << PAGE_SHIFT;
    if crate::tables::amdgpu_gtt_size[0] != -1 {
        gtt_size = (crate::tables::amdgpu_gtt_size[0] as u64) << 20;
    }
    // Cap GTT so that it does not exceed total physical RAM.
    if adev.flags & AMD_IS_APU != 0 && gtt_size > total_ram {
        gtt_size = total_ram;
        Line::new().s("Capping GTT to ").dec(gtt_size / (1024 * 1024)).s("M to not exceed available system memory")
            .say();
    }
    // Initialize GTT memory pool
    let r = amdgpu_gtt_mgr_init(adev, gtt_size);
    if r != 0 {
        Line::new().s("Failed initializing GTT heap.").say();
        return r;
    }
    Line::new().s("amdgpu: ").dec(gtt_size / (1024 * 1024)).s("M of GTT memory ready.").say();
    if adev.flags & AMD_IS_APU != 0 && adev.gmc.real_vram_size < gtt_size {
        adev.apu_prefer_gtt = true;
    }

    // Initialize doorbell pool on PCI BAR
    let r = amdgpu_ttm_init_on_chip(adev, AMDGPU_PL_DOORBELL, adev.doorbell.size / PAGE_SIZE);
    if r != 0 {
        Line::new().s("Failed initializing doorbell heap.").say();
        return r;
    }
    // Create a boorbell page for kernel usages
    let r = crate::amdgpu_device::amdgpu_doorbell_create_kernel_doorbells(adev);
    if r != 0 {
        Line::new().s("Failed to initialize kernel doorbells.").say();
        return r;
    }
    // Initialize MMIO-remap pool (single page 4K)
    let r = amdgpu_ttm_init_on_chip(adev, AMDGPU_PL_MMIO_REMAP, 1);
    if r != 0 {
        Line::new().s("Failed initializing MMIO-remap heap.").say();
        return r;
    }
    // Allocate the singleton MMIO_REMAP BO (4K) if supported
    if adev.rmmio_remap.bus_addr != 0 {
        let bp = crate::amdgpu_object::amdgpu_bo_param {
            type_: ttm_bo_type_device,
            size: AMDGPU_GPU_PAGE_SIZE as u64,
            byte_align: AMDGPU_GPU_PAGE_SIZE as u64,
            domain: AMDGPU_GEM_DOMAIN_MMIO_REMAP,
            ..Default::default()
        };
        match crate::amdgpu_object::amdgpu_bo_create(adev, &bp) {
            Ok(h) => adev.rmmio_remap.bo = Some(h),
            Err(r) => return r,
        }
    }
    // Initialize preemptible memory pool
    let r = amdgpu_preempt_mgr_init(adev);
    if r != 0 {
        Line::new().s("Failed initializing PREEMPT heap.").say();
        return r;
    }
    // Initialize various on-chip memory pools
    let (gds, gws, oa) = (adev.gds.gds_size as u64, adev.gds.gws_size as u64, adev.gds.oa_size as u64);
    for (t, n, name) in [(AMDGPU_PL_GDS, gds, "GDS"), (AMDGPU_PL_GWS, gws, "gws"), (AMDGPU_PL_OA, oa, "oa")] {
        let r = amdgpu_ttm_init_on_chip(adev, t, n);
        if r != 0 {
            Line::new().s("Failed initializing ").s(name).s(" heap.").say();
            return r;
        }
    }
    match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, PAGE_SIZE, PAGE_SIZE, AMDGPU_GEM_DOMAIN_GTT, true) {
        Ok(h) => adev.mman.sdma_access_bo = h,
        Err(_) => Line::new().s("Debug VRAM access will use slowpath MM access").say(),
    }
    0
}

/// `amdgpu_ttm_prepare_job`: the job waits for everything in `resv`.
fn amdgpu_ttm_prepare_job(adev: &mut amdgpu_device, direct_submit: bool, num_dw: u32,
                          resv: Option<&crate::dma_resv::dma_resv>, vm_needs_flush: bool, delayed: bool)
                          -> Result<u64, i32> {
    let pool = if direct_submit { AMDGPU_IB_POOL_DIRECT } else { AMDGPU_IB_POOL_DELAYED };
    let entity = if delayed { adev.mman.low_pr } else { adev.mman.high_pr };
    let j = crate::amdgpu_job::amdgpu_job_alloc_with_ib(adev, entity, crate::amdgpu_sync::AMDGPU_FENCE_OWNER_UNDEFINED,
                                                        num_dw * 4, pool)?;
    if vm_needs_flush {
        // no pdb0_bo without XGMI: the GART table
        let pd = crate::amdgpu_gmc::amdgpu_gmc_pd_addr(adev, adev.gart.bo.unwrap());
        let job = crate::amdgpu_job::job(adev, j);
        job.vm_pd_addr = pd;
        job.vm_needs_flush = true;
    }
    if let Some(resv) = resv {
        let r = crate::drm_sched::drm_sched_job_add_resv_dependencies(
            adev, j, resv, crate::dma_resv::dma_resv_usage::DMA_RESV_USAGE_BOOKKEEP);
        if r != 0 {
            crate::amdgpu_job::amdgpu_job_free(adev, j);
            return Err(r);
        }
    }
    Ok(j)
}

/// `amdgpu_ttm_fill_mem`.
fn amdgpu_ttm_fill_mem(adev: &mut amdgpu_device, r: usize, src_data: u32, mut dst_addr: u64, mut byte_count: u32,
                       resv: Option<&crate::dma_resv::dma_resv>, vm_needs_flush: bool, delayed: bool)
                       -> Result<crate::dma_fence::Fence, i32> {
    use crate::sdma_v4_0::{SDMA_V4_0_FILL_MAX_BYTES, SDMA_V4_0_FILL_NUM_DW};
    let max_bytes = SDMA_V4_0_FILL_MAX_BYTES;
    let num_loops = byte_count.div_ceil(max_bytes);
    let num_dw = crate::linux::align((num_loops * SDMA_V4_0_FILL_NUM_DW) as u64, 8) as u32;
    let j = amdgpu_ttm_prepare_job(adev, false, num_dw, resv, vm_needs_flush, delayed)?;
    let mut ib = crate::amdgpu_job::job(adev, j).ibs[0].clone();
    for _ in 0..num_loops {
        let cur_size = byte_count.min(max_bytes);
        crate::sdma_v4_0::sdma_v4_0_emit_fill_buffer(adev, &mut ib, src_data, dst_addr, cur_size);
        dst_addr += cur_size as u64;
        byte_count -= cur_size;
    }
    crate::sdma_v4_0::sdma_v4_0_ring_pad_ib(adev, r, &mut ib);
    if ib.length_dw > num_dw {
        Line::new().s("WARNING: amdgpu_ttm_fill_mem: IB overrun").say();
    }
    crate::amdgpu_job::job(adev, j).ibs[0] = ib;
    Ok(crate::amdgpu_job::amdgpu_job_submit(adev, j))
}

/// `amdgpu_ttm_map_buffer` for memory the engine reaches directly: VRAM
/// and a TT with its GART address. A TT without one goes through the
/// transfer window, which no fill here needs.
fn amdgpu_ttm_map_buffer(adev: &amdgpu_device, mem: &crate::ttm::ttm_resource,
                         cur: &crate::amdgpu_res_cursor::amdgpu_res_cursor) -> Result<u64, i32> {
    if mem.mem_type == AMDGPU_PL_PREEMPT || mem.start == AMDGPU_BO_INVALID_OFFSET {
        return Err(-22);
    }
    Ok(amdgpu_ttm_domain_start(adev, mem.mem_type) + cur.start)
}

/// `amdgpu_res_cleared`: a VRAM block the buddy allocator knows is zero.
fn amdgpu_res_cleared(adev: &amdgpu_device, mem: &crate::ttm::ttm_resource,
                      cur: &crate::amdgpu_res_cursor::amdgpu_res_cursor) -> bool {
    use crate::ttm::ttm_resource_priv;
    match (&mem.priv_, cur.node(), adev.mman.vram_mgr.as_ref()) {
        (ttm_resource_priv::vram { blocks, .. }, Some(i), Some(m)) if mem.mem_type == TTM_PL_VRAM =>
            blocks.get(i).is_some_and(|&b| m.mm.is_clear(b)),
        _ => false,
    }
}

/// `amdgpu_ttm_clear_buffer`: the blocks not known to be zero, zeroed.
pub fn amdgpu_ttm_clear_buffer(adev: &mut amdgpu_device, h: usize, resv: &crate::dma_resv::dma_resv)
                               -> Result<crate::dma_fence::Fence, i32> {
    if !adev.mman.buffer_funcs_enabled {
        return Err(-22);
    }
    // `mman.buffer_funcs_ring`: SDMA 0, as `amdgpu_ttm_set_buffer_funcs_status` set it.
    let r = adev.sdma.instance[0].ring.ok_or(-22)?;
    let mut fence = crate::dma_fence::dma_fence_get_stub();
    let mem = bo(adev, h).tbo.resource.clone().ok_or(-22)?;
    let size = crate::amdgpu_object::amdgpu_bo_size(adev, h);
    let mut cur = crate::amdgpu_res_cursor::amdgpu_res_first(adev, Some(&mem), 0, size);
    while cur.remaining != 0 {
        if amdgpu_res_cleared(adev, &mem, &cur) {
            let n = cur.size;
            crate::amdgpu_res_cursor::amdgpu_res_next(adev, Some(&mem), &mut cur, n);
            continue;
        }
        // Never clear more than 256MiB at once to avoid timeouts
        let n = cur.size.min(256 << 20);
        let addr = amdgpu_ttm_map_buffer(adev, &mem, &cur)?;
        fence = amdgpu_ttm_fill_mem(adev, r, 0, addr, n as u32, Some(resv), true, true)?;
        crate::amdgpu_res_cursor::amdgpu_res_next(adev, Some(&mem), &mut cur, n);
    }
    Ok(fence)
}

/// `amdgpu_fill_buffer`.
pub fn amdgpu_fill_buffer(adev: &mut amdgpu_device, h: usize, src_data: u32, resv: &crate::dma_resv::dma_resv,
                          delayed: bool) -> Result<crate::dma_fence::Fence, i32> {
    if !adev.mman.buffer_funcs_enabled {
        Line::new().s("Trying to clear memory with ring turned off.").say();
        return Err(-22);
    }
    // `mman.buffer_funcs_ring`: SDMA 0, as `amdgpu_ttm_set_buffer_funcs_status` set it.
    let r = adev.sdma.instance[0].ring.ok_or(-22)?;
    let mem = bo(adev, h).tbo.resource.clone().ok_or(-22)?;
    let size = crate::amdgpu_object::amdgpu_bo_size(adev, h);
    let mut dst = crate::amdgpu_res_cursor::amdgpu_res_first(adev, Some(&mem), 0, size);
    let mut fence = None;
    while dst.remaining != 0 {
        // Never fill more than 256MiB at once to avoid timeouts
        let n = dst.size.min(256 << 20);
        let to = amdgpu_ttm_map_buffer(adev, &mem, &dst)?;
        fence = Some(amdgpu_ttm_fill_mem(adev, r, src_data, to, n as u32, Some(resv), true, delayed)?);
        crate::amdgpu_res_cursor::amdgpu_res_next(adev, Some(&mem), &mut dst, n);
    }
    Ok(fence.unwrap_or_else(crate::dma_fence::dma_fence_get_stub))
}

/// `amdgpu_copy_buffer` without a reservation object, over spans: one
/// span is Linux's call, several are the rows of a rectangle in one job.
/// The copy runs on the buffer functions' entity unless `direct_submit`.
pub fn amdgpu_copy_spans(adev: &mut amdgpu_device, r: usize, spans: &[(u64, u64, u32)], direct_submit: bool,
                         vm_needs_flush: bool, copy_flags: u32) -> Result<crate::dma_fence::Fence, i32> {
    use crate::sdma_v4_0::{SDMA_V4_0_COPY_MAX_BYTES, SDMA_V4_0_COPY_NUM_DW};
    if !direct_submit && !adev.rings[r].sched.ready {
        Line::new().s("Trying to move memory with ring turned off.").say();
        return Err(-22);
    }
    let max_bytes = SDMA_V4_0_COPY_MAX_BYTES;
    let num_loops: u32 = spans.iter().map(|s| s.2.div_ceil(max_bytes)).sum();
    let num_dw = crate::linux::align((num_loops * SDMA_V4_0_COPY_NUM_DW) as u64, 8) as u32;
    let j = amdgpu_ttm_prepare_job(adev, direct_submit, num_dw, None, vm_needs_flush, false)?;
    let mut ib = crate::amdgpu_job::job(adev, j).ibs[0].clone();
    for &(mut src_offset, mut dst_offset, mut byte_count) in spans {
        while byte_count != 0 {
            let cur_size_in_bytes = byte_count.min(max_bytes);
            crate::sdma_v4_0::sdma_v4_0_emit_copy_buffer(adev, &mut ib, src_offset, dst_offset, cur_size_in_bytes,
                                                         copy_flags);
            src_offset += cur_size_in_bytes as u64;
            dst_offset += cur_size_in_bytes as u64;
            byte_count -= cur_size_in_bytes;
        }
    }
    crate::sdma_v4_0::sdma_v4_0_ring_pad_ib(adev, r, &mut ib);
    if ib.length_dw > num_dw {
        Line::new().s("WARNING: amdgpu_copy_buffer: IB overrun").say();
    }
    crate::amdgpu_job::job(adev, j).ibs[0] = ib;
    if direct_submit {
        match crate::amdgpu_job::amdgpu_job_submit_direct(adev, j, r) {
            Ok(f) => Ok(f),
            Err(e) => {
                crate::amdgpu_job::amdgpu_job_free(adev, j);
                Line::new().s("Error scheduling IBs (").dec((-e) as u64).s(")").say();
                Err(e)
            }
        }
    } else {
        Ok(crate::amdgpu_job::amdgpu_job_submit(adev, j))
    }
}
