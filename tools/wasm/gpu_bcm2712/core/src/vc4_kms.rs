//! `drivers/gpu/drm/vc4/vc4_kms.c` (Linux rpi-6.18) with the parts of
//! the DRM atomic helpers (`drm_atomic_helper.c`) a vc4 commit runs
//! through, in their order, for vc4's two pixel valves with one primary
//! plane each and the two HDMI connectors: the state duplicated from the
//! current one, `vc4_atomic_check`, then `vc4_atomic_commit_tail`.
//!
//! A commit turns outputs off (`vc4_crtc_atomic_disable`), on, or both
//! for a new mode, and updates their planes. The CTM and the load
//! tracker's limits are gen 4/5 features and stay off on gen 6, as in
//! Linux. No userspace: no writeback, no async (cursor) updates.

use crate::hw::{Hw, Spi};
use crate::linux::{EINVAL, ETIMEDOUT};
use crate::vc4_crtc::{vc4_crtc_atomic_check, vc4_crtc_mode_valid, CrtcVblank, Vc4CrtcState};
use crate::vc4_drv::Vc4;
use crate::vc4_hdmi::{vc4_hdmi_connector_atomic_check, vc4_hdmi_encoder_mode_valid};
use crate::vc4_hvs::Vc4HvsState;
use crate::vc4_plane::{vc4_plane_atomic_check, PlaneCrtc, Vc4PlaneState};
use alloc::vec::Vec;
use drm_core::drm_client_modeset::drm_client_dev;
use drm_core::drm_connector::{drm_atomic_connector_check, drm_connector_state};
use drm_core::drm_modes::{drm_mode_equal, drm_mode_vrefresh};
use drm_core::drm_plane::{drm_framebuffer, drm_plane_state};
use drm_core::drm_vblank::{
    drm_calc_timestamping_constants, drm_crtc_handle_vblank, drm_crtc_send_vblank_event, drm_crtc_vblank_get,
    drm_crtc_vblank_on, drm_crtc_vblank_put,
};
use drm_core::regs::{DRM_MODE_ROTATE_0, MODE_OK};

/// How long a commit waits for its flip (`drm_atomic_helper_wait_for_flip_done`).
const FLIP_DONE_TIMEOUT_MS: u32 = 10_000;

/// `struct drm_atomic_state` for vc4: every CRTC, plane and connector,
/// the HVS's global state and the load tracker's.
#[derive(Clone, Debug)]
pub struct Vc4AtomicState {
    pub crtcs: Vec<Vc4CrtcState>,
    pub planes: Vec<Vc4PlaneState>,
    pub connectors: Vec<drm_connector_state>,
    pub hvs: Vc4HvsState,
    /// `struct vc4_load_tracker_state`.
    pub hvs_load: u64,
    pub membus_load: u64,
}

/// The current state of every object (`crtc->state`, `plane->state`,
/// `connector->state`, the private objects').
#[derive(Clone, Debug)]
pub struct Vc4KmsState {
    pub crtcs: Vec<Vc4CrtcState>,
    pub planes: Vec<Vc4PlaneState>,
    pub connectors: Vec<drm_connector_state>,
    pub hvs: Vc4HvsState,
    pub hvs_load: u64,
    pub membus_load: u64,
    pub vblank: Vec<drm_core::drm_vblank::drm_vblank_crtc>,
}

impl Vc4KmsState {
    /// `drm_mode_config_reset`: every object's reset state.
    pub fn reset(vc4_connectors: &[drm_core::drm_connector::drm_connector], num_crtc: usize) -> Self {
        let connectors = vc4_connectors
            .iter()
            .map(|c| {
                let mut s = drm_connector_state::default();
                drm_core::drm_connector::__drm_atomic_helper_connector_hdmi_reset(c, &mut s);
                s
            })
            .collect();
        Vc4KmsState {
            crtcs: (0..num_crtc).map(|_| Vc4CrtcState::reset()).collect(),
            planes: (0..num_crtc).map(|_| Vc4PlaneState::default()).collect(),
            connectors,
            hvs: Vc4HvsState::default(),
            hvs_load: 0,
            membus_load: 0,
            vblank: (0..num_crtc).map(|_| Default::default()).collect(),
        }
    }
}

