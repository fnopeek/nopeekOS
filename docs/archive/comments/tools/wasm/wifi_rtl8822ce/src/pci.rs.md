# `tools/wasm/wifi_rtl8822ce/src/pci.rs` @ 5e0102684

## L1-26 · `#![allow(dead_code)]`

```
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
```

## L28-30 · `#![allow(dead_code)]`

```
// `idx`/`handle` gehoeren zu `struct rtw_pci_ring` und werden in 2b/2c
// gebraucht (Kick-off, Nachfuellen). Sie stehen hier, weil sie zum Ring
// gehoeren — nicht, weil Stufe 2a sie liest.
```

## L35 · `pub const RTK_DEFAULT_TX_DESC_NUM: u32 = 128;`

```
// ── pci.h:10-15 ──────────────────────────────────────────────────
```

## L41 · `const TX_BUF_DESC_SZ: u32 = 16;`

```
// rtw8822c.c `rtw8822c_hw_spec`
```

## L45-47 · `const RX_BUF_STRIDE: u32 = 12288; // ceil(11478 / 4096) * 4096`

```
/// Seitenraster unserer DMA-Vergabe. Der Puffer ist 11478 Bytes gross; der
/// Schritt wird auf die naechste Seite aufgerundet, damit jede Adresse
/// seitenbuendig liegt.
```

## L48 · `const RX_BUF_STRIDE: u32 = 12288; // ceil(11478 / 4096) * 4096`

```
// ceil(11478 / 4096) * 4096
```

## L51 · `pub const Q_BK: usize = 0;`

```
/// main.h:202-215 — die Reihenfolge ist Vertrag, `RTW_TX_QUEUE_BK` ist 0.
```

## L62 · `fn max_num_of_tx_queue(q: usize) -> u32 {`

```
/// pci.h `max_num_of_tx_queue`
```

## L71-73 · `struct QueueRegs {`

```
/// Die Adress-, Anzahl- und Indexregister je Queue (pci.h:44-73).
/// `num` ist `None` fuer BCNQ — der Kommentar in pci.h sagt warum:
/// „BCNQ is specialized for rsvd page, does not need to specify a number".
```

## L100 · `pub const BIT_RST_TRXDMA_INTF: u32 = 1 << 20; // pci.h:18`

```
// pci.h:18
```

## L101 · `pub const BIT_RX_TAG_EN: u32 = 1 << 15; // pci.h:19`

```
// pci.h:19
```

## L102 · `pub const TRX_BD_IDX_MASK: u32 = 0xFFF; // GENMASK(11, 0)`

```
// GENMASK(11, 0)
```

## L103 · `pub const TRX_BD_HW_IDX_MASK: u32 = 0x0FFF_0000; // GENMASK(27, 16)`

```
// GENMASK(27, 16)
```

## L114 · `pub dma: u32,`

```
/// Deskriptorring (512 x 8 Byte)
```

## L120 · `chunk_phys: [u32; 2],`

```
/// Bis zu zwei zusammenhaengende Stuecke, aus denen die Puffer stammen.
```

## L122-126 · `chunk_handle: [i32; 2],`

```
/// **Und ihre DMA-Handles.** Bis 2a wurden die weggeworfen — der Chip
/// braucht nur die physische Adresse, aber WIR muessen die Puffer auch
/// LESEN koennen, und dafuer gibt es nur den Handle. Ein Empfangsring,
/// dessen Inhalt niemand lesen kann, faellt erst auf, wenn das erste
/// Paket da ist.
```

## L132-133 · `pub fn buf_phys(&self, i: u32) -> u32 {`

```
/// Physische Adresse des i-ten Empfangspuffers — das, was der Chip
/// bekommt.
```

## L139 · `pub fn buf_loc(&self, i: u32) -> (i32, u32) {`

```
/// Handle und Versatz desselben Puffers — das, womit WIR ihn lesen.
```

## L151-152 · `pub rx_tag: u16,`

