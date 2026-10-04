//! `pci.c` from Linux 6.18.26 rtw88: TX/RX rings, reserved page and H2C
//! queues, interrupts, RX/TX paths and PCIe link configuration.
//!
//! Ring setup: `rtw_pci_init_tx_ring` · `rtw_pci_init_rx_ring` ·
//! `rtw_pci_reset_rx_desc` · `rtw_pci_init_trx_ring` ·
//! `rtw_pci_reset_buf_desc` · `rtw_pci_reset_trx_ring` ·
//! `rtw_pci_dma_reset` · `rtw_pci_setup`.
//!
//! `rtw_hci_setup` is `rtw_pci_setup`, which is two calls: without
//! `rtw_pci_dma_reset` the TRX DMA interface never starts and the first
//! reserved page is never fetched. `setup()` is one function so the second
//! half cannot be forgotten.
//!
//! Two deliberate deviations:
//!
//! 1. Only the MPDU RX ring. Linux allocates `RTK_MAX_RX_QUEUE_NUM = 2`
//!    (MPDU and C2H), but `rtw_pci_reset_buf_desc` writes only
//!    `RXBD_DESA_MPDUQ` to the hardware; PCIe has no address register for
//!    C2H. Firmware messages arrive through the MPDU ring and are separated
//!    by `pkt_stat.is_c2h`.
//! 2. RX buffers come from a few large chunks instead of 512 separate
//!    allocations. The hardware sees one physical address per descriptor
//!    either way; `npk_dma_alloc` returns contiguous pages below 4 GB.

// `idx`/`handle` belong to `struct rtw_pci_ring` and are used for kick-off
// and refill.
#![allow(dead_code)]

use crate::host;

// ── pci.h:10-15 ──────────────────────────────────────────────────
pub const RTK_DEFAULT_TX_DESC_NUM: u32 = 128;
pub const RTK_BEQ_TX_DESC_NUM: u32 = 256;
pub const RTK_MAX_RX_DESC_NUM: u32 = 512;
pub const RTK_PCI_RX_BUF_SIZE: u32 = 11454 + 24;

// rtw8822c.c `rtw8822c_hw_spec`
const TX_BUF_DESC_SZ: u32 = 16;
const RX_BUF_DESC_SZ: u32 = 8;

/// Page granularity of our DMA allocation. The buffer is 11478 bytes; the
/// stride is rounded up to the next page so every address is page-aligned.
const RX_BUF_STRIDE: u32 = 12288; // ceil(11478 / 4096) * 4096
const PAGE: u32 = 4096;

/// main.h:202-215: the order is a contract, `RTW_TX_QUEUE_BK` is 0.
pub const Q_BK: usize = 0;
pub const Q_BE: usize = 1;
pub const Q_VI: usize = 2;
pub const Q_VO: usize = 3;
pub const Q_BCN: usize = 4;
pub const Q_MGMT: usize = 5;
pub const Q_HI0: usize = 6;
pub const Q_H2C: usize = 7;
pub const N_TX_QUEUES: usize = 8;

/// pci.h `max_num_of_tx_queue`
fn max_num_of_tx_queue(q: usize) -> u32 {
    match q {
        Q_BE => RTK_BEQ_TX_DESC_NUM,
        Q_BCN => 1,
        _ => RTK_DEFAULT_TX_DESC_NUM,
    }
}

/// The address, count and index registers per queue (pci.h:44-73).
/// `num` is `None` for BCNQ; pci.h says why: "BCNQ is specialized for rsvd
/// page, does not need to specify a number".
struct QueueRegs {
    desa: u32,
    num: Option<u32>,
    idx: Option<u32>,
    name: &'static str,
}

const TXQ: [QueueRegs; N_TX_QUEUES] = [
    QueueRegs { desa: 0x330, num: Some(0x38A), idx: Some(0x3AC), name: "BK  " },
    QueueRegs { desa: 0x328, num: Some(0x388), idx: Some(0x3A8), name: "BE  " },
    QueueRegs { desa: 0x320, num: Some(0x386), idx: Some(0x3A4), name: "VI  " },
    QueueRegs { desa: 0x318, num: Some(0x384), idx: Some(0x3A0), name: "VO  " },
    QueueRegs { desa: 0x308, num: None, idx: None, name: "BCN " },
    QueueRegs { desa: 0x310, num: Some(0x380), idx: Some(0x3B0), name: "MGMT" },
    QueueRegs { desa: 0x340, num: Some(0x38C), idx: Some(0x3B8), name: "HI0 " },
    QueueRegs { desa: 0x1320, num: Some(0x1328), idx: Some(0x132C), name: "H2C " },
];

pub const RTK_PCI_RXBD_DESA_MPDUQ: u32 = 0x338;
pub const RTK_PCI_RXBD_NUM_MPDUQ: u32 = 0x382;
pub const RTK_PCI_RXBD_IDX_MPDUQ: u32 = 0x3B4;
pub const RTK_PCI_TXBD_RWPTR_CLR: u32 = 0x39C;
pub const RTK_PCI_TXBD_H2CQ_CSR: u32 = 0x1330;
pub const BIT_CLR_H2CQ_HOST_IDX: u32 = 1 << 16;
pub const BIT_CLR_H2CQ_HW_IDX: u32 = 1 << 8;
pub const RTK_PCI_CTRL: u32 = 0x300;
pub const BIT_RST_TRXDMA_INTF: u32 = 1 << 20; // pci.h:18
pub const BIT_RX_TAG_EN: u32 = 1 << 15; // pci.h:19
pub const TRX_BD_IDX_MASK: u32 = 0xFFF; // GENMASK(11, 0)
pub const TRX_BD_HW_IDX_MASK: u32 = 0x0FFF_0000; // GENMASK(27, 16)

pub struct TxRing {
    pub dma: u32,
    pub len: u32,
    pub handle: i32,
    pub wp: u32,
    pub rp: u32,
}

pub struct RxRing {
    /// Descriptor ring (512 x 8 bytes)
    pub dma: u32,
    pub len: u32,
    pub handle: i32,
    pub wp: u32,
    pub rp: u32,
    /// Up to two contiguous chunks the buffers come from.
    chunk_phys: [u32; 2],
    /// And their DMA handles: the chip only needs the physical address, but
    /// the driver needs the handle to read the buffers.
    chunk_handle: [i32; 2],
    per_chunk: u32,
}

impl RxRing {
    /// Physical address of the i-th RX buffer, as given to the chip.
    pub fn buf_phys(&self, i: u32) -> u32 {
        let c = (i / self.per_chunk) as usize;
        self.chunk_phys[c] + (i % self.per_chunk) * RX_BUF_STRIDE
    }

    /// Handle and offset of the same buffer, used to read it.
    pub fn buf_loc(&self, i: u32) -> (i32, u32) {
        let c = (i / self.per_chunk) as usize;
        (self.chunk_handle[c], (i % self.per_chunk) * RX_BUF_STRIDE)
    }
}

