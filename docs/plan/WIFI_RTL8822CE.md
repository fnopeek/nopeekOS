# RTL8822CE — der Plan, bevor die erste Zeile steht

**Chip:** `10ec:c822` · Realtek RTL8822CE · Wi-Fi 5, 2T2R, PCIe · im Lenovo
IdeaPad Flex 5 14ALC7 das **einzige** eingebaute Netzgerät (`02:00.0`).
**Linux:** 6.18.26, `drivers/net/wireless/realtek/rtw88/`, Modul `rtw_8822ce`.
**Firmware:** `rtw88/rtw8822c_fw.bin`, 202 600 Bytes, **Version 9.9.15**.
**Karte:** [WIFI_RTL8822CE_LINUX_MAP.md](WIFI_RTL8822CE_LINUX_MAP.md) — was der
Linux-Treiber hat, Datei für Datei, ausgezählt.

**Stand 2026-09-20:** Kernel **0.383.0** · Modul **wifi_rtl8822ce 0.10.3**.
**Stufen 0 bis 3c am Gerät GRÜN.** Der Weg PCI → Bridge → Power → Ringe →
DMA → Firmware → C2H → efuse → MAC-Init → Parametertabellen → BB/RF ist Ende
zu Ende bewiesen: `rsvd_boundary 1938`, der H2C-Ring meldet sich leer, die
Link-List-Tabelle baut sich selbst, alle sechs Tabellen geben **Zahl für
Zahl** so viele Schreibzugriffe ab wie vorausgerechnet (bb 1289 · agc 450 ·
rfk_init 2460 · rf_b 697 · rf_a 789), und beide RF-Pfade antworten mit
Tabellenwerten. **Als Nächstes: Stufe 4.**

**Geprüft ist, was sich ohne Gerät prüfen LÄSST**, und zwar mechanisch:
`check_regs.py` hält **462 Konstanten** gegen die Linux-Quelle (0 Abweichungen),
`seqdiff.py` vergleicht **22 Funktionen Zugriff für Zugriff und Zahl für Zahl**
mit dem C-Original, und `gen_tables.py` rechnet vorher aus, wieviele
Schreibzugriffe jede Tabelle auf DIESEM Chip abgeben muss — der Treiber hält
seine eigene Zahl dagegen. Was davon der Gerätelauf noch beantworten muss,
steht unten unter „HIER WEITERMACHEN".

**Der teuerste Fund lag nicht im Treiber.** Jede DMA-Anfrage des Chips endete
mit *Received Master Abort*: `npk_pci_enable_bus_master` setzte das Bit nur am
GERÄT, und eine PCI-Bridge leitet von ihrem Sekundärbus nur nach oben weiter,
wenn **sie** Bus-Master ist. NVMe und xHCI fielen nicht darauf herein, weil die
Firmware sie selbst benutzt und mit eingeschalteten Bridges übergibt — eine
WLAN-Karte fasst UEFI nie an. Drei Hypothesen davor sind durch Messung
gestorben, siehe L1–L8.

**Zwei Portierfehler, die dieselbe Form hatten:** `rtw_hci_setup` ist
`rtw_pci_setup` und damit ZWEI Aufrufe (`reset_trx_ring` **und**
`rtw_pci_dma_reset`), und `check_hw_ready` hält in Linux eine **Frist** von
10 ms (1000 × `udelay(10)`), nicht 1000 Runden — ohne die Pause waren das hier
1–2 ms, und die Firmware bekam ein Zehntel der Zeit für ihr `FW_INIT_RDY`.
Beide Male habe ich die Zahl übernommen und den Mechanismus dahinter nicht.

**Werkzeuge im Modulverzeichnis:** `gen_pwrseq.py` und `gen_tables.py`
erzeugen die Power-Sequenz- und die Parametertabellen aus der C-Quelle;
`check_regs.py` hält **jede** Konstante gegen ihren `#define`/Enum-Eintrag in
der Linux-Quelle (462 geprüft, 0 Abweichungen), `seqdiff.py` hält jede
portierte Funktion Zugriff für Zugriff und Zahl für Zahl gegen das C-Original
(22 Funktionen). Die Regel „vor jedem Commit grep gegen reg.h" tun damit zwei
Skripte statt eines Vorsatzes — und beide sind gegen einen absichtlich
eingebauten Fehler geprüft.

---

## 0 — Warum dieses Papier vor dem Code steht

Ausgezählt, nicht behauptet:

    tools/wasm/wifi_ax200/   0.108.0    148 Versionen, 149 Commits
    tools/wasm/wifi/         1.53.1     200 Commits   (RTL8852BE)

Der AX200 hat 148 Versionen gebraucht, der RTL8852BE 200 — und in beiden
Fällen ging der teure Teil nicht für Funk drauf, sondern dafür, dass
stückweise portiert wurde. Die Lehre steht als Regel im Repo
(`memory/feedback_port_completely_debug_never.md`,
`memory/feedback_linux_strict.md`): **eine Abweichung von Linux ist ein Bug,
bis das Gegenteil bewiesen ist**, und Debuggen auf unvollständiger Portierung
sagt nichts aus.

Dieses Papier ist die Antwort darauf. Es nennt die **ganze** Aufrufkette in
Linux-Reihenfolge, die Löcher, die es jetzt schon gibt, und für jede Stufe ein
**Gate** — eine Messung, die entweder stimmt oder nicht.

**Der Vorteil diesmal:** rtw88 ist mechanischer als iwlwifi. Die Power-Sequenz
ist eine Tabelle mit 54 Kommandos, die ein 40-zeiliger Interpreter abarbeitet.
Der PHY-Aufbau ist ein Tabellenlader. IQK macht die Firmware. Es gibt wenig,
worüber man raten KÖNNTE.

---

## 1 — Was schon gelöst ist

Das ist der Grund, warum das hier nicht bei null anfängt.

### 1a. Die ganze Verdrahtung um den Treiber herum

