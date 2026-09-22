//! `phy.c` aus Linux 6.18.26 rtw88 — Stufe 3b: die Parametertabellen.
//!
//! Portiert: `rtw_phy_setup_phy_cond` · `check_positive` ·
//! `rtw_parse_tbl_phy_cond` · `rtw_phy_cfg_mac` · `rtw_phy_cfg_agc` ·
//! `rtw_phy_cfg_bb` · `rtw_phy_cfg_rf` · `rtw_load_rfk_table` ·
//! `rtw_phy_load_tables` · `rtw_phy_read_rf` · `rtw_phy_write_rf_reg` ·
//! `rtw_phy_write_rf_reg_sipi` · `rtw_phy_write_rf_reg_mix`.
//!
//! **`rtw_phy_read_rf_sipi` ist NICHT portiert, und das ist keine Luecke:**
//! es liest `chip->rf_sipi_read_addr`, und der 8822C setzt das Feld nicht.
//! In Linux endet die Funktion fuer diesen Chip in `rf_sipi_read_addr isn't
//! defined` und gibt `INV_RF_DATA` zurueck. Eine Portierung waere Code fuer
//! einen Zweig, den dieser Chip nicht hat.
#![allow(dead_code)]

use crate::host;
use crate::regs::*;
use crate::tables;

// ── main.h:32-33, phy.h:179-197 ──────────────────────────────────
pub const RFREG_MASK: u32 = 0xfffff; // phy.h:179
pub const INV_RF_DATA: u32 = 0xffffffff; // main.h:33
pub const LSSI_READ_ADDR_MASK: u32 = 0x7f800000; // phy.h:195
pub const LSSI_READ_EDGE_MASK: u32 = 0x80000000; // phy.h:196
pub const LSSI_READ_DATA_MASK: u32 = 0xfffff; // phy.h:197

pub const RF_PATH_A: usize = 0; // main.h:135
pub const RF_PATH_B: usize = 1; // main.h:136

/// rtw8822c.c:5375 `.rf_base_addr` — der direkte Fensterzugang je Pfad.
const RF_BASE_ADDR: [u32; 2] = [0x3c00, 0x4c00];
/// rtw8822c.c:5376 `.rf_sipi_addr` — der serielle Weg, nur fuer Register 0.
const RF_SIPI_ADDR: [u32; 2] = [0x1808, 0x4108];

// main.h:1862-1869 — die Felder von `struct rtw_phy_cond`.
const INTF_PCIE: u32 = 1 << 0;
const BRANCH_IF: u32 = 0;
const BRANCH_ELIF: u32 = 1;
const BRANCH_ELSE: u32 = 2;
const BRANCH_ENDIF: u32 = 3;

/// main.h:1839-1860 `struct rtw_phy_cond`, als das Wort, das es ist.
///
/// In C ist es ein Bitfeld ueber `u32`; hier steht es als Zahl da, weil die
/// Tabelle genau diese Zahl enthaelt. Die Zerlegung ist Little-Endian —
/// `rfe` liegt unten, `pos` ganz oben.
#[derive(Clone, Copy, Default)]
pub struct PhyCond(pub u32);

impl PhyCond {
    pub fn rfe(self) -> u32 { self.0 & 0xff }
    pub fn intf(self) -> u32 { (self.0 >> 8) & 0xf }
    pub fn pkg(self) -> u32 { (self.0 >> 12) & 0xf }
    pub fn plat(self) -> u32 { (self.0 >> 16) & 0xf }
    pub fn cut(self) -> u32 { (self.0 >> 24) & 0xf }
    pub fn branch(self) -> u32 { (self.0 >> 28) & 0x3 }
    pub fn neg(self) -> bool { self.0 & (1 << 30) != 0 }
    pub fn pos(self) -> bool { self.0 & (1 << 31) != 0 }
}

/// phy.c:1083-1128 `rtw_phy_setup_phy_cond`, PCIe-Zweig.
///
/// `pkg` kommt in Linux aus `hal->pkg_type` — und **dieses Feld wird
/// nirgends beschrieben**, im ganzen Treiber nicht. Es ist also immer 0, und
/// damit greift `pkg ? pkg : 15` und die Bedingung lautet 15. Das steht hier,
/// weil es sonst wie ein vergessener Wert aussieht.
///
/// Der 8812A/8821A-Zweig (rfe aus ext_lna/ext_pa/btcoex zusammengesetzt,
/// cond2 aus den LNA/PA-Typen) gilt fuer andere Chips.
pub fn setup_phy_cond(cut_version: u8, rfe_option: u8) -> PhyCond {
    let cut = if cut_version != 0 { cut_version as u32 } else { 15 };
    let pkg = 15u32; // hal->pkg_type ist 0, siehe oben
    let plat = 0x04u32;
    let rfe = rfe_option as u32;
    let intf = INTF_PCIE;
    PhyCond(rfe | (intf << 8) | (pkg << 12) | (plat << 16) | (cut << 24))
}

/// phy.c:1130-1171 `check_positive`, Zweig fuer alles ausser 8812A/8821A.
///
/// `cut`, `pkg` und `intf` gelten nur, wenn die Tabelle sie NENNT (0 heisst
/// „egal"). `rfe` wird dagegen IMMER verglichen — auch auf 0. Das ist keine
/// Unachtsamkeit in Linux, sondern die Regel, mit der `rfe_option 0` von
/// `rfe_option 1` getrennt wird.
fn check_positive(cond: PhyCond, drv: PhyCond) -> bool {
    if cond.cut() != 0 && cond.cut() != drv.cut() {
        return false;
    }
    if cond.pkg() != 0 && cond.pkg() != drv.pkg() {
        return false;
    }
    if cond.intf() != 0 && cond.intf() != drv.intf() {
        return false;
    }
    cond.rfe() == drv.rfe()
}

/// Welche der vier `do_cfg`-Funktionen eine Tabelle benutzt.
/// In Linux ein Funktionszeiger in `struct rtw_table`; hier eine Art, weil
/// `rtw_phy_cfg_rf` zusaetzlich den Pfad der Tabelle braucht.
#[derive(Clone, Copy, PartialEq)]
pub enum Cfg {
    Mac,
    Agc,
    Bb,
    Rf(usize),
}

