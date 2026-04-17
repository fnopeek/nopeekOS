//! PHY initialization — strict 1:1 port of Linux rtw89_phy_init_bb_reg +
//! rtw89_phy_init_rf_reg + rtw89_phy_init_rf_nctl.
//!
//! Structure:
//!   1. BB table (MMIO writes via config_bb_reg)
//!   2. bb_reset
//!   3. BB gain table — NOT MMIO. Linux parses each entry's "addr" as a
//!      struct (type/path/gain_band/cfg_type) and stores values in RAM
//!      for later per-channel RSSI calibration. Writing them as MMIO
//!      clobbers SYS_ISO_CTRL / SYS_PW_CTRL (addrs 0x000..0x002 overlap
//!      PCIe power regs) and kills the chip. We skip the table for now.
//!   4. RF table per path (path A: base 0xE000; path B: base 0xF000)
//!      Each entry routed via rtw89_phy_write_rf_v1:
//!         - bit 16 of addr set (ad_sel=1) → direct MMIO at base+((addr&0xff)<<2)
//!         - else                         → SWSI at R_SWSI_DATA_V1 (0x0370)
//!   5. preinit_rf_nctl_ax (mandatory before NCTL, polls 0x8080 == 0x4)
//!   6. NCTL table (MMIO writes, addrs 0x8000+)
//!
//! Conditional state machine (PHY_COND_BRANCH_IF/ELIF/ELSE/END/CHECK) is
//! implemented per Linux rtw89_phy_init_reg. Our flat .bin tables trigger
//! the parser with headline_size=0 (no headers), so only the unconditional
//! pre-IF entries exist — functional for basic init.

use crate::host;
use crate::fw;

static BB_TABLE: &[u8]      = include_bytes!("rtw8852b_bb.bin");
static _BB_GAIN_TABLE: &[u8] = include_bytes!("rtw8852b_bb_gain.bin");
static RF_A_TABLE: &[u8]    = include_bytes!("rtw8852b_rf_a.bin");
static RF_B_TABLE: &[u8]    = include_bytes!("rtw8852b_rf_b.bin");
static NCTL_TABLE: &[u8]    = include_bytes!("rtw8852b_nctl.bin");

const PHY_COND_CHECK:       u32 = 0x4;
const PHY_COND_BRANCH_IF:   u32 = 0x8;
const PHY_COND_BRANCH_ELIF: u32 = 0x9;
const PHY_COND_BRANCH_ELSE: u32 = 0xa;
const PHY_COND_BRANCH_END:  u32 = 0xb;
const PHY_HEADLINE_VALID:   u32 = 0xf;
const PHY_COND_DONT_CARE:   u8  = 0xff;

// Linux rtw89_phy_gen_ax.cr_base — ALL PHY/BB/RF/NCTL register addresses
// in the Linux phy table constants and in rtw89_phy_write32() helpers are
// relative to this base. MAC registers (R_AX_* with cr_base=0) stay at
// their raw offset, but everything going through rtw89_phy_write32/_mask
// lands at addr + CR_BASE. Missing this offset was writing the BB table
// into SYS/PCIe/MAC registers, killing the chip.
const PHY_CR_BASE: u32 = 0x10000;

// SWSI RF indirect write (Linux reg.h) — PHY-space addresses, add CR_BASE
const R_SWSI_DATA_V1: u32 = 0x0370;
// SWSI busy status register (Linux rtw89_phy_check_swsi_busy)
const R_SWSI_V1:        u32 = 0x174C;
const B_SWSI_W_BUSY_V1: u32 = 1 << 24;
const B_SWSI_R_BUSY_V1: u32 = 1 << 25;

// 8852B RF base addresses (Linux rtw8852b.c: .rf_base_addr = {0xe000, 0xf000})
const RF_BASE_ADDR_A: u32 = 0xE000;
const RF_BASE_ADDR_B: u32 = 0xF000;
const RTW89_RF_ADDR_ADSEL_MASK: u32 = 1 << 16;
const RFREG_MASK: u32 = 0xF_FFFF;

