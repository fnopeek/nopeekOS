# RTL8822CE gegen Linux — die vollständige Karte

**Stand:** 2026-09-19 · noch kein Treiber · Referenz **Linux 6.18.26**
**Chip:** `10ec:c822` — Realtek RTL8822CE, Wi-Fi 5 (802.11ac), 2T2R, PCIe
**Linux-Treiber:** `drivers/net/wireless/realtek/rtw88/`, Modul `rtw_8822ce`

Nicht nach Eindruck, sondern ausgezählt. Die Quelle liegt vollständig unter
`~/.cache/nopeekos/linux-src/linux-6.18.26/drivers/net/wireless/realtek/rtw88/`
— genau die Dateien, die das `Makefile` für `CONFIG_RTW88_8822CE` baut, plus
die Header. Gezählt wird mit

```
python3 tools/linux-coverage.py --chip rtl8822ce        # Übersicht
python3 tools/linux-coverage.py --chip rtl8822ce -v     # mit den fehlenden Namen
```

Die Heuristik ist **großzügig**: ein Treffer im Kommentar zählt schon. Jede
Zahl hier ist damit eine **Obergrenze** der Abdeckung.

---

## 0 — Die Gesamtzahl, und warum sie kleiner ist als beim AX200

| | rtw88 (8822CE) | iwlwifi (AX200) |
|---|---|---|
| Treiberfunktionen | **940** | 1985 |
| `net/mac80211/` | 1732 | 1732 |
| Parametertabellen | 46 105 Zeilen | Firmware-intern |
| Firmware-Blob | 202 600 Bytes | 1,3 MB |

940 ist die Zahl für **alle** Dateien, die das Modul baut — Kern + Chip + PCI.
Fremde Chips im selben Verzeichnis (8703b, 8723, 8812a, 8814a, 8821, 8822b,
88xxa) und die Busse SDIO/USB sind ausgeschlossen; das Werkzeug filtert sie.

---

## 1 — Datei für Datei

### Anwendbar, im Hauptweg

| Datei | fn | Was drin ist |
|---|---|---|
| `rtw8822c.c` | **171** | Der Chip. Init (BB/RF/MAC), Kanal, Sendeleistung, **und 48 Funktionen DPK + ~25 DAC-Kalibrierung + ~15 TXGAPK**. |
| `coex.c` | **111** | BT-Koexistenz. Der Chip ist ein Combo-Teil (WiFi+BT auf einem Die), das ist keine Kür. |
| `fw.c` | **98** | H2C/C2H-Protokoll, Reserved Pages, Scan-Offload, Beacon-Filter, RA-Report. |
| `phy.c` | **97** | Tabellenlader, DIG, CCK-PD, Ratenmasken, Tx-Power-Auflösung, `rtw_phy_init`. |
| `main.c` | **84** | `rtw_core_init/start`, `rtw_power_on`, Kanalwahl, Watchdog, VIF/STA-Buch. |
| `pci.c` | **81** | Transport: BAR**2**, 8 TX-Ringe + 2 RX-Ringe (nur MPDU wird der HW gemeldet), ASPM/L1, DBI/MDIO-Tuning, ISR. |
| `mac.c` | **49** | Power-Sequenz-Interpreter, **Firmware-Download**, TRX-FIFO-Aufteilung, H2C-Init. |
| `mac80211.c` | **41** | Die 38 Ops, die mac80211 ruft — **unsere Schnittstelle nach oben**. |
| `tx.c` | **31** | Sendedeskriptor bauen, Ratenwahl, Queue-Mapping, TX-Report. |
| `ps.c` | **22** | LPS/IPS. Für den ersten Link nicht nötig, für den Akku schon. |
| `regd.c` | **15** | Regulatorik (Kanalplan aus efuse + Land). |
| `bf.c` | **14** | Beamforming (VHT su/mu). |
| `util.c` | **9** | `check_hw_ready`, LTE-Coex-Register, Iteratoren. |
| `rx.c` | **8** | Empfangsdeskriptor auswerten, PHY-Status, `rx_status` füllen. |
| `efuse.c` | **5** | efuse-Bank lesen und die logische Karte auspacken. |
| `sec.c` | **5** | Hardware-CAM: Schlüssel eintragen. **CCMP macht der Chip.** |
| `sar.c` | **4** | SAR-Grenzen. |
| **Summe anwendbar** | **845** | |

