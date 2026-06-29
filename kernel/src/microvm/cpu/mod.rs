//! CPU virtualization extensions — vendor dispatch.
//!
//! Detects the host CPU vendor at boot via CPUID leaf 0 (vendor
//! string) and dispatches MicroVM operations to the matching
//! backend:
//!
//!   * `vmx` — Intel VT-x (VMCS, EPT)
//!   * `svm` — AMD-V (VMCB, NPT)  — stub, returns Err for now
//!
//! Public API (`init`, `report`, `run_substrate_test`, `run_linux`,
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

pub mod rip_sample;
pub mod svm;
pub mod vmx;

use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicU64, Ordering};

// ── VM-exit reason histogram (diagnosis) ───────────────────────────
//
// Why is the dedicated VM core busy? Both backends bump a bucket per
// guest exit so `cores` can show the mix while a VM runs: lots of `mmio`
// = the guest is rendering (legit busy); `hlt`/`intr` dominating = idle
// spin; etc. Backend-agnostic categories — each backend maps its own
// exit codes onto these.
pub const VMEXIT_BUCKETS: usize = 7;
pub const VMX_INTR: usize = 0;  // ext-interrupt / timer
pub const VMX_HLT: usize = 1;
pub const VMX_MMIO: usize = 2;  // NPF / EPT-violation (incl. virtio MMIO)
pub const VMX_IO: usize = 3;
pub const VMX_MSR: usize = 4;
pub const VMX_CPUID: usize = 5;
pub const VMX_OTHER: usize = 6;
pub const VMEXIT_LABELS: [&str; VMEXIT_BUCKETS] =
    ["intr", "hlt", "mmio", "io", "msr", "cpuid", "other"];

static VM_EXIT_COUNTS: [AtomicU64; VMEXIT_BUCKETS] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; VMEXIT_BUCKETS]
};

/// Bump the exit-reason bucket for one guest exit.
pub fn record_vm_exit(bucket: usize) {
    if bucket < VMEXIT_BUCKETS {
        VM_EXIT_COUNTS[bucket].fetch_add(1, Ordering::Relaxed);
    }
}

/// Snapshot all VM-exit buckets (double-sample to get a rate).
pub fn vm_exit_snapshot() -> [u64; VMEXIT_BUCKETS] {
    let mut out = [0u64; VMEXIT_BUCKETS];
    for i in 0..VMEXIT_BUCKETS {
        out[i] = VM_EXIT_COUNTS[i].load(Ordering::Relaxed);
    }
    out
}

// ── Per-port I/O exit breakdown ────────────────────────────────────
// The `io` exit bucket is dominated, during heavy guest RX, by the PIC
// EOI (`outb 0x20`): the guest runs `noapic`, so every device IRQ is
// ack'd through the 8259, and each ack is its own port-I/O VM-exit.
// Bucketing the ports proves where an io-exit storm actually comes from.
pub const IO_PORT_BUCKETS: usize = 7;
pub const IO_PORT_LABELS: [&str; IO_PORT_BUCKETS] =
    ["pic", "pit", "serial", "pci", "rtc", "kbd", "other"];
static IO_PORT_COUNTS: [AtomicU64; IO_PORT_BUCKETS] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; IO_PORT_BUCKETS]
};
fn io_port_bucket(port: u16) -> usize {
    match port {
        0x20 | 0x21 | 0xA0 | 0xA1 => 0,     // 8259 PIC (EOI / mask)
        0x40..=0x43 | 0x61 => 1,            // PIT
        0x2F8..=0x2FF | 0x3F8..=0x3FF => 2, // serial (COM1/COM2)
        0xCF8..=0xCFF => 3,                 // PCI config space
        0x70 | 0x71 => 4,                   // RTC / CMOS
        0x60 | 0x64 => 5,                   // i8042 keyboard
        _ => 6,                             // other
    }
}
/// Bucket one guest port-I/O exit by port (call from the IOIO handler).
pub fn record_io_port(port: u16) {
    IO_PORT_COUNTS[io_port_bucket(port)].fetch_add(1, Ordering::Relaxed);
}
/// Snapshot the per-port I/O buckets (double-sample for a rate).
pub fn io_port_snapshot() -> [u64; IO_PORT_BUCKETS] {
    let mut out = [0u64; IO_PORT_BUCKETS];
    for i in 0..IO_PORT_BUCKETS {
        out[i] = IO_PORT_COUNTS[i].load(Ordering::Relaxed);
    }
    out
}

// ── Host-time profiler ─────────────────────────────────────────────
// Where do the dedicated guest cores' cycles actually go? Counts (above)
// say HOW OFTEN we exit; these say HOW LONG each kind of handling costs vs
// time spent running the guest (VMRESUME). Proves whether the host wastes
// the core in mmio-decode / io-PIC handling (→ guest starved, "0% CPU"
// because it never gets the time) or genuinely runs the guest.
static VM_EXIT_CYCLES: [AtomicU64; VMEXIT_BUCKETS] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; VMEXIT_BUCKETS]
};
static VM_GUEST_CYCLES: AtomicU64 = AtomicU64::new(0);

/// TSC cycles spent IN the handler for `bucket` (between the exit and the
/// next VMRESUME).
pub fn record_exit_cycles(bucket: usize, cycles: u64) {
    if bucket < VMEXIT_BUCKETS {
        VM_EXIT_CYCLES[bucket].fetch_add(cycles, Ordering::Relaxed);
    }
}
/// TSC cycles spent inside VMRESUME (the guest actually executing).
pub fn record_guest_cycles(cycles: u64) {
    VM_GUEST_CYCLES.fetch_add(cycles, Ordering::Relaxed);
}
/// `(per-bucket handler cycles, total guest cycles)` for a rate snapshot.
pub fn vm_cycle_snapshot() -> ([u64; VMEXIT_BUCKETS], u64) {
    let mut out = [0u64; VMEXIT_BUCKETS];
    for i in 0..VMEXIT_BUCKETS {
        out[i] = VM_EXIT_CYCLES[i].load(Ordering::Relaxed);
    }
    (out, VM_GUEST_CYCLES.load(Ordering::Relaxed))
}

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

/// 64-byte-aligned XSAVE area. 4 KiB ≫ the ~2.4 KiB the host XCR0
/// (incl AVX-512) needs (CPUID.0xD.0:EBX). Zeroed = XSTATE_BV/XCOMP_BV
/// 0 → `xrstor64` loads the architectural FPU *init* state (x87 init,
/// MXCSR 0x1F80, vectors zeroed) — the correct fresh-guest FPU.
#[repr(C, align(64))]
pub(crate) struct FpuArea(pub(crate) [u8; 4096]);

impl FpuArea {
    pub(crate) fn boxed() -> alloc::boxed::Box<FpuArea> {
        alloc::boxed::Box::new(FpuArea([0u8; 4096]))
    }
}

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

/// `xsave64 [area]`, all XCR0-enabled components (EDX:EAX = -1).
#[inline(never)]
pub(crate) unsafe fn fpu_xsave(area: *mut FpuArea) {
    // SAFETY: CR4.OSXSAVE=1 on every core (trampoline.s — blake3 AVX2
    // runs AP-side); area is valid + 64-aligned.
    unsafe {
        core::arch::asm!("xsave64 [{p}]", p = in(reg) area,
            in("eax") 0xffff_ffffu32, in("edx") 0xffff_ffffu32,
            options(nostack));
    }
}

/// `xrstor64 [area]`, all XCR0-enabled components. Zeroed area =
/// XSTATE_BV/XCOMP_BV 0 → architectural FPU init state (fresh guest).
#[inline(never)]
pub(crate) unsafe fn fpu_xrstor(area: *const FpuArea) {
    // SAFETY: see fpu_xsave; area is a valid XSAVE image or zeroed.
    unsafe {
        core::arch::asm!("xrstor64 [{p}]", p = in(reg) area,
            in("eax") 0xffff_ffffu32, in("edx") 0xffff_ffffu32,
            options(nostack, readonly));
    }
}

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
pub fn choose_guest_ram_bytes() -> u64 {
    const RESERVE_MB: usize = 256;
    const MIN_MB: usize = 256;
    let cap_mb =
        (crate::microvm::devices::guest_mem::GUEST_RAM_BYTES / (1024 * 1024)) as usize;

    let (_free_frames, free_mb) = crate::mm::memory::stats();
    let mb = free_mb
        .saturating_sub(RESERVE_MB)
        .max(MIN_MB)
        .min(cap_mb);

    // Floor to 2 MB so it maps as whole EPT/NPT 2-MB leaves and the
    // frame count is a clean multiple of 512.
    const TWO_MB: u64 = 2 * 1024 * 1024;
    ((mb as u64) * 1024 * 1024) & !(TWO_MB - 1)
}

