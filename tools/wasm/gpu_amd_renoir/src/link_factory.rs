//! display/dc/link/link_factory.c + link/protocols/link_hpd.c (get_hpd_line,
//! program_hpd_filter) (Linux 6.18.55): a link per VBIOS connector, the
//! construct path. `struct dc_link` grows as the path reaches its members.
//! Where Linux writes `res_pool->link_encoders[]` the link keeps its encoder
//! and the pool the index of the link owning it.
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::bios_parser2::{
    bios_parser, bios_parser_get_connector_id, bios_parser_get_device_tag, bios_parser_get_disp_connector_caps_info,
    bios_parser_get_src_obj, bios_parser_is_device_id_supported, bp_disp_connector_caps_info, bp_result,
    connector_device_tag_info, dal_device_type,
};
use crate::dc_types::{graphics_object_id, object_enum_id, object_type};
use crate::dce_panel_cntl::{dce_panel_cntl, dcn21_panel_cntl_create, panel_cntl_init_data};
use crate::dcn21_link_encoder::{dcn21_link_encoder, dcn21_link_encoder_create, encoder_init_data, link_encoder,
    TRANSMITTER_UNKNOWN};
use crate::gpio_base::{dal_gpio_close, dal_gpio_open, dal_gpio_unlock_pin, dal_hw_hpd_set_config, gpio, gpio_mode,
    gpio_result, hw_container};
use crate::gpio_service::{dal_irq_get_read_request, dal_irq_get_rx_source, dal_irq_get_source, gpio_service};
use crate::link_ddc::{dal_ddc_get_line, ddc_service, ddc_service_init_data, link_create_ddc_service};
use crate::link_hpd::link_get_hpd_gpio;
use crate::regs::*;
use alloc::boxed::Box;

/// `CONNECTOR_ID_*` (grph_object_id.h) that construct_phy switches on.
const CONNECTOR_ID_SINGLE_LINK_DVII: u32 = 1;
const CONNECTOR_ID_DUAL_LINK_DVII: u32 = 2;
const CONNECTOR_ID_SINGLE_LINK_DVID: u32 = 3;
const CONNECTOR_ID_DUAL_LINK_DVID: u32 = 4;
const CONNECTOR_ID_HDMI_TYPE_A: u32 = 12;
const CONNECTOR_ID_LVDS: u32 = 14;
const CONNECTOR_ID_DISPLAY_PORT: u32 = 19;
const CONNECTOR_ID_EDP: u32 = 20;
const CONNECTOR_ID_MXM: u32 = 21;
const CONNECTOR_ID_USBC: u32 = 24;

/// `ENCODER_ID_*` (grph_object_id.h) that translate_encoder_to_transmitter
/// knows.
const ENCODER_ID_INTERNAL_UNIPHY: u32 = 13;
const ENCODER_ID_INTERNAL_UNIPHY1: u32 = 15;
const ENCODER_ID_INTERNAL_UNIPHY2: u32 = 16;
const ENCODER_ID_EXTERNAL_NUTMEG: u32 = 17;
const ENCODER_ID_EXTERNAL_TRAVIS: u32 = 18;
const ENCODER_ID_INTERNAL_UNIPHY3: u32 = 20;

/// `enum hpd_en_for_edp` (dc_types.h).
const HPD_EN_FOR_ALL_EDP: u32 = 0;
const HPD_EN_FOR_PRIMARY_EDP_ONLY: u32 = 1;
const HPD_EN_FOR_SECONDARY_EDP_ONLY: u32 = 2;

/// The link's encoder: a DIG encoder for a physical link, the virtual one
/// for a virtual link.
#[derive(Debug)]
pub enum link_enc {
    Dcn21(Box<dcn21_link_encoder>),
    Virtual(Box<link_encoder>),
}

impl link_enc {
    pub fn base(&self) -> &link_encoder {
        match self {
            link_enc::Dcn21(e) => &e.base,
            link_enc::Virtual(e) => e,
        }
    }

