//! display/dc/bios/bios_parser2.c + bios_parser_interface.c +
//! bios_parser_helper.c + bios_parser_common.c (Linux 6.18.55): the
//! atomfirmware VBIOS parser of DC, the construct path. The image itself is
//! not stored: every reader takes it as `bios` (Linux' `bp->base.bios`, which
//! points at `adev->bios`), and a table is its offset into that image.
//! `master_data_tbl` is the offset of `atom_master_data_table_v2_1`.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::command_table2::{cmd_tbl, dal_bios_parser_init_cmd_tbl_helper2, dal_firmware_parser_init_cmd_tbl,
    command_table_helper_id};
use crate::dc_types::{dc_bios, dc_firmware_info, dce_version, integrated_info, graphics_object_id,
    object_type, object_enum_id, MAX_NUMBER_OF_EXT_DISPLAY_PATH, NUMBER_OF_DISP_CLK_VOLTAGE,
    NUMBER_OF_UCHAR_FOR_GUID};
use crate::regs::*;
use alloc::boxed::Box;

/// `enum bp_result` (bios_parser_types.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum bp_result {
    BP_RESULT_OK = 0,
    BP_RESULT_BADINPUT,
    BP_RESULT_BADBIOSTABLE,
    BP_RESULT_UNSUPPORTED,
    BP_RESULT_NORECORD,
    BP_RESULT_FAILURE,
}

/// `struct atom_data_revision` (bios_parser_types_internal2.h).
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct atom_data_revision {
    pub major: u32,
    pub minor: u32,
}

/// `struct object_info_table` (bios_parser_types_internal2.h). `tbl` is the
/// anonymous union of the `v1_4` / `v1_5` pointers: one offset, read through
/// the layout `revision` names.
#[derive(Default, Clone, Copy)]
pub struct object_info_table {
    pub revision: atom_data_revision,
    pub tbl: Option<u32>,
}

/// `struct bios_parser` (bios_parser_types_internal2.h). `bios_helper` is
/// unset on the atomfirmware path.
#[derive(Default)]
pub struct bios_parser {
    pub base: dc_bios,
    pub object_info_tbl: object_info_table,
    pub object_info_tbl_offset: u32,
    pub master_data_tbl: u32,
    pub cmd_helper: Option<command_table_helper_id>,
    pub cmd_tbl: cmd_tbl,
    pub remap_device_tags: bool,
}

/// `struct bp_init_data` (bios_parser_types.h).
pub struct bp_init_data<'a> {
    pub bios: &'a [u8],
}

fn u8_at(bios: &[u8], at: u32) -> u8 {
    bios[at as usize]
}

fn u16_at(bios: &[u8], at: u32) -> u16 {
    u16::from_le_bytes([bios[at as usize], bios[at as usize + 1]])
}

fn u32_at(bios: &[u8], at: u32) -> u32 {
    let a = at as usize;
    u32::from_le_bytes([bios[a], bios[a + 1], bios[a + 2], bios[a + 3]])
}

/// `bios_get_image` (bios_parser_helper.c): the offset if `size` bytes from
/// `offset` lie inside the image.
pub fn bios_get_image(bp: &dc_bios, bios: &[u8], offset: u32, size: u32) -> Option<u32> {
    if bios.is_empty() {
        return None;
    }
    if offset > bp.bios_size || size > bp.bios_size - offset {
        return None;
    }
    Some(offset)
}

impl bios_parser {
    /// `GET_IMAGE(type, offset)`.
    fn get_image(&self, bios: &[u8], offset: u32, size: u32) -> Option<u32> {
        bios_get_image(&self.base, bios, offset, size)
    }

    /// `DATA_TABLES(table)`: the u16 at `field` of the master data table.
    fn data_tables(&self, bios: &[u8], field: u32) -> u32 {
        u16_at(bios, self.master_data_tbl + field) as u32
    }
}

/// `get_atom_data_table_revision` (bios_parser2.c).
fn get_atom_data_table_revision(bios: &[u8], atom_data_tbl: Option<u32>) -> atom_data_revision {
    let mut tbl_revision = atom_data_revision::default();
    let Some(at) = atom_data_tbl else { return tbl_revision };
    tbl_revision.major = (u8_at(bios, at + BP2_HDR_FORMAT_REVISION) & 0x3f) as u32;
    tbl_revision.minor = (u8_at(bios, at + BP2_HDR_CONTENT_REVISION) & 0x3f) as u32;
    tbl_revision
}