/// phy.c:1169-1220 `rtw_parse_tbl_phy_cond`.
///
/// Die Tabelle ist eine Folge von Kacheln zu zwei Woertern. Ein Wort mit
/// Bit 31 ist der KOPF eines Bedingungsblocks (`#if`/`#elif`/`#else`/
/// `#endif`), eines mit Bit 30 sein Ende, alles andere ist Adresse und Wert.
pub fn parse_tbl_phy_cond(h: i32, data: &[u32], cfg: Cfg, drv: PhyCond) -> u32 {
    let mut pos_cond = PhyCond::default();
    let mut is_matched = true;
    let mut is_skipped = false;
    let mut written = 0u32;

    let mut i = 0;
    while i + 1 < data.len() {
        let c = PhyCond(data[i]);
        let val = data[i + 1];
        i += 2;

        if c.pos() {
            match c.branch() {
                BRANCH_ENDIF => {
                    is_matched = true;
                    is_skipped = false;
                }
                BRANCH_ELSE => {
                    is_matched = !is_skipped;
                }
                // BRANCH_IF, BRANCH_ELIF und alles andere
                _ => {
                    // `cond2` (das zweite Wort) traegt nur fuer 8812A/8821A
                    // Inhalt — die LNA/PA-Typen. Hier ist es ungenutzt, und
                    // das ist der Grund, warum es hier nicht mitgefuehrt wird.
                    let _ = BRANCH_IF;
                    let _ = BRANCH_ELIF;
                    pos_cond = c;
                }
            }
        } else if c.neg() {
            if !is_skipped {
                if check_positive(pos_cond, drv) {
                    is_matched = true;
                    is_skipped = true;
                } else {
                    is_matched = false;
                    is_skipped = false;
                }
            } else {
                is_matched = false;
            }
        } else if is_matched {
            do_cfg(h, cfg, c.0, val);
            written += 1;
        }
    }
    written
}

/// phy.c:1782-1830 — die vier `rtw_phy_cfg_*`, hinter EINER Verzweigung.
fn do_cfg(h: i32, cfg: Cfg, addr: u32, data: u32) {
    match cfg {
        // phy.c:1782 `rtw_phy_cfg_mac`
        Cfg::Mac => host::w8(h, addr, data as u8),
        // phy.c:1789 `rtw_phy_cfg_agc`
        Cfg::Agc => host::w32(h, addr, data),
        // phy.c:1796 `rtw_phy_cfg_bb` — sechs Adressen sind Pausen, keine
        // Register. Unter einer Millisekunde wird gedreht statt geschlafen.
        Cfg::Bb => match addr {
            0xfe => host::sleep_ms(50),
            0xfd => host::sleep_ms(5),
            0xfc => host::sleep_ms(1),
            0xfb => host::delay_us(50),
            0xfa => host::delay_us(5),
            0xf9 => host::delay_us(1),
            _ => host::w32(h, addr, data),
        },
        // phy.c:1815 `rtw_phy_cfg_rf`
        Cfg::Rf(path) => match addr {
            0xffe => host::sleep_ms(50),
            0xfe => host::delay_us(100),
            _ => {
                write_rf_reg_mix(h, path, addr, RFREG_MASK, data);
                host::delay_us(1);
            }
        },
    }
}

// ── RF-Zugriff (phy.c:937-1081) ──────────────────────────────────

/// phy.c:937-987 `rtw_phy_read_rf` — der 8822C liest DIREKT, ueber ein
/// Fenster je Pfad (`chip->ops->read_rf = rtw_phy_read_rf`).
pub fn read_rf(h: i32, rf_path: usize, addr: u32, mask: u32) -> u32 {
    if rf_path >= RF_BASE_ADDR.len() {
        return INV_RF_DATA;
    }
    let addr = addr & 0xff;
    let direct_addr = RF_BASE_ADDR[rf_path] + (addr << 2);
    host::r32_mask(h, direct_addr, mask & RFREG_MASK)
}

/// phy.c:1047-1070 `rtw_phy_write_rf_reg`
fn write_rf_reg(h: i32, rf_path: usize, addr: u32, mask: u32, data: u32) -> bool {
    if rf_path >= RF_BASE_ADDR.len() {
        return false;
    }
    let addr = addr & 0xff;
    let direct_addr = RF_BASE_ADDR[rf_path] + (addr << 2);
    host::w32_mask(h, direct_addr, mask & RFREG_MASK, data);
    host::delay_us(1);
    true
}

/// phy.c:1009-1046 `rtw_phy_write_rf_reg_sipi`.
///
/// Beim 8822C nur fuer Register 0 erreichbar (siehe `write_rf_reg_mix`).
/// `mask != RFREG_MASK` fuehrt vorher einen Lesezugriff — der geht ueber
/// `chip->ops->read_rf`, also ueber den DIREKTEN Weg, nicht ueber SIPI.
fn write_rf_reg_sipi(h: i32, rf_path: usize, addr: u32, mask: u32, data: u32) -> bool {
    if rf_path >= RF_SIPI_ADDR.len() {
        return false;
    }
    let addr = addr & 0xff;
    let mask = mask & RFREG_MASK;
    let mut data = data;

    if mask != RFREG_MASK {
        let old_data = read_rf(h, rf_path, addr, RFREG_MASK);
        if old_data == INV_RF_DATA {
            host::print("[rtl8822ce] Write fail, rf is disabled\n");
            return false;
        }
        let shift = mask.trailing_zeros();
        data = (old_data & !mask) | (data << shift);
    }

    let data_and_addr = ((addr << 20) | (data & 0x000f_ffff)) & 0x0fff_ffff;
    host::w32(h, RF_SIPI_ADDR[rf_path], data_and_addr);
    host::delay_us(13);
    true
}

/// phy.c:1072-1081 `rtw_phy_write_rf_reg_mix` — der Schreibweg des 8822C.
pub fn write_rf_reg_mix(h: i32, rf_path: usize, addr: u32, mask: u32, data: u32) -> bool {
    if addr != 0x00 {
        return write_rf_reg(h, rf_path, addr, mask, data);
    }
    write_rf_reg_sipi(h, rf_path, addr, mask, data)
}

