//! amdgpu/mmhub_v1_0.c (Linux 6.18.55): the multimedia VM hub, MMHUB 1.x.

use crate::amdgpu_device::amdgpu_device;
use crate::nbio_v7_0::rreg32_soc15;
use crate::regs::*;

/// `SOC15_REG_OFFSET(MMHUB, 0, reg)`.
fn mm_offset(adev: &amdgpu_device, base_idx: u32, reg: u32) -> u32 {
    adev.reg_offset[MMHUB_HWIP as usize][0].get(base_idx as usize).copied().unwrap_or(0) + reg
}

/// `mmhub_v1_0_get_fb_location`.
pub fn mmhub_v1_0_get_fb_location(adev: &mut amdgpu_device) -> u64 {
    let mut base = rreg32_soc15(adev, MMHUB_HWIP, 0, MM_mmMC_VM_FB_LOCATION_BASE_BASE_IDX, MM_mmMC_VM_FB_LOCATION_BASE)
        .unwrap_or(0) as u64;
    let mut top = rreg32_soc15(adev, MMHUB_HWIP, 0, MM_mmMC_VM_FB_LOCATION_TOP_BASE_IDX, MM_mmMC_VM_FB_LOCATION_TOP)
        .unwrap_or(0) as u64;
    base &= MM_MC_VM_FB_LOCATION_BASE__FB_BASE_MASK as u64;
    base <<= 24;
    top &= MM_MC_VM_FB_LOCATION_TOP__FB_TOP_MASK as u64;
    top <<= 24;
    adev.gmc.fb_start = base;
    adev.gmc.fb_end = top;
    base
}

/// `mmhub_v1_0_init`.
pub fn mmhub_v1_0_init(adev: &mut amdgpu_device) {
    let o = |i, r| mm_offset(adev, i, r);
    let hub = crate::amdgpu_device::amdgpu_vmhub {
        ctx0_ptb_addr_lo32: o(MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32),
        ctx0_ptb_addr_hi32: o(MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32),
        vm_inv_eng0_sem: o(MM_mmVM_INVALIDATE_ENG0_SEM_BASE_IDX, MM_mmVM_INVALIDATE_ENG0_SEM),
        vm_inv_eng0_req: o(MM_mmVM_INVALIDATE_ENG0_REQ_BASE_IDX, MM_mmVM_INVALIDATE_ENG0_REQ),
        vm_inv_eng0_ack: o(MM_mmVM_INVALIDATE_ENG0_ACK_BASE_IDX, MM_mmVM_INVALIDATE_ENG0_ACK),
        vm_context0_cntl: o(MM_mmVM_CONTEXT0_CNTL_BASE_IDX, MM_mmVM_CONTEXT0_CNTL),
        vm_l2_pro_fault_status: o(MM_mmVM_L2_PROTECTION_FAULT_STATUS_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_STATUS),
        vm_l2_pro_fault_cntl: o(MM_mmVM_L2_PROTECTION_FAULT_CNTL_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_CNTL),
        ctx_distance: MM_CTX_DISTANCE,
        ctx_addr_distance: MM_CTX_ADDR_DISTANCE,
        eng_distance: MM_ENG_DISTANCE,
        eng_addr_distance: MM_ENG_ADDR_DISTANCE,
    };
    adev.vmhub[AMDGPU_MMHUB0_0 as usize] = hub;
}

/// `RREG32_SOC15(MMHUB, 0, reg)` / `RREG32_SOC15_OFFSET`.
fn rd(adev: &amdgpu_device, idx: u32, reg: u32, off: u32) -> u32 {
    rreg32_soc15(adev, MMHUB_HWIP, 0, idx, reg + off).unwrap_or(0)
}

/// `WREG32_SOC15(MMHUB, 0, reg, v)` / `_OFFSET`.
fn wr(adev: &amdgpu_device, idx: u32, reg: u32, off: u32, v: u32) {
    crate::nbio_v7_0::wreg32_soc15(adev, MMHUB_HWIP, 0, idx, reg + off, v);
}

/// `REG_SET_FIELD`.
fn set(v: u32, mask: u32, shift: u32, f: u32) -> u32 {
    (v & !mask) | (mask & (f << shift))
}

