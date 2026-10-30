# nopeekOS

**A capability-based operating system where every app and driver is a
sandboxed WASM module.**

A bare-metal Rust kernel. No Unix. No POSIX. No root. No legacy from
the '70s. Apps, drivers (WiFi, audio, even the GPU) and a from-scratch
web browser all run inside the same WASM trust boundary. The kernel
underneath stays small in what it *trusts*, and keeps shrinking: every
driver that can live in a module moves there.

> **Status:** experimental and **not audited:** don't trust it with real
> data yet. Built by one person with [Claude Code](https://claude.com/claude-code)
> as a pair programmer; every change is tested on real hardware.

https://github.com/user-attachments/assets/474fe66d-89df-46f2-92ea-c1b92360587d

*A quick tour, recorded in QEMU. The same image runs on real hardware.*

---

## The Idea

Why does an operating system look the way it does? Mostly because of
decisions made decades ago: processes because that's how 1960s hardware
was shared, `chmod 755` because that's how 1971 multi-user Unix drew its
lines, a filesystem tree because that's how Multics organised a disk in
1965.

nopeekOS asks a different question: **what does an OS look like if you
start over today?**

- A **capability vault** instead of permissions: no `chmod`, no ACLs, no root
- **WASM sandboxes** instead of processes: one trust boundary for everything
- A **content-addressed store** (npkFS) instead of a filesystem tree
- **Intents** instead of commands: express what you want, not the protocol
- **Encrypted by default:** every object on disk is AES-256-GCM at rest

The kernel is written in Rust (`no_std`, nightly, x86_64) and boots
straight from UEFI. Everything above it (the panels, the file browser,
the text editor, the password manager, the WiFi and GPU drivers, the web
browser) is a signed, capability-gated WASM module. And where the legacy
world is still needed, it gets a cage of its own: a full Linux with
Firefox runs in a microVM, as one tiled window.

---

## What It Can Do Today

A tiling desktop with real apps, networking, encrypted storage, GPU
drivers of its own, and a native web browser, running on bare metal
(the machines in [HARDWARE.md](HARDWARE.md): an Intel Tiger Lake NUC and
an AMD Ryzen 5 5500U laptop) and in QEMU.

```
# Desktop
Mod+D                    # App launcher (drun): search + Enter to launch
Mod+F                    # Fullscreen for any window (Firefox: F11 / video too)
loft                     # File browser: grid/list, copy/cut/paste, sortable columns
spell                    # Text editor: tabs, syntax highlight, markdown preview
iris                     # Image viewer (PNG)
tune                     # Player: MP3 and WAV (folder as playlist), MP4 video
snap                     # Screenshot to npkFS
vault                    # Password manager on the kernel's keyvault
link                     # Network card: address, networks, connect/forget
beak [url]               # Native web browser (from scratch, no Linux, own JS)
firefox                  # Firefox in a Linux microVM

# The intent loop (terminal)
npk> status              # System overview (memory, capabilities, disk, net)
npk> store notes v=1     # Store an object (keyed BLAKE3 address, encrypted at rest)
npk> fetch notes         # Retrieve, decrypt and authenticate it
npk> grep TODO notes     # Search inside an object
npk> https nopeek.ch /   # HTTPS GET (TLS 1.3): add `> page` to store the body
npk> install <module>    # Fetch a module, checked against the signed manifest
npk> update              # OTA update of kernel, modules and assets (signed)
npk> cert list           # TLS root store: list, add, remove a CA
npk> time                # Wall clock (RTC, NTP, time zone)
npk> driver wifi_rtl8822ce  # Start a driver module by hand (normally autostart)
npk> wlan                # Link report: rate, retries, airtime, handshake rung
npk> wlan connect <name> # Join a network; the password goes into the vault
npk> python fib.py       # CPython 3.13 as a WASM module (WASI)
npk> forge <module>      # Translate a module with forge and report the cost
npk> cores               # Per-core CPU and per-engine GPU load
npk> usb                 # What the USB driver enumerated, with descriptors
```

Intents that read, write or run something check the session's
capability first. No ambient authority, no sudo. The passphrase unlocks the disk key; no users, no accounts.
Everything npkFS stores is encrypted at rest.

---

## Architecture

```
 ┌────────────────────────────────────────────────────────────┐
 │  WASM apps & drivers (sandboxed, capability-gated)         │
 │  deck: the panel, status items, popup cards                │
 │  drun / loft / spell / iris / tune / snap / vault / link   │
 │  beak: from-scratch web browser (HTML/CSS/JS engine)       │
 │  gpu_intel_tgl / gpu_amd_renoir: GPU drivers (from Linux)  │
 │  wifi_rtl8822ce / wifid / acpi / i2c_hid / audio_hda       │
 ├────────────────────────────────────────────────────────────┤
 │  MicroVM (compatibility layer)                             │
 │  VT-x / AMD-V hypervisor · custom Linux 6.18 · virtio      │
 │  virtio-gpu native context: the guest's Mesa on our GPU    │
 │  9p onto npkFS · runs Firefox as a tiled window            │
 ├────────────────────────────────────────────────────────────┤
 │  WASM engine: forge (WASM to x86-64, W^X) · wasmi          │
 │  host ABI (npk_*) · WASI · npk_scene_commit: widgets       │
 │  npk_fs_* · npk_http_* · npk_input_* · npk_vault_*         │
 │  npk_pci/mmio/dma/irq: capability-gated driver ABI         │
 │  gpudev: a GPU module becomes the system's GPU service     │
 ├────────────────────────────────────────────────────────────┤
 │  Kernel (Rust, no_std)                                     │
 │  SMP: one task inbox, per-core stackful fibers             │
 │  npkFS v4 (content-addressed, CoW, encrypted) · keyvault   │
 │  Crypto (AES-256-GCM · BLAKE3 · TLS 1.3 · ECDSA P-384)     │
 │  Shade compositor · network stack · IOMMU (VT-d/AMD-Vi)    │
 │  Capability vault · NVMe / xHCI / NIC drivers · GOP        │
 ├────────────────────────────────────────────────────────────┤
 │  Hardware: x86_64, UEFI                                    │
 └────────────────────────────────────────────────────────────┘
```

---

## Core Principles

### Capabilities, not permissions

No `chmod`, no ACLs, no root, no sudo. Every resource requires a
256-bit token drawn from the kernel's CSPRNG. A token is delegated only
with the same or fewer rights, expires on the kernel's tick clock, and
revoking it revokes everything delegated from it. A driver's token is
bound to one PCI device; a file picked in the file dialog is granted to
the asking app for exactly that path. Creations, revocations, denials and
expiries go into an audit ring; passing checks are counted. **Deny by
default:** without a token, nothing happens.

### WASM as the universal execution model

Every app and every driver is a WASM module, installed only if its
SHA-384 matches the signed module manifest, and stored encrypted in
npkFS. A module started from the launcher, a file or another app gets the
rights its `.npk.caps` section declares (one or two bytes; without one:
READ + EXECUTE + RENDER, never WRITE); `run` at the prompt adds
READ + WRITE + EXECUTE + RENDER. A hardware driver reaches its
device through capability-gated `npk_pci_*` / `npk_mmio_*` / `npk_dma_*`
/ `npk_irq_*` host functions, so the same sandbox contains a text editor
and a WiFi stack. A guest trap ends exactly one instance, nothing else.

### Content-addressed storage (npkFS)

No paths as identity, no hierarchy in the on-disk format. Objects are
identified by a keyed BLAKE3 hash of their content (keyed, so the disk
does not reveal which known files it holds); directories are Git-style
tree objects. Copy-on-write B-tree, dedup on write, AES-256-GCM at rest,
per-entry mtime, mark-and-sweep GC. Apps still see opaque path strings;
the tree is content-addressed underneath.

### Intents, not commands

Instead of `curl -X GET https://…`, you say `https example.com /`. The
system owns DNS, TCP, TLS, HTTP; you express intent, not protocol. Any
installed module can be called by its name at the prompt.

---

## What's Built

Grouped by subsystem. Versions and the full change history live in the
git log and in `release/manifest`.

**Kernel & SMP:** a UEFI PE32+ application, no bootloader; 4-level
paging with NX and write-combining; a heap with segregated free lists
and boundary tags (O(1) allocation and free) that grows in 64 MB steps
up to 2 GB. All cores boot (ACPI MADT, INIT-SIPI-SIPI). New work waits in
one shared inbox; each core runs its own queue of stackful fibers, so a
blocking host call yields instead of pinning a core, and an idle core
takes over a runnable fiber from a busy one. Idle cores sleep in
MONITOR/MWAIT or HLT; on Intel the deeper C-states come from the
firmware's `_CST`, as Linux `intel_idle` takes them. Device registers, ports, DMA
buffers and MSRs go through typed, bounds-checked accessors
(`kernel/src/hw.rs`); a panic stops every core and writes to COM1 and
the screen without taking a lock. The wall clock starts from the RTC, is
stepped by NTP and written back; local time follows a named or POSIX
time zone.

**Security & crypto:** 256-bit capability vault (delegation, expiry,
transitive revocation, audit ring). AES-256-GCM at rest via AES-NI +
PCLMULQDQ; BLAKE3 with AVX2. TLS 1.3 (RFC 8446: X25519 and P-384 key
exchange, the three TLS 1.3 cipher suites, CertificateVerify and SAN
checks, X.509 chain validation and expiry against 30 root CAs compiled
into the kernel, extendable at runtime through a `sys/certs` store) on
RustCrypto primitives. The CSPRNG is ChaCha20, seeded through BLAKE3 from
RDSEED/RDRAND, TSC jitter and the RTC. The passphrase unlocks a random
disk key through an Argon2id keyslot; no accounts. OTA manifests are
signed as a whole with ECDSA P-384, carry a monotonic `issued=` against
rollback, and name each artifact by its SHA-384.

**npkFS v4:** content-addressed CoW B-tree with keyed addresses and
encrypted objects (trees included), a random data key behind Argon2id
keyslots (changing the passphrase rewraps the key, it does not
re-encrypt the disk), AES-GCM authentication on every read, an 8-slot
rotating superblock, LRU block cache, a journal of deferred frees for
crash recovery, batched TRIM, dedup on write, chunked large files,
per-entry mtime, auto-GC.

**Networking:** Ethernet / ARP / IPv4 / ICMP / UDP / TCP from scratch,
plus DNS, DHCP, NTP, and an HTTP/HTTPS client with a keep-alive
connection pool and TCP window scaling (RFC 7323). **HTTP/2** (HPACK,
multiplexed streams) is offered through ALPN on every browser request,
with a per-host memory of who does not speak it; gzip on the receive
path. A port of Linux's fq_codel sits in front of the WiFi driver. OTA
and module installs stay on HTTP/1.1, fetch each artifact under its
content hash, and resume a broken download with a range request.

**Drivers:** the kernel itself has NVMe, xHCI USB (HID ported from Linux
`hid-core.c` and `usbhid`, hubs and hotplug), Intel Ethernet (e1000 /
e1000e / I219, and I225/I226 through a port of Linux `igc`), RTL8153 USB
Ethernet, virtio-blk and virtio-net, a PS/2 keyboard, the ACPI embedded
controller and SCI, a Smart Battery reader over the i801 SMBus, the UEFI
GOP framebuffer, and the IOMMU (Intel VT-d, AMD-Vi), which can confine a
device to its own page table. As **WASM modules** on a shared host-ABI
crate (`npk_sys`): both GPU drivers, Intel HDA audio (including sound to
a DisplayPort monitor), an I2C-HID touchpad, the `acpi` AML interpreter
(battery `_BST`/`_BIF`, EC events, processor idle states), and WiFi for
Realtek RTL8822CE. Hardware drivers are ported from Linux and kept as
close to it as possible, with Linux as the reference.

**GPU:** the kernel drives no GPU itself. It boots on GOP and the CPU; a
GPU module that comes up registers as the `gpudev` service, and from then
on the desktop is a scene list composed by the GPU's own compute shaders,
compiled at build time (`gpu_scene`, `gpu_shaders`). `gpu_intel_tgl` is
i915 for Tiger Lake (display readout and takeover of the firmware
framebuffer, all engines with execlists, DP audio). `gpu_amd_renoir` is
amdgpu for Renoir/Lucienne (PSP firmware load, SMU, GFX, SDMA, VCN + JPEG
video, GPU recovery); its display core (DC, DCN 2.1) runs only when
`gpu_amd_renoir.dc` is set. On both, a microVM guest's Mesa reaches the
card through a **virtio-gpu native context:** the guest gets real 3D and
hardware video decode, the module checks every command, and the kernel
offers it only once the IOMMU confines the card (`set gpu_iommu on`).

**WiFi:** rtw88 (RTL8822CE), ported from Linux and kept as close to it as
possible: firmware load, scan, roaming, A-MPDU aggregation and CCMP
replay checks in the driver; the WPA2 four-way and group-key handshakes
in a resident `wifid` supplicant. `wifid` is also the network manager: it
picks the best known network, remembers where it last joined for a fast
start, and takes a fresh DHCP lease when it moves. Known networks and
their passwords live in the keyvault, not in a file; the `link` card
shows and manages them. The `wlan` intent prints the driver's own report
(rate, retries, airtime, handshake rung, ring state) next to the
kernel's view, so a bad link says *which* stage is bad. The contract is
`docs/spec/WIFI_CLASS_ABI.md`.

**forge, the engine:** the default way a WASM module runs is our own
single-pass **WASM to x86-64 compiler inside the kernel** (`forge/core`,
decoding and validation by wasmparser). Code pages are W^X (writable
while emitted, executable after, never both); linear memory gets an
8 GiB reservation, so a wasm address (a `u32`) cannot reach past it and
the generator emits **no bounds check at all**; a trap unwinds any depth
of wasm frames in four instructions, which is what lets a WASI program
`proc_exit` cleanly. Each import is bound only if the module declares it
with the host routine's exact signature; one list in the kernel
(`host_imports!`) generates the forge adapters, the resolver and the
wasmi registrations, and the release gate (`tools/forge-gate.py`)
refuses any module forge cannot translate completely. wasmi stays in the
image as the second engine (`forge default off` makes it the default),
so the same binary can be compared on real hardware.

**Python:** CPython 3.13 runs as an ordinary signed WASM module. The
kernel implements `wasi_snapshot_preview1` against npkFS, so nothing
about Python is special-cased: it gets the same grant any WASI binary
would, plus two directories. `python fib.py`, stdlib from a bundle in
`sys/python`.

**Desktop (Shade):** Hyprland-style dwindle tiling compositor with rounded
corners via SDF, light/dark `theme=auto` following wallpaper luminance,
glass tinted and blurred from the wallpaper, swap animations, per-window
scrolling, fullscreen over the whole screen, and one UI scale factor
(`shade.scale`, fractions included) that every widget app, the window
chrome and the terminal follow. The panel is itself a WASM app: `deck`,
one bottom panel with launcher, workspaces, window list, clock and
**status items** (volume, network, battery, ...), whose popups (`volume`,
`link`) are windows of their own. Contract: `docs/spec/PANEL.md`.

**Widget platform:** apps build a declarative `Widget` tree with the
`nopeek_widgets` SDK and commit it through one host function; the
compositor owns layout, rasterization (Inter font metrics via fontdue),
theming, and animation; apps never touch pixels. Popover, Scroll,
TextArea (2-D caret, live syntax highlighting), self-editing Input, the
Phosphor icon atlas, per-app capabilities and file associations
(double-click opens the handler app) are in. Vocabulary and design
tokens: `docs/spec/WIDGET_VOCAB.md`, `docs/spec/UI_REFRESH.md`.

**Apps:** `loft` (file browser: sidebar, grid/list, sortable columns,
copy/cut/paste/rename), `spell` (text editor: tabs, multi-language
syntax highlight, markdown preview), `iris` (PNG viewer), `tune`
(player: MP3 + WAV with the folder as playlist, MP4 with H.264 video and
AAC audio, seek), `snap` (screenshot), `vault` (password manager:
entries, generator), `link` (network card), `volume` (volume card),
`pick` (the file dialog, as a portal), `drun` (launcher), `top` (system
monitor).

**Keyvault:** a secret is not a file. Secrets live in one encrypted
vault object with two tiers and two data keys: a **system tier** (WiFi
passwords) unlocked by a device key derived from the disk key, so the
network comes up without anyone typing, and handed out only against a
capability; and a **user tier** (website passwords, notes) behind a
vault password of its own. The device key opens the system tier, never
the user tier. No generic file call reaches either, only the vault.

**MicroVM:** a per-app hypervisor inside the kernel (Intel VT-x + EPT
and AMD-V + NPT, vendor-symmetric), running a custom **Linux 6.18** build
with an Arch Linux userspace. virtio-blk / -net / -gpu / -input / -snd /
-console / -balloon backends (freed guest pages go back to the host), a
9p bridge that mounts npkFS into the guest, a persistent home image,
guest SMP (vCPUs on the host's free worker cores), and speculation
mitigations between guest and host chosen per CPU as Linux `bugs.c`
chooses them. Its app is **Firefox** as a tiled window, with WebGL and
hardware video decode through VA-API (Intel's iHD, AMD's radeonsi) over
the native context. Each Linux app is a squashfs plus a home image,
delivered over OTA.

---

## beak, the native browser

The microVM browser works, but a whole Linux + Firefox behind the WASM
boundary is the one place nopeekOS leans on legacy. **beak** is a web
browser built from scratch as a *single WASM app*: no Linux, no vendored
engine.

It parses HTML into a DOM, runs a CSS cascade (UA sheet, author sheets,
inline style, `!important`; `@media`, `@container`, `@supports`,
`@layer`, `@font-face`, custom properties, `calc()`, CSS Color 4) and
lays out block and inline flow, tables, flexbox, grid, positioning and
floats, then paints into a canvas of its window. Images are PNG, JPEG,
ICO, lossy WebP and SVG; fonts include WOFF and WOFF2. Forms submit by GET
and POST, cookies persist, and every fetch goes over the kernel's TLS 1.3
and HTTP/2 client.

**JavaScript** is an engine of our own: parser, a bytecode VM with a
tree walker for what the compiler declines, RegExp, Proxy, BigInt, ES
modules, and a DOM binding with timers, microtasks, `fetch`, `XMLHttpRequest`,
`WebSocket`, `history`, custom elements and the mutation, resize and
intersection observers. An interpreter, never a JIT: a JIT would need a
hole in W^X.

The engine (`tools/wasm/beak-engine`) has no host dependencies, so its
test suites run on the dev box without booting the OS: WPT CSS reftests
and test262, with results in `docs/spec/CONFORMANCE.md`. On the device,
`beak:selftest` is a check page compiled into the module.

Gaps: no JIT (by design), no `<canvas>` context, no Workers, no
`WebAssembly`, no cycle collector (a page's reference cycles live until
the next navigation). The full description, including the security
model and every host function beak uses, is `docs/spec/BROWSER.md`.

---

## Technical Decisions

| Area | Choice | Why |
|------|--------|-----|
| Language | Rust (`no_std`, nightly, edition 2024) | Memory safety, no GC |
| Target | `x86_64-nopeek` (own spec) | Bare metal *with* SSE/AVX2/AES-NI |
| Boot | UEFI (PE32+ direct) | Modern firmware, no GRUB |
| WASM engine | forge, our own WASM to x86-64 compiler | Native code, W^X, no bounds checks |
| Second engine | wasmi (interpreter) | Same binary on two engines |
| Filesystem | npkFS (content-addressed, CoW) | Keyed BLAKE3 addresses, encrypted, batched TRIM |
| At-rest AEAD | AES-256-GCM (AES-NI + PCLMULQDQ) | Hardware-accelerated, authenticated on read |
| Hashing | BLAKE3 (AVX2) | Fast, keyed content addresses |
| TLS | 1.3 (RFC 8446) | X25519 + P-384, RustCrypto |
| OTA | Signed manifests (ECDSA P-384), SHA-384-named blobs | Rollback-protected, cache-proof |
| Identity | Passphrase, Argon2id keyslot, disk key | No users, no accounts |
| GPU | Driver modules (i915 TGL, amdgpu Renoir), else GOP + CPU | Desktop on the GPU, the CPU stays the fallback |
| Compositor | Shade (native Rust) | Dwindle tiling, scene lists |
| SMP | All cores, stackful fibers | A blocking host call yields its core |
| Linux apps | MicroVM (VT-x / AMD-V) | Vendor HAL, custom Linux + virtio |
| Guest 3D/video | virtio-gpu native context | Guest Mesa on the real card, every command checked |
| Secrets | Keyvault, two tiers | A secret is not a file |
| Hardware drivers | Ported from Linux, as close as possible | Linux is the reference |

---

## Build & Run

```bash
# Prerequisites (Arch): rustup installs the nightly pinned in
# rust-toolchain.toml on the first build
sudo pacman -S rustup edk2-ovmf mtools gdisk binutils python openssl qemu-system-x86
# or (Debian/Ubuntu): sudo apt install ovmf mtools gdisk binutils python3 openssl qemu-system-x86

# Run
./build.sh build                 # Compile kernel + bootable image
./build.sh qemu                  # Serial console (KVM, 4 cores)
./build.sh qemu-gui              # Serial + framebuffer window
./build.sh debug                 # + GDB stub on :1234
./build.sh qemu-intel            # TCG with an Intel CPU model (also qemu-amd)
./build.sh installer             # Two-pass installer build with bundled assets
./build.sh usb /dev/sdX          # USB installer (Linux apps arrive via OTA)
./build.sh release               # Compile + sign (ECDSA P-384) into release/
```

The full list is in the header of `build.sh`. WASM modules are built and
staged with `tools/stage-module.sh <name>`; host-side tests run with
`cargo test` inside a crate (e.g. `forge/core`, `tools/wasm/beak-engine`).

### Release + OTA flow

Every kernel or module change ships to hardware through this loop:

1. Bump the version in `kernel/Cargo.toml` and sync the lock
   (`cargo update --offline -p nopeekos-kernel`).
2. Rebuild changed WASM modules and stage them: `tools/stage-module.sh <name>`.
3. `./build.sh release` builds the kernel, runs the forge gate over
   `release/modules/` (nothing is signed if a module fails it), signs the
   kernel, modules and assets with `update.key` (ECDSA P-384), writes and
   signs the three manifests with one `issued=` time, and copies every
   artifact to `release/blobs/<sha384>`. A module-only change can use
   `./build.sh sign-modules`, which re-signs only modules whose hash
   changed and leaves the kernel alone.
4. Commit + push `release/` so `raw.githubusercontent.com/…/release/`
   serves the signed artifacts. Large guest images go to a GitHub
   release (`./build.sh release-large`), and the manifest points at them
   with a `url=` line.
5. On the device: `update` checks each manifest's signature against
   the key in `kernel/src/crypto/update_key.rs` and its `issued=`
   against the last one seen, then fetches each changed artifact by its
   hash and checks the SHA-384 before touching disk.

> Skipping `./build.sh release` means OTA users keep getting the *last*
> signed release, a silent downgrade. It is mandatory after any
> kernel/module commit.

---

## Project Structure

```
nopeekOS/
├── build.sh                  # Build, QEMU, installer, USB, release
├── docs/                     # spec/ = contracts the code follows
├── targets/                  # x86_64-nopeek.json: the kernel's own target
├── forge/                    # core/ = WASM to x86-64 compiler, harness/ = host runner
├── hid/                      # hid_core: HID report parser (from Linux hid-core.c)
├── kernel/src/
│   ├── main.rs               # Entry, boot sequence
│   ├── boot.s / boot_uefi.rs # UEFI _start, ExitBootServices, GDT
│   ├── hw.rs                 # Typed MMIO, ports, DMA, firmware tables, MSRs
│   ├── fatal.rs              # Panic path: stop all cores, lock-free output
│   ├── drivers/              # PCI, NVMe, xHCI, NICs, IOMMU, ACPI, EC, RTC, ...
│   ├── mm/                   # Frame allocator, heap, paging
│   ├── security/             # Capability vault, audit log, CSPRNG, keyvault
│   ├── crypto/               # AES-GCM, keyslots, TLS 1.3, OTA key
│   ├── storage/npkfs/        # Content-addressed filesystem
│   ├── net/                  # Ethernet, ARP, IPv4, TCP/UDP, DNS/DHCP/NTP, fq_codel
│   ├── smp/                  # MADT, SIPI, task inbox, fibers
│   ├── time/                 # Wall clock, time zones
│   ├── gpu/                  # GOP + gpudev (the GPU-module service)
│   ├── gui/                  # Login screen, fonts, theme
│   ├── shade/                # Compositor: tiling, widgets, panels
│   ├── intent/               # The intent loop (dispatch, fs, net, http, update, ...)
│   ├── microvm/              # VT-x / AMD-V hypervisor + virtio + Linux loader
│   ├── forge_rt.rs           # forge address space, W^X mapping, host trap
│   ├── wasi.rs + wasi/       # wasi_snapshot_preview1 against npkFS
│   ├── wasm.rs               # Module lifecycle, engine glue
│   └── wasm/                 # host_core.rs (npk_* calls), forge_glue.rs (import list)
├── microvm/linux/            # Guest init, Mesa, compositor and iHD builds
├── microvm-linux/            # Guest kernel build (Linux 6.18 + config)
├── microvm-userspace/        # Guest userspace bundle (Arch Linux + Firefox)
├── release/                  # Signed OTA tree that devices update from
└── tools/wasm/               # WASM apps & drivers
    ├── beak/ + beak-engine/  #   Native browser + portable engine
    ├── loft/ spell/ iris/    #   File browser, editor, image viewer
    ├── tune/                 #   Audio and video player
    ├── snap/ drun/ top/      #   Screenshot, launcher, monitor
    ├── vault/ link/          #   Password manager, network card
    ├── deck/ volume/         #   Panel + volume card
    ├── pick/ wallpaper/      #   File-dialog portal, wallpaper decoder
    ├── gpu_intel_tgl/ gpu_amd_renoir/  # GPU drivers (i915, amdgpu)
    ├── gpu_scene/ gpu_shaders/  #   Shared scene compositor + shaders
    ├── wifi_rtl8822ce/       #   WiFi driver (rtw88)
    ├── wifid/ acpi/ i2c_hid/ #   WPA supplicant, ACPI AML, touchpad
    ├── audio_hda/            #   Intel HDA driver
    ├── debug/ testdisk/      #   Reverse debug shell, npkFS benchmark
    ├── vendor/               #   Vendored H.264 and AAC decoders (for tune)
    └── sdk/                  #   npk_sys (host ABI) + widgets (declarative UI SDK)
```

---

## Security Architecture

1. **Deny by default:** without a capability token, nothing happens
2. **Encrypted at rest:** every npkFS object is AES-256-GCM, and its tag
   is checked on every read
3. **Passphrase identity:** no users, no accounts; the passphrase
   unlocks the disk key through an Argon2id keyslot
4. **256-bit tokens:** from a ChaCha20 CSPRNG, least-privilege
5. **Temporal scoping:** module capabilities expire; rights only shrink
   on delegation
6. **One trust boundary:** the WASM sandbox contains apps *and* drivers;
   a module's memory grows only while it leaves the rest of the system
   its reserve, so a runaway module fails, not the machine
7. **Devices behind the IOMMU:** VT-d / AMD-Vi can confine a device's
   DMA to the pages it was given (today opt-in, for the GPU, with
   `set gpu_iommu on`); guest native contexts require it
8. **Secrets in the keyvault:** never a file, handed out by policy
9. **Signed OTA:** signed manifests with rollback protection, every
   artifact checked against its SHA-384
10. **TLS 1.3 by default:** OTA, installs and module requests go over TLS;
    a module gets plaintext HTTP only for literal private or loopback
    addresses when `net.allow_plain_http` is on
11. **Guest isolation:** speculation mitigations between the microVM and
    the host are chosen per CPU, as Linux chooses them

> Before every commit: *"Can a WASM module escape its sandbox through
> this change?"* If the answer isn't clearly **no**, it doesn't ship.

All artifacts are signed with a single ECDSA P-384 key. Vulnerabilities
are reported privately, see [SECURITY.md](SECURITY.md).

---

## What nopeekOS Is NOT

- **Not a Linux clone:** no systemd, no ext4, no procfs; the one Linux in
  the system is the microVM guest
- **Not POSIX:** no `fork()`, no `exec()`, no pipes
- **Not a unikernel:** multi-app, not single-purpose
- **Not a container runtime:** an app is a WASM instance, not a Linux
  process in a namespace
- **Not only a paper design:** it runs on the hardware in
  [HARDWARE.md](HARDWARE.md)

---

## Community

- **Questions, ideas, design arguments:** [Discussions](https://github.com/fnopeek/nopeekOS/discussions)
- **Which device comes next:** [HARDWARE.md](HARDWARE.md); vote on hardware requests with 👍
- **Security issues:** privately, see [SECURITY.md](SECURITY.md)
- **Code:** read [CONTRIBUTING.md](CONTRIBUTING.md) first
- **Website:** [nopeek.ch](https://nopeek.ch)

---

## License

nopeekOS is dual-licensed.

- **Open source: [AGPL-3.0-or-later](LICENSE).** Use it, study it, change
  it, share it, privately or commercially. If you distribute a modified
  version, or offer it to others over a network, you publish your source
  under the same license.
- **Commercial license on request.** For building nopeekOS or its SDK
  into a product or service without the AGPL's source obligations,
  contact florian@nopeek.ch.

Bundled third-party components keep their own licenses: Inter and IBM
Plex fonts (SIL OFL 1.1, texts in `release/assets/`), the Phosphor icons
(MIT), CPython (PSF), the vendored decoders in `tools/wasm/vendor/`
(rusty_h264: BSD-2-Clause, rusty_aac: Apache-2.0), the GPU firmware the AMD driver loads (AMD's
redistributable license), and the microVM guest (Linux: GPL-2.0, Mesa:
MIT, Firefox: MPL-2.0).

**Contributions** are welcome, but can only be merged under a
contributor license agreement that keeps dual licensing possible; ask
before sending a larger change.

"nopeekOS" is the project's name; the license covers the code, not the
name. A fork is welcome under a name of its own.

## Author

nopeek · [nopeek.ch](https://nopeek.ch)
