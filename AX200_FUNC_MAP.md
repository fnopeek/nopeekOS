# AX200 Funktions-Map — Linux `iwlwifi-mvm` ↔ unser `wifi_ax200.wasm`

Ziel: systematischer Überblick, **was im Linux-Treiber existiert und was wir nachgebaut
haben** — damit wir Gaps sehen, bevor sie zu stundenlangen Debug-Sessions werden
(Lektion: dreimal an einem fehlenden Befehl der mandatorischen Sequenz verloren).

- Quelle: `~/.cache/nopeekos/linux-src/linux-6.18.26/drivers/net/wireless/intel/iwlwifi/`
- Familie **22000 / gen2 / non-MLD** → die `mvm/*.c`-Dateien (NICHT `mld/*.c`).
- Unser Code: `tools/wasm/wifi_ax200/src/{lib.rs,regs.rs}`.
- Stand: **2026-06-07, wifi_ax200 0.26.0.** Bringup→Scan→Connect-TX-Pfad **HW-validiert**
  (TX-Completion `cmd=0x1c` empfangen, Auth-Frame raus).

Legende: ✅ gebaut & HW-validiert · 🟡 gebaut, HW-Test offen · 🔶 teilweise/vereinfacht ·
❌ fehlt noch · ⬜ bewusst weggelassen (no-op/nicht nötig für unseren Pfad).

---

## Phase A — Transport-Bringup (PCIe / APM / DMA / FW-Load)

| Linux (`pcie/…`, `iwl-…`) | unsere fn (`lib.rs`) | Status |
|---|---|---|
| `iwl_pcie_set_hw_ready` / `iwl_pcie_prepare_card_hw` | `set_hw_ready` / `prepare_card_hw` | ✅ |
| `iwl_trans_pcie_sw_reset` | `sw_reset` | ✅ |
| `iwl_trans_pcie_clear_persistence_bit` | `clear_persistence_bit` | ✅ |
| `iwl_pcie_apm_config` (ASPM/LTR) | `apm_config` | ✅ |
| `iwl_pcie_apm_init` / `gen2_apm_init` | `apm_init` / `gen2_apm_init` | ✅ |
| `_iwl_trans_pcie_start_hw` | `start_hw` | ✅ |
| `iwl_pcie_gen1_2_activate_nic` | `activate_nic` | ✅ |
| `iwl_pcie_gen2_rx_init` | `gen2_rx_init` | ✅ |
| `iwl_txq_gen2_init` (cmd-queue) | `txq_gen2_init` | ✅ |
| `iwl_pcie_gen2_nic_init` | `nic_init` | ✅ |
| `iwl_pcie_set_ltr` (22000) | `set_ltr` | ✅ |
| `iwl_pcie_init_fw_sec` (ctxt-info) | `init_fw_sec` | ✅ |
| `iwl_pcie_ctxt_info_init` + start_fw | `load_firmware` | ✅ |
| PRPH/HBUS-Zugriff, `grab_nic_access`, `read_mem` | `prph_read/write`, `grab_nic_access`, `read_mem` | ✅ |
| `iwl_set_hw_address_from_csr` / `iwl_flip_hw_address` | `read_mac_address` / `mac_from_regs` | ✅ |
| PNVM-Load (`iwl_pnvm_load`) | — | ⬜ `sku_id=0` → entfällt |

## Phase B — Post-ALIVE Init-Handshake + NVM

| Linux (`mvm/fw.c`, `iwl-nvm-parse.c`) | unsere fn | Status |
|---|---|---|
| `iwl_alive_fn` (ALIVE-Notif parse, v6) | `parse_alive_ntf` | ✅ |
| `iwl_pcie_gen2_enqueue_hcmd` (gen2 hcmd-TX) | `send_hcmd` | ✅ |
| RX-Drain / Notif-Wait | `drain_rx_until` / `wait_rx` / `pump_rx` | ✅ |
| `iwl_run_unified_mvm_ucode` Init-Tail (INIT_EXTENDED_CFG, NVM_ACCESS_COMPLETE) | `run_init_handshake` | ✅ |
| `iwl_get_nvm` / `iwl_mvm_nvm_get_info` | `read_nvm` | ✅ |
| `iwl_init_channel_map` (regulatory channel_profile) | `read_nvm` (Kanalliste) | ✅ |
| `iwl_pcie_rxmq_restock` (RB-Recycle) | `rx_restock_and_alive` / `recycle_rb` / `flush_free_bd` | ✅ |
| `iwl_mvm_sf_update` (Spatial Filter) | — | ⬜ best-effort, geparkt |

## Phase C — `iwl_mvm_up` Pre-Scan-Sequenz

