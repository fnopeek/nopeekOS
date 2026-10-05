//! CPU virtualization extensions — vendor dispatch.
//!
//! Detects the host CPU vendor at boot via CPUID leaf 0 (vendor
//! string) and dispatches MicroVM operations to the matching
//! backend:
//!
//!   * `vmx` — Intel VT-x (VMCS, EPT)
//!   * `svm` — AMD-V (VMCB, NPT)
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
//! differ in encoding. A trait across that boundary would be
//! method-by-method passthrough with vendor-specific output types and
//! no shared implementation, so a plain match is used.

pub mod rip_sample;
pub mod guest_cpuid; // guest CPUID allowlist (KVM kvm_cpu_cap_init model)
pub mod guest_msr; // vendor-neutral guest MSR emulation
pub mod svm;
pub mod vmx;

use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicU64, AtomicUsize, Ordering};

// ── VM-exit reason histogram (diagnosis) ───────────────────────────
//
// Both backends bump a bucket per guest exit so `cores` can show the mix
// while a VM runs: many `mmio` = the guest is rendering; `hlt`/`intr`
// dominating = idle. Backend-agnostic categories; each backend maps its
// own exit codes onto these.
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
// The `io` exit bucket can be dominated by the PIC EOI (`outb 0x20`): a
// guest running `noapic` acks every device IRQ through the 8259, and each
// ack is its own port-I/O VM exit. Bucketing by port shows where an
// io-exit storm comes from.
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
/// `kvm_emulate_hypercall`: `nr` + four args, result for RAX. The set is
/// what `guest_cpuid` announces — KVM_HC_SEND_IPI; the rest is -KVM_ENOSYS.
pub fn kvm_hypercall(apic_id: u8, cpl: u8, nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    const KVM_HC_SEND_IPI: u64 = 10;
    const KVM_ENOSYS: i64 = 1000;
    const KVM_EPERM: i64 = 1;
    if cpl != 0 { return -KVM_EPERM; }
    match nr {
        KVM_HC_SEND_IPI => svm::lapic::pv_send_ipi(apic_id, a0, a1, a2, a3),
        _ => -KVM_ENOSYS,
    }
}

/// Nested-page-fault targets: which device BAR, the LAPIC page, a first
/// touch of guest RAM (demand paging), or unclaimed.
pub const NPF_BUCKETS: usize = 11;
pub const NPF_LABELS: [&str; NPF_BUCKETS] =
    ["blk", "net", "gpu", "input", "9p", "sqfs", "snd", "lapic", "ram", "other", "ioapic"];
pub const NPF_BLK: usize = 0;
pub const NPF_NET: usize = 1;
pub const NPF_GPU: usize = 2;
pub const NPF_INPUT: usize = 3;
pub const NPF_P9: usize = 4;
pub const NPF_SQFS: usize = 5;
pub const NPF_SND: usize = 6;
pub const NPF_LAPIC: usize = 7;
pub const NPF_RAM: usize = 8;
pub const NPF_OTHER: usize = 9;
pub const NPF_IOAPIC: usize = 10;
static NPF_COUNTS: [AtomicU64; NPF_BUCKETS] = [const { AtomicU64::new(0) }; NPF_BUCKETS];
pub fn record_npf(kind: usize) {
    if kind < NPF_BUCKETS { NPF_COUNTS[kind].fetch_add(1, Ordering::Relaxed); }
}
pub fn npf_snapshot() -> [u64; NPF_BUCKETS] {
    core::array::from_fn(|i| NPF_COUNTS[i].load(Ordering::Relaxed))
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
// Counts (above) say how often we exit; these say how long each kind of
// handling costs versus time spent running the guest (VMRESUME), i.e.
// whether the host burns the core in exit handling or runs the guest.
static VM_EXIT_CYCLES: [AtomicU64; VMEXIT_BUCKETS] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; VMEXIT_BUCKETS]
};
static VM_GUEST_CYCLES: AtomicU64 = AtomicU64::new(0);

/// TSC cycles spent in the handler for `bucket` (between the exit and the
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
// `vmrun`/VMRESUME do not save or restore x87/SSE/AVX/AVX-512: host and
// guest share the one physical vector-register file. Any host FPU use
// between two guest entries (memcpy/checksum in nat::pump, virtio buffer
// copies, kprintln, font rasterization) silently corrupts the guest's live
// vector state; with musl using AVX-512 in memcpy/strlen this shows up as
// guest SIGSEGVs. KVM swaps unconditionally (kvm_load_guest_fpu /
// kvm_put_guest_fpu); we do the same.

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

// The XCR0 register is not touched. XSETBV is not intercepted, so the
// guest's Linux owns XCR0. Forcing a mask breaks things: the host mask vs
// the guest's CPUID-0xD-masked set panics fpu__init_system_xstate, and a
// reset mask while +avx2-built host code runs gives #UD on `vmovups ymm`.
// Mask = -1: xsave64/xrstor64 operate on every component enabled in the
// current XCR0. Guest XCR0 ⊇ host XCR0 (guest Linux enables at least
// x87+SSE+AVX, the host's set; extra AVX-512 bits are covered by -1), so
// host save/restore under the guest's XCR0 preserves the host's subset,
// and guest save/restore under it covers all guest state.

