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
use crate::drm_scdc_helper::{drm_scdc_get_scrambling_status, drm_scdc_set_high_tmds_clock_ratio, drm_scdc_set_scrambling};
use crate::hw::{Block, Hw, RPI_FIRMWARE_M2MC_CLK_ID, RPI_FIRMWARE_PIXEL_BVB_CLK_ID};
use crate::i2c_brcmstb::{Bsc, Ddc};
use crate::irq_brcmstb_l2::L2Intc;
use crate::linux::{field_prep, wait_for, EINVAL, ENOMEM};
use crate::regs::*;
use crate::vc4_hvs::Hvs;
use alloc::format;
use core::sync::atomic::{AtomicBool, Ordering};
use alloc::string::String;
use drm_core::drm_connector::{drm_connector, drm_connector_state, drm_connector_status};
use drm_core::drm_edid::drm_edid_connector_add_modes;
use drm_core::drm_hdmi_helper::drm_hdmi_compute_mode_clock;
use drm_core::drm_hdmi_state_helper::{
    drm_atomic_helper_connector_hdmi_check, drm_atomic_helper_connector_hdmi_hotplug,
    drm_atomic_helper_connector_hdmi_update_infoframes, drm_connector_hdmi_funcs, drm_hdmi_connector_mode_valid,
};
use drm_core::drm_modes::{drm_display_mode, drm_mode_set_crtcinfo, drm_mode_status, drm_mode_vrefresh};
use drm_core::regs::{
    CRTC_INTERLACE_HALVE_V, CRTC_STEREO_DOUBLE, DRM_MODE_CONNECTOR_HDMIA, DRM_MODE_FLAG_DBLCLK, DRM_MODE_FLAG_INTERLACE,
    DRM_MODE_FLAG_PHSYNC, DRM_MODE_FLAG_PVSYNC, HDMI_COLORSPACE_RGB, HDMI_COLORSPACE_YUV422, HDMI_COLORSPACE_YUV444,
    MODE_CLOCK_HIGH, MODE_H_ILLEGAL, MODE_OK,
};

/// The `aon_intr` lines of `hpd-connected` and `hpd-removed` per port
/// (`interrupt-names` of the HDMI nodes).
pub const HPD_IRQ: [[u32; 2]; 2] = [[7, 8], [14, 15]];

/// `SCRAMBLING_POLLING_DELAY_MS`.
const SCRAMBLING_POLLING_DELAY_MS: u64 = 1000;

/// The 2.4 GHz WiFi channel 1 a TMDS bit rate keeps out of.
const WIFI_2_4GHZ_CH1_MIN_FREQ: u64 = 2_400_000_000;
const WIFI_2_4GHZ_CH1_MAX_FREQ: u64 = 2_422_000_000;

/// `struct vc4_hdmi`, the fields the BCM2712 paths use.
pub struct Vc4Hdmi {
    pub port: u8,
    pub hsm_clock: Clk,
    pub pixel_bvb_clock: Clk,
    pub audio_clock: DvpGate,
    pub scdc_enabled: bool,
    /// `wifi-2.4ghz-coexistence` (none of the Pi 5 device trees sets it).
    pub disable_wifi_frequencies: bool,
    /// What `vc4_hdmi_encoder_atomic_mode_set` saved for the enable path.
    pub saved_adjusted_mode: drm_display_mode,
    pub output_bpc: u32,
    pub output_format: u32,
    /// `scrambling_work`: when the queued check is due (`now_us`).
    pub scrambling_work: Option<u64>,
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
            disable_wifi_frequencies: false,
            saved_adjusted_mode: drm_display_mode::default(),
            output_bpc: 0,
            output_format: 0,
            scrambling_work: None,
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

        self.scrambling_work = None;

