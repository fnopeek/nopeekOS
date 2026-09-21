//! `struct rtw_dm_info` (main.h:1687-1780) — der Zustand, den die
//! PHY-Schicht zwischen ihren Funktionen traegt.
//!
//! Hier stehen nur die Felder, die Stufe 3 wirklich liest oder schreibt.
//! Ein Feld, das kein portierter Code anfasst, waere eine Behauptung ueber
//! den naechsten Posten und keine Portierung.
#![allow(dead_code)]

use crate::regs::*;

/// `DECLARE_EWMA(name, precision, weight_rcp)` aus
/// `include/linux/average.h`.
///
/// **Die einzige Stelle dieses Treibers, deren Linux-Quelle NICHT in
/// unserem Teilbaum liegt** (`include/linux/average.h` fehlt in
/// `~/.cache/nopeekos/linux-src`). Deshalb steht die Rechnung hier
/// ausgeschrieben, damit sie nachschlagbar ist statt geglaubt:
///
/// ```text
/// add:  internal = internal
///           ? (((internal << w) - internal) + (val << p)) >> w
///           : (val << p)
/// read: internal >> p            w = ilog2(weight_rcp)
/// ```
///
/// `DECLARE_EWMA(thermal, 10, 4)` heisst also p = 10, w = 2.
#[derive(Clone, Copy, Default)]
pub struct Ewma {
    internal: u32,
}

impl Ewma {
    pub const fn new() -> Self {
        Ewma { internal: 0 }
    }

    /// `ewma_*_add`. `precision` und `weight_rcp` kommen von der
    /// Deklarationsstelle — beim Thermometer 10 und 4.
    pub fn add(&mut self, val: u32, precision: u32, weight_rcp: u32) {
        let w = weight_rcp.trailing_zeros(); // ilog2, weight_rcp ist 2^n
        self.internal = if self.internal != 0 {
            (((self.internal << w) - self.internal) + (val << precision)) >> w
        } else {
            val << precision
        };
    }

    /// `ewma_*_read`
    pub fn read(&self, precision: u32) -> u32 {
        self.internal >> precision
    }
}

/// main.h:1592 `DECLARE_EWMA(thermal, 10, 4)`
pub const EWMA_THERMAL_PRECISION: u32 = 10;
pub const EWMA_THERMAL_WEIGHT_RCP: u32 = 4;
/// main.h:667 `DECLARE_EWMA(tp, 10, 2)`
pub const EWMA_TP_PRECISION: u32 = 10;
pub const EWMA_TP_WEIGHT_RCP: u32 = 2;
/// main.h:760 `DECLARE_EWMA(rssi, 10, 16)`
pub const EWMA_RSSI_PRECISION: u32 = 10;
pub const EWMA_RSSI_WEIGHT_RCP: u32 = 16;

/// main.h:1650-1657 `struct rtw_cfo_track`.
///
/// **Der Akkumulator, nicht die Momentaufnahme.** `DmInfo::cfo_tail`
/// daneben ist der letzte Empfangsstatus „for debug"; hier laufen die
/// Summen, aus denen `rtw8822c_cfo_track` alle zwei Sekunden den Quarz
/// nachdreht.
#[derive(Clone, Copy)]
pub struct CfoTrack {
    pub is_adjust: bool,
    pub crystal_cap: u8,
    pub cfo_tail: [i32; 4],
    pub cfo_cnt: [i32; 4],
    pub packet_count: u32,
    pub packet_count_pre: u32,
}

impl CfoTrack {
    pub const fn new() -> Self {
        CfoTrack {
            is_adjust: false,
            crystal_cap: 0,
            cfo_tail: [0; 4],
            cfo_cnt: [0; 4],
            packet_count: 0,
            packet_count_pre: 0,
        }
    }
}

