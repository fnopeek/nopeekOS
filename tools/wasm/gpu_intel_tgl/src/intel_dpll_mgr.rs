//! display/intel_dpll_mgr.c (Linux 6.18.55), the Tiger Lake part:
//! `tgl_pll_mgr` with the two combo PLLs, the TBT PLL and six Dekel PLLs
//! for the Type-C ports. Readout, reference clocks, frequencies, dump and
//! sanitize; computing and programming a new PLL state comes with modeset.

use crate::i915_drv::drm_i915_private;
use crate::intel_bios::intel_vbt_data;
use crate::intel_de::*;
use crate::intel_display_power::{intel_display_power_get_if_enabled, intel_display_power_put};
use crate::intel_dkl_phy::*;
use crate::linux::Line;
use crate::regs::*;
use crate::tables;

/// `struct dpll_info`.
#[derive(Clone, Copy)]
pub struct dpll_info {
    pub name: &'static [u8],
    pub funcs: dpll_info_funcs,
    pub id: u32,
    pub power_domain: u32,
    pub always_on: u32,
    pub is_alt_port_dpll: u32,
}

/// The `struct intel_dpll_funcs` tables `tgl_plls` uses.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum dpll_info_funcs {
    combo_pll_funcs,
    tbt_pll_funcs,
    dkl_pll_funcs,
}

/// `struct icl_dpll_hw_state`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct icl_dpll_hw_state {
    pub cfgcr0: u32,
    pub cfgcr1: u32,
    pub div0: u32,
    pub mg_refclkin_ctl: u32,
    pub mg_clktop2_coreclkctl1: u32,
    pub mg_clktop2_hsclkctl: u32,
    pub mg_pll_div0: u32,
    pub mg_pll_div1: u32,
    pub mg_pll_lf: u32,
    pub mg_pll_frac_lock: u32,
    pub mg_pll_ssc: u32,
    pub mg_pll_bias: u32,
    pub mg_pll_tdc_coldst_bias: u32,
    pub mg_pll_bias_mask: u32,
    pub mg_pll_tdc_coldst_bias_mask: u32,
}

/// `struct intel_dpll_hw_state`: the `icl` member is the one TGL uses.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct intel_dpll_hw_state {
    pub icl: icl_dpll_hw_state,
}

/// `struct intel_dpll_state`.
#[derive(Clone, Copy, Default)]
pub struct intel_dpll_state {
    pub pipe_mask: u8,
    pub hw_state: intel_dpll_hw_state,
}

/// `struct intel_dpll`; the wakeref is whether the PLL's power domain is
/// held.
#[derive(Clone, Copy, Default)]
pub struct intel_dpll {
    pub state: intel_dpll_state,
    pub index: u8,
    pub active_mask: u8,
    pub on: bool,
    pub info: Option<&'static dpll_info>,
    pub wakeref: bool,
}

/// `struct intel_dpll_mgr`, the hooks TGL reaches.
pub struct intel_dpll_mgr {
    pub dpll_info: &'static [dpll_info],
}

static tgl_pll_mgr: intel_dpll_mgr = intel_dpll_mgr { dpll_info: &tables::tgl_plls };

/// The `dpll` member of `struct intel_display`.
#[derive(Clone, Copy, Default)]
pub struct intel_display_dpll {
    pub dplls: [intel_dpll; I915_NUM_PLLS as usize],
    pub num_dpll: usize,
    pub mgr: Option<&'static intel_dpll_mgr>,
    pub ref_clks_nssc: u32,
    pub ref_clks_ssc: u32,
}

fn info(pll: &intel_dpll) -> &'static dpll_info {
    pll.info.expect("DPLL without info")
}

pub fn dpll_name(pll: &intel_dpll) -> &'static str {
    core::str::from_utf8(info(pll).name).unwrap_or("?")
}

/// `intel_dpll_init`: display version 12, not DG1/RKL/ADL, is `tgl_pll_mgr`.
pub fn intel_dpll_init(i915: &mut drm_i915_private) {
    let dpll_mgr = &tgl_pll_mgr;
    let dpll_info = dpll_mgr.dpll_info;
    let d = &mut i915.display.dpll;

    let mut i = 0;
    while i < dpll_info.len() {
        if i >= d.dplls.len() || dpll_info[i].id >= 32 {
            crate::linux::say("warning: DPLL table larger than the DPLL array");
            break;
        }
        d.dplls[i].info = Some(&dpll_info[i]);
        d.dplls[i].index = i as u8;
        i += 1;
    }

    d.mgr = Some(dpll_mgr);
    d.num_dpll = i;
}

