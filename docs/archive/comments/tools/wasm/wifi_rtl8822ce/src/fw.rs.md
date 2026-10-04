# `tools/wasm/wifi_rtl8822ce/src/fw.rs` @ 5e0102684

## L1-3 · `#![allow(dead_code)]`

```
//! `fw.c` aus Linux 6.18.26 rtw88 — der Teil, den der Download braucht.
//!
//! Portiert: `rtw_fw_write_data_rsvd_page` (der 3081-Zweig).
```

## L10-19 · `pub fn dump_pci_cmd(tag: &str) {`

```
/// PCI-Konfigurationsraum: Kommando und Status.
///
/// Das Kommandoregister sagt, ob der Chip ueberhaupt Busmaster ist — ohne
/// das kann er keinen Deskriptor aus dem Hauptspeicher holen, und genau so
/// sieht es aus: MMIO geht, DMA nicht.
///
/// Das STATUSregister ist die zweite Haelfte der Frage. Bit 13 (Received
/// Master Abort) und Bit 12 (Received Target Abort) stehen, wenn der Chip
/// es VERSUCHT hat und abgewiesen wurde. Bleiben sie leer und Busmaster ist
/// an, hat er gar nicht erst hingesehen. Das sind zwei verschiedene Fehler.
```

## L58-59 · `pub fn check_hw_ready(h: i32, addr: u32, mask: u32, target: u32) -> bool {`

```
/// util.c `check_hw_ready`: 1000 Runden, 10 us auseinander, und gelesen wird
/// mit `rtw_read32_mask` — also 32 Bit, auch wenn das Register ein Byte ist.
```

## L64-65 · `pub const LINUX_FRIST_US: u64 = 1000 * 10;`

```
/// Linux: 1000 Runden mit je `udelay(10)` — die Frist ist also **10 ms**,
/// und die Rundenzahl ist nur die Art, wie sie dort gezaehlt wird.
```

## L68-76 · `pub fn check_hw_ready_for(`

```
/// util.c `check_hw_ready`, aber an der UHR statt an der Rundenzahl.
///
/// Die erste Fassung hielt 1000 Runden ohne Pause — das sind hier eine bis
/// zwei Millisekunden statt zehn, also ein Zehntel von Linux' Frist. Der
/// Kommentar daneben behauptete schon die Uhr; der Code tat etwas anderes,
/// und die Firmware bekam zu wenig Zeit, ihr FW_INIT_RDY zu setzen.
///
/// Gibt zurueck, ob es geklappt hat UND wie lange es gedauert hat — die
/// zweite Zahl ist der Unterschied zwischen „zu knapp" und „kommt nie".
```

## L90-92 · `if waited > 10_000 {`

```
// 10 us liegen unter unserer Schlafaufloesung, also wird eng
// gelesen. Ab 10 ms wird zwischen den Lesungen abgegeben, damit
// eine lange Frist nicht den Kern blockiert.
```

## L99-103 · `#[allow(clippy::too_many_arguments)]`

```
/// fw.c `rtw_fw_write_data_rsvd_page`, 3081-Zweig, PCIe.
///
/// `rsvd_boundary` ist beim Firmware-Download noch 0: es wird erst von
/// `rtw_set_trx_fifo_info` gesetzt, und das laeuft in `rtw_mac_init` — also
/// NACH dem Download. Linux schreibt hier also ebenfalls eine 0 zurueck.
```

## L123 · `let bckp1 = host::r8(h, REG_FWHW_TXQ_CTRL + 2);`

```
// PCIe: Beacon-Download der Queue abschalten, solange wir sie benutzen.
```

## L127 · `if verbose {`

```
// `rtw_hci_write_data_rsvd_page` -> `rtw_pci_write_data_rsvd_page`
```

## L155-157 · `host::print("    BCN_WORK  @0x383 = 0x");`

```
// 0x382 ist NICHT vierfach ausgerichtet; der 32-Bit-Lesezugriff
// im letzten Lauf gab 0xffffffff und war damit mein eigener
// Messfehler, nicht die Antwort des Chips.
```