pub struct Trx {
    pub tx: [TxRing; N_TX_QUEUES],
    pub rx: RxRing,
    pub dma_pages: u32,
    pub dma_allocs: u32,
    /// `rtwpci->rx_tag`: set to 0 by `rtw_pci_dma_reset`, read by
    /// `rtw_pci_dma_check`.
    pub rx_tag: u16,
}

const EMPTY_TX: TxRing = TxRing { dma: 0, len: 0, handle: -1, wp: 0, rp: 0 };

/// Upper limit for every DMA chunk of this driver.
///
/// The DESA registers are 32 bits wide, so everything must be below 4 GB.
/// Additionally, `allocate_contiguous_below` searches from the top and
/// with a 4 GB limit lands just below the PCI MMIO hole; on some AMD
/// platforms (TSEG/DPR region) the chip's DMA there ends in a received
/// master abort while the CPU accesses the same memory fine. 1 GB stays
/// well clear of that.
const DMA_LIMIT_MB: u32 = 1024;

fn alloc_pages(pages: u32) -> Option<(i32, u32)> {
    let h = host::dma_alloc_below(pages as u16, DMA_LIMIT_MB);
    if h < 0 {
        return None;
    }
    let phys = host::dma_phys(h);
    // The TX/RX descriptor has a 32-bit address field. The kernel allocates
    // below 4 GB, but it is checked here rather than assumed.
    if phys == 0 || phys >= 0x1_0000_0000 {
        return None;
    }
    Some((h, phys as u32))
}

/// pci.c `rtw_pci_init_trx_ring`
pub fn init_trx_ring() -> Option<Trx> {
    let mut tx = [EMPTY_TX; N_TX_QUEUES];
    let mut pages = 0u32;
    let mut allocs = 0u32;

    // pci.c `rtw_pci_init_tx_ring`
    for q in 0..N_TX_QUEUES {
        let len = max_num_of_tx_queue(q);
        if len > TRX_BD_IDX_MASK {
            return None; // Linux: "len %d exceeds maximum TX entries"
        }
        let ring_sz = TX_BUF_DESC_SZ * len;
        let np = ring_sz.div_ceil(PAGE).max(1);
        let (h, phys) = alloc_pages(np)?;
        pages += np;
        allocs += 1;
        tx[q] = TxRing { dma: phys, len, handle: h, wp: 0, rp: 0 };
    }

    // pci.c `rtw_pci_init_rx_ring`, MPDU
    let desc_sz = RX_BUF_DESC_SZ * RTK_MAX_RX_DESC_NUM;
    let (desc_h, desc_phys) = alloc_pages(desc_sz.div_ceil(PAGE))?;
    pages += desc_sz.div_ceil(PAGE);
    allocs += 1;

    // The buffers in two chunks: 512 x 12288 = 6 MiB = 1536 pages, and
    // MAX_DMA_PAGES_PER_CALL is 1024, so one chunk is not enough.
    let per_chunk = RTK_MAX_RX_DESC_NUM / 2;
    let chunk_pages = per_chunk * RX_BUF_STRIDE / PAGE;
    let mut chunk_phys = [0u32; 2];
    let mut chunk_handle = [-1i32; 2];
    for c in 0..2 {
        let (hh, phys) = alloc_pages(chunk_pages)?;
        pages += chunk_pages;
        allocs += 1;
        chunk_phys[c] = phys;
        chunk_handle[c] = hh;
    }

    let rx = RxRing {
        dma: desc_phys,
        len: RTK_MAX_RX_DESC_NUM,
        handle: desc_h,
        wp: 0,
        rp: 0,
        chunk_phys,
        chunk_handle,
        per_chunk,
    };

    // pci.c `rtw_pci_reset_rx_desc` for each entry: buf_size + dma.
    // `struct rtw_pci_rx_buffer_desc` (pci.h:193) is
    // { __le16 buf_size; __le16 total_pkt_size; __le32 dma; } = 8 bytes.
    for i in 0..RTK_MAX_RX_DESC_NUM {
        let off = i * RX_BUF_DESC_SZ;
        host::dma_w32(desc_h, off, RTK_PCI_RX_BUF_SIZE & 0xFFFF);
        host::dma_w32(desc_h, off + 4, rx.buf_phys(i));
    }

    Some(Trx { tx, rx, dma_pages: pages, dma_allocs: allocs, rx_tag: 0 })
}

/// pci.c `rtw_pci_dma_reset`: "reset dma and rx tag".
///
/// Without this the TRX DMA interface does not start: the ring addresses
/// are in the registers (and read back correctly), but the chip never
/// fetches the descriptors.
fn dma_reset(h: i32, trx: &mut Trx) {
    host::set32(h, RTK_PCI_CTRL, BIT_RST_TRXDMA_INTF | BIT_RX_TAG_EN);
    trx.rx_tag = 0;
}

/// pci.c `rtw_pci_setup`, what `rtw_hci_setup` means for PCIe. Both halves,
/// always together.
pub fn setup(h: i32, trx: &mut Trx, verbose: bool) {
    reset_buf_desc(h, trx, verbose); // = rtw_pci_reset_trx_ring
    dma_reset(h, trx);
}

/// Clear the upper half of a DESA register.
///
/// The DESA registers are eight bytes apart (0x308, 0x310, 0x318, …):
/// each is a 64-bit address register, and `rtw_pci_reset_buf_desc` writes
/// only the low 32 bits with `rtw_write32`. Linux gets away with that
/// because `pci_enable_device` runs on a freshly reset device whose upper
/// halves are zero; `rtw_pci_claim` sets no DMA mask, so the default is 32
/// bits.
///
/// This driver does not reset the PCIe device. A leftover upper half makes
/// the chip build a 64-bit address nobody answers (received master abort,
/// OWN bit untouched). So the upper half is cleared explicitly, before the
/// lower half, so the address is complete when the chip latches it.
fn desa_hi_clear(h: i32, desa: u32, name: &str, verbose: bool) {
    let hi = host::r32(h, desa + 4);
    if verbose {
        host::print("    DESA-hi ");
        host::print(name);
        host::print(" @0x");
        host::print_hex16((desa + 4) as u16);
        host::print(" = 0x");
        host::print_hex32(hi);
        host::print(if hi != 0 { "  <- NICHT null\n" } else { "\n" });
    }
    host::w32(h, desa + 4, 0);
}