        let v = self.read(hw, &HDMI_SCRAMBLER_CTL);
        self.write(hw, &HDMI_SCRAMBLER_CTL, v & !VC5_HDMI_SCRAMBLER_CTL_ENABLE);
        let mut ddc = Ddc { bsc, n: self.port as usize };
        drm_scdc_set_scrambling(hw, &mut ddc, false);
        drm_scdc_set_high_tmds_clock_ratio(hw, &mut ddc, false);
    }

    /// `vc4_hdmi_encoder_atomic_check`: odd timings fixed up where the
    /// variant needs it, the WiFi band avoided, the CRTC timings rebuilt.
    pub fn encoder_atomic_check(&self, mode: &mut drm_display_mode) -> i32 {
        let mut tmds_char_rate = mode.clock as i64 as u64 * 1000;

        if BCM2712_UNSUPPORTED_ODD_H_TIMINGS != 0
            || (BCM2712_UNSUPPORTED_INT_ODD_H_TIMINGS != 0 && mode.flags & DRM_MODE_FLAG_INTERLACE != 0)
        {
            if mode.flags & DRM_MODE_FLAG_DBLCLK != 0 {
                // Only try to fixup DBLCLK modes to get 480i and 576i
                // working.
                // A generic solution for all modes with odd horizontal
                // timing values seems impossible based on trying to
                // solve it for 1366x768 monitors.
                if (mode.hsync_start - mode.hdisplay) & 1 != 0 {
                    mode.hsync_start -= 1;
                }
                if (mode.hsync_end - mode.hsync_start) & 1 != 0 {
                    mode.hsync_end -= 1;
                }
            }

            // Now check whether we still have odd values remaining
            if mode.hdisplay % 2 != 0 || mode.hsync_start % 2 != 0 || mode.hsync_end % 2 != 0
                || mode.htotal % 2 != 0
            {
                return -EINVAL;
            }
        }

        // The 1440p@60 pixel rate is in the same range than the first
        // WiFi channel (between 2.4GHz and 2.422GHz with 22MHz
        // bandwidth). Slightly lower the frequency to bring it out of
        // the WiFi range.
        let tmds_bit_rate = tmds_char_rate * 10;
        if self.disable_wifi_frequencies
            && (WIFI_2_4GHZ_CH1_MIN_FREQ..=WIFI_2_4GHZ_CH1_MAX_FREQ).contains(&tmds_bit_rate)
        {
            mode.clock = 238560;
            tmds_char_rate = mode.clock as u64 * 1000;
        }
        let _ = tmds_char_rate;

        // Rebuilds every crtc_* field, so CRTC_INTERLACE_HALVE_V is needed too
        drm_mode_set_crtcinfo(mode, CRTC_INTERLACE_HALVE_V | CRTC_STEREO_DOUBLE);

        0
    }

    /// `vc4_hdmi_encoder_atomic_mode_set`.
    pub fn encoder_atomic_mode_set(&mut self, adjusted_mode: &drm_display_mode, conn_state: &drm_connector_state) {
        self.saved_adjusted_mode = adjusted_mode.clone();
        self.output_bpc = conn_state.hdmi.output_bpc;
        self.output_format = conn_state.hdmi.output_format;
    }

    /// `vc4_hdmi_encoder_pre_crtc_configure`: runtime PM taken (until the
    /// output is disabled), HSM and BVB clocks for the TMDS rate, the PHY,
    /// the scheduler and the timings. vc5 has no `pixel` clock and CEC is
    /// not built, so neither is touched.
    pub fn encoder_pre_crtc_configure(&mut self, hw: &mut impl Hw, clocks: &mut FwClocks,
        conn_state: &drm_connector_state)
    {
        let tmds_char_rate = conn_state.hdmi.tmds_char_rate;

        if let Err(ret) = self.pm_runtime_resume_and_get(hw, clocks) {
            drm_core::linux::Line::new().s(&format!("Failed to retain power domain: {}", ret)).say();
            return;
        }

        // As stated in RPi's vc4 firmware "HDMI state machine (HSM) clock
        // must be faster than pixel clock, infinitesimally faster, tested
        // in simulation. Otherwise, exact value is unimportant for HDMI
        // operation." Real life tests leave a 1% margin between both
        // clocks.
        let hsm_rate = (HSM_MIN_CLOCK_FREQ as u64).max(tmds_char_rate / 100 * 101);
        if let Err(ret) = clocks.set_min_rate(hw, self.hsm_clock, hsm_rate) {
            drm_core::linux::Line::new().s(&format!("Failed to set HSM clock rate: {}", ret)).say();
            self.pm_runtime_put(hw, clocks);
            return;
        }

        let bvb_rate = if tmds_char_rate > 297_000_000 {
            300_000_000
        } else if tmds_char_rate > 148_500_000 {
            150_000_000
        } else {
            75_000_000
        };

        if let Err(ret) = clocks.set_min_rate(hw, self.pixel_bvb_clock, bvb_rate) {
            drm_core::linux::Line::new().s(&format!("Failed to set pixel bvb clock rate: {}", ret)).say();
            self.pm_runtime_put(hw, clocks);
            return;
        }

        if let Err(ret) = clocks.prepare_enable(hw, self.pixel_bvb_clock) {
            drm_core::linux::Line::new().s(&format!("Failed to turn on pixel bvb clock: {}", ret)).say();
            self.pm_runtime_put(hw, clocks);
            return;
        }

        crate::vc4_hdmi_phy::vc6_hdmi_phy_init(self, hw, conn_state);

        let v = self.read(hw, &HDMI_SCHEDULER_CONTROL);
        self.write(hw, &HDMI_SCHEDULER_CONTROL,
            v | VC4_HDMI_SCHEDULER_CONTROL_MANUAL_FORMAT | VC4_HDMI_SCHEDULER_CONTROL_IGNORE_VSYNC_PREDICTS);

        let mode = self.saved_adjusted_mode.clone();
        self.set_timings(hw, conn_state, &mode);
    }

    /// `vc5_hdmi_set_timings`.
    fn set_timings(&self, hw: &mut impl Hw, state: &drm_connector_state, mode: &drm_display_mode) {
        let hsync_pos = mode.flags & DRM_MODE_FLAG_PHSYNC != 0;
        let vsync_pos = mode.flags & DRM_MODE_FLAG_PVSYNC != 0;
        let interlaced = (mode.flags & DRM_MODE_FLAG_INTERLACE != 0) as u32;
        let pixel_rep: u32 = if mode.flags & DRM_MODE_FLAG_DBLCLK != 0 { 2 } else { 1 };
        let verta = field_prep(VC5_HDMI_VERTA_VSP_MASK, (mode.crtc_vsync_end - mode.crtc_vsync_start) as u32)
            | field_prep(VC5_HDMI_VERTA_VFP_MASK, (mode.crtc_vsync_start - mode.crtc_vdisplay) as u32)
            | field_prep(VC5_HDMI_VERTA_VAL_MASK, mode.crtc_vdisplay as u32);
        let vertb = field_prep(VC5_HDMI_VERTB_VSPO_MASK, mode.htotal as u32 >> (2 - pixel_rep))
            | field_prep(VC4_HDMI_VERTB_VBP_MASK, (mode.crtc_vtotal - mode.crtc_vsync_end) as u32 + interlaced);
        let vertb_even = field_prep(VC5_HDMI_VERTB_VSPO_MASK, 0)
            | field_prep(VC4_HDMI_VERTB_VBP_MASK, (mode.crtc_vtotal - mode.crtc_vsync_end) as u32);

        self.write(hw, &HDMI_HORZA,
            (if vsync_pos { VC5_HDMI_HORZA_VPOS } else { 0 })
                | (if hsync_pos { VC5_HDMI_HORZA_HPOS } else { 0 })
                | field_prep(VC5_HDMI_HORZA_HAP_MASK, mode.hdisplay as u32 * pixel_rep)
                | field_prep(VC5_HDMI_HORZA_HFP_MASK, (mode.hsync_start - mode.hdisplay) as u32 * pixel_rep));

        self.write(hw, &HDMI_HORZB,
            field_prep(VC5_HDMI_HORZB_HBP_MASK, (mode.htotal - mode.hsync_end) as u32 * pixel_rep)
                | field_prep(VC5_HDMI_HORZB_HSP_MASK, (mode.hsync_end - mode.hsync_start) as u32 * pixel_rep));

        self.write(hw, &HDMI_VERTA0, verta);
        self.write(hw, &HDMI_VERTA1, verta);

        self.write(hw, &HDMI_VERTB0, vertb_even);
        self.write(hw, &HDMI_VERTB1, vertb);

        let mut gcp = match state.hdmi.output_bpc {
            12 => 6,
            10 => 5,
            _ => 0,
        };

        // YCC422 is always 36-bit and not considered deep colour so
        // doesn't signal in GCP.
        if state.hdmi.output_format == HDMI_COLORSPACE_YUV422 {
            gcp = 0;
        }

        let mut reg = self.read(hw, &HDMI_DEEP_COLOR_CONFIG_1);
        reg &= !(VC5_HDMI_DEEP_COLOR_CONFIG_1_INIT_PACK_PHASE_MASK | VC5_HDMI_DEEP_COLOR_CONFIG_1_COLOR_DEPTH_MASK);
        reg |= field_prep(VC5_HDMI_DEEP_COLOR_CONFIG_1_INIT_PACK_PHASE_MASK, 2)
            | field_prep(VC5_HDMI_DEEP_COLOR_CONFIG_1_COLOR_DEPTH_MASK, gcp);
        self.write(hw, &HDMI_DEEP_COLOR_CONFIG_1, reg);

        let mut reg = self.read(hw, &HDMI_GCP_WORD_1);
        reg &= !VC5_HDMI_GCP_WORD_1_GCP_SUBPACKET_BYTE_1_MASK;
        reg |= field_prep(VC5_HDMI_GCP_WORD_1_GCP_SUBPACKET_BYTE_1_MASK, gcp);
        reg &= !VC5_HDMI_GCP_WORD_1_GCP_SUBPACKET_BYTE_0_MASK;
        reg |= VC5_HDMI_GCP_WORD_1_GCP_SUBPACKET_BYTE_0_CLEAR_AVMUTE;
        self.write(hw, &HDMI_GCP_WORD_1, reg);

        let reg = self.read(hw, &HDMI_GCP_CONFIG);
        self.write(hw, &HDMI_GCP_CONFIG, reg | VC5_HDMI_GCP_CONFIG_GCP_ENABLE);

        let mut reg = self.read(hw, &HDMI_MISC_CONTROL);
        reg &= !VC5_HDMI_MISC_CONTROL_PIXEL_REP_MASK;
        reg |= field_prep(VC5_HDMI_MISC_CONTROL_PIXEL_REP_MASK, pixel_rep - 1);
        self.write(hw, &HDMI_MISC_CONTROL, reg);

        self.write(hw, &HDMI_CLOCK_STOP, 0);
    }

    /// `vc5_hdmi_set_csc_coeffs`: `coeffs` is one `[3][4]` matrix.
    fn set_csc_coeffs(&self, hw: &mut impl Hw, c: &[u32]) {
        self.write(hw, &HDMI_CSC_12_11, (c[1] << 16) | c[0]);
        self.write(hw, &HDMI_CSC_14_13, (c[3] << 16) | c[2]);
        self.write(hw, &HDMI_CSC_22_21, (c[5] << 16) | c[4]);
        self.write(hw, &HDMI_CSC_24_23, (c[7] << 16) | c[6]);
        self.write(hw, &HDMI_CSC_32_31, (c[9] << 16) | c[8]);
        self.write(hw, &HDMI_CSC_34_33, (c[11] << 16) | c[10]);
    }

    /// `vc5_hdmi_set_csc_coeffs_swap`: YUV444 needs the CSC matrices
    /// using the channels in a different order.
    fn set_csc_coeffs_swap(&self, hw: &mut impl Hw, c: &[u32]) {
        self.write(hw, &HDMI_CSC_12_11, (c[5] << 16) | c[4]);
        self.write(hw, &HDMI_CSC_14_13, (c[7] << 16) | c[6]);
        self.write(hw, &HDMI_CSC_22_21, (c[9] << 16) | c[8]);
        self.write(hw, &HDMI_CSC_24_23, (c[11] << 16) | c[10]);
        self.write(hw, &HDMI_CSC_32_31, (c[1] << 16) | c[0]);
        self.write(hw, &HDMI_CSC_34_33, (c[3] << 16) | c[2]);
    }

    /// `vc5_hdmi_csc_setup`.
    fn csc_setup(&self, hw: &mut impl Hw, state: &drm_connector_state) {
        let lim_range = state.hdmi.is_limited_range as usize;
        let mut if_cfg = 0;
        let mut if_xbar = 0x543210;
        let mut csc_chan_ctl = 0;
        let mut csc_ctl =
            VC5_MT_CP_CSC_CTL_ENABLE | field_prep(VC5_MT_CP_CSC_CTL_MODE_MASK, VC4_HD_CSC_CTL_MODE_CUSTOM);

        match state.hdmi.output_format {
            HDMI_COLORSPACE_YUV444 => {
                let csc = vc5_hdmi_find_yuv_csc_coeffs(state.colorspace, lim_range);

                self.set_csc_coeffs_swap(hw, csc);
            }

            HDMI_COLORSPACE_YUV422 => {
                let csc = vc5_hdmi_find_yuv_csc_coeffs(state.colorspace, lim_range);

                csc_ctl |= field_prep(VC5_MT_CP_CSC_CTL_FILTER_MODE_444_TO_422_MASK,
                    VC5_MT_CP_CSC_CTL_FILTER_MODE_444_TO_422_STANDARD)
                    | VC5_MT_CP_CSC_CTL_USE_444_TO_422
                    | VC5_MT_CP_CSC_CTL_USE_RNG_SUPPRESSION;

                csc_chan_ctl |=
                    field_prep(VC5_MT_CP_CHANNEL_CTL_OUTPUT_REMAP_MASK, VC5_MT_CP_CHANNEL_CTL_OUTPUT_REMAP_LEGACY_STYLE);

                if_cfg |= field_prep(VC5_DVP_HT_VEC_INTERFACE_CFG_SEL_422_MASK,
                    VC5_DVP_HT_VEC_INTERFACE_CFG_SEL_422_FORMAT_422_LEGACY);

                self.set_csc_coeffs(hw, csc);
            }

            HDMI_COLORSPACE_RGB => {
                if_xbar = 0x354021;

                self.set_csc_coeffs(hw, &VC5_HDMI_CSC_FULL_RGB_TO_RGB[lim_range * 12..lim_range * 12 + 12]);
            }

            _ => {}
        }

        self.write(hw, &HDMI_VEC_INTERFACE_CFG, if_cfg);
        self.write(hw, &HDMI_VEC_INTERFACE_XBAR, if_xbar);
        self.write(hw, &HDMI_CSC_CHANNEL_CTL, csc_chan_ctl);
        self.write(hw, &HDMI_CSC_CTL, csc_ctl);
    }

    /// `vc4_hdmi_encoder_pre_crtc_enable`.
    pub fn encoder_pre_crtc_enable(&self, hw: &mut impl Hw, conn_state: &drm_connector_state) {
        self.csc_setup(hw, conn_state);

        self.write(hw, &HDMI_FIFO_CTL, VC4_HDMI_FIFO_CTL_MASTER_SLAVE_N);
    }

    /// `vc4_hdmi_encoder_post_crtc_enable`: video on, HDMI mode and its
    /// infoframes, the FIFO recentred, scrambling when the rate needs it.
    pub fn encoder_post_crtc_enable(&mut self, hw: &mut impl Hw, bsc: &mut Bsc, connector: &drm_connector,
        old_conn_state: &drm_connector_state, new_conn_state: &drm_connector_state, hvs: &Hvs)
    {
        let mode = &self.saved_adjusted_mode;
        let display = &connector.display_info;
        let hsync_pos = mode.flags & DRM_MODE_FLAG_PHSYNC != 0;
        let vsync_pos = mode.flags & DRM_MODE_FLAG_PVSYNC != 0;

        let v = self.read(hw, &HDMI_VID_CTL);
        self.write(hw, &HDMI_VID_CTL,
            (v & !(VC4_HD_VID_CTL_VSYNC_LOW | VC4_HD_VID_CTL_HSYNC_LOW))
                | VC4_HD_VID_CTL_ENABLE
                | VC4_HD_VID_CTL_CLRRGB
                | VC4_HD_VID_CTL_UNDERFLOW_ENABLE
                | VC4_HD_VID_CTL_FRAME_COUNTER_RESET
                | VC4_HD_VID_CTL_BLANK_INSERT_EN
                | (if vsync_pos { 0 } else { VC4_HD_VID_CTL_VSYNC_LOW })
                | (if hsync_pos { 0 } else { VC4_HD_VID_CTL_HSYNC_LOW }));

        let v = self.read(hw, &HDMI_VID_CTL);
        self.write(hw, &HDMI_VID_CTL, v & !VC4_HD_VID_CTL_BLANKPIX);

        if display.is_hdmi {
            let v = self.read(hw, &HDMI_SCHEDULER_CONTROL);
            self.write(hw, &HDMI_SCHEDULER_CONTROL, v | VC4_HDMI_SCHEDULER_CONTROL_MODE_HDMI);

            let ret = wait_for(hw, 1000, |hw| {
                self.read(hw, &HDMI_SCHEDULER_CONTROL) & VC4_HDMI_SCHEDULER_CONTROL_HDMI_ACTIVE != 0
            });
            if ret != 0 {
                drm_core::linux::Line::new().s("Timeout waiting for VC4_HDMI_SCHEDULER_CONTROL_HDMI_ACTIVE").say();
            }
        } else {
            let v = self.read(hw, &HDMI_SCHEDULER_CONTROL);
            self.write(hw, &HDMI_SCHEDULER_CONTROL, v & !VC4_HDMI_SCHEDULER_CONTROL_MODE_HDMI);

            let ret = wait_for(hw, 1000, |hw| {
                self.read(hw, &HDMI_SCHEDULER_CONTROL) & VC4_HDMI_SCHEDULER_CONTROL_HDMI_ACTIVE == 0
            });
            if ret != 0 {
                drm_core::linux::Line::new().s("Timeout waiting for !VC4_HDMI_SCHEDULER_CONTROL_HDMI_ACTIVE").say();
            }
        }

        if display.is_hdmi {
            // WARN_ON(!HDMI_ACTIVE)
            let _ = self.read(hw, &HDMI_SCHEDULER_CONTROL) & VC4_HDMI_SCHEDULER_CONTROL_HDMI_ACTIVE;

            let mut funcs = Vc4HdmiFuncs { hdmi: self, hw, hvs };
            drm_atomic_helper_connector_hdmi_update_infoframes(connector, old_conn_state, new_conn_state, &mut funcs);
        }

        self.recenter_fifo(hw);
        self.enable_scrambling(hw, bsc, connector);
    }

    /// `vc4_hdmi_recenter_fifo`.
    fn recenter_fifo(&self, hw: &mut impl Hw) {
        let mut drift = self.read(hw, &HDMI_FIFO_CTL);
        drift &= VC4_HDMI_FIFO_VALID_WRITE_MASK;

        self.write(hw, &HDMI_FIFO_CTL, drift & !VC4_HDMI_FIFO_CTL_RECENTER);
        self.write(hw, &HDMI_FIFO_CTL, drift | VC4_HDMI_FIFO_CTL_RECENTER);

        hw.delay_us(1000);

        self.write(hw, &HDMI_FIFO_CTL, drift & !VC4_HDMI_FIFO_CTL_RECENTER);
        self.write(hw, &HDMI_FIFO_CTL, drift | VC4_HDMI_FIFO_CTL_RECENTER);

        let ret = wait_for(hw, 1, |hw| self.read(hw, &HDMI_FIFO_CTL) & VC4_HDMI_FIFO_CTL_RECENTER_DONE != 0);
        if ret != 0 {
            drm_core::linux::Line::new().s("Timeout waiting for VC4_HDMI_FIFO_CTL_RECENTER_DONE").say();
        }
    }

    /// `vc4_hdmi_enable_scrambling`: the sink told over SCDC first, then
    /// the scrambler on, and a check queued.
    fn enable_scrambling(&mut self, hw: &mut impl Hw, bsc: &mut Bsc, connector: &drm_connector) {
        if !vc4_hdmi_supports_scrambling(connector) {
            return;
        }

        if !vc4_hdmi_mode_needs_scrambling(&self.saved_adjusted_mode, self.output_bpc, self.output_format) {
            return;
        }

        let mut ddc = Ddc { bsc, n: self.port as usize };
        drm_scdc_set_high_tmds_clock_ratio(hw, &mut ddc, true);
        drm_scdc_set_scrambling(hw, &mut ddc, true);

        let v = self.read(hw, &HDMI_SCRAMBLER_CTL);
        self.write(hw, &HDMI_SCRAMBLER_CTL, v | VC5_HDMI_SCRAMBLER_CTL_ENABLE);

        self.scdc_enabled = true;

        self.scrambling_work = Some(hw.now_us() + SCRAMBLING_POLLING_DELAY_MS * 1000);
    }

    /// `vc4_hdmi_scrambling_wq`, when `scrambling_work` is due: a sink
    /// that lost scrambling is told again.
    pub fn scrambling_wq(&mut self, hw: &mut impl Hw, bsc: &mut Bsc) {
        self.scrambling_work = None;

        let mut ddc = Ddc { bsc, n: self.port as usize };
        if drm_scdc_get_scrambling_status(hw, &mut ddc) {
            return;
        }

        drm_scdc_set_high_tmds_clock_ratio(hw, &mut ddc, true);
        drm_scdc_set_scrambling(hw, &mut ddc, true);

        self.scrambling_work = Some(hw.now_us() + SCRAMBLING_POLLING_DELAY_MS * 1000);
    }

    /// `vc4_hdmi_stop_packet`.
    fn stop_packet(&self, hw: &mut impl Hw, r#type: u32, poll: bool) -> i32 {
        let packet_id = r#type - 0x80;

        let v = self.read(hw, &HDMI_RAM_PACKET_CONFIG);
        self.write(hw, &HDMI_RAM_PACKET_CONFIG, v & !(1 << packet_id));

        if poll {
            return wait_for(hw, 100, |hw| self.read(hw, &HDMI_RAM_PACKET_STATUS) & (1 << packet_id) == 0);
        }

        0
    }

    /// `vc4_hdmi_write_infoframe`: the packed frame into its slot of the
    /// packet RAM, seven bytes per two words, the rest of the slot
    /// cleared, then sent.
    fn write_infoframe(&self, hw: &mut impl Hw, r#type: u32, infoframe: &[u8]) -> i32 {
        let packet_id = r#type - 0x80;
        let (window, start) = HDMI_RAM_PACKET_START[self.port as usize];
        let mut packet_reg = start + VC4_HDMI_PACKET_STRIDE * packet_id;
        let packet_reg_next = start + VC4_HDMI_PACKET_STRIDE * (packet_id + 1);
        let block = Block::Hdmi(self.port, window);
        let mut buffer = [0u8; VC4_HDMI_PACKET_STRIDE as usize];

        if infoframe.len() > VC4_HDMI_PACKET_SIZE as usize {
            return -ENOMEM;
        }

        buffer[..infoframe.len()].copy_from_slice(infoframe);

        // WARN_ONCE: the packet RAM has to be on to store the packet.
        let _ = self.read(hw, &HDMI_RAM_PACKET_CONFIG) & VC4_HDMI_RAM_PACKET_ENABLE;

        let ret = self.stop_packet(hw, r#type, true);
        if ret != 0 {
            drm_core::linux::Line::new().s(&format!("Failed to wait for infoframe to go idle: {}", ret)).say();
            return ret;
        }

        let mut i = 0;
        while i < infoframe.len() {
            let b = |k: usize| buffer[i + k] as u32;
            hw.write(block, packet_reg, b(0) | b(1) << 8 | b(2) << 16);
            packet_reg += 4;

            if packet_reg >= packet_reg_next {
                break;
            }

            hw.write(block, packet_reg, b(3) | b(4) << 8 | b(5) << 16 | b(6) << 24);
            packet_reg += 4;
            i += 7;
        }

        // clear remainder of packet ram as it's included in the
        // infoframe and triggers a checksum error on hdmi analyser
        while packet_reg < packet_reg_next {
            hw.write(block, packet_reg, 0);
            packet_reg += 4;
        }

        let v = self.read(hw, &HDMI_RAM_PACKET_CONFIG);
        self.write(hw, &HDMI_RAM_PACKET_CONFIG, v | (1 << packet_id));

        let ret = wait_for(hw, 100, |hw| self.read(hw, &HDMI_RAM_PACKET_STATUS) & (1 << packet_id) != 0);
        if ret != 0 {
            drm_core::linux::Line::new().s(&format!("Failed to wait for infoframe to start: {}", ret)).say();
        }

        ret
    }

    /// `vc4_hdmi_clear_infoframe`.
    fn clear_infoframe(&self, hw: &mut impl Hw, r#type: u32) -> i32 {
        let ret = self.stop_packet(hw, r#type, true);
        if ret != 0 {
            drm_core::linux::Line::new().s(&format!("Failed to wait for infoframe to go idle: {}", ret)).say();
        }

        ret
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
    connector.max_bpc = if BCM2712_SUPPORTS_HDR != 0 { 12 } else { 8 };
    connector.hdmi.vendor = String::from("Broadcom");
    connector.hdmi.product = String::from("Videocore");
    connector.hdmi.supported_formats =
        1 << HDMI_COLORSPACE_RGB | 1 << HDMI_COLORSPACE_YUV422 | 1 << HDMI_COLORSPACE_YUV444;
    connector
}

/// `vc4_hdmi_supports_scrambling`.
pub fn vc4_hdmi_supports_scrambling(connector: &drm_connector) -> bool {
    let display = &connector.display_info;

    if !display.is_hdmi {
        return false;
    }

    display.hdmi.scdc.supported && display.hdmi.scdc.scrambling.supported
}

/// `vc5_hdmi_find_yuv_csc_coeffs`: the `[3][4]` matrix for a colorspace.
fn vc5_hdmi_find_yuv_csc_coeffs(colorspace: u32, limited: usize) -> &'static [u32] {
    let table: &'static [u32; 24] = match colorspace {
        DRM_MODE_COLORIMETRY_SMPTE_170M_YCC
        | DRM_MODE_COLORIMETRY_XVYCC_601
        | DRM_MODE_COLORIMETRY_SYCC_601
        | DRM_MODE_COLORIMETRY_OPYCC_601
        | DRM_MODE_COLORIMETRY_BT601_YCC => &VC5_HDMI_CSC_FULL_RGB_TO_YUV_BT601,

        DRM_MODE_COLORIMETRY_BT2020_CYCC
        | DRM_MODE_COLORIMETRY_BT2020_YCC
        | DRM_MODE_COLORIMETRY_BT2020_RGB
        | DRM_MODE_COLORIMETRY_DCI_P3_RGB_D65
        | DRM_MODE_COLORIMETRY_DCI_P3_RGB_THEATER => &VC5_HDMI_CSC_FULL_RGB_TO_YUV_BT2020,

        _ => &VC5_HDMI_CSC_FULL_RGB_TO_YUV_BT709,
    };

    &table[limited * 12..limited * 12 + 12]
}

