# `tools/wasm/wifi_rtl8822ce/src/rx.rs` @ 5e0102684

## L1-14 · `#![allow(dead_code)]`

```
//! `rx.c` aus Linux 6.18.26 rtw88, plus `query_phy_status_page0/1` und die
//! dB-Umrechnung aus `phy.c` — Stufe 5a.
//!
//! Portiert: `rtw_rx_query_rx_desc` · `query_phy_status` ·
//! `query_phy_status_page0` · `query_phy_status_page1` ·
//! `rtw_phy_power_2_db` · `rtw_phy_db_2_linear` · `rtw_phy_linear_2_db` ·
//! `rtw_phy_rf_power_2_rssi`.
//!
//! **Nicht portiert und benannt:** `rtw_rx_fill_rx_status` fuellt eine
//! `ieee80211_rx_status` fuer mac80211 — die gibt es hier nicht. Was daraus
//! gebraucht wird (Rate, Bandbreite, Signalstaerke, Kanal), steht in
//! `RxPktStat` und geht von dort an den Netzweg. Ebenso
//! `rtw_phy_parsing_cfo` und die Pfaddiversitaet: beide fuettern die
//! laufende Regelung, die es ohne Verbindung nicht gibt.
```

## L20-21 · `#[derive(Default, Clone, Copy)]`

```
/// main.h:640-670 `struct rtw_rx_pkt_stat` — die Felder, die der
/// Empfangsweg fuellt.
```

## L37 · `pub channel_invalid: bool,`

```
// aus query_phy_status
```

## L43-44 · `pub rx_evm: [u8; 4],`

```
/// main.h:657 — in Linux `u8`, und in `query_phy_status_page1` wird es
/// nach `s8` genommen. Beides steht hier so.
```

## L49-50 · `pub channel: u8,`

```
/// Nicht aus Linux: `rtw_rx_pkt_stat` fuehrt nur `freq`/`band`. Fuer
/// den Bericht ist die Kanalnummer die Zahl, die man lesen will.
```

## L54 · `fn set_rx_freq_band(s: &mut RxPktStat, channel: u8) {`

```
/// main.c:720-730 `rtw_set_rx_freq_band` (+ `IS_CH_*_BAND`, main.h:73-84).
```

## L77 · `fn channel_to_frequency(chan: u8, band: u8) -> u16 {`

```
/// cfg80211 `ieee80211_channel_to_frequency`, in MHz.
```

## L103 · `const W0_PKT_LEN: u32 = 0x3fff; // GENMASK(13, 0)`

```
// rx.h:26-44 — die Felder des Empfangsdeskriptors.
```

## L104 · `const W0_PKT_LEN: u32 = 0x3fff; // GENMASK(13, 0)`

```
// GENMASK(13, 0)
```

## L107 · `const W0_DRV_INFO_SIZE: u32 = 0x000f_0000; // GENMASK(19, 16)`

```
// GENMASK(19, 16)
```

## L108 · `const W0_ENC_TYPE: u32 = 0x0070_0000; // GENMASK(22, 20)`

```
// GENMASK(22, 20)
```

## L109 · `const W0_SHIFT: u32 = 0x0300_0000; // GENMASK(25, 24)`

```
// GENMASK(25, 24)
```

## L112 · `const W1_MACID: u32 = 0x7f; // GENMASK(6, 0)`

```
// GENMASK(6, 0)
```

## L114 · `const W2_PPDU_CNT: u32 = 0x6000_0000; // GENMASK(30, 29)`

```
// GENMASK(30, 29)
```

## L115 · `const W3_RX_RATE: u32 = 0x7f; // GENMASK(6, 0)`

```
// GENMASK(6, 0)
```

## L116 · `const W4_BW: u32 = 0x30; // GENMASK(5, 4)`

```
// GENMASK(5, 4)
```

## L117 · `const RX_DESC_ENC_NONE: u32 = 0;`

```
/// rx.h:9 `RX_DESC_ENC_NONE = 0`
```

## L120-123 · `pub fn query_rx_desc(d: &[u8]) -> RxPktStat {`

```
/// rx.c:264-313 `rtw_rx_query_rx_desc`, nur der Deskriptorteil.
///
/// Reicht, um zu wissen, WIE VIEL zu lesen ist — der PHY-Status liegt
/// dahinter und wird erst in `query_rx_desc_full` ausgewertet.
```

