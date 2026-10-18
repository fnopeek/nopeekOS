# gpu_amd_renoir gegen Linux — die Karte

**Stand:** 2026-10-18 · Modul `gpu_amd_renoir` 0.1.1 (Stufe 0) · Referenz **Linux 6.18.55**, amdgpu
**Geraet:** IdeaPad, Ryzen 5 5500U, iGPU `1002:164c` Lucienne = `CHIP_RENOIR | AMD_IS_APU`, `AMD_APU_IS_RENOIR`
**Papier darueber:** `GPU_STACK.md` §7.2 und §9 (Stufen 8, 9), `GPU_IN_VM.md`
**Vorlage:** `GPU_INTEL_LINUX_MAP.md` — dieselbe Regel: jede Linux-Funktion auf
dem Weg wird vollstaendig portiert, auch die „vermutlich no-op".
**IP-Bloecke, Versionen, Firmware aus der Quelle:** `GPU_AMD_RENOIR_IP_DRAFT.md`
(aus der Quelle; die Discovery-Tabelle des Geraets bestaetigt ihn, §2).

---

## 0 — Werkzeug und Wiederholbarkeit

```
~/.cache/nopeekos/amdgpu-callgraph/linux-6.18.55        amdgpu, alle Chips, mit -fcallgraph-info
~/.cache/nopeekos/amdgpu-callgraph/linux-6.18.55-renoir dieselbe, auf Renoir festgelegt
tools/amdgpu-renoir-specialize.py   legt eine KOPIE fest: asic_type, apu_flags,
                                    AMD_IS_APU, sriov_vf, amdgpu_ip_version
tools/gpu_amd_renoir.ipversions     die festgelegten IP-Versionen (nur belegte;
                                    fehlende bleiben Laufzeitwerte)
tools/linux-callgraph.py            der Graph-Lauf (wie bei i915)
tools/wasm/gpu_amd_renoir/gen_regs.py  jede Konstante, jedes Layout, jede Tabelle
                                    rechnet GCC mit den Flags des Kernelbaus aus
```

Bau des Graphen (beide Baeume gleich, der zweite nach der Festlegung):

```
make x86_64_defconfig
scripts/config -m DRM_AMDGPU -e DRM_AMD_DC -e DRM_AMD_DC_FP -d DRM_AMDGPU_SI \
    -d DRM_AMDGPU_CIK -d DRM_AMDGPU_USERPTR -d DRM_AMD_ACP -d HSA_AMD \
    -d DRM_AMD_SECURE_DISPLAY -d WERROR -d DRM_AMDGPU_WERROR
make olddefconfig && make -j12 modules_prepare
make -j12 M=drivers/gpu/drm/amd/amdgpu modules \
    KCFLAGS="-fcallgraph-info -fno-inline -fno-inline-small-functions -fno-inline-functions-called-once"
```

(modpost scheitert danach mangels Module.symvers; die `.ci` sind vorher
geschrieben — wie bei i915.) Der festgelegte Baum: Kernel per Hardlink, `amd/`
als echte Kopie, dann `amdgpu-renoir-specialize.py <tree>/drivers/gpu/drm/amd
tools/gpu_amd_renoir.ipversions`, dann derselbe `make`.

Graph-Lauf:

```
python3 tools/linux-callgraph.py --dir <tree>/drivers/gpu/drm/amd reach amdgpu_pci_probe [--list|--tree]
```

Ohne `--follow` erreicht `amdgpu_pci_probe` nur rund 800 Funktionen: amdgpu
waehlt fast alles ueber Funktionszeiger (`ip_funcs`, `gfx.funcs`, `ring->funcs`,
`psp->funcs`, `smu->ppt_funcs`, `nbio.funcs` …). Die Entscheidung, welche
Funktion auf Renoir dort sitzt, steht mit der Quellzeile in
`tools/gpu_amd_renoir.follow` (Entwurf: `GPU_AMD_RENOIR_IP_DRAFT.md` §4).

### Unterschied zu i915, der die Festlegung praegt

Renoir hat **keine feste IP-Tabelle im Treiber**: `amdgpu_discovery_set_ip_blocks`
nimmt fuer CHIP_RENOIR den `default`-Zweig (`amdgpu_discovery_reg_base_init`).
Versionen und Registerbasen jedes IP-Blocks kommen zur Laufzeit aus der
Discovery-Tabelle oben im Carve-out. Die Festlegung laeuft deshalb ueber
`amdgpu_ip_version()`; `.ipversions` traegt die 19 Versionen, die das Geraet
in Stufe 0 meldet (§2).

