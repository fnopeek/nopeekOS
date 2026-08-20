# AX200-Treiber gegen Linux — die Karte

**Stand:** 2026-08-20 · wifi_ax200 0.95.0 · Referenz: Linux **6.18.26**,
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

### C1. TX-Aggregation — die Firmware macht sie selbst, und wir sehen nicht nach

> **Korrektur 2026-08-20, gleicher Tag.** Hier stand zuerst: „TX-Aggregation ist
> per `tid_disable_tx = 0xffff` dauerhaft abgeschaltet." **Das ist falsch.**
> Weiterlesen in Linux hat es widerlegt, und der falsche Befund war schon
> committet. Was wirklich gilt:

Unsere Firmware hat **TLC-Offload** (`IWL_UCODE_TLV_CAPA_TLC_OFFLOAD` — wir
schicken `TLC_MNG_CONFIG_CMD`, und die Ratensteuerung läuft). Für genau diesen
Fall setzt iwlwifi

```c
/* mvm/mac80211.c:396 */
if (iwl_mvm_has_tlc_offload(mvm)) {
        ieee80211_hw_set(hw, TX_AMPDU_SETUP_IN_HW);
        ieee80211_hw_set(hw, HAS_RATE_CONTROL);
}
```

`IEEE80211_HW_TX_AMPDU_SETUP_IN_HW` heißt laut `mac80211.h:2711`: *„The device
handles TX A-MPDU session setup strictly in HW. mac80211 should not attempt to
do this in software."* Und `ieee80211_start_tx_ba_session` weigert sich dann
auch:

```c
/* net/mac80211/agg-tx.c:626 */
if ((tid >= IEEE80211_NUM_TIDS) ||
    !ieee80211_hw_check(&local->hw, AMPDU_AGGREGATION) ||
    ieee80211_hw_check(&local->hw, TX_AMPDU_SETUP_IN_HW))
        return -EINVAL;
```

Damit wird `IEEE80211_AMPDU_TX_OPERATIONAL` nie erreicht,
`iwl_mvm_sta_tx_agg_oper` nie aufgerufen (seine eigene erste Zeile sagt das:
`WARN_ON_ONCE(iwl_mvm_has_tlc_offload(mvm))`), `iwl_mvm_sta_tx_agg` nie — und
**Linux lässt `tid_disable_agg` auf dieser Firmware genauso bei `0xffff`
stehen wie wir.** Es ist kein Schalter, den wir vergessen haben. Die Firmware
handelt die ADDBA-Sitzung selbst mit dem AP aus.

Ebenso hinfällig: „45 069 Einzel-Frames". `tx frames` zählt, was wir der
Firmware **übergeben** haben — ein Frame pro Paket, aggregiert oder nicht. Die
Aggregation passiert unter uns. Die Zahl sagt über sie gar nichts.

**Was wirklich offen ist: wir sehen nicht nach.** Zwei Zeugen liegen bereit und
wurden nie gelesen:

- `frame_count` in **jeder** TX-Antwort — „1 no aggregation, >1 aggregation"
  (`fw/api/tx.h:497`). Die Konstante `TXR_OFF_FRAME_COUNT` steht seit dem
  ersten Port in `regs.rs`; der Compiler meldet sie als *never used*.
- **`BA_NOTIF` (0xc5)**, `struct iwl_compressed_ba_notif` — pro Aggregat mit
  `txed` und `done`. Treffer im Treiber vor 0.93.0: **null**. Wir haben die
  Meldung schlicht nicht angesehen.

**wifi_ax200 0.93.0** liest beide und meldet sie, ohne irgendein Verhalten zu
ändern:

```
tx agg   subframes N over M resp; aggregated K max X  ba-notif B txed T done D
```

`aggregated 0, max 1, ba-notif 0` ⇒ die Firmware aggregiert nicht, und **dann**
ist zu suchen, warum. `max 20` ⇒ sie aggregiert längst, der Engpass liegt
woanders, und C4 rückt nach vorn. Erst messen, dann bauen.

### C1b. GEMESSEN: sie aggregiert nicht — und wir schreiben die Sequenznummer

wifi_ax200 0.93.0 am Gerät, VHT80, 200 MB:

```
tx agg   subframes 21583 over 21583 resp; aggregated 0 max 1  ba-notif 0 txed 0 done 0
```

**Jede** der 21 583 TX-Antworten meldet `frame_count = 1`, keine einzige
Block-Ack-Meldung. Die Firmware aggregiert nichts.

Aus der Firmware-TLV selbst gelesen (`iwlwifi-cc-a0-77.ucode`, CAPA-Wörter
`0x9def037f 0xbf7ffaee 0xdb91eedb 0x002052ae`): **Bit 43
`IWL_UCODE_TLV_CAPA_TLC_OFFLOAD` = gesetzt.** Der Aufbau liegt also tatsächlich
bei der Firmware — C1 gilt.

Die Abweichung von Linux, die dazu passt, steht in `iwl_mvm_tx_mpdu`
(`mvm/tx.c:1174`) — mit einem ausdrücklichen Guard:

```c
if (ieee80211_is_data_qos(fc) && !ieee80211_is_qos_nullfunc(fc)) {
        seq_number = mvmsta->tid_data[tid].seq_number;
        seq_number &= IEEE80211_SCTL_SEQ;

        if (!iwl_mvm_has_new_tx_api(mvm)) {          /* <- nur ALTE TX-API */
                hdr->seq_ctrl &= cpu_to_le16(IEEE80211_SCTL_FRAG);
                hdr->seq_ctrl |= cpu_to_le16(seq_number);
        }
}
```

