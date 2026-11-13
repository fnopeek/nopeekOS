//! drivers/gpu/drm/drm_client_modeset.c (Linux rpi-6.18): how an
//! in-kernel DRM client (the fbdev emulation) picks a mode and a CRTC for
//! each connector, `drm_client_modeset_probe`. A CRTC is its index in the
//! device's CRTC list; a connector its index in the client's connector
//! list (all connectors but writeback, in device order). No kernel command
//! line, so no `video=` mode.

use crate::drm_connector::{drm_connector, drm_connector_force, drm_connector_status};
use crate::drm_edid::drm_mode_find_dmt;
use crate::drm_modes::*;
use crate::drm_probe_helper::{drm_helper_probe_single_connector_modes, drm_probe_funcs};
use crate::regs::*;
use alloc::vec;
use alloc::vec::Vec;

/// `struct drm_mode_set`, what a client sets on one CRTC.
#[derive(Clone, Debug, Default)]
pub struct drm_mode_set {
    pub crtc: u32,
    pub mode: Option<drm_display_mode>,
    pub x: u32,
    pub y: u32,
    /// The client's connector indices.
    pub connectors: Vec<usize>,
}

/// `struct drm_client_dev`, its modesets: one per CRTC
/// (`drm_client_modeset_create`).
pub struct drm_client_dev {
    pub modesets: Vec<drm_mode_set>,
}

/// What the client reads of the device beyond the probe: the mode
/// config's limits, whether it is atomic, and the current atomic state
/// the firmware configuration starts from.
pub trait drm_client_funcs: drm_probe_funcs {
    /// `dev->mode_config.num_crtc`, `max_width`, `max_height`.
    fn num_crtc(&self) -> u32;
    fn max_width(&self) -> u32;
    fn max_height(&self) -> u32;
    /// `drm_drv_uses_atomic_modeset`.
    fn uses_atomic_modeset(&self) -> bool {
        true
    }
    /// `connector->state->best_encoder` and `connector->state->crtc`.
    fn connector_state(&self, connector: &drm_connector) -> (Option<u32>, Option<u32>);
    /// `crtc->state->mode`.
    fn crtc_state_mode(&self, crtc: u32) -> drm_display_mode;
}

/// `drm_client_offset`.
#[derive(Clone, Copy, Default)]
struct drm_client_offset {
    x: i32,
    y: i32,
}

impl drm_client_dev {
    /// `drm_client_modeset_create`: a modeset per CRTC, in device order.
    pub fn new(num_crtc: u32) -> Self {
        drm_client_dev {
            modesets: (0..num_crtc).map(|crtc| drm_mode_set { crtc, ..Default::default() }).collect(),
        }
    }

    /// `drm_client_find_modeset`.
    fn find_modeset(&mut self, crtc: u32) -> Option<&mut drm_mode_set> {
        self.modesets.iter_mut().find(|m| m.crtc == crtc)
    }

    /// `drm_client_modeset_release`.
    fn release(&mut self) {
        for modeset in self.modesets.iter_mut() {
            modeset.mode = None;
            modeset.connectors.clear();
        }
    }
}

/// `drm_connector_get_tiled_mode`.
fn drm_connector_get_tiled_mode(connector: &drm_connector) -> Option<drm_display_mode> {
    connector
        .modes
        .iter()
        .find(|m| m.hdisplay == connector.tile_h_size && m.vdisplay == connector.tile_v_size)
        .cloned()
}

/// `drm_connector_fallback_non_tiled_mode`.
fn drm_connector_fallback_non_tiled_mode(connector: &drm_connector) -> Option<drm_display_mode> {
    connector
        .modes
        .iter()
        .find(|m| !(m.hdisplay == connector.tile_h_size && m.vdisplay == connector.tile_v_size))
        .cloned()
}

/// `drm_connector_preferred_mode`.
fn drm_connector_preferred_mode(connector: &drm_connector, width: i32, height: i32) -> Option<drm_display_mode> {
    for mode in &connector.modes {
        if mode.hdisplay as i32 > width || mode.vdisplay as i32 > height {
            continue;
        }
        if mode.r#type as u32 & DRM_MODE_TYPE_PREFERRED != 0 {
            return Some(mode.clone());
        }
    }
    None
}

