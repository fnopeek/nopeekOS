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
npk> status                          # Full system overview
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
npk> lock                            # Lock system (clear keys)
npk> passwd                          # Change passphrase
npk> disk read 0                     # Raw sector hex dump
```

Every operation is capability-gated. No ambient authority. No root. No sudo.
All data encrypted at rest. Passphrase-based identity — no users, no accounts.

---

## Architecture

```
 ┌──────────────────────────────────────────────────────────┐
 │  Intent Loop                                             │
 │  Express intention, not instructions.                    │
 ├──────────────────────────────────────────────────────────┤
 │  WASM Runtime (wasmi v1.0, fuel-metered)                  │
 │  Sandboxed modules loaded from npkFS.                    │
 │  Capability-gated host functions. No ambient authority.  │
 ├──────────────────────────────────────────────────────────┤
 │  npkFS                          │  Network Stack         │
 │  COW B-tree, BLAKE3 hashing     │  Ethernet, ARP, IPv4   │
 │  Rotating superblock (8 slots)  │  ICMP, UDP, TCP        │
 │  LRU cache, WAL journal         │  DNS, DHCP, NTP        │
 │  Batch TRIM for SSD             │  HTTP client            │
 ├──────────────────────────────────────────────────────────┤
 │  Capability Vault           │  Crypto Engine             │
 │  256-bit tokens, deny-all   │  ChaCha20-Poly1305 AEAD   │
 │  Passphrase identity        │  AES-128/256-GCM (TLS)    │
 │  Temporal scoping, audit    │  TLS 1.3: X25519 + P-384  │
 ├──────────────────────────────────────────────────────────┤
 │  Drivers                                                 │
 │  virtio-blk, virtio-net, NVMe, Intel I226-V, xHCI USB    │
 ├──────────────────────────────────────────────────────────┤
 │  Kernel Core (Rust, no_std, ~17500 lines)                │
 │  64GB Paging, Heap, IDT+PIC, ACPI, Framebuffer, Serial  │
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
- [x] Passphrase-based identity (BLAKE3-KDF → 256-bit master key)
- [x] Encryption at rest (all npkFS objects encrypted by default)
- [ ] Post-quantum crypto: ML-KEM (Kyber) + ML-DSA (Dilithium), hybrid with X25519/Ed25519
- [x] TLS 1.3 (RFC 8446, X25519 + ECDH P-384 key exchange)
- [x] TLS 1.3: AES-128-GCM-SHA256 + AES-256-GCM-SHA384 cipher suites (via RustCrypto aes-gcm crate)
- [x] HTTPS client (`https <host> [path]`)
- [x] X.509 certificate chain validation
- [x] Embedded root CAs (ISRG Root X1, DigiCert G2, AAA, GTS Root R1)
- [x] SHA-256, HMAC-SHA256, HKDF, RSA PKCS#1 v1.5 verify

### Phase 7 -- Bare Metal (target: Intel N100 NUC)

- [x] xHCI USB keyboard driver (BIOS handoff, HID boot protocol, disconnect detection)
- [x] PS/2 keyboard (extended scancodes, arrow keys, lock-free ringbuffer)
- [x] Intel I226-V Ethernet driver (igc, MMIO, RX/TX advanced descriptors, PCIe bridge)
- [x] Framebuffer driver (UEFI GOP, shadow buffer, 32-bit pixel write)
- [x] TSC timer fallback (CPUID 0x15 calibration, replaces PIT on UEFI-only systems)
- [x] NVMe driver (PCIe BAR0 MMIO, Admin/IO queues, Read/Write/TRIM)
- [x] 64GB RAM support (1GB huge pages, dynamic memory detection)
- [x] Tick-based USB timeouts (CPU-speed independent)
- [x] ACPI power-off (RSDP, FADT, DSDT \_S5 parsing)
- [x] Gateway routing, serial detection, dual xHCI controllers
- [ ] npk-shell: TLS-encrypted remote intent loop (TCP listener, passphrase auth)
- [ ] USB mouse (HID boot protocol, same xHCI infrastructure)
- [ ] WASM driver model (drivers as sandboxed modules, capability-gated I/O)

### Phase 8 -- Human View (next)

- [ ] Tiling window manager (Hyprland-inspired, framebuffer compositing)
- [ ] USB mouse input (xHCI HID, multi-device support)
- [ ] KeyEvent abstraction (Unicode chars, arrow keys, modifiers)
- [ ] GPU acceleration (Intel UHD, hardware-accelerated compositing)
- [ ] Web rendering engine (long-term)

### Phase 9 -- AI Integration

- [ ] External AI service via network
- [ ] Intent resolution through LLM
- [ ] Runtime WASM generation (AI writes modules)
- [ ] Semantic search in content store

---

## Technical Decisions