| Stück | Wo | Zustand |
|---|---|---|
| **Treiber-ABI** | `npk_pci_*`, `npk_mmio_*`, `npk_dma_*` | steht, von zwei Treibern belegt |
| **PCI binden + BAR abbilden** | `npk_pci_bind`, `npk_mmio_map_bar` | BAR 0–5, 64-bit-BARs, unassigned-BAR-Zuteilung |
| **Automatische Gerätewahl** | `kernel/src/intent/wasm.rs:343` | `driver wifi_*` → Klasse 02:80. **`pci` am Gerät meldet für `c822` genau „Network controller" = 02:80 — passt ohne eine Zeile Kernel.** |
| **Zweitinstanz-Sperre** | `intent_run_driver` | ein zweiter `driver …` wird abgelehnt, statt die Karte unter dem Laufenden zu resetten |
| **Datenpfad** | `npk_netdev_register/submit_rx/poll_tx/set_link` | Ethernet-Frames ↔ IP-Stack, seit Kernel 0.205.0 |
| **Steuerpfad** | `npk_wifi_poll_cmd` / `npk_wifi_send_event` | die WiFi-Klassen-ABI, `docs/spec/WIFI_CLASS_ABI.md` |
| **Supplicant** | `tools/wasm/wifid/` | SHA-1, HMAC, PBKDF2, PRF, AES, EAPOL-4-Wege — **herstellerunabhängig, einmal für alle Treiber** |
| **UI** | `bar` WLAN-Applet | Scanliste, Verbinden, Link-Zustand |
| **Selbstauskunft** | `npk_driver_report` + Intent `wlan` | ein Klartextblock pro Sekunde |
| **Modul-Release** | `./build.sh sign-modules` | nur geänderte Module signieren, kein 5-MB-Kernel-Download |

**Praktisch heißt das:** sobald der Treiber `LINK_UP` meldet und Frames
schiebt, funktionieren Scanliste, Passworteingabe, Auto-Reconnect und IP
**ohne eine einzige neue Zeile außerhalb des Treibers.**

### 1b. Der Zuschnitt ist derselbe wie bei den zwei bestehenden Treibern

rtw88 ist **SoftMAC über mac80211** — wie iwlwifi-mvm, wie rtw89. Kein
FullMAC-Sonderfall. `wifid` ist also der richtige Weg, und die obere Hälfte
(Scan → Auth → Assoc → 4-Wege-Handschlag) ist in `wifi_ax200` schon einmal
gebaut worden: `run_scan`, `parse_beacon`, `connect_send_auth`,
`connect_send_assoc`, `wait_mgmt_response`, `rx_classify`, dazu `ba.rs` für
Block-Ack. **Der Code ist nicht übertragbar, das Muster schon** — und das ist
der Unterschied zwischen „zwei Wochen" und „zwei Tagen" für diese Schicht.

### 1c. Die Tabellen sind ein gelöstes Problem

46 105 Zeilen Parametertabellen (`rtw8822c_table.c`) schreibt niemand ab.
`tools/wasm/wifi/gen_tables.py`, `gen_rfk.py`, `gen_iqk.py`, `gen_tssi.py`,
`gen_pmac.py` erzeugen genau so etwas für den 8852BE aus der Linux-Quelle. Ein
`gen_*.py` für rtw88 ist ein Nachmittag, und danach ist jede Tabelle
**beweisbar** dieselbe wie in Linux, nicht abgetippt.

### 1d. Die Firmware liegt vor und ist gegen Linux geprüft

    /lib/firmware/rtw88/rtw8822c_fw.bin.zst   →  202 600 Bytes
    signature 0x8822 · Version 9.9.15 · h2c_fmt_ver 15
    dmem @0x80200000  19 328      imem @0x80030000  54 064
    emem @0x80100000 129 120      mem_usage 0x18

Nachgerechnet mit `check_firmware_size` aus `mac.c`:
`64 + (19328+8) + (54064+8) + (129120+8) = 202600`. **Stimmt auf das Byte.**

Und die Fähigkeitsbits aus dem Kopf (`feature = 0x000001e7`) sagen, was diese
Firmware kann: `SIG · LPS_C2H · LCLK · BCN_FILTER · NOTIFY_SCAN ·
ADAPTIVITY · SCAN_OFFLOAD`. Dass `9.9.15 ≥ 9.9.13` ist, heißt außerdem: der
`FW_FEATURE_EXT_OLD_PAGE_NUM`-Sonderfall entfällt.

**Mitliefern:** der AX200-Blob liegt in `tools/wasm/wifi_ax200/firmware/`, es
gibt also einen Präzedenzfall. rtw88-Firmware steht unter derselben Lizenz wie
die, die wir schon ausliefern.

**Was das Modul am Ende wiegt** — gemessen, nicht geschätzt. Bei einem
WLAN-Treiber ist fast alles Blob:

| | Code | Blobs | gesamt |
|---|---|---|---|
| `wifi_ax200` (fertig) | 78,5 KiB | 1336,0 KiB Firmware | **1,38 MiB** |
| `wifi` / 8852BE (fertig) | ~78 KiB | 1216 KiB FW + 149 KiB Tabellen | **1,41 MiB** |
| `wifi_rtl8822ce` heute (Stufe 0) | 6,2 KiB | — | **6,2 KiB** |
| `wifi_rtl8822ce` Prognose | ~80 KiB | 198 KiB FW + ~450 KiB Tabellen | **~0,7 MiB** |

Halb so groß wie der Intel-Treiber, weil Realteks Firmware **sechsmal
kleiner** ist (198 KiB gegen 1336 KiB). Dafür wiegen hier die Tabellen mehr
als die Firmware: `rtw8822c_rf_a` und `_rf_b` allein sind 40 070 + 40 706
Werte = 315 KiB. Sie kommen in Stufe 3.

---

## 2 — Die Löcher, die JETZT schon benannt sind

Gefunden beim Lesen der Quelle, nicht beim Debuggen. Jedes hat einen Platz im
Stufenplan.

### L1 — ~~Es gibt kein `npk_mmio_read8`/`npk_mmio_write8`.~~ ✅ **Kernel 0.376.0**

Der Kernel bietet 16, 32 und 64 Bit. rtw88 greift **247-mal** 8-bit zu, davon
**88-mal in `mac.c`** — und der Power-Sequenz-Interpreter besteht aus nichts
anderem:

```c
value = rtw_read8(rtwdev, offset);
value &= ~cur_cmd->mask;
value |= (cur_cmd->value & cur_cmd->mask);
rtw_write8(rtwdev, offset, value);
```

Ein 8-Bit-Schreiber lässt sich **nicht** durch 32-Bit-RMW ersetzen: der
32-Bit-Lesezugriff berührt drei Nachbarbytes, und bei Registern mit
Leseeffekt ist das ein anderer Vorgang. Linux wählt die Breite absichtlich
(`feedback_linux_strict`, Punkt 4). Zwei neue Host-Fns, **anhängend**
([[feedback_abi_append_only]]), in **beiden** ABI-Wegen (`wasm.rs` und
`forge_glue.rs` — das war beim TLS-Posten schon die Falle).

