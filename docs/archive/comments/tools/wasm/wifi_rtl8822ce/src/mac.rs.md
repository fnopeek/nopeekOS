# `tools/wasm/wifi_rtl8822ce/src/mac.rs` @ 5e0102684

## L1-12 · `use crate::host;`

```
//! `mac.c` aus Linux 6.18.26 rtw88 — Stufe 1: der Strom.
//!
//! Portiert sind, in Aufrufreihenfolge und vollstaendig:
//! `rtw_mac_pre_system_cfg` · `do_pwr_poll_cmd` · `rtw_pwr_cmd_polling` ·
//! `rtw_sub_pwr_seq_parser` · `rtw_pwr_seq_parser` · `rtw_mac_power_switch` ·
//! `__rtw_mac_init_system_cfg` · `rtw_mac_init_system_cfg` ·
//! `rtw_mac_power_on` · `rtw_mac_power_off`.
//!
//! **Der 8822C ist WCPU_3081, nicht 8051.** Jede `rtw_chip_wcpu_8051()`-Abzweigung
//! ist damit statisch falsch — sie steht trotzdem als Konstante da, damit beim
//! Lesen sichtbar bleibt, dass Linux dort einen zweiten Weg hat und welcher
//! Zweig hier gilt.
```

## L18 · `const WCPU_8051: bool = false;`

```
/// rtw8822c.c `rtw8822c_hw_spec.wlan_cpu = RTW_WCPU_3081`.
```

## L21 · `const INTF_MASK: u8 = RTW_PWR_INTF_PCI_MSK;`

```
/// Wir sind PCIe. `rtw_hci_type()` ist bei uns eine Konstante.
```

## L25 · `Already,`

```
/// Linux: `-EALREADY` — der Chip ist schon in dem Zustand, den wir wollen.
```

## L27 · `Busy,`

```
/// Linux: `-EBUSY` — ein Polling-Kommando ist ausgelaufen.
```

## L31 · `pub fn pre_system_cfg(h: i32) {`

```
/// mac.c `rtw_mac_pre_system_cfg`, PCIe-Zweig.
```

## L36-38 · `return;`

```
// Linux setzt hier REG_LDO_SWR_CTRL nach BIT_LDO und kehrt SOFORT
// zurueck — der ganze Rest dieser Funktion gilt nur fuer die
// 3081-Familie. Fuer den 8822C unerreichbar.
```

## L42 · `host::set32(h, REG_HCI_OPT_CTRL, BIT_USB_SUS_DIS);`

```
// PCIe
```

## L45 · `let mut v = host::r32(h, REG_PAD_CTRL1);`

```
// config PIN Mux
```

## L58 · `let mut v8 = host::r8(h, REG_SYS_FUNC_EN);`

```
// disable BB/RF
```

## L72-79 · `fn do_pwr_poll_cmd(h: i32, addr: u32, mask: u8, target: u8) -> bool {`

```
/// mac.c `do_pwr_poll_cmd`. Linux pollt alle 50 us bis
/// `50 * RTW_PWR_POLLING_CNT` us = **1 s**.
///
/// Wir haben keinen 50-us-Schlaf — `npk_sleep` rastert in Millisekunden.
/// Also wird eng gelesen und die FRIST an `now_us()` gehalten: dieselbe
/// Gesamtfrist wie Linux, nur ohne den Takt dazwischen. Ein Deckel auf die
/// Runden gibt es bewusst nicht; die Frist ist die Frist
/// ([[feedback_a_cap_set_from_a_guess_is_below_the_normal_case]]).
```

## L84-88 · `const TIGHT_US: u64 = 2000;`

```
// Eng lesen, solange der Normalfall dauert (Linux pollt alle 50 us und
// ist meist nach wenigen Runden durch). Danach wird zwischen den Lesungen
// abgegeben: drei Polling-Kommandos gelten auf PCIe, und drei Fristen
// zu je einer Sekunde sind sechs Sekunden, in denen sonst niemand auf
// diesem Kern drankaeme.
```

## L104-107 · `fn pwr_cmd_polling(h: i32, cmd: &PwrCmd) -> Result<(), PwrErr> {`