/// Guest-RAM size for the next VM, chosen at `vm_open` from live host
/// free memory.
///
/// Policy: take host free RAM minus a host reserve, clamp to
/// [`MIN`, cap], floor to the 2-MB EPT/NPT leaf granularity. With enough
/// free memory this is exactly the cap; it only shrinks on a RAM-starved
/// host instead of failing `allocate_contiguous`.
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
/// init paths that run before `microvm::cpu::init()` has set the
/// cached `VENDOR` static, e.g. `smp::per_core::init_dedicated_vm_core`.
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
/// A copy: `match *VENDOR.lock() { … }` keeps the guard for the whole
/// match, and the vCPU fiber's match is its entire run.
pub fn current_vendor() -> Vendor {
    let v = *VENDOR.lock();
    v
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
    match current_vendor() {
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
    match current_vendor() {
        Vendor::Intel => vmx::run_substrate_test(),
        Vendor::Amd => svm::run_substrate_test(),
        Vendor::Unknown(reason) => Err(reason),
    }
}

// ── Re-entrant active VM (Core-0 cooperative) ──────────────────────
//
// One backend-agnostic active VM, driven by the Core-0 event loop
// via `vm_poll_slice()` instead of a blocking whole-VM run. Holds the
// VmContext so Shade keeps rendering between bounded slices. A single
// global (one VM at a time); consumers never assume the count.
// Core-0-only access in practice; the Mutex guards against misuse.

enum ActiveVm {
    Vmx(vmx::VmContext),
    Svm(svm::VmContext),
}

static ACTIVE_VM: Mutex<Option<ActiveVm>> = Mutex::new(None);

/// Shade window the active VM's framebuffer is bound to (0 = none).
/// virtio-gpu FLUSH reads this to know which surface to write; the
/// teardown path closes it. One VM ↔ one window; keyed by id.
static ACTIVE_VM_WINDOW: AtomicU32 = AtomicU32::new(0);

/// Set when the user closes the VM's window so the next slice tears
/// the guest down instead of running it headless.
static VM_CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Cross-boundary "open my files in loft" trigger: the guest's browser
/// opens the magic 9p file `<root>/.open-in-loft`, the 9p server (on the
/// VM core) sets this, and Core 0 reaps it in `vm_poll_slice` to spawn
/// loft (a compositor op that must run on Core 0). Mirrors the
/// VM_CLOSE_REQUESTED cross-core handoff.
static OPEN_LOFT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request that the host open loft. Safe to call from the VM core; the
/// spawn itself happens on Core 0 (`vm_poll_slice`).
pub fn request_open_loft() {
    OPEN_LOFT_REQUESTED.store(true, Ordering::Release);
    crate::intent::wake_shell();
}

// ── Dedicated-core path ────────────────────────────────────────────
//
// When `per_core::dedicated_vm_core()` is Some, the guest runs in a
// continuous loop on that worker core instead of cooperative Core-0
// slicing. VMXON / host-state capture / the run loop / VMXOFF must
// all execute on that one core — a cross-core open would restore
// Core 0's GDT/TR/RSP onto the VM core on the first VM-exit. So
// Core 0 only stashes a request (`PENDING_VM`); the dedicated core
// (`vm_core_serve`, driven from `smp_ap_entry`) owns the VmContext on
// its own stack for the VM's whole lifetime. Core 0 coordinates only
// via these atomics + `ACTIVE_VM_WINDOW` / `VM_CLOSE_REQUESTED`, never
// the `ACTIVE_VM` mutex (cooperative path only), so it can't deadlock
// against the unbounded run loop.

const VM_IDLE: u8 = 0;
const VM_REQUESTED: u8 = 1;
const VM_RUNNING: u8 = 2;
const VM_EXITED: u8 = 3;

/// Dedicated-core VM lifecycle. Only meaningful when a core is
/// dedicated (cooperative path uses `ACTIVE_VM` instead). Also reused by
/// the fiber path (REQUESTED → RUNNING → EXITED).
static VM_RUN_STATE: AtomicU8 = AtomicU8::new(VM_IDLE);

// ── vCPU-as-fiber (unified core pool, docs/plan/SCHEDULER_FIBERS.md) ──
//
// Instead of statically carving a core for the guest at boot, run the
// guest's VMRESUME loop as a normal pool fiber (smp::fiber): admitted when
// a launch is requested, pinned to whatever worker core picks it up (so the
// VMX/SVM core binding holds — no migration), yielding the core to peer app
// fibers on guest idle, and freed on guest exit. No core is wasted when no
// VM runs. Setting the flag to `false` falls back to the dedicated path.
pub const VCPU_AS_FIBER: bool = true;

/// Run the VMX guest as a pool fiber too, so it leaves the cooperative
/// Core-0 path (which shares Core 0 with Shade, input and the cursor and
/// starves both under load). Relies on every core having its own TSS
/// (`tss::init_core`). `false` reverts Intel to
/// cooperative Core 0; AMD's fiber mode keys off the vendor, not this flag.
pub const VMX_VCPU_AS_FIBER: bool = true;

/// Trap-and-emulate the guest local APIC (`svm::lapic`) instead of booting
/// `nolapic`. When true, the guest cmdline omits `nolapic` so Linux brings
/// up the LAPIC and uses its timer; the LAPIC MMIO page faults into the
/// emulator. `false` boots the guest with the LAPIC disabled and the
/// emulator inert, without a guest-kernel rebuild. Prerequisite for APs.
pub const GUEST_LAPIC: bool = true;

/// Enumerate more than one vCPU to Linux via an MP table (`linux::mptable`,
/// floating pointer at 0xF0000). Whether the APs are actually brought up is
/// `GUEST_SMP_AP`: an enumerated AP that never responds hangs Linux's cpuhp
/// bring-up (it waits for the CPU to report alive). Requires `GUEST_LAPIC`.
/// `false` means no MP table and a uniprocessor guest.
pub const GUEST_SMP: bool = true;

/// Number of vCPUs the MP-table enumerates to the guest when `GUEST_SMP` is on
/// (BSP apic_id 0 + APs 1..). Dynamic: one vCPU per host worker core
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
    // When the off-vCPU net backend runs it needs its own worker core, never
    // a vCPU's (`place_worker`). Co-located with a vCPU, the vCPU gets
    // preempted by the worker, answers cross-vCPU TLB-shootdown IPIs late,
    // and the other vCPUs spin in csd_lock_wait. Leaving one worker core
    // free maps each vCPU 1:1 to a core so IPIs are answered promptly.
    if reserve_offload_core() {
        workers = workers.saturating_sub(1);
    }
    // A second reserved core for the off-vCPU GPU worker (framebuffer copy off
    // the vCPU). One fewer vCPU; the browser is not guest-CPU-bound so it wins.
    if reserve_gpu_core() {
        workers = workers.saturating_sub(1);
    }
    // And one per core carrying the network's own fibers — see `protected_cores`.
    workers = workers.saturating_sub(protected_cores().count_ones() as usize);
    workers.clamp(1, MAX_VCPUS_CAP) as u8
}

