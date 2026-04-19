//! Efuse (OTP) parser — 1:1 port of Linux rtw89_parse_efuse_map_ax +
//! rtw8852bx_efuse parsing.
//!
//! Efuse = one-time-programmable ROM on the chip. Holds per-chip
//! calibration data that HW defaults cannot supply:
//!   - path-A/B thermal meters (needed by TSSI _tssi_set_tmeter_tbl)
//!   - TSSI CCK/MCS per-channel offsets (needed by TSSI set_efuse_to_de)
//!   - RX gain offsets per path/band (needed by set_gain_offset)
//!   - RFE type (external PA presence → DPK bypass decision)
//!   - chip MAC address (replaces our pseudo 00:11:22:33:44:55)
//!   - country code, crystal cap, coex type, etc.
//!
//! Without efuse, TSSI runs with thermal=0xff fallback (zero thermal
//! offset table), set_txpwr uses uniform hardcoded dBm, and we send
//! Probe Requests with a pseudo MAC. All three of those are broken.
//!
//! Read path (8852BE is AX, non-DAV): direct MMIO via R_AX_EFUSE_CTRL.
//!   1. enable_efuse_pwr_cut_ddv (SYS_ISO_CTRL bit sequence, +1ms)
//!   2. for each byte in physical range:
//!        write R_AX_EFUSE_CTRL = addr << 16 (clears RDY)
//!        poll until bit 29 (RDY) = 1
//!        read data from low 8 bits
//!   3. disable_efuse_pwr_cut_ddv
//!
//! Decode: physical map is sparse (block header + 4 word-enable bits
//! per 2-byte word); decode_logical expands it into a flat 2048-byte
//! logical map addressed by field offset.

use crate::host;

// ── Register addresses ──────────────────────────────────────────

const R_AX_SYS_WL_EFUSE_CTRL: u32  = 0x000A;
const B_AX_AUTOLOAD_SUS: u16       = 1 << 5;

const R_AX_SYS_ISO_CTRL: u32       = 0x0000; // 16-bit at offset, ISO bits in upper half
// B_AX_PWC_EV2EF_B14 = bit 14, B_AX_PWC_EV2EF_B15 = bit 15,
// B_AX_ISO_EB2CORE = bit 8 — all within the 16-bit ISO_CTRL reg.
const B_AX_PWC_EV2EF_B14: u16      = 1 << 14;
const B_AX_PWC_EV2EF_B15: u16      = 1 << 15;
const B_AX_ISO_EB2CORE: u16        = 1 << 8;

const R_AX_PMC_DBG_CTRL2: u32      = 0x00CC;
const B_AX_SYSON_DIS_PMCR_AX_WRMSK: u8 = 1 << 2;

const R_AX_EFUSE_CTRL: u32         = 0x0030;
const B_AX_EF_ADDR_MASK: u32       = 0x07FF_0000; // GENMASK(26,16)
const B_AX_EF_RDY: u32             = 1 << 29;

// ── Chip constants (8852BE) ─────────────────────────────────────

pub const PHY_EFUSE_SIZE: u32  = 1216;
pub const LOG_EFUSE_SIZE: u32  = 2048;
pub const PHYCAP_ADDR: u32     = 0x580;
pub const PHYCAP_SIZE: u32     = 128;

// Sec-ctrl size: first + last N bytes are secure-control, not scanned.
// chip->sec_ctrl_efuse_size for RTL8852BE = 4 (rtw8852b.c:993).
// My first version used 2 — the logical decode started 2 bytes too
// early, read sec-ctrl bytes as headers, got 0xFFFF on the first one,
// and aborted before decoding any actual block. Result: log_map stays
// all-0xFF, every parsed field reads 0xFF.
const SEC_CTRL_SIZE: u32 = 4;

