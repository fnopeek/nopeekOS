# Microkernel Driver Refactor

**Goal:** All hardware drivers exit the kernel and run as WASM drivers via the
existing `npk_pci_*/mmio_*/dma_*` driver-ABI. The kernel keeps only HW-agnostic
primitives (PCI enumeration, MSI/IRQ dispatch, MM, capabilities, crypto, WASM
runtime). Demo-VM-Targets (QEMU/VBox/VMware) keep working flawlessly throughout
the refactor — NUC is proof, not default.

**Not goals.** Not a POSIX-driver-framework. Not online-built kernels. Not "every
WASM driver replaces a Linux module 1:1." The constraint is the trust boundary
and HW-pluggability, not Linux-compat.

> **Status:** spec phase, kein Code. Living document. Each open-question block
> is `open` / `decided <date>` plus reasoning.

---

## Driver-layering model

```
Layer 1 — HW-agnostic kernel primitives (stays)
    PCI bus, MSI/MSI-X dispatch, IRQ routing, MM, paging, capabilities,
    crypto AEAD, scheduler, WASM runtime, blkdev/netdev abstractions.

Layer 2 — Bootstrap WASM drivers (embedded in kernel binary)
    Demo-set + UEFI-generic. ~150 KB total. Always present, even on first boot
    before npkFS mounts. Solves the boot chicken-and-egg.

Layer 3 — Installed WASM drivers (npkFS-resident)
    HW-specific (Intel Xe, RTL8852BE WiFi, vendor NICs). Loaded via PCI
    auto-match against driver manifests. Optional, post-boot, OTA-updateable.
```

The bootstrap set in Layer 2 is the answer to "how does a WASM-driver-only
system load drivers off a disk that requires drivers to read."

---

## Inventory

### Stays in kernel (HW-agnostic primitives)

| File | LoC | Role |
|---|---|---|
| `drivers/pci.rs` | 293 | Bus enumerator — primitive behind `npk_pci_*` |
| `drivers/serial.rs` | 239 | Primary I/O, debug console, boot log |
| `drivers/acpi.rs` | 293 | Bring-up + reboot |
| `drivers/rtc.rs` | 117 | CMOS clock |
| `drivers/blkdev.rs` | 247 | Block-Layer-Abstraktion (driver ↔ npkFS) |
| `drivers/netdev.rs` | 152 | Net-Layer-Abstraktion (driver ↔ TCP/IP-Stack) |
| `mm/`, `interrupts.rs`, `security/`, `crypto/aead.rs`, `smp/`, `process.rs`, `wasm.rs`, `setup.rs`, `intent/`, `shade/` (separate Compositor-Frage) | — | Microkernel core |

### Migrates to WASM driver — Bootstrap-Set (embedded)

| File | LoC | Demo-target | Order |
|---|---|---|---|
| `drivers/virtio_blk.rs` | 450 | QEMU + VBox-virtio + VMware-paravirt | 1 (ABI-validation) |
| `drivers/keyboard.rs` | 370 | VBox/VMware default, legacy NUC | 2 (small, IRQ1 path) |
| `drivers/intel_nic.rs` | 679 | VBox-default, NUC i225/i226, OEM-Pcs | 3 (MSI-path validation) |
| `drivers/virtio_net.rs` | 463 | QEMU + VBox-virtio + VMware-paravirt | 4 |

### Migrates to WASM driver — HW-Extension-Set (npkFS)

| File | LoC | Notes | Order |
|---|---|---|---|
| `drivers/nvme.rs` | 1300 | Performance-critical — needs ABI perf plan first | 5 |
| `drivers/xhci.rs` | 1880 | Large, no demo-blocker (PS/2 covers VM keyboards) | 6 |
| `drivers/framebuffer.rs` | 836 | Compositor-Architektur-Frage zuerst | 7 |
| `gpu/intel_xe.rs` + `gpu/ggtt_*` | 2746 | NUC-only, GPU host-fn surface | 8 (endboss) |

### Cosmetic cleanup — `intent/system.rs:845–873`

Hardcoded `(vendor, device) → human name` for Intel WiFi/Ethernet/UHD Graphics.
Move to a static table in a system-info app or generate from PCI auto-match
manifests when those land. Low priority, no architecture impact.

---

## Bootstrap-WASM-Driver-Set

The set that lives **embedded in the kernel binary**, mandatory for first-boot.
Built into `kernel/src/install_data/assets/` analog to existing bootstrap apps
(`hello`, `fib`, …).

