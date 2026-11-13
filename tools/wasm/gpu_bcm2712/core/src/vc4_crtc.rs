//! `drivers/gpu/drm/vc4/vc4_crtc.c` (Linux rpi-6.18): the BCM2712's two
//! pixel valves (`bcm2712_pv0_data`, `bcm2712_pv1_data`), each feeding one
//! HDMI encoder from one HVS channel.

use crate::clk::FwClocks;
use crate::hw::{Block, Hw};
use crate::i2c_brcmstb::Bsc;
use crate::linux::field_get;
use crate::regs::*;
use crate::vc4_hdmi::Vc4Hdmi;
use crate::vc4_hvs::Hvs;

/// `struct vc4_pv_data` with its `struct vc4_crtc_data`.
pub struct PvData {
    pub hvs_available_channels: u32,
    pub hvs_output: u32,
    pub fifo_depth: u32,
    pub pixels_per_clock: u32,
    pub pixels_per_clock_int: u32,
    /// `encoder_types[0]`: the HDMI port.
    pub hdmi: u8,
}

pub const BCM2712_PV_DATA: [PvData; 2] = [
    PvData { hvs_available_channels: 1 << 0, hvs_output: 0, fifo_depth: 64, pixels_per_clock: 1, pixels_per_clock_int: 2, hdmi: 0 },
    PvData { hvs_available_channels: 1 << 1, hvs_output: 1, fifo_depth: 64, pixels_per_clock: 1, pixels_per_clock_int: 2, hdmi: 1 },
];

/// `struct vc4_crtc`.
pub struct Vc4Crtc {
    pub pv: u8,
    pub data: &'static PvData,
}

impl Vc4Crtc {
    fn read(&self, hw: &mut impl Hw, off: u32) -> u32 {
        hw.read(Block::Pv(self.pv), off)
    }

    fn write(&self, hw: &mut impl Hw, off: u32, v: u32) {
        hw.write(Block::Pv(self.pv), off, v)
    }

    /// `vc4_crtc_bind`: the valve's interrupts off and its VFP_START
    /// status cleared. The interrupt line itself is the kernel's
    /// (`npk_irq_register_dt`).
    pub fn bind(hw: &mut impl Hw, pv: u8) -> Self {
        let c = Vc4Crtc { pv, data: &BCM2712_PV_DATA[pv as usize] };
        c.write(hw, PV_INTEN, 0);
        c.write(hw, PV_INTSTAT, PV_INT_VFP_START);
        c
    }

    /// `vc4_crtc_pixelvalve_reset`.
    fn pixelvalve_reset(&self, hw: &mut impl Hw) {
        let v = self.read(hw, PV_CONTROL);
        self.write(hw, PV_CONTROL, v & !PV_CONTROL_EN);
        let v = self.read(hw, PV_CONTROL);
        self.write(hw, PV_CONTROL, v | PV_CONTROL_FIFO_CLR);
    }

    /// `vc4_crtc_disable`.
    fn disable(&self, hw: &mut impl Hw, hvs: &Hvs, hdmi: &mut Vc4Hdmi, bsc: &mut Bsc, clocks: &mut FwClocks, channel: u32) {
        let v = self.read(hw, PV_V_CONTROL);
        self.write(hw, PV_V_CONTROL, v & !PV_VCONTROL_VIDEN);
        // wait_for(!VIDEN, 1): Linux WARNs once on a timeout.
        let deadline = hw.now_us() + 1000;
        while self.read(hw, PV_V_CONTROL) & PV_VCONTROL_VIDEN != 0 && hw.now_us() < deadline {}
        // A pixel can stick in the FIFO between the valve and HDMI
        // without it.
        hw.delay_us(20_000);
        hdmi.encoder_post_crtc_disable(hw, bsc);
        self.pixelvalve_reset(hw);
        hvs.stop_channel(hw, channel);
        hdmi.encoder_post_crtc_powerdown(hw, clocks);
    }

    /// `vc4_crtc_disable_at_boot`: an output the firmware left running is
    /// stopped before the first modeset.
    pub fn disable_at_boot(&self, hw: &mut impl Hw, hvs: &Hvs, hdmi: &mut Vc4Hdmi, bsc: &mut Bsc,
        clocks: &mut FwClocks) -> Result<(), i32>
    {
        if self.read(hw, PV_CONTROL) & PV_CONTROL_EN == 0 {
            return Ok(());
        }
        if self.read(hw, PV_V_CONTROL) & PV_VCONTROL_VIDEN == 0 {
            return Ok(());
        }
        let Some(channel) = hvs.get_fifo_from_output(self.data.hvs_output) else { return Ok(()) };
        let encoder_sel = field_get(PV_CONTROL_CLK_SELECT_MASK, self.read(hw, PV_CONTROL));
        if encoder_sel != 0 {
            return Ok(());
        }
        hdmi.pm_runtime_resume_and_get(hw, clocks)?;
        // post_crtc_powerdown drops the reference again.
        self.disable(hw, hvs, hdmi, bsc, clocks, channel);
        Ok(())
    }
}
