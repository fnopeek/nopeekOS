//! `phy.c` from Linux 6.18.26 rtw88: TX power.
//!
//! In Linux `rtw_chip_board_info_setup` (main.c:2064) runs at probe time,
//! right after `rtw_chip_efuse_info_setup` and well before `rtw_power_on`.
//! It writes no register; it only fills the tables in `hal` from which
//! `rtw_set_channel` later computes TX power per channel, rate and path.
//!
//! Ported: `rtw_phy_init_tx_power` · `rtw_phy_init_tx_power_limit` ·
//! `bcd_to_dec_pwr_by_rate` · `tbl_to_dec_pwr_by_rate` ·
//! `rtw_phy_get_rate_values_of_txpwr_by_rate` · `rtw_phy_store_tx_power_by_rate` ·
//! `rtw_parse_tbl_bb_pg` · `rtw_channel_to_idx` · `rtw_phy_set_tx_power_limit` ·
//! `rtw_regd_has_alt` · `__cfg_txpwr_lmt_by_alt` · `rtw_cfg_txpwr_lmt_by_alt` ·
//! `rtw_xref_5g_txpwr_lmt` · `rtw_xref_txpwr_lmt_by_rs` ·
//! `rtw_xref_5g_txpwr_lmt_by_ch` · `rtw_xref_txpwr_lmt_by_bw` ·
//! `rtw_xref_txpwr_lmt` · `rtw_parse_tbl_txpwr_lmt` ·
//! `rtw_phy_tx_power_by_rate_config_by_path` · `rtw_phy_tx_power_by_rate_config` ·
//! `__rtw_phy_tx_power_limit_config` · `rtw_phy_tx_power_limit_config`.
#![allow(dead_code)]

use crate::host;
use crate::regs::*;
use crate::tables;

// ── Sizes from main.h ────────────────────────────────────────────
pub const RTW_RF_PATH_MAX: usize = 4; // main.h:38
pub const DESC_RATE_MAX: usize = 0x54; // main.h:341, follows 0x53
pub const RTW_RATE_SECTION_NUM: usize = 10; // main.h:176
pub const RTW_REGD_MAX: usize = 13; // main.h:359, follows RTW_REGD_WW
pub const RTW_REGD_WW: usize = 12; // main.h:358
pub const RTW_CHANNEL_WIDTH_MAX: usize = 3; // main.h:37
pub const RTW_MAX_CHANNEL_NUM_2G: usize = 14; // main.h:49
pub const RTW_MAX_CHANNEL_NUM_5G: usize = 49; // main.h:50
pub const PHY_BAND_2G: u8 = 0; // main.h, enum rtw_phy_band_type
pub const PHY_BAND_5G: u8 = 1;

/// rtw8822c.c:5351 `.max_power_index = 0x7f`
pub const MAX_POWER_INDEX: i8 = 0x7f;

/// regd.c:522-533 `rtw_regd_alt[]`: which regulatory domain copies from
/// which when the table has nothing for it. `None` means no alternative,
/// and WW applies.
const REGD_ALT: [Option<usize>; RTW_REGD_MAX] = [
    None,       // FCC
    None,       // MKK
    None,       // ETSI
    Some(0),    // IC      -> FCC
    Some(2),    // KCC     -> ETSI
    Some(2),    // ACMA    -> ETSI
    Some(0),    // CHILE   -> FCC
    Some(2),    // UKRAINE -> ETSI
    Some(0),    // MEXICO  -> FCC
    Some(2),    // CN      -> ETSI
    Some(2),    // QATAR   -> ETSI
    Some(2),    // UK      -> ETSI
    None,       // WW
];

/// main.h:1993-2011 `struct rtw_hal`, the TX power part.
///
/// About 25 KiB: `tx_pwr_limit_5g` alone is 13 domains x 3 bandwidths x
/// 10 rate sections x 49 channels.
pub struct TxPower {
    pub by_rate_offset_2g: [[i8; DESC_RATE_MAX]; RTW_RF_PATH_MAX],
    pub by_rate_offset_5g: [[i8; DESC_RATE_MAX]; RTW_RF_PATH_MAX],
    pub by_rate_base_2g: [[i8; RTW_RATE_SECTION_NUM]; RTW_RF_PATH_MAX],
    pub by_rate_base_5g: [[i8; RTW_RATE_SECTION_NUM]; RTW_RF_PATH_MAX],
    pub limit_2g: [[[[i8; RTW_MAX_CHANNEL_NUM_2G]; RTW_RATE_SECTION_NUM];
                    RTW_CHANNEL_WIDTH_MAX]; RTW_REGD_MAX],
    pub limit_5g: [[[[i8; RTW_MAX_CHANNEL_NUM_5G]; RTW_RATE_SECTION_NUM];
                    RTW_CHANNEL_WIDTH_MAX]; RTW_REGD_MAX],
    pub cch_by_bw: [u8; RTW_CHANNEL_WIDTH_MAX],
}