    /// `link_enc->funcs`.
    pub fn funcs(&self) -> &'static crate::dcn21_link_encoder::link_encoder_ops {
        self.base().funcs.expect("link encoder funcs").ops()
    }
}

/// `struct dc_link`'s `bios_forced_drive_settings`.
#[derive(Default, Clone, Copy, Debug)]
pub struct bios_forced_drive_settings {
    pub VOLTAGE_SWING: u32,
    pub PRE_EMPHASIS: u32,
}

/// `struct psr_settings` (dc_types.h), the members construct sets.
#[derive(Default, Clone, Copy, Debug)]
pub struct psr_settings {
    pub psr_feature_enabled: bool,
    pub psr_vtotal_control_support: bool,
    pub psr_version: u32,
}

/// `struct dc_link`'s `phy_state`; `symclk_state` is `enum
/// symclk_state`.
#[derive(Default, Clone, Copy, Debug)]
pub struct phy_state {
    pub symclk_state: u32,
}

/// `struct replay_settings` (dc_types.h), the members reached.
#[derive(Default, Clone, Copy, Debug)]
pub struct replay_settings {
    pub replay_feature_enabled: bool,
    /// `config.replay_supported`.
    pub replay_supported: bool,
}

/// `struct link_flags` (dc.h), the link's `wa_flags`.
#[derive(Default, Clone, Copy, Debug)]
pub struct link_wa_flags {
    pub dp_keep_receiver_powered: bool,
    pub dp_skip_DID2: bool,
    pub dp_skip_reset_segment: bool,
    pub dp_skip_fs_144hz: bool,
    pub dp_mot_reset_segment: bool,
    // Some USB4 docks do not handle turning off MST DSC once it has been enabled.
    pub dpia_mst_dsc_always_on: bool,
    // Forced DPIA into TBT3 compatibility mode.
    pub dpia_forced_tbt3_mode: bool,
    pub dongle_mode_timing_override: bool,
    pub blank_stream_on_ocs_change: bool,
    pub read_dpcd204h_on_irq_hpd: bool,
    pub force_dp_ffe_preset: bool,
    pub skip_phy_ssc_reduction: bool,
}

/// `struct dc_link_training_overrides` (dc_dp_types.h), the member
/// detection reads.
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_link_training_overrides {
    pub voltage_swing: Option<u32>,
    pub pre_emphasis: Option<u32>,
    pub post_cursor2: Option<u32>,
    pub ffe_preset: Option<u8>,
    pub cr_pattern_time: Option<u16>,
    pub eq_pattern_time: Option<u16>,
    pub pattern_for_cr: Option<u32>,
    pub pattern_for_eq: Option<u32>,
    pub downspread: Option<bool>,
    pub alternate_scrambler_reset: Option<bool>,
    pub enhanced_framing: Option<bool>,
    pub mst_enable: Option<bool>,
    pub fec_enable: Option<bool>,
}

/// `struct dc_link_status` (dc.h), the members reached.
#[derive(Default, Clone, Copy, Debug)]
pub struct dc_link_status {
    pub link_active: bool,
}

