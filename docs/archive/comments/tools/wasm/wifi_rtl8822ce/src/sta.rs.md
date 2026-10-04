# `tools/wasm/wifi_rtl8822ce/src/sta.rs` @ 5e0102684

## L1-13 · `#![allow(dead_code)]`

```
//! `rtw_sta_info` und `rtw_update_sta_info` — die Ratenanpassung
//! (Stufe 5f), main.c:1117-1240.
//!
//! Der Treiber schickt der Firmware KEINE einzelne Rate, sondern eine
//! MASKE: welche der 64 Raten dieses Gegenueber ueberhaupt kann. Die
//! Firmware waehlt daraus laufend und meldet ihre Wahl als C2H zurueck.
//!
//! **Woher die Maske kommt, ist der ganze Punkt.** In Linux steht sie in
//! `ieee80211_sta`, das mac80211 aus der Anmeldeantwort baut. Bei uns gibt
//! es kein mac80211, also wird die Antwort hier selbst gelesen: HT- und
//! VHT-Element, unterstuetzte Raten. Was dabei NICHT herauskommt, ist
//! geraten — und eine geratene Ratenmaske sendet zu schnell und faellt bei
//! jedem Paket aus.
```

## L18-19 · `#[derive(Default, Clone, Copy)]`

```
/// main.h:775-799 `struct rtw_sta_info` — die Felder, die `send_ra_info`
/// liest. Der Rest ist Linux' Buchfuehrung (Arbeitsschlangen, Mittelwerte).
```

## L32-33 · `pub avg_rssi: crate::dm::Ewma,`

```
/// main.h:760 `DECLARE_EWMA(rssi, 10, 16)` — von `rtw_rx_addr_match`
/// je Rahmen gefuettert, vom Watchdog alle zwei Sekunden gelesen.
```

## L35-37 · `pub ra_report_desc_rate: u8,`

```
/// main.h `si->ra_report.desc_rate` — die Rate, die die FIRMWARE
/// zuletzt gewaehlt hat. Sie kommt als C2H `RA_RPT` herein und ist
/// die Eingabe von `rtw_phy_rrsr_update`.
```

## L41-42 · `#[derive(Default, Clone, Copy)]`

```
/// Was aus der Anmeldeantwort des AP herausfaellt — bei Linux
/// `ieee80211_sta`, von mac80211 gefuellt.
```

## L47 · `pub ht_mcs: [u8; 4],`

```
/// `ht_cap.mcs.rx_mask[0..4]`
```

## L49-50 · `pub ht_ampdu_factor: u8,`

```
/// Byte 2 des HT-CAPABILITIES-Elements, zerlegt: der
/// Laengen-Exponent (Bit 1:0) und der Mindestabstand (Bit 4:2).
```

## L55-56 · `pub vht_mcs_map: u16,`

```
/// `vht_cap.vht_mcs.rx_mcs_map` — was das Gegenueber EMPFANGEN kann.
/// Daraus baut `get_vht_ra_mask` die Sendemaske (main.c:1011).
```

## L58-60 · `pub vht_tx_mcs_map: u16,`

```
/// `vht_cap.vht_mcs.tx_mcs_map` — was es SENDEN kann. Eine andere
/// Karte und eine andere Frage: `get_highest_vht_tx_rate` liest diese
/// (tx.c:132), und ein AP darf sich hier anders eintragen.
```

## L62-63 · `pub supp_rates: u16,`

```
/// Bitmaske der Grundraten, wie `supp_rates[NL80211_BAND_2GHZ]`:
/// Bit 0..3 = CCK 1/2/5,5/11, Bit 4..11 = OFDM 6..54.
```

## L65 · `pub bandwidth: u8,`

```
/// 0 = 20 MHz, 1 = 40, 2 = 80 (`ieee80211_sta.bandwidth`)
```

## L67-76 · `pub ht_op_info: u8,`

```
// ── Die BETRIEBS-Elemente aus der Anmeldeantwort ────────────
//
// **Sie sind maßgeblich, nicht die der Bake.** Linux liest sie hier
// und nirgends sonst: `ieee80211_assoc_success` ruft
// `ieee80211_config_bw(link, elems, ...)` mit den Elementen der
// ANTWORT (mlme.c:7666), und `ieee80211_determine_ap_chan` leitet
// daraus Betriebsart und Breite ab. Wir nahmen beides aus der Bake
// und sahen die Antwort nie an.
/// Byte 1 des HT-Operation-Elements (id 61): Bit 1:0 die Lage des
/// Zweitkanals, Bit 2 „STA Channel Width".
```