```
/// `rtwpci->rx_tag` — von `rtw_pci_dma_reset` auf 0 gesetzt, gelesen von
/// `rtw_pci_dma_check` in Stufe 2d.
```

## L158-169 · `const DMA_LIMIT_MB: u32 = 1024;`

```
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
```

## L178-179 · `if phys == 0 || phys >= 0x1_0000_0000 {`

```
// Der TX-/RX-Deskriptor hat ein 32-Bit-Adressfeld. Der Kernel vergibt
// unter 4 GB, aber geprueft wird es hier, nicht geglaubt.
```

## L186 · `pub fn init_trx_ring() -> Option<Trx> {`

```
/// pci.c `rtw_pci_init_trx_ring`
```

## L192 · `for q in 0..N_TX_QUEUES {`

```
// pci.c `rtw_pci_init_tx_ring`
```

## L196 · `return None; // Linux: "len %d exceeds maximum TX entries"`

```
// Linux: "len %d exceeds maximum TX entries"
```

## L206 · `let desc_sz = RX_BUF_DESC_SZ * RTK_MAX_RX_DESC_NUM;`

```
// pci.c `rtw_pci_init_rx_ring`, MPDU
```

## L212-213 · `let per_chunk = RTK_MAX_RX_DESC_NUM / 2;`

```
// Die Puffer in zwei Stuecken. 512 x 12288 = 6 MiB = 1536 Seiten, und
// MAX_DMA_PAGES_PER_CALL ist 1024 — ein Stueck reicht also nicht.
```

## L237-239 · `for i in 0..RTK_MAX_RX_DESC_NUM {`

```
// pci.c `rtw_pci_reset_rx_desc` fuer jeden Eintrag: buf_size + dma.
// `struct rtw_pci_rx_buffer_desc` (pci.h:193) ist
// { __le16 buf_size; __le16 total_pkt_size; __le32 dma; } = 8 Byte.
```

## L249-253 · `fn dma_reset(h: i32, trx: &mut Trx) {`

```
/// pci.c `rtw_pci_dma_reset` — „reset dma and rx tag".
///
/// Ohne diese eine Zeile faehrt die TRX-DMA-Schnittstelle nicht an. Die
/// Ringadressen stehen dann korrekt in den Registern (und lesen sich auch
/// zurueck), nur holt der Chip die Deskriptoren nie ab.
```

## L259-260 · `pub fn setup(h: i32, trx: &mut Trx, verbose: bool) {`

```
/// pci.c `rtw_pci_setup` — was `rtw_hci_setup` fuer PCIe bedeutet.
/// Beide Haelften, immer zusammen.
```

## L262 · `reset_buf_desc(h, trx, verbose); // = rtw_pci_reset_trx_ring`

```
// = rtw_pci_reset_trx_ring
```

## L266-280 · `fn desa_hi_clear(h: i32, desa: u32, name: &str, verbose: bool) {`

```
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
```

## L295 · `pub fn reset_buf_desc(h: i32, trx: &mut Trx, verbose: bool) {`

```
/// pci.c `rtw_pci_reset_buf_desc`
```

## L304 · `desa_hi_clear(h, TXQ[Q_BCN].desa, TXQ[Q_BCN].name, verbose);`

```
// BCNQ: nur die Adresse, keine Anzahl.
```

## L308 · `for &q in &[Q_H2C, Q_BK, Q_BE, Q_VO, Q_VI, Q_MGMT, Q_HI0] {`

```
// Reihenfolge wie in Linux: H2C, BK, BE, VO, VI, MGMT, HI0.
```

## L326 · `host::w32(h, RTK_PCI_TXBD_RWPTR_CLR, 0xffff_ffff);`

```
// reset read/write point
```

## L329 · `host::set32(h, RTK_PCI_TXBD_H2CQ_CSR,`

```
// reset H2C Queue index in a single write (nur 3081)
```

