//! `drivers/gpu/drm/vc4/vc4_hdmi_phy.c` (Linux rpi-6.18), the BCM2712 PHY
//! (`vc6_*`): power-down, and init for a TMDS rate.

use crate::hw::Hw;
use crate::linux::field_prep;
use drm_core::drm_connector::drm_connector_state;
use crate::regs::*;
use crate::vc4_hdmi::Vc4Hdmi;

/// `vc6_hdmi_reset_phy`.
fn vc6_hdmi_reset_phy(h: &Vc4Hdmi, hw: &mut impl Hw) {
    h.write(hw, &HDMI_TX_PHY_RESET_CTL, 0);
    h.write(hw, &HDMI_TX_PHY_POWERUP_CTL, 0);
    h.write(hw, &HDMI_TX_PHY_PLL_POST_KDIV, VC6_HDMI_TX_PHY_PLL_POST_KDIV_BYPASS_EN);
}

/// `vc6_hdmi_phy_disable`.
pub fn vc6_hdmi_phy_disable(h: &Vc4Hdmi, hw: &mut impl Hw) {
    vc6_hdmi_reset_phy(h, hw);
}

/// `VC6_VCO_MIN_FREQ` / `VC6_VCO_MAX_FREQ` (unsigned long long).
const VC6_VCO_MIN_FREQ: u64 = VC6_VCO_MIN_FREQ_KHZ as u64 * 1000;
const VC6_VCO_MAX_FREQ: u64 = VC6_VCO_MAX_FREQ_KHZ as u64 * 1000;

/// `vc6_phy_get_vco_freq`: the VCO rate and its divider for a TMDS rate.
fn vc6_phy_get_vco_freq(tmds_rate: u64) -> (u64, u32) {
    let mut div: u32 = 0;
    while tmds_rate * div as u64 * 10 < VC6_VCO_MIN_FREQ {
        div += 1;
    }
    let min_div = div;

    while tmds_rate * (div as u64 + 1) * 10 < VC6_VCO_MAX_FREQ {
        div += 1;
    }
    let max_div = div;

    let div = min_div + (max_div - min_div) / 2;

    (tmds_rate * div as u64 * 10, div)
}

/// `phy_get_rm_offset`: the RM offset in 9.22 format.
fn phy_get_rm_offset(vco_freq: u64) -> u32 {
    let fref = OSCILLATOR_FREQUENCY as u64;

    let mut offset = vco_freq * 2;
    offset <<= 22;
    offset /= fref;
    offset >>= 2;

    offset as u32
}

/// `vc6_phy_get_settings`: the index into `vc6_hdmi_phy_settings`. If the
/// pixel clock exceeds the max setting, the max setting anyway.
fn vc6_phy_get_settings(tmds_rate: u64) -> usize {
    let count = VC6_PHY_SETTINGS as usize;

    for i in 0..count {
        if tmds_rate >= VC6_PHY_MIN_RATE[i] as u64 && tmds_rate <= VC6_PHY_MAX_RATE[i] as u64 {
            return i;
        }
    }

    count - 1
}

