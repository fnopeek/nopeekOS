//! amdgpu/amdgpu_object.c (Linux 6.18.55): kernel buffer objects. A
//! buffer is named by its handle into `adev.mman.bos`; reservation (the
//! dma_resv lock) is a no-op, one thread drives the device.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ttm;
use crate::linux::Line;
use crate::regs::*;
use crate::ttm::{ttm_buffer_object, ttm_place};
use alloc::vec::Vec;

const PAGE_SHIFT: u32 = 12;
pub const PAGE_SIZE: u64 = 1 << PAGE_SHIFT;

/// Where a kernel mapping of a buffer points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum amdgpu_kptr {
    /// Into the mapped stolen memory, at this byte offset.
    vram(u64),
    /// The DMA buffer of a TT.
    gtt(i32),
    /// Into BAR2, at this byte offset.
    doorbell(u64),
}

/// `struct amdgpu_bo`.
#[derive(Clone, Debug)]
pub struct amdgpu_bo {
    pub tbo: ttm_buffer_object,
    pub flags: u64,
    pub preferred_domains: u32,
    pub allowed_domains: u32,
    pub placements: Vec<ttm_place>,
    pub xcp_id: i32,
    pub kptr: Option<amdgpu_kptr>,
}

/// `struct amdgpu_bo_param`.
#[derive(Clone, Copy, Default)]
pub struct amdgpu_bo_param {
    pub size: u64,
    pub byte_align: u64,
    pub domain: u32,
    pub preferred_domain: u32,
    pub flags: u64,
    pub type_: u32,
}

pub fn bo(adev: &amdgpu_device, h: usize) -> &amdgpu_bo {
    adev.mman.bos[h].as_ref().unwrap()
}

pub fn bo_mut(adev: &mut amdgpu_device, h: usize) -> &mut amdgpu_bo {
    adev.mman.bos[h].as_mut().unwrap()
}

/// `amdgpu_bo_placement_from_domain`: no memory partitions, so VRAM is
/// the whole range.
pub fn amdgpu_bo_placement_from_domain(adev: &mut amdgpu_device, h: usize, domain: u32) {
    let visible_pfn = (adev.gmc.visible_vram_size >> PAGE_SHIFT) as u32;
    let is_apu = adev.flags & AMD_IS_APU != 0;
    let b = bo_mut(adev, h);
    let flags = b.flags;
    let mut places = Vec::new();
    if domain & AMDGPU_GEM_DOMAIN_VRAM != 0 {
        let mut p = ttm_place { fpfn: 0, lpfn: 0, mem_type: TTM_PL_VRAM, flags: 0 };
        if flags & AMDGPU_GEM_CREATE_CPU_ACCESS_REQUIRED as u64 != 0 {
            // min_not_zero
            p.lpfn = if p.lpfn == 0 { visible_pfn } else { p.lpfn.min(visible_pfn) };
        } else {
            p.flags |= TTM_PL_FLAG_TOPDOWN;
        }
        if b.tbo.type_ == ttm_bo_type_kernel && flags & AMDGPU_GEM_CREATE_VRAM_CONTIGUOUS as u64 != 0 {
            p.flags |= TTM_PL_FLAG_CONTIGUOUS;
        }
        places.push(p);
    }
    if domain & AMDGPU_GEM_DOMAIN_DOORBELL != 0 {
        places.push(ttm_place { fpfn: 0, lpfn: 0, mem_type: AMDGPU_PL_DOORBELL, flags: 0 });
    }
    if domain & AMDGPU_GEM_DOMAIN_MMIO_REMAP != 0 {
        places.push(ttm_place { fpfn: 0, lpfn: 0, mem_type: AMDGPU_PL_MMIO_REMAP, flags: 0 });
    }
    if domain & AMDGPU_GEM_DOMAIN_GTT != 0 {
        let mem_type = if flags & AMDGPU_GEM_CREATE_PREEMPTIBLE as u64 != 0 { AMDGPU_PL_PREEMPT } else { TTM_PL_TT };
        let mut p = ttm_place { fpfn: 0, lpfn: 0, mem_type, flags: 0 };
        // When GTT is just an alternative to VRAM make sure that we only
        // use it as fallback and still try to fill up VRAM first.
        if b.tbo.resource.is_some() && !is_apu && domain & b.preferred_domains & AMDGPU_GEM_DOMAIN_VRAM != 0 {
            p.flags |= TTM_PL_FLAG_FALLBACK;
        }
        places.push(p);
    }
    if domain & AMDGPU_GEM_DOMAIN_CPU != 0 {
        places.push(ttm_place { fpfn: 0, lpfn: 0, mem_type: TTM_PL_SYSTEM, flags: 0 });
    }
    if domain & AMDGPU_GEM_DOMAIN_GDS != 0 {
        places.push(ttm_place { fpfn: 0, lpfn: 0, mem_type: AMDGPU_PL_GDS, flags: 0 });
    }
    if domain & AMDGPU_GEM_DOMAIN_GWS != 0 {
        places.push(ttm_place { fpfn: 0, lpfn: 0, mem_type: AMDGPU_PL_GWS, flags: 0 });
    }
    if domain & AMDGPU_GEM_DOMAIN_OA != 0 {
        places.push(ttm_place { fpfn: 0, lpfn: 0, mem_type: AMDGPU_PL_OA, flags: 0 });
    }
    if places.is_empty() {
        places.push(ttm_place { fpfn: 0, lpfn: 0, mem_type: TTM_PL_SYSTEM, flags: 0 });
    }
    b.placements = places;
}