// ── Die Tabellen laden (phy.c:1832-1871) ─────────────────────────

/// phy.c:1832-1848 `rtw_load_rfk_table`.
///
/// Die fuenf Schreibzugriffe davor stehen ohne Kommentar in Linux; sie
/// schalten den DPK-Block an, bevor seine Initialtabelle laeuft.
fn load_rfk_table(h: i32, drv: PhyCond) -> u32 {
    host::w32_mask(h, 0x1e24, 1 << 17, 0x1);
    host::w32_mask(h, 0x1cd0, 1 << 28, 0x1);
    host::w32_mask(h, 0x1cd0, 1 << 29, 0x1);
    host::w32_mask(h, 0x1cd0, 1 << 30, 0x1);
    host::w32_mask(h, 0x1cd0, 1 << 31, 0x0);

    // `dpk_info->is_dpk_pwr_on = true` ist reiner Treiberzustand und wird
    // erst von der DPK-Kalibrierung gelesen — ein Posten der Stufe 4.
    parse_tbl_phy_cond(h, &tables::RFK_INIT, Cfg::Bb, drv)
}

/// phy.c:1850-1871 `rtw_phy_load_tables`.
///
/// **Die Reihenfolge der RF-Tabellen ist umgekehrt zu ihrem Namen** und das
/// ist kein Tippfehler: `rtw8822c_hw_spec` sagt
/// `.rf_tbl = {&rtw8822c_rf_b_tbl, &rtw8822c_rf_a_tbl}` (rtw8822c.c:5382),
/// die Schleife laeuft ueber den Index, und jede Tabelle traegt ihren Pfad
/// selbst. Geladen wird also erst B, dann A.
///
/// Gibt zurueck, ob jede Tabelle so viele Schreibzugriffe abgegeben hat, wie
/// `gen_tables.py` fuer DIESEN Chipzustand vorausgerechnet hat.
pub fn load_tables(h: i32, rf_path_num: u8, drv: PhyCond) -> bool {
    // Die Vorausrechnung gilt fuer cut D und rfe_option 1 — das Geraet, an
    // dem gemessen wurde. Steht dort etwas anderes, wird nur gezaehlt und
    // nicht verglichen; eine Zahl gegen die falsche Erwartung zu halten
    // waere schlimmer als gar keine.
    let reference = setup_phy_cond(crate::regs::RTW_CHIP_VER_CUT_D, 1);
    let comparable = drv.0 == reference.0;
    let mut all_ok = true;

    let mut check = |name: &str, n: u32| {
        host::print("    ");
        host::print(name);
        host::print(": ");
        host::print_dec(n);
        host::print(" Schreibzugriffe");
        if comparable {
            let mut want = None;
            for &(nm, w) in tables::EXPECTED_WRITES_CUT_D_RFE1.iter() {
                if nm == name {
                    want = Some(w);
                }
            }
            match want {
                Some(w) if w == n => host::print("  (erwartet)"),
                Some(w) => {
                    host::print("  <- ERWARTET ");
                    host::print_dec(w);
                    all_ok = false;
                }
                None => {
                    host::print("  <- keine Erwartung hinterlegt");
                    all_ok = false;
                }
            }
        }
        host::print("\n");
    };

    check("mac", parse_tbl_phy_cond(h, &tables::MAC, Cfg::Mac, drv));
    check("bb", parse_tbl_phy_cond(h, &tables::BB, Cfg::Bb, drv));
    check("agc", parse_tbl_phy_cond(h, &tables::AGC, Cfg::Agc, drv));

    // `rfe_def->agc_btg_tbl` — der 8822C benutzt `RTW_DEF_RFE` ohne btg
    // (rtw8822c.c:5277-5285), das Feld ist also NULL und Linux ueberspringt
    // den Aufruf. Es gibt hier nichts zu laden.

    check("rfk_init", load_rfk_table(h, drv));

    // rf_tbl[0] = rf_b, rf_tbl[1] = rf_a — siehe oben.
    let rf_tbl: [(&[u32], usize, &str); 2] = [
        (&tables::RF_B, RF_PATH_B, "rf_b"),
        (&tables::RF_A, RF_PATH_A, "rf_a"),
    ];
    for &(data, path, name) in rf_tbl.iter().take(rf_path_num as usize) {
        check(name, parse_tbl_phy_cond(h, data, Cfg::Rf(path), drv));
    }

    all_ok
}

// ── Stufe 3c: rtw_phy_init (phy.c:236-261) ───────────────────────

use crate::dm::{DmInfo, PathDiv, EWMA_THERMAL_PRECISION, EWMA_THERMAL_WEIGHT_RCP};

/// phy.c:1673-1674 · rtw8822c.c:4906-4909 `rtw8822c_dig` — Adresse und
/// Maske des IGI-Felds je Pfad. Beide liegen in DEMSELBEN Register.
const DIG: [(u32, u32); 2] = [(0x1d70, 0x7f), (0x1d70, 0x7f00)];

/// phy.c:1600-1611 `rtw_phy_cck_pd_init`. Reiner Treiberzustand.
fn cck_pd_init(dm: &mut DmInfo) {
    // `i <= RTW_CHANNEL_WIDTH_40` sind die Breiten 20 und 40, also zwei.
    for i in 0..=1usize {
        for j in 0..4usize {
            dm.cck_pd_lv[i][j] = 0; // CCK_PD_LV0 (phy.h:164)
        }
    }
    dm.cck_fa_avg = CCK_FA_AVG_RESET;
}
/// phy.h:193

/// phy.c:175-200 `rtw_phy_adaptivity_set_mode`.
///
/// `rtwdev->regd.dfs_region` steht vor der ersten Kanalwahl auf
/// `NL80211_DFS_UNSET`, und der `default`-Zweig setzt dann `RTW_EDCCA_NORMAL`
/// ohne `l2h_th_ini`. Die zwei Sonderfaelle (ETSI, Japan) brauchen eine
/// Regulierungszone, die es hier noch nicht gibt — sie kommen mit
/// `rtw_regd_init`, und das ist die Stufe, die einen Kanal setzt.
fn adaptivity_set_mode(dm: &mut DmInfo) {
    dm.edcca_mode = 0; // RTW_EDCCA_NORMAL (main.h:1683)
}

