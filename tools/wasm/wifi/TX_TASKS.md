# WiFi TX — Multi-Session Port Roadmap

**Session-Stand: nach v1.44 (2026-04-19, Session-Ende).**

Sessions A–F dieser Roadmap sind **alle abgearbeitet**. Was Linux an
Kalibrier-Infrastruktur hat, haben wir jetzt auch — Efuse, TSSI
(inkl. alimentk), set_txpwr, IQK (clean), DPK (full). Dazu kamen
strukturelle Fixes die hier nicht geplant waren (CH8 STOP/ENABLE,
addr_info 8B, default_cmac_tbl NTX_PATH, Pre-AUTH NO_LINK).

Trotzdem transmittet der Chip nicht auf die Antenne. Siehe "Post-Port
Befund" unten — die Roadmap ist durchgearbeitet, der Bug liegt nicht
mehr in diesen Session-Scopes.

## Post-Port Befund (2026-04-19, Sniffer-Test)

Externer Sniffer (RTL8852CE in monitor mode auf ch 7, 70 s Capture
während `driver wifi` Phase 7 AUTH-Versuch):

- 30 785 Frames total von Nachbar-APs empfangen
- 20+ unique Source-MACs (alle anderen Clients + FritzBox-Virtuals)
- **0 Frames mit NUC-MAC `bc:2b:02:45:7a:20`** — in irgendeiner Rolle

Das heisst: alles was bisher als "CH8_BUSY toggled + TXBD_IDX advanced
+ TX_COUNTER inc" aussah, war **rein interne DMA/CMAC-Aktivität**.
Die Frames haben den PA / die Antenne nie erreicht.

IQK / TSSI-Setup / DPK liefen durch (ADC-Feedback funktioniert
intern), aber der Sende-Pfad zur Antenne wird nicht aktiviert.

## Status-Checkliste nach v1.44

### Hardware-Pfad (wie Linux)
- ✅ FWDL: FW läuft stabil
- ✅ MAC/PHY init komplett
- ✅ PCIe post-init inkl. HOST_ADDR_INFO_8B_SEL + WD_ADDR_INFO_LENGTH
- ✅ default_cmac_tbl mit NTX_PATH_EN=RF_AB + PATH_MAP_B=1
- ✅ CH8 TXBD-Ring mit STOP/ENABLE-Wrap, 8-byte addr_info
- ✅ PTCL init inkl. FSM_MON TX_ARB_TO_THR=2ms
- ✅ Full VIF init (8 steps)
- ✅ Pre-AUTH addr_cam + BSSID (NO_LINK korrekt gehalten)

### Kalibrierung
- ✅ Efuse: MAC + thermal + rfe + tssi cck/mcs/trim komplett gelesen
- ✅ IQK: LOK coarse, LOK fine, TXK, RXK — alle 4 flags = 0 auf
       beiden Paths nach v1.38 (RFK ausserhalb set_channel_help bracket)
- ✅ TSSI: full setup + alimentk re-enabled in v1.42 (alimentk
       CW-Rpt timet aber weiter — Details unten)
- ✅ DPK: 989 LoC Linux-1:1-Port in v1.44, alle 34 Sub-Funktionen
       + AGC FSM + drei Tabellen. Scheitert jedoch bei sync mit
       txagc=0xFF (sync_check DC/CorrVal überschreitet Thresholds)
- ✅ set_txpwr: full 6-Step-Pipeline (byrate + offset + shape + lmt +
       lmt_ru + ref), FORCE_PWR_BY_RATE OFF

### Funkpfad
- ❌ **TX on-air: 0 Frames** (Sniffer-Bestätigt)

## Noch offene Vermutungen (neue Richtung nach Sniffer-Befund)

Die bisherigen Fixes deckten das MAC-/BB-/Cal-Layer ab. Was verhindert
dass CMAC-erzeugte Samples den PA und die Antenne erreichen?

1. **RFE / Antennen-Switch-Register** — Linux hat `cfg_rfe_gpio` das für
   8852B auf NULL zeigt (`rtw89_mac_gen_ax.cfg_rfe_gpio=NULL`), aber in
   `rtw8852b.c` gibt es RFE-bezogene Init-Sequenzen. Unser `rfe=1` ist
   hardcodiert, die konkreten RFE-Write-Sequenzen haben wir aber nicht
   geprüft.

2. **BT-Coex init** — `rtw89_btc_init_cfg` konfiguriert auf 8852B
   (2-Antennen-shared-BT-WiFi) die PA-Enable-Bits für die gemeinsame
   Antenne. Wir skippen BT-Coex komplett → möglich dass PA auf BT-Side
   gepinnt bleibt und WiFi-TX nie rauskommt.

3. **`apply_txpwr_ctrl` / set_txpwr_ref Wert** — wir schreiben
   `0x02B27000` in R_DPD_A/B. Berechnet aus `bb_cal_txpwr_ref(ref=0,
   dec=0)`. Wenn die Formel für 8852B anders ist als geschätzt oder das
   falsche Register trifft, bleibt der PA-Referenz-Level auf 0 und PA
   sendet nichts.