/// pci.c `rtw_pci_reset_buf_desc`
pub fn reset_buf_desc(h: i32, trx: &mut Trx, verbose: bool) {
    let tmp = host::r8(h, RTK_PCI_CTRL + 3);
    host::w8(h, RTK_PCI_CTRL + 3, tmp | 0xf7);

    if verbose {
        host::print("  [dump] obere Haelfte der DESA-Register vor dem Nullen:\n");
    }

    // BCNQ: address only, no count.
    desa_hi_clear(h, TXQ[Q_BCN].desa, TXQ[Q_BCN].name, verbose);
    host::w32(h, TXQ[Q_BCN].desa, trx.tx[Q_BCN].dma);

    // Order as in Linux: H2C, BK, BE, VO, VI, MGMT, HI0.
    for &q in &[Q_H2C, Q_BK, Q_BE, Q_VO, Q_VI, Q_MGMT, Q_HI0] {
        let r = &mut trx.tx[q];
        r.rp = 0;
        r.wp = 0;
        if let Some(num) = TXQ[q].num {
            host::w16(h, num, (r.len & TRX_BD_IDX_MASK) as u16);
        }
        desa_hi_clear(h, TXQ[q].desa, TXQ[q].name, verbose);
        host::w32(h, TXQ[q].desa, r.dma);
    }

    trx.rx.rp = 0;
    trx.rx.wp = 0;
    host::w16(h, RTK_PCI_RXBD_NUM_MPDUQ, (trx.rx.len & TRX_BD_IDX_MASK) as u16);
    desa_hi_clear(h, RTK_PCI_RXBD_DESA_MPDUQ, "MPDU", verbose);
    host::w32(h, RTK_PCI_RXBD_DESA_MPDUQ, trx.rx.dma);

    // reset read/write point
    host::w32(h, RTK_PCI_TXBD_RWPTR_CLR, 0xffff_ffff);

    // reset H2C Queue index in a single write (3081 only)
    host::set32(h, RTK_PCI_TXBD_H2CQ_CSR,
                BIT_CLR_H2CQ_HOST_IDX | BIT_CLR_H2CQ_HW_IDX);
}

/// Every address and count register must read back what was written. A
/// ring whose address the chip does not keep shows up here rather than
/// when the firmware is pushed through the BCN queue.
pub fn verify_rings(h: i32, trx: &Trx) -> bool {
    let mut ok = true;

    for q in 0..N_TX_QUEUES {
        let desa = host::r32(h, TXQ[q].desa);
        let mut line_ok = desa == trx.tx[q].dma;
        host::print(if line_ok { "  [ JA  ] " } else { "  [NEIN ] " });
        host::print(TXQ[q].name);
        host::print(" DESA 0x");
        host::print_hex32(desa);
        host::print("  (erwartet 0x");
        host::print_hex32(trx.tx[q].dma);
        host::print(")  NUM ");
        match TXQ[q].num {
            Some(reg) => {
                let v = host::r16(h, reg) as u32 & TRX_BD_IDX_MASK;
                let want = trx.tx[q].len & TRX_BD_IDX_MASK;
                line_ok &= v == want;
                host::print_dec(v);
                host::print(" (erwartet ");
                host::print_dec(want);
                host::print(")");
            }
            // pci.h:57: "BCNQ is specialized for rsvd page, does not need to specify
            // a number".
            None => host::print("— (BCNQ hat kein NUM-Register)"),
        }
        host::print("\n");
        ok &= line_ok;
    }

    let desa = host::r32(h, RTK_PCI_RXBD_DESA_MPDUQ);
    let num = host::r16(h, RTK_PCI_RXBD_NUM_MPDUQ) as u32 & TRX_BD_IDX_MASK;
    let line_ok = desa == trx.rx.dma && num == (trx.rx.len & TRX_BD_IDX_MASK);
    host::print(if line_ok { "  [ JA  ] " } else { "  [NEIN ] " });
    host::print("MPDU DESA 0x");
    host::print_hex32(desa);
    host::print("  NUM ");
    host::print_dec(num);
    host::print("  (erwartet 0x");
    host::print_hex32(trx.rx.dma);
    host::print(", ");
    host::print_dec(trx.rx.len);
    host::print(")\n");
    ok &= line_ok;

    ok
}

// ── A reserved page through the BCN queue ────────────────────────
// pci.h:162-163
pub const RTK_PCI_TXBD_OWN_OFFSET: u32 = 15;
pub const RTK_PCI_TXBD_BCN_WORK: u32 = 0x383;
pub const BIT_PCI_BCNQ_FLAG: u8 = 1 << 4; // pci.h:36

/// Largest chunk `download_firmware_to_mem` pushes at once
/// (mac.c: `max_size = 0x1000`), plus the descriptor in front.
pub const RSVD_STAGE_BYTES: u32 = 0x1000 + crate::tx::TX_PKT_DESC_SZ as u32;

/// pci.c `rtw_pci_write_data_rsvd_page` + `rtw_pci_tx_write_data` for
/// `RTW_TX_QUEUE_BCN`.
///
/// The BCN path is the special case: no `avail_desc`, no advancing of
/// `wp`, instead the OWN bit in `psb_len` and a kick via
/// `RTK_PCI_TXBD_BCN_WORK`. `rtw_pci_release_rsvd_page` frees the previous
/// skb in Linux; here the staging buffer is fixed and nothing is freed.
pub fn write_data_rsvd_page(
    h: i32, trx: &Trx, stage: i32, payload: &[u8], current_band_type: u8,
    verbose: bool,
) -> bool {
    // tx.c `rtw_tx_write_data_rsvd_page_get` builds an skb with 48 bytes of
    // headroom in Linux and then calls `rtw_tx_rsvd_page_pkt_info_update`.
    // Here the headroom is the fixed staging buffer.
    let desc_sz = crate::tx::TX_PKT_DESC_SZ;
    let mut info = crate::tx::rsvd_page_pkt_info_update(payload, current_band_type);
    // pci.c: `pkt_info->qsel = rtw_pci_get_tx_qsel(skb, queue)`, i.e. BEACON
    // for the BCN queue, and only then is the descriptor filled.
    info.qsel = crate::tx::TX_DESC_QSEL_BEACON;

    let mut desc = [0u8; crate::tx::TX_PKT_DESC_SZ];
    crate::tx::fill_tx_desc(&info, &mut desc);

    // Descriptor and payload are contiguous, like the skb in Linux after
    // `skb_push`: the second buffer descriptor points to dma + 48.
    host::dma_write_buf(stage, 0, &desc);
    host::dma_write_buf(stage, desc_sz as u32, payload);

    let dma = host::dma_phys(stage) as u32;
    let total = desc_sz + payload.len(); // = skb->len after the push
    let mut psb_len = ((total as u32 - 1) / 128) + 1;
    psb_len |= 1 << RTK_PCI_TXBD_OWN_OFFSET;

    // `get_tx_buffer_desc(ring, 16)` with wp = 0 -> offset 0.
    //
    // Two descriptors fit in one ring slot: `struct rtw_pci_tx_buffer_desc`
    // (pci.h:165) is { __le16 buf_size; __le16 psb_len; __le32 dma; } =
    // 8 bytes, while `tx_buf_desc_sz` is 16. The first points to the 48
    // descriptor bytes, the second to the payload behind them.
    let ring = trx.tx[Q_BCN].handle;
    // buf_desc[0] = { buf_size: 48, psb_len, dma }
    host::dma_w32(ring, 0, (desc_sz as u32 & 0xFFFF) | (psb_len << 16));
    host::dma_w32(ring, 4, dma);
    // buf_desc[1] = { buf_size: payload, psb_len: 0, dma + 48 }
    host::dma_w32(ring, 8, payload.len() as u32 & 0xFFFF);
    host::dma_w32(ring, 12, dma + desc_sz as u32);

    host::fence();

    if verbose {
        // Read back: are descriptor and payload really in the DMA buffer, and is
        // the ring entry as written?
        host::print("    stage @0x");
        host::print_hex32(dma);
        host::print("  desc[0..8] =");
        for i in 0..2 {
            host::print(" 0x");
            host::print_hex32(host::dma_r32(stage, i * 4));
        }
        host::print("\n    payload[0..8] =");
        for i in 0..2 {
            host::print(" 0x");
            host::print_hex32(host::dma_r32(stage, desc_sz as u32 + i * 4));
        }
        host::print("\n    erwartet      =");
        for i in 0..2 {
            let o = (i * 4) as usize;
            let w = u32::from_le_bytes([payload[o], payload[o + 1],
                                        payload[o + 2], payload[o + 3]]);
            host::print(" 0x");
            host::print_hex32(w);
        }
        host::print("\n    BCN-Ring[0..16] =");
        for i in 0..4 {
            host::print(" 0x");
            host::print_hex32(host::dma_r32(ring, i * 4));
        }
        host::print("\n    psb_len = 0x");
        host::print_hex32(psb_len);
        host::print("  total = ");
        host::print_dec(total as u32);
        host::print("\n");
    }

    // pci.c: "reserved pages go through beacon queue"
    let work = host::r8(h, RTK_PCI_TXBD_BCN_WORK);
    host::w8(h, RTK_PCI_TXBD_BCN_WORK, work | BIT_PCI_BCNQ_FLAG);
    if verbose {
        host::print("    BCN_WORK @0x383: 0x");
        host::print_hex8(work);
        host::print(" -> 0x");
        host::print_hex8(host::r8(h, RTK_PCI_TXBD_BCN_WORK));
        host::print("\n");
    }
    true
}

