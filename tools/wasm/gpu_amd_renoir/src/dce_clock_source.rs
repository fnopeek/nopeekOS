//! display/dc/dce/dce_clock_source.{c,h} + inc/clock_source.h +
//! include/grph_object_id.h, with the `clk_src_regs` table of
//! resource/dcn21/dcn21_resource.c (Linux 6.18.55): the clock-source block of
//! Renoir's resource pool — the first of the "Create resources" constructors.
//!
//! Renoir reaches `dce112_clk_src_construct` (through `dcn20_clk_src_construct`),
//! the short body: it stores the register pointers, reads `ext_clk_khz` from the
//! firmware info and returns — no spread-spectrum, no `calc_pll` (those are the
//! longer `dce110_clk_src_construct`, which Renoir does not take). So this unit
//! writes no registers; its substance is the register *table*, verified number
//! for number against Linux' own macro expansion by hostcheck/clk_src_check.rs.
#![allow(dead_code)]

use crate::dc_types::{dc_bios, MAX_PIPES};
use crate::regs::*;
use alloc::boxed::Box;

/// `enum clock_source_id` (include/grph_object_id.h).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u32)]
pub enum clock_source_id {
    #[default]
    CLOCK_SOURCE_ID_UNDEFINED = 0,
    CLOCK_SOURCE_ID_PLL0,
    CLOCK_SOURCE_ID_PLL1,
    CLOCK_SOURCE_ID_PLL2,
    /// (Phy) ref. clk. for DP.
    CLOCK_SOURCE_ID_EXTERNAL,
    CLOCK_SOURCE_ID_DCPLL,
    /// DENTIST.
    CLOCK_SOURCE_ID_DFS,
    /// VCE does not need a real PLL.
    CLOCK_SOURCE_ID_VCE,
    /// Distinguishes programming a pixel clock from the ID (Phy) clock.
    CLOCK_SOURCE_ID_DP_DTO,
    /// Combo PHY PLL defines (DC 11.2 and up).
    CLOCK_SOURCE_COMBO_PHY_PLL0,
    CLOCK_SOURCE_COMBO_PHY_PLL1,
    CLOCK_SOURCE_COMBO_PHY_PLL2,
    CLOCK_SOURCE_COMBO_PHY_PLL3,
    CLOCK_SOURCE_COMBO_PHY_PLL4,
    CLOCK_SOURCE_COMBO_PHY_PLL5,
    CLOCK_SOURCE_COMBO_DISPLAY_PLL0,
}

/// Which `clock_source_funcs` vtable `clock_source.funcs` points at. Linux tells
/// clock sources apart by this pointer — `dce112` and `dcn20` share the construct
/// body but take different tables. The ops are a later stage (none is called on
/// the Renoir boot path), so the base carries only the identity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum clock_source_funcs {
    dce112_clk_src_funcs,
    dcn20_clk_src_funcs,
}

/// `struct clock_source` (inc/clock_source.h): the base object every clock
/// source embeds. Linux' `ctx` (dc_context*) is not stored — the module threads
/// `&amdgpu_device` to the register helpers instead, as `struct dc` notes; it
/// joins a clock source only when a funcs op that reads it is ported.
#[derive(Default)]
pub struct clock_source {
    pub funcs: Option<clock_source_funcs>,
    pub id: clock_source_id,
    pub dp_clk_src: bool,
}

/// `struct dce110_clk_src_regs` (dce_clock_source.h): the register offsets one
/// clock source uses. CS_COMMON_REG_LIST_DCN2_1 fills `PIXCLK_RESYNC_CNTL` (per
/// combo-PHY PLL) and `PHASE`/`MODULO`/`PIXEL_RATE_CNTL[0..3]`; `RESYNC_CNTL`,
/// `PLL_CNTL` and indices [4..5] stay 0 (the designated initializer leaves them).
#[derive(Default, Clone, Copy)]
pub struct dce110_clk_src_regs {
    pub RESYNC_CNTL: u32,
    pub PIXCLK_RESYNC_CNTL: u32,
    pub PLL_CNTL: u32,
    pub PHASE: [u32; MAX_PIPES],
    pub MODULO: [u32; MAX_PIPES],
    pub PIXEL_RATE_CNTL: [u32; MAX_PIPES],
}

