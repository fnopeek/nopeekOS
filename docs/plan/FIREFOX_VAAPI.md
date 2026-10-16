# Firefox + VA-API (iHD) in der microVM — Plan

Stand 2026-10-16. Entscheid Florian: LibreWolf-Image einfrieren, ein
eigenes **Firefox**-Image daneben (Befehl `firefox`, nicht `browser`), und
Video über den Weg, den Firefox und Chromium auf Intel im Normalbetrieb
gehen: **VA-API mit Intels Medientreiber iHD**. Vulkan Video über anv und
Mesas `intel-virtio-experimental` bleibt, wo es ist; beides ist aus gutem
Grund experimentell.

## Was steht und mitgenommen wird

Die Wirt-Seite ist für beide Wege dieselbe und bleibt:

- `gpu_intel_tgl` (ab 0.22.0): i915-uAPI als native context —
  GETPARAM inkl. `HAS_BSD`/`HAS_BSD2`/`HAS_VEBOX`, QUERY (Engine-Info mit
  VCS0, VCS2 als Video-Instanz 1, VECS0; Topologie; Speicherregionen),
  CONTEXT_CREATE mit Engine-Liste, VM_CREATE, execbuffer mit
  `I915_EXEC_BSD` + `BSD_RING1/2` und `I915_EXEC_VEBOX`, softpin.
  Adressräume/Clients/GEM-Kontexte wachsen (RIGID_LIMITS 2.1–2.8).
- Kernel 0.617.5: virtio-gpu native context, Blobs, Antwortpuffer je
  Kontext richtig auch bei wiederverwendeter Kontext-Id.
- Bewiesen: Vulkan Video dekodiert H.264/HEVC/VP9/AV1 auf den
  VCS-Engines (`npk-vkprobe`), Firefox' Vulkan-Weg spielt — aber mit
  Kopie je Bild, Rückfall auf Software bei Stockern, Flackern.

## Einfrieren

- Image `librewolf-0.9.2` (GitHub `assets/librewolf-0.9.2`, im Manifest
  als `microvm:userspace-sqfs` → `sys/microvm/userspace.sqfs`) bleibt
  unverändert, `browser` startet es weiter. Rückzugsstand
  `assets/librewolf-0.6.1` bleibt auch.
- Enthält gdb + musl-dbg (Diagnose); bleibt drin, weil eingefroren.

## Neues Image `firefox`

1. **Bundle**: `microvm-userspace/build.sh` mit `BUNDLE_NAME=firefox`,
   Paketliste ohne `librewolf`, mit `firefox` (Alpine; prüfen, ob
   mozjemalloc oder System-Allocator — LibreWolf hatte scudo),
   `intel-media-driver` (iHD), `libva`, `libva-utils` (`vainfo` als
   Probe), dazu wie bisher Mesa-Overlay, seatd, libinput …
   Eigenes Asset (z. B. `microvm:firefox-sqfs` →
   `sys/microvm/firefox.sqfs`) und **eigenes Home-Image** (Profil getrennt
   von LibreWolf).
2. **Kernel**: Intent `firefox` neben `browser` (`intent/mod.rs:2043`,
   Hilfe `intent/system.rs:1632`, `wasm/host_core.rs:2757`), Pfad des
   sqfs (`virtio_blk_pci.rs:111` `SQFS_PATH` ist heute fest) und des
   Home-Images je App; `update.rs:80` zweites `AssetSpec`;
   `install_data/assets/mod.rs:200` entsprechend. `boottime.rs:76`
   (`APP_EXEC_LINE`) je App.
3. **Init** (`microvm/linux/init/src/main.rs`): heute fest auf
   `/usr/lib/librewolf` (Vorlesen von libxul/omni.ja Z. 281, Autoconfig
   `librewolf.cfg` Z. 355-362, `gfxtest` Z. 364, `exec librewolf` Z. 397).
   Die App aus dem Image ableiten (Firefox: `/usr/lib/firefox`,
   Autoconfig über `defaults/pref/autoconfig.js` + `mozilla.cfg`).
   Prefs für den GPU-Fall: `media.ffmpeg.vaapi.enabled=true`,
   `media.hardware-video-decoding.force-enabled=true`;
   **kein** Vulkan-Video-force. `LIBVA_DRIVER_NAME=iHD`.

## Der eigentliche Port: iHD über virtio-gpu

iHD spricht direkt mit dem i915-Kerneltreiber (eigener Buffer-Manager
`mos_bufmgr`, `drmIoctl` auf `/dev/dri/renderD128`). Im Gast ist
`renderD128` aber virtio_gpu; iHD hat keinen virtio-Weg.