```
/// mac.c `rtw_pwr_cmd_polling` — samt dem PCIe-Sonderweg: laeuft das Polling
/// aus, wird `BIT_PFM_WOWL` getoggelt und EINMAL neu gepollt. Ohne diesen
/// zweiten Versuch schlaegt die Sequenz auf manchen Boards beim ersten
/// Kaltstart fehl.
```

## L109-111 · `let offset = cmd.offset as u32;`

```
// `base == RTW_PWR_ADDR_SDIO` haengt in Linux SDIO_LOCAL_OFFSET an. Jede
// solche Zeile traegt intf_mask SDIO und wird eine Ebene hoeher schon
// aussortiert — auf PCIe ist der Fall unerreichbar.
```

## L118 · `let value = host::r8(h, REG_SYS_PW_CTRL);`

```
// PCIe: BIT_PFM_WOWL toggeln und noch einmal.
```

## L137 · `fn sub_pwr_seq_parser(h: i32, cut_mask: u8, seq: &[PwrCmd]) -> Result<(), PwrErr> {`

```
/// mac.c `rtw_sub_pwr_seq_parser`
```

## L156-158 · `if cmd.value == RTW_PWR_DELAY_US {`

```
// Die vier 8822C-Sequenzen enthalten KEIN DELAY (ausgezaehlt:
// 46 WRITE, 4 POLLING, 4 END). Der Zweig steht trotzdem hier,
// weil er in `rtw_sub_pwr_seq_parser` steht.
```

## L160-161 · `host::sleep_ms(1);`

```
// Unter unserer Aufloesung; eine Millisekunde ist die
// kleinste Pause, die wir ehrlich machen koennen.
```

## L174 · `pub fn pwr_seq_parser(h: i32, cut_version: u8, flow: &[&[PwrCmd]]) -> Result<(), PwrErr> {`

```
/// mac.c `rtw_pwr_seq_parser`
```

## L183 · `pub fn mac_power_switch(h: i32, cut_version: u8, pwr_on: bool) -> Result<(), PwrErr> {`

```
/// mac.c `rtw_mac_power_switch`
```

## L185 · `if !WCPU_8051 {`

```
// rtw_chip_wcpu_3081 gilt fuer den 8822C.
```

## L188-189 · `if host::r16(h, REG_MCUFW_CTRL) == MCUFW_CTRL_FW_ALIVE {`

```
// Laeuft noch Firmware? Dann den RPWM-Umschalter kippen, damit sie
// den Wechsel mitbekommt.
```

## L210 · `fn init_system_cfg(h: i32) {`

```
/// mac.c `__rtw_mac_init_system_cfg` (der 3081-Weg).
```

## L213 · `return; // __rtw_mac_init_system_cfg_legacy, hier unerreichbar`

```
// `__rtw_mac_init_system_cfg_legacy`, hier unerreichbar
```

## L224 · `let tmp = host::r32(h, REG_MCUFW_CTRL);`

```
// disable boot-from-flash for driver's DL FW
```

## L233-236 · `pub fn mac_power_on(h: i32, cut_version: u8) -> Result<(), PwrErr> {`

```
/// mac.c `rtw_mac_power_on`.
///
/// Der `-EALREADY`-Rueckfall ist kein Sonderfall: nach einem Warmstart steht
/// der Chip noch an, und dann ist AUS-dann-AN der normale Weg.
```

## L255 · `pub fn mac_power_off(h: i32, cut_version: u8) {`

```
/// mac.c `rtw_mac_power_off`
```

## L260 · `use crate::fw::{check_hw_ready, write_data_rsvd_page};`

```
// ── Stufe 2b: der Firmware-Download (mac.c) ──────────────────────
```

## L265 · `#[derive(Clone, Copy, Default)]`

```
/// `struct rtw_backup_info` (main.h)
```

## L273 · `const DLFW_RESTORE_REG_NUM: usize = 6;`

```
/// mac.c `DLFW_RESTORE_REG_NUM`
```