/// Host CPU vendor identified at boot from CPUID leaf 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Intel,
    Amd,
    /// CPUID returned a string we don't recognize. MicroVM stays
    /// disabled. The variant carries a short reason for `report()`.
    Unknown(&'static str),
}

static VENDOR: Mutex<Vendor> = Mutex::new(Vendor::Unknown("not detected yet"));

/// Identify the CPU via CPUID leaf 0 vendor string. Three known
/// strings: `GenuineIntel` (Intel), `AuthenticAMD` (AMD), anything
/// else returns `Unknown` with the raw bytes lost.
///
/// Standalone (no kernel state needed): safe to call from boot
/// init paths that run BEFORE `microvm::cpu::init()` has set the
/// cached `VENDOR` static. `smp::per_core::init_dedicated_vm_core`
/// uses this to vendor-gate A2 without writing its own CPUID
/// inline asm (the v0.172.62 attempt hung AMD QEMU).
pub fn detect_vendor() -> Vendor {
    let (_, ebx, ecx, edx) = vmx::host_cpuid(0, 0);
    // Vendor string is ebx, edx, ecx (yes, that order — Intel SDM
    // Vol. 2A §3.3 "CPUID Vendor String").
    let bytes = [
        (ebx & 0xFF) as u8, ((ebx >> 8) & 0xFF) as u8, ((ebx >> 16) & 0xFF) as u8, ((ebx >> 24) & 0xFF) as u8,
        (edx & 0xFF) as u8, ((edx >> 8) & 0xFF) as u8, ((edx >> 16) & 0xFF) as u8, ((edx >> 24) & 0xFF) as u8,
        (ecx & 0xFF) as u8, ((ecx >> 8) & 0xFF) as u8, ((ecx >> 16) & 0xFF) as u8, ((ecx >> 24) & 0xFF) as u8,
    ];
    match &bytes {
        b"GenuineIntel" => Vendor::Intel,
        b"AuthenticAMD" => Vendor::Amd,
        _ => Vendor::Unknown("CPUID vendor string not Intel/AMD"),
    }
}

#[allow(dead_code)] // public surface for future vendor-aware decoders
pub fn current_vendor() -> Vendor {
    *VENDOR.lock()
}

/// Boot-time entry: detect vendor, run vendor-specific probe.
pub fn init() {
    let v = detect_vendor();
    *VENDOR.lock() = v;
    match v {
        Vendor::Intel => vmx::init(),
        Vendor::Amd => svm::init(),
        Vendor::Unknown(reason) => {
            use crate::kprintln;
            kprintln!("[microvm] CPU vendor unknown ({}) — MicroVM disabled", reason);
        }
    }
}

/// Print vendor-specific virt capability snapshot.
pub fn report() {
    match *VENDOR.lock() {
        Vendor::Intel => vmx::report(),
        Vendor::Amd => svm::report(),
        Vendor::Unknown(reason) => {
            use crate::kprintln;
            kprintln!("[microvm] no virt extensions: {}", reason);
        }
    }
}

/// Run the vendor-specific substrate test (`microvm test`).
pub fn run_substrate_test() -> Result<LaunchOutcome, &'static str> {
    match *VENDOR.lock() {
        Vendor::Intel => vmx::run_substrate_test(),
        Vendor::Amd => svm::run_substrate_test(),
        Vendor::Unknown(reason) => Err(reason),
    }
}

/// Boot a Linux bzImage in the MicroVM (`microvm linux`).
pub fn run_linux(
    bzimage: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
    inject: &[u8],
) -> Result<LaunchOutcome, &'static str> {
    match *VENDOR.lock() {
        Vendor::Intel => vmx::run_linux(bzimage, cmdline, initramfs, inject),
        Vendor::Amd => svm::run_linux(bzimage, cmdline, initramfs, inject),
        Vendor::Unknown(reason) => Err(reason),
    }
}

// ── Re-entrant active VM (12.4 step 1b — Core-0 cooperative) ───────
//
// One backend-agnostic active VM, driven by the Core-0 event loop
// via `vm_poll_slice()` instead of a blocking `run_linux`. Holds the
// VmContext so Shade keeps rendering between bounded slices. Single
// global (one VM for now); keyed-registry generalisation deferred per
// the forward-compat contract (consumer side never assumes count).
// Core-0-only access in practice; the Mutex guards against misuse.

enum ActiveVm {
    Vmx(vmx::VmContext),
    Svm(svm::VmContext),
}

static ACTIVE_VM: Mutex<Option<ActiveVm>> = Mutex::new(None);

/// Shade window the active VM's framebuffer is bound to (0 = none).
/// virtio-gpu FLUSH reads this to know which surface to write; the
/// teardown path closes it. One VM ↔ one window for now; keyed by id
/// so it generalises (forward-compat #2).
static ACTIVE_VM_WINDOW: AtomicU32 = AtomicU32::new(0);

/// Set when the user closes the VM's window so the next slice tears
/// the guest down instead of running it headless.
static VM_CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Cross-boundary "open my files in loft" trigger: the guest's browser
/// opens the magic 9p file `<root>/.open-in-loft`, the 9p server (on the
/// VM core) sets this, and Core 0 reaps it in `vm_poll_slice` to spawn
/// loft (a compositor op that MUST run on Core 0). Mirrors the
/// VM_CLOSE_REQUESTED cross-core handoff.
static OPEN_LOFT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request that the host open loft. Safe to call from the VM core; the
/// spawn itself happens on Core 0 (`vm_poll_slice`).
pub fn request_open_loft() {
    OPEN_LOFT_REQUESTED.store(true, Ordering::Release);
}

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

const VM_IDLE: u8 = 0;
const VM_REQUESTED: u8 = 1;
const VM_RUNNING: u8 = 2;
const VM_EXITED: u8 = 3;

/// Dedicated-core VM lifecycle. Only meaningful when a core is
/// dedicated (cooperative path uses `ACTIVE_VM` instead). Also reused by
/// the fiber path (REQUESTED → RUNNING → EXITED).
static VM_RUN_STATE: AtomicU8 = AtomicU8::new(VM_IDLE);

// ── vCPU-as-fiber (unified core pool, Stage: SCHEDULER_FIBERS.md) ───────
//
// Instead of statically carving a core for the guest at boot, run the
// guest's VMRESUME loop as a normal pool FIBER (smp::fiber): admitted when
// a launch is requested, pinned to whatever worker core picks it up (so the
// VMX/SVM core-binding holds — no migration), yielding the core to peer app
// fibers on guest-idle, and freed on guest exit. No wasted core when no VM
// runs; the guest naturally dominates its core while busy. Multi-vCPU later
// = N such fibers. Flag-gated so a bad release reverts to the validated
// dedicated path via OTA (no reinstall): flip to `false` + re-release.
pub const VCPU_AS_FIBER: bool = true;

/// Intel parity: run the VMX guest as a pool fiber too (not just AMD/SVM),
/// so the browser leaves the cooperative Core-0 path (which shares Core 0
/// with Shade/input/cursor and made the guest — and the Core-0 PS/2 mouse
/// poll — unusably slow under load). Single-vCPU only (guest-SMP AP bring-
/// up stays SVM-only). Requires per-core TSS on the worker (`tss::ensure_
/// core`, done in `vmx::vm_open`). Flag-gated for clean OTA rollback: flip
/// to `false` + re-release → Intel reverts to cooperative Core 0, AMD
/// unaffected (its fiber mode keys off vendor, not this flag).
pub const VMX_VCPU_AS_FIBER: bool = true;

