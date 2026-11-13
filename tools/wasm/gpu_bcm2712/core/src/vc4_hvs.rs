//! `drivers/gpu/drm/vc4/vc4_hvs.c` (Linux rpi-6.18), the BCM2712 (gen 6)
//! paths: the Hardware Video Scaler that composes the planes of each
//! output from its display list.
//!
//! Only VC4_GEN_6_C and VC4_GEN_6_D exist here; the gen 4/5 branches of
//! Linux are for older boards. The debugfs register sets are not ported.

use crate::clk::{rpi_firmware_clk_get_max_rate, Clk, FwClocks};
use crate::drm_mm::{DrmMm, DrmMmNode};
use crate::hw::{Block, Hw, RPI_FIRMWARE_CORE_CLK_ID, RPI_FIRMWARE_DISP_CLK_ID};
use crate::irq_brcmstb_l2::L2Intc;
use crate::linux::{field_get, field_prep};
use crate::regs::*;

/// `enum vc4_gen`, the BCM2712's two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gen {
    /// VC4_GEN_6_C: BCM2712 C1.
    Gen6C,
    /// VC4_GEN_6_D: BCM2712 D0.
    Gen6D,
}

impl Gen {
    /// The index of a `[C1, D0]` value in `regs`.
    pub fn idx(self) -> usize {
        self as usize
    }
}

/// The `disp_intr` lines of the HVS's `ch0-eof`, `ch1-eof`, `ch2-eof`.
pub const EOF_IRQ: [u32; HVS_NUM_CHANNELS as usize] = [2, 9, 16];

/// `struct vc4_lbm_refcounts` / `struct vc4_upm_refcounts`, with the
/// handle's place in its `ida`.
#[derive(Clone, Copy, Debug, Default)]
pub struct MmRefcount {
    pub refcount: u32,
    pub size: u32,
    pub node: DrmMmNode,
    pub allocated: bool,
}

/// `struct vc4_hvs_dlist_allocation`: a CRTC's space in the dlist memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DlistAlloc {
    pub node: DrmMmNode,
    pub channel: u32,
    pub target_frame_count: u8,
    pub dlist_programmed: bool,
}

/// `struct vc4_hvs_state`'s `fifo_state[i]`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FifoState {
    pub in_use: bool,
    pub fifo_load: u64,
    /// A commit on the channel whose hardware is not done yet.
    pub pending_commit: bool,
}

/// `struct vc4_hvs_state`, the HVS's part of the atomic state.
#[derive(Clone, Copy, Debug, Default)]
pub struct Vc4HvsState {
    pub fifo_state: [FifoState; HVS_NUM_CHANNELS as usize],
    pub core_clock_rate: u64,
}

/// `ida_alloc_range(ida, 1, max)`: the lowest free handle.
pub fn ida_alloc_range(refcounts: &mut [MmRefcount], max: u32) -> Option<u32> {
    let h = (1..=max).find(|&h| !refcounts[h as usize].allocated)?;
    refcounts[h as usize].allocated = true;
    Some(h)
}

/// `struct vc4_hvs`, the fields the gen 6 paths use.
pub struct Hvs {
    /// `vc4->gen`.
    pub generation: Gen,
    /// The HVS's dlist memory in words (`SCALER6_CXM_SIZE`).
    pub dlist_mem_size: u32,
    pub dlist_mm: DrmMm,
    pub lbm_mm: DrmMm,
    pub upm_mm: DrmMm,
    pub mitchell_netravali_filter: DrmMmNode,
    pub nearest_neighbour_filter: DrmMmNode,
    pub core_clk: Clk,
    pub disp_clk: Clk,
    pub max_core_rate: u32,
    pub vc5_hdmi_enable_hdmi_20: bool,
    pub vc5_hdmi_enable_4096by2160: bool,
    /// `lbm_refcounts[VC4_NUM_LBM_HANDLES + 1]` with `lbm_handles`.
    pub lbm_refcounts: alloc::vec::Vec<MmRefcount>,
    /// `upm_refcounts[VC4_NUM_UPM_HANDLES + 1]` with `upm_handles`.
    pub upm_refcounts: alloc::vec::Vec<MmRefcount>,
    pub bg_fill: [bool; HVS_NUM_CHANNELS as usize],
    /// `eof_irq[i].enabled`.
    pub eof_irq_enabled: [bool; HVS_NUM_CHANNELS as usize],
    pub stale_dlist_entries: alloc::vec::Vec<DlistAlloc>,
}