/// `amdgpu_bo_validate_size`.
fn amdgpu_bo_validate_size(adev: &amdgpu_device, size: u64, domain: u32) -> bool {
    let man = if domain & AMDGPU_GEM_DOMAIN_GTT != 0 {
        adev.mman.gtt_mgr.as_ref().map(|m| m.manager)
    } else if domain & AMDGPU_GEM_DOMAIN_VRAM != 0 {
        adev.mman.vram_mgr.as_ref().map(|m| m.manager)
    } else {
        return true;
    };
    let Some(man) = man else { return false };
    if size < man.size {
        return true;
    }
    false
}

/// `amdgpu_bo_support_uswc`: x86-64 with PAT.
fn amdgpu_bo_support_uswc(_bo_flags: u64) -> bool {
    true
}

/// `amdgpu_bo_create`: kernel buffers, no VRAM_CLEARED (that needs the
/// SDMA ring).
pub fn amdgpu_bo_create(adev: &mut amdgpu_device, bp: &amdgpu_bo_param) -> Result<usize, i32> {
    let mut size = bp.size;
    let page_align;
    // Note that GDS/GWS/OA allocates 1 page per byte/resource.
    if bp.domain & (AMDGPU_GEM_DOMAIN_GWS | AMDGPU_GEM_DOMAIN_OA) != 0 {
        // GWS and OA don't need any alignment.
        page_align = bp.byte_align;
        size <<= PAGE_SHIFT;
    } else if bp.domain & AMDGPU_GEM_DOMAIN_GDS != 0 {
        // Both size and alignment must be a multiple of 4.
        page_align = crate::linux::align(bp.byte_align, 4);
        size = crate::linux::align(size, 4) << PAGE_SHIFT;
    } else {
        // Memory should be aligned at least to a page size.
        page_align = crate::linux::align(bp.byte_align, PAGE_SIZE) >> PAGE_SHIFT;
        size = crate::linux::align(size, PAGE_SIZE);
    }
    if !amdgpu_bo_validate_size(adev, size, bp.domain) {
        return Err(-12);
    }
    let preferred_domains = if bp.preferred_domain != 0 { bp.preferred_domain } else { bp.domain };
    let mut allowed_domains = preferred_domains;
    if bp.type_ != ttm_bo_type_kernel && bp.flags & AMDGPU_GEM_CREATE_DISCARDABLE as u64 == 0
        && allowed_domains == AMDGPU_GEM_DOMAIN_VRAM {
        allowed_domains |= AMDGPU_GEM_DOMAIN_GTT;
    }
    let mut flags = bp.flags;
    if !amdgpu_bo_support_uswc(flags) {
        flags &= !(AMDGPU_GEM_CREATE_CPU_GTT_USWC as u64);
    }
    let b = amdgpu_bo {
        tbo: ttm_buffer_object { size, ..Default::default() },
        flags,
        preferred_domains,
        allowed_domains,
        placements: Vec::new(),
        // For GPUs without spatial partitioning
        xcp_id: 0,
        kptr: None,
    };
    let h = match adev.mman.bos.iter().position(|x| x.is_none()) {
        Some(i) => {
            adev.mman.bos[i] = Some(b);
            i
        }
        None => {
            adev.mman.bos.push(Some(b));
            adev.mman.bos.len() - 1
        }
    };
    bo_mut(adev, h).tbo.type_ = bp.type_;
    if bp.domain & (AMDGPU_GEM_DOMAIN_GWS | AMDGPU_GEM_DOMAIN_OA | AMDGPU_GEM_DOMAIN_GDS) != 0 {
        amdgpu_bo_placement_from_domain(adev, h, AMDGPU_GEM_DOMAIN_CPU);
    } else {
        amdgpu_bo_placement_from_domain(adev, h, bp.domain);
    }
    bo_mut(adev, h).tbo.priority = if bp.type_ == ttm_bo_type_kernel {
        2
    } else if bp.flags & AMDGPU_GEM_CREATE_DISCARDABLE as u64 == 0 {
        1
    } else {
        0
    };
    let r = amdgpu_ttm::ttm_bo_init_reserved(adev, h, bp.type_, page_align as u32);
    if r != 0 {
        adev.mman.bos[h] = None;
        return Err(r);
    }
    // Treat CPU_ACCESS_REQUIRED only as a hint if given by UMD
    if bp.type_ == ttm_bo_type_device {
        bo_mut(adev, h).flags &= !(AMDGPU_GEM_CREATE_CPU_ACCESS_REQUIRED as u64);
    }
    Ok(h)
}

