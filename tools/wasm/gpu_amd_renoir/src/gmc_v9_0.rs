//! amdgpu/gmc_v9_0.c (Linux 6.18.55): the GMC block's early_init.

use crate::amdgpu_device::amdgpu_device;
use crate::linux::Line;
use crate::regs::*;

/// `struct amdgpu_irq_src`, as early_init sets it: the type count and
/// the name of the `funcs` table.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_irq_src {
    pub num_types: u32,
    pub funcs: Option<&'static str>,
}

/// `gmc_v9_0_set_gmc_funcs`.
fn gmc_v9_0_set_gmc_funcs(adev: &mut amdgpu_device) {
    adev.gmc.gmc_funcs = Some("gmc_v9_0_gmc_funcs");
}

/// `gmc_v9_0_set_irq_funcs`. Bare metal.
fn gmc_v9_0_set_irq_funcs(adev: &mut amdgpu_device) {
    adev.gmc.vm_fault = amdgpu_irq_src { num_types: 1, funcs: Some("gmc_v9_0_irq_funcs") };
    if !adev.gmc.xgmi.connected_to_cpu && !adev.gmc.is_app_apu {
        adev.gmc.ecc_irq = amdgpu_irq_src { num_types: 1, funcs: Some("gmc_v9_0_ecc_funcs") };
    }
}

/// `gmc_v9_0_set_mmhub_funcs`: MMHUB 1.5.0, the default case.
fn gmc_v9_0_set_mmhub_funcs(adev: &mut amdgpu_device) {
    adev.mmhub_funcs = Some("mmhub_v1_0_funcs");
}

/// `gmc_v9_0_set_gfxhub_funcs`: not multi-AID.
fn gmc_v9_0_set_gfxhub_funcs(adev: &mut amdgpu_device) {
    adev.gfxhub_funcs = Some(if adev.aid_mask != 0 { "gfxhub_v1_2_funcs" } else { "gfxhub_v1_0_funcs" });
}

/// `gmc_v9_0_set_hdp_ras_funcs`.
fn gmc_v9_0_set_hdp_ras_funcs(adev: &mut amdgpu_device) {
    adev.hdp_ras = Some("hdp_v4_0_ras");
}

/// `gmc_v9_0_set_xgmi_ras_funcs`.
fn gmc_v9_0_set_xgmi_ras_funcs(adev: &mut amdgpu_device) {
    if !adev.gmc.xgmi.connected_to_cpu {
        adev.gmc.xgmi.ras = Some("xgmi_ras");
    }
}

/// `gmc_v9_0_early_init`. GC 9.3.0 with no XGMI and one AID: no XGMI and
/// no app APU; UMC 7.6.0 and MMHUB 1.5.0 take no umc, mmhub-ras or mca
/// funcs.
pub fn gmc_v9_0_early_init(adev: &mut amdgpu_device) -> i32 {
    // 9.4.0, 9.4.1 and 9.4.3 don't have XGMI defined
    // in their IP discovery tables
    let gc = adev.ip_version(GC_HWIP, 0);
    if gc == 9 << 24 | 4 << 16 || gc == 9 << 24 | 4 << 16 | 1 << 8 || adev.aid_mask != 0 {
        adev.gmc.xgmi.supported = true;
    }
    // XGMI 6.1.0 (is_host_gpu_xgmi_supported) and GC 9.4.3 (get_pkg_type)
    // are not Renoir's.

    gmc_v9_0_set_gmc_funcs(adev);
    gmc_v9_0_set_irq_funcs(adev);
    gmc_v9_0_set_mmhub_funcs(adev);
    gmc_v9_0_set_gfxhub_funcs(adev);
    gmc_v9_0_set_hdp_ras_funcs(adev);
    gmc_v9_0_set_xgmi_ras_funcs(adev);

    adev.gmc.shared_aperture_start = 0x2000000000000000;
    adev.gmc.shared_aperture_end = adev.gmc.shared_aperture_start + (4u64 << 30) - 1;
    adev.gmc.private_aperture_start = 0x1000000000000000;
    adev.gmc.private_aperture_end = adev.gmc.private_aperture_start + (4u64 << 30) - 1;
    adev.gmc.noretry_flags = AMDGPU_VM_NORETRY_FLAGS_TF;
    0
}

/// `gmc_v9_0_get_vbios_fb_size`: DCN 2.1.
pub fn gmc_v9_0_get_vbios_fb_size(adev: &amdgpu_device) -> u32 {
    use crate::nbio_v7_0::rreg32_soc15;
    let d1vga_control = rreg32_soc15(adev, DCE_HWIP, 0, mmD1VGA_CONTROL_BASE_IDX, mmD1VGA_CONTROL).unwrap_or(0);
    // TODO move to DC so GMC doesn't need to hard-code DCN registers
    if (d1vga_control & D1VGA_CONTROL__D1VGA_MODE_ENABLE_MASK) >> D1VGA_CONTROL__D1VGA_MODE_ENABLE__SHIFT != 0 {
        return AMDGPU_VBIOS_VGA_ALLOCATION;
    }
    if adev.ip_version(DCE_HWIP, 0) != 2 << 24 | 1 << 16 {
        // DCE 1.0.x and the SCL0 default are other chips'.
        return 0;
    }
    let viewport = rreg32_soc15(adev, DCE_HWIP, 0, mmHUBP0_DCSURF_PRI_VIEWPORT_DIMENSION_DCN2_BASE_IDX,
        mmHUBP0_DCSURF_PRI_VIEWPORT_DIMENSION_DCN2).unwrap_or(0);
    let h = (viewport & HUBP0_DCSURF_PRI_VIEWPORT_DIMENSION__PRI_VIEWPORT_HEIGHT_MASK)
        >> HUBP0_DCSURF_PRI_VIEWPORT_DIMENSION__PRI_VIEWPORT_HEIGHT__SHIFT;
    let w = (viewport & HUBP0_DCSURF_PRI_VIEWPORT_DIMENSION__PRI_VIEWPORT_WIDTH_MASK)
        >> HUBP0_DCSURF_PRI_VIEWPORT_DIMENSION__PRI_VIEWPORT_WIDTH__SHIFT;
    h * w * 4
}