## L334-337 · `pub fn verify_rings(h: i32, trx: &Trx) -> bool {`

```
/// Das Gate der Stufe: jedes Adress- und Anzahlregister muss zurueckgeben,
/// was wir hineingeschrieben haben. Ein Ring, dessen Adresse der Chip nicht
/// behaelt, ist kein Ring — und das faellt hier auf, nicht erst, wenn die
/// Firmware durch die BCN-Queue geschoben wird.
```

## L361-363 · `None => host::print("— (BCNQ hat kein NUM-Register)"),`

```
// pci.h:57 — "BCNQ is specialized for rsvd page, does not need to
// specify a number". Eine Null hier waere keine Abweichung,
// sondern eine Frage, die es gar nicht gibt.
```

## L388-389 · `pub const RTK_PCI_TXBD_OWN_OFFSET: u32 = 15;`

```
// ── Stufe 2b: eine Reserved Page ueber die BCN-Queue ─────────────
// pci.h:162-163
```

## L392 · `pub const BIT_PCI_BCNQ_FLAG: u8 = 1 << 4; // pci.h:36`

```
// pci.h:36
```

## L394-395 · `pub const RSVD_STAGE_BYTES: u32 = 0x1000 + crate::tx::TX_PKT_DESC_SZ as u32;`

```
/// Groesstes Stueck, das `download_firmware_to_mem` am Stueck schiebt
/// (mac.c: `max_size = 0x1000`), plus der Deskriptor davor.
```

## L398-405 · `pub fn write_data_rsvd_page(`

```
/// pci.c `rtw_pci_write_data_rsvd_page` + `rtw_pci_tx_write_data` fuer
/// `RTW_TX_QUEUE_BCN`.
///
/// Der BCN-Weg ist der Sonderfall im Sonderfall: kein `avail_desc`, kein
/// Vorruecken von `wp`, dafuer das OWN-Bit in `psb_len` und ein Anstoss ueber
/// `RTK_PCI_TXBD_BCN_WORK`. `rtw_pci_release_rsvd_page` gibt in Linux das
/// vorige skb frei — bei uns ist der Staging-Puffer fest, es gibt nichts
/// freizugeben.
```

## L410-412 · `let desc_sz = crate::tx::TX_PKT_DESC_SZ;`

```
// tx.c `rtw_tx_write_data_rsvd_page_get` baut in Linux ein skb mit 48
// Byte Vorlauf und ruft dann `rtw_tx_rsvd_page_pkt_info_update`. Bei uns
// ist der Vorlauf der feste Staging-Puffer, das skb faellt weg.
```

## L415-416 · `info.qsel = crate::tx::TX_DESC_QSEL_BEACON;`

```
// pci.c: `pkt_info->qsel = rtw_pci_get_tx_qsel(skb, queue)` — fuer die
// BCN-Queue also BEACON, und erst DANACH wird der Deskriptor gefuellt.
```

## L422-423 · `host::dma_write_buf(stage, 0, &desc);`

```
// Deskriptor und Nutzdaten liegen zusammenhaengend, wie das skb in Linux
// nach `skb_push`: der zweite Buffer-Deskriptor zeigt auf dma + 48.
```

## L428 · `let total = desc_sz + payload.len(); // = skb->len nach dem Push`

```
// = skb->len nach dem Push
```

## L432-439 · `let ring = trx.tx[Q_BCN].handle;`

```
// `get_tx_buffer_desc(ring, 16)` mit wp = 0 -> Offset 0.
//
// ZWEI Deskriptoren in EINEN Ringplatz, und das passt genau:
// `struct rtw_pci_tx_buffer_desc` (pci.h:165) ist
// { __le16 buf_size; __le16 psb_len; __le32 dma; } = 8 Bytes, waehrend
// `tx_buf_desc_sz` 16 ist. Ein Platz fasst also Kopf- UND Nutzdaten-
// Deskriptor. Der erste zeigt auf die 48 Deskriptorbytes, der zweite auf
// die Nutzdaten dahinter.
```