/// `mmhub_v1_0_setup_vm_pt_regs`.
fn mmhub_v1_0_setup_vm_pt_regs(adev: &amdgpu_device, vmid: u32, page_table_base: u64) {
    let hub = adev.vmhub[AMDGPU_MMHUB0_0 as usize];
    wr(adev, MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32,
       hub.ctx_addr_distance * vmid, page_table_base as u32);
    wr(adev, MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32,
       hub.ctx_addr_distance * vmid, (page_table_base >> 32) as u32);
}

/// `mmhub_v1_0_init_gart_aperture_regs`: no PDB0.
fn mmhub_v1_0_init_gart_aperture_regs(adev: &amdgpu_device) {
    let pt_base = crate::amdgpu_gmc::amdgpu_gmc_pd_addr(adev, adev.gart.bo.unwrap());
    mmhub_v1_0_setup_vm_pt_regs(adev, 0, pt_base);
    let g = &adev.gmc;
    wr(adev, MM_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_LO32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_LO32, 0,
       (g.gart_start >> 12) as u32);
    wr(adev, MM_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_HI32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_HI32, 0,
       (g.gart_start >> 44) as u32);
    wr(adev, MM_mmVM_CONTEXT0_PAGE_TABLE_END_ADDR_LO32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_END_ADDR_LO32, 0,
       (g.gart_end >> 12) as u32);
    wr(adev, MM_mmVM_CONTEXT0_PAGE_TABLE_END_ADDR_HI32_BASE_IDX, MM_mmVM_CONTEXT0_PAGE_TABLE_END_ADDR_HI32, 0,
       (g.gart_end >> 44) as u32);
}

/// `mmhub_v1_0_init_system_aperture_regs`: bare metal, no PDB0.
fn mmhub_v1_0_init_system_aperture_regs(adev: &amdgpu_device) {
    let g = &adev.gmc;
    // Program the AGP BAR
    wr(adev, MM_mmMC_VM_AGP_BASE_BASE_IDX, MM_mmMC_VM_AGP_BASE, 0, 0);
    wr(adev, MM_mmMC_VM_AGP_BOT_BASE_IDX, MM_mmMC_VM_AGP_BOT, 0, (g.agp_start >> 24) as u32);
    wr(adev, MM_mmMC_VM_AGP_TOP_BASE_IDX, MM_mmMC_VM_AGP_TOP, 0, (g.agp_end >> 24) as u32);
    // Program the system aperture low logical page number.
    wr(adev, MM_mmMC_VM_SYSTEM_APERTURE_LOW_ADDR_BASE_IDX, MM_mmMC_VM_SYSTEM_APERTURE_LOW_ADDR, 0,
       (g.fb_start.min(g.agp_start) >> 18) as u32);
    if adev.apu_flags & (AMD_APU_IS_RAVEN2 | AMD_APU_IS_RENOIR | AMD_APU_IS_GREEN_SARDINE) != 0 {
        // Raven2 has a HW issue that it is unable to use the vram which is
        // out of MC_VM_SYSTEM_APERTURE_HIGH_ADDR. So here is the workaround
        // that increase system aperture high address (add 1) to get rid of
        // the VM fault and hardware hang.
        wr(adev, MM_mmMC_VM_SYSTEM_APERTURE_HIGH_ADDR_BASE_IDX, MM_mmMC_VM_SYSTEM_APERTURE_HIGH_ADDR, 0,
           ((g.fb_end >> 18) + 1).max(g.agp_end >> 18) as u32);
    } else {
        wr(adev, MM_mmMC_VM_SYSTEM_APERTURE_HIGH_ADDR_BASE_IDX, MM_mmMC_VM_SYSTEM_APERTURE_HIGH_ADDR, 0,
           (g.fb_end.max(g.agp_end) >> 18) as u32);
    }
    // Set default page address.
    let value = crate::amdgpu_gmc::amdgpu_gmc_vram_mc2pa(adev, adev.mem_scratch_gpu_addr);
    wr(adev, MM_mmMC_VM_SYSTEM_APERTURE_DEFAULT_ADDR_LSB_BASE_IDX, MM_mmMC_VM_SYSTEM_APERTURE_DEFAULT_ADDR_LSB, 0,
       (value >> 12) as u32);
    wr(adev, MM_mmMC_VM_SYSTEM_APERTURE_DEFAULT_ADDR_MSB_BASE_IDX, MM_mmMC_VM_SYSTEM_APERTURE_DEFAULT_ADDR_MSB, 0,
       (value >> 44) as u32);
    // Program "protection fault".
    wr(adev, MM_mmVM_L2_PROTECTION_FAULT_DEFAULT_ADDR_LO32_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_DEFAULT_ADDR_LO32, 0,
       (adev.dummy_page_addr >> 12) as u32);
    wr(adev, MM_mmVM_L2_PROTECTION_FAULT_DEFAULT_ADDR_HI32_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_DEFAULT_ADDR_HI32, 0,
       (adev.dummy_page_addr >> 44) as u32);
    let mut tmp = rd(adev, MM_mmVM_L2_PROTECTION_FAULT_CNTL2_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_CNTL2, 0);
    tmp = set(tmp, MM_VM_L2_PROTECTION_FAULT_CNTL2__ACTIVE_PAGE_MIGRATION_PTE_READ_RETRY_MASK,
              MM_VM_L2_PROTECTION_FAULT_CNTL2__ACTIVE_PAGE_MIGRATION_PTE_READ_RETRY__SHIFT, 1);
    wr(adev, MM_mmVM_L2_PROTECTION_FAULT_CNTL2_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_CNTL2, 0, tmp);
}