/// `gmc_v9_0_vram_gtt_location`: not pdb0, `amdgpu_agp` auto.
fn gmc_v9_0_vram_gtt_location(adev: &mut amdgpu_device) {
    let mut base = crate::mmhub_v1_0::mmhub_v1_0_get_fb_location(adev);
    crate::amdgpu_gmc::amdgpu_gmc_set_agp_default(adev);
    // add the xgmi offset of the physical node
    base += adev.gmc.xgmi.physical_node_id as u64 * adev.gmc.xgmi.node_segment_size;
    crate::amdgpu_gmc::amdgpu_gmc_vram_location(adev, base);
    crate::amdgpu_gmc::amdgpu_gmc_gart_location(adev, AMDGPU_GART_PLACEMENT_BEST_FIT);
    if crate::tables::amdgpu_agp[0] == 1 {
        crate::amdgpu_gmc::amdgpu_gmc_agp_location(adev);
    }
    // base offset of vram pages
    adev.vm_manager.vram_base_offset = crate::gfxhub_v1_0::gfxhub_v1_0_get_mc_fb_offset(adev)
        + adev.gmc.xgmi.physical_node_id as u64 * adev.gmc.xgmi.node_segment_size;
}

/// `gmc_v9_0_mc_init`: an APU on bare metal, so the aperture is the
/// stolen system memory itself.
fn gmc_v9_0_mc_init(adev: &mut amdgpu_device) -> i32 {
    // size in MB on si
    adev.gmc.mc_vram_size = crate::nbio_v7_0::nbio_v7_0_get_memsize(adev) as u64 * 1024 * 1024;
    adev.gmc.real_vram_size = adev.gmc.mc_vram_size;
    // APU: no amdgpu_device_resize_fb_bar.
    adev.gmc.aper_base = adev.bar0.0;
    adev.gmc.aper_size = adev.bar0.1;
    // For APUs, VRAM is just the stolen system memory and can be accessed
    // directly.
    adev.gmc.aper_base = crate::gfxhub_v1_0::gfxhub_v1_0_get_mc_fb_offset(adev)
        + adev.gmc.xgmi.physical_node_id as u64 * adev.gmc.xgmi.node_segment_size;
    adev.gmc.aper_size = adev.gmc.real_vram_size;
    adev.gmc.visible_vram_size = adev.gmc.aper_size;

    // set the gart size
    if crate::tables::amdgpu_gart_size[0] == -1 {
        adev.gmc.gart_size = if adev.ip_version(GC_HWIP, 0) == 9 << 24 | 3 << 16 { 1024 << 20 } else { 512 << 20 };
    } else {
        adev.gmc.gart_size = (crate::tables::amdgpu_gart_size[0] as u64) << 20;
    }
    adev.gmc.gart_size += adev.pm.smu_prv_buffer_size as u64;
    gmc_v9_0_vram_gtt_location(adev);
    0
}

/// `gmc_v9_0_sw_init` up to `amdgpu_bo_init`, the part that only reads:
/// the hubs' registers, the VRAM type from the VBIOS, the VM size, the
/// fault sources, the DMA mask and the address space layout.
pub fn gmc_v9_0_sw_init_layout(adev: &mut amdgpu_device) -> i32 {
    crate::gfxhub_v1_0::gfxhub_v1_0_init(adev);
    crate::mmhub_v1_0::mmhub_v1_0_init(adev);

    if adev.bios.is_empty() {
        adev.gmc.vram_type = AMDGPU_VRAM_TYPE_DDR4;
        adev.gmc.vram_width = 64 * 64;
    } else {
        let (vram_width, vram_type, vram_vendor) =
            crate::amdgpu_atomfirmware::amdgpu_atomfirmware_get_vram_info(adev).unwrap_or((0, 0, 0));
        if crate::tables::amdgpu_emu_mode[0] != 1 {
            adev.gmc.vram_width = vram_width;
        }
        // An APU's df_v1_7 has get_hbm_channel_number, but only an HBM
        // width of 0 asks it.
        adev.gmc.vram_type = vram_type;
        adev.gmc.vram_vendor = vram_vendor;
    }
    // GC 9.3.0: GFXHUB(0) and MMHUB0(0); 4-level page support, vm size
    // 256TB (48bit), block size 512 (9bit).
    adev.vmhubs_mask |= 1 << AMDGPU_GFXHUB_0 | 1 << AMDGPU_MMHUB0_0;
    crate::amdgpu_vm::amdgpu_vm_adjust_size(adev, 256 * 1024, 9, 3, 48);

    // This interrupt is VMC page fault.
    let vm_fault = adev.gmc.vm_fault;
    let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_VMC, VMC_1_0__SRCID__VM_FAULT,
        &vm_fault, "gmc.vm_fault");
    if r != 0 {
        return r;
    }
    let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_UTCL2, UTCL2_1_0__SRCID__FAULT,
        &vm_fault, "gmc.vm_fault");
    if r != 0 {
        return r;
    }
    if !adev.gmc.xgmi.connected_to_cpu && !adev.gmc.is_app_apu {
        // interrupt sent to DF.
        let ecc_irq = adev.gmc.ecc_irq;
        let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_DF, 0, &ecc_irq, "gmc.ecc_irq");
        if r != 0 {
            return r;
        }
    }

    // Set the internal MC address mask: the max address of the GPU's
    // internal address space.
    adev.gmc.mc_mask = 0xffff_ffff_ffff; // 48 bit MC
    let dma_addr_bits = if adev.ip_version(GC_HWIP, 0) >= 9 << 24 | 4 << 16 | 2 << 8 { 48 } else { 44 };
    if npk_sys::dma_set_mask(dma_addr_bits) != 0 {
        crate::linux::Line::new().s("amdgpu: No suitable DMA available.").say();
        return -5;
    }
    // drm_need_swiotlb: every buffer the kernel hands out lies below the mask.
    adev.need_swiotlb = false;

    let r = gmc_v9_0_mc_init(adev);
    if r != 0 {
        return r;
    }
    crate::amdgpu_gmc::amdgpu_gmc_get_vbios_allocations(adev);
    0
}