impl Vc4 {
    /// The atomic state's copies of the current states
    /// (`drm_atomic_get_*_state` for every object): a CRTC keeps its
    /// channel and margins, a plane references its LBM and UPM again.
    pub fn atomic_state_duplicate(&mut self) -> Vc4AtomicState {
        let crtcs = self
            .kms
            .crtcs
            .iter()
            .map(|old| Vc4CrtcState {
                // __drm_atomic_helper_crtc_duplicate_state
                enable: old.enable,
                active: old.active,
                mode: old.mode.clone(),
                adjusted_mode: old.adjusted_mode.clone(),
                plane_mask: old.plane_mask,
                connector_mask: old.connector_mask,
                encoder_mask: old.encoder_mask,
                background_color: old.background_color,
                // vc4_crtc_duplicate_state
                margins: old.margins,
                assigned_channel: old.assigned_channel,
                ..Default::default()
            })
            .collect();
        let planes = self.kms.planes.iter().map(|p| p.duplicate(&mut self.hvs)).collect();
        let mut hvs = Vc4HvsState::default();
        for i in 0..hvs.fifo_state.len() {
            hvs.fifo_state[i].in_use = self.kms.hvs.fifo_state[i].in_use;
            hvs.fifo_state[i].fifo_load = self.kms.hvs.fifo_state[i].fifo_load;
        }
        hvs.core_clock_rate = self.kms.hvs.core_clock_rate;
        Vc4AtomicState {
            crtcs,
            planes,
            connectors: self.kms.connectors.clone(),
            hvs,
            hvs_load: self.kms.hvs_load,
            membus_load: self.kms.membus_load,
        }
    }

    /// `drm_client_modeset_commit_atomic` of the fbdev client: every
    /// primary plane at rotation 0, each modeset's CRTC set to its mode
    /// with `fb` on the primary plane (`__drm_atomic_helper_set_config`),
    /// a CRTC without a mode turned off, then the commit.
    pub fn fbdev_commit(&mut self, hw: &mut impl Hw, client: &drm_client_dev, fb: &drm_framebuffer) -> i32 {
        self.client_commit(hw, client, fb, false)
    }

    /// The commit that takes the screen over from the firmware: like the
    /// fbdev client's, but the primary plane shows all of `fb` (the
    /// framebuffer the kernel draws into, of the firmware's size) and the
    /// HVS scales it to the mode when the sizes differ.
    pub fn takeover_commit(&mut self, hw: &mut impl Hw, client: &drm_client_dev, fb: &drm_framebuffer) -> i32 {
        self.client_commit(hw, client, fb, true)
    }

    /// `drm_atomic_helper_page_flip` on every CRTC that shows a
    /// framebuffer: the primary plane keeps its source and position and
    /// shows `fb`, a buffer of the same size and format; the commit
    /// returns once the flip is done (a blocking commit). No modeset.
    pub fn page_flip(&mut self, hw: &mut impl Hw, fb: &drm_framebuffer) -> i32 {
        let mut s = self.atomic_state_duplicate();
        let mut any = false;
        for (c, p) in s.planes.iter_mut().enumerate() {
            if !s.crtcs[c].active || p.base.fb.is_none() {
                continue;
            }
            // drm_atomic_set_fb_for_plane
            p.base.fb = Some(*fb);
            any = true;
        }
        if !any {
            self.atomic_state_put(hw, s);
            return -drm_core::linux::EINVAL;
        }
        let ret = self.atomic_check(&mut s);
        if ret != 0 {
            self.atomic_state_put(hw, s);
            return ret;
        }
        self.atomic_commit(hw, s)
    }

