# gpu_amd_renoir Stufe 7 — native context fuer den Gast

**Stand:** 2026-10-20 · Modul `gpu_amd_renoir` 0.11.2 (5b ✅ Geraet) · Kernel 0.653
**Vorbild:** virglrenderer 1.3.0 `src/drm/amdgpu/amdgpu_renderer.c` (1292 Zeilen,
10 ccmds) + `src/drm/drm_context.c`, Gast-Seite Mesa 26.2.4
`src/amd/common/virtio/`, libdrm 2.4.134 `amdgpu/` (Capset), Linux 6.18.55
amdgpu (alles, was virglrenderer per ioctl ruft).
**Daneben:** `GPU_IN_VM.md` §4a/§4b (der Intel-Weg, der dafuer verallgemeinert
wird), `GPU_AMD_LINUX_MAP.md` §1.

Quellen lokal: `~/.cache/nopeekos/virglrenderer/virglrenderer-1.3.0`,
`~/.cache/nopeekos/libdrm/libdrm-2.4.134`.

---

## 1. Was der Gast schickt

radeonsi (und radv) im Gast reden statt mit `/dev/dri/renderD*` mit
virtio-gpu, Kontexttyp 2. Jeder Aufruf ist ein ccmd im SUBMIT_3D-Strom;
virglrenderer spielt ihn als ioctl auf einem eigenen amdgpu-`drm_file` je
Gastkontext nach. Bei uns ist das Modul der amdgpu.

| ccmd | virglrenderer ruft | Linux | im Modul heute |
|---|---|---|---|
| 1 QUERY_INFO | `DRM_AMDGPU_INFO` | `amdgpu_info_ioctl` (757 Zeilen, 79 Faelle) | — |
| 2 GEM_NEW (blob_id) | `amdgpu_bo_alloc` | `amdgpu_gem_create_ioctl` | `amdgpu_gem_object_create` (nur GTT, Kernelpuffer) |
| 3 BO_VA_OP | `amdgpu_bo_va_op_raw` | `amdgpu_gem_va_ioctl` | ✅ aus 5b |
| 4 CS_SUBMIT | `amdgpu_cs_submit_raw2` + Syncobj rein/raus | `amdgpu_cs_ioctl` | ✅ IB + BO-Liste; fehlt: FENCE-, DEPENDENCIES-, SYNCOBJ-Chunks |
| 5 SET_METADATA | `amdgpu_bo_set_metadata` | `amdgpu_gem_metadata_ioctl` | — |
| 6 BO_QUERY_INFO | `amdgpu_bo_query_info` | `GEM_OP(GET_GEM_CREATE_INFO)` + `GEM_METADATA(GET)` | — |
| 7 CREATE_CTX (+DESTROY) | `amdgpu_cs_ctx_create2` | `amdgpu_ctx_ioctl` | ✅ alloc; fehlt free, Prioritaet |
| 8 RESERVE_VMID | `amdgpu_vm_(un)reserve_vmid` | `amdgpu_vm_ioctl` | — |
| 9 SET_PSTATE | `amdgpu_cs_ctx_stable_pstate` | `amdgpu_ctx_ioctl` (STABLE_PSTATE) | — |
| 10 CS_QUERY_FENCE_STATUS | `amdgpu_cs_query_fence_status` | `amdgpu_cs_wait_ioctl` | — |

Dazu ausserhalb der ccmds: `get_blob` (blob_id 0 = Antwortpuffer
`amdvgpu_shmem`, sonst das GEM-Objekt zur blob_id, Cache-Art WC ausser GTT
ohne `CPU_GTT_USWC`), `resource_map` (`GEM_MMAP`), `attach_resource`
(dma-buf-Import fremder Ressourcen), `submit_fence` (Zeitleiste je Ring).