/// phy.c:2245-2257 `rtw_phy_set_edcca_th`
pub fn set_edcca_th(h: i32, l2h: u8, h2l: u8) {
    let (addr, mask, off) = EDCCA_TH_L2H;
    host::w32_mask(h, addr, mask, (l2h + off) as u32);
    let (addr, mask, off) = EDCCA_TH_H2L;
    host::w32_mask(h, addr, mask, (h2l + off) as u32);
}

/// rtw8822c.c:2178-2196 `rtw8822c_adaptivity` — der Chip-Zweig von
/// `rtw_phy_adaptivity`, alle zwei Sekunden.
///
/// `set_edcca_th` nimmt vorzeichenlose Werte, Linux rechnet in `s8`.
/// Beide Schwellen sind nach der Rechnung positiv (`EDCCA_TH_L2H_LB` = 48
/// ist die Untergrenze, `h2l` liegt sieben bis acht darunter), also ist
/// die Umdeutung hier ein Wechsel der Darstellung und keine Klemmung.
pub fn adaptivity(h: i32, dm: &DmInfo) {
    let igi = dm.igi_history[0] as i8;
    let (l2h, h2l);
    if dm.edcca_mode == 0 {
        // RTW_EDCCA_NORMAL
        l2h = igi.saturating_add(EDCCA_IGI_L2H_DIFF).max(EDCCA_TH_L2H_LB);
        h2l = l2h - EDCCA_L2H_H2L_DIFF_NORMAL;
    } else {
        l2h = if igi < (dm.l2h_th_ini as i8) - EDCCA_ADC_BACKOFF {
            igi.saturating_add(EDCCA_ADC_BACKOFF)
        } else {
            dm.l2h_th_ini as i8
        };
        h2l = l2h - EDCCA_L2H_H2L_DIFF;
    }
    set_edcca_th(h, l2h as u8, h2l as u8);
}

/// phy.c:202-209 `rtw_phy_adaptivity_init` + `chip->ops->adaptivity_init`
/// = rtw8822c.c `rtw8822c_adaptivity_init`.
fn adaptivity_init(h: i32, dm: &mut DmInfo) {
    adaptivity_set_mode(dm);

    set_edcca_th(h, RTW8822C_EDCCA_MAX, RTW8822C_EDCCA_MAX);
    // mac edcca state setting
    host::clr32(h, REG_TX_PTCL_CTRL, BIT_DIS_EDCCA);
    host::set32(h, REG_RD_CTRL, BIT_EDCCA_MSK_CNTDOWN_EN);
    // edcca decistion opt
    host::clr32(h, REG_EDCCA_DECISION, BIT_EDCCA_OPTION);
}

/// phy.c:236-261 `rtw_phy_init`.
///
/// Die Verzweigungen ueber `chip->ops` sind hier ausgeschrieben: der 8822C
/// fuehrt `adaptivity_init` und `cfo_init`, also gelten beide.
pub fn phy_init(h: i32, dm: &mut DmInfo, path_div: &mut PathDiv,
                crystal_cap: u8, default_1ss_tx_path: u8) {
    dm.fa_history = [0; 4];
    dm.igi_bitmap = 0;
    dm.igi_history[3] = 0;
    dm.igi_history[2] = 0;
    dm.igi_history[1] = 0;

    let (addr, mask) = DIG[0];
    dm.igi_history[0] = host::r32_mask(h, addr, mask) as u8;
    cck_pd_init(dm);

    dm.iqk_done = false;
    adaptivity_init(h, dm);

    // rtw8822c.c:4257-4263 `rtw8822c_cfo_init`
    dm.cfo_track.crystal_cap = crystal_cap;
    dm.cfo_track.is_adjust = true;

    // phy.c:222-234 `rtw_phy_tx_path_div_init`
    path_div.current_tx_path = default_1ss_tx_path;
    path_div.path_a_cnt = 0;
    path_div.path_a_sum = 0;
    path_div.path_b_cnt = 0;
    path_div.path_b_sum = 0;
}

// ═══════════════════════════════════════════════════════════════════
// rtw_watch_dog_work — die LAUFENDE Haelfte (main.c:224-310)
//
// Linux fuehrt sie alle zwei Sekunden, das ganze Leben einer Verbindung
// lang. Bis 0.26.0 gab es sie hier nicht: gebaut war der Aufbau, und der
// Chip blieb danach sich selbst ueberlassen. Drei ihrer Posten sind
// SENDEseite und haengen an der Temperatur — und ein Empfaenger rastet
// sich an jeder Praeambel neu ein, ein Sender nicht.
// ═══════════════════════════════════════════════════════════════════

/// phy.c:292-309 `rtw_phy_get_rssi_level`
pub fn get_rssi_level(old_level: u8, rssi: u8) -> u8 {
    let mut table: [u8; RA_FLOOR_TABLE_SIZE] = [20, 34, 38, 42, 46, 50, 100];
    let mut new_level = 0u8;

    for (i, t) in table.iter_mut().enumerate() {
        if i as u8 >= old_level {
            *t += RA_FLOOR_UP_GAP;
        }
    }

    for i in 0..RA_FLOOR_TABLE_SIZE {
        if rssi < table[i] {
            new_level = i as u8;
            break;
        }
    }

    new_level
}

/// phy.c:318-321 `rtw_phy_stat_rate_cnt`
pub fn stat_rate_cnt(dm: &mut DmInfo) {
    dm.last_pkt_count = dm.cur_pkt_count;
    dm.cur_pkt_count = crate::dm::PktCount::new();
}

