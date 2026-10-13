//! display/intel_combo_phy.c (Linux 6.18.55) for Tiger Lake: combo PHYs A
//! and B (`intel_phy_is_combo`, display version 12), both with PHY_MISC.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::linux::Line;
use crate::regs::*;
use crate::tables::icl_procmon_values;

/// `struct icl_procmon`.
#[derive(Clone, Copy)]
pub struct icl_procmon {
    pub name: &'static [u8],
    pub dw1: u32,
    pub dw9: u32,
    pub dw10: u32,
}

/// `intel_phy_is_combo` (display version 12, not RKL/DG1/ADL).
fn intel_phy_is_combo(phy: u32) -> bool {
    phy <= PHY_B
}

fn phy_name(phy: u32) -> &'static str {
    ["A", "B", "C", "D", "E", "F", "G", "H", "I"][phy as usize]
}

/// `icl_get_procmon_ref_values`.
fn icl_get_procmon_ref_values(i915: &mut drm_i915_private, phy: u32) -> &'static icl_procmon {
    let val = intel_de_read(i915, ICL_PORT_COMP_DW3(phy));
    let v = val & (PROCESS_INFO_MASK | VOLTAGE_INFO_MASK);
    let idx = if v == VOLTAGE_INFO_0_85V | PROCESS_INFO_DOT_0 {
        PROCMON_0_85V_DOT_0
    } else if v == VOLTAGE_INFO_0_95V | PROCESS_INFO_DOT_0 {
        PROCMON_0_95V_DOT_0
    } else if v == VOLTAGE_INFO_0_95V | PROCESS_INFO_DOT_1 {
        PROCMON_0_95V_DOT_1
    } else if v == VOLTAGE_INFO_1_05V | PROCESS_INFO_DOT_0 {
        PROCMON_1_05V_DOT_0
    } else if v == VOLTAGE_INFO_1_05V | PROCESS_INFO_DOT_1 {
        PROCMON_1_05V_DOT_1
    } else {
        Line::new().s("MISSING_CASE ").hex(val).say();
        PROCMON_0_85V_DOT_0
    };
    &icl_procmon_values[idx as usize]
}

/// `icl_set_procmon_ref_values`.
fn icl_set_procmon_ref_values(i915: &mut drm_i915_private, phy: u32) {
    let procmon = icl_get_procmon_ref_values(i915, phy);
    intel_de_rmw(i915, ICL_PORT_COMP_DW1(phy), (0xff << 16) | 0xff, procmon.dw1);
    intel_de_write(i915, ICL_PORT_COMP_DW9(phy), procmon.dw9);
    intel_de_write(i915, ICL_PORT_COMP_DW10(phy), procmon.dw10);
}

/// `check_phy_reg`.
fn check_phy_reg(i915: &mut drm_i915_private, phy: u32, reg: u32, mask: u32, expected_val: u32) -> bool {
    let val = intel_de_read(i915, reg);
    if val & mask != expected_val {
        Line::new().s("Combo PHY ").s(phy_name(phy)).s(" reg ").hex(reg).s(" state mismatch: current ")
            .hex(val).s(" mask ").hex(mask).s(" expected ").hex(expected_val).say();
        return false;
    }
    true
}

/// `icl_verify_procmon_ref_values`.
fn icl_verify_procmon_ref_values(i915: &mut drm_i915_private, phy: u32) -> bool {
    let procmon = icl_get_procmon_ref_values(i915, phy);
    let mut ret = check_phy_reg(i915, phy, ICL_PORT_COMP_DW1(phy), (0xff << 16) | 0xff, procmon.dw1);
    ret &= check_phy_reg(i915, phy, ICL_PORT_COMP_DW9(phy), !0u32, procmon.dw9);
    ret &= check_phy_reg(i915, phy, ICL_PORT_COMP_DW10(phy), !0u32, procmon.dw10);
    ret
}

/// `has_phy_misc`: every combo PHY on Tiger Lake.
fn has_phy_misc(_phy: u32) -> bool {
    true
}