**DC (Anzeige) rechnet nicht mit den Discovery-Basen**, sondern mit
`include/renoir_ip_offset.h`. Stufe 0 vergleicht beide am Geraet.

**DC hat seine eigene Chip-Kennung** (`struct hw_asic_id`, `dce_version`) und
verzweigt ueberall darauf. Die Festlegung setzt `dce_version` →
`DCN_VERSION_2_1`, `chip_family` → `FAMILY_RV`, `hw_internal_rev` →
`RENOIR_A0` (0x91, die kleinste, die `ASICREV_IS_RENOIR` annimmt) und den
Schalter in `dc_create_resource_pool`. `external_rev_id` aus Stufe 0 muss dazu
passen. Jede Ersetzung haelt die Zeilenzahl, damit die `file:line`-Stellen der
`.follow`-Datei in beiden Baeumen gelten.

**Stand des Graphen (2026-10-18, Geraeteversionen):** `reach amdgpu_pci_probe` mit
`tools/gpu_amd_renoir.follow` — ungefiltert 2636 Funktionen, festgelegt
**1636** (mit den fuenf Entwurfsversionen waren es 1709; die Geraetewerte
fuer MP1, UMC und MMHUB nehmen die SMU-Zweige fremder Chips und die
RAS-EEPROM-Pfade heraus); von DC bleiben `dcn21` und die `dcn20`-Teile, die DCN 2.1 in Linux
mitbenutzt. Liste und Baum: `GPU_AMD_RENOIR_FUNCTIONS.txt`,
`GPU_AMD_RENOIR_TREE.txt` (Klassen A/B/N noch nicht vergeben; RAS, XCP, ACA,
CPER, MCA sind dabei und gehoeren gemessen, ob Renoir sie durchlaeuft).

---

## 1 — Stufen des Moduls

Jede Stufe hat ein Ergebnis am Geraet. Reihenfolge = `amdgpu_device_init`.

