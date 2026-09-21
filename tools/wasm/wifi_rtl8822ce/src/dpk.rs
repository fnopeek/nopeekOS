//! `rtw8822c_do_dpk` — die digitale Vorverzerrung (Stufe 5d),
//! rtw8822c.c:3171-4186.
//!
//! DPK misst die Kennlinie des Leistungsverstaerkers und legt eine
//! Umkehrfunktion davor. Ohne sie sendet der Chip linear nur bis zu einem
//! Pegel sauber; darueber verzerrt er, und die hohen Modulationen fallen
//! als erstes aus.
//!
//! **Der Einstieg haengt an `is_dpk_pwr_on`**, und das setzt
//! `rtw_load_rfk_table` (phy.c:1847) am Ende des RFK-Tabellenladens. Ohne
//! geladene RFK-Tabelle gibt es keine DPK — das ist kein Sonderfall,
//! sondern die Reihenfolge.
#![allow(dead_code)]

use crate::host;
use crate::phy;
use crate::regs::*;
use crate::tables;

pub const DPK_RF_PATH_NUM_U: usize = DPK_RF_PATH_NUM as usize;
pub const DPK_BB_REG_NUM_U: usize = DPK_BB_REG_NUM as usize;
pub const DPK_RF_REG_NUM_U: usize = DPK_RF_REG_NUM as usize;
const PATHS: usize = 4; // RTW_RF_PATH_MAX

/// main.h:1591-1614 `struct rtw_dpk_info`.
///
/// `avg_thermal` ist der gleitende Mittelwert fuer `rtw8822c_dpk_track`
/// — seit 0.26.0 hat er einen Leser (der Watchdog alle zwei Sekunden).
pub struct DpkInfo {
    pub is_dpk_pwr_on: bool,
    pub is_reload: bool,
    pub dpk_path_ok: u8, // DECLARE_BITMAP(…, DPK_RF_PATH_NUM)
    pub thermal_dpk: [u8; DPK_RF_PATH_NUM_U],
    /// main.h:1598 `struct ewma_thermal avg_thermal[DPK_RF_PATH_NUM]`
    pub avg_thermal: [crate::dm::Ewma; DPK_RF_PATH_NUM_U],
    pub gnt_control: u32,
    pub gnt_value: u32,
    pub result: [u8; PATHS],
    pub dpk_txagc: [u8; PATHS],
    pub coef: [[u32; 20]; PATHS],
    pub dpk_gs: [u16; PATHS],
    pub thermal_dpk_delta: [u8; PATHS],
    pub pre_pwsf: [u8; PATHS],
    pub dpk_band: u8,
    pub dpk_ch: u8,
    pub dpk_bw: u8,
}

impl DpkInfo {
    pub const fn new() -> Self {
        DpkInfo {
            is_dpk_pwr_on: false,
            is_reload: false,
            dpk_path_ok: 0,
            thermal_dpk: [0; DPK_RF_PATH_NUM_U],
            avg_thermal: [crate::dm::Ewma::new(); DPK_RF_PATH_NUM_U],
            gnt_control: 0,
            gnt_value: 0,
            result: [0; PATHS],
            dpk_txagc: [0; PATHS],
            coef: [[0; 20]; PATHS],
            dpk_gs: [0; PATHS],
            thermal_dpk_delta: [0; PATHS],
            pre_pwsf: [0; PATHS],
            dpk_band: 0,
            dpk_ch: 0,
            dpk_bw: 0,
        }
    }
    fn path_ok(&self, path: usize) -> bool {
        self.dpk_path_ok & (1 << path) != 0
    }
    fn set_path_ok(&mut self, path: usize) {
        self.dpk_path_ok |= 1 << path;
    }
    fn clear_path_ok(&mut self, path: usize) {
        self.dpk_path_ok &= !(1 << path);
    }
}

impl Default for DpkInfo {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn fget(v: u32, mask: u32) -> u32 {
    (v & mask) >> mask.trailing_zeros()
}

/// util.c `rtw_backup_info` — Adresse und Wert, alle vier Byte breit.
#[derive(Clone, Copy, Default)]
pub struct Backup {
    reg: u32,
    val: u32,
}

/// rtw8822c.c:3171-3185 `rtw8822c_dpk_set_gnt_wl`.
///
/// Waehrend der Kalibrierung gehoert die Antenne dem WLAN allein. Der
/// vorige Zustand wird gemerkt und hinterher zurueckgegeben — sonst nimmt
/// die naechste Koexistenz-Entscheidung eine Stellung an, die nicht steht.
fn set_gnt_wl(h: i32, d: &mut DpkInfo, is_before_k: bool) {
    if is_before_k {
        d.gnt_control = host::r32(h, 0x70);
        d.gnt_value = crate::coex::read_indirect_reg(h, 0x38);
        host::w32_mask(h, 0x70, 1 << 26, 0x1);
        crate::coex::write_indirect_reg(h, 0x38, MASKBYTE1, 0x77);
    } else {
        crate::coex::write_indirect_reg(h, 0x38, MASKDWORD, d.gnt_value);
        host::w32(h, 0x70, d.gnt_control);
    }
}

/// rtw8822c.c:3187-3194 `rtw8822c_dpk_restore_registers`
fn restore_registers(h: i32, bckp: &[Backup]) {
    for b in bckp {
        host::w32(h, b.reg, b.val);
    }
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0xc);
    host::w32_mask(h, REG_RXSRAM_CTL, BIT_DPD_CLK, 0x4);
}

/// rtw8822c.c:3196-3207 `rtw8822c_dpk_backup_registers`
fn backup_registers(h: i32, reg: &[u32], bckp: &mut [Backup]) {
    for (i, &r) in reg.iter().enumerate() {
        bckp[i].reg = r;
        bckp[i].val = host::r32(h, r);
    }
}

