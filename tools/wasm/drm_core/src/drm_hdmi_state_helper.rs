//! drivers/gpu/drm/display/drm_hdmi_state_helper.c (Linux rpi-6.18): what
//! an HDMI connector initialised with `drmm_connector_hdmi_init` gets from
//! the core: the mode check against the sink's and the driver's TMDS
//! limit, the EDID update on hotplug, the output format and bpc a commit
//! uses and its infoframes. The driver's `drm_connector_hdmi_funcs` are
//! [`drm_connector_hdmi_funcs`]. HDMI audio and CEC, which the hotplug
//! notifies, are not ported; nor is the HDR infoframe, which needs an HDR
//! metadata property.

use crate::drm_connector::{drm_connector, drm_connector_hdmi_infoframe, drm_connector_state, drm_connector_status};
use crate::drm_edid::{
    drm_default_rgb_quant_range, drm_edid_connector_update, drm_edid_read_ddc,
    drm_hdmi_avi_infoframe_from_display_mode, drm_hdmi_avi_infoframe_quant_range,
    drm_hdmi_vendor_infoframe_from_display_mode, drm_match_cea_mode,
};
use crate::drm_hdmi_helper::{drm_hdmi_avi_infoframe_bars, drm_hdmi_avi_infoframe_colorimetry, drm_hdmi_compute_mode_clock};
use crate::drm_modes::{drm_display_mode, drm_mode_is_420, drm_mode_is_420_only, drm_mode_status};
use crate::hdmi::*;
use crate::i2c::i2c_transfer_fn;
use crate::linux::{Line, EINVAL};
use crate::regs::*;

/// `struct drm_connector_hdmi_funcs`. A missing callback is the default.
pub trait drm_connector_hdmi_funcs {
    /// `tmds_char_rate_valid`.
    fn tmds_char_rate_valid(&mut self, _connector: &drm_connector, _mode: &drm_display_mode, _tmds_rate: u64)
        -> drm_mode_status {
        MODE_OK as i32
    }

    /// `write_infoframe`; without one, `write_device_infoframe` fails.
    fn write_infoframe(&mut self, _connector: &drm_connector, _type: u32, _buffer: &[u8]) -> i32 {
        -EINVAL
    }

    /// `clear_infoframe`; without one, nothing to clear.
    fn clear_infoframe(&mut self, _connector: &drm_connector, _type: u32) -> i32 {
        0
    }
}

/// `hdmi_is_limited_range`. `mode` is `connector_state_get_mode`.
fn hdmi_is_limited_range(connector: &drm_connector, conn_state: &drm_connector_state,
                         mode: &drm_display_mode) -> bool {
    let info = &connector.display_info;

    // The Broadcast RGB property only applies to RGB format, and
    // i915 just assumes limited range for YCbCr output, so let's
    // just do the same.
    if conn_state.hdmi.output_format != HDMI_COLORSPACE_RGB {
        return true;
    }

    if conn_state.hdmi.broadcast_rgb == DRM_HDMI_BROADCAST_RGB_FULL {
        return false;
    }

    if conn_state.hdmi.broadcast_rgb == DRM_HDMI_BROADCAST_RGB_LIMITED {
        return true;
    }

    if !info.is_hdmi {
        return false;
    }

    drm_default_rgb_quant_range(mode) == HDMI_QUANTIZATION_RANGE_LIMITED
}