impl Hvs {
    fn read(&self, hw: &mut impl Hw, off: u32) -> u32 {
        hw.read(Block::Hvs, off)
    }

    fn write(&self, hw: &mut impl Hw, off: u32, v: u32) {
        hw.write(Block::Hvs, off, v)
    }

    /// `hvs->dlist[i]`, in the HVS's own memory.
    pub fn dlist_write(&self, hw: &mut impl Hw, i: u32, v: u32) {
        hw.write(Block::Hvs, SCALER5_DLIST_START + 4 * i, v)
    }

    /// `__vc4_hvs_alloc` for gen 6, with `vc4->gen` still VC4_GEN_6_C (the
    /// compatible's) as when Linux calls it. Linux sets `dlist_mm` up twice;
    /// the second, from `SCALER_DLIST_SIZE`, is the one that stays.
    fn alloc(hw: &mut impl Hw, clocks: &mut FwClocks) -> Self {
        let dlist_mem_size = hw.read(Block::Hvs, SCALER6_CXM_SIZE);
        Hvs {
            generation: Gen::Gen6C,
            dlist_mem_size,
            dlist_mm: DrmMm::new(HVS_BOOTLOADER_DLIST_END as u64,
                ((SCALER_DLIST_SIZE >> 2) - HVS_BOOTLOADER_DLIST_END) as u64),
            lbm_mm: DrmMm::new(0, 1024),
            upm_mm: DrmMm::new(0, 1024 * HVS_UBM_WORD_SIZE as u64),
            mitchell_netravali_filter: DrmMmNode::default(),
            nearest_neighbour_filter: DrmMmNode::default(),
            core_clk: clocks.get(RPI_FIRMWARE_CORE_CLK_ID),
            disp_clk: clocks.get(RPI_FIRMWARE_DISP_CLK_ID),
            max_core_rate: 0,
            vc5_hdmi_enable_hdmi_20: false,
            vc5_hdmi_enable_4096by2160: false,
            lbm_refcounts: alloc::vec![MmRefcount::default(); VC4_NUM_LBM_HANDLES as usize + 1],
            upm_refcounts: alloc::vec![MmRefcount::default(); VC4_NUM_UPM_HANDLES as usize + 1],
            bg_fill: [false; HVS_NUM_CHANNELS as usize],
            eof_irq_enabled: [false; HVS_NUM_CHANNELS as usize],
            stale_dlist_entries: alloc::vec::Vec::new(),
        }
    }