/// `gmc_v9_0_use_invalidate_semaphore`: the MMHUB of anything but a
/// Picasso, on bare metal.
fn gmc_v9_0_use_invalidate_semaphore(adev: &amdgpu_device, vmhub: u32) -> bool {
    if adev.ip_version(GC_HWIP, 0) == 9 << 24 | 4 << 16 | 2 << 8 || adev.aid_mask != 0 {
        return false;
    }
    vmhub == AMDGPU_MMHUB0_0
        && !(adev.apu_flags & AMD_APU_IS_RAVEN2 == 0 && adev.apu_flags & AMD_APU_IS_PICASSO != 0)
}

/// `REG_SET_FIELD`.
fn set_field(v: u32, mask: u32, shift: u32, f: u32) -> u32 {
    (v & !mask) | (mask & (f << shift))
}

/// `gmc_v9_0_get_invalidate_req`.
fn gmc_v9_0_get_invalidate_req(vmid: u32, flush_type: u32) -> u32 {
    let mut req = 0;
    req = set_field(req, VM_INVALIDATE_ENG0_REQ__PER_VMID_INVALIDATE_REQ_MASK,
        VM_INVALIDATE_ENG0_REQ__PER_VMID_INVALIDATE_REQ__SHIFT, 1 << vmid);
    req = set_field(req, VM_INVALIDATE_ENG0_REQ__FLUSH_TYPE_MASK, VM_INVALIDATE_ENG0_REQ__FLUSH_TYPE__SHIFT, flush_type);
    req = set_field(req, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L2_PTES_MASK, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L2_PTES__SHIFT, 1);
    req = set_field(req, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L2_PDE0_MASK, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L2_PDE0__SHIFT, 1);
    req = set_field(req, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L2_PDE1_MASK, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L2_PDE1__SHIFT, 1);
    req = set_field(req, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L2_PDE2_MASK, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L2_PDE2__SHIFT, 1);
    req = set_field(req, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L1_PTES_MASK, VM_INVALIDATE_ENG0_REQ__INVALIDATE_L1_PTES__SHIFT, 1);
    set_field(req, VM_INVALIDATE_ENG0_REQ__CLEAR_PROTECTION_FAULT_STATUS_ADDR_MASK,
        VM_INVALIDATE_ENG0_REQ__CLEAR_PROTECTION_FAULT_STATUS_ADDR__SHIFT, 0)
}

/// `gmc_v9_0_get_vm_pde`.
pub fn gmc_v9_0_get_vm_pde(adev: &amdgpu_device, level: i32, addr: &mut u64, flags: &mut u64) {
    if *flags & AMDGPU_PDE_PTE == 0 && *flags & AMDGPU_PTE_SYSTEM as u64 == 0 {
        *addr = crate::amdgpu_gmc::amdgpu_gmc_vram_mc2pa(adev, *addr);
    }
    if *addr & 0xFFFF00000000003F != 0 {
        Line::new().s("BUG: page directory entry address 0x").hexw((*addr >> 32) as u32, 8).hexw(*addr as u32, 8)
            .say();
    }
    if !adev.gmc.translate_further {
        return;
    }
    if level == AMDGPU_VM_PDB1 as i32 {
        // Set the block fragment size
        if *flags & AMDGPU_PDE_PTE == 0 {
            *flags |= AMDGPU_PDE_BFS_9;
        }
    } else if level == AMDGPU_VM_PDB0 as i32 {
        if *flags & AMDGPU_PDE_PTE != 0 {
            *flags &= !AMDGPU_PDE_PTE;
            if *flags & AMDGPU_PTE_VALID as u64 == 0 {
                *addr |= 1 << 12;
            }
        } else {
            *flags |= AMDGPU_PTE_TF;
        }
    }
}

/// `gmc_v9_0_get_coherence_flags`: GC 9.3.0 is the default case.
fn gmc_v9_0_get_coherence_flags(adev: &amdgpu_device, bo: usize, flags: &mut u64) {
    let b = crate::amdgpu_object::bo(adev, bo);
    let is_vram = b.tbo.resource.as_ref().is_some_and(|r| r.mem_type == TTM_PL_VRAM);
    let coherent = b.flags & (AMDGPU_GEM_CREATE_COHERENT | AMDGPU_GEM_CREATE_EXT_COHERENT) as u64 != 0;
    let uncached = b.flags & AMDGPU_GEM_CREATE_UNCACHED as u64 != 0;
    let mtype_uc = uncached || coherent;
    let snoop = !is_vram;
    if mtype_uc {
        *flags = (*flags & !AMDGPU_PTE_MTYPE_VG10_MASK) | AMDGPU_PTE_MTYPE_VG10_UC;
    }
    if snoop {
        *flags |= AMDGPU_PTE_SNOOPED as u64;
    }
}

