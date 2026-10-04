# `tools/wasm/wifi/src/tx.rs` @ 5e0102684

## L1-13 · `use crate::host;`

```
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
```

## L18 · `const BD_NUM:        u32 = 32;   // 32 BDs × 8 B = 256 B (fits in 1 page)`

```
// ── Ring dimensions (smaller than Linux for Phase 1; easy to grow) ──
```

## L20 · `const BD_NUM:        u32 = 32;   // 32 BDs × 8 B = 256 B (fits in 1 page)`

```
// 32 BDs × 8 B = 256 B (fits in 1 page)
```

## L21 · `const WD_PAGES:      u32 = 16;   // 16 WD pages`

```
// 16 WD pages
```

## L22 · `const WD_PAGE_SIZE:  u32 = 128;  // Linux constant`

```
// Linux constant
```

## L23 · `const FRAME_SLOT:    u32 = 256;  // 256 B per frame (AUTH/ASSOC fit)`

```
// 256 B per frame (AUTH/ASSOC fit)
```

## L25 · `const OFF_BODY:      u32 = 0;`

```
// WD offsets
```

## L32 · `const BDRAM_SIDX:    u32 = 20;`

```
// BDRAM config for CH8 (single-band): start=20 max=4 min=1 (pci.c:1716).
```

## L37 · `const QSEL_B0_MGMT:  u32 = 0x12;`

```
// QSEL for MGMT Band 0 (txrx.h RTW89_TX_QSEL_B0_MGMT).
```

## L40 · `const HW_SSN_SEL:    u32 = 1;`

```
// HW sequence mode values for mgmt (core.c RTW89_MGMT_HW_SSN_SEL / _MODE).
```

## L44 · `const RATE_CCK1:     u32 = 0x0;`

```
// Rate (txrx.h): 0x0 = CCK1 (1 Mbps DSSS). 2.4G mgmt default.
```

## L54 · `pub wp:           u16,    // host write pointer (BD index, 0..BD_NUM-1)`

```
// host write pointer (BD index, 0..BD_NUM-1)
```

## L55 · `pub next_slot:    u32,    // round-robin allocator for WD/frame slots`

```
// round-robin allocator for WD/frame slots
```

## L59 · `let bd_handle = host::dma_alloc(1);`

```
// BD ring: 256 B needs 1 page
```

## L64 · `let wd_handle = host::dma_alloc(1);`

```
// WD page pool: 16 × 128 = 2 KB, fits in 1 page
```

## L69 · `let frame_handle = host::dma_alloc(1);`

```
// Frame pool: 16 slots × 256 B = 4 KB
```

## L74 · `for off in (0..BD_NUM * 8).step_by(4) {`

```
// Zero BD ring
```

## L78 · `for off in (0..WD_PAGES * WD_PAGE_SIZE).step_by(4) {`

```
// Zero WD pool
```

## L92-103 · `pub fn init_ch8(mmio: i32, ring: &TxRing) {`

```
/// Program the CH8 TXBD ring into HW. Call once after alloc(), before any
/// send. Mirrors rtw89_pci_reset_trx_rings for CH8 (pci.c:1776) plus the
/// Linux disable→configure→enable sequence (pci.c:3105/3130).
///
/// Why the disable wrap: mac_init already enabled ALL TX DMA channels via
/// mac::pcie_post_init (clearing STOP bits in R_AX_PCIE_DMA_STOP1) long
/// before we configure the CH8 ring. A DMA channel that is enabled while
/// its DESA/NUM/BDRAM registers are garbage sits in a partial-error
/// state; later configuring the ring doesn't necessarily clear that
/// state. Linux avoids the whole problem by doing
/// `ctrl_dma_all(false) → reset_trx_rings() → ctrl_dma_all(true)`
/// atomically. We mimic that for CH8 specifically.
```

## L105 · `host::mmio_set32(mmio, regs::R_AX_PCIE_DMA_STOP1, regs::B_AX_STOP_CH8);`

```
// 1. STOP CH8 DMA while we rewrite the descriptors.
```

## L108 · `host::mmio_w16(mmio, regs::R_AX_CH8_TXBD_NUM, BD_NUM as u16);`

```
// 2. Ring size (16-bit write, like Linux order: NUM before DESA).
```

