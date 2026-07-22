# nopeekOS

**A capability-based operating system where every app and driver is a
sandboxed WASM module.**

A bare-metal Rust kernel. No Unix. No POSIX. No root. No legacy from
the '70s. Apps, drivers, and even a from-scratch web browser all run
inside the same WASM trust boundary — the only thing outside it is the
microkernel itself.

---

## The Idea

Why does an operating system look the way it does? Mostly because of
decisions made decades ago: processes because that's how 1969 hardware
was shared, `chmod 755` because that's how 1984 multi-user Unix drew its
lines, a filesystem tree because that's how you found things on a tape.

nopeekOS asks a different question: **what does an OS look like if you
start over today?**

- A **capability vault** instead of permissions — no `chmod`, no ACLs, no root
- **WASM sandboxes** instead of processes — one trust boundary for everything
- A **content-addressed store** (npkFS) instead of a filesystem tree
- **Intents** instead of commands — express what you want, not the protocol
- **Encrypted by default** — every byte on disk is AES-256-GCM at rest

The kernel is written in Rust (`no_std`, nightly, x86_64) and boots
straight from UEFI. Everything above it — the compositor, the panels,
the file browser, the text editor, the WiFi driver, the web browser — is
a signed, capability-gated WASM module.

---

## What It Can Do Today

A tiling desktop with real apps, networking, encrypted storage, and a
native web browser — running on bare metal (Intel N100 NUC, HP laptop)
and in QEMU.

```
# Desktop
Mod+D                    # App launcher (drun) — search + Enter to launch
loft                     # File browser: grid/list, copy/cut/paste, sortable columns
spell                    # Text editor: tabs, syntax highlight, save to npkFS
iris                     # Image viewer (PNG)
snap                     # Screenshot → npkFS
beak                     # Native web browser (from scratch, no Linux)
browser                  # LibreWolf in a Linux microVM (compatibility browser)

# The intent loop (terminal)
npk> status              # System overview (cores, RAM, disk, net)
npk> store notes v=1     # Store an object (BLAKE3-addressed, encrypted at rest)
npk> fetch notes         # Retrieve + decrypt + integrity-check
npk> find report         # Search object names   ·   grep TODO notes
npk> https nopeek.ch /   # HTTPS GET (TLS 1.3, hardware AES) — add `> page` to store
npk> install <module>    # Fetch + verify (ECDSA P-384) a signed WASM module
npk> update              # OTA kernel update (signed, SHA-384 verified)
npk> driver wifi         # Bring up the AX200 WiFi driver + scan
npk> cores               # Trustworthy per-core CPU instrumentation
```

Every operation is capability-gated. No ambient authority, no sudo.
Identity is your passphrase — no users, no accounts. All data encrypted
at rest.

---

## Architecture

```
 ┌──────────────────────────────────────────────────────────┐
 │  WASM apps & drivers (sandboxed, capability-gated)         │
 │  shade  — tiling compositor (theme, panels, animation)     │
 │  bar / dock  — top + bottom panels                         │
 │  drun / loft / spell / iris / snap — launcher + apps       │
 │  beak   — from-scratch web browser (HTML/CSS engine)       │
 │  wifi / aml / audio_hda — hardware drivers as WASM         │
 ├──────────────────────────────────────────────────────────┤
 │  MicroVM (compatibility layer)                             │
 │  VT-x / AMD-V hypervisor · custom Linux 6.18 · virtio      │
 │  bridges · 9p↔npkFS · runs LibreWolf as a tiled window     │
 ├──────────────────────────────────────────────────────────┤
 │  WASM runtime (wasmi, fuel-metered) + host ABI (npk_*)     │
 │  npk_scene_commit — declarative widget rendering           │
 │  npk_fs_* · npk_http_request · npk_input · npk_battery     │
 │  npk_pci/mmio/dma — capability-gated driver ABI            │
 ├──────────────────────────────────────────────────────────┤
 │  Kernel (Rust, no_std)                                     │
 │  SMP scheduler (work-stealing + stackful fibers)           │
 │  npkFS v3 (content-addressed, CoW, encrypted)              │
 │  Crypto (AES-256-GCM · BLAKE3 · TLS 1.3 · ECDSA P-384)     │
 │  Capability vault · network stack · GPU HAL · drivers      │
 ├──────────────────────────────────────────────────────────┤
 │  Hardware: x86_64, UEFI                                    │
 └──────────────────────────────────────────────────────────┘
```

---

## Core Principles

### Capabilities, not permissions

