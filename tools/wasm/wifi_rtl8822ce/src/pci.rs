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
    /// **Und ihre DMA-Handles.** Bis 2a wurden die weggeworfen — der Chip
    /// braucht nur die physische Adresse, aber WIR muessen die Puffer auch
    /// LESEN koennen, und dafuer gibt es nur den Handle. Ein Empfangsring,
    /// dessen Inhalt niemand lesen kann, faellt erst auf, wenn das erste
    /// Paket da ist.
    chunk_handle: [i32; 2],
    per_chunk: u32,
}

impl RxRing {
    /// Physische Adresse des i-ten Empfangspuffers — das, was der Chip
    /// bekommt.
    pub fn buf_phys(&self, i: u32) -> u32 {
        let c = (i / self.per_chunk) as usize;
        self.chunk_phys[c] + (i % self.per_chunk) * RX_BUF_STRIDE
    }

    /// Handle und Versatz desselben Puffers — das, womit WIR ihn lesen.
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

// Der DBI-Weg: die PCIe-Konfigurationsregister des Chips, erreicht ueber
// MMIO statt ueber den Konfigurationsraum des Busses. Realtek legt seine
// eigenen Link-Schalter dorthin.
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
pub fn tx_qsel(queue: usize) -> u8 {
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

    // Ein Platz je Ringindex, damit ein noch nicht abgeholtes Paket nicht
    // unter dem Chip weggeschrieben wird. Der H2C-Weg hat sein eigenes,
    // engeres Raster — dort sind die Nutzdaten immer 32 Bytes.
    let stride = if queue == Q_H2C { H2C_SLOT_BYTES } else { TX_SLOT_BYTES };
    let slot = wp * stride;
    // **Beide Schreibzugriffe werden GEPRUEFT.** `npk_dma_write` lehnt
    // einen Versatz hinter dem Puffer ab und gibt -1; wer das wegwirft,
    // traegt gleich darauf eine Adresse in den Buffer-Deskriptor ein, die
    // dem Chip nicht gehoert — und der sendet dann, was dort liegt. Ein
    // abgelehnter DMA-Schreibzugriff ist keine Nebensache, er ist die
    // Meldung, dass der Zwischenpuffer nicht zum Ring passt.
    // Und der Platz muss den Rahmen ueberhaupt fassen. Heute reicht er
    // (48 + 1540 bei MTU 1500), aber ein Ueberlauf HIER liefe in den
    // NACHBARplatz und nicht aus dem Puffer heraus — der Kernel saehe
    // nichts davon.
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
    let mut info = crate::tx::write_data_h2c_get(buf.len() as u32);
    if !tx_write_data(h, trx, stage, Q_H2C, &mut info, buf) {
        host::print("[rtl8822ce] failed to write h2c data\n");
        return false;
    }
    tx_kick_off_queue(h, trx, Q_H2C);
    true
}

/// Wartet, bis die Firmware die H2C-Queue leergeraeumt hat.
///
/// **Kein Linux-Gegenstueck** — dort holt der Treiber den Lesezeiger gar
/// nicht ab, er schreibt und geht weiter. Hier ist es eine MESSUNG: in
/// 0.11.0 stand der HW-Zeiger auf 1, als unserer schon auf 2 stand, und
/// eine Stichprobe einen Befehl nach dem Anstoss sagt nichts darueber, ob
/// der Chip nicht will oder nur noch nicht fertig ist
/// ([[feedback_a_test_of_a_state_must_say_when]]).
///
/// Gibt zurueck, ob er aufgeholt hat, und wie lange es gedauert hat.
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
// Stufe 5a: der Empfangsweg
// ════════════════════════════════════════════════════════════════

/// pci.h:204 `RX_TAG_MAX`
pub const RX_TAG_MAX: u16 = 8192;

/// pci.c:1023-1039 `rtw_pci_get_hw_rx_ring_nr`.
///
/// Der Chip schreibt seinen Stand in die oberen zwoelf Bit desselben
/// Registers, aus dem wir unseren lesen. Die Differenz ist die Zahl der
/// Puffer, die er gefuellt hat.
// ── Interrupts: `pci.c` 374-387, 480-513, 1119-1141; `pci.h` 81-145 ──
//
// Der 8822C ist `RTW_WCPU_3081` (rtw8822c.c:5337), also gehoert HIMR3/HISR3
// dazu. Linux fordert EINEN MSI-Vektor an (`rtw_pci_request_irq`); die
// Behandlung ist zweigeteilt: der harte Teil schaltet HIMR ab, der Faden
// quittiert HISR, arbeitet und schaltet HIMR wieder an. Bei uns zaehlt der
// Kernel-ISR nur und weckt den Treiber-Fiber — beide Haelften laufen dort.
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

/// `rtw_pci_irq_recognized`: HISR lesen, auf die Maske beschraenken und
/// genau das Gelesene wieder loeschen (write-1-to-clear). Bleibt ein Bit
/// stehen, erzeugt der Chip beim naechsten Ereignis KEINE neue MSI-Flanke
/// (Kommentar in `rtw_pci_interrupt_handler`).
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
/// **Hier bekommt `rx_tag` aus Stufe 2a seinen ersten Leser.** Der Chip
/// schreibt in `total_pkt_size` des Pufferdeskriptors eine fortlaufende
/// Marke; stimmt sie nicht mit unserer, hat der Bus etwas verschluckt.
/// Linux warnt und rechnet weiter — genau so steht es hier.
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

/// pci.c:235-250 `rtw_pci_sync_rx_desc_device` — den Platz wieder
/// freigeben, damit der Chip ihn erneut fuellt.
fn sync_rx_desc_device(trx: &Trx, idx: u32) {
    let off = idx * RX_BUF_DESC_SZ;
    // `memset(buf_desc, 0, sizeof(*buf_desc))`, dann buf_size und dma.
    host::dma_w32(trx.rx.handle, off, RTK_PCI_RX_BUF_SIZE & 0xFFFF);
    host::dma_w32(trx.rx.handle, off + 4, trx.rx.buf_phys(idx));
}

/// pci.c:1041-1117 `rtw_pci_rx_napi`, als ABFRAGEweg.
///
/// **Benannte Abweichung:** Linux wird vom Interrupt geweckt und gibt das
/// Paket an `ieee80211_rx_napi`. Wir fragen ab und rufen `deliver` je
/// Paket; alles dazwischen — Deskriptor lesen, `rx_tag` pruefen, Platz
/// wieder freigeben, Zeiger fortschreiben — ist dieselbe Folge.
///
/// Der `skb`-Tausch aus Linux entfaellt: dort wird ein NEUER Puffer
/// angelegt und der alte sofort wieder an die DMA gehaengt, damit der
/// Empfang nicht stockt. Bei uns wird der Inhalt in den Linearspeicher
/// gelesen, und danach ist der Puffer genauso wieder frei.
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
        // Erst den Deskriptorkopf, dann so viel, wie er ansagt.
        let head = crate::tx::TX_PKT_DESC_SZ.min(buf.len());
        host::dma_read_buf(bh, boff, &mut buf[..head]);
        let stat = crate::rx::query_rx_desc(&buf[..head]);

