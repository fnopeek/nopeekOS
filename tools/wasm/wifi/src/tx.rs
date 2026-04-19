//! TX ring for CH8 (MGMT Band 0).
//!
//! Strict Linux port (rtw89_pci_ops_tx_write path, AX chip, non-V1).
//! See TX_AUDIT.md for the complete field/register reference.
//!
//! Layout per WD page (128 B, Linux RTW89_PCI_TXWD_PAGE_SIZE):
//!   0..24   TXWD Body  (6 dwords; AX fills only dw0/2/3)
//!   24..48  TXWD Info  (6 dwords; only if en_wd_info=1)
//!   48..56  TXWP Info  (seq0..3)
//!   56..64  Addr Info  (length | option | dma_low, 1 entry)
//!
//! The 802.11 frame lives in a SEPARATE DMA buffer (frame pool). The
//! addr_info points at it via DMA address.

use crate::host;
use crate::regs;

// ── Ring dimensions (smaller than Linux for Phase 1; easy to grow) ──

const BD_NUM:        u32 = 32;   // 32 BDs × 8 B = 256 B (fits in 1 page)
const WD_PAGES:      u32 = 16;   // 16 WD pages
const WD_PAGE_SIZE:  u32 = 128;  // Linux constant
const FRAME_SLOT:    u32 = 256;  // 256 B per frame (AUTH/ASSOC fit)

// WD offsets
const OFF_BODY:      u32 = 0;
const OFF_INFO:      u32 = 24;
const OFF_WP:        u32 = 48;
const OFF_ADDR:      u32 = 56;
const WD_HDR_TOTAL:  u32 = 64;

// BDRAM config for CH8 (single-band): start=20 max=4 min=1 (pci.c:1716).
const BDRAM_SIDX:    u32 = 20;
const BDRAM_MAX:     u32 = 4;
const BDRAM_MIN:     u32 = 1;

// QSEL for MGMT Band 0 (txrx.h RTW89_TX_QSEL_B0_MGMT).
const QSEL_B0_MGMT:  u32 = 0x12;

// HW sequence mode values for mgmt (core.c RTW89_MGMT_HW_SSN_SEL / _MODE).
const HW_SSN_SEL:    u32 = 1;
const HW_SSN_MODE:   u32 = 1;

// Rate (txrx.h): 0x0 = CCK1 (1 Mbps DSSS). 2.4G mgmt default.
const RATE_CCK1:     u32 = 0x0;

pub struct TxRing {
    pub bd_handle:    i32,
    pub bd_phys:      u64,
    pub wd_handle:    i32,
    pub wd_phys:      u64,
    pub frame_handle: i32,
    pub frame_phys:   u64,
    pub wp:           u16,    // host write pointer (BD index, 0..BD_NUM-1)
    pub next_slot:    u32,    // round-robin allocator for WD/frame slots
}

pub fn alloc() -> Option<TxRing> {
    // BD ring: 256 B needs 1 page
    let bd_handle = host::dma_alloc(1);
    if bd_handle < 0 { return None; }
    let bd_phys = host::dma_phys(bd_handle);

    // WD page pool: 16 × 128 = 2 KB, fits in 1 page
    let wd_handle = host::dma_alloc(1);
    if wd_handle < 0 { return None; }
    let wd_phys = host::dma_phys(wd_handle);

    // Frame pool: 16 slots × 256 B = 4 KB
    let frame_handle = host::dma_alloc(1);
    if frame_handle < 0 { return None; }
    let frame_phys = host::dma_phys(frame_handle);

    // Zero BD ring
    for off in (0..BD_NUM * 8).step_by(4) {
        host::dma_w32(bd_handle, off, 0);
    }
    // Zero WD pool
    for off in (0..WD_PAGES * WD_PAGE_SIZE).step_by(4) {
        host::dma_w32(wd_handle, off, 0);
    }

    Some(TxRing {
        bd_handle, bd_phys,
        wd_handle, wd_phys,
        frame_handle, frame_phys,
        wp: 0,
        next_slot: 0,
    })
}

