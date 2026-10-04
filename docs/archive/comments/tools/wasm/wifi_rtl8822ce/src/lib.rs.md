# `tools/wasm/wifi_rtl8822ce/src/lib.rs` @ 5e0102684

## L1-50 · `#![no_std]`

```
//! wifi_rtl8822ce — Realtek RTL8822CE (Wi-Fi 5, 2T2R, PCIe), WASM-Treiber.
//!
//! Strikte 1:1-Portierung von Linux 6.18.26,
//! `drivers/net/wireless/realtek/rtw88/` (Modul `rtw_8822ce`).
//! Plan: `docs/plan/WIFI_RTL8822CE.md` · Karte:
//! `docs/plan/WIFI_RTL8822CE_LINUX_MAP.md`.
//!
//! **Stufe 0: die Tuer.** PCI binden, Bus-Master, **BAR2**
//! abbilden (pci.c `rtw_pci_io_mapping`: `u8 bar_id = 2` — nicht BAR0), und
//! `REG_SYS_CFG1` lesen wie `rtw_chip_parameter_setup` es tut.
//!
//! **Stufe 0 SCHREIBT NICHTS.** Kein Register wird angefasst, keine
//! Power-Sequenz gefahren. Was hier schiefgeht, kann also nicht an einem
//! Schreibzugriff von uns liegen — und das ist der ganze Sinn einer ersten
//! Stufe, die nur eine Frage stellt.
//!
//! **Die Stufen 0-2a sind Diagnose und KEHREN ZURUECK.** Ein Treiber, der
//! nicht endet, haelt das Terminal, aus dem er gestartet wurde
//! (`spawn_on_worker` setzt `APP_RUNNING`) — und ein Lauf, nach dem man nicht
//! weiterarbeiten kann, ist beim Suchen schlimmer als kein Lauf. Der Chip
//! bleibt dabei AUS zurueck, und der Kernel gibt beim Ende alles DMA frei.
//! Erst wenn die Firmware laeuft und Frames fliessen (ab 2b), wird daraus ein
//! Treiber, der bleiben muss — und dann ist der Startweg die Frage, nicht das
//! Modul.
//!
//! **Ab 2b kommt eine zweite Pflicht dazu, und die stand schon im AX200:**
//! „the kernel frees our DMA buffers on return and a still-running firmware
//! must not DMA into them afterwards". Solange nur der MAC an- und wieder
//! ausgeht, ist das erledigt; sobald eine Firmware laeuft, muss sie VOR dem
//! Zurueckkehren angehalten werden.
//!
//! Gate: `chip_version` plausibel, RF-Typ 2T2R, und der frische 8-Bit-Pfad
//! (`npk_mmio_read8`) liefert byteweise dasselbe wie der 32-Bit-Pfad.
//!
//! **Stufe 1: der Strom.** `rtw_mac_power_on` vollstaendig (`mac.rs`), mit
//! den vier Power-Sequenz-Tabellen aus `pwrseq.rs` — erzeugt aus der
//! C-Quelle, nicht abgetippt. Noch keine Firmware, noch keine Ringe.
//! Gate in BEIDE Richtungen: `REG_CR` verlaesst `0xea` beim Einschalten und
//! kehrt beim Abschalten dorthin zurueck. Nur eine Richtung zu messen hiesse,
//! einen Zustand zu pruefen, den der Chip vielleicht schon hatte.
//!
//! **Stufe 2b: die Firmware.** `rtw_download_firmware` vollstaendig
//! (`mac.rs` + `fw.rs` + `tx.rs`), 202600 Bytes ueber die BCN-Queue und
//! DDMA nach dmem/imem/emem. Gate: `REG_MCUFW_CTRL` liest `FW_READY`.
//!
//! **Stufe 2a: die Ringe.** `rtw_pci_init_trx_ring` + `rtw_pci_reset_buf_desc`
//! (`pci.rs`), in Linux' Reihenfolge — die Ringregister werden programmiert,
//! BEVOR der MAC angeht (`rtw_power_on` ruft `rtw_hci_setup` vor
//! `rtw_mac_power_on`). Gate: jedes Adress- und Anzahlregister gibt zurueck,
//! was hineingeschrieben wurde, einmal mit MAC aus und einmal mit MAC an.
```

## L84-86 · `#[panic_handler]`

```
/// Nie still sterben: ein `loop {}` ohne Meldung sieht von aussen aus wie
/// „der Chip antwortet nicht" und hat beim AX200 einen Abend gekostet.
/// `Location` ueberlebt `strip = true`, weil es statische Daten sind.
```

## L89-90 · `host::loud_begin();`

```
// Eine Panik ist nie Stufenausgabe: laut, auch ohne `debug: 1`, und
// die Klammer wird nicht mehr geschlossen — danach kommt nichts.
```

## L104-105 · `const DRIVER_VERSION: &str = env!("CARGO_PKG_VERSION");`

```
/// Eine Quelle fuer die Version — Banner und Bericht koennen nicht
/// auseinanderlaufen.
```

## L108-110 · `static FW: &[u8] = include_bytes!("../firmware/rtw8822c_fw.bin");`

```
/// rtw8822c.c `.fw_name = "rtw88/rtw8822c_fw.bin"` — mitgeliefert wie der
/// AX200-Blob. Version 9.9.15, und `check_firmware_size` rechnet den Kopf
/// gegen die Dateilaenge nach, bevor ein Byte an den Chip geht.
```

## L113-116 · `const BAND_AT_FWDL: u8 = 0;`

```
/// `hal.current_band_type` ist beim Download noch 0 — gesetzt wird es erst
/// in `rtw_set_channel`, also lange danach. Das entscheidet in
/// `rtw_tx_pkt_info_update_rate`, welcher Zweig gilt, und wir nehmen
/// denselben wie Linux.
```

## L119-135 · `struct Dev {`

```
/// **`struct rtw_dev` — der Zustand, der so lange lebt wie der Treiber.**
///
/// Bis 0.26.0 legte JEDE Stufe ihr eigenes `DmInfo`, `DpkInfo` und
/// `Coex` an. Solange nur Stufen liefen, war das folgenlos; mit dem
/// Watchdog ist es ein Fehler, und zwar ein stiller:
///
/// * `cfo_track.crystal_cap` kommt aus `rtw_phy_init` (dem Wert der
///   efuse). Ein frisches `DmInfo` traegt dort NULL — die
///   Quarznachfuehrung wuerde von null hochlaufen statt von der
///   Werkseinstellung.
/// * `dpk_info.thermal_dpk` kommt aus der Kalibrierung von Stufe 5d.
///   Ohne sie kehrt `dpk_track` in der ersten Zeile um.
/// * `coex.bt_disabled` entscheidet, ob die Quarznachfuehrung ueberhaupt
///   laufen darf.
///
/// In Linux liegt all das in `rtwdev` und lebt vom Laden bis zum
/// Entladen. Hier jetzt auch.
```

## L141 · `stats: TrafficStats,`

```
/// main.h:660-672 `struct rtw_traffic_stats`
```

## L143-144 · `watch_dog_cnt: u32,`

```
/// `rtwdev->watch_dog_cnt` — `rtw_phy_ra_info_update` laeuft nur auf
/// jedem vierten.
```

## L146 · `busy_traffic: bool,`

```
/// `RTW_FLAG_BUSY_TRAFFIC`
```

## L148 · `beacon_loss: bool,`

```
/// `rtwdev->beacon_loss`
```

## L150-157 · `cur_bw: usize,`

```
/// `hal->current_band_width` — die Breite, auf der die PHY GERADE
/// steht, gesetzt von Stufe 5e beim Kanalwechsel.
///
/// **Sie steht hier, damit sie nur EINMAL entschieden wird.** Stufe 5f
/// klemmte die Breite des Gegenuebers vorher gegen ein zweites
/// `chan_params` auf denselben Eingaben — dieselbe Rechnung an einer
/// zweiten Stelle, und damit eine zweite Antwort, sobald eine von
/// beiden sich aendert.
```

## L161-163 · `#[derive(Default, Clone, Copy)]`

```
/// main.h:660-672 `struct rtw_traffic_stats`. Die Einheiten stehen dort
/// als Kommentar und sind hier Teil des Namens: Bytes je zwei Sekunden,
/// umgerechnet mit `RTW_TP_SHIFT`.
```

## L172-173 · `tx_peak: u32,`

```
/// Der hoechste je gesehene Wert — der geglaettete faellt nach dem
/// Ende einer Uebertragung auf null.
```

## L191 · `struct Hal {`

```
/// Ergebnis von `rtw_chip_parameter_setup` (main.c:1876-1900).
```

## L199 · `antenna_tx: u8,`

```
/// main.c:1884-1893 — bei 2T2R beide `BB_PATH_AB`, sonst `BB_PATH_A`.
```

## L202-204 · `rcr: u32,`

```
/// main.c:2183 die Vorgabe, main.c:1903 das ODER mit `BIT_VHT_DACK`.
/// `rtw_core_start` schreibt das ins Register, NACHDEM `mac_init` dort
/// `WLAN_RCR_CFG` hinterlassen hat — „rcr reset after powered on".
```

## L208-209 · `fn chip_parameter_setup(h: i32) -> Hal {`

```
/// main.c `rtw_chip_parameter_setup` — der Teil, der aus EINEM Register
/// liest. Der Rest der Funktion setzt nur Felder aus `chip`.
```

## L216 · `mp_chip: if chip_version & BIT_RTL_ID != 0 { 0 } else { 1 },`

```
// main.c:1883 — gesetztes BIT_RTL_ID heisst NICHT mp_chip.
```

## L223 · `rcr: BIT_APP_FCS | BIT_APP_MIC | BIT_APP_ICV | BIT_PKTCTL_DLEN`

```
// main.c:2183 „default rx filter setting" plus main.c:1903.
```

## L230-236 · `fn check_access_widths(h: i32, word: u32) -> bool {`

```
/// Der 8-Bit-Pfad ist neu im Kernel. Bevor irgendetwas darauf aufbaut, wird
/// er GEGEN den bewaehrten 32-Bit-Pfad gehalten: vier Bytes einzeln gelesen
/// muessen dasselbe Wort ergeben. Dasselbe fuer 16 Bit.
///
/// Das ist billig und es ist read-only — und es beantwortet in einer Zeile
/// die Frage, die sonst erst in Stufe 1 mitten in der Power-Sequenz auffaellt,
/// wo zehn andere Dinge gleichzeitig neu sind.
```

## L261-266 · `let (verbose, cfg_rc) = read_debug_flag();`

```
// ── Wie laut? ────────────────────────────────────────────────
// **Zuerst, vor der ersten Zeile.** Im Autostart druckt der Treiber
// sechs Stufen mit ihren Toren und macht die Konsole unbrauchbar;
// `debug: 1` in `sys/config/wifi` holt sie zurueck. Die Datei ist
// dieselbe, aus der Stufe 5c ihr `ssid:` liest — eine zweite Stelle
// fuer dieselbe Sache driftet.
```

## L274-281 · `host::say("[rtl8822ce] v");`

```
// Die eine Zeile, die auch ein stiller Lauf schuldet: dass es
// den Treiber gibt und wo der Schalter steht.
//
// **Und ob die Datei ueberhaupt gelesen wurde.** `wifid`
// dokumentiert fuer genau dieses Objekt ein Rennen mit dem Rest
// des Bootvorgangs; ohne diesen Zusatz saehe ein gescheiterter
// Lesezugriff aus wie ein Schalter, der nicht greift — und das
// kostet einen ganzen Geraetelauf.
```

## L292-294 · `static mut DEV: Dev = Dev {`

```
// ── `rtwdev`: der Zustand ueber den ganzen Treiberlauf ───────
// Gross genug, um nicht auf den Stapel zu gehoeren (die
// DACK-Sicherungen und die Ratenzaehler machen den Loewenanteil).
```

## L306-307 · `let rtwdev = unsafe { &mut *core::ptr::addr_of_mut!(DEV) };`

```
// SAFETY: einfaedig, genau ein Rufer, und `_start` kehrt erst
// zurueck, wenn der Treiber endet.
```

## L310-311 · `let mut dev = RTL8822CE_DEVICE;`

```
// ── PCI binden ───────────────────────────────────────────────
// rtw8822ce.c fuehrt zwei Geraete-IDs fuer denselben Chip.
```

## L334-336 · `let bm = host::pci_enable_bus_master();`

```
// Rueckgabewert lesen, nicht wegwerfen: ohne Busmaster kann der Chip
// keinen Deskriptor aus dem Hauptspeicher holen, und das sieht dann aus
// wie ein Fehler im Treiber statt wie eine fehlende Erlaubnis.
```

## L343-350 · `match pci::power_up_d0(0) {`

```
// ── D0, bevor jemand ein Register liest ──────────────────────
//
// Florian: „dass die karte nicht initialsiert hat.. als waers ein
// timing problem.. beim laden.. er bricht bei den phasen 1-5 ab."
// Ein Geraet in D3hot antwortet auf jede MMIO-Lesung mit lauter
// Einsen, waehrend der Konfigurationsraum normal antwortet — und
// genau das sieht aus wie ein Zeitproblem, weil es davon abhaengt,
// in welchem Zustand der vorige Lauf die Karte hinterlassen hat.
```

## L363 · `let h = host::mmio_map_bar(BAR_REG, BAR_PAGES);`

```
// ── BAR2 abbilden ────────────────────────────────────────────
```

## L373-374 · `pci::link_cfg(h);`

```
// ── `rtw_pci_phy_cfg` / `rtw_pci_link_cfg` ───────────────────
// Muss NACH der BAR-Abbildung stehen: der DBI-Weg laeuft ueber MMIO.
```

## L405-413 · `const WINDOW_WAIT_US: u64 = 200_000;`

```
// ── Kennung lesen (rtw_chip_parameter_setup) ─────────────────
//
// **Einmal lesen war zu wenig.** Hier stand ein einziger Zugriff,
// und war die Antwort lauter Einsen, endete der Treiber. Ein Chip,
// der gerade aufwacht, braucht aber Zeit — und mit `power_up_d0`
// davor sind es genau die 10 ms, die die Spezifikation nennt.
// Trotzdem wird hier gewartet statt geraten: eine Frist kostet im
// Normalfall nichts (die erste Lesung trifft) und im Fehlerfall
// 200 ms statt eines Neustarts.
```

## L426-427 · `let dead = hal.chip_version == 0xFFFF_FFFF || hal.chip_version == 0;`

```
// Ein Fenster, das nur Einsen liefert, ist keine Antwort des Chips,
// sondern die Antwort des Busses auf eine Adresse, an der niemand ist.
```

## L439-442 · `host::loud_begin();`

```
// **Der Konfigurationsraum antwortet, auch wenn MMIO es nicht
// tut.** Steht hier eine gueltige Kennung, ist die Karte da und
// es ist der Speicherpfad — eine andere Krankheit als „nicht
// gefunden", und ohne diese Zeile sehen beide gleich aus.
```

## L462-464 · `let cr = host::r8(h, REG_CR);`

```
// Die zwei Werte, die Linux SELBST als Klartext vergleicht — und der
// erste ist ausdruecklich ein 8-Bit-Lesezugriff (mac.c
// `rtw_mac_power_switch`: `rtw_read8(rtwdev, REG_CR) == 0xea`).
```

## L478 · `host::print("[rtl8822ce] Chip: cut ");`

```
// ── Auswertung ───────────────────────────────────────────────
```

## L493 · `let widths_ok = check_access_widths(h, hal.chip_version);`

```
// ── Gates ────────────────────────────────────────────────────
```

## L507-510 · `let mut trx = match pci::init_trx_ring() {`

```
// ── Stufe 2a: die Ringe (rtw_pci_setup_resource) ─────────────
// Die Reihenfolge ist Linux': rtw_power_on ruft rtw_hci_setup — und
// damit rtw_pci_setup — VOR rtw_mac_power_on. Die Ringregister liegen
// im PCIe-Block und leben unabhaengig vom MAC.
```

## L526 · `pci::setup(h, &mut trx, true); // = rtw_hci_setup: reset_trx_ring + dma_reset`

```
// = rtw_hci_setup: reset_trx_ring + dma_reset
```

## L530 · `host::print("[rtl8822ce] Stufe 1: Power-Sequenz (");`

```
// ── Stufe 1: der Strom ───────────────────────────────────────
```

## L565-569 · `host::print("[rtl8822ce] dieselben Register mit MAC AN:\n");`

```
// Dieselbe Pruefung mit laufendem MAC. Linux programmiert die Ringe
// nach dem Firmware-Download NOCH EINMAL (`rtw_hci_setup` in
// `__rtw_download_firmware`, Kommentar: „reset desc and index") — also
// ist die Frage, ob dazwischen etwas verlorengeht, berechtigt und
// billig zu beantworten.
```

## L584-587 · `let hdr = mac::parse_fw_hdr(FW);`

```
// ── Stufe 2b: die Firmware (rtw_download_firmware) ───────────
// Reihenfolge wie `rtw_power_on`: hci_setup, mac_power_on, DANN der
// Download. Vorher gibt es keinen laufenden MAC, durch dessen BCN-Queue
// die Seiten gehen koennten.
```

## L601-603 · `let mut fifo = mac::Fifo::default();`

```
// `rtwdev->fifo` lebt in Linux ueber den ganzen Treiber und ist bis zum
// ersten `rtw_mac_init` NULL. Der Download liest daraus `rsvd_boundary`
// — hier also noch 0, genau wie dort.
```

## L606-608 · `let fw_feature = mac::parse_fw_hdr(FW).feature;`

```
// Die Merkmalsbits der Firmware entscheiden, welche H2C-Kommandos sie
// ueberhaupt kennt (`rtw_fw_feature_check`). Sie stehen im Kopf des
// Abbilds, also gibt es sie schon vor dem Download.
```

## L611-614 · `let mut h2c = fw::H2cState::default();`

```
// `rtwdev->h2c` — EINER fuer das ganze Geraet. Die Reihenfolge der vier
// Postfaecher ist der Sinn der Sache: der Treiber reicht sie im Kreis
// weiter, damit die Firmware Zeit hat, das vorige zu leeren. Bis 0.17.0
// legte jede Stufe einen eigenen an und fing wieder bei Fach 0 an.
```

## L619-620 · `let h2c_buf = host::dma_alloc_below(`

```
// Der H2C-Ring braucht einen EIGENEN Zwischenpuffer: der oben ist fuer
// den Firmware-Download gedacht und genau ein Stueck gross.
```

## L623-624 · `let mgmt_buf = host::dma_alloc_below(`

```
// Und die MGMT-Queue einen dritten: ihre Rahmen sind bis 2 KB gross,
// das H2C-Raster von 128 Bytes traegt keinen einzigen davon.
```

## L648 · `let mut stage2c = false;`

```
// ── Stufe 2c: efuse und hw_feature ───────────────────────────
```

## L691 · `let valid = e.addr != [0u8; 6]`

```
// main.c: is_valid_ether_addr — nicht null, nicht multicast.
```

## L702-705 · `mac::mac_power_off(h, hal.cut_version);`

```
// Wie Linux es in rtw_chip_efuse_info_setup tut: wieder ausschalten.
// Ab hier ist das PFLICHT und nicht Kosmetik — eine laufende Firmware
// darf nicht mehr in Puffer schreiben, die der Kernel beim Zurueckkehren
// freigibt (dieselbe Begruendung steht am Ende von wifi_ax200).
```

## L732-736 · `let (stage4b, _txpwr) = match efuse.as_ref() {`

```
// ── Stufe 4b: rtw_chip_board_info_setup ──────────────────────
// Sie steht VOR Stufe 3, weil sie in Linux vor `rtw_power_on` steht:
// `rtw_chip_info_setup` = parameter_setup -> efuse_info_setup ->
// board_info_setup. Die Nummer 4b ist die Reihenfolge, in der wir
// gebaut haben, nicht die, in der gelaufen wird.
```

## L745-758 · `let mut stage3b = false;`

```
// ── Stufe 3a: rtw_power_on, bis rtw_mac_init ─────────────────
//
// Alles davor war `rtw_chip_info_setup` — in Linux die Probe-Zeit, die
// den Chip anschaltet, NUR um die efuse zu lesen, und ihn danach wieder
// ausschaltet. Das hier ist der ZWEITE Zyklus, `rtw_power_on`
// (main.c:1374), und er faengt wieder ganz vorne an:
//
//     rtw_hci_setup -> rtw_mac_power_on -> rtw_download_firmware
//                   -> rtw_mac_init
//
// Die Firmware wird also ein zweites Mal geladen. Das ist keine
// Verschwendung aus Unachtsamkeit, sondern was Linux tut: zwischen den
// Zyklen war der MAC aus, und ein ausgeschalteter MAC hat keine
// Firmware mehr.
```

## L780 · `let stage4a = match (stage3c, efuse.as_ref()) {`

```
// ── Stufe 4a: der Rest von rtw_power_on und rtw_core_start ───
```

## L790 · `let stage4c = match (stage4a && stage4b, efuse.as_ref(), _txpwr.as_ref()) {`

```
// ── Stufe 4c: rtw_set_channel ────────────────────────────────
```

## L799 · `let stage5a = if stage4c {`

```
// ── Stufe 5a: der Empfangsweg ────────────────────────────────
```

## L807 · `let stage5b = match (stage5a, efuse.as_ref()) {`

```
// ── Stufe 5b: der Sendeweg ───────────────────────────────────
```

## L816 · `let mut target: Option<Bss> = None;`

```
// ── Stufe 5c: der Suchlauf ───────────────────────────────────
```

## L829 · `let stage5d = match (stage5c, efuse.as_ref()) {`

```
// ── Stufe 5d: die RF-Kalibrierung ────────────────────────────
```

## L838 · `let mut linked: Option<vif::Vif> = None;`

```
// ── Stufe 5e: Auth und Assoc ─────────────────────────────────
```

## L855 · `let mut rates: Option<(sta::PeerCaps, sta::StaInfo)> = None;`

```
// ── Stufe 5f: die Ratenanpassung ─────────────────────────────
```

## L866 · `let mut link: Option<Link> = None;`

```
// ── Stufe 6a: Steuerkanal, Handschlag, Datenweg ──────────────
```

## L922-930 · `if stage6a {`

```
// ── Stufe 6b: stehenbleiben ──────────────────────────────────
//
// **Hier kehrt der Treiber nicht mehr zurueck.** Bis 6a lief eine
// Stufenkette, und am Ende schaltete `mac_power_off` den Chip ab — die
// Verbindung stand, die DHCP-Adresse kam, und Sekunden spaeter war
// beides weg. Ein Treiber, der seine Arbeit beendet, ist kein Treiber.
//
// Die Zusammenfassung steht deshalb DAVOR: wer nie zurueckkehrt, kann
// sie hinterher nicht mehr drucken.
```

## L934-937 · `report_connected(l, target.as_ref(), linked.as_ref());`

```
// **Die eine Zeile eines stillen Laufs.** Sie steht hier und
// nicht bei AUTHORIZED: erst hinter den Toren von 6a ist sie
// eine Aussage ueber eine VERBINDUNG und nicht ueber einen
// Zwischenstand. Wer sie liest, muss nichts weiter fragen.
```

## L939-942 · `let caps = rates.as_ref().map(|(c, _)| *c).unwrap_or_default();`

```
// **Dieselbe Schleife, derselbe Link, dieselben Zaehler.**
// 6b setzt fort, statt neu anzufangen — ein zweites
// `EV_READY` liesse `wifid` einen frischen Supplicant bauen,
// der auf ein msg1 wartet, das der AP nie wieder schickt.
```

## L944-948 · `let mut fehlschlaege = 0u32;`

```
// **Ab hier laeuft die Verbindung, und sie kann enden.**
// Bis 0.26.0 kehrte `link_pump` nie zurueck: ein Rauswurf
// hinterliess eine tote Leitung, bis jemand neu bootete.
// Jetzt ist der Weg zurueck derselbe wie der Weg hin —
// Stufe 5e und 5f, ohne den Kernel noch einmal anzumelden.
```

## L955-960 · `if end == PumpEnd::Roam {`

```
// **Der Wechsel geht denselben Weg wie ein
// Wiederverbinden** — Stufe 5e und 5f, mit einer anderen
// Zelle. `reconnect` raeumt die Schluessel, zieht den
// `Link` nach und laesst `wifid` einen frischen
// Supplicant bauen; nichts davon muessen wir zweimal
// schreiben.
```

## L983-987 · `host::say("[rtl8822ce] kein Ziel mehr — der Treiber \`

```
// **Wenn das hier je greift, endet der Treiber.**
// Der Rufer schaltet danach die MAC ab, und die
// Verbindung bleibt fuer immer unten. Es darf nicht
// still geschehen — Florian: „zwar versucht aber
// blieb stumm".
```

## L998-1013 · `if fehlschlaege >= RESCAN_AFTER_TRIES {`

```
// **Nach zwei Fehlschlaegen suchen wir NEU.**
//
// Bis 0.55.1 ging der Weg zurueck immer auf DIESELBE
// BSSID und denselben Kanal — die eine Zelle, die der
// Suchlauf beim Start gewaehlt hatte. Wer aus ihrer
// Reichweite laeuft, versuchte es von da an endlos bei
// einem AP, der nicht mehr da ist. Und genau dieser
// Fall wurde mit der Verbindungswache aus 0.55.0 erst
// erreichbar: vorher blieb die tote Leitung einfach
// stehen.
//
// Ein voller Suchlauf ist hier richtig und nicht zu
// teuer: die Verbindung ist ohnehin weg, es gibt nichts
// zu unterbrechen. **Waehrend sie STEHT**, waere er es —
// das ist Teil C und bekommt einen gerichteten Lauf auf
// den bekannten Kanaelen.
```

## L1021-1022 · `host::sleep_ms(RECONNECT_BACKOFF_MS);`

```
// Nicht aufgeben, aber auch nicht im Kreis rennen:
// ein AP, der gerade neu startet, braucht Sekunden.
```

## L1028-1030 · `mac::mac_power_off(h, hal.cut_version);`

```
// Ab hier nur noch, wenn eine Stufe NICHT steht: der Kernel gibt
// gleich die DMA-Puffer frei, in die eine laufende Firmware sonst
// weiterschriebe.
```

## L1034-1042 · `host::say("[rtl8822ce] fertig — Chip ist aus, Geraet freigegeben\n");`

```
// Zurueckkehren, nicht schlafen. Der Kernel raeumt danach auf: DMA
// freigeben, PCI loesen, und den Bericht loeschen — letzteres mit der
// ausdruecklichen Begruendung, dass die Zahlen eines toten Treibers nicht
// wie lebende aussehen duerfen. Das Ergebnis dieser Stufen steht deshalb
// HIER im Terminal und nicht in `wlan`.
// Der Chip ist hier bereits aus (Stufe 1 schaltet ihn zuletzt ab), also
// kann niemand mehr in die gleich freigegebenen Puffer schreiben.
// **Hierher kommt nur ein Lauf, der NICHT steht** — mit Verbindung
// kehrt 6b nie zurueck. Also laut, auch ohne `debug: 1`.
```

## L1046-1057 · `fn stage4b_board_info_setup(rfe_option: u8) -> (bool, Option<txpower::TxPower>) {`

```
/// main.c:2064-2081 `rtw_chip_board_info_setup` — Stufe 4b.
///
/// **Sie steht hier und nicht spaeter, weil sie in Linux hier steht:**
/// `rtw_chip_info_setup` ruft `parameter_setup`, dann `efuse_info_setup`,
/// dann `board_info_setup` — alles zur Probe-Zeit, VOR `rtw_power_on`. Sie
/// fasst kein Register an; sie fuellt die Tabellen, aus denen
/// `rtw_set_channel` spaeter die Sendeleistung rechnet.
///
/// Das Gate braucht deshalb kein Geraet: `gen_tables.py` rechnet dieselbe
/// Kette in Python nach und legt Pruefsummen ueber die 25 KiB abgeleiteten
/// Zustand ab. Stimmt eine nicht, ist ein einzelnes Byte anders — und das
/// faellt hier auf statt als schiefe Sendeleistung auf einem Kanal.
```

## L1102 · `host::print("  Beispiel: FCC/20MHz/CCK/Kanal 1 -> ");`

```
// Eine Zahl zum Anfassen: die Grenze fuer FCC, 20 MHz, CCK, Kanal 1.
```

## L1126-1131 · `fn power_on_and_mac_init(`

```
/// main.c:1374-1411 `rtw_power_on`, bis einschliesslich `rtw_mac_init`.
///
/// Was danach kommt (`phy_set_param`, `mac_postinit`, `hci_start`, die
/// H2C-Nachrichten und die Koexistenz) ist Stufe 3b und 3c — es steht hier
/// bewusst NICHT als Platzhalter, damit niemand eine halbe Kette fuer eine
/// ganze haelt.
```

## L1135 · `pci::setup(h, trx, false);`

```
// rtw_hci_setup
```

## L1138 · `let t0 = host::now_us();`

```
// rtw_mac_power_on
```

## L1150-1151 · `if stage_buf < 0 {`

```
// rtw_wait_firmware_completion entfaellt: unsere Firmware liegt im
// Binaerbild, es gibt kein asynchrones Nachladen, auf das zu warten waere.
```

## L1153 · `if stage_buf < 0 {`

```
// rtw_download_firmware
```

## L1168 · `let t0 = host::now_us();`

```
// rtw_mac_init
```

## L1187-1188 · `host::print("  Seitenplan: txff ");`

```
// Der Seitenplan im Klartext. Er ist die Zahl, an der ab jetzt jede
// Reserved Page haengt — und er steht nirgendwo sonst.
```

## L1213-1214 · `let llt = host::r8(h, REG_AUTO_LLT_V1) & BIT_AUTO_INIT_LLT_V1 as u8;`

```
// Die Gates der Stufe: die zwei Quittungen der Hardware und die zwei
// Zahlen, die aus Linux' eigener Rechnung fallen.
```

## L1220 · `let mix = host::r32(h, REG_HCI_MIX_CFG);`

```
// rtw_pci_interface_cfg auf cut >= D.
```

## L1224 · `let cr = host::r8(h, REG_CR);`

```
// Der MAC laeuft: REG_CR traegt alle acht TRX-Bits.
```

## L1230-1239 · `fn phy_set_param_and_check(h: i32, hal: &Hal, e: &efuse::Efuse, d: &mut Dev)`

```
/// main.c:1413 `chip->ops->phy_set_param` — Stufe 3b (Tabellen) und
/// 3c (BB/RF-Aufbau) in einem Zug, weil `rtw_phy_load_tables` MITTEN in
/// `rtw8822c_phy_set_param` steht und nicht daneben.
///
/// Die zwei Gates sind getrennt, weil sie verschiedene Fragen stellen:
/// **3b** — kommt aus den Tabellen ueberhaupt etwas an? Gemessen wird das
/// am RF-Register 0x00 BEIDER Pfade: es traegt nach dem Laden den Wert, den
/// die Tabelle hineingeschrieben hat, und ist weder 0 noch 0xfffff.
/// **3c** — hoert der Empfaenger? Gemessen an `false_alarm_statistics`:
/// die CCA-Zaehler des Chips laufen nur, wenn die BB arbeitet.
```

