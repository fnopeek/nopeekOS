//! PHY initialization — BB, RF, and NCTL register tables
//!
//! Tables extracted from Linux rtw89 rtw8852b_table.c.
//! Format: [count:u32][addr:u32, data:u32] × count (little-endian).
//!
//! BB/NCTL: direct MMIO writes to PHY register space.
//! RF: indirect write via SWSI interface at 0x0370.

use crate::host;
use crate::fw;

// Embedded PHY table binaries
static BB_TABLE: &[u8]      = include_bytes!("rtw8852b_bb.bin");
static BB_GAIN_TABLE: &[u8] = include_bytes!("rtw8852b_bb_gain.bin");
static RF_A_TABLE: &[u8]    = include_bytes!("rtw8852b_rf_a.bin");
static RF_B_TABLE: &[u8]    = include_bytes!("rtw8852b_rf_b.bin");
static NCTL_TABLE: &[u8]    = include_bytes!("rtw8852b_nctl.bin");

// SWSI RF register access (from Linux reg.h)
const R_SWSI_DATA_V1: u32     = 0x0370;
const R_SWSI_BIT_MASK_V1: u32 = 0x0374;

/// Initialize all PHY registers. Call after MAC init and BB/RF enable.
pub fn init(mmio: i32) {
    host::print("  PHY: loading BB regs...\n");
    let bb = write_bb_table(mmio, BB_TABLE);

    // BB reset after main table (rtw89_phy_bb_reset)
    bb_reset(mmio);

    host::print("  PHY: loading BB gain...\n");
    let gain = write_bb_table(mmio, BB_GAIN_TABLE);

    host::print("  PHY: loading RF path A...\n");
    let rf_a = write_rf_table(mmio, RF_A_TABLE, 0); // path A

    host::print("  PHY: loading RF path B...\n");
    let rf_b = write_rf_table(mmio, RF_B_TABLE, 1); // path B

    host::print("  PHY: loading NCTL...\n");
    let nctl = write_bb_table(mmio, NCTL_TABLE);

    host::print("  PHY: done (");
    fw::print_dec(bb + gain + rf_a + rf_b + nctl);
    host::print(" regs)\n");
}

/// Write a BB/NCTL table — direct MMIO writes.
fn write_bb_table(mmio: i32, table: &[u8]) -> usize {
    if table.len() < 4 { return 0; }
    let count = u32::from_le_bytes([table[0], table[1], table[2], table[3]]) as usize;
    let mut written = 0;

    for i in 0..count {
        let off = 4 + i * 8;
        if off + 8 > table.len() { break; }
        let addr = u32::from_le_bytes([table[off], table[off+1], table[off+2], table[off+3]]);
        let data = u32::from_le_bytes([table[off+4], table[off+5], table[off+6], table[off+7]]);

        // Handle delay commands (from rtw89_phy_config_bb_reg)
        match addr {
            0xFE => host::sleep_ms(50),
            0xFD => host::sleep_ms(5),
            0xFC => host::sleep_ms(1),
            0xFB | 0xFA | 0xF9 => { /* short delays, skip in WASM */ }
            _ => {
                host::mmio_w32(mmio, addr, data);
                written += 1;
            }
        }
    }
    written
}

/// Write an RF table via SWSI indirect interface.
/// Each entry: write_rf(path, addr, RFREG_MASK, data)
fn write_rf_table(mmio: i32, table: &[u8], path: u8) -> usize {
    if table.len() < 4 { return 0; }
    let count = u32::from_le_bytes([table[0], table[1], table[2], table[3]]) as usize;
    let mut written = 0;

    for i in 0..count {
        let off = 4 + i * 8;
        if off + 8 > table.len() { break; }
        let addr = u32::from_le_bytes([table[off], table[off+1], table[off+2], table[off+3]]);
        let data = u32::from_le_bytes([table[off+4], table[off+5], table[off+6], table[off+7]]);

        // Handle delay commands
        match addr {
            0xFE => host::sleep_ms(50),
            0xFD => host::sleep_ms(5),
            0xFC => host::sleep_ms(1),
            0xFB | 0xFA | 0xF9 => {}
            _ => {
                write_rf_swsi(mmio, path, addr, data);
                written += 1;
            }
        }
    }
    written
}

/// Write a single RF register via SWSI (Serial Wire Serial Interface).
/// Linux: rtw89_phy_write_rf_a() in phy.c
fn write_rf_swsi(mmio: i32, path: u8, addr: u32, data: u32) {
    // Poll busy: bit 31 of R_SWSI_DATA_V1 must be 0
    for _ in 0..1000u32 {
        if host::mmio_r32(mmio, R_SWSI_DATA_V1) & (1 << 31) == 0 {
            break;
        }
    }

    // Pack value: data[19:0] | addr[7:0]<<20 | path[2:0]<<28
    // mask == RFREG_MASK(0xFFFFF) → b_msk_en = false (bit 31 = 0)
    let val = (data & 0xFFFFF)
            | ((addr & 0xFF) << 20)
            | ((path as u32 & 0x7) << 28);

    host::mmio_w32(mmio, R_SWSI_DATA_V1, val);
}

/// BB reset — toggle BB_GLB_RSTN after loading BB table.
/// Linux: rtw89_phy_bb_reset → __rtw8852bx_bb_reset
fn bb_reset(mmio: i32) {
    // Toggle BB global reset
    host::mmio_clr8(mmio, 0x0002, 1 << 1); // clear FEN_BB_GLB_RSTN
    host::mmio_set8(mmio, 0x0002, 1 << 1); // set FEN_BB_GLB_RSTN
}