// Struct rtw8852bx_efuse field offsets (logical space).
// Derived from rtw8852b_common.h struct layout with __packed semantics.
const OFF_PATH_A_TSSI:   usize = 0x210;
const OFF_PATH_B_TSSI:   usize = 0x23A;
const OFF_CHANNEL_PLAN:  usize = 0x2B8;
const OFF_XTAL_K:        usize = 0x2B9;
const OFF_RFE_TYPE:      usize = 0x2CA;
const OFF_COUNTRY_CODE:  usize = 0x2CB;
const OFF_PATH_A_THERM:  usize = 0x2D0;
const OFF_PATH_B_THERM:  usize = 0x2D1;
const OFF_RX_GAIN_2G_OFDM: usize = 0x2D4;
const OFF_RX_GAIN_2G_CCK:  usize = 0x2D6;
const OFF_RX_GAIN_5G_LOW:  usize = 0x2D8;
const OFF_RX_GAIN_5G_MID:  usize = 0x2DA;
const OFF_RX_GAIN_5G_HIGH: usize = 0x2DC;
const OFF_PCIE_MAC_ADDR:   usize = 0x400; // rtw8852bx_e_efuse (PCIe variant)

// TSSI offset sub-struct sizes (from common.h)
const TSSI_CCK_CH_GROUP_NUM:    usize = 6;
const TSSI_MCS_2G_CH_GROUP_NUM: usize = 5;
const TSSI_MCS_5G_CH_GROUP_NUM: usize = 14;
const TSSI_OFFSET_STRUCT_SIZE:  usize =
    TSSI_CCK_CH_GROUP_NUM + TSSI_MCS_2G_CH_GROUP_NUM + 7 + TSSI_MCS_5G_CH_GROUP_NUM;

// ── Parsed output struct ────────────────────────────────────────

pub const TSSI_TRIM_CH_GROUP_NUM: usize = 8;

#[derive(Copy, Clone)]
pub struct EfuseData {
    pub autoload_valid: bool,
    pub mac_addr:       [u8; 6],
    pub thermal:        [u8; 2],
    pub tssi_cck:       [[u8; TSSI_CCK_CH_GROUP_NUM]; 2],
    pub tssi_mcs_2g:    [[u8; TSSI_MCS_2G_CH_GROUP_NUM]; 2],
    pub tssi_mcs_5g:    [[u8; TSSI_MCS_5G_CH_GROUP_NUM]; 2],
    // From phycap (0x580 range, separate from logical efuse).
    pub tssi_trim:      [[i8; TSSI_TRIM_CH_GROUP_NUM]; 2],
    pub tssi_trim_valid: bool,
    pub rx_gain_2g_cck:  u8,
    pub rx_gain_2g_ofdm: u8,
    pub rx_gain_5g_low:  u8,
    pub rx_gain_5g_mid:  u8,
    pub rx_gain_5g_high: u8,
    pub rfe_type:        u8,
    pub xtal_k:          u8,
    pub channel_plan:    u8,
    pub country_code:    [u8; 2],
}

impl EfuseData {
    pub const fn empty() -> Self {
        EfuseData {
            autoload_valid: false,
            mac_addr: [0; 6],
            thermal: [0xFF; 2],
            tssi_cck:     [[0xFF; TSSI_CCK_CH_GROUP_NUM]; 2],
            tssi_mcs_2g:  [[0xFF; TSSI_MCS_2G_CH_GROUP_NUM]; 2],
            tssi_mcs_5g:  [[0xFF; TSSI_MCS_5G_CH_GROUP_NUM]; 2],
            tssi_trim:    [[0; TSSI_TRIM_CH_GROUP_NUM]; 2],
            tssi_trim_valid: false,
            rx_gain_2g_cck: 0xFF,
            rx_gain_2g_ofdm: 0xFF,
            rx_gain_5g_low: 0xFF,
            rx_gain_5g_mid: 0xFF,
            rx_gain_5g_high: 0xFF,
            rfe_type: 0xFF,
            xtal_k:   0xFF,
            channel_plan: 0xFF,
            country_code: [0xFF; 2],
        }
    }
}

// ── Power-cut sequence (enable_efuse_pwr_cut_ddv / disable_*) ───

