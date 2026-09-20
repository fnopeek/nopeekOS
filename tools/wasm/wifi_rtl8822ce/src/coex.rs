//! `coex.c` aus Linux 6.18.26 rtw88 — der Teil, den `rtw_power_on` faehrt.
//!
//! **Warum das ueberhaupt hier steht:** unsere efuse meldet `btcoex JA` und
//! `share_ant JA`. Auf diesem Board teilen WLAN und Bluetooth EINE Antenne,
//! und wer sie bekommt, entscheidet dieser Block. Linux' allererste
//! Coex-Handlung nach dem Einschalten ist woertlich „set antenna path to BT"
//! (`rtw_coex_power_on_setting`), und erst `rtw_coex_init_hw_config` holt sie
//! zurueck. Ohne diese Kette steht der Schalter dort, wo ihn das BIOS
//! gelassen hat — und der Empfaenger hoert nichts.
//!
//! Portiert, in Aufrufreihenfolge: `rtw_coex_power_on_setting` ·
//! `rtw_coex_read_scbd` · `rtw_coex_write_scbd` · `rtw_coex_monitor_bt_enable` ·
//! `rtw_coex_check_rfk` · `rtw_coex_coex_ctrl_owner` · `rtw_coex_set_gnt_bt` ·
//! `rtw_coex_set_gnt_wl` · `rtw_coex_set_ant_path` · `rtw_coex_set_table` ·
//! `rtw_btc_wltoggle_table_a` · `rtw_coex_table` · `rtw_coex_init_coex_var` ·
//! `rtw_coex_wl_slot_extend` · `rtw_coex_wl_ccklock_action` ·
//! `rtw_coex_set_wl_pri_mask` · `rtw_coex_power_save_state` ·
//! `rtw_coex_set_tdma` · `rtw_coex_tdma_timer_base` · `rtw_coex_tdma` ·
//! `rtw_coex_query_bt_info` · `__rtw_coex_init_hw_config` ·
//! `rtw_coex_read_indirect_reg` · `rtw_coex_write_indirect_reg`.
#![allow(dead_code)]

use crate::fw::H2cState;
use crate::host;
use crate::mac;
use crate::regs::*;
use crate::tables;

/// main.h:1587-1620 `struct rtw_coex_stat` und `rtw_coex_dm` — die Felder,
/// die der Anlaufweg liest oder schreibt. Alles andere gehoert zum
/// LAUFENDEN Betrieb (Verkehrsmessung, BT-Profile, RSSI-Zustaende) und
/// kommt mit der Stufe, die einen Kanal hat.
#[derive(Clone, Copy)]
pub struct Coex {
    // rtw_coex
    pub stop_dm: bool,
    pub wl_rf_off: bool,
    pub freeze: bool,
    pub manual_control: bool,
    pub under_5g: bool,
    pub freerun: bool,
    // rtw_coex_stat
    pub bt_disabled: bool,
    pub bt_ble_scan_type: u8,
    pub bt_mailbox_reply: bool,
    pub bt_reenable: bool,
    pub bt_iqk_state: u8,
    pub score_board: u16,
    pub kt_ver: u8,
    pub wl_slot_extend: bool,
    pub wl_slot_toggle: bool,
    pub wl_slot_toggle_change: bool,
    pub wl_force_lps_ctrl: bool,
    pub wl_coex_mode: u8,
    pub wl_beacon_interval: u16,
    pub tdma_timer_base: u8,
    pub gnt_workaround_state: u8,
    pub cnt_wl_5ms_noextend: u32,
    // rtw_coex_dm
    pub cur_table: u8,
    pub cur_ps_tdma: u8,
    pub cur_ps_tdma_on: bool,
    pub cur_ant_pos_type: u8,
    pub cur_bt_lna_lvl: u8,
    pub reason: u8,
    pub ps_tdma_para: [u8; 5],
    // rtw_coex_rfe (rtw8822c_coex_cfg_rfe_type)
    pub rfe_module_type: u8,
    pub ant_switch_polarity: u8,
    pub ant_switch_exist: bool,
    pub ant_switch_with_bt: bool,
    pub ant_switch_diversity: bool,
    pub wlg_at_btg: bool,
}