impl TxPower {
    /// phy.c:2427-2446 `rtw_phy_init_tx_power`, including
    /// `rtw_phy_init_tx_power_limit`, which only sets every cell to
    /// `max_power_index`. The zeroing before it comes from construction.
    pub fn new() -> Self {
        let mut t = TxPower {
            by_rate_offset_2g: [[0; DESC_RATE_MAX]; RTW_RF_PATH_MAX],
            by_rate_offset_5g: [[0; DESC_RATE_MAX]; RTW_RF_PATH_MAX],
            by_rate_base_2g: [[0; RTW_RATE_SECTION_NUM]; RTW_RF_PATH_MAX],
            by_rate_base_5g: [[0; RTW_RATE_SECTION_NUM]; RTW_RF_PATH_MAX],
            limit_2g: [[[[0; RTW_MAX_CHANNEL_NUM_2G]; RTW_RATE_SECTION_NUM];
                        RTW_CHANNEL_WIDTH_MAX]; RTW_REGD_MAX],
            limit_5g: [[[[0; RTW_MAX_CHANNEL_NUM_5G]; RTW_RATE_SECTION_NUM];
                        RTW_CHANNEL_WIDTH_MAX]; RTW_REGD_MAX],
            cch_by_bw: [0; RTW_CHANNEL_WIDTH_MAX],
        };
        for regd in 0..RTW_REGD_MAX {
            for bw in 0..RTW_CHANNEL_WIDTH_MAX {
                for rs in 0..RTW_RATE_SECTION_NUM {
                    // phy.c:2411-2425 `rtw_phy_init_tx_power_limit`
                    for ch in 0..RTW_MAX_CHANNEL_NUM_2G {
                        t.limit_2g[regd][bw][rs][ch] = MAX_POWER_INDEX;
                    }
                    for ch in 0..RTW_MAX_CHANNEL_NUM_5G {
                        t.limit_5g[regd][bw][rs][ch] = MAX_POWER_INDEX;
                    }
                }
            }
        }
        t
    }
}

// ── Power per rate (bb_pg) ───────────────────────────────────────

/// phy.c:1219 `#define bcd_to_dec_pwr_by_rate(val, i) bcd2bin(val >> (i * 8))`
///
/// `bcd2bin(x)` is `(x & 0x0f) + (x >> 4) * 10`: a number whose two
/// nibbles are decimal digits.
fn bcd_to_dec_pwr_by_rate(val: u32, i: u32) -> i8 {
    let b = ((val >> (i * 8)) & 0xff) as u8;
    ((b & 0x0f) + (b >> 4) * 10) as i8
}

/// phy.c:1221-1227 `tbl_to_dec_pwr_by_rate`.
///
/// `chip->is_pwr_by_rate_dec` is false on the 8822C (rtw8822c.c:5350), so
/// the simple branch applies: the byte as is.
fn tbl_to_dec_pwr_by_rate(hex: u32, i: u32) -> i8 {
    ((hex >> (i * 8)) & 0xff) as i8
}

/// phy.c:1229-1532 `rtw_phy_get_rate_values_of_txpwr_by_rate`.
///
/// The mapping lives in `tables::TXPWR_BY_RATE_MAP`, generated from this
/// `switch`. Only the two cases that compute rather than map are here.
///
/// Returns `(rates, values, count)`.
fn rate_values_of_txpwr_by_rate(addr: u32, mask: u32, val: u32)
    -> ([u8; 4], [i8; 4], usize)
{
    let mut rate = [0u8; 4];
    let mut pwr = [0i8; 4];

    // phy.c:1259 `case 0xE08:`: one value, from BCD, byte 1.
    if addr == 0xE08 {
        rate[0] = 0x00; // DESC_RATE1M
        pwr[0] = bcd_to_dec_pwr_by_rate(val, 1);
        return (rate, pwr, 1);
    }

    // phy.c:1264-1277 `case 0x86C:`: depends on the mask.
    if addr == 0x86C {
        if mask == 0xffffff00 {
            rate[0] = 0x01; // DESC_RATE2M
            rate[1] = 0x02; // DESC_RATE5_5M
            rate[2] = 0x03; // DESC_RATE11M
            for i in 1..4u32 {
                pwr[(i - 1) as usize] = tbl_to_dec_pwr_by_rate(val, i);
            }
            return (rate, pwr, 3);
        } else if mask == 0x000000ff {
            rate[0] = 0x03; // DESC_RATE11M
            pwr[0] = bcd_to_dec_pwr_by_rate(val, 0);
            return (rate, pwr, 1);
        }
        // Linux exits here without setting `rate_num`, so it is 0.
        return (rate, pwr, 0);
    }

    for &(a, rs) in tables::TXPWR_BY_RATE_MAP.iter() {
        if a != addr {
            continue;
        }
        for (i, &r) in rs.iter().enumerate() {
            rate[i] = r;
            pwr[i] = tbl_to_dec_pwr_by_rate(val, i as u32);
        }
        return (rate, pwr, rs.len());
    }
    // `default:` in Linux: only an error message, `rate_num` stays 0.
    (rate, pwr, 0)
}

/// phy.c:1534-1562 `rtw_phy_store_tx_power_by_rate`
fn store_tx_power_by_rate(t: &mut TxPower, band: u32, rfpath: usize,
                          regaddr: u32, bitmask: u32, data: u32) {
    let (rates, pwr, rate_num) = rate_values_of_txpwr_by_rate(regaddr, bitmask, data);

    if rfpath >= RTW_RF_PATH_MAX
        || (band as u8 != PHY_BAND_2G && band as u8 != PHY_BAND_5G)
        || rate_num > RTW_RF_PATH_MAX
    {
        return;
    }

    for i in 0..rate_num {
        let rate = rates[i] as usize;
        if rate >= DESC_RATE_MAX {
            continue;
        }
        if band as u8 == PHY_BAND_2G {
            t.by_rate_offset_2g[rfpath][rate] = pwr[i];
        } else {
            t.by_rate_offset_5g[rfpath][rate] = pwr[i];
        }
    }
}

