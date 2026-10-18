//! amdgpu/gfxhub_v1_0.c (Linux 6.18.55): the GFX VM hub of GC 9.x.

use crate::amdgpu_device::amdgpu_device;
use crate::nbio_v7_0::rreg32_soc15;
use crate::regs::*;

/// `SOC15_REG_OFFSET(GC, 0, reg)`.
fn gc_offset(adev: &amdgpu_device, base_idx: u32, reg: u32) -> u32 {
    adev.reg_offset[GC_HWIP as usize][0].get(base_idx as usize).copied().unwrap_or(0) + reg
}

/// `gfxhub_v1_0_get_mc_fb_offset`.
pub fn gfxhub_v1_0_get_mc_fb_offset(adev: &amdgpu_device) -> u64 {
    (rreg32_soc15(adev, GC_HWIP, 0, GC_mmMC_VM_FB_OFFSET_BASE_IDX, GC_mmMC_VM_FB_OFFSET).unwrap_or(0) as u64) << 24
}

/// `gfxhub_v1_0_init`.
pub fn gfxhub_v1_0_init(adev: &mut amdgpu_device) {
    let o = |i, r| gc_offset(adev, i, r);
    let hub = crate::amdgpu_device::amdgpu_vmhub {
        ctx0_ptb_addr_lo32: o(GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32),
        ctx0_ptb_addr_hi32: o(GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32),
        vm_inv_eng0_sem: o(GC_mmVM_INVALIDATE_ENG0_SEM_BASE_IDX, GC_mmVM_INVALIDATE_ENG0_SEM),
        vm_inv_eng0_req: o(GC_mmVM_INVALIDATE_ENG0_REQ_BASE_IDX, GC_mmVM_INVALIDATE_ENG0_REQ),
        vm_inv_eng0_ack: o(GC_mmVM_INVALIDATE_ENG0_ACK_BASE_IDX, GC_mmVM_INVALIDATE_ENG0_ACK),
        vm_context0_cntl: o(GC_mmVM_CONTEXT0_CNTL_BASE_IDX, GC_mmVM_CONTEXT0_CNTL),
        vm_l2_pro_fault_status: o(GC_mmVM_L2_PROTECTION_FAULT_STATUS_BASE_IDX, GC_mmVM_L2_PROTECTION_FAULT_STATUS),
        vm_l2_pro_fault_cntl: o(GC_mmVM_L2_PROTECTION_FAULT_CNTL_BASE_IDX, GC_mmVM_L2_PROTECTION_FAULT_CNTL),
        ctx_distance: GC_CTX_DISTANCE,
        ctx_addr_distance: GC_CTX_ADDR_DISTANCE,
        eng_distance: GC_ENG_DISTANCE,
        eng_addr_distance: GC_ENG_ADDR_DISTANCE,
    };
    adev.vmhub[AMDGPU_GFXHUB_0 as usize] = hub;
}

/// `RREG32_SOC15(GC, 0, reg)` / `RREG32_SOC15_OFFSET`.
fn rd(adev: &amdgpu_device, idx: u32, reg: u32, off: u32) -> u32 {
    rreg32_soc15(adev, GC_HWIP, 0, idx, reg + off).unwrap_or(0)
}

/// `WREG32_SOC15(GC, 0, reg, v)` / `_OFFSET` / `_RLC` on bare metal.
fn wr(adev: &amdgpu_device, idx: u32, reg: u32, off: u32, v: u32) {
    crate::nbio_v7_0::wreg32_soc15(adev, GC_HWIP, 0, idx, reg + off, v);
}

/// `REG_SET_FIELD`.
fn set(v: u32, mask: u32, shift: u32, f: u32) -> u32 {
    (v & !mask) | (mask & (f << shift))
}

/// `gfxhub_v1_0_setup_vm_pt_regs`.
fn gfxhub_v1_0_setup_vm_pt_regs(adev: &amdgpu_device, vmid: u32, page_table_base: u64) {
    let hub = adev.vmhub[AMDGPU_GFXHUB_0 as usize];
    wr(adev, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32,
       hub.ctx_addr_distance * vmid, page_table_base as u32);
    wr(adev, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32,
       hub.ctx_addr_distance * vmid, (page_table_base >> 32) as u32);
}