/// `icl_combo_phy_enabled`.
fn icl_combo_phy_enabled(i915: &mut drm_i915_private, phy: u32) -> bool {
    if !has_phy_misc(phy) {
        return intel_de_read(i915, ICL_PORT_COMP_DW0(phy)) & COMP_INIT != 0;
    }
    intel_de_read(i915, ICL_PHY_MISC(phy)) & ICL_PHY_MISC_DE_IO_COMP_PWR_DOWN == 0
        && intel_de_read(i915, ICL_PORT_COMP_DW0(phy)) & COMP_INIT != 0
}

/// `phy_is_master`: on ICL and TGL, A drives the compensation of B and C.
fn phy_is_master(phy: u32) -> bool {
    phy == PHY_A
}

/// `icl_combo_phy_verify_state` (display version 12, not JSL/EHL).
fn icl_combo_phy_verify_state(i915: &mut drm_i915_private, phy: u32) -> bool {
    if !icl_combo_phy_enabled(i915, phy) {
        return false;
    }
    let mut ret = check_phy_reg(i915, phy, ICL_PORT_TX_DW8_LN(0, phy),
                                ICL_PORT_TX_DW8_ODCC_CLK_SEL | ICL_PORT_TX_DW8_ODCC_CLK_DIV_SEL_MASK,
                                ICL_PORT_TX_DW8_ODCC_CLK_SEL | ICL_PORT_TX_DW8_ODCC_CLK_DIV_SEL_DIV2);
    ret &= check_phy_reg(i915, phy, ICL_PORT_PCS_DW1_LN(0, phy), DCC_MODE_SELECT_MASK, RUN_DCC_ONCE);

    ret &= icl_verify_procmon_ref_values(i915, phy);

    if phy_is_master(phy) {
        ret &= check_phy_reg(i915, phy, ICL_PORT_COMP_DW8(phy), IREFGEN, IREFGEN);
    }

    ret &= check_phy_reg(i915, phy, ICL_PORT_CL_DW5(phy), CL_POWER_DOWN_ENABLE, CL_POWER_DOWN_ENABLE);
    ret
}

/// `icl_combo_phys_init`.
fn icl_combo_phys_init(i915: &mut drm_i915_private) {
    for phy in PHY_A..I915_MAX_PHYS {
        if !intel_phy_is_combo(phy) {
            continue;
        }
        if icl_combo_phy_verify_state(i915, phy) {
            continue;
        }

        let procmon = icl_get_procmon_ref_values(i915, phy);
        Line::new().s("Initializing combo PHY ").s(phy_name(phy)).s(" (Voltage/Process Info : ")
            .s(core::str::from_utf8(procmon.name).unwrap_or("?")).s(")").say();

        if has_phy_misc(phy) {
            let mut val = intel_de_read(i915, ICL_PHY_MISC(phy));
            val &= !ICL_PHY_MISC_DE_IO_COMP_PWR_DOWN;
            intel_de_write(i915, ICL_PHY_MISC(phy), val);
        }

        let mut val = intel_de_read(i915, ICL_PORT_TX_DW8_LN(0, phy));
        val &= !ICL_PORT_TX_DW8_ODCC_CLK_DIV_SEL_MASK;
        val |= ICL_PORT_TX_DW8_ODCC_CLK_SEL;
        val |= ICL_PORT_TX_DW8_ODCC_CLK_DIV_SEL_DIV2;
        intel_de_write(i915, ICL_PORT_TX_DW8_GRP(phy), val);

        let mut val = intel_de_read(i915, ICL_PORT_PCS_DW1_LN(0, phy));
        val &= !DCC_MODE_SELECT_MASK;
        val |= RUN_DCC_ONCE;
        intel_de_write(i915, ICL_PORT_PCS_DW1_GRP(phy), val);

        icl_set_procmon_ref_values(i915, phy);

        if phy_is_master(phy) {
            intel_de_rmw(i915, ICL_PORT_COMP_DW8(phy), 0, IREFGEN);
        }

        intel_de_rmw(i915, ICL_PORT_COMP_DW0(phy), 0, COMP_INIT);
        intel_de_rmw(i915, ICL_PORT_CL_DW5(phy), 0, CL_POWER_DOWN_ENABLE);
    }
}

/// `intel_combo_phy_init`.
pub fn intel_combo_phy_init(i915: &mut drm_i915_private) {
    icl_combo_phys_init(i915);
}