/// `mmhub_v1_0_init_tlb_regs`.
fn mmhub_v1_0_init_tlb_regs(adev: &amdgpu_device) {
    // Setup TLB control
    let mut tmp = rd(adev, MM_mmMC_VM_MX_L1_TLB_CNTL_BASE_IDX, MM_mmMC_VM_MX_L1_TLB_CNTL, 0);
    tmp = set(tmp, MM_MC_VM_MX_L1_TLB_CNTL__ENABLE_L1_TLB_MASK, MM_MC_VM_MX_L1_TLB_CNTL__ENABLE_L1_TLB__SHIFT, 1);
    tmp = set(tmp, MM_MC_VM_MX_L1_TLB_CNTL__SYSTEM_ACCESS_MODE_MASK, MM_MC_VM_MX_L1_TLB_CNTL__SYSTEM_ACCESS_MODE__SHIFT, 3);
    tmp = set(tmp, MM_MC_VM_MX_L1_TLB_CNTL__ENABLE_ADVANCED_DRIVER_MODEL_MASK,
              MM_MC_VM_MX_L1_TLB_CNTL__ENABLE_ADVANCED_DRIVER_MODEL__SHIFT, 1);
    tmp = set(tmp, MM_MC_VM_MX_L1_TLB_CNTL__SYSTEM_APERTURE_UNMAPPED_ACCESS_MASK,
              MM_MC_VM_MX_L1_TLB_CNTL__SYSTEM_APERTURE_UNMAPPED_ACCESS__SHIFT, 0);
    // XXX for emulation.
    tmp = set(tmp, MM_MC_VM_MX_L1_TLB_CNTL__MTYPE_MASK, MM_MC_VM_MX_L1_TLB_CNTL__MTYPE__SHIFT, MTYPE_UC);
    tmp = set(tmp, MM_MC_VM_MX_L1_TLB_CNTL__ATC_EN_MASK, MM_MC_VM_MX_L1_TLB_CNTL__ATC_EN__SHIFT, 1);
    wr(adev, MM_mmMC_VM_MX_L1_TLB_CNTL_BASE_IDX, MM_mmMC_VM_MX_L1_TLB_CNTL, 0, tmp);
}