/// `amdgpu_mem_type_to_domain`.
fn amdgpu_mem_type_to_domain(mem_type: u32) -> u32 {
    match mem_type {
        t if t == TTM_PL_VRAM => AMDGPU_GEM_DOMAIN_VRAM,
        t if t == TTM_PL_TT => AMDGPU_GEM_DOMAIN_GTT,
        t if t == TTM_PL_SYSTEM => AMDGPU_GEM_DOMAIN_CPU,
        t if t == AMDGPU_PL_GDS => AMDGPU_GEM_DOMAIN_GDS,
        t if t == AMDGPU_PL_GWS => AMDGPU_GEM_DOMAIN_GWS,
        t if t == AMDGPU_PL_OA => AMDGPU_GEM_DOMAIN_OA,
        t if t == AMDGPU_PL_DOORBELL => AMDGPU_GEM_DOMAIN_DOORBELL,
        t if t == AMDGPU_PL_MMIO_REMAP => AMDGPU_GEM_DOMAIN_MMIO_REMAP,
        _ => 0,
    }
}

/// `amdgpu_bo_pin`: no imported buffers, no user pages; Carrizo/Stoney's
/// preferred-domain rule is not Renoir's.
pub fn amdgpu_bo_pin(adev: &mut amdgpu_device, h: usize, mut domain: u32) -> i32 {
    let preferred = bo(adev, h).preferred_domains;
    // Check domain to be pinned to against preferred domains
    if preferred & domain != 0 {
        domain &= preferred;
    }
    if bo(adev, h).tbo.pin_count != 0 {
        let b = bo(adev, h);
        let res = b.tbo.resource.as_ref().unwrap();
        if domain & amdgpu_mem_type_to_domain(res.mem_type) == 0 {
            return -22;
        }
        if res.mem_type == TTM_PL_VRAM && b.flags & AMDGPU_GEM_CREATE_VRAM_CONTIGUOUS as u64 != 0
            && res.placement & TTM_PL_FLAG_CONTIGUOUS == 0 {
            return -22;
        }
        bo_mut(adev, h).tbo.pin_count += 1;
        return 0;
    }
    // force to pin into visible video ram
    if bo(adev, h).flags & AMDGPU_GEM_CREATE_NO_CPU_ACCESS as u64 == 0 {
        bo_mut(adev, h).flags |= AMDGPU_GEM_CREATE_CPU_ACCESS_REQUIRED as u64;
    }
    amdgpu_bo_placement_from_domain(adev, h, domain);
    let contiguous = bo(adev, h).flags & AMDGPU_GEM_CREATE_VRAM_CONTIGUOUS as u64 != 0;
    for p in bo_mut(adev, h).placements.iter_mut() {
        if contiguous && p.mem_type == TTM_PL_VRAM {
            p.flags |= TTM_PL_FLAG_CONTIGUOUS;
        }
    }
    let placement = bo(adev, h).placements.clone();
    let r = amdgpu_ttm::ttm_bo_validate(adev, h, &placement);
    if r != 0 {
        Line::new().s("pin failed").say();
        return r;
    }
    bo_mut(adev, h).tbo.pin_count += 1;
    0
}

/// `amdgpu_bo_unpin`.
pub fn amdgpu_bo_unpin(adev: &mut amdgpu_device, h: usize) {
    let b = bo_mut(adev, h);
    b.tbo.pin_count = b.tbo.pin_count.saturating_sub(1);
}

/// `amdgpu_gmc_sign_extend` (amdgpu_gmc.h).
pub fn amdgpu_gmc_sign_extend(addr: u64) -> u64 {
    if addr >= AMDGPU_GMC_HOLE_START { addr | AMDGPU_GMC_HOLE_END } else { addr }
}