## L133 · `drv_info_sz: (bits(w0, W0_DRV_INFO_SIZE) * 8) as u8,`

```
// „drv_info_sz is in unit of 8-bytes"
```

## L148-150 · `pub fn query_rx_desc_full(d: &[u8], dm: &mut crate::dm::DmInfo,`

```
/// rx.c:120-170, der Rest: PHY-Status auswerten, wenn er da ist.
///
/// `d` muss den ganzen Puffer ab dem Deskriptor enthalten.
```

## L158 · `if s.is_c2h {`

```
// „c2h cmd pkt's rx/phy status is not interested"
```

## L171 · `fn query_phy_status(p: &[u8], s: &mut RxPktStat, dm: &mut crate::dm::DmInfo,`

```
/// rtw8822c.c:2673-2691 `query_phy_status`
```

## L182 · `_ => {}`

```
// Linux: `default: rtw_warn("unused phy status page")`
```

## L187-188 · `const P0_PWDB_A: (usize, u32) = (0x00, 0x0000_ff00); // GENMASK(15, 8)`

```
// rtw8822c.h:143-154 — Seite 0. Die Zahl hinter `+` ist ein WORTindex,
// kein Byteversatz: `*((__le32 *)(phy_stat) + 0x04)`.
```

## L189 · `const P0_PWDB_A: (usize, u32) = (0x00, 0x0000_ff00); // GENMASK(15, 8)`

```
// GENMASK(15, 8)
```

## L190 · `const P0_PWDB_B: (usize, u32) = (0x04, 0x0000_00ff); // GENMASK(7, 0)`

```
// GENMASK(7, 0)
```

## L191 · `const P0_GAIN_A: (usize, u32) = (0x00, 0x003f_0000); // GENMASK(21, 16)`

```
// GENMASK(21, 16)
```

## L192 · `const P0_CHANNEL: (usize, u32) = (0x01, 0x00ff_0000); // GENMASK(23, 16)`

```
// GENMASK(23, 16)
```

## L193 · `const P0_GAIN_B: (usize, u32) = (0x04, 0x3f00_0000); // GENMASK(29, 24)`

```
// GENMASK(29, 24)
```

## L195 · `const P1_PWDB_A: (usize, u32) = (0x00, 0x0000_ff00); // GENMASK(15, 8)`

```
// rtw8822c.h:156-178 — Seite 1.
```

## L196 · `const P1_PWDB_A: (usize, u32) = (0x00, 0x0000_ff00); // GENMASK(15, 8)`

```
// GENMASK(15, 8)
```

## L197 · `const P1_PWDB_B: (usize, u32) = (0x00, 0x00ff_0000); // GENMASK(23, 16)`

```
// GENMASK(23, 16)
```

## L198 · `const P1_L_RXSC: (usize, u32) = (0x01, 0x0000_0f00); // GENMASK(11, 8)`

```
// GENMASK(11, 8)
```

## L199 · `const P1_HT_RXSC: (usize, u32) = (0x01, 0x0000_f000); // GENMASK(15, 12)`

```
// GENMASK(15, 12)
```

## L200 · `const P1_CHANNEL: (usize, u32) = (0x01, 0x00ff_0000); // GENMASK(23, 16)`

```
// GENMASK(23, 16)
```

## L201 · `const P1_RXEVM_A: (usize, u32) = (0x04, 0x0000_00ff); // GENMASK(7, 0)`

```
// GENMASK(7, 0)
```

## L202 · `const P1_RXEVM_B: (usize, u32) = (0x04, 0x0000_ff00); // GENMASK(15, 8)`

```
// GENMASK(15, 8)
```

## L203 · `const P1_CFO_TAIL_A: (usize, u32) = (0x05, 0x0000_00ff); // GENMASK(7, 0)`

```
// GENMASK(7, 0)
```

## L204 · `const P1_CFO_TAIL_B: (usize, u32) = (0x05, 0x0000_ff00); // GENMASK(15, 8)`

```
// GENMASK(15, 8)
```

## L205 · `const P1_RXSNR_A: (usize, u32) = (0x06, 0x0000_00ff); // GENMASK(7, 0)`

