//! `pci.c` aus Linux 6.18.26 rtw88 — Stufe 2a: die Ringe.
//!
//! Portiert: `rtw_pci_init_tx_ring` · `rtw_pci_init_rx_ring` ·
//! `rtw_pci_reset_rx_desc` · `rtw_pci_init_trx_ring` ·
//! `rtw_pci_reset_buf_desc` · `rtw_pci_reset_trx_ring`.
//!
//! **Zwei benannte Abweichungen, beide begruendet:**
//!
//! 1. **Nur der MPDU-Empfangsring.** Linux legt `RTK_MAX_RX_QUEUE_NUM = 2`
//!    an (MPDU und C2H), aber `rtw_pci_reset_buf_desc` schreibt NUR
//!    `RXBD_DESA_MPDUQ` in die Hardware — fuer C2H gibt es auf PCIe gar kein
//!    Adressregister. Die Firmwareantworten kommen durch den MPDU-Ring und
//!    werden an `pkt_stat.is_c2h` getrennt. Der zweite Ring ist auf diesem
//!    Bus tote Last: 5,9 MB, die das Geraet nie sieht.
//! 2. **Die Empfangspuffer kommen aus wenigen grossen Stuecken** statt aus
//!    512 einzelnen Allokationen. Die Hardware sieht je Deskriptor nur eine
//!    physische Adresse; ob die aus einem grossen oder einem kleinen Stueck
//!    stammt, ist fuer sie derselbe Vorgang. `npk_dma_alloc` gibt
//!    zusammenhaengende Seiten unter 4 GB, also gilt das auch bei uns.

// `idx`/`handle` gehoeren zu `struct rtw_pci_ring` und werden in 2b/2c
// gebraucht (Kick-off, Nachfuellen). Sie stehen hier, weil sie zum Ring
// gehoeren — nicht, weil Stufe 2a sie liest.
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

/// Seitenraster unserer DMA-Vergabe. Der Puffer ist 11478 Bytes gross; der
/// Schritt wird auf die naechste Seite aufgerundet, damit jede Adresse
/// seitenbuendig liegt.
const RX_BUF_STRIDE: u32 = 12288; // ceil(11478 / 4096) * 4096
const PAGE: u32 = 4096;

/// main.h:202-215 — die Reihenfolge ist Vertrag, `RTW_TX_QUEUE_BK` ist 0.
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

/// Die Adress-, Anzahl- und Indexregister je Queue (pci.h:44-73).
/// `num` ist `None` fuer BCNQ — der Kommentar in pci.h sagt warum:
/// „BCNQ is specialized for rsvd page, does not need to specify a number".
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
    /// Deskriptorring (512 x 8 Byte)
    pub dma: u32,
    pub len: u32,
    pub handle: i32,
    pub wp: u32,
    pub rp: u32,
    /// Bis zu zwei zusammenhaengende Stuecke, aus denen die Puffer stammen.
    chunk_phys: [u32; 2],
    per_chunk: u32,
}

impl RxRing {
    /// Physische Adresse des i-ten Empfangspuffers.
    pub fn buf_phys(&self, i: u32) -> u32 {
        let c = (i / self.per_chunk) as usize;
        self.chunk_phys[c] + (i % self.per_chunk) * RX_BUF_STRIDE
    }
}

pub struct Trx {
    pub tx: [TxRing; N_TX_QUEUES],
    pub rx: RxRing,
    pub dma_pages: u32,
    pub dma_allocs: u32,
}

const EMPTY_TX: TxRing = TxRing { dma: 0, len: 0, handle: -1, wp: 0, rp: 0 };