/// `struct dc_link` (dc.h), the members the construct path reaches.
#[derive(Debug, Default)]
pub struct dc_link {
    pub link_index: u32,
    pub link_id: graphics_object_id,
    pub ep_type: u32,
    pub is_internal_display: bool,
    pub irq_source_hpd: u32,
    pub irq_source_hpd_rx: u32,
    pub irq_source_read_request: u32,
    pub hpd_gpio: Option<Box<gpio>>,
    pub connector_signal: u32,
    pub ddc: Option<Box<ddc_service>>,
    pub ddc_hw_inst: u32,
    pub hpd_src: u32,
    pub link_enc: Option<link_enc>,
    pub eng_id: u32,
    pub link_enc_hw_inst: u32,
    pub panel_cntl: Option<Box<dce_panel_cntl>>,
    pub device_tag: connector_device_tag_info,
    pub ddi_channel_mapping: u8,
    pub chip_caps: u16,
    pub bios_forced_drive_settings: bios_forced_drive_settings,
    pub psr_settings: psr_settings,
    pub replay_settings: replay_settings,
    pub is_hpd_filter_disabled: bool,
    pub edp_sink_present: bool,
    pub link_powered_externally: bool,
    pub sync_lt_in_progress: bool,
    pub skip_fallback_on_link_loss: bool,
    pub skip_implict_edp_power_control: bool,
    pub dp_ss_off: bool,
    /// `forced_dp_ffe_preset.raw`.
    pub forced_dp_ffe_preset: u8,
    pub cur_lane_setting: [crate::link_dp_training::dc_lane_settings; 4],
    /// `enum dp_panel_mode`.
    pub panel_mode: u32,
    pub phy_state: phy_state,
    pub is_hpd_pending: bool,
    /// `local_sink`: the sink detection found.
    pub local_sink: Option<Box<crate::dc_sink::dc_sink>>,
    /// `priv`: the DM's connector (amdgpu_dm_initialize_drm_device).
    pub priv_: Option<Box<crate::amdgpu_dm_helpers::amdgpu_dm_connector>>,
    /// `enum dc_connection_type`.
    pub r#type: u32,
    pub link_state_valid: bool,
    pub aux_mode: bool,
    pub aux_access_disabled: bool,
    pub is_dig_mapping_flexible: bool,
    pub is_dds: bool,
    pub test_pattern_enabled: bool,
    /// `compliance_test_state.raw`.
    pub compliance_test_state: u8,
    pub dpcd_caps: crate::dc_dp_types::dpcd_caps,
    /// `dpcd_sink_ext_caps.raw`.
    pub dpcd_sink_ext_caps: u8,
    pub dpcd_sink_count: u32,
    pub reported_link_cap: crate::dc_dp_types::dc_link_settings,
    pub verified_link_cap: crate::dc_dp_types::dc_link_settings,
    pub preferred_link_setting: crate::dc_dp_types::dc_link_settings,
    pub cur_link_settings: crate::dc_dp_types::dc_link_settings,
    pub dongle_max_pix_clk: u32,
    pub hdcp_caps: crate::dc_dp_types::hdcp_caps,
    pub dprx_states: crate::dc_dp_types::dprx_states,
    pub wa_flags: link_wa_flags,
    pub preferred_training_settings: dc_link_training_overrides,
    pub link_status: dc_link_status,
    /// `enum dc_link_fec_state` (dc_dp_types.h).
    pub fec_state: u32,
    pub panel_config: panel_config,
    pub dpia_preferred_eng_id: u32,
    pub dpia_bw_alloc_config: dc_dpia_bw_alloc,
    pub dp_trace: dp_trace,
}

/// `struct dc_dpia_bw_alloc` (dc.h), the members a stream's tunnel
/// settings copy.
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_dpia_bw_alloc {
    pub estimated_bw: u32,
    pub allocated_bw: u32,
    pub bw_granularity: u32,
}

/// `struct dc_panel_config` (dc_types.h), its sub-structs flattened; the
/// `pps` delays are in ms.
#[derive(Default, Clone, Copy, Debug)]
pub struct panel_config {
    pub pps_extra_t3_ms: u32,
    pub pps_extra_t7_ms: u32,
    pub pps_extra_delay_backlight_off: u32,
    pub pps_extra_post_t7_ms: u32,
    pub pps_extra_pre_t11_ms: u32,
    pub pps_extra_t12_ms: u32,
    pub pps_extra_post_oui_ms: u32,
    /// `nits_brightness`.
    pub nits_peak: u32,
    pub nits_max_avg: u32,
    pub nits_min: u32,
    pub nits_max_nonboost_brightness_millinits: u32,
    pub nits_min_brightness_millinits: u32,
    /// `psr`.
    pub psr_disable_psr: bool,
    pub psr_disallow_psrsu: bool,
    pub psr_disallow_replay: bool,
    pub psr_rc_disable: bool,
    pub psr_rc_allow_static_screen: bool,
    pub psr_rc_allow_fullscreen_vpb: bool,
    pub psr_read_psrcap_again: bool,
    pub psr_replay_enable_option: u32,
    /// `varib`.
    pub varib_varibright_feature_enable: u32,
    pub varib_def_varibright_level: u32,
    pub varib_abm_config_setting: u32,
    /// `dsc`.
    pub dsc_disable_dsc_edp: bool,
    pub dsc_force_dsc_edp_policy: u32,
    /// `ilr.optimize_edp_link_rate`.
    pub ilr_optimize_edp_link_rate: bool,
}

