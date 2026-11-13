//! drivers/gpu/drm/drm_probe_helper.c (Linux rpi-6.18): probing a
//! connector, its modes and their validation against the driver. The
//! driver's callbacks are [`drm_probe_funcs`]. Single-threaded: the
//! modeset locks and their -EDEADLK retries guard nothing. There is no
//! kernel command line, so no `video=` mode (`cmdline_mode.specified` is
//! never set), and no output polling.

use crate::drm_connector::{drm_connector, drm_connector_force, drm_connector_status};
use crate::drm_edid::{drm_add_modes_noedid, drm_connector_update_edid_property, drm_edid_override_connector_update};
use crate::drm_modes::*;
use crate::regs::*;
use alloc::vec::Vec;

/// What the driver gives the probe: `drm_connector_helper_funcs`
/// (detect_ctx, get_modes, mode_valid), `drm_connector_funcs.force`, the
/// connector's possible encoders and CRTCs with their `mode_valid`, and
/// `drm_mode_config_funcs.mode_valid`. A missing callback is the default.
pub trait drm_probe_funcs {
    /// `detect_ctx` or `funcs->detect`; without one, connected.
    fn detect(&mut self, _connector: &mut drm_connector, _force: bool) -> drm_connector_status {
        drm_connector_status::connector_status_connected
    }

    /// `funcs->force`.
    fn force(&mut self, _connector: &mut drm_connector) {}

    /// `get_modes`: the number of modes added to `probed_modes`.
    fn get_modes(&mut self, connector: &mut drm_connector) -> i32;

    /// `mode_valid` (or `mode_valid_ctx`) of the connector.
    fn mode_valid(&mut self, _connector: &drm_connector, _mode: &drm_display_mode) -> drm_mode_status {
        MODE_OK as i32
    }

    /// `dev->mode_config.funcs->mode_valid`.
    fn config_mode_valid(&mut self, _mode: &drm_display_mode) -> drm_mode_status {
        MODE_OK as i32
    }

    /// `drm_connector_for_each_possible_encoder`, each encoder with the
    /// CRTCs `drm_encoder_crtc_ok` accepts, in `drm_for_each_crtc` order.
    fn possible_encoders(&self, connector: &drm_connector) -> Vec<(u32, Vec<u32>)>;

    /// The encoder's `mode_valid`.
    fn encoder_mode_valid(&mut self, _encoder: u32, _mode: &drm_display_mode) -> drm_mode_status {
        MODE_OK as i32
    }

    /// The CRTC's `mode_valid`.
    fn crtc_mode_valid(&mut self, _crtc: u32, _mode: &drm_display_mode) -> drm_mode_status {
        MODE_OK as i32
    }
}

/// `drm_mode_validate_flag`.
fn drm_mode_validate_flag(mode: &drm_display_mode, flags: u32) -> drm_mode_status {
    if mode.flags & DRM_MODE_FLAG_INTERLACE != 0 && flags & DRM_MODE_FLAG_INTERLACE == 0 {
        return MODE_NO_INTERLACE as i32;
    }

    if mode.flags & DRM_MODE_FLAG_DBLSCAN != 0 && flags & DRM_MODE_FLAG_DBLSCAN == 0 {
        return MODE_NO_DBLESCAN as i32;
    }

    if mode.flags & DRM_MODE_FLAG_3D_MASK != 0 && flags & DRM_MODE_FLAG_3D_MASK == 0 {
        return MODE_NO_STEREO as i32;
    }

    MODE_OK as i32
}

/// `drm_mode_validate_pipeline`: the connector, then any encoder (no
/// bridges: `drm_bridge_chain_mode_valid` of none is MODE_OK) with any of
/// its CRTCs.
fn drm_mode_validate_pipeline(mode: &drm_display_mode, connector: &drm_connector,
                              funcs: &mut dyn drm_probe_funcs) -> drm_mode_status {
    // Step 1: Validate against connector
    let mut status = funcs.mode_valid(connector, mode);
    if status != MODE_OK as i32 {
        return status;
    }

    // Step 2: Validate against encoders and crtcs
    for (encoder, crtcs) in funcs.possible_encoders(connector) {
        status = funcs.encoder_mode_valid(encoder, mode);
        if status != MODE_OK as i32 {
            // No point in continuing for crtc check as this encoder
            // will not accept the mode anyway. If all encoders
            // reject the mode then, at exit, ret will not be
            // MODE_OK.
            continue;
        }

        for crtc in crtcs {
            status = funcs.crtc_mode_valid(crtc, mode);
            if status == MODE_OK as i32 {
                // If we get to this point there is at least
                // one combination of encoder+crtc that works
                // for this mode. Lets return now.
                return status;
            }
        }
    }

    status
}

/// `drm_helper_probe_detect`: the driver's detect; the epoch counter
/// counts changes.
pub fn drm_helper_probe_detect(connector: &mut drm_connector, funcs: &mut dyn drm_probe_funcs, force: bool)
    -> drm_connector_status {
    let ret = funcs.detect(connector, force);

    if ret != connector.status {
        connector.epoch_counter += 1;
    }

    ret
}

