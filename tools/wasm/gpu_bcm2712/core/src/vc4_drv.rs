//! `drivers/gpu/drm/vc4/vc4_drv.c` (Linux rpi-6.18): the display bound as
//! one device, in the order Linux brings its parts up: the L2 interrupt
//! controllers, the firmware clocks and the DDC controllers (built-in and
//! module drivers that probe first), then `vc4_drm_bind` with its
//! components, `vc4_kms_load` and the takeover of what the firmware left
//! running.
//!
//! Not bound: the writeback engines (`vc4_txp`, MOP and MOPLET), which
//! write memory outside the HVS's IOMMU; their `disp_intr` lines 0 and 1
//! stay masked.

use crate::clk::FwClocks;
use crate::hw::{Block, Hw};
use crate::i2c_brcmstb::Bsc;
use crate::irq_brcmstb_l2::{L2Intc, L2_2711_LVL_INTC_INIT, L2_LVL_INTC_INIT};
use crate::vc4_crtc::Vc4Crtc;
use crate::vc4_hdmi::Vc4Hdmi;
use crate::vc4_hvs::Hvs;

/// `clock-frequency` of `ddc0`/`ddc1`, and their lines on `bsc_irq`.
const DDC_CLOCK_FREQUENCY: u32 = 97_500;
const DDC_IRQ: [u32; 2] = [1, 2];

/// `struct vc4_dev` with the components bound to it.
pub struct Vc4 {
    pub aon_intr: L2Intc,
    pub disp_intr: L2Intc,
    pub bsc: Bsc,
    pub clocks: FwClocks,
    pub hvs: Hvs,
    pub hdmi: [Vc4Hdmi; 2],
    pub crtc: [Vc4Crtc; 2],
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
        let mut vc4 = Vc4 { aon_intr, disp_intr, bsc, clocks, hvs, hdmi, crtc };
        // vc4_kms_load: drm_mode_config_reset, then every output the
        // firmware left running stopped.
        for i in 0..2 {
            let hdmi = &mut vc4.hdmi[vc4.crtc[i].data.hdmi as usize];
            vc4.crtc[i].disable_at_boot(hw, &vc4.hvs, hdmi, &mut vc4.bsc, &mut vc4.clocks)?;
        }
        Ok(vc4)
    }
}
