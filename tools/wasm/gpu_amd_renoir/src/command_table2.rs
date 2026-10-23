//! display/dc/bios/command_table2.c + command_table_helper2.c (Linux
//! 6.18.55): which VBIOS command-table executor each `cmd_tbl` entry binds,
//! chosen by the table's content revision. The executors run with the hw
//! sequencer (stage D); here each binding is an identity enum, as with the
//! other DC vtables.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::atom::amdgpu_atom_parse_cmd_header;
use crate::dc_types::dce_version;
use crate::regs::{
    BP2_CMD_DIG1TRANSMITTERCONTROL, BP2_CMD_DIGXENCODERCONTROL, BP2_CMD_ENABLECRTC, BP2_CMD_ENABLEDISPPOWERGATING,
    BP2_CMD_EXTERNALENCODERCONTROL, BP2_CMD_SETCRTC_USINGDTDTIMING, BP2_CMD_SETDCECLOCK, BP2_CMD_SETPIXELCLOCK,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum dig_encoder_control_fn {
    encoder_control_digx_v1_5,
    encoder_control_fallback,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum transmitter_control_fn {
    transmitter_control_v1_6,
    transmitter_control_v1_7,
    transmitter_control_fallback,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum set_pixel_clock_fn {
    set_pixel_clock_v7,
    set_pixel_clock_fallback,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum set_crtc_timing_fn {
    set_crtc_using_dtd_timing_v3,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum enable_crtc_fn {
    enable_crtc_v1,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum external_encoder_control_fn {
    external_encoder_control_v3,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum enable_disp_power_gating_fn {
    enable_disp_power_gating_v2_1,
    enable_disp_power_gating_fallback,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum set_dce_clock_fn {
    set_dce_clock_v2_1,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum get_smu_clock_info_fn {
    get_smu_clock_info_v3_1,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum enable_lvtma_control_fn {
    enable_lvtma_control,
}

/// `struct cmd_tbl` (command_table2.h): the entries the atomfirmware parser
/// binds. The remaining members are the pre-atomfirmware tables and stay
/// NULL on this path.
#[derive(Default, Clone, Copy, Debug)]
pub struct cmd_tbl {
    pub dig_encoder_control: Option<dig_encoder_control_fn>,
    pub transmitter_control: Option<transmitter_control_fn>,
    pub set_pixel_clock: Option<set_pixel_clock_fn>,
    pub set_crtc_timing: Option<set_crtc_timing_fn>,
    pub enable_crtc: Option<enable_crtc_fn>,
    pub external_encoder_control: Option<external_encoder_control_fn>,
    pub enable_disp_power_gating: Option<enable_disp_power_gating_fn>,
    pub set_dce_clock: Option<set_dce_clock_fn>,
    pub get_smu_clock_info: Option<get_smu_clock_info_fn>,
    pub enable_lvtma_control: Option<enable_lvtma_control_fn>,
}

/// `BIOS_CMD_TABLE_REVISION(fname, frev, crev)`: (frev, crev) of command
/// table `index`, None if it is absent.
fn bios_cmd_table_revision(adev: &amdgpu_device, index: u32) -> Option<(u8, u8)> {
    amdgpu_atom_parse_cmd_header(adev.mode_info.atom_context.as_ref()?, index)
}

/// `bios_cmd_table_para_revision`: the content revision, 0 if absent.
fn bios_cmd_table_para_revision(adev: &amdgpu_device, index: u32) -> u32 {
    match bios_cmd_table_revision(adev, index) {
        Some((_, crev)) => crev as u32,
        None => 0,
    }
}

/// `init_dig_encoder_control`.
fn init_dig_encoder_control(adev: &amdgpu_device, t: &mut cmd_tbl) {
    t.dig_encoder_control = Some(match bios_cmd_table_para_revision(adev, BP2_CMD_DIGXENCODERCONTROL) {
        5 => dig_encoder_control_fn::encoder_control_digx_v1_5,
        _ => dig_encoder_control_fn::encoder_control_fallback,
    });
}

/// `init_transmitter_control`. Linux breaks to the debugger when the table
/// is missing on DCN <= 2.0; Renoir is 2.1.
fn init_transmitter_control(adev: &amdgpu_device, t: &mut cmd_tbl) {
    let crev = bios_cmd_table_revision(adev, BP2_CMD_DIG1TRANSMITTERCONTROL).map_or(0, |(_, c)| c);
    t.transmitter_control = Some(match crev {
        6 => transmitter_control_fn::transmitter_control_v1_6,
        7 => transmitter_control_fn::transmitter_control_v1_7,
        _ => transmitter_control_fn::transmitter_control_fallback,
    });
}

/// `init_set_pixel_clock`.
fn init_set_pixel_clock(adev: &amdgpu_device, t: &mut cmd_tbl) {
    t.set_pixel_clock = Some(match bios_cmd_table_para_revision(adev, BP2_CMD_SETPIXELCLOCK) {
        7 => set_pixel_clock_fn::set_pixel_clock_v7,
        _ => set_pixel_clock_fn::set_pixel_clock_fallback,
    });
}

/// `init_set_crtc_timing`.
fn init_set_crtc_timing(adev: &amdgpu_device, t: &mut cmd_tbl) {
    t.set_crtc_timing = match bios_cmd_table_para_revision(adev, BP2_CMD_SETCRTC_USINGDTDTIMING) {
        3 => Some(set_crtc_timing_fn::set_crtc_using_dtd_timing_v3),
        _ => None,
    };
}

/// `init_enable_crtc`.
fn init_enable_crtc(adev: &amdgpu_device, t: &mut cmd_tbl) {
    t.enable_crtc = match bios_cmd_table_para_revision(adev, BP2_CMD_ENABLECRTC) {
        1 => Some(enable_crtc_fn::enable_crtc_v1),
        _ => None,
    };
}

/// `init_external_encoder_control`.
fn init_external_encoder_control(adev: &amdgpu_device, t: &mut cmd_tbl) {
    t.external_encoder_control = match bios_cmd_table_para_revision(adev, BP2_CMD_EXTERNALENCODERCONTROL) {
        3 => Some(external_encoder_control_fn::external_encoder_control_v3),
        _ => None,
    };
}

/// `init_enable_disp_power_gating`.
fn init_enable_disp_power_gating(adev: &amdgpu_device, t: &mut cmd_tbl) {
    t.enable_disp_power_gating = Some(match bios_cmd_table_para_revision(adev, BP2_CMD_ENABLEDISPPOWERGATING) {
        1 => enable_disp_power_gating_fn::enable_disp_power_gating_v2_1,
        _ => enable_disp_power_gating_fn::enable_disp_power_gating_fallback,
    });
}

/// `init_set_dce_clock`.
fn init_set_dce_clock(adev: &amdgpu_device, t: &mut cmd_tbl) {
    t.set_dce_clock = match bios_cmd_table_para_revision(adev, BP2_CMD_SETDCECLOCK) {
        1 => Some(set_dce_clock_fn::set_dce_clock_v2_1),
        _ => None,
    };
}

/// `init_get_smu_clock_info`.
fn init_get_smu_clock_info(t: &mut cmd_tbl) {
    t.get_smu_clock_info = Some(get_smu_clock_info_fn::get_smu_clock_info_v3_1);
}

/// `init_enable_lvtma_control`.
fn init_enable_lvtma_control(t: &mut cmd_tbl) {
    t.enable_lvtma_control = Some(enable_lvtma_control_fn::enable_lvtma_control);
}

/// `dal_firmware_parser_init_cmd_tbl`.
pub fn dal_firmware_parser_init_cmd_tbl(adev: &amdgpu_device, t: &mut cmd_tbl) {
    init_dig_encoder_control(adev, t);
    init_transmitter_control(adev, t);
    init_set_pixel_clock(adev, t);

    init_set_crtc_timing(adev, t);

    init_enable_crtc(adev, t);

    init_external_encoder_control(adev, t);
    init_enable_disp_power_gating(adev, t);
    init_set_dce_clock(adev, t);
    init_get_smu_clock_info(t);

    init_enable_lvtma_control(t);
}

/// Which `command_table_helper` table `dal_bios_parser_init_cmd_tbl_helper2`
/// binds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum command_table_helper_id {
    dal_cmd_tbl_helper_dce112_get_table2,
}

/// `dal_bios_parser_init_cmd_tbl_helper2` (command_table_helper2.c): every
/// DCN binds the dce112 helper table.
pub fn dal_bios_parser_init_cmd_tbl_helper2(h: &mut Option<command_table_helper_id>, dce: dce_version) -> bool {
    match dce {
        dce_version::DCN_VERSION_2_1 => {
            *h = Some(command_table_helper_id::dal_cmd_tbl_helper_dce112_get_table2);
            true
        }
        _ => false,
    }
}