## L178 · `host::w16(h, REG_FIFOPAGE_CTRL_2, rsvd_boundary | (BIT_BCN_VALID_V1 as u16));`

```
// restore
```

## L187-201 · `#[derive(Default, Clone, Copy)]`

```
// ════════════════════════════════════════════════════════════════
// Stufe 4a: die zwei H2C-Wege
// ════════════════════════════════════════════════════════════════
//
// Sie sehen gleich aus und sind es nicht:
//
//   `rtw_fw_send_h2c_command`  schreibt ACHT Byte in eines von vier
//                              HMEBOX-Registern. Die Koexistenz redet so.
//   `rtw_fw_send_h2c_packet`   schiebt ZWEIUNDDREISSIG Byte durch die
//                              H2C-QUEUE, also durch den Ring, dessen
//                              Adresse `init_h2c` gesetzt hat. General-
//                              und PHYDM-Info gehen so.
//
// Wer den einen fuer den anderen haelt, schickt alles ins Leere — und
// merkt es nicht, weil beide Wege stumm sind.
```

## L203-205 · `#[derive(Default, Clone, Copy)]`

```
/// `struct rtw_h2c_cmd` (fw.h) — acht Byte, als zwei Woerter.
/// Der Zustand zwischen zwei Kommandos: welches Postfach als naechstes
/// drankommt und welche Folgenummer ein PAKET traegt.
```

## L207-214 · `pub struct H2cState {`

```
/// `struct rtw_dev.h2c` (main.h) — **EINER fuer das ganze Geraet.**
///
/// Bis 0.17.0 legte jede Stufe einen eigenen an. Damit fing Stufe 5d
/// wieder bei Fach 0 an, obwohl 5c es zuletzt beschrieben hatte, und
/// `seq` lief mehrfach von null los. In Linux gibt es genau ein
/// `rtwdev->h2c`, und die Reihenfolge der Faecher ist der ganze Sinn:
/// der Treiber reicht sie im Kreis weiter, damit die Firmware Zeit hat,
/// das vorige zu leeren.
```

## L220 · `pub fn hmetfr(h: i32) -> u8 {`

```
/// Die vier Fachfahnen, wie der Chip sie gerade meldet.
```

## L225-226 · `fn h2c_set(pkt: &mut [u8; H2C_PKT_SIZE], word: usize, mask: u32, value: u32) {`

```
/// Feld an seine Schiebestelle, im Wort `word` des H2C-Puffers.
/// Das ist `le32p_replace_bits((__le32 *)(h2c) + word, value, mask)`.
```

## L234 · `pub fn send_h2c_command(h: i32, st: &mut H2cState, pkt: &[u8; H2C_PKT_SIZE]) -> bool {`

```
// ── Weg 1: die MAILBOX (fw.c:76-127) ─────────────────────────────
```

## L236-241 · `pub fn send_h2c_command(h: i32, st: &mut H2cState, pkt: &[u8; H2C_PKT_SIZE]) -> bool {`

```
/// fw.c `rtw_fw_send_h2c_command`.
///
/// Linux pollt mit `read_poll_timeout_atomic(rtw_read8, ..., 100, 3000, ...)`
/// — alle 100 us, Frist **3 ms**. Gehalten wird hier die Frist, nicht die
/// Rundenzahl (derselbe Fehler wie in `check_hw_ready` soll sich nicht
/// wiederholen).
```

## L262-264 · `host::print("[rtl8822ce] failed to send h2c command (Fach ");`

```
// Linux sagt nur „failed to send h2c command". Welches Fach und
// welche Fahnen — das ist der Unterschied zwischen einer
// Meldung und einer Diagnose.
```

## L275-276 · `let msg = u32::from_le_bytes([pkt[0], pkt[1], pkt[2], pkt[3]]);`

```
// `h2c_cmd->msg` sind die Bytes 0..4, `msg_ext` die Bytes 4..8 —
// und das EX-Register wird ZUERST geschrieben.
```