/// `gmc_v9_0_get_vm_pte`.
pub fn gmc_v9_0_get_vm_pte(adev: &amdgpu_device, bo: Option<usize>, vm_flags: u32, flags: &mut u64) {
    if vm_flags & AMDGPU_VM_PAGE_EXECUTABLE != 0 {
        *flags |= AMDGPU_PTE_EXECUTABLE as u64;
    } else {
        *flags &= !(AMDGPU_PTE_EXECUTABLE as u64);
    }
    let mtype = match vm_flags & AMDGPU_VM_MTYPE_MASK {
        AMDGPU_VM_MTYPE_WC => AMDGPU_PTE_MTYPE_VG10_WC,
        AMDGPU_VM_MTYPE_RW => AMDGPU_PTE_MTYPE_VG10_RW,
        AMDGPU_VM_MTYPE_CC => AMDGPU_PTE_MTYPE_VG10_CC,
        AMDGPU_VM_MTYPE_UC => AMDGPU_PTE_MTYPE_VG10_UC,
        // AMDGPU_VM_MTYPE_DEFAULT, AMDGPU_VM_MTYPE_NC
        _ => AMDGPU_PTE_MTYPE_VG10_NC as u64,
    };
    *flags = (*flags & !AMDGPU_PTE_MTYPE_VG10_MASK) | mtype;
    if vm_flags & AMDGPU_VM_PAGE_PRT != 0 {
        *flags |= AMDGPU_PTE_PRT;
        *flags &= !(AMDGPU_PTE_VALID as u64);
    }
    if let Some(bo) = bo {
        if *flags & AMDGPU_PTE_VALID as u64 != 0 {
            gmc_v9_0_get_coherence_flags(adev, bo, flags);
        }
    }
}

/// `gmc_v9_0_emit_flush_gpu_tlb`: point the VMID at `pd_addr` and
/// invalidate it, from the ring.
pub fn gmc_v9_0_emit_flush_gpu_tlb(adev: &mut amdgpu_device, r: usize, vmid: u32, pd_addr: u64) -> u64 {
    use crate::amdgpu_ring as ring;
    let vm_hub = adev.rings[r].vm_hub;
    let use_semaphore = gmc_v9_0_use_invalidate_semaphore(adev, vm_hub);
    let hub = adev.vmhub[vm_hub as usize];
    let req = gmc_v9_0_get_invalidate_req(vmid, 0);
    let eng = adev.rings[r].vm_inv_eng;
    // It may lose gpuvm invalidate acknowldege state across power-gating
    // off cycle, add semaphore acquire before invalidation and semaphore
    // release after invalidation to avoid entering power gated state to
    // WA the Issue
    if use_semaphore {
        // a read return value of 1 means semaphore acuqire
        ring::amdgpu_ring_emit_reg_wait(adev, r, hub.vm_inv_eng0_sem + hub.eng_distance * eng, 0x1, 0x1);
    }
    ring::amdgpu_ring_emit_wreg(adev, r, hub.ctx0_ptb_addr_lo32 + hub.ctx_addr_distance * vmid, pd_addr as u32);
    ring::amdgpu_ring_emit_wreg(adev, r, hub.ctx0_ptb_addr_hi32 + hub.ctx_addr_distance * vmid, (pd_addr >> 32) as u32);
    ring::amdgpu_ring_emit_reg_write_reg_wait(adev, r, hub.vm_inv_eng0_req + hub.eng_distance * eng,
                                              hub.vm_inv_eng0_ack + hub.eng_distance * eng, req, 1 << vmid);
    // add semaphore release after invalidation, write with 0 means
    // semaphore release
    if use_semaphore {
        ring::amdgpu_ring_emit_wreg(adev, r, hub.vm_inv_eng0_sem + hub.eng_distance * eng, 0);
    }
    pd_addr
}

/// `gmc_v9_0_emit_pasid_mapping`: the IH's VMID-to-PASID table.
pub fn gmc_v9_0_emit_pasid_mapping(adev: &mut amdgpu_device, r: usize, vmid: u32, pasid: u32) {
    let vm_hub = adev.rings[r].vm_hub;
    // MMHUB1 is not in the IH's table
    if vm_hub == AMDGPU_MMHUB1_0_ {
        return;
    }
    let oss = |idx: u32, reg: u32| adev.reg_offset[OSSSYS_HWIP as usize][0].get(idx as usize).copied().unwrap_or(0) + reg;
    let reg = if vm_hub == AMDGPU_GFXHUB_0 {
        oss(OSS_mmIH_VMID_0_LUT_BASE_IDX, OSS_mmIH_VMID_0_LUT) + vmid
    } else {
        oss(OSS_mmIH_VMID_0_LUT_MM_BASE_IDX, OSS_mmIH_VMID_0_LUT_MM) + vmid
    };
    crate::amdgpu_ring::amdgpu_ring_emit_wreg(adev, r, reg, pasid);
}

/// `gmc_v9_0_flush_gpu_tlb`, the MMIO path (before KIQ/MES/GFXOFF). The
/// hub's registers are absolute (`*_IP_NO_KIQ` with one GC instance).
pub fn gmc_v9_0_flush_gpu_tlb(adev: &mut amdgpu_device, vmid: u32, vmhub: u32, flush_type: u32) {
    let use_semaphore = gmc_v9_0_use_invalidate_semaphore(adev, vmhub);
    const ENG: u32 = 17;
    let hub = adev.vmhub[vmhub as usize];
    let inv_req = gmc_v9_0_get_invalidate_req(vmid, flush_type);
    let sem = hub.vm_inv_eng0_sem + hub.eng_distance * ENG;
    let req = hub.vm_inv_eng0_req + hub.eng_distance * ENG;
    let ack = hub.vm_inv_eng0_ack + hub.eng_distance * ENG;
    // It may lose gpuvm invalidate acknowldege state across power-gating
    // off cycle, add semaphore acquire before invalidation and semaphore
    // release after invalidation to avoid entering power gated state to
    // WA the Issue
    if use_semaphore {
        let mut j = 0;
        while j < adev.usec_timeout {
            // a read return value of 1 means semaphore acquire
            if adev.rreg32(sem).unwrap_or(0) & 1 != 0 {
                break;
            }
            crate::linux::udelay(1);
            j += 1;
        }
        if j >= adev.usec_timeout {
            crate::linux::Line::new().s("Timeout waiting for sem acquire in VM flush!").say();
        }
    }
    adev.wreg32(req, inv_req);
    // Issue a dummy read to wait for the ACK register to be cleared to
    // avoid a false ACK due to the new fast GRBM interface.
    if vmhub == AMDGPU_GFXHUB_0 && adev.ip_version(GC_HWIP, 0) < 9 << 24 | 4 << 16 | 2 << 8 {
        adev.rreg32(req);
    }
    let mut j = 0;
    while j < adev.usec_timeout {
        if adev.rreg32(ack).unwrap_or(0) & (1 << vmid) != 0 {
            break;
        }
        crate::linux::udelay(1);
        j += 1;
    }
    if use_semaphore {
        // add semaphore release after invalidation, write with 0 means
        // semaphore release
        adev.wreg32(sem, 0);
    }
    if j < adev.usec_timeout {
        return;
    }
    crate::linux::Line::new().s("Timeout waiting for VM flush ACK!").say();
}

