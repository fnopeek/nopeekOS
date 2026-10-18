//! amdgpu/soc15.c (Linux 6.18.55): the COMMON IP block of a SOC15 chip, as
//! far as `amdgpu_device_ip_early_init` runs it.

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

/// `soc15_common_early_init`. The register access hooks it sets
/// (`pcie_rreg` = `amdgpu_device_indirect_rreg` and the rest) are the
/// functions the port calls directly. -EINVAL for a GC version without a
/// case, as in Linux.
pub fn soc15_common_early_init(adev: &mut amdgpu_device) -> i32 {
    match adev.nbio_funcs {
        Some(amdgpu_nbio_funcs::nbio_v7_0) => nbio_v7_0::nbio_v7_0_set_reg_remap(adev),
        None => {}
    }

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