/// coex.h:173-178 `enum coex_wl_link_mode` — die letzte Marke folgt auf
/// 0x7, ist also **8**. `rtw_coex_init_coex_var` setzt sie als „noch kein
/// Modus" ein, und `rtw8822c_coex_cfg_gnt_fix` vergleicht dagegen.
const COEX_WLINK_MAX: u8 = 8; // coex.h:177
/// coex.h:101 `COEX_RSN_LPS = 13`
const COEX_RSN_LPS: u8 = 13;

impl Coex {
    pub const fn new() -> Self {
        Coex {
            stop_dm: false, wl_rf_off: false, freeze: false,
            manual_control: false, under_5g: false, freerun: false,
            bt_disabled: false, bt_ble_scan_type: 0, bt_mailbox_reply: false,
            bt_reenable: false, bt_iqk_state: 0, score_board: 0, kt_ver: 0,
            wl_slot_extend: false, wl_slot_toggle: false,
            wl_slot_toggle_change: false, wl_force_lps_ctrl: false,
            wl_coex_mode: 0, wl_beacon_interval: 0, tdma_timer_base: 0,
            gnt_workaround_state: 0, cnt_wl_5ms_noextend: 0,
            cur_table: 0, cur_ps_tdma: 0, cur_ps_tdma_on: false,
            cur_ant_pos_type: 0, cur_bt_lna_lvl: 0, reason: 0,
            ps_tdma_para: [0; 5],
            rfe_module_type: 0, ant_switch_polarity: 0,
            ant_switch_exist: false, ant_switch_with_bt: false,
            ant_switch_diversity: false, wlg_at_btg: false,
        }
    }

    /// coex.c `rtw_coex_init_coex_var`.
    ///
    /// Linux `memset`t `coex_dm` und `coex_stat` und setzt danach drei
    /// Felder. Die RSSI-Zustaende auf `COEX_RSSI_STATE_LOW` = 0 fallen mit
    /// dem Nullen zusammen, `wl_rx_rate`/`wl_rts_rx_rate` gehoeren zum
    /// laufenden Betrieb.
    fn init_coex_var(&mut self) {
        let keep_rfe = (self.rfe_module_type, self.wlg_at_btg,
                        self.ant_switch_exist);
        let stop_dm = self.stop_dm;
        let wl_rf_off = self.wl_rf_off;
        *self = Coex::new();
        // `coex` selbst wird NICHT genullt — nur `coex_dm` und `coex_stat`.
        self.stop_dm = stop_dm;
        self.wl_rf_off = wl_rf_off;
        self.rfe_module_type = keep_rfe.0;
        self.wlg_at_btg = keep_rfe.1;
        self.ant_switch_exist = keep_rfe.2;
        self.wl_coex_mode = COEX_WLINK_MAX;
    }
}

// ── Score-Board: das Zwei-Byte-Gespraech mit dem BT-Kern ─────────

/// coex.c `rtw_coex_read_scbd`. `chip->scbd_support` ist beim 8822C `true`.
pub fn read_scbd(h: i32) -> u16 {
    host::r16(h, REG_WIFI_BT_INFO) & !BIT_BT_INT_EN
}

/// coex.c `rtw_coex_write_scbd`.
///
/// **`new_scbd10_def` ist beim 8822C `true`** (rtw8822c.c:5407), also gilt
/// der EINFACHE Zweig: `FIX2M` wird wie jedes andere Bit gesetzt. Beim
/// 8822B ist es umgekehrt herum, und genau dafuer steht das Feld.
pub fn write_scbd(h: i32, c: &mut Coex, bitpos: u16, set: bool) {
    let mut val: u16 = 0x2;
    val |= c.score_board;

    // new_scbd10_def == true -> der else-Zweig
    if set {
        val |= bitpos;
    } else {
        val &= !bitpos;
    }

    if val != c.score_board {
        c.score_board = val;
        host::w16(h, REG_WIFI_BT_INFO, val | BIT_BT_INT_EN);
    }
}

