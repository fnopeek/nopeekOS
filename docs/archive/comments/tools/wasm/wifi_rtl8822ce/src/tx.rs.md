# `tools/wasm/wifi_rtl8822ce/src/tx.rs` @ 5e0102684

## L1-6 · `#![allow(dead_code)]`

```
//! `tx.c` aus Linux 6.18.26 rtw88 — nur der Teil, den der Firmware-Download
//! braucht: der 48-Byte-Sendedeskriptor einer Reserved Page.
//!
//! Portiert: `rtw_tx_pkt_info_update_rate` · `rtw_tx_pkt_info_update_sec` ·
//! `rtw_tx_rsvd_page_pkt_info_update` (Typ `RSVD_BEACON`) ·
//! `rtw_tx_fill_tx_desc`.
```

## L11 · `pub const RTW_RATEID_G: u8 = 7;`

```
// main.h:232-248
```

## L14 · `pub const DESC_RATE1M: u8 = 0x00;`

```
// main.h:250-262
```

## L18 · `pub const RTW_BAND_2G: u8 = 1;`

```
// main.h:87
```

## L22-24 · `pub fn band_of(channel: u8) -> u8 {`

```
/// Das Band zu einem Kanal. **Kein Rateschritt, sondern die Frage, ob es
/// die Rate ueberhaupt gibt**: `pkt_info_update_rate` waehlt fuer 2,4 GHz
/// 1 Mbit DSSS, und DSSS gibt es oberhalb von Kanal 14 nicht.
```

## L29-31 · `pub const TX_DESC_QSEL_BEACON: u8 = 16;`

```
// tx.h:79-83 — abgelesen, nicht abgeleitet. HIGH ist 17 und H2C 19; mit
// einem geratenen 17 fuer H2C haette `more_data` (qsel == HIGH) still
// mitgefeuert.
```

## L37 · `pub const TX_PKT_DESC_SZ: usize = 48;`

```
/// rtw8822c.c `rtw8822c_hw_spec.tx_pkt_desc_sz`
```

## L40-42 · `#[derive(Default)]`

```
/// `struct rtw_tx_pkt_info` (tx.h) — nur die Felder, die der
/// Reserved-Page-Weg setzt. Alles andere bleibt null, wie in Linux, wo
/// `pkt_info` als `{0}` angelegt wird.
```

## L81-88 · `fn get_mgmt_rate(lowest_rate: u8) -> u8 {`

```
/// tx.c:44-58 `rtw_get_mgmt_rate`.
///
/// Ohne vif, ohne `basic_rates`, mit `ignore_rate` oder mit
/// `RTW_FLAG_FORCE_LOWEST_RATE` kommt `lowest_rate` heraus. Vor dem
/// Verbinden gibt es keine `basic_rates` — die kennt erst
/// `bss_conf`, also frueher als Stufe 5c nie. Der `__ffs`-Zweig ist
/// damit noch nicht erreichbar und steht bewusst nicht da; er waere eine
/// Behauptung ueber einen Zustand, den der Treiber nicht fuehrt.
```

## L93 · `fn pkt_info_update_rate(info: &mut TxPktInfo, current_band_type: u8) {`

```
/// tx.c:60-77 `rtw_tx_pkt_info_update_rate`.
```

## L106-112 · `pub fn rsvd_page_pkt_info_update(payload: &[u8], current_band_type: u8) -> TxPktInfo {`

```
/// tx.c `rtw_tx_rsvd_page_pkt_info_update` fuer `type == RSVD_BEACON`.
///
/// **`bmc` wird aus dem NUTZDATEN gelesen**, und das ist kein Versehen:
/// Linux legt `struct ieee80211_hdr *hdr = skb->data` auf den Puffer, auch
/// wenn darin Firmware steht. `addr1` liegt bei Offset 4. Wir machen es
/// genauso — eine Abweichung hier waere ein anderer Deskriptor als der, den
/// der Chip unter Linux bekommt.
```

## L116 · `pkt_info_update_rate(&mut info, current_band_type);`

```
// type == RSVD_BEACON -> qsel wird hier NICHT gesetzt (pci.c tut es).
```

## L127 · `info.dis_qselseq = true;`

```
// RSVD_BEACON ist kein RSVD_PS_POLL:
```

## L131-132 · `info`

```
// RSVD_BEACON mit leerer rsvd_page_list -> tim_offset bleibt 0.
// `rtw_tx_pkt_info_update_sec` ohne hw_key -> sec_type bleibt 0.
```

