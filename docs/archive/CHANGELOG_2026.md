# CHANGELOG 2026 (Archiv)

Der Session-für-Session-Verlauf, der bis 2026-08-11 im `Current Status`-
Abschnitt von `CLAUDE.md` stand — von Phase 12.1 (VT-x-Bringup, 2026-05-01)
bis Phase 12.7 (Spell-Editor, 2026-05-29). Aus CLAUDE.md ausgelagert, weil
er dort in jeden Session-Kontext geladen wurde, obwohl er drei Monate
hinterherlief.

Ab 2026-05-29 führt das Memory (`memory/MEMORY.md` + die `project_*.md`)
den Verlauf weiter; die harte Wahrheit ist der git log.

---


- **Phase:** **12.7 — Apps: Text-Editor "Spell" + Widget-Plattform-Ausbau
  (2026-05-29, kernel v0.180.1, spell 0.5.3, loft 0.3.1, dock 0.3.9,
  nopeek_widgets 0.11.0).** Eigener Text-Editor als WASM-Widget-App, plus
  die Plattform-Bausteine, die er gebraucht hat. Voll im Memory:
  `memory/project_editor_kickoff.md` (+ `project_file_associations.md`,
  `project_widget_app_caps.md`).
- **2026-05-29 (sehr lange Session, kernel v0.175 → v0.180.1): Spell-Editor
  + Widget-Plattform.**
  - **`Widget::TextArea`** ins ABI (mehrzeilig, compositor-eigener 2-D-
    Cursor: Pfeile/Enter/Home-End/PageUp-Down, Tab = 4 Leerzeichen). Plus
    **`Span { start, len, token }`** + `TextArea.spans` → **Live-Syntax-
    Highlighting beim Tippen** (App tokenisiert, Compositor malt farbige
    Runs über den Live-Buffer). Pfad B (Input pro Zeile) verworfen: keine
    programmatische Fokus-Host-fn.
  - **Spell-App** (`tools/wasm/spell`): loft-Design, **Tabs** (VS-Code:
    schon offene Datei → vorhandener Tab), Markdown-Vorschau + Mehrsprach-
    Highlight (Rust/C/JS/Py/Shell/JSON/HTML), Öffnen/Speichern via npkFS,
    „Speichern unter"-Namensdialog, **„Ungespeicherte Änderungen"-Guard**
    beim Schließen, echter Dateipfad im Footer.
  - **Dateizuordnungen**: `npk_open(app, arg)` + `npk_launch_arg()` +
    `Event::Open` → loft-Doppelklick öffnet die Datei mit der Handler-App
    (Endung→App in loft + `sys/config/associations`, nicht im Kernel).
    Singleton-Routing: 2. Öffnen → Tab in der laufenden Instanz (Fenster
    synchron angelegt gegen Doppelklick-Race). Widget-Fenster tragen jetzt
    den **Modulnamen** als Titel (fixt latenten re-focus-Bug + Bar zeigt
    App-Namen).
  - **Per-App-Capabilities**: App deklariert Rechte in 1-Byte `.npk.caps`-
    Section; Kernel vergibt genau die (Default ohne Section: READ|EXEC|
    RENDER, **nie** WRITE). Ersetzt das pauschale WRITE. **Sicherheit:**
    Apps dürfen nicht nach `sys/wasm/` schreiben (sonst WRITE→Modul-
    Überschreiben→Eskalation). Neue Host-fn `npk_home_dir` (Username liegt
    nur im `.system/config`-Blob, nicht als `sys/config/name`-Objekt).
  - **Zwei Bump-Allocator-Fallstricke** (gelten für JEDE loft-Loop-Widget-
    App, in Memory als feedback gesichert): Szenen-Commit IMMER via
    `wire::encode` (sonst leeres Fenster); Event-String (Open/InputChange)
    VOR `alloc_reset` in statischen Puffer kopieren (sonst use-after-free →
    verfälschte Pfade → doppelte Tabs).
  - **Offen (morgen):** Text-**Markieren** (Maus-Drag + Shift+Pfeil; braucht
    Modifier-Durchreichung durch `handle_input_key`, schaltet auch Ctrl+S
    frei), Suchen/Ersetzen, Cursor-Zeile:Spalte im Footer, Tab-Drag-Reorder.
- **Phase (vorher):** **12.6 + Shell-Polish ✅ — Panel-System (Bar + Dock als
  WASM) + Browser daily-driver (2026-05-26, kernel v0.173.37, bar
  0.1.10, dock 0.2.5).** Top-Bar lebt jetzt als `bar.wasm` (Workspaces-
  Pills + Titel + zentrierte Uhr + Tray/Power), das Dock als `dock.wasm`
  — beide über die Widget-ABI mit **echtem Alpha-Compositing**
  (transluzente Panels, scharfe Glyphen, kein Halo; Stack-Root =
  Panel-Szene). Theme `auto` (folgt Wallpaper-Luminanz), Eklogit-
  Wallpapers (npk01 hell / npk02 dunkel). Details im 2026-05-26-Eintrag.
