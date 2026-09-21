# RTL8822CE — der Plan, bevor die erste Zeile steht

**Chip:** `10ec:c822` · Realtek RTL8822CE · Wi-Fi 5, 2T2R, PCIe · im Lenovo
IdeaPad Flex 5 14ALC7 das **einzige** eingebaute Netzgerät (`02:00.0`).
**Linux:** 6.18.26, `drivers/net/wireless/realtek/rtw88/`, Modul `rtw_8822ce`.
**Firmware:** `rtw88/rtw8822c_fw.bin`, 202 600 Bytes, **Version 9.9.15**.
**Karte:** [WIFI_RTL8822CE_LINUX_MAP.md](WIFI_RTL8822CE_LINUX_MAP.md) — was der
Linux-Treiber hat, Datei für Datei, ausgezählt.

**Stand 2026-09-20: DER EMPFÄNGER HÖRT.** Kernel **0.383.0** · Modul
**wifi_rtl8822ce 0.13.0**. **Stufen 0 bis 4c am Gerät GRÜN.**

Der Lauf von 0.13.0 auf Kanal 1, 20 MHz, nach 200 ms:

    Falschalarme: cck 57 · ofdm 91 · gesamt 148
    CCA:          cck 137 · ofdm 93 · gesamt 230
    CRC ok/err:   cck 34/0 · ofdm 2/1 · ht 0/0
    RF 0x18: A 0x00003001 B 0x00003001  (Kanal 1, Bandbreite 20 MHz)
    Leistungsindex A: 1M 71 · 6M 72 · MCS7 72  ·  B: 1M 88 · 6M 83 · MCS7 83

**34 CCK-Pakete mit gültiger Prüfsumme und KEIN einziger Fehler.** Das sind
Beacons der Nachbarschaft, sauber dekodiert. Damit ist die ganze Kette Ende
zu Ende bewiesen: PCI → Bridge → Power-Sequenz → Ringe → DMA → Firmware →
C2H → efuse → Sendeleistungstabellen → MAC-Init → Parametertabellen → BB/RF
→ DAC-Kalibrierung → Coex-Antenne → Kanal → AGC und CCA-Maske.

**Geprüft ist, was sich ohne Gerät prüfen LÄSST**, und zwar mechanisch:
`check_regs.py` hält **651 Konstanten** gegen die Linux-Quelle (0 Abweichungen),
`seqdiff.py` vergleicht **107 Funktionen Zugriff für Zugriff und Zahl für Zahl**
mit dem C-Original, `txpwrcheck.py` baut die Sendeleistungskette host-seitig
und hält sechs Prüfsummen gegen eine unabhängige Nachrechnung, und
`gen_tables.py` rechnet vorher aus, wieviele
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
der Linux-Quelle (651 geprüft, 0 Abweichungen), `seqdiff.py` hält jede
portierte Funktion Zugriff für Zugriff und Zahl für Zahl gegen das C-Original
(107 Funktionen), und `txpwrcheck.py` prüft die Sendeleistung host-seitig
gegen eine unabhängige Nachrechnung. Die Regel „vor jedem Commit grep gegen reg.h" tun damit zwei
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
    python3 tools/wasm/wifi_rtl8822ce/seqdiff.py      # 107 Funktionen, Zugriff fuer Zugriff
    python3 tools/wasm/wifi_rtl8822ce/txpwrcheck.py   # Sendeleistung host-seitig
    python3 tools/wasm/wifi_rtl8822ce/gen_tables.py   # src/tables.rs aus rtw8822c_table.c
    python3 tools/wasm/wifi_rtl8822ce/gen_pwrseq.py   # src/pwrseq.rs aus rtw8822c.c
    python3 tools/linux-coverage.py --chip rtl8822ce   # 265 / 940 (war 89)

**Abdeckung nach Stufe 6b: 398 von 940 rtw88-Funktionen** (vor dieser Runde
89). `rtw8822c.c` 68/171 · `phy.c` 59/97 · `mac.c` 39/49 · `pci.c` 30/81 ·
`coex.c` 27/111 · `main.c` 18/84 · `tx.c` 15/31 · `efuse.c` 5/5 · `rx.c` 3/8.
Auf 0 stehen nur noch `mac80211.c` (die obere Hälfte, die `wifid` ersetzt),
`debug.c`, `led.c` und `wow.c` — die letzten drei stehen unter „wird bewusst
nicht gebaut".

**Die vier Prüfer vor jedem Commit:** `check_regs.py` · `seqdiff.py` ·
`txpwrcheck.py` · `cfgcheck.py` (der Konfigurationsleser, host-seitig gegen
13 Randfälle — greift er daneben, bleibt `target` leer, Stufe 5e wird
übersprungen, und nichts im Log sagt, dass ein Parser schuld war).

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

**Gate 4a, fünf Teile:** beide H2C-Pakete geschrieben · **die Firmware hat die
H2C-Queue leergeräumt** (der HW-Lesezeiger holt unseren Schreibzeiger ein —
der Beweis, dass der Ring aus Stufe 3a wirklich trägt) · **GNT_WL und GNT_BT
stehen so, wie `set_ant_path(INIT)` sie setzt** · der Pfadbesitzer ist WLAN ·
`REG_RCR` steht auf `hal->rcr`.

**Das Score-Board ist KEIN Gate**, und das ist am Gerät gelernt. Es ist ein
gemeinsames Postfach; die Bits, die interessieren, schreibt der BT-Kern.
Läuft der nicht, steht dort 0 — im Lauf von 0.11.0 stand dort 0, `kt_ver 3`,
`bt_disabled`. Linux prüft es nirgends. Was sich prüfen lässt, ist die
**Wirkung**: bei `bt_disabled` nimmt `set_ant_path(COEX_SET_ANT_INIT)` den
Zweig `GNT_BT = SW_LOW`, `GNT_WL = SW_HIGH` — die Antenne geht an WLAN. Das
steht im indirekten LTE-Registerraum und ist lesbar. Der Rohwert des
Score-Boards steht trotzdem im Log, ohne die Maske von `read_scbd`: sonst
lässt sich eine 0 nicht von „unser eigener Schreibzugriff kam nie an"
unterscheiden.

**Und die offene Frage, die 4a beantwortet:** zählen die CCA-Zähler jetzt
schon? Die Antenne ist der wahrscheinlichste einzelne Grund. Der Treiber sagt
es in Klartext — zählt er, war es die Antenne; zählt er nicht, fehlt der
Kanal und das ist 4c. Beides ist eine Antwort.

**Zwei benannte Abweichungen:** `rtw_hci_start` = `rtw_pci_start` schaltet
ausschließlich Interrupts frei — dieses Modul fährt den Chip im
Abfragebetrieb, es gibt keine Interruptleitung dorthin. Und der Lesezeiger
der H2C-Queue wird von uns ABGEWARTET; Linux holt ihn gar nicht ab. Das ist
eine Messung, kein Verhalten: in 0.11.0 stand er auf 1, während unserer schon
auf 2 stand, und eine Stichprobe einen Befehl nach dem Anstoß sagt nichts
darüber, ob der Chip nicht will oder nur noch nicht fertig ist.

**Ein Fehler, den die Portierung selbst gefangen hat:** der H2C-Ring braucht
einen Zwischenpuffer mit einem Platz je RINGeintrag (128 × 128 Byte), nicht
je gesendetem Paket — `wp` läuft über die ganze Ringlänge. Die erste Fassung
teilte sich den Puffer mit dem Firmware-Download, der genau ein Stück groß
ist. Beim Anlauf mit zwei Paketen wäre das nie aufgefallen.

**4b — die Sendeleistung. GEBAUT in 0.12.0.** `rtw_chip_board_info_setup`
(main.c:2064) vollständig: `rtw_phy_init_tx_power` · `bb_pg_type0` und
`txpwr_lmt_type0` laden · `rtw_phy_tx_power_by_rate_config` ·
`rtw_phy_tx_power_limit_config`, samt der Regulierungszonen-Ersatzlogik und
den vier Querabgleich-Stufen.

**Sie läuft VOR Stufe 3**, weil sie in Linux vor `rtw_power_on` läuft:
`rtw_chip_info_setup` = `parameter_setup` → `efuse_info_setup` →
`board_info_setup`. Die Nummer 4b ist die Reihenfolge, in der gebaut wurde,
nicht die, in der gelaufen wird.

Drei Tabellen dazu erzeugt und gegen die Quelle gezählt: `bb_pg_type0` 46
Zeilen, `txpwr_lmt_type0` 2340, `txpwr_lmt_type5` 1365. **Beide RFE-Typen**,
nicht nur der, den dieses Board meldet — `rtw_get_rfe_def` schlägt in
`rtw8822c_rfe_defs[]` nach, und wer nur einen Eintrag baut, hat einen Treiber
für genau ein Board.

**Gate 4b braucht kein Gerät, und das ist der Punkt.** Die Funktion fasst
kein Register an; sie füllt 25 KiB abgeleiteten Zustand. Ein falsches Byte
darin ist am Gerät eine schiefe Sendeleistung auf einem Kanal — und nichts,
was ein Log zeigt. Also rechnet `gen_tables.py` dieselbe Kette ein zweites
Mal nach, in Python, und legt sechs Prüfsummen ab; `txpwrcheck.py` baut
`txpower.rs` host-seitig und hält die eigenen dagegen. **6 von 6 gleich.**
Gegengeprüft mit EINEM falschen Zeichen (`size - 3` statt `size - 2` in der
VHT-Basisrate) — das schlägt auf alle sechs Summen durch.

**Eine benannte Abweichung:** `rtw_phy_setup_phy_cond` steht in Linux in
`board_info_setup`, bei uns in `phy_set_param`. Es rechnet die Bedingung für
die PARAMETERtabellen aus `cut_version` und `rfe_option` — beide dort schon
bekannt, kein Registerzugriff, gleiches Ergebnis.

**4c — `rtw_set_channel`. GEBAUT in 0.13.0.** `rtw8822c_set_channel_bb`
(157 Zeilen: AGC, CCA-Maske, RX-Filter, Bandbreite) · `rtw_set_channel_mac` ·
`rtw8822c_set_channel_rf` (Band, Kanal, RFSI und Bandbreite in EINER Zahl,
RF 0x18) · `rtw8822c_toggle_igi` · `rtw_phy_set_tx_power_level` mit der
ganzen Kette darunter: `get_tx_power_index` · `get_2g/5g_tx_power_index` ·
`get_tx_power_limit` · `dis_dpd_by_rate_diff` · `rate_to_rate_section` ·
`channel_group` · `rtw8822c_set_tx_power_index` mit
`set_write_tx_power_ref` und `set_tx_power_diff`.

**Gate 4c — am Gerät GRÜN.** RF 0x18 trägt auf beiden Pfaden Kanal 1 und
20 MHz (`0x00003001`), `false_alarm_statistics` zählt 230 CCA-Ereignisse in
200 ms, und **34 CCK-Pakete kamen mit gültiger Prüfsumme und null Fehlern
durch**. Der Empfänger hört nicht nur, er versteht.

**Zwei benannte Abweichungen.** `rtw_get_channel_params` liest in Linux eine
`cfg80211_chan_def`; die gibt es ohne obere Hälfte nicht. Für 20 MHz ist ihr
Ergebnis genau `center = primary = Kanal`, und das wird eingesetzt — die
Rechnung für 40 und 80 MHz kommt mit der Stufe, die eine Bandbreite wählt.
Und `rtw_coex_switchband_notify` gehört zur laufenden Koexistenz
(`rtw_coex_run_coex` mit `COEX_RSN_2GSWITCHBAND`) und braucht den
Verkehrszustand, den erst eine Verbindung hat.

**Ein Fehler, den das Lesen gefangen hat:** ich hatte
`rtw8822c_set_write_tx_power_ref` geschrieben, bevor ich sie gelesen hatte —
und sie sieht anders aus, als sie aussehen „müsste": zwei Bezugswerte je Pfad
in vier festen Registern (`0x18a0`/`0x41a0`, `0x18e8`/`0x41e8`), und vor
JEDEM Schreibzugriff wird `0x1c90` Bit 15 gelöscht. Ersetzt durch die echte.

### Stufe 5a — der Empfangsweg (0.14.0)

Gebaut: `rtw_pci_get_hw_rx_ring_nr` · `rtw_pci_dma_check` (das `rx_tag` aus
Stufe 2a hat seinen ersten Leser) · `rtw_pci_sync_rx_desc_device` ·
`rtw_pci_rx_napi` als Abfrageweg · `rtw_rx_query_rx_desc` ·
`query_phy_status` mit **beiden** Seiten · `rtw_phy_power_2_db` /
`db_2_linear` / `linear_2_db` / `rf_power_2_rssi` · `rtw_set_rx_freq_band` ·
`rtw_update_rx_freq_for_invalid`. Gate: ein Rahmen landet mit Länge, Rate,
Bandbreite, Kanal und Signalstärke im Ring.

**Ein Ring, dessen Inhalt niemand lesen kann.** `RxRing` warf seit Stufe 2a
die DMA-Handles seiner Puffer weg (`let (_h, phys) = alloc_pages(...)`) — der
Chip braucht nur die physische Adresse, und solange nichts gelesen wurde,
fiel es nicht auf. Ein Empfangsring ohne Lesezugang fällt genau einmal auf:
beim ersten Paket. Jetzt trägt er `chunk_handle` und `buf_loc()`.

**`query_phy_status_page1` stand zuerst als Kurzfassung da, und ihre Masken
waren falsch.** Gegen `rtw8822c.h:156-178` nachgelesen: `PWDB_B` liegt in
Wort 0 Bits 23:16 (nicht 15:8), `L_RXSC` in Bits 11:8 (nicht 3:0), der Kanal
in Wort **1**, `RXSNR_A/B` in Wort 6 Bits 7:0 und 15:8. Dazu fehlten
`cfo_tail`, die vier `dm_info`-Rückschreibungen und die Pfaddiversität.
Beide Seiten stehen jetzt ganz da — einschliesslich der `<=`-Schleife, die
in Linux bei zwei Pfaden **dreimal** läuft und `rssi[2]` schreibt. Sie ist
abgeschrieben, nicht stillschweigend korrigiert.

