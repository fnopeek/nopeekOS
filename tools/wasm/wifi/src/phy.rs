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
static BB_GAIN_TABLE: &[u8] = include_bytes!("rtw8852b_bb_gain.bin");
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
pub const PHY_CR_BASE: u32 = 0x10000;

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

    host::print("  PHY: BB gain...\n");
    let gain = run_table(mmio, BB_GAIN_TABLE, WriteKind::BbGain);
    report("GAIN", &gain);
    dbg(mmio, "after-BB-gain");

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

    // edcca_init — Linux rtw89_phy_edcca_init for 8852B is 1 register write.
    // Without proper EDCCA threshold, the MAC's CCA engine thinks the channel
    // is always busy → RX is blocked for real frames.
    //   R_TX_COLLISION_T2R_ST = 0x0C70
    //   B_TX_COLLISION_T2R_ST_M = GENMASK(25, 20)
    //   value = 0x29
    const R_TX_COLLISION_T2R_ST: u32 = 0x0C70;
    const TX_COLL_MASK: u32 = 0x3F << 20;
    let v = host::mmio_r32(mmio, PHY_CR_BASE + R_TX_COLLISION_T2R_ST);
    host::mmio_w32(mmio, PHY_CR_BASE + R_TX_COLLISION_T2R_ST,
                   (v & !TX_COLL_MASK) | (0x29 << 20));
    host::print("  PHY: edcca_init done\n");
    dbg(mmio, "after-edcca_init");

    // RFK baseline — Linux rtw8852b_rfk_init (rtw8852b.c:649)
    // Part 1: dpk_init + rck (inline in phy.rs)
    rfk_init(mmio);
    dbg(mmio, "after-rck/dpk");
    // Part 2: dack + rx_dck (rfk.rs module)
    crate::rfk::init(mmio);
    dbg(mmio, "after-dack/rx_dck");

    let total = bb.written + rfa.written + rfb.written + nctl.written;
    host::print("  PHY: done ("); fw::print_dec(total as usize);
    host::print(" regs written)\n");
}

/// Gated by `VERBOSE` — dumps CFG1 at every PHY-init milestone. Was
/// critical while tracking down "BB table kills PCIe" regressions in
/// v0.80-era; now that init passes cleanly these 15+ lines are pure
/// noise. Keep code to re-enable when a future chip change breaks CFG1.
const VERBOSE: bool = false;

