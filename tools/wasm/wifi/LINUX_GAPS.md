# WiFi Driver (rtw8852be) — Linux-Audit v2

Vollständiger Abgleich der Linux `rtw89` Call-Chain gegen unser `wifi.wasm`.
Jedes Linux-Builder-Paar (benannte Funktion) = eine Zeile im Audit. Wir haben
in v1 nur die Phase **nach** `rtw89_mac_init` erfasst — das Fundament davor
fehlte. Dieser v2 listet vom `pwr_on` bis zum `scan-ready` Zustand **jeden**
Builder.

**Ausgangs-Symptom** (v1.2.0): FW läuft, `scan_offload` läuft durch, aber
`SCANOFLD_START` → `ret=4`. Bei aktivierten VIF-H2Cs (v1.0/v1.1) wedgt die
gesamte H2C-Pipe — wahrscheinlichste Ursache: fehlende Per-Block-IMR-Enables
nach FWDL (17 IMR-Register die Linux setzt, wir nicht).

**🎉 v1.5.0 DURCHBRUCH:** Hypothese bestätigt. Mit Phase 1 vollständig
(17 per-block IMRs + sys_init_ax re-assert + korrekte ERR_IMR-Werte)
laufen alle 8 VIF-Init-H2Cs mit `ret=0` durch, `SCANOFLD_START` kehrt
`ret=0` statt `ret=4` zurück, FW iteriert alle 13 2G-Kanäle, und der
NETGEAR88-AP wird live empfangen (82 Beacons / 305 Frames in 30s).
IQK LOK fail bleibt — stellt sich heraus dass das ein Red-Herring ist,
FW macht interne per-channel RFK sobald scan läuft.

Legende: `[x]` erledigt · `[/]` partiell · `[ ]` fehlt · `[·]` nicht nötig für 8852BE

---

## Kapitel 0 — Pre-FWDL (Chip power-on)

- [x] **0.1** `rtw89_mac_pwr_on_func` → `rtw8852b_pwr_on_func` (rtw8852b.c) — pwr_off → pwr_on → XTAL_SI → ISO → LDO → DMAC_FUNC_EN + CMAC_FUNC_EN + CLK_EN. Unser `fw::pwr_off` + `fw::pwr_on`.
- [·] 0.2 `rtw89_chip_bb_preinit` — NULL für 8852B.
- [ ] **0.3** `rtw89_phy_init_bb_afe` (phy.c:1903) — **fehlt**. Lädt AFE-Parameter vor FWDL.

## Kapitel 1 — `rtw89_mac_partial_init` (mac.c:4207)

- [ ] **1.1** `rtw89_mac_ctrl_hci_dma_trx(true)` — **prüfen**. Wir setzen `HCI_FUNC_EN=0x03` (TXDMA+RXDMA) manuell in `fw::download` vor `dmac_pre_init_dlfw`. Ob das identisch zu Linux' Builder ist: open.
- [/] **1.2** `rtw89_mac_dmac_pre_init` — wir haben `fw::dmac_pre_init_dlfw` für FWDL-Mode. Linux ruft eine andere Variante **nach** FWDL für Normal-Mode. Nicht verifiziert.
- [x] 1.3 `hci.mac_pre_init` (= `rtw89_pci_ops_mac_pre_init_ax`) — wir haben `fw::pcie_dma_pre_init` mit allen helpers (l1off, hci_ldo, set_sic, set_lbc, keep_reg).
- [x] 1.4 `rtw89_fw_download` — FWDL via CH12, 3 Sektionen, ready nach ~69ms.

## Kapitel 2 — `rtw89_mac_init` (mac.c:4257, läuft nach FWDL)

### 2.1 `rtw89_chip_enable_bb_rf` (= `__rtw8852bx_mac_enable_bb_rf`)

- [x] BB reset + global reset, SPS dig, AFE toggle, XTAL_SI RFC S0/S1, PHYREG_SET. Unser `mac::enable_bb_rf`.

### 2.2 `sys_init_ax` (mac.c:1696) — **re-assert nach FWDL**

- [x] **2.2.1** `dmac_func_en_ax` (mac.c:1651) — v1.4.0: `mac::sys_init_ax` schreibt `R_AX_DMAC_FUNC_EN`+`R_AX_DMAC_CLK_EN` direkt mit exakten Linux-Bits.
- [x] **2.2.2** `cmac_func_en_ax(mac_idx=0, en=true)` (mac.c:1605) — v1.4.0: set32 auf `R_AX_CK_EN`+`R_AX_CMAC_FUNC_EN`.
- [x] **2.2.3** `chip_func_en_ax` (mac.c:1685) — v1.4.0: in `sys_init_ax` integriert (B_AX_OCP_L1_MASK).