/// `bios_parser_get_firmware_info` (bios_parser2.c). Linux keeps `result`
/// in a function-local `static`, so a later call starts from the previous
/// outcome; the construct calls it once.
fn bios_parser_get_firmware_info(bp: &bios_parser, bios: &[u8], info: &mut dc_firmware_info) -> bp_result {
    let mut result = bp_result::BP_RESULT_BADBIOSTABLE;

    if bp.data_tables(bios, BP2_DT_firmwareinfo) != 0 {
        let header = bp.get_image(bios, bp.data_tables(bios, BP2_DT_firmwareinfo), BP2_COMMON_HDR_SIZE);
        let revision = get_atom_data_table_revision(bios, header);
        if revision.major == 3 {
            match revision.minor {
                1 => result = get_firmware_info_v3_1(bp, bios, info),
                2 | 3 => result = get_firmware_info_v3_2(bp, bios, info),
                // v3_4 / v3_5 are DCN 3.x VBIOSes.
                _ => {}
            }
        }
    }

    result
}

/// `get_firmware_info_v3_1` (bios_parser2.c).
fn get_firmware_info_v3_1(bp: &bios_parser, bios: &[u8], info: &mut dc_firmware_info) -> bp_result {
    let firmware_info = bp.get_image(bios, bp.data_tables(bios, BP2_DT_firmwareinfo), BP2_FWI31_SIZE);
    let firmware_info32 = bp.get_image(bios, bp.data_tables(bios, BP2_DT_firmwareinfo), BP2_FWI32_SIZE);
    let dce_info = bp.get_image(bios, bp.data_tables(bios, BP2_DT_dce_info), BP2_DCEI41_SIZE);

    let (Some(firmware_info), Some(firmware_info32), Some(dce_info)) = (firmware_info, firmware_info32, dce_info)
    else {
        return bp_result::BP_RESULT_BADBIOSTABLE;
    };

    *info = dc_firmware_info::default();

    // We need to convert from 10KHz units into KHz units
    info.default_memory_clk = u32_at(bios, firmware_info + BP2_FWI31_BOOTUP_MCLK_IN10KHZ) * 10;
    info.default_engine_clk = u32_at(bios, firmware_info + BP2_FWI31_BOOTUP_SCLK_IN10KHZ) * 10;

    info.pll_info.crystal_frequency = u16_at(bios, dce_info + BP2_DCEI41_DCE_REFCLK_10KHZ) as u32 * 10;

    // Hardcode frequency if BIOS gives no DCE Ref Clk
    if info.pll_info.crystal_frequency == 0 {
        info.pll_info.crystal_frequency = 27000;
    }
    info.dp_phy_ref_clk = u16_at(bios, dce_info + BP2_DCEI41_DPPHY_REFCLK_10KHZ) as u32 * 10;
    info.i2c_engine_ref_clk = u16_at(bios, dce_info + BP2_DCEI41_I2C_ENGINE_REFCLK_10KHZ) as u32 * 10;

    // smu_gpu_pll_output_freq = get_smu_clock_info(bp, SMU9_SYSPLL0_ID) * 10
    // runs the getsmuclockinfo command table; Renoir's firmware info is v3.2.

    if u8_at(bios, firmware_info32 + BP2_FWI32_BOARD_I2C_FEATURE_ID) == 0x2 {
        info.oem_i2c_present = true;
        info.oem_i2c_obj_id = u8_at(bios, firmware_info32 + BP2_FWI32_BOARD_I2C_FEATURE_GPIO_ID);
    } else {
        info.oem_i2c_present = false;
    }

    bp_result::BP_RESULT_OK
}