## L441 · `host::dma_w32(ring, 0, (desc_sz as u32 & 0xFFFF) | (psb_len << 16));`

```
// buf_desc[0] = { buf_size: 48, psb_len, dma }
```

## L444 · `host::dma_w32(ring, 8, payload.len() as u32 & 0xFFFF);`

```
// buf_desc[1] = { buf_size: payload, psb_len: 0, dma + 48 }
```

## L451-453 · `host::print("    stage @0x");`

```
// Zurueckgelesen, nicht geglaubt: liegen Deskriptor und Nutzdaten
// wirklich im DMA-Puffer, und steht der Ringeintrag so da, wie wir
// ihn geschrieben haben?
```

## L486 · `let work = host::r8(h, RTK_PCI_TXBD_BCN_WORK);`

```
// pci.c: "reserved pages go through beacon queue"
```

## L499 · `pub fn interface_cfg(h: i32, cut_version: u8) {`

```
// ── Stufe 3a: rtw_hci_interface_cfg ──────────────────────────────
```

## L501-505 · `pub fn interface_cfg(h: i32, cut_version: u8) {`

```
/// pci.c:1437-1450 `rtw_pci_interface_cfg`.
///
/// Der einzige Chip mit einem Zweig ist der 8822C, und der gilt ab
/// **cut D**. Unser Geraet meldet cut 3 = `RTW_CHIP_VER_CUT_D`, also gilt er.
/// Die Zeile schaltet den PCIe-EMAC im Aux-Takt auf den schnellen Takt um.
```

## L513 · `pub const RTK_PCI_TXBD_IDX_H2CQ: u32 = 0x132C; // pci.h:66`

```
// ── Stufe 4a: die H2C-Queue ──────────────────────────────────────
```

## L515 · `pub const RTK_PCI_TXBD_IDX_H2CQ: u32 = 0x132C; // pci.h:66`

```
/// pci.h:62-73 — der Schreibzeiger der H2C-Queue.
```

## L516 · `pub const RTK_PCI_TXBD_IDX_H2CQ: u32 = 0x132C; // pci.h:66`

```
// pci.h:66
```

## L518-520 · `pub const REG_DBI_WDATA_V1: u32 = 0x03E8; // pci.h:20`

```
// Der DBI-Weg: die PCIe-Konfigurationsregister des Chips, erreicht ueber
// MMIO statt ueber den Konfigurationsraum des Busses. Realtek legt seine
// eigenen Link-Schalter dorthin.
```

## L521 · `pub const REG_DBI_WDATA_V1: u32 = 0x03E8; // pci.h:20`

```
// pci.h:20
```

## L522 · `pub const REG_DBI_RDATA_V1: u32 = 0x03EC; // pci.h:21`

```
// pci.h:21
```

## L523 · `pub const REG_DBI_FLAG_V1: u32 = 0x03F0; // pci.h:22`

```
// pci.h:22
```

## L524 · `pub const BIT_DBI_RFLAG: u32 = 1 << 17; // pci.h:23`

```
// pci.h:23
```

## L525 · `pub const BIT_DBI_WFLAG: u32 = 1 << 16; // pci.h:24`

```
// pci.h:24
```

## L526 · `pub const BITS_DBI_WREN: u32 = 0xF000; // pci.h:25 GENMASK(15,12)`

```
// pci.h:25 GENMASK(15,12)
```

## L527 · `pub const BITS_DBI_ADDR_MASK: u32 = 0x0FFC; // pci.h:26 GENMASK(11,2)`

```
// pci.h:26 GENMASK(11,2)
```

## L528 · `pub const RTW_PCI_WR_RETRY_CNT: u32 = 20; // pci.h:35`

```
// pci.h:35
```

## L529 · `pub const RTK_PCIE_LINK_CFG: u16 = 0x0719; // pci.h:37`

