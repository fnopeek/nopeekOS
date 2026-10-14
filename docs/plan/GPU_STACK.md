# GPU-Stack — eine Schnittstelle, die Karte darunter egal

**Stand:** 2026-10-14. `gpu_intel_tgl` 0.8.0 faehrt die Karte am NUC nach
Linux hoch (Display-Auslesung, Uebernahme des Firmware-Bildes, GT, alle vier
Engines mit Execlists und Kernelkontexten, MSI). Konzept fuer den Desktop und
die Vertrauensstufen festgelegt (§2.3, §3.3, §5, §9); gpudev selbst ist noch
nicht gebaut.
**Ausloeser:** Florian: „aktuell machen wir kaum gebrauch von einer gpu alles
laeuft ueber die cpu, microvm, unser system etc." — und dazu: eine
Schnittstelle, damit die Karte darunter keine Rolle spielt; die microVM bleibt
schlank und bekommt trotzdem 3D; der Desktop (Fenster, Glas, Effekte) kommt
von der CPU weg; **die CPU bleibt der Rueckfall** fuer Karten, die wir noch
nicht kennen. Und: **der Treiber als Modul, analog zum WLAN-Treiber, nicht im
Kernel.**

Portierkarte fuer `gpu_intel_tgl`: `GPU_INTEL_LINUX_MAP.md`.
Verwandt: `MICROKERNEL_REFACTOR.md` (Treiber als WASM), `MICROVM_V2.md`,
`CORES_AND_EVENTS.md` (Ereignisse statt Takt), Memory
`project_baremetal_gfx_perf.md` (BCS-Bring-up am NUC, UC-Framebuffer),
`feedback_microkernel_drivers_out.md`.

---

## 0. Kurzfassung

```
 microVM (Linux + Mesa)     Desktop (shade)          Apps (WASM)
        │                        │                       │
  virtio-gpu                Szenenliste              Flaechen wie heute,
  DRM native context        (Daten, feste Shader     der Compositor
  (rohe Befehlspuffer,       des Moduls)             beschleunigt
   eigenes PPGTT je VM)
        └──────────┬─────────────┘
                   ▼
        gpudev — Kernel-Seite: Puffer, Kapabilitaeten, Weiterleitung
                   │  (Befehlsring Kernel → Modul, wie netdev beim WLAN)
                   ▼
        GPU-Modul (WASM, forge)  — EINES je Karte, besitzt die Karte ganz
      gpu_intel_tgl (i915-Weg)  gpu_amd_renoir (amdgpu-Weg)
                   │
        kein Modul / Modul tot  →  CPU-Rueckfall (heute: GOP + memcpy)
```

Drei Entscheidungen tragen das Papier:

1. **Zwei Schnittstellen nach oben, nicht eine.** Die microVM bekommt die
   Kernel-Schnittstelle der Karte durchgereicht (native context), weil im Gast
   ein echtes Mesa laeuft. Unsere eigenen Programme haben kein Mesa und
   bekommen eine Szenenliste, die das Modul mit eigenen, beim Bauen
   uebersetzten Shadern ausfuehrt (§5). Ein gemeinsames 3D-API fuer beide
   waere ein eigenes Mesa — genau das, was wir vermeiden. Rohe
   Befehlspuffer gibt es nur auf der Fremd-Stufe (§2.3).
2. **Der Wirt braucht kein Mesa.** Bei native context laeuft der
   kartenspezifische Teil (Shader-Compiler, Befehlsaufbau) IM GAST; der Wirt
   legt Puffer an, reicht Befehlspuffer an die Karte und meldet Fences. Das
   ist genau die Haelfte, die wir ohnehin aus dem Linux-Kernel portieren.
3. **Ein Modul je Karte, und es besitzt die Karte ganz** (§3.2). Benannt
   wie die WLAN-Treiber nach dem, was es treibt: `gpu_intel_tgl` (Tiger
   Lake, i915-Weg ohne GuC), spaeter z. B. `gpu_amd_renoir` — Linux deckt
   mit einem `i915` alle Generationen ab, wir portieren je Plattform den
   festgelegten Graphen. Der Kernel
   behaelt nur, was er fuer den Rueckfall braucht.

---

## 1. Ist-Zustand (ausgezaehlt 2026-10-12)

| Teil | Wo | Was es tut |
|---|---|---|
| `GpuHal` | `kernel/src/gpu/mod.rs` | Trait: Modus, Flip, vblank, BCS-Blit (GGTT-Offsets) |
| `intel_xe.rs` | Kernel, 2295 Zeilen | TGL `0x9A78` + ADL-N: GGTT, ForceWake, Execlists, LRC, **BCS-Ring** (`XY_FAST_COPY_BLT`), Flip ueber `PLANE_SURF`. Modeset nur ADL-N. |
| GOP | `kernel/src/gpu/gop.rs` | Rueckfall, alles andere (AMD, HP-Whiskey-Lake) |
| Compositor | `shade/compositor.rs` | **Keine Pixelpuffer je Fenster** — rendert direkt in den Schattenpuffer |
| Rasterizer | `shade/widgets/abi.rs:938` | Trait `Rasterizer` (rect, rounded, text, …), einzige Umsetzung `raster/cpu.rs`; „a GPU backend would implement the same trait" |
| Gast-Flaechen | `shade/surface.rs` | je VM-Fenster drei Puffer (back → ready → front), Kopie auf CPU |
| virtio-gpu | `microvm/devices/virtio_gpu_pci.rs` | **nur 2D**, 0 Capsets, keine Blob-Ressourcen; GPU-Arbeiter-Fiber (`gpu_backend.rs`) |
| Treiber-ABI | `wasm/forge_glue.rs` | `npk_pci_bind*`, `npk_mmio_map_bar` + `read/write8..64`, `npk_dma_alloc*`, `npk_dma_phys_addr`, `npk_irq_register/arm/wait`, `npk_wait` |
| IOMMU | `drivers/acpi.rs` | DMAR/IVRS werden **nur gelesen** (Registerbloecke), nicht programmiert |

Die Geraete:

| | NUC | IdeaPad |
|---|---|---|
| GPU | `8086:9A78` Tiger Lake-LP GT2 (Gen12.1) | `1002:164c` Lucienne (Renoir-Familie, Vega, DCN 2.1) |
| Linux-Treiber | i915 (Execlists auf TGL, **ohne GuC**) | amdgpu (PSP 12, SMU 12) |
| Heute | nativ, BCS-Blit, Flip | GOP 1920x1200, CPU-Blit |
| Firmware fuer Befehle | keine noetig | Pflicht: `renoir_*` ueber den PSP (sOS laeuft auf der APU schon) |

**Der Weg eines VM-Bildes heute:** Gastseiten → `back` (CPU-Kopie im
GPU-Arbeiter) → Compositor → Schattenpuffer (CPU-Kopie) → Scanout (BCS am NUC,
CPU am IdeaPad). Zwei bis drei volle Kopien je Bild, und der Gast rendert mit
llvmpipe — auch das auf der CPU.

**Nebenbefund:** `gpu` sagt auf dem IdeaPad `BCS: off (probe failed)` und
`blit did not paint`, obwohl es dort gar keinen Probe gibt
(`intent/system.rs:1255`). Soll „no native driver" heissen. Dasselbe am NUC
ohne Bild der Firmware: dort steht `Pending … use 'gpu init'`, und der
eigentliche Grund — die Firmware hat kein Bild geliefert, Monitor beim
Booten aus? — steht nirgends. Die Ausgabe stammt aus der ADL-N-Zeit (N100)
und wird mit gpudev neu: Besitzer der Karte, Modul, Selbsttest, Rueckfall,
und warum.

---

## 2. Zielbild

### 2.1 Die drei Nutzer

- **microVM:** rendert mit dem echten Mesa-Treiber der Karte (radeonsi/radv
  bzw. iris/anv), OpenGL, Vulkan, WebGL in LibreWolf, Videodekodierung ueber
  VA-API. Ihre Bilder liegen in GPU-Puffern, die der Compositor ohne CPU-Kopie
  verwendet.
- **Desktop:** Fenster als eigene Puffer; Zusammensetzen, Glas, Ecken,
  Schatten auf der GPU; Zeiger und Video auf Display-Ebenen.
- **Apps (WASM):** unveraendert. Sie malen in Flaechen; ob die CPU oder die
  GPU rastert, entscheidet der Compositor.

### 2.2 Der CPU-Rueckfall ist ein Backend, kein Sonderweg

Jede Schnittstelle nach oben hat eine CPU-Umsetzung, und die ist heute schon
da: `raster/cpu.rs`, der Compositor, GOP + memcpy, llvmpipe im Gast. Ein
GPU-Modul **ersetzt** Umsetzungen, es fuegt keine Pflichtpfade hinzu. Faellt
das Modul aus (Absturz, unbekannte Karte, Selbsttest rot), laeuft alles auf
dem heutigen Weg weiter. Dieselbe Regel wie beim BCS: **erst der
Readback-Selbsttest, dann darf die GPU das Display treiben** — sonst ist das
Bild schwarz statt langsam.

### 2.3 Wie Linux es macht, und was wir davon nehmen

Linux teilt die Grafik in drei Schichten: der **Kerneltreiber** (i915,
amdgpu) besitzt die Hardware, verwaltet Puffer (GEM), gibt jedem Prozess
einen eigenen GPU-Adressraum (PPGTT/GPUVM) und reicht Befehlspuffer an die
Engines; der **Userspace-Treiber** (Mesa) in jeder App uebersetzt
OpenGL/Vulkan in Befehle der Karte und kompiliert die Shader; der
**Compositor** bekommt die Fensterpuffer als Griffe (dma-buf) und setzt sie
selbst mit OpenGL/Vulkan auf der GPU zusammen, wo es geht ganz ohne Rechnen
auf Display-Ebenen (direct scanout, Overlays). Die CPU beschreibt je Bild
nur noch, WAS gezeichnet wird; die Pixel fasst nur die GPU an.

