//! `coex.c` from Linux 6.18.26 rtw88: the part `rtw_power_on` runs, plus
//! the watchdog's monitoring.
//!
//! With `btcoex` and `share_ant` set in the efuse, WLAN and Bluetooth share
//! one antenna, and this code decides who gets it. Linux's first coex
//! action after power-on is "set antenna path to BT"
//! (`rtw_coex_power_on_setting`); only `rtw_coex_init_hw_config` takes it
//! back. Without this chain the switch stays where the firmware left it
//! and the receiver hears nothing.
//!
//! Ported, in call order: `rtw_coex_power_on_setting` ·
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

/// main.h:1587-1620 `struct rtw_coex_stat` and `rtw_coex_dm`: the fields
/// the bring-up path reads or writes. Runtime state (traffic statistics,
/// BT profiles, RSSI states) is not ported.
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
    /// coex.c:454-475 `rtw_coex_monitor_bt_ctr`: the four counters the
    /// watchdog collects every two seconds. They are the only sign of BT
    /// traffic when the scoreboard says nothing.
    pub hi_pri_tx: u16,
    pub hi_pri_rx: u16,
    pub lo_pri_tx: u16,
    pub lo_pri_rx: u16,
    pub bt_disable_cnt: u32,
    /// `wl_under_ips` / `wl_under_lps`: always false, since this driver runs
    /// neither IPS nor LPS.
    pub wl_under_ips: bool,
    pub wl_under_lps: bool,
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

/// coex.h:173-178 `enum coex_wl_link_mode`: the last marker follows 0x7,
/// so it is 8. `rtw_coex_init_coex_var` uses it as "no mode yet", and
/// `rtw8822c_coex_cfg_gnt_fix` compares against it.
const COEX_WLINK_MAX: u8 = 8; // coex.h:177
/// coex.h:101 `COEX_RSN_LPS = 13`
const COEX_RSN_LPS: u8 = 13;