/// phy.c:1564-1579 `rtw_parse_tbl_bb_pg`
pub fn parse_tbl_bb_pg(t: &mut TxPower) {
    for p in tables::BB_PG_TYPE0.iter() {
        let (band, rf_path, _tx_num, addr, bitmask, data) =
            (p[0], p[1], p[2], p[3], p[4], p[5]);
        if addr == 0xfe || addr == 0xffe {
            host::sleep_ms(50);
            continue;
        }
        store_tx_power_by_rate(t, band, rf_path as usize, addr, bitmask, data);
    }
}

// ── Limit per domain, bandwidth, rate section and channel ────────

/// phy.c:1590-1611 `rtw_channel_to_idx`
fn channel_to_idx(band: u8, channel: u8) -> Option<usize> {
    if band == PHY_BAND_2G {
        let idx = (channel as i32) - 1;
        if idx < 0 || idx >= RTW_MAX_CHANNEL_NUM_2G as i32 {
            return None;
        }
        Some(idx as usize)
    } else if band == PHY_BAND_5G {
        tables::CHANNEL_IDX_5G.iter().position(|&c| c == channel)
    } else {
        None
    }
}

/// phy.c:1613-1645 `rtw_phy_set_tx_power_limit`.
///
/// Every value is stored twice: once in its domain and once as a minimum
/// in the world domain `RTW_REGD_WW`, which thus ends up the strictest of
/// all and is the fallback for anything the table does not name.
fn set_tx_power_limit(t: &mut TxPower, regd: usize, band: u8, bw: usize,
                      rs: usize, ch: u8, pwr_limit: i8) {
    let pwr_limit = pwr_limit.clamp(-MAX_POWER_INDEX, MAX_POWER_INDEX);
    let ch_idx = match channel_to_idx(band, ch) {
        Some(i) => i,
        None => return,
    };
    if regd >= RTW_REGD_MAX || bw >= RTW_CHANNEL_WIDTH_MAX
        || rs >= RTW_RATE_SECTION_NUM
    {
        return;
    }

    if band == PHY_BAND_2G {
        t.limit_2g[regd][bw][rs][ch_idx] = pwr_limit;
        let ww = t.limit_2g[RTW_REGD_WW][bw][rs][ch_idx].min(pwr_limit);
        t.limit_2g[RTW_REGD_WW][bw][rs][ch_idx] = ww;
    } else if band == PHY_BAND_5G {
        t.limit_5g[regd][bw][rs][ch_idx] = pwr_limit;
        let ww = t.limit_5g[RTW_REGD_WW][bw][rs][ch_idx].min(pwr_limit);
        t.limit_5g[RTW_REGD_WW][bw][rs][ch_idx] = ww;
    }
}

/// phy.c:1716-1728 `__cfg_txpwr_lmt_by_alt`
fn cfg_txpwr_lmt_by_alt_one(t: &mut TxPower, regd: usize, regd_alt: usize,
                            bw: usize, rs: usize) {
    for ch in 0..RTW_MAX_CHANNEL_NUM_2G {
        t.limit_2g[regd][bw][rs][ch] = t.limit_2g[regd_alt][bw][rs][ch];
    }
    for ch in 0..RTW_MAX_CHANNEL_NUM_5G {
        t.limit_5g[regd][bw][rs][ch] = t.limit_5g[regd_alt][bw][rs][ch];
    }
}

/// phy.c:1730-1738 `rtw_cfg_txpwr_lmt_by_alt`
fn cfg_txpwr_lmt_by_alt(t: &mut TxPower, regd: usize, regd_alt: usize) {
    for bw in 0..RTW_CHANNEL_WIDTH_MAX {
        for rs in 0..RTW_RATE_SECTION_NUM {
            cfg_txpwr_lmt_by_alt_one(t, regd, regd_alt, bw, rs);
        }
    }
}

// phy.c:1662-1666: the pairs that are cross-referenced.
const RS_HT_1S: usize = 2;
const RS_HT_2S: usize = 3;
const RS_VHT_1S: usize = 4;
const RS_VHT_2S: usize = 5;
const RS_HT_3S: usize = 6;
const RS_HT_4S: usize = 7;
const RS_VHT_3S: usize = 8;
const RS_VHT_4S: usize = 9;

/// phy.c:1647-1664 `rtw_xref_5g_txpwr_lmt`.
///
/// If the table has a value for HT but not for VHT (or vice versa), the
/// missing one gets the other's value. "No value" means `max_power_index`,
/// the value `init_tx_power_limit` filled in.
fn xref_5g_txpwr_lmt(t: &mut TxPower, regd: usize, bw: usize, ch_idx: usize,
                     rs_ht: usize, rs_vht: usize) {
    let lmt_ht = t.limit_5g[regd][bw][rs_ht][ch_idx];
    let lmt_vht = t.limit_5g[regd][bw][rs_vht][ch_idx];

    if lmt_ht == lmt_vht {
        return;
    }
    if lmt_ht == MAX_POWER_INDEX {
        t.limit_5g[regd][bw][rs_ht][ch_idx] = lmt_vht;
    } else if lmt_vht == MAX_POWER_INDEX {
        t.limit_5g[regd][bw][rs_vht][ch_idx] = lmt_ht;
    }
}

