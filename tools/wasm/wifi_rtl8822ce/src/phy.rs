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

use crate::dm::{DmInfo, PathDiv};

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
const CCK_FA_AVG_RESET: u32 = 0xffffffff;

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

    // rtw8822c.c `rtw8822c_cfo_init`
    dm.cfo_crystal_cap = crystal_cap;
    dm.cfo_is_adjust = true;

    // phy.c:222-234 `rtw_phy_tx_path_div_init`
    path_div.current_tx_path = default_1ss_tx_path;
    path_div.path_a_cnt = 0;
    path_div.path_a_sum = 0;
    path_div.path_b_cnt = 0;
    path_div.path_b_sum = 0;
}