| Driver | WASM size est. | Covers |
|---|---|---|
| `virtio-blk` | ~10 KB | QEMU + VBox virtio + VMware paravirt |
| `virtio-net` | ~15 KB | QEMU + VBox virtio + VMware paravirt |
| `intel-nic` (e1000/i225) | ~25 KB | VBox-default, many OEMs, NUC |
| `ps2-keyboard` | ~5 KB | VBox + VMware + legacy |
| `xhci` (USB-keyboard subset) | ~50 KB | UEFI-standard, modern NUC ohne PS/2 |
| `gop-framebuffer` | ~10 KB | UEFI-standard |
| `nvme` | ~30 KB | QEMU NVMe option, modern NUC |
| **Total** | **~150 KB** | **>95 % real boot targets** |

Kernel-binary growth: 0,15 MB on a ~3 MB kernel — vernachlässigbar.

**First-boot flow:**

```
1. Kernel starts: serial, ACPI, IDT, MM, scheduler.
2. PCI bus enumerated.
3. For each device: match (vendor, device) → embedded bootstrap-WASM.
4. virtio-blk loads → blkdev exposes /dev/vda → npkFS mounts.
5. Rest of system starts from npkFS (Shade, apps, additional WASM drivers
   via `install <driver>` if HW-specific).
```

No internet needed. No two-stage. No online build.

**Update path:** OTA `update` rebuilds the kernel binary including the latest
bootstrap-set. Same ECDSA-P-384 chain, no new mechanism.

---

## Driver-ABI gaps

These must be closed before drivers can move out.

### `decided 2026-04-30` — `npk_irq_subscribe(vector_or_msi, cap) → IrqHandle`

Both legacy IRQ (PIC/APIC) and MSI/MSI-X go through this. Driver registers a
vector or MSI-vector-table-entry; kernel routes the hardware IRQ to a
WASM-event-pump owned by the driver task. WASM driver pulls events via
`npk_irq_poll` (analog `npk_input_poll`).

**Why decided:** the alternative (per-IRQ-class API) fragments the surface and
complicates auto-match. Single subscribe-poll surface is symmetric to the
existing input/event-pump pattern.

### `open` — Framebuffer ownership

When `gpu/intel_xe.rs` and `framebuffer.rs` move out, who owns the pixel
buffer that Shade renders into?

- **A.** Shade stays kernel, calls WASM-GPU-driver per-frame via host-fn
  (synchronous, slow at 4K@60Hz).
- **B.** Shared framebuffer in DMA region; WASM driver `npk_present()` blits
  when ready (decoupled, more complex).
- **C.** Shade itself moves out as WASM (largest scope).

**Default-if-stuck:** **B.** Decoupling matches the rest of the architecture;
synchronous-per-frame call (A) is unacceptable for the 4K@60Hz target. C is
out of scope for the driver refactor — it's a separate compositor decision.

### `open` — NVMe performance plan

1 MB read at 411 MB/s, 1 MB dedup-write at 759 MB/s — hard-fought wins. Each
DMA submit through the WASM boundary is overhead.

- **A.** Batch-submit host-fn (`npk_nvme_submit_batch`) — amortize
  boundary cost.
- **B.** Map SQ/CQ pages directly into WASM linear-memory — driver writes
  commands without a host-fn per submit.
- **C.** Defer NVMe migration entirely — keep in kernel. Pure-Microkernel
  sacrifice for performance.

**Default-if-stuck:** **B** with strict capability scoping (one ring, one
driver). Validate at virtio-blk milestone first; if WASM-boundary cost > 5 %
throughput on the bench, reconsider C for NVMe specifically. Bench gate is
binding before NVMe migration starts.

### `open` — PCI auto-match manifest format

WASM-driver-Manifest declares which PCI devices it claims:

```toml
[match]
pairs = [
  { vendor = 0x8086, device = 0x10D3 },  # 82574L
  { vendor = 0x8086, device = 0x15F3 },  # I225-V
]
```

Open: priorities (specific vs. generic match), conflict resolution (two
drivers claim same device), revocation (uninstalled driver releases match).

**Default-if-stuck:** strict (vendor, device) pairs, no wildcards,
first-loaded-wins, conflict = boot-error with manifest-list dump.

### `open` — DMA-coherent allocator surface for WASM drivers

NVMe + virtio-blk need contiguous, DMA-coherent buffers with stable phys-
addresses. Current `npk_dma_alloc` is enough or do we need lifetime+pinning
extensions for ring-style structures? Audit at virtio-blk milestone.

---

## virtio dual role — critical

virtio-blk/virtio-net exist **twice** in the system after Phase 12:

- **Guest-side driver** (in QEMU/VBox/VMware-host running nopeekOS):
  bootstrap WASM-driver, talks to the host's virtio-backend.
- **Host-side backend** (in nopeekOS-host running a Linux MicroVM):
  kernel-internal, feeds the Linux guest's virtio-driver.

Same wire protocol, opposite directions. **Different code paths.** The
bootstrap WASM-driver must not be confused with the MicroVM virtio-backend —
they share specs, not source.

**Sequencing implication:** virtio-blk WASM-driver migration **must happen
before** Phase 12.2 (MicroVM virtio-blk backend). If we build the host-backend
first while the guest-driver is still kernel-resident, we end up duplicating
virtio code with subtle drift between the two.

---

## Migration order

Strict order — each step validates the next.

| # | Driver | Why this slot | Risk |
|---|---|---|---|
| 0 | Driver-ABI gaps closed (MSI, auto-match, IRQ-subscribe, DMA audit) | Foundation | high — gets the ABI right |
| 1 | virtio-blk (Demo) | Smallest paravirt driver, tests MSI-X + DMA + ring-protocol | low |
| 2 | ps2-keyboard (Demo) | Smallest driver, tests legacy IRQ-subscribe path | low |
| 3 | intel-nic (Demo) | Tests sustained MSI throughput, mid-size driver | medium |
| 4 | virtio-net (Demo) | Mirror of #1, validates ABI consistency | low |
| — | **Phase 12.2-12.6 MicroVM** runs from here, virtio-host-backends in kernel against now-stable WASM-guest-ABI | — | — |
| 5 | NVMe (HW-extension) | Performance plan validated; bench gate ≥95 % of pre-refactor MB/s | high — perf |
| 6 | xhci (HW-extension) | Large, late so not blocking | medium |
| 7 | framebuffer (HW-extension) | Needs Compositor-Architektur-Entscheidung (Open Q above) | medium |
| 8 | intel_xe (HW-extension) | Endboss; requires GPU host-fn surface | high |

---

## Test strategy — QEMU local + NUC parallel

Two test pipelines run in parallel during the refactor:

| Pipeline | Target | Use | Cycle time |
|---|---|---|---|
| **QEMU local** | Demo-Set drivers | Fast iteration on virtio-* and PS/2 — `./build.sh qemu` round-trip is seconds | seconds |
| **NUC physical** | HW-Extension-Set + integration | Real-HW validation via debug-console, validates intel-nic on i225/i226, GOP-FB on real UEFI, NVMe perf bench, intel_xe later | minutes |

Each refactor step (1–8) must pass on both pipelines for its target tier
before proceeding. Demo-Set steps (1–4) gate on QEMU first, NUC second; HW-
Extension-Set steps (5–8) gate on NUC first because that's where the relevant
HW lives.

**Parallel work:** while one driver is in QEMU iteration on the developer
machine, NUC-debug-console can run an integration build of a previous
milestone. The two pipelines are not blocking on each other.

---

## Sequencing relative to PHASE12_MICROVM.md

```
12.1 Hello Bash (PoC)            ← VMX/EPT/VCPU/virtio-console-backend bring-up
↓
MICROKERNEL_REFACTOR steps 0–4    ← ABI gaps + Demo-Set out
↓
12.2 virtio-blk + Profile-Image   ← MicroVM resumes against stable ABI
12.3 virtio-net + cap-filter
12.4 virtio-gpu cross-domain
12.5 Picker bridge
12.6 Firefox
↓
MICROKERNEL_REFACTOR steps 5–8    ← HW-extension-set out
```

Refactor adds ~2–3 weeks between 12.1 and 12.2. Bought back as a cleaner
virtio architecture and zero drift between guest WASM-driver and host kernel-
backend.

---

## Decided

### `decided 2026-04-30` — Bootstrap-WASM-Driver-Set embedded in kernel binary
Demo + UEFI-generic drivers (~150 KB total) embedded analog to
`install_data/assets/` pattern. **Why:** solves boot chicken-and-egg without
adding a new mechanism. OTA-update path stays one ECDSA-signed kernel-binary.