fn print_u8(v: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let buf = [HEX[((v >> 4) & 0xF) as usize], HEX[(v & 0xF) as usize]];
    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    host::print(s);
}

fn r16(mmio: i32, off: u32) -> u16 {
    host::mmio_r16(mmio, off)
}
fn w16(mmio: i32, off: u32, v: u16) {
    host::mmio_w16(mmio, off, v);
}
fn r8(mmio: i32, off: u32) -> u8 {
    let aligned = off & !0x3;
    let shift = (off & 0x3) * 8;
    ((host::mmio_r32(mmio, aligned) >> shift) & 0xFF) as u8
}
fn w8(mmio: i32, off: u32, v: u8) {
    // 8-bit write via set/clr in the containing 32-bit word.
    let aligned = off & !0x3;
    let shift = (off & 0x3) * 8;
    let mask: u32 = 0xFFu32 << shift;
    let cur = host::mmio_r32(mmio, aligned);
    host::mmio_w32(mmio, aligned, (cur & !mask) | ((v as u32) << shift));
}

fn enable_pwr_cut_ddv(mmio: i32) {
    // PMC_DBG_CTRL2 |= DIS_PMCR_AX_WRMSK
    w8(mmio, R_AX_PMC_DBG_CTRL2, r8(mmio, R_AX_PMC_DBG_CTRL2) | B_AX_SYSON_DIS_PMCR_AX_WRMSK);
    // SYS_ISO_CTRL |= PWC_EV2EF_B14
    w16(mmio, R_AX_SYS_ISO_CTRL, r16(mmio, R_AX_SYS_ISO_CTRL) | B_AX_PWC_EV2EF_B14);
    host::sleep_ms(1); // fsleep(1000 us) — at least 1 ms
    // SYS_ISO_CTRL |= PWC_EV2EF_B15
    w16(mmio, R_AX_SYS_ISO_CTRL, r16(mmio, R_AX_SYS_ISO_CTRL) | B_AX_PWC_EV2EF_B15);
    // SYS_ISO_CTRL &= ~ISO_EB2CORE
    w16(mmio, R_AX_SYS_ISO_CTRL, r16(mmio, R_AX_SYS_ISO_CTRL) & !B_AX_ISO_EB2CORE);
}

fn disable_pwr_cut_ddv(mmio: i32) {
    w16(mmio, R_AX_SYS_ISO_CTRL, r16(mmio, R_AX_SYS_ISO_CTRL) | B_AX_ISO_EB2CORE);
    w16(mmio, R_AX_SYS_ISO_CTRL, r16(mmio, R_AX_SYS_ISO_CTRL) & !B_AX_PWC_EV2EF_B15);
    host::sleep_ms(1);
    w16(mmio, R_AX_SYS_ISO_CTRL, r16(mmio, R_AX_SYS_ISO_CTRL) & !B_AX_PWC_EV2EF_B14);
    w8(mmio, R_AX_PMC_DBG_CTRL2, r8(mmio, R_AX_PMC_DBG_CTRL2) & !B_AX_SYSON_DIS_PMCR_AX_WRMSK);
}

/// Read one byte from physical efuse at `addr` via R_AX_EFUSE_CTRL.
/// Returns None if RDY doesn't come up within the poll window.
fn read_one(mmio: i32, addr: u32) -> Option<u8> {
    let req = (addr << 16) & B_AX_EF_ADDR_MASK; // RDY=0 implicit
    host::mmio_w32(mmio, R_AX_EFUSE_CTRL, req);

    // Poll up to ~1 ms for RDY.
    for _ in 0..10_000u32 {
        let v = host::mmio_r32(mmio, R_AX_EFUSE_CTRL);
        if v & B_AX_EF_RDY != 0 {
            return Some((v & 0xFF) as u8);
        }
        core::hint::spin_loop();
    }
    None
}

/// Dump `size` bytes of physical efuse starting at `addr` into `out`.
/// Returns false on timeout. Caller must have enabled pwr-cut first.
fn dump_physical(mmio: i32, addr: u32, size: usize, out: &mut [u8]) -> bool {
    for i in 0..size {
        match read_one(mmio, addr + i as u32) {
            Some(b) => out[i] = b,
            None => return false,
        }
    }
    true
}