/// `get_firmware_info_v3_2` (bios_parser2.c): Renoir's table.
fn get_firmware_info_v3_2(bp: &bios_parser, bios: &[u8], info: &mut dc_firmware_info) -> bp_result {
    let firmware_info = bp.get_image(bios, bp.data_tables(bios, BP2_DT_firmwareinfo), BP2_FWI32_SIZE);
    let dce_info = bp.get_image(bios, bp.data_tables(bios, BP2_DT_dce_info), BP2_DCEI41_SIZE);

    let (Some(firmware_info), Some(dce_info)) = (firmware_info, dce_info) else {
        return bp_result::BP_RESULT_BADBIOSTABLE;
    };

    *info = dc_firmware_info::default();

    let header = bp.get_image(bios, bp.data_tables(bios, BP2_DT_smu_info), BP2_COMMON_HDR_SIZE);
    let revision = get_atom_data_table_revision(bios, header);

    if revision.minor == 2 {
        // Vega12
        let Some(smu_info_v3_2) = bp.get_image(bios, bp.data_tables(bios, BP2_DT_smu_info), BP2_SMUI32_SIZE) else {
            return bp_result::BP_RESULT_BADBIOSTABLE;
        };
        info.default_engine_clk = u32_at(bios, smu_info_v3_2 + BP2_SMUI32_BOOTUP_DCEFCLK_10KHZ) * 10;
    } else if revision.minor == 3 {
        // Vega20
        let Some(smu_info_v3_3) = bp.get_image(bios, bp.data_tables(bios, BP2_DT_smu_info), BP2_SMUI33_SIZE) else {
            return bp_result::BP_RESULT_BADBIOSTABLE;
        };
        info.default_engine_clk = u32_at(bios, smu_info_v3_3 + BP2_SMUI33_BOOTUP_DCEFCLK_10KHZ) * 10;
    }

    // We need to convert from 10KHz units into KHz units.
    info.default_memory_clk = u32_at(bios, firmware_info + BP2_FWI32_BOOTUP_MCLK_IN10KHZ) * 10;

    info.pll_info.crystal_frequency = u16_at(bios, dce_info + BP2_DCEI41_DCE_REFCLK_10KHZ) as u32 * 10;
    // Hardcode frequency if BIOS gives no DCE Ref Clk
    if info.pll_info.crystal_frequency == 0 {
        if revision.minor == 2 {
            info.pll_info.crystal_frequency = 27000;
        } else if revision.minor == 3 {
            info.pll_info.crystal_frequency = 100000;
        }
    }
    info.dp_phy_ref_clk = u16_at(bios, dce_info + BP2_DCEI41_DPPHY_REFCLK_10KHZ) as u32 * 10;
    info.i2c_engine_ref_clk = u16_at(bios, dce_info + BP2_DCEI41_I2C_ENGINE_REFCLK_10KHZ) as u32 * 10;

    // Get GPU PLL VCO Clock: only for an smu_info of minor 2 or 3 (the
    // getsmuclockinfo command table); Renoir's smu_info is v3.1.

    if u8_at(bios, firmware_info + BP2_FWI32_BOARD_I2C_FEATURE_ID) == 0x2 {
        info.oem_i2c_present = true;
        info.oem_i2c_obj_id = u8_at(bios, firmware_info + BP2_FWI32_BOARD_I2C_FEATURE_GPIO_ID);
    } else {
        info.oem_i2c_present = false;
    }

    bp_result::BP_RESULT_OK
}

/// `object_type_from_bios_object_id` (bios_parser_common.c).
fn object_type_from_bios_object_id(bios_object_id: u32) -> object_type {
    match (bios_object_id & BP2_OBJECT_TYPE_MASK) >> BP2_OBJECT_TYPE_SHIFT {
        BP2_GRAPH_OBJECT_TYPE_GPU => object_type::OBJECT_TYPE_GPU,
        BP2_GRAPH_OBJECT_TYPE_ENCODER => object_type::OBJECT_TYPE_ENCODER,
        BP2_GRAPH_OBJECT_TYPE_CONNECTOR => object_type::OBJECT_TYPE_CONNECTOR,
        BP2_GRAPH_OBJECT_TYPE_ROUTER => object_type::OBJECT_TYPE_ROUTER,
        BP2_GRAPH_OBJECT_TYPE_GENERIC => object_type::OBJECT_TYPE_GENERIC,
        _ => object_type::OBJECT_TYPE_UNKNOWN,
    }
}

