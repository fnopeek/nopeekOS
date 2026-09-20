//! `pci.c` aus Linux 6.18.26 rtw88 — Stufe 2a: die Ringe.
//!
//! Portiert: `rtw_pci_init_tx_ring` · `rtw_pci_init_rx_ring` ·
//! `rtw_pci_reset_rx_desc` · `rtw_pci_init_trx_ring` ·
//! `rtw_pci_reset_buf_desc` · `rtw_pci_reset_trx_ring` ·
//! `rtw_pci_dma_reset` · `rtw_pci_setup`.
//!
//! **`rtw_hci_setup` ist `rtw_pci_setup`, und das sind ZWEI Aufrufe.** In
//! 0.4.0 stand hier nur `reset_buf_desc`; `rtw_pci_dma_reset` fehlte, und
//! damit lief die TRX-DMA-Schnittstelle nie an — die erste Reserved Page
//! blieb liegen und `BIT_BCN_VALID_V1` wurde nie 1. Deshalb gibt es `setup()`
//! als EINE Funktion: wer sie ruft, kann die zweite Haelfte nicht vergessen.
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
    /// `rtwpci->rx_tag` — von `rtw_pci_dma_reset` auf 0 gesetzt, gelesen von
    /// `rtw_pci_dma_check` in Stufe 2d.
    pub rx_tag: u16,
}

const EMPTY_TX: TxRing = TxRing { dma: 0, len: 0, handle: -1, wp: 0, rp: 0 };

/// Obergrenze fuer JEDES DMA-Stueck dieses Treibers.
///
/// Zwei Gruende, und nur der erste steht in Linux: die DESA-Register sind
/// 32 Bit breit, also muss alles unter 4 GB liegen. Der zweite kommt vom
/// Geraetelauf — mit der 4-GB-Grenze sucht `allocate_contiguous_below` von
/// oben und landet direkt unter dem PCI-MMIO-Loch (0xcdffa000 abwaerts).
/// Von dort holte der Chip nichts ab und quittierte mit **Received Master
/// Abort**, waehrend die CPU dieselben Bytes ungestoert las und schrieb.
/// Auf AMD-Blech ist das die Gegend von TSEG/DPR.
///
/// 1 GB ist bewusst deutlich darunter und immer noch weit ueber allem,
/// was ein PCIe-Geraet an Ausrichtung verlangt.
const DMA_LIMIT_MB: u32 = 1024;

fn alloc_pages(pages: u32) -> Option<(i32, u32)> {
    let h = host::dma_alloc_below(pages as u16, DMA_LIMIT_MB);
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

    Some(Trx { tx, rx, dma_pages: pages, dma_allocs: allocs, rx_tag: 0 })
}

/// pci.c `rtw_pci_dma_reset` — „reset dma and rx tag".
///
/// Ohne diese eine Zeile faehrt die TRX-DMA-Schnittstelle nicht an. Die
/// Ringadressen stehen dann korrekt in den Registern (und lesen sich auch
/// zurueck), nur holt der Chip die Deskriptoren nie ab.
fn dma_reset(h: i32, trx: &mut Trx) {
    host::set32(h, RTK_PCI_CTRL, BIT_RST_TRXDMA_INTF | BIT_RX_TAG_EN);
    trx.rx_tag = 0;
}

/// pci.c `rtw_pci_setup` — was `rtw_hci_setup` fuer PCIe bedeutet.
/// Beide Haelften, immer zusammen.
pub fn setup(h: i32, trx: &mut Trx, verbose: bool) {
    reset_buf_desc(h, trx, verbose); // = rtw_pci_reset_trx_ring
    dma_reset(h, trx);
}

