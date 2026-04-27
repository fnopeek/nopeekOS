# nopeekOS

**An AI-native operating system. Rethought from scratch.**

Not a Unix clone. Not POSIX. No legacy from the '70s.
A system where AI is the primary operator -- and the human is the conductor.

---

## The Idea

Why does an operating system look the way it does? Because in 1969 humans had to type every command manually. Because in 1984 a desktop metaphor was needed. Because in 2025 humans still manage processes, install drivers, and type `chmod 755`.

nopeekOS flips the question: **What remains when you remove fifty years of assumptions?**

- A **Capability Vault** instead of permissions
- A **WASM Sandbox** instead of processes
- An **Intent Loop** instead of a shell
- A **Content-Addressed Store** instead of a filesystem
- **Runtime-generated tools** instead of pre-installed software

Everything else is generated at runtime.

---

## What It Can Do Today

```
npk> status                          # Full system overview (cores, RAM, disk, net)
npk> store config version=1.0        # Store object (BLAKE3-hashed, encrypted at rest)
npk> fetch config                    # Retrieve + decrypt + integrity check
npk> list                            # All objects with hashes
npk> run hello                       # Execute WASM from npkFS (sandboxed, cap-gated)
npk> run fib 20                      # Compute fibonacci(20) = 6765 in WASM sandbox
npk> ping google.ch                  # ICMP ping (with DNS resolution)
npk> traceroute 8.8.8.8              # Network path tracing
npk> resolve google.com              # DNS resolution
npk> http example.com /              # HTTP GET (full TCP/IP stack)
npk> https sandbox.nopeek.ch /       # HTTPS GET (TLS 1.3, AES-256-GCM / ChaCha20)
npk> http example.com / > mypage     # Fetch and store in npkFS
npk> update                          # OTA update (ECDSA P-384 signed, SHA-384 verified)
npk> reboot                          # ACPI reset + PCI CF9 + triple-fault fallback
npk> uname                           # Kernel version info
npk> uptime                          # Time since boot
npk> history                         # Last 32 commands (arrow up/down to recall)
npk> lock                            # Lock system (clear keys)
npk> passwd                          # Change passphrase
npk> top                               # System monitor (WASM app: cores, memory, scheduler)
npk> install wallpaper                 # Install WASM module (signed, verified)
npk> install top                       # Install system monitor app
npk> install debug                     # Install remote debug shell
npk> uninstall wallpaper               # Remove module
npk> modules                          # List installed modules
npk> debug 192.168.1.50 22222         # Reverse mirror — laptop: `nc -l 22222`
npk> driver wifi                       # RTL8852BE bring-up + 3× scan (BSSID/SSID/ch)
npk> wallpaper demo                    # Generate 3 demo wallpapers + auto-theme
npk> wallpaper set ocean              # Set wallpaper (extracts theme colors)
npk> wallpaper random                  # Random wallpaper from collection
npk> wallpaper clear                   # Revert to aurora background
npk> install drun                      # Mod+D app launcher (widget-kind)
[ Mod+D ]                               # Open drun overlay — ↑↓ select, Enter launch, Esc close
npk> gpu init                          # Initialize Intel Xe GPU (auto 4K@60Hz)
npk> gpu 4k60                         # Switch to 4K@60Hz (HDMI 2.0 scrambling)
npk> gpu 4k                           # Switch to 4K@30Hz
npk> disk read 0                     # Raw sector hex dump
```

Every operation is capability-gated. No ambient authority. No root. No sudo.
All data encrypted at rest. Passphrase-based identity -- no users, no accounts.

---

## Architecture

```
 ┌──────────────────────────────────────────────────────────┐
 │  Linux Apps (Firefox, etc.)                              │
 │  MicroVM (VT-x/VT-d, Mini-Linux, virtio bridges)        │
 ├──────────────────────────────────────────────────────────┤
 │  WASM Modules (sandboxed, capability-gated)              │
 │  shade.wasm — Compositor (tiling, borders, bar, theme)   │
 │  loop.wasm  — Intent Loop (command dispatch, terminal)   │
 │  wallpaper.wasm — PNG decoder + color extraction         │
 │  wifi.wasm  — RTL8852BE WiFi driver (PCIe, DMA, FW)     │
 │  Future: file manager, browser, user apps                │
 ├──────────────────────────────────────────────────────────┤
 │  WASM Runtime                                            │
 │  wasmi v1.0 (interpreter, fuel-metered)                  │
 │  → Cranelift JIT (WASM → x86_64, near-native speed)     │
 ├──────────────────────────────────────────────────────────┤
 │  Host-Function API (npk_*)                               │
 │  npk_layer_write/composite — Layer-based rendering       │
 │  npk_fb_info — Screen dimensions, scale                  │
 │  npk_input_poll — Keyboard/mouse events                  │
 │  npk_fs_* — npkFS access    │  npk_net_* — Network      │
 │  npk_pci_*/mmio_*/dma_* — Driver ABI (capability-gated) │
 ├──────────────────────────────────────────────────────────┤
 │  SMP Scheduler              │  Layer Compositor          │
 │  Work-stealing pool         │  Background / Chrome /     │
 │  Core 0 = Kernel/IRQ        │  Text / Cursor layers      │
 │  Cores 1..N = Workers       │  Dirty-region compositing  │
 │  MONITOR/MWAIT wakeup       │  GPU BCS blit (ExecList)   │
 ├──────────────────────────────────────────────────────────┤
 │  npkFS                      │  Crypto Engine             │
 │  COW B-tree, BLAKE3 hashing │  ChaCha20-Poly1305 AEAD   │
 │  Rotating superblock        │  AES-128/256-GCM (TLS)    │
 │  LRU cache, WAL journal     │  TLS 1.3: X25519 + P-384  │
 │  Batch TRIM for SSD         │  ECDSA P-384 signatures   │
 ├──────────────────────────────────────────────────────────┤
 │  Capability Vault           │  OTA Updates               │
 │  256-bit tokens, deny-all   │  ECDSA P-384 signed        │
 │  Passphrase identity        │  SHA-384 verified           │
 │  Temporal scoping, audit    │  npk install (modules)     │
 ├──────────────────────────────────────────────────────────┤
 │  GPU HAL                    │  Network Stack             │
 │  GOP (QEMU/VBox/any HW)    │  Ethernet, ARP, IPv4       │
 │  Intel Xe (4K@60Hz HDMI)   │  ICMP, UDP, TCP            │
 │  VirtIO GPU (planned)       │  DNS, DHCP, NTP, HTTP/S   │
 ├──────────────────────────────────────────────────────────┤
 │  Kernel Core (Rust, no_std, Microkernel)                 │
 │  SMP (N cores), 64GB Paging, Heap, IDT, ACPI, Serial    │
 ├──────────────────────────────────────────────────────────┤
 │  Hardware: x86_64, Multiboot2                            │
 └──────────────────────────────────────────────────────────┘
```

---

## Core Principles

### Capabilities, Not Permissions