/// `mmhub_v1_0_init_cache_regs`.
fn mmhub_v1_0_init_cache_regs(adev: &amdgpu_device) {
    // Setup L2 cache
    let mut tmp = rd(adev, MM_mmVM_L2_CNTL_BASE_IDX, MM_mmVM_L2_CNTL, 0);
    tmp = set(tmp, MM_VM_L2_CNTL__ENABLE_L2_CACHE_MASK, MM_VM_L2_CNTL__ENABLE_L2_CACHE__SHIFT, 1);
    tmp = set(tmp, MM_VM_L2_CNTL__ENABLE_L2_FRAGMENT_PROCESSING_MASK, MM_VM_L2_CNTL__ENABLE_L2_FRAGMENT_PROCESSING__SHIFT, 1);
    // XXX for emulation, Refer to closed source code.
    tmp = set(tmp, MM_VM_L2_CNTL__L2_PDE0_CACHE_TAG_GENERATION_MODE_MASK,
              MM_VM_L2_CNTL__L2_PDE0_CACHE_TAG_GENERATION_MODE__SHIFT, 0);
    tmp = set(tmp, MM_VM_L2_CNTL__PDE_FAULT_CLASSIFICATION_MASK, MM_VM_L2_CNTL__PDE_FAULT_CLASSIFICATION__SHIFT, 0);
    tmp = set(tmp, MM_VM_L2_CNTL__CONTEXT1_IDENTITY_ACCESS_MODE_MASK, MM_VM_L2_CNTL__CONTEXT1_IDENTITY_ACCESS_MODE__SHIFT, 1);
    tmp = set(tmp, MM_VM_L2_CNTL__IDENTITY_MODE_FRAGMENT_SIZE_MASK, MM_VM_L2_CNTL__IDENTITY_MODE_FRAGMENT_SIZE__SHIFT, 0);
    wr(adev, MM_mmVM_L2_CNTL_BASE_IDX, MM_mmVM_L2_CNTL, 0, tmp);

    let mut tmp = rd(adev, MM_mmVM_L2_CNTL2_BASE_IDX, MM_mmVM_L2_CNTL2, 0);
    tmp = set(tmp, MM_VM_L2_CNTL2__INVALIDATE_ALL_L1_TLBS_MASK, MM_VM_L2_CNTL2__INVALIDATE_ALL_L1_TLBS__SHIFT, 1);
    tmp = set(tmp, MM_VM_L2_CNTL2__INVALIDATE_L2_CACHE_MASK, MM_VM_L2_CNTL2__INVALIDATE_L2_CACHE__SHIFT, 1);
    wr(adev, MM_mmVM_L2_CNTL2_BASE_IDX, MM_mmVM_L2_CNTL2, 0, tmp);

    let mut tmp = MM_mmVM_L2_CNTL3_DEFAULT;
    if adev.gmc.translate_further {
        tmp = set(tmp, MM_VM_L2_CNTL3__BANK_SELECT_MASK, MM_VM_L2_CNTL3__BANK_SELECT__SHIFT, 12);
        tmp = set(tmp, MM_VM_L2_CNTL3__L2_CACHE_BIGK_FRAGMENT_SIZE_MASK, MM_VM_L2_CNTL3__L2_CACHE_BIGK_FRAGMENT_SIZE__SHIFT, 9);
    } else {
        tmp = set(tmp, MM_VM_L2_CNTL3__BANK_SELECT_MASK, MM_VM_L2_CNTL3__BANK_SELECT__SHIFT, 9);
        tmp = set(tmp, MM_VM_L2_CNTL3__L2_CACHE_BIGK_FRAGMENT_SIZE_MASK, MM_VM_L2_CNTL3__L2_CACHE_BIGK_FRAGMENT_SIZE__SHIFT, 6);
    }
    wr(adev, MM_mmVM_L2_CNTL3_BASE_IDX, MM_mmVM_L2_CNTL3, 0, tmp);

    let mut tmp = MM_mmVM_L2_CNTL4_DEFAULT;
    tmp = set(tmp, MM_VM_L2_CNTL4__VMC_TAP_PDE_REQUEST_PHYSICAL_MASK, MM_VM_L2_CNTL4__VMC_TAP_PDE_REQUEST_PHYSICAL__SHIFT, 0);
    tmp = set(tmp, MM_VM_L2_CNTL4__VMC_TAP_PTE_REQUEST_PHYSICAL_MASK, MM_VM_L2_CNTL4__VMC_TAP_PTE_REQUEST_PHYSICAL__SHIFT, 0);
    wr(adev, MM_mmVM_L2_CNTL4_BASE_IDX, MM_mmVM_L2_CNTL4, 0, tmp);
}