## L1255-1257 · `let mut rf_ok = true;`

```
// ── Gate 3b ──────────────────────────────────────────────────
// `rtw_phy_read_rf` geht ueber das direkte Fenster; ein Pfad, der nicht
// antwortet, liefert 0xfffff (alle Bits) oder 0.
```

## L1276-1296 · `let stage3c = gate("phy_set_param lief durch, beide RF-Pfade antworten",`

```
// ── Gate 3c ──────────────────────────────────────────────────
// **Das Gate stand in 0.10.0 an der falschen Stelle**, und in 0.10.1
// stand es an der zweiten falschen Stelle. Beide Fehler sind derselbe:
// eine Bedingung erfinden, die Linux nicht hat.
//
// 0.10.0 fragte nach CCA-Ereignissen. Die kann es hier nicht geben,
// auch in Linux nicht: `false_alarm_statistics` laeuft dort erst im
// Wachhund (main.c:280), also NACH `rtw_coex_power_on_setting` (das die
// gemeinsame Antenne umlegt) und NACH `rtw_set_channel` (das AGC,
// CCA-Maske und RX-Filter programmiert). Beides ist Stufe 4.
//
// 0.10.1 fragte nach der Konvergenz der DAC-Kalibrierung. **Linux
// prueft das nirgends** — `rtw8822c_rf_dac_cal` laeuft zehnmal und geht
// weiter, ob der Restversatz unter 5 faellt oder nicht. Wir haben kein
// Linux auf diesem Geraet, koennen also nicht wissen, was dort
// herauskaeme. Ein Gate ohne Vergleichsmass ist eine Meinung
// ([[feedback_a_test_of_a_state_must_say_when]]).
//
// Was Stufe 3c beantworten KANN: lief `phy_set_param` durch und
// antworten beide RF-Pfade mit dem, was die Tabellen hineingeschrieben
// haben. Die Konvergenz steht als BEFUND darunter, mit Zahlen.
```

## L1306-1308 · `chip::false_alarm_statistics(h, dm);`

```
// Gemessen, aber NICHT gewertet: die Zaehler stehen hier
// erwartungsgemaess auf 0. Sie stehen trotzdem im Log, weil sie ab
// Stufe 4 das Gate sind und man dann die Ausgangslage kennen will.
```

## L1342-1354 · `fn stage4a_power_on_tail(h: i32, hal: &Hal, trx: &mut pci::Trx, h2c_buf: i32,`

```
/// main.c:1413-1434 der Rest von `rtw_power_on`, dann main.c:1517-1533
/// `rtw_core_start` bis zum RCR-Schreibzugriff.
///
///     rtw_mac_postinit        beim 8822C NULL (rtw8822c.c:4967) -> nichts
///     rtw_hci_start           = rtw_pci_start: schaltet NUR Interrupts
///                               frei. Wir pollen -> BENANNTE ABWEICHUNG,
///                               siehe docs/plan/WIFI_RTL8822CE.md
///     rtw_fw_send_general_info    H2C-PAKET durch die H2C-Queue
///     rtw_fw_send_phydm_info      dito
///     rtw_coex_power_on_setting   Antenne auf BT
///     rtw_coex_init_hw_config     danach auf INIT
///     rtw_sec_enable_sec_engine
///     rtw_write32(REG_RCR, hal->rcr)
```

## L1367-1368 · `let vec = host::irq_register();`

```
// `rtw_mac_postinit`: `chip->ops->mac_postinit` ist beim 8822C NULL,
// die Funktion kehrt ohne einen Registerzugriff zurueck.
```

## L1370-1374 · `let vec = host::irq_register();`

```
// `rtw_hci_start` = `rtw_pci_start`: setzt `rtwpci->running` und ruft
// `rtw_pci_enable_interrupt`. Den MSI melden wir hier an
// (`rtw_pci_request_irq`: EIN Vektor). HIMR schalten wir erst in der
// Pumpschleife scharf, im Leerlauf — waehrend des Hochfahrens arbeitet
// der Treiber seine Schritte der Reihe nach ab und wartet auf nichts.
```

## L1376 · `unsafe { IRQ_VEC = vec; }`

```
// SAFETY: nur dieser Fiber schreibt und liest IRQ_VEC.
```

## L1386-1388 · `let wp_before = trx.tx[pci::Q_H2C].wp;`

```
// ── Die zwei H2C-PAKETE ──────────────────────────────────────
// Sie gehen durch die H2C-QUEUE, nicht durch die Mailbox. Der Ring
// dafuer steht seit Stufe 3a (`init_h2c`).
```

## L1403-1406 · `let (consumed, dt_h2c, hw_idx) =`

```
// Der Chip holt die Eintraege selbst ab: die oberen zwoelf Bit des
// Indexregisters sind SEIN Lesezeiger. **Gewartet, nicht gestochert** —
// in 0.11.0 stand er auf 1, waehrend unserer schon auf 2 stand, und das
// war nur die Zeit zwischen Anstoss und Lesung.
```

## L1419-1421 · `let wifi_only = !e.btcoex;`

```
// ── Die Koexistenz, und damit die ANTENNE ────────────────────
// `wifi_only = !efuse->btcoex` (main.c:1431). Unsere efuse sagt
// btcoex JA, also ist es false und der INIT-Zweig gilt.
```

## L1440-1441 · `host::print("  Score-Board roh: vorher 0x");`

```
// ROH, ohne die Maske von `read_scbd` — sonst laesst sich eine 0 nicht
// von „unser eigener Schreibzugriff kam nie an" unterscheiden.
```

## L1458 · `let ant = coex::read_ant_state(h);`

```
// **Das ist die Sache, um die es geht.** Wo steht die Antenne?
```

## L1470 · `sec::enable_sec_engine(h);`

```
// ── rtw_core_start ───────────────────────────────────────────
```

## L1481 · `let mut ok = true;`

```
// ── Gates ────────────────────────────────────────────────────
```

## L1486-1493 · `let (want_wl, want_bt) = if cx.bt_disabled {`

```
// **Das Score-Board ist KEIN Gate.** Es ist ein gemeinsames Postfach:
// die Bits, die uns interessieren wuerden, schreibt der BT-Kern. Laeuft
// der nicht, steht dort 0 — und das ist dann wahr, nicht falsch. Linux
// prueft es nirgends. Was wir pruefen koennen, ist die Wirkung:
//
// Bei `bt_disabled` nimmt `set_ant_path(COEX_SET_ANT_INIT)` den Zweig
// GNT_BT = SW_LOW (1), GNT_WL = SW_HIGH (3) — die Antenne geht an
// WLAN. Laeuft BT, ist es umgekehrt, und dann teilt die PTA sie.
```

## L1504 · `let dm = &mut d.dm;`

```
// ── Und die Frage, fuer die 4a da ist ────────────────────────
```

## L1527-1543 · `fn stage4c_set_channel(h: i32, hal: &Hal, e: &efuse::Efuse,`

```
/// main.c:1440-1476 `rtw_set_channel` — Stufe 4c.
///
///     rtw_get_channel_params      aus der Kanalwahl werden Mittenkanal,
///                                 Bandbreite und Unterkanallage
///     rtw_update_channel          derselbe Zustand in `hal`
///     chip->ops->set_channel      = rtw8822c_set_channel: BB, MAC, RF,
///                                   toggle_igi
///     rtw_coex_switchband_notify  BENANNTE ABWEICHUNG, siehe unten
///     rtw_phy_set_tx_power_level  aus den Tabellen von 4b wird ein
///                                 Leistungsindex je Rate und Pfad
///
/// **Der Kanal kommt hier von uns, nicht von mac80211.**
/// `rtw_get_channel_params` liest in Linux eine `cfg80211_chan_def`; die
/// gibt es ohne obere Haelfte nicht. Fuer 20 MHz ist das Ergebnis dieser
/// Funktion genau `center = primary = Kanal`, und das ist, was hier
/// eingesetzt wird — die Rechnung fuer 40 und 80 MHz kommt mit der Stufe,
/// die eine Bandbreite auswaehlt.
```

## L1546-1547 · `const CH: u8 = 1;`

```
// Kanal 1, 20 MHz. Der niedrigste 2,4-GHz-Kanal ist der, auf dem am
// ehesten jemand funkt — und genau darum geht es beim Messen.
```

## L1549 · `const BW: usize = 0; // RTW_CHANNEL_WIDTH_20`

```
// RTW_CHANNEL_WIDTH_20
```

## L1556-1558 · `let mut t2 = txpower::TxPower { cch_by_bw: t.cch_by_bw, ..*t };`

```
// `rtw_update_channel` — bei 20 MHz ist der Mittenkanal der primaere,
// und `cch_by_bw[20M]` traegt ihn. Der Rest von `hal` (sar_band,
// current_band_*) wird hier als lokale Groesse gefuehrt.
```

## L1566-1569 · `let t0 = host::now_us();`

```
// `rtw_coex_switchband_notify` gehoert zur laufenden Koexistenz
// (`rtw_coex_run_coex` mit COEX_RSN_2GSWITCHBAND) und braucht den
// Verkehrszustand, den erst eine Verbindung hat. BENANNT UND NICHT
// GEBAUT — er entscheidet nicht, ob der Empfaenger hoert.
```

## L1571 · `let t0 = host::now_us();`

```
// `rtw_phy_set_tx_power_level`
```

## L1603 · `let rf18_a = phy::read_rf(h, phy::RF_PATH_A, 0x18, phy::RFREG_MASK);`

```
// RF 0x18 traegt jetzt Band, Kanal und Bandbreite — zurueckgelesen.
```

## L1619 · `ok &= gate("RF 0x18 traegt die Bandbreite 20 MHz",`

```
// 20 MHz ist RF18_BW_20M = BIT(13)|BIT(12), also 0x3 im Feld.
```

## L1623 · `let dm = &mut d.dm;`

```
// ── Das Gate, das seit 0.10.0 auf seine Stufe gewartet hat ───
```

## L1664-1665 · `fn pwr_cmds_for_us(cut: u8) -> usize {`

```
/// Wieviele der 54 Kommandos auf UNSEREM Geraet ueberhaupt laufen. Eine
/// Zahl, die man sonst erst beim Suchen vermisst.
```

## L1682-1687 · `fn stage5a_rx(h: i32, hal: &Hal, trx: &mut pci::Trx, d: &mut Dev) -> bool {`

```
/// Stufe 5a — der WIRT um `pci::rx_poll` herum, nicht der Port selbst.
///
/// Der Port ist `pci::rx_poll` (= `rtw_pci_rx_napi`); hier steht nur, wie
/// lange gefragt wird und was gemeldet wird. Linux laeuft dort aus dem
/// Interrupt in NAPI; wir haben keine Geraete-Interrupts (benannte
/// Abweichung seit Stufe 2), also wird der Schreibzeiger gelesen.
```

## L1689 · `const CH_5A: u8 = 1;`

```
/// Derselbe Kanal wie in Stufe 4c — `hal.current_channel`.
```

## L1694-1696 · `let dm = &mut d.dm;`

```
// `dm_info` traegt die CCK-Verstaerkungsgrenzen, an denen ein
// CCK-Paket seine Signalstaerke bekommt. Sie stehen in der Hardware,
// seit `phy_set_param` sie dort gelesen hat.
```

## L1706-1707 · `static mut RXBUF: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =`

```
// Ein ganzer Empfangspuffer. Er liegt statisch, weil dieser Treiber
// keinen Allokator hat und 11 KB auf dem Stapel nicht stehen.
```

## L1710-1712 · `let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF) };`

```
// SAFETY: Einfaeden, ein Rufer, und der Puffer verlaesst diese
// Funktion nicht. Es gibt in diesem Treiber keinen zweiten Pfad, der
// ihn anfasst.
```

## L1722-1724 · `let t0 = host::now_us();`

```
// 2000 ms. Ein Beacon-Intervall sind 102,4 ms, also kommt in dieser
// Zeit von JEDEM erreichbaren Netz mehr als ein Rahmen — wenn der Weg
// traegt. Kommt in zwei Sekunden nichts, ist es kein Timing-Problem.
```

## L1741 · `if shown < 8 && !st.crc_err {`

```
// Die ersten acht ganz, damit man SIEHT, was ankommt.
```

## L1776-1778 · `fn print_pkt(st: &rx::RxPktStat, pkt: &[u8]) {`

```
/// Eine Zeile je Paket: Laenge, Rate, Bandbreite, Kanal, Signal — und die
/// ersten Bytes des Rahmens, denn an `frame_control` sieht man, ob es ein
/// Beacon ist.
```

## L1804 · `if fc & 0xfc == 0x80 {`

```
// 802.11: Typ in Bits 3:2, Subtyp in 7:4. 0x80 = Beacon.
```

## L1822-1835 · `fn stage5b_tx(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,`

```
/// Stufe 5b — der Sendeweg, und die Antwort darauf.
///
/// Der Port bekommt seine Adresse (`rtw_ops_add_interface`), dann geht ein
/// Probe Request hinaus und wir hoeren zu. **Das Gate ist die ANTWORT**,
/// nicht der verbrauchte Deskriptor: dass der Chip einen Deskriptor abholt,
/// sagt nur, dass DMA laeuft — dass ein fremder AP antwortet, sagt, dass
/// der Rahmen die Antenne verlassen hat und richtig gebaut war.
///
/// **Kalibriert wird hier bewusst nicht.** `rtw_set_channel` setzt am Ende
/// `need_rfk = true`, und `rtw_chip_prepare_tx` fuehrt GAPK/IQK/DPK erst
/// aus, wenn mac80211 `mgd_prepare_tx` ruft — also VOR dem Anmelden, nicht
/// beim Kanalwechsel. Linux' Kommentar nennt den Grund: waehrend eines
/// Scans auf jedem Kanal zu kalibrieren dauert zu lange. Ein Probe Request
/// geht in Linux genauso unkalibriert hinaus wie hier.
```

## L1844-1846 · `let vif = vif::add_interface_station(h, mac);`

```
// `rtw_ops_add_interface`, Zweig STATION. Ohne diesen Schritt steht im
// Port-Register keine Adresse, und `BIT_APM` im RCR laesst dann nur
// Broadcast durch — eine Probe Response ist an UNS gerichtet.
```

## L1868-1870 · `let mut frame = [0u8; 128];`

```
// Ein Probe Request. Das baut in Linux `ieee80211_build_probe_req` —
// die OBERE Haelfte, die hier `wifid` wird. Er steht hier, weil der
// Sendeweg sonst nichts zu senden haette; 5c loest ihn ab.
```

## L1892-1893 · `let mut sent = 0u32;`

```
// Dreimal, mit Abstand: ein einzelner Probe Request kann kollidieren,
// und ein AP darf ihn auch schlicht verwerfen.
```

## L1926 · `let dm = &mut d.dm;`

```
// Und jetzt zuhoeren. Eine Probe Response ist Subtyp 5.
```

## L1932-1933 · `let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF2) };`

```
// SAFETY: wie in Stufe 5a — ein Faden, ein Rufer, der Puffer verlaesst
// diese Funktion nicht.
```

## L1951 · `if fc & 0xfc != 0x50 {`

```
// Subtyp 5 = Probe Response, Typ 0 = Verwaltung.
```

## L1955-1956 · `if pkt[off + 4..off + 10] != mac[..] {`

```
// `addr1` ist unsere Adresse — sonst haette der Filter ihn
// gar nicht durchgelassen, aber gemessen ist besser.
```

## L1980-1984 · `fn build_probe_req(out: &mut [u8; 128], mac: &[u8; 6], ch: u8) -> usize {`

```
/// `ieee80211_build_probe_req` in klein: ein Wildcard-Probe-Request.
///
/// Die Sequenznummer bleibt null — `en_hwseq` steht im Deskriptor, also
/// vergibt sie der Chip. Gebaut wird nur, was ein AP zum Antworten
/// braucht: die drei Adressen, das leere SSID-Element und die Raten.
```

## L1989-1999 · `fn build_probe_req_to(out: &mut [u8; 128], mac: &[u8; 6], ch: u8,`

```
/// `ieee80211_build_probe_req` mit `IEEE80211_PROBE_FLAG_DIRECTED`
/// (net/mac80211/util.c, gerufen aus `ieee80211_ap_probereq_get`,
/// mlme.c:4518-4521).
///
/// **Ein GERICHTETER Probe Request ist die Frage „lebst du noch?" an
/// genau einen AP** — Empfaenger und BSSID sind seine Adresse, und das
/// SSID-Element traegt seinen Namen statt der Null-Laenge. Nur er
/// beantwortet sie, und niemand sonst auf dem Kanal muss antworten.
///
/// Der Suchlauf ruft weiter ohne Ziel: dort ist die leere SSID die
/// Frage „wer ist da?".
```

## L2004 · `out[0..2].copy_from_slice(&0x0040u16.to_le_bytes()); // Verwaltung, Subtyp 4`

```
// Verwaltung, Subtyp 4
```

## L2005 · `out[2..4].copy_from_slice(&0u16.to_le_bytes()); // duration`

```
// duration
```

## L2006 · `out[4..10].copy_from_slice(ziel); // addr1 = Empfaenger`

```
// addr1 = Empfaenger
```

## L2007 · `out[10..16].copy_from_slice(mac); // addr2 = wir`

```
// addr2 = wir
```

## L2008 · `out[16..22].copy_from_slice(ziel); // addr3 = BSSID`

```
// addr3 = BSSID
```

## L2009 · `out[22..24].copy_from_slice(&0u16.to_le_bytes()); // seq, siehe oben`

```
// seq, siehe oben
```

## L2011 · `let sl = ssid.len().min(32);`

```
// SSID-Element. Laenge 0 = „jedes Netz", sonst der Name des einen.
```

## L2017-2018 · `out[n] = 1;`

```
// Supported Rates: 1, 2, 5.5, 11, 6, 9, 12, 18 Mbit. Das hohe Bit
// markiert eine GRUNDrate.
```

## L2024 · `out[n] = 3;`

```
// DS Parameter Set: der Kanal, auf dem wir fragen.
```

## L2031-2034 · `fn print_last(b: &Bss) {`

```
/// Die Kanalauslastung, die eine Zelle SELBST meldet — in Prozent.
///
/// Fehlt das Element, steht nichts da: eine erfundene Null waere eine
/// Aussage.
```

## L2044-2045 · `fn print_probe_resp(f: &[u8], st: &rx::RxPktStat) {`

```
/// Eine Zeile je Antwort: BSSID, Signal und der Netzname aus dem
/// SSID-Element. Ein Name macht aus „ein Rahmen kam" ein „wir sehen X".
```

## L2052 · `host::print_hex8(f[16 + i]); // addr3 = BSSID`

```
// addr3 = BSSID
```

## L2057-2058 · `let mut i = 36;`

```
// 24 Kopf + 12 feste Felder (Zeitstempel, Intervall, Faehigkeiten),
// dann die Elemente. Das SSID-Element hat die Kennung 0.
```

## L2078-2079 · `#[derive(Clone, Copy)]`

```
/// Eine gefundene Funkzelle. Nur das, was aus Beacon oder Probe Response
/// sicher herausfaellt — nichts Abgeleitetes.
```

## L2089 · `capability: u16,`

```
/// Faehigkeitsfeld aus Beacon/Probe Response (802.11 §9.4.1.4).
```

## L2091-2092 · `rsn: [u8; 64],`

```
/// Das RSN-Element des AP, roh. Daraus waehlt der Anmeldeantrag
/// SEINE Verfahren — ein AP lehnt sonst mit Status 43 ab.
```

## L2095-2099 · `ht_param: u8,`

```
/// Byte 1 des HT-Operation-Elements (802.11 §9.4.2.56, id 61):
/// Bit 1:0 die Lage des Zweitkanals, Bit 2 ob der AP ueberhaupt
/// breiter als 20 MHz zulaesst. **Ohne dieses Byte gibt es kein
/// HT40** — welche HAELFTE die breite Zelle belegt, sagt allein der
/// AP, und eine geratene Haelfte ist ein anderer Kanal.
```

## L2101-2103 · `ht_op_seen: bool,`

```
/// **War das HT-Operation-Element ueberhaupt da?** `ht_param == 0`
/// heisst sonst zweierlei: „der AP faehrt 20 MHz" ODER „wir haben das
/// Element nie gesehen". Das sind zwei verschiedene Baustellen.
```

## L2105-2108 · `ht_cap: u16,`

```
/// Byte 0:1 des HT-CAPABILITIES-Elements (id 45). Bit 1 ist
/// `SUP_WIDTH_20_40`: was der AP KANN. Das HT-Operation-Element sagt,
/// was er gerade TUT. Nur beide nebeneinander beantworten die Frage,
/// ob 20 MHz seine Entscheidung oder unsere Luecke ist.
```

## L2110-2114 · `vht_chanwidth: u8,`

```
/// Byte 0 des VHT-Operation-Elements (802.11 §9.4.2.158, id 192):
/// die Breite, die die Zelle FAEHRT. `0` = „nimm die HT-Angabe",
/// `1` = 80 MHz (und, mit Segment 1, auch 160 und 80+80), `2` und `3`
/// sind die mit 802.11-2016 ABGESCHAFFTEN Kodierungen fuer 160 und
/// 80+80.
```

## L2116-2121 · `vht_cch0: u8,`

```
/// Byte 1: Mittenkanal-Segment 0. Bei Breite `1` ist das die Mitte
/// der PRIMAEREN 80 MHz — auch dann, wenn der AP 160 faehrt. Genau
/// dafuer ist das Feld da: wer nur 80 kann, findet hier seinen Kanal,
/// ohne die 160 zu verstehen (802.11 Tabelle 9-250; in mac80211
/// `ieee80211_chandef_vht_oper`, Fall `supp_chwidth == 0` →
/// `ccf1 = 0` → `center_freq1 = cf0`).
```

## L2123-2126 · `vht_cch1: u8,`

```
/// Byte 2: Mittenkanal-Segment 1 — bei 160 MHz die Mitte der ganzen
/// 160. Wir LESEN es nur fuer den Bericht: unsere VHT-Faehigkeiten
/// sagen „kein 160", und dann ist die Antwort laut derselben Tabelle
/// Segment 0.
```

## L2128-2130 · `vht_op_seen: bool,`

```
/// War das VHT-Operation-Element ueberhaupt da? Dieselbe Frage wie
/// bei `ht_op_seen`, und aus demselben Grund: ohne sie sieht „die
/// Zelle faehrt 20/40" aus wie „wir lesen das Element nicht".
```

## L2132-2151 · `ap_vht_cap: u32,`

```
/// Byte 2 des BSS-Load-Elements (802.11 §9.4.2.26, id 11):
/// **wieviel Prozent der Zeit der AP seinen Kanal belegt SIEHT**,
/// als 0..255.
///
/// **Das ist die einzige Zahl im Beacon, die einen Repeater
/// verraten kann.** Er teilt sich die Luft mit seinem eigenen
/// Backhaul zur Basis — jedes Byte geht zweimal durch den Aether —
/// und sieht seinen Kanal deshalb deutlich voller als eine Basis am
/// Kabel. Pegel und Bandbreite sehen das NICHT: am Geraet meldete
/// der Repeater 866 Mbit bei -23 dBm und lieferte 222, weil 53 % der
/// Zeit sein Backhaul lief.
///
/// **Vorerst wird der Wert nur GEZEIGT und geht in keine
/// Entscheidung ein.** wpa_supplicant wertet ihn auch nicht aus: an
/// der Stelle, wo es hingehoerte, steht in `scan.c:3425` woertlich
/// `TODO: channel utilization and AP load (e.g., from AP Beacon)`.
/// Es gibt hier also keine Referenz — und eine Regel ohne Quelle und
/// ohne Messung waere geraten.
/// Das Feld „VHT Capabilities Info" des AP aus seiner Bake.
/// Nur dafuer da, unser eigenes Angebot daran zu stutzen.
```

## L2156-2160 · `wmm: bool,`

```
/// **Sagt der AP WMM an?** `bss->wmm_used` in mac80211
/// (scan.c:139: `elems->wmm_param || elems->wmm_info`). Davon haengt
/// ab, ob unser Anmeldeantrag das WMM-Element traegt — und davon
/// wiederum, ob ein AP uns VHT gibt: hostapd streicht einer Station
/// ohne WMM-Element VHT (`copy_sta_vht_capab`, ieee802_11_vht.c:200).
```

## L2166-2174 · `#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]`

```
/// Was die ZELLE ueber ihre Breite sagt — die rohen Bytes aus ihren
/// Operation-Elementen, nicht deren Deutung.
///
/// Es steht als eigener Wert da, weil `chan_params` und `switch_channel`
/// inzwischen DREI Angaben brauchen und der Suchlauf keine davon hat.
/// Drei Bytes einzeln durchzureichen hiesse, an jeder Rufstelle die
/// Reihenfolge richtig zu treffen; `CellWidth::default()` ist „ich weiss
/// nichts ueber diese Zelle", und das ist genau der Zustand des
/// Suchlaufs.
```

## L2177 · `ht_param: u8,`

```
/// Byte 1 des HT-Operation-Elements (id 61).
```

## L2179 · `vht_chanwidth: u8,`

```
/// Byte 0 des VHT-Operation-Elements (id 192).
```

## L2181 · `vht_cch0: u8,`

```
/// Byte 1 des VHT-Operation-Elements: Mittenkanal-Segment 0.
```

## L2195-2198 · `const CSA_BEACON_WAIT_MS: u64 = 1000;`

```
/// Wie lange wir nach einem Kanalwechsel auf eine Bake warten, bevor
/// wir umkehren. Ein Bakenintervall sind 102 ms; zehn davon sind
/// reichlich und immer noch eine Zehntelsekunde schneller als die
/// Verbindungswache.
```

## L2201-2207 · `const VHT_CHANWIDTH_80: u8 = 1;`

```
/// `IEEE80211_VHT_CHANWIDTH_80MHZ` — der EINZIGE Wert, aus dem wir eine
/// Breite ableiten. `USE_HT` (0) faellt auf HT zurueck; `160MHZ` (2) und
/// `80P80MHZ` (3) sind die abgeschafften Kodierungen, in denen Segment 0
/// die Mitte der ganzen 160 traegt statt die unserer 80 — daraus unsere
/// Haelfte zu RECHNEN waere eine Behauptung ueber einen Fall, den seit
/// 802.11-2016 kein AP mehr sendet und den wir nie gemessen haben. Sie
/// fallen deshalb auf HT zurueck und werden im Bericht genannt.
```

## L2210-2213 · `const CENTERS_80: [u8; 7] = [42, 58, 106, 122, 138, 155, 171];`

```
/// Die zulaessigen Mittenkanaele eines 80-MHz-Blocks im 5-GHz-Band.
/// Sie liegen fest im Raster (802.11 Anhang E), jeder deckt vier
/// 20-MHz-Kanaele: 36-48, 52-64, 100-112, 116-128, 132-144, 149-161,
/// 165-177.
```

## L2216-2228 · `#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]`

```
// ═══════════════════════════════════════════════════════════════
// Kanalwechsel — CSA (802.11 §11.9, mac80211 spectmgmt.c:220-330 und
// mlme.c:2742-3024)
//
// **Ein AP darf umziehen, und er sagt es vorher an.** Auf einem
// DFS-Kanal ist das kein Sonderfall: erkennt er Radar, MUSS er den
// Kanal binnen Sekunden raeumen (ETSI EN 301 893). Ein Client, der die
// Ansage nicht liest, bleibt auf dem leeren Kanal zurueck und merkt es
// erst, wenn die Baken ausbleiben — bei uns nach Sekunden, und dann
// mit dem vollen Wiederverbinden.
//
// Wir horchen auf Kanal 104. Das ist DFS.
// ═══════════════════════════════════════════════════════════════
```

## L2230 · `#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]`

```
/// Was eine Wechselansage sagt.
```

## L2233-2234 · `mode: u8,`

```
/// `mode` (802.11 §9.4.2.19). **1 heisst: ab jetzt nichts mehr
/// senden**, bis der Wechsel vollzogen ist — der AP raeumt gerade.
```

## L2236 · `channel: u8,`

```
/// Der neue primaere Kanal.
```

## L2238-2239 · `count: u8,`

```
/// In so vielen Bakenintervallen ist es soweit. `0` und `1` heissen
/// beide „jetzt" (mlme.c:2991: `(max(count, 1) - 1) * beacon_int`).
```

## L2241 · `width: CellWidth,`

```
/// Und wie breit es danach weitergeht.
```

## L2245-2261 · `fn parse_csa(f: &[u8]) -> Option<Csa> {`

```
/// `ieee80211_parse_ch_switch_ie` (spectmgmt.c:220), auf das
/// zusammengezogen, was ein Beacon traegt.
///
/// Gelesen werden vier Elemente:
/// * **37** Channel Switch Announcement — `{mode, neuer Kanal, count}`
/// * **60** Extended CSA — dasselbe plus Betriebsklasse davor
/// * **62** Secondary Channel Offset — wo der Zweitkanal danach liegt
/// * **194** Wide Bandwidth Channel Switch — VHT-Breite und Mitte,
///   allein oder im Wrapper **196**
///
/// **Eine Abweichung, und sie ist benannt:** Linux zieht die
/// Betriebsklasse des Elements 60 heran, um einen BANDwechsel zu
/// erkennen (`ieee80211_operating_class_to_band`). Wir fuehren keine
/// Betriebsklassen; deshalb gilt bei uns Element 37, wenn es da ist,
/// und 60 nur als Ersatz. Eine Ansage, die uns in ein anderes Band
/// schicken will, faellt damit auf die Plausibilitaetspruefung des
/// Kanals zurueck — dieselbe, die auch `chan_params` schuetzt.
```

## L2263-2264 · `if f.len() < 36 {`

```
// Beacon: 24 Kopf + 8 Zeitstempel + 2 Bakenintervall + 2
// Faehigkeiten, dann die Elemente.
```

## L2287 · `csa.mode = b[0];`

```
// b[1] ist die Betriebsklasse, die wir nicht fuehren.
```

## L2297 · `196 => {`

```
// 196 Channel Switch Wrapper — darin steckt 194.
```

## L2318 · `return None;`

```
// spectmgmt.c:278 „nothing here we understand"
```

## L2321-2323 · `csa.width.ht_param = match sec_offs {`

```
// Der Zweitkanal-Versatz wird in ein HT-Operation-Byte uebersetzt,
// damit `chan_params` ihn versteht — eine Rechnung, eine Antwort.
// Bit 2 (`WIDTH_ANY`) muss stehen, sonst gilt 20 MHz.
```

## L2325 · `1 => 0x05, // Zweitkanal OBEN`

```
// Zweitkanal OBEN
```

## L2326 · `3 => 0x07, // Zweitkanal UNTEN`

```
// Zweitkanal UNTEN
```

## L2327-2329 · `_ => 0x00,`

```
// spectmgmt.c:307-310: ohne das Element wissen wir die Lage
// nach dem Wechsel nicht, und dann ist 20 MHz das Beste, was
// wir sagen koennen.
```

