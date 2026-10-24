//! display/dc/inc/hw/dccg.h + dccg/dcn20/dcn20_dccg.h + dccg/dcn21/dcn21_dccg.c
//! (Linux 6.18.55): the display clock generator (DCCG) of Renoir's resource
//! pool. `dccg21_create` is a pointer-only constructor like the clock source —
//! it stores `dccg_regs`/`dccg_shift`/`dccg_mask` and the funcs table. The
//! ops are ported as the hw sequencer reaches them.
//!
//! Renoir takes `DCCG_COMMON_REG_LIST_DCN_BASE` for the registers and
//! `DCCG_MASK_SH_LIST_DCN2_1` for the fields. The register struct and the
//! shift/mask field lists carry only the reached members (the DCN3+ clock trees
//! — HDMI/PHY/DTB/DSC/SYMCLK — stay out until a block reaches them).
#![allow(dead_code)]

use crate::amdgpu_device::amdgpu_device;
use crate::dc_helper::reg_get_n;
use crate::dc_types::MAX_PIPES;
use crate::regs::*;
use alloc::boxed::Box;

/// Which `dccg_funcs` vtable `dccg.funcs` points at (dcn21_dccg.c).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum dccg_funcs {
    dccg21_funcs,
}

/// `struct dccg_funcs` (dccg.h), the members ported.
#[derive(Default, Clone, Copy)]
pub struct dccg_ops {
    pub update_dpp_dto: Option<fn(&amdgpu_device, &mut dcn_dccg, usize, i32)>,
    pub get_dccg_ref_freq: Option<fn(&amdgpu_device, &dcn_dccg, u32, &mut u32)>,
    pub dccg_init: Option<fn(&dcn_dccg)>,
}

/// `dccg21_funcs` (dcn21_dccg.c), as far as ported.
const DCCG21_FUNCS: dccg_ops = dccg_ops {
    update_dpp_dto: Some(dccg21_update_dpp_dto),
    get_dccg_ref_freq: Some(dccg2_get_dccg_ref_freq),
    dccg_init: Some(dccg2_init),
};

impl dccg_funcs {
    pub fn ops(self) -> &'static dccg_ops {
        match self {
            dccg_funcs::dccg21_funcs => &DCCG21_FUNCS,
        }
    }
}

impl dcn_dccg {
    /// `dccg->funcs`.
    pub fn funcs(&self) -> &'static dccg_ops {
        self.base.funcs.expect("dccg funcs").ops()
    }

    fn rsm(&self) -> (&'static dccg_registers, &'static dccg_shift, &'static dccg_mask) {
        (self.regs.unwrap(), self.dccg_shift.unwrap(), self.dccg_mask.unwrap())
    }
}

/// `dccg2_get_dccg_ref_freq` (dcn20_dccg.c): DCN 2.0 was never validated
/// with a reference other than the crystal (Linux asserts on one), so the
/// answer is always the crystal.
pub fn dccg2_get_dccg_ref_freq(adev: &amdgpu_device, dccg_dcn: &dcn_dccg, xtalin_freq_inKhz: u32,
                               dccg_ref_freq_inKhz: &mut u32) {
    let [_clk_en, _clk_sel] = reg_get_n!(adev, dccg_dcn.rsm(), REFCLK_CNTL, REFCLK_CLOCK_EN, REFCLK_SRC_SEL);
    *dccg_ref_freq_inKhz = xtalin_freq_inKhz;
}

/// `dccg2_init` (dcn20_dccg.c): empty on DCN 2.0/2.1.
pub fn dccg2_init(_dccg: &dcn_dccg) {}

/// `struct dccg` (inc/hw/dccg.h): the base object. Linux' `ctx` is not stored
/// (see `clock_source`). The clock-rate state (`ref_dppclk`, …) is 0 at
/// construct and set by the funcs ops.
#[derive(Default)]
pub struct dccg {
    pub funcs: Option<dccg_funcs>,
    pub pipe_dppclk_khz: [i32; MAX_PIPES],
    pub ref_dppclk: i32,
    pub dpp_clock_gated: [bool; MAX_PIPES],
}

