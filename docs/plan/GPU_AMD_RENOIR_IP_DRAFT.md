# GPU AMD Renoir (Lucienne 1002:164c) — IP-Versionen, Blöcke, Firmware (DRAFT)

**Status: DRAFT, nur aus der Quelle gelesen.** Die IP-Versionen kommen auf Renoir
zur Laufzeit aus der IP-Discovery-Tabelle; die liest das Geraet morgen. Bis dahin
ist jede Version unten eine Herleitung, keine Messung. Alle Pfade relativ zu
`linux-6.18.55/drivers/gpu/drm/amd/`.

## 0 — Wie Linux zu Renoir kommt

| Schritt | Quelle |
|---|---|
| `{0x1002, 0x164C, …, CHIP_RENOIR\|AMD_IS_APU}` | amdgpu/amdgpu_drv.c:2130 |
| `0x1636`/`0x164c` → `AMD_APU_IS_RENOIR`, sonst `AMD_APU_IS_GREEN_SARDINE` | amdgpu/amdgpu_device.c:2138-2143 |
| `amdgpu_device_ip_early_init` → `default:` → `amdgpu_discovery_set_ip_blocks` | amdgpu/amdgpu_device.c:2882-2883 |
| `CHIP_RENOIR` hat keinen eigenen Fall → `default:` → `amdgpu_discovery_reg_base_init`, `harvest_ip`, `get_gfx_info`, `get_mall_info`, `get_vcn_info` | amdgpu/amdgpu_discovery.c:2798-2810 |
| `reg_base_init` ruft `amdgpu_discovery_init` | amdgpu/amdgpu_discovery.c:1340 |
| `amdgpu_discovery_get_fw_name` → `NULL` fuer Renoir (nur `amdgpu_discovery=2` nimmt `ip_discovery.bin`) | amdgpu/amdgpu_discovery.c:418-443 |
| also Tabelle aus dem Speicher: warten auf `mmMP0_SMN_C2PMSG_33` Bit 31 (bis 2 s), `RCC_CONFIG_MEMSIZE` lesen, Tabelle bei `vram_size - DISCOVERY_TMR_OFFSET` (64 KiB), Groesse `DISCOVERY_TMR_SIZE` (10 KiB); ist MEMSIZE 0/~0 → ueber ACPI-TMR-Info aus dem Systemspeicher | amdgpu/amdgpu_discovery.c:270-315, 244-264; amdgpu/amdgpu_discovery.h:27-28 |
| Signatur + Checksumme der Tabelle | amdgpu/amdgpu_discovery.c:476-496 |

Auf einer APU ist "VRAM" der BIOS-Carve-out (stolen). Die Discovery-Tabelle liegt
also in den obersten 64 KiB des Carve-outs. **Das ist der erste Messpunkt morgen.**

Danach in `amdgpu_discovery_set_ip_blocks`, abhaengig von den gelesenen Versionen:

- `family`: GC 9.3.0 → `AMDGPU_FAMILY_RV` (amdgpu/amdgpu_discovery.c:2826-2830)
- `AMD_IS_APU` erneut fuer GC 9.3.0 (amdgpu/amdgpu_discovery.c:2877-2895)
- `nbio.funcs`, `hdp.funcs`, `df.funcs`, `smuio.funcs` je nach Version (2900-3094, Tabelle §1)

## 1 — IP fuer IP: wahrscheinliche Version, Beleg, Block

"stark" = die Quelle nennt Renoir bei genau dieser Version. "schwach" = nur
Ausschluss oder Indiz. Green Sardine (Cezanne, gleiche `CHIP_RENOIR`, andere
`apu_flags`) teilt fast alles; wo es sich trennt, steht es dabei.

