# Phase 12 — MicroVM Subsystem

**Goal:** A scharf umrissene Capability to run **legacy Linux GUI apps**
(Browser, Office, evtl. Steam) inside a per-app VT-x VM with a
hardware-enforced trust boundary. nopeekOS bleibt WASM-first für eigene
Apps, Treiber und CLI; der MicroVM-Pfad ist ausschliesslich für Tools,
für die es realistisch keinen WASM- oder native-Rust-Ersatz gibt.

**Not goals.** Nicht ein generisches Linux-Container-Subsystem, nicht
Docker, nicht Flatpak, nicht POSIX-on-nopeek. Eine einzige
Capability ("ich darf eine signierte App-VM starten"), kein
gemeinsames Filesystem mit npkFS-Wurzel, kein ambient Linux.

> **Status:** **Phase 12.1 vendor-symmetrisch grün** (2026-05-05).
> Intel VT-x bis 12.1.4 NUC-bare-metal-validated (v0.137.2);
> AMD-V (SVM) bis 12.1.4 KVM-nested-validated (v0.143.0). Living
> document — jeder Open-Question-Block trägt `open` / `decided
> <date>` + Begründung. 12.2-12.5 Plumbing (virtio-blk/net,
> container-format, picker-bridge) ist als nächstes dran, dann
> 12.6 Firefox.

---

## Three-layer execution model

nopeekOS hat ab Phase 12 drei klar getrennte Ausführungsumgebungen.
Jede hat eine eigene Trust-Boundary.

```
┌─────────────────────────────────────────────────────────────────────┐
│  Layer A — WASM-native (Hauptpfad)                                  │
│    Eigene Apps, Treiber, CLI-Tools. wasmi runtime, fuel-metered.    │
│    Trust-Boundary: WASM linear-memory + capability tokens.          │
│    Beispiele: drun, loft, debug, wifi, top, wallpaper, testdisk.    │
├─────────────────────────────────────────────────────────────────────┤
│  Layer B — MicroVM (this document)                                  │
│    Legacy Linux GUI apps. VT-x non-root + EPT + VT-d.               │
│    Trust-Boundary: VMX hardware isolation.                          │
│    Beispiele (geplant): Firefox, Thunderbird, VSCode, Signal, ...   │
├─────────────────────────────────────────────────────────────────────┤
│  Layer C — Servo embedded (Phase 13+, langfristig)                  │
│    Native Rust browser engine als WASM-app (eventually).            │
│    Trust-Boundary: WASM. Wenn Servo production-ready ist (2030+),   │
│    löst das den primären MicroVM-Use-Case ab.                       │
└─────────────────────────────────────────────────────────────────────┘
```

Phase 12 baut nur Layer B. Layer A ist live, Layer C ist Wartezustand.

---

## Trust boundary — the central architectural insight

Wer sperrt was ein. Diese Schichtung ist die wichtigste Eigenschaft
des ganzen Subsystems und muss vor jedem anderen Detail klar sein.

```
┌─────────────────────────────────────────────────────────────────────┐
│  WASM Container-Manager  (e.g. tools/wasm/microvm-firefox/)         │
│    owns:  Manifest laden + verifizieren, VM-Lifecycle (create/run/  │
│           stop), Bridge-Endpoints (Picker, Clipboard), IO-Event-    │
│           pumping zwischen Guest und Shade-Terminal.                │
│    limits: WASM linear-memory + fuel + 1 worker core.               │
│    NEVER: VMX-Instructions, EPT-Tables, MSR-Writes, raw DMA.        │
╞═════════════════════════════════════════════════════════════════════╡  ← WASM sandbox boundary
│  Host functions  npk_vm_*  (kernel/src/wasm.rs, capability-gated)   │
│    npk_vm_create     VM cap  → VmHandle from manifest_hash          │
│    npk_vm_run        VM cap  → schedule guest, returns on VM-exit   │
│    npk_vm_event_poll VM cap  → IoEvent (console byte, virtio kick)  │
│    npk_vm_stop       VM cap  → tear down, free EPT + VCPU threads   │
│    npk_vm_inject     VM cap  → bytes into virtio-console RX         │
├─────────────────────────────────────────────────────────────────────┤
│  Kernel MicroVM subsystem  (kernel/src/microvm/cpu/{vmx,svm}/)      │
│    owns:  CPUID-vendor-dispatch, VMXON/VMCS+EPT (Intel) or          │
│           EFER.SVME/VMCB+NPT (AMD), host-state save/restore,        │
│           VT-d / IOMMU domains, VCPU thread (one Rust task per      │
│           VCPU), VM-Exit dispatcher per backend.                    │
├─────────────────────────────────────────────────────────────────────┤
│  Hardware  (Intel VT-x + EPT + VT-d  /  AMD-V + NPT — both live)    │
│    enforces: Guest cannot see Host-Phys outside its EPT/NPT        │
│              mapping, Guest DMA blocked outside its IOMMU domain.   │
└─────────────────────────────────────────────────────────────────────┘
```