/// Bitmask of cores that must stay clear of vCPUs: every core a hardware
/// driver lives on (`per_core::driver_cores` — a fiber there bound a device
/// or waits on its interrupt), the WASM NIC driver's and the WiFi manager's.
///
/// A vCPU fiber and a driver fiber on one core are cooperative peers: the
/// driver runs only when the vCPU yields, once per `SLICE_MS` at best. That
/// is too rare for input devices, and a NIC driver's receive buffers (which
/// the host's own network relies on) overflow.
///
/// A core whose fibers are apps (panels, dock) is not protected: they sleep
/// on events, and a vCPU yielding every slice costs them at most that slice.
///
/// Snapshotted while no VM exists: `guest_vcpus()` sizes the MP table and the
/// IPI broadcast and must not change under a running guest.
static PROTECTED_CORES: AtomicU32 = AtomicU32::new(0);

fn protected_cores() -> u32 {
    if VM_RUN_STATE.load(Ordering::Acquire) != VM_IDLE {
        return PROTECTED_CORES.load(Ordering::Acquire);
    }
    let mut mask = 0u32;
    if crate::smp::per_core::core_count() >= 3 {
        if let Some(c) = crate::netdev::wasm_nic_core() { mask |= 1 << c; }
        if let Some(c) = crate::wifi::manager_core() { mask |= 1 << c; }
        mask |= crate::smp::per_core::driver_cores() as u32;
        // Every other fiber core too, for now: with cooperative fibers and
        // spin locks, a peer that yields while holding a lock the vCPU
        // needs may never run again, and vCPUs beside app fibers have been
        // seen to hang. vCPUs get empty cores until that is ruled out.
        let n = crate::smp::per_core::core_count().min(32);
        for c in 1..n {
            if crate::smp::fiber::fiber_count(c) > 0 { mask |= 1 << c; }
        }
    }
    mask &= !1; // Core 0 is never a vCPU core anyway
    PROTECTED_CORES.store(mask, Ordering::Release);
    mask
}

/// Among the cores in `candidates` (bitmask), the one with the fewest resident
/// fibers — an idle core first, then one whose apps sleep.
fn least_fibered(candidates: u32) -> Option<usize> {
    let n = crate::smp::per_core::core_count().min(32);
    (1..n)
        .filter(|&c| candidates & (1 << c) != 0)
        .min_by_key(|&c| crate::smp::fiber::fiber_count(c))
}

/// True if `cid` runs a vCPU or a microvm worker (not a protected host core).
/// Such a core must not pick up new host work while the guest runs.
pub fn is_vm_worker_core(cid: usize) -> bool {
    if cid == 0 || cid >= 32 || VM_RUN_STATE.load(Ordering::Acquire) == VM_IDLE {
        return false;
    }
    let busy = VM_CORE_MASK.load(Ordering::Acquire) & !protected_cores();
    busy & (1 << cid) != 0
}

/// Lowest worker core the network does not need. Whichever core picks up the BSP
/// vCPU fiber owns the guest for its whole lifetime, so the choice is made here
/// rather than left to work-stealing.
fn pick_vcpu_core() -> usize {
    let n = crate::smp::per_core::core_count().min(32);
    if n <= 1 { return 0; }
    let all = ((1u64 << n) - 1) as u32 & !1;
    least_fibered(all & !protected_cores()).unwrap_or(n - 1)
}

/// Reserve a dedicated worker core for the off-vCPU net backend fiber
/// (see `guest_vcpus`). `false` co-locates the worker (one more vCPU, but
/// csd_lock_wait contention).
pub const RESERVE_OFFLOAD_CORE: bool = true;

/// True when the off-vCPU net worker will actually claim a core this run and
/// the reservation is on and there is a core to spare. Mirrors the
/// `full_backend` gate at `start_worker`.
fn reserve_offload_core() -> bool {
    // Not vendor-gated. The worker owns the guest's rings and, for a card
    // that raises no RX interrupt, polls that card as well, so it is the
    // inbound path for the guest and, while it holds the drain guard, for
    // the host's own sockets. An unmarked core lets an AP vCPU land on top
    // of it at the next guest SIPI, so `place_worker(true)` marks it.
    //
    // Needs >= 3 cores: Core 0 + >= 1 vCPU + 1 worker. Below that,
    // co-location is the lesser evil against starving the guest.
    RESERVE_OFFLOAD_CORE && crate::smp::per_core::core_count() >= 3
}