/// main.h:1633-1636 `struct rtw_pkt_count` — was in einem Watchdog-Takt
/// hereinkam, nach Rate aufgeschluesselt. `rtw_phy_stat_rate_cnt`
/// schiebt es nach `last_pkt_count` und faengt von vorn an.
#[derive(Clone, Copy)]
pub struct PktCount {
    pub num_bcn_pkt: u16,
    pub num_qry_pkt: [u16; DESC_RATE_MAX],
}

impl PktCount {
    pub const fn new() -> Self {
        PktCount { num_bcn_pkt: 0, num_qry_pkt: [0; DESC_RATE_MAX] }
    }
}

/// main.h:1621-1631 `struct rtw_dack_info` — der Teil, der die DAC-
/// Kalibrierung ueber einen zweiten Lauf rettet.
#[derive(Clone, Copy)]
pub struct DmInfo {
    /// `dack_adck[path]` — von `dac_cal_adc` gerechnet, von `dac_cal_step1`
    /// wieder eingesetzt.
    pub dack_adck: [u32; DACK_PATH_8822C],
    /// `dack_msbk[path][vec][i]` · vec 0 = I, vec 1 = Q
    pub dack_msbk: [[[u16; DACK_MSBK_BACKUP_NUM]; 2]; DACK_PATH_8822C],
    /// `dack_dck[path][vec][i]`
    pub dack_dck: [[[u8; DACK_DCK_BACKUP_NUM]; 2]; DACK_PATH_8822C],

    /// aus `rtw8822c_phy_set_param`, gelesen von `query_phy_status_page0`
    pub cck_gi_u_bnd: u8,
    pub cck_gi_l_bnd: u8,

    // main.h:1766-1770 — was der EMPFANGSweg zurueckschreibt (Stufe 5a).
    pub rx_snr: [i8; 4],
    pub rx_evm_dbm: [u8; 4],
    pub cfo_tail: [i16; 4],
    pub rssi: [u8; 4],
    pub curr_rx_rate: u8,

    // rtw_phy_init
    /// **u16 wie in Linux** (main.h `u16 fa_history[4]`) — die Breite ist
    /// Semantik: `rtw_phy_dig_recorder` schreibt einen u32-Zaehler hinein
    /// und schneidet ihn dabei ab, und `dig_check_damping` vergleicht
    /// GEGEN diesen abgeschnittenen Wert.
    pub fa_history: [u16; 4],
    pub igi_history: [u8; 4],
    pub igi_bitmap: u8,
    /// `cck_pd_lv[bw][path]`, bw laeuft bis `RTW_CHANNEL_WIDTH_40` = 1.
    pub cck_pd_lv: [[u8; 4]; 2],
    pub cck_fa_avg: u32,
    pub iqk_done: bool,
    pub edcca_mode: u8,
    pub l2h_th_ini: u8,

    // rtw8822c_cfo_init / rtw8822c_cfo_track
    pub cfo_track: CfoTrack,

    // rtw8822c_pwrtrack_init / rtw8822c_pwr_track
    pub delta_power_index: [i8; 4],
    pub delta_power_index_last: [i8; 4],
    pub thermal_avg: [u8; 4],
    pub avg_thermal: [Ewma; 4],
    pub pwr_trk_triggered: bool,
    pub pwr_trk_init_trigger: bool,
    pub thermal_meter_k: u8,
    pub thermal_meter_lck: u8,
    pub default_ofdm_index: u8,
    pub default_cck_index: u8,
    pub txagc_remnant_cck: i8,
    pub txagc_remnant_ofdm: [i8; 4],

    // rtw_phy_stat_rssi / rtw_phy_dig
    pub min_rssi: u8,
    pub pre_min_rssi: u8,
    pub damping: bool,
    pub damping_cnt: u8,
    pub damping_rssi: u8,

    // rtw_phy_stat_rate_cnt
    pub cur_pkt_count: PktCount,
    pub last_pkt_count: PktCount,

    // rtw_phy_ra_track / rtw_phy_rrsr_update
    pub fix_rate: u8,
    pub tx_rate: u8,
    pub rrsr_val_init: u32,
    pub rrsr_mask_min: u32,