## L276-277 · `fn restore_reg(h: i32, bckp: &[Backup]) {`

```
/// util.c `rtw_restore_reg`. mac.c `download_firmware_reg_restore` ist ein
/// Einzeiler darum herum und faellt deshalb hier mit hinein.
```

## L289 · `pub fn ltecoex_read_reg(h: i32, offset: u16) -> Option<u32> {`

```
/// util.c `ltecoex_read_reg`
```

## L298 · `pub fn ltecoex_reg_write(h: i32, offset: u16, value: u32) -> bool {`

```
/// util.c `ltecoex_reg_write`
```

## L308 · `fn wlan_cpu_enable(h: i32, enable: bool) {`

```
/// mac.c `wlan_cpu_enable`
```

## L319-321 · `#[allow(dead_code)]`

```
/// Felder aus `struct rtw_fw_hdr` (fw.h:20-40), alle little-endian.
/// `h2c_fmt_ver` liest heute niemand — es entscheidet ab 2c, welches
/// H2C-Format gilt, und gehoert deshalb schon hier hin.
```

## L362 · `pub fn check_firmware_size(d: &[u8]) -> bool {`

```
/// mac.c `check_firmware_size`
```

## L378 · `fn download_firmware_reg_backup(h: i32) -> [Backup; DLFW_RESTORE_REG_NUM] {`

```
/// mac.c `download_firmware_reg_backup`
```

## L383 · `b[i] = Backup { len: 1, reg: REG_TXDMA_PQ_MAP + 1,`

```
// set HIQ to hi priority
```

## L389 · `b[i] = Backup { len: 1, reg: REG_CR, val: host::r8(h, REG_CR) as u32 };`

```
// DLFW only use HIQ, map HIQ to hi priority
```

## L397 · `b[i] = Backup { len: 2, reg: REG_FIFOPAGE_INFO_1,`

```
// Config hi priority queue and public priority queue page number
```

## L407 · `let tmp = host::r8(h, REG_BCN_CTRL);`

```
// Disable beacon related functions
```

## L417 · `fn download_firmware_reset_platform(h: i32) {`

```
/// mac.c `download_firmware_reset_platform`
```

## L425 · `fn iddma_enable(h: i32, src: u32, dst: u32, ctrl: u32) -> bool {`

```
/// mac.c `iddma_enable`
```

## L433 · `fn iddma_download_firmware(h: i32, src: u32, dst: u32, len: u32, first: bool) -> bool {`

```
/// mac.c `iddma_download_firmware`
```

## L446 · `fn check_fw_checksum(h: i32, addr: u32) -> bool {`

```
/// mac.c `check_fw_checksum`
```

## L470 · `#[allow(clippy::too_many_arguments)]`

```
/// mac.c `download_firmware_to_mem`
```

## L494-500 · `let verbose = dump_first && first_part && mem_offset == 0;`

```
// mac.c `send_firmware_pkt` -> `send_firmware_pkt_rsvd_page`:
// pg_addr = src >> 7. Der USB-Sonderfall (+1 Byte, wenn
// (size + TX_DESC_SIZE) auf 512 aufgeht) gilt nur dort, und
// `kmemdup` daneben ist Linux-Speicherverwaltung.
// Nur das allererste Stueck des ganzen Downloads wird ausgeschuettet
// — fuenfzig Stuecke mal zwoelf Register waeren keine Diagnose mehr,
// sondern eine Wand.
```

## L526 · `fn start_download_firmware(h: i32, trx: &Trx, stage: i32, fw: &[u8], band: u8,`

```
/// mac.c `start_download_firmware`
```

## L572 · `fn download_firmware_end_flow(h: i32) {`

```
/// mac.c `download_firmware_end_flow`
```

## L584-591 · `const VALIDATE_DIAG_US: u64 = 500_000;`