/// phy.c:1667-1684 `rtw_xref_txpwr_lmt_by_rs`
fn xref_txpwr_lmt_by_rs(t: &mut TxPower, regd: usize, bw: usize, ch_idx: usize) {
    const RS_CMP: [(usize, usize); 4] = [
        (RS_HT_1S, RS_VHT_1S),
        (RS_HT_2S, RS_VHT_2S),
        (RS_HT_3S, RS_VHT_3S),
        (RS_HT_4S, RS_VHT_4S),
    ];
    for (rs_ht, rs_vht) in RS_CMP {
        xref_5g_txpwr_lmt(t, regd, bw, ch_idx, rs_ht, rs_vht);
    }
}

/// phy.c:1686-1694 `rtw_xref_5g_txpwr_lmt_by_ch`
fn xref_5g_txpwr_lmt_by_ch(t: &mut TxPower, regd: usize, bw: usize) {
    for ch_idx in 0..RTW_MAX_CHANNEL_NUM_5G {
        xref_txpwr_lmt_by_rs(t, regd, bw, ch_idx);
    }
}

/// phy.c:1696-1704 `rtw_xref_txpwr_lmt_by_bw`, 20 and 40 MHz only.
fn xref_txpwr_lmt_by_bw(t: &mut TxPower, regd: usize) {
    for bw in 0..=1usize {
        xref_5g_txpwr_lmt_by_ch(t, regd, bw);
    }
}

/// phy.c:1706-1713 `rtw_xref_txpwr_lmt`
fn xref_txpwr_lmt(t: &mut TxPower) {
    for regd in 0..RTW_REGD_MAX {
        xref_txpwr_lmt_by_bw(t, regd);
    }
}

/// phy.c:1740-1780 `rtw_parse_tbl_txpwr_lmt`.
///
/// Followed by the two cleanup steps Linux does along with it: a domain
/// absent from the table copies from its alternative (`rtw_regd_has_alt`),
/// or from the world domain if it has none.
pub fn parse_tbl_txpwr_lmt(t: &mut TxPower, tbl: &[(u8, u8, u8, u8, u8, i8)]) {
    let mut regd_cfg_flag: u32 = 0;

    for p in tbl.iter() {
        let (regd, band, bw, rs, ch, lmt) = *p;
        regd_cfg_flag |= 1 << regd;
        set_tx_power_limit(t, regd as usize, band, bw as usize, rs as usize,
                           ch, lmt);
    }

    for i in 0..RTW_REGD_MAX {
        if i == RTW_REGD_WW {
            continue;
        }
        if regd_cfg_flag & (1 << i) != 0 {
            continue;
        }
        match REGD_ALT[i] {
            Some(alt) if regd_cfg_flag & (1 << alt) != 0 => {
                cfg_txpwr_lmt_by_alt(t, i, alt);
            }
            _ => cfg_txpwr_lmt_by_alt(t, i, RTW_REGD_WW),
        }
    }

    xref_txpwr_lmt(t);
}

// ── Absolute values become offsets ───────────────────────────────

/// phy.c:2348-2369 `rtw_phy_tx_power_by_rate_config_by_path`.
///
/// The base rate is the third from last for VHT and the last otherwise;
/// with ten VHT rates that is MCS7, not MCS9. Everything is then expressed
/// relative to it.
fn tx_power_by_rate_config_by_path(t: &mut TxPower, path: usize, rs: usize,
                                   rates: &[u8]) {
    let size = rates.len();
    let base_idx = if size == 10 {
        rates[size - 3] as usize
    } else {
        rates[size - 1] as usize
    };
    let base_2g = t.by_rate_offset_2g[path][base_idx];
    let base_5g = t.by_rate_offset_5g[path][base_idx];
    t.by_rate_base_2g[path][rs] = base_2g;
    t.by_rate_base_5g[path][rs] = base_5g;
    for &r in rates.iter() {
        let i = r as usize;
        t.by_rate_offset_2g[path][i] -= base_2g;
        t.by_rate_offset_5g[path][i] -= base_5g;
    }
}

/// phy.c:2371-2380 `rtw_phy_tx_power_by_rate_config`
pub fn tx_power_by_rate_config(t: &mut TxPower) {
    for path in 0..RTW_RF_PATH_MAX {
        for rs in 0..RTW_RATE_SECTION_NUM {
            tx_power_by_rate_config_by_path(t, path, rs,
                                            tables::RATE_SECTION[rs]);
        }
    }
}

/// phy.c:2382-2396 `__rtw_phy_tx_power_limit_config`
fn tx_power_limit_config_one(t: &mut TxPower, regd: usize, bw: usize, rs: usize) {
    // Path 0, also for path 1, as in Linux.
    let base_2g = t.by_rate_base_2g[0][rs];
    for ch in 0..RTW_MAX_CHANNEL_NUM_2G {
        t.limit_2g[regd][bw][rs][ch] -= base_2g;
    }
    let base_5g = t.by_rate_base_5g[0][rs];
    for ch in 0..RTW_MAX_CHANNEL_NUM_5G {
        t.limit_5g[regd][bw][rs][ch] -= base_5g;
    }
}