**Was Mesa abfragt** (ausgezaehlt in `ac_gpu_info.c`, `ac_linux_drm.c`,
`virtio/*.c`, beiden Winsys): DEV_INFO, HW_IP_INFO/COUNT, FW_VERSION (PFP,
ME, MEC, VCN, UVD, VCE …), MEMORY, VRAM_GTT, READ_MMR_REG, VIDEO_CAPS,
TIMESTAMP, *_USAGE, NUM_EVICTIONS/BYTES_MOVED/VRAM_CPU_PAGE_FAULTS,
GPUVM_FAULT, SENSOR (SCLK, MCLK, Temperatur), MAX_IBS, UQ_FW_AREAS.
Gebaut wird trotzdem `amdgpu_info_ioctl` GANZ — eine ausgezaehlte Teilmenge
waere eine Schaetzung darueber, was der naechste Mesa fragt.

## 2. Was am Wirt Intel-fest ist (Bestandsaufnahme 2026-10-20)

Generisch und weiter benutzbar: der virtio-gpu-Teil (CONTEXT_INIT, Capset-
Rahmen 1024 B, CTX_CREATE, SUBMIT_3D-Strom, Zaeune je (Kontext, Ring),
Host-visible-Fenster, MAP_BLOB, SET_SCANOUT_BLOB), die IOMMU-Pruefung (VT-d
oder AMD-Vi), gpudev `OP_CLIENT`/`OP_OBJECT`/`OP_CCMD`, `npk_gpudev_reply`,
`npk_gpudev_ring`, die Zombie-Freigabe.

Intel-fest und zu verallgemeinern:

1. `gpu_backend.rs` waehlt immer `virtio_gpu_i915::GpudevBackend`.
2. Das Capset baut `i915_capset` im Kernel aus dem PCI-Raum. Fuer amdgpu
   steht darin `address32_hi`, `amdgpu_buffer_size_alignments`,
   `amdgpu_gpu_info` (libdrm `amdgpu_query_gpu_info_init`: DEV_INFO plus
   Registerlesungen GB_ADDR_CONFIG, Raster-Config je SE, Tile-Modes …),
   `marketing_name` (libdrm `amdgpu.ids`), `has_vm_always_valid` = 1 —
   das weiss nur das Modul. **Neu: `npk_gpudev_capset(bytes)`**, vom Modul
   vor `ready` gesetzt; der Kernel reicht es durch und prueft nur Groesse
   und Kontexttyp.
3. `RSP_MEM_OFFSET` = 16 und `banned_ctx_mask` an 8/12 sind i915s
   Antwortpuffer. amdgpu: `rsp_mem_offset = sizeof(amdvgpu_shmem)`,
   dahinter `async_error` und drei `amdgpu_heap_info`. → je Backend.
4. `i915_error_response` → je Backend (amdgpu: `ret` immer an 4).
5. `ccmd_alignment`: i915 4, amdgpu 8 (`drm_context_init`-Vorgabe).
6. `ProbeBackend`, `GPU_STAT_ENGINES`, Logzeilen: i915/TGL.
7. Gast: `LIBVA_DRIVER_NAME=iHD` und `ANV_DEBUG` haengen an „GPU ja", nicht
   am Hersteller; Mesa-Bundle ohne radv und ohne `gallium-va`.

## 3. Die eine Entscheidung vorab: wem gehoert der Speicher?

Bei Intel legt der **Kernel** die Rahmen eines GEM-Objekts an (Zaehlung je
VM, IOMMU-Freigabe, Abbildung in den Gast), das Modul bekommt eine
Seitenliste. Fuer i915 passt das: GEM-Objekte sind dort shmem-Seiten.

amdgpu kennt **Domaenen**: GEM_NEW sagt VRAM, GTT oder beides, dazu
`CPU_ACCESS_REQUIRED`, `NO_CPU_ACCESS`, `CPU_GTT_USWC`, `VM_ALWAYS_VALID`,
`EXPLICIT_SYNC`. Das IdeaPad hat 512 MB VRAM-Carve-out, radeonsi legt fast
alles dorthin und faellt auf GTT zurueck.

