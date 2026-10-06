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
native web browser — running on bare metal (Intel N100 NUC, AMD Ryzen
laptop) and in QEMU.

```
# Desktop
Mod+D                    # App launcher (drun) — search + Enter to launch
loft                     # File browser: grid/list, copy/cut/paste, sortable columns
spell                    # Text editor: tabs, syntax highlight, save to npkFS
iris                     # Image viewer (PNG)
tune                     # Audio player (MP3, WAV) — folder as playlist
snap                     # Screenshot → npkFS
beak [url]               # Native web browser (from scratch, no Linux, own JS)
browser                  # LibreWolf in a Linux microVM (compatibility browser)

# The intent loop (terminal)
npk> status              # System overview (cores, RAM, disk, net)
npk> store notes v=1     # Store an object (BLAKE3-addressed, encrypted at rest)
npk> fetch notes         # Retrieve + decrypt + integrity-check
npk> find report         # Search object names   ·   grep TODO notes
npk> https nopeek.ch /   # HTTPS GET (TLS 1.3, hardware AES) — add `> page` to store
npk> install <module>    # Fetch + verify (ECDSA P-384) a signed WASM module
npk> update              # OTA update of kernel, modules and assets (signed)
npk> cert list           # TLS root store — inspect, add, drop a CA
npk> driver wifi_rtl8822ce  # Start a driver module by hand (normally autostart)
npk> wlan                # Link report: rate, retries, airtime, handshake rung
npk> wlan set ssid <name> # WiFi settings; passphrase: store /sys/config/wifi_psk
npk> python fib.py       # CPython 3.13 as a WASM module (own WASI layer)
npk> forge run <mod>     # Run a module on the compiler — A/B against wasmi
npk> cores               # Trustworthy per-core CPU instrumentation
```

Every operation is capability-gated. No ambient authority, no sudo.
The passphrase unlocks the disk key — no users, no accounts. All data
encrypted at rest.

---

## Architecture

```
 ┌──────────────────────────────────────────────────────────┐
 │  WASM apps & drivers (sandboxed, capability-gated)         │
 │  shade  — tiling compositor (theme, panels, animation)     │
 │  bar / dock  — top + bottom panels                         │
 │  drun / loft / spell / iris / snap — launcher + apps       │
 │  beak   — from-scratch web browser (HTML/CSS engine)       │
 │  wifi_* / wifid / aml / i2c_hid / audio_hda — drivers      │
 ├──────────────────────────────────────────────────────────┤
 │  MicroVM (compatibility layer)                             │
 │  VT-x / AMD-V hypervisor · custom Linux 6.18 · virtio      │
 │  bridges · 9p↔npkFS · runs LibreWolf as a tiled window     │
 ├──────────────────────────────────────────────────────────┤
 │  WASM engine: forge (WASM→x86-64, W^X) · wasmi fallback    │
 │  host ABI (npk_*) · wasi · npk_scene_commit — widgets      │
 │  npk_fs_* · npk_http_request · npk_input · npk_battery     │
 │  npk_pci/mmio/dma — capability-gated driver ABI            │
 ├──────────────────────────────────────────────────────────┤
 │  Kernel (Rust, no_std)                                     │
 │  SMP scheduler (work-stealing + stackful fibers)           │
 │  npkFS v4 (content-addressed, CoW, encrypted)              │
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

Every app and every driver is a WASM module — installed only if its
SHA-384 matches a signed manifest, stored encrypted and integrity-checked
in npkFS, and given exactly the capabilities it declares in a 1-byte
`.npk.caps` section (default: READ + EXECUTE + RENDER, never WRITE). A
hardware driver reaches its device through capability-gated `npk_pci_*`
/ `npk_mmio_*` / `npk_dma_*` host functions — the same sandbox contains
a text editor and a WiFi stack. A guest trap kills exactly one instance,
nothing else.

### Content-addressed storage (npkFS)

No paths as identity, no hierarchy in the on-disk format. Objects are
identified by a keyed BLAKE3 hash of their content (keyed, so the disk
does not reveal which known files it holds); directories are Git-style
tree objects. Copy-on-write B-tree, dedup on write, AES-256-GCM at rest,
per-entry mtime, mark-and-sweep GC. Apps still see opaque path strings — the tree
is content-addressed underneath.

### Intents, not commands

Instead of `curl -X GET https://…`, you say `https example.com /`. The
system owns DNS, TCP, TLS, HTTP — you express intent, not protocol.

---

## What's Built

Grouped by subsystem. Versions and the full change history live in the
git log and in `release/manifest`.