/// coex.c `rtw_coex_monitor_bt_enable`, Zweig mit `scbd_support`.
///
/// Der `bt_reenable_work`-Zeitgeber (15 s) haengt an mac80211's Arbeitswarte-
/// schlange; wir merken nur die Fahne, die er dort setzt.
fn monitor_bt_enable(h: i32, c: &mut Coex) {
    let score_board = read_scbd(h);
    let bt_disabled = score_board & COEX_SCBD_ONOFF == 0;

    if c.bt_disabled != bt_disabled {
        c.bt_disabled = bt_disabled;
        c.bt_ble_scan_type = 0;
        c.cur_bt_lna_lvl = 0;

        if !c.bt_disabled {
            c.bt_reenable = true;
        } else {
            c.bt_mailbox_reply = false;
            c.bt_reenable = false;
        }
    }
}

// ── Der indirekte LTE-Registerraum ───────────────────────────────

/// coex.c `rtw_coex_read_indirect_reg`
pub fn read_indirect_reg(h: i32, addr: u16) -> u32 {
    match mac::ltecoex_read_reg(h, addr) {
        Some(v) => v,
        None => {
            host::print("[rtl8822ce] failed to read indirect register\n");
            0
        }
    }
}

/// coex.c `rtw_coex_write_indirect_reg`
pub fn write_indirect_reg(h: i32, addr: u16, mask: u32, val: u32) {
    let shift = mask.trailing_zeros();
    let tmp = read_indirect_reg(h, addr);
    let tmp = (tmp & !mask) | ((val << shift) & mask);
    if !mac::ltecoex_reg_write(h, addr, tmp) {
        host::print("[rtl8822ce] failed to write indirect register\n");
    }
}

// ── Wer bekommt die Antenne ──────────────────────────────────────

/// coex.c `rtw_coex_set_gnt_bt`
fn set_gnt_bt(h: i32, state: u32) {
    write_indirect_reg(h, LTE_COEX_CTRL, 0xc000, state);
    write_indirect_reg(h, LTE_COEX_CTRL, 0x0c00, state);
}

/// coex.c `rtw_coex_set_gnt_wl`
fn set_gnt_wl(h: i32, state: u32) {
    write_indirect_reg(h, LTE_COEX_CTRL, 0x3000, state);
    write_indirect_reg(h, LTE_COEX_CTRL, 0x0300, state);
}

/// coex.c `rtw_coex_coex_ctrl_owner`.
///
/// `chip->btg_reg` ist beim 8822C NICHT gesetzt (kein Treffer in
/// rtw8822c.c), der zweite Schreibzugriff entfaellt also.
fn coex_ctrl_owner(h: i32, wifi_control: bool) {
    if wifi_control {
        host::set8(h, REG_SYS_SDIO_CTRL + 3, (BIT_LTE_MUX_CTRL_PATH >> 24) as u8);
    } else {
        host::clr8(h, REG_SYS_SDIO_CTRL + 3, (BIT_LTE_MUX_CTRL_PATH >> 24) as u8);
    }
}

/// coex.c `rtw_coex_check_rfk`.
///
/// Wartet, bis weder BT noch WLAN kalibrieren, bevor der Besitzer des
/// Antennenpfads wechselt. `wlg_at_btg` ist bei gemeinsamer Antenne wahr,
/// `scbd_support` ist es immer — der Zweig gilt also bei uns.
fn check_rfk(h: i32, c: &mut Coex) {
    if !(c.wlg_at_btg && c.bt_iqk_state != 0xff) {
        return;
    }
    let wait_cnt = COEX_RFK_TIMEOUT / COEX_MIN_DELAY;
    let mut cnt = 0u32;
    loop {
        let btk = read_scbd(h) & COEX_SCBD_BT_RFK != 0;
        let wlk = host::r8(h, REG_ARFR4) & BIT_WL_RFK != 0;
        if !btk && !wlk {
            break;
        }
        host::sleep_ms(COEX_MIN_DELAY);
        cnt += 1;
        if cnt >= wait_cnt {
            c.bt_iqk_state = 0xff;
            break;
        }
    }
}