// preinit_rf_nctl_ax register addresses (Linux reg.h)
const R_IOQ_IQK_DPK:   u32 = 0x0C60;
const R_GNT_BT_WGT_EN: u32 = 0x0C6C;
const R_P0_PATH_RST:   u32 = 0x58AC;
const R_P1_PATH_RST:   u32 = 0x78AC;
const R_NCTL_CFG:      u32 = 0x8000;
const R_NCTL_POLL:     u32 = 0x8080;

// 8852BE: rfe/cv values. cv=2 confirmed from SYS_CFG1 register dump.
// v0.63.1 with full Linux tables showed rfe=0 fails all 4 sel_headline
// cases (RF_A headlines have rfe ∈ {1..8, 0x29, 0x2B}). 1 is most common
// for 8852BE consumer cards. If still aborts, try 2.
const RFE: u8 = 1;
const CV:  u8 = 2;

fn get_phy_cond(addr: u32) -> u32    { (addr >> 28) & 0xF }
fn get_phy_target(addr: u32) -> u32  { addr & 0x0FFF_FFFF }
fn get_phy_cond_rfe(addr: u32) -> u8 { ((addr >> 16) & 0xFF) as u8 }
fn get_phy_cond_cv(addr: u32) -> u8  { (addr & 0xFF) as u8 }
fn cfg_compare(rfe: u8, cv: u8) -> u32 { ((rfe as u32) << 16) | (cv as u32) }

// ═══════════════════════════════════════════════════════════════════
//  Entry point — mirrors Linux chip->ops->bb_cfg + rfk_init sequence
// ═══════════════════════════════════════════════════════════════════

pub fn init(mmio: i32) {
    host::print("  PHY: rfe="); fw::print_dec(RFE as usize);
    host::print(" cv=");         fw::print_dec(CV as usize); host::print("\n");
    dbg(mmio, "phy-start");

    host::print("  PHY: BB regs...\n");
    let bb = run_table(mmio, BB_TABLE, WriteKind::Bb);
    // bb_reset IMMEDIATELY after last BB write, before any serial prints.
    // Linux phy_init_bb_reg runs BB table → init_txpwr_unit → bb_gain →
    // bb_reset, all in tight sequence. Our serial prints take 10-15ms
    // and the BB subsystem destabilises during that gap.
    bb_reset(mmio);
    report("BB", &bb);
    host::print("  PHY: bb_reset done\n");
    dbg(mmio, "after-BB+bb_reset");

    host::print("  PHY: BB gain... SKIPPED (needs config_bb_gain struct parser)\n");

    host::print("  PHY: RF path A...\n");
    let rfa = run_table(mmio, RF_A_TABLE, WriteKind::Rf(0));
    report("RF_A", &rfa);
    dbg(mmio, "after-RF_A");

    host::print("  PHY: RF path B...\n");
    let rfb = run_table(mmio, RF_B_TABLE, WriteKind::Rf(1));
    report("RF_B", &rfb);
    dbg(mmio, "after-RF_B");

    preinit_rf_nctl_ax(mmio);
    dbg(mmio, "after-preinit_nctl");

    host::print("  PHY: NCTL...\n");
    let nctl = run_table(mmio, NCTL_TABLE, WriteKind::Bb);
    report("NCTL", &nctl);
    dbg(mmio, "after-NCTL");

    let total = bb.written + rfa.written + rfb.written + nctl.written;
    host::print("  PHY: done ("); fw::print_dec(total as usize);
    host::print(" regs written)\n");
}

fn dbg(mmio: i32, tag: &str) {
    let cfg1 = host::mmio_r32(mmio, 0x1000);
    host::print("    [dbg "); host::print(tag);
    host::print("] CFG1=0x"); host::print_hex32(cfg1);
    host::print("\n");
}