## L79-80 · `pub vht_op_chanwidth: u8,`

```
/// Byte 0:2 des VHT-Operation-Elements (id 192) — Breite und die
/// zwei Mittenkanal-Segmente.
```

## L84-85 · `pub vht_op_basic_mcs: u16,`

```
/// Byte 3:4 — die „Basic VHT-MCS and NSS Set". Sie sagt, was eine
/// Station MINDESTENS koennen muss, um in dieser Zelle zu leben.
```

## L90-94 · `pub fn parse_assoc_resp(f: &[u8]) -> PeerCaps {`

```
/// Die Elemente einer Anmeldeantwort lesen.
///
/// **Das ist die obere Haelfte** — in Linux baut mac80211 daraus
/// `ieee80211_sta`. Es steht hier, weil `rtw_update_sta_info` ohne diese
/// Zahlen nichts rechnen kann; `wifid` loest es spaeter ab.
```

## L97 · `let mut i = 30usize;`

```
// 24 Kopf + Faehigkeiten(2) + Status(2) + AID(2) = 30, dann Elemente.
```

## L108-109 · `c.supp_rates |= rate_bit(r & 0x7f);`

```
// Das hohe Bit markiert eine GRUNDrate und gehoert nicht
// zum Wert.
```

## L115-125 · `c.ht_ampdu_factor = b[2] & 0x03; // IEEE80211_HT_AMPDU_PARM_FACTOR`

```
// Byte 2 sind die A-MPDU-Parameter: Bit 1:0 der
// Laengen-Exponent, Bit 4:2 der Mindestabstand. Sie sagen,
// wieviel der AP am Stueck EMPFANGEN kann — also genau die
// zwei Zahlen, die im Sendedeskriptor stehen muessen.
//
// **Zerlegt wird hier und nicht beim Gebrauch**, weil das in
// Linux auch hier geschieht: `ieee80211_ht_cap_ie_to_sta_ht_cap`
// (net/mac80211/ht.c) legt `ampdu_factor` und `ampdu_density`
// getrennt ab, und `get_tx_ampdu_factor` in `tx.c` bekommt sie
// fertig. Wer die Maske in die Treiberfunktion zieht, hat sie
// eine Schicht zu tief.
```

## L126 · `c.ht_ampdu_factor = b[2] & 0x03; // IEEE80211_HT_AMPDU_PARM_FACTOR`

```
// IEEE80211_HT_AMPDU_PARM_FACTOR
```

## L127 · `c.ht_ampdu_density = (b[2] & 0x1c) >> 2; // ..._PARM_DENSITY`

```
// ..._PARM_DENSITY
```

## L128 · `c.ht_mcs.copy_from_slice(&b[3..7]);`

```
// `ht_cap.mcs` beginnt bei Versatz 3 (nach cap und ampdu).
```

## L143 · `c.vht_tx_mcs_map = u16::from_le_bytes([b[8], b[9]]);`

```
// Versatz 8:9 — hinter rx_mcs_map(4:5) und rx_highest(6:7).
```

## L149-151 · `c.bandwidth = if c.vht_supported {`

```
// `ieee80211_sta.bandwidth` — mac80211 rechnet sie aus den
// Faehigkeiten UND der Kanalbreite der Zelle. Ohne die Zelle bleibt
// das, was das Gegenueber kann.
```

## L162-163 · `fn rate_bit(half_mbps: u8) -> u16 {`

```
/// Eine Rate in halben Mbit/s auf ihr Bit in `supp_rates` abbilden.
/// Die Reihenfolge ist die von `ieee80211_rate` im 2,4-GHz-Band.
```

## L166 · `2 => 1 << 0,    // 1 Mbit`

```
// 1 Mbit
```

## L167 · `4 => 1 << 1,    // 2`

```
// 2
```

## L168 · `11 => 1 << 2,   // 5,5`

```
// 5,5
```

## L169 · `22 => 1 << 3,   // 11`

```
// 11
```

## L170 · `12 => 1 << 4,   // 6`

```
// 6
```

## L171 · `18 => 1 << 5,   // 9`

```
// 9
```

## L172 · `24 => 1 << 6,   // 12`

```
// 12
```

## L173 · `36 => 1 << 7,   // 18`

```
// 18
```

## L174 · `48 => 1 << 8,   // 24`

```
// 24
```

## L175 · `72 => 1 << 9,   // 36`

