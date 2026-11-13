//! `drivers/gpu/drm/vc4/vc4_crtc.c` (Linux rpi-6.18): the BCM2712's two
//! pixel valves (`bcm2712_pv0_data`, `bcm2712_pv1_data`), each feeding one
//! HDMI encoder from one HVS channel.

use crate::clk::FwClocks;
use crate::hw::{Block, Hw};
use crate::i2c_brcmstb::Bsc;
use crate::linux::field_get;
use crate::regs::*;
use crate::vc4_hdmi::Vc4Hdmi;
use crate::linux::field_prep;
use crate::vc4_hvs::{DlistAlloc, Hvs};
use drm_core::drm_modes::{drm_display_mode, drm_mode_status};
use drm_core::drm_vblank::drm_vblank_funcs;
use drm_core::regs::{DRM_MODE_FLAG_DBLCLK, DRM_MODE_FLAG_DBLSCAN, DRM_MODE_FLAG_INTERLACE, MODE_NO_DBLESCAN, MODE_OK};

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

/// `struct vc4_crtc_state` with its `struct drm_crtc_state`, the members
/// the gen 6 paths read. Margins are left, right, top, bottom.
#[derive(Clone, Debug, Default)]
pub struct Vc4CrtcState {
    pub enable: bool,
    pub active: bool,
    pub mode: drm_display_mode,
    pub adjusted_mode: drm_display_mode,
    pub mode_changed: bool,
    pub active_changed: bool,
    pub connectors_changed: bool,
    pub zpos_changed: bool,
    pub color_mgmt_changed: bool,
    pub plane_mask: u32,
    pub connector_mask: u32,
    pub encoder_mask: u32,
    /// `event`: the commit completes on the next vblank after it.
    pub event: bool,
    pub background_color: u64,
    /// `assigned_channel`; None is VC4_HVS_CHANNEL_DISABLED.
    pub assigned_channel: Option<u32>,
    pub update_muxing: bool,
    pub hvs_load: u64,
    pub margins: [u32; 4],
    pub mm: Option<DlistAlloc>,
}

impl Vc4CrtcState {
    /// `vc4_crtc_reset` → `__drm_atomic_helper_crtc_reset`.
    pub fn reset() -> Self {
        Vc4CrtcState {
            background_color: 0xffff << 48,
            assigned_channel: None,
            ..Default::default()
        }
    }

    /// `drm_atomic_crtc_needs_modeset`.
    pub fn needs_modeset(&self) -> bool {
        self.mode_changed || self.active_changed || self.connectors_changed
    }
}

