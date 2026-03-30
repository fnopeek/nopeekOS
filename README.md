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
npk> store config version=1.0        # Store object (BLAKE3-hashed, COW B-tree)
npk> fetch config                    # Retrieve with integrity check
npk> list                            # All objects with hashes
npk> run hello                       # Execute WASM from npkFS (sandboxed, cap-gated)
npk> run fib 20                      # Compute fibonacci(20) = 6765 in WASM sandbox
npk> ping 10.0.2.2                   # ICMP ping
npk> traceroute 8.8.8.8              # Network path tracing
npk> resolve google.com              # DNS resolution
npk> http example.com /              # HTTP GET (full TCP/IP stack)
npk> http example.com / > mypage     # Fetch and store in npkFS
npk> disk read 0                     # Raw sector hex dump
```

Every operation is capability-gated. No ambient authority. No root. No sudo.

---

## Architecture

```
 ┌──────────────────────────────────────────────────────────┐
 │  Intent Loop                                             │
 │  Express intention, not instructions.                    │
 ├──────────────────────────────────────────────────────────┤
 │  WASM Runtime (wasmi, fuel-metered)                      │
 │  Sandboxed modules loaded from npkFS.                    │
 │  Capability-gated host functions. No ambient authority.  │
 ├──────────────────────────────────────────────────────────┤
 │  npkFS                          │  Network Stack         │
 │  COW B-tree, BLAKE3 hashing     │  Ethernet, ARP, IPv4   │
 │  Rotating superblock (8 slots)  │  ICMP, UDP, TCP        │
 │  LRU cache, WAL journal         │  DNS, DHCP, NTP        │
 │  Batch TRIM for SSD             │  HTTP client            │
 ├──────────────────────────────────────────────────────────┤
 │  Capability Vault           │  CSPRNG (ChaCha20)         │
 │  256-bit tokens, deny-all   │  RDRAND-seeded when avail  │
 │  Temporal scoping, audit    │  Forward secrecy re-keying │
 ├──────────────────────────────────────────────────────────┤
 │  Drivers                                                 │
 │  PCI bus scanner, virtio-blk (TRIM), virtio-net          │
 ├──────────────────────────────────────────────────────────┤
 │  Kernel Core (Rust, no_std, ~5500 lines)                 │
 │  Memory Manager, Heap, Paging+NX, IDT+PIC, Serial, VGA  │
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
- [x] Virtual Memory (4-level paging, NX bit, identity-mapped first 1GB)
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
- [x] npkFS: Copy-on-Write B-tree (19 entries/leaf, 56 keys/internal node)
- [x] BLAKE3 content hashing (integrity verification on every fetch)
- [x] 8-slot rotating superblock (SSD wear leveling)
- [x] LRU block cache (64 slots, 256KB, write coalescing)
- [x] Circular WAL journal (crash recovery for deferred frees)
- [x] Batch TRIM/DISCARD (merged adjacent ranges)
- [x] Extent-based allocation
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

### Phase 6 -- Crypto & Identity (in progress)

- [x] ChaCha20-Poly1305 AEAD encryption (RFC 8439)
- [x] Passphrase-based identity (BLAKE3-KDF → 256-bit master key)
- [x] Encryption at rest (all npkFS objects encrypted by default)
- [ ] Post-quantum crypto: ML-KEM (Kyber) + ML-DSA (Dilithium), hybrid with X25519/Ed25519
- [ ] TLS for secure connections
- [ ] Hardware manifest collector (PCI + CPUID + ACPI probe)
- [ ] WASM driver model (drivers as sandboxed modules, capability-gated I/O)
- [ ] Driver mirror (fetch matching WASM drivers on demand based on HW manifest)

### Phase 7 -- Human View

- [ ] Framebuffer driver (VESA/GOP)
- [ ] Tiling window manager (Hyprland-inspired)
- [ ] Keyboard + mouse input (PS/2, virtio-input)
- [ ] Web rendering engine (long-term)

### Phase 8 -- AI Integration

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
| Crypto (planned) | Hybrid classical + post-quantum | Future-proof |
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
│       ├── wasm.rs              # WASM runtime + host functions
│       ├── intent.rs            # Intent loop + all intents
│       ├── vga.rs               # VGA text mode
│       └── store.rs             # Content store (legacy)
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
[npk] Physical memory: 123 MB free
[npk] Heap: 1024 KB
[npk] Paging: 512 x 2MB, NX enabled
[npk] PCI: 5 devices
[npk] virtio-blk: 32768 sectors (16 MB), TRIM=yes
[npk] virtio-net: MAC 52:54:00:12:34:56
[npk] DHCP: configured 10.0.2.15
[npk] npkfs: mounted (gen=1, 0 objects, 3830 free blocks)
[npk] WASM runtime: wasmi v1.0 (fuel-metered)
[npk] Bootstrap: stored 4 WASM modules
[npk] CSPRNG: ChaCha20 (RDRAND-seeded)
[npk] Vault online.

[npk] System ready. Express your intent.

npk>
```

---

## Security Architecture

1. **Deny by Default** -- Without a capability token, nothing happens
2. **Least Privilege** -- WASM modules get only what they need (READ+EXECUTE, no WRITE)
3. **Temporal Scoping** -- Module capabilities expire after 60 seconds
4. **Audit Everything** -- Every token operation logged
5. **Formal Boundaries** -- WASM sandbox is the trust boundary
6. **No Ambient Authority** -- No root, no sudo, no privilege elevation
7. **Fuel Metering** -- 10M instruction budget per module prevents DoS
8. **CSPRNG** -- ChaCha20 (RFC 7539) for all token generation, not predictable PRNG

Attack surface: ~5500 lines of Rust in the trust boundary,
vs 30M+ (Linux) or 50M+ (Windows). Factor 5000x less code.

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