Die Grenze ist bei Linux die Hardware: jeder Prozess darf **rohe
Befehlspuffer** abgeben, geschuetzt durch das PPGTT je Prozess, die
Register-Positivliste der Hardware fuer unprivilegierte Befehlspuffer
(`intel_workarounds.c` whitelist), Engine-Reset bei Haengern und die IOMMU.
Einen Befehlsparser gibt es auf Gen12 nicht mehr. Genau diese Flaeche — der
Kerneltreiber beim Seitenverwalten, Hardwarefehler hinter raffinierten
Befehlspuffern — ist die groesste Angriffsflaeche im Linux-Grafikstapel.

**Was wir uebernehmen:** die drei Schichten (das GPU-Modul ist der
Kerneltreiber, Puffergriffe sind Kapabilitaeten, der Compositor setzt auf
der GPU zusammen und nutzt Ebenen). **Was wir anders machen:** wer rohe
Befehlspuffer abgeben darf.

| Vertrauensstufe | Wer | Was er der GPU gibt | Grenze |
|---|---|---|---|
| **Eigen** | Compositor (shade) | eine **Szenenliste**: Daten, kein Code (§5.3) | das Modul fuehrt sie mit eigenen, festen Shadern aus und prueft jeden Griff und jedes Rechteck |
| **Eigen, signiert** | native WASM-Apps | Flaechen wie heute; spaeter dieselbe Art Liste | Signatur + Caps: ohne beides kein Zugriff |
| **Fremd** | microVM-Gast (Mesa im Gast) | **rohe Befehlspuffer** (native context, §4) | eigenes PPGTT je VM, Register-Positivliste der Hardware, Zeitlimit + Engine-Reset + Kontextsperre, IOMMU (§8) |

Native Apps stammen zwangslaeufig von uns — ohne Signatur und Caps gibt es
keinen Zugriff, das Risiko ist dort klein (Florian, 2026-10-14). Die
Szenenliste bleibt trotzdem der Weg fuer sie, aus einem Grund, der mit der
Herkunft des Codes nichts zu tun hat: **eine signierte App kann fremde DATEN
verarbeiten.** beak rendert Webseiten; ein WebGL-Shader einer Seite ist
fremder GPU-Code in einer signierten App. Solcher Code gehoert auf die
Fremd-Stufe (eigener Kontext mit PPGTT, wie ein Gast), nicht auf den Weg des
Compositors.

---

## 3. Der GPU-Dienst als Modul

### 3.1 Warum ein Modul — und warum forge das traegt

Nach `feedback_microkernel_drivers_out.md`: neue Treiber als WASM, ueber die
Treiber-ABI, wie `wifi_rtl8822ce`. Fuer eine GPU passt das besser als fuer
eine Netzkarte, weil der **heisse Pfad wenige Registerzugriffe hat**: ein
Befehlspuffer ist Speicher, den die Karte selbst liest. Je Abgabe sind es eine
Handvoll MMIO-Schreibzugriffe (Execlist-Port bzw. Doorbell) — die Pixel
fasst das Modul nie an. Mit forge (Modul als nativer Code) kostet ein
Hostaufruf wenig; das Modul ist damit nicht der Engpass. **Gemessen ist das
noch nicht** — Stufe 1 misst es (§9).

Was **nicht** durch das Modul gehen darf: Pixeldaten. `npk_dma_read/write`
kopiert durch den WASM-Speicher — fuer Ringe und Seitentabellen richtig, fuer
ein 33-MB-Bild falsch. Puffer legt deshalb der Kernel an (§3.3), das Modul
bekommt Griffe und schreibt nur, wo sie in den GPU-Adressraum gehoeren.

### 3.2 Ein Besitzer je Geraet

Die Lehre vom IdeaPad (*ein Controller hat einen Besitzer*, Memory
`project_lenovo_no_keyboard.md`) gilt hier doppelt: Display, GGTT, ForceWake
und die Ringe haengen alle an derselben MMIO-BAR. **Das Modul bindet die Karte
ganz** (`npk_pci_bind`), und der Kernel fasst danach nichts mehr an ausser
seinem Rueckfallwissen: wo der letzte gueltige Scanout-Puffer liegt.

**Ablauf, entschieden 2026-10-13 (Florian):**

| Phase | Wer zeichnet | Wie |
|---|---|---|
| Boot bis Login | CPU | GOP + CPU-Kopie — wie heute (der BCS startet schon heute erst nach dem Login, `main.rs` vor `shade::init`) |
| nach dem Login, Modul noch nicht bereit | CPU | dasselbe; der Desktop steht sofort, die GPU kommt dazu |
| Modul bereit | GPU | `gpu_intel_tgl` steht in `autostart` wie der WLAN-Treiber, faehrt die Karte nach Linux hoch und meldet sich ueber gpudev |
| kein Modul / unbekannte Karte | CPU | bleibt beim Rueckfall — kein Fehler, nur langsamer |
| Modul stirbt | CPU | Compositor schaltet zurueck, schreibt nichts in die Karte |
| Monitor beim Booten aus | — | die Firmware richtet keinen Modus ein, GOP fehlt (`GPU: no display available`, gesehen am NUC 2026-10-13). Stufe 1 hat dann nichts zu uebernehmen; ein Bild gibt es erst mit Stufe 4 (Modeset + Hotplug). Bis dahin: Neustart mit Monitor an. |

**Die Uebergabe im Einzelnen:**

1. Das Modul bindet die Karte **exklusiv**. `npk_pci_bind` kennt heute
   keinen Besitzer — zwei Module (oder Kernel und Modul) koennten dieselbe
   Karte binden. Neu: ein Besitzer je Geraet; ein Kerneltreiber, der die
   Karte gerade benutzt (`intel_xe.rs`, solange es ihn gibt), zaehlt als
   Besitzer und gibt erst nach `gpudev_handover` ab.
2. Der Kernel haelt den Compositor an (das laufende Bild wird fertig),
   laesst den Kernel-BCS leerlaufen und fasst die Karte ab da nicht mehr an.
3. Das Modul faehrt **den ganzen Linux-Weg**: GT, und vom Display die
   Zustandsauslesung samt Uebernahme des Firmware-Bildes
   (`intel_initial_plane_config` reserviert es in Stolen und GGTT, bevor
   `init_ggtt` laeuft — `GPU_INTEL_LINUX_MAP.md` §4). Kein Modeset: das Bild
   der Firmware bleibt stehen.
4. Readback-Selbsttest (dieselbe Regel wie heute beim BCS), dann
   `gpudev_ready` — erst jetzt schaltet der Compositor auf den gpudev-Weg.
   Faellt der Test durch, bleibt alles auf der CPU.

**Gebaut 2026-10-13 (d43bdff4c):** `pci::Owner` — ein Besitzer je Geraet;
`npk_pci_bind` gibt `-3`, wenn ein anderer Treiber das Geraet haelt. NVMe,
xHCI und die Intel-NIC tragen sich als Kernelbesitzer ein (vorher haette
ein Modul den Controller binden koennen, auf dem npkFS liegt). Der native
GPU-Treiber traegt sich ebenso ein und ist der einzige Kernelbesitzer, der
abgibt: `gpu::yield_to_module` tauscht ihn unter der GPU-Sperre gegen einen
GOP-Treiber auf dem gezeigten Bild, ohne Registerzugriff. Beim Modultod
bleibt auf einem Display-Controller das Bus-Master-Bit an (der Scanout
liest auch als Bus-Master), und die DMA-Puffer des Moduls werden deshalb
**nicht** freigegeben, sondern bis zum Neustart behalten — die Karte
koennte sie noch lesen. Sobald `gpu_intel_tgl` abgibt, gehoert davor ein
Stillsetzen der Engines (GT-Reset) in den Rueckweg; dann koennen die
Puffer wieder zurueck.

**Geschlossen (Kernel 0.600.0):** `npk_mmio_map_phys` lehnt Adressen in der
BAR eines Geraets ab, das ein anderer Treiber besitzt. Moeglich durch die
BAR-Tabelle, die der Kernel jetzt beim Booten anlegt (`pci::size_bars`,
wie Linux `__pci_read_base`); `npk_mmio_map_bar` misst damit keine
laufende Karte mehr aus.

**Rueckweg beim Modultod (F1):** Das Modul scannt nur Puffer aus, die dem
Kernel gehoeren (`flip(bo)` nimmt einen Griff, keine Adresse) — der erste
ist das uebernommene Firmware-Bild. Die GGTT-Eintraege dafuer bleiben
stehen, wenn das Modul stirbt. Der Kernel zeichnet also auf der CPU in den
zuletzt geflippten Puffer weiter und schreibt dabei kein Register. Offen
bleibt der Tod MITTEN in einem GGTT-Umbau — dagegen schuetzt nur, dass das
Modul den Scanout-Eintrag nie anfasst, solange er im Scanout steht.

### 3.3 Die neue ABI: `gpudev`

Analog zu `npk_netdev_register` beim WLAN: das Modul meldet sich als
GPU-Geraet an, bleibt resident und bedient danach Auftraege des Kernels. Wie
bei netdev holt es sie per Hostaufruf ab und wird ueber `npk_wait` geweckt —
gemeinsam mit seinem MSI-Vektor (F3). Ein Kern dient allen drei Nutzern:

| Baustein | Compositor | microVM |
|---|---|---|
| Puffergriffe vom Kernel (Kapabilitaeten) | ja | ja |
| Fences (Sequenznummer + Weckgriff) | ja | ja |
| Kontext mit eigenem PPGTT | einer | einer je VM |
| Szenenliste, ausgefuehrt mit festen Shadern des Moduls | ja | nein |
| roher Befehlspuffer | **nie** | ja, nur in sein PPGTT |
| Ebenen, Flip, vblank | ja | sein Bild als Textur in der Szene |

**Der erste Schnitt (v0)** baut den Kern und als ersten Nutzer die heutige
Bildkopie (Schattenpuffer → Firmware-Bild per `XY_FAST_COPY` auf bcs0):
Gleichstand mit `intel_xe.rs`, der danach entfaellt. Die Kopie ist die
billigste Probe, ob Griffe, Abgabe und Fences zwischen Kernel und Modul
tragen — ein Fehler dort zeigt sich sofort am Bild. Synchron je Bild mit
Zeitlimit wie heute; die Sequenznummer steht trotzdem von Anfang an in der
ABI, damit spaeteres Pipelining sie nicht bricht. Auftragscodes sind
versioniert.