/// Program the CH8 TXBD ring into HW. Call once after alloc(), before any
/// send. Mirrors rtw89_pci_reset_trx_rings for CH8 (pci.c:1776).
pub fn init_ch8(mmio: i32, ring: &TxRing) {
    // 1. Reset wp/rp in HW
    host::mmio_w32(mmio, regs::R_AX_TXBD_RWPTR_CLR1, regs::B_AX_CLR_CH8_IDX);

    // 2. DMA base address (low + high)
    host::mmio_w32(mmio, regs::R_AX_CH8_TXBD_DESA_L, ring.bd_phys as u32);
    host::mmio_w32(mmio, regs::R_AX_CH8_TXBD_DESA_H, (ring.bd_phys >> 32) as u32);

    // 3. Ring size (16-bit write, like Linux)
    host::mmio_w16(mmio, regs::R_AX_CH8_TXBD_NUM, BD_NUM as u16);

    // 4. BDRAM config: start_idx[7:0] | max_num[15:8] | min_num[23:16]
    let bdram = (BDRAM_SIDX & 0xFF)
              | ((BDRAM_MAX & 0xFF) << 8)
              | ((BDRAM_MIN & 0xFF) << 16);
    host::mmio_w32(mmio, regs::R_AX_CH8_BDRAM_CTRL, bdram);

    host::fence();
}

