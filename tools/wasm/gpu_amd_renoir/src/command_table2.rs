//! display/dc/bios/command_table2.c + command_table_helper2.c (Linux
//! 6.18.55): which VBIOS command-table executor each `cmd_tbl` entry binds,
//! chosen by the table's content revision, and the executors the hw
//! sequencer has reached. Each binding is an identity enum, as with the
//! other DC vtables; with `dmub_command_table` the executors hand the
//! table's parameters to the DMCUB instead of running the ATOM table.
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

/// `struct bp_transmitter_control` (bios_parser_types.h), the members the
/// v1.6 executor reads.
#[derive(Default, Clone, Copy)]
pub struct bp_transmitter_control {
    pub action: u32,
    pub engine_id: u32,
    pub transmitter: u32,
    /// `enum clock_source_id`.
    pub pll_id: u32,
    /// `enum dc_color_depth`.
    pub color_depth: u32,
    pub lane_select: u32,
    pub connector_obj_id: crate::dc_types::graphics_object_id,
    pub hpd_sel: u32,
    pub lanes_number: u32,
    pub pixel_clock: u32,
    pub signal: u32,
    pub lane_settings: u32,
    pub coherent: bool,
}

/// `phy_id_to_atom` (command_table_helper.c).
fn phy_id_to_atom(t: u32) -> u8 {
    use crate::regs::*;
    (match t {
        LF_TRANSMITTER_UNIPHY_A => ATOM_PHY_ID_UNIPHYA,
        LF_TRANSMITTER_UNIPHY_B => ATOM_PHY_ID_UNIPHYB,
        LF_TRANSMITTER_UNIPHY_C => ATOM_PHY_ID_UNIPHYC,
        LF_TRANSMITTER_UNIPHY_D => ATOM_PHY_ID_UNIPHYD,
        LF_TRANSMITTER_UNIPHY_E => ATOM_PHY_ID_UNIPHYE,
        LF_TRANSMITTER_UNIPHY_F => ATOM_PHY_ID_UNIPHYF,
        LF_TRANSMITTER_UNIPHY_G => ATOM_PHY_ID_UNIPHYG,
        _ => ATOM_PHY_ID_UNIPHYA,
    }) as u8
}

/// `signal_type_to_atom_dig_mode` (command_table_helper2_dce112.c).
fn signal_type_to_atom_dig_mode(s: u32) -> u8 {
    use crate::regs::*;
    (match s {
        LF_SIGNAL_TYPE_DISPLAY_PORT | LF_SIGNAL_TYPE_EDP => ATOM_TRANSMITTER_DIGMODE_V6_DP,
        LF_SIGNAL_TYPE_DVI_SINGLE_LINK | LF_SIGNAL_TYPE_DVI_DUAL_LINK => ATOM_TRANSMITTER_DIGMODE_V6_DVI,
        LF_SIGNAL_TYPE_HDMI_TYPE_A => ATOM_TRANSMITTER_DIGMODE_V6_HDMI,
        LF_SIGNAL_TYPE_DISPLAY_PORT_MST => ATOM_TRANSMITTER_DIGMODE_V6_DP_MST,
        _ => ATOM_TRANSMITTER_DIGMODE_V6_DVI,
    }) as u8
}

/// `hpd_sel_to_atom` (command_table_helper2_dce112.c).
fn hpd_sel_to_atom(id: u32) -> u8 {
    use crate::regs::*;
    (match id {
        LF_HPD_SOURCEID1 => ATOM_TRANSMITTER_V6_HPD1_SEL,
        LF_HPD_SOURCEID2 => ATOM_TRANSMITTER_V6_HPD2_SEL,
        LF_HPD_SOURCEID3 => ATOM_TRANSMITTER_V6_HPD3_SEL,
        LF_HPD_SOURCEID4 => ATOM_TRANSMITTER_V6_HPD4_SEL,
        LF_HPD_SOURCEID5 => ATOM_TRANSMITTER_V6_HPD5_SEL,
        LF_HPD_SOURCEID6 => ATOM_TRANSMITTER_V6_HPD6_SEL,
        _ => 0,
    }) as u8
}