Auf der **neuen** TX-API — unser gen2-Gerät — schreibt Linux die Sequenznummer
eines QoS-Frames **nicht**. Die Firmware besitzt sie, und sie besitzt sie, weil
das Block-Ack-Fenster darauf aufbaut. In den Gen2-TX-Flags (`IWL_TX_FLAGS_*`)
gibt es dafür auch kein Bit — es ist schlicht die Firmware.

Wir haben sie in jedes Frame geschrieben. Der Kommentar an der Stelle sagte:
*„writing our own costs nothing and covers us if it does not."* Gemessen kostet
es `aggregated 0 of 21583`.

**wifi_ax200 0.94.0** überlässt sie bei QoS-Frames der Firmware; Nicht-QoS-Daten
bekommen weiter eine von uns, weil darunter kein mac80211 liegt, das sie schon
vergeben hätte. Ob das die ganze Ursache ist, entscheidet die nächste `tx
agg`-Zeile — nicht dieser Absatz.

### C1c. Der MAC-Kontext sagte der Firmware nie, dass dies ein QoS/HT-BSS ist

0.94.0 hat die Sequenznummer der Firmware überlassen. Ergebnis am Gerät:

| | 0.93.0 | 0.94.0 |
|---|---|---|
| `dups` | 1930 | **0** |
| `retries` | 2968 (13 %) | **801 (3 %)** |
| `rts-fail` | 12217 | 7195 |
| `tx drops full` | 25 | **0** |
| `longest silence` | 13 ms | 3 ms |
| `aggregated` | 0 | **0** |

Ein echter Fehler weniger — der AP schickt kein einziges Frame mehr doppelt —
aber nicht die Aggregation.

Die fehlt woanders. `struct iwl_mac_ctx_cmd` trägt ab Offset 56 den
QoS-Block:

```c
	__le32 filter_flags;                   /* 52 */
	/* MAC_QOS_PARAM_API_S_VER_1 */
	__le32 qos_flags;                      /* 56 */
	struct iwl_ac_qos ac[AC_NUM + 1];      /* 60 .. 100 */
```

**Wir haben beide seit dem ersten Port auf null gelassen.** Unsere `MC_OFF_*`
sprangen von `PROT_FLAGS` (40) direkt auf `FILTER_FLAGS` (52) und dann auf
`STA_IS_ASSOC` (100) — der ganze Block dazwischen blieb leer. Linux füllt ihn
in `iwl_mvm_set_fw_qos_params` (`mvm/mac-ctxt.c:475`):

```c
	for (i = 0; i < IEEE80211_NUM_ACS; i++) {
		u8 txf = iwl_mvm_mac_ac_to_tx_fifo(mvm, i);
		u8 ucode_ac = iwl_mvm_mac80211_ac_to_ucode_ac(i);
		ac[ucode_ac].cw_min = ...; ac[ucode_ac].cw_max = ...;
		ac[ucode_ac].edca_txop = cpu_to_le16(queue_params[i].txop * 32);
		ac[ucode_ac].aifsn = queue_params[i].aifs;
		ac[ucode_ac].fifos_mask = BIT(txf);
	}
	if (link_conf->qos)
		*qos_flags |= cpu_to_le32(MAC_QOS_FLG_UPDATE_EDCA);
	if (link_conf->chanreq.oper.width != NL80211_CHAN_WIDTH_20_NOHT)
		*qos_flags |= cpu_to_le32(MAC_QOS_FLG_TGN);
```

`qos_flags = 0` sagt der Firmware zwei Dinge: keine EDCA-Konfiguration, und
**`MAC_QOS_FLG_TGN` nicht gesetzt — also kein 802.11n-BSS.** Auf dieser ucode
führt die Firmware den TX-Aggregationsmanager selbst (C1); sie hat keinen
Grund, für ein BSS eine Sitzung zu eröffnen, von dem sie weiß, dass es weder
QoS noch HT ist. Dass der Empfang trotzdem aggregiert wird, passt dazu: den
ADDBA schickt dort der AP, wir vergeben nur eine BAID.

Dazu fehlte die Quelle der Werte: das **WMM-Parameter-Element** des AP
(vendor 221, OUI 00:50:F2, Typ 2, **Subtyp 1**) haben wir nie gelesen. Wir
schicken selbst nur das kürzere Information-Element im Assoc-Request.

**wifi_ax200 0.95.0** liest es (`ieee80211_sta_wmm_params`: ACI in Bit 5-6,
AIFSN im unteren Nibble, ECWmin/ECWmax im nächsten Byte, TXOP als LE16) und
füllt beides in **jeden** MAC-Kontext-Befehl, wie
`iwl_mvm_mac_ctxt_cmd_common` es tut. Neue Report-Zeile `ap qos` zeigt die
Tabelle, damit „QoS-BSS" prüfbar ist statt geglaubt.

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
| **TX-A-MPDU-Sicht** | `frame_count` (`fw/api/tx.h:497`), `iwl_mvm_rx_ba_notif` / `iwl_compressed_ba_notif` | Der Aufbau liegt bei der Firmware (C1), aber wir hatten **keinen Blick darauf**, ob sie läuft. Seit 0.93.0 gelesen und gemeldet. |
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

1. ~~Nachsehen, ob TX-Aggregation läuft~~ — **gemessen: sie läuft nicht**
   (0.93.0, `aggregated 0 of 21583`). Erste Gegenmaßnahme in **0.94.0**: die
   Sequenznummer von QoS-Frames der Firmware überlassen, wie Linux es auf der
   neuen TX-API tut (C1b). Messen. Bleibt `aggregated 0`, ist der nächste
   Verdächtige die A-MPDU-Größe aus der VHT- statt der HT-Fähigkeit
   (`iwl_mvm_get_sta_ampdu_dens`) und danach `uapsd_acs`/`sp_length`, die wir
   im ADD_STA gar nicht setzen.
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