No `chmod`, no ACLs, no root, no sudo.
Every resource requires a cryptographic token (256-bit, ChaCha20 CSPRNG, post-quantum safe).
WASM modules receive delegated capabilities with limited rights and expiry.
Everything is audited.

**Security model: Deny by Default.** Without a token, nothing happens.

### Intents, Not Commands

Instead of `curl -X GET http://...`, you say: `http example.com /`.
The system handles DNS, TCP, HTTP -- the user expresses intent, not protocol details.

### Content-Addressed Storage (npkFS)

No paths. No hierarchy. Every object identified by its BLAKE3 hash.
SSD-native: Copy-on-Write B-tree, rotating superblock, batch TRIM/DISCARD.
Store a web page: `http example.com / > mypage` -- content-addressed caching.

### WASM as Universal Execution

Every execution is a sandboxed WASM module:
- Loaded from npkFS, BLAKE3-verified before execution
- Each module gets its own delegated capability token (READ+EXECUTE, 60s TTL)
- Fuel-metered: 10M instruction budget prevents infinite loops
- Host functions capability-gated: `npk_fetch` needs READ, `npk_store` needs WRITE

---

## Completed Phases

### Phase 1 -- Bare Metal Boot

- [x] Multiboot2 boot (32-bit Protected Mode -> 64-bit Long Mode)
- [x] Physical Memory Manager (bitmap allocator, contiguous allocation for DMA)
- [x] Heap Allocator (linked-list, first-fit, coalescing)
- [x] Virtual Memory (4-level paging, NX bit, 64GB identity-mapped via 1GB huge pages)
- [x] Interrupts (IDT + PIC, 100Hz timer)
- [x] Serial Console (COM1, 115200 baud, line editing)
- [x] VGA Boot Banner

### Phase 2 -- Capability System

- [x] Capability Vault (256-bit random token IDs (post-quantum safe))
- [x] ChaCha20 CSPRNG (RFC 7539, RDRAND-seeded, forward secrecy)
- [x] Token delegation with rights monotonicity
- [x] Temporal scoping (tick-based expiry)
- [x] Transitive revocation
- [x] Audit log (ring buffer, all operations logged)
- [x] Intent-capability coupling (every intent checked)

### Phase 3 -- WASM Runtime

- [x] wasmi v1.0 interpreter (no_std)
- [x] Fuel metering (10M instruction budget, prevents hangs)
- [x] Module loading from npkFS (BLAKE3 integrity check)
- [x] Per-module delegated capabilities (READ+EXECUTE, 60s TTL)
- [x] Host functions: `npk_log`, `npk_print`, `npk_fetch`, `npk_store`
- [x] Bootstrap modules: hello, fib, add, multiply (auto-stored on first boot)

### Phase 4 -- Block I/O + npkFS

- [x] PCI bus scanner (config space via 0xCF8/0xCFC)
- [x] virtio-blk driver (legacy PCI, 4KB block API, TRIM/DISCARD)
- [x] npkFS: Copy-on-Write B-tree (19 entries/leaf, 56 keys/internal, recursive split)
- [x] BLAKE3 content hashing + B-tree node checksums
- [x] 8-slot rotating superblock (SSD wear leveling)
- [x] LRU block cache (64 slots, 256KB, write coalescing)
- [x] 2-phase journal (crash-safe deferred frees, idempotent replay)
- [x] Indirect extent blocks (3 direct + chained indirect, unlimited file size)
- [x] Batch TRIM/DISCARD (merged adjacent ranges)
- [x] Next-fit allocator (amortized O(1) block allocation)
- [x] Intents: `store`, `fetch`, `delete`, `list`, `fsinfo`, `disk read/write`

### Phase 5 -- Network Stack

- [x] virtio-net driver (legacy PCI, RX/TX virtqueues, 32 RX buffers)
- [x] Ethernet frame handling
- [x] ARP (request/reply, 16-entry cache)
- [x] IPv4 (send/receive, header checksum, custom TTL)
- [x] ICMP (ping/pong, TTL exceeded for traceroute)
- [x] UDP (stateless, port-based listeners)
- [x] TCP (full state machine, no Nagle, 40ms delayed ACK, 3 retries max 10s)
- [x] DNS resolver (A records, 16-entry cache)
- [x] DHCP client (auto-configure IP at boot)
- [x] NTP client (SNTP, wall-clock time)
- [x] HTTP client (`http <host> [path]`, response stored in npkFS with `>`)
- [x] Traceroute, netstat

---

## Roadmap

### Phase 6 -- Crypto & Identity

- [x] ChaCha20-Poly1305 AEAD encryption (RFC 8439)
- [x] Passphrase-based identity (BLAKE3-KDF -> 256-bit master key)
- [x] Encryption at rest (all npkFS objects encrypted by default)
- [ ] Post-quantum crypto: ML-KEM (Kyber) + ML-DSA (Dilithium), hybrid with X25519/Ed25519
- [x] TLS 1.3 (RFC 8446, X25519 + ECDH P-384 key exchange)
- [x] TLS 1.3: AES-128-GCM-SHA256 + AES-256-GCM-SHA384 cipher suites (via RustCrypto aes-gcm crate)
- [x] HTTPS client (`https <host> [path]`)
- [x] X.509 certificate chain validation
- [x] Embedded root CAs (ISRG Root X1, DigiCert G2, AAA, GTS Root R1)
- [x] SHA-256, HMAC-SHA256, HKDF, RSA PKCS#1 v1.5 verify
- [x] X.509 SAN (Subject Alternative Name) for TLS hostname verification
- [x] ECDSA P-384 signature verification (OTA updates, PrehashVerifier)

### Phase 7 -- Bare Metal (target: Intel N100 NUC)

- [x] xHCI USB keyboard driver (BIOS handoff, HID boot protocol, disconnect detection)
- [x] PS/2 keyboard (extended scancodes, arrow keys, lock-free ringbuffer)
- [x] Intel I226-V Ethernet driver (igc, MMIO, RX/TX advanced descriptors, PCIe bridge)
- [x] Framebuffer driver (UEFI GOP, double-buffer shadow A/B, 32-bit pixel write)
- [x] TSC timer fallback (CPUID 0x15 calibration, replaces PIT on UEFI-only systems)
- [x] NVMe driver (PCIe BAR0 MMIO, Admin/IO queues, Read/Write/TRIM)
- [x] 64GB RAM support (1GB huge pages, dynamic memory detection)
- [x] Tick-based USB timeouts (CPU-speed independent)
- [x] ACPI power-off (RSDP, FADT, DSDT \_S5 parsing)
- [x] Gateway routing, serial detection, dual xHCI controllers
- [x] APIC timer (auto-detect PIT vs APIC, TSC-calibrated, 100Hz periodic)
- [ ] SSH-compatible remote access (replaces old npk-shell prototype)
- [x] USB mouse (HID boot protocol, xHCI multi-device, software cursor, click-to-focus, Mod+drag)
- [x] IRQ-driven USB polling (APIC timer drains xHCI, atomic SPSC ring buffers)
- [x] WASM driver model (drivers as sandboxed modules, capability-gated I/O)
- [x] WiFi driver: RTL8852BE firmware download (WASM module, PCIe DMA, MFW container)
- [ ] WiFi driver: MAC init, RF calibration, scan, association

