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

// ════════════════════════════════════════════════════════════════
// Stufe 6a: der Schluesselspeicher (sec.c:24-100)
// ════════════════════════════════════════════════════════════════

/// sec.h:20-27 `struct rtw_cam_entry` — ein Platz im Schluesselspeicher.
#[derive(Clone, Copy)]
pub struct CamEntry {
    pub valid: bool,
    pub group: bool,
    pub hw_key_type: u8,
    pub addr: [u8; 6],
    pub key: [u8; 16],
    pub keyidx: u8,
}

impl Default for CamEntry {
    fn default() -> Self {
        CamEntry { valid: false, group: false, hw_key_type: 0,
                   addr: [0; 6], key: [0; 16], keyidx: 0 }
    }
}

/// sec.c:24-84 `rtw_sec_write_cam`.
///
/// Acht Worte je Platz, RUECKWAERTS geschrieben (`for i = 7; i >= 0`) —
/// Wort 0 traegt das Gueltig-Bit und geht damit zuletzt hinaus. Wer
/// vorwaerts schriebe, machte den Platz gueltig, bevor der Schluessel
/// drin steht.
pub fn write_cam(h: i32, cam: &mut CamEntry, hw_key_idx: u8,
                 hw_key_type: u8, keyidx: u8, group: bool,
                 addr: &[u8; 6], key: &[u8]) {
    cam.valid = true;
    cam.group = group;
    cam.hw_key_type = hw_key_type;
    cam.keyidx = keyidx;
    cam.addr = *addr;
    cam.key = [0; 16];
    let n = key.len().min(16);
    cam.key[..n].copy_from_slice(&key[..n]);

    let write_cmd = RTW_SEC_CMD_WRITE_ENABLE | RTW_SEC_CMD_POLLING;
    let base = (hw_key_idx as u32) << RTW_SEC_CAM_ENTRY_SHIFT;

    for i in (0..8u32).rev() {
        let content = match i {
            0 => {
                (cam.keyidx as u32 & 0x3)
                    | ((hw_key_type as u32 & 0x7) << 2)
                    | ((cam.group as u32) << 6)
                    | ((cam.valid as u32) << 15)
                    | ((cam.addr[0] as u32) << 16)
                    | ((cam.addr[1] as u32) << 24)
            }
            1 => {
                (cam.addr[2] as u32)
                    | ((cam.addr[3] as u32) << 8)
                    | ((cam.addr[4] as u32) << 16)
                    | ((cam.addr[5] as u32) << 24)
            }
            6 | 7 => 0,
            _ => {
                let j = ((i - 2) << 2) as usize;
                (cam.key[j] as u32)
                    | ((cam.key[j + 1] as u32) << 8)
                    | ((cam.key[j + 2] as u32) << 16)
                    | ((cam.key[j + 3] as u32) << 24)
            }
        };
        let command = write_cmd | (base + i);
        host::w32(h, RTW_SEC_WRITE_REG, content);
        host::w32(h, RTW_SEC_CMD_REG, command);
    }
}

/// sec.c:86-104 `rtw_sec_clear_cam`
pub fn clear_cam(h: i32, cam: &mut CamEntry, hw_key_idx: u8) {
    cam.valid = false;
    cam.addr = [0; 6];

    let write_cmd = RTW_SEC_CMD_WRITE_ENABLE | RTW_SEC_CMD_POLLING;
    let command = write_cmd | ((hw_key_idx as u32) << RTW_SEC_CAM_ENTRY_SHIFT);
    host::w32(h, RTW_SEC_WRITE_REG, 0);
    host::w32(h, RTW_SEC_CMD_REG, command);
}