**Die CCK-Verstaerkungsgrenzen gehören dem Treiber.** `l_bnd`/`u_bnd` sind
`dm_info->cck_gi_l_bnd`/`_u_bnd`, einmal in `phy_set_param` aus der Hardware
gelesen (am Gerät 16 und 63). Sie stehen jetzt in `chip::read_cck_gi_bnd` —
demselben Code, mit einem zweiten Rufer.

**Drei Abweichungen von `rtw_pci_rx_napi` geschlossen**, alle beim
Gegenlesen gefunden: `rx_done` zählt in Linux **nur** die Funkrahmen, nicht
die C2H-Antworten · der Schreibzeiger geht ungemaskt ins Register ·
`rtw_update_rx_freq_for_invalid` fehlte ganz (ein CCK-Rahmen kann mit Kanal 0
kommen, dann gilt der laufende Kanal).

**`seqdiff.py` meldete eine Funktion mit null Zugriffen, und das war das
Werkzeug.** Steht ein `#[allow(...)]` zwischen Doc-Kommentar und Definition,
brach der Rumpf-Finder ab — die Funktion erschien als „null Zugriffe", und
eine stille Null sieht aus wie Übereinstimmung. Dabei kam
`rtw_get_tx_power_params` zum Vorschein, das seit Stufe 4b unbemerkt
mitlief. Dazu kennt das Werkzeug jetzt **Teilstücke**
(`/// <datei>.c:<zeilen>, ein Stueck aus \`<name>\``): was bei uns aus einer
C-Funktion herausgelöst steht, wird für den Vergleich wieder angehängt,
statt aus der Prüfung zu fallen. **124 von 124 Funktionen Zugriff für
Zugriff gleich.**

### Stufe 5b — der Sendeweg (0.15.0)

Gebaut: `rtw_vif_port_config` + `rtw_vif_write_addr` + der STATION-Zweig von
`rtw_ops_add_interface` · `rtw_get_mgmt_rate` · `rtw_tx_mgmt_pkt_info_update`
· `rtw_tx_pkt_info_update` · `rtw_tx_queue_mapping` · `rtw_pci_tx_write`.
`rtw_pci_tx_write_data` nimmt sein `pkt_info` jetzt vom Rufer, wie in Linux —
bis 5a baute es sich selbst eins, weil es nur den H2C-Weg kannte.

**Das Gate ist die ANTWORT, nicht der verbrauchte Deskriptor.** Dass der
Chip einen Deskriptor abholt, sagt nur, dass DMA läuft; dass ein fremder AP
eine Probe Response an unsere Adresse schickt, sagt, dass der Rahmen die
Antenne verlassen hat und richtig gebaut war.

**Warum der Port zuerst kommt:** ohne `rtw_vif_port_config` steht in
`0x0610` keine Adresse, und `BIT_APM` im RCR lässt dann nur Broadcast
durch. Eine Probe Response ist an UNS gerichtet — sie käme nie an, und das
sieht am Gerät genauso aus wie „der AP hat nicht geantwortet".

**Kalibriert wird hier bewusst nicht, und das ist Linux' Entscheidung.**
`rtw_set_channel` setzt am Ende nur `need_rfk = true`; GAPK, IQK und DPK
laufen in `rtw_chip_prepare_tx`, das mac80211 aus `mgd_prepare_tx` ruft —
also VOR dem Anmelden, nicht beim Kanalwechsel. Linux' Kommentar nennt den
Grund: während eines Scans auf jedem Kanal zu kalibrieren dauert zu lange.
Ein Probe Request geht dort genauso unkalibriert hinaus wie hier. **Damit
hat `rtw8822c_phy_calibration` seinen Platz: Stufe 5c, vor dem Auth.**

**Zwei weitere Werkzeuglücken geschlossen**, beide derselben Art wie die aus
5a: `check_regs.py` liest jetzt die Tabelle `rtw_vif_port[]` aus
`mac80211.c` — die acht Portadressen sind Struct-Felder ohne `#define` und
waren damit unsichtbar; ein falsches `PORT0_MAC_ADDR` sieht am Gerät aus wie
„der AP antwortet nicht". Und `seqdiff.py` fand den Rumpf von
`rtw_tx_queue_mapping` nicht, weil sein Rückgabetyp aus ZWEI Wörtern
besteht (`enum rtw_tx_queue_type`) — die Funktion wurde übersprungen statt
geprüft. **133 von 133 Funktionen, 0 ohne C-Rumpf.**

**Offen und benannt:** `r.rp` der Sendequeues steht still — in Linux zieht
`rtw_pci_tx_isr` ihn nach. Bei drei Rahmen in einem Ring von 128 folgenlos,
bei einem laufenden Sender der nächste Posten.

### Stufe 5c — der Suchlauf (0.16.0)

Florians Befund nach 5b: „meiner war nicht drunter obwohl der eig. das
beste signal habe müsste". Die Antwort brauchte keine Untersuchung: **wir
hörten auf genau einem Kanal.** Stufe 4c setzt Kanal 1 und nie wieder einen
anderen; alles, was auf 6 oder 11 oder auf 5 GHz funkt, war nie in der Luft,
die wir gehört haben. Der Beweis stand im Log selbst — `yff-25919-5Ghz`
antwortete auf 2412 MHz, also das 2,4-GHz-Funkteil eines Doppelband-APs.

Gebaut: `rtw_set_channel` für JEDEN Kanal (bis hier war er auf 1 genagelt),
`rtw_core_scan_start`/`_complete`, `rtw_fw_scan_notify` (H2C 0x59, gegated
auf `FW_FEATURE_NOTIFY_SCAN` aus dem Firmware-Kopf), die Elementeauswertung
von Beacon und Probe Response, und eine Liste gefundener Zellen.

**Aktiv auf 2,4 GHz, passiv auf 5 GHz.** Der Unterschied ist keine
Bequemlichkeit: auf welchen 5-GHz-Kanälen gesendet werden DARF, entscheidet
die Zulassungszone, und diese Regeln gehören der oberen Hälfte
(`wifid`/cfg80211). Empfangen ist überall erlaubt, also hört der Suchlauf
dort, wo er nicht fragen darf.

**Das Gate misst UNS, nicht die Nachbarschaft.** Ob auf einem Kanal jemand
funkt, entscheidet nicht der Treiber; ob der Chip den Kanal angenommen hat,
schon. Also: RF 0x18 wird nach JEDEM Kanalwechsel auf beiden Pfaden
zurückgelesen, und das Gate ist 38 von 38.

**Und der Kanalfeger fand einen echten Absturz, bevor das Gerät ihn fand.**
`txpwrcheck.py` rechnet jetzt jeden Kanal × Bandbreite × Zone einmal durch
(1512 Kombinationen) — die 5-GHz-Pfade waren 1:1 portiert, aber nie
gelaufen, und ein Indexfehler wäre dort kein Fehlwert, sondern ein Trap.
Er trat sofort auf: **`rtw_5g_ht_ns_pwr_idx_diff` ist EIN Byte, das
2G-Gegenstück `rtw_2g_ns_pwr_idx_diff` sind ZWEI** — ich hatte den 2G-Schritt
kopiert. Damit lasen `g5_ns_bw20/bw40` die Bytes 33/35/37 statt 33/34/35,
und `g5_vht_bw80` griff auf Byte 42 eines 42-Byte-Blocks. Dazu stand `bw80`
im falschen Nibble (in `rtw_5g_vht_ns_pwr_idx_diff` liegt `bw160` unten).
Der Bauplan steht jetzt als eigene Prüfung da: eine Probe mit genau einem
gesetzten Nibble je Zugriff, **16 von 16**.

**Dabei verschluckte der Prüfstand selbst die Panik**: er schrieb stderr nur,
wenn stdout LEER war — also gerade dann nicht, wenn es mitten im Lauf kracht.
Dieselbe Klasse wie die zwei Werkzeuglücken aus 5a und 5b.

### Stufe 5d — die RF-Kalibrierung (0.17.0)

`rtw8822c_phy_calibration` ganz: `rfk_power_save` · `rfk_handshake` ·
**TXGAPK** (17 Funktionen, rtw8822c.c:1191-1823) · **IQK** (die rechnet die
Firmware; der Treiber stösst sie mit H2C-Paket 0x0E an und wartet auf
`REG_RPT_CIP == 0xaa`) · **DPK** (42 Funktionen, rtw8822c.c:3171-4186).
Dazu `rtw_fw_inform_rfk_status` und `rtw_fw_do_iqk`.

**Wann sie läuft, entscheidet Linux und nicht wir.** `rtw_set_channel` setzt
nur `need_rfk = true`; ausgeführt wird sie in `rtw_chip_prepare_tx`, das
mac80211 aus `mgd_prepare_tx` ruft — also nach dem Suchlauf und VOR dem
Anmelden. Der Kommentar in `main.c` nennt den Grund: während eines Scans auf
jedem Kanal zu kalibrieren dauert zu lange.

**Zwei Ausstiege sind echte Zweige, keine Auslassung.** `do_gapk` prüft
`dm_flags & BIT(RTW_DM_CAP_TXGAPK)` — und die Prüfung ist UMGEKEHRT: Bit
gesetzt heisst abgeschaltet; `dm_flags` wird nur aus debugfs beschrieben und
ist beim Start null. `txgapk` selbst kehrt bei `power_track_type` 4..7 um,
weil der Chip dann über TSSI regelt. `do_dpk` hängt an `is_dpk_pwr_on`, und
das setzt `rtw_load_rfk_table` — die Reihenfolge ist der Grund, nicht ein
Sonderfall.

**Das vierte Gate ist das wichtigste:** hört der Empfänger danach noch?
Dieselbe Messung wie in 4c, damit die Zahlen vergleichbar sind. Eine
Kalibrierung, die den Empfang kaputtmacht, ist schlimmer als keine.

**Die Werkzeuge haben diesmal mehr gefunden als der Code.** Neu ist
`gen_regs.py`: Namen hinein, fertige Rust-Konstanten mit Quellenangabe
heraus. **131 Register für diese Stufe, kein einziger von Hand getippt.**
Dazu vier Lücken geschlossen, alle derselben Art — der Prüfer sah etwas
nicht und meldete deshalb Übereinstimmung:

* **Aufzählungen ohne geschriebene Werte** (`enum rtw_rf_band { RF_BAND_2G_CCK,
  … }`) waren für `check_regs.py` unsichtbar. Jetzt zählt es selbst hoch —
  und fand sofort **`COEX_SWITCH_TO_MAX` als 7 statt 5**.
* **`read_poll_timeout(rtw_read32_mask, …)`** versteckt einen Registerzugriff
  als Makro-Argument; `seqdiff.py` sah ihn nicht und meldete für fünf
  Funktionen zu wenige Zugriffe.
* **`GENMASK(27,16)` gegen `0x0fff0000`** — dieselbe Zahl, zwei
  Schreibweisen. Der Zahlenvergleich löst beide jetzt auf, was vier
  DPK-Funktionen grün machte und zugleich **vier Stellen aufdeckte, die nur
  durch beidseitige Abwesenheit grün waren**.
* **Die DPK-Tabellen sind TRIPEL** (Adresse, Maske, Wert) und nicht Paare wie
  alle anderen. Ein Paar-Leser hätte sie still falsch geschrieben;
  `gen_tables.py` bricht jetzt ab, statt zu raten.

**204 von 204 Funktionen Zugriff für Zugriff gleich · 845 Konstanten, 0
Abweichungen · Abdeckung 299 → 374 von 940, `rtw8822c.c` 141/171.**

### Stufe 5d am Gerät: ✅ GRÜN — der Sender ist kalibriert (0.17.1)

**Die Kalibrierung läuft, und die Zahlen sind Linux' Zahlen.**
`power_track_type 0` (kein TSSI, TXGAPK rechnet wirklich) · TXGAPK
**14 ms**, Versätze Pfad A `0,1,0,0,1,1,-6,-6,-6,-6` · IQK `RPT_CIP 0xaa`
nach 20 ms · **DPK beide Pfade, `gs 94/93`, `txagc 15/16`, `coef1 fertig`,
105 ms** · **gesamt 139 ms**. Danach CCA 178 und **14 CCK-Pakete mit
gültiger Prüfsumme bei null Fehlern** — der Empfänger hört unverändert.

**Der Postfach-Fehler war unserer, und der Log beweist es Zeile für
Zeile.** Mit EINEM `H2cState` (wie `rtwdev->h2c`) gehen 4a's Coex-Kommandos
in die Fächer 0 und 1, 5c's `scan_notify` in **2 und 3** — `HMETFR 0x04`
und `0x08` zeigen genau das gerade beschriebene Fach im Flug —, und bis 5d
beginnt, steht `HMETFR 0x00`: die Firmware hat alles geleert.

**Und damit ist die eigentliche Regel benannt: die Reihe der vier
Postfächer ist ein PROTOKOLL, keine Buchführung.** Die Firmware läuft
denselben Ring mit und erwartet das NÄCHSTE Fach. Springt der Treiber
zurück — wie unsere drei getrennten Zustände es taten, als 5d wieder bei
Fach 0 anfing —, wartet die Firmware auf ein Fach, das nie kommt, und das
zurückgesprungene bleibt für immer „voll". Deshalb hat Linux genau ein
`last_box_num` für das Gerät.

Zwei Nebenbefunde aus demselben Lauf:

* **TXGAPK fiel von 219 ms auf 14 ms.** Die 200 ms waren zweimal
  `wait_rfk_ack` im Leerlauf; jetzt kommen die Quittungen in **542 µs und
  2 µs**. Ein Fehlschlag kostet nicht nur das Ergebnis, er kostet Zeit, und
  beides sah vorher nach „so lange dauert die Kalibrierung" aus.