/// `struct dccg_registers` (DCCG_REG_VARIABLE_LIST, dcn20_dccg.h): ported
/// grow-as-reached — only the members `DCCG_COMMON_REG_LIST_DCN_BASE` fills for
/// Renoir. The field order follows the C list.
#[derive(Default, Clone, Copy)]
pub struct dccg_registers {
    pub DPPCLK_DTO_CTRL: u32,
    pub DPPCLK_DTO_PARAM: [u32; 6],
    pub REFCLK_CNTL: u32,
    pub DISPCLK_FREQ_CHANGE_CNTL: u32,
    pub OTG_PIXEL_RATE_CNTL: [u32; MAX_PIPES],
}

/// `struct dccg_shift` (DCCG_REG_FIELD_LIST, dcn20_dccg.h), the shift half.
/// Renoir's `DCCG_MASK_SH_LIST_DCN2_1` fills every field here except
/// `DISPCLK_FREQ_CHANGE_CNTL` and `OTG_*_PIXEL[4..5]`, which stay 0.
#[derive(Default, Clone, Copy)]
pub struct dccg_shift {
    pub DPPCLK0_DTO_PHASE: u8,
    pub DPPCLK0_DTO_MODULO: u8,
    pub DPPCLK_DTO_ENABLE: [u8; 6],
    pub DPPCLK_DTO_DB_EN: [u8; 6],
    pub REFCLK_CLOCK_EN: u8,
    pub REFCLK_SRC_SEL: u8,
    pub DISPCLK_STEP_DELAY: u8,
    pub DISPCLK_STEP_SIZE: u8,
    pub DISPCLK_FREQ_RAMP_DONE: u8,
    pub DISPCLK_MAX_ERRDET_CYCLES: u8,
    pub DCCG_FIFO_ERRDET_RESET: u8,
    pub DCCG_FIFO_ERRDET_STATE: u8,
    pub DCCG_FIFO_ERRDET_OVR_EN: u8,
    pub DISPCLK_CHG_FWD_CORR_DISABLE: u8,
    pub DISPCLK_FREQ_CHANGE_CNTL: u8,
    pub OTG_ADD_PIXEL: [u8; MAX_PIPES],
    pub OTG_DROP_PIXEL: [u8; MAX_PIPES],
}

/// `struct dccg_mask` (DCCG_REG_FIELD_LIST, dcn20_dccg.h), the mask half.
#[derive(Default, Clone, Copy)]
pub struct dccg_mask {
    pub DPPCLK0_DTO_PHASE: u32,
    pub DPPCLK0_DTO_MODULO: u32,
    pub DPPCLK_DTO_ENABLE: [u32; 6],
    pub DPPCLK_DTO_DB_EN: [u32; 6],
    pub REFCLK_CLOCK_EN: u32,
    pub REFCLK_SRC_SEL: u32,
    pub DISPCLK_STEP_DELAY: u32,
    pub DISPCLK_STEP_SIZE: u32,
    pub DISPCLK_FREQ_RAMP_DONE: u32,
    pub DISPCLK_MAX_ERRDET_CYCLES: u32,
    pub DCCG_FIFO_ERRDET_RESET: u32,
    pub DCCG_FIFO_ERRDET_STATE: u32,
    pub DCCG_FIFO_ERRDET_OVR_EN: u32,
    pub DISPCLK_CHG_FWD_CORR_DISABLE: u32,
    pub DISPCLK_FREQ_CHANGE_CNTL: u32,
    pub OTG_ADD_PIXEL: [u32; MAX_PIPES],
    pub OTG_DROP_PIXEL: [u32; MAX_PIPES],
}

/// `struct dcn_dccg` (dcn20_dccg.h): the concrete DCCG.
#[derive(Default)]
pub struct dcn_dccg {
    pub base: dccg,
    pub regs: Option<&'static dccg_registers>,
    pub dccg_shift: Option<&'static dccg_shift>,
    pub dccg_mask: Option<&'static dccg_mask>,
}

