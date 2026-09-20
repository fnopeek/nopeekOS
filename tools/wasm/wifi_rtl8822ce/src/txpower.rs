//! `phy.c` aus Linux 6.18.26 rtw88 — die Sendeleistung.
//!
//! **Warum das VOR Stufe 3 steht:** in Linux laeuft `rtw_chip_board_info_setup`
//! (main.c:2064) zur Probe-Zeit, direkt nach `rtw_chip_efuse_info_setup` und
//! LANGE vor `rtw_power_on`. Es schreibt kein einziges Register — es fuellt
//! nur die Tabellen in `hal`, aus denen `rtw_set_channel` spaeter die
//! Sendeleistung je Kanal, Rate und Pfad ausrechnet.
//!
//! Portiert: `rtw_phy_init_tx_power` · `rtw_phy_init_tx_power_limit` ·
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
use crate::tables;

// ── Masse aus main.h ─────────────────────────────────────────────
pub const RTW_RF_PATH_MAX: usize = 4; // main.h:38
pub const DESC_RATE_MAX: usize = 0x54; // main.h:341, folgt auf 0x53
pub const RTW_RATE_SECTION_NUM: usize = 10; // main.h:176
pub const RTW_REGD_MAX: usize = 13; // main.h:359, folgt auf RTW_REGD_WW
pub const RTW_REGD_WW: usize = 12; // main.h:358
pub const RTW_CHANNEL_WIDTH_MAX: usize = 3; // main.h:37
pub const RTW_MAX_CHANNEL_NUM_2G: usize = 14; // main.h:49
pub const RTW_MAX_CHANNEL_NUM_5G: usize = 49; // main.h:50
pub const PHY_BAND_2G: u8 = 0; // main.h, enum rtw_phy_band_type
pub const PHY_BAND_5G: u8 = 1;

/// rtw8822c.c:5351 `.max_power_index = 0x7f`
pub const MAX_POWER_INDEX: i8 = 0x7f;

/// regd.c:522-533 `rtw_regd_alt[]` — welche Regulierungszone von welcher
/// abschreibt, wenn die Tabelle fuer sie nichts sagt. `None` heisst
/// „keine Ersatzzone", und dann gilt WW.
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

/// main.h:1993-2011 `struct rtw_hal`, der Sendeleistungsteil.
///
/// Rund 25 KiB. Das ist kein Versehen: `tx_pwr_limit_5g` allein ist
/// 13 Zonen x 3 Bandbreiten x 10 Ratenabschnitte x 49 Kanaele.
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
    /// phy.c:2427-2446 `rtw_phy_init_tx_power` — samt
    /// `rtw_phy_init_tx_power_limit`, das nichts tut, als jede Zelle auf
    /// `max_power_index` zu setzen. Die Null davor kommt vom Anlegen.
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

// ── Die Leistung JE RATE (bb_pg) ─────────────────────────────────

/// phy.c:1219 `#define bcd_to_dec_pwr_by_rate(val, i) bcd2bin(val >> (i * 8))`
///
/// `bcd2bin(x)` ist `(x & 0x0f) + (x >> 4) * 10` — eine Zahl, deren zwei
/// Nibbles DEZIMALziffern sind.
fn bcd_to_dec_pwr_by_rate(val: u32, i: u32) -> i8 {
    let b = ((val >> (i * 8)) & 0xff) as u8;
    ((b & 0x0f) + (b >> 4) * 10) as i8
}

/// phy.c:1221-1227 `tbl_to_dec_pwr_by_rate`.
///
/// `chip->is_pwr_by_rate_dec` ist beim 8822C **false** (rtw8822c.c:5350),
/// also gilt der einfache Zweig: das Byte, wie es dasteht.
fn tbl_to_dec_pwr_by_rate(hex: u32, i: u32) -> i8 {
    ((hex >> (i * 8)) & 0xff) as i8
}