/// rtw8822c.c:3209-3221 `rtw8822c_dpk_backup_rf_registers`
fn backup_rf_registers(h: i32, rf_reg: &[u32],
                       bak: &mut [[u32; 2]; DPK_RF_REG_NUM_U]) {
    for i in 0..DPK_RF_REG_NUM_U {
        bak[i][phy::RF_PATH_A] =
            phy::read_rf(h, phy::RF_PATH_A, rf_reg[i], phy::RFREG_MASK);
        bak[i][phy::RF_PATH_B] =
            phy::read_rf(h, phy::RF_PATH_B, rf_reg[i], phy::RFREG_MASK);
    }
}

/// rtw8822c.c:3223-3235 `rtw8822c_dpk_reload_rf_registers`
fn reload_rf_registers(h: i32, rf_reg: &[u32],
                       bak: &[[u32; 2]; DPK_RF_REG_NUM_U]) {
    for i in 0..DPK_RF_REG_NUM_U {
        phy::write_rf_reg_mix(h, phy::RF_PATH_A, rf_reg[i], phy::RFREG_MASK,
                              bak[i][phy::RF_PATH_A]);
        phy::write_rf_reg_mix(h, phy::RF_PATH_B, rf_reg[i], phy::RFREG_MASK,
                              bak[i][phy::RF_PATH_B]);
    }
}

/// rtw8822c.c:3237-3249 `rtw8822c_dpk_information`
fn information(h: i32, d: &mut DpkInfo) {
    let reg = phy::read_rf(h, phy::RF_PATH_A, 0x18, phy::RFREG_MASK);
    let band_shift = fget(reg, 1 << 16);
    d.dpk_band = (1 << band_shift) as u8;
    d.dpk_ch = fget(reg, 0xff) as u8;
    d.dpk_bw = fget(reg, 0x3000) as u8;
}

/// rtw8822c.c:3251-3258 `rtw8822c_dpk_rxbb_dc_cal`
pub fn rxbb_dc_cal(h: i32, path: usize) {
    phy::write_rf_reg_mix(h, path, 0x92, phy::RFREG_MASK, 0x84800);
    host::delay_us(5);
    phy::write_rf_reg_mix(h, path, 0x92, phy::RFREG_MASK, 0x84801);
    host::delay_us(600);
    phy::write_rf_reg_mix(h, path, 0x92, phy::RFREG_MASK, 0x84800);
}

/// rtw8822c.c:3260-3283 `rtw8822c_dpk_dc_corr_check`
fn dc_corr_check(h: i32, _path: usize) -> u8 {
    host::w32(h, REG_RXSRAM_CTL, 0x000900f0);
    let mut dc_i = host::r32_mask(h, REG_STAT_RPT, 0x0fff_0000) as u16;
    let mut dc_q = host::r32_mask(h, REG_STAT_RPT, 0x0000_0fff) as u16;

    if dc_i & (1 << 11) != 0 {
        dc_i = 0x1000 - dc_i;
    }
    if dc_q & (1 << 11) != 0 {
        dc_q = 0x1000 - dc_q;
    }

    host::w32(h, REG_RXSRAM_CTL, 0x000000f0);
    let corr_idx = host::r32_mask(h, REG_STAT_RPT, 0xff) as u8;
    // Der zweite Lesezugriff steht in Linux ohne Empfaenger da. Er bleibt,
    // weil ein Lesezugriff auf diesem Bus eine WIRKUNG haben kann.
    let _ = host::r32_mask(h, REG_STAT_RPT, 0xff00);

    u8::from(dc_i > 200 || dc_q > 200 || corr_idx < 40 || corr_idx > 65)
}

/// rtw8822c.c:3285-3299 `rtw8822c_dpk_tx_pause`
fn tx_pause(h: i32) {
    host::w8(h, 0x522, 0xff);
    host::w32_mask(h, 0x1e70, 0xf, 0x2);

    let mut count = 0u16;
    loop {
        let reg_a = phy::read_rf(h, phy::RF_PATH_A, 0x00, 0xf0000) as u8;
        let reg_b = phy::read_rf(h, phy::RF_PATH_B, 0x00, 0xf0000) as u8;
        host::delay_us(2);
        count += 1;
        if !((reg_a == 2 || reg_b == 2) && count < 2500) {
            break;
        }
    }
}

/// `rtw8822c_parse_tbl_dpk` — Tripel aus Adresse, Maske und Wert.
fn load_dpk_table(h: i32, tbl: &[u32]) {
    for t in tbl.chunks_exact(3) {
        host::w32_mask(h, t[0], t[1], t[2]);
    }
}

/// rtw8822c.c:3301-3305 `rtw8822c_dpk_mac_bb_setting`
fn mac_bb_setting(h: i32) {
    tx_pause(h);
    load_dpk_table(h, &tables::DPK_MAC_BB);
}

/// rtw8822c.c:3307-3313 `rtw8822c_dpk_afe_setting`
fn afe_setting(h: i32, is_do_dpk: bool) {
    if is_do_dpk {
        load_dpk_table(h, &tables::DPK_AFE_IS_DPK);
    } else {
        load_dpk_table(h, &tables::DPK_AFE_NO_DPK);
    }
}

/// rtw8822c.c:3315-3332 `rtw8822c_dpk_pre_setting`
fn pre_setting(h: i32, d: &DpkInfo, rf_path_num: u8) {
    for path in 0..rf_path_num as u32 {
        phy::write_rf_reg_mix(h, path as usize, RF_RXAGC_OFFSET,
                              phy::RFREG_MASK, 0x0);
        host::w32(h, REG_NCTL0, 0x8 | (path << 1));
        if d.dpk_band as u32 == RTW_BAND_2G_MASK {
            host::w32(h, REG_DPD_CTL1_S1, 0x1f100000);
        } else {
            host::w32(h, REG_DPD_CTL1_S1, 0x1f0d0000);
        }
        host::w32_mask(h, REG_DPD_LUT0, BIT_GLOSS_DB, 0x4);
        host::w32_mask(h, REG_IQK_CTL1, BIT_TX_CFIR, 0x3);
    }
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0xc);
    host::w32(h, REG_DPD_CTL11, 0x3b23170b);
    host::w32(h, REG_DPD_CTL12, 0x775f5347);
}