```
// GENMASK(7, 0)
```

## L206 · `const P1_RXSNR_B: (usize, u32) = (0x06, 0x0000_ff00); // GENMASK(15, 8)`

```
// GENMASK(15, 8)
```

## L212 · `const DESC_RATE11M: u8 = 0x03;`

```
/// main.h:44-47 `DESC_RATE11M` = 3, `DESC_RATEMCS0` = 12.
```

## L216 · `fn query_phy_status_page0(p: &[u8], s: &mut RxPktStat,`

```
/// rtw8822c.c:2548-2596 `query_phy_status_page0` — CCK.
```

## L224-226 · `let l_bnd = dm.cck_gi_l_bnd;`

```
// Die beiden Grenzen gehoeren dem TREIBER: `rtw8822c_phy_set_param`
// liest sie einmal aus der Hardware (Stufe 3c), hier werden sie nur
// angewandt. Am Geraet l/u = 16/63.
```

## L260-263 · `for path in 0..=rf_path_num as usize {`

```
// `path <= rf_path_num` steht so in Linux — bei zwei Pfaden laeuft die
// Schleife DREIMAL und schreibt `rssi[2]`. Das Feld hat vier Plaetze,
// also ist es folgenlos; abgeschrieben wird es trotzdem, weil eine
// stillschweigend korrigierte Schleife kein 1:1-Port mehr ist.
```

## L273 · `fn query_phy_status_page1(p: &[u8], s: &mut RxPktStat,`

```
/// rtw8822c.c:2598-2671 `query_phy_status_page1` — OFDM/HT/VHT.
```

## L297-299 · `let channel = stat(p, P1_CHANNEL) as u8;`

```
// Ohne `if channel != 0` — Linux prueft das auf Seite 1 nicht, und
// `set_rx_freq_band` kehrt bei einer Zahl ausserhalb beider Baender
// von selbst um.
```

## L322 · `for path in 0..=rf_path_num as usize {`

```
// Dieselbe `<=`-Schleife wie auf Seite 0.
```

## L346-348 · `}`

```
// `rtw_phy_parsing_cfo` braucht die angemeldeten Schnittstellen einer
// Verbindung (`rtw_iterate_vifs_atomic`). Ohne Verbindung gibt es
// keine, der Aufruf waere in Linux hier ein Leerlauf.
```

## L357 · `const FRAC_BITS: u32 = 3;`

```
// ── Die dB-Umrechnung (phy.c:128-235) ────────────────────────────
```

## L359 · `const FRAC_BITS: u32 = 3;`

```
/// phy.c `FRAC_BITS`
```

## L362 · `fn power_2_db(power: i8) -> u8 {`

```
/// phy.c:826-834 `rtw_phy_power_2_db`
```

## L373 · `fn db_2_linear(power_db: u8) -> u64 {`

```
/// phy.c:836-854 `rtw_phy_db_2_linear`
```

## L385 · `fn linear_2_db(linear: u64) -> u8 {`

```
/// phy.c:856-901 `rtw_phy_linear_2_db`
```

## L406 · `return 96; // maximum 96 dB`

```
// maximum 96 dB
```

## L426 · `pub fn rf_power_2_rssi(rf_power: &[i8], path_num: u8) -> u8 {`

```
/// phy.c:903-934 `rtw_phy_rf_power_2_rssi`
```

## L445-453 · `pub fn update_rx_freq_for_invalid(s: &mut RxPktStat, current_channel: u8,`

```
/// rx.h:56-62 `rtw_update_rx_freq_for_invalid` +
/// rx.c:155-193 `rtw_update_rx_freq_from_ie`.
///
/// Ein CCK-Rahmen kann mit Kanal 0 kommen (`query_phy_status_page0` setzt
/// dann `channel_invalid`). Linux nimmt dann den LAUFENDEN Kanal — und
/// liest die Kanalnummer nur dann aus dem Beacon, wenn gerade GESUCHT
/// wird, weil nur beim Suchen ein Rahmen von einem anderen Kanal
/// hereinkommen kann. Ohne Suche ist der Zweig von `RTW_FLAG_SCANNING`
/// tot, und deshalb steht hier nur seine Wirkung.
```

## L460-463 · `}`