```
// 36
```

## L176 · `96 => 1 << 10,  // 48`

```
// 48
```

## L177 · `108 => 1 << 11, // 54`

```
// 54
```

## L182 · `fn get_vht_ra_mask(mut mcs_map: u16) -> u64 {`

```
/// main.c:1139-1163 `get_vht_ra_mask`
```

## L188 · `2 => ra_mask |= 0x3ffu64 << nss, // MCS9`

```
// MCS9
```

## L189 · `1 => ra_mask |= 0x1ffu64 << nss, // MCS8`

```
// MCS8
```

## L190 · `0 => ra_mask |= 0x0ffu64 << nss, // MCS7`

```
// MCS7
```

## L199 · `fn rate_mask_rssi(rssi_level: u8, wireless_set: u32) -> u64 {`

```
/// main.c:1165-1183 `rtw_rate_mask_rssi`
```

## L214 · `fn rate_mask_recover(mut ra_mask: u64, bak: u64) -> u64 {`

```
/// main.c:1185-1194 `rtw_rate_mask_recover`
```

## L226 · `fn get_rate_id(wireless_set: u32, bw_mode: u8, tx_num: u8) -> u8 {`

```
/// main.c:1240-1310 `get_rate_id`
```

## L281-282 · `}`

```
// Die 3SS- und 4SS-Zweige stehen nicht da: `hw_cap.nss` ist auf
// diesem Chip hoechstens 2, und `tx_num` kommt allein daher.
```

## L285-289 · `pub fn update_sta_info(si: &mut StaInfo, c: &PeerCaps, nss: u8,`

```
/// main.c:1312-1430 `rtw_update_sta_info`, Band 2,4 GHz.
///
/// `rtw_rate_mask_cfg` faellt weg: es greift nur bei `use_cfg_mask`, und
/// das setzt in Linux ein Nutzerbefehl (`cfg80211_bitrate_mask`), den es
/// hier nicht gibt — die Funktion kehrt dann unveraendert um.
```

## L389-391 · `pub fn build_ht_cap_ie(out: &mut [u8], hw_cap_bw: u8, nss: u8) -> usize {`

```
// ════════════════════════════════════════════════════════════════
// Was WIR koennen — und damit auch anbieten muessen
// ════════════════════════════════════════════════════════════════
```

## L393-402 · `pub fn build_ht_cap_ie(out: &mut [u8], hw_cap_bw: u8, nss: u8) -> usize {`

```
/// main.c:1580-1600 `rtw_init_ht_cap`, als fertiges HT-Element
/// (802.11 §9.4.2.55: id 45, 26 Byte Rumpf).
///
/// **Ohne dieses Element im Anmeldeantrag nimmt der AP uns als
/// LEGACY-Station an** — und laesst HT dann auch in seiner Antwort weg.
/// Genau das ist in 0.19.0 passiert: `ra_mask 0x0ff5`, keine MCS-Bits, und
/// die Firmware waehlte OFDM 54M als Bestes, das sie DURFTE.
///
/// `rx_ldpc` und `tx_stbc` sind beim 8822C beide `true`
/// (rtw8822c.c:5391-5392).
```

## L407 · `cap |= IEEE80211_HT_CAP_LDPC_CODING; // rx_ldpc`

```
// rx_ldpc
```

## L408 · `cap |= IEEE80211_HT_CAP_TX_STBC; // tx_stbc`

```
// tx_stbc
```

## L409-429 · `cap |= WLAN_HT_CAP_SM_PS_DISABLED << IEEE80211_HT_CAP_SM_PS_SHIFT;`

```
// **SM Power Save: AUS — und das ist die Zutat, die rtw88 nicht hat.**
//
// `rtw_init_ht_cap` setzt diese zwei Bits NIE, und das ist dort richtig:
// in Linux baut MAC80211 das Element und traegt sie beim Anmeldeantrag
// nach (`mlme.c:1425-1444`, `cap &= ~IEEE80211_HT_CAP_SM_PS` gefolgt vom
// `switch (smps)`; eine gewoehnliche Station steht auf
// `IEEE80211_SMPS_OFF` und bekommt `WLAN_HT_CAP_SM_PS_DISABLED`).
//
// Wir haben kein mac80211. Ungesetzt sind die Bits **null**, und null
// ist nicht „egal", sondern `WLAN_HT_CAP_SM_PS_STATIC`: *ich halte nur
// EINE Empfangskette aktiv*. Ein AP, der sich daran haelt — und sie
// halten sich daran —, schickt ab da nur noch EINEN raeumlichen Strom,
// also hoechstens MCS7.
//
// Am Geraet gemessen (2026-09-21, HomeAP_New auf K7, -40 dBm): wir
// boten `nss 2` an, der AP meldete `MCS ff:ff:00:00` (kann selbst 2),
// unsere Sendemaske trug MCS0-15 und wir SENDETEN mit MCS15 -- und
// empfingen trotzdem MCS7 in 29804 von 35605 Rahmen. 49 Mbit aus
// 72 Mbit brutto sind 68 % Effizienz, der Empfangsweg war also nie das
// Problem. Es fehlte der zweite Strom, und wir hatten ihn selbst
// abbestellt.
```

