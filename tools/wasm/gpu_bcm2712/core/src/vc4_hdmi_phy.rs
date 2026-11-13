//! `drivers/gpu/drm/vc4/vc4_hdmi_phy.c` (Linux rpi-6.18), the BCM2712 PHY
//! (`vc6_*`).

use crate::hw::Hw;
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
