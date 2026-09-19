//! `efuse.c` aus Linux 6.18.26 rtw88, plus `rtw8822c_read_efuse` und
//! `rtw_dump_hw_feature` — Stufe 2c.
//!
//! Portiert: `switch_efuse_bank` · `rtw_dump_physical_efuse_map` ·
//! `rtw_dump_logical_efuse_map` · `rtw_parse_efuse_map` ·
//! `rtw8822c_read_efuse` · `rtw8822ce_efuse_parsing` · `rtw_dump_hw_feature` ·
//! `rtw_check_supported_rfe` · `rtw8822c_cfg_ldo25`.
#![allow(dead_code)]

use crate::host;
use crate::regs::*;

// efuse.c:12
const RTW_EFUSE_BANK_WIFI: u32 = 0x0;

/// rtw8822c.c `rtw8822c_hw_spec`
pub const PHYSICAL_SIZE: usize = 512;
pub const LOGICAL_SIZE: usize = 768;
pub const PROTECT_SIZE: usize = 124;

// ── Feldlagen in `struct rtw8822c_efuse` (rtw8822c.h) ────────────
//
// Die Struktur ist `__packed`, die Offsets also die Summe der Felder davor.
// Linux kommentiert DREI davon ausdruecklich (0xb8, 0xc9, 0xd0, 0x120) —
// die dienen hier als Anker, gegen die die Rechnung geprueft wird.
//
//   rtl_id 2 + res0 4 + usb_mode 1 + res1 9            = 0x10
//   txpwr_idx_table[4], je 42 Byte                     = 0xa8
//   -> channel_plan bei 0x10 + 0xa8                    = 0xb8  ✓ Anker
const OFF_USB_MODE: usize = 0x06;
const OFF_TXPWR_IDX_TABLE: usize = 0x10;
const TXPWR_IDX_SIZE: usize = 42;
const OFF_CHANNEL_PLAN: usize = 0xb8;
const OFF_XTAL_K: usize = 0xb9;
const OFF_IQK_LCK: usize = 0xbb;
const OFF_RF_BOARD_OPTION: usize = 0xc1;
const OFF_RF_FEATURE_OPTION: usize = 0xc2;
const OFF_RF_BT_SETTING: usize = 0xc3;
const OFF_EEPROM_VERSION: usize = 0xc4;
const OFF_EEPROM_CUSTOMER_ID: usize = 0xc5;
const OFF_TX_BB_SWING_2G: usize = 0xc6;
const OFF_TX_BB_SWING_5G: usize = 0xc7;
const OFF_TX_PWR_CALIBRATE_RATE: usize = 0xc8;
const OFF_RF_ANTENNA_OPTION: usize = 0xc9;
const OFF_RFE_OPTION: usize = 0xca;
const OFF_COUNTRY_CODE: usize = 0xcb;
const OFF_PATH_A_THERMAL: usize = 0xd0;
const OFF_PATH_B_THERMAL: usize = 0xd1;
/// `struct rtw8822ce_efuse.mac_addr` — der vierte Anker.
const OFF_MAC_ADDR: usize = 0x120;

// Die Anker aus der Linux-Quelle, zur Bauzeit geprueft. Stimmt die
// Rechnung nicht mehr, baut es nicht — statt still die falschen Bytes
// zu lesen.
const _: () = assert!(OFF_TXPWR_IDX_TABLE + 4 * TXPWR_IDX_SIZE == OFF_CHANNEL_PLAN);
const _: () = assert!(OFF_RF_ANTENNA_OPTION == 0xc9);
const _: () = assert!(OFF_PATH_A_THERMAL == 0xd0);
const _: () = assert!(OFF_MAC_ADDR == 0x120);

/// rtw8822c.h:183
const XCAP_MASK: u8 = 0x7f;