// ── Physical → logical decode (1:1 rtw89_dump_logical_efuse_map) ─

fn invalid_header(h1: u8, h2: u8) -> bool { h1 == 0xFF || h2 == 0xFF }
fn blk_idx(h1: u8, h2: u8) -> u16 { ((h2 as u16 & 0xF0) >> 4) | ((h1 as u16 & 0x0F) << 4) }
fn blk_to_log(blk: u16, word: u8) -> usize { ((blk as usize) << 3) + ((word as usize) << 1) }

/// Expand the sparse physical map into a flat logical map.
/// Walks 2-byte headers; each header declares a block-idx and word-
/// enable bits. For every word whose enable bit is clear, 2 payload
/// bytes follow. Stops on 0xFFFF header (unwritten region).
fn decode_logical(phy: &[u8], log: &mut [u8]) {
    let phys_size = phy.len();
    let log_size  = log.len();
    // fill log with 0xFF first
    for b in log.iter_mut() { *b = 0xFF; }

    let mut phy_idx = SEC_CTRL_SIZE as usize;
    while phy_idx + 1 < phys_size - SEC_CTRL_SIZE as usize {
        let h1 = phy[phy_idx];
        let h2 = phy[phy_idx + 1];
        if invalid_header(h1, h2) { break; }

        let blk = blk_idx(h1, h2);
        let word_en = h2 & 0x0F;
        phy_idx += 2;

        for i in 0u8..4 {
            if word_en & (1 << i) != 0 { continue; } // bit set = word NOT present
            let log_idx = blk_to_log(blk, i);
            if phy_idx + 1 > phys_size - SEC_CTRL_SIZE as usize
                || log_idx + 1 >= log_size { return; }
            log[log_idx] = phy[phy_idx];
            log[log_idx + 1] = phy[phy_idx + 1];
            phy_idx += 2;
        }
    }
}

// ── Field extraction (1:1 rtw8852bx_efuse_parsing_*) ────────────

fn parse_fields(log: &[u8]) -> EfuseData {
    let mut e = EfuseData::empty();

    // MAC address (PCIe variant)
    for i in 0..6 { e.mac_addr[i] = log[OFF_PCIE_MAC_ADDR + i]; }

    // Thermal
    e.thermal[0] = log[OFF_PATH_A_THERM];
    e.thermal[1] = log[OFF_PATH_B_THERM];

    // TSSI offsets per path
    for path in 0..2usize {
        let base = if path == 0 { OFF_PATH_A_TSSI } else { OFF_PATH_B_TSSI };
        for i in 0..TSSI_CCK_CH_GROUP_NUM {
            e.tssi_cck[path][i] = log[base + i];
        }
        let b2 = base + TSSI_CCK_CH_GROUP_NUM;
        for i in 0..TSSI_MCS_2G_CH_GROUP_NUM {
            e.tssi_mcs_2g[path][i] = log[b2 + i];
        }
        let b5 = b2 + TSSI_MCS_2G_CH_GROUP_NUM + 7; // +rsvd[7]
        for i in 0..TSSI_MCS_5G_CH_GROUP_NUM {
            e.tssi_mcs_5g[path][i] = log[b5 + i];
        }
        let _ = TSSI_OFFSET_STRUCT_SIZE; // silence unused warning in no_std
    }

    // RX gain offsets
    e.rx_gain_2g_cck  = log[OFF_RX_GAIN_2G_CCK];
    e.rx_gain_2g_ofdm = log[OFF_RX_GAIN_2G_OFDM];
    e.rx_gain_5g_low  = log[OFF_RX_GAIN_5G_LOW];
    e.rx_gain_5g_mid  = log[OFF_RX_GAIN_5G_MID];
    e.rx_gain_5g_high = log[OFF_RX_GAIN_5G_HIGH];

    // RFE / channel_plan / xtal / country
    e.rfe_type     = log[OFF_RFE_TYPE];
    e.xtal_k       = log[OFF_XTAL_K];
    e.channel_plan = log[OFF_CHANNEL_PLAN];
    e.country_code[0] = log[OFF_COUNTRY_CODE];
    e.country_code[1] = log[OFF_COUNTRY_CODE + 1];

    e
}