/// `dig_encoder_sel_to_atom` (command_table_helper2_dce112.c): after DCE 8
/// the DIG front end is selected by the link encoder, so no FE here.
fn dig_encoder_sel_to_atom(_id: u32) -> u8 {
    0
}

/// `transmitter_control_dmcub`: the parameters as a DMUB VBIOS command.
fn transmitter_control_dmcub(adev: &amdgpu_device, dc: &mut crate::dc::dc,
                             dig: &[u8; crate::regs::SIZEOF_dig_transmitter_control_parameters_v1_6 as usize]) {
    use crate::regs::*;
    let mut cmd = crate::dmub_srv::dmub_rb_cmd::default();
    cmd.set_header(DMUB_CMD__VBIOS, DMUB_CMD__VBIOS_DIG1_TRANSMITTER_CONTROL,
        SIZEOF_dmub_rb_cmd_dig1_transmitter_control - SIZEOF_dmub_cmd_header);
    cmd.0[DIG1TC_DATA as usize..][..dig.len()].copy_from_slice(dig);
    crate::dc_dmub_srv::dc_wake_and_execute_dmub_cmd(adev, dc, &mut cmd, DM_DMUB_WAIT_TYPE_WAIT);
}

/// `transmitter_control_v1_6`. Executing the ATOM table itself (without
/// `dmub_command_table`) is not ported: DCN 2.1's defaults route it to
/// the DMCUB.
fn transmitter_control_v1_6(adev: &amdgpu_device, dc: &mut crate::dc::dc, cntl: &bp_transmitter_control) -> crate::bios_parser2::bp_result {
    use crate::regs::*;
    let mut param = [0u8; SIZEOF_dig_transmitter_control_parameters_v1_6 as usize];

    param[DIG16_PHYID as usize] = phy_id_to_atom(cntl.transmitter);
    param[DIG16_ACTION as usize] = cntl.action as u8;

    param[DIG16_MODE_LANESET as usize] = if cntl.action == TRANSMITTER_CONTROL_SET_VOLTAGE_AND_PREEMPASIS {
        cntl.lane_settings as u8
    } else {
        signal_type_to_atom_dig_mode(cntl.signal)
    };

    param[DIG16_LANENUM as usize] = cntl.lanes_number as u8;
    param[DIG16_HPDSEL as usize] = hpd_sel_to_atom(cntl.hpd_sel);
    param[DIG16_DIGFE_SEL as usize] = dig_encoder_sel_to_atom(cntl.engine_id);
    param[DIG16_CONNOBJ_ID as usize] = cntl.connector_obj_id.id as u8;
    param[DIG16_SYMCLK_10KHZ as usize..][..4].copy_from_slice(&(cntl.pixel_clock / 10).to_le_bytes());

    if dc.ctx.dmub_srv.is_some() && dc.debug.dmub_command_table {
        transmitter_control_dmcub(adev, dc, &param);
        return crate::bios_parser2::bp_result::BP_RESULT_OK;
    }
    crate::linux::Line::new().s("transmitter_control_v1_6: the ATOM table path is not ported").say();
    crate::bios_parser2::bp_result::BP_RESULT_FAILURE
}

/// `bios_parser_transmitter_control` (bios_parser2.c). This VBIOS binds
/// the v1.6 executor; v1.7 and the fallback are not ported.
pub fn bios_parser_transmitter_control(adev: &amdgpu_device, dc: &mut crate::dc::dc,
                                       cntl: &bp_transmitter_control) -> crate::bios_parser2::bp_result {
    match dc.ctx.dc_bios.cmd_tbl.transmitter_control {
        None => crate::bios_parser2::bp_result::BP_RESULT_FAILURE,
        Some(transmitter_control_fn::transmitter_control_v1_6) => transmitter_control_v1_6(adev, dc, cntl),
        Some(f) => {
            crate::linux::Line::new().s("bios_parser_transmitter_control: executor not ported: ")
                .s(match f {
                    transmitter_control_fn::transmitter_control_v1_7 => "v1_7",
                    _ => "fallback",
                }).say();
            crate::bios_parser2::bp_result::BP_RESULT_FAILURE
        }
    }
}

