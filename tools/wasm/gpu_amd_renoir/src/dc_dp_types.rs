//! display/dc/dc_dp_types.h + the HDCP caps of dc_types.h (Linux 6.18.55):
//! what DC knows about a DisplayPort receiver. A union of bitfields is its
//! `raw` byte(s) here; the bits detection reads have accessors with Linux's
//! names.
#![allow(dead_code)]

/// `enum dc_lane_count` / `enum dc_link_rate` / `enum dc_link_spread`.
pub const LANE_COUNT_UNKNOWN: u32 = 0;
pub const LANE_COUNT_ONE: u32 = 1;
pub const LANE_COUNT_TWO: u32 = 2;
pub const LANE_COUNT_FOUR: u32 = 4;
pub const LINK_RATE_UNKNOWN: u32 = 0;
pub const LINK_RATE_LOW: u32 = 0x06;
pub const LINK_RATE_RATE_2: u32 = 0x08;
pub const LINK_RATE_RATE_3: u32 = 0x09;
pub const LINK_RATE_HIGH: u32 = 0x0A;
pub const LINK_RATE_RBR2: u32 = 0x0C;
pub const LINK_RATE_RATE_6: u32 = 0x10;
pub const LINK_RATE_HIGH2: u32 = 0x14;
pub const LINK_RATE_RATE_8: u32 = 0x19;
pub const LINK_RATE_HIGH3: u32 = 0x1E;
pub const LINK_RATE_UHBR10: u32 = 1000;
pub const LINK_RATE_UHBR13_5: u32 = 1350;
pub const LINK_RATE_UHBR20: u32 = 2000;
pub const LINK_SPREAD_DISABLED: u32 = 0x00;
pub const LINK_SPREAD_05_DOWNSPREAD_30KHZ: u32 = 0x10;
pub const LINK_SPREAD_05_DOWNSPREAD_33KHZ: u32 = 0x11;

/// `enum dp_link_encoding`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum dp_link_encoding {
    DP_UNKNOWN_ENCODING = 0,
    DP_8b_10b_ENCODING = 1,
    DP_128b_132b_ENCODING = 2,
}

/// `struct dc_link_settings`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct dc_link_settings {
    pub lane_count: u32,
    pub link_rate: u32,
    pub link_spread: u32,
    pub use_link_rate_set: bool,
    pub link_rate_set: u8,
}

/// `struct dc_lttpr_caps`; the unions are their bytes.
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_lttpr_caps {
    pub revision: u8,
    pub mode: u8,
    pub max_lane_count: u8,
    pub max_link_rate: u8,
    pub phy_repeater_cnt: u8,
    pub max_ext_timeout: u8,
    pub main_link_channel_coding: u8,
    pub supported_128b_132b_rates: u8,
    pub alpm: u8,
    pub aux_rd_interval: [u8; 8],
    pub lttpr_ieee_oui: [u8; 3],
    pub lttpr_device_id: [u8; 6],
}

/// `struct dc_dongle_dfp_cap_ext`; the encoding/colour depth caps are
/// their bytes.
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_dongle_dfp_cap_ext {
    pub supported: bool,
    pub max_pixel_rate_in_mps: u16,
    pub max_video_h_active_width: u16,
    pub max_video_v_active_height: u16,
    pub encoding_format_caps: u8,
    pub rgb_color_depth_caps: u8,
    pub ycbcr444_color_depth_caps: u8,
    pub ycbcr422_color_depth_caps: u8,
    pub ycbcr420_color_depth_caps: u8,
}

/// `struct dc_dongle_caps`.
#[derive(Clone, Copy, Default, Debug)]
pub struct dc_dongle_caps {
    pub dongle_type: u32,
    pub extendedCapValid: bool,
    pub is_dp_hdmi_s3d_converter: bool,
    pub is_dp_hdmi_ycbcr422_pass_through: bool,
    pub is_dp_hdmi_ycbcr420_pass_through: bool,
    pub is_dp_hdmi_ycbcr422_converter: bool,
    pub is_dp_hdmi_ycbcr420_converter: bool,
    pub dp_hdmi_max_bpc: u32,
    pub dp_hdmi_max_pixel_clk_in_khz: u32,
    pub dp_hdmi_frl_max_link_bw_in_kbps: u32,
    pub dp_hdmi_regulated_autonomous_mode_support: u32,
    pub dfp_cap_ext: dc_dongle_dfp_cap_ext,
}

