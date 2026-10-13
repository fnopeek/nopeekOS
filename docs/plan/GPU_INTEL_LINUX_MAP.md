# gpu_intel gegen Linux — die Karte

**Stand:** 2026-10-13 · noch kein Modul · Referenz **Linux 6.18.55**, i915
**Geraet:** NUC, `8086:9A78` Tiger Lake-LP GT2, Gen12.1
**Papier darueber:** `GPU_STACK.md` (Stufen §9) — diese Karte ist die
Grundlage fuer Stufe 1 und 2.

Nicht nach Eindruck, sondern aus dem **Aufrufgraphen, den GCC beim
Kompilieren von i915 sieht**. Dieselbe Regel wie beim RTL8822CE
(`WIFI_RTL8822CE_LINUX_MAP.md`): jede Linux-Funktion auf dem Weg wird
vollstaendig portiert, auch die „vermutlich no-op".

---

## 0 — Werkzeug und Wiederholbarkeit

```
tools/linux-callgraph.py      Graph aus -fcallgraph-info; der Kopf der
                              Datei sagt, wie i915 dafuer gebaut wird
tools/i915-tgl-specialize.py  legt eine KOPIE von i915 auf TGL GT2 fest:
                              GRAPHICS_VER, IS_PLATFORM, INTEL_INFO werden
                              Konstanten, GCC wirft fremde Zweige weg
tools/gpu_intel_tgl.follow    jede Plattformweiche (indirekter Aufruf),
                              mit der Quellzeile, die TGL dorthin schickt
tools/gpu_intel_tgl.classes   A / B / N je Datei
GPU_INTEL_TGL_FUNCTIONS.txt   das Ergebnis: jede Funktion mit Klasse,
                              Tiefe und Fundstelle — der Auftragszettel
```

Fertige Graphen liegen unter `~/.cache/nopeekos/i915-callgraph/`
(`linux-6.18.55` allgemein, `linux-6.18.55-tgl` festgelegt). Lauf:

```
python3 tools/linux-callgraph.py --dir ~/.cache/nopeekos/i915-callgraph/linux-6.18.55-tgl reach \
    i915_driver_probe gen11_irq_handler heartbeat retire_work_handler \
    execlists_timeslice execlists_preempt \
    --prune 'intel_display_|^intel_power_domains|^intel_dmc|^intel_bios|^intel_opregion|^i915_driver_register$|guc|huc|gsc|pxp|perf|pmu' \
    --prune-file '^(display/|\./\./display|gt/uc/|\./\./gt/uc/|pxp/|\./\./pxp|gvt/|i915_perf|i915_pmu|i915_debugfs|i915_hwmon|i915_gpu_error|soc/|gt/.*debugfs|i915_gsc|gt/intel_gsc|i915_sysfs|gt/sysfs|gt/intel_gt_sysfs|i915_hdcp|i915_ioc32|gt/selftest|selftests/|i915_trace)' \
    --follow-file tools/gpu_intel_tgl.follow --classes tools/gpu_intel_tgl.classes [--list]
```

Zwei Stufen der Plattformwahl, beide maschinell:

1. **Direkte Zweige** (`if (GRAPHICS_VER(i915) >= 12)`, `IS_TIGERLAKE`,
   `HAS_*` aus der Geraeteinfo) wirft der Compiler weg, weil die Kopie sie
   als Konstanten sieht. Nicht festgelegt sind Stepping-Abfragen (bleiben
   im Graphen, also eher zu viel als zu wenig) und Werte, die erst zur
   Laufzeit aus der Hardware kommen (Engine-Maske nach Fuses, `CCS_MASK`):
   eine Funktion wie `xehp_enable_ccs_engines` steht deshalb drin und
   kehrt auf TGL sofort zurueck.
2. **Indirekte Aufrufe** folgt das Werkzeug nie von selbst. Welcher
   Kandidat auf TGL laeuft, ist gelesen und steht mit Fundstelle in der
   `.follow`-Datei. Ungefolgt bleiben nur als „nicht auf TGL" belegte
   Zweige und Rueckrufe der Linux-Infrastruktur (`i915_active`,
   `i915_sw_fence`).

Ausgeschlossen und warum:

| Weg | Grund |
|---|---|
| `display/` | Stufe 1 uebernimmt den Modus der Firmware; nur Ebene + Flip kommen dazu (§4) |
| `gt/uc/`, GuC/HuC/GSC | **TGL laedt weder GuC noch HuC** — `uc_expand_default_options`: `IS_TIGERLAKE → enable_guc = 0` |
| PXP, Perf/OA, PMU, hwmon, debugfs, sysfs, Fehlerabzug | keine Funktion fuer den Betrieb; was davon zaehlt, geht in `npk_driver_report` |

