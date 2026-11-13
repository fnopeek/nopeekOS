//! `drivers/gpu/drm/vc4/vc4_hdmi.c` (Linux rpi-6.18), the BCM2712
//! variants (`bcm2712_hdmi0_variant`, `bcm2712_hdmi1_variant`), built as
//! with `CONFIG_DRM_VC4_HDMI_CEC=n`. Audio is not ported: its init
//! registers ASoC devices and touches no register.
//!
//! Runtime PM is the PM core's for a device without parent, domain or
//! autosuspend (`drivers/base/power/runtime.c`): a usage count, resume on
//! the first user, suspend when the last one is gone; an asynchronous put
//! suspends at once.

use crate::clk::{Clk, DvpGate, FwClocks};
use crate::hw::{Block, Hw, RPI_FIRMWARE_M2MC_CLK_ID, RPI_FIRMWARE_PIXEL_BVB_CLK_ID};
use crate::irq_brcmstb_l2::L2Intc;
use crate::linux::EINVAL;
use crate::regs::*;

/// The `aon_intr` lines of `hpd-connected` and `hpd-removed` per port
/// (`interrupt-names` of the HDMI nodes).
pub const HPD_IRQ: [[u32; 2]; 2] = [[7, 8], [14, 15]];

/// `struct vc4_hdmi`, the fields the BCM2712 paths use.
pub struct Vc4Hdmi {
    pub port: u8,
    pub hsm_clock: Clk,
    pub pixel_bvb_clock: Clk,
    pub audio_clock: DvpGate,
    pub scdc_enabled: bool,
    rpm_usage: u32,
    rpm_active: bool,
    rpm_enabled: bool,
}

impl Vc4Hdmi {
    /// `HDMI_READ`.
    pub fn read(&self, hw: &mut impl Hw, f: &HdmiField) -> u32 {
        let (reg, off) = f[self.port as usize];
        hw.read(Block::Hdmi(self.port, reg), off)
    }

    /// `HDMI_WRITE`.
    pub fn write(&self, hw: &mut impl Hw, f: &HdmiField, v: u32) {
        let (reg, off) = f[self.port as usize];
        hw.write(Block::Hdmi(self.port, reg), off, v)
    }

    /// `vc5_hdmi_reset`. `reset_control_reset` on the DVP's reset-simple
    /// line does nothing: reset-simple has no `reset` op.
    fn reset(&self, hw: &mut impl Hw) {
        self.write(hw, &HDMI_DVP_CTL, 0);
        let v = self.read(hw, &HDMI_CLOCK_STOP);
        self.write(hw, &HDMI_CLOCK_STOP, v | VC4_DVP_HT_CLOCK_STOP_PIXEL);
    }

    /// `vc4_hdmi_runtime_resume`.
    fn runtime_resume(&mut self, hw: &mut impl Hw, clocks: &mut FwClocks) -> Result<(), i32> {
        clocks.prepare_enable(hw, self.hsm_clock)?;
        // Without a monitor at boot the firmware leaves the HSM clock at
        // 0, and a register access then stalls the bus.
        if clocks.get_rate(hw, self.hsm_clock) == 0 {
            clocks.disable_unprepare(hw, self.hsm_clock);
            return Err(-EINVAL);
        }
        self.audio_clock.prepare_enable(hw);
        self.reset(hw);
        self.write(hw, &HDMI_RAM_PACKET_CONFIG, VC4_HDMI_RAM_PACKET_ENABLE);
        Ok(())
    }

    /// `vc4_hdmi_runtime_suspend`.
    fn runtime_suspend(&mut self, hw: &mut impl Hw, clocks: &mut FwClocks) {
        self.write(hw, &HDMI_RAM_PACKET_CONFIG, 0);
        self.audio_clock.disable_unprepare(hw);
        let _ = clocks.set_min_rate(hw, self.hsm_clock, 0);
        clocks.disable_unprepare(hw, self.hsm_clock);
    }

    /// `pm_runtime_resume_and_get`.
    pub fn pm_runtime_resume_and_get(&mut self, hw: &mut impl Hw, clocks: &mut FwClocks) -> Result<(), i32> {
        self.rpm_usage += 1;
        if self.rpm_active {
            return Ok(());
        }
        let ret = if self.rpm_enabled { self.runtime_resume(hw, clocks) } else { Err(-crate::linux::EACCES) };
        match ret {
            Ok(()) => {
                self.rpm_active = true;
                Ok(())
            }
            Err(e) => {
                self.rpm_usage -= 1;
                Err(e)
            }
        }
    }

    /// `pm_runtime_put` / `pm_runtime_put_sync`: the last user suspends.
    pub fn pm_runtime_put(&mut self, hw: &mut impl Hw, clocks: &mut FwClocks) {
        if self.rpm_usage == 0 { return; }
        self.rpm_usage -= 1;
        if self.rpm_usage == 0 && self.rpm_active && self.rpm_enabled {
            self.runtime_suspend(hw, clocks);
            self.rpm_active = false;
        }
    }

    /// `vc4_hdmi_bind` of port `port`: `vc5_hdmi_init_resources`, the
    /// clocks a running output needs (as the firmware left it), and the
    /// hotplug lines (threaded, `IRQF_ONESHOT`) on `aon_intr`.
    pub fn bind(hw: &mut impl Hw, port: u8, aon_intr: &mut L2Intc, clocks: &mut FwClocks) -> Result<Self, i32> {
        let mut h = Vc4Hdmi {
            port,
            hsm_clock: clocks.get(RPI_FIRMWARE_M2MC_CLK_ID),
            pixel_bvb_clock: clocks.get(RPI_FIRMWARE_PIXEL_BVB_CLK_ID),
            audio_clock: DvpGate::new(port),
            scdc_enabled: BCM2712_MAX_PIXEL_CLOCK > HDMI_14_MAX_TMDS_CLK,
            rpm_usage: 0,
            rpm_active: false,
            rpm_enabled: false,
        };
        // devm_pm_runtime_enable
        h.rpm_enabled = true;
        h.pm_runtime_resume_and_get(hw, clocks)?;
        // vc5 has no `pixel` clock: clk_prepare_enable(NULL) is a no-op.
        if h.read(hw, &HDMI_VID_CTL) & VC4_HD_VID_CTL_ENABLE != 0 {
            clocks.prepare_enable(hw, h.hsm_clock)?;
            clocks.prepare_enable(hw, h.pixel_bvb_clock)?;
        }
        for line in HPD_IRQ[port as usize] {
            aon_intr.request_irq(hw, line);
        }
        h.pm_runtime_put(hw, clocks);
        Ok(h)
    }
}