- **2026-05-26 (lange Session, kernel v0.172.x → v0.173.37): Shell-
  Polish — Panel-System, Theme, OTA/Installer-Fixes.**
  - **Dock-Optik** (v0.173.6 → .17): chromeless flacher transluzenter
    Tray, schwebend (Bottom-Gap), zentrierte Icons (Root-Padding-
    Doppel-Bug umgangen — `[[project-widget-root-padding-bug]]`),
    grauer Collapsed-Balken statt Pille, **Desktop-leer-Reveal**
    (kein Fenster → Dock sticky sichtbar).
  - **Bar → WASM** (Panel-System P1, docs/spec/PANEL.md): `npk_window_set_panel
    (edge, behavior)` (Dock = Bottom/AutoHide-Wrapper, Bar =
    Top/Strut), Host-Fns `npk_bar_state`/`npk_workspace_switch`/
    `npk_power`, `bar.wasm` mit config-Segmenten (`sys/config/bar`).
    Native ShadeBar nur noch Strut-Geometrie + Fallback. Uhr
    bildschirm-zentriert via Stack-Layer. Workspace-Pills (Kapsel,
    aktiv = Accent + OnAccent-Text).
  - **Alpha-Compositing** (v0.173.36/.37, `[[project-widget-alpha-
    panels]]`): `blend_over` alpha-erhaltend (opake Szenen identisch),
    Stack-Root → transparenter Clear + `bg_alpha` → transluzente
    Panels + scharfe Glyphen, **kein weißes Halo**. Setzbare Textfarbe
    (`Modifier::Tint` auf Text). Bar UND Dock auf einem Pfad.
  - **Resident-App-Core-Pinning gefixt** (v0.173.24, `[[project-
    resident-app-core-pinning]]`): `npk_sleep` führt während der
    Wartezeit Scheduler-Arbeit aus (statt Busy-Spin) → 2 residente
    Panels (Dock+Bar) verhungern Intents nicht mehr.
  - **OTA + Installer**: DiskFull-Fix (gc + altes Asset vor Stream
    freigeben, `[[project-ota-asset-diskfull]]`), Installer-ESP-Image
    skaliert mit kernel.efi (272 MB Bundle-Build), Fresh-Install
    seedet `autostart=dock bar` + `theme=auto`.
  - **Eklogit-Wallpapers** (`[[project-codename-eklogit]]`): npk01
    (hell) + npk02 (dunkel) gebündelt, `theme=auto` folgt der
    Luminanz.