/// `struct dpcd_usb4_dp_tunneling_info`.
#[derive(Clone, Copy, Default, Debug)]
pub struct dpcd_usb4_dp_tunneling_info {
    pub dp_tun_cap: u8,
    pub dpia_info: u8,
    pub driver_bw_cap: u8,
    pub dpia_tunnel_info: u8,
    pub usb4_driver_id: u8,
    pub usb4_topology_id: [u8; 5],
}

/// `struct edp_psr_info`.
#[derive(Clone, Copy, Default, Debug)]
pub struct edp_psr_info {
    pub psr_version: u8,
    pub psr_dpcd_caps: u8,
    pub psr2_su_y_granularity_cap: u8,
    pub force_psrsu_cap: u8,
}

/// `struct replay_info`.
#[derive(Clone, Copy, Default, Debug)]
pub struct replay_info {
    pub pixel_deviation_per_line: u8,
    pub max_deviation_line: u8,
}

/// `struct dpcd_dsc_capabilities`.
#[derive(Clone, Copy, Default, Debug)]
pub struct dpcd_dsc_capabilities {
    pub dsc_basic_caps: [u8; 16],
    pub dsc_branch_decoder_caps: [u8; 3],
}

/// `struct dpcd_caps`.
#[derive(Clone, Copy, Default, Debug)]
pub struct dpcd_caps {
    pub dpcd_rev: u8,
    pub max_ln_count: u8,
    pub max_down_spread: u8,
    pub dprx_feature: u8,
    // valid only for eDP v1.4 or higher
    pub edp_supported_link_rates_count: u8,
    pub edp_supported_link_rates: [u32; 8],
    // dongle type (DP converter, CV smart dongle)
    pub dongle_type: u32,
    pub is_dongle_type_one: bool,
    // branch device or sink device
    pub is_branch_dev: bool,
    // Dongle's downstream count.
    pub sink_count: u8,
    pub is_mst_capable: bool,
    pub dongle_caps: dc_dongle_caps,
    pub sink_dev_id: u32,
    pub sink_dev_id_str: [u8; 6],
    pub sink_hw_revision: u8,
    pub sink_fw_revision: [u8; 2],
    pub branch_dev_id: u32,
    pub branch_dev_name: [u8; 6],
    pub branch_hw_revision: u8,
    pub branch_fw_revision: [u8; 2],
    pub branch_vendor_specific_data: [u8; 4],
    pub allow_invalid_MSA_timing_param: bool,
    pub panel_mode_edp: bool,
    pub dpcd_display_control_capable: bool,
    pub ext_receiver_cap_field_present: bool,
    pub set_power_state_capable_edp: bool,
    pub dynamic_backlight_capable_edp: bool,
    pub fec_cap: u8,
    pub dsc_caps: dpcd_dsc_capabilities,
    pub lttpr_caps: dc_lttpr_caps,
    /// `adaptive_sync_caps.dp_adap_sync_caps`.
    pub adaptive_sync_caps: u8,
    pub usb4_dp_tun_info: dpcd_usb4_dp_tunneling_info,
    pub max_uncompressed_pixel_rate_cap: [u8; 2],
    pub dp_128b_132b_supported_link_rates: u8,
    pub channel_coding_cap: u8,
    pub fallback_formats: u8,
    pub fec_cap1: u8,
    pub panel_luminance_control: bool,
    pub cable_id: u8,
    pub edp_rev: u8,
    pub alpm_caps: u8,
    pub psr_info: edp_psr_info,
    pub pr_info: replay_info,
    pub edp_oled_emission_rate: u16,
    pub receive_port0_cap: [u8; 2],
    // Indicates the number of SST links supported by MSO (Multi-Stream Output)
    pub mso_cap_sst_links_supported: u8,
    pub dp_edp_general_cap_2: u8,
}

/// `struct dprx_states`.
#[derive(Clone, Copy, Default, Debug)]
pub struct dprx_states {
    pub cable_id_written: bool,
}