No `chmod`, no ACLs, no root, no sudo. Every resource requires a
256-bit cryptographic token (CSPRNG, Grover-resistant). Tokens delegate
with monotonically shrinking rights, expire on a tick clock, and can be
revoked transitively. Everything is audited. **Deny by default:** without
a token, nothing happens.

### WASM as the universal execution model

Every app and every driver is a WASM module — loaded from npkFS,
BLAKE3-verified before execution, fuel-metered, and given exactly the
capabilities it declares in a 1-byte `.npk.caps` section (default:
READ + EXECUTE + RENDER, never WRITE). A hardware driver reaches its
device through capability-gated `npk_pci_*` / `npk_mmio_*` / `npk_dma_*`
host functions — the same sandbox contains a text editor and a WiFi
stack. A guest trap kills exactly one instance, nothing else.

### Content-addressed storage (npkFS)

No paths as identity, no hierarchy in the on-disk format. Objects are
identified by their BLAKE3 hash; directories are Git-style tree objects.
Copy-on-write B-tree, dedup on write, AES-256-GCM at rest, per-entry
mtime, mark-and-sweep GC. Apps still see opaque path strings — the tree
is content-addressed underneath.

### Intents, not commands

Instead of `curl -X GET https://…`, you say `https example.com /`. The
system owns DNS, TCP, TLS, HTTP — you express intent, not protocol.

---

## What's Built

Grouped by subsystem. Kernel is at **v0.228.12**; the full change history
lives in the git log and `memory/`.

**Kernel & SMP** — UEFI PE32+ boot straight to long mode; 4-level paging
with NX + write-combining; growable heap (64 MB → 2 GB). All cores boot
(no limit) via a Chase-Lev work-stealing scheduler with MONITOR/MWAIT
sleep, plus a stackful-fiber layer so blocking host calls yield instead
of pinning a core. Per-core idle quiesces to real HLT.

**Security & crypto** — 256-bit capability vault (delegation, temporal
scoping, transitive revocation, audit log). AES-256-GCM at rest via
AES-NI + PCLMULQDQ; BLAKE3 via AVX2. Full TLS 1.3 (RFC 8446, X25519 +
ECDH P-384, three cipher suites, X.509 chain validation with 4 embedded
root CAs) on RustCrypto primitives. Passphrase-derived identity
(BLAKE3-KDF), no accounts. OTA updates signed with ECDSA P-384 + SHA-384.

**npkFS v3** — content-addressed CoW B-tree, per-block BLAKE3, rotating
superblock, LRU cache, journalled crash recovery, batch TRIM, dedup
fastpath, per-entry mtime, auto-GC. ~480 MB/s write, ~370 MB/s read on
an N100 (encrypted, on top of NVMe).

**Networking** — Ethernet / ARP / IPv4 / ICMP / UDP / TCP from scratch,
plus DNS, DHCP, NTP, and an HTTP/HTTPS client with a keep-alive
connection pool and TCP window scaling (RFC 7323, ~gigabit).

**Drivers** — NVMe, xHCI USB (keyboard + mouse, HID boot protocol),
Intel I226-V and RTL8153 USB Ethernet, Intel Xe GPU (native 4K@60Hz
HDMI 2.0 + a Gen-12 BCS blitter for compositing), Intel HDA audio, an
AML interpreter driving firmware `_BST`/`_BIF` for a vendor-independent
battery %, and an Intel AX200 WiFi driver. WiFi, AML, and audio run as
**WASM modules**; hardware drivers are ported 1:1 from Linux.

**Desktop (Shade)** — Hyprland-style dwindle tiling compositor: rounded
corners via SDF, light/dark `theme=auto` following wallpaper luminance,
GPU-composited cursor, smooth swap animations, per-window scrolling. The
top bar and bottom dock are themselves WASM **panels**, alpha-composited
translucent over the wallpaper.

**Widget platform** — apps build a declarative `Widget` tree with the
`nopeek_widgets` SDK and commit it through one host function; the
compositor owns layout, rasterization (real Inter font metrics via
fontdue), theming, and animation — apps never touch pixels. Popover,
Scroll, TextArea (2-D caret + live syntax highlighting), self-editing
Input, Phosphor icon atlas, per-app capabilities, and file associations
(double-click → handler app) are all in.

**Apps** — `loft` (file browser: sidebar, grid/list, sortable columns,
copy/cut/paste/rename), `spell` (text editor: tabs, multi-language
syntax highlight, markdown preview), `iris` (image viewer), `snap`
(screenshot), `drun` (launcher), `top`/`cores` (system monitors).