/// rtw8822c.c:3334-3370 `rtw8822c_dpk_rf_setting`
fn rf_setting(h: i32, d: &DpkInfo, path: usize) -> u32 {
    phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, phy::RFREG_MASK, 0x50017);
    let ori_txbb = phy::read_rf(h, path, RF_TX_GAIN, phy::RFREG_MASK);

    phy::write_rf_reg_mix(h, path, RF_DEBUG, BIT_DE_TX_GAIN, 0x1);
    phy::write_rf_reg_mix(h, path, RF_DEBUG, BIT_DE_PWR_TRIM, 0x1);
    phy::write_rf_reg_mix(h, path, RF_TX_GAIN_OFFSET, BIT_BB_GAIN, 0x0);
    phy::write_rf_reg_mix(h, path, RF_TX_GAIN, phy::RFREG_MASK, ori_txbb);

    if d.dpk_band as u32 == RTW_BAND_2G_MASK {
        phy::write_rf_reg_mix(h, path, RF_TX_GAIN_OFFSET, BIT_RF_GAIN, 0x1);
        phy::write_rf_reg_mix(h, path, RF_RXG_GAIN, BIT_RXG_GAIN, 0x0);
    } else {
        phy::write_rf_reg_mix(h, path, RF_TXA_LB_SW, BIT_TXA_LB_ATT, 0x0);
        phy::write_rf_reg_mix(h, path, RF_TXA_LB_SW, BIT_LB_ATT, 0x6);
        phy::write_rf_reg_mix(h, path, RF_TXA_LB_SW, BIT_LB_SW, 0x1);
        phy::write_rf_reg_mix(h, path, RF_RXA_MIX_GAIN, BIT_RXA_MIX_GAIN, 0);
    }

    phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, BIT_RXAGC, 0xf);
    phy::write_rf_reg_mix(h, path, RF_DEBUG, BIT_DE_TRXBW, 0x1);
    phy::write_rf_reg_mix(h, path, RF_BW_TRXBB, BIT_BW_RXBB, 0x0);

    if d.dpk_bw as u32 == DPK_CHANNEL_WIDTH_80 {
        phy::write_rf_reg_mix(h, path, RF_BW_TRXBB, BIT_BW_TXBB, 0x2);
    } else {
        phy::write_rf_reg_mix(h, path, RF_BW_TRXBB, BIT_BW_TXBB, 0x1);
    }

    phy::write_rf_reg_mix(h, path, RF_EXT_TIA_BW, 1 << 1, 0x1);

    host::delay_us(100);

    ori_txbb & 0x1f
}

/// rtw8822c.c:3372-3395 `rtw8822c_dpk_get_cmd`
fn get_cmd(d: &DpkInfo, action: u32, path: usize) -> u32 {
    let bw = if d.dpk_bw as u32 == DPK_CHANNEL_WIDTH_80 { 2 } else { 0 };
    let cmd = match action {
        RTW_DPK_GAIN_LOSS => 0x14 + path as u32,
        RTW_DPK_DO_DPK => 0x16 + path as u32 + bw,
        RTW_DPK_DPK_ON => 0x1a + path as u32,
        RTW_DPK_DAGC => 0x1c + path as u32 + bw,
        _ => return 0,
    };
    (cmd << 8) | 0x48
}

/// mac.c `check_hw_ready` — lesen, maskieren, gegen einen Wert, 20 ms.
fn check_hw_ready(h: i32, addr: u32, mask: u32, target: u32) -> bool {
    for _ in 0..20 {
        if host::r32_mask(h, addr, mask) == target {
            return true;
        }
        host::sleep_ms(1);
    }
    false
}

/// rtw8822c.c:3397-3436 `rtw8822c_dpk_one_shot`
fn one_shot(h: i32, d: &mut DpkInfo, path: usize, action: u32) -> u8 {
    let mut result = 0u8;

    set_gnt_wl(h, d, true);

    if action == RTW_DPK_CAL_PWR {
        host::w32_mask(h, REG_DPD_CTL0, 1 << 12, 0x1);
        host::w32_mask(h, REG_DPD_CTL0, 1 << 12, 0x0);
        host::w32_mask(h, REG_RXSRAM_CTL, BIT_RPT_SEL, 0x0);
        host::sleep_ms(10);
        if !check_hw_ready(h, REG_STAT_RPT, 1 << 31, 0x1) {
            result = 1;
        }
    } else {
        host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));
        host::w32_mask(h, REG_R_CONFIG, BIT_IQ_SWITCH, 0x9);

        let dpk_cmd = get_cmd(d, action, path);
        host::w32(h, REG_NCTL0, dpk_cmd);
        host::w32(h, REG_NCTL0, dpk_cmd + 1);
        host::sleep_ms(10);
        if !check_hw_ready(h, 0x2d9c, 0xff, 0x55) {
            result = 1;
        }
        host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));
        host::w32_mask(h, REG_R_CONFIG, BIT_IQ_SWITCH, 0x0);
    }

    set_gnt_wl(h, d, false);

    host::w8(h, 0x1b10, 0x0);

    result
}

/// rtw8822c.c:3438-3448 `rtw8822c_dpk_dgain_read`
fn dgain_read(h: i32) -> u16 {
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0xc);
    host::w32_mask(h, REG_RXSRAM_CTL, 0x00ff0000, 0x0);
    host::r32_mask(h, REG_STAT_RPT, 0x0fff_0000) as u16
}

/// rtw8822c.c:3450-3458 `rtw8822c_dpk_thermal_read`
fn thermal_read(h: i32, path: usize) -> u8 {
    phy::write_rf_reg_mix(h, path, RF_T_METER, 1 << 19, 0x1);
    phy::write_rf_reg_mix(h, path, RF_T_METER, 1 << 19, 0x0);
    phy::write_rf_reg_mix(h, path, RF_T_METER, 1 << 19, 0x1);
    host::delay_us(15);
    phy::read_rf(h, path, RF_T_METER, 0x0007e) as u8
}