/// `enable_lvtma_control_dmcub`. Linux leaves `payload_bytes` zero here.
fn enable_lvtma_control_dmcub(adev: &amdgpu_device, dc: &mut crate::dc::dc, uc_pwr_on: u8, pwrseq_instance: u8,
                              bypass_panel_control_wait: u8) {
    use crate::regs::*;
    let mut cmd = crate::dmub_srv::dmub_rb_cmd::default();
    cmd.set_header(DMUB_CMD__VBIOS, DMUB_CMD__VBIOS_LVTMA_CONTROL, 0);
    cmd.0[LVTMA_UC_PWR_ACTION as usize] = uc_pwr_on;
    cmd.0[LVTMA_PWRSEQ_INST as usize] = pwrseq_instance;
    cmd.0[LVTMA_BYPASS_PANEL_CONTROL_WAIT as usize] = bypass_panel_control_wait;
    crate::dc_dmub_srv::dc_wake_and_execute_dmub_cmd(adev, dc, &mut cmd, DM_DMUB_WAIT_TYPE_WAIT);
}

/// `enable_lvtma_control`.
fn enable_lvtma_control(adev: &amdgpu_device, dc: &mut crate::dc::dc, uc_pwr_on: u8, pwrseq_instance: u8,
                        bypass_panel_control_wait: u8) -> crate::bios_parser2::bp_result {
    if dc.ctx.dmub_srv.is_some() && dc.debug.dmub_command_table {
        enable_lvtma_control_dmcub(adev, dc, uc_pwr_on, pwrseq_instance, bypass_panel_control_wait);
        return crate::bios_parser2::bp_result::BP_RESULT_OK;
    }
    crate::bios_parser2::bp_result::BP_RESULT_FAILURE
}

/// `bios_parser_enable_lvtma_control` (bios_parser2.c).
pub fn bios_parser_enable_lvtma_control(adev: &amdgpu_device, dc: &mut crate::dc::dc, uc_pwr_on: u8,
                                        pwrseq_instance: u8, bypass_panel_control_wait: u8) -> crate::bios_parser2::bp_result {
    match dc.ctx.dc_bios.cmd_tbl.enable_lvtma_control {
        None => crate::bios_parser2::bp_result::BP_RESULT_FAILURE,
        Some(enable_lvtma_control_fn::enable_lvtma_control) => {
            enable_lvtma_control(adev, dc, uc_pwr_on, pwrseq_instance, bypass_panel_control_wait)
        }
    }
}

/// `dal_cmd_table_helper_controller_id_to_atom2` (command_table_helper2.c).
fn controller_id_to_atom(id: u32, atom_id: &mut u8) -> bool {
    use crate::regs::*;
    *atom_id = (match id {
        CONTROLLER_ID_D0 => ATOM_CRTC1,
        CONTROLLER_ID_D1 => ATOM_CRTC2,
        CONTROLLER_ID_D2 => ATOM_CRTC3,
        CONTROLLER_ID_D3 => ATOM_CRTC4,
        CONTROLLER_ID_D4 => ATOM_CRTC5,
        CONTROLLER_ID_D5 => ATOM_CRTC6,
        CONTROLLER_ID_UNDEFINED => ATOM_CRTC_INVALID,
        // Wrong controller id
        _ => return false,
    }) as u8;
    true
}

/// `disp_power_gating_action_to_atom` (command_table_helper2_dce112.c).
fn disp_power_gating_action_to_atom(action: u32) -> u8 {
    use crate::regs::*;
    (match action {
        ASIC_PIPE_DISABLE => ATOM_DISABLE,
        ASIC_PIPE_ENABLE => ATOM_ENABLE,
        ASIC_PIPE_INIT => ATOM_INIT,
        _ => 0,
    }) as u8
}

/// `enable_disp_power_gating_dmcub`: the parameters as a DMUB VBIOS command.
fn enable_disp_power_gating_dmcub(adev: &amdgpu_device, dc: &mut crate::dc::dc, disp_pipe_id: u8, enable: u8) {
    use crate::regs::*;
    let mut cmd = crate::dmub_srv::dmub_rb_cmd::default();
    cmd.set_header(DMUB_CMD__VBIOS, DMUB_CMD__VBIOS_ENABLE_DISP_POWER_GATING,
        SIZEOF_dmub_rb_cmd_enable_disp_power_gating - SIZEOF_dmub_cmd_header);
    cmd.0[EDPG_DISP_PIPE_ID as usize] = disp_pipe_id;
    cmd.0[EDPG_ENABLE as usize] = enable;
    crate::dc_dmub_srv::dc_wake_and_execute_dmub_cmd(adev, dc, &mut cmd, DM_DMUB_WAIT_TYPE_WAIT);
}