// ── rtw_hci_interface_cfg ────────────────────────────────────────

/// pci.c:1437-1450 `rtw_pci_interface_cfg`.
///
/// The only chip with a branch is the 8822C, from cut D on. It switches
/// the PCIe EMAC from the aux clock to the fast clock.
pub fn interface_cfg(h: i32, cut_version: u8) {
    if cut_version >= crate::regs::RTW_CHIP_VER_CUT_D {
        host::w32_mask(h, crate::regs::REG_HCI_MIX_CFG,
                       crate::regs::BIT_PCIE_EMAC_PDN_AUX_TO_FAST_CLK, 1);
    }
}

// ── H2C queue ────────────────────────────────────────────────────

/// pci.h:62-73: the H2C queue write pointer.
pub const RTK_PCI_TXBD_IDX_H2CQ: u32 = 0x132C; // pci.h:66

// The DBI path: the chip's PCIe configuration registers, reached via MMIO
// instead of the bus configuration space. Realtek places its own link
// switches there.
pub const REG_DBI_WDATA_V1: u32 = 0x03E8; // pci.h:20
pub const REG_DBI_RDATA_V1: u32 = 0x03EC; // pci.h:21
pub const REG_DBI_FLAG_V1: u32 = 0x03F0; // pci.h:22
pub const BIT_DBI_RFLAG: u32 = 1 << 17; // pci.h:23
pub const BIT_DBI_WFLAG: u32 = 1 << 16; // pci.h:24
pub const BITS_DBI_WREN: u32 = 0xF000; // pci.h:25 GENMASK(15,12)
pub const BITS_DBI_ADDR_MASK: u32 = 0x0FFC; // pci.h:26 GENMASK(11,2)
pub const RTW_PCI_WR_RETRY_CNT: u32 = 20; // pci.h:35
pub const RTK_PCIE_LINK_CFG: u16 = 0x0719; // pci.h:37
pub const BIT_CLKREQ_SW_EN: u8 = 1 << 4; // pci.h:38
pub const BIT_L1_SW_EN: u8 = 1 << 3; // pci.h:39
pub const RTK_PCIE_CLKDLY_CTRL: u16 = 0x0725; // pci.h:41

/// One slot per ring entry for the H2C staging buffer. Linux allocates an
/// skb per packet; its contents must not be overwritten until the chip has
/// fetched the descriptor. One slot per index gives the same guarantee
/// without an allocator.
pub const H2C_SLOT_BYTES: u32 = 128; // 48 descriptor + 32 payload, rounded up

/// The H2C staging buffer needs one slot per ring entry, not per sent
/// packet: `wp` runs over the whole ring length and then wraps.
pub const H2C_STAGE_BYTES: u32 = RTK_DEFAULT_TX_DESC_NUM * H2C_SLOT_BYTES;

/// pci.h:154-160 `avail_desc`
#[inline]
pub fn avail_desc(wp: u32, rp: u32, len: u32) -> u32 {
    if rp > wp { rp - wp - 1 } else { len - wp + rp - 1 }
}

/// pci.c:34-52 `rtw_pci_get_tx_qsel`, only the queues in use.
pub fn tx_qsel(queue: usize) -> u8 {
    match queue {
        Q_BCN => crate::tx::TX_DESC_QSEL_BEACON,
        Q_H2C => crate::tx::TX_DESC_QSEL_H2C,
        Q_MGMT => crate::tx::TX_DESC_QSEL_MGMT,
        Q_HI0 => crate::tx::TX_DESC_QSEL_HIGH,
        // Linux: `default: return skb->priority;`, the data queues carry the
        // packet's priority.
        _ => 0,
    }
}

/// pci.c:914-926 `rtw_pci_tx_kick_off_queue`.
///
/// The preceding `rtw_pci_deep_ps_leave` only applies without
/// `FW_FEATURE_TX_WAKE`; the firmware has that bit, and deep PS is not
/// implemented.
pub fn tx_kick_off_queue(h: i32, trx: &Trx, queue: usize) {
    let idx = match TXQ[queue].idx {
        Some(reg) => reg,
        None => return,
    };
    host::w16(h, idx, (trx.tx[queue].wp & TRX_BD_IDX_MASK) as u16);
}