## L2335-2365 · `fn chan_params(primary: u8, w: CellWidth, max_bw: usize) -> (u8, usize, u8) {`

```
/// main.c:822-867 `rtw_get_channel_params` und main.c:759-792, der Teil von
/// `rtw_update_channel`, der die Unterkanallage waehlt — zusammengezogen,
/// weil beide dieselbe Fallunterscheidung fahren und wir keine `chandef`
/// haben, sondern das HT-Operation-Byte des AP.
///
/// Linux rechnet in FREQUENZEN (`primary_freq > center_freq`), wir in
/// Kanalnummern. Das ist dieselbe Aussage: ein Kanalschritt sind 5 MHz,
/// und der Vergleich dreht sich mit. Ausgeschrieben, damit es nachpruefbar
/// ist statt geglaubt:
///
/// `​``text
///   Zweitkanal OBEN  (0x1): Mitte = primaer + 2, primaer ist die UNTERE
///   Zweitkanal UNTEN (0x3): Mitte = primaer - 2, primaer ist die OBERE
/// `​``
///
/// **80 MHz** kommt aus dem VHT-Operation-Element und ist der Grund,
/// warum hier eine `CellWidth` steht und nicht mehr ein Byte:
/// `rtw_get_channel_params` unterscheidet bei 80 MHz die Lage des
/// primaeren 20ers in VIER Stufen (main.c:769-791), und die Mitte kommt
/// nicht aus einer Rechnung, sondern aus dem Element.
///
/// `​``text
///   |primaer - Mitte| == 2 : der primaere ist ein INNERES Viertel
///                            -> RTW_SC_20_UPPER / _LOWER
///   |primaer - Mitte| == 6 : er ist ein AEUSSERES
///                            -> RTW_SC_20_UPMOST / _LOWEST
/// `​``
///
/// `max_bw` ist die Obergrenze, die der RUFER erlaubt: `0` im Suchlauf
/// (dort ist jede Breite ueber 20 MHz eine Behauptung ueber den
/// Nachbarkanal), sonst das Minimum aus Karte und `bw:` aus der Konfig.
```

## L2367 · `const SEC_OFFSET: u8 = 0x03; // IEEE80211_HT_PARAM_CHA_SEC_OFFSET`

```
// IEEE80211_HT_PARAM_CHA_SEC_OFFSET
```

## L2368 · `const SEC_ABOVE: u8 = 0x01; // IEEE80211_HT_PARAM_CHA_SEC_ABOVE`

```
// IEEE80211_HT_PARAM_CHA_SEC_ABOVE
```

## L2369 · `const SEC_BELOW: u8 = 0x03; // IEEE80211_HT_PARAM_CHA_SEC_BELOW`

```
// IEEE80211_HT_PARAM_CHA_SEC_BELOW
```

## L2370 · `const WIDTH_ANY: u8 = 0x04; // IEEE80211_HT_PARAM_CHAN_WIDTH_ANY`

```
// IEEE80211_HT_PARAM_CHAN_WIDTH_ANY
```

## L2372-2380 · `if max_bw == 0 || w.ht_param & WIDTH_ANY == 0 {`

```
// **Beide Bedingungen, nicht eine.** Ein AP darf den Zweitkanal nennen
// und die Breite trotzdem verbieten (Bit 2 aus), waehrend er gerade
// einen 20-MHz-Nachbarn schuetzt. Wer nur den Versatz liest, sendet
// dann 40 MHz in eine Zelle, die 20 erwartet.
//
// **Das Bit gilt auch fuer 80 MHz.** In 802.11 heisst es „STA Channel
// Width" und sagt: alles ueber 20 MHz ist hier gerade untersagt. Ein
// VHT-Operation-Element daneben aendert daran nichts — deshalb steht
// die Pruefung VOR dem VHT-Zweig und nicht in ihm.
```

## L2385-2399 · `if max_bw >= 2 && primary > 14 && w.vht_chanwidth == VHT_CHANWIDTH_80 {`

```
// ── 80 MHz ───────────────────────────────────────────────────────
//
// **Wir nehmen Segment 0 und rechnen nichts.** Unsere VHT-Faehigkeiten
// melden `supp_chan_width = 0` (kein 160, kein 80+80), und fuer genau
// diesen Fall schreibt 802.11 Tabelle 9-250 vor, dass Segment 0 die
// Mitte UNSERER 80 MHz traegt — auch an einem 160-MHz-AP, der seine
// 160er-Mitte daneben in Segment 1 legt. mac80211 faehrt dieselbe
// Zeile (`ccf1 = 0` → `center_freq1 = cf0`).
//
// Geprueft wird wie beim 40er, und aus demselben Grund: die Eingabe
// ist ein Byte aus einem fremden Beacon. Zwei Bedingungen zusammen
// legen die Mitte eindeutig fest — sie muss ein Mittenkanal des
// 80-MHz-Rasters sein, UND der primaere muss eines ihrer vier Viertel
// sein. Passt eines von beiden nicht, gilt der 40er-Weg darunter:
// eine schmalere Breite ist immer erlaubt, eine erfundene Mitte nie.
```

## L2404-2405 · `let idx = match (primary > c, d) {`

```
// main.c:769-791, in Kanaelen statt Frequenzen: 10 MHz
// Abstand sind zwei Kanalschritte, 30 MHz sind sechs.
```

## L2416 · `let center = match w.ht_param & SEC_OFFSET {`

```
// ── 40 MHz ───────────────────────────────────────────────────────
```

## L2423-2433 · `let plausibel = if primary <= 14 {`

```
// **Eine Zutat gegenueber Linux, und hier ist der Grund.** `rtw88`
// bekommt eine `cfg80211_chan_def`, die cfg80211 vorher geprueft hat
// (`cfg80211_chandef_valid`); es RECHNET nur noch. Wir haben kein
// cfg80211 — unsere Eingabe ist ein Byte aus einem fremden Beacon,
// und wenn das „Zweitkanal oben" auf Kanal 13 sagt, faehrt die
// Rechnung auf Kanal 15. Den gibt es nicht, seine Sendeleistung steht
// in keiner Tabelle, und in Europa ist er nicht zugelassen.
//
// Geprueft wird deshalb der MITTENkanal gegen das Band, aus dem der
// primaere kommt. Faellt er heraus, gilt 20 MHz — ein schmaler Kanal
// ist immer erlaubt, ein erfundener nie.
```

## L2443-2444 · `if center > primary {`

```
// main.c:766-768: liegt der primaere UEBER der Mitte, ist er die obere
// Haelfte. Bei „Zweitkanal oben" ist er also die untere.
```

## L2452-2461 · `fn switch_channel(h: i32, hal: &Hal, e: &efuse::Efuse, t: &txpower::TxPower,`

```
/// main.c:880-913 `rtw_set_channel` — der Teil, der auf JEDEN Kanal passt.
///
/// `primary` ist der Kanal, auf dem die Zelle ihre Beacons sendet, `w`
/// das, was sie ueber ihre Breite sagt. Der Suchlauf ruft mit
/// `max_bw = 0` — auf einem Kanal, den man nur abhorcht, ist jede Breite
/// ueber 20 MHz eine Behauptung ueber den Nachbarkanal.
///
/// **Was an den Chip geht, ist der MITTENkanal**, nicht der primaere
/// (main.c:817 `hal->current_channel = center_channel`) — und deshalb
/// prueft auch die Gegenprobe an RF 0x18 gegen die Mitte.
```

## L2466-2467 · `let mut t2 = txpower::TxPower { cch_by_bw: t.cch_by_bw, ..*t };`

```
// `rtw_update_channel`: der 20-MHz-Eintrag ist IMMER der primaere
// Kanal, der Eintrag der laufenden Breite die Mitte (main.c:754-757).
```

## L2471-2480 · `if bw == 2 {`

```
// **Bei 80 MHz fehlt sonst der 40er-Eintrag, und das ist kein
// Schoenheitsfehler.** `rtw_phy_get_tx_power_limit` nimmt das MINIMUM
// ueber ALLE Breiten von 20 bis zur laufenden (phy.c:2149-2196) und
// schlaegt dafuer `cch_by_bw[1]` nach. Steht dort die Null, findet
// `channel_to_idx` keinen Kanal und die Grenze faellt ganz weg — wir
// saehen also ausgerechnet auf der breitesten Einstellung KEINE
// Sendeleistungsgrenze.
//
// main.c:777-791: die 40er-Mitte liegt in derselben HAELFTE der 80
// wie der primaere Kanal, also vier Schritte von der 80er-Mitte weg.
```

## L2487-2490 · `let band = if ch > 14 { txpower::PHY_BAND_5G } else { txpower::PHY_BAND_2G };`

```
// `rtw_coex_switchband_notify` steht hier in Linux, mit drei
// verschiedenen Gruenden je nach Band und Suchlauf. Er muendet in
// `rtw_coex_run_coex` — die LAUFENDE Koexistenz, die den Verkehrs- und
// BT-Zustand braucht. Benannt und nicht gebaut, seit Stufe 4c.
```

## L2502-2504 · `let a = phy::read_rf(h, phy::RF_PATH_A, 0x18, phy::RFREG_MASK);`

```
// `need_rfk` wird beim Suchen NICHT gesetzt — genau das ist der Sinn des
// `RTW_FLAG_SCANNING`-Zweigs: auf jedem Kanal zu kalibrieren dauert zu
// lange. Die Kalibrierung gehoert vor das Anmelden.
```

## L2506-2509 · `let a = phy::read_rf(h, phy::RF_PATH_A, 0x18, phy::RFREG_MASK);`

```
// Und die Gegenprobe: RF 0x18 traegt Band, Kanal und Bandbreite. Sie
// kostet zwei Lesezugriffe und sagt etwas ueber UNS statt ueber die
// Nachbarschaft — ob ein Netz auf einem Kanal funkt, entscheidet nicht
// der Treiber, ob der Chip den Kanal angenommen hat schon.
```

## L2515-2522 · `#[allow(clippy::too_many_arguments)]`

```
/// Stufe 5c — der Suchlauf.
///
/// **Aktiv auf 2,4 GHz, passiv auf 5 GHz.** Aktiv heisst: ein Probe Request
/// hinaus, dann zuhoeren. Passiv heisst: nur zuhoeren. Der Unterschied ist
/// keine Bequemlichkeit — auf welchen 5-GHz-Kanaelen gesendet werden DARF,
/// entscheidet die Zulassungszone, und die Regeln dafuer gehoeren der
/// oberen Haelfte (`wifid`/cfg80211), nicht dem Treiber. Empfangen ist
/// ueberall erlaubt, also hoert der Suchlauf dort, wo er nicht fragen darf.
```

## L2528-2529 · `const ACTIVE_2G: [u8; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];`

```
// 2,4 GHz: die dreizehn Kanaele, die es in Europa gibt. Kanal 14 ist
// nur in Japan und nur mit DSSS zugelassen — er steht bewusst nicht da.
```

## L2531 · `const PASSIVE_5G: [u8; 25] = [`

```
// 5 GHz: UNII-1 bis UNII-3, wie cfg80211 sie fuehrt. Nur zum Hoeren.
```

## L2537-2538 · `const DWELL_MS: u32 = 130;`

```
// Ein Beacon-Intervall sind 102,4 ms. Wer kuerzer horcht, verpasst ein
// Netz nicht wegen schwachem Signal, sondern wegen der Uhr.
```

## L2547-2551 · `let notify = fw_feature & FW_FEATURE_NOTIFY_SCAN != 0;`

```
// `rtw_core_scan_start`. Die Adresse steht seit 5b im Port (mac80211
// reicht hier eine ggf. gewuerfelte durch; wir nehmen unsere eigene).
// `rtw_leave_lps` und `RTW_FLAG_DIG_DISABLE` sind bei uns wirkungslos —
// es gibt weder Stromsparen noch eine laufende Verstaerkungsregelung.
// `rtw_coex_scan_notify` ist dieselbe benannte Luecke wie oben.
```

## L2574 · `let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF3) };`

```
// SAFETY: ein Faden, ein Rufer, der Puffer verlaesst die Funktion nicht.
```

## L2630-2631 · `let is_beacon = fc & 0xfc == 0x80;`

```
// Beacon (0x80) und Probe Response (0x50) tragen
// beide denselben Rumpf: 12 feste Bytes, dann Elemente.
```

## L2649 · `if notify {`

```
// `rtw_core_scan_complete`
```

## L2658 · `let _ = switch_channel(h, hal, e, t, 1, CellWidth::default(), 0);`

```
// Zurueck auf den Kanal, auf dem die Stufen davor gemessen haben.
```

## L2735-2747 · `let mut cfg = [0u8; 512];`

```
// **Das Ziel kommt aus `sys/config/wifi_ssid`, nicht aus der
// Lautstaerke.** docs/spec/WIFI_CLASS_ABI.md sagt warum: ohne
// SSID-Filter nimmt der Treiber den lautesten AP IRGENDEINES Netzes,
// auch den des Nachbarn — und fuer den hat `wifid` keinen PSK. Das
// endet in einem stillen MIC-Fehlschlag, und niemand sieht, woran.
// Nur 2,4 GHz: dort duerfen wir senden, auf 5 GHz haben wir nur
// gehorcht.
// **Eine Datei, `key: value` je Zeile — dieselbe, die `wifid` und
// `wifi_ax200` lesen.** Bis 0.20.0 stand hier `sys/config/wifi_ssid`;
// das steht so in einem veralteten Absatz der Spec, und es waere eine
// ZWEITE Stelle gewesen, die dasselbe konfiguriert. Zwei Stellen
// driften auseinander, und dann assoziiert der Treiber zu einem Netz,
// fuer das `wifid` keinen PSK hat.
```

## L2766-2776 · `let pref = read_band_pref();`

```
// **5 GHz ist jetzt waehlbar.** Hier stand `b.channel <= 14`, also
// wurde jede 5-GHz-Zelle gesucht, gemessen, gedruckt — und dann
// weggeworfen. Der Suchlauf faehrt die 25 Kanaele PASSIV (siehe
// `PASSIVE_5G`), es geht dort also kein Probe Request raus, und das
// ist genau die Sorgfalt, die ein DFS-Kanal verlangt: erst hoeren,
// dann senden.
//
// `Auto` nimmt 5 GHz, sobald es brauchbar steht, sonst das staerkste
// ueberhaupt. Eine reine "staerkstes Signal"-Wahl waere falsch: ein
// 2,4-GHz-AP im selben Raum ist fast immer lauter als sein
// 5-GHz-Zwilling und wuerde ihn dauerhaft verdecken.
```

## L2829-2840 · `match (gewaehlt, *target) {`

```
// **Ein Suchlauf ohne Fund loescht das Ziel NICHT.**
//
// Hier stand `*target = ...`, also auch `*target = None`. Der
// Wiederverbinden-Weg ruft diesen Suchlauf nach zwei Fehlschlaegen,
// und ein `None` faellt dort in ein `break`, das aus der Schleife
// HERAUS faellt: der Rufer schaltet die MAC ab und der Treiber ist
// zu Ende. **Ein Kanal, auf dem gerade niemand antwortet, ist kein
// Beweis, dass es die Zelle nicht mehr gibt.**
//
// 0.58.2 hat genau das als behoben GEMELDET und nur die Logzeile
// gebaut — die Wirkung stand in der Commit-Nachricht und nicht im
// Code. Hier ist sie.
```

## L2850-2861 · `if n_found > 0 {`

```
// **Die Kandidaten, nach Signal.**
//
// Gewaehlt wird nach Feldstaerke, und das ist eine ANNAHME: dass
// das lauteste Netz auch das schnellste ist. Am Geraet stimmte sie
// nicht — ein Repeater bei -50 dBm mit HT40 schlaegt den AP bei
// -55 dBm mit VHT80, und liefert die HAELFTE. Gesehen haben wir es
// erst, als der Durchsatz einbrach und die BSSID im Bericht eine
// andere war.
//
// Die Zahlen dafuer liegen seit dem Suchlauf alle vor; sie standen
// nur nirgends. Was die Zelle KANN, entscheidet dieselbe Funktion,
// die spaeter auch den Kanal legt — eine Rechnung, eine Antwort.
```

## L2904-2906 · `if let Some(t) = target {`

```
// **Die Kanaele unserer SSID merken** — sie sind der Suchraum fuer
// das Roaming. Nur die Kanaele, nicht die Pegel: die sind gleich
// veraltet, sobald jemand einen Schritt geht.
```

## L2915-2916 · `unsafe {`

```
// SAFETY: einfaedig, genau ein Schreiber, und der Suchlauf
// laeuft nicht parallel zum Pumpen.
```

## L2953-2956 · `ok &= gate("jeder angefahrene Kanal steht danach im RF", rf_ok == n_ch);`

```
// **Das Gate misst UNS, nicht die Nachbarschaft.** Ob auf einem Kanal
// jemand funkt, entscheidet nicht der Treiber — ob der Chip den Kanal
// angenommen hat, schon. Die Zahl der gefundenen Netze ist ein BEFUND
// und steht oben.
```

## L2958-2961 · `ok &= gate("ein fremder AP antwortet auf unseren Probe Request\n         \x20         (ueber alle aktiven Kanaele)", n_r`

```
// **Hier gehoert dieses Tor hin und nicht in 5b.** Dass ein Rahmen die
// Antenne verlaesst, beweist nur eine ANTWORT — und ob auf EINEM Kanal
// gerade jemand antwortet, ist ein Muenzwurf. Ueber dreizehn Kanaele
// ist es keiner mehr.
```

## L2974-2976 · `fn record_bss(found: &mut [Bss], n: &mut usize, f: &[u8], ch: u8,`

```
/// Einen Beacon oder eine Probe Response in die Liste aufnehmen. Gibt
/// `false` zurueck, wenn kein Platz mehr ist — eine volle Liste ist ein
/// BEFUND und darf nicht wie ein leerer Kanal aussehen.
```

## L2980 · `bssid.copy_from_slice(&f[16..22]); // addr3`

```
// addr3
```

## L2982-2989 · `let idx = match found[..*n].iter().position(|b| b.bssid == bssid) {`

```
// **Die Elemente werden bei JEDEM Rahmen neu gelesen.**
//
// Hier stand vorher ein `return` fuer eine schon bekannte Zelle: sie
// bekam nur ihren Zaehler hochgesetzt, und die Elemente blieben die
// des ERSTEN Rahmens, den wir je von ihr gesehen haben. Damit
// entschied der Zufall — Beacon oder Probe Response, frueh oder spaet
// —, welche Kanalbreite wir ihr fuer immer zuschreiben. Linux
// aktualisiert den BSS-Eintrag mit jedem Beacon.
```

## L3013-3014 · `if f.len() >= 36 {`

```
// 24 Kopf + 8 Zeitstempel + 2 Beacon-Intervall, dann das
// Faehigkeitsfeld, dann die Elemente.
```

## L3031 · `48 if len + 2 <= 64 => {`

```
// 48 = RSN (802.11 §9.4.2.24). Roh behalten, samt Kopf.
```

## L3036-3043 · `11 if len >= 3 => {`

```
// 61 = HT Operation (802.11 §9.4.2.56). Byte 0 ist der
// primaere Kanal, Byte 1 traegt die Lage des Zweitkanals.
// Wir behalten nur Byte 1 — den Kanal wissen wir, wir
// standen darauf, als der Beacon hereinkam.
// 45 = HT Capabilities (802.11 §9.4.2.55). Byte 0:1 ist das
// Faehigkeitsfeld; Bit 1 sagt, ob der AP 40 MHz KANN.
// 11 = BSS Load (802.11 §9.4.2.26). Byte 0:1 die Zahl der
// Stationen, Byte 2 die Kanalauslastung als 0..255.
```

## L3055-3060 · `191 if len >= 4 => {`

```
// 191 = VHT Capabilities (802.11 §9.4.2.157). Nur die ersten
// vier Byte, das Feld „VHT Capabilities Info" — daraus stutzt
// `build_vht_cap_ie` unser eigenes Angebot, wie mac80211 es
// tut (`ieee80211_add_vht_ie`, mlme.c:1481-1526). Der Grund
// steht dort woertlich: „Some APs apparently get confused if
// our capabilities are better than theirs."
```

## L3066-3074 · `221 if len >= 5`

```
// 192 = VHT Operation (802.11 §9.4.2.158). Byte 0 ist die
// Breite, Byte 1 und 2 sind die zwei Mittenkanal-Segmente.
// **Ohne dieses Element gibt es kein 80 MHz** — die Mitte
// einer 80er steht nirgendwo sonst, und aus dem primaeren
// Kanal zu raten waere eine Behauptung ueber drei
// Nachbarkanaele.
// 221 = herstellerspezifisch. Microsoft-OUI 00:50:f2, Typ 2
// ist WMM, Untertyp 0 das Info-, 1 das Parameter-Element —
// genau die Pruefung aus mac80211 `parse.c:407-421`.
```

## L3105-3112 · `#[allow(clippy::too_many_arguments)]`

```
/// rtw8822c.c:4179-4186 `rtw8822c_phy_calibration` — Stufe 5d.
///
/// **Sie laeuft hier, weil Linux sie hier laufen laesst.** `rtw_set_channel`
/// setzt nur `need_rfk = true`; ausgefuehrt wird sie in
/// `rtw_chip_prepare_tx`, das mac80211 aus `mgd_prepare_tx` ruft — also
/// nach dem Suchlauf und VOR dem Anmelden. Waehrend des Suchens auf jedem
/// Kanal zu kalibrieren dauert zu lange, und genau das sagt der Kommentar
/// in `main.c`.
```

## L3123-3124 · `const DM_FLAGS: u32 = 0;`

```
// `dm_flags` wird in Linux NUR aus debugfs beschrieben; beim Start ist
// es null, und damit ist keine Kalibrierung abgeschaltet.
```

## L3137-3143 · `{`

```
// **Die C2H-Antworten der Firmware holt bisher niemand ab.** Auf PCIe
// kommen sie durch DENSELBEN Empfangsring wie die Funkrahmen, an
// `pkt_stat.is_c2h` getrennt. Linux liest sie fortwaehrend; bei uns
// laeuft `rx_poll` nur in den Messfenstern von 5a bis 5c. Was seit dem
// letzten Fenster aufgelaufen ist, wird hier zuerst geleert — eine
// Firmware, deren Ausgang keiner leert, ist ein Verdaechtiger fuer
// jedes „die Firmware antwortet nicht".
```

## L3149 · `let b = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF4) };`

```
// SAFETY: ein Faden, ein Rufer, der Puffer verlaesst den Block nicht.
```

## L3187-3189 · `dpkinfo.is_dpk_pwr_on = true;`

```
// `rtw_load_rfk_table` hat die RFK-Tabelle in Stufe 3c geschrieben und
// setzt dabei dieses Flag (phy.c:1847). Ohne geladene Tabelle gaebe es
// keine DPK — die Reihenfolge ist der Grund, nicht ein Sonderfall.
```

## L3195 · `rfkcal::power_save(h, hal.rf_path_num, false);`

```
// `rtw8822c_rfk_power_save(rtwdev, false)`
```

## L3198 · `let t0 = host::now_us();`

```
// ── do_gapk ──────────────────────────────────────────────────
```

## L3245 · `let t0 = host::now_us();`

```
// ── do_iqk ───────────────────────────────────────────────────
```

## L3257 · `let t0 = host::now_us();`

```
// ── do_dpk ───────────────────────────────────────────────────
```

## L3291 · `rfkcal::power_save(h, hal.rf_path_num, true);`

```
// `rtw8822c_rfk_power_save(rtwdev, true)`
```

## L3297-3300 · `let dm = &mut d.dm;`

```
// ── Und die Frage, die zaehlt: hoert der Empfaenger danach noch? ──
// Eine Kalibrierung, die den Empfang kaputtmacht, ist schlimmer als
// keine. Dieselbe Messung wie in Stufe 4c, damit die Zahlen
// vergleichbar sind.
```

## L3336-3357 · `#[allow(clippy::too_many_arguments)]`

```
/// Stufe 5e — Authentifizierung und Anmeldung.
///
/// **Die Reihenfolge ist Linux' Reihenfolge**: Kanal des Ziels setzen →
/// `rtw_chip_prepare_tx` (die Kalibrierung aus 5d, denn `rtw_set_channel`
/// hat `need_rfk` gesetzt) → `PORT_SET_BSSID` → Auth → Assoc → und bei
/// Erfolg `net_type = RTW_NET_MGD_LINKED` mit der AID in den Port, dazu
/// `rtw_fw_media_status_report`.
///
/// **Was daneben steht und hier NICHT gebaut ist, namentlich:**
/// * `rtw_update_sta_info` + `rtw_fw_send_ra_info` — die Ratenanpassung
///   braucht die HT/VHT-Faehigkeiten des Gegenuebers aus der
///   Anmeldeantwort. Das ist ein Elementeparser der OBEREN Haelfte, und
///   ohne ihn waere jede Ratenmaske geraten.
/// * `rtw_fw_download_rsvd_page` + `rtw_send_rsvd_page_h2c` — PS-Poll,
///   Null- und QoS-Null-Rahmen in den reservierten Seitenbereich. Der Weg
///   dahin steht seit Stufe 2 (`download_firmware` schreibt dort), der
///   INHALT ist obere Haelfte.
/// * `rtw_fw_default_port`, `rtw_coex_media_status_notify`,
///   `rtw_bf_assoc`, `rtw_set_ampdu_factor`, `rtw_fw_beacon_filter_config`.
/// * Der Vierwegehandschlag und der Schluesselspeicher — ab da ist es
///   `wifid`, und eine Anmeldung ohne ihn endet nach wenigen Sekunden in
///   einem Deauth. Das Tor dieser Stufe steht DAVOR.
```

## L3369-3373 · `let max_bw = max_bw_for(e);`

```
// `rtw_set_channel` auf den Kanal des Ziels — und **hier zum ersten
// Mal mit der Breite, die die Zelle ansagt.** Bis 0.32.x stand die
// Breite auf 20 MHz genagelt, waehrend der Anmeldeantrag
// `SUP_WIDTH_20_40` versprach und jeder Sendedeskriptor 40 MHz
// eintrug: drei Stellen, drei Antworten.
```

## L3380-3381 · `d.cur_bw = bw;`

```
// **Hier faellt die Entscheidung, und nur hier.** Stufe 5f liest sie,
// statt sie ein zweites Mal zu rechnen.
```

## L3393-3396 · `host::print(" · HT-Operation 0x");`

```
// **Das rohe Byte dazu.** Ohne es sieht „der AP erlaubt kein HT40"
// genauso aus wie „wir lesen das Element falsch" — und beides endet
// in derselben Zeile `20 MHz`. Der Zweitkanal steht in Bit 1:0, die
// Erlaubnis fuer mehr als 20 MHz in Bit 2.
```

## L3410-3414 · `host::print(" · AP kann 40: ");`

```
// **Kann er 40, oder tut er nur 20?** Das HT-Operation-Element sagt,
// was der AP GERADE faehrt; Bit 1 seiner HT-FAEHIGKEITEN sagt, was er
// KANN. Nur beide nebeneinander trennen „seine Entscheidung" von
// „unsere Luecke" — und ein Element, das gar nicht da war, ist ein
// dritter Fall, der bisher wie „nur 20 MHz" aussah.
```

## L3422-3426 · `if bss.channel > 14 {`

```
// **Dasselbe fuer die 80 MHz, und aus demselben Grund.** Eine Zeile
// `40 MHz` auf einem 5-GHz-AP hat drei moegliche Ursachen — die Zelle
// faehrt wirklich nur 40, sie sendet kein VHT-Operation-Element, oder
// ihre Mitte hat unsere Pruefung nicht bestanden. Ohne die rohen
// Bytes sehen alle drei gleich aus.
```

## L3441-3443 · `host::print(" · Segment 1 K");`

```
// Ein zweites Segment heisst: der AP faehrt breiter als
// 80. Wir nehmen trotzdem Segment 0 — das ist genau die
// Zeile, fuer die 802.11 es dort hinschreibt.
```

## L3461-3462 · `let mut gapk = txgapk::GapkInfo::new();`

```
// `rtw_chip_prepare_tx`: `need_rfk` steht, also wird kalibriert — und
// zwar auf DIESEM Kanal, nicht auf dem des Suchlaufs.
```

## L3486-3487 · `let mut vifc = vif::add_interface_station(h, mac);`

```
// `rtw_ops_bss_info_changed`, Zweig `BSS_CHANGED_BSSID`. Ab jetzt
// nimmt die Hardware Rahmen dieser Zelle an.
```

## L3497 · `let rxbuf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF5) };`

```
// SAFETY: ein Faden, ein Rufer, der Puffer verlaesst die Funktion nicht.
```

## L3503-3505 · `let n = build_auth_req(&mut frame, &mac, &bss.bssid);`

```
// ── Authentifizierung (Open System) ──────────────────────────
// Auch WPA2 authentifiziert OFFEN; die Schluessel kommen erst nach
// der Anmeldung, im Vierwegehandschlag.
```

## L3510 · `if f.len() < 30 {`

```
// Auth-Antwort: Algorithmus, Folge 2, Status.
```

## L3525 · `let n = build_assoc_req(&mut frame, &mac, bss, e, hal.rf_path_num);`

```
// ── Anmeldung ────────────────────────────────────────────────
```

## L3541-3549 · `if n > 28 {`

```
// **Die rohen Elemente, so wie sie hinausgehen.**
//
// Drei Runden lang haben wir ueber den Inhalt dieses Rahmens
// GEREDET — ob 191 drinsteht, was es sagt, ob der AP es sieht. Er
// ist 60 Byte lang und steht jetzt da. Erst die rohe Eingabe
// abziehen, dann die Auswertung lesen.
//
// Ab Versatz 28: 24 Byte Kopf, dann Capability Info und Listen
// Interval, dann die Elemente.
```

## L3577 · `if f.len() < 30 {`

```
// Anmeldeantwort: Faehigkeiten, Status, AID.
```

## L3584-3587 · `aid = unsafe { LAST_ASSOC_AID };`

```
// Die AID steht im SELBEN Rahmen wie der Status, zwei Byte
// dahinter. `exchange` traegt nur eine Zahl zurueck, also legt
// der Leser sie daneben ab.
// SAFETY: einfaedig, ein Schreiber, ein Leser.
```

## L3601 · `vifc.aid = (aid & 0x3fff) as u32;`

```
// `rtw_vif_assoc_changed` + `PORT_SET_NET_TYPE | PORT_SET_AID`
```

## L3605 · `let msr = fw::media_status_report(h, h2c, vifc.mac_id, true);`

```
// `rtw_fw_media_status_report`
```

## L3620-3621 · `static mut LAST_ASSOC_AID: u16 = 0;`

```
/// Die AID der letzten Anmeldeantwort. Sie steht im selben Rahmen wie der
/// Status, und der Rueckgabeweg von `exchange` traegt nur EINE Zahl.
```

## L3624-3625 · `static mut LAST_ASSOC_RESP: [u8; 256] = [0; 256];`