/// `gfxhub_v1_0_init_gart_aperture_regs`: no PDB0.
fn gfxhub_v1_0_init_gart_aperture_regs(adev: &amdgpu_device) {
    let pt_base = crate::amdgpu_gmc::amdgpu_gmc_pd_addr(adev, adev.gart.bo.unwrap());
    gfxhub_v1_0_setup_vm_pt_regs(adev, 0, pt_base);
    let g = &adev.gmc;
    wr(adev, GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_LO32, 0,
       (g.gart_start >> 12) as u32);
    wr(adev, GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_HI32, 0,
       (g.gart_start >> 44) as u32);
    wr(adev, GC_mmVM_CONTEXT0_PAGE_TABLE_END_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_END_ADDR_LO32, 0,
       (g.gart_end >> 12) as u32);
    wr(adev, GC_mmVM_CONTEXT0_PAGE_TABLE_END_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_END_ADDR_HI32, 0,
       (g.gart_end >> 44) as u32);
}

/// `gfxhub_v1_0_init_system_aperture_regs`: bare metal, no PDB0.
fn gfxhub_v1_0_init_system_aperture_regs(adev: &amdgpu_device) {
    let g = &adev.gmc;
    // Program the AGP BAR
    wr(adev, GC_mmMC_VM_AGP_BASE_BASE_IDX, GC_mmMC_VM_AGP_BASE, 0, 0);
    wr(adev, GC_mmMC_VM_AGP_BOT_BASE_IDX, GC_mmMC_VM_AGP_BOT, 0, (g.agp_start >> 24) as u32);
    wr(adev, GC_mmMC_VM_AGP_TOP_BASE_IDX, GC_mmMC_VM_AGP_TOP, 0, (g.agp_end >> 24) as u32);
    // Program the system aperture low logical page number.
    wr(adev, GC_mmMC_VM_SYSTEM_APERTURE_LOW_ADDR_BASE_IDX, GC_mmMC_VM_SYSTEM_APERTURE_LOW_ADDR, 0,
       (g.fb_start.min(g.agp_start) >> 18) as u32);
    if adev.apu_flags & (AMD_APU_IS_RAVEN2 | AMD_APU_IS_RENOIR | AMD_APU_IS_GREEN_SARDINE) != 0 {
        // Raven2 has a HW issue that it is unable to use the vram which is
        // out of MC_VM_SYSTEM_APERTURE_HIGH_ADDR. So here is the workaround
        // that increase system aperture high address (add 1) to get rid of
        // the VM fault and hardware hang.
        wr(adev, GC_mmMC_VM_SYSTEM_APERTURE_HIGH_ADDR_BASE_IDX, GC_mmMC_VM_SYSTEM_APERTURE_HIGH_ADDR, 0,
           ((g.fb_end >> 18) + 1).max(g.agp_end >> 18) as u32);
    } else {
        wr(adev, GC_mmMC_VM_SYSTEM_APERTURE_HIGH_ADDR_BASE_IDX, GC_mmMC_VM_SYSTEM_APERTURE_HIGH_ADDR, 0,
           (g.fb_end.max(g.agp_end) >> 18) as u32);
    }
    // Set default page address.
    let value = crate::amdgpu_gmc::amdgpu_gmc_vram_mc2pa(adev, adev.mem_scratch_gpu_addr);
    wr(adev, GC_mmMC_VM_SYSTEM_APERTURE_DEFAULT_ADDR_LSB_BASE_IDX, GC_mmMC_VM_SYSTEM_APERTURE_DEFAULT_ADDR_LSB, 0,
       (value >> 12) as u32);
    wr(adev, GC_mmMC_VM_SYSTEM_APERTURE_DEFAULT_ADDR_MSB_BASE_IDX, GC_mmMC_VM_SYSTEM_APERTURE_DEFAULT_ADDR_MSB, 0,
       (value >> 44) as u32);
    // Program "protection fault".
    wr(adev, GC_mmVM_L2_PROTECTION_FAULT_DEFAULT_ADDR_LO32_BASE_IDX, GC_mmVM_L2_PROTECTION_FAULT_DEFAULT_ADDR_LO32, 0,
       (adev.dummy_page_addr >> 12) as u32);
    wr(adev, GC_mmVM_L2_PROTECTION_FAULT_DEFAULT_ADDR_HI32_BASE_IDX, GC_mmVM_L2_PROTECTION_FAULT_DEFAULT_ADDR_HI32, 0,
       (adev.dummy_page_addr >> 44) as u32);
    // WREG32_FIELD15
    let (i, r) = (GC_mmVM_L2_PROTECTION_FAULT_CNTL2_BASE_IDX, GC_mmVM_L2_PROTECTION_FAULT_CNTL2);
    let v = rd(adev, i, r, 0);
    wr(adev, i, r, 0, (v & !GC_VM_L2_PROTECTION_FAULT_CNTL2__ACTIVE_PAGE_MIGRATION_PTE_READ_RETRY_MASK)
       | 1 << GC_VM_L2_PROTECTION_FAULT_CNTL2__ACTIVE_PAGE_MIGRATION_PTE_READ_RETRY__SHIFT);
}