/// coex.c `rtw_coex_set_ant_path`.
///
/// **Der Aufruf, um den es geht.** `ant_switch_exist` ist beim 8822C
/// `false` (rtw8822c_coex_cfg_rfe_type) und `coex_set_ant_switch` ist
/// ohnehin `NULL` — der Schalter haengt also allein an GNT_BT/GNT_WL und
/// am Besitzer des Pfads. `ctrl_type`/`pos_type` werden trotzdem gesetzt,
/// weil sie in Linux gesetzt werden.
pub fn set_ant_path(h: i32, c: &mut Coex, force: bool, phase: u8) {
    if !force && c.cur_ant_pos_type == phase {
        return;
    }
    c.cur_ant_pos_type = phase;

    // avoid switch coex_ctrl_owner during BT IQK
    check_rfk(h, c);

    let (ctrl_type, pos_type): (u8, u8);
    match phase {
        COEX_SET_ANT_POWERON => {
            // set path control owner to BT at power-on
            coex_ctrl_owner(h, c.bt_disabled);
            ctrl_type = COEX_SWITCH_CTRL_BY_BBSW;
            pos_type = 0; // COEX_SWITCH_TO_BT
        }
        COEX_SET_ANT_INIT => {
            if c.bt_disabled {
                set_gnt_bt(h, COEX_GNT_SET_SW_LOW);
                set_gnt_wl(h, COEX_GNT_SET_SW_HIGH);
            } else {
                set_gnt_bt(h, COEX_GNT_SET_SW_HIGH);
                set_gnt_wl(h, COEX_GNT_SET_SW_LOW);
            }
            // set path control owner to wl at initial step
            coex_ctrl_owner(h, true);
            ctrl_type = COEX_SWITCH_CTRL_BY_BBSW;
            pos_type = 0; // COEX_SWITCH_TO_BT
        }
        COEX_SET_ANT_WONLY => {
            set_gnt_bt(h, COEX_GNT_SET_SW_LOW);
            set_gnt_wl(h, COEX_GNT_SET_SW_HIGH);
            coex_ctrl_owner(h, true);
            ctrl_type = COEX_SWITCH_CTRL_BY_BBSW;
            pos_type = 1; // COEX_SWITCH_TO_WLG
        }
        COEX_SET_ANT_WOFF => {
            coex_ctrl_owner(h, false);
            ctrl_type = COEX_SWITCH_CTRL_BY_BT;
            pos_type = 4; // COEX_SWITCH_TO_NOCARE
        }
        COEX_SET_ANT_2G => {
            set_gnt_bt(h, COEX_GNT_SET_HW_PTA);
            set_gnt_wl(h, COEX_GNT_SET_HW_PTA);
            coex_ctrl_owner(h, true);
            ctrl_type = COEX_SWITCH_CTRL_BY_PTA;
            pos_type = 4; // COEX_SWITCH_TO_NOCARE
        }
        COEX_SET_ANT_5G => {
            set_gnt_bt(h, COEX_GNT_SET_HW_PTA);
            set_gnt_wl(h, COEX_GNT_SET_SW_HIGH);
            coex_ctrl_owner(h, true);
            ctrl_type = COEX_SWITCH_CTRL_BY_BBSW;
            pos_type = 2; // COEX_SWITCH_TO_WLA
        }
        COEX_SET_ANT_2G_FREERUN => {
            set_gnt_bt(h, COEX_GNT_SET_HW_PTA);
            set_gnt_wl(h, COEX_GNT_SET_SW_HIGH);
            coex_ctrl_owner(h, true);
            ctrl_type = COEX_SWITCH_CTRL_BY_BBSW;
            pos_type = 3; // COEX_SWITCH_TO_WLG_BT
        }
        COEX_SET_ANT_2G_WLBT => {
            set_gnt_bt(h, COEX_GNT_SET_HW_PTA);
            set_gnt_wl(h, COEX_GNT_SET_HW_PTA);
            coex_ctrl_owner(h, true);
            ctrl_type = COEX_SWITCH_CTRL_BY_BBSW;
            pos_type = 3; // COEX_SWITCH_TO_WLG_BT
        }
        _ => {
            host::print("[rtl8822ce] unknown phase when setting antenna path\n");
            return;
        }
    }

    // `rtw_coex_set_ant_switch` — `chip->ops->coex_set_ant_switch` ist beim
    // 8822C NULL (rtw8822c.c:4993) und `ant_switch_exist` ist false. Beide
    // Gruende einzeln reichen; der Aufruf kann nie stattfinden.
    let _ = (ctrl_type, pos_type, COEX_SWITCH_CTRL_MAX, COEX_SWITCH_TO_MAX);
}

// ── Die Koexistenz-Tabelle ───────────────────────────────────────

