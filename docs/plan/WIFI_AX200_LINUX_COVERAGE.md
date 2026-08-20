# AX200 gegen Linux — die vollständige Karte

**Stand:** 2026-08-20 · wifi_ax200 0.97.0 · Referenz **Linux 6.18.26**

Nicht nach Eindruck, sondern ausgezählt. `tools/linux-coverage.py` geht jede
`.c`-Datei von `iwlwifi/` und `net/mac80211/` durch, zieht die
Funktionsdefinitionen heraus und prüft, ob der Name irgendwo in unseren
Quellen vorkommt. Die Heuristik ist **großzügig**: ein Treffer im Kommentar
zählt schon. Jede Zahl hier ist damit eine **Obergrenze** der Abdeckung.

```
python3 tools/linux-coverage.py            # Übersicht
python3 tools/linux-coverage.py <baum> -v  # mit den fehlenden Namen
```

## Die Gesamtzahl

| Bereich | erwähnt / gesamt |
|---|---|
| `pcie/` — Transport | 31 / 297 |
| `mvm/` — Treiberlogik | 76 / 1264 |
| `fw/` — Firmware-Runtime | 3 / 223 |
| `iwlwifi/` — Kern | 16 / 201 |
| `net/mac80211/` | **17 / 1732** |
| **Summe** | **143 / 3717** |

Roh gelesen sagt das wenig — ein großer Teil von Linux ist AP-Modus, Mesh,
IBSS, TDLS, P2P, WoWLAN, debugfs, FTM, MLD/EHT für neuere Chips. Die Karte
unten trennt deshalb **anwendbar** von **nicht anwendbar**, Datei für Datei.

---

## Teil 1 — `pcie/`: der Transport

| Datei | ab | Urteil |
|---|---|---|
| `trans.c` | 11/122 | **anwendbar, Kern portiert.** Reset, APM, HW-ready, Persistence, LTR — die Bring-up-Kette steht. Es fehlt: Suspend/Resume, Fehler-Dump-Sammlung, IRQ-Handling (wir pollen). |
| `rx.c` | 7/52 | **anwendbar, Kern portiert.** Ring-Init, Restock, Drain, RB-Besitz. Es fehlt: mehrere RX-Queues (RSS), Interrupt-Coalescing, Napi. |
| `tx-gen2.c` | 5/25 | **anwendbar, Kern portiert.** TFD-Bau, Queue-Alloc. Es fehlt: dynamisches Queue-Management, Flush/Drain-Pfade. |
| `tx.c` | 3/52 | **nicht anwendbar** — gen1-Transport. |
| `trans-gen2.c` | 3/12 | anwendbar, portiert. |
| `ctxt-info.c` | 2/7 | anwendbar, portiert. |
| `ctxt-info-v2.c` | 0/16 | **nicht anwendbar** — AX210+. |
| `drv.c` | 0/10 | **nicht anwendbar** — Linux-Gerätemodell. |

**Urteil Transport: im Wesentlichen fertig.** Was fehlt, ist Interrupt-Betrieb
statt Polling und die Fehlerdiagnose-Infrastruktur.

---

## Teil 2 — `mvm/`: die Treiberlogik

### Anwendbar und teilweise portiert