### `decided 2026-04-30` — Online-Kernel-Build verworfen
Just-in-time built kernels per-HW: rejected. **Why:** trust-loop
(build-server holds master key), single-point-of-failure (no offline
first-boot), reproducibility lost (per-user binaries), bootstrap-loop
(mini-kernel still needs network drivers it doesn't have).

### `decided 2026-04-30` — Demo-Set first, HW-extension-Set second
Migration order is strict: demo-VM-targets keep working at every refactor
step. **Why:** demo is the must-work sales-test; NUC is proof, not default.
ABI hardens at small + paravirt drivers, not at NUC-only intel_xe.

### `decided 2026-04-30` — Microkernel-Refactor between 12.1 and 12.2
PoC validates VMX bring-up; refactor closes ABI; MicroVM 12.2-12.6 builds
against stable ABI. **Why:** prevents virtio-code-drift between WASM-guest-
driver and kernel-host-backend.

### `decided 2026-04-30` — Stays in kernel: HW-agnostic primitives
PCI-bus, MSI-dispatch, IRQ, MM, scheduler, capabilities, crypto-AEAD,
WASM-runtime, blkdev/netdev abstractions, serial, ACPI, RTC. **Why:** these
are not "drivers" — they're the substrate every driver builds against.

### `decided 2026-04-30` — `npk_irq_subscribe` as unified IRQ surface
One host-fn handles legacy IRQ + MSI + MSI-X. **Why:** symmetric to existing
event-pump pattern, simpler auto-match, smaller ABI.

### `decided 2026-04-30` — Two parallel test pipelines (QEMU + NUC)
QEMU local for Demo-Set fast-iteration, NUC physical for HW-Extension-Set +
integration. **Why:** seconds-cycle for the bulk of the work, real-HW only
where needed. Doubles throughput during the refactor.

---

## Open questions

(see also: per-section open blocks above)

### `open` — Bootstrap-set distribution: embedded vs. separate-signed-files
Embedded (decided default): one signed kernel-binary, simplest update.
Alternative: separate `.sqfs` + sig in boot partition, kernel loader picks
them up. More flexible (driver-update without kernel-update), more complexity.

**Default-if-stuck:** embedded, decided above. Re-evaluate if driver-update
cadence diverges hard from kernel-update cadence (unlikely in Phase 12).

### `open` — Cross-driver dependencies (e.g. xhci → usb-keyboard)
xhci is a host-controller; usb-keyboard is a device-driver above it. Two-tier
WASM-driver hierarchy? Or xhci-WASM exposes both?

**Default-if-stuck:** flat (xhci-WASM owns USB-HID-keyboard internally for
the bootstrap case; full USB-stack-as-WASM-tree is post-Phase-12).

### `open` — Driver-WASM-Manifest extension to existing app-manifest
Today: app manifest = caps + entry. Driver-WASM-manifest needs `[match]`
block + IRQ requirements + DMA size. Extension to existing TOML or new file?

**Default-if-stuck:** extend app-manifest with optional `[driver]` section.
Wire-Version 0x02 of the manifest schema (analog Widget-ABI append-only).

### `open` — Test harness for WASM drivers
Today driver-tests run on real HW or QEMU. Testing a WASM-driver in isolation
needs a virtual PCI/IRQ harness. Build effort?

**Default-if-stuck:** defer until step 1 (virtio-blk migration) — first test
is real-VM via QEMU + NUC pipelines.

### `open` — Phase-Nummer for the refactor in README
Refactor is wedged between 12.1 and 12.2. Numbering: 12.1.5? 12-Refactor?
Treat as Phase 11 placeholder reuse?

**Default-if-stuck:** unbenummert in README "Microkernel Refactor (between
12.1 and 12.2)", linked to this doc. Phase-Nummer-Diskussion deferred.

---

## Glossar

| Term | Bedeutung |
|---|---|
| **Bootstrap set** | WASM drivers embedded in the kernel binary, available before npkFS mounts |
| **HW-extension set** | WASM drivers in npkFS, installed via `install <driver>`, optional |
| **Driver-ABI** | `npk_pci_*/mmio_*/dma_*/irq_*` host-fns plus capability tokens |
| **Auto-match** | PCI scan compares (vendor, device) against driver manifests, loads matching WASM |
| **Demo-VM-targets** | QEMU, VirtualBox, VMware — must-work-1A reference set |
| **NUC-Proof** | nopeekOS running on the developer's Intel N100 NUC, validating "real HW works" |
| **virtio dual role** | virtio-blk/net exists as guest-WASM-driver (bootstrap) AND as kernel-host-backend (MicroVM) — same protocol, different code |

---

## What this document is not

- Not a host-fn implementation spec (will live in `kernel/src/wasm.rs`
  doc-comments when steps land).
- Not a manifest schema reference (separate `DRIVER_MANIFEST.md` once
  stable, like `WIDGET_VOCAB.md`).
- Not a per-driver migration guide (each step gets its own commit-trail).
