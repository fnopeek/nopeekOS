//! display/dc/dm_pp_smu.h + amdgpu_dm/amdgpu_dm_pp_smu.c (Linux 6.18.55):
//! the pp_smu interface DC's clock manager calls into the power-play SMU
//! through. The bound functions are identity enums; the port calls them with
//! `adev` where Linux reaches it through `pp_smu.dm->driver_context`.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc_types::dce_version;

pub const PP_SMU_NUM_SOCCLK_DPM_LEVELS: usize = 8;
pub const PP_SMU_NUM_DCFCLK_DPM_LEVELS: usize = 8;
pub const PP_SMU_NUM_FCLK_DPM_LEVELS: usize = 4;
pub const PP_SMU_NUM_MEMCLK_DPM_LEVELS: usize = 4;
pub const PP_SMU_NUM_DCLK_DPM_LEVELS: usize = 8;
pub const PP_SMU_NUM_VCLK_DPM_LEVELS: usize = 8;
pub const PP_SMU_NUM_VPECLK_DPM_LEVELS: usize = 8;

/// `enum pp_smu_ver`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum pp_smu_ver {
    #[default]
    PP_SMU_UNSUPPORTED,
    PP_SMU_VER_RV,
    PP_SMU_VER_NV,
    PP_SMU_VER_RN,
    PP_SMU_VER_VG,
    PP_SMU_VER_DCN301,
}

/// `enum pp_smu_status`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum pp_smu_status {
    PP_SMU_RESULT_UNDEFINED = 0,
    PP_SMU_RESULT_OK = 1,
    PP_SMU_RESULT_FAIL,
    PP_SMU_RESULT_UNSUPPORTED,
}

/// `struct dpm_clock`.
#[derive(Default, Clone, Copy, Debug)]
pub struct dpm_clock {
    pub Freq: u32,
    pub Vol: u32,
}

/// `struct dpm_clocks`: a copy of the SMU's DpmClocks_t layout.
#[derive(Default, Clone, Copy, Debug)]
pub struct dpm_clocks {
    pub DcfClocks: [dpm_clock; PP_SMU_NUM_DCFCLK_DPM_LEVELS],
    pub SocClocks: [dpm_clock; PP_SMU_NUM_SOCCLK_DPM_LEVELS],
    pub FClocks: [dpm_clock; PP_SMU_NUM_FCLK_DPM_LEVELS],
    pub MemClocks: [dpm_clock; PP_SMU_NUM_MEMCLK_DPM_LEVELS],
    pub VClocks: [dpm_clock; PP_SMU_NUM_VCLK_DPM_LEVELS],
    pub DClocks: [dpm_clock; PP_SMU_NUM_DCLK_DPM_LEVELS],
    pub VPEClocks: [dpm_clock; PP_SMU_NUM_VPECLK_DPM_LEVELS],
}

/// `PP_SMU_WM_SET_RANGE_CLK_UNCONSTRAINED_MIN` / `_MAX`.
pub const PP_SMU_WM_SET_RANGE_CLK_UNCONSTRAINED_MIN: u16 = 0x0;
pub const PP_SMU_WM_SET_RANGE_CLK_UNCONSTRAINED_MAX: u16 = 0xFFFF;

/// `MAX_WATERMARK_SETS`.
pub const MAX_WATERMARK_SETS: usize = 4;

/// `struct pp_smu_wm_set_range`.
#[derive(Default, Clone, Copy, Debug)]
pub struct pp_smu_wm_set_range {
    pub min_fill_clk_mhz: u16,
    pub max_fill_clk_mhz: u16,
    pub min_drain_clk_mhz: u16,
    pub max_drain_clk_mhz: u16,
    pub wm_inst: u8,
    pub wm_type: u8,
}

/// `struct pp_smu_wm_range_sets`.
#[derive(Default, Clone, Copy, Debug)]
pub struct pp_smu_wm_range_sets {
    pub num_reader_wm_sets: u32,
    pub reader_wm_sets: [pp_smu_wm_set_range; MAX_WATERMARK_SETS],
    pub num_writer_wm_sets: u32,
    pub writer_wm_sets: [pp_smu_wm_set_range; MAX_WATERMARK_SETS],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum rn_set_wm_ranges_fn {
    pp_rn_set_wm_ranges,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum rn_get_dpm_clock_table_fn {
    pp_rn_get_dpm_clock_table,
}

/// `struct pp_smu_funcs_rn`.
#[derive(Default, Clone, Copy, Debug)]
pub struct pp_smu_funcs_rn {
    pub set_wm_ranges: Option<rn_set_wm_ranges_fn>,
    pub get_dpm_clock_table: Option<rn_get_dpm_clock_table_fn>,
}

/// `struct pp_smu_funcs`: `ctx.ver` and the Renoir member of the union of
/// per-family tables.
#[derive(Default, Clone, Copy, Debug)]
pub struct pp_smu_funcs {
    pub ver: pp_smu_ver,
    pub rn_funcs: pp_smu_funcs_rn,
}

/// `pp_rn_get_dpm_clock_table` (amdgpu_dm_pp_smu.c).
pub fn pp_rn_get_dpm_clock_table(adev: &amdgpu_device, clock_table: &mut dpm_clocks) -> pp_smu_status {
    let ret = crate::amdgpu_smu::amdgpu_dpm_get_dpm_clock_table(adev, clock_table);
    if ret == -95 {
        pp_smu_status::PP_SMU_RESULT_UNSUPPORTED
    } else if ret != 0 {
        pp_smu_status::PP_SMU_RESULT_FAIL
    } else {
        pp_smu_status::PP_SMU_RESULT_OK
    }
}

/// `pp_rn_set_wm_ranges` (amdgpu_dm_pp_smu.c): hand DC's watermark clock
/// ranges to the SMU. The SMU half (`smu_set_watermarks_table`, a
/// driver-to-SMU table transfer) is not ported; while power play is down
/// the SMU declines it anyway.
pub fn pp_rn_set_wm_ranges(adev: &amdgpu_device, ranges: &pp_smu_wm_range_sets) -> pp_smu_status {
    let _ = ranges;
    if adev.smu.pm_enabled && adev.pm.dpm_enabled {
        crate::linux::Line::new().s("pp_rn_set_wm_ranges: smu_set_watermarks_table not ported").say();
    }
    pp_smu_status::PP_SMU_RESULT_OK
}

/// `dm_pp_get_funcs` (amdgpu_dm_pp_smu.c), the DCN 2.1 case.
pub fn dm_pp_get_funcs(dce_version: dce_version, funcs: &mut pp_smu_funcs) {
    match dce_version {
        dce_version::DCN_VERSION_2_1 => {
            funcs.ver = pp_smu_ver::PP_SMU_VER_RN;
            funcs.rn_funcs.set_wm_ranges = Some(rn_set_wm_ranges_fn::pp_rn_set_wm_ranges);
            funcs.rn_funcs.get_dpm_clock_table = Some(rn_get_dpm_clock_table_fn::pp_rn_get_dpm_clock_table);
        }
        _ => {}
    }
}