| Linux (`mvm/…`) | unsere fn | Status |
|---|---|---|
| `iwl_send_tx_ant_cfg` (TX_ANT 0x98) | `run_scan_prereqs` | ✅ |
| `iwl_mvm_send_bt_init_conf` (BT_CONFIG 0x9b) | `send_bt_init` | ✅ |
| `iwl_set_soc_latency` (SOC_CONFIG g2/0x01) | `send_soc_latency` | ✅ |
| `iwl_mvm_send_dqa_cmd` (DQA_ENABLE) | `send_dqa` | 🔶 cap-gated, FW=no-DQA → no-op |
| `iwl_mvm_power_update_device` (POWER_TABLE 0x77) | `send_power` | ✅ |
| `iwl_mvm_init_mcc` / `update_mcc` (MCC_UPDATE 0xc8) | `set_regulatory` | ✅ |
| `iwl_mvm_config_scan` (SCAN_CFG 0x0c) | `run_scan_prereqs` | ✅ |
| `iwl_mvm_mac_ctxt_add` (MAC_CONTEXT add, BSSID broadcast) | `add_mac_context` | ✅ |
| BIOS/ACPI-gated (lari/ppag/sar/sgom/tas) | — | ⬜ keine Platform-Tabellen → senden nichts |
| `iwl_mvm_config_ltr`, `sf_update`, `tt_tx_backoff` | — | ⬜ best-effort, non-fatal |

## Phase D — Scan

| Linux (`mvm/scan.c`, `rxmq.c`) | unsere fn | Status |
|---|---|---|
| `iwl_mvm_scan_umac_v14_and_above` (SCAN_REQ_UMAC v15) | `build_scan_cmd` | ✅ |
| `iwl_mvm_reg_scan_start` / Scan-Loop | `run_scan` | ✅ |
| `iwl_mvm_rx_umac_scan_complete_notif` (SCAN_COMPLETE 0xc1-Pfad) | `run_scan` (Notif-Match) | ✅ |
| `iwl_mvm_rx_mpdu_mq` (Beacon → BSS) | `parse_beacon` | ✅ |
| `iwl_pcie_rx_handle` (mq-RX-Ring drain) | `service_rx` | ✅ |
| `iwl_mvm_sched_scan` / EBS / Scan-Abort | — | ❌ nicht nötig (One-Shot-Scan) |

## Phase E — Connect: Chanctx + Station (5a/5b/5b′) — **TX-Pfad lebt ✅**

| Linux | FW-Cmd | unsere fn | Status |
|---|---|---|---|
| `iwl_mvm_phy_ctxt_add/changed` | PHY_CONTEXT 0x08 | `connect_phy_binding` | ✅ |
| `iwl_mvm_phy_ctxt_set_rxchain` (RLC) | RLC_CONFIG g5/0x08 | `connect_phy_binding` | ✅ |
| `iwl_mvm_binding_add_vif` | BINDING_CONTEXT 0x2b | `connect_phy_binding` | ✅ |
| `iwl_mvm_add_sta` (AP-Peer, LINK) | ADD_STA 0x18 | `connect_add_station` | ✅ |
| gen2 TX-Queue (`iwl_mvm_tvqm_enable_txq`) | SCD_QUEUE_CONFIG g5/0x17 | `connect_add_station` | ✅ |
| `iwl_mvm_power_update_mac` | MAC_PM_POWER_TABLE 0xa9 | `connect_finish_chanctx` | ✅ |
| `iwl_mvm_mac_ctxt_changed` (MODIFY, AP-BSSID) | MAC_CONTEXT 0x28 | `connect_finish_chanctx` | ✅ |
| `iwl_mvm_update_quotas` | TIME_QUOTA 0x2c | — | ⬜ STA skippt's pre-assoc (nur Monitor) |
| `iwl_mvm_rs_rate_init` (Legacy-Rate) | — (host) | — | 🔶 wir nutzen host-rate fürs Auth-Frame |

## Phase F — Connect: Auth-TX + Session-Protection (5c) — **HW-validiert ✅**

| Linux | FW-Cmd | unsere fn | Status |
|---|---|---|---|
| `iwl_mvm_mac_mgd_prepare_tx` → `protect_assoc` → `schedule_session_protection` | SESSION_PROTECTION g3/0x05 | `connect_send_auth` (Kopf) | ✅ |
| `iwl_mvm_set_tx_params` (tx_cmd_v9 bauen) | — | `connect_send_auth` | ✅ |
| `iwl_txq_gen2_tx` / `_build_tx` (TFD/TB/bc/Doorbell) | TX_CMD 0x1c | `connect_send_auth` | ✅ |
| `iwl_mvm_rx_tx_cmd` (TX-Completion 0x1c) | — | `connect_send_auth` (txdiag) | ✅ (Completion empfangen) |
| `iwl_mvm_mac_mgd_complete_tx` (bei Fail: stop session prot) | — | — | ❌ (Cleanup-Pfad) |