/// `struct dce110_clk_src_shift` (dce_clock_source.h): CS_REG_FIELD_LIST plus the
/// DCN32 field. Renoir's CS_COMMON_MASK_SH_LIST_DCN2_0 fills only four of them;
/// the rest stay 0.
#[derive(Default, Clone, Copy)]
pub struct dce110_clk_src_shift {
    pub PLL_REF_DIV_SRC: u8,
    pub DCCG_DEEP_COLOR_CNTL1: u8,
    pub PHYPLLA_DCCG_DEEP_COLOR_CNTL: u8,
    pub PHYPLLA_PIXCLK_DOUBLE_RATE_ENABLE: u8,
    pub PLL_POST_DIV_PIXCLK: u8,
    pub PLL_REF_DIV: u8,
    pub DP_DTO0_PHASE: u8,
    pub DP_DTO0_MODULO: u8,
    pub DP_DTO0_ENABLE: u8,
    pub PIPE0_DTO_SRC_SEL: u8,
}

/// `struct dce110_clk_src_mask` (dce_clock_source.h): the same field list as
/// `dce110_clk_src_shift`, as masks.
#[derive(Default, Clone, Copy)]
pub struct dce110_clk_src_mask {
    pub PLL_REF_DIV_SRC: u32,
    pub DCCG_DEEP_COLOR_CNTL1: u32,
    pub PHYPLLA_DCCG_DEEP_COLOR_CNTL: u32,
    pub PHYPLLA_PIXCLK_DOUBLE_RATE_ENABLE: u32,
    pub PLL_POST_DIV_PIXCLK: u32,
    pub PLL_REF_DIV: u32,
    pub DP_DTO0_PHASE: u32,
    pub DP_DTO0_MODULO: u32,
    pub DP_DTO0_ENABLE: u32,
    pub PIPE0_DTO_SRC_SEL: u32,
}

/// `struct dce110_clk_src` (dce_clock_source.h): the concrete clock source. The
/// spread-spectrum tables, `calc_pll*`, `ref_freq_khz` and the stored `bios`
/// pointer belong to the `dce110_clk_src_construct` path Renoir does not take,
/// so they join when that path (or a funcs op) is ported.
#[derive(Default)]
pub struct dce110_clk_src {
    pub base: clock_source,
    pub regs: Option<&'static dce110_clk_src_regs>,
    pub cs_mask: Option<&'static dce110_clk_src_mask>,
    pub cs_shift: Option<&'static dce110_clk_src_shift>,
    pub ext_clk_khz: u32,
}

/// One `clk_src_regs[]` entry: `PIXCLK_RESYNC_CNTL` is this PLL's, the DTO/OTG
/// arrays are the same for every PLL (they index DP_DTO/OTG 0..3, not the PLL).
/// `SRII(.., 0..3)` fills [0..3]; [4..5] stay 0, matching MAX_PIPES = 6.
const fn cs_regs(pixclk_resync_cntl: u32) -> dce110_clk_src_regs {
    dce110_clk_src_regs {
        RESYNC_CNTL: 0,
        PIXCLK_RESYNC_CNTL: pixclk_resync_cntl,
        PLL_CNTL: 0,
        PHASE: [CS_DP_DTO0_PHASE, CS_DP_DTO1_PHASE, CS_DP_DTO2_PHASE, CS_DP_DTO3_PHASE, 0, 0],
        MODULO: [CS_DP_DTO0_MODULO, CS_DP_DTO1_MODULO, CS_DP_DTO2_MODULO, CS_DP_DTO3_MODULO, 0, 0],
        PIXEL_RATE_CNTL: [
            CS_OTG0_PIXEL_RATE_CNTL, CS_OTG1_PIXEL_RATE_CNTL,
            CS_OTG2_PIXEL_RATE_CNTL, CS_OTG3_PIXEL_RATE_CNTL, 0, 0,
        ],
    }
}

