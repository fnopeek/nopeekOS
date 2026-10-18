//! The `amdgpu_ip_block_version`s Renoir gets from
//! `amdgpu_discovery_set_ip_blocks`, each defined where Linux defines it:
//! soc15.c, gmc_v9_0.c, vega10_ih.c, amdgpu_psp.c, pm/swsmu/amdgpu_smu.c,
//! display/amdgpu_dm/amdgpu_dm.c, gfx_v9_0.c, sdma_v4_0.c, vcn_v2_0.c,
//! jpeg_v2_0.c. Type, version and `funcs->name` are read off those files by
//! gen_regs.py. The `amd_ip_funcs` themselves are ported block by block.

use crate::regs::*;

/// `struct amdgpu_ip_block_version`, with `funcs` reduced to its name.
pub struct amdgpu_ip_block_version {
    pub type_: u32,
    pub major: u32,
    pub minor: u32,
    pub rev: u32,
    pub funcs_name: &'static [u8],
}

macro_rules! block {
    ($name:ident, $t:ident, $ma:ident, $mi:ident, $r:ident, $f:ident) => {
        pub static $name: amdgpu_ip_block_version =
            amdgpu_ip_block_version { type_: $t, major: $ma, minor: $mi, rev: $r, funcs_name: $f };
    };
}

block!(vega10_common_ip_block, VEGA10_COMMON_IP_BLOCK_type, VEGA10_COMMON_IP_BLOCK_major,
       VEGA10_COMMON_IP_BLOCK_minor, VEGA10_COMMON_IP_BLOCK_rev, VEGA10_COMMON_IP_BLOCK_funcs_name);
block!(gmc_v9_0_ip_block, GMC_V9_0_IP_BLOCK_type, GMC_V9_0_IP_BLOCK_major, GMC_V9_0_IP_BLOCK_minor,
       GMC_V9_0_IP_BLOCK_rev, GMC_V9_0_IP_BLOCK_funcs_name);
block!(vega10_ih_ip_block, VEGA10_IH_IP_BLOCK_type, VEGA10_IH_IP_BLOCK_major, VEGA10_IH_IP_BLOCK_minor,
       VEGA10_IH_IP_BLOCK_rev, VEGA10_IH_IP_BLOCK_funcs_name);
block!(psp_v12_0_ip_block, PSP_V12_0_IP_BLOCK_type, PSP_V12_0_IP_BLOCK_major, PSP_V12_0_IP_BLOCK_minor,
       PSP_V12_0_IP_BLOCK_rev, PSP_V12_0_IP_BLOCK_funcs_name);
block!(smu_v12_0_ip_block, SMU_V12_0_IP_BLOCK_type, SMU_V12_0_IP_BLOCK_major, SMU_V12_0_IP_BLOCK_minor,
       SMU_V12_0_IP_BLOCK_rev, SMU_V12_0_IP_BLOCK_funcs_name);
block!(dm_ip_block, DM_IP_BLOCK_type, DM_IP_BLOCK_major, DM_IP_BLOCK_minor, DM_IP_BLOCK_rev,
       DM_IP_BLOCK_funcs_name);
block!(gfx_v9_0_ip_block, GFX_V9_0_IP_BLOCK_type, GFX_V9_0_IP_BLOCK_major, GFX_V9_0_IP_BLOCK_minor,
       GFX_V9_0_IP_BLOCK_rev, GFX_V9_0_IP_BLOCK_funcs_name);
block!(sdma_v4_0_ip_block, SDMA_V4_0_IP_BLOCK_type, SDMA_V4_0_IP_BLOCK_major, SDMA_V4_0_IP_BLOCK_minor,
       SDMA_V4_0_IP_BLOCK_rev, SDMA_V4_0_IP_BLOCK_funcs_name);
block!(vcn_v2_0_ip_block, VCN_V2_0_IP_BLOCK_type, VCN_V2_0_IP_BLOCK_major, VCN_V2_0_IP_BLOCK_minor,
       VCN_V2_0_IP_BLOCK_rev, VCN_V2_0_IP_BLOCK_funcs_name);
block!(jpeg_v2_0_ip_block, JPEG_V2_0_IP_BLOCK_type, JPEG_V2_0_IP_BLOCK_major, JPEG_V2_0_IP_BLOCK_minor,
       JPEG_V2_0_IP_BLOCK_rev, JPEG_V2_0_IP_BLOCK_funcs_name);

/// `ip_block_name`: the name `ip_block_names` gives a block type.
pub fn ip_block_name(type_: u32) -> &'static [u8] {
    const NAMES: &[(u32, &[u8])] = &[
        (AMD_IP_BLOCK_TYPE_COMMON, IP_BLOCK_NAME_COMMON), (AMD_IP_BLOCK_TYPE_GMC, IP_BLOCK_NAME_GMC),
        (AMD_IP_BLOCK_TYPE_IH, IP_BLOCK_NAME_IH), (AMD_IP_BLOCK_TYPE_SMC, IP_BLOCK_NAME_SMC),
        (AMD_IP_BLOCK_TYPE_PSP, IP_BLOCK_NAME_PSP), (AMD_IP_BLOCK_TYPE_DCE, IP_BLOCK_NAME_DCE),
        (AMD_IP_BLOCK_TYPE_GFX, IP_BLOCK_NAME_GFX), (AMD_IP_BLOCK_TYPE_SDMA, IP_BLOCK_NAME_SDMA),
        (AMD_IP_BLOCK_TYPE_UVD, IP_BLOCK_NAME_UVD), (AMD_IP_BLOCK_TYPE_VCE, IP_BLOCK_NAME_VCE),
        (AMD_IP_BLOCK_TYPE_ACP, IP_BLOCK_NAME_ACP), (AMD_IP_BLOCK_TYPE_VCN, IP_BLOCK_NAME_VCN),
        (AMD_IP_BLOCK_TYPE_MES, IP_BLOCK_NAME_MES), (AMD_IP_BLOCK_TYPE_JPEG, IP_BLOCK_NAME_JPEG),
        (AMD_IP_BLOCK_TYPE_VPE, IP_BLOCK_NAME_VPE), (AMD_IP_BLOCK_TYPE_UMSCH_MM, IP_BLOCK_NAME_UMSCH_MM),
        (AMD_IP_BLOCK_TYPE_ISP, IP_BLOCK_NAME_ISP),
    ];
    NAMES.iter().find(|n| n.0 == type_).map_or(b"unknown", |n| n.1)
}