    /// `vc6_hvs_hw_init`.
    fn hw_init(&self, hw: &mut impl Hw) {
        self.write(hw, SCALER6_CONTROL, SCALER6_CONTROL_HVS_EN
            | field_prep(SCALER6_CONTROL_PF_LINES_MASK, 8)
            | field_prep(SCALER6_CONTROL_MAX_REQS_MASK, 15));
        let g = self.generation.idx();
        self.write(hw, SCALER6_PRI_MAP0[g], 0xffff_ffff);
        self.write(hw, SCALER6_PRI_MAP1[g], 0xffff_ffff);
        match self.generation {
            Gen::Gen6C => {
                let regs = [
                    [&CFC1_N_MA_CSC_COEFF_C00, &CFC1_N_MA_CSC_COEFF_C01, &CFC1_N_MA_CSC_COEFF_C02,
                     &CFC1_N_MA_CSC_COEFF_C03, &CFC1_N_MA_CSC_COEFF_C04],
                    [&CFC1_N_MA_CSC_COEFF_C10, &CFC1_N_MA_CSC_COEFF_C11, &CFC1_N_MA_CSC_COEFF_C12,
                     &CFC1_N_MA_CSC_COEFF_C13, &CFC1_N_MA_CSC_COEFF_C14],
                    [&CFC1_N_MA_CSC_COEFF_C20, &CFC1_N_MA_CSC_COEFF_C21, &CFC1_N_MA_CSC_COEFF_C22,
                     &CFC1_N_MA_CSC_COEFF_C23, &CFC1_N_MA_CSC_COEFF_C24],
                ];
                for i in 0..6 {
                    for (r, row) in regs.iter().enumerate() {
                        for (c, reg) in row.iter().enumerate() {
                            self.write(hw, reg[i], CSC_COEFFS[i * 15 + r * 5 + c]);
                        }
                    }
                    self.write(hw, CFC1_N_NL_CSC_CTRL[i], 1 << 15);
                }
            }
            Gen::Gen6D => {
                for i in 0..6 {
                    let c = &VC6_D0_CSC_COEFFS[i * 6..i * 6 + 6];
                    self.write(hw, SCALER_PI_CMP_CSC_RED0[i], c[0]);
                    self.write(hw, SCALER_PI_CMP_CSC_RED1[i], c[1]);
                    self.write(hw, SCALER_PI_CMP_CSC_RED_CLAMP[i], 0xfff0_0000);
                    self.write(hw, SCALER_PI_CMP_CSC_CFG[i], 1);
                    self.write(hw, SCALER_PI_CMP_CSC_GREEN0[i], c[2]);
                    self.write(hw, SCALER_PI_CMP_CSC_GREEN1[i], c[3]);
                    self.write(hw, SCALER_PI_CMP_CSC_GREEN_CLAMP[i], 0xfff0_0000);
                    self.write(hw, SCALER_PI_CMP_CSC_BLUE0[i], c[4]);
                    self.write(hw, SCALER_PI_CMP_CSC_BLUE1[i], c[5]);
                    self.write(hw, SCALER_PI_CMP_CSC_BLUE_CLAMP[i], 0xfff0_0000);
                }
            }
        }
    }

    /// `vc4_hvs_upload_linear_kernel`.
    fn upload_linear_kernel(&mut self, hw: &mut impl Hw, kernel: &[u32; 11]) -> Result<DrmMmNode, i32> {
        let space = self.dlist_mm.insert_node(VC4_KERNEL_DWORDS as u64)?;
        for (i, &k) in kernel.iter().enumerate() {
            self.dlist_write(hw, space.start as u32 + i as u32, k);
        }
        Ok(space)
    }

    /// `vc4_hvs_cob_init`, gen 6.
    fn cob_init(&self, hw: &mut impl Hw) {
        const VC6_COB_LINE_WIDTH: u32 = 3840;
        const VC6_COB_NUM_LINES: u32 = 4;
        let cob = |top, base| field_prep(SCALER6_DISPX_COB_TOP_MASK, top) | field_prep(SCALER6_DISPX_COB_BASE_MASK, base);
        let g = self.generation.idx();
        let mut base = 0;
        let mut top = 3840;
        self.write(hw, SCALER6_DISPX_COB[g][2], cob(top, base));
        base = top + 16;
        top += VC6_COB_LINE_WIDTH * VC6_COB_NUM_LINES;
        self.write(hw, SCALER6_DISPX_COB[g][1], cob(top, base));
        base = top + 16;
        top += VC6_COB_LINE_WIDTH * VC6_COB_NUM_LINES;
        self.write(hw, SCALER6_DISPX_COB[g][0], cob(top, base));
    }

    /// `vc4_hvs_bind` for gen 6. The three EOF lines are requested
    /// disabled (`IRQF_NO_AUTOEN`) on `disp_intr`.
    pub fn bind(hw: &mut impl Hw, disp_intr: &mut L2Intc, clocks: &mut FwClocks) -> Result<Self, i32> {
        let mut hvs = Hvs::alloc(hw, clocks);
        if field_get(SCALER6_VERSION_MASK, hvs.read(hw, SCALER6_VERSION)) == SCALER6_VERSION_D0 {
            hvs.generation = Gen::Gen6D;
        }
        let max_rate = rpi_firmware_clk_get_max_rate(hw, RPI_FIRMWARE_CORE_CLK_ID);
        hvs.vc5_hdmi_enable_hdmi_20 = max_rate >= 550_000_000;
        hvs.vc5_hdmi_enable_4096by2160 = max_rate >= 600_000_000;
        hvs.max_core_rate = max_rate;
        clocks.prepare_enable(hw, hvs.core_clk)?;
        clocks.prepare_enable(hw, hvs.disp_clk)?;
        hvs.hw_init(hw);
        hvs.mitchell_netravali_filter = hvs.upload_linear_kernel(hw, &MITCHELL_NETRAVALI_1_3_1_3_KERNEL)?;
        hvs.nearest_neighbour_filter = hvs.upload_linear_kernel(hw, &NEAREST_NEIGHBOUR_KERNEL)?;
        hvs.cob_init(hw);
        for line in EOF_IRQ {
            disp_intr.request_irq_noautoen(line);
        }
        Ok(hvs)
    }