## L289 · `fn set_cmd_id_class(pkt: &mut [u8; H2C_PKT_SIZE], value: u32) {`

```
/// fw.h:586 `SET_H2C_CMD_ID_CLASS`
```

## L294 · `pub fn bt_wifi_control(h: i32, st: &mut H2cState, op_code: u8, data: &[u8; 5]) -> bool {`

```
/// fw.c `rtw_fw_bt_wifi_control` — fw.h:568-574, Kommando 0x69.
```

## L298 · `h2c_set(&mut pkt, 0, 0x0000_ff00, op_code as u32); // OP_CODE`

```
// OP_CODE
```

## L299 · `h2c_set(&mut pkt, 0, 0x00ff_0000, data[0] as u32); // DATA1`

```
// DATA1
```

## L300 · `h2c_set(&mut pkt, 0, 0xff00_0000, data[1] as u32); // DATA2`

```
// DATA2
```

## L301 · `h2c_set(&mut pkt, 1, 0x0000_00ff, data[2] as u32); // DATA3`

```
// DATA3
```

## L302 · `h2c_set(&mut pkt, 1, 0x0000_ff00, data[3] as u32); // DATA4`

```
// DATA4
```

## L303 · `h2c_set(&mut pkt, 1, 0x00ff_0000, data[4] as u32); // DATA5`

```
// DATA5
```

## L307 · `pub fn query_bt_info(h: i32, st: &mut H2cState) -> bool {`

```
/// fw.c `rtw_fw_query_bt_info` — Kommando 0x61.
```

## L311 · `h2c_set(&mut pkt, 0, 1 << 8, 1); // SET_QUERY_BT_INFO(h2c_pkt, true)`

```
// SET_QUERY_BT_INFO(h2c_pkt, true)
```

## L315 · `fn h2c_pkt_set_header(pkt: &mut [u8; H2C_PKT_SIZE], sub_id: u32) {`

```
// ── Weg 2: das PAKET durch die H2C-Queue (fw.c:495-565) ──────────
```

## L317 · `fn h2c_pkt_set_header(pkt: &mut [u8; H2C_PKT_SIZE], sub_id: u32) {`

```
/// fw.c `rtw_h2c_pkt_set_header`
```

## L319 · `h2c_set(pkt, 0, 0x0000_007f, H2C_PKT_CATEGORY); // SET_PKT_H2C_CATEGORY`

```
// SET_PKT_H2C_CATEGORY
```

## L320 · `h2c_set(pkt, 0, 0x0000_ff00, H2C_PKT_CMD_ID); // SET_PKT_H2C_CMD_ID`

```
// SET_PKT_H2C_CMD_ID
```

## L321 · `h2c_set(pkt, 0, 0xffff_0000, sub_id); // SET_PKT_H2C_SUB_CMD_ID`

```
// SET_PKT_H2C_SUB_CMD_ID
```

## L324 · `fn send_h2c_packet(h: i32, trx: &mut Trx, stage: i32, st: &mut H2cState,`

```
/// fw.c `rtw_fw_send_h2c_packet`
```

## L327 · `h2c_set(pkt, 1, 0xffff_0000, st.seq as u32); // FW_OFFLOAD_H2C_SET_SEQ_NUM`

```
// FW_OFFLOAD_H2C_SET_SEQ_NUM
```

## L332 · `st.seq = st.seq.wrapping_add(1);`

```
// Linux erhoeht `seq` auch bei Fehlschlag.
```

## L337-340 · `pub fn send_general_info(h: i32, trx: &mut Trx, stage: i32, st: &mut H2cState,`

```
/// fw.c:517-535 `rtw_fw_send_general_info`.
///
/// Sagt der Firmware, wieviele Seiten sie hinter `rsvd_boundary` fuer ihren
/// eigenen Sendepuffer hat. Bei uns: 1994 − 1938 = **56**.
```

## L347 · `h2c_set(&mut pkt, 1, 0x0000_ffff, total_size as u32); // SET_PKT_H2C_TOTAL_LEN`

```
// SET_PKT_H2C_TOTAL_LEN
```