**Kernel & SMP** — UEFI PE32+ boot straight to long mode; 4-level paging
with NX + write-combining; a growable size-class heap (64 MB → 2 GB) that
costs ~1 step per allocation instead of walking a free list. All cores boot
(no limit) via a Chase-Lev work-stealing scheduler with MONITOR/MWAIT
sleep, plus a stackful-fiber layer so blocking host calls yield instead
of pinning a core. Per-core idle quiesces to real HLT. Device registers,
ports, DMA buffers and MSRs go through typed, bounds-checked accessors
(`kernel/src/hw.rs`); a panic stops every core and writes to COM1 and the
screen without taking a lock.

**Security & crypto** — 256-bit capability vault (delegation, temporal
scoping, transitive revocation, audit log). AES-256-GCM at rest via
AES-NI + PCLMULQDQ; BLAKE3 via AVX2. Full TLS 1.3 (RFC 8446, X25519 +
ECDH P-384, three cipher suites, CertificateVerify and SAN checks, X.509
chain validation and expiry against 30 embedded root CAs, extendable at
runtime through a `sys/certs` store) on RustCrypto primitives. The
passphrase unlocks a random disk key through an Argon2id keyslot; no
accounts. OTA manifests are signed as a whole with ECDSA P-384, carry a
monotonic `issued=` against rollback, and name each artifact by its
SHA-384.

**npkFS v4** — content-addressed CoW B-tree with keyed addresses and
encrypted trees, a random data key behind Argon2id keyslots (changing the
passphrase rewraps the key, it does not re-encrypt the disk), per-block
BLAKE3, rotating superblock, LRU cache, journalled crash recovery, batch
TRIM, dedup fastpath, per-entry mtime, auto-GC.

**Networking** — Ethernet / ARP / IPv4 / ICMP / UDP / TCP from scratch,
plus DNS, DHCP, NTP, and an HTTP/HTTPS client with a keep-alive
connection pool and TCP window scaling (RFC 7323, ~gigabit). **HTTP/2**
(HPACK, multiplexed streams) carries the document fetch and beak's
subresource batches, with a per-host memory of who does not speak it;
gzip on the receive path. OTA deliberately stays on HTTP/1.1 — an update
that cannot be decoded is worse than a slow one — fetches each artifact
under its content hash, and resumes a broken download with a range
request.

**Drivers** — NVMe, xHCI USB (keyboard + mouse, HID boot protocol),
Intel I226-V and RTL8153 USB Ethernet, Intel Xe GPU (native 4K@60Hz
HDMI 2.0 + a Gen-12 BCS blitter for compositing), GOP framebuffer
elsewhere, Intel HDA audio, an I2C-HID touchpad, an AML interpreter
driving firmware `_BST`/`_BIF` for a vendor-independent battery %, and
WiFi for Intel AX200 and Realtek RTL8822CE. WiFi, the touchpad, AML and
audio run as **WASM modules** on a shared host-ABI crate (`npk_sys`);
hardware drivers are ported 1:1 from Linux.

**WiFi** — iwlwifi (AX200) and rtw88 (RTL8822CE) ported function by
function: firmware load, scan, roaming, a WPA2 four-way handshake in a
resident `wifid` supplicant with CCMP replay checks, A-MPDU aggregation,
fq_codel and AQL, which budgets airtime rather than bytes. The `wlan`
intent prints the driver's own report — rate, retries, airtime,
handshake rung, ring state — next to the kernel's view, so a bad link
says *which* stage is bad.

**forge — the engine** — the default way a WASM module runs is our own
single-pass **WASM→x86-64 compiler inside the kernel**.
Code pages are W^X (writable while emitted, executable after, never
both); linear memory gets an 8 GiB reservation, so a wasm address — a
`u32` — cannot reach past it and the generator emits **no bounds check at
all**; a host function can trap out of any depth of wasm frames in four
instructions, which is what lets a WASI program `proc_exit` cleanly.
Each import is bound only if the module declares it with the host
routine's exact signature; one list in the kernel (`host_imports!`)
generates the forge adapters, the resolver and the wasmi registrations,
and a release gate refuses any module forge cannot translate completely.
Measured on the device against wasmi on the same binary: **10.1x** on the
run phase of a CPython workload. wasmi stays in the image so the A/B
comparison stays possible on real hardware: `run` vs `forge run`,
`python` vs `forge python`.

**Python** — CPython 3.13 runs as an ordinary signed WASM module. The
kernel implements `wasi_snapshot_preview1` against npkFS, so nothing
about Python is special-cased: it gets the same grant any WASI binary
would, plus two directories. `python fib.py`, stdlib from a bundle in
`sys/python`.

