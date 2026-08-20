# AX200-Treiber gegen Linux — die Karte

**Stand:** 2026-08-20 · wifi_ax200 0.92.0 · Referenz: Linux **6.18.26**,
`~/.cache/nopeekos/linux-src/linux-6.18.26/` (iwlwifi 290 Dateien + mac80211 +
cfg80211, siehe `memory/project_wifi_linux_gap_audit.md`).

Zweck: Schluss mit Einzelsymptomen. Drei Spalten — **was 1:1 aus Linux kommt**,
**wo eine Zahl von uns stammt statt von Linux**, **was ganz fehlt**. Jede
Behauptung hier ist gegen die Quelle geprüft; wo nicht, steht es dabei.

Gemessen am Gerät (200 MB, `netbench get`, FRITZ-Repeater, rssi −56…−59):

| | 20 MHz | HT40 | VHT80 |
|---|---|---|---|
| Durchsatz | 66–72 Mbit | 68 Mbit | 55 / 49 Mbit |
| PHY rx / tx | 144 / 144 | 240 / 300 (2ss) | 520 / 433 (**1ss**) |
| `air rx` | 66 % | 49 % | 32 % |

Die Breite ist nicht der Engpass — der Kanal ist bei VHT80 zu 61 % leer und wir
werden langsamer. Diese Karte sagt, warum.

---

## A — Sauber 1:1 portiert

Verifiziert gegen die genannte Funktion, inklusive der Verzweigungen, nicht nur
des Zweigs der bei uns lief.

| Bereich | Linux | Stand |
|---|---|---|
| Bus/Reset/APM | `iwl_pcie_set_hw_ready`, `prepare_card_hw`, `sw_reset`, `clear_persistence_bit`, `apm_config`, `apm_init`, `gen2_apm_init`, `_iwl_trans_pcie_start_hw` | vollständig |
| Firmware-Laden | `iwl_pcie_init_fw_sec`, `iwl_pcie_ctxt_info_init`, `iwl_alive_fn` (v6-Pfad) | vollständig |
| Kommandopfad | `iwl_pcie_gen2_enqueue_hcmd`, Wide-Header, cmd_ver aus der TLV | vollständig |
| RX-Ring | `iwl_pcie_gen2_rx_init`, `iwl_pcie_rxmq_restock`, `iwl_pcie_rx_handle`, `iwl_pcie_get_rxb`, `iwl_rx_mem_buffer.invalid`, Doppelmaske in `closed_rb` | vollständig, seit 0.83.0 |
| TX gen2 | `iwl_txq_gen2_init`, `iwl_txq_gen2_tx`, `iwl_txq_gen2_build_tx` | Rahmenbau vollständig |
| TX-Antwort | `struct iwl_tx_resp` — `failure_rts`@2, `failure_frame`@3, `byte_cnt`@30, `status`@40, Länge 44 | Layout geprüft, **Auswertung nur der Erfolgsfall** (→ D) |
| NVM/MAC | `iwl_get_nvm`, `iwl_set_hw_address_from_csr`, `iwl_flip_hw_address` | vollständig |
| Scan | `iwl_mvm_scan_umac_v14_and_above` (v15) | vollständig |
| PHY-Kontext | `iwl_mvm_phy_ctxt_cmd_data`, `iwl_mvm_get_ctrl_pos`, ULTRA_HB-Kanalfeld | vollständig |
| Breite | `ieee80211_determine_ap_chan` inkl. HE-Operation-VHT-Info (0.92.0) | vollständig für HT/VHT; EHT/6 GHz fehlen (kein 6E-Gerät) |
| Station | `iwl_mvm_sta_send_to_fw` — FAT_EN, MIMO_EN aus `rx_nss`, AGG_SIZE/DENS, **kein** `RTS_MIMO_PROT` (setzt Linux nur bei SMPS *dynamic*) | Flags korrekt |
| Schutz | `iwl_mvm_set_fw_protection_flags` | vollständig |
| RX-Aggregation | `ieee80211_process_addba_request`, `agg-rx.c` ganz gelesen, 8 TIDs, `iwl_mvm_fw_baid_op` **beide** Zweige (BAID_ML + ADD_STA) | vollständig |
| Ratensteuerung | `iwl_mvm_rs_fw_rate_init` (TLC-Offload), `rate_n_flags` v2→v3 (`iwl_v3_rate_from_v2_v3`) | vollständig |
| Init-Sequenz | `TX_ANT`, `BT_CONFIG`, `SOC_LATENCY`, `DQA_ENABLE`, `POWER_TABLE`, `MCC_UPDATE`, `SCAN_CFG`, `RLC_CONFIG` v2 | siehe D für das, was Linux zusätzlich schickt |