* **Der geleerte Empfangsring gab genau eine Nachricht her: `c2h id 0x38`
  = `C2H_SCAN_RESULT`** — die Antwort, auf die
  `rtw_core_fw_scan_notify(false)` in Linux ausdrücklich wartet. Die
  Firmware hatte geantwortet; es hörte nur niemand zu.

**Offen und benannt:** C2H wird gezählt und gemeldet, aber nicht
ausgewertet (`rtw_fw_c2h_cmd_handle` fehlt ganz) · `r.rp` der Sendequeues
steht weiter still (`rtw_pci_tx_isr`).

### Stufe 5e — Auth und Assoc (0.18.0)

Der Suchlauf gibt jetzt sein Ziel heraus: das **stärkste Netz auf 2,4 GHz**
mit BSSID, Kanal, Fähigkeitsfeld und dem rohen RSN-Element. Danach in
Linux' Reihenfolge: Kanal des Ziels setzen → `rtw_chip_prepare_tx` (die
Kalibrierung aus 5d, denn `rtw_set_channel` hat `need_rfk` gesetzt) →
`PORT_SET_BSSID` → Auth → Assoc → bei Erfolg `RTW_NET_MGD_LINKED` mit der
AID in den Port und `rtw_fw_media_status_report`.

**Kalibriert wird auf dem ZIELkanal, nicht auf dem des Suchlaufs.** Das ist
der Grund, warum `rtw_chip_prepare_tx` in Linux hinter dem Kanalwechsel
steht und nicht davor.

**Ein Antrag wählt, ein Beacon zählt auf.** Das RSN-Element des AP nennt
alle Verfahren, die er kann; unseres muss genau EINES nennen, sonst lehnt
er mit Status 43 ab. Gewählt wird CCMP als Paarschlüssel und PSK als
Authentifizierung; die Gruppenchiffre wird übernommen, denn die bestimmt
der AP allein.

Dazu `rtw_pci_tx_isr` (der Lesezeiger `r.rp` stand seit 5b still — bei drei
Rahmen folgenlos, bei einem laufenden Sender nach 127), `rtw_fw_media_status_report`
und der C2H-Verteiler: **jede Nachricht der Firmware wird jetzt mit ihrem
NAMEN gemeldet** statt gezählt.

**Namentlich nicht gebaut, und jedes ist ein eigener Posten:**
`rtw_update_sta_info` + `rtw_fw_send_ra_info` (die Ratenanpassung braucht
die HT/VHT-Fähigkeiten des Gegenübers aus der Anmeldeantwort — ein
Elementeparser der oberen Hälfte) · `rtw_fw_download_rsvd_page` +
`rtw_send_rsvd_page_h2c` (der WEG steht seit Stufe 2, der INHALT ist obere
Hälfte) · `rtw_fw_default_port` · `rtw_coex_media_status_notify` ·
`rtw_bf_assoc` · `rtw_set_ampdu_factor` · `rtw_fw_beacon_filter_config`.

**Das Tor steht bewusst VOR dem Vierwegehandschlag.** Eine Anmeldung ohne
ihn endet nach wenigen Sekunden in einem Deauth — das ist erwartet und kein
Fehler. Ab da ist es `wifid`.

### 0.18.1 — ein Tor, das die Nachbarschaft mass, stand in 5b

Ein Lauf von 0.18.0 blieb in Stufe 5b stehen: drei Deskriptoren in 4 µs
abgeholt, **null Probe Responses**, und damit hingen 5c bis 5e. Der
Sendeweg war dabei byte-gleich zu 0.17.1 (`tx.rs` unverändert, `pci.rs`
nur um `tx_isr` reicher, das 5b gar nicht ruft) — die Luft war leiser: 21
statt 62 Pakete, Beacons bei −87…−96 statt −84…−90 dBm.

**Das ist dieselbe Klasse, die 5c schon gelöst hatte, und ich hatte sie in
5b stehen lassen.** Dass ein Rahmen die Antenne verlässt, beweist nur eine
ANTWORT — aber ob auf EINEM Kanal gerade jemand antwortet, ist ein
Münzwurf. Dreimal gewonnen, einmal verloren, und ein fremder AP hielt die
ganze Kette an.

Jetzt: **5b behält die Tore, die UNS messen** (Adresse im Port, richtige
Queue, Deskriptoren abgeholt) und meldet die Antworten als Befund; **das
Tor „ein fremder AP antwortet" steht in 5c**, über dreizehn aktive Kanäle
gezählt. Das ist kein weicheres Tor, sondern ein belastbareres: in jedem
bisherigen 5c-Lauf kamen Antworten von mehreren Zellen.

### Stufe 5e am Gerät: ✅ GRÜN — der AP hat uns angenommen (0.18.1)

```
Auth:  Antwort nach Versuch 1, Status 0 (angenommen)
Assoc: Antwort nach Versuch 1, Status 0 (angenommen)   AID 1
Port: net_type MGD_LINKED, AID gesetzt · media_status_report raus
```

**Ziel war `IvyPie_New` auf K7, −49 dBm, Fähigkeiten `0x1431`, RSN 22
Bytes** — also WPA2 (Privacy-Bit gesetzt). Beide Schritte antworteten beim
ERSTEN Versuch; bei −49 dBm braucht es keine Wiederholung.

**Die RSN-Wahl stimmte auf Anhieb.** Ein falsches Paarschlüssel-Verfahren
hätte Status 43 gegeben, ein falsches AKM Status 44 — Status 0 heisst, dass
CCMP/PSK aus dem Element des AP richtig gewählt und richtig gebaut war.
Dass das Fähigkeitsfeld echot wird (`ESS | (AP & 0x0030)` = `0x0031`),
gehört dazu: wer mehr behauptet, als der AP kann, wird abgelehnt.

**Und 0.18.1 hat sich sofort bewährt:** 5b ist grün über seine mechanischen
Tore, und das Tor „ein fremder AP antwortet" meldete in 5c **4 Antworten
über 13 Kanäle** — bei genau einer davon auf Kanal 1. Auf einem Kanal wäre
es wieder ein Münzwurf gewesen.

**Was diese Stufe NICHT behauptet:** die Verbindung trägt noch keine Daten.
Ohne Vierwegehandschlag wirft der AP uns nach wenigen Sekunden wieder
hinaus, und der Chip wird am Ende des Laufs ohnehin abgeschaltet.
Angemeldet ist nicht verbunden — das Tor stand bewusst genau hier.

**Stand: Stufen 0 bis 5e am Gerät grün.** Die ganze Kette von PCI bis zur
Anmeldung an einer echten Funkzelle steht, 1:1 aus rtw88 portiert, 207 von
207 Funktionen Zugriff für Zugriff gleich.

### Stufe 5f — die Ratenanpassung (0.19.0)

Der Treiber schickt der Firmware **keine Rate, sondern eine MASKE**: welche
der 64 Raten dieses Gegenüber kann. Die Firmware wählt daraus laufend und
meldet ihre Wahl als `C2H_RA_RPT` zurück — **und genau das ist das Tor.**
Eine Maske, die niemand beantwortet, ist eine Behauptung.

Gebaut: `rtw_update_sta_info` · `get_vht_ra_mask` · `rtw_rate_mask_rssi` ·
`rtw_rate_mask_recover` · `get_rate_id` · `rtw_fw_send_ra_info` ·
`rtw_fw_default_port` · und der Parser, der die Fähigkeiten des AP aus der
Anmeldeantwort liest (HT-, VHT-Element, Raten). In Linux baut mac80211
daraus `ieee80211_sta`; hier steht er in `sta.rs` und gehört später `wifid`.

**Ein echter Fund beim Gegenlesen: `SET_RA_INFO_VHT_EN` ist
`GENMASK(29,28)`, zwei Bit** — ich hatte ein einzelnes geschrieben. Bei
einem `bool` schreibt das denselben Wert und löscht Bit 29 nicht; hier
folgenlos, weil der Puffer bei null beginnt, und trotzdem falsch.

**Die Werkzeuge haben wieder vier Lücken geschlossen**, alle derselben Art:

* **`include/linux/ieee80211.h` liegt ausserhalb des Treiberbaums.** Die
  Bits, mit denen rtw88 die Fähigkeiten des Gegenübers liest (HT/VHT),
  wären sonst von Hand getippt und ungeprüft gewesen.
* **`RA_MASK_*` steht in `main.c`, nicht in einem Header.** Der Generator
  las es, der Prüfer nicht — **zwei Werkzeuge mit verschiedenen Quellen
  lassen genau dort eine Lücke, wo der Generator am meisten hilft.** Jetzt
  lesen beide dieselben Dateien.
* **Ein `/* … */` hinter einem Aufzählungseintrag frass den Namen des
  nächsten.** So verschwanden `WLAN_EID_RSN` und `WLAN_EID_DS_PARAMS`
  lautlos aus beiden Werkzeugen.
* **Masken breiter als 32 Bit** (`0x3ff000ULL << 20`) und C-Zahlensuffixe:
  als `u32` wären sie still abgeschnitten worden.

**Namentlich nicht gebaut:** `rtw_fw_download_rsvd_page` +
`rtw_send_rsvd_page_h2c`. Die reservierten Seiten tragen PS-Poll-, Null-
und QoS-Null-Rahmen, die die FIRMWARE im Stromsparbetrieb selbst sendet.
Stromsparen gibt es hier nicht, also würden die Seiten geschrieben und nie
gelesen — sie gehören zu LPS, nicht hierher.

**214 von 214 Funktionen · 917 Konstanten, 0 Abweichungen · Abdeckung 389
→ 394 von 940.**

### 0.19.1 — der Anmeldeantrag bot kein HT an, also bekamen wir keins

Stufe 5f war am Gerät grün, und **eine Zahl darin war trotzdem falsch**:
`Gegenueber: HT nein · VHT nein`. Bei einem AP mit 5-GHz-Zwilling auf K100
kann das nicht stimmen. Die Folge stand zwei Zeilen tiefer — `ra_mask
0x0ff5`, nur Legacy-Bits, und die Firmware wählte **OFDM 54M als Bestes,
das sie DURFTE**.

**Der AP hat uns korrekt das geantwortet, was wir gefragt haben.** Unser
Anmeldeantrag trug SSID, Raten und RSN — **kein HT-Element**. Eine Station,
die kein HT anbietet, wird als Legacy-Station angenommen, und dann lässt
der AP HT auch in seiner Antwort weg. Das war kein Parserfehler; der Parser
las korrekt, dass nichts da war.

Gebaut: `rtw_init_ht_cap` und `rtw_init_vht_cap` als fertige Elemente
(id 45 / id 191), aus unseren eigenen Werten — `hw_cap.nss`, `hw_cap.bw`,
`hw_cap.ptcl` aus der efuse, `rx_ldpc`/`tx_stbc` des 8822C,
`bfee_sts_cap = 3`. **VHT nur auf 5 GHz**: auf 2,4 GHz ist es nicht
zugelassen, und ein AP darf einen Antrag mit VHT im falschen Band ablehnen.

**Das ist die Klasse „stillschweigend weggelassen", und sie kostete genau
eine Stufe Verzögerung.** Ich hatte den Anmeldeantrag auf das Minimum
gebaut, das durchgeht — das Tor von 5e war „Status 0", und das kam. Was
fehlte, zeigte sich erst als Deckel bei 54 Mbit in 5f.

### Stufe 6a — der Steuerkanal, der Handschlag und der Datenweg (0.20.0)

**Hier hört der Stufentest auf und der Treiber fängt an.** Bis 5f arbeitete
`main` eine Kette ab; 6a ist eine Schleife: Empfangsring leeren,
Sendequittungen einsammeln, Kommandos von `wifid` ausführen, Ereignisse
hinaufmelden.

**Den Handschlag rechnet `wifid`, nicht wir.** Er ist herstellerunabhängig
und steht einmal da (`tools/wasm/wifid/core/src/eapol.rs`) —
`docs/spec/WIFI_CLASS_ABI.md` §1 sagt es klar: *der Treiber sieht nie den
PSK*. Wir transportieren die Rahmen und schreiben die fertigen Schlüssel in
den Speicher.

Gebaut: `rtw_sec_write_cam` + `rtw_sec_clear_cam` (acht Worte je Platz,
**rückwärts** geschrieben — Wort 0 trägt das Gültig-Bit und geht zuletzt
hinaus) · `rtw_tx_data_pkt_info_update` · `get_highest_ht_tx_rate` ·
802.3↔802.11 in beide Richtungen mit LLC/SNAP · die Demux-Regel aus §2b
(Ethertyp 0x888E → `EV_EAPOL_RX` an `wifid`, sonst `npk_netdev_submit_rx`)
· `EV_READY`/`EV_LINK_UP` hinauf, `TX_EAPOL`/`SET_KEY`/`AUTHORIZED` herunter.

**Drei Verträge, die keine ABI ausdrückt, und alle drei hätten still
versagt:**

* **Unser RSN-Element ist jetzt BYTE-GLEICH mit dem in `wifid`.** Der
  Vierwegehandschlag rechnet seinen MIC über genau das Element, das die
  Station im Anmeldeantrag geschickt hat. Ich kopierte vorher die
  Gruppenchiffre des AP — bei einem reinen CCMP-AP dasselbe, bei einem
  Misch-AP nicht, und dann verwirft der AP msg2, ohne zu sagen warum.
* **Das Ziel kommt aus `sys/config/wifi_ssid`**, nicht aus der Lautstärke.
  Die Spec sagt warum: ohne SSID-Filter nimmt der Treiber den lautesten AP
  *irgendeines* Netzes, auch den des Nachbarn — für den `wifid` keinen PSK
  hat, und das endet im stillen MIC-Fehlschlag.
* **Gewöhnliche Datenrahmen, kein QoS.** Unser Anmeldeantrag trägt kein
  WMM-Element, also hat der AP uns als Nicht-QoS-Station angenommen. Wer
  WMM nicht anbietet, darf kein QoS senden.

