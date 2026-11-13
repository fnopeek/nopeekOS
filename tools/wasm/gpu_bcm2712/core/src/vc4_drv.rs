//! `drivers/gpu/drm/vc4/vc4_drv.c` (Linux rpi-6.18): the display bound as
//! one device, in the order Linux brings its parts up: the L2 interrupt
//! controllers, the firmware clocks and the DDC controllers (built-in and
//! module drivers that probe first), then `vc4_drm_bind` with its
//! components, `vc4_kms_load` and the takeover of what the firmware left
//! running.
//!
//! The fbdev client's probe ([`Vc4::fbdev_probe`]) runs the DRM core's
//! `drm_client_modeset_probe` against vc4's connector, encoder and CRTC
//! callbacks.
//!
//! Not bound: the writeback engines (`vc4_txp`, MOP and MOPLET), which
//! write memory outside the HVS's IOMMU; their `disp_intr` lines 0 and 1
//! stay masked.

use crate::clk::FwClocks;
use crate::hw::{Block, Hw};
use crate::i2c_brcmstb::Bsc;
use crate::irq_brcmstb_l2::{L2Intc, L2_2711_LVL_INTC_INIT, L2_LVL_INTC_INIT};
use crate::vc4_crtc::{vc4_crtc_mode_valid, Vc4Crtc};
use crate::vc4_hdmi::{
    vc4_hdmi_connector_get_modes, vc4_hdmi_connector_init, vc4_hdmi_connector_mode_valid,
    vc4_hdmi_encoder_mode_valid, Vc4Hdmi,
};
use crate::vc4_hvs::Hvs;
use alloc::vec;
use alloc::vec::Vec;
use drm_core::drm_client_modeset::{drm_client_dev, drm_client_funcs, drm_client_modeset_probe};
use drm_core::drm_connector::{drm_connector, drm_connector_status};
use drm_core::drm_modes::{drm_display_mode, drm_mode_status};
use drm_core::drm_probe_helper::drm_probe_funcs;

/// `clock-frequency` of `ddc0`/`ddc1`, and their lines on `bsc_irq`.
const DDC_CLOCK_FREQUENCY: u32 = 97_500;
const DDC_IRQ: [u32; 2] = [1, 2];

/// `dev->mode_config.max_width` / `max_height` from generation 6 on
/// (`vc4_kms_load`).
const MAX_WIDTH: u32 = 8192;
const MAX_HEIGHT: u32 = 8192;

/// `struct vc4_dev` with the components bound to it.
pub struct Vc4 {
    pub aon_intr: L2Intc,
    pub disp_intr: L2Intc,
    pub bsc: Bsc,
    pub clocks: FwClocks,
    pub hvs: Hvs,
    pub hdmi: [Vc4Hdmi; 2],
    pub crtc: [Vc4Crtc; 2],
    /// The connectors in device order: HDMI-A-1 and HDMI-A-2. Encoder
    /// `n` is HDMI `n`'s, CRTC `n` pixel valve `n`; Linux's writeback
    /// CRTCs and connectors are not bound.
    pub connectors: Vec<drm_connector>,
}

impl Vc4 {
    /// Everything up to the fbdev client's first probe.
    pub fn bind(hw: &mut impl Hw) -> Result<Self, i32> {
        let mut aon_intr = L2Intc::probe(hw, Block::AonIntr, &L2_2711_LVL_INTC_INIT);
        let mut disp_intr = L2Intc::probe(hw, Block::DispIntr, &L2_2711_LVL_INTC_INIT);
        let bsc_irq = L2Intc::probe(hw, Block::BscIrq, &L2_LVL_INTC_INIT);
        let mut clocks = FwClocks::register(hw)?;
        let mut bsc = Bsc::new(bsc_irq);
        for n in 0..2 {
            bsc.probe(hw, n, DDC_IRQ[n], DDC_CLOCK_FREQUENCY);
        }
        // vc4_drm_bind: the firmware stops driving the display first.
        hw.fw_display_done();
        let hvs = Hvs::bind(hw, &mut disp_intr, &mut clocks)?;
        let hdmi = [
            Vc4Hdmi::bind(hw, 0, &mut aon_intr, &mut clocks)?,
            Vc4Hdmi::bind(hw, 1, &mut aon_intr, &mut clocks)?,
        ];
        let crtc = [Vc4Crtc::bind(hw, 0), Vc4Crtc::bind(hw, 1)];
        let connectors = vec![vc4_hdmi_connector_init(0), vc4_hdmi_connector_init(1)];
        let mut vc4 = Vc4 { aon_intr, disp_intr, bsc, clocks, hvs, hdmi, crtc, connectors };
        // vc4_kms_load: drm_mode_config_reset, then every output the
        // firmware left running stopped.
        for i in 0..2 {
            let hdmi = &mut vc4.hdmi[vc4.crtc[i].data.hdmi as usize];
            vc4.crtc[i].disable_at_boot(hw, &vc4.hvs, hdmi, &mut vc4.bsc, &mut vc4.clocks)?;
        }
        Ok(vc4)
    }