```
// pci.h:37
```

## L530 · `pub const BIT_CLKREQ_SW_EN: u8 = 1 << 4; // pci.h:38`

```
// pci.h:38
```

## L531 · `pub const BIT_L1_SW_EN: u8 = 1 << 3; // pci.h:39`

```
// pci.h:39
```

## L532 · `pub const RTK_PCIE_CLKDLY_CTRL: u16 = 0x0725; // pci.h:41`

```
// pci.h:41
```

## L534-537 · `pub const H2C_SLOT_BYTES: u32 = 128; // 48 Deskriptor + 32 Nutzdaten, aufgerundet`

```
/// Ein Platz je Ringeintrag fuer den H2C-Zwischenpuffer. Linux legt je
/// Paket ein skb an; solange der Chip einen Deskriptor nicht abgeholt hat,
/// darf sein Inhalt nicht ueberschrieben werden. Ein Platz je Index ist die
/// gleiche Zusage ohne Allokator.
```

## L538 · `pub const H2C_SLOT_BYTES: u32 = 128; // 48 Deskriptor + 32 Nutzdaten, aufgerundet`

```
// 48 Deskriptor + 32 Nutzdaten, aufgerundet
```

## L540-544 · `pub const H2C_STAGE_BYTES: u32 = RTK_DEFAULT_TX_DESC_NUM * H2C_SLOT_BYTES;`

```
/// Der H2C-Zwischenpuffer braucht einen Platz je RINGeintrag, nicht je
/// gesendetem Paket: `wp` laeuft ueber die ganze Ringlaenge und faengt
/// dann von vorn an. Beim Anlauf gehen zwei Pakete raus, danach viele —
/// und ein Puffer, der nur fuer den Anlauf reicht, faellt genau dann um,
/// wenn schon alles zu laufen scheint.
```

## L547 · `#[inline]`

```
/// pci.h:154-160 `avail_desc`
```

## L553 · `pub fn tx_qsel(queue: usize) -> u8 {`

```
/// pci.c:34-52 `rtw_pci_get_tx_qsel` — nur die Queues, die wir fahren.
```

## L560-561 · `_ => 0,`

```
// Linux: `default: return skb->priority;` — die Datenqueues tragen
// die Priorität des Pakets. Kein Weg, den Stufe 4a benutzt.
```

## L566-570 · `pub fn tx_kick_off_queue(h: i32, trx: &Trx, queue: usize) {`

```
/// pci.c:914-926 `rtw_pci_tx_kick_off_queue`.
///
/// `rtw_pci_deep_ps_leave` davor gilt nur ohne `FW_FEATURE_TX_WAKE` — unsere
/// Firmware (9.9.15, feature 0x1e7) fuehrt das Bit, und Deep-PS ist ohnehin
/// nicht gebaut.
```

## L579-583 · `pub fn tx_write_data(_h: i32, trx: &mut Trx, stage: i32, queue: usize,`

```
/// pci.c:806-895 `rtw_pci_tx_write_data`, fuer eine Queue MIT Schreibzeiger.
///
/// Der Unterschied zum Reserved-Page-Weg: hier gibt es `avail_desc`, der
/// Ringplatz richtet sich nach `wp`, das OWN-Bit wird NICHT gesetzt (das ist
/// der BCN-Sonderfall), und `wp` rueckt danach vor.
```

## L595 · `return false; // Linux: -ENOSPC`

```
// Linux: -ENOSPC
```

## L602-604 · `let stride = if queue == Q_H2C { H2C_SLOT_BYTES } else { TX_SLOT_BYTES };`

```
// Ein Platz je Ringindex, damit ein noch nicht abgeholtes Paket nicht
// unter dem Chip weggeschrieben wird. Der H2C-Weg hat sein eigenes,
// engeres Raster — dort sind die Nutzdaten immer 32 Bytes.
```

## L607-616 · `if desc_sz + payload.len() > stride as usize {`

```
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
```

