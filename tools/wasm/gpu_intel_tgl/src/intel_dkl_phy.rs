//! display/intel_dkl_phy.c and intel_dkl_phy_regs.h (Linux 6.18.55): the
//! Dekel PHY of a Tiger Lake Type-C port, reached through a 4 KB window
//! per PHY whose bank is chosen in HIP_INDEX_REG.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::regs::*;

/// `struct intel_dkl_phy_reg`: `reg:24`, `bank_idx:4`.
#[derive(Clone, Copy)]
pub struct intel_dkl_phy_reg {
    pub reg: u32,
    pub bank_idx: u32,
}

/// `_DKL_REG`.
const fn _DKL_REG(tc_port: u32, phy_offset: u32) -> intel_dkl_phy_reg {
    intel_dkl_phy_reg {
        reg: (_DKL_REG_PHY_BASE(tc_port) + (phy_offset & ((1 << _DKL_BANK_SHIFT) - 1))) & 0xff_ffff,
        bank_idx: (phy_offset >> _DKL_BANK_SHIFT) & 0xf,
    }
}

pub const fn DKL_REFCLKIN_CTL(tc_port: u32) -> intel_dkl_phy_reg { _DKL_REG(tc_port, _DKL_REFCLKIN_CTL) }
pub const fn DKL_CLKTOP2_HSCLKCTL(tc_port: u32) -> intel_dkl_phy_reg { _DKL_REG(tc_port, _DKL_CLKTOP2_HSCLKCTL) }
pub const fn DKL_CLKTOP2_CORECLKCTL1(tc_port: u32) -> intel_dkl_phy_reg { _DKL_REG(tc_port, _DKL_CLKTOP2_CORECLKCTL1) }
pub const fn DKL_PLL_DIV0(tc_port: u32) -> intel_dkl_phy_reg { _DKL_REG(tc_port, _DKL_PLL_DIV0) }
pub const fn DKL_PLL_DIV1(tc_port: u32) -> intel_dkl_phy_reg { _DKL_REG(tc_port, _DKL_PLL_DIV1) }
pub const fn DKL_PLL_SSC(tc_port: u32) -> intel_dkl_phy_reg { _DKL_REG(tc_port, _DKL_PLL_SSC) }
pub const fn DKL_PLL_BIAS(tc_port: u32) -> intel_dkl_phy_reg { _DKL_REG(tc_port, _DKL_PLL_BIAS) }
pub const fn DKL_PLL_TDC_COLDST_BIAS(tc_port: u32) -> intel_dkl_phy_reg { _DKL_REG(tc_port, _DKL_PLL_TDC_COLDST_BIAS) }

