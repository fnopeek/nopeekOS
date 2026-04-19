//! BB (BaseBand) helpers used by TSSI alimentk.
//!
//! Port of the rtw8852bx BB helper functions from
//! drivers/net/wireless/realtek/rtw89/rtw8852b_common.c:
//!
//!   __rtw8852bx_bb_set_plcp_tx       — load PMAC HT20 MCS7 table
//!   __rtw8852bx_bb_cfg_tx_path       — route TX chain to path A/B/AB
//!   __rtw8852bx_bb_ctrl_rx_path      — route RX chain (minimal variant)
//!   __rtw8852bx_bb_set_power         — set test-TX power in R_TXPWR
//!   __rtw8852bx_bb_set_pmac_pkt_tx   — start/stop PMAC test-TX
//!   __rtw8852bx_bb_backup_tssi       — snapshot 7 BB registers
//!   __rtw8852bx_bb_restore_tssi      — restore them
//!
//! These are only used by the TSSI alimentk cal loop; normal traffic TX
//! uses the CH8 ring path and doesn't touch these registers.

use crate::host;
use crate::phy::PHY_CR_BASE;
use crate::bb_tables::PMAC_HT20_MCS7_TBL;

// ── Register addresses (reg.h, PHY space, +PHY_CR_BASE at access) ─

pub const R_RSTB_ASYNC:       u32 = 0x0704;
pub const B_RSTB_ASYNC_ALL:   u32 = 1 << 1;

pub const R_PMAC_GNT:         u32 = 0x0980;
pub const B_PMAC_GNT_TXEN:    u32 = 1 << 0;
pub const B_PMAC_GNT_RXEN:    u32 = 1 << 4;
pub const B_PMAC_GNT_P1:      u32 = 0xF00;

pub const R_PMAC_RX_CFG1:     u32 = 0x0988;
pub const B_PMAC_OPT1_MSK:    u32 = 0x0000_003F;

pub const R_PMAC_TX_CTRL:     u32 = 0x09C0;
pub const B_PMAC_TXEN_DIS:    u32 = 1 << 0;

pub const R_PMAC_TX_PRD:      u32 = 0x09C4;
pub const B_PMAC_CTX_EN:      u32 = 1 << 0;
pub const B_PMAC_PTX_EN:      u32 = 1 << 1;
pub const B_PMAC_TX_PRD_MSK:  u32 = 0xFFFF_0000;

pub const R_PMAC_TX_CNT:      u32 = 0x09C8;
pub const B_PMAC_TX_CNT_MSK:  u32 = 0xFFFF_FFFF;

pub const R_RXHT_MCS_LIMIT:   u32 = 0x0D18;
pub const R_RXVHT_MCS_LIMIT:  u32 = 0x0D18;
pub const B_RXHT_MCS_LIMIT:   u32 = 0x07E0_0000;
pub const B_RXVHT_MCS_LIMIT:  u32 = 0x003F_8000;

pub const R_RXHE:             u32 = 0x0D80;
pub const B_RXHE_USER_MAX:    u32 = 0x0000_01C0;
pub const B_RXHE_MAX_NSS:     u32 = 0x0000_3800;
pub const B_RXHETB_MAX_NSS:   u32 = 0x000E_0000;

pub const R_P0_RFMODE:        u32 = 0x12AC;
pub const R_P0_RFMODE_FTM_RX: u32 = 0x12B0;
pub const R_P1_RFMODE:        u32 = 0x32AC;
pub const R_P1_RFMODE_FTM_RX: u32 = 0x32B0;

pub const R_TX_COUNTER:       u32 = 0x1A40;

pub const R_TXPATH_SEL:       u32 = 0x458C;
pub const B_TXPATH_SEL_MSK:   u32 = 0xF000_0000;

pub const R_TXPWR:            u32 = 0x4594;
pub const B_TXPWR_MSK:        u32 = 0x0001_FF00;

pub const R_TXNSS_MAP:        u32 = 0x45B4;
pub const B_TXNSS_MAP_MSK:    u32 = 0x0000_0780;

pub const R_MAC_SEL:          u32 = 0x09A4;
pub const B_MAC_SEL_PWR_EN:   u32 = 1 << 7;
pub const B_MAC_SEL_MOD:      u32 = 0x000000E0;

pub const R_PD_CTRL:          u32 = 0x0C3C;
pub const B_PD_HIT_DIS:       u32 = 1 << 9;

pub const R_RXCCA:            u32 = 0x2344;
pub const B_RXCCA_DIS:        u32 = 1 << 31;

pub const R_CHBW_MOD_V1:      u32 = 0x49C4;
pub const B_ANT_RX_SEG0:      u32 = 0x0000_0300;

