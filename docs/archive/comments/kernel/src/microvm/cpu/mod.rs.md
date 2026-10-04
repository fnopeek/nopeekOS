# `kernel/src/microvm/cpu/mod.rs` @ 5e0102684

## L1-24 · `pub mod rip_sample;`

```
//! CPU virtualization extensions — vendor dispatch.
//!
//! Detects the host CPU vendor at boot via CPUID leaf 0 (vendor
//! string) and dispatches MicroVM operations to the matching
//! backend:
//!
//!   * `vmx` — Intel VT-x (VMCS, EPT)
//!   * `svm` — AMD-V (VMCB, NPT)  — stub, returns Err for now
//!
//! Public API (`init`, `report`, `run_substrate_test`, `vm_open`,
//! `decode_io_exit_qualification`) is re-exported one level up at
//! `crate::microvm` so callers stay vendor-agnostic.
//!
//! ## Why dispatch-enum, not a Hypervisor trait
//!
//! The two backends share no concrete code paths: VMX uses VMCS
//! reads/writes, SVM mutates a VMCB struct directly; VMX uses EPT,
//! SVM uses NPT; exit reasons / I/O bitmaps / control registers all
//! differ in encoding. A trait pulled across that boundary would be
//! method-by-method passthrough with vendor-specific Output types,
//! providing zero shared implementation. Once both backends ship
//! and we can see what actually generalizes (likely guest-RAM
//! window setup + Linux loader integration), a real trait can be
//! lifted from the convergent code. For now: simple match.
```

## L27 · `pub mod guest_cpuid; // guest CPUID allowlist (KVM kvm_cpu_cap_init model)`

```
// guest CPUID allowlist (KVM kvm_cpu_cap_init model)
```

## L28 · `pub mod guest_msr; // vendor-neutral guest MSR emulation`

```
// vendor-neutral guest MSR emulation
```

## L35-41 · `pub const VMEXIT_BUCKETS: usize = 7;`

```
// ── VM-exit reason histogram (diagnosis) ───────────────────────────
//
// Why is the dedicated VM core busy? Both backends bump a bucket per
// guest exit so `cores` can show the mix while a VM runs: lots of `mmio`
// = the guest is rendering (legit busy); `hlt`/`intr` dominating = idle
// spin; etc. Backend-agnostic categories — each backend maps its own
// exit codes onto these.
```

## L43 · `pub const VMX_INTR: usize = 0;  // ext-interrupt / timer`

```
// ext-interrupt / timer
```

## L45 · `pub const VMX_MMIO: usize = 2;  // NPF / EPT-violation (incl. virtio MMIO)`

```
// NPF / EPT-violation (incl. virtio MMIO)
```

## L58 · `pub fn record_vm_exit(bucket: usize) {`

```
/// Bump the exit-reason bucket for one guest exit.
```

## L65 · `pub fn vm_exit_snapshot() -> [u64; VMEXIT_BUCKETS] {`

```
/// Snapshot all VM-exit buckets (double-sample to get a rate).
```

## L74-78 · `pub const IO_PORT_BUCKETS: usize = 7;`

```
// ── Per-port I/O exit breakdown ────────────────────────────────────
// The `io` exit bucket is dominated, during heavy guest RX, by the PIC
// EOI (`outb 0x20`): the guest runs `noapic`, so every device IRQ is
// ack'd through the 8259, and each ack is its own port-I/O VM-exit.
// Bucketing the ports proves where an io-exit storm actually comes from.
```

## L88 · `0x20 | 0x21 | 0xA0 | 0xA1 => 0,     // 8259 PIC (EOI / mask)`

```
// 8259 PIC (EOI / mask)
```

## L89 · `0x40..=0x43 | 0x61 => 1,            // PIT`

```
// PIT
```

## L90 · `0x2F8..=0x2FF | 0x3F8..=0x3FF => 2, // serial (COM1/COM2)`

```
// serial (COM1/COM2)
```

## L91 · `0xCF8..=0xCFF => 3,                 // PCI config space`

```
// PCI config space
```

## L92 · `0x70 | 0x71 => 4,                   // RTC / CMOS`

```
// RTC / CMOS
```

## L93 · `0x60 | 0x64 => 5,                   // i8042 keyboard`

```
// i8042 keyboard
```

## L94 · `_ => 6,                             // other`

```
// other
```

## L97-99 · `pub fn kvm_hypercall(apic_id: u8, cpl: u8, nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {`

```
/// Bucket one guest port-I/O exit by port (call from the IOIO handler).
/// `kvm_emulate_hypercall`: `nr` + four args, result for RAX. The set is
/// what `guest_cpuid` announces — KVM_HC_SEND_IPI; the rest is -KVM_ENOSYS.
```

## L111-112 · `pub const NPF_BUCKETS: usize = 11;`

```
/// Nested-page-fault targets: which device BAR, the LAPIC page, a first
/// touch of guest RAM (demand paging), or unclaimed.
```

## L138 · `pub fn io_port_snapshot() -> [u64; IO_PORT_BUCKETS] {`

```
/// Snapshot the per-port I/O buckets (double-sample for a rate).
```

## L147-152 · `static VM_EXIT_CYCLES: [AtomicU64; VMEXIT_BUCKETS] = {`

```
// ── Host-time profiler ─────────────────────────────────────────────
// Where do the dedicated guest cores' cycles actually go? Counts (above)
// say HOW OFTEN we exit; these say HOW LONG each kind of handling costs vs
// time spent running the guest (VMRESUME). Proves whether the host wastes
// the core in mmio-decode / io-PIC handling (→ guest starved, "0% CPU"
// because it never gets the time) or genuinely runs the guest.
```

## L159-160 · `pub fn record_exit_cycles(bucket: usize, cycles: u64) {`

```
/// TSC cycles spent IN the handler for `bucket` (between the exit and the
/// next VMRESUME).
```

## L166 · `pub fn record_guest_cycles(cycles: u64) {`

```
/// TSC cycles spent inside VMRESUME (the guest actually executing).
```

## L170 · `pub fn vm_cycle_snapshot() -> ([u64; VMEXIT_BUCKETS], u64) {`

```
/// `(per-bucket handler cycles, total guest cycles)` for a rate snapshot.
```

## L179-190 · `#[repr(C, align(64))]`

```
// ── Guest/host FPU (XSAVE) swap ────────────────────────────────────
//
// `vmrun`/VMRESUME do NOT save or restore x87/SSE/AVX/AVX-512 — host
// and guest share the one physical vector-register file. Any host FPU
// use between two guest entries (nat::pump memcpy/checksum, virtio
// buffer copies, kprintln, the cooperative Shade/fontdue pass)
// silently corrupts the guest's live vector state. musl + librewolf
// use AVX-512 pervasively (memcpy/strlen); corruption mid signal
// restore → the `ret` after rt_sigprocmask faults → SIGSEGV. Rate ∝
// VMRUN/s, so it bites the busy dedicated core hard and the mostly-
// HLT-idle cooperative one rarely. KVM swaps unconditionally
// (kvm_load_guest_fpu / kvm_put_guest_fpu); we do the same.
```

## L192-195 · `#[repr(C, align(64))]`

```
/// 64-byte-aligned XSAVE area. 4 KiB ≫ the ~2.4 KiB the host XCR0
/// (incl AVX-512) needs (CPUID.0xD.0:EBX). Zeroed = XSTATE_BV/XCOMP_BV
/// 0 → `xrstor64` loads the architectural FPU *init* state (x87 init,
/// MXCSR 0x1F80, vectors zeroed) — the correct fresh-guest FPU.
```

