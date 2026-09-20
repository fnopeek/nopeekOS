//! `sec.c` aus Linux 6.18.26 rtw88 — nur `rtw_sec_enable_sec_engine`.
//!
//! Der Rest von sec.c (Schluessel in die CAM schreiben, `rtw_sec_write_cam`,
//! `rtw_sec_clear_cam`) haengt an einer VERBINDUNG und gehoert zu der Stufe,
//! die eine aufbaut. Hier steht, was `rtw_core_start` beim Anlaufen tut.
#![allow(dead_code)]

use crate::host;
use crate::regs::*;

/// sec.c `rtw_sec_enable_sec_engine`.
///
/// `sec->default_key_search` wird in derselben Funktion auf `true` gesetzt
/// („default use default key search for now"), also gilt der Zweig immer.
pub fn enable_sec_engine(h: i32) {
    let ctrl_reg = host::r16(h, REG_CR) | RTW_SEC_ENGINE_EN;
    host::w16(h, REG_CR, ctrl_reg);

    let mut sec_config = host::r16(h, RTW_SEC_CONFIG);
    sec_config |= RTW_SEC_TX_DEC_EN | RTW_SEC_RX_DEC_EN;
    // default_key_search == true
    sec_config |= RTW_SEC_TX_UNI_USE_DK | RTW_SEC_RX_UNI_USE_DK
        | RTW_SEC_TX_BC_USE_DK | RTW_SEC_RX_BC_USE_DK;
    host::w16(h, RTW_SEC_CONFIG, sec_config);
}
