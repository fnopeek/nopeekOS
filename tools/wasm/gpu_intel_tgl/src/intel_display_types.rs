//! display/intel_display_types.h (Linux 6.18.55): the hardware side of
//! `struct intel_crtc`, `intel_crtc_state`, `intel_plane` and
//! `intel_plane_state` that the readout fills. DRM object bookkeeping
//! (`uapi` blobs, properties) is not kept.

use crate::intel_dpll_mgr::intel_dpll_hw_state;
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

/// `struct intel_link_m_n`.
#[derive(Clone, Copy, Default)]
pub struct intel_link_m_n {
    pub tu: u32,
    pub data_m: u32,
    pub data_n: u32,
    pub link_m: u32,
    pub link_n: u32,
}

/// `struct icl_port_dpll`; `pll` indexes `i915.display.dpll.dplls`.
#[derive(Clone, Copy, Default)]
pub struct icl_port_dpll {
    pub pll: Option<usize>,
    pub hw_state: intel_dpll_hw_state,
}

/// `crtc_state->infoframes`: which DIPs are on, and the GCP. The DIP
/// payloads are not read back (see intel_ddi_get_config).
#[derive(Clone, Copy, Default)]
pub struct intel_crtc_infoframes {
    pub enable: u32,
    pub gcp: u32,
}

/// `struct skl_ddb_entry`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct skl_ddb_entry {
    pub start: u16,
    pub end: u16,
}

/// `struct skl_wm_level`.
#[derive(Clone, Copy, Default)]
pub struct skl_wm_level {
    pub min_ddb_alloc: u16,
    pub blocks: u16,
    pub lines: u8,
    pub enable: bool,
    pub ignore_lines: bool,
    pub auto_min_alloc_wm_enable: bool,
    pub can_sagv: bool,
}

/// `struct skl_plane_wm`; `sagv` holds `sagv.wm0` and `sagv.trans_wm`.
#[derive(Clone, Copy, Default)]
pub struct skl_plane_wm {
    pub wm: [skl_wm_level; crate::skl_watermark::NUM_LATENCY],
    pub uv_wm: [skl_wm_level; crate::skl_watermark::NUM_LATENCY],
    pub trans_wm: skl_wm_level,
    pub sagv_wm0: skl_wm_level,
    pub sagv_trans_wm: skl_wm_level,
    pub is_planar: bool,
}

/// `struct skl_pipe_wm`.
#[derive(Clone, Copy, Default)]
pub struct skl_pipe_wm {
    pub planes: [skl_plane_wm; I915_MAX_PLANES as usize],
    pub use_sagv_wm: bool,
}

/// `crtc_state->wm.skl`.
#[derive(Clone, Copy, Default)]
pub struct intel_crtc_wm_skl {
    pub optimal: skl_pipe_wm,
    pub raw: skl_pipe_wm,
    pub ddb: skl_ddb_entry,
    pub plane_ddb: [skl_ddb_entry; I915_MAX_PLANES as usize],
    pub plane_ddb_y: [skl_ddb_entry; I915_MAX_PLANES as usize],
    pub plane_min_ddb: [u16; I915_MAX_PLANES as usize],
    pub plane_interim_ddb: [u16; I915_MAX_PLANES as usize],
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
    /// `uapi.encoder_mask` / `uapi.connector_mask`, by list index.
    pub uapi_encoder_mask: u32,
    pub uapi_connector_mask: u32,
    /// `intel_dpll`: index into `i915.display.dpll.dplls`.
    pub intel_dpll: Option<usize>,
    pub dpll_hw_state: intel_dpll_hw_state,
    pub icl_port_dplls: [icl_port_dpll; ICL_PORT_DPLL_COUNT as usize],
    pub has_pch_encoder: bool,
    pub double_wide: bool,
    pub lane_count: i32,
    pub has_audio: bool,
    pub has_hdmi_sink: bool,
    pub has_infoframe: bool,
    pub hdmi_scrambling: bool,
    pub hdmi_high_tmds_clock_ratio: bool,
    pub enhanced_framing: bool,
    pub fec_enable: bool,
    pub has_psr: bool,
    pub mst_master_transcoder: u32,
    pub master_transcoder: u32,
    pub sync_mode_slaves_mask: u8,
    pub dp_m_n: intel_link_m_n,
    pub dp_m2_n2: intel_link_m_n,
    pub infoframes: intel_crtc_infoframes,
    pub min_voltage_level: u8,
    /// Set by the readout: the next commit recomputes the derived state.
    pub inherited: bool,
    pub data_rate: [u32; I915_MAX_PLANES as usize],
    pub min_cdclk: [i32; I915_MAX_PLANES as usize],
    pub wm_skl: intel_crtc_wm_skl,
}

impl intel_crtc_state {
    /// `intel_crtc_has_type`.
    pub fn has_type(&self, t: u32) -> bool {
        self.output_types & (1 << t) != 0
    }

    /// `intel_crtc_has_dp_encoder`.
    pub fn has_dp_encoder(&self) -> bool {
        self.has_type(INTEL_OUTPUT_DP) || self.has_type(INTEL_OUTPUT_DP_MST) || self.has_type(INTEL_OUTPUT_EDP)
    }

    /// `icl_ddi_tc_pll_is_tbt(crtc_state->intel_dpll)`. The DPLL array
    /// holds `tgl_plls` in table order, so the index names the PLL.
    pub fn pll_is_tbt(&self) -> Option<bool> {
        self.intel_dpll.map(|i| crate::tables::tgl_plls[i].id == DPLL_ID_ICL_TBTPLL)
    }
}

/// `struct intel_crtc`.
#[derive(Clone, Copy, Default)]
pub struct intel_crtc {
    pub pipe: u32,
    pub num_scalers: u32,
    pub plane_ids_mask: u8,
    /// `crtc->hw_readout_power_domains`.
    pub hw_readout_power_domains: crate::intel_display_power::intel_display_power_domain_set,
    /// `crtc->enabled_power_domains`.
    pub enabled_power_domains: crate::intel_display_power::intel_display_power_domain_set,
    /// `crtc->active`, `crtc->base.enabled`.
    pub active: bool,
    pub enabled: bool,
    pub vmax_vblank_start: i32,
    pub mode_flags: u32,
    pub scanline_offset: i32,
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

/// `struct intel_plane_state`: the readout part and what taking over the
/// firmware framebuffer sets. `intel_plane_copy_uapi_to_hw_state` makes
/// `hw` a copy of `uapi` for these fields, so they are kept once.
#[derive(Clone, Copy, Default)]
pub struct intel_plane_state {
    /// `uapi.visible`.
    pub visible: bool,
    /// `uapi.fb` / `hw.fb`.
    pub fb: Option<crate::intel_plane_initial::intel_initial_fb>,
    /// `uapi.crtc` / `hw.crtc`: the pipe.
    pub crtc: Option<u32>,
    pub rotation: u32,
    /// `uapi.src_x/y/w/h`, 16.16 fixed point.
    pub src: [u32; 4],
    /// `uapi.crtc_x/y/w/h`.
    pub dst: [i32; 4],
    pub ggtt_vma: Option<crate::intel_ggtt::i915_vma>,
    /// `plane_state->surf`: the GGTT offset the plane scans out.
    pub surf: u32,
    /// `PLANE_HAS_FENCE`.
    pub flags: u32,
}