/// `intel_get_dpll_by_id`.
pub fn intel_get_dpll_by_id(i915: &drm_i915_private, id: u32) -> Option<usize> {
    let d = &i915.display.dpll;
    for i in 0..d.num_dpll {
        if info(&d.dplls[i]).id == id {
            return Some(i);
        }
    }
    Line::new().s("MISSING_CASE DPLL id ").dec(id as u64).say();
    None
}

/// `icl_pll_id_to_tc_port`.
fn icl_pll_id_to_tc_port(id: u32) -> u32 {
    TC_PORT_1 + id - DPLL_ID_ICL_MGPLL1
}

/// `icl_tc_port_to_pll_id`.
pub fn icl_tc_port_to_pll_id(tc_port: u32) -> u32 {
    tc_port - TC_PORT_1 + DPLL_ID_ICL_MGPLL1
}

/// `intel_combo_pll_enable_reg` (not DG1, JSL or EHL).
fn intel_combo_pll_enable_reg(pll: &intel_dpll) -> u32 {
    [ICL_DPLL_ENABLE_0, ICL_DPLL_ENABLE_1][info(pll).id as usize]
}

/// `intel_tc_pll_enable_reg` (not ADL-P).
fn intel_tc_pll_enable_reg(pll: &intel_dpll) -> u32 {
    MG_PLL_ENABLE(icl_pll_id_to_tc_port(info(pll).id))
}

fn TGL_DPLL_CFGCR0(id: u32) -> u32 {
    [TGL_DPLL_CFGCR0_0, TGL_DPLL_CFGCR0_1, TGL_DPLL_CFGCR0_2][id as usize]
}

fn TGL_DPLL_CFGCR1(id: u32) -> u32 {
    [TGL_DPLL_CFGCR1_0, TGL_DPLL_CFGCR1_1, TGL_DPLL_CFGCR1_2][id as usize]
}

/// `dkl_pll_get_hw_state`.
fn dkl_pll_get_hw_state(i915: &mut drm_i915_private, vbt: &intel_vbt_data, pll: &intel_dpll,
                        dpll_hw_state: &mut intel_dpll_hw_state) -> bool {
    let hw_state = &mut dpll_hw_state.icl;
    let tc_port = icl_pll_id_to_tc_port(info(pll).id);
    let mut ret = false;

    if !intel_display_power_get_if_enabled(i915, POWER_DOMAIN_DISPLAY_CORE) {
        return false;
    }

    let val = intel_de_read(i915, intel_tc_pll_enable_reg(pll));
    if val & PLL_ENABLE != 0 {
        // All registers read here share HIP_INDEX_REG even though they are
        // on different building blocks.
        hw_state.mg_refclkin_ctl = intel_dkl_phy_read(i915, DKL_REFCLKIN_CTL(tc_port));
        hw_state.mg_refclkin_ctl &= MG_REFCLKIN_CTL_OD_2_MUX_MASK;

        hw_state.mg_clktop2_hsclkctl = intel_dkl_phy_read(i915, DKL_CLKTOP2_HSCLKCTL(tc_port));
        hw_state.mg_clktop2_hsclkctl &= MG_CLKTOP2_HSCLKCTL_TLINEDRV_CLKSEL_MASK
            | MG_CLKTOP2_HSCLKCTL_CORE_INPUTSEL_MASK
            | MG_CLKTOP2_HSCLKCTL_HSDIV_RATIO_MASK
            | MG_CLKTOP2_HSCLKCTL_DSDIV_RATIO_MASK;

        hw_state.mg_clktop2_coreclkctl1 = intel_dkl_phy_read(i915, DKL_CLKTOP2_CORECLKCTL1(tc_port));
        hw_state.mg_clktop2_coreclkctl1 &= MG_CLKTOP2_CORECLKCTL1_A_DIVRATIO_MASK;

        hw_state.mg_pll_div0 = intel_dkl_phy_read(i915, DKL_PLL_DIV0(tc_port));
        let mut mask = DKL_PLL_DIV0_MASK;
        if vbt.override_afc_startup {
            mask |= DKL_PLL_DIV0_AFC_STARTUP_MASK;
        }
        hw_state.mg_pll_div0 &= mask;

        hw_state.mg_pll_div1 = intel_dkl_phy_read(i915, DKL_PLL_DIV1(tc_port));
        hw_state.mg_pll_div1 &= DKL_PLL_DIV1_IREF_TRIM_MASK | DKL_PLL_DIV1_TDC_TARGET_CNT_MASK;

        hw_state.mg_pll_ssc = intel_dkl_phy_read(i915, DKL_PLL_SSC(tc_port));
        hw_state.mg_pll_ssc &= DKL_PLL_SSC_IREF_NDIV_RATIO_MASK as u32
            | DKL_PLL_SSC_STEP_LEN_MASK
            | DKL_PLL_SSC_STEP_NUM_MASK
            | DKL_PLL_SSC_EN;

        hw_state.mg_pll_bias = intel_dkl_phy_read(i915, DKL_PLL_BIAS(tc_port));
        hw_state.mg_pll_bias &= DKL_PLL_BIAS_FRAC_EN_H | DKL_PLL_BIAS_FBDIV_FRAC_MASK;

        hw_state.mg_pll_tdc_coldst_bias = intel_dkl_phy_read(i915, DKL_PLL_TDC_COLDST_BIAS(tc_port));
        hw_state.mg_pll_tdc_coldst_bias &= DKL_PLL_TDC_SSC_STEP_SIZE_MASK | DKL_PLL_TDC_FEED_FWD_GAIN_MASK;

        ret = true;
    }

    intel_display_power_put(i915, POWER_DOMAIN_DISPLAY_CORE);
    ret
}