/// `sink_supports_format_bpc`.
fn sink_supports_format_bpc(connector: &drm_connector, mode: &drm_display_mode, format: u32, bpc: u32) -> bool {
    let info = &connector.display_info;
    let vic = drm_match_cea_mode(mode);

    // CTA-861-F, section 5.4 - Color Coding & Quantization states
    // that the bpc must be 8, 10, 12 or 16 except for the default
    // 640x480 VIC1 where the value must be 8.
    //
    // The definition of default here is ambiguous but the spec
    // refers to VIC1 being the default timing in several occasions
    // so our understanding is that for the default timing (ie,
    // VIC1), the bpc must be 8.
    if vic == 1 && bpc != 8 {
        return false;
    }

    if !info.is_hdmi && (format != HDMI_COLORSPACE_RGB || bpc != 8) {
        return false;
    }

    if connector.hdmi.supported_formats & (1 << format) == 0 {
        return false;
    }

    if drm_mode_is_420_only(info, mode) && format != HDMI_COLORSPACE_YUV420 {
        return false;
    }

    match format {
        HDMI_COLORSPACE_RGB => {
            // In some cases, like when the EDID readout fails, or
            // is not an HDMI compliant EDID for some reason, the
            // color_formats field will be blank and not report any
            // format supported. In such a case, assume that RGB is
            // supported so we can keep things going and light up
            // the display.
            if info.color_formats & DRM_COLOR_FORMAT_RGB444 == 0 {
                Line::new().s("HDMI Sink doesn't support RGB, something's wrong.").say();
            }

            if bpc == 10 && info.edid_hdmi_rgb444_dc_modes as u32 & DRM_EDID_HDMI_DC_30 == 0 {
                return false;
            }

            if bpc == 12 && info.edid_hdmi_rgb444_dc_modes as u32 & DRM_EDID_HDMI_DC_36 == 0 {
                return false;
            }

            true
        }

        HDMI_COLORSPACE_YUV420 => {
            if info.color_formats & DRM_COLOR_FORMAT_YCBCR420 == 0 {
                return false;
            }

            if !drm_mode_is_420(info, mode) {
                return false;
            }

            if bpc == 10 && info.hdmi.y420_dc_modes as u32 & DRM_EDID_YCBCR420_DC_30 == 0 {
                return false;
            }

            if bpc == 12 && info.hdmi.y420_dc_modes as u32 & DRM_EDID_YCBCR420_DC_36 == 0 {
                return false;
            }

            if bpc == 16 && info.hdmi.y420_dc_modes as u32 & DRM_EDID_YCBCR420_DC_48 == 0 {
                return false;
            }

            true
        }

        HDMI_COLORSPACE_YUV422 => {
            if info.color_formats & DRM_COLOR_FORMAT_YCBCR422 == 0 {
                return false;
            }

            if bpc > 12 {
                return false;
            }

            // HDMI Spec 1.3 - Section 6.5 Pixel Encodings and Color Depth
            // states that Deep Color is not relevant for YUV422 so we
            // don't need to check the Deep Color bits in the EDIDs here.
            true
        }

        HDMI_COLORSPACE_YUV444 => {
            if info.color_formats & DRM_COLOR_FORMAT_YCBCR444 == 0 {
                return false;
            }

            if bpc == 10 && info.edid_hdmi_ycbcr444_dc_modes as u32 & DRM_EDID_HDMI_DC_30 == 0 {
                return false;
            }

            if bpc == 12 && info.edid_hdmi_ycbcr444_dc_modes as u32 & DRM_EDID_HDMI_DC_36 == 0 {
                return false;
            }

            true
        }

        _ => false,
    }
}

/// `hdmi_clock_valid`.
fn hdmi_clock_valid(connector: &drm_connector, mode: &drm_display_mode, clock: u64,
                    funcs: &mut dyn drm_connector_hdmi_funcs) -> drm_mode_status {
    let info = &connector.display_info;

    if info.max_tmds_clock != 0 && clock > info.max_tmds_clock as u64 * 1000 {
        return MODE_CLOCK_HIGH as i32;
    }

    let status = funcs.tmds_char_rate_valid(connector, mode, clock);
    if status != MODE_OK as i32 {
        return status;
    }

    MODE_OK as i32
}

/// `hdmi_compute_clock`.
fn hdmi_compute_clock(connector: &drm_connector, conn_state: &mut drm_connector_state, mode: &drm_display_mode,
                      bpc: u32, fmt: u32, funcs: &mut dyn drm_connector_hdmi_funcs) -> i32 {
    let clock = drm_hdmi_compute_mode_clock(mode, bpc, fmt);
    if clock == 0 {
        return -EINVAL;
    }

    let status = hdmi_clock_valid(connector, mode, clock, funcs);
    if status != MODE_OK as i32 {
        return -EINVAL;
    }

    conn_state.hdmi.tmds_char_rate = clock;

    0
}

