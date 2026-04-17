//! PHY initialization — BB, RF, and NCTL register tables.
//! 1:1 port of Linux rtw89 rtw89_phy_init_reg + rtw89_phy_sel_headline.
//!
//! Table format (extracted from rtw8852b_table.c):
//!   [count:u32][addr:u32, data:u32] × count, little-endian.
//!
//! Each addr is encoded:
//!   [31:28] = cond opcode
//!               0x4 = PHY_COND_CHECK
//!               0x8 = PHY_COND_BRANCH_IF
//!               0x9 = PHY_COND_BRANCH_ELIF
//!               0xa = PHY_COND_BRANCH_ELSE
//!               0xb = PHY_COND_BRANCH_END
//!               0xf = PHY_HEADLINE_VALID (at top of table)
//!               other = normal register write
//!   [27:0]  = target (MMIO addr for writes; (rfe<<16)|pkg<<8|cv for predicates)
//!
//! Linux picks cfg_target via sel_headline in 4 fallback cases based on
//! (rfe_type, cv) from EFUSE + hal. We don't have EFUSE yet, so rfe is
//! a constant — the fallback cases still select a sane default headline.
//! If sel_headline fails entirely (no case matches), we skip the whole
//! table (same as Linux), leaving the chip untouched.

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

// SWSI register layout (Linux reg.h)
const R_SWSI_DATA_V1: u32 = 0x0370;
// const R_SWSI_BIT_MASK_V1: u32 = 0x0374;  // only needed for masked writes

// 8852B RF base addresses per path (Linux rtw8852b.c)
const RF_BASE_ADDR_A: u32 = 0xE000;
const RF_BASE_ADDR_B: u32 = 0xF000;
const RTW89_RF_ADDR_ADSEL_MASK: u32 = 1 << 16;

// 8852BE: we currently don't read EFUSE. Use rfe=0, cv=2.
// If headline selection logs "no match" for a table, change RFE here
// and recompile. Linux' rfe_type for 8852B can be 0, 1, 2, or 5.
const RFE: u8 = 0;
const CV:  u8 = 2;

fn cfg_compare(rfe: u8, cv: u8) -> u32 {
    // get_phy_compare(rfe, cv) = (rfe << 16) | cv
    ((rfe as u32) << 16) | (cv as u32)
}

fn get_phy_cond(addr: u32) -> u32     { (addr >> 28) & 0xF }
fn get_phy_target(addr: u32) -> u32   { addr & 0x0FFF_FFFF }
fn get_phy_cond_rfe(addr: u32) -> u8  { ((addr >> 16) & 0xFF) as u8 }
fn get_phy_cond_cv(addr: u32) -> u8   { (addr & 0xFF) as u8 }

pub fn init(mmio: i32) {
    host::print("  PHY: rfe="); fw::print_dec(RFE as usize);
    host::print(" cv=");         fw::print_dec(CV as usize); host::print("\n");

    host::print("  PHY: loading BB regs...\n");
    let bb = run_table(mmio, BB_TABLE, WriteKind::Bb);
    report("BB", &bb);

    bb_reset(mmio);

    host::print("  PHY: loading BB gain...\n");
    let gain = run_table(mmio, BB_GAIN_TABLE, WriteKind::Bb);
    report("BBgain", &gain);

    host::print("  PHY: loading RF path A...\n");
    let rfa = run_table(mmio, RF_A_TABLE, WriteKind::Rf(0));
    report("RF_A", &rfa);

    host::print("  PHY: loading RF path B...\n");
    let rfb = run_table(mmio, RF_B_TABLE, WriteKind::Rf(1));
    report("RF_B", &rfb);

    host::print("  PHY: loading NCTL...\n");
    let nctl = run_table(mmio, NCTL_TABLE, WriteKind::Bb);
    report("NCTL", &nctl);

    let total = bb.written + gain.written + rfa.written + rfb.written + nctl.written;
    host::print("  PHY: done (");
    fw::print_dec(total as usize);
    host::print(" regs written)\n");
}