---

## ▶▶ NOCH NICHT GEBAUT — die offenen Gaps (Connect-to-Authorized)

### Phase G — Auth-Response + Assoc (5d) ✅ HW-VALIDIERT (0.27.0, aid=2 gegen WPA2-AP)
Erster echter **Empfangs→Sende-Dialog**. Gemeinsamer TX-Helper `tx_mgmt_frame` +
RX-Parse `rx_mgmt_for_us`/`wait_mgmt_response`.
| Linux | FW-Cmd | unsere fn | Status |
|---|---|---|---|
| `iwl_mvm_rx_mpdu` → **Auth-Response parsen** (seq2/status0) | — | `connect_send_auth` | ✅ |
| **Assoc-Request bauen + TX** (SSID, Rates, ExtRates, RSN für WPA2) | TX_CMD 0x1c | `connect_send_assoc` | ✅ |
| **Assoc-Response parsen** → status + AID | — | `connect_send_assoc` | ✅ (aid=2) |
| HT/VHT/HE-Caps im Assoc-Req | — | — | ❌ (legacy-Assoc erst; bei Reject nachrüsten) |
| `iwl_mvm_sta_state` AUTH→ASSOC: `iwl_mvm_update_sta` (ADD_STA modify) | ADD_STA 0x18 | — | ❌ |
| MAC_CONTEXT MODIFY `is_assoc=1` (braucht dtim aus Assoc) | MAC_CONTEXT 0x28 | — | ❌ |

### Phase H — WPA2 4-Way-Handshake + Keys (5e) 🟡 Crypto-Foundation gebaut (wifid 0.1.0)
In **`wifid.wasm`** (Supplicant, vendor-unabhängig, aml-Struktur core/wasm/harness).
| Schritt | Cmd | Status |
|---|---|---|
| **`wifid_core` Crypto** (SHA1/HMAC/PBKDF2→PMK, PRF→PTK) | — | ✅ std-getestet (IEEE-802.11i-Vektor) |
| wifid liest SSID+PSK (2 npkFS-Objekte), leitet PMK ab | — | ✅ HW (PMK auf HP, IvyPie_New) |
| wifid Control-Channel-Client (send_cmd/poll_event, NETCTL) | — | 🟡 (NETCTL nur via Autostart) |
| **AES-128 + AES-Key-Unwrap (RFC 3394)** für GTK-unwrap | — | ❌ core nächste Slice |
| **AES-128 + AES-Key-Unwrap (RFC 3394)** für GTK-unwrap | — | ✅ std-getestet (FIPS-197/RFC-3394) |
| **4-Way-State-Machine** (`eapol::Supplicant`: msg1→PTK, msg2+MIC, msg3 GTK-unwrap, msg4) | — | ✅ std-getestet (msg1→msg4-Roundtrip) |
| **EAPOL-RX-Demux** (Ethertype 0x888E → wifid via send_event) | — | 🟡 gebaut (0.28.0, HW-Test offen) |
| Control-Host-Fns Treiber (`npk_wifi_poll_cmd`/`send_event`) | — | ✅ verdrahtet (0.28.0) |
| **EAPOL-RX-Demux** + **EAPOL-TX** (Daten-Queue tid 0 + LLC/SNAP) | TX_CMD 0x1c | ✅ HW (4-Way komplett!) |
| `iwl_mvm_send_sta_key`: **PTK/GTK install** | ADD_STA_KEY 0x17 (cmd_ver 3, 76 B) | ✅ HW |
| wifid resident 4-Way (msg1→4, MIC, GTK-unwrap) | — | ✅ HW (AP akzeptierte!) |
| `*** AUTHORIZED ***` — Keys drin, link up | — | ✅ HW |
| `is_assoc=1` war NICHT nötig (FW akzeptierte Daten-TX so) | — | ✅ (gut zu wissen) |
| `mvmvif->authorized=1` + MAC_CONTEXT is_assoc=1 | MAC_CONTEXT 0x28 | ❌ slice C |