/// pci.c:806-895 `rtw_pci_tx_write_data`, for a queue with a write pointer.
///
/// Unlike the reserved page path there is `avail_desc`, the ring slot
/// follows `wp`, the OWN bit is not set (that is the BCN special case), and
/// `wp` advances afterwards.
pub fn tx_write_data(_h: i32, trx: &mut Trx, stage: i32, queue: usize,
                     info: &mut crate::tx::TxPktInfo, payload: &[u8]) -> bool {
    let desc_sz = crate::tx::TX_PKT_DESC_SZ;
    let ring_len = trx.tx[queue].len;
    let wp = trx.tx[queue].wp;
    let rp = trx.tx[queue].rp;

    if avail_desc(wp, rp, ring_len) == 0 {
        host::print("[rtl8822ce] Sendering voll, Queue ");
        host::print_dec(queue as u32);
        host::print("\n");
        return false; // Linux: -ENOSPC
    }

    info.qsel = tx_qsel(queue);
    let mut desc = [0u8; crate::tx::TX_PKT_DESC_SZ];
    crate::tx::fill_tx_desc(info, &mut desc);

    // One slot per ring index, so a packet not yet fetched is not overwritten
    // under the chip. The H2C path has its own smaller stride; its payload is
    // always 32 bytes.
    let stride = if queue == Q_H2C { H2C_SLOT_BYTES } else { TX_SLOT_BYTES };
    let slot = wp * stride;
    // Both writes are checked: `npk_dma_write` rejects an offset beyond the
    // buffer with -1, and ignoring that would put an address the driver does
    // not own into the buffer descriptor, so the chip would transmit whatever
    // lies there.
    // The slot must also hold the frame (48 + 1540 at MTU 1500): an overflow
    // here would run into the neighbouring slot, invisible to the kernel.
    if desc_sz + payload.len() > stride as usize {
        host::loud_begin();
        host::print("[rtl8822ce] Rahmen passt nicht in einen Platz: ");
        host::print_dec((desc_sz + payload.len()) as u32);
        host::print(" > ");
        host::print_dec(stride);
        host::print("\n");
        host::loud_end();
        return false;
    }
    let a = host::dma_write_buf(stage, slot, &desc);
    let b = host::dma_write_buf(stage, slot + desc_sz as u32, payload);
    if a < 0 || b < 0 {
        host::loud_begin();
        host::print("[rtl8822ce] Zwischenpuffer passt nicht zum Ring: Queue ");
        host::print_dec(queue as u32);
        host::print(", Platz ");
        host::print_dec(wp);
        host::print(" von ");
        host::print_dec(ring_len);
        host::print(", Versatz ");
        host::print_dec(slot);
        host::print(" — NICHTS gesendet\n");
        host::loud_end();
        return false;
    }
    let dma = host::dma_phys(stage) as u32 + slot;

    let total = desc_sz + payload.len();
    let psb_len = ((total as u32 - 1) / 128) + 1;

    // `get_tx_buffer_desc(ring, tx_buf_desc_sz)`: the slot at `wp`.
    let ring = trx.tx[queue].handle;
    let off = wp * TX_BUF_DESC_SZ;
    host::dma_w32(ring, off, (desc_sz as u32 & 0xFFFF) | (psb_len << 16));
    host::dma_w32(ring, off + 4, dma);
    host::dma_w32(ring, off + 8, payload.len() as u32 & 0xFFFF);
    host::dma_w32(ring, off + 12, dma + desc_sz as u32);

    host::fence();

    trx.tx[queue].wp += 1;
    if trx.tx[queue].wp >= ring_len {
        trx.tx[queue].wp = 0;
    }
    true
}

/// pci.c:1206-1224 `rtw_pci_write_data_h2c`
pub fn write_data_h2c(h: i32, trx: &mut Trx, stage: i32, buf: &[u8]) -> bool {
    let mut info = crate::tx::write_data_h2c_get(buf.len() as u32);
    if !tx_write_data(h, trx, stage, Q_H2C, &mut info, buf) {
        host::print("[rtl8822ce] failed to write h2c data\n");
        return false;
    }
    tx_kick_off_queue(h, trx, Q_H2C);
    true
}

/// Wait until the firmware has drained the H2C queue.
///
/// No Linux counterpart: there the driver never reads back the read
/// pointer. A single sample right after the kick says nothing, so this
/// waits up to a deadline.
///
/// Returns whether it caught up and how long it took.
pub fn h2c_wait_consumed(h: i32, trx: &Trx, frist_us: u64) -> (bool, u64, u32) {
    let want = trx.tx[Q_H2C].wp & TRX_BD_IDX_MASK;
    let start = host::now_us();
    loop {
        let idx = host::r32(h, RTK_PCI_TXBD_IDX_H2CQ);
        let hw = (idx & TRX_BD_HW_IDX_MASK) >> 16;
        if hw == want {
            return (true, host::now_us() - start, hw);
        }
        let waited = host::now_us() - start;
        if waited >= frist_us {
            return (false, waited, hw);
        }
    }
}

// ════════════════════════════════════════════════════════════════
// RX path
// ════════════════════════════════════════════════════════════════

/// pci.h:204 `RX_TAG_MAX`
pub const RX_TAG_MAX: u16 = 8192;

// ── Interrupts: `pci.c` 374-387, 480-513, 1119-1141; `pci.h` 81-145 ──
//
// The 8822C is `RTW_WCPU_3081` (rtw8822c.c:5337), so HIMR3/HISR3 apply.
// Linux requests one MSI vector (`rtw_pci_request_irq`) and splits
// handling: the hard part masks HIMR, the thread acks HISR, works and
// unmasks HIMR. Here the kernel ISR only counts and wakes the driver
// fiber, where both halves run.
pub const RTK_PCI_HIMR0: u32 = 0x0B0;
pub const RTK_PCI_HISR0: u32 = 0x0B4;
pub const RTK_PCI_HIMR1: u32 = 0x0B8;
pub const RTK_PCI_HISR1: u32 = 0x0BC;
pub const RTK_PCI_HIMR3: u32 = 0x10B8;
pub const RTK_PCI_HISR3: u32 = 0x10BC;

const IMR_BCNDMAINT_E: u32 = 1 << 14;
const IMR_C2HCMD: u32 = 1 << 10;
const IMR_HIGHDOK: u32 = 1 << 7;
const IMR_MGNTDOK: u32 = 1 << 6;
const IMR_BKDOK: u32 = 1 << 5;
const IMR_BEDOK: u32 = 1 << 4;
const IMR_VIDOK: u32 = 1 << 3;
const IMR_VODOK: u32 = 1 << 2;
pub const IMR_ROK: u32 = 1 << 0;
const IMR_TXFOVW: u32 = 1 << 9; // HIMR1
const IMR_H2CDOK: u32 = 1 << 16; // HIMR3

/// `rtw_pci_setup`: `rtwpci->irq_mask[0..3]`.
pub const IRQ_MASK: [u32; 4] = [
    IMR_HIGHDOK | IMR_MGNTDOK | IMR_BKDOK | IMR_BEDOK | IMR_VIDOK | IMR_VODOK
        | IMR_ROK | IMR_BCNDMAINT_E | IMR_C2HCMD,
    IMR_TXFOVW,
    0,
    IMR_H2CDOK,
];

/// `rtw_pci_enable_interrupt`.
pub fn enable_interrupt(h: i32, exclude_rx: bool) {
    let imr0_unmask = if exclude_rx { IMR_ROK } else { 0 };
    host::w32(h, RTK_PCI_HIMR0, IRQ_MASK[0] & !imr0_unmask);
    host::w32(h, RTK_PCI_HIMR1, IRQ_MASK[1]);
    host::w32(h, RTK_PCI_HIMR3, IRQ_MASK[3]);
}

