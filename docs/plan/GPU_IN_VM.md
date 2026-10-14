# GPU und Video in der microVM

Stand 2026-10-14. Wie der Gast (LibreWolf in der microVM) an die GPU des
Wirts kommt — fuer 3D (WebRender, WebGL) und fuer Videodekodierung — und
wie die Schalter stehen muessen, damit **dasselbe Gastimage** mit GPU die
GPU nimmt und ohne GPU von selbst auf die CPU faellt. Ergaenzt
`GPU_STACK.md` §4.3 (Stufe 6), §9 (Stufe 7, 9) und §9a (F6, F7).

Quellen sind am Ende; was nicht am Quelltext oder an offiziellen Papieren
geprueft ist, steht mit **[ungeprueft]** da.

---

## 1. Der Weg: DRM native context

```
Gast:  LibreWolf → GL/Vulkan/VA-API → Mesa-Treiber der ECHTEN Karte (iris, anv / radeonsi, radv)
                                      └ statt ioctl an i915/amdgpu: virtio-gpu (vdrm)
Wirt:  virtio-gpu (duenn: Griffe pruefen, ioctls nachspielen) → unser i915-Port / amdgpu-Port
```

- Der Gast faehrt denselben Mesa-Treiber wie auf blankem Metall; nur die
  unterste Schicht (ioctl) geht ueber virtio-gpu. Der Wirt braucht **kein
  Mesa**: virglrenderers Teil dafuer (`src/drm/drm_context.c`,
  `drm_fence.c`, `src/drm/i915/` ≈ 2–3k Zeilen, `i915_ccmd.c` ≈ 730) prueft
  Griffe und spielt die ioctls auf einem eigenen i915-Client nach. Das wird
  1:1 portiert, auf unseren i915-Port statt auf `drmIoctl`.
- **virgl** (GL-Aufrufe) und **Venus** (Vulkan-Aufrufe) scheiden aus: beide
  brauchen auf dem Wirt einen vollen GL- bzw. Vulkan-Treiber.
- Welche Karte darunter steckt, sagt der Wirt im **Capset `DRM`**
  (`struct virgl_renderer_capset_drm`, Mesa `src/virtio/virtio-gpu/drm_hw.h`:
  `context_type` 3 = i915, 2 = amdgpu, dazu PCI-Kennung). Mesa im Gast waehlt
  danach iris oder radeonsi. **Ein Gastimage mit beiden Treibern laeuft auf
  NUC (Intel) und IdeaPad (AMD).**