#[derive(Copy, Clone)]
enum WriteKind {
    Bb,
    Rf(u8), // path
}

struct TableStats {
    written: u32,
    skipped: u32,
    headline_size: u32,
    headline_case: u8,   // 0=none, 1..4 from Linux
    cfg_target: u32,
    aborted: bool,
}

fn report(tag: &str, s: &TableStats) {
    host::print("    "); host::print(tag);
    host::print(": headline_size="); fw::print_dec(s.headline_size as usize);
    host::print(" case="); fw::print_dec(s.headline_case as usize);
    host::print(" cfg_target=0x"); host::print_hex32(s.cfg_target);
    host::print(" w="); fw::print_dec(s.written as usize);
    host::print(" skip="); fw::print_dec(s.skipped as usize);
    if s.aborted { host::print(" ABORTED"); }
    host::print("\n");
}

/// 1:1 Linux rtw89_phy_init_reg. Returns stats.
fn run_table(mmio: i32, table: &[u8], kind: WriteKind) -> TableStats {
    let mut stats = TableStats {
        written: 0, skipped: 0, headline_size: 0, headline_case: 0,
        cfg_target: 0, aborted: false,
    };
    if table.len() < 4 { return stats; }
    let n_regs = u32::from_le_bytes([table[0], table[1], table[2], table[3]]) as usize;

    // Step 1: sel_headline — pick the best headline entry.
    let (headline_size, headline_idx, case) = match sel_headline(table, n_regs, RFE, CV) {
        Some(r) => r,
        None => {
            // Linux: rtw89_err("invalid PHY package") + return.
            // We skip the table entirely, chip untouched.
            return stats;
        }
    };
    stats.headline_size = headline_size as u32;
    stats.headline_case = case;

    // Step 2: cfg_target = get_phy_target(headline entry address)
    let hdr_off = 4 + headline_idx * 8;
    let hdr_addr = u32::from_le_bytes([
        table[hdr_off], table[hdr_off+1], table[hdr_off+2], table[hdr_off+3],
    ]);
    let cfg_target = get_phy_target(hdr_addr);
    stats.cfg_target = cfg_target;

    // Step 3: walk the body, executing the state machine.
    let mut is_matched = true;
    let mut target_found = false;
    let mut branch_target: u32 = 0;

    for i in headline_size..n_regs {
        let off = 4 + i * 8;
        if off + 8 > table.len() { break; }
        let addr = u32::from_le_bytes([table[off], table[off+1], table[off+2], table[off+3]]);
        let data = u32::from_le_bytes([table[off+4], table[off+5], table[off+6], table[off+7]]);

        let cond = get_phy_cond(addr);
        match cond {
            PHY_COND_BRANCH_IF | PHY_COND_BRANCH_ELIF => {
                branch_target = get_phy_target(addr);
            }
            PHY_COND_BRANCH_ELSE => {
                // Linux: is_matched = false; if !target_found abort.
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
                } else if branch_target == cfg_target {
                    is_matched = true;
                    target_found = true;
                } else {
                    is_matched = false;
                    target_found = false;
                }
            }
            _ => {
                if is_matched {
                    write_entry(mmio, kind, addr, data);
                    stats.written += 1;
                } else {
                    stats.skipped += 1;
                }
            }
        }
    }

    stats
}

