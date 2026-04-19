//! DPK — Digital Pre-Distortion, full 1:1 port of Linux rtw8852b_rfk.c
//! (rfk.c:1649..2587, the complete _dpk flow).
//!
//! Linux entry: rtw8852b_dpk → _dpk → _dpk_bypass_check ? _dpk_force_bypass
//! : _dpk_cal_select → for each path: _dpk_main → _dpk_agc → _dpk_idl_mpa
//! → _dpk_fill_result.
//!
//! State: rtw89_dpk_info + bp[path][kidx] backup entries, cur_idx[path],
//! dpk_gs[phy], corr_idx/corr_val/dc_i/dc_q arrays.
//!
//! The cal loop sends PMAC test-TX bursts and reads feedback via the
//! DPK sync/gainloss/PAS channels. Each one-shot writes a cmd-ID to
//! R_NCTL_CFG and polls 0xBFF8 for 0x55 (same path as IQK).

use crate::host;
use crate::fw;
use crate::phy::{rf_read, rf_write_mask, PHY_CR_BASE};
use crate::dpk_tables::{
    RTW8852B_DPK_AFE_DEFS,
    RTW8852B_DPK_AFE_RESTORE_DEFS,
    RTW8852B_DPK_KIP_DEFS,
};

// ── State ─────────────────────────────────────────────────────────
const BKUP_NUM: usize = 2;        // RTW89_DPK_BKUP_NUM
const DPK_RF_PATH: usize = 2;     // RTW8852B_DPK_RF_PATH
const KIP_REG_NUM: usize = 3;     // RTW8852B_DPK_KIP_REG_NUM
const BACKUP_BB_NR: usize = 3;
const BACKUP_RF_NR: usize = 11;

#[derive(Copy, Clone, Default)]
struct Bp {
    ch: u8, band: u8, bw: u8,
    path_ok: bool,
    ther_dpk: u8,
    gs: u8,
    pwsf: u16,
    txagc_dpk: u8,
}

struct DpkInfo {
    is_dpk_enable: bool,
    is_dpk_reload_en: bool,
    cur_idx: [u8; 2],
    bp: [[Bp; BKUP_NUM]; 2],
    dpk_gs: [u8; 2],
    corr_idx: [[u8; BKUP_NUM]; 2],
    corr_val: [[u8; BKUP_NUM]; 2],
    dc_i: [[u16; BKUP_NUM]; 2],
    dc_q: [[u16; BKUP_NUM]; 2],
}

impl DpkInfo {
    const fn new() -> Self {
        Self {
            is_dpk_enable: true,
            is_dpk_reload_en: false,
            cur_idx: [0; 2],
            bp: [[Bp { ch: 0, band: 0, bw: 0, path_ok: false,
                      ther_dpk: 0, gs: 0, pwsf: 0, txagc_dpk: 0 }; BKUP_NUM]; 2],
            dpk_gs: [0x7f; 2],
            corr_idx: [[0; BKUP_NUM]; 2],
            corr_val: [[0; BKUP_NUM]; 2],
            dc_i: [[0; BKUP_NUM]; 2],
            dc_q: [[0; BKUP_NUM]; 2],
        }
    }
}

static mut DPK: DpkInfo = DpkInfo::new();
fn st() -> &'static mut DpkInfo { unsafe { &mut *core::ptr::addr_of_mut!(DPK) } }

// ── BB register addresses (reg.h) ──────────────────────────────────
const R_DPD_BF:            u32 = 0x44A0;
const B_DPD_BF_OFDM:       u32 = 0x0001_F000;
const B_DPD_BF_SCA:        u32 = 0x0000_007F;

const R_DPD_CH0:           u32 = 0x81AC;
const R_DPD_CH0A:          u32 = 0x81BC;
const R_DPD_V1:            u32 = 0x81A0;
const R_DPD_COM:           u32 = 0x81C8;
const R_DPD_BND:           u32 = 0x81B4;
const B_DPD_CFG:           u32 = 0x007F_FFFF;
const B_DPD_COM_OF:        u32 = 1 << 15;
const B_DPD_ORDER_V1:      u32 = 0x3 << 25;

const R_MDPK_RX_DCK:       u32 = 0x8074;
const B_MDPK_RX_DCK_EN:    u32 = 1 << 31;
const R_MDPK_SYNC:         u32 = 0x8070;
const B_MDPK_SYNC_MAN:     u32 = 0xF << 28;
const B_MDPK_SYNC_SEL:     u32 = 1 << 31;

const R_LDL_NORM:          u32 = 0x80A0;
const B_LDL_NORM_OP:       u32 = 0x3;
const B_LDL_NORM_PN:       u32 = 0x1F << 8;

const R_TPG_MOD:           u32 = 0x806C;
const B_TPG_MOD_F:         u32 = 0x3 << 1;

const R_RPT_COM:           u32 = 0x80FC;
const B_PRT_COM_CORI:      u32 = 0xFF << 0;
const B_PRT_COM_CORV:      u32 = 0xFF << 8;
const B_PRT_COM_DCI:       u32 = 0xFFF << 16;
const B_PRT_COM_DCQ:       u32 = 0xFFF << 0;
const B_PRT_COM_GL:        u32 = 0xF << 4;
const B_PRT_COM_RXBB_V1:   u32 = 0x1F << 0;

const R_KIP_RPT1:          u32 = 0x80D4;
const B_KIP_RPT1_SEL:      u32 = 0x3F << 16;
const B_KIP_RPT1_SEL_V1:   u32 = 0xF << 16;

const R_DPK_CFG2:          u32 = 0x80BC;
const B_DPK_CFG2_ST:       u32 = 1 << 14;
const R_DPK_CFG3:          u32 = 0x80C0;

const R_KPATH_CFG:         u32 = 0x80D0;
const B_KPATH_CFG_ED:      u32 = 0x3 << 20;

