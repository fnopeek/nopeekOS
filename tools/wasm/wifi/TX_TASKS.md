# WiFi TX — Multi-Session Port Roadmap

Nach v1.17 haben wir **Batch 1 der LINUX_GAPS** geschlossen
(sch_tx stop/resume, 5m_mask, bb_set_pop, txpwr_ctrl). RX läuft sauber,
aber Probe Responses kommen nicht. Die grossen Kalibrierungs-Gaps
(TSSI, DPK, set_txpwr, Efuse) sind noch offen. Dieser Plan teilt die
Arbeit in portierbare Sessions auf.

## Status nach v1.17.0 (2026-04-19)

- ✅ RX: Scan findet APs zuverlässig (+50 Beacons/5s auf ch 13)
- ✅ TX descriptor: CH8 BD ring + WD pool + `hw_idx` advances
- ✅ PA scheduler: CTN_TXEN=0xFFFF (MGQ open)
- ✅ CH8_BUSY toggles post-TX — DMA → PHY-Engine läuft
- ❌ **Kein Probe Response** — Frame geht raus aber AP empfängt nichts Verwertbares
- ❌ Kanal-Switch nach Scan klemmt (FW parked auf ch 13 persistent)
- ⏸️ IQK skipped (v1.16 diagnostic) — nicht der Blocker, wird später wieder enabled

## Verdachtsursachen (priorisiert)

1. **PA open-loop ohne TSSI** — kein Feedback auf tatsächliche Output-Power
2. **Keine DPK** — PA-Nichtlinearität unkompensiert → Modulation korrupt
3. **set_txpwr byrate/limit leer** — per-Rate-Tabelle hat nur Defaults
4. **Efuse ungelesen** — chip-spezifische Kalibrier-Offsets fehlen

## Session-Plan

### Session A — Efuse-Parser (Infrastructure)

Efuse ist die OTP-Region auf dem Chip. Enthält:
- `efuse_gain` (PA-Gain-Offsets pro path)
- `tssi_offset` (Per-channel TSSI DE-values)
- `xtal_cap` (Crystal capacitance tuning)
- Chip-MAC (nutzen wir derzeit mit pseudo `00:11:22:33:44:55`)
- RFE-Type (wir hardcoden `rfe=1` in `phy.rs`)

**Linux-Entry**: `rtw89_fw_h2c_read_efuse` oder direkt `rtw89_parse_efuse_map`.
8852BE: `rtw8852b_read_efuse` (common.c).

**Deliverable**: `tools/wasm/wifi/src/efuse.rs` mit:
- `read_efuse(mmio) -> EfuseData`
- Struct mit benötigten Feldern (pro path LNA/TIA gain, TSSI CCK/MCS DE[channel], chip MAC)

### Session B — TSSI Tables (gen_tssi.py)

TSSI hat 10 tables im Linux (rtw8852b_rfk_table.c):
```
rtw8852b_tssi_sys_defs              — set_sys  (~30 writes)
rtw8852b_tssi_init_txpwr_defs_a/b   — ini_txpwr_ctrl_bb (~20 writes × 2)
rtw8852b_tssi_init_txpwr_he_tb_defs — he_tb variant (~15 × 2)
rtw8852b_tssi_dck_defs_a/b          — set_dck  (~10 × 2)
rtw8852b_tssi_tracking_defs         — dynamic (tmeter_tbl)
rtw8852b_tssi_slope_cal_org_defs    — slope_cal_org
rtw8852b_tssi_enable_defs_a/b       — enable (~15 × 2)
rtw8852b_tssi_disable_defs          — disable (~10 writes)
```

**Deliverable**: `gen_tssi.py` (analog `gen_rfk.py`) + `src/tssi_tables.rs`.

### Session C — TSSI Implementation (Phase 1: setup-only)

Port der 14 sub-functions (aus rtw8852b_rfk.c line 2700–3050):
- `_tssi_disable` / `_tssi_enable`
- `_tssi_rf_setting`
- `_tssi_set_sys`, `_tssi_ini_txpwr_ctrl_bb`, `_tssi_set_dck`
- `_tssi_set_tmeter_tbl` (braucht thermal-cache aus efuse)
- `_tssi_set_dac_gain_tbl` (braucht efuse gain)
- `_tssi_slope_cal_org`
- `_tssi_alignment_default` (Per-channel alignment)
- `_tssi_set_tssi_slope`, `_tssi_set_tssi_track`
- `_tssi_set_efuse_to_de` (efuse → default-error table)

**Skipping für Phase 1**: `_tssi_alimentk` (alignment-cal). Das ist der
fullblown TX-auto-kalibrierter Vorgang der mehrere ms braucht und
zusätzliche HW-Pfade. Erst wenn Phase-1 TSSI läuft.

### Session D — DPK (Digital Pre-Distortion)

Größter Brocken. rtw8852b_rfk.c hat ~500 LoC `_dpk_*` functions.
DPK misst PA-Nichtlinearität und programmiert BB-Vorverzerrung.
Ohne DPK ist jede OFDM-Übertragung mit EVM weit vom Ziel.

Tables: `rtw8852b_dpk_*_defs` (~15 tables).

**Deliverable**: `gen_dpk.py` + `src/dpk_tables.rs` + `src/dpk.rs`.

### Session E — set_txpwr Pipeline (Gap 5.3)

Per-channel TX-Power-Kette:
- `set_txpwr_byrate` (R_AX_PWR_BY_RATE_TABLE0..10) — braucht efuse + regd
- `set_txpwr_offset` (R_AX_PWR_RATE_OFST_CTRL) — 5-Wert lookup
- `set_tx_shape` (DFIR table + OFDM triangular) — Per-channel pro Band
- `set_txpwr_limit` (R_AX_PWR_LMT) — Regulatory (regional)
- `set_txpwr_limit_ru` (R_AX_PWR_RU_LMT) — RU limits
- `set_txpwr_diff` (A-vs-B differential) — braucht efuse + SAR

Für Phase 1 Hardcode: **regd=FCC, SAR=0, efuse-defaults aus Session A**.

### Session F — IQK Re-enable + Integration Test

Nach A–E: IQK-Aufruf wieder aktivieren, vollständiger Durchlauf testen.
Erwarteter Durchbruch: Probe Response von ≥1 AP = TX funktioniert.

### Session G — Channel-Switch Fix (parallel)

`SCANOFLD(OP=0, TARGET_CH_MODE=1)` reicht aktuell nicht. Linux macht
zusätzlich `rtw89_chanctx_proceed` + `rtw89_mac_port_cfg_rx_sync(true)`
nach scan. Port das.

## Zeitplan (realistisch)

| Session | Inhalt | Größe |
|---------|--------|-------|
| A | Efuse-Parser | M |
| B | TSSI Tables (Script + tables.rs) | M |
| C | TSSI Implementation | L |
| D | DPK (Tables + Implementation) | XL |
| E | set_txpwr Pipeline | L |
| F | IQK restore + Integration | S |
| G | Channel-Switch Fix | S (parallel) |

**Total**: ~6-7 Sessions. Nicht alle in einer Sitzung machbar.

## Nächste Aktion (diese Session)

1. Dieses Dokument commiten
2. `gen_tssi.py` bauen (Script steht)
3. `tssi_tables.rs` generieren
4. **Nur das**. Nächste Session: Session A (Efuse) oder direkt C (TSSI setup) mit Efuse-Hardcodes.