## L205-216 · `pub fn choose_guest_ram_bytes() -> u64 {`

```
// We do NOT touch the XCR0 register. XSETBV is not intercepted, so the
// guest's Linux owns XCR0 exactly as it did before this swap existed
// (it boots fine that way — managing XCR0 ourselves only ever caused
// regressions: a forced host-mask vs the guest's CPUID-0xD-masked set
// → fpu__init_system_xstate panic; a forced reset-mask while our
// +avx2-built host code runs → #UD on `vmovups ymm` → KVM emulation
// failure). Mask = -1: xsave64/xrstor64 then operate on every
// component enabled in the *current* XCR0. Guest XCR0 ⊇ host XCR0
// (guest Linux enables ≥ x87+SSE+AVX = our host's set; any extra
// AVX-512 bits it adds are still covered by -1), so host save/restore
// under the guest's XCR0 preserves the host's subset, and guest
// save/restore under it covers all guest state.
```

## L218-231 · `pub fn choose_guest_ram_bytes() -> u64 {`

```
/// Guest-RAM size for the next VM, chosen at `vm_open` from live host
/// free memory instead of a fixed 1 GiB constant.
///
/// B2: the window is still one contiguous, single-PD ≤ 1 GiB block,
/// so advertised == committed == this value. B3 decouples them — the
/// guest will be *advertised* a generous size (demand-paged, scattered)
/// while only touched pages are *committed*, bounded by host free RAM.
///
/// Policy: take host free RAM minus a host reserve, clamp to
/// [`MIN`, cap], floor to the 2-MB EPT/NPT leaf granularity. On a fat
/// host (≥ ~1.3 GB free) this yields exactly the cap (= the validated
/// 1 GiB), so behaviour is unchanged where it was validated; it only
/// shrinks on a genuinely RAM-starved host instead of OOM-failing
/// `allocate_contiguous`.
```

## L244-245 · `const TWO_MB: u64 = 2 * 1024 * 1024;`

```
// Floor to 2 MB so it maps as whole EPT/NPT 2-MB leaves and the
// frame count is a clean multiple of 512.
```

## L250 · `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`

```
/// Host CPU vendor identified at boot from CPUID leaf 0.
```

## L255-256 · `Unknown(&'static str),`

```
/// CPUID returned a string we don't recognize. MicroVM stays
/// disabled. The variant carries a short reason for `report()`.
```

## L262-270 · `pub fn detect_vendor() -> Vendor {`

```
/// Identify the CPU via CPUID leaf 0 vendor string. Three known
/// strings: `GenuineIntel` (Intel), `AuthenticAMD` (AMD), anything
/// else returns `Unknown` with the raw bytes lost.
///
/// Standalone (no kernel state needed): safe to call from boot
/// init paths that run BEFORE `microvm::cpu::init()` has set the
/// cached `VENDOR` static. `smp::per_core::init_dedicated_vm_core`
/// uses this to vendor-gate A2 without writing its own CPUID
/// inline asm (the v0.172.62 attempt hung AMD QEMU).
```

## L273-274 · `let bytes = [`

```
// Vendor string is ebx, edx, ecx (yes, that order — Intel SDM
// Vol. 2A §3.3 "CPUID Vendor String").
```

## L287 · `#[allow(dead_code)] // public surface for future vendor-aware decoders`

```
// public surface for future vendor-aware decoders
```

## L288-289 · `pub fn current_vendor() -> Vendor {`

```
/// A copy: `match *VENDOR.lock() { … }` keeps the guard for the whole
/// match, and the vCPU fiber's match is its entire run.
```

## L295 · `pub fn init() {`

```
/// Boot-time entry: detect vendor, run vendor-specific probe.
```

## L309 · `pub fn report() {`

```
/// Print vendor-specific virt capability snapshot.
```

## L321 · `pub fn run_substrate_test() -> Result<LaunchOutcome, &'static str> {`

```
/// Run the vendor-specific substrate test (`microvm test`).
```

## L330-337 · `enum ActiveVm {`

```
// ── Re-entrant active VM (12.4 step 1b — Core-0 cooperative) ───────
//
// One backend-agnostic active VM, driven by the Core-0 event loop
// via `vm_poll_slice()` instead of a blocking whole-VM run. Holds the
// VmContext so Shade keeps rendering between bounded slices. Single
// global (one VM for now); keyed-registry generalisation deferred per
// the forward-compat contract (consumer side never assumes count).
// Core-0-only access in practice; the Mutex guards against misuse.
```

## L346-349 · `static ACTIVE_VM_WINDOW: AtomicU32 = AtomicU32::new(0);`

```
/// Shade window the active VM's framebuffer is bound to (0 = none).
/// virtio-gpu FLUSH reads this to know which surface to write; the
/// teardown path closes it. One VM ↔ one window for now; keyed by id
/// so it generalises (forward-compat #2).
```

## L352-353 · `static VM_CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);`

```
/// Set when the user closes the VM's window so the next slice tears
/// the guest down instead of running it headless.
```

## L356-360 · `static OPEN_LOFT_REQUESTED: AtomicBool = AtomicBool::new(false);`

```
/// Cross-boundary "open my files in loft" trigger: the guest's browser
/// opens the magic 9p file `<root>/.open-in-loft`, the 9p server (on the
/// VM core) sets this, and Core 0 reaps it in `vm_poll_slice` to spawn
/// loft (a compositor op that MUST run on Core 0). Mirrors the
/// VM_CLOSE_REQUESTED cross-core handoff.
```

## L363-364 · `pub fn request_open_loft() {`

```
/// Request that the host open loft. Safe to call from the VM core; the
/// spawn itself happens on Core 0 (`vm_poll_slice`).
```

## L370-383 · `const VM_IDLE: u8 = 0;`

```
// ── Dedicated-core path (substrate rework A2) ──────────────────────
//
// When `per_core::dedicated_vm_core()` is Some, the guest runs in a
// continuous loop on that worker core instead of cooperative Core-0
// slicing. VMXON / host-state capture / the run loop / VMXOFF must
// ALL execute on that one core — a cross-core open would restore
// Core 0's GDT/TR/RSP onto the VM core on the first VM-exit. So
// Core 0 only stashes a request (`PENDING_VM`); the dedicated core
// (`vm_core_serve`, driven from `smp_ap_entry`) owns the VmContext on
// its own stack for the VM's whole lifetime. Core 0 coordinates only
// via these atomics + the existing `ACTIVE_VM_WINDOW` /
// `VM_CLOSE_REQUESTED` — never the `ACTIVE_VM` mutex (cooperative
// path only), so it can't deadlock against the unbounded run loop.
// The cooperative path (≤2 cores) is byte-for-byte unchanged.
```

## L390-392 · `static VM_RUN_STATE: AtomicU8 = AtomicU8::new(VM_IDLE);`

```
/// Dedicated-core VM lifecycle. Only meaningful when a core is
/// dedicated (cooperative path uses `ACTIVE_VM` instead). Also reused by
/// the fiber path (REQUESTED → RUNNING → EXITED).
```

## L395-404 · `pub const VCPU_AS_FIBER: bool = true;`