/// phy.c:2398-2409 `rtw_phy_tx_power_limit_config`
pub fn tx_power_limit_config(t: &mut TxPower) {
    // default at channel 1
    t.cch_by_bw[0] = 1;

    for regd in 0..RTW_REGD_MAX {
        for bw in 0..RTW_CHANNEL_WIDTH_MAX {
            for rs in 0..RTW_RATE_SECTION_NUM {
                tx_power_limit_config_one(t, regd, bw, rs);
            }
        }
    }
}

/// phy.h:119-136 `rtw_get_rfe_def`: which RFE set applies to this board.
///
/// rtw8822c.c:5277-5285: seven entries, only `[5]` differs (it uses
/// `txpwr_lmt_type5`). An `rfe_option` outside 0..6 has no entry; Linux
/// then returns NULL and `rtw_chip_board_info_setup` fails.
pub fn txpwr_lmt_tbl(rfe_option: u8) -> Option<&'static [(u8, u8, u8, u8, u8, i8)]> {
    match rfe_option {
        5 => Some(&tables::TXPWR_LMT_TYPE5),
        0..=6 => Some(&tables::TXPWR_LMT_TYPE0),
        _ => None,
    }
}

/// main.c:2064-2081 `rtw_chip_board_info_setup`, without
/// `rtw_phy_setup_phy_cond`, which lives in `phy.rs` because it computes
/// the condition for the parameter tables, not for TX power.
///
/// No register access at all: this only fills the tables `rtw_set_channel`
/// computes from, which is why Linux runs it at probe time.
pub fn board_info_setup(rfe_option: u8) -> Option<TxPower> {
    let lmt = txpwr_lmt_tbl(rfe_option)?;
    let mut t = TxPower::new();
    parse_tbl_bb_pg(&mut t);
    parse_tbl_txpwr_lmt(&mut t, lmt);
    tx_power_by_rate_config(&mut t);
    tx_power_limit_config(&mut t);
    Some(t)
}

/// Our own sums, in the same order as `tables::EXPECTED_TXPWR_SUMS`.
/// Bytewise and unsigned, as the generator computes them.
pub fn checksums(t: &TxPower) -> [u32; 6] {
    fn s2(a: &[[i8; DESC_RATE_MAX]; RTW_RF_PATH_MAX]) -> u32 {
        let mut n = 0u32;
        for r in a.iter() {
            for &v in r.iter() {
                n = n.wrapping_add(v as u8 as u32);
            }
        }
        n
    }
    fn sb(a: &[[i8; RTW_RATE_SECTION_NUM]; RTW_RF_PATH_MAX]) -> u32 {
        let mut n = 0u32;
        for r in a.iter() {
            for &v in r.iter() {
                n = n.wrapping_add(v as u8 as u32);
            }
        }
        n
    }
    let mut l2 = 0u32;
    for regd in t.limit_2g.iter() {
        for bw in regd.iter() {
            for rs in bw.iter() {
                for &v in rs.iter() {
                    l2 = l2.wrapping_add(v as u8 as u32);
                }
            }
        }
    }
    let mut l5 = 0u32;
    for regd in t.limit_5g.iter() {
        for bw in regd.iter() {
            for rs in bw.iter() {
                for &v in rs.iter() {
                    l5 = l5.wrapping_add(v as u8 as u32);
                }
            }
        }
    }
    [s2(&t.by_rate_offset_2g), s2(&t.by_rate_offset_5g),
     sb(&t.by_rate_base_2g), sb(&t.by_rate_base_5g), l2, l5]
}

// ════════════════════════════════════════════════════════════════
// From the tables to a power index per rate
// ════════════════════════════════════════════════════════════════

/// main.h:438-531 `struct rtw_txpwr_idx` as stored in the efuse: 42 bytes
/// per path, packed, with 4-bit fields.
///
///     2G: cck_base[6] · bw40_base[5] · ht_1s_diff · ht_2s/3s/4s_diff
///     5G: bw40_base[14] · ht_1s · ht_2s/3s/4s · ofdm · vht_1s..4s
pub struct TxPwrIdx<'a>(pub &'a [u8; 42]);

