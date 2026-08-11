# WIFI_AX200 — Intel Wi-Fi 6 AX200 als WASM-Driver

> **Archiv (2026-08-11).** Der WLAN-Teil ist gebaut und an Hardware bestätigt
> (`wifi_ax200` findet Netze, Vendor-Code 1:1 aus Linux). `bt_ax200` (Bluetooth)
> wurde nie angefangen. Lebende Dokumente: `docs/spec/WIFI_CLASS_ABI.md` (Vertrag)
> und `docs/spec/AX200_FUNC_MAP.md` (Linux-Gap-Karte).

Plan für `wifi_ax200` (PCIe iwlwifi) + `bt_ax200` (USB btusb). Ziel:
WLAN **und** BT auf dem HP-Notebook. Strikt 1:1 Linux 6.18.26
(`~/.cache/nopeekos/linux-src/linux-6.18.26/drivers/net/wireless/intel/iwlwifi/`),
keine Auslassungen, sinnvoll pro Logik-Stufe gebündelt.

## Gerät

| | `wifi_ax200` | `bt_ax200` |
|---|---|---|
| PCI/USB | PCIe `6c:00.0 8086:2723` | USB `8087:0029` (PCH-xHCI) |
| Linux | iwlwifi-mvm, family **22000** (gen2) | btusb + btintel |
| Transport | PCIe + DMA-Ringe | USB-Bulk (bestehender Stack) |
| FW | `iwlwifi-cc-a0-77.ucode` + PNVM | btintel HCI-Vendor-FW |
| RF | `iwl_rf_hr` (HR) | — |

Combo-Chip, aber zwei getrennte Geräte/Treiber. Coex (geteilte Antenne) =
WiFi-seitige Host-Commands, kein BT-Code.

## Architektur-Entscheidungen (getroffen)

- **WASM-Driver**, nicht MicroVM-Passthrough (das bräuchte IOMMU + Topologie-
  Rework + Dauer-Linux). Reiht sich in den Microkernel-Driver-Refactor ein.
- **Bestehende Treiber-ABI wird wiederverwendet** (`kernel/src/wasm.rs`,
  erprobt vom RTL8852BE-`wifi`-Treiber): `npk_pci_*`, `npk_mmio_*` (inkl.
  16/64-bit), `npk_dma_*`, `npk_memory_fence`, `npk_netdev_register`.
- **DMA-Sicherheit:** ABI gibt WASM die rohe Bus-Adresse (`npk_dma_phys_addr`),
  Treiber schreibt sie selbst in Deskriptoren. Heißt: signierte DMA-Treiber =
  Trusted-Base (wie Linux, besser durch Sandbox für die Logik). IOMMU als
  spätere Härtung. Diese Entscheidung war im Kernel bereits implizit getroffen.
- **Polling, keine IRQs.** Wie der RTL-Treiber. ALIVE/RX über Status-Register
  (`CSR_INT`) pollen statt MSI-X.
- **Firmware-Distribution:** Driver-Store-Modell — signiert in npkFS,
  Pull-by-(vendor,device) ohne Telemetrie, AX200 auf dem Installer gebündelt.

## Nötige Kernel-Änderung (einmalig, nur Kapazität)

`kernel/src/wasm.rs` Limits sind für eine kleine NIC dimensioniert, die
iwlwifi-FW sprengt sie:
- `MAX_DMA_PAGES = 256` (1 MB) → die `cc-a0-77`-FW allein ist ~1,3 MB. **Bump
  auf ~2048 (8 MB)** für FW-Sektionen + RX-Buffer-Pool + Ringe.
- `npk_dma_alloc` Per-Call-Cap `pages > 64` → einzelne FW-Sektion kann
  > 256 KB sein. **Bump auf z.B. 1024 (4 MB)/Call.**
- `MAX_DMA_ALLOCS = 64` → viele FW-Sektionen + Ringe + RX. **Bump auf 128.**