impl Coex {
    pub const fn new() -> Self {
        Coex {
            stop_dm: false, wl_rf_off: false, freeze: false,
            manual_control: false, under_5g: false, freerun: false,
            bt_disabled: false, hi_pri_tx: 0, hi_pri_rx: 0,
            lo_pri_tx: 0, lo_pri_rx: 0, bt_disable_cnt: 0,
            wl_under_ips: false, wl_under_lps: false,
            bt_ble_scan_type: 0, bt_mailbox_reply: false,
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
    /// Linux `memset`s `coex_dm` and `coex_stat` and then sets three fields.
    /// The RSSI states `COEX_RSSI_STATE_LOW` = 0 coincide with the zeroing;
    /// `wl_rx_rate`/`wl_rts_rx_rate` are runtime state.
    fn init_coex_var(&mut self) {
        let keep_rfe = (self.rfe_module_type, self.wlg_at_btg,
                        self.ant_switch_exist);
        let stop_dm = self.stop_dm;
        let wl_rf_off = self.wl_rf_off;
        *self = Coex::new();
        // `coex` itself is not zeroed, only `coex_dm` and `coex_stat`.
        self.stop_dm = stop_dm;
        self.wl_rf_off = wl_rf_off;
        self.rfe_module_type = keep_rfe.0;
        self.wlg_at_btg = keep_rfe.1;
        self.ant_switch_exist = keep_rfe.2;
        self.wl_coex_mode = COEX_WLINK_MAX;
    }
}

// ── Scoreboard: the two-byte channel to the BT core ──────────────

/// coex.c `rtw_coex_read_scbd`. `chip->scbd_support` is `true` on the 8822C.
pub fn read_scbd(h: i32) -> u16 {
    host::r16(h, REG_WIFI_BT_INFO) & !BIT_BT_INT_EN
}

/// coex.c `rtw_coex_write_scbd`.
///
/// `new_scbd10_def` is `true` on the 8822C (rtw8822c.c:5407), so the
/// simple branch applies: `FIX2M` is set like any other bit. The 8822B is
/// the other way round, which is what the field is for.
pub fn write_scbd(h: i32, c: &mut Coex, bitpos: u16, set: bool) {
    let mut val: u16 = 0x2;
    val |= c.score_board;

    // new_scbd10_def == true -> the else branch
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

/// coex.c `rtw_coex_monitor_bt_enable`, branch with `scbd_support`.
///
/// The `bt_reenable_work` timer (15 s) lives on mac80211's work queue;
/// only the flag it sets is kept here. `rtw8822c_cfo_need_adjust` holds
/// off crystal tracking while Bluetooth is not disabled, so this must run
/// periodically or that lock never opens.
pub fn monitor_bt_enable(h: i32, c: &mut Coex) {
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

// ── Indirect LTE register space ──────────────────────────────────

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

// ── Who gets the antenna ─────────────────────────────────────────

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
/// `chip->btg_reg` is not set on the 8822C (no match in rtw8822c.c), so
/// the second write is omitted.
fn coex_ctrl_owner(h: i32, wifi_control: bool) {
    if wifi_control {
        host::set8(h, REG_SYS_SDIO_CTRL + 3, (BIT_LTE_MUX_CTRL_PATH >> 24) as u8);
    } else {
        host::clr8(h, REG_SYS_SDIO_CTRL + 3, (BIT_LTE_MUX_CTRL_PATH >> 24) as u8);
    }
}

/// coex.c `rtw_coex_check_rfk`.
///
/// Waits until neither BT nor WLAN is calibrating before the antenna path
/// owner changes. `wlg_at_btg` is true with a shared antenna and
/// `scbd_support` always is, so the branch applies.
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
/// `ant_switch_exist` is `false` on the 8822C (rtw8822c_coex_cfg_rfe_type)
/// and `coex_set_ant_switch` is `NULL`, so the switch depends only on
/// GNT_BT/GNT_WL and the path owner. `ctrl_type`/`pos_type` are still set
/// because Linux sets them.
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

    // `rtw_coex_set_ant_switch`: `chip->ops->coex_set_ant_switch` is NULL on
    // the 8822C (rtw8822c.c:4993) and `ant_switch_exist` is false. Either
    // reason alone means the call never happens.
    let _ = (ctrl_type, pos_type, COEX_SWITCH_CTRL_MAX, COEX_SWITCH_TO_MAX);
}

// ── Coexistence table ────────────────────────────────────────────

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

    // h2c_para[1] = 0x1 ("no definition"), then the four bytes of table_wl
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
/// The tables are generated (`gen_tables.py` from rtw8822c.c), and the
/// case count is `ARRAY_SIZE`, i.e. the length of the generated array.
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
/// Reachable only from `tdma_timer_base` with base 3; at bring-up the base
/// is 0. Ported in full because it is in the call tree. `wl_fw_dbg_info`
/// comes from a C2H report not collected here and is 0.
fn wl_ccklock_action(h: i32, c: &mut Coex, st: &mut H2cState) {
    if c.manual_control || c.stop_dm {
        return;
    }
    if c.tdma_timer_base == 3 && c.wl_slot_extend {
        wl_slot_extend(h, c, st, false);
        return;
    }
    // `wl_cck_lock` and `wl_cck_lock_ever` come from runtime traffic
    // statistics and are both false at bring-up, so the second branch does
    // nothing.
    let wl_cck_lock = false;
    let wl_cck_lock_ever = false;
    if c.wl_slot_extend && c.wl_force_lps_ctrl && !wl_cck_lock_ever {
        // wl_fw_dbg_info[7] is 0 here, so <= 5
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
/// `__rtw_leave_lps_deep` and `__rtw_leave_lps` both check
/// `RTW_FLAG_LEISURE_PS*` and return if it is not set. Leisure PS is never
/// enabled here, so this is a genuine no-op, the same path Linux takes.
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
            // `lps_conf.mode` is 0 while no LPS is active.
            leave_lps(h);
        }
        _ => {}
    }
}

/// coex.c `rtw_coex_set_tdma`.
///
/// `ap_enable` is a local `false` in Linux, so the AP branch is dead.
/// `chip->pstdma_type` is `COEX_PSTDMA_FORCE_LPSOFF` (rtw8822c.c:5410).
#[allow(clippy::too_many_arguments)]
fn set_tdma(h: i32, c: &mut Coex, st: &mut H2cState,
            byte1: u8, byte2: u8, byte3: u8, byte4: u8, byte5: u8) {
    const COEX_PS_WIFI_NATIVE: u8 = 0;
    const COEX_PS_LPS_OFF: u8 = 1;
    const COEX_WLINK_2GFREE: u8 = 0x7; // coex.h:176

    let ap_enable = false;
    if ap_enable && (byte1 & (1 << 4) != 0 && byte1 & (1 << 5) == 0) {
        // Unreachable: `ap_enable` is a local constant in Linux.
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
        // plus PARA1_H2C69_TBTT_DIV100 = BIT(7), coex.h:29
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

    // `wl_busy` is `RTW_FLAG_BUSY_TRAFFIC` and false at bring-up, so the
    // first branch applies and `bt_a2dp_exist` need not be checked.
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

// ── The two entry points `rtw_power_on` calls ────────────────────

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

// ── Read back: where is the antenna really? ──────────────────────

/// The state `set_ant_path` leaves in the hardware.
///
/// GNT_WL and GNT_BT live in the indirect LTE register space, each twice
/// (bits 13:12 and 9:8, resp. 15:14 and 11:10); `set_gnt_wl`/`set_gnt_bt`
/// write both pairs. The path owner is in `REG_SYS_SDIO_CTRL+3`.
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

/// The raw mailbox value, without the mask `read_scbd` applies.
/// `read_scbd` strips `BIT_BT_INT_EN`, so from its result alone a 0 cannot
/// be told apart from "our own write never arrived".
pub fn read_scbd_raw(h: i32) -> u16 {
    host::r16(h, REG_WIFI_BT_INFO)
}

// ═══════════════════════════════════════════════════════════════════
// What `rtw_watch_dog_work` does for coexistence every two seconds.
//
// The decision tree of `rtw_coex_run_coex` (antenna and TDMA while
// Bluetooth is active) is not ported. This is the monitoring part: the
// four traffic counters and the one state crystal tracking needs.
// ═══════════════════════════════════════════════════════════════════

/// coex.c:454-475 `rtw_coex_monitor_bt_ctr`
pub fn monitor_bt_ctr(h: i32, c: &mut Coex) {
    let tmp = host::r32(h, REG_BT_ACT_STATISTICS);
    c.hi_pri_tx = (tmp & 0xffff) as u16;
    c.hi_pri_rx = (tmp >> 16) as u16;

    let tmp = host::r32(h, REG_BT_ACT_STATISTICS_1);
    c.lo_pri_tx = (tmp & 0xffff) as u16;
    c.lo_pri_rx = (tmp >> 16) as u16;

    host::w8(h, REG_BT_COEX_ENH_INTR_CTRL,
             (BIT_R_GRANTALL_WLMASK | BIT_STATIS_BT_EN) as u8);
}

/// coex.c:3941-3950 `rtw_coex_wl_status_check`
pub fn wl_status_check(h: i32, c: &mut Coex) {
    if (c.wl_under_lps && !c.wl_force_lps_ctrl) || c.wl_under_ips {
        return;
    }
    monitor_bt_ctr(h, c);
}

/// coex.h:423-432 `rtw_coex_active_query_bt_info`.
///
/// A no-op for this chip, as in Linux: the function only queries on the
/// RTL8821AU, whose firmware does not send `C2H_BT_INFO` by itself when BT
/// headphones disconnect.
pub fn active_query_bt_info(_h: i32, _c: &mut Coex) {}