    /// `vc4_hvs_get_fifo_from_output`, gen 6: the channel feeding output
    /// `output`.
    pub fn get_fifo_from_output(&self, output: u32) -> Option<u32> {
        match output {
            0 => Some(0),
            2 => Some(2),
            1 | 3 | 4 => Some(1),
            _ => None,
        }
    }

    /// `vc4_hvs_stop_channel` → `__vc6_hvs_stop_channel`.
    pub fn stop_channel(&self, hw: &mut impl Hw, chan: u32) {
        let g = self.generation.idx();
        let ctrl0 = SCALER6_DISPX_CTRL0[g][chan as usize];
        if self.read(hw, ctrl0) & SCALER6_DISPX_CTRL0_ENB == 0 {
            return;
        }
        let v = self.read(hw, ctrl0);
        self.write(hw, ctrl0, v | SCALER6_DISPX_CTRL0_RESET);
        let v = self.read(hw, ctrl0);
        self.write(hw, ctrl0, v & !SCALER6_DISPX_CTRL0_ENB);
        // Linux WARNs once if the channel did not stop.
        let _ = field_get(SCALER6_DISPX_STATUS_MODE_MASK, self.read(hw, SCALER6_DISPX_STATUS[g][chan as usize]))
            != SCALER6_DISPX_STATUS_MODE_DISABLED;
    }
}

impl Hvs {
    /// `vc4_hvs_get_fifo_frame_count`, gen 6.
    pub fn get_fifo_frame_count(&self, hw: &mut impl Hw, fifo: u32) -> u8 {
        let g = self.generation.idx();
        field_get(SCALER6_DISPX_STATUS_FRCNT_MASK, self.read(hw, SCALER6_DISPX_STATUS[g][fifo as usize])) as u8
    }

    /// `vc4_hvs_check_channel_active`, gen 6.
    fn check_channel_active(&self, hw: &mut impl Hw, fifo: u32) -> bool {
        let g = self.generation.idx();
        self.read(hw, SCALER6_DISPX_CTRL0[g][fifo as usize]) & SCALER6_DISPX_CTRL0_ENB != 0
    }

    /// `vc4_hvs_alloc_dlist_entry`: `dlist_count` words for `channel`;
    /// None for a disabled channel. When the space is gone, the stale
    /// entries are freed first.
    pub fn alloc_dlist_entry(&mut self, channel: Option<u32>, dlist_count: u32) -> Result<Option<DlistAlloc>, i32> {
        let Some(channel) = channel else { return Ok(None) };

        let node = match self.dlist_mm.insert_node(dlist_count as u64) {
            Ok(node) => node,
            Err(_) => {
                drm_core::linux::Line::new().s("Failed to allocate DLIST entry").say();

                // This should never happen as stale entries should get
                // released as the frame counter interrupt triggers.
                for cur in core::mem::take(&mut self.stale_dlist_entries) {
                    self.dlist_mm.remove_node(cur.node);
                }

                self.dlist_mm.insert_node(dlist_count as u64)?
            }
        };

        Ok(Some(DlistAlloc { node, channel, target_frame_count: 0, dlist_programmed: false }))
    }

    /// `vc4_hvs_irq_enable_eof`, gen 6: the channel's EOF line on.
    fn irq_enable_eof(&mut self, hw: &mut impl Hw, disp_intr: &mut L2Intc, channel: u32) {
        if self.eof_irq_enabled[channel as usize] {
            return;
        }

        disp_intr.enable_irq(hw, EOF_IRQ[channel as usize]);

        self.eof_irq_enabled[channel as usize] = true;
    }