    fn client_commit(&mut self, hw: &mut impl Hw, client: &drm_client_dev, fb: &drm_framebuffer, whole_fb: bool)
        -> i32
    {
        let mut s = self.atomic_state_duplicate();

        for p in s.planes.iter_mut() {
            p.base.rotation = DRM_MODE_ROTATE_0;
        }

        for ms in &client.modesets {
            let c = ms.crtc as usize;
            match &ms.mode {
                None => {
                    // drm_atomic_set_mode_for_crtc(NULL), the primary
                    // plane and every connector off the CRTC.
                    s.crtcs[c].enable = false;
                    s.crtcs[c].active = false;
                    s.crtcs[c].mode = Default::default();
                    s.planes[c].base.crtc = None;
                    s.planes[c].base.fb = None;
                    for conn in s.connectors.iter_mut() {
                        if conn.crtc == Some(ms.crtc) {
                            conn.crtc = None;
                        }
                    }
                }
                Some(mode) => {
                    s.crtcs[c].enable = true;
                    s.crtcs[c].active = true;
                    s.crtcs[c].mode = mode.clone();
                    for conn in s.connectors.iter_mut() {
                        if conn.crtc == Some(ms.crtc) {
                            conn.crtc = None;
                        }
                    }
                    for &i in &ms.connectors {
                        s.connectors[i].crtc = Some(ms.crtc);
                    }
                    let (hdisplay, vdisplay) = drm_core::drm_modes::drm_mode_get_hv_timing(mode);
                    let p = &mut s.planes[c].base;
                    p.crtc = Some(ms.crtc);
                    p.fb = Some(*fb);
                    p.crtc_x = 0;
                    p.crtc_y = 0;
                    p.crtc_w = hdisplay as u32;
                    p.crtc_h = vdisplay as u32;
                    p.src_x = ms.x << 16;
                    p.src_y = ms.y << 16;
                    p.src_w = (hdisplay as u32) << 16;
                    p.src_h = (vdisplay as u32) << 16;
                    if whole_fb {
                        p.src_x = 0;
                        p.src_y = 0;
                        p.src_w = fb.width << 16;
                        p.src_h = fb.height << 16;
                    }
                }
            }
        }

        let ret = self.atomic_check(&mut s);
        if ret != 0 {
            self.atomic_state_put(hw, s);
            return ret;
        }

        self.atomic_commit(hw, s)
    }

    /// The states a commit did not keep, destroyed: a CRTC's dlist space
    /// marked stale, a plane's LBM and UPM references dropped.
    fn atomic_state_put(&mut self, hw: &mut impl Hw, s: Vc4AtomicState) {
        for c in s.crtcs {
            self.hvs.mark_dlist_entry_stale(hw, &mut self.disp_intr, c.mm);
        }
        for p in s.planes {
            p.destroy(&mut self.hvs);
        }
    }

    /// `vc4_pv_muxing_atomic_check`: a free HVS channel for each CRTC
    /// being turned on (in `hvs_output` order), its channel back for one
    /// being turned off.
    fn pv_muxing_atomic_check(&mut self, s: &mut Vc4AtomicState) -> i32 {
        let mut unassigned_channels = 0u32;
        for (i, f) in s.hvs.fifo_state.iter().enumerate() {
            if !f.in_use {
                unassigned_channels |= 1 << i;
            }
        }

        let mut sorted: Vec<usize> = (0..self.crtc.len()).collect();
        sorted.sort_by_key(|&i| self.crtc[i].data.hvs_output);

        for i in sorted {
            let old = &self.kms.crtcs[i];
            let new = &mut s.crtcs[i];

            // Nothing to do here, let's skip it
            if old.enable == new.enable {
                continue;
            }

            // Muxing will need to be modified, mark it as such
            new.update_muxing = true;

            // If we're disabling our CRTC, we put back our channel
            if !new.enable {
                if let Some(channel) = old.assigned_channel {
                    s.hvs.fifo_state[channel as usize].in_use = false;
                }
                new.assigned_channel = None;
                continue;
            }

            let matching_channels = unassigned_channels & self.crtc[i].data.hvs_available_channels;
            if matching_channels == 0 {
                return -EINVAL;
            }

            let channel = matching_channels.trailing_zeros();

            new.assigned_channel = Some(channel);
            unassigned_channels &= !(1 << channel);
            s.hvs.fifo_state[channel as usize].in_use = true;
        }

        0
    }