/// coex.c `rtw_coex_set_table`
fn set_table(h: i32, c: &Coex, force: bool, table0: u32, table1: u32) {
    const DEF_BRK_TABLE_VAL: u32 = 0xf0ff_ffff;
    if !force && c.reason != COEX_RSN_LPS
        && table0 == host::r32(h, REG_BT_COEX_TABLE0)
        && table1 == host::r32(h, REG_BT_COEX_TABLE1)
    {
        return;
    }
    host::w32(h, REG_BT_COEX_TABLE0, table0);
    host::w32(h, REG_BT_COEX_TABLE1, table1);
    host::w32(h, REG_BT_COEX_BRK_TABLE, DEF_BRK_TABLE_VAL);
}

/// coex.c `rtw_btc_wltoggle_table_a`
fn wltoggle_table_a(h: i32, st: &mut H2cState, share_ant: bool, table_case: u8) {
    let mut table_wl: u32 = 0x5a5a_5a5a;
    if share_ant {
        if (table_case as usize) < tables::COEX_TABLE_SANT.len() {
            table_wl = tables::COEX_TABLE_SANT[table_case as usize][1];
        }
    } else if (table_case as usize) < tables::COEX_TABLE_NSANT.len() {
        table_wl = tables::COEX_TABLE_NSANT[table_case as usize][1];
    }

    // h2c_para[1] = 0x1 ("no definition"), dann die vier Bytes von table_wl
    let data = [
        0x1u8,
        (table_wl & 0xff) as u8,
        ((table_wl >> 8) & 0xff) as u8,
        ((table_wl >> 16) & 0xff) as u8,
        ((table_wl >> 24) & 0xff) as u8,
    ];
    crate::fw::bt_wifi_control(h, st, COEX_H2C69_TOGGLE_TABLE_A, &data);
}

/// coex.c `rtw_coex_table`.
///
/// **Die Tabellen sind ERZEUGT** (`gen_tables.py` aus rtw8822c.c), und die
/// Zahl der Faelle ist `ARRAY_SIZE` — also genau die Laenge des erzeugten
/// Feldes, nicht eine hingeschriebene Konstante.
pub fn table(h: i32, c: &mut Coex, st: &mut H2cState, share_ant: bool,
             force: bool, mut ty: u8) {
    c.cur_table = ty;

    if share_ant {
        if (ty as usize) < tables::COEX_TABLE_SANT.len() {
            let e = tables::COEX_TABLE_SANT[ty as usize];
            set_table(h, c, force, e[0], e[1]);
        }
    } else {
        ty -= 100;
        if (ty as usize) < tables::COEX_TABLE_NSANT.len() {
            let e = tables::COEX_TABLE_NSANT[ty as usize];
            set_table(h, c, force, e[0], e[1]);
        }
    }
    if c.wl_slot_toggle_change {
        wltoggle_table_a(h, st, share_ant, ty);
    }
}

// ── TDMA ─────────────────────────────────────────────────────────

/// coex.c `rtw_coex_wl_slot_extend`
fn wl_slot_extend(h: i32, c: &mut Coex, st: &mut H2cState, enable: bool) {
    let para1 = if enable {
        PARA1_H2C69_EN_5MS
    } else {
        c.cnt_wl_5ms_noextend = 0;
        PARA1_H2C69_DIS_5MS
    };
    c.wl_slot_extend = enable;
    crate::fw::bt_wifi_control(h, st, COEX_H2C69_WL_LEAKAP,
                               &[para1, 0, 0, 0, 0]);
}

/// coex.c `rtw_coex_wl_ccklock_action`.
///
/// Erreichbar nur aus `tdma_timer_base` und nur bei Basis 3 — beim Anlauf
/// ist die Basis 0. Sie steht hier vollstaendig, weil sie im Aufrufbaum
/// steht; `wl_fw_dbg_info` kommt aus einem C2H-Bericht, den erst der
/// laufende Betrieb holt, und ist hier 0.
fn wl_ccklock_action(h: i32, c: &mut Coex, st: &mut H2cState) {
    if c.manual_control || c.stop_dm {
        return;
    }
    if c.tdma_timer_base == 3 && c.wl_slot_extend {
        wl_slot_extend(h, c, st, false);
        return;
    }
    // `wl_cck_lock` und `wl_cck_lock_ever` kommen aus der Verkehrsmessung
    // des laufenden Betriebs und sind beim Anlauf beide false, der zweite
    // Zweig faellt damit ins Leere.
    let wl_cck_lock = false;
    let wl_cck_lock_ever = false;
    if c.wl_slot_extend && c.wl_force_lps_ctrl && !wl_cck_lock_ever {
        // wl_fw_dbg_info[7] ist hier 0, also <= 5
        c.cnt_wl_5ms_noextend += 1;
        if c.cnt_wl_5ms_noextend == 7 {
            wl_slot_extend(h, c, st, false);
        }
    } else if !c.wl_slot_extend && wl_cck_lock {
        wl_slot_extend(h, c, st, true);
    }
}