| IP | Version (Renoir) | Beleg | Staerke | Block / Funcs (file:line) |
|---|---|---|---|---|
| **GC** | **9.3.0** | `case IP_VERSION(9, 3, 0): /* RENOIR */` amdkfd/kfd_device.c:150; ucode-Prefix 9.3.0 → `AMD_APU_IS_RENOIR` ? "renoir" : "green_sardine" amdgpu/amdgpu_ucode.c:1366-1369; `/* RENOIR looks like RAVEN */` amdgpu/amdgpu_gmc.c:884-885; Renoir-Golden `golden_settings_gc_9_1_rn` amdgpu/gfx_v9_0.c:1141-1145 | stark (GS gleich) | `gfx_v9_0_ip_block` amdgpu/amdgpu_discovery.c:2261-2265 (Def. amdgpu/gfx_v9_0.c:7861) |
| COMMON (an GC) | GC 9.3.0 | — | stark | `vega10_common_ip_block` amdgpu/amdgpu_discovery.c:1901-1908 (Def. amdgpu/soc15.c:707); `soc15_common_early_init` 9.3.0-Zweig: `asic_funcs = soc15_asic_funcs`, `external_rev_id = rev_id + 0x91` (GS: `+0xa1`) amdgpu/soc15.c:1160-1166 |
| GMC (an GC) | GC 9.3.0 | — | stark | `gmc_v9_0_ip_block` amdgpu/amdgpu_discovery.c:1957-1964 (Def. amdgpu/gmc_v9_0.c:2380) |
| **SDMA0** | **4.1.2** | `case IP_VERSION(4, 1, 2):/* RENOIR */` amdkfd/kfd_device.c:76; ucode 4.1.2 → `AMD_APU_IS_RENOIR` ? "renoir_sdma" : "green_sardine_sdma" amdgpu/amdgpu_ucode.c:1279-1282; Golden `golden_settings_sdma_4_3` amdgpu/sdma_v4_0.c:571-574 | stark (GS gleich) | `sdma_v4_0_ip_block` amdgpu/amdgpu_discovery.c:2317-2321 (Def. amdgpu/sdma_v4_0.c:2738); Anzahl Instanzen aus Tabelle (amdgpu/amdgpu_discovery.c:1415-1418) |
| **UVD/VCN** | **2.2.0** | ucode 2.2.0 → `AMD_APU_IS_RENOIR` ? "renoir_vcn" : "green_sardine_vcn" amdgpu/amdgpu_ucode.c:1321-1324; `rn_video_codecs_decode` amdgpu/soc15.c:214-220 | stark (GS gleich) | `vcn_v2_0_ip_block` amdgpu/amdgpu_discovery.c:2404-2405 (Def. amdgpu/vcn_v2_0.c:2200) |
| JPEG | an VCN 2.2.0 | gleicher `case` | stark | `jpeg_v2_0_ip_block` amdgpu/amdgpu_discovery.c:2406-2407, nur ohne SR-IOV (Def. amdgpu/jpeg_v2_0.c:850) |
| VCE | 0 / nicht vorhanden | `if (VCE_HWIP)` faellt in den VCN-Zweig amdgpu/amdgpu_discovery.c:2369, 2396 | schwach (Annahme: Tabelle hat kein VCE) | — |
| **DCE/DMU (DCN)** | **2.1.0** | `case IP_VERSION(2, 1, 0): fw_name_dmub = FIRMWARE_RENOIR_DMUB; if (ASICREV_IS_GREEN_SARDINE(external_rev_id)) … GREEN_SARDINE_DMUB` display/amdgpu_dm/amdgpu_dm.c:5762-5765; `DMUB_ASIC_DCN21` amdgpu_dm.c:2517-2518; DC: `FAMILY_RV` + `ASICREV_IS_RENOIR` → `DCN_VERSION_2_1` display/dc/core/dc_resource.c:159-162 | stark | `dm_ip_block` amdgpu/amdgpu_discovery.c:2202, 2221-2225 (Def. display/amdgpu_dm/amdgpu_dm.c:3715) |
| **MP0 / PSP** | **11.0.3** (GS: 12.0.1) | ucode-Prefix MP0 11.0.3 → "renoir", 12.0.1 → "green_sardine" amdgpu/amdgpu_ucode.c:1213-1214, 1232-1233; `psp_early_init`: 11.0.3 und 12.0.1 → `psp_v12_0_set_psp_funcs`, `autoload_supported = false`, `boot_time_tmr = false` amdgpu/amdgpu_psp.c:205-210 | mittel-stark (Name nur aus Versionstabelle, nicht aus `apu_flags`) | `psp_v12_0_ip_block` amdgpu/amdgpu_discovery.c:2075-2077 (Def. amdgpu/amdgpu_psp.c:4518) |
| **MP1 / SMU** | **12.0.0** (GS vermutlich 12.0.1) | beide → `renoir_set_ppt_funcs` pm/swsmu/amdgpu_smu.c:727-729; Leistungsgrenze nach FW-Version getrennt: 12.0.1 & `fw >= 0x40000f`, 12.0.0 & `fw >= 0x373200` pm/swsmu/smu12/renoir_ppt.c:1218-1223 (zwei Generationen FW → zwei Chips) | schwach fuer die Zuordnung 12.0.0↔Renoir; stark fuer den Block | `smu_v12_0_ip_block` amdgpu/amdgpu_discovery.c:2140-2142 (Def. pm/swsmu/amdgpu_smu.c:2771); `get_xclk` → 10000 fuer beide amdgpu/soc15.c:349-351; Reset = MODE2 fuer beide amdgpu/soc15.c:550-554 |
| **OSSSYS / IH** | 4.x aus {4.0.0, 4.0.1, 4.1.0, 4.1.1, 4.3.0}; Kandidat **4.3.0** | Block sicher: `vega10_ih.c` hat zwei `CHIP_RENOIR`-Zweige (IH_CHICKEN amdgpu/vega10_ih.c:275-282, IH_CLK_CTRL amdgpu/vega10_ih.c:588-590). Version: 4.0.0/4.0.1/4.1.0/4.1.1 sind von Vega10/12/Raven/Raven2 hartcodiert belegt (amdgpu/amdgpu_discovery.c:2575, 2604, 2635, 2652), 4.3.0 von keinem | Block stark, Version schwach | `vega10_ih_ip_block` amdgpu/amdgpu_discovery.c:2007-2012 (Def. amdgpu/vega10_ih.c:652). **Achtung:** die Renoir-Zweige haengen an `asic_type`, also auch fuer GS |
| **NBIO / NBIF** | aus {7.0.0, 7.0.1, 2.5.0} → `nbio_v7_0_funcs`; Kandidat **2.5.0** | 7.0.0/7.0.1 sind Raven/Raven2 (amdgpu/amdgpu_discovery.c:2639, 2656); 2.5.0 hat keinen hartcodierten Besitzer, aber einen eigenen Zweig in `nbio_v7_0_init_registers` (RCC_DEV0_EPF6_STRAP4 Bit 23) amdgpu/nbio_v7_0.c:281-284 | schwach | `adev->nbio.funcs = &nbio_v7_0_funcs` amdgpu/amdgpu_discovery.c:2907-2911 (Tabelle amdgpu/nbio_v7_0.c:302). Hinweis: alter Pfad `soc15_reg_base_init` legt Renoir auf `vega10_reg_base_init` amdgpu/soc15.c:722-723 — im Discovery-Pfad gilt das NICHT |
| **HDP** | 4.x; Kandidat **4.1.2** | jede 4.x → `hdp_v4_0_funcs`; 4.1.2 hat keinen hartcodierten Besitzer (Muster wie SDMA 4.1.2) | Block stark, Version schwach | amdgpu/amdgpu_discovery.c:2971-2982 (Tabelle amdgpu/hdp_v4_0.c:171) |
| **MMHUB** | Kandidat **1.5.0** | Fault-Client-Tabelle `mmhub_client_ids_renoir` fuer 1.5.0 und 2.4.0 amdgpu/gmc_v9_0.c:714-718; 2.4.0 erscheint auch in `mmhub_v2_3.c:95` (gmc_v10-Welt) | schwach | `gmc_v9_0_set_mmhub_funcs` default → `mmhub_v1_0_funcs` amdgpu/gmc_v9_0.c:1479-1481 (gilt fuer jede Version ausser 9.4.x/1.8.x). Renoir-Sonderfaelle: System-Apertur +1 amdgpu/mmhub_v1_0.c:99-101, CG `CHIP_RENOIR` amdgpu/mmhub_v1_0.c:588 |
| GFXHUB | an GC | — | stark | `gfxhub_v1_0_funcs` (nicht multi-AID) amdgpu/gmc_v9_0.c:1510-1513; Apertur +1 fuer RENOIR amdgpu/gfxhub_v1_0.c:105-107 |
| **ATHUB** | Kandidat **1.5.0** | `athub_v1_0_set_clockgating` nennt 9.0/9.1/9.2/9.3/9.4 und 1.5.0 amdgpu/athub_v1_0.c:72-77; Renoir hat `AMD_CG_SUPPORT_ATHUB_*` amdgpu/soc15.c:1183-1184 | schwach | kein eigener Block (Teil von GMC) |
| **DF** | aus {2.1.0, 2.1.1, 2.5.0, 3.5.1, 3.5.2} → `df_v1_7_funcs` | Quelle sagt nichts Renoir-spezifisches; `AMD_CG_SUPPORT_DF_MGCG` gesetzt amdgpu/soc15.c:1185 | **unbekannt** | amdgpu/amdgpu_discovery.c:3008-3019 (Tabelle amdgpu/df_v1_7.c:121) |
| **SMUIO** | aus {9.0.0, 9.0.1, 10.0.0, 10.0.1, 10.0.2} → `smuio_v9_0_funcs`, oder 11.x | 10.0.2 ohne hartcodierten Besitzer | **unbekannt** | amdgpu/amdgpu_discovery.c:3035-3041 (Tabelle amdgpu/smuio_v9_0.c:72; hat KEIN `get_pkg_type`, Ruf bei 3067 nur fuer 13.0.3/13.0.11) |
| **THM** | — | kein Verbraucher im Discovery-Pfad; nur `reg_offset` | **unbekannt** | — |
| **CLK** | — | Version nirgends gelesen; DC nimmt `reg_offset[CLK_HWIP][0]` display/amdgpu_dm/amdgpu_dm.c:2042; `rn_clk_mgr_construct` fuer Renoir UND GS display/dc/clk_mgr/clk_mgr.c:231-239 | **unbekannt** (egal fuer Blockwahl) | — |
| **UMC** | — | `gmc_v9_0_set_umc_funcs` kennt keine APU-Version amdgpu/gmc_v9_0.c:1414-1455 → `umc.funcs` bleibt NULL (sofern nicht 6.0.0) | **unbekannt** | — |
| ISP | — | `isp` nur fuer 4.1.0/4.1.1 amdgpu/amdgpu_discovery.c:2539-2552 | — | keiner zu erwarten |
| MES/VPE/UMSCH | — | nur GC 11/12, VPE 6.1, VCN 4.0.5/6 | sicher keiner | — |