| Datei | ab | Was fehlt (namentlich) |
|---|---|---|
| `sta.c` | 10/92 | **82 Funktionen.** Der ganze Queue-Manager (`sta_alloc_queue`, `sta_alloc_queue_tvqm`, `find_free_queue`, `redirect_queue`, `unshare_queue`, `inactivity_check`, `remove_inactive_tids`) — wir haben EINE feste Datenqueue. Die TX-Aggregations-Schnittstelle (`sta_tx_agg_start/oper/stop/flush`). Der Schlüsselpfad (`set_sta_key`, `send_sta_key`, `remove_sta_key`, `send_sta_igtk`, `update_tkip_key`) — wir haben feste Schlüsselplätze. `get_sta_ampdu_dens` (VHT-A-MPDU-Größe!), `get_sta_uapsd_acs`, `get_queue_size`, `add_sta_cmd_size`. Dazu alles zu int-/bcast-/mcast-/aux-/sniffer-Stationen und `drain_sta`/`modify_disable_tx`. |
| `mac-ctxt.c` | 10/50 | Kanalwechsel (CSA), Beacon-Bau (AP-Modus), Multicast-Filter, `mac_ctxt_recalc_tsf_id`, Statistik-Notifications. |
| `tx.c` | 3/42 | **39 Funktionen — der komplette Sende- und Sendestatus-Pfad.** `set_tx_cmd`, `set_tx_cmd_rate`, `set_tx_cmd_crypto`, `set_tx_cmd_pn`, `set_tx_params`, `tx_skb_sta`, `tx_tso` (A-MSDU!), `max_amsdu_size`, `tx_reclaim`, `rx_tx_cmd_single`/`_agg`, `get_tx_fail_reason`, `get_scd_ssn`, `check_ratid_empty`, `flush_tx_path`, `tx_airtime`. Wir haben davon den Erfolgsfall. |
| `rxmq.c` | 6/35 | **29 Funktionen.** `is_dup` (Duplikatfilter!), `check_pn` (Replay-Schutz!), `rx_csum`, `rx_he`/`rx_eht` (PHY-Info), `rx_frame_release` und `rx_bar_frame_release` (Reorder-Freigabe durch die Firmware!), `rx_beacon_filter_notif`, `del_ba`, `rx_mgmt_prot`. |
| `power.c` | 5/28 | **23 Funktionen.** Der **gesamte Beacon-Filter** (`enable_beacon_filter`, `beacon_filter_send_cmd`, `beacon_filter_set_cqm_params`) — deshalb `fw notifs 0`. Dazu U-APSD komplett, DTIM-Skip, `power_update_ps`, `power_vif_assoc`. |
| `rs-fw.c` | 4/19 | `rs_fw_set_supp_rates` im Detail (HE/EHT-Raten), `rs_fw_get_max_amsdu_len`, `rs_fw_tlc_update_notif` (die Firmware MELDET Ratenwechsel — wir lesen sie nur aus TX-Antworten). |
| `fw.c` | 5/31 | `sf_update` (Smart FIFO), `lari_cfg`, `ppag_init`, `sar_init`, `sgom_init`, `tas_init`, `uats_init`, `send_recovery_cmd`, `config_ltr`. |
| `scan.c` | 7/100 | Scheduled Scan, Netzwerk-Listen, 6-GHz-Scan, Scan-Abbruch, `scan_umac_fill_ch_p_v*` in voller Breite. |
| `phy-ctxt.c` | 5/14 | RLC in voller Breite, `phy_ctxt_unref`, Kanal-Wechsel. |
| `mac80211.c` | 7/157 | Die Anbindungsschicht selbst. Relevant daraus: `mac_ampdu_action`, `sta_state`-Übergänge (0.97.0 teilweise), `bss_info_changed`, `conf_tx` (EDCA je AC!), `mgd_prepare_tx`, `flush`. |
| `nvm.c` | 2/10 | Regulatorik in voller Breite. |
| `utils.c` | 1/54 | Fehlerprotokoll, `mac_ac_to_tx_fifo` (heute korrigiert), Diagnose. |
| `rx.c` | 1/24 | Statistik-Notifications, `rx_ba_notif` (!), Energie-Messung. |
| `coex.c` | 1/18 | BT-Koexistenz über die Init hinaus. |
| `time-event.c` | 1/36 | Session Protection in voller Breite, Time-Event-Notifications. |
| `sf.c` | 0/4 | **Smart FIFO — gar nicht.** |
| `tt.c` | 0/30 | **Thermik — gar nicht.** Kein `tt_tx_backoff`, kein CTDP. |
| `link.c` | 0/8 | Link-Kontext (für MLD, für uns randständig). |

### Nicht anwendbar

`d3.c` (0/70, WoWLAN) · `tdls.c` (0/14) · `ftm-initiator.c`/`ftm-responder.c` (1/57) ·
`ptp.c` (0/12) · `led.c` (0/6) · `rfi.c` (0/4) · `vendor-cmd.c` (0/4) ·
`debugfs*.c` (0/81) · `mld-*.c` (1/94, neuere Chips) · `rs.c` (2/101, der
Host-Ratenscaler — unsere Firmware hat TLC-Offload) · `time-sync.c` (0/7)

---

## Teil 3 — `fw/` und `iwlwifi/`