/// `struct dp_trace`'s eDP power timestamps (link_types.h), in ns.
#[derive(Default, Clone, Copy, Debug)]
pub struct dp_trace {
    pub edp_poweroff: u64,
    pub edp_poweron: u64,
}

/// `dp_trace_set_edp_power_timestamp` (link_dp_trace.c).
pub fn dp_trace_set_edp_power_timestamp(link: &mut dc_link, power_up: bool) {
    if !power_up {
        // save driver power off time stamp
        link.dp_trace.edp_poweroff = crate::dc_helper::dm_get_timestamp();
    } else {
        link.dp_trace.edp_poweron = crate::dc_helper::dm_get_timestamp();
    }
}

/// `dp_trace_get_edp_poweron_timestamp` (link_dp_trace.c).
pub fn dp_trace_get_edp_poweron_timestamp(link: &dc_link) -> u64 {
    link.dp_trace.edp_poweron
}

/// `dp_trace_get_edp_poweroff_timestamp` (link_dp_trace.c).
pub fn dp_trace_get_edp_poweroff_timestamp(link: &dc_link) -> u64 {
    link.dp_trace.edp_poweroff
}

/// `struct link_init_data` (link.h... link_service.h).
#[derive(Clone, Copy, Debug)]
pub struct link_init_data {
    pub connector_index: u32,
    pub link_index: u32,
    pub is_dpia_link: bool,
}

/// What construct_phy reads of `struct dc` and writes back into it: the
/// parser, GPIO service and pool live in `dc`, which the caller splits.
pub struct link_ctx<'a> {
    pub adev: &'a amdgpu_device,
    pub bp: &'a bios_parser,
    pub gpio_service: &'a mut gpio_service,
    pub dc_edp_id_count: &'a mut u32,
    pub link_encoders: &'a mut [Option<u32>],
    pub dig_link_enc_count: &'a mut u32,
    pub is_apu: bool,
    pub smart_mux_version: i32,
    pub allow_edp_hotplug_detection: u32,
    pub hdmi20_disable: bool,
}

/// `translate_encoder_to_transmitter` (link_factory.c).
fn translate_encoder_to_transmitter(encoder: graphics_object_id) -> u32 {
    let e = encoder.enum_id;
    match encoder.id {
        ENCODER_ID_INTERNAL_UNIPHY => match e {
            object_enum_id::ENUM_ID_1 => LF_TRANSMITTER_UNIPHY_A,
            object_enum_id::ENUM_ID_2 => LF_TRANSMITTER_UNIPHY_B,
            _ => TRANSMITTER_UNKNOWN,
        },
        ENCODER_ID_INTERNAL_UNIPHY1 => match e {
            object_enum_id::ENUM_ID_1 => LF_TRANSMITTER_UNIPHY_C,
            object_enum_id::ENUM_ID_2 => LF_TRANSMITTER_UNIPHY_D,
            _ => TRANSMITTER_UNKNOWN,
        },
        ENCODER_ID_INTERNAL_UNIPHY2 => match e {
            object_enum_id::ENUM_ID_1 => LF_TRANSMITTER_UNIPHY_E,
            object_enum_id::ENUM_ID_2 => LF_TRANSMITTER_UNIPHY_F,
            _ => TRANSMITTER_UNKNOWN,
        },
        ENCODER_ID_INTERNAL_UNIPHY3 => match e {
            object_enum_id::ENUM_ID_1 => LF_TRANSMITTER_UNIPHY_G,
            _ => TRANSMITTER_UNKNOWN,
        },
        ENCODER_ID_EXTERNAL_NUTMEG => match e {
            object_enum_id::ENUM_ID_1 => LF_TRANSMITTER_NUTMEG_CRT,
            _ => TRANSMITTER_UNKNOWN,
        },
        ENCODER_ID_EXTERNAL_TRAVIS => match e {
            object_enum_id::ENUM_ID_1 => LF_TRANSMITTER_TRAVIS_CRT,
            object_enum_id::ENUM_ID_2 => LF_TRANSMITTER_TRAVIS_LCD,
            _ => TRANSMITTER_UNKNOWN,
        },
        _ => TRANSMITTER_UNKNOWN,
    }
}