**Two cages exist in parallel, gated by different mechanisms.**

| Cage | What it confines | Limits | Enforced by |
|---|---|---|---|
| **WASM-Manager** | only the manager itself | Linear-Memory + Fuel + 1 worker core | wasmi runtime |
| **VM (= the Linux app)** | the Linux guest | VCPU count + EPT range + IOMMU domain | VMCS / EPT / VT-d hardware |

The Linux guest is **not** running in WASM. The manager is small Rust
glue; the cage that matters for the guest is the VMX hardware
boundary. A bug in the WASM-manager kills the manager but cannot
escape into the host kernel; a bug in the kernel VMX code is a
ring-0 issue and is the unavoidable trust base. The Linux guest can
crash freely (kernel-panic, OOM, anything) and the host stays up.

---

## Container format — recycled OTA pipeline

Same trust pipeline as kernel + WASM-module updates. Triple per app
version, atomically replaceable in npkFS:

```
release/apps/firefox/
├── firefox-138.0.1.sqfs        (~70 MB, read-only rootfs, BLAKE3-hashed)
├── firefox-138.0.1.manifest    (TOML, caps + runtime)
├── firefox-138.0.1.sig         (ECDSA P-384 over manifest+sqfs hash)
└── current → 138.0.1
```

`install firefox` zieht das Triple, verifiziert ECDSA-Signatur,
schreibt das `.sqfs` als content-addressed npkFS-Objekt. `update
firefox` ersetzt atomar. Rollback = pointer-switch, vorheriges sqfs
ist noch im store. **Identische Logik zu WASM-Modul-Updates**, nur
grössere Files. Keine neue Trust-Hierarchie.

**Per running VM, three storage tiers:**

```
[ /dev/vda  =  Container.sqfs  read-only, signed by us               ]   ← geupdated als ganzes
[ /dev/vdb  =  App-Profile     read-write, encrypted ext4 in npkFS   ]   ← User-Daten, persistent
[ /tmp, /run = tmpfs           read-write, ephemeral                 ]   ← weg beim VM-stop
```

Read-only base + writable user-overlay ist klassisch (ChromeOS,
Android APEX, Flatpak/OSTree). Die Verschlüsselung des
App-Profile-Images läuft auf der Host-Seite — der Guest sieht
unverschlüsseltes ext4, der Host hält den Key.

---

## Bridges — how the VM talks to anything

Drei Bridge-Typen. Jede ist eine eigene Capability, default-deny.

### Pattern X — Block-Image Bridge (App-Profile, immer aktiv)

```
Host npkFS-Objekt: firefox-profile-{user-hash}
  └─ AES-256-GCM verschlüsselt mit User-derived Key (HW AES-NI, ~700 MB/s)
  └─ Inhalt: rohes ext4-Image, sparse, ~512 MB initial

VM-Boot:
  Host virtio-blk-Backend liest aus npkFS, dekryptiert pro 4KB-Block
  Guest sieht /dev/vdb als unverschlüsseltes ext4
  Guest mountet nach /home/<app>/.config etc.
```

Der Guest hat den Key nie. VM kompromittiert → Angreifer sieht das
gemountete Profil (das ist der Punkt der App), kann aber andere
App-Profile nicht lesen. VM aus → Image ist opaker AEAD-Blob im
npkFS-Store.

Das ist konzeptuell wie LUKS, nur mit Key-Verwaltung zentral im Host
statt im Guest.

### Pattern A — Picker Bridge (default für GUI-Apps, User-Files)

App fragt explizit nach Files (open-dialog, save-dialog,
drag-and-drop), Host zeigt nopeek-Picker in Shade, User wählt aus
npkFS, Host reicht **genau dieses File** in einen tmpfs-Pfad in der VM.

```
Firefox: <input type="file"> klick
  └─ Linux-Side: virtio-fs request "open file dialog"
  └─ Host: zeigt nopeek-Picker (Shade-overlay), User wählt /docs/CV.pdf
  └─ Host: schreibt CV.pdf nach /upload/CV.pdf (tmpfs in der VM)
  └─ Firefox: bekommt einen Dateipfad, lädt hoch
```

Save-Dialog reverse. Drag-and-Drop mappt natürlich darauf.