/// `static const struct dce110_clk_src_regs clk_src_regs[]` (dcn21_resource.c),
/// one entry per combo-PHY PLL A..E.
pub static CLK_SRC_REGS: [dce110_clk_src_regs; 5] = [
    cs_regs(CS_PHYPLLA_PIXCLK_RESYNC_CNTL),
    cs_regs(CS_PHYPLLB_PIXCLK_RESYNC_CNTL),
    cs_regs(CS_PHYPLLC_PIXCLK_RESYNC_CNTL),
    cs_regs(CS_PHYPLLD_PIXCLK_RESYNC_CNTL),
    cs_regs(CS_PHYPLLE_PIXCLK_RESYNC_CNTL),
];

/// `static const struct dce110_clk_src_shift cs_shift` (dcn21_resource.c), from
/// CS_COMMON_MASK_SH_LIST_DCN2_0(__SHIFT).
pub static CS_SHIFT: dce110_clk_src_shift = dce110_clk_src_shift {
    PLL_REF_DIV_SRC: 0,
    DCCG_DEEP_COLOR_CNTL1: 0,
    PHYPLLA_DCCG_DEEP_COLOR_CNTL: PHYPLLA_PIXCLK_RESYNC_CNTL__PHYPLLA_DCCG_DEEP_COLOR_CNTL__SHIFT as u8,
    PHYPLLA_PIXCLK_DOUBLE_RATE_ENABLE: 0,
    PLL_POST_DIV_PIXCLK: 0,
    PLL_REF_DIV: 0,
    DP_DTO0_PHASE: DP_DTO0_PHASE__DP_DTO0_PHASE__SHIFT as u8,
    DP_DTO0_MODULO: DP_DTO0_MODULO__DP_DTO0_MODULO__SHIFT as u8,
    DP_DTO0_ENABLE: OTG0_PIXEL_RATE_CNTL__DP_DTO0_ENABLE__SHIFT as u8,
    PIPE0_DTO_SRC_SEL: 0,
};

/// `static const struct dce110_clk_src_mask cs_mask` (dcn21_resource.c), from
/// CS_COMMON_MASK_SH_LIST_DCN2_0(_MASK).
pub static CS_MASK: dce110_clk_src_mask = dce110_clk_src_mask {
    PLL_REF_DIV_SRC: 0,
    DCCG_DEEP_COLOR_CNTL1: 0,
    PHYPLLA_DCCG_DEEP_COLOR_CNTL: PHYPLLA_PIXCLK_RESYNC_CNTL__PHYPLLA_DCCG_DEEP_COLOR_CNTL_MASK,
    PHYPLLA_PIXCLK_DOUBLE_RATE_ENABLE: 0,
    PLL_POST_DIV_PIXCLK: 0,
    PLL_REF_DIV: 0,
    DP_DTO0_PHASE: DP_DTO0_PHASE__DP_DTO0_PHASE_MASK,
    DP_DTO0_MODULO: DP_DTO0_MODULO__DP_DTO0_MODULO_MASK,
    DP_DTO0_ENABLE: OTG0_PIXEL_RATE_CNTL__DP_DTO0_ENABLE_MASK,
    PIPE0_DTO_SRC_SEL: 0,
};