| | Intel (NUC, Tiger Lake) | AMD (IdeaPad: Zen 2 = Vega/GFX9) |
|---|---|---|
| Mesa im Gast | seit 26.1.0, `-Dintel-virtio-experimental=true`; iris + anv; TGL+ mit i915 genannt | seit 25.0, `-Damdgpu-virtio=true`; radeonsi, radv, **radeonsi_drv_video (VA-API)** |
| Alpine-Mesa | **ohne** Intel-Option (3.24: 26.1.6, edge: 26.2.4) → eigenes Mesa (F6) | **mit** AMD-Option → unveraendert |
| Wirt-Kerneltreiber | i915 (upstream); Xe nur ausserhalb | amdgpu |
| Wirt-Port aus virglrenderer | `src/drm/i915` (seit 1.3.0, „i915-experimental") | `src/drm/amdgpu` (seit 1.1.0) |
| Gastkernel | ≥ 6.14 (wir: 6.18) | dito |

Ein Notebook mit zwei GPUs (Chip + dedizierte Karte): der Wirt entscheidet,
welche er der VM gibt — eine Regel ist „Display und Desktop auf der
Chip-GPU, die VM bekommt die staerkere Karte"; das Bild kommt ueber die
Guest-Blobs (§4.4 in `GPU_STACK.md`) zurueck. Der Gast weiss davon nichts.
NVIDIA hat keinen reifen native-context-Weg; dort bleibt der Gast auf der CPU.

## 2. Was unser virtio-gpu dafuer koennen muss (Stufe 6/7)

1. Merkmale `CONTEXT_INIT` (+ `RESOURCE_BLOB`, gibt es), `num_capsets`,
   Capset `DRM` mit der Karte. **Nur anbieten, wenn ein GPU-Modul laeuft**
   — das ist der Rueckfall-Schalter (§4).
2. `CTX_CREATE` mit `context_init`, `SUBMIT_3D`, Zaeune je Ring
   (`VIRTIO_GPU_FLAG_INFO_RING_IDX`, der Gast nimmt 64 Ringe).
3. `RESOURCE_CREATE_BLOB` mit `BLOB_MEM_HOST3D` (blob_id → GEM-Objekt),
   `RESOURCE_MAP_BLOB`/`UNMAP_BLOB` in den **Host-visible-Bereich**
   (virtio-pci Shared-Memory-Capability, `SHM_ID_HOST_VISIBLE`) — ueber EPT/NPT
   eingeblendet, **Speichertyp nach dem mmap-Modus des GEM-Objekts**
   (`I915_CCMD_GEM_SET_MMAP_MODE`; QEMU braucht dafuer `honor-guest-pat`).
4. Antwortpuffer `vdrm_shmem` (selbst ein abbildbarer HOST3D-Blob).
5. `SET_SCANOUT_BLOB` auf HOST3D-Ressourcen — der Compositor nimmt das
   GEM-Objekt direkt.
6. i915-Port, je Gastkontext ein eigener Client: GEM_CREATE(_EXT),
   CONTEXT_CREATE_EXT (VM, Engines), VM_CREATE, EXECBUFFER2 mit Zaun rein/raus,
   QUERY, GETPARAM, MMAP_OFFSET, WAIT, BUSY, SET_DOMAIN, TILING,
   GET_RESET_STATS, REG_READ. Die Engine-Bits gehen durch — **die
   Video-Engines (VCS) sind damit erreichbar**.
7. Sicherheit: der Gast schreibt Befehlspuffer, die unveraendert laufen. Die
   Isolation ist das PPGTT je Kontext plus GPU-Reset — deshalb vorher
   **IOMMU (Stufe 2)**, wie im Plan.

## 3. Video

TGL dekodiert H.264, HEVC, VP9 und AV1 in Hardware. In den Gast:

| Weg | Intel | AMD | Firefox |
|---|---|---|---|
| **VA-API ueber native context** | iHD (intel-media-driver) ist **kein Mesa** und hat upstream keinen vdrm-Weg; es gibt nur einen Xe-Shim ausserhalb (cmspam). Ein i915-Shim waere eigener Gastcode | **radeonsi_drv_video geht mit** (laut Mesa-MR; Ende-zu-Ende an einem Geraet [ungeprueft]) | Firefoxs ausgereifter Weg (RDD, ffmpeg, DMA-BUF) |
| **Vulkan Video** (anv/radv) | anv: H.264/H.265, AV1 seit Mesa 25.0 (bis TGL), VP9 seit 25.2; laeuft ueber native context (Engines gehen durch) [ueber native context ungeprueft] | radv: VP9 erst ab RDNA1 — Vega nicht | seit Firefox 153, **aus** (`media.hardware-video-decoding-vulkan.enabled`), vor allem an NVIDIA erprobt |
| virtio-media (V4L2) | Gasttreiber nicht in Linux (v8 in Review), Wirt braeuchte eigenen Decoder | dito | Firefoxs V4L2-Weg ist ARM/zustandsbehaftet — fuer x86 nicht brauchbar |
| virtio-video | eingeschlafen | — | — |

Das IdeaPad (VCN 2.x) dekodiert H.264, HEVC, VP9, **kein AV1** — dort
bleibt AV1 auf der CPU (dav1d).

**Reihenfolge fuer YouTube auf dem NUC:** erst 3D (Stufe 7), dann im Gast
`ffmpeg -hwaccel vulkan` gegen AV1/VP9 pruefen; traegt das, Firefox' Vulkan-
Schalter ausprobieren. Sonst ein i915-vdrm-Shim fuer iHD nach dem Vorbild
des Xe-Shims (dauerhafter Gastpatch). Auf AMD ist VA-API ueber radeonsi der
Standardweg und braucht nichts Eigenes.

## 4. Die Schalter — mit GPU die GPU, ohne GPU die CPU

Die **eine** Entscheidung trifft der Wirt: bietet er `CONTEXT_INIT` + Capset
`DRM` an (nur mit laufendem GPU-Modul), oder nicht. Alles im Gast folgt
daraus, ohne eine Einstellung zu wechseln:

- **Mesa:** ohne Capset bleibt der Treiber „virtio_gpu" (virgl), der keinen
  Kontext bekommt → EGL/GBM fallen auf llvmpipe, Vulkan auf lavapipe. Mit
  Capset waehlt der pipe-loader iris/radeonsi.
- **Firefox:** `gfxtest` prueft EGL. Ein llvmpipe-Renderer steht auf der
  Sperrliste (`FEATURE_FAILURE_SOFTWARE_GL`) → Software-WebRender; eine
  fehlgeschlagene Pruefung ebenso (`FEATURE_FAILURE_GLXTEST_FAILED`). Mit
  echter Karte: Hardware-WebRender, DMA-BUF, dann die VA-API-Pruefung
  (`gfxtest vaapi -d <renderD>`), die bei Misserfolg auf Software-Dekodierung
  faellt.

### Gast (`microvm/linux/init/src/main.rs`)

| Schalter | heute | mit Stufe 7 |
|---|---|---|
| `gfxtest` → `/bin/false` | **seit 0.610.x nur ohne GPU** (Merkmal-Bit 4 `CONTEXT_INIT` in `/sys/bus/virtio/devices/*/features`). Vorher band das init `glxtest`/`vaapitest` — die heissen seit Firefox 155 `gfxtest`, das Binden ging ins Leere und die Pruefung lief bei jedem Start echt | bleibt so — mit GPU prueft Firefox selbst |
| `gfx.webrender.software=true` | gesetzt | **entfernen**: erzwingt Software auch mit GPU (`UserDisable`). Ohne GPU faellt Firefox ueber die Pruefung von selbst auf Software |
| `gfx.canvas.accelerated=false` | gesetzt | entfernen (unter Software-WebRender ohnehin aus) |
| `widget.dmabuf.force-enabled=false` | gesetzt | entfernen (= Vorgabe) |
| `layers.gpu-process.enabled=false` | gesetzt | entfernen (= Vorgabe unter GTK) |
| `gfx.webrender.max-partial-present-rects=1` | gesetzt (0.610.0) | behalten — laut Quelltext ist 1 unter GTK Vorgabe, gemessen hat das Setzen den Schaden aber von ~95 auf ~50–90 % gesenkt [Widerspruch ungeklaert] |
| `media.av1.enabled=false` | gesetzt | Codec-Wahl, kein GPU-Schalter: aus, solange AV1 im Gast nicht in Hardware laeuft (VP9 in Software ist billiger als AV1) |
| `gfx.webrender.all`, `*.force-enabled` | — | **nie setzen**: hebeln die Sperrliste aus, auch fuer llvmpipe |
| `WLR_RENDERER=pixman` | gesetzt; npkwm ist ohne GL/Vulkan gebaut (`-Drenderers=[]`) | npkwm mit `gles2` bauen und den Renderer **nur ohne GPU** auf pixman setzen (derselbe Bit-4-Test). Ohne dmabuf-Import im Compositor bekommt Firefox keine Hardware-Flaeche [Folgerung] |
| Pakete | `mesa` (radeonsi + AMD-virtio), kein `mesa-va-gallium`, kein Vulkan | AMD: `mesa-va-gallium`, `mesa-vulkan-ati`; Intel: eigenes Mesa mit Intel-Option (+ anv) |

### Wirt

| Schalter | heute | mit Stufe 6/7 |
|---|---|---|
| `RESOURCE_BLOB` | nur mit gpudev-Szenen; `set microvm_blob off` | bleibt |
| `CONTEXT_INIT` + Capset `DRM` | nicht angeboten | nur mit laufendem GPU-Modul **und** IOMMU; ein Schalter `set microvm_gpu off` erzwingt die CPU |
| Host-visible-Bereich | keiner | Shared-Memory-Capability, nur wenn `CONTEXT_INIT` angeboten |

## 5. Offen

- Modultod waehrend einer 3D-Sitzung: der Gastkontext ist dann verloren.
  Wie Mesa/Firefox darauf reagieren (GPU-Reset-Meldung, Firefox' Rueckfall
  auf Software-WebRender zur Laufzeit) — klaeren, bevor Stufe 7 Vorgabe wird.
- Ob libva in einem virtio-Gast `virtio_gpu_drv_video` laedt und auf radeonsi
  umschaltet, oder `LIBVA_DRIVER_NAME=radeonsi` noetig ist [ungeprueft].
- Der Widerspruch bei `max-partial-present-rects` (§4).

## Quellen

- Firefox 157 (github.com/mozilla-firefox/firefox, Tag `FIREFOX_157_0_RELEASE`):
  `widget/gtk/GfxInfo.cpp`, `gfx/thebes/gfxPlatformGtk.cpp`,
  `gfx/config/gfxConfigManager.cpp`, `toolkit/xre/gfxtest/`,
  `modules/libpref/init/StaticPrefList.yaml`,
  `dom/media/platforms/ffmpeg/FFmpegDecoderModule.h`,
  `security/sandbox/linux/broker/SandboxBrokerPolicyFactory.cpp`;
  bugzilla 1949093 (gfxtest), 1762384 (VA-API), 1833354 (V4L2)
- Mesa: relnotes 26.1.0; `meson.options`; `src/virtio/virtio-gpu/drm_hw.h`;
  `src/virtio/vdrm/vdrm_virtgpu.c`; `pipe_loader_drm.c`; MR !29870 (Intel),
  !21658 (amdgpu), !28777 (pipe-loader)
- virglrenderer: `src/drm/i915`, `meson_options.txt`, MR !1384
- QEMU virtio-gpu-Doku (Versionsmatrix native context)
- Alpine aports `main/mesa/APKBUILD` (3.24-stable, edge)
- github.com/cmspam/xe-native-context-enablement (Xe + iHD ausserhalb)
- virtio-media: github.com/chromeos/virtio-media, Treiber v8 (2026-08),
  virtio-spec Issue #214; crosvm-Buch „video"
- Phoronix: AMDGPU VirtIO native in Mesa 25.0, ANV AV1, ANV VP9, RADV VP9
