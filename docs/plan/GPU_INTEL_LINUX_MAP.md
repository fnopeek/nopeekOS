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
tools/linux-callgraph.py      Graph aus -fcallgraph-info, Kopf der Datei
                              sagt, wie i915 dafuer gebaut wird
tools/gpu_intel_tgl.follow    jede Plattformweiche (indirekter Aufruf),
                              mit der Quellzeile, die TGL dorthin schickt
```

Lauf (Wurzeln = alles, was ohne Aufrufer im Treiber beginnt):

```
# Graph liegt fertig unter ~/.cache/nopeekos/i915-callgraph/linux-6.18.55
python3 tools/linux-callgraph.py --dir ~/.cache/nopeekos/i915-callgraph/linux-6.18.55 reach \
    i915_driver_probe gen11_irq_handler heartbeat retire_work_handler \
    execlists_timeslice execlists_preempt \
    --prune 'intel_display_|^intel_power_domains|^intel_dmc|^intel_bios|^intel_opregion|^i915_driver_register$|guc|huc|gsc|pxp|perf|pmu' \
    --prune-file '^(display/|\./\./display|gt/uc/|\./\./gt/uc/|pxp/|\./\./pxp|gvt/|i915_perf|i915_pmu|i915_debugfs|i915_hwmon|i915_gpu_error|soc/|gt/.*debugfs|i915_gsc|gt/intel_gsc|i915_sysfs|gt/sysfs|gt/intel_gt_sysfs|i915_hdcp|i915_ioc32|gt/selftest|selftests/|i915_trace)' \
    --follow-file tools/gpu_intel_tgl.follow --extern
