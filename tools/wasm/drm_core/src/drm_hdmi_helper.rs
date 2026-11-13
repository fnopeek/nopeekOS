//! drivers/gpu/drm/display/drm_hdmi_helper.c (Linux rpi-6.18): the TMDS
//! character rate a mode needs.

use crate::drm_edid::drm_match_cea_mode;
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