/// `drm_connector_first_mode`.
fn drm_connector_first_mode(connector: &drm_connector) -> Option<drm_display_mode> {
    connector.modes.first().cloned()
}

/// `drm_connector_pick_cmdline_mode`: a user-defined mode; without a
/// command line mode the search ends there.
fn drm_connector_pick_cmdline_mode(connector: &drm_connector) -> Option<drm_display_mode> {
    connector.modes.iter().find(|m| m.r#type as u32 & DRM_MODE_TYPE_USERDEF != 0).cloned()
}

/// `drm_connector_enabled`.
fn drm_connector_enabled(connector: &drm_connector, strict: bool) -> bool {
    if connector.display_info.non_desktop {
        return false;
    }

    if strict {
        connector.status == drm_connector_status::connector_status_connected
    } else {
        connector.status != drm_connector_status::connector_status_disconnected
    }
}

/// `drm_client_connectors_enabled`.
fn drm_client_connectors_enabled(connectors: &[&drm_connector], enabled: &mut [bool]) {
    let mut any_enabled = false;

    for (i, connector) in connectors.iter().enumerate() {
        enabled[i] = drm_connector_enabled(connector, true);
        any_enabled |= enabled[i];
    }

    if any_enabled {
        return;
    }

    for (i, connector) in connectors.iter().enumerate() {
        enabled[i] = drm_connector_enabled(connector, false);
    }
}

/// `drm_client_target_cloned`.
fn drm_client_target_cloned(funcs: &dyn drm_client_funcs, connectors: &[&drm_connector],
                            modes: &mut [Option<drm_display_mode>], enabled: &[bool]) -> bool {
    const CLONE_MATCH: u32 =
        DRM_MODE_MATCH_TIMINGS | DRM_MODE_MATCH_CLOCK | DRM_MODE_MATCH_FLAGS | DRM_MODE_MATCH_3D_FLAGS;

    // only contemplate cloning in the single crtc case
    if funcs.num_crtc() > 1 {
        return false;
    }

    // only contemplate cloning if more than one connector is enabled
    if enabled.iter().filter(|&&e| e).count() <= 1 {
        return false;
    }

    // check the command line or if nothing common pick 1024x768
    let mut can_clone = true;
    for i in 0..connectors.len() {
        if !enabled[i] {
            continue;
        }

        modes[i] = drm_connector_pick_cmdline_mode(&connectors[i]);
        let Some(mi) = modes[i].clone() else {
            can_clone = false;
            break;
        };
        for j in 0..i {
            if !enabled[j] {
                continue;
            }
            if !modes[j].as_ref().is_some_and(|mj| drm_mode_match(mj, &mi, CLONE_MATCH)) {
                can_clone = false;
            }
        }
    }

    if can_clone {
        return true;
    }

    // try and find a 1024x768 mode on each connector
    can_clone = true;
    let Some(dmt_mode) = drm_mode_find_dmt(1024, 768, 60, false) else { return false };

    for i in 0..connectors.len() {
        if !enabled[i] {
            continue;
        }

        for mode in &connectors[i].modes {
            if drm_mode_match(mode, &dmt_mode, CLONE_MATCH) {
                modes[i] = Some(mode.clone());
            }
        }
        if modes[i].is_none() {
            can_clone = false;
        }
    }

    // fail: "kms: can't enable cloning when we probably wanted to."
    can_clone
}

/// `drm_client_get_tile_offsets`.
fn drm_client_get_tile_offsets(connectors: &[&drm_connector], modes: &[Option<drm_display_mode>],
                               offsets: &mut [drm_client_offset], idx: usize, h_idx: i32, v_idx: i32) {
    let mut hoffset = 0;
    let mut voffset = 0;

    for (i, connector) in connectors.iter().enumerate() {
        if !connector.has_tile {
            continue;
        }

        let Some(m) = &modes[i] else {
            // no modes for connector tiled: Linux skips it only when
            // (h_idx || v_idx) and would read a NULL mode otherwise.
            continue;
        };
        if (connector.tile_h_loc as i32) < h_idx {
            hoffset += m.hdisplay as i32;
        }

        if (connector.tile_v_loc as i32) < v_idx {
            voffset += m.vdisplay as i32;
        }
    }
    offsets[idx].x = hoffset;
    offsets[idx].y = voffset;
}

/// `drm_client_target_preferred`.
fn drm_client_target_preferred(connectors: &[&drm_connector], modes: &mut [Option<drm_display_mode>],
                               offsets: &mut [drm_client_offset], enabled: &[bool], width: i32, height: i32)
    -> bool {
    let connector_count = connectors.len();
    let mask: u64 = if connector_count >= 64 { u64::MAX } else { (1u64 << connector_count) - 1 };
    let mut conn_configured: u64 = 0;
    let mut tile_pass = 0;
    let num_tiled_conns = connectors
        .iter()
        .filter(|c| c.has_tile && c.status == drm_connector_status::connector_status_connected)
        .count() as i32;

    loop {
        for i in 0..connector_count {
            let connector = &connectors[i];

            if conn_configured & (1u64 << i) != 0 {
                continue;
            }

            if !enabled[i] {
                conn_configured |= 1u64 << i;
                continue;
            }

            // first pass over all the untiled connectors
            if tile_pass == 0 && connector.has_tile {
                continue;
            }

            if tile_pass == 1 {
                if connector.tile_h_loc != 0 || connector.tile_v_loc != 0 {
                    continue;
                }
            } else if tile_pass > 1 {
                if connector.tile_h_loc as i32 != tile_pass - 1 && connector.tile_v_loc as i32 != tile_pass - 1 {
                    // if this tile_pass doesn't cover any of the tiles - keep going
                    continue;
                }

                // find the tile offsets for this pass - need to find
                // all tiles left and above
                drm_client_get_tile_offsets(connectors, modes, offsets, i, connector.tile_h_loc as i32,
                                            connector.tile_v_loc as i32);
            }

            modes[i] = drm_connector_pick_cmdline_mode(connector);

            if modes[i].is_none() {
                modes[i] = drm_connector_preferred_mode(connector, width, height);
            }

            if modes[i].is_none() {
                modes[i] = drm_connector_first_mode(connector);
            }

            // In case of tiled mode if all tiles not present fallback to
            // first available non tiled mode.
            // After all tiles are present, try to find the tiled mode
            // for all and if tiled mode not present due to fbcon size
            // limitations, use first non tiled mode only for
            // tile 0,0 and set to no mode for all other tiles.
            if connector.has_tile {
                if num_tiled_conns < connector.num_h_tile as i32 * connector.num_v_tile as i32
                    || (connector.tile_h_loc == 0
                        && connector.tile_v_loc == 0
                        && drm_connector_get_tiled_mode(connector).is_none())
                {
                    modes[i] = drm_connector_fallback_non_tiled_mode(connector);
                } else {
                    modes[i] = drm_connector_get_tiled_mode(connector);
                }
            }

            conn_configured |= 1u64 << i;
        }

        if (conn_configured & mask) != mask {
            tile_pass += 1;
            continue;
        }
        return true;
    }
}

/// `connector_has_possible_crtc`.
fn connector_has_possible_crtc(funcs: &dyn drm_client_funcs, connector: &drm_connector, crtc: u32) -> bool {
    funcs.possible_encoders(connector).iter().any(|(_, crtcs)| crtcs.contains(&crtc))
}

/// `drm_client_pick_crtcs`: the best-scoring CRTC for each connector from
/// `n` on.
#[allow(clippy::too_many_arguments)]
fn drm_client_pick_crtcs(client: &drm_client_dev, funcs: &dyn drm_client_funcs, connectors: &[&drm_connector],
                         best_crtcs: &mut [Option<u32>], modes: &[Option<drm_display_mode>], n: usize, width: i32,
                         height: i32) -> i32 {
    let connector_count = connectors.len();

    if n == connector_count {
        return 0;
    }

    let connector = &connectors[n];

    best_crtcs[n] = None;
    let mut best_score =
        drm_client_pick_crtcs(client, funcs, connectors, best_crtcs, modes, n + 1, width, height);
    let Some(mode_n) = &modes[n] else { return best_score };

    let mut crtcs: Vec<Option<u32>> = vec![None; connector_count];

    let mut my_score = 1;
    if connector.status == drm_connector_status::connector_status_connected {
        my_score += 1;
    }
    // cmdline_mode.specified: never
    if drm_connector_preferred_mode(connector, width, height).is_some() {
        my_score += 1;
    }

    // select a crtc for this connector and then attempt to configure
    // remaining connectors
    for modeset in &client.modesets {
        let crtc = modeset.crtc;

        if !connector_has_possible_crtc(funcs, connector, crtc) {
            continue;
        }

        if let Some(o) = (0..n).find(|&o| best_crtcs[o] == Some(crtc)) {
            // ignore cloning unless only a single crtc
            if funcs.num_crtc() > 1 {
                continue;
            }

            if !modes[o].as_ref().is_some_and(|mo| drm_mode_equal(mo, mode_n)) {
                continue;
            }
        }

        crtcs[n] = Some(crtc);
        crtcs[..n].copy_from_slice(&best_crtcs[..n]);
        let score =
            my_score + drm_client_pick_crtcs(client, funcs, connectors, &mut crtcs, modes, n + 1, width, height);
        if score > best_score {
            best_score = score;
            best_crtcs.copy_from_slice(&crtcs);
        }
    }

    best_score
}

/// `drm_client_firmware_config`: the configuration the firmware left in
/// the atomic state, when it lit every output it could.
#[allow(clippy::too_many_arguments)]
fn drm_client_firmware_config(funcs: &dyn drm_client_funcs, connectors: &[&drm_connector],
                              crtcs: &mut [Option<u32>], modes: &mut [Option<drm_display_mode>],
                              offsets: &mut [drm_client_offset], enabled: &mut [bool], width: i32, height: i32)
    -> bool {
    let count = connectors.len().min(64);
    let mut fallback = true;
    let mut ret = true;
    let mut num_connectors_enabled = 0;
    let mut num_connectors_detected = 0;

    if !funcs.uses_atomic_modeset() {
        return false;
    }

    if count == 0 {
        return false;
    }

    let save_enabled: Vec<bool> = enabled[..count].to_vec();
    let mask: u64 = if count >= 64 { u64::MAX } else { (1u64 << count) - 1 };
    let mut conn_configured: u64 = 0;
    let num_tiled_conns = connectors
        .iter()
        .filter(|c| c.has_tile && c.status == drm_connector_status::connector_status_connected)
        .count() as i32;

    'bail: {
        loop {
            // retry:
            let conn_seq = conn_configured;
            for i in 0..count {
                let connector = &connectors[i];

                if conn_configured & (1u64 << i) != 0 {
                    continue;
                }

                if conn_seq == 0 && !connector.has_tile {
                    continue;
                }

                if connector.status == drm_connector_status::connector_status_connected {
                    num_connectors_detected += 1;
                }

                if !enabled[i] {
                    conn_configured |= 1u64 << i;
                    continue;
                }

                if connector.force == drm_connector_force::DRM_FORCE_OFF {
                    enabled[i] = false;
                    continue;
                }

                let (encoder, state_crtc) = funcs.connector_state(connector);
                let (Some(_), Some(crtc)) = (encoder, state_crtc) else {
                    if connector.force as u32 > drm_connector_force::DRM_FORCE_OFF as u32 {
                        break 'bail;
                    }

                    // has no encoder or crtc, skipping
                    enabled[i] = false;
                    conn_configured |= 1u64 << i;
                    continue;
                };

                num_connectors_enabled += 1;

                // Make sure we're not trying to drive multiple connectors
                // with a single CRTC, since our cloning support may not
                // match the BIOS.
                if crtcs[..count].contains(&Some(crtc)) {
                    // fallback: cloned configuration
                    break 'bail;
                }

                modes[i] = drm_connector_pick_cmdline_mode(connector);

                if modes[i].is_none() {
                    modes[i] = drm_connector_preferred_mode(connector, width, height);
                }

                if modes[i].is_none() {
                    modes[i] = drm_connector_first_mode(connector);
                }

                // last resort: use current mode
                if modes[i].is_none() {
                    modes[i] = Some(funcs.crtc_state_mode(crtc));
                }

                // In case of tiled modes, if all tiles are not present
                // then fallback to a non tiled mode.
                if connector.has_tile && num_tiled_conns < connector.num_h_tile as i32 * connector.num_v_tile as i32 {
                    modes[i] = drm_connector_fallback_non_tiled_mode(connector);
                }
                crtcs[i] = Some(crtc);

                fallback = false;
                conn_configured |= 1u64 << i;
            }

            if (conn_configured & mask) != mask && conn_configured != conn_seq {
                continue;
            }
            break;
        }

        for (i, connector) in connectors.iter().enumerate().take(count) {
            if connector.has_tile {
                drm_client_get_tile_offsets(connectors, modes, offsets, i, connector.tile_h_loc as i32,
                                            connector.tile_v_loc as i32);
            }
        }

        // If the BIOS didn't enable everything it could, fall back to have the
        // same user experiencing of lighting up as much as possible like the
        // fbdev helper library.
        if num_connectors_enabled != num_connectors_detected && num_connectors_enabled < funcs.num_crtc() {
            // fallback: Not all outputs enabled
            fallback = true;
        }

        if !fallback {
            return ret;
        }
    }

    // bail: Not using firmware configuration
    enabled[..count].copy_from_slice(&save_enabled);
    ret = false;
    ret
}