/// `amdgpu_gmc_agp_addr` (amdgpu_gmc.c).
fn amdgpu_gmc_agp_addr(adev: &amdgpu_device, h: usize) -> u64 {
    let Some(tt) = bo(adev, h).tbo.ttm.as_ref() else { return AMDGPU_BO_INVALID_OFFSET };
    if tt.num_pages != 1 || tt.caching == crate::ttm::ttm_caching::ttm_cached {
        return AMDGPU_BO_INVALID_OFFSET;
    }
    if tt.dma_address(0) + PAGE_SIZE >= adev.gmc.agp_size {
        return AMDGPU_BO_INVALID_OFFSET;
    }
    adev.gmc.agp_start + tt.dma_address(0)
}

/// `amdgpu_bo_gpu_offset_no_check`.
pub fn amdgpu_bo_gpu_offset(adev: &amdgpu_device, h: usize) -> u64 {
    let res = bo(adev, h).tbo.resource.as_ref().unwrap();
    let mut offset = AMDGPU_BO_INVALID_OFFSET;
    if res.mem_type == TTM_PL_TT {
        offset = amdgpu_gmc_agp_addr(adev, h);
    }
    if offset == AMDGPU_BO_INVALID_OFFSET {
        offset = (res.start << PAGE_SHIFT) + amdgpu_ttm::amdgpu_ttm_domain_start(adev, res.mem_type);
    }
    amdgpu_gmc_sign_extend(offset)
}

/// `amdgpu_bo_kmap`: the kernel mapping, made once (`ttm_bo_kmap` on
/// the resource: VRAM through the mapped stolen memory, a TT through its
/// DMA buffer, a doorbell through BAR2).
pub fn amdgpu_bo_kmap(adev: &mut amdgpu_device, h: usize) -> Result<amdgpu_kptr, i32> {
    if bo(adev, h).flags & AMDGPU_GEM_CREATE_NO_CPU_ACCESS as u64 != 0 {
        return Err(-1);
    }
    if let Some(k) = bo(adev, h).kptr {
        return Ok(k);
    }
    let res = bo(adev, h).tbo.resource.clone().unwrap();
    let ttm = bo(adev, h).tbo.ttm;
    let k = match res.mem_type {
        t if t == TTM_PL_VRAM => {
            if adev.mman.aper_base_kaddr.is_none() {
                return Err(-12);
            }
            amdgpu_kptr::vram(res.start << PAGE_SHIFT)
        }
        t if t == TTM_PL_TT || t == AMDGPU_PL_PREEMPT || t == TTM_PL_SYSTEM => {
            match ttm.and_then(|t| t.dma) {
                Some((handle, _)) => amdgpu_kptr::gtt(handle),
                None => return Err(-12),
            }
        }
        t if t == AMDGPU_PL_DOORBELL => {
            let off = res.start << PAGE_SHIFT;
            // ioremap of the doorbell BAR (amdgpu_ttm_io_mem_reserve).
            if adev.doorbell_mmio.is_none() {
                let pages = (adev.doorbell.size >> PAGE_SHIFT) as i32;
                adev.doorbell_mmio = npk_sys::Mmio::map_bar(2, pages);
                if adev.doorbell_mmio.is_none() {
                    return Err(-12);
                }
            }
            amdgpu_kptr::doorbell(off)
        }
        _ => return Err(-22),
    };
    bo_mut(adev, h).kptr = Some(k);
    Ok(k)
}

/// `amdgpu_bo_create_reserved`: the buffer created and pinned, given a
/// GART address if it lives in GTT, mapped if `kmap`. None for size 0.
pub fn amdgpu_bo_create_reserved(adev: &mut amdgpu_device, size: u64, align: u64, domain: u32, kmap: bool)
                                 -> Result<Option<usize>, i32> {
    if size == 0 {
        return Ok(None);
    }
    let mut flags = if kmap { AMDGPU_GEM_CREATE_CPU_ACCESS_REQUIRED } else { AMDGPU_GEM_CREATE_NO_CPU_ACCESS } as u64;
    flags |= AMDGPU_GEM_CREATE_VRAM_CONTIGUOUS as u64;
    let bp = amdgpu_bo_param { size, byte_align: align, domain, flags, type_: ttm_bo_type_kernel, ..Default::default() };
    let h = match amdgpu_bo_create(adev, &bp) {
        Ok(h) => h,
        Err(r) => {
            Line::new().s("(").dec((-r) as u64).s(") failed to allocate kernel bo").say();
            return Err(r);
        }
    };
    let r = amdgpu_bo_pin(adev, h, domain);
    if r != 0 {
        Line::new().s("(").dec((-r) as u64).s(") kernel bo pin failed").say();
        amdgpu_bo_unref(adev, h);
        return Err(r);
    }
    let r = amdgpu_ttm::amdgpu_ttm_alloc_gart(adev, h);
    if r != 0 {
        Line::new().s("bind failed").say();
        amdgpu_bo_unpin(adev, h);
        amdgpu_bo_unref(adev, h);
        return Err(r);
    }
    if kmap {
        if let Err(r) = amdgpu_bo_kmap(adev, h) {
            Line::new().s("(").dec((-r) as u64).s(") kernel bo map failed").say();
            amdgpu_bo_unpin(adev, h);
            amdgpu_bo_unref(adev, h);
            return Err(r);
        }
    }
    Ok(Some(h))
}