/// `drm_helper_probe_get_modes`.
fn drm_helper_probe_get_modes(connector: &mut drm_connector, funcs: &mut dyn drm_probe_funcs) -> i32 {
    let mut count = funcs.get_modes(connector);

    // The .get_modes() callback should not return negative values.
    if count < 0 {
        count = 0;
    }

    // Fallback for when DDC probe failed in drm_get_edid() and thus skipped
    // override/firmware EDID.
    if count == 0 && connector.status == drm_connector_status::connector_status_connected {
        count = drm_edid_override_connector_update(connector);
    }

    count
}

/// `__drm_helper_update_and_validate`.
fn __drm_helper_update_and_validate(connector: &mut drm_connector, maxX: u32, maxY: u32,
                                    funcs: &mut dyn drm_probe_funcs) {
    let mut mode_flags = 0;

    drm_connector_list_update(connector);

    if connector.interlace_allowed {
        mode_flags |= DRM_MODE_FLAG_INTERLACE;
    }
    if connector.doublescan_allowed {
        mode_flags |= DRM_MODE_FLAG_DBLSCAN;
    }
    if connector.stereo_allowed {
        mode_flags |= DRM_MODE_FLAG_3D_MASK;
    }

    let mut modes = core::mem::take(&mut connector.modes);
    for mode in modes.iter_mut() {
        if mode.status != MODE_OK as i32 {
            continue;
        }

        mode.status = drm_mode_validate_driver(mode, &mut |m| funcs.config_mode_valid(m));
        if mode.status != MODE_OK as i32 {
            continue;
        }

        mode.status = drm_mode_validate_size(mode, maxX as i32, maxY as i32);
        if mode.status != MODE_OK as i32 {
            continue;
        }

        mode.status = drm_mode_validate_flag(mode, mode_flags);
        if mode.status != MODE_OK as i32 {
            continue;
        }

        mode.status = drm_mode_validate_ycbcr420(mode, connector);
        if mode.status != MODE_OK as i32 {
            continue;
        }

        mode.status = drm_mode_validate_pipeline(mode, connector, funcs);
    }
    connector.modes = modes;
}

/// `drm_helper_probe_single_connector_modes`: detect the connector, fill
/// `modes` with its valid modes up to maxX x maxY (0: any), sorted. The
/// number of modes the probe found.
pub fn drm_helper_probe_single_connector_modes(connector: &mut drm_connector, maxX: u32, maxY: u32,
                                               funcs: &mut dyn drm_probe_funcs) -> i32 {
    let mut count;

    // set all old modes to the stale state
    for mode in connector.modes.iter_mut() {
        mode.status = MODE_STALE as i32;
    }

    if connector.force != drm_connector_force::DRM_FORCE_UNSPECIFIED {
        if connector.force == drm_connector_force::DRM_FORCE_ON
            || connector.force == drm_connector_force::DRM_FORCE_ON_DIGITAL
        {
            connector.status = drm_connector_status::connector_status_connected;
        } else {
            connector.status = drm_connector_status::connector_status_disconnected;
        }
        funcs.force(connector);
    } else {
        connector.status = drm_helper_probe_detect(connector, funcs, true);
    }

    'exit: {
        if connector.status == drm_connector_status::connector_status_disconnected {
            drm_connector_update_edid_property(connector, None);
            drm_mode_prune_invalid(&mut connector.modes);
            count = 0;
            break 'exit;
        }

        count = drm_helper_probe_get_modes(connector, funcs);

        if count == 0
            && (connector.status == drm_connector_status::connector_status_connected
                || connector.status == drm_connector_status::connector_status_unknown)
        {
            count = drm_add_modes_noedid(connector, 1024, 768);

            // Section 4.2.2.6 (EDID Corruption Detection) of the DP 1.4a
            // Link CTS specifies that 640x480 (the official "failsafe"
            // mode) needs to be the default if there's no EDID.
            if connector.connector_type == DRM_MODE_CONNECTOR_DisplayPort {
                drm_set_preferred_mode(connector, 640, 480);
            }
        }
        // drm_helper_probe_add_cmdline_mode: no command line mode.
        if count != 0 {
            __drm_helper_update_and_validate(connector, maxX, maxY, funcs);
        }

        drm_mode_prune_invalid(&mut connector.modes);

        // Displayport spec section 5.2.1.2 ("Video Timing Format") says that
        // all detachable sinks shall support 640x480 @60Hz as a fail safe
        // mode. If all modes were pruned, perhaps because they need more
        // lanes or a higher pixel clock than available, at least try to add
        // in 640x480.
        if connector.modes.is_empty() && connector.connector_type == DRM_MODE_CONNECTOR_DisplayPort {
            count = drm_add_modes_noedid(connector, 640, 480);
            __drm_helper_update_and_validate(connector, maxX, maxY, funcs);
            drm_mode_prune_invalid(&mut connector.modes);
        }
    }

    // exit:
    if connector.modes.is_empty() {
        return 0;
    }

    drm_mode_sort(&mut connector.modes);

    for mode in connector.modes.iter_mut() {
        drm_mode_set_crtcinfo(mode, CRTC_INTERLACE_HALVE_V);
    }

    count
}
