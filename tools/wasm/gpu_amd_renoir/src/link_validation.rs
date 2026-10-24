//! display/dc/link/link_validation.c (Linux 6.18.55): whether a stream's
//! timing fits its link, a dongle in between, and the link's bandwidth.
//! The MST and DP tunnel bandwidth checks are later stages.
#![allow(dead_code)]

use crate::dc::dc;
use crate::dc_dp_types::{bits, channel_coding_cap_DP_128b_132b_SUPPORTED, dc_link_settings, dpcd_caps, LANE_COUNT_UNKNOWN, LINK_RATE_UNKNOWN,
    dsc_support_DSC_PASSTHROUGH_SUPPORT};
use crate::dc_hw_types::dc_crtc_timing;
use crate::dc_types::dc_status;
use crate::regs::*;
use crate::dc_ddc_types::{DISPLAY_DONGLE_DP_DVI_CONVERTER, DISPLAY_DONGLE_DP_DVI_DONGLE, DISPLAY_DONGLE_DP_HDMI_CONVERTER,
    DISPLAY_DONGLE_DP_VGA_CONVERTER};

/// `get_tmds_output_pixel_clock_100hz` (link_validation.c).
fn get_tmds_output_pixel_clock_100hz(timing: &dc_crtc_timing) -> u32 {
    let mut pxl_clk = timing.pix_clk_100hz;

    if timing.pixel_encoding == PIXEL_ENCODING_YCBCR420 {
        pxl_clk /= 2;
    } else if timing.pixel_encoding == PIXEL_ENCODING_YCBCR422 {
        pxl_clk = pxl_clk * 2 / 3;
    }

    if timing.display_color_depth == COLOR_DEPTH_101010 {
        pxl_clk = pxl_clk * 10 / 8;
    } else if timing.display_color_depth == COLOR_DEPTH_121212 {
        pxl_clk = pxl_clk * 12 / 8;
    }

    pxl_clk
}

// `union dp_encoding_format_caps` and `union dp_color_depth_caps` bits.
fn support_rgb(raw: u8) -> bool { bits(raw, 0, 1) != 0 }
fn support_6bpc(raw: u8) -> bool { bits(raw, 0, 1) != 0 }
fn support_8bpc(raw: u8) -> bool { bits(raw, 1, 1) != 0 }
fn support_10bpc(raw: u8) -> bool { bits(raw, 2, 1) != 0 }
fn support_12bpc(raw: u8) -> bool { bits(raw, 3, 1) != 0 }
fn support_16bpc(raw: u8) -> bool { bits(raw, 4, 1) != 0 }

/// `dp_active_dongle_validate_timing` (link_validation.c).
fn dp_active_dongle_validate_timing(timing: &dc_crtc_timing, dpcd_caps: &dpcd_caps) -> bool {
    let dongle_caps = &dpcd_caps.dongle_caps;

    match dpcd_caps.dongle_type {
        DISPLAY_DONGLE_DP_VGA_CONVERTER | DISPLAY_DONGLE_DP_DVI_CONVERTER | DISPLAY_DONGLE_DP_DVI_DONGLE => {
            return timing.pixel_encoding == PIXEL_ENCODING_RGB;
        }
        _ => {}
    }

    if dpcd_caps.dongle_type == DISPLAY_DONGLE_DP_HDMI_CONVERTER && dongle_caps.extendedCapValid {
        // Check Pixel Encoding
        match timing.pixel_encoding {
            PIXEL_ENCODING_RGB | PIXEL_ENCODING_YCBCR444 => {}
            PIXEL_ENCODING_YCBCR422 => {
                if !dongle_caps.is_dp_hdmi_ycbcr422_pass_through {
                    return false;
                }
            }
            PIXEL_ENCODING_YCBCR420 => {
                if !dongle_caps.is_dp_hdmi_ycbcr420_pass_through {
                    return false;
                }
            }
            // These color depths are currently not supported
            PIXEL_ENCODING_UNDEFINED => {}
            // Invalid Pixel Encoding
            _ => return false,
        }

        match timing.display_color_depth {
            // 888 and 666 should always be supported
            COLOR_DEPTH_666 | COLOR_DEPTH_888 => {}
            COLOR_DEPTH_101010 => {
                if dongle_caps.dp_hdmi_max_bpc < 10 {
                    return false;
                }
            }
            COLOR_DEPTH_121212 => {
                if dongle_caps.dp_hdmi_max_bpc < 12 {
                    return false;
                }
            }
            // These color depths are currently not supported
            COLOR_DEPTH_UNDEFINED => {}
            _ => return false,
        }

        // Check 3D format: only frame alternate 3D is supported on an
        // active dongle, other formats are not supported due to bad
        // infoframe translation.
        match timing.timing_3d_format {
            TIMING_3D_FORMAT_NONE | TIMING_3D_FORMAT_FRAME_ALTERNATE => {}
            _ => return false,
        }

        if dongle_caps.dp_hdmi_frl_max_link_bw_in_kbps > 0 {
            // DP to HDMI FRL converter
            let mut output_timing = *timing;

            if timing.flags.DSC && !timing.dsc_cfg.is_frl {
                // DP input has DSC, HDMI FRL output doesn't have DSC, remove
                // DSC from output timing
                output_timing.flags.DSC = false;
            }
            if crate::dc_dsc::dc_bandwidth_in_kbps_from_timing(&output_timing, DC_LINK_ENCODING_HDMI_FRL)
                > dongle_caps.dp_hdmi_frl_max_link_bw_in_kbps
            {
                return false;
            }
        } else {
            // DP to HDMI TMDS converter
            if get_tmds_output_pixel_clock_100hz(timing) > dongle_caps.dp_hdmi_max_pixel_clk_in_khz * 10 {
                return false;
            }
        }
    }

    let ext = &dongle_caps.dfp_cap_ext;
    if channel_coding_cap_DP_128b_132b_SUPPORTED(dpcd_caps.channel_coding_cap) == 0
        && dsc_support_DSC_PASSTHROUGH_SUPPORT(dpcd_caps.dsc_caps.dsc_basic_caps[0]) == 0
        && ext.supported
    {
        if (ext.max_pixel_rate_in_mps as u32) < timing.pix_clk_100hz / 10000 {
            return false;
        }

        if (ext.max_video_h_active_width as u32) < timing.h_addressable {
            return false;
        }

        if (ext.max_video_v_active_height as u32) < timing.v_addressable {
            return false;
        }

        let depth = timing.display_color_depth;
        let caps = match timing.pixel_encoding {
            PIXEL_ENCODING_RGB => Some((ext.rgb_color_depth_caps, true)),
            PIXEL_ENCODING_YCBCR444 => Some((ext.ycbcr444_color_depth_caps, false)),
            PIXEL_ENCODING_YCBCR422 => Some((ext.ycbcr422_color_depth_caps, false)),
            PIXEL_ENCODING_YCBCR420 => Some((ext.ycbcr420_color_depth_caps, false)),
            _ => None,
        };
        if let Some((caps, rgb)) = caps {
            if !support_rgb(ext.encoding_format_caps) {
                return false;
            }
            if rgb && depth == COLOR_DEPTH_666 && !support_6bpc(caps) {
                return false;
            } else if depth == COLOR_DEPTH_888 && !support_8bpc(caps) {
                return false;
            } else if depth == COLOR_DEPTH_101010 && !support_10bpc(caps) {
                return false;
            } else if depth == COLOR_DEPTH_121212 && !support_12bpc(caps) {
                return false;
            } else if depth == COLOR_DEPTH_161616 && !support_16bpc(caps) {
                return false;
            }
        }
    }

    true
}