```
// `cfg80211_get_ies_channel_number` auf dem DS-Parameter-Set des
// Beacons. BENANNT UND NICHT GEBAUT: es gibt noch keine Suche
// (Stufe 5c). Bis dahin waere ein Parser fuer Informationselemente
// Code ohne Rufer.
```

## L468-475 · `#[derive(Clone, Copy)]`

```
// ═══════════════════════════════════════════════════════════════════
// Was der Empfangsweg dem Watchdog zutraegt (rx.c:42-133, phy.c:678-704)
//
// Ohne diese vier Zeilen rechnet `rtw_watch_dog_work` auf Nullen: der
// Frequenzversatz wird NICHT aufsummiert, die Ratenzaehler bleiben leer,
// und `min_rssi` ist 255. Die Nachfuehrung tut dann nichts und meldet
// auch nichts — der schlimmste Zustand von allen.
// ═══════════════════════════════════════════════════════════════════
```

## L477-482 · `#[derive(Clone, Copy)]`

```
/// Was EIN Ringdurchlauf dem Watchdog zutraegt.
///
/// **Im Rueckruf gesammelt, danach eingetragen.** Waehrend `rx_poll`
/// laeuft, haelt es `DmInfo` selbst (es schreibt den PHY-Status hinein);
/// zwei Schreiber auf denselben Zustand gibt es nicht, und ein roher
/// Zeiger waere hier eine Umgehung des Ausleihers statt einer Loesung.
```

## L492 · `pub ra_rpt: Option<(u8, u8)>,`

```
/// Der Ratenbericht der Firmware: `(rate, mac_id)`.
```

## L494 · `pub tx_rpt: [(u8, bool); 8],`

```
/// Die Sendequittungen dieses Durchlaufs: `(Folgenummer, quittiert)`.
```

## L497-498 · `pub c2h_seen: [u8; 8],`

```
/// Die Kennungen der C2H, die wir NICHT behandeln — gezaehlt statt
/// verworfen.
```

## L501-502 · `pub mgmt: [(u8, (u8, u8)); 8],`