**v0, festgelegt 2026-10-14** (Stufe 1b). Hostaufrufe, alle gegated mit
`Rights::HARDWARE` und einem gebundenen Display-Controller (PCI-Klasse 0x03);
ein eigenes `Rights::GPU` kommt, wenn der erste Nicht-Treiber gpudev braucht.

| Aufruf | Bedeutung |
|---|---|
| `npk_gpudev_register(version) -> 0 \| <0` | das Modul wird der GPU-Dienst seiner Karte; der Kernel legt die Puffer an (unten) |
| `npk_gpudev_bo_info(id, out) -> 0 \| -1` | schreibt `{phys: u64, size: u64, pitch, width, height, format: u32}`; `phys` 0 bei Puffer 0 |
| `npk_gpudev_bo_bound(id, gpu_addr: i64)` | wo das Modul den Puffer in seine GGTT gelegt hat (fuer den Kernel nur ein Name, den der Compositor heute als Quelle/Ziel weiterreicht) |
| `npk_gpudev_ready(ok)` | Selbsttest-Ergebnis; erst mit `ok = 1` schaltet der Kernel den Compositor um |
| `npk_gpudev_poll(out, max) -> n` | holt Auftraege `{seq, op, a0..a5: u32}` (32 Bytes) |
| `npk_gpudev_complete(seq, status)` | Auftrag fertig (`status` 0) oder abgelehnt/gescheitert (<0) |
| `npk_wait(WAIT_GPU_CMD = 64)` | weckt, wenn ein Auftrag wartet |

Puffer in v0: **0** = das uebernommene Firmware-Bild (das Modul kennt es aus
`intel_plane_initial`), **1/2** = die zwei Schattenpuffer des Compositors
(physisch zusammenhaengend, seitenausgerichtet). Auftrag v0:
`OP_COPY_RECT = 1` mit `src_bo, dst_bo, x, y, w, h` — das Modul prueft
beide Griffe und dass das Rechteck in beiden Puffern liegt, und lehnt sonst
mit `status < 0` ab. Der Kernel wartet synchron mit Zeitlimit (100 ms) und
kopiert bei Ablehnung, Zeitueberschreitung oder Modultod auf der CPU.

**Modultod:** `release_hw` meldet gpudev ab: wartende Auftraege scheitern
sofort, der Compositor faellt auf GOP + CPU-Kopie auf das Firmware-Bild
zurueck, kein Registerzugriff. Die Schattenpuffer gehoeren dem Kernel und
bleiben; die Karte liest sie nach dem Tod nicht mehr (kein Auftrag mehr).

**v1, festgelegt 2026-10-14** (A3 + A5). Ersetzt v0; Kernel und Modul
werden zusammen freigegeben (`npk_gpudev_register(1)`, v0 wird abgelehnt).

*Puffer:* 0, 1, 2 bleiben fest (Firmware-Bild, zwei Schattenpuffer). Ab 3
vergibt der Kernel Griffe **dynamisch** und nie zweimal: fuer jeden Puffer,
den eine Szene liest (Fensterpuffer, Terminal-Textebenen, Gast-Fronts,
scharfe und unscharfe Tapete). Ein Puffer ist eine **Seitenliste** — der
erste Pixel darf mitten in einer Seite liegen (`offset`). Gibt der Besitzer
einen angemeldeten Puffer frei, haelt der Kernel die Seiten, bis das Modul
den Abmeldeauftrag erledigt hat; erst dann gehen sie an den Allokator.

| Aufruf | Bedeutung |
|---|---|
| `npk_gpudev_register(1)` | wie v0 |
| `npk_gpudev_bo_info(id, out) -> 0 \| -1` | 40 Bytes `{phys: u64, size: u64, pitch, width, height, format, offset, pages: u32}`; `phys` = Seite des ersten Pixels (0 bei Puffer 0), `size` = Bytes ab dem ersten Pixel |
| `npk_gpudev_bo_pages(id, first, out, max) -> n \| -1` | physische Adressen (`u64`) der Seiten `first..`, hoechstens `max`; -1 bei Puffer 0 |
| `npk_gpudev_bo_bound(id, gpu_addr)` | wo der erste Pixel im PPGTT liegt (Seite + `offset`) |
| `npk_gpudev_ready(caps)` | Bit 0: Kopien, Bit 1: Szenen; 0 = Selbsttest gescheitert |
| `npk_gpudev_poll(out, max) -> n` | wie v0, 32 Bytes `{seq, op, a0..a5}` |
| `npk_gpudev_job_data(seq, offset, out, len) -> n \| -1` | Nutzlast eines abgeholten, noch nicht erledigten Auftrags |
| `npk_gpudev_complete(seq, status)` | wie v0 |

*Auftraege* (in Reihenfolge erledigt; das Modul arbeitet sie der Reihe nach ab):

| op | Name | Argumente | Bedeutung |
|---|---|---|---|
| 1 | `COPY_RECT` | `src, dst, x, y, w, h` | wie v0 |
| 2 | `BO_ADD` | `id` | Puffer abfragen, binden, `bo_bound` melden; `status < 0`: der Kernel setzt Szenen mit ihm auf der CPU zusammen |
| 3 | `BO_REMOVE` | `id` | entbinden und vergessen; danach gibt der Kernel die Seiten frei |
| 4 | `SCENE` | `target, bytes` | eine Szenenliste ausfuehren (Nutzlast unten) in den Puffer `target` |

*Nutzlast von `SCENE`* (alles `u32` LE): Kopf 64 Bytes, dann `n_damage`
Rechtecke zu 16 Bytes `{x, y, w, h}`, dann `n_items` Eintraege zu 64 Bytes
(Wort 0 = Art, Woerter 1.. = Felder), dann `glyph_bytes` Alpha-Bytes (auf 4
aufgefuellt).

| Kopf-Byte | Feld |
|---|---|
| 0 | `magic` = `0x314E4353` („SCN1“) |
| 4, 8, 12 | `n_items`, `n_damage`, `glyph_bytes` |
| 16 | `flags`: Bit 0 Tapete da, Bit 1 unscharfe Tapete fertig |
| 20, 24, 28 | Griffe: Tapete scharf, Tapete unscharf, Glasquelle (unscharf, sonst scharf); `0xFFFFFFFF` = keiner |
| 32 | `blur_radius` (Kastenradius in 4er-Bloecken; 0 = Paneele liegen auf den scharfen Pixeln) |
| 36, 40, 44 | Glas-Tinte `ceil`, `tint`, `tint_w` (`render::GlassInk`) |
| 48, 52 | Bildschirm `width`, `height` |

| Art | Eintrag | Felder ab Wort 1 |
|---|---|---|
| 1 | `Background` | — |
| 2 | `Glow` | `x, y, w, h, radius, color, band, alpha` |
| 3 | `GlassBackdrop` | `x, y, w, h, radius` |
| 4 | `Chrome` | `x, y, w, h, border_a, border_b, bg, rounding, border, border_opacity, bg_opacity, paint_content` |
| 5 | `Content` | `x, y, w, h, radius, fit` (0 Scale, 1 Extend)`, src` |
| 6 | `Panel` | `x, y, w, h, src` |
| 7 | `Text` | `x, y, w, h, src` |
| 8 | `Fill` | `x, y, w, h, color, alpha, radius` |
| 9 | `CloseGlyph` | `x, y, w, h, size` (Kantenlaenge der Glyphe)`, color, glyph_off` |

Bedeutung: genau `scene::execute` — fuer jedes Schadensrechteck (am
Bildschirm beschnitten) jeder Eintrag der Reihe nach, beschnitten auf das
Rechteck; Quellmasse sind die des Puffers (`width`, `height`, Zeilen dicht
ausser bei der Tapete). Das Ergebnis muss Bit fuer Bit (alle 32 Bit) dem
CPU-Ausfuehrer gleichen; der Kernel prueft das am ersten echten Bild
(Kopie des Ziels, CPU und GPU je in eine, Vergleich) und schaltet erst bei
Gleichstand um. Das Modul prueft jeden Griff (gebunden, Masse passen zur
Groesse) und dass das Ziel bildschirmgross ist.

*Zaeune je Puffer:* jeder Auftrag merkt sich in jedem Puffer, den er liest
oder schreibt, seine Nummer; wer auf der CPU in einen Puffer schreibt (oder
den Bildschirm liest), wartet nur auf dessen Zaun. Bis A6 wartet das
Zusammensetzen selbst auf sein Ergebnis, weil danach der Zeiger auf der CPU
hineingebacken wird.

**Kernel → Modul:**

| Befehl | Bedeutung |
|---|---|
| `ctx_create(kind)` | neuer Kontext mit **eigenem GPU-Adressraum** (PPGTT / GPUVM); `kind` = Compositor oder VM-Kontext n |
| `bo_bind(ctx, bo, gpu_va, flags)` | einen Kernelpuffer in den Adressraum eines Kontexts legen |
| `bo_unbind(ctx, gpu_va)` | wieder heraus |
| `submit(ctx, engine, batch_va, fence)` | Befehlspuffer abgeben |
| `plane_set(plane, bo, rect, alpha)` | Display-Ebene setzen (Scanout, Zeiger, Video) |
| `flip(bo)` | Hauptebene beim naechsten vblank |
| `query(param)` | Kennwerte (fuer den Gast: `GETPARAM`/`QUERY` bzw. `amdgpu_info`) |

**Modul → Kernel:** Fence erreicht, vblank, Fehler/Hang (Engine-Reset),
Selbsttest-Ergebnis.