// `_DKL_REG` against GCC's values of the macros at the first and last port.
const fn same(r: intel_dkl_phy_reg, reg: u32, bank: u32) -> bool {
    r.reg == reg && r.bank_idx == bank
}
const _: () = assert!(
    same(DKL_REFCLKIN_CTL(0), DKL_REFCLKIN_CTL_TC0_REG, DKL_REFCLKIN_CTL_TC0_BANK)
        && same(DKL_REFCLKIN_CTL(5), DKL_REFCLKIN_CTL_TC5_REG, DKL_REFCLKIN_CTL_TC5_BANK)
        && same(DKL_CLKTOP2_HSCLKCTL(0), DKL_CLKTOP2_HSCLKCTL_TC0_REG, DKL_CLKTOP2_HSCLKCTL_TC0_BANK)
        && same(DKL_CLKTOP2_HSCLKCTL(5), DKL_CLKTOP2_HSCLKCTL_TC5_REG, DKL_CLKTOP2_HSCLKCTL_TC5_BANK)
        && same(DKL_CLKTOP2_CORECLKCTL1(0), DKL_CLKTOP2_CORECLKCTL1_TC0_REG, DKL_CLKTOP2_CORECLKCTL1_TC0_BANK)
        && same(DKL_CLKTOP2_CORECLKCTL1(5), DKL_CLKTOP2_CORECLKCTL1_TC5_REG, DKL_CLKTOP2_CORECLKCTL1_TC5_BANK)
        && same(DKL_PLL_DIV0(0), DKL_PLL_DIV0_TC0_REG, DKL_PLL_DIV0_TC0_BANK)
        && same(DKL_PLL_DIV0(5), DKL_PLL_DIV0_TC5_REG, DKL_PLL_DIV0_TC5_BANK)
        && same(DKL_PLL_DIV1(0), DKL_PLL_DIV1_TC0_REG, DKL_PLL_DIV1_TC0_BANK)
        && same(DKL_PLL_DIV1(5), DKL_PLL_DIV1_TC5_REG, DKL_PLL_DIV1_TC5_BANK)
        && same(DKL_PLL_SSC(0), DKL_PLL_SSC_TC0_REG, DKL_PLL_SSC_TC0_BANK)
        && same(DKL_PLL_SSC(5), DKL_PLL_SSC_TC5_REG, DKL_PLL_SSC_TC5_BANK)
        && same(DKL_PLL_BIAS(0), DKL_PLL_BIAS_TC0_REG, DKL_PLL_BIAS_TC0_BANK)
        && same(DKL_PLL_BIAS(5), DKL_PLL_BIAS_TC5_REG, DKL_PLL_BIAS_TC5_BANK)
        && same(DKL_PLL_TDC_COLDST_BIAS(0), DKL_PLL_TDC_COLDST_BIAS_TC0_REG, DKL_PLL_TDC_COLDST_BIAS_TC0_BANK)
        && same(DKL_PLL_TDC_COLDST_BIAS(5), DKL_PLL_TDC_COLDST_BIAS_TC5_REG, DKL_PLL_TDC_COLDST_BIAS_TC5_BANK)
);

/// `DKL_REG_TC_PORT`.
fn DKL_REG_TC_PORT(reg: intel_dkl_phy_reg) -> u32 {
    TC_PORT_1 + (reg.reg - _DKL_PHY1_BASE) / (_DKL_PHY2_BASE - _DKL_PHY1_BASE)
}

/// `HIP_INDEX_REG`.
fn HIP_INDEX_REG(tc_port: u32) -> u32 {
    [HIP_INDEX_REG_0, HIP_INDEX_REG_1, HIP_INDEX_REG_2, HIP_INDEX_REG_3, HIP_INDEX_REG_4, HIP_INDEX_REG_5]
        [tc_port as usize]
}

/// `dkl_phy_set_hip_idx`.
fn dkl_phy_set_hip_idx(i915: &mut drm_i915_private, reg: intel_dkl_phy_reg) {
    let tc_port = DKL_REG_TC_PORT(reg);
    if tc_port < TC_PORT_1 || tc_port >= I915_MAX_TC_PORTS {
        crate::linux::say("warning: DKL register outside the Type-C ports");
        return;
    }
    intel_de_write(i915, HIP_INDEX_REG(tc_port), HIP_INDEX_VAL(tc_port, reg.bank_idx));
}

/// `intel_dkl_phy_read`. The lock is the module's single thread.
pub fn intel_dkl_phy_read(i915: &mut drm_i915_private, reg: intel_dkl_phy_reg) -> u32 {
    dkl_phy_set_hip_idx(i915, reg);
    intel_de_read(i915, reg.reg)
}

/// `intel_dkl_phy_write`.
pub fn intel_dkl_phy_write(i915: &mut drm_i915_private, reg: intel_dkl_phy_reg, val: u32) {
    dkl_phy_set_hip_idx(i915, reg);
    intel_de_write(i915, reg.reg, val);
}

/// `intel_dkl_phy_rmw`.
pub fn intel_dkl_phy_rmw(i915: &mut drm_i915_private, reg: intel_dkl_phy_reg, clear: u32, set: u32) {
    dkl_phy_set_hip_idx(i915, reg);
    intel_de_rmw(i915, reg.reg, clear, set);
}