/// `hdmi_try_format_bpc`.
fn hdmi_try_format_bpc(connector: &drm_connector, conn_state: &mut drm_connector_state, mode: &drm_display_mode,
                       bpc: u32, fmt: u32, funcs: &mut dyn drm_connector_hdmi_funcs) -> bool {
    if !sink_supports_format_bpc(connector, mode, fmt, bpc) {
        return false;
    }

    hdmi_compute_clock(connector, conn_state, mode, bpc, fmt, funcs) == 0
}

/// `hdmi_compute_format_bpc`.
fn hdmi_compute_format_bpc(connector: &drm_connector, conn_state: &mut drm_connector_state,
                           mode: &drm_display_mode, max_bpc: u32, min_bpc: u32, fmt: u32,
                           funcs: &mut dyn drm_connector_hdmi_funcs) -> i32 {
    let mut bpc = max_bpc;
    while bpc >= min_bpc {
        if hdmi_try_format_bpc(connector, conn_state, mode, bpc, fmt, funcs) {
            conn_state.hdmi.output_bpc = bpc;
            conn_state.hdmi.output_format = fmt;
            return 0;
        }
        bpc -= 2;
    }

    -EINVAL
}

/// `hdmi_compute_config`.
fn hdmi_compute_config(connector: &drm_connector, conn_state: &mut drm_connector_state, mode: &drm_display_mode,
                       funcs: &mut dyn drm_connector_hdmi_funcs) -> i32 {
    let max_bpc = conn_state.max_bpc.clamp(8, connector.max_bpc.max(8));

    // If more than 8 bpc was requested, YUV422 at 12 bpc needs the same
    // TMDS rate as RGB at 8 bpc, so try it before dropping RGB to 8 bpc.
    let mut ret = hdmi_compute_format_bpc(connector, conn_state, mode, max_bpc, max_bpc.min(10),
                                          HDMI_COLORSPACE_RGB, funcs);

    if ret != 0 {
        ret = hdmi_compute_format_bpc(connector, conn_state, mode, max_bpc, 8, HDMI_COLORSPACE_YUV422, funcs);
    }

    if ret != 0 && max_bpc > 8 {
        ret = hdmi_compute_format_bpc(connector, conn_state, mode, 8, 8, HDMI_COLORSPACE_RGB, funcs);
    }

    if ret != 0 {
        if connector.ycbcr_420_allowed {
            ret = hdmi_compute_format_bpc(connector, conn_state, mode, max_bpc, 8, HDMI_COLORSPACE_YUV420, funcs);
        } else {
            ret = -EINVAL;
        }
    }

    ret
}

/// `hdmi_generate_avi_infoframe`.
fn hdmi_generate_avi_infoframe(connector: &drm_connector, conn_state: &mut drm_connector_state,
                               mode: &drm_display_mode) -> i32 {
    let is_limited_range = conn_state.hdmi.is_limited_range;
    let rgb_quant_range =
        if is_limited_range { HDMI_QUANTIZATION_RANGE_LIMITED } else { HDMI_QUANTIZATION_RANGE_FULL };

    conn_state.hdmi.infoframes.avi.set = false;

    let mut frame = hdmi_avi_infoframe::default();
    let ret = drm_hdmi_avi_infoframe_from_display_mode(&mut frame, connector, mode);
    if ret != 0 {
        return ret;
    }

    frame.colorspace = conn_state.hdmi.output_format;

    // FIXME: drm_hdmi_avi_infoframe_quant_range() doesn't handle
    // YUV formats at all at the moment, so if we ever support YUV
    // formats this needs to be revised.
    drm_hdmi_avi_infoframe_quant_range(&mut frame, connector, mode, rgb_quant_range);
    drm_hdmi_avi_infoframe_colorimetry(&mut frame, conn_state);
    drm_hdmi_avi_infoframe_bars(&mut frame, conn_state);

    conn_state.hdmi.infoframes.avi = drm_connector_hdmi_infoframe { data: Some(hdmi_infoframe::avi(frame)), set: true };

    0
}