**Puffer (`bo`) gehoeren dem Kernel.** Er legt sie an (Gast-Blob, Fenster,
Schattenpuffer), haelt ihre Seiten fest und gibt dem Modul fuer jede Seite die
physische Adresse nur ueber den Griff heraus. Damit gilt: das Modul kann nur
Seiten in GPU-Tabellen eintragen, die der Kernel ihm fuer diesen Kontext
gegeben hat — **solange es sich an die Griffe haelt**. Was es in Wahrheit
eintraegt, prueft ohne IOMMU niemand (§8).

### 3.4 Was der Kernel dafuer neu braucht

- `gpudev`: Registrierung, Befehlsring, Pufferverwaltung, Fences als
  Weckgriffe (`CORES_AND_EVENTS.md`, `npk_wait`).
- Eine Kapabilitaet `Rights::GPU` (Karte binden + gpudev) — getrennt von
  `NET`, damit ein GPU-Modul kein Netz bekommt und umgekehrt.
- Ein Weg, Kernelpuffer **in den Gast** abzubilden (§4.3, NPT/EPT).

---

## 4. Nutzer A: microVM ueber DRM native context

### 4.1 Was native context ist

virtio-gpu mit dem Capset `DRM` (Nr. 6): der Gast spricht ueber virtio die
**Kernel-Schnittstelle der echten Karte** (bei Intel die i915-ioctls
`GEM_CREATE`, `EXECBUFFER2`, …; bei AMD die amdgpu-ioctls). Der Wirt
uebersetzt sie auf seinen Treiber. Shader-Compiler und Befehlsaufbau bleiben im
Gast-Mesa. Im Gegensatz dazu: **virgl** (OpenGL-Strom, Wirt braucht OpenGL,
seit Mesa 26.1 „unmaintained") und **Venus** (Vulkan-Strom, Wirt braucht ein
Vulkan, also ein Mesa bei uns). Beide scheiden aus.

### 4.2 Stand, recherchiert 2026-10-12

Anforderungen laut QEMU-Dokumentation:

| Karte | Wirt-Kernel | virglrenderer | Gast-Kernel | Gast-Mesa |
|---|---|---|---|---|
| AMDGPU | 6.13+ | 1.1.0+ | 6.14+ | 25.0.0+ |
| Intel i915 | 6.13+ | 1.3.0+ | 6.14+ | 26.1.0+ |

Unser Gast: **Linux 6.18.53, Mesa 26.2.4** (Alpine 3.25-alpha,
`microvm-userspace/packages.txt`) — beide Versionen reichen.

**Der Schalter entscheidet, nicht die Version.** Beide Backends sind in Mesa
hinter einer Bauoption, Vorgabe `false`:

| Option | Alpines Mesa (`main/mesa/APKBUILD`) |
|---|---|
| `-Damdgpu-virtio=true` | **an** |
| `-Dintel-virtio-experimental=true` | **aus** |

Folge: **fuer AMD laeuft der Gast mit dem Mesa, das Alpine liefert. Fuer
Intel muessen wir Mesa selbst bauen** (oder ein eigenes apk), und die Option
heisst nicht umsonst „experimental". Native context fuer den neueren
Intel-Treiber `xe` gibt es nur als Fremdpatch — fuer TGL egal, i915 ist dort
der Linux-Treiber.

### 4.3 Was der Wirt dafuer bauen muss

Die Wirtsseite in virglrenderer ist **klein**, gezaehlt am `main`-Zweig:

| Teil | Zeilen |
|---|---|
| gemeinsam (`drm_context.c`, `drm_fence.c`, `drm_renderer.c`, `drm_util.c`) | 894 |
| AMD (`amdgpu_renderer.c` + Protokoll) | 1583 |
| Intel (`i915_ccmd.c`, `i915_object.c`, `i915_renderer.c`, `i915_resource.c` + Protokoll) | 1261 |

Das ist das Uebersetzungsstueck; darunter liegt die eigentliche Arbeit, die
Semantik der Kernel-ioctls (GEM, Kontexte, Execbuffer mit festen GPU-Adressen,
Fences) — und die liefert das Modul ueber gpudev. Die Intel-Befehle sind
ueberschaubar: `GETPARAM`, `QUERYPARAM`, `GEM_CREATE(_EXT)`,
`GEM_CONTEXT_CREATE`, `GEM_EXECBUFFER2(_V2)`, `GEM_SET_MMAP_MODE`, `GEM_BUSY`
und `IOCTL_SIMPLE` (ein Durchreicher — der braucht eine **Positivliste**).

An `virtio_gpu_pci.rs` fehlt:

- Features `VIRTIO_GPU_F_CONTEXT_INIT`, `RESOURCE_BLOB`, `VIRGL` (fuer die
  Capset-Abfrage), Capset `DRM` mit dem Untertyp der Karte.
- `CTX_CREATE`/`CTX_DESTROY`, `SUBMIT_3D` (traegt die ccmds),
  `RESOURCE_CREATE_BLOB`, `RESOURCE_MAP_BLOB`/`UNMAP_BLOB`, Fences je Ring.
- **Die hostmem-Region**: eine Shared-Memory-Capability am PCI-Geraet, in die
  der Wirt Puffer **direkt** in den Gast-Adressraum legt (NPT/EPT-Eintraege
  auf Kernelseiten). Ohne sie gibt es kein `mmap` eines GPU-Puffers im Gast.
- Gast-Kernel: `CONFIG_DRM_VIRTIO_GPU` ist an; die Zeile
  `# CONFIG_DRM_NATIVE_CONTEXT_* — TBD` in `nopeek-tiny.config` ist zu
  klaeren (der Gast braucht nach Linux-Quelle kein eigenes Symbol, nur blob +
  context init im virtio-gpu-Treiber — pruefen statt glauben).

### 4.4 Die Gast-Bilder ohne Kopie

Rendert der Gast in einen Blob, ist das Bild **ein Kernelpuffer**. Der
Compositor setzt ihn direkt zusammen (GPU) oder legt ihn auf eine Ebene. Die
drei CPU-Kopien aus §1 fallen weg. Im CPU-Rueckfall bleibt `surface.rs`, wie
es ist.

**Entscheid 2026-10-14: zuerst Guest-Blobs, ohne Zwischenschritt ueber die
Kopier-Engine.** Gemessen am NUC (Kernel 0.608.2, LibreWolf in 4K): der Gast
schickt 58 TRANSFER/s, `xfer-KB` ~870 MB/s, die der GPU-Worker auf Kern 0
mit der CPU kopiert und Zeile fuer Zeile vergleicht; die Kopie in die
Gast-Flaeche ist seit 0.608.2 auf den Schaden beschraenkt (115 von 460 MB/s).
Der Gast-Treiber (6.18.55, nachgelesen) macht den Rest selbst, sobald der
Wirt `VIRTIO_GPU_F_RESOURCE_BLOB` anbietet (ohne virgl):

- `virtgpu_gem.c:84`: Dumb-Puffer werden `VIRTGPU_BLOB_MEM_GUEST` mit
  `USE_SHAREABLE` — Bildpuffer im Gastspeicher, am Gast nichts zu aendern.
- `virtgpu_plane.c`: fuer Guest-Blobs `SET_SCANOUT_BLOB` statt `SET_SCANOUT`;
  TRANSFER_TO_HOST_2D kommt weiter (`update_dumb_bo`) und wird beim Wirt fuer
  Blobs zum Nichts; `RESOURCE_FLUSH` traegt einen Zaun, auf den der Gast bis
  50 ms wartet, bevor er den Puffer wieder beschreibt (`:197ff`, `:368`).
- QEMU (`blob=true`) und crosvm machen es so: Gastseiten per udmabuf als
  GPU-Puffer, gezeichnet ohne Kopie.

Bauplan (Stufe 6a, Baustein fuer native context):
1. virtio-gpu: `F_RESOURCE_BLOB` anbieten **nur wenn gpudev Szenen anbietet**
   (sonst bleibt der Gast beim 2D-Weg), `RESOURCE_CREATE_BLOB`
   (`BLOB_MEM_GUEST`), `SET_SCANOUT_BLOB`, TRANSFER fuer Blobs leer.
2. Die Seiten des Blobs als gpudev-Puffer (Seitenliste ueber die
   Gast-Speicherabbildung); die Gast-Flaeche nennt diesen Puffer als Quelle
   (`Source::Surface` → Blob statt Kopie).
3. Den Flush-Zaun erst melden, wenn die Szene, die den Blob liest, fertig ist.
4. Rueckfall, wenn das Modul waehrend der Sitzung stirbt: die CPU liest den
   Schaden aus den Gastseiten in die Flaeche (heutiger `write_frame`-Weg ohne
   Zwischenpuffer und ohne Zeilenvergleich).
5. Lebensdauer: Blob-Puffer werden vor dem Freigeben des VM-Speichers bei
   gpudev abgemeldet und ihre Zaeune abgewartet.

**Gebaut (Kernel 0.609.0), NUC-Test offen.** Alle fuenf Punkte, das Modul
bleibt unveraendert (es bindet schon Seite fuer Seite aus `bo_pages`):

- `shade::buffers::ForeignPixels`/`ForeignView`: Pixel in fremden Rahmen
  (Host-Seitenliste aus `GuestMem::host_page`), mit Lesezaehler.
- gpudev: `bo_for_pages` / `forget_pages` / `pages_idle` /
  `pages_released`, Schluessel ist die Seriennummer statt der Adresse.
- `scene::Sources` hat `in_place` (die GPU liest die Quelle, wo sie liegt)
  und `with_source_keyed` (damit die Verifikation dasselbe Bild auf beiden
  Wegen erkennt). Die Flaeche haelt ein Blob-Bild ohne Kopie; die CPU
  kopiert nur, wenn sie selbst zusammensetzt, und nur den Schaden seit
  ihrer letzten Kopie (dieselbe Historie wie 0.608.2).
- Zaun: der FLUSH-Abschluss wird gehalten, bis der vorher gezeigte Blob
  weder von der CPU noch von einem GPU-Auftrag noch von einer gerade
  kodierten Szene gelesen wird (hoechstens 40 ms; der Gast wartet 50).
  Gewartet wird im GPU-Arbeiter ausserhalb des Geraete-Locks; ohne
  Arbeiter (INTx) wird nicht gehalten. `cores` zeigt `blob` und `hold-us`.