/// `get_ddc_line` (link_factory.c).
fn get_ddc_line(link: &dc_link) -> u32 {
    let Some(ddc) = link.ddc.as_deref().and_then(|d| d.ddc_pin.as_deref()) else {
        return LF_CHANNEL_ID_UNKNOWN;
    };
    match dal_ddc_get_line(ddc) {
        0 => LF_CHANNEL_ID_DDC1,
        1 => LF_CHANNEL_ID_DDC2,
        2 => LF_CHANNEL_ID_DDC3,
        3 => LF_CHANNEL_ID_DDC4,
        4 => LF_CHANNEL_ID_DDC5,
        5 => LF_CHANNEL_ID_DDC6,
        6 => LF_CHANNEL_ID_DDC_VGA,
        7 => LF_CHANNEL_ID_I2C_PAD,
        _ => LF_CHANNEL_ID_UNKNOWN,
    }
}

/// `get_hpd_line` (link_hpd.c): a fresh HPD handle, asked for its IRQ
/// source and dropped.
fn get_hpd_line(cx: &link_ctx, link: &dc_link) -> u32 {
    let Some(hpd) = link_get_hpd_gpio(cx.bp, &cx.adev.bios, link.link_id, cx.gpio_service) else {
        return LF_HPD_SOURCEID_UNKNOWN;
    };
    match dal_irq_get_source(&hpd) {
        DC_IRQ_SOURCE_HPD1 => LF_HPD_SOURCEID1,
        LF_DC_IRQ_SOURCE_HPD2 => LF_HPD_SOURCEID2,
        LF_DC_IRQ_SOURCE_HPD3 => LF_HPD_SOURCEID3,
        LF_DC_IRQ_SOURCE_HPD4 => LF_HPD_SOURCEID4,
        LF_DC_IRQ_SOURCE_HPD5 => LF_HPD_SOURCEID5,
        LF_DC_IRQ_SOURCE_HPD6 => LF_HPD_SOURCEID6,
        _ => LF_HPD_SOURCEID_UNKNOWN,
    }
}

/// `dal_irq_setup_hpd_filter` (gpio_service.c) through the HPD pin's
/// `set_config`.
fn dal_irq_setup_hpd_filter(adev: &amdgpu_device, irq: &gpio, delay_on_connect: u32,
                            delay_on_disconnect: u32) -> gpio_result {
    if !irq.pin_open {
        return gpio_result::GPIO_RESULT_NULL_HANDLE;
    }
    match &irq.hw_container {
        hw_container::Hpd(h) => dal_hw_hpd_set_config(adev, h, delay_on_connect, delay_on_disconnect),
        _ => gpio_result::GPIO_RESULT_NON_SPECIFIC_ERROR,
    }
}

