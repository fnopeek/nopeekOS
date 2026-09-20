//! Die RF-Kalibrierung des 8822C — `rtw8822c_phy_calibration` und was sie
//! umgibt (Stufe 5d).
//!
//! **Wann sie laeuft, entscheidet Linux und nicht wir:** `rtw_set_channel`
//! setzt nur `need_rfk = true`, und `rtw_chip_prepare_tx` fuehrt sie aus,
//! wenn mac80211 `mgd_prepare_tx` ruft — also VOR dem Anmelden. Auf jedem
//! Kanal eines Suchlaufs zu kalibrieren dauert zu lange; der Kommentar in
//! `main.c` sagt genau das.
//!
//! Portiert: `rtw8822c_rfk_power_save` · `rtw8822c_rfk_handshake` ·
//! `rtw8822c_do_iqk` · `rtw8822c_phy_calibration`.
#![allow(dead_code)]

use crate::host;
use crate::pci::Trx;
use crate::regs::*;

/// rtw8822c.c:1231-1242 `rtw8822c_rfk_power_save`
pub fn power_save(h: i32, rf_path_num: u8, is_power_save: bool) {
    for path in 0..rf_path_num as u32 {
        host::w32_mask(h, REG_NCTL0, BIT_SEL_PATH, path);
        host::w32_mask(h, REG_DPD_CTL1_S0, BIT_PS_EN,
                       if is_power_save { 0 } else { 1 });
    }
}

/// Was der Handschlag gemeldet hat. Drei Wartezeiten, drei Ausgaenge —
/// Linux schreibt sie in die Debugausgabe, wir behalten sie, weil sie die
/// einzige Auskunft darueber sind, ob die Firmware mitspielt.
#[derive(Default, Clone, Copy)]
pub struct HandshakeRpt {
    pub bt_iqk_waited_us: u64,
    pub bt_iqk_timeout: bool,
    pub start_ack: bool,
    pub start_ack_us: u64,
    pub finish_ack: bool,
    pub finish_ack_us: u64,
}

/// rtw8822c.c:1134-1178 `rtw8822c_rfk_handshake`.
///
/// **`is_bt_iqk_timeout` merkt sich einen Fehlschlag fuer immer** — nach
/// einer Zeitueberschreitung wird auf die BT-IQK nie wieder gewartet. Das
/// ist Absicht: eine BT-Seite, die einmal nicht antwortet, kostet sonst bei
/// JEDER Kalibrierung 600 ms.
pub fn handshake(h: i32, is_before_k: bool, is_bt_iqk_timeout: &mut bool,
                 st: &mut crate::fw::H2cState) -> HandshakeRpt {
    let mut r = HandshakeRpt::default();

    if is_before_k {
        if !*is_bt_iqk_timeout {
            // `read_poll_timeout(rtw_read32_mask, …, 20, 600000, …)`
            let t0 = host::now_us();
            loop {
                if host::r32_mask(h, REG_PMC_DBG_CTRL1, BITS_PMC_BT_IQK_STS) == 0 {
                    break;
                }
                if host::now_us() - t0 >= 600_000 {
                    r.bt_iqk_timeout = true;
                    *is_bt_iqk_timeout = true;
                    break;
                }
                host::delay_us(20);
            }
            r.bt_iqk_waited_us = host::now_us() - t0;
        }

        crate::fw::inform_rfk_status(h, st, true);
        let (ok, us) = wait_rfk_ack(h);
        r.start_ack = ok;
        r.start_ack_us = us;
    } else {
        crate::fw::inform_rfk_status(h, st, false);
        let (ok, us) = wait_rfk_ack(h);
        r.finish_ack = ok;
        r.finish_ack_us = us;
    }
    r
}

/// `read_poll_timeout(rtw_read8_mask, u1b_tmp, u1b_tmp == 1, 20, 100000, …,
/// REG_ARFR4, BIT_WL_RFK)` — beide Zweige des Handschlags warten damit auf
/// dieselbe Quittung.
fn wait_rfk_ack(h: i32) -> (bool, u64) {
    let t0 = host::now_us();
    loop {
        if host::r8(h, REG_ARFR4) & BIT_WL_RFK == BIT_WL_RFK {
            return (true, host::now_us() - t0);
        }
        if host::now_us() - t0 >= 100_000 {
            return (false, host::now_us() - t0);
        }
        host::delay_us(20);
    }
}

/// rtw8822c.c:1836-1851 `rtw8822c_do_iqk`.
///
/// **Die IQK rechnet die Firmware.** Der Treiber schickt ein H2C-Paket und
/// wartet bis zu 300 ms darauf, dass `REG_RPT_CIP` den Wert `0xaa` traegt.
/// Danach wird `REG_IQKSTAT` geloescht — auch dann, wenn die Wartezeit
/// abgelaufen ist; Linux macht dazwischen keinen Unterschied.
pub fn do_iqk(h: i32, trx: &mut Trx, stage: i32, st: &mut crate::fw::H2cState)
    -> (bool, u64, u8)
{
    // `para.clear = 1`, `segment_iqk` bleibt 0 (`{0}`-Initialisierung).
    crate::fw::do_iqk(h, trx, stage, st, true, false);

    let t0 = host::now_us();
    let mut chk;
    loop {
        chk = host::r8(h, REG_RPT_CIP);
        if chk == IQK_DONE_8822C {
            break;
        }
        if host::now_us() - t0 >= 300_000 {
            break;
        }
        host::delay_us(20_000);
    }
    let us = host::now_us() - t0;
    host::w8(h, REG_IQKSTAT, 0x0);
    (chk == IQK_DONE_8822C, us, chk)
}

/// rtw8822c.c:1825-1834 `rtw8822c_do_gapk`.
///
/// **Die Pruefung ist umgekehrt, als der Name vermuten laesst:** ist das
/// Bit GESETZT, ist TXGAPK ABGESCHALTET. `dm_flags` wird in Linux nur aus
/// debugfs beschrieben und ist beim Start null — TXGAPK laeuft also im
/// Normalfall mit.
pub fn do_gapk(h: i32, g: &mut crate::txgapk::GapkInfo, rf_path_num: u8,
               dm_flags: u32, power_track_type: u8,
               is_bt_iqk_timeout: &mut bool, st: &mut crate::fw::H2cState)
    -> (crate::txgapk::TxgapkRpt, HandshakeRpt, HandshakeRpt)
{
    if dm_flags & (1 << RTW_DM_CAP_TXGAPK) != 0 {
        return (crate::txgapk::TxgapkRpt::Disabled,
                HandshakeRpt::default(), HandshakeRpt::default());
    }
    let hs1 = handshake(h, true, is_bt_iqk_timeout, st);
    let rpt = crate::txgapk::txgapk(h, g, rf_path_num, dm_flags,
                                    power_track_type);
    let hs2 = handshake(h, false, is_bt_iqk_timeout, st);
    (rpt, hs1, hs2)
}
