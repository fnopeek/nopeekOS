# Starre Regeln — Bestandsaufnahme 2026-10-16

Florian, 2026-10-16: „es wird an anderen stellen noch mehr solche sachen
geben ich möchte alle wissen damit wir sie dann sep. angehen können … und
nicht nur workaround rundherum bauen".

Anlass: zwei Regeln, die am Gerät echte Arbeit kaputt machten — feste 2048
Seitentabellen je Adressraum (gpu_intel_tgl 0.19.0) und „jede DMA-Seite
unter 4 GB" (Kernel 0.615.13, `npk_dma_set_mask`). Gesucht wurde danach mit
drei Durchgängen über GPU-Modul, Kernel-Unterbau und microVM, jeweils gegen
das Original (Linux 6.18.55 i915, KVM/QEMU/crosvm, virglrenderer, Mesa
26.2.4). Jede Zeile nennt, was das Original tut; die Lösung ist dessen Weg,
nicht ein Weg um die Regel herum.

**Lesart:** geordnet nach *Wurzel*, weil die richtige Lösung meist eine je
Wurzel ist und viele Zeilen auf einmal erledigt. Innerhalb einer Wurzel nach
Wahrscheinlichkeit, mit der es trifft (LibreWolf in der microVM, viele Tabs,
4K-Video, WebGL, NUC mit 16 GB). Aufwand S/M/L. Wo nicht anders vermerkt, ist
die Stelle im Code gelesen; *(v)* heißt: Wirkung vermutet, nicht gemessen.
Stellen ohne Pfadpräfix: `tools/wasm/gpu_intel_tgl/src/` (Modul) bzw.
`kernel/src/` (Kernel).

## 0. Heute schon behoben