```
/// Und der ganze Rahmen dazu: Stufe 5f liest daraus die Faehigkeiten des
/// AP (HT, VHT, Raten). In Linux baut mac80211 daraus `ieee80211_sta`.
```

## L3629-3632 · `#[allow(clippy::too_many_arguments)]`

```
/// Einen Verwaltungsrahmen senden und auf die Antwort warten.
///
/// Dreimal, mit Abstand: ein einzelner Rahmen kann kollidieren, und ein AP
/// darf ihn verwerfen. Gibt (Antwort gekommen, Status, Versuche) zurueck.
```

## L3648-3649 · `pci::tx_isr(h, trx, queue);`

```
// `rtw_pci_tx_isr` — den Lesezeiger nachziehen, sonst zaehlt der
// Ring sich ueber mehrere Rahmen voll.
```

## L3673 · `if want_fc == 0x10 && f.len() >= 32 {`

```
// Bei der Anmeldeantwort traegt derselbe Rahmen die AID.
```

## L3675 · `unsafe {`

```
// SAFETY: einfaedig, ein Schreiber.
```

## L3719 · `fn build_auth_req(out: &mut [u8; 256], mac: &[u8; 6], bssid: &[u8; 6])`

```
/// 802.11 §9.3.3.12 — Authentifizierungsrahmen, Open System, Folge 1.
```

## L3724 · `out[24..26].copy_from_slice(&0u16.to_le_bytes()); // Algorithmus 0 = offen`

```
// Algorithmus 0 = offen
```

## L3725 · `out[26..28].copy_from_slice(&1u16.to_le_bytes()); // Folge 1`

```
// Folge 1
```

## L3726 · `out[28..30].copy_from_slice(&0u16.to_le_bytes()); // Status 0`

```
// Status 0
```

## L3730-3735 · `fn build_assoc_req(out: &mut [u8; 256], mac: &[u8; 6], bss: &Bss,`

```
/// 802.11 §9.3.3.6 — Anmeldeantrag.
///
/// **Mit HT- und VHT-Element.** Ohne sie nimmt der AP uns als
/// LEGACY-Station an und laesst HT auch in seiner Antwort weg — in 0.19.0
/// kam genau das heraus: `ra_mask 0x0ff5`, keine MCS-Bits, Deckel bei
/// OFDM 54M. Ein Antrag, der weniger anbietet, bekommt weniger.
```

## L3739-3745 · `let preamble = if bss.channel > 14 { 0 } else { bss.capability & 0x0020 };`

```
// Faehigkeiten: ESS, dazu Privacy und Short Preamble so, wie der AP
// sie ansagt. Wer hier mehr behauptet, als der AP kann, wird abgelehnt.
//
// **Short Preamble nur auf 2,4 GHz.** mac80211 setzt SHORT_SLOT und
// SHORT_PREAMBLE ausschliesslich im 2,4-GHz-Band (mlme.c:1771-1774);
// auf 5 GHz gibt es die lange Praeambel gar nicht, das Bit ist dort
// ohne Bedeutung und Linux laesst es weg.
```

## L3749 · `out[26..28].copy_from_slice(&10u16.to_le_bytes()); // Listen Interval`

```
// Listen Interval
```

## L3752 · `let sl = bss.ssid_len as usize;`

```
// SSID
```

## L3759-3777 · `if bss.channel > 14 {`

```
// ── Die Raten, und sie haengen am BAND ───────────────────────
//
// **Hier standen 1, 2, 5.5 und 11 Mbit — auf JEDER Anmeldung, auch
// auf 5 GHz.** Das sind CCK/DSSS-Raten; es gibt sie im 5-GHz-Band
// nicht, und sie standen obendrein mit gesetztem hohen Bit da, also
// als BASIS-Raten. Wir haben einer reinen VHT-Zelle gesagt, wir
// seien eine 11b-Station.
//
// Linux baut diese Elemente je Band aus `sband->bitrates`
// (`ieee80211_assoc_add_rates` -> `ieee80211_put_srates_elem`,
// mlme.c), schneidet sie gegen die Raten des AP und setzt in einem
// Anmeldeantrag KEINE Basis-Bits (`basic_rates` ist dort 0). Der
// Kommentar daneben nennt den Grund: „some APs don't like getting a
// superset of their rates in the association request".
//
// Das 2,4-GHz-Bein bleibt Byte fuer Byte, wie es war — es ist am
// Geraet gemessen und traegt 96 Mbit. Die Basis-Bits dort sind
// dieselbe Abweichung von Linux, hier benannt und NICHT angefasst:
// ein funktionierender Pfad wird nicht auf Verdacht umgebaut.
```

## L3779-3780 · `out[n] = 1;`

```
// 6, 9, 12, 18, 24, 36, 48, 54 — alle acht passen in EIN
// Element, also faellt das erweiterte ganz weg.
```

## L3787 · `out[n] = 1;`

```
// Supported Rates: 1, 2, 5.5, 11, 6, 9, 12, 18 Mbit
```

## L3794 · `out[n] = 50;`

```
// Extended Supported Rates: 24, 36, 48, 54 Mbit
```

## L3801-3815 · `if bss.rsn_len > 0 {`

```
// **Die Reihenfolge ist die der Spezifikation, nicht die der
// Kennungen.** Hier stand „die Elemente stehen in AUFSTEIGENDER
// Kennung", und das ist nicht die Regel: 802.11 Tabelle 9-34 ordnet
// den Rumpf eines Anmeldeantrags nach TABELLENPOSITION, und danach
// steht RSN (Ordnung 8) VOR HT Capabilities (Ordnung 13). Die
// Ext-Raten (Ordnung 5) stehen deshalb auch vor HT, obwohl ihre
// Kennung 50 groesser ist als 45 — die alte Begruendung haette
// genau das verboten.
//
// mac80211 macht es so: die RSN-Elemente kommen aus
// `ieee80211_add_before_ht_elems` (mlme.c), also vor HT. Wir hatten
// sie dahinter.
//
// Fuer den Vierwegehandschlag aendert sich nichts: sein MIC rechnet
// ueber den INHALT des RSN-Elements, nicht ueber seine Stelle.
```

## L3817 · `if bss.rsn_len > 0 {`

```
// RSN — aus dem, was der AP ansagt, EINE Wahl gebaut.
```

## L3822 · `n += sta::build_ht_cap_ie(&mut out[n..], e.hw_cap_bw, e.hw_cap_nss);`

```
// HT — immer. Es entscheidet, ob wir als 11n-Station angenommen werden.
```

## L3825-3826 · `if bss.channel > 14 {`

```
// VHT nur auf 5 GHz: auf 2,4 GHz ist es nicht zugelassen, und ein AP
// darf einen Antrag mit VHT im falschen Band ablehnen.
```

## L3835-3843 · `unsafe {`

```
// **Was WIRKLICH hinausging, nicht die Bedingung dafuer.**
//
// Der Bericht sagte bisher „wir bieten VHT 2SS MCS0-9", und das
// war aus `hw_cap_ptcl` und dem Kanal GERECHNET — dieselbe
// Bedingung wie hier, also keine unabhaengige Aussage. Kehrte
// `build_vht_cap_ie` aus einem anderen Grund um, sagte die
// Zeile trotzdem ja. Hier steht das Byte vom Draht.
// SAFETY: einfaedig, ein Schreiber, und der Bericht liest es
// erst, wenn die Anmeldung durch ist.
```

## L3854 · `unsafe { SENT_VHT = None };`

```
// SAFETY: wie oben.
```

## L3858-3875 · `if bss.wmm && n + 9 <= out.len() {`

```
// ── WMM-Information, als LETZTES Element ─────────────────────
//
// **Ohne dieses Element bekommen wir kein VHT.** hostapd streicht
// einer Station VHT, wenn ihr Antrag kein gueltiges WMM-Element
// traegt (`copy_sta_vht_capab`, ieee802_11_vht.c:200-207:
// `!(sta->flags & WLAN_STA_WMM)`); `check_wmm` (ieee802_11.c:5361)
// setzt die Fahne nur aus genau diesem Element. Am Geraet: der AP
// schickte uns 100 % HT40 und 0 % VHT, obwohl beide Seiten VHT80
// koennen — ein iPhone an derselben Box holt 500-600 Mbit.
//
// Der Kommentar an `tx_8023` hielt das Element fuer eine Altlast der
// Wi-Fi Alliance, weil der AP uns trotzdem HT und Block Ack gab. Das
// stimmt fuer HT auf DIESER Box; fuer VHT gilt die Regel oben.
//
// Gebaut wie `ieee80211_add_wmm_info_ie` (util.c:4290-4303), an der
// Stelle aus `ieee80211_send_assoc` (mlme.c:2299-2309): nach allen
// Nicht-Hersteller-Elementen, nur wenn der AP selbst WMM ansagt
// (`assoc_data->wmm` = `bss->wmm_used`), QoS-Info 0 — kein U-APSD.
```

## L3878 · `221, 7,             // herstellerspezifisch, Laenge`

```
// herstellerspezifisch, Laenge
```

## L3879 · `0x00, 0x50, 0xf2,   // Microsoft-OUI`

```
// Microsoft-OUI
```

## L3880 · `2,                  // WME`

```
// WME
```

## L3881 · `0,                  // WME-Info`

```
// WME-Info
```

## L3882 · `1,                  // Version`

```
// Version
```

## L3883 · `0,                  // QoS-Info: U-APSD nicht in Gebrauch`

```
// QoS-Info: U-APSD nicht in Gebrauch
```

## L3887 · `unsafe { SENT_WMM = bss.wmm };`

```
// SAFETY: wie oben — einfaedig, gelesen erst nach der Anmeldung.
```

## L3892 · `static mut SENT_WMM: bool = false;`

```
/// Ob der letzte Anmeldeantrag das WMM-Element trug.
```

## L3895-3896 · `static mut SENT_VHT: Option<u32> = None;`

```
/// Das Feld „VHT Capabilities Info", das der letzte Anmeldeantrag
/// wirklich getragen hat. `None` = es ging kein VHT-Element hinaus.
```

## L3899-3907 · `const RSN_IE_WPA2_CCMP_PSK: [u8; 22] = [`

```
/// 802.11 §9.4.2.24 — unser RSN-Element.
///
/// **Es ist BYTE-GLEICH mit dem in `wifid`** (`wasm/src/lib.rs:124`), und
/// das ist kein Zufall, sondern ein Vertrag, den die ABI nicht ausdrueckt:
/// der Vierwegehandschlag rechnet seinen MIC ueber GENAU das RSN-Element,
/// das die Station im Anmeldeantrag geschickt hat. Weicht unseres ab,
/// verwirft der AP msg2 — und sagt nicht warum.
///
/// CCMP als Gruppen- und Paarschluessel, PSK als Authentifizierung.
```

## L3913-3919 · `fn build_rsn_ie(out: &mut [u8], ap: &[u8]) -> usize {`

```
/// Unser RSN-Element schreiben — und MELDEN, wenn der AP etwas anderes
/// ansagt, als wir anbieten koennen.
///
/// **Ein Antrag waehlt, ein Beacon zaehlt auf**: das Element des AP nennt
/// alle Verfahren, die er kann; unseres nennt genau eines. Kann er CCMP
/// nicht als Gruppenchiffre, scheitert die Verbindung spaeter im
/// Handschlag — und dann soll hier schon stehen, warum.
```

## L3929-3931 · `fn mgmt_header(out: &mut [u8; 256], subtype_fc: u8, mac: &[u8; 6],`

```
/// Der gemeinsame 24-Byte-Kopf eines Verwaltungsrahmens an einen AP.
/// Die Folgenummer bleibt null — `en_hwseq` steht im Sendedeskriptor,
/// also vergibt sie der Chip.
```

## L3937 · `out[2..4].copy_from_slice(&0u16.to_le_bytes()); // duration`

```
// duration
```

## L3938 · `out[4..10].copy_from_slice(bssid); // addr1 = Empfaenger`

```
// addr1 = Empfaenger
```

## L3939 · `out[10..16].copy_from_slice(mac); // addr2 = wir`

```
// addr2 = wir
```

## L3940 · `out[16..22].copy_from_slice(bssid); // addr3 = BSSID`

```
// addr3 = BSSID
```

## L3941 · `out[22..24].copy_from_slice(&0u16.to_le_bytes()); // seq`

```
// seq
```

## L3944-3961 · `fn stage5f_rates(h: i32, trx: &mut pci::Trx, h2c: &mut fw::H2cState,`

```
/// Stufe 5f — die Ratenanpassung.
///
/// Der Treiber schickt der Firmware KEINE Rate, sondern eine MASKE:
/// welche der 64 Raten dieses Gegenueber kann. Die Firmware waehlt daraus
/// laufend und meldet ihre Wahl als `C2H_RA_RPT` zurueck — und genau das
/// ist das Tor dieser Stufe. Eine Maske, die niemand beantwortet, ist eine
/// Behauptung.
///
/// **Die Maske kommt aus der Anmeldeantwort**, die Stufe 5e aufgehoben
/// hat: HT- und VHT-Element, unterstuetzte Raten. In Linux baut mac80211
/// daraus `ieee80211_sta`; hier steht der Parser in `sta.rs` und gehoert
/// spaeter `wifid`.
///
/// **Nicht gebaut und namentlich:** `rtw_fw_download_rsvd_page` +
/// `rtw_send_rsvd_page_h2c`. Die reservierten Seiten tragen PS-Poll, Null-
/// und QoS-Null-Rahmen, die die FIRMWARE im Stromsparbetrieb selbst
/// sendet. Stromsparen gibt es hier nicht, also wuerden die Seiten
/// geschrieben und nie gelesen. Sie gehoeren zu LPS, nicht hierher.
```

## L3967 · `let (resp, len) = unsafe {`

```
// SAFETY: einfaedig, und 5e hat vorher geschrieben.
```

## L3978-3992 · `let zellen_bw = d.cur_bw;`

```
// **Die Breite einer Station ist das MINIMUM aus ihrem Koennen und der
// Zelle** (mac80211 `ieee80211_sta_cur_vht_bw`). `parse_assoc_resp`
// kennt nur das Koennen — der Kommentar dort sagt es woertlich: „Ohne
// die Zelle bleibt das, was das Gegenueber kann." Hier haben wir die
// Zelle, also gehoert die Klemme hierher.
//
// Ohne sie trug jeder Sendedeskriptor 40 MHz, waehrend die PHY auf 20
// stand. Ein Deskriptor, der eine andere Breite behauptet als das
// Funkteil fuehrt, ist kein Schoenheitsfehler: die Firmware waehlt
// ihre Raten danach.
// **Die Zahl kommt aus 5e und wird nicht nachgerechnet.** Sie haengt
// inzwischen an drei Elementen und an der Konfiguration; dieselbe
// Rechnung ein zweites Mal zu fahren hiesse, zwei Antworten zu
// pflegen, und die eine hier entscheidet, was in JEDEN Sendedeskriptor
// geschrieben wird.
```

## L4035-4040 · `d.dm.rrsr_val_init = if bss.channel <= 14 { RRSR_INIT_2G } else { RRSR_INIT_5G };`

```
// main.c:1266/1286 — `rtw_update_sta_info` setzt im selben Zug die
// Grundmenge der Antwortraten, je Band. Unser `update_sta_info`
// haelt kein `dm`, also steht die Zeile hier und im Watchdog
// (`phy::ra_track`). **Ohne sie schrieb `rrsr_update` alle zwei
// Sekunden `0 & mask = 0` nach REG_RRSR** — die Raten, aus denen der
// MAC seine ACKs und Block-Acks waehlt.
```

## L4072 · `let dm = &mut d.dm;`

```
// ── Und jetzt zuhoeren, was die Firmware daraus macht ────────
```

## L4078 · `let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF6) };`

```
// SAFETY: ein Faden, ein Rufer, der Puffer verlaesst die Funktion nicht.
```

## L4104 · `if c.id as u32 == C2H_RA_RPT && c.payload.len() >= 7 {`

```
// `rtw_fw_ra_report_handle`: rate_sgi, mac_id, …, bw
```

## L4138-4141 · `fn rate_name(r: u8) -> &'static str {`

```
/// main.h:250-262 `DESC_RATE*` als Namen — fuer den Bericht.
/// **Der genaue Name, nicht die Klasse.** Bis 0.31.0 stand hier
/// „HT MCS8-15" fuer acht verschiedene Raten — fuer die Frage „mit
/// welcher Rate laeuft die Leitung wirklich" ist das keine Antwort.
```

## L4170-4172 · `#[derive(Clone, Copy)]`

```
/// Die Zaehler der Verbindung. Sie gehoeren dem LINK, nicht der Stufe —
/// 6b setzt fort, wo 6a aufgehoert hat, und ein Zaehler, der dabei auf
/// null springt, ist eine Luege ueber die Leitung.
```

## L4185-4186 · `last_seq_ctrl: [u32; 9],`

```
/// **Das letzte Sequenz-Kontrollfeld je TID** (Platz 8 = ohne QoS),
/// `u32::MAX` = noch keins. `rx.c:1480` `last_seq_ctrl[seqno_idx]`.
```

## L4188 · `dup_rx: u32,`

```
/// Verworfene 802.11-Wiederholungen (`dot11FrameDuplicateCount`).
```

## L4190-4198 · `retry_rx: u32,`

```
/// Rahmen mit gesetztem Retry-Bit (802.11 §9.2.4.1.8).
///
/// **Die eine Zahl, die sagt, ob der AP Grund hat, langsamer zu
/// werden.** `dup_rx` misst das nicht: die Duplikatspruefung merkt
/// sich GENAU EINEN Sequenzwert je TID, und eine Wiederholung, die
/// nach einem Aggregat von 37 Rahmen kommt, trifft ihn nie. Am
/// Geraet standen deshalb 17 Duplikate neben 32649 „zu spaet".
/// Das Retry-Bit ist die Aussage des SENDERS und braucht kein
/// Gedaechtnis.
```

## L4200 · `ro_on: [bool; RO_TIDS],`

```
/// Umsortierpuffer je TID: laeuft eine Block-Ack-Sitzung?
```

## L4202 · `ro_head: [u16; RO_TIDS],`

```
/// Naechste erwartete Sequenznummer (12 Bit).
```

## L4204 · `ro_slot: [[u8; RO_WIN]; RO_TIDS],`

```
/// Platz im Fenster -> Poolindex + 1, 0 = leer.
```

## L4206 · `ro_held: [u8; RO_TIDS],`

```
/// Wieviele Rahmen dieser TID gerade liegen.
```

## L4208 · `ro_since: [u32; RO_TIDS],`

```
/// Wann der Kopf zuletzt blockiert wurde (ms), fuer die Frist.
```

## L4210 · `ro_sorted: u32,`

```
/// Zaehler fuer den Bericht.
```

## L4215-4216 · `amsdu_rx: u32,`

```
/// **A-MSDU**: MPDUs mit gesetztem A-MSDU-Bit, die Teilrahmen daraus,
/// und wieviele ganz verworfen wurden (`goto purge` in cfg80211).
```

## L4220-4225 · `rekey_rx: u32,`

```
/// **Der Gruppen-Neuschluessel, gezaehlt statt vermutet.** Jedes
/// EAPOL NACH dem Handschlag ist einer (msg1 der
/// Gruppenschluessel-Sequenz, oder ein ganz neues Vierwege), und
/// jede Antwort darauf zaehlt daneben. „3 empfangen, 0 beantwortet"
/// heisst an uns; „3/3" und trotzdem Rauswurf heisst woanders — und
/// der Grundcode sagt dann wo.
```

## L4228 · `gtk_set: u32,`

```
/// Jede GTK, die `wifid` uns ins CAM schreiben laesst.
```

## L4230-4233 · `gone: Option<(bool, u16)>,`

```
/// Was die Empfangsschleife gesehen hat und die Schleife DANACH
/// behandelt: `(war es ein Deauth, Grundcode)`. Im Rueckruf steht
/// nur das Sehen — `netdev_set_link` und `EV_LINK_DOWN` gehoeren
/// nicht in einen Rueckruf, der mitten im Ringleeren laeuft.
```

## L4235 · `kicked: u32,`

```
/// Wie oft wir hinausgeworfen wurden, und womit zuletzt begruendet.
```

## L4238-4241 · `probes: [TxProbe; TX_PROBE_SLOTS],`

```
/// **Die Sendequittung der Firmware** (`rtw_tx_report_*`, tx.c).
/// Bis 0.26.0 wussten wir von KEINEM gesendeten Rahmen, ob er
/// ankam — genau der Beobachter, der bei zwei Fehlern hintereinander
/// gefehlt hat.
```

## L4244 · `tx_acked: u32,`

```
/// quittiert · nicht quittiert · gar keine Antwort der Firmware
```

## L4248 · `fw_crash: u32,`

```
/// Die Firmware hat sich selbst fuer tot erklaert.
```

## L4250 · `reconnects: u32,`

```
/// Wie oft wir die Verbindung neu aufgebaut haben.
```

## L4252-4256 · `c2h_ids: [(u8, u32); 4],`

```
/// **Der Zensus der unbehandelten C2H-Kennungen.** Vier Plaetze,
/// jeder `(Kennung, Anzahl)` — mehr verschiedene schickt diese
/// Firmware nicht, und die haeufigste ist die interessante. Bis
/// 0.27.2 wurden sie verworfen, und die ausbleibende Sendequittung
/// war dadurch eine Null ohne Hinweis.
```

## L4258-4259 · `mgmt_sub: [u32; 16],`

```
/// Verwaltungsrahmen unserer Zelle, nach Subtyp gezaehlt (16
/// Plaetze, einer je Subtyp — die Liste ist abgeschlossen).
```

## L4261-4262 · `addba_req: u32,`

```
/// Und fuer Action-Rahmen die Kategorie/Aktion des letzten sowie
/// die Zahl der **ADDBA Requests** — die Frage dieser Runde.
```

## L4264 · `addba_tx: u32,`

```
/// Und UNSERE Fragen, in die andere Richtung.
```

## L4266 · `addba_drop: u32,`

```
/// Bitten des AP, die nicht einmal in den Zwischenpuffer passten.
```

## L4268 · `poll_on: bool,`

```
/// `IEEE80211_STA_CONNECTION_POLL` — wir stupsen gerade an.
```

## L4270 · `probe_send_count: u32,`

```
/// `ifmgd->probe_send_count`
```

## L4272 · `probe_timeout_ms: u64,`

```
/// `ifmgd->probe_timeout`
```

## L4274 · `poll_started: u32,`

```
/// Wie oft die Wache angeschlagen hat und wie oft sie recht hatte.
```

## L4277-4278 · `csa_done: u32,`

```
/// Wie oft wir dem AP auf einen neuen Kanal gefolgt sind — und wie
/// oft dort niemand war.
```

## L4281-4283 · `tx_batch_n: u32,`

```
/// Wieviele Rahmen je Anstoss im Ring lagen. **Es ist die
/// Obergrenze dessen, was die Hardware aggregieren KANN** — liegt
/// dort im Mittel einer, hilft die beste Block-Ack-Sitzung nichts.
```

## L4287-4291 · `tx_ring_sum: u32,`

```
/// Und wieviel die HARDWARE beim selben Augenblick noch vor sich
/// hatte. **Das ist die Zahl, die ueber Aggregation entscheidet** —
/// `tx_batch_*` sagt nur, wieviel der Treiber in EINEM Durchlauf
/// eingelegt hat, und das ist etwas anderes, sobald das Medium
/// belegt ist.
```

## L4294-4295 · `rx_ppdu_n: u32,`

```
/// Die Aggregatgroesse in EMPFANGSrichtung, aus `ppdu_cnt` des
/// Deskriptors: Sendevorgaenge und die Rahmen darin.
```

## L4299 · `last_tsf: u32,`

```
/// Der Abstand zweier Sendevorgaenge des AP, aus der 802.11-Uhr.
```

## L4304-4305 · `rx_gap_buckets: [u32; 5],`

```
/// Die VERTEILUNG, nicht der Mittelwert. Eimer nach `GAP_BUCKETS`,
/// der fuenfte ist „ueber 10 ms" und fuehrt seine Summe mit.
```

## L4308 · `rx_gap_idle: u32,`

```
/// Und die Abstaende, bei denen gar kein Verkehr war.
```

## L4310-4311 · `last_rx_at: u64,`

```
/// Die Umkehrzeit unseres eigenen Stapels: Daten an den Kernel ->
/// Rahmen vom Kernel zurueck.
```

## L4319-4320 · `addba_resp: u32,`

```
/// Wie oft wir zugestimmt haben — und wie oft die Antwort nicht in
/// den Sendering passte.
```

## L4323-4326 · `addba_win: u16,`

```
/// Das Fenster, das wir zuletzt zugestanden haben, und das, um das
/// gebeten wurde. **Ohne die Zahl im Bericht ist nicht zu sehen, ob
/// eine geaenderte `ampdu:`-Zeile ueberhaupt gelesen wurde** — der
/// Treiber liest sie einmal beim Start der Schleife.
```

## L4329-4331 · `rx_polls: u32,`

```
/// Die Form der Empfangsschleife: Bliecke mit und ohne Beute, die
/// Summe der Rahmen, und wie oft ein Blick den Stapel voll
/// ausschoepfte.
```

## L4336-4350 · `rx_us: u64,`

```
/// **Das Ratenhistogramm ueber die GANZE Verbindung.**
///
/// Linux fuehrt `cur_pkt_count.num_qry_pkt[rate]` je Watchdog-Takt
/// und schiebt es nach `last_pkt_count`; debugfs liest es LAUFEND
/// mit. Wir haben kein debugfs — ein Bericht, den jemand NACH einer
/// Uebertragung liest, braucht eine Zahl, die sie ueberlebt.
///
/// Und er braucht sie dringend: `curr_rx_rate` ist die Rate des
/// LETZTEN Rahmens, und eine halbe Sekunde nach einem Download ist
/// das ein Beacon — die gehen auf der niedrigsten Grundrate. Der
/// Bericht zeigte deshalb „OFDM 6M", waehrend die Daten mit etwas
/// ganz anderem kamen.
/// Wanduhrzeit in `rx_poll`, wenn es etwas brachte, und wie lange
/// die Schleife insgesamt laeuft. Ihr Verhaeltnis ist die
/// Auslastung des Empfangspfades.
```

## L4353-4356 · `ht_ok: u64,`

```
/// **Was die Luft kaputt macht**, aufsummiert: `false_alarm_statistics`
/// liest je Modulation einen CRC-Zaehler und SETZT IHN ZURUECK. Eine
/// Momentaufnahme sagt darueber nichts; die Summe ueber die
/// Verbindung sagt, ob der AP staendig wiederholen muss.
```

## L4362-4369 · `bw_hist: [u32; 4],`

```
/// In welcher BREITE die Rahmen wirklich hereinkamen — 20/40/80 und
/// ein vierter Platz fuer alles andere.
///
/// **Es ist die einzige Zahl, die 80 MHz BEWEIST.** Alles andere im
/// Bericht (`bw 80 MHz`) ist unsere eigene Einstellung: was wir in den
/// Deskriptor schreiben und in die PHY gesetzt haben. Der
/// Empfangsstatus sagt, was der AP wirklich sendet — und ob beides
/// zusammenpasst, dafuer gibt es sonst keinen Zeugen.
```

## L4371-4374 · `ra_rpt_n: u32,`

```
/// Wie oft die Firmware ihre Ratenwahl gemeldet hat (`C2H_RA_RPT`).
/// **Null hiesse: `dm.tx_rate` steht auf 0 = CCK 1M**, und damit
/// waehlt `config_swing_table` die CCK-Kurve der
/// Sendeleistungs-Nachfuehrung.
```

## L4379-4382 · `fn default() -> Self {`

```
/// **Von Hand, weil `[u32; 84]` kein `Default` hat** (die Ableitung
/// reicht nur bis 32). `zeroed` waere hier richtig und trotzdem
/// falsch: ein `unsafe` fuer eine Struktur aus lauter Zahlen und
/// `bool` spart nichts und verpflichtet den naechsten Leser.
```

## L4421-4426 · `fn arm_probe(&mut self, now: u64) -> Option<u8> {`

```
/// tx.c:166-211 `rtw_tx_report_enable` + `rtw_tx_report_enqueue` in
/// einem: Nummer vergeben und Platz belegen.
///
/// Gibt `None`, wenn alle acht Plaetze belegt sind — dann antwortet
/// die Firmware ohnehin nicht, und eine neunte Frage macht es nicht
/// besser.
```

## L4434 · `fn settle_probe(&mut self, sn: u8, acked: bool) {`

```
/// tx.c:229-256 `rtw_tx_report_handle` — die Antwort zuordnen.
```

## L4446 · `fn note_mgmt(&mut self, subtype: u8, cat: u8, action: u8) {`

```
/// Einen Verwaltungsrahmen zaehlen.
```

## L4457-4459 · `fn note_c2h(&mut self, id: u8) {`

```
/// Eine unbehandelte C2H-Kennung zaehlen. Vier Plaetze, danach nur
/// noch die, die schon dastehen — der Zensus soll die haeufigste
/// finden, nicht jede einzelne.
```

## L4470-4471 · `fn purge_probes(&mut self, now: u64) {`

```
/// tx.c:179-194 `rtw_tx_report_purge_timer` — „failed to get tx
/// report from firmware". Eine Frist, keine Rundenzahl.
```

## L4481-4482 · `fn next_probe_due(&self) -> Option<u64> {`

```
/// Wann `purge_probes` die naechste offene Quittung aufgibt — der
/// Zeitpunkt, zu dem die Pumpe wieder hinsehen muss.
```

## L4491-4498 · `#[derive(Clone, Copy, Default)]`

```
/// tx.c `struct rtw_tx_report` — die Rahmen, deren Quittung aussteht.
///
/// **Linux haengt dafuer die `sk_buff`s in eine Warteschlange**, weil es
/// sie danach an mac80211 zurueckgibt. Wir brauchen den Rahmen nicht
/// mehr, nur die Frage „ist er angekommen?" — also eine Folgenummer und
/// wann gefragt wurde. Acht Plaetze: mehr als acht offene Quittungen
/// hiesse, dass die Firmware gar nicht antwortet, und dann sagt das der
/// Zaehler `tx_no_report`.
```

## L4508-4514 · `const GAP_BUCKETS: [u32; 4] = [500, 2_000, 5_000, 10_000];`

```
/// Die Grenzen der Abstands-Eimer, in Mikrosekunden.
///
/// **Ein Mittelwert versteckt genau die Verteilung, um die es geht.**
/// `abstand 1899 us im mittel` kann heissen: jedes Aggregat kommt nach
/// 1,9 ms — oder die meisten nach 0,3 ms und alle dreissig eine Pause
/// von dreizehn Millisekunden. Das sind zwei verschiedene Fehler, und
/// nur der zweite ist ein Fehler.
```

## L4516-4517 · `const TURN_BUCKETS: [u32; 4] = [200, 1_000, 5_000, 20_000];`

```
/// Dieselbe Frage fuer die Umkehrzeit unseres Stapels, eine
/// Groessenordnung feiner: dort ist schon eine Millisekunde viel.
```

