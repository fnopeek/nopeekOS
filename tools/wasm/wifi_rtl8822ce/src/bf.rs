//! `bf.c` aus Linux 6.18.26 rtw88 — nur `rtw_bf_phy_init`.
//!
//! Der Rest von bf.c (Beamformer/Beamformee anmelden, CSI-Raten, die
//! Gruppentabelle) haengt an einer VERBINDUNG und gehoert zu der Stufe, die
//! eine aufbaut. Hier steht die Grundeinstellung, die `phy_set_param` zum
//! Schluss setzt.
#![allow(dead_code)]

use crate::host;
use crate::regs::*;

/// bf.c:344-377 `rtw_bf_phy_init`
pub fn phy_init(h: i32) {
    let retry_limit: u32 = 0xA;
    let ndpa_rate: u8 = 0x10;
    let ack_policy: u8 = 3;

    let mut tmp32 = host::r32(h, REG_MU_TX_CTL);
    // Enable P1 aggr new packet according to P0 transfer time
    tmp32 |= BIT_MU_P1_WAIT_STATE_EN;
    // MU Retry Limit
    tmp32 &= !BIT_MASK_R_MU_RL;
    tmp32 |= (retry_limit << BIT_SHIFT_R_MU_RL) & BIT_MASK_R_MU_RL;
    // Disable Tx MU-MIMO until sounding done
    tmp32 &= !BIT_EN_MU_MIMO;
    // Clear validity of MU STAs
    tmp32 &= !BIT_MASK_R_MU_TABLE_VALID;
    host::w32(h, REG_MU_TX_CTL, tmp32);

    // MU-MIMO Option as default value
    let tmp8 = (ack_policy << BIT_SHIFT_WMAC_TXMU_ACKPOLICY)
        | BIT_WMAC_TXMU_ACKPOLICY_EN;
    host::w8(h, REG_WMAC_MU_BF_OPTION, tmp8);

    // MU-MIMO Control as default value
    host::w16(h, REG_WMAC_MU_BF_CTL, 0);
    // Set MU NDPA rate & BW source
    host::set32(h, REG_TXBF_CTRL, BIT_USE_NDPA_PARAMETER);
    // Set NDPA Rate
    host::w8(h, REG_NDPA_OPT_CTRL, ndpa_rate);

    host::w32_mask(h, REG_BBPSF_CTRL, BIT_MASK_CSI_RATE, DESC_RATE6M);
}