### 2.3 `trx_init_ax` (mac.c:3929)

- [/] **2.3.1** `dmac_init_ax(mac_idx=0)` (mac.c:2458):
  - [/] **2.3.1.1** `rtw89_mac_dle_init` (mac.c:2227) — Linux referenziert `mac_size.wde_size7` + `ple_size0` Tabellen. Wir haben Werte hart einkodiert in `mac::dle_init` (SCC quotas). Verifizieren ob identisch.
  - [·] 2.3.1.2 `rtw89_mac_preload_init` (mac.c:2324) — NO-OP für 8852B (Early-Return `rtw89_is_rtl885xb`).
  - [/] **2.3.1.3** `rtw89_mac_hfc_init(reset=true, en=true, h2c_en=true)` (mac.c:1193) — wir haben `mac::hfc_init`. Prüfen ob Kanalwerte (min/max) identisch zu Linux `mac_size.hfc_prec_cfg_c0`.
  - [/] 2.3.1.4 `sta_sch_init_ax` (mac.c:2374) — haben wir als `sta_sch_init`.
  - [/] 2.3.1.5 `mpdu_proc_init_ax` — haben wir als `mpdu_proc_init`, Werte 1:1 geprüft.
  - [/] 2.3.1.6 `sec_eng_init_ax` — haben wir als `sec_eng_init`.
- [/] **2.3.2** `cmac_init_ax(mac_idx=0)` (mac.c:2993) — **12 Sub-Builder**, wir haben alle **inline** in `mac::cmac_init`. Pro Builder prüfen:
  - [/] 2.3.2.1 `scheduler_init_ax`
  - [/] 2.3.2.2 `addr_cam_init_ax` — wir klaren CAM, Linux hat Poll-Timeout
  - [/] 2.3.2.3 `rx_fltr_init_ax` — wir haben nur partiell (MGNT/CTRL/DATA/PLCP). Linux Default-Filter-Werte prüfen.
  - [/] 2.3.2.4 `cca_ctrl_init_ax`
  - [/] 2.3.2.5 `nav_ctrl_init_ax`
  - [/] 2.3.2.6 `spatial_reuse_init_ax`
  - [/] 2.3.2.7 `tmac_init_ax` — partiell (LOOPBACK, TCR0, TXD_FIFO). Linux-Default-Werte prüfen.
  - [/] 2.3.2.8 `trxptcl_init_ax` — partiell (SIFS, FCSCHK).
  - [/] 2.3.2.9 `rmac_init_ax` — partiell (SSN_SEL, CH_EN, MPDU_MAX_LEN).
  - [/] 2.3.2.10 `cmac_com_init_ax`
  - [x] 2.3.2.11 `ptcl_init_ax` — v1.31: PCIe-Block (SIFS_SETTING CTS2SELF, **PTCL_FSM_MON.TX_ARB_TO_THR=0x3F**) + MAC_0 PTCLRPT_FULL_HDL.SPE_RPT_PATH=FWD_TO_WLCPU eingefügt. Hypothese: Default-Timeout 0 liess PTCL-Arbiter TX sofort abbrechen → TX_COUNTER=0 trotz CH8_BUSY-toggle.
  - [/] 2.3.2.12 `cmac_dma_init_ax` — nur 1 Register `0xC804`. Linux-Werte prüfen.
- [x] **2.3.3** `enable_imr_ax(MAC_0, DMAC_SEL)` (mac.c:3836) — v1.3.0: alle 11 Sub-IMR-Enables in `imr.rs`:
  - [x] `wdrls_imr_enable`
  - [x] `wsec_imr_enable`
  - [x] `mpdu_trx_imr_enable`
  - [x] `sta_sch_imr_enable`
  - [x] `txpktctl_imr_enable`
  - [x] `wde_imr_enable`
  - [x] `ple_imr_enable`
  - [x] `pktin_imr_enable`
  - [x] `dispatcher_imr_enable`
  - [x] `cpuio_imr_enable`
  - [x] `bbrpt_imr_enable`