### Nicht anwendbar

| Datei | fn | Warum |
|---|---|---|
| `wow.c` | 43 | WoWLAN — braucht die zweite Firmware und Suspend/Resume. Wir haben beides nicht. |
| `debug.c` | 49 | debugfs — Linux-Gerätemodell. Was davon zählt, geht in `npk_driver_report`. |
| `led.c` | 3 | LED-Klasse. |
| **Summe** | **95** | |

---

## 2 — Der Weg nach oben: was mac80211 tut, das wir tun müssen

rtw88 ist **SoftMAC**. Was der Treiber NICHT tut, tut mac80211 — und das ist
bei uns eine Lücke, kein fertiger Baustein. Zwei Richtungen, beide gezählt:

**(a) Was mac80211 vom Treiber VERLANGT** — die Liste ist genau `rtw_ops`,
**38 Einträge**. Das ist der Auftragszettel für unsere MLME-Schicht:

```
tx · wake_tx_queue · start · stop · config · add_interface · remove_interface
change_interface · configure_filter · bss_info_changed · start_ap · stop_ap
conf_tx · sta_add · sta_remove · set_tim · set_key · ampdu_action
can_aggregate_in_amsdu · sw_scan_start · sw_scan_complete · mgd_prepare_tx
set_rts_threshold · sta_statistics · flush · set_bitrate_mask · set_antenna
get_antenna · reconfig_complete · hw_scan · cancel_hw_scan · link_sta_rc_update
set_sar_specs · suspend · resume · set_wakeup · + 4× chanctx-Emulation
```

Davon entfallen für uns sofort: `start_ap`/`stop_ap` (kein AP-Modus),
`suspend`/`resume`/`set_wakeup` (kein PM), die vier `chanctx`-Emulationen.
**Bleiben 29.**

**(b) Was der Treiber von mac80211 RUFT** — 73 verschiedene `ieee80211_*`-Namen
in den anwendbaren Dateien, davon sind die meisten Strukturtypen. Echte
Funktionen, die wir stellen müssen:

| Gruppe | Namen |
|---|---|
| Rahmen bauen | `probereq_get`, `nullfunc_get`, `pspoll_get`, `proberesp_get`, `beacon_get_tim` |
| Zustellung nach oben | `rx_napi`, `tx_status_irqsafe`, `tx_info_clear_status`, `free_txskb` |
| MLME-Meldungen | `scan_completed`, `connection_loss`, `cqm_rssi_notify`, `restart_hw` |
| Aggregation | `start_tx_ba_session`, `stop_tx_ba_cb_irqsafe` |
| Queues | `wake_queue(s)`, `stop_queue(s)`, `tx_dequeue`, `txq_get_depth` |
| Buch | `find_sta`, `find_sta_by_ifaddr`, `iterate_stations_atomic`, `iterate_active_interfaces_atomic` |
| Sonstiges | `channel_to_frequency`, `request_smps`, `queue_work`, `queue_delayed_work` |

**Das ist der ehrliche Umfang der oberen Hälfte: ~25 Funktionen plus eine
STA-MLME (Scan → Auth → Assoc → 4-Wege → Link).** Die MLME selbst steht in
`net/mac80211/mlme.c` (183 Funktionen) — die portieren wir **nicht**, sondern
wir bauen die STA-Teilmenge, so wie es `wifi_ax200` schon einmal getan hat
(`connect_send_auth`, `connect_send_assoc`, `parse_beacon`, `wait_mgmt_response`).