## L348 · `h2c_set(&mut pkt, 2, 0x00ff_0000, // GENERAL_INFO_SET_FW_TX_BOUNDARY`

```
// GENERAL_INFO_SET_FW_TX_BOUNDARY
```

## L354-357 · `#[allow(clippy::too_many_arguments)]`

```
/// fw.c:538-565 `rtw_fw_send_phydm_info`.
///
/// `rx_ant_status`/`tx_ant_status` kommen aus `hal->antenna_rx`/`antenna_tx`
/// — bei 2T2R beide `BB_PATH_AB`.
```

## L368 · `h2c_set(&mut pkt, 2, 0x0000_00ff, rfe_option as u32); // REF_TYPE`

```
// REF_TYPE
```

## L369 · `h2c_set(&mut pkt, 2, 0x0000_ff00, fw_rf_type as u32); // RF_TYPE`

```
// RF_TYPE
```

## L370 · `h2c_set(&mut pkt, 2, 0x00ff_0000, cut_version as u32); // CUT_VER`

```
// CUT_VER
```

## L371 · `h2c_set(&mut pkt, 2, 0x0f00_0000, antenna_rx as u32); // RX_ANT_STATUS`

```
// RX_ANT_STATUS
```

## L372 · `h2c_set(&mut pkt, 2, 0xf000_0000, antenna_tx as u32); // TX_ANT_STATUS`

```
// TX_ANT_STATUS
```

## L377 · `#[allow(clippy::too_many_arguments)]`

```
/// fw.c `rtw_fw_coex_tdma_type` — Mailbox-Kommando 0x60, fw.h:567.
```

## L383 · `h2c_set(&mut pkt, 0, 0x0000_ff00, para1 as u32); // PARA1`

```
// PARA1
```

## L384 · `h2c_set(&mut pkt, 0, 0x00ff_0000, para2 as u32); // PARA2`

```
// PARA2
```

## L385 · `h2c_set(&mut pkt, 0, 0xff00_0000, para3 as u32); // PARA3`

```
// PARA3
```

## L386 · `h2c_set(&mut pkt, 1, 0x0000_00ff, para4 as u32); // PARA4`

```
// PARA4
```

## L387 · `h2c_set(&mut pkt, 1, 0x0000_ff00, para5 as u32); // PARA5`

```
// PARA5
```

## L391 · `pub fn scan_notify(h: i32, st: &mut H2cState, start: bool) -> bool {`

```
/// fw.c:1080-1088 `rtw_fw_scan_notify` — Kommando 0x59.
```

## L395 · `h2c_set(&mut pkt, 0, 1 << 8, start as u32); // SET_SCAN_START`

```
// SET_SCAN_START
```

## L399 · `pub fn inform_rfk_status(h: i32, st: &mut H2cState, start: bool) -> bool {`

```
/// fw.c:437-446 `rtw_fw_inform_rfk_status` — Kommando 0x6d, Postfach.
```

## L403 · `h2c_set(&mut pkt, 0, 1 << 8, start as u32); // RFK_SET_INFORM_START`

```
// RFK_SET_INFORM_START
```

## L407-410 · `pub fn do_iqk(h: i32, trx: &mut Trx, stage: i32, st: &mut H2cState,`

```
/// fw.c:448-459 `rtw_fw_do_iqk` — der QUEUE-Weg, nicht das Postfach.
///
/// Die IQK rechnet die FIRMWARE; der Treiber stoesst sie nur an und wartet
/// danach auf `REG_RPT_CIP`.
```

## L416 · `h2c_set(&mut pkt, 1, 0x0000_ffff, total_size as u32); // SET_PKT_H2C_TOTAL_LEN`

```
// SET_PKT_H2C_TOTAL_LEN
```

## L417 · `h2c_set(&mut pkt, 2, 1 << 0, clear as u32); // IQK_SET_CLEAR`

```
// IQK_SET_CLEAR
```

## L418 · `h2c_set(&mut pkt, 2, 1 << 1, segment_iqk as u32); // IQK_SET_SEGMENT_IQK`