```
// ── vCPU-as-fiber (unified core pool, Stage: docs/plan/SCHEDULER_FIBERS.md) ───────
//
// Instead of statically carving a core for the guest at boot, run the
// guest's VMRESUME loop as a normal pool FIBER (smp::fiber): admitted when
// a launch is requested, pinned to whatever worker core picks it up (so the
// VMX/SVM core-binding holds — no migration), yielding the core to peer app
// fibers on guest-idle, and freed on guest exit. No wasted core when no VM
// runs; the guest naturally dominates its core while busy. Multi-vCPU later
// = N such fibers. Flag-gated so a bad release reverts to the validated
// dedicated path via OTA (no reinstall): flip to `false` + re-release.
```

## L407-414 · `pub const VMX_VCPU_AS_FIBER: bool = true;`

```
/// Intel parity: run the VMX guest as a pool fiber too (not just AMD/SVM),
/// so the browser leaves the cooperative Core-0 path (which shares Core 0
/// with Shade/input/cursor and made the guest — and the Core-0 PS/2 mouse
/// poll — unusably slow under load). Single-vCPU only (guest-SMP AP bring-
/// up stays SVM-only). Requires per-core TSS on the worker (`tss::ensure_
/// core`, done in `vmx::vm_open`). Flag-gated for clean OTA rollback: flip
/// to `false` + re-release → Intel reverts to cooperative Core 0, AMD
/// unaffected (its fiber mode keys off vendor, not this flag).
```

## L417-423 · `pub const GUEST_LAPIC: bool = true;`

```
/// Guest-SMP Stage 1: trap-and-emulate the guest local APIC
/// (`svm::lapic`) instead of booting `nolapic`. When true, the guest
/// cmdline omits `nolapic` so Linux brings up the LAPIC + uses its timer;
/// the LAPIC MMIO page NPT-faults into the emulator. Flag-gated so a bad
/// release reverts via OTA (flip to `false` + re-release → `nolapic`
/// back → the UP guest boots with the LAPIC disabled, emulator inert —
/// no guest-kernel rebuild needed). Prerequisite for AP bringup (Stage 3).
```

## L426-438 · `pub const GUEST_SMP: bool = true;`

```
/// Guest-SMP Stage 2: ENUMERATE a 2nd vCPU to Linux via an MP-table
/// (`linux::mptable`, floating pointer @ 0xF0000). Linux counts
/// GUEST_VCPUS CPUs; CPU1 becomes present-but-offline. The boot cmdline
/// stays `maxcpus=1`, so Linux does NOT online (bring up) the AP yet —
/// starting it is Stage 3, and an enumerated-but-never-responding AP
/// HANGS Linux's cpuhp bring-up (it waits for CPU1 to report alive; the
/// non-responding-AP timeout doesn't recover on this backend — observed
/// as a freeze right after INIT/SIPI on HW, v0.192.1). The `svm::lapic`
/// ICR INIT/SIPI decode is in place + verified firing (Stage 3 wires the
/// actual AP-fiber spawn). Requires `GUEST_LAPIC` (no LAPIC → no MP-table
/// point). Flag-gated so a bad release reverts via OTA (flip to `false` +
/// re-release → no MP-table → identical to the validated v0.191.13 UP
/// guest; no guest-kernel rebuild needed).
```

## L441-450 · `pub fn guest_vcpus() -> u8 {`

```
/// Number of vCPUs the MP-table enumerates to the guest when `GUEST_SMP` is on
/// (BSP apic_id 0 + APs 1..). **Dynamic**: one vCPU per host *worker* core
/// (Core 0 stays the shell/reaper, so worker count = `core_count() - 1`),
/// capped at `MAX_VCPUS_CAP` and floored at 1. Bigger machines therefore run
/// more guest vCPUs automatically; a 1-2 core host falls back to single-vCPU
/// (no AP). The browser dominates the workers while busy; idle vCPUs park.
///
/// Called at VM-launch time (well after SMP bring-up, so `core_count()` is
/// stable) by the MP-table builder, the `maxcpus=` cmdline, and the IPI
/// broadcast loops — all see the same value for one run.
```

## L453-459 · `if reserve_offload_core() {`

```
// EXPERIMENT (RESERVE_OFFLOAD_CORE): when the off-vCPU net backend runs it
// needs its OWN worker core, never a vCPU's (`place_worker`).
// Co-located with a vCPU, the vCPU gets
// preempted by the worker, so it answers cross-vCPU TLB-shootdown IPIs late
// → the other vCPUs spin in csd_lock_wait (~40%). Leaving one worker core
// free maps each vCPU 1:1 to a core (like nested-Linux/iperf → 1 Gbit), so
// IPIs are answered promptly. Flag-gated for clean before/after measurement.
```

## L463-464 · `if reserve_gpu_core() {`

```
// A second reserved core for the off-vCPU GPU worker (framebuffer copy off
// the vCPU). One fewer vCPU; the browser is not guest-CPU-bound so it wins.
```

## L468 · `workers = workers.saturating_sub(protected_cores().count_ones() as usize);`

```
// And one per core carrying the network's own fibers — see `protected_cores`.
```

## L473-487 · `static PROTECTED_CORES: AtomicU32 = AtomicU32::new(0);`

```
/// Bitmask of cores that must stay clear of vCPUs: every core a hardware
/// driver lives on (`per_core::driver_cores` — a fiber there bound a device
/// or waits on its interrupt), the WASM NIC driver's and the WiFi manager's.
///
/// A vCPU fiber and a driver fiber on one core are cooperative peers — the
/// driver runs only when the vCPU yields, once per `SLICE_MS` (3 ms) at best.
/// The touchpad went dead that way, and a NIC driver holds only ~50 ms of
/// receive buffers (the host's own network rides it too).
///
/// A core whose fibers are apps (panels, dock) is NOT protected: they sleep on
/// events, and a vCPU yielding every slice costs them at most that slice. It
/// was protected until 0.443 — which left QEMU's guest one vCPU on six cores.
///
/// Snapshotted while no VM exists: `guest_vcpus()` sizes the MP table and the
/// IPI broadcast and must not change under a running guest.
```

## L499-503 · `let n = crate::smp::per_core::core_count().min(32);`

```
// Every other fiber core too, for now: 0.444 put an AP beside the
// bar and both vCPUs hung in the host (no VM exits at all). With
// cooperative fibers and spin locks, a peer that yields while holding
// a lock the vCPU needs is never run again. Until that is proven or
// ruled out (`cores` shows each vCPU's phase), vCPUs get empty cores.
```

## L509 · `mask &= !1; // Core 0 is never a vCPU core anyway`

```
// Core 0 is never a vCPU core anyway
```

## L514-515 · `fn least_fibered(candidates: u32) -> Option<usize> {`

```
/// Among the cores in `candidates` (bitmask), the one with the fewest resident
/// fibers — an idle core first, then one whose apps sleep.
```

## L523-524 · `pub fn is_vm_worker_core(cid: usize) -> bool {`

```
/// True if `cid` runs a vCPU or a microvm worker (not a protected host core).
/// Such a core must not pick up new host work while the guest runs.
```

## L533-535 · `fn pick_vcpu_core() -> usize {`

```
/// Lowest worker core the network does not need. Whichever core picks up the BSP
/// vCPU fiber owns the guest for its whole lifetime, so the choice is made here
/// rather than left to work-stealing.
```

## L543-545 · `pub const RESERVE_OFFLOAD_CORE: bool = true;`

```
/// EXPERIMENT toggle: reserve a dedicated worker core for the off-vCPU net
/// backend fiber (see `guest_vcpus`). Flip to `false` + re-release to revert to
/// the co-located-worker behavior (one more vCPU, but csd_lock_wait contention).
```

## L548-550 · `fn reserve_offload_core() -> bool {`

```
/// True when the off-vCPU net worker will actually claim a core this run (full
/// RX+TX backend, AMD/SVM only today) AND the reservation experiment is on AND
/// there is a core to spare. Mirrors the `full_backend` gate at `start_worker`.
```

## L552-563 · `RESERVE_OFFLOAD_CORE && crate::smp::per_core::core_count() >= 3`

```
// NOT vendor-gated, and the AMD gate was the bug: the reservation existed
// only where it was least needed. The worker owns the guest's rings and, for
// a card that raises no RX interrupt, polls that card as well — so it is the
// machine's inbound path for the guest AND, while it holds the drain guard,
// for the host's own sockets. An unmarked core lets an AP vCPU land on top
// of it at the next guest SIPI, so `place_worker(true)` marks it.
// Measured: a host TCP connection to GitHub sat ESTABLISHED with nothing
// arriving, at the same moment the guest's network died. AMD never showed it
// because the reservation already gave the worker a core of its own there.
//
// Needs >= 3 cores: Core 0 + >= 1 vCPU + 1 worker. Below that, co-location is
// the lesser evil against starving the guest to nothing.
```

## L567-570 · `fn reserve_gpu_core() -> bool {`

```
/// Reserve a SECOND dedicated worker core for the off-vCPU GPU backend (the
/// ~8 MB/frame framebuffer copy + write_frame). Needs ≥4 cores: Core 0 + ≥1 vCPU
/// + net worker + gpu worker. AMD-only (the off-vCPU backends are SVM-only today).
/// Below that the GPU stays inline on the vCPU (Stage 1 / the framerate-throttle).
```

## L577-581 · `pub const MAX_VCPUS_CAP: usize = 8;`

```
/// Hard cap on guest vCPUs (sizes the per-backend IPI bitmaps + the spawn
/// bitmask). 8 covers an 8-thread notebook fully and is plenty for a browser;
/// a 16/32-core desktop caps here rather than spawning a vCPU per core (idle
/// vCPUs each carry a small wake overhead). Must be ≤ each backend's
/// `MAX_VCPUS` and ≤ 32 (the `u32` spawn bitmask).
```

## L584-590 · `pub const GUEST_SMP_AP: bool = true;`

```
/// Guest-SMP Stage 3b-2: actually BRING UP the AP vCPU. When true the boot
/// cmdline raises `maxcpus` to `GUEST_VCPUS`, the guest's INIT-SIPI spawns
/// a second vCPU fiber sharing the BSP's `VmShared` (svm only), and the
/// cross-vCPU IPI path is exercised. Default FALSE: shipping is byte-
/// identical (no spawn, `maxcpus=1`, big-lock never taken) and flipping it
/// on is the AP test — a bad release reverts via OTA by flipping back to
/// false + re-release (clean rollback, no reinstall). Requires `GUEST_SMP`.
```

## L593-596 · `pub fn smp_ap_active() -> bool {`

```
/// Whether guest-SMP AP bring-up is enabled AND supported on this host. Both
/// backends now have an AP-vCPU open path (`svm::vm_open_ap` / `vmx::vm_open_ap`)
/// + SIPI/LAPIC routing, so this gates the guest's `maxcpus` (and the AP spawn)
/// on AMD and Intel alike. Unknown vendor stays single-vCPU.
```

## L601-604 · `fn vm_set_ap_active(on: bool) {`

```
/// Set/clear the active backend's guest-SMP big-VM-lock engagement. Dispatches
/// via `detect_vendor` (lock-free CPUID) NOT `current_vendor()` (which locks
/// VENDOR) — the BSP vCPU fiber holds the VENDOR lock for its whole run loop,
/// so the last-one-out call from inside that arm must not re-lock it.
```

## L613-623 · `pub const GUEST_IOAPIC: bool = true;`

```
/// Whether the guest's local APIC is emulated on this host. LAPIC emulation
/// (`svm/lapic.rs`) is SVM-only — there is no VMX equivalent yet. With
/// `GUEST_LAPIC` on but no emulation, an Intel guest would program the LAPIC
/// (TSC-deadline) timer and then never receive a tick → its event loops (cage/
/// Wayland, schedulers) hang forever. So on Intel we must keep `nolapic` in the
/// cmdline and let the guest fall back to the PIT IRQ0 the VMX path injects.
/// An I/O APIC in the MP table, and the guest booted without `noapic`: device
/// lines reach the vCPUs as LAPIC vectors (PV-EOI, no 8259 port exits), and
/// Linux enables x2APIC — which it will not do under `noapic`
/// (`enable_IR_x2apic` returns before trying). Needs the MP table and a LAPIC.
/// Flip to `false` + re-release to go back to the 8259.
```

## L639-644 · `pub const VMX_GUEST_LAPIC: bool = true;`

```
/// Intel parity #2: emulate the guest local APIC on VMX too (`vmx::lapic`
/// reuses the pure `svm::lapic::LocalApic`). When true the Intel guest boots
/// WITHOUT `nolapic` and the LAPIC MMIO page EPT-faults into the emulator;
/// when false it keeps `nolapic` (byte-identical to the validated pre-LAPIC
/// Intel boot). Flag-gated for clean OTA rollback. Prerequisite for VMX
/// guest-SMP (#4) — Linux needs the per-CPU LAPIC timer to schedule APs.
```

## L647-654 · `const ORCH_MAX_VCPUS: usize = 8;`

```
// ── AP (secondary vCPU) spawn orchestration (guest SMP, N vCPUs) ────────
//
// The guest's INIT-SIPI is decoded on a vCPU fiber (a worker core), which
// CANNOT push a fiber itself (the run-queue deque is single-producer, owned by
// Core 0). So the SIPI handler only records a request here (per target
// apic_id); the Core-0 reaper (`vm_poll_slice`) does the actual `spawn_fiber`
// for each newly-requested AP. N-vCPU-ready: each AP is tracked by its apic_id
// bit, so a guest with several APs brings them all up.
```

## L656-657 · `const ORCH_MAX_VCPUS: usize = 8;`

```
/// Largest apic_id + 1 the orchestration tracks. Matches the per-backend
/// `MAX_VCPUS` (vmx + svm) that size the IPI bitmaps + `MAX_VCPUS_CAP`.
```

## L660-661 · `static AP_SPAWN_REQUESTED: AtomicU32 = AtomicU32::new(0);`

```
/// Bitmask of apic_ids the guest has SIPI'd (1 << apic_id). The reaper spawns
/// a fiber for each set bit not yet in `AP_SPAWNED`.
```

## L663-664 · `static AP_SPAWNED: AtomicU32 = AtomicU32::new(0);`

```
/// Bitmask of apic_ids already spawned (the reaper's idempotence guard:
/// absorbs the retried 2nd SIPI per AP).
```

## L666-667 · `static AP_SIPI_VECTORS: [AtomicU8; ORCH_MAX_VCPUS] =`

```
/// SIPI start vector per apic_id (Linux uses one trampoline page, but track
/// per-id to stay correct if that ever changes).
```

## L670-671 · `static AP_SHARED_PTR: AtomicU64 = AtomicU64::new(0);`

```
/// The BSP's `*mut VmShared` (as u64), published once the BSP opens, for an
/// AP fiber to alias. 0 = not yet published.
```

## L673-676 · `static VCPU_COUNT: AtomicU32 = AtomicU32::new(0);`

```
/// Live vCPU count for this VM. The BSP sets it to 1 at open; the reaper bumps
/// it per AP it spawns; each fiber decrements on exit. The BSP (owner) waits
/// for it to return to 1 before `close()` so no AP touches freed shared state
/// (last-one-out).
```

## L679-685 · `static VM_CORE_MASK: AtomicU32 = AtomicU32::new(1); // bit 0 = Core 0 reserved`

```
/// Bitmask of host cores currently running a vCPU fiber (bit c). Core 0 (bit 0)
/// is always reserved (shell/reaper). Each vCPU MUST get a DISTINCT core: VMX
/// root is per-physical-core, so two vCPUs VMXONing one core fails with
/// VMfailValid (the N>2 bug); on SVM it just starves. The reaper places each AP
/// on the lowest free worker core via `reserve_ap_core` + `fiber::admit`
/// instead of `spawn_fiber` (whose work-stealing piles every fiber onto the one
/// awake core). Reset at BSP open + teardown.
```

## L686 · `static VM_CORE_MASK: AtomicU32 = AtomicU32::new(1); // bit 0 = Core 0 reserved`

```
// bit 0 = Core 0 reserved
```

## L688-690 · `static VCPU_CORES: AtomicU32 = AtomicU32::new(0);`

```
/// Host cores running a vCPU fiber (the BSP's and each AP's). A worker is
/// never placed here: fibers are cooperative, and a vCPU gives its core up
/// once per slice at best — a worker beside it answers a kick in milliseconds.
```

## L692 · `static WORKER_CORES: AtomicU32 = AtomicU32::new(0);`

```
/// Cores already given a microvm worker, so the next one goes elsewhere.
```

## L695 · `pub fn is_vcpu_core(cid: usize) -> bool {`

```
/// Does a vCPU run on host core `cid` right now?
```

## L701-709 · `pub fn place_worker(claim: bool) -> usize {`

```
/// Place a microvm worker (net data plane, GPU copy, 9p persist). Never Core 0
/// and never a vCPU's core. In order:
///   1. a core nobody uses (not in `VM_CORE_MASK`) — `claim` marks it, so a
///      later AP vCPU does not land on it;
///   2. a host-fiber core without a worker yet, fewest fibers first. Those
///      fibers park on events; an event-driven worker shares with them fine
///      (the NIC driver's and WiFi manager's cores excluded — they pump);
///   3. any core that is not a vCPU's, fewest fibers first.
/// Only when every worker core runs a vCPU does it share one.
```

## L745-749 · `fn reserve_ap_core() -> Option<usize> {`

```
/// Reserve a distinct idle worker core (1..core_count()) for an AP vCPU fiber,
/// marking it taken. `None` if every worker core already runs a vCPU (the
/// caller then falls back to the shared deque — only safe on SVM; on VMX that
/// means more vCPUs than host cores, which `guest_vcpus()` avoids by capping at
/// the worker count). Called only from the Core-0 reaper (single producer).
```

## L766-769 · `pub fn request_ap_spawn(apic_id: u8, sipi_vector: u8) {`

```
/// Record a guest SIPI (from a backend ICR router) → ask Core 0 to spawn the
/// AP with `apic_id` at `sipi_vector`. No-op unless guest-SMP AP bring-up is
/// enabled or apic_id is out of range. Idempotent: the reaper's `AP_SPAWNED`
/// guard ignores a re-SIPI of an already-spawned AP.
```

## L776 · `crate::intent::wake_shell();`

```
// Core 0 spawns it in `vm_poll_slice`; it no longer looks every 10 ms.
```

## L780-781 · `pub fn publish_ap_shared(ptr: u64) {`

```
/// The BSP publishes the address of its (heap-boxed) `VmShared` so a
/// spawned AP fiber can alias it.
```

## L786-787 · `fn ap_orch_reset() {`

```
/// Reset the AP-spawn orchestration statics after the last vCPU has exited
/// (BSP teardown), so a relaunch starts clean.
```

## L792 · `VM_CORE_MASK.store(1, Ordering::Release); // only Core 0 reserved`

```
// only Core 0 reserved
```

## L797-799 · `static VM_FIBER_MODE: AtomicBool = AtomicBool::new(false);`

```
/// Decided once at boot (`set_vm_fiber_mode`, from `init_dedicated_vm_core`,
/// which has the vendor + worker count): true → the guest runs as a pool
/// fiber and NO core is dedicated. Cheap atomic read on the hot poll path.
```

## L802 · `pub fn set_vm_fiber_mode(on: bool) {`

```
/// Set the fiber-mode decision (called once from `init_dedicated_vm_core`).
```

## L807-808 · `pub fn vm_fiber_mode() -> bool {`

```
/// True if the guest runs as a pool fiber (vs the dedicated-core or the
/// cooperative-Core-0 path).
```

## L813-814 · `struct PendingVm {`

```
/// One pending launch request, owned copies so the caller's npkFS
/// buffers can drop while the dedicated core consumes them.
```

## L823-824 · `pub fn vm_bind_window(window_id: u32) {`

```
/// Bind the active VM to a Shade Surface window (called by the
/// microvm intent right after a successful `vm_open`).
```

## L829 · `pub fn vm_window() -> u32 {`

```
/// The Shade window the active VM renders into (0 = none/unbound).
```

## L834-835 · `pub fn vm_close_for_window(window_id: u32) {`

```
/// Window closed by the user → ask the bound VM to power off on the
/// next slice. No-op if it isn't the active VM's window.
```

## L843-848 · `pub fn note_guest_shutdown() {`

```
/// The guest shut itself down (e.g. LibreWolf's own window-X → cage exits →
/// PID-1 `halt` → `reboot: System halted`). The serial scanner calls this so
/// the run loop takes the SAME clean exit as a user Mod+Q: break → `close()`
/// (saves the home image) → Core-0 reaper closes the window. Without it a
/// `cli;hlt`-halted guest just spins the run loop forever (black tile, no
/// save) until the user closes the host window manually.
```

## L854-856 · `fn teardown_vm_window() {`

```
/// Drop the VM↔window binding + its surface and close the Shade
/// window. MUST be called with the ACTIVE_VM lock NOT held (it locks
/// the compositor, whose close path re-enters microvm). Idempotent.
```

## L866-870 · `const SLICE_BUDGET: u32 = 4096;`

```
/// Exit-count cap per Core-0 poll, a secondary bound: `run_slice`
/// also enforces a ~3 ms wall-clock deadline (see vmx/svm
/// `SLICE_MS`), which is what actually keeps a busy guest from
/// starving Shade. Cheap boot exits hit this count first → same
/// boot wall-time; a busy compositor hits the deadline first.
```

## L873-886 · `static BSP_HOST_CORE: AtomicUsize = AtomicUsize::new(usize::MAX);`

```
/// True if Core 0 should spin-feed a cooperative microvm. On the
/// dedicated path the guest runs on its own core, so Core 0 does NOT
/// spin — it idles/composites normally and reaps via `vm_poll_slice`.
/// Hence: false on the dedicated path (the guest is still composited
/// through the `focused_surface_id` branch, independent of this).
/// Is a guest running right now, on ANY path?
///
/// `vm_active` is NOT this question despite the name — it answers "is a guest
/// running on the cooperative Core-0 path" and returns false for every
/// fiber-mode guest, which is all of them today. Anything that means "is there
/// a guest" wants this one.
/// Host core running the BSP vCPU fiber, or `usize::MAX`. Vendor-neutral, so
/// the RX producer can wake the consumer on BOTH backends — SVM had
/// `kick_bsp_net_irq` and VMX had nothing at all.
```

## L889-900 · `pub fn bsp_host_core() -> Option<usize> {`

```
/// The wake half of irqfd (`virt/kvm/eventfd.c`): an RX-ready signal both RAISES
/// the guest's IRQ line and WAKES the vCPU, in one act. `net_backend::raise_irq`
/// is the raise; this is the wake — kick the core running the BSP vCPU so it
/// takes an exit and folds the line into `pending_irqs`.
///
/// Vendor-neutral on purpose. It lived under `svm/` and keyed off SVM's own
/// `VCPU_HOST_CORE`, so on Intel the off-vCPU data plane had no way to wake the
/// guest at all — and both target machines are Intel. `BSP_HOST_CORE` is written
/// by `vcpu_fiber_task` before either vendor's run loop starts.
/// Also the wake for every other device source the BSP services (input,
/// 9p replies): a blocked vCPU parks until a timer deadline or this kick.
/// Core running the BSP vCPU, if a guest runs.
```

## L908-915 · `#[inline]`

```
/// Last step before a guest entry (`vcpu_enter_guest`: IRQs off, then
/// recheck): disable host interrupts, arm the host one-shot at `deadline`,
/// and cancel the entry if a kick came in since `kick_gen` was read. With
/// IF=1 here, a kick or the timer interrupt was taken by the host and exited
/// nothing: the vCPU ran on with a posted vector or with no armed timer, and
/// nothing ever brought it out (both vCPUs hung in guest for minutes). With
/// IF=0 either stays pending and exits the guest at once — a deadline
/// already past included. `false` = IF is back on, loop again.
```

## L918-919 · `unsafe { core::arch::asm!("cli", options(nomem, nostack)) };`

```
// SAFETY: the vCPU loop runs with IF=1; `entry_irqs_on` or the VMX
// exit asm sets it again.
```

## L929 · `#[inline]`

```
/// Host interrupts back on after the guest exit; a pending one is taken now.
```

## L932 · `unsafe { core::arch::asm!("sti", options(nomem, nostack)) };`

```
// SAFETY: counterpart of `entry_irqs_off`.
```

## L954-957 · `pub fn vm_open(`

```
/// Open a microvm and register it as the active VM. Non-blocking:
/// does the (synchronous, one-time) substrate + guest-image setup,
/// then returns — slices run later via `vm_poll_slice`. Errors if a
/// VM is already active.
```

## L964-967 · `if vm_fiber_mode() {`

```
// Fiber path: stash the request + spawn a vCPU fiber. A worker admits
// it (smp_ap_entry → fiber::admit) and runs the guest ON that core for
// its lifetime (VMXON/run/VMXOFF all bind there; the fiber is pinned).
// Owned copies so the caller's npkFS Vecs can drop.
```

## L979 · `let _ = protected_cores();`

```
// Snapshot the protected cores while still idle (see `protected_cores`).
```

## L982-985 · `crate::smp::fiber::admit(pick_vcpu_core(), vcpu_fiber_task, 0);`

```
// Place it deliberately: `spawn_fiber` hands the task to whichever
// worker steals it first, and that core owns the guest for its whole
// lifetime. Landing on the core a WASM NIC driver polls from costs the
// machine its network (see `nic_core`).
```

## L990-991 · `if crate::smp::per_core::dedicated_vm_core().is_some() {`

```
// Dedicated path: hand the request to the VM core (which opens it
// on itself). Owned copies so the caller's npkFS Vecs can drop.
```

## L1022-1025 · `pub fn vm_poll_slice() {`

```
/// Run one bounded slice of the active VM, if any. Called from the
/// Core-0 poll cadence (next to `net::poll`). Cheap no-op when no VM.
/// On guest exit / fault: log, free resources, clear the slot so a
/// new VM can be opened (relaunch).
```

## L1027-1029 · `if OPEN_LOFT_REQUESTED.swap(false, Ordering::AcqRel) {`

```
// Cross-boundary trigger: the microvm browser asked (via 9p) to open
// loft. Spawn it here on Core 0 (compositor op). Runs on both the
// cooperative and dedicated paths since it's before the early return.
```

## L1034-1045 · `if GUEST_SMP_AP {`

```
// Dedicated path AND fiber path: Core 0 is only the reaper. The VM
// core (dedicated core, or the worker running the vCPU fiber) owns the
// VmContext for its whole lifetime and does its own VMXOFF; Core 0 just
// runs the compositor-locking teardown once it has exited
// (teardown_vm_window must run on Core 0). Cheap atomic load on the hot
// poll path when nothing has exited.
// Guest-SMP (Stage 3b-2): a guest SIPI asked us to bring up the AP.
// Core 0 owns the run-queue deque, so it does the spawn here — the BSP
// vCPU fiber that decoded the SIPI runs on a worker and cannot push.
// Once per VM (AP_SPAWNED guard absorbs the retried 2nd SIPI). AP_ACTIVE
// is set BEFORE the spawn so the BSP starts taking the big-VM lock
// before the AP can run.
```

## L1054 · `if AP_SPAWNED.fetch_or(1u32 << apic_id, Ordering::AcqRel)`

```
// Claim it (idempotent against a re-SIPI / re-poll).
```

## L1061-1062 · `VCPU_COUNT.fetch_add(1, Ordering::AcqRel);`

```
// AP_ACTIVE before the spawn so the BSP starts taking the
// big-VM lock before the AP can run.
```

## L1066-1070 · `match reserve_ap_core() {`

```
// Place the AP on a DISTINCT idle worker core (one vCPU per
// core — VMX root is per-core). `fiber::admit` pushes straight
// to that core's fiber queue and wakes it by IPI (idle workers
// have no tick) instead of `spawn_fiber`, whose work-stealing piled every
// vCPU onto the one awake core → VMXON-VMfailValid on Intel.
```

## L1080-1082 · `crate::kprintln!(`

```
// More vCPUs than host cores — share via the deque (SVM
// tolerates it; guest_vcpus() caps at the worker count
// so VMX shouldn't reach here).
```

## L1117-1118 · `if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {`

```
// User closed the VM's window → force the guest down this tick
// instead of running it headless until idle.
```

## L1126 · `drop(slot); // release before teardown — it locks the compositor`

```
// release before teardown — it locks the compositor
```

## L1138-1139 · `Some(ActiveVm::Vmx(ctx)) => match ctx.run_slice(SLICE_BUDGET) {`

```
// Cooperative Core-0 path: Idle == StillRunning here — just hand
// Core 0 back to the shell, whose own idle HLT throttles the loop.
```

## L1158 · `drop(slot); // release before teardown — it locks the compositor`

```
// release before teardown — it locks the compositor
```

## L1173-1174 · `#[inline]`

```
/// Host-idle the dedicated core while the guest is halted: until its next
/// timer deadline or an interrupt / kick IPI (`kvm_vcpu_block`).
```

## L1183-1191 · `pub fn vm_core_serve() {`

```
/// Dedicated-core entry point — called every iteration of the
/// dedicated worker core's `smp_ap_entry` loop. Cheap no-op unless a
/// launch is pending. When one is, this opens the VM **on this core**
/// (so VMXON / `write_host_state` / VMPTRLD / VMRESUME / VMXOFF all
/// bind here, never Core 0), runs it to exit / window-close in a
/// continuous loop, closes it, and signals Core 0 to reap. Blocks the
/// dedicated core for the guest's whole lifetime — that is the point:
/// the guest no longer fights Shade + the shell for Core 0. Never
/// touches `ACTIVE_VM` (cooperative-path only).
```

## L1205-1208 · `unsafe { core::arch::asm!("sti") };`

```
// Host interrupts ON between VMRUNs, so the one-shot armed before
// each entry (`arm_vcpu_timer`) and kick IPIs are taken and EOI'd.
// SAFETY: ring-0; CLGI/STGI still brackets the VMRUN-critical
// region inside run_guest_once. This mirrors Core 0's IF=1.
```

## L1211-1217 · `match current_vendor() {`

```
// Continuous run: identical primitive to `vmx::run_linux` step 1a
// (open + `loop run_slice` + close) plus a window-close check so
// the user can `Mod+Q` the tile. `run_slice` returns periodically
// on its wall-clock deadline; we loop straight back (no Shade
// composite, no hlt on this core) → near-native guest. The two
// backends have distinct `SliceOutcome` enums, so match each
// concretely.
```

## L1298-1300 · `unsafe { core::arch::asm!("cli") };`

```
// Restore the IF=0 state `smp_ap_entry`'s park loop expects (it does
// its own sti;hlt;cli).
// SAFETY: ring-0; return the core to the parked-loop invariant.
```

## L1303 · `drop(pending); // owned guest-image buffers freed`

```
// owned guest-image buffers freed
```

## L1304 · `VM_RUN_STATE.store(VM_EXITED, Ordering::Release);`

```
// Hand off to Core 0's reaper (compositor-locking teardown).
```

## L1309-1330 · `const PARK_SAFETY_MS: u64 = 8;`

```
/// vCPU-as-fiber entry (fiber mode). Same lifecycle as `vm_core_serve` —
/// consume the pending request, open the guest ON THIS (the admitting)
/// core, run it slice-by-slice, close it, signal Core 0 to reap — but
/// instead of a dedicated forever-loop it YIELDS the core to peer app
/// fibers between slices (immediately on `StillRunning`, ~2 ms on guest
/// `Idle`). Pinned to its core (no fiber migration) so the VMX/SVM
/// core-binding holds. IF=1 only across each `run_slice` (host tick
/// servicing between VMRUNs), restored to IF=0 before every yield so peer
/// fibers keep the cooperative IF=0 invariant. The guest IRQ0 clock is
/// `ticks()`-paced (global, Core-0 100 Hz), independent of this cadence.
/// Park the BSP vCPU fiber when the guest is idle. While a download is in
/// flight (`nat::recently_active`), park EVENT-DRIVEN on the host NIC RX IRQ
/// (routed to this core by `irq::arm`) so a momentary lull is woken the instant
/// the next RX batch arrives — instead of `yield_sleep`, whose wake is the
/// 100 Hz worker timer (~10 ms granularity), which was the measured loaded-
/// latency floor (drainmax ~10 ms ≈ rxlat max). 2 ms timeout is the fallback if
/// the NIC is polled (RX IRQ never fires) or the link goes quiet mid-park.
/// Idle-park safety cap. The real wakes are event-driven — an RX/TX/IPI kick
/// (net-kick generation) or the guest's own LAPIC-timer deadline — so this only
/// bounds a truly idle guest (no timer armed, no traffic) from sleeping forever
/// and never re-checking VM_CLOSE_REQUESTED. 8 ms ≈ the old idle yield, but a
/// download/active guest never reaches it.
```

## L1333-1347 · `fn vcpu_block_deadline(next_timer_tsc: Option<u64>) -> u64 {`

```
/// Park the BSP vCPU fiber when the guest is idle — the unified block-on-event
/// model (KVM `kvm_vcpu_block`): one park, woken by whichever event fires first.
///   * RX/TX/IPI ready → the off-vCPU backend / a peer vCPU bumps this core's
///     net-kick generation + sends a VCPU_KICK IPI → we resume in ~µs.
///   * Guest timer due → `next_timer_tsc` is the guest's next LAPIC-timer
///     deadline (KVM `apic_timer_fn` hrtimer); the park ends there so the guest
///     1 kHz clock advances at its programmed rate INDEPENDENT of VMRUN. This
///     replaced the magic 1/2 ms parks that polled the timer only while in
///     VMRUN, freezing the guest clock to ~300 effective HZ under load.
///   * Safety cap (`PARK_SAFETY_MS`) — only an idle guest with no timer reaches it.
/// (A bounded spin-while-active test was REVERTED: it pegged a worker core and
/// starved the compositor — the consumer-wake must be event-driven, not spun.)
/// Wake-up bound for a blocked vCPU: the guest's next timer tick, clamped to
/// [now, safety]. A past deadline re-enters at once; a far or absent one
/// re-checks no later than the safety cap.
```

## L1359-1361 · `if crate::microvm::devices::net_dataplane::active() {`

```
// When the off-vCPU RX backend (or producer) owns the NIC drain, RX wakes us
// via the net-kick generation — we must NOT arm/route the host RX IRQ here
// (that would steal the worker's event-wake). Block on the unified deadline.
```

## L1363-1367 · `crate::smp::fiber::kick_wait_until(deadline);`

```
// The off-vCPU worker injects RX + kicks this fiber. A vCPU-side halt-poll
// (busy-spin before HLT) was REVERTED: it pegged the BSP core at 100% and
// pushed guest HLT exits to ~76k/s with NO throughput gain — the guest
// idles waiting for data (cwnd=10 server-limited), it is not CPU-bound, so
// keeping it warm buys nothing. Event-driven park (kick or timer deadline).
```

## L1372-1373 · `let timeout_ms = ((deadline.saturating_sub(now)) / (freq / 1000).max(1)).max(1);`

```
// No backend: THIS vCPU drains the NIC itself, so it must also wake on the
// host RX IRQ (routed to this core). Bound the wait by the timer deadline.
```

## L1375-1376 · `if let Some(vec) = crate::netdev::rx_wake_vector().filter(|_| !crate::net::napi::active()) {`

```
// Not while the NAPI fiber owns the vector: `arm` would route the card's
// interrupt away from the fiber that drains it.
```

## L1378 · `let since = crate::irq::arm(vec); // snapshot + route IRQ to this core`

```
// snapshot + route IRQ to this core
```

## L1402-1405 · `VM_CORE_MASK.store((1u32 << 0) | (1u32 << cid), Ordering::Release);`

```
// Guest SMP: (re)initialise the vCPU-core mask with the BSP's own core so
// APs are placed on OTHER cores (one vCPU per distinct core). Fresh store,
// not OR, so a relaunch starts clean. Runs before the guest boots → before
// any SIPI → the reaper always sees the BSP bit.
```

## L1409-1410 · `VM_CORE_MASK.fetch_or(protected_cores(), Ordering::AcqRel);`

```
// Mark the network's cores taken so no AP vCPU and no offload worker is ever
// placed there.
```

## L1412-1414 · `crate::microvm::devices::nat::reset_counters();`

```
// Clean counter window for this run. Deliberately here and not at teardown,
// so the numbers from the last run survive it — a post-mortem is the one
// time anyone reads them.
```

## L1417-1427 · `let worker_core = place_worker(reserve_offload_core());`

```
// Spawn the data-plane worker on another core: it moves frames between the
// tap and the guest rings, so THIS BSP vCPU does no net work beyond the TX
// doorbell and its IRQ. Stopped in this fiber's teardown + vm_poll_slice.
//
// No vendor condition, and no switch. It carried `&& Amd` because the IRQ
// fold and the BSP kick lived under `svm/`, which meant the development
// machine (AMD) and both target machines (Intel) ran DIFFERENT programs —
// months of hunting a fault on hardware the test machine could not
// reproduce. There is one data path and every machine runs it.
// `place_worker` never puts it on a vCPU's core; with the reservation on it
// marks a free core so the SIPI'd APs skip it.
```

## L1435-1439 · `if reserve_gpu_core() {`

```
// Off-vCPU GPU worker (Stage 2): on AMD with a spare core, claim a SECOND
// reserved core and run the ~8 MB/frame framebuffer copy + write_frame there,
// off the vCPU — so the browser's rendering never steals the net-servicing
// cycles (the 166 ms-loaded-latency root). guest_vcpus() already left this
// core free. Gated on FULL_GPU_BACKEND + AMD + ≥4 cores; else GPU stays inline.
```

## L1456-1459 · `VCPU_COUNT.store(1, Ordering::Release);`

```
// Guest-SMP: this is the BSP (owner). Seed the live-vCPU
// count and publish our shared state's address so a
// later AP fiber can alias it (the SIPI → Core-0 spawn
// path reads AP_SHARED_PTR).
```

## L1469-1471 · `unsafe { core::arch::asm!("sti") };`

```
// IF=1 only for the slice (host tick servicing between
// VMRUNs); CLGI/STGI still bracket the VMRUN inside.
// SAFETY: ring-0; mirrors the dedicated path's `sti`.
```

## L1474-1476 · `unsafe { core::arch::asm!("cli") };`

```
// Back to IF=0 before yielding so peer fibers run under
// the cooperative IF=0 invariant.
// SAFETY: ring-0.
```

## L1479 · `Ok(svm::SliceOutcome::StillRunning) => {`

```
// Busy guest: let peers take a turn, resume next pass.
```

## L1484-1485 · `Ok(svm::SliceOutcome::Idle) => {`

```
// Idle guest: park briefly → core runs app fibers.
// Event-driven on host RX IRQ while downloading.
```

## L1504-1506 · `VM_CLOSE_REQUESTED.store(true, Ordering::Release);`

```
// Last-one-out: the BSP owns the shared box. If an AP is
// still running, signal it down and wait for it to stop
// touching the shared state before we free it (close()).
```

## L1521-1527 · `match vmx::vm_open(`

```
// VMX guest as a fiber (Intel parity, #3) + guest-SMP BSP (#4).
// `vmx::vm_open` installs this worker's per-core TSS so VMX
// host-state is valid off Core 0. Same IF/yield discipline as the
// AMD arm; mirrors `vm_core_serve`'s Intel arm but yields the core
// between slices. Owns the shared `VmShared`: seeds the live-vCPU
// count + publishes its address so AP fibers can alias it, and
// last-one-out teardown waits for APs before `close()`.
```

## L1544 · `unsafe { core::arch::asm!("sti") };`

```
// SAFETY: ring-0; host tick serviced between VMRESUMEs.
```

## L1547-1548 · `unsafe { core::arch::asm!("cli") };`

```
// SAFETY: ring-0; back to IF=0 before yielding so peer
// fibers run under the cooperative IF=0 invariant.
```

## L1571-1572 · `VM_CLOSE_REQUESTED.store(true, Ordering::Release);`

```
// Last-one-out: the BSP owns the shared box. If an AP is
// still running, signal down + wait before freeing (close).
```

## L1589-1590 · `crate::microvm::devices::net_dataplane::stop_worker();`

```
// Stop the RX producer (also covers an open-FAILED path where vm_poll_slice
// teardown might not run). Idempotent with the vm_poll_slice stop sites.
```

## L1594 · `drop(pending); // owned guest-image buffers freed`

```
// owned guest-image buffers freed
```

## L1596 · `VM_RUN_STATE.store(VM_EXITED, Ordering::Release);`

```
// Hand off to Core 0's reaper (compositor-locking teardown).
```

## L1601-1609 · `fn ap_vcpu_fiber_task(arg: u64) {`

```
/// AP (secondary) vCPU fiber (guest SMP). Spawned by the Core-0 reaper after
/// the guest's SIPI; `arg` is the AP's apic_id (1..). Aliases the BSP's
/// `VmShared` (it does NOT open guest RAM / EPT|NPT / devices). Runs its own
/// VMRUN/VMRESUME loop on whatever worker core picks it up, in parallel with
/// the BSP. Decrements `VCPU_COUNT` on exit so the BSP's last-one-out teardown
/// can proceed.
///
/// Vendor is resolved via `detect_vendor` (lock-free CPUID). On Intel the AP must `close_ap` (VMXOFF on its
/// own core); on AMD it just stops VMRUNning (the BSP owns teardown).
```

## L1634 · `unsafe { core::arch::asm!("sti") };`

```
// SAFETY: ring-0; same IF discipline as the BSP fiber.
```

## L1637 · `unsafe { core::arch::asm!("cli") };`

```
// SAFETY: ring-0.
```

## L1658-1659 · `ctx.close_ap();`

```
// VMX: the AP entered VMX root on this core → VMXOFF + free its
// own VMXON/VMCS (but NOT the shared state — the BSP owns it).
```

## L1669 · `unsafe { core::arch::asm!("sti") };`

```
// SAFETY: ring-0; same IF discipline as the BSP fiber.
```

## L1672 · `unsafe { core::arch::asm!("cli") };`

```
// SAFETY: ring-0.
```

## L1696 · `},`

```
// AMD: the AP does NOT close() — the BSP owns + frees the box.
```

## L1704 · `VCPU_COUNT.fetch_sub(1, Ordering::AcqRel);`

```
// Let the BSP's last-one-out teardown proceed.
```

## L1708-1711 · `pub fn decode_io_exit_qualification(qual: u64) -> (u16, bool, u8) {`

```
/// Decode the I/O VM-exit qualification field from a substrate-test
/// `LaunchOutcome.exit_qualification`. Currently vendor-agnostic by
/// dispatch — only Intel populates I/O exits today; the AMD VMCB
/// EXITINFO1/2 layout will be plumbed through here when SVM lands.
```

## L1715-1716 · `Vendor::Amd | Vendor::Unknown(_) => (0, false, 0),`

```
// AMD VMCB exitinfo1 layout differs (port in bits 16-31,
// type in bit 0); plumb in svm:: when backend lands.
```

## L1721-1732 · `pub use vmx::LaunchOutcome;`

```
/// Outcome of one VM-entry/exit cycle.
///
/// The numeric fields are vendor-specific in their meaning:
///   * Intel: `exit_reason` is the Intel basic exit reason
///     (SDM Vol. 3C App. C); `exit_qualification` is VMCS field
///     `VM_EXIT_QUALIFICATION`.
///   * AMD (future): `exit_reason` will be the VMCB EXITCODE;
///     `exit_qualification` will be a packed EXITINFO1/EXITINFO2.
///
/// Callers that decode reason values must currently dispatch on
/// `current_vendor()`. Once both backends ship we'll consider
/// hoisting a vendor-agnostic `ExitReason` enum here.
```