/// rtw8822c.c:3460-3484 `rtw8822c_dpk_pas_read`
fn pas_read(h: i32, path: usize) -> u32 {
    host::w32(h, REG_NCTL0, 0x8 | ((path as u32) << 1));
    host::w32_mask(h, 0x1b48, 1 << 14, 0x0);
    host::w32(h, REG_RXSRAM_CTL, 0x00060001);
    host::w32(h, 0x1b4c, 0x00000000);
    host::w32(h, 0x1b4c, 0x00080000);

    let mut q_val = host::r32_mask(h, REG_STAT_RPT, MASKHWORD);
    let mut i_val = host::r32_mask(h, REG_STAT_RPT, MASKLWORD);

    if i_val & (1 << 15) != 0 {
        i_val = 0x10000 - i_val;
    }
    if q_val & (1 << 15) != 0 {
        q_val = 0x10000 - q_val;
    }

    host::w32(h, 0x1b4c, 0x00000000);

    i_val * i_val + q_val * q_val
}

/// rtw8822c.c:3486-3507 `rtw8822c_psd_log2base`
fn psd_log2base(val: u32) -> u32 {
    const TABLE_FRACTION: [u32; 21] = [
        0, 432, 332, 274, 232, 200, 174, 151, 132, 115, 100, 86, 74, 62, 51,
        42, 32, 23, 15, 7, 0,
    ];
    if val == 0 {
        return 0;
    }
    // `__fls(val) + 1` — Stelle des hoechsten gesetzten Bits, von eins an.
    let val_integerd_b = 32 - val.leading_zeros();
    let tmp = (val * 100) / (1 << val_integerd_b);
    let mut tindex = (tmp / 5) as usize;
    if tindex >= TABLE_FRACTION.len() {
        tindex = TABLE_FRACTION.len() - 1;
    }
    val_integerd_b * 100 - TABLE_FRACTION[tindex]
}

/// rtw8822c.c:3509-3522 `rtw8822c_dpk_gainloss_result`
fn gainloss_result(h: i32, path: usize) -> u8 {
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));
    host::w32_mask(h, 0x1b48, 1 << 14, 0x1);
    host::w32(h, REG_RXSRAM_CTL, 0x00060000);
    let result = host::r32_mask(h, REG_STAT_RPT, 0x000000f0) as u8;
    host::w32_mask(h, 0x1b48, 1 << 14, 0x0);
    result
}

/// rtw8822c.c:3524-3539 `rtw8822c_dpk_agc_gain_chk`
fn agc_gain_chk(h: i32, d: &mut DpkInfo, path: usize, limited_pga: u8) -> u32 {
    one_shot(h, d, path, RTW_DPK_DAGC);
    let dgain = dgain_read(h);

    if dgain > 1535 && limited_pga == 0 {
        RTW_DPK_GAIN_LESS
    } else if dgain < 768 && limited_pga == 0 {
        RTW_DPK_GAIN_LARGE
    } else {
        0 // = RTW_DPK_GAIN_CHECK
    }
}

/// rtw8822c.c:3541-3556 `rtw8822c_dpk_agc_loss_chk`
fn agc_loss_chk(h: i32, path: usize) -> u32 {
    let loss = pas_read(h, path);
    if loss < 0x4000000 {
        return RTW_DPK_GL_LESS;
    }
    // `3 * log2base(loss >> 13) - 3870` in VORZEICHENLOSER Arithmetik, wie
    // in C: wird es negativ, laeuft es um und ist damit gross.
    let loss_db = (3u32.wrapping_mul(psd_log2base(loss >> 13)))
        .wrapping_sub(3870);

    if loss_db > 1000 {
        RTW_DPK_GL_LARGE
    } else if loss_db < 250 {
        RTW_DPK_GL_LESS
    } else {
        RTW_DPK_AGC_OUT
    }
}

/// rtw8822c.c:3558-3566 `struct rtw8822c_dpk_data`
#[derive(Default, Clone, Copy)]
struct DpkData {
    txbb: u8,
    pga: u8,
    limited_pga: u8,
    agc_cnt: u8,
    loss_only: bool,
    gain_only: bool,
    path: usize,
}

/// rtw8822c.c:3568-3593 `rtw8822c_gain_check_state`
fn gain_check_state(h: i32, d: &mut DpkInfo, s: &mut DpkData) -> u32 {
    s.txbb = phy::read_rf(h, s.path, RF_TX_GAIN, BIT_GAIN_TXBB) as u8;
    s.pga = phy::read_rf(h, s.path, RF_MODE_TRXAGC, BIT_RXAGC) as u8;

    let mut state = if s.loss_only {
        RTW_DPK_LOSS_CHECK
    } else {
        let st = agc_gain_chk(h, d, s.path, s.limited_pga);
        if st == RTW_DPK_GAIN_CHECK && s.gain_only {
            RTW_DPK_AGC_OUT
        } else if st == RTW_DPK_GAIN_CHECK {
            RTW_DPK_LOSS_CHECK
        } else {
            st
        }
    };

    s.agc_cnt += 1;
    if s.agc_cnt >= 6 {
        state = RTW_DPK_AGC_OUT;
    }
    state
}

/// rtw8822c.c:3595-3608 `rtw8822c_gain_large_state`
fn gain_large_state(h: i32, s: &mut DpkData) -> u32 {
    let pga = s.pga;
    if pga > 0xe {
        phy::write_rf_reg_mix(h, s.path, RF_MODE_TRXAGC, BIT_RXAGC, 0xc);
    } else if pga > 0xb && pga < 0xf {
        phy::write_rf_reg_mix(h, s.path, RF_MODE_TRXAGC, BIT_RXAGC, 0x0);
    } else if pga < 0xc {
        s.limited_pga = 1;
    }
    RTW_DPK_GAIN_CHECK
}

/// rtw8822c.c:3610-3623 `rtw8822c_gain_less_state`
fn gain_less_state(h: i32, s: &mut DpkData) -> u32 {
    let pga = s.pga;
    if pga < 0xc {
        phy::write_rf_reg_mix(h, s.path, RF_MODE_TRXAGC, BIT_RXAGC, 0xc);
    } else if pga > 0xb && pga < 0xf {
        phy::write_rf_reg_mix(h, s.path, RF_MODE_TRXAGC, BIT_RXAGC, 0xf);
    } else if pga > 0xe {
        s.limited_pga = 1;
    }
    RTW_DPK_GAIN_CHECK
}