// RF path selector (passed to bb_cfg_tx_path / ctrl_rx_path).
pub const RF_PATH_A:  u8 = 0;
pub const RF_PATH_B:  u8 = 1;
pub const RF_PATH_AB: u8 = 2;

// ── Helpers ─────────────────────────────────────────────────────

fn pw(mmio: i32, addr: u32, val: u32) {
    host::mmio_w32(mmio, PHY_CR_BASE + addr, val);
}
fn pwm(mmio: i32, addr: u32, mask: u32, val: u32) {
    host::mmio_w32_mask(mmio, PHY_CR_BASE + addr, mask, val);
}
fn pset(mmio: i32, addr: u32, bits: u32) {
    host::mmio_set32(mmio, PHY_CR_BASE + addr, bits);
}
fn pclr(mmio: i32, addr: u32, bits: u32) {
    host::mmio_clr32(mmio, PHY_CR_BASE + addr, bits);
}
fn pr(mmio: i32, addr: u32) -> u32 {
    host::mmio_r32(mmio, PHY_CR_BASE + addr)
}

// ── Public API ──────────────────────────────────────────────────

/// __rtw8852bx_bb_set_plcp_tx (common.c:1433).
/// Applies 120-entry HT20 MCS7 preset table — sets up the PMAC PLCP
/// generator for the test TX sweeps done by TSSI alimentk.
pub fn set_plcp_tx(mmio: i32) {
    for &(addr, mask, data) in PMAC_HT20_MCS7_TBL {
        pwm(mmio, addr, mask, data);
    }
}

/// __rtw8852bx_bb_cfg_tx_path (common.c:1531). MAC_SEL_MOD=7 enables
/// PMAC TX. Path=A/B/AB sets the TX routing.
pub fn cfg_tx_path(mmio: i32, path: u8) {
    pwm(mmio, R_MAC_SEL, B_MAC_SEL_MOD, 7);
    match path {
        RF_PATH_A => {
            pwm(mmio, R_TXPATH_SEL, B_TXPATH_SEL_MSK, 1);
            pwm(mmio, R_TXNSS_MAP,  B_TXNSS_MAP_MSK, 0);
        }
        RF_PATH_B => {
            pwm(mmio, R_TXPATH_SEL, B_TXPATH_SEL_MSK, 2);
            pwm(mmio, R_TXNSS_MAP,  B_TXNSS_MAP_MSK, 0);
        }
        RF_PATH_AB => {
            pwm(mmio, R_TXPATH_SEL, B_TXPATH_SEL_MSK, 3);
            pwm(mmio, R_TXNSS_MAP,  B_TXNSS_MAP_MSK, 4);
        }
        _ => {}
    }
}

/// __rtw8852bx_bb_ctrl_rx_path (common.c:1655). Minimal variant —
/// sets ANT_RX_SEG0 + 1RCCA_SEG0/1 + MCS_LIMIT + RXHE fields per
/// rx_path (RF_A=1, RF_B=2, RF_AB=3).
pub fn ctrl_rx_path(mmio: i32, rx_path: u8) {
    let (seg, mcs_lim, he_max_nss) = match rx_path {
        RF_PATH_A  => (1, 0, 0),
        RF_PATH_B  => (2, 0, 0),
        RF_PATH_AB => (3, 1, 1),
        _          => (3, 1, 1),
    };
    pwm(mmio, R_CHBW_MOD_V1, B_ANT_RX_SEG0, seg);
    pwm(mmio, 0x49C0, 0x0000_0300, seg);  // R_FC0_BW_V1.B_ANT_RX_1RCCA_SEG0
    pwm(mmio, 0x49C0, 0x0000_0C00, seg);  // B_ANT_RX_1RCCA_SEG1
    pwm(mmio, R_RXHT_MCS_LIMIT,  B_RXHT_MCS_LIMIT,  mcs_lim);
    pwm(mmio, R_RXVHT_MCS_LIMIT, B_RXVHT_MCS_LIMIT, mcs_lim);
    pwm(mmio, R_RXHE, B_RXHE_USER_MAX,  4);
    pwm(mmio, R_RXHE, B_RXHE_MAX_NSS,   he_max_nss);
    pwm(mmio, R_RXHE, B_RXHETB_MAX_NSS, he_max_nss);
}

/// __rtw8852bx_bb_set_power (common.c:1521). Writes pwr_dbm into
/// R_TXPWR. Used by alimentk to sweep power levels.
pub fn set_power(mmio: i32, pwr_dbm: i16) {
    pwm(mmio, R_MAC_SEL, B_MAC_SEL_PWR_EN, 1);
    pwm(mmio, R_TXPWR,   B_TXPWR_MSK,     (pwr_dbm as u32) & 0x1FF);
}