/// `enable_disp_power_gating_v2_1`. Executing the ATOM table itself
/// (without `dmub_command_table`) is not ported: DCN 2.1's defaults route it
/// to the DMCUB.
fn enable_disp_power_gating_v2_1(adev: &amdgpu_device, dc: &mut crate::dc::dc, crtc_id: u32,
                                 action: u32) -> crate::bios_parser2::bp_result {
    let mut atom_crtc_id = 0u8;

    if !controller_id_to_atom(crtc_id, &mut atom_crtc_id) {
        return crate::bios_parser2::bp_result::BP_RESULT_BADINPUT;
    }
    let disp_pipe_id = atom_crtc_id;

    let enable = disp_power_gating_action_to_atom(action);

    if dc.ctx.dmub_srv.is_some() && dc.debug.dmub_command_table {
        enable_disp_power_gating_dmcub(adev, dc, disp_pipe_id, enable);
        return crate::bios_parser2::bp_result::BP_RESULT_OK;
    }
    crate::linux::Line::new().s("enable_disp_power_gating_v2_1: the ATOM table path is not ported").say();
    crate::bios_parser2::bp_result::BP_RESULT_FAILURE
}

/// `bios_parser_enable_disp_power_gating` (bios_parser2.c). The fallback
/// executor is not ported.
pub fn bios_parser_enable_disp_power_gating(adev: &amdgpu_device, dc: &mut crate::dc::dc, controller_id: u32,
                                            action: u32) -> crate::bios_parser2::bp_result {
    match dc.ctx.dc_bios.cmd_tbl.enable_disp_power_gating {
        None => crate::bios_parser2::bp_result::BP_RESULT_FAILURE,
        Some(enable_disp_power_gating_fn::enable_disp_power_gating_v2_1) => {
            enable_disp_power_gating_v2_1(adev, dc, controller_id, action)
        }
        Some(enable_disp_power_gating_fn::enable_disp_power_gating_fallback) => {
            crate::linux::Line::new().s("bios_parser_enable_disp_power_gating: fallback not ported").say();
            crate::bios_parser2::bp_result::BP_RESULT_FAILURE
        }
    }
}

/// `struct bp_pixel_clock_parameters` (bios_parser_types.h), the members
/// the v7 executor reads; `flags` is the raw view of its bitfield, and the
/// enums are their values.
#[derive(Default, Clone, Copy)]
pub struct bp_pixel_clock_parameters {
    pub controller_id: u32,
    pub pll_id: crate::dce_clock_source::clock_source_id,
    pub signal_type: u32,
    pub target_pixel_clock_100hz: u32,
    pub reference_divider: u32,
    pub feedback_divider: u32,
    pub fractional_feedback_divider: u32,
    pub pixel_clock_post_divider: u32,
    pub encoder_object_id: crate::dc_types::graphics_object_id,
    pub dfs_bypass_display_clock: u32,
    pub color_depth: u32,
    pub flags: u32,
    pub dio_se_pix_per_cycle: u32,
}

/// `struct program_pixel_clock_flags` bits.
pub const FORCE_PROGRAMMING_OF_PLL: u32 = 1 << 0;
pub const USE_E_CLOCK_AS_SOURCE_FOR_D_CLOCK: u32 = 1 << 1;
pub const SET_EXTERNAL_REF_DIV_SRC: u32 = 1 << 2;
pub const SET_DISPCLK_DFS_BYPASS: u32 = 1 << 3;
pub const PROGRAM_PHY_PLL_ONLY: u32 = 1 << 4;
pub const SUPPORT_YUV_420: u32 = 1 << 5;
pub const SET_XTALIN_REF_SRC: u32 = 1 << 6;
pub const SET_GENLOCK_REF_DIV_SRC: u32 = 1 << 7;