        let pkt_offset = crate::regs::RX_PKT_DESC_SZ as usize
            + stat.drv_info_sz as usize + stat.shift as usize;
        let total = (pkt_offset + stat.pkt_len as usize).min(buf.len());
        if total > head {
            host::dma_read_buf(bh, boff, &mut buf[..total]);
        }

        // `query_phy_status` braucht den Block HINTER dem Deskriptor.
        let mut stat = crate::rx::query_rx_desc_full(&buf[..total], dm,
                                                     path_div, rf_path_num,
                                                     current_band_width);
        // `rtw_pci_rx_napi` tut das nach dem Abziehen des Deskriptors.
        // Ohne Suche ist `scanning` falsch, siehe dort.
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
// Stufe 5b: der Sendeweg
// ════════════════════════════════════════════════════════════════

/// Der Ring, aus dem ein gewoehnlicher Rahmen gesendet wird.
///
/// Linux gibt `dma_map_single` auf dem skb selbst — jeder Rahmen liegt
/// dort, wo der Netzstapel ihn hingelegt hat. Wir haben keinen Allokator,
/// also gibt es einen festen Platz je Ringindex, genau wie beim H2C-Weg.
pub const TX_SLOT_BYTES: u32 = 2048;

/// So gross muss der Zwischenpuffer sein: ein Platz je Ringindex — und
/// zwar des GROESSTEN Rings, der ihn benutzt.
///
/// **Hier stand bis 0.25.1 `RTK_DEFAULT_TX_DESC_NUM` (128), waehrend der
/// Datenring `Q_BE` 256 Eintraege hat.** Ab dem 128. gesendeten Rahmen
/// lag `wp * TX_SLOT_BYTES` hinter dem Puffer. Der Kernel lehnte den
/// Schreibzugriff ab (`npk_dma_write` prueft `off + len > pages * 4096`
/// und gibt -1), der Rueckgabewert wurde weggeworfen — und in den
/// Buffer-Deskriptor ging trotzdem `dma_phys(stage) + slot`, also eine
/// Adresse AUSSERHALB unserer Belegung. Der Chip holte sich von dort
/// fremden Speicher und sendete ihn.
///
/// Das ergibt genau die beobachtete Form: **im Leerlauf haelt die
/// Verbindung lange, unter Verkehr stirbt sie** — die Schwelle ist keine
/// Zeit, sondern eine ANZAHL gesendeter Rahmen. Und danach ist jeder
/// zweite Ringumlauf kaputt (Plaetze 128-255), was von aussen aussieht
/// wie eine Leitung, auf der manchmal etwas durchkommt.
pub const MGMT_STAGE_SLOTS: u32 = RTK_BEQ_TX_DESC_NUM;
pub const MGMT_STAGE_BYTES: u32 = MGMT_STAGE_SLOTS * TX_SLOT_BYTES;

// **Ein Deckel, der nicht wegdriften kann.** Waechst ein Ring, faellt
// der Bau um — statt dass ab einem bestimmten Rahmen still daneben
// geschrieben wird. Genau diese Zusicherung hat bis 0.25.1 gefehlt.
const _: () = assert!(MGMT_STAGE_SLOTS >= RTK_BEQ_TX_DESC_NUM);
const _: () = assert!(MGMT_STAGE_SLOTS >= RTK_DEFAULT_TX_DESC_NUM);
const _: () = assert!(H2C_STAGE_BYTES / H2C_SLOT_BYTES >= RTK_DEFAULT_TX_DESC_NUM);

/// pci.c:897-913 `rtw_pci_tx_write`.
///
/// Der Zweig `avail_desc < 2` haelt in Linux die mac80211-Queue an. Ohne
/// obere Haelfte gibt es nichts anzuhalten; gemeldet wird es trotzdem,
/// denn ein voller Ring ist Gegendruck und kein Fehler.
pub fn tx_write(h: i32, trx: &mut Trx, stage: i32, queue: usize,
                info: &mut crate::tx::TxPktInfo, frame: &[u8]) -> bool {
    if !tx_write_data(h, trx, stage, queue, info, frame) {
        return false;
    }
    // **`rp` steht bei uns still.** In Linux zieht `rtw_pci_tx_isr` ihn
    // nach, wenn der Chip einen Deskriptor abgearbeitet hat; ohne
    // Interrupt und ohne Sendequittung gibt es dafuer noch keinen Weg.
    // Bei drei Rahmen in einem Ring von 128 ist das folgenlos — bei einem
    // LAUFENDEN Sender ist es der naechste Posten (Stufe 5c).
    let r = &trx.tx[queue];
    if avail_desc(r.wp, r.rp, r.len) < 2 {
        host::print("[rtl8822ce] Sendering fast voll (Gegendruck)\n");
    }
    true
}

/// Wie `h2c_wait_consumed`, aber fuer eine beliebige Sendequeue.
///
/// **Das ist eine DEADLINE, keine Stichprobe.** Ein Blick gleich nach dem
/// Anstossen sagt nichts: der Chip hat den Deskriptor dann noch nicht
/// gelesen, und ein `hw != wp` waere kein Befund.
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

/// pci.c:915-1021 `rtw_pci_tx_isr`, der Teil, der den LESEzeiger nachzieht.
///
/// **Ohne ihn steht `r.rp` still** und `avail_desc` zaehlt den Ring
/// langsam voll, obwohl der Chip laengst alles abgeholt hat. Bei drei
/// Rahmen ist das folgenlos, bei einem laufenden Sender nach 127.
///
/// Der Rest von Linux' Funktion ist Pufferverwaltung (`skb_dequeue`,
/// `dma_unmap_single`, `ieee80211_tx_status_irqsafe`) — wir haben feste
/// Plaetze je Ringindex und keinen Netzstapel, der eine Quittung erwartet.
/// Gibt zurueck, wie viele Deskriptoren seit dem letzten Mal fertig wurden.
/// Wieviele Deskriptoren die Hardware in dieser Queue noch VOR SICH hat.
///
/// **Das ist die Zahl, die ueber Aggregation entscheidet.** Der Chip
/// fasst zusammen, was beim Griff nach der Sendegelegenheit im Ring
/// liegt — nicht, was der Treiber in einem Durchlauf eingelegt hat. Die
/// zwei sind verschieden, sobald das Medium belegt ist: dann stapeln
/// sich die Deskriptoren im Ring, waehrend der Treiber sie einzeln
/// nachlegt.
///
/// Gelesen wird derselbe Registerwert wie in `tx_isr`: der Lesezeiger
/// der HARDWARE steht in den oberen sechzehn Bit.
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

/// Der Zustand der PCIe-Strecke: ausgehandelte Geschwindigkeit und Breite,
/// und vor allem **ob ASPM L1 an ist**.
///
/// Warum das hier steht und nicht in einem Papier: rtw88 verteidigt sich
/// aktiv dagegen. `rtw_pci_link_ps` (pci.c:1373) wird beim Betreten und
/// Verlassen JEDES Abholtakts gerufen, und der Kommentar darueber ist eine
/// Warnung, keine Fussnote:
///
/// > we've experienced some inter-operability issues that the link tends to
/// > enter L1 state on the fly even when driver is having high throughput
///
/// Wir portieren diese Verteidigung nicht. Ob uns das etwas kostet, haengt
/// an genau einem Bit im Link-Control-Register der Karte — und das ist eine
/// MESSUNG, keine Vermutung. Deshalb zuerst die Zeile und dann, falls sie
/// „L1 an" sagt, der Umbau.
///
/// Gibt `None`, wenn das Geraet gar keine PCIe-Capability fuehrt (dann ist
/// es kein PCIe-Geraet, und die Frage stellt sich nicht).
pub struct LinkState {
    /// LNKCTL Bit 1:0 — 0 aus · 1 L0s · 2 L1 · 3 beide
    pub aspm: u8,
    /// LNKCTL Bit 8
    pub clkreq: bool,
    /// LNKSTA Bit 3:0 — 1 = 2,5 GT/s · 2 = 5 GT/s · 3 = 8 GT/s
    pub speed: u8,
    /// LNKSTA Bit 9:4
    pub width: u8,
    /// LNKCAP Bit 17:15 — die L1-Austrittszeit, die der Chip ANSAGT.
    /// 0..6 = 1/2/4/8/16/32/64 us, 7 = mehr als 64.
    pub l1_exit: u8,
}

/// PCI Power Management Capability (ID 0x01) — das Geraet nach **D0**
/// holen, bevor jemand ein Register liest.
///
/// **Linux tut das im PCI-Kern, nicht im Treiber** (`pci_enable_device`
/// -> `pci_power_up` -> `pci_raw_set_power_state`), und deshalb steht in
/// rtw88 keine Zeile davon. Unser Kernel kennt Power States gar nicht:
/// `kernel/src/drivers/pci.rs` hat keinen PM-Capability-Gang, kein D0 und
/// keine Wartezeit. Dieselbe Klasse wie
/// [[feedback_the_layer_above_the_driver_fills_in_what_it_never_sets]] —
/// nur liegt die Schicht diesmal unter uns statt darueber.
///
/// **Was das kostet:** ein Geraet in D3hot antwortet auf JEDE
/// MMIO-Lesung mit lauter Einsen. Stufe 0 las die Chipkennung genau
/// einmal, nannte sie tot und der Treiber war zu Ende — mal so, mal so,
/// je nachdem in welchem Zustand die Firmware oder der vorige Lauf die
/// Karte hinterlassen hat. Der Konfigurationsraum antwortet dabei
/// normal, was den Fall so verwirrend macht.
///
/// D3hot -> D0 braucht **10 ms** (PCI PM 1.2 §5.6.1; Linux
/// `PCI_PM_D3HOT_WAIT`), und vorher darf nichts gelesen werden.
///
/// Gibt den Zustand ZURUECK, in dem das Geraet vorgefunden wurde, oder
/// `None`, wenn es keine PM-Capability hat.
pub fn power_up_d0(_h: i32) -> Option<u8> {
    // Capability-Liste wie in `link_state`: 0x34 zeigt auf den ersten
    // Eintrag, Byte 0 ist die Art, Byte 1 der naechste Zeiger. Der
    // Zaehler deckelt eine ringfoermige Liste.
    let mut ptr = (host::pci_read_config(0x34) & 0xff) as u8;
    let mut schritte = 0;
    while ptr >= 0x40 && ptr != 0xff && schritte < 48 {
        let hdr = host::pci_read_config(ptr);
        if hdr & 0xff == 0x01 {
            // PMCSR liegt bei cap+4, Bit 1:0 ist der Zustand.
            let pmcsr = host::pci_read_config(ptr + 4);
            let state = (pmcsr & 0x3) as u8;
            if state != 0 {
                // Wie Linux: NUR die zwei Zustandsbits ersetzen und den
                // Rest zurueckschreiben. Bit 15 ist PME_Status und
                // loescht sich beim Zurueckschreiben einer gelesenen
                // Eins — das tut `pci_raw_set_power_state` genauso.
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
    // Standard-Capability-Liste: 0x34 zeigt auf den ersten Eintrag, jeder
    // traegt seine Art in Byte 0 und den naechsten Zeiger in Byte 1.
    // Ein Zaehler deckelt den Gang — eine ringfoermige Liste gibt es in
    // kaputter Firmware wirklich, und ohne Deckel steht der Treiber.
    let mut ptr = (host::pci_read_config(0x34) & 0xff) as u8;
    let mut schritte = 0;
    while ptr >= 0x40 && ptr != 0xff && schritte < 48 {
        let base = ptr & 0xfc;
        let w = host::pci_read_config(base);
        // Der Eintrag muss nicht auf vier ausgerichtet liegen.
        let shift = ((ptr & 0x3) * 8) as u32;
        let id = ((w >> shift) & 0xff) as u8;
        let next = ((w >> (shift + 8)) & 0xff) as u8;
        if id == 0x10 {
            // PCI_CAP_ID_EXP. LNKCAP bei +0x0C, LNKCTL bei +0x10,
            // LNKSTA bei +0x12 — LNKCTL und LNKSTA teilen sich ein Wort.
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
/// **Die Adresse wandert in ZWEI Teile.** Die unteren zwei Bit waehlen das
/// Byte im Datenwort (`REG_DBI_WDATA_V1 + remainder`), die Bits 11:2 die
/// Wortadresse — und das Byte wird ein zweites Mal gebraucht, als
/// Freigabemaske `BIT(remainder)` in den Bits 15:12. Wer nur die Wortadresse
/// schreibt, schreibt nichts: ohne gesetztes WREN-Bit passiert nichts.
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

/// pci.c:1298-1334 `rtw_pci_link_cfg`, der Zweig fuer 8822C — und EINE
/// benannte Abweichung.
///
/// Linux tut hier zwei Dinge: `RTK_PCIE_CLKDLY_CTRL = 0` (der 8822C
/// kalibriert seinen Referenztakt selbst und braucht keine Verzoegerung),
/// und, wenn der Wirt CLKREQ fuehrt, `rtw_pci_clkreq_set(true)` — es
/// SCHALTET Realteks eigenes Stromsparmodul EIN.
///
/// **Das tun wir nicht, und der Grund ist die Haelfte, die wir nicht
/// haben.** rtw88 schaltet das Modul ein und nimmt es danach in JEDEM
/// Abholtakt wieder heraus (`rtw_pci_link_ps`, pci.c:1373), weil es sonst
/// unter Last in L1 faellt — der Kommentar dort sagt es woertlich. Wir
/// haben keinen solchen Takt und keinen Schlaf ueberhaupt. Das Modul
/// einzuschalten, ohne es zu verwalten, waere die schlechte Haelfte von
/// beidem.
///
/// Ab Werk ist es aus. Wir lassen es aus und sagen es.
pub fn link_cfg(h: i32) {
    dbi_write8(h, RTK_PCIE_CLKDLY_CTRL, 0);
}

/// Der Zustand von Realteks eigenem Link-Schalter, zum Nachsehen.
pub fn link_cfg_state(h: i32) -> Option<(bool, bool)> {
    dbi_read8(h, RTK_PCIE_LINK_CFG)
        .map(|v| (v & BIT_L1_SW_EN != 0, v & BIT_CLKREQ_SW_EN != 0))
}

/// Das STANDARD-ASPM der Karte im Konfigurationsraum ab- oder anschalten —
/// dasselbe, was Linux' `pci_disable_link_state(PCIE_LINK_STATE_L1)` tut.
///
/// **Das ist nicht Realteks Schalter**, sondern das Bit, mit dem die Karte
/// dem Bus gegenueber erklaert, dass sie L1 betreten darf. Es steht auf
/// diesem Geraet AN, und die Karte sagt selbst, dass sie 64 us braucht, um
/// wieder herauszukommen (LNKCAP Bit 17:15). Wer alle 700 us ein Paket
/// bekommt, zahlt das womoeglich jedes Mal.
///
/// Gibt den vorherigen Wert der zwei Bits zurueck, damit der Bericht sagen
/// kann, was er VORGEFUNDEN hat — nicht nur, was er eingestellt hat.
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
            // LNKCTL und LNKSTA teilen sich das Wort. LNKSTA ist rein
            // lesend, also darf das ganze Wort zurueckgeschrieben werden —
            // aber NUR die zwei ASPM-Bits werden veraendert.
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