/// Send a management frame (Probe Request, AUTH, ASSOC) via CH8.
/// `frame` is the raw 802.11 frame (MAC header + body), unencrypted.
/// Returns true on enqueue success (does NOT wait for TX completion).
/// The multicast/broadcast decision is taken from bit 0 of addr1 (DA) —
/// Linux core.c:1569 `rts_en = !is_bmc`: unicast frames do RTS, group
/// frames skip RTS.
pub fn send_mgmt(mmio: i32, ring: &mut TxRing, frame: &[u8]) -> bool {
    if frame.len() == 0 || frame.len() as u32 > FRAME_SLOT { return false; }
    // addr1 (DA) is at offset 4..10. A group address has bit 0 of the
    // first octet set.
    let is_bmc = frame.len() >= 5 && (frame[4] & 0x01) != 0;

    // Round-robin WD page + frame slot (paired index).
    let slot = ring.next_slot;
    ring.next_slot = (ring.next_slot + 1) % WD_PAGES;

    let wd_off    = slot * WD_PAGE_SIZE;
    let wd_phys   = ring.wd_phys + wd_off as u64;
    let frame_off = slot * FRAME_SLOT;
    let frame_phys = ring.frame_phys + frame_off as u64;

    // ── 1. Copy frame into DMA ─────────────────────────────────────
    host::dma_write_buf(ring.frame_handle, frame_off, frame);

    // ── 2. TXWD Body (AX: dw0/2/3 only; dw1/4/5 stay zero) ─────────
    // dw0: WP_OFFSET=0 | WD_INFO_EN=1 | CHANNEL_DMA=8 | WD_PAGE=1
    //      | HW_SSN_SEL=1 | HW_SSN_MODE=1
    let dw0: u32 = (1u32 << 22)                    // WD_INFO_EN
                 | (8u32  << 16)                   // CHANNEL_DMA = CH8
                 | (1u32  << 7)                    // WD_PAGE
                 | (HW_SSN_SEL  << 2)              // HW_SSN_SEL
                 | HW_SSN_MODE;                    // HW_SSN_MODE
    host::dma_w32(ring.wd_handle, wd_off + OFF_BODY + 0, dw0);
    // dw1 = 0 (AX doesn't use body1 fields in fill_txdesc)
    host::dma_w32(ring.wd_handle, wd_off + OFF_BODY + 4, 0);
    // dw2: MACID=0 | QSEL=0x12 | TXPKT_SIZE=frame.len()
    let dw2: u32 = (0u32 << 24)                    // MACID
                 | (QSEL_B0_MGMT << 17)            // QSEL
                 | (frame.len() as u32 & 0x3FFF);  // TXPKT_SIZE
    host::dma_w32(ring.wd_handle, wd_off + OFF_BODY + 8, dw2);
    // dw3..dw5 = 0 (seq=0, HW fills via HW_SSN_SEL)
    host::dma_w32(ring.wd_handle, wd_off + OFF_BODY + 12, 0);
    host::dma_w32(ring.wd_handle, wd_off + OFF_BODY + 16, 0);
    host::dma_w32(ring.wd_handle, wd_off + OFF_BODY + 20, 0);

    // ── 3. TXWD Info (only when en_wd_info=1) ──────────────────────
    // dw0: USE_RATE=1 | DATA_BW=0 | GI_LTF=0 | DATA_RATE=CCK1 | DISDATAFB=1
    let info0: u32 = (1u32 << 30)                  // USE_RATE
                   | (RATE_CCK1 << 16)             // DATA_RATE
                   | (1u32 << 10);                 // DISDATAFB
    host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 0,  info0);
    host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 4,  0);      // info1
    host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 8,  0);      // info2 (no SEC)
    host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 12, 0);      // info3 (no rpt)
    // info4: HW_RTS_EN=BIT(31) always, RTS_EN=BIT(27) = !is_bmc.
    let info4: u32 = (1u32 << 31) | if is_bmc { 0 } else { 1u32 << 27 };
    host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 16, info4);
    host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 20, 0);      // info5

    // ── 4. TXWP Info (seq0 = slot | VALID; rest zero) ──────────────
    let seq0: u32 = (slot & 0xFFFF) | (1u32 << 15);   // RTW89_PCI_TXWP_VALID
    host::dma_w32(ring.wd_handle, wd_off + OFF_WP + 0, seq0);
    host::dma_w32(ring.wd_handle, wd_off + OFF_WP + 4, 0);

    // ── 5. Addr Info (1 entry, points at frame DMA) ───────────────
    // length[15:0] | option[31:16] (MSDU_LS | NUM(1) | DMA_HI<<6)
    let frame_dma_hi = (frame_phys >> 32) as u32 & 0xFF;
    let option: u32 = (1u32 << 15)                  // MSDU_LS
                    | 1                             // NUM(1)
                    | (frame_dma_hi << 6);          // DMA_HI
    let w0: u32 = (frame.len() as u32 & 0xFFFF) | (option << 16);
    host::dma_w32(ring.wd_handle, wd_off + OFF_ADDR + 0, w0);
    host::dma_w32(ring.wd_handle, wd_off + OFF_ADDR + 4, frame_phys as u32);

    host::fence();

    // ── 6. TXBD (points at WD page) ───────────────────────────────
    // Linux pci.c:1539: `txwd->len = txwd_len + txwp_len + txaddr_info_len`
    // — 24 (body) + 24 (info) + 8 (wp) + 8 (one addr entry) = 64 bytes.
    // The 802.11 frame lives in a SEPARATE DMA buffer referenced by the
    // addr_info entry, so its length is NOT added here. Adding it caused
    // HW to read 45 bytes of zero-padded slop past the real metadata and
    // silently drop the frame — visible as TX_COUNTER=0 despite CH8_BUSY
    // toggling and TXBD_IDX advancing (v1.30/v1.31 diagnostic).
    let bd_off        = ring.wp as u32 * 8;
    let wd_total_len  = WD_HDR_TOTAL;
    let wd_dma_hi     = (wd_phys >> 32) as u32 & 0xFF;
    // BD word0: length[15:0] | opt[31:16] (LS | DMA_HI<<6)
    let bd_opt: u32   = (1u32 << 14) | (wd_dma_hi << 6);  // LS + DMA_HI
    let bd_w0: u32    = (wd_total_len & 0xFFFF) | (bd_opt << 16);
    host::dma_w32(ring.bd_handle, bd_off + 0, bd_w0);
    host::dma_w32(ring.bd_handle, bd_off + 4, wd_phys as u32);

    host::fence();

    // ── 7. Kick-off: advance wp and write it to HW ────────────────
    ring.wp = ((ring.wp as u32 + 1) % BD_NUM) as u16;
    host::mmio_w16(mmio, regs::R_AX_CH8_TXBD_IDX, ring.wp);

    true
}