| Datei | ab | Urteil |
|---|---|---|
| `iwl-nvm-parse.c` | 6/28 | anwendbar, Kern portiert. Es fehlt die volle Kanal-/Regulatorik-Auswertung. |
| `iwl-io.c` | 4/25 | anwendbar, portiert. |
| `iwl-trans.c` | 3/58 | anwendbar, Kern portiert. |
| `fw/acpi.c` + `fw/uefi.c` | 0/48 | **anwendbar, fehlt ganz** — Sendeleistungs-Tabellen aus der Plattform (SAR/WGDS/PPAG/WTAS). Ohne sie nimmt die Firmware NVM-Vorgaben. Wirkung ungemessen. |
| `fw/regulatory.c` | 0/16 | **anwendbar, fehlt ganz.** |
| `fw/dbg.c` | 0/97 | nicht anwendbar (Debug-Dumps). |
| `fw/pnvm.c` | 1/9 | anwendbar, portiert (Platform NVM). |
| `iwl-phy-db.c` | 0/13 | nicht anwendbar bei unified ucode. |

---

## Teil 4 — `net/mac80211/`: die Schicht, die wir von Hand ersetzt haben

**17 von 1732.** Das ist die eigentliche Antwort auf „was hat Linux, was haben
wir nicht". Nicht alles davon ist nötig — aber die folgenden Dateien sind
anwendbar und stehen auf **null oder fast null**:

| Datei | ab | Was das konkret bedeutet |
|---|---|---|
| `ht.c` | 1/11 | **`ieee80211_ht_cap_ie_to_sta_ht_cap` fehlt** — die SCHNITTMENGE aus dem HT-Element des AP und unseren eigenen Fähigkeiten. Genau daraus liest iwlwifi `ampdu_factor`, `ampdu_density`, `bandwidth`, `smps_mode` für jedes ADD_STA. Wir geben der Firmware die **Rohwerte des AP**. Dazu fehlen `apply_htcap_overrides`, `ba_session_work`, `request_smps`, `ht_handle_chanwidth_notif`. |
| `vht.c` | 0/14 | **komplett.** `vht_cap_ie_to_sta_vht_cap`, `sta_init_nss`, `_ieee80211_sta_cur_vht_bw`, `sta_cap_rx_bw`, `vht_handle_opmode` (der AP kann die Breite im Betrieb ändern — wir bekommen das nie mit), `process_mu_groups`. |
| `he.c` | 0/9 | **komplett.** `he_cap_ie_to_sta_he_cap`, `he_op_ie_to_bss_conf`, `he_spr_ie_to_bss_conf`, OMI. |
| `wme.c` | 0/6 | **komplett.** `ieee80211_select_queue` (TID/AC je Frame — wir schicken alles auf TID 0), `ieee80211_set_qos_hdr` (QoS-Control-Feld — bei uns durchgehend null), `downgrade_queue`. |
| `status.c` | 0/23 | **komplett.** Der ganze Sendestatus-Pfad: `frame_acked`, `report_low_ack`, `lost_packet`, `tx_rate_update`, `handle_filtered_frame`, `check_pending_bar`. |
| `agg-tx.c` | 1/23 | **komplett** — auf TLC-Offload-Firmware macht das die Firmware, ABER der Zustand (`sta->ampdu_mlme`, `tid_tx`) fehlt damit auch. |
| `key.c` | 0/41 | **komplett.** Schlüsselverwaltung, Platzvergabe, Rekey-Übergänge. Wir haben zwei feste Plätze. |
| `wpa.c` | 0/27 | **komplett.** CCMP/GCMP-Verarbeitung im Host (bei uns macht das die Firmware — teilweise anwendbar). |
| `parse.c` | 0/10 | **komplett.** Der generische Element-Parser. Wir parsen von Hand, Element für Element, an einer Stelle. |
| `spectmgmt.c` | 0/6 | **komplett.** `parse_ch_switch_ie` — ein AP, der den Kanal wechselt, verliert uns. |
| `sta_info.c` | 0/89 | **komplett.** Die Stationsdatenstruktur samt Zustandsmaschine, Alterung, Statistik. |
| `mlme.c` | 5/183 | **fast komplett fehlend.** Verbindungsaufsicht, Beacon-Verlust, Roaming, BSS-Auswahl, Power-Save-Übergänge, 802.11v/r/k, CSA, OMI, `sta_wmm_params` (seit 0.95.0 die eine Funktion, die wir haben), `mgd_probe_ap`. |
| `airtime.c` | 0/8 | Airtime-Fairness. |
| `chan.c` | 0/56 | Kanal-Kontext-Verwaltung (mehrere VIFs — für uns randständig). |
| `rate.c` | 0/30 | Ratensteuerungs-Rahmen — nicht anwendbar bei TLC-Offload. |

