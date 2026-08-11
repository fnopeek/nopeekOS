# docs/ — Karte

Vier Ablagen, nach **Verbindlichkeit** getrennt, nicht nach Thema. Wer wissen
will, woran der Code sich halten muss, liest `spec/` — und sonst nichts.

| Ordner | Bedeutung | Lebensdauer |
|---|---|---|
| `spec/` | **Lebende Verträge.** Der Code hält sich daran; Abweichung ist ein Bug. Aus Quelldateien heraus referenziert. | wird gepflegt |
| `plan/` | **Offene Arbeitspapiere.** Vorschlag, Analyse, Reihenfolge — noch nicht (ganz) gebaut. | bis erledigt |
| `notes/` | **Dev-How-tos.** Handgriffe an der Werkbank, kein Vertrag. | bei Bedarf |
| `archive/` | **Erledigt oder überholt.** Nur Herleitung und Geschichte, nie Referenz. | eingefroren |

Nicht hier: der laufende Stand. Der steht im Memory (`memory/MEMORY.md`) und im
git log. Diese Ordner sind für das, was länger gilt als eine Session.

---

## spec/ — lebende Verträge

| Datei | Worum es geht |
|---|---|
| [`BROWSER.md`](spec/BROWSER.md) | `beak`, der eigene Browser: Architektur, Stufen, Grenzen |
| [`CONFORMANCE.md`](spec/CONFORMANCE.md) | beaks Strichliste gegen die offiziellen Testsuiten. **Die WPT-Zahl steht hier und nirgends sonst.** |
| [`WIDGET_VOCAB.md`](spec/WIDGET_VOCAB.md) | `nopeek_widgets`-Vokabular — die Referenz zum Bauen von Apps |
| [`UI_REFRESH.md`](spec/UI_REFRESH.md) | Design-Kontrakt: Tokens, Farben, Abstände. Apps lesen dieses File, nicht das Mockup |
| [`PANEL.md`](spec/PANEL.md) | Panel-Primitiv (Bar + Dock als WASM am Bildschirmrand) |
| [`WIFI_CLASS_ABI.md`](spec/WIFI_CLASS_ABI.md) | Vertrag zwischen Applet, `wifid` und den Vendor-Treibern |
| [`AX200_FUNC_MAP.md`](spec/AX200_FUNC_MAP.md) | Was Linux' `iwlwifi-mvm` kann und was wir davon nachgebaut haben |

## plan/ — offen

Jedes Papier trägt oben einen datierten Status-Kopf, weil die Lage sich seit
dem Schreiben bewegt hat. Erst den lesen.

| Datei | Stand |
|---|---|
| [`MICROKERNEL_REFACTOR.md`](plan/MICROKERNEL_REFACTOR.md) | teilweise: `aml`, `wifi`, `audio_hda` sind WASM-Driver; NVMe/xHCI/GPU/NIC nicht |
| [`SCHEDULER_FIBERS.md`](plan/SCHEDULER_FIBERS.md) | grösstenteils gebaut; offen ist der Multi-Core-Fan-out |
| [`NPKFS_PERF_SAFETY.md`](plan/NPKFS_PERF_SAFETY.md) | auto-gc + Write-Barriere + fsck sind drin; offen ist Commit-Batching |
| [`NOTES_framebuffer_netperf.md`](plan/NOTES_framebuffer_netperf.md) | Hypothese, nie nachgemessen — vor dem Umbau reproduzieren |

## notes/

| Datei | Worum es geht |
|---|---|
| [`RECORDING.md`](notes/RECORDING.md) | Demo-Video vom QEMU-Gast aufnehmen, ohne dass die Schrift matscht |

## archive/

Gebaut oder abgelöst: [`CHANGELOG_2026.md`](archive/CHANGELOG_2026.md) (der
Session-Verlauf, der bis August in `CLAUDE.md` stand),
[`PHASE10_WIDGETS.md`](archive/PHASE10_WIDGETS.md),
[`PHASE12_MICROVM.md`](archive/PHASE12_MICROVM.md),
[`PHASE12_DISPLAY_BRIDGE.md`](archive/PHASE12_DISPLAY_BRIDGE.md),
[`NPKFS_V2.md`](archive/NPKFS_V2.md) (wir sind bei v3),
[`DOCK.md`](archive/DOCK.md) (→ `spec/PANEL.md`),
[`WIFI_AX200.md`](archive/WIFI_AX200.md),
[`fix_uefi_boot.md`](archive/fix_uefi_boot.md).

---

## Regeln

1. **Ein Dokument hat genau einen Ort.** Steht etwas in `spec/`, wird es nicht
   in README oder CLAUDE.md nacherzählt — dort steht der Zeiger.
2. **Zahlen leben an einer Stelle.** Versionen im Cargo.toml, die WPT-Zahl in
   `spec/CONFORMANCE.md`, der Verlauf im git log. Was doppelt steht, driftet.
3. **Fertig heisst umziehen.** Ein Plan, der gebaut ist, wandert mit einem
   datierten Kopf nach `archive/` — er wird nicht stillschweigend liegen
   gelassen, wo ihn jemand für gültig hält.