| Area | Choice | Rationale |
|------|--------|-----------|
| Language | Rust (no_std, nightly) | Memory safety without GC |
| Boot | Multiboot2 | QEMU/GRUB compatible |
| Target | x86_64 | Later aarch64 |
| WASM | wasmi v1.0 | no_std, fuel metering |
| Filesystem | npkFS | COW, BLAKE3, SSD-native |
| Hashing | BLAKE3 | Fast, secure, streaming |
| CSPRNG | ChaCha20 (RFC 7539) | RDRAND seed, forward secrecy |
| AEAD | ChaCha20-Poly1305 (RFC 8439) | Encryption at rest + TLS |
| AES-GCM | aes-gcm crate (RustCrypto) | TLS 1.3 (128/256-bit) |
| TLS | 1.3 (RFC 8446) | X25519 + P-384, 3 cipher suites |
| Identity | Passphrase → BLAKE3-KDF | No users, no accounts |
| Key Exchange | X25519 + ECDH P-384 | Ephemeral, per-connection |
| Certificates | X.509, 4 embedded root CAs | ISRG X1, DigiCert G2, AAA, GTS R1 |
| Crypto libs | sha2, hmac, hkdf, aes-gcm, p384 | RustCrypto, audited, no_std |
| TCP defaults | No Nagle, 40ms ACK, 3 retries | Optimized for request/response |
| Drivers (planned) | WASM modules | Sandboxed, on-demand from mirror |

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
├── build.sh                     # Build + QEMU/VirtualBox launch
├── kernel/
│   ├── Cargo.toml
│   ├── linker.ld                # Memory layout (256KB stack, heap)
│   └── src/
│       ├── boot.s               # Multiboot2 -> Long Mode
│       ├── main.rs              # Kernel entry, boot sequence
│       ├── serial.rs            # Serial console + port I/O
│       ├── csprng.rs            # ChaCha20 CSPRNG (RFC 7539)
│       ├── capability.rs        # Capability Vault
│       ├── audit.rs             # Audit log ring buffer
│       ├── memory.rs            # Physical frame allocator
│       ├── heap.rs              # Linked-list heap allocator
│       ├── paging.rs            # Virtual memory manager
│       ├── interrupts.rs        # IDT + PIC
│       ├── pci.rs               # PCI bus scanner
│       ├── virtio_blk.rs        # Block device driver (TRIM)
│       ├── virtio_net.rs        # Network device driver
│       ├── nvme.rs              # NVMe driver (PCIe, TRIM)
│       ├── blkdev.rs            # Block device abstraction
│       ├── xhci.rs              # xHCI USB driver (keyboard)
│       ├── keyboard.rs          # PS/2 keyboard (extended scancodes)
│       ├── framebuffer.rs       # UEFI GOP framebuffer
│       ├── acpi.rs              # ACPI power management
│       ├── npkfs/               # Filesystem
│       │   ├── mod.rs           # Public API: mkfs, mount, store, fetch
│       │   ├── types.rs         # On-disk format definitions
│       │   ├── cache.rs         # LRU block cache
│       │   ├── bitmap.rs        # Block allocation + TRIM
│       │   ├── superblock.rs    # Rotating superblock ring
│       │   ├── journal.rs       # WAL for crash recovery
│       │   └── btree.rs        # COW B-tree
│       ├── net/                 # Network stack
│       │   ├── mod.rs           # Packet dispatch + poll
│       │   ├── eth.rs           # Ethernet
│       │   ├── arp.rs           # ARP
│       │   ├── ipv4.rs          # IPv4
│       │   ├── icmp.rs          # ICMP (ping, traceroute)
│       │   ├── udp.rs           # UDP
│       │   ├── tcp.rs           # TCP
│       │   ├── dns.rs           # DNS resolver
│       │   ├── dhcp.rs          # DHCP client
│       │   └── ntp.rs           # NTP time sync
│       ├── crypto.rs            # ChaCha20-Poly1305 AEAD, KDF
│       ├── tls/                 # TLS 1.3 stack
│       │   ├── mod.rs           # TLS handshake + record layer
│       │   ├── sha256.rs        # SHA-256 + SHA-384 (via sha2 crate)
│       │   ├── hmac.rs          # HMAC/HKDF SHA-256 + SHA-384
│       │   ├── x25519.rs        # Curve25519 ECDH
│       │   ├── rsa.rs           # RSA PKCS#1 v1.5 verify
│       │   ├── asn1.rs          # ASN.1 DER parser
│       │   ├── x509.rs          # X.509 certificate parser
│       │   └── certstore.rs     # Root CAs + chain validation
│       ├── wasm.rs              # WASM runtime + host functions
│       ├── intent/              # Intent loop
│       │   ├── mod.rs           # Loop, dispatch, CWD, tab-completion
│       │   ├── fs.rs            # store, fetch, cat, grep, head, wc, hexdump, list
│       │   ├── net.rs           # ping, traceroute, netstat, resolve
│       │   ├── http.rs          # HTTP/HTTPS GET (TLS 1.3)
│       │   ├── wasm.rs          # run, add, multiply, bootstrap
│       │   ├── system.rs        # status, time, help, caps, audit, halt, config
│       │   └── auth.rs          # lock, passwd
│       ├── vga.rs               # VGA text mode
│       ├── config.rs            # Runtime configuration
│       └── gpt.rs               # GPT partition detection
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
./build.sh qemu          # Serial console in terminal
./build.sh qemu-gui      # Serial + VGA window
./build.sh debug         # With GDB stub on :1234
./build.sh build         # Compile only
```

### First Boot

```
[npk] AI-native Operating System v0.1.0
[npk] Multiboot2: verified
[npk] Interrupts enabled.
[npk] Physical memory: 15892 MB free (16 GB detected)
[npk] Heap: 65536 KB
[npk] Paging: 64 GB identity-mapped, NX enabled
[npk] PCI: 8 devices
[npk] nvme: KINGSTON SNV2S500G (SN: ...), TRIM=yes
[npk] nvme: 465 GB (976773168 sectors)
[npk] xhci: device on port 2
[npk] xhci: USB keyboard (HID boot protocol)
[npk] Framebuffer: 1920x1080 @ 0xfd000000 (32bpp)
[npk] Intel I226-V: link UP, MAC 48:21:0b:...
[npk] DHCP: configured 192.168.1.100
[npk] npkfs: mounted (gen=42, 15 objects, 120000 free blocks)
[npk] CSPRNG: ChaCha20 (RDRAND-seeded)
[npk] WASM runtime: wasmi v1.0 (fuel-metered)

[npk] ══════════════════════════════════
[npk]  Welcome, Florian.
[npk]  System ready. Express your intent.
[npk] ══════════════════════════════════

Florian@npk ~>
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