/// `rtw_pci_disable_interrupt`.
pub fn disable_interrupt(h: i32) {
    host::w32(h, RTK_PCI_HIMR0, 0);
    host::w32(h, RTK_PCI_HIMR1, 0);
    host::w32(h, RTK_PCI_HIMR3, 0);
}

/// `rtw_pci_irq_recognized`: read HISR, restrict to the mask and clear
/// exactly what was read (write-1-to-clear). If a bit stays set, the chip
/// produces no new MSI edge on the next event (comment in
/// `rtw_pci_interrupt_handler`).
pub fn irq_recognized(h: i32) -> [u32; 4] {
    let mut st = [
        host::r32(h, RTK_PCI_HISR0),
        host::r32(h, RTK_PCI_HISR1),
        0,
        host::r32(h, RTK_PCI_HISR3),
    ];
    for i in 0..4 {
        st[i] &= IRQ_MASK[i];
    }
    host::w32(h, RTK_PCI_HISR0, st[0]);
    host::w32(h, RTK_PCI_HISR1, st[1]);
    host::w32(h, RTK_PCI_HISR3, st[3]);
    st
}

/// pci.c:1023-1039 `rtw_pci_get_hw_rx_ring_nr`.
///
/// The chip writes its position into the upper twelve bits of the same
/// register we read ours from; the difference is the number of buffers it
/// has filled.
pub fn get_hw_rx_ring_nr(h: i32, trx: &Trx) -> u32 {
    let tmp = host::r32(h, RTK_PCI_RXBD_IDX_MPDUQ);
    let cur_wp = (tmp & TRX_BD_HW_IDX_MASK) >> 16;
    if cur_wp >= trx.rx.wp {
        cur_wp - trx.rx.wp
    } else {
        trx.rx.len - (trx.rx.wp - cur_wp)
    }
}

/// pci.c:683-702 `rtw_pci_dma_check`.
///
/// The chip writes a running tag into `total_pkt_size` of the buffer
/// descriptor; if it does not match ours, the bus lost something. Linux
/// warns and continues, as does this.
pub fn dma_check(trx: &mut Trx, idx: u32) -> bool {
    let off = idx * RX_BUF_DESC_SZ;
    // `struct rtw_pci_rx_buffer_desc` (pci.h:193):
    // { __le16 buf_size; __le16 total_pkt_size; __le32 dma; }
    let w0 = host::dma_r32(trx.rx.handle, off);
    let total_pkt_size = (w0 >> 16) as u16;
    let ok = total_pkt_size == trx.rx_tag;
    if !ok {
        host::print("[rtl8822ce] pci bus timeout, check dma status (rx_tag ");
        host::print_dec(trx.rx_tag as u32);
        host::print(", gelesen ");
        host::print_dec(total_pkt_size as u32);
        host::print(")\n");
    }
    trx.rx_tag = (trx.rx_tag + 1) % RX_TAG_MAX;
    ok
}

/// pci.c:235-250 `rtw_pci_sync_rx_desc_device`: hand the slot back so the
/// chip fills it again.
fn sync_rx_desc_device(trx: &Trx, idx: u32) {
    let off = idx * RX_BUF_DESC_SZ;
    // `memset(buf_desc, 0, sizeof(*buf_desc))`, then buf_size and dma.
    host::dma_w32(trx.rx.handle, off, RTK_PCI_RX_BUF_SIZE & 0xFFFF);
    host::dma_w32(trx.rx.handle, off + 4, trx.rx.buf_phys(idx));
}

/// pci.c:1041-1117 `rtw_pci_rx_napi`.
///
/// Linux passes each packet to `ieee80211_rx_napi`; here `deliver` is
/// called per packet. Everything in between (read the descriptor, check
/// `rx_tag`, release the slot, advance the pointer) is the same sequence.
///
/// Linux's skb swap is omitted: it allocates a new buffer and hands the
/// old one straight back to DMA so reception does not stall. Here the
/// contents are copied into linear memory, after which the buffer is free
/// again just the same.
#[allow(clippy::too_many_arguments)]
pub fn rx_poll(h: i32, trx: &mut Trx, limit: u32, buf: &mut [u8],
               dm: &mut crate::dm::DmInfo, path_div: &mut crate::dm::PathDiv,
               rf_path_num: u8, current_band_width: u8,
               current_channel: u8,
               mut deliver: impl FnMut(&crate::rx::RxPktStat, &[u8])) -> u32 {
    let count = get_hw_rx_ring_nr(h, trx).min(limit);
    let mut cur_rp = trx.rx.rp;
    let mut rx_done = 0u32;

    for _ in 0..count {
        dma_check(trx, cur_rp);

        let (bh, boff) = trx.rx.buf_loc(cur_rp);
        // First the descriptor header, then as much as it announces.
        let head = crate::tx::TX_PKT_DESC_SZ.min(buf.len());
        host::dma_read_buf(bh, boff, &mut buf[..head]);
        let stat = crate::rx::query_rx_desc(&buf[..head]);

        let pkt_offset = crate::regs::RX_PKT_DESC_SZ as usize
            + stat.drv_info_sz as usize + stat.shift as usize;
        let total = (pkt_offset + stat.pkt_len as usize).min(buf.len());
        if total > head {
            host::dma_read_buf(bh, boff, &mut buf[..total]);
        }

        // `query_phy_status` needs the block behind the descriptor.
        let mut stat = crate::rx::query_rx_desc_full(&buf[..total], dm,
                                                     path_div, rf_path_num,
                                                     current_band_width);
        // `rtw_pci_rx_napi` does this after stripping the descriptor. Without
        // scanning, `scanning` is false; see there.
        crate::rx::update_rx_freq_for_invalid(&mut stat, current_channel,
                                              false);
        deliver(&stat, &buf[..total]);
        if !stat.is_c2h {
            rx_done += 1;
        }

        sync_rx_desc_device(trx, cur_rp);
        cur_rp += 1;
        if cur_rp >= trx.rx.len {
            cur_rp = 0;
        }
    }

    trx.rx.rp = cur_rp;
    // „'rp', the last position we have read, is seen as previous position
    //  of 'wp' that is used to calculate 'count' next time."
    trx.rx.wp = cur_rp;
    host::w16(h, RTK_PCI_RXBD_IDX_MPDUQ, trx.rx.rp as u16);

    rx_done
}

// ════════════════════════════════════════════════════════════════
// TX path
// ════════════════════════════════════════════════════════════════

/// The ring an ordinary frame is sent from.
///
/// Linux uses `dma_map_single` on the skb itself. There is no allocator
/// here, so there is a fixed slot per ring index, as on the H2C path.
pub const TX_SLOT_BYTES: u32 = 2048;