## L431 · `if hw_cap_bw & (1 << 1) != 0 {`

```
// `hw_cap.bw & BIT(RTW_CHANNEL_WIDTH_40)`
```

## L443 · `b[2] = (IEEE80211_HT_MAX_AMPDU_64K as u8 & 0x3)`

```
// A-MPDU: Faktor in Bit 1:0, Dichte in Bit 4:2.
```

## L446 · `for i in 0..nss.min(4) as usize {`

```
// Supported MCS Set: rx_mask[0..10], rx_highest(2), tx_params(1), Rest 0.
```

## L450 · `b[3 + 4] = 0x01; // mcs.rx_mask[4] = 0x01`

```
// `mcs.rx_mask[4] = 0x01`
```

## L456-461 · `pub fn build_vht_cap_ie(out: &mut [u8], hw_cap_ptcl: u8, nss: u8,`

```
/// main.c:1602-1643 `rtw_init_vht_cap`, als fertiges VHT-Element
/// (802.11 §9.4.2.157: id 191, 12 Byte Rumpf).
///
/// Kehrt um, wenn die efuse etwas anderes als VHT ansagt — dieselbe
/// Bedingung wie in Linux. `bfee_sts_cap` ist 3 (main.c:1905),
/// `rf_path_num > 1` gilt hier.
```

## L479 · `cap |= 3 << IEEE80211_VHT_CAP_BEAMFORMEE_STS_SHIFT; // bfee_sts_cap`

```
// bfee_sts_cap
```

## L480 · `cap |= IEEE80211_VHT_CAP_RXLDPC; // rx_ldpc`

```
// rx_ldpc
```

## L482-500 · `if let Some(ap) = ap_vht_cap {`

```
// ── Und hier stutzt mac80211, was rtw88 gesetzt hat ──────────
//
// `ieee80211_add_vht_ie` (mlme.c:1481-1526). Bis hierher war diese
// Funktion eine treue Portierung von `rtw_init_vht_cap` — und
// genau das war zu wenig: in Linux geht das Ergebnis NICHT so
// hinaus, wie der Treiber es baut. Dazwischen liegt eine Schicht,
// und ihr Kommentar sagt woertlich, wofuer sie da ist:
//
//     Some APs apparently get confused if our capabilities are
//     better than theirs, so restrict what we advertise in the
//     assoc request.
//
// Dasselbe Muster wie bei SM Power Save (0.48.0) und beim
// Duplikatsfilter: die Zutat sitzt eine Schicht UEBER dem Treiber,
// und wer nur den Treiber portiert, liefert ein Element aus, das so
// nie auf der Luft war.
//
// Ohne den Bezugspunkt bleibt alles stehen — ein Suchlauf ohne
// Bake des AP soll nicht anders anbieten als einer mit.
```

## L508-509 · `let ap_sts = ap & IEEE80211_VHT_CAP_BEAMFORMEE_STS_MASK;`

```
// Und die Zahl der Raumzeit-Stroeme, die wir als Beamformee
// annehmen: nie mehr, als der AP zu senden angibt.
```

## L533 · `b[4..6].copy_from_slice(&mcs_map.to_le_bytes()); // rx_mcs_map`

```
// rx_mcs_map
```

## L534 · `b[6..8].copy_from_slice(&highest.to_le_bytes()); // rx_highest`

```
// rx_highest
```

## L535 · `b[8..10].copy_from_slice(&mcs_map.to_le_bytes()); // tx_mcs_map`

```
// tx_mcs_map
```

## L536 · `b[10..12].copy_from_slice(&highest.to_le_bytes()); // tx_highest`

```
// tx_highest
```

## L540-549 · `#[derive(Clone, Copy, Default)]`