**MicroVM** — a per-app KVM-style hypervisor inside the kernel (Intel
VT-x + EPT and AMD-V + NPT, vendor-symmetric), running a custom
9.5 MB **Linux 6.18-nopeek** build. virtio-blk / -net / -gpu / -input
backends, a 9p bridge that mounts npkFS into the guest, profile-image
persistence, and guest-SMP (one vCPU per host worker core). It runs
**LibreWolf** as a tiled window — the compatibility browser for the
legacy web.

---

## beak — the native browser (current focus)

The microVM browser works, but a whole Linux + Firefox behind the WASM
boundary is the one place nopeekOS leans on legacy. **beak** is the
answer: a web browser built from scratch as a *single WASM app*, no
Linux, no vendored engine.

It renders HTML → a real DOM → a full CSS cascade (UA sheet → author
`<style>` and external `<link>` sheets with selectors/specificity →
inline → `!important`) → layout → paint, all hand-rolled: block +
inline flow, tables, flexbox, CSS Grid, the box model, positioning,
custom properties, `calc()`, `@media`, CSS Color 4, PNG images and an
SVG subset. Plus **HTML forms** — typing into a page's search box and
pressing Enter submits it, no JavaScript involved. It fetches over the
kernel's TLS stack, is theme-aware, and scrolls smoothly.

Fidelity is measured, not guessed: the engine is a portable core with
no host dependencies, and the official **WPT reftests** (5,786 of them)
are vendored as a render-and-compare oracle that runs on the dev box —
currently **3,626 passing** (1,898 fail, 262 inconclusive). Nearly every
rendering bug this engine has had was found by rendering the real page
to a bitmap on Linux, without booting the OS.

Honest gaps: no JavaScript yet (Stage 1 — an interpreter, never a JIT,
since a JIT would need a hole in W^X; test262 is its oracle), JPEG
images fall back to placeholders, and the kernel still speaks only
HTTP/1.1 — which parts of the web now rate-limit outright. Spec and
status live in `BROWSER.md`, per-feature conformance in
`CONFORMANCE.md`.

---

## Technical Decisions

| Area | Choice | Why |
|------|--------|-----|
| Language | Rust (`no_std`, nightly, edition 2024) | Memory safety, no GC |
| Boot | UEFI (PE32+ direct) | Modern firmware, no GRUB |
| Sandbox | wasmi (interpreter, fuel-metered) | `no_std`, one trust boundary |
| Filesystem | npkFS (content-addressed, CoW) | BLAKE3, SSD-native, encrypted |
| At-rest AEAD | AES-256-GCM (AES-NI + PCLMULQDQ) | Hardware-accelerated |
| Hashing | BLAKE3 (AVX2) | Fast, streaming, verify on read |
| TLS | 1.3 (RFC 8446) | X25519 + P-384, RustCrypto |
| OTA | ECDSA P-384 + SHA-384 | Signed kernel + modules |
| Identity | Passphrase → BLAKE3-KDF | No users, no accounts |
| GPU | Intel Xe Gen 12.2 | 4K@60Hz HDMI 2.0, BCS blitter |
| Compositor | Shade (native Rust) | Dwindle tiling, layer-based |
| SMP | N cores (no limit) | Work-stealing + stackful fibers |
| Linux apps | MicroVM (VT-x / AMD-V) | Vendor HAL, mini-Linux + virtio |
| Hardware drivers | Ported 1:1 from Linux | Linux is the reference truth |

---

## Performance

npkFS on an N100 NUC (AirDisk 512 GB SSD, all figures **with**
AES-256-GCM at rest + BLAKE3 on every operation):

| Op | Throughput |
|----|------------|
| 1 MB write (dedup hit) | 479 MB/s |
| 1 MB read | 411 MB/s |
| 16 MB write (dedup hit) | 759 MB/s |
| 100 MB read | 406 MB/s |
| Mixed workload | W 491 MB/s · R 370 MB/s |

Crypto on the same N100: BLAKE3 ~1670 MB/s, AES-256-GCM ~715 MB/s dec /
~622 MB/s enc. Encrypted throughput lands near unencrypted ext4 on
comparable hardware, at ~2 W over idle on a fanless 6 W TDP CPU.

---

## Build & Run