```
// IQK_SET_SEGMENT_IQK
```

## L422-425 · `pub fn media_status_report(h: i32, st: &mut H2cState, mac_id: u8,`

```
/// fw.c:1123-1132 `rtw_fw_media_status_report` — Kommando 0x01, Postfach.
///
/// Sagt der Firmware, dass diese `mac_id` verbunden ist. Sie richtet
/// daraufhin ihre eigene Buchfuehrung ein (Ratenanpassung, Stromsparen).
```

## L430 · `h2c_set(&mut pkt, 0, 1 << 8, connect as u32); // SET_OP_MODE`

```
// SET_OP_MODE
```

## L431 · `h2c_set(&mut pkt, 0, 0x00ff_0000, mac_id as u32); // SET_MACID`

```
// SET_MACID
```

## L435 · `pub struct C2hCmd<'a> {`

```
/// fw.h:73-77 `struct rtw_c2h_cmd` — Kennung, Folgenummer, Nutzlast.
```

## L442-450 · `pub fn c2h_name(id: u8) -> &'static str {`

```
/// fw.c:334-380 `rtw_fw_c2h_cmd_handle`, der Verteiler.
///
/// **Was hinter den Kennungen liegt, ist noch nicht gebaut** — und das
/// steht hier namentlich statt als stiller `_ =>`. Jede dieser Zeilen ist
/// ein eigener Posten: `rtw_tx_report_handle` braucht die Sendequittungen,
/// `rtw_coex_bt_info_notify` die laufende Koexistenz,
/// `rtw_fw_ra_report_handle` die Ratenanpassung. Gemeldet wird jede
/// Nachricht trotzdem, denn eine Firmware, die etwas sagt, sagt es aus
/// einem Grund.
```

## L452-454 · `const NAMES: &[(u32, &str)] = &[`

```
// Eine Tabelle statt `match`: die Kennungen stehen in `regs.rs` teils
// als `u8` (weil jemand sie in ein Byteregister schreibt) und teils
// als `u32`, und ein Muster darf keine Umwandlung tragen.
```

## L478-479 · `pub fn c2h_parse(frame: &[u8]) -> Option<C2hCmd<'_>> {`

```
/// Den C2H-Kopf aus einem Empfangspuffer ziehen. `pkt_offset` ist in Linux
/// derselbe Versatz wie beim Funkrahmen: Deskriptor + drv_info + shift.
```

## L487-495 · `pub fn send_ra_info(h: i32, st: &mut H2cState, si: &mut crate::sta::StaInfo,`

```
/// fw.c:1023-1063 `rtw_fw_send_ra_info` — Kommando 0x40, Postfach.
///
/// Der Treiber schickt eine MASKE, keine Rate: welche der 64 Raten dieses
/// Gegenueber kann. Die Firmware waehlt daraus laufend und meldet ihre
/// Wahl als `C2H_RA_RPT` zurueck.
///
/// Der `H2C_CMD_RA_INFO_HI`-Teil daneben gilt nur fuer den 8814A (vier
/// Sendeketten, Maske breiter als 32 Bit); `chip->id` ist hier 8822C, und
/// Linux kehrt an derselben Stelle um.
```

## L501 · `h2c_set(&mut pkt, 0, 0x0000_ff00, si.mac_id as u32); // MACID`

```
// MACID
```

## L502 · `h2c_set(&mut pkt, 0, 0x001f_0000, si.rate_id as u32); // RATE_ID`

```
// RATE_ID
```

## L503 · `h2c_set(&mut pkt, 0, 0x0060_0000, si.init_ra_lv as u32); // INIT_RA_LVL`

```
// INIT_RA_LVL
```

## L504 · `h2c_set(&mut pkt, 0, 1 << 23, si.sgi_enable as u32); // SGI_EN`

```
// SGI_EN
```

## L505 · `h2c_set(&mut pkt, 0, 0x0300_0000, si.bw_mode as u32); // BW_MODE`

```
// BW_MODE
```