**DC rechnet NICHT mit Discovery-Basen:** `dcn21_resource.c`, `rn_clk_mgr.c`,
`irq_service_dcn21.c`, `hw_*_dcn21.c` und `dmub_dcn21.c` binden die
compile-time-Tabelle `include/renoir_ip_offset.h` ein (z. B. `DMU_BASE`,
`CLK_BASE`, `MP1_BASE` dort Z. 51-156). Die Discovery-Basen gelten fuer den
amdgpu-Kern; beide muessen am Geraet uebereinstimmen — ein billiger Abgleich
morgen.

## 2 — Reihenfolge der Bloecke (Init folgt ihr)

`amdgpu_discovery_set_ip_blocks`, Bare-Metal (kein SR-IOV), `load_type = PSP` (§3):

| # | Aufruf (amdgpu_discovery.c) | Block fuer Renoir | Typ |
|---|---|---|---|
| 0 | `set_common_ip_blocks` :3099 | `vega10_common_ip_block` | COMMON |
| 1 | `set_gmc_ip_blocks` :3103 | `gmc_v9_0_ip_block` | GMC |
| 2 | `set_ih_ip_blocks` :3116 (nicht-SR-IOV-Zweig) | `vega10_ih_ip_block` | IH |
| 3 | `set_psp_ip_blocks` :3120-3121 (nur bei `FW_LOAD_PSP`) | `psp_v12_0_ip_block` | PSP |
| 4 | `set_smu_ip_blocks` :3127-3128 (nur bei `FW_LOAD_PSP`) | `smu_v12_0_ip_block` | SMC |
| 5 | `set_display_ip_blocks` :3133 | `dm_ip_block` (falls `amdgpu_device_has_dc_support`, Vorgabe `amdgpu_dc != 0` amdgpu/amdgpu_device.c:4286) | DCE |
| 6 | `set_gc_ip_blocks` :3137 | `gfx_v9_0_ip_block` | GFX |
| 7 | `set_sdma_ip_blocks` :3141 | `sdma_v4_0_ip_block` | SDMA |
| – | :3145-3150 SMU nochmals, nur bei `FW_LOAD_DIRECT` | entfaellt | |
| 8 | `set_mm_ip_blocks` :3153 | `vcn_v2_0_ip_block` | VCN |
| 9 | (derselbe Aufruf) | `jpeg_v2_0_ip_block` | JPEG |
| – | `set_mes` :3157, `set_vpe` :3161, `set_umsch_mm` :3165, `set_isp` :3169 | nichts | |