| Stufe | Inhalt (Linux) | Ergebnis |
|---|---|---|
| **0** ✅ gebaut | `amdgpu_device_init` bis `amdgpu_discovery_reg_base_init`: BAR5, `amdgpu_device_init_apu_flags`, `amdgpu_discovery_read_binary_from_mem` (C2PMSG_33, RCC_CONFIG_MEMSIZE, VRAM ueber MM_INDEX/MM_DATA), alle Pruefungen von `amdgpu_discovery_init`, die Die-/IP-Schleife; dazu `nbio_v7_0_get_rev_id` + `external_rev_id`, `amdgpu_acpi_vfct_bios` + `check_atom_bios`, Vergleich mit `renoir_ip_offset.h` | Log mit jeder IP-Version und Basis; `priv/gpu_amd_renoir/ip_discovery.bin` und `vbios.bin` |
| **1a** ✅ gebaut, Host ✅ | `amdgpu_device_ip_early_init` bis vor das `early_init` des ersten Blocks nach COMMON: `amdgpu_discovery_set_ip_blocks` ganz (harvest, gfx/mall/vcn info, Familie, nbio/hdp/df/smuio-funcs, alle `set_*_ip_blocks` → zehn Bloecke), `soc15_common_early_init`, `amdgpu_get_bios` (VFCT), `amdgpu_atombios_init` (`amdgpu_atom_parse`, atomfirmware scratch/fb/capability) | Log: Bloecke wie Linux, family/flags/rev, cg/pg, ATOM-Name/-Version, fw caps |
| **1b** ✅ gebaut, Host ✅ | `early_init` von gmc_v9_0, vega10_ih, psp_v12_0 (`psp_v12_0_init_microcode`: asd, ta v2), smu_v12_0 (renoir_ppt), dm (dmcub), gfx_v9_0 (pfp, me, ce, rlc v2.1, mec; keine mec2 auf GC 9.3.0), sdma_v4_0, vcn_v2_0, jpeg_v2_0; davor `amdgpu_device_set_mcbp` | Log: Microcode-Versionen, PSP-Ladeliste (10 ucode ids, fw_size), alle Bloecke `valid` |
| **1c** ✅ gebaut, Host ✅ | `amdgpu_device_init` bis vor `amdgpu_device_ip_init`: Felder und `check_arguments`, `virt_init` (blankes Metall), `get_pcie_info` (Link-Faehigkeiten von GPU und vorgeschalteter Bridge, `drivers/pci`), Timeouts, `set_mcbp`, `ip_early_init`, TMZ, noretry, Atomics, `amdgpu_doorbell_init` (BAR2, `vega10_doorbell_index_init`), `need_post` (BIOS-Scratch 7), `get_clock_info`. Der **Reset beim Start** (`soc15_need_reset_on_init` ist fuer CHIP_RENOIR immer wahr: MODE2 ueber die SMU) wird entschieden und gemeldet, **nicht ausgefuehrt** | Log: PCIe-Rohwerte und Masken, Doorbells, Reset-Entscheid, Takte |
| 1d | Der Reset beim Start: `amdgpu_asic_reset` → `soc15_asic_reset` → `amdgpu_dpm_mode2_reset` → `smu_v12_0_mode2_reset` (`smu_cmn_send_smc_msg_with_param`, GfxDeviceDriverReset 2). Der erste Schreibzugriff, der den Zustand der Karte aendert, waehrend sie den Bildschirm treibt: eigener Geraetetest | Bild bleibt oder nicht; SMU antwortet |
| 2 | `amdgpu_device_ip_init` Phase 1: COMMON (`soc15_common_hw_init`: NBIO-Register, Doorbell-Apertur), IH (`vega10_ih`, Ring im Systemspeicher, MSI) | IRQ-Ring laeuft, ein Interrupt kommt an |
| 3 | GMC (`gmc_v9_0`: GART, GFXHUB/MMHUB v1, VMID 0), PSP 12 (`psp_v12_0`: Ring, ASD + TA laden, Firmware ueber den PSP), SMU 12 (`renoir_ppt`: Postfach, Takte, GFXOFF) | PSP meldet ASD geladen; SMU antwortet; GART-Seite lesbar |
| 4 | GFX 9 (`gfx_v9_0`: RLC, CP pfp/me/ce/mec ueber PSP, KIQ, GFX- und Compute-Ring), SDMA 4 (`sdma_v4_0`) | Ring-Test und IB-Test wie Linux (`amdgpu_ring_test_helper`), SDMA-Selbsttest — **GPU_STACK §9 Stufe 8** |
| 5 | gpudev-Dienst: Kopie und Szenen wie bei Intel; Fences, GPUVM je Kontext | Desktop-Kopie per SDMA |
| 6 | VCN 2.2 + JPEG 2.0 | Video-Engine-Ringtest |
| 7 | native context AMD: Wirtsseite aus virglrenderer `src/drm/amdgpu` (Kontexttyp 2), Gast-Mesa von Alpine (`-Damdgpu-virtio`) | `glxinfo` im Gast nennt radeonsi — **GPU_STACK §9 Stufe 9** |
| 8 | DC (`dm`, DCN 2.1): Anzeige uebernehmen | Bild bleibt, dann Flip |

**Voraussetzungen ausserhalb des Moduls:**

- **AMD-Vi (IOMMU) am IdeaPad.** Native Kontexte verlangen, dass die Karte
  nur freigegebene Seiten erreicht (`gpudev::native_device` fragt
  `vtd::protected`). Am NUC ist das VT-d, am IdeaPad fehlt die AMD-Seite ganz.
  Fuer Stufe 2–4 (DMA der Karte selbst) ist sie schon noetig — vorher kein
  Bus-Mastering.
- **Firmware:** `renoir_{pfp,me,ce,rlc,mec,sdma,vcn,asd,ta,dmcub}.bin` aus
  linux-firmware, als signierte Assets mit dem Modul ausgeliefert (Lizenz:
  `LICENSE.amdgpu`, Weitergabe erlaubt). Welche genau, sagt Stufe 0
  (`external_rev_id` entscheidet Renoir- gegen Green-Sardine-DMCUB; MP0 12.0.1
  wuerde `green_sardine_asd/ta` laden).

---

## 2 — Was der Geraetelauf von Stufe 0 beantwortet hat (2026-10-18)

1. **IP-Versionen** (Discovery v1.1, Tabelle v2, 1124 Bytes, ein Die, 41 IPs):
   GC 9.3.0 · HDP 4.1.2 · SDMA0 4.1.2 · MMHUB 1.5.0 · ATHUB 1.5.0 ·
   NBIO/NBIF 2.5.0 · **MP0 11.0.3** · MP1 12.0.0 · VCN 2.2.0 · DF 3.5.1 ·
   DCE 2.1.0 · OSSSYS 4.3.0 · SMUIO 10.0.2 · THM 10.1.1 · CLK 10.0.2 ·
   UMC 7.6.0 (2 Instanzen) · PCIE 5.1.0 · ISP 2.1.0. Damit `renoir_asd/ta`,
   nicht `green_sardine_*`. Die Entwuerfe in `.follow` stimmen:
   NBIO → `nbio_v7_0`, DF → `df_v1_7`, SMUIO → `smuio_v9_0`.
