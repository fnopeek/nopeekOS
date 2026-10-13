# GPU-Stack — eine Schnittstelle, die Karte darunter egal

**Stand:** 2026-10-12, Entwurf. Noch nichts gebaut.
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
  virtio-gpu                eigene kleine            Flaechen wie heute,
  DRM native context        2D-Befehlsliste          der Compositor
        │                   (Rasterizer-Trait)       beschleunigt
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
   bekommen eine kleine, feste 2D-Befehlsliste. Ein gemeinsames 3D-API fuer
   beide waere ein eigenes Mesa — genau das, was wir vermeiden.
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

**Offen:** `npk_mmio_map_phys` laesst ein Modul mit globalem `HARDWARE`
die BARs eines fremden Geraets ueber die physische Adresse abbilden, an
der Besitzerpruefung vorbei. Dafuer braucht es eine BAR-Tabelle aus der
Enumeration (BARs an einer laufenden GPU auszumessen stoert das Bild).
`HARDWARE` ist heute ohnehin Kernelvertrauen (§8.4).

**Rueckweg beim Modultod (F1):** Das Modul scannt nur Puffer aus, die dem
Kernel gehoeren (`flip(bo)` nimmt einen Griff, keine Adresse) — der erste
ist das uebernommene Firmware-Bild. Die GGTT-Eintraege dafuer bleiben
stehen, wenn das Modul stirbt. Der Kernel zeichnet also auf der CPU in den
zuletzt geflippten Puffer weiter und schreibt dabei kein Register. Offen
bleibt der Tod MITTEN in einem GGTT-Umbau — dagegen schuetzt nur, dass das
Modul den Scanout-Eintrag nie anfasst, solange er im Scanout steht.

### 3.3 Die neue ABI: `gpudev`

Analog zu `npk_netdev_register` beim WLAN: das Modul meldet sich als
GPU-Geraet an und bedient danach einen Befehlsring, den der Kernel fuellt.

**Kernel → Modul** (Ring im geteilten Speicher, Wecken ueber `npk_wait`):

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

1. **Display-Ebenen.** Intel (Universal Planes je Pipe) und AMD (DCN: HUBP +
   MPC) ueberlagern mehrere Ebenen mit Alpha in Hardware. Zeiger auf eine
   eigene Ebene (heute „save-under"), ein Video oder ein Vollbild-VM-Fenster
   ohne Kopie. **Kein Shader noetig.**
2. **Kopier-Engine** (BCS / SDMA) fuer Fensterpuffer → Bild. Am NUC gibt es
   das schon.
3. **Feste Compute-Kernel** fuer Blenden, abgerundete Ecken (SDF), Schatten,
   Blur, Skalieren — **eine Handvoll, je GPU-Familie von Hand geschrieben**,
   kein Shader-Compiler. Sie sitzen im Modul, der Compositor ruft sie ueber
   die 2D-Befehlsliste.
4. **Text** bleibt auf der CPU (Glyphen-Atlas), bis es gemessen einen Grund
   gibt.

### 5.3 Die 2D-Befehlsliste

Ausgangspunkt ist der vorhandene `Rasterizer`-Trait: eine GPU-Umsetzung
schreibt die Aufrufe eines Bildes in einen Befehlspuffer und gibt ihn mit
einem `submit` ab — **ein Hostaufruf je Bild, nicht je Rechteck**. Was der
Trait heute nicht kennt (Puffer zusammensetzen, Blur, Ebenen), kommt dazu,
jeweils mit CPU-Umsetzung.

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

---

## 9. Stufen

Jede Stufe hat ein Ergebnis, das man am Geraet sieht oder misst.

| Stufe | Inhalt | Geraet | Ergebnis |
|---|---|---|---|
| **0a** | `gpu_amd` Stufe 0: SMU-Postfach + Panel-PWM | IdeaPad | Helligkeit per Fn; Watt vorher/nachher mit `power` |
| **0b** | `gpu`-Ausgabe ohne Treiber korrigieren | beide | kleine Aufraeumarbeit |
| **1** | `gpudev`-ABI + `gpu_intel_tgl` uebernimmt die Karte vom Kernel: GGTT, ForceWake, BCS, Flip — **Gleichstand mit `intel_xe.rs`** | NUC | gleiche Bildzeiten wie heute; Hostaufrufe je Bild gezaehlt (forge-Kosten) |
| **2** | PPGTT je Kontext, GT-Interrupts, RCS, erster Compute-Dispatch mit Readback-Selbsttest | NUC | Selbsttest gruen; Fences per Interrupt |
| **3** | Fenster als Puffer im Compositor (CPU-Weg zuerst) | beide | kein Rueckschritt auf dem CPU-Weg |
| **4** | Display-Ebenen: Zeiger, dann ein Fenster | NUC | Zeiger ohne save-under |
| **5** | Compute-Kernel: Zusammensetzen, Glas, Ecken | NUC | `comp`-Zeit je 4K-Bild vorher/nachher |
| **6** | virtio-gpu: blob, context init, hostmem, Capset `DRM` | — | Gast sieht das Capset |
| **7** | native context Intel (Wirtsseite i915-Protokoll) + Gast-Mesa mit `intel-virtio-experimental` | NUC | `glxinfo` im Gast nennt iris; WebGL in LibreWolf |
| **8** | `gpu_amd` Fundament: PSP, Firmware, SDMA, GFX, GPUVM | IdeaPad | SDMA-Selbsttest gruen |
| **9** | native context AMD (Gast-Mesa von Alpine) | IdeaPad | `glxinfo` im Gast nennt radeonsi |

Zu Stufe 7 gegen 9: fuer die microVM ist AMD auf der GAST-Seite billiger
(Alpine liefert das Backend), Intel auf der WIRT-Seite (der Treiber laeuft
schon halb, keine Firmware). Die Reihenfolge folgt der Wirtsseite, weil die
der grosse Posten ist.

---

## 9a. Wo getestet wird

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