Danach in `amdgpu_device_ip_init`: `sw_init` aller Bloecke der Reihe nach
(amdgpu/amdgpu_device.c:3201), COMMON und GMC sofort `hw_init` (3218, 3238);
`hw_init_phase1` faehrt COMMON und IH (3006-3021); `amdgpu_device_fw_loading`
faehrt PSP (3061-3088) und dann `amdgpu_pm_load_smu_firmware` (3103-3104) —
das kehrt auf einer swSMU-APU sofort mit 0 zurueck (pm/amdgpu_dpm.c:677-679);
`hw_init_phase2` faehrt den Rest (3048).

## 3 — Firmware

### Ladeart

`amdgpu_ucode_get_load_type`: `CHIP_RENOIR` faellt in `default:` —
`load_type == 0` → DIRECT, `== 3` → RLC_BACKDOOR_AUTO, sonst PSP
(amdgpu/amdgpu_ucode.c:534-575). Vorgabe `amdgpu_fw_load_type = -1`
(amdgpu/amdgpu_drv.c:163) → **`AMDGPU_FW_LOAD_PSP`**. Aufgerufen in
`amdgpu_device_check_arguments` (amdgpu/amdgpu_device.c:2226), also VOR der
Blockwahl — die Wahl in §2 haengt daran.

PSP auf Renoir (`psp_v12_0`):
- `psp_v12_0_init_microcode` holt nur **ASD** und **TA** (amdgpu/psp_v12_0.c:44-66).
  Kein SOS, kein SYS-Treiber, kein TOC: SOS laeuft bereits (vom BIOS/PSP-Bootloader
  der APU). `psp_hw_start` ueberspringt jeden Bootloader-Schritt, dessen Blob
  nicht gueltig ist (`is_psp_fw_valid`, amdgpu/amdgpu_psp.c:2503-2570).
- `autoload_supported = false`, `boot_time_tmr = false` (amdgpu/amdgpu_psp.c:205-210).
- Secure Display nur auf Renoir, bei GS auf 0 gesetzt (amdgpu/psp_v12_0.c:62-64).
- GFX/SDMA/VCN-Ucode geht ueber den PSP-Ring (`psp_ring_cmd_submit`), nicht per MMIO:
  der DIRECT-Zweig ist in `gfx_v9_0.c:3207`, `:3945` und `sdma_v4_0.c:1392`
  ausgeschlossen; VCN prueft `load_type == PSP` (amdgpu/vcn_v2_0.c:391, 446).
- SMU-FW: keine Datei. MP1 12.x hat keinen ucode-Prefix (amdgpu/amdgpu_ucode.c:1240-1265),
  und `amdgpu_pm_load_smu_firmware` laedt auf der APU nichts (pm/amdgpu_dpm.c:677-679).
  Die SMU-FW kommt aus dem SBIOS.