    /// `drm_atomic_helper_check_modeset` with vc4's callbacks: what
    /// changed per CRTC, the connectors' routing (HDMI n's encoder n) and
    /// their checks, the modes against encoder and CRTC, then the mode
    /// fixed up by the encoder.
    fn atomic_helper_check_modeset(&mut self, s: &mut Vc4AtomicState) -> i32 {
        for i in 0..s.crtcs.len() {
            let old = &self.kms.crtcs[i];
            let new = &mut s.crtcs[i];

            if !drm_mode_equal(&old.mode, &new.mode) {
                new.mode_changed = true;
            }

            if old.enable != new.enable {
                new.mode_changed = true;
            }

            if old.active != new.active {
                new.active_changed = true;
            }

            if new.enable && new.mode.clock == 0 {
                return -EINVAL;
            }
        }

        // update_connector_routing: a connector's encoder is its HDMI's.
        for c in s.crtcs.iter_mut() {
            c.connector_mask = 0;
            c.encoder_mask = 0;
        }
        for (i, conn) in s.connectors.iter_mut().enumerate() {
            let old = &self.kms.connectors[i];
            conn.best_encoder = conn.crtc.map(|_| i as u32);
            if let Some(crtc) = conn.crtc {
                let c = &mut s.crtcs[crtc as usize];
                if self.crtc[crtc as usize].data.hdmi as usize != i {
                    return -EINVAL;
                }
                c.connector_mask |= 1 << i;
                c.encoder_mask |= 1 << i;
            }
            if old.crtc != conn.crtc {
                if let Some(c) = old.crtc {
                    s.crtcs[c as usize].connectors_changed = true;
                }
                if let Some(c) = conn.crtc {
                    s.crtcs[c as usize].connectors_changed = true;
                }
            }
        }

        // The connectors' atomic_check (drm_atomic_connector_check first).
        for i in 0..s.connectors.len() {
            let connector = &self.connectors[i];
            drm_atomic_connector_check(connector, &mut s.connectors[i]);
            let Some(crtc) = s.connectors[i].crtc else { continue };
            let mode = s.crtcs[crtc as usize].mode.clone();
            match vc4_hdmi_connector_atomic_check(connector, &self.kms.connectors[i], &mut s.connectors[i], &mode,
                &self.hvs)
            {
                Ok(true) => s.crtcs[crtc as usize].mode_changed = true,
                Ok(false) => {}
                Err(e) => return e,
            }
        }

        // mode_valid: each connector's mode against its encoder and CRTC.
        for i in 0..s.connectors.len() {
            let Some(crtc) = s.connectors[i].crtc else { continue };
            let c = &s.crtcs[crtc as usize];
            if !c.needs_modeset() {
                continue;
            }
            if vc4_hdmi_encoder_mode_valid(&c.mode) != MODE_OK as i32 {
                return -EINVAL;
            }
            if vc4_crtc_mode_valid(&c.mode) != MODE_OK as i32 {
                return -EINVAL;
            }
        }

        // mode_fixup: the adjusted mode starts as the mode, the encoder
        // fixes it up.
        for c in s.crtcs.iter_mut() {
            if c.needs_modeset() {
                c.adjusted_mode = c.mode.clone();
            }
        }
        for i in 0..s.connectors.len() {
            let Some(crtc) = s.connectors[i].crtc else { continue };
            let c = &mut s.crtcs[crtc as usize];
            if !c.needs_modeset() {
                continue;
            }
            let ret = self.hdmi[i].encoder_atomic_check(&mut c.adjusted_mode);
            if ret != 0 {
                return ret;
            }
        }

        0
    }