    // rtw_phy_cck_pd
    pub cck_pd_default: u8,
    pub dm_flags: u32,
    /// main.h:1780 `u8 scan_density` — Eingabe von `rtw_fw_adaptivity`.
    pub scan_density: u8,

    // rtw8822c_false_alarm_statistics
    pub cck_fa_cnt: u32,
    pub ofdm_fa_cnt: u32,
    pub total_fa_cnt: u32,
    pub cck_ok_cnt: u32,
    pub cck_err_cnt: u32,
    pub ofdm_ok_cnt: u32,
    pub ofdm_err_cnt: u32,
    pub ht_ok_cnt: u32,
    pub ht_err_cnt: u32,
    pub vht_ok_cnt: u32,
    pub vht_err_cnt: u32,
    pub cck_cca_cnt: u32,
    pub ofdm_cca_cnt: u32,
    pub total_cca_cnt: u32,
}

impl DmInfo {
    pub const fn new() -> Self {
        DmInfo {
            dack_adck: [0; DACK_PATH_8822C],
            dack_msbk: [[[0; DACK_MSBK_BACKUP_NUM]; 2]; DACK_PATH_8822C],
            dack_dck: [[[0; DACK_DCK_BACKUP_NUM]; 2]; DACK_PATH_8822C],
            cck_gi_u_bnd: 0,
            cck_gi_l_bnd: 0,
            rx_snr: [0; 4],
            rx_evm_dbm: [0; 4],
            cfo_tail: [0; 4],
            rssi: [0; 4],
            curr_rx_rate: 0,
            fa_history: [0u16; 4],
            igi_history: [0; 4],
            igi_bitmap: 0,
            cck_pd_lv: [[0; 4]; 2],
            cck_fa_avg: 0,
            iqk_done: false,
            edcca_mode: 0,
            l2h_th_ini: 0,
            cfo_track: CfoTrack::new(),
            delta_power_index: [0; 4],
            delta_power_index_last: [0; 4],
            thermal_avg: [0; 4],
            avg_thermal: [Ewma::new(); 4],
            pwr_trk_triggered: false,
            pwr_trk_init_trigger: false,
            thermal_meter_k: 0,
            thermal_meter_lck: 0,
            default_ofdm_index: 0,
            default_cck_index: 0,
            txagc_remnant_cck: 0,
            txagc_remnant_ofdm: [0; 4],
            min_rssi: 0,
            pre_min_rssi: 0,
            damping: false,
            damping_cnt: 0,
            damping_rssi: 0,
            cur_pkt_count: PktCount::new(),
            last_pkt_count: PktCount::new(),
            fix_rate: 0,
            tx_rate: 0,
            rrsr_val_init: 0,
            rrsr_mask_min: 0,
            cck_pd_default: 0,
            dm_flags: 0,
            scan_density: 0,
            cck_fa_cnt: 0,
            ofdm_fa_cnt: 0,
            total_fa_cnt: 0,
            cck_ok_cnt: 0,
            cck_err_cnt: 0,
            ofdm_ok_cnt: 0,
            ofdm_err_cnt: 0,
            ht_ok_cnt: 0,
            ht_err_cnt: 0,
            vht_ok_cnt: 0,
            vht_err_cnt: 0,
            cck_cca_cnt: 0,
            ofdm_cca_cnt: 0,
            total_cca_cnt: 0,
        }
    }
}

/// main.h:1782-1790 `struct rtw_path_div`
#[derive(Clone, Copy, Default)]
pub struct PathDiv {
    pub current_tx_path: u8,
    pub path_a_sum: u32,
    pub path_b_sum: u32,
    pub path_a_cnt: u16,
    pub path_b_cnt: u16,
}

impl PathDiv {
    pub const fn new() -> Self {
        PathDiv {
            current_tx_path: 0,
            path_a_sum: 0,
            path_b_sum: 0,
            path_a_cnt: 0,
            path_b_cnt: 0,
        }
    }
}