2. **Basen:** 14 von 15 wie `renoir_ip_offset.h`. MP1 weicht ab, die Tabelle
   meldet fuer MP1 die Basen von MP0; Segment 0 (`0x16000`) ist in beiden
   gleich, und jedes MP1-Register, das Linux benutzt, hat `BASE_IDX 0`.
   Linux nimmt die Discovery-Basen, wir auch.
3. `rev_id` 0, `external_rev_id` **0x91 = RENOIR_A0**, wie die DC-Festlegung
   annimmt. PCI-Revision 0xc1.
4. **VFCT liefert das VBIOS:** 55296 Bytes, ATOM-Kopf bei 0x18a.
5. SDMA 1 Instanz, VCN 1, GC-XCC-Maske 1, UMC 2. `RCC_CONFIG_MEMSIZE` 512 MB.

## 2a — Wie gegen Linux geprueft wird

`tools/wasm/gpu_amd_renoir/hostcheck/check.sh` haelt die Portierung gegen
Linux' eigenen Code, auf `vbios.bin` und `ip_discovery.bin` vom Geraet
(`~/.cache/nopeekos/amd-device/`):

- `oracle/build.sh` baut die Orakel aus dem Graph-Baum: `atom.c` ganz, mit
  duennen Ersatzheadern, und die amdgpu-Funktionen, die `oracle/extract.py`
  zur Bauzeit woertlich herausschneidet (atomfirmware, discovery von
  `reg_base_init` bis `get_vcn_info`). Im Repo liegt kein Linux-Quelltext.
- Die Rust-Dateien des Moduls werden mit `rustc` auf dem Host gebaut;
  `npk_sys` ist dort `hostcheck/npk_sys.rs`, eine Ersatzkarte (Register,
  VRAM ueber MM_INDEX/MM_DATA) und eine Ersatz-ACPI (VFCT).
- Verglichen werden Zeilen: atom 66/66, discovery 91/91. Danach laeuft
  Stufe 1a am Stueck (`stage1a_run.rs`) und druckt das Log des Geraets.

- Drittes Orakel (`main_ucode.c`): die Microcode-Funktionen aus
  amdgpu_ucode.c, amdgpu_psp.c, psp_v12_0.c, amdgpu_gfx.c, amdgpu_rlc.c,
  gfx_v9_0.c, amdgpu_sdma.c, sdma_v4_0.c, amdgpu_vcn.c, amdgpu_dm.c auf den
  bereitgestellten Firmware-Dateien: 45/45 Zeilen (Versionen, Offsets der
  Teil-Images, Hash ueber die RLC-Registerliste, PSP-Ladeliste, fw_size).

**Grenze:** geprueft ist, was die Daten des Geraets durchlaufen. Die
GC-Tabelle des Geraets ist v2.0 — eine Mutation im v1-Zweig faellt nicht
auf, eine im v2-Zweig sofort. Zweige fuer andere Tabellenversionen sind
gegen die Quelle gelesen, nicht ausgefuehrt. `fw_size` summiert
seitengerundete Groessen: ein Fehler in der MEC-Sprungtabelle unter einer
Seite rundet sich weg (gemessen); die genauen Groessen prueft erst Stufe 3,
wenn die PSP laedt.

**Firmware:** `tools/wasm/gpu_amd_renoir/firmware.list` nennt die Dateien
aus linux-firmware; `tools/stage-module.sh` packt sie nach
`release/modules/gpu_amd_renoir.fw/`, das Modul-Manifest fuehrt je Datei eine
Zeile `firmware=<pfad> <groesse> <sha384>`, und `install` legt sie (vor dem
Modul) nach `sys/firmware/<pfad>` — schreibgeschuetzt fuer Module
(`is_trust_critical_path`). `uninstall` nimmt sie mit.

## 3 — Was Stufe 0 bewusst noch nicht tut

- `amdgpu_discovery_read_binary_from_sysmem` (TMR ueber ACPI): nur noetig,
  wenn `RCC_CONFIG_MEMSIZE` 0 oder ~0 meldet; das Modul sagt es dann.
- `amdgpu_device_rreg` jenseits von BAR5 (PCIe-Index/Data ueber NBIO).
- `harvest_ip`, `get_gfx_info`, `get_vcn_info`: Stufe 1.
