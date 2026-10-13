//! display/intel_cdclk.c (Linux 6.18.55) for a Tiger Lake display
//! (version 12): `intel_init_cdclk_hooks` selects `tgl_cdclk_funcs`
//! (`bxt_get_cdclk`, `bxt_set_cdclk`, `tgl_calc_voltage_level`) and
//! `icl_cdclk_table`. TGL has neither the CDCLK crawl nor the squasher, so
//! the paths behind HAS_CDCLK_CRAWL/HAS_CDCLK_SQUASH are not taken.
//!
//! `display->cdclk` lives in `intel_display_cdclk`, passed explicitly.

use crate::i915_drv::drm_i915_private;
use crate::intel_pcode;
use crate::linux::{self, Line};
use crate::regs::*;
use crate::tables;

const _: () = assert!(TGL_DISPLAY_HAS_CDCLK_CRAWL == 0 && TGL_DISPLAY_HAS_CDCLK_SQUASH == 0);

/// `struct intel_cdclk_vals`.
#[derive(Clone, Copy)]
pub struct intel_cdclk_vals {
    pub cdclk: u32,
    pub refclk: u16,
    pub waveform: u16,
    pub ratio: u8,
}

/// `struct intel_cdclk_config`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct intel_cdclk_config {
    pub cdclk: u32,
    pub vco: u32,
    pub r#ref: u32,
    pub bypass: u32,
    pub voltage_level: u8,
    pub joined_mbus: bool,
}

/// `struct intel_cdclk_state`, the global state object's payload.
#[derive(Clone, Copy, Default)]
pub struct intel_cdclk_state {
    pub logical: intel_cdclk_config,
    pub actual: intel_cdclk_config,
    pub bw_min_cdclk: i32,
    pub min_cdclk: [i32; I915_MAX_PIPES as usize],
    pub min_voltage_level: [u8; I915_MAX_PIPES as usize],
    pub pipe: i32,
    pub force_min_cdclk: i32,
    pub active_pipes: u8,
    pub disable_pipes: bool,
}

/// The `cdclk` member of `struct intel_display`.
#[derive(Clone, Copy, Default)]
pub struct intel_display_cdclk {
    pub hw: intel_cdclk_config,
    pub table: &'static [intel_cdclk_vals],
    pub obj: intel_cdclk_state,
    pub max_cdclk_freq: u32,
    pub max_dotclk_freq: u32,
    pub skl_preferred_vco_freq: u32,
}

/// `intel_init_cdclk_hooks`, DISPLAY_VER 12 (not DG2, ADL-P or RKL).
pub fn intel_init_cdclk_hooks() -> intel_display_cdclk {
    intel_display_cdclk {
        hw: intel_cdclk_config::default(),
        table: &tables::icl_cdclk_table,
        obj: intel_cdclk_state::default(),
        max_cdclk_freq: 0,
        max_dotclk_freq: 0,
        skl_preferred_vco_freq: 0,
    }
}

/// `intel_cdclk_init`: the zeroed global state.
pub fn intel_cdclk_init(cd: &mut intel_display_cdclk) -> Result<(), i32> {
    cd.obj = intel_cdclk_state::default();
    Ok(())
}

fn div_round_closest(x: u32, d: u32) -> u32 {
    (x + d / 2) / d
}

fn div_round_closest_i(x: i32, d: i32) -> i32 {
    if (x > 0) == (d > 0) { (x + d / 2) / d } else { (x - d / 2) / d }
}

fn intel_de_wait_for_set(i915: &mut drm_i915_private, reg: u32, mask: u32, timeout_ms: u32) -> Result<(), ()> {
    i915.uncore.__intel_wait_for_register(reg, mask, mask, 2, timeout_ms, None)
}

fn intel_de_wait_for_clear(i915: &mut drm_i915_private, reg: u32, mask: u32, timeout_ms: u32) -> Result<(), ()> {
    i915.uncore.__intel_wait_for_register(reg, mask, 0, 2, timeout_ms, None)
}

const cdclk_squash_len: i32 = 16;

/// `cdclk_squash_divider`.
fn cdclk_squash_divider(waveform: u16) -> i32 {
    (if waveform != 0 { waveform } else { 0xffff }).count_ones() as i32
}