/// Reserve a second dedicated worker core for the off-vCPU GPU backend (the
/// per-frame framebuffer copy + write_frame). Needs ≥4 cores: Core 0 + ≥1
/// vCPU + net worker + gpu worker. AMD-only (the off-vCPU GPU backend is
/// SVM-only). Below that the GPU work stays inline on the vCPU.
fn reserve_gpu_core() -> bool {
    reserve_offload_core()
        && crate::microvm::devices::gpu_backend::FULL_GPU_BACKEND
        && crate::smp::per_core::core_count() >= 4
}

/// Hard cap on guest vCPUs (sizes the per-backend IPI bitmaps + the spawn
/// bitmask). 8 covers an 8-thread machine fully and is plenty for a browser;
/// a 16/32-core desktop caps here rather than spawning a vCPU per core (idle
/// vCPUs each carry a small wake overhead). Must be ≤ each backend's
/// `MAX_VCPUS` and ≤ 32 (the `u32` spawn bitmask).
pub const MAX_VCPUS_CAP: usize = 8;

/// Bring up the AP vCPUs. When true the boot cmdline raises `maxcpus` to
/// `GUEST_VCPUS`, the guest's INIT-SIPI spawns further vCPU fibers sharing
/// the BSP's `VmShared`, and the cross-vCPU IPI path is used. When false
/// there is no spawn, `maxcpus=1`, and the big VM lock is never taken.
/// Requires `GUEST_SMP`.
pub const GUEST_SMP_AP: bool = true;

/// Whether guest-SMP AP bring-up is enabled and supported on this host. Both
/// backends now have an AP-vCPU open path (`svm::vm_open_ap` / `vmx::vm_open_ap`)
/// + SIPI/LAPIC routing, so this gates the guest's `maxcpus` (and the AP spawn)
/// on AMD and Intel alike. Unknown vendor stays single-vCPU.
pub fn smp_ap_active() -> bool {
    GUEST_SMP_AP && matches!(current_vendor(), Vendor::Amd | Vendor::Intel)
}

/// Set/clear the active backend's guest-SMP big-VM-lock engagement. Dispatches
/// via `detect_vendor` (lock-free CPUID), not `current_vendor()` (which locks
/// VENDOR) — the BSP vCPU fiber holds the VENDOR lock for its whole run loop,
/// so the last-one-out call from inside that arm must not re-lock it.
fn vm_set_ap_active(on: bool) {
    match detect_vendor() {
        Vendor::Intel => vmx::set_ap_active(on),
        Vendor::Amd => svm::set_ap_active(on),
        Vendor::Unknown(_) => {}
    }
}

/// An I/O APIC in the MP table, and the guest booted without `noapic`: device
/// lines reach the vCPUs as LAPIC vectors (PV-EOI, no 8259 port exits), and
/// Linux enables x2APIC — which it will not do under `noapic`
/// (`enable_IR_x2apic` returns before trying). Needs the MP table and a LAPIC.
/// `false` goes back to the 8259.
pub const GUEST_IOAPIC: bool = true;

pub fn guest_ioapic_active() -> bool {
    GUEST_IOAPIC && GUEST_SMP && guest_lapic_active()
}

/// Whether the guest's local APIC is emulated on this host. Without
/// emulation, a guest booted without `nolapic` would program the LAPIC
/// (TSC-deadline) timer and never receive a tick, hanging its event loops.
/// In that case the cmdline keeps `nolapic` and the guest falls back to the
/// PIT IRQ0 the VMX path injects.
pub fn guest_lapic_active() -> bool {
    GUEST_LAPIC
        && match current_vendor() {
            Vendor::Amd => true,
            Vendor::Intel => VMX_GUEST_LAPIC,
            Vendor::Unknown(_) => false,
        }
}

/// Emulate the guest local APIC on VMX too (`vmx::lapic` reuses the pure
/// `svm::lapic::LocalApic`). When true the Intel guest boots without
/// `nolapic` and the LAPIC MMIO page EPT-faults into the emulator; when
/// false it keeps `nolapic`. Prerequisite for VMX guest SMP: Linux needs
/// the per-CPU LAPIC timer to schedule APs.
pub const VMX_GUEST_LAPIC: bool = true;

// ── AP (secondary vCPU) spawn orchestration (guest SMP, N vCPUs) ────────
//
// The guest's INIT-SIPI is decoded on a vCPU fiber (a worker core), which
// cannot push a fiber itself (the run-queue deque is single-producer, owned by
// Core 0). So the SIPI handler only records a request here (per target
// apic_id); the Core-0 reaper (`vm_poll_slice`) does the actual `spawn_fiber`
// for each newly-requested AP. Each AP is tracked by its apic_id bit, so a
// guest with several APs brings them all up.

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
/// is always reserved (shell/reaper). Each vCPU must get a distinct core: VMX
/// root is per physical core, so two vCPUs VMXONing one core fail with
/// VMfailValid; on SVM it just starves. The reaper places each AP on the
/// lowest free worker core via `reserve_ap_core` + `fiber::admit` instead of
/// `spawn_fiber` (whose work-stealing piles every fiber onto the one awake
/// core). Reset at BSP open + teardown.
static VM_CORE_MASK: AtomicU32 = AtomicU32::new(1); // bit 0 = Core 0 reserved

/// Host cores running a vCPU fiber (the BSP's and each AP's). A worker is
/// never placed here: fibers are cooperative, and a vCPU gives its core up
/// once per slice at best — a worker beside it answers a kick in milliseconds.
static VCPU_CORES: AtomicU32 = AtomicU32::new(0);
/// Cores already given a microvm worker, so the next one goes elsewhere.
static WORKER_CORES: AtomicU32 = AtomicU32::new(0);