/// `clock_source_id_to_atom` (command_table_helper2_dce112.c).
fn clock_source_id_to_atom(id: crate::dce_clock_source::clock_source_id, atom_pll_id: &mut u32) -> bool {
    use crate::dce_clock_source::clock_source_id::*;
    use crate::regs::*;
    *atom_pll_id = match id {
        CLOCK_SOURCE_COMBO_PHY_PLL0 => ATOM_COMBOPHY_PLL0,
        CLOCK_SOURCE_COMBO_PHY_PLL1 => ATOM_COMBOPHY_PLL1,
        CLOCK_SOURCE_COMBO_PHY_PLL2 => ATOM_COMBOPHY_PLL2,
        CLOCK_SOURCE_COMBO_PHY_PLL3 => ATOM_COMBOPHY_PLL3,
        CLOCK_SOURCE_COMBO_PHY_PLL4 => ATOM_COMBOPHY_PLL4,
        CLOCK_SOURCE_COMBO_PHY_PLL5 => ATOM_COMBOPHY_PLL5,
        CLOCK_SOURCE_COMBO_DISPLAY_PLL0 => ATOM_PPLL0,
        CLOCK_SOURCE_ID_DFS => ATOM_GCK_DFS,
        CLOCK_SOURCE_ID_VCE => ATOM_DP_DTO,
        CLOCK_SOURCE_ID_DP_DTO => ATOM_DP_DTO,
        CLOCK_SOURCE_ID_UNDEFINED => {
            // Should not happen
            *atom_pll_id = ATOM_PPLL_INVALID;
            return false;
        }
        _ => return false,
    };
    true
}

/// `dal_cmd_table_helper_encoder_id_to_atom2` (command_table_helper2.c).
fn encoder_id_to_atom(id: u32) -> u8 {
    use crate::regs::*;
    (match id {
        CT_ENCODER_ID_INTERNAL_LVDS => ENCODER_OBJECT_ID_INTERNAL_LVDS,
        CT_ENCODER_ID_INTERNAL_TMDS1 => ENCODER_OBJECT_ID_INTERNAL_TMDS1,
        CT_ENCODER_ID_INTERNAL_TMDS2 => ENCODER_OBJECT_ID_INTERNAL_TMDS2,
        CT_ENCODER_ID_INTERNAL_DAC1 => ENCODER_OBJECT_ID_INTERNAL_DAC1,
        CT_ENCODER_ID_INTERNAL_DAC2 => ENCODER_OBJECT_ID_INTERNAL_DAC2,
        CT_ENCODER_ID_INTERNAL_LVTM1 => ENCODER_OBJECT_ID_INTERNAL_LVTM1,
        CT_ENCODER_ID_INTERNAL_HDMI => ENCODER_OBJECT_ID_HDMI_INTERNAL,
        CT_ENCODER_ID_EXTERNAL_TRAVIS => ENCODER_OBJECT_ID_TRAVIS,
        CT_ENCODER_ID_EXTERNAL_NUTMEG => ENCODER_OBJECT_ID_NUTMEG,
        CT_ENCODER_ID_INTERNAL_KLDSCP_TMDS1 => ENCODER_OBJECT_ID_INTERNAL_KLDSCP_TMDS1,
        CT_ENCODER_ID_INTERNAL_KLDSCP_DAC1 => ENCODER_OBJECT_ID_INTERNAL_KLDSCP_DAC1,
        CT_ENCODER_ID_INTERNAL_KLDSCP_DAC2 => ENCODER_OBJECT_ID_INTERNAL_KLDSCP_DAC2,
        CT_ENCODER_ID_EXTERNAL_MVPU_FPGA => ENCODER_OBJECT_ID_MVPU_FPGA,
        CT_ENCODER_ID_INTERNAL_DDI => ENCODER_OBJECT_ID_INTERNAL_DDI,
        CT_ENCODER_ID_INTERNAL_UNIPHY => ENCODER_OBJECT_ID_INTERNAL_UNIPHY,
        CT_ENCODER_ID_INTERNAL_KLDSCP_LVTMA => ENCODER_OBJECT_ID_INTERNAL_KLDSCP_LVTMA,
        CT_ENCODER_ID_INTERNAL_UNIPHY1 => ENCODER_OBJECT_ID_INTERNAL_UNIPHY1,
        CT_ENCODER_ID_INTERNAL_UNIPHY2 => ENCODER_OBJECT_ID_INTERNAL_UNIPHY2,
        CT_ENCODER_ID_INTERNAL_UNIPHY3 => ENCODER_OBJECT_ID_INTERNAL_UNIPHY3,
        CT_ENCODER_ID_INTERNAL_WIRELESS => ENCODER_OBJECT_ID_INTERNAL_VCE,
        CT_ENCODER_ID_INTERNAL_VIRTUAL => ENCODER_OBJECT_ID_NONE,
        CT_ENCODER_ID_UNKNOWN => ENCODER_OBJECT_ID_NONE,
        // Invalid encoder id
        _ => ENCODER_OBJECT_ID_NONE,
    }) as u8
}