/// `cdclk_divider`: 2 * cd2x divider.
fn cdclk_divider(cdclk: i32, vco: i32, waveform: u16) -> i32 {
    div_round_closest_i(vco * cdclk_squash_divider(waveform), cdclk * cdclk_squash_len)
}

/// `skl_cdclk_decimal`.
fn skl_cdclk_decimal(cdclk: i32) -> u32 {
    div_round_closest_i(cdclk - 1000, 500) as u32
}

/// `bxt_calc_cdclk`.
fn bxt_calc_cdclk(cd: &intel_display_cdclk, min_cdclk: u32) -> u32 {
    for t in cd.table.iter().take_while(|t| t.refclk != 0) {
        if t.refclk as u32 == cd.hw.r#ref && t.cdclk >= min_cdclk {
            return t.cdclk;
        }
    }
    Line::new().s("warning: Cannot satisfy minimum cdclk ").dec(min_cdclk as u64)
        .s(" with refclk ").dec(cd.hw.r#ref as u64).say();
    0
}

/// `bxt_calc_cdclk_pll_vco`.
fn bxt_calc_cdclk_pll_vco(cd: &intel_display_cdclk, cdclk: u32) -> u32 {
    if cdclk == cd.hw.bypass {
        return 0;
    }
    for t in cd.table.iter().take_while(|t| t.refclk != 0) {
        if t.refclk as u32 == cd.hw.r#ref && t.cdclk == cdclk {
            return cd.hw.r#ref * t.ratio as u32;
        }
    }
    Line::new().s("warning: cdclk ").dec(cdclk as u64).s(" not valid for refclk ")
        .dec(cd.hw.r#ref as u64).say();
    0
}

/// `calc_voltage_level`.
fn calc_voltage_level(cdclk: i32, voltage_level_max_cdclk: &[i32]) -> u8 {
    for (voltage_level, max) in voltage_level_max_cdclk.iter().enumerate() {
        if cdclk <= *max {
            return voltage_level as u8;
        }
    }
    Line::new().s("warning: missing case cdclk ").dec(cdclk as u64).say();
    (voltage_level_max_cdclk.len() - 1) as u8
}

/// `tgl_calc_voltage_level`, the `calc_voltage_level` hook.
fn tgl_calc_voltage_level(cdclk: i32) -> u8 {
    calc_voltage_level(cdclk, &tables::tgl_voltage_level_max_cdclk)
}

/// `intel_cdclk_calc_voltage_level`.
fn intel_cdclk_calc_voltage_level(cdclk: u32) -> u8 {
    tgl_calc_voltage_level(cdclk as i32)
}

/// `icl_readout_refclk`.
fn icl_readout_refclk(i915: &mut drm_i915_private, cdclk_config: &mut intel_cdclk_config) {
    let dssm = i915.uncore.intel_uncore_read(SKL_DSSM) & ICL_DSSM_CDCLK_PLL_REFCLK_MASK as u32;
    cdclk_config.r#ref = match dssm {
        ICL_DSSM_CDCLK_PLL_REFCLK_24MHz => 24000,
        ICL_DSSM_CDCLK_PLL_REFCLK_19_2MHz => 19200,
        ICL_DSSM_CDCLK_PLL_REFCLK_38_4MHz => 38400,
        other => {
            Line::new().s("warning: missing case dssm ").hex(other).say();
            24000
        }
    };
}

/// `bxt_de_pll_readout`.
fn bxt_de_pll_readout(i915: &mut drm_i915_private, cdclk_config: &mut intel_cdclk_config) {
    icl_readout_refclk(i915, cdclk_config);

    let val = i915.uncore.intel_uncore_read(BXT_DE_PLL_ENABLE);
    if val & BXT_DE_PLL_PLL_ENABLE as u32 == 0 || val & BXT_DE_PLL_LOCK == 0 {
        // A disabled PLL is signalled as VCO 0.
        cdclk_config.vco = 0;
        return;
    }
    let ratio = val & ICL_CDCLK_PLL_RATIO_MASK;
    cdclk_config.vco = ratio * cdclk_config.r#ref;
}