```
// ═══════════════════════════════════════════════════════════════════
// Block Ack: die Antwort auf den ADDBA Request des AP
//
// **In rtw88 macht das mac80211, nicht der Treiber** —
// `rtw_ops_ampdu_action` behandelt `IEEE80211_AMPDU_RX_START` mit einem
// leeren `break`. Die Empfangs-Aggregation ist also reine
// 802.11-Verwaltung: wer zustimmt, bekommt Aggregate; die BlockAcks
// darauf erzeugt die HARDWARE, weil sie eine SIFS nach dem Aggregat
// hinaus muessen (16 us) — kein Treiber der Welt schafft das.
// ═══════════════════════════════════════════════════════════════════
```

## L551-552 · `#[derive(Clone, Copy, Default)]`

```
/// Was in einem ADDBA Request steht (802.11 §9.6.7.2,
/// `struct ieee80211_mgmt.u.action.u.addba_req`).
```

## L564-565 · `pub fn parse_addba_req(f: &[u8]) -> Option<AddbaReq> {`

```
/// Den Rahmen lesen. `f` ist der ganze 802.11-Rahmen ab `frame_control`;
/// Kategorie und Aktionscode stehen hinter dem 24 Byte langen Kopf.
```

## L567 · `if f.len() < 24 + 1 + 1 + 1 + 2 + 2 + 2 {`

```
// 24 Kopf + Kategorie + Aktion + Token + capab + timeout + ssn
```

## L586-594 · `pub fn tx_ampdu_factor(ampdu_factor: u8) -> u8 {`

```
/// tx.c:95-105 `get_tx_ampdu_factor` — und der Kommentar dort ist der
/// ganze Grund fuer die Rechnung.
///
/// Im Deskriptor steht **nicht** der Exponent, sondern `MAX_AGG_NUM`, und
/// dessen Wert mal zwei ist die Zahl der Rahmen. Die kleinste
/// A-MPDU-Laenge ist 8 K, also ist die Basis 8/2 = 4.
///
/// Exponent 0..3 ergibt damit 3, 7, 15, 31 — und 31 ist genau der groesste
/// Wert, den das fuenf Bit breite Feld traegt.
```

## L596-597 · `(0x4u8 << ampdu_factor) - 1`

```
// `0x4` und nicht `4`, damit `seqdiff.py` die Zahl gegen Linux'
// `BIT(2)` halten kann — der Zahlenvergleich liest Hexliterale.
```

## L601-602 · `pub fn tx_ampdu_density(ampdu_density: u8) -> u8 {`

```
/// tx.c:107-110 `get_tx_ampdu_density` — Bit 4:2, der Mindestabstand
/// zwischen zwei Rahmen im Aggregat.
```

## L607-611 · `pub const BA_TX_BUF_SIZE: u16 = 64;`

```
/// Die Fensterbreite, die wir ERBITTEN.
///
/// mac80211 nimmt fuer eine Station ohne HE `IEEE80211_MAX_AMPDU_BUF_HT`
/// (64) und schreibt daneben, warum es nicht die Zahl des Treibers ist:
/// manche APs stuerzen bei kleineren Werten ab (agg-tx.c:472-481).
```

## L614-631 · `pub fn build_addba_req(out: &mut [u8; 256], mac: &[u8; 6], bssid: &[u8; 6],`

```
/// Den ADDBA **Request** bauen — `ieee80211_send_addba_request`
/// (net/mac80211/agg-tx.c:61-101), Feld fuer Feld.
///
/// **Das ist die Haelfte, die uns die ganze Zeit gefehlt hat.** Seit
/// 0.29.0 beantworten wir die Bitte des AP und bekommen deshalb
/// Aggregate — gefragt haben wir nie, also sendet jeder unserer Rahmen
/// einzeln. Auf einer schnellen Strecke ist das der teuerste Posten, den
/// es gibt: die Kosten je Sendevorgang sind fast ganz fix (AIFS, Backoff,
/// Praeambel, SIFS, ACK), und die Datenzeit schrumpft mit der Rate.
///
/// `ssn` ist die Folgenummer, ab der die Sitzung zaehlt — sie faehrt um
/// vier Stellen nach links, weil die unteren vier Bit des Feldes die
/// Fragmentnummer sind.
///
/// **`amsdu` steht auf JA, obwohl wir keine A-MSDU bauen.** Linux setzt
/// das Bit bedingungslos; es sagt, was der Absender senden DARF, nicht
/// was er sendet. Wer hier weniger ansagt, bekommt nichts geschenkt und
/// weicht ohne Grund ab.
```