/// `icl_pll_get_hw_state` (display version 12, not ADL-S/DG1/RKL).
fn icl_pll_get_hw_state(i915: &mut drm_i915_private, vbt: &intel_vbt_data, pll: &intel_dpll,
                        dpll_hw_state: &mut intel_dpll_hw_state, enable_reg: u32) -> bool {
    let hw_state = &mut dpll_hw_state.icl;
    let id = info(pll).id;
    let mut ret = false;

    if !intel_display_power_get_if_enabled(i915, POWER_DOMAIN_DISPLAY_CORE) {
        return false;
    }

    let val = intel_de_read(i915, enable_reg);
    if val & PLL_ENABLE != 0 {
        hw_state.cfgcr0 = intel_de_read(i915, TGL_DPLL_CFGCR0(id));
        hw_state.cfgcr1 = intel_de_read(i915, TGL_DPLL_CFGCR1(id));
        if vbt.override_afc_startup {
            hw_state.div0 = intel_de_read(i915, TGL_DPLL0_DIV0(id));
            hw_state.div0 &= TGL_DPLL0_DIV0_AFC_STARTUP_MASK;
        }
        ret = true;
    }

    intel_display_power_put(i915, POWER_DOMAIN_DISPLAY_CORE);
    ret
}

/// `intel_dpll_get_hw_state`: `get_hw_state` of the PLL's funcs.
pub fn intel_dpll_get_hw_state(i915: &mut drm_i915_private, vbt: &intel_vbt_data, pll: &intel_dpll,
                               dpll_hw_state: &mut intel_dpll_hw_state) -> bool {
    match info(pll).funcs {
        // combo_pll_get_hw_state
        dpll_info_funcs::combo_pll_funcs => {
            let enable_reg = intel_combo_pll_enable_reg(pll);
            icl_pll_get_hw_state(i915, vbt, pll, dpll_hw_state, enable_reg)
        }
        // tbt_pll_get_hw_state
        dpll_info_funcs::tbt_pll_funcs => icl_pll_get_hw_state(i915, vbt, pll, dpll_hw_state, TBT_PLL_ENABLE),
        dpll_info_funcs::dkl_pll_funcs => dkl_pll_get_hw_state(i915, vbt, pll, dpll_hw_state),
    }
}

/// `icl_wrpll_ref_clock`: a 38.4 MHz reference is divided by 2 in the PLL.
fn icl_wrpll_ref_clock(i915: &drm_i915_private) -> u32 {
    let ref_clock = i915.display.dpll.ref_clks_nssc;
    if ref_clock == 38400 { 19200 } else { ref_clock }
}