fn alloc_pages(pages: u32) -> Option<(i32, u32)> {
    let h = host::dma_alloc(pages as u16);
    if h < 0 {
        return None;
    }
    let phys = host::dma_phys(h);
    // Der TX-/RX-Deskriptor hat ein 32-Bit-Adressfeld. Der Kernel vergibt
    // unter 4 GB, aber geprueft wird es hier, nicht geglaubt.
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

    // Die Puffer in zwei Stuecken. 512 x 12288 = 6 MiB = 1536 Seiten, und
    // MAX_DMA_PAGES_PER_CALL ist 1024 — ein Stueck reicht also nicht.
    let per_chunk = RTK_MAX_RX_DESC_NUM / 2;
    let chunk_pages = per_chunk * RX_BUF_STRIDE / PAGE;
    let mut chunk_phys = [0u32; 2];
    for c in 0..2 {
        let (_h, phys) = alloc_pages(chunk_pages)?;
        pages += chunk_pages;
        allocs += 1;
        chunk_phys[c] = phys;
    }

    let rx = RxRing {
        dma: desc_phys,
        len: RTK_MAX_RX_DESC_NUM,
        handle: desc_h,
        wp: 0,
        rp: 0,
        chunk_phys,
        per_chunk,
    };

    // pci.c `rtw_pci_reset_rx_desc` fuer jeden Eintrag: buf_size + dma.
    // `struct rtw_pci_rx_buffer_desc` (pci.h:193) ist
    // { __le16 buf_size; __le16 total_pkt_size; __le32 dma; } = 8 Byte.
    for i in 0..RTK_MAX_RX_DESC_NUM {
        let off = i * RX_BUF_DESC_SZ;
        host::dma_w32(desc_h, off, RTK_PCI_RX_BUF_SIZE & 0xFFFF);
        host::dma_w32(desc_h, off + 4, rx.buf_phys(i));
    }

    Some(Trx { tx, rx, dma_pages: pages, dma_allocs: allocs })
}

/// pci.c `rtw_pci_reset_buf_desc`
pub fn reset_buf_desc(h: i32, trx: &mut Trx) {
    let tmp = host::r8(h, RTK_PCI_CTRL + 3);
    host::w8(h, RTK_PCI_CTRL + 3, tmp | 0xf7);

    // BCNQ: nur die Adresse, keine Anzahl.
    host::w32(h, TXQ[Q_BCN].desa, trx.tx[Q_BCN].dma);

    // Reihenfolge wie in Linux: H2C, BK, BE, VO, VI, MGMT, HI0.
    for &q in &[Q_H2C, Q_BK, Q_BE, Q_VO, Q_VI, Q_MGMT, Q_HI0] {
        let r = &mut trx.tx[q];
        r.rp = 0;
        r.wp = 0;
        if let Some(num) = TXQ[q].num {
            host::w16(h, num, (r.len & TRX_BD_IDX_MASK) as u16);
        }
        host::w32(h, TXQ[q].desa, r.dma);
    }

    trx.rx.rp = 0;
    trx.rx.wp = 0;
    host::w16(h, RTK_PCI_RXBD_NUM_MPDUQ, (trx.rx.len & TRX_BD_IDX_MASK) as u16);
    host::w32(h, RTK_PCI_RXBD_DESA_MPDUQ, trx.rx.dma);

    // reset read/write point
    host::w32(h, RTK_PCI_TXBD_RWPTR_CLR, 0xffff_ffff);

    // reset H2C Queue index in a single write (nur 3081)
    host::set32(h, RTK_PCI_TXBD_H2CQ_CSR,
                BIT_CLR_H2CQ_HOST_IDX | BIT_CLR_H2CQ_HW_IDX);
}

/// Das Gate der Stufe: jedes Adress- und Anzahlregister muss zurueckgeben,
/// was wir hineingeschrieben haben. Ein Ring, dessen Adresse der Chip nicht
/// behaelt, ist kein Ring — und das faellt hier auf, nicht erst, wenn die
/// Firmware durch die BCN-Queue geschoben wird.
pub fn verify_rings(h: i32, trx: &Trx) -> bool {
    let mut ok = true;

    for q in 0..N_TX_QUEUES {
        let desa = host::r32(h, TXQ[q].desa);
        let mut line_ok = desa == trx.tx[q].dma;
        let num = match TXQ[q].num {
            Some(reg) => {
                let v = host::r16(h, reg) as u32 & TRX_BD_IDX_MASK;
                line_ok &= v == (trx.tx[q].len & TRX_BD_IDX_MASK);
                v
            }
            None => 0,
        };
        host::print(if line_ok { "  [ JA  ] " } else { "  [NEIN ] " });
        host::print(TXQ[q].name);
        host::print(" DESA 0x");
        host::print_hex32(desa);
        host::print("  NUM ");
        host::print_dec(num);
        host::print("  (erwartet 0x");
        host::print_hex32(trx.tx[q].dma);
        host::print(", ");
        host::print_dec(trx.tx[q].len);
        host::print(")\n");
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