## L648 · `let ring = trx.tx[queue].handle;`

```
// `get_tx_buffer_desc(ring, tx_buf_desc_sz)` — der Platz nach `wp`.
```

## L665 · `pub fn write_data_h2c(h: i32, trx: &mut Trx, stage: i32, buf: &[u8]) -> bool {`

```
/// pci.c:1206-1224 `rtw_pci_write_data_h2c`
```

## L676-685 · `pub fn h2c_wait_consumed(h: i32, trx: &Trx, frist_us: u64) -> (bool, u64, u32) {`

```
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
```

## L702-704 · `pub const RX_TAG_MAX: u16 = 8192;`

```
// ════════════════════════════════════════════════════════════════
// Stufe 5a: der Empfangsweg
// ════════════════════════════════════════════════════════════════
```

## L706 · `pub const RX_TAG_MAX: u16 = 8192;`

```
/// pci.h:204 `RX_TAG_MAX`
```

## L709-720 · `pub const RTK_PCI_HIMR0: u32 = 0x0B0;`

```
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
```

## L737 · `const IMR_TXFOVW: u32 = 1 << 9; // HIMR1`

```
// HIMR1
```

## L738 · `const IMR_H2CDOK: u32 = 1 << 16; // HIMR3`

```
// HIMR3
```

## L740 · `pub const IRQ_MASK: [u32; 4] = [`

```
/// `rtw_pci_setup`: `rtwpci->irq_mask[0..3]`.
```

## L749 · `pub fn enable_interrupt(h: i32, exclude_rx: bool) {`

```
/// `rtw_pci_enable_interrupt`.
```

## L757 · `pub fn disable_interrupt(h: i32) {`

```
/// `rtw_pci_disable_interrupt`.
```

## L764-767 · `pub fn irq_recognized(h: i32) -> [u32; 4] {`

```
/// `rtw_pci_irq_recognized`: HISR lesen, auf die Maske beschraenken und
/// genau das Gelesene wieder loeschen (write-1-to-clear). Bleibt ein Bit
/// stehen, erzeugt der Chip beim naechsten Ereignis KEINE neue MSI-Flanke
/// (Kommentar in `rtw_pci_interrupt_handler`).
```

## L794-799 · `pub fn dma_check(trx: &mut Trx, idx: u32) -> bool {`

```
/// pci.c:683-702 `rtw_pci_dma_check`.
///
/// **Hier bekommt `rx_tag` aus Stufe 2a seinen ersten Leser.** Der Chip
/// schreibt in `total_pkt_size` des Pufferdeskriptors eine fortlaufende
/// Marke; stimmt sie nicht mit unserer, hat der Bus etwas verschluckt.
/// Linux warnt und rechnet weiter — genau so steht es hier.
```

## L802-803 · `let w0 = host::dma_r32(trx.rx.handle, off);`

```
// `struct rtw_pci_rx_buffer_desc` (pci.h:193):
// { __le16 buf_size; __le16 total_pkt_size; __le32 dma; }
```

## L818-819 · `fn sync_rx_desc_device(trx: &Trx, idx: u32) {`

```
/// pci.c:235-250 `rtw_pci_sync_rx_desc_device` — den Platz wieder
/// freigeben, damit der Chip ihn erneut fuellt.
```

## L822 · `host::dma_w32(trx.rx.handle, off, RTK_PCI_RX_BUF_SIZE & 0xFFFF);`

```
// `memset(buf_desc, 0, sizeof(*buf_desc))`, dann buf_size und dma.
```

## L827-837 · `#[allow(clippy::too_many_arguments)]`

```
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
```

## L852 · `let head = crate::tx::TX_PKT_DESC_SZ.min(buf.len());`

```
// Erst den Deskriptorkopf, dann so viel, wie er ansagt.
```

## L864 · `let mut stat = crate::rx::query_rx_desc_full(&buf[..total], dm,`

