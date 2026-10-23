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