    /// `vc4_hvs_irq_clear_eof`, gen 6.
    fn irq_clear_eof(&mut self, disp_intr: &mut L2Intc, channel: u32) {
        if !self.eof_irq_enabled[channel as usize] {
            return;
        }

        disp_intr.disable_irq(EOF_IRQ[channel as usize]);

        self.eof_irq_enabled[channel as usize] = false;
    }

    /// `vc4_hvs_mark_dlist_entry_stale`: an entry the hardware may still
    /// read waits for the next frame; one never programmed goes at once.
    pub fn mark_dlist_entry_stale(&mut self, hw: &mut impl Hw, disp_intr: &mut L2Intc, alloc: Option<DlistAlloc>) {
        let Some(mut alloc) = alloc else { return };

        if !alloc.dlist_programmed {
            self.dlist_mm.remove_node(alloc.node);
            return;
        }

        let frcnt = self.get_fifo_frame_count(hw, alloc.channel);
        alloc.target_frame_count = (frcnt + 1) & ((1 << 6) - 1);

        self.stale_dlist_entries.push(alloc);

        self.write(hw, SCALER_DISPSTAT, SCALER_DISPSTAT_EOF[alloc.channel as usize]);
        self.irq_enable_eof(hw, disp_intr, alloc.channel);
    }

    /// `vc6_hvs_eof_irq_handler` for the channel whose EOF line fired:
    /// the background fill as the last flush wanted, then the sweep.
    pub fn eof_irq_handler(&mut self, hw: &mut impl Hw, disp_intr: &mut L2Intc, channel: u32) -> bool {
        let i = channel as usize;
        if !self.eof_irq_enabled[i] {
            return false;
        }

        let g = self.generation.idx();
        let ctrl1 = SCALER6_DISPX_CTRL1[g][i];
        let v = self.read(hw, ctrl1);
        if self.bg_fill[i] {
            self.write(hw, ctrl1, v | SCALER6_DISPX_CTRL1_BGENB);
        } else {
            self.write(hw, ctrl1, v & !SCALER6_DISPX_CTRL1_BGENB);
        }

        self.schedule_dlist_sweep(hw, disp_intr, channel);
        true
    }

    /// `vc4_hvs_schedule_dlist_sweep`, the free work run in place.
    fn schedule_dlist_sweep(&mut self, hw: &mut impl Hw, disp_intr: &mut L2Intc, channel: u32) {
        if !self.stale_dlist_entries.is_empty() {
            self.dlist_free_work(hw);
        }

        if self.stale_dlist_entries.is_empty() {
            self.irq_clear_eof(disp_intr, channel);
        }
    }

    /// `vc4_hvs_dlist_free_work`: the stale entries whose frame has come.
    fn dlist_free_work(&mut self, hw: &mut impl Hw) {
        let mut frcnt = [0u8; 3];
        let mut active = [false; 3];
        for i in 0..3 {
            frcnt[i] = self.get_fifo_frame_count(hw, i as u32);
            active[i] = self.check_channel_active(hw, i as u32);
        }

        let entries = core::mem::take(&mut self.stale_dlist_entries);
        for cur in entries {
            let c = cur.channel as usize;
            if active[c] && !vc4_hvs_frcnt_lte(cur.target_frame_count, frcnt[c]) {
                self.stale_dlist_entries.push(cur);
                continue;
            }

            self.dlist_mm.remove_node(cur.node);
        }
    }

    /// `vc4_hvs_install_dlist`, gen 6.
    pub fn install_dlist(&self, hw: &mut impl Hw, channel: u32, mm: &mut DlistAlloc) {
        mm.dlist_programmed = true;

        let g = self.generation.idx();
        self.write(hw, SCALER6_DISPX_LPTRS[g][channel as usize],
            field_prep(SCALER6_DISPX_LPTRS_HEADE_MASK, mm.node.start as u32));
    }