    /// `drm_atomic_helper_check_planes`: each plane's check, then each
    /// CRTC's (`vc4_hvs_atomic_check`, then `vc4_crtc_atomic_check`).
    fn atomic_helper_check_planes(&mut self, s: &mut Vc4AtomicState) -> i32 {
        for i in 0..s.planes.len() {
            let old_enabled = {
                let o: &drm_plane_state = &self.kms.planes[i].base;
                o.fb.is_some() && o.crtc.is_some()
            };
            let crtc_state = s.planes[i].base.crtc.map(|c| &s.crtcs[c as usize]);
            let margins = s.planes[i].base.crtc.and_then(|c| {
                s.connectors.iter().find(|conn| conn.crtc == Some(c)).map(|conn| {
                    [conn.margins.left, conn.margins.right, conn.margins.top, conn.margins.bottom]
                })
            });
            let pc = crtc_state.map(|c| PlaneCrtc {
                enable: c.enable,
                mode: &c.mode,
                adjusted_mode: &c.adjusted_mode,
                margins: margins.unwrap_or(c.margins),
            });
            let mut p = core::mem::take(&mut s.planes[i]);
            let ret = vc4_plane_atomic_check(&mut p, old_enabled, &mut self.hvs, pc.as_ref());
            s.planes[i] = p;
            if ret != 0 {
                return ret;
            }
        }

        for i in 0..s.crtcs.len() {
            // The CRTC's planes: its primary.
            s.crtcs[i].plane_mask = if s.planes[i].base.crtc == Some(i as u32) { 1 << i } else { 0 };

            // vc4_hvs_atomic_check: the pixelvalve can only feed one
            // encoder (and encoders are 1:1 with connectors.)
            if s.crtcs[i].connector_mask.count_ones() > 1 {
                return -EINVAL;
            }

            let mut dlist_count = 0;
            if s.crtcs[i].plane_mask & (1 << i) != 0 {
                dlist_count += s.planes[i].dlist_size();
            }

            dlist_count += 1; // Account for SCALER_CTL0_END.

            match self.hvs.alloc_dlist_entry(s.crtcs[i].assigned_channel, dlist_count) {
                Ok(mm) => s.crtcs[i].mm = mm,
                Err(e) => return e,
            }

            let margins = s.connectors.iter().find(|c| c.crtc == Some(i as u32)).map(|c| {
                [c.margins.left, c.margins.right, c.margins.top, c.margins.bottom]
            });
            vc4_crtc_atomic_check(&self.crtc[i], &mut s.crtcs[i], margins);
        }

        0
    }

    /// `vc4_load_tracker_atomic_check`: the planes' loads summed (the
    /// limits are not enforced on gen 6).
    fn load_tracker_atomic_check(&mut self, s: &mut Vc4AtomicState) {
        for i in 0..s.planes.len() {
            let old = &self.kms.planes[i];
            if old.base.fb.is_some() && old.base.crtc.is_some() {
                s.membus_load = s.membus_load.wrapping_sub(old.membus_load);
                s.hvs_load = s.hvs_load.wrapping_sub(old.hvs_load);
            }

            let new = &s.planes[i];
            if new.base.fb.is_some() && new.base.crtc.is_some() {
                s.membus_load = s.membus_load.wrapping_add(new.membus_load);
                s.hvs_load = s.hvs_load.wrapping_add(new.hvs_load);
            }
        }
    }

    /// `vc4_core_clock_atomic_check`: the core clock the channels need.
    fn core_clock_atomic_check(&mut self, s: &mut Vc4AtomicState) {
        for i in 0..s.crtcs.len() {
            let old = &self.kms.crtcs[i];
            if old.active {
                if let Some(channel) = old.assigned_channel {
                    s.hvs.fifo_state[channel as usize].fifo_load = 0;
                }
            }

            let new = &s.crtcs[i];
            if new.active {
                if let Some(channel) = new.assigned_channel {
                    s.hvs.fifo_state[channel as usize].fifo_load = new.hvs_load;
                }
            }
        }

        let mut cob_rate = 0;
        let mut num_outputs = 0;
        for f in &s.hvs.fifo_state {
            if !f.in_use {
                continue;
            }

            num_outputs += 1;
            cob_rate = cob_rate.max(f.fifo_load);
        }

        let mut pixel_rate = s.hvs_load;
        if num_outputs > 1 {
            pixel_rate = (pixel_rate * 40) / 100;
        } else {
            pixel_rate = (pixel_rate * 60) / 100;
        }

        s.hvs.core_clock_rate = cob_rate.max(pixel_rate);
    }

    /// `vc4_atomic_check`.
    pub fn atomic_check(&mut self, s: &mut Vc4AtomicState) -> i32 {
        let ret = self.pv_muxing_atomic_check(s);
        if ret != 0 {
            return ret;
        }

        let ret = self.atomic_helper_check_modeset(s);
        if ret != 0 {
            return ret;
        }

        let ret = self.atomic_helper_check_planes(s);
        if ret != 0 {
            return ret;
        }

        self.load_tracker_atomic_check(s);
        self.core_clock_atomic_check(s);

        0
    }