- [x] **2.3.4** `enable_imr_ax(MAC_0, CMAC_SEL)` — v1.3.0: 6 Sub-IMR-Enables:
  - [x] `scheduler_imr_enable`
  - [x] `ptcl_imr_enable`
  - [x] `cdma_imr_enable`
  - [·] `phy_intf_imr_enable` (8852B imr_info: clr=set=0, effektiv NO-OP)
  - [x] `rmac_imr_enable`
  - [x] `tmac_imr_enable`
- [x] **2.3.5** `err_imr_ctrl_ax(true)` (mac.c:3874) — `DMAC_ERR_IMR=CMAC_ERR_IMR=0xFFFFFFFF` entspricht `DMAC_ERR_IMR_EN = CMAC0_ERR_IMR_EN = GENMASK(31,0)` (reg.h:663). **Werte waren schon korrekt.**
- [x] 2.3.6 `set_host_rpr_ax` (mac.c:3909) — haben wir als `RPR: POH mode` Block.

### 2.4 `rtw89_mac_feat_init` (mac.c:3982) — BA CAM init

- [ ] **2.4** — **fehlt komplett**. BA CAM = Block-ACK CAM für Aggregation. Nur TX-seitig, für passive Listen möglicherweise unkritisch, aber im Audit listen.

### 2.5 `hci.mac_post_init` (PCI)

- [x] `rtw89_pci_ops_mac_post_init_ax` — wir haben `mac::pcie_post_init` (LTR + ctrl_dma_all + ring addresses).

### 2.6 `rtw89_fw_send_all_early_h2c` (fw.c:7874)

- [·] **2.6** — **NO-OP für uns**. Linux iteriert `rtwdev->early_h2c_list` (debugfs.c:3536) — nur via `/sys/kernel/debug/.../early_h2c` manuell befüllt. In Produktion leer, daher effektiv No-Op. Kein Call-Punkt nötig.

### 2.7 `rtw89_fw_h2c_set_ofld_cfg`

- [x] Haben wir — DONE_ACK `ret=0` bestätigt.

## Kapitel 3 — `rtw89_core_start` Rest

### 3.1 `rtw89_btc_ntfy_poweron`

- [·] BT-Coex — NO-OP ohne BT-HW.

### 3.2 `rtw89_chip_reset_bb_rf`

- [x] `disable_bb_rf` + `enable_bb_rf` — haben wir als `mac::reset_bb_rf`.

### 3.3 `rtw89_phy_init_bb_reg`

- [x] 3.3.1 BB-Tabelle (1013 regs) — `phy::run_table(BB_TABLE)` ✓
- [x] 3.3.2 BB-Gain-Parser (66 Einträge) — `phy::cfg_bb_gain` + gain_info struct (v0.95.0) ✓
- [x] 3.3.3 `rtw89_chip_init_txpwr_unit` — in unserem phy::init? → zu verifizieren
- [x] 3.3.4 `rtw89_phy_bb_reset` — haben wir (bb_reset)

### 3.4 `rtw89_chip_bb_postinit`

- [·] NULL für 8852B.

### 3.5 `rtw89_phy_init_rf_reg`

- [x] RF-Pfade A + B (8295 + 8392 regs nach Conditional Match) — `phy::run_table(RF_A_TABLE/RF_B_TABLE)` ✓

### 3.6 `rtw89_btc_ntfy_init`

- [·] NO-OP ohne BT.

### 3.7 `rtw89_phy_dm_init` (phy.c:7683) — **teilweise, viele fehlend**