impl<'a> TxPwrIdx<'a> {
    fn n4(b: u8, hi: bool) -> i8 {
        // A signed 4-bit bitfield, little-endian: the low nibble is the first
        // field.
        let v = if hi { b >> 4 } else { b & 0x0f };
        if v & 0x8 != 0 { (v as i8) - 16 } else { v as i8 }
    }
    pub fn cck_base(&self, g: usize) -> u8 { self.0[g] }
    pub fn bw40_base_2g(&self, g: usize) -> u8 { self.0[6 + g] }
    // 2G ht_1s_diff @11: ofdm (unten), bw20 (oben)
    pub fn g2_ht1s_ofdm(&self) -> i8 { Self::n4(self.0[11], false) }
    pub fn g2_ht1s_bw20(&self) -> i8 { Self::n4(self.0[11], true) }
    // 2G ht_2s/3s/4s_diff @12,13,14: bw20 (low), bw40 (high) in the first
    // byte; `rtw_2g_ns_pwr_idx_diff` is two bytes: bw20,bw40 | cck,ofdm
    pub fn g2_ns_bw20(&self, n: usize) -> i8 { Self::n4(self.0[12 + (n - 2) * 2], false) }
    pub fn g2_ns_bw40(&self, n: usize) -> i8 { Self::n4(self.0[12 + (n - 2) * 2], true) }
    // 5G starts at 18: bw40_base[14]
    pub fn bw40_base_5g(&self, g: usize) -> u8 { self.0[18 + g] }
    pub fn g5_ht1s_ofdm(&self) -> i8 { Self::n4(self.0[32], false) }
    pub fn g5_ht1s_bw20(&self) -> i8 { Self::n4(self.0[32], true) }
    // `rtw_5g_ht_ns_pwr_idx_diff` is one byte (bw20:4, bw40:4), unlike its
    // two-byte 2G counterpart. @33,34,35 for 2s/3s/4s.
    pub fn g5_ns_bw20(&self, n: usize) -> i8 { Self::n4(self.0[33 + (n - 2)], false) }
    pub fn g5_ns_bw40(&self, n: usize) -> i8 { Self::n4(self.0[33 + (n - 2)], true) }
    // ofdm_diff @36,37 (two bytes) is not read by
    // `rtw_phy_get_5g_tx_power_index`; vht_1s..4s follow at @38,39,40,41, and
    // in `rtw_5g_vht_ns_pwr_idx_diff` bw160 is the low nibble, bw80 the high.
    pub fn g5_vht_bw80(&self, n: usize) -> i8 { Self::n4(self.0[38 + (n - 1)], true) }
}

// Rate boundaries from main.h:249-340, used for section and stream
// mapping.
//
// They are derived from `regs.rs` rather than written out separately: an
// off-by-one here shifts the first entry of each section, e.g. VHT MCS0
// would land in the HT 4SS section.
//
// The ends (`_MCS7/9/15/23/31`) are computed because the rows in `main.h`
// are contiguous: eight MCS per HT section, ten per VHT section.
const DESC_RATE11M: u8 = crate::regs::DESC_RATE11M as u8;
const DESC_RATE6M: u8 = crate::regs::DESC_RATE6M as u8;
const DESC_RATE54M: u8 = crate::regs::DESC_RATE54M as u8;
const DESC_RATEMCS0: u8 = crate::regs::DESC_RATEMCS0 as u8;
const DESC_RATEMCS7: u8 = DESC_RATEMCS0 + 7;
const DESC_RATEMCS8: u8 = crate::regs::DESC_RATEMCS8 as u8;
const DESC_RATEMCS15: u8 = DESC_RATEMCS8 + 7;
const DESC_RATEMCS16: u8 = crate::regs::DESC_RATEMCS16 as u8;
const DESC_RATEMCS23: u8 = DESC_RATEMCS16 + 7;
const DESC_RATEMCS24: u8 = crate::regs::DESC_RATEMCS24 as u8;
const DESC_RATEMCS31: u8 = DESC_RATEMCS24 + 7;
const DESC_RATEVHT1SS_MCS0: u8 = crate::regs::DESC_RATEVHT1SS_MCS0 as u8;
const DESC_RATEVHT1SS_MCS9: u8 = DESC_RATEVHT1SS_MCS0 + 9;
const DESC_RATEVHT2SS_MCS0: u8 = crate::regs::DESC_RATEVHT2SS_MCS0 as u8;
const DESC_RATEVHT2SS_MCS9: u8 = DESC_RATEVHT2SS_MCS0 + 9;
const DESC_RATEVHT3SS_MCS0: u8 = crate::regs::DESC_RATEVHT3SS_MCS0 as u8;
const DESC_RATEVHT3SS_MCS9: u8 = DESC_RATEVHT3SS_MCS0 + 9;
const DESC_RATEVHT4SS_MCS0: u8 = crate::regs::DESC_RATEVHT4SS_MCS0 as u8;
const DESC_RATEVHT4SS_MCS9: u8 = DESC_RATEVHT4SS_MCS0 + 9;

/// phy.c:1962-1990 `rtw_phy_rate_to_rate_section`.
/// `RTW_RATE_SECTION_NUM` means invalid rate.
pub fn rate_to_rate_section(rate: u8) -> usize {
    match rate {
        0x00..=DESC_RATE11M => 0,
        DESC_RATE6M..=DESC_RATE54M => 1,
        DESC_RATEMCS0..=DESC_RATEMCS7 => 2,
        DESC_RATEMCS8..=DESC_RATEMCS15 => 3,
        DESC_RATEMCS16..=DESC_RATEMCS23 => 6,
        DESC_RATEMCS24..=DESC_RATEMCS31 => 7,
        DESC_RATEVHT1SS_MCS0..=DESC_RATEVHT1SS_MCS9 => 4,
        DESC_RATEVHT2SS_MCS0..=DESC_RATEVHT2SS_MCS9 => 5,
        DESC_RATEVHT3SS_MCS0..=DESC_RATEVHT3SS_MCS9 => 8,
        DESC_RATEVHT4SS_MCS0..=DESC_RATEVHT4SS_MCS9 => 9,
        _ => RTW_RATE_SECTION_NUM,
    }
}