/// Die OBERE Haelfte eines DESA-Registers auf null setzen.
///
/// Die DESA-Register liegen **acht** Byte auseinander (0x308, 0x310, 0x318,
/// …) — jedes ist ein 64-Bit-Adressregister, und `rtw_pci_reset_buf_desc`
/// schreibt mit `rtw_write32` nur die unteren 32 Bit. Linux kommt damit
/// durch, weil `pci_enable_device` auf einem frisch zurueckgesetzten Gerdt
/// laeuft und die obere Haelfte dann null ist; eine DMA-Maske setzt
/// `rtw_pci_claim` ausdruecklich nicht, die Vorgabe sind 32 Bit.
///
/// Wir setzen kein PCIe-Geraet zurueck. Bleibt oben ein Rest stehen, baut
/// der Chip eine 64-Bit-Adresse, auf die niemand antwortet — und genau das
/// sagte der Geraetelauf: Busmaster an, Anfrage abgeschickt, Received
/// Master Abort, OWN-Bit unberuehrt. Deshalb wird die obere Haelfte
/// AUSDRUECKLICH genullt, und zwar VOR der unteren, damit die Adresse in
/// dem Moment vollstaendig ist, in dem der Chip sie uebernimmt.
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

    // BCNQ: nur die Adresse, keine Anzahl.
    desa_hi_clear(h, TXQ[Q_BCN].desa, TXQ[Q_BCN].name, verbose);
    host::w32(h, TXQ[Q_BCN].desa, trx.tx[Q_BCN].dma);

    // Reihenfolge wie in Linux: H2C, BK, BE, VO, VI, MGMT, HI0.
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
            // pci.h:57 — "BCNQ is specialized for rsvd page, does not need to
            // specify a number". Eine Null hier waere keine Abweichung,
            // sondern eine Frage, die es gar nicht gibt.
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

// ── Stufe 2b: eine Reserved Page ueber die BCN-Queue ─────────────
// pci.h:162-163
pub const RTK_PCI_TXBD_OWN_OFFSET: u32 = 15;
pub const RTK_PCI_TXBD_BCN_WORK: u32 = 0x383;
pub const BIT_PCI_BCNQ_FLAG: u8 = 1 << 4; // pci.h:36

/// Groesstes Stueck, das `download_firmware_to_mem` am Stueck schiebt
/// (mac.c: `max_size = 0x1000`), plus der Deskriptor davor.
pub const RSVD_STAGE_BYTES: u32 = 0x1000 + crate::tx::TX_PKT_DESC_SZ as u32;