- [·] 3.7.1 `rtw89_phy_stat_init` (phy.c:5792) — nur EWMA-SW-State + thermal-cache, kein MMIO. Wir tracken diese SW-State nicht.
- [x] 3.7.2 `rtw89_chip_bb_sethw` (common.c:1099) — haben wir inline
- [x] 3.7.3 `rtw89_phy_env_monitor_init` (ccx_top) — haben wir
- [·] 3.7.4 `rtw89_phy_nhm_setting_init` (phy.c:6020) — `support_noise=false` für 8852B → Early-Return, NO-OP.
- [x] 3.7.5 `rtw89_physts_parsing_init` — haben wir
- [/] 3.7.6 `__rtw89_phy_dig_init` — Subset geportet (PD-Thresholds)
- [/] 3.7.7 `rtw89_phy_cfo_init` — Subset (DCFO + hw comp)
- [·] 3.7.8 `rtw89_phy_bb_wrap_init` — AX-Stub `static inline {}` in phy.h:898, NO-OP für 8852B.
- [/] 3.7.9 `rtw89_phy_edcca_init` — nur 1 reg (TX_COLLISION_T2R_ST). Linux hat mehr.
- [·] 3.7.10 `rtw89_phy_ch_info_init` — AX-Stub `static inline {}` in phy.h:912, NO-OP.
- [·] 3.7.11 `rtw89_phy_ul_tb_info_init` (phy.c:5551) — setzt nur SW-Flag `dyn_tb_tri_en`, kein MMIO.
- [·] 3.7.12 `rtw89_phy_antdiv_init` (phy.c:5670) — `hal.ant_diversity=false` default → Early-Return.
- [·] 3.7.13 `rtw89_chip_rfe_gpio` — NULL für 8852B
- [·] 3.7.14 `rtw89_chip_rfk_hw_init` — NULL für 8852B
- [x] 3.7.15 `rtw89_phy_init_rf_nctl` (preinit + NCTL table, 1320 regs)
- [x] 3.7.16 `rtw89_chip_rfk_init` (dpk_init + rck + dack + rx_dck) — haben wir als phy::rfk_init + rfk::init
- [ ] 3.7.17 `rtw89_chip_set_txpwr_ctrl`
- [ ] 3.7.18 `rtw89_chip_power_trim`
- [x] 3.7.19 `rtw89_chip_cfg_txrx_path` (RF_AB, 2G) — haben wir inline

### 3.8 `rtw89_mac_set_edcca_mode_bands`

- [·] **3.8** — `.set_edcca_mode = NULL` in `rtw89_mac_gen_ax` (mac.c:7355). NO-OP für 8852B.

### 3.9 `rtw89_mac_cfg_ppdu_status_bands`

- [x] **3.9** — MAC_0 schreibt `R_AX_PPDU_STAT (0xCE40)` mit PPDU_STAT_RPT_EN|APP_MAC_INFO_RPT|APP_PLCP_HDR_RPT|PPDU_STAT_RPT_CRC32 und `R_AX_HW_RPT_FWD (0x9C18) mask GENMASK(1,0)=1` (= RTW89_PRPT_DEST_HOST). dbcc_en=false für 8852B → MAC_1 nicht nötig.

### 3.10 `rtw89_mac_cfg_phy_rpt_bands`

- [·] **3.10** — `.cfg_phy_rpt = NULL` für AX (mac.c:7354). NO-OP.

### 3.11 `rtw89_mac_update_rts_threshold`

- [~] 3.11 — TX-seitig (RTS time/len thresholds). Nicht kritisch für passive Listen. Erst wenn wir TX brauchen.

### 3.12 `rtw89_hci_start` (= `rtw89_pci_ops_start`)

- [x] IMR unmask (HIMR0 + HIMR00 + HIMR10) — haben wir als `mac::hci_start` (v0.98.0). NAPI-Start ist Linux-Kernel-only.

### 3.13 `rtw89_fw_h2c_fw_log`

- [x] haben wir (v0.96.0).

### 3.14 `rtw89_chip_rfk_init_late`

- [·] NULL für 8852B.

## Kapitel 4 — `rtw89_mac_vif_init` (per VIF, mac.c:4942)

Aktuell **disabled** in v1.2.0 (wedged Pipe). Helpers existieren in `vif.rs` aber vermutlich mit einem Encoding-Bug.

- [/] **4.1** `rtw89_mac_port_update` — unser `port_update_p0_nolink` deckt NO_LINK STA auf port 0 ab. Nicht getestet ob alle Linux-Funktionen drin sind (`port_cfg_func_sw`, `cfg_tx_rpt`, `cfg_rx_rpt`, `cfg_net_type`, `cfg_bcn_prct`, `cfg_rx_sw`, `cfg_rx_sync_by_nettype`, `cfg_tx_sw_by_nettype`, `cfg_bcn_intv`, `cfg_hiq_win`, `cfg_hiq_dtim`, `cfg_hiq_drop`, `cfg_bcn_setup_time`, `cfg_bcn_hold_time`, `cfg_bcn_mask_area`, `cfg_tbtt_early`, `cfg_tbtt_agg`, `cfg_bss_color`, `cfg_mbssid`, `cfg_func_en`, `tsf_resync_all`, `cfg_bcn_early`, `cfg_bcn_psr_rpt`).
- [x] **4.2** `rtw89_mac_dmac_tbl_init` — 4 × INDIR_ACCESS 0 → DMAC_TBL[macid] ✓
- [x] **4.3** `rtw89_mac_cmac_tbl_init` — 8 × INDIR_ACCESS mit Defaults ✓
- [/] **4.4** `rtw89_mac_set_macid_pause(false)` — Helper vorhanden, Struct-Größe + rack/dack korrekt.
- [/] **4.5** `rtw89_fw_h2c_role_maintain(CREATE)` — Helper vorhanden, w0-Bits 1:1 geprüft.
- [/] **4.6** `rtw89_fw_h2c_join_info(dis_conn=true)` — Helper vorhanden, w0-Bits 1:1 geprüft.
- [/] **4.7** `rtw89_cam_init + rtw89_fw_h2c_cam(CREATE)` — `h2c_cam` Helper mit 60-Byte-Buffer. `cam_init` (SW-state setup für addr_cam + bssid_cam) haben wir nicht separat.
- [/] **4.8** `rtw89_chip_h2c_default_cmac_tbl` — Helper vorhanden, 68-Byte-Buffer, Masken-Modell (alle 0 heisst "FW-Default behalten").
- [·] 4.9 `rtw89_chip_h2c_default_dmac_tbl` — NULL für 8852B.