/// `vc6_phy_get_channel_settings` written as `HDMI_TX_PHY_CTL_*`: lane
/// `chan` (`PHY_LANE_CK` the clock) of the settings for `tmds_rate`.
fn vc6_phy_channel_ctl(chan: u32, tmds_rate: u64) -> u32 {
    let lane = if chan == PHY_LANE_CK { 3 } else { chan as usize };
    let i = vc6_phy_get_settings(tmds_rate) * 4 + lane;

    field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_EXT_CURRENT_CTL_MASK, VC6_PHY_EXT_CURRENT_CTL[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_FFE_ENABLE_MASK, VC6_PHY_FFE_ENABLE[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_SLEW_RATE_CTL_MASK, VC6_PHY_SLEW_RATE_CTL[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_FFE_POST_TAP_EN_MASK, VC6_PHY_FFE_POST_TAP_EN[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_LDMOS_BIAS_CTL_MASK, VC6_PHY_LDMOS_BIAS_CTL[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_COM_MODE_LDMOS_EN_MASK, VC6_PHY_COM_MODE_LDMOS_EN[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_EDGE_SEL_MASK, VC6_PHY_EDGE_SEL[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_EXT_CURRENT_SRC_HS_EN_MASK, VC6_PHY_EXT_CURRENT_SRC_HS_EN[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_TERM_CTL_MASK, VC6_PHY_TERM_CTL[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_EXT_CURRENT_SRC_EN_MASK, VC6_PHY_EXT_CURRENT_SRC_EN[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_INT_CURRENT_SRC_EN_MASK, VC6_PHY_INT_CURRENT_SRC_EN[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_INT_CURRENT_CTL_MASK, VC6_PHY_INT_CURRENT_CTL[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_INT_CURRENT_SRC_HS_EN_MASK, VC6_PHY_INT_CURRENT_SRC_HS_EN[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_MAIN_TAP_CURRENT_SELECT_MASK, VC6_PHY_MAIN_TAP_CURRENT_SELECT[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_POST_TAP_CURRENT_SELECT_MASK, VC6_PHY_POST_TAP_CURRENT_SELECT[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_SLEW_CTL_SLOW_LOADING_MASK, VC6_PHY_SLEW_CTL_SLOW_LOADING[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_SLEW_CTL_SLOW_DRIVING_MASK, VC6_PHY_SLEW_CTL_SLOW_DRIVING[i])
        | field_prep(VC6_HDMI_TX_PHY_HDMI_CTRL_CHX_FFE_PRE_TAP_EN_MASK, VC6_PHY_FFE_PRE_TAP_EN[i])
}

/// The BCM2712 variants' `phy_lane_mapping`: lane n on PHY channel n.
const PHY_LANE_MAPPING: [u32; 4] = [0, 1, 2, PHY_LANE_CK];

/// `vc6_hdmi_phy_init` for the connector state's TMDS rate.
pub fn vc6_hdmi_phy_init(h: &Vc4Hdmi, hw: &mut impl Hw, conn_state: &drm_connector_state) {
    let pixel_freq = conn_state.hdmi.tmds_char_rate;

    let (vco_freq, vco_div) = vc6_phy_get_vco_freq(pixel_freq);

    vc6_hdmi_reset_phy(h, hw);

    h.write(hw, &HDMI_TX_PHY_PLL_MISC_0, 0x810c6000);
    h.write(hw, &HDMI_TX_PHY_PLL_MISC_1, 0x00b8c451);
    h.write(hw, &HDMI_TX_PHY_PLL_MISC_2, 0x46402e31);
    h.write(hw, &HDMI_TX_PHY_PLL_MISC_3, 0x00b8c005);
    h.write(hw, &HDMI_TX_PHY_PLL_MISC_4, 0x42410261);
    h.write(hw, &HDMI_TX_PHY_PLL_MISC_5, 0xcc021001);
    h.write(hw, &HDMI_TX_PHY_PLL_MISC_6, 0xc8301c80);
    h.write(hw, &HDMI_TX_PHY_PLL_MISC_7, 0xb0804444);
    h.write(hw, &HDMI_TX_PHY_PLL_MISC_8, 0xf80f8000);

    h.write(hw, &HDMI_TX_PHY_PLL_REFCLK,
        VC6_HDMI_TX_PHY_PLL_REFCLK_REFCLK_SEL_CMOS | field_prep(VC6_HDMI_TX_PHY_PLL_REFCLK_REFFRQ_MASK, 54));

    h.write(hw, &HDMI_TX_PHY_RESET_CTL, 0x7f);

    h.write(hw, &HDMI_RM_OFFSET,
        VC4_HDMI_RM_OFFSET_ONLY | field_prep(VC4_HDMI_RM_OFFSET_OFFSET_MASK, phy_get_rm_offset(vco_freq)));

    h.write(hw, &HDMI_TX_PHY_PLL_VCOCLK_DIV,
        VC6_HDMI_TX_PHY_PLL_VCOCLK_DIV_VCODIV_EN | field_prep(VC6_HDMI_TX_PHY_PLL_VCOCLK_DIV_VCODIV_MASK, vco_div));

    h.write(hw, &HDMI_TX_PHY_PLL_CFG, field_prep(VC4_HDMI_TX_PHY_PLL_CFG_PDIV_MASK, 0));

    h.write(hw, &HDMI_TX_PHY_PLL_POST_KDIV,
        field_prep(VC6_HDMI_TX_PHY_PLL_POST_KDIV_CLK0_SEL_MASK, 2) | field_prep(VC6_HDMI_TX_PHY_PLL_POST_KDIV_KDIV_MASK, 1));

    h.write(hw, &HDMI_TX_PHY_CTL_0, vc6_phy_channel_ctl(PHY_LANE_MAPPING[0], pixel_freq));
    h.write(hw, &HDMI_TX_PHY_CTL_1, vc6_phy_channel_ctl(PHY_LANE_MAPPING[1], pixel_freq));
    h.write(hw, &HDMI_TX_PHY_CTL_2, vc6_phy_channel_ctl(PHY_LANE_MAPPING[2], pixel_freq));
    h.write(hw, &HDMI_TX_PHY_CTL_CK, vc6_phy_channel_ctl(PHY_LANE_MAPPING[3], pixel_freq));

    let word_sel = if pixel_freq >= 340_000_000 { 3 } else { 0 };
    h.write(hw, &HDMI_TX_PHY_TMDS_CLK_WORD_SEL, word_sel);

    h.write(hw, &HDMI_TX_PHY_POWERUP_CTL,
        VC6_HDMI_TX_PHY_HDMI_POWERUP_CTL_BG_PWRUP
            | VC6_HDMI_TX_PHY_HDMI_POWERUP_CTL_LDO_PWRUP
            | VC6_HDMI_TX_PHY_HDMI_POWERUP_CTL_BIAS_PWRUP
            | VC6_HDMI_TX_PHY_HDMI_POWERUP_CTL_TX_CK_PWRUP
            | VC6_HDMI_TX_PHY_HDMI_POWERUP_CTL_TX_2_PWRUP
            | VC6_HDMI_TX_PHY_HDMI_POWERUP_CTL_TX_1_PWRUP
            | VC6_HDMI_TX_PHY_HDMI_POWERUP_CTL_TX_0_PWRUP);

    h.write(hw, &HDMI_TX_PHY_PLL_POWERUP_CTL, VC6_HDMI_TX_PHY_PLL_POWERUP_CTL_PLL_PWRUP);

    let v = h.read(hw, &HDMI_TX_PHY_PLL_RESET_CTL);
    h.write(hw, &HDMI_TX_PHY_PLL_RESET_CTL, v & !VC6_HDMI_TX_PHY_PLL_RESET_CTL_PLL_RESETB);

    let v = h.read(hw, &HDMI_TX_PHY_PLL_RESET_CTL);
    h.write(hw, &HDMI_TX_PHY_PLL_RESET_CTL, v | VC6_HDMI_TX_PHY_PLL_RESET_CTL_PLL_RESETB);
}