/// `gfxhub_v1_0_init_tlb_regs`.
fn gfxhub_v1_0_init_tlb_regs(adev: &amdgpu_device) {
    // Setup TLB control
    let mut tmp = rd(adev, GC_mmMC_VM_MX_L1_TLB_CNTL_BASE_IDX, GC_mmMC_VM_MX_L1_TLB_CNTL, 0);
    tmp = set(tmp, GC_MC_VM_MX_L1_TLB_CNTL__ENABLE_L1_TLB_MASK, GC_MC_VM_MX_L1_TLB_CNTL__ENABLE_L1_TLB__SHIFT, 1);
    tmp = set(tmp, GC_MC_VM_MX_L1_TLB_CNTL__SYSTEM_ACCESS_MODE_MASK, GC_MC_VM_MX_L1_TLB_CNTL__SYSTEM_ACCESS_MODE__SHIFT, 3);
    tmp = set(tmp, GC_MC_VM_MX_L1_TLB_CNTL__ENABLE_ADVANCED_DRIVER_MODEL_MASK,
              GC_MC_VM_MX_L1_TLB_CNTL__ENABLE_ADVANCED_DRIVER_MODEL__SHIFT, 1);
    tmp = set(tmp, GC_MC_VM_MX_L1_TLB_CNTL__SYSTEM_APERTURE_UNMAPPED_ACCESS_MASK,
              GC_MC_VM_MX_L1_TLB_CNTL__SYSTEM_APERTURE_UNMAPPED_ACCESS__SHIFT, 0);
    // XXX for emulation.
    tmp = set(tmp, GC_MC_VM_MX_L1_TLB_CNTL__MTYPE_MASK, GC_MC_VM_MX_L1_TLB_CNTL__MTYPE__SHIFT, MTYPE_UC);
    tmp = set(tmp, GC_MC_VM_MX_L1_TLB_CNTL__ATC_EN_MASK, GC_MC_VM_MX_L1_TLB_CNTL__ATC_EN__SHIFT, 1);
    wr(adev, GC_mmMC_VM_MX_L1_TLB_CNTL_BASE_IDX, GC_mmMC_VM_MX_L1_TLB_CNTL, 0, tmp);
}

