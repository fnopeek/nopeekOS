//! drivers/gpu/drm/display/drm_hdmi_helper.c (Linux rpi-6.18): the TMDS
//! character rate a mode needs, and the AVI infoframe fields a connector
//! state sets.

use crate::drm_connector::drm_connector_state;
use crate::drm_edid::drm_match_cea_mode;
use crate::hdmi::hdmi_avi_infoframe;
use crate::drm_modes::drm_display_mode;
use crate::regs::*;

/// `drm_hdmi_compute_mode_clock`: the TMDS character rate in Hz of `mode`
/// at `bpc` bits per component in format `fmt` (`HDMI_COLORSPACE_*`), or
/// 0 when the combination is invalid. Arithmetic in `unsigned long long`.
pub fn drm_hdmi_compute_mode_clock(mode: &drm_display_mode, bpc: u32, fmt: u32) -> u64 {
    let mut bpc = bpc;
    let mut clock = (mode.clock as i64 as u64).wrapping_mul(1000);
    let vic = drm_match_cea_mode(mode);

    // CTA-861-G Spec, section 5.4 - Color Coding and Quantization
    // mandates that VIC 1 always uses 8 bpc.
    if vic == 1 && bpc != 8 {
        return 0;
    }

    if fmt == HDMI_COLORSPACE_YUV422 {
        // HDMI 1.0 Spec, section 6.5 - Pixel Encoding states that
        // YUV422 sends 24 bits over three channels, with Cb and Cr
        // components being sent on odd and even pixels, respectively.
        //
        // If fewer than 12 bpc are sent, data are left justified.
        if bpc > 12 {
            return 0;
        }

        // HDMI 1.0 Spec, section 6.5 - Pixel Encoding
        // specifies that YUV422 sends two 12-bits components over
        // three TMDS channels per pixel clock, which is equivalent to
        // three 8-bits components over three channels used by RGB as
        // far as the clock rate goes.
        bpc = 8;
    }

    // HDMI 2.0 Spec, Section 7.1 - YCbCr 4:2:0 Pixel Encoding
    // specifies that YUV420 encoding is carried at a TMDS Character Rate
    // equal to half the pixel clock rate.
    if fmt == HDMI_COLORSPACE_YUV420 {
        clock /= 2;
    }

    if mode.flags & DRM_MODE_FLAG_DBLCLK != 0 {
        clock = clock.wrapping_mul(2);
    }

    // HDMI 1.4b Spec, Section 8.2.3.2 - Frame Packing Structure
    // transmits both eyes within a single frame, at twice the vertical
    // total, and thus twice the pixel clock, of the base 2D mode.
    if mode.flags & DRM_MODE_FLAG_3D_MASK == DRM_MODE_FLAG_3D_FRAME_PACKING {
        clock = clock.wrapping_mul(2);
    }

    // DIV_ROUND_CLOSEST_ULL(clock * bpc, 8)
    clock.wrapping_mul(bpc as u64).wrapping_add(4) / 8
}

/// `drm_hdmi_avi_infoframe_colorimetry`.
pub fn drm_hdmi_avi_infoframe_colorimetry(frame: &mut hdmi_avi_infoframe, conn_state: &drm_connector_state) {
    let colorimetry_index = (conn_state.colorspace & FULL_COLORIMETRY_MASK) as usize;

    let colorimetry_val = if colorimetry_index >= hdmi_colorimetry_val.len() {
        HDMI_COLORIMETRY_NO_DATA
    } else {
        hdmi_colorimetry_val[colorimetry_index]
    };

    frame.colorimetry = colorimetry_val & NORMAL_COLORIMETRY_MASK;
    // ToDo: Extend it for ACE formats as well. Modify the infoframe
    // structure and extend it in drivers/video/hdmi
    frame.extended_colorimetry = (colorimetry_val >> 2) & EXTENDED_COLORIMETRY_MASK;
}

/// `drm_hdmi_avi_infoframe_bars`.
pub fn drm_hdmi_avi_infoframe_bars(frame: &mut hdmi_avi_infoframe, conn_state: &drm_connector_state) {
    frame.right_bar = conn_state.margins.right as u16;
    frame.left_bar = conn_state.margins.left as u16;
    frame.top_bar = conn_state.margins.top as u16;
    frame.bottom_bar = conn_state.margins.bottom as u16;
}