- **(a) Wie Intel, Kernel-Rahmen als importiertes sg-Objekt.** Alles wird
  GTT, VRAM-Wuensche fallen still weg, Objekte sind gepinnt. Das ist eine
  andere Semantik als Linux — genau die Art Abweichung, die spaeter Stunden
  kostet.
- **(b) Das Modul legt an, 1:1 `amdgpu_gem_create_ioctl`** (TTM-Platzierung
  wie in 5b, VRAM ueber den vorhandenen Buddy, GTT ueber DMA-Speicher des
  Moduls). Fuer `MAP_BLOB` meldet das Modul dem Kernel die Seiten
  (GTT: seine DMA-Rahmen; VRAM: Adressen im BAR0-Fenster); der Kernel prueft,
  dass jede Seite dem Modul gehoert, bevor er sie in den Gast blendet. Die
  Zaehlung je VM macht der Kernel am GEM_NEW-Kopf (`alloc_size`), bevor er
  weiterreicht.

**Entschieden 2026-10-20: (b).** Kosten: eine Kernel-Pruefung „gehoert dem Modul"
(DMA-Besitz oder BAR des eigenen Geraets), und eine offene Frage fuer
Schritt C: in Linux darf TTM ein abgebildetes Objekt verschieben, weil die
CPU-Abbildung ueber `ttm_bo_vm_fault` neu aufgebaut wird. Bei uns liegt die
Abbildung im EPT/NPT des Gastes; ein abgebildetes Objekt muss seine Seiten
also behalten, bis `UNMAP_BLOB` kommt, oder der Kernel muss die Abbildung
beim Verschieben zurueckziehen.

### 3a. Was (b) im Einzelnen heisst (Bestandsaufnahme 2026-10-20)

Gemessen am Kernel: Modul-DMA ist **ein zusammenhaengender Lauf je Aufruf,
hoechstens 4 MB**, eine GPU-Karte darf insgesamt **128 MB** halten, und
nichts haelt solche Seiten fest — `npk_dma_free` und `release_hw` geben sie
sofort an den Rahmenallokator zurueck. Ein Gast-Objekt darin waere also zu
klein, zu knapp bemessen und beim Modultod ein Use-after-free im Gast.

Darum, Linux naeher: **die Platzierung entscheidet das Modul (TTM wie
Linux), die Seiten eines GTT-Objekts kommen wie bei `ttm_pool` vom
Seitenallokator** — hier vom Kernel:

- `npk_gpudev_tt_alloc(client, pages)` → Kernelpuffer-Id: einzelne Rahmen
  (nicht zusammenhaengend), genullt, der IOMMU der Karte freigegeben, dem
  Gast-Client zugerechnet (Deckel je VM, Schutz, keine Rechnung). Das Modul
  liest die Seiten mit `npk_gpudev_bo_pages` und baut daraus die `ttm_tt`
  des Objekts (wie ein sg-Import). `npk_gpudev_tt_free(id)`.
- Freigegeben werden die Rahmen erst, wenn **beide** Seiten losgelassen
  haben: das Modul (`tt_free`) und der Gast (UNMAP_BLOB, Ressource weg) —
  derselbe `Held`-Mechanismus wie bei Intel.
- **VRAM** wird ueber BAR0 eingeblendet (WC). Stirbt das Modul, zieht der
  Kernel jede solche Abbildung aus dem Gast zurueck, bevor die Karte neu
  vergeben wird.
- Neuer Auftrag `OP_BLOB` (Client, blob_id, res_id): das Modul antwortet
  mit der Art (TT-Id oder VRAM-Bereiche im BAR) und der Cache-Art
  (`enable_cache_wc` wie `amdgpu_renderer_get_blob`). Der Kernel prueft,
  dass die TT-Id dem Client gehoert bzw. die Bereiche im BAR des Moduls
  liegen.