---

## B — Unsere Zahl statt Linux'

Das sind **keine** Portierungsfehler, sondern bewusste eigene Werte. Sie stehen
hier, damit niemand sie für Linux hält.

| Konstante | Unser Wert | Linux | Wirkung |
|---|---|---|---|
| `TX_INFLIGHT_MAX` | **48** | **gibt es nicht** — Linux nutzt `ieee80211_stop_queue`/`wake_queue` | Reine Eigenkonstruktion gegen Bufferbloat. Am Gerät steht sie am Anschlag (`inflight peak 48`, `blocked 282`), ist also heute die Sendeschranke. |
| `IWL_DATA_QUEUE_SIZE` | **64** | `IWL_DEFAULT_QUEUE_SIZE` = **256**, für HE **1024** (`fw/api/txq.h:89-91`) | Vier- bis sechzehnfach zu klein. Mit TX-Aggregation zwingend zu erhöhen. |
| `RX_NUM_RBS` | **512** | `num_rx_bufs - 1` = **2047** (`iwl_pcie_rx_init`) | Aktuell nicht bindend (`drain-peak 64/512`, `pool-exhausted 0`), aber Reserve verschenkt. |
| `BA_WIN` | **32** | `IEEE80211_MAX_AMPDU_BUF_HT` = **64** (HE: 256) | Halbes Empfangsfenster pro TID. Verdoppeln kostet 8 × 32 × 1600 B zusätzliches .bss. |
| `TX_PAYLOAD_STRIDE` | **2048** | Frame bis 11 KB (A-MSDU) | Deckelt jedes Sende-Frame auf ~2 KB → **A-MSDU-Senden strukturell unmöglich**. |
| `STALL_MS` (ba.rs) | **60** | mac80211: Reorder-Timeout aus dem ADDBA-Timeout des Peers | Eigene Zahl, nie gegen Linux geprüft. |
| `MAX_APS` | 64 | cfg80211: unbegrenzt | unkritisch |
| `restock_all_rbs` | eigener Rettungspfad | **existiert in Linux nicht** | War bis 0.83.0 selbst die Ursache eines Totalausfalls. Bleibt ein Fremdkörper. |
| Luftzeit-Schätzung | Frame-für-Frame geschätzt | Linux schätzt nicht, es liest `wireless_media_time` | Unsere `air`-Zeile ist eine Rechnung, keine Messung. Zweimal schon falsch gewesen. |
| `TX_WD_TIMEOUT_MS` | 10 000 | `IWL_LONG_WD_TIMEOUT` = 10 000 | ✔ inzwischen Linux' Wert; die Reaktion ist aber unsere (Slots zurückgeben statt Firmware-Neustart). |

---

## C — Der Deckel, der uns heute wirklich bremst

### C1. TX-Aggregation ist in der Firmware ABGESCHALTET — dauerhaft