```
// `query_phy_status` braucht den Block HINTER dem Deskriptor.
```

## L868-869 · `crate::rx::update_rx_freq_for_invalid(&mut stat, current_channel,`

```
// `rtw_pci_rx_napi` tut das nach dem Abziehen des Deskriptors.
// Ohne Suche ist `scanning` falsch, siehe dort.
```

## L885-886 · `trx.rx.wp = cur_rp;`

```
// „'rp', the last position we have read, is seen as previous position
//  of 'wp' that is used to calculate 'count' next time."
```

## L893-895 · `pub const TX_SLOT_BYTES: u32 = 2048;`

```
// ════════════════════════════════════════════════════════════════
// Stufe 5b: der Sendeweg
// ════════════════════════════════════════════════════════════════
```

## L897-901 · `pub const TX_SLOT_BYTES: u32 = 2048;`

```
/// Der Ring, aus dem ein gewoehnlicher Rahmen gesendet wird.
///
/// Linux gibt `dma_map_single` auf dem skb selbst — jeder Rahmen liegt
/// dort, wo der Netzstapel ihn hingelegt hat. Wir haben keinen Allokator,
/// also gibt es einen festen Platz je Ringindex, genau wie beim H2C-Weg.
```

## L904-920 · `pub const MGMT_STAGE_SLOTS: u32 = RTK_BEQ_TX_DESC_NUM;`

```
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
```

## L924-926 · `const _: () = assert!(MGMT_STAGE_SLOTS >= RTK_BEQ_TX_DESC_NUM);`

```
// **Ein Deckel, der nicht wegdriften kann.** Waechst ein Ring, faellt
// der Bau um — statt dass ab einem bestimmten Rahmen still daneben
// geschrieben wird. Genau diese Zusicherung hat bis 0.25.1 gefehlt.
```

## L931-935 · `pub fn tx_write(h: i32, trx: &mut Trx, stage: i32, queue: usize,`

```
/// pci.c:897-913 `rtw_pci_tx_write`.
///
/// Der Zweig `avail_desc < 2` haelt in Linux die mac80211-Queue an. Ohne
/// obere Haelfte gibt es nichts anzuhalten; gemeldet wird es trotzdem,
/// denn ein voller Ring ist Gegendruck und kein Fehler.
```

## L941-945 · `let r = &trx.tx[queue];`

```
// **`rp` steht bei uns still.** In Linux zieht `rtw_pci_tx_isr` ihn
// nach, wenn der Chip einen Deskriptor abgearbeitet hat; ohne
// Interrupt und ohne Sendequittung gibt es dafuer noch keinen Weg.
// Bei drei Rahmen in einem Ring von 128 ist das folgenlos — bei einem
// LAUFENDEN Sender ist es der naechste Posten (Stufe 5c).
```

## L953-957 · `pub fn tx_wait_consumed(h: i32, trx: &Trx, queue: usize, frist_us: u64)`

```
/// Wie `h2c_wait_consumed`, aber fuer eine beliebige Sendequeue.
///
/// **Das ist eine DEADLINE, keine Stichprobe.** Ein Blick gleich nach dem
/// Anstossen sagt nichts: der Chip hat den Deskriptor dann noch nicht
/// gelesen, und ein `hw != wp` waere kein Befund.
```

## L979-999 · `pub fn tx_pending(h: i32, trx: &Trx, queue: usize) -> u32 {`

```
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
```

## L1031-1048 · `pub struct LinkState {`

```
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
```

## L1050 · `pub aspm: u8,`

```
/// LNKCTL Bit 1:0 — 0 aus · 1 L0s · 2 L1 · 3 beide
```

## L1052 · `pub clkreq: bool,`

```
/// LNKCTL Bit 8
```

## L1054 · `pub speed: u8,`

```
/// LNKSTA Bit 3:0 — 1 = 2,5 GT/s · 2 = 5 GT/s · 3 = 8 GT/s
```

## L1056 · `pub width: u8,`