## L136 · `pub fn fill_tx_desc(info: &TxPktInfo, desc: &mut [u8; TX_PKT_DESC_SZ]) {`

```
/// tx.c `rtw_tx_fill_tx_desc`. Schreibt die 48 Bytes an den Anfang von `desc`.
```

## L140 · `let w0 = bits(info.tx_pkt_size, 0x0000_FFFF)          // TXPKTSIZE 15:0`

```
// TXPKTSIZE 15:0
```

## L141 · `| bits(info.offset as u32, 0x00FF_0000)           // OFFSET 23:16`

```
// OFFSET 23:16
```

## L142 · `| bits(info.bmc as u32, 1 << 24)                  // BMC`

```
// BMC
```

## L143 · `| bits(info.ls as u32, 1 << 26)                   // LS`

```
// LS
```

## L144 · `| bits(info.dis_qselseq as u32, 1 << 31);         // DISQSELSEQ`

```
// DISQSELSEQ
```

## L146 · `let w1 = bits(info.mac_id as u32, 0x0000_00FF)        // MACID 7:0`

```
// MACID 7:0
```

## L147 · `| bits(info.qsel as u32, 0x0000_1F00)             // QSEL 12:8`

```
// QSEL 12:8
```

## L148 · `| bits(info.rate_id as u32, 0x001F_0000)          // RATE_ID 20:16`

```
// RATE_ID 20:16
```

## L149 · `| bits(info.sec_type as u32, 0x00C0_0000)         // SEC_TYPE 23:22`

```
// SEC_TYPE 23:22
```

## L150 · `| bits(info.pkt_offset as u32, 0x1F00_0000)       // PKT_OFFSET 28:24`

```
// PKT_OFFSET 28:24
```

## L151 · `| bits(more_data as u32, 1 << 29);                // MORE_DATA`

```
// MORE_DATA
```

## L153 · `let w2 = bits(info.ampdu_en as u32, 1 << 12)          // AGG_EN`

```
// AGG_EN
```

## L154 · `| bits(info.report as u32, 1 << 19)               // SPE_RPT`

```
// SPE_RPT
```

## L155 · `| bits(info.ampdu_density as u32, 0x0070_0000)    // AMPDU_DEN 22:20`

```
// AMPDU_DEN 22:20
```

## L156 · `| bits(info.bt_null as u32, 1 << 23);             // BT_NULL`

```
// BT_NULL
```

## L158 · `let w3 = bits(info.hw_ssn_sel as u32, 0x0000_00C0)    // HW_SSN_SEL 7:6`

```
// HW_SSN_SEL 7:6
```

## L159 · `| bits(info.use_rate as u32, 1 << 8)              // USE_RATE`

```
// USE_RATE
```

## L160 · `| bits(info.dis_rate_fallback as u32, 1 << 10)    // DISDATAFB`

```
// DISDATAFB
```

## L161 · `| bits(info.rts as u32, 1 << 12)                  // USE_RTS`

```
// USE_RTS
```

## L162 · `| bits(info.nav_use_hdr as u32, 1 << 15)          // NAVUSEHDR`

```
// NAVUSEHDR
```

## L163 · `| bits(info.ampdu_factor as u32, 0x003E_0000);    // MAX_AGG_NUM 21:17`

```
// MAX_AGG_NUM 21:17
```

## L165 · `let mut w4 = bits(info.rate as u32, 0x0000_007F);     // DATARATE 6:0`

```
// `old_datarate_fb_limit` ist beim 8822C false — der Zweig entfaellt.
```

## L166 · `let mut w4 = bits(info.rate as u32, 0x0000_007F);     // DATARATE 6:0`

```
// DATARATE 6:0
```

## L168 · `let mut w5 = bits(info.short_gi as u32, 1 << 4)       // DATA_SHORT`

```
// DATA_SHORT
```

## L169 · `| bits(info.bw as u32, 0x0000_0060)               // DATA_BW 6:5`

```
// DATA_BW 6:5
```

## L170 · `| bits(info.ldpc as u32, 1 << 7)                  // DATA_LDPC`

```
// DATA_LDPC
```

## L171 · `| bits(info.stbc as u32, 0x0000_0300);            // DATA_STBC 9:8`

```
// DATA_STBC 9:8
```