/// pci.c `rtw_pci_write_data_rsvd_page` + `rtw_pci_tx_write_data` fuer
/// `RTW_TX_QUEUE_BCN`.
///
/// Der BCN-Weg ist der Sonderfall im Sonderfall: kein `avail_desc`, kein
/// Vorruecken von `wp`, dafuer das OWN-Bit in `psb_len` und ein Anstoss ueber
/// `RTK_PCI_TXBD_BCN_WORK`. `rtw_pci_release_rsvd_page` gibt in Linux das
/// vorige skb frei — bei uns ist der Staging-Puffer fest, es gibt nichts
/// freizugeben.
pub fn write_data_rsvd_page(
    h: i32, trx: &Trx, stage: i32, payload: &[u8], current_band_type: u8,
    verbose: bool,
) -> bool {
    // tx.c `rtw_tx_write_data_rsvd_page_get` baut in Linux ein skb mit 48
    // Byte Vorlauf und ruft dann `rtw_tx_rsvd_page_pkt_info_update`. Bei uns
    // ist der Vorlauf der feste Staging-Puffer, das skb faellt weg.
    let desc_sz = crate::tx::TX_PKT_DESC_SZ;
    let mut info = crate::tx::rsvd_page_pkt_info_update(payload, current_band_type);
    // pci.c: `pkt_info->qsel = rtw_pci_get_tx_qsel(skb, queue)` — fuer die
    // BCN-Queue also BEACON, und erst DANACH wird der Deskriptor gefuellt.
    info.qsel = crate::tx::TX_DESC_QSEL_BEACON;

    let mut desc = [0u8; crate::tx::TX_PKT_DESC_SZ];
    crate::tx::fill_tx_desc(&info, &mut desc);

    // Deskriptor und Nutzdaten liegen zusammenhaengend, wie das skb in Linux
    // nach `skb_push`: der zweite Buffer-Deskriptor zeigt auf dma + 48.
    host::dma_write_buf(stage, 0, &desc);
    host::dma_write_buf(stage, desc_sz as u32, payload);

    let dma = host::dma_phys(stage) as u32;
    let total = desc_sz + payload.len(); // = skb->len nach dem Push
    let mut psb_len = ((total as u32 - 1) / 128) + 1;
    psb_len |= 1 << RTK_PCI_TXBD_OWN_OFFSET;

    // `get_tx_buffer_desc(ring, 16)` mit wp = 0 -> Offset 0.
    //
    // ZWEI Deskriptoren in EINEN Ringplatz, und das passt genau:
    // `struct rtw_pci_tx_buffer_desc` (pci.h:165) ist
    // { __le16 buf_size; __le16 psb_len; __le32 dma; } = 8 Bytes, waehrend
    // `tx_buf_desc_sz` 16 ist. Ein Platz fasst also Kopf- UND Nutzdaten-
    // Deskriptor. Der erste zeigt auf die 48 Deskriptorbytes, der zweite auf
    // die Nutzdaten dahinter.
    let ring = trx.tx[Q_BCN].handle;
    // buf_desc[0] = { buf_size: 48, psb_len, dma }
    host::dma_w32(ring, 0, (desc_sz as u32 & 0xFFFF) | (psb_len << 16));
    host::dma_w32(ring, 4, dma);
    // buf_desc[1] = { buf_size: payload, psb_len: 0, dma + 48 }
    host::dma_w32(ring, 8, payload.len() as u32 & 0xFFFF);
    host::dma_w32(ring, 12, dma + desc_sz as u32);

    host::fence();

    if verbose {
        // Zurueckgelesen, nicht geglaubt: liegen Deskriptor und Nutzdaten
        // wirklich im DMA-Puffer, und steht der Ringeintrag so da, wie wir
        // ihn geschrieben haben?
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

// ── Stufe 3a: rtw_hci_interface_cfg ──────────────────────────────

/// pci.c:1437-1450 `rtw_pci_interface_cfg`.
///
/// Der einzige Chip mit einem Zweig ist der 8822C, und der gilt ab
/// **cut D**. Unser Geraet meldet cut 3 = `RTW_CHIP_VER_CUT_D`, also gilt er.
/// Die Zeile schaltet den PCIe-EMAC im Aux-Takt auf den schnellen Takt um.
pub fn interface_cfg(h: i32, cut_version: u8) {
    if cut_version >= crate::regs::RTW_CHIP_VER_CUT_D {
        host::w32_mask(h, crate::regs::REG_HCI_MIX_CFG,
                       crate::regs::BIT_PCIE_EMAC_PDN_AUX_TO_FAST_CLK, 1);
    }
}

// ── Stufe 4a: die H2C-Queue ──────────────────────────────────────

/// pci.h:62-73 — der Schreibzeiger der H2C-Queue.
pub const RTK_PCI_TXBD_IDX_H2CQ: u32 = 0x132C; // pci.h:66

/// Ein Platz je Ringeintrag fuer den H2C-Zwischenpuffer. Linux legt je
/// Paket ein skb an; solange der Chip einen Deskriptor nicht abgeholt hat,
/// darf sein Inhalt nicht ueberschrieben werden. Ein Platz je Index ist die
/// gleiche Zusage ohne Allokator.
pub const H2C_SLOT_BYTES: u32 = 128; // 48 Deskriptor + 32 Nutzdaten, aufgerundet

/// Der H2C-Zwischenpuffer braucht einen Platz je RINGeintrag, nicht je
/// gesendetem Paket: `wp` laeuft ueber die ganze Ringlaenge und faengt
/// dann von vorn an. Beim Anlauf gehen zwei Pakete raus, danach viele —
/// und ein Puffer, der nur fuer den Anlauf reicht, faellt genau dann um,
/// wenn schon alles zu laufen scheint.
pub const H2C_STAGE_BYTES: u32 = RTK_DEFAULT_TX_DESC_NUM * H2C_SLOT_BYTES;

/// pci.h:154-160 `avail_desc`
#[inline]
pub fn avail_desc(wp: u32, rp: u32, len: u32) -> u32 {
    if rp > wp { rp - wp - 1 } else { len - wp + rp - 1 }
}

/// pci.c:34-52 `rtw_pci_get_tx_qsel` — nur die Queues, die wir fahren.
fn tx_qsel(queue: usize) -> u8 {
    match queue {
        Q_BCN => crate::tx::TX_DESC_QSEL_BEACON,
        Q_H2C => crate::tx::TX_DESC_QSEL_H2C,
        Q_MGMT => crate::tx::TX_DESC_QSEL_MGMT,
        Q_HI0 => crate::tx::TX_DESC_QSEL_HIGH,
        // Linux: `default: return skb->priority;` — die Datenqueues tragen
        // die Priorität des Pakets. Kein Weg, den Stufe 4a benutzt.
        _ => 0,
    }
}

/// pci.c:914-926 `rtw_pci_tx_kick_off_queue`.
///
/// `rtw_pci_deep_ps_leave` davor gilt nur ohne `FW_FEATURE_TX_WAKE` — unsere
/// Firmware (9.9.15, feature 0x1e7) fuehrt das Bit, und Deep-PS ist ohnehin
/// nicht gebaut.
pub fn tx_kick_off_queue(h: i32, trx: &Trx, queue: usize) {
    let idx = match TXQ[queue].idx {
        Some(reg) => reg,
        None => return,
    };
    host::w16(h, idx, (trx.tx[queue].wp & TRX_BD_IDX_MASK) as u16);
}

/// pci.c:806-895 `rtw_pci_tx_write_data`, fuer eine Queue MIT Schreibzeiger.
///
/// Der Unterschied zum Reserved-Page-Weg: hier gibt es `avail_desc`, der
/// Ringplatz richtet sich nach `wp`, das OWN-Bit wird NICHT gesetzt (das ist
/// der BCN-Sonderfall), und `wp` rueckt danach vor.
pub fn tx_write_data(_h: i32, trx: &mut Trx, stage: i32, queue: usize,
                     payload: &[u8]) -> bool {
    let desc_sz = crate::tx::TX_PKT_DESC_SZ;
    let ring_len = trx.tx[queue].len;
    let wp = trx.tx[queue].wp;
    let rp = trx.tx[queue].rp;

    if avail_desc(wp, rp, ring_len) == 0 {
        host::print("[rtl8822ce] H2C-Ring voll\n");
        return false; // Linux: -ENOSPC
    }

    let mut info = crate::tx::write_data_h2c_get(payload.len() as u32);
    info.qsel = tx_qsel(queue);
    let mut desc = [0u8; crate::tx::TX_PKT_DESC_SZ];
    crate::tx::fill_tx_desc(&info, &mut desc);

    // Ein Platz je Ringindex, damit ein noch nicht abgeholtes Paket nicht
    // unter dem Chip weggeschrieben wird.
    let slot = wp * H2C_SLOT_BYTES;
    host::dma_write_buf(stage, slot, &desc);
    host::dma_write_buf(stage, slot + desc_sz as u32, payload);
    let dma = host::dma_phys(stage) as u32 + slot;

    let total = desc_sz + payload.len();
    let psb_len = ((total as u32 - 1) / 128) + 1;

    // `get_tx_buffer_desc(ring, tx_buf_desc_sz)` — der Platz nach `wp`.
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
    if !tx_write_data(h, trx, stage, Q_H2C, buf) {
        host::print("[rtl8822ce] failed to write h2c data\n");
        return false;
    }
    tx_kick_off_queue(h, trx, Q_H2C);
    true
}