/// `drm_client_modeset_probe`: probe every connector of the client
/// (`fill_modes`: the probe helper, up to width x height, 0 for the
/// device's limits), then choose a mode and a CRTC for each enabled one
/// and store that in the client's modesets.
pub fn drm_client_modeset_probe(client: &mut drm_client_dev, connectors: &mut [drm_connector],
                                funcs: &mut dyn drm_client_funcs, width: u32, height: u32) -> i32 {
    let width = if width == 0 { funcs.max_width() } else { width };
    let height = if height == 0 { funcs.max_height() } else { height };

    // drm_client_for_each_connector_iter: writeback connectors are not the
    // client's.
    let idx: Vec<usize> = (0..connectors.len())
        .filter(|&i| connectors[i].connector_type != DRM_MODE_CONNECTOR_WRITEBACK)
        .collect();
    let connector_count = idx.len();
    if connector_count == 0 {
        return 0;
    }

    let mut crtcs: Vec<Option<u32>> = vec![None; connector_count];
    let mut modes: Vec<Option<drm_display_mode>> = vec![None; connector_count];
    let mut offsets: Vec<drm_client_offset> = vec![drm_client_offset::default(); connector_count];
    let mut enabled: Vec<bool> = vec![false; connector_count];

    let mut total_modes_count = 0;
    for &i in &idx {
        total_modes_count += drm_helper_probe_single_connector_modes(&mut connectors[i], width, height, funcs);
    }
    // total_modes_count == 0: "No connectors reported connected with modes"
    let _ = total_modes_count;

    let list: Vec<&drm_connector> = idx.iter().map(|&i| &connectors[i]).collect();

    drm_client_connectors_enabled(&list, &mut enabled);

    if !drm_client_firmware_config(funcs, &list, &mut crtcs, &mut modes, &mut offsets, &mut enabled, width as i32,
                                   height as i32)
    {
        modes.iter_mut().for_each(|m| *m = None);
        crtcs.iter_mut().for_each(|c| *c = None);
        offsets.iter_mut().for_each(|o| *o = drm_client_offset::default());

        if !drm_client_target_cloned(funcs, &list, &mut modes, &enabled)
            && !drm_client_target_preferred(&list, &mut modes, &mut offsets, &enabled, width as i32, height as i32)
        {
            crate::linux::Line::new().s("Unable to find initial modes").say();
        }

        drm_client_pick_crtcs(client, funcs, &list, &mut crtcs, &modes, 0, width as i32, height as i32);
    }

    client.release();

    let mut ret = 0;
    for i in 0..connector_count {
        let (Some(mode), Some(crtc)) = (&modes[i], crtcs[i]) else { continue };
        let num_crtc = funcs.num_crtc();
        let Some(modeset) = client.find_modeset(crtc) else { continue };

        if modeset.connectors.len() == DRM_CLIENT_MAX_CLONED_CONNECTORS as usize
            || (num_crtc > 1 && modeset.connectors.len() == 1)
        {
            ret = -crate::linux::EINVAL;
            break;
        }

        modeset.mode = Some(mode.clone());
        modeset.connectors.push(idx[i]);
        modeset.x = offsets[i].x as u32;
        modeset.y = offsets[i].y as u32;
    }

    ret
}