/// `struct rtw_efuse`, nur die Felder, die `rtw8822c_read_efuse` setzt.
#[derive(Default)]
pub struct Efuse {
    pub addr: [u8; 6],
    pub usb_mode_switch: u8,
    pub rfe_option: u8,
    pub rf_board_option: u8,
    pub crystal_cap: u8,
    pub channel_plan: u8,
    pub country_code: [u8; 2],
    pub bt_setting: u8,
    pub regd: u8,
    pub thermal_meter: [u8; 2],
    pub thermal_meter_k: u8,
    pub power_track_type: u8,
    pub share_ant: bool,
    pub btcoex: bool,
    // `rtw8822c_read_efuse` setzt pa_type/lna_type NICHT — sie bleiben 0,
    // damit bleiben auch die vier ext_*-Fahnen 0. Das ist Linux, nicht
    // Bequemlichkeit.
    pub ext_pa_2g: u8,
    pub ext_lna_2g: u8,
    pub ext_pa_5g: u8,
    pub ext_lna_5g: u8,
    // aus rtw_dump_hw_feature
    pub hw_cap_hci: u8,
    pub hw_cap_bw: u8,
    pub hw_cap_nss: u8,
    pub hw_cap_ptcl: u8,
    pub hw_cap_ant_num: u8,
}

/// rtw8822c.c `rtw8822c_cfg_ldo25`
fn cfg_ldo25(h: i32, enable: bool) {
    let mut ldo_pwr = host::r8(h, REG_ANAPARLDO_POW_MAC);
    ldo_pwr = if enable {
        ldo_pwr | BIT_LDOE25_PON
    } else {
        ldo_pwr & !BIT_LDOE25_PON
    };
    host::w8(h, REG_ANAPARLDO_POW_MAC, ldo_pwr);
}

/// efuse.c `switch_efuse_bank`
fn switch_efuse_bank(h: i32) {
    let shift = BIT_MASK_EFUSE_BANK_SEL.trailing_zeros();
    let mut v = host::r32(h, REG_LDO_EFUSE_CTRL);
    v &= !BIT_MASK_EFUSE_BANK_SEL;
    v |= (RTW_EFUSE_BANK_WIFI << shift) & BIT_MASK_EFUSE_BANK_SEL;
    host::w32(h, REG_LDO_EFUSE_CTRL, v);
}

/// efuse.c `rtw_dump_physical_efuse_map`.
///
/// `rtw_chip_efuse_grant_on/off` sind fuer den 8822C ohne Wirkung — seine
/// `rtw_chip_ops` fuehren kein `efuse_grant`, und der Wrapper prueft das.
///
/// Linux pollt je Byte mit `udelay(1)` bis zu 1000000-mal, die Frist ist
/// also EINE SEKUNDE je Byte. Gehalten wird hier die Frist, nicht die
/// Rundenzahl — derselbe Fehler wie in `check_hw_ready` soll sich nicht
/// wiederholen.
fn dump_physical_efuse_map(h: i32, map: &mut [u8]) -> bool {
    switch_efuse_bank(h);

    // disable 2.5V LDO
    cfg_ldo25(h, false);

    let mut efuse_ctl = host::r32(h, REG_EFUSE_CTRL);

    for addr in 0..map.len() as u32 {
        efuse_ctl &= !(BIT_MASK_EF_DATA | BITS_EF_ADDR);
        efuse_ctl |= (addr & BIT_MASK_EF_ADDR) << BIT_SHIFT_EF_ADDR;
        host::w32(h, REG_EFUSE_CTRL, efuse_ctl & !BIT_EF_FLAG);

        let start = host::now_us();
        loop {
            efuse_ctl = host::r32(h, REG_EFUSE_CTRL);
            if efuse_ctl & BIT_EF_FLAG != 0 {
                break;
            }
            if host::now_us() - start >= 1_000_000 {
                host::print("[rtl8822ce] efuse haengt bei Adresse ");
                host::print_dec(addr);
                host::print("\n");
                return false;
            }
        }
        map[addr as usize] = (efuse_ctl & BIT_MASK_EF_DATA) as u8;
    }
    true
}

// efuse.c:20-29 — die vier Kopfmakros, Zeichen fuer Zeichen.
#[inline]
fn invalid_efuse_header(hdr1: u8, hdr2: u8) -> bool {
    hdr1 == 0xff || ((hdr1 & 0x1f) == 0xf && hdr2 == 0xff)
}
#[inline]
fn invalid_efuse_content(word_en: u8, i: u8) -> bool {
    word_en & (1 << i) != 0
}
#[inline]
fn get_efuse_blk_idx_2_byte(hdr1: u8, hdr2: u8) -> u8 {
    ((hdr2 & 0xf0) >> 1) | ((hdr1 >> 5) & 0x07)
}
#[inline]
fn get_efuse_blk_idx_1_byte(hdr1: u8) -> u8 {
    (hdr1 & 0xf0) >> 4
}
#[inline]
fn block_idx_to_logical_idx(blk_idx: u8, i: u8) -> usize {
    ((blk_idx as usize) << 3) + ((i as usize) << 1)
}