/// Does a vCPU run on host core `cid` right now?
pub fn is_vcpu_core(cid: usize) -> bool {
    cid < 32 && VM_RUN_STATE.load(Ordering::Acquire) != VM_IDLE
        && VCPU_CORES.load(Ordering::Acquire) & (1 << cid) != 0
}

/// Place a microvm worker (net data plane, GPU copy, 9p persist). Never Core 0
/// and never a vCPU's core. In order:
///   1. a core nobody uses (not in `VM_CORE_MASK`) — `claim` marks it, so a
///      later AP vCPU does not land on it;
///   2. a host-fiber core without a worker yet, fewest fibers first. Those
///      fibers park on events; an event-driven worker shares with them fine
///      (the NIC driver's and WiFi manager's cores excluded — they pump);
///   3. any core that is not a vCPU's, fewest fibers first.
/// Only when every worker core runs a vCPU does it share one.
pub fn place_worker(claim: bool) -> usize {
    let ncores = crate::smp::per_core::core_count().min(32);
    if ncores <= 1 { return 0; }
    let vcpus = VCPU_CORES.load(Ordering::Acquire);
    let pumps = {
        let mut m = 0u32;
        if let Some(c) = crate::netdev::wasm_nic_core() { m |= 1 << c; }
        if let Some(c) = crate::wifi::manager_core() { m |= 1 << c; }
        m
    };
    let chosen = loop {
        let mask = VM_CORE_MASK.load(Ordering::Acquire);
        let Some(c) = (1..ncores).find(|&c| mask & (1u32 << c) == 0) else { break None };
        if !claim { break Some(c); }
        if VM_CORE_MASK
            .compare_exchange(mask, mask | (1u32 << c), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            break Some(c);
        }
    };
    let c = chosen.unwrap_or_else(|| {
        let workers = WORKER_CORES.load(Ordering::Acquire);
        let least = |skip: u32| (1..ncores)
            .filter(|&c| skip & (1u32 << c) == 0)
            .min_by_key(|&c| crate::smp::fiber::fiber_count(c));
        least(vcpus | pumps | workers)
            .or_else(|| least(vcpus | pumps))
            .or_else(|| least(vcpus))
            .unwrap_or(ncores - 1)
    });
    WORKER_CORES.fetch_or(1u32 << c, Ordering::AcqRel);
    c
}

/// Reserve a distinct idle worker core (1..core_count()) for an AP vCPU fiber,
/// marking it taken. `None` if every worker core already runs a vCPU (the
/// caller then falls back to the shared deque — only safe on SVM; on VMX that
/// means more vCPUs than host cores, which `guest_vcpus()` avoids by capping at
/// the worker count). Called only from the Core-0 reaper (single producer).
fn reserve_ap_core() -> Option<usize> {
    let ncores = crate::smp::per_core::core_count().min(32);
    let all = ((1u64 << ncores) - 1) as u32 & !1;
    loop {
        let mask = VM_CORE_MASK.load(Ordering::Acquire);
        let c = least_fibered(all & !mask)?;
        if VM_CORE_MASK
            .compare_exchange(mask, mask | (1u32 << c), Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            VCPU_CORES.fetch_or(1u32 << c, Ordering::AcqRel);
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
    // Core 0 spawns it in `vm_poll_slice`.
    crate::intent::wake_shell();
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
    VCPU_CORES.store(0, Ordering::Release);
    WORKER_CORES.store(0, Ordering::Release);
}

/// Decided once at boot (`set_vm_fiber_mode`, from `init_dedicated_vm_core`,
/// which has the vendor + worker count): true → the guest runs as a pool
/// fiber and no core is dedicated. Cheap atomic read on the hot poll path.
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
        crate::intent::wake_shell();
    }
}

/// The guest shut itself down (e.g. LibreWolf's own window-X → cage exits →
/// PID-1 `halt` → `reboot: System halted`). The serial scanner calls this so
/// the run loop takes the same clean exit as a user Mod+Q: break → `close()`
/// (saves the home image) → Core-0 reaper closes the window. Without it a
/// `cli;hlt`-halted guest would spin the run loop forever (black tile, no
/// save) until the user closes the host window.
pub fn note_guest_shutdown() {
    VM_CLOSE_REQUESTED.store(true, Ordering::Release);
    crate::intent::wake_shell();
}

/// Drop the VM↔window binding + its surface and close the Shade
/// window. Must be called without the ACTIVE_VM lock held (it locks
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

/// Host core running the BSP vCPU fiber, or `usize::MAX`. Vendor-neutral,
/// so the RX producer can wake the consumer on both backends.
static BSP_HOST_CORE: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Core running the BSP vCPU, if a guest runs.
pub fn bsp_host_core() -> Option<usize> {
    match BSP_HOST_CORE.load(Ordering::Relaxed) {
        usize::MAX => None,
        c => Some(c),
    }
}

/// Last step before a guest entry (`vcpu_enter_guest`: IRQs off, then
/// recheck): disable host interrupts, arm the host one-shot at `deadline`,
/// and cancel the entry if a kick came in since `kick_gen` was read. With
/// IF=1 here, a kick or the timer interrupt would be taken by the host and
/// exit nothing: the vCPU would run on with a posted vector or with no armed
/// timer, and nothing would bring it out. With IF=0 either stays pending and
/// exits the guest at once, a deadline already past included. `false` = IF
/// is back on, loop again.
#[inline]
pub fn entry_irqs_off(kick_gen: u64, host_core: usize, deadline: u64) -> bool {
    // SAFETY: the vCPU loop runs with IF=1; `entry_irqs_on` or the VMX
    // exit asm sets it again.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
    crate::interrupts::arm_vcpu_timer(deadline);
    if crate::smp::fiber::net_kick_gen(host_core) != kick_gen {
        entry_irqs_on();
        return false;
    }
    true
}