/// `gfxhub_v1_0_init_cache_regs`.
fn gfxhub_v1_0_init_cache_regs(adev: &amdgpu_device) {
    // Setup L2 cache
    let mut tmp = rd(adev, GC_mmVM_L2_CNTL_BASE_IDX, GC_mmVM_L2_CNTL, 0);
    tmp = set(tmp, GC_VM_L2_CNTL__ENABLE_L2_CACHE_MASK, GC_VM_L2_CNTL__ENABLE_L2_CACHE__SHIFT, 1);
    tmp = set(tmp, GC_VM_L2_CNTL__ENABLE_L2_FRAGMENT_PROCESSING_MASK, GC_VM_L2_CNTL__ENABLE_L2_FRAGMENT_PROCESSING__SHIFT, 1);
    // XXX for emulation, Refer to closed source code.
    tmp = set(tmp, GC_VM_L2_CNTL__L2_PDE0_CACHE_TAG_GENERATION_MODE_MASK,
              GC_VM_L2_CNTL__L2_PDE0_CACHE_TAG_GENERATION_MODE__SHIFT, 0);
    tmp = set(tmp, GC_VM_L2_CNTL__PDE_FAULT_CLASSIFICATION_MASK, GC_VM_L2_CNTL__PDE_FAULT_CLASSIFICATION__SHIFT, 0);
    tmp = set(tmp, GC_VM_L2_CNTL__CONTEXT1_IDENTITY_ACCESS_MODE_MASK, GC_VM_L2_CNTL__CONTEXT1_IDENTITY_ACCESS_MODE__SHIFT, 1);
    tmp = set(tmp, GC_VM_L2_CNTL__IDENTITY_MODE_FRAGMENT_SIZE_MASK, GC_VM_L2_CNTL__IDENTITY_MODE_FRAGMENT_SIZE__SHIFT, 0);
    wr(adev, GC_mmVM_L2_CNTL_BASE_IDX, GC_mmVM_L2_CNTL, 0, tmp);

    let mut tmp = rd(adev, GC_mmVM_L2_CNTL2_BASE_IDX, GC_mmVM_L2_CNTL2, 0);
    tmp = set(tmp, GC_VM_L2_CNTL2__INVALIDATE_ALL_L1_TLBS_MASK, GC_VM_L2_CNTL2__INVALIDATE_ALL_L1_TLBS__SHIFT, 1);
    tmp = set(tmp, GC_VM_L2_CNTL2__INVALIDATE_L2_CACHE_MASK, GC_VM_L2_CNTL2__INVALIDATE_L2_CACHE__SHIFT, 1);
    wr(adev, GC_mmVM_L2_CNTL2_BASE_IDX, GC_mmVM_L2_CNTL2, 0, tmp);

    let mut tmp = GC_mmVM_L2_CNTL3_DEFAULT;
    if adev.gmc.translate_further {
        tmp = set(tmp, GC_VM_L2_CNTL3__BANK_SELECT_MASK, GC_VM_L2_CNTL3__BANK_SELECT__SHIFT, 12);
        tmp = set(tmp, GC_VM_L2_CNTL3__L2_CACHE_BIGK_FRAGMENT_SIZE_MASK, GC_VM_L2_CNTL3__L2_CACHE_BIGK_FRAGMENT_SIZE__SHIFT, 9);
    } else {
        tmp = set(tmp, GC_VM_L2_CNTL3__BANK_SELECT_MASK, GC_VM_L2_CNTL3__BANK_SELECT__SHIFT, 9);
        tmp = set(tmp, GC_VM_L2_CNTL3__L2_CACHE_BIGK_FRAGMENT_SIZE_MASK, GC_VM_L2_CNTL3__L2_CACHE_BIGK_FRAGMENT_SIZE__SHIFT, 6);
    }
    wr(adev, GC_mmVM_L2_CNTL3_BASE_IDX, GC_mmVM_L2_CNTL3, 0, tmp);

    let mut tmp = GC_mmVM_L2_CNTL4_DEFAULT;
    let phys = adev.gmc.xgmi.connected_to_cpu as u32;
    tmp = set(tmp, GC_VM_L2_CNTL4__VMC_TAP_PDE_REQUEST_PHYSICAL_MASK, GC_VM_L2_CNTL4__VMC_TAP_PDE_REQUEST_PHYSICAL__SHIFT, phys);
    tmp = set(tmp, GC_VM_L2_CNTL4__VMC_TAP_PTE_REQUEST_PHYSICAL_MASK, GC_VM_L2_CNTL4__VMC_TAP_PTE_REQUEST_PHYSICAL__SHIFT, phys);
    wr(adev, GC_mmVM_L2_CNTL4_BASE_IDX, GC_mmVM_L2_CNTL4, 0, tmp);
}

/// `gfxhub_v1_0_enable_system_domain`.
fn gfxhub_v1_0_enable_system_domain(adev: &amdgpu_device) {
    let mut tmp = rd(adev, GC_mmVM_CONTEXT0_CNTL_BASE_IDX, GC_mmVM_CONTEXT0_CNTL, 0);
    tmp = set(tmp, GC_VM_CONTEXT0_CNTL__ENABLE_CONTEXT_MASK, GC_VM_CONTEXT0_CNTL__ENABLE_CONTEXT__SHIFT, 1);
    tmp = set(tmp, GC_VM_CONTEXT0_CNTL__PAGE_TABLE_DEPTH_MASK, GC_VM_CONTEXT0_CNTL__PAGE_TABLE_DEPTH__SHIFT,
              adev.gmc.vmid0_page_table_depth);
    tmp = set(tmp, GC_VM_CONTEXT0_CNTL__PAGE_TABLE_BLOCK_SIZE_MASK, GC_VM_CONTEXT0_CNTL__PAGE_TABLE_BLOCK_SIZE__SHIFT,
              adev.gmc.vmid0_page_table_block_size);
    tmp = set(tmp, GC_VM_CONTEXT0_CNTL__RETRY_PERMISSION_OR_INVALID_PAGE_FAULT_MASK,
              GC_VM_CONTEXT0_CNTL__RETRY_PERMISSION_OR_INVALID_PAGE_FAULT__SHIFT, 0);
    wr(adev, GC_mmVM_CONTEXT0_CNTL_BASE_IDX, GC_mmVM_CONTEXT0_CNTL, 0, tmp);
}