- Dazu im Modul, fuer die Isolation zwischen Clients Pflicht:
  `VRAM_CLEARED` beim Anlegen und `VRAM_WIPE_ON_RELEASE` beim Freigeben
  (`amdgpu_fill_buffer` auf SDMA), bisher benannt und nicht portiert.

## 4. Schritte, jeder mit einem Ergebnis am Geraet

| Schritt | Inhalt | Ergebnis |
|---|---|---|
| **A** Wirt verallgemeinern | Backend je Hersteller (`virtio_gpu_amdgpu.rs` neben `_i915.rs`), Antwortpuffer-/Fehler-/Alignment-Layout je Backend, `npk_gpudev_capset`, Probe-Backend fuer 04:00.0 | Gast: `npk-gpuprobe` sieht Kontexttyp 2 und das Capset; Mesa faellt sauber auf llvmpipe |
| **B** Abfragen | `amdgpu_info_ioctl` ganz, Capset (libdrm `amdgpu_query_gpu_info_init`, `query_buffer_size_alignment`, `amdgpu.ids`), CREATE_CTX/DESTROY, `amdgpu_driver_open_kms` je Gastkontext (`OP_CLIENT`) | `glxinfo -B` im Gast nennt radeonsi und RENOIR |
| **C** Speicher | GEM_NEW (`amdgpu_gem_create_ioctl`), Seiten an den Kernel, MAP_BLOB, SET_METADATA, BO_QUERY_INFO, `GEM_OP`, Freigabe ueber `OP_OBJECT` | `glxinfo` vollstaendig, ein Puffer gefuellt und gelesen |
| **D** Ausfuehren | CS_SUBMIT mit allen Chunks (BO_HANDLES, IB, FENCE = Benutzerzaun, DEPENDENCIES, SYNCOBJ_IN/OUT → Ringzaehler je (Client, Ring) wie Intel), CS_QUERY_FENCE_STATUS (`amdgpu_cs_wait_ioctl`), RESERVE_VMID (`amdgpu_vm_ioctl`), SET_PSTATE | `glxgears`, dann Firefox mit Hardware-WebRender |
| **E** Haenger | `drm_sched`-Zeitueberschreitung → `amdgpu_job_timedout`: Soft-Recovery (`gfx_v9_0_ring_soft_recovery`, Wellen der VMID beenden), sonst `amdgpu_device_gpu_recover` mit MODE2 und Wiederaufbau; Kontext verloren → `async_error`, `AMDGPU_CTX_QUERY2` | ein absichtlich haengender Gast-Shader kostet den Gast den Kontext, nicht den Desktop |
| **F** Gast | Mesa-Bundle mit radv und `gallium-va` (radeonsi VA-API, fuer Stufe 6), init waehlt VA-API-Treiber nach Hersteller | Firefox: WebRender HW, Video weiter CPU bis Stufe 6 |

Stand 2026-10-20: A–D gebaut, host-seitig geprueft (`hostcheck/stage1_run.rs`
faehrt einen Gast von DEV_INFO bis SET_PSTATE, eine Gast-IB schreibt auf
VMID 2), Modul 0.12.0 ausgeliefert — CAP_NATIVE bleibt aus bis E. In D fehlt
nur SYNCOBJ_IN: virglrenderer nimmt ihn aus dem Eingangszaun des Wirts, und
den reicht unser virtio-gpu nicht durch (der Gastkern wartet selbst).

### E im Einzelnen (Linux 6.18.55, `amdgpu_job_timedout`)

Linux versucht der Reihe nach: Soft-Recovery, Reset der einen Queue, voller
Recover. Auf Renoir gilt je Ring:

| Ring | Soft | Queue-Reset | sonst |
|---|---|---|---|
| GFX | `gfx_v9_0_ring_soft_recovery` (SQ_CMD: Wellen der VMID beenden) | keiner (gfx9 hat kein kgq-Reset) | voll |
| Compute | dieselbe | `gfx_v9_0_reset_kcq` (KIQ unmap RESET_QUEUES, HQD aus, MQD aus der Sicherung, map) | voll |
| SDMA | keine | keiner (`supported_reset` bleibt 0) | voll |

