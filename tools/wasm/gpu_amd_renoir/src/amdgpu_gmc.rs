//! amdgpu/amdgpu_gmc.c (Linux 6.18.55): where VRAM, GART and AGP sit in
//! the GPU's address space, and what the pre-OS display keeps.

use crate::amdgpu_device::amdgpu_device;
use crate::linux::{align, Line};
use crate::regs::*;
use crate::tables;

const FOUR_GB: u64 = 0x1_0000_0000;

/// `amdgpu_gmc_set_agp_default`.
pub fn amdgpu_gmc_set_agp_default(adev: &mut amdgpu_device) {
    let mc = &mut adev.gmc;
    mc.agp_start = 0xffff_ffff_ffff;
    mc.agp_end = 0;
    mc.agp_size = 0;
}

/// `amdgpu_gmc_is_pdb0_enabled`: bare metal, no XGMI to the CPU.
pub fn amdgpu_gmc_is_pdb0_enabled(adev: &amdgpu_device) -> bool {
    adev.gmc.xgmi.connected_to_cpu
}

/// `amdgpu_gmc_vram_full_visible` (amdgpu_gmc.h).
pub fn amdgpu_gmc_vram_full_visible(gmc: &crate::amdgpu_device::amdgpu_gmc) -> bool {
    if gmc.real_vram_size < gmc.visible_vram_size {
        Line::new().s("WARNING: amdgpu_gmc_vram_full_visible: visible VRAM larger than VRAM").say();
    }
    gmc.real_vram_size == gmc.visible_vram_size
}

/// `amdgpu_gmc_vram_location`.
pub fn amdgpu_gmc_vram_location(adev: &mut amdgpu_device, base: u64) {
    let vis_limit = (tables::amdgpu_vis_vram_limit[0] as u64) << 20;
    let limit = (tables::amdgpu_vram_limit[0] as i64 as u64) << 20;
    let mc = &mut adev.gmc;
    mc.vram_start = base;
    mc.vram_end = mc.vram_start + mc.mc_vram_size - 1;
    if limit < mc.real_vram_size {
        mc.real_vram_size = limit;
    }
    if vis_limit != 0 && vis_limit < mc.visible_vram_size {
        mc.visible_vram_size = vis_limit;
    }
    if mc.real_vram_size < mc.visible_vram_size {
        mc.visible_vram_size = mc.real_vram_size;
    }
    if mc.xgmi.num_physical_nodes == 0 {
        mc.fb_start = mc.vram_start;
        mc.fb_end = mc.vram_end;
    }
    Line::new().s("VRAM: ").dec(mc.mc_vram_size >> 20).s("M ").hex64(mc.vram_start).s(" - ").hex64(mc.vram_end)
        .s(" (").dec(mc.real_vram_size >> 20).s("M used)").say();
}

/// `amdgpu_gmc_gart_location`.
pub fn amdgpu_gmc_gart_location(adev: &mut amdgpu_device, gart_placement: u32) {
    // To avoid the hole, limit the max mc address to AMDGPU_GMC_HOLE_START
    let max_mc_address = adev.gmc.mc_mask.min(AMDGPU_GMC_HOLE_START - 1);
    let mc = &mut adev.gmc;
    // VCE doesn't like it when BOs cross a 4GB segment, so align the GART
    // base on a 4GB boundary as well.
    let size_bf = mc.fb_start;
    let size_af = max_mc_address + 1 - align(mc.fb_end + 1, FOUR_GB);
    if mc.gart_size > size_bf.max(size_af) {
        Line::new().s("limiting GART").say();
        mc.gart_size = size_bf.max(size_af);
    }
    // AMDGPU_GART_PLACEMENT_BEST_FIT; HIGH and LOW are not asked for here.
    let _ = gart_placement;
    if (size_bf >= mc.gart_size && size_bf < size_af) || size_af < mc.gart_size {
        mc.gart_start = 0;
    } else {
        mc.gart_start = max_mc_address - mc.gart_size + 1;
    }
    mc.gart_start &= !(FOUR_GB - 1);
    mc.gart_end = mc.gart_start + mc.gart_size - 1;
    Line::new().s("GART: ").dec(mc.gart_size >> 20).s("M ").hex64(mc.gart_start).s(" - ").hex64(mc.gart_end).say();
}