/// `dal_cmd_table_helper_encoder_mode_bp_to_atom2` (command_table_helper2.c).
pub fn encoder_mode_bp_to_atom(s: u32, enable_dp_audio: bool) -> u32 {
    use crate::regs::*;
    match s {
        LF_SIGNAL_TYPE_DVI_SINGLE_LINK | LF_SIGNAL_TYPE_DVI_DUAL_LINK => ATOM_ENCODER_MODE_DVI,
        LF_SIGNAL_TYPE_HDMI_TYPE_A => ATOM_ENCODER_MODE_HDMI,
        LF_SIGNAL_TYPE_LVDS => ATOM_ENCODER_MODE_LVDS,
        LF_SIGNAL_TYPE_EDP | LF_SIGNAL_TYPE_DISPLAY_PORT_MST | LF_SIGNAL_TYPE_DISPLAY_PORT | LF_SIGNAL_TYPE_VIRTUAL => {
            if enable_dp_audio { ATOM_ENCODER_MODE_DP_AUDIO } else { ATOM_ENCODER_MODE_DP }
        }
        LF_SIGNAL_TYPE_RGB => ATOM_ENCODER_MODE_CRT,
        _ => ATOM_ENCODER_MODE_CRT,
    }
}

/// `transmitter_color_depth_to_atom` (command_table_helper2_dce112.c).
fn transmitter_color_depth_to_atom(id: u32) -> u8 {
    use crate::regs::*;
    (match id {
        TRANSMITTER_COLOR_DEPTH_24 => PIXEL_CLOCK_V7_DEEPCOLOR_RATIO_DIS,
        TRANSMITTER_COLOR_DEPTH_30 => PIXEL_CLOCK_V7_DEEPCOLOR_RATIO_5_4,
        TRANSMITTER_COLOR_DEPTH_36 => PIXEL_CLOCK_V7_DEEPCOLOR_RATIO_3_2,
        TRANSMITTER_COLOR_DEPTH_48 => PIXEL_CLOCK_V7_DEEPCOLOR_RATIO_2_1,
        // Unhandle action in driver!
        _ => 0,
    }) as u8
}

/// `set_pixel_clock_dmcub` (command_table2.c).
fn set_pixel_clock_dmcub(adev: &amdgpu_device, dc: &mut crate::dc::dc,
                         clk: &[u8; crate::regs::SIZEOF_set_pixel_clock_parameter_v1_7 as usize]) {
    use crate::regs::*;
    let mut cmd = crate::dmub_srv::dmub_rb_cmd::default();
    cmd.set_header(DMUB_CMD__VBIOS, DMUB_CMD__VBIOS_SET_PIXEL_CLOCK,
        SIZEOF_dmub_rb_cmd_set_pixel_clock - SIZEOF_dmub_cmd_header);
    cmd.0[SPC_DATA as usize..][..clk.len()].copy_from_slice(clk);
    crate::dc_dmub_srv::dc_wake_and_execute_dmub_cmd(adev, dc, &mut cmd, DM_DMUB_WAIT_TYPE_WAIT);
}