```bash
# Prerequisites (Arch)
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
sudo pacman -S edk2-ovmf mtools gdisk qemu-system-x86
# or (Debian/Ubuntu): sudo apt install ovmf mtools gdisk qemu-system-x86

# Run
./build.sh build                 # Compile only
./build.sh qemu                  # Serial console (4 cores)
./build.sh qemu-gui              # Serial + VGA window
./build.sh debug                 # + GDB stub on :1234
./build.sh release               # Compile + sign (ECDSA P-384) → release/
./build.sh usb /dev/sdX          # USB installer (~30 MB, browser via OTA)
./build.sh usb-full /dev/sdX     # USB installer + LibreWolf bundle (~290 MB)
```

### Release + OTA flow

Every kernel or module change ships to hardware through this loop:

1. Bump the version (patch for a fix, minor for a feature).
2. Rebuild changed WASM modules and copy them into `release/modules/`.
3. `./build.sh release` — signs `kernel.efi` + all modules with
   `update.key` (ECDSA P-384) and regenerates the manifests.
4. Commit + push `release/` so `raw.githubusercontent.com/…/release/`
   serves the signed artifacts.
5. On the device: `update` (kernel) / `install <module>` (module) —
   each verifies SHA-384 + the ECDSA signature against the embedded
   root key in `kernel/src/crypto/update_key.rs` before touching disk.

> Skipping `./build.sh release` means OTA users keep getting the *last*
> signed release — a silent downgrade. It is mandatory after any
> kernel/module commit.

---

## Project Structure

```
nopeekOS/
├── build.sh                  # Build + QEMU/VirtualBox/USB
├── kernel/src/
│   ├── main.rs               # Entry, boot sequence
│   ├── boot.s / boot_uefi.rs # UEFI _start, ExitBootServices, GDT
│   ├── drivers/              # PCI, NVMe, xHCI, NICs, GPU, HDA, RTC, ...
│   ├── mm/                   # Frame allocator, growable heap, paging
│   ├── security/             # Capability vault, audit log, CSPRNG
│   ├── crypto/               # AES-GCM, BLAKE3, TLS 1.3, OTA key
│   ├── storage/npkfs/        # Content-addressed filesystem
│   ├── net/                  # Ethernet → ARP → IPv4 → TCP/UDP, DNS/DHCP/NTP
│   ├── smp/                  # MADT, SIPI, work-stealing + fibers
│   ├── gpu/                  # GOP + Intel Xe backend
│   ├── shade/                # Compositor: tiling, widgets, panels
│   ├── intent/               # The intent loop (dispatch, fs, net, http, ...)
│   ├── microvm/              # VT-x / AMD-V hypervisor + virtio + Linux loader
│   └── wasm.rs               # WASM runtime + host functions (npk_*)
└── tools/wasm/               # WASM apps & drivers
    ├── beak/ + beak-engine/  #   Native browser + portable render engine
    ├── loft/ spell/ iris/    #   File browser, editor, image viewer
    ├── snap/ drun/ top/      #   Screenshot, launcher, monitor
    ├── bar/ dock/ volume/    #   Panels + volume overlay
    ├── wifi/ aml/ audio_hda/ #   Hardware drivers as WASM
    └── sdk/widgets/          #   nopeek_widgets — declarative UI SDK
```

---

## Security Architecture

1. **Deny by default** — without a capability token, nothing happens
2. **Encrypted at rest** — every npkFS blob is AES-256-GCM, verified on
   read via BLAKE3
3. **Passphrase identity** — no users, no accounts; your passphrase is
   your identity
4. **256-bit tokens** — CSPRNG, Grover-resistant, least-privilege
5. **Temporal scoping** — module capabilities expire; rights only shrink
   on delegation
6. **One trust boundary** — the WASM sandbox contains apps *and* drivers
7. **Signed OTA** — ECDSA P-384 kernel + modules, SHA-384 integrity
8. **TLS 1.3 everywhere** — all network traffic encrypted

> Before every commit: *"Can a WASM module escape its sandbox through
> this change?"* If the answer isn't clearly **no**, it doesn't ship.

Today all artifacts are signed with a single ECDSA P-384 key. When
third-party modules become possible this evolves into a key hierarchy
(offline root signs sub-keys, per-publisher keys, revocation).

---

## What nopeekOS Is NOT

- **Not a Linux clone** — no systemd, no ext4, no procfs
- **Not POSIX** — no `fork()`, no `exec()`, no pipes
- **Not a unikernel** — multi-app, not single-purpose
- **Not a container runtime** — WASM modules are lighter than containers
- **Not an academic experiment** — every phase produces working code

---

## License

GPL-3.0 — see [LICENSE](LICENSE)

## Author

nopeek — [nopeek.ch](https://nopeek.ch) · from Luzern