/// efuse.c `rtw_dump_logical_efuse_map`
fn dump_logical_efuse_map(phy_map: &[u8], log_map: &mut [u8]) -> bool {
    let physical_size = phy_map.len();
    let logical_size = log_map.len();
    let limit = physical_size - PROTECT_SIZE;

    let mut phy_idx = 0usize;
    while phy_idx < limit {
        let hdr1 = phy_map[phy_idx];
        let hdr2 = if phy_idx + 1 < physical_size { phy_map[phy_idx + 1] } else { 0xff };
        if invalid_efuse_header(hdr1, hdr2) {
            break;
        }

        let blk_idx;
        let word_en;
        if (hdr1 & 0x1f) == 0xf {
            blk_idx = get_efuse_blk_idx_2_byte(hdr1, hdr2);
            word_en = hdr2 & 0xf;
            phy_idx += 2;
        } else {
            blk_idx = get_efuse_blk_idx_1_byte(hdr1);
            word_en = hdr1 & 0xf;
            phy_idx += 1;
        }

        for i in 0..4u8 {
            if invalid_efuse_content(word_en, i) {
                continue;
            }
            let log_idx = block_idx_to_logical_idx(blk_idx, i);
            if phy_idx + 1 > limit || log_idx + 1 > logical_size {
                return false;
            }
            log_map[log_idx] = phy_map[phy_idx];
            log_map[log_idx + 1] = phy_map[phy_idx + 1];
            phy_idx += 2;
        }
    }
    true
}

/// rtw8822c.c `rtw8822c_read_efuse` + `rtw8822ce_efuse_parsing`
fn read_efuse(log_map: &[u8]) -> Efuse {
    let mut e = Efuse {
        usb_mode_switch: (log_map[OFF_USB_MODE] >> 7) & 1,
        rfe_option: log_map[OFF_RFE_OPTION],
        rf_board_option: log_map[OFF_RF_BOARD_OPTION],
        crystal_cap: log_map[OFF_XTAL_K] & XCAP_MASK,
        channel_plan: log_map[OFF_CHANNEL_PLAN],
        country_code: [log_map[OFF_COUNTRY_CODE], log_map[OFF_COUNTRY_CODE + 1]],
        bt_setting: log_map[OFF_RF_BT_SETTING],
        regd: log_map[OFF_RF_BOARD_OPTION] & 0x7,
        thermal_meter: [log_map[OFF_PATH_A_THERMAL], log_map[OFF_PATH_B_THERMAL]],
        thermal_meter_k: ((log_map[OFF_PATH_A_THERMAL] as u16
            + log_map[OFF_PATH_B_THERMAL] as u16) >> 1) as u8,
        power_track_type: (log_map[OFF_TX_PWR_CALIBRATE_RATE] >> 4) & 0xf,
        ..Default::default()
    };
    // rtw8822ce_efuse_parsing: ether_addr_copy(efuse->addr, map->e.mac_addr)
    e.addr.copy_from_slice(&log_map[OFF_MAC_ADDR..OFF_MAC_ADDR + 6]);
    e
}