/// phy.c:335-371 `rtw_phy_dig_check_damping`.
///
/// **Die Daempfungsbremse.** Sie erkennt, dass die Verstaerkungsregelung
/// zwischen zwei Werten hin und her springt, und laesst sie dann in Ruhe
/// — `igi_bitmap` traegt die Richtung der letzten vier Schritte als Bits.
fn dig_check_damping(dm: &mut DmInfo) -> bool {
    let fa_lo = DIG_PERF_FA_TH_LOW as u16;
    let fa_hi = DIG_PERF_FA_TH_HIGH as u16;
    let mut damping = false;

    let min_rssi = dm.min_rssi;
    if dm.damping {
        let damping_rssi = dm.damping_rssi;
        let diff = if min_rssi > damping_rssi {
            min_rssi - damping_rssi
        } else {
            damping_rssi - min_rssi
        };
        // Linux: `dm_info->damping_cnt++ > 20` — NACHzaehlend, der
        // Vergleich sieht also den Wert VOR dem Erhoehen.
        let cnt = dm.damping_cnt;
        dm.damping_cnt = dm.damping_cnt.wrapping_add(1);
        if diff > 3 || cnt > 20 {
            dm.damping = false;
            return false;
        }
        return true;
    }

    let igi = dm.igi_history;
    let fa = dm.fa_history;
    match dm.igi_bitmap & 0xf {
        // runter -> rauf -> runter -> rauf
        5 => {
            if igi[0] > igi[1] && igi[2] > igi[3]
                && igi[0] - igi[1] >= 2 && igi[2] - igi[3] >= 2
                && fa[0] > fa_hi && fa[1] < fa_lo
                && fa[2] > fa_hi && fa[3] < fa_lo
            {
                damping = true;
            }
        }
        // rauf -> runter -> runter -> rauf
        9 => {
            if igi[0] > igi[1] && igi[3] > igi[2]
                && igi[0] - igi[1] >= 4 && igi[3] - igi[2] >= 2
                && fa[0] > fa_hi && fa[1] < fa_lo
                && fa[2] < fa_lo && fa[3] > fa_hi
            {
                damping = true;
            }
        }
        _ => return false,
    }

    if damping {
        dm.damping = true;
        dm.damping_cnt = 0;
        dm.damping_rssi = min_rssi;
    }

    damping
}

/// phy.c:373-395 `rtw_phy_dig_get_boundary` — gibt `(upper, lower)`.
fn dig_get_boundary(dm: &DmInfo, linked: bool) -> (u8, u8) {
    let (mut dig_max, dig_mid, dig_min, min_rssi);
    if linked {
        dig_max = DIG_PERF_MAX as u8;
        dig_mid = DIG_PERF_MID as u8;
        dig_min = RTW8822C_DIG_MIN;
        min_rssi = dm.min_rssi.max(dig_min);
    } else {
        dig_max = DIG_CVRG_MAX as u8;
        dig_mid = DIG_CVRG_MID as u8;
        dig_min = DIG_CVRG_MIN as u8;
        min_rssi = dig_min;
    }

    // „DIG MAX should be bounded by minimum RSSI with offset +15"
    dig_max = dig_max.min(min_rssi.saturating_add(DIG_RSSI_GAIN_OFFSET as u8));

    let lower = min_rssi.clamp(dig_min, dig_mid);
    let upper = lower.saturating_add(DIG_RSSI_GAIN_OFFSET as u8)
        .clamp(dig_min, dig_max.max(dig_min));
    (upper, lower)
}

/// phy.c:397-417 `rtw_phy_dig_get_threshold` — gibt `(fa_th, step)`.
fn dig_get_threshold(dm: &DmInfo, linked: bool) -> ([u16; 3], [u8; 3]) {
    let mut step = [4u8, 3, 2];
    let fa_th;

    if linked {
        fa_th = [DIG_PERF_FA_TH_EXTRA_HIGH as u16,
                 DIG_PERF_FA_TH_HIGH as u16,
                 DIG_PERF_FA_TH_LOW as u16];
        if dm.pre_min_rssi > dm.min_rssi {
            step = [6, 4, 2];
        }
    } else {
        fa_th = [DIG_CVRG_FA_TH_EXTRA_HIGH as u16,
                 DIG_CVRG_FA_TH_HIGH as u16,
                 DIG_CVRG_FA_TH_LOW as u16];
    }

    (fa_th, step)
}

/// phy.c:419-441 `rtw_phy_dig_recorder`
fn dig_recorder(dm: &mut DmInfo, igi: u8, fa: u16) {
    let mut igi_bitmap = (dm.igi_bitmap << 1) & 0xfe;
    let up = igi > dm.igi_history[0];
    igi_bitmap |= up as u8;

    dm.igi_history[3] = dm.igi_history[2];
    dm.igi_history[2] = dm.igi_history[1];
    dm.igi_history[1] = dm.igi_history[0];
    dm.igi_history[0] = igi;

    dm.fa_history[3] = dm.fa_history[2];
    dm.fa_history[2] = dm.fa_history[1];
    dm.fa_history[1] = dm.fa_history[0];
    dm.fa_history[0] = fa;

    dm.igi_bitmap = igi_bitmap;
}

/// phy.c:443-459 `rtw_phy_dig_write`.
///
/// `rtw8822c` fuehrt `.dig_cck = NULL` (rtw8822c.c:5374), der CCK-Zweig
/// entfaellt also — er steht hier als Kommentar und nicht als Code, weil
/// ein toter Zweig sonst wie eine Auslassung aussieht.
pub fn dig_write(h: i32, rf_path_num: u8, igi: u8) {
    for path in 0..rf_path_num as usize {
        let (addr, mask) = DIG[path.min(1)];
        host::w32_mask(h, addr, mask, igi as u32);
    }
}

/// phy.c:461-518 `rtw_phy_dig`.
///
/// `linked` ist Linux' `!!rtwdev->sta_cnt`. Der 8812A-Sonderfall am Ende
/// gilt fuer einen anderen Chip und steht deshalb nicht hier.
pub fn dig(h: i32, dm: &mut DmInfo, rf_path_num: u8, linked: bool) {
    // `RTW_FLAG_DIG_DISABLE` setzt bei uns niemand — der Test entfaellt.
    if dig_check_damping(dm) {
        return;
    }

    let fa_cnt = dm.total_fa_cnt as u16;
    let pre_igi = dm.igi_history[0];

    let (fa_th, step) = dig_get_threshold(dm, linked);

    // „test the false alarm count from the highest threshold level first,
    //  and increase it by corresponding step size — note that the step
    //  size is offset by -2, compensate it afterall"
    let mut cur_igi = pre_igi;
    for level in 0..3usize {
        if fa_cnt > fa_th[level] {
            cur_igi = cur_igi.wrapping_add(step[level]);
            break;
        }
    }
    cur_igi = cur_igi.wrapping_sub(2);

    let (upper, lower) = dig_get_boundary(dm, linked);
    cur_igi = cur_igi.clamp(lower, upper.max(lower));

    dig_recorder(dm, cur_igi, fa_cnt);

    if cur_igi != pre_igi {
        dig_write(h, rf_path_num, cur_igi);
    }
}