/// `gfxhub_v1_0_disable_identity_aperture`.
fn gfxhub_v1_0_disable_identity_aperture(adev: &amdgpu_device) {
    wr(adev, GC_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_LOW_ADDR_LO32_BASE_IDX,
       GC_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_LOW_ADDR_LO32, 0, 0xFFFF_FFFF);
    wr(adev, GC_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_LOW_ADDR_HI32_BASE_IDX,
       GC_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_LOW_ADDR_HI32, 0, 0x0000_000F);
    wr(adev, GC_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_LO32_BASE_IDX,
       GC_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_LO32, 0, 0);
    wr(adev, GC_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_HI32_BASE_IDX,
       GC_mmVM_L2_CONTEXT1_IDENTITY_APERTURE_HIGH_ADDR_HI32, 0, 0);
    wr(adev, GC_mmVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_LO32_BASE_IDX,
       GC_mmVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_LO32, 0, 0);
    wr(adev, GC_mmVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_HI32_BASE_IDX,
       GC_mmVM_L2_CONTEXT_IDENTITY_PHYSICAL_OFFSET_HI32, 0, 0);
}

/// `gfxhub_v1_0_setup_vmid_config`.
fn gfxhub_v1_0_setup_vmid_config(adev: &amdgpu_device) {
    let hub = adev.vmhub[AMDGPU_GFXHUB_0 as usize];
    let mut num_level = adev.vm_manager.num_level;
    let mut block_size = adev.vm_manager.block_size;
    if adev.gmc.translate_further {
        num_level -= 1;
    } else {
        block_size -= 9;
    }
    for i in 0..=14u32 {
        let mut tmp = rd(adev, GC_mmVM_CONTEXT1_CNTL_BASE_IDX, GC_mmVM_CONTEXT1_CNTL, i * hub.ctx_distance);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__ENABLE_CONTEXT_MASK, GC_VM_CONTEXT1_CNTL__ENABLE_CONTEXT__SHIFT, 1);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__PAGE_TABLE_DEPTH_MASK, GC_VM_CONTEXT1_CNTL__PAGE_TABLE_DEPTH__SHIFT, num_level);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__RANGE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  GC_VM_CONTEXT1_CNTL__RANGE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__DUMMY_PAGE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  GC_VM_CONTEXT1_CNTL__DUMMY_PAGE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__PDE0_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  GC_VM_CONTEXT1_CNTL__PDE0_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__VALID_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  GC_VM_CONTEXT1_CNTL__VALID_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__READ_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  GC_VM_CONTEXT1_CNTL__READ_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__WRITE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  GC_VM_CONTEXT1_CNTL__WRITE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__EXECUTE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
                  GC_VM_CONTEXT1_CNTL__EXECUTE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT, 1);
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__PAGE_TABLE_BLOCK_SIZE_MASK, GC_VM_CONTEXT1_CNTL__PAGE_TABLE_BLOCK_SIZE__SHIFT,
                  block_size);
        // Send no-retry XNACK on fault to suppress VM fault storm.
        tmp = set(tmp, GC_VM_CONTEXT1_CNTL__RETRY_PERMISSION_OR_INVALID_PAGE_FAULT_MASK,
                  GC_VM_CONTEXT1_CNTL__RETRY_PERMISSION_OR_INVALID_PAGE_FAULT__SHIFT,
                  (adev.gmc.noretry == 0 || adev.asic_type == CHIP_ALDEBARAN) as u32);
        wr(adev, GC_mmVM_CONTEXT1_CNTL_BASE_IDX, GC_mmVM_CONTEXT1_CNTL, i * hub.ctx_distance, tmp);
        let d = i * hub.ctx_addr_distance;
        wr(adev, GC_mmVM_CONTEXT1_PAGE_TABLE_START_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT1_PAGE_TABLE_START_ADDR_LO32, d, 0);
        wr(adev, GC_mmVM_CONTEXT1_PAGE_TABLE_START_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT1_PAGE_TABLE_START_ADDR_HI32, d, 0);
        wr(adev, GC_mmVM_CONTEXT1_PAGE_TABLE_END_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT1_PAGE_TABLE_END_ADDR_LO32, d,
           (adev.vm_manager.max_pfn - 1) as u32);
        wr(adev, GC_mmVM_CONTEXT1_PAGE_TABLE_END_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT1_PAGE_TABLE_END_ADDR_HI32, d,
           ((adev.vm_manager.max_pfn - 1) >> 32) as u32);
    }
}