/// `hdmi_generate_spd_infoframe`.
fn hdmi_generate_spd_infoframe(connector: &drm_connector, conn_state: &mut drm_connector_state) -> i32 {
    conn_state.hdmi.infoframes.spd.set = false;

    let mut frame = hdmi_spd_infoframe::default();
    let ret = hdmi_spd_infoframe_init(&mut frame, &connector.hdmi.vendor, &connector.hdmi.product);
    if ret != 0 {
        return ret;
    }

    frame.sdi = HDMI_SPD_SDI_PC;

    conn_state.hdmi.infoframes.spd = drm_connector_hdmi_infoframe { data: Some(hdmi_infoframe::spd(frame)), set: true };

    0
}

/// `hdmi_generate_hdr_infoframe`: without an HDR metadata property there
/// is none.
fn hdmi_generate_hdr_infoframe(_connector: &drm_connector, conn_state: &mut drm_connector_state) -> i32 {
    conn_state.hdmi.infoframes.hdr_drm.set = false;

    0
}

/// `hdmi_generate_hdmi_vendor_infoframe`.
fn hdmi_generate_hdmi_vendor_infoframe(connector: &drm_connector, conn_state: &mut drm_connector_state,
                                       mode: &drm_display_mode) -> i32 {
    let info = &connector.display_info;

    conn_state.hdmi.infoframes.hdmi.set = false;

    if !info.has_hdmi_infoframe {
        return 0;
    }

    let mut frame = hdmi_vendor_infoframe::default();
    let ret = drm_hdmi_vendor_infoframe_from_display_mode(&mut frame, connector, mode);
    if ret != 0 {
        return ret;
    }

    conn_state.hdmi.infoframes.hdmi =
        drm_connector_hdmi_infoframe { data: Some(hdmi_infoframe::vendor(frame)), set: true };

    0
}

/// `hdmi_generate_infoframes`.
fn hdmi_generate_infoframes(connector: &drm_connector, conn_state: &mut drm_connector_state,
                            mode: &drm_display_mode) -> i32 {
    let info = &connector.display_info;

    if !info.is_hdmi {
        return 0;
    }

    let ret = hdmi_generate_avi_infoframe(connector, conn_state, mode);
    if ret != 0 {
        return ret;
    }

    let ret = hdmi_generate_spd_infoframe(connector, conn_state);
    if ret != 0 {
        return ret;
    }

    // Audio Infoframes will be generated by ALSA, and updated by
    // drm_atomic_helper_connector_hdmi_update_audio_infoframe().

    let ret = hdmi_generate_hdr_infoframe(connector, conn_state);
    if ret != 0 {
        return ret;
    }

    hdmi_generate_hdmi_vendor_infoframe(connector, conn_state, mode)
}

/// `drm_atomic_helper_connector_hdmi_check`: the output format, bpc, TMDS
/// rate and infoframes of the new state. `mode` is the new CRTC state's
/// mode. Ok(true) when the CRTC needs a modeset for it
/// (`crtc_state->mode_changed`).
pub fn drm_atomic_helper_connector_hdmi_check(connector: &drm_connector, old_conn_state: &drm_connector_state,
                                              new_conn_state: &mut drm_connector_state, mode: &drm_display_mode,
                                              funcs: &mut dyn drm_connector_hdmi_funcs) -> Result<bool, i32> {
    if new_conn_state.crtc.is_none() || new_conn_state.best_encoder.is_none() {
        return Ok(false);
    }

    let ret = hdmi_compute_config(connector, new_conn_state, mode, funcs);
    if ret != 0 {
        return Err(ret);
    }

    new_conn_state.hdmi.is_limited_range = hdmi_is_limited_range(connector, new_conn_state, mode);

    let ret = hdmi_generate_infoframes(connector, new_conn_state, mode);
    if ret != 0 {
        return Err(ret);
    }

    Ok(old_conn_state.hdmi.broadcast_rgb != new_conn_state.hdmi.broadcast_rgb
        || old_conn_state.hdmi.output_bpc != new_conn_state.hdmi.output_bpc
        || old_conn_state.hdmi.output_format != new_conn_state.hdmi.output_format)
}