/// Guest-SMP Stage 1: trap-and-emulate the guest local APIC
/// (`svm::lapic`) instead of booting `nolapic`. When true, the guest
/// cmdline omits `nolapic` so Linux brings up the LAPIC + uses its timer;
/// the LAPIC MMIO page NPT-faults into the emulator. Flag-gated so a bad
/// release reverts via OTA (flip to `false` + re-release → `nolapic`
/// back → the UP guest boots with the LAPIC disabled, emulator inert —
/// no guest-kernel rebuild needed). Prerequisite for AP bringup (Stage 3).
pub const GUEST_LAPIC: bool = true;

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
pub const GUEST_SMP: bool = true;

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
pub fn guest_vcpus() -> u8 {
    let mut workers = crate::smp::per_core::core_count().saturating_sub(1);
    // EXPERIMENT (RESERVE_OFFLOAD_CORE): when the off-vCPU net backend runs it
    // needs its OWN worker core. Otherwise pick_offload_core() falls through to
    // `ncores-1` and co-locates the net worker with a vCPU. That vCPU gets
    // preempted by the worker, so it answers cross-vCPU TLB-shootdown IPIs late
    // → the other vCPUs spin in csd_lock_wait (~40%). Leaving one worker core
    // free maps each vCPU 1:1 to a core (like nested-Linux/iperf → 1 Gbit), so
    // IPIs are answered promptly. Flag-gated for clean before/after measurement.
    if reserve_offload_core() {
        workers = workers.saturating_sub(1);
    }
    workers.clamp(1, MAX_VCPUS_CAP) as u8
}

/// EXPERIMENT toggle: reserve a dedicated worker core for the off-vCPU net
/// backend fiber (see `guest_vcpus`). Flip to `false` + re-release to revert to
/// the co-located-worker behavior (one more vCPU, but csd_lock_wait contention).
pub const RESERVE_OFFLOAD_CORE: bool = true;

/// True when the off-vCPU net worker will actually claim a core this run (full
/// RX+TX backend, AMD/SVM only today) AND the reservation experiment is on AND
/// there is a core to spare. Mirrors the `full_backend` gate at `start_worker`.
fn reserve_offload_core() -> bool {
    RESERVE_OFFLOAD_CORE
        && crate::microvm::devices::net_backend::FULL_RX_BACKEND
        // detect_vendor() (raw CPUID, lock-free) — NOT current_vendor(): the
        // latter takes VENDOR.lock(), but guest_vcpus() is called from the
        // MP-table build + SIPI loop INSIDE `match *VENDOR.lock()` (the whole
        // guest run holds it) → reentrant lock = deadlock (hang right after the
        // net-worker spawn). CPUID is the same on every core, so this is sound.
        && matches!(detect_vendor(), Vendor::Amd)
        // Need ≥3 cores: Core 0 (BSP) + ≥1 vCPU + 1 worker. Below that, fall
        // back to co-location rather than starving the guest to a single vCPU.
        && crate::smp::per_core::core_count() >= 3
}

/// Hard cap on guest vCPUs (sizes the per-backend IPI bitmaps + the spawn
/// bitmask). 8 covers an 8-thread notebook fully and is plenty for a browser;
/// a 16/32-core desktop caps here rather than spawning a vCPU per core (idle
/// vCPUs each carry a small wake overhead). Must be ≤ each backend's
/// `MAX_VCPUS` and ≤ 32 (the `u32` spawn bitmask).
pub const MAX_VCPUS_CAP: usize = 8;

/// Guest-SMP Stage 3b-2: actually BRING UP the AP vCPU. When true the boot
/// cmdline raises `maxcpus` to `GUEST_VCPUS`, the guest's INIT-SIPI spawns
/// a second vCPU fiber sharing the BSP's `VmShared` (svm only), and the
/// cross-vCPU IPI path is exercised. Default FALSE: shipping is byte-
/// identical (no spawn, `maxcpus=1`, big-lock never taken) and flipping it
/// on is the AP test — a bad release reverts via OTA by flipping back to
/// false + re-release (clean rollback, no reinstall). Requires `GUEST_SMP`.
pub const GUEST_SMP_AP: bool = true;

/// Whether guest-SMP AP bring-up is enabled AND supported on this host. Both
/// backends now have an AP-vCPU open path (`svm::vm_open_ap` / `vmx::vm_open_ap`)
/// + SIPI/LAPIC routing, so this gates the guest's `maxcpus` (and the AP spawn)
/// on AMD and Intel alike. Unknown vendor stays single-vCPU.
pub fn smp_ap_active() -> bool {
    GUEST_SMP_AP && matches!(current_vendor(), Vendor::Amd | Vendor::Intel)
}

/// Set/clear the active backend's guest-SMP big-VM-lock engagement. Dispatches
/// via `detect_vendor` (lock-free CPUID) NOT `current_vendor()` (which locks
/// VENDOR) — the BSP vCPU fiber holds the VENDOR lock for its whole run loop,
/// so the last-one-out call from inside that arm must not re-lock it.
fn vm_set_ap_active(on: bool) {
    match detect_vendor() {
        Vendor::Intel => vmx::set_ap_active(on),
        Vendor::Amd => svm::set_ap_active(on),
        Vendor::Unknown(_) => {}
    }
}

/// Whether the guest's local APIC is emulated on this host. LAPIC emulation
/// (`svm/lapic.rs`) is SVM-only — there is no VMX equivalent yet. With
/// `GUEST_LAPIC` on but no emulation, an Intel guest would program the LAPIC
/// (TSC-deadline) timer and then never receive a tick → its event loops (cage/
/// Wayland, schedulers) hang forever. So on Intel we must keep `nolapic` in the
/// cmdline and let the guest fall back to the PIT IRQ0 the VMX path injects.
pub fn guest_lapic_active() -> bool {
    GUEST_LAPIC
        && match current_vendor() {
            Vendor::Amd => true,
            Vendor::Intel => VMX_GUEST_LAPIC,
            Vendor::Unknown(_) => false,
        }
}

/// Intel parity #2: emulate the guest local APIC on VMX too (`vmx::lapic`
/// reuses the pure `svm::lapic::LocalApic`). When true the Intel guest boots
/// WITHOUT `nolapic` and the LAPIC MMIO page EPT-faults into the emulator;
/// when false it keeps `nolapic` (byte-identical to the validated pre-LAPIC
/// Intel boot). Flag-gated for clean OTA rollback. Prerequisite for VMX
/// guest-SMP (#4) — Linux needs the per-CPU LAPIC timer to schedule APs.
pub const VMX_GUEST_LAPIC: bool = true;

// ── AP (secondary vCPU) spawn orchestration (guest SMP, N vCPUs) ────────
//
// The guest's INIT-SIPI is decoded on a vCPU fiber (a worker core), which
// CANNOT push a fiber itself (the run-queue deque is single-producer, owned by
// Core 0). So the SIPI handler only records a request here (per target
// apic_id); the Core-0 reaper (`vm_poll_slice`) does the actual `spawn_fiber`
// for each newly-requested AP. N-vCPU-ready: each AP is tracked by its apic_id
// bit, so a guest with several APs brings them all up.

/// Largest apic_id + 1 the orchestration tracks. Matches the per-backend
/// `MAX_VCPUS` (vmx + svm) that size the IPI bitmaps + `MAX_VCPUS_CAP`.
const ORCH_MAX_VCPUS: usize = 8;

/// Bitmask of apic_ids the guest has SIPI'd (1 << apic_id). The reaper spawns
/// a fiber for each set bit not yet in `AP_SPAWNED`.
static AP_SPAWN_REQUESTED: AtomicU32 = AtomicU32::new(0);
/// Bitmask of apic_ids already spawned (the reaper's idempotence guard:
/// absorbs the retried 2nd SIPI per AP).
static AP_SPAWNED: AtomicU32 = AtomicU32::new(0);
/// SIPI start vector per apic_id (Linux uses one trampoline page, but track
/// per-id to stay correct if that ever changes).
static AP_SIPI_VECTORS: [AtomicU8; ORCH_MAX_VCPUS] =
    { const Z: AtomicU8 = AtomicU8::new(0); [Z; ORCH_MAX_VCPUS] };