/// `static const struct dccg_registers dccg_regs` (dcn21_resource.c), from
/// `DCCG_COMMON_REG_LIST_DCN_BASE()`: DPPCLK_DTO_PARAM[0..3] and
/// OTG_PIXEL_RATE_CNTL[0..1] are filled, the rest stay 0.
pub static DCCG_REGS: dccg_registers = dccg_registers {
    DPPCLK_DTO_CTRL: DCCG_DPPCLK_DTO_CTRL,
    DPPCLK_DTO_PARAM: [
        DCCG_DPPCLK0_DTO_PARAM, DCCG_DPPCLK1_DTO_PARAM,
        DCCG_DPPCLK2_DTO_PARAM, DCCG_DPPCLK3_DTO_PARAM, 0, 0,
    ],
    REFCLK_CNTL: DCCG_REFCLK_CNTL,
    DISPCLK_FREQ_CHANGE_CNTL: DCCG_DISPCLK_FREQ_CHANGE_CNTL,
    OTG_PIXEL_RATE_CNTL: [
        DCCG_OTG0_PIXEL_RATE_CNTL, DCCG_OTG1_PIXEL_RATE_CNTL, 0, 0, 0, 0,
    ],
};

/// `static const struct dccg_shift dccg_shift` (dcn21_resource.c),
/// `DCCG_MASK_SH_LIST_DCN2_1(__SHIFT)`.
pub static DCCG_SHIFT: dccg_shift = dccg_shift {
    DPPCLK0_DTO_PHASE: DPPCLK0_DTO_PARAM__DPPCLK0_DTO_PHASE__SHIFT as u8,
    DPPCLK0_DTO_MODULO: DPPCLK0_DTO_PARAM__DPPCLK0_DTO_MODULO__SHIFT as u8,
    DPPCLK_DTO_ENABLE: [
        DPPCLK_DTO_CTRL__DPPCLK0_DTO_ENABLE__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK1_DTO_ENABLE__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK2_DTO_ENABLE__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK3_DTO_ENABLE__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK4_DTO_ENABLE__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK5_DTO_ENABLE__SHIFT as u8,
    ],
    DPPCLK_DTO_DB_EN: [
        DPPCLK_DTO_CTRL__DPPCLK0_DTO_DB_EN__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK1_DTO_DB_EN__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK2_DTO_DB_EN__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK3_DTO_DB_EN__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK4_DTO_DB_EN__SHIFT as u8,
        DPPCLK_DTO_CTRL__DPPCLK5_DTO_DB_EN__SHIFT as u8,
    ],
    REFCLK_CLOCK_EN: REFCLK_CNTL__REFCLK_CLOCK_EN__SHIFT as u8,
    REFCLK_SRC_SEL: REFCLK_CNTL__REFCLK_SRC_SEL__SHIFT as u8,
    DISPCLK_STEP_DELAY: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_STEP_DELAY__SHIFT as u8,
    DISPCLK_STEP_SIZE: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_STEP_SIZE__SHIFT as u8,
    DISPCLK_FREQ_RAMP_DONE: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_FREQ_RAMP_DONE__SHIFT as u8,
    DISPCLK_MAX_ERRDET_CYCLES: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_MAX_ERRDET_CYCLES__SHIFT as u8,
    DCCG_FIFO_ERRDET_RESET: DISPCLK_FREQ_CHANGE_CNTL__DCCG_FIFO_ERRDET_RESET__SHIFT as u8,
    DCCG_FIFO_ERRDET_STATE: DISPCLK_FREQ_CHANGE_CNTL__DCCG_FIFO_ERRDET_STATE__SHIFT as u8,
    DCCG_FIFO_ERRDET_OVR_EN: DISPCLK_FREQ_CHANGE_CNTL__DCCG_FIFO_ERRDET_OVR_EN__SHIFT as u8,
    DISPCLK_CHG_FWD_CORR_DISABLE: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_CHG_FWD_CORR_DISABLE__SHIFT as u8,
    DISPCLK_FREQ_CHANGE_CNTL: 0,
    OTG_ADD_PIXEL: [
        OTG0_PIXEL_RATE_CNTL__OTG0_ADD_PIXEL__SHIFT as u8,
        OTG1_PIXEL_RATE_CNTL__OTG1_ADD_PIXEL__SHIFT as u8,
        OTG2_PIXEL_RATE_CNTL__OTG2_ADD_PIXEL__SHIFT as u8,
        OTG3_PIXEL_RATE_CNTL__OTG3_ADD_PIXEL__SHIFT as u8,
        0, 0,
    ],
    OTG_DROP_PIXEL: [
        OTG0_PIXEL_RATE_CNTL__OTG0_DROP_PIXEL__SHIFT as u8,
        OTG1_PIXEL_RATE_CNTL__OTG1_DROP_PIXEL__SHIFT as u8,
        OTG2_PIXEL_RATE_CNTL__OTG2_DROP_PIXEL__SHIFT as u8,
        OTG3_PIXEL_RATE_CNTL__OTG3_DROP_PIXEL__SHIFT as u8,
        0, 0,
    ],
};

