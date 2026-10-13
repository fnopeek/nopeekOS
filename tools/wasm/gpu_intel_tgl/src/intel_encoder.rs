//! The output objects of display/intel_display_types.h (Linux 6.18.55):
//! `struct intel_encoder`, `intel_digital_port`, `intel_dp`, `intel_hdmi`,
//! `intel_connector` and the TypeC port state of intel_tc.c, plus the
//! encoder helpers of intel_display.c, for a Tiger Lake display.
//!
//! Linux allocates each object and links them by pointer; here they live
//! in fixed arrays in `i915.display.outputs` and refer to each other by
//! index, in the order Linux adds them to its lists. The vtable a Linux
//! encoder carries is fixed on Tiger Lake (DDI or fake MST), so an encoder
//! records which of the two it is instead.

use crate::regs::*;

/// Encoders: one DDI per VBT port, plus one fake MST encoder per pipe for
/// every DP port.
pub const MAX_ENCODERS: usize = 32;
pub const MAX_DIG_PORTS: usize = I915_MAX_PORTS as usize;
pub const MAX_CONNECTORS: usize = I915_MAX_PORTS as usize * 2;
const NAME_LEN: usize = 24;

/// What an encoder's vtable is.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum encoder_kind {
    #[default]
    none,
    /// `intel_ddi_funcs`; the index is the digital port.
    ddi(usize),
    /// `mst_stream_encoder_funcs`: primary digital port and pipe.
    dp_mst(usize, u32),
}

/// `encoder->get_config` / `enable_clock` choice of `intel_ddi_init`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum ddi_clock_ops {
    #[default]
    none,
    /// `icl_ddi_combo_*`.
    icl_ddi_combo,
    /// `icl_ddi_tc_*`, with `icl_ddi_tc_port_pll_type`.
    icl_ddi_tc,
}

/// `encoder->set_signal_levels` / `get_buf_trans` choice.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum ddi_phy_ops {
    #[default]
    none,
    /// `icl_combo_phy_set_signal_levels`, `tgl_get_combo_buf_trans`.
    tgl_combo,
    /// `tgl_dkl_phy_set_signal_levels`, `tgl_get_dkl_buf_trans`.
    tgl_dkl,
}

/// `struct intel_encoder`.
#[derive(Clone, Copy, Default)]
pub struct intel_encoder {
    pub kind: encoder_kind,
    pub name: [u8; NAME_LEN],
    pub name_len: usize,
    /// `enum intel_output_type`.
    pub type_: u32,
    pub port: u32,
    pub power_domain: u32,
    pub pipe_mask: u32,
    pub cloneable: u32,
    pub hpd_pin: u32,
    /// Index into the VBT's display devices (`encoder->devdata`).
    pub devdata: usize,
    pub clock_ops: ddi_clock_ops,
    pub phy_ops: ddi_phy_ops,
    /// `encoder->base.possible_crtcs` / `possible_clones`.
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}

impl intel_encoder {
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }

    pub fn set_name(&mut self, parts: &[&str]) {
        self.name_len = 0;
        for p in parts {
            for &b in p.as_bytes() {
                if self.name_len < NAME_LEN {
                    self.name[self.name_len] = b;
                    self.name_len += 1;
                }
            }
        }
    }
}

/// `enum tc_port_mode` with the TypeC port state of `struct intel_tc_port`.
#[derive(Clone, Copy, Default)]
pub struct intel_tc_port {
    pub legacy_port: bool,
    pub mode: u32,
    pub init_mode: u32,
    pub phy_fia: u32,
    pub pin_assignment: u32,
    pub phy_fia_idx: u32,
    pub max_lane_count: u32,
    pub link_refcount: i32,
    /// `tc->lock_wakeref`: a reference on the TC-cold-off domain is held.
    pub lock_wakeref: bool,
    /// `tc->lock` is held (the port is between lock and unlock).
    pub locked: bool,
}

/// `struct intel_dp_link_config`.
#[derive(Clone, Copy, Default)]
pub struct intel_dp_link_config {
    pub link_rate_idx: u8,
    pub lane_count_exp: u8,
}

/// `intel_dp->link`.
#[derive(Clone, Copy)]
pub struct intel_dp_link {
    pub max_lane_count: i32,
    pub max_rate: i32,
    pub mst_probed_lane_count: i32,
    pub mst_probed_rate: i32,
    pub retrain_disabled: bool,
    pub seq_train_failures: i32,
    pub num_configs: usize,
    pub configs: [intel_dp_link_config; INTEL_DP_MAX_LINK_CONFIGS as usize],
    pub active: bool,
}