/// The BSP's `*mut VmShared` (as u64), published once the BSP opens, for an
/// AP fiber to alias. 0 = not yet published.
static AP_SHARED_PTR: AtomicU64 = AtomicU64::new(0);
/// Live vCPU count for this VM. The BSP sets it to 1 at open; the reaper bumps
/// it per AP it spawns; each fiber decrements on exit. The BSP (owner) waits
/// for it to return to 1 before `close()` so no AP touches freed shared state
/// (last-one-out).
static VCPU_COUNT: AtomicU32 = AtomicU32::new(0);

/// Bitmask of host cores currently running a vCPU fiber (bit c). Core 0 (bit 0)
/// is always reserved (shell/reaper). Each vCPU MUST get a DISTINCT core: VMX
/// root is per-physical-core, so two vCPUs VMXONing one core fails with
/// VMfailValid (the N>2 bug); on SVM it just starves. The reaper places each AP
/// on the lowest free worker core via `reserve_ap_core` + `fiber::admit`
/// instead of `spawn_fiber` (whose work-stealing piles every fiber onto the one
/// awake core). Reset at BSP open + teardown.
static VM_CORE_MASK: AtomicU32 = AtomicU32::new(1); // bit 0 = Core 0 reserved

/// Pick the least-contended core for an async-offload worker (e.g. the 9p
/// persist fiber). NEVER Core 0 — it is the keep-alive minimum (shell,
/// compositor) and must not compete. Prefers a core with no vCPU on it (not in
/// `VM_CORE_MASK`); if every worker core runs a vCPU, returns the highest one
/// (it shares, but still off Core 0). Single-core hosts return 0 (no choice).
/// v1: chosen at spawn. v2 (future): the worker migrates to follow load/heat.
pub fn pick_offload_core() -> usize {
    let ncores = crate::smp::per_core::core_count();
    if ncores <= 1 { return 0; }
    let mask = VM_CORE_MASK.load(Ordering::Acquire);
    // Prefer a free (no-vCPU) worker core.
    for c in 1..ncores {
        if mask & (1u32 << c) == 0 { return c; }
    }
    // All busy with vCPUs → share the highest worker core, never Core 0.
    ncores - 1
}

/// Like `pick_offload_core`, but MARKS the chosen core in `VM_CORE_MASK` so the
/// later AP placement (`reserve_ap_core`, fired by guest SIPIs) skips it — the
/// net worker gets a core to itself instead of being overwritten by a vCPU.
/// Used for the off-vCPU net backend when `reserve_offload_core()` is on
/// (`guest_vcpus()` already left one worker core free for exactly this). Falls
/// back to a shared core (no mark) if none is free. Single producer (the vCPU
/// fiber, before guest boot → before any SIPI), same CAS discipline as
/// `reserve_ap_core`.
fn claim_offload_core() -> usize {
    let ncores = crate::smp::per_core::core_count();
    if ncores <= 1 { return 0; }
    loop {
        let mask = VM_CORE_MASK.load(Ordering::Acquire);
        let Some(c) = (1..ncores).find(|c| mask & (1u32 << c) == 0) else {
            return ncores - 1; // all busy → co-locate on highest worker core
        };
        if VM_CORE_MASK
            .compare_exchange(mask, mask | (1u32 << c), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return c;
        }
    }
}

/// Reserve a distinct idle worker core (1..core_count()) for an AP vCPU fiber,
/// marking it taken. `None` if every worker core already runs a vCPU (the
/// caller then falls back to the shared deque — only safe on SVM; on VMX that
/// means more vCPUs than host cores, which `guest_vcpus()` avoids by capping at
/// the worker count). Called only from the Core-0 reaper (single producer).
fn reserve_ap_core() -> Option<usize> {
    let ncores = crate::smp::per_core::core_count();
    loop {
        let mask = VM_CORE_MASK.load(Ordering::Acquire);
        let c = (1..ncores).find(|c| mask & (1u32 << c) == 0)?;
        if VM_CORE_MASK
            .compare_exchange(mask, mask | (1u32 << c), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Some(c);
        }
    }
}

/// Record a guest SIPI (from a backend ICR router) → ask Core 0 to spawn the
/// AP with `apic_id` at `sipi_vector`. No-op unless guest-SMP AP bring-up is
/// enabled or apic_id is out of range. Idempotent: the reaper's `AP_SPAWNED`
/// guard ignores a re-SIPI of an already-spawned AP.
pub fn request_ap_spawn(apic_id: u8, sipi_vector: u8) {
    if !GUEST_SMP_AP || apic_id == 0 || apic_id as usize >= ORCH_MAX_VCPUS {
        return;
    }
    AP_SIPI_VECTORS[apic_id as usize].store(sipi_vector, Ordering::Release);
    AP_SPAWN_REQUESTED.fetch_or(1u32 << apic_id, Ordering::AcqRel);
}

/// The BSP publishes the address of its (heap-boxed) `VmShared` so a
/// spawned AP fiber can alias it.
pub fn publish_ap_shared(ptr: u64) {
    AP_SHARED_PTR.store(ptr, Ordering::Release);
}

/// Reset the AP-spawn orchestration statics after the last vCPU has exited
/// (BSP teardown), so a relaunch starts clean.
fn ap_orch_reset() {
    AP_SPAWNED.store(0, Ordering::Release);
    AP_SPAWN_REQUESTED.store(0, Ordering::Release);
    AP_SHARED_PTR.store(0, Ordering::Release);
    VM_CORE_MASK.store(1, Ordering::Release); // only Core 0 reserved
}

/// Decided once at boot (`set_vm_fiber_mode`, from `init_dedicated_vm_core`,
/// which has the vendor + worker count): true → the guest runs as a pool
/// fiber and NO core is dedicated. Cheap atomic read on the hot poll path.
static VM_FIBER_MODE: AtomicBool = AtomicBool::new(false);

/// Set the fiber-mode decision (called once from `init_dedicated_vm_core`).
pub fn set_vm_fiber_mode(on: bool) {
    VM_FIBER_MODE.store(on, Ordering::Release);
}

/// True if the guest runs as a pool fiber (vs the dedicated-core or the
/// cooperative-Core-0 path).
pub fn vm_fiber_mode() -> bool {
    VM_FIBER_MODE.load(Ordering::Acquire)
}

/// One pending launch request, owned copies so the caller's npkFS
/// buffers can drop while the dedicated core consumes them.
struct PendingVm {
    bzimage: alloc::vec::Vec<u8>,
    cmdline: alloc::vec::Vec<u8>,
    initramfs: Option<alloc::vec::Vec<u8>>,
    inject: alloc::vec::Vec<u8>,
}
static PENDING_VM: Mutex<Option<PendingVm>> = Mutex::new(None);

/// Bind the active VM to a Shade Surface window (called by the
/// microvm intent right after a successful `vm_open`).
pub fn vm_bind_window(window_id: u32) {
    ACTIVE_VM_WINDOW.store(window_id, Ordering::Release);
}

/// The Shade window the active VM renders into (0 = none/unbound).
pub fn vm_window() -> u32 {
    ACTIVE_VM_WINDOW.load(Ordering::Acquire)
}

/// Window closed by the user → ask the bound VM to power off on the
/// next slice. No-op if it isn't the active VM's window.
pub fn vm_close_for_window(window_id: u32) {
    if window_id != 0 && window_id == ACTIVE_VM_WINDOW.load(Ordering::Acquire) {
        VM_CLOSE_REQUESTED.store(true, Ordering::Release);
    }
}

/// The guest shut itself down (e.g. LibreWolf's own window-X → cage exits →
/// PID-1 `halt` → `reboot: System halted`). The serial scanner calls this so
/// the run loop takes the SAME clean exit as a user Mod+Q: break → `close()`
/// (saves the home image) → Core-0 reaper closes the window. Without it a
/// `cli;hlt`-halted guest just spins the run loop forever (black tile, no
/// save) until the user closes the host window manually.
pub fn note_guest_shutdown() {
    VM_CLOSE_REQUESTED.store(true, Ordering::Release);
}