/// `program_hpd_filter` (link_hpd.c).
fn program_hpd_filter(cx: &mut link_ctx, link: &dc_link) -> bool {
    if link.is_hpd_filter_disabled {
        return false;
    }
    let (delay_on_connect_in_ms, delay_on_disconnect_in_ms) = match link.connector_signal {
        LF_SIGNAL_TYPE_DVI_SINGLE_LINK | LF_SIGNAL_TYPE_DVI_DUAL_LINK | LF_SIGNAL_TYPE_HDMI_TYPE_A => (500, 100),
        // DP needs time to settle (MST<->SST switches, passive dongles)
        LF_SIGNAL_TYPE_DISPLAY_PORT | LF_SIGNAL_TYPE_DISPLAY_PORT_MST => (80, 0),
        // LVDS, eDP: no HPD filter
        _ => return false,
    };

    let Some(mut hpd) = link_get_hpd_gpio(cx.bp, &cx.adev.bios, link.link_id, cx.gpio_service) else {
        return false;
    };

    let mut result = false;
    if dal_gpio_open(cx.adev, cx.gpio_service, &mut hpd, gpio_mode::GPIO_MODE_INTERRUPT) == gpio_result::GPIO_RESULT_OK {
        dal_irq_setup_hpd_filter(cx.adev, &hpd, delay_on_connect_in_ms, delay_on_disconnect_in_ms);
        dal_gpio_close(cx.adev, cx.gpio_service, &mut hpd);
        result = true;
    }

    result
}

/// `is_smartmux_suported` (link_edp_panel_control.c).
fn is_smartmux_suported(cx: &link_ctx) -> bool {
    !cx.is_apu && cx.smart_mux_version != 0
}