/// rtw8822c.c:3625-3640 `rtw8822c_gl_state`
fn gl_state(h: i32, s: &mut DpkData, is_large: usize) -> u32 {
    const TXBB_BOUND: [u8; 2] = [0x1f, 0];
    if s.txbb == TXBB_BOUND[is_large] {
        return RTW_DPK_AGC_OUT;
    }
    if is_large == 1 {
        s.txbb -= 2;
    } else {
        s.txbb += 3;
    }
    phy::write_rf_reg_mix(h, s.path, RF_TX_GAIN, BIT_GAIN_TXBB, s.txbb as u32);
    s.limited_pga = 0;
    RTW_DPK_GAIN_CHECK
}

/// rtw8822c.c:3664-3675 `rtw8822c_loss_check_state`
fn loss_check_state(h: i32, d: &mut DpkInfo, s: &mut DpkData) -> u32 {
    one_shot(h, d, s.path, RTW_DPK_GAIN_LOSS);
    agc_loss_chk(h, s.path)
}

/// rtw8822c.c:3677-3696 `rtw8822c_dpk_pas_agc`.
///
/// In Linux ist das eine Tabelle von Funktionszeigern (`dpk_state[]`), die
/// sich gegenseitig den naechsten Zustand nennen. Hier steht dieselbe
/// Maschine als `match` — ein Zeigerfeld ueber Funktionen mit Wirkung auf
/// Hardware waere in Rust nur Umstand, und die REIHENFOLGE ist dieselbe.
fn pas_agc(h: i32, d: &mut DpkInfo, path: usize, gain_only: bool,
           loss_only: bool) -> u8 {
    let mut s = DpkData { loss_only, gain_only, path, ..Default::default() };
    let mut state = RTW_DPK_GAIN_CHECK;
    loop {
        state = match state {
            RTW_DPK_GAIN_CHECK => gain_check_state(h, d, &mut s),
            RTW_DPK_GAIN_LARGE => gain_large_state(h, &mut s),
            RTW_DPK_GAIN_LESS => gain_less_state(h, &mut s),
            RTW_DPK_GL_LARGE => gl_state(h, &mut s, 1),
            RTW_DPK_GL_LESS => gl_state(h, &mut s, 0),
            RTW_DPK_LOSS_CHECK => loss_check_state(h, d, &mut s),
            _ => RTW_DPK_AGC_OUT,
        };
        if state == RTW_DPK_AGC_OUT {
            break;
        }
    }
    s.txbb
}

/// rtw8822c.c:3698-3706 `rtw8822c_dpk_coef_iq_check`
fn coef_iq_check(coef_i: u16, coef_q: u16) -> bool {
    coef_i == 0x1000 || coef_i == 0x0fff || coef_q == 0x1000 || coef_q == 0x0fff
}

/// rtw8822c.c:3708-3723 `rtw8822c_dpk_coef_transfer`
fn coef_transfer(h: i32) -> u32 {
    // Linux liest erst das ganze Wort und verwirft es wieder; der Zugriff
    // bleibt, weil er auf diesem Bus eine Wirkung haben kann.
    let _reg = host::r32(h, REG_STAT_RPT);

    let coef_i = host::r32_mask(h, REG_STAT_RPT, MASKHWORD) as u16 & 0x1fff;
    let coef_q = host::r32_mask(h, REG_STAT_RPT, MASKLWORD) as u16 & 0x1fff;
    let coef_q = ((0x2000u16.wrapping_sub(coef_q)) & 0x1fff).wrapping_sub(1);

    ((coef_i as u32) << 16) | coef_q as u32
}

/// rtw8822c.c:3725-3730 `rtw8822c_dpk_get_coef_tbl`
const GET_COEF_TBL: [u32; 20] = [
    0x000400f0, 0x040400f0, 0x080400f0, 0x010400f0, 0x050400f0,
    0x090400f0, 0x020400f0, 0x060400f0, 0x0a0400f0, 0x030400f0,
    0x070400f0, 0x0b0400f0, 0x0c0400f0, 0x100400f0, 0x0d0400f0,
    0x110400f0, 0x0e0400f0, 0x120400f0, 0x0f0400f0, 0x130400f0,
];

/// rtw8822c.c:3732-3742 `rtw8822c_dpk_coef_tbl_apply`
fn coef_tbl_apply(h: i32, d: &mut DpkInfo, path: usize) {
    for i in 0..20 {
        host::w32(h, REG_RXSRAM_CTL, GET_COEF_TBL[i]);
        d.coef[path][i] = coef_transfer(h);
    }
}

/// rtw8822c.c:3744-3757 `rtw8822c_dpk_get_coef`
fn get_coef(h: i32, d: &mut DpkInfo, path: usize) {
    host::w32(h, REG_NCTL0, 0x0000000c);

    if path == phy::RF_PATH_A {
        host::w32_mask(h, REG_DPD_CTL0, 1 << 24, 0x0);
        host::w32(h, REG_DPD_CTL0_S0, 0x30000080);
    } else if path == phy::RF_PATH_B {
        host::w32_mask(h, REG_DPD_CTL0, 1 << 24, 0x1);
        host::w32(h, REG_DPD_CTL0_S1, 0x30000080);
    }

    coef_tbl_apply(h, d, path);
}

/// rtw8822c.c:3759-3775 `rtw8822c_dpk_coef_read`
fn coef_read(d: &DpkInfo, path: usize) -> u8 {
    for addr in 0..20 {
        let coef_i = fget(d.coef[path][addr], 0x1fff0000) as u16;
        let coef_q = fget(d.coef[path][addr], 0x1fff) as u16;
        if coef_iq_check(coef_i, coef_q) {
            return 0;
        }
    }
    1
}