```
/// mac.c `download_firmware_validate`
///
/// Hier wartet nicht die Hardware auf ein Register, sondern WIR auf eine
/// Firmware, die gerade anlaeuft: `BIT_FW_INIT_RDY` setzt sie selbst,
/// nachdem `wlan_cpu_enable` ihren Kern gestartet hat. Linux gibt dafuer
/// 10 ms. Schafft sie das nicht, wird hier NICHT einfach aufgegeben,
/// sondern weitergemessen — die Zahl sagt, ob die Frist zu knapp ist oder
/// ob das Bit nie kommt, und das sind zwei verschiedene Fehler.
```

## L631 · `pub fn download_firmware(h: i32, trx: &mut Trx, stage: i32, fw: &[u8], band: u8,`

```
/// mac.c `__rtw_download_firmware`, alle dreizehn Schritte in Reihenfolge.
```

## L647-649 · `host::w8(h, REG_C2HEVT, C2H_HW_FEATURE_DUMP);`

```
// rtw_chip_efuse_enable: DAS steht zwischen mac_power_on und dem
// Download, und ohne es liefert die Firmware spaeter keinen
// hw-feature-Bericht.
```

## L652-656 · `host::print("  [dump] nach power_on, vor dem Backup:\n");`

```
// Was die Power-Sequenz hinterlassen hat, BEVOR das Backup es umschreibt.
// REG_RQPN_CTRL_2 ist der interessante: das Backup ODERt nur BIT_LD_RQPN
// darauf, es SETZT die Seitenzahlen nicht — die kommen erst in
// __priority_queue_cfg, also nach dem Download. Steht hier eine 0, hat
// die HIQ null Seiten, und dann kann keine Reserved Page landen.
```

## L679-680 · `crate::pci::setup(h, trx, false);`

```
// "reset desc and index" — rtw_hci_setup nach dem Download,
// also wieder BEIDE Haelften.
```

## L686 · `host::clr8(h, REG_MCUFW_CTRL, BIT_MCUFWDL_EN);`

```
// dlfw_fail
```

## L692-702 · `use crate::chip;`

```
// ── Stufe 3a: rtw_mac_init (mac.c:1391) ──────────────────────────
//
// Reihenfolge wie in Linux:
//   rtw_mac_init
//    ├─ rtw_init_trx_cfg
//    │   ├─ txdma_queue_mapping
//    │   ├─ priority_queue_cfg   (rtw_set_trx_fifo_info + __priority_queue_cfg)
//    │   └─ init_h2c
//    ├─ chip->ops->mac_init      = chip::mac_init
//    ├─ rtw_drv_info_cfg
//    └─ rtw_hci_interface_cfg    = pci::interface_cfg
```

## L706-713 · `#[derive(Default, Clone, Copy)]`

```
/// main.h:1886-1900 `struct rtw_fifo_conf`, ohne den Zeiger auf `rqpn` —
/// der ist bei uns eine Konstante, weil wir genau einen Bus haben.
///
/// **Der Vorgabewert ist NICHT Kosmetik.** `rtw_fw_write_data_rsvd_page`
/// schreibt `rsvd_boundary` beim Aufraeumen zurueck, und solange
/// `rtw_mac_init` nicht gelaufen ist, steht dort in Linux eine 0. Der
/// Firmware-Download passiert VOR `rtw_mac_init` — also mit 0, und das ist
/// richtig so.
```

## L731 · `NoMem,`

```
/// Linux: `-ENOMEM` — der Seitenplan passt nicht in den TX-FIFO.
```

## L733-734 · `Inval,`

```
/// Linux: `-EINVAL` — rsvd_boundary und rsvd_drv_addr laufen auseinander,
/// oder der H2C-Ring meldet eine andere Fuellung als seine Groesse.
```

## L736 · `Busy,`

```
/// Linux: `-EBUSY` — die Hardware hat die Link-List-Tabelle nicht gebaut.
```

## L740 · `fn txdma_queue_mapping(h: i32) -> &'static chip::Rqpn {`

```
/// mac.c:1086-1135 `txdma_queue_mapping`, PCIe-Zweig.
```

## L753-755 · `host::w8(h, REG_CR, 0);`

```
// Erst AUS, dann alle acht TRX-Bits an. Das ist kein Vorsichtsschritt,
// sondern steht so in Linux — und ein `write8` auf REG_CR laesst die
// oberen Bytes des 32-Bit-Registers in Ruhe.
```