// ── Frame builders ────────────────────────────────────────────────

/// Build an 802.11 Open-System Authentication Request (IEEE 802.11-2020 §9.3.3.11).
/// Unicast to `bssid`. The FW/HW fills the Sequence-Control field via
/// HW_SSN_SEL in the TXWD body, so we leave it at zero here.
///
///   MAC hdr (24 B): FC=0xB0 0x00 (Mgmt subtype=11 AUTH) | Dur=0
///                   DA=bssid | SA=sma | BSSID=bssid | SeqCtrl=0
///   Body (6 B):     Auth Alg=0 (Open) | Auth Seq=1 | Status=0
///
/// Returns 30.
pub fn build_auth_open(sa: &[u8; 6], bssid: &[u8; 6], buf: &mut [u8]) -> usize {
    buf[0] = 0xB0; buf[1] = 0x00;      // FC: Mgmt, subtype=11 (AUTH)
    buf[2] = 0x00; buf[3] = 0x00;      // Duration
    for i in 0..6 { buf[4  + i] = bssid[i]; }   // DA = AP
    for i in 0..6 { buf[10 + i] = sa[i];    }   // SA = us
    for i in 0..6 { buf[16 + i] = bssid[i]; }   // BSSID = AP
    buf[22] = 0x00; buf[23] = 0x00;    // SeqCtrl (HW fills)

    // Authentication body
    buf[24] = 0x00; buf[25] = 0x00;    // Auth Alg = 0 (Open System)
    buf[26] = 0x01; buf[27] = 0x00;    // Auth Seq = 1 (request)
    buf[28] = 0x00; buf[29] = 0x00;    // Status Code = 0

    30
}

/// Build a wildcard Probe Request (no SSID, broadcast DA/BSSID).
/// Returns the byte length (always 45 for this variant).
pub fn build_probe_req(sa: &[u8; 6], channel: u8, buf: &mut [u8]) -> usize {
    // 24-byte MAC header (IEEE 802.11-2020 §9.3.3.10)
    buf[0] = 0x40; buf[1] = 0x00;              // FC: Type=Mgmt, Subtype=ProbeReq
    buf[2] = 0x00; buf[3] = 0x00;              // Duration
    for i in 0..6 { buf[4 + i]  = 0xFF; }      // DA = broadcast
    for i in 0..6 { buf[10 + i] = sa[i]; }     // SA = our MAC
    for i in 0..6 { buf[16 + i] = 0xFF; }      // BSSID = wildcard
    buf[22] = 0x00; buf[23] = 0x00;            // SeqCtrl (HW fills)

    let mut o = 24;

    // IE SSID (0): zero-length = wildcard
    buf[o] = 0x00; buf[o + 1] = 0x00;
    o += 2;

    // IE Supported Rates (1): 8 basic rates (bit7 = basic)
    buf[o] = 0x01; buf[o + 1] = 0x08;
    buf[o + 2] = 0x82; buf[o + 3] = 0x84;      // 1, 2 Mbps
    buf[o + 4] = 0x8B; buf[o + 5] = 0x96;      // 5.5, 11 Mbps
    buf[o + 6] = 0x0C; buf[o + 7] = 0x12;      // 6, 9 Mbps
    buf[o + 8] = 0x18; buf[o + 9] = 0x24;      // 12, 18 Mbps
    o += 10;

    // IE Extended Supported Rates (50): 24, 36, 48, 54
    buf[o] = 0x32; buf[o + 1] = 0x04;
    buf[o + 2] = 0x30; buf[o + 3] = 0x48;
    buf[o + 4] = 0x60; buf[o + 5] = 0x6C;
    o += 6;

    // IE DS Param Set (3): current channel
    buf[o] = 0x03; buf[o + 1] = 0x01;
    buf[o + 2] = channel;
    o += 3;

    o  // 45 bytes
}