/// rtw8822c.c:3777-3798 `rtw8822c_dpk_coef_write`
fn coef_write(h: i32, d: &DpkInfo, path: usize, result: u8) {
    const REG: [u32; DPK_RF_PATH_NUM_U] = [0x1b0c, 0x1b64];

    host::w32(h, REG_NCTL0, 0x0000000c);
    host::w32(h, REG_RXSRAM_CTL, 0x000000f0);

    for addr in 0..20u32 {
        let coef = if result == 0 {
            if addr == 3 { 0x04001fff } else { 0x00001fff }
        } else {
            d.coef[path][addr as usize]
        };
        host::w32(h, REG[path] + addr * 4, coef);
    }
}

/// rtw8822c.c:3800-3816 `rtw8822c_dpk_fill_result`
fn fill_result(h: i32, d: &mut DpkInfo, dpk_txagc: u32, path: usize,
               result: bool) {
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));

    if result {
        host::w8(h, REG_DPD_AGC, dpk_txagc.wrapping_sub(6) as u8);
    } else {
        host::w8(h, REG_DPD_AGC, 0x00);
    }

    d.result[path] = result as u8;
    d.dpk_txagc[path] = host::r8(h, REG_DPD_AGC);

    coef_write(h, d, path, result as u8);
}

/// rtw8822c.c:3818-3854 `rtw8822c_dpk_gainloss`
fn gainloss(h: i32, d: &mut DpkInfo, path: usize) -> u32 {
    let ori_txbb = rf_setting(h, d, path);
    let ori_txagc = phy::read_rf(h, path, RF_MODE_TRXAGC, BIT_TXAGC) as u8;

    rxbb_dc_cal(h, path);
    one_shot(h, d, path, RTW_DPK_DAGC);
    dgain_read(h);

    if dc_corr_check(h, path) != 0 {
        rxbb_dc_cal(h, path);
        one_shot(h, d, path, RTW_DPK_DAGC);
        dc_corr_check(h, path);
    }

    let t1 = thermal_read(h, path);
    let mut tx_bb = pas_agc(h, d, path, false, true);
    let tx_agc_search = gainloss_result(h, path);

    if tx_bb < tx_agc_search {
        tx_bb = 0;
    } else {
        tx_bb -= tx_agc_search;
    }

    phy::write_rf_reg_mix(h, path, RF_TX_GAIN, BIT_GAIN_TXBB, tx_bb as u32);

    let tx_agc = (ori_txagc as u32).wrapping_sub(ori_txbb.wrapping_sub(tx_bb as u32));

    let t2 = thermal_read(h, path);
    d.thermal_dpk_delta[path] = t2.abs_diff(t1);

    tx_agc
}

/// rtw8822c.c:3856-3871 `rtw8822c_dpk_by_path`
fn by_path(h: i32, d: &mut DpkInfo, _tx_agc: u32, path: usize) -> u8 {
    let mut result = one_shot(h, d, path, RTW_DPK_DO_DPK);

    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));

    result |= host::r32_mask(h, REG_DPD_CTL1_S0, 1 << 26) as u8;

    phy::write_rf_reg_mix(h, path, RF_MODE_TRXAGC, phy::RFREG_MASK, 0x33e14);

    get_coef(h, d, path);

    result
}

/// rtw8822c.c:3873-3941 `rtw8822c_dpk_cal_gs`
fn cal_gs(h: i32, d: &mut DpkInfo, path: usize) {
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));
    host::w32_mask(h, REG_IQK_CTL1, BIT_BYPASS_DPD, 0x0);
    host::w32_mask(h, REG_IQK_CTL1, BIT_TX_CFIR, 0x0);
    host::w32_mask(h, REG_R_CONFIG, BIT_IQ_SWITCH, 0x9);
    host::w32_mask(h, REG_R_CONFIG, BIT_INNER_LB, 0x1);
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0xc);
    host::w32_mask(h, REG_RXSRAM_CTL, BIT_DPD_CLK, 0xf);

    if path == phy::RF_PATH_A {
        host::w32_mask(h, REG_DPD_CTL0_S0, BIT_GS_PWSF, 0x1066680);
        host::w32_mask(h, REG_DPD_CTL1_S0, BIT_DPD_EN, 0x1);
    } else {
        host::w32_mask(h, REG_DPD_CTL0_S1, BIT_GS_PWSF, 0x1066680);
        host::w32_mask(h, REG_DPD_CTL1_S1, BIT_DPD_EN, 0x1);
    }

    if d.dpk_bw as u32 == DPK_CHANNEL_WIDTH_80 {
        host::w32(h, REG_DPD_CTL16, 0x80001310);
        host::w32(h, REG_DPD_CTL16, 0x00001310);
        host::w32(h, REG_DPD_CTL16, 0x810000db);
        host::w32(h, REG_DPD_CTL16, 0x010000db);
        host::w32(h, REG_DPD_CTL16, 0x0000b428);
        host::w32(h, REG_DPD_CTL15, 0x05020000 | ((1u32 << path) << 28));
    } else {
        host::w32(h, REG_DPD_CTL16, 0x8200190c);
        host::w32(h, REG_DPD_CTL16, 0x0200190c);
        host::w32(h, REG_DPD_CTL16, 0x8301ee14);
        host::w32(h, REG_DPD_CTL16, 0x0301ee14);
        host::w32(h, REG_DPD_CTL16, 0x0000b428);
        host::w32(h, REG_DPD_CTL15, 0x05020008 | ((1u32 << path) << 28));
    }

    host::w32_mask(h, REG_DPD_CTL0, MASKBYTE3, 0x8 | path as u32);

    one_shot(h, d, path, RTW_DPK_CAL_PWR);

    host::w32_mask(h, REG_DPD_CTL15, MASKBYTE3, 0x0);
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));
    host::w32_mask(h, REG_R_CONFIG, BIT_IQ_SWITCH, 0x0);
    host::w32_mask(h, REG_R_CONFIG, BIT_INNER_LB, 0x0);
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0xc);

    if path == phy::RF_PATH_A {
        host::w32_mask(h, REG_DPD_CTL0_S0, BIT_GS_PWSF, 0x5b);
    } else {
        host::w32_mask(h, REG_DPD_CTL0_S1, BIT_GS_PWSF, 0x5b);
    }

    host::w32_mask(h, REG_RXSRAM_CTL, BIT_RPT_SEL, 0x0);

    let mut tmp_gs = host::r32_mask(h, REG_STAT_RPT, BIT_RPT_DGAIN);
    tmp_gs = (tmp_gs * 910) >> 10;
    // `DIV_ROUND_CLOSEST(tmp_gs, 10)`
    tmp_gs = tmp_gs.div_ceil(10).min((tmp_gs + 5) / 10);

    if path == phy::RF_PATH_A {
        host::w32_mask(h, REG_DPD_CTL0_S0, BIT_GS_PWSF, tmp_gs);
    } else {
        host::w32_mask(h, REG_DPD_CTL0_S1, BIT_GS_PWSF, tmp_gs);
    }

    d.dpk_gs[path] = tmp_gs as u16;
}