/// `mmhub_v1_0_enable_system_domain`.
fn mmhub_v1_0_enable_system_domain(adev: &amdgpu_device) {
    let mut tmp = rd(adev, MM_mmVM_CONTEXT0_CNTL_BASE_IDX, MM_mmVM_CONTEXT0_CNTL, 0);
    tmp = set(tmp, MM_VM_CONTEXT0_CNTL__ENABLE_CONTEXT_MASK, MM_VM_CONTEXT0_CNTL__ENABLE_CONTEXT__SHIFT, 1);
    tmp = set(tmp, MM_VM_CONTEXT0_CNTL__PAGE_TABLE_DEPTH_MASK, MM_VM_CONTEXT0_CNTL__PAGE_TABLE_DEPTH__SHIFT, 0);
    tmp = set(tmp, MM_VM_CONTEXT0_CNTL__RETRY_PERMISSION_OR_INVALID_PAGE_FAULT_MASK,
              MM_VM_CONTEXT0_CNTL__RETRY_PERMISSION_OR_INVALID_PAGE_FAULT__SHIFT, 0);
    wr(adev, MM_mmVM_CONTEXT0_CNTL_BASE_IDX, MM_mmVM_CONTEXT0_CNTL, 0, tmp);
}

/// `mmhub_v1_0_disable_identity_aperture`.
fn mmhub_v1_0_disable_identity_aperture(adev: &amdgpu_device) {
    wr(adev, MM_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_LOW_ADDR_LO32_BASE_IDX,
       MM_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_LOW_ADDR_LO32, 0, 0xFFFF_FFFF);
    wr(adev, MM_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_LOW_ADDR_HI32_BASE_IDX,
       MM_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_LOW_ADDR_HI32, 0, 0x0000_000F);
    wr(adev, MM_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_LO32_BASE_IDX,
       MM_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_LO32, 0, 0);
    wr(adev, MM_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_HI32_BASE_IDX,
       MM_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_HI32, 0, 0);
    wr(adev, MM_mmVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_LO32_BASE_IDX,
       MM_mmVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_LO32, 0, 0);
    wr(adev, MM_mmVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_HI32_BASE_IDX,
       MM_mmVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_HI32, 0, 0);
}

/// `mmhub_v1_0_setup_vmid_config`.
fn mmhub_v1_0_setup_vmid_config(adev: &amdgpu_device) {
    let hub = adev.vmhub[AMDGPU_MMHUB0_0 as usize];
    let mut num_level = adev.vm_manager.num_level;
    let mut block_size = adev.vm_manager.block_size;
    if adev.gmc.translate_further {
        num_level -= 1;
    } else {
        block_size -= 9;
    }
    for i in 0..=14u32 {
        let mut tmp = rd(adev, MM_mmVM_CONTEXT1_CNTL_BASE_IDX, MM_mmVM_CONTEXT1_CNTL, i * hub.ctx_distance);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__ENABLE_CONTEXT_MASK, MM_VM_CONTEXT1_CNTL__ENABLE_CONTEXT__SHIFT, 1);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__PAGE_TABLE_DEPTH_MASK, MM_VM_CONTEXT1_CNTL__PAGE_TABLE_DEPTH__SHIFT, num_level);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__RANGE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  MM_VM_CONTEXT1_CNTL__RANGE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__DUMMY_PAGE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  MM_VM_CONTEXT1_CNTL__DUMMY_PAGE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__PDE0_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  MM_VM_CONTEXT1_CNTL__PDE0_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__VALID_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  MM_VM_CONTEXT1_CNTL__VALID_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__READ_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  MM_VM_CONTEXT1_CNTL__READ_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__WRITE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  MM_VM_CONTEXT1_CNTL__WRITE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__EXECUTE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  MM_VM_CONTEXT1_CNTL__EXECUTE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__PAGE_TABLE_BLOCK_SIZE_MASK, MM_VM_CONTEXT1_CNTL__PAGE_TABLE_BLOCK_SIZE__SHIFT,
                  block_size);
        // Send no-retry XNACK on fault to suppress VM fault storm.
        tmp = set(tmp, MM_VM_CONTEXT1_CNTL__RETRY_PERMISSION_OR_INVALID_PAGE_FAULT_MASK,
                  MM_VM_CONTEXT1_CNTL__RETRY_PERMISSION_OR_INVALID_PAGE_FAULT__SHIFT,
                  (adev.gmc.noretry == 0) as u32);
        wr(adev, MM_mmVM_CONTEXT1_CNTL_BASE_IDX, MM_mmVM_CONTEXT1_CNTL, i * hub.ctx_distance, tmp);
        let d = i * hub.ctx_addr_distance;
        wr(adev, MM_mmVM_CONTEXT1_PAGE_TABLE_START_ADDR_LO32_BASE_IDX, MM_mmVM_CONTEXT1_PAGE_TABLE_START_ADDR_LO32, d, 0);
        wr(adev, MM_mmVM_CONTEXT1_PAGE_TABLE_START_ADDR_HI32_BASE_IDX, MM_mmVM_CONTEXT1_PAGE_TABLE_START_ADDR_HI32, d, 0);
        wr(adev, MM_mmVM_CONTEXT1_PAGE_TABLE_END_ADDR_LO32_BASE_IDX, MM_mmVM_CONTEXT1_PAGE_TABLE_END_ADDR_LO32, d,
           (adev.vm_manager.max_pfn - 1) as u32);
        wr(adev, MM_mmVM_CONTEXT1_PAGE_TABLE_END_ADDR_HI32_BASE_IDX, MM_mmVM_CONTEXT1_PAGE_TABLE_END_ADDR_HI32, d,
           ((adev.vm_manager.max_pfn - 1) >> 32) as u32);
    }
    if adev.ip_version(ISP_HWIP, 0) != 0 {
        mmhub_v1_0_init_saw(adev);
    }
}