/// `construct_phy` (link_factory.c). The failure labels release what was
/// taken, which here is dropping the link.
fn construct_phy(cx: &mut link_ctx, link: &mut dc_link, init_params: &link_init_data) -> bool {
    let bios = &cx.adev.bios[..];
    let mut disp_connect_caps_info = bp_disp_connector_caps_info::default();

    link.irq_source_hpd = DC_IRQ_SOURCE_INVALID;
    link.irq_source_hpd_rx = DC_IRQ_SOURCE_INVALID;
    link.irq_source_read_request = DC_IRQ_SOURCE_INVALID;

    link.link_index = init_params.link_index;

    link.link_id = bios_parser_get_connector_id(cx.bp, bios, init_params.connector_index as u8);

    link.ep_type = LF_DISPLAY_ENDPOINT_PHY;

    bios_parser_get_disp_connector_caps_info(cx.bp, bios, link.link_id, &mut disp_connect_caps_info);
    link.is_internal_display = disp_connect_caps_info.INTERNAL_DISPLAY;

    if link.link_id.r#type != object_type::OBJECT_TYPE_CONNECTOR {
        return false;
    }

    // res_pool->funcs->link_init is NULL on DCN 2.1.

    link.hpd_gpio = link_get_hpd_gpio(cx.bp, bios, link.link_id, cx.gpio_service);
    if let Some(h) = link.hpd_gpio.as_deref_mut() {
        dal_gpio_open(cx.adev, cx.gpio_service, h, gpio_mode::GPIO_MODE_INTERRUPT);
        dal_gpio_unlock_pin(cx.gpio_service, h);
        link.irq_source_hpd = dal_irq_get_source(h);
    }

    match link.link_id.id {
        CONNECTOR_ID_HDMI_TYPE_A => {
            link.connector_signal = LF_SIGNAL_TYPE_HDMI_TYPE_A;
            if let Some(h) = link.hpd_gpio.as_deref() {
                link.irq_source_read_request = dal_irq_get_read_request(h);
            }
        }
        CONNECTOR_ID_SINGLE_LINK_DVID | CONNECTOR_ID_SINGLE_LINK_DVII => {
            link.connector_signal = LF_SIGNAL_TYPE_DVI_SINGLE_LINK;
        }
        CONNECTOR_ID_DUAL_LINK_DVID | CONNECTOR_ID_DUAL_LINK_DVII => {
            link.connector_signal = LF_SIGNAL_TYPE_DVI_DUAL_LINK;
        }
        CONNECTOR_ID_DISPLAY_PORT | CONNECTOR_ID_MXM | CONNECTOR_ID_USBC => {
            link.connector_signal = LF_SIGNAL_TYPE_DISPLAY_PORT;
            if let Some(h) = link.hpd_gpio.as_deref() {
                link.irq_source_hpd_rx = dal_irq_get_rx_source(h);
            }
        }
        CONNECTOR_ID_EDP => {
            // With smartmux only the primary eDP gets a link.
            if !(cx.smart_mux_version == 0 || *cx.dc_edp_id_count == 0) {
                return false;
            }
            link.connector_signal = LF_SIGNAL_TYPE_EDP;
            if let Some(h) = link.hpd_gpio.as_deref() {
                if cx.allow_edp_hotplug_detection == 0 && !is_smartmux_suported(cx) {
                    link.irq_source_hpd = DC_IRQ_SOURCE_INVALID;
                }
                match cx.allow_edp_hotplug_detection {
                    HPD_EN_FOR_ALL_EDP => link.irq_source_hpd_rx = dal_irq_get_rx_source(h),
                    HPD_EN_FOR_PRIMARY_EDP_ONLY => {
                        if link.link_index == 0 {
                            link.irq_source_hpd_rx = dal_irq_get_rx_source(h);
                        } else {
                            link.irq_source_hpd = DC_IRQ_SOURCE_INVALID;
                        }
                    }
                    HPD_EN_FOR_SECONDARY_EDP_ONLY => {
                        if link.link_index == 1 {
                            link.irq_source_hpd_rx = dal_irq_get_rx_source(h);
                        } else {
                            link.irq_source_hpd = DC_IRQ_SOURCE_INVALID;
                        }
                    }
                    _ => link.irq_source_hpd = DC_IRQ_SOURCE_INVALID,
                }
            }
        }
        CONNECTOR_ID_LVDS => link.connector_signal = LF_SIGNAL_TYPE_LVDS,
        _ => return false,
    }

    let ddc_service_init_data = ddc_service_init_data { id: link.link_id, has_link: true, is_dpia_link: false };
    link.ddc = Some(link_create_ddc_service(cx.bp, bios, cx.gpio_service, &ddc_service_init_data));
    let Some(ddc_pin) = link.ddc.as_deref().and_then(|d| d.ddc_pin.as_deref()) else {
        return false;
    };

    link.ddc_hw_inst = dal_ddc_get_line(ddc_pin);

    let mut enc_init_data = encoder_init_data::default();
    bios_parser_get_src_obj(cx.bp, bios, link.link_id, 0, &mut enc_init_data.encoder);
    enc_init_data.connector = link.link_id;
    enc_init_data.channel = get_ddc_line(link);
    enc_init_data.hpd_source = get_hpd_line(cx, link);

    link.hpd_src = enc_init_data.hpd_source;

    enc_init_data.transmitter = translate_encoder_to_transmitter(enc_init_data.encoder);
    let Some(enc) = dcn21_link_encoder_create(cx.bp, bios, &enc_init_data, cx.hdmi20_disable) else {
        return false;
    };

    // The link encoder tracking for the dynamic encoder-to-stream assignment.
    link.eng_id = enc.base.preferred_engine;
    if let Some(slot) = cx.link_encoders.get_mut(link.eng_id as usize) {
        *slot = Some(link.link_index);
    }
    *cx.dig_link_enc_count += 1;

    link.link_enc_hw_inst = enc.base.transmitter;
    link.link_enc = Some(link_enc::Dcn21(enc));

    if link.link_id.id == CONNECTOR_ID_EDP || link.link_id.id == CONNECTOR_ID_LVDS {
        let panel_cntl_init_data = panel_cntl_init_data { inst: *cx.dc_edp_id_count, eng_id: link.eng_id };
        link.panel_cntl = Some(dcn21_panel_cntl_create(&panel_cntl_init_data));
        *cx.dc_edp_id_count += 1;
    }

    for i in 0..4u32 {
        if bios_parser_get_device_tag(cx.bp, bios, link.link_id, i, &mut link.device_tag) != bp_result::BP_RESULT_OK {
            return false;
        }

        // Look for the device tag matching the connector signal: CRT for
        // RGB, LCD for the others.
        if !bios_parser_is_device_id_supported(cx.bp, bios, link.device_tag.dev_id) {
            continue;
        }
        if link.device_tag.dev_id.device_type == dal_device_type::DEVICE_TYPE_CRT
            && link.connector_signal != LF_SIGNAL_TYPE_RGB
        {
            continue;
        }
        if link.device_tag.dev_id.device_type == dal_device_type::DEVICE_TYPE_LCD
            && link.connector_signal == LF_SIGNAL_TYPE_RGB
        {
            continue;
        }
        break;
    }

    if let Some(ii) = cx.bp.base.integrated_info.as_deref() {
        // Look for the channel mapping of this connector and device tag.
        for path in ii.ext_disp_conn_info.path.iter() {
            if path.device_connector_id.enum_id == link.link_id.enum_id
                && path.device_connector_id.id == link.link_id.id
                && path.device_connector_id.r#type == link.link_id.r#type
            {
                if link.device_tag.acpi_device != 0 && path.device_acpi_enum == link.device_tag.acpi_device {
                    link.ddi_channel_mapping = path.channel_mapping;
                    link.chip_caps = path.caps;
                } else if path.device_tag == link.device_tag.dev_id.raw_device_tag as u32 {
                    link.ddi_channel_mapping = path.channel_mapping;
                    link.chip_caps = path.caps;
                }

                if (link.chip_caps as u32 & LF_AMD_EXT_DISPLAY_PATH_CAPS__EXT_CHIP_MASK)
                    == LF_AMD_EXT_DISPLAY_PATH_CAPS__DP_FIXED_VS_EN
                {
                    link.bios_forced_drive_settings.VOLTAGE_SWING = (ii.ext_disp_conn_info.fixdpvoltageswing & 0x3) as u32;
                    link.bios_forced_drive_settings.PRE_EMPHASIS =
                        ((ii.ext_disp_conn_info.fixdpvoltageswing >> 2) & 0x3) as u32;
                }
                break;
            }
        }
    }

    // get_atom_dc_golden_table: dce_info v4.4 only; Renoir's is v4.3, so the
    // golden table stays zero.

    // TODO in Linux: check that the GPIO is programmed correctly, or HPD
    // may bounce.
    program_hpd_filter(cx, link);

    link.psr_settings.psr_vtotal_control_support = false;
    link.psr_settings.psr_version = LF_DC_PSR_VERSION_UNSUPPORTED;

    true
}