/// `amdgpu_gmc_agp_location`.
pub fn amdgpu_gmc_agp_location(adev: &mut amdgpu_device) {
    let sixteen_gb: u64 = 1 << 34;
    let sixteen_gb_mask = !(sixteen_gb - 1);
    let mc = &mut adev.gmc;
    let (size_bf, size_af) = if mc.fb_start > mc.gart_start {
        ((mc.fb_start & sixteen_gb_mask).wrapping_sub(align(mc.gart_end + 1, sixteen_gb)),
         (mc.mc_mask + 1).wrapping_sub(align(mc.fb_end + 1, sixteen_gb)))
    } else {
        (mc.fb_start & sixteen_gb_mask,
         (mc.gart_start & sixteen_gb_mask).wrapping_sub(align(mc.fb_end + 1, sixteen_gb)))
    };
    if size_bf > size_af {
        mc.agp_start = (mc.fb_start - size_bf) & sixteen_gb_mask;
        mc.agp_size = size_bf;
    } else {
        mc.agp_start = align(mc.fb_end + 1, sixteen_gb);
        mc.agp_size = size_af;
    }
    mc.agp_end = mc.agp_start.wrapping_add(mc.agp_size).wrapping_sub(1);
    Line::new().s("AGP: ").dec(mc.agp_size >> 20).s("M ").hex64(mc.agp_start).s(" - ").hex64(mc.agp_end).say();
}

/// `amdgpu_device_has_display_hardware` (amdgpu_device.c), the IP
/// discovery case.
pub fn amdgpu_device_has_display_hardware(adev: &amdgpu_device) -> bool {
    !(adev.ip_version(DCE_HWIP, 0) == 0 || adev.harvest_ip_mask & AMD_HARVEST_IP_DMU_MASK != 0)
}

/// `amdgpu_gmc_get_vbios_allocations`: bare metal, CHIP_RENOIR.
pub fn amdgpu_gmc_get_vbios_allocations(adev: &mut amdgpu_device) {
    // Some ASICs need to reserve a region of video memory to avoid access
    // from driver
    adev.mman.stolen_reserved_offset = 0;
    adev.mman.stolen_reserved_size = 0;
    // CHIP_RAVEN and CHIP_RENOIR keep the stolen VGA memory: "some memory
    // client outside of the driver writes to first 8M of VRAM on S3 resume".
    adev.mman.keep_stolen_vga_memory = adev.asic_type == CHIP_RENOIR;

    let mut size = if !amdgpu_device_has_display_hardware(adev) {
        0
    } else {
        let mut size = crate::gmc_v9_0::gmc_v9_0_get_vbios_fb_size(adev);
        if adev.mman.keep_stolen_vga_memory {
            size = size.max(AMDGPU_VBIOS_VGA_ALLOCATION);
        }
        size
    } as u64;

    // set to 0 if the pre-OS buffer uses up most of vram
    if adev.gmc.real_vram_size.wrapping_sub(size) < 8 * 1024 * 1024 {
        size = 0;
    }
    if size > AMDGPU_VBIOS_VGA_ALLOCATION as u64 {
        adev.mman.stolen_vga_size = AMDGPU_VBIOS_VGA_ALLOCATION as u64;
        adev.mman.stolen_extended_size = size - adev.mman.stolen_vga_size;
    } else {
        adev.mman.stolen_vga_size = size;
        adev.mman.stolen_extended_size = 0;
    }
}

/// `amdgpu_gmc_vram_mc2pa`.
pub fn amdgpu_gmc_vram_mc2pa(adev: &amdgpu_device, mc_addr: u64) -> u64 {
    mc_addr.wrapping_sub(adev.gmc.vram_start).wrapping_add(adev.vm_manager.vram_base_offset)
}

/// `amdgpu_gmc_get_pde_for_bo`: the address and flags a directory entry
/// pointing at `h` gets.
pub fn amdgpu_gmc_get_pde_for_bo(adev: &amdgpu_device, h: usize, level: i32) -> (u64, u64) {
    let b = crate::amdgpu_object::bo(adev, h);
    let res = b.tbo.resource.as_ref().unwrap();
    let mut addr = match res.mem_type {
        t if t == TTM_PL_TT => crate::amdgpu_object::tt_page(b, 0),
        t if t == TTM_PL_VRAM => crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h),
        _ => 0,
    };
    let mut flags = crate::amdgpu_ttm::amdgpu_ttm_tt_pde_flags(b.tbo.ttm.as_ref(), Some(res));
    crate::gmc_v9_0::gmc_v9_0_get_vm_pde(adev, level, &mut addr, &mut flags);
    (addr, flags)
}

/// `amdgpu_gmc_pd_addr`.
pub fn amdgpu_gmc_pd_addr(adev: &amdgpu_device, h: usize) -> u64 {
    let (pd_addr, flags) = amdgpu_gmc_get_pde_for_bo(adev, h, -1);
    pd_addr | flags
}