/// Drop the VM↔window binding + its surface and close the Shade
/// window. MUST be called with the ACTIVE_VM lock NOT held (it locks
/// the compositor, whose close path re-enters microvm). Idempotent.
fn teardown_vm_window() {
    let wid = ACTIVE_VM_WINDOW.swap(0, Ordering::AcqRel);
    VM_CLOSE_REQUESTED.store(false, Ordering::Release);
    if wid != 0 {
        crate::shade::surface::remove_surface(wid);
        crate::shade::close_window(crate::shade::window::WindowId(wid));
    }
}

/// Exit-count cap per Core-0 poll, a secondary bound: `run_slice`
/// also enforces a ~3 ms wall-clock deadline (see vmx/svm
/// `SLICE_MS`), which is what actually keeps a busy guest from
/// starving Shade. Cheap boot exits hit this count first → same
/// boot wall-time; a busy compositor hits the deadline first.
const SLICE_BUDGET: u32 = 4096;

/// True if Core 0 should spin-feed a cooperative microvm. On the
/// dedicated path the guest runs on its own core, so Core 0 does NOT
/// spin — it idles/composites normally and reaps via `vm_poll_slice`.
/// Hence: false on the dedicated path (the guest is still composited
/// through the `focused_surface_id` branch, independent of this).
pub fn vm_active() -> bool {
    if vm_fiber_mode() || crate::smp::per_core::dedicated_vm_core().is_some() {
        return false;
    }
    ACTIVE_VM.lock().is_some()
}

/// Open a microvm and register it as the active VM. Non-blocking:
/// does the (synchronous, one-time) substrate + guest-image setup,
/// then returns — slices run later via `vm_poll_slice`. Errors if a
/// VM is already active.
pub fn vm_open(
    bzimage: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
    inject: &[u8],
) -> Result<(), &'static str> {
    // Fiber path: stash the request + spawn a vCPU fiber. A worker admits
    // it (smp_ap_entry → fiber::admit) and runs the guest ON that core for
    // its lifetime (VMXON/run/VMXOFF all bind there; the fiber is pinned).
    // Owned copies so the caller's npkFS Vecs can drop.
    if vm_fiber_mode() {
        if VM_RUN_STATE.load(Ordering::Acquire) != VM_IDLE {
            return Err("a microvm is already running");
        }
        *PENDING_VM.lock() = Some(PendingVm {
            bzimage: bzimage.to_vec(),
            cmdline: cmdline.to_vec(),
            initramfs: initramfs.map(<[u8]>::to_vec),
            inject: inject.to_vec(),
        });
        VM_CLOSE_REQUESTED.store(false, Ordering::Release);
        VM_RUN_STATE.store(VM_REQUESTED, Ordering::Release);
        crate::smp::scheduler::spawn_fiber(
            crate::smp::scheduler::Priority::Interactive,
            vcpu_fiber_task,
            0,
        );
        return Ok(());
    }

    // Dedicated path: hand the request to the VM core (which opens it
    // on itself). Owned copies so the caller's npkFS Vecs can drop.
    if crate::smp::per_core::dedicated_vm_core().is_some() {
        if VM_RUN_STATE.load(Ordering::Acquire) != VM_IDLE {
            return Err("a microvm is already running");
        }
        *PENDING_VM.lock() = Some(PendingVm {
            bzimage: bzimage.to_vec(),
            cmdline: cmdline.to_vec(),
            initramfs: initramfs.map(<[u8]>::to_vec),
            inject: inject.to_vec(),
        });
        VM_CLOSE_REQUESTED.store(false, Ordering::Release);
        VM_RUN_STATE.store(VM_REQUESTED, Ordering::Release);
        return Ok(());
    }

    let mut slot = ACTIVE_VM.lock();
    if slot.is_some() {
        return Err("a microvm is already running");
    }
    let vm = match *VENDOR.lock() {
        Vendor::Intel => ActiveVm::Vmx(vmx::vm_open(bzimage, cmdline, initramfs, inject)?),
        Vendor::Amd => ActiveVm::Svm(svm::vm_open(bzimage, cmdline, initramfs, inject)?),
        Vendor::Unknown(reason) => return Err(reason),
    };
    *slot = Some(vm);
    VM_CLOSE_REQUESTED.store(false, Ordering::Release);
    Ok(())
}

/// Run one bounded slice of the active VM, if any. Called from the
/// Core-0 poll cadence (next to `net::poll`). Cheap no-op when no VM.
/// On guest exit / fault: log, free resources, clear the slot so a
/// new VM can be opened (relaunch).
pub fn vm_poll_slice() {
    // Cross-boundary trigger: the microvm browser asked (via 9p) to open
    // loft. Spawn it here on Core 0 (compositor op). Runs on both the
    // cooperative and dedicated paths since it's before the early return.
    if OPEN_LOFT_REQUESTED.swap(false, Ordering::AcqRel) {
        crate::shade::launch_app("loft");
    }

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
    if GUEST_SMP_AP {
        let fresh = AP_SPAWN_REQUESTED.load(Ordering::Acquire)
            & !AP_SPAWNED.load(Ordering::Acquire);
        if fresh != 0 {
            for apic_id in 1..ORCH_MAX_VCPUS as u32 {
                if fresh & (1u32 << apic_id) == 0 {
                    continue;
                }
                // Claim it (idempotent against a re-SIPI / re-poll).
                if AP_SPAWNED.fetch_or(1u32 << apic_id, Ordering::AcqRel)
                    & (1u32 << apic_id)
                    != 0
                {
                    continue;
                }
                // AP_ACTIVE before the spawn so the BSP starts taking the
                // big-VM lock before the AP can run.
                VCPU_COUNT.fetch_add(1, Ordering::AcqRel);
                vm_set_ap_active(true);
                let vec = AP_SIPI_VECTORS[apic_id as usize].load(Ordering::Acquire);
                // Place the AP on a DISTINCT idle worker core (one vCPU per
                // core — VMX root is per-core). `fiber::admit` pushes straight
                // to that core's fiber queue (it wakes ≤10 ms later and runs
                // it) instead of `spawn_fiber`, whose work-stealing piled every
                // vCPU onto the one awake core → VMXON-VMfailValid on Intel.
                match reserve_ap_core() {
                    Some(c) => {
                        crate::kprintln!(
                            "[microvm] spawning AP vCPU fiber apic_id={} on core {} (sipi vec {:#x})",
                            apic_id, c, vec
                        );
                        crate::smp::fiber::admit(c, ap_vcpu_fiber_task, apic_id as u64);
                    }
                    None => {
                        // More vCPUs than host cores — share via the deque (SVM
                        // tolerates it; guest_vcpus() caps at the worker count
                        // so VMX shouldn't reach here).
                        crate::kprintln!(
                            "[microvm] AP apic_id={} — no free core, sharing via deque (sipi vec {:#x})",
                            apic_id, vec
                        );
                        crate::smp::scheduler::spawn_fiber(
                            crate::smp::scheduler::Priority::Interactive,
                            ap_vcpu_fiber_task,
                            apic_id as u64,
                        );
                    }
                }
            }
        }
    }

    if vm_fiber_mode() || crate::smp::per_core::dedicated_vm_core().is_some() {
        if VM_RUN_STATE.load(Ordering::Acquire) == VM_EXITED {
            crate::microvm::devices::net_rx_worker::stop_worker();
            crate::microvm::devices::p9_async::stop_worker();
            crate::microvm::devices::nat::reset_sessions();
            crate::microvm::cpu::rip_sample::reset();
            teardown_vm_window();
            VM_CLOSE_REQUESTED.store(false, Ordering::Release);
            VM_RUN_STATE.store(VM_IDLE, Ordering::Release);
            crate::kprintln!("[microvm] guest exited (dedicated core)");
        }
        return;
    }

    let mut slot = ACTIVE_VM.lock();
    if slot.is_none() {
        return;
    }

    // User closed the VM's window → force the guest down this tick
    // instead of running it headless until idle.
    if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
        match slot.as_mut() {
            Some(ActiveVm::Vmx(ctx)) => ctx.close(),
            Some(ActiveVm::Svm(ctx)) => ctx.close(),
            None => {}
        }
        *slot = None;
        drop(slot); // release before teardown — it locks the compositor
        crate::microvm::devices::net_rx_worker::stop_worker();
        crate::microvm::devices::p9_async::stop_worker();
        crate::microvm::devices::nat::reset_sessions();
        crate::microvm::cpu::rip_sample::reset();
        teardown_vm_window();
        crate::kprintln!("[microvm] guest stopped (window closed)");
        return;
    }

    let finished: Option<Result<LaunchOutcome, &'static str>> = match slot.as_mut() {
        None => return,
        // Cooperative Core-0 path: Idle == StillRunning here — just hand
        // Core 0 back to the shell, whose own idle HLT throttles the loop.
        Some(ActiveVm::Vmx(ctx)) => match ctx.run_slice(SLICE_BUDGET) {
            Ok(vmx::SliceOutcome::StillRunning) | Ok(vmx::SliceOutcome::Idle) => None,
            Ok(vmx::SliceOutcome::Exited(o)) => Some(Ok(o)),
            Err(e) => Some(Err(e)),
        },
        Some(ActiveVm::Svm(ctx)) => match ctx.run_slice(SLICE_BUDGET) {
            Ok(svm::SliceOutcome::StillRunning) | Ok(svm::SliceOutcome::Idle) => None,
            Ok(svm::SliceOutcome::Exited(o)) => Some(Ok(o)),
            Err(e) => Some(Err(e)),
        },
    };
    let Some(result) = finished else { return };
    match slot.as_mut() {
        Some(ActiveVm::Vmx(ctx)) => ctx.close(),
        Some(ActiveVm::Svm(ctx)) => ctx.close(),
        None => {}
    }
    *slot = None;
    drop(slot); // release before teardown — it locks the compositor
    crate::microvm::devices::net_rx_worker::stop_worker();
    crate::microvm::devices::p9_async::stop_worker();
    crate::microvm::devices::nat::reset_sessions();
    crate::microvm::cpu::rip_sample::reset();
    teardown_vm_window();
    match result {
        Ok(o) => crate::kprintln!(
            "[microvm] guest exited — final reason {:#x} qual {:#x}",
            (o.exit_reason & 0xFFFF) as u16, o.exit_qualification,
        ),
        Err(e) => crate::kprintln!("[microvm] launch FAILED: {}", e),
    }
}