/// vc4's `drm_connector_hdmi_funcs` without hardware: the TMDS check
/// the mode validation needs.
pub struct Vc4HdmiClock<'a> {
    pub hvs: &'a Hvs,
}

impl drm_connector_hdmi_funcs for Vc4HdmiClock<'_> {
    fn tmds_char_rate_valid(&mut self, _connector: &drm_connector, mode: &drm_display_mode, clock: u64)
        -> drm_mode_status
    {
        vc4_hdmi_connector_clock_valid(self.hvs, mode, clock)
    }
}

/// `vc4_hdmi_hdmi_connector_funcs` on one controller.
pub struct Vc4HdmiFuncs<'a, H: Hw> {
    pub hdmi: &'a Vc4Hdmi,
    pub hw: &'a mut H,
    pub hvs: &'a Hvs,
}

impl<H: Hw> drm_connector_hdmi_funcs for Vc4HdmiFuncs<'_, H> {
    fn tmds_char_rate_valid(&mut self, _connector: &drm_connector, mode: &drm_display_mode, clock: u64)
        -> drm_mode_status
    {
        vc4_hdmi_connector_clock_valid(self.hvs, mode, clock)
    }

    fn write_infoframe(&mut self, _connector: &drm_connector, r#type: u32, buffer: &[u8]) -> i32 {
        self.hdmi.write_infoframe(self.hw, r#type, buffer)
    }

    fn clear_infoframe(&mut self, _connector: &drm_connector, r#type: u32) -> i32 {
        self.hdmi.clear_infoframe(self.hw, r#type)
    }
}

/// `vc4_hdmi_connector_atomic_check`: Ok(true) when the CRTC needs a
/// modeset. Changed margins add the CRTC's planes to the commit in
/// Linux; a commit here always carries them.
pub fn vc4_hdmi_connector_atomic_check(connector: &drm_connector, old_state: &drm_connector_state,
    new_state: &mut drm_connector_state, mode: &drm_display_mode, hvs: &Hvs) -> Result<bool, i32>
{
    if new_state.crtc.is_none() {
        return Ok(false);
    }

    let colorspace_changed = old_state.colorspace != new_state.colorspace;

    let changed = drm_atomic_helper_connector_hdmi_check(connector, old_state, new_state, mode,
        &mut Vc4HdmiClock { hvs })?;

    Ok(colorspace_changed || changed)
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
    drm_hdmi_connector_mode_valid(connector, mode, &mut Vc4HdmiClock { hvs })
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