/// `drm_hdmi_connector_mode_valid`: the mode at 8 bpc RGB.
pub fn drm_hdmi_connector_mode_valid(connector: &drm_connector, mode: &drm_display_mode,
                                     funcs: &mut dyn drm_connector_hdmi_funcs) -> drm_mode_status {
    let clock = drm_hdmi_compute_mode_clock(mode, 8, HDMI_COLORSPACE_RGB);
    if clock == 0 {
        return MODE_ERROR as i32;
    }

    hdmi_clock_valid(connector, mode, clock, funcs)
}

/// `clear_infoframe` (`clear_device_infoframe`).
fn clear_infoframe(connector: &drm_connector, old_frame: &drm_connector_hdmi_infoframe,
                   funcs: &mut dyn drm_connector_hdmi_funcs) -> i32 {
    let Some(data) = &old_frame.data else { return 0 };

    funcs.clear_infoframe(connector, data.r#type())
}

/// `write_infoframe` (`write_device_infoframe`).
fn write_infoframe(connector: &drm_connector, new_frame: &drm_connector_hdmi_infoframe,
                   funcs: &mut dyn drm_connector_hdmi_funcs) -> i32 {
    let Some(mut frame) = new_frame.data else { return -EINVAL };
    let mut buffer = [0u8; HDMI_INFOFRAME_SIZE_MAX as usize];

    let len = hdmi_infoframe_pack(&mut frame, &mut buffer);
    if len < 0 {
        return len as i32;
    }

    funcs.write_infoframe(connector, frame.r#type(), &buffer[..len as usize])
}

/// `write_or_clear_infoframe`.
fn write_or_clear_infoframe(connector: &drm_connector, old_frame: &drm_connector_hdmi_infoframe,
                            new_frame: &drm_connector_hdmi_infoframe,
                            funcs: &mut dyn drm_connector_hdmi_funcs) -> i32 {
    if new_frame.set {
        return write_infoframe(connector, new_frame, funcs);
    }

    if old_frame.set && !new_frame.set {
        return clear_infoframe(connector, old_frame, funcs);
    }

    0
}

/// `drm_atomic_helper_connector_hdmi_update_infoframes`: the state's
/// infoframes written (no audio infoframe: audio is not ported).
pub fn drm_atomic_helper_connector_hdmi_update_infoframes(connector: &drm_connector,
                                                          old_conn_state: &drm_connector_state,
                                                          new_conn_state: &drm_connector_state,
                                                          funcs: &mut dyn drm_connector_hdmi_funcs) -> i32 {
    let info = &connector.display_info;
    let (old, new) = (&old_conn_state.hdmi.infoframes, &new_conn_state.hdmi.infoframes);

    if !info.is_hdmi {
        return 0;
    }

    let ret = write_or_clear_infoframe(connector, &old.avi, &new.avi, funcs);
    if ret != 0 {
        return ret;
    }

    let ret = write_or_clear_infoframe(connector, &old.hdr_drm, &new.hdr_drm, funcs);
    if ret != 0 {
        return ret;
    }

    let ret = write_or_clear_infoframe(connector, &old.spd, &new.spd, funcs);
    if ret != 0 {
        return ret;
    }

    if info.has_hdmi_infoframe {
        return write_or_clear_infoframe(connector, &old.hdmi, &new.hdmi, funcs);
    }

    0
}

/// `drm_atomic_helper_connector_hdmi_update`. `ddc` is the connector's
/// DDC adapter (`drm_edid_read` without `funcs->read_edid`).
fn drm_atomic_helper_connector_hdmi_update(connector: &mut drm_connector, status: drm_connector_status,
                                           ddc: &mut i2c_transfer_fn) {
    if status == drm_connector_status::connector_status_disconnected {
        drm_edid_connector_update(connector, None);
        return;
    }

    let drm_edid = drm_edid_read_ddc(connector, ddc);

    drm_edid_connector_update(connector, drm_edid.as_ref());
}

/// `drm_atomic_helper_connector_hdmi_hotplug`: the EDID read again (or
/// dropped) after a hotplug or a detect.
pub fn drm_atomic_helper_connector_hdmi_hotplug(connector: &mut drm_connector, status: drm_connector_status,
                                                ddc: &mut i2c_transfer_fn) {
    drm_atomic_helper_connector_hdmi_update(connector, status, ddc);
}