/// Host-idle the dedicated core for ~one dedicated-VM-timer tick when
/// the guest is idle (SliceOutcome::Idle), instead of spinning VMRUN.
/// With cpu-pm dropped the HLT VMEXITs → KVM frees the host core. Records
/// the halt so `cores` reports the dedicated core's real idle (otherwise
/// it shows a misleading 100% busy with 0 HALTS while the guest idles).
#[inline]
fn idle_host_sleep() {
    let t0 = crate::interrupts::rdtsc();
    // SAFETY: ring-0; the dedicated VM timer (armed in vm_core_serve)
    // wakes us within ~1 ms.
    unsafe { core::arch::asm!("sti; hlt") };
    if let Some(c) = crate::smp::per_core::dedicated_vm_core() {
        crate::smp::per_core::record_halt(
            c, crate::interrupts::rdtsc().saturating_sub(t0));
        crate::smp::per_core::record_wake(c, crate::smp::per_core::WAKE_HLT_FALLBACK);
    }
}

/// Dedicated-core entry point — called every iteration of the
/// dedicated worker core's `smp_ap_entry` loop. Cheap no-op unless a
/// launch is pending. When one is, this opens the VM **on this core**
/// (so VMXON / `write_host_state` / VMPTRLD / VMRESUME / VMXOFF all
/// bind here, never Core 0), runs it to exit / window-close in a
/// continuous loop, closes it, and signals Core 0 to reap. Blocks the
/// dedicated core for the guest's whole lifetime — that is the point:
/// the guest no longer fights Shade + the shell for Core 0. Never
/// touches `ACTIVE_VM` (cooperative-path only).
pub fn vm_core_serve() {
    if VM_RUN_STATE.load(Ordering::Acquire) != VM_REQUESTED {
        return;
    }
    let pending = match PENDING_VM.lock().take() {
        Some(p) => p,
        None => {
            VM_RUN_STATE.store(VM_IDLE, Ordering::Release);
            return;
        }
    };
    VM_RUN_STATE.store(VM_RUNNING, Ordering::Release);

    // Per-core periodic VMEXIT source + host interrupts ON, so a
    // pending tick is delivered to the EOI handler between VMRUNs —
    // exactly how Core 0 runs the cooperative path. Without IF=1 the
    // first tick would never EOI and the guest would freeze one
    // iteration later; without the timer there is no tick at all.
    crate::interrupts::arm_dedicated_vm_timer();
    // SAFETY: ring-0; CLGI/STGI still brackets the VMRUN-critical
    // region inside run_guest_once. This mirrors Core 0's IF=1.
    unsafe { core::arch::asm!("sti") };

    // Continuous run: identical primitive to `vmx::run_linux` step 1a
    // (open + `loop run_slice` + close) plus a window-close check so
    // the user can `Mod+Q` the tile. `run_slice` returns periodically
    // on its wall-clock deadline; we loop straight back (no Shade
    // composite, no hlt on this core) → near-native guest. The two
    // backends have distinct `SliceOutcome` enums, so match each
    // concretely.
    match *VENDOR.lock() {
        Vendor::Intel => {
            match vmx::vm_open(
                &pending.bzimage,
                &pending.cmdline,
                pending.initramfs.as_deref(),
                &pending.inject,
            ) {
                Ok(mut ctx) => {
                    loop {
                        if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
                            crate::kprintln!("[microvm] window closed — stopping guest");
                            break;
                        }
                        match ctx.run_slice(SLICE_BUDGET) {
                            Ok(vmx::SliceOutcome::StillRunning) => continue,
                            Ok(vmx::SliceOutcome::Idle) => {
                                idle_host_sleep();
                                continue;
                            }
                            Ok(vmx::SliceOutcome::Exited(o)) => {
                                crate::kprintln!(
                                    "[microvm] guest exited — reason {:#x} qual {:#x}",
                                    (o.exit_reason & 0xFFFF) as u16,
                                    o.exit_qualification
                                );
                                break;
                            }
                            Err(e) => {
                                crate::kprintln!("[microvm] run FAILED: {}", e);
                                break;
                            }
                        }
                    }
                    ctx.close();
                }
                Err(e) => crate::kprintln!("[microvm] open FAILED: {}", e),
            }
        }
        Vendor::Amd => {
            match svm::vm_open(
                &pending.bzimage,
                &pending.cmdline,
                pending.initramfs.as_deref(),
                &pending.inject,
            ) {
                Ok(mut ctx) => {
                    loop {
                        if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
                            crate::kprintln!("[microvm] window closed — stopping guest");
                            break;
                        }
                        match ctx.run_slice(SLICE_BUDGET) {
                            Ok(svm::SliceOutcome::StillRunning) => continue,
                            Ok(svm::SliceOutcome::Idle) => {
                                idle_host_sleep();
                                continue;
                            }
                            Ok(svm::SliceOutcome::Exited(o)) => {
                                crate::kprintln!(
                                    "[microvm] guest exited — reason {:#x} qual {:#x}",
                                    (o.exit_reason & 0xFFFF) as u16,
                                    o.exit_qualification
                                );
                                break;
                            }
                            Err(e) => {
                                crate::kprintln!("[microvm] run FAILED: {}", e);
                                break;
                            }
                        }
                    }
                    ctx.close();
                }
                Err(e) => crate::kprintln!("[microvm] open FAILED: {}", e),
            }
        }
        Vendor::Unknown(reason) => crate::kprintln!("[microvm] {}", reason),
    }

    // VM done — swap the 1 kHz VM timer back to the 100 Hz worker idle
    // timer so this core's park-loop HLT keeps a wake source (it idles
    // like any worker until the next launch). Restore the IF=0 state
    // `smp_ap_entry`'s park loop expects (it does its own sti;hlt;cli).
    crate::interrupts::disarm_dedicated_vm_timer();
    crate::interrupts::arm_worker_timer();
    // SAFETY: ring-0; return the core to the parked-loop invariant.
    unsafe { core::arch::asm!("cli") };

    drop(pending); // owned guest-image buffers freed
    // Hand off to Core 0's reaper (compositor-locking teardown).
    VM_RUN_STATE.store(VM_EXITED, Ordering::Release);
}

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
fn park_vcpu_idle() {
    // (A bounded spin-while-recently-active test was REVERTED: it pegged a worker
    // core at 100% and starved Core 0's compositor → massive UI lag + latency
    // peaks. Confirms the "never an unbounded spin" rule — the consumer-wake must
    // be event-driven, not spun.)
    // When the dedicated RX producer is active it OWNS the host NIC RX IRQ
    // (routed to its own core). The BSP here is the CONSUMER — it injects what
    // the producer staged. It must NOT arm/route that IRQ (that would steal the
    // producer's event-wake). A short timer park suffices: the dedicated ~1 kHz
    // VM timer + the guest's own RX-repost/ACK MMIO exits drive the inject; with
    // a download in flight that's frequent, so a queued frame is injected within
    // ~1 ms instead of waiting on a blind 10 ms worker tick.
    if crate::microvm::devices::net_rx_worker::active() {
        crate::smp::fiber::yield_sleep(1);
        return;
    }
    if crate::microvm::devices::nat::recently_active() {
        let vec = crate::drivers::virtio_net::rx_irq_vector();
        if vec != 0 {
            let since = crate::irq::arm(vec); // snapshot + route IRQ to this core
            crate::smp::fiber::irq_wait(vec, since, 2);
            return;
        }
    }
    crate::smp::fiber::yield_sleep(2);
}