/// `amdgpu_gmc_allocate_vm_inv_eng`: no MES, no UMSCH, no CPER ring, and
/// without an SDMA page queue no ring shares an engine.
pub fn amdgpu_gmc_allocate_vm_inv_eng(adev: &mut amdgpu_device) -> i32 {
    let mut vm_inv_engs = [0u32; AMDGPU_MAX_VMHUBS as usize];
    // init the vm inv eng for all vmhubs
    for i in 0..AMDGPU_MAX_VMHUBS as usize {
        if adev.vmhubs_mask & (1 << i) != 0 {
            vm_inv_engs[i] = AMDGPU_VMHUB_INV_ENG_BITMAP;
        }
    }
    for r in 0..adev.rings.len() {
        let vmhub = adev.rings[r].vm_hub as usize;
        let inv_eng = vm_inv_engs[vmhub].trailing_zeros();
        if vm_inv_engs[vmhub] == 0 {
            Line::new().s("no VM inv eng for ring ").s(&adev.rings[r].name.clone()).say();
            return -22;
        }
        adev.rings[r].vm_inv_eng = inv_eng;
        vm_inv_engs[vmhub] &= !(1 << inv_eng);
        let ring = &adev.rings[r];
        Line::new().s("ring ").s(&ring.name).s(" uses VM inv eng ").dec(ring.vm_inv_eng as u64).s(" on hub ")
            .dec(ring.vm_hub as u64).say();
    }
    0
}


/// `amdgpu_gmc_flush_gpu_tlb`. `hub->sdma_invalidation_workaround` is set
/// only by gfxhub_v2_0 (Navi 1x), so the flush through an SDMA job is
/// unreachable on GC 9.3 and every flush takes the `gmc_funcs` path.
pub fn amdgpu_gmc_flush_gpu_tlb(adev: &mut amdgpu_device, vmid: u32, vmhub: u32, flush_type: u32) {
    // A GPU reset should flush all TLBs anyway, so no need to do this
    // while one is ongoing. The reset holds the reset domain's semaphore
    // exactly while `amdgpu_in_reset`, so that is when the trylock fails.
    if crate::amdgpu_device::amdgpu_in_reset(adev) {
        return;
    }
    if adev.gmc.flush_tlb_needs_extra_type_2 {
        crate::gmc_v9_0::gmc_v9_0_flush_gpu_tlb(adev, vmid, vmhub, 2);
    }
    if adev.gmc.flush_tlb_needs_extra_type_0 && flush_type == 2 {
        crate::gmc_v9_0::gmc_v9_0_flush_gpu_tlb(adev, vmid, vmhub, 0);
    }
    crate::gmc_v9_0::gmc_v9_0_flush_gpu_tlb(adev, vmid, vmhub, flush_type);
}

/// `amdgpu_gmc_fw_reg_write_reg_wait`: GC 9 has no MES, so the write and
/// wait go through the KIQ. One XCC: `xcc_inst` is always 0. The module
/// is single-threaded and runs no interrupt context, so `kiq->ring_lock`
/// guards nothing, `in_interrupt()` is false and the reset semaphore is
/// never contended (`amdgpu_reset_pending`).
pub fn amdgpu_gmc_fw_reg_write_reg_wait(adev: &mut amdgpu_device, reg0: u32, reg1: u32, ref_: u32, mask: u32,
                                        xcc_inst: u32) {
    use crate::amdgpu_fence::{amdgpu_fence_emit_polling, amdgpu_fence_wait_polling};
    use crate::amdgpu_ring as ring;
    let _ = xcc_inst;
    let Some(kiq) = adev.gfx.kiq.ring else { return };
    ring::amdgpu_ring_alloc(adev, kiq, 32);
    ring::amdgpu_ring_emit_reg_write_reg_wait(adev, kiq, reg0, reg1, ref_, mask);
    let seq = match amdgpu_fence_emit_polling(adev, kiq, MAX_KIQ_REG_WAIT as i64) {
        Ok(seq) => seq,
        Err(_) => {
            // failed_undo
            ring::amdgpu_ring_undo(adev, kiq);
            Line::new().s("failed to write reg ").hex(reg0).s(" wait reg ").hex(reg1).say();
            return;
        }
    };
    ring::amdgpu_ring_commit(adev, kiq);
    let mut r = amdgpu_fence_wait_polling(adev, kiq, seq, MAX_KIQ_REG_WAIT as i64);
    let mut cnt: i64 = 0;
    while r < 1 {
        let tried = cnt;
        cnt += 1;
        if tried >= MAX_KIQ_REG_TRY as i64 {
            break;
        }
        crate::linux::msleep(MAX_KIQ_REG_BAILOUT_INTERVAL as u64);
        r = amdgpu_fence_wait_polling(adev, kiq, seq, MAX_KIQ_REG_WAIT as i64);
    }
    if cnt > MAX_KIQ_REG_TRY as i64 {
        // failed_kiq
        Line::new().s("failed to write reg ").hex(reg0).s(" wait reg ").hex(reg1).say();
    }
}
