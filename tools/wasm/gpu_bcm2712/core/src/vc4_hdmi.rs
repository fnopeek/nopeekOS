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
use crate::drm_scdc_helper::{drm_scdc_set_high_tmds_clock_ratio, drm_scdc_set_scrambling};
use crate::hw::{Block, Hw, RPI_FIRMWARE_M2MC_CLK_ID, RPI_FIRMWARE_PIXEL_BVB_CLK_ID};
use crate::i2c_brcmstb::{Bsc, Ddc};
use crate::irq_brcmstb_l2::L2Intc;
use crate::linux::EINVAL;
use crate::regs::*;
use crate::vc4_hvs::Hvs;
use alloc::format;
use core::sync::atomic::{AtomicBool, Ordering};
use drm_core::drm_connector::{drm_connector, drm_connector_status};
use drm_core::drm_edid::drm_edid_connector_add_modes;
use drm_core::drm_hdmi_helper::drm_hdmi_compute_mode_clock;
use drm_core::drm_hdmi_state_helper::{drm_atomic_helper_connector_hdmi_hotplug, drm_hdmi_connector_mode_valid};
use drm_core::drm_modes::{drm_display_mode, drm_mode_status, drm_mode_vrefresh};
use drm_core::regs::{
    DRM_MODE_CONNECTOR_HDMIA, DRM_MODE_FLAG_DBLCLK, HDMI_COLORSPACE_RGB, MODE_CLOCK_HIGH, MODE_H_ILLEGAL, MODE_OK,
};

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

    /// `vc5_hdmi_hp_detect`.
    fn hp_detect(&self, hw: &mut impl Hw) -> bool {
        self.read(hw, &HDMI_HOTPLUG) & VC4_HDMI_HOTPLUG_CONNECTED != 0
    }

    /// `vc4_hdmi_connector_detect_ctx`, without `force_hotplug` (no
    /// module parameters) and without an HPD GPIO (the BCM2712 nodes have
    /// none).
    pub fn connector_detect_ctx(&mut self, hw: &mut impl Hw, connector: &mut drm_connector, bsc: &mut Bsc,
        clocks: &mut FwClocks) -> drm_connector_status
    {
        if let Err(ret) = self.pm_runtime_resume_and_get(hw, clocks) {
            drm_core::linux::Line::new()
                .s(&format!("Failed to retain HDMI power domain: {}", ret))
                .say();
            return drm_connector_status::connector_status_unknown;
        }

        let status = if self.hp_detect(hw) {
            drm_connector_status::connector_status_connected
        } else {
            drm_connector_status::connector_status_disconnected
        };

        self.handle_hotplug(hw, connector, bsc, status);
        self.pm_runtime_put(hw, clocks);

        status
    }

    /// `vc4_hdmi_handle_hotplug`.
    fn handle_hotplug(&mut self, hw: &mut impl Hw, connector: &mut drm_connector, bsc: &mut Bsc,
        status: drm_connector_status)
    {
        let mut ddc = Ddc { bsc, n: self.port as usize };
        drm_atomic_helper_connector_hdmi_hotplug(connector, status, &mut |m| ddc.i2c_transfer(hw, m));

        if status != drm_connector_status::connector_status_connected {
            return;
        }

        self.reset_link();
    }

    /// `vc4_hdmi_reset_link`: a connector whose state has no CRTC needs
    /// nothing. The atomic state is not ported, so no connector state has
    /// a CRTC.
    fn reset_link(&mut self) -> i32 {
        0
    }

    /// `vc4_hdmi_disable_scrambling`: once, also at boot (`scdc_enabled`
    /// starts true, since the firmware's state is unknown).
    fn disable_scrambling(&mut self, hw: &mut impl Hw, bsc: &mut Bsc) {
        if !self.scdc_enabled {
            return;
        }
        self.scdc_enabled = false;
        let v = self.read(hw, &HDMI_SCRAMBLER_CTL);
        self.write(hw, &HDMI_SCRAMBLER_CTL, v & !VC5_HDMI_SCRAMBLER_CTL_ENABLE);
        let mut ddc = Ddc { bsc, n: self.port as usize };
        drm_scdc_set_scrambling(hw, &mut ddc, false);
        drm_scdc_set_high_tmds_clock_ratio(hw, &mut ddc, false);
    }

    /// `vc4_hdmi_encoder_post_crtc_disable`, gen 6 (the output stays
    /// enabled: clearing `VC4_HD_VID_CTL_ENABLE` locks the BCM2712 up).
    pub fn encoder_post_crtc_disable(&mut self, hw: &mut impl Hw, bsc: &mut Bsc) {
        self.write(hw, &HDMI_RAM_PACKET_CONFIG, VC4_HDMI_RAM_PACKET_ENABLE);
        let v = self.read(hw, &HDMI_VID_CTL);
        self.write(hw, &HDMI_VID_CTL, v | VC4_HD_VID_CTL_CLRRGB);
        let v = self.read(hw, &HDMI_VID_CTL);
        self.write(hw, &HDMI_VID_CTL, v | VC4_HD_VID_CTL_BLANKPIX);
        hw.delay_us(1000);
        self.disable_scrambling(hw, bsc);
    }

    /// `vc4_hdmi_encoder_post_crtc_powerdown`: the PHY off, the clocks a
    /// running output holds released, the runtime-PM reference dropped.
    pub fn encoder_post_crtc_powerdown(&mut self, hw: &mut impl Hw, clocks: &mut FwClocks) {
        let v = self.read(hw, &HDMI_VID_CTL);
        self.write(hw, &HDMI_VID_CTL, v | VC4_HD_VID_CTL_BLANKPIX);
        crate::vc4_hdmi_phy::vc6_hdmi_phy_disable(self, hw);
        let _ = clocks.set_min_rate(hw, self.pixel_bvb_clock, 0);
        clocks.disable_unprepare(hw, self.pixel_bvb_clock);
        self.pm_runtime_put(hw, clocks);
    }
}