    /// `drm_atomic_helper_commit` with `vc4_atomic_commit_tail`: the new
    /// state swapped in, the modes set, the planes' dlists written, the
    /// outputs turned on, then the wait for the flip.
    pub fn atomic_commit(&mut self, hw: &mut impl Hw, mut s: Vc4AtomicState) -> i32 {
        // drm_atomic_helper_setup_commit: every active CRTC in the
        // commit completes on a vblank.
        for c in s.crtcs.iter_mut() {
            c.event = c.active;
        }

        // vc4_atomic_commit_setup
        for c in &s.crtcs {
            let Some(channel) = c.assigned_channel else { continue };
            if !s.hvs.fifo_state[channel as usize].in_use {
                continue;
            }
            s.hvs.fifo_state[channel as usize].pending_commit = true;
        }

        let armed: Vec<u64> = (0..self.crtc.len()).map(|i| hw.irq_arm(Spi::Pv(self.crtc[i].pv))).collect();
        let flips: Vec<bool> = s.crtcs.iter().map(|c| c.event).collect();

        // drm_atomic_helper_swap_state
        let old_crtcs = core::mem::replace(&mut self.kms.crtcs, s.crtcs);
        let old_planes = core::mem::replace(&mut self.kms.planes, s.planes);
        let old_connectors = core::mem::replace(&mut self.kms.connectors, s.connectors);
        self.kms.hvs = s.hvs;
        self.kms.hvs_load = s.hvs_load;
        self.kms.membus_load = s.membus_load;

        // drm_atomic_helper_commit_modeset_disables: disable_outputs (the
        // HDMI encoders have no disable of their own; vc4 stops them from
        // the CRTC), the timestamping constants, then the encoders'
        // mode_set.
        for i in 0..self.crtc.len() {
            let new = &self.kms.crtcs[i];
            if old_crtcs[i].active && new.needs_modeset() {
                self.crtc_atomic_disable(hw, i, &old_crtcs[i]);
            }
        }
        for i in 0..self.crtc.len() {
            let c = &self.kms.crtcs[i];
            if c.enable {
                drm_calc_timestamping_constants(&mut self.kms.vblank[i], &c.adjusted_mode.clone());
            }
        }
        for i in 0..self.kms.connectors.len() {
            let Some(crtc) = self.kms.connectors[i].crtc else { continue };
            let c = &self.kms.crtcs[crtc as usize];
            if !c.needs_modeset() || !c.enable {
                continue;
            }
            let adjusted = c.adjusted_mode.clone();
            let conn_state = self.kms.connectors[i];
            self.hdmi[i].encoder_atomic_mode_set(&adjusted, &conn_state);
        }

        // vc6_hvs_pv_muxing_commit: only channel 1 has a mux, and both
        // of its outputs here (HDMI 1, TXP 1 not bound) select mux 0.
        for i in 0..self.crtc.len() {
            let c = &self.kms.crtcs[i];
            if !c.update_muxing || c.assigned_channel != Some(1) {
                continue;
            }
            let v = hw.read(crate::hw::Block::Hvs, crate::regs::SCALER6_CONTROL);
            hw.write(crate::hw::Block::Hvs, crate::regs::SCALER6_CONTROL,
                (v & !crate::regs::SCALER6_CONTROL_DSP1_TARGET_MASK)
                    | crate::linux::field_prep(crate::regs::SCALER6_CONTROL_DSP1_TARGET_MASK, 0));
        }

        // drm_atomic_helper_commit_planes, active CRTCs only.
        for i in 0..self.crtc.len() {
            if !self.kms.crtcs[i].active {
                continue;
            }
            // vc4_hvs_atomic_begin
            self.crtc[i].current_hvs_channel = self.kms.crtcs[i].assigned_channel.unwrap_or(u32::MAX);
        }
        for i in 0..self.crtc.len() {
            if !self.kms.crtcs[i].active {
                continue;
            }
            self.hvs_atomic_flush(hw, i, old_crtcs[i].active);
        }

        // drm_atomic_helper_commit_modeset_enables
        for i in 0..self.crtc.len() {
            let c = &self.kms.crtcs[i];
            if !c.active || !c.needs_modeset() {
                continue;
            }
            self.crtc_atomic_enable(hw, i, &old_connectors);
        }

        // drm_atomic_helper_wait_for_flip_done
        let mut ret = 0;
        for i in 0..self.crtc.len() {
            if !flips[i] {
                continue;
            }
            let mut since = armed[i];
            while self.crtc[i].event {
                if !hw.irq_wait(Spi::Pv(self.crtc[i].pv), since, FLIP_DONE_TIMEOUT_MS) {
                    drm_core::linux::Line::new().s("flip_done timed out").say();
                    ret = -ETIMEDOUT;
                    break;
                }
                since = hw.irq_arm(Spi::Pv(self.crtc[i].pv));
                self.crtc_irq_handler(hw, i);
            }
            self.kms.crtcs[i].event = false;
        }
        for f in self.kms.hvs.fifo_state.iter_mut() {
            f.pending_commit = false;
        }

        // The old states destroyed.
        for c in old_crtcs {
            self.hvs.mark_dlist_entry_stale(hw, &mut self.disp_intr, c.mm);
        }
        for p in old_planes {
            p.destroy(&mut self.hvs);
        }

        ret
    }