Vorteile: App sieht nichts vom Filesystem ausser dem einen explizit
gewählten File. Default-deny ist trivial. Kein POSIX-on-npkFS
Adapter nötig.

### Pattern B — virtiofs Subtree-Mount

Zwei Varianten, dieselbe Mechanik (single npkFS-Subtree → virtio-fs in
Guest), unterschiedlicher Scope:

**B-mini — Per-App-Downloads** (Phase 12.5, gebraucht für Firefox).
Genau ein read-write-Subtree pro App, default unter
`home/<user>/downloads/<app-name>/`. Deckt den Auto-Download-Fall ab,
den der Picker nicht trifft (Firefox: Right-Click "Save Image", Bulk-
Downloads, Direct-Save aus dem Browser). User sieht die Files **direkt
in loft**, weil sie im npkFS-Home liegen. Kein POSIX-on-npkFS-Adapter
nötig — flacher Subtree mit File-CRUD reicht. npkFS v3 liefert
`mtime`/`size`/`kind` schon, deckt `stat()` + `readdir()`.

```toml
[caps.files]
picker    = true                       # Pattern A
downloads = "~/Downloads"              # B-mini, default-Mapping zu home/<user>/downloads/<app>/
```

**B-full — Subtree-Mount mit Manifest-Pfaden** (post-12.6, später).
Mehrere Pfade, ro/rw-Mix. Für IDE / File-Manager / Editor. Braucht
echten POSIX-on-npkFS-Adapter (Permissions, Symlinks, Hardlinks,
xattr).

```toml
[caps.files]
mount = [
  { path = "~/Documents", mode = "rw" },
  { path = "fonts/",      mode = "ro" },
]
```

Wird für die ersten 5 Apps (Browser, Mail, Terminal, Mediaplayer,
Messenger) **nicht** gebraucht — die leben gut mit Picker + B-mini.

### Pattern Y — Cross-VM Channels (sehr später, off by default)

Bitwarden-Extension im Browser-VM will mit Bitwarden-Vault-VM reden.
Default-deny, explizit pairwise im Manifest gepinnt. Spec offen
(siehe Open Questions).

---

## Manifest schema — DRAFT

Wire-Version `0x01`, append-only Cap-Enum (analog Widget-ABI).

```toml
[meta]
name         = "firefox"
version      = "138.0.1"
sqfs_blake3  = "0x..."
sqfs_size    = 73891840
ecdsa_sig    = "0x..."

[runtime]
kernel_min   = "6.18.0"
init         = "/usr/bin/firefox"
memory_mb    = 1024
vcpus        = 2

[caps]
network      = { allow = ["https/443", "dns/53"] }
display      = { protocol = "virtio-gpu", mode = "cross-domain" }
audio        = { playback = true, capture = false }
storage      = { profile = true, size_mb = 512 }
clipboard    = { read = true, write = true, types = ["text", "image"] }
files        = { picker = true, downloads = "~/Downloads", mount = [] }
```

Schema is **frozen per Wire-Version**. Adding a new cap = bump to
`0x02`. Cap field types are append-only (you can add a new
sub-field, you cannot rename or remove). Same discipline as
`WIDGET_VOCAB.md`.

**`display.mode` values** (append-only):
- `"cross-domain"` — virtio-gpu cross-domain context (kernel ≥ 5.16,
  voll im 6.18 LTS). Wayland-Passthrough auf virgl-Protocol-Level.
  Default für Phase 12.6 (Firefox).
- `"drm-native"` — virtio-gpu DRM native context (Patch-Reife
  6.17/6.18, bessere Performance, lightweight contexts). Opt-in pro
  App wenn die Patch-Level-Reife stimmt; in Phase 12 wahrscheinlich
  noch nicht Default.

---

## Kernel API surface — DRAFT

New host functions, capability-gated. The whole MicroVM API is
behind a single new cap (`VM`) plus the existing read-cap on the
sqfs object.

```rust
// kernel/src/wasm.rs (new section)

// Construct VM from a verified manifest. Reads sqfs+profile-image
// from npkFS by hash, allocates EPT, creates VCPU threads (paused).
fn npk_vm_create(manifest_hash_ptr: u32, manifest_hash_len: u32) -> u64;
//   returns VmHandle, or error code in upper bits

// Schedule the VM. Returns when guest hits a VM-exit that needs the
// manager (IO, halt, panic). Does NOT return on every minor exit —
// the kernel handles trivial exits internally.
fn npk_vm_run(vm: u64) -> u32;
//   returns VM-exit reason

// Drain pending IO events (console byte, virtio kick, etc.)
fn npk_vm_event_poll(vm: u64, buf: u32, buf_len: u32) -> u32;

// Inject a byte stream into virtio-console RX (keyboard input).
fn npk_vm_inject_console(vm: u64, ptr: u32, len: u32) -> u32;

// Tear down. Frees EPT, kills VCPU threads, releases IOMMU domain.
fn npk_vm_stop(vm: u64) -> u32;
```