/// phy.c:705-717 `rtw_phy_cck_pd_lv_unlink`
fn cck_pd_lv_unlink(dm: &DmInfo) -> u8 {
    if dm.cck_fa_avg > CCK_PD_FA_LV1_MIN {
        return CCK_PD_LV1;
    }
    if dm.cck_fa_avg < CCK_PD_FA_LV0_MAX {
        return CCK_PD_LV0;
    }
    CCK_PD_LV_MAX
}

/// phy.c:719-735 `rtw_phy_cck_pd_lv_link`
fn cck_pd_lv_link(dm: &DmInfo) -> u8 {
    let igi = dm.igi_history[0];
    let rssi = dm.min_rssi;

    if igi > CCK_PD_IGI_LV4_VAL && rssi > CCK_PD_RSSI_LV4_VAL {
        return CCK_PD_LV4;
    }
    if igi > CCK_PD_IGI_LV3_VAL && rssi > CCK_PD_RSSI_LV3_VAL {
        return CCK_PD_LV3;
    }
    if igi > CCK_PD_IGI_LV2_VAL || rssi > CCK_PD_RSSI_LV2_VAL {
        return CCK_PD_LV2;
    }
    if dm.cck_fa_avg > CCK_PD_FA_LV1_MIN {
        return CCK_PD_LV1;
    }
    if dm.cck_fa_avg < CCK_PD_FA_LV0_MAX {
        return CCK_PD_LV0;
    }
    CCK_PD_LV_MAX
}

/// phy.c:737-743 `rtw_phy_cck_pd_lv`
fn cck_pd_lv(dm: &DmInfo, linked: bool) -> u8 {
    if !linked {
        cck_pd_lv_unlink(dm)
    } else {
        cck_pd_lv_link(dm)
    }
}

/// phy.c:745-772 `rtw_phy_cck_pd`.
///
/// Nur auf 2,4 GHz — auf 5 GHz gibt es kein CCK, und der Zweig steht in
/// Linux genauso weit oben.
pub fn cck_pd(h: i32, dm: &mut DmInfo, band_2g: bool, linked: bool) {
    if !band_2g {
        return;
    }

    let cck_fa = dm.cck_fa_cnt;
    if dm.cck_fa_avg == CCK_FA_AVG_RESET {
        dm.cck_fa_avg = cck_fa;
    } else {
        dm.cck_fa_avg = (dm.cck_fa_avg.wrapping_mul(3).wrapping_add(cck_fa)) >> 2;
    }

    let level = cck_pd_lv(dm, linked);
    if level >= CCK_PD_LV_MAX {
        return;
    }

    crate::chip::phy_cck_pd_set(h, dm, level);
}

/// phy.c:1021-1049 `rtw_phy_get_rrsr_mask`
pub fn get_rrsr_mask(rate_idx: u8) -> u32 {
    let mut rate_order = rate_idx;

    if rate_idx >= DESC_RATEVHT4SS_MCS0 as u8 {
        rate_order -= DESC_RATEVHT4SS_MCS0 as u8;
    } else if rate_idx >= DESC_RATEVHT3SS_MCS0 as u8 {
        rate_order -= DESC_RATEVHT3SS_MCS0 as u8;
    } else if rate_idx >= DESC_RATEVHT2SS_MCS0 as u8 {
        rate_order -= DESC_RATEVHT2SS_MCS0 as u8;
    } else if rate_idx >= DESC_RATEVHT1SS_MCS0 as u8 {
        rate_order -= DESC_RATEVHT1SS_MCS0 as u8;
    } else if rate_idx >= DESC_RATEMCS24 as u8 {
        rate_order -= DESC_RATEMCS24 as u8;
    } else if rate_idx >= DESC_RATEMCS16 as u8 {
        rate_order -= DESC_RATEMCS16 as u8;
    } else if rate_idx >= DESC_RATEMCS8 as u8 {
        rate_order -= DESC_RATEMCS8 as u8;
    } else if rate_idx >= DESC_RATEMCS0 as u8 {
        rate_order -= DESC_RATEMCS0 as u8;
    } else if rate_idx >= DESC_RATE6M as u8 {
        rate_order -= DESC_RATE6M as u8;
    } else {
        rate_order -= DESC_RATE1M as u8;
    }

    if rate_idx >= DESC_RATEMCS0 as u8 || rate_order == 0 {
        rate_order += 1;
    }

    // GENMASK(rate_order + RRSR_RATE_ORDER_CCK_LEN - 1, 0)
    let hi = rate_order as u32 + RRSR_RATE_ORDER_CCK_LEN - 1;
    if hi >= 31 { u32::MAX } else { (1u32 << (hi + 1)) - 1 }
}

/// phy.c:1062-1069 `rtw_phy_rrsr_update`.
///
/// Mit EINER Station ist der Iterator ein Aufruf; `rate` ist ihr
/// `ra_report.desc_rate`.
pub fn rrsr_update(h: i32, dm: &mut DmInfo, sta_rate: Option<u8>) {
    dm.rrsr_mask_min = RRSR_RATE_ORDER_MAX;
    if let Some(r) = sta_rate {
        let mask = get_rrsr_mask(r);
        if mask < dm.rrsr_mask_min {
            dm.rrsr_mask_min = mask;
        }
    }
    host::w32(h, REG_RRSR, dm.rrsr_val_init & dm.rrsr_mask_min);
}