/// `static const struct dccg_mask dccg_mask` (dcn21_resource.c),
/// `DCCG_MASK_SH_LIST_DCN2_1(_MASK)`.
pub static DCCG_MASK: dccg_mask = dccg_mask {
    DPPCLK0_DTO_PHASE: DPPCLK0_DTO_PARAM__DPPCLK0_DTO_PHASE_MASK,
    DPPCLK0_DTO_MODULO: DPPCLK0_DTO_PARAM__DPPCLK0_DTO_MODULO_MASK,
    DPPCLK_DTO_ENABLE: [
        DPPCLK_DTO_CTRL__DPPCLK0_DTO_ENABLE_MASK,
        DPPCLK_DTO_CTRL__DPPCLK1_DTO_ENABLE_MASK,
        DPPCLK_DTO_CTRL__DPPCLK2_DTO_ENABLE_MASK,
        DPPCLK_DTO_CTRL__DPPCLK3_DTO_ENABLE_MASK,
        DPPCLK_DTO_CTRL__DPPCLK4_DTO_ENABLE_MASK,
        DPPCLK_DTO_CTRL__DPPCLK5_DTO_ENABLE_MASK,
    ],
    DPPCLK_DTO_DB_EN: [
        DPPCLK_DTO_CTRL__DPPCLK0_DTO_DB_EN_MASK,
        DPPCLK_DTO_CTRL__DPPCLK1_DTO_DB_EN_MASK,
        DPPCLK_DTO_CTRL__DPPCLK2_DTO_DB_EN_MASK,
        DPPCLK_DTO_CTRL__DPPCLK3_DTO_DB_EN_MASK,
        DPPCLK_DTO_CTRL__DPPCLK4_DTO_DB_EN_MASK,
        DPPCLK_DTO_CTRL__DPPCLK5_DTO_DB_EN_MASK,
    ],
    REFCLK_CLOCK_EN: REFCLK_CNTL__REFCLK_CLOCK_EN_MASK,
    REFCLK_SRC_SEL: REFCLK_CNTL__REFCLK_SRC_SEL_MASK,
    DISPCLK_STEP_DELAY: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_STEP_DELAY_MASK,
    DISPCLK_STEP_SIZE: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_STEP_SIZE_MASK,
    DISPCLK_FREQ_RAMP_DONE: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_FREQ_RAMP_DONE_MASK,
    DISPCLK_MAX_ERRDET_CYCLES: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_MAX_ERRDET_CYCLES_MASK,
    DCCG_FIFO_ERRDET_RESET: DISPCLK_FREQ_CHANGE_CNTL__DCCG_FIFO_ERRDET_RESET_MASK,
    DCCG_FIFO_ERRDET_STATE: DISPCLK_FREQ_CHANGE_CNTL__DCCG_FIFO_ERRDET_STATE_MASK,
    DCCG_FIFO_ERRDET_OVR_EN: DISPCLK_FREQ_CHANGE_CNTL__DCCG_FIFO_ERRDET_OVR_EN_MASK,
    DISPCLK_CHG_FWD_CORR_DISABLE: DISPCLK_FREQ_CHANGE_CNTL__DISPCLK_CHG_FWD_CORR_DISABLE_MASK,
    DISPCLK_FREQ_CHANGE_CNTL: 0,
    OTG_ADD_PIXEL: [
        OTG0_PIXEL_RATE_CNTL__OTG0_ADD_PIXEL_MASK,
        OTG1_PIXEL_RATE_CNTL__OTG1_ADD_PIXEL_MASK,
        OTG2_PIXEL_RATE_CNTL__OTG2_ADD_PIXEL_MASK,
        OTG3_PIXEL_RATE_CNTL__OTG3_ADD_PIXEL_MASK,
        0, 0,
    ],
    OTG_DROP_PIXEL: [
        OTG0_PIXEL_RATE_CNTL__OTG0_DROP_PIXEL_MASK,
        OTG1_PIXEL_RATE_CNTL__OTG1_DROP_PIXEL_MASK,
        OTG2_PIXEL_RATE_CNTL__OTG2_DROP_PIXEL_MASK,
        OTG3_PIXEL_RATE_CNTL__OTG3_DROP_PIXEL_MASK,
        0, 0,
    ],
};