## L638 · `out[4..10].copy_from_slice(bssid); // addr1 = Empfaenger`

```
// addr1 = Empfaenger
```

## L639 · `out[10..16].copy_from_slice(mac); // addr2 = wir`

```
// addr2 = wir
```

## L640 · `out[16..22].copy_from_slice(bssid); // addr3 = BSSID`

```
// addr3 = BSSID
```

## L647 · `| ADDBA_PARAM_POLICY_MASK // 1 = sofortiger Block Ack`

```
// 1 = sofortiger Block Ack
```

## L656-660 · `#[derive(Clone, Copy)]`

```
/// Was in einer ADDBA **Response** steht (802.11 §9.6.7.3).
///
/// Die Reihenfolge ist eine andere als im Request: hier steht der
/// STATUS vor den Faehigkeiten, dort die Folgenummer dahinter. Wer die
/// zwei Rahmen mit einem Parser liest, liest den Status als Fenster.
```

## L671-672 · `pub fn parse_addba_resp(f: &[u8]) -> Option<AddbaResp> {`

```
/// `ieee80211_process_addba_resp` (net/mac80211/agg-tx.c:969-1000), der
/// lesende Teil.
```

## L674 · `if f.len() < 24 + 1 + 1 + 1 + 2 + 2 + 2 {`

```
// 24 Kopf + Kategorie + Aktion + Token + Status + capab + timeout
```

## L692-721 · `pub fn build_addba_resp(out: &mut [u8; 256], mac: &[u8; 6], bssid: &[u8; 6],`

```
/// Die Antwort bauen — `ieee80211_send_addba_resp` (net/mac80211/agg-rx.c)
/// Feld fuer Feld.
///
/// **`buf_size` ist unsere Entscheidung, nicht seine.** Der AP fragt, wie
/// viele Rahmen er offen haben darf; mac80211 antwortet mit
/// `min(erbeten, hw.max_rx_aggregation_subframes)` — und das ist
/// `IEEE80211_MAX_AMPDU_BUF_HT` = 64 (main.c:953), von rtw88 NICHT
/// ueberschrieben.
///
/// **64 ist keine Einstellung, sondern die Decke des Protokolls.** Der
/// komprimierte Block Ack traegt eine Bitmaske von 64 Bit, ein Bit je
/// Rahmen; mehr gaebe es erst mit 802.11ax (`..._BUF_HE` = 256) oder
/// 802.11be (`..._BUF_EHT` = 1024). Der 8822CE ist 802.11ac.
///
/// Hier stand bis 0.52.2 eine kleine Zahl, und die Begruendung darunter
/// war: *„Wir haben keinen Umsortierpuffer."* **Seit 0.49.0 haben wir
/// einen** (`RO_WIN` = 64, derselbe Wert), die Begruendung war also drei
/// Versionen alt und der Deckel blieb stehen. Wer `ampdu:` nicht in
/// seiner Konfig hatte, bekam acht offene Rahmen statt
/// vierundsechzig — und damit hoechstens ein Achtel der Aggregation,
/// die der AP angeboten hat.
///
/// **`amsdu` ist JA, wie in Linux.** mac80211 setzt das Bit aus
/// `SUPPORTS_AMSDU_IN_AMPDU` (agg-rx.c:239, 256), und rtw88 setzt die
/// Fahne fuer den 8822C (`amsdu_in_ampdu = true`, rtw8822c.c:5356;
/// main.c:2271). Hier stand NEIN, weil es keinen Entpacker gab — seit
/// 0.67.0 gibt es ihn (`amsdu_to_8023s`). Ohne A-MSDU traegt ein
/// Sendevorgang hoechstens 64 Pakete, eines je MPDU; mit ihm mehrere je
/// MPDU, und der feste Aufwand je Sendevorgang verteilt sich auf ein
/// Vielfaches.
```

## L727 · `out[4..10].copy_from_slice(bssid); // addr1 = Empfaenger`

```
// addr1 = Empfaenger
```

## L728 · `out[10..16].copy_from_slice(mac); // addr2 = wir`

```
// addr2 = wir
```

## L729 · `out[16..22].copy_from_slice(bssid); // addr3 = BSSID`

```
// addr3 = BSSID
```

## L736 · `let capab = ADDBA_PARAM_AMSDU_MASK`

```
// capab: A-MSDU an, Policy und TID wie erbeten, unsere Fenstergroesse.
```