- **E1 Soft-Recovery** — gebaut (`amdgpu_ring_soft_recovery`).
- **E2 Queue-Reset Compute** — gebaut, host-seitig an einer haengenden Gast-IB geprueft: `amdgpu_ring_reset_helper_begin/end`,
  `amdgpu_ring_backup_unprocessed_commands`, `amdgpu_fence_driver_guilty_force_completion`,
  `amdgpu_ring_alloc_reemit`, `drm_sched_wqueue_stop/start`, `gfx_v9_0_reset_kcq`,
  `kcq_init_queue(restore)`.
- **E3 voller Recover** — gebaut, host-seitig an einem haengenden GFX-Auftrag geprueft (MODE2, PSP/SMU-Resume, ein neuer Kontext laeuft danach) (`amdgpu_device_gpu_recover`, MODE2 ueber
  `smu_v12_0_mode2_reset`, kein `reset_cntl` fuer MP1 12): `in_gpu_reset`,
  `drm_sched_stop/start`, `increase_karma`, `pre_asic_reset` (Zaeune erzwingen,
  `amdgpu_device_ip_suspend` Phase 1+2), `reinit_after_reset`
  (`amdgpu_atom_asic_init`, `resume_phase1..3`, `check_vram_lost`,
  `fw_loading` mit `psp_resume`, `irq_gpu_reset_resume_helper`, IB-Tests,
  bei Fehler neuer Versuch). Dazu die Suspend/Resume-Wege von COMMON, GMC, IH,
  PSP (`psp_suspend`, `psp_ring_stop`, `psp_resume`), SMU (`smu_suspend`,
  `smu_resume`), GFX (`hw_fini` im Reset: `deactivate_kcq_hqd`, kein
  `kiq_fini_register`; `kiq/kcq_init_queue` aus der MQD-Sicherung), SDMA.
  **Nur Bloecke, die das Modul hochgebracht hat**: DCN, VCN und JPEG bekommen
  nur sw_init — Linux wuerde sie mit suspendieren und wieder hochfahren.
- **E4 Gast sieht es** — kein eigener Befehl: Mesa (`amdvgpu_cs_query_reset_state2`)
  liest nur `async_error`, und den erhoehen GEM_NEW, BO_VA_OP und CS_SUBMIT
  beim Scheitern wie virglrenderer. Nach einem Reset scheitert die naechste
  Einreichung des schuldigen Kontexts (`drm_sched_entity_error`, `guilty`).
- **CAP_NATIVE** — gemeldet ab Modul 0.13.0, wenn die Szenen laufen und das
  Capset abgegeben ist; der Kernel bietet die Karte nur hinter der IOMMU an.

**F im Gast**: das Mesa im Firefox-Image hat radeonsi mit `-Damdgpu-virtio`
schon. Offen: radv (`-Dvulkan-drivers=intel,amd`) und die VA-API-Wahl nach
Hersteller — auf AMD faellt libva mit `LIBVA_DRIVER_NAME=iHD` sauber aus, und
VA-API auf AMD braucht ohnehin VCN (Stufe 6).

**E gehoert in Stufe 7, nicht danach.** Heute steht im Modul „GPU-Recovery
bei Job-Timeout: benannt, nicht portiert" — fuer den eigenen Compositor
war das vertretbar, fuer fremden Shadercode aus einer VM nicht: ein
Endlos-Shader im Gast haelt sonst die Compute-Pipe des Desktops mit fest.

## 5. Pruefung auf dem Host

Wie in 5b: `hostcheck/` faehrt die ccmds gegen die Ersatz-GPU. Orakel fuer
B ist libdrm selbst — `amdgpu_query_gpu_info_init` gegen unsere
`amdgpu_info_ioctl`-Antworten gebaut, Byte fuer Byte gegen das Capset; fuer
die ccmd-Layouts die `static_assert`s aus `amdgpu_virtio_proto.h` als
Rust-Tests.