/// `mmhub_v1_0_init_saw`.
fn mmhub_v1_0_init_saw(adev: &amdgpu_device) {
    let pt_base = crate::amdgpu_gmc::amdgpu_gmc_pd_addr(adev, adev.gart.bo.unwrap());
    let g = &adev.gmc;
    // VM_9_X_REGISTER_VM_L2_SAW_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32 / HI32
    wr(adev, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32_BASE_IDX, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32,
       0, (pt_base >> 12) as u32);
    wr(adev, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32_BASE_IDX, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32,
       0, ((pt_base >> 12) >> 32) as u32);
    // VM_9_X_REGISTER_VM_L2_SAW_CONTEXT0_PAGE_TABLE_START_ADDR_LO32 / HI32
    wr(adev, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_START_ADDR_LO32_BASE_IDX, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_START_ADDR_LO32,
       0, (g.gart_start >> 12) as u32);
    wr(adev, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_START_ADDR_HI32_BASE_IDX, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_START_ADDR_HI32,
       0, (g.gart_start >> 44) as u32);
    // VM_9_X_REGISTER_VM_L2_SAW_CONTEXT0_PAGE_TABLE_END_ADDR_LO32 / HI32
    wr(adev, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_END_ADDR_LO32_BASE_IDX, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_END_ADDR_LO32,
       0, (g.gart_end >> 12) as u32);
    wr(adev, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_END_ADDR_HI32_BASE_IDX, MM_mmVM_L2_SAW_CONTEXT0_PAGE_TABLE_END_ADDR_HI32,
       0, (g.gart_end >> 44) as u32);
    // Program SAW CONTEXT0 CNTL
    let mut tmp = rd(adev, MM_mmVM_L2_SAW_CONTEXT0_CNTL_BASE_IDX, MM_mmVM_L2_SAW_CONTEXT0_CNTL, 0);
    tmp |= 1 << CONTEXT0_CNTL_ENABLE_OFFSET;
    tmp &= !(3 << CONTEXT0_CNTL_PAGE_TABLE_DEPTH_OFFSET);
    wr(adev, MM_mmVM_L2_SAW_CONTEXT0_CNTL_BASE_IDX, MM_mmVM_L2_SAW_CONTEXT0_CNTL, 0, tmp);
    // Disable all Contexts except Context0
    wr(adev, MM_mmVM_L2_SAW_CONTEXTS_DISABLE_BASE_IDX, MM_mmVM_L2_SAW_CONTEXTS_DISABLE, 0, 0xfffe);
    // Program SAW CNTL4
    let mut tmp = rd(adev, MM_mmVM_L2_SAW_CNTL4_BASE_IDX, MM_mmVM_L2_SAW_CNTL4, 0);
    tmp |= 1 << VMC_TAP_PDE_REQUEST_SNOOP_OFFSET;
    tmp |= 1 << VMC_TAP_PTE_REQUEST_SNOOP_OFFSET;
    wr(adev, MM_mmVM_L2_SAW_CNTL4_BASE_IDX, MM_mmVM_L2_SAW_CNTL4, 0, tmp);
}