Keiner der zwei bestehenden Treiber hat das je gebraucht: iwlwifi und rtw89
sind 32-Bit-Chips. Das Loch ist neu, weil der Chip neu ist.

**Gebaut in 0.376.0**, `host_core.rs` + `wasm.rs` + `forge_glue.rs`; das
Forge-Tor (`tools/forge-gate.py`) prüft die zweite Tabelle mit. Dabei
mitgenommen: **alle zehn MMIO-/DMA-Zugriffsfunktionen lehnten einen negativen
`offset` nicht ab** — `offset as usize` wird riesig, `off + 4` läuft über und
die Grenzprüfung geht durch. Ein gebundener Treiber konnte damit unter seine
eigene BAR lesen und schreiben. Jetzt trägt jede der zehn ein `offset < 0`.

**Offen, bewusst nicht angefasst:** `wifi_ax200/src/host.rs` baut `mmio_w8`
per 32-Bit-RMW nach, mit dem Kommentar „No `npk_mmio_write8` host fn exists".
Der Satz stimmt seit 0.376.0 nicht mehr — aber der AX200-Faden ist pausiert
und seine Verbindung läuft, also ist das eine Änderung mit eigenem Gerätelauf
und nicht nebenbei ([[feedback_a_comment_that_names_its_condition_expires]]).

### L2 — **BAR2, nicht BAR0.** *(Stufe 0)*

`rtw_pci_io_mapping` setzt `u8 bar_id = 2`. Der AX200 liegt auf BAR0. Wer aus
Gewohnheit BAR0 abbildet, liest Müll und sucht den Fehler im Chip.

### L3 — **Das DMA-Budget, und ein Ring, den die Hardware nie sieht.** *(Stufe 2)*

Linux legt `RTK_MAX_RX_QUEUE_NUM = 2` Empfangsringe an — MPDU **und C2H** — zu
je `RTK_MAX_RX_DESC_NUM = 512` Puffern à `RTK_PCI_RX_BUF_SIZE = 11454 + 24 =
11478` Bytes. Das sind **11,76 MB**. Unser Deckel ist
`MAX_DMA_PAGES = 2048` (8 MB) bei `MAX_DMA_ALLOCS = 1024`. **So passt es nicht.**

Nachgelesen statt gekürzt: **auf PCIe wird der C2H-Ring nie in die Hardware
geschrieben.** `rtw_pci_reset_buf_desc` programmiert `RXBD_DESA_MPDUQ` und
`RXBD_NUM_MPDUQ` — und sonst nichts; für C2H gibt es gar kein
Adressregister in `pci.h`. Die Firmware-Antworten kommen durch den
MPDU-Ring, `rtw_pci_rx_napi` trennt sie an `pkt_stat.is_c2h` und reicht sie
an `rtw_fw_c2h_cmd_rx_irqsafe`. Der zweite Ring ist auf diesem Bus tote Last.

**Also: nur den MPDU-Ring anlegen — 512 × 11478 = 5,88 MB, 1536 Seiten, 512
Allokationen.** Das ist eine Abweichung von Linux' *Allokation*, aber keine
von dem, was die Hardware sieht; sie steht deshalb hier, mit dem Beweis
daneben. Bleiben 2,1 MB und 512 Allokationen für acht TX-Ringe und den
Firmware-Weg — **das wird an Stufe 2 gemessen, nicht geschätzt.**

Wird es doch eng, dann kürzt man die **Ringtiefe** (eine Zahl, die Linux
selbst als Konstante wählt) und **nie** die Puffergröße: 11454 ist die
maximale A-MSDU, und ein zu kleiner Puffer zerschneidet Frames, die der Chip
als ganze abliefert.

### L4 — **Es gibt keine Geräte-IRQs.** *(gilt für jede Stufe)*

Bekannt und ausdrücklich getragen ([[project_host_device_irq]]). rtw88 macht
es einfach: der Hardware-Schreibzeiger steht in einem MMIO-Register
(`RTK_PCI_RXBD_IDX_MPDUQ = 0x3B4`), es gibt also einen sauberen Pollpfad ohne
eine einzige geratene Zeile. `rtw_pci_interrupt_handler` und `napi_poll`
werden zu einer Schleife, die diesen Zeiger liest — und **das ist eine
begründete Abweichung, die hier festgehalten ist**, keine Auslassung.

### L5 — **88 Funktionen RF-Kalibrierung.** *(eigene Stufe, eigenes Gate)*

DPK 48, DAC-Cal ~25, TXGAPK ~15 — alles hostseitig. Genau die Fläche, an der
der 8852BE-Port 100 Stunden verloren hat. Zwei Entlastungen, beide gemessen:
**IQK macht die Firmware** (`rtw8822c_do_iqk` = ein H2C plus Pollen auf
`REG_RPT_CIP` — beim 8852BE waren das 851 Zeilen), und sowohl DPK als auch
TXGAPK haben in Linux **eigene Abschalt-Wächter** (`dpk_info->is_dpk_pwr_on`,
`dm_flags & BIT(RTW_DM_CAP_TXGAPK)`). Damit ist eine Stufe ohne sie **ein
Linux-Pfad**, kein Weglassen — und der Übergang ist ein Schalter, kein Umbau.

### L6 — **Koexistenz, 111 Funktionen.** *(Stufe 6)*

Der 8822C ist ein Combo-Die (WiFi + BT). `rtw_coex_power_on_setting` und
`rtw_coex_init_hw_config` stehen in `rtw_power_on` — also schon im ersten
Hochfahrweg. `wifi_only = !rtwdev->efuse.btcoex` entscheidet, wie viel davon
läuft; **was `btcoex` auf DIESEM Gerät ist, sagt erst die efuse.** Das ist
eine Messung an Stufe 1, keine Annahme.

### L7 — **Die obere Hälfte ist nicht in den 940 enthalten.**

29 der 38 `rtw_ops` gelten für uns, dazu ~25 `ieee80211_*`-Hilfsfunktionen und
eine STA-MLME. Details in der Karte, §2.

---

## 3 — Die Stufen

Jede Stufe: **vollständig portierte Funktionen** aus der Aufrufkette
(Karte §3), ein **Gate**, und erst danach die nächste. Florian fährt die Läufe
([[feedback_florian_runs_the_test]]).