/// `enum_id_from_bios_object_id` (bios_parser_common.c).
fn enum_id_from_bios_object_id(bios_object_id: u32) -> object_enum_id {
    match (bios_object_id & BP2_ENUM_ID_MASK) >> BP2_ENUM_ID_SHIFT {
        BP2_GRAPH_OBJECT_ENUM_ID1 => object_enum_id::ENUM_ID_1,
        BP2_GRAPH_OBJECT_ENUM_ID2 => object_enum_id::ENUM_ID_2,
        BP2_GRAPH_OBJECT_ENUM_ID3 => object_enum_id::ENUM_ID_3,
        BP2_GRAPH_OBJECT_ENUM_ID4 => object_enum_id::ENUM_ID_4,
        BP2_GRAPH_OBJECT_ENUM_ID5 => object_enum_id::ENUM_ID_5,
        BP2_GRAPH_OBJECT_ENUM_ID6 => object_enum_id::ENUM_ID_6,
        BP2_GRAPH_OBJECT_ENUM_ID7 => object_enum_id::ENUM_ID_7,
        _ => object_enum_id::ENUM_ID_UNKNOWN,
    }
}

/// `gpu_id_from_bios_object_id` (bios_parser_common.c).
fn gpu_id_from_bios_object_id(bios_object_id: u32) -> u32 {
    (bios_object_id & BP2_OBJECT_ID_MASK) >> BP2_OBJECT_ID_SHIFT
}

/// `encoder_id_from_bios_object_id` (bios_parser_common.c): `enum
/// encoder_id` (grph_object_id.h) as its value.
fn encoder_id_from_bios_object_id(bios_object_id: u32) -> u32 {
    match gpu_id_from_bios_object_id(bios_object_id) {
        BP2_ENCODER_OBJECT_ID_INTERNAL_LVDS => 1,
        BP2_ENCODER_OBJECT_ID_INTERNAL_TMDS1 => 2,
        BP2_ENCODER_OBJECT_ID_INTERNAL_TMDS2 => 3,
        BP2_ENCODER_OBJECT_ID_INTERNAL_DAC1 => 4,
        BP2_ENCODER_OBJECT_ID_INTERNAL_DAC2 => 5,
        BP2_ENCODER_OBJECT_ID_INTERNAL_LVTM1 => 6,
        BP2_ENCODER_OBJECT_ID_HDMI_INTERNAL => 7,
        BP2_ENCODER_OBJECT_ID_INTERNAL_KLDSCP_TMDS1 => 8,
        BP2_ENCODER_OBJECT_ID_INTERNAL_KLDSCP_DAC1 => 9,
        BP2_ENCODER_OBJECT_ID_INTERNAL_KLDSCP_DAC2 => 10,
        BP2_ENCODER_OBJECT_ID_MVPU_FPGA => 11,
        BP2_ENCODER_OBJECT_ID_INTERNAL_DDI => 12,
        BP2_ENCODER_OBJECT_ID_INTERNAL_UNIPHY => 13,
        BP2_ENCODER_OBJECT_ID_INTERNAL_KLDSCP_LVTMA => 14,
        BP2_ENCODER_OBJECT_ID_INTERNAL_UNIPHY1 => 15,
        BP2_ENCODER_OBJECT_ID_INTERNAL_UNIPHY2 => 16,
        BP2_ENCODER_OBJECT_ID_ALMOND => 17,
        BP2_ENCODER_OBJECT_ID_TRAVIS => 18,
        BP2_ENCODER_OBJECT_ID_INTERNAL_UNIPHY3 => 20,
        _ => 0,
    }
}

/// `connector_id_from_bios_object_id` (bios_parser_common.c): `enum
/// connector_id` (grph_object_id.h) as its value.
fn connector_id_from_bios_object_id(bios_object_id: u32) -> u32 {
    match gpu_id_from_bios_object_id(bios_object_id) {
        BP2_CONNECTOR_OBJECT_ID_SINGLE_LINK_DVI_I => 1,
        BP2_CONNECTOR_OBJECT_ID_DUAL_LINK_DVI_I => 2,
        BP2_CONNECTOR_OBJECT_ID_SINGLE_LINK_DVI_D => 3,
        BP2_CONNECTOR_OBJECT_ID_DUAL_LINK_DVI_D => 4,
        BP2_CONNECTOR_OBJECT_ID_VGA => 5,
        BP2_CONNECTOR_OBJECT_ID_HDMI_TYPE_A => 12,
        BP2_CONNECTOR_OBJECT_ID_LVDS => 14,
        BP2_CONNECTOR_OBJECT_ID_PCIE_CONNECTOR => 16,
        BP2_CONNECTOR_OBJECT_ID_HARDCODE_DVI => 18,
        BP2_CONNECTOR_OBJECT_ID_DISPLAYPORT => 19,
        BP2_CONNECTOR_OBJECT_ID_eDP => 20,
        BP2_CONNECTOR_OBJECT_ID_MXM => 21,
        BP2_CONNECTOR_OBJECT_ID_USBC => 24,
        _ => 0,
    }
}