```

**Indirekte Aufrufe folgt das Werkzeug nie von selbst.** Dort waehlt i915
die Plattform (`engine->emit_flush`, `uncore->funcs.mmio_readl`, …); welcher
Kandidat auf TGL laeuft, ist in der Quelle gelesen und steht mit Fundstelle
in der `.follow`-Datei. Die verbleibenden ungefolgten Aufrufe sind entweder
als „nicht auf TGL" belegt oder Rueckrufe der Linux-Infrastruktur
(`i915_active`, `i915_sw_fence`).

Ausgeschlossen und warum:

| Weg | Grund |
|---|---|
| `display/` | Stufe 1 uebernimmt den Modus der Firmware; nur Ebene + Flip kommen dazu (§4) |
| `gt/uc/`, GuC/HuC/GSC | **TGL laedt weder GuC noch HuC** — `uc_expand_default_options`: `IS_TIGERLAKE → enable_guc = 0` |
| PXP, Perf/OA, PMU, hwmon, debugfs, sysfs, Fehlerabzug | keine Funktion fuer den Betrieb; was davon zaehlt, geht in `npk_driver_report` |

---

## 1 — Was TGL laut Linux IST

Aus `i915_pci.c` (`tgl_info` ← `GEN12_FEATURES` ← … ← `GEN7_FEATURES`):

| | |
|---|---|
| Engines | **RCS0, BCS0, VCS0, VCS2, VECS0 — keine CCS.** Compute laeuft ueber die GPGPU-Pipeline der RCS (`GPU_STACK.md` §7.1 sagt „RCS/CCS" — falsch fuer TGL). |
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

**2275 Funktionen in 153 Dateien** auf TGL erreichbar (Stand des Graphen
oben; Header-Inlines eingeschlossen).

| Klasse | fn | Dateien | Umgang |
|---|---|---|---|
| **A — programmiert die Hardware** | **1221** | 56 | 1:1 portieren |
| **B — Linux-Verwaltung** | 993 | 83 | Gegenstueck mit derselben Semantik, nicht abschreiben (§3) |
| **N — nicht TGL** | 61 | 14 | entfaellt, Grund je Datei unten |

**Vorbehalt:** direkte Aufrufe hinter `if (IS_VALLEYVIEW(…))` usw. zaehlen
noch mit — der Graph kennt keine Bedingungen. Klasse A ist deshalb eine
Obergrenze; die Feinklassifizierung je Funktion (TGL / nicht TGL) ist der
naechste Schritt (§5).

### A — Hardware, Datei fuer Datei

| Datei | fn | Inhalt |
|---|---|---|
| `gt/intel_workarounds.c` | 104 | GT-, Engine-, Kontext-Workarounds **und die Registerpositivliste** (F4 aus `GPU_STACK.md`) |
| `gt/intel_execlists_submission.c` | 99 | ELSQ, CSB-Auswertung, Preemption, Timeslice, Reset-Pfade |
| `intel_uncore.c` + `.h` | 168 | MMIO mit Forcewake, Domaenentabellen, Shadow-Register |
| `gt/intel_rps.c` + `.h` | 84 | Taktsteuerung (RPS) — fuer Watt und Leistung, nicht fuer das erste Bild |
| `gt/intel_lrc.c` + `.h` | 55 | Kontextbild (LRC), Register-Offsets je Engine, indirekter Kontext-BB |
| `gt/intel_engine_cs.c` + `intel_engine.h` | 64 | Engine-Init, MMIO-Basen, Status-Page, Idle-Pruefung |
| `gt/intel_reset.c` | 49 | Engine- und GT-Reset — Pflicht, sobald ein Gast abgibt |
| `gt/intel_gtt.c/.h`, `intel_ggtt.c`, `intel_ppgtt.c`, `gen8_ppgtt.c` | 148 | GGTT, PPGTT vier Ebenen, PTE-Kodierung `gen12_pte_encode` |
| `i915_irq.c`, `gt/intel_gt_irq.c`, `intel_gt_pm_irq.c` | 65 | `gen11_irq_handler`, GT-Interrupts je Engine |
| `gt/intel_gt.c` + `.h` | 49 | `intel_gt_init_hw`, Sanitize, Probe |
| `gt/intel_rc6.c` | 26 | RC6 — Leerlaufstrom |
| `gt/intel_ggtt_fencing.c` | 21 | Fence-Register (Detiling ueber die Apertur) |
| `gt/intel_sseu.c/.h` | 23 | Slice/Subslice-Topologie, `R_PWR_CLK_STATE` |
| `gt/gen8_engine_cs.c/.h` | 27 | **die Befehlsstroeme**: Flush, Breadcrumb, BB-Start |
| `gt/intel_timeline.c/.h` | 24 | HWSP-Seqnos — die Fences der Hardware |
| `gt/intel_gt_mcr.c` | 16 | Multicast/Replicated-Register (Steering) |
| `gt/intel_gt_pm.c/.h`, `intel_engine_pm.c/.h` | 65 | Park/Unpark — Mischung aus A und B |
| `gt/intel_breadcrumbs.c/.h` | 18 | Warten auf Seqnos per Interrupt |
| `gt/intel_mocs.c` | 13 | MOCS-Tabellen (Cache-Steuerung je Zugriff) |
| `gt/intel_ring.c/.h` | 20 | Ringpuffer |
| Rest (`clock_utils`, `heartbeat`, `wopcm`, `llc`, `tlb`, `renderstate`, `pcode`, `step`, `device_info`, `clock_gating`) | 77 | |

### N — nicht TGL

`vlv_iosf_sb.c` 11, `vlv_suspend.c` 2 (Valleyview), `i915_vgpu.c` 9 (GVT-Gast-
Erkennung; wir laufen nackt), `i915_cmd_parser.c` 8 (auf Gen12 gibt es keinen
Parser), `gt/intel_region_lmem.c` 7, `gem/i915_gem_lmem.c` 3, TTM 6 (kein
LMEM), `gt/intel_migrate.c` 6 (LMEM-Kopien), `gt/gen6_ppgtt.c` 5,
`gt/intel_ggtt_gmch.c` 4 (Gen5 und aelter), `gt/intel_gt_ccs_mode.c` 1 (keine
CCS), `gt/intel_sa_media.c` 1 (MTL).

---

## 3 — Klasse B: was Linux verwaltet und wir selbst

Groesste Posten: `i915_request` 124, `i915_vma` 128 + `vma_resource` 15,
GEM-Objekte/Seiten/shmem/stolen ~200, `intel_wakeref` + Runtime-PM 97,
`i915_active` 55, `intel_context` 63, Scheduler 22, `sw_fence` 22,
`syncmap` 16.

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

1. **Feinklassifizierung A**: jede Funktion TGL / nicht TGL, an den
   Plattformbedingungen gelesen (`GRAPHICS_VER`, `IS_*`, `HAS_*`). Ergebnis:
   die exakte Portierliste.
2. **Reihenfolge**: die Liste in Aufrufreihenfolge ab
   `i915_driver_probe` → `intel_gt_init_hw` → `intel_engines_init` → erste
   Abgabe auf BCS. Das ist Stufe 1.
3. Erst danach Code (`tools/wasm/gpu_intel/`), und `linux-coverage.py`
   bekommt den Eintrag `gpu_intel` gegen genau diese Liste.

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