## L759 · `if !WCPU_8051 {`

```
// rtw_chip_wcpu_3081 — gilt fuer den 8822C.
```

## L767-774 · `pub fn set_trx_fifo_info() -> Result<Fifo, MacErr> {`

```
/// mac.c:1138-1186 `rtw_set_trx_fifo_info`, 3081-Zweig.
///
/// Rechnet den ganzen Seitenplan des Sende-FIFOs, von oben nach unten: was
/// die Firmware fuer sich behaelt, liegt AM ENDE des FIFOs, und
/// `rsvd_boundary` ist die Grenze, unter der die Sendequeues arbeiten.
/// Die Schlusspruefung (`rsvd_boundary == rsvd_drv_addr`) ist Linux' eigene
/// Gegenrechnung: die Grenze wird zweimal auf verschiedenen Wegen bestimmt,
/// und wenn beide nicht dasselbe sagen, stimmt der Plan nicht.
```

## L828 · `fn priority_queue_cfg_3081(h: i32, f: &Fifo, pg: &chip::PageTable, pubq_num: u16)`

```
/// mac.c:1192-1236 `__priority_queue_cfg` (3081-Zweig, PCIe).
```

## L847-848 · `host::set8(h, REG_AUTO_LLT_V1, BIT_AUTO_INIT_LLT_V1 as u8);`

```
// Der USB-Zweig (BIT_MASK_BLK_DESC_NUM, TXDMA_OFFSET_CHK+1) steht in
// Linux dazwischen und gilt fuer uns nicht.
```

## L852-854 · `if !check_hw_ready(h, REG_AUTO_LLT_V1, BIT_AUTO_INIT_LLT_V1, 0) {`

```
// Die Hardware baut jetzt ihre Link-List-Tabelle und LOESCHT das Bit,
// wenn sie fertig ist. Das ist die einzige Quittung, die es fuer den
// Seitenplan gibt.
```

## L864 · `fn priority_queue_cfg(h: i32) -> Result<Fifo, MacErr> {`

```
/// mac.c:1260-1299 `priority_queue_cfg`, PCIe-Zweig.
```

## L867 · `let pg = &chip::PAGE_TABLE[1]; // RTW_HCI_TYPE_PCIE`

```
// RTW_HCI_TYPE_PCIE
```

## L873 · `return Err(MacErr::Inval);`

```
// `__priority_queue_cfg_legacy`, fuer den 8822C unerreichbar.
```

## L880-886 · `fn init_h2c(h: i32, f: &Fifo) -> Result<(), MacErr> {`

```
/// mac.c:1301-1352 `init_h2c` (3081).
///
/// Der H2C-Ring liegt IM Sende-FIFO, auf den Seiten, die `set_trx_fifo_info`
/// dafuer reserviert hat. Geschrieben werden Kopf, Schwanz und Lesezeiger als
/// BYTE-Adressen (`seite << 7`), und die Schlusspruefung fragt die Hardware,
/// wie voll sie den Ring sieht: ein frischer Ring ist leer, also muss
/// `h2cq_free` genau `h2cq_size` sein.
```

## L940 · `fn init_trx_cfg(h: i32) -> Result<Fifo, MacErr> {`

```
/// mac.c:1354-1371 `rtw_init_trx_cfg`
```

## L948 · `fn drv_info_cfg(h: i32) {`

```
/// mac.c:1373-1389 `rtw_drv_info_cfg`
```

## L952 · `let value8 = (host::r8(h, REG_TRXFF_BNDY + 1) & 0xF0) | 0xF;`

```
// "For rxdesc len = 0 issue"
```

## L960-964 · `pub fn mac_init(h: i32, cut_version: u8) -> Result<Fifo, MacErr> {`

```
/// mac.c:1391-1411 `rtw_mac_init`.
///
/// Gibt den Seitenplan zurueck, weil er ab hier gebraucht wird: jede
/// Reserved Page, die spaeter geschrieben wird, liegt relativ zu
/// `rsvd_boundary`.
```