## L173 · `let w6 = bits(info.sn as u32, 0x0000_0FFF);           // SW_DEFINE 11:0`

```
// SW_DEFINE 11:0
```

## L174 · `let w8 = bits(info.en_hwseq as u32, 1 << 15);         // EN_HWSEQ`

```
// EN_HWSEQ
```

## L175 · `let mut w9 = bits(info.seq as u32, 0x00FF_F000);      // SW_SEQ 23:12`

```
// SW_SEQ 23:12
```

## L178 · `w4 |= bits(DESC_RATE24M as u32, 0x1F00_0000);     // RTSRATE 28:24`

```
// RTSRATE 28:24
```

## L179 · `w5 |= bits(1, 1 << 12);                           // DATA_RTS_SHORT`

```
// DATA_RTS_SHORT
```

## L182 · `w9 |= bits(1, 1 << 7)                             // TIM_EN`

```
// TIM_EN
```

## L183 · `| bits(info.tim_offset as u32, 0x0000_007F);  // TIM_OFFSET 6:0`

```
// TIM_OFFSET 6:0
```

## L192-197 · `pub fn write_data_h2c_get(size: u32) -> TxPktInfo {`

```
/// tx.c:524-546 `rtw_tx_write_data_h2c_get`.
///
/// Linux legt dafuer ein skb an, reserviert `tx_pkt_desc_sz` Vorlauf und
/// kopiert die 32 Bytes dahinter. Das ist bei uns der Staging-Puffer; vom
/// `pkt_info` setzt die Funktion **genau ein Feld**, alles andere bleibt
/// null — auch `offset` und `ls`, anders als beim Reserved-Page-Weg.
```

## L202 · `fn mgmt_pkt_info_update(info: &mut TxPktInfo, current_band_type: u8) {`

```
// ── Stufe 5b: ein gewoehnlicher Rahmen statt einer Reserved Page ──
```

## L204 · `fn mgmt_pkt_info_update(info: &mut TxPktInfo, current_band_type: u8) {`

```
/// tx.c:391-398 `rtw_tx_mgmt_pkt_info_update`
```

## L210 · `}`

```
// Linux: „TODO: need to change hw port and hw ssn sel for multiple vifs"
```

## L213 · `pub const RTW_TX_QUEUE_BCN: usize = 4;`

```
/// main.h:202-215 — die Queues, plus `ac_to_hwq`.
```

## L218-222 · `pub fn queue_mapping(fc: u16, addr1: &[u8]) -> usize {`

```
/// tx.c:641-662 `rtw_tx_queue_mapping`.
///
/// `q_mapping` ist die Zugangsklasse, die mac80211 an den skb haengt; ohne
/// obere Haelfte gibt es sie nicht, und deshalb steht hier BE — derselbe
/// Wert, den Linux' `WARN_ON_ONCE`-Zweig nimmt, wenn sie fehlt.
```

## L242-251 · `pub fn pkt_info_update(frame: &[u8], mac_id: u8, current_band_type: u8)`

```
/// tx.c:414-455 `rtw_tx_pkt_info_update`, fuer einen Rahmen OHNE `sta`.
///
/// **Was nicht da steht und warum:** `si`/`rtwvif` fuer `mac_id` kommt vom
/// Rufer (bei uns die 0 aus `vif::add_interface_station`) ·
/// `rtw_tx_data_pkt_info_update` gehoert zum Datenzweig, den es vor einer
/// Verbindung nicht gibt · `rtw_tx_report_enable` haengt an
/// `IEEE80211_TX_CTL_REQ_TX_STATUS`, einer Fahne der oberen Haelfte ·
/// `rtw_tx_pkt_info_update_sec` ohne `hw_key` laesst `sec_type` auf null,
/// wie beim Reserved-Page-Weg · `rtw_tx_stats` zaehlt fuer die obere
/// Haelfte.
```

## L259-260 · `let is_mgmt = ftype == 0;`

```
// `ieee80211_is_nullfunc`: Datentyp, Subtyp 4 (Null) — hier mitgefuehrt,
// weil Linux ihn in DENSELBEN Zweig schickt wie die Verwaltung.
```

## L274-275 · `info.ls = true;`

```
// `pkt_info->qsel = skb->priority` — bei uns setzt es `tx_qsel` aus der
// Queue, genau wie `rtw_pci_get_tx_qsel` es fuer jede andere Queue tut.
```

## L280-290 · `pub fn data_pkt_info_update(info: &mut TxPktInfo, seq: u16,`