    /// `vc6_hvs_init_channel`.
    pub fn init_channel(&self, hw: &mut impl Hw, channel: u32, mode: &drm_core::drm_modes::drm_display_mode,
        oneshot: bool)
    {
        let g = self.generation.idx();
        let interlace = mode.flags & drm_core::regs::DRM_MODE_FLAG_INTERLACE != 0;

        // Doubled for frame packing stereo modes
        let (hdisplay, vdisplay) = drm_core::drm_modes::drm_mode_get_hv_timing(mode);

        self.write(hw, SCALER6_DISPX_CTRL0[g][channel as usize], SCALER6_DISPX_CTRL0_RESET);

        let mut disp_ctrl1 = self.read(hw, SCALER6_DISPX_CTRL1[g][channel as usize]);
        disp_ctrl1 &= !SCALER6_DISPX_CTRL1_INTLACE;
        self.write(hw, SCALER6_DISPX_CTRL1[g][channel as usize],
            disp_ctrl1 | if interlace { SCALER6_DISPX_CTRL1_INTLACE } else { 0 });

        self.write(hw, SCALER6_DISPX_CTRL0[g][channel as usize],
            SCALER6_DISPX_CTRL0_ENB
                | field_prep(SCALER6_DISPX_CTRL0_FWIDTH_MASK, hdisplay as u32 - 1)
                | if oneshot { SCALER6_DISPX_CTRL0_ONESHOT } else { 0 }
                | field_prep(SCALER6_DISPX_CTRL0_LINES_MASK, vdisplay as u32 - 1));
    }

    /// `vc4_crtc_get_cob_allocation`, gen 6: the channel's FIFO size.
    pub fn get_cob_allocation(&self, hw: &mut impl Hw, channel: u32) -> u32 {
        let g = self.generation.idx();
        let dispbase = self.read(hw, SCALER6_DISPX_COB[g][channel as usize]);
        // Top/base are supposed to be 4-pixel aligned, but the Raspberry
        // Pi firmware fills the low bits (which are presumably ignored).
        let top = field_get(SCALER6_DISPX_COB_TOP_MASK, dispbase) & !3;
        let base = field_get(SCALER6_DISPX_COB_BASE_MASK, dispbase) & !3;

        top.wrapping_sub(base).wrapping_add(4)
    }

    /// The flush's background colour from the CRTC state's 16-bit ARGB:
    /// 8 bits per component on C1, 12 on D0.
    pub fn write_background(&mut self, hw: &mut impl Hw, channel: u32, bgcolor: u64, enable_bg_fill: bool) {
        let c = channel as usize;
        let get = |shift: u32, bpc: u32| ((bgcolor >> shift) as u16 >> (16 - bpc)) as u32;

        if self.generation == Gen::Gen6C {
            // This sets the background color fill, as is the case with
            // other DRM drivers.
            self.write(hw, SCALER6_DISPX_BGND[self.generation.idx()][c],
                field_prep(SCALER6_DISPX_BGND_FILL_RED_MASK, get(32, 8))
                    | field_prep(SCALER6_DISPX_BGND_FILL_GREEN_MASK, get(16, 8))
                    | field_prep(SCALER6_DISPX_BGND_FILL_BLUE_MASK, get(0, 8))
                    | field_prep(SCALER6_DISPX_BGND_FILL_ALPHA_MASK, get(48, 8)));
            self.bg_fill[c] = enable_bg_fill;
            return;
        }

        // GEN_6_D takes a 12bit background colour
        self.write(hw, SCALER6D_DISPX_BGND0[c],
            field_prep(SCALER6D_DISPX_BGND0_FILL_GREEN_MASK, get(16, 12))
                | field_prep(SCALER6D_DISPX_BGND0_FILL_BLUE_MASK, get(0, 12)));
        self.write(hw, SCALER6D_DISPX_BGND1[c],
            field_prep(SCALER6D_DISPX_BGND1_FILL_RED_MASK, get(32, 12))
                | field_prep(SCALER6D_DISPX_BGND1_FILL_ALPHA_MASK, get(48, 12)));
        self.bg_fill[c] = enable_bg_fill;
    }
}

/// `vc4_hvs_frcnt_lte`: cnt1 <= cnt2 on the 6-bit frame counter.
fn vc4_hvs_frcnt_lte(cnt1: u8, cnt2: u8) -> bool {
    ((cnt1 << 2).wrapping_sub(cnt2 << 2) as i8) <= 0
}
