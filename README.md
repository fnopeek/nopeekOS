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
npk> uninstall wallpaper               # Remove module
npk> modules                          # List installed modules
npk> wallpaper demo                    # Generate 3 demo wallpapers + auto-theme
npk> wallpaper set ocean              # Set wallpaper (extracts theme colors)
npk> wallpaper random                  # Random wallpaper from collection
npk> wallpaper clear                   # Revert to aurora background
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
- [ ] WiFi driver: MAC init, RF calibration, scan, association, data path

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

### Phase 10 -- Widget API & GUI Apps

Reusable UI components for WASM applications. Every widget is a host function —
apps describe layout, the compositor renders. No per-app framebuffers.

- [ ] `npk_widget_list(items, selected)` — scrollable list (file manager, settings)
- [ ] `npk_widget_input(prompt, buf)` — text input field with cursor
- [ ] `npk_widget_select(options, selected)` — dropdown / choice selector
- [ ] `npk_widget_text(content, scroll)` — scrollable text view
- [ ] `npk_widget_progress(value, max)` — progress bar
- [ ] `npk_widget_layout(rows, cols)` — grid layout for combining widgets
- [ ] Keyboard navigation (Tab between widgets, Enter to select)
- [ ] Mouse interaction (click on widget, scroll list)
- [ ] Theming (widgets inherit shade theme colors)
- [ ] File manager app (Thunar-inspired: tree view, file list, preview)
- [ ] Settings app (keyboard layout, shade config, theme)

Design principle: widgets are **data-driven** — the app passes data (list items,
current selection), the host renders. The app never touches pixels directly.
This allows the compositor to handle focus, theming, scaling, and accessibility.

### Phase 11 -- AI Integration

- [ ] External AI service via network
- [ ] Intent resolution through LLM
- [ ] Runtime WASM generation (AI writes modules)
- [ ] Semantic search in content store

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
│       │   ├── mod.rs                #   Dispatch, CWD, tab-completion
│       │   ├── fs.rs                 #   store, fetch, cat, grep, list
│       │   ├── net.rs                #   ping, traceroute, resolve
│       │   ├── http.rs               #   HTTP/HTTPS GET
│       │   ├── wasm.rs               #   run, bootstrap
│       │   ├── system.rs             #   status, help, halt, config
│       │   ├── update.rs             #   OTA update
│       │   └── auth.rs               #   lock, passwd
│       │
│       ├── input.rs                   # KeyEvent abstraction (KeyCode, Modifiers)
│       ├── wasm.rs                   # WASM runtime + host functions
│       └── setup.rs                  # First-boot setup wizard
│
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

### First Boot (Intel N100 NUC)

```
[npk] AI-native Operating System v0.46.1
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