/// `ehl_combo_pll_div_frac_wa_needed` (display version 12).
fn ehl_combo_pll_div_frac_wa_needed(i915: &drm_i915_private) -> bool {
    i915.display.dpll.ref_clks_nssc == 38400
}

/// `icl_ddi_combo_pll_get_freq`.
fn icl_ddi_combo_pll_get_freq(i915: &drm_i915_private, dpll_hw_state: &intel_dpll_hw_state) -> u32 {
    let hw_state = &dpll_hw_state.icl;
    let ref_clock = icl_wrpll_ref_clock(i915);

    let mut p0 = hw_state.cfgcr1 & DPLL_CFGCR1_PDIV_MASK;
    let mut p2 = hw_state.cfgcr1 & DPLL_CFGCR1_KDIV_MASK;

    let p1 = if hw_state.cfgcr1 & DPLL_CFGCR1_QDIV_MODE(1) != 0 {
        (hw_state.cfgcr1 & DPLL_CFGCR1_QDIV_RATIO_MASK) >> DPLL_CFGCR1_QDIV_RATIO_SHIFT
    } else {
        1
    };

    match p0 {
        DPLL_CFGCR1_PDIV_2 => p0 = 2,
        DPLL_CFGCR1_PDIV_3 => p0 = 3,
        DPLL_CFGCR1_PDIV_5 => p0 = 5,
        DPLL_CFGCR1_PDIV_7 => p0 = 7,
        _ => {}
    }
    match p2 {
        DPLL_CFGCR1_KDIV_1 => p2 = 1,
        DPLL_CFGCR1_KDIV_2 => p2 = 2,
        DPLL_CFGCR1_KDIV_3 => p2 = 3,
        _ => {}
    }

    let mut dco_freq = (hw_state.cfgcr0 & DPLL_CFGCR0_DCO_INTEGER_MASK) * ref_clock;

    let mut dco_fraction = (hw_state.cfgcr0 & DPLL_CFGCR0_DCO_FRACTION_MASK) >> DPLL_CFGCR0_DCO_FRACTION_SHIFT;

    if ehl_combo_pll_div_frac_wa_needed(i915) {
        dco_fraction *= 2;
    }

    dco_freq += (dco_fraction * ref_clock) / 0x8000;

    if p0 == 0 || p1 == 0 || p2 == 0 {
        crate::linux::say("warning: combo PLL with a zero divider");
        return 0;
    }

    dco_freq / (p0 * p1 * p2 * 5)
}

/// `icl_ddi_mg_pll_get_freq` (display version 12, Dekel registers).
fn icl_ddi_mg_pll_get_freq(i915: &drm_i915_private, dpll_hw_state: &intel_dpll_hw_state) -> u32 {
    let hw_state = &dpll_hw_state.icl;
    let ref_clock = i915.display.dpll.ref_clks_nssc;

    let mut m1 = hw_state.mg_pll_div0 & DKL_PLL_DIV0_FBPREDIV_MASK;
    m1 >>= DKL_PLL_DIV0_FBPREDIV_SHIFT;
    let m2_int = hw_state.mg_pll_div0 & DKL_PLL_DIV0_FBDIV_INT_MASK;

    let m2_frac = if hw_state.mg_pll_bias & DKL_PLL_BIAS_FRAC_EN_H != 0 {
        (hw_state.mg_pll_bias & DKL_PLL_BIAS_FBDIV_FRAC_MASK) >> DKL_PLL_BIAS_FBDIV_SHIFT
    } else {
        0
    };

    let div1 = match hw_state.mg_clktop2_hsclkctl & MG_CLKTOP2_HSCLKCTL_HSDIV_RATIO_MASK {
        MG_CLKTOP2_HSCLKCTL_HSDIV_RATIO_2 => 2,
        MG_CLKTOP2_HSCLKCTL_HSDIV_RATIO_3 => 3,
        MG_CLKTOP2_HSCLKCTL_HSDIV_RATIO_5 => 5,
        MG_CLKTOP2_HSCLKCTL_HSDIV_RATIO_7 => 7,
        _ => {
            Line::new().s("MISSING_CASE hsclkctl ").hex(hw_state.mg_clktop2_hsclkctl).say();
            return 0;
        }
    };

    let mut div2 = (hw_state.mg_clktop2_hsclkctl & MG_CLKTOP2_HSCLKCTL_DSDIV_RATIO_MASK)
        >> MG_CLKTOP2_HSCLKCTL_DSDIV_RATIO_SHIFT;

    // A div2 of 0 means no division, as 1 does.
    if div2 == 0 {
        div2 = 1;
    }

    // The division by 2^22 is delayed to keep the rounding error small.
    let mut tmp = m1 as u64 * m2_int as u64 * ref_clock as u64
        + ((m1 as u64 * m2_frac as u64 * ref_clock as u64) >> 22);
    tmp /= (5 * div1 * div2) as u64;

    tmp as u32
}