```
/// LNKSTA Bit 9:4
```

## L1058-1059 · `pub l1_exit: u8,`

```
/// LNKCAP Bit 17:15 — die L1-Austrittszeit, die der Chip ANSAGT.
/// 0..6 = 1/2/4/8/16/32/64 us, 7 = mehr als 64.
```

## L1063-1085 · `pub fn power_up_d0(_h: i32) -> Option<u8> {`

```
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
```

## L1087-1089 · `let mut ptr = (host::pci_read_config(0x34) & 0xff) as u8;`

```
// Capability-Liste wie in `link_state`: 0x34 zeigt auf den ersten
// Eintrag, Byte 0 ist die Art, Byte 1 der naechste Zeiger. Der
// Zaehler deckelt eine ringfoermige Liste.
```

## L1095 · `let pmcsr = host::pci_read_config(ptr + 4);`

```
// PMCSR liegt bei cap+4, Bit 1:0 ist der Zustand.
```

## L1099-1102 · `host::pci_write_config(ptr + 4, (pmcsr & !0x3u32) | 0);`

```
// Wie Linux: NUR die zwei Zustandsbits ersetzen und den
// Rest zurueckschreiben. Bit 15 ist PME_Status und
// loescht sich beim Zurueckschreiben einer gelesenen
// Eins — das tut `pci_raw_set_power_state` genauso.
```

## L1115-1118 · `let mut ptr = (host::pci_read_config(0x34) & 0xff) as u8;`

```
// Standard-Capability-Liste: 0x34 zeigt auf den ersten Eintrag, jeder
// traegt seine Art in Byte 0 und den naechsten Zeiger in Byte 1.
// Ein Zaehler deckelt den Gang — eine ringfoermige Liste gibt es in
// kaputter Firmware wirklich, und ohne Deckel steht der Treiber.
```

## L1124 · `let shift = ((ptr & 0x3) * 8) as u32;`

```
// Der Eintrag muss nicht auf vier ausgerichtet liegen.
```

## L1129-1130 · `let lnkcap = host::pci_read_config(ptr + 0x0c);`

```
// PCI_CAP_ID_EXP. LNKCAP bei +0x0C, LNKCTL bei +0x10,
// LNKSTA bei +0x12 — LNKCTL und LNKSTA teilen sich ein Wort.
```

## L1147-1153 · `pub fn dbi_write8(h: i32, addr: u16, data: u8) {`

```
/// pci.c:1236-1258 `rtw_dbi_write8`.
///
/// **Die Adresse wandert in ZWEI Teile.** Die unteren zwei Bit waehlen das
/// Byte im Datenwort (`REG_DBI_WDATA_V1 + remainder`), die Bits 11:2 die
/// Wortadresse — und das Byte wird ein zweites Mal gebraucht, als
/// Freigabemaske `BIT(remainder)` in den Bits 15:12. Wer nur die Wortadresse
/// schreibt, schreibt nichts: ohne gesetztes WREN-Bit passiert nichts.
```

## L1171 · `pub fn dbi_read8(h: i32, addr: u16) -> Option<u8> {`

```
/// pci.c:1260-1282 `rtw_dbi_read8`.
```

## L1186-1202 · `pub fn link_cfg(h: i32) {`

```
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
```

## L1207 · `pub fn link_cfg_state(h: i32) -> Option<(bool, bool)> {`

```
/// Der Zustand von Realteks eigenem Link-Schalter, zum Nachsehen.
```

## L1213-1223 · `pub fn aspm_host_set(enable: bool) -> Option<u8> {`

```
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
```

## L1234-1236 · `let neu = if enable { cur | 0x2 } else { cur & !0x3 };`

```
// LNKCTL und LNKSTA teilen sich das Wort. LNKSTA ist rein
// lesend, also darf das ganze Wort zurueckgeschrieben werden —
// aber NUR die zwei ASPM-Bits werden veraendert.
```