**12.1 minimum** ships only `create / run / event_poll /
inject_console / stop`. virtio-blk, virtio-net, virtio-gpu come in
12.2/12.3/12.4 with their own host-fn additions.

---

## Roadmap

| Phase | Title | Goal | Notes |
|---|---|---|---|
| 12.0 | **Spec freeze** | This document agreed; manifest schema + host-fn signatures frozen at Wire-Version 0x01 | Kein Code. Open-Questions abgearbeitet bis sie `decided` sind |
| 12.1 | **Hello Bash** | Mainline Linux LTS bootet in VT-x, BusyBox-bash auf virtio-console, `echo hi` round-trip durch Shade-Terminal | Kein Disk, kein Net, kein GPU. ~256 MB Guest-RAM, 1 VCPU. Validiert: VMX bring-up, EPT, VCPU-Thread, virtio-console, Manager-WASM-Loop |
| 12.2 | **virtio-blk + Profile-Image** | Verschlüsseltes ext4-Block-Image als npkFS-Objekt, Guest mountet als `/dev/vdb`. Erste persistente Daten | AES-256-GCM Block-AEAD (HW AES-NI), Block-Nummer im AAD. Recyclet bestehende `crypto/aead.rs` |
| 12.3 | **virtio-net mit Cap-Filter** | Guest hat IP, kann HTTPS, Cap-Liste enforced am Host (kein iptables im Guest) | Erste Internet-fähige App möglich (z.B. `curl`) |
| 12.4 | **virtio-gpu cross-domain + Shade-Bridge** | Wayland-Forwarding aus Guest in Shade-Surface, Maus/Tastatur rein, Pixel raus | virtio-gpu cross-domain context (Mainline ≥ 5.16, vollständig in 6.18). Erste GUI-App möglich. DRM native context als späteres Performance-Upgrade pro App |
| 12.5 | **Picker + Mini-virtiofs** | Open/Save-Dialog im Host (Shade-overlay, Pattern A) **+** ein read-write Subtree pro App unter `home/<user>/downloads/<app>/` als virtio-fs (Pattern B-mini, deckt Auto-Downloads die der Picker nicht trifft — Files in loft sichtbar) | Blocking call from Guest für Picker, Wayland-DnD später. B-mini braucht keinen POSIX-Adapter, flacher Subtree reicht |
| 12.6 | **Firefox** | Erste echte App. Multi-VCPU, GPU-Sharing, Audio, Picker, Profile-Image, virtio-net | Endboss von Phase 12. Daily-driver-Test |

12.0 ist explizit ein Diskussions-Meilenstein. Kein Code bevor das
Spec-File so steht, dass die Open Questions entweder beantwortet oder
dokumentiert deferred sind.

---

## Sequencing — Microkernel-Refactor NACH 12.6 (revised 2026-05-05)

`decided 2026-04-30, revised 2026-05-05`. Ursprüngliche Reihenfolge
war Refactor zwischen 12.1 und 12.2 (Code-Drift-Argument: virtio-blk/net
in zwei Rollen — Guest-WASM-Driver vs. Host-Kernel-Backend). Beim
zweiten Hinsehen halten die zwei Rollen aber **unterschiedlichen Code**:
Host-Backend = Trap-and-Emulate im `microvm/`-Subtree (handle IOIO/MMIO,
fill descriptor used-ring, inject virtual IRQ); Guest-WASM-Driver =
Linux-spec virtio-Client (read available-ring, process IRQs). Sie
teilen nur die Wire-spec, nicht den Code.

Die einzige reale Berührungsfläche: wo der Host-Backend "gib Paket an
Host-NIC" sagt. Das geht heute direkt an `intel_nic::send_frame()`,
post-Refactor an `npk_net_*`-host-fn — ein Call-Site-Swap, keine
Re-Implementation. Die `net::eth`-Abstraktion fängt das.

**Neue Reihenfolge (2026-05-05):** 12.1.4 ✓ → **12.2-12.6 (Firefox)** →
**Microkernel-Refactor** → HW-Extension-Set (NVMe/xhci/framebuffer/
intel_xe).

**Why neu:** Firefox-in-MicroVM ist der eigentliche Demo-Win der ganzen
Vision. Refactor verkürzt 12.2-12.6 nicht und macht es nicht günstiger.
Time-to-Firefox: 4-6 Wochen direkt vs. 6-9 Wochen mit Refactor-Vorlauf.