/// `gmc_v9_0_gart_init`: not pdb0; the table lives in VRAM.
fn gmc_v9_0_gart_init(adev: &mut amdgpu_device) -> i32 {
    if adev.gart.bo.is_some() {
        crate::linux::Line::new().s("VEGA10 PCIE GART already initialized").say();
        return 0;
    }
    adev.gmc.vmid0_page_table_depth = 0;
    adev.gmc.vmid0_page_table_block_size = 0;
    // Initialize common gart structure
    let r = crate::amdgpu_gart::amdgpu_gart_init(adev);
    if r != 0 {
        return r;
    }
    adev.gart.table_size = adev.gart.num_gpu_pages as u64 * 8;
    adev.gart.gart_pte_flags = AMDGPU_GART_PTE_FLAGS_UC;
    crate::amdgpu_gart::amdgpu_gart_table_vram_alloc(adev)
}

/// `amdgpu_vm_manager_init` (amdgpu_vm.c) with `amdgpu_vmid_mgr_init`.
fn amdgpu_vm_manager_init(adev: &mut amdgpu_device) {
    // Concurrent flushes are only possible starting with Vega10 and are
    // broken on Navi10 and Navi14.
    adev.vm_manager.concurrent_flush =
        !(adev.asic_type < CHIP_VEGA10 || adev.asic_type == CHIP_NAVI10 || adev.asic_type == CHIP_NAVI14);
    crate::amdgpu_ids::amdgpu_vmid_mgr_init(adev);
    adev.vm_manager.fence_context = crate::dma_fence::dma_fence_context_alloc(AMDGPU_MAX_RINGS);
    adev.vm_manager.seqno = alloc::vec![0; AMDGPU_MAX_RINGS as usize];
    // CONFIG_X86_64: only in large BAR systems Compute VM tables will be
    // updated by CPU.
    if crate::tables::amdgpu_vm_update_mode[0] == -1 {
        adev.vm_manager.vm_update_mode =
            if adev.gmc.visible_vram_size >= adev.gmc.real_vram_size { AMDGPU_VM_USE_CPU_FOR_COMPUTE } else { 0 };
    } else {
        adev.vm_manager.vm_update_mode = crate::tables::amdgpu_vm_update_mode[0] as u32;
    }
}

/// `amdgpu_gmc_ras_sw_init` (amdgpu_gmc.c): Renoir has HDP and XGMI RAS
/// funcs (gmc_v9_0_early_init), no UMC, MMHUB or MCA ones.
fn amdgpu_gmc_ras_sw_init(adev: &mut amdgpu_device) -> i32 {
    if adev.hdp_ras.is_some() {
        adev.ras_list.push("hdp");
    }
    if adev.gmc.xgmi.ras.is_some() {
        adev.ras_list.push("xgmi_wafl");
    }
    0
}

/// `gmc_v9_0_sw_init`.
pub fn gmc_v9_0_sw_init(adev: &mut amdgpu_device) -> i32 {
    let r = gmc_v9_0_sw_init_layout(adev);
    if r != 0 {
        return r;
    }
    // Memory manager
    let r = crate::amdgpu_object::amdgpu_bo_init(adev);
    if r != 0 {
        return r;
    }
    let r = gmc_v9_0_gart_init(adev);
    if r != 0 {
        return r;
    }
    // gmc_v9_0_init_nps_details: an APU has no NPS modes.
    // number of VMs: VMID 0 is reserved for System, amdgpu graphics/compute
    // will use VMIDs 1..n-1, amdkfd will use VMIDs n..15. The first KFD VMID
    // is 8 for GPUs with graphics.
    adev.vm_manager.first_kfd_vmid = 8;
    amdgpu_vm_manager_init(adev);
    // gmc_v9_0_save_registers: DCE 1.0.x only.
    amdgpu_gmc_ras_sw_init(adev)
}

/// `gmc_v9_0_gart_enable`: not pdb0, not in s0ix.
fn gmc_v9_0_gart_enable(adev: &mut amdgpu_device) -> i32 {
    if adev.gart.bo.is_none() {
        crate::linux::Line::new().s("No VRAM object for PCIE GART.").say();
        return -22;
    }
    crate::amdgpu_ttm::amdgpu_gtt_mgr_recover(adev);
    let r = crate::gfxhub_v1_0::gfxhub_v1_0_gart_enable(adev);
    if r != 0 {
        return r;
    }
    let r = crate::mmhub_v1_0::mmhub_v1_0_gart_enable(adev);
    if r != 0 {
        return r;
    }
    crate::linux::Line::new().s("PCIE GART of ").dec(adev.gmc.gart_size >> 20).s("M enabled.").say();
    let ptb = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, adev.gart.bo.unwrap());
    crate::linux::Line::new().s("PTB located at ").hex64(ptb).say();
    0
}