/// __rtw8852bx_bb_set_pmac_pkt_tx (common.c:1504). Enable/disable the
/// PMAC test-TX packet generator with cnt packets per period.
pub fn set_pmac_pkt_tx(mmio: i32, enable: bool, cnt: u16, period: u16) {
    if !enable {
        // Stop: clear PTX_EN (PKTS_TX mode always in alimentk).
        pwm(mmio, R_PMAC_TX_PRD, B_PMAC_PTX_EN, 0);
        pwm(mmio, R_PD_CTRL,     B_PD_HIT_DIS,  0);
        pclr(mmio, R_RXCCA,      B_RXCCA_DIS);
        return;
    }

    // Enable path: prepare MAC, pause PD, set TX-only CCA, then kick off.
    pwm(mmio, R_PMAC_GNT,   B_PMAC_GNT_TXEN,  1);
    pwm(mmio, R_PMAC_GNT,   B_PMAC_GNT_RXEN,  1);
    pwm(mmio, R_PMAC_RX_CFG1, B_PMAC_OPT1_MSK, 0x3F);
    pwm(mmio, R_RSTB_ASYNC, B_RSTB_ASYNC_ALL, 0);
    pwm(mmio, R_PD_CTRL,    B_PD_HIT_DIS,     1);
    pset(mmio, R_RXCCA,     B_RXCCA_DIS);
    pwm(mmio, R_RSTB_ASYNC, B_RSTB_ASYNC_ALL, 1);

    // Start PKTS_TX mode
    pwm(mmio, R_PMAC_TX_PRD, B_PMAC_PTX_EN, 1);
    pwm(mmio, R_PMAC_TX_PRD, B_PMAC_TX_PRD_MSK, period as u32);
    pwm(mmio, R_PMAC_TX_CNT, B_PMAC_TX_CNT_MSK, cnt as u32);

    // Pulse TXEN_DIS to force packet gen start
    pwm(mmio, R_PMAC_TX_CTRL, B_PMAC_TXEN_DIS, 1);
    pwm(mmio, R_PMAC_TX_CTRL, B_PMAC_TXEN_DIS, 0);
}

/// Snapshot of BB registers touched by alimentk for save/restore.
#[derive(Copy, Clone, Default)]
pub struct TssiBak {
    pub tx_path:       u32,
    pub rx_path:       u32,
    pub p0_rfmode:     u32,
    pub p0_rfmode_ftm: u32,
    pub p1_rfmode:     u32,
    pub p1_rfmode_ftm: u32,
    pub tx_pwr:        i32,  // sign-extended s9
}

/// __rtw8852bx_bb_backup_tssi (common.c:1570).
pub fn backup_tssi(mmio: i32) -> TssiBak {
    let mut b = TssiBak::default();
    b.tx_path       = (pr(mmio, R_TXPATH_SEL)   & B_TXPATH_SEL_MSK) >> 28;
    b.rx_path       = (pr(mmio, R_CHBW_MOD_V1) & B_ANT_RX_SEG0)    >> 8;
    b.p0_rfmode     = pr(mmio, R_P0_RFMODE);
    b.p0_rfmode_ftm = pr(mmio, R_P0_RFMODE_FTM_RX);
    b.p1_rfmode     = pr(mmio, R_P1_RFMODE);
    b.p1_rfmode_ftm = pr(mmio, R_P1_RFMODE_FTM_RX);
    // Sign-extend 9-bit signed power
    let raw = ((pr(mmio, R_TXPWR) & B_TXPWR_MSK) >> 8) & 0x1FF;
    b.tx_pwr = if raw & 0x100 != 0 { (raw | 0xFFFF_FE00u32) as i32 } else { raw as i32 };
    b
}

/// __rtw8852bx_bb_restore_tssi (common.c:1586).
pub fn restore_tssi(mmio: i32, b: &TssiBak) {
    pwm(mmio, R_TXPATH_SEL, B_TXPATH_SEL_MSK, b.tx_path);
    if b.tx_path == 3 /* RF_AB */ {
        pwm(mmio, R_TXNSS_MAP, B_TXNSS_MAP_MSK, 4);
    } else {
        pwm(mmio, R_TXNSS_MAP, B_TXNSS_MAP_MSK, 0);
    }
    pwm(mmio, R_CHBW_MOD_V1, B_ANT_RX_SEG0, b.rx_path);
    pwm(mmio, R_MAC_SEL, B_MAC_SEL_PWR_EN, 1);
    pw(mmio, R_P0_RFMODE,        b.p0_rfmode);
    pw(mmio, R_P0_RFMODE_FTM_RX, b.p0_rfmode_ftm);
    pw(mmio, R_P1_RFMODE,        b.p1_rfmode);
    pw(mmio, R_P1_RFMODE_FTM_RX, b.p1_rfmode_ftm);
    pwm(mmio, R_TXPWR, B_TXPWR_MSK, (b.tx_pwr as u32) & 0x1FF);
}