---

## 3 — Die Aufrufkette beim Hochfahren, in Linux-Reihenfolge

Jede Zeile ist eine Funktion, die VOLLSTÄNDIG portiert wird. Keine Auswahl.

```
rtw_pci_probe                                  (pci.c)
 ├─ rtw_core_init                              (main.c)   Zustand + Firmware laden
 ├─ rtw_pci_claim                              (pci.c)    PCI aktivieren, Bus-Master
 ├─ rtw_pci_setup_resource                     (pci.c)
 │   ├─ rtw_pci_io_mapping                                **BAR2** (nicht BAR0!)
 │   └─ rtw_pci_init  → rtw_pci_init_trx_ring
 │        ├─ rtw_pci_init_tx_ring ×8                      BK BE VI VO BCN MGMT HI0 H2C
 │        └─ rtw_pci_init_rx_ring ×2                      MPDU + C2H — C2H bekommt auf
 │                                                        PCIe KEIN Adressregister (L3)
 ├─ rtw_chip_info_setup                        (main.c)
 │   ├─ rtw_chip_parameter_setup                          REG_SYS_CFG1 → cut, rf_type
 │   ├─ rtw_chip_efuse_info_setup
 │   │    ├─ rtw_chip_efuse_enable
 │   │    │    ├─ rtw_hci_setup · rtw_mac_power_on        (!) MAC an
 │   │    │    ├─ write8(REG_C2HEVT, C2H_HW_FEATURE_DUMP)
 │   │    │    └─ rtw_download_firmware                   (!) FWDL passiert ZWEIMAL
 │   │    ├─ rtw_parse_efuse_map    (efuse.c)             physisch → logisch
 │   │    │    └─ rtw8822c_read_efuse → rtw8822ce_efuse_parsing
 │   │    ├─ rtw_dump_hw_feature                          MAC, Bandbreite, Pfade
 │   │    ├─ rtw_check_supported_rfe                      rfe_option → rfe_def
 │   │    └─ rtw_chip_efuse_disable → rtw_mac_power_off
 │   └─ rtw_chip_board_info_setup                         Antennen/PA/LNA aus efuse
 ├─ rtw_pci_phy_cfg                            (pci.c)    gen1/gen2-PHY, ASPM, CLKREQ
 ├─ rtw_register_hw                            (main.c)   → bei uns: Bänder + MAC melden
 └─ rtw_pci_request_irq                                   → bei uns: Pollen

rtw_ops_start → rtw_core_start                (main.c)
 └─ chip->ops->power_on = rtw_power_on         (main.c)
     ├─ rtw_hci_setup = rtw_pci_setup → rtw_pci_reset_trx_ring
     ├─ rtw_mac_power_on                       (mac.c)
     │   ├─ rtw_mac_pre_system_cfg                        RF-Kill, LTE-Coex, HCI
     │   ├─ rtw_mac_power_switch(true)
     │   │    └─ rtw_pwr_seq_parser(card_enable_flow_8822c)
     │   │         ├─ trans_carddis_to_cardemu_8822c      Tabelle: write/poll/delay
     │   │         └─ trans_cardemu_to_act_8822c
     │   └─ rtw_mac_init_system_cfg
     ├─ rtw_download_firmware                  (mac.c)
     │   ├─ check_firmware_size                           HDR64 + dmem+8 + imem+8 + emem+8
     │   ├─ wlan_cpu_enable(false)
     │   ├─ download_firmware_reg_backup / reset_platform
     │   ├─ start_download_firmware → send_firmware_pkt (rsvd page) + iddma_download_firmware
     │   ├─ download_firmware_reg_restore / end_flow      Prüfsummen, BIT_FW_DW_RDY
     │   ├─ wlan_cpu_enable(true)
     │   └─ download_firmware_validate                    REG_MCUFW_CTRL == FW_READY
     ├─ rtw_mac_init                           (mac.c)
     │   ├─ rtw_init_trx_cfg → txdma_queue_mapping · priority_queue_cfg · init_h2c
     │   ├─ rtw8822c_mac_init                  (rtw8822c.c)  SIFS, EDCA, AMPDU, RRSR …
     │   └─ rtw_drv_info_cfg                               PHY-Status im RX-Deskriptor
     ├─ rtw8822c_phy_set_param                 (rtw8822c.c)
     │   ├─ rtw8822c_header_file_init(pre)
     │   ├─ rtw_phy_load_tables                (phy.c)     mac · bb · agc · rfk · rf_a · rf_b
     │   ├─ rtw8822c_header_file_init(post)
     │   ├─ rtw8822c_config_trx_mode
     │   ├─ rtw_phy_init                       (phy.c)     DIG, CCK-PD, false-alarm
     │   ├─ rtw8822c_rf_init                               DAC-Kalibrierung, Power-Trim
     │   ├─ rtw8822c_pwrtrack_init
     │   └─ rtw_bf_phy_init                    (bf.c)
     ├─ rtw_mac_postinit                                   NULL beim 8822C
     ├─ rtw_hci_start = rtw_pci_start                      Interrupts/Polling an
     ├─ rtw_fw_send_general_info / rtw_fw_send_phydm_info   erste H2C
     └─ rtw_coex_power_on_setting / rtw_coex_init_hw_config (coex.c)
```