## L111 · `let bdram = (BDRAM_SIDX & 0xFF)`

```
// 3. BDRAM config: start_idx[7:0] | max_num[15:8] | min_num[23:16]
```

## L117 · `host::mmio_w32(mmio, regs::R_AX_CH8_TXBD_DESA_L, ring.bd_phys as u32);`

```
// 4. DMA base address (low + high, matching Linux order).
```

## L121 · `host::mmio_w32(mmio, regs::R_AX_TXBD_RWPTR_CLR1, regs::B_AX_CLR_CH8_IDX);`

```
// 5. Reset wp/rp pointers AFTER DESA/NUM/BDRAM are in place.
```

## L126 · `host::mmio_clr32(mmio, regs::R_AX_PCIE_DMA_STOP1, regs::B_AX_STOP_CH8);`

```
// 6. Re-enable CH8 DMA with valid ring state.
```

## L131-136 · `pub fn send_mgmt(mmio: i32, ring: &mut TxRing, frame: &[u8]) -> bool {`

```
/// Send a management frame (Probe Request, AUTH, ASSOC) via CH8.
/// `frame` is the raw 802.11 frame (MAC header + body), unencrypted.
/// Returns true on enqueue success (does NOT wait for TX completion).
/// The multicast/broadcast decision is taken from bit 0 of addr1 (DA) —
/// Linux core.c:1569 `rts_en = !is_bmc`: unicast frames do RTS, group
/// frames skip RTS.
```

## L139-140 · `let is_bmc = frame.len() >= 5 && (frame[4] & 0x01) != 0;`

```
// addr1 (DA) is at offset 4..10. A group address has bit 0 of the
// first octet set.
```

## L143 · `let slot = ring.next_slot;`

```
// Round-robin WD page + frame slot (paired index).
```

## L152 · `host::dma_write_buf(ring.frame_handle, frame_off, frame);`

```
// ── 1. Copy frame into DMA ─────────────────────────────────────
```

## L155-157 · `let dw0: u32 = (1u32 << 22)                    // WD_INFO_EN`

```
// ── 2. TXWD Body (AX: dw0/2/3 only; dw1/4/5 stay zero) ─────────
// dw0: WP_OFFSET=0 | WD_INFO_EN=1 | CHANNEL_DMA=8 | WD_PAGE=1
//      | HW_SSN_SEL=1 | HW_SSN_MODE=1
```

## L158 · `let dw0: u32 = (1u32 << 22)                    // WD_INFO_EN`

```
// WD_INFO_EN
```

## L159 · `| (8u32  << 16)                   // CHANNEL_DMA = CH8`

```
// CHANNEL_DMA = CH8
```

## L160 · `| (1u32  << 7)                    // WD_PAGE`

```
// WD_PAGE
```

## L161 · `| (HW_SSN_SEL  << 2)              // HW_SSN_SEL`

```
// HW_SSN_SEL
```

## L162 · `| HW_SSN_MODE;                    // HW_SSN_MODE`

```
// HW_SSN_MODE
```

## L164 · `host::dma_w32(ring.wd_handle, wd_off + OFF_BODY + 4, 0);`

```
// dw1 = 0 (AX doesn't use body1 fields in fill_txdesc)
```

## L166 · `let dw2: u32 = (0u32 << 24)                    // MACID`

```
// dw2: MACID=0 | QSEL=0x12 | TXPKT_SIZE=frame.len()
```

## L167 · `let dw2: u32 = (0u32 << 24)                    // MACID`

```
// MACID
```

## L168 · `| (QSEL_B0_MGMT << 17)            // QSEL`

```
// QSEL
```

## L169 · `| (frame.len() as u32 & 0x3FFF);  // TXPKT_SIZE`

```
// TXPKT_SIZE
```

## L171 · `host::dma_w32(ring.wd_handle, wd_off + OFF_BODY + 12, 0);`

```
// dw3..dw5 = 0 (seq=0, HW fills via HW_SSN_SEL)
```

## L176-177 · `let info0: u32 = (1u32 << 30)                  // USE_RATE`

```
// ── 3. TXWD Info (only when en_wd_info=1) ──────────────────────
// dw0: USE_RATE=1 | DATA_BW=0 | GI_LTF=0 | DATA_RATE=CCK1 | DISDATAFB=1
```