/// `dce112_clk_src_construct` (dce_clock_source.c): the short body. Stores the
/// id, funcs, register pointers and `ext_clk_khz`; fails if the firmware info is
/// not valid (Linux' ASSERT_CRITICAL). Linux also stores `ctx` and `bios`; those
/// join with the funcs op that reads them.
pub fn dce112_clk_src_construct(
    clk_src: &mut dce110_clk_src,
    bios: &dc_bios,
    id: clock_source_id,
    regs: &'static dce110_clk_src_regs,
    cs_shift: &'static dce110_clk_src_shift,
    cs_mask: &'static dce110_clk_src_mask,
) -> bool {
    clk_src.base.id = id;
    clk_src.base.funcs = Some(clock_source_funcs::dce112_clk_src_funcs);
    clk_src.regs = Some(regs);
    clk_src.cs_shift = Some(cs_shift);
    clk_src.cs_mask = Some(cs_mask);

    if !bios.fw_info_valid {
        // ASSERT_CRITICAL(false)
        return false;
    }

    clk_src.ext_clk_khz = bios.fw_info.external_clock_source_frequency_for_dp;
    true
}

/// `dcn20_clk_src_construct` (dce_clock_source.c): dce112's body, then swap the
/// funcs table for dcn20's.
pub fn dcn20_clk_src_construct(
    clk_src: &mut dce110_clk_src,
    bios: &dc_bios,
    id: clock_source_id,
    regs: &'static dce110_clk_src_regs,
    cs_shift: &'static dce110_clk_src_shift,
    cs_mask: &'static dce110_clk_src_mask,
) -> bool {
    let ret = dce112_clk_src_construct(clk_src, bios, id, regs, cs_shift, cs_mask);
    clk_src.base.funcs = Some(clock_source_funcs::dcn20_clk_src_funcs);
    ret
}

/// `dcn21_clock_source_create` (dcn21_resource.c): allocate a `dce110_clk_src`,
/// construct it, set `dp_clk_src` on success. None on failure (Linux frees and
/// BREAK_TO_DEBUGGER()).
pub fn dcn21_clock_source_create(
    bios: &dc_bios,
    id: clock_source_id,
    regs: &'static dce110_clk_src_regs,
    dp_clk_src: bool,
) -> Option<Box<dce110_clk_src>> {
    let mut clk_src = Box::new(dce110_clk_src::default());
    if dcn20_clk_src_construct(&mut clk_src, bios, id, regs, &CS_SHIFT, &CS_MASK) {
        clk_src.base.dp_clk_src = dp_clk_src;
        Some(clk_src)
    } else {
        None
    }
}

/// `struct pixel_clk_flags` (inc/clock_source.h).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct pixel_clk_flags {
    pub ENABLE_SS: bool,
    pub DISPLAY_BLANKED: bool,
    pub PROGRAM_PIXEL_CLOCK: bool,
    pub PROGRAM_ID_CLOCK: bool,
    pub SUPPORT_YCBCR420: bool,
}

impl pixel_clk_flags {
    /// The bitfield word, in declaration order.
    pub fn raw(&self) -> u32 {
        self.ENABLE_SS as u32
            | (self.DISPLAY_BLANKED as u32) << 1
            | (self.PROGRAM_PIXEL_CLOCK as u32) << 2
            | (self.PROGRAM_ID_CLOCK as u32) << 3
            | (self.SUPPORT_YCBCR420 as u32) << 4
    }
}

/// `struct pixel_clk_params` (inc/clock_source.h), without the de-spread
/// parameters of an on-the-fly retune.
#[derive(Clone, Copy, Debug, Default)]
pub struct pixel_clk_params {
    pub requested_pix_clk_100hz: u32,
    pub requested_sym_clk: u32,
    pub dp_ref_clk: u32,
    pub encoder_object_id: crate::dc_types::graphics_object_id,
    /// `enum signal_type`.
    pub signal_type: u32,
    /// `enum controller_id`.
    pub controller_id: u32,
    /// `enum dc_color_depth`.
    pub color_depth: u32,
    /// `enum dc_pixel_encoding`.
    pub pixel_encoding: u32,
    pub flags: pixel_clk_flags,
    pub dio_se_pix_per_cycle: u32,
}