### Stufe 0 — Die Tür: binden, abbilden, den Chip erkennen ✅ **gebaut, ungeprüft**
- **Kernel 0.376.0:** `npk_mmio_read8` / `npk_mmio_write8` in beiden ABI-Wegen (L1).
- `npk_pci_bind(0x10ec, 0xc822)` mit Rückfall auf `0xc82f` (rtw8822ce.c führt
  beide IDs), Bus-Master, `npk_mmio_map_bar(2, 16)` (L2).
- `rtw_chip_parameter_setup`: `REG_SYS_CFG1` lesen → `cut_version`,
  `vendor_id`, `mp_chip`, `BIT_RF_TYPE_ID` → 2T2R erwartet.
- Dazu gelesen, weil Linux sie selbst als Klartext vergleicht:
  `REG_CR` **als 8 Bit** (`== 0xea` heißt MAC aus) und `REG_MCUFW_CTRL` als
  16 Bit (`== 0xC078` heißt Firmware läuft noch).
- **Stufe 0 schreibt kein einziges Register.** Was hier schiefgeht, kann
  damit nicht an einem Schreibzugriff von uns liegen.
- **Gates:** (1) Registerfenster antwortet — nicht `0x00000000`/`0xFFFFFFFF`;
  (2) `rf_type = RF_2T2R`; (3) **der frische 8-Bit-Pfad liefert byteweise
  dasselbe Wort wie der 32-Bit-Pfad** — vier `r8` und zwei `r16` gegen ein
  `r32` auf `REG_SYS_CFG1`. Ohne diesen dritten Test fiele ein Fehler in der
  neuen Host-Fn erst mitten in der Power-Sequenz auf, wo zehn andere Dinge
  gleichzeitig neu sind.

    driver wifi_rtl8822ce        # startet Stufe 0
    wlan                         # zeigt den Bericht danach noch einmal