**Refactor-Cost bleibt:** ~2-3 Wochen, wird hinter 12.6 gehängt.

---

## Decided  *(sortiert nach Hebel, neuer-zuerst)*

### `decided 2026-04-29` — Kernel-Strategie: Mainline LTS + `nopeek-tiny.config`
Wir kompilieren den Linux-Kernel selbst aus mainline LTS mit eigener
minimaler Config. Nicht Alpine-Kernel, nicht Buildroot, nicht
Custom-Fork.

**Branch: Linux 6.18 LTS.** Begründung:
- 6.12 LTS ist unter der Schwelle für `virtio-gpu` DRM native
  context (braucht 6.13+, properly stabil 6.17/6.18) — kostet uns
  den Performance-Pfad bei 12.6 (Firefox) und später Steam.
- 6.18 hat den vollen modernen `virtio-gpu` Stack inkl.
  cross-domain context (für Wayland-Passthrough) und DRM native
  context (für Performance-Vulkan).
- 7.0 (released 2026-04-12) ist auf kernel.org als **stable**,
  nicht longterm — ~3 Monate Lifecycle bis 7.1 das ablöst.
  Re-Evaluation wenn Linus eine 7.x als longterm designiert
  (typisch Q4/Jahresende).
- Linus-Pattern: 6.18 wird mind. bis ~2031 supportet.

**Pinning:** Major-Branch (`6.18`) im Spec-File, Patch-Level
(`6.18.x`) im App-Recipe analog `alpine_snapshot`:
```toml
[base]
linux_lts_branch     = "6.18"
linux_patch          = "6.18.X"
linux_tarball_blake3 = "0x..."
```
Damit bleibt das Spec stabil (sagt nur "6.18 LTS"), Patch-Bumps
sind Recipe-PRs.

**Re-Evaluation-Trigger** für Bump auf 7.x:
1. kernel.org designiert eine 7.x als longterm (üblicherweise
   Q4/Jahresende), und
2. Performance- oder Feature-Bedarf rechtfertigt den Bump
   (DRM native context erweiterte Features, neue virtio-Bits).

**Allgemeines Why** (Branch-unabhängig): Trust-Layer #1, wir
kontrollieren den Kernel. Aufwand klein (einmalig 2–3 Tage
Config-Tuning, danach LTS-Bumps alle 2 Jahre). Linux-Security-Team
macht CVE-Patches upstream. Image deutlich schlanker als
Alpine-Generic (3–5 MB vs ~8 MB).