## L4520-4522 · `const GAP_IDLE_US: u32 = 200_000;`

```
/// **Ab hier ist es keine Pause mehr, sondern kein Verkehr.** Zwischen
/// zwei Downloads liegen Sekunden; die gehoeren nicht in dieselbe
/// Summe wie eine Stockung mitten im Strom.
```

## L4536-4557 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
// ═══════════════════════════════════════════════════════════════
// Roaming — der AP wechseln, BEVOR die Verbindung abreisst
//
// Florian: *„ich moechte ja nicht die verbindung verlieren muessen..
// oder auf einem fast totem ap sitzen bleiben"*, und: *„wenn daneben ein
// perfekter waere .. das waere genau der unterbruch den ich nicht
// moechte"*.
//
// **Der Ausloeser kommt aus mac80211** (`ieee80211_handle_beacon_sig`,
// mlme.c:6780-6870): ein EWMA ueber den Bakenpegel, erst ab
// `IEEE80211_SIGNAL_AVE_MIN_COUNT` Baken, mit Schwelle UND Hysterese —
// ein Ereignis feuert erst wieder, wenn der Pegel um die Hysterese
// darueber hinausgeht. Ohne das loest ein einzelner schlechter Beacon
// einen Suchlauf aus.
//
// **Die Auswahl ist eine SETZUNG.** Sie steht bei Linux in
// wpa_supplicant (`wpa_scan_result_compar`), und die Quelle liegt nicht
// im Cache — nur `wpa.c` und `wpa_common.h`. Die Regel hier hat die
// Form, die in diesem Treiber schon gilt (`PREFER_5G_DBM`: „5 GHz ab
// -70 dBm bevorzugt"), und die Zahlen stehen als benannte Konstanten,
// damit man sie an Messungen aendern kann statt im Code zu suchen.
// ═══════════════════════════════════════════════════════════════
```

## L4559 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// `roam:` aus `sys/config/wifi`.
```

## L4562 · `An,`

```
/// Umhoeren und wechseln.
```

## L4564 · `Aus,`

```
/// Gar nicht umhoeren — der Zustand vor 0.57.0.
```

## L4566-4568 · `NurBericht,`

```
/// Umhoeren und BERICHTEN, aber nicht wechseln. Das Werkzeug fuer
/// den ersten Abend: damit die Schwellen an Zahlen aus der eigenen
/// Wohnung festgelegt werden und nicht an geschaetzten.
```

## L4572-4585 · `pub fn roam_from(v: &[u8]) -> RoamMode {`

```
/// Der reine Teil, damit `framecheck.py` ihn ohne Geraet fahren kann.
///
/// **Die Vorgabe ist `NurBericht`, und das ist eine Abweichung von der
/// Regel „ein unverstandener Wert ist die Vorgabe".**
///
/// Sie hat einen Grund: ein Wechsel ist ein EINGRIFF in eine laufende
/// Verbindung, und am Geraet endete jede Neuanmeldung nach dem Wechsel
/// in `Grund 15: Vierwegehandschlag: Zeitueberschreitung` — dreizehn
/// Mal hintereinander. Solange das nicht bewiesen durchlaeuft, darf
/// Roaming keine stehende Verbindung anfassen. Es HOERT sich um und
/// SAGT, was es taete; das kostet nichts und ist genau die Messung, aus
/// der die Schwellen kommen.
///
/// `on` schaltet es scharf, `off` ganz ab.
```

## L4608-4609 · `const SIGNAL_AVE_MIN_COUNT: u32 = 4;`

```
/// mlme.c:96 `IEEE80211_SIGNAL_AVE_MIN_COUNT` — unter vier Baken sagt
/// der geglaettete Pegel nichts.
```

## L4612-4613 · `const ROAM_THOLD_DBM: i8 = -70;`

```
/// Ab hier horchen wir uns um. **Setzung**, dieselbe Schwelle, die
/// `PREFER_5G_DBM` schon fuehrt.
```

## L4615-4616 · `const ROAM_HYST_DB: i8 = 4;`

```
/// `cqm_rssi_hyst` — so weit muss der Pegel wieder steigen, bevor die
/// Schwelle erneut ausloest. **Setzung.**
```

## L4618-4619 · `const ROAM_SCAN_GAP_MS: u64 = 10_000;`

```
/// Mindestabstand zweier Umhoerversuche. **Setzung**: jeder kostet
/// Latenz, und unter zehn Sekunden aendert sich in einer Wohnung nichts.
```

## L4621-4622 · `const ROAM_GAP_MS: u64 = 10_000;`

```
/// Mindestabstand zweier Wechsel — die Hysterese gegen das Pendeln
/// zwischen zwei gleich guten Zellen. **Setzung.**
```

## L4624 · `const ROAM_BETTER_DB: i8 = 8;`

```
/// So viel staerker muss ein Kandidat sein. **Setzung.**
```

## L4626-4630 · `const ROAM_NARROWER_COST_DB: i8 = 10;`

```
/// Was eine HALBIERUNG der Bandbreite kosten darf, in dB. **Setzung**,
/// und die Groessenordnung ist nicht gegriffen: die Hälfte der Breite ist
/// die Haelfte der Bruttorate, und drei dB mehr Pegel bringen auf einer
/// belegten Strecke bei weitem nicht das Doppelte. Zehn dB je Stufe
/// heisst: von 80 auf 20 MHz muss ein Kandidat zwanzig dB besser sein.
```

## L4632-4635 · `const ROAM_WIDER_TOLERANCE_DB: i8 = 6;`

```
/// ... ODER er ist BREITER (VHT80 gegen HT40) und hoechstens so viel
/// schwaecher. **Setzung** — und der Fall, der Florian getroffen hat:
/// ein Repeater bei -50 dBm mit HT40 schlaegt den AP bei -55 dBm mit
/// VHT80 und liefert die Haelfte.
```

## L4637-4639 · `const ROAM_DWELL_MS: u32 = 25;`

```
/// Wie lange wir je Kanal horchen. **Ein gerichteter Probe Request wird
/// in Millisekunden beantwortet**; passives Lauschen braeuchte ein
/// volles Bakenintervall (102 ms) je Kanal.
```

## L4641 · `const ROAM_CHANNELS_MAX: usize = 6;`

```
/// Wieviele Kanaele wir uns aus dem Suchlauf merken.
```

## L4643 · `const ROAM_BSS_MAX: usize = 8;`

```
/// Und wieviele Zellen ein Umhoerversuch findet.
```

## L4646-4657 · `static mut ROAM_CHANNELS: [u8; ROAM_CHANNELS_MAX] = [0; ROAM_CHANNELS_MAX];`

```
/// Die Kanaele, auf denen der Startsuchlauf Zellen UNSERER SSID gesehen
/// hat.
///
/// **Das ist der Grund, warum ein Umhoerversuch billig ist.** Florian:
/// *„ich moechte nicht einen ganzen suchlauf.. das macht kaum sinn.
/// sondern eig. kennen wir ja die SSID bereits und auf welchem kanal es
/// funkt."* Genau so macht es `bgscan simple` in wpa_supplicant auch.
///
/// **Die gespeicherten PEGEL benutzen wir NICHT** — die sind vom
/// Startsuchlauf und damit von einem anderen Ort in der Wohnung. Ein
/// alter Pegelwert ist schlechter als keiner. Gespeichert wird nur,
/// WO wir suchen.
```

## L4661 · `#[derive(Clone, Copy)]`

```
/// Der Zustand des Roamings an einer stehenden Verbindung.
```

## L4664-4666 · `ave: dm::Ewma,`

```
/// `ewma_beacon_signal`, `DECLARE_EWMA(beacon_signal, 4, 4)`
/// (mac80211 ieee80211_i.h:518). Gerechnet auf `Pegel + 128`, weil
/// unsere `Ewma` vorzeichenlos rechnet.
```

## L4668 · `count: u32,`

```
/// `count_beacon_signal`
```

## L4670 · `last_event: i8,`

```
/// `last_cqm_event_signal` — 0 heisst „noch nie gefeuert".
```

## L4674 · `scans: u32,`

```
/// Wie oft wir gewechselt haben und wie oft wir uns umgehoert haben.
```

## L4677 · `to: Option<Bss>,`

```
/// Der Kandidat, zu dem der Rufer wechseln soll.
```

## L4687 · `fn note_beacon(&mut self, dbm: i8) {`

```
/// Eine Bake der eigenen Zelle.
```

## L4693 · `fn dbm(&self) -> i8 {`

```
/// Der geglaettete Pegel in dBm.
```

## L4699 · `const EWMA_BEACON_PRECISION: u32 = 4;`

```
/// `DECLARE_EWMA(beacon_signal, 4, 4)` — Genauigkeit 4, Gewicht 1/16.
```

## L4703-4712 · `fn build_nullfunc(out: &mut [u8; 32], mac: &[u8; 6], bssid: &[u8; 6],`

```
/// Ein Null-Data-Rahmen — `ieee80211_send_nullfunc` (mlme.c:2364).
///
/// **Das ist der Rahmen, der einen Umhoerversuch billig macht.** Mit
/// gesetztem Power-Management-Bit sagt er dem AP „ich schlafe kurz";
/// der PUFFERT dann unsere Pakete, statt sie auf einen Kanal zu senden,
/// auf dem wir nicht mehr sind. Beim Zurueckkommen dasselbe mit
/// geloeschtem Bit, und er schiebt das Gepufferte nach
/// (`ieee80211_offchannel_ps_enable`/`_disable`, offchannel.c:25-81).
///
/// **Ohne ihn kostet jeder Umhoerversuch Pakete. Mit ihm nur Latenz.**
```

## L4716 · `out[0] = DOT11_FC_TYPE_DATA | (4 << 4);`

```
// Typ Daten (0b10), Subtyp 4 = Null Data.
```

## L4718 · `out[1] = 0x01 | if powersave { 0x10 } else { 0x00 };`

```
// ToDS, dazu das Power-Management-Bit (802.11 §9.2.4.1.7).
```

## L4720 · `out[4..10].copy_from_slice(bssid); // addr1 = Empfaenger`

```
// addr1 = Empfaenger
```

## L4721 · `out[10..16].copy_from_slice(mac); // addr2 = wir`

```
// addr2 = wir
```

## L4722 · `out[16..22].copy_from_slice(bssid); // addr3 = BSSID`

```
// addr3 = BSSID
```

## L4726-4734 · `const MAX_PROBE_TRIES: u32 = 5;`

```
// ═══════════════════════════════════════════════════════════════
// Die Verbindungswache — mlme.c:4278-4481, 8516-8560
//
// **Ausbleibende Baken sind kein Verbindungsverlust.** Linux stupst den
// AP erst an und gibt erst auf, wenn auch das schweigt. Wir haben
// `rtw_sw_beacon_loss_check` seit je portiert (`d.beacon_loss`) und den
// Wert NIE gelesen: eine Verbindung, deren AP verschwindet, blieb bei
// uns stehen, bis jemand neu startete.
// ═══════════════════════════════════════════════════════════════
```

## L4736 · `const MAX_PROBE_TRIES: u32 = 5;`

```
/// mlme.c:58 `max_probe_tries`.
```

## L4738 · `const PROBE_WAIT_MS: u64 = 500;`

```
/// mlme.c:86 `probe_wait_ms`.
```

## L4740-4744 · `const PROBE_UNICAST_LIMIT: u32 = if MAX_PROBE_TRIES > 4 {`

```
/// mlme.c:4391 `unicast_limit = max(1, max_probe_tries - 3)`.
///
/// **Die letzten drei Versuche gehen als Rundruf hinaus**, und der
/// Grund steht im Quellkommentar: manche APs beantworten NUR einen
/// Rundruf. Wer nur gerichtet fragt, erklaert die fuer tot.
```

## L4751-4753 · `const BA_TX_TID: u8 = 0;`

```
/// Der TID, auf dem unsere Daten laufen. Best Effort, und es ist der
/// einzige: ohne EDCA vom AP gibt es keinen Grund, eine zweite Schlange
/// aufzumachen, und jede weitere kostet eine eigene Block-Ack-Sitzung.
```

## L4756-4757 · `const BA_RESP_MS: u64 = 200;`

```
/// Wie lange wir auf die ADDBA-Antwort warten, bevor wir nachfragen.
/// mac80211: `ADDBA_RESP_INTERVAL` = HZ/5.
```

## L4759-4762 · `const BA_MAX_TRIES: u32 = 3;`

```
/// Wieviele Male. mac80211 gibt nach `HT_AGG_MAX_RETRIES` (15) auf; wir
/// nach drei — danach sagt die Konsole, dass der AP nicht will, und eine
/// Verbindung ohne Aggregation ist kein Fehlerzustand, sondern eine
/// langsame Verbindung.
```

## L4767 · `Aus,`

```
/// Noch nicht gefragt — oder nicht zu fragen (`txagg: off`).
```

## L4769 · `Gefragt,`

```
/// Gefragt, Antwort steht aus.
```

## L4771 · `Laeuft,`

```
/// Der AP hat zugesagt. Ab hier traegt jeder Rahmen dieses TID AGG_EN.
```

## L4773-4776 · `Aufgegeben,`

```
/// Abgelehnt oder nach drei Versuchen unbeantwortet. Kein Wiederholen
/// — ein AP, der dreimal geschwiegen hat, schweigt auch beim vierten
/// Mal, und eine Schleife auf dem Verwaltungspfad kostet Sendezeit,
/// die genau das zunichtemacht, was sie holen soll.
```

## L4780-4786 · `#[derive(Clone, Copy)]`

```
/// Unsere Block-Ack-Sitzung in SENDErichtung — die Haelfte, die seit
/// 0.29.0 fehlte.
///
/// In Linux liegt sie in `tid_ampdu_tx` und wird von
/// `ieee80211_tx_ba_session_handle_start` gefahren; der Treiber sieht nur
/// `IEEE80211_AMPDU_TX_OPERATIONAL`. Wir haben kein mac80211, also steht
/// der Automat hier.
```

## L4791-4792 · `token: u8,`

```
/// Die Nummer, unter der wir gefragt haben. Eine Antwort mit einer
/// anderen gehoert zu einer frueheren Frage (agg-tx.c:1002).
```

## L4796 · `win: u16,`

```
/// Was der AP zugesagt hat, in Rahmen.
```

## L4798-4799 · `factor: u8,`

```
/// `MAX_AGG_NUM` und `AMPDU_DEN` fuer den Deskriptor, aus den
/// HT-Faehigkeiten des AP.
```

## L4802 · `status: u16,`

```
/// Der Status seiner Absage, fuer den Bericht.
```

## L4816 · `struct Link {`

```
/// Der Zustand einer stehenden Verbindung — Stufe 6a.
```

## L4821-4824 · `ssid: [u8; 32],`

```
/// Der Name der Zelle. **Er wird fuer den gerichteten Probe Request
/// gebraucht** (`ieee80211_ap_probereq_get`, mlme.c:4518-4521: das
/// SSID-Element traegt den Namen des EINEN AP, nicht die Null-Laenge)
/// — und spaeter, um beim Wechseln Zellen derselben SSID zu finden.
```

## L4829-4831 · `seq: u16,`

```
/// Laufende Folgenummer fuer Datenrahmen. Der Chip vergibt sie bei
/// `en_hwseq` selbst, aber `pkt_info.seq` steht trotzdem im
/// Deskriptor — Linux fuellt es aus dem Rahmenkopf.
```

## L4834-4836 · `tx_pn: u64,`

```
/// 802.11 §12.5.3.2 — die 48-Bit-Paketnummer des Paarschluessels.
/// Sie faengt bei eins an und zaehlt je Rahmen hoch; eine wiederholte
/// Nummer verwirft der AP als Wiedereinspielung.
```

## L4839 · `ba_tx: BaTx,`

```
/// Unsere Block-Ack-Sitzung in Senderichtung.
```

## L4841 · `roam: Roam,`

```
/// Roaming: geglaetteter Pegel, Sperren, Kandidat.
```

## L4843-4846 · `ht_param_now: u8,`

```
/// Die Breite, in der die Verbindung LAEUFT — als die drei Bytes,
/// aus denen `chan_params` sie rechnet. **Der Rueckweg von einem
/// Umhoerversuch braucht sie**: wer auf 20 MHz zurueckkommt, hat die
/// Verbindung auf 20 MHz, und niemand sagt es ihm.
```

## L4850-4851 · `csa: Option<Csa>,`

```
/// Eine laufende Wechselansage: wohin, wie breit, und ab wann.
/// `None` heisst: kein Wechsel angesagt.
```

## L4853-4854 · `csa_at_ms: u64,`

```
/// Der Augenblick, zu dem umgezogen wird (`link->u.mgd.csa.time`,
/// mlme.c:2992).
```

## L4856 · `csa_zurueck: Option<(u8, CellWidth)>,`

```
/// Wohin zurueck, falls auf dem neuen Kanal niemand ist.
```

## L4861-4866 · `fn ccmp_hdr(out: &mut [u8], pn: u64, key_id: u8) {`

```
/// 802.11 §12.5.3.2 — der acht Byte lange CCMP-Kopf.
///
/// Die Paketnummer steht in zwei Stuecken, und das ist kein Versehen der
/// Spezifikation: Byte 2 ist reserviert und Byte 3 traegt das ExtIV-Bit
/// und die Schluesselnummer, damit ein alter WEP-Empfaenger den Rahmen
/// als erweitert erkennt.
```

## L4868 · `out[0] = (pn & 0xff) as u8; // PN0`

```
// PN0
```

## L4869 · `out[1] = ((pn >> 8) & 0xff) as u8; // PN1`

```
// PN1
```

## L4870 · `out[2] = 0; // reserviert`

```
// reserviert
```

## L4871 · `out[3] = 0x20 | (key_id << 6); // ExtIV | KeyID`

```
// ExtIV | KeyID
```

## L4872 · `out[4] = ((pn >> 16) & 0xff) as u8; // PN2`

```
// PN2
```

## L4873 · `out[5] = ((pn >> 24) & 0xff) as u8; // PN3`

```
// PN3
```

## L4874 · `out[6] = ((pn >> 32) & 0xff) as u8; // PN4`

```
// PN4
```

## L4875 · `out[7] = ((pn >> 40) & 0xff) as u8; // PN5`

```
// PN5
```

## L4878-4908 · `fn tx_8023(h: i32, trx: &mut pci::Trx, mgmt_buf: i32, link: &mut Link,`

```
/// docs/spec/WIFI_CLASS_ABI.md §2b — ein Ethernet-Rahmen als
/// 802.11-Datenrahmen an den AP.
///
/// 802.3: `[DA 6][SA 6][ethertype 2][Nutzlast]`
/// 802.11 ToDS: `[fc 2][dur 2][addr1=BSSID][addr2=SA][addr3=DA][seq 2]`
/// plus LLC/SNAP (RFC 1042) und den Ethertyp.
///
/// **`qos` entscheidet ueber die Rahmenart, und daran haengt alles
/// andere.** Ein Block Ack braucht einen TID, einen TID traegt nur ein
/// QoS-Rahmen, und ohne Block Ack geht jeder Rahmen einzeln hinaus.
///
/// Der Kommentar, der hier stand, sagte das Gegenteil: unser
/// Anmeldeantrag trage kein WMM-Element, also habe der AP uns als
/// Nicht-QoS-Station angenommen. **Der Geraetelauf widerlegt ihn
/// dreifach.** Eine HT-Station IST eine QoS-Station (802.11 §10.2.3, und
/// wir senden HT- und VHT-Elemente); ein Block Ack gibt es nur zwischen
/// QoS-Stationen (§10.24.2) — und der AP hat uns zwei davon ANGEBOTEN;
/// und jeder Rahmen, den er uns schickt, ist ein QoS-Rahmen, sonst haette
/// der Umsortierpuffer keinen TID, nach dem er ordnet. Das WMM-Element
/// ist eine Zutat der Wi-Fi Alliance aus der Zeit vor 802.11n, nicht die
/// Bedingung.
///
/// `probe` ist `IEEE80211_TX_CTL_REQ_TX_STATUS` — Linux setzt es aus
/// mac80211 fuer die Rahmen, deren Verlust die Verbindung kostet
/// (Steuerport, also EAPOL). Gibt die Folgenummer zurueck, unter der
/// die Firmware antworten wird.
///
/// **EAPOL faehrt bewusst OHNE QoS**, also genau wie bisher: der
/// Vierwegehandschlag laeuft, bevor es eine Block-Ack-Sitzung gibt, und
/// ein Rahmen, dessen Verlust die Verbindung kostet, ist der falsche Ort
/// fuer eine Aenderung, die er nicht braucht.
```

## L4925-4926 · `frame[0] |= DOT11_STYPE_QOS << 4;`

```
// Der Subtyp steht in Bit 7:4, `DOT11_STYPE_QOS` ist die
// Nibble-Nummer — daher der Schiebeschritt.
```

## L4929 · `frame[1] = 0x01; // ToDS`

```
// ToDS
```

## L4934 · `frame[4..10].copy_from_slice(&link.bssid); // addr1 = Empfaenger`

```
// addr1 = Empfaenger
```

## L4935 · `frame[10..16].copy_from_slice(&link.mac); // addr2 = Quelle`

```
// addr2 = Quelle
```

## L4936 · `frame[16..22].copy_from_slice(&eth[0..6]); // addr3 = Ziel`

```
// addr3 = Ziel
```

## L4939-4943 · `frame[24] = tid & 0x0f;`

```
// 802.11 §9.2.4.5 — QoS Control. Bit 3:0 der TID, Bit 6:5 die
// Quittungsregel (00 = normal, und das ist IM Block Ack der
// implizite Block-Ack-Antrag), Bit 7 A-MSDU: nein. Byte 1 ist die
// TXOP-Dauer bzw. Schlangenlaenge und gehoert dem, der sie
// ANFORDERT — wir fordern nichts.
```

## L4948-4954 · `let ofs = if encrypt {`

```
// **Der CCMP-Kopf wird vom TREIBER geschrieben, nicht von der
// Hardware.** `rtw_ops_set_key` setzt `IEEE80211_KEY_FLAG_GENERATE_IV`,
// und das heisst in mac80211: der Stapel macht acht Byte Platz und
// schreibt die Paketnummer hinein (`ccmp_pn2hdr`), die Hardware
// verschluesselt nur. Ohne ihn stehen unsere Rahmen fuer den AP nicht
// zur Entschluesselung bereit — und das sieht aus wie eine Leitung,
// auf der nichts zurueckkommt.
```

## L4963 · `frame[ofs + 6..ofs + 8].copy_from_slice(&eth[12..14]); // Ethertyp`

```
// Ethertyp
```

## L4967-4968 · `tx::data_pkt_info_update(&mut info, link.seq, Some(&link.si),`

```
// `rtw_tx_pkt_info_update` fuer einen Datenrahmen: erst die Rate,
// dann die gemeinsamen Felder.
```

## L4977-4978 · `if encrypt {`

```
// `rtw_tx_pkt_info_update_sec`: mit installiertem Schluessel traegt der
// Deskriptor die acht Byte des CCMP-Kopfes als zusaetzliche Laenge.
```

## L4980 · `info.sec_type = 0x3; // AES`

```
// AES
```

## L4985-4987 · `if let Some(sn) = probe {`

```
// tx.c:432-433 `if (info->flags & IEEE80211_TX_CTL_REQ_TX_STATUS)`.
// Die Nummer vergibt der Rufer (`rtw_tx_report_enable`), weil er sie
// gleich darauf in seine offene Liste eintraegt.
```

## L4993-5002 · `if let Some(tid) = qos {`