/// `struct pll_settings` (inc/clock_source.h).
#[derive(Clone, Copy, Debug, Default)]
pub struct pll_settings {
    pub actual_pix_clk_100hz: u32,
    pub adjusted_pix_clk_100hz: u32,
    pub calculated_pix_clk_100hz: u32,
    pub vco_freq: u32,
    pub reference_freq: u32,
    pub reference_divider: u32,
    pub feedback_divider: u32,
    pub fract_feedback_divider: u32,
    pub pix_clk_post_divider: u32,
    pub ss_percentage: u32,
    pub use_external_clk: bool,
}

/// `dce112_get_pix_clk_dividers_helper`.
fn dce112_get_pix_clk_dividers_helper(pll_settings: &mut pll_settings, pix_clk_params: &pixel_clk_params) {
    let mut actual_pixel_clock_100hz = pix_clk_params.requested_pix_clk_100hz;
    // Calculate Dividers
    if pix_clk_params.signal_type == LF_SIGNAL_TYPE_HDMI_TYPE_A {
        match pix_clk_params.color_depth {
            COLOR_DEPTH_101010 => {
                actual_pixel_clock_100hz = (actual_pixel_clock_100hz * 5) >> 2;
                actual_pixel_clock_100hz -= actual_pixel_clock_100hz % 10;
            }
            COLOR_DEPTH_121212 => {
                actual_pixel_clock_100hz = (actual_pixel_clock_100hz * 6) >> 2;
                actual_pixel_clock_100hz -= actual_pixel_clock_100hz % 10;
            }
            COLOR_DEPTH_161616 => {
                actual_pixel_clock_100hz *= 2;
            }
            _ => {}
        }
    }
    pll_settings.actual_pix_clk_100hz = actual_pixel_clock_100hz;
    pll_settings.adjusted_pix_clk_100hz = actual_pixel_clock_100hz;
    pll_settings.calculated_pix_clk_100hz = pix_clk_params.requested_pix_clk_100hz;
}

/// `dce112_get_pix_clk_dividers`, the `get_pix_clk_dividers` of both
/// `dce112_clk_src_funcs` and `dcn20_clk_src_funcs`.
pub fn dce112_get_pix_clk_dividers(cs: &dce110_clk_src, pix_clk_params: &pixel_clk_params,
                                   pll_settings: &mut pll_settings) -> u32 {
    if pix_clk_params.requested_pix_clk_100hz == 0 {
        crate::linux::Line::new().s("dce112_get_pix_clk_dividers: Invalid parameters!!").say();
        return u32::MAX;
    }

    *pll_settings = Default::default();

    if cs.base.id == clock_source_id::CLOCK_SOURCE_ID_DP_DTO || cs.base.id == clock_source_id::CLOCK_SOURCE_ID_EXTERNAL {
        pll_settings.adjusted_pix_clk_100hz = cs.ext_clk_khz * 10;
        pll_settings.calculated_pix_clk_100hz = cs.ext_clk_khz * 10;
        pll_settings.actual_pix_clk_100hz = pix_clk_params.requested_pix_clk_100hz;
        return u32::MAX;
    }

    dce112_get_pix_clk_dividers_helper(pll_settings, pix_clk_params);

    0
}

/// `struct clock_source_funcs` (clock_source.h), the members ported. A
/// clock source lives in the pool, so the members that reach the VBIOS
/// name it by its pipe reference.
#[derive(Default, Clone, Copy)]
pub struct clock_source_ops {
    pub cs_power_down: Option<fn(&crate::amdgpu_device::amdgpu_device, &mut crate::dc::dc,
                                 crate::dc_state::clock_source_ref) -> bool>,
    pub program_pix_clk: Option<fn(&crate::amdgpu_device::amdgpu_device, &mut crate::dc::dc,
                                   crate::dc_state::clock_source_ref, &pixel_clk_params, u32, &pll_settings) -> bool>,
    pub get_pixel_clk_frequency_100hz: Option<fn(&crate::amdgpu_device::amdgpu_device, &dce110_clk_src, u32,
                                                 &mut u32) -> bool>,
}