**Vorlage, 1:1:** Mesa `src/intel/dev/virtio/` + `src/virtio/vdrm/`
(Mesa 26.2.4, Tarball in `~/.cache/nopeekos/mesa/`). Mesa leitet dort
jedes i915-ioctl, das auf ein virtio-fd geht, in `intel_virtio_ioctl`
um (`intel_gem.h` `intel_ioctl` → `is_intel_virtio_fd`):

| i915-ioctl | virtio-Weg in Mesa |
|---|---|
| GETPARAM, QUERY, CONTEXT_CREATE(_EXT), CONTEXT_GET/SETPARAM, VM_CREATE/DESTROY, GET/SET_TILING, SET_DOMAIN, GET_APERTURE, REG_READ, GET_RESET_STATS | ccmd über vdrm (`i915_virtio_ccmd.c`) |
| GEM_CREATE(_EXT) | `RESOURCE_CREATE_BLOB` mit ccmd |
| GEM_MMAP_OFFSET / mmap | `VIRTGPU_MAP` + mmap (`intel_virtio_bo.c`) |
| EXECBUFFER2 | `vdrm_execbuf` mit in/out syncobj (`i915_virtio_ccmd_execbuf.c`) |
| GEM_WAIT / BUSY | `VIRTGPU_WAIT` |
| PRIME, SYNCOBJ_* | virtgpu-Kernel direkt |
| USERPTR | nicht möglich (ENODEV) — prüfen, ob iHD es braucht |

**Wie einbauen** (zu entscheiden im neuen Chat, Vorschlag zuerst):

- **(A) iHD patchen** wie Mesa sich selbst: in `mos_bufmgr` die ioctl-
  Aufrufe durch eine `intel_ioctl`-gleiche Weiche ersetzen und Mesas
  `intel/dev/virtio` + `vdrm` als Bibliothek mitbauen. Gleiche Semantik
  wie iris/anv, eine Stelle.
- (B) Shim per `LD_PRELOAD` auf `drmIoctl`/`mmap`. Weniger Patch, aber
  mmap-Abfangen ist heikel → eher nicht.

**Mitzunehmen aus heute** (Mesa-Fehler, die iHD-Schicht nicht erben darf):
der Sync-Provider gehört dem `intel_virtio_device`, nicht dem Gerät/Kontext
(`microvm/linux/mesa/patches/anv-virtio-sync-ownership.patch`); die
execbuffer-Antwort liest Mesa nie (`vdrm_execbuf`) — Ablehnungen sind still,
das Modul stellt deshalb zurück statt abzulehnen (W1).

**Zu prüfen beim Port gegen Linux:**
- `I915_EXEC_BSD_DEFAULT`: Linux verteilt per Datei reihum auf die
  VCS-Engines (`gen8_dispatch_bsd_engine`), unser Modul nimmt immer VCS0.
- iHD nutzt ggf. `I915_PARAM_HAS_EXEC_SOFTPIN`, `HAS_CONTEXT_ISOLATION`,
  `I915_CONTEXT_PARAM_SSEU`, perf/OA-Abfragen, `HUC_STATUS` — jede Antwort
  muss Linux entsprechen (HuC: wir laden keine HuC-Firmware; Linux meldet
  dann ENODEV/0, iHD fährt ohne HuC — TGL-Dekodierung geht ohne HuC, Teile
  der Encoder nicht).
- Engine-Info: VCS2 als Video-Instanz 1 nur, wenn die Fuses ihn freigeben.

## Offen aus der Vulkan-Phase (gilt für jeden Weg)

- `client slot … an earlier batch wrote through an unbound address`
  während Firefox-Video (Slot des Dekoders). Meldung sagt nicht welcher
  Ring/Batch — ausbauen, dann Ursache (fehlende Bindung bei uns oder
  Null-Adresse im Treiber).
- Gast-vCPU bei Video `guest=98%` — wer rechnet, `npkload` zeigt es.
- Keine Preemption/Timeslicing (RIGID 5.6): Desktop- und Gastarbeit
  warten aufeinander.
- Gast-Konsole über 8250: ein VM-Exit je Byte; Diagnose-Logs (MOZ_LOG)
  stören die Wiedergabe selbst. Linux/crosvm: virtio-console.
- RIGID 2.9 (Wirt `MAX_CONTEXTS` 64), WebGL „currently disabled“ (Grund
  offen), Mesa-Patch upstream melden.

## Werkzeuge, die es schon gibt

`npklogs`, `npkenv=NAME=wert`, `npkapp=NAME`, `npksupport` (about:support
nach 20/90 s), `npkload`, `npkgdb` / `npkgdb=SEKUNDEN` (gdb-Stacks des
Dekoder-Prozesses; nur im LibreWolf-Image, gdb ist dort drin),
`INTEL_DEBUG=perf` (Mesas vk_error-Meldungen), Mesa-Symbole in
`~/.cache/nopeekos/mesa/symbols/` (Offset = pc − gdb-„From“ +
`.text`-Adresse, `addr2line -f`).