**Desktop (Shade)** — Hyprland-style dwindle tiling compositor: rounded
corners via SDF, light/dark `theme=auto` following wallpaper luminance,
glass panels tinted from the wallpaper, smooth swap animations,
per-window scrolling. The
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
syntax highlight, markdown preview), `iris` (image viewer), `tune`
(audio player: MP3 + WAV, folder as playlist, seek), `snap`
(screenshot), `drun` (launcher), `top`/`cores` (system monitors).

**MicroVM** — a per-app KVM-style hypervisor inside the kernel (Intel
VT-x + EPT and AMD-V + NPT, vendor-symmetric), running a custom
9.5 MB **Linux 6.18-nopeek** build. virtio-blk / -net / -gpu / -input
backends, a 9p bridge that mounts npkFS into the guest, profile-image
persistence, guest-SMP (one vCPU per host worker core), sound and
networking. It runs **LibreWolf** as a tiled window — the
compatibility browser for the legacy web.

---

## beak — the native browser

The microVM browser works, but a whole Linux + Firefox behind the WASM
boundary is the one place nopeekOS leans on legacy. **beak** is the
answer: a web browser built from scratch as a *single WASM app*, no
Linux, no vendored engine.

It renders HTML → a real DOM → a full CSS cascade (UA sheet → author
`<style>` and external `<link>` sheets with selectors/specificity →
inline → `!important`) → layout → paint, all hand-rolled: block +
inline flow, tables, flexbox, CSS Grid, the box model, positioning,
custom properties resolved at *use* rather than at definition, `calc()`,
`@media`, CSS Color 4, PNG / JPEG / WebP / ICO images and an SVG subset.
Plus HTML forms — GET *and* POST, cookies, and the charset rules real
pages depend on (header → `<meta>` → sniffing, cp1252). It fetches over
the kernel's TLS stack (HTTP/1.1 and HTTP/2, gzip), is theme-aware, and
scrolls smoothly.

**JavaScript is in, and pages react.** A whole engine of our own: lexer,
parser, RegExp, DOM bindings, an event loop with timers and microtasks —
running on a **bytecode VM**, where the state is an array rather than the
Rust stack, and nearly every program executes on it. The language side has
`Proxy`, `BigInt` on our own bignum, `eval` (direct and indirect), the
iterator helpers, strict mode as a *difference* rather than a flag,
private fields with a brand, and a `defineProperty` that really checks.
The web side has ES modules with cycles and live bindings, custom
elements, `history`, `fetch`, `WebSocket`, the intersection and resize
observers, keyboard, mouse and focus events, and a form bridge — React
pages such as DuckDuckGo's results render. An interpreter, never a
JIT: a JIT would need a hole in W^X.

Fidelity is measured, not guessed. The engine is a portable core with no
host dependencies, so every oracle runs on the dev box without booting
the OS — which is how nearly every bug it has had was found. **The
numbers live in files, not in this paragraph:**

| Oracle | What it measures | Runner |
|---|---|---|
| WPT CSS reftests | rendering against the reference, without test vehicles | `docs/spec/CONFORMANCE.md` |
| test262 | the JavaScript language, with V8 on the same corpus as yardstick | `…/beak-engine/tests/test262.rs` |
| DOM surface | a Chromium call census of real pages | `…/beak-engine/tests/apigap.rs` |

The counterpart runs on the device: `beak:selftest`, a check page baked
into the image that fetches nothing and reports on screen *and* in the
log. One run found nine gaps that weeks of foreign pages had not.

Honest gaps: no JIT (by design), no `<canvas>` 2D context, no Shadow DOM
beyond the interface, and no cycle collector — a page whose framework
builds reference cycles grows until the module's memory cap ends it.
Spec and status live in `docs/spec/BROWSER.md` — that file will not
drift, this paragraph will.

---

## Technical Decisions