4. **alimentk CW-Rpt-Timeout + DPK-Sync-Fail** — beide nutzen den
   gleichen PMAC-Feedback-Pfad (R_KIP_RPT1 / R_RPT_COM) und beide
   schlagen fehl. Das heisst der chip misst PMAC TX intern nicht.
   Kohärent mit "PA nicht aktiv" Hypothese.

5. **`cfg_txrx_path`** — wir haben inline einen 2G-RF_AB-Setup. Linux
   hat eine eigene Funktion die TX+RX Antenna-Enable-Bits schreibt.
   Evtl. fehlt der letzte Switch dort.

### Workflow von hier

Nicht mehr raten welcher Kalibrier-Schritt fehlt — die sind durch.
Stattdessen systematisch Linux durchsuchen nach jedem Register-Write
der zwischen MAC-Init und erstem erfolgreichen TX auf-schaltet:
PA-Enable, Antennen-Switch, RF-Power-Pin. Keine davon ist in unserer
bisherigen Arbeit garantiert abgedeckt.

---

## Ursprüngliche Session-Roadmap (historisch, alle A-F erledigt)

### Session A — Efuse-Parser ✅

Linux-Entry `rtw8852b_read_efuse` — `src/efuse.rs` implementiert.
EfuseData mit MAC (bc:2b:02:45:7a:20 real), thermal[2], rfe_type,
tssi_cck/mcs/trim — alle gelesen und im Init-Pfad verwendet.

### Session B — TSSI Tables ✅

`gen_tssi.py` + `src/tssi_tables.rs` mit 10 Linux-Tables generiert.

### Session C — TSSI Implementation ✅

`src/tssi.rs` mit 14 Sub-Funktionen
(`disable`, `rf_setting`, `set_sys`, `ini_txpwr_ctrl_bb(+he_tb)`,
`set_dck`, `set_tmeter_tbl` mit real efuse-thermal, `set_dac_gain_tbl`,
`slope_cal_org`, `alignment_default`, `set_tssi_slope`, `enable`,
`set_efuse_to_de`) + alimentk.

Alimentk re-enabled v1.42 nach IQK-Fix. CW-Rpt-Timeout bleibt (gleiche
Ursache wie DPK-Sync-Fail — siehe Befund oben).

### Session D — DPK ✅

**v1.44**: `src/dpk.rs` (~800 LoC) + `src/dpk_tables.rs` (3 tables).
Linux-1:1-Port aller 34 Sub-Funktionen inkl. AGC-FSM, fill_result,
cal_select. Scheitert bei sync — siehe oben.

### Session E — set_txpwr Pipeline ✅

**v1.43**: `chan::set_txpwr()` mit 6 Sub-Steps. byrate-Werte
1:1 aus Linux rtw8852b_table.c (rtw89_8852b_txpwr_byrate). Limit +
Limit_RU mit FCC-Approximation 0x50 (nicht full regulatory table).
Kritisch: `FORCE_PWR_BY_RATE_EN` wird OFF gelassen (Linux setzt das
nie).

### Session F — IQK Re-enable + Integration Test ✅

**v1.38**: IQK outside set_channel_help bracket → alle 8 flags = 0.
Full Linux-Audit in `iqk.rs` bestätigt: alle Register/Masken/Funktionen
byte-identisch mit rtw8852b_rfk.c.

### Session G — Channel-Switch Fix ✅

`scan_stop_to_channel()` + SCANOFLD mit TARGET_CH_MODE portiert.
Channel-Switch funktioniert (sniffer bestätigt: wir sind auf ch 7
während Phase 7, empfangen IvyPie_New Beacons dort).

---

## Appendix — Commit-Verlauf dieser Session (v1.31..v1.44)

| Version | Fix |
|---------|-----|
| v1.31 | ptcl_init_ax PCIe-Block (FSM_MON.TX_ARB_TO_THR=2ms) |
| v1.32 | TXBD length = WD_HDR_TOTAL only (no frame_len) |
| v1.33 | Active scan via H2C_ADD_PKT_OFFLOAD |
| v1.34 | INFRA switch + AUTH Open via CH8 (später revertiert) |
| v1.35 | default_cmac_tbl NTX_PATH_EN=RF_AB + PATH_MAP_B=1 |
| v1.36 | addr_info 8B mode selector (HOST_ADDR_INFO_8B_SEL) |
| v1.37 | PTCL/WMAC TX debug registers diagnostic |
| v1.38 | **RFK outside set_channel_help bracket → IQK clean** |
| v1.39 | Pre-AUTH stays NO_LINK (addr_cam only) |
| v1.40 | CH8 ring init wraps STOP_CH8 disable/enable |
| v1.41 | Non-beacon RX classifier (AUTH/ACK/DATA reveal) |
| v1.42 | TSSI alimentk re-enabled (IQK now clean) |
| v1.43 | Full Linux set_txpwr pipeline, FORCE off |
| v1.44 | **Full DPK cal Linux 1:1 (989 LoC)** |