    /// `vc4_hvs_atomic_flush`: the CRTC's planes in zpos order into its
    /// dlist space, the end marker, the background colour; a running
    /// CRTC takes the new dlist at once.
    fn hvs_atomic_flush(&mut self, hw: &mut impl Hw, i: usize, old_active: bool) {
        let c = &self.kms.crtcs[i];
        let Some(channel) = c.assigned_channel else { return };
        let mm = c.mm.expect("an active CRTC has its dlist space");
        let bgcolor = c.background_color;
        let dlist_start = mm.node.start as u32;
        let mut dlist_next = dlist_start;
        let mut enable_bg_fill = true;

        // Copy all the active planes' dlist contents to the hardware
        // dlist; the CRTC's one plane is its primary.
        if c.plane_mask & (1 << i) != 0 {
            let p = &self.kms.planes[i];
            enable_bg_fill = p.needs_bg_fill;
            dlist_next += p.write_dlist(hw, &self.hvs, dlist_next);
        }

        self.hvs.dlist_write(hw, dlist_next, crate::regs::SCALER_CTL0_END);
        dlist_next += 1;

        debug_assert_eq!((dlist_next - dlist_start) as u64, mm.node.size);

        self.hvs.write_background(hw, channel, bgcolor, enable_bg_fill);

        // Only update DISPLIST if the CRTC was already running.
        if self.kms.crtcs[i].active && old_active {
            let mut mm = mm;
            self.hvs.install_dlist(hw, channel, &mut mm);
            self.kms.crtcs[i].mm = Some(mm);
            self.hvs_update_dlist(hw, i);
        }
    }

    /// `vc4_hvs_update_dlist`: a commit's event waits for the vblank on
    /// which the HVS takes this dlist.
    fn hvs_update_dlist(&mut self, hw: &mut impl Hw, i: usize) {
        let channel = self.kms.crtcs[i].assigned_channel.unwrap_or(u32::MAX);

        if self.kms.crtcs[i].event {
            let mut f = CrtcVblank { crtc: &self.crtc[i], hvs: &self.hvs, hw, channel };
            let _ = drm_crtc_vblank_get(&mut self.kms.vblank[i], &mut f);

            self.crtc[i].event = true;
            self.kms.crtcs[i].event = false;
        }

        self.crtc[i].current_dlist = self.kms.crtcs[i].mm.map_or(0, |m| m.node.start as u32);
    }