const R_LOAD_COEF:         u32 = 0x81DC;
const B_LOAD_COEF_MDPD:    u32 = 1 << 16;
const B_LOAD_COEF_DI:      u32 = 1 << 1;

const R_TXAGC_RFK:         u32 = 0x81C4;
const R_KIP_MOD:           u32 = 0x8078;
const B_KIP_MOD:           u32 = 0x000F_FFFF;

const R_P0_CFCH_BW1:       u32 = 0xC0D8;
const B_P0_CFCH_EX:        u32 = 1 << 13;
const R_PATH1_BW_SEL_V1:   u32 = 0xC1D8;
const B_PATH1_BW_SEL_EX:   u32 = 1 << 13;

const R_P0_TSSI_TRK:       u32 = 0x5818;
const B_P0_TSSI_TRK_EN:    u32 = 1 << 30;

const R_COEF_SEL:          u32 = 0x8104;
const B_COEF_SEL_MDPD:     u32 = 1 << 8;

// Shared with iqk.rs:
const R_NCTL_CFG:          u32 = 0x8000;
const R_NCTL_N1:           u32 = 0x8010;
const R_NCTL_RPT:          u32 = 0x8008;
const R_KIP_SYSCFG:        u32 = 0x8088;
const R_CFIR_SYS:          u32 = 0x8120;
const R_IQK_RES:           u32 = 0x8124;
const B_IQK_RES_RXCFIR:    u32 = 0xF;
const R_IQK_DIF4:          u32 = 0x802C;
const B_IQK_DIF4_RXT:      u32 = 0xFFF << 16;
const R_RXIQC:             u32 = 0x813C;
const B_RXIQC_BYPASS:      u32 = 1 << 0;
const B_RXIQC_BYPASS2:     u32 = 1 << 2;
const R_P0_RFCTM:          u32 = 0x5864;
const B_P0_RFCTM_EN:       u32 = 1 << 29;
const R_RFK_ST:            u32 = 0xBFF8;

// ── RF register defs ───────────────────────────────────────────────
const RR_MOD:              u32 = 0x00;
const RR_MOD_MASK:         u32 = 0xF0000;
const RFREG_MASKRXBB:      u32 = 0x003E0;
const RFREG_MASKMODE:      u32 = 0xF0000;
const RR_MOD_V_RX:         u32 = 0x3;
const RR_RSV1:             u32 = 0x05;
const RR_RSV1_RST:         u32 = 1 << 0;
const RR_BBDC:             u32 = 0x10005;
const RR_BBDC_SEL:         u32 = 1 << 0;
const RR_CFGCH:            u32 = 0x18;
const RR_RSV4:             u32 = 0x1F;
const RR_RXK:              u32 = 0x20;
const RR_RXK_PLLEN:        u32 = 1 << 5;
const RR_TM:               u32 = 0x42;
const RR_TM_TRI:           u32 = 1 << 19;
const RR_TM_VAL:           u32 = 0x3F << 1;
const RR_TXG1:             u32 = 0x51;         // (not used here but kept for parity)
const RR_RXBB:             u32 = 0x83;
const RR_RXBB_FATT:        u32 = 0xFF;
const RR_XGLNA2:           u32 = 0x85;
const RR_XGLNA2_SW:        u32 = 0x3;
const RR_RXA_LNA:          u32 = 0x8B;
const RR_RXA2:             u32 = 0x8C;
const RR_RAA2_SWATT:       u32 = 0x7F << 9;
const RR_RXBB2:            u32 = 0x8F;
const RR_EN_TIA_IDA:       u32 = 0x3 << 10;
const RR_XALNA2:           u32 = 0x90;
const RR_IQGEN:            u32 = 0x97;
const RR_TXIQK:            u32 = 0x98;
const RR_TXIQK_ATT1:       u32 = 0x7F;
const RR_TIA:              u32 = 0x9E;
const RR_TIA_N6:           u32 = 1 << 8;
const RR_RCKD:             u32 = 0xDE;
const RR_RCKD_BW:          u32 = 1 << 2;
const RR_LUTDBG:           u32 = 0xDF;
const RR_LUTDBG_TIA:       u32 = 1 << 12;
const RR_BTC:              u32 = 0x1A;
const RR_BTC_TXBB:         u32 = 0x7 << 12;
const RR_BTC_RXBB:         u32 = 0x3 << 10;
const RR_DCK1:             u32 = 0x1D;
const RR_DCK1_CLR:         u32 = 0x7 << 5;
const RR_DCK:              u32 = 0x1C;
const RR_DCK_LV:           u32 = 0x7 << 5;
const RR_RXKPLL:           u32 = 0x1E;
const RR_RXKPLL_POW:       u32 = 1 << 19;
const RR_TXAGC:            u32 = 0x10001;

const RFREG_MASK:          u32 = 0xFFFFF;

// ── One-shot IDs (rtw8852b_dpk_id) ─────────────────────────────────
const LBK_RXIQK:     u32 = 0x06;
const SYNC:          u32 = 0x10;
const MDPK_IDL:      u32 = 0x11;
const GAIN_LOSS:     u32 = 0x13;
const DPK_RXAGC:     u32 = 0x15;
const KIP_PRESET:    u32 = 0x16;
const DPK_TXAGC_ID:  u32 = 0x19;

const DPK_TXAGC_LOWER: u8 = 0x2E;
const DPK_TXAGC_UPPER: u8 = 0x3F;

// Sync thresholds
const DPK_SYNC_TH_DC_I: u16 = 200;
const DPK_SYNC_TH_DC_Q: u16 = 200;
const DPK_SYNC_TH_CORR: u8  = 170;