/// ps.c `rtw_leave_lps`.
///
/// `__rtw_leave_lps_deep` und `__rtw_leave_lps` pruefen beide
/// `RTW_FLAG_LEISURE_PS*` und kehren zurueck, wenn die Fahne nicht steht.
/// Wir schalten Leisure-PS nie ein, also ist das hier ein echter Nullweg —
/// und kein weggelassener, sondern einer, den Linux an dieser Stelle
/// genauso nimmt.
fn leave_lps(_h: i32) {}

/// coex.c `rtw_coex_power_save_state`
fn power_save_state(h: i32, c: &mut Coex, ps_type: u8) {
    const COEX_PS_WIFI_NATIVE: u8 = 0;
    const COEX_PS_LPS_OFF: u8 = 1;
    match ps_type {
        COEX_PS_WIFI_NATIVE => {
            c.wl_force_lps_ctrl = false;
            leave_lps(h);
        }
        COEX_PS_LPS_OFF => {
            c.wl_force_lps_ctrl = true;
            // `lps_conf.mode` ist 0, solange kein LPS laeuft.
            leave_lps(h);
        }
        _ => {}
    }
}

/// coex.c `rtw_coex_set_tdma`.
///
/// `ap_enable` ist in Linux eine lokale `false` — der AP-Zweig ist tot.
/// `chip->pstdma_type` ist `COEX_PSTDMA_FORCE_LPSOFF` (rtw8822c.c:5410).
#[allow(clippy::too_many_arguments)]
fn set_tdma(h: i32, c: &mut Coex, st: &mut H2cState,
            byte1: u8, byte2: u8, byte3: u8, byte4: u8, byte5: u8) {
    const COEX_PS_WIFI_NATIVE: u8 = 0;
    const COEX_PS_LPS_OFF: u8 = 1;
    const COEX_WLINK_2GFREE: u8 = 0x7; // coex.h:176

    let ap_enable = false;
    if ap_enable && (byte1 & (1 << 4) != 0 && byte1 & (1 << 5) == 0) {
        // Unerreichbar: `ap_enable` ist in Linux eine lokale Konstante.
        power_save_state(h, c, COEX_PS_WIFI_NATIVE);
    } else if (byte1 & (1 << 4) != 0 && byte1 & (1 << 5) == 0)
        || c.wl_coex_mode == COEX_WLINK_2GFREE
    {
        // pstdma_type == COEX_PSTDMA_FORCE_LPSOFF
        power_save_state(h, c, COEX_PS_LPS_OFF);
    } else {
        power_save_state(h, c, COEX_PS_WIFI_NATIVE);
    }

    c.ps_tdma_para = [byte1, byte2, byte3, byte4, byte5];
    crate::fw::coex_tdma_type(h, st, byte1, byte2, byte3, byte4, byte5);

    if byte1 & (1 << 2) != 0 {
        c.wl_slot_toggle = true;
        c.wl_slot_toggle_change = false;
    } else {
        c.wl_slot_toggle_change = c.wl_slot_toggle;
        c.wl_slot_toggle = false;
    }
}

