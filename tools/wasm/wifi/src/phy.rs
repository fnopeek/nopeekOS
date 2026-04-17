//! PHY initialization — BB, RF, and NCTL register tables
//!
//! Tables extracted from Linux rtw89 rtw8852b_table.c.
//! Format: [count:u32][addr:u32, data:u32] × count (little-endian).
//!
//! Each addr is 32-bit encoded:
//!   [31:28] = cond opcode (0x4=CHECK, 0x8=IF, 0x9=ELIF, 0xa=ELSE,
//!             0xb=END, 0xf=HEADLINE, other = normal write)
//!   [27:0]  = target (MMIO address for normal writes, or
//!             (rfe<<16) | cv for branch predicates)
//!
//! Branches are evaluated against cfg_target = (rfe<<16) | cv.
//! For 8852BE we hard-code rfe=0 (no EFUSE read yet), cv=2.

use crate::host;
use crate::fw;

// Embedded PHY table binaries
static BB_TABLE: &[u8]      = include_bytes!("rtw8852b_bb.bin");
static BB_GAIN_TABLE: &[u8] = include_bytes!("rtw8852b_bb_gain.bin");
static RF_A_TABLE: &[u8]    = include_bytes!("rtw8852b_rf_a.bin");
static RF_B_TABLE: &[u8]    = include_bytes!("rtw8852b_rf_b.bin");
static NCTL_TABLE: &[u8]    = include_bytes!("rtw8852b_nctl.bin");

// Branch opcodes (Linux phy.h)
const PHY_COND_CHECK:       u32 = 0x4;
const PHY_COND_BRANCH_IF:   u32 = 0x8;
const PHY_COND_BRANCH_ELIF: u32 = 0x9;
const PHY_COND_BRANCH_ELSE: u32 = 0xa;
const PHY_COND_BRANCH_END:  u32 = 0xb;
const PHY_HEADLINE_VALID:   u32 = 0xf;

// SWSI RF register access (Linux reg.h)
const R_SWSI_DATA_V1: u32 = 0x0370;

// 8852BE defaults — EFUSE-derived in Linux, hard-coded for first pass.
// rfe_type=0 is typical for consumer cards; cv=2 is confirmed from SYS_CFG1.
const RFE: u32 = 0;
const CV:  u32 = 2;

fn cfg_target() -> u32 {
    (RFE << 16) | CV
}

/// Initialize all PHY registers.
pub fn init(mmio: i32) {
    host::print("  PHY: loading BB regs...\n");
    let (bb_w, bb_s) = write_bb_table(mmio, BB_TABLE);
    report_counts("BB", bb_w, bb_s);

    // BB reset after main table (rtw89_phy_bb_reset)
    bb_reset(mmio);

    host::print("  PHY: loading BB gain...\n");
    let (g_w, g_s) = write_bb_table(mmio, BB_GAIN_TABLE);
    report_counts("BBgain", g_w, g_s);

    host::print("  PHY: loading RF path A...\n");
    let (a_w, a_s) = write_rf_table(mmio, RF_A_TABLE, 0);
    report_counts("RF_A", a_w, a_s);

    host::print("  PHY: loading RF path B...\n");
    let (b_w, b_s) = write_rf_table(mmio, RF_B_TABLE, 1);
    report_counts("RF_B", b_w, b_s);

    host::print("  PHY: loading NCTL...\n");
    let (n_w, n_s) = write_bb_table(mmio, NCTL_TABLE);
    report_counts("NCTL", n_w, n_s);

    host::print("  PHY: done (");
    fw::print_dec(bb_w + g_w + a_w + b_w + n_w);
    host::print(" regs written, ");
    fw::print_dec(bb_s + g_s + a_s + b_s + n_s);
    host::print(" skipped)\n");
}

fn report_counts(tag: &str, written: usize, skipped: usize) {
    host::print("    "); host::print(tag); host::print(": ");
    fw::print_dec(written); host::print(" written / ");
    fw::print_dec(written + skipped); host::print(" total\n");
}