/// `WREG32_FIELD15(DCE, 0, reg, field, val)`.
fn dce_field15(adev: &amdgpu_device, idx: u32, reg: u32, mask: u32, shift: u32, val: u32) {
    use crate::nbio_v7_0::{rreg32_soc15, wreg32_soc15};
    let v = rreg32_soc15(adev, DCE_HWIP, 0, idx, reg).unwrap_or(0);
    wreg32_soc15(adev, DCE_HWIP, 0, idx, reg, (v & !mask) | val << shift);
}

/// `gmc_v9_0_hw_init`: no golden registers for MMHUB 1.5.0, no UMC
/// funcs, no XGMI, not in s0ix; an APU flushes no HDP.
pub fn gmc_v9_0_hw_init(adev: &mut amdgpu_device) -> i32 {
    adev.gmc.flush_pasid_uses_kiq = true;
    // Vega20+XGMI caches PTEs in TC and TLB: not GC 9.3.
    adev.gmc.flush_tlb_needs_extra_type_2 = false;
    // The sequence of these two function calls matters.
    // gmc_v9_0_init_golden_registers: MMHUB 1.5.0 is the default case.
    if adev.mode_info.num_crtc != 0 {
        // Lockout access through VGA aperture
        dce_field15(adev, DCE_mmVGA_HDP_CONTROL_BASE_IDX, DCE_mmVGA_HDP_CONTROL,
                    DCE_VGA_HDP_CONTROL__VGA_MEMORY_DISABLE_MASK, DCE_VGA_HDP_CONTROL__VGA_MEMORY_DISABLE__SHIFT, 1);
        // disable VGA render
        dce_field15(adev, DCE_mmVGA_RENDER_CONTROL_BASE_IDX, DCE_mmVGA_RENDER_CONTROL,
                    DCE_VGA_RENDER_CONTROL__VGA_VSTATUS_CNTL_MASK, DCE_VGA_RENDER_CONTROL__VGA_VSTATUS_CNTL__SHIFT, 0);
    }
    crate::mmhub_v1_0::mmhub_v1_0_update_power_gating(adev, true);
    crate::hdp_v4_0::hdp_v4_0_init_registers(adev);
    // After HDP is initialized, flush HDP: an APU has none to flush.
    let value = crate::tables::amdgpu_vm_fault_stop[0] != AMDGPU_VM_FAULT_STOP_ALWAYS as i32;
    crate::gfxhub_v1_0::gfxhub_v1_0_set_fault_enable_default(adev, value);
    crate::mmhub_v1_0::mmhub_v1_0_set_fault_enable_default(adev, value);
    for i in 0..AMDGPU_MAX_VMHUBS {
        if adev.vmhubs_mask & (1 << i) != 0 {
            gmc_v9_0_flush_gpu_tlb(adev, 0, i, 0);
        }
    }
    let r = gmc_v9_0_gart_enable(adev);
    if r != 0 {
        return r;
    }
    if crate::tables::amdgpu_emu_mode[0] == 1 {
        crate::linux::Line::new().s("amdgpu_gmc_vram_checking: emulation only, not ported").say();
        return -22;
    }
    0
}

// ---- interrupts (stage 4) ----

/// `struct amdgpu_gmc_fault`: `timestamp` and `timestamp_expiry` are 48-bit
/// fields, `next` is AMDGPU_GMC_FAULT_RING_ORDER bits.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_gmc_fault {
    pub timestamp: u64,
    pub next: u64,
    pub key: u64,
    pub timestamp_expiry: u64,
}

const TS48: u64 = (1 << 48) - 1;
const RING_MASK: u64 = (1 << AMDGPU_GMC_FAULT_RING_ORDER) - 1;

/// `amdgpu_ih_ts_after(t1, t2)`.
fn ts_after(t1: u64, t2: u64) -> bool {
    ((t2 << 16) as i64).wrapping_sub((t1 << 16) as i64) > 0
}

/// `amdgpu_gmc_filter_faults`.
pub fn amdgpu_gmc_filter_faults(adev: &mut amdgpu_device, processed_timestamp: u64, addr: u64, pasid: u16,
                                timestamp: u64) -> bool {
    let gmc = &mut adev.gmc;
    if gmc.fault_ring.is_empty() {
        gmc.fault_ring = alloc::vec![amdgpu_gmc_fault::default(); AMDGPU_GMC_FAULT_RING_SIZE as usize];
        gmc.fault_hash = alloc::vec![0; AMDGPU_GMC_FAULT_HASH_SIZE as usize];
    }
    // amdgpu_gmc_fault_key
    let key = addr << 4 | pasid as u64;

    // Stale retry fault if timestamp goes backward
    if ts_after(timestamp, processed_timestamp) {
        return true;
    }

    // If we don't have space left in the ring buffer return immediately
    let stamp = timestamp.max(AMDGPU_GMC_FAULT_TIMEOUT as u64 + 1) - AMDGPU_GMC_FAULT_TIMEOUT as u64;
    if gmc.fault_ring[gmc.last_fault as usize].timestamp >= stamp {
        return true;
    }

    // Try to find the fault in the hash: hash_64
    let hash = (key.wrapping_mul(GOLDEN_RATIO_64) >> (64 - AMDGPU_GMC_FAULT_HASH_ORDER)) as usize;
    let mut fault = gmc.fault_hash[hash] as usize;
    while gmc.fault_ring[fault].timestamp >= stamp {
        let f = gmc.fault_ring[fault];
        if f.key == key {
            // if we get a fault which is already present in the fault_ring
            // and the timestamp of the fault is after the expired
            // timestamp, then this is a new fault that needs to be added
            // into the fault ring.
            if f.timestamp_expiry != 0 && ts_after(f.timestamp_expiry, timestamp) {
                break;
            } else {
                return true;
            }
        }
        let tmp = f.timestamp;
        fault = f.next as usize;
        // Check if the entry was reused
        if gmc.fault_ring[fault].timestamp >= tmp {
            break;
        }
    }

    // Add the fault to the ring
    let last = gmc.last_fault as usize;
    gmc.fault_ring[last].key = key;
    gmc.fault_ring[last].timestamp = timestamp & TS48;
    // And update the hash
    gmc.fault_ring[last].next = gmc.fault_hash[hash] & RING_MASK;
    gmc.fault_hash[hash] = gmc.last_fault;
    gmc.last_fault = (gmc.last_fault + 1) & RING_MASK;
    false
}