/// `vc4_hdmi_connector_init`: the connector `drmm_connector_hdmi_init`
/// sets up for port `port`, the device's connector `port`.
pub fn vc4_hdmi_connector_init(port: u8) -> drm_connector {
    let mut connector = drm_connector::new(format!("HDMI-A-{}", port + 1), DRM_MODE_CONNECTOR_HDMIA);
    connector.index = port as u32;
    connector.rpi_downstream = true;
    connector.interlace_allowed = true;
    connector.doublescan_allowed = false;
    connector.stereo_allowed = true;
    connector
}

/// `vc4_hdmi_connector_get_modes`.
pub fn vc4_hdmi_connector_get_modes(connector: &mut drm_connector, hvs: &Hvs) -> i32 {
    let ret = drm_edid_connector_add_modes(connector);

    // drm_warn_once
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !hvs.vc5_hdmi_enable_hdmi_20
        && connector.probed_modes.iter().any(|m| vc4_hdmi_mode_needs_scrambling(m, 8, HDMI_COLORSPACE_RGB))
        && !WARNED.swap(true, Ordering::Relaxed)
    {
        drm_core::linux::Line::new()
            .s("The core clock cannot reach frequencies high enough to support 4k @ 60Hz.")
            .say();
        drm_core::linux::Line::new()
            .s("Please change your config.txt file to add hdmi_enable_4kp60.")
            .say();
    }

    ret
}

/// `vc4_hdmi_mode_needs_scrambling`.
pub fn vc4_hdmi_mode_needs_scrambling(mode: &drm_display_mode, bpc: u32, fmt: u32) -> bool {
    drm_hdmi_compute_mode_clock(mode, bpc, fmt) > HDMI_14_MAX_TMDS_CLK as u64
}

/// `vc4_hdmi_connector_clock_valid`, the connector's
/// `tmds_char_rate_valid`.
pub fn vc4_hdmi_connector_clock_valid(hvs: &Hvs, mode: &drm_display_mode, clock: u64) -> drm_mode_status {
    if clock > BCM2712_MAX_PIXEL_CLOCK as u64 {
        return MODE_CLOCK_HIGH as i32;
    }

    if !hvs.vc5_hdmi_enable_hdmi_20 && clock > HDMI_14_MAX_TMDS_CLK as u64 {
        return MODE_CLOCK_HIGH as i32;
    }

    // 4096x2160@60 is not reliable without overclocking core
    if !hvs.vc5_hdmi_enable_4096by2160 && mode.hdisplay > 3840 && mode.vdisplay >= 2160
        && drm_mode_vrefresh(mode) >= 50
    {
        return MODE_CLOCK_HIGH as i32;
    }

    MODE_OK as i32
}

/// The connector's `mode_valid`: `drm_hdmi_connector_mode_valid` with
/// vc4's TMDS check.
pub fn vc4_hdmi_connector_mode_valid(connector: &drm_connector, mode: &drm_display_mode, hvs: &Hvs)
    -> drm_mode_status
{
    drm_hdmi_connector_mode_valid(connector, mode, Some(&mut |_, m, clock| vc4_hdmi_connector_clock_valid(hvs, m, clock)))
}

/// `vc4_hdmi_encoder_mode_valid`.
pub fn vc4_hdmi_encoder_mode_valid(mode: &drm_display_mode) -> drm_mode_status {
    if BCM2712_UNSUPPORTED_ODD_H_TIMINGS != 0
        && mode.flags & DRM_MODE_FLAG_DBLCLK == 0
        && (mode.hdisplay % 2 != 0 || mode.hsync_start % 2 != 0 || mode.hsync_end % 2 != 0 || mode.htotal % 2 != 0)
    {
        return MODE_H_ILLEGAL as i32;
    }

    MODE_OK as i32
}