- UNREF/DETACH/Reset/VM-Ende: ein noch gezeigtes Bild wird Kopie
  (`surface::detach_blob`), gpudev bindet ab, erst dann Antwort bzw.
  Freigabe; bindet gpudev nicht rechtzeitig ab, bleibt der VM-Speicher
  stehen (`stop_worker` → false).
- Angeboten nur, wenn gpudev Szenen anbietet und `gpu_scene` nicht auf
  die CPU zurueckgefallen ist; `set microvm_blob off` erzwingt den
  2D-Weg.

**Der Gast musste doch angefasst werden:** `virtio_gpu_plane_atomic_check`
verwirft die Schadensrechtecke bei jedem Pufferwechsel („uploads are done
per-buffer"), und wlroots wechselt jedes Bild. Ohne den Zeilenvergleich
des 2D-Wegs saehe der Wirt also jedes Gastbild als ganz neu und setzte das
ganze Fenster neu zusammen. Fuer Guest-Blobs gilt die Begruendung nicht —
der Wirt liest den neuen Puffer, wo er liegt, wlroots' Schaden gegen das
vorige Bild stimmt genau. `microvm-linux/patches/virtio-blob-damage.patch`
laesst die Rechtecke fuer Guest-Blobs stehen; `microvm-linux/build.sh`
wendet `patches/*.patch` einmal auf den Quellbaum an.

---

## 5. Nutzer B: der Desktop

### 5.1 Voraussetzung: Fenster als Puffer

Der Compositor hat heute bewusst **keine Puffer je Fenster** — er rendert in
den Schattenpuffer. Fuer GPU-Zusammensetzen und Ebenen braucht jedes Fenster
seinen Puffer. Kosten: die Fenster kacheln, ihre Summe ist ungefaehr eine
Bildschirmflaeche (4K: ~33 MB) plus Dock/Bar/Glas. Das ist der erste Umbau,
und er hilft auch dem CPU-Weg (ein unveraendertes Fenster wird nicht neu
gerastert).

### 5.2 In welcher Reihenfolge die GPU uebernimmt

Ziel: die CPU beschreibt je Bild nur, was gezeichnet wird; auch in 4K und
waehrend Animationen fasst sie keine Pixel des Bildes an. Eine Animation
(Einblenden, Skalieren, Verschieben) ist dann eine andere Zahl in der
Szenenliste.

1. **Display-Ebenen.** Intel (Universal Planes je Pipe) und AMD (DCN: HUBP +
   MPC) ueberlagern mehrere Ebenen mit Alpha in Hardware. Zeiger auf eine
   eigene Ebene (heute „save-under"), ein Video oder ein Vollbild-VM-Fenster
   ohne Kopie. **Kein Shader noetig.**
2. **Kopier-Engine** (BCS / SDMA) fuer Fensterpuffer → Bild. Am NUC gibt es
   das schon.
3. **Feste Shader** fuer Textur zeichnen, Blenden, abgerundete Ecken (SDF),
   Schatten, Glas-Blur, Skalieren — **eine Handvoll, beim BAUEN uebersetzt**
   mit Mesas Compiler fuer die jeweilige GPU-Familie (Intel: brw fuer Gen12)
   und als Binaerdaten ins Modul gelegt; dasselbe Prinzip wie `gen_regs.py`,
   das GCC die Registerwerte rechnen laesst. **Kein Shader-Compiler zur
   Laufzeit, kein Mesa auf dem Wirt.** Vorbild: Xorgs SNA-Beschleunigung im
   alten Intel-Treiber, die genau so mit festen GPU-Programmen
   zusammengesetzt hat. Das Ergebnis landet in einem Puffer, der per **Flip**
   zum Bild wird — keine Kopie nach GGTT 0 mehr.
4. **Text** bleibt auf der CPU (Glyphen-Atlas), bis es gemessen einen Grund
   gibt.

### 5.3 Die Szenenliste

Je Bild baut der Compositor eine Liste: Vierecke mit Quellpuffer (Griff),
Zielrechteck, Deckkraft, Eckenradius, Transformation, Blur-Bereich, Farbe.
Ein paar hundert Bytes statt 33 MB. Das Modul prueft jeden Eintrag (Griff
gueltig und fuer diesen Kontext freigegeben, Rechteck im Puffer) und uebersetzt
ihn in Befehle fuer die eigenen Shader. Die Liste ist Daten, nie Code.

Ausgangspunkt ist der vorhandene `Rasterizer`-Trait: eine GPU-Umsetzung
schreibt die Aufrufe eines Bildes in einen Befehlspuffer und gibt ihn mit
einem `submit` ab — **ein Hostaufruf je Bild, nicht je Rechteck**. Was der
Trait heute nicht kennt (Puffer zusammensetzen, Blur, Ebenen), kommt dazu,
jeweils mit CPU-Umsetzung.

### 5.4 Plan fuer A (2026-10-14, Florian: Desktop zuerst)

**Ist (ausgezaehlt):** alles Zusammensetzen laeuft auf der CPU in EINEN
Schattenpuffer; ein volles Bild kopiert dafuer `LAYER_BG` (33 MB) und
setzt jedes Fenster mit `render_window` in Bildschirmkoordinaten neu
zusammen (`shade/mod.rs:504`, `compositor.rs:1923`). Eigene Puffer haben
nur Widget-Szenen (`WidgetScene.pixels`) und Gast-Flaechen (`surface.rs`).
Teilwege (Terminalzeile, Fokus-Glow, Zeiger) schreiben direkt in den
VORDEREN Puffer. Glas ist eine Heuristik (`glass_base_at`: unscharf nur, wo
die Tapete sichtbar ist). `LAYER_CHROME`/`LAYER_TEXT` sind tot (63 MB).
Animationen (Dock, Tausch) aendern jedes Bild die Fenstergroesse und
erzwingen ein komplettes Neu-Rastern der Widget-Szenen. Die GPU kopiert
heute nur das fertige Bild (Stufe 1b).

**Ziel:** jedes Fenster und Panel lebt in seinem eigenen Puffer; ein Bild
ist eine Szenenliste (§5.3); die GPU setzt sie zusammen, direkt in einen
von zwei Scanout-Puffern, und die Anzeige wechselt per Flip im vblank. Der
Zeiger liegt auf der Cursor-Plane. Die CPU rastert nur noch, was sich in
einem Fenster wirklich geaendert hat.

| Schritt | Inhalt | Ergebnis am Geraet |
|---|---|---|
| **A0** | Aufraeumen ohne Verhaltensaenderung: tote Layer weg (63 MB); Widget-Szenen in Fensterkoordinaten rastern (Verschieben rastert nicht mehr neu, Treffertests mit Versatz) | gleiches Bild, weniger Speicher, Dock-Animation ohne Neu-Rastern |
| **A1** | Fenster als Puffer, **CPU-Weg zuerst**: Terminal rendert in seinen Puffer; alles, was heute in Luecken malt (Glow, Ecken, Dock-Griff, Blitz) wird ein eigener Szeneneintrag; Teilwege werden „Fensterpuffer aendern + Schaden melden“ | gleiches Bild; Bildzeit gemessen vorher/nachher |
| **A2** | Die Szenenliste als Datenstruktur mit **CPU-Ausfuehrer** (aus dem heutigen Code); Zusammensetzen nur im Schadensbereich; Glas tastet ab, was WIRKLICH darunter liegt | CPU-Referenz fuer A5; Glas auch ueber Fenstern richtig |
| **A3** (gebaut, Kernel 0.607.0) | gpudev: Puffer dynamisch anmelden/abmelden (Fenster, Tapete, Unschaerfe, Scanout x2), Seitenlisten statt zusammenhaengend, `OP_SCENE`, Fences je Puffer statt `wait_inflight` | Kernel und Modul tauschen Szenen aus |
| **A4** | Shader-Werkzeug (F8): Bauwerkzeug gegen Mesas `brw_compile_cs` (GLSL-Compute → Gen12-Binaerdaten + Parameterbeschreibung, wie `gen_regs.py` erzeugt) und der GPGPU-Start im Modul (Vorlagen: IGT `gpgpu_fill.c`/`gpu_cmds.c` Gen12, Mesa `genX_simple_shader.c`); Probe: Fuellen + Readback | erstes eigenes GPU-Programm laeuft am NUC |
| **A5** (gebaut, Modul 0.11.0) | Die festen Programme: Viereck mit Textur/Deckkraft/Eckenmaske, Unschaerfe (verkleinern + Box), Glow-Ring, Fuellen; GPU-Ausfuehrer der Szenenliste; Pixelvergleich gegen den CPU-Ausfuehrer als Selbsttest | Desktop setzt auf der GPU zusammen; CPU-Last bei Animation in 4K gemessen |
| **A6** | Ebenen: Cursor-Plane (save-under faellt weg, das Stocken mit), Flip zwischen zwei Scanout-Puffern im vblank statt Kopie (PLANE_SURF wie `skl_fixup_initial_plane_config`, Ebenen-Register aus `skl_universal_plane.c`) | Zeiger ruckelfrei, kein Kopieren mehr |

**Reihenfolge, weil:** A0–A2 sind reine CPU-Umbauten mit gleichem Bild und
lassen sich einzeln pruefen — ein Fehler dort ist ein falsches Bild, nicht
ein GPU-Haenger. A4 ist die groesste Unbekannte (F8) und kann parallel zu
A1/A2 laufen. A6 haengt nur an A3 und koennte vorgezogen werden, wenn der
Zeiger draengt.

**Bewusste Entscheidungen:**
- **Compute statt 3D-Pipeline:** GPGPU braucht keinen Vertex-/Raster-
  Zustand; Zusammensetzen ist je Pixel eine Schleife ueber die
  Szeneneintraege, die ihn bedecken. Die 3D-Pipeline bleibt der Gast-Welt.
- **Animationen skalieren, statt neu zu rastern:** waehrend einer Dock- oder
  Tausch-Animation wird der alte Fensterpuffer gestreckt oder beschnitten;
  neu gerastert wird erst am Ende.
- **Text bleibt auf der CPU** (`font.rs`, fontdue), in den Fensterpuffer.
- **Alles Zusammensetzen bleibt auf Kern 0** (Invariante heute); Rastern der
  Widgets laeuft wie bisher auf den App-Kernen.
- **Der CPU-Ausfuehrer bleibt fuer immer:** ohne Modul (§2.2) setzt er
  dieselbe Szenenliste zusammen.

**A1/A2, gebaut (Kernel 0.606.0) — die Spezifikation, an die A3/A5 anschliessen:**
- **Puffer** (`shade/buffers.rs`, `PixelBuffer`): seitenausgerichtete,
  physisch zusammenhaengende Rahmen aus dem Rahmenallokator, dichte Zeilen
  (`pitch = width * 4`), 32 Bit je Pixel; der Kernel zeichnet ueber die
  Identitaetsabbildung, A3 meldet denselben Bereich bei gpudev an.
  Widget-Szenen (`WidgetScene.pixels`) und die Textebene jedes Terminals
  sind solche Puffer; Gast-Flaechen (Front-Rahmen) sowie scharfe und
  unscharfe Tapete sind bereits zusammenhaengend und werden als Quelle
  ueber ihren Besitzer benannt.
- **Szenenliste** (`shade/scene.rs`): reine Daten, feste Groesse je Eintrag,
  Quellen ueber den besitzenden Fensterschluessel (`Source::Widget/Surface/
  TermText`). Eintraege: `Background`, `Glow`, `GlassBackdrop`, `Chrome`,
  `Content` (Ecken-SDF, `Fit::Scale` fuer Animationen, `Fit::Extend` fuer
  Gastbilder), `Panel` (Alpha, Glas ueber der Unschaerfe dessen, was
  darunter liegt), `Text` (Alpha-Ebene), `Fill`, `CloseGlyph`. Der Zeiger
  ist nicht Teil der Szene (A6: Cursor-Plane).
- **CPU-Ausfuehrer** (`scene::execute`): malt die Liste innerhalb von
  Schadensrechtecken in ein bildschirmgrosses Ziel; Schaden, der ein Panel
  beruehrt, waechst auf Panel plus Unschaerferand (`expand_damage`). Der
  GPU-Ausfuehrer (A5) muss dasselbe Bild liefern.
- **Wege:** volles Bild = Szene ueber den ganzen Schirm in den hinteren
  Puffer; Terminalausgabe, Eingabezeile, Fokus-Glow und `render_damaged`
  aktualisieren den Puffer des Fensters und setzen nur ihren Schaden im
  vorderen Puffer neu zusammen. `LAYER_BG`, der Chrome-Cache und der
  Eingabezeilen-Cache sind entfallen.
- **Bewegung:** waehrend Dock-/Tausch-Animationen und Groessen-Zug wird der
  alte Puffer gestreckt (Widgets) bzw. beschnitten (Terminal), neu
  gerastert erst am Ende.

**A3/A5, gebaut (Kernel 0.607.0, gpu_intel_tgl 0.11.0)** — ABI v1 in §3.3.
Kernel: `gpu/gpudev.rs` (Griffe, Seitenlisten ueber `paging::translate`,
Zaeune je Puffer, verzoegerte Freigabe), `shade/gpu_scene.rs` (Kodierung,
Vergleich am ersten ganzen Bild, Rueckfall, `gpu verify` / `gpu cpu`);
Gast-Fronts sind jetzt `PixelBuffer`. Modul: `compose.rs` (Pruefung, Segmente
an jedem Paneel, Unschaerfe als `blur_down` + 6 × `blur_box`, alle Aufrufe
eines Auftrags in einem Stapel auf rcs0), Programme `compose.comp`,
`blur_down.comp`, `blur_box.comp`; dynamische Puffer liegen im PPGTT ueber
4 GiB, Abmelden ist `gen8_ppgtt_clear`. Ein GPU-Modul darf 128 MB DMA halten
(Seitentabellen je gebundenem Puffer). Am Host gegen den CPU-Ausfuehrer
gepruefte Shader-Logik (C++-Uebersetzung derselben Quellen, 2300 Zufallsszenen,
0 Abweichungen) — was brw daraus macht, zeigt erst der Vergleich am NUC.

**Gelernt am NUC (2026-10-14):** Auf die GPU wird **nie unter dem
Compositor-Lock** gewartet. Dock und Bar fragen den Compositor nach jedem
Bild ab und warten aktiv auf seinen Lock; teilen sie den Kern mit dem
GPU-Modul, kommt das Modul nicht dran, und die Szene laeuft ins Zeitlimit
(122 ms je Bild bei 17 ms GPU-Arbeit). Deshalb gibt `gpu_scene::compose` nur
ab, und `gpu_scene::finish` wartet, nachdem der Aufrufer den Lock
losgelassen hat. Und das Dock ist ein Overlay (`docs/archive/DOCK.md` §3):
die Kacheln werden einmal je Gleiten neu gelegt, nicht je Schritt, und ein
Schritt zeichnet nur den Streifen des Docks neu.

**Stand 2026-10-14 abends (Kernel 0.608.2, gpu_intel_tgl 0.14.0, NUC ✅):**
3516 Szenen ohne Fehlschlag; Szenen im Schnitt 0,3–2 ms, ganze 4K-Bilder
7–9 ms. Was den Weg dahin bestimmt hat:
- Die Szenenausfuehrung: Itemdaten einmal je SIMD16-Thread laden (vorher je
  Lane und Feld: 95 Speichernachrichten), 64x64-Kacheln mit Verdeckung (auch
  ueber Paneel-Grenzen, ausser im Lesebereich der Unschaerfe), ein Thread =
  16x16 px, Kopierpfad fuer Kacheln mit einem deckenden Eintrag, Terminal-Glas
  im Geradzeilen-Bereich deckend mit eigenem Schnellweg. GPU-Zeitstempel je
  Aufruf im Log (`gpudev: scene …`, erste 3 und alle ueber 16 ms).
- Gastbilder setzen nur ihren Schaden zusammen (vorher je Gastbild der ganze
  Schirm) und kopieren nur ihren Schaden in die Flaeche (`write_frame` mit
  Schadens-Historie je Puffer).
- Offen: fairer Vergleich ganzes Bild GPU gegen CPU mit Fenstern (`gpu cpu`);
  Dock-Gleiten ohne Zeitbasis (geht jetzt praktisch sofort); Groesse beim
  Ziehen erst beim Loslassen; A6.

**Bekannte Fallen:** Overlays verlassen sich auf Pixel unter ihren Ecken
(`compositor.rs:1938`); `render_damaged` zeichnet Fenster darueber nicht neu
(`compositor.rs:2340`, heute nur fuer Overlays geflickt); `virtio_gpu_pci.rs:1456`
schreibt am Compositor vorbei in den vorderen Puffer; Speicher: neun
Bildschirmpuffer heute (≈285 MB), mit A0 sieben, danach je Fenster sein
Inhalt statt eines geteilten Puffers.

---

## 6. Nutzer C: Apps

Keine Aenderung der App-ABI. Spaeter denkbar: WebGL/WebGPU in beak ueber
dieselbe Leitung wie die microVM — eigenes Papier, wenn es so weit ist.

---

## 7. Die Karten

### 7.1 `gpu_intel_tgl` — Tiger Lake, i915-Weg

Linux-Quelle 1:1, der Execlist-Weg von i915 (nicht `xe`, der verlangt GuC):

- `intel_uncore` (ForceWake), `intel_ggtt`, **`gen8_ppgtt`** (Adressraum je
  Kontext — Voraussetzung fuer alles, was der Gast abgibt)
- `intel_execlists_submission`, `intel_lrc`, `intel_engine_cs` fuer RCS,
  BCS, VCS/VECS — **TGL hat keine CCS** (`tgl_info`), Compute laeuft ueber
  die GPGPU-Pipeline der RCS
- `intel_gt_irq` — Fences und Kontextwechsel per Interrupt statt Abfrage
- `skl_universal_plane`, `intel_display` soweit fuer Ebenen und Flip noetig
- Engine-Reset bei Haengern (`intel_reset`) — ein Gast kann die Karte
  aufhaengen, also muss das Modul sie wieder freibekommen

Vorlage im Haus: `intel_xe.rs` hat GGTT, ForceWake, Execlists und LRC fuer den
BCS schon laufen; das Modul uebernimmt das Wissen, nicht den Code.

### 7.2 `gpu_amd` — Lucienne, amdgpu-Weg

Zwei Teile, die getrennt nuetzen:

- **Stufe 0, ohne Firmware-Laden:** das SMU-Postfach (SMU 12, die Firmware
  laeuft auf der APU schon) fuer Stromzustaende — VCN/JPEG/SDMA abschalten,
  GFXOFF — und die Hintergrundbeleuchtung ueber das Panel-PWM von DCN 2.1.
  Beide haengen an offenen Faeden (`_BCM` erwartet amdgpu; Package 3,9 W im
  Leerlauf). Ob dort Watt zu holen sind, ist **nicht** gemessen.
- **Das Fundament:** PSP-Ring, `renoir_*`-Firmware ueber den PSP (asd, ta,
  pfp, me, ce, mec, rlc, sdma, dmcub, vcn), GMC/GART, IH, RLC, dann SDMA
  (Gegenstueck zum BCS), GFX, GPUVM je Kontext. Deutlich mehr als bei Intel.

Die Firmware-Dateien kommen aus linux-firmware, signiert mit unserem Modul
ausgeliefert; ihr Hash steht im Manifest wie jedes andere Asset.

### 7.3 Andere Karten

Bekommen kein Modul und laufen auf dem CPU-Rueckfall — genau wie heute. Der
Gast faellt dann auf llvmpipe zurueck (virtio-gpu bietet das Capset `DRM`
nicht an).

---

## 8. Sicherheit

**Checkpoint-Frage: kann ein WASM-Modul — oder jetzt auch ein Gast — ueber
diesen Weg aus seiner Sandbox?** Ehrliche Antwort fuer den Entwurf: **eine
GPU ist eine DMA-Maschine, die Fremde programmieren**. Die Grenzen:

1. **Gast gegen Wirt: der GPU-Adressraum je Kontext.** Ein Befehlspuffer des
   Gasts sieht nur, was in seinem PPGTT/GPUVM steht. Kein Gast-Kontext
   bekommt je die GGTT. Deshalb kommt der eigene Adressraum **vor** der ersten
   Abgabe eines Gasts — nicht danach.
2. **Gast gegen Hardware-Register:** Befehlspuffer laufen unprivilegiert;
   auf Gen12 verweigert die Hardware Registerschreibzugriffe ausserhalb ihrer
   Positivliste (i915 verlaesst sich darauf, einen Befehlsparser gibt es auf
   Gen12 nicht). Bei AMD dasselbe ueber die Ring-Privilegien. Zu pruefen,
   welche Register die Positivliste je Engine freigibt (F4).
3. **`IOCTL_SIMPLE` und alle Durchreicher:** nur eine feste Positivliste.
4. **Modul gegen Kernel:** die WASM-Sandbox schuetzt den Kernelspeicher vor
   dem Modul-Code, **nicht** vor dem, was das Modul die Karte tun laesst. Das
   gilt fuer den WLAN-Treiber heute genauso. Die echte Grenze ist die
   **IOMMU** (VT-d am NUC, AMD-Vi am IdeaPad): Karte nur auf die Seiten, die
   der Kernel freigegeben hat. Wir lesen DMAR/IVRS, programmieren sie aber
   nicht. **Bis dahin ist das GPU-Modul so vertrauenswuerdig wie der
   Kernel** — dieselbe Lage wie bei jedem Treibermodul mit DMA. Das gehoert
   benannt in die `.npk.caps` und in den Release-Text.
5. **Haenger:** ein Gast-Shader kann die Engine blockieren. Zeitlimit je
   Abgabe, Engine-Reset, betroffener Kontext wird verworfen, der Compositor
   faellt fuer dieses Bild auf die CPU.

### 8a. Stufe 2 gebaut: VT-d fuer die Grafik (Kernel 0.611.0)

`kernel/src/drivers/vtd.rs`, nach Linux `drivers/iommu/intel/` im
Legacy-Modus mit Register-Invalidierung (`iommu_set_root_entry`,
`__iommu_flush_context`, `__iommu_flush_iotlb`, `iommu_flush_write_buffer`,
`iommu_disable_protect_mem_regions`, `iommu_enable_translation`,
`dmar_fault`).

- Nur die Einheit, deren Geltungsbereich **allein** die Karte ist (bei Intel
  hat die Grafik eine eigene DRHD) — sonst wird nichts eingeschaltet und das
  Log sagt warum. Andere Geraete bleiben unberuehrt.
- Zweitstufen-Tabelle **1:1** (IOVA = physische Adresse) nur fuer
  freigegebene Seiten: RMRR der Karte, Stolen Memory (BDSM 0xC0/0xC4, GGC
  0x50 wie Linux `intel_early_quirks`, GSM direkt darunter), die DMA-Puffer
  des Moduls (`npk_dma_alloc*`), die gpudev-Puffer samt Gast-Blobs. Das Modul
  aendert sich nicht; jede andere Adresse ist ein DMAR-Fehler.
- Eingeschaltet in `npk_pci_enable_bus_master` des Anzeige-Moduls, vor dem
  ersten DMA. **Nie wieder ausgeschaltet** (Linux
  `quirk_igfx_skip_te_disable` fuer Gen12-Grafik).
- gpudev gibt beim Anmelden eines Puffers frei und nimmt zurueck, wenn das
  Modul die Abmeldung abgeschlossen hat — unter dem gpudev-Lock, damit
  `pages_released` (VM-Speicher) nie vorher wahr wird. Modultod: alles
  zurueck.
- Modulende: die DMA-Puffer der Anzeige gehen jetzt **zurueck an den
  Allocator** (vorher bis zum Neustart behalten, weil die Karte sie noch
  erreichen konnte).
- Fehler werden abgefragt (je 64 Auftraege, bei Fehlschlag, `gpu iommu`),
  nicht per Interrupt gemeldet.
- Schalter `set gpu_iommu on` (Vorgabe aus, bis am NUC bewiesen), wirkt beim
  naechsten Start des Moduls. `gpu iommu` zeigt Stand und Fehler,
  `gpu iommu test` ist die Gegenprobe (Puffer entziehen, die GPU liest ihn:
  Muster darf nicht ankommen, Lesefehler an genau der Adresse).

Offen: AMD-Vi (IdeaPad); die Einheit fuer alle uebrigen Geraete
(INCLUDE_PCI_ALL) — dafuer braeuchten NVMe, Netz und USB eigene Freigaben.

### 8b. Stufe 6 gebaut: native context im virtio-gpu (Kernel 0.612.0)

- `microvm/devices/virtio_gpu_ctx.rs`: der allgemeine Teil von
  virglrenderer `drm_context.c` — Kontexte, ccmd-Strom aus `SUBMIT_3D`
  (Laengen- und Ausrichtungspruefung je Befehl), Antwort in den
  Antwortpuffer des Gastes mit festem `rsp_mem_offset` (nie aus dem
  gast-beschreibbaren Puffer zurueckgelesen) und der Laenge, die der Gast
  im Fach hinterlassen hat (einmal gelesen, gegen TOCTOU), Seqno nach
  jedem Befehl; HOST3D-Blobs (Blob-ID 0 = Antwortpuffer, sonst ein Objekt
  des Backends). Die i915-Befehle selbst gehen an `trait Backend` — das
  baut Stufe 7.
- `virtio_gpu_pci.rs`: Merkmale `VIRGL` + `CONTEXT_INIT` (der Gasttreiber
  legt Kontexte nur mit `VIRGL` an; seine Dumb-Puffer gehen dann wieder den
  2D-Weg), Capset `DRM` (Nr. 6), `CTX_CREATE/DESTROY/ATTACH/DETACH`,
  `SUBMIT_3D` mit Ring-Index und Zaun (Abschluss erst, wenn das Backend den
  Zaun meldet; der GPU-Arbeiter schaut bei offenen Zaeunen wieder nach),
  `RESOURCE_MAP_BLOB/UNMAP_BLOB`, BAR2 (64-bit, 1 GiB bei 4 GiB) mit
  Shared-Memory-Capability (`SHM_ID_HOST_VISIBLE`).
- `microvm/devices/hostmem.rs` + `ept.rs`/`npt.rs`: Host-Seiten im
  Fenster ein- und ausblenden; Ausblenden kehrt erst zurueck, wenn keine
  vCPU die alte Uebersetzung mehr nutzen kann (Generationszaehler, Kick,
  INVEPT bzw. TLB-Flush vor dem naechsten Eintritt — KVMs
  `KVM_REQ_TLB_FLUSH`). Nicht abgebildete Fensterseiten lesen 0.
- Angeboten nur, wenn ein Backend registriert ist (Stufe 7) oder mit
  `set microvm_gpu test`: dann ein Pruef-Backend mit dem Capset der Karte
  an 00:02.0, das jeden ccmd mit „kein Geraet" beantwortet. Erwartet im
  Log: `[gpu] guest features … (… virgl on, context init on)` und
  `GET_CAPSET drm`. Alpines Mesa hat kein Intel-virtio, faellt also auf
  llvmpipe — das ist zugleich die Probe fuer den Rueckfall.
- Gastpatch: Schadensrechtecke bleiben auch fuer HOST3D-Blobs stehen.

---

## 9. Stufen

Jede Stufe hat ein Ergebnis, das man am Geraet sieht oder misst.

| Stufe | Inhalt | Geraet | Ergebnis |
|---|---|---|---|
| **0a** | `gpu_amd` Stufe 0: SMU-Postfach + Panel-PWM | IdeaPad | Helligkeit per Fn; Watt vorher/nachher mit `power` |
| **0b** | `gpu`-Ausgabe ohne Treiber korrigieren | beide | kleine Aufraeumarbeit |
| **1a** ✅ | `gpu_intel_tgl` faehrt die Karte nach Linux hoch: Display-Auslesung, Uebernahme des Firmware-Bildes, GGTT, GT, Workarounds, RC6/RPS, PPGTT, alle Engines mit Execlists, MSI (0.2.0–0.8.0) | NUC | 2026-10-14: `default context recorded` auf rcs0/bcs0/vcs2/vecs0, Bild bleibt |
| **1b** | gpudev-Kern (§3.3: Griffe, Abgabe, Fences, Kontexte) + erster Nutzer Bildkopie per bcs0 + Autostart nach dem Login + Readback-Selbsttest; danach **`intel_xe.rs` raus** | NUC | gleiche Bildzeiten wie heute; Hostaufrufe je Bild gezaehlt (forge-Kosten) |
| **2** | IOMMU (VT-d) fuer die Karte — Voraussetzung fuer fremde Befehlspuffer (F5). **Gebaut (Kernel 0.611.0), NUC-Test offen** — §8a | NUC | Karte erreicht nur freigegebene Seiten; Gegenprobe mit absichtlich falscher Adresse (`gpu iommu test`) |
| **3–5** | Desktop auf der GPU, in Schritten A0–A6 (§5.4): Fenster als Puffer, Szenenliste mit CPU- und GPU-Ausfuehrer, feste Shader, Cursor-Plane, Flip | NUC | `comp`-Zeit je 4K-Bild vorher/nachher, CPU-Last bei Animation |
| **6** | virtio-gpu: blob, context init, hostmem, Capset `DRM`. **Gebaut (Kernel 0.612.0), Test offen** — §8b | — | Gast sieht das Capset (`set microvm_gpu test`) |
| **7** | native context Intel (Wirtsseite i915-Protokoll) + Gast-Mesa mit `intel-virtio-experimental` | NUC | `glxinfo` im Gast nennt iris; WebGL in LibreWolf |
| **8** | `gpu_amd` Fundament: PSP, Firmware, SDMA, GFX, GPUVM | IdeaPad | SDMA-Selbsttest gruen |
| **9** | native context AMD (Gast-Mesa von Alpine) | IdeaPad | `glxinfo` im Gast nennt radeonsi |

Stufe 5 und die Stufen 6–7 (microVM) haengen nicht voneinander ab; beide
brauchen 1b, die microVM zusaetzlich 2. Welche zuerst kommt, entscheidet,
was die CPU staerker belastet.

Zu Stufe 7 gegen 9: fuer die microVM ist AMD auf der GAST-Seite billiger
(Alpine liefert das Backend), Intel auf der WIRT-Seite (der Treiber laeuft
schon halb, keine Firmware). Die Reihenfolge folgt der Wirtsseite, weil die
der grosse Posten ist.

---

## 9a. Wo getestet wird

Weg der GPU in die microVM, Video und die Schalter fuer GPU/CPU im selben
Gastimage: `GPU_IN_VM.md`.

QEMU ist schnell, aber nopeekOS sieht dort nur eine nachgebaute Karte ohne
Recheneinheit. Der Entwicklungsrechner bringt mehr mit (abgefragt
2026-10-12, nur lesend):

| | |
|---|---|
| `03:00.0` | Radeon RX 9060 XT (Navi 44, RDNA4) — treibt den Monitor, `renderD128` |
| `15:00.0` | Radeon 610M (Granite Ridge, RDNA2) — **kein Bildschirm daran**, eigene IOMMU-Gruppe, `renderD129` |
| IOMMU | AMD-Vi aktiv · nested KVM an |
| Software | QEMU 11.1.2, virglrenderer 1.3.0, Linux 7.2.9 |

**Regel: am Entwicklungsrechner aendert sich nichts, was das Booten in Arch
beruehren kann** — kein Bootloader, keine Kernel-Parameter, kein initramfs,
kein `modprobe.d`. Pakete nachinstallieren ist erlaubt.

| Wo | Was | Stufen |
|---|---|---|
| QEMU nackt | `gpudev` mit einem `gpu_soft`-Modul (gpudev auf der CPU): ABI, Puffer, Fences, Uebergabe beim Laden, Rueckfall beim Modultod | 1, 3 |
| QEMU + native context | **Referenz** fuer die microVM-Seite: unser Gast unter QEMU, was er schickt, was er erwartet — wie die Galerien beak gegen Chromium | 6–9 |
| QEMU + 610M (VFIO) | amdgpu-Grundlagen auf echter Hardware (PSP, Firmware, SDMA) — **aber RDNA2, nicht Vega**, und AMD-iGPUs sind in VFIO zickig (VBIOS aus VFCT, Reset nach VM-Ende). Erst nach Entscheid, braucht `vfio-pci`-Bindung zur Laufzeit | 8 |
| NUC | `gpu_intel_tgl` | 1, 2, 4, 5, 7 |
| IdeaPad | Lucienne: SMU, Helligkeit, DCN 2.1 | 0a, 8, 9 |

### Die Referenz: `tools/gpu_nativectx.py`

Startet **unseren** Gast-Kernel mit **unserem** sqfs unter QEMU, aber nicht
unseren PID 1: ein Wegwerf-initramfs (busybox und musl aus dem sqfs,
`eglinfo`/`glxinfo` aus Alpines `mesa-utils`, sha256 gepinnt) haengt das sqfs
ein, laesst `eglinfo -B` laufen und startet neu. `--mode drm` (native
context), `virgl`, `off` (2D, llvmpipe — die Gegenprobe). Ausgabe: die
Feature-Zeile von virtio-gpu im Gast, der GL-Renderer je EGL-Plattform, und
eine Zaehlung der virtio-gpu-Befehle, die QEMU gesehen hat (`-trace
virtio_gpu_*`).

Vorher geprueft: das sqfs enthaelt `libgallium-26.2.4.so` **mit radeonsi und
dem virtio-Backend** (`vdrm_*`), dazu `libdrm_amdgpu`. Fehlt am Wirt:
`qemu-hw-display-virtio-gpu-gl` und `qemu-hw-display-virtio-gpu-pci-gl`.

**Erster Lauf, 2026-10-13 — native context traegt mit UNSEREM Gast.**
`--mode drm` auf dem Entwicklungsrechner: unser bzImage (6.18.55-nopeek) und
das Mesa aus unserem sqfs (26.2.4) melden auf GBM, Surfaceless und Device
`AMD Radeon RX 9060 XT (radeonsi, gfx1200, ACO, DRM 3.64)`; der Gast sieht
`+virgl +edid +resource_blob +host_visible +context_init`. Am Gast war
nichts zu aendern. Die Gegenprobe `--mode off` bleibt bei llvmpipe.

Was der Gast fuer `eglinfo -B` schickt (QEMU-Trace) — die Bauliste fuer
`virtio_gpu_pci.rs`:

| Befehl | Anzahl |
|---|---|
| `CTX_SUBMIT` (die ccmds) | 168 |
| Fences (`fence_ctrl`/`fence_resp`) | 135 |
| `RESOURCE_CREATE_BLOB`, `CTX_ATTACH/DETACH_RESOURCE`, `RESOURCE_UNREF` | je 21 |
| `CTX_CREATE` / `CTX_DESTROY` | je 3 |
| `GET_DISPLAY_INFO`, `GET_EDID` | je 2 |

`RESOURCE_MAP_BLOB` kommt in diesem Lauf nicht vor — `eglinfo` rendert
nichts. Ein echter Zeichenlauf (z. B. `eglgears` oder LibreWolf mit WebGL)
ist der naechste Referenzlauf, und mit ihm die hostmem-Region. Die
Capset-Abfrage (`GET_CAPSET_INFO`/`GET_CAPSET`) hat in QEMU keinen
Trace-Punkt und fehlt deshalb in der Zaehlung, nicht im Ablauf.

**Befund fuer spaeter:** der Sitzungsstart im Gast bindet `glxtest` und
`vaapitest` von LibreWolf auf `/bin/false` (`microvm/linux/init/src/main.rs`)
und npkwm laeuft mit `WLR_RENDERER=pixman`. Damit nimmt LibreWolf heute
**nie** die GPU, auch wenn eine da waere. Beides muss mit Stufe 7/9 fallen.

---

## 10. Offene Fragen

- **F1** Rueckgabe beim Modultod — Entwurf in §3.2: nur Kernelpuffer im
  Scanout, Kernel zeichnet auf der CPU in den zuletzt geflippten weiter.
  Offen: Tod mitten in einem GGTT-Umbau; und ob das auf AMD genauso traegt.
- **F2** forge-Kosten: wie viele Hostaufrufe kostet eine Abgabe, und wie teuer
  ist einer? Stufe 1 misst es; bei Bedarf ein `npk_mmio_write_batch`.
- **F3** ~~Geteilter Speicher Kernel ↔ Modul fuer den Befehlsring?~~
  Beantwortet 2026-10-13: netdev hat keinen geteilten Ring — das Modul holt
  per Hostaufruf (`npk_netdev_poll_tx`) und wird ueber `npk_wait` geweckt.
  gpudev nimmt dasselbe Muster; die Befehle sind Griffe, nie Pixel.
- **F4** Welche Register darf ein unprivilegierter Befehlspuffer auf Gen12
  schreiben (i915 `intel_workarounds`/Whitelist), und reicht das als Grenze?
- **F5** IOMMU: VT-d/AMD-Vi programmieren — eigenes Papier, aber die
  Voraussetzung dafuer, dass ein Treibermodul NICHT so viel Vertrauen braucht
  wie der Kernel.
- **F6** Gast-Mesa fuer Intel selbst bauen: im `microvm-userspace/build.sh`
  oder als eigenes apk? Und wie stabil ist `intel-virtio-experimental` in
  26.2 wirklich — vor Stufe 7 im QEMU auf einem Linux-Wirt mit TGL pruefen,
  bevor wir die Wirtsseite bauen.
- **F8** Shader beim Bauen: Mesas Intel-Compiler (brw) aus NIR/GLSL fuer
  Gen12 ausserhalb von Mesa aufrufen — welches Werkzeug, wie gross, und
  wie die Binaerdaten samt Bindungstabellen ins Modul kommen. Vorab pruefen
  an einem einzigen Shader (Textur mit Deckkraft).
- **F9** WebGL in beak: fremder GPU-Code in einer signierten App gehoert auf
  die Fremd-Stufe (§2.3). Eigener Kontext je Seite wie ein Gast — oder
  WebGL nur in der microVM?
- **F7** Videodekodierung: VA-API im Gast ueber native context
  (`radeonsi_drv_video` ist im AMD-MR enthalten, bei Intel `intel-media-driver`
  mit Fremdpatch) — und `tune` auf dem Wirt ueber dieselbe VCS/VCN-Engine.

---

## Quellen

- QEMU-Dokumentation virtio-gpu, Tabelle „DRM native context":
  <https://www.qemu.org/docs/master/system/devices/virtio/virtio-gpu.html>
- Mesa MR „amdgpu: native context support for virtio":
  <https://gitlab.freedesktop.org/mesa/mesa/-/merge_requests/21658>;
  Phoronix zur Aufnahme in Mesa 25.0:
  <https://phoronix.com/news/AMDGPU-VirtIO-Native-Mesa-25.0>
- Mesa MR „Support virtio-intel native context":
  <https://gitlab.freedesktop.org/mesa/mesa/-/merge_requests/29870>;
  Mesa 26.1 (iris/crocus/ANV, virgl unmaintained):
  <https://linuxiac.com/mesa-26-1-graphics-stack-brings-vulkan-and-opengl-improvements/>
- Mesa `meson.options` (Zweig 26.2): `amdgpu-virtio`,
  `intel-virtio-experimental`, beide Vorgabe `false`
- Alpine aports `main/mesa/APKBUILD` (pkgver 26.2.4): `-Damdgpu-virtio=true`,
  keine Intel-Option
- virglrenderer `src/drm/` (`main`), Zeilenzahlen oben
- xe native context (Fremdprojekt):
  <https://github.com/cmspam/xe-native-context-enablement>