---

## 1 — Was TGL laut Linux IST

Aus `i915_pci.c` (`tgl_info` ← `GEN12_FEATURES` ← … ← `GEN7_FEATURES`),
`9A78` steht in `INTEL_TGL_GT2_IDS` → Unterplattform UY:

| | |
|---|---|
| Engines | **RCS0, BCS0, VCS0, VCS2, VECS0 — keine CCS.** Compute laeuft ueber die GPGPU-Pipeline der RCS. |
| Abgabe | Execlists mit **ELSQ** (`has_logical_ring_elsq`), CSB mit `GEN11_CSB_ENTRIES` |
| Adressraum | **PPGTT voll, 48 Bit** → vier Ebenen (`i915_vm_is_4lvl`) |
| Speicher | kein LMEM; Regionen `SMEM` + `STOLEN_SMEM` |
| PAT | `TGL_CACHELEVEL`, `max_pat_index = 3`, `has_global_mocs` |
| Forcewake | Domaenen GT, RENDER, je VDBOX/VEBOX; `uncore_get_fallback`, Tabellen `__gen12_fw_ranges` + `gen12_shadowed_regs` |
| IRQ | `gen11_irq_handler` (IP < 12.10) |
| Reset | `gen8_reset_engines` |
| Clock-Gating | `nop` — bei Gen12 tragen das die Workarounds |

---

## 2 — Der Umfang, ausgezaehlt

| Graph | Funktionen | Dateien |
|---|---|---|
| allgemein (alle Plattformen) | 2275 | 153 |
| **auf TGL festgelegt** | **1925** | 145 |

Die Festlegung allein nimmt 350 Funktionen heraus — `intel_workarounds.c`
104 → 47, `intel_rps.c` 72 → 41, `i915_irq.c` 42 → 17.

| Klasse | fn | Dateien | Umgang |
|---|---|---|---|
| **A — programmiert die Hardware** | **958** | 54 | 1:1 portieren |
| **B — Linux-Verwaltung** | 937 | 81 | Gegenstueck mit derselben Semantik (§3) |
| **N — nicht TGL** | 30 | 10 | entfaellt, Grund in der `.classes`-Datei |

**Stufe 1 ist der ganze Init-Weg aus A, nicht „nur BCS".** Linux bringt
die GT nie mit einer einzelnen Engine hoch: `intel_gt_init` legt alle
Engines, Workarounds, MOCS, RC6 und RPS in einem Gang an, und die
Workaround-Listen setzen Register, die alle Engines betreffen. Eine BCS
allein waere genau die Auswahl einzelner Schritte, die beim RTL8852BE den
Chip nie in einen stimmigen Gesamtzustand brachte.

### A — Hardware, Datei fuer Datei (TGL-Graph)

| Datei | fn | Inhalt |
|---|---|---|
| `gt/intel_execlists_submission.c` | 96 | ELSQ, CSB-Auswertung, Preemption, Timeslice, Reset-Pfade |
| `intel_uncore.c` + `.h` | 127 | MMIO mit Forcewake, Domaenentabellen, Shadow-Register |
| `gt/intel_engine_cs.c` + `intel_engine.h` | 61 | Engine-Init, MMIO-Basen, Status-Page, Idle-Pruefung |
| `gt/intel_workarounds.c` + `.h` | 49 | GT-, Engine-, Kontext-Workarounds **und die Registerpositivliste** (F4 aus `GPU_STACK.md`) |
| `gt/intel_reset.c` | 46 | Engine- und GT-Reset — Pflicht, sobald ein Gast abgibt |
| `gt/intel_lrc.c` + `.h` | 45 | Kontextbild (LRC), Register-Offsets je Engine, indirekter Kontext-BB |
| `gt/intel_rps.c` + `.h` | 52 | Taktsteuerung |
| `gt/intel_gtt.c/.h`, `intel_ggtt.c`, `intel_ppgtt.c`, `gen8_ppgtt.c` | 128 | GGTT, PPGTT vier Ebenen, PTE-Kodierung `gen12_pte_encode` |
| `gt/intel_gt.c` + `.h` | 43 | `intel_gt_init_hw`, Sanitize, Probe |
| `gt/intel_gt_pm.c/.h`, `intel_engine_pm.c/.h` | 62 | Park/Unpark — Mischung aus A und B |
| `i915_irq.c`, `gt/intel_gt_irq.c/.h`, `intel_gt_pm_irq.c` | 35 | `gen11_irq_handler`, GT-Interrupts je Engine |
| `gt/intel_timeline.c/.h` | 24 | HWSP-Seqnos — die Fences der Hardware |
| `gt/gen8_engine_cs.c/.h` | 24 | **die Befehlsstroeme**: Flush, Breadcrumb, BB-Start |
| `gt/intel_ring.c/.h` | 20 | Ringpuffer |
| `gt/intel_breadcrumbs.c/.h` | 18 | Warten auf Seqnos per Interrupt |
| `gt/intel_rc6.c` | 16 | RC6 — Leerlaufstrom |
| `gt/intel_sseu.c/.h` | 14 | Slice/Subslice-Topologie, `R_PWR_CLK_STATE` |
| `gt/intel_gt_mcr.c` | 13 | Multicast/Replicated-Register (Steering) |
| `gt/intel_ggtt_fencing.c` | 12 | Fence-Register |
| Rest (`heartbeat`, `mocs`, `clock_utils`, `device_info`, `llc`, `tlb`, `pcode`, `wopcm`, `renderstate`, `clock_gating`, `step`) | 71 | |