### Dateien, die Renoir anfordert

Prefix `renoir` kommt aus `amdgpu_ucode_ip_version_decode` (§1). `.zst` loest der
Firmware-Lader auf.

| Datei | Anforderung | /lib/firmware/amdgpu |
|---|---|---|
| `renoir_pfp.bin` | amdgpu/gfx_v9_0.c:1459-1461 | ✔ eigene Datei (9998 B) |
| `renoir_me.bin` | amdgpu/gfx_v9_0.c:1466-1468 | ✔ eigene Datei (6497 B) |
| `renoir_ce.bin` | amdgpu/gfx_v9_0.c:1473-1475 | ✔ eigene Datei (3354 B) |
| `renoir_rlc.bin` | amdgpu/gfx_v9_0.c:1521-1523 (picasso_am4/raven_kicker nur per Name) | ✔ → Symlink auf `green_sardine_rlc.bin.zst` |
| `renoir_mec.bin` | amdgpu/gfx_v9_0.c:1558-1560 | ✔ → Symlink auf `green_sardine_mec.bin.zst` |
| `renoir_mec2.bin` | **nicht angefordert**: `gfx_v9_0_load_mec2_fw_bin_support` = false fuer 9.3.0 (amdgpu/gfx_v9_0.c:1538-1545) | liegt trotzdem da (Symlink) |
| `renoir_sdma.bin` | amdgpu/amdgpu_sdma.c:215-218 (Instanz 0; weitere als `renoir_sdma1.bin` :220-222) | ✔ → Symlink `green_sardine_sdma.bin.zst` |
| `renoir_vcn.bin` | amdgpu/amdgpu_vcn.c:114-116 | ✔ → Symlink `green_sardine_vcn.bin.zst` |
| `renoir_asd.bin` | amdgpu/amdgpu_psp.c:3497-3498 | ✔ → Symlink `green_sardine_asd.bin.zst` |
| `renoir_ta.bin` | amdgpu/amdgpu_psp.c:3975-3976 (`_ta_kicker` nur falls `amdgpu_is_kicker_fw`) | ✔ **eigene** Datei (14675 B, GS 13888 B) |
| `renoir_dmcub.bin` | display/amdgpu_dm/amdgpu_dm.c:5763, 5817 | ✔ → Symlink `green_sardine_dmcub.bin.zst` |
| `renoir_gpu_info.bin` | **nicht angefordert**: `amdgpu_device_parse_gpu_info_fw` kennt kein `CHIP_RENOIR` (amdgpu/amdgpu_device.c:2674-2720) | liegt da (66 B) |
| `ip_discovery.bin` | nur mit `amdgpu_discovery=2` | nicht vorhanden, nicht noetig |

Befund: ausser `pfp/me/ce/ta` sind alle Renoir-Dateien in linux-firmware Symlinks
auf Green Sardine. Die Dateien selbst sind also fuer beide gleich; nur der Name
haengt am Prefix.

**Falle MP0:** waere Lucienne MP0 = 12.0.1, hiesse der Prefix `green_sardine`
(amdgpu/amdgpu_ucode.c:1232-1233), und es kaemen `green_sardine_asd/ta.bin` —
unabhaengig von `apu_flags`. Erste Pruefung morgen.

## 4 — Folge-Entscheidungen fuer den Probe-Pfad (DRAFT)

Format wie `tools/gpu_intel_tgl.follow`. `--dir` =
`…/amdgpu-callgraph/linux-6.18.55/drivers/gpu/drm/amd`; die `@`-Stellen sind
so geschrieben, wie `linux-callgraph.py reach amdgpu_pci_probe` sie ausgibt
(127 offene Mitglieder im Lauf heute). **Alles haengt an den Versionen aus §1
und wird nach dem Discovery-Lauf neu geprueft.** Zeilen mit `?` haengen an einer
"schwach"/"unbekannt"-Version.

