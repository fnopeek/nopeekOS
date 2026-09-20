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

**Abdeckung nach Stufe 5f: 396 von 940 rtw88-Funktionen** (vor dieser Runde
89). `rtw8822c.c` 68/171 · `phy.c` 59/97 · `mac.c` 39/49 · `pci.c` 30/81 ·
`coex.c` 27/111 · `main.c` 18/84 · `tx.c` 15/31 · `efuse.c` 5/5 · `rx.c` 3/8.
Auf 0 stehen nur noch `mac80211.c` (die obere Hälfte, die `wifid` ersetzt),
`debug.c`, `led.c` und `wow.c` — die letzten drei stehen unter „wird bewusst
nicht gebaut".

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

### ▶ Danach — hier weitermachen

**5g — die Verbindung halten.** Der Vierwegehandschlag und der
Schlüsselspeicher (`rtw_sec_write_cam`), dann LPS mit den reservierten
Seiten,
`rtw_get_channel_params` fürs Kanalhüpfen, die Elementeauswertung des
Beacons, und `rtw_rx_addr_match`. Der `netdev`-Anschluss
(`npk_netdev_register`, `npk_submit_rx`) kommt ans ENDE dieser Stufe, nicht
an ihren Anfang: vor einer Verbindung gibt es keine Datenrahmen, die er
weiterreichen könnte.

Ab dort ist `wifid` dran; die obere Hälfte ist herstellerunabhängig und in
`wifi_ax200` einmal gebaut ([[project_wifi_ax200]]).

**Offen und benannt:** die DAC-Kalibrierung konvergiert nicht (siehe oben) ·
`rtw_coex_switchband_notify` und `rtw_coex_run_coex` fehlen, also gibt es
keine laufende Koexistenz · `rtw_get_channel_params` für 40/80 MHz ·
`rtw_regd_init` (die Zone kommt derzeit aus der efuse, `regd 1`).

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