/// `link_create` (link_factory.c), the physical path (`construct_dpia` is
/// for USB4 tunnels, which Renoir has none of).
pub fn link_create(cx: &mut link_ctx, init_params: &link_init_data) -> Option<Box<dc_link>> {
    let mut link = Box::new(dc_link::default());
    if !construct_phy(cx, &mut link, init_params) {
        return None;
    }
    Some(link)
}

/// The virtual link `create_links` (dc.c) adds per `num_virtual_links`.
pub fn create_virtual_link(link_index: u32) -> Box<dc_link> {
    let mut link = Box::new(dc_link::default());
    link.link_index = link_index;
    link.connector_signal = LF_SIGNAL_TYPE_VIRTUAL;
    link.link_id.r#type = object_type::OBJECT_TYPE_CONNECTOR;
    link.link_id.id = 100; // CONNECTOR_ID_VIRTUAL
    link.link_id.enum_id = object_enum_id::ENUM_ID_1;
    link.psr_settings.psr_version = LF_DC_PSR_VERSION_UNSUPPORTED;

    let enc_init = encoder_init_data {
        channel: LF_CHANNEL_ID_UNKNOWN,
        hpd_source: LF_HPD_SOURCEID_UNKNOWN,
        transmitter: TRANSMITTER_UNKNOWN,
        connector: link.link_id,
        encoder: graphics_object_id::init(21, object_enum_id::ENUM_ID_1, object_type::OBJECT_TYPE_ENCODER),
    };
    let mut enc = Box::new(link_encoder::default());
    crate::dcn21_link_encoder::virtual_link_encoder_construct(&mut enc, &enc_init);
    link.link_enc = Some(link_enc::Virtual(enc));
    link
}

/// `get_num_of_internal_disp` (dc.c).
pub fn get_num_of_internal_disp(links: &[Box<dc_link>]) -> u32 {
    links.iter().filter(|l| l.connector_signal == LF_SIGNAL_TYPE_EDP || l.is_internal_display).count() as u32
}