impl Default for intel_dp_link {
    fn default() -> Self {
        intel_dp_link {
            max_lane_count: 0,
            max_rate: 0,
            mst_probed_lane_count: 0,
            mst_probed_rate: 0,
            retrain_disabled: false,
            seq_train_failures: 0,
            num_configs: 0,
            configs: [intel_dp_link_config::default(); INTEL_DP_MAX_LINK_CONFIGS as usize],
            active: false,
        }
    }
}

/// `struct intel_dp`, the fields its init and the readout set.
#[derive(Clone, Copy, Default)]
pub struct intel_dp {
    /// `output_reg`; 0 = `INVALID_MMIO_REG`.
    pub output_reg: u32,
    pub DP: u32,
    pub reset_link_params: bool,
    pub attached_connector: Option<usize>,
    pub aux_ch_ctl_reg: u32,
    pub aux_ch_data_reg: [u32; 5],
    pub sink_rates: [i32; DP_MAX_SUPPORTED_RATES as usize],
    pub num_sink_rates: usize,
    pub max_sink_lane_count: i32,
    pub source_rates: &'static [i32],
    pub num_source_rates: usize,
    pub common_rates: [i32; DP_MAX_SUPPORTED_RATES as usize],
    pub num_common_rates: usize,
    pub link: intel_dp_link,
    /// `intel_dp_mst_source_support`: `mst.mgr.cbs` is set.
    pub mst_source_support: bool,
    pub mst_stream_encoders: [Option<usize>; I915_MAX_PIPES as usize],
    /// `dpcd[DP_DPCD_REV]`; nothing reads the DPCD yet.
    pub dpcd_rev: u8,
    pub is_mst: bool,
    pub mso_link_count: i32,
    pub port_clock: i32,
    pub lane_count: i32,
}

/// `struct intel_hdmi`.
#[derive(Clone, Copy, Default)]
pub struct intel_hdmi {
    /// `hdmi_reg`; 0 = `INVALID_MMIO_REG`.
    pub hdmi_reg: u32,
    pub attached_connector: Option<usize>,
}

/// `dig_port->connected`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum connected_fn {
    #[default]
    none,
    intel_tc_port_connected,
    lpt_digital_port_connected,
}

/// `dig_port->write_infoframe` & co.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum infoframe_ops {
    #[default]
    none,
    hsw,
    lspcon,
}

/// `struct intel_digital_port`.
#[derive(Clone, Copy, Default)]
pub struct intel_digital_port {
    /// Index of `dig_port->base`.
    pub encoder: usize,
    pub lane_reversal: bool,
    pub ddi_a_4_lanes: bool,
    pub max_lanes: i32,
    /// `enum aux_ch`.
    pub aux_ch: i32,
    pub ddi_io_power_domain: u32,
    pub tc: Option<intel_tc_port>,
    pub dp: intel_dp,
    pub hdmi: intel_hdmi,
    pub connected: connected_fn,
    pub infoframes: infoframe_ops,
    /// `dig_port->ddi_io_wakeref` is held.
    pub ddi_io_wakeref: bool,
    /// `dig_port->aux_wakeref`: the domain it holds, if any.
    pub aux_wakeref: Option<u32>,
}

/// `struct intel_connector`.
#[derive(Clone, Copy, Default)]
pub struct intel_connector {
    /// `DRM_MODE_CONNECTOR_*`.
    pub connector_type: u32,
    pub encoder: usize,
    pub polled: u32,
    /// `connector->base.ddc`: the GMBUS pin of an HDMI connector, 0 for DP
    /// (whose DDC is the AUX channel).
    pub ddc_pin: u8,
}

/// The encoder, digital port and connector lists of `struct drm_device`.
#[derive(Clone, Copy)]
pub struct intel_outputs {
    pub encoders: [intel_encoder; MAX_ENCODERS],
    pub n_encoders: usize,
    pub dig_ports: [intel_digital_port; MAX_DIG_PORTS],
    pub n_dig_ports: usize,
    pub connectors: [intel_connector; MAX_CONNECTORS],
    pub n_connectors: usize,
}

impl Default for intel_outputs {
    fn default() -> Self {
        intel_outputs {
            encoders: [intel_encoder::default(); MAX_ENCODERS],
            n_encoders: 0,
            dig_ports: [intel_digital_port::default(); MAX_DIG_PORTS],
            n_dig_ports: 0,
            connectors: [intel_connector::default(); MAX_CONNECTORS],
            n_connectors: 0,
        }
    }
}