// ═══════════════════════════════════════════════════════════════════
//  preinit_rf_nctl_ax — 1:1 port of Linux rtw89_phy_preinit_rf_nctl_ax
// ═══════════════════════════════════════════════════════════════════

fn preinit_rf_nctl_ax(mmio: i32) {
    host::print("  PHY: preinit NCTL...\n");
    // All addresses here are PHY-space (Linux uses rtw89_phy_write32_*) → +CR_BASE

    host::mmio_set32(mmio, PHY_CR_BASE + R_IOQ_IQK_DPK,   0x3);
    host::mmio_set32(mmio, PHY_CR_BASE + R_GNT_BT_WGT_EN, 0x1);
    host::mmio_set32(mmio, PHY_CR_BASE + R_P0_PATH_RST,   0x08000000);
    host::mmio_set32(mmio, PHY_CR_BASE + R_P1_PATH_RST,   0x08000000);
    host::mmio_set32(mmio, PHY_CR_BASE + R_IOQ_IQK_DPK,   0x2);
    host::mmio_w32(mmio, PHY_CR_BASE + R_NCTL_CFG, 0x8);

    for i in 0..1000u32 {
        host::mmio_w32(mmio, PHY_CR_BASE + R_NCTL_POLL, 0x4);
        for _ in 0..100 { core::hint::spin_loop(); }
        let v = host::mmio_r32(mmio, PHY_CR_BASE + R_NCTL_POLL);
        if v == 0x4 {
            host::print("    NCTL ready after "); fw::print_dec(i as usize);
            host::print(" iters\n");
            return;
        }
    }
    host::print("    WARN: NCTL poll timeout\n");
}

// ═══════════════════════════════════════════════════════════════════
//  rtw89_phy_init_reg state machine (1:1 Linux)
// ═══════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
enum WriteKind {
    Bb,
    Rf(u8),
}

struct TableStats {
    written: u32,
    skipped: u32,
    headline_size: u32,
    headline_case: u8,
    cfg_target: u32,
    aborted: bool,
}

fn report(tag: &str, s: &TableStats) {
    host::print("    "); host::print(tag);
    host::print(": hdr="); fw::print_dec(s.headline_size as usize);
    host::print(" case="); fw::print_dec(s.headline_case as usize);
    host::print(" cfg=0x"); host::print_hex32(s.cfg_target);
    host::print(" w="); fw::print_dec(s.written as usize);
    host::print(" skip="); fw::print_dec(s.skipped as usize);
    if s.aborted { host::print(" ABORT"); }
    host::print("\n");
}