/// `set_pixel_clock_v7` (command_table2.c). Executing the ATOM table
/// itself (without `dmub_command_table`) is not ported: DCN 2.1's
/// defaults route it to the DMCUB.
fn set_pixel_clock_v7(adev: &amdgpu_device, dc: &mut crate::dc::dc, bp_params: &bp_pixel_clock_parameters)
                      -> crate::bios_parser2::bp_result {
    use crate::regs::*;
    let mut result = crate::bios_parser2::bp_result::BP_RESULT_FAILURE;
    let mut clk = [0u8; SIZEOF_set_pixel_clock_parameter_v1_7 as usize];
    let mut controller_id = 0u8;
    let mut pll_id = 0u32;

    if clock_source_id_to_atom(bp_params.pll_id, &mut pll_id)
        && controller_id_to_atom(bp_params.controller_id, &mut controller_id)
    {
        clk[SPC7_CRTC_ID as usize] = controller_id;
        clk[SPC7_PLL_ID as usize] = pll_id as u8;
        clk[SPC7_ENCODEROBJID as usize] = encoder_id_to_atom(bp_params.encoder_object_id.get_encoder_id());

        clk[SPC7_ENCODER_MODE as usize] = encoder_mode_bp_to_atom(bp_params.signal_type, false) as u8;

        clk[SPC7_PIXCLK_100HZ as usize..][..4].copy_from_slice(&bp_params.target_pixel_clock_100hz.to_le_bytes());

        clk[SPC7_DEEP_COLOR_RATIO as usize] = transmitter_color_depth_to_atom(bp_params.color_depth);

        let mut miscinfo = 0u32;
        if bp_params.flags & FORCE_PROGRAMMING_OF_PLL != 0 {
            miscinfo |= PIXEL_CLOCK_V7_MISC_FORCE_PROG_PPLL;
        }

        if bp_params.flags & PROGRAM_PHY_PLL_ONLY != 0 {
            miscinfo |= PIXEL_CLOCK_V7_MISC_PROG_PHYPLL;
        }

        if bp_params.flags & SUPPORT_YUV_420 != 0 {
            miscinfo |= PIXEL_CLOCK_V7_MISC_YUV420_MODE;
        }

        if bp_params.flags & SET_XTALIN_REF_SRC != 0 {
            miscinfo |= PIXEL_CLOCK_V7_MISC_REF_DIV_SRC_XTALIN;
        }

        if bp_params.flags & SET_GENLOCK_REF_DIV_SRC != 0 {
            miscinfo |= PIXEL_CLOCK_V7_MISC_REF_DIV_SRC_GENLK;
        }

        if bp_params.signal_type == LF_SIGNAL_TYPE_DVI_DUAL_LINK {
            miscinfo |= PIXEL_CLOCK_V7_MISC_DVI_DUALLINK_EN;
        }
        clk[SPC7_MISCINFO as usize] = miscinfo as u8;

        if dc.ctx.dmub_srv.is_some() && dc.debug.dmub_command_table {
            set_pixel_clock_dmcub(adev, dc, &clk);
            return crate::bios_parser2::bp_result::BP_RESULT_OK;
        }

        crate::linux::Line::new().s("set_pixel_clock_v7: the ATOM table path is not ported").say();
        result = crate::bios_parser2::bp_result::BP_RESULT_FAILURE;
    }
    result
}

/// `bios_parser_set_pixel_clock` (bios_parser2.c). The fallback hands the
/// parameters to v7 when the DMCUB runs the command tables.
pub fn bios_parser_set_pixel_clock(adev: &amdgpu_device, dc: &mut crate::dc::dc, bp_params: &bp_pixel_clock_parameters)
                                   -> crate::bios_parser2::bp_result {
    match dc.ctx.dc_bios.cmd_tbl.set_pixel_clock {
        None => crate::bios_parser2::bp_result::BP_RESULT_FAILURE,
        Some(set_pixel_clock_fn::set_pixel_clock_v7) => set_pixel_clock_v7(adev, dc, bp_params),
        Some(set_pixel_clock_fn::set_pixel_clock_fallback) => {
            if dc.ctx.dmub_srv.is_some() && dc.debug.dmub_command_table {
                return set_pixel_clock_v7(adev, dc, bp_params);
            }
            crate::bios_parser2::bp_result::BP_RESULT_FAILURE
        }
    }
}