/// `gmc_v9_0_vm_fault_interrupt_state`.
pub fn gmc_v9_0_vm_fault_interrupt_state(adev: &mut amdgpu_device, _type: u32, state: u32) -> i32 {
    let bits = GMC_VM_CONTEXT1_CNTL__RANGE_PROTECTION_FAULT_ENABLE_INTERRUPT_MASK
        | GMC_VM_CONTEXT1_CNTL__DUMMY_PAGE_PROTECTION_FAULT_ENABLE_INTERRUPT_MASK
        | GMC_VM_CONTEXT1_CNTL__PDE0_PROTECTION_FAULT_ENABLE_INTERRUPT_MASK
        | GMC_VM_CONTEXT1_CNTL__VALID_PROTECTION_FAULT_ENABLE_INTERRUPT_MASK
        | GMC_VM_CONTEXT1_CNTL__READ_PROTECTION_FAULT_ENABLE_INTERRUPT_MASK
        | GMC_VM_CONTEXT1_CNTL__WRITE_PROTECTION_FAULT_ENABLE_INTERRUPT_MASK
        | GMC_VM_CONTEXT1_CNTL__EXECUTE_PROTECTION_FAULT_ENABLE_INTERRUPT_MASK;
    if state != AMDGPU_IRQ_STATE_DISABLE && state != AMDGPU_IRQ_STATE_ENABLE {
        return 0;
    }
    for j in 0..AMDGPU_MAX_VMHUBS {
        if adev.vmhubs_mask & (1 << j) == 0 {
            continue;
        }
        let hub = adev.vmhub[j as usize];
        for i in 0..16 {
            let reg = hub.vm_context0_cntl + i;
            // This works because this interrupt is only enabled at
            // init/resume and disabled in fini/suspend, so the overall
            // state doesn't change over the course of suspend/resume.
            if adev.in_s0ix && j == AMDGPU_GFXHUB_0 {
                continue;
            }
            // RREG32_SOC15_IP(MMHUB, reg) / RREG32_XCC(reg, j): the plain
            // register on bare metal.
            let mut tmp = adev.rreg32(reg).unwrap_or(0);
            if state == AMDGPU_IRQ_STATE_DISABLE {
                tmp &= !bits;
            } else {
                tmp |= bits;
            }
            adev.wreg32(reg, tmp);
        }
    }
    0
}

/// `gmc_v9_0_ecc_interrupt_state`: Renoir is newer than Vega20, the PSP
/// bootloader programs it.
pub fn gmc_v9_0_ecc_interrupt_state(adev: &mut amdgpu_device, _type: u32, _state: u32) -> i32 {
    if adev.asic_type >= CHIP_VEGA20 {
        return 0;
    }
    Line::new().s("gmc_v9_0_ecc_interrupt_state: pre-Vega20 path is not ported").say();
    0
}

/// `REG_GET_FIELD`.
fn get_field(v: u32, mask: u32, shift: u32) -> u32 {
    (v & mask) >> shift
}