/// phy.c:1872-1960 `rtw_get_channel_group`: the mapping is generated, the
/// computed case (channel 14) is handled here.
fn channel_group(channel: u8, rate: u8) -> u8 {
    for &(ch, cck, other) in tables::CHANNEL_GROUP_CCK.iter() {
        if ch == channel {
            return if rate <= DESC_RATE11M { cck } else { other };
        }
    }
    let i = channel as usize;
    if i < tables::CHANNEL_GROUP.len() && tables::CHANNEL_GROUP[i] != 0xff {
        tables::CHANNEL_GROUP[i]
    } else {
        // Linux: `default: WARN_ON(1); fallthrough;` -> group 0
        0
    }
}

fn mcs_rate(rate: u8) -> bool {
    (DESC_RATEMCS0..=DESC_RATEMCS31).contains(&rate)
        || (DESC_RATEVHT1SS_MCS0..=DESC_RATEVHT4SS_MCS9).contains(&rate)
}
fn above_2ss(rate: u8) -> bool {
    (DESC_RATEMCS8..=DESC_RATEMCS31).contains(&rate) || rate >= DESC_RATEVHT2SS_MCS0
}
fn above_3ss(rate: u8) -> bool {
    (DESC_RATEMCS16..=DESC_RATEMCS31).contains(&rate) || rate >= DESC_RATEVHT3SS_MCS0
}
fn above_4ss(rate: u8) -> bool {
    (DESC_RATEMCS24..=DESC_RATEMCS31).contains(&rate) || rate >= DESC_RATEVHT4SS_MCS0
}

/// phy.c:1993-2050 `rtw_phy_get_2g_tx_power_index`
fn get_2g_tx_power_index(p: &TxPwrIdx, bw: usize, rate: u8, group: u8) -> u8 {
    let f = TXGI_FACTOR;
    let g = group as usize;
    let mut tx = if rate <= DESC_RATE11M {
        p.cck_base(g) as i16
    } else {
        p.bw40_base_2g(g) as i16
    };

    if (DESC_RATE6M..=DESC_RATE54M).contains(&rate) {
        tx += p.g2_ht1s_ofdm() as i16 * f;
    }
    if !mcs_rate(rate) {
        return tx as u8;
    }
    match bw {
        1 => {
            // bw40 is the base
            if above_2ss(rate) { tx += p.g2_ns_bw40(2) as i16 * f; }
            if above_3ss(rate) { tx += p.g2_ns_bw40(3) as i16 * f; }
            if above_4ss(rate) { tx += p.g2_ns_bw40(4) as i16 * f; }
        }
        _ => {
            // RTW_CHANNEL_WIDTH_20 and Linux's `default: WARN_ON(1)`
            tx += p.g2_ht1s_bw20() as i16 * f;
            if above_2ss(rate) { tx += p.g2_ns_bw20(2) as i16 * f; }
            if above_3ss(rate) { tx += p.g2_ns_bw20(3) as i16 * f; }
            if above_4ss(rate) { tx += p.g2_ns_bw20(4) as i16 * f; }
        }
    }
    tx as u8
}

/// phy.c:2052-2120 `rtw_phy_get_5g_tx_power_index`
fn get_5g_tx_power_index(p: &TxPwrIdx, bw: usize, rate: u8, group: u8) -> u8 {
    let f = TXGI_FACTOR;
    let g = group as usize;
    let mut tx = p.bw40_base_5g(g) as i16;

    if !mcs_rate(rate) {
        tx += p.g5_ht1s_ofdm() as i16 * f;
        return tx as u8;
    }
    match bw {
        1 => {
            if above_2ss(rate) { tx += p.g5_ns_bw40(2) as i16 * f; }
            if above_3ss(rate) { tx += p.g5_ns_bw40(3) as i16 * f; }
            if above_4ss(rate) { tx += p.g5_ns_bw40(4) as i16 * f; }
        }
        2 => {
            // the 80 MHz base is the mean of bw40+ and bw40-
            let lower = p.bw40_base_5g(g) as i16;
            let upper = p.bw40_base_5g(g + 1) as i16;
            tx = (lower + upper) / 2;
            tx += p.g5_vht_bw80(1) as i16 * f;
            if above_2ss(rate) { tx += p.g5_vht_bw80(2) as i16 * f; }
            if above_3ss(rate) { tx += p.g5_vht_bw80(3) as i16 * f; }
            if above_4ss(rate) { tx += p.g5_vht_bw80(4) as i16 * f; }
        }
        _ => {
            tx += p.g5_ht1s_bw20() as i16 * f;
            if above_2ss(rate) { tx += p.g5_ns_bw20(2) as i16 * f; }
            if above_3ss(rate) { tx += p.g5_ns_bw20(3) as i16 * f; }
            if above_4ss(rate) { tx += p.g5_ns_bw20(4) as i16 * f; }
        }
    }
    tx as u8
}