/// rtw8822c.c:3943-3975 `rtw8822c_dpk_cal_coef1`
fn cal_coef1(h: i32, d: &DpkInfo, rf_path_num: u8) -> bool {
    const OFFSET: [u32; DPK_RF_PATH_NUM_U] = [0, 0x58];

    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x0000000c);
    host::w32(h, REG_RXSRAM_CTL, 0x000000f0);
    host::w32(h, REG_NCTL0, 0x00001148);
    host::w32(h, REG_NCTL0, 0x00001149);

    let ready = check_hw_ready(h, 0x2d9c, MASKBYTE0, 0x55);
    if !ready {
        host::print("[rtl8822ce] DPK stuck, performance may be suboptimal\n");
    }

    host::w8(h, 0x1b10, 0x0);
    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x0000000c);

    for path in 0..rf_path_num as usize {
        // Linux teilt hier ohne Pruefung; `dpk_gs` ist nach `cal_gs` nie
        // null, und wenn doch, waere eine Division durch null bei uns ein
        // Trap statt eines Unsinnswerts.
        let gs = d.dpk_gs[path];
        let i_scaling = if gs == 0 { 0 } else { 0x16c00 / gs as u32 };

        host::w32_mask(h, 0x1b18 + OFFSET[path], MASKHWORD, i_scaling);
        host::w32_mask(h, REG_DPD_CTL0_S0 + OFFSET[path], 0xf000_0000, 0x9);
        host::w32_mask(h, REG_DPD_CTL0_S0 + OFFSET[path], 0xf000_0000, 0x1);
        host::w32_mask(h, REG_DPD_CTL0_S0 + OFFSET[path], 0xf000_0000, 0x0);
        host::w32_mask(h, REG_DPD_CTL1_S0 + OFFSET[path], 1 << 14, 0x0);
    }
    ready
}

/// rtw8822c.c:3977-3988 `rtw8822c_dpk_on`
fn dpk_on(h: i32, d: &mut DpkInfo, path: usize) {
    one_shot(h, d, path, RTW_DPK_DPK_ON);

    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));
    host::w32_mask(h, REG_IQK_CTL1, BIT_TX_CFIR, 0x0);

    if d.path_ok(path) {
        cal_gs(h, d, path);
    }
}

/// rtw8822c.c:3990-4007 `rtw8822c_dpk_check_pass`
fn check_pass(h: i32, d: &mut DpkInfo, is_fail: bool, dpk_txagc: u32,
              path: usize) -> bool {
    let result = if !is_fail { coef_read(d, path) != 0 } else { false };
    fill_result(h, d, dpk_txagc, path, result);
    result
}

/// rtw8822c.c:4009-4027 `rtw8822c_dpk_result_reset`
fn result_reset(h: i32, d: &mut DpkInfo, rf_path_num: u8) {
    for path in 0..rf_path_num as usize {
        d.clear_path_ok(path);
        host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));
        host::w32_mask(h, 0x1b58, 0x0000007f, 0x0);

        d.dpk_txagc[path] = 0;
        d.result[path] = 0;
        d.dpk_gs[path] = 0x5b;
        d.pre_pwsf[path] = 0;
        if path < DPK_RF_PATH_NUM_U {
            d.thermal_dpk[path] = thermal_read(h, path);
        }
    }
}

/// rtw8822c.c:4029-4048 `rtw8822c_dpk_calibrate`
fn calibrate(h: i32, d: &mut DpkInfo, path: usize) -> bool {
    let dpk_txagc = gainloss(h, d, path);
    let dpk_fail = by_path(h, d, dpk_txagc, path);
    let passed = check_pass(h, d, dpk_fail != 0, dpk_txagc, path);
    if !passed {
        host::print("[rtl8822ce] failed to do dpk calibration\n");
    }
    if d.result[path] != 0 {
        d.set_path_ok(path);
    }
    passed
}

/// rtw8822c.c:4050-4057 `rtw8822c_dpk_path_select`
fn path_select(h: i32, d: &mut DpkInfo, rf_path_num: u8) -> bool {
    calibrate(h, d, phy::RF_PATH_A);
    calibrate(h, d, phy::RF_PATH_B);
    dpk_on(h, d, phy::RF_PATH_A);
    dpk_on(h, d, phy::RF_PATH_B);
    cal_coef1(h, d, rf_path_num)
}

/// rtw8822c.c:4059-4079 `rtw8822c_dpk_enable_disable`
fn enable_disable(h: i32, d: &DpkInfo) {
    let mask = (1u32 << 15) | (1 << 14);

    host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0xc);
    host::w32_mask(h, REG_DPD_CTL1_S0, BIT_DPD_EN, d.is_dpk_pwr_on as u32);
    host::w32_mask(h, REG_DPD_CTL1_S1, BIT_DPD_EN, d.is_dpk_pwr_on as u32);

    if d.path_ok(phy::RF_PATH_A) {
        host::w32_mask(h, REG_DPD_CTL1_S0, mask, 0x0);
        host::w8(h, REG_DPD_CTL0_S0, d.dpk_gs[phy::RF_PATH_A] as u8);
    }
    if d.path_ok(phy::RF_PATH_B) {
        host::w32_mask(h, REG_DPD_CTL1_S1, mask, 0x0);
        host::w8(h, REG_DPD_CTL0_S1, d.dpk_gs[phy::RF_PATH_B] as u8);
    }
}