fn run_table(mmio: i32, table: &[u8], kind: WriteKind) -> TableStats {
    let mut stats = TableStats {
        written: 0, skipped: 0, headline_size: 0, headline_case: 0,
        cfg_target: 0, aborted: false,
    };
    if table.len() < 4 { return stats; }
    let n_regs = u32::from_le_bytes([table[0], table[1], table[2], table[3]]) as usize;

    let (headline_size, headline_idx, case) = match sel_headline(table, n_regs, RFE, CV) {
        Some(r) => r,
        None => return stats, // Linux behavior: "invalid PHY package" → skip table
    };
    stats.headline_size = headline_size as u32;
    stats.headline_case = case;

    // cfg_target from the chosen headline entry (or entry 0 if no headlines).
    let hdr_off = 4 + headline_idx * 8;
    let hdr_addr = if hdr_off + 4 <= table.len() {
        u32::from_le_bytes([table[hdr_off], table[hdr_off+1], table[hdr_off+2], table[hdr_off+3]])
    } else { 0 };
    stats.cfg_target = get_phy_target(hdr_addr);

    let mut is_matched = true;
    let mut target_found = false;
    let mut branch_target: u32 = 0;

    let mut last_addr: u32 = 0;
    let mut last_data: u32 = 0;
    let mut last_written_at: u32 = 0;

    for i in headline_size..n_regs {
        let off = 4 + i * 8;
        if off + 8 > table.len() { break; }
        let addr = u32::from_le_bytes([table[off], table[off+1], table[off+2], table[off+3]]);
        let data = u32::from_le_bytes([table[off+4], table[off+5], table[off+6], table[off+7]]);

        match get_phy_cond(addr) {
            PHY_COND_BRANCH_IF | PHY_COND_BRANCH_ELIF => {
                branch_target = get_phy_target(addr);
            }
            PHY_COND_BRANCH_ELSE => {
                is_matched = false;
                if !target_found {
                    stats.aborted = true;
                    return stats;
                }
            }
            PHY_COND_BRANCH_END => {
                is_matched = true;
                target_found = false;
            }
            PHY_COND_CHECK => {
                if target_found {
                    is_matched = false;
                } else if branch_target == stats.cfg_target {
                    is_matched = true;
                    target_found = true;
                } else {
                    is_matched = false;
                    target_found = false;
                }
            }
            _ => {
                if is_matched {
                    let target = get_phy_target(addr);
                    write_entry(mmio, kind, target, data);
                    stats.written += 1;
                    last_addr = addr;
                    last_data = data;
                    last_written_at = stats.written;
                    let _ = last_addr; let _ = last_data; let _ = last_written_at; // used below
                    // Periodic liveness check — tight window for killer.
                    let check_freq = 16u32;
                    if stats.written % check_freq == 0 {
                        let cfg1 = host::mmio_r32(mmio, 0x1000);
                        if cfg1 == 0xFFFF_FFFF {
                            host::print("    [kill] after w=");
                            fw::print_dec(stats.written as usize);
                            host::print(" addr=0x"); host::print_hex32(addr);
                            host::print(" data=0x"); host::print_hex32(data);
                            host::print(" target=0x"); host::print_hex32(target);
                            host::print("\n");
                            stats.aborted = true;
                            return stats;
                        }
                    }
                } else {
                    stats.skipped += 1;
                }
            }
        }
    }

    // Final kill check — catches killers in the last <16 writes that never
    // hit a periodic checkpoint. Reports the last written entry as the
    // prime suspect (it's within 0..check_freq writes of the actual death).
    if stats.written > 0 {
        let cfg1 = host::mmio_r32(mmio, 0x1000);
        if cfg1 == 0xFFFF_FFFF {
            host::print("    [final-kill] after w=");
            fw::print_dec(stats.written as usize);
            host::print(" last_addr=0x"); host::print_hex32(last_addr);
            host::print(" last_data=0x"); host::print_hex32(last_data);
            host::print("\n");
            stats.aborted = true;
        }
    }

    stats
}

fn sel_headline(table: &[u8], n_regs: usize, rfe: u8, cv: u8)
    -> Option<(usize, usize, u8)>
{
    let mut headline_size = 0usize;
    for i in 0..n_regs {
        let off = 4 + i * 8;
        if off + 8 > table.len() { break; }
        let addr = u32::from_le_bytes([table[off], table[off+1], table[off+2], table[off+3]]);
        if get_phy_cond(addr) != PHY_HEADLINE_VALID { break; }
        headline_size += 1;
    }
    if headline_size == 0 {
        return Some((0, 0, 0));
    }

    let hdr = |i: usize| -> u32 {
        let off = 4 + i * 8;
        u32::from_le_bytes([table[off], table[off+1], table[off+2], table[off+3]])
    };

    // case 1: RFE match, CV match
    let compare = cfg_compare(rfe, cv);
    for i in 0..headline_size {
        if get_phy_target(hdr(i)) == compare { return Some((headline_size, i, 1)); }
    }

    // case 2: RFE match, CV don't care
    let compare = cfg_compare(rfe, PHY_COND_DONT_CARE);
    for i in 0..headline_size {
        if get_phy_target(hdr(i)) == compare { return Some((headline_size, i, 2)); }
    }

    // case 3: RFE match, CV max
    let mut cv_max: u8 = 0;
    let mut matched: Option<usize> = None;
    for i in 0..headline_size {
        let a = hdr(i);
        if get_phy_cond_rfe(a) == rfe {
            let c = get_phy_cond_cv(a);
            if c >= cv_max { cv_max = c; matched = Some(i); }
        }
    }
    if let Some(i) = matched { return Some((headline_size, i, 3)); }

    // case 4: RFE don't care, CV max
    let mut cv_max: u8 = 0;
    let mut matched: Option<usize> = None;
    for i in 0..headline_size {
        let a = hdr(i);
        if get_phy_cond_rfe(a) == PHY_COND_DONT_CARE {
            let c = get_phy_cond_cv(a);
            if c >= cv_max { cv_max = c; matched = Some(i); }
        }
    }
    if let Some(i) = matched { return Some((headline_size, i, 4)); }

    None
}