## L178 · `let info0: u32 = (1u32 << 30)                  // USE_RATE`

```
// USE_RATE
```

## L179 · `| (RATE_CCK1 << 16)             // DATA_RATE`

```
// DATA_RATE
```

## L180 · `| (1u32 << 10);                 // DISDATAFB`

```
// DISDATAFB
```

## L182 · `host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 4,  0);      // info1`

```
// info1
```

## L183 · `host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 8,  0);      // info2 (no SEC)`

```
// info2 (no SEC)
```

## L184 · `host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 12, 0);      // info3 (no rpt)`

```
// info3 (no rpt)
```

## L185 · `let info4: u32 = (1u32 << 31) | if is_bmc { 0 } else { 1u32 << 27 };`

```
// info4: HW_RTS_EN=BIT(31) always, RTS_EN=BIT(27) = !is_bmc.
```

## L188 · `host::dma_w32(ring.wd_handle, wd_off + OFF_INFO + 20, 0);      // info5`

```
// info5
```

## L190 · `let seq0: u32 = (slot & 0xFFFF) | (1u32 << 15);   // RTW89_PCI_TXWP_VALID`

```
// ── 4. TXWP Info (seq0 = slot | VALID; rest zero) ──────────────
```

## L191 · `let seq0: u32 = (slot & 0xFFFF) | (1u32 << 15);   // RTW89_PCI_TXWP_VALID`

```
// RTW89_PCI_TXWP_VALID
```

## L195-196 · `let frame_dma_hi = (frame_phys >> 32) as u32 & 0xFF;`

```
// ── 5. Addr Info (1 entry, points at frame DMA) ───────────────
// length[15:0] | option[31:16] (MSDU_LS | NUM(1) | DMA_HI<<6)
```

## L198 · `let option: u32 = (1u32 << 15)                  // MSDU_LS`

```
// MSDU_LS
```

## L199 · `| 1                             // NUM(1)`

```
// NUM(1)
```

## L200 · `| (frame_dma_hi << 6);          // DMA_HI`

```
// DMA_HI
```

## L207-214 · `let bd_off        = ring.wp as u32 * 8;`

```
// ── 6. TXBD (points at WD page) ───────────────────────────────
// Linux pci.c:1539: `txwd->len = txwd_len + txwp_len + txaddr_info_len`
// — 24 (body) + 24 (info) + 8 (wp) + 8 (one addr entry) = 64 bytes.
// The 802.11 frame lives in a SEPARATE DMA buffer referenced by the
// addr_info entry, so its length is NOT added here. Adding it caused
// HW to read 45 bytes of zero-padded slop past the real metadata and
// silently drop the frame — visible as TX_COUNTER=0 despite CH8_BUSY
// toggling and TXBD_IDX advancing (v1.30/v1.31 diagnostic).
```

## L218 · `let bd_opt: u32   = (1u32 << 14) | (wd_dma_hi << 6);  // LS + DMA_HI`

```
// BD word0: length[15:0] | opt[31:16] (LS | DMA_HI<<6)
```

## L219 · `let bd_opt: u32   = (1u32 << 14) | (wd_dma_hi << 6);  // LS + DMA_HI`

```
// LS + DMA_HI
```

## L226 · `ring.wp = ((ring.wp as u32 + 1) % BD_NUM) as u16;`

```
// ── 7. Kick-off: advance wp and write it to HW ────────────────
```

## L233 · `pub fn build_auth_open(sa: &[u8; 6], bssid: &[u8; 6], buf: &mut [u8]) -> usize {`

```
// ── Frame builders ────────────────────────────────────────────────
```

## L235-243 · `pub fn build_auth_open(sa: &[u8; 6], bssid: &[u8; 6], buf: &mut [u8]) -> usize {`

```
/// Build an 802.11 Open-System Authentication Request (IEEE 802.11-2020 §9.3.3.11).
/// Unicast to `bssid`. The FW/HW fills the Sequence-Control field via
/// HW_SSN_SEL in the TXWD body, so we leave it at zero here.
///
///   MAC hdr (24 B): FC=0xB0 0x00 (Mgmt subtype=11 AUTH) | Dur=0
///                   DA=bssid | SA=sma | BSSID=bssid | SeqCtrl=0
///   Body (6 B):     Auth Alg=0 (Open) | Auth Seq=1 | Status=0
///
/// Returns 30.
```