/// rtw8822c.c:4081-4116 `rtw8822c_dpk_reload_data`
fn reload_data(h: i32, d: &mut DpkInfo, rf_path_num: u8) {
    if !d.path_ok(phy::RF_PATH_A) && !d.path_ok(phy::RF_PATH_B)
        && d.dpk_ch == 0
    {
        return;
    }

    for path in 0..rf_path_num as usize {
        host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));
        if d.dpk_band as u32 == RTW_BAND_2G_MASK {
            host::w32(h, REG_DPD_CTL1_S1, 0x1f100000);
        } else {
            host::w32(h, REG_DPD_CTL1_S1, 0x1f0d0000);
        }

        host::w8(h, REG_DPD_AGC, d.dpk_txagc[path]);
        coef_write(h, d, path, d.path_ok(path) as u8);
        one_shot(h, d, path, RTW_DPK_DPK_ON);
        host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0xc);

        if path == phy::RF_PATH_A {
            host::w32_mask(h, REG_DPD_CTL0_S0, BIT_GS_PWSF,
                           d.dpk_gs[path] as u32);
        } else {
            host::w32_mask(h, REG_DPD_CTL0_S1, BIT_GS_PWSF,
                           d.dpk_gs[path] as u32);
        }
    }
    cal_coef1(h, d, rf_path_num);
}

/// rtw8822c.c:4118-4135 `rtw8822c_dpk_reload`
fn reload(h: i32, d: &mut DpkInfo, rf_path_num: u8) -> bool {
    d.is_reload = false;
    let channel = (phy::read_rf(h, phy::RF_PATH_A, 0x18, phy::RFREG_MASK)
                   & 0xff) as u8;
    if channel == d.dpk_ch {
        reload_data(h, d, rf_path_num);
        d.is_reload = true;
    }
    d.is_reload
}

/// Warum DPK nicht gelaufen ist — oder wie es ausging.
#[derive(Clone, Copy, PartialEq)]
pub enum DpkRpt {
    Ran { path_ok: u8, gs: [u16; 2], txagc: [u8; 2], coef1_ready: bool },
    PwrOff,
    Reloaded,
}

/// rtw8822c.c:4137-4186 `rtw8822c_do_dpk`
pub fn do_dpk(h: i32, d: &mut DpkInfo, rf_path_num: u8) -> DpkRpt {
    const BB_REG: [u32; DPK_BB_REG_NUM_U] = [
        0x520, 0x820, 0x824, 0x1c3c, 0x1d58, 0x1864,
        0x4164, 0x180c, 0x410c, 0x186c, 0x416c,
        0x1a14, 0x1e70, 0x80c, 0x1d70, 0x1e7c, 0x18a4, 0x41a4];
    const RF_REG: [u32; DPK_RF_REG_NUM_U] =
        [0x0, 0x1a, 0x55, 0x63, 0x87, 0x8f, 0xde];

    if !d.is_dpk_pwr_on {
        return DpkRpt::PwrOff;
    }
    if reload(h, d, rf_path_num) {
        return DpkRpt::Reloaded;
    }

    // `ewma_thermal_init` fuellt den gleitenden Mittelwert fuer
    // `rtw8822c_dpk_track`; die Nachfuehrung gibt es noch nicht.

    information(h, d);

    let mut bckp = [Backup::default(); DPK_BB_REG_NUM_U];
    let mut rf_bak = [[0u32; 2]; DPK_RF_REG_NUM_U];
    backup_registers(h, &BB_REG, &mut bckp);
    backup_rf_registers(h, &RF_REG, &mut rf_bak);

    mac_bb_setting(h);
    afe_setting(h, true);
    pre_setting(h, d, rf_path_num);
    result_reset(h, d, rf_path_num);
    let coef1_ready = path_select(h, d, rf_path_num);
    afe_setting(h, false);
    enable_disable(h, d);

    reload_rf_registers(h, &RF_REG, &rf_bak);
    for path in 0..rf_path_num as usize {
        rxbb_dc_cal(h, path);
    }
    restore_registers(h, &bckp);

    DpkRpt::Ran {
        path_ok: d.dpk_path_ok,
        gs: [d.dpk_gs[0], d.dpk_gs[1]],
        txagc: [d.dpk_txagc[0], d.dpk_txagc[1]],
        coef1_ready,
    }
}

/// rtw8822c.c:3629-3660 `rtw8822c_dpk_track`.
///
/// **Die Nachfuehrung der Vorverzerrung ueber die Temperatur.** Sie
/// laeuft nur, wenn eine DPK-Kalibrierung ueberhaupt stattgefunden hat
/// (`thermal_dpk` beider Pfade null heisst: es gibt nichts
/// nachzufuehren).
pub fn track(h: i32, d: &mut DpkInfo) {
    if d.thermal_dpk[0] == 0 && d.thermal_dpk[1] == 0 {
        return;
    }

    for path in 0..DPK_RF_PATH_NUM_U {
        let raw = thermal_read(h, path);
        d.avg_thermal[path].add(raw as u32, crate::dm::EWMA_THERMAL_PRECISION,
                                crate::dm::EWMA_THERMAL_WEIGHT_RCP);
        let thermal_value =
            d.avg_thermal[path].read(crate::dm::EWMA_THERMAL_PRECISION) as u8;
        // Linux rechnet beides in `s8`, und der Umlauf ist gewollt: die
        // Maske 0x7f darunter schneidet ohnehin auf sieben Bit.
        let delta_dpk = (d.thermal_dpk[path] as i8)
            .wrapping_sub(thermal_value as i8);
        let offset = delta_dpk
            .wrapping_sub(d.thermal_dpk_delta[path] as i8) & 0x7f;

        if offset as u8 != d.pre_pwsf[path] {
            host::w32_mask(h, REG_NCTL0, BIT_SUBPAGE, 0x8 | ((path as u32) << 1));
            host::w32_mask(h, 0x1b58, 0x7f, offset as u32);
            d.pre_pwsf[path] = offset as u8;
        }
    }
}