/// main.c `rtw_dump_hw_feature`. `hw_feature_report` ist beim 8822C true.
///
/// Der Ausloeser dafuer steht in `rtw_chip_efuse_enable` und ist ein
/// einzelner Registerschreiber VOR dem Firmware-Download:
/// `rtw_write8(REG_C2HEVT, C2H_HW_FEATURE_DUMP)`.
fn dump_hw_feature(h: i32, e: &mut Efuse, rf_path_num: u8) -> bool {
    let id = host::r8(h, REG_C2HEVT);
    if id != C2H_HW_FEATURE_REPORT {
        host::print("  C2HEVT = 0x");
        host::print_hex8(id);
        host::print(" statt 0x19 — kein hw feature report\n");
        return false;
    }

    let mut hw_feature = [0u8; HW_FEATURE_LEN];
    for (i, b) in hw_feature.iter_mut().enumerate() {
        *b = host::r8(h, REG_C2HEVT + 2 + i as u32);
    }
    host::w8(h, REG_C2HEVT, 0);

    // efuse.h:15-24 — alle Felder aus dem ZWEITEN 32-Bit-Wort.
    let w1 = u32::from_le_bytes([hw_feature[4], hw_feature[5],
                                 hw_feature[6], hw_feature[7]]);
    let bw = ((w1 >> 16) & 0x7) as u8;
    e.hw_cap_bw = hw_bw_cap_to_bitamp(bw);
    e.hw_cap_hci = (w1 & 0xf) as u8;
    e.hw_cap_nss = ((w1 >> 19) & 0x3) as u8;
    e.hw_cap_ant_num = ((w1 >> 21) & 0x7) as u8;
    e.hw_cap_ptcl = ((w1 >> 26) & 0x3) as u8;

    if e.hw_cap_nss == EFUSE_HW_CAP_IGNORE || e.hw_cap_nss > rf_path_num {
        e.hw_cap_nss = rf_path_num;
    }
    true
}

/// main.c `hw_bw_cap_to_bitamp`
fn hw_bw_cap_to_bitamp(bw_cap: u8) -> u8 {
    // BIT(RTW_CHANNEL_WIDTH_20) ist immer dabei; 40 und 80 kommen dazu.
    let mut bw = 1 << 0;
    match bw_cap {
        EFUSE_HW_CAP_IGNORE | EFUSE_HW_CAP_SUPP_BW80 => bw |= (1 << 2) | (1 << 1),
        EFUSE_HW_CAP_SUPP_BW40 => bw |= 1 << 1,
        _ => {}
    }
    bw
}

/// phy.h `rtw_check_supported_rfe`. Fuer den 8822C fuehren alle sieben
/// `rfe_defs` gueltige Tabellen, die Pruefung ist also die Reichweite.
fn check_supported_rfe(rfe_option: u8) -> bool {
    (rfe_option as usize) < RTW8822C_RFE_DEFS_SIZE
}

/// rtw8822c.c `rtw8822c_rfe_defs` hat sieben Eintraege.
const RTW8822C_RFE_DEFS_SIZE: usize = 7;

/// main.c `rtw_chip_efuse_info_setup`, ab `rtw_parse_efuse_map`.
///
/// Der Chip muss dafuer AN sein und die Firmware laufen — `hw_feature`
/// kommt von ihr.
pub fn efuse_info_setup(h: i32, rf_path_num: u8) -> Option<Efuse> {
    let mut phy_map = [0u8; PHYSICAL_SIZE];
    // Linux: `memset(log_map, 0xff, log_size)` VOR dem Auspacken — was der
    // Kopf nicht beschreibt, bleibt 0xff und faellt damit in die
    // Vorgabe-Zweige weiter unten.
    let mut log_map = [0xffu8; LOGICAL_SIZE];

    let t0 = host::now_us();
    if !dump_physical_efuse_map(h, &mut phy_map) {
        return None;
    }
    let t_phys = host::now_us() - t0;

    if !dump_logical_efuse_map(&phy_map, &mut log_map) {
        host::print("[rtl8822ce] logische efuse-Karte ungueltig\n");
        return None;
    }

    let mut e = read_efuse(&log_map);

    let hw_ok = dump_hw_feature(h, &mut e, rf_path_num);

    if !check_supported_rfe(e.rfe_option) {
        host::print("[rtl8822ce] rfe ");
        host::print_dec(e.rfe_option as u32);
        host::print(" wird nicht unterstuetzt\n");
        return None;
    }

    // main.c:2024-2046 — die Vorgaben und die abgeleiteten Fahnen.
    if e.crystal_cap == 0xff { e.crystal_cap = 0; }
    if e.channel_plan == 0xff { e.channel_plan = 0x7f; }
    if e.rf_board_option == 0xff { e.rf_board_option = 0; }
    if e.bt_setting & 1 != 0 { e.share_ant = true; }
    if e.regd == 0xff { e.regd = 0; }
    e.btcoex = (e.rf_board_option & 0xe0) == 0x20;

    host::print("  efuse gelesen in ");
    host::print_dec((t_phys / 1000) as u32);
    host::print(" ms");
    if !hw_ok {
        host::print("  (hw_feature FEHLT)");
    }
    host::print("\n");
    Some(e)
}
