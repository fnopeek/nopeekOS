# WiFi Driver (rtw8852be) — Linux vs. nopeekOS Lücken-Audit

Systematischer Abgleich unserer `wifi.wasm` gegen `/tmp/linux-rtw89/drivers/net/wireless/realtek/rtw89/`.
Jeder Bugfix: einzelner Commit + `install wifi` + Debug-Log → hier abhaken.

**Ausgangs-Symptom** (v0.95.0): FW läuft, MAC+PHY init durch, BB-Gain parst (66 Einträge), `set_gain_error` läuft — aber **0 Luft-Pakete in 60s** + **IQK LOK fail beide Pfade** (cor=1 fin=1 tx=1 rx=1).

Legende: `[ ]` offen · `[x]` erledigt · `[·]` nicht nötig für unsere HW

---

## A — Wahrscheinlich direkt RX-blockierend

- [ ] **A1** `rtw89_pci_ops_reset` → `rtw89_pci_reset_trx_rings` (pci.c:1776) — Ring-Reset + wp/rp init nach MAC-init
- [·] A2 `rtw89_pci_init_wp_16sel` (pci.c:1722) — NO-OP: `wp_sel_addr=0` für 8852BE (rtw8852be.c:53)
- [ ] **A3** `rtw89_mac_set_edcca_mode_bands` — EDCCA mode NORMAL pro Band
- [ ] **A4** `rtw89_mac_cfg_ppdu_status_bands` — nur PHY_0 statt pro MAC-Band
- [ ] **A5** `rtw89_mac_cfg_phy_rpt_bands` — PHY report config pro Band
- [ ] A6 `rtw89_mac_update_rts_threshold` (mac.c:6222) — TX-seitig, evtl. unkritisch
- [ ] **A7** `rtw89_hci_start` / `rtw89_pci_ops_start` — IRQ enable + RX-DMA arm (sehr verdächtig!)

## B — `rtw89_phy_dm_init` (core.c:5963)

- [ ] B1 `rtw89_phy_stat_init`
- [x] B2 `rtw89_chip_bb_sethw` (common.c:1099) — inline in mac.rs
- [x] B3 `rtw89_phy_env_monitor_init` / ccx_top (phy.c:5799)
- [ ] B4 `rtw89_phy_nhm_setting_init` (phy.c:6020) — Noise Histogram
- [x] B5 `rtw89_physts_parsing_init` (phy.c:6683)
- [ ] B6 `__rtw89_phy_dig_init` (phy.c:6838) — nur Subset geportet
- [ ] B7 `rtw89_phy_cfo_init` (phy.c:4957) — nur Subset
- [ ] B8 `rtw89_phy_bb_wrap_init`
- [ ] B9 `rtw89_phy_edcca_init` — nur 1 reg geportet, Linux hat mehr
- [ ] B10 `rtw89_phy_ch_info_init`
- [ ] B11 `rtw89_phy_ul_tb_info_init` (phy.c:5551)
- [ ] B12 `rtw89_phy_antdiv_init` + `set_ant` (phy.c:5670)
- [x] B13 `rtw89_phy_init_rf_nctl` (preinit + NCTL table)
- [x] B14 `rtw89_chip_rfk_init` (dpk_init+rck+dack+rx_dck)

## C — `set_channel_help` (rtw8852b.c:627)

- [ ] **C1** enter: `chip_stop_sch_tx(ALL)`
- [x] C2 enter: `cfg_ppdu_status(false)` + `tssi_cont_en(false)` + `adc_en(false)` + `bb_reset_en(false)`
- [x] C3 exit: Umkehrung von C2
- [ ] **C4** exit: `chip_resume_sch_tx(tx_en)`

## D — `__rtw8852bx_set_channel_bb` (common.c:1167)

- [x] D1 `ctrl_sco_cck`
- [x] D2a `ctrl_ch` + `set_gain_error` (v0.95.0)
- [ ] D2b `set_gain_offset` (braucht Efuse-Parser)
- [ ] D2c `set_rxsc_rpl_comp` (braucht Efuse-Parser)
- [x] D3 `ctrl_bw`
- [x] D4 `ctrl_cck_en`
- [x] D5 `chan_idx` Encoding
- [ ] D6 `rtw8852bx_5m_mask`
- [ ] D7 `rtw8852bx_bb_set_pop`
- [x] D8 `__rtw8852bx_bb_reset_all` (wir rufen phy::bb_reset)

## E — `rtw8852b_iqk` Entry (rtw8852b_rfk.c:3757)

- [·] E1 `btc_ntfy_wl_rfk(START)` — NO-OP ohne BT
- [x] **E2** `chip_stop_sch_tx(ALL)` — v0.97.0: H2CREG transport + `fw::stop_sch_tx` vor IQK
- [x] E3 `_wait_rx_mode`
- [x] E4 `_iqk_init`
- [x] E5 `_iqk` → `_doiqk` Kette
- [x] **E6** `chip_resume_sch_tx` — v0.97.0: `fw::resume_sch_tx` am IQK-Ende
- [·] E7 `btc_ntfy_wl_rfk(STOP)`

## F — RFK per-channel (rtw8852b.c:663 `rtw8852b_rfk_channel`)

- [x] F1 `rx_dck`
- [x] F2 `iqk` (mit Lücken E2/E6)
- [ ] F3 `tssi(start=true)` — TX-only, erstmal unkritisch für RX
- [ ] F4 `dpk` — TX-only

## G — FW-Kommunikation

- [x] **G1** `rtw89_fw_h2c_fw_log` — v0.96.0: fw.rs `h2c_fw_log` + call in mac::init nach set_ofld_cfg
- [ ] G2 `rtw89_fw_h2c_ccxrpt_parsing_para`

---

## Umsetzungsreihenfolge (je ein Commit + Test)

1. **G1** `h2c_set_fwlog` — sofortiger FW-Info-Gewinn, minimaler Code
2. **E2/E6** `chip_stop_sch_tx` um IQK — möglicher LOK-Fix
3. **A7** `hci_start` / `pci_ops_start` — IRQ/DMA-Arm, Verdacht für 0-Pakete
4. **A1** `pci_ops_reset` / `reset_trx_rings` — Ring-State sauber
5. **A3 + A5** `mac_set_edcca_mode_bands` + `cfg_phy_rpt_bands`
6. **B1/B4/B8/B10/B11/B12** phy_dm_init Rest (nhm, bb_wrap, ch_info, ul_tb, antdiv)
7. **D6 + D7** `5m_mask`, `bb_set_pop`
8. **D2b + D2c** `set_gain_offset` + `set_rxsc_rpl_comp` (braucht Efuse-Arbeit)