/// `generic_id_from_bios_object_id` (bios_parser_common.c): `enum
/// generic_id` as its value.
fn generic_id_from_bios_object_id(bios_object_id: u32) -> u32 {
    match gpu_id_from_bios_object_id(bios_object_id) {
        BP2_GENERIC_OBJECT_ID_MXM_OPM => 1,
        BP2_GENERIC_OBJECT_ID_GLSYNC => 2,
        BP2_GENERIC_OBJECT_ID_STEREO_PIN => 3,
        _ => 0,
    }
}

/// `id_from_bios_object_id` (bios_parser_common.c).
fn id_from_bios_object_id(r#type: object_type, bios_object_id: u32) -> u32 {
    match r#type {
        object_type::OBJECT_TYPE_GPU => gpu_id_from_bios_object_id(bios_object_id),
        object_type::OBJECT_TYPE_ENCODER => encoder_id_from_bios_object_id(bios_object_id),
        object_type::OBJECT_TYPE_CONNECTOR => connector_id_from_bios_object_id(bios_object_id),
        object_type::OBJECT_TYPE_GENERIC => generic_id_from_bios_object_id(bios_object_id),
        _ => 0,
    }
}

/// `object_id_from_bios_object_id` (bios_parser_common.c).
pub fn object_id_from_bios_object_id(bios_object_id: u32) -> graphics_object_id {
    let r#type = object_type_from_bios_object_id(bios_object_id);
    if r#type == object_type::OBJECT_TYPE_UNKNOWN {
        return graphics_object_id::default();
    }

    let enum_id = enum_id_from_bios_object_id(bios_object_id);
    if enum_id == object_enum_id::ENUM_ID_UNKNOWN {
        return graphics_object_id::default();
    }

    graphics_object_id::init(id_from_bios_object_id(r#type, bios_object_id), enum_id, r#type)
}

/// One `atom_hdmi_retimer_redriver_set` into `slv_addr`/`reg_num`/settings
/// (get_integrated_info_v11 does this four times, dp0..dp3).
fn read_retimer_set(
    bios: &[u8],
    set: u32,
    slv_addr: &mut u8,
    reg_num: &mut u8,
    reg_settings: &mut [crate::dc_types::i2c_reg_info],
    reg_num_6g: &mut u8,
    reg_settings_6g: &mut [crate::dc_types::i2c_reg_info],
) {
    *slv_addr = u8_at(bios, set + BP2_RETIMER_HDMISLVADDR);
    *reg_num = u8_at(bios, set + BP2_RETIMER_HDMIREGNUM).min(reg_settings.len() as u8);
    for i in 0..*reg_num as u32 {
        let r = set + BP2_RETIMER_HDMIREGSETTING + i * BP2_I2CREG_SIZE;
        reg_settings[i as usize].i2c_reg_index = u8_at(bios, r + BP2_I2CREG_UCI2CREGINDEX);
        reg_settings[i as usize].i2c_reg_val = u8_at(bios, r + BP2_I2CREG_UCI2CREGVAL);
    }
    *reg_num_6g = u8_at(bios, set + BP2_RETIMER_HDMI6GREGNUM).min(reg_settings_6g.len() as u8);
    for i in 0..*reg_num_6g as u32 {
        let r = set + BP2_RETIMER_HDMI6GHZREGSETTING + i * BP2_I2CREG_SIZE;
        reg_settings_6g[i as usize].i2c_reg_index = u8_at(bios, r + BP2_I2CREG_UCI2CREGINDEX);
        reg_settings_6g[i as usize].i2c_reg_val = u8_at(bios, r + BP2_I2CREG_UCI2CREGVAL);
    }
}

/// `get_integrated_info_v11` (bios_parser2.c): read for a v1.11 and a v1.12
/// table alike, through the v1.11 layout.
fn get_integrated_info_v11(bp: &bios_parser, bios: &[u8], info: &mut integrated_info) -> bp_result {
    let Some(info_v11) = bp.get_image(bios, bp.data_tables(bios, BP2_DT_integratedsysteminfo), BP2_IGP11_SIZE)
    else {
        return bp_result::BP_RESULT_BADBIOSTABLE;
    };

    info.gpu_cap_info = u32_at(bios, info_v11 + BP2_IGP11_GPUCAPINFO);
    info.system_config = u32_at(bios, info_v11 + BP2_IGP11_SYSTEM_CONFIG);
    info.cpu_cap_info = u32_at(bios, info_v11 + BP2_IGP11_CPUCAPINFO);
    info.memory_type = u8_at(bios, info_v11 + BP2_IGP11_MEMORYTYPE);
    info.ma_channel_number = u8_at(bios, info_v11 + BP2_IGP11_UMACHANNELNUMBER);
    info.lvds_ss_percentage = u16_at(bios, info_v11 + BP2_IGP11_LVDS_SS_PERCENTAGE) as u32;
    info.dp_ss_control = u8_at(bios, info_v11 + BP2_IGP11_RESERVED1) as u32;
    info.lvds_sspread_rate_in_10hz = u16_at(bios, info_v11 + BP2_IGP11_LVDS_SS_RATE_10HZ) as u32;
    info.hdmi_ss_percentage = u16_at(bios, info_v11 + BP2_IGP11_HDMI_SS_PERCENTAGE) as u32;
    info.hdmi_sspread_rate_in_10hz = u16_at(bios, info_v11 + BP2_IGP11_HDMI_SS_RATE_10HZ) as u32;
    info.dvi_ss_percentage = u16_at(bios, info_v11 + BP2_IGP11_DVI_SS_PERCENTAGE) as u32;
    info.dvi_sspread_rate_in_10_hz = u16_at(bios, info_v11 + BP2_IGP11_DVI_SS_RATE_10HZ) as u32;
    info.lvds_misc = u16_at(bios, info_v11 + BP2_IGP11_LVDS_MISC) as u32;
    for i in 0..NUMBER_OF_UCHAR_FOR_GUID {
        info.ext_disp_conn_info.gu_id[i] = u8_at(bios, info_v11 + BP2_IGP11_EXTDISPCONNINFO_GUID + i as u32);
    }

    for i in 0..MAX_NUMBER_OF_EXT_DISPLAY_PATH {
        let p = info_v11 + BP2_IGP11_EXTDISPCONNINFO_PATH + i as u32 * BP2_EXTPATH_SIZE;
        let path = &mut info.ext_disp_conn_info.path[i];
        path.device_connector_id = object_id_from_bios_object_id(u16_at(bios, p + BP2_EXTPATH_CONNECTOROBJID) as u32);
        path.ext_encoder_obj_id = object_id_from_bios_object_id(u16_at(bios, p + BP2_EXTPATH_EXT_ENCODER_OBJID) as u32);
        path.device_tag = u16_at(bios, p + BP2_EXTPATH_DEVICE_TAG) as u32;
        path.device_acpi_enum = u16_at(bios, p + BP2_EXTPATH_DEVICE_ACPI_ENUM) as u32;
        path.ext_aux_ddc_lut_index = u8_at(bios, p + BP2_EXTPATH_AUXDDCLUT_INDEX);
        path.ext_hpd_pin_lut_index = u8_at(bios, p + BP2_EXTPATH_HPDLUT_INDEX);
        path.channel_mapping = u8_at(bios, p + BP2_EXTPATH_CHANNELMAPPING);
        path.caps = u16_at(bios, p + BP2_EXTPATH_CAPS);
    }
    info.ext_disp_conn_info.checksum = u8_at(bios, info_v11 + BP2_IGP11_EXTDISPCONNINFO_CHECKSUM);

    read_retimer_set(bios, info_v11 + BP2_IGP11_DP0_RETIMER_SET, &mut info.dp0_ext_hdmi_slv_addr,
        &mut info.dp0_ext_hdmi_reg_num, &mut info.dp0_ext_hdmi_reg_settings,
        &mut info.dp0_ext_hdmi_6g_reg_num, &mut info.dp0_ext_hdmi_6g_reg_settings);
    read_retimer_set(bios, info_v11 + BP2_IGP11_DP1_RETIMER_SET, &mut info.dp1_ext_hdmi_slv_addr,
        &mut info.dp1_ext_hdmi_reg_num, &mut info.dp1_ext_hdmi_reg_settings,
        &mut info.dp1_ext_hdmi_6g_reg_num, &mut info.dp1_ext_hdmi_6g_reg_settings);
    read_retimer_set(bios, info_v11 + BP2_IGP11_DP2_RETIMER_SET, &mut info.dp2_ext_hdmi_slv_addr,
        &mut info.dp2_ext_hdmi_reg_num, &mut info.dp2_ext_hdmi_reg_settings,
        &mut info.dp2_ext_hdmi_6g_reg_num, &mut info.dp2_ext_hdmi_6g_reg_settings);
    read_retimer_set(bios, info_v11 + BP2_IGP11_DP3_RETIMER_SET, &mut info.dp3_ext_hdmi_slv_addr,
        &mut info.dp3_ext_hdmi_reg_num, &mut info.dp3_ext_hdmi_reg_settings,
        &mut info.dp3_ext_hdmi_6g_reg_num, &mut info.dp3_ext_hdmi_6g_reg_settings);

    bp_result::BP_RESULT_OK
}

/// `construct_integrated_info` (bios_parser2.c). The v2.1/v2.2 readers are
/// DCN 3.x tables. After the reader Linux logs each path and, with
/// `config.force_bios_fixed_vs`, forces the fixed-VS caps; the flag is 0 on
/// Renoir, so the voltage sort is what changes `info`.
fn construct_integrated_info(bp: &bios_parser, bios: &[u8], info: &mut integrated_info) -> bp_result {
    let mut result = bp_result::BP_RESULT_BADBIOSTABLE;

    if bp.data_tables(bios, BP2_DT_integratedsysteminfo) != 0 {
        let header = bp.get_image(bios, bp.data_tables(bios, BP2_DT_integratedsysteminfo), BP2_COMMON_HDR_SIZE);
        let revision = get_atom_data_table_revision(bios, header);

        match (revision.major, revision.minor) {
            (1, 11) | (1, 12) => result = get_integrated_info_v11(bp, bios, info),
            _ => return result,
        }
    }

    if result != bp_result::BP_RESULT_OK {
        return result;
    }

    // Sort voltage table from low to high
    for i in 1..NUMBER_OF_DISP_CLK_VOLTAGE {
        let mut j = i;
        while j > 0 {
            if info.disp_clk_voltage[j].max_supported_clk < info.disp_clk_voltage[j - 1].max_supported_clk {
                info.disp_clk_voltage.swap(j - 1, j);
            }
            j -= 1;
        }
    }

    result
}

/// `bios_parser_create_integrated_info` (bios_parser2.c).
fn bios_parser_create_integrated_info(bp: &bios_parser, bios: &[u8]) -> Option<Box<integrated_info>> {
    let mut info = Box::new(integrated_info::default());
    if construct_integrated_info(bp, bios, &mut info) == bp_result::BP_RESULT_OK {
        return Some(info);
    }
    None
}

/// `bios_parser_get_vram_info` (bios_parser2.c). Renoir carries neither an
/// umc_info nor a vram_info table, so the result is BADBIOSTABLE and
/// `vram_info` stays zero; the umc v4.0 / vram v2.3–v3.0 readers belong to
/// dGPUs.
fn bios_parser_get_vram_info(bp: &bios_parser, bios: &[u8]) -> bp_result {
    debug_assert!(bp.data_tables(bios, BP2_DT_umc_info) == 0 && bp.data_tables(bios, BP2_DT_vram_info) == 0);
    bp_result::BP_RESULT_BADBIOSTABLE
}

/// `bios_parser_get_soc_bb_info` (bios_parser2.c): only a dce_info of v4.4
/// or v4.5 carries a SoC bounding box; Renoir's is v4.3, so `bb_info` stays
/// zero.
fn bios_parser_get_soc_bb_info(bp: &bios_parser, bios: &[u8]) -> bp_result {
    if bp.data_tables(bios, BP2_DT_dce_info) == 0 {
        return bp_result::BP_RESULT_UNSUPPORTED;
    }
    let header = bp.get_image(bios, bp.data_tables(bios, BP2_DT_dce_info), BP2_COMMON_HDR_SIZE);
    let tbl_revision = get_atom_data_table_revision(bios, header);
    debug_assert!(!(tbl_revision.major == 4 && (tbl_revision.minor == 4 || tbl_revision.minor == 5)));
    bp_result::BP_RESULT_UNSUPPORTED
}

/// `bios_parser2_construct` (bios_parser2.c).
fn bios_parser2_construct(
    adev: &amdgpu_device,
    bp: &mut bios_parser,
    init: &bp_init_data,
    dce_version: dce_version,
) -> bool {
    let bios = init.bios;
    if bios.is_empty() {
        return false;
    }

    bp.base.bios_size = u8_at(bios, BP2_OFFSET_TO_ATOM_ROM_IMAGE_SIZE) as u32 * BP2_BIOS_IMAGE_SIZE_UNIT;

    let Some(rom_header_offset) = bp.get_image(bios, BP2_OFFSET_TO_ATOM_ROM_HEADER_POINTER, 2) else {
        return false;
    };

    let Some(rom_header) = bp.get_image(bios, u16_at(bios, rom_header_offset) as u32, BP2_ROM_HEADER_SIZE) else {
        return false;
    };

    let tbl_rev = get_atom_data_table_revision(bios, Some(rom_header));
    if !(tbl_rev.major >= 2 && tbl_rev.minor >= 2) {
        return false;
    }

    let Some(master_data_tbl) = bp.get_image(
        bios,
        u16_at(bios, rom_header + BP2_ROM_HEADER_MASTERDATATABLE) as u32,
        BP2_MASTER_DATA_TABLE_SIZE,
    ) else {
        return false;
    };
    bp.master_data_tbl = master_data_tbl;

    bp.object_info_tbl_offset = bp.data_tables(bios, BP2_DT_displayobjectinfo);
    if bp.object_info_tbl_offset == 0 {
        return false;
    }

    let Some(object_info_tbl) = bp.get_image(bios, bp.object_info_tbl_offset, BP2_OBJINFO_V1_4_SIZE) else {
        return false;
    };

    bp.object_info_tbl.revision = get_atom_data_table_revision(bios, Some(object_info_tbl));

    match (bp.object_info_tbl.revision.major, bp.object_info_tbl.revision.minor) {
        (1, 4) => {
            let Some(tbl_v1_4) = bp.get_image(bios, bp.object_info_tbl_offset, BP2_OBJINFO_V1_4_SIZE) else {
                return false;
            };
            bp.object_info_tbl.tbl = Some(tbl_v1_4);
        }
        (1, 5) => {
            let Some(tbl_v1_5) = bp.get_image(bios, bp.object_info_tbl_offset, BP2_OBJINFO_V1_5_SIZE) else {
                return false;
            };
            bp.object_info_tbl.tbl = Some(tbl_v1_5);
        }
        _ => return false,
    }

    dal_firmware_parser_init_cmd_tbl(adev, &mut bp.cmd_tbl);
    dal_bios_parser_init_cmd_tbl_helper2(&mut bp.cmd_helper, dce_version);

    bp.base.integrated_info = bios_parser_create_integrated_info(bp, bios);
    let mut fw_info = dc_firmware_info::default();
    bp.base.fw_info_valid = bios_parser_get_firmware_info(bp, bios, &mut fw_info) == bp_result::BP_RESULT_OK;
    bp.base.fw_info = fw_info;
    bios_parser_get_vram_info(bp, bios);
    bios_parser_get_soc_bb_info(bp, bios);
    true
}

/// `firmware_parser_create` (bios_parser2.c).
fn firmware_parser_create(adev: &amdgpu_device, init: &bp_init_data, dce_version: dce_version) -> Option<Box<bios_parser>> {
    let mut bp = Box::new(bios_parser::default());
    if bios_parser2_construct(adev, &mut bp, init, dce_version) {
        return Some(bp);
    }
    None
}

/// `dal_bios_parser_create` (bios_parser_interface.c). The fallback to the
/// pre-atomfirmware `bios_parser_create` is for ASICs before Vega.
pub fn dal_bios_parser_create(adev: &amdgpu_device, init: &bp_init_data, dce_version: dce_version) -> Option<Box<bios_parser>> {
    firmware_parser_create(adev, init, dce_version)
}