| Was | Stand |
|---|---|
| Seitentabellen: feste 2048 Plätze, halber Baum nach Fehlschlag | Modul 0.19.0 — Linux-Baum, `i915_vm_alloc_pt_stash` vor dem Walk |
| DMA nur unter 4 GB | Kernel 0.615.13 / Modul 0.19.1 — DMA-Maske je Gerät |
| Screenshot: 33-MB-Puffer auf dem Kernel-Heap | Kernel 0.615.11 — zeilenweise in die App |
| `get_info()` nahm `CONSOLE` unter `with_fb` (Selbstverklemmung) | Kernel 0.615.12 |
| 2.1, 2.2, 2.3, 2.7, 2.8: Clients, Adressräume, GEM-Kontexte, Bindungen, VM-Ids fest | Modul 0.22.0 — wachsende Tabellen; jede VM hält ihre VMAs und `drm_mm`-Knoten, gibt ihren Baum beim letzten Put zurück (`gen8_ppgtt_cleanup`). Am Gerät gefunden: der neunte `VM_CREATE` (Firefox' Video-Dekoder) scheiterte, Mesa gab danach doppelt frei |
| Rechtsklick nahm die Compositor-Sperre zweimal | Kernel 0.615.12 |
| `CTX_DESTROY` verwarf seine Ressourcen ohne Freigabe (GEM-Leck) | Kernel 0.615.14 — Ressourcen gehören dem Gerät (virglrenderer) |
| `MAX_RESOURCES` 256 für HOST3D | Kernel 0.615.14 — keine Zählgrenze |
| Kontext-/VM-Nummern zählten nur hoch, ≤ 8 | Modul 0.19.2 — `xa_alloc` ALLOC1 |
| „Overlap" durch ein Objekt mit neuer Adresse | Modul 0.19.2 — `i915_gem_evict_for_node` |

## Stand 2026-10-17 (am Code nachgeprüft)

**Erledigt:** 1.1, 1.6, 1.8 (Modul 0.20.0: zurückstellen statt ablehnen) ·
2.1–2.3, 2.7, 2.8 · 3.5 · 5.3, 5.4 · 6.4 · 7.9 · 8.1–8.3, 8.5 · 9.6.
**Teilweise:** 1.2–1.5 (wartet jetzt, die Zahlen 16/128/8/8 stehen noch) ·
1.7 (2048-Objekte weg, `CCMD_MAX` 256 KB steht) · 4.1 (`npk_dma_free` da,
Deckel 16384/128 MB jetzt Live-Grenzen) · 4.2 (Balloon mit `F_REPORTING`;
kein `DEFLATE_ON_OOM`, Scudo `-1`, Startfenster 256 MB nie zurück) · 4.8 ·
5.10 · 7.1 (BAR2 wandert schon, RAM noch < 3 GB) · 8.6 · 8.9 · 9.1.
**Kernel 0.623.0:** 6.1 und der RAM-Teil von 7.1 (Gast-RAM = halber
Wirts-RAM, über 4 GiB geteilt). 1.9/1.10 brauchen zuerst Abschlüsse je
`seq` im Kernel (`CPU_COST.md` G9).
**Offen:** alles andere, insbesondere 1.9/1.10, 2.4–2.6, 2.9, W3, 6.2, 6.3,
6.5–6.10, 7.2–7.8, 7.11. CPU-Kosten stehen getrennt in `CPU_COST.md`.

## W1. Ablehnen, wo das Original wartet — **die Wurzel der Hänger**

**Der Befund, der alle Zeilen hier ernst macht:** Mesa im Gast liest die
Antwort auf ein execbuffer nie (`intel/dev/virtio/i915_virtio_ccmd_execbuf.c`
gibt den Status von `vdrm_execbuf` zurück, `rsp->ret` wird nicht gelesen).
Die Annahme im Kopf von `i915_gem_execbuffer.rs` („Mesa wiederholt EAGAIN")
ist falsch. **Jede Ablehnung ist ein still verworfener Batch, dessen Zaun
trotzdem signalisiert** — Mesas Zustand läuft auseinander, der nächste
Batch hängt (rcs0 auf PIPE_CONTROL, `IPEHR 0x7a000004`, dreimal gesehen).
Linux blockiert an diesen Stellen den Aufrufer. Richtig ist: **das Modul hält
den Auftrag zurück, bis die Mittel da sind, und lehnt nie ab**; der Zaun des
Gastes bleibt offen, bis der Batch wirklich lief.

| # | Stelle | Regel | Folge | Original / Lösung | Aufw. |
|---|---|---|---|---|---|
| 1.1 | `i915_gem_execbuffer.rs` gesamt | jede Mittelknappheit = Ablehnung | stiller Verlust, Hänger | blockieren (`eb_throttle`, `evict_for_node` wartet, `i915_sw_fence`); im Modul: Auftrag zurückstellen | M–L |
| 1.2 | `intel_timeline.rs:11`, `execbuffer.rs:298` | 16 Requests je Zeitleiste, Ring < 4 KB frei | „ring or request table full" | `tl->requests` ist eine Liste; auf Ringplatz warten | S |
| 1.3 | `i915_request.rs:20,23` | 128 Requests insgesamt (8 für das Modul) | „no request free" | Slab ohne Deckel | M |
| 1.4 | `i915_gem_context.rs:154` | 8 Zäune je Reservierung | „reservation full" | `dma_resv_reserve_fences` wächst | S |
| 1.5 | `i915_request.rs:28` | 8 fremde Zeitleisten je Request | „too many fences" | `i915_sw_fence` beliebig | S |
| 1.6 | `execbuffer.rs:261-289` | belegtes Objekt im Weg → EAGAIN | Verlust | entbinden und warten | M |
| 1.7 | `execbuffer.rs:154` | > 2048 Objekte, ccmd > 256 KB | EINVAL bzw. ganzes SUBMIT verworfen | nur durch Speicher begrenzt | S–M |
| 1.8 | `execbuffer.rs:81` | `context_id > 8` | ganzes SUBMIT verworfen | nur bei V2 nötig (Ringnummer aus der Id) | S |
| 1.9 | `virtio_ccmd.rs:304-320` | SET_DOMAIN auf belegtes Objekt → EAGAIN | Gast dreht virtio-Runden | `i915_gem_object_wait` (verzögerte Antwort) | M |
| 1.10 | `virtio_ccmd.rs:293` | SET_TILING belegt → EBUSY | Fehlschlag *(v selten)* | warten | S |

## W2. Feste Tabellen im GPU-Modul

Grund im Code: „das Modul hat keinen Heap, der freigibt". **Den hat es seit
0.19.0** (talc aus dem SDK). Linux nimmt xarray/idr/slab/`drm_mm`.

| # | Stelle | Regel | Folge | Lösung | Aufw. |
|---|---|---|---|---|---|
| 2.1 | `i915_gem_context.rs:24` | 8 Clients für den ganzen Rechner | Gastprozess fällt auf llvmpipe *(v: Firefox-Prozesse + npkwm + Proben reichen)* | wachsende Tabelle | S–M |
| 2.2 | `i915_gem_context.rs:25` | 8 Adressräume = Clientzahl | VM_CREATE ENOMEM | wachsend | S |
| 2.3 | `i915_gem_context.rs:26` | 64 GEM-Kontexte gesamt | WebGL nicht verfügbar bei vielen Tabs | wachsend | S–M |
| 2.4 | `intel_context.rs:16` | 128 Hardware-Kontexte, Pool gibt nie frei | ENOSPC „hardware context" | freigeben mit dem GEM-Kontext, Pool nur als Cache | M |
| 2.5 | `drm_mm.rs:17`, `i915_vma.rs:110` | GGTT 512 Knoten, Pins für immer (3 je HW-Kontext) | ENOSPC beim Pin *(v ~170 Kontexte)* | wachsender `drm_mm`, Eviction | M |
| 2.6 | `i915_gem_context.rs:27` | 8192 Objekte gesamt; Kernel prüft `object_attach` nicht | Attach scheitert still → „unknown object" | wachsend; Kernel wartet auf das Ergebnis | M |
| 2.7 | `i915_gem_context.rs:134` | 2 Bindungen je Objekt | ENOSPC *(v: geteiltes Objekt in > 2 Adressräumen)* | Liste von VMAs | S |
| 2.8 | `i915_gem_context.rs:34` | 16 VM-Ids je Client | ENOMEM *(v selten)* | wachsend | S |
| 2.9 | `microvm/devices/virtio_gpu_ctx.rs:36-41` | 64 Kontexte, 64 Ringe, 1 MB shmem | *(v selten)* | ableiten | S |

## W3. Zusammenhängender Speicher, wo Seitenlisten reichen

Heap, DMA, Bildpuffer, Fiber-Stacks, das Gast-Startfenster und 2-MB-Blöcke
konkurrieren um dieselben langen Läufe; die Reserven zählen aber Rahmen,
nicht Läufe. Fragmentierung wird so zur Panik oder zum schwarzen Fenster.
Linux bildet große Kernelpuffer virtuell ab (vmalloc), Geräte nehmen
Scatterlisten.

| # | Stelle | Regel | Folge | Lösung | Aufw. |
|---|---|---|---|---|---|
| 3.1 | `mm/heap.rs:305-331` | Heap wächst nur um einen zusammenhängenden Lauf ≥ 64 MB, gibt nie zurück | **misslungenes Wachsen = Panik = Halt**, für jedes `Vec`/`Box` | eigener virtueller Bereich aus verstreuten Rahmen, beliebige Schritte, leere Regionen zurück | M |
| 3.2 | `microvm/cpu/vmx/ept.rs:247`, `svm/npt.rs:270` | Gast nimmt erst 2-MB-Blöcke, darf bis auf 96 MB Reserve | frisst die Läufe, die 3.1 braucht | gemeinsame Reserve (s. W6), Heap aus 3.1 | M |
| 3.3 | `wasm/host_core.rs:2027-2100` | jede DMA zusammenhängend, ≤ 4 MB je Aufruf | große GPU-Objekte unmöglich, Fehlschlag bei Fragmentierung | `npk_dma_alloc_pages` mit Seitenliste | M |
| 3.4 | `shade/buffers.rs:47`, `widgets/mod.rs:2802`, `surface.rs:226` | jeder Fenster-/Widget-Puffer zusammenhängend (4K = 8100 Seiten), neuer je Neuzeichnen | nach Stunden schwarzes Fenster, misslungener Verify → GPU dauerhaft aus | virtuell aus verstreuten Rahmen; Puffer bei gleicher Größe wiederverwenden | M |
| 3.5 | `smp/fiber.rs:329`, `mm/stack.rs:53` | Fiber-Stack 1 MB zusammenhängend | Aufgabe fällt weg, `JOB_DONE` nie gesetzt → Aufrufer wartet ewig *(v)* | verstreut; bei Fehler Platz räumen | S |
| 3.6 | `microvm/cpu/vmx/ept.rs:60` | 256 MB Startfenster zusammenhängend | VM startet nicht auf fragmentiertem Wirt | über den 2-MB-Bedarfspfad | S–M |

## W4. Nur belegen, nie zurückgeben

Die Grenzen sind dadurch *Lebens*grenzen statt *Live*-Grenzen.

| # | Stelle | Regel | Folge | Lösung | Aufw. |
|---|---|---|---|---|---|
| 4.1 | `wasm/host_core.rs` | **kein `npk_dma_free`**; GPU-Modul 16384 Zuteilungen / 128 MB je Lauf | Modul-ENOMEM nach genug Kontexten/Bindungen | `npk_dma_free` (+ VT-d revoke), Deckel aus RAM | M |
| 4.2 | Gast | kein virtio-balloon / free page reporting; scudo `release_to_os_interval_ms=-1` | Gast-RAM wächst nur, Wirt verliert ihn für die Sitzung | `VIRTIO_BALLOON_F_REPORTING` + `DEFLATE_ON_OOM` (QEMU/crosvm), scudo-Wert prüfen | M |
| 4.3 | `microvm/devices/virtio_blk_pci.rs:768`, `blk_image.rs` | ganzes sqfs (343 MB) + Home-Stücke bleiben im Wirts-RAM | ~350 MB Wirt weg | verdrängbarer Stück-Cache | M |
| 4.4 | `virtio_gpu_pci.rs:225` | Freigabe wartet 200 ms, danach bleiben die Seiten | Frames bis Neustart weg | Liste zurückgestellter Freigaben, später eingesammelt | M |
| 4.5 | `virtio_gpu_i915.rs:409-421`, `gpudev.rs:723` | nach Wedge/Modulende kein Abbau der Clients | „frames are kept" bis Neustart | Clients bei Wedge abbauen | S |
| 4.6 | `microvm/cpu/mod.rs:931` | vCPU/Arbeiter nach 5 s nicht gestoppt → `VM_WEDGED` | keine neue VM bis Neustart | vCPU erzwingen (IPI/NMI), erneut versuchen | M |
| 4.7 | `wasm.rs:1103-1128` | Anzeige-DMA bleibt nach Modulende | bis 128 MB je Modulneustart | nach Umschalten der Anzeige freigeben | M |
| 4.8 | `gen8_ppgtt.rs`, `i915_gem_context.rs:111` | Tabellenseiten, leere VMs, Kontextspeicher nie an den Kernel | Hochwassermarke bis Modulende | mit 4.1 zurückgeben | M |

## W5. Warten unter Spin-Sperren, erfundene Fristen

Zeitüberschreitungen werden so zu Einfrierern oder zu dauerhaftem Schaden
(gesperrte Kontexte, GPU aus).

| # | Stelle | Regel | Folge | Lösung | Aufw. |
|---|---|---|---|---|---|
| 5.1 | `gpu/gpudev.rs:912-950` über `shade/mod.rs:485` | Compositor hält `CONSOLE`+`COMPOSITOR` und **hält den Kern an** bis 100 ms / 2 s; Apps spinnen auf `COMPOSITOR` ohne abzugeben | teilt das GPU-Modul den Kern → sicheres Einfrieren | abschicken, Sperren lösen, dann warten; abgebende Sperren | M |
| 5.2 | `microvm/devices/gpu_backend.rs:136,443` | `GPU` über `spin_or_yield` gehalten, vCPU-Exits spinnen darauf | teilt eine vCPU den Kern → **dauerhafter Hänger** *(v)* | vor dem Abgeben lösen | M |
| 5.3 | `gpu/gpudev.rs:80` + `virtio_gpu_i915.rs:285` | alle ccmds eines SUBMIT in 2 s, sonst EIO | **EIO sperrt den Kontext im Gast für immer** | keine Frist; nur Modultod beendet | S–M |
| 5.4 | `gpu/gpudev.rs:273,982` | `STATUS[64]`, Schlüssel `seq % 64`, Reihenfolge angenommen | falsches −5 → EIO-Sperre, 8 Fehlschläge → **GPU-Compositing dauerhaft aus** | Ergebnis je Auftrag bis gelesen; `FAILURE_LIMIT` abklingen lassen | S |
| 5.5 | `i915_gem_context.rs:41,802-861` | Request > ~5,6 s → GT wedged | schwerer WebGL-Frame tötet die GPU für alle | echter Heartbeat nach 5.6 | S nach 5.6 |
| 5.6 | `intel_execlists_submission.rs` | kein Timeslicing, keine Preemption, kein Engine-Reset | jeder Hänger/CS-Fehler wedged die GT bis Neustart | Linux: Timeslice ~1 ms, Preempt-to-busy, `intel_engine_reset`, Unwedge | L |
| 5.7 | `gpu/gpudev.rs:78`, `virtio_gpu_pci.rs:223` | GPU-Auftrag > 100 ms → CPU; FLUSH-Zaun ≤ 40 ms | Desktop auf CPU unter Last | zaungetrieben, Fristen nur zur Diagnose | M |
| 5.8 | `gpudev.rs:58,61` | Desktop-Kopien: 80 ms Warteschlange, 10 s Start | blockiert die Dienstschleife | ohne Blockieren abschicken | M |
| 5.9 | `virtio_gpu_pci.rs:183,719` | controlq 16; nach **jedem** FLUSH globale 1/60-s-Pause | alle Fenster teilen 60 Bilder/s, GL-Submits warten hinter Flushes | Zaun je Ausgabe bis zu deren vblank; Queue 256. **0.621.0/.1 so gebaut und am NUC deutlich schlechter (Maus ruckelt beim Firefox-Start, Video pausiert sekundenlang) → 0.621.2 zurück auf 0.620; Ursache offen** | M |
| 5.10 | `shade/mod.rs` | ~~Gastbilder ≤ 50 Hz, 20 Hz bei Mausbewegung~~ **Kernel 0.620.0:** mit GPU-Compositing 60 Hz, Weckzeit auf den Termin | 60-fps-Video ruckelte | offen: an die echte vblank takten | S |
| 5.11 | `microvm/cpu/mod.rs:544,549` | 3 s bis QUIT, 10 s hart | Sitzungsspeichern abgeschnitten | fortschrittsabhängig | S |

## W6. Feste Zahlen statt aus dem RAM abgeleitet

Dazu fehlt eine gemeinsame Abrechnung über Gast, Module und Heap.

| # | Stelle | Regel | Folge | Lösung | Aufw. |
|---|---|---|---|---|---|
| 6.1 | `microvm/devices/guest_mem.rs:89` | Gast-RAM 2 GB fest | OOM bei Tabs/4K-Video | aus Wirts-RAM, wachsen per virtio-mem; über 3 GB braucht 7.1 | S (≤ 3 GB) |
| 6.2 | `microvm/devices/nat.rs:116,222` | 1024 NAT-Verbindungen, TCP nach 2 min Leerlauf weg, kein FIN-Zustand | „masquerade table full", WebSockets/HTTP2 sterben still | wachsende Tabelle, conntrack-Zeiten (established 5 Tage, TIME_WAIT 120 s) | M |
| 6.3 | `virtio_gpu_i915.rs:25` | 4 GB GEM — laut Kommentar je VM, tatsächlich rechnerweit | GEM_CREATE ENOMEM | weg; `frames_spare` schützt schon | S |
| 6.4 | `microvm/devices/hostmem.rs:26` | GPU-Fenster (BAR2) fest 1 GB | MAP_BLOB scheitert *(v Video + WebRender)* | 64-Bit-BAR über dem RAM (7.1), aus RAM bemessen | M |
| 6.5 | `virtio_gpu_pci.rs:208-220` | 512 MB Wirtspixel, 2 GB Gast-Blobs, 8192 px | OUT_OF_MEMORY auf dem 2D-Weg | aus RAM ableiten | S |
| 6.6 | `forge_rt.rs:32,44` | Reserven 96 + 512 MB fest | nicht gegen Heap/DMA/Gast gerechnet | mit RAM skalieren, eine Abrechnung | S |
| 6.7 | `wasm.rs:79`, `host_core.rs:1671` | 4 MMIO-Abbildungen, BAR ≤ 16 MB | 5. Abbildung −1; Apertur nicht abbildbar | ohne Deckel, aus dem BAR | S |
| 6.8 | `shade/canvas.rs:22`, `host_core.rs:2425` | Canvas auf dem Heap, kein globaler Deckel, 4096 px | über 3.1 → Panik | in Rahmen (3.4), `try_reserve`, Budget aus RAM | S–M |
| 6.9 | `gui/background.rs:223-452` | Hintergrund 3–4 × Bildschirm auf dem Heap (~133 MB bei 4K) | über 3.1 → Panik beim Wechsel | Rahmen, `try_` | S |
| 6.10 | `host_core.rs:54` | HTTP-Körper bis 256 MB auf dem Heap | über 3.1 | in Modulspeicher streamen | M |

## W7. Gast-Layout und fehlende Standardmechanismen

| # | Stelle | Regel | Folge | Original / Lösung | Aufw. |
|---|---|---|---|---|---|
| 7.1 | `microvm/cpu/vmx/ept.rs:38,295`, `svm/npt.rs:55`, `linux/bzimage.rs:278` | RAM nur unter 3 GB, ein e820-Eintrag, hostmem genau bei 4 GB | Gast-RAM nie über 3 GB, BAR2 kann nicht wachsen | e820 geteilt, RAM über 4 GB, 64-Bit-BARs über dem RAM (QEMU pci-hole64) | L |
| 7.2 | `microvm/cpu/mod.rs:848`, `svm/lapic.rs:133`, `hostmem.rs:30` | ≤ 8 vCPUs; Kernmasken u32 / 32 Kerne | Wirte > 32 Kerne: unvollständige TLB-Verfolgung *(v)* | aus `core_count` | M |
| 7.3 | `virtio_gpu_pci.rs:192` | 8 Ausgaben (Protokoll: 16), 9. Fenster still verworfen | 9. Browserfenster ohne Wirtsfenster | 16; langfristig Fenster ohne Scanout | S/L |
| 7.4 | `virtio_blk_pci.rs:89` | kein FLUSH/DISCARD/MQ, Home erst am VM-Ende gespeichert | Wirtsabsturz verliert die Sitzung; Bild schrumpft nie | `F_FLUSH`, `F_DISCARD`, `F_MQ` | M |
| 7.5 | `virtio_9p_pci.rs:1324,331` | ganze Datei im Wirts-RAM, ≤ 1 GB je Datei | EFBIG bei großen Uploads | stückweise gegen npkFS | M |
| 7.6 | `virtio_snd_pci.rs:94` | nur Wiedergabe, ein Format; Gast auf 2 s Latenz | kein Mikrofon/WebRTC | Eingabestrom, Formate, Latenz aus dem Abfluss | M |
| 7.7 | `virtio_net_dev.rs:96` | ein Queue-Paar, keine indirekten Deskriptoren, nur IPv4 | ein Arbeiter für alles; IPv6-only unerreichbar | mq, indirect, IPv6-NAT | M–L |
| 7.8 | `microvm/cpu/mod.rs:687` | eine microVM systemweit (Globale) | zweite App-VM unmöglich | Gerätezustand je VM | L |
| 7.9 | `microvm/devices/virtio_input_pci.rs:127`, `shade/widgets/mod.rs:312` | 256 / 64 Ereignisse, älteste verworfen | hängende Taste | Bewegung zusammenfassen, Loslassen nie verwerfen | S |
| 7.10 | Gast-Init `microvm/linux/init/src/main.rs:322,339,356` | `sync` alle 3 s, Plattencache aus, 2 s Audio-Latenz | Folgen von 7.4/7.6 auf der Gastseite | fallen mit 7.4/7.6 weg | — |
| 7.11 | Gast-Konsole über 8250 (`console=ttyS0`) | ein VM-Exit je Byte; `npklogs`/MOZ_LOG lassen den Gast stehen, Video stockt im Takt des Logs | Diagnose verfälscht die Messung | virtio-console mit gebündelten Puffern (crosvm/QEMU) | M |

## W8. Echte Fehler und stille falsche Ergebnisse

| # | Stelle | Was | Folge | Lösung | Aufw. |
|---|---|---|---|---|---|
| 8.1 | `wasm/host_core.rs:1709-1720`, `forge_rt.rs:20` | MMIO 1:1 an der physischen Adresse; ab 64 GB liegen dort die forge-Instanzen (16 GB je Platz). Der NUC-Framebuffer liegt bei `0x40_0000_0000` = Platz 12. `npk_mmio_map_bar` loggt einen Abbildungsfehler **und gibt trotzdem den Handle zurück** | Instanz auf Platz 12 startet nicht; umgekehrt landen Treiber-MMIO-Zugriffe im Speicher einer Instanz (`map_page` überschreibt nicht, meldet `AlreadyMapped`) | MMIO in ein eigenes virtuelles Fenster; −1 bei Fehler | S |
| 8.2 | `drivers/framebuffer.rs:229,678` | `init_from_gpu`/`clear` geben Schattenpuffer frei, während gpudev-BOs 1/2 und VT-d-Freigaben noch darauf zeigen | GPU schreibt in freigegebenen Speicher | BOs zurückziehen und widerrufen, dann freigeben | S |
| 8.3 | `gui/login.rs:363` + `drivers/framebuffer.rs:594` | nach `lock` bleibt der GUI-Modus aus | `kprintln` unter `with_fb` nimmt `CONSOLE` erneut → Verklemmung; SERIAL↔CONSOLE über Kerne | GUI-Modus nach dem Entsperren zurück; `kprintln` fragt `shade::ACTIVE` | S |
| 8.4 | `gpu/gpudev.rs:66,1150` | Antwort > 64 KB → leere Antwort mit Status 0 | stilles falsches Ergebnis | sichtbar scheitern, Puffer wachsen lassen | S |
| 8.5 | `smp/fiber.rs:168` | 1024 Wecker; voll → `NO_WAKER` | Fiber wird nie geweckt | Tabelle wachsen lassen oder Spawn scheitern | S |
| 8.6 | `gpudev.rs` `FAILURE_LIMIT` (s. 5.4) | 8 Fehlschläge rasten ein | GPU-Compositing aus bis `gpu verify` | abklingen | S |
| 8.7 | `shade/terminal.rs:13,18` | 256 Spalten (4K-Zeile ~400), Platz 255 = „kein Terminal" | abgeschnittene Zeilen; Fehlmarkierungen | dynamisch, `Option` | S |
| 8.8 | `microvm/devices/virtio_gpu_ctx.rs` `run_submit`/`write_response` | leere oder nicht schreibbare Antwort, Sequenznummer läuft trotzdem weiter | Gast liest alten Antwortslot; bei QUERY ein Heap-Überlauf im Gast *(v, heute kein Auslöser bekannt)* | ohne Antwort keine Sequenznummer, sichtbar scheitern | S |
| 8.9 | `virtio_ccmd.rs` (fehlerhaft = `Err`) | unbekannter ioctl/Parameter, QUERY > 64 KB → keine Antwort, keine Sequenznummer | Gast wartet ewig in `vdrm_host_sync` | virglrenderer tut dasselbe; wo Linux ein errno hat, errno antworten (Kontextparameter in 0.21.1 erledigt) | S |

## W9. Langsame Suchen auf heißen Pfaden

Werden mit wachsenden Tabellen schlimmer — deshalb zusammen mit W2/W3
umbauen.

| # | Stelle | Was | Lösung |
|---|---|---|---|
| 9.1 | `execbuffer.rs:269-289` | Überlappungsprüfung: Objekte × 8192 × 2, je Belegtprüfung 8 × 128 | Intervallbaum (`drm_mm` des Adressraums) |
| 9.2 | `i915_gem_context.rs:821` | bei jedem Zombie: Dienstschleife über alle 8192 Objekte je Durchlauf | Zombieliste |
| 9.3 | `mm/memory.rs:213-252` | `allocate_contiguous_below` bitweise von oben unter der globalen Sperre; bei jedem Gast-Seitenfehler | Buddy/Lauf-Index, „kein 2-MB-Block mehr" merken |
| 9.4 | `mm/memory.rs:77` | `allocate_frame` linear | Freiliste / Cache je Kern |
| 9.5 | `forge_rt.rs:109` | `memory.grow` je Seite zwei Sperren | bündeln |
| 9.6 | `wasm/host_core.rs:2037` | Summe über alle DMA-Zuteilungen je Zuteilung | laufende Summe |
| 9.7 | `virtio_gpu_pci.rs` | Ressourcen als Vec, linear gesucht | Map nach Id |
| 9.8 | `drivers/vtd.rs:512` | je 4-KB-Seite BTreeMap-Eintrag + clflush; IOTLB-Flush bis 1 s unter Sperre | Bereichsbaum, 2-MB-Blätter |
| 9.9 | `shade/mod.rs:539`, `:803` | Fläche ohne Schaden trägt ganzes Fenster bei; jeder Widget-Aufruf = ganzer Bildschirm | Schaden-basiert, unsichtbare filtern |
| 9.10 | `gen8_ppgtt.rs`, `virtio_gpu_i915.rs:105` | nur 4-KB-PTEs; GEM-Seiten einzeln, synchron genullt | `gen8_ppgtt_insert_huge`, Rahmen am Stück, später füllen |

## Vorschlag zur Reihenfolge

1. **W1 (zurückstellen statt ablehnen)** — Modul 0.20.0 — die Hänger am Gerät kommen daher;
   sonst bleibt jede andere Zeile in W1/W2 ein Absturz statt einer Wartezeit.
2. **4.1 (`npk_dma_free`) und 4.2 (Balloon / free page reporting)** — die
   zwei Stellen, an denen ein langer Betrieb sicher vollläuft (Florian
   2026-10-16: „sonst laufen wir ja zwangsläufig irgendwann mal voll").
   Regel dahinter: was je Benutzung belegt wird, geht beim Ende zurück;
   ein Pool nur, wenn sein Höchststand durch eine echte Größe begrenzt ist.
3. **5.3, 5.4, 8.6** (S) — sperren heute unschuldige Kontexte bzw. schalten
   die GPU ab.
4. **8.1, 8.2, 8.3** (S) — echte Fehler (8.1, 8.3 in Kernel 0.616.0).
5. **W2** — mit dem neuen Heap direkt; zusammen mit 9.1/9.2.
6. **6.1 + 7.1** — Gast-RAM dynamisch, mit Layout über 4 GB (6.4, das
   GPU-Fenster, ist in Kernel 0.616.0 schon so groß wie der Wirts-RAM).
7. **3.1** (Heap ohne Zusammenhang) — die Kernel-Wurzel, an der viele
   W3/W4/W6-Zeilen hängen.
8. **5.6** Preemption + Engine-Reset — groß, aber erst danach ist ein
   Gast-Hänger kein Ende der GPU für alle mehr.

Der Rest einzeln nach Bedarf.