```
// tx.c:361-365 — `ampdu_en` haengt in Linux an
// `IEEE80211_TX_CTL_AMPDU`, einer Fahne, die mac80211 setzt, SOBALD
// ein Block-Ack-Block offen ist. Bei uns ist die Fahne `BaState::
// Laeuft` auf genau diesem TID.
//
// **Aggregiert wird von der HARDWARE**, nicht vom Treiber: der Chip
// fasst aufeinanderfolgende Rahmen derselben MACID und desselben TID
// zusammen, wenn AGG_EN steht. Der Treiber sagt nur, wieviel am
// Stueck erlaubt ist — und das sind die Zahlen, die der AP in seinen
// HT-Faehigkeiten angesagt hat, nicht unsere.
```

## L5011-5017 · `let queue = pci::Q_BE;`

```
// **Angestossen wird NICHT hier.** tx.c:660-676: Linux schiebt
// `frame_cnt` Rahmen in den Ring und ruft `rtw_hci_tx_kick_off`
// EINMAL danach — und das ist keine Sparsamkeit beim MMIO-Schreiben,
// sondern die Voraussetzung der Aggregation. Aggregiert wird von der
// Hardware, und sie kann nur zusammenfassen, was beim Griff nach der
// Sendegelegenheit schon im Ring liegt. Wer je Rahmen an die Tuer
// klopft, laesst sie mit einem losfahren.
```

## L5022-5040 · `fn data_hdrlen(f: &[u8]) -> usize {`

```
/// Wo der LLC/SNAP-Kopf eines Datenrahmens steht, und wieviel hinten
/// nicht dazugehoert.
///
/// **Ein verschluesselter Rahmen traegt acht Byte CCMP-Kopf zwischen dem
/// 802.11-Kopf und den Nutzdaten**, und die Hardware entfernt ihn NICHT:
/// `rtw_rx_fill_rx_status` setzt `RX_FLAG_DECRYPTED`, aber nicht
/// `RX_FLAG_IV_STRIPPED` — in Linux raeumt mac80211 ihn weg. Hinten haengen
/// die Pruefsumme (immer) und bei CCMP der acht Byte lange MIC, beides
/// weil `WLAN_RCR_CFG` APP_FCS und APP_MIC gesetzt hat.
///
/// Findet sich LLC/SNAP nicht an der gerechneten Stelle, wird an den zwei
/// anderen moeglichen gesucht und das GEMELDET. Ein stiller Fehlgriff
/// hier verwirft jeden Rahmen und sieht aus wie eine tote Leitung.
/// cfg80211 `ieee80211_hdrlen` (util.c:430-447) fuer einen DATENrahmen
/// einer Station: 24, bei QoS plus 2 fuer das QoS-Steuerfeld — und mit
/// gesetztem Order-Bit plus 4 fuer HT-Control. **Die vier fehlten**: wir
/// sagen `+HTC-VHT` an, also darf der AP das Feld senden, und dann stand
/// der LLC/SNAP-Kopf vier Byte hinter jeder Stelle, an der wir suchten.
/// Adressfeld 4 (FromDS und ToDS) gibt es fuer eine Station nicht.
```

## L5050-5057 · `let hdrlen = data_hdrlen(f);`

```
// **Der Subtyp steht in Bit 7:4.** Hier stand `f[0] & DOT11_STYPE_QOS`
// ohne den Schiebeschritt — und `DOT11_STYPE_QOS` (0x08) ist
// zufaellig derselbe Wert wie `DOT11_FC_TYPE_DATA`, also war die
// Antwort fuer JEDEN Datenrahmen „ja, QoS". Gemerkt hat es niemand,
// weil dieser AP uns ausschliesslich QoS-Rahmen schickt (`llc_miss`
// steht ueber die ganze Verbindung auf null) — die Suche daneben
// haette einen Nicht-QoS-Rahmen mit CCMP gar nicht gefunden, denn
// 24+8 = 32 steht in keinem ihrer drei Versuche.
```

## L5096-5111 · `fn disconnect_reason(f: &[u8], bssid: &[u8; 6]) -> Option<(bool, u16)> {`

```
/// **Der Rauswurf, und warum er bisher unsichtbar war.**
///
/// `rx_to_8023` filtert in seiner ERSTEN Zeile auf Datenrahmen. Ein
/// Deauth ist ein VERWALTUNGSrahmen und faellt dort lautlos durch: aus
/// Treibersicht stirbt die Verbindung nicht, sie wird nur still — und
/// genau deshalb wirkt ein Rauswurf zufaellig. Der Kernel glaubt
/// derweil weiter an `carrier UP` und schiebt Pakete in eine tote
/// Leitung.
///
/// 802.11 §9.4.1.7: Deauthentication (Subtyp 12) und Disassociation
/// (Subtyp 10) tragen einen Grundcode, little-endian, direkt hinter dem
/// 24 Byte langen Kopf. Gibt `(war es ein Deauth, Grundcode)` zurueck.
///
/// **Nur von `addr2 == BSSID`.** Die Luft ist voll; der Deauth einer
/// fremden Zelle geht uns nichts an, und ein Treiber, der auf ihn
/// hoert, legt seine eigene Verbindung wegen des Nachbarn nieder.
```

## L5113-5114 · `if f.len() < 26 {`

```
// 24 Byte Kopf + 2 Byte Grund. Kuerzer ist kein gueltiger Rahmen,
// und raten waere hier schlimmer als schweigen.
```

## L5129-5131 · `fn reason_name(code: u16) -> &'static str {`

```
/// 802.11 §9.4.1.7 Tabelle 9-49. **Die 15 und die 16 sind die Frage
/// dieser Runde**: sie waeren die Bestaetigung, dass es am Handschlag
/// bzw. am Gruppen-Neuschluessel haengt und nicht an der Luft.
```

## L5158-5162 · `fn rx_to_8023(f: &[u8], out: &mut [u8], miss: &mut u32)`

```
/// docs/spec/WIFI_CLASS_ABI.md §2b, Demux-Regel: ein empfangener
/// 802.11-Datenrahmen wird zu 802.3 und geht dann entweder als `EAPOL_RX`
/// an `wifid` oder in den IP-Stapel.
///
/// Gibt die Laenge des 802.3-Rahmens in `out` zurueck und ob es EAPOL war.
```

## L5169 · `if f[0] & DOT11_STYPE_NODATA != 0 {`

```
// Null und QoS-Null tragen keinen Rumpf.
```

## L5183 · `out[0..6].copy_from_slice(&f[4..10]);`

```
// FromDS: addr1 = wir, addr2 = BSSID, addr3 = Quelle.
```

## L5191-5203 · `#[allow(clippy::too_many_arguments)]`

```
/// PLATZ
///
/// **Hier hoert der Stufentest auf und der Treiber faengt an.** Bis 5f
/// arbeitete `main` eine Kette ab und schaltete den Chip aus; hier laeuft
/// eine Schleife: Empfangsring leeren, Sendequittungen einsammeln,
/// Kommandos von `wifid` ausfuehren, Ereignisse hinaufmelden.
///
/// **Den Handschlag rechnet `wifid`, nicht wir** — er ist
/// herstellerunabhaengig und steht einmal da
/// (`tools/wasm/wifid/core/src/eapol.rs`). Der Treiber transportiert die
/// Rahmen und schreibt die fertigen Schluessel in den Speicher. Genau so
/// steht es in `docs/spec/WIFI_CLASS_ABI.md` §1: der Treiber sieht nie
/// den PSK.
```

## L5205-5216 · `fn highest_tx_rate(caps: &sta::PeerCaps, hal: &Hal) -> u8 {`

```
/// Den Link aufbauen: beim Kernel anmelden und `wifid` scharf machen.
///
/// **Das darf genau EINMAL geschehen.** Ein zweites `EV_READY` laesst
/// `wifid` einen frischen Supplicant bauen, der auf ein msg1 wartet, das
/// der AP nie wieder schickt — genau das ist in 0.23.0 passiert, weil
/// Stufe 6b die Funktion von 6a ein zweites Mal rief.
/// tx.c:367-374, die Reihenfolge in `rtw_tx_data_pkt_info_update`.
///
/// **VHT steht VOR HT, und das war die Luecke.** Beide Rufstellen fragten
/// nur `ht_supported` — auf einer VHT-Verbindung kam damit
/// `DESC_RATEMCS15` heraus, also eine HT-Rate fuer eine Strecke, die VHT
/// faehrt.
```

## L5224 · `DESC_RATE11M as u8`

```
// tx.c:371 `supp_rates[0] <= 0xf` — nur die vier CCK-Bits.
```

## L5256-5257 · `let reg = host::netdev_register(&mac);`

```
// Der Datenkanal existiert seit Kernel 0.205.0; ohne Anmeldung sieht
// ihn der IP-Stapel nicht.
```

## L5263-5264 · `let mut ready = [0u8; 13];`

```
// `EV_READY` = [0x83][ap_mac 6][our_mac 6] — damit baut `wifid` seinen
// Supplicant fuer GENAU diese Zelle.
```

## L5277-5306 · `#[allow(clippy::too_many_arguments)]`

```
/// main.c:224-310 `rtw_watch_dog_work` — **alle zwei Sekunden, das
/// ganze Leben einer Verbindung lang.**
///
/// Bis 0.26.0 gab es sie nicht. Gebaut war der Aufbau, und danach blieb
/// der Chip sich selbst ueberlassen: kein Quarz-Nachziehen, keine
/// Sendeleistung ueber die Temperatur, keine Vorverzerrungs-Nachfuehrung,
/// keine RSSI an die Ratenwahl der Firmware. Drei davon sind SENDEseite,
/// und ein Empfaenger rastet sich an jeder Praeambel neu ein — ein
/// Sender nicht. Das ist die Form, in der eine Leitung einseitig wird,
/// ohne dass irgendwo ein Fehler steht.
///
/// **Vier Posten aus Linux stehen hier NICHT, und jeder hat seinen
/// Grund:**
///
/// * `rtw_leave_lps` / `rtw_enter_lps` / `rtw_recalc_lps` — wir fahren
///   kein Power-Save (offener Posten im Plan).
/// * `rtw_hci_dynamic_rx_agg` — `.dynamic_rx_agg = NULL` fuer PCI
///   (pci.c:1605), also auf unserem Bus ein Nichts.
/// * `rtw_dynamic_csi_rate` — kehrt um, solange die Gegenstelle keine
///   Beamforming-Rolle hat; wir bauen `bf.c` nicht.
/// * `rtw_coex_run_coex` (ueber `wl_status_change_notify`) — der
///   Entscheidungsbaum der Koexistenz ist L6 des Plans, 111 Funktionen,
///   eigene Stufe.
///
/// **Eine Abweichung, die hier stehen MUSS:** `rtw_coex_monitor_bt_enable`
/// wird in Linux nur aus `rtw_coex_run_coex` gerufen. Sie erzeugt
/// `bt_disabled`, und daran haengt `rtw8822c_cfo_need_adjust`. Ohne sie
/// bliebe die Zahl auf ihrem Anfangswert stehen, der Riegel zu und die
/// Quarznachfuehrung fuer immer aus — ein Tor, das nie aufgeht. Also
/// wird sie hier gerufen, bis L6 steht.
```

## L5308-5315 · `static mut WD_MAX: [u32; 8] = [0; 8];`

```
/// **Wie lange jeder Teil des Watchdogs laengstens brauchte**, in us.
///
/// Linux laesst den Watchdog in einer Workqueue NEBEN dem Empfang laufen;
/// bei uns steht er in der Pumpschleife, und solange er rechnet, holt
/// niemand den Ring leer. Am Geraet (0.67.0) setzte die Schleife bis zu
/// 119 ms aus, der Kartenring lief ueber, und TCP verlor 400-600 Segmente
/// auf einen Schlag. `do_lck` allein darf nach Linux bis zu 100 ms pollen.
/// Diese Zahlen sagen, WELCHER Teil es war, statt es zu raten.
```

## L5323 · `unsafe {`

```
// SAFETY: einfaedig, nur der Pumpfaden schreibt, der Bericht liest.
```

## L5331-5334 · `static mut IRQ_VEC: i32 = -1;`

```
/// Die laengste Runde der Pumpschleife ohne ihren Schlaf, in us, und ob
/// in ihr der Watchdog lief.
/// Der MSI-Vektor des Chips, `-1` = Abfragebetrieb. Gesetzt beim Start
/// (`rtw_hci_start`), gelesen im Leerlauf der Pumpschleife.
```

## L5337-5339 · `const CSA_WAIT_MS: u64 = 10;`

```
/// Waehrend eines Kanalwechsels (CSA) parkt die Pumpe hoechstens so lange:
/// Beacon-Frist und Umschaltzeitpunkt sind selten und kurz, dort bleibt es
/// beim feinen Raster.
```

## L5350 · `let busy_pre = d.busy_traffic;`

```
// main.c:241-248 — die Schwelle ist 100 Rahmen je Takt.
```

## L5355 · `}`

```
// `rtw_coex_wl_status_change_notify(rtwdev, 0)` -> run_coex (L6)
```

## L5358 · `let tx_mbps = (d.stats.tx_unicast >> RTW_TP_SHIFT) as u32;`

```
// main.c:255-268 — Bytes je zwei Sekunden in Mbit/s, geglaettet.
```

## L5375 · `coex::wl_status_check(h, &mut d.cx);`

```
// main.c:275-279
```

## L5382-5384 · `let sta_rate = if linked { Some(link.si.ra_report_desc_rate) } else { None };`

```
// `si->ra_report.desc_rate` — was die FIRMWARE zuletzt gewaehlt hat,
// nicht was wir angeboten haben. Sie meldet es als C2H `RA_RPT`;
// solange keiner kam, steht dort die Anfangsrate.
```

## L5389-5393 · `phy::statistics(h, &mut d.dm, h2c, if linked { Some(&mut link.si) } else { None });`

```
// **Eine Ausleihe, zwei Rufe.** `si` und `rssi_si` sind in Linux
// derselbe Iterator ueber dieselbe Station; hier muessen sie
// nacheinander gehen, weil der Ausleiher nur eine mutable Referenz
// zulaesst. Die Reihenfolge ist Linux': erst `statistics` (und darin
// der RSSI), dann der Rest.
```

## L5421-5422 · `if fw_feature & FW_FEATURE_BCN_FILTER == 0 && beacon_int > 0 {`

```
// main.c:196-207 `rtw_sw_beacon_loss_check`. Die Firmware mit
// `FW_FEATURE_BCN_FILTER` macht es selbst.
```

## L5424 · `let watchdog_delay = 2_000_000u32 / 1024;`

```
// watchdog_delay = 2000000 / 1024 TU
```

## L5433-5436 · `#[allow(clippy::too_many_arguments)]`

```
/// Die Schleife des Treibers. `frist_us == 0` heisst: nicht mehr aufhoeren.
///
/// Sie bekommt Link UND Zaehler von aussen, damit Stufe 6b dort fortsetzt,
/// wo 6a aufgehoert hat.
```

## L5440-5441 · `Roam,`

```
/// Ein besserer AP ist gefunden — der Kandidat steht in
/// `link.roam.to`, und der Rufer meldet uns dort an.
```

## L5443 · `Frist,`

```
/// Die Frist von Stufe 6a ist abgelaufen — der Normalfall dort.
```

## L5445-5446 · `LinkLost,`

```
/// Die Zelle hat uns verloren (Deauth/Disassoc) oder die Firmware
/// hat sich fuer tot erklaert. Der Rufer verbindet neu.
```

## L5450-5468 · `const RO_TIDS: usize = 8;`

```
// ── Umsortierpuffer fuer empfangene A-MPDUs ───────────────────────
//
// `ieee80211_rx_reorder_ampdu` + `ieee80211_sta_reorder_release`
// (net/mac80211/rx.c), 802.11 §10.24.7 „Receive reordering buffer control".
//
// **Warum es ihn braucht, gemessen statt behauptet.** Ein Rahmen, der im
// A-MPDU ausfaellt, kommt im NAECHSTEN Buendel nach — wir lieferten bis
// hierher in ANKUNFTsreihenfolge, also 6,7,8…63 und dann 5. Unser TCP sieht
// eine Luecke, schickt Doppelquittungen, und drei davon loesen beim Sender
// eine Schnellwiederholung aus, die ueberfluessig ist. Auf Florians
// 5-GHz-Strecke (10 % CRC) am 2026-09-21 gemessen:
//
//     Fenster  256 KB  ->  81 Mbit   retr  40   dsack  40
//     Fenster 1024 KB  ->  68 Mbit   retr 802   dsack 887
//
// Mehr Fenster, zwanzigfache Wiederholungsrate, WENIGER Durchsatz — bei
// `lost=0` und `dsack ~ retr`, also war fast jede ueberfluessig. Ohne
// Umsortierung laesst sich das Empfangsfenster gar nicht aufmachen, und
// ohne grosses Fenster kommt man an 380 Mbit Bruttorate nie heran.
```

## L5470-5471 · `const RO_TIDS: usize = 8;`

```
/// So viele TIDs koennen gleichzeitig eine Sitzung haben. Florians AP
/// macht zwei auf (TID 0 und 6).
```

## L5473-5475 · `const RO_WIN: usize = 64;`

```
/// Die Fensterbreite, die wir im ADDBA ZUSAGEN. Beides darf nicht
/// auseinanderlaufen: wer 64 zusagt und 32 puffert, verwirft, was er
/// angenommen hat.
```

## L5477-5482 · `const RO_FRAME: usize = 11454;`

```
/// **Der Puffer haelt die rohe MPDU, nicht den 802.3-Rahmen.** mac80211
/// sortiert MPDUs um und entpackt ein A-MSDU erst DANACH
/// (`ieee80211_rx_h_amsdu` steht in der Kette hinter dem Umsortieren);
/// eine MPDU mit einem A-MSDU traegt viele Rahmen unter EINER
/// Folgenummer. Die Groesse ist deshalb die groesste MPDU, die wir im
/// VHT-Element ansagen (`MAX_MPDU_LENGTH_11454`).
```

## L5484-5486 · `const RO_POOL: usize = 64;`

```
/// Gleichzeitig zurueckgehaltene Rahmen ueber ALLE TIDs. Im Normalfall
/// liegt hier nichts — nur solange ein Loch offen ist. Laeuft der Pool
/// voll, wird zugestellt statt verworfen (`ro_full` zaehlt es).
```

## L5488-5489 · `const RO_TIMEOUT_MS: u32 = 100;`

```
/// Frist fuer ein Loch, danach wird darueber hinweg freigegeben.
/// mac80211: `HT_RX_REORDER_BUF_TIMEOUT` = HZ/10.
```

## L5496 · `fn ro_take(mpdu: &[u8]) -> Option<usize> {`

```
/// Einen freien Platz im Pool nehmen und die MPDU hineinlegen.
```

## L5501 · `unsafe {`

```
// SAFETY: ein Faden, ein Rufer — derselbe Vertrag wie bei RXBUF6.
```

## L5514 · `fn ro_release_slot(ls: &mut LinkStats, i: usize) {`

```
/// Den Rahmen an Platz `i` zustellen und den Platz freigeben.
```

## L5516-5518 · `let mut tmp = [0u8; RO_FRAME];`

```
// Der Rahmen wird ZUERST kopiert, dann zugestellt: `deliver` nimmt
// `&mut LinkStats`, und eine Anleihe auf den Pool darueber hinaus
// waere ein zweiter veraenderlicher Zugriff auf denselben Speicher.
```

## L5520-5522 · `let len = unsafe {`

```
// SAFETY: wie `ro_take` — ein Faden, ein Rufer. Die Zeiger werden
// ZUERST an Bezuege gebunden; ein `&(*ptr)[i]` mitten im Ausdruck
// waere eine stillschweigende Anleihe auf einen rohen Zeiger.
```

## L5536 · `fn ro_release_ready(ls: &mut LinkStats, tid: usize) {`

```
/// Alles freigeben, was ab dem Kopf LUECKENLOS daliegt.
```

## L5551-5552 · `fn ro_advance_to(ls: &mut LinkStats, tid: usize, want: u16) {`

```
/// Den Kopf bis `want` vorschieben und alles darunter herausgeben —
/// Loecher werden dabei uebersprungen (`ieee80211_sta_reorder_release`).
```

## L5554-5558 · `let dist = want.wrapping_sub(ls.ro_head[tid]) & 0x0fff;`

```
// **Ein Sprung weiter als das Fenster laeuft nicht Platz fuer Platz.**
// Die Entfernung kann bis 2047 betragen (halber Sequenzraum); dann
// waeren das 2047 Runden fuer hoechstens 64 liegende Rahmen. Hier
// wird stattdessen einmal ueber das Fenster gegangen und der Kopf
// direkt gesetzt.
```

## L5585-5593 · `fn ro_close(ls: &mut LinkStats, tid: usize) {`

```
/// Eine Sitzung beginnt: der ADDBA nennt die Startsequenz.
/// Eine Sitzung beenden und alles herausgeben, was noch liegt.
///
/// **Gerufen beim Wiederverbinden.** Eine Block-Ack-Sitzung gehoert der
/// ASSOZIATION: nach einem Wechsel haelt der Puffer sonst Rahmen der
/// neuen Zelle gegen die Folgenummern der alten, und die Plaetze im Pool
/// bleiben belegt. Die zurueckgehaltenen Rahmen werden VERWORFEN, nicht
/// zugestellt — sie gehoeren zu einer Verbindung, die es nicht mehr
/// gibt, und TCP holt sie sich ohnehin neu.
```

## L5617-5621 · `ls.ro_head[t] = (ssn >> 4) & 0x0fff;`

```
// **Bits 4..15, nicht das ganze Feld.** Das Block-Ack-Startfeld ist
// ein Sequenz-KONTROLLfeld: die unteren vier Bits sind die
// Fragmentnummer. Wer es ungeschoben nimmt, setzt den Kopf um das
// Sechzehnfache daneben — und der erste echte Rahmen sieht dann aus
// wie einer aus der fernen Vergangenheit.
```

## L5627-5628 · `fn deliver_or_reorder(ls: &mut LinkStats, tid: usize, sn: u16, have_sn: bool,`

```
/// **Der Eintritt.** Ohne Sitzung geht der Rahmen unveraendert durch —
/// das ist der Zustand vor dem Handschlag und jeder Rahmen ohne QoS.
```

## L5636-5640 · `if d >= 0x800 {`

```
// **Der Sequenzraum ist 12 Bit, also ist „aelter" die obere Haelfte.**
// Ein Rahmen unter dem Kopf ist zu spaet und war laengst durch ein
// Loch oder eine Frist ersetzt — ihn jetzt noch zuzustellen hiesse,
// die Reihenfolge, die wir gerade hergestellt haben, wieder zu
// brechen.
```

## L5646-5647 · `let want = sn.wrapping_sub(RO_WIN as u16 - 1) & 0x0fff;`

```
// Der Sender ist weiter, als unser Fenster reicht: den Kopf
// nachziehen, bis `sn` gerade noch hineinpasst.
```

## L5652 · `ls.ro_head[tid] = (ls.ro_head[tid] + 1) & 0x0fff;`

```
// Der Normalfall: er passt genau, nichts muss liegenbleiben.
```

## L5660-5661 · `ls.ro_old += 1;`

```
// Schon belegt — ein Duplikat, das der Tiefe-1-Zwischenspeicher
// nicht gesehen hat, weil andere Rahmen dazwischen lagen.
```

## L5674-5676 · `ls.ro_full += 1;`

```
// **Ein voller Pool wird ZUGESTELLT, nicht verworfen.** Die
// Reihenfolge leidet, die Daten nicht — und der Zaehler sagt,
// dass es passiert ist.
```

## L5684-5686 · `fn ro_tick(ls: &mut LinkStats) {`

```
/// Einmal je Runde: ein Loch, das zu lange offen ist, wird uebersprungen.
/// Ohne das haelt ein einziger verlorener Rahmen den Strom an, bis der
/// Sender 64 weitere geschickt hat.
```

## L5688-5692 · `if ls.ro_held.iter().all(|&h| h == 0) {`

```
// **Zuerst die billige Frage.** Diese Funktion steht in der
// Pumpschleife, und die dreht ueber eine Million Mal je Sitzung. Der
// Normalfall ist „es liegt nichts" — dann darf sie keinen einzigen
// Wirtsaufruf kosten. `now_us()` wird erst gefragt, wenn wirklich ein
// Loch offen ist.
```

## L5705-5706 · `let want = (ls.ro_head[tid] + 1) & 0x0fff;`

```
// Den Kopf um EINEN weiterschieben — das Loch ist damit
// uebersprungen — und dann alles herausgeben, was zusammenhaengt.
```

## L5714-5715 · `fn ro_due_ms(ls: &LinkStats) -> Option<u64> {`

```
/// In wie vielen Millisekunden `ro_tick` das naechste Loch ueberspringt;
/// None, solange nichts zurueckgehalten wird.
```

## L5727-5733 · `fn deliver_mpdu(ls: &mut LinkStats, f: &[u8]) {`

```
/// **Eine MPDU zustellen: umwandeln, oder ein A-MSDU entpacken.**
///
/// Der Ort in der Kette ist der von mac80211: NACH dem Umsortieren
/// (`ieee80211_rx_h_amsdu`, rx.c:3114, steht hinter
/// `ieee80211_rx_reorder_ampdu`). Ob die MPDU ein A-MSDU traegt, sagt das
/// Bit 7 des QoS-Steuerfelds (`IEEE80211_QOS_CTL_A_MSDU_PRESENT`,
/// rx.c:914-915) — und NUR das.
```

## L5746-5761 · `fn amsdu_to_8023s(ls: &mut LinkStats, f: &[u8], out: &mut [u8; 2048]) {`

```
/// cfg80211 `ieee80211_amsdu_to_8023s` (util.c:842-937), fuer eine
/// Station an einer Nicht-Mesh-Zelle — also `iftype` STATION,
/// `mesh_control` 0, und aus `__ieee80211_rx_h_amsdu` (rx.c:3028-3058)
/// `check_da` = unsere Adresse (addr1), `check_sa` = keine.
///
/// Der Rumpf ist eine Folge von Teilrahmen:
///
///     DA(6) SA(6) Laenge(2, gross-endig) | MSDU | Fuellung auf 4 Byte
///
/// Der letzte hat keine Fuellung. Passt EIN Teilrahmen nicht in den Rest,
/// wird die GANZE MPDU verworfen (`goto purge`) — auch die Teilrahmen, die
/// schon passten. Wir sammeln deshalb erst und stellen dann zu.
///
/// Abwehr der A-MSDU-Einschleusung (util.c:824-825, CVE-2020-24588): beim
/// ERSTEN Teilrahmen darf die Ziel-Adresse nicht der LLC/SNAP-Kopf sein —
/// sonst hat jemand eine gewoehnliche MSDU in ein A-MSDU umgedeutet.
```

## L5764-5766 · `let hdrlen = data_hdrlen(f);`

```
// Wo der Rumpf steht: derselbe Kopf-/CCMP-/Schwanz-Abzug wie bei einer
// einzelnen MSDU, nur ohne LLC/SNAP-Suche (der Rumpf eines A-MSDU
// beginnt mit dem ersten Teilrahmenkopf).
```

## L5778-5780 · `if amsdu_walk(body, &da_ok, &mut |_, _| {}).is_none() {`

```
// Zwei Durchgaenge statt einer Liste: der erste prueft ALLE
// Teilrahmen (einer kaputt heisst alle verworfen), der zweite stellt
// zu. Eine feste Liste haette eine Obergrenze, die cfg80211 nicht hat.
```

## L5788-5790 · `let (proto, payload) = if msdu.len() >= 8 {`

```
// `ieee80211_get_8023_tunnel_proto` (util.c:512-525): steht vorn
// RFC 1042 (ausser AARP und IPX) oder der Bruecken-Tunnel, wird
// er samt Typfeld abgenommen und der Typ in den 802.3-Kopf gesetzt.
```

## L5812-5815 · `fn amsdu_walk(body: &[u8], da_ok: &[u8; 6],`

```
/// Die Schleife aus `ieee80211_amsdu_to_8023s` ohne das Zustellen: geht
/// die Teilrahmen durch, ruft `each(anfang, laenge)` fuer jeden, der an
/// uns geht (oder Rundruf ist), und gibt `None` zurueck, wo cfg80211
/// `goto purge` nimmt.
```

## L5828 · `if subframe_len > remaining {`

```
// „the last MSDU has no padding"
```

## L5832 · `if offset == 0 && body[0..6] == LLC_SNAP_HDR {`

```
// „mitigate A-MSDU aggregation injection attacks"
```

## L5846-5852 · `fn deliver(ls: &mut LinkStats, eth: &[u8], is_eapol: bool) {`

```
/// **Der EINE Ausgang fuer einen empfangenen Datenrahmen.**
///
/// Herausgeloest, weil es ihn seit dem Umsortierpuffer ZWEIMAL gibt: der
/// Rahmen, der in Reihenfolge ankommt, geht sofort hier hindurch; einer,
/// der ein Loch fuellt, wird gespeichert und spaeter durch dieselbe Tuer
/// geschickt. Zwei Ausgaenge waeren zwei Semantiken, und eine davon wuerde
/// irgendwann abweichen.
```

## L5862-5876 · `let raw = &eth[14..];`

```
// `EV_EAPOL_RX` = [0x84][len u16 LE][Rahmen] — und der
// Rahmen ist der EAPOL-RUMPF hinter dem Ethertyp.
//
// **Auf die ANGESAGTE Laenge kuerzen.** Der EAPOL-Kopf
// traegt sie in den Bytes 2..4 (802.1X, gross-endig), und
// der ganze Rahmen ist 4 + diese Zahl. Was die Hardware
// dahinter anhaengt, gehoert nicht dazu: `WLAN_RCR_CFG`
// hat APP_FCS, APP_MIC und APP_ICV gesetzt, also liefert
// der Deskriptor mehr Bytes, als der Rahmen lang ist.
//
// **Das ist nicht kosmetisch.** `wifid` rechnet den MIC
// ueber die GANZE Scheibe, die es bekommt
// (`compute_mic`: `frame.len()`). Vier Bytes zu viel, und
// msg3 schlaegt fehl — msg1 nicht, denn das traegt gar
// keinen MIC. Genau dieses Muster stand im Geraetelauf.
```

## L5919 · `let (rxbuf, ethbuf, cmdbuf) = unsafe {`

```
// SAFETY: einfaedig, je ein Rufer, keiner verlaesst diese Funktion.
```

## L5927-5930 · `let bssid = link.bssid;`

```
// `frist_us == 0` heisst: nicht mehr aufhoeren. Stufe 6a gibt acht
// Sekunden vor — der Handschlag braucht vier Rahmen und ist in
// Millisekunden durch, wer laenger wartet, wartet auf einen Fehler.
// Stufe 6b ruft dieselbe Schleife ohne Frist.
```

## L5932-5934 · `let bssid = link.bssid;`

```
// Der Rueckruf braucht die BSSID, um einen Deauth der EIGENEN Zelle
// von dem des Nachbarn zu unterscheiden. Als Kopie, damit er `link`
// nicht festhalten muss, waehrend die Schleife darauf schreibt.
```

## L5941 · `let mut probe_due = true;`

```
// Ein Datenrahmen je Watchdog-Takt bekommt eine Quittung.
```

## L5947-5950 · `let ampdu_buf = read_ampdu_buf();`

```
// **Das Empfangsfenster der Aggregation, aus `sys/config/wifi`.**
// `ampdu: off` schaltet sie ab, `ampdu: 16` gibt ein anderes
// Fenster. Die Vorgabe ist klein und der Grund steht bei
// `build_addba_resp`: es gibt keinen Umsortierpuffer.
```

## L5952-5956 · `let txagg = read_txagg();`

```
// **`txagg:` deckelt nur die SENDErichtung.** `ampdu:` ist die
// Empfangsseite und bleibt, wo sie war; die zwei Richtungen sind
// getrennte Sitzungen und gehoeren nicht unter einen Schalter.
// Vorgabe AN — der Rueckfall `off` ist genau der Zustand von 0.51.1,
// also einer, der gemessen ist.
```

## L5959-5961 · `let beacon_int: u16 = 100;`

```
// `bss_conf.beacon_int` — 100 TU ist der Wert, den praktisch jeder AP
// ansagt; aus dem Beacon gelesen wird er noch nicht, und eine Null
// waere hier schlimmer als der Normalfall (sie teilt).
```

## L5963-5967 · `let mut cur_bw = d.cur_bw as u8;`

```
// **Die Breite, auf der die PHY steht, geht in den Empfangsstatus.**
// Hier stand eine NULL, und die heisst „20 MHz": jeder Rahmen mit
// `rxsc == 0` — also jeder, der die ganze Breite belegt und damit der
// Normalfall — wurde als 20 MHz gemeldet. Das ist keine fehlende
// Messung, sondern eine falsche.
```

## L5974 · `let mut acc = rx::WdAcc::new(link.si.avg_rssi);`

```
// ── Empfangen ────────────────────────────────────────────
```

## L5976-5983 · `let messen = d.stats.rx_throughput >= 10;`

```
// **Die Zeit IM Ring, und warum sie hier gemessen wird.**
// `rahmen/blick 1,2` kann zweierlei heissen: schnell genug, oder
// exakt so langsam wie die Ankunft. Zwischen beidem entscheidet
// nur, wieviel Wanduhrzeit im Empfangspfad steckt. Gemessen
// wird NUR, wenn etwas kam (rund 1400 Mal je Sekunde, also
// 2800 Wirtsaufrufe) — bei den 65 000 leeren Bliecken waere es
// die Messung, die den Zustand erzeugt.
// Fuer den Rueckruf: er darf `d` nicht anfassen.
```

## L5992-6004 · `if st.phy_status {`

```
// **In welcher Breite kam er herein — und NUR, wenn die
// Frage ueberhaupt beantwortet ist.**
//
// Das Breitenfeld des Deskriptors (`GET_RX_DESC_BW`) ist nur
// auf den Rahmen gefuellt, die einen PHY-Status tragen; in
// einem A-MPDU ist das einer von vielen. Ohne diese Bedingung
// stand im ersten Geraetelauf `20:72302 40:32 80:12482` —
// und das las sich, als kaeme das meiste schmal an, waehrend
// die Ratenmeldung `VHT 2SS` sagte, was es nur bei 80 MHz
// gibt. Die 12482 waren echt, die 72302 waren leere Felder.
//
// Es steht vor dem Ausstieg unten: eine Breite, die nur die
// weiterverarbeiteten Rahmen misst, misst den Ausstieg mit.
```

## L6013-6016 · `if st.is_c2h {`

```
// **C2H wurde bis 0.26.0 verworfen.** Der Ratenbericht der
// Firmware ist die Eingabe von `config_swing_table` und
// `rrsr_update`; ohne ihn rechnet der Watchdog auf der
// Anfangsrate.
```

## L6030-6032 · `let v1 = c.id as u32 == C2H_HALMAC;`

```
// **Beide Wege.** `C2H_CCX_TX_RPT` traegt die
// Quittung selbst (V0), `C2H_HALMAC` traegt sie
// als Unterkommando 0x0f (V1) — fw.c:93-113.
```

## L6041-6044 · `acc.c2h_seen[acc.n_c2h_seen] = c.id;`

```
// **Und was sonst hereinkommt, wird gezaehlt.**
// Dass die Quittung ausblieb, war in 0.26.0 eine
// Null ohne Hinweis; ein Zensus der Kennungen
// haette die Frage in EINEM Lauf beantwortet.
```

## L6052-6054 · `if f.len() >= 16 && f[10..16] == bssid {`

```
// mlme.c:131-145 — JEDER Rahmen vom AP setzt die Wache
// zurueck, nicht nur eine Bake. `addr2` ist der Sender, und
// bei allem, was von ihm kommt, ist das die BSSID.
```

## L6057-6060 · `if f[0] == 0x80 {`

```
// **Eine Bake DIESER Zelle kann eine Wechselansage
// tragen.** Gelesen wird sie hier, ausgefuehrt draussen:
// der Kanalwechsel braucht `trx`, und der Rueckruf darf
// es nicht halten.
```

## L6064-6068 · `None => acc.beacon_ohne_csa = true,`

```
// mlme.c:2820-2824: `else if (res)
// ieee80211_sta_abort_chanswitch(link)` — `res`
// ist 1, wenn in dieser Bake kein CSA-Element
// steht. **Eine Ansage, die verschwindet, ist
// zurueckgezogen.**
```

## L6071-6072 · `acc.beacon_dbm = Some(st.signal_power);`

```
// mlme.c:6789-6798 — der Pegel JEDER Bake dieser
// Zelle geht in den geglaetteten Wert.
```

## L6076-6091 · `if f[0] & 0x0c == DOT11_FC_TYPE_DATA {`

```
// **Wieviele Rahmen der AP je Sendevorgang buendelt.**
//
// `ppdu_cnt` sind zwei Bit im Empfangsdeskriptor, und sie
// zaehlen die PPDUs hoch — rtw88 liest das Feld und benutzt
// es nie. Ein Wechsel heisst: neuer Sendevorgang. Damit ist
// `Rahmen / PPDUs` die ECHTE Aggregatgroesse in
// Empfangsrichtung, von der Hardware gezaehlt und nicht
// gerechnet.
//
// Sie ist die Gegenprobe zu `sendering` auf der Sendeseite
// und beantwortet die Frage, die keine Durchsatzzahl
// beantwortet: liegen die 41 % Effizienz an der STRECKE oder
// daran, dass gar nicht gebuendelt wird.
//
// Nur DATENrahmen: eine Bake ist immer ihr eigener
// Sendevorgang und wuerde den Schnitt druecken.
```

## L6096-6109 · `let dt = st.tsf_low.wrapping_sub(ls.last_tsf);`

```
// **Der Abstand zweier Sendevorgaenge, von der
// HARDWARE gestempelt.** `tsf_low` ist die
// 802.11-Uhr in Mikrosekunden, gesetzt beim Empfang
// — keine Wirtsuhr, keine Schaetzung.
//
// Sie beantwortet die Frage, an der jede Rechnung
// aus Raten und Laengen scheitert: **wieviel von der
// Zeit sendet der AP ueberhaupt?** Steht der Abstand
// bei 3 ms, waehrend das Aggregat 1 ms dauert, ist
// die Luft zu einem Drittel belegt — und dann ist
// nicht die Strecke der Deckel.
//
// Der KLEINSTE Abstand ist der Massstab: er ist das,
// was die Strecke kann, wenn nichts dazwischenkommt.
```

## L6112-6117 · `if ls.last_tsf != 0 && d > 0 {`

```
// **Die langen Abstaende werden EINGEORDNET, nicht
// verworfen.** In 0.53.0 fielen sie aus der Rechnung,
// und genau sie waren der Befund: 190 Stueck a 13 ms
// in einem Lauf von 5,4 s — 45 % der Zeit. Wer sie
// wegwirft, misst die Strecke nur dann, wenn sie
// laeuft.
```

## L6139-6140 · `ls.last_rx_at = host::now_us();`

```
// Der Augenblick, in dem der Kernel Daten bekommt —
// Anfang der Messstrecke unten.
```

## L6143 · `rx::watchdog_feed(&mut acc, st, f, &mac, &bssid,`

```
// rx.c:100-133 + phy.c:678-704 — was der Watchdog braucht.
```

## L6146-6150 · `if let Some((sub, act)) = rx::mgmt_census(f, &bssid) {`

```
// **Zuerst der Rauswurf.** Er ist ein Verwaltungsrahmen und
// kaeme durch `rx_to_8023` nicht hindurch. Nur SEHEN hier —
// gehandelt wird nach dem Ringleeren.
// Der Zensus der Verwaltungsrahmen laeuft VOR dem Rauswurf
// und schliesst ihn ein — ein Deauth ist auch einer.
```

## L6156-6158 · `if act == Some((DOT11_ACTION_CAT_BA, DOT11_ACTION_ADDBA_REQ))`

```
// **Der ADDBA Request wird nur GESEHEN**, beantwortet
// wird er nach dem Ringleeren — ein Sendevorgang gehoert
// nicht in einen Rueckruf, der `trx` nicht halten darf.
```

## L6182-6198 · `let mut ro_tid = RO_TIDS;`

```
// **Doppelte 802.11-Wiederholungen verwerfen** —
// `ieee80211_rx_h_check_dup` (rx.c:1438-1490), 802.11-2012
// §9.3.2.10 „Duplicate detection and recovery".
//
// Der AP wiederholt einen Rahmen auf MAC-Ebene, wenn unsere
// Quittung ausbleibt oder zu spaet kommt. Die Wiederholung
// traegt dasselbe Sequenz-Kontrollfeld und das Retry-Bit. Wer
// sie nicht verwirft, liefert dieselben Bytes ZWEIMAL an TCP.
//
// Am Geraet gemessen (2026-09-21, WLAN mit D-SACK): der Server
// meldete `dsack=481` bei `retrans=186`. Eine TCP-Wiederholung
// kann hoechstens EIN Duplikat erzeugen — die uebrigen ~295
// entstanden also UNTER TCP, und das hier ist die Stelle.
//
// Verglichen wird das GANZE Feld, nicht nur die Sequenznummer:
// die unteren vier Bits sind die Fragmentnummer, und zwei
// Bruchstuecke desselben Rahmens sind keine Duplikate.
```

## L6220-6225 · `if is_qos && idx < RO_TIDS && sc & 0x000f == 0 {`

```
// **Nur ganze Rahmen werden umsortiert.** Die unteren
// vier Bits sind die Fragmentnummer; ein Bruchstueck
// gehoert in die Zusammensetzung, nicht in den
// Umsortierpuffer, und die faehrt mac80211 auch
// getrennt (`ieee80211_rx_h_defragment` laeuft VOR
// dem Puffer).
```

## L6233-6235 · `if f.len() < 24 || f[0] & 0x0c != DOT11_FC_TYPE_DATA`

```
// Nur Datenrahmen mit Rumpf gehen weiter — dieselbe erste
// Pruefung wie in `rx_to_8023`. Umgewandelt wird erst beim
// Zustellen (`deliver_mpdu`), nach dem Umsortieren.
```

## L6242-6243 · `if f[4] & 0x01 == 0 {`

```
// rx.c:14-32 `rtw_rx_stats` — nur UNICAST zaehlt, und
// gezaehlt wird die Laenge des 802.11-Rahmens.
```

## L6251-6257 · `ro_tick(ls);`

```
// **Die Form der Schleife, ohne einen einzigen zusaetzlichen
// Wirtsaufruf gemessen.** Wieviele Rahmen ein Blick bringt sagt,
// auf welcher Seite der Deckel liegt: knapp ueber eins heisst,
// wir sehen schneller nach als etwas kommt (die Luft ist die
// Grenze); volle Stapel heissen, wir kommen nicht nach.
// Ein Loch, das zu lange offen steht, haelt sonst den ganzen
// Strom an. Einmal je Runde reicht — die Frist ist 100 ms.
```

## L6270-6273 · `if let Some(dbm) = acc.beacon_dbm.take() {`

```
// mlme.c:4525-4528 `if (!ifmgd->probe_send_count)
// ieee80211_reset_ap_probe(sdata)` — der AP hat geantwortet.
// **Es zaehlt JEDER Rahmen von ihm**, nicht nur eine Antwort auf
// unsere Frage: wer Daten schickt, lebt.
```

## L6277-6288 · `let misst = d.stats.rx_throughput >= 10;`

```
// **Gemessen wird nur, waehrend Daten fliessen.**
//
// Die Histogramme summierten bis hierher die ganze Verbindung —
// elf Downloads UND zweieinhalb Minuten Leerlauf dazwischen. Ein
// Abstand von 19 ms zwischen zwei Rahmen heisst waehrend eines
// Downloads „die Strecke stand still" und im Leerlauf „es war
// nichts zu senden". Dieselbe Zahl, zwei Bedeutungen, und der
// Mittelwert darueber ist keine von beiden.
//
// `rx_throughput` steht in Mbit und wird je Watchdog (zwei
// Sekunden) nachgezogen. Zehn ist die Grenze zwischen
// Hintergrundverkehr und Uebertragung.
```

## L6297 · `acc.merge(&mut d.dm, &mut link.si);`

```
// Was der Ringdurchlauf dem Watchdog zugetragen hat, eintragen.
```

## L6316-6327 · `if let Some(r) = acc.addba_resp.take() {`

```
// ── Die Aggregation FRAGEN ───────────────────────────────
//
// Der Zweig darunter beantwortet die Bitte des AP, dieser hier
// stellt unsere. Beides ist dieselbe Sache in zwei Richtungen,
// und wir hatten bis 0.52.0 nur die eine.
//
// **Gefragt wird erst nach dem Vierwegehandschlag.** Vorher
// liegt kein Schluessel, der AP wuerde einen Verwaltungsrahmen
// ungeschuetzt sehen, und vor allem: bis dahin fliessen keine
// Daten, also gibt es nichts zu aggregieren. Es ist auch der
// Moment, in dem die Folgenummer noch still steht — und die
// Startsequenz im Antrag MUSS die sein, ab der wir senden.
```

## L6335-6336 · `link.ba_tx.win = r.buf_size.min(sta::BA_TX_BUF_SIZE);`

```
// agg-tx.c:992 — der AP darf WENIGER zusagen, als wir
// erbeten haben, und mehr als 64 kann HT nicht.
```

## L6374-6377 · `link.ba_tx.factor = sta::tx_ampdu_factor(caps.ht_ampdu_factor);`

```
// Die zwei Zahlen fuer den Deskriptor kommen aus den
// HT-Faehigkeiten des AP und stehen fest, sobald er
// zusagt — gerechnet werden sie hier, weil `caps` hier
// zur Hand ist.
```

## L6409-6412 · `let qos_tid = if link.ba_tx.state == BaState::Laeuft {`

```
// **Erst wenn die Sitzung steht, wird QoS gesendet.** Ein
// QoS-Rahmen ohne Sitzung ginge auch, aber dann aendert sich die
// Rahmenart mitten im Betrieb, und die Folgenummer waere schon
// vergeben, bevor der Antrag seine Startsequenz nennt.
```

## L6419-6422 · `ls.addba_drop += acc.addba_drop;`

```
// ── Die Aggregation zulassen ─────────────────────────────
// Der AP bittet mit einem ADDBA Request und wiederholt ihn,
// solange keine Antwort kommt — im Geraetelauf 180 Mal, und
// genau so lange konnte er nicht aggregieren.
```

## L6438-6441 · `ro_open(ls, req.tid, req.ssn);`

```
// **Erst jetzt**, und mit der Startsequenz aus SEINER
// Bitte: ab diesem Rahmen aggregiert der AP, und ab
// hier muss umsortiert werden. Frueher gaebe es einen
// Kopf ohne Sitzung, spaeter ein Loch am Anfang.
```

## L6465-6466 · `d.dm.tx_rate = rate;`

```
// fw.c:308 — `dm_info->tx_rate` unabhaengig von der Station,
// `si->ra_report.desc_rate` nur bei passender mac_id.
```

## L6473-6477 · `if let Some((deauth, reason)) = ls.gone.take() {`

```
// ── Der Rauswurf, gehandelt ──────────────────────────────
// Gesehen hat ihn der Rueckruf oben; hier ist der Ring leer und
// `link` wieder frei. **Der Kernel erfaehrt es als erster** —
// bis hierher glaubte er an `carrier UP` und schob Pakete in
// eine tote Leitung.
```

## L6482-6485 · `if ls.kicked <= 3 {`

```
// **Gezaehlt wird jeder, gedruckt die ersten drei.** Ein AP
// schickt seinen Rauswurf gern als Salve; die vierte Zeile
// sagt nichts, was die erste nicht sagte, und der Zaehler im
// Bericht bleibt vollstaendig.
```

## L6518-6520 · `if frist_us == 0 {`

```
// **Und zurueck zum Rufer.** Stufe 6a laeuft weiter (dort
// ist ein Rauswurf ein Befund fuer das Tor, keine Aufgabe);
// in 6b baut der Rufer die Verbindung neu auf.
```

## L6526-6538 · `if acc.beacon_ohne_csa && acc.csa.is_none() && link.csa.is_some() {`

```
// ── Kanalwechsel (CSA) ───────────────────────────────────
//
// mlme.c:2980-3010. Jede Bake mit einer Ansage rechnet die
// Frist NEU — `(max(count, 1) - 1) * beacon_int` —, damit eine
// verpasste Bake den Termin nicht verschiebt. `count` zaehlt im
// Beacon herunter, und 0 wie 1 heissen beide „jetzt".
// **Zuruecknehmen, wenn die Ansage verschwindet** (mlme.c:2822).
//
// Hier fehlte der ganze Zweig, und er hat Florian auf einen
// Kanal gesetzt, auf dem der AP gar nicht war: wir merkten uns
// die Ankuendigung, der AP hoerte auf, sie zu senden, und wir
// zogen trotzdem um. Danach hoerten wir ihn mit -90 dBm statt
// -23, das Roaming feuerte, und die Verbindung war weg.
```

## L6546 · `link.csa_at_ms = now + (tu * 1024) / 1000;`

```
// Ein TU sind 1024 us; wir rechnen in Millisekunden.
```

## L6590-6597 · `link.csa_zurueck = Some((link.channel, CellWidth {`

```
// **Der Rueckweg, falls dort niemand ist.**
//
// mac80211 wartet nach dem Wechsel auf eine Bake
// (`csa.waiting_bcn`, mlme.c:2812). Wir merken uns den
// alten Kanal und kehren um, wenn binnen einer Sekunde
// keine Bake der Zelle kommt — sonst sitzt man auf einem
// leeren Kanal und merkt es erst, wenn die Wache
// anschlaegt.
```

## L6611-6612 · `ls.poll_on = false;`

```
// Die Verbindungswache faengt von vorn an: auf dem neuen
// Kanal haben wir noch keine Bake gesehen.
```

## L6618 · `if let Some((alt_ch, alt_w)) = link.csa_zurueck {`

```
// **Kam auf dem neuen Kanal eine Bake?** Wenn nicht, zurueck.
```

## L6645-6649 · `if ls.poll_on && now >= ls.probe_timeout_ms {`

```
// ── Die Verbindungswache ─────────────────────────────────
//
// mlme.c:8516-8560, der Zweig `IEEE80211_STA_CONNECTION_POLL`:
// laeuft die Frist ab und sind noch Versuche uebrig, wird noch
// einmal gefragt; sonst ist die Verbindung verloren.
```

## L6669 · `let gerichtet = ls.probe_send_count < PROBE_UNICAST_LIMIT;`

```
// mlme.c:4391-4396 — die letzten drei als Rundruf.
```

## L6691 · `loop {`

```
// ── Kommandos von wifid ──────────────────────────────────
```

## L6699 · `Some(CMD_TX_EAPOL) if cmd.len() >= 3 => {`

```
// TX_EAPOL: [op][len u16 LE][Rahmen]
```

## L6709-6713 · `let enc = link.ptk_installed;`

```
// **Verschluesselt, sobald die PTK steht.** msg2 und
// msg4 gehen im Klartext hinaus, wie sie muessen —
// ein GRUPPEN-Neuschluessel kommt Minuten spaeter,
// mit der PTK laengst im Speicher, und der AP
// erwartet ihn geschuetzt.
```

## L6715-6720 · `let sn = ls.arm_probe(now);`

```
// **EAPOL bekommt IMMER eine Quittung.** In Linux
// kommt das aus mac80211: Rahmen des Steuerports
// tragen `IEEE80211_TX_CTL_REQ_TX_STATUS`. Es
// sind die Rahmen, deren Verlust die Verbindung
// kostet — und beim letzten Fehler genau die, von
// denen der AP keinen einzigen hoerte.
```

## L6724-6725 · `pci::tx_kick_off_queue(h, trx, pci::Q_BE);`

```
// Ein einzelner Rahmen, und einer, auf den
// der AP wartet: sofort.
```

## L6738 · `Some(CMD_SET_KEY) if cmd.len() >= 5 => {`

```
// SET_KEY: [op][key_type][key_idx][cipher][key_len][key][rsc 6]
```

## L6746-6747 · `let slot = if group { key_idx.min(3) } else { 0 };`

```
// Paarschluessel auf Platz 0, Gruppenschluessel auf
// seinen Index — so haelt es auch mac80211.
```

## L6768 · `Some(CMD_AUTHORIZED) => {`

```
// AUTHORIZED: der Handschlag ist durch.
```

## L6783-6795 · `let sendesperre = link.csa.map_or(false, |c| c.mode != 0);`

```
// ── Senden, was der IP-Stapel loswerden will ─────────────
//
// **Erst alles in den Ring, dann EINMAL anstossen** — tx.c:660-676.
// Die Schleife holte schon immer, bis nichts mehr da war; neu ist,
// dass der Anstoss danach kommt statt je Rahmen. Damit sieht die
// Hardware bei ihrem naechsten Griff nach der Sendegelegenheit den
// ganzen Stapel und kann ihn zu einem A-MPDU zusammenfassen.
// mlme.c:2982 `if (csa_ie.mode) ieee80211_vif_block_queues_csa`.
//
// **`mode = 1` heisst: ab jetzt nichts mehr senden.** Der AP
// raeumt den Kanal — auf einem DFS-Kanal, weil er Radar erkannt
// hat, und dann ist jeder weitere Rahmen von uns einer zuviel
// auf einer Frequenz, die frei werden muss.
```

## L6805-6811 · `let sn = if probe_due { probe_due = false; ls.arm_probe(now) }`

```
// **Ein Datenrahmen je Watchdog-Takt wird quittiert.**
// Das ist eine benannte Abweichung: Linux erfaehrt den
// Sendeerfolg ueber mac80211 und fragt deshalb nur fuer
// Steuerrahmen nach. Uns fehlt dieser Weg ganz, und eine
// Leitung, auf der NICHTS quittiert wird, war zweimal der
// Fehler. Einer je zwei Sekunden kostet nichts und
// beantwortet „hoert der AP mich ueberhaupt".
```

## L6814-6827 · `if ls.last_rx_at != 0 {`

```
// **Wie lange unser Stapel braucht.**
//
// Beim Herunterladen ist praktisch jeder gesendete Rahmen
// eine TCP-Quittung, die von empfangenen Daten ausgeloest
// wurde. Der Abstand zwischen „wir haben dem Kernel Daten
// gegeben" und „der Kernel gibt uns einen Rahmen zurueck"
// ist damit die Zeit, die UNSERE Seite zur Umkehr
// braucht — und sie steckt eins zu eins in der RTT, die
// der Server misst.
//
// Das ist die Zahl, die „liegt es an der Luft oder an
// uns" entscheidet: bei 12,7 ms gemessener RTT und
// ~4,6 ms Sendezeit fehlen acht Millisekunden, und
// entweder stehen sie hier oder beim AP.
```

## L6846-6847 · `if ethbuf[0] & 0x01 == 0 {`

```
// tx.c `rtw_tx` — dieselbe Buchfuehrung wie beim
// Empfang, damit `tx_throughput` eine Zahl hat.
```

## L6855-6857 · `let im_ring = pci::tx_pending(h, trx, pci::Q_BE);`

```
// **Erst messen, dann anstossen.** Nach dem Anstoss ist
// die Zahl eine andere — die Hardware faengt an, sobald
// der Schreibzeiger steht.
```

## L6872 · `pci::tx_isr(h, trx, pci::Q_BE);`

```
// `rtw_pci_tx_isr` — den Lesezeiger nachziehen.
```

## L6876-6879 · `ls.purge_probes(now);`

```
// ── Alle zwei Sekunden: `rtw_watch_dog_work` ─────────────
// **Der Takt ist Linux'**: `RTW_WATCH_DOG_DELAY_TIME` = HZ * 2.
// Die Nachfuehrungen darin rechnen auf dem, was seit dem letzten
// Takt hereinkam — ein anderer Takt waere ein anderer Regler.
```

## L6901-6912 · `if roam_mode != RoamMode::Aus`

```
// ── Roaming: wechseln, BEVOR es abreisst ─────────
//
// mlme.c:6800-6828 `ieee80211_handle_beacon_sig`: erst ab
// vier Baken, dann Schwelle mit HYSTERESE. Das Ereignis
// feuert erst wieder, wenn der Pegel um die Hysterese
// darueber hinausgeht — sonst loest ein einzelner schlechter
// Beacon einen Umhoerversuch aus, und danach der naechste.
// **Erst nach dem Vierwegehandschlag.** Ohne diese
// Bedingung lief der Ausloeser schon in Stufe 6a, also
// WAEHREND des Handschlags — und `stage6a` wirft die
// Rueckgabe des Pumpens weg, der Kandidat waere also nur
// haengengeblieben.
```

## L6915-6918 · `&& now.saturating_sub(link.roam.last_roam_ms) > ROAM_GAP_MS`

```
// **Und nicht gleich nach einem Aufbau.** `authorized`
// steht, sobald der Handschlag durch ist; die Zelle hat
// dann aber noch keine vier Baken geliefert und der
// Verkehr faengt gerade erst an.
```

## L6925-6947 · `let tief = sig < ROAM_THOLD_DBM`

```
// **Der Pegel ist der einzige Ausloeser — wie bei
// mac80211.**
//
// Hier standen zwei Zugaben von mir, und BEIDE waren
// derselbe Fehler: ein Zaehler, der nicht misst, was ich
// annahm.
//
// * „Rate am Boden" las `curr_rx_rate`, also die Rate des
//   LETZTEN Rahmens — und eine Bake geht immer mit OFDM
//   6M hinaus. Fast immer wahr.
// * „jeder vierte Rahmen kaputt" las `ofdm_err/ofdm_ok`.
//   Die kommen aus einem CRC32-Zaehler der HARDWARE
//   (rtw8822c.c:2855) und zaehlen OFDM-Rahmen AUF DEM
//   KANAL, auch fremde. rtw88 benutzt sie fuer nichts
//   ausser Debug- und Coex-Ausgaben. Auf dieser Strecke
//   liegt die Quote dauerhaft bei 21-25 %, also war auch
//   dieser Ausloeser permanent wahr — und die Verbindung
//   wechselte im Zehnsekundentakt.
//
// Was bleibt, ist `ieee80211_handle_beacon_sig`: Schwelle
// mit Hysterese auf dem geglaetteten Bakenpegel, und
// sonst nichts. **Ein Ausloeser, der nie wieder ausgeht,
// ist keiner.**
```

## L6950-6951 · `let ruhig = d.stats.rx_throughput < 2 && d.stats.tx_throughput < 2;`

```
// Nie suchen, waehrend Daten fliessen: ein
// Umhoerversuch kostet dann Durchsatz fuer nichts.
```

## L7014-7031 · `if b.best > ROAM_THOLD_DBM {`

```
// **Wir haben uns GERADE gemessen.**
//
// Am Geraet: der geglaettete Bakenpegel sagte
// -81 dBm und loeste den Umzug aus, waehrend
// derselbe Suchlauf dieselbe Zelle in
// derselben Sekunde mit -19 dBm hoerte — 62 dB
// auseinander. Umgezogen wurde von -19 auf
// -53, also vom besten auf einen schlechteren
// AP.
//
// Welche der beiden Zahlen stimmt, ist noch
// offen und wird gemessen. Aber die FRISCHE
// ist die, auf die man sich verlassen kann:
// eine Probe von eben schlaegt einen
// Mittelwert, der aus Rahmen stammt, die
// niemand nachgezaehlt hat. Liegt sie ueber
// der Schwelle, gibt es keinen Grund zu
// gehen.
```

## L7037-7046 · `let besser = match best {`

```
// **Unter mehreren Guten gewinnt die
// BREITERE, erst dann die lautere.**
//
// Hier stand nur `b.best > x.best`. Am
// Geraet standen vier Zellen als BESSER da —
// `K7 -30 dBm 20 MHz` und
// `K104 -67 dBm 80 MHz` —, und die Auswahl
// nahm die laute schmale. `roam_better`
// prueft die Breite gegen UNS; unter den
// Kandidaten hat sie niemand verglichen.
```

## L7071-7072 · `link.roam.ave = dm::Ewma::new();`

```
// Den geglaetteten Wert neu saeen, sonst loest er
// beim naechsten Takt wieder aus.
```

## L7099-7110 · `if ls.authorized || ls.link_up_sent {`

```
// **Die Verbindung ORDENTLICH abbauen,
// genau wie auf dem Rauswurf-Weg.**
//
// Hier stand nur `return`. Der Kernel
// behielt damit seinen Traeger und schob
// waehrend der ganzen Neuanmeldung
// weiter Daten hinein (`tx refused
// no-link`, `SENDETOR ZU`), und `wifid`
// sah kein `link down` — es haette
// seinen alten Supplicant behalten,
// waehrend wir uns bei einer ANDEREN
// Zelle anmelden.
```

## L7126-7132 · `if d.beacon_loss && !ls.poll_on {`

```
// mlme.c:4427-4480 `ieee80211_mgd_probe_ap(sdata, true)`.
//
// **Hier wurde `d.beacon_loss` bis 0.55.0 nie gelesen.** Der
// Wert wird seit Stufe 6 richtig gerechnet
// (`rtw_sw_beacon_loss_check`), und eine Verbindung, deren AP
// verschwindet, blieb trotzdem stehen — bis jemand neu
// startete.
```

## L7136 · `ls.probe_timeout_ms = 0; // sofort fragen`

```
// sofort fragen
```

## L7140-7142 · `for i in 0..DESC_RATE_MAX {`

```
// `rtw_phy_stat_rate_cnt` hat das Fenster gerade nach
// `last_pkt_count` geschoben — jetzt und nur jetzt steht es
// vollstaendig da.
```

## L7147-7148 · `ls.ht_ok += d.dm.ht_ok_cnt as u64;`

```
// Dieselbe Stelle, derselbe Grund: `false_alarm_statistics`
// hat gerade gelesen UND zurueckgesetzt.
```

## L7155-7159 · `if now.wrapping_sub(report_ms) >= 1000 {`

```
// ── Einmal je Sekunde: der Bericht fuer `wlan` ───────────
// Die Luft ist fuer den Kernel unsichtbar. Rate, Zaehler und
// Schluesselzustand stehen nirgends sonst — ohne sie ist eine
// Leitung, die wegen einer Legacy-Rate langsam ist, nicht von
// einer zu unterscheiden, die wegen voller Schlangen langsam ist.
```

## L7165-7168 · `if got > 0 {`

```
// ── RX-Stille als Wachhund ───────────────────────────────
// Auf einer lebenden Zelle kommt IMMER etwas: Beacons allein sind
// zehn je Sekunde. Voelliges Schweigen heisst, dass der Ring
// steht, nicht dass die Luft leer ist.
```

## L7175-7176 · `host::loud_begin();`

```
// Ein stehender Ring ist kein Stufenbefund, sondern ein
// Fehler: laut, auch ohne `debug: 1`.
```

## L7187-7203 · `{`

```
// ── Wann wir die Hand vom Ring nehmen ────────────────────
//
// **Hier stand `if got == 0 { sleep_ms(1) }`, und das war der
// Durchsatzdeckel.** Zwischen zwei Buendeln ist der Ring einen
// Moment leer — beim ersten leeren Blick eine ganze Millisekunde
// zu schlafen heisst, hoechstens tausend Mal je Sekunde
// nachzusehen. Gemessen: 1375 Rahmen/s bei 1,37 Rahmen je Blick,
// also genau `1000 x Buendelgroesse`. Die Aggregation aus 0.29.0
// machte die Buendel groesser und brachte deshalb nur +26 %
// statt eines Vielfachen.
//
// Jetzt die Form, die Linux NAPI nennt: **ein Budget leerer
// Blicke, dann erst schlafen** — und liegend bleiben, bis wieder
// etwas kommt. Unter Last faellt der Zaehler bei jedem Buendel
// auf null und wir schlafen nie; im Leerlauf ist das Budget nach
// einem knappen halben Millisekunde aufgebraucht und wir
// schlafen wie vorher.
```

## L7206 · `unsafe {`

```
// SAFETY: wie `WD_MAX`.
```

## L7214 · `let irq = unsafe { IRQ_VEC } >= 0;`

```
// SAFETY: nur dieser Fiber liest IRQ_VEC.
```

## L7217-7221 · `pci::irq_recognized(h);`

```
// **Mit MSI: parken, bis der Chip etwas meldet.** Die Form von
// `rtw_pci_napi_poll`, wenn weniger als das Budget kam: HISR
// quittieren (sonst keine neue Flanke), HIMR scharf, und den Ring
// noch einmal ansehen — was zwischen dem letzten Blick und dem
// Scharfmachen ankam, loest keinen Interrupt mehr aus.
```

## L7224-7236 · `pci::tx_isr(h, trx, pci::Q_BE);`

```
// **Und den Sendering noch einmal abraeumen — NACH dem
// Scharfmachen.** HIMR steht die ganze Runde ueber auf null, und
// bei null setzt der Chip kein HISR-Bit: eine Sendequittung
// (TX-DOK), die waehrend der Runde kam, loest nie einen Interrupt
// aus. Fuer den Empfang faengt das der Blick auf den Ring unten;
// fuer das Senden fehlte er. Bis 0.68.0 raeumte die 10-ms-Frist
// den Ring spaetestens dann ab — mit 0.69.0 schlief die Pumpe bis
// zu einer Sekunde, der Ring lief voll, und der Upload fiel auf
// 7 Mbit mit 503 Zeitueberschreitungen. Linux laesst dafuer die
// DOK-Interrupts waehrend der Empfangsverarbeitung an
// (`rtw_pci_enable_interrupt(.., exclude_rx = true)`); hier
// gilt: was vor diesem Aufruf quittiert wurde, raeumt er ab, was
// danach kommt, weckt uns.
```

## L7241-7242 · `if ls.authorized && !sendesperre {`

```
// Nur, wenn diese Runde die Schlange auch leert — sonst
// meldet sie sich sofort wieder und die Schleife dreht leer.
```

## L7246-7250 · `let jetzt = host::now_ms();`

```
// **Bis zum naechsten eigenen Zeitgeber, nicht pauschal.**
// Jede zeitabhaengige Pruefung dieser Schleife meldet hier
// ihren Termin; geparkt wird bis zum fruehesten. Die 10-ms-
// Frist aus 0.68.0 war der Platzhalter dafuer — sie machte
// im Leerlauf 100 der 118 Aufwachungen je Sekunde aus.
```

## L7276-7279 · `host::wait(mask, warte.max(1) as u32);`

```
// Mindestens 1 ms: `now_ms` zaehlt in 10-ms-Schritten, und
// ein Termin, der „jetzt" sagt, dessen Bedingung aber erst
// im naechsten Schritt greift, liesse die Schleife sonst bis
// zu 10 ms leer drehen.
```

## L7282 · `pci::disable_interrupt(h);`

```
// `rtw_pci_interrupt_handler`: HIMR aus, bis die Runde durch ist.
```

## L7299-7305 · `const RX_SPIN_BUDGET: u32 = 64;`

```
/// Wieviele leere Blicke auf den Ring, bevor wir uns schlafen legen.
///
/// Eine Schleifenrunde kostet ein halbes Dutzend Wirtsaufrufe, also
/// grob fuenf bis zehn Mikrosekunden. 64 leere Runden sind damit knapp
/// eine halbe Millisekunde Wachbleiben — unter Last kommt der naechste
/// Rahmen lange vorher, im Leerlauf ist es ein einmaliger Preis je
/// Beacon.
```

## L7308-7310 · `const RECONNECT_BACKOFF_MS: u32 = 3000;`

```
/// Wie lange gewartet wird, wenn ein Anlauf scheitert. Ein AP, der
/// gerade neu startet, braucht Sekunden; oefter zu fragen hilft nicht
/// und fuellt nur den Log.
```

## L7313-7316 · `const RESCAN_AFTER_TRIES: u32 = 2;`

```
/// Nach wievielen vergeblichen Anlaeufen die Umgebung neu abgesucht
/// wird. **Zwei, nicht einer**: ein AP, der gerade neu startet, ist nach
/// drei Sekunden wieder da, und ein Suchlauf dafuer waere teurer als
/// das Warten.
```

## L7319-7338 · `#[allow(clippy::too_many_arguments)]`

```
/// **Der Weg zurueck in eine stehende Verbindung.**
///
/// Er ist derselbe wie der Weg hin — Stufe 5e (Auth + Assoc) und 5f
/// (Ratenanpassung) —, mit vier Unterschieden, und jeder hat einen
/// Grund:
///
/// * **Kein `netdev_register`.** Der Kernel kennt die Schnittstelle
///   schon; ein zweites Anmelden gaebe eine zweite.
/// * **Die Schluessel raus, BEVOR neu verhandelt wird.** Ein alter
///   Paarschluessel im CAM entschluesselt die ersten Rahmen der neuen
///   Verbindung falsch, und das sieht aus wie ein kaputter Handschlag.
/// * **`rtw_mac_flush_queues`** — was noch in den Sendeschlangen liegt,
///   gehoert zur alten Verbindung und wuerde mit dem alten Schluessel
///   hinausgehen.
/// * **Die Paketnummer faengt wieder bei eins an** (802.11 §12.5.3.2:
///   sie gehoert zum SCHLUESSEL, und der ist gleich ein neuer).
///
/// Und ein neues `EV_READY`: `wifid` braucht einen frischen Supplicant
/// mit neuem SNonce. **Das ist der feine Unterschied zu 0.23.0** — dort
/// kam das zweite `EV_READY` ohne neue Verbindung, hier gehoert es dazu.
```

## L7346-7350 · `if link.bssid != bss.bssid {`

```
// **Die Zelle kann eine ANDERE sein.** Seit 0.55.1 sucht der Rufer
// nach zwei Fehlschlaegen neu, und dann traegt `bss` eine andere
// BSSID, einen anderen Kanal, vielleicht einen anderen Namen. Wer
// das hier nicht nachzieht, adressiert seine Datenrahmen weiter an
// den AP, den er gerade verloren hat.
```

## L7365-7366 · `link.ht_param_now = bss.ht_param;`

```
// **Die Breite der neuen Zelle** — ohne sie kommt ein
// Umhoerversuch auf der Breite der ALTEN zurueck.
```

## L7371-7379 · `link.roam.ave = dm::Ewma::new();`

```
// **Der geglaettete Pegel gehoert der ZELLE, nicht der
// Verbindung.** Er stand nach einem Wechsel weiter auf dem Wert
// des alten AP — wir zogen zu einem starken um und rechneten
// weiter mit -72 dBm, also sah der naechste Kandidat sofort
// wieder „besser" aus. Genau das hat nonstop gewechselt.
//
// `count` faellt mit: bis vier Baken der NEUEN Zelle da sind,
// sagt der Mittelwert nichts, und solange wird nicht gewechselt
// (`SIGNAL_AVE_MIN_COUNT`, mlme.c:96).
```

## L7395 · `host::netdev_set_link(false);`

```
// Der Kernel soll nichts mehr in die tote Leitung schieben.
```

## L7398-7400 · `for slot in 0..link.cam.len() {`

```
// Alte Schluessel aus dem CAM. `write_cam` hat sie hineingelegt,
// `clear_cam` nimmt sie heraus — Platz fuer Platz, wie sie belegt
// wurden.
```

## L7407-7416 · `link.ba_tx = BaTx::new();`

```
// **Die Block-Ack-Sitzungen gehoeren der ASSOZIATION, nicht dem
// Treiber.**
//
// Hier fehlte beides, und das hat den Durchsatz nach einem Wechsel
// halbiert: `ba_tx` stand noch auf `Laeuft` von der ALTEN Zelle,
// also trug jeder Datenrahmen weiter QoS und AGG_EN — an einen AP,
// mit dem wir nie eine Sitzung ausgehandelt hatten. Am Geraet:
// 180 Mbit vor dem Wechsel, 36 danach.
//
// Der Automat faengt jetzt von vorn an und fragt den NEUEN AP.
```

## L7418-7421 · `for t in 0..RO_TIDS {`

```
// Dasselbe in Empfangsrichtung. Der neue AP schickt zwar seinen
// eigenen ADDBA Request und `ro_open` setzt den TID dann zurueck —
// aber bis dahin wuerde der Puffer Rahmen der neuen Zelle gegen die
// Folgenummern der alten halten.
```

## L7437 · `if !stage5e_connect(h, hal, trx, mgmt_buf, h2c, e, t, e.addr, bss,`

```
// Stufe 5e: Auth und Assoc, auf demselben Kanal.
```

## L7444 · `let mut rates: Option<(sta::PeerCaps, sta::StaInfo)> = None;`

```
// Stufe 5f: die Firmware waehlt wieder die Rate.
```

## L7454 · `let mut ready = [0u8; 13];`

```
// Und `wifid` bekommt einen frischen Supplicant.
```

## L7470-7483 · `#[allow(clippy::too_many_arguments)]`

```
/// Sich umhoeren, ohne die Verbindung zu verlieren.
///
/// Der Ablauf ist der von mac80211 (`ieee80211_offchannel_stop_vifs`,
/// offchannel.c:83-131), auf das zusammengezogen, was wir haben:
///
/// 1. **dem AP sagen, dass wir kurz schlafen** — Null-Data mit gesetztem
///    Power-Management-Bit. Ab da PUFFERT er fuer uns.
/// 2. je bekanntem Kanal: hinwechseln, einen Probe Request mit UNSERER
///    SSID hinaus, kurz horchen.
/// 3. zurueck auf den eigenen Kanal, in der eigenen Breite.
/// 4. **aufwachen** — dasselbe ohne das Bit, und er schiebt nach.
///
/// Gefragt wird mit RUNDRUF-Adresse und gesetzter SSID: so antwortet
/// jede Zelle dieses Netzes, nicht nur eine.
```

## L7502 · `let mut nf = [0u8; 32];`

```
// (1) schlafen gehen
```

## L7510-7511 · `host::sleep_ms(2);`

```
// Dem Rahmen Zeit lassen, hinauszugehen — sonst wechseln wir den
// Kanal, bevor der AP erfahren hat, dass wir weg sind.
```

## L7515-7516 · `let (chs, n_ch) = unsafe {`

```
// SAFETY: einfaedig, und der Suchlauf schreibt nicht, waehrend
// gepumpt wird.
```

## L7521 · `let _ = switch_channel(h, hal, e, t_pwr, ch, CellWidth::default(), 0);`

```
// (2) hin, fragen, horchen
```

## L7556 · `let heim = CellWidth {`

```
// (3) zurueck — in der Breite, in der die Verbindung laeuft
```

## L7565 · `let n = build_nullfunc(&mut nf, &mac, &link.bssid, false);`

```
// (4) aufwachen
```

## L7574-7586 · `fn roam_better(jetzt_dbm: i8, jetzt_bw: usize, kand: &Bss,`

```
/// Ist der Kandidat besser als das, worauf wir sitzen?
///
/// **Die Regel ist eine Setzung, kein Port** — sie steht in
/// wpa_supplicant, und die Quelle liegt nicht im Cache. Zwei Wege
/// fuehren zum Wechsel:
///
/// * er ist deutlich STAERKER (`ROAM_BETTER_DB`), oder
/// * er ist BREITER und dabei hoechstens `ROAM_WIDER_TOLERANCE_DB`
///   schwaecher.
///
/// Der zweite Weg ist der Fall, der Florian getroffen hat: ein Repeater
/// bei -50 dBm mit HT40 gewinnt jede reine Pegelwahl gegen einen AP bei
/// -55 dBm mit VHT80 — und liefert die Haelfte.
```

## L7591-7602 · `let schmaler = jetzt_bw.saturating_sub(kand_bw) as i32;`

```
// **Breite wird nicht gegen Pegel verkauft.**
//
// Am Geraet: `K104 -58 dBm 80 MHz (wir)` gegen `K7 -49 dBm 20 MHz`,
// und die Regel nahm den zweiten — neun dB lauter, aber ein VIERTEL
// der Bandbreite. Die Regel schuetzte die Breite nur in die eine
// Richtung: der „breiter"-Zweig durfte Pegel kosten, der
// „staerker"-Zweig durfte Breite kosten, und niemand hat ihn daran
// gehindert.
//
// Jede Halbierung der Breite muss mit `ROAM_NARROWER_COST_DB`
// bezahlt werden. Von 80 auf 20 MHz sind das zwei Stufen — bei 10 dB
// je Stufe also zwanzig, und die neun dB reichen nicht mehr.
```

## L7611 · `#[allow(clippy::too_many_arguments)]`

```
/// Stufe 6a — Aufbau, Handschlag und die ersten acht Sekunden.
```

## L7622-7623 · `let _ = link_pump(h, hal, trx, mgmt_buf, &mut l, ls, mac, 8_000_000, d,`

```
// Acht Sekunden: der Handschlag braucht vier Rahmen und ist in
// Millisekunden durch; wer laenger wartet, wartet auf einen Fehler.
```

## L7651-7653 · `fn gate(name: &str, ok: bool) -> bool {`

```
/// Ein Tor. **Ein gefallenes geht immer hinaus** — sonst sagt ein
/// stiller Lauf nicht, wo er stehengeblieben ist, und das waere genau
/// der Zustand, aus dem die sechs Stufen herausfuehren sollten.
```

## L7667 · `fn d_c2h(ls: &LinkStats) -> impl Iterator<Item = (u8, u32)> + '_ {`

```
/// Die belegten Plaetze des C2H-Zensus.
```

## L7672-7679 · `fn report_connected(link: &Link, bss: Option<&Bss>, vif: Option<&vif::Vif>) {`

```
/// **Die eine Zeile, die auch ein stiller Lauf druckt.**
///
/// Sie steht nicht im `driver_report` — der landet in `wlan` und ist
/// Zustand, der sich je Sekunde erneuert. Hier steht das EREIGNIS: die
/// Verbindung ist zustande gekommen, mit wem, wie schnell und wie breit.
///
///     [rtl8822ce] verbunden: "HomeAP_New" K7 -49 dBm · HT MCS8-15
///                 (0x1b) · 40 MHz · AID 3
```

## L7716-7717 · `fn stage_line(ok: bool, green: &str, red: &str) {`

```
/// Eine Zeile der Schlusszusammenfassung. Gruen ist Stufenausgabe, rot
/// geht immer hinaus.
```

## L7726-7734 · `fn read_debug_flag() -> (bool, i32) {`

```
/// `debug:` aus `sys/config/wifi`. Fehlt die Datei oder die Zeile, ist
/// die Antwort NEIN: ein Treiber im Autostart schweigt, bis jemand
/// danach fragt.
///
/// **Gibt den Rueckgabewert des Lesezugriffs mit zurueck**, und das ist
/// kein Beiwerk: „kein `debug:` in der Datei" und „die Datei war nicht
/// da" fuehren zum selben Schweigen und haben verschiedene Heilungen.
/// Ein NEIN aus einem gescheiterten Lesezugriff ist keine Antwort auf
/// die gestellte Frage.
```

## L7748-7766 · `#[derive(Clone, Copy, PartialEq, Eq)]`

```
/// `ampdu:` aus `sys/config/wifi` — das Empfangsfenster der
/// Aggregation.
///
/// **Drei Faelle, und alle drei absichtlich:** die Zeile fehlt → die
/// Vorgabe (8, siehe `build_addba_resp`); `off`/`0` → gar keine
/// Aggregation, der Zustand bis 0.28.0; eine ZAHL → genau dieses
/// Fenster. So kann der naechste Lauf 8 gegen 32 messen, ohne dass
/// jemand neu uebersetzt — und eine Messung schlaegt eine Vermutung
/// darueber, wieviel Umsortierung TCP hier vertraegt.
/// `aspm:` aus `sys/config/wifi` — `an` · `aus` · `wie-gefunden`.
///
/// **Vorgabe ist AUS, und das ist eine Entscheidung mit zwei Seiten.** Der
/// Treiber schlaeft nie (kein LPS, §6 des Plans), also hat das Stromsparen
/// des Links bei uns keinen Gegenpart, der es wieder aufweckt — und die
/// Karte sagt selbst, dass sie 64 us braucht, um aus L1 herauszukommen.
/// Dafuer kostet es Leerlaufstrom, und genau daran haengt ein anderer
/// offener Posten (`project_idle_power_21w`). Deshalb ein Schalter und
/// kein stilles Verhalten: `aspm: an` faehrt die Gegenprobe.
/// Welches Band die Konfiguration will.
```

## L7769 · `Auto,`

```
/// Vorgabe: 5 GHz, sobald es brauchbar steht, sonst 2,4.
```

## L7771 · `Only24,`

```
/// Nur 2,4 GHz — der Rueckfall, wenn 5 GHz Aerger macht.
```

## L7773-7774 · `Only5,`

```
/// Nur 5 GHz — fuer die Messung, damit kein starker
/// 2,4-GHz-Nachbar die Entscheidung uebernimmt.
```

## L7778-7784 · `pub fn band_pref_from(v: &[u8]) -> BandPref {`

```
/// `band:` aus `sys/config/wifi`. Der reine Teil, damit `framecheck.py`
/// ihn ohne Geraet fahren kann.
///
/// **Ein unverstandener Wert ist `Auto`, nicht ein Band.** Anders als bei
/// `aspm` gibt es hier keine "sichere" Seite: wer sich vertippt, soll die
/// Vorgabe bekommen und nicht in einem Band festsitzen, in dem sein Netz
/// vielleicht gar nicht funkt.
```

## L7807-7809 · `const PREFER_5G_DBM: i8 = -70;`

```
/// Ab dieser Feldstaerke ist 5 GHz die bessere Wahl (Klassen-ABI
/// `WIFI_CLASS_ABI.md`: „5 GHz ab -70 dBm bevorzugt"). Darunter traegt
/// 2,4 GHz weiter, und ein schwaches 5-GHz-Signal waere ein Rueckschritt.
```

## L7824-7831 · `fn aspm_pref_from(v: &[u8]) -> Option<bool> {`

```
/// Der reine Teil von `read_aspm_pref` — getrennt, damit `framecheck.py`
/// ihn ohne Geraet und ohne Dateisystem fahren kann.
///
/// `Some(true)` anschalten · `Some(false)` ausschalten · `None` nicht
/// anfassen. **Ein unverstandener Wert heisst AUS, nicht „nicht
/// anfassen"** — wer etwas hinschreibt, will etwas aendern, und die
/// sichere Auslegung eines Tippfehlers ist die Vorgabe, nicht das
/// Gegenteil davon.
```

## L7842-7850 · `fn max_bw_for(e: &efuse::Efuse) -> usize {`

```
/// Die groesste Breite, die wir fahren duerfen — `RTW_CHANNEL_WIDTH_*`,
/// also 0/1/2.
///
/// **Zwei Quellen, und das Minimum davon.** Die Karte sagt in der efuse,
/// was sie kann (`hw_cap_bw`, ein Bitfeld: Bit 0 immer, Bit 1 fuer 40,
/// Bit 2 fuer 80 — der 8822CE meldet 0x07 und kann damit 80, aber kein
/// 160). `bw:` in `sys/config/wifi` ist die Hand am Regler: wer eine
/// Messreihe fahren oder eine breite Einstellung ausschliessen will,
/// schreibt `bw: 40` hin.
```

## L7853 · `for i in 0..3usize {`

```
// Das hoechste gesetzte Bit ist das Koennen der Karte.
```

## L7862-7870 · `pub fn bw_cap_from(v: &[u8]) -> usize {`

```
/// `bw:` aus `sys/config/wifi` — der reine Teil, damit `framecheck.py`
/// ihn ohne Geraet fahren kann.
///
/// **Ein unverstandener Wert ist die Vorgabe (80), nicht die schmalste
/// Einstellung.** Es ist dieselbe Regel wie bei `band:`: wer sich
/// vertippt, soll das bekommen, was ohne die Zeile herauskaeme, und nicht
/// in einer Einstellung festsitzen, die er nicht gewaehlt hat. Anders als
/// bei `aspm:` gibt es hier keine „sichere" Seite — schmal ist nicht
/// sicherer, nur langsamer.
```

## L7893-7898 · `pub fn txagg_from(v: &[u8]) -> bool {`

```
/// `txagg:` aus `sys/config/wifi` — der reine Teil fuer `framecheck.py`.
///
/// **Ein unverstandener Wert ist AN, also die Vorgabe.** Dieselbe Regel
/// wie bei `band:` und `bw:`: wer sich vertippt, bekommt das, was ohne
/// die Zeile herauskaeme. `off` ist der Notausgang, und er fuehrt in
/// einen Zustand, der GEMESSEN ist — den von 0.51.1.
```

## L7915-7917 · `fn read_ampdu_buf() -> u16 {`

```
/// `ampdu:` — die Fensterbreite, die wir dem AP fuer seine Aggregation
/// ZUSAGEN. Vorgabe ist die Decke des Protokolls, siehe
/// `sta::build_addba_resp`.
```

## L7942-7944 · `if any { num.clamp(1, sta::BA_TX_BUF_SIZE) } else { VORGABE }`

```
// Das Feld ist zehn Bit breit (`ADDBA_PARAM_BUF_SIZE_MASK`), und
// mehr als 64 kann HT/VHT ohnehin nicht — die Bitmaske des
// komprimierten Block Ack hat 64 Plaetze.
```

## L7948-7949 · `fn cfg_on(v: &[u8]) -> bool {`

```
/// `on` oder `1` — dieselbe Regel, die `wifi_ax200` fuer `ampdu:` und
/// `ps:` fuehrt. Ein unbekanntes Wort ist ein NEIN und keine Vermutung.
```

## L7954-7956 · `fn cfg_get(text: &[u8], key: &[u8]) -> Option<(usize, usize)> {`

```
/// `cfg_get` aus `wifid`/`wifi_ax200` — eine Zeile `key: value`, `#` ist
/// ein Kommentar. Gibt die GRENZEN des Wertes zurueck, nicht eine
/// Scheibe: der Puffer wird daneben weiterbenutzt.
```

## L7980 · `fn trim(t: &[u8], mut a: usize, mut b: usize) -> (usize, usize) {`

```
/// Leerzeichen und Wagenruecklauf an beiden Enden weg.
```

## L7991-7994 · `const REPORT_CAP: usize = 3584;`

```
/// Wieviel unser Bericht fassen darf. Der Kernel nimmt bis
/// `drivers::report::REPORT_MAX` = 4096 (`host_core.rs:3639`); die
/// 3584 laesst dem Kernel einen Rand; 2048 war mit der Watchdog-Zeile
/// aus 0.67.1 knapp am Ende — und `put` schneidet still ab.
```

## L7997-8000 · `fn publish_report(link: &Link, ls: &LinkStats, d: &Dev,`

```
/// docs/spec/WIFI_CLASS_ABI.md §3 — `npk_driver_report`.
///
/// Ein Klartextblock, den das Intent `wlan` neben die Kernelsicht druckt.
/// Der Kernel parst nichts; was berichtenswert ist, ist Geraetewissen.
```

## L8003-8011 · `let mut b = [0u8; REPORT_CAP];`

```
// **Der Kernel nimmt 4096** (`drivers::report::REPORT_MAX`); hier
// standen 896, und `put` schneidet STILL ab — `s.len().min(b.len() -
// *n)`. Mit jeder Zeile, die dazukam, fiel eine hinten heraus, und
// zwar ohne ein Zeichen darueber. Am Geraet endete der Bericht
// mitten in `abstand 2509 us im mittel, ` — genau vor den zwei
// Zahlen, fuer die die Version gebaut war.
//
// Zweitausend statt 896, und wenn es doch einmal nicht reicht, sagt
// es der Bericht am Ende selbst.
```

## L8042-8049 · `let hex = b"0123456789abcdef";`

```
// **Drei Raten, und nur eine davon war bisher zu sehen.**
//
// `angeboten` ist `link.highest_rate` — einmal bei der Anmeldung aus
// den Faehigkeiten des AP gerechnet. Das ist eine BEHAUPTUNG ueber
// das Moegliche, und sie stand hier bis 0.31.0 allein als „rate".
//
// `tx` ist, was die FIRMWARE gewaehlt hat (C2H `RA_RPT`), `rx` was
// im Empfangsdeskriptor JEDES Rahmens steht. Das sind die Messungen.
```

## L8058-8074 · `let mcs0 = DESC_RATEMCS0 as usize;`

```
// **Die haeufigste Empfangsrate der ganzen Verbindung**, nicht die
// des letzten Rahmens. Bei einem Download sind 65 000 Datenrahmen
// gegen 900 Beacons kein Zweifelsfall.
// **Die haeufigste Rate der DATEN, nicht die der Baken.**
//
// Hier stand die Spitze ueber alle Raten, und am Geraet kam heraus:
// `rx OFDM 6M in 8350 von 15628` — bei `beacon 8320` in derselben
// Zeile. Eine Bake geht immer mit der niedrigsten Rate hinaus, und
// je laenger eine Verbindung steht, desto sicherer gewinnt sie: nach
// vierzehn Minuten sind es 8320 Baken gegen die paar tausend
// Datenrahmen, die einen PHY-Status tragen. Die Zeile sagte dann
// „6 Mbit" ueber eine Strecke, die gerade 240 Mbit lieferte.
//
// Gezaehlt wird jetzt ab `DESC_RATEMCS0` — alles darunter ist
// Legacy und auf einer HT/VHT-Verbindung Verwaltung. Gibt es keine
// einzige HT/VHT-Rate, faellt es auf die Spitze ueber alles zurueck,
// denn dann IST die Verbindung legacy.
```

## L8082-8085 · `let (top_rate, top_cnt, nur_legacy) = match spitze(mcs0) {`

```
// **Faellt es auf die Spitze ueber ALLES zurueck, gehoert auch der
// Nenner ueber alles.** Am Geraet stand sonst `0x04 in 304 von 0
// ht/vht` — ein Zaehler ohne Nenner, weil der Nenner die
// HT/VHT-Rahmen zaehlte und es keine gab.
```

## L8106-8112 · `put(" ht/vht (HT ", &mut b, &mut n);`

```
// **HT und VHT sind zwei Klassen, und der Unterschied ist der
// Faktor auf der Strecke.** Die Spitzenrate darueber nennt nur
// EINE; steht dort eine HT-Rate, waehrend wir VHT80 angemeldet
// haben, sendet der AP eine Klasse unter dem, was ausgehandelt
// ist — und das sieht man an keiner anderen Zahl. Gezaehlt wird
// nichts Neues: `rate_hist` traegt den Schnitt seit je, er wurde
// nur nie gelesen.
```

## L8134-8137 · `put(" (empfangen ", &mut b, &mut n);`

```
// **Und daneben, was WIRKLICH ankam.** Die Zahl davor ist unsere
// Einstellung; diese hier kommt aus dem Empfangsstatus des Chips.
// Stehen sie auseinander, faehrt der AP eine andere Breite als wir —
// und das sieht man an keiner anderen Stelle.
```

## L8151-8159 · `let vhtmap = |m: u16, b: &mut [u8; REPORT_CAP], n: &mut usize| {`

```
// **Was der AP SELBST angibt zu koennen.**
//
// Die Zeile darueber sagt, WOMIT er sendet. Diese sagt, WOMIT ER
// KOENNTE — aus seiner eigenen Anmeldeantwort, die `parse_assoc_resp`
// seit je liest und die nirgends stand. Stehen die zwei auseinander,
// ist die Rate seine ENTSCHEIDUNG und nicht seine Grenze, und dann
// liegt der Deckel in seiner Ratenwahl statt bei uns. Fehlt VHT hier
// ganz, hat er uns gar nicht als VHT-Station angenommen — und DAS
// waere unseres.
```

## L8179-8183 · `put("\n  wir schickten  HT ", &mut b, &mut n);`

```
// **Unsere Seite zuerst.** Ohne sie unterscheidet die Zeile nicht
// zwischen „er hat nein gesagt" und „wir haben nie gefragt":
// `build_vht_cap_ie` kehrt UM, wenn die efuse etwas anderes als VHT
// ansagt, und dann geht gar kein VHT-Element hinaus. Dieselbe
// Bedingung, an derselben Zahl.
```

## L8187-8188 · `match unsafe { SENT_VHT } {`

```
// SAFETY: einfaedig; geschrieben beim Bauen des Antrags, hier nur
// gelesen.
```

## L8206 · `put(if unsafe { SENT_WMM } { " + WMM" } else { " + KEIN WMM (AP sagt keins an)" },`

```
// SAFETY: wie oben.
```

## L8209-8215 · `put("\n  ap betreibt  ", &mut b, &mut n);`

```
// **Und wie breit er seine Zelle BETREIBT** — aus der
// Anmeldeantwort, nicht aus der Bake. Linux liest genau diese
// Elemente und nur diese: `ieee80211_assoc_success` gibt die
// Elemente der ANTWORT an `ieee80211_config_bw` (mlme.c:7666), und
// `ieee80211_determine_ap_chan` leitet daraus Betriebsart und
// Breite ab. Wir nahmen beides aus der Bake und sahen die Antwort
// nie an — wenn die zwei auseinanderstehen, steht es hier.
```

## L8262-8264 · `if let Some(l) = pci::link_state() {`

```
// Die PCIe-Strecke. Steht hier und nicht einmalig beim Start, weil
// ASPM ein Verdaechtiger fuer den Durchsatz ist und ein Verdaechtiger
// in DEN Bericht gehoert, den Florian einschickt.
```

## L8328-8330 · `if link.roam.scans > 0 || link.roam.roams > 0 {`

```
// **Die Verbindungswache.** Ein Anstupser, dem eine Erholung folgt,
// ist ein Fall, in dem wir die Verbindung FRUEHER weggeworfen
// haetten — die zwei Zahlen nebeneinander sagen, wie oft.
```

## L8372 · `let zehntel = if ls.rx_polls > 0 {`

```
// Rahmen je Blick mit einer Nachkommastelle, in ganzen Zahlen.
```

## L8384 · `let lauf = host::now_us().wrapping_sub(ls.pump_us0).max(1);`

```
// Auslastung in Prozent: Zeit im Ring gegen Zeit der Schleife.
```

## L8391-8393 · `put("\nsendequittung ", &mut b, &mut n);`

```
// **Die Zeile, die sagt, ob der AP uns HOERT.** Bis 0.26.0 stand
// hier nichts dergleichen: „raus 360" hiess nur, dass wir 360 Rahmen
// in einen Ring gelegt haben.
```

## L8417-8419 · `put("\nneuschluessel ", &mut b, &mut n);`

```
// **Die Zeile, die diese Runde beantwortet.** Ein Neuschluessel
// laeuft Minuten nach dem Handschlag und hinterlaesst sonst keine
// Spur; ein Rauswurf war bis hierher gar nicht sichtbar.
```

## L8430-8433 · `put("\nmgmt beacon ", &mut b, &mut n);`

```
// **Die Frage dieser Runde, in einer Zahl.** Versucht der AP
// ueberhaupt, eine Aggregation aufzubauen? Er tut das mit einem
// ADDBA Request, und wir verwerfen bis heute jeden
// Verwaltungsrahmen ausser Deauth und Disassoc.
```

## L8456-8467 · `if ls.rx_ppdu_n > 0 {`

```
// **Und die andere Richtung, die seit 0.29.0 fehlte.** Sie steht
// daneben und nicht darunter, damit man in EINER Zeile sieht, dass
// eine Verbindung zwei Sitzungen hat und dass sie verschiedene
// Zustaende haben koennen.
// **Und wieviel je Anstoss im Ring lag.** Eine Block-Ack-Sitzung
// sagt, was die Hardware aggregieren DARF; diese Zahl sagt, was sie
// aggregieren KANN. Steht hier eine Eins, ist der Engpass nicht die
// Sitzung, sondern dass nie mehr als ein Rahmen gleichzeitig da ist.
// **Die Aggregatgroesse in EMPFANGSrichtung**, von der Hardware
// gezaehlt. Sie steht vor der Sendeseite, weil sie beim
// Herunterladen die groessere ist — und weil die zwei nebeneinander
// sagen, ob eine Richtung buendelt und die andere nicht.
```

## L8476-8479 · `if ls.rx_gap_n > 0 {`

```
// **Und wieviel Zeit dazwischen lag.** Der kleinste Abstand ist
// das, was die Strecke kann; der mittlere das, was sie tut. Die
// zwei nebeneinander sagen, ob die Luft der Deckel ist — ohne
// eine einzige geschaetzte Konstante.
```

## L8488 · `put("\n  verteilt  <0,5ms ", &mut b, &mut n);`

```
// **Die Verteilung, und sie ist der eigentliche Befund.**
```

## L8511-8514 · `if ls.turn_n > 0 {`

```
// **Die Umkehrzeit unseres eigenen Stapels.** Von „Daten an den
// Kernel" bis „Rahmen vom Kernel zurueck" — beim Herunterladen ist
// das die Zeit, die WIR zur TCP-Quittung brauchen, und sie steckt
// eins zu eins in der RTT, die der Server misst.
```

## L8523-8527 · `put("\n  verteilt  <0,2ms ", &mut b, &mut n);`

```
// **Und hier faellt die Entscheidung.** Stehen in den zwei
// rechten Eimern ungefaehr so viele Faelle wie oben bei
// `>=10ms`, dann wartet der AP auf UNS — die Pause im Funk und
// die Pause im Stapel sind dann dasselbe Ereignis. Stehen dort
// null, kommt der Stillstand von woanders.
```

## L8545-8550 · `put("\nsendering ", &mut b, &mut n);`

```
// **Zwei Zahlen, und nur die zweite entscheidet.** `eingelegt`
// ist, was der Treiber in EINEM Durchlauf in den Ring schob;
// `im ring` ist, was die Hardware im selben Augenblick noch vor
// sich hatte. Aggregiert wird die zweite. Sie sind verschieden,
// sobald das Medium belegt ist — dann stapeln sich die
// Deskriptoren, waehrend der Treiber einzeln nachlegt.
```

## L8610-8615 · `put("\nwatchdog ", &mut b, &mut n);`

```
// **Die Zeile, die beweist, dass der Watchdog laeuft.** Vier Zahlen,
// die sich bewegen muessen: der Takt, die Verstaerkungsregelung, der
// Quarz (gegen den Wert der efuse) und die Temperatur. Steht der
// Quarz auf dem efuse-Wert und `bt` auf „an", ist die Nachfuehrung
// durch den Koexistenz-Riegel abgestellt — das ist kein Fehler,
// sondern Linux' eigene Regel, und man sieht es hier.
```

## L8618 · `let (wd, it, it_wd) = unsafe {`

```
// SAFETY: einfaedig; der Pumpfaden schreibt, hier nur gelesen.
```

## L8641-8643 · `put("  crc ht ", &mut b, &mut n);`

```
// **Der Zustand der Luft in einer Zahl.** Ein hoher Anteil heisst:
// der AP muss staendig wiederholen, und dann ist der Deckel die
// Strecke und nicht der Treiber.
```

## L8661-8664 · `put("  snr ", &mut b, &mut n);`

```
// **Warum die Gegenseite waehlt, was sie waehlt.** Der
// Stoerabstand je Pfad steht in jedem Empfangsdeskriptor
// (`query_phy_status_page1`) und sagt, ob eine niedrige Rate
// berechtigt ist oder ob jemand unter Wert faehrt.
```

## L8669-8673 · `put("  quarz ", &mut b, &mut n);`

```
// **Zwei Zahlen, die sich gegenseitig aufloesen.** Allein sagt der
// Quarzwert nichts: erst der Abstand zur efuse sagt, ob die
// Nachfuehrung ueberhaupt etwas tut. Steht er auf dem efuse-Wert und
// `bt` auf „aus", dann hat sie gelaufen und nichts zu korrigieren
// gefunden — steht er darauf und `bt` auf „AN", ist sie abgestellt.
```

## L8688-8692 · `put("  tp ", &mut b, &mut n);`

```
// **Der Spitzenwert daneben.** Der geglaettete Wert faellt nach dem
// Ende einer Uebertragung binnen Sekunden auf null — wer danach
// `wlan` tippt, sieht `0/0` und kann ihn mit nichts vergleichen. Der
// Hoechststand bleibt stehen und ist die Zahl, die neben der von
// `netbench` steht.
```

## L8702-8704 · `if n == b.len() {`

```
// **Ein abgeschnittener Bericht muss es sagen.** Sonst liest man
// eine Zeile zu Ende, die keine ist — und das war genau der Fall,
// der diese Zeilen ausgeloest hat.
```