/// `bxt_get_cdclk`, the `get_cdclk` hook.
fn bxt_get_cdclk(i915: &mut drm_i915_private, cdclk_config: &mut intel_cdclk_config) {
    bxt_de_pll_readout(i915, cdclk_config);

    cdclk_config.bypass = cdclk_config.r#ref / 2;

    if cdclk_config.vco == 0 {
        cdclk_config.cdclk = cdclk_config.bypass;
    } else {
        let divider = i915.uncore.intel_uncore_read(CDCLK_CTL) & BXT_CDCLK_CD2X_DIV_SEL_MASK;
        let div: u32 = match divider {
            BXT_CDCLK_CD2X_DIV_SEL_1 => 2,
            BXT_CDCLK_CD2X_DIV_SEL_1_5 => 3,
            BXT_CDCLK_CD2X_DIV_SEL_2 => 4,
            BXT_CDCLK_CD2X_DIV_SEL_4 => 8,
            other => {
                Line::new().s("warning: missing case cdclk divider ").hex(other).say();
                return;
            }
        };
        cdclk_config.cdclk = div_round_closest(cdclk_config.vco, div);
    }

    // The voltage level cannot be read out; assume what the CDCLK requires.
    cdclk_config.voltage_level = intel_cdclk_calc_voltage_level(cdclk_config.cdclk);
}

/// `intel_cdclk_get_cdclk`.
pub fn intel_cdclk_get_cdclk(i915: &mut drm_i915_private, cdclk_config: &mut intel_cdclk_config) {
    bxt_get_cdclk(i915, cdclk_config);
}

/// `icl_cdclk_pll_disable`.
fn icl_cdclk_pll_disable(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk) {
    i915.uncore.intel_uncore_rmw(BXT_DE_PLL_ENABLE, BXT_DE_PLL_PLL_ENABLE as u32, 0);

    // Timeout 200us
    if intel_de_wait_for_clear(i915, BXT_DE_PLL_ENABLE, BXT_DE_PLL_LOCK, 1).is_err() {
        linux::drm_err("timeout waiting for CDCLK PLL unlock");
    }
    cd.hw.vco = 0;
}