## 6. Sicherheit

Der Gast schreibt IBs, die unveraendert laufen. Grenzen: eigene VM (VMID,
Seitentabellen) je Gastkontext — ✅ aus 5b; AMD-Vi vor der Karte — ✅;
privilegierte Pakete im IB sind bei GFX9 durch den CP auf Benutzer-IBs
begrenzt (`IB` ohne PRIV); Haenger → E. Offen zu pruefen: welche Register
ein Benutzer-IB per `SET_*_REG`/`WRITE_DATA` erreicht (Linux verlaesst sich
auf die CP-Firmware), und `READ_MMR_REG` nur mit Linux' Positivliste
(`soc15_allowed_read_registers`).

## 5. Gast-Bilder im Wirts-Compositor: Kachelung und DCC (Stand 2026-10-20)

Am IdeaPad laeuft Firefox im Gast mit WebRender auf radeonsi (Kernel 0.662.0,
Modul 0.15.1). Der Wirts-Compositor zeigt die Bilder aber als Kachelmuster,
weil er sie linear liest. Gemessen (0.15.1, `dmesg gpudev: bo`):

- erstes Scanout-Bild von npkwm: Swizzle 25 = **64KB_S_X**, kein DCC,
  Objekt 1184 Seiten, Sicht 1115 Seiten (pitch 4096 bei 943 px)
- laufende Frames: Swizzle 26 = **64KB_D_X mit DCC** (DCC-Offset 4718592,
  `dcc pitch` 1023, scanout 0), Objekt 1168 Seiten
- `tiling_flags` z. B. 0x0000087FE009001A; die UMD-Metadaten-Worte wurden im
  Log abgeschnitten — als Erstes auf eine eigene Zeile legen.

Wie Linux es macht: der Konsument importiert den dmabuf mit implizitem
Modifier, liest das Layout aus den Objekt-Metadaten (radeonsi legt beim
Export die Textur-Beschreibung T# in die UMD-Metadaten, `si_set_tex_bo_metadata`;
Import: `ac_surface_apply_umd_metadata`) und **sampelt ueber die
Textur-Einheit**, die Kacheln und DCC in Hardware aufloest. SDMA-Entkacheln
allein reicht nicht (kein DCC).

Plan:
1. Import-Schritt im Modul-Compositor: ein Shader (RADV/acoc wie die anderen,
   nichts von Hand) liest das Gast-Bild ueber ein Image und schreibt linear in
   einen Puffer, den `compose` wie bisher nimmt.
2. Die T# aus den UMD-Metadaten (Port der radeonsi-Import-Logik), Basis- und
   DCC-Adresse vom Modul eingesetzt.
3. **Sicherheit:** Breite, Hoehe, Pitch, Swizzle und DCC-Bereich gegen die
   Objektgroesse pruefen — eine gefaelschte Beschreibung darf nichts ausser dem
   eigenen Objekt im Compositor-Adressraum lesen.

### 5.1 Gebaut (Modul 0.16.3)

Alle drei Punkte, und die Gast-T# wird **nie** benutzt: der Compositor baut
die T# selbst, wie radeonsi beim Import (`si_texture_from_winsys_buffer`):

| Schritt (radeonsi)               | Mesa, gemeinsamer Code            | Modul `src/import.rs` |
|----------------------------------|-----------------------------------|-----------------------|
| `amdgpu_bo_get_metadata`         | `ac_surface_apply_bo_metadata`    | Swizzle + Scanout aus den Tiling-Flags |
| `si_init_surface` (IMPORTED)     | `ac_compute_surface`              | Bild- und DCC-Masse |
| Import                           | `ac_surface_apply_umd_metadata`   | aus den Metadaten NUR DCC an/aus, Offset, pipe/rb-aligned |
| `si_make_texture_descriptor`     | `ac_build_texture_descriptor`     | T# fuer B8G8R8A8_UNORM, 2D |
| `si_set_mutable_tex_desc_fields` | `ac_set_mutable_tex_desc_fields`  | Basis, SW_MODE, PITCH, DCC-Adresse |