/// `gmc_v9_0_process_interrupt`: no KFD.
pub fn gmc_v9_0_process_interrupt(adev: &mut amdgpu_device, entry: &crate::amdgpu_irq::amdgpu_iv_entry,
                                  ih_soft: bool) -> i32 {
    let retry_fault = entry.src_data[1] & 0x80 != 0;
    let node_id = entry.node_id;
    let mut addr = (entry.src_data[0] as u64) << 12;
    addr |= ((entry.src_data[1] as u64) & 0xf) << 44;

    let (hub_name, vmhub) = if entry.client_id == SOC15_IH_CLIENTID_VMC {
        ("mmhub0", AMDGPU_MMHUB0_0_ + node_id / 4)
    } else if entry.client_id == SOC15_IH_CLIENTID_VMC1 {
        ("mmhub1", AMDGPU_MMHUB1_0_)
    } else {
        // gfx_v9_0 has no ih_node_to_logical_xcc
        ("gfxhub0", 0)
    };
    let hub = adev.vmhub[vmhub as usize];

    if retry_fault {
        // retry_cam_enabled is off on GC 9.3.0.
        // Process it onyl if it's the first fault for this address
        let processed = if ih_soft { adev.irq.ih_soft.processed_timestamp } else { adev.irq.ih.processed_timestamp };
        if !ih_soft && amdgpu_gmc_filter_faults(adev, processed, addr, entry.pasid as u16, entry.timestamp) {
            return 1;
        }
        // Delegate it to a different ring if the hardware hasn't already
        // done it.
        if !ih_soft {
            crate::amdgpu_irq::amdgpu_irq_delegate(adev, entry, 8);
            return 1;
        }
        // Try to handle the recoverable page faults by filling page tables
        let write_fault = entry.src_data[1] & 0x20 != 0;
        if crate::amdgpu_vm::amdgpu_vm_handle_fault(adev, entry.pasid as u32, entry.vmid as u32, node_id, addr,
                                                    entry.timestamp, write_fault) {
            return 1;
        }
    }
    if !crate::linux::printk_ratelimit() {
        return 0;
    }

    Line::new().s("[").s(hub_name).s("] ").s(if retry_fault { "retry" } else { "no-retry" })
        .s(" page fault (src_id:").dec(entry.src_id as u64).s(" ring:").dec(entry.ring_id as u64).s(" vmid:")
        .dec(entry.vmid as u64).s(" pasid:").dec(entry.pasid as u64).s(")").say();
    let client = crate::tables::soc15_ih_clientid_name.get(entry.client_id as usize).copied().unwrap_or(b"");
    Line::new().s("  in page starting at address ").hex64(addr).s(" from IH client 0x").hexu(entry.client_id)
        .s(" (").bytes(client).s(")").say();

    // Issue a dummy read to wait for the status register to be updated to
    // avoid reading an incorrect value due to the new fast GRBM interface.
    if entry.vmid_src == AMDGPU_GFXHUB_0 && adev.ip_version(GC_HWIP, 0) < 9 << 24 | 4 << 16 | 2 << 8 {
        adev.rreg32(hub.vm_l2_pro_fault_status);
    }
    let status = adev.rreg32(hub.vm_l2_pro_fault_status).unwrap_or(0);
    let cid = get_field(status, GMC_VM_L2_PROTECTION_FAULT_STATUS__CID_MASK, GMC_VM_L2_PROTECTION_FAULT_STATUS__CID__SHIFT);
    let rw = get_field(status, GMC_VM_L2_PROTECTION_FAULT_STATUS__RW_MASK, GMC_VM_L2_PROTECTION_FAULT_STATUS__RW__SHIFT);

    // Only print L2 fault status if the status register could be read and
    // contains useful information
    if status == 0 {
        return 0;
    }
    // WREG32_P(hub->vm_l2_pro_fault_cntl, 1, ~1)
    let c = adev.rreg32(hub.vm_l2_pro_fault_cntl).unwrap_or(0);
    adev.wreg32(hub.vm_l2_pro_fault_cntl, (c & !1) | 1);

    Line::new().s("VM_L2_PROTECTION_FAULT_STATUS:0x").hexw(status, 8).say();
    if entry.vmid_src == AMDGPU_GFXHUB_0 {
        let name = crate::tables::gfxhub_client_ids.get(cid as usize).copied().unwrap_or(b"unknown");
        Line::new().s("\t Faulty UTCL2 client ID: ").bytes(name).s(" (0x").hexu(cid).s(")").say();
    } else {
        // MMHUB 1.5.0: mmhub_client_ids_renoir, [cid][rw]
        let v = adev.ip_version(MMHUB_HWIP, 0);
        let name = if v == 1 << 24 | 5 << 16 || v == 2 << 24 | 4 << 16 {
            crate::tables::mmhub_client_ids_renoir.get((cid * 2 + rw) as usize).copied().filter(|s| !s.is_empty())
        } else {
            None
        };
        Line::new().s("\t Faulty UTCL2 client ID: ").bytes(name.unwrap_or(b"unknown")).s(" (0x").hexu(cid).s(")").say();
    }
    for (what, mask, shift) in [
        ("\t MORE_FAULTS: 0x", GMC_VM_L2_PROTECTION_FAULT_STATUS__MORE_FAULTS_MASK,
         GMC_VM_L2_PROTECTION_FAULT_STATUS__MORE_FAULTS__SHIFT),
        ("\t WALKER_ERROR: 0x", GMC_VM_L2_PROTECTION_FAULT_STATUS__WALKER_ERROR_MASK,
         GMC_VM_L2_PROTECTION_FAULT_STATUS__WALKER_ERROR__SHIFT),
        ("\t PERMISSION_FAULTS: 0x", GMC_VM_L2_PROTECTION_FAULT_STATUS__PERMISSION_FAULTS_MASK,
         GMC_VM_L2_PROTECTION_FAULT_STATUS__PERMISSION_FAULTS__SHIFT),
        ("\t MAPPING_ERROR: 0x", GMC_VM_L2_PROTECTION_FAULT_STATUS__MAPPING_ERROR_MASK,
         GMC_VM_L2_PROTECTION_FAULT_STATUS__MAPPING_ERROR__SHIFT),
    ] {
        Line::new().s(what).hexu(get_field(status, mask, shift)).say();
    }
    Line::new().s("\t RW: 0x").hexu(rw).say();
    0
}

/// `gmc_v9_0_set_clockgating_state`.
pub fn gmc_v9_0_set_clockgating_state(adev: &amdgpu_device, gate: bool) -> i32 {
    crate::mmhub_v1_0::mmhub_v1_0_set_clockgating(adev, gate);
    crate::athub_v1_0::athub_v1_0_set_clockgating(adev, gate);
    0
}

/// `gmc_v9_0_late_init`: UMC 7.6.0, no RAS.
pub fn gmc_v9_0_late_init(adev: &mut amdgpu_device) -> i32 {
    let r = crate::amdgpu_gmc::amdgpu_gmc_allocate_vm_inv_eng(adev);
    if r != 0 {
        return r;
    }
    // amdgpu_ras_reset_error_count for MMHUB and HDP: no RAS block object,
    // -EOPNOTSUPP; amdgpu_gmc_ras_late_init returns 0.
    crate::amdgpu_irq::amdgpu_irq_get(adev, "gmc.vm_fault", 0)
}

/// `gmc_v9_0_hw_fini`: bare metal, default init level, no UMC RAS.
pub fn gmc_v9_0_hw_fini(adev: &mut amdgpu_device) -> i32 {
    // gmc_v9_0_gart_disable
    if !adev.in_s0ix {
        crate::gfxhub_v1_0::gfxhub_v1_0_gart_disable(adev);
    }
    crate::mmhub_v1_0::mmhub_v1_0_gart_disable(adev);
    // Pair the operations did in gmc_v9_0_hw_init and thus maintain a
    // correct cached state for GMC.
    crate::mmhub_v1_0::mmhub_v1_0_update_power_gating(adev, false);
    crate::amdgpu_irq::amdgpu_irq_put(adev, "gmc.vm_fault", 0);
    0
}