fn vcpu_fiber_task(_arg: u64) {
    if VM_RUN_STATE.load(Ordering::Acquire) != VM_REQUESTED {
        return;
    }
    let pending = match PENDING_VM.lock().take() {
        Some(p) => p,
        None => {
            VM_RUN_STATE.store(VM_IDLE, Ordering::Release);
            return;
        }
    };
    VM_RUN_STATE.store(VM_RUNNING, Ordering::Release);

    let cid = crate::smp::per_core::current_core_id();
    crate::kprintln!("[microvm] vCPU fiber opening guest on core {}", cid);

    // Guest SMP: (re)initialise the vCPU-core mask with the BSP's own core so
    // APs are placed on OTHER cores (one vCPU per distinct core). Fresh store,
    // not OR, so a relaunch starts clean. Runs before the guest boots → before
    // any SIPI → the reaper always sees the BSP bit.
    VM_CORE_MASK.store((1u32 << 0) | (1u32 << cid), Ordering::Release);

    // 1 kHz wake source on THIS core so the fiber's idle yields resume
    // promptly (the 100 Hz worker timer alone would stretch a 2 ms idle
    // yield to ~10 ms). Restored to the worker timer when the guest ends.
    crate::interrupts::arm_dedicated_vm_timer();

    // Spawn the dedicated host-NIC RX producer on another core (the Linux-NAPI
    // split): it drains the NIC + IP stack + GRO into INBOUND_Q, event-driven on
    // the RX IRQ, so THIS BSP vCPU only injects — the two halves pipeline instead
    // of serializing on one core. Stopped in this fiber's teardown + vm_poll_slice.
    // Full off-vCPU RX backend (Stage 2b) is SVM-only (the IRQ fold + BSP kick
    // live in svm::enable). `current_vendor()` is safe here — the BSP hasn't
    // taken the VENDOR lock for its run loop yet (that's the match below).
    let full_backend = crate::microvm::devices::net_backend::FULL_RX_BACKEND
        && matches!(current_vendor(), Vendor::Amd);
    // When the reservation experiment is on, guest_vcpus() left one worker core
    // free and claim_offload_core() marks it so the SIPI'd APs skip it → the net
    // worker runs on its OWN core (no vCPU co-location → no csd_lock_wait spin).
    let worker_core = if reserve_offload_core() {
        claim_offload_core()
    } else {
        pick_offload_core()
    };
    crate::kprintln!(
        "[microvm] net worker core {} (vCPUs={}, reserve={})",
        worker_core, guest_vcpus(), reserve_offload_core()
    );
    crate::microvm::devices::net_rx_worker::start_worker(worker_core, full_backend);

    match *VENDOR.lock() {
        Vendor::Amd => {
            match svm::vm_open(
                &pending.bzimage,
                &pending.cmdline,
                pending.initramfs.as_deref(),
                &pending.inject,
            ) {
                Ok(mut ctx) => {
                    // Guest-SMP: this is the BSP (owner). Seed the live-vCPU
                    // count and publish our shared state's address so a
                    // later AP fiber can alias it (the SIPI → Core-0 spawn
                    // path reads AP_SHARED_PTR).
                    VCPU_COUNT.store(1, Ordering::Release);
                    if GUEST_SMP_AP {
                        publish_ap_shared(ctx.shared_ptr() as u64);
                    }
                    loop {
                    if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
                        crate::kprintln!("[microvm] window closed — stopping guest");
                        break;
                    }
                    // IF=1 only for the slice (host tick servicing between
                    // VMRUNs); CLGI/STGI still bracket the VMRUN inside.
                    // SAFETY: ring-0; mirrors the dedicated path's `sti`.
                    unsafe { core::arch::asm!("sti") };
                    let outcome = ctx.run_slice(SLICE_BUDGET);
                    // Back to IF=0 before yielding so peer fibers run under
                    // the cooperative IF=0 invariant.
                    // SAFETY: ring-0.
                    unsafe { core::arch::asm!("cli") };
                    match outcome {
                        // Busy guest: let peers take a turn, resume next pass.
                        Ok(svm::SliceOutcome::StillRunning) => {
                            crate::smp::fiber::yield_ready();
                        }
                        // Idle guest: park briefly → core runs app fibers.
                        // Event-driven on host RX IRQ while downloading.
                        Ok(svm::SliceOutcome::Idle) => {
                            park_vcpu_idle();
                        }
                        Ok(svm::SliceOutcome::Exited(o)) => {
                            crate::kprintln!(
                                "[microvm] guest exited — reason {:#x} qual {:#x}",
                                (o.exit_reason & 0xFFFF) as u16,
                                o.exit_qualification
                            );
                            break;
                        }
                        Err(e) => {
                            crate::kprintln!("[microvm] run FAILED: {}", e);
                            break;
                        }
                    }
                    }
                    // Last-one-out: the BSP owns the shared box. If an AP is
                    // still running, signal it down and wait for it to stop
                    // touching the shared state before we free it (close()).
                    VM_CLOSE_REQUESTED.store(true, Ordering::Release);
                    while VCPU_COUNT.load(Ordering::Acquire) > 1 {
                        crate::smp::fiber::yield_sleep(2);
                    }
                    svm::set_ap_active(false);
                    ap_orch_reset();
                    ctx.close();
                    VCPU_COUNT.store(0, Ordering::Release);
                }
                Err(e) => crate::kprintln!("[microvm] open FAILED: {}", e),
            }
        }
        Vendor::Intel => {
            // VMX guest as a fiber (Intel parity, #3) + guest-SMP BSP (#4).
            // `vmx::vm_open` installs this worker's per-core TSS so VMX
            // host-state is valid off Core 0. Same IF/yield discipline as the
            // AMD arm; mirrors `vm_core_serve`'s Intel arm but yields the core
            // between slices. Owns the shared `VmShared`: seeds the live-vCPU
            // count + publishes its address so AP fibers can alias it, and
            // last-one-out teardown waits for APs before `close()`.
            match vmx::vm_open(
                &pending.bzimage,
                &pending.cmdline,
                pending.initramfs.as_deref(),
                &pending.inject,
            ) {
                Ok(mut ctx) => {
                    VCPU_COUNT.store(1, Ordering::Release);
                    if GUEST_SMP_AP {
                        publish_ap_shared(ctx.shared_ptr() as u64);
                    }
                    loop {
                        if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
                            crate::kprintln!("[microvm] window closed — stopping guest");
                            break;
                        }
                        // SAFETY: ring-0; host tick serviced between VMRESUMEs.
                        unsafe { core::arch::asm!("sti") };
                        let outcome = ctx.run_slice(SLICE_BUDGET);
                        // SAFETY: ring-0; back to IF=0 before yielding so peer
                        // fibers run under the cooperative IF=0 invariant.
                        unsafe { core::arch::asm!("cli") };
                        match outcome {
                            Ok(vmx::SliceOutcome::StillRunning) => {
                                crate::smp::fiber::yield_ready();
                            }
                            Ok(vmx::SliceOutcome::Idle) => {
                                park_vcpu_idle();
                            }
                            Ok(vmx::SliceOutcome::Exited(o)) => {
                                crate::kprintln!(
                                    "[microvm] guest exited — reason {:#x} qual {:#x}",
                                    (o.exit_reason & 0xFFFF) as u16,
                                    o.exit_qualification
                                );
                                break;
                            }
                            Err(e) => {
                                crate::kprintln!("[microvm] run FAILED: {}", e);
                                break;
                            }
                        }
                    }
                    // Last-one-out: the BSP owns the shared box. If an AP is
                    // still running, signal down + wait before freeing (close).
                    VM_CLOSE_REQUESTED.store(true, Ordering::Release);
                    while VCPU_COUNT.load(Ordering::Acquire) > 1 {
                        crate::smp::fiber::yield_sleep(2);
                    }
                    vmx::set_ap_active(false);
                    ap_orch_reset();
                    ctx.close();
                    VCPU_COUNT.store(0, Ordering::Release);
                }
                Err(e) => crate::kprintln!("[microvm] open FAILED: {}", e),
            }
        }
        Vendor::Unknown(reason) => crate::kprintln!("[microvm] {}", reason),
    }

    // Stop the RX producer (also covers an open-FAILED path where vm_poll_slice
    // teardown might not run). Idempotent with the vm_poll_slice stop sites.
    crate::microvm::devices::net_rx_worker::stop_worker();

    // Restore this core to the 100 Hz worker idle timer + the IF=0
    // park-loop invariant `smp_ap_entry` expects.
    crate::interrupts::disarm_dedicated_vm_timer();
    crate::interrupts::arm_worker_timer();

    drop(pending); // owned guest-image buffers freed
    crate::kprintln!("[microvm] vCPU fiber finished on core {}", cid);
    // Hand off to Core 0's reaper (compositor-locking teardown).
    VM_RUN_STATE.store(VM_EXITED, Ordering::Release);
}