### `decided 2026-04-29` — Init in der Guest-VM: eigener Rust-PID-1
Statisch gelinkt, ~50 KB. Mountet `/proc /sys /dev`, exec'd das
Binary aus `manifest.init`, fertig. Keine sysvinit-Skripte, keine
service-units, kein systemd.
**Why:** Wir reden über VMs mit einem einzigen Anwendungsprozess
(Firefox / Foot / mpv) — das ist kein Server-OS-Init. BusyBox-init
wäre Overkill, systemd-shrink ein No-Go (riesige Codebase). Eigener
Rust-Init passt zur nopeekOS-Linie ("alles selbst, scharf umrissen,
auditable"), ist trivial zu reviewen, hat kein Drift-Risiko gegen
Upstream-Init-Systeme. Konsistent mit Layer-A-Philosophie (eigene
Rust-Module statt Fremdcode für Trust-kritische Schichten).

### `decided 2026-04-29` — Userspace-Quelle: Alpine-apk aus gepinntem Snapshot
App-Runtime (libc, ICU, freetype, Mesa, Wayland-libs, …) und Apps
selbst (Firefox, Thunderbird, …) kommen aus Alpine-apk im
CI-Build, nicht from-source.
**Why:** Mesa+Firefox from-source wäre Wartungs-Selbstmord (200+
transitive Deps, stundenlange Build-Times). Alpine macht den
Compile-Job, wir prüfen Hashes pro Paket im Recipe und signieren das
fertige Image. Trust-Chain: Alpine signiert apk → wir verifizieren
beim Build → wir signieren sqfs+manifest → User verifiziert beim
Install.

**Disziplin (nicht-verhandelbar):**
- Pin alles. `alpine_snapshot = "YYYY-MM-DD"`, jede apk-Version exakt,
  BLAKE3-Hash pro Paket im Recipe. Kein "latest" jemals.
- Snapshot-Mirror in eigenes GitHub-Repo (Resilience gegen
  dl-cdn.alpinelinux.org-Outages, plus Datenschutz wenn Alpine
  Snapshots irgendwann GC'd).
- Reproducible-Build-Discipline: bit-genaues sqfs aus Recipe
  rekonstruierbar (timestamps=0, sortierte Listen, deterministische
  Compression).

**Eskalationspfad** (nicht Default): wenn in 1–2 Jahren ein konkreter
Anlass für ein bestimmtes Paket aufkommt (Hardening, Patches die nicht
upstream sind, Alpine entscheidet sich gegen unsere Bedürfnisse),
optional pro App-Recipe `[overrides] pkg = { source = "git://...",
patches = [...] }`. Single-Pakete from-source, **nicht** "alle, weil
Prinzip".

### `decided 2026-04-29` — Insel-pro-App
Jede App ist eine eigene VM mit eigenem Profile-Image. Kein
Cross-App-Filesystem-Zugriff by default. Wenn Interaktion zwischen
Apps nötig ist, wird sie pairwise im Manifest gepinnt (Pattern Y).
**Why:** Capability-Philosophie. Overhead (mehr RAM pro App) ist
akzeptabel für saubere Trennung. Qubes-Modell.

### `decided 2026-04-29` — Hybrid Kernel/Manager-Schichtung
Kernel besitzt VMX-Instructions, VMCS, EPT, VT-d, VCPU-Threads.
WASM-Manager besitzt Manifest-Loading, VM-Lifecycle,
Bridge-Endpoints. **Why:** EPT-Page-Tables direkt aus WASM zu
konstruieren wäre eine riesige Angriffsfläche. Kleine scharf
umrissene Kernel-Primitive + großer WASM-Glue folgt dem bestehenden
Driver-ABI-Muster (PCI/MMIO/DMA-Hostfns mit Capability-Check).

### `decided 2026-04-29` — Browser als erste produktive App
Phase 12.6 ist Firefox. Vorher (12.1–12.5) sind Infrastruktur-
Meilensteine. **Why:** Browser ist der primäre Use-Case, der
überhaupt zur MicroVM-Entscheidung geführt hat. Alles davor ist
Plumbing.

### `decided 2026-04-29` — Container = signiertes Squashfs-Triple
Read-only sqfs + manifest.toml + ECDSA P-384 sig. Identische
Pipeline zu OTA-Modul-Updates. **Why:** Recyclet komplette
bestehende Trust-Infrastruktur. Nichts neues zu bauen ausser dem
VM-Builder selbst.

### `decided 2026-04-29` — App-Profile als verschlüsseltes Block-Image
Pro App ein verschlüsseltes ext4-Image als npkFS-Objekt, virtio-blk
zum Guest. Crypto bleibt Host-side. **Why:** Linux braucht keine
npkFS-Awareness, sieht normales ext4. Backup ist Kopie eines
npkFS-Objekts. Kein POSIX-on-npkFS-Adapter nötig.

### `decided 2026-04-29, refined 2026-05-05` — Picker + B-mini default, B-full opt-in
Pattern A (Picker, PowerBox-Style) ist Default für **explizite**
File-Flows (Save As, Drag-and-Drop). Pattern B-mini (single read-write
Subtree pro App, default `home/<user>/downloads/<app>/`) deckt **Auto-
Downloads** die der Picker nicht trifft — kommt **mit 12.5**, nicht
später. Pattern B-full (Multi-Pfad-Manifest-Mounts mit ro/rw-Mix) ist
opt-in pro App, kommt erst mit IDE/File-Manager-Use-Cases nach 12.6.
**Why refined:** Firefox-Auto-Downloads sind kein Picker-Flow, müssen
aber für den User in loft sichtbar sein. B-mini braucht keinen
POSIX-on-npkFS-Adapter (flacher Subtree, npkFS v3 liefert mtime/size/
kind), ist also kein 12.5-Cost-Treiber.

### `decided 2026-04-29` — GitHub Releases als Distribution
Containers werden auf GitHub Releases gehostet, identisch zu
Kernel-OTA. **Why:** Bei <100 Usern null Kostenproblem, GitHub
Releases ist kostenlos und schnell. Wechsel zu IPFS/BitTorrent
später ist trivial (nur `distribution`-Section im Manifest).

### `decided 2026-04-29` — AI komplett raus für Phase 12
Kein AI-Resolver, keine AI-Manifest-Generierung, keine AI in der
Build-Pipeline. **Why:** Alles im Phase 12 funktioniert statisch
ohne AI-Schicht. Container kommen über `install <name>` (deterministisch),
Manifeste sind kuratiert und im Repo. AI ist Phase 13+ als reines
Frontend-Feature, nicht als Trust-Komponente.

---

## Open Questions

Each block: status, options with trade-offs, default-if-stuck.
Document gets updated when these collapse.

### `decided 2026-05-05` — POSIX-on-npkFS für Pattern B-full

War offen, ist mit npkFS v3 effektiv beantwortet: npkFS hat schon
Git-style Tree-Objects (`paths::store` + `TreeEntry { name, hash, kind,
size, mtime, flags }`). Pattern B-full bekommt also einen virtiofs-
Adapter der direkt gegen die npkFS-Tree-API geht. Permissions/Symlinks/
Hardlinks/xattr sind die offene Mehrarbeit — kein neues Daten-Modell.

Pattern B-mini (12.5) braucht das **nicht** — flacher Subtree, npkFS-
list/stat reicht, kein Permissions-Mapping.

### `open` — Display-Path für GUI-Apps

Wie kommen Pixel aus dem Guest auf das Host-Display?

> **Update 2026-04-29:** virtio-wl ist nie in Mainline-Linux gelandet
> (lebt nur in der ChromiumOS-Tree). Modernes crosvm hat sich davon
> weg bewegt zu virtio-gpu cross-domain context. Optionen revidiert:

- **A. virtio-gpu cross-domain context.** Wayland-Passthrough über
  virtio-gpu, virgl-Protocol-Level. Mainline ≥ 5.16, vollständig in
  6.18 LTS. Crosvm nutzt das produktiv, gut studierbar. **Default**.
- **B. virtio-gpu DRM native context.** Variante von (A) auf
  Kernel-UAPI-Level statt API-Level → leichtgewichtigere Contexts,
  bessere Performance. Patch-Reife 6.17/6.18, Stand Anfang 2026 noch
  in v9–v12 Patch-Iterationen. **Performance-Upgrade-Pfad**, opt-in
  pro App ab Phase 12.6+ wenn Reife stimmt.
- **C. virtio-gpu + Wayland-Compositor im Guest.** Guest läuft
  weston/sway, Frames raus via virtio-gpu. Einfacher zu starten,
  doppelter Compositor-Overhead, schlechte Latenz. **Verwerfen**.
- **D. vsock + Wayland-Forward.** X-Forwarding-Style. Schlechte
  GPU-Performance. **Verwerfen**.

**Default-if-stuck:** A (virtio-gpu cross-domain) als Phase-12.4-Default,
B als Performance-Upgrade-Pfad pro App ab 12.6+ wenn Patch-Reife
stimmt. Der Manager-WASM bekommt einen `display.mode`-Flag (siehe
Manifest-Schema).

### `open` — virtio-Device-Backends: WASM oder Kernel?

- **virtio-blk** mit AES-GCM pro Block: vermutlich Kernel
  (Crypto-Hot-Path, jeder Read durchläuft AEAD).
- **virtio-net** mit Cap-Filter: Kernel oder WASM. Diskutabel.
- **virtio-console**: Kernel (klein, einfach).
- **virtio-gpu** (cross-domain / drm-native): Kernel (GGTT/BCS-
  Zugriff nötig, Cross-Domain-Protocol-Routing kann teilweise im
  WASM-Manager liegen).

**Default-if-stuck:** Backends, die Hardware-Beschleunigung oder
Kernel-Hot-Path brauchen → Kernel. Backends, die nur Protocol-Routing
machen (virtio-clipboard, evtl. virtio-gpu Cross-Domain-Endpoint) →
WASM-Manager.

### `open` — musl/glibc-Strategie

Alpine ist musl-native; Steam, Spotify, einige proprietäre Apps
sind glibc-only.

- **A. gcompat-Wrapper.** musl mit glibc-Symbolen-Bridge. Funktioniert
  meistens, manchmal nicht.
- **B. Pro-App-Auswahl der libc.** App-Manifest sagt `libc = "glibc"`,
  Builder zieht glibc-Userspace.
- **C. Erstmal nur musl-kompatible Apps.** Firefox läuft mit musl.
  Steam später mit Spezial-Recipe.

**Default-if-stuck:** C. Steam ist Phase 13.

### `open` — Cross-VM-Channels (Pattern Y)

Wenn Bitwarden-Browser-Extension mit Bitwarden-Vault-VM reden will
oder Element-Calls Audio mit anderer App teilen.

- **A. Vsock-basierte Pairs.** Manifest deklariert `peer_with =
  ["bitwarden-vault"]`, Host setzt vsock zwischen den beiden auf.
- **B. Host-vermittelte Bridge mit Protocol-Spec pro Pair.** Host
  versteht das Protokoll, kann filtern.

Komplex, default-deny ist trivial einzuhalten. Spec wird erst nötig
wenn ein konkreter Use-Case auftaucht.

**Default-if-stuck:** Auf Phase 13+ verschoben.

### `open` — Audio-Bridge

PipeWire-Bridge ist heikel (Capture vs Playback, low-latency,
Volume-Mixing).

- **A. virtio-snd ohne Mixer.** Pro VM ein dedizierter Audio-Stream,
  Host mixt extern.
- **B. PipeWire-vsock-Forward.** Guest hat eigenen PipeWire-Client,
  Socket über vsock zum Host.

**Default-if-stuck:** A für Phase 12, B wenn Mehrfach-Apps mit Audio
gleichzeitig laufen sollen.

### `open` — GPU-Sharing

Browser braucht Hardware-Compositing, Steam braucht Vulkan.

- **A. virtio-gpu mit virgl (für GL).** Browser läuft, Steam
  problematisch.
- **B. virtio-gpu mit Venus (für Vulkan).** Beides läuft, Komplexität
  steigt deutlich.
- **C. SR-IOV.** Echte GPU-Partitionierung. Hardware-spezifisch (kein
  N100-Support). Verwerfen.

**Default-if-stuck:** A für Phase 12.6 (Firefox), B wenn Steam in
Phase 13 dran ist.

### `open` — Phase-Nummer

Aktuell ist Phase 11 = "AI Integration", Phase 11.5 = npkFS v2 (DONE).
AI-Integration als Phase ist obsolet (AI raus). MicroVM könnte
Phase 12 sein **oder** Phase 11 wird umbenannt.

**Default-if-stuck:** Phase 12 für MicroVM, Phase 11 wird in
README.md zu "reserved / future" oder gelöscht. Entscheidung beim
ersten README-Update für 12.0.

---

## Glossar

| Begriff | Bedeutung |
|---|---|
| **Container** | 1-File read-only signiertes squashfs-Image für eine App-Version |
| **Manifest** | TOML-Datei mit Metadaten + Caps für einen Container |
| **Manager** | WASM-App, die einen Container in eine VM startet und verwaltet |
| **VM / Guest** | Die VT-x non-root Umgebung, in der die Linux-App nativ läuft |
| **Bridge** | Capability-gated Kanal zwischen Guest und Host (Picker / Block-Image / Cross-VM / virtio-*) |
| **Insel** | Eine VM ist isoliert, kein File-System-Sharing mit anderen VMs |
| **Profile-Image** | Verschlüsseltes ext4-Block-Image im npkFS, virtio-blk an den Guest |
| **Picker** | Host-side Open/Save-Dialog (Shade-overlay), reicht 1 File explizit rein |
| **VMCS** | Virtual Machine Control Structure (Intel-Term für die Datenstruktur, die einen VCPU-Zustand beschreibt) |
| **EPT** | Extended Page Tables (Intel hardware-beschleunigtes Guest-Phys → Host-Phys Mapping) |
| **VT-d** | IOMMU-Technologie (Intel) für DMA-Isolation zwischen Guest und anderem Host-Speicher |
| **vsock** | virtio-Socket, Host-Guest-IPC-Channel ohne IP-Stack |

---

## README + CLAUDE.md update notes

When 12.0 closes (this document agreed), pull these in:

- **`README.md`**:
  - Phase 11 entry: replace "AI Integration" with placeholder oder delete
  - New Phase 12 section with the roadmap-table from this file (kondensiert)
  - Three-layer execution model in the Architecture section
  - Add `MicroVM` row to the Technical Decisions table

- **`CLAUDE.md`**:
  - Add MicroVM-Subsystem unter "Current Status" wenn 12.1 startet
  - Add `crypto-stack` related deferred items context (TLS 1.3 swap will likely come up again when virtio-net cap-filter discussion starts)
  - Cross-link to this doc

- **New memory file:** `project_microvm.md` referencing this doc + the
  decided list. Update `MEMORY.md` index.

When 12.1 ships:
- README: tick `[x] Hello-Bash MicroVM round-trip` under Phase 12
- README + CLAUDE.md `Current Status` block update
- Memory: project_microvm.md gets a status section

---

## What this document is not

- Not a kernel-config draft (12.0 deliverable, not part of this file).
- Not a `npk_vm_*` host-fn implementation spec (12.0 deliverable, will
  live in `kernel/src/wasm.rs` doc-comments).
- Not a manifest schema reference (will move to a separate
  `MICROVM_MANIFEST.md` once stable, like `WIDGET_VOCAB.md`).
- Not the build-pipeline spec (separate doc when 12.6 is closer).