    /// `vc4_crtc_atomic_disable`: vblank off, the valve, HVS channel and
    /// HDMI encoder stopped (`vc4_crtc_disable`), the channel stopped
    /// again (`vc4_hvs_atomic_disable`), then 20 ms for the hardware.
    fn crtc_atomic_disable(&mut self, hw: &mut impl Hw, i: usize, old: &Vc4CrtcState) {
        let Some(channel) = old.assigned_channel else { return };
        let port = self.crtc[i].data.hdmi as usize;

        // require_hvs_enabled
        let _ = hw.read(crate::hw::Block::Hvs, crate::regs::SCALER6_CONTROL) & crate::regs::SCALER6_CONTROL_HVS_EN;

        // Disable vblank irq handling before crtc is disabled.
        {
            let mut f = CrtcVblank { crtc: &self.crtc[i], hvs: &self.hvs, hw, channel };
            drm_core::drm_vblank::drm_crtc_vblank_off(&mut self.kms.vblank[i], &mut f);
        }

        self.crtc[i].disable(hw, &self.hvs, &mut self.hdmi[port], &mut self.bsc, &mut self.clocks, channel);

        self.hvs.stop_channel(hw, channel);

        // vc4_crtc_send_vblank: the old state's event went out with its
        // flip; nobody waits here.
        hw.delay_us(20_000);
    }

    /// `vc4_crtc_atomic_enable` with `vc4_hvs_atomic_enable` and the HDMI
    /// encoder's hooks.
    fn crtc_atomic_enable(&mut self, hw: &mut impl Hw, i: usize, old_connectors: &[drm_connector_state]) {
        let channel = self.kms.crtcs[i].assigned_channel.expect("an enabled CRTC has a channel");
        let port = self.crtc[i].data.hdmi as usize;
        let adjusted = self.kms.crtcs[i].adjusted_mode.clone();

        // require_hvs_enabled
        let _ = hw.read(crate::hw::Block::Hvs, crate::regs::SCALER6_CONTROL) & crate::regs::SCALER6_CONTROL_HVS_EN;

        // Enable vblank irq handling before crtc is started otherwise
        // drm_crtc_get_vblank() fails in vc4_crtc_update_dlist().
        {
            let mut f = CrtcVblank { crtc: &self.crtc[i], hvs: &self.hvs, hw, channel };
            drm_crtc_vblank_on(&mut self.kms.vblank[i], &mut f, true);
        }

        // vc4_hvs_atomic_enable
        let mut mm = self.kms.crtcs[i].mm.expect("an active CRTC has its dlist space");
        self.hvs.install_dlist(hw, channel, &mut mm);
        self.kms.crtcs[i].mm = Some(mm);
        self.hvs_update_dlist(hw, i);
        self.hvs.init_channel(hw, channel, &adjusted, false);

        let conn_state = self.kms.connectors[port];
        self.hdmi[port].encoder_pre_crtc_configure(hw, &mut self.clocks, &conn_state);

        self.crtc[i].config_pv(hw, &adjusted);

        self.crtc[i].enable(hw);

        self.hdmi[port].encoder_pre_crtc_enable(hw, &conn_state);

        // When feeding the transposer block the pixelvalve is unneeded
        // and should not be enabled.
        self.crtc[i].video_enable(hw);

        let connector = &self.connectors[port];
        self.hdmi[port].encoder_post_crtc_enable(hw, &mut self.bsc, connector, &old_connectors[port], &conn_state,
            &self.hvs);
    }

    /// `vc4_crtc_irq_handler` for pixel valve `i`: on a vblank the count,
    /// then the page flip.
    pub fn crtc_irq_handler(&mut self, hw: &mut impl Hw, i: usize) -> bool {
        if !self.crtc[i].irq_status(hw) {
            return false;
        }

        // vc4_crtc_handle_vblank
        self.crtc[i].t_vblank = hw.now_us() as i64 * 1000;

        let channel = self.kms.crtcs[i].assigned_channel.unwrap_or(u32::MAX);
        {
            let mut f = CrtcVblank { crtc: &self.crtc[i], hvs: &self.hvs, hw, channel };
            drm_crtc_handle_vblank(&mut self.kms.vblank[i], &mut f);
        }

        if self.crtc[i].handle_page_flip(hw, &self.hvs) {
            let _ = drm_crtc_send_vblank_event(&self.kms.vblank[i]);
            let mut f = CrtcVblank { crtc: &self.crtc[i], hvs: &self.hvs, hw, channel };
            drm_crtc_vblank_put(&mut self.kms.vblank[i], &mut f);
        }

        true
    }

    /// The refresh rate of an output, for the module's frame pacing.
    pub fn vrefresh(&self, i: usize) -> i32 {
        drm_mode_vrefresh(&self.kms.crtcs[i].adjusted_mode)
    }
}