### Stufe 1 — Nur der Strom
Kein Byte Firmware, kein Ring. Das geht, und es ist der Grund für den Schnitt
genau hier (siehe „Reihenfolge" unten).

- `rtw_mac_pre_system_cfg` · `rtw_pwr_seq_parser` mit beiden Tabellen
  (`trans_carddis_to_cardemu_8822c`, `trans_cardemu_to_act_8822c`) ·
  `rtw_mac_init_system_cfg` · `rtw_mac_power_switch` samt
  `-EALREADY`-Rückfall (der zweite Anlauf ist kein Sonderfall, er ist der
  Normalfall nach einem Warmstart) · `rtw_mac_power_off`.
- **Gate:** `REG_CR` **verlässt** `0xea` beim Einschalten und **kehrt dorthin
  zurück** beim Ausschalten. Beide Richtungen, sonst misst man einen Zustand,
  den der Chip schon vorher hatte.
- Der 8-Bit-Pfad trägt hier zum ersten Mal echte Last: der Interpreter ist
  nichts als `read8`/`write8`. Deshalb steht sein Test schon in Stufe 0.

### Stufe 2 — Ringe, Firmware, efuse
Die drei hängen zusammen (siehe unten), aber jede hat ihr eigenes, billiges
Gate — deshalb drei Teilstufen statt einer. **In 2b kommt der Firmware-Blob
ins Modul** (197,9 KiB; das Modul wächst von 12 KB auf 217 KB).

**2a — die Ringe.** ✅ am Gerät grün
`rtw_pci_init_trx_ring` + `rtw_pci_reset_buf_desc`: 8 TX-Ringe + der
MPDU-RX-Ring. Gate: jedes Adress- und Anzahlregister liest zurück, mit MAC
aus UND an. Gemessen: 1545 von 2048 DMA-Seiten, 11 von 1024 Stücken.

**2b — die Firmware.**
`rtw_download_firmware` vollständig, alle dreizehn Schritte:
`check_firmware_size` · `ltecoex_read_reg` · `wlan_cpu_enable(false)` ·
`download_firmware_reg_backup` (6 Register) · `reset_platform` ·
`start_download_firmware` → je Abschnitt `download_firmware_to_mem` →
`send_firmware_pkt` → `rtw_fw_write_data_rsvd_page` → **BCN-Queue** →
`iddma_download_firmware` → `check_fw_checksum` · `reg_restore` ·
`end_flow` · `wlan_cpu_enable(true)` · `ltecoex_reg_write` ·
`download_firmware_validate` · `rtw_hci_setup`.
Dazu aus `tx.c` der 48-Byte-Sendedeskriptor einer Reserved Page.
**Gate:** `REG_MCUFW_CTRL` liest `FW_READY`. **Ein `Ok()` auf dem Schreibweg
ist keine Quittung** ([[feedback_a_bus_ack_is_not_an_accepted_report]]) — das
Gate ist die Antwort, nicht der Versand.

**2c — efuse und hw_feature.**
`rtw_chip_efuse_info_setup` vollständig: `rtw_parse_efuse_map` ·
`rtw8822ce_efuse_parsing` · `rtw_dump_hw_feature` (C2H, braucht also 2b) ·
`rtw_check_supported_rfe`.
**Gate:** die MAC-Adresse aus der efuse ist gültig, mit `rfe_option`,
`channel_plan`, `crystal_cap` und **`btcoex`** im Log (L6).

> **Warum diese Reihenfolge, und warum der Plan sie zuerst falsch hatte.**
> Der erste Entwurf trennte „Strom und efuse" von „Ringe und Firmware" — als
> ob die efuse vor der Firmware käme. Sie kommt nicht: `rtw_chip_efuse_enable`
> lädt **erst die Firmware**, schreibt dann `C2H_HW_FEATURE_DUMP` und liest
> die Antwort. Und die Firmware geht auf PCIe **durch die BCN-TX-Queue**
> (`rtw_pci_write_data_rsvd_page` → `rtw_pci_tx_write_data(…,
> RTW_TX_QUEUE_BCN)`), also müssen die Ringe vorher stehen. Die echte Kette
> ist **Ringe → FWDL → efuse**, und ein Stufenschnitt quer dazu hätte
> bedeutet, Stufe 1 mit einem Gate zu beenden, das ohne Stufe 2 nicht
> erreichbar ist.

### Stufe 3 — MAC und PHY
- `rtw_init_trx_cfg` (`txdma_queue_mapping`, `priority_queue_cfg`, `init_h2c`) ·
  `rtw8822c_mac_init` · `rtw_drv_info_cfg`.
- `rtw8822c_phy_set_param` vollständig, inklusive beider
  `rtw8822c_header_file_init`-Hälften, `rtw_phy_load_tables` (erzeugte
  Tabellen, §1c), `config_trx_mode`, `rtw_phy_init`, `rtw8822c_rf_init`,
  `pwrtrack_init`, `rtw_bf_phy_init`.
- **Gate:** `rtw_phy_read_rf` auf beiden Pfaden liefert erwartete Werte,
  `false_alarm_statistics` zählt etwas ≠ 0 — der Empfänger hört.

### Stufe 4 — Empfangen
- `rtw_pci_get_hw_rx_ring_nr` als Pollpfad (L4) · `rtw_rx_query_rx_desc` ·
  `query_phy_status` (page0/page1) · `rtw_rx_fill_rx_status` ·
  `rtw_set_channel` samt `rtw8822c_set_channel_bb/_rf`.
- **Gate:** Beacons der Umgebung mit SSID und plausiblem RSSI im Log. Das ist
  der erste Punkt, an dem der Funk selbst etwas sagt.

### Stufe 5 — Senden und verbinden
- `rtw_tx_pkt_info_update` · `rtw_tx_fill_tx_desc` · `rtw_pci_tx_write` ·
  `tx_kick_off` · `rtw_tx_report_*`.
- Obere Hälfte: sw-Scan (`rtw_ops_sw_scan_start/complete` — ein vollständiger
  Linux-Pfad, nicht die Abkürzung um hw_scan herum), Auth, Assoc,
  `rtw_ops_sta_add` → `rtw_fw_send_ra_info`, `rtw_sec_write_cam` für CCMP,
  EAPOL über `npk_wifi_send_event` an `wifid`.
- **Gate:** `LINK_UP`, DHCP, und `ping` durch.

### Stufe 6 — Was einen Link zu einer Verbindung macht
Reihenfolge nach Wirkung, jede für sich messbar:
1. **Koexistenz** (L6) — sonst ist der Durchsatz eine Lotterie, sobald BT geht.
2. **RF-Kalibrierung** (L5) — DPK/TXGAPK/DAC. Gate: Durchsatz und EVM vorher/nachher.
3. **Block-Ack / A-MPDU** — ohne das kein Wi-Fi-5-Durchsatz.
4. **DIG, CCK-PD, CFO-Tracking, Power-Tracking** — `rtw_phy_dynamic_mechanism`.
5. **hw_scan-Offload** (die Firmware kann es, §1d) und **Beacon-Filter**.
6. **LPS/IPS** — Akku.

---

### L8 — **Eine Diagnosestufe muss ZURUECKKEHREN.** *(gilt ab sofort)*

`driver <modul>` geht ueber `spawn_on_worker`, und das setzt `APP_RUNNING`
fuer das Terminal: die Tasten gehen an das Modul, der Prompt bleibt aus. Ein
Modul, das in einer Endlosschleife sitzt, macht damit genau das Terminal
unbrauchbar, aus dem man gerade debuggt. Florian, 2026-09-19: *„sonst hängt
mir das System beim Debug."*

Die Stufen 0–2a kehren deshalb zurueck. Der Kernel raeumt danach auf — DMA
freigeben, PCI loesen — **und loescht den Bericht**, ausdruecklich begruendet
mit „a dead driver's snapshot must not read as live numbers". Das Ergebnis
einer terminierenden Stufe steht also im TERMINAL, nicht in `wlan`.

**Ab 2b kommt eine zweite Pflicht dazu**, und sie stand schon im AX200
(`wifi_ax200/src/lib.rs`, Ende von `_start`): *„the kernel frees our DMA
buffers on return and a still-running firmware must not DMA into them
afterwards."* Sobald die Firmware laeuft, muss sie **vor** dem Zurueckkehren
angehalten werden. Solange nur der MAC an- und ausgeht, ist das erledigt.

**Offen fuer Stufe 5:** ein fertiger Treiber muss bleiben. Dann ist zu
entscheiden, ob `intent_run_driver` auf `spawn_on_worker_background`
umgestellt wird — den Weg, den `debug.wasm` schon nimmt und der Tasten und
Prompt beim Terminal laesst. Ein Treiber liest keine Tasten; `APP_RUNNING`
ist fuer ihn die falsche Einstufung. Heute nicht angefasst, weil kein
Treiber im Baum bleibt.

## ▶ HIER WEITERMACHEN (Stand 2026-09-20)

**Alles gepusht, Baum sauber. Der nächste Schritt ist ein GERÄTELAUF von
0.10.0** — `install wifi_rtl8822ce && driver wifi_rtl8822ce`. Die Stufen 3a,
3b und 3c melden sich einzeln mit ihren Gates; was sie sagen, entscheidet, ob
Stufe 4 dran ist oder eine der drei nachgebessert wird.

### Was das Gerät zuletzt gemessen hat (nicht abschreiben, das steht hier)

    SYS_CFG1 0x0c493d3d -> cut 3 = RTW_CHIP_VER_CUT_D, vendor 9, RF 2T2R
    MAC       e0:0a:f6:8b:bf:83   (aus der efuse)
    rfe_option 1 · channel_plan 0x7f · crystal_cap 63 · regd 1
    rf_board_option 0x21 -> btcoex JA, share_ant JA
    thermal A/B 27/27 · hw_cap nss 2, ant 2, bw 0x07 (bis 80 MHz), hci 0x04
    Power-Sequenz 605-614 us · FW_READY nach 3778-3938 us · efuse in 2 ms
    DMA: 1545 von 2048 Seiten, 11 von 1024 Stuecken, alles unter 1 GiB

### Was in 0.10.0 dazugekommen ist

**Stufe 3a — `rtw_mac_init` (mac.c:1391), vollständig.**
`txdma_queue_mapping` · `rtw_set_trx_fifo_info` · `__priority_queue_cfg` ·
`init_h2c` · `rtw8822c_mac_init` (77 Registerzugriffe) · `rtw_drv_info_cfg` ·
`rtw_pci_interface_cfg`. Der Seitenplan fällt daraus heraus und steht im Log:
`txff 2048 Seiten, rsvd 110, acq 1938 -> rsvd_boundary 1938`.

**Der Ablauf ist jetzt Linux' Ablauf, und das heißt: die Firmware wird ZWEIMAL
geladen.** Alles bis 2c ist `rtw_chip_efuse_info_setup` (main.c:1999) — die
Probe-Zeit, die den Chip nur für die efuse anwirft und danach wieder
ausschaltet. Stufe 3 ist `rtw_power_on` (main.c:1374) und fängt wieder ganz
vorn an: `rtw_hci_setup` → `rtw_mac_power_on` → `rtw_download_firmware` →
`rtw_mac_init` → `phy_set_param`. Ein ausgeschalteter MAC hat keine Firmware
mehr; der zweite Download ist kein Versehen.

**`fifo.rsvd_boundary` fährt jetzt durch den Download-Pfad** statt als feste 0
— das war die eine Stelle, an der 3a in bestehenden Code greift. Sie ist beim
ERSTEN Download trotzdem 0, weil `rtw_mac_init` da noch nicht gelaufen ist;
genau so steht es in Linux.

**Stufe 3b — die Tabellen.** `gen_tables.py` erzeugt `src/tables.rs` aus
`rtw8822c_table.c`: **92 430 Wörter = 361 KiB** in sechs Tabellen (mac, bb,
agc, rfk_init, rf_a, rf_b). Dazu der Bedingungsläufer `rtw_parse_tbl_phy_cond`
und die vier `rtw_phy_cfg_*`, sowie `rtw_phy_read_rf` /
`rtw_phy_write_rf_reg_mix`.

**Die RF-Tabellen werden in UMGEKEHRTER Reihenfolge geladen** und das ist kein
Tippfehler: `rtw8822c_hw_spec` sagt `.rf_tbl = {&rtw8822c_rf_b_tbl,
&rtw8822c_rf_a_tbl}` (rtw8822c.c:5382), die Schleife läuft über den Index, und
jede Tabelle trägt ihren Pfad selbst. Erst B, dann A.

**Der Erzeuger rechnet das Gate gleich mit.** Für unseren Chipzustand
(cut D · rfe_option 1 · PCIe · pkg 15) sagt er vorher, wieviele
Schreibzugriffe jede Tabelle abgeben muss:

    mac 0 · bb 1289 · agc 450 · rfk_init 2460 · rf_a 789 · rf_b 697

Der Treiber zählt mit und meldet jede Abweichung. **Das prüft den
Bedingungsläufer selbst** — die Regel „`rfe` wird IMMER verglichen, auch auf
0, `cut`/`pkg`/`intf` nur wenn genannt" (phy.c:1130-1171) ist genau die Art
Detail, das man still falsch baut und erst als Funkausfall merkt.

**Stufe 3c — `rtw8822c_phy_set_param` (rtw8822c.c:1862).**
`header_file_init(pre)` · `rtw_phy_load_tables` · Quarzkapazität ·
`header_file_init(post)` · `config_trx_mode` (mit cck/ofdm-Pfaden, `bb_reset`,
`toggle_igi`) · `rtw_phy_init` · `rtw8822c_rf_init` · `pwrtrack_init` ·
`rtw_bf_phy_init`.

Der dickste Brocken darin ist die **DAC-Kalibrierung** (`rfk.rs`, ~900 Zeilen
C): sie misst den Gleichspannungsversatz von ADC und DAC beider Pfade und
trägt den Ausgleich ein. `dac_cal_restore` ist mit portiert, greift beim ersten
Lauf aber nicht (`dack_msbk` ist null) — sobald der Treiber den Chip ein
zweites Mal anwirft, spart sie die ganze Messung.

### Die Gates der Stufe 3, die nur das Gerät beantworten kann

| Gate | Was es misst |
|---|---|
| **3a** | `AUTO_INIT_LLT_V1` löscht sich selbst (die Hardware hat die Link-List-Tabelle gebaut) · `init_h2c` findet `h2cq_size == h2cq_free` · `rsvd_boundary == 1938` · `REG_CR` trägt `MAC_TRX_ENABLE` · `PCIE_EMAC_PDN_AUX_TO_FAST_CLK` steht (cut D) |
| **3b** | jede Tabelle gibt genau so viele Schreibzugriffe ab wie gerechnet · RF-Register 0x00 und 0x18 antworten auf BEIDEN Pfaden mit etwas, das weder 0 noch 0xfffff ist |
| **3c** | `phy_set_param` läuft durch und beide RF-Pfade antworten mit Tabellenwerten. **Die CCA-Zähler sind hier KEIN Gate** — sie laufen in Linux erst nach der Coex-Antenne und `set_channel`, also ab Stufe 4 |

### Werkzeuge (im Modulverzeichnis)

    python3 tools/wasm/wifi_rtl8822ce/check_regs.py   # 462 Konstanten, 0 Abweichungen
    python3 tools/wasm/wifi_rtl8822ce/seqdiff.py      # 22 Funktionen, Zugriff fuer Zugriff
    python3 tools/wasm/wifi_rtl8822ce/gen_tables.py   # src/tables.rs aus rtw8822c_table.c
    python3 tools/wasm/wifi_rtl8822ce/gen_pwrseq.py   # src/pwrseq.rs aus rtw8822c.c
    python3 tools/linux-coverage.py --chip rtl8822ce   # 165 / 940 (war 89)

**Abdeckung nach Stufe 3: 165 von 940 rtw88-Funktionen** (vorher 89). Die
Zuwächse liegen dort, wo sie hingehören: `rtw8822c.c` 56/171, `mac.c` 37/49,
`phy.c` 23/97, `pci.c` 19/81. Eine Datei im Hauptweg auf 0 gibt es nicht mehr
außer `coex.c`, `ps.c`, `rx.c` und `mac80211.c` — und die sind alle Stufe 4
oder später.

**`check_regs.py` UND `seqdiff.py` vor jedem Commit laufen lassen.** Der erste
hat schon einen echten Fehler gefunden (`TX_DESC_QSEL_H2C` war 17 geraten, ist
19); der zweite prüft die Sache, die keine Konstantenliste sehen kann — ob die
Zugriffe in derselben REIHENFOLGE, mit derselben BREITE und mit denselben
rohen Hexzahlen stehen wie in Linux. Beide sind gegen einen absichtlich
eingebauten Fehler geprüft: ein Zahlendreher in der DACK und ein
32-auf-16-Bit-Wechsel werden beide gemeldet.

**`check_regs.py` löst jetzt beide Seiten REKURSIV auf** und liest auch
`rtw8822c.c` und `bf.h`. Vorher fielen zusammengesetzte Makros wie
`WLAN_SIFS_CFG` (vier Werte über drei Zeilen) still durch — also genau die,
die man beim Abtippen falsch macht. Von 147 geprüften Konstanten auf 462.

### Benannt und offen: die DAC-Kalibrierung konvergiert nicht

**Gemessen am Geraet (0.10.2), beide Pfade, je zehn Runden:**

    ADCK A  12/2 13/2 12/2 12/3 12/2 12/2 12/2 12/2 12/2 12/2   nicht konvergiert
    ADCK B  20/24 ... zehnmal derselbe Wert                      nicht konvergiert
    DACK A  0/1                                                  konvergiert
    DACK B  36/39 36/40 ... zehnmal derselbe Wert                nicht konvergiert

Der Abbruch haengt an „Restversatz unter 5". Die Korrektur wird geschrieben
(`base_addr + 0x68`), die Hardware nimmt sie an — `failed to write IQ vector
to hardware` steht **nicht** im Log — und die Nachmessung aendert sich nicht.
In Runde 5 der ADCK A steht sogar ein anderer Ausgleich (`0x0c0c` statt
`0x080c`), das Register wird also wirklich beschrieben.

**Was ausgeschlossen ist, durch Messung:**

- **Der Pfadzugriff.** `RF 0x3e` liest `A=0x3, B=0x20` — genau die Werte, die
  `rf_a` und `rf_b` dort schreiben. Es ist das einzige Register, auf das die
  zwei Tabellen verschieden schreiben und das danach niemand mehr anfasst.
  Dass `RF 0x00` und `0x18` auf beiden Pfaden gleich lesen, ist KEIN Befund:
  dort schreiben beide Tabellen denselben Wert (nachgerechnet gegen den
  Bedingungslaeufer).
- **Eine eingefrorene Messung.** Die Rohproben aus `0x2dbc` streuen und
  unterscheiden sich je Pfad (A: i −18…−7, B: i −23…−19), 100 von 100
  Lesungen gueltig.
- **Ein Portierfehler.** `seqdiff.py` findet die Funktionspaare jetzt SELBST
  ueber die Doc-Kommentare und vergleicht **65 Funktionen** statt 22 — darunter
  die ganze DACK. Alle gleich, Zugriff fuer Zugriff und Zahl fuer Zahl. Der
  eine scheinbare Unterschied in `dac_cal_adc` war Linux' eigene
  `rtw_dbg`-Formatzeichenkette, die das Werkzeug als Registerwerte gelesen hat.

**Warum es trotzdem kein Gate ist.** Linux prueft die Konvergenz **nirgends**:
`rtw8822c_rf_dac_cal` laeuft zehnmal und geht weiter. Es gibt also kein
Vergleichsmass dafuer, was auf DIESEM Board herauskommen muesste, und der
Entwicklungsrechner hat eine andere Karte (8852CE/rtw89). Daraus ein Tor zu
machen hiesse, eine Meinung zu pruefen. Es steht als BEFUND im Log, mit
Zahlen, und hier.

**Die Folge, soweit absehbar:** ein unkompensierter Gleichspannungsversatz
verschlechtert die Empfindlichkeit des betroffenen Pfades. Er haelt den
Empfaenger nicht an. Wenn Stufe 4 steht und Pfad B messbar schlechter hoert
als A, ist das hier die erste Spur.

### Stufe 4 — bis der Empfänger hört

Gemessen, nicht vermutet: nach `phy_set_param` stehen die CCA-Zähler auf 0,
und **in Linux ist das genauso**. `false_alarm_statistics` läuft dort erst im
Wachhund (main.c:280), und davor liegen drei Dinge. Sie sind die drei
Unterstufen, jede eine ganze Linux-Funktion, keine davon übersprungen.

**4a — der Rest von `rtw_power_on` und `rtw_core_start`. GEBAUT in 0.11.0,
am Gerät ungeprüft.**

    rtw_power_on (main.c:1374), ab wo wir stehen:
     ├─ rtw_mac_postinit            beim 8822C NULL, also nichts
     ├─ rtw_hci_start = rtw_pci_start   schaltet NUR Interrupts frei;
     │                                  wir pollen -> benannte Abweichung
     ├─ rtw_fw_send_general_info    H2C-PAKET durch die H2C-Queue
     ├─ rtw_fw_send_phydm_info      dito
     ├─ rtw_coex_power_on_setting   "set antenna path to BT"
     └─ rtw_coex_init_hw_config     danach ANT_INIT (btcoex JA, share_ant JA)
    rtw_core_start (main.c:1517):
     ├─ rtw_sec_enable_sec_engine
     └─ rtw_write32(REG_RCR, hal->rcr)   "rcr reset after powered on"

**Was die Coex-Kette mitzieht, ausgezählt statt geschätzt:** `set_ant_path`
(152 Zeilen) · `table` + `set_table` · `tdma` · `write_scbd`/`read_scbd` ·
`init_coex_var` · `monitor_bt_enable` · `wl_slot_extend` · `set_init` ·
`set_wl_pri_mask` · `set_gnt_debug` · `query_bt_info` — und darunter die
**BT-Mailbox** (`rtw_fw_bt_wifi_control`, `rtw_fw_query_bt_info` über
`rtw_fw_send_h2c_command`, also die HMEBOX-Register, nicht die H2C-Queue)
sowie die Chip-Ops `rtw8822c_coex_cfg_init/_ant_switch/_gnt_fix/_gnt_debug/
_rfe_type/_wl_tx_power/_wl_rx_gain`. Zusammen ~400 Zeilen C plus die
Koexistenz-Tabellen des Chips.

**Zwei H2C-WEGE, und sie sind nicht dasselbe** — das ist die Falle dieser
Stufe: `rtw_fw_send_h2c_command` schreibt in die **HMEBOX**-Register
(Mailbox, 8 Byte, für Coex), `rtw_fw_send_h2c_packet` schiebt ein 32-Byte-
Paket durch die **H2C-Queue** (der Ring, dessen Adresse `init_h2c` gesetzt
hat, für general/phydm info). Wer den einen für den anderen hält, schickt
alles ins Leere.

**Gate 4a, vier Teile:** beide H2C-Pakete geschrieben · **der Chip hat sie
abgeholt** (der HW-Lesezeiger der H2C-Queue steht auf unserem Schreibzeiger —
das ist der Beweis, dass der Ring aus Stufe 3a wirklich trägt) · das
Score-Board trägt `ACTIVE|ONOFF` · `REG_RCR` steht auf `hal->rcr`.

**Und die offene Frage, die 4a beantwortet:** zählen die CCA-Zähler jetzt
schon? Die Antenne ist der wahrscheinlichste einzelne Grund. Der Treiber sagt
es in Klartext — zählt er, war es die Antenne; zählt er nicht, fehlt der
Kanal und das ist 4c. Beides ist eine Antwort.

**Eine benannte Abweichung, und nur eine:** `rtw_hci_start` = `rtw_pci_start`
schaltet ausschließlich Interrupts frei. Dieses Modul fährt den Chip im
Abfragebetrieb, es gibt keine Interruptleitung dorthin.

**Ein Fehler, den die Portierung selbst gefangen hat:** der H2C-Ring braucht
einen Zwischenpuffer mit einem Platz je RINGeintrag (128 × 128 Byte), nicht
je gesendetem Paket — `wp` läuft über die ganze Ringlänge. Die erste Fassung
teilte sich den Puffer mit dem Firmware-Download, der genau ein Stück groß
ist. Beim Anlauf mit zwei Paketen wäre das nie aufgefallen.

**4b — die Sendeleistung.** `rtw_chip_board_info_setup` (main.c:2064):
`rtw_phy_init_tx_power` · `bb_pg_type0` und `txpwr_lmt_type0` laden ·
`rtw_phy_tx_power_by_rate_config` · `rtw_phy_tx_power_limit_config`. Zwei
weitere erzeugte Tabellen (~100 KiB) und ein eigener Parser mit der
Regulierungszonen-Ersatzlogik. In Linux läuft das zur PROBE-Zeit, nicht in
`power_on` — es steht hier, weil 4c es braucht.

**4c — `rtw_set_channel`.** `rtw8822c_set_channel_bb` (157 Zeilen, AGC,
CCA-Maske, RX-Filter) · `rtw_set_channel_mac` · `rtw8822c_set_channel_rf` ·
`toggle_igi` · `rtw_coex_switchband_notify` · `rtw_phy_set_tx_power_level`.
**Gate 4c: `false_alarm_statistics` zählt CCA-Ereignisse ≠ 0** — das Gate,
das in 0.10.0 fälschlich schon an Stufe 3 hing.

### Danach

`rtw_hci_start` im Ernst (Empfangsring füllen, `rx_tag`, `is_c2h` trennen),
`rtw_set_channel` aus der oberen Hälfte heraus, Scan, Auth, Assoc — und dann
ist `wifid` dran, das herstellerunabhängig schon steht.

### Der Ablauf für eine neue Version

    # Modul allein (Kernel unverändert):
    sed -i 's/^version = .*/version = "0.X.Y"/' tools/wasm/wifi_rtl8822ce/Cargo.toml
    tools/stage-module.sh wifi_rtl8822ce
    python3 tools/forge-gate.py release/modules/wifi_rtl8822ce.wasm
    ./build.sh sign-modules
    # commit: NUR eigene Pfade, nie `git add -A` — nebenan liegt fremde Arbeit
    # am Gerät: install wifi_rtl8822ce && driver wifi_rtl8822ce

**Stagen, signieren und committen gehören in EINEN Zug.** Dazwischen kann ein
fremder Release das Manifest mitnehmen und ein Paar zerreißen — genau das ist
am 2026-09-19 einmal passiert.

## 4 — Die Regeln, die in jeder Stufe gelten

1. **Ganze Funktionen, alle Zweige**, auch die „vermutlich no-op". Keine
   Abkürzung aus Bequemlichkeit. Wo wir bewusst abweichen (Polling statt IRQ),
   steht es in diesem Papier — **eine Abweichung, die nicht hier steht, ist
   ein Bug.**
2. **Jede Konstante gegen `reg.h` prüfen**, nicht aus dem Zusammenhang raten.
3. **Write-Breite ist Semantik** (L1). 8, 16 und 32 sind nicht austauschbar.
4. **Tabellen werden erzeugt, nicht abgetippt**, und das Erzeugte wird gegen
   die Linux-Zeilenzahl geprüft.
5. **Abdeckung nach jeder Stufe zählen:**
   `python3 tools/linux-coverage.py --chip rtl8822ce`. Fällt eine Datei im
   Hauptweg auf 0, fehlt eine Schicht — dann nicht debuggen, portieren.
6. **Die rohen Bytes vor der Auswertung**
   ([[feedback_dump_the_raw_input_before_debugging_the_interpretation]]).
7. **Kein Wachhund an ein Zeitlimit ohne Verkehr**
   ([[feedback_silence_is_not_evidence_a_watchdog_can_act_on]]).

## 5 — Ablage und Release

    tools/wasm/wifi_rtl8822ce/        Modulname = Treibername (Praefix `wifi` -> Klasse 02:80)
      src/host.rs      Treiber-ABI
      src/regs.rs      Register und Bits, jede Zeile mit ihrer Quellzeile
      src/pwrseq.rs    ERZEUGT aus rtw8822c.c (gen_pwrseq.py)
      src/tables.rs    ERZEUGT aus rtw8822c_table.c (gen_tables.py), 361 KiB
      src/mac.rs       mac.c   — Strom, Firmware-Download, rtw_mac_init
      src/pci.rs       pci.c   — Ringe, rsvd page, interface_cfg
      src/fw.rs        fw.c    — check_hw_ready, write_data_rsvd_page
      src/tx.rs        tx.c    — der Sendedeskriptor
      src/efuse.rs     efuse.c — efuse-Abzug, hw_feature
      src/phy.rs       phy.c   — Tabellenlader, Bedingungslaeufer, RF-Zugriff
      src/bf.rs        bf.c    — rtw_bf_phy_init
      src/chip.rs      rtw8822c.c — was NUR dieser Chip tut
      src/rfk.rs       rtw8822c.c — die DAC-Kalibrierung
      src/dm.rs        struct rtw_dm_info, der PHY-Zustand
      firmware/rtw8822c_fw.bin
      gen_pwrseq.py · gen_tables.py · check_regs.py · seqdiff.py

Ein reiner Modulwechsel geht über `tools/stage-module.sh wifi_rtl8822ce` +
`./build.sh sign-modules` — **kein Kernel-Versionssprung**. Die zwei neuen
Host-Fns aus L1 sind dagegen eine Kernel-Änderung und damit ein
`kernel+module wifi_rtl8822ce:`-Commit.

**Der AX200-Faden bleibt unberührt** — seine Verbindung läuft
([[project_wifi_stability_handover]]).

## 6 — Was bewusst nicht gebaut wird

AP-Modus · Mesh/IBSS/TDLS · WoWLAN (`wow.c`, 43 fn) · debugfs (`debug.c`,
49 fn) · LED · SDIO/USB-Busse · Suspend/Resume · `sar.c`, bis jemand es
verlangt. Zusammen 95 der 940 Funktionen plus die Bus-Varianten.