impl intel_outputs {
    /// `drm_encoder_init`: append to the encoder list.
    pub fn add_encoder(&mut self, e: intel_encoder) -> Option<usize> {
        if self.n_encoders == MAX_ENCODERS {
            crate::linux::drm_err("encoder list is full");
            return None;
        }
        self.encoders[self.n_encoders] = e;
        self.n_encoders += 1;
        Some(self.n_encoders - 1)
    }

    /// `intel_connector_alloc` + `drm_connector_init`.
    pub fn add_connector(&mut self, c: intel_connector) -> Option<usize> {
        if self.n_connectors == MAX_CONNECTORS {
            crate::linux::drm_err("connector list is full");
            return None;
        }
        self.connectors[self.n_connectors] = c;
        self.n_connectors += 1;
        Some(self.n_connectors - 1)
    }

    /// `enc_to_dig_port`.
    pub fn dig_port_of(&self, encoder: usize) -> Option<usize> {
        match self.encoders[encoder].kind {
            encoder_kind::ddi(d) => Some(d),
            _ => None,
        }
    }
}

// ── intel_display.c encoder helpers (display version 12, Tiger Lake) ───

/// `intel_phy_is_combo`.
pub fn intel_phy_is_combo(phy: i32) -> bool {
    phy != PHY_NONE && phy as u32 <= PHY_B
}

/// `intel_phy_is_tc`: Tiger Lake, integrated.
pub fn intel_phy_is_tc(phy: i32) -> bool {
    phy >= PHY_D as i32 && phy <= PHY_I as i32
}

/// `intel_port_to_phy`: none of the version 13, ADL-S, DG1, RKL or
/// JSL/EHL cases.
pub fn intel_port_to_phy(port: u32) -> i32 {
    (PHY_A + port - PORT_A) as i32
}

/// `intel_port_to_tc`.
pub fn intel_port_to_tc(port: u32) -> i32 {
    if !intel_phy_is_tc(intel_port_to_phy(port)) {
        return TC_PORT_NONE;
    }
    (TC_PORT_1 + port - PORT_TC1) as i32
}

pub fn intel_encoder_to_phy(e: &intel_encoder) -> i32 {
    intel_port_to_phy(e.port)
}

pub fn intel_encoder_is_combo(e: &intel_encoder) -> bool {
    intel_phy_is_combo(intel_encoder_to_phy(e))
}

pub fn intel_encoder_is_tc(e: &intel_encoder) -> bool {
    intel_phy_is_tc(intel_encoder_to_phy(e))
}

pub fn intel_encoder_to_tc(e: &intel_encoder) -> i32 {
    intel_port_to_tc(e.port)
}

/// `intel_encoder_is_dp`.
pub fn intel_encoder_is_dp(o: &intel_outputs, encoder: usize) -> bool {
    let e = &o.encoders[encoder];
    match e.type_ {
        INTEL_OUTPUT_DP | INTEL_OUTPUT_EDP => true,
        INTEL_OUTPUT_DDI => match o.dig_port_of(encoder) {
            // Skip pure HDMI/DVI DDI encoders.
            Some(d) => o.dig_ports[d].dp.output_reg != 0,
            None => false,
        },
        _ => false,
    }
}

pub fn port_name(port: u32) -> [u8; 1] {
    [b'A' + port as u8]
}

/// The crtc state the encoder hooks read (`struct intel_crtc_state`): the
/// readout fills it from the crtc it attaches the encoder to.
#[derive(Clone, Copy, Default)]
pub struct encoder_crtc_state {
    pub active: bool,
    /// `crtc_state->output_types`, a mask of `BIT(INTEL_OUTPUT_*)`.
    pub output_types: u32,
    pub port_clock: i32,
    pub lane_count: i32,
    /// `icl_ddi_tc_pll_is_tbt(crtc_state->intel_dpll)`, None without a PLL.
    pub pll_is_tbt: Option<bool>,
}

impl encoder_crtc_state {
    /// `intel_crtc_has_type`.
    pub fn has_type(&self, t: u32) -> bool {
        self.output_types & (1 << t) != 0
    }

    /// `intel_crtc_has_dp_encoder`.
    pub fn has_dp_encoder(&self) -> bool {
        self.has_type(INTEL_OUTPUT_DP) || self.has_type(INTEL_OUTPUT_DP_MST) || self.has_type(INTEL_OUTPUT_EDP)
    }
}