Exakte Werte fixieren, sobald die FW-Datei vorliegt (Sektionsgrößen messen).
`allocate_contiguous_below(pages, 0x1_0000_0000)` garantiert schon
contiguous + unter 4 GB → matcht Linux' „kein 4-GB-Grenzüberschritt".

## „Firmware auf den Chip" — gen2-Mechanismus (der Kern)

Der Chip **lädt die FW selbst per DMA**. Wir bauen eine Context-Info-Struktur,
legen die FW-Bytes in DMA-Buffer, schreiben dem Chip die Adresse → er zieht.
Call-Path aus `pcie/gen1_2/trans-gen2.c:iwl_trans_pcie_gen2_start_fw`.

## Stages (geordnet, gebündelt — eine Version/Commit pro Stage)

### Stage 0a — Chip ansprechen ✅ (Scaffold)
`pci_bind(0x8086,0x2723)` → bus master → `mmio_map_bar(0)` → `CSR_HW_REV`
(0x028) + `CSR_HW_RF_ID` (0x09c) lesen + loggen. Bestätigt, dass wir mit dem
AX200 reden (RF-ID muss HR-Typ sein). Braucht nur verifizierte Offsets, keine
Bit-Masken. **Erster HW-Test.**

### Stage 0b — Reset + APM bringup
1:1 aus `pcie/gen1_2/trans.c` + `trans-gen2.c`:
- `iwl_pcie_prepare_card_hw` / `iwl_pcie_set_hw_ready` (CSR_HW_IF_CONFIG
  PREPARE/PREPARE_DONE handshake) — Ownership von AMT/BIOS
- `iwl_trans_pcie_sw_reset` (CSR_RESET SW_RESET, dann GP_CNTRL ready-poll)
- `iwl_pcie_gen2_apm_init`: GIO_CHICKEN L1A_NO_L0S_RX; DBG_HPET_MEM_REG;
  HW_IF_CONFIG HAP_WAKE; `apm_config`; `iwl_trans_activate_nic`
  (GP_CNTRL INIT_DONE + poll MAC_CLOCK_READY)
→ **Bit-Masken vorher aus `iwl-csr.h` ziehen** (PREPARE, PREPARE_DONE,
  SW_RESET, INIT_DONE, MAC_CLOCK_READY, HAP_WAKE, L1A_NO_L0S_RX).

### Stage 1 — RX/TX-Ringe + Command-Queue
`iwl_pcie_gen2_nic_init`: `gen2_rx_init` (RX-BD-Ring `bd_dma`, used-BD
`used_bd_dma`, status `rb_stts_dma`, RB-Pool) + `iwl_txq_gen2_init`
(TFD-Command-Queue) + `CSR_MAC_SHADOW_REG_CTRL` shadow-regs.
DMA-Buffer via `npk_dma_alloc`, Adressen via `npk_dma_phys_addr`.
Quellen: `pcie/gen1_2/rx.c` (`iwl_pcie_alloc_rxq_dma`, `gen2_rx_init`),
`pcie/gen1_2/tx-gen2.c` (`iwl_txq_gen2_init`), `iwl-fh.h` (FH-Register).

### Stage 2 — Context-Info + FW-Load + ALIVE (Milestone #1)
`pcie/ctxt-info.c:iwl_pcie_ctxt_info_init`:
- ctxt_info-DMA-Struct (Layout aus `pcie/iwl-context-info.h`)
- RX-Ring- + Command-Queue-Adressen rein
- `iwl_pcie_init_fw_sec`: pro FW-Sektion (lmac/umac/paging) ein DMA-Buffer,
  FW-Bytes rein (`npk_dma_write`), Phys in `ctxt_info->dram`