/// `intel_dpll_get_freq`. The TBT PLL puts out several frequencies at
/// once; the DDI clock mux picks one, so it has none of its own.
pub fn intel_dpll_get_freq(i915: &drm_i915_private, pll: &intel_dpll,
                           dpll_hw_state: &intel_dpll_hw_state) -> u32 {
    match info(pll).funcs {
        dpll_info_funcs::combo_pll_funcs => icl_ddi_combo_pll_get_freq(i915, dpll_hw_state),
        dpll_info_funcs::tbt_pll_funcs => {
            crate::linux::say("warning: icl_ddi_tbt_pll_get_freq");
            0
        }
        dpll_info_funcs::dkl_pll_funcs => icl_ddi_mg_pll_get_freq(i915, dpll_hw_state),
    }
}

/// `icl_update_dpll_ref_clks`: no SSC reference.
fn icl_update_dpll_ref_clks(i915: &mut drm_i915_private) {
    i915.display.dpll.ref_clks_nssc = i915.display.cdclk.hw.r#ref;
}

/// `intel_dpll_update_ref_clks`.
pub fn intel_dpll_update_ref_clks(i915: &mut drm_i915_private) {
    if i915.display.dpll.mgr.is_some() {
        icl_update_dpll_ref_clks(i915);
    }
}

/// `intel_dpll_crtc_get`: `pll` is used by the CRTC on `pipe`.
pub fn intel_dpll_crtc_get(pll: &intel_dpll, pipe: u32, dpll_state: &mut intel_dpll_state) {
    if dpll_state.pipe_mask & (1 << pipe) != 0 {
        crate::linux::say("warning: DPLL already reserved by this pipe");
    }
    dpll_state.pipe_mask |= 1 << pipe;
    Line::new().s("[CRTC pipe ").dec(pipe as u64).s("] reserving ").s(dpll_name(pll)).say();
}

/// `readout_dpll_hw_state`. `active_pipes` gives, for a PLL index, the
/// pipes whose active CRTC state names this PLL (the `for_each_intel_crtc`
/// loop of Linux, answered by the CRTC readout).
fn readout_dpll_hw_state(i915: &mut drm_i915_private, vbt: &intel_vbt_data, idx: usize,
                         active_pipes: &dyn Fn(usize) -> u32) {
    let mut pll = i915.display.dpll.dplls[idx];
    let mut hw_state = pll.state.hw_state;
    pll.on = intel_dpll_get_hw_state(i915, vbt, &pll, &mut hw_state);
    pll.state.hw_state = hw_state;

    if pll.on && info(&pll).power_domain != 0 {
        crate::intel_display_power::intel_display_power_get(i915, info(&pll).power_domain);
        pll.wakeref = true;
    }

    pll.state.pipe_mask = 0;
    let pipes = active_pipes(idx);
    for pipe in 0..32 {
        if pipes & (1 << pipe) != 0 {
            let mut st = pll.state;
            intel_dpll_crtc_get(&pll, pipe, &mut st);
            pll.state = st;
        }
    }
    pll.active_mask = pll.state.pipe_mask;

    Line::new().s(dpll_name(&pll)).s(" hw state readout: pipe_mask ").hex(pll.state.pipe_mask as u32)
        .s(", on ").dec(pll.on as u64).say();
    i915.display.dpll.dplls[idx] = pll;
}

/// `intel_dpll_readout_hw_state`.
pub fn intel_dpll_readout_hw_state(i915: &mut drm_i915_private, vbt: &intel_vbt_data,
                                   active_pipes: &dyn Fn(usize) -> u32) {
    for i in 0..i915.display.dpll.num_dpll {
        readout_dpll_hw_state(i915, vbt, i, active_pipes);
    }
}