// ═══════════════════════════════════════════════════════════════════
//  write_entry — dispatches delay vs MMIO/SWSI
// ═══════════════════════════════════════════════════════════════════

fn write_entry(mmio: i32, kind: WriteKind, addr: u32, data: u32) {
    // Delay encodings — same for BB and RF paths (rtw89_phy_config_bb_reg
    // and rtw89_phy_config_rf_reg check these explicitly).
    match addr {
        0xFE => { host::sleep_ms(50); return; }
        0xFD => { host::sleep_ms(5);  return; }
        0xFC => { host::sleep_ms(1);  return; }
        0xFB | 0xFA | 0xF9 => { return; }
        _ => {}
    }

    match kind {
        WriteKind::Bb => {
            // Linux rtw89_phy_config_bb_reg → rtw89_phy_write32(addr+cr_base, data)
            host::mmio_w32(mmio, PHY_CR_BASE + addr, data);
        }
        WriteKind::Rf(path) => {
            // Linux rtw89_phy_write_rf_v1: ad_sel = addr & BIT(16)
            //   ad_sel=1: rtw89_phy_write_rf → rtw89_phy_write32_mask(direct, mask, data)
            //             direct = base_addr[path] + ((addr&0xff)<<2), MASKED by RFREG_MASK.
            //             All via rtw89_phy_write32_mask → +cr_base.
            //   ad_sel=0: rtw89_phy_write_rf_a → SWSI (also via rtw89_phy_write32_mask → +cr_base)
            if addr & RTW89_RF_ADDR_ADSEL_MASK != 0 {
                let base = if path == 0 { RF_BASE_ADDR_A } else { RF_BASE_ADDR_B };
                let direct = PHY_CR_BASE + base + ((addr & 0xFF) << 2);
                let old = host::mmio_r32(mmio, direct);
                let new = (old & !RFREG_MASK) | (data & RFREG_MASK);
                host::mmio_w32(mmio, direct, new);
            } else {
                write_rf_swsi(mmio, path, addr, data);
            }
        }
    }
}