/// `dccg21_create` (dcn21_dccg.c): allocate a `dcn_dccg`, store the funcs table
/// and the register pointers. Never fails here (Linux returns NULL only on a
/// failed kzalloc).
pub fn dccg21_create(
    regs: &'static dccg_registers,
    dccg_shift: &'static dccg_shift,
    dccg_mask: &'static dccg_mask,
) -> Box<dcn_dccg> {
    let mut d = Box::new(dcn_dccg::default());
    d.base.funcs = Some(dccg_funcs::dccg21_funcs);
    d.regs = Some(regs);
    d.dccg_shift = Some(dccg_shift);
    d.dccg_mask = Some(dccg_mask);
    d
}

/// `dccg21_update_dpp_dto` (dcn21_dccg.c): the per-pipe DPP clock as a DTO
/// of the global one. The phase is ceil(pipe clock / 10 MHz), the modulo
/// trunc(global clock / 10 MHz), so the divided clock is never too slow; an
/// unused pipe gets phase 10, which keeps its registers accessible.
fn dccg21_update_dpp_dto(adev: &amdgpu_device, dccg: &mut dcn_dccg, dpp_inst: usize, req_dppclk: i32) {
    if dccg.base.ref_dppclk != 0 {
        let ref_dppclk = dccg.base.ref_dppclk;
        let modulo = ref_dppclk / 10000;

        let phase = if req_dppclk != 0 {
            // phase > modulo result in screen corruption ie phase = 30,
            // mod = 29 for 4k@60 HDMI in these case we don't want pipe
            // clock to be divided
            ((req_dppclk + 9999) / 10000).min(modulo)
        } else {
            10
        };

        let (r, sh, m) = dccg.rsm();
        crate::dc_helper::generic_reg_set_ex(adev, r.DPPCLK_DTO_PARAM[dpp_inst], 0,
            &[(sh.DPPCLK0_DTO_PHASE, m.DPPCLK0_DTO_PHASE, phase as u32),
              (sh.DPPCLK0_DTO_MODULO, m.DPPCLK0_DTO_MODULO, modulo as u32)]);

        crate::dc_helper::generic_reg_update_ex(adev, r.DPPCLK_DTO_CTRL,
            &[(sh.DPPCLK_DTO_ENABLE[dpp_inst], m.DPPCLK_DTO_ENABLE[dpp_inst], 1)]);
    }

    dccg.base.pipe_dppclk_khz[dpp_inst] = req_dppclk;
}