// AGC FSM steps
const AGC_SYNC_DGAIN: u8       = 0;
const AGC_GAIN_ADJ: u8         = 1;
const AGC_GAIN_LOSS_IDX: u8    = 2;
const AGC_GL_GT_CRITERION: u8  = 3;
const AGC_GL_LT_CRITERION: u8  = 4;
const AGC_SET_TX_GAIN: u8      = 5;

// ── I/O helpers ────────────────────────────────────────────────────
fn pr(mmio: i32, a: u32) -> u32 { host::mmio_r32(mmio, PHY_CR_BASE + a) }
fn pw(mmio: i32, a: u32, v: u32) { host::mmio_w32(mmio, PHY_CR_BASE + a, v) }
fn pwm(mmio: i32, a: u32, m: u32, v: u32) { host::mmio_w32_mask(mmio, PHY_CR_BASE + a, m, v) }
fn pclr(mmio: i32, a: u32, m: u32) {
    let cur = pr(mmio, a); pw(mmio, a, cur & !m);
}
fn pset(mmio: i32, a: u32, m: u32) {
    let cur = pr(mmio, a); pw(mmio, a, cur | m);
}
fn rr(mmio: i32, path: u8, a: u32) -> u32 { rf_read(mmio, path, a) }
fn rw(mmio: i32, path: u8, a: u32, m: u32, v: u32) { rf_write_mask(mmio, path, a, m, v) }

fn udelay_1() { for _ in 0..1000 { core::hint::spin_loop(); } }
fn udelay_200() { for _ in 0..200000 { core::hint::spin_loop(); } }
fn udelay_70() { for _ in 0..70000 { core::hint::spin_loop(); } }
fn mdelay_1() { for _ in 0..1_000_000 { core::hint::spin_loop(); } }

fn apply(mmio: i32, tbl: &[(u32, u32, u32)]) {
    for &(a, m, v) in tbl { pwm(mmio, a, m, v); }
}

fn sign_ext(v: u32, bits: u32) -> i32 {
    let sh = 32 - bits;
    ((v << sh) as i32) >> sh
}

fn abs_s(v: i32) -> u16 { v.unsigned_abs().min(0xFFFF) as u16 }

fn field(raw: u32, mask: u32) -> u32 {
    if mask == 0 { 0 } else { (raw & mask) >> mask.trailing_zeros() }
}