| Area | Choice | Why |
|------|--------|-----|
| Language | Rust (`no_std`, nightly, edition 2024) | Memory safety, no GC |
| Target | `x86_64-nopeek` (own spec) | Bare metal *with* SSE/AVX2/AES-NI |
| Boot | UEFI (PE32+ direct) | Modern firmware, no GRUB |
| WASM engine | forge — own WASM→x86-64 compiler | 10x the interpreter, W^X, `no_std` |
| Fallback engine | wasmi (interpreter) | Keeps an A/B measurement possible |
| Filesystem | npkFS (content-addressed, CoW) | BLAKE3, SSD-native, encrypted |
| At-rest AEAD | AES-256-GCM (AES-NI + PCLMULQDQ) | Hardware-accelerated |
| Hashing | BLAKE3 (AVX2) | Fast, streaming, verify on read |
| TLS | 1.3 (RFC 8446) | X25519 + P-384, RustCrypto |
| OTA | Signed manifests (ECDSA P-384), SHA-384-named blobs | Rollback-protected, cache-proof |
| Identity | Passphrase → Argon2id keyslot → disk key | No users, no accounts |
| GPU | Intel Xe Gen 12.2, else UEFI GOP | 4K@60Hz HDMI 2.0, BCS blitter |
| Compositor | Shade (native Rust) | Dwindle tiling, layer-based |
| SMP | N cores (no limit) | Work-stealing + stackful fibers |
| Linux apps | MicroVM (VT-x / AMD-V) | Vendor HAL, mini-Linux + virtio |
| Hardware drivers | Ported 1:1 from Linux | Linux is the reference truth |

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
2. Rebuild changed WASM modules and stage them: `tools/stage-module.sh <name>`.
3. `./build.sh release` — refuses any module forge cannot translate
   completely, then writes and signs the manifests with `update.key`
   (ECDSA P-384) and copies every artifact to `release/blobs/<sha384>`.
   A module-only change can use `./build.sh sign-modules` instead.
4. Commit + push `release/` so `raw.githubusercontent.com/…/release/`
   serves the signed artifacts.
5. On the device: `update` — checks each manifest's signature against
   the root key in `kernel/src/crypto/update_key.rs` and its `issued=`
   against the last one seen, then fetches each changed artifact by its
   hash and checks the SHA-384 before touching disk.

> Skipping `./build.sh release` means OTA users keep getting the *last*
> signed release — a silent downgrade. It is mandatory after any
> kernel/module commit.

---

## Project Structure

```
nopeekOS/
├── build.sh                  # Build + QEMU/VirtualBox/USB
├── docs/                     # spec/ = living contracts · plan/ = open
│                             # notes/ = how-tos · archive/ = shipped
├── targets/                  # x86_64-nopeek.json — the kernel's own target
├── forge/core/               # WASM→x86-64 compiler (no_std, host-testable)
├── kernel/src/
│   ├── main.rs               # Entry, boot sequence
│   ├── boot.s / boot_uefi.rs # UEFI _start, ExitBootServices, GDT
│   ├── hw.rs                 # Typed MMIO, ports, DMA, firmware tables, MSRs
│   ├── fatal.rs              # Panic path: stop all cores, lock-free output
│   ├── drivers/              # PCI, NVMe, xHCI, NICs, ACPI, EC, RTC, ...
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
│   ├── forge_rt.rs           # forge address space, W^X mapping, host trap
│   ├── wasi.rs               # wasi_snapshot_preview1 against npkFS
│   ├── wasm.rs               # Module lifecycle, engine glue
│   └── wasm/                 # host_core.rs (npk_* calls), forge_glue.rs (import list)
└── tools/wasm/               # WASM apps & drivers
    ├── beak/ + beak-engine/  #   Native browser + portable render engine
    ├── loft/ spell/ iris/    #   File browser, editor, image viewer
    ├── tune/                 #   Audio player (MP3/WAV -> kernel mixer)
    ├── snap/ drun/ top/      #   Screenshot, launcher, monitor
    ├── bar/ dock/ volume/    #   Panels + volume overlay
    ├── pick/ wallpaper/      #   File-dialog portal, wallpaper decoder
    ├── wifi_ax200/ wifi_rtl8822ce/  # WiFi drivers
    ├── wifid/ aml/ i2c_hid/  #   WPA supplicant, ACPI AML, touchpad
    ├── audio_hda/            #   Intel HDA driver as WASM
    └── sdk/                  #   npk_sys (host ABI) + widgets (declarative UI SDK)
```

---

## Security Architecture

1. **Deny by default** — without a capability token, nothing happens
2. **Encrypted at rest** — every npkFS blob is AES-256-GCM, verified on
   read via BLAKE3
3. **Passphrase identity** — no users, no accounts; the passphrase
   unlocks the disk key through an Argon2id keyslot
4. **256-bit tokens** — CSPRNG, Grover-resistant, least-privilege
5. **Temporal scoping** — module capabilities expire; rights only shrink
   on delegation
6. **One trust boundary** — the WASM sandbox contains apps *and* drivers
7. **Signed OTA** — signed manifests with rollback protection, every
   artifact checked against its SHA-384
8. **TLS 1.3 by default** — everything the system fetches goes over TLS;
   plaintext HTTP only for literal private addresses when
   `net.allow_plain_http` is on

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