```
# Indirect calls in amdgpu resolved for Renoir/Lucienne (1002:164c), Linux 6.18.55.
# DRAFT — pending the device's IP discovery table.
# Assumed: GC 9.3.0, SDMA 4.1.2, VCN 2.2.0, DCN 2.1.0, MP0 11.0.3, MP1 12.0.x,
#   OSSSYS 4.x, HDP 4.x, NBIO in {7.0.0,7.0.1,2.5.0}, load_type PSP, no SR-IOV.

# --- ip_block funcs: amdgpu_discovery_set_ip_blocks order (amdgpu_discovery.c:3099-3153)
#     COMMON GMC IH PSP SMC DCE GFX SDMA VCN JPEG
@amdgpu_device.c:2933 = soc15_common_early_init
@amdgpu_device.c:2933 = gmc_v9_0_early_init
@amdgpu_device.c:2933 = vega10_ih_early_init
@amdgpu_device.c:2933 = psp_early_init
@amdgpu_device.c:2933 = smu_early_init
@amdgpu_device.c:2933 = dm_early_init
@amdgpu_device.c:2933 = gfx_v9_0_early_init
@amdgpu_device.c:2933 = sdma_v4_0_early_init
@amdgpu_device.c:2933 = vcn_v2_0_early_init
@amdgpu_device.c:2933 = jpeg_v2_0_early_init
@amdgpu_device.c:3201 = soc15_common_sw_init
@amdgpu_device.c:3201 = gmc_v9_0_sw_init
@amdgpu_device.c:3201 = vega10_ih_sw_init
@amdgpu_device.c:3201 = psp_sw_init
@amdgpu_device.c:3201 = smu_sw_init
@amdgpu_device.c:3201 = dm_sw_init
@amdgpu_device.c:3201 = gfx_v9_0_sw_init
@amdgpu_device.c:3201 = sdma_v4_0_sw_init
@amdgpu_device.c:3201 = vcn_v2_0_sw_init
@amdgpu_device.c:3201 = jpeg_v2_0_sw_init
# COMMON then GMC early (amdgpu_device.c:3216-3238)
@amdgpu_device.c:3218 = soc15_common_hw_init
@amdgpu_device.c:3238 = gmc_v9_0_hw_init
# hw_init_phase1: COMMON + IH
@amdgpu_device.c:3021 = soc15_common_hw_init
@amdgpu_device.c:3021 = vega10_ih_hw_init
# fw_loading: PSP (asic_type >= CHIP_VEGA10)
@amdgpu_device.c:3088 = psp_hw_init
# hw_init_phase2: rest in block order
@amdgpu_device.c:3048 = smu_hw_init
@amdgpu_device.c:3048 = dm_hw_init
@amdgpu_device.c:3048 = gfx_v9_0_hw_init
@amdgpu_device.c:3048 = sdma_v4_0_hw_init
@amdgpu_device.c:3048 = vcn_v2_0_hw_init
@amdgpu_device.c:3048 = jpeg_v2_0_hw_init
@amdgpu_device.c:3570 = soc15_common_late_init
@amdgpu_device.c:3570 = gmc_v9_0_late_init
@amdgpu_device.c:3570 = smu_late_init
@amdgpu_device.c:3570 = dm_late_init
@amdgpu_device.c:3570 = gfx_v9_0_late_init
@amdgpu_device.c:3570 = sdma_v4_0_late_init
# set_clockgating_state / set_powergating_state follow the same tables
#   (soc15.c:1513, gmc_v9_0.c:2362, vega10_ih.c:624, amdgpu_psp.c:4473,
#    amdgpu_smu.c:2744, amdgpu_dm.c:3696, gfx_v9_0.c:7419, sdma_v4_0.c:2393,
#    vcn_v2_0.c:2096, jpeg_v2_0.c:785)

# --- asic: soc15.c:1160-1161 GC 9.3.0 -> soc15_asic_funcs (soc15.c:900)
@amdgpu_device.c:463  = soc15_supports_baco
@amdgpu_device.c:1448 = soc15_pre_asic_init
@amdgpu_device.c:1830 = soc15_get_config_memsize
@amdgpu_device.c:4777 = soc15_need_reset_on_init
# soc15_need_reset_on_init returns true for CHIP_RENOIR (soc15.c:856-858)
@amdgpu_device.c:4791 = soc15_asic_reset
@amdgpu_doorbell_mgr.c:207 = vega10_doorbell_index_init
@amdgpu_bios.c:176 = amdgpu_soc15_read_bios_from_rom
@amdgpu_bios.c:197 = amdgpu_soc15_read_bios_from_rom
@amdgpu_bios.c:355 = soc15_read_disabled_bios
# soc15_asic_funcs has no flush_hdp/invalidate_hdp -> hdp.funcs (amdgpu.h:1540-1544)
@amdgpu_device.c:7403 = amdgpu_hdp_generic_flush
@amdgpu_device.c:7416 = hdp_v4_0_invalidate_hdp

# --- indirect register access: soc15_common_early_init (soc15.c:971-978)
@amdgpu_device.c:744 = amdgpu_device_indirect_rreg
@amdgpu_device.c:871 = amdgpu_device_indirect_wreg

# --- NBIO ? (amdgpu_discovery.c:2907-2911 -> nbio_v7_0_funcs, nbio_v7_0.c:302)
@amdgpu_device.c:991  = nbio_v7_0_get_pcie_index_offset
@amdgpu_device.c:992  = nbio_v7_0_get_pcie_data_offset
# nbio_v7_0_funcs has no get_pcie_index_hi_offset: amdgpu_device.c:999 not taken
@amdgpu_device.c:1278 = nbio_v7_0_get_rev_id
get_memsize        = nbio_v7_0_get_memsize
ih_control         = nbio_v7_0_ih_control
init_registers     = nbio_v7_0_init_registers
set_reg_remap      = nbio_v7_0_set_reg_remap
remap_hdp_registers = nbio_v7_0_remap_hdp_registers
sdma_doorbell_range = nbio_v7_0_sdma_doorbell_range
vcn_doorbell_range = nbio_v7_0_vcn_doorbell_range
ih_doorbell_range  = nbio_v7_0_ih_doorbell_range
enable_doorbell_aperture = nbio_v7_0_enable_doorbell_aperture
enable_doorbell_selfring_aperture = nbio_v7_0_enable_doorbell_selfring_aperture
mc_access_enable   = nbio_v7_0_mc_access_enable
# --- HDP 4.x (amdgpu_discovery.c:2982)
init_registers     = hdp_v4_0_init_registers
# --- DF ? (amdgpu_discovery.c:3019, df_v1_7.c:121)
sw_init            = df_v1_7_sw_init
enable_broadcast_mode = df_v1_7_enable_broadcast_mode
get_fb_channel_number = df_v1_7_get_fb_channel_number
# --- SMUIO ? (amdgpu_discovery.c:3041, smuio_v9_0.c:72)
get_rom_index_offset = smuio_v9_0_get_rom_index_offset
get_rom_data_offset  = smuio_v9_0_get_rom_data_offset
# amdgpu_discovery.c:3067 (get_pkg_type) only for SMUIO 13.0.3/13.0.11: not taken

# --- GMC: gmc_v9_0.c:1410, 1481, 1513, 761
@amdgpu_gmc.c:682 = gmc_v9_0_flush_gpu_tlb
@amdgpu_gmc.c:686 = gmc_v9_0_flush_gpu_tlb
@amdgpu_gmc.c:689 = gmc_v9_0_flush_gpu_tlb
@amdgpu_gmc.c:128 = gmc_v9_0_get_vm_pde
get_vbios_fb_size  = gmc_v9_0_get_vbios_fb_size
need_reset_on_init = gmc_v9_0_need_reset_on_init
get_fb_location    = mmhub_v1_0_get_fb_location
init               = mmhub_v1_0_init
gart_enable        = mmhub_v1_0_gart_enable
gart_enable        = gfxhub_v1_0_gart_enable
init               = gfxhub_v1_0_init
get_mc_fb_offset   = gfxhub_v1_0_get_mc_fb_offset
setup_vm_pt_regs   = mmhub_v1_0_setup_vm_pt_regs
setup_vm_pt_regs   = gfxhub_v1_0_setup_vm_pt_regs
set_fault_enable_default = mmhub_v1_0_set_fault_enable_default
set_fault_enable_default = gfxhub_v1_0_set_fault_enable_default
@amdgpu_device.c:4731 = gfxhub_v1_1_get_xgmi_info
# umc.funcs stays NULL unless UMC 6.0.0 (gmc_v9_0.c:1414-1417)

# --- IH: vega10_ih.c:649 -> vega10_ih_funcs (vega10_ih.c:640)
get_wptr  = vega10_ih_get_wptr
decode_iv = amdgpu_ih_decode_iv_helper
set_rptr  = vega10_ih_set_rptr

# --- PSP: amdgpu_psp.c:205-207 -> psp_v12_0_funcs (psp_v12_0.c:280)
init_microcode         = psp_v12_0_init_microcode
bootloader_load_sysdrv = psp_v12_0_bootloader_load_sysdrv
bootloader_load_sos    = psp_v12_0_bootloader_load_sos
ring_create            = psp_v12_0_ring_create
ring_stop              = psp_v12_0_ring_stop
@amdgpu_psp.c:3455 = psp_v12_0_ring_get_wptr
@amdgpu_psp.c:3487 = psp_v12_0_ring_set_wptr
@amdgpu_psp.c:3418 = psp_v12_0_mode1_reset
# psp_v12_0_funcs has no wait_for_bootloader: amdgpu_psp.c:2453 not taken

# --- SMU: amdgpu_smu.c:727-729 -> renoir_ppt_funcs (renoir_ppt.c:1450),
#     amdgpu_smu.c:814 pp_funcs = swsmu_pm_funcs (amdgpu_smu.c:3910)
@pm/amdgpu_dpm.c:530 = smu_read_sensor
# pm/amdgpu_dpm.c:682 (load_firmware) not reached: APU + swSMU returns at :677-679
check_fw_status        = smu_v12_0_check_fw_status
check_fw_version       = smu_v12_0_check_fw_version
send_smc_msg_with_param = smu_cmn_send_smc_msg_with_param
send_smc_msg           = smu_cmn_send_smc_msg
init_smc_tables        = renoir_init_smc_tables
set_default_dpm_table  = smu_v12_0_set_default_dpm_tables
set_driver_table_location = smu_v12_0_set_driver_table_location
get_enabled_mask       = renoir_get_enabled_mask
is_dpm_running         = renoir_is_dpm_running
get_vbios_bootup_values = smu_v12_0_get_vbios_bootup_values
gfx_off_control        = smu_v12_0_gfx_off_control
set_gfx_cgpg           = smu_v12_0_set_gfx_cgpg
powergate_sdma         = smu_v12_0_powergate_sdma
dpm_set_vcn_enable     = renoir_dpm_set_vcn_enable
dpm_set_jpeg_enable    = renoir_dpm_set_jpeg_enable
set_watermarks_table   = renoir_set_watermarks_table
get_dpm_clock_table    = renoir_get_dpm_clock_table
mode2_reset            = smu_v12_0_mode2_reset

# --- GFX 9.3.0: gfx_v9_0.c:4833, 7696-7698, 1093, 7629-7640
#     no MEC2 blob (gfx_v9_0.c:1538-1545)
get_gpu_clock_counter = gfx_v9_0_get_gpu_clock_counter
select_se_sh       = gfx_v9_0_select_se_sh
init               = gfx_v9_0_rlc_init
resume             = gfx_v9_0_rlc_resume
start              = gfx_v9_0_rlc_start
stop               = gfx_v9_0_rlc_stop
reset              = gfx_v9_0_rlc_reset
is_rlc_enabled     = gfx_v9_0_is_rlc_enabled
set_safe_mode      = gfx_v9_0_set_safe_mode
unset_safe_mode    = gfx_v9_0_unset_safe_mode
get_csb_size       = gfx_v9_0_get_csb_size
get_csb_buffer     = gfx_v9_0_get_csb_buffer
get_cp_table_num   = gfx_v9_0_cp_jump_table_num
set_wptr           = gfx_v9_0_ring_set_wptr_gfx
set_wptr           = gfx_v9_0_ring_set_wptr_compute
insert_nop         = gfx_v9_ring_insert_nop
insert_nop         = amdgpu_ring_insert_nop
emit_fence         = gfx_v9_0_ring_emit_fence
emit_fence         = gfx_v9_0_ring_emit_fence_kiq
@amdgpu_gfx.c:1100 = gfx_v9_0_ring_emit_rreg
@amdgpu_gfx.c:1172 = gfx_v9_0_ring_emit_wreg
emit_hdp_flush     = gfx_v9_0_ring_emit_hdp_flush
test_ring          = gfx_v9_0_ring_test_ring
begin_use          = amdgpu_gfx_enforce_isolation_ring_begin_use
end_use            = amdgpu_gfx_enforce_isolation_ring_end_use

# --- SDMA 4.1.2: one ring per instance, no page queue
#     (sdma_v4_0_fw_support_paging_queue default false, sdma_v4_0.c:1737-1752)
set_wptr           = sdma_v4_0_ring_set_wptr
insert_nop         = sdma_v4_0_ring_insert_nop
emit_fence         = sdma_v4_0_ring_emit_fence
emit_hdp_flush     = sdma_v4_0_ring_emit_hdp_flush
test_ring          = sdma_v4_0_ring_test_ring
test_ib            = sdma_v4_0_ring_test_ib
pad_ib             = sdma_v4_0_ring_pad_ib
@amdgpu_ttm.c:243  = sdma_v4_0_emit_copy_buffer
@amdgpu_ttm.c:2391 = sdma_v4_0_emit_fill_buffer

# --- VCN 2.0 / JPEG 2.0 rings: vcn_v2_0.c:2178-2197, jpeg_v2_0.c:836-847
#     (vcn_v2_0_dec_ring_vm_funcs, vcn_v2_0_enc_ring_vm_funcs, jpeg_v2_0_dec_ring_vm_funcs)

# --- display: amdgpu_dm.c:5762-5765 DMCUB fw, :2517 DMUB_ASIC_DCN21,
#     dc_resource.c:159-160 DCN_VERSION_2_1 -> dcn21_create_resource_pool (:289-290)
#     -> dcn21_hw_sequencer_construct (dcn21_resource.c:1680),
#     clk_mgr.c:231-233 rn_clk_mgr_construct. DC tables are their own pass.

# Not taken on Renoir: every SR-IOV branch (amdgpu_virt.c:*),
# XCP (amdgpu_xcp.c:*, aqua_vanjaram only, amdgpu_discovery.c:2494-2505),
# RAS/ACA/MCA/CPER (no RAS on this APU — to confirm with the device's
# ras_hw_enabled), userq, MES rreg/wreg (no MES on GC 9).
```

## 5 — Was das Geraet morgen beantworten muss

1. Discovery-Tabelle: Header-Signatur, Version, Ort (Carve-out-Ende − 64 KiB, §0).
2. Die Versionen aller Zeilen mit "schwach"/"unbekannt": **MP0** (11.0.3 vs 12.0.1 —
   entscheidet den PSP-FW-Namen), **MP1** (12.0.0 vs 12.0.1), **OSSSYS**,
   **NBIO** (entscheidet `nbio_v7_0` und den EPF6-Zweig), **HDP**, **MMHUB**,
   **ATHUB**, **DF**, **SMUIO**, **UMC**, THM/CLK.
3. Abgleich Discovery-Basen gegen `include/renoir_ip_offset.h` (DC liest Letztere).
4. `pdev->revision` → `external_rev_id = rev_id + 0x91` muss in
   `[RENOIR_A0=0x91, RAVEN1_F0)` liegen (display/include/dal_asic_id.h:167, 206),
   sonst nimmt DC den Green-Sardine-DMCUB.
5. SDMA-Instanzen und VCN-Instanzen aus der Tabelle (Harvest-Bits).