RADVs Legacy-Import ist das falsche Vorbild: er ignoriert den Swizzle der
Tiling-Flags und rechnet DCC mit Groesse 0 (eigener Kommentar: „basic
ability … that don't have DCC").

**Orakel:** `shaders/imgdesc.c` faehrt genau diese Mesa-Schritte (statisch
gegen `libamd_common.a`/`libaddrlib.a`, Geraet aus RADV hinter dem
drm-shim). `hostcheck/check.sh` vergleicht 1832 Faelle Feld fuer Feld und
**1450 T# Wort fuer Wort** (Groessen bis 16384, Modi 21/22/24–27, Scanout,
DCC an/aus, beide Ausrichtungen, Metadaten, die Mesa ignoriert oder
ablehnt). Gemessen dabei: Bild = ganze Bloecke (4 KB: 32x32, 64 KB:
128x128 px); DCC = `align(pitch,512) * align(hoehe,512) / 64` Byte, auf die
Blockgroesse aufgerundet; mit Scanout laesst addrlib nur 64KB_S_X zu.
Modi 9/10 (64 KB ohne XOR) rechnen anders und werden nicht angenommen.

**Sicherheit, strenger als Linux:** radeonsi prueft nur
`total_size <= bo_size`, und `total_size` wird nach dem Einlesen des
DCC-Offsets aus den Metadaten nicht neu gerechnet — Linux verlaesst sich auf
die eigene VM des Konsumenten. Die Compositor-VM haelt fremde Puffer, also
`import::check`: Bild im Objekt, DCC-Offset ausgerichtet, hinter dem Bild,
`offset + groesse <= Objekt`, keine gemischte pipe/rb-Ausrichtung (macht
Mesa nie). Acht Grenzfaelle im Hostcheck (`import_check bounds`). Jede
Abweichung → das Bild wird wie bisher linear gelesen, mit Grund im Log.

**Shader:** `shaders/import.comp` (nur AMD, liegt deshalb nicht in
`gpu_shaders/`): `texelFetch` auf ein `texture2D` in Set 0, schreibt die
Bytes unveraendert linear. `acoc` kennt dafuer einen Descriptor-Set
(`image`-Modus, je Dispatch neu gebunden, damit jede Vorlage den Zeiger
traegt); der Generator prueft, dass das Set genau die T# der Sicht ist
und jede Vorlage die Set-Adresse genau einmal traegt. Die vier alten
Programme sind Wort fuer Wort gleich geblieben (`--check`).