- `iwl_enable_fw_load_int_ctx_info`
- **`mmio_write64(CSR_CTXT_INFO_BA, ctxt_info_phys)`** = kickt Self-Load
- `iwl_pcie_set_ltr`
- **`iwl_write_prph(UREG_CPU_INIT_RUN, 1)`** (PRPH via HBUS_TARG_PRPH_*) = CPU run
- **`CSR_INT` pollen auf ALIVE-Bit** (statt MSI-X / notif_wait)
FW-Image-TLV-Parsing: `iwl-drv.c` (`iwl_parse_tlv_firmware`), `fw/img.h`,
`fw/file.h`. **= ALIVE = der entscheidende Milestone.**

### Stage 3 — PNVM + INIT-FW + NVM-Daten
`fw/pnvm.c:iwl_pnvm_load` (PNVM-Sektion parsen → DMA → `UREG_DOORBELL_TO_ISR6
= PNVM`), dann INIT-ucode ALIVE, PHY-Kalibrierung (FW-intern!), NVM-Daten
holen (Kanäle, MAC) via `mvm/nvm.c`. `mvm/fw.c:iwl_mvm_load_ucode_wait_alive`
+ `iwl_alive_fn` als Vorlage (Notif → bei uns Poll).

### Stage 4 — Scan
mvm Host-Commands: Station/MAC-Context, Scan-Command → APs sehen. `mvm/scan.c`,
`mvm/mac-ctxt.c`, `mvm/sta.c`. Host-Commands gehen über die TFD-Command-Queue
(Stage 1).

### Stage 5 — Connect / Assoc + TX/RX-Datenpfad (= „#3 connect")
Der erste **TX-Datenpfad** + 802.11-Verbindung. Gestaffelt, jede Stage 1:1 nach
der kartierten iwlwifi-mvm-Sequenz (linux-6.18.26), eine Version/Commit pro
Stage, HW-Test am Ende. **Zuerst hartkodiert** gegen einen Test-AP (wie der Scan
zuerst hartkodiert war), **dann** an die WiFi-Klassen-ABI verdrahtet.

**cmd_ver am Build-Start aus der FW-Datei parsen** (Python-TLV, wie beim Scan):
PHY_CONTEXT, BINDING, ADD_STA(=12 bekannt), ADD_STA_KEY, SCD_QUEUE_CONFIG, TX_CMD.

- **5a — PHY-Context + Binding** (reine Control-Cmds, kein TX): `PHY_CONTEXT_CMD`
  (0x08, `iwl_phy_context_cmd`, v?; ci=channel-info des AP, band, 20 MHz,
  action ADD) + ggf. `RLC_CONFIG_CMD` (DATA_PATH) + `BINDING_CONTEXT_CMD` (0x2b,
  `iwl_binding_cmd`, MAC↔PHY, action ADD). Quellen: `mvm/phy-ctxt.c`,
  `mvm/binding.c`, `fw/api/phy-ctxt.h`+`binding.h`. **Test:** Echo/Status OK.
- **5b — Station (AP-Peer) + gen2-TX-Queue:** `ADD_STA` v12 (`iwl_mvm_add_sta_cmd`,
  sta_id, addr=BSSID, station_type=IWL_STA_LINK, add_modify=0) → Status SUCCESS;
  dann **dyn. TX-Queue** via `SCD_QUEUE_CONFIG_CMD` (DATA_PATH_GROUP, v3,
  `iwl_scd_queue_cfg_cmd`: op ADD, tfdq_dram_addr, **bc_dram_addr**, sta_mask,
  tid) → FW gibt **queue_id** zurück. Quellen: `mvm/sta.c`, `pcie/.../tx-gen2.c`
  `iwl_trans_txq_alloc`. **Test:** ADD_STA SUCCESS + queue_id geloggt.