Nicht anwendbar: `mesh*.c` (0/169), `ibss.c` (0/34), `ocb.c` (0/8),
`tdls.c` (0/42), `wep.c`/`tkip.c` (0/22), `rc80211_minstrel*` (0/64),
`debugfs*.c` (0/102), `s1g.c` (0/10), `cfg.c` (0/152, nl80211-Anbindung),
`ethtool.c`, `led.c`, `wbrf.c`.

---

## Teil 5 — Was daraus folgt

Drei strukturelle Befunde, die aus der Zählung kommen und nicht aus einem
Verdacht:

**1. Wir geben der Firmware die Fähigkeiten des AP, nicht die ausgehandelten.**
`ht.c` und `vht.c` stehen auf 1/11 und 0/14. In Linux erzeugt
`ieee80211_ht_cap_ie_to_sta_ht_cap` / `ieee80211_vht_cap_ie_to_sta_vht_cap`
die **Schnittmenge** aus dem, was der AP kann, und dem, was wir können — und
**jedes** ADD_STA, jede TLC-Konfiguration und jede Breitenentscheidung in
iwlwifi liest ausschließlich diese Schnittmenge (`sta->deflink.ht_cap`,
`link_sta->bandwidth`, `link_sta->rx_nss`). Wir lesen stattdessen direkt das
Beacon-Element des AP. Das ist eine ganze Schicht, kein Feld.

**2. Der Sendepfad ist der am wenigsten portierte Teil.**
`mvm/tx.c` 3/42, `mac80211/status.c` 0/23, `mac80211/wme.c` 0/6. Wir haben:
Frame bauen, absenden, Erfolg zählen. Wir haben nicht: Ratenwahl je Frame,
Krypto-/PN-Felder, TID-Auswahl, QoS-Control, A-MSDU, Sendestatus-Auswertung,
Reclaim über `scd_ssn`. Alles, was zwischen „Frame gebaut" und „Frame
quittiert" liegt, ist bei uns ein Sonderfall des Erfolgs.

**3. Es gibt keine Verbindungsaufsicht.**
`mlme.c` 5/183, `power.c` 5/28 ohne Beacon-Filter, `rxmq.c` ohne
`rx_beacon_filter_notif`. Wir merken einen toten Link erst, wenn gar nichts
mehr kommt.

## Teil 6 — Reihenfolge, die daraus folgt

Nach Hebel, jeweils eine ganze Funktion, nicht ein Feld:

1. **`ieee80211_ht_cap_ie_to_sta_ht_cap` + `ieee80211_vht_cap_ie_to_sta_vht_cap`**
   (`ht.c`, `vht.c`) — die ausgehandelte Schnittmenge herstellen und ALLE
   Stationsbefehle daraus speisen. Das ersetzt die heutigen Einzelgriffe ins
   AP-Element und ist Voraussetzung dafür, dass `iwl_mvm_get_sta_ampdu_dens`
   überhaupt sinnvoll portierbar ist.
2. **`mvm/tx.c` Sendestatus** — `rx_tx_cmd_single` mit allen
   `TX_STATUS_FAIL_*`-Zweigen, `get_scd_ssn`, `tx_reclaim`.
3. **`mac80211/wme.c`** — `select_queue` + `set_qos_hdr`, dazu je TID eine
   Queue (`mvm/sta.c: sta_alloc_queue_tvqm`).
4. **Beacon-Filter** (`power.c`) + `rx_beacon_filter_notif` (`rxmq.c`) +
   `ieee80211_mgd_probe_ap` (`mlme.c`) — Verbindungsaufsicht.
5. **A-MSDU** — `mvm/tx.c: tx_tso`, `max_amsdu_size`, `rs_fw_get_max_amsdu_len`,
   RX-Seite in `rxmq.c`.
6. **HE** — `he.c` ganz, plus HE-Raten im TLC.
7. **Sendeleistung** — `fw/acpi.c`, `fw/uefi.c`, `fw/regulatory.c`.

Die Aggregationsfrage (`aggregated 0`) hängt an 1 und 2: solange die Firmware
Rohwerte des AP als Stationsfähigkeiten bekommt und wir den Sendestatus nur
im Erfolgsfall auswerten, fehlt ihr die Grundlage für eine Sitzung — und uns
der Blick darauf, warum.