## L245 · `buf[0] = 0xB0; buf[1] = 0x00;      // FC: Mgmt, subtype=11 (AUTH)`

```
// FC: Mgmt, subtype=11 (AUTH)
```

## L246 · `buf[2] = 0x00; buf[3] = 0x00;      // Duration`

```
// Duration
```

## L247 · `for i in 0..6 { buf[4  + i] = bssid[i]; }   // DA = AP`

```
// DA = AP
```

## L248 · `for i in 0..6 { buf[10 + i] = sa[i];    }   // SA = us`

```
// SA = us
```

## L249 · `for i in 0..6 { buf[16 + i] = bssid[i]; }   // BSSID = AP`

```
// BSSID = AP
```

## L250 · `buf[22] = 0x00; buf[23] = 0x00;    // SeqCtrl (HW fills)`

```
// SeqCtrl (HW fills)
```

## L252 · `buf[24] = 0x00; buf[25] = 0x00;    // Auth Alg = 0 (Open System)`

```
// Authentication body
```

## L253 · `buf[24] = 0x00; buf[25] = 0x00;    // Auth Alg = 0 (Open System)`

```
// Auth Alg = 0 (Open System)
```

## L254 · `buf[26] = 0x01; buf[27] = 0x00;    // Auth Seq = 1 (request)`

```
// Auth Seq = 1 (request)
```

## L255 · `buf[28] = 0x00; buf[29] = 0x00;    // Status Code = 0`

```
// Status Code = 0
```

## L260-261 · `pub fn build_probe_req(sa: &[u8; 6], channel: u8, buf: &mut [u8]) -> usize {`

```
/// Build a wildcard Probe Request (no SSID, broadcast DA/BSSID).
/// Returns the byte length (always 45 for this variant).
```

## L263 · `buf[0] = 0x40; buf[1] = 0x00;              // FC: Type=Mgmt, Subtype=ProbeReq`

```
// 24-byte MAC header (IEEE 802.11-2020 §9.3.3.10)
```

## L264 · `buf[0] = 0x40; buf[1] = 0x00;              // FC: Type=Mgmt, Subtype=ProbeReq`

```
// FC: Type=Mgmt, Subtype=ProbeReq
```

## L265 · `buf[2] = 0x00; buf[3] = 0x00;              // Duration`

```
// Duration
```

## L266 · `for i in 0..6 { buf[4 + i]  = 0xFF; }      // DA = broadcast`

```
// DA = broadcast
```

## L267 · `for i in 0..6 { buf[10 + i] = sa[i]; }     // SA = our MAC`

```
// SA = our MAC
```

## L268 · `for i in 0..6 { buf[16 + i] = 0xFF; }      // BSSID = wildcard`

```
// BSSID = wildcard
```

## L269 · `buf[22] = 0x00; buf[23] = 0x00;            // SeqCtrl (HW fills)`

```
// SeqCtrl (HW fills)
```

## L273 · `buf[o] = 0x00; buf[o + 1] = 0x00;`

```
// IE SSID (0): zero-length = wildcard
```

## L277 · `buf[o] = 0x01; buf[o + 1] = 0x08;`

```
// IE Supported Rates (1): 8 basic rates (bit7 = basic)
```

## L279 · `buf[o + 2] = 0x82; buf[o + 3] = 0x84;      // 1, 2 Mbps`

```
// 1, 2 Mbps
```

## L280 · `buf[o + 4] = 0x8B; buf[o + 5] = 0x96;      // 5.5, 11 Mbps`

```
// 5.5, 11 Mbps
```

## L281 · `buf[o + 6] = 0x0C; buf[o + 7] = 0x12;      // 6, 9 Mbps`

```
// 6, 9 Mbps
```

## L282 · `buf[o + 8] = 0x18; buf[o + 9] = 0x24;      // 12, 18 Mbps`

```
// 12, 18 Mbps
```

## L285 · `buf[o] = 0x32; buf[o + 1] = 0x04;`

```
// IE Extended Supported Rates (50): 24, 36, 48, 54
```

## L291 · `buf[o] = 0x03; buf[o + 1] = 0x01;`

```
// IE DS Param Set (3): current channel
```

## L296 · `o  // 45 bytes`

```
// 45 bytes
```