`iwl_mvm_sta_send_to_fw` schickt `tid_disable_tx = mvm_sta->tid_disable_agg`.
Der Startwert ist `0xffff` („No aggs at first", `sta.c:1779`) und wird pro TID
**gelöscht**, sobald mac80211 eine TX-Session öffnet:

```c
/* iwl_mvm_sta_tx_agg, mvm/sta.c:3020 */
if (start) {
        mvm_sta->tfd_queue_msk |= BIT(queue);
        mvm_sta->tid_disable_agg &= ~BIT(tid);
}
cmd.modify_mask |= STA_MODIFY_TID_DISABLE_TX;
cmd.tid_disable_tx = cpu_to_le16(mvm_sta->tid_disable_agg);
```

Wir schicken `TID_DISABLE_AGG_INIT = 0xffff` in **allen drei** ADD_STA-Aufrufen
(`connect_add_station`, `sta_assoc_update`, `retarget_station`) und löschen es
**nie**. Es gibt keinen `iwl_mvm_sta_tx_agg`-Gegenpart im Treiber.

Folge: **jedes** gesendete Frame ist ein einzelnes MPDU mit eigener Präambel,
eigenem SIFS, eigenem ACK, eigenem DIFS und eigenem Backoff. Am Gerät waren das
im VHT80-Lauf **45 069 Einzel-Frames** — fast alles TCP-ACKs. Nicht die
Luftzeit ist das Problem (7–10 %), sondern dass wir für jedes ACK einzeln um
das Medium kämpfen. Das ist die Quelle der Latenz, und Latenz ist bei
`cwnd/RTT` der Durchsatz.

Das erklärt auch, warum `tx drops full` bei jeder Breite auftaucht
(20 MHz: 620 · HT40: 583 · VHT80: 13 bei entsprechend kleinerem Durchsatz):
der Deckel ist breitenunabhängig, weil er nicht in der Luft sitzt.

### C2. A-MSDU-Empfang fehlt → wir löschen das Bit in der ADDBA-Antwort

Wir können kein A-MSDU auspacken, also verbieten wir es dem AP. Damit trägt
jedes MPDU genau eine MSDU. Im VHT80-Lauf: **342 578 RX-Frames für 400 MB**,
also 1,2 KB pro Frame. Mit A-MSDU wären es grob halb so viele MPDUs für
dieselben Bytes — halb so viel Medienzugriff, halb so viele Block-ACKs.

### C3. Eine einzige Datenqueue, ein einziger TID

`alloc_data_queue` legt **eine** Queue an. Linux' DQA (`iwl_mvm_sta_alloc_queue`)
vergibt eine Queue je TID/AC. Bei uns teilen sich Nutzdaten und ACKs eine
Queue und eine Zugangsklasse — und `TX_INFLIGHT_MAX` gilt für beides zusammen.

### C4. Der TFD-Lesezeiger läuft in Reihenfolge

`data_in_flight = (write_ptr - read_ptr) & 63`. Ein Frame, das die Firmware nie
quittiert, hält den Lesezeiger an — **alles dahinter zählt als unterwegs**.
`inflight` klemmt bei 48, `blocked` feuert, Senden steht bis der 10-s-Wachhund
räumt. Am Gerät genau einmal passiert (`WD-RECLAIM 1`), und der Lauf war 30 s
lang. Linux behandelt das in `iwl_mvm_rx_tx_cmd_single` über die
`TX_STATUS_FAIL_*`-Zweige, die wir nicht haben (→ D).

---

## D — Was ganz fehlt

### Durchsatz-relevant

| Fehlt | Linux | Was es kostet |
|---|---|---|
| **TX-A-MPDU** | `iwl_mvm_mac_ampdu_action` / `IEEE80211_AMPDU_TX_START` → `iwl_mvm_sta_tx_agg` | siehe C1 — der größte Posten |
| **A-MSDU** rx+tx | `iwl_mvm_rx_mpdu_mq` AMSDU-Pfad, `IWL_UCODE_TLV_CAPA_AMSDU_IN_AMPDU` | siehe C2 |
| **HE (802.11ax)** | `iwl_mvm_cfg_he_sta`, HE-Cap im Assoc-Request, `RATE_MCS_MOD_TYPE_HE`, HE-TLC | Treffer für HE im Treiber: **0** (die Konstante existiert, wird nirgends benutzt). Die AX200 ist 2x2 Wi-Fi 6: HE80 2ss MCS11 = **1201 Mbit** gegen VHT80 MCS9 = 866. Wichtiger als die rohe Zahl: HE-Symbole sind 12,8 µs statt 3,2 — deutlich robuster bei genau dem Rand, an dem uns VHT80 gerade wegbricht. |
| **Mehrere TX-Queues (DQA)** | `iwl_mvm_sta_alloc_queue`, `iwl_mvm_tx_mpdu` TID-Auswahl | siehe C3 |
| **`TX_STATUS_FAIL_*`-Auswertung** | `iwl_mvm_rx_tx_cmd_single` | siehe C4; wir sehen nur `status == SUCCESS` |
| **Sendeleistungs-Tabellen** | `iwl_mvm_sar_init`, `sar_geo_init`, `ppag_init`, `tas_init`, `lari_cfg`, `sgom_init`, `uats_init` | Alle sieben fehlen. Die Firmware fällt auf NVM-Vorgaben zurück. Wieviel dB das kostet: **ungeprüft** — messbar über `rssi` gegen einen Linux-Client am selben Ort. |

### Verbindungspflege

| Fehlt | Linux |
|---|---|
| Connection Monitor (Null-Data-Probe, `ieee80211_connection_loss`) | `ieee80211_mgd_probe_ap` |
| Beacon-Filter-Konfiguration (deshalb `fw notifs 0`, missed-beacon feuert nie) | `iwl_mvm_beacon_filter_send_cmd` |
| Power-Save / DTIM (wir fahren CAM) | `iwl_mvm_power_update_mac` |
| SMPS-Behandlung | `iwl_mvm_update_smps` |
| 802.11v BTM / Steering | `ieee80211_process_neighbor_report` |
| AP-Wahl kennt Mesh-Backhaul nicht — nur RSSI + Band | cfg80211-BSS-Auswahl |
| Reconnect nutzt MODIFY statt Remove/Re-Add (bewusst, wegen DMA-Churn) — BAID wird dabei nicht freigegeben | `iwl_mvm_rm_sta` |

### Sicherheit (aus `project_wifi_linux_gap_audit.md`, unverändert offen)

- SNonce ist ein fester Wert (`0x5a ^ mac ^ index`) — es gibt kein `npk_random`
- Hardware-Schlüsselplätze fest verdrahtet statt `iwl_mvm_set_fw_key_idx`
- RSC beim Schlüsselsetzen fest null statt Key-RSC aus der EAPOL-Nachricht
- Entschlüsselungsstatus wird gezählt, nicht **erzwungen** (`iwl_mvm_rx_crypto` verwirft bei fehlendem `MIC_OK`, rxmq.c:452)

### Sonstiges, Wirkung ungeprüft

`iwl_mvm_sf_update` (Smart FIFO) · `iwl_mvm_config_ltr` · Thermik
(`temp_report_ths`, `tt_tx_backoff`, `ctdp_command`) · RSS / mehrere RX-Queues
(`iwl_configure_rxq`, `iwl_send_rss_cfg_cmd`) · `iwl_mvm_ppag_init`

---

## E — Reihenfolge

Nach erwartetem Ertrag, jeweils **eine Änderung, eine Gerätemessung**
(`netbench get 192.168.178.97 /get?mb=200` + `wlan`).

1. **TX-A-MPDU** (C1). `iwl_mvm_sta_tx_agg` portieren: ADDBA-Request an den AP
   senden, `tid_disable_agg` per TID löschen, `STA_MODIFY_TID_DISABLE_TX` im
   modify_mask, TX-BA-Session-Zustand führen. Dazu `IWL_DATA_QUEUE_SIZE` auf
   Linux' 256. Erwartung: die 45 069 Einzel-Frames werden ein Bruchteil davon,
   RTT fällt, `cwnd/RTT` steigt.
2. **A-MSDU-Empfang** (C2). `iwl_mvm_rx_mpdu_mq`-AMSDU-Pfad, danach das Bit in
   der ADDBA-Antwort stehen lassen.
3. **`BA_WIN` 32 → 64** (B). Eine Zeile plus .bss.
4. **`TX_STATUS_FAIL_*`** (C4). Damit hört der 10-s-Wachhund auf, der einzige
   Umgang mit einem verlorenen Frame zu sein.
5. **HE** (D). Eigenes Projekt: HE-Cap ins Assoc-Request, HE-Ratentabelle,
   HE_CAP aus dem Beacon, TLC im HE-Modus, `RATE_MCS_MOD_TYPE_HE` im Dekoder.
6. **Sendeleistungs-Tabellen** (D). Vorher messen, ob überhaupt etwas fehlt:
   `rssi` gegen einen Linux-Client am selben Ort.

Bis 1 und 2 stehen, ist die Breitenfrage müßig — bei VHT80 ist der Kanal zu
61 % leer und wir werden trotzdem langsamer. Alltagseinstellung solange:
`ht40: on`, `vht: off` (68 Mbit, `retrans 7`, `rtt 34 ms`, TX mit zwei Strömen).