// ── Public entry ────────────────────────────────────────────────

/// Read and parse the full efuse. Returns EfuseData::empty() on any
/// failure (the defaults are sentinel 0xFF values that consumers can
/// detect as "not available").
pub fn read(mmio: i32) -> EfuseData {
    host::print("  EFUSE: reading physical map...\n");

    // 1. Check autoload status. bit is in upper byte of R_AX_SYS_WL_EFUSE_CTRL.
    let autoload_bits = host::mmio_r16(mmio, R_AX_SYS_WL_EFUSE_CTRL);
    let autoload = autoload_bits & B_AX_AUTOLOAD_SUS != 0;
    if !autoload {
        host::print("  EFUSE: autoload not ready (autoload_sus=0) — defaults in use\n");
        return EfuseData::empty();
    }

    enable_pwr_cut_ddv(mmio);

    // 2. Static buffers — stack-friendly since WASM pages are small.
    //    PHY_EFUSE_SIZE = 1216, LOG = 2048.
    let mut phy_map = [0u8; 1216];
    let mut log_map = [0xFFu8; 2048];

    let ok = dump_physical(mmio, 0, PHY_EFUSE_SIZE as usize, &mut phy_map);

    disable_pwr_cut_ddv(mmio);

    if !ok {
        host::print("  EFUSE: physical dump timed out — defaults in use\n");
        return EfuseData::empty();
    }

    // 3. Decode sparse physical → flat logical.
    decode_logical(&phy_map, &mut log_map);

    // 4. Extract fields.
    let mut e = parse_fields(&log_map);
    e.autoload_valid = true;

    // 4b. Dump the 128-byte phycap region at 0x580 (separate from the
    //     logical efuse) and pull tssi_trim per path from it.
    //     Port of rtw8852bx_phycap_parsing_tssi (common.c:281).
    //     Trim addresses are in *descending* order:
    //       path A: 0x5D6, 0x5D5, 0x5D4, ...
    //       path B: 0x5AB, 0x5AA, 0x5A9, ...
    enable_pwr_cut_ddv(mmio);
    let mut phycap = [0u8; 128];
    let pcap_ok = dump_physical(mmio, PHYCAP_ADDR, PHYCAP_SIZE as usize, &mut phycap);
    disable_pwr_cut_ddv(mmio);

    if pcap_ok {
        let base = PHYCAP_ADDR as usize;
        let trim_addr = [0x5D6usize, 0x5ABusize];
        let mut pg = false;
        for i in 0..2 {
            for j in 0..TSSI_TRIM_CH_GROUP_NUM {
                let off = trim_addr[i].wrapping_sub(base).wrapping_sub(j);
                if off < phycap.len() {
                    e.tssi_trim[i][j] = phycap[off] as i8;
                    if phycap[off] != 0xFF { pg = true; }
                }
            }
        }
        if pg {
            e.tssi_trim_valid = true;
        } else {
            // No valid trim programmed — Linux zeros the whole array.
            e.tssi_trim = [[0; TSSI_TRIM_CH_GROUP_NUM]; 2];
        }
    }

    // 5. Log highlights (MAC as 6×u8, thermal/rfe as u8).
    host::print("  EFUSE: MAC=");
    for i in 0..6 {
        print_u8(e.mac_addr[i]);
        if i < 5 { host::print(":"); }
    }
    host::print(" thermA=0x"); print_u8(e.thermal[0]);
    host::print(" thermB=0x"); print_u8(e.thermal[1]);
    host::print(" rfe=0x");    print_u8(e.rfe_type);
    host::print(" cc=");       print_u8(e.country_code[0]); print_u8(e.country_code[1]);
    host::print("\n");

    e
}
