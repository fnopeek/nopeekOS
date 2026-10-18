//! amdgpu/gmc_v9_0.c (Linux 6.18.55): the GMC block's early_init.

use crate::amdgpu_device::amdgpu_device;
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