**Dazu ein neuer Prüfer:** `check_regs.py` vergleicht die
Steuerkanal-Konstanten (`CMD_*`, `EV_*`, `DOT11_*`) gegen **`wifi_ax200`** —
zwei Treiber, eine ABI. Weichen sie voneinander ab, redet der Manager mit
einem von ihnen falsch, und niemand merkt es. 13 Konstanten, 0 Abweichungen.

**Voraussetzung am Gerät:** `wifid` muss laufen und seine Zugangsdaten
haben — `store /sys/config/wifi_ssid <name>` und
`store /sys/config/wifi_psk <pass>`.

**Was 6a NICHT behauptet:** der Chip wird am Ende des Laufs weiter
abgeschaltet. Auch ein gelungener Handschlag endet mit dem Stufentest — ein
Treiber, der die Verbindung HÄLT, ist 6b.

**220 von 220 Funktionen · 960 Konstanten, 0 Abweichungen · Abdeckung 396
→ 397 von 940.**

### 0.21.0 — msg1 ging durch, msg3 nicht: vier Bytes zu viel

Der erste Lauf von 6a: **der Steuerkanal trägt Ende zu Ende.** `EV_READY`
hinaus, `[wifid] READY — supplicant armed`, msg1 vom AP herein, msg2
hinaus. Dann viermal `4-way FAILED (bad MIC / unwrap)`.

**Die Stelle im Code sagt, was das heisst.** `Step::Fail` kommt aus dem
**msg3**-Zweig — msg1 trägt gar keinen MIC (`ki & KI_MIC == 0` → immer
`Step::Reply`). Der AP hat unser msg2 also **angenommen**: der PMK stimmt,
der Sendeweg trägt, der Anmeldeantrag war richtig. Es scheiterte daran,
dass `wifid` msg3 nicht verifizieren konnte.

**`compute_mic` rechnet über `frame.len()`** — die ganze Scheibe, die der
Treiber übergibt. Und `WLAN_RCR_CFG = 0xE400220E` hat **APP_FCS (31),
APP_MIC (30) und APP_ICV (29) gesetzt**, also liefert der Deskriptor mehr
Bytes, als der Rahmen lang ist. Vier davon sind die Prüfsumme.

Der EAPOL-Rahmen wird jetzt auf seine **angesagte** Länge gekürzt: der
802.1X-Kopf trägt sie in den Bytes 2..4 (gross-endig), der ganze Rahmen ist
`4 + diese Zahl`. Das ist durch den Rahmen selbst bestimmt und nicht
geraten — und es meldet zugleich, **wie viele** Bytes zu viel kamen, damit
der nächste Lauf die Vermutung „vier, die Prüfsumme" bestätigt oder
widerlegt.

**Offen und bewusst noch nicht angefasst:** derselbe Überhang geht auch an
`npk_netdev_submit_rx`. Bei IP ist er harmlos (der IP-Kopf trägt seine
eigene Länge), aber er ist falsch. Sobald die Messung die Zahl nennt, wird
er dort ebenso abgeschnitten — vorher wäre es geraten.

### Stufe 6a am Gerät: ✅ der Handschlag ist durch (0.22.0)

```
EAPOL: 103 Bytes geliefert, 99 angesagt (4 zu viel)
[wifid] 4-way: sending msg2
EAPOL: 159 Bytes geliefert, 155 angesagt (4 zu viel)
[wifid] 4-way: msg3 OK — sending msg4 + installing keys
[wifid] *** 4-way complete — AUTHORIZED ***
[npk] net: link usb-lan (up) -> wifi (up)
```

**Die Messung bestätigt die Herleitung auf den Punkt: vier Bytes, die
Prüfsumme.** Und `wifid` fährt den Vierwegehandschlag über unseren
Steuerkanal durch, ohne eine Zeile Verschlüsselung im Treiber.

**Aber DHCP bekam keine Antwort** — und das bei 20 gesendeten Datenrahmen.
Zwei Fehler, beide in der VERSCHLÜSSELUNG, und deshalb lief der Handschlag
(unverschlüsselt) durch und alles danach nicht:

* **Senden: der CCMP-Kopf fehlte.** `rtw_ops_set_key` setzt
  `IEEE80211_KEY_FLAG_GENERATE_IV`, und das heisst in mac80211: der Stapel
  macht acht Byte Platz und schreibt die Paketnummer hinein
  (`ccmp_pn2hdr`), die Hardware verschlüsselt nur. Wir schrieben ihn nicht.
* **Empfangen: der CCMP-Kopf wurde nicht übersprungen.** Die Hardware
  entfernt ihn NICHT — `rtw_rx_fill_rx_status` setzt `RX_FLAG_DECRYPTED`,
  aber nicht `RX_FLAG_IV_STRIPPED`; in Linux räumt mac80211 ihn weg. Unser
  LLC/SNAP-Vergleich griff acht Byte zu früh und verwarf **jeden**
  verschlüsselten Rahmen.

Dazu wird hinten jetzt abgeschnitten, was nicht dazugehört: Prüfsumme
(immer) und bei CCMP der 8-Byte-MIC — beides, weil `WLAN_RCR_CFG` APP_FCS
und APP_MIC gesetzt hat.

**Und wie beim AX200 gibt es einen Rückfallpfad mit MELDUNG:** sitzt
LLC/SNAP nicht an der gerechneten Stelle, wird an den zwei anderen
möglichen gesucht und gesagt, wo es stand. Ein stiller Fehlgriff dort
verwirft jeden Rahmen und sieht aus wie eine tote Leitung.

### Stufe 6b — der Treiber bleibt stehen (0.23.0)

Florians Lauf von 0.22.0: **DHCP lief durch, er hatte die Adresse** — und
Sekunden später war sie weg. Das war keine Überraschung, sondern die
Grenze, die 6a selbst benannt hatte: nach acht Sekunden endete die Stufe,
`mac_power_off` schaltete den Chip ab, und der Kernel fiel auf `usb-lan`
zurück.

**Ein Treiber, der seine Arbeit beendet, ist kein Treiber.** Dieselbe
Schleife läuft jetzt ohne Frist (`frist_us == 0`), und die Zusammenfassung
der Stufen steht DAVOR — wer nie zurückkehrt, kann sie hinterher nicht
mehr drucken. `mac_power_off` gilt nur noch für den Fall, dass eine Stufe
nicht steht: dann kehrt der Treiber zurück, und die laufende Firmware darf
nicht mehr in Puffer schreiben, die der Kernel gleich freigibt.

Dazu zwei Dinge, die ein stehender Treiber braucht und ein Stufentest nicht:

* **`npk_driver_report` je Sekunde** (Spec §3). Die Luft ist für den Kernel
  unsichtbar: Kanal, ausgehandelte Rate, Bandbreite, BSSID, Rahmenzähler,
  Schlüsselzustand stehen nirgends sonst. Ohne sie ist eine Leitung, die
  wegen einer Legacy-Rate langsam ist, nicht von einer zu unterscheiden,
  die wegen voller Schlangen langsam ist. `wlan` druckt den Block neben der
  Kernelsicht.
* **Ein RX-Wachhund.** Auf einer lebenden Zelle kommt immer etwas — Beacons
  allein sind zehn je Sekunde. Fünf Sekunden völliges Schweigen heissen,
  dass der Ring steht, nicht dass die Luft leer ist; gemeldet werden dann
  Ringzeiger und `rx_tag`, die vier ersten Male.

**Offen und benannt:** ein TX-Wachhund (der Sendering kann genauso
steckenbleiben) · Wiederverbinden nach einem Deauth (`EV_LINK_DOWN` und ein
neuer Durchlauf ab 5e) · LPS mit den reservierten Seiten · die laufende
Koexistenz.

### Benannt und noch nicht gebaut: die Ausgabe muss leise werden

Florian, nachdem der Treiber im Autostart lief: *„der ganze debug phase 0-6
spammt beim booten … das system voll."* Stimmt — ein Treiber, der bei jedem
Boot sechs Stufen mit Gates ausdruckt, macht die Konsole für alles andere
unbrauchbar.

**Form:** derselbe Schalter, den `wifi_ax200` schon benutzt — ein Schlüssel
in `sys/config/wifi`. `debug: 1` schaltet die Stufenkette ein; ohne ihn
bleibt sie still.

**Still heisst nicht stumm.** Eine Zeile, wenn es steht, und jede Zeile,
wenn etwas nicht steht:

    [rtl8822ce] verbunden: "IvyPie_New" K7 -49 dBm · HT MCS7 40 MHz · AID 3

**Die Tore bleiben.** Sie sind der Grund, warum sechs Stufen entstanden
sind, ohne im Dunkeln zu suchen — sie gehören hinter den Schalter, nicht in
den Müll. Und der Treiberbericht (`npk_driver_report`, je Sekunde) ist
davon unberührt: der geht an `wlan` und nicht auf die Konsole.

### 0.23.1 — 6b startete 6a neu, statt sie fortzusetzen

Florians Lauf ohne LAN-Dongle: **die Verbindung stand** — `carrier UP`,
`address 192.168.178.172`, Handschlag komplett, `AUTHORIZED`,
`link up connected`. Und dann kein Ping, kein DNS, nichts.

Der Treiberbericht sagte, warum:

```
[wifid] READY   supplicant armed for 4-way     ← ein ZWEITES Mal
rtl8822ce NICHT verbunden ... schluessel 0
tx queue enq 126  deq 4   backlog 6176 B
```

**Stufe 6b rief `stage6a_link` ein zweites Mal.** Die Funktion legte einen
frischen `Link` an (`ptk_installed: false`, Zähler auf null,
`authorized = false`) und schickte **`EV_READY` erneut**. `wifid` machte
daraufhin einen neuen Supplicant scharf und wartete auf ein msg1, das der
AP nie wieder schickt. Weil der Sendeweg hinter `if authorized` hängt, nahm
der Treiber seither nichts mehr aus der Kernelschlange — daher
`enq 126, deq 4`.

**Eine Stufe, die ihren Vorgänger neu startet statt fortzusetzen, ist keine
Fortsetzung.** Jetzt sind Aufbau und Schleife getrennt: `link_setup`
meldet beim Kernel an und macht `wifid` scharf — **genau einmal** —, und
`link_pump` läuft mit Link und Zählern von aussen. 6a ruft es mit acht
Sekunden Frist, 6b mit keiner. Die Zähler gehören dem LINK, nicht der
Stufe: einer, der beim Übergang auf null springt, ist eine Lüge über die
Leitung.

### ✅ NETZ. Stufen 0 bis 6b am Gerät grün (0.23.1)

```
wlan       registered, carrier UP
tx queue   enq 29  deq 29  backlog 0 B
rtl8822ce  verbunden  kanal 7  rate 0x1b  bw 1
daten rein/raus 27/27  eapol 2/2  schluessel 2  rx-wachhund 0

[npk] google.ch -> 172.217.208.94
[npk] 4 sent, 4 received, 0% lost   rtt min/avg/max = 10/10/10 ms
```

**Der Fix von 0.23.1 ist in genau der Zeile bewiesen, die vorher das
Gegenteil sagte:** `tx queue` stand bei `enq 126 deq 4 backlog 6176 B` und
steht jetzt bei `enq 29 deq 29 backlog 0`. Der Treiber holt ab, was der
IP-Stapel loswerden will. Dazu: EIN `READY` im Supplicant-Log statt zwei,
`schluessel 2` statt 0, und `ctrl chan driver→wifid` 4 statt 9 Nachrichten.

**Damit ist die Kette vollständig** — PCI, Power, Ringe, DMA, Firmware,
efuse, Sendeleistung, MAC-Init, Tabellen, BB/RF, Coex-Antenne, Kanal,
Empfang, Senden, Suchlauf, RF-Kalibrierung, Auth, Assoc, Ratenanpassung
(MCS15 auf 40 MHz), Vierwegehandschlag, Schlüssel, DHCP, ARP, DNS, ICMP.
**220 von 220 Funktionen Zugriff für Zugriff wie Linux, 960 Konstanten
ohne eine Abweichung.**

**Was das NICHT beweist**, und deshalb steht es hier: Durchsatz unter Last ·
Stabilität über Stunden · Wiederverbinden nach einem Deauth · Verhalten bei
einem Gruppen-Neuschlüssel. Das sind Messungen, keine Gates.

### 0.24.0 — der Schalter und die zwei Augen (Posten 1-3 gebaut)

**Was es aendert, in einem Satz je Posten.**

**(1) `debug: 1` in `sys/config/wifi`.** Gebaut nicht als `if verbose` um
Bloecke herum, sondern IN `host::print` — wer die Bloecke wegschaltet,
schaltet auch die Registerzugriffe weg, die als Argumente drinstehen, und
aendert damit, was der Treiber TUT. So bleibt jeder Zugriff, nur die Bytes
gehen nicht auf die Leitung. Daneben `host::say` und die Klammer
`loud_begin`/`loud_end` — **die Klammer ist der Grund, warum es keine
zweite Garnitur Zahlenformatierer braucht**: `print_dec` ruft `print`, und
`print` sieht die Klammer.

Immer laut, auch ohne den Schalter: jedes **gefallene** Tor, jede
Abbruch- und Ueberspringzeile, die Panik, der RX-Wachhund, ein nicht
gesendetes EAPOL, der Rauswurf — und **die eine Zeile, wenn es steht**:

    [rtl8822ce] verbunden: "IvyPie_New" K7 -49 dBm · HT MCS8-15 (0x1b) · 40 MHz · AID 3

Sie steht hinter den Toren von 6a, nicht bei `AUTHORIZED`: erst dort ist
sie eine Aussage ueber eine VERBINDUNG und nicht ueber einen
Zwischenstand.

**Was das nebenbei aendert und hier benannt gehoert:** der Bringup wird
schneller, weil zehntausende Serienbytes wegfallen. Nachgesehen: **alle
Wartezeiten im Treiber haengen an `now_us()`**, keine an einer
Rundenzahl — die Stufen messen also dasselbe wie vorher.