**Bug-Vermutung:** einer der H2Cs (macid_pause / role_maintain / cam / default_cmac_tbl) hat ein falsch kodiertes Byte das die FW in einen dauerhaft silent state kippt. Da die Kapitel-2-IMR-Enables fehlen, könnte der Silent-State auch von dort kommen — erst Kapitel 2 fixen, dann Kapitel 4 wieder testen.

## Kapitel 5 — `__rtw89_set_channel` (core.c:515, per channel change)

### 5.1 `rtw89_chip_set_channel_prepare` (= `set_channel_help(enter=true)`)

- [ ] 5.1.1 `rtw89_chip_stop_sch_tx(ALL)` — fehlt in unserem `set_channel_help_enter`
- [x] 5.1.2 `cfg_ppdu_status(false)`
- [x] 5.1.3 `tssi_cont_en(false)`
- [x] 5.1.4 `adc_en(false)`
- [x] 5.1.5 `bb_reset_en(band, false)`

### 5.2 `chip->set_channel` (= `rtw8852b_set_channel`)

#### 5.2.1 `__rtw8852bx_set_channel_mac`

- [x] 5.2.1.1 `ctrl_rfmod` (20MHz)
- [x] 5.2.1.2 `tx_sub_carrier_value = 0`
- [x] 5.2.1.3 `txrate_chk`

#### 5.2.2 `__rtw8852bx_set_channel_bb` (common.c:1167)

- [x] 5.2.2.1 `ctrl_sco_cck`
- [/] 5.2.2.2 `rtw8852bx_ctrl_ch` (incl. `set_gain_error`, `set_gain_offset`, `set_rxsc_rpl_comp`):
  - [x] `set_gain_error_2g` (v0.95.0)
  - [ ] `set_gain_offset` — braucht Efuse-Parser (D2b)
  - [ ] `set_rxsc_rpl_comp` — braucht Efuse (D2c)
- [x] 5.2.2.3 `ctrl_bw` (20MHz)
- [x] 5.2.2.4 `ctrl_cck_en`
- [x] 5.2.2.5 `chan_idx` encode
- [ ] 5.2.2.6 `rtw8852bx_5m_mask` — fehlt (D6)
- [ ] 5.2.2.7 `rtw8852bx_bb_set_pop` — fehlt (D7)
- [x] 5.2.2.8 `__rtw8852bx_bb_reset_all` — haben wir (phy::bb_reset)

#### 5.2.3 `rtw8852b_set_channel_rf` (= `ctrl_bw_ch`)

- [x] `_ctrl_ch` (RR_CFGCH mit CH + BAND + BW2 + LCKST trigger)
- [x] `_ctrl_bw` (BW = 20M)
- [x] `_rxbb_bw` (4 RF writes per path)

### 5.3 `chip->set_txpwr`

- [ ] 5.3 `rtw8852bx_set_txpwr` — fehlt (TX-seitig, evtl. unkritisch für Listen).

### 5.4 `rtw89_chip_set_channel_done` (= `set_channel_help(enter=false)`)

- [x] 5.4.1 `cfg_ppdu_status(true)`
- [x] 5.4.2 `adc_en(true)`
- [x] 5.4.3 `tssi_cont_en(true)`
- [x] 5.4.4 `bb_reset_en(band, true)`
- [ ] 5.4.5 `rtw89_chip_resume_sch_tx(tx_en)` — fehlt

## Kapitel 6 — `rtw89_chip_rfk_channel` (rtw8852b.c:663, per-channel RFK)