/// SWSI RF write (Linux rtw89_phy_write_rf_a, mask == RFREG_MASK path).
/// R_SWSI_DATA_V1 layout: [31]=bit_mask_en | [30:28]=path | [27:20]=addr | [19:0]=data
/// All SWSI registers are PHY-space → need +PHY_CR_BASE offset.
fn write_rf_swsi(mmio: i32, path: u8, addr: u32, data: u32) {
    for _ in 0..30u32 {
        let v = host::mmio_r32(mmio, PHY_CR_BASE + R_SWSI_V1);
        if v & (B_SWSI_W_BUSY_V1 | B_SWSI_R_BUSY_V1) == 0 { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }
    let val = (data & RFREG_MASK)
            | ((addr & 0xFF) << 20)
            | (((path as u32) & 0x7) << 28);
    host::mmio_w32(mmio, PHY_CR_BASE + R_SWSI_DATA_V1, val);
}

/// 1:1 Linux rtw8852b_bb_reset (rtw8852b.c:566) + rtw8852bx_bb_reset_all
/// (rtw8852b_common.c:1077). Called at the end of phy_init_bb_reg to
/// commit the BB state. Without this the BB subsystem drifts and kills
/// the PCIe interface some milliseconds after the last BB write.
fn bb_reset(mmio: i32) {
    // Addresses from Linux reg.h
    const R_P0_TXPW_RSTB: u32 = 0x58DC;
    const R_P0_TSSI_TRK:  u32 = 0x5818;
    const R_P1_TXPW_RSTB: u32 = 0x78DC;
    const R_P1_TSSI_TRK:  u32 = 0x7818;
    const B_TXPW_RSTB_MANON: u32 = 1 << 30;
    const B_TSSI_TRK_EN:     u32 = 1 << 30;

    const R_S0_HW_SI_DIS: u32 = 0x1200;
    const R_S1_HW_SI_DIS: u32 = 0x3200;
    const B_HW_SI_DIS_W_R_TRIG_MASK: u32 = 0x7 << 28; // GENMASK(30,28)

    const R_RSTB_ASYNC:   u32 = 0x0704;
    const B_RSTB_ASYNC_ALL: u32 = 1 << 1;

    // All addresses are PHY-space (Linux uses rtw89_phy_write32_*) → +CR_BASE

    // Pre-gate
    host::mmio_set32(mmio, PHY_CR_BASE + R_P0_TXPW_RSTB, B_TXPW_RSTB_MANON);
    host::mmio_set32(mmio, PHY_CR_BASE + R_P0_TSSI_TRK,  B_TSSI_TRK_EN);
    host::mmio_set32(mmio, PHY_CR_BASE + R_P1_TXPW_RSTB, B_TXPW_RSTB_MANON);
    host::mmio_set32(mmio, PHY_CR_BASE + R_P1_TSSI_TRK,  B_TSSI_TRK_EN);

    // rtw8852bx_bb_reset_all body
    let s0 = host::mmio_r32(mmio, PHY_CR_BASE + R_S0_HW_SI_DIS);
    host::mmio_w32(mmio, PHY_CR_BASE + R_S0_HW_SI_DIS, (s0 & !B_HW_SI_DIS_W_R_TRIG_MASK) | (0x7 << 28));
    let s1 = host::mmio_r32(mmio, PHY_CR_BASE + R_S1_HW_SI_DIS);
    host::mmio_w32(mmio, PHY_CR_BASE + R_S1_HW_SI_DIS, (s1 & !B_HW_SI_DIS_W_R_TRIG_MASK) | (0x7 << 28));

    host::sleep_ms(1); // Linux fsleep(1)

    host::mmio_set32(mmio, PHY_CR_BASE + R_RSTB_ASYNC, B_RSTB_ASYNC_ALL);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_RSTB_ASYNC, B_RSTB_ASYNC_ALL);

    let s0 = host::mmio_r32(mmio, PHY_CR_BASE + R_S0_HW_SI_DIS);
    host::mmio_w32(mmio, PHY_CR_BASE + R_S0_HW_SI_DIS, s0 & !B_HW_SI_DIS_W_R_TRIG_MASK);
    let s1 = host::mmio_r32(mmio, PHY_CR_BASE + R_S1_HW_SI_DIS);
    host::mmio_w32(mmio, PHY_CR_BASE + R_S1_HW_SI_DIS, s1 & !B_HW_SI_DIS_W_R_TRIG_MASK);

    host::mmio_set32(mmio, PHY_CR_BASE + R_RSTB_ASYNC, B_RSTB_ASYNC_ALL);

    // Post-gate
    host::mmio_clr32(mmio, PHY_CR_BASE + R_P0_TXPW_RSTB, B_TXPW_RSTB_MANON);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_P0_TSSI_TRK,  B_TSSI_TRK_EN);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_P1_TXPW_RSTB, B_TXPW_RSTB_MANON);
    host::mmio_clr32(mmio, PHY_CR_BASE + R_P1_TSSI_TRK,  B_TSSI_TRK_EN);
}