/// `dp_get_verified_link_cap` (link_dp_capability.c).
pub fn dp_get_verified_link_cap(dc: &dc, li: usize) -> dc_link_settings {
    let link = &dc.links[li];
    if link.preferred_link_setting.lane_count != LANE_COUNT_UNKNOWN
        && link.preferred_link_setting.link_rate != LINK_RATE_UNKNOWN
    {
        return link.preferred_link_setting;
    }
    link.verified_link_cap
}

/// `dp_validate_mode_timing` (link_validation.c).
fn dp_validate_mode_timing(dc: &dc, li: usize, timing: &dc_crtc_timing) -> bool {
    let link = &dc.links[li];

    // According to spec, VSC SDP should be used if pixel format is YCbCr420
    if timing.pixel_encoding == PIXEL_ENCODING_YCBCR420
        && bits(link.dpcd_caps.dprx_feature, 3, 1) == 0
        && link.link_id.get_connector_id() != CONNECTOR_ID_VIRTUAL
    {
        return false;
    }

    // always DP fail safe mode
    if timing.pix_clk_100hz / 10 == 25175 && timing.h_addressable == 640 && timing.v_addressable == 480 {
        return true;
    }

    let link_setting = dp_get_verified_link_cap(dc, li);

    let req_bw = crate::dc_dsc::dc_bandwidth_in_kbps_from_timing(timing,
        crate::link_dp_capability::dc_link_get_highest_encoding_format(dc, li));
    let max_bw = crate::link_dp_capability::dp_link_bandwidth_kbps(dc, li, &link_setting);

    let cap = u16::from_le_bytes(link.dpcd_caps.max_uncompressed_pixel_rate_cap);
    let is_max_uncompressed_pixel_rate_exceeded = cap >> 15 != 0
        && timing.pix_clk_100hz > (cap & 0x7fff) as u32 * 10000;

    if is_max_uncompressed_pixel_rate_exceeded && !timing.flags.DSC {
        return false;
    }

    req_bw <= max_bw
}

/// `link_validate_mode_timing` (link_validation.c). A link's remote sinks
/// are MST's; the port has none.
pub fn link_validate_mode_timing(dc: &dc, signal: u32, li: usize, timing: &dc_crtc_timing) -> dc_status {
    let link = &dc.links[li];
    let max_pix_clk = link.dongle_max_pix_clk * 10;
    let dpcd_caps = &link.dpcd_caps;

    // Passive Dongle
    if max_pix_clk != 0 && get_tmds_output_pixel_clock_100hz(timing) > max_pix_clk {
        return dc_status::DC_EXCEED_DONGLE_CAP;
    }

    // Active Dongle
    if !dp_active_dongle_validate_timing(timing, dpcd_caps) {
        return dc_status::DC_EXCEED_DONGLE_CAP;
    }

    match signal {
        LF_SIGNAL_TYPE_EDP | LF_SIGNAL_TYPE_DISPLAY_PORT => {
            if !dp_validate_mode_timing(dc, li, timing) {
                return dc_status::DC_NO_DP_LINK_BANDWIDTH;
            }
        }
        _ => {}
    }

    dc_status::DC_OK
}