```
/// tx.c:400-412 `rtw_tx_data_pkt_info_update`, Zweig MIT `sta`.
///
/// **Was `!sta` (Broadcast/Multicast) angeht, steht in Linux VOR dem
/// Sprung**: Rate 6M, rate_id 6, 20 MHz. Das ist hier der Zweig
/// `si = None`.
///
/// `ampdu_en` haengt an `IEEE80211_TX_CTL_AMPDU` — eine Fahne, die
/// mac80211 setzt, wenn ein Block-Ack-Block offen ist. Ohne
/// Block-Ack-Aushandlung gibt es sie nicht, und ein erfundenes
/// A-MPDU-Flag waere schlimmer als keins. `dm_info->fix_rate` ist eine
/// debugfs-Einstellung und steht auf `DESC_RATE_MAX` (= aus).
```

## L296 · `let mut bw = 0u8; // RTW_CHANNEL_WIDTH_20`

```
// RTW_CHANNEL_WIDTH_20
```

## L317-321 · `pub fn highest_ht_tx_rate(ht_mcs: &[u8; 4], rf_2t2r: bool) -> u8 {`

```
/// tx.c:112-123 `get_highest_ht_tx_rate`.
///
/// Die Bedingung ist `rf_type == RF_2T2R`, nicht `nss > 1` — auf diesem
/// Chip dasselbe, aber die Quelle fragt nach dem RF-Aufbau und nicht nach
/// der Zahl der Stroeme.
```

## L330-342 · `pub fn highest_vht_tx_rate(tx_mcs_map: u16, nss: u8) -> u8 {`

```
/// tx.c:125-165 `get_highest_vht_tx_rate`.
///
/// **VHT geht VOR HT** (tx.c:367-370), und das ist der Grund, warum es
/// diese Funktion hier ueberhaupt geben muss: bis hierher fragte der
/// Treiber nur `ht_supported`, und damit stand auf einer VHT-Verbindung
/// `DESC_RATEMCS15` im Deskriptor — eine HT-Rate auf einer Strecke, die
/// gerade VHT faehrt. Der Bericht meldete sie als „angeboten", waehrend
/// die Firmware VHT-Raten zurueckmeldete: zwei Antworten auf eine Frage.
///
/// Gelesen wird die SENDE-Karte des Gegenuebers (`tx_mcs_map`), zwei Bit
/// je Strom. Linux fragt `efuse->hw_cap.nss`, also UNSERE Stroeme — die
/// Karte sagt, wie hoch der andere darf, die efuse, wie viele Stroeme wir
/// ueberhaupt haben.
```

## L344-356 · `let top = |code: u16| -> u8 {`

```
// **Die Basiswerte kommen aus `regs.rs`, nicht von Hand.** Erster
// Entwurf hatte hier 0x2d und 0x37 stehen — abgeschrieben aus der
// zweiten, verschobenen Liste in `txpower.rs`, die im selben Zug
// herausgeflogen ist. Beide Zahlen waren um eins zu hoch, und eine
// Rate um eins daneben ist eine ANDERE Rate.
//
// Die Reihen sind lueckenlos, MCS9 liegt neun ueber MCS0
// (main.h:297-317).
//
// `IEEE80211_VHT_MCS_SUPPORT_0_7/0_8/0_9` = 0/1/2; 3 heisst „gar
// nicht". Linux faellt fuer 3 in den `default`-Zweig, also auf MCS9 —
// `rtw_update_sta_info` hat den Strom dann ohnehin schon aus der
// Ratenmaske genommen.
```

## L370-372 · `vht1ss_mcs0 + 9`

```
// `nss == 0` — Linux' dritter Zweig. Er ist hier unerreichbar
// (die efuse meldet 1 oder 2), steht aber da, weil ein Port, der
// einen Zweig weglaesst, nicht mehr nachpruefbar ist.
```

## L377-382 · `pub fn report_seqnum(counter: &mut u8) -> u8 {`

```
/// tx.c:166-177 `rtw_tx_report_enable`.
///
/// **Die Firmware quittiert einen Rahmen nur, wenn man danach fragt.**
/// Die Folgenummer liegt in den Bits 7:2 — die unteren zwei gehoeren
/// der Firmware, die oberen vier des Feldes sind reserviert. Es gibt
/// also 64 unterscheidbare Nummern, und der Zaehler laeuft um.
```