fn dbg(mmio: i32, tag: &str) {
    if !VERBOSE { return; }
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
    BbGain,
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
        WriteKind::BbGain => {
            // No MMIO. addr is a packed arg (type/path/gband/cfg_type);
            // dispatch to cfg_bb_gain which stores into BB_GAIN software state.
            cfg_bb_gain(addr, data);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
//  RF register read/write helpers — used by RFK functions
//  Linux rtw89_phy_{read,write}_rf_v1 — route via ad_sel bit.
// ═══════════════════════════════════════════════════════════════════

const R_SWSI_READ_ADDR_V1: u32 = 0x0378;
// B_SWSI_READ_ADDR_ADDR_V1 = GENMASK(7,0), B_SWSI_READ_ADDR_PATH_V1 = GENMASK(10,8)
const B_SWSI_R_DATA_DONE_V1: u32 = 1 << 26;
const R_SWSI_BIT_MASK_V1: u32 = 0x0374;

/// Read an RF register. Returns the full 20-bit value (unmasked).
/// Linux rtw89_phy_read_rf_v1 — dispatches ad_sel=1 to direct MMIO,
/// ad_sel=0 to SWSI read via R_SWSI_READ_ADDR_V1.
pub fn rf_read(mmio: i32, path: u8, addr: u32) -> u32 {
    if addr & RTW89_RF_ADDR_ADSEL_MASK != 0 {
        // Direct MMIO read (Linux rtw89_phy_read_rf): base + (addr & 0xff) << 2
        let base = if path == 0 { RF_BASE_ADDR_A } else { RF_BASE_ADDR_B };
        let direct = PHY_CR_BASE + base + ((addr & 0xFF) << 2);
        return host::mmio_r32(mmio, direct) & RFREG_MASK;
    }
    // SWSI read (Linux rtw89_phy_read_rf_a)
    // Wait busy clear
    for _ in 0..30u32 {
        let v = host::mmio_r32(mmio, PHY_CR_BASE + R_SWSI_V1);
        if v & (B_SWSI_W_BUSY_V1 | B_SWSI_R_BUSY_V1) == 0 { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }
    // Trigger read: path[10:8] | addr[7:0]
    let req = ((path as u32 & 0x7) << 8) | (addr & 0xFF);
    host::mmio_w32(mmio, PHY_CR_BASE + R_SWSI_READ_ADDR_V1, req);
    for _ in 0..200 { core::hint::spin_loop(); } // ~2us
    // Poll B_SWSI_R_DATA_DONE
    for _ in 0..30u32 {
        let v = host::mmio_r32(mmio, PHY_CR_BASE + R_SWSI_V1);
        if v & B_SWSI_R_DATA_DONE_V1 != 0 {
            return v & RFREG_MASK;
        }
        for _ in 0..100 { core::hint::spin_loop(); }
    }
    0xFFFFFFFF // timeout marker
}

/// Write an RF register with a bit mask. Read-modify-write internally.
pub fn rf_write_mask(mmio: i32, path: u8, addr: u32, mask: u32, data: u32) {
    let mask = mask & RFREG_MASK;
    let shift = if mask == 0 { 0 } else { mask.trailing_zeros() };
    if mask == RFREG_MASK {
        // Full register write
        rf_write_full(mmio, path, addr, data & RFREG_MASK);
    } else {
        let cur = rf_read(mmio, path, addr);
        let new_val = (cur & !mask) | ((data << shift) & mask);
        rf_write_full(mmio, path, addr, new_val);
    }
}

/// Write full RF register (unmasked) — RFREG_MASK = 0xFFFFF.
pub fn rf_write_full(mmio: i32, path: u8, addr: u32, data: u32) {
    if addr & RTW89_RF_ADDR_ADSEL_MASK != 0 {
        let base = if path == 0 { RF_BASE_ADDR_A } else { RF_BASE_ADDR_B };
        let direct = PHY_CR_BASE + base + ((addr & 0xFF) << 2);
        let old = host::mmio_r32(mmio, direct);
        host::mmio_w32(mmio, direct, (old & !RFREG_MASK) | (data & RFREG_MASK));
    } else {
        write_rf_swsi(mmio, path, addr, data);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  RFK: baseline — 1:1 Linux rtw8852b_rfk_init
//  rtw8852b.c:649 → dpk_init + rck + dack + rx_dck
//  We implement dpk_init + rck for now (minimal baseline);
//  dack/rx_dck are much larger and can be added if still needed.
// ═══════════════════════════════════════════════════════════════════

// RF register addresses (Linux reg.h)
const RR_MOD: u32      = 0x00;
const RR_MOD_MASK: u32 = 0xF0000;     // GENMASK(19,16)
const RR_MOD_V_RX: u32 = 0x3;
const RR_RSV1: u32     = 0x05;
const RR_RSV1_RST: u32 = 0x1;         // BIT(0)
const RR_RCKC: u32     = 0x1B;
const RR_RCKC_CA: u32  = 0x7C00;      // GENMASK(14,10)
const RR_RCKS: u32     = 0x1C;

// DPK backoff registers (PHY space, CR_BASE applies)
const R_DPD_BF: u32      = 0x4CF8;
const B_DPD_BF_OFDM: u32 = 0x1F00;    // GENMASK(12,8)
const B_DPD_BF_SCA: u32  = 0x3E;      // GENMASK(5,1) — from Linux bb def
const R_DPD_CH0A: u32    = 0x5800;    // Per-path base (path 0 = 0x5800, path 1 = 0x5900)
const B_DPD_CFG: u32     = 0x00FF_FFFF; // bottom 24 bits

fn rfk_init(mmio: i32) {
    host::print("  RFK: dpk_init\n");
    set_dpd_backoff(mmio);
    host::print("  RFK: rck path A\n");
    rck(mmio, 0);
    host::print("  RFK: rck path B\n");
    rck(mmio, 1);
    host::print("  RFK: baseline done\n");
}

/// 1:1 Linux _set_dpd_backoff for phy=0 (rtw8852b_rfk.c:2692).
fn set_dpd_backoff(mmio: i32) {
    // phy_read32_mask(R_DPD_BF + (phy << 13), B_DPD_BF_OFDM/SCA)
    // For phy=0, offset=0.
    let v = host::mmio_r32(mmio, PHY_CR_BASE + R_DPD_BF);
    let ofdm_bkof = (v & B_DPD_BF_OFDM) >> B_DPD_BF_OFDM.trailing_zeros();
    let tx_scale  = (v & B_DPD_BF_SCA)  >> B_DPD_BF_SCA.trailing_zeros();
    if ofdm_bkof + tx_scale >= 44 {
        // Linux: for each path, write B_DPD_CFG=0x7f7f7f at R_DPD_CH0A + (path << 8)
        for path in 0..2u32 {
            let reg = PHY_CR_BASE + R_DPD_CH0A + (path << 8);
            let cur = host::mmio_r32(mmio, reg);
            host::mmio_w32(mmio, reg, (cur & !B_DPD_CFG) | 0x7F7F7F);
        }
    }
}

/// 1:1 Linux _rck (rtw8852b_rfk.c:336).
fn rck(mmio: i32, path: u8) {
    let rf_reg5 = rf_read(mmio, path, RR_RSV1);
    rf_write_mask(mmio, path, RR_RSV1, RR_RSV1_RST, 0x0);
    rf_write_mask(mmio, path, RR_MOD, RR_MOD_MASK, RR_MOD_V_RX);

    // RCK trigger
    rf_write_full(mmio, path, RR_RCKC, 0x00240);

    // Poll RR_RCKS bit 3 (~30us timeout, Linux: 2us sleep × 30)
    for _ in 0..30u32 {
        let v = rf_read(mmio, path, RR_RCKS);
        if v & (1 << 3) != 0 { break; }
        for _ in 0..200 { core::hint::spin_loop(); }
    }

    let rck_val = (rf_read(mmio, path, RR_RCKC) & RR_RCKC_CA) >> RR_RCKC_CA.trailing_zeros();

    rf_write_full(mmio, path, RR_RCKC, rck_val);
    rf_write_full(mmio, path, RR_RSV1, rf_reg5);
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

// ═══════════════════════════════════════════════════════════════════
//  BB gain parser + HW apply  — 1:1 Linux rtw89_phy_config_bb_gain_ax
//  + rtw8852bx_set_gain_error (rtw8852b_common.c:577).
//
//  BB gain entries in the bb_gain table do NOT write to MMIO. Instead
//  the table addr field is a packed `rtw89_phy_bb_gain_arg` union:
//      [ 7: 0] type   (or rxsc_start[3:0] | bw[7:4] for rpl_ofst)
//      [15: 8] path
//      [23:16] gain_band  (0=2G, 1..3=5G, 4..7=6G)
//      [31:24] cfg_type   (0=error, 1=rpl_ofst, 2=bypass, 3=op1db, 4=eFEM)
//  The data field carries 4 s8 values packed LE — unpacked by cfg_*.
//
//  Linux stores results in rtwdev->bb_gain.ax and uses them in
//  rtw8852bx_ctrl_ch → set_gain_error to write LNA/TIA gain registers
//  per channel band. Skipping this leaves LNA/TIA gain = 0 on every
//  channel switch → RX front-end has no gain → zero packets received.
// ═══════════════════════════════════════════════════════════════════

const GAIN_BAND_NR: usize = 8;   // 2G + 5G L/M/H + 6G L/M/H/UH
const PATH_NR: usize      = 2;   // 8852B uses RF_PATH_A + RF_PATH_B
const LNA_NUM: usize      = 7;
const TIA_NUM: usize      = 2;
const TIA_LNA_OP1DB_NUM: usize = 8;  // LNA_GAIN_NUM + 1 per Linux core.h
const RXSC_NUM_40: usize  = 9;
const RXSC_NUM_80: usize  = 13;
const RXSC_NUM_160: usize = 15;

struct BbGainInfo {
    lna_gain:         [[[i8; LNA_NUM]; PATH_NR]; GAIN_BAND_NR],
    tia_gain:         [[[i8; TIA_NUM]; PATH_NR]; GAIN_BAND_NR],
    lna_gain_bypass:  [[[i8; LNA_NUM]; PATH_NR]; GAIN_BAND_NR],
    lna_op1db:        [[[i8; LNA_NUM]; PATH_NR]; GAIN_BAND_NR],
    tia_lna_op1db:    [[[i8; TIA_LNA_OP1DB_NUM]; PATH_NR]; GAIN_BAND_NR],
    rpl_ofst_20:      [[i8; PATH_NR]; GAIN_BAND_NR],
    rpl_ofst_40:      [[[i8; RXSC_NUM_40];  PATH_NR]; GAIN_BAND_NR],
    rpl_ofst_80:      [[[i8; RXSC_NUM_80];  PATH_NR]; GAIN_BAND_NR],
    rpl_ofst_160:     [[[i8; RXSC_NUM_160]; PATH_NR]; GAIN_BAND_NR],
}

static mut BB_GAIN: BbGainInfo = BbGainInfo {
    lna_gain:        [[[0i8; LNA_NUM]; PATH_NR]; GAIN_BAND_NR],
    tia_gain:        [[[0i8; TIA_NUM]; PATH_NR]; GAIN_BAND_NR],
    lna_gain_bypass: [[[0i8; LNA_NUM]; PATH_NR]; GAIN_BAND_NR],
    lna_op1db:       [[[0i8; LNA_NUM]; PATH_NR]; GAIN_BAND_NR],
    tia_lna_op1db:   [[[0i8; TIA_LNA_OP1DB_NUM]; PATH_NR]; GAIN_BAND_NR],
    rpl_ofst_20:     [[0i8; PATH_NR]; GAIN_BAND_NR],
    rpl_ofst_40:     [[[0i8; RXSC_NUM_40];  PATH_NR]; GAIN_BAND_NR],
    rpl_ofst_80:     [[[0i8; RXSC_NUM_80];  PATH_NR]; GAIN_BAND_NR],
    rpl_ofst_160:    [[[0i8; RXSC_NUM_160]; PATH_NR]; GAIN_BAND_NR],
};

// Linux enum rtw89_phy_bb_rxsc_start_idx
const RXSC_IDX_FULL:    u8 = 0;
const RXSC_IDX_20:      u8 = 1;
const RXSC_IDX_20_1:    u8 = 5;
const RXSC_IDX_40:      u8 = 9;
const RXSC_IDX_80:      u8 = 13;

// Linux enum rtw89_channel_width
const BW_20:  u8 = 0;
const BW_40:  u8 = 1;
const BW_80:  u8 = 2;
const BW_160: u8 = 3;

fn cfg_bb_gain(addr: u32, data: u32) {
    // Reject flow-ctrl/delay addrs — Linux: "bb gain table with flow ctrl".
    let addr_lo = addr & 0xFF;
    if addr_lo >= 0xF9 && addr_lo <= 0xFE { return; }

    let typ       = (addr & 0xFF) as u8;                 // also rxsc_start[3:0]|bw[7:4]
    let path      = ((addr >>  8) & 0xFF) as u8;
    let gband     = ((addr >> 16) & 0xFF) as u8;
    let cfg_type  = ((addr >> 24) & 0xFF) as u8;

    if (gband as usize) >= GAIN_BAND_NR { return; }
    if (path  as usize) >= PATH_NR      { return; }

    match cfg_type {
        0 => cfg_gain_error(typ, path, gband, data),
        1 => cfg_rpl_ofst(typ, path, gband, data),
        2 => cfg_gain_bypass(typ, path, gband, data),
        3 => cfg_gain_op1db(typ, path, gband, data),
        _ => { /* 4 = eFEM (needs efuse rfe_type>=50), ignore */ }
    }
}

fn cfg_gain_error(typ: u8, path: u8, gband: u8, mut data: u32) {
    let p = path as usize; let b = gband as usize;
    unsafe {
        match typ {
            0 => for i in 0..4 { BB_GAIN.lna_gain[b][p][i] = (data & 0xFF) as i8; data >>= 8; },
            1 => for i in 4..7 { BB_GAIN.lna_gain[b][p][i] = (data & 0xFF) as i8; data >>= 8; },
            2 => for i in 0..2 { BB_GAIN.tia_gain[b][p][i] = (data & 0xFF) as i8; data >>= 8; },
            _ => {}
        }
    }
}

fn cfg_gain_bypass(typ: u8, path: u8, gband: u8, mut data: u32) {
    let p = path as usize; let b = gband as usize;
    unsafe {
        match typ {
            0 => for i in 0..4 { BB_GAIN.lna_gain_bypass[b][p][i] = (data & 0xFF) as i8; data >>= 8; },
            1 => for i in 4..7 { BB_GAIN.lna_gain_bypass[b][p][i] = (data & 0xFF) as i8; data >>= 8; },
            _ => {}
        }
    }
}

fn cfg_gain_op1db(typ: u8, path: u8, gband: u8, mut data: u32) {
    let p = path as usize; let b = gband as usize;
    unsafe {
        match typ {
            0 => for i in 0..4 { BB_GAIN.lna_op1db[b][p][i] = (data & 0xFF) as i8; data >>= 8; },
            1 => for i in 4..7 { BB_GAIN.lna_op1db[b][p][i] = (data & 0xFF) as i8; data >>= 8; },
            2 => for i in 0..4 { BB_GAIN.tia_lna_op1db[b][p][i] = (data & 0xFF) as i8; data >>= 8; },
            3 => for i in 4..8 { BB_GAIN.tia_lna_op1db[b][p][i] = (data & 0xFF) as i8; data >>= 8; },
            _ => {}
        }
    }
}

fn cfg_rpl_ofst(typ: u8, path: u8, gband: u8, mut data: u32) {
    // typ = rxsc_start[3:0] | bw[7:4]
    let rxsc_start = typ & 0xF;
    let bw         = typ >> 4;
    let p = path as usize; let b = gband as usize;
    unsafe {
        match bw {
            BW_20 => BB_GAIN.rpl_ofst_20[b][p] = data as i8,
            BW_40 => {
                if rxsc_start == RXSC_IDX_FULL {
                    BB_GAIN.rpl_ofst_40[b][p][0] = data as i8;
                } else if rxsc_start == RXSC_IDX_20 {
                    for i in 0..2 {
                        let rxsc = RXSC_IDX_20 + i;
                        BB_GAIN.rpl_ofst_40[b][p][rxsc as usize] = (data & 0xFF) as i8;
                        data >>= 8;
                    }
                }
            }
            BW_80 => {
                if rxsc_start == RXSC_IDX_FULL {
                    BB_GAIN.rpl_ofst_80[b][p][0] = data as i8;
                } else if rxsc_start == RXSC_IDX_20 {
                    for i in 0..4 {
                        let rxsc = RXSC_IDX_20 + i;
                        BB_GAIN.rpl_ofst_80[b][p][rxsc as usize] = (data & 0xFF) as i8;
                        data >>= 8;
                    }
                } else if rxsc_start == RXSC_IDX_40 {
                    for i in 0..2 {
                        let rxsc = RXSC_IDX_40 + i;
                        BB_GAIN.rpl_ofst_80[b][p][rxsc as usize] = (data & 0xFF) as i8;
                        data >>= 8;
                    }
                }
            }
            BW_160 => {
                if rxsc_start == RXSC_IDX_FULL {
                    BB_GAIN.rpl_ofst_160[b][p][0] = data as i8;
                } else if rxsc_start == RXSC_IDX_20 {
                    for i in 0..4 {
                        let rxsc = RXSC_IDX_20 + i;
                        BB_GAIN.rpl_ofst_160[b][p][rxsc as usize] = (data & 0xFF) as i8;
                        data >>= 8;
                    }
                } else if rxsc_start == RXSC_IDX_20_1 {
                    for i in 0..4 {
                        let rxsc = RXSC_IDX_20_1 + i;
                        BB_GAIN.rpl_ofst_160[b][p][rxsc as usize] = (data & 0xFF) as i8;
                        data >>= 8;
                    }
                } else if rxsc_start == RXSC_IDX_40 {
                    for i in 0..4 {
                        let rxsc = RXSC_IDX_40 + i;
                        BB_GAIN.rpl_ofst_160[b][p][rxsc as usize] = (data & 0xFF) as i8;
                        data >>= 8;
                    }
                } else if rxsc_start == RXSC_IDX_80 {
                    for i in 0..2 {
                        let rxsc = RXSC_IDX_80 + i;
                        BB_GAIN.rpl_ofst_160[b][p][rxsc as usize] = (data & 0xFF) as i8;
                        data >>= 8;
                    }
                }
            }
            _ => {}
        }
    }
}

// 8852B bb_gain_lna/tia register tables — 1:1 Linux
// rtw8852b_common.c:553 (bb_gain_lna, 2G only: .gain_g).
// PHY-space, + PHY_CR_BASE when writing.
const BB_GAIN_LNA_G: [(u32, u32, u32); LNA_NUM] = [
    (0x4678, 0x475C, 0x00FF_0000),
    (0x4678, 0x475C, 0xFF00_0000),
    (0x467C, 0x4760, 0x0000_00FF),
    (0x467C, 0x4760, 0x0000_FF00),
    (0x467C, 0x4760, 0x00FF_0000),
    (0x467C, 0x4760, 0xFF00_0000),
    (0x4680, 0x4764, 0x0000_00FF),
];

const BB_GAIN_TIA_G: [(u32, u32, u32); TIA_NUM] = [
    (0x4680, 0x4764, 0x00FF_0000),
    (0x4680, 0x4764, 0xFF00_0000),
];

/// 1:1 Linux rtw8852bx_set_gain_error for subband == RTW89_CH_2G.
/// Writes LNA + TIA gain values stored in BB_GAIN (populated by
/// cfg_bb_gain during BB gain table load) into per-path HW registers.
/// Must be called on each 2G channel change (Linux: inside rtw8852bx_ctrl_ch).
pub fn apply_gain_error_2g(mmio: i32, path: u8) {
    let p = path as usize;
    let gband = 0usize; // RTW89_BB_GAIN_BAND_2G
    unsafe {
        for i in 0..LNA_NUM {
            let (reg_a, reg_b, mask) = BB_GAIN_LNA_G[i];
            let reg = if path == 0 { reg_a } else { reg_b };
            let val = BB_GAIN.lna_gain[gband][p][i] as i32 as u32;
            host::mmio_w32_mask(mmio, PHY_CR_BASE + reg, mask, val);
        }
        for i in 0..TIA_NUM {
            let (reg_a, reg_b, mask) = BB_GAIN_TIA_G[i];
            let reg = if path == 0 { reg_a } else { reg_b };
            let val = BB_GAIN.tia_gain[gband][p][i] as i32 as u32;
            host::mmio_w32_mask(mmio, PHY_CR_BASE + reg, mask, val);
        }
    }
}