/// Size of the staging buffer: one slot per ring index of the largest ring
/// using it.
///
/// If it were smaller than `Q_BE`'s 256 entries, `wp * TX_SLOT_BYTES` would
/// run past the buffer; `npk_dma_write` would reject the write, but the
/// buffer descriptor would still point outside our allocation and the chip
/// would transmit foreign memory.
pub const MGMT_STAGE_SLOTS: u32 = RTK_BEQ_TX_DESC_NUM;
pub const MGMT_STAGE_BYTES: u32 = MGMT_STAGE_SLOTS * TX_SLOT_BYTES;

// A limit that cannot drift: if a ring grows, the build fails instead of
// silently writing out of bounds from a certain frame on.
const _: () = assert!(MGMT_STAGE_SLOTS >= RTK_BEQ_TX_DESC_NUM);
const _: () = assert!(MGMT_STAGE_SLOTS >= RTK_DEFAULT_TX_DESC_NUM);
const _: () = assert!(H2C_STAGE_BYTES / H2C_SLOT_BYTES >= RTK_DEFAULT_TX_DESC_NUM);

/// pci.c:897-913 `rtw_pci_tx_write`.
///
/// The `avail_desc < 2` branch stops the mac80211 queue in Linux. There is
/// nothing to stop here, but it is reported: a full ring is backpressure,
/// not an error.
pub fn tx_write(h: i32, trx: &mut Trx, stage: i32, queue: usize,
                info: &mut crate::tx::TxPktInfo, frame: &[u8]) -> bool {
    if !tx_write_data(h, trx, stage, queue, info, frame) {
        return false;
    }
    // `rp` is only advanced by `tx_isr`; in Linux `rtw_pci_tx_isr` does that
    // once the chip has processed a descriptor.
    let r = &trx.tx[queue];
    if avail_desc(r.wp, r.rp, r.len) < 2 {
        host::print("[rtl8822ce] Sendering fast voll (Gegendruck)\n");
    }
    true
}

/// Like `h2c_wait_consumed`, but for any TX queue.
///
/// Waits up to a deadline: right after the kick the chip has not read the
/// descriptor yet, so `hw != wp` alone means nothing.
pub fn tx_wait_consumed(h: i32, trx: &Trx, queue: usize, frist_us: u64)
    -> (bool, u64, u32)
{
    let idx_reg = match TXQ[queue].idx {
        Some(reg) => reg,
        None => return (false, 0, 0),
    };
    let want = trx.tx[queue].wp & TRX_BD_IDX_MASK;
    let start = host::now_us();
    loop {
        let hw = (host::r32(h, idx_reg) & TRX_BD_HW_IDX_MASK) >> 16;
        if hw == want {
            return (true, host::now_us() - start, hw);
        }
        let waited = host::now_us() - start;
        if waited >= frist_us {
            return (false, waited, hw);
        }
    }
}

/// How many descriptors the hardware still has ahead of it in this queue.
///
/// This decides aggregation: the chip aggregates what is in the ring when
/// it wins the medium, not what the driver queued in one pass. The two
/// differ when the medium is busy and descriptors pile up.
///
/// Reads the same register as `tx_isr`: the hardware read pointer is in
/// the upper sixteen bits.
pub fn tx_pending(h: i32, trx: &Trx, queue: usize) -> u32 {
    let idx_reg = match TXQ[queue].idx {
        Some(reg) => reg,
        None => return 0,
    };
    let hw_rp = (host::r32(h, idx_reg) >> 16) & TRX_BD_IDX_MASK;
    let r = &trx.tx[queue];
    if r.wp >= hw_rp {
        r.wp - hw_rp
    } else {
        r.len - (hw_rp - r.wp)
    }
}

/// pci.c:915-1021 `rtw_pci_tx_isr`, the part that advances the read
/// pointer. Without it `r.rp` stands still and `avail_desc` slowly counts
/// the ring full although the chip has fetched everything.
///
/// The rest of Linux's function is buffer management (`skb_dequeue`,
/// `dma_unmap_single`, `ieee80211_tx_status_irqsafe`); here slots are fixed
/// per ring index. Returns how many descriptors completed since last time.
pub fn tx_isr(h: i32, trx: &mut Trx, queue: usize) -> u32 {
    let idx_reg = match TXQ[queue].idx {
        Some(reg) => reg,
        None => return 0,
    };
    let bd_idx = host::r32(h, idx_reg);
    let cur_rp = (bd_idx >> 16) & TRX_BD_IDX_MASK;
    let r = &trx.tx[queue];
    let count = if cur_rp >= r.rp {
        cur_rp - r.rp
    } else {
        r.len - (r.rp - cur_rp)
    };
    trx.tx[queue].rp = cur_rp;
    count
}

/// The PCIe link state: negotiated speed and width, and whether ASPM L1 is
/// enabled.
///
/// rtw88 defends against L1 actively: `rtw_pci_link_ps` (pci.c:1373) is
/// called on entering and leaving every NAPI poll, and its comment warns:
///
/// > we've experienced some inter-operability issues that the link tends to
/// > enter L1 state on the fly even when driver is having high throughput
///
/// That defence is not ported; this reports whether it would matter.
///
/// Returns `None` if the device has no PCIe capability.
pub struct LinkState {
    /// LNKCTL bits 1:0: 0 off · 1 L0s · 2 L1 · 3 both
    pub aspm: u8,
    /// LNKCTL bit 8
    pub clkreq: bool,
    /// LNKSTA bits 3:0: 1 = 2.5 GT/s · 2 = 5 GT/s · 3 = 8 GT/s
    pub speed: u8,
    /// LNKSTA bits 9:4
    pub width: u8,
    /// LNKCAP bits 17:15: the L1 exit latency the chip advertises.
    /// 0..6 = 1/2/4/8/16/32/64 us, 7 = more than 64.
    pub l1_exit: u8,
}

/// PCI power management capability (ID 0x01): bring the device to D0
/// before any register is read.
///
/// Linux does this in the PCI core, not the driver (`pci_enable_device` ->
/// `pci_power_up` -> `pci_raw_set_power_state`), so rtw88 has no line of
/// it. The nopeekOS kernel does not handle PCI power states.
///
/// A device in D3hot answers every MMIO read with all ones while config
/// space answers normally, so the chip ID would read as dead depending on
/// the state the firmware or a previous run left it in.
///
/// D3hot -> D0 takes 10 ms (PCI PM 1.2 §5.6.1; Linux `PCI_PM_D3HOT_WAIT`),
/// and nothing may be read before that.
///
/// Returns the state the device was found in, or `None` if it has no PM
/// capability.
pub fn power_up_d0(_h: i32) -> Option<u8> {
    // Capability list as in `link_state`: 0x34 points to the first entry,
    // byte 0 is the ID, byte 1 the next pointer. The counter bounds a cyclic
    // list.
    let mut ptr = (host::pci_read_config(0x34) & 0xff) as u8;
    let mut schritte = 0;
    while ptr >= 0x40 && ptr != 0xff && schritte < 48 {
        let hdr = host::pci_read_config(ptr);
        if hdr & 0xff == 0x01 {
            // PMCSR is at cap+4, bits 1:0 are the state.
            let pmcsr = host::pci_read_config(ptr + 4);
            let state = (pmcsr & 0x3) as u8;
            if state != 0 {
                // As in Linux: replace only the two state bits and write the rest back.
                // Bit 15 is PME_Status, which clears when a read one is written back;
                // `pci_raw_set_power_state` does the same.
                host::pci_write_config(ptr + 4, (pmcsr & !0x3u32) | 0);
                host::sleep_ms(10);
            }
            return Some(state);
        }
        ptr = ((hdr >> 8) & 0xff) as u8;
        schritte += 1;
    }
    None
}

