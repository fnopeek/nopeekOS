//! `struct rtw_dm_info` (main.h:1687-1780) — der Zustand, den die
//! PHY-Schicht zwischen ihren Funktionen traegt.
//!
//! Hier stehen nur die Felder, die Stufe 3 wirklich liest oder schreibt.
//! Ein Feld, das kein portierter Code anfasst, waere eine Behauptung ueber
//! den naechsten Posten und keine Portierung.
#![allow(dead_code)]

use crate::regs::*;

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

    // rtw_phy_init
    pub fa_history: [u32; 4],
    pub igi_history: [u8; 4],
    pub igi_bitmap: u8,
    /// `cck_pd_lv[bw][path]`, bw laeuft bis `RTW_CHANNEL_WIDTH_40` = 1.
    pub cck_pd_lv: [[u8; 4]; 2],
    pub cck_fa_avg: u32,
    pub iqk_done: bool,
    pub edcca_mode: u8,
    pub l2h_th_ini: u8,

    // rtw8822c_cfo_init
    pub cfo_crystal_cap: u8,
    pub cfo_is_adjust: bool,

    // rtw8822c_pwrtrack_init
    pub delta_power_index: [i8; 4],
    pub thermal_avg: [u8; 4],
    pub pwr_trk_triggered: bool,
    pub thermal_meter_k: u8,
    pub thermal_meter_lck: u8,

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
            fa_history: [0; 4],
            igi_history: [0; 4],
            igi_bitmap: 0,
            cck_pd_lv: [[0; 4]; 2],
            cck_fa_avg: 0,
            iqk_done: false,
            edcca_mode: 0,
            l2h_th_ini: 0,
            cfo_crystal_cap: 0,
            cfo_is_adjust: false,
            delta_power_index: [0; 4],
            thermal_avg: [0; 4],
            pwr_trk_triggered: false,
            thermal_meter_k: 0,
            thermal_meter_lck: 0,
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
    pub path_a_cnt: u16,
    pub path_a_sum: u16,
    pub path_b_cnt: u16,
    pub path_b_sum: u16,
}