/// 1:1 Linux rtw89_phy_sel_headline. Returns (headline_size, chosen_idx, case)
/// on success, None when no case matches.
fn sel_headline(table: &[u8], n_regs: usize, rfe: u8, cv: u8) -> Option<(usize, usize, u8)> {
    // count headline entries (leading cond==0xf)
    let mut headline_size = 0usize;
    for i in 0..n_regs {
        let off = 4 + i * 8;
        if off + 8 > table.len() { break; }
        let addr = u32::from_le_bytes([table[off], table[off+1], table[off+2], table[off+3]]);
        if get_phy_cond(addr) != PHY_HEADLINE_VALID { break; }
        headline_size += 1;
    }
    if headline_size == 0 {
        // Linux: `*headline_size = 0; return 0;` — success, cfg_target becomes
        // get_phy_target(table->regs[0].addr). Keep same semantics: return
        // headline_idx=0 so caller reads entry 0 for cfg_target.
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

    // case 3: RFE match, CV max in table
    let mut cv_max: u8 = 0;
    let mut matched_idx: Option<usize> = None;
    for i in 0..headline_size {
        let a = hdr(i);
        if get_phy_cond_rfe(a) == rfe {
            let c = get_phy_cond_cv(a);
            if c >= cv_max {
                cv_max = c;
                matched_idx = Some(i);
            }
        }
    }
    if let Some(i) = matched_idx { return Some((headline_size, i, 3)); }

    // case 4: RFE don't care, CV max in table
    let mut cv_max: u8 = 0;
    let mut matched_idx: Option<usize> = None;
    for i in 0..headline_size {
        let a = hdr(i);
        if get_phy_cond_rfe(a) == PHY_COND_DONT_CARE {
            let c = get_phy_cond_cv(a);
            if c >= cv_max {
                cv_max = c;
                matched_idx = Some(i);
            }
        }
    }
    if let Some(i) = matched_idx { return Some((headline_size, i, 4)); }

    None
}

/// Execute one `config` call (BB or RF). Handles delay encodings.
fn write_entry(mmio: i32, kind: WriteKind, addr_full: u32, data: u32) {
    let addr = get_phy_target(addr_full);
    // Linux rtw89_phy_config_bb_reg checks reg->addr (full), but the only
    // delay encodings are at addr 0xFE/FD/FC/FB/FA/F9 with cond=0, so they
    // survive both interpretations.
    match addr {
        0xFE => { host::sleep_ms(50); return; }
        0xFD => { host::sleep_ms(5);  return; }
        0xFC => { host::sleep_ms(1);  return; }
        0xFB | 0xFA | 0xF9 => { return; } // microsecond delays, skip
        _ => {}
    }

    match kind {
        WriteKind::Bb => {
            host::mmio_w32(mmio, addr, data);
        }
        WriteKind::Rf(path) => {
            // rtw89_phy_write_rf_v1: ad_sel = addr & BIT(16)
            if addr & RTW89_RF_ADDR_ADSEL_MASK != 0 {
                // direct MMIO into base_addr[path] + ((addr & 0xff) << 2)
                let base = if path == 0 { RF_BASE_ADDR_A } else { RF_BASE_ADDR_B };
                let direct = base + ((addr & 0xFF) << 2);
                host::mmio_w32(mmio, direct, data & 0xFFFFF);
            } else {
                write_rf_swsi(mmio, path, addr, data);
            }
        }
    }
}

/// SWSI RF write — rtw89_phy_write_rf_a for mask == RFREG_MASK path.
/// Register layout: [31] BIT_MASK_EN | [30:28] PATH | [27:20] ADDR | [19:0] DATA.
fn write_rf_swsi(mmio: i32, path: u8, addr: u32, data: u32) {
    // Poll busy: bit 31 of R_SWSI_DATA_V1 must be 0 (linux uses a different
    // busy register; this is the best we have without reading swsi_busy).
    for _ in 0..1000u32 {
        if host::mmio_r32(mmio, R_SWSI_DATA_V1) & (1 << 31) == 0 {
            break;
        }
    }
    let val = (data & 0xFFFFF)                 // [19:0] VAL
            | ((addr & 0xFF) << 20)            // [27:20] ADDR
            | (((path as u32) & 0x7) << 28);   // [30:28] PATH (BIT_MASK_EN=0)
    host::mmio_w32(mmio, R_SWSI_DATA_V1, val);
}

fn bb_reset(mmio: i32) {
    host::mmio_clr8(mmio, 0x0002, 1 << 1);
    host::mmio_set8(mmio, 0x0002, 1 << 1);
}