### Phase I — IP-Daten-Pfad (echtes Internet) 🟡 Frames fließen, noch keine IP ← MORGEN
Keys + authorized; Daten-Pfad gebaut (wifi_ax200 0.30.3 + Kernel 0.207.0). DHCP läuft noch nicht durch.
| Linux | unsere fn | Status |
|---|---|---|
| RX-Data → `npk_netdev_submit_rx` (EAPOL/IP/Broadcast-Split) | `rx_classify` | 🟡 HW (RX-Events kommen) |
| `npk_netdev_poll_tx` → verschlüsseltes 802.11-Data (toDS+LLC, no ENCRYPT_DIS) | `tx_eth`/`tx_8023`/`tx_raw` | 🟡 HW (TX-Discover raus) |
| Net-Stack bevorzugt wlan bei Link-up (stale rtl8153 umgehen) | Kernel `netdev::send/recv/mac/list` | ✅ 0.207.0 |
| `dhcp`-Kommando (on-demand, Boot-DHCP war vor WiFi) | Kernel intent | ✅ 0.206.0 |
| **DHCP completed → echte IP** | Kernel-IP-Stack | ❌ (Verdacht: rx_classify CCMP/MIC-Offset → mangled Frame) |
| → Ping/Browse | — | ❌ |

### Phase J — WiFi-Class-ABI-Integration (→ wifid) ❌
Siehe `WIFI_CLASS_ABI.md` §8. Control-Channel-Host-Fns existieren im Kernel (v0.205.0),
sind aber im Treiber-Loop noch nicht verdrahtet.
| Schritt | Status |
|---|---|
| `npk_wifi_poll_cmd` im resident-Loop pollen | ❌ |
| Scan-on-cmd → SCAN_AP/SCAN_DONE-Events | ❌ |
| CONNECT-cmd → die Phase-E/F/G/H-Kette anstoßen | ❌ |
| SET_KEY-cmd → ADD_STA_KEY | ❌ |

### Phase K — Lifecycle / Robustheit 🔶
| Linux | unsere fn | Status |
|---|---|---|
| `iwl_mvm_rm_sta` (REMOVE_STA 0x19) | — | ❌ |
| `iwl_mvm_dump_nic_error_log` (FW-Assert-Dump) | `dump_fw_error_log` | ✅ (Werkzeug) |
| FW-Restart / Error-Recovery | — | ❌ |
| `iwl_mvm_disable_beacon_filter` / Power-Save-Sleep | — | ⬜ bewusst PS=disabled |
| BT-Coex (combo-chip, `bt_ax200`) | — | ❌ separates Kapitel |

---

## Konsolidierte FW-Command-Liste (Connect→Authorized, grob chronologisch)

| # | Command | Hex | Group | bei uns |
|---|---|---|---|---|
| 1 | PHY_CONTEXT_CMD | 0x08 | LONG(1) | ✅ |
| 2 | RLC_CONFIG_CMD | 0x08 | DATA_PATH(5) | ✅ |
| 3 | BINDING_CONTEXT_CMD | 0x2b | LONG(1) | ✅ |
| 4 | ADD_STA | 0x18 | LONG(1) | ✅ (add) / ❌ (modify assoc/auth) |
| 5 | SCD_QUEUE_CONFIG_CMD | 0x17 | DATA_PATH(5) | ✅ |
| 6 | MAC_PM_POWER_TABLE | 0xa9 | LONG(1) | ✅ |
| 7 | MAC_CONTEXT_CMD | 0x28 | LONG(1) | ✅ (add+modify-bssid) / ❌ (is_assoc=1) |
| 8 | SESSION_PROTECTION_CMD | 0x05 | MAC_CONF(3) | ✅ |
| 9 | TX_CMD | 0x1c | g0 (short hdr) | ✅ (auth+assoc) / ❌ (eapol/data) |
| 10 | TIME_QUOTA_CMD | 0x2c | LONG(1) | ⬜ pre-assoc skip |
| 11 | ADD_STA_KEY | 0x17 | LONG(1) | ❌ (WPA-Keys) |
| 12 | REMOVE_STA | 0x19 | LONG(1) | ❌ (Teardown) |

> 🔑 Alle „legacy"-Befehle (g0 im enum) gehen bei gen2/wide-header als **LONG_GROUP(1)**
> raus (`send_hcmd` remap) — Ausnahme: **TX_CMD** nutzt den **kurzen Header (group 0)**,
> weil es kein Host-Command ist, sondern ein Geräte-TX-Command über die TX-Queue.

## Empfohlene Reihenfolge der nächsten Schritte
1. **Phase G** (Auth-Resp parse → Assoc-Req/Resp → ADD_STA modify + MAC_CONTEXT is_assoc=1)
   = Assoziation an **OFFENES Netz** zuerst (erster Daten-Pfad ohne 4-Way).
2. **Phase I** Daten-TX/RX an netdev verdrahten → DHCP/Ping über offenes WLAN.
3. **Phase J** ABI-Verdrahtung + **Phase H** WPA2 in `wifid.wasm` → WPA2-Daily-Driver.
4. **Phase K**/`bt_ax200` als Politur/Folgekapitel.