/// phy.c:1918-1930 `rtw_phy_set_tx_path_by_reg`
fn set_tx_path_by_reg(h: i32, path_div: &mut PathDiv, antenna_tx: u8,
                      tx_path_sel_1ss: u8) {
    if tx_path_sel_1ss == path_div.current_tx_path {
        return;
    }
    path_div.current_tx_path = tx_path_sel_1ss;
    // `chip->ops->config_tx_path(…, tx_path_sel_1ss, tx_path_sel_cck, false)`
    // — beide Auswahlen sind derselbe Wert (phy.c:1921).
    crate::chip::config_tx_path(h, antenna_tx, tx_path_sel_1ss,
                                tx_path_sel_1ss, false);
}

/// phy.c:1932-1955 `rtw_phy_tx_path_div_select`
fn tx_path_div_select(h: i32, path_div: &mut PathDiv, antenna_tx: u8) {
    let mut path = path_div.current_tx_path;

    let rssi_a = if path_div.path_a_cnt != 0 {
        path_div.path_a_sum / path_div.path_a_cnt as u32
    } else {
        0
    };
    let rssi_b = if path_div.path_b_cnt != 0 {
        path_div.path_b_sum / path_div.path_b_cnt as u32
    } else {
        0
    };

    if rssi_a != rssi_b {
        path = if rssi_a > rssi_b { BB_PATH_A } else { BB_PATH_B };
    }

    path_div.path_a_cnt = 0;
    path_div.path_a_sum = 0;
    path_div.path_b_cnt = 0;
    path_div.path_b_sum = 0;
    set_tx_path_by_reg(h, path_div, antenna_tx, path);
}

/// phy.c:1957-1972 `rtw_phy_tx_path_diversity_2ss` + phy.c:1974-1982
/// `rtw_phy_tx_path_diversity`.
///
/// `rtw8822c` fuehrt `.path_div_supported = true` (rtw8822c.c:5362).
pub fn tx_path_diversity(h: i32, path_div: &mut PathDiv, antenna_tx: u8,
                         antenna_rx: u8, linked: bool) {
    if !RTW8822C_PATH_DIV_SUPPORTED {
        return;
    }
    if antenna_rx != BB_PATH_AB {
        return;
    }
    if !linked {
        return;
    }
    tx_path_div_select(h, path_div, antenna_tx);
}

// ── rtw_phy_pwr_track und seine Helfer ───────────────────────────

/// phy.c `rtw_phy_pwrtrack_avg`
pub fn pwrtrack_avg(dm: &mut DmInfo, thermal: u8, path: usize) {
    dm.avg_thermal[path].add(thermal as u32, EWMA_THERMAL_PRECISION,
                             EWMA_THERMAL_WEIGHT_RCP);
    dm.thermal_avg[path] =
        dm.avg_thermal[path].read(EWMA_THERMAL_PRECISION) as u8;
}

/// phy.c `rtw_phy_pwrtrack_thermal_changed`
pub fn pwrtrack_thermal_changed(dm: &DmInfo, thermal: u8, path: usize) -> bool {
    dm.avg_thermal[path].read(EWMA_THERMAL_PRECISION) as u8 != thermal
}

/// phy.c `rtw_phy_pwrtrack_get_delta` — der Abstand zur efuse, gedeckelt
/// auf die Tabellenlaenge.
pub fn pwrtrack_get_delta(dm: &DmInfo, thermal_meter: &[u8],
                          path: usize) -> u8 {
    let therm_avg = dm.thermal_avg[path];
    let therm_efuse = thermal_meter[path];
    let therm_delta = if therm_avg > therm_efuse {
        therm_avg - therm_efuse
    } else {
        therm_efuse - therm_avg
    };
    therm_delta.min(RTW_PWR_TRK_TBL_SZ as u8 - 1)
}

/// phy.c `rtw_phy_pwrtrack_need_lck`
pub fn pwrtrack_need_lck(dm: &mut DmInfo) -> bool {
    let a = dm.thermal_avg[0];
    let b = dm.thermal_meter_lck;
    let delta_lck = if a > b { a - b } else { b - a };
    if delta_lck >= RTW8822C_LCK_THRESHOLD {
        dm.thermal_meter_lck = dm.thermal_avg[0];
        return true;
    }
    false
}

/// phy.c `rtw_phy_pwrtrack_need_iqk`
pub fn pwrtrack_need_iqk(dm: &mut DmInfo) -> bool {
    let a = dm.thermal_avg[0];
    let b = dm.thermal_meter_k;
    let delta_iqk = if a > b { a - b } else { b - a };
    if delta_iqk >= RTW8822C_IQK_THRESHOLD {
        dm.thermal_meter_k = dm.thermal_avg[0];
        return true;
    }
    false
}