/// `mmhub_v1_0_update_power_gating`: the SMU powers the MMHUB down only
/// with AMD_PG_SUPPORT_MMHUB.
pub fn mmhub_v1_0_update_power_gating(adev: &amdgpu_device, _enable: bool) -> bool {
    adev.pg_flags & AMD_PG_SUPPORT_MMHUB != 0
}

/// `mmhub_v1_0_program_invalidation`.
fn mmhub_v1_0_program_invalidation(adev: &amdgpu_device) {
    let hub = adev.vmhub[AMDGPU_MMHUB0_0 as usize];
    for i in 0..18u32 {
        wr(adev, MM_mmVM_INVALIDATE_ENG0_ADDR_RANGE_LO32_BASE_IDX, MM_mmVM_INVALIDATE_ENG0_ADDR_RANGE_LO32,
           i * hub.eng_addr_distance, 0xffff_ffff);
        wr(adev, MM_mmVM_INVALIDATE_ENG0_ADDR_RANGE_HI32_BASE_IDX, MM_mmVM_INVALIDATE_ENG0_ADDR_RANGE_HI32,
           i * hub.eng_addr_distance, 0x1f);
    }
}

/// `mmhub_v1_0_gart_enable`: bare metal, so MC_VM_FB_LOCATION stays as the
/// VBIOS left it.
pub fn mmhub_v1_0_gart_enable(adev: &amdgpu_device) -> i32 {
    // GART Enable.
    mmhub_v1_0_init_gart_aperture_regs(adev);
    mmhub_v1_0_init_system_aperture_regs(adev);
    mmhub_v1_0_init_tlb_regs(adev);
    mmhub_v1_0_init_cache_regs(adev);
    mmhub_v1_0_enable_system_domain(adev);
    mmhub_v1_0_disable_identity_aperture(adev);
    mmhub_v1_0_setup_vmid_config(adev);
    mmhub_v1_0_program_invalidation(adev);
    0
}

/// `mmhub_v1_0_set_fault_enable_default`.
pub fn mmhub_v1_0_set_fault_enable_default(adev: &amdgpu_device, value: bool) {
    let v = value as u32;
    let mut tmp = rd(adev, MM_mmVM_L2_PROTECTION_FAULT_CNTL_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_CNTL, 0);
    for (m, s) in [
        (MM_VM_L2_PROTECTION_FAULT_CNTL__RANGE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__RANGE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__PDE0_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__PDE0_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__PDE1_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__PDE1_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__PDE2_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__PDE2_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__TRANSLATE_FURTHER_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__TRANSLATE_FURTHER_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__NACK_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__NACK_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__DUMMY_PAGE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__DUMMY_PAGE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__VALID_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__VALID_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__READ_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__READ_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__WRITE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__WRITE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (MM_VM_L2_PROTECTION_FAULT_CNTL__EXECUTE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         MM_VM_L2_PROTECTION_FAULT_CNTL__EXECUTE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
    ] {
        tmp = set(tmp, m, s, v);
    }
    if !value {
        tmp = set(tmp, MM_VM_L2_PROTECTION_FAULT_CNTL__CRASH_ON_NO_RETRY_FAULT_MASK,
                  MM_VM_L2_PROTECTION_FAULT_CNTL__CRASH_ON_NO_RETRY_FAULT__SHIFT, 1);
        tmp = set(tmp, MM_VM_L2_PROTECTION_FAULT_CNTL__CRASH_ON_RETRY_FAULT_MASK,
                  MM_VM_L2_PROTECTION_FAULT_CNTL__CRASH_ON_RETRY_FAULT__SHIFT, 1);
    }
    wr(adev, MM_mmVM_L2_PROTECTION_FAULT_CNTL_BASE_IDX, MM_mmVM_L2_PROTECTION_FAULT_CNTL, 0, tmp);
}