/// `gfxhub_v1_0_program_invalidation`.
fn gfxhub_v1_0_program_invalidation(adev: &amdgpu_device) {
    let hub = adev.vmhub[AMDGPU_GFXHUB_0 as usize];
    for i in 0..18u32 {
        wr(adev, GC_mmVM_INVALIDATE_ENG0_ADDR_RANGE_LO32_BASE_IDX, GC_mmVM_INVALIDATE_ENG0_ADDR_RANGE_LO32,
           i * hub.eng_addr_distance, 0xffff_ffff);
        wr(adev, GC_mmVM_INVALIDATE_ENG0_ADDR_RANGE_HI32_BASE_IDX, GC_mmVM_INVALIDATE_ENG0_ADDR_RANGE_HI32,
           i * hub.eng_addr_distance, 0x1f);
    }
}

/// `gfxhub_v1_0_gart_enable`: bare metal.
pub fn gfxhub_v1_0_gart_enable(adev: &amdgpu_device) -> i32 {
    // GART Enable.
    gfxhub_v1_0_init_gart_aperture_regs(adev);
    gfxhub_v1_0_init_system_aperture_regs(adev);
    gfxhub_v1_0_init_tlb_regs(adev);
    gfxhub_v1_0_init_cache_regs(adev);
    gfxhub_v1_0_enable_system_domain(adev);
    gfxhub_v1_0_disable_identity_aperture(adev);
    gfxhub_v1_0_setup_vmid_config(adev);
    gfxhub_v1_0_program_invalidation(adev);
    0
}

/// `gfxhub_v1_0_set_fault_enable_default`.
pub fn gfxhub_v1_0_set_fault_enable_default(adev: &amdgpu_device, value: bool) {
    let v = value as u32;
    let mut tmp = rd(adev, GC_mmVM_L2_PROTECTION_FAULT_CNTL_BASE_IDX, GC_mmVM_L2_PROTECTION_FAULT_CNTL, 0);
    for (m, s) in [
        (GC_VM_L2_PROTECTION_FAULT_CNTL__RANGE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__RANGE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__PDE0_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__PDE0_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__PDE1_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__PDE1_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__PDE2_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__PDE2_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__TRANSLATE_FURTHER_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__TRANSLATE_FURTHER_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__NACK_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__NACK_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__DUMMY_PAGE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__DUMMY_PAGE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__VALID_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__VALID_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__READ_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__READ_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__WRITE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__WRITE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
        (GC_VM_L2_PROTECTION_FAULT_CNTL__EXECUTE_PROTECTION_FAULT_ENABLE_DEFAULT_MASK,
         GC_VM_L2_PROTECTION_FAULT_CNTL__EXECUTE_PROTECTION_FAULT_ENABLE_DEFAULT__SHIFT),
    ] {
        tmp = set(tmp, m, s, v);
    }
    if !value {
        tmp = set(tmp, GC_VM_L2_PROTECTION_FAULT_CNTL__CRASH_ON_NO_RETRY_FAULT_MASK,
                  GC_VM_L2_PROTECTION_FAULT_CNTL__CRASH_ON_NO_RETRY_FAULT__SHIFT, 1);
        tmp = set(tmp, GC_VM_L2_PROTECTION_FAULT_CNTL__CRASH_ON_RETRY_FAULT_MASK,
                  GC_VM_L2_PROTECTION_FAULT_CNTL__CRASH_ON_RETRY_FAULT__SHIFT, 1);
    }
    wr(adev, GC_mmVM_L2_PROTECTION_FAULT_CNTL_BASE_IDX, GC_mmVM_L2_PROTECTION_FAULT_CNTL, 0, tmp);
}