// ═══════════════════════════════════════════════════════════════════
//  _set_rx_dck (rfk.c:295) — shared by IQK/DPK
// ═══════════════════════════════════════════════════════════════════
fn set_rx_dck(mmio: i32, path: u8) {
    rw(mmio, path, RR_DCK1, RR_DCK1_CLR, 0x0);
    rw(mmio, path, RR_DCK,  RR_DCK_LV,   0x0);
    rw(mmio, path, RR_DCK,  RR_DCK_LV,   0x1);
    mdelay_1();
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_order_convert (rfk.c:1675)
// ═══════════════════════════════════════════════════════════════════
fn order_convert(mmio: i32) -> u32 {
    let order = pr(mmio, R_LDL_NORM) & B_LDL_NORM_OP;
    3u32 >> order
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_onoff (rfk.c:1688)
// ═══════════════════════════════════════════════════════════════════
fn onoff(mmio: i32, path: u8, off: bool) {
    let kidx = st().cur_idx[path as usize] as u32;
    let path_ok = st().bp[path as usize][kidx as usize].path_ok;
    let val: u32 = if st().is_dpk_enable && !off && path_ok { 1 } else { 0 };
    // MASKBYTE3 of (R_DPD_CH0A + path*0x100 + kidx*4) = bits[31:24].
    // Byte value = (order << 1) | val
    let reg = R_DPD_CH0A + ((path as u32) << 8) + (kidx << 2);
    let byte = (order_convert(mmio) << 1) | val;
    pwm(mmio, reg, 0xFF << 24, byte);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_one_shot (rfk.c:1702)
// ═══════════════════════════════════════════════════════════════════
fn one_shot(mmio: i32, path: u8, id: u32) {
    let cmd = (id << 8) | (0x19 + ((path as u32) << 4));
    pw(mmio, R_NCTL_CFG, cmd);

    // Wait up to 20ms for 0xBFF8 byte0 == 0x55
    let mut ok = false;
    for _ in 0..20000u32 {
        let v = pr(mmio, R_RFK_ST) & 0xFF;
        if v == 0x55 { ok = true; break; }
        udelay_1();
    }
    if !ok {
        host::print("    [dpk] one-shot id=");
        fw::print_dec(id as usize);
        host::print(" poll timeout (20ms)\n");
    }
    udelay_1();

    // Secondary poll on R_RPT_COM low16 == 0x8000 (2ms)
    pw(mmio, R_KIP_RPT1, 0x00030000);
    for _ in 0..2000u32 {
        let v = pr(mmio, R_RPT_COM) & 0xFFFF;
        if v == 0x8000 { break; }
        udelay_1();
    }

    pwm(mmio, R_NCTL_N1, 0xFF, 0);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_rx_dck (rfk.c:1744)
// ═══════════════════════════════════════════════════════════════════
fn rx_dck(mmio: i32, path: u8) {
    rw(mmio, path, RR_RXBB2, RR_EN_TIA_IDA, 0x3);
    set_rx_dck(mmio, path);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_information (rfk.c:1751) — bookkeeping only
// ═══════════════════════════════════════════════════════════════════
fn information(mmio: i32, path: u8, band: u8, ch: u8, bw: u8) {
    let _ = mmio;
    let kidx = st().cur_idx[path as usize] as usize;
    st().bp[path as usize][kidx].band = band;
    st().bp[path as usize][kidx].ch = ch;
    st().bp[path as usize][kidx].bw = bw;
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_bb_afe_setting / _restore (rfk.c:1775/1793)
// ═══════════════════════════════════════════════════════════════════
fn bb_afe_setting(mmio: i32, bw: u8) {
    apply(mmio, RTW8852B_DPK_AFE_DEFS);
    if bw == 2 /* RTW89_CHANNEL_WIDTH_80 */ {
        pwm(mmio, R_P0_CFCH_BW1,     B_P0_CFCH_EX,     0x1);
        pwm(mmio, R_PATH1_BW_SEL_V1, B_PATH1_BW_SEL_EX, 0x1);
    }
}

fn bb_afe_restore(mmio: i32, bw: u8) {
    apply(mmio, RTW8852B_DPK_AFE_RESTORE_DEFS);
    if bw == 2 {
        pwm(mmio, R_P0_CFCH_BW1,     B_P0_CFCH_EX,     0x0);
        pwm(mmio, R_PATH1_BW_SEL_V1, B_PATH1_BW_SEL_EX, 0x0);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_tssi_pause (rfk.c:1811) — pause/resume TSSI tracking per path
// ═══════════════════════════════════════════════════════════════════
fn tssi_pause(mmio: i32, path: u8, is_pause: bool) {
    let reg = R_P0_TSSI_TRK + ((path as u32) << 13);
    let v: u32 = if is_pause { 1 } else { 0 };
    pwm(mmio, reg, B_P0_TSSI_TRK_EN, v);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_kip_restore (rfk.c:1821)
// ═══════════════════════════════════════════════════════════════════
fn kip_restore(mmio: i32, path: u8) {
    apply(mmio, RTW8852B_DPK_KIP_DEFS);
    // cv > CHIP_CAV: our hal reports cv=2 so always apply.
    pwm(mmio, R_DPD_COM + ((path as u32) << 8), B_DPD_COM_OF, 0x1);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_lbk_rxiqk (rfk.c:1832)
// ═══════════════════════════════════════════════════════════════════
fn lbk_rxiqk(mmio: i32, path: u8) {
    let cur_rxbb = rr(mmio, path, RR_MOD) & RFREG_MASKRXBB;
    let cur_rxbb = (cur_rxbb >> RFREG_MASKRXBB.trailing_zeros()) as u8;

    pwm(mmio, R_MDPK_RX_DCK, B_MDPK_RX_DCK_EN, 0x1);
    pwm(mmio, R_IQK_RES + ((path as u32) << 8), B_IQK_RES_RXCFIR, 0x0);

    let cfgch = rr(mmio, path, RR_CFGCH);
    rw(mmio, path, RR_RSV4, RFREG_MASK, cfgch);
    rw(mmio, path, RR_MOD, RFREG_MASKMODE, 0xD);
    rw(mmio, path, RR_RXK, RR_RXK_PLLEN, 0x1);

    if cur_rxbb >= 0x11 {
        rw(mmio, path, RR_TXIQK, RR_TXIQK_ATT1, 0x13);
    } else if cur_rxbb <= 0xA {
        rw(mmio, path, RR_TXIQK, RR_TXIQK_ATT1, 0x00);
    } else {
        rw(mmio, path, RR_TXIQK, RR_TXIQK_ATT1, 0x05);
    }

    rw(mmio, path, RR_XGLNA2, RR_XGLNA2_SW, 0x0);
    rw(mmio, path, RR_RXKPLL, RR_RXKPLL_POW, 0x0);
    rw(mmio, path, RR_RXKPLL, RFREG_MASK, 0x80014);
    udelay_70();

    pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 0x1);
    pwm(mmio, R_IQK_DIF4, B_IQK_DIF4_RXT, 0x025);

    one_shot(mmio, path, LBK_RXIQK);

    pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 0x0);
    rw(mmio, path, RR_RXK, RR_RXK_PLLEN, 0x0);
    pwm(mmio, R_MDPK_RX_DCK, B_MDPK_RX_DCK_EN, 0x0);
    pwm(mmio, R_KPATH_CFG, B_KPATH_CFG_ED, 0x0);
    pwm(mmio, R_LOAD_COEF + ((path as u32) << 8), B_LOAD_COEF_DI, 0x1);
    rw(mmio, path, RR_MOD, RFREG_MASKMODE, 0x5);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_get_thermal (rfk.c:1876)
// ═══════════════════════════════════════════════════════════════════
fn get_thermal(mmio: i32, path: u8, kidx: u8) {
    rw(mmio, path, RR_TM, RR_TM_TRI, 0x1);
    rw(mmio, path, RR_TM, RR_TM_TRI, 0x0);
    rw(mmio, path, RR_TM, RR_TM_TRI, 0x1);
    udelay_200();
    let t = field(rr(mmio, path, RR_TM), RR_TM_VAL) as u8;
    st().bp[path as usize][kidx as usize].ther_dpk = t;
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_rf_setting (rfk.c:1892)
// ═══════════════════════════════════════════════════════════════════
fn rf_setting(mmio: i32, path: u8, kidx: u8) {
    let band = st().bp[path as usize][kidx as usize].band;
    let bw = st().bp[path as usize][kidx as usize].bw;
    if band == 0 /* 2G */ {
        rw(mmio, path, RR_MOD, RFREG_MASK, 0x50220);
        rw(mmio, path, RR_RXBB, RR_RXBB_FATT, 0xF2);
        rw(mmio, path, RR_LUTDBG, RR_LUTDBG_TIA, 0x1);
        rw(mmio, path, RR_TIA, RR_TIA_N6, 0x1);
    } else {
        rw(mmio, path, RR_MOD, RFREG_MASK, 0x50220);
        rw(mmio, path, RR_RXA2, RR_RAA2_SWATT, 0x5);
        rw(mmio, path, RR_LUTDBG, RR_LUTDBG_TIA, 0x1);
        rw(mmio, path, RR_TIA, RR_TIA_N6, 0x1);
        rw(mmio, path, RR_RXA_LNA, RFREG_MASK, 0x920FC);
        rw(mmio, path, RR_XALNA2, RFREG_MASK, 0x002C0);
        rw(mmio, path, RR_IQGEN, RFREG_MASK, 0x38800);
    }
    rw(mmio, path, RR_RCKD, RR_RCKD_BW, 0x1);
    rw(mmio, path, RR_BTC, RR_BTC_TXBB, (bw as u32) + 1);
    rw(mmio, path, RR_BTC, RR_BTC_RXBB, 0x0);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_bypass_rxcfir (rfk.c:1923)
// ═══════════════════════════════════════════════════════════════════
fn bypass_rxcfir(mmio: i32, path: u8, enable: bool) {
    let off = (path as u32) << 8;
    if enable {
        pwm(mmio, R_RXIQC + off, B_RXIQC_BYPASS2, 0x1);
        pwm(mmio, R_RXIQC + off, B_RXIQC_BYPASS,  0x1);
    } else {
        pclr(mmio, R_RXIQC + off, B_RXIQC_BYPASS2);
        pclr(mmio, R_RXIQC + off, B_RXIQC_BYPASS);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_tpg_sel (rfk.c:1946)
// ═══════════════════════════════════════════════════════════════════
fn tpg_sel(mmio: i32, path: u8, kidx: u8) {
    let bw = st().bp[path as usize][kidx as usize].bw;
    let v = match bw { 2 => 0u32, 1 => 2u32, _ => 1u32 };
    pwm(mmio, R_TPG_MOD, B_TPG_MOD_F, v);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_table_select (rfk.c:1962)
// ═══════════════════════════════════════════════════════════════════
fn table_select(mmio: i32, path: u8, kidx: u8, gain: u8) {
    let val = 0x80u32 + (kidx as u32) * 0x20 + (gain as u32) * 0x10;
    pwm(mmio, R_DPD_CH0 + ((path as u32) << 8), 0xFF << 24, val);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_sync_check / _dpk_sync (rfk.c:1974/2016)
// ═══════════════════════════════════════════════════════════════════
fn sync_check(mmio: i32, path: u8, kidx: u8) -> bool {
    pclr(mmio, R_KIP_RPT1, B_KIP_RPT1_SEL);
    let corr_idx = field(pr(mmio, R_RPT_COM), B_PRT_COM_CORI) as u8;
    let corr_val = field(pr(mmio, R_RPT_COM), B_PRT_COM_CORV) as u8;
    st().corr_idx[path as usize][kidx as usize] = corr_idx;
    st().corr_val[path as usize][kidx as usize] = corr_val;

    pwm(mmio, R_KIP_RPT1, B_KIP_RPT1_SEL, 0x9);
    let dc_i_raw = field(pr(mmio, R_RPT_COM), B_PRT_COM_DCI);
    let dc_q_raw = field(pr(mmio, R_RPT_COM), B_PRT_COM_DCQ);
    let dc_i = abs_s(sign_ext(dc_i_raw, 12));
    let dc_q = abs_s(sign_ext(dc_q_raw, 12));
    st().dc_i[path as usize][kidx as usize] = dc_i;
    st().dc_q[path as usize][kidx as usize] = dc_q;

    dc_i > DPK_SYNC_TH_DC_I || dc_q > DPK_SYNC_TH_DC_Q || corr_val < DPK_SYNC_TH_CORR
}

fn sync(mmio: i32, path: u8, kidx: u8) -> bool {
    one_shot(mmio, path, SYNC);
    sync_check(mmio, path, kidx)
}

// ═══════════════════════════════════════════════════════════════════
//  DGain read + mapping (rfk.c:2024/2037)
// ═══════════════════════════════════════════════════════════════════
fn dgain_read(mmio: i32) -> u16 {
    pwm(mmio, R_KIP_RPT1, B_KIP_RPT1_SEL, 0x0);
    field(pr(mmio, R_RPT_COM), B_PRT_COM_DCI) as u16
}

fn dgain_mapping(dg: u16) -> i8 {
    const BND: [u16; 15] = [
        0xbf1, 0xaa5, 0x97d, 0x875, 0x789, 0x6b7, 0x5fc, 0x556,
        0x4c1, 0x43d, 0x3c7, 0x35e, 0x2ac, 0x262, 0x220,
    ];
    if dg >= BND[0] { 0x6 }
    else if dg >= BND[1] { 0x6 }
    else if dg >= BND[2] { 0x5 }
    else if dg >= BND[3] { 0x4 }
    else if dg >= BND[4] { 0x3 }
    else if dg >= BND[5] { 0x2 }
    else if dg >= BND[6] { 0x1 }
    else if dg >= BND[7] { 0x0 }
    else if dg >= BND[8] { -1 }
    else if dg >= BND[9] { -2 }
    else if dg >= BND[10] { -3 }
    else if dg >= BND[11] { -4 }
    else if dg >= BND[12] { -5 }
    else if dg >= BND[13] { -6 }
    else if dg >= BND[14] { -7 }
    else { -8 }
}

// ═══════════════════════════════════════════════════════════════════
//  Gain loss (rfk.c:2085/2093)
// ═══════════════════════════════════════════════════════════════════
fn gainloss_read(mmio: i32) -> u8 {
    pwm(mmio, R_KIP_RPT1, B_KIP_RPT1_SEL, 0x6);
    pwm(mmio, R_DPK_CFG2, B_DPK_CFG2_ST, 0x1);
    field(pr(mmio, R_RPT_COM), B_PRT_COM_GL) as u8
}

fn gainloss(mmio: i32, path: u8, kidx: u8) {
    table_select(mmio, path, kidx, 1);
    one_shot(mmio, path, GAIN_LOSS);
}

// ═══════════════════════════════════════════════════════════════════
//  KIP preset / pwr_clk / set_txagc / set_rxagc (rfk.c:2100..2144)
// ═══════════════════════════════════════════════════════════════════
fn kip_preset(mmio: i32, path: u8, kidx: u8) {
    tpg_sel(mmio, path, kidx);
    one_shot(mmio, path, KIP_PRESET);
}

fn kip_pwr_clk_on(mmio: i32, path: u8) {
    pw(mmio, R_NCTL_RPT, 0x00000080);
    pw(mmio, R_KIP_SYSCFG, 0x807F030A);
    pw(mmio, R_CFIR_SYS + ((path as u32) << 8), 0xCE000A08);
}

fn kip_set_txagc(mmio: i32, path: u8, txagc: u8) {
    rw(mmio, path, RR_TXAGC, RFREG_MASK, txagc as u32);
    pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 0x1);
    one_shot(mmio, path, DPK_TXAGC_ID);
    pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 0x0);
}

fn kip_set_rxagc(mmio: i32, path: u8) {
    let v = rr(mmio, path, RR_MOD);
    pwm(mmio, R_KIP_MOD, B_KIP_MOD, v);
    pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 0x1);
    one_shot(mmio, path, DPK_RXAGC);
    pwm(mmio, R_P0_RFCTM, B_P0_RFCTM_EN, 0x0);
    pwm(mmio, R_KIP_RPT1, B_KIP_RPT1_SEL_V1, 0x8);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_set_offset (rfk.c:2146)
// ═══════════════════════════════════════════════════════════════════
fn set_offset(mmio: i32, path: u8, gain_offset: i8) -> u8 {
    let cur = (rr(mmio, path, RR_TXAGC) & 0x3F) as i16;
    let adj = cur - (gain_offset as i16);
    let txagc = if adj < DPK_TXAGC_LOWER as i16 {
        DPK_TXAGC_LOWER
    } else if adj > DPK_TXAGC_UPPER as i16 {
        DPK_TXAGC_UPPER
    } else {
        adj as u8
    };
    kip_set_txagc(mmio, path, txagc);
    txagc
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_pas_read (rfk.c:2167)
// ═══════════════════════════════════════════════════════════════════
fn pas_read(mmio: i32, is_check: bool) -> bool {
    pwm(mmio, R_KIP_RPT1, 0xFF << 16, 0x06);
    pwm(mmio, R_DPK_CFG2, B_DPK_CFG2_ST, 0x0);
    pwm(mmio, R_DPK_CFG3, 0xFF << 16, 0x08);

    if !is_check { return false; }

    pwm(mmio, R_DPK_CFG3, 0xFF << 24, 0x00);
    let v1_i = abs_s(sign_ext(pr(mmio, R_RPT_COM) >> 16, 12)) as u32;
    let v1_q = abs_s(sign_ext(pr(mmio, R_RPT_COM) & 0xFFFF, 12)) as u32;
    pwm(mmio, R_DPK_CFG3, 0xFF << 24, 0x1F);
    let v2_i = abs_s(sign_ext(pr(mmio, R_RPT_COM) >> 16, 12)) as u32;
    let v2_q = abs_s(sign_ext(pr(mmio, R_RPT_COM) & 0xFFFF, 12)) as u32;

    let a = v1_i.saturating_mul(v1_i).saturating_add(v1_q.saturating_mul(v1_q));
    let b = v2_i.saturating_mul(v2_i).saturating_add(v2_q.saturating_mul(v2_q));
    a.saturating_mul(5) >= b.saturating_mul(8)
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_agc (rfk.c:2208) — the FSM
// ═══════════════════════════════════════════════════════════════════
fn agc(mmio: i32, path: u8, kidx: u8, init_txagc: u8, loss_only: bool) -> u8 {
    let mut step = AGC_SYNC_DGAIN;
    let mut txagc = init_txagc;
    let mut tmp_gl = 0u8;
    let mut dgain: u16 = 0;
    let mut limited_rxbb = false;
    let mut agc_cnt: u8 = 0;
    let mut goout = false;
    let mut limit = 200;

    while !goout && agc_cnt < 6 && limit > 0 {
        limit -= 1;
        match step {
            AGC_SYNC_DGAIN => {
                if sync(mmio, path, kidx) {
                    txagc = 0xFF;
                    goout = true;
                    continue;
                }
                dgain = dgain_read(mmio);
                step = if loss_only || limited_rxbb { AGC_GAIN_LOSS_IDX } else { AGC_GAIN_ADJ };
            }
            AGC_GAIN_ADJ => {
                let rxbb = field(rr(mmio, path, RR_MOD), RFREG_MASKRXBB) as i16;
                let off = dgain_mapping(dgain) as i16;
                let mut new_rxbb = rxbb + off;
                if new_rxbb > 0x1F { new_rxbb = 0x1F; limited_rxbb = true; }
                else if new_rxbb < 0 { new_rxbb = 0; limited_rxbb = true; }
                rw(mmio, path, RR_MOD, RFREG_MASKRXBB, new_rxbb as u32);
                if off != 0 || agc_cnt == 0 {
                    // bw < 80 → bypass; else lbk_rxiqk. Our chan is 20M so bypass.
                    bypass_rxcfir(mmio, path, true);
                }
                step = if dgain > 1922 || dgain < 342 { AGC_SYNC_DGAIN } else { AGC_GAIN_LOSS_IDX };
                agc_cnt += 1;
            }
            AGC_GAIN_LOSS_IDX => {
                gainloss(mmio, path, kidx);
                tmp_gl = gainloss_read(mmio);
                step = if (tmp_gl == 0 && pas_read(mmio, true)) || tmp_gl >= 7 {
                    AGC_GL_GT_CRITERION
                } else if tmp_gl == 0 {
                    AGC_GL_LT_CRITERION
                } else {
                    AGC_SET_TX_GAIN
                };
            }
            AGC_GL_GT_CRITERION => {
                if txagc == 0x2E { goout = true; }
                else { txagc = set_offset(mmio, path, 0x3); }
                step = AGC_GAIN_LOSS_IDX;
                agc_cnt += 1;
            }
            AGC_GL_LT_CRITERION => {
                if txagc == 0x3F { goout = true; }
                else { txagc = set_offset(mmio, path, -2); }
                step = AGC_GAIN_LOSS_IDX;
                agc_cnt += 1;
            }
            AGC_SET_TX_GAIN => {
                txagc = set_offset(mmio, path, tmp_gl as i8);
                goout = true;
                agc_cnt += 1;
            }
            _ => goout = true,
        }
    }
    txagc
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_set_mdpd_para + _dpk_idl_mpa (rfk.c:2327/2355)
// ═══════════════════════════════════════════════════════════════════
fn set_mdpd_para(mmio: i32, order: u32) {
    match order {
        0 => {
            pwm(mmio, R_LDL_NORM, B_LDL_NORM_OP, 0);
            pwm(mmio, R_LDL_NORM, B_LDL_NORM_PN, 0x3);
            pwm(mmio, R_MDPK_SYNC, B_MDPK_SYNC_MAN, 0x1);
        }
        1 | 2 => {
            pwm(mmio, R_LDL_NORM, B_LDL_NORM_OP, order);
            pclr(mmio, R_LDL_NORM, B_LDL_NORM_PN);
            pclr(mmio, R_MDPK_SYNC, B_MDPK_SYNC_MAN);
        }
        _ => {}
    }
}

fn idl_mpa(mmio: i32, path: u8, kidx: u8, _gain: u8) {
    let bw = st().bp[path as usize][kidx as usize].bw;
    let band = st().bp[path as usize][kidx as usize].band;
    if bw < 2 /* <80M */ && band == 1 /* 5G */ {
        set_mdpd_para(mmio, 2);
    } else {
        set_mdpd_para(mmio, 0);
    }
    one_shot(mmio, path, MDPK_IDL);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_fill_result (rfk.c:2369)
// ═══════════════════════════════════════════════════════════════════
fn fill_result(mmio: i32, path: u8, kidx: u8, gain: u8, txagc: u8) {
    let pwsf: u16 = 0x78;
    let gs = st().dpk_gs[0]; // phy 0

    pwm(mmio, R_COEF_SEL + ((path as u32) << 8), B_COEF_SEL_MDPD, kidx as u32);

    st().bp[path as usize][kidx as usize].txagc_dpk = txagc;
    pwm(mmio, R_TXAGC_RFK + ((path as u32) << 8),
        0x3F << ((gain << 3) + (kidx << 4)), txagc as u32);

    st().bp[path as usize][kidx as usize].pwsf = pwsf;
    pwm(mmio, R_DPD_BND + ((path as u32) << 8) + ((kidx as u32) << 2),
        0x1FF << (gain << 4), pwsf as u32);

    pwm(mmio, R_LOAD_COEF + ((path as u32) << 8), B_LOAD_COEF_MDPD, 0x1);
    pwm(mmio, R_LOAD_COEF + ((path as u32) << 8), B_LOAD_COEF_MDPD, 0x0);

    st().bp[path as usize][kidx as usize].gs = gs;
    let word: u32 = if gs == 0x7F { 0x007F7F7F } else { 0x005B5B5B };
    pw(mmio, R_DPD_CH0A + ((path as u32) << 8) + ((kidx as u32) << 2), word);

    pwm(mmio, R_DPD_CH0A + ((path as u32) << 8) + ((kidx as u32) << 2),
        B_DPD_ORDER_V1, order_convert(mmio));
    pw(mmio, R_DPD_V1 + ((path as u32) << 8), 0);
    pwm(mmio, R_MDPK_SYNC, B_MDPK_SYNC_SEL, 0x0);
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_reload_check (rfk.c:2408)
// ═══════════════════════════════════════════════════════════════════
fn reload_check(mmio: i32, path: u8, band: u8, ch: u8) -> bool {
    let mut reloaded = false;
    for idx in 0..BKUP_NUM {
        if band != st().bp[path as usize][idx].band { continue; }
        if ch != st().bp[path as usize][idx].ch { continue; }
        pwm(mmio, R_COEF_SEL + ((path as u32) << 8), B_COEF_SEL_MDPD, idx as u32);
        st().cur_idx[path as usize] = idx as u8;
        reloaded = true;
    }
    reloaded
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_main (rfk.c:2435) — per-path cal
// ═══════════════════════════════════════════════════════════════════
fn dpk_main(mmio: i32, path: u8, gain: u8) -> bool {
    let kidx = st().cur_idx[path as usize];
    let mut txagc: u8 = 0x38;

    // _rfk_rf_direct_cntrl(path, false) → RR_RSV1 RST = 0
    rw(mmio, path, RR_RSV1, RR_RSV1_RST, 0x0);
    // _rfk_drf_direct_cntrl(path, false) → RR_BBDC SEL = 0
    rw(mmio, path, RR_BBDC, RR_BBDC_SEL, 0x0);

    kip_pwr_clk_on(mmio, path);
    kip_set_txagc(mmio, path, txagc);
    rf_setting(mmio, path, kidx);
    rx_dck(mmio, path);

    kip_preset(mmio, path, kidx);
    kip_set_rxagc(mmio, path);
    table_select(mmio, path, kidx, gain);

    txagc = agc(mmio, path, kidx, txagc, false);
    host::print("    dpk_main path=");
    fw::print_dec(path as usize);
    host::print(" txagc=");
    fw::print_dec(txagc as usize);
    host::print("\n");

    let is_fail = txagc == 0xFF;
    if !is_fail {
        get_thermal(mmio, path, kidx);
        idl_mpa(mmio, path, kidx, gain);
        rw(mmio, path, RR_MOD, RR_MOD_MASK, RR_MOD_V_RX);
        fill_result(mmio, path, kidx, gain, txagc);
    }

    st().bp[path as usize][kidx as usize].path_ok = !is_fail;
    is_fail
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_cal_select (rfk.c:2484)
// ═══════════════════════════════════════════════════════════════════
fn cal_select(mmio: i32, band: u8, ch: u8, bw: u8) {
    // KIP backup (3 regs × 2 paths)
    let kip_reg: [u32; KIP_REG_NUM] = [0x813C, 0x8124, 0x8120];
    let mut kip_bak = [[0u32; KIP_REG_NUM]; DPK_RF_PATH];
    let mut bb_bak = [0u32; BACKUP_BB_NR];
    let mut rf_bak = [[0u32; BACKUP_RF_NR]; DPK_RF_PATH];

    // reload_check
    let mut reloaded = [false; DPK_RF_PATH];
    if st().is_dpk_reload_en {
        for path in 0u8..2 {
            reloaded[path as usize] = reload_check(mmio, path, band, ch);
            if !reloaded[path as usize] && st().bp[path as usize][0].ch != 0 {
                st().cur_idx[path as usize] = 1 - st().cur_idx[path as usize];
            } else {
                onoff(mmio, path, false);
            }
        }
    } else {
        for path in 0..DPK_RF_PATH { st().cur_idx[path] = 0; }
    }

    // Backup BB + RF regs (same as IQK — rtw8852b_backup_bb_regs/rf_regs)
    const BB_REGS: [u32; 3] = [0x2344, 0x5800, 0x7800];
    const RF_REGS: [u32; 11] = [0xDE,0xDF,0x8B,0x90,0x97,0x85,0x1E,0x00,0x02,0x05,0x10005];
    for i in 0..3 { bb_bak[i] = pr(mmio, BB_REGS[i]); }

    for path in 0u8..2 {
        // backup KIP
        for i in 0..KIP_REG_NUM {
            kip_bak[path as usize][i] = pr(mmio, kip_reg[i] + ((path as u32) << 8));
        }
        // backup RF
        for i in 0..11 { rf_bak[path as usize][i] = rr(mmio, path, RF_REGS[i]); }
        information(mmio, path, band, ch, bw);
        // TSSI pause (is_tssi_mode is true after TSSI setup)
        tssi_pause(mmio, path, true);
    }

    bb_afe_setting(mmio, bw);

    for path in 0u8..2 {
        let fail = dpk_main(mmio, path, 1);
        onoff(mmio, path, fail);
    }

    bb_afe_restore(mmio, bw);
    for i in 0..3 { host::mmio_w32(mmio, PHY_CR_BASE + BB_REGS[i], bb_bak[i]); }

    for path in 0u8..2 {
        kip_restore(mmio, path);
        for i in 0..KIP_REG_NUM {
            host::mmio_w32(mmio, PHY_CR_BASE + kip_reg[i] + ((path as u32) << 8),
                           kip_bak[path as usize][i]);
        }
        for i in 0..11 {
            rf_write_mask(mmio, path, RF_REGS[i], RFREG_MASK, rf_bak[path as usize][i]);
        }
        tssi_pause(mmio, path, false);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  _set_dpd_backoff (rfk.c:2692) → dpk_init
// ═══════════════════════════════════════════════════════════════════
pub fn init(mmio: i32) {
    host::print("  DPK: init (set_dpd_backoff)\n");
    let bf = pr(mmio, R_DPD_BF);
    let ofdm_bkof = (bf & B_DPD_BF_OFDM) >> 12;
    let tx_scale = bf & B_DPD_BF_SCA;
    if ofdm_bkof + tx_scale >= 44 {
        for path in 0u8..2 {
            let reg = R_DPD_CH0A + ((path as u32) << 8);
            pwm(mmio, reg, B_DPD_CFG, 0x007F_7F7F & B_DPD_CFG);
        }
        host::print("  DPK: backoff moved to BB (ofdm+sca >= 44)\n");
    }
}

// ═══════════════════════════════════════════════════════════════════
//  _dpk_force_bypass (rfk.c:2562)
// ═══════════════════════════════════════════════════════════════════
pub fn force_bypass(mmio: i32) {
    host::print("  DPK: force bypass (both paths disabled)\n");
    for path in 0u8..2 { onoff(mmio, path, true); }
}

// ═══════════════════════════════════════════════════════════════════
//  rtw8852b_dpk (rfk.c:3790) — public per-channel entry
// ═══════════════════════════════════════════════════════════════════
pub fn run(mmio: i32, band: u8, ch: u8, bw: u8) {
    host::print("  DPK: run band=");
    fw::print_dec(band as usize);
    host::print(" ch=");
    fw::print_dec(ch as usize);
    host::print("\n");

    // No eFEM for 8852B (non-ePA) → always run cal.
    // Linux: rtw89_chip_stop_sch_tx + _wait_rx_mode before _dpk, and
    // rtw89_chip_resume_sch_tx after. Handled by our iqk::wait_rx_mode_pub
    // and fw::stop_sch_tx / resume_sch_tx wrappers.
    let tx_en = fw::stop_sch_tx(mmio, 0);
    crate::iqk::wait_rx_mode_pub(mmio);
    cal_select(mmio, band, ch, bw);
    fw::resume_sch_tx(mmio, 0, tx_en);

    host::print("  DPK: done | A ok=");
    fw::print_dec(st().bp[0][st().cur_idx[0] as usize].path_ok as usize);
    host::print(" B ok=");
    fw::print_dec(st().bp[1][st().cur_idx[1] as usize].path_ok as usize);
    host::print("\n");
}