/// `dcn20_clk_src_funcs` (dce_clock_source.c), as far as ported.
const DCN20_CLK_SRC_FUNCS: clock_source_ops = clock_source_ops {
    cs_power_down: Some(dce110_clock_source_power_down),
    program_pix_clk: Some(dcn20_program_pix_clk),
    get_pixel_clk_frequency_100hz: Some(get_pixel_clk_frequency_100hz),
};

/// `dce112_clk_src_funcs` (dce_clock_source.c), as far as ported.
const DCE112_CLK_SRC_FUNCS: clock_source_ops = clock_source_ops {
    cs_power_down: Some(dce110_clock_source_power_down),
    program_pix_clk: Some(dce112_program_pix_clk),
    get_pixel_clk_frequency_100hz: Some(get_pixel_clk_frequency_100hz),
};

impl clock_source_funcs {
    pub fn ops(self) -> &'static clock_source_ops {
        match self {
            clock_source_funcs::dce112_clk_src_funcs => &DCE112_CLK_SRC_FUNCS,
            clock_source_funcs::dcn20_clk_src_funcs => &DCN20_CLK_SRC_FUNCS,
        }
    }
}

impl dce110_clk_src {
    /// `clock_source->funcs`.
    pub fn funcs(&self) -> &'static clock_source_ops {
        self.base.funcs.expect("clock source funcs").ops()
    }
}

/// `dce110_clock_source_power_down` (dce_clock_source.c): a pixel clock of
/// 0 powers the PLL down. The DP DTO source has no PLL.
pub fn dce110_clock_source_power_down(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut crate::dc::dc,
                                      cs: crate::dc_state::clock_source_ref) -> bool {
    use crate::command_table2::*;
    let clk_src = crate::dc_resource::clock_source_of(dc.res_pool.as_deref().unwrap(), cs);

    if clk_src.base.dp_clk_src {
        return true;
    }

    // If Pixel Clock is 0 it means Power Down Pll
    let bp_pixel_clock_params = bp_pixel_clock_parameters {
        controller_id: CONTROLLER_ID_UNDEFINED,
        pll_id: clk_src.base.id,
        flags: FORCE_PROGRAMMING_OF_PLL,
        ..Default::default()
    };

    // Call ASICControl to process ATOMBIOS Exec table
    let bp_result = bios_parser_set_pixel_clock(adev, dc, &bp_pixel_clock_params);

    bp_result == crate::bios_parser2::bp_result::BP_RESULT_OK
}

/// `get_pixel_clk_frequency_100hz` (dce_clock_source.c): the DP DTO's
/// phase is the pixel clock, the VBIOS programming its modulo to the DP
/// reference (vblank synchronization is not a member of dcn21_funcs).
fn get_pixel_clk_frequency_100hz(adev: &crate::amdgpu_device::amdgpu_device, clk_src: &dce110_clk_src, inst: u32,
                                 pixel_clk_khz: &mut u32) -> bool {
    if clk_src.base.id == clock_source_id::CLOCK_SOURCE_ID_DP_DTO {
        let clock_hz = crate::dc_helper::dm_read_reg(adev, clk_src.regs.unwrap().PHASE[inst as usize]);

        *pixel_clk_khz = clock_hz / 100;
        return true;
    }

    false
}

