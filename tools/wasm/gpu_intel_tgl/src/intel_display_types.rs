//! display/intel_display_types.h (Linux 6.18.55): the hardware side of
//! `struct intel_crtc`, `intel_crtc_state`, `intel_plane` and
//! `intel_plane_state` that the readout fills. DRM object bookkeeping
//! (`uapi` blobs, properties) is not kept.

use crate::regs::*;

/// Planes per pipe on display version 12: PLANE_1..PLANE_7 and the cursor.
pub const MAX_PLANES: usize = I915_MAX_PIPES as usize * I915_MAX_PLANES as usize;

/// `struct drm_rect`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct drm_rect {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

impl drm_rect {
    /// `drm_rect_init`.
    pub fn init(x: i32, y: i32, width: i32, height: i32) -> Self {
        drm_rect { x1: x, y1: y, x2: x + width, y2: y + height }
    }

    /// `drm_rect_width`.
    pub fn width(&self) -> i32 {
        self.x2 - self.x1
    }

    /// `drm_rect_height`.
    pub fn height(&self) -> i32 {
        self.y2 - self.y1
    }

    /// `drm_rect_translate_to`.
    pub fn translate_to(&mut self, x: i32, y: i32) {
        let (w, h) = (self.width(), self.height());
        *self = drm_rect::init(x, y, w, h);
    }
}

/// `struct drm_display_mode`, timings only.
#[derive(Clone, Copy, Default)]
pub struct drm_display_mode {
    pub clock: i32,
    pub hdisplay: i32,
    pub hsync_start: i32,
    pub hsync_end: i32,
    pub htotal: i32,
    pub vdisplay: i32,
    pub vsync_start: i32,
    pub vsync_end: i32,
    pub vtotal: i32,
    pub flags: u32,
    pub type_: u32,
    pub crtc_clock: i32,
    pub crtc_hdisplay: i32,
    pub crtc_hblank_start: i32,
    pub crtc_hblank_end: i32,
    pub crtc_hsync_start: i32,
    pub crtc_hsync_end: i32,
    pub crtc_htotal: i32,
    pub crtc_vdisplay: i32,
    pub crtc_vblank_start: i32,
    pub crtc_vblank_end: i32,
    pub crtc_vsync_start: i32,
    pub crtc_vsync_end: i32,
    pub crtc_vtotal: i32,
}

/// `struct intel_csc_matrix`.
#[derive(Clone, Copy, Default)]
pub struct intel_csc_matrix {
    pub coeff: [u16; 9],
    pub preoff: [u16; 3],
    pub postoff: [u16; 3],
}

/// `struct intel_scaler`, the part the readout sets.
#[derive(Clone, Copy, Default)]
pub struct intel_scaler {
    pub in_use: bool,
}

/// `struct intel_crtc_scaler_state`.
#[derive(Clone, Copy, Default)]
pub struct intel_crtc_scaler_state {
    pub scalers: [intel_scaler; SKL_NUM_SCALERS as usize],
    pub scaler_users: u32,
    pub scaler_id: i32,
}

/// `crtc_state->pch_pfit`.
#[derive(Clone, Copy, Default)]
pub struct intel_pch_pfit {
    pub dst: drm_rect,
    pub enabled: bool,
    pub force_thru: bool,
}

/// `crtc_state->dsc`, without the PPS (`intel_dsc_get_pps_config` is not
/// ported).
#[derive(Clone, Copy, Default)]
pub struct intel_crtc_dsc {
    pub compression_enable: bool,
    pub num_streams: i32,
}

/// `crtc_state->vrr`.
#[derive(Clone, Copy, Default)]
pub struct intel_crtc_vrr {
    pub enable: bool,
    pub pipeline_full: u8,
    pub flipline: u16,
    pub vmin: u16,
    pub vmax: u16,
    pub guardband: u16,
}

/// `crtc_state->splitter` (eDP MSO). Only an encoder's readout sets it.
#[derive(Clone, Copy, Default)]
pub struct intel_crtc_splitter {
    pub enable: bool,
    pub link_count: i32,
    pub pixel_overlap: i32,
}

/// Placeholder for `struct intel_dpll_hw_state`: the DPLL port
/// (intel_dpll_mgr.rs) owns the real type and replaces this one.
#[derive(Clone, Copy, Default)]
pub struct intel_dpll_hw_state_placeholder {
    pub raw: [u32; 16],
}

/// `crtc_state->hw`.
#[derive(Clone, Copy, Default)]
pub struct intel_crtc_state_hw {
    pub active: bool,
    pub enable: bool,
    pub mode: drm_display_mode,
    pub pipe_mode: drm_display_mode,
    pub adjusted_mode: drm_display_mode,
}

/// `struct intel_crtc_state`, the members the hardware readout fills.
#[derive(Clone, Copy, Default)]
pub struct intel_crtc_state {
    /// `uapi.active`, `uapi.enable`, `uapi.plane_mask`.
    pub uapi_active: bool,
    pub uapi_enable: bool,
    pub uapi_plane_mask: u32,
    pub hw: intel_crtc_state_hw,
    pub cpu_transcoder: u32,
    pub pipe_src: drm_rect,
    pub pixel_rate: u32,
    pub pipe_bpp: i32,
    pub output_format: u32,
    pub sink_format: u32,
    pub output_types: u32,
    pub port_clock: i32,
    pub pixel_multiplier: u32,
    pub framestart_delay: u32,
    pub linetime: u16,
    pub ips_linetime: u16,
    pub ips_enabled: bool,
    pub joiner_pipes: u8,
    pub mode_flags: u32,
    pub pch_pfit: intel_pch_pfit,
    pub scaler_state: intel_crtc_scaler_state,
    pub dsc: intel_crtc_dsc,
    pub vrr: intel_crtc_vrr,
    pub splitter: intel_crtc_splitter,
    pub gamma_mode: u32,
    pub csc_mode: u32,
    pub gamma_enable: bool,
    pub csc_enable: bool,
    pub csc: intel_csc_matrix,
    pub output_csc: intel_csc_matrix,
    pub c8_planes: u8,
    pub enabled_planes: u8,
    pub active_planes: u8,
    /// `shared_dpll`: the id of the DPLL the encoder readout finds.
    pub shared_dpll: Option<u32>,
    pub dpll_hw_state: intel_dpll_hw_state_placeholder,
}

/// `struct intel_crtc`.
#[derive(Clone, Copy, Default)]
pub struct intel_crtc {
    pub pipe: u32,
    pub num_scalers: u32,
    pub plane_ids_mask: u8,
    /// `crtc->hw_readout_power_domains`.
    pub hw_readout_power_domains: crate::intel_display_power::intel_display_power_domain_set,
    /// `crtc->base.state`.
    pub state: intel_crtc_state,
}

/// How a plane reads its enable state (`plane->get_hw_state`).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum plane_kind {
    #[default]
    skl_universal,
    i9xx_cursor,
}

/// `struct intel_plane`.
#[derive(Clone, Copy, Default)]
pub struct intel_plane {
    pub pipe: u32,
    pub id: u32,
    pub i9xx_plane: u32,
    pub frontbuffer_bit: u32,
    /// The FBC instance (`plane->fbc`), when the plane has one.
    pub fbc_id: Option<u32>,
    pub kind: plane_kind,
    /// `plane->base.state`.
    pub state: intel_plane_state,
}

/// `struct intel_plane_state`, the readout part.
#[derive(Clone, Copy, Default)]
pub struct intel_plane_state {
    /// `uapi.visible`.
    pub visible: bool,
}