/// phy.c:1229-1532 `rtw_phy_get_rate_values_of_txpwr_by_rate`.
///
/// Die ZUORDNUNG steht in `tables::TXPWR_BY_RATE_MAP`, erzeugt aus genau
/// diesem `switch`. Hier stehen nur die zwei Faelle, die RECHNEN statt
/// zuzuordnen — und dass sie hier stehen, ist der Grund, warum sie nicht
/// in der Tabelle sind.
///
/// Gibt `(raten, werte, anzahl)` zurueck.
fn rate_values_of_txpwr_by_rate(addr: u32, mask: u32, val: u32)
    -> ([u8; 4], [i8; 4], usize)
{
    let mut rate = [0u8; 4];
    let mut pwr = [0i8; 4];

    // phy.c:1259 `case 0xE08:` — EIN Wert, und er kommt aus BCD, Byte 1.
    if addr == 0xE08 {
        rate[0] = 0x00; // DESC_RATE1M
        pwr[0] = bcd_to_dec_pwr_by_rate(val, 1);
        return (rate, pwr, 1);
    }

    // phy.c:1264-1277 `case 0x86C:` — haengt an der MASKE.
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
        // Linux faellt hier ohne `rate_num` heraus, also mit 0.
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
    // `default:` in Linux: nur eine Fehlermeldung, `rate_num` bleibt 0.
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

// ── Die GRENZE je Zone, Bandbreite, Ratenabschnitt und Kanal ─────

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
/// **Jeder Wert geht zweimal hinein:** einmal in seine Zone und einmal als
/// MINIMUM in die Welt-Zone `RTW_REGD_WW`. Die ist damit am Ende die
/// strengste aller Zonen — und der Rueckfall fuer alles, was die Tabelle
/// gar nicht nennt.
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

// phy.c:1662-1666 — die Paare, ueber die quergeglichen wird.
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
/// Sagt die Tabelle fuer HT etwas und fuer VHT nichts (oder umgekehrt),
/// bekommt der stumme den Wert des anderen. „Nichts gesagt" heisst hier
/// `max_power_index` — der Wert, mit dem `init_tx_power_limit` gefuellt hat.
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

/// phy.c:1696-1704 `rtw_xref_txpwr_lmt_by_bw` — nur 20 und 40 MHz.
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
/// Danach die zwei Aufraeumschritte, die Linux gleich mitmacht: eine Zone,
/// die in der Tabelle gar nicht vorkommt, schreibt von ihrer Ersatzzone ab
/// (`rtw_regd_has_alt`) — und wenn es keine gibt, von der Welt-Zone.
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

// ── Aus absoluten Werten werden Abweichungen ─────────────────────

/// phy.c:2348-2369 `rtw_phy_tx_power_by_rate_config_by_path`.
///
/// **Die Basisrate ist bei VHT die DRITTLETZTE**, sonst die letzte — bei
/// zehn VHT-Raten also MCS7 und nicht MCS9. Alles wird danach relativ zu
/// ihr ausgedrueckt.
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
    // **Pfad 0**, auch fuer Pfad 1 — so steht es in Linux.
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

/// phy.h:119-136 `rtw_get_rfe_def` — welcher RFE-Satz fuer dieses Board.
///
/// rtw8822c.c:5277-5285: sieben Eintraege, und nur `[5]` weicht ab (er
/// nimmt `txpwr_lmt_type5`). `rfe_option` ausserhalb 0..6 hat keinen
/// Eintrag; Linux gibt dann NULL und `rtw_chip_board_info_setup`
/// scheitert.
pub fn txpwr_lmt_tbl(rfe_option: u8) -> Option<&'static [(u8, u8, u8, u8, u8, i8)]> {
    match rfe_option {
        5 => Some(&tables::TXPWR_LMT_TYPE5),
        0..=6 => Some(&tables::TXPWR_LMT_TYPE0),
        _ => None,
    }
}

/// main.c:2064-2081 `rtw_chip_board_info_setup`, ohne
/// `rtw_phy_setup_phy_cond` — das steht in `phy.rs`, weil es die
/// Bedingung fuer die PARAMETERTABELLEN rechnet und nicht fuer die
/// Sendeleistung.
///
/// **Kein einziger Registerzugriff.** Diese Stufe fuellt nur die Tabellen,
/// aus denen `rtw_set_channel` spaeter rechnet — und genau deshalb laeuft
/// sie in Linux zur Probe-Zeit und nicht im Anlaufweg.
pub fn board_info_setup(rfe_option: u8) -> Option<TxPower> {
    let lmt = txpwr_lmt_tbl(rfe_option)?;
    let mut t = TxPower::new();
    parse_tbl_bb_pg(&mut t);
    parse_tbl_txpwr_lmt(&mut t, lmt);
    tx_power_by_rate_config(&mut t);
    tx_power_limit_config(&mut t);
    Some(t)
}

/// Die eigenen Summen, in derselben Reihenfolge wie
/// `tables::EXPECTED_TXPWR_SUMS`. Byteweise, ohne Vorzeichen — genau wie
/// die Nachrechnung im Erzeuger.
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