## L506 · `h2c_set(&mut pkt, 0, 1 << 26, (si.ldpc_en != 0) as u32); // LDPC`

```
// LDPC
```

## L507 · `h2c_set(&mut pkt, 0, 1 << 27, (!reset_ra_mask) as u32); // NO_UPDATE`

```
// NO_UPDATE
```

## L508-512 · `h2c_set(&mut pkt, 0, 0x3000_0000, si.vht_enable as u32); // VHT_EN`

```
// GENMASK(29, 28) — ZWEI Bit. Bei einem `bool` schreibt eine
// Einzelbitmaske denselben Wert, loescht aber Bit 29 nicht. Hier ist
// das folgenlos (der Puffer beginnt bei null), und trotzdem steht die
// Maske der Quelle da: wer spaeter einen Wert > 1 setzt, bekaeme mit
// der falschen Maske stillschweigend etwas anderes.
```

## L513 · `h2c_set(&mut pkt, 0, 0x3000_0000, si.vht_enable as u32); // VHT_EN`

```
// VHT_EN
```

## L514 · `h2c_set(&mut pkt, 0, 1 << 30, 1); // DIS_PT — Linux: disable_pt = true`

```
// DIS_PT — Linux: `disable_pt = true`
```

## L524-527 · `pub fn default_port(h: i32, st: &mut H2cState, port: u8, mac_id: u8,`

```
/// fw.c:1065-1080 `rtw_fw_default_port`.
///
/// Kehrt um, solange der Port nicht verbunden ist — das ist keine
/// Abkuerzung, es steht so in der ersten Zeile.
```

## L540-542 · `pub fn send_rssi_info(h: i32, st: &mut H2cState,`

```
// ═══════════════════════════════════════════════════════════════════
// Was `rtw_watch_dog_work` alle zwei Sekunden an die Firmware schickt.
// ═══════════════════════════════════════════════════════════════════
```

## L544-548 · `pub fn send_rssi_info(h: i32, st: &mut H2cState,`

```
/// fw.c:713-727 `rtw_fw_send_rssi_info`.
///
/// **Die Firmware waehlt die Rate, und das hier ist ihre Eingabe.** Ohne
/// sie rechnet die Ratenwahl auf dem Wert, den sie beim Anmelden bekommen
/// hat — auch noch, wenn die Leitung laengst schlechter ist.
```

## L555 · `h2c_set(&mut pkt, 0, 0x0000_ff00, si.mac_id as u32); // MACID`

```
// MACID
```

## L556 · `h2c_set(&mut pkt, 0, 0xff00_0000, rssi); // RSSI`

```
// RSSI
```

## L557 · `h2c_set(&mut pkt, 1, 1 << 1, (si.stbc_en != 0) as u32); // STBC`

```
// STBC
```

## L561 · `pub fn update_wl_phy_info(h: i32, st: &mut H2cState, dm: &crate::dm::DmInfo,`

```
/// fw.c:1000-1013 `rtw_fw_update_wl_phy_info`
```

## L566 · `h2c_set(&mut pkt, 0, 0x0003_ff00, tx_throughput); // TX_TP  GENMASK(17, 8)`

```
// TX_TP  GENMASK(17, 8)
```

## L567 · `h2c_set(&mut pkt, 0, 0x0ffc_0000, rx_throughput); // RX_TP  GENMASK(27, 18)`

```
// RX_TP  GENMASK(27, 18)
```

## L568 · `h2c_set(&mut pkt, 1, 0x0000_00ff, dm.tx_rate as u32); // TX_RATE_DESC`

```
// TX_RATE_DESC
```

## L569 · `h2c_set(&mut pkt, 1, 0x0000_ff00, dm.curr_rx_rate as u32); // RX_RATE_DESC`

```
// RX_RATE_DESC
```

## L570 · `h2c_set(&mut pkt, 1, 0x00ff_0000, dm.rx_evm_dbm[0] as u32); // RX_EVM`

```
// RX_EVM
```

## L574-577 · `pub fn adaptivity(h: i32, st: &mut H2cState, dm: &crate::dm::DmInfo) -> bool {`