**Im Modul:** ein gekacheltes Gastobjekt wird GANZ in die Compositor-VM
gelegt (der DCC-Bereich liegt hinter der „Sicht"), dazu ein linearer
Puffer; die Szene sieht nur den linearen Puffer. Jede Szene importiert
zuerst alle gekachelten Gastbilder (ein Dispatch je Bild), dann laeuft
`compose` unveraendert.

**Log am Geraet:** `gpudev: bo N metadata[i] …` (alle Worte, 8 je Zeile),
`gpudev: bo N tiled, swizzle S, PxH, DCC at O (…): through the texture
unit into a linear buffer` oder `… read as it lies: <Grund>`; alle 600
Szenen `guest bo N imported pixels …` — die Pixel NACH dem Import.

Der lineare Puffer liegt in VRAM (0.16.1): ein GTT-Puffer des Compositors
ist EIN zusammenhaengender DMA-Block, und der Kernel gibt davon hoechstens
1024 Seiten (`MAX_DMA_PAGES_PER_CALL`) — 943x1115 sind schon 1046, 1080p
waeren 2025. Erster Geraetelauf mit 0.16.0: `bo 57 … not added (12)`.

Das Gastobjekt liegt in der Compositor-VM auf der Ausrichtung seiner
Oberflaeche (64 KB bei den 64KB-Modi, 0.16.2): die unteren Bits der
T#-Basis sind das Pipe/Bank-XOR, und die DCC-Adresse wird blockweise
gerechnet. Mit nur seitenausgerichteter Basis (0.16.1) las die
Textur-Einheit den DCC-Block HINTER dem Objekt — Lesefehler `TCP` verstreut
in einem 64-KB-Block, die Szene hing, der Import schrieb nie.

Eine Szene importiert und synchronisiert nur die Puffer, die sie NENNT
(0.16.3; `buffers::surface` merkt sich jede Abfrage der Szenenpruefung).
Vorher standen ALLE Gastobjekte in Import und BO-Liste: der Szenen-Job
wartete per impliziter Sync auf den Puffer, den der Gast gerade zeichnet,
und haengte selbst einen Fence an jeden Gastpuffer — jede Mausbewegung
(eine Szene) sperrte das WebGL-Aquarium (30 000 Fische).

### 5.2 Stand am Geraet (IdeaPad, 0.16.2)

Firefox steht im Wirt, WebRender + WebGL auf radeonsi, keine Lesefehler.
Gemessen:
- Rahmen: 64KB_S_X mit Scanout ohne DCC (erste npkwm-Bilder, Objekt 1184
  Seiten) und 64KB_D_X mit DCC (laufende, 1168 Seiten, DCC bei 4718592,
  65536 Bytes, pipe/rb 1/1). Metadaten-Wort 1 = `0x1002164c`.
- Alle Objekte 64-KB-ausgerichtet abgebildet, z. B. `0x1043C0000+0x490000`.

Offen, in dieser Reihenfolge:
1. **0.16.3 am Geraet**: blockiert WebGL beim Bewegen der Maus noch?
2. **Fehlende Buchstaben/Elemente**, nur manchmal (zweiter Start im
   Vollbild sauber) — also Timing, nicht Layout. Verdacht: Lesen vor dem
   Ende der Gastarbeit. Implizite Sync ist im CS-Port Linux-treu
   (radeonsi setzt nie EXPLICIT_SYNC, geteilte BOs stehen in jeder
   Gast-BO-Liste) — am Geraet nicht gemessen. Naechster Schritt: alle 600
   Szenen je Gastbild die Zahl offener Gast-Fences beim Abschicken loggen.
   DCC-Fast-Clears sind unwahrscheinlich: radeonsi loest DCC vor dem
   Weitergeben selbst auf (`si_flush_resource`, `gfx/si_blit.c`).
3. **„Bugwelle" beim Scrollen** (Florian: wie ein vsync-Problem, unter
   Intel nicht). Nicht untersucht. Kandidaten: der Gast zeichnet schon in
   den Puffer, den wir gerade importieren (Puffer-Freigabe an den Gast zu
   frueh?), oder die Szene laeuft laenger und das Bild kommt verspaetet.
   Erst messen: Szenendauer mit/ohne Import (`gpu_us` im Szenenbericht).
4. Die 600-Szenen-Pixelzeile liest den linearen VRAM-Puffer ueber
   `kptr` ab `res.start` — fuer einen NICHT zusammenhaengenden VRAM-Puffer
   zeigt sie fremden Speicher (`0xffffffff … 0x10101010` in allen alten
   Puffern). Diagnose korrigieren, bevor man ihr glaubt.
5. Import nur bei geaendertem Gastbild (heute jede Szene je gezeigtem
   Bild).
6. Ein Lesefehler der Compositor-VM meldet sich als „retry page fault" und
   flutet den IH-Ring, obwohl Renoir `noretry` hat — eigener Posten.
7. Hardware-Videodekodierung „unsupported" ist erwartet: VA-API auf AMD
   braucht VCN (Stufe 6).