## 3 — Klasse B: was Linux verwaltet und wir selbst

Groesste Posten (TGL-Graph): GEM-Objekte/Seiten/shmem/stolen 210,
`i915_request` 124, `i915_vma` 115 + `vma_resource` 15, `intel_wakeref` +
Runtime-PM 91, `intel_context` 65, `i915_active` 54, `sw_fence` 25,
Scheduler 21, `syncmap` 16, Rest (Treiberkern, Hilfen) 201.

Hier liegt die eine echte Abweichung von „1:1": diese Schicht verwaltet
Linux-Objekte (Referenzzaehler, Sperren, Speicherdruck), keine Register. Sie
wird **nicht** abgeschrieben, sondern auf `gpudev` abgebildet — Puffer
gehoeren dem Kernel (`GPU_STACK.md` §3.3). **Was 1:1 bleiben muss, ist ihre
Wirkung auf die Hardware:** in welcher Reihenfolge ein Request seine Befehle
in den Ring schreibt, wann eine Seqno als erreicht gilt, wann ein Kontext
gepinnt sein muss, solange die Hardware ihn liest. Jede Funktion aus B, die
ein Register oder einen Befehlsstrom beruehrt, wandert bei der
Feinklassifizierung nach A.

---

## 4 — Noch nicht im Graphen

- **Display fuer Stufe 1:** Ebene + Flip + vblank auf TGL
  (`skl_universal_plane.c`, `PLANE_SURF`, vblank-Interrupt). Eigene,
  kleine Wurzelmenge — der Modus bleibt der der Firmware.
- **uAPI fuer native context (Stufe 7):** `i915_gem_execbuffer.c`,
  `i915_getparam.c`, `i915_query.c`, `i915_gem_create.c`, `i915_gem_context.c`
  — eigene Wurzeln, wenn Stufe 7 ansteht.

---

## 5 — Naechste Schritte

1. **Reihenfolge**: die A-Liste in Aufrufreihenfolge ab `i915_driver_probe`
   (die `--list`-Ausgabe ist breadth-first; fuer die Portierung gilt die
   Reihenfolge der Aufrufe IN jeder Funktion, gelesen in der Quelle).
2. **Die Grenze A/B je Funktion**: Funktionen aus B, die ein Register oder
   einen Befehlsstrom beruehren, wandern nach A; Funktionen aus A, die nur
   Linux-Zustand halten (Referenzen, Sperren), werden auf gpudev
   abgebildet. Erst danach ist die Liste der Auftragszettel.
3. Erst danach Code (`tools/wasm/gpu_intel/`), und `linux-coverage.py`
   bekommt den Eintrag `gpu_intel` gegen `GPU_INTEL_TGL_FUNCTIONS.txt`.

---

## 6 — Befunde nebenbei

- **`intel_xe.rs` ist kein Port.** Es nennt fast keinen Linux-Funktionsnamen
  (`grep` gegen die Namen der Karte). Es hat GGTT, Forcewake, Execlists und
  LRC fuer den BCS frei geschrieben — `gpu_intel` uebernimmt das Wissen
  nicht, sondern portiert aus Linux. Der Vergleich der beiden bei Stufe 1
  sagt, was `intel_xe.rs` ausgelassen hat.
- **ADL-N braucht GuC.** `intel_xe.rs` kennt ADL-N; laut
  `uc_expand_default_options` bekommt jede Plattform nach TGL/RKL/ADL-S
  GuC-Submission. Ein `gpu_intel` fuer ADL-N ist damit ein anderer Weg
  (GuC-Firmware, CT-Kanal) — offen, ob es dafuer ein Geraet gibt.