**(2) Der Rauswurf ist sichtbar.** `disconnect_reason` steht direkt neben
`rx_to_8023` — dort, wo das Filter sitzt, das ihn verschluckte —, und die
Empfangsschleife fragt ihn VOR dem Datenfilter. Deauth `0xc0`, Disassoc
`0xa0`, beide nur von `addr2 == BSSID`; der Grundcode little-endian aus
24..26. **Gesehen im Rueckruf, gehandelt danach**: `netdev_set_link(false)`
und `EV_LINK_DOWN` gehoeren nicht in einen Rueckruf, der mitten im
Ringleeren laeuft und `link` nicht halten darf.

    [rtl8822ce] DEAUTH vom AP — Grund 16 (Gruppenschluessel-Handschlag: Zeitueberschreitung)
                Laufzeit 412 s · daten rein/raus 1834/902 · neuschluessel 3/3 · gtk 4

Gezaehlt wird jeder, **gedruckt die ersten drei** — ein AP schickt seinen
Rauswurf als Salve.

**(3) Der Neuschluessel wird gezaehlt.** Jedes EAPOL NACH `AUTHORIZED` ist
einer, jede Antwort darauf daneben, und die GTK-Einschreibungen ins CAM
als dritte Zahl. Der Bericht fuer `wlan` hat eine Zeile dazubekommen:

    neuschluessel 3 empfangen, 3 beantwortet  gtk 4  rauswurf 1 (zuletzt Grund 16: …)

**`publish_report` nimmt jetzt `&LinkStats`** statt acht Argumenten; das
`#[allow(clippy::too_many_arguments)]` faellt weg.

**Ein fuenfter Pruefer: `framecheck.py`.** `disconnect_reason` ist der
einzige Weg, auf dem der Treiber einen Rauswurf ueberhaupt SIEHT — greift
er daneben, gibt er `None`, und `None` ist genau der Zustand, aus dem wir
kommen. Ein Offset daneben ist hier keine falsche Zahl, sondern Schweigen,
und ein Geraetelauf waere verschenkt. 31 Faelle gegen von Hand gebaute
802.11-Rahmen, dazu `cfg_on` und `reason_name`. **Die Vorlage setzt addr3
absichtlich ANDERS als addr2 und SeqCtl absichtlich nicht null** — sonst
kann kein Test merken, ob die Funktion das richtige Feld liest.

Gegen fuenf absichtlich eingebaute Fehler geprueft, alle fuenf gefangen:
addr3 statt addr2 · Grundcode zwei Byte zu frueh · big-endian statt little
· Laengenpruefung zu kurz · die Konstante `DOT11_FC_DEAUTH` falsch (die
steht in KEINEM Linux-Header, `check_regs.py` sieht sie also nicht — hier
ist ihre einzige Kontrolle). Dazu prueft er die **Balance der
`loud_*`-Klammern**: eine offene macht den ganzen Posten 1 wirkungslos und
sieht aus wie ein Schalter, der nicht greift. Ausnahme ist `fn panic`,
danach kommt nichts mehr.

**Posten 4 (Wiederverbinden) ist bewusst NICHT gebaut.** Er steht unten
unveraendert. Ein Reconnect auf eine unbekannte Ursache ist geraten; die
zwei Zaehler und der Grundcode sagen im naechsten Lauf, worauf er zu
antworten hat.

**Was der naechste Geraetelauf beantwortet:**

* **Grund 15 oder 16** → es haengt am Handschlag bzw. am Neuschluessel,
  und Posten 4 muss ihn heilen, nicht nur neu verbinden.
* **`neuschluessel 3 empfangen, 0 beantwortet`** → es liegt an uns, und
  zwar im Weg `wifid` → `Step::Rekey` → `CMD_TX_EAPOL`.
* **`3/3` und trotzdem Rauswurf** → es liegt woanders, und der Grundcode
  sagt wo.
* **gar kein DEAUTH, und die Leitung wird trotzdem still** → dann ist es
  kein Rauswurf, sondern der Empfangsring, und der RX-Wachhund meldet
  sich als naechster.

**Am Geraet — und NICHT mit `store`.** `intent_store` ruft
`npkfs::upsert`, das das GANZE Objekt ersetzt, und der Text hinter dem
Namen ist eine Zeile ohne Zeilenumbrueche. `store /sys/config/wifi
debug: 1` wuerde also die `ssid:`-Zeile loeschen und die Verbindung
kappen. Die Datei wird mit dem Editor geaendert:

    spell /sys/config/wifi      → Zeile `debug: 1` dazu, speichern
    install wifi_rtl8822ce && driver wifi_rtl8822ce

(So sind auch die mehrzeiligen AX200-Konfigurationen mit `ampdu:`/`ps:`
entstanden — `store` kann nur den Einzeiler.)

### 0.24.1 — ein NEIN aus einem gescheiterten Lesezugriff ist keine Antwort