/// coex.c `rtw_coex_tdma_timer_base`
fn tdma_timer_base(h: i32, c: &mut Coex, st: &mut H2cState, ty: u8) {
    if c.tdma_timer_base == ty {
        return;
    }
    c.tdma_timer_base = ty;

    let tbtt_interval = c.wl_beacon_interval;
    let para1: u8 = if ty == TDMA_TIMER_TYPE_4SLOT && tbtt_interval < 120 {
        PARA1_H2C69_TDMA_4SLOT
    } else if tbtt_interval < 80 && tbtt_interval > 0 {
        let mut times = 100 / tbtt_interval;
        if 100 % tbtt_interval != 0 {
            times += 1;
        }
        // FIELD_PREP(PARA1_H2C69_TBTT_TIMES, times), coex.h:28 = GENMASK(5,0)
        (times as u8) & 0x3f
    } else if tbtt_interval >= 180 {
        let mut times = tbtt_interval / 100;
        if tbtt_interval % 100 <= 80 {
            times -= 1;
        }
        // dazu PARA1_H2C69_TBTT_DIV100 = BIT(7), coex.h:29
        ((times as u8) & 0x3f) | (1 << 7)
    } else {
        PARA1_H2C69_TDMA_2SLOT
    };

    crate::fw::bt_wifi_control(h, st, COEX_H2C69_TDMA_SLOT,
                               &[para1, 0, 0, 0, 0]);

    // no 5ms_wl_slot_extend for 4-slot mode
    if c.tdma_timer_base == 3 {
        wl_ccklock_action(h, c, st);
    }
}

/// coex.c `rtw_coex_tdma`
pub fn tdma(h: i32, c: &mut Coex, st: &mut H2cState, share_ant: bool,
            force: bool, tcase: u32) {
    if tcase & TDMA_4SLOT != 0 {
        tdma_timer_base(h, c, st, TDMA_TIMER_TYPE_4SLOT);
    } else {
        tdma_timer_base(h, c, st, TDMA_TIMER_TYPE_2SLOT);
    }

    let ty = (tcase & 0xff) as u8;
    let turn_on = !(ty == 0 || ty == 100);

    if !force && turn_on == c.cur_ps_tdma_on && ty == c.cur_ps_tdma {
        return;
    }

    // `wl_busy` ist `RTW_FLAG_BUSY_TRAFFIC` und beim Anlauf false; der erste
    // Zweig gilt also, und `bt_a2dp_exist` braucht gar nicht geprueft zu
    // werden.
    let wl_busy = false;
    let bt_a2dp_exist = false;
    let bt_inq_remain = false;
    let bt_multi_link = false;
    if (bt_a2dp_exist && (bt_inq_remain || bt_multi_link)) || !wl_busy {
        write_scbd(h, c, COEX_SCBD_TDMA, false);
    } else {
        write_scbd(h, c, COEX_SCBD_TDMA, true);
    }

    c.cur_ps_tdma_on = turn_on;
    c.cur_ps_tdma = ty;

    if share_ant {
        if (ty as usize) < tables::COEX_TDMA_SANT.len() {
            let p = tables::COEX_TDMA_SANT[ty as usize];
            set_tdma(h, c, st, p[0], p[1], p[2], p[3], p[4]);
        }
    } else {
        let n = ty - 100;
        if (n as usize) < tables::COEX_TDMA_NSANT.len() {
            let p = tables::COEX_TDMA_NSANT[n as usize];
            set_tdma(h, c, st, p[0], p[1], p[2], p[3], p[4]);
        }
    }
}

/// coex.c `rtw_coex_query_bt_info`
pub fn query_bt_info(h: i32, c: &Coex, st: &mut H2cState) {
    if c.bt_disabled {
        return;
    }
    crate::fw::query_bt_info(h, st);
}

/// coex.c `rtw_coex_set_wl_pri_mask`
fn set_wl_pri_mask(h: i32, bitmap: u8, data: u8) {
    let addr = REG_BT_COEX_TABLE_H + (bitmap / 8) as u32;
    let bit = bitmap % 8;
    host::w8_mask(h, addr, 1 << bit, data);
}

// ── Die zwei Einstiege, die `rtw_power_on` ruft ──────────────────

/// coex.c `rtw_coex_power_on_setting`
pub fn power_on_setting(h: i32, c: &mut Coex, st: &mut H2cState,
                        share_ant: bool, rfe_option: u8) {
    let table_case = 1u8;

    c.stop_dm = true;
    c.wl_rf_off = false;

    // enable BB, we can write 0x948
    host::set8(h, REG_SYS_FUNC_EN, BIT_FEN_BB_GLB_RST | BIT_FEN_BB_RSTB);

    monitor_bt_enable(h, c);
    crate::chip::coex_cfg_rfe_type(h, c, share_ant, rfe_option);

    // set antenna path to BT
    set_ant_path(h, c, true, COEX_SET_ANT_POWERON);

    table(h, c, st, share_ant, true, table_case);
    // red x issue
    host::w8(h, 0xff1a, 0x0);
    crate::chip::coex_cfg_gnt_debug(h);
}

