//! amdgpu/soc15.c (Linux 6.18.55): the COMMON IP block of a SOC15 chip:
//! early_init, sw_init and hw_init.

use crate::amdgpu_device::*;
use crate::nbio_v7_0;
use crate::regs::*;

/// `amdgpu_device_get_rev_id` (amdgpu_device.c): `nbio.funcs->get_rev_id`.
pub fn amdgpu_device_get_rev_id(adev: &amdgpu_device) -> u32 {
    match adev.nbio_funcs {
        Some(amdgpu_nbio_funcs::nbio_v7_0) => nbio_v7_0::nbio_v7_0_get_rev_id(adev).unwrap_or(0),
        None => 0,
    }
}

/// `soc15_common_early_init`. Of the register access hooks it sets, the
/// port has `pcie_rreg`/`pcie_wreg` = `amdgpu_device_indirect_rreg`/`wreg`.
/// -EINVAL for a GC version without a case, as in Linux.
pub fn soc15_common_early_init(adev: &mut amdgpu_device) -> i32 {
    match adev.nbio_funcs {
        Some(amdgpu_nbio_funcs::nbio_v7_0) => nbio_v7_0::nbio_v7_0_set_reg_remap(adev),
        None => {}
    }
    adev.pcie_reg_indirect = true;

    adev.rev_id = amdgpu_device_get_rev_id(adev);
    adev.external_rev_id = 0xFF;
    match adev.ip_version(GC_HWIP, 0) {
        v if v == 9 << 24 | 3 << 16 => {
            adev.asic_funcs = Some(amdgpu_asic_funcs::soc15);

            if adev.apu_flags & AMD_APU_IS_RENOIR != 0 {
                adev.external_rev_id = adev.rev_id + 0x91;
            } else {
                adev.external_rev_id = adev.rev_id + 0xa1;
            }
            adev.cg_flags = (AMD_CG_SUPPORT_GFX_MGCG
                | AMD_CG_SUPPORT_GFX_MGLS
                | AMD_CG_SUPPORT_GFX_3D_CGCG
                | AMD_CG_SUPPORT_GFX_3D_CGLS
                | AMD_CG_SUPPORT_GFX_CGCG
                | AMD_CG_SUPPORT_GFX_CGLS
                | AMD_CG_SUPPORT_GFX_CP_LS
                | AMD_CG_SUPPORT_MC_MGCG
                | AMD_CG_SUPPORT_MC_LS
                | AMD_CG_SUPPORT_SDMA_MGCG
                | AMD_CG_SUPPORT_SDMA_LS
                | AMD_CG_SUPPORT_BIF_LS
                | AMD_CG_SUPPORT_HDP_LS
                | AMD_CG_SUPPORT_VCN_MGCG
                | AMD_CG_SUPPORT_JPEG_MGCG
                | AMD_CG_SUPPORT_IH_CG
                | AMD_CG_SUPPORT_ATHUB_LS
                | AMD_CG_SUPPORT_ATHUB_MGCG
                | AMD_CG_SUPPORT_DF_MGCG) as u64;
            adev.pg_flags = AMD_PG_SUPPORT_SDMA | AMD_PG_SUPPORT_VCN | AMD_PG_SUPPORT_JPEG | AMD_PG_SUPPORT_VCN_DPG;
        }
        // FIXME: not supported yet
        _ => return -22,
    }
    // Bare metal: no amdgpu_virt_init_setting.
    0
}

/// `df_v1_7_sw_init`.
fn df_v1_7_sw_init(adev: &mut amdgpu_device) {
    adev.df_hash_status = amdgpu_df_hash_status { hash_64k: false, hash_2m: false, hash_1g: false };
}

/// `soc15_common_sw_init`: bare metal, so no mailbox IRQ.
pub fn soc15_common_sw_init(adev: &mut amdgpu_device) -> i32 {
    match adev.df_funcs {
        Some(amdgpu_df_funcs::df_v1_7) => df_v1_7_sw_init(adev),
        None => {}
    }
    0
}

/// `amdgpu_device_should_use_aspm` (amdgpu_device.c) with `amdgpu_aspm`
/// at its default -1: an APU never does.
fn amdgpu_device_should_use_aspm(adev: &amdgpu_device) -> bool {
    if adev.flags & AMD_IS_APU != 0 {
        return false;
    }
    // A dGPU's upstream link is not ported.
    false
}

/// `soc15_program_aspm`: nbio_v7_0 has no `program_aspm`.
fn soc15_program_aspm(adev: &amdgpu_device) {
    if !amdgpu_device_should_use_aspm(adev) {
        return;
    }
}

/// `soc15_sdma_doorbell_range_init`.
fn soc15_sdma_doorbell_range_init(adev: &amdgpu_device) {
    // sdma doorbell range is programed by hypervisor
    for i in 0..adev.sdma.num_instances as usize {
        nbio_v7_0::nbio_v7_0_sdma_doorbell_range(adev, i, true, adev.doorbell_index.sdma_engine[i] << 1,
            adev.doorbell_index.sdma_doorbell_range);
    }
}

/// `soc15_common_hw_init`.
pub fn soc15_common_hw_init(adev: &mut amdgpu_device) -> i32 {
    // enable aspm
    soc15_program_aspm(adev);
    let Some(amdgpu_nbio_funcs::nbio_v7_0) = adev.nbio_funcs else { return -22 };
    // setup nbio registers
    nbio_v7_0::nbio_v7_0_init_registers(adev);
    // remap HDP registers to a hole in mmio space, for the purpose of
    // expose those registers to process space
    nbio_v7_0::nbio_v7_0_remap_hdp_registers(adev);
    // enable the doorbell aperture
    nbio_v7_0::nbio_v7_0_enable_doorbell_aperture(adev, true);
    // HW doorbell routing policy: doorbell writing not in SDMA/IH/MM/ACV
    // range will be routed to CP. So we need to init SDMA doorbell range
    // prior to CP ip block init and ring test.
    soc15_sdma_doorbell_range_init(adev);
    0
}