/// Parse a table (BB/NCTL) with the full Linux conditional state machine.
/// Returns (written, skipped).
fn write_bb_table(mmio: i32, table: &[u8]) -> (usize, usize) {
    parse_table(table, |addr, data| {
        match addr {
            0xFE => { host::sleep_ms(50); None }
            0xFD => { host::sleep_ms(5);  None }
            0xFC => { host::sleep_ms(1);  None }
            0xFB | 0xFA | 0xF9 => None,
            _ => Some((addr, data)),
        }
    }, |target_mmio, data| {
        host::mmio_w32(mmio, target_mmio, data);
    })
}

/// RF tables: same conditional logic, but writes go through SWSI.
fn write_rf_table(mmio: i32, table: &[u8], path: u8) -> (usize, usize) {
    parse_table(table, |addr, data| {
        match addr {
            0xFE => { host::sleep_ms(50); None }
            0xFD => { host::sleep_ms(5);  None }
            0xFC => { host::sleep_ms(1);  None }
            0xFB | 0xFA | 0xF9 => None,
            _ => Some((addr, data)),
        }
    }, |target, data| {
        write_rf_swsi(mmio, path, target, data);
    })
}

/// Core table parser — shared by BB and RF paths.
/// `filter(addr, data)` returns Some((effective_addr, data)) to write, or None to skip.
/// `write(addr, data)` performs the actual MMIO/SWSI write.
fn parse_table<F, W>(table: &[u8], filter: F, mut write: W) -> (usize, usize)
where
    F: Fn(u32, u32) -> Option<(u32, u32)>,
    W: FnMut(u32, u32),
{
    if table.len() < 4 { return (0, 0); }
    let count = u32::from_le_bytes([table[0], table[1], table[2], table[3]]) as usize;

    let mut written = 0usize;
    let mut skipped = 0usize;
    let mut is_matched = true;
    let mut target_found = false;
    let mut branch_target: u32 = 0;
    let cfg = cfg_target();

    for i in 0..count {
        let off = 4 + i * 8;
        if off + 8 > table.len() { break; }
        let addr = u32::from_le_bytes([table[off], table[off+1], table[off+2], table[off+3]]);
        let data = u32::from_le_bytes([table[off+4], table[off+5], table[off+6], table[off+7]]);

        let cond = (addr >> 28) & 0xF;
        let target = addr & 0x0FFF_FFFF;

        match cond {
            PHY_COND_BRANCH_IF | PHY_COND_BRANCH_ELIF => {
                branch_target = target;
            }
            PHY_COND_CHECK => {
                if target_found {
                    is_matched = false;
                } else if branch_target == cfg {
                    is_matched = true;
                    target_found = true;
                } else {
                    is_matched = false;
                }
            }
            PHY_COND_BRANCH_ELSE => {
                is_matched = !target_found;
            }
            PHY_COND_BRANCH_END => {
                is_matched = true;
                target_found = false;
            }
            PHY_HEADLINE_VALID => {
                // Headline metadata at the top of the table — tells Linux
                // which target to match. We hard-code cfg_target, so skip.
            }
            _ => {
                if is_matched {
                    if let Some((eff_addr, eff_data)) = filter(target, data) {
                        write(eff_addr, eff_data);
                        written += 1;
                    } else {
                        // delay or explicit skip — don't count as skipped
                    }
                } else {
                    skipped += 1;
                }
            }
        }
    }

    (written, skipped)
}

/// Write a single RF register via SWSI.
fn write_rf_swsi(mmio: i32, path: u8, addr: u32, data: u32) {
    // Poll busy: bit 31 of R_SWSI_DATA_V1 must be 0
    for _ in 0..1000u32 {
        if host::mmio_r32(mmio, R_SWSI_DATA_V1) & (1 << 31) == 0 {
            break;
        }
    }

    let val = (data & 0xFFFFF)
            | ((addr & 0xFF) << 20)
            | ((path as u32 & 0x7) << 28);

    host::mmio_w32(mmio, R_SWSI_DATA_V1, val);
}

/// BB reset — toggle BB_GLB_RSTN after loading BB table.
fn bb_reset(mmio: i32) {
    host::mmio_clr8(mmio, 0x0002, 1 << 1);
    host::mmio_set8(mmio, 0x0002, 1 << 1);
}