/// `icl_cdclk_pll_enable`.
fn icl_cdclk_pll_enable(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk, vco: u32) {
    let ratio = div_round_closest(vco, cd.hw.r#ref);

    let mut val = ICL_CDCLK_PLL_RATIO(ratio);
    i915.uncore.intel_uncore_write(BXT_DE_PLL_ENABLE, val);

    val |= BXT_DE_PLL_PLL_ENABLE as u32;
    i915.uncore.intel_uncore_write(BXT_DE_PLL_ENABLE, val);

    // Timeout 200us
    if intel_de_wait_for_set(i915, BXT_DE_PLL_ENABLE, BXT_DE_PLL_LOCK, 1).is_err() {
        linux::drm_err("timeout waiting for CDCLK PLL lock");
    }
    cd.hw.vco = vco;
}

/// `bxt_cdclk_cd2x_pipe`.
fn bxt_cdclk_cd2x_pipe(pipe: i32) -> u32 {
    if pipe == INVALID_PIPE {
        TGL_CDCLK_CD2X_PIPE_NONE
    } else {
        TGL_CDCLK_CD2X_PIPE(pipe as u32)
    }
}

/// `bxt_cdclk_cd2x_div_sel`: cdclk = vco / 2 / div{1,1.5,2,4}.
fn bxt_cdclk_cd2x_div_sel(cd: &intel_display_cdclk, cdclk: u32, vco: u32, waveform: u16) -> u32 {
    match cdclk_divider(cdclk as i32, vco as i32, waveform) {
        2 => BXT_CDCLK_CD2X_DIV_SEL_1,
        3 => BXT_CDCLK_CD2X_DIV_SEL_1_5,
        4 => BXT_CDCLK_CD2X_DIV_SEL_2,
        8 => BXT_CDCLK_CD2X_DIV_SEL_4,
        _ => {
            if cdclk != cd.hw.bypass {
                Line::new().s("warning: cdclk ").dec(cdclk as u64).s(" is not the bypass").say();
            }
            if vco != 0 {
                Line::new().s("warning: cd2x divider for VCO ").dec(vco as u64).say();
            }
            BXT_CDCLK_CD2X_DIV_SEL_1
        }
    }
}

/// `cdclk_squash_waveform`.
fn cdclk_squash_waveform(cd: &intel_display_cdclk, cdclk: u32) -> u16 {
    if cdclk == cd.hw.bypass {
        return 0;
    }
    for t in cd.table.iter().take_while(|t| t.refclk != 0) {
        if t.refclk as u32 == cd.hw.r#ref && t.cdclk == cdclk {
            return t.waveform;
        }
    }
    Line::new().s("warning: cdclk ").dec(cdclk as u64).s(" not valid for refclk ")
        .dec(cd.hw.r#ref as u64).say();
    0xffff
}

/// `icl_cdclk_pll_update`.
fn icl_cdclk_pll_update(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk, vco: u32) {
    if cd.hw.vco != 0 && cd.hw.vco != vco {
        icl_cdclk_pll_disable(i915, cd);
    }
    if cd.hw.vco != vco {
        icl_cdclk_pll_enable(i915, cd, vco);
    }
}

/// `bxt_cdclk_ctl`.
fn bxt_cdclk_ctl(cd: &intel_display_cdclk, cdclk_config: &intel_cdclk_config, pipe: i32) -> u32 {
    let cdclk = cdclk_config.cdclk;
    let vco = cdclk_config.vco;
    let waveform = cdclk_squash_waveform(cd, cdclk);

    bxt_cdclk_cd2x_div_sel(cd, cdclk, vco, waveform)
        | bxt_cdclk_cd2x_pipe(pipe)
        | skl_cdclk_decimal(cdclk as i32)
}

/// `_bxt_set_cdclk`. `pll_enable_wa_needed` is DG2/MTL/Xe2 only.
fn _bxt_set_cdclk(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk,
                  cdclk_config: &intel_cdclk_config, pipe: i32) {
    icl_cdclk_pll_update(i915, cd, cdclk_config.vco);

    let ctl = bxt_cdclk_ctl(cd, cdclk_config, pipe);
    i915.uncore.intel_uncore_write(CDCLK_CTL, ctl);

    if pipe != INVALID_PIPE {
        Line::new().s("warning: vblank wait on pipe ").dec(pipe as u64).s(" not ported").say();
    }
}

/// `bxt_set_cdclk`, the `set_cdclk` hook. Without crawl and squash there
/// is no midpoint (`cdclk_compute_crawl_and_squash_midpoint` is false).
fn bxt_set_cdclk(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk,
                 cdclk_config: &intel_cdclk_config, pipe: i32) {
    let cdclk = cdclk_config.cdclk;

    // Inform the power controller of the upcoming frequency change.
    if let Err(ret) = intel_pcode::skl_pcode_request(i915, SKL_PCODE_CDCLK_CONTROL,
                                                     SKL_CDCLK_PREPARE_FOR_CHANGE,
                                                     SKL_CDCLK_READY_FOR_CHANGE,
                                                     SKL_CDCLK_READY_FOR_CHANGE, 3) {
        Line::new().s("error: Failed to inform PCU about cdclk change (err ").dec((-ret) as u64)
            .s(", freq ").dec(cdclk as u64).s(")").say();
        return;
    }

    _bxt_set_cdclk(i915, cd, cdclk_config, pipe);

    if let Err(ret) = intel_pcode::intel_pcode_write(i915, SKL_PCODE_CDCLK_CONTROL,
                                                     cdclk_config.voltage_level as u32) {
        Line::new().s("error: PCode CDCLK freq set failed, (err ").dec((-ret) as u64)
            .s(", freq ").dec(cdclk as u64).s(")").say();
        return;
    }

    intel_update_cdclk(i915, cd);

    // The voltage level cannot be read out; assume it is as requested.
    cd.hw.voltage_level = cdclk_config.voltage_level;
}

/// `bxt_sanitize_cdclk`.
fn bxt_sanitize_cdclk(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk) {
    intel_update_cdclk(i915, cd);
    intel_cdclk_dump_config(&cd.hw, "Current CDCLK");

    if !(cd.hw.vco == 0 || cd.hw.cdclk == cd.hw.bypass) {
        // A legal cdclk for the platform, with the VCO it needs.
        let cdclk = bxt_calc_cdclk(cd, cd.hw.cdclk);
        if cdclk == cd.hw.cdclk && bxt_calc_cdclk_pll_vco(cd, cdclk) == cd.hw.vco {
            // Some BIOSes leave a wrong decimal frequency or set MBZ bits in
            // CDCLK_CTL. The pipe field is ignored: the BIOS may sync the
            // dividers to an active pipe or to none.
            let mut cdctl = i915.uncore.intel_uncore_read(CDCLK_CTL);
            let mut expected = bxt_cdclk_ctl(cd, &cd.hw, INVALID_PIPE);
            cdctl &= !bxt_cdclk_cd2x_pipe(INVALID_PIPE);
            expected &= !bxt_cdclk_cd2x_pipe(INVALID_PIPE);
            if cdctl == expected {
                return;
            }
        }
    }

    Line::new().s("Sanitizing cdclk programmed by pre-os").say();
    // Force cdclk programming and a full PLL disable + enable.
    cd.hw.cdclk = 0;
    cd.hw.vco = !0;
}

/// `bxt_cdclk_init_hw`.
fn bxt_cdclk_init_hw(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk) {
    bxt_sanitize_cdclk(i915, cd);

    if cd.hw.cdclk != 0 && cd.hw.vco != 0 {
        return;
    }

    let mut cdclk_config = cd.hw;
    cdclk_config.cdclk = bxt_calc_cdclk(cd, 0);
    cdclk_config.vco = bxt_calc_cdclk_pll_vco(cd, cdclk_config.cdclk);
    cdclk_config.voltage_level = intel_cdclk_calc_voltage_level(cdclk_config.cdclk);

    bxt_set_cdclk(i915, cd, &cdclk_config, INVALID_PIPE);
}

/// `intel_cdclk_init_hw`: reads `cd.hw` and reprograms CDCLK only when the
/// pre-OS setup is not a legal one.
pub fn intel_cdclk_init_hw(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk) {
    bxt_cdclk_init_hw(i915, cd);
}

/// `intel_cdclk_clock_changed`.
pub fn intel_cdclk_clock_changed(a: &intel_cdclk_config, b: &intel_cdclk_config) -> bool {
    a.cdclk != b.cdclk || a.vco != b.vco || a.r#ref != b.r#ref
}

/// `intel_cdclk_dump_config`.
pub fn intel_cdclk_dump_config(cdclk_config: &intel_cdclk_config, context: &str) {
    Line::new().s(context).s(" ").dec(cdclk_config.cdclk as u64).s(" kHz, VCO ")
        .dec(cdclk_config.vco as u64).s(" kHz, ref ").dec(cdclk_config.r#ref as u64)
        .s(" kHz, bypass ").dec(cdclk_config.bypass as u64).s(" kHz, voltage level ")
        .dec(cdclk_config.voltage_level as u64).say();
}

/// `intel_cdclk_read_hw`.
pub fn intel_cdclk_read_hw(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk) {
    intel_update_cdclk(i915, cd);
    intel_cdclk_dump_config(&cd.hw, "Current CDCLK");
    cd.obj.actual = cd.hw;
    cd.obj.logical = cd.hw;
}

/// `intel_update_cdclk`.
pub fn intel_update_cdclk(i915: &mut drm_i915_private, cd: &mut intel_display_cdclk) {
    let mut hw = cd.hw;
    intel_cdclk_get_cdclk(i915, &mut hw);
    cd.hw = hw;
}

/// `intel_compute_max_dotclk`: two pixels per clock (DISPLAY_VER >= 10),
/// guardband 100 %.
fn intel_compute_max_dotclk(cd: &intel_display_cdclk) -> u32 {
    let ppc = 2;
    let guardband = 100;
    ppc * cd.max_cdclk_freq * guardband / 100
}

/// `intel_update_max_cdclk`.
pub fn intel_update_max_cdclk(cd: &mut intel_display_cdclk) {
    cd.max_cdclk_freq = if cd.hw.r#ref == 24000 { 648000 } else { 652800 };
    cd.max_dotclk_freq = intel_compute_max_dotclk(cd);

    Line::new().s("Max CD clock rate: ").dec(cd.max_cdclk_freq as u64).s(" kHz").say();
    Line::new().s("Max dotclock rate: ").dec(cd.max_dotclk_freq as u64).s(" kHz").say();
}