```
/// fw.c:2465-2483 `rtw_fw_adaptivity`.
///
/// Der `rtw_edcca_enabled`-Zweig ist ein debugfs-Schalter (Vorgabe an);
/// debugfs bauen wir nicht, also gilt hier immer der eingeschaltete Fall.
```

## L581 · `h2c_set(&mut pkt, 0, 0x0000_0f00, dm.edcca_mode as u32); // MODE`

```
// MODE
```

## L582 · `h2c_set(&mut pkt, 0, 0x0000_f000, 1); // OPTION — Linux: fest 1`

```
// OPTION — Linux: fest 1
```

## L583 · `h2c_set(&mut pkt, 0, 0x00ff_0000, dm.igi_history[0] as u32); // IGI`

```
// IGI
```

## L584 · `h2c_set(&mut pkt, 0, 0xff00_0000, dm.l2h_th_ini as u32); // L2H`

```
// L2H
```

## L585 · `h2c_set(&mut pkt, 1, 0x0000_00ff, dm.scan_density as u32); // DENSITY`

```
// DENSITY
```

## L589-598 · `pub fn ra_report_handle(payload: &[u8], dm: &mut crate::dm::DmInfo,`

```
/// fw.c:265-323 `rtw_fw_ra_report_handle` + `_iter`.
///
/// **Die Rueckmeldung, welche Rate die FIRMWARE gerade fliegt.** Zwei
/// Posten des Watchdogs haengen daran: `rtw_phy_config_swing_table`
/// waehlt ueber `dm_info->tx_rate` zwischen CCK- und OFDM-Kurve, und
/// `rtw_phy_rrsr_update` rechnet aus `si->ra_report.desc_rate` die
/// Antwortraten. Ohne diesen Weg steht beides auf dem Anfangswert.
///
/// Der Rest von `_iter` (Flags, `bit_rate`, `max_rc_amsdu_len`) fuellt
/// `struct rate_info` fuer `cfg80211` und hat bei uns keinen Leser.
```

## L617-634 · `pub fn tx_report_parse(payload: &[u8], v1: bool) -> Option<(u8, bool)> {`

```
/// tx.c:229-256 `rtw_tx_report_handle`. Gibt `(Folgenummer, quittiert)`.
///
/// **rtw88 hat ZWEI Wege fuer dieselbe Quittung**, und `src` entscheidet
/// die Aufteilung:
///
/// * `C2H_CCX_TX_RPT` (0x03) — ein eigenes C2H, Aufteilung **V0**:
///   Nummer in `payload[6]`, Status in `payload[0]`.
/// * `C2H_CCX_RPT` (0x0f) — ein UNTERkommando von `C2H_HALMAC`
///   (fw.c:93-113), Aufteilung **V1**: Nummer in `payload[8]`, Status in
///   `payload[9]`, und `payload[0]` ist die Unterkommandokennung.
///
/// Welchen eine Firmware nimmt, steht in keinem Header. Der erste
/// Geraetelauf mit 0.26.0 hat es beantwortet: `0 ok, 0 ohne ACK, 49 ohne
/// bericht` — wir hoerten nur auf 0x03, und diese Firmware nimmt den
/// anderen Weg.
///
/// `st == 0` heisst quittiert — die zwei Statusbits sind ein Code, und
/// jeder von null verschiedene ist ein Misserfolg.
```

## L649-656 · `pub fn fw_crashed(h: i32) -> bool {`

```
/// fw.c:383-390 `rtw_fw_c2h_cmd_isr` — die Firmware meldet ihren EIGENEN
/// Absturz.
///
/// **In Linux ist das eine Unterbrechung**; wir haben keine und sehen
/// deshalb im Watchdog nach. Derselbe Test, dieselbe Stelle, anderer
/// Takt: steht in `REG_MCU_TST_CFG` der Ausloeserwert, hat die Firmware
/// sich selbst fuer tot erklaert. Bis hierher wurde daraus eine stille
/// Leitung.
```