pub fn link_state() -> Option<LinkState> {
    // Standard capability list: 0x34 points to the first entry, each carries
    // its ID in byte 0 and the next pointer in byte 1. A counter bounds the
    // walk, since broken firmware can produce a cyclic list.
    let mut ptr = (host::pci_read_config(0x34) & 0xff) as u8;
    let mut schritte = 0;
    while ptr >= 0x40 && ptr != 0xff && schritte < 48 {
        let base = ptr & 0xfc;
        let w = host::pci_read_config(base);
        // The entry need not be 4-byte aligned.
        let shift = ((ptr & 0x3) * 8) as u32;
        let id = ((w >> shift) & 0xff) as u8;
        let next = ((w >> (shift + 8)) & 0xff) as u8;
        if id == 0x10 {
            // PCI_CAP_ID_EXP. LNKCAP at +0x0C, LNKCTL at +0x10, LNKSTA at +0x12;
            // LNKCTL and LNKSTA share a dword.
            let lnkcap = host::pci_read_config(ptr + 0x0c);
            let ctlsta = host::pci_read_config(ptr + 0x10);
            return Some(LinkState {
                aspm: (ctlsta & 0x3) as u8,
                clkreq: ctlsta & (1 << 8) != 0,
                speed: ((ctlsta >> 16) & 0xf) as u8,
                width: ((ctlsta >> 20) & 0x3f) as u8,
                l1_exit: ((lnkcap >> 15) & 0x7) as u8,
            });
        }
        ptr = next;
        schritte += 1;
    }
    None
}

/// pci.c:1236-1258 `rtw_dbi_write8`.
///
/// The address is split in two: the low two bits select the byte in the
/// data word (`REG_DBI_WDATA_V1 + remainder`), bits 11:2 the word address,
/// and the byte position is used again as the enable mask `BIT(remainder)`
/// in bits 15:12. Without a WREN bit set, nothing is written.
pub fn dbi_write8(h: i32, addr: u16, data: u8) {
    let remainder = (addr as u32) & !(BITS_DBI_WREN | BITS_DBI_ADDR_MASK);
    let write_addr = ((addr as u32) & BITS_DBI_ADDR_MASK)
        | ((1u32 << remainder) << 12);
    host::w8(h, REG_DBI_WDATA_V1 + remainder, data);
    host::w16(h, REG_DBI_FLAG_V1, write_addr as u16);
    host::w8(h, REG_DBI_FLAG_V1 + 2, (BIT_DBI_WFLAG >> 16) as u8);

    for _ in 0..RTW_PCI_WR_RETRY_CNT {
        if host::r8(h, REG_DBI_FLAG_V1 + 2) == 0 {
            return;
        }
        host::delay_us(10);
    }
    host::say("[rtl8822ce] DBI-Schreibzugriff kam nicht durch\n");
}

/// pci.c:1260-1282 `rtw_dbi_read8`.
pub fn dbi_read8(h: i32, addr: u16) -> Option<u8> {
    let read_addr = (addr as u32) & BITS_DBI_ADDR_MASK;
    host::w16(h, REG_DBI_FLAG_V1, read_addr as u16);
    host::w8(h, REG_DBI_FLAG_V1 + 2, (BIT_DBI_RFLAG >> 16) as u8);

    for _ in 0..RTW_PCI_WR_RETRY_CNT {
        if host::r8(h, REG_DBI_FLAG_V1 + 2) == 0 {
            return Some(host::r8(h, REG_DBI_RDATA_V1 + ((addr as u32) & 3)));
        }
        host::delay_us(10);
    }
    None
}

/// pci.c:1298-1334 `rtw_pci_link_cfg`, the 8822C branch, with one
/// deliberate deviation.
///
/// Linux does two things here: `RTK_PCIE_CLKDLY_CTRL = 0` (the 8822C
/// calibrates its reference clock itself), and, if the host supports
/// CLKREQ, `rtw_pci_clkreq_set(true)`, which enables Realtek's own power
/// saving module.
///
/// The latter is not done: rtw88 enables the module and then takes it out
/// again on every NAPI poll (`rtw_pci_link_ps`, pci.c:1373) because it
/// otherwise drops into L1 under load. This driver has no such poll hook,
/// and enabling the module without managing it would be worse than
/// leaving it off, which is the factory default.
pub fn link_cfg(h: i32) {
    dbi_write8(h, RTK_PCIE_CLKDLY_CTRL, 0);
}

/// The state of Realtek's own link switch, for inspection.
pub fn link_cfg_state(h: i32) -> Option<(bool, bool)> {
    dbi_read8(h, RTK_PCIE_LINK_CFG)
        .map(|v| (v & BIT_L1_SW_EN != 0, v & BIT_CLKREQ_SW_EN != 0))
}

/// Enable or disable the card's standard ASPM in config space, the same as
/// Linux's `pci_disable_link_state(PCIE_LINK_STATE_L1)`.
///
/// This is not Realtek's switch but the bit by which the card tells the
/// bus it may enter L1. The card advertises up to 64 us to exit L1 again
/// (LNKCAP bits 17:15), which may be paid on every packet.
///
/// Returns the previous value of the two bits so the caller can report
/// what it found, not only what it set.
pub fn aspm_host_set(enable: bool) -> Option<u8> {
    let mut ptr = (host::pci_read_config(0x34) & 0xff) as u8;
    let mut schritte = 0;
    while ptr >= 0x40 && ptr != 0xff && schritte < 48 {
        let w = host::pci_read_config(ptr & 0xfc);
        let shift = ((ptr & 0x3) * 8) as u32;
        if ((w >> shift) & 0xff) as u8 == 0x10 {
            let off = ptr + 0x10;
            let cur = host::pci_read_config(off);
            let vorher = (cur & 0x3) as u8;
            // LNKCTL and LNKSTA share the dword. LNKSTA is read-only, so the whole
            // dword may be written back, but only the two ASPM bits change.
            let neu = if enable { cur | 0x2 } else { cur & !0x3 };
            if neu != cur {
                host::pci_write_config(off, neu);
            }
            return Some(vorher);
        }
        ptr = ((w >> (shift + 8)) & 0xff) as u8;
        schritte += 1;
    }
    None
}