- **5c — gen2-TX-Datenpfad (das harte neue Fundament):** TFD-Bau für Daten-Queue
  + **eingebettetes `tx_cmd`** (`iwl_tx_cmd` v?; len/flags/rate_n_flags/802.11-hdr)
  + **byte-count-table-Update** (`iwl_pcie_gen2_update_byte_tbl`) + Doorbell +
  **TX-Completion** (`TX_CMD`-Resp über RX-Ring). Quellen: `pcie/.../tx-gen2.c`
  `iwl_txq_gen2_tx`, `fw/api/tx.h`. **Test:** ein 802.11-Frame (QoS-Null oder
  hartkodierter Auth-Frame) raus → TX-Completion empfangen. Make-or-break.
- **5d — Auth + Assoc (OPEN-Netz zuerst!):** Auth-Frame (open-system) bauen+TX,
  Auth-Resp im RX (0xc1) sehen; Assoc-Req bauen+TX, Assoc-Resp mit **AID** lesen;
  dann `MAC_CONTEXT` is_assoc=1 + `ADD_STA` modify (assoc_id). Gegen ein **offenes**
  Test-Netz → **erster echter Daten-Pfad ohne 4-Way** = Meilenstein (DHCP läuft).
  Quellen: `mvm/mac-ctxt.c` (is_assoc), `mvm/sta.c` update, `mvm/mac80211.c`
  state-machine-Reihenfolge.
- **5e — WPA2 Keys + EAPOL + authorized:** ab hier **kommt `wifid` ins Spiel**
  (PSK/4-Way gehören NICHT in den Treiber). Treiber: EAPOL-Demux (Ethertype
  0x888E → uplink) + `ADD_STA_KEY` (0x17, `iwl_mvm_add_sta_key_cmd`, v?; PTK key_off
  0 / GTK key_off 1, STA_KEY_FLG_CCM) auf SET_KEY + `set_link(up)`. `wifid` macht
  PMK(PBKDF2)/PTK/GTK + die 4 EAPOL-Frames. Quellen: `mvm/sta.c`
  `iwl_mvm_send_sta_key`, `fw/api/sta.h`.

**Treiber- vs ABI-Arbeit:** PHY/Bind/Sta/TXQ/Keys = reine Treiber-FW-Plumbing.
Auth/Assoc/EAPOL-**Frames** baut die obere Schicht (`wifid`); der Treiber
transportiert sie nur (TX_MGMT/RX_MGMT/EAPOL über die ABI). **ABI-Verfeinerung
(in docs/spec/WIFI_CLASS_ABI.md nachziehen):** für die MLME-in-`wifid`-Trennung braucht's
neben TX_EAPOL auch generisches TX_MGMT/RX_MGMT + Signale ASSOC_READY (FW geprepped)
und ASSOCIATED{aid} (→ mac_ctxt is_assoc + update_sta) + AUTHORIZED.
**Größte neue Infrastruktur:** dyn. gen2-Daten-TXQ, **bc_tbl**, eingebettetes tx_cmd.

### `bt_ax200` (unabhängig, parallel möglich)
USB `8087:0029` über bestehenden xHCI/USB-Stack. btusb-HCI-Transport
(Bulk/Interrupt-EPs) + btintel-FW-Load (HCI-Vendor-Commands). Deutlich
einfacher: kein DMA, keine Kalibrierung. `drivers/bluetooth/btusb.c` +
`btintel.c`. Eigene spätere Stage.

## Disziplin

STRIKT 1:1 Linux. Vor jeder Stage die Bit-Masken/Konstanten aus den Headern
ziehen (`iwl-csr.h`, `iwl-prph.h`, `iwl-fh.h`, `iwl-context-info.h`), nie raten
(`grep` gegen Header). Keine „vermutlich no-op"-Auslassungen. Aber gebündelt
pro Stage, nicht pro Mini-Helper → vernünftige Zeit. Siehe
`memory/feedback_linux_strict.md`.

## Praktische Reibung

- Kein QEMU-Modell für AX200 → HW-only ab Stage 0a (gegen demo-first-Regel).
- AX200 im **serial-losen HP-Notebook** → Debug via Framebuffer-Farbstufen
  (`memory/project_uefi_relocatable.md`). Florian bleibt auf dem Notebook.
