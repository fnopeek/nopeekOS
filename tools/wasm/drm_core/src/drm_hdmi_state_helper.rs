//! drivers/gpu/drm/display/drm_hdmi_state_helper.c (Linux rpi-6.18): what
//! an HDMI connector initialised with `drmm_connector_hdmi_init` gets from
//! the core: the mode check against the sink's and the driver's TMDS
//! limit, and the EDID update on hotplug. The driver's
//! `tmds_char_rate_valid` is a closure; HDMI audio and CEC, which the
//! hotplug notifies, are not ported.

use crate::drm_connector::{drm_connector, drm_connector_status};
use crate::drm_edid::{drm_edid_connector_update, drm_edid_read_ddc};
use crate::drm_hdmi_helper::drm_hdmi_compute_mode_clock;
use crate::drm_modes::{drm_display_mode, drm_mode_status};
use crate::i2c::i2c_transfer_fn;
use crate::regs::*;

/// `drm_connector_hdmi_funcs.tmds_char_rate_valid`.
pub type tmds_char_rate_valid_fn<'a> = dyn FnMut(&drm_connector, &drm_display_mode, u64) -> drm_mode_status + 'a;

/// `hdmi_clock_valid`.
fn hdmi_clock_valid(connector: &drm_connector, mode: &drm_display_mode, clock: u64,
                    tmds_char_rate_valid: Option<&mut tmds_char_rate_valid_fn>) -> drm_mode_status {
    let info = &connector.display_info;

    if info.max_tmds_clock != 0 && clock > info.max_tmds_clock as u64 * 1000 {
        return MODE_CLOCK_HIGH as i32;
    }

    if let Some(valid) = tmds_char_rate_valid {
        let status = valid(connector, mode, clock);
        if status != MODE_OK as i32 {
            return status;
        }
    }

    MODE_OK as i32
}

/// `drm_hdmi_connector_mode_valid`: the mode at 8 bpc RGB.
pub fn drm_hdmi_connector_mode_valid(connector: &drm_connector, mode: &drm_display_mode,
                                     tmds_char_rate_valid: Option<&mut tmds_char_rate_valid_fn>)
    -> drm_mode_status {
    let clock = drm_hdmi_compute_mode_clock(mode, 8, HDMI_COLORSPACE_RGB);
    if clock == 0 {
        return MODE_ERROR as i32;
    }

    hdmi_clock_valid(connector, mode, clock, tmds_char_rate_valid)
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