/// AP (secondary) vCPU fiber (guest SMP). Spawned by the Core-0 reaper after
/// the guest's SIPI; `arg` is the AP's apic_id (1..). Aliases the BSP's
/// `VmShared` (it does NOT open guest RAM / EPT|NPT / devices). Runs its own
/// VMRUN/VMRESUME loop on whatever worker core picks it up, in parallel with
/// the BSP. Decrements `VCPU_COUNT` on exit so the BSP's last-one-out teardown
/// can proceed.
///
/// Vendor is resolved via `detect_vendor` (lock-free CPUID), NOT the VENDOR
/// mutex: the BSP vCPU fiber holds that mutex for its ENTIRE run loop
/// (`match *VENDOR.lock() { … }`), so contending on it here would block forever
/// (the AMD v0.193.1 freeze). On Intel the AP must `close_ap` (VMXOFF on its
/// own core); on AMD it just stops VMRUNning (the BSP owns teardown).
fn ap_vcpu_fiber_task(arg: u64) {
    let apic_id = arg as u8;
    let ptr = AP_SHARED_PTR.load(Ordering::Acquire);
    let vector = AP_SIPI_VECTORS
        .get(apic_id as usize)
        .map(|v| v.load(Ordering::Acquire))
        .unwrap_or(0);
    if ptr == 0 {
        VCPU_COUNT.fetch_sub(1, Ordering::AcqRel);
        return;
    }
    let cid = crate::smp::per_core::current_core_id();
    crate::kprintln!(
        "[microvm] AP vCPU fiber (apic_id {}) opening on core {} (sipi vec {:#x})",
        apic_id, cid, vector
    );
    // 1 kHz wake source on this core so the AP's idle yields resume promptly
    // (mirrors the BSP fiber).
    crate::interrupts::arm_dedicated_vm_timer();

    match detect_vendor() {
        Vendor::Intel => match vmx::vm_open_ap(ptr, vector, apic_id) {
            Ok(mut ctx) => {
                loop {
                    if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
                        break;
                    }
                    // SAFETY: ring-0; same IF discipline as the BSP fiber.
                    unsafe { core::arch::asm!("sti") };
                    let outcome = ctx.run_slice(SLICE_BUDGET);
                    // SAFETY: ring-0.
                    unsafe { core::arch::asm!("cli") };
                    match outcome {
                        Ok(vmx::SliceOutcome::StillRunning) => { crate::smp::fiber::yield_ready(); }
                        Ok(vmx::SliceOutcome::Idle) => { crate::smp::fiber::yield_sleep(2); }
                        Ok(vmx::SliceOutcome::Exited(o)) => {
                            crate::kprintln!(
                                "[microvm] AP vCPU exited — reason {:#x}",
                                (o.exit_reason & 0xFFFF) as u16
                            );
                            break;
                        }
                        Err(e) => {
                            crate::kprintln!("[microvm] AP run FAILED: {}", e);
                            break;
                        }
                    }
                }
                // VMX: the AP entered VMX root on this core → VMXOFF + free its
                // own VMXON/VMCS (but NOT the shared state — the BSP owns it).
                ctx.close_ap();
            }
            Err(e) => crate::kprintln!("[microvm] AP open FAILED: {}", e),
        },
        Vendor::Amd => match svm::vm_open_ap(ptr, vector, apic_id) {
            Ok(mut ctx) => loop {
                if VM_CLOSE_REQUESTED.load(Ordering::Acquire) {
                    break;
                }
                // SAFETY: ring-0; same IF discipline as the BSP fiber.
                unsafe { core::arch::asm!("sti") };
                let outcome = ctx.run_slice(SLICE_BUDGET);
                // SAFETY: ring-0.
                unsafe { core::arch::asm!("cli") };
                match outcome {
                    Ok(svm::SliceOutcome::StillRunning) => { crate::smp::fiber::yield_ready(); }
                    Ok(svm::SliceOutcome::Idle) => { crate::smp::fiber::yield_sleep(2); }
                    Ok(svm::SliceOutcome::Exited(o)) => {
                        crate::kprintln!(
                            "[microvm] AP vCPU exited — reason {:#x}",
                            (o.exit_reason & 0xFFFF) as u16
                        );
                        break;
                    }
                    Err(e) => {
                        crate::kprintln!("[microvm] AP run FAILED: {}", e);
                        break;
                    }
                }
                // AMD: the AP does NOT close() — the BSP owns + frees the box.
            },
            Err(e) => crate::kprintln!("[microvm] AP open FAILED: {}", e),
        },
        Vendor::Unknown(_) => {}
    }

    crate::interrupts::disarm_dedicated_vm_timer();
    crate::interrupts::arm_worker_timer();
    crate::kprintln!("[microvm] AP vCPU fiber finished on core {}", cid);
    // Let the BSP's last-one-out teardown proceed.
    VCPU_COUNT.fetch_sub(1, Ordering::AcqRel);
}

/// Decode the I/O VM-exit qualification field from a substrate-test
/// `LaunchOutcome.exit_qualification`. Currently vendor-agnostic by
/// dispatch — only Intel populates I/O exits today; the AMD VMCB
/// EXITINFO1/2 layout will be plumbed through here when SVM lands.
pub fn decode_io_exit_qualification(qual: u64) -> (u16, bool, u8) {
    match *VENDOR.lock() {
        Vendor::Intel => vmx::decode_io_exit_qualification(qual),
        // AMD VMCB exitinfo1 layout differs (port in bits 16-31,
        // type in bit 0); plumb in svm:: when backend lands.
        Vendor::Amd | Vendor::Unknown(_) => (0, false, 0),
    }
}

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
pub use vmx::LaunchOutcome;