Aufgefallen beim Nachsehen vor der Freigabe: `read_debug_flag` gab `false`
zurueck, wenn `sys/config/wifi` **gar nicht lesbar** war — dasselbe Ergebnis
wie „die Zeile steht nicht drin". Und `wifid` dokumentiert fuer genau
dieses Objekt ein Rennen mit dem Rest des Bootvorgangs („On autostart this
races the rest of boot"). Ein `debug: 1`, das in dieses Rennen faellt,
haette ausgesehen wie ein Schalter, der nicht greift — und das kostet einen
ganzen Geraetelauf, statt den Fehler zu finden, fuer den der Schalter da
ist.

Der Rueckgabewert faehrt jetzt mit, und die eine stille Zeile sagt es:

    [rtl8822ce] v0.24.1 — still, und sys/config/wifi war beim Start nicht
                lesbar: ein `debug: 1` darin greift dann NICHT

Nicht gebaut: eine Warteschleife wie in `wifid`. Ein Treiber, der auf
seine Konfiguration wartet, verzoegert den Bringup fuer eine Ausgabefrage
— die Meldung reicht, denn sie nennt die Heilung.

### 0.25.0 — `rtw_watch_dog_work`: die laufende Haelfte des Treibers

**Der Befund, der diese Runde ausgeloest hat.** Florians Lauf: kein
Deauth, kein Wachhund, `tx queue enq 308 deq 308 backlog 0`, `tx drops
full 0` — **nichts lief ueber**. Dann, nach 708 s, vier Rekeys in Salve
und `DEAUTH Grund 16`. Die Zahlen sagen die Richtung eindeutig:

    Momentaufnahme:  daten rein/raus 136/308   neuschluessel 1/1
    beim Deauth:     daten rein/raus 140/360   neuschluessel 5/5   gtk 6

`rein` +4, `neuschluessel` +4: **jeder einzelne empfangene Datenrahmen im
Endfenster war ein Rekey-msg1.** Der AP hielt uns fuer da und sprach mit
uns, verschluesselter Unicast kam an und entschluesselte — der EMPFANG
lebte. Wir antworteten fuenfmal, der AP hoerte keine davon und sagte
`Grund 16` („ich habe gefragt und nie eine Antwort bekommen"). **Der
Deauth ist die Folge, nicht die Ursache.**

**Und die Ursache war kein Fehler, sondern eine Auslassung.** Gezaehlt
statt vermutet: `rtw_watch_dog_work` laeuft in Linux **alle zwei
Sekunden, das ganze Leben einer Verbindung lang** — und gab es hier
nicht. Wir hatten den Aufbau gebaut und danach den Chip sich selbst
ueberlassen. Drei der fehlenden Posten sind SENDEseite und haengen an
der Temperatur:

| alle 2 s in rtw88 | vorher | jetzt |
|---|---|---|
| `rtw_phy_cfo_track` → `rtw8822c_cfo_track` | Zustand da, **nie gefahren** | ✅ |
| `rtw_phy_pwr_track` → `rtw8822c_pwr_track` | **fehlte ganz** | ✅ |
| `rtw_phy_dpk_track` | **fehlte** | ✅ |
| `rtw_phy_ra_track` (`update_wl_phy_info`, `ra_info_update`, `rrsr_update`) | **fehlte** | ✅ |
| `rtw_phy_statistics` (`stat_rssi`, `false_alarm`, `rate_cnt`) | nur `false_alarm` | ✅ |
| `rtw_phy_dig` + fuenf Helfer | **fehlte** | ✅ |
| `rtw_phy_cck_pd` (Linkzweig) | halb | ✅ |
| `rtw_phy_tx_path_diversity` | **fehlte** | ✅ |
| `rtw8822c_adaptivity` / `rtw_fw_adaptivity` | nur Init | ✅ |
| `rtw_coex_wl_status_check`, `monitor_bt_ctr`, `monitor_bt_enable` | nur Init | ✅ |
| `rtw_fw_c2h_cmd_handle` → `RA_RPT` | Rahmen **verworfen** | ✅ |
| `rtw_sw_beacon_loss_check` | **fehlte** | ✅ |

**Warum gerade die Quarznachfuehrung so gut passt.**
`rtw8822c_cfo_track` misst den Frequenzversatz zum AP aus empfangenen
Paketen und dreht den Quarz nach (`set_crystal_cap`). Unser EMPFAENGER
rastet sich an jeder Praeambel neu ein und merkt von Drift nichts; unser
SENDER laeuft auf dem eigenen Quarz. Der Chip waermt ueber Minuten auf —
und das ergibt genau die beobachtete Form: **Empfang einwandfrei, Senden
unhoerbar, nirgends ein Fehler, und es erholt sich nie von allein.**

**Ein struktureller Fund, den erst der Port sichtbar gemacht hat.** Bis
hierher legte JEDE Stufe ihr eigenes `DmInfo`, `DpkInfo` und `Coex` an.
Solange nur Stufen liefen, war das folgenlos; mit dem Watchdog waere es
ein stiller Fehler gewesen:

* `cfo_track.crystal_cap` kommt aus `rtw_phy_init` (Wert der efuse) — ein
  frisches `DmInfo` traegt dort NULL, die Nachfuehrung waere von null
  hochgelaufen statt von der Werkseinstellung.
* `dpk_info.thermal_dpk` kommt aus Stufe 5d — ohne sie kehrt
  `dpk_track` in der ersten Zeile um.
* `coex.bt_disabled` entscheidet, ob die Quarznachfuehrung ueberhaupt
  darf.

Es gibt jetzt **`struct Dev`** in `_start` — das ist `rtwdev` — und die
zehn Stufenfunktionen nehmen es als Parameter. In Linux lebt all das vom
Laden bis zum Entladen. Hier jetzt auch.

**Vier Posten aus Linux stehen bewusst NICHT im Watchdog**, jeder mit
Grund im Code: `rtw_leave_lps`/`rtw_enter_lps` (wir fahren kein
Power-Save) · `rtw_hci_dynamic_rx_agg` (`.dynamic_rx_agg = NULL` fuer
PCI, pci.c:1605) · `rtw_dynamic_csi_rate` (kehrt ohne
Beamforming-Rolle um; `bf.c` bauen wir nicht) · **`rtw_coex_run_coex`**
— der Entscheidungsbaum der Koexistenz ist **L6 dieses Papiers**, 111
Funktionen, eigene Stufe.

**Eine Abweichung, die hier stehen MUSS:** `rtw_coex_monitor_bt_enable`
wird in Linux nur aus `rtw_coex_run_coex` gerufen. Sie erzeugt
`bt_disabled`, und daran haengt `rtw8822c_cfo_need_adjust` — steht BT
nicht als abgeschaltet fest, stellt Linux die Quarznachfuehrung AB und
pinnt den Quarz auf die efuse. Ohne den Aufruf bliebe die Zahl auf ihrem
Anfangswert und das Tor fuer immer zu. Also wird sie aus dem Watchdog
gerufen, bis L6 steht.

**Tabellen erzeugt, nicht abgetippt:** `gen_tables.py` zieht jetzt auch
die zwoelf Kurven der Sendeleistungs-Nachfuehrung
(`rtw8822c_pwr_track_type0_tbl`, je 30 Stuetzstellen) aus `rtw8822c.c`.
Fuer den 8822C gibt es genau EINE — alle sieben RFE-Varianten zeigen auf
`type0`, eine Auswahl nach RFE waere eine erfundene Verzweigung.

**Die Tore:** 1032 Konstanten gegen Linux ohne Abweichung ·
**`seqdiff.py` 263 von 263 Funktionen Zugriff fuer Zugriff** (vorher
220) · txpwrcheck 6 Pruefsummen + 1512 Kanalkombinationen · cfgcheck
13/13 · framecheck 31/31. Abdeckung **398 → 456 von 940**, `phy.c`
59 → 89, `rtw8822c.c` 141 → 156.

`seqdiff.py` hat dabei zwei echte Nachlaessigkeiten von mir gefunden
(`rtw_phy_dig_write` und `rtw8822c_phy_cck_pd_set_reg`) und vier
Zahlenfolgen der Klasse „Makro gegen Maske"; alle sechs stehen jetzt
NAMENTLICH in seiner Liste statt als stiller Filter im Code.

**Was der Geraetelauf sagen wird**, in einer Zeile des Berichts:

    watchdog 214  igi 0x2a  fehlalarm 37  rssi 41  quarz 52  thermo 28/29
                  txidx 3  bt aus  tp 12/48 Mbit

* **`watchdog` waechst** → der Takt laeuft (alle 2 s eins).
* **`quarz` bewegt sich weg vom efuse-Wert** → die Nachfuehrung greift.
  Steht er fest und `bt` sagt `AN (Quarz fest)`, ist sie durch Linux'
  eigenen Koexistenz-Riegel abgestellt — dann ist BT der naechste Posten
  und nicht die Drift.
* **`thermo` steigt ueber die Minuten** → der Chip waermt auf, und
  `txidx` ist die Antwort darauf.
* **Und die Frage dieser Runde:** haelt die Verbindung ueber die
  zwoelf Minuten hinweg, nach denen sie bisher einseitig wurde.

### 0.25.1 — der Zwischenpuffer war halb so gross wie der Ring

**Florians Beobachtung hat den Fall entschieden:** *„sobald ich traffic
machen würde auf der karte dass die vebindung dann mal stirbt.. solang
das nur debug oder mal ein ping ist.. scheint es relativ lange zu
halten."* Verkehrsabhaengig — und das kann die Temperatur nicht erklaeren.

    MGMT_STAGE_BYTES = RTK_DEFAULT_TX_DESC_NUM * TX_SLOT_BYTES  // 128 Plaetze
    max_num_of_tx_queue(Q_BE) = RTK_BEQ_TX_DESC_NUM             // 256 Eintraege

**Der Datenring hat doppelt so viele Eintraege wie der Zwischenpuffer
Plaetze.** Ab dem 128. gesendeten Rahmen liegt `wp * TX_SLOT_BYTES`
hinter dem Puffer. `npk_dma_write` prueft `off + len > pages * 4096` und
gibt **-1** zurueck — und `tx_write_data` warf den Rueckgabewert weg.
Geschrieben wurde also nichts, in den Buffer-Deskriptor ging trotzdem
`dma_phys(stage) + slot`, eine Adresse ausserhalb unserer Belegung, und
**der Chip holte sich von dort fremden Speicher und sendete ihn.**

Das erklaert jede einzelne Beobachtung des vorigen Laufs, und zwar
besser als die Drift:

* **Die Schwelle ist eine ANZAHL, keine Zeit** — im Leerlauf (ein Ping,
  der Debug-Log) dauert es lange bis 128, unter Verkehr Sekunden.
* `tx queue enq 308 deq 308 backlog 0`, `tx drops full 0` — die
  Buchfuehrung von `wp`/`rp` stimmte die ganze Zeit; der Chip ARBEITETE
  die Deskriptoren ab, er las nur aus falschem Speicher.
* Der Empfang blieb einwandfrei (eigener Ring, eigene Puffer).
* Nirgends ein Fehler: die einzige Stelle, die es haette merken koennen,
  war der weggeworfene Rueckgabewert.
* Und danach ist **jeder zweite Ringumlauf kaputt** (Plaetze 128-255),
  was von aussen aussieht wie eine Leitung, auf der manchmal etwas
  durchkommt.

**Drei Dinge gebaut, nicht eins:**

1. Der Puffer wird nach dem GROESSTEN Ring bemessen, der ihn benutzt
   (`MGMT_STAGE_SLOTS = RTK_BEQ_TX_DESC_NUM`, 256 KB → 512 KB).
2. **Beide DMA-Schreibzugriffe werden geprueft.** Ein abgelehnter
   Schreibzugriff ist keine Nebensache, sondern die Meldung, dass Puffer
   und Ring nicht zusammenpassen — jetzt laut und ohne Senden.
3. **Eine Zusicherung zur Bauzeit**, die nicht wegdriften kann:
   `const _: () = assert!(MGMT_STAGE_SLOTS >= RTK_BEQ_TX_DESC_NUM);`
   Gegengeprueft — mit dem alten Wert faellt der Bau um
   (`evaluation panicked: assertion failed`). Dazu ein Test, dass ein
   Rahmen ueberhaupt in einen Platz passt: ein Ueberlauf DORT liefe in
   den Nachbarplatz und nicht aus dem Puffer heraus, der Kernel saehe
   nichts davon.

**Was das fuer 0.25.0 heisst:** der Watchdog bleibt richtig und
notwendig — er fehlte, und die drei Temperaturnachfuehrungen fehlten mit
ihm. Aber **die Ursache des Sterbens war er nicht.** Der naechste
Geraetelauf trennt beides sauber: haelt die Verbindung jetzt unter Last,
war es der Puffer; bleibt ein langsames Einseitigwerden im Leerlauf
uebrig, ist es die Drift — und `quarz` im Bericht sagt, ob die
Nachfuehrung dagegen arbeitet.

### ✅ 0.25.1 AM GERAET BESTAETIGT — 30 Minuten, und die Zahlen sagen warum

Florians Lauf mit 0.25.1 + wifid 0.10.0:

```
tx queue   enq 224  deq 224  backlog 0 B
daten rein/raus 692/224  eapol 5/5  schluessel 5  rx-wachhund 0
neuschluessel 3 empfangen, 3 beantwortet  gtk 4  rauswurf 0
watchdog 903  igi 0x31  fehlalarm 71  rssi 49  quarz 70
           thermo 31/32  txidx 4  bt aus  tp 0/3 Mbit
```

**`watchdog 903` sind 1806 Sekunden** — dreissig Minuten, und vorher war
nach zwoelf Schluss. Der Beweis steht aber nicht in der Dauer, sondern
in drei Zahlen:

* **`daten raus 224`** liegt GENAU im Bereich, der kaputt war. Mit dem
  alten Zwischenpuffer waeren die Rahmen 128 bis 223 aus fremdem
  Speicher gesendet worden. Sie gingen durch.
* **`neuschluessel 3/3` bei `rauswurf 0`.** Vorher war die Salve aus vier
  Rekeys der Moment, in dem der AP aufgab — weil er keine Antwort
  hoerte. Jetzt hoert er sie.
* **`enq 224 deq 224` gegen `daten raus 224`**: Kernel- und Treibersicht
  stimmen Zahl fuer Zahl.

**Und der Watchdog aus 0.25.0 arbeitet sichtbar.** `thermo 31/32` bei
`txidx 4` heisst: der Chip ist warm geworden, und die
Sendeleistungs-Nachfuehrung hat um vier Stufen korrigiert — genau das,
was vorher niemand tat. `igi 0x31` bei `fehlalarm 71`: die
Verstaerkungsregelung laeuft. `bt aus`: der Koexistenz-Riegel ist offen,
die Quarznachfuehrung darf.

**Eine Zahl blieb stumm, und das war ein Berichtsfehler, kein
Codefehler:** `quarz 70` allein sagt nicht, ob die Nachfuehrung etwas
getan hat. **0.27.1 stellt den efuse-Wert daneben** — `quarz 70
(efuse 70)`. Steht er darauf und `bt` sagt „aus", dann ist sie gelaufen
und hat nichts zu korrigieren gefunden; steht er darauf und `bt` sagt
„AN", ist sie abgestellt. Zwei Zahlen, die sich gegenseitig aufloesen.

Dazu, und es ist ein eigener Meilenstein: **das Update kam ueber WLAN.**

### ✅ 100 MB UNTER LAST — und die Sendequittung meldete sich selbst als tot

Florians erster Durchsatzlauf, gegen `tools/netbench_server.py` im LAN:

```
[netbench] GET: 100 MB in 66322 ms = 12 Mbit/s (1 MB/s)

tx queue   enq 9622  deq 9622  backlog 0 B
tx drops   aqm 0   full 0
rx ring    in 72476  dropped 0
daten rein/raus 72478/9620   rauswurf 0   neuverbunden 0
sendequittung 0 ok, 0 ohne ACK, 49 ohne bericht
watchdog 295  igi 0x37  fehlalarm 106  rssi 55  quarz 69
              thermo 32/33  txidx 5  bt aus
```

**Die Last-Frage ist beantwortet.** 72 478 Rahmen empfangen, 9 620
gesendet — das sind **siebenunddreissig volle Umlaeufe** des 256er
Senderinges durch genau den Bereich, der bis 0.25.1 aus fremdem Speicher
sendete. Nichts lief ueber, nichts riss ab.

**Und der neue Beobachter sagte, dass es ihn nicht gibt:** `0 ok, 0 ohne
ACK, 49 ohne bericht`. Neunundvierzig Mal gefragt, keine einzige
Antwort. Das ist der Fall, fuer den `tx_no_report` gebaut wurde — eine
Null mit Begruendung statt einer Null.

**Die Ursache steht in rtw88 selbst: es gibt ZWEI Quittungswege.**

| Weg | Kennung | Aufteilung |
|---|---|---|
| eigenes C2H | `C2H_CCX_TX_RPT` = 0x03 | **V0**: Nummer `payload[6]`, Status `payload[0]` |
| Unterkommando von `C2H_HALMAC` | `C2H_CCX_RPT` = 0x0f | **V1**: Nummer `payload[8]`, Status `payload[9]` |

`rtw_fw_c2h_cmd_handle` behandelt den ersten, `rtw_fw_c2h_cmd_handle_ext`
(fw.c:93-113) den zweiten — und `rtw_tx_report_handle` waehlt die
Aufteilung am `src`. **Wir hoerten nur auf 0x03.** In keinem Header
steht, welchen eine Firmware nimmt; das beantwortet nur der Geraetelauf,
und er hat es beantwortet.

**0.27.2 hoert auf beide** — und zaehlt ab jetzt die C2H-Kennungen, die
es NICHT behandelt (`c2h 0x0fx49` im Bericht). Diese eine Zeile haette
die Frage im ersten Lauf beantwortet, statt im zweiten; das ist die
Lehre, nicht der Fix. `framecheck.py` 41 → 46 Faelle, beide Aufteilungen,
gegen ein Zurueckfallen auf V0 geprueft.

**Was der Lauf noch sagt:** `txidx 5` bei `thermo 32/33` — die
Sendeleistungs-Nachfuehrung hat unter Last eine Stufe mehr korrigiert
als im Leerlauf. `igi 0x37` bei `fehlalarm 106`: DIG regelt mit.

**Und die 12 Mbit/s sind erklaert, nicht raetselhaft:** wir aggregieren
nicht. `rtw_txq_check_agg`, `rtw_txq_push`, `rtw_txq_dequeue`,
`rtw_tx_work` fehlen (Topf 4, Punkt 4 des Audits), also kostet jeder
Rahmen seinen eigenen Medienzugriff. 9 620 Rahmen in 66 s sind 146 je
Sekunde — das ist der Deckel eines Senders ohne A-MPDU, nicht der der
Luft.

### ✅ Der zweite Durchsatzlauf: drei Antworten, eine offene Frage

Mit 0.27.2 + wifid 0.12.0, wieder 100 MB:

```
[netbench] GET: 100 MB in 66718 ms = 12 Mbit/s
daten rein/raus 72553/9579   rauswurf 0   neuverbunden 0
sendequittung 43 ok, 0 ohne ACK, 1 ohne bericht   c2h 0x37x54
watchdog 54  igi 0x36  quarz 68 (efuse 63)  thermo 32/33  txidx 5  bt aus
```

**(1) Die Sendequittung lebt, und sie ist gruen.** `43 ok, 0 ohne ACK` —
der AP hat JEDEN gepruefte Rahmen quittiert. Der Beobachter, der zwei
Runden lang gefehlt hat, sagt jetzt zum ersten Mal etwas, und was er
sagt ist: die Sendeseite ist in Ordnung. Das eine `ohne bericht` ist die
Quittung, die beim Abbruch der Messung noch offen war.

**(2) Die Quarznachfuehrung ARBEITET.** `quarz 68 (efuse 63)` — fuenf
Stufen ueber dem Werkswert. Das ist genau der Mechanismus, den 0.25.0
portiert hat und den ich zuerst faelschlich fuer die Ursache des
Abrisses hielt: er war es nicht (der Zwischenpuffer war es), aber er tut
echte Arbeit, und ohne die zweite Zahl im Bericht haette man es nicht
gesehen.

**(3) Der C2H-Zensus hat seinen ersten Fall geloest — mit einem
Nichts.** `c2h 0x37 x54` ist `C2H_ADAPTIVITY`, die Antwort auf unser
eigenes Kommando, das der Watchdog alle zwei Sekunden schickt (54 Takte,
54 Antworten). **Linux tut damit nichts als sie zu loggen**
(`rtw_fw_adaptivity_result` ist reines `rtw_dbg`), Verwerfen ist also
richtig. Aus einem Unbekannten ein geklaertes Nichts — genau wofuer der
Zensus da ist.

**Offen: die 12 Mbit/s, zweimal auf die Sekunde gleich.**

    Lauf 1: 9622 Rahmen in 66,3 s
    Lauf 2: 9583 Rahmen in 66,7 s   -> 72 553 empfangene Rahmen, 1088/s

Ein Deckel, der sich zweimal auf ein Prozent wiederholt, ist strukturell
und kein Rauschen. Die naheliegende Erklaerung ist die fehlende
Aggregation — aber **naheliegend ist nicht gemessen**, und der Weg
dorthin ist bemerkenswert kurz: `rtw_ops_ampdu_action` behandelt
`IEEE80211_AMPDU_RX_START` mit einem **leeren `break`**. Die
Empfangs-Aggregation ist also reine 802.11-Verwaltung — mac80211
beantwortet den ADDBA Request des AP, die Hardware braucht nichts.

**Also erst die Frage stellen, dann bauen.** 0.28.0 zaehlt die
Verwaltungsrahmen unserer Zelle, die wir bis heute samt und sonders
verwerfen — nach Subtyp, und Action-Rahmen zusaetzlich nach Kategorie
und Aktion:

    mgmt beacon 640  action 3 (zuletzt kat 3/akt 0)  ADDBA-anfragen 3  sonst 0

* **`ADDBA-anfragen > 0`** → der AP WILL aggregieren und wir antworten
  nicht. Dann ist die Antwort darauf der naechste Posten, und sie ist
  klein.
* **`ADDBA-anfragen 0`** → er versucht es gar nicht, und der Deckel
  liegt woanders. Dann zaehlt als naechstes die Zeit im Empfangspfad,
  nicht die Luft.

Dieselbe Regel wie beim C2H-Zensus, und sie hat dort gerade ihren Wert
bewiesen: **zaehlen, was man verwirft.**

Dazu bleibt der Spitzenwert des Durchsatzes jetzt stehen
(`tp 0/0 Mbit (spitze 1/12)`) — der geglaettete faellt nach dem Ende
einer Uebertragung binnen Sekunden auf null, und wer danach `wlan`
tippt, konnte ihn mit nichts vergleichen.

### ✅ Der Deckel hat einen Namen: 180 ADDBA-Anfragen, keine Antwort

Der Zensus aus 0.28.0 hat die Frage in EINEM Lauf beantwortet:

```
mgmt beacon 2634  action 180 (zuletzt kat 3/akt 0)  ADDBA-anfragen 180  sonst 0
```

**Jeder einzelne Action-Rahmen der Zelle war ein ADDBA Request.** Der AP
bittet um die Aggregation, wiederholt es 180 Mal ueber viereinhalb
Minuten — und wir haben ihn nie auch nur gelesen. Solange er keine
Zustimmung hat, darf er nicht aggregieren, und jeder der 72 550 Rahmen
braucht seinen eigenen Medienzugriff. **Das sind die 12 Mbit/s**, drei
Laeufe lang auf ein Prozent dieselben.

**Und der Weg dahin ist kurz, weil rtw88 ihn gar nicht geht:**
`rtw_ops_ampdu_action` behandelt `IEEE80211_AMPDU_RX_START` mit einem
leeren `break`. Die Empfangs-Aggregation ist reine 802.11-Verwaltung —
mac80211 beantwortet den Request, die Hardware braucht nichts. **Die
BlockAcks darauf erzeugt die HARDWARE**, und das ist kein Zufall der
Bauweise: sie muessen eine SIFS nach dem Aggregat hinaus, sechzehn
Mikrosekunden, und das schafft kein Treiber.

0.29.0 liest den Request (`parse_addba_req`) und antwortet
(`build_addba_resp`, Feld fuer Feld nach `ieee80211_send_addba_resp` aus
`net/mac80211/agg-rx.c`).

**Die eine Zahl, die UNSERE Entscheidung ist, und warum sie klein ist:**
der AP fragt, wieviele Rahmen er offen haben darf; mac80211 antwortet
mit dem, was sein Umsortierpuffer fasst. **Wir haben keinen** — es gibt
keinen Nachbau von `ieee80211_sta_manage_reorder_buf`, und ein Rahmen,
den eine Wiederholung nach hinten schiebt, geht bei uns als solcher an
TCP. Also **acht**: eine Wiederholung sortiert dann um hoechstens sieben
um, und das absorbiert jede TCP-Verbindung. Die Aggregation wirkt schon
bei acht — sie spart sieben von acht Medienzugriffen.

**Und weil das eine Abwaegung und keine Messung ist, steht sie in der
Konfiguration:** `ampdu:` in `sys/config/wifi` — fehlt die Zeile, gilt
8; `off` schaltet ab (der Zustand bis 0.28.0); eine ZAHL gibt genau
dieses Fenster, bis 64. Damit misst der naechste Lauf 8 gegen 32, ohne
dass jemand neu uebersetzt.

    ADDBA 180 erbeten, 180 angenommen

`framecheck.py` 55 → 58 Faelle, gegen drei absichtliche Feldfehler
geprueft (TID-Schiebung, Antwort mit Request-Aktionscode, Fenster falsch
geschoben) — alle drei gefangen.

**Benannt und nicht gebaut:** der Umsortierpuffer
(`ieee80211_sta_manage_reorder_buf`). Er ist der Grund fuer das kleine
Fenster, und mit ihm waeren 64 gefahrlos.

### ✅ ADDBA traegt — und deckt den WAHREN Deckel auf: ein `sleep_ms(1)`

Der Lauf mit 0.29.0:

```
[netbench] GET: 100 MB in 52953 ms = 15 Mbit/s      (vorher 66977 ms, 12)
mgmt beacon 822  action 2  ADDBA 2 erbeten, 2 angenommen  sonst 0
```

**Die Antwort geht hinaus, und der AP hoert auf zu fragen** — zwei
Anfragen statt hundertachtzig. Die Sitzung steht.

**Aber +26 % ist kein Vielfaches, und die Zahl daneben sagt warum:**

    vorher     72 549 Rahmen in 67,0 s = 1083 Rahmen/s
    mit ADDBA  72 800 Rahmen in 53,0 s = 1375 Rahmen/s

Der Durchsatz stieg um 26 %, die RAHMENRATE um 26 %, die Groesse je
Rahmen blieb gleich. **Der Deckel ist eine Rahmenrate, nicht die Luft** —
728 us je Rahmen, waehrend die Uebertragung bei MCS15/40 MHz etwa 60 us
dauert.

**Und er stand in unserer eigenen Schleife:**

```rust
if got == 0 {
    host::sleep_ms(1);
}
```

Zwischen zwei Buendeln ist der Ring einen Moment leer. Beim ERSTEN
leeren Blick eine ganze Millisekunde zu schlafen heisst: hoechstens
tausend Mal je Sekunde nachsehen. Rechne es nach:

    1000 Schlafzyklen/s x 1,37 Rahmen je Blick = 1370 Rahmen/s
    gemessen:                                    1375 Rahmen/s

**Das ist keine Naeherung, das ist die Gleichung.** Und es erklaert
auch, warum die Aggregation nur +26 % brachte: sie machte die BUENDEL
groesser, nicht die Blicke haeufiger.

0.30.0 gibt der Schleife die Form, die Linux NAPI nennt: **ein Budget
leerer Blicke, dann erst schlafen** — und liegen bleiben, bis wieder
etwas kommt. Unter Last faellt der Zaehler bei jedem Buendel auf null
und wir schlafen nie; im Leerlauf ist das Budget nach einer knappen
halben Millisekunde aufgebraucht und der Ruhestrom bleibt, wie er war.

Dazu die Messung, die es belegt — **ohne einen einzigen zusaetzlichen
Wirtsaufruf**, weil sie nur zaehlt, was die Schleife ohnehin weiss:

    rx-schleife 53000 blicke mit beute, 812345 leer, 1,4 rahmen/blick, 0 volle stapel

* **`rahmen/blick` knapp ueber eins und `volle stapel 0`** → wir sehen
  schneller nach, als etwas kommt: die Grenze liegt in der Luft oder
  beim AP.
* **`volle stapel` gross** → wir kommen nicht nach, und der naechste
  Posten ist die Zeit IM Empfangspfad (drei Kopien und drei
  Wirtsaufrufe je Rahmen).

**Der Fehler ist aelter als jede Stufe** — die Zeile stammt aus Stufe 5a,
wo sie richtig war: dort lief kein Verkehr, und ohne sie haette der
Kern gebrannt. Sie ist mit der Verbindung mitgewandert und hat dort eine
andere Bedeutung bekommen.

### ❌ Der `sleep_ms(1)` war NICHT der Deckel — die Messung hat mich widerlegt

Mit 0.30.0, derselbe Lauf:

```
[netbench] GET: 100 MB in 55647 ms = 15 Mbit/s      (mit 0.29.0: 52953 ms)
rx-schleife 70867 blicke mit beute, 2936961 leer, 1,2 rahmen/blick, 0 volle stapel
```

**Wir sehen jetzt 54 000 Mal je Sekunde nach statt 1 000 Mal — und die
Rahmenrate blieb gleich** (1312/s statt 1375/s, eher etwas langsamer).
`0 volle stapel` bei `1,2 rahmen/blick`: der Treiber ist nicht die
langsame Seite. Die Rahmen kommen schlicht nicht schneller.

**Die Gleichung stimmte und die Ursache war trotzdem falsch.**
1000 × 1,37 = 1370 gegen gemessene 1375 — das sah aus wie ein Beweis und
war eine Koinzidenz zweier Groessen, die beide bei ~1300 liegen. Genau
dafuer war das Instrument da, und es hat in einem Lauf entschieden,
wofuer sonst eine Runde Vermutungen draufgegangen waere.

**Die Aenderung bleibt trotzdem drin**, aber als das, was sie ist: die
richtige Form (NAPI), nicht ein Fix. Im Leerlauf kostet sie nichts, und
sie nimmt eine Latenz von bis zu einer Millisekunde aus jedem
Empfangsweg — nur den Durchsatz hebt sie nicht.

**Was damit ausgeschlossen ist:**

* die Empfangsschleife (`0 volle stapel`),
* der IP-Stapel (`rx ring dropped 0` — Kern 0 kommt nach),
* das TCP-Empfangsfenster (Window Scaling mit 8 MiB Puffer,
  `OUR_WSCALE = 8`, ausgelegt auf ~700 Mbit),
* unser HT-Element (`IEEE80211_HT_MAX_AMPDU_64K`, Dichte 2 — wir sagen
  64 KB an, der AP duerfte also).

**Was bleibt, ist die Strecke selbst**, und dafuer gibt es eine Messung,
die nichts kostet: `tools/netbench_server.py` liest bei jeder
Uebertragung `TCP_INFO` der Verbindung und schreibt cwnd, das von UNS
angesagte Fenster, RTT, Wiederholungen und DSACKs mit. Bis eben lief er
mit gepufferter Ausgabe und hat nichts protokolliert — das war mein
Fehler beim Starten, nicht am Werkzeug.

    setsid python3 -u tools/netbench_server.py 8080 >/tmp/netbench.log 2>&1 &

**Der naechste Lauf braucht KEINE neue Version** — dieselbe 0.30.0
gegen den jetzt protokollierenden Server. Die eine Zeile sagt dann:

* `retrans` hoch → Verluste in der Luft, und die Ratenwahl ist dran.
* `snd_wnd` klein → wir sagen ein kleines Fenster an, obwohl der Puffer
  gross ist (dann stimmt etwas an `recv_window`).
* `rtt` gross bei kleinem `cwnd` → die Strecke ist langsam, nicht eng;
  dann ist die Latenz der Posten, und `ping` sagte 10 ms, was fuer WLAN
  im selben Raum viel ist.

### ⚠ Die Rate im Bericht war eine BEHAUPTUNG, keine Messung

Florian: *„der AP liefert mehr als 15 Mbit, ich sitze mit einem Geraet
direkt neben dem Notebook und habe das x-fache."* Damit ist die
Gegenseite raus, und der Deckel liegt bei uns.

**Wie Linux die Geschwindigkeit aushandelt — und warum wir sie nicht
sehen:**

* **Senden:** der Treiber schickt der Firmware KEINE Rate, sondern eine
  **Maske** (`rtw_fw_send_ra_info`): welche der 64 Raten das Gegenueber
  kann, nach RSSI beschnitten (`rtw_rate_mask_rssi`). Die **Firmware**
  waehlt daraus laufend und meldet ihre Wahl als C2H `RA_RPT` zurueck
  → `dm_info->tx_rate`.
* **Empfangen:** ausgehandelt wird gar nichts — der AP entscheidet, und
  die tatsaechliche Rate steht in **jedem Empfangsdeskriptor**
  (`pkt_stat.rate` → `dm_info->curr_rx_rate`).

**Und genau diese beiden Zahlen haben wir gesammelt und nie gezeigt.**
Im Bericht stand `rate 0x1b`, und das war `link.highest_rate` — EINMAL
bei der Anmeldung aus den Faehigkeiten des AP gerechnet. Eine Aussage
darueber, was moeglich WAERE. Drei Durchsatzlaeufe lang hat sie
„MCS15" gesagt, waehrend die Leitung mit irgendetwas anderem lief, und
niemand konnte es sehen.

0.31.0 zeigt die Messungen:

    rx HT MCS7 (0x13)  tx HT MCS15 (0x1b, 412 meldungen)  angeboten 0x1b
    ... rssi 54  snr 28/26 ...

* **`meldungen 0`** hiesse: die Firmware meldet ihre Wahl gar nicht, und
  `dm.tx_rate` steht auf **0 = CCK 1M** — womit auch
  `rtw_phy_config_swing_table` die CCK-Kurve der
  Sendeleistungs-Nachfuehrung nimmt statt der OFDM-Kurve.
* **`rx` weit unter `angeboten`** heisst, der AP faehrt uns unter Wert,
  und dann sagt `snr` daneben, ob er recht hat.

Dazu nennt `rate_name` jetzt die genaue Stufe statt der Klasse — bis
hierher stand „HT MCS8-15" fuer acht verschiedene Raten, und fuer die
Frage „mit welcher Rate laeuft die Leitung wirklich" ist das keine
Antwort.

### ✅ Ein Gruppen-Neuschluessel MITTEN im Download — und ein Instrument, das log

**Die wichtigere Nachricht des Laufs**, und Florian hat sie zuerst
gesehen: `[wifid] group rekey` waehrend einer laufenden 100-MB-
Uebertragung, `neuschluessel 1 empfangen, 1 beantwortet`, `gtk 2`,
`rauswurf 0`. **Genau das Szenario, an dem die Verbindung vor 0.25.1
gestorben ist** — der AP erneuert den Gruppenschluessel, und wer nicht
antwortet, fliegt. Jetzt unter Volllast ueberstanden, ohne einen
verlorenen Rahmen.

**Und die neue Ratenzeile hat gleich ihre eigene Luege gezeigt:**

    rx OFDM 6M (0x04)   tx HT MCS4 (0x10, 6 meldungen)   angeboten 0x1b

6 Mbit/s kann nicht stimmen, und das sagt die Arithmetik ohne einen
zweiten Lauf:

    1445 Byte bei 6 Mbit/s   = 1927 us je Rahmen
    gemessen 1312 Rahmen/s   =  762 us je Rahmen

**`curr_rx_rate` ist die Rate des LETZTEN Rahmens.** Eine halbe Sekunde
nach dem Download ist das ein Beacon — und Beacons gehen auf der
niedrigsten Grundrate. Der Bericht zeigte den Takt der Zelle, nicht den
der Daten.

Linux sammelt die richtige Zahl, und ich hatte sie portiert und nie
gezeigt: **`cur_pkt_count.num_qry_pkt[rate]`**, ein Histogramm je Rate,
das `rtw_phy_stat_rate_cnt` jeden Watchdog-Takt nach `last_pkt_count`
schiebt. debugfs liest es LAUFEND mit; wir haben kein debugfs, und ein
Bericht, den jemand NACH einer Uebertragung liest, braucht eine Zahl,
die sie ueberlebt. **0.31.1 summiert es ueber die ganze Verbindung** und
nennt die haeufigste Rate:

    rx HT MCS7 (0x13 in 64812 von 65700, zuletzt OFDM 6M)

Bei 65 000 Datenrahmen gegen 900 Beacons ist das kein Zweifelsfall
mehr. Und `zuletzt` steht daneben, damit die Verwechslung, die mich
erwischt hat, sichtbar bleibt statt verschwunden zu sein.

**Der zweite Befund der Zeile ist echt:** `tx HT MCS4` mit nur **6**
Meldungen in 46 Watchdog-Takten. Die Firmware waehlt fuer UNSEREN
Sendeweg MCS4, obwohl wir MCS15 anbieten — und `snr 27/19` sagt, dass
Pfad B deutlich schlechter steht als Pfad A. Das ist die naechste Spur,
sobald die Empfangsrate bekannt ist.

### 📏 Die echte Rate steht — und der Server nennt eine RTT von 1,3 SEKUNDEN

```
rx HT MCS7 (0x13 in 9566 von 12852, zuletzt CCK 1M)  tx HT MCS4 (0x10, 2 meldungen)
daten rein/raus 72643/10538
rx-schleife 74506 blicke mit beute, 3280375 leer, 1,2 rahmen/blick, 0 volle stapel
```

**Drei Befunde, und einer davon faellt nebenbei ab.**

**(1) Die Aggregation LAEUFT, und zwar nachweisbar.** 72 643 Datenrahmen,
aber nur **12 852** im Ratenhistogramm. Der PHY-Status haengt je **PPDU**
an, nicht je MPDU — also **5,7 MPDU je Aggregat**. Das ist die Zahl, die
ADDBA gebracht hat, und niemand musste sie glauben.

**(2) Die Empfangsrate ist HT MCS7**, nicht MCS15. MCS7 ist
EINstroemig; fuer MCS8-15 braucht es beide Pfade, und `snr 28/20` sagt,
warum der AP es nicht tut: **Pfad B steht acht dB unter Pfad A.** Ob das
die Antenne ist oder der Kalibrierungsrest, der seit 0.10.3 auf Pfad B
nicht konvergiert, ist eine eigene Frage — aber selbst MCS7 auf 40 MHz
sind 135 Mbit/s PHY.

**(3) Und damit ist die Luft ausgeschlossen:**

    ein Aggregat aus 5,7 Rahmen bei MCS7/40 MHz  ~640 us
    -> moeglich ~8800 Rahmen/s,  gemessen 1409   = Faktor 6 darunter

**Der Server sagt, wo die sechs Faktoren liegen** (`TCP_INFO`, seit dem
ungepufferten Start):

```
rtt=1318159us  retrans=116  cwnd=3181  ssthresh=105  pacing=34Mbit
snd_wnd=8376832   (unser Fenster, nie geschlossen)
```

**1,3 Sekunden Round-Trip.** Nicht 10 ms. Das Fenster ist es also nicht
— wir sagen 8 MB an und halten sie offen. Es ist die LATENZ, und Linux'
Pacing rechnet daraus brav 34 Mbit und liefert 16.

**Woher 1,3 Sekunden kommen, ist die Frage dieser Runde**, und
`rahmen/blick 1,2` beantwortet sie NICHT: die Zahl heisst „schnell
genug" ODER „exakt so langsam wie die Ankunft". Wenn ein Rahmen 700 us
Bearbeitung kostet, leert sich der Ring bei jedem Blick, und das sieht
identisch aus.

**0.31.2 misst deshalb die Wanduhrzeit IM Empfangspfad** — nur auf
Bliecken, die etwas brachten (rund 1400 je Sekunde), denn bei den
65 000 leeren waere die Messung der Zustand:

    ... 0 volle stapel, 63 % der zeit im empfangspfad (450 us je rahmen)

* **Anteil hoch** → wir SIND die langsame Seite, trotz `1,2
  rahmen/blick`, und der Posten ist der Empfangspfad selbst: drei Kopien
  (`dma_read_buf` ueber die volle Pufferlaenge, `rx_to_8023`,
  `netdev_submit_rx`) und drei Wirtsaufrufe je Rahmen.
* **Anteil klein** → die Zeit geht woanders hin, und dann ist der
  naechste Messpunkt die Senderichtung: ein `ping` WAEHREND der
  Uebertragung trennt eine stehende Warteschlange (Bufferbloat) von
  einer langsamen Quittung.

### 📐 Der Treiber ist ES NICHT: 2 us je Rahmen, 0 % der Zeit

```
rx-schleife 74190 blicke, 1,1 rahmen/blick, 0 volle stapel,
            0 % der zeit im empfangspfad (2 us je rahmen)
```

**Zwei Mikrosekunden je Rahmen bei 65 000 Bliecken je Sekunde.** Der
Empfangspfad ist zu 99,99 % untaetig. Damit ist die letzte Vermutung
ueber unsere Seite erledigt — und drei Runden Instrumente haben sich
gelohnt, weil sie jede einzeln ausgeschlossen haben statt einer nach der
anderen geraten zu werden.

**Die Kette schliesst sich so:**

    1435 Segmente/s rein, 206 ACKs/s raus = 7,0 Segmente je ACK

Das ist `ACK_COALESCE = 8` aus `kernel/src/net/tcp.rs` und damit FOLGE,
nicht Ursache — bei 1435 Segmenten/s sind 179 ACKs/s genau richtig. Den
Takt setzt die Luft, und dort steht das Missverhaeltnis:

    ein Aggregat aus 5,7 Rahmen bei MCS7/40 MHz:
       488 us Luft + 187 us Overhead (Praeambel, SIFS, BlockAck,
                                      DIFS, mittlerer Backoff) = 675 us
    gemessen:                                                  = 4000 us

**3,3 Millisekunden je Zyklus gehen fuer nichts drauf** — und der
Overhead faellt **einmal je AGGREGAT** an, nicht je Rahmen.

**Damit zeigt alles auf die eine Zahl, die ICH gewaehlt habe: das
Empfangsfenster von 8.** Mit 8 muss der AP alle acht Rahmen neu um das
Medium kaempfen. Er hatte **64** erbeten; ich gab 8, weil es keinen
Nachbau von `ieee80211_sta_manage_reorder_buf` gibt und ein umsortierter
Rahmen bei uns als solcher an TCP geht.

**Und genau diese Sorge war zu gross:** unser TCP puffert Umsortierung
selbst — `OOO_MAX_BYTES = 2 MiB` in `tcp.rs`, ein Segment vor `rcv_nxt`
wird aufgehoben statt verworfen. Die Umsortierung, vor der ich das
Fenster klein gehalten habe, faengt der Stapel darueber ohnehin ab.

**Das Experiment kostet keine neue Version**, weil das Fenster seit
0.29.0 in der Konfiguration steht:

    spell /sys/config/wifi   ->   ampdu: 64

Erwartung, wenn der Zyklus der Deckel ist: acht Mal weniger Zyklen je
Rahmen. Bleibt es bei 16 Mbit/s, ist der Zyklus NICHT die Ursache, und
dann ist der naechste Messpunkt die Luft selbst (Wiederholungen,
Fremdverkehr auf Kanal 7) — dafuer gibt es `fehlalarm` und die
Sendequittung.

### 🔒 Fenster 64 bestaetigt, Luft sauber — und die Rahmenrate ruehrt sich nicht

```
ADDBA 2 erbeten, 2 angenommen (fenster 64 von 64 erbetenen)
crc ht 471/73041 (0 %)   ofdm 357/13152
daten rein/raus 72708/10371      16 Mbit/s
```

|                     | MPDU/Agg | Agg/s | Rahmen/s | Mbit/s |
|---------------------|---------:|------:|---------:|-------:|
| Fenster 8 (0.29.0)  |      5,7 |   243 |     1375 |   15,9 |
| Fenster 8 (0.31.2)  |      6,1 |   237 |     1435 |   16,6 |
| **Fenster 64**      |  **7,4** |   196 |     1441 |   16,7 |

**Die Aggregate wurden 30 % groesser, es gibt entsprechend weniger, und
die Rahmenrate steht wie festgenagelt bei ~1430/s.** Damit ist die
Aggregation als Deckel erledigt — meine letzte Hypothese, und sie ist
tot.

**Die Luft auch:** 471 von 73 041 HT-Rahmen scheitern an der Pruefsumme,
also **0 %**. Kein Wiederholungssturm. Und die Luftzeit:

    ein Aggregat aus 7,4 Rahmen bei MCS7/40 MHz = 819 us
    x 196 je Sekunde = 16 % der Luft belegt
    -> 84 % der Zeit passiert NICHTS

**Ein Deckel, der eine RAHMENRATE ist und keine Luftzeit**, ueberlebt
jede Aenderung an der Aggregation. Und daneben steht die Zahl, die dazu
passt:

    10 371 ACKs / 50,44 s = 206/s  -> alle 4,9 ms
     9 891 Aggregate      = 196/s  -> alle 5,1 ms
    7,0 Segmente je ACK, und ACK_COALESCE = 8

**EIN Aggregat je ACK.** Das ist eine Stopp-und-warte-Schleife und kein
Fenster von 3181 Segmenten, das der Server meldet.

**Zwei Messungen, die das entscheiden und KEINEN neuen Bau brauchen:**

1. **`netbench put 192.168.178.97:8080 /upload 50`** — die
   Senderichtung, vom SERVER gestoppt. Sie nimmt den ganzen
   Empfangs-/Quittungsweg aus der Rechnung. Aehnlich langsam → die
   Strecke; deutlich schneller → der Deckel sitzt in der
   Empfangsrichtung, und das ist TCP und nicht die Luft.
2. **`ping 192.168.178.1` WAEHREND des Downloads** — bleibt er bei
   ~10 ms, ist die 1,3-s-RTT des Servers eine stehende Warteschlange
   (Bufferbloat aus unserem 8-MB-Fenster) ueber einer gesaettigten
   Strecke; steigt er mit, steckt die Verzoegerung im Weg selbst.

### ▶ Danach — hier weitermachen

Stand: Netz läuft, **stabil ist es nicht**. Florian: *„er schmeisst uns nach
einer weile random raus"*. Vier Posten, in dieser Reihenfolge.

#### 1. ✅ Die Stufenausgabe hinter `debug: 1` — gebaut in 0.24.0

Im Autostart druckt der Treiber bei jedem Boot sechs Stufen mit Gates und
macht die Konsole für alles andere unbrauchbar. Schalter: ein Schlüssel in
`sys/config/wifi` (derselbe Weg wie `ampdu:`/`ps:` beim AX200), gelesen mit
dem `cfg_get`, das seit 0.20.1 in `lib.rs` steht.

Still heisst **nicht stumm**: eine Zeile wenn es steht, jede Zeile wenn
etwas nicht steht.

    [rtl8822ce] verbunden: "IvyPie_New" K7 -49 dBm · HT MCS7 40 MHz · AID 3

Die Tore bleiben — sie sind der Grund, warum sechs Stufen entstanden sind,
ohne im Dunkeln zu suchen. Hinter den Schalter, nicht in den Müll.

#### 2. ✅ Den Rauswurf SEHEN — gebaut in 0.24.0

**Wir sind blind dafür.** `rx_to_8023` filtert in der ersten Zeile auf
Datenrahmen:

```rust
if f.len() < 24 || f[0] & 0x0c != DOT11_FC_TYPE_DATA { return None; }
```

Ein Deauth ist ein VERWALTUNGSrahmen und fällt lautlos durch. Aus
Treibersicht stirbt die Verbindung nicht — sie wird nur still. **Genau
deshalb wirkt es zufällig.** Und der Kernel glaubt weiter an `carrier UP`
und schiebt Pakete in eine tote Leitung.

Zu bauen, in `link_pump`s Empfangsschleife:

* **Deauth** `fc[0] == 0xc0` (Subtyp 12), **Disassoc** `fc[0] == 0xa0`
  (Subtyp 10), beide nur, wenn `addr2 == BSSID`. Der **Grundcode** steht
  little-endian in den Bytes 24..26 — 802.11 §9.4.1.7. Er gehört ins Log:
  1 = unspecified, 2 = prev auth no longer valid, 7 = class-3 frame from
  nonassociated STA, **15 = 4-way handshake timeout**, 16 = group key
  handshake timeout. Die 15 und die 16 wären die Bestätigung, dass es am
  Neuschlüssel hängt.
* Dann `netdev_set_link(false)`, `EV_LINK_DOWN` mit `reason 1` an `wifid`
  (Spec §4b), und `ls.authorized = false`.

#### 3. ✅ Den Gruppen-Neuschlüssel ZÄHLEN — gebaut in 0.24.0

Der Weg ist gebaut — `wifid` hat `Step::Rekey`, und wir verschlüsseln
EAPOL, sobald die PTK steht (genau der Fehler, den der AX200-Kommentar
teuer bezahlt hat) —, aber **nichts zählt ihn**. Zwei Zähler im
Treiberbericht beantworten die Frage in einem Lauf:

    neuschluessel 3 empfangen, 3 beantwortet · gtk-installationen 4

Kommt „3 empfangen, 0 beantwortet", liegt es an uns. Kommt „3/3" und der
AP wirft uns trotzdem raus, liegt es woanders — und der Grundcode aus
Posten 2 sagt wo.

#### 4. ▶ Wiederverbinden (die Heilung) — OFFEN, wartet auf den Lauf

Erst wenn 2 und 3 messen, lohnt sich das: nach `EV_LINK_DOWN` zurück zu
5e (Auth + Assoc auf demselben Kanal), Schlüssel im CAM löschen
(`sec::clear_cam`), `link_setup` **nicht** noch einmal — `EV_READY` ein
zweites Mal war der Fehler von 0.23.0. `wifid` braucht allerdings einen
frischen Supplicant, also gehört genau dort ein neues `EV_READY` hin. Das
ist der feine Unterschied, den 0.23.1 nicht auflöst: **einmal je
Verbindung, nicht einmal je Stufe.**

Dazu ein TX-Wachhund (der Sendering kann genauso steckenbleiben wie der
Empfangsring) und, weiter hinten: LPS + reservierte Seiten, die laufende
Koexistenz, DIG, und die DAC-Kalibrierung, die seit 0.10.3 nicht
konvergiert.

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
      txpwrcheck.py · cfgcheck.py · framecheck.py

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