/// `struct hdcp_caps`: `rx_caps` (3 bytes, `version` first) and `bcaps`.
#[derive(Clone, Copy, Default, Debug)]
pub struct hdcp_caps {
    pub rx_caps: [u8; 3],
    pub bcaps: u8,
}

/// A bitfield of a raw byte.
pub fn bits(raw: u8, shift: u32, width: u32) -> u8 {
    (raw >> shift) & ((1u16 << width) - 1) as u8
}

// The bitfields detection reads, by union and member.
pub fn training_aux_rd_interval_EXT_RECEIVER_CAP_FIELD_PRESENT(raw: u8) -> u8 { bits(raw, 7, 1) }
pub fn dp_downstream_port_present_PORT_PRESENT(raw: u8) -> u8 { bits(raw, 0, 1) }
pub fn dp_downstream_port_present_PORT_TYPE(raw: u8) -> u8 { bits(raw, 1, 2) }
pub fn dp_downstream_port_present_DETAILED_CAPS(raw: u8) -> u8 { bits(raw, 4, 1) }
pub fn down_stream_port_count_IGNORE_MSA_TIMING_PARAM(raw: u8) -> u8 { bits(raw, 6, 1) }
pub fn max_lane_count_MAX_LANE_COUNT(raw: u8) -> u8 { bits(raw, 0, 5) }
pub fn max_down_spread_MAX_DOWN_SPREAD(raw: u8) -> u8 { bits(raw, 0, 1) }
pub fn edp_configuration_cap_ALT_SCRAMBLER_RESET(raw: u8) -> u8 { bits(raw, 0, 1) }
pub fn edp_configuration_cap_DPCD_DISPLAY_CONTROL_CAPABLE(raw: u8) -> u8 { bits(raw, 3, 1) }
pub fn channel_coding_cap_DP_128b_132b_SUPPORTED(raw: u8) -> u8 { bits(raw, 1, 1) }
pub fn mstm_cap_MST_CAP(raw: u8) -> u8 { bits(raw, 0, 1) }
pub fn fec_cap_FEC_CAPABLE(raw: u8) -> u8 { bits(raw, 0, 1) }
pub fn dsc_support_DSC_SUPPORT(raw: u8) -> u8 { bits(raw, 0, 1) }
pub fn dsc_support_DSC_PASSTHROUGH_SUPPORT(raw: u8) -> u8 { bits(raw, 1, 1) }
pub fn psr_dpcd_caps_Y_COORDINATE_REQUIRED(raw: u8) -> u8 { bits(raw, 4, 1) }
pub fn sink_ext_caps_oled(raw: u8) -> u8 { bits(raw, 4, 1) }
pub fn sink_ext_caps_emission_output(raw: u8) -> u8 { bits(raw, 7, 1) }
pub fn cable_id_UHBR10_20_CAPABILITY(raw: u8) -> u8 { bits(raw, 0, 2) }
pub fn cable_id_UHBR13_5_CAPABILITY(raw: u8) -> u8 { bits(raw, 2, 1) }
pub fn cable_id_CABLE_TYPE(raw: u8) -> u8 { bits(raw, 3, 3) }
pub fn rates_128b_132b_UHBR10(raw: u8) -> u8 { bits(raw, 0, 1) }
pub fn rates_128b_132b_UHBR20(raw: u8) -> u8 { bits(raw, 1, 1) }
pub fn rates_128b_132b_UHBR13_5(raw: u8) -> u8 { bits(raw, 2, 1) }
pub fn dwnstream_port_caps_byte0_DWN_STRM_PORTX_TYPE(raw: u8) -> u8 { bits(raw, 0, 3) }
pub fn sink_count_SINK_COUNT(raw: u8) -> u8 { bits(raw, 0, 6) }
pub fn lane_count_set_LANE_COUNT_SET(raw: u8) -> u8 { bits(raw, 0, 5) }
pub fn dp_tun_cap_dp_tunneling(raw: u8) -> u8 { bits(raw, 0, 1) }
pub fn dp_tun_cap_dpia_bw_alloc(raw: u8) -> u8 { bits(raw, 7, 1) }
pub fn usb4_driver_bw_cap_driver_bw_alloc_support(raw: u8) -> u8 { bits(raw, 7, 1) }