## L968 · `if !chip::mac_init(h) {`

```
// chip->ops->mac_init
```

## L975 · `crate::pci::interface_cfg(h, cut_version);`

```
// rtw_hci_interface_cfg
```

## L981-985 · `pub fn set_channel_mac(h: i32, channel: u8, bw: usize, primary_ch_idx: u8) {`

```
/// mac.c:1029-1076 `rtw_set_channel_mac`, 3081-Zweig.
///
/// Die MAC-Haelfte des Kanalwechsels: Unterkanallage, Bandbreite im
/// Sendeprotokoll, MAC-Takt und die CCK-Pruefung, die auf 5 GHz AN ist —
/// dort darf gar keine CCK-Rate ankommen.
```

## L990 · `txsc40 = if txsc20 == RTW_SC_20_UPPER || txsc20 == RTW_SC_20_UPMOST {`

```
// RTW_CHANNEL_WIDTH_80
```

## L997 · `host::w8(h, REG_DATA_SC,`

```
// reg.h:260-267 `BIT_TXSC_20M(x)` und `BIT_TXSC_40M(x)`
```

## L1006 · `_ => {}`

```
// RTW_CHANNEL_WIDTH_20 und Linux' `default:` — nichts dazu.
```

## L1029-1035 · `const PRIOQ_ADDRS: [(u32, u32); 4] = [`

```
// ═══════════════════════════════════════════════════════════════════
// rtw_mac_flush_queues (mac.c:1020-1080) — die Sendeschlangen leeren
//
// Linux tut das vor einem Kanalwechsel und vor dem Trennen. Ohne das
// gehen nach einem Wechsel noch Rahmen auf dem ALTEN Kanal hinaus — und
// genau dieser Fall liegt beim Wiederverbinden vor uns.
// ═══════════════════════════════════════════════════════════════════
```

## L1037-1039 · `const PRIOQ_ADDRS: [(u32, u32); 4] = [`

```
/// rtw8822c.c:4943-4957 `prioq_addrs_8822c` — je Prioritaetsschlange
/// `(rsvd, avail)`, in der Reihenfolge von `enum rtw_dma_mapping`
/// (EXTRA, LOW, NORMAL, HIGH). `.wsize = true`, also 16-Bit-Zugriffe.
```

## L1041 · `(REG_FIFOPAGE_INFO_4, REG_FIFOPAGE_INFO_4 + 2), // EXTRA`

```
// EXTRA
```

## L1042 · `(REG_FIFOPAGE_INFO_2, REG_FIFOPAGE_INFO_2 + 2), // LOW`

```
// LOW
```

## L1043 · `(REG_FIFOPAGE_INFO_3, REG_FIFOPAGE_INFO_3 + 2), // NORMAL`

```
// NORMAL
```

## L1044 · `(REG_FIFOPAGE_INFO_1, REG_FIFOPAGE_INFO_1 + 2), // HIGH`

```
// HIGH
```

## L1047-1053 · `fn flush_prio_queue(h: i32, prio: usize) -> bool {`

```
/// mac.c:1024-1060 `__rtw_mac_flush_prio_queue`.
///
/// **Die Schlange ist leer, wenn alle reservierten Seiten wieder
/// verfuegbar sind.** Fuenf Runden zu 20 ms — Linux' eigener Kommentar
/// sagt, dass eine volle Schlange bei 100 Mbit/s bis zu zwei Sekunden
/// braucht und dabei Rahmen fallen koennen; die Frist hier ist also
/// absichtlich kurz.
```

## L1067-1074 · `pub fn flush_queues(h: i32) -> u32 {`

```
/// mac.c:1062-1069 `rtw_mac_flush_prio_queues` + mac.c:1071-1080
/// `rtw_mac_flush_queues`.
///
/// Wir leeren immer ALLE vier — das ist Linux' Zweig „alle Schlangen
/// angefordert oder die Zuordnung steht noch nicht", und einen Rufer,
/// der einzelne Zugangsklassen leeren will, gibt es hier nicht.
///
/// Gibt zurueck, wie viele Schlangen in der Frist leer wurden.
```