```
/// Verwaltungsrahmen dieses Durchlaufs: `(Subtyp, (Kategorie,
/// Aktion))`, `0xff` wo es keine Aktion gibt.
```

## L505-513 · `pub addba: [crate::sta::AddbaReq; 4],`

```
/// Die ADDBA Requests dieses Durchlaufs — beantwortet werden sie
/// draussen, mit freiem `trx`.
///
/// **Es war EINER, und das war zu wenig.** Am Geraet stand
/// `ADDBA 14 erbeten, 8 angenommen`: ein Ringdurchlauf bringt
/// mehrere Rahmen auf einmal, und alles nach dem ersten fiel weg.
/// Der AP wiederholt zwar, aber jede Wiederholung ist eine
/// Sendegelegenheit, in der er NICHT aggregiert — und bis zur
/// Antwort bleibt seine Sitzung zu.
```

## L516-519 · `pub heard_ap: bool,`

```
/// **Haben wir in diesem Durchlauf ueberhaupt etwas vom AP
/// gehoert?** mlme.c:131-145 `ieee80211_sta_reset_conn_monitor`:
/// jeder Rahmen von ihm setzt die Wache zurueck, nicht nur eine
/// Bake. Ein Download ohne Baken ist eine lebende Verbindung.
```

## L521-523 · `pub beacon_dbm: Option<i8>,`

```
/// Der Pegel der letzten Bake DIESER Zelle, in dBm. Er fuettert den
/// geglaetteten Wert, an dem das Roaming haengt
/// (`ieee80211_handle_beacon_sig`).
```

## L525-526 · `pub beacon_ohne_csa: bool,`

```
/// Kam in diesem Durchlauf eine Bake DIESER Zelle OHNE Ansage?
/// **Das bricht einen angekuendigten Wechsel ab** (mlme.c:2822).
```

## L528-530 · `pub csa: Option<crate::Csa>,`

```
/// Die Wechselansage aus einer Bake dieser Zelle, wenn eine da war.
/// Sie faehrt heraus, weil der Kanalwechsel `trx` braucht und der
/// Rueckruf es nicht halten darf.
```

## L532-533 · `pub addba_drop: u32,`

```
/// Und was auch in vier Plaetze nicht passte. Eine Zahl, damit ein
/// zu kleiner Puffer nicht wieder still kuerzt.
```

## L535-537 · `pub addba_resp: Option<crate::sta::AddbaResp>,`

```
/// Und die Antwort auf UNSERE Frage. Sie faehrt denselben Weg, aus
/// demselben Grund: der Zustandswechsel gehoert nach dem Ringleeren
/// hin, wo `link` veraenderlich ist.
```

## L539 · `pub rx_unicast: u64,`

```
/// rx.c:14-32 `rtw_rx_stats` — Bytes und Rahmen, nur Unicast.
```

## L542-545 · `pub bw_cnt: [u32; 4],`

```
/// Die BREITE, in der die Rahmen dieses Durchlaufs hereinkamen:
/// 20/40/80 und ein vierter Platz fuer alles andere. Sie kommt aus
/// dem Empfangsstatus des Chips, ist also eine Messung und keine
/// Einstellung.
```

## L566 · `pub fn merge(&self, dm: &mut crate::dm::DmInfo,`

```
/// Nach dem Ringdurchlauf in den langlebigen Zustand eintragen.
```

## L589-590 · `pub fn hdr_bssid(f: &[u8]) -> Option<[u8; 6]> {`

```
/// util.h:28-41 `get_hdr_bssid` — welche der drei Adressen die BSSID ist,
/// haengt an den zwei DS-Bits.
```

## L603 · `fn is_ctl(f: &[u8]) -> bool {`

```
/// `ieee80211_is_ctl` — Typ 01 im ersten Byte.
```

## L608 · `fn is_beacon(f: &[u8]) -> bool {`

```
/// `ieee80211_is_beacon` — Verwaltung, Subtyp 8.
```

## L613-618 · `pub fn watchdog_feed(a: &mut WdAcc, st: &RxPktStat, f: &[u8],`

```
/// rx.c:100-133 `rtw_rx_addr_match` + `_iter`, und phy.c:690-704 in
/// EINEM Gang: beide laufen in Linux ueber dieselbe Adressprobe, nur aus
/// zwei Rufstellen.
///
/// `our_mac`/`bssid` ersetzen den vif-Iterator — wir fahren genau eine
/// Schnittstelle, und mehr als eine waere hier eine Erfindung.
```

## L629 · `for i in 0..path_num as usize {`

```
// phy.c: der CFO-Zweig prueft NUR die BSSID.
```

## L636-637 · `if &f[4..10] != our_mac && !is_beacon(f) {`

```
// rx.c: der Statistikzweig verlangt zusaetzlich, dass der Rahmen an
// UNS gerichtet ist — oder ein Beacon.
```

## L641 · `a.curr_rx_rate = st.rate;`

```
// rx.c:42-99 `rtw_rx_phy_stat`
```

## L651-652 · `if f[10..16] == bssid[..] {`

```
// `ewma_rssi_add(&si->avg_rssi, pkt_stat->rssi)` — nur, wenn der
// Sender die bekannte Station ist.
```

## L659-670 · `pub fn mgmt_census(f: &[u8], bssid: &[u8; 6]) -> Option<(u8, Option<(u8, u8)>)> {`

```
/// **Der Zensus der Verwaltungsrahmen: zaehlen, was wir verwerfen.**
///
/// Dieselbe Regel, die der C2H-Zensus gerade bewiesen hat. Die Frage
/// dahinter ist konkret: **versucht der AP ueberhaupt, eine Aggregation
/// aufzubauen?** Er tut das mit einem Action-Rahmen (Kategorie 3,
/// Aktion 0 = ADDBA Request), und wir verwerfen bis heute jeden
/// Verwaltungsrahmen ausser Deauth und Disassoc. Kommt keiner, ist der
/// Durchsatzdeckel woanders; kommt einer, ist die Antwort darauf der
/// naechste Posten.
///
/// Gibt den Subtyp zurueck (0..15), und bei einem Action-Rahmen
/// zusaetzlich `(Kategorie, Aktion)`.
```

## L679-680 · `let act = if subtype == 13 && f.len() >= 26 {`

```
// Action = Subtyp 13; Kategorie und Aktion stehen gleich hinter dem
// 24 Byte langen Kopf.
```