    /// `drm_client_modeset_probe` of the fbdev client
    /// (`drm_fb_helper_initial_config`): every connector detected and
    /// its modes validated, a mode and a CRTC chosen for each one in use.
    pub fn fbdev_probe(&mut self, hw: &mut impl Hw) -> drm_client_dev {
        let mut connectors = core::mem::take(&mut self.connectors);
        let mut client = drm_client_dev::new(self.crtc.len() as u32);
        drm_client_modeset_probe(&mut client, &mut connectors, &mut Kms { vc4: self, hw }, 0, 0);
        self.connectors = connectors;
        client
    }
}

/// The device as the DRM core's probe sees it.
struct Kms<'a, H: Hw> {
    vc4: &'a mut Vc4,
    hw: &'a mut H,
}

impl<H: Hw> drm_probe_funcs for Kms<'_, H> {
    fn detect(&mut self, connector: &mut drm_connector, _force: bool) -> drm_connector_status {
        let v = &mut *self.vc4;
        v.hdmi[connector.index as usize].connector_detect_ctx(self.hw, connector, &mut v.bsc, &mut v.clocks)
    }

    fn get_modes(&mut self, connector: &mut drm_connector) -> i32 {
        vc4_hdmi_connector_get_modes(connector, &self.vc4.hvs)
    }

    fn mode_valid(&mut self, connector: &drm_connector, mode: &drm_display_mode) -> drm_mode_status {
        vc4_hdmi_connector_mode_valid(connector, mode, &self.vc4.hvs)
    }

    /// The connector's one encoder, with the CRTCs
    /// `vc4_set_crtc_possible_masks` gave it: those whose pixel valve
    /// feeds that HDMI port.
    fn possible_encoders(&self, connector: &drm_connector) -> Vec<(u32, Vec<u32>)> {
        let port = connector.index;
        let crtcs = (0..self.vc4.crtc.len() as u32)
            .filter(|&i| self.vc4.crtc[i as usize].data.hdmi as u32 == port)
            .collect();
        vec![(port, crtcs)]
    }

    fn encoder_mode_valid(&mut self, _encoder: u32, mode: &drm_display_mode) -> drm_mode_status {
        vc4_hdmi_encoder_mode_valid(mode)
    }

    fn crtc_mode_valid(&mut self, _crtc: u32, mode: &drm_display_mode) -> drm_mode_status {
        vc4_crtc_mode_valid(mode)
    }
}

impl<H: Hw> drm_client_funcs for Kms<'_, H> {
    fn num_crtc(&self) -> u32 {
        self.vc4.crtc.len() as u32
    }

    fn max_width(&self) -> u32 {
        MAX_WIDTH
    }

    fn max_height(&self) -> u32 {
        MAX_HEIGHT
    }

    /// `drm_mode_config_reset` left every connector without encoder and
    /// CRTC, and nothing has been committed since.
    fn connector_state(&self, _connector: &drm_connector) -> (Option<u32>, Option<u32>) {
        (None, None)
    }

    fn crtc_state_mode(&self, _crtc: u32) -> drm_display_mode {
        drm_display_mode::default()
    }
}