Gewrapped von `rtw89_btc_ntfy_conn_rfk(true/false)` (no-op ohne BT) und bei
jedem Builder: `chip_stop_sch_tx(ALL)` + `_wait_rx_mode` + … + `resume_sch_tx`.

- [ ] **6.1** `rtw8852b_mcc_get_ch_info` — MCC state (nicht relevant für uns)
- [x] **6.2** `rtw8852b_rx_dck`
- [x] **6.3** `rtw8852b_iqk` — inklusive `chip_stop_sch_tx`-Wrapper (v0.97.0). LOK fail bleibt — möglicherweise durch fehlendes PHY-Dm-Init (B4/B8/B10) verursacht.
- [ ] 6.4 `rtw8852b_tssi(start=true)` — fehlt (TX-seitig)
- [ ] 6.5 `rtw8852b_dpk` — fehlt (TX-seitig)

---

## Priorisierte Umsetzungsreihenfolge

Jeder Schritt = ein Commit + NUC-Test. Hypothese pro Schritt im Commit-Message.

### Phase 1 — Fundament (Kapitel 2 Gaps)

1. **2.3.3 + 2.3.4** DMAC/CMAC Per-Block-IMR-Enables (17 Register-Sets) — wahrscheinlichster Kandidat für den Wedge.
2. **2.3.5** `err_imr_ctrl_ax` mit korrekten Linux-Konstanten (`DMAC_ERR_IMR_EN` / `CMAC0_ERR_IMR_EN`) statt 0xFFFFFFFF.
3. **2.2.1 + 2.2.2** `dmac_func_en_ax` + `cmac_func_en_ax` re-assert nach FWDL.
4. **2.6** `rtw89_fw_send_all_early_h2c` — auch wenn die Queue leer ist, ggf. wichtiger Sync-Punkt.

### Phase 2 — PHY DM Init Rest (Kapitel 3.7 Gaps)

5. **3.7.1, 3.7.4, 3.7.8, 3.7.10, 3.7.11, 3.7.12** (stat_init, nhm_setting, bb_wrap_init, ch_info_init, ul_tb_info_init, antdiv_init) — alle Builder 1:1 portieren.

### Phase 3 — Band + PPDU Config (Kapitel 3.8-3.11)

6. **3.8** `mac_set_edcca_mode_bands`
7. **3.9** `mac_cfg_ppdu_status_bands` (statt nur PHY_0)
8. **3.10** `mac_cfg_phy_rpt_bands`
9. **3.11** `mac_update_rts_threshold`

### Phase 4 — set_channel Bereinigung (Kapitel 5 Gaps)

10. **5.1.1 + 5.4.5** `chip_stop_sch_tx`/`resume_sch_tx` im `set_channel_help`-Wrapper
11. **5.2.2.6** `5m_mask`
12. **5.2.2.7** `bb_set_pop`

### Phase 5 — VIF Init re-test (Kapitel 4)

13. Nach Phase 1-4: `vif::init` wieder aktivieren (v1.1 code). Hypothese: IMR-Gaps waren der Grund für den Wedge.

### Phase 6 — Per-channel RFK Rest (Kapitel 6)

14. **6.4** `tssi(start=true)` — nur wenn TX relevant wird
15. **6.5** `dpk` — nur wenn TX relevant wird

### Phase 7 — Efuse-basierte Gain-Offsets (Kapitel 5.2.2.2 Rest)

16. **D2b+D2c** `set_gain_offset` + `set_rxsc_rpl_comp` — Efuse-Parser implementieren

---

## Gesamt-Bilanz

| Kapitel | Items | [x] done | [/] partial | [ ] fehlt | [·] n/a |
|---|---|---|---|---|---|
| 0 | 3 | 1 | 0 | 1 | 1 |
| 1 | 4 | 2 | 1 | 1 | 0 |
| 2 | ~30 | 3 | 8 | 17+ | 1 |
| 3 | ~28 | 10 | 4 | 9 | 5 |
| 4 | 9 | 2 | 6 | 0 | 1 |
| 5 | ~18 | 11 | 1 | 5 | 1 |
| 6 | 6 | 2 | 0 | 3 | 1 |

Am meisten offen im Fundament (Kapitel 2). Besonders die **17 Per-Block-IMR-Enables** stechen heraus — das ist genau der Unterschied zwischen "FW verarbeitet H2C und antwortet" und "FW verarbeitet H2C aber Antwort kommt nie raus".