/// `icl_pll_disable`. The first steps are done by intel_ddi_post_disable().
fn icl_pll_disable(i915: &mut drm_i915_private, pll: &intel_dpll, enable_reg: u32) {
    intel_de_rmw(i915, enable_reg, PLL_ENABLE, 0);

    // Timeout is actually 1us.
    if intel_de_wait_for_clear(i915, enable_reg, PLL_LOCK, 1).is_err() {
        Line::new().s("PLL ").dec(info(pll).id as u64).s(" locked").say();
    }

    intel_de_rmw(i915, enable_reg, PLL_POWER_ENABLE, 0);

    // The spec says "wait" but also that it is immediate.
    if intel_de_wait_for_clear(i915, enable_reg, PLL_POWER_STATE, 1).is_err() {
        Line::new().s("PLL ").dec(info(pll).id as u64).s(" Power not disabled").say();
    }
}

/// `disable` of the PLL's funcs: combo, TBT and Dekel PLLs all end in
/// `icl_pll_disable` with their own enable register.
fn dpll_funcs_disable(i915: &mut drm_i915_private, pll: &intel_dpll) {
    let enable_reg = match info(pll).funcs {
        dpll_info_funcs::combo_pll_funcs => intel_combo_pll_enable_reg(pll),
        dpll_info_funcs::tbt_pll_funcs => TBT_PLL_ENABLE,
        dpll_info_funcs::dkl_pll_funcs => intel_tc_pll_enable_reg(pll),
    };
    icl_pll_disable(i915, pll, enable_reg);
}

/// `_intel_disable_shared_dpll`.
fn _intel_disable_shared_dpll(i915: &mut drm_i915_private, idx: usize) {
    let pll = i915.display.dpll.dplls[idx];
    dpll_funcs_disable(i915, &pll);
    i915.display.dpll.dplls[idx].on = false;

    if info(&pll).power_domain != 0 {
        intel_display_power_put(i915, info(&pll).power_domain);
        i915.display.dpll.dplls[idx].wakeref = false;
    }
}

/// `sanitize_dpll_state` (`adlp_cmtg_clock_gating_wa` is ADL-P A0 only).
fn sanitize_dpll_state(i915: &mut drm_i915_private, idx: usize) {
    let pll = i915.display.dpll.dplls[idx];
    if !pll.on || pll.active_mask != 0 {
        return;
    }
    Line::new().s(dpll_name(&pll)).s(" enabled but not in use, disabling").say();
    _intel_disable_shared_dpll(i915, idx);
}

/// `intel_dpll_sanitize_state` (`intel_cx0_pll_power_save_wa` is display
/// version 30 only). Must run after the CRTC readout has set the active
/// masks, or every running PLL counts as unused.
pub fn intel_dpll_sanitize_state(i915: &mut drm_i915_private) {
    for i in 0..i915.display.dpll.num_dpll {
        sanitize_dpll_state(i915, i);
    }
}

/// `icl_dump_hw_state`, over three lines.
pub fn intel_dpll_dump_hw_state(dpll_hw_state: &intel_dpll_hw_state) {
    let h = &dpll_hw_state.icl;
    Line::new().s("dpll_hw_state: cfgcr0: ").hex(h.cfgcr0).s(", cfgcr1: ").hex(h.cfgcr1)
        .s(", div0: ").hex(h.div0).s(", mg_refclkin_ctl: ").hex(h.mg_refclkin_ctl).say();
    Line::new().s("  hg_clktop2_coreclkctl1: ").hex(h.mg_clktop2_coreclkctl1)
        .s(", mg_clktop2_hsclkctl: ").hex(h.mg_clktop2_hsclkctl)
        .s(", mg_pll_div0: ").hex(h.mg_pll_div0).s(", mg_pll_div2: ").hex(h.mg_pll_div1).say();
    Line::new().s("  mg_pll_lf: ").hex(h.mg_pll_lf).s(", mg_pll_frac_lock: ").hex(h.mg_pll_frac_lock)
        .s(", mg_pll_ssc: ").hex(h.mg_pll_ssc).s(", mg_pll_bias: ").hex(h.mg_pll_bias)
        .s(", mg_pll_tdc_coldst_bias: ").hex(h.mg_pll_tdc_coldst_bias).say();
}