/// `dce112_program_pixel_clk_resync` (dce_clock_source.c): the HDMI deep
/// colour ratio of the TMDS clock (24 bit 1:1, 30 bit 5:4, 36 bit 3:2,
/// 48 bit 2:1).
fn dce112_program_pixel_clk_resync(adev: &crate::amdgpu_device::amdgpu_device, clk_src: &dce110_clk_src,
                                   signal_type: u32, colordepth: u32, enable_ycbcr420: bool) {
    let mut deep_color_cntl = 0;
    let mut double_rate_enable = 0;

    if signal_type == LF_SIGNAL_TYPE_HDMI_TYPE_A {
        double_rate_enable = enable_ycbcr420 as u32;

        deep_color_cntl = match colordepth {
            COLOR_DEPTH_888 => 0,
            COLOR_DEPTH_101010 => 1,
            COLOR_DEPTH_121212 => 2,
            COLOR_DEPTH_161616 => 3,
            _ => 0,
        };
    }

    let c = (clk_src.regs.unwrap(), clk_src.cs_shift.unwrap(), clk_src.cs_mask.unwrap());
    if c.2.PHYPLLA_PIXCLK_DOUBLE_RATE_ENABLE != 0 {
        crate::dc_helper::reg_update!(adev, c, PIXCLK_RESYNC_CNTL,
            PHYPLLA_DCCG_DEEP_COLOR_CNTL, deep_color_cntl,
            PHYPLLA_PIXCLK_DOUBLE_RATE_ENABLE, double_rate_enable);
    } else {
        crate::dc_helper::reg_update!(adev, c, PIXCLK_RESYNC_CNTL, PHYPLLA_DCCG_DEEP_COLOR_CNTL, deep_color_cntl);
    }
}

/// `dce112_program_pix_clk` (dce_clock_source.c): the pixel clock through
/// the VBIOS, adjusted by the deep colour ratio. DCN has no spread
/// spectrum to switch off first (that is DCE 11.0 and older).
fn dce112_program_pix_clk(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut crate::dc::dc,
                          cs: crate::dc_state::clock_source_ref, pix_clk_params: &pixel_clk_params, _encoding: u32,
                          pll_settings: &pll_settings) -> bool {
    use crate::command_table2::*;
    let id = crate::dc_resource::clock_source_of(dc.res_pool.as_deref().unwrap(), cs).base.id;

    // ATOMBIOS expects pixel rate adjusted by deep color ratio)
    let mut bp_pc_params = bp_pixel_clock_parameters {
        controller_id: pix_clk_params.controller_id,
        pll_id: id,
        target_pixel_clock_100hz: pll_settings.actual_pix_clk_100hz,
        encoder_object_id: pix_clk_params.encoder_object_id,
        signal_type: pix_clk_params.signal_type,
        ..Default::default()
    };

    if id != clock_source_id::CLOCK_SOURCE_ID_DP_DTO {
        if pll_settings.use_external_clk {
            bp_pc_params.flags |= SET_GENLOCK_REF_DIV_SRC;
        } else {
            bp_pc_params.flags |= SET_XTALIN_REF_SRC;
        }
        if pix_clk_params.flags.SUPPORT_YCBCR420 {
            bp_pc_params.flags |= SUPPORT_YUV_420;
        }
    }
    if bios_parser_set_pixel_clock(adev, dc, &bp_pc_params) != crate::bios_parser2::bp_result::BP_RESULT_OK {
        return false;
    }
    // Resync deep color DTO
    if id != clock_source_id::CLOCK_SOURCE_ID_DP_DTO {
        let clk_src = crate::dc_resource::clock_source_of(dc.res_pool.as_deref().unwrap(), cs);
        dce112_program_pixel_clk_resync(adev, clk_src, pix_clk_params.signal_type, pix_clk_params.color_depth,
            pix_clk_params.flags.SUPPORT_YCBCR420);
    }

    true
}

/// `dcn20_program_pix_clk` (dce_clock_source.c). Vblank synchronization,
/// which would reprogram the DTO modulo, is not a member of dcn21_funcs.
fn dcn20_program_pix_clk(adev: &crate::amdgpu_device::amdgpu_device, dc: &mut crate::dc::dc,
                         cs: crate::dc_state::clock_source_ref, pix_clk_params: &pixel_clk_params, encoding: u32,
                         pll_settings: &pll_settings) -> bool {
    dce112_program_pix_clk(adev, dc, cs, pix_clk_params, encoding, pll_settings);

    true
}