/// phy.c:2149-2196 `rtw_phy_get_tx_power_limit`.
///
/// Takes the minimum over all bandwidths from 20 MHz up to the current
/// one, not only the current one. CCK/OFDM know only 20 MHz, HT at most 40.
fn get_tx_power_limit(t: &TxPower, band: u8, bw: usize, rate: u8,
                      regd: usize) -> i8 {
    let mut power_limit = MAX_POWER_INDEX;
    if regd > RTW_REGD_WW {
        return power_limit;
    }
    let rs = rate_to_rate_section(rate);
    if rs == RTW_RATE_SECTION_NUM {
        return MAX_POWER_INDEX;
    }

    let mut bw = bw;
    if rs == 0 || rs == 1 {
        bw = 0; // only 20 MHz for CCK and OFDM
    }
    if (DESC_RATEMCS0..=DESC_RATEMCS31).contains(&rate) {
        bw = bw.min(1); // HT at most 40 MHz
    }

    for cur_bw in 0..=bw {
        let cur_ch = t.cch_by_bw[cur_bw];
        let ch_idx = match channel_to_idx(band, cur_ch) {
            Some(i) => i,
            None => return MAX_POWER_INDEX,
        };
        let cur_lmt = if cur_ch as usize <= RTW_MAX_CHANNEL_NUM_2G {
            t.limit_2g[regd][cur_bw][rs][ch_idx]
        } else {
            t.limit_5g[regd][cur_bw][rs][ch_idx]
        };
        power_limit = power_limit.min(cur_lmt);
    }
    power_limit
}

/// phy.c:2122-2147 `rtw_phy_get_dis_dpd_by_rate_diff`.
/// `en_dis_dpd` is true on the 8822C, `dpd_ratemask` is `DIS_DPD_RATEALL`.
fn dis_dpd_by_rate_diff(rate: u8) -> i16 {
    if !EN_DIS_DPD {
        return 0;
    }
    let bit: u16 = match rate {
        0x04 => DIS_DPD_RATE6M,
        0x05 => DIS_DPD_RATE9M,
        DESC_RATEMCS0 => DIS_DPD_RATEMCS0,
        0x0d => DIS_DPD_RATEMCS1,
        DESC_RATEMCS8 => DIS_DPD_RATEMCS8,
        0x15 => DIS_DPD_RATEMCS9,
        DESC_RATEVHT1SS_MCS0 => DIS_DPD_RATEVHT1SS_MCS0,
        0x2e => DIS_DPD_RATEVHT1SS_MCS1,
        DESC_RATEVHT2SS_MCS0 => DIS_DPD_RATEVHT2SS_MCS0,
        0x38 => DIS_DPD_RATEVHT2SS_MCS1,
        _ => return 0,
    };
    if bit & DPD_RATEMASK != 0 { -6 * TXGI_FACTOR } else { 0 }
}

/// phy.c:2219-2256 `rtw_get_tx_power_params` + phy.c:2258-2283
/// `rtw_phy_get_tx_power_index`, merged.
///
/// `pwr_sar` is `max_power_index`: `hal->sar.src` is `RTW_SAR_SOURCE_NONE`
/// without an ACPI SAR block, and `rtw_query_sar` then returns exactly
/// that. `pwr_remnant` is 0: `txagc_remnant_*` is set by runtime power
/// tracking.
#[allow(clippy::too_many_arguments)]
pub fn get_tx_power_index(t: &TxPower, p: &TxPwrIdx, path: usize, rate: u8,
                          bw: usize, ch: u8, regd: usize, band: u8) -> u8 {
    let group = channel_group(ch, rate);

    let (base, offset) = if band == PHY_BAND_2G {
        (get_2g_tx_power_index(p, bw, rate, group),
         t.by_rate_offset_2g[path][rate as usize])
    } else {
        (get_5g_tx_power_index(p, bw, rate, group),
         t.by_rate_offset_5g[path][rate as usize])
    };

    let limit = get_tx_power_limit(t, band, bw, rate, regd);
    let sar = MAX_POWER_INDEX;
    let remnant: i16 = 0;

    let mut off = (offset as i16).min(limit as i16).min(sar as i16);
    off += dis_dpd_by_rate_diff(rate);

    let tx_power = base as i16 + off + remnant;
    if tx_power > MAX_POWER_INDEX as i16 {
        MAX_POWER_INDEX as u8
    } else if tx_power < 0 {
        // Linux computes in u8 and lets it wrap; the value is truncated to seven
        // bits afterwards anyway.
        (tx_power as i32 as u32 & 0xff) as u8
    } else {
        tx_power as u8
    }
}

/// phy.c:2285-2330 `rtw_phy_set_tx_power_index_by_rs` +
/// `rtw_phy_set_tx_power_level_by_path`. Fills `tx_pwr_tbl`.
///
/// In Linux `regd` comes from `rtw_regd_get`, the regulatory domain
/// `rtw_regd_init` derives from the country code. Until that is set it is
/// the efuse domain.
pub fn set_tx_power_level(t: &TxPower, p: &[TxPwrIdx], tbl: &mut [[u8; DESC_RATE_MAX]; RTW_RF_PATH_MAX],
                          rf_path_num: u8, ch: u8, bw: usize, band: u8, regd: usize) {
    for path in 0..rf_path_num as usize {
        // No CCK rates without 2.4 GHz.
        let start = if band == PHY_BAND_2G { 0 } else { 1 };
        for rs in start..RTW_RATE_SECTION_NUM {
            for &rate in tables::RATE_SECTION[rs].iter() {
                let idx = get_tx_power_index(t, &p[path], path, rate, bw, ch,
                                             regd, band);
                tbl[path][rate as usize] = idx;
            }
        }
    }
}