### Phase 8 -- Human View

- [x] GUI login screen (Hyprlock-inspired: large clock, centered dots, pill input)
- [x] Spleen bitmap font system (8x16, 16x32, 32x64, BSD 2-Clause licensed)
- [x] Procedural aurora background (animated, per-frame generated)
- [x] 4K auto-scaling (2x when resolution >1920px)
- [x] Semi-transparent rounded rectangles with 4x4 subpixel anti-aliasing
- [x] Damage tracking for efficient framebuffer updates
- [x] OTA update system (`update` intent, ECDSA P-384 signed, SHA-384 verified, ESP FAT32 write)
- [x] `build.sh release` for signing kernel + manifest generation
- [x] HTTP/1.1 client (RFC 7230: Content-Length, chunked transfer, proper response parsing)
- [x] `reboot` intent (ACPI reset register + PCI CF9 + keyboard controller + triple-fault)
- [x] `uname`/`version`/`kernel`/`uptime` intents
- [x] Command history (arrow up/down, 32 entries, ring buffer)
- [x] AltGr support for Swiss German keyboard (@ # | [ ] { } \ ~)
- [x] Purple `[npk]` accent color in boot output
- [x] Shade compositor (Hyprland-inspired tiling WM, dwindle layout)
- [x] Per-window terminal sessions (heap-allocated, no limit, independent input/output)
- [x] Shadebar (Waybar-inspired: workspace indicators, clock, window title)
- [x] Window keybindings: Mod+Enter/Q/1-4/Arrows/Shift+Arrows/Ctrl+Arrows/F/V/PgUp/PgDn
- [x] Smooth window swap animation (ease-out cubic, 250ms)
- [x] Aurora background cache (render once, memcpy per region, ~100x faster)
- [x] USB mouse input (xHCI HID, composite device support, multi-device)
- [x] Software cursor overlay (truly lock-free, cached shadow_front AtomicPtr)
- [x] Click-to-focus, Mod+LMB swap-drag, Mod+RMB resize-drag (throttled 33fps)
- [x] Tiling-aware resize (adjusts dwindle split ratio, neighbors adapt)
- [x] Text cursor navigation (Left/Right/Home/End, insert at position, history recall)
- [x] KeyEvent abstraction (KeyCode + Modifiers, layout-independent, no ESC state machine)
- [x] GPU modesetting (Intel Xe Gen 12.2, native 4K@60Hz HDMI 2.0 via DDI-B, DPLL1, combo PHY)
- [x] HDMI 2.0 scrambling (GMBUS I2C, SCDC, DVI->HDMI mode switch, auto-fallback to 4K@30)
- [x] Write-Combining framebuffer (PAT MSR, ~5-10x faster blits)
- [x] USB keyboard key repeat (timer-based, 500ms delay, 50ms rate)
- [x] Growable heap (64MB initial, on-demand 64MB chunks, max 2GB, local O(1) coalescing)
- [x] Wallpaper demo (4 procedural themes, quarter-res, auto-theme extraction)
- [x] BCS blitter engine (Gen 12 ExecList, GPU-accelerated compositing)
- [x] GPU-composited cursor (save-under, eliminates blit race)
- [ ] VSync via PLANE_SURF double-buffer flip (zero-tearing + zero-latency)
- [ ] Web rendering engine (long-term)

### Phase 9 -- SMP & Event-Driven Architecture (in progress)

The kernel is transitioning to an event-driven microkernel. Core 0 becomes a thin
event dispatcher (IRQ + input + blit). All work moves to worker cores via the
Chase-Lev work-stealing scheduler. SMP is live -- all cores boot and steal work.

**SMP (Symmetric Multiprocessing)**
- [x] ACPI MADT parsing (Type 0 Local APIC + Type 9 x2APIC, no core limit)
- [x] AP trampoline (16-bit real -> 32-bit protected -> 64-bit long mode, copied to 0x8000)
- [x] INIT-SIPI-SIPI AP startup (sequential boot, atomic readiness counter)
- [x] Per-AP infrastructure (64KB stack, shared GDT/IDT/CR3, LAPIC enabled)
- [x] Tested on Intel N100 NUC (4 cores) and QEMU (configurable via -smp)
- [x] Work-stealing scheduler (Chase-Lev SPMC deque, 256 tasks/core)
- [x] MONITOR/MWAIT wakeup (C1E sleep, nanosecond wake on memory write)
- [x] spawn()/spawn_local() API with priority system (Critical/Interactive/Normal/Background)
- [x] Global WORK_AVAILABLE signal (cache-line aligned, all APs monitor)
- [x] Lock-free mouse cursor (AtomicI32 x/y, cached AtomicPtr shadow, no lock for movement)
- [x] Intel HWP auto-scaling (per-core, efficiency→turbo, CPUID-based)
- [x] WASM apps on worker cores (non-blocking, per-app key buffers)
- [x] Double-buffer framebuffer (shadow A/B, render→back, swap, blit←front, commit_front)
- [ ] Per-core APIC timer
- [ ] Thermal load balancing (migrate tasks when core >80% busy)

**Event-Driven Intent Architecture**
- [x] IntentSession struct on heap (input_buf, cursor, history, cwd per window)
- [x] Core 0 = event dispatcher only (never blocks >100μs)
- [ ] handle_key() as fire-and-forget task on worker core
- [x] execute_intent() spawns sub-tasks for heavy work
- [x] HTTP/HTTPS as async worker task (non-blocking, UI stays responsive)
- [x] OTA update as async worker task (non-blocking)
- [x] Module install as async worker task (non-blocking)

**App Display API**
- [x] `npk_print` / `npk_clear` — write/clear app's terminal display
- [x] `npk_input_wait(timeout_ms)` — blocking wait for key or timeout
- [x] `npk_sys_info(key)` — system information (cores, memory, freq, usage, processes)
- [x] Per-app SPSC key buffers (one per terminal, heap-allocated)
- [x] Inline key routing (shade keybinds intercepted, rest to app)
- [x] OTA module updates (`update` checks kernel + WASM modules)
- [x] Process tracking (per-app CPU time, memory, core, name, uptime)
- [x] Windows registered in process table (each loop gets a PID)
- [ ] Widget API (`npk_widget_list`, `npk_widget_input`, `npk_widget_select`)

**WASM Runtime**
- [x] wasmi v1.0 interpreter (register-based, fuel-metered)
- [x] Interactive execution on worker cores (1B fuel budget)
- [x] Dynamic process table (BTreeMap, heap-allocated, unlimited PIDs)

**WASM Driver ABI**
- [x] Hardware host functions: `npk_pci_config_read/write`, `npk_mmio_map/read/write`, `npk_dma_alloc`
- [x] Device-bound capability validation (each MMIO/DMA call checked)
- [x] PCI BAR auto-assignment + PCIe bridge window configuration
- [x] DMA buffer allocation below 4GB (32-bit TX BD constraint)
- [x] WiFi driver: RTL8852BE probe, power-on, XTAL SI, DLE/HFC, DMA rings
- [x] WiFi driver: firmware download (MFW cv-matching, WD+H2C, BDRAM, all sections)
- [x] WiFi driver: MAC init + full PHY table load (4212 regs), RFK baseline, set_channel
- [x] WiFi driver: BB gain parser (config_bb_gain_ax, 66 entries, gain_error → HW)
- [x] WiFi driver: 17 per-block DMAC/CMAC IMR enables + sys_init_ax re-assert
- [x] WiFi driver: H2CREG transport (SCH_TX_EN async TX-pause), fw_log_cfg
- [x] WiFi driver: full VIF init (port_update, dmac/cmac_tbl, macid_pause, role_maintain, join_info, addr_cam, default_cmac_tbl)
- [x] WiFi driver: scan_offload (3× 13-channel sweep, BSSID/SSID/channel dedupe)
- [x] WiFi driver: live beacon reception (Nachbar-APs + FritzBox im Scan sichtbar)
- [ ] WiFi driver: RSSI per AP, OUI vendor lookup
- [ ] WiFi driver: association (AUTH + ASSOC frames, 4-way WPA2 handshake, CCMP)
- [ ] WiFi driver: data path (TX + encrypted RX)

**Remote Debug (debug.wasm)**
- [x] Terminal stream sink (64KB ringbuffer per terminal, `npk_stream_open/read`)
- [x] Keyboard input injection (`npk_key_inject` into global KEY_BUF)
- [x] TCP user-API as host fns (`npk_tcp_connect/send/recv/close`)
- [x] Background WASM spawn (doesn't set `APP_RUNNING` → shell stays active)
- [x] `debug <ip> <port>` intent dials out to `nc -l` listener for live mirror

**GPU Rendering**
- [x] GPU HAL trait: init, set_mode, blit_rect_hw, flip, wait_vblank, supports_blit
- [x] BCS Blitter Engine (Gen 12 ExecList/ELSQ, XY_FAST_COPY_BLT)
- [x] GPU-accelerated compositing (shadow → MMIO via BCS, zero-CPU blit)
- [x] GPU-composited cursor (save-under pattern, no MMIO overlay race)
- [ ] VSync (PLANE_SURF double-buffer flip, zero-tearing + zero-latency)
- [ ] VirtIO GPU backend (QEMU/VBox support)

**Virtualization**
- [ ] MicroVM (VT-x/VT-d, Mini-Linux kernel for Linux app compatibility)
- [ ] virtio bridges for MicroVM (blk, net, gpu)

### Phase 10 -- Widget API & GUI Apps (in progress)

Declarative GUI for WASM apps. Apps build a `Widget` tree via the
`nopeek_widgets` SDK, serialize with postcard (version-prefixed), commit
through **one** host function (`npk_scene_commit`). The Shade compositor
owns layout, rasterization, GPU compositing, theming, animation. Apps
never touch pixels, fonts, or the framebuffer.

See `PHASE10_WIDGETS.md` for full architecture + ABI rules.

**SDK** (`tools/wasm/sdk/widgets/`)
- [x] Crate `nopeek_widgets 0.1.0` — no_std + alloc
- [x] `Widget` / `Modifier` / `Event` / `Action` / `Token` / `IconId` / `Role` / `TextStyle` — all `#[non_exhaustive]`, append-only, wire-version pinned
- [x] postcard serialize with `WIRE_VERSION = 0x01` prefix byte
- [x] Compile-time variant-order lock (`check_abi.rs`)
- [x] 8 host-side round-trip tests (tree, modifiers, events, reserved slots)

**Kernel — Compositor pipeline** (`kernel/src/shade/widgets/`)
- [x] ABI mirror of SDK with serde derives, postcard deserialize
- [x] `npk_scene_commit(ptr, len)` host fn — RENDER capability-gated
- [x] Serial pretty-printer for decoded tree + computed layout geometry
- [x] Flexbox-lite layout engine (Column/Row, Spacer flex, Align Start/Center/End/Stretch, Padding)
- [x] Real Inter Variable metrics via fontdue (`advance_width` / `measure` / `line_height` / `ascent` / `descent` / `cap_height` / `x_height`, kerning via `horizontal_kern`)
- [x] `CpuRasterizer` impl — clear / rect / text (alpha-composited glyphs) / icon-stub / canvas memcpy
- [x] Render walker — Widget + LayoutNode trees in lockstep, Background/Border modifiers → filled rects, variants → paint ops
- [x] Persistent scene overlay in shade's render cycle (widget survives terminal redraws)
- [x] Grid-aware placement (renders into focused window's content rect; fallback to centred preview)
- [x] Clear on window close (`Mod+Shift+Q`)
- [ ] Tile subdivision (512×512 tiles instead of one big window buffer)
- [ ] Diff + per-app cache (skip unchanged nodes between commits)
- [ ] Composition layers for opacity / transition / blur / shadow
- [ ] BCS batched blit of dirty tiles

**Font system** (`kernel/src/gui/text.rs`)
- [x] Inter Variable v4.1 (OFL) shipped via bundled assets + npkFS (`sys/fonts/inter-variable`)
- [x] BLAKE3-verified at load, fontdue-parsed
- [x] Glyph cache keyed by `(glyph, size, weight)`, LRU-managed via GGTT slab slots
- [ ] `tnum` tabular numerals enabled at load
- [ ] Shaping (`rustybuzz`, ligatures, BiDi — deferred to v2)

**GGTT slab allocator** (`kernel/src/gpu/ggtt_slab.rs`)
- [x] 7 fixed-bucket sizes (1K/4K/16K/64K/256K/**1M primary**/4M) over 912 MB GGTT region
- [x] Per-bucket freelist + LRU queue, eviction on overflow
- [x] Self-test intent: 1000+ alloc/free cycles, leak-free
- [x] Glyph atlas migrated to slab slots (CompSmall4K bucket)

**Icons**
- [x] Phosphor icon atlas (16/24/32/48/64 px logical, alpha-only) — build-time SVG rasterization
- [x] `IconId` enum populated (18 Regular variants, append-only)

**Events & interaction**
- [x] Mouse hit-test against layout tree → `Event::Action(ActionId)`
- [x] Keyboard → `Event::Key` routed to focused widget window (`read_line_with_tab` bailout, `npk_event_poll`)
- [ ] Focus stack + Tab navigation (within-widget focus, not window focus)
- [ ] `npk_event_wait` blocking host fn

**Animation**
- [x] Spring physics + linear curves, fixed-point Q16.16 scaffolding (v0.61.0)
- [ ] Self-scheduling 60Hz tick while interpolating, dirty-driven otherwise (no active consumers yet)

**Canvas (escape hatch)**
- [ ] `npk_canvas_commit(canvas_id, pixels, w, h, canvas_cap)` — CANVAS cap separate from RENDER
- [ ] Size caps: 4096×4096 px, 64 MB pixels total per app

**Window configuration (app-driven)**
- [x] `npk_window_set_overlay(w, h)` — app declares itself a centred overlay (bypasses tiling grid)
- [x] `npk_window_set_modal(modal)` — app declares itself modal, shade suppresses focus-shift shortcuts while visible
- [x] `npk_spawn_module(name_ptr, name_len)` — launcher apps spawn another module in a fresh terminal window (`loop + <app>` semantics)
- [x] `npk_close_widget()` — app tears down its own widget window
- [x] `npk_log_serial(ptr, len)` — direct serial logging, bypasses shade-terminal (safe when no loop is open)
- [x] `npk_list_modules(buf, max)` — enumerate installed `sys/wasm/*` modules (filters `.version` sidecars)

**First-party apps**
- [x] `files-stub` — P10.2 dummy commit app, bundled + OTA (`install files-stub`)
- [x] `drun` — Mod+D app launcher (centred overlay, modal, keyboard nav, Enter launches)
- [ ] `files` — real file browser (walks npkFS, opens via intent)

**Window-manager integration**
- [x] Widget-kind windows first-class in shade (own grid slot, rounded corners, separate from terminal windows)
- [x] Per-window scene storage (`SCENES: BTreeMap<WindowId, WidgetScene>`)
- [x] Widget follows focus / workspace switches correctly
- [x] `Window.is_overlay` + `Window.modal` flags (set by app, not by kernel)
- [x] Configurable launcher binding — `sys/config/launcher` (defaults to "drun")

Progress milestones (per `PHASE10_WIDGETS.md`):
- [x] P10.0 ABI freeze (`v0.50.7`)
- [x] P10.1 SDK + fontdue + Inter Variable (`v0.51.0`)
- [x] P10.2 `npk_scene_commit` + first end-to-end round-trip (`v0.54.0`)
- [x] P10.3 Layout engine with real font metrics (`v0.55.0`)
- [x] P10.4 GGTT slab allocator (`v0.56.0`)
- [x] P10.5 CPU rasterizer + first visible render (`v0.57.0`–`.2`)
- [x] P10.5b Widget-kind windows first-class in shade (`v0.58.0`)
- [x] Widget polish — rounded corners, Opacity, theme integration (`v0.58.1`)
- [x] P10.6 Diff+cache — payload-hash skip-render (`v0.59.0`, full diff pending)
- [x] P10.7 Event routing — mouse hit-test + `npk_event_poll` (`v0.60.0`, keyboard/blocking pending)
- [x] P10.8 Animation — Q16.16 math scaffold (`v0.61.0`, no active consumers yet)
- [x] P10.9 Phosphor icon atlas (`v0.62.0`) — **last visual checkpoint, 18 icons shipped**
- [x] drun — Mod+D app launcher (`v0.64.2` + drun `0.2.1`) — first interactive widget app, keyboard nav, modal overlay
- [x] drun v0.5.1 (`v0.75.x`) — live search / hover / click, icon + title + subtitle, AppMeta custom-section, reads own metadata from each wasm
- [x] SDK `style` + `prefab` modules — design tokens (Radius/Spacing/Padding) + cookbook (panel, searchbar, list_row, footer, badge, scroll_list)
- [x] `Modifier::Tint(Token)` — icons inherit accent color from theme
- [x] Two-theme palette (dark/light/auto) — curated surface/border/text per mode, wallpaper-derived accent with contrast adjust (`theme` intent)
- [x] Rounded-rect 16×16 centered subpixel AA across chrome + widget rasterizer
- [x] npkFS hardening — 6 write-path bugs fixed (`v0.73.x`): btree rightmost-child leak, TRIM partition-offset, indirect free-before-journal, store-leak on insert-fail, unjournaled indirect chain, partial-extent cache invalidation
- [x] Aurora procedural BG retired — kernel default is solid grey, all pixel generation lives in `wallpaper.wasm`
- [x] Wallpaper generator set — `solid`, `gradient` (2 & 4-corner bilinear), `pattern` (dots/stripes/checker/grid/noise), all inside `wallpaper.wasm`
- [x] **P10.11 file browser** — `loft` shipped (`kernel v0.76.0` + `loft 0.1.x`). Thunar-clone with sidebar (PLACES + DEVICES), toolbar (back/forward/up/refresh), breadcrumb, icon-grid, file-type icon heuristic. Hand-rolled `Modifier::Padding(8)` later replaced by `prefab::sidebar_pane` in v2.
- [x] **Vocab v2 — Tailwind-style modifiers + pseudo-state engine** (`kernel v0.77–0.79`, `sdk 0.2.0–0.4.0`):
  - 9 new `Modifier` variants append-only — `Hover` / `Focus` / `Active` / `Disabled` / `WhenDensity` (each `Vec<Modifier>`), `Scale(u16)` Q8.8, `MinWidth` / `MaxWidth` (u16), `Rounded(u8)`. Wire-version stays `0x01`.
  - New `Density` enum (`Compact <600 px / Regular 600–1200 / Spacious >1200`) drives `WhenDensity(d, …)` matching; `Motion` SDK helper (`Quick=120 / Normal=200 / Slow=400 ms`) lowers to existing `Transition::Linear`.
  - Compositor tracks per-window `hover_path` / `focus_path` / `active_path`; `effective_modifiers` merges state mods with CSS `:hover`-ancestor semantics; `Disabled` overrides interactive states + propagates through hit-testing.
  - **Tab / Shift+Tab** navigate focusable widgets in document order (DFS, wraparound, disabled-skipped). Click-to-focus + mouse-press → active state with re-rasterize triggered only when the tree has any pseudo-state mod (`has_pseudo` cache → zero cost on plain trees).
  - `prefab::card` / `button` / `input` / `dialog` / `sidebar_pane` added; `prefab::searchbar` removed (subsumed by `input(Search)` with optional trailing widget). All interactive prefabs now ship Hover + Focus + (where appropriate) Active states out of the box.
  - **`WIDGET_VOCAB.md`** at the repo root: single-file Tailwind-style cheat sheet for app developers and AI code-generators.
- [x] `Widget::Input` and `Widget::Button` respect `Modifier::Background` instead of hardcoding `SurfaceElevated` / `Accent` — lets prefabs own the chrome.
- [x] **SDF rounded corners** (`v0.79.4–.5`, kernel-only) — `gui/render.rs` switches the rounded-rect AA from 16×16 supersampling to a signed-distance-field + smoothstep pass with concentric two-arc geometry (Hyprland-style); border width is uniform across straights and curves. `fill_rounded_chrome_aa` gains a `paint_content` flag so widget windows leave the inner-full area transparent and the widget blit AAs against the chrome border via `rect_coverage_sdf` instead of leaking `win.bg_color` through the inner fringe.
- [x] **Layout-rect fix** (`v0.80.0`, abi+kernel) — `place_axis` for Row/Column/Stack now returns `rect: container` instead of `rect: content`, so `Modifier::Background` / `Modifier::Border` paint on the full allocated rect and children sit inside the padded inner. drun's selected list-row finally has 16 px breathing room around its accent border; hover backgrounds cover the full row including padding.
- [x] **`TextStyle::Heading`** (`v0.80.0`, ABI append, variant 5) — 18 px regular weight, sized between `Body` (14) and `Title` (24+bold). Used by `Widget::Input` placeholder + value so search bars read at a sensible size next to a 24 px magnifier icon. Wire-version stays `0x01`.
- [x] **Mockup-grade prefab polish round 1–3** (drun `0.5.7–0.5.10`, loft `0.1.7–0.1.10`, sdk `0.4.1–0.5.1`) — Raycast/Spotlight selection style (SurfaceElevated card + Accent border instead of AccentMuted fill), `prefab::input` no longer paints its own SurfaceElevated bg (blends into panel), tighter `Spacing::Xxs` between rows, `prefab::footer` wraps in a Column with a trailing zero-size widget so `Spacing::Md` acts as bottom-margin, `prefab::input` does the same for top-margin, `prefab::panel` gains a `Padding::Xs` inset so dividers and rows breathe vs the chrome, `widgets::suppress_hover(window_id)` on intent-loop keyboard dispatch so arrow-key nav owns the highlight until the next mouse motion.
- [ ] **`Widget::Input` self-editing** (next, ~2 d) — compositor-side cursor + key-routing-to-focused-input + Submit-on-Enter. Unblocks drun search without per-app `read_line` plumbing; apps still route Char/Backspace themselves today.
- [ ] **Tile subdivision + full diff cache** (~3–5 d) — 512×512 grid + per-tile content-hash, hover/key change → only dirty tiles re-rasterized instead of whole window.
- [ ] **Static visual effects** (`Shadow` / `Transition` / `Scale` outside pseudo-states) — needs compositing-layer pass (sub-tree → off-screen layer texture → blit with transform/effect). ~1 Woche, größerer Brocken.
- [ ] **P10.10 Canvas escape hatch** — `npk_canvas_commit` + `CANVAS` cap, on hold bis ein konkreter Consumer (image viewer, chart) danach fragt.

### Phase 11 -- AI Integration

- [ ] External AI service via network
- [ ] Intent resolution through LLM
- [ ] Runtime WASM generation (AI writes modules)
- [ ] Semantic search in content store

### Phase 11.5 -- npkFS v2: Real Content-Addressed Directories

Acknowledged tech debt. v1's path-as-key model leaks across the
intent loop, the wallpaper subsystem, loft, and every `.dir`-marker
write. List performance is `O(N_total)` per call; rename is
structurally impossible. Will not scale to AI-generated content.

- [ ] Tree-object format (Git-style `(name, hash, kind, size)` lists, encrypted)
- [ ] Walk-by-hash path resolution (`O(depth × log N)` instead of `O(N)`)
- [ ] `O(depth)` mutations + cheap rename
- [ ] One-shot `migrate-fs` intent for v1 → v2 conversion
- [ ] Mark-and-sweep GC, snapshots fall out for free
- [ ] Host-fn surface unchanged — apps don't rebuild

Spec + design rationale + migration sketch: see [`NPKFS_V2.md`](NPKFS_V2.md).

---

## Technical Decisions

| Area | Choice | Rationale |
|------|--------|-----------|
| Language | Rust (no_std, nightly, edition 2024) | Memory safety without GC |
| Boot | Multiboot2 | QEMU/GRUB compatible |
| Target | x86_64 | Later aarch64 |
| WASM | wasmi v1.0 | no_std, fuel metering |
| Filesystem | npkFS | COW, BLAKE3, SSD-native |
| Hashing | BLAKE3 | Fast, secure, streaming |
| CSPRNG | ChaCha20 (RFC 7539) | RDRAND seed, forward secrecy |
| AEAD | ChaCha20-Poly1305 (RFC 8439) | Encryption at rest + TLS |
| AES-GCM | aes-gcm crate (RustCrypto) | TLS 1.3 (128/256-bit) |
| TLS | 1.3 (RFC 8446) | X25519 + P-384, 3 cipher suites |
| Identity | Passphrase -> BLAKE3-KDF | No users, no accounts |
| Key Exchange | X25519 + ECDH P-384 | Ephemeral, per-connection |
| Certificates | X.509, 4 embedded root CAs | ISRG X1, DigiCert G2, AAA, GTS R1 |
| Crypto libs | sha2, hmac, hkdf, aes-gcm, p384 | RustCrypto, audited, no_std |
| Bitmap Font | Spleen (8x16, 16x32, 32x64) | BSD 2-Clause, clean glyphs |
| OTA Updates | ECDSA P-384 + SHA-384 | Signed manifests, ESP FAT32 write (4MB reserved) |
| TCP defaults | No Nagle, 40ms ACK, 3 retries | Optimized for request/response |
| GPU | Intel Xe Gen 12.2 (ADL-N) | Display-only, 4K@60Hz HDMI 2.0, GGTT+WC aperture |
| Compositor | Shade (native Rust) | Dwindle tiling, layer-based rendering |
| Rendering | Layer compositor + double-buffer | Shadow A/B swap, selective partial render, dirty-region compositing |
| GPU HAL | GOP + Intel Xe (+ VirtIO planned) | Vendor-neutral, same API for all backends |
| Input | KeyEvent (KeyCode + Modifiers) | Layout-independent, no ESC state machines, foundation for configurable keybindings |
| Mouse | xHCI HID boot protocol | Composite device, GPU-composited cursor (save-under), IRQ-driven polling |
| USB Polling | APIC timer (100Hz) | IRQ drains xHCI -> atomic SPSC buffers, no main-thread HW access |
| Heap | Growable (64MB->2GB) | On-demand 64MB chunks, local O(1) coalescing |
| Terminals | Heap-allocated (AtomicPtr) | ~264KB per window, on-demand alloc/free, no artificial limit |
| Animations | Ease-out cubic (250ms) | Integer math, tick-based, no floating point |
| Intent Model | Event-driven, heap state | Fire-and-forget tasks, no Core blocked when idle |
| Core 0 | Event dispatcher only | IRQ + input + blit, never blocks >100μs |
| Linux apps (future) | MicroVM (VT-x/VT-d) | Mini-Linux kernel, virtio bridges |
| Modules | npk install | ECDSA P-384 signed, SHA-384 verified, OTA from GitHub |
| WiFi | RTL8852BE (WASM driver) | PCIe MMIO, DMA, MFW firmware download, capability-gated |
| Driver ABI | Host functions (npk_pci_*, npk_mmio_*, npk_dma_*) | Stable ABI, device-bound, sandboxed |
| SMP | N cores (no limit) | Core 0 = event dispatcher, Cores 1..N = work-stealing pool |
| SMP Scheduler | Chase-Lev SPMC deque | Owner push/pop, thieves steal, MONITOR/MWAIT sleep |
| SMP Wakeup | MONITOR/MWAIT | Nanosecond wake on memory write, HLT fallback |
| Power | C-states per core | Idle cores sleep, wake on demand, thermal balancing |

---

## Performance: npkFS vs ext4

| Operation | npkFS | ext4 (theoretical) |
|-----------|-------|---------------------|
| Store | 5.2 I/O per op | 8-10 I/O per op |
| Fetch (cached) | 0 I/O | 2 I/O (cold) |
| Delete | 4.4 I/O per op | 8-10 I/O per op |

~50% fewer disk writes per operation = ~50% less SSD wear.

---

## Project Structure

```
nopeekOS/
├── build.sh                          # Build + QEMU/VirtualBox launch
├── kernel/
│   ├── Cargo.toml
│   ├── linker.ld                     # Memory layout (256KB stack, heap)
│   └── src/
│       ├── boot.s                    # Multiboot2 -> Long Mode
│       ├── main.rs                   # Kernel entry, boot sequence, module re-exports
│       ├── interrupts.rs             # IDT + PIC + APIC timer
│       ├── vga.rs                    # VGA text mode (boot banner)
│       ├── config.rs                 # Runtime configuration
│       │
│       ├── drivers/                  # Hardware drivers
│       │   ├── serial.rs             #   COM1 serial console + kprint macros
│       │   ├── pci.rs                #   PCI bus scanner
│       │   ├── nvme.rs               #   NVMe (PCIe, TRIM)
│       │   ├── virtio_blk.rs         #   virtio block device
│       │   ├── virtio_net.rs         #   virtio network device
│       │   ├── intel_nic.rs          #   Intel I226-V Ethernet
│       │   ├── xhci.rs              #   xHCI USB (keyboard + mouse)
│       │   ├── keyboard.rs           #   PS/2 keyboard
│       │   ├── framebuffer.rs        #   UEFI GOP framebuffer
│       │   ├── rtc.rs                #   Real-time clock
│       │   ├── blkdev.rs             #   Block device abstraction
│       │   ├── netdev.rs             #   Network device abstraction
│       │   └── acpi.rs               #   ACPI (power, MADT, table lookup)
│       │
│       ├── mm/                       # Memory management
│       │   ├── memory.rs             #   Physical frame allocator (bitmap)
│       │   ├── heap.rs               #   Growable heap (64MB->2GB)
│       │   └── paging.rs             #   4-level paging, NX, WC
│       │
│       ├── security/                 # Security subsystem
│       │   ├── capability.rs         #   Capability Vault (256-bit tokens)
│       │   ├── audit.rs              #   Audit log ring buffer
│       │   └── csprng.rs             #   ChaCha20 CSPRNG
│       │
│       ├── crypto/                   # Cryptography engine
│       │   ├── aead.rs               #   ChaCha20-Poly1305 AEAD, KDF
│       │   ├── update_key.rs         #   ECDSA P-384 OTA signing key
│       │   └── tls/                  #   TLS 1.3 stack
│       │       ├── mod.rs            #     Handshake + record layer
│       │       ├── sha256.rs         #     SHA-256/384 (via sha2 crate)
│       │       ├── hmac.rs           #     HMAC/HKDF
│       │       ├── x25519.rs         #     Curve25519 ECDH
│       │       ├── rsa.rs            #     RSA PKCS#1 v1.5 verify
│       │       ├── asn1.rs           #     ASN.1 DER parser
│       │       ├── x509.rs           #     X.509 certificate parser
│       │       └── certstore.rs      #     Root CAs + chain validation
│       │
│       ├── storage/                  # Storage subsystem
│       │   ├── fat32.rs              #   FAT32 (ESP access for OTA)
│       │   ├── gpt.rs                #   GPT partition detection
│       │   └── npkfs/                #   Content-addressed filesystem
│       │       ├── mod.rs            #     API: mkfs, mount, store, fetch
│       │       ├── types.rs          #     On-disk format
│       │       ├── btree.rs          #     COW B-tree
│       │       ├── cache.rs          #     LRU block cache
│       │       ├── bitmap.rs         #     Block allocation + TRIM
│       │       ├── superblock.rs     #     Rotating superblock ring
│       │       └── journal.rs        #     WAL for crash recovery
│       │
│       ├── smp/                      # Symmetric Multiprocessing
│       │   ├── mod.rs                #   MADT parsing, SIPI, init
│       │   ├── trampoline.s          #   AP boot (16->32->64 bit)
│       │   └── per_core.rs           #   CoreInfo, AP entry
│       │
│       ├── net/                      # Network stack
│       │   ├── mod.rs                #   Packet dispatch + poll
│       │   ├── eth.rs                #   Ethernet
│       │   ├── arp.rs                #   ARP
│       │   ├── ipv4.rs               #   IPv4
│       │   ├── icmp.rs               #   ICMP (ping, traceroute)
│       │   ├── udp.rs                #   UDP
│       │   ├── tcp.rs                #   TCP
│       │   ├── dns.rs                #   DNS resolver
│       │   ├── dhcp.rs               #   DHCP client
│       │   └── ntp.rs                #   NTP time sync
│       │
│       ├── gpu/                      # GPU subsystem
│       │   ├── mod.rs                #   Backend abstraction (GOP/Xe)
│       │   ├── intel_xe.rs           #   Intel Xe Gen 12.2 display
│       │   └── gop.rs                #   UEFI GOP fallback
│       │
│       ├── gui/                      # GUI subsystem
│       │   ├── mod.rs                #   Module index
│       │   ├── login.rs              #   Graphical login screen
│       │   ├── background.rs         #   Procedural aurora
│       │   ├── font.rs               #   Spleen bitmap fonts
│       │   ├── render.rs             #   Rounded rects, gradients
│       │   ├── theme.rs              #   Color themes
│       │   └── layers.rs             #   Layer compositor
│       │
│       ├── shade/                    # Shade compositor (tiling WM)
│       │   ├── mod.rs                #   Init, render, mouse, tick
│       │   ├── compositor.rs         #   Dwindle tiling, swap anim
│       │   ├── window.rs             #   Window metadata
│       │   ├── bar.rs                #   Shadebar (workspaces, clock)
│       │   ├── terminal.rs           #   Per-window terminal buffers
│       │   ├── input.rs              #   Keybindings
│       │   └── cursor.rs             #   Software mouse cursor
│       │
│       ├── intent/                   # Intent loop
│       │   ├── mod.rs                #   Dispatch, CWD, tab-completion, key injection routing
│       │   ├── fs.rs                 #   store, fetch, cat, grep, list
│       │   ├── net.rs                #   ping, traceroute, resolve
│       │   ├── http.rs               #   HTTP/HTTPS GET (async on worker core)
│       │   ├── wasm.rs               #   run (interactive), run_background (debug.wasm)
│       │   ├── system.rs             #   status, help, halt, config
│       │   ├── install.rs            #   install <mod> — GH download + ECDSA verify
│       │   ├── update.rs             #   OTA kernel update — GH download + ECDSA + ESP write
│       │   └── auth.rs               #   lock, passwd
│       │
│       ├── input.rs                   # KeyEvent abstraction (KeyCode, Modifiers)
│       ├── wasm.rs                   # WASM runtime + host functions (stream, tcp, key_inject)
│       └── setup.rs                  # First-boot setup wizard
│
├── tools/wasm/debug/                 # Reverse debug shell (WASM module, ~1.6KB)
│   └── src/
│       ├── lib.rs                    #   Relay loop: stream ↔ TCP ↔ key inject
│       └── host.rs                   #   Host function bindings
├── tools/wasm/wifi/                  # WiFi driver (WASM module)
│   └── src/
│       ├── lib.rs                    #   Entry point, init + FW download sequence
│       ├── host.rs                   #   Host function bindings (PCI, MMIO, DMA)
│       ├── regs.rs                   #   RTL8852BE register definitions
│       └── fw.rs                     #   MFW container parser, firmware upload
```

---

## Build & Run

```bash
# Prerequisites
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
sudo pacman -S grub xorriso mtools qemu-system-x86   # Arch
# or: sudo apt install grub-pc-bin xorriso mtools qemu-system-x86

# Build + Run
./build.sh qemu          # Serial console in terminal (4 cores)
./build.sh qemu-gui      # Serial + VGA window
./build.sh debug         # With GDB stub on :1234
./build.sh build         # Compile only
./build.sh release       # Build + sign kernel (ECDSA P-384) + generate manifest
```

### Release + OTA Flow (bare metal testing)

Each feature lands on the NUC through this loop:

1. **Bump** `kernel/Cargo.toml` version (patch for fix, minor for feature).
2. **Build WASM modules** if changed:
   `cd tools/wasm/<name> && cargo build --release --target wasm32-unknown-unknown`
   then copy `target/wasm32-unknown-unknown/release/<name>.wasm` to
   `release/modules/<name>.wasm` and update `release/modules/<name>.version`.
3. **`./build.sh release`** — compiles the kernel, signs `kernel.bin` +
   all `release/modules/*.wasm` with `update.key` (ECDSA P-384), regenerates
   `release/manifest` and `release/modules/manifest` (sha384 + size per entry).
4. **Commit + push** — all release artifacts go to `main` so
   `raw.githubusercontent.com/fnopeek/nopeekOS/main/release/` serves them.
5. **On the NUC:**
   - `update` — `kernel/src/intent/update.rs`: fetches `release/manifest`, verifies
     ECDSA signature over the new kernel, writes to the ESP FAT32 partition via
     `storage/fat32.rs`. Reboots into the new kernel.
   - `install <name>` — `kernel/src/intent/install.rs`: fetches
     `release/modules/manifest`, matches `<name>`, downloads `.wasm` + `.sig`,
     verifies sha384 + ECDSA, stores under `sys/wasm/<name>` in npkFS.
   - `run <name>` loads and executes the module in a sandboxed WASM worker.

Both verification paths share the embedded root key in `kernel/src/crypto/update_key.rs`
and reject any artifact whose signature doesn't match.

### First Boot (Intel N100 NUC)

```
[npk] AI-native Operating System v0.49.0
[npk] Multiboot2: verified
[npk] Interrupts enabled.
[npk] TSC: 691 MHz
[npk] Physical memory: 15892 MB free (16 GB detected)
[npk] Kernel footprint: 6340 KB
[npk] Heap: 64 MB (grows on demand, max 2048 MB)
[npk] Paging: 64 GB identity-mapped, NX enabled
[npk] APIC timer: 100Hz (base=0xfee00000)
[npk] smp: 4 cores detected (BSP + 3 APs)
[npk] smp: 3/3 APs online
[npk] PCI: 8 devices
[npk] nvme: KINGSTON SNV2S500G, TRIM=yes, 465 GB
[npk] xhci: USB keyboard (HID boot protocol)
[npk] xhci: USB mouse (HID boot protocol)
[npk] Intel Xe GPU: ADL-N (device 46d0), 4K@60Hz HDMI 2.0
[npk] Framebuffer: 3840x2160 @ BAR2+GGTT (32bpp, scale=2)
[npk] BCS: blitter engine ready
[npk] Intel I226-V: link UP, MAC 48:21:0b:...
[npk] WiFi: RTL8852BE (10ec:b852) probed, BAR2 MMIO assigned
[npk] DHCP: configured 192.168.1.100
[npk] npkfs: mounted (gen=42, 15 objects)
[npk] CSPRNG: ChaCha20 (RDRAND-seeded)
[npk] WASM runtime: wasmi v1.0 (fuel-metered)

[npk] Welcome, Florian.
[npk] System ready. Express your intent.

~>
```

---

## Security Architecture

1. **Deny by Default** -- Without a capability token, nothing happens
2. **Encryption at Rest** -- All data encrypted with ChaCha20-Poly1305 AEAD
3. **Passphrase Identity** -- No users, no accounts. Your passphrase IS your identity
4. **256-bit Tokens** -- Post-quantum safe (Grover-resistant), ChaCha20 CSPRNG
5. **Least Privilege** -- WASM modules get only what they need (READ+EXECUTE, no WRITE)
6. **Temporal Scoping** -- Module capabilities expire after 60 seconds
7. **Audit Everything** -- Every token operation logged
8. **Formal Boundaries** -- WASM sandbox is the trust boundary
9. **No Ambient Authority** -- No root, no sudo, no privilege elevation
10. **Fuel Metering** -- 10M instruction budget per module prevents DoS
11. **TLS 1.3** -- All network communication encrypted (3 cipher suites, X25519 + P-384)
12. **Signed OTA Updates** -- ECDSA P-384 signed kernel + modules, SHA-384 integrity check

> **Future: Code Signing Key Hierarchy**
>
> Currently all artifacts (kernel, WASM modules) are signed with a single ECDSA P-384 key.
> When third-party modules become possible, this needs to evolve:
> - Separate keys for kernel vs. modules (compromise isolation)
> - Per-publisher keys for third-party WASM apps
> - Root key (offline) signs sub-keys; sub-keys sign artifacts
> - Sub-key revocation mechanism (capability-based, temporal)

---

## What nopeekOS Is NOT

- **Not a Linux clone** -- no systemd, no ext4, no procfs
- **Not POSIX** -- no fork(), no exec(), no pipes
- **Not a unikernel** -- multi-intent, not single-purpose
- **Not a container runtime** -- WASM modules are lighter than containers
- **Not an academic experiment** -- every phase produces working code

---

## Vision

```
Today:    Human installs app -> configures -> operates -> debugs
Tomorrow: Human expresses intent -> system generates -> executes -> delivers
```

nopeekOS is the attempt to build "tomorrow".
Without compromise to the past.
From Luzern.

---

## License

GPL-3.0 -- see [LICENSE](LICENSE)

## Author

nopeek -- [nopeek.ch](https://nopeek.ch)
