//! `drivers/gpu/drm/vc4/vc4_hvs.c` (Linux rpi-6.18), the BCM2712 (gen 6)
//! paths: the Hardware Video Scaler that composes the planes of each
//! output from its display list.
//!
//! Only VC4_GEN_6_C and VC4_GEN_6_D exist here; the gen 4/5 branches of
//! Linux are for older boards. The debugfs register sets are not ported.

use crate::clk::{rpi_firmware_clk_get_max_rate, FwClk};
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
    pub core_clk: FwClk,
    pub disp_clk: FwClk,
    pub max_core_rate: u32,
    pub vc5_hdmi_enable_hdmi_20: bool,
    pub vc5_hdmi_enable_4096by2160: bool,
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
    fn alloc(hw: &mut impl Hw) -> Self {
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
            core_clk: FwClk::new(RPI_FIRMWARE_CORE_CLK_ID),
            disp_clk: FwClk::new(RPI_FIRMWARE_DISP_CLK_ID),
            max_core_rate: 0,
            vc5_hdmi_enable_hdmi_20: false,
            vc5_hdmi_enable_4096by2160: false,
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
    pub fn bind(hw: &mut impl Hw, disp_intr: &mut L2Intc) -> Result<Self, i32> {
        let mut hvs = Hvs::alloc(hw);
        if field_get(SCALER6_VERSION_MASK, hvs.read(hw, SCALER6_VERSION)) == SCALER6_VERSION_D0 {
            hvs.generation = Gen::Gen6D;
        }
        let max_rate = rpi_firmware_clk_get_max_rate(hw, RPI_FIRMWARE_CORE_CLK_ID);
        hvs.vc5_hdmi_enable_hdmi_20 = max_rate >= 550_000_000;
        hvs.vc5_hdmi_enable_4096by2160 = max_rate >= 600_000_000;
        hvs.max_core_rate = max_rate;
        hvs.core_clk.prepare_enable(hw)?;
        hvs.disp_clk.prepare_enable(hw)?;
        hvs.hw_init(hw);
        hvs.mitchell_netravali_filter = hvs.upload_linear_kernel(hw, &MITCHELL_NETRAVALI_1_3_1_3_KERNEL)?;
        hvs.nearest_neighbour_filter = hvs.upload_linear_kernel(hw, &NEAREST_NEIGHBOUR_KERNEL)?;
        hvs.cob_init(hw);
        for line in EOF_IRQ {
            disp_intr.request_irq_noautoen(line);
        }
        Ok(hvs)
    }
}
