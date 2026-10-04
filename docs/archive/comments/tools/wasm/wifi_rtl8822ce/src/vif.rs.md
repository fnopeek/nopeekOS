# `tools/wasm/wifi_rtl8822ce/src/vif.rs` @ 5e0102684

## L1-8 · `#![allow(dead_code)]`

```
//! `rtw_vif_port_config` und der Teil von `rtw_ops_add_interface`, der
//! Hardware anfasst — Stufe 5b.
//!
//! Ohne diese Funktion steht im Port-Register des Chips keine Adresse, und
//! der Empfangsfilter (`WLAN_RCR_CFG` hat `BIT_APM`) laesst dann nur
//! Broadcast und Multicast durch. Eine Probe Response ist an UNSERE
//! Adresse gerichtet — sie kaeme nie an, und das sieht am Geraet genauso
//! aus wie „der AP hat nicht geantwortet".
```

## L14 · `pub struct VifPort {`

```
/// main.h:594-600 `struct rtw_vif_port`, Eintrag 0 aus mac80211.c:108-115.
```

## L23-28 · `pub const PORT0: VifPort = VifPort {`

```
/// mac80211.c:108-115 `rtw_vif_port[0]`.
///
/// Linux fuehrt fuenf Ports und vergibt den ersten freien
/// (`find_first_zero_bit(rtwdev->hw_port, RTW_PORT_NUM)`). Bei EINER
/// Schnittstelle ist das immer die Null; die anderen vier stehen hier
/// nicht, weil kein Rufer sie waehlen kann.
```

## L37 · `#[derive(Default, Clone, Copy)]`

```
/// main.h:812-830 `struct rtw_vif` — die Felder, die der Port traegt.
```

## L49 · `fn write_addr(h: i32, start: u32, addr: &[u8; 6]) {`

```
/// main.c:659-665 `rtw_vif_write_addr` — sechs EINZELNE Bytes.
```

## L56 · `pub fn port_config(h: i32, v: &Vif, config: u32) {`

```
/// main.c:667-694 `rtw_vif_port_config`
```

## L76-93 · `pub fn add_interface_station(h: i32, mac_addr: [u8; 6]) -> Vif {`

```
/// mac80211.c:155-225 `rtw_ops_add_interface`, Zweig
/// `NL80211_IFTYPE_STATION`.
///
/// **Was daneben steht und hier NICHT:**
/// * `rtw_acquire_macid` sucht das erste freie Bit in `mac_id_map` — bei
///   der ersten Schnittstelle ist das die 0, und eine zweite gibt es bei
///   uns nicht. Die Karte selbst waere eine Behauptung ueber Stufe 5c.
/// * `rtw_add_rsvd_page_sta` haengt PS-Poll, Null- und QoS-Null-Rahmen an
///   `rsvd_page_list`. Die LISTE fasst keine Hardware an; sie wird erst von
///   `rtw_fw_download_rsvd_page` geschrieben, und das ruft in Linux
///   `rtw_bss_info_changed` beim Verbinden. Kein Rufer, also kein Code.
/// * `rtw_core_port_switch` kehrt in seiner ERSTEN Zeile um, wenn der Typ
///   nicht AP ist (main.c). Fuer uns ist es kein weggelassener Aufruf,
///   sondern ein Aufruf ohne Wirkung.
/// * `rtw_recalc_lps` gehoert zum Stromsparen, das dieser Treiber nicht
///   fuehrt — `rtw_leave_lps_deep` daneben ebenso.
/// * `rtw_txq_init`, die Statistik und `rtwvif->bfee` sind Zustand der
///   oberen Haelfte.
```