**Gate der Stufe:** `download_firmware_validate` gibt 0 zurück, und
`REG_MCUFW_CTRL` liest `FW_READY`. Ohne das ist alles dahinter Raten.

---

## 4 — Wo der Aufwand wirklich sitzt

Ausgezählt, in absteigender Reihenfolge:

1. **RF-Kalibrierung, 88 Funktionen in `rtw8822c.c`** — DPK (48), DAC-Cal (~25),
   TXGAPK (~15). Jede ist Registerfolge + Messschleife. **Das ist die Stelle,
   an der der RTL8852BE-Port 100 Stunden verloren hat.** Sie bekommt eine
   eigene Stufe mit eigenem Gate.
   **IQK dagegen macht die FIRMWARE** (`rtw8822c_do_iqk` = ein H2C-Paket plus
   Pollen auf `REG_RPT_CIP`) — beim 8852BE waren das 851 Zeilen Rust.
2. **Koexistenz, 111 Funktionen in `coex.c`** — Combo-Chip. Ohne sie ist der
   Durchsatz unvorhersehbar, sobald BT aktiv ist.
3. **Parametertabellen, 46 105 Zeilen** — `mac` (≈0), `agc` 3728, `bb` 3006,
   `rf_a` 40 070, `rf_b` 40 706, `bb_pg` 322, `txpwr_lmt` 16 380 + 9 555,
   `rfk_init` 4920, DPK-AFE 105/117/49 Einträge.
   **Von Hand abschreiben ist ausgeschlossen** — die werden erzeugt, wie
   `tools/wasm/wifi/gen_tables.py` es für den 8852BE tut.
4. **Der Rest ist mechanisch**: Power-Sequenz ist ein Tabelleninterpreter
   (54 Kommandos), FWDL ist eine Seite Code, `mac_init` ist eine Folge
   benannter Registerschreiber.

---

## 5 — Was die Zahlen NICHT sagen

- **940 ist nicht „wenig"**. Der AX200 steht nach 108 Versionen bei 151/3717
  erwähnten Namen, und seine Verbindung läuft. Abdeckung ist kein Fortschritt,
  sie ist ein **Frühwarnsystem**: fällt eine ganze Datei auf 0, fehlt eine
  Schicht, und dann ist Debuggen sinnlos ([[feedback_port_completely_debug_never]]).
- **Die obere Hälfte steht in keiner dieser Zahlen.** Die STA-MLME ist Arbeit,
  die `rtw88` gar nicht enthält.