/// Host interrupts back on after the guest exit; a pending one is taken now.
#[inline]
pub fn entry_irqs_on() {
    // SAFETY: counterpart of `entry_irqs_off`.
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
}

/// The wake half of irqfd (`virt/kvm/eventfd.c`): an RX-ready signal both
/// raises the guest's IRQ line and wakes the vCPU. `net_backend::raise_irq`
/// is the raise; this is the wake: kick the core running the BSP vCPU so it
/// takes an exit and folds the line into `pending_irqs`. Vendor-neutral;
/// `BSP_HOST_CORE` is written by `vcpu_fiber_task` before either vendor's
/// run loop starts. Also the wake for every other device source the BSP
/// services (input, 9p replies): a blocked vCPU parks until a timer
/// deadline or this kick.
pub fn kick_bsp_net_irq() {
    let hc = BSP_HOST_CORE.load(Ordering::Relaxed);
    if hc != usize::MAX {
        crate::smp::kick_host_core(hc);
    }
}

/// Is a guest running right now, on any path? Anything that means "is
/// there a guest" wants this rather than `vm_active`.
pub fn guest_running() -> bool {
    VM_RUN_STATE.load(Ordering::Acquire) == VM_RUNNING || vm_active()
}

/// True if Core 0 should spin-feed a cooperative microvm, i.e. a guest runs
/// on the cooperative Core-0 path. False on the dedicated and fiber paths:
/// there the guest runs on its own core, and Core 0 idles/composites
/// normally and reaps via `vm_poll_slice`.
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
    // it (smp_ap_entry → fiber::admit) and runs the guest on that core for
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
        // Snapshot the protected cores while still idle (see `protected_cores`).
        let _ = protected_cores();
        VM_RUN_STATE.store(VM_REQUESTED, Ordering::Release);
        // Place it deliberately: `spawn_fiber` hands the task to whichever
        // worker steals it first, and that core owns the guest for its whole
    // lifetime. Landing on the core a WASM NIC driver polls from would cost
    // the machine its network (see `nic_core`).
        crate::smp::fiber::admit(pick_vcpu_core(), vcpu_fiber_task, 0);
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
        let _ = protected_cores();
        VM_RUN_STATE.store(VM_REQUESTED, Ordering::Release);
        return Ok(());
    }

    let mut slot = ACTIVE_VM.lock();
    if slot.is_some() {
        return Err("a microvm is already running");
    }
    let vm = match current_vendor() {
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

    // Dedicated path and fiber path: Core 0 is only the reaper. The VM
    // core (dedicated core, or the worker running the vCPU fiber) owns the
    // VmContext for its whole lifetime and does its own VMXOFF; Core 0 just
    // runs the compositor-locking teardown once it has exited
    // (teardown_vm_window must run on Core 0). Cheap atomic load on the hot
    // poll path when nothing has exited.
    // Guest SMP: a guest SIPI asked us to bring up an AP. Core 0 owns the
    // run-queue deque, so it does the spawn here; the BSP vCPU fiber that
    // decoded the SIPI runs on a worker and cannot push. Once per AP
    // (AP_SPAWNED guard absorbs the retried 2nd SIPI). AP_ACTIVE is set
    // before the spawn so the BSP starts taking the big VM lock before the
    // AP can run.
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
                // Place the AP on a distinct idle worker core (one vCPU per
                // core, since VMX root is per core). `fiber::admit` pushes
                // straight to that core's fiber queue and wakes it by IPI
                // (idle workers have no tick); `spawn_fiber`'s work-stealing
                // would pile every vCPU onto the one awake core.
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
            crate::microvm::devices::net_dataplane::stop_worker();
            crate::microvm::devices::p9_async::stop_worker();
            crate::microvm::devices::nat::reset_sessions();
            BSP_HOST_CORE.store(usize::MAX, Ordering::Release);
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
        crate::microvm::devices::net_dataplane::stop_worker();
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
    crate::microvm::devices::net_dataplane::stop_worker();
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

/// Host-idle the dedicated core while the guest is halted: until its next
/// timer deadline or an interrupt / kick IPI (`kvm_vcpu_block`).
#[inline]
fn idle_host_sleep(next_timer_tsc: Option<u64>) {
    crate::interrupts::halt_until(
        Some(vcpu_block_deadline(next_timer_tsc)),
        crate::smp::per_core::WAKE_HLT_FALLBACK,
    );
}

/// Dedicated-core entry point — called every iteration of the
/// dedicated worker core's `smp_ap_entry` loop. Cheap no-op unless a
/// launch is pending. When one is, this opens the VM on this core
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

    // Host interrupts ON between VMRUNs, so the one-shot armed before
    // each entry (`arm_vcpu_timer`) and kick IPIs are taken and EOI'd.
    // SAFETY: ring-0; CLGI/STGI still brackets the VMRUN-critical
    // region inside run_guest_once. This mirrors Core 0's IF=1.
    unsafe { core::arch::asm!("sti") };

    // Continuous run: open + `loop run_slice` + close, plus a window-close
    // check so the user can `Mod+Q` the tile. `run_slice` returns
    // periodically on its wall-clock deadline; we loop straight back (no
    // Shade composite, no hlt on this core). The two backends have distinct
    // `SliceOutcome` enums, so match each concretely.
    match current_vendor() {
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
                                idle_host_sleep(ctx.next_timer_deadline_tsc());
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
                                idle_host_sleep(ctx.next_timer_deadline_tsc());
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

    // Restore the IF=0 state `smp_ap_entry`'s park loop expects (it does
    // its own sti;hlt;cli).
    // SAFETY: ring-0; return the core to the parked-loop invariant.
    unsafe { core::arch::asm!("cli") };

    drop(pending); // owned guest-image buffers freed
    // Hand off to Core 0's reaper (compositor-locking teardown).
    VM_RUN_STATE.store(VM_EXITED, Ordering::Release);
    crate::intent::wake_shell();
}

/// Idle-park safety cap. The real wakes are event-driven (an RX/TX/IPI kick
/// or the guest's own LAPIC-timer deadline), so this only bounds a truly
/// idle guest (no timer armed, no traffic) from sleeping forever and never
/// re-checking VM_CLOSE_REQUESTED.
const PARK_SAFETY_MS: u64 = 8;

/// Wake-up bound for a blocked vCPU: the guest's next timer tick, clamped to
/// [now, safety]. A past deadline re-enters at once; a far or absent one
/// re-checks no later than the safety cap.
fn vcpu_block_deadline(next_timer_tsc: Option<u64>) -> u64 {
    let now = crate::interrupts::rdtsc();
    let safety = now + PARK_SAFETY_MS.saturating_mul(crate::interrupts::tsc_freq() / 1000);
    next_timer_tsc.map(|d| d.clamp(now, safety)).unwrap_or(safety)
}

/// Park the BSP vCPU fiber when the guest is idle — block-on-event as in
/// KVM `kvm_vcpu_block`: one park, woken by whichever event fires first.
///   * RX/TX/IPI ready → the off-vCPU backend / a peer vCPU bumps this core's
///     net-kick generation + sends a VCPU_KICK IPI.
///   * Guest timer due → `next_timer_tsc` is the guest's next LAPIC-timer
///     deadline (KVM `apic_timer_fn` hrtimer); the park ends there so the
///     guest clock advances at its programmed rate independent of VMRUN.
///   * Safety cap (`PARK_SAFETY_MS`) — only an idle guest with no timer
///     reaches it.
/// The wake must be event-driven; spinning while active pegs a worker core
/// and starves the compositor.
fn park_vcpu_idle(next_timer_tsc: Option<u64>) {
    let now = crate::interrupts::rdtsc();
    let freq = crate::interrupts::tsc_freq();
    let deadline = vcpu_block_deadline(next_timer_tsc);

    // When the off-vCPU RX backend (or producer) owns the NIC drain, RX wakes us
    // via the net-kick generation — we must not arm/route the host RX IRQ here
    // (that would steal the worker's event-wake). Block on the unified deadline.
    if crate::microvm::devices::net_dataplane::active() {
        // The off-vCPU worker injects RX + kicks this fiber. No halt-poll
        // (busy spin before HLT): the guest idles waiting for data, it is
        // not CPU-bound, so keeping it warm only burns the core.
        crate::smp::fiber::kick_wait_until(deadline);
        return;
    }

    // No backend: this vCPU drains the NIC itself, so it must also wake on the
    // host RX IRQ (routed to this core). Bound the wait by the timer deadline.
    let timeout_ms = ((deadline.saturating_sub(now)) / (freq / 1000).max(1)).max(1);
    // Not while the NAPI fiber owns the vector: `arm` would route the card's
    // interrupt away from the fiber that drains it.
    if let Some(vec) = crate::netdev::rx_wake_vector().filter(|_| !crate::net::napi::active()) {
        let since = crate::irq::arm(vec); // snapshot + route IRQ to this core
        crate::smp::fiber::irq_wait(vec, since, timeout_ms);
        return;
    }
    crate::smp::fiber::yield_sleep(timeout_ms);
}

/// vCPU-as-fiber entry (fiber mode). Same lifecycle as `vm_core_serve`:
/// consume the pending request, open the guest on this (the admitting)
/// core, run it slice by slice, close it, signal Core 0 to reap. Instead
/// of a dedicated forever-loop it yields the core to peer app fibers
/// between slices. Pinned to its core (no fiber migration) so the VMX/SVM
/// core binding holds. IF=1 only across each `run_slice` (host tick
/// servicing between VMRUNs), restored to IF=0 before every yield so peer
/// fibers keep the cooperative IF=0 invariant.
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
    BSP_HOST_CORE.store(cid, Ordering::Release);
    crate::kprintln!("[microvm] vCPU fiber opening guest on core {}", cid);

    // Guest SMP: (re)initialise the vCPU-core mask with the BSP's own core so
    // APs are placed on other cores (one vCPU per distinct core). Fresh store,
    // not OR, so a relaunch starts clean. Runs before the guest boots → before
    // any SIPI → the reaper always sees the BSP bit.
    VM_CORE_MASK.store((1u32 << 0) | (1u32 << cid), Ordering::Release);
    VCPU_CORES.store(1u32 << cid, Ordering::Release);
    WORKER_CORES.store(0, Ordering::Release);
    // Mark the network's cores taken so no AP vCPU and no offload worker is ever
    // placed there.
    VM_CORE_MASK.fetch_or(protected_cores(), Ordering::AcqRel);
    // Clean counter window for this run. Deliberately here and not at teardown,
    // so the numbers from the last run survive it — a post-mortem is the one
    // time anyone reads them.
    crate::microvm::devices::nat::reset_counters();

    // Spawn the data-plane worker on another core: it moves frames between the
    // tap and the guest rings, so this BSP vCPU does no net work beyond the TX
    // doorbell and its IRQ. Stopped in this fiber's teardown + vm_poll_slice.
    // There is one data path for both vendors.
    // `place_worker` never puts it on a vCPU's core; with the reservation on it
    // marks a free core so the SIPI'd APs skip it.
    let worker_core = place_worker(reserve_offload_core());
    crate::kprintln!(
        "[microvm] net worker core {} (vCPUs={}, reserve={})",
        worker_core, guest_vcpus(), reserve_offload_core()
    );
    crate::microvm::devices::net_dataplane::start_worker(worker_core);

    // Off-vCPU GPU worker: on AMD with a spare core, claim a second reserved
    // core and run the per-frame framebuffer copy + write_frame there, off
    // the vCPU, so the browser's rendering never steals the net-servicing
    // cycles. guest_vcpus() already left this core free. Gated on
    // FULL_GPU_BACKEND + AMD + ≥4 cores; else GPU stays inline.
    if reserve_gpu_core() {
        let gpu_core = place_worker(true);
        crate::kprintln!(
            "[microvm] gpu worker core {} (off-vCPU framebuffer copy)", gpu_core);
        crate::microvm::devices::gpu_backend::start_worker(gpu_core);
    }

    match current_vendor() {
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
                            svm::lapic::phase(0, svm::lapic::PH_YIELD);
                            crate::smp::fiber::yield_ready();
                        }
                        // Idle guest: park briefly → core runs app fibers.
                        // Event-driven on host RX IRQ while downloading.
                        Ok(svm::SliceOutcome::Idle) => {
                            svm::lapic::phase(0, svm::lapic::PH_PARK);
                            park_vcpu_idle(ctx.next_timer_deadline_tsc());
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
                    crate::intent::wake_shell();
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
            // VMX guest as a fiber + guest-SMP BSP. `vmx::vm_open` installs
            // this worker's per-core TSS so VMX host state is valid off Core 0.
            // Same IF/yield discipline as the AMD arm. Owns the shared
            // `VmShared`: seeds the live-vCPU count + publishes its address so
            // AP fibers can alias it, and last-one-out teardown waits for APs
            // before `close()`.
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
                                park_vcpu_idle(ctx.next_timer_deadline_tsc());
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
                    crate::intent::wake_shell();
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

    // Stop the RX producer (also covers an open-failed path where vm_poll_slice
    // teardown might not run). Idempotent with the vm_poll_slice stop sites.
    crate::microvm::devices::net_dataplane::stop_worker();


    drop(pending); // owned guest-image buffers freed
    crate::kprintln!("[microvm] vCPU fiber finished on core {}", cid);
    // Hand off to Core 0's reaper (compositor-locking teardown).
    VM_RUN_STATE.store(VM_EXITED, Ordering::Release);
    crate::intent::wake_shell();
}

/// AP (secondary) vCPU fiber (guest SMP). Spawned by the Core-0 reaper after
/// the guest's SIPI; `arg` is the AP's apic_id (1..). Aliases the BSP's
/// `VmShared` (it does not open guest RAM / EPT|NPT / devices). Runs its own
/// VMRUN/VMRESUME loop on whatever worker core picks it up, in parallel with
/// the BSP. Decrements `VCPU_COUNT` on exit so the BSP's last-one-out teardown
/// can proceed.
///
/// Vendor is resolved via `detect_vendor` (lock-free CPUID). On Intel the
/// AP must `close_ap` (VMXOFF on its own core); on AMD it just stops
/// VMRUNning (the BSP owns teardown).
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
                        Ok(vmx::SliceOutcome::Idle) => {
                            crate::smp::fiber::kick_wait_until(
                                vcpu_block_deadline(ctx.next_timer_deadline_tsc()));
                        }
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
                // own VMXON/VMCS (but not the shared state — the BSP owns it).
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
                    Ok(svm::SliceOutcome::StillRunning) => {
                        svm::lapic::phase(apic_id, svm::lapic::PH_YIELD);
                        crate::smp::fiber::yield_ready();
                    }
                    Ok(svm::SliceOutcome::Idle) => {
                        svm::lapic::phase(apic_id, svm::lapic::PH_PARK);
                        crate::smp::fiber::kick_wait_until(
                            vcpu_block_deadline(ctx.next_timer_deadline_tsc()));
                    }
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
                // AMD: the AP does not close() — the BSP owns + frees the box.
            },
            Err(e) => crate::kprintln!("[microvm] AP open FAILED: {}", e),
        },
        Vendor::Unknown(_) => {}
    }

    crate::kprintln!("[microvm] AP vCPU fiber finished on core {}", cid);
    // Let the BSP's last-one-out teardown proceed.
    VCPU_COUNT.fetch_sub(1, Ordering::AcqRel);
}

/// Decode the I/O VM-exit qualification field from a substrate-test
/// `LaunchOutcome.exit_qualification`. Only Intel populates I/O exits.
pub fn decode_io_exit_qualification(qual: u64) -> (u16, bool, u8) {
    match current_vendor() {
        Vendor::Intel => vmx::decode_io_exit_qualification(qual),
        // AMD VMCB exitinfo1 layout differs (port in bits 16-31,
        // type in bit 0). Not implemented.
        Vendor::Amd | Vendor::Unknown(_) => (0, false, 0),
    }
}

/// Outcome of one VM-entry/exit cycle.
///
/// The numeric fields are vendor-specific in their meaning:
///   * Intel: `exit_reason` is the Intel basic exit reason
///     (SDM Vol. 3C App. C); `exit_qualification` is VMCS field
///     `VM_EXIT_QUALIFICATION`.
///   * AMD: `exit_reason` would be the VMCB EXITCODE and
///     `exit_qualification` a packed EXITINFO1/EXITINFO2.
///
/// Callers that decode reason values must dispatch on
/// `current_vendor()`.
pub use vmx::LaunchOutcome;