- **2026-05-22 (Monster-Session, kernel v0.172.63 → v0.172.83,
  Standard LibreWolf-Config (e10s + fission +
  Sandboxes alle wieder default), 2 GiB Guest-RAM (B3 demand-paging
  + B4 multi-PD in main), `browser` Top-Level-Intent + Drun-Eintrag
  "Browser" (via `npk_run_intent` Host-Fn), D4 live-resize (EDID-
  Emulation + Disconnect/Reconnect-Cycle), Cursor-in-Shadow (kein
  Flicker mehr über Tile), `./build.sh usb-full /dev/sdX` Modus
  bundelt 261 MB LibreWolf-sqfs in Installer (USB ~290 MB, kein OTA
  nötig für ersten Boot). YouTube AV1 + Audio + Multi-Site live
  bestätigt.
- **2026-05-22 (Monster-Session, kernel v0.172.63 → v0.172.83,
  microvm-init 0.4.14 → 0.4.21): Bare-Metal-Intel-Browser + Profil-
  Persistenz + komplette 9p-npkFS-Brücke.**
  - **Bare-Metal NUC (Intel-VMX) BOOTET JETZT** — LibreWolf läuft auf
    echter Hardware. Die exit-reason-33-Wände waren NICHT die
    IA32E/CR/EFER-Triade (Dump bewies: konsistent) sondern
    **ungegatete Interrupt-Injection** (datengetrieben gefunden via
    per-exit CS.L/LMA-Diag + Match gegen echte KVM-Quelle in
    `~/.cache/nopeekos/linux-src/`): (1) v0.172.76 — D4-virtio-gpu-IRQ9
    in die winzige Boot-IDT (32 Gates) → Gate auf `pic.irq_unmasked(9)`;
    (2) v0.172.77 — Timer-IRQ0 in Guest mit IF=0 (mid i8042-poll) →
    alle 5 reason-1-Injects auf neues `vmcs::guest_interruptible()`
    gegated (= KVM `vmx_interrupt_allowed`). **Durable Lektion: jede
    VMX-vs-SVM-Divergenz war „AMD-VMRUN nachsichtig, Intel strikt" → bei
    reason-33 nach reason-1/48 zuerst ungegatete Injection verdächtigen.**
    Intel-Perf noch suboptimal (kooperativer Core-0-Pfad, A2-dedicated-
    core für Intel un-gaten ist geparkt — „bearmetal später").
  - **Profil-Persistenz (Deliverable 1, QEMU/AMD validiert)** — die
    profile.img-virtio-blk-Mechanik vergrößert auf **64 MiB ext4
    Home-Image** (`sys/microvm/apps/browser/home.img`), auf npkFS-Miss
    aus einem **eingebetteten sparse-ext4-Template** geseedet
    (`home_template.bin`, kein mke2fs im Guest, kein inflate im Kernel).
    PID-1 mountet `/dev/vda` ext4 RW an `/tmp/moz` (NICHT `/home/...` —
    squashfs-Root ist RO → EROFS-Panik). **`save()` in `close()`**
    (war nur am run_slice-Loop-Ende → Mod+Q persistierte nie). Cache →
    tmpfs (Image bleibt klein), glean-Telemetrie → tmpfs-Symlink (sonst
    StorageFull). Close-Button via `halt` (NICHT `reboot` — reboot riss
    den ganzen Host runter). Session-Restore statt CLI-Start-URL.
  - **9p-npkFS-Brücke (Deliverable 2, Read+Write validiert)** — neuer
    host-seitiger **virtio-9p (slot 6, `devices/virtio_9p_pci.rs`)** +
    9P2000.L-Server, gewurzelt + CONFINED auf `home/<user>/`. Kein
    Guest-Rebuild (CONFIG_NET_9P_VIRTIO=y). Read: attach/walk/getattr/
    readdir/lopen/read (Browser zeigt npkFS-Dateien via
    `file:///tmp/npkhome/`). Write: Tlcreate/Twrite/Tfsync/Tsetattr/
    Tmkdir/Tunlinkat/Trenameat — **Downloads landen in npkFS
    `home/<user>/downloads`, sichtbar in loft** (`browser.download.dir`).
    Trenameat musste POSIX-overwrite (Firefox `.part`→final). **Capstone
    (v0.172.83):** `file:///tmp/npkhome/.open-in-loft` (synthetische
    Magic-Datei) → Host spawnt loft (`shade::launch_app` via Core-0-
    Atomic-Handoff). `.ssh`/`documents` sind jetzt nur weitere Mounts.
  - **Offen:** File-Open/Save-Dialog (GTK-Chooser rendert evtl. nicht im
    cage-Kiosk → evtl. XDG-Portal nötig), „open in folder" Cross-
    Boundary, loft-Start-Pfad für exakten Ordner, Intel-Perf (A2),
    p9diag-Logs strippen. Voll in `memory/project_browser_v1.md`.
- **2026-05-21 (sehr lange Session, kernel v0.172.54 → v0.172.63,
  microvm-init 0.4.8 → 0.4.14):**
  - **D4 live-resize end-to-end**: virtio-gpu EDID-Emulation
    (v0.172.54) + Disconnect/Reconnect-Cycle (v0.172.55). Linux's
    virtio_gpu_cmd_get_edid_cb ruft drm_kms_helper_hotplug_event
    UNCONDITIONAL → wlroots/cage reagiert. Erfordert echte
    Disconnect→Reconnect-Sequenz weil wlroots mode-set nur bei
    connector-up macht. 100 ms-Gap, R2 debounce 25 ticks.
  - **RAM bump 1 → 2 GiB** (v0.172.56): B3 demand-paging ON
    (`DEMAND_ENABLED=true`) + B4 multi-PD (PDPT[0..num_pds], Cap
    3 GiB, PDPT[3] reserved für MMIO). Vendor-symmetric. Browser-
    Freeze auf example.com war 1-GiB-Cap + #11-Cripples, nicht
    Netzwerk.
  - **user.js auf wirklich standard** (microvm-init 0.4.13): alle
    #11-Cripples raus (e10s, fission, network-process, content-
    sandbox, OCSP, SW-render-robustness-Prefs, MOZ_DISABLE_*_
    SANDBOX env vars). Behalten nur env-required (no-GPU-process,
    software-webrender) + dark-mode + userChrome.css enabler.
  - **App-Surface**: `browser` Intent (v0.172.57) + `npk_run_intent`
    Host-Fn (v0.172.58) + Drun EntryKind::Module/Intent dispatch
    (drun 0.6.1). Drun listet Browser, click → microvm startet.
    Zukünftige Apps plug in via Match-Arm + Drun-Eintrag.
  - **Cursor-in-Shadow** (v0.172.60): Cursor wird Teil des
    shadow→MMIO-Blits statt separater Post-Blit-MMIO-Write.
    Eliminiert Race über 60-Hz-Surface-Tile, Flicker weg. xhci
    IRQ macht nur noch `update_atomic` + `request_render()`.
  - **Quiet boot + Log-Strip** (v0.172.59): `quiet loglevel=3`
    in Cmdline + `[gpu]`/`[nat]`/`[virtio-blk]`/`[virtio-net]`
    Per-Event-Spam raus. ~150 → ~15 Lines pro Launch. (Temporär
    in v0.172.61 reverted für bare-metal-Diag.)
  - **USB-full Modus**: `./build.sh usb-full` + Cargo-Feature
    `bundle-userspace` + cfg-gated BundledAsset → 261 MB
    LibreWolf-sqfs im Installer. USB ~290 MB.
  - **A2 vendor-gated** (v0.172.62): AMD bleibt dedicated VM-core
    (validiert), Intel cooperative-Core-0-Fallback. VMX-A2 audit
    pending.
  - **VMCS-Entry-Fail-Dump** (v0.172.63): Diag-Hook für reason
    33/34/41 dumpt CR/EFER/ENTRY_CTLS/CS/RIP/RSP/VM_INSTR_ERR.
    Next-session-Tool.
  - **Wichtige Erkenntnis**: SWGL `RenderCompositorSWGL failed
    mapping default framebuffer` Warning IST NICHT der historische
    Crash. War durch `cage … >/dev/null` versteckt, microvm-init
    0.4.14's Diag-Tap (cage.log + Watchdog) hat ihn erstmals
    sichtbar gemacht. Browser läuft visuell trotz Spam.
  - Vollständige Iterations-Historie + Lessons:
    `memory/project_browser_v1.md`.
- **2026-05-16 (sehr lange Session — 12.4e' → Phase B → LibreWolf,
  kernel v0.170.0 → v0.171.4):**
  - **12.4e' Maus**: absoluter Pointer (qemu-usb-tablet-Modell,
    EV_ABS 0..32767 + EV_REL-Wheel + BTN_* in `virtio_input_pci::
    fill_ev_bits`, ABS_INFO), `shade::forward_pointer_to_guest`.
    5 Bugs: `as u8`-Bitmap-Overflow, poll_render-drain-Order,
    unfokussiertes Fenster, **Core-0 poll_mouse Multi-Consumer-
    Race** (Kern-Fix: forward aus `handle_mouse` — jeder Consumer
    ruft das), Diags gestrippt. HW-validiert.
  - **Phase B**: cage-Kiosk-Compositor + Bundle-Pipeline. weston→
    cage-Pivot (Alpine-weston zog mesa/gstreamer/pipewire = 339 MB;
    cage = wlroots, schlank + Ziel-Topologie ein-Surface-pro-Tile).
    `microvm-userspace/build.sh`: apk → gzip-sqfs, **mksquashfs-
    Schritt neu** (PID-1 lädt sqfs via /dev/vdb, nicht cpio; gzip
    Pflicht — zstd #PFt auf Stack-Canary), `release-large` Repo-
    Parse host-agnostisch + RELEASE_DIR/KEY_FILE/.sqfs-Case
    gefixt (Branch nie zuvor gelaufen).
  - **Schicht-Kette userspace** (jede via Log iteriert): RO-/run-
    chmod → XDG_RUNTIME_DIR=/tmp/xrt; Alpine-libseat ohne builtin
    → seatd-Daemon; seatd-Socket-RO + kein -s-Flag → tmpfs über
    /run; PID-1 auto-fokussiert das Surface-Fenster beim Launch.
  - **Drei Keystone-Kernel-Fixes**: `v0.171.0` Gast-Timer-IRQ0
    (~100 Hz aus EXIT_INTR/reason-1, svm+vmx; ohne Timer hängt
    *jedes* zeitbasierte Userspace — die Wurzel); `v0.171.1`
    keine VMRUN-Lifetime-Cap für fenstergebundene VMs (60-fps-
    Compositor sprengte 100 000 Iters → Teardown); `v0.171.3`+
    `v0.171.4` **zwei stale-256MB-Zwillinge** (`guest_mem` +
    `guest_fetch` `GUEST_RAM_BYTES`) vom 256MB→1GB-Bump übersehen
    → virtio-DMA/MMIO-insn-fetch ≥256 MB still verworfen → sqfs-
    Korruption / NPF. **Gast-RAM-Größe ist an 5 Stellen kodiert**
    (ept/npt/bzimage/guest_mem/guest_fetch) — alle synchron halten.
  - **Bundle `librewolf-0.1.1`**: + `font-dejavu`/`font-noto`,
    `mksquashfs -all-time 0` (Gast hat keine RTC → Jahr-2000-Uhr;
    2026-mtimes „in der Zukunft" → fontconfig scannte nicht →
    Tofu), `fc-cache -f` im chroot vorberechnet.
  - **Resultat**: LibreWolf rendert lesbar im Tile. Display-
    Bridge (`WindowKind::Surface`, `forward_pointer_to_guest` aus
    `handle_mouse`, render-on-FLUSH) + virtio-input-Keyboard
    (v0.169.3) bleiben gültig. Vollständige Iterations-Historie:
    `memory/project_phase_b_wayland.md` +
    `memory/project_microvm_pointer_race.md`.
- **2026-05-11 spätestens — 12.4 (virtio-gpu 2D scanout) NUC-validated:**
  - Neuer `microvm/devices/virtio_gpu_pci.rs` (~650 LoC): PCI slot 3,
    vendor 0x1AF4 device 0x1050 class 0x03_80_00, IRQ 9, BAR0 @
    0xFE00_8000, modern cap chain. 1 scanout @ 1280x720, 0 capsets.
    Zwei virtqueues (controlq + cursorq).
  - Control queue commands: GET_DISPLAY_INFO, RESOURCE_CREATE_2D /
    UNREF, RESOURCE_ATTACH_BACKING / DETACH_BACKING, SET_SCANOUT,
    TRANSFER_TO_HOST_2D (page-list walk → host_pixels buffer),
    RESOURCE_FLUSH (hex-preview log throttled nach 5). Cursor queue
    → OK_NODATA stub.
  - vmx/svm enable.rs: handle_mmio_{ept,npf}_gpu mirror.
  - **Zwei kritische Linux-Config-Flags** die wir erst durch trial-
    and-error gefunden haben:
    1. `CONFIG_DRM_VIRTIO_GPU_KMS=y` — sonst compiliert virtio-gpu
       ohne 2D-scanout-Code. `virtgpu_kms.c:228` forciert dann
       num_scanouts=0 + loggt "KMS disabled". Ich hatte den Flag
       initial fälschlich für virgl/3D gehalten und disabled.
    2. `CONFIG_FRAMEBUFFER_CONSOLE=y` — sonst hängt die crtc im
       disabled state. fbcon's vt-init triggert den initial atomic
       commit der SET_SCANOUT auslöst. Ohne fbcon bleibt /dev/fb0
       ein In-RAM-Buffer ohne dass die Pixel je zu uns gepusht
       werden.
  - PID-1 v0.3.8: `fb_write_pattern()` macht FBIOGET +
    FBIOPUT_VSCREENINFO (belt+suspenders falls fbcon's auto-modeset
    nicht greift), dann 64 KB BGRA rot zu /dev/fb0. Default-route fix
    in 0.3.5, http_get_ip mit direkter IP in 0.3.6.
  - **Resultat NUC-validated**: fbcon rendert Linux-Boot-Messages live
    auf 1280x720 Framebuffer (160×45 chars @ 8×16 font). Mehrere
    TRANSFER + FLUSH cycles während fbcon den Text-Modus aufbaut. PID-1
    schreibt zusätzlich 64KB rot, durchläuft komplett. Wire-Protocol
    end-to-end validiert.
  - **TODO Polish**: Shade-Window-Integration (FLUSH → tatsächliches
    Rendering in shade-Compositor-Surface statt nur Hex-Log), cursor
    queue rendering, virgl/3D für später (12.6+).
- **2026-05-11 spät — 12.3.4 NUC end-to-end (v0.156.0 → v0.156.6), 6 iter:**
  - **v0.156.1** IPv4-src/dst-Direction-Fix: TCP-Replies hatten IPv4
    src=GATEWAY_IP statt target_ip und dst=target_ip statt GUEST_IP.
    Linux filterte sie weil dst != eigene IP.
  - **v0.156.2** Pump deadlock-safe via snapshot-pattern: SESSIONS-lock
    droppen vor `tcp::recv`-call (das CONNECTIONS lockt), sonst potentieller
    Lock-Order-Deadlock mit NIC-IRQ-Pfad. Plus pump-heartbeat-Logs alle 2s.
  - **v0.156.3** Target umgezogen auf 1.1.1.1:80 (Cloudflare DNS-Portal,
    zuverlässiger als example.com). Heartbeat erweitert um
    `in_flight`/`buffered` via neuer `net::tcp::debug_progress()`.
  - **v0.156.4** **`net::poll()` als erste Zeile im pump**. Intel I226-V
    ist polling-driver (keine IRQ → handle_frame Verbindung). Während
    VM läuft ruft niemand poll(), Response-Pakete stapeln sich im NIC-Ring.
    Heartbeat zeigte `in_flight=52 buffered=0` permanent → smoking gun.
  - **v0.156.5** virtio-net `num_buffers=1` im virtio_net_hdr (per virtio
    1.2 §5.1.6.4.1 bei VERSION_1 ohne MRG_RXBUF). Spec-compliant, half
    nicht alleine, aber notwendig.
  - **v0.156.6** **`inject_rx` setzt `self.isr |= 1`** nach erfolgreichem
    write. **Das war's**. Bisher nur `service_tx` setzte ISR; Pump
    bypassed das. Linux's IRQ-Handler liest ISR (W1C), sieht 0 → "spurious"
    → kehrt zurück ohne RX-Queue zu drainen. ARP/SYN-ACK/bare-ACK kamen
    durch weil sie über service_tx liefen. Pump bypassed service_tx → ISR
    blieb 0 → Linux ignorierte den IRQ.
  - **Resultat**: 381-byte Cloudflare-Response geht durch, Linux ACKed
    (tx#6), Linux FINed (tx#7), PID-1 read returnt mit 381 bytes, kmsg
    loggt `HTTP/1.1 301 Moved Permanently`. Voller HTTP-Roundtrip in
    ~5 ms guest-time nach connect.
  - PID-1 v0.3.5: SIOCADDRT default route 0.0.0.0/0 via 10.99.0.1
    (sonst connect()→ENETUNREACH zu non-on-link IPs).
  - PID-1 v0.3.6: http_get_ip(host, [1,1,1,1]) statt http_get(host) +
    DNS, isoliert TCP-NAT vom DNS-Pfad.
  - TODO 12.3.5+: TLS-Termination (für Firefox HTTPS), Multi-Session
    Concurrency-Test, Retransmission. Genauer Plan via picker → 12.6
    Firefox-Userspace-Bundle Strategie.
- **2026-05-11 — Phase 12.3.4 (TCP-NAT + HTTP GET), v0.156.0:**
  - **TCP-NAT in nat.rs** (~430 LoC neu) — Termination-Style:
    Guest-side virtuelles TCP-Endpoint auf 10.99.0.1:<dst>, gegenüber
    eine frische host-`net::tcp::connect`. State pro Session
    {guest_port, target_ip, target_port, host_handle, snd_nxt, snd_una,
    rcv_nxt}. Table mit 4 Slots (Mutex array).
    Lifecycle: SYN → blocking host connect (~100-500ms, VM eh paused)
    → synth SYN+ACK mit MSS-Option (1400) → guest ACK → Established.
    Daten guest→host via `tcp::send`. FIN guest→host close + synth
    FIN+ACK + Closed.
  - **Pump aus Timer-Tick** — `nat::pump(net, host_base)` called bei
    jedem `EXIT_INTR` / reason-1 in vmx + svm enable.rs. Drains
    `tcp::recv` für jede aktive Session, baut TCP-PSH-ACK-Segmente,
    inject via `virtio_net.inject_rx` (jetzt `pub(super)`). Bei
    Erfolg IRQ 10 inject. Erkennt host-side close → synth FIN+ACK +
    Closed. Idle-Counter resetted wenn `active_session_count() > 0`
    — sonst würden Sessions in der Idle-Timeout-Falle landen.
  - **NetCaps default** dns_only() → dns_tcp() — DNS + TCP an,
    ICMP + raw UDP weiterhin cap-reject.
  - **PID-1 v0.3.4** — `http_get("example.com")` macht: dns_query
    (jetzt mit `Option<[u8;4]>` return), SYS_SOCKET SOCK_STREAM,
    SO_RCVTIMEO 5s, SYS_CONNECT zu ip:80, SYS_WRITE der minimal
    HTTP/1.0-GET-Zeile, SYS_READ erste 256 bytes, kmsg-log erste
    Zeile ASCII-sanitized. Refactor: `parse_dns_a` (pure) +
    `log_dns_result` (formatting only).
  - **TODO / Limitationen v1**: TCP-Checksum-Calc unsegmentierte
    Pakete only (PSH+ACK segments mit Payload &lt; MAX_SEG_PAYLOAD).
    Keine Retransmission. Out-of-order Daten gedroppt. RST-Pfade
    pessimistisch (jeder unbekannte 4-Tuple → RST). 4 Sessions max.
    Reicht für HTTP-Smoke-Test; Browser-Multi-Connect kommt in
    12.3.4b wenn Firefox da ist.
- **2026-05-11 — Phase 12.3.3 (NAT + Cap-Filter), v0.155.0:**
  - Neuer `microvm/devices/nat.rs` (~310 LoC) — synthetic gateway-Logik
    raus aus `virtio_net_pci.rs`. ARP-Reply + IPv4-Dispatch + DNS-
    Shortcut + Builder für `build_ipv4_udp_reply` + IPv4-Checksum.
    Konstanten `GUEST_MAC/GATEWAY_MAC/GATEWAY_IP/GUEST_IP` zentralisiert.
  - `NetCaps { allow_dns/icmp/udp/tcp }` mit `dns_only()` default.
    Cap-rejects loggen erste 8 dropped flows pro VM-run, dann silent.
  - **DNS-Shortcut** — guest UDP→10.99.0.1:53 → `parse_dns_query` →
    `crate::net::dns::resolve(qname)` (cache + real upstream) →
    `build_dns_reply` (A-record + compression pointer + TTL 60) →
    `build_ipv4_udp_reply` → RX-inject. NXDOMAIN-Fallback (rcode=3)
    falls qtype != A oder resolve None.
  - **PID-1 v0.3.3** — `udp_poke` raus, `dns_query("nopeek.ch")` rein:
    baut handgemachte DNS-A-Query (header + QNAME labels + qtype/qclass),
    setsockopt SO_RCVTIMEO 2s, sendto + recvfrom, parst erste A-Record
    aus dem Reply (compression-pointer-aware skip_dns_name) und kmsg-
    logged `DNS nopeek.ch -> a.b.c.d`. Drei neue Syscall-Nummern
    (SYS_RECVFROM=45, SYS_SETSOCKOPT=54) + SOL_SOCKET/SO_RCVTIMEO.
  - `push_dec` Signatur erweitert von `&mut [u8; 256]` zu `&mut [u8]`
    damit der 128-Byte-Output-Buffer in `log_dns_result` passt.
  - **TODO 12.3.4**: TCP-NAT-Sessiontable für curl/HTTPS — der jetzige
    Cap-reject droppt SYN-Pakete, also blockt curl wie erwartet.
- **2026-05-05 (sehr lange session, ~25 commits, v0.148.3 → v0.154.5):**
  - **Cleanup v0.148.4** — bootstrap WASM-Modules add/multiply/hello/fib
    raus (~140 LoC weg).
  - **Sequencing-Decision** — Microkernel-Refactor wandert von
    "zwischen 12.1 und 12.2" zu **nach 12.6 Firefox**. Code-Drift-Argument
    hielt nicht (Host-Backend ist Trap-and-Emulate, Guest-WASM-Driver ist
    Linux-spec-Client — teilen nur die Wire-spec). Time-to-Firefox: 4-6
    statt 6-9 Wochen.
  - **Spec-Update** — at-rest-AEAD ist AES-256-GCM (war ChaCha20 in der
    Spec, falsch — ChaCha lebt nur noch in TLS). Pattern B-mini
    (per-app-downloads-Subtree) vorgezogen in 12.5.
  - **Phase 12.2 (virtio-blk end-to-end), v0.149 → v0.153.1, ~10 commits:**
    - PCI-bus emu (slot 0 host-bridge, slot 1 virtio-blk)
    - BAR-sizing-Handshake (write 0xFFFFFFFF → size mask)
    - Modern virtio cap list (Common/Notify/ISR/Device cfg)
    - MMIO-BAR-trap mit Guest-Page-Walker für Inst-Fetch
      (decode-assists war auf KVM-nested-SVM nicht zuverlässig).
      Vendor-neutral, funktioniert auf VMX und SVM identisch.
    - VIRTIO_F_VERSION_1 + queue_size = MAX_QUEUE_SIZE
    - **Eigener Linux-Build** in `microvm-linux/` — defconfig + overlay
      nopeek-virt.config, VIRTIO_BLK=y/_NET=y/_PCI=y built-in, USB/Sound/
      DRM/HID raus. 9.5 MB bzImage statt Alpine's 12 MB. `bash
      microvm-linux/build.sh` lädt linux-6.18.26.tar.xz von kernel.org.
    - PKU+OSPKE in CPUID 7 ECX maskieren (XSAVE-consistency-check fix —
      sonst panic in `fpstate_reset`).
    - 8259 PIC stub (master/slave IMR readback, ICW1/2/3/4 sequence
      tracking) damit Linux's `request_irq` für virtio-pci INTx-fallback
      nicht -EINVAL zurückbekommt.
    - virtqueue-walker in `devices/virtqueue.rs`: split-virtqueue-Spec,
      avail-ring lesen, descriptor-chains folgen, used-ring schreiben mit
      release-fence. virtio-blk-spezifischer Service: 4 MB
      in-RAM-backing, IN/OUT/GET_ID/FLUSH-handler, status-byte writeback.
    - IRQ-Injection: VMX VM_ENTRY_INTR_INFO_FIELD, SVM VMCB.EVENT_INJ.
      8259-Stub ICW2 trackt Vector-Base damit IRQ 11 auf den richtigen
      Linux-Vector landet.
    - **Profile-Image-Persistenz**: `sys/microvm/profile.img` via npkFS
      auto-AES-256-GCM-encrypted (master_key), upsert-API (insert-or-
      replace). 4 MB save: enc 5.9 ms + BLAKE3 2.4 ms + NVMe-DMA 4 ms =
      ~26 ms. 4 MB load: ~9 ms.
    - PID-1 v0.3.0 erweitert: open(/dev/vda) + read(32 bytes) + hex+ASCII
      log. Magic-pattern "nopeekOS-microvm-blk\0+counter" überlebt zwei
      VM-runs (Run 1 = fresh, Run 2 = loaded → identical bytes).
  - **Phase 12.3 virtio-net (12.3.0–12.3.2), v0.154.0 → v0.154.5:**
    - virtio-net-pci device auf slot 2, VIRTIO_NET_F_MAC + _STATUS
      advertised, MAC `52:54:00:6E:70:6B`, GATEWAY_MAC
      `52:54:00:6E:70:01`, GATEWAY_IP `10.99.0.1`.
    - virtqueue.rs Helpers public (avail_idx/avail_ring/read_desc/used_push)
      damit virtio-net + virtio-blk dieselbe Mechanik teilen.
    - TX-Path: q1-notify → walk avail-ring → concat descriptor chunks →
      log eth+IP+L4-ports.
    - RX-Path: synth ARP-Reply für Gateway-IP — wenn TX ein ARP-Request
      für 10.99.0.1 ist, baue Reply mit GATEWAY_MAC, walke RX-q0-avail,
      schreibe in driver-buffer, used-ring update, IRQ inject.
    - PID-1 v0.3.2: SIOCSIFADDR + SIOCSIFFLAGS via ifreq[40] manuell
      (kein copy_from_slice → kein memcpy-link-error). UDP-poke an
      10.99.0.1:53 triggert ARP. Eth0 Bringup ohne Linux IP_PNP
      (das hängt in unserer microvm-env, late_initcall blockiert PID-1).
    - **End-to-End validated**: PID-1 sendto → Linux ARP-Request → wir
      antworten synthetisch → Linux's ARP-cache populated → echte UDP/IP-
      Frame mit GATEWAY_MAC als dst raus. Voll auf NUC-Hardware bestätigt.
- **Earlier (2026-05-05 — long session: SVM end-to-end + npkfs v3 +
  Popover + wallpapers, v0.142 → v0.148.3):**
  - **Phase 12.1 SVM end-to-end** (v0.142 → v0.143). Linux 6.18 bootet
    auf KVM nested SVM, PID-1 echo-roundtrip funktioniert. Drei Fixes
    auf dem Weg: MSRPM trap-all → pass-through (EFER LME muss durch),
    hypervisor-CPUID-leaf hide (kvm-clock divide-by-zero), 
    `tsc_early_khz=2000000` cmdline (AMD kein CPUID 0x15). Details
    in `memory/project_svm_bringup.md`.
  - **build.sh resource bump** (b2fd120) — qemu-RAM 256 MB → 1024 MB
    + disk.img 256 MB → 1024 MB (microvm linux brauchte mehr).
  - **Loft polish round 5** (v0.2.2 → v0.2.5): bump-allocator-state-
    mutation panic gefixt (alloc_reset BEFORE handle, mark recapture
    AFTER), neuer **`Modifier::Flex(u8)`** in SDK + kernel layout
    (CSS-style flex-grow für non-Spacer Children), magnifier 18 → 24
    px atlas-native, panel-padding raus für edge-to-edge sidebar +
    menu fill.
  - **npkFS Konsolidierung + v3 schema** (v0.145 + v0.146):
    - v0.145 — v1-leftover gelöscht (`btree.rs`, dead code), `v2/`
      subdir flach in `npkfs/` integriert, alle externen `npkfs::v2::*`
      → `npkfs::*` umbenannt (34 refs). Net –838 LoC.
    - v0.146 — schema-bump v2 → v3: `TreeEntry.mtime: u64` (UTC sec
      seit epoch, captured via `rtc::read_unix_time()`), magic
      `npkFS\x02\0\0` → `npkFS\x03\0\0`, mount-time guard für legacy.
      WASM ABIs erweitert: `npk_fs_list` 10 → 19 byte tail per record
      (mtime appended), `npk_fs_stat` 9 → 17 byte. Loft v0.2.4 nutzt
      mtime in der Modified-Spalte.
    - v0.146.1 — followup: loft's `dir_exists` checkte strikt `n == 9`,
      neue ABI gibt 17 → fix auf `n > 0`, npkfs2: → npkfs:
      log-strings.
  - **Echte Popovers** (Phase 11 vorgezogen, v0.147 + loft v0.2.5):
    - **`Modifier::NodeId(NodeId)`** — Widget-Tagging für Anchor-Lookup.
    - **`Widget::Popover { anchor, child, on_dismiss, modifiers }`** —
      finalised, floating layout an anchor-rect (auto-flip oben/unten).
    - Layout returnt jetzt `LayoutOutput { tree, anchors, popovers }`.
      Render: popovers drawn last (top z-order). Hit-test: popovers
      first (reverse-decl), click outside fires on_dismiss (außer auf
      anchor selbst — der toggled).
    - Loft v0.2.5: OpenMenu enum, **Ansicht** dropdown switched
      Grid/List view, **List view** mit Spalten Name/Size/Type/Modified
      (Modified via Howard-Hinnant civil_from_days, "YYYY-MM-DD HH:MM"
      UTC oder "—" bei mtime=0). Datei→Quit, Hilfe→About, Gehe zu
      →Home/Filesystem.
  - **Bundled wallpapers** (v0.148.0 → v0.148.3 + wallpaper v0.4.2):
    - `release/assets/wallpapers/<name>.png` ist die kanonische
      Source-of-Truth. build.sh Pass 2 staged jeden file in
      `install_data/assets/wallpapers/`. BUNDLED_ASSETS-Eintrag
      schreibt nach `sys/wallpapers/<name>` bei seed-time. setup.rs'
      `copy_system_wallpapers_to_user` kopiert nach
      `home/<user>/pictures/wallpapers/<name>` (idempotent — re-run
      clobbert keine umbenannten files).
    - Erstes wallpaper: `npk01.png` (downsized 4K → 1080p, 8.9 MB →
      1.3 MB; 4K-source dropte ~3-5 sec WASM-decode-time).
    - **Wallpaper module v0.4.2**: heap 64 MB → 256 MB (4K-decode
      OOM'd), max-fetch-buf 6 MB → 32 MB (truncierte 9 MB inputs →
      panic), idat-Vec mit `with_capacity(data.len())` pre-sized
      (verhindert ~16 MB doubling-leak im bump-alloc).
    - **`decode_with_wasm`** nutzt jetzt `INTERACTIVE_FUEL`
      (`u64::MAX/2`) statt heuristic — bundled+signed module hat
      keine DoS-surface, fuel-cap dort sinnlos.
  - **Vollständige Iterations-Historie** in
    `memory/project_microvm.md` + neuer `memory/project_npkfs_v3.md`
    + `memory/project_popover.md` + `memory/project_wallpapers.md`.
- **Earlier (2026-05-05 morning — SVM bring-up first push, v0.142 → v0.143):**
  - **v0.142.0 — 12.1.1c-svm Linux-Entry-Pfad** (+628 LoC):
    `enable::run_linux` + `run_linux_loop` + `setup_vmcb_linux` +
    `handle_linux_io` + `SerialState`, `npt::allocate_window_npt`
    (non-identity 256 MB + MMIO-scratch-Alias), VMCB-Konstanten
    (NRIP/CPUID/SHUTDOWN/MSR_PROT/IOIO_PROT/INTR). Substrate-Test
    smoke-validated post-refactor (exit=0x7B byte-identical zu v0.141).
  - **v0.143.0 — 3 Iterationen vom Compile zum echten Linux-Boot:**
    1. **MSRPM trap-all → pass-through** — trap-all absorbed Linux's
       `WRMSR EFER=LME` → CR0.PG ohne LME → legacy 32-bit paging →
       triple-fault nach 8 iters. Pass-through lässt CPU arch-state
       MSRs auto-via VMCB.SAVE handhaben (APM §15.11.1).
    2. **Hide hypervisor CPUID** — Leaf 1 ECX[31] cleared, Leafs
       0x4000_00xx zero. L2 Linux sah L1 KVMs Signature, aktivierte
       kvm-clock, divide-by-zero in `pvclock_tsc_khz` weil unser
       MSR-Handler die KVM_SYSTEM_TIME-Schreibe absorbierte.
    3. **`tsc_early_khz=2000000`** in Cmdline — AMD exposed kein
       CPUID 0x15, Linux fällt auf PIT-Calibration zurück, deadlocks
       gegen unsere Zero-Returning-IO-Emulation. Hint
       short-circuited das. Idle-threshold auch 200 → 5000 INTRs.
  - **End-to-end auf KVM nested SVM**:
    `[guest] [microvm-init] Hello from nopeekOS PID-1` →
    `[guest] [init] echo: hi-svm` → HLT nach 41355 VM-exits.
    Self-bestätigt durch User-Test auf AMD-Box.
  - **build.sh-Bump**: 256 MB → 1024 MB qemu-RAM + disk.img
    (256 MB-RAM OOM'd `microvm linux` weil 256 MB Guest-Window
    + Kernel + Heap nicht reinpasste).
  - **Vollständige Lessons** in `memory/project_svm_bringup.md`.
- **Earlier (2026-05-02 — late stragglers, freeze fix, panic detection, initramfs+pid1, v0.122 → v0.130):**
  - **v0.130 — initramfs + Rust-PID-1 (12.1.3).** Eigene `microvm-init`
    Crate (`microvm/linux/init/`, ~1.3 KB statisch gelinktes Linux ELF),
    no_std, no_main, raw syscalls (write/pause/reboot). Wird bei
    `./build.sh release` via `bsdtar --format newc + gzip` zu
    `release/assets/microvm-initramfs.cpio.gz` (694 Bytes), per ECDSA
    P-384 signiert, im Installer als `sys/microvm/initramfs.cpio.gz`
    in npkFS gepflanzt. `intent::microvm_linux` lädt's via npkfs::fetch,
    übergibt an `vmx::run_linux(bzimage, cmdline, initramfs)`. Loader
    in `bzimage::load_into_guest_ram` legt's bei Guest-Phys 0xC000000
    ab, setzt boot_params.hdr.ramdisk_image + ramdisk_size. Linux
    unpackt cpio → rootfs, exec'd /init. Erstes Userspace-Banner
    erwartet: "[microvm-init] Hello from nopeekOS PID-1".
  - **v0.129 — formal panic-detection (12.1.1d).** SerialState scant
    auf "Kernel panic - not syncing: ", erkennt Panic-Reason, klassifiziert
    den nachfolgenden triple-fault als erwartet. AMD-MSR-Spam-Filter
    daneben (LS_CFG/HWCR/NB_CFG werden auf Intel always-absent → kein
    Log).
  - **v0.128 — Pin-based external-interrupt-exiting fix.** Erster
    `microvm linux` froze NUC komplett (hard-reset nötig), weil
    Pin-based bit 0 = 0 → Host-LAPIC-IRQs gingen während Guest-Run
    direkt in Guest-IDT, mit echtem LAPIC-Acknowledge → ISR-stuck
    → Host-Tastatur/Timer tot nach VMXOFF. Fix: bit gesetzt, IRQs
    causen jetzt VM-exit reason 1, der `sti` am Ende von
    `run_guest_once` lässt den pending IRQ durch Host-IDT laufen.
    Architekturell wichtig: das war ein Host-Config-Bug, kein
    Guest-Escape — VMX-Hardware-Boundary hat gehalten. **Erster
    echter Trust-Boundary-Test bestanden**: Linux gepanict, Host
    bleibt responsiv.
  - **Linux 6.18.26 bootet komplett durch subsys-init.** Final state
    auf NUC: `Kernel panic - not syncing: VFS: Unable to mount root
    fs on "" or unknown-block(0,0)` → `Rebooting in 1 seconds..` →
    triple-fault (exit reason 2). = geplanter v0.121-Endstate, jetzt
    erreicht. 12.1.1c-Serie (3b3b1 → 3b3b23) komplett abgehakt.
  - **6 heutige Patches** räumten late CPU-Feature-Stolperer:
    v0.122 XSETBV-ack, v0.123 RDTSCP secondary-bit, v0.124
    USER_WAIT_PAUSE secondary-bit (für MWAIT-idle), v0.125 XSAVES
    + RDMSR/WRMSR-Handler (AMD-MSRs return 0, others ignore), v0.126
    256 MB Guest-RAM (von 64 → 256, SLAB-init OOM'd vorher) +
    #CP-Trap im EXCEPTION_BITMAP, v0.127 CET-Bits aus Guest-CPUID
    maskiert (CET vom Host, ohne Shadow-Stack-Setup im Guest = #CP).
  - **Pattern für CPUID/MSR-Stragglers etabliert**: enable wenn
    Linux's Code-Pfad's Capability spiegelbar ist (RDTSCP, MWAIT,
    XSAVES), hide wenn Guest dann Setup machen müsste den wir nicht
    spiegeln (CET), stub-return wenn AMD-spezifisch und Linux's
    fallback eh greift (RDMSR 0xc0011029).
  - **Vollständige Iterations-Historie** + Lessons in
    `memory/project_microvm.md`.
- **Earlier (2026-05-01 — Phase 12.1.0 + 12.1.1 in one push, v0.90 → v0.121):**
  - **VT-x MicroVM substrate from scratch to live earlycon-Stream**:
    VMXON/VMCS/VMCLEAR/VMPTRLD round-trip, host-state full round-
    trip mit GDT-walk-resolved TR-Base, TSS-install, VMLAUNCH gegen
    long-mode HLT-loop, EPT (1 GB identity → 16 MB non-identity →
    extension für IOAPIC/HPET/LAPIC-region), real-mode +
    unrestricted-guest, full VMRESUME-Loop mit GPR save/restore,
    CR3-load + I/O-bitmap (alle Ports trapped) + MSR-bitmap (zero)
    + CPUID pass-through + EFER load/save + dynamic IA-32e sync.
  - **bzImage-Loader**: Alpine `vmlinuz-virt` 6.18.26 (12 MB) als
    bundled installer-asset, landet in npkFS bei
    `sys/microvm/linux-virt.bzImage`. 32-bit boot protocol entry,
    boot_params + e820 + cmdline gefügt.
  - **`microvm` Shell-Intent** mit `test` / `linux-info` / `linux`.
    BSP-only (`is_core0_intent`) wegen TR/VMXE-state.
  - **Cmdline-Workaround**: `nolapic noapic acpi=off pci=off
    tsc=reliable` → Linux skipped Hardware-Probing, bootet als
    minimal-PC. Wird zurückgenommen sobald virtio-Backends da sind.