/// `struct vc4_crtc`.
pub struct Vc4Crtc {
    pub pv: u8,
    pub data: &'static PvData,
    /// `current_hvs_channel` and `current_dlist`, for the page flip.
    pub current_hvs_channel: u32,
    pub current_dlist: u32,
    /// `event`: a commit waiting for its flip.
    pub event: bool,
    /// `t_vblank` (ns).
    pub t_vblank: i64,
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
        let c = Vc4Crtc {
            pv,
            data: &BCM2712_PV_DATA[pv as usize],
            current_hvs_channel: 0,
            current_dlist: 0,
            event: false,
            t_vblank: 0,
        };
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
    pub fn disable(&self, hw: &mut impl Hw, hvs: &Hvs, hdmi: &mut Vc4Hdmi, bsc: &mut Bsc, clocks: &mut FwClocks, channel: u32) {
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

    /// `vc4_get_fifo_full_level` for `PV_CONTROL_FORMAT_24` (HDMI).
    fn get_fifo_full_level(&self) -> u32 {
        let fifo_len_bytes = self.data.fifo_depth;

        // For some reason, the pixelvalve4 doesn't work with the usual
        // formula and will only work with 32.
        if self.data.hvs_output == 5 {
            return 32;
        }

        // Pixels are pulled from the HVS if the number of bytes is lower
        // than the FIFO full level; the fetch latency is 6 pixels.
        fifo_len_bytes - 3 * HVS_FIFO_LATENCY_PIX
    }

    /// `vc4_crtc_get_fifo_full_level_bits`.
    fn get_fifo_full_level_bits(&self) -> u32 {
        let level = self.get_fifo_full_level();

        field_prep(PV5_CONTROL_FIFO_LEVEL_HIGH_MASK, level >> 6) | field_prep(PV_CONTROL_FIFO_LEVEL_MASK, level & 0x3f)
    }

    /// `vc4_crtc_config_pv` for an HDMI encoder (`clock_select` 0), on
    /// the state's adjusted mode.
    pub fn config_pv(&self, hw: &mut impl Hw, mode: &drm_display_mode) {
        let interlace = mode.flags & DRM_MODE_FLAG_INTERLACE != 0;
        let is_hdmi = true;
        let pixel_rep: u32 = if mode.flags & DRM_MODE_FLAG_DBLCLK != 0 && !is_hdmi { 2 } else { 1 };
        let format = PV_CONTROL_FORMAT_24;
        let ppc = if interlace { self.data.pixels_per_clock_int } else { self.data.pixels_per_clock };
        let clock_select = 0;

        let mut vert_bp = mode.crtc_vtotal.wrapping_sub(mode.crtc_vsync_end) as u32;
        let vert_sync = mode.crtc_vsync_end.wrapping_sub(mode.crtc_vsync_start) as u32;
        let vert_fp = mode.crtc_vsync_start.wrapping_sub(mode.crtc_vdisplay) as u32;

        self.pixelvalve_reset(hw);

        self.write(hw, PV_HORZA,
            field_prep(PV_HORZA_HBP_MASK, (mode.htotal - mode.hsync_end) as u32 * pixel_rep / ppc)
                | field_prep(PV_HORZA_HSYNC_MASK, (mode.hsync_end - mode.hsync_start) as u32 * pixel_rep / ppc));

        self.write(hw, PV_HORZB,
            field_prep(PV_HORZB_HFP_MASK, (mode.hsync_start - mode.hdisplay) as u32 * pixel_rep / ppc)
                | field_prep(PV_HORZB_HACTIVE_MASK, mode.hdisplay as u32 * pixel_rep / ppc));

        if interlace {
            let odd_field_first = false;
            let field_delay = mode.htotal as u32 * pixel_rep / (2 * ppc);
            let vert_bp_even = vert_bp;
            let vert_fp_even = vert_fp;

            // Not a VEC (composite output): even field first.
            vert_bp += 1;

            self.write(hw, PV_VERTA_EVEN,
                field_prep(PV_VERTA_VBP_MASK, vert_bp_even) | field_prep(PV_VERTA_VSYNC_MASK, vert_sync));
            self.write(hw, PV_VERTB_EVEN,
                field_prep(PV_VERTB_VFP_MASK, vert_fp_even)
                    | field_prep(PV_VERTB_VACTIVE_MASK, mode.crtc_vdisplay as u32));

            // We set up first field even mode for HDMI and VEC's PAL.
            self.write(hw, PV_V_CONTROL,
                PV_VCONTROL_CONTINUOUS
                    | if ppc == 1 { PV_VCONTROL_ODD_TIMING } else { 0 }
                    | PV_VCONTROL_INTERLACE
                    | field_prep(PV_VCONTROL_ODD_DELAY_MASK, field_delay));
            self.write(hw, PV_VSYNCD_EVEN, if odd_field_first { field_delay } else { 0 });
        } else {
            self.write(hw, PV_V_CONTROL,
                PV_VCONTROL_CONTINUOUS | if ppc == 1 { PV_VCONTROL_ODD_TIMING } else { 0 });
            self.write(hw, PV_VSYNCD_EVEN, 0);
        }

        self.write(hw, PV_VERTA, field_prep(PV_VERTA_VBP_MASK, vert_bp) | field_prep(PV_VERTA_VSYNC_MASK, vert_sync));
        self.write(hw, PV_VERTB,
            field_prep(PV_VERTB_VFP_MASK, vert_fp) | field_prep(PV_VERTB_VACTIVE_MASK, mode.crtc_vdisplay as u32));

        self.write(hw, PV_MUX_CFG,
            field_prep(PV_MUX_CFG_RGB_PIXEL_MUX_MODE_MASK, PV_MUX_CFG_RGB_PIXEL_MUX_MODE_NO_SWAP));

        self.write(hw, PV_PIPE_INIT_CTRL,
            field_prep(PV_PIPE_INIT_CTRL_PV_INIT_WIDTH_MASK, 1)
                | field_prep(PV_PIPE_INIT_CTRL_PV_INIT_IDLE_MASK, 1)
                | PV_PIPE_INIT_CTRL_PV_INIT_EN);

        self.write(hw, PV_CONTROL,
            PV_CONTROL_FIFO_CLR
                | self.get_fifo_full_level_bits()
                | field_prep(PV_CONTROL_FORMAT_MASK, format)
                | field_prep(PV_CONTROL_PIXEL_REP_MASK, pixel_rep - 1)
                | PV_CONTROL_CLR_AT_START
                | PV_CONTROL_TRIGGER_UNDERFLOW
                | PV_CONTROL_WAIT_HSTART
                | field_prep(PV_CONTROL_CLK_SELECT_MASK, clock_select));
    }

    /// `PV_CONTROL |= PV_CONTROL_EN`.
    pub fn enable(&self, hw: &mut impl Hw) {
        let v = self.read(hw, PV_CONTROL);
        self.write(hw, PV_CONTROL, v | PV_CONTROL_EN);
    }

    /// `PV_V_CONTROL |= PV_VCONTROL_VIDEN`.
    pub fn video_enable(&self, hw: &mut impl Hw) {
        let v = self.read(hw, PV_V_CONTROL);
        self.write(hw, PV_V_CONTROL, v | PV_VCONTROL_VIDEN);
    }

    /// `vc4_crtc_irq_handler`'s status: was it a vblank (VFP start)? The
    /// status is cleared when it was.
    pub fn irq_status(&self, hw: &mut impl Hw) -> bool {
        let stat = self.read(hw, PV_INTSTAT);

        if stat & PV_INT_VFP_START != 0 {
            self.write(hw, PV_INTSTAT, PV_INT_VFP_START);
            return true;
        }

        false
    }

    /// `vc4_crtc_handle_page_flip`: Some when the commit waiting for
    /// this vblank completed (the HVS reads its dlist now).
    pub fn handle_page_flip(&mut self, hw: &mut impl Hw, hvs: &Hvs) -> bool {
        let g = hvs.generation.idx();
        let chan = self.current_hvs_channel as usize;
        let current_dlist = crate::linux::field_get(SCALER6_DISPX_DL_LACT_MASK, hw.read(crate::hw::Block::Hvs, SCALER6_DISPX_DL[g][chan]));

        if self.event && self.current_dlist == current_dlist {
            self.event = false;
            return true;
        }

        false
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

/// `vc4_crtc_mode_valid`.
pub fn vc4_crtc_mode_valid(mode: &drm_display_mode) -> drm_mode_status {
    // Do not allow doublescan modes from user space
    if mode.flags & DRM_MODE_FLAG_DBLSCAN != 0 {
        return MODE_NO_DBLESCAN as i32;
    }

    MODE_OK as i32
}

/// `vc4_crtc_get_scanout_position` and `vc4_enable_vblank` /
/// `vc4_disable_vblank` for the DRM core's vblank count.
pub struct CrtcVblank<'a, H: Hw> {
    pub crtc: &'a Vc4Crtc,
    pub hvs: &'a Hvs,
    pub hw: &'a mut H,
    /// `assigned_channel` of the CRTC's state.
    pub channel: u32,
}

impl<H: Hw> drm_vblank_funcs for CrtcVblank<'_, H> {
    fn get_scanout_position(&mut self, in_vblank_irq: bool, mode: &drm_display_mode)
        -> Option<(i32, i32, i64, i64)>
    {
        let g = self.hvs.generation.idx();
        let channel = self.channel;

        // Get optional system timestamp before query.
        let mut stime = self.ktime_get();

        // Read vertical scanline which is currently composed for our
        // pixelvalve by the HVS, and also the scaler status.
        let val = self.hw.read(crate::hw::Block::Hvs, SCALER6_DISPX_STATUS[g][channel as usize]);

        // Get optional system timestamp after query.
        let mut etime = self.ktime_get();

        // Vertical position of hvs composed scanline.
        let mut vpos = crate::linux::field_get(SCALER6_DISPX_STATUS_YLINE_MASK, val) as i32;
        let mut hpos = 0;

        if mode.flags & DRM_MODE_FLAG_INTERLACE != 0 {
            vpos /= 2;

            // Use hpos to correct for field offset in interlaced mode.
            if self.hvs.get_fifo_frame_count(self.hw, channel) % 2 != 0 {
                hpos += mode.crtc_htotal as i32 / 2;
            }
        }

        let cob_size = self.hvs.get_cob_allocation(self.hw, channel);
        // This is the offset we need for translating hvs -> pv scanout pos.
        let fifo_lines = (cob_size / mode.crtc_hdisplay as u32) as i32;

        let ret = fifo_lines > 0;

        // HVS more than fifo_lines into frame for compositing?
        if vpos > fifo_lines {
            // In active scanout the PV trails the HVS by the FIFO's
            // lines.
            vpos -= fifo_lines + 1;

            return ret.then_some((vpos, hpos, stime, etime));
        }

        // In vblank the HVS refills its FIFO from the new frame's top and
        // pauses; the position is made up consistently.
        let vblank_lines = mode.vtotal as i32 - mode.vdisplay as i32;

        if in_vblank_irq {
            // The irq handler runs close to the first line of vblank;
            // its entry time is the base timestamp.
            vpos = -vblank_lines;

            stime = self.crtc.t_vblank;
            etime = self.crtc.t_vblank;
        } else {
            // No clue where we are inside vblank. Return a vpos of zero,
            // which will cause calling code to just return the etime
            // timestamp uncorrected.
            vpos = 0;
        }

        ret.then_some((vpos, hpos, stime, etime))
    }

    fn enable_vblank(&mut self) -> i32 {
        self.hw.write(crate::hw::Block::Pv(self.crtc.pv), PV_INTEN, PV_INT_VFP_START);
        0
    }

    fn disable_vblank(&mut self) {
        self.hw.write(crate::hw::Block::Pv(self.crtc.pv), PV_INTEN, 0);
    }

    fn ktime_get(&mut self) -> i64 {
        self.hw.now_us() as i64 * 1000
    }
}

/// `vc4_crtc_atomic_check` after `vc4_hvs_atomic_check`: the HVS load of
/// the mode (HDMI 0's pixel valve counts the active part plus a margin)
/// and the connector's margins.
pub fn vc4_crtc_atomic_check(crtc: &Vc4Crtc, state: &mut Vc4CrtcState, conn_margins: Option<[u32; 4]>) {
    if state.encoder_mask != 0 {
        // In C's int, overflow included.
        let mode = &state.adjusted_mode;
        let clock = mode.crtc_clock;

        if crtc.data.hdmi == 0 {
            let a = clock.wrapping_mul(mode.hdisplay as i32) / mode.htotal as i32 + 8000;
            let b = clock.wrapping_mul(9) / 10;
            state.hvs_load = a.max(b).wrapping_mul(1000) as i64 as u64;
        } else {
            state.hvs_load = clock.wrapping_mul(1000) as i64 as u64;
        }
    }

    if let Some(m) = conn_margins {
        if state.margins != m {
            state.margins = m;

            // Need to force the dlist entries for all planes to be
            // updated so that the dest rectangles are changed.
            state.zpos_changed = true;
        }
    }
}
