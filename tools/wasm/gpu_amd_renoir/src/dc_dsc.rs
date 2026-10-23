//! display/dc/dsc/dc_dsc.c (Linux 6.18.55): the stream bandwidth link
//! settings are decided against. The DSC-compressed bandwidth
//! (`dc_dsc_stream_bandwidth_in_kbps`) joins with DSC.
#![allow(dead_code)]

use crate::dc_hw_types::dc_crtc_timing;
use crate::fixpt31_32::*;
use crate::regs::*;

/// `disable_128b_132b_stream_overhead` (a module parameter, off).
const disable_128b_132b_stream_overhead: bool = false;

/// `apply_128b_132b_stream_overhead`.
fn apply_128b_132b_stream_overhead(timing: &dc_crtc_timing, kbps: u32) -> u32 {
    let mut total_kbps = kbps;

    if disable_128b_132b_stream_overhead {
        return kbps;
    }

    if !timing.flags.DSC {
        let mut bpp = dc_fixpt_from_int(kbps as i32);
        bpp = dc_fixpt_div_int(bpp, (timing.pix_clk_100hz / 10) as i64);

        // Symbols_per_HActive = HActive * bpp / (4 lanes * 32-bit symbol size)
        // Overhead_factor = ceil(Symbols_per_HActive) / Symbols_per_HActive
        let mut overhead_factor = dc_fixpt_from_int(timing.h_addressable as i32);
        overhead_factor = dc_fixpt_mul(overhead_factor, bpp);
        overhead_factor = dc_fixpt_div_int(overhead_factor, 128);
        overhead_factor = dc_fixpt_div(dc_fixpt_from_int(dc_fixpt_ceil(overhead_factor)), overhead_factor);

        total_kbps = dc_fixpt_ceil(dc_fixpt_mul_int(overhead_factor, total_kbps as i32)) as u32;
    }

    total_kbps
}

/// `dc_bandwidth_in_kbps_from_timing`.
pub fn dc_bandwidth_in_kbps_from_timing(timing: &dc_crtc_timing, link_encoding: u32) -> u32 {
    if timing.flags.DSC {
        crate::linux::Line::new().s("dc_dsc_stream_bandwidth_in_kbps not ported").say();
        return 0;
    }

    let bits_per_channel = match timing.display_color_depth {
        COLOR_DEPTH_666 => 6,
        COLOR_DEPTH_888 => 8,
        COLOR_DEPTH_101010 => 10,
        COLOR_DEPTH_121212 => 12,
        COLOR_DEPTH_141414 => 14,
        COLOR_DEPTH_161616 => 16,
        _ => 8,
    };

    let mut kbps = timing.pix_clk_100hz / 10;
    kbps *= bits_per_channel;

    if !timing.flags.Y_ONLY {
        // Only YOnly make reduce bandwidth by 1/3 compares to RGB
        kbps *= 3;
        if timing.pixel_encoding == PIXEL_ENCODING_YCBCR420 {
            kbps /= 2;
        } else if timing.pixel_encoding == PIXEL_ENCODING_YCBCR422 {
            kbps = kbps * 2 / 3;
        }
    }

    if link_encoding == DC_LINK_ENCODING_DP_128b_132b {
        kbps = apply_128b_132b_stream_overhead(timing, kbps);
    }

    if link_encoding == DC_LINK_ENCODING_HDMI_FRL
        && timing.vic == 0
        && timing.hdmi_vic == 0
        && timing.frl_uncompressed_video_bandwidth_in_kbps != 0
    {
        kbps = timing.frl_uncompressed_video_bandwidth_in_kbps;
    }

    kbps
}