/// coex.c `__rtw_coex_init_hw_config`
pub fn init_hw_config(h: i32, c: &mut Coex, st: &mut H2cState,
                      share_ant: bool, wifi_only: bool, rfe_option: u8) {
    c.init_coex_var();

    c.kt_ver = (host::r8(h, 0xf1) >> 4) & 0xf; // u8_get_bits(..., GENMASK(7,4))

    monitor_bt_enable(h, c);
    wl_slot_extend(h, c, st, c.wl_slot_extend);

    host::set8(h, REG_BCN_CTRL, BIT_EN_BCN_FUNCTION);

    crate::chip::coex_cfg_rfe_type(h, c, share_ant, rfe_option);
    crate::chip::coex_cfg_init(h);

    // set Tx response = Hi-Pri (ex: Transmitting ACK,BA,CTS)
    set_wl_pri_mask(h, COEX_WLPRI_TX_RSP, 1);
    // set Tx beacon = Hi-Pri
    set_wl_pri_mask(h, COEX_WLPRI_TX_BEACON, 1);
    // set Tx beacon queue = Hi-Pri
    set_wl_pri_mask(h, COEX_WLPRI_TX_BEACONQ, 1);

    // antenna config
    if c.wl_rf_off {
        set_ant_path(h, c, true, COEX_SET_ANT_WOFF);
        write_scbd(h, c, COEX_SCBD_ALL, false);
        c.stop_dm = true;
    } else if wifi_only {
        set_ant_path(h, c, true, COEX_SET_ANT_WONLY);
        write_scbd(h, c, COEX_SCBD_ACTIVE | COEX_SCBD_ONOFF, true);
        c.stop_dm = true;
    } else {
        set_ant_path(h, c, true, COEX_SET_ANT_INIT);
        write_scbd(h, c, COEX_SCBD_ACTIVE | COEX_SCBD_ONOFF, true);
        c.stop_dm = false;
        c.freeze = true;
    }

    // PTA parameter
    table(h, c, st, share_ant, true, 1);
    tdma(h, c, st, share_ant, true, 0);
    query_bt_info(h, c, st);
}

// ── Zurueckgelesen: wo steht die Antenne wirklich? ───────────────

/// Der Zustand, den `set_ant_path` in der Hardware hinterlaesst.
///
/// **Das ist die Sache selbst, nicht ihr Nebeneffekt.** GNT_WL und GNT_BT
/// stehen im indirekten LTE-Registerraum, jedes zweimal (Bits 13:12 und 9:8
/// bzw. 15:14 und 11:10) — `set_gnt_wl`/`set_gnt_bt` schreiben beide Paare.
/// Der Besitzer des Pfads steht in `REG_SYS_SDIO_CTRL+3`.
pub struct AntState {
    pub lte_coex_ctrl: u32,
    pub gnt_wl: u32,
    pub gnt_bt: u32,
    pub wifi_owns_path: bool,
}

pub fn read_ant_state(h: i32) -> AntState {
    let v = read_indirect_reg(h, LTE_COEX_CTRL);
    AntState {
        lte_coex_ctrl: v,
        gnt_wl: (v & 0x3000) >> 12,
        gnt_bt: (v & 0xc000) >> 14,
        wifi_owns_path: host::r8(h, REG_SYS_SDIO_CTRL + 3)
            & (BIT_LTE_MUX_CTRL_PATH >> 24) as u8 != 0,
    }
}

/// Die ROHE Zahl im Postfach, ohne die Maske, die `read_scbd` anlegt.
/// `read_scbd` schneidet `BIT_BT_INT_EN` weg — und wer nur das Ergebnis
/// sieht, kann eine 0 nicht von „unser eigener Schreibzugriff kam nie an"
/// unterscheiden.
pub fn read_scbd_raw(h: i32) -> u16 {
    host::r16(h, REG_WIFI_BT_INFO)
}