/// Die vier Kurvenpaare, die `rtw_phy_config_swing_table` fuer den
/// aktuellen Kanal und die aktuelle Rate auswaehlt: je Pfad ein `p` und
/// ein `n`.
pub struct SwingTable {
    pub p: [&'static [u8; RTW_PWR_TRK_TBL_SZ]; 2],
    pub n: [&'static [u8; RTW_PWR_TRK_TBL_SZ]; 2],
}

/// phy.c `rtw_phy_config_swing_table`.
///
/// **Fuer den 8822C gibt es genau EINE Tabelle**: alle sieben
/// RFE-Varianten zeigen auf `type0` (rtw8822c.c:5277-5285). Eine Auswahl
/// nach RFE waere hier eine erfundene Verzweigung.
pub fn config_swing_table(channel: u8, tx_rate: u8) -> SwingTable {
    // IS_CH_2G_BAND(channel) — Kanal 1..14
    if channel <= 14 {
        if tx_rate <= DESC_RATE11M as u8 {
            SwingTable {
                p: [&tables::PWRTRK_2G_CCKA_P, &tables::PWRTRK_2G_CCKB_P],
                n: [&tables::PWRTRK_2G_CCKA_N, &tables::PWRTRK_2G_CCKB_N],
            }
        } else {
            SwingTable {
                p: [&tables::PWRTRK_2GA_P, &tables::PWRTRK_2GB_P],
                n: [&tables::PWRTRK_2GA_N, &tables::PWRTRK_2GB_N],
            }
        }
    } else {
        // IS_CH_5G_BAND_1/2 -> Index 0, BAND_3 -> 1, BAND_4 -> 2.
        let i = if channel <= 64 { 0 } else if channel <= 144 { 1 } else { 2 };
        SwingTable {
            p: [&tables::PWRTRK_5GA_P[i], &tables::PWRTRK_5GB_P[i]],
            n: [&tables::PWRTRK_5GA_N[i], &tables::PWRTRK_5GB_N[i]],
        }
    }
}

/// phy.c `rtw_phy_pwrtrack_get_pwridx` — waermer als die efuse heisst
/// nach oben, kaelter nach unten.
pub fn pwrtrack_get_pwridx(dm: &DmInfo, swing: &SwingTable,
                           thermal_meter: &[u8], tbl_path: usize,
                           therm_path: usize, delta: u8) -> i8 {
    if delta as usize >= RTW_PWR_TRK_TBL_SZ {
        return 0;
    }
    if dm.thermal_avg[therm_path] > thermal_meter[therm_path] {
        swing.p[tbl_path][delta as usize] as i8
    } else {
        -(swing.n[tbl_path][delta as usize] as i8)
    }
}

/// phy.c:311-316 `rtw_phy_statistics`.
///
/// Der `rssi`-Zweig ist bei Linux ein Stationen-Iterator; wir fahren
/// EINE Gegenstelle, also ist er ein `Option`. Ohne Verbindung bleibt
/// `min_rssi` auf `U8_MAX` stehen — genau wie dort.
pub fn statistics(h: i32, dm: &mut DmInfo, st: &mut crate::fw::H2cState,
                  si: Option<&mut crate::sta::StaInfo>) {
    // rtw_phy_stat_rssi
    let mut min_rssi = u8::MAX;
    if let Some(si) = si {
        let rssi = si.avg_rssi.read(crate::dm::EWMA_RSSI_PRECISION) as u8;
        si.rssi_level = get_rssi_level(si.rssi_level, rssi);
        crate::fw::send_rssi_info(h, st, si);
        min_rssi = min_rssi.min(rssi);
    }
    dm.pre_min_rssi = dm.min_rssi;
    dm.min_rssi = min_rssi;

    // rtw_phy_stat_false_alarm -> chip->ops->false_alarm_statistics
    crate::chip::false_alarm_statistics(h, dm);

    // rtw_phy_stat_rate_cnt
    stat_rate_cnt(dm);
}

/// phy.c:1051-1060 `rtw_phy_ra_info_update` — **nur jeden VIERTEN Takt**
/// (`watch_dog_cnt & 0x3`), also alle acht Sekunden.
fn ra_info_update(h: i32, st: &mut crate::fw::H2cState, watch_dog_cnt: u32,
                  si: Option<(&mut crate::sta::StaInfo,
                              &crate::sta::PeerCaps, u8, bool)>) {
    if watch_dog_cnt & 0x3 != 0 {
        return;
    }
    if let Some((si, caps, nss, band_2g)) = si {
        // `rtw_update_sta_info(rtwdev, si, false)`
        crate::sta::update_sta_info(si, caps, nss, band_2g);
        crate::fw::send_ra_info(h, st, si, false);
    }
}

/// phy.c:1071-1076 `rtw_phy_ra_track`
#[allow(clippy::too_many_arguments)]
pub fn ra_track(h: i32, dm: &mut DmInfo, st: &mut crate::fw::H2cState,
                tx_tp: u32, rx_tp: u32, watch_dog_cnt: u32,
                si: Option<(&mut crate::sta::StaInfo,
                            &crate::sta::PeerCaps, u8, bool)>,
                sta_rate: Option<u8>) {
    crate::fw::update_wl_phy_info(h, st, dm, tx_tp, rx_tp);
    // main.c:1266/1286 — der Teil von `rtw_update_sta_info`, der `dm`
    // schreibt: die Grundmenge der Antwortraten je Band.
    if let Some((_, _, _, band_2g)) = &si {
        dm.rrsr_val_init = if *band_2g { RRSR_INIT_2G } else { RRSR_INIT_5G };
    }
    ra_info_update(h, st, watch_dog_cnt, si);
    rrsr_update(h, dm, sta_rate);
}

/// phy.c:791-806 `rtw_phy_dynamic_mechanism` — die neun Posten in
/// Linux' Reihenfolge.
///
/// Alles, was hier `dm` liest, hat der Empfangsweg zwischen zwei Takten
/// gefuellt (`rx::watchdog_feed`). Laeuft der nicht, rechnet der ganze
/// Block auf Nullen und meldet nichts.
#[allow(clippy::too_many_arguments)]
pub fn dynamic_mechanism(h: i32, dm: &mut DmInfo, path_div: &mut PathDiv,
                         dpk: &mut crate::dpk::DpkInfo,
                         st: &mut crate::fw::H2cState, e: &crate::efuse::Efuse,
                         hal_rf_path_num: u8, antenna_tx: u8, antenna_rx: u8,
                         channel: u8, band_2g: bool, linked: bool,
                         bt_disabled: bool, tx_tp: u32, rx_tp: u32,
                         watch_dog_cnt: u32,
                         si: Option<(&mut crate::sta::StaInfo,
                                     &crate::sta::PeerCaps, u8, bool)>,
                         sta_rate: Option<u8>,
                         rssi_si: Option<&mut crate::sta::StaInfo>,
                         fw_has_adaptivity: bool) {
    statistics(h, dm, st, rssi_si);
    dig(h, dm, hal_rf_path_num, linked);
    cck_pd(h, dm, band_2g, linked);
    ra_track(h, dm, st, tx_tp, rx_tp, watch_dog_cnt, si, sta_rate);
    tx_path_diversity(h, path_div, antenna_tx, antenna_rx, linked);
    crate::chip::cfo_track(h, dm, hal_rf_path_num, e.crystal_cap, linked,
                           bt_disabled);
    crate::dpk::track(h, dpk);
    crate::chip::pwr_track(h, dm, e.power_track_type, &e.thermal_meter,
                           hal_rf_path_num, channel);

    if fw_has_adaptivity {
        crate::fw::adaptivity(h, st, dm);
    } else {
        adaptivity(h, dm);
    }
}