/// `amdgpu_bo_create_kernel`.
pub fn amdgpu_bo_create_kernel(adev: &mut amdgpu_device, size: u64, align: u64, domain: u32, kmap: bool)
                               -> Result<Option<usize>, i32> {
    amdgpu_bo_create_reserved(adev, size, align, domain, kmap)
}

/// `amdgpu_bo_create_kernel_at`: VRAM at a fixed offset.
pub fn amdgpu_bo_create_kernel_at(adev: &mut amdgpu_device, offset: u64, size: u64, kmap: bool)
                                  -> Result<Option<usize>, i32> {
    let offset = offset & !(PAGE_SIZE - 1);
    let size = crate::linux::align(size, PAGE_SIZE);
    let Some(h) = amdgpu_bo_create_reserved(adev, size, PAGE_SIZE, AMDGPU_GEM_DOMAIN_VRAM, kmap)? else {
        return Ok(None);
    };
    // Remove the original mem node and create a new one at the request
    // position.
    if kmap {
        bo_mut(adev, h).kptr = None;
    }
    amdgpu_ttm::ttm_resource_free(adev, h);
    for p in bo_mut(adev, h).placements.iter_mut() {
        p.fpfn = (offset >> PAGE_SHIFT) as u32;
        p.lpfn = ((offset + size) >> PAGE_SHIFT) as u32;
    }
    let placement = bo(adev, h).placements.clone();
    match amdgpu_ttm::ttm_bo_mem_space(adev, h, &placement) {
        Ok(res) => bo_mut(adev, h).tbo.resource = Some(res),
        Err(r) => {
            amdgpu_bo_unref(adev, h);
            return Err(r);
        }
    }
    if kmap {
        if let Err(r) = amdgpu_bo_kmap(adev, h) {
            amdgpu_bo_unref(adev, h);
            return Err(r);
        }
    }
    Ok(Some(h))
}

/// `amdgpu_bo_unref` of the last reference: the resource and pages go
/// back.
pub fn amdgpu_bo_unref(adev: &mut amdgpu_device, h: usize) {
    amdgpu_ttm::ttm_resource_free(adev, h);
    if let Some(tt) = bo(adev, h).tbo.ttm {
        if let Some((handle, phys)) = tt.dma {
            (npk_sys::Dma { handle, phys }).free();
        }
    }
    adev.mman.bos[h] = None;
}

/// `amdgpu_bo_free_kernel`.
pub fn amdgpu_bo_free_kernel(adev: &mut amdgpu_device, h: &mut Option<usize>) {
    let Some(b) = h.take() else { return };
    bo_mut(adev, b).kptr = None;
    amdgpu_bo_unpin(adev, b);
    amdgpu_bo_unref(adev, b);
}

/// `amdgpu_bo_size`.
pub fn amdgpu_bo_size(adev: &amdgpu_device, h: usize) -> u64 {
    bo(adev, h).tbo.size
}

/// `amdgpu_bo_init`: write-combining for the aperture is the kernel's
/// mapping of it (`arch_io_reserve_memtype_wc`, `arch_phys_wc_add`).
pub fn amdgpu_bo_init(adev: &mut amdgpu_device) -> i32 {
    Line::new().s("Detected VRAM RAM=").dec(adev.gmc.mc_vram_size >> 20).s("M, BAR=")
        .dec(adev.gmc.aper_size >> 20).s("M").say();
    let name = crate::tables::amdgpu_vram_names.get(adev.gmc.vram_type as usize).copied().unwrap_or(b"?");
    Line::new().s("RAM width ").dec(adev.gmc.vram_width as u64).s("bits ").bytes(name).say();
    amdgpu_ttm::amdgpu_ttm_init(adev)
}
