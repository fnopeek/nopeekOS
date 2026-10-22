//! display/dc/dc_types.h, dc/dc.h, include/dal_types.h (Linux 6.18.55): the
//! self-contained core types of Display Core — the ones `dc_create` touches
//! that do not pull in a sub-block (bios, link, dml). The nested caps structs
//! (dc_caps → dc_plane_cap / dc_color_caps / …), dc_context and struct dc come
//! with the object graph (docs/plan/GPU_AMD_DC.md stage A).
#![allow(dead_code)]

/// `enum dce_environment` (dc_types.h). Renoir boots `DCE_ENV_PRODUCTION_DRV`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum dce_environment {
    DCE_ENV_PRODUCTION_DRV = 0,
    /// Emulation on FPGA, "Maximus" — only DC registers may be touched.
    DCE_ENV_FPGA_MAXIMUS,
    /// Emulation on real HW or FPGA, for Diagnostics.
    DCE_ENV_DIAG,
    /// Guest VM: DC HW may exist but is not virtualized; SW support for VDI.
    DCE_ENV_VIRTUAL_HW,
}

/// `enum dce_version` (include/dal_types.h). `resource_parse_asic_id` maps
/// Renoir/Lucienne to `DCN_VERSION_2_1`. The whole ladder is kept so other
/// code can match on it; the discriminants follow the C order (UNKNOWN = -1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum dce_version {
    DCE_VERSION_UNKNOWN = -1,
    DCE_VERSION_6_0,
    DCE_VERSION_6_1,
    DCE_VERSION_6_4,
    DCE_VERSION_8_0,
    DCE_VERSION_8_1,
    DCE_VERSION_8_3,
    DCE_VERSION_10_0,
    DCE_VERSION_11_0,
    DCE_VERSION_11_2,
    DCE_VERSION_11_22,
    DCE_VERSION_12_0,
    DCE_VERSION_12_1,
    DCE_VERSION_MAX,
    DCN_VERSION_1_0,
    DCN_VERSION_1_01,
    DCN_VERSION_2_0,
    DCN_VERSION_2_01,
    DCN_VERSION_2_1,
    DCN_VERSION_3_0,
    DCN_VERSION_3_01,
    DCN_VERSION_3_02,
    DCN_VERSION_3_03,
    DCN_VERSION_3_1,
    DCN_VERSION_3_14,
    DCN_VERSION_3_15,
    DCN_VERSION_3_16,
    DCN_VERSION_3_2,
    DCN_VERSION_3_21,
    DCN_VERSION_3_5,
    DCN_VERSION_3_51,
    DCN_VERSION_3_6,
    DCN_VERSION_4_01,
    DCN_VERSION_MAX,
}

/// `enum dp_protocol_version` (dc.h). `dc_create` sets `max_dp_protocol_version
/// = DP_VERSION_1_4`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum dp_protocol_version {
    DP_VERSION_1_4 = 0,
    DP_VERSION_2_1,
    DP_VERSION_UNKNOWN,
}

/// `struct dmcu_version` (dc.h): DCN 2.1 has no DMCU, so this stays zero.
#[derive(Default, Clone, Copy)]
pub struct dmcu_version {
    pub interface_version: u32,
    pub abm_version: u32,
    pub psr_version: u32,
    pub build_version: u32,
}

/// `struct dc_versions` (dc.h).
#[derive(Default, Clone, Copy)]
pub struct dc_versions {
    /// `dc_ver` = `DC_VER`.
    pub dc_ver: &'static str,
    pub dmcu_version: dmcu_version,
}

/// `#define DC_VER` (dc.h).
pub const DC_VER: &str = "3.2.351";
