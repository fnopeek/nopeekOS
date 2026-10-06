//! Per-Core State
//!
//! Tracks CPU cores discovered at boot. Dynamically sized — no hardcoded limit.
//! Scales from 1 (BSP only) to 1024+ cores.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;
use crate::hw::{msr, Mmio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreState {
    Bsp,
    Online,
    Failed,
}

pub struct CoreInfo {
    /// Sequential index (0 = BSP)
    #[allow(dead_code)]
    pub id: u32,
    /// Hardware APIC ID (may not be sequential)
    pub apic_id: u32,
    pub state: CoreState,
}

pub static CORES: Mutex<Vec<CoreInfo>> = Mutex::new(Vec::new());
static CORE_COUNT: AtomicUsize = AtomicUsize::new(1);

/// Set to true once scheduler is initialized and APs should start working
static SCHEDULER_READY: AtomicBool = AtomicBool::new(false);

/// True if CPU supports MONITOR/MWAIT (detected at boot)
static HAS_MWAIT: AtomicBool = AtomicBool::new(false);

/// True if CPU supports IA32_APERF/IA32_MPERF MSRs (CPUID.06H:ECX[0]).
/// Intel since Nehalem; not present on AMD / qemu64. Guarded at every rdmsr.
static HAS_APERFMPERF: AtomicBool = AtomicBool::new(false);

/// Per-core average frequency in MHz (APERF/MPERF ratio)
static CORE_MHZ: [AtomicU32; 256] = {
    const ZERO: AtomicU32 = AtomicU32::new(0);
    [ZERO; 256]
};

/// Per-core CPU usage in percent (0-100)
static CORE_USAGE: [AtomicU32; 256] = {
    const ZERO: AtomicU32 = AtomicU32::new(0);
    [ZERO; 256]
};

/// Per-core cumulative busy TSC cycles (only incremented during actual task work)
static CORE_BUSY_TSC: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};

/// Snapshots for delta computation.
/// Per-core snapshot of CORE_HALT_TSC at the last `update_core_freq`.
/// Two samples and a difference are what turns a cumulative halt counter
/// into a percentage.
static LAST_HALT: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};

static LAST_TSC_CORE: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};
/// APERF/MPERF snapshots (MSR 0xE8 / 0xE7).
/// APERF ticks at actual freq when active, MPERF at nominal TSC rate when active.
/// Ratio APERF/MPERF gives effective running frequency; MPERF/TSC gives activity fraction.
static LAST_APERF: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};
static LAST_MPERF: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};
/// Per-core active flag: true while executing a scheduler task
static CORE_ACTIVE: [AtomicBool; 256] = {
    const FALSE: AtomicBool = AtomicBool::new(false);
    [FALSE; 256]
};

/// Per-core work-start TSC (set when task begins or resumes after wait).
/// Used for checkpoint-based busy tracking in long-running WASM apps.
static WORK_START_TSC: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};

// ── Idle-based instrumentation ─────────────────────────────────────
//
// `CORE_BUSY_TSC` is self-reported: code adds cycles it believes were
// work, so it cannot tell a halted core from a spinning one. These
// counters measure TSC cycles a core spends actually halted (HLT/MWAIT),
// recorded at every halt site. Busy% over a window = 100 − halted%.
// Halt entries expose spurious-wake spin: many entries with tiny
// residency mean the core keeps waking and re-arming.

/// Per-core cumulative TSC cycles spent halted (HLT/MWAIT).
static CORE_HALT_TSC: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};

/// Per-core count of halt entries (each HLT/MWAIT execution).
static CORE_HALT_COUNT: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};

/// Record one genuine idle halt of `cycles` TSC duration on `core_id`.
///
/// Invariant: every site that executes HLT/MWAIT must call this. It is the
/// only signal that distinguishes halted from spinning and the source of
/// the usage figure (`100 − halted%`), so an unreported halt reads as full
/// load.
///
/// A deliberate spin (e.g. draining a moving pointer) must not call it;
/// there the core really is busy.
pub fn record_halt(core_id: usize, cycles: u64) {
    if core_id >= 256 { return; }
    // Clear the in-progress mark before adding: a concurrent snapshot then
    // at worst misses this halt once (and sees it next time), never counts
    // it twice.
    HALT_SINCE[core_id].store(0, Ordering::Relaxed);
    CORE_HALT_TSC[core_id].fetch_add(cycles, Ordering::Relaxed);
    CORE_HALT_COUNT[core_id].fetch_add(1, Ordering::Relaxed);
}

/// TSC at which the halt now in progress began, 0 if the core is not
/// halted in `interrupts::halt_until`.
///
/// A halt is booked when it ends. Workers are tickless and may sleep for
/// seconds, so without this a long halt with no end inside the measuring
/// window would read as 100 % busy.
static HALT_SINCE: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

/// Mark `core_id` as halted from TSC `t0` on (cleared by `record_halt`).
pub fn halt_begin(core_id: usize, t0: u64) {
    if core_id < 256 {
        // The core's running cycles up to here, for a reader on another
        // core (`update_core_freq`): APERF/MPERF are core-local MSRs and
        // stand still while the core is halted, so this snapshot is exact
        // until the core runs again.
        if HAS_APERFMPERF.load(Ordering::Relaxed) {
            let (a, m) = read_aperf_mperf();
            RUN_APERF[core_id].store(a, Ordering::Relaxed);
            RUN_MPERF[core_id].store(m, Ordering::Relaxed);
        }
        HALT_SINCE[core_id].store(t0, Ordering::Relaxed);
    }
}

/// APERF/MPERF as of the core's last halt entry (see `halt_begin`).
static RUN_APERF: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
static RUN_MPERF: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

fn read_aperf_mperf() -> (u64, u64) {
    if !HAS_APERFMPERF.load(Ordering::Relaxed) { return (0, 0); }
    // SAFETY: CPUID.06H:ECX[0] says MSRs 0xE7/0xE8 exist (checked above);
    // reading them has no side effect.
    unsafe { (msr::read(0xE8), msr::read(0xE7)) }
}

/// Cumulative halted TSC for `core_id`, including a halt still in progress.
fn halted_tsc(core_id: usize) -> u64 {
    let acc = CORE_HALT_TSC[core_id].load(Ordering::Relaxed);
    let since = HALT_SINCE[core_id].load(Ordering::Relaxed);
    if since == 0 {
        acc
    } else {
        acc + crate::interrupts::rdtsc().saturating_sub(since)
    }
}

/// Snapshot (cumulative halted TSC, halt-entry count) for `core_id`.
/// Sample twice and diff to get true busy% + halt rate over a window.
pub fn halt_snapshot(core_id: usize) -> (u64, u64) {
    if core_id >= 256 { return (0, 0); }
    (halted_tsc(core_id), CORE_HALT_COUNT[core_id].load(Ordering::Relaxed))
}

// ── Per-source wake attribution ──────────────────────────────────────
//
// Halts/s says how often a core wakes but not why. These counters record
// the cause at each wake: an ISR bumps its vector class as it runs (an
// ISR that runs while the core was halted is the wake that returned the
// HLT). Unattributed = halts − Σcauses. Under KVM a HLT can return with no
// guest ISR (the host resumes the vCPU on its own events); on bare metal
// every wake must attribute to a cause.
pub const WAKE_CAUSES: usize = 4;
pub const WAKE_TIMER: usize = 0;        // 100 Hz PIT / APIC timer ISR (Core 0)
pub const WAKE_KEYBOARD: usize = 1;     // input IRQ: i8042 or xHCI (Core 0)
pub const WAKE_HLT_FALLBACK: usize = 2; // worker idle sti;hlt;cli returned
pub const WAKE_NPK_SLEEP: usize = 3;    // npk_sleep HLT returned

/// Short labels for the `cores` breakdown, indexed by cause.
pub const WAKE_LABELS: [&str; WAKE_CAUSES] = ["timer", "input", "hlt-fb", "npk-sleep"];

static CORE_WAKE: [[AtomicU64; WAKE_CAUSES]; 256] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    const ROW: [AtomicU64; WAKE_CAUSES] = [Z; WAKE_CAUSES];
    [ROW; 256]
};

/// Deadline-timer health per core: one-shot fires, and the worst lateness of
/// a halt that had a deadline (wake TSC − deadline), window peak.
static TIMER_FIRES: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
static DEADLINE_LATE_MAX: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

pub fn note_timer_fire(core_id: usize) {
    if core_id < 256 { TIMER_FIRES[core_id].fetch_add(1, Ordering::Relaxed); }
}

pub fn note_deadline_late(core_id: usize, late_tsc: u64) {
    if core_id < 256 { DEADLINE_LATE_MAX[core_id].fetch_max(late_tsc, Ordering::Relaxed); }
}

/// (fires, late_max_tsc) — late_max is swap-reset so `cores` reads the window peak.
pub fn timer_snapshot(core_id: usize) -> (u64, u64) {
    if core_id >= 256 { return (0, 0); }
    (TIMER_FIRES[core_id].load(Ordering::Relaxed),
     DEADLINE_LATE_MAX[core_id].swap(0, Ordering::Relaxed))
}

/// Cores a hardware driver lives on: a fiber there bound a device or waits
/// on a device interrupt. A vCPU beside it would hold it off for a whole
/// slice. Sticky: drivers stay put.
static DRIVER_CORES: AtomicU64 = AtomicU64::new(0);

pub fn mark_driver_core(core_id: usize) {
    if core_id > 0 && core_id < 64 {
        DRIVER_CORES.fetch_or(1 << core_id, Ordering::Relaxed);
    }
}

pub fn driver_cores() -> u64 { DRIVER_CORES.load(Ordering::Relaxed) }

/// Record one wake of `cause` on `core_id`. Cheap and lock-free, safe to
/// call from interrupt context.
pub fn record_wake(core_id: usize, cause: usize) {
    if core_id >= 256 || cause >= WAKE_CAUSES { return; }
    CORE_WAKE[core_id][cause].fetch_add(1, Ordering::Relaxed);
}

/// Snapshot all wake-cause counters for `core_id`.
pub fn wake_snapshot(core_id: usize) -> [u64; WAKE_CAUSES] {
    let mut out = [0u64; WAKE_CAUSES];
    if core_id >= 256 { return out; }
    for i in 0..WAKE_CAUSES {
        out[i] = CORE_WAKE[core_id][i].load(Ordering::Relaxed);
    }
    out
}

/// Whether `core_id` is currently inside a scheduler task (CORE_ACTIVE).
pub fn is_active(core_id: usize) -> bool {
    if core_id >= 256 { return false; }
    CORE_ACTIVE[core_id].load(Ordering::Relaxed)
}

/// Platform frequency limits (set once by enable_hwp)
static MAX_TURBO_MHZ: AtomicU32 = AtomicU32::new(0);
static MIN_EFF_MHZ: AtomicU32 = AtomicU32::new(0);

pub fn register_bsp(apic_id: u32) {
    CORES.lock().push(CoreInfo { id: 0, apic_id, state: CoreState::Bsp });
    map_apic(apic_id, 0);
    // Detect MONITOR/MWAIT support via CPUID.01H:ECX bit 3
    let ecx: u32;
    // SAFETY: CPUID leaf 1 exists everywhere; rbx is reserved by LLVM and
    // saved by hand.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {0:e}, ecx",
            "pop rbx",
            out(reg) ecx,
            out("eax") _,
            out("edx") _,
        );
    }
    HAS_MWAIT.store(ecx & (1 << 3) != 0, Ordering::Release);

    // Detect IA32_APERF/IA32_MPERF via CPUID.06H:ECX[0]
    // Absent on AMD and on KVM guest w/ generic `-cpu qemu64`.
    let ecx: u32;
    // SAFETY: CPUID leaf 6 exists on every CPU we boot on; rbx is saved by
    // hand.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 6",
            "cpuid",
            "mov {0:e}, ecx",
            "pop rbx",
            out(reg) ecx,
            out("eax") _,
            out("edx") _,
        );
    }
    HAS_APERFMPERF.store(ecx & 1 != 0, Ordering::Release);
    rapl_probe();
}

// ── RAPL: AMD package and core energy counters ───────────────────────
//
// Separates the CPU package's power draw from the rest of the platform.
// Ported from Linux 6.18. Feature bit from `arch/x86/kernel/cpu/amd.c`:
//
//     /* Bit 14 indicates the Runtime Average Power Limit interface. */
//     if (c->x86_power & BIT(14)) set_cpu_cap(c, X86_FEATURE_RAPL);
//
// where `x86_power` = CPUID Fn8000_0007_EDX. MSR numbers from
// `arch/x86/include/asm/msr-index.h`, the unit field from
// `arch/x86/events/rapl.c` (`(msr_rapl_power_unit_bits >> 8) & 0x1F`).
const MSR_AMD_RAPL_POWER_UNIT: u32 = 0xC001_0299;
const MSR_AMD_CORE_ENERGY_STATUS: u32 = 0xC001_029A;
const MSR_AMD_PKG_ENERGY_STATUS: u32 = 0xC001_029B;

static HAS_RAPL: AtomicBool = AtomicBool::new(false);
/// Nanojoules per counter step. The unit is 2^-ESU joules; in nJ it stays
/// an integer (ESU 16 -> 15258 nJ), so no floating point is needed.
static RAPL_NJ_PER_UNIT: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
enum RaplMsr {
    PowerUnit,
    PkgEnergy,
    CoreEnergy,
}

/// Low dword of a RAPL MSR, 0 without RAPL. Core-local: measures the core
/// it runs on.
fn rapl_read(r: RaplMsr) -> u32 {
    if !has_rapl() { return 0; }
    let m = match r {
        RaplMsr::PowerUnit => MSR_AMD_RAPL_POWER_UNIT,
        RaplMsr::PkgEnergy => MSR_AMD_PKG_ENERGY_STATUS,
        RaplMsr::CoreEnergy => MSR_AMD_CORE_ENERGY_STATUS,
    };
    // SAFETY: `has_rapl()` (CPUID 0x80000007:EDX[14]) says these MSRs exist
    // on every core of this socket; reading them has no side effect.
    unsafe { msr::read(m) as u32 }
}

fn rapl_probe() {
    let edx: u32;
    // SAFETY: CPUID 0x80000007 is valid on every x86-64; rbx is reserved by
    // LLVM, so it is saved by hand.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0x80000007",
            "cpuid",
            "pop rbx",
            out("edx") edx,
            out("eax") _,
            out("ecx") _,
        );
    }
    if edx & (1 << 14) == 0 { return; }
    HAS_RAPL.store(true, Ordering::Release);
    // Only read after the feature check; before it this would #GP.
    let esu = (rapl_read(RaplMsr::PowerUnit) >> 8) & 0x1F;
    // 2^-ESU joules in nanojoules: 1e9 >> ESU. An absurd ESU (0 or > 30)
    // would give nonsense, so report no RAPL instead.
    if esu == 0 || esu > 30 {
        HAS_RAPL.store(false, Ordering::Release);
        return;
    }
    RAPL_NJ_PER_UNIT.store((1_000_000_000u64 >> esu) as u32, Ordering::Release);
}

pub fn has_rapl() -> bool { HAS_RAPL.load(Ordering::Acquire) }

/// Nanojoules per counter step (0 = no RAPL).
pub fn rapl_nj_per_unit() -> u32 { RAPL_NJ_PER_UNIT.load(Ordering::Relaxed) }

/// Raw package energy counter. 32 bits and wraps: always take the
/// difference of two samples (`wrapping_sub`).
pub fn rapl_pkg_raw() -> u32 {
    if !has_rapl() { return 0; }
    rapl_read(RaplMsr::PkgEnergy)
}

/// The same for the core this call runs on.
pub fn rapl_core_raw() -> u32 {
    if !has_rapl() { return 0; }
    rapl_read(RaplMsr::CoreEnergy)
}

/// Milliwatts from a counter delta and a window: mW = nJ / us, so no
/// conversion constant is needed.
pub fn rapl_mw(delta_units: u32, window_us: u64) -> u64 {
    let nj = rapl_nj_per_unit() as u64;
    if nj == 0 || window_us == 0 { return 0; }
    (delta_units as u64).saturating_mul(nj) / window_us
}

pub fn register_ap(apic_id: u32, core_id: u32) {
    let mut cores = CORES.lock();
    cores.push(CoreInfo { id: core_id, apic_id, state: CoreState::Online });
    map_apic(apic_id, core_id);
    CORE_COUNT.store(cores.len(), Ordering::Release);
}

pub fn mark_failed(apic_id: u32) {
    if let Some(c) = CORES.lock().iter_mut().find(|c| c.apic_id == apic_id) {
        c.state = CoreState::Failed;
    }
}

/// Total cores (BSP + online APs)
pub fn core_count() -> usize {
    CORE_COUNT.load(Ordering::Acquire)
}

/// Signal APs to start their scheduler loops
pub fn start_scheduler() {
    SCHEDULER_READY.store(true, Ordering::Release);
}

// ── Dedicated microvm core ─────────────────────────────────────────

/// Sentinel: no core is dedicated → microvm stays cooperatively
/// time-sliced on Core 0.
pub const NO_DEDICATED_CORE: u32 = u32::MAX;

/// Worker core exclusively reserved for the microvm. It is carved out
/// of the pool: it never calls `next_task`, so no task is ever assigned
/// to it and the scheduler needs no other change. The guest's
/// VMRESUME/VMRUN loop runs here instead of competing with the compositor
/// and the shell for Core 0.
pub static DEDICATED_VM_CORE: AtomicU32 = AtomicU32::new(NO_DEDICATED_CORE);

/// Master switch for the dedicated-core path. `false` → the microvm
/// always stays cooperatively time-sliced on Core 0, regardless of core
/// count. `true` → the vendor and core-count gates below decide.
/// Only consulted when the vCPU does not run as a pool fiber.
pub const A2_DEDICATED_CORE_ENABLED: bool = true;

/// Minimum logical cores (incl. BSP) before dedicating one to the
/// microvm — below this, dedicating would starve the rest, so keep
/// cooperative Core-0 slicing.
const MIN_CORES_FOR_DEDICATED: usize = 3;

/// Decide the dedicated VM core from the live core count. Called once
/// right after `scheduler::init` (so WORKER_COUNT is known). Picks the
/// highest worker id (= `worker_count`, since worker ids are
/// `1..=worker_count`) so the low cores keep taking work; Core 0 stays
/// the IRQ/compositor/intent core regardless.
pub fn init_dedicated_vm_core(worker_count: usize) {
    let total = worker_count + 1; // + BSP

    // The dedicated-core path is validated on AMD-SVM only; the VMX
    // equivalents (VMCS host-state lifecycle, IDT vectoring info, MSR
    // auto-save/load) are unaudited, so Intel falls back to cooperative
    // Core-0 slicing. `detect_vendor` is stateless and safe to call before
    // `microvm::cpu::init`.
    let vendor = crate::microvm::cpu::detect_vendor();
    let is_amd = matches!(vendor, crate::microvm::cpu::Vendor::Amd);
    let is_intel = matches!(vendor, crate::microvm::cpu::Vendor::Intel);

    // vCPU-as-fiber: the guest runs as a normal pool fiber on a dynamic
    // core, so no core is carved out (a carve-out wastes a core while no VM
    // runs). See microvm::cpu and docs/plan/SCHEDULER_FIBERS.md. On VMX this
    // is flag-gated; the per-core TSS VMX host-state needs is installed
    // lazily in vmx::vm_open.
    let fiber_mode = crate::microvm::cpu::VCPU_AS_FIBER
        && worker_count >= 1
        && (is_amd || (is_intel && crate::microvm::cpu::VMX_VCPU_AS_FIBER));
    crate::microvm::cpu::set_vm_fiber_mode(fiber_mode);
    if fiber_mode {
        crate::kdebug!(
            "[npk] smp: microvm runs as a pool fiber on a dynamic core ({} cores, no carve-out)",
            total
        );
        return;
    }

    let dedicate = A2_DEDICATED_CORE_ENABLED
        && is_amd
        && worker_count >= 1
        && total >= MIN_CORES_FOR_DEDICATED;

    if dedicate {
        DEDICATED_VM_CORE.store(worker_count as u32, Ordering::Release);
        crate::kprintln!(
            "[npk] smp: core {} dedicated to microvm ({} cores, carved out of work-stealing)",
            worker_count, total
        );
    } else if !is_amd && total >= MIN_CORES_FOR_DEDICATED {
        crate::kprintln!(
            "[npk] smp: {} core(s) — A2 disabled (vendor-gated, unvalidated on this CPU) → microvm cooperative on Core 0",
            total
        );
    } else {
        crate::kprintln!(
            "[npk] smp: {} core(s) → microvm stays cooperative on Core 0",
            total
        );
    }
}

/// The dedicated microvm core id, or `None` if cooperative Core-0.
pub fn dedicated_vm_core() -> Option<usize> {
    match DEDICATED_VM_CORE.load(Ordering::Acquire) {
        NO_DEDICATED_CORE => None,
        c => Some(c as usize),
    }
}

/// Check if MONITOR/MWAIT is available
pub fn has_mwait() -> bool {
    HAS_MWAIT.load(Ordering::Relaxed)
}

/// Update per-core CPU usage and frequency.
///
/// Usage:     explicit busy-TSC tracking (task execution time / total time).
/// Activity:  MPERF/TSC ratio — hardware-measured fraction of wall clock the
///            core was not halted. Captures idle that scheduler tracking misses
///            (e.g. Core 0 in HLT between events).
/// Freq:      APERF/MPERF * nominal_MHz — effective frequency while running.
///            Both MSRs stop ticking during C-states, so ratio is independent
///            of idle; captures true average P-state when active.
///
/// Callable from any core: the halt counter is shared, and APERF/MPERF of
/// another core come from the snapshot it takes at every halt entry, so a
/// sleeping core still gets a current value. Relies on one TSC across
/// cores (`smp::init` corrects core 0 at boot).
pub fn update_core_freq(core_id: usize) {
    if core_id >= 256 { return; }

    // Evaluate only windows of at least 100 ms. The worker loop calls this
    // on every pass, and two passes back to back would make a microsecond
    // window that reads 100 % busy. Until the window is long enough, keep
    // accumulating.
    let prev = LAST_TSC_CORE[core_id].load(Ordering::Relaxed);
    if prev != 0
        && crate::interrupts::rdtsc().wrapping_sub(prev) < crate::interrupts::tsc_freq() / 10
    {
        return;
    }

    // APERF/MPERF are gated by CPUID.06H:ECX[0] — absent on AMD and on KVM
    // guests with `-cpu qemu64`. Reading them unconditionally raises #GP.
    let has_apmp = HAS_APERFMPERF.load(Ordering::Relaxed);

    let (aperf, mperf): (u64, u64) = if !has_apmp {
        (0, 0)
    } else if core_id == current_core_id() {
        read_aperf_mperf()
    } else {
        (RUN_APERF[core_id].load(Ordering::Relaxed), RUN_MPERF[core_id].load(Ordering::Relaxed))
    };
    let tsc = crate::interrupts::rdtsc();

    let halted = halted_tsc(core_id);
    let prev_halted = LAST_HALT[core_id].swap(halted, Ordering::Relaxed);
    let prev_tsc = LAST_TSC_CORE[core_id].swap(tsc, Ordering::Relaxed);
    let prev_aperf = LAST_APERF[core_id].swap(aperf, Ordering::Relaxed);
    let prev_mperf = LAST_MPERF[core_id].swap(mperf, Ordering::Relaxed);

    if prev_tsc == 0 { return; } // First call — seed only

    let delta_tsc = tsc.wrapping_sub(prev_tsc);
    let delta_aperf = aperf.wrapping_sub(prev_aperf);
    let delta_mperf = mperf.wrapping_sub(prev_mperf);
    let delta_halt = halted.wrapping_sub(prev_halted);

    if delta_tsc == 0 { return; }

    // Usage = 100 − halted%, the same definition `intent_cores` prints.
    // Measured, not self-reported: a spinner never halts and shows ~100 %,
    // an idle core shows ~0 %.
    let halt_pct = ((delta_halt as u128) * 100 / (delta_tsc as u128)).min(100) as u32;
    CORE_USAGE[core_id].store(100 - halt_pct, Ordering::Relaxed);

    // Effective running frequency: APERF/MPERF * nominal_TSC_freq.
    // TSC is calibrated to nominal base; MPERF ticks at that same rate.
    // A core that did not run in the window has no frequency: 0 = asleep.
    if delta_mperf == 0 {
        CORE_MHZ[core_id].store(0, Ordering::Relaxed);
    } else {
        let nominal_mhz = (crate::interrupts::tsc_freq() / 1_000_000) as u64;
        // Guard against overflow: cap aperf delta ratio implicitly via u128.
        let eff_mhz = ((delta_aperf as u128) * (nominal_mhz as u128)
                       / (delta_mperf as u128)) as u64;
        CORE_MHZ[core_id].store(eff_mhz.min(u32::MAX as u64) as u32, Ordering::Relaxed);
    }
}

/// Start tracking work time for a core (called when task begins or resumes).
pub fn start_work(core_id: usize) {
    if core_id >= 256 { return; }
    WORK_START_TSC[core_id].store(crate::interrupts::rdtsc(), Ordering::Relaxed);
}

/// Flush accumulated work time since last start_work (called before wait/idle).
/// Returns the flushed cycles count.
pub fn flush_busy(core_id: usize) -> u64 {
    if core_id >= 256 { return 0; }
    let start = WORK_START_TSC[core_id].swap(0, Ordering::Relaxed);
    if start == 0 { return 0; }
    let elapsed = crate::interrupts::rdtsc().saturating_sub(start);
    CORE_BUSY_TSC[core_id].fetch_add(elapsed, Ordering::Relaxed);
    elapsed
}

/// Set per-core active flag (true = executing work, false = waiting/idle).
pub fn set_active(core_id: usize, active: bool) {
    if core_id < 256 {
        CORE_ACTIVE[core_id].store(active, Ordering::Relaxed);
    }
}

/// Get last measured frequency in MHz for a core.
pub fn core_freq_mhz(core_id: usize) -> u32 {
    if core_id >= 256 { return 0; }
    CORE_MHZ[core_id].load(Ordering::Relaxed)
}

/// Max turbo frequency in MHz (from HWP capabilities).
pub fn max_turbo_mhz() -> u32 { MAX_TURBO_MHZ.load(Ordering::Relaxed) }

/// Min efficiency frequency in MHz (from HWP capabilities).
pub fn min_eff_mhz() -> u32 { MIN_EFF_MHZ.load(Ordering::Relaxed) }

/// Per-core CPU usage in percent (0-100): 100 − halted% over the last
/// sampling window. Measured, not self-reported.
pub fn core_usage(core_id: usize) -> u32 {
    if core_id >= 256 { return 0; }
    CORE_USAGE[core_id].load(Ordering::Relaxed)
}

/// Cached APIC base (MSR 0x1B). Identical on every core and never changes, so
/// reading it once avoids an `rdmsr` — a VM-exit under virtualization — on every
/// current_core_id() call (hot: poll loops, idle, core-gating).
static APIC_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Read current core's sequential ID via LAPIC.
pub fn current_core_id() -> usize {
    use core::sync::atomic::Ordering::Relaxed;
    let base = APIC_BASE.load(Relaxed);
    let lapic = if base == 0 {
        let l = crate::interrupts::lapic_from_msr();
        APIC_BASE.store(l.base(), Relaxed);
        l
    } else {
        // SAFETY: `APIC_BASE` holds only bases from
        // `interrupts::lapic_from_msr`, the xAPIC register page.
        unsafe { Mmio::from_mapped(base, 0x1000) }
    };
    // The per-core APIC ID register.
    let apic_id = lapic.r32(crate::interrupts::LAPIC_ID) >> 24;
    // An unregistered id reads as core 0.
    APIC_TO_CORE[(apic_id & 0xFF) as usize].load(Relaxed) as usize
}

/// xAPIC id → sequential core id, filled once per core at registration.
///
/// Lets `current_core_id` avoid a lock on the hottest paths (`poll_rx_only`,
/// `pump_peers`, every fiber yield). The mapping never changes after boot.
/// The xAPIC id register holds 8 bits, hence 256 entries.
static APIC_TO_CORE: [core::sync::atomic::AtomicU16; 256] =
    [const { core::sync::atomic::AtomicU16::new(0) }; 256];

/// Sequential core id → xAPIC id, the reverse of `APIC_TO_CORE`.
static CORE_APIC: [AtomicU32; 256] = [const { AtomicU32::new(0) }; 256];

fn map_apic(apic_id: u32, core_id: u32) {
    APIC_TO_CORE[(apic_id & 0xFF) as usize].store(core_id as u16, Ordering::Release);
    if (core_id as usize) < 256 {
        CORE_APIC[core_id as usize].store(apic_id, Ordering::Release);
    }
}

/// Enable Hardware P-states (HWP / Speed Shift) on the current core.
/// CPU automatically scales frequency: idle → min, load → turbo.
/// Returns true if HWP was enabled successfully.
pub fn enable_hwp() -> bool {
    // Check HWP support: CPUID.06H:EAX bit 7
    let eax: u32;
    // SAFETY: CPUID leaf 6 exists on every CPU we boot on; rbx is saved by
    // hand.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 6",
            "cpuid",
            "mov {0:e}, eax",
            "pop rbx",
            out(reg) eax,
            out("ecx") _,
            out("edx") _,
        );
    }
    if eax & (1 << 7) == 0 { return false; }

    // Enable HWP: MSR 0x770 (IA32_PM_ENABLE) = 1
    // SAFETY: HWP supported (checked above); enabling it only hands
    // P-state selection to the CPU.
    unsafe { msr::write(0x770, 1) };

    // Read HWP capabilities: MSR 0x771 (IA32_HWP_CAPABILITIES)
    // [7:0]=Highest, [15:8]=Guaranteed, [23:16]=Efficient, [31:24]=Lowest
    // SAFETY: MSR 0x771 exists when HWP is supported; read-only.
    let cap_lo = unsafe { msr::read(0x771) } as u32;
    let highest = cap_lo & 0xFF;
    let lowest = (cap_lo >> 24) & 0xFF;

    // Store platform limits (first caller wins)
    if MAX_TURBO_MHZ.load(Ordering::Relaxed) == 0 {
        MAX_TURBO_MHZ.store(highest * 100, Ordering::Relaxed);
        MIN_EFF_MHZ.store(lowest * 100, Ordering::Relaxed);
    }

    // Configure HWP request: MSR 0x774 (IA32_HWP_REQUEST)
    // [7:0]=Min, [15:8]=Max, [23:16]=Desired(0=auto), [31:24]=EPP
    // EPP range: 0=perf, 128=balanced, 192=power_save, 255=max_power_save
    //
    // EPP=0 (max perf): the CPU ramps to turbo on the first instruction of a
    // burst. Bursty workloads (crypto, I/O batches) last ~10-100 ms, too
    // short for the firmware heuristics at EPP=192 to ramp up. Battery-
    // powered targets may prefer 128 or 192; not configurable yet.
    let hwp_req = (lowest as u32)
        | ((highest as u32) << 8)
        | (0u32 << 24);
    // SAFETY: MSR 0x774 exists when HWP is enabled; min/max come from the
    // CPU's own capabilities.
    unsafe { msr::write(0x774, hwp_req as u64) };

    true
}

/// AMD Zen C-state base address (MSRC001_0073 CStateBaseAddr, bits 15:0):
/// an I/O read of CStateBaseAddr+n is trapped by the core as a request for
/// C-state n (ACPI `_CST` lists these ports as SystemIO entries). None off
/// AMD, before family 17h, under a hypervisor (the MSR is not emulated and
/// a read would #GP), or when the firmware left it 0 (no trapping).
pub fn amd_cstate_base() -> Option<u16> {
    if !matches!(crate::microvm::cpu::detect_vendor(), crate::microvm::cpu::Vendor::Amd) {
        return None;
    }
    let (eax, ecx): (u32, u32);
    // SAFETY: CPUID leaf 1 exists everywhere; rbx is reserved by LLVM.
    unsafe {
        core::arch::asm!("push rbx", "mov eax, 1", "cpuid", "pop rbx",
            out("eax") eax, out("ecx") ecx, out("edx") _);
    }
    if ecx & (1 << 31) != 0 { return None; } // hypervisor
    let base_fam = (eax >> 8) & 0xF;
    let family = if base_fam == 0xF { base_fam + ((eax >> 20) & 0xFF) } else { base_fam };
    if family < 0x17 { return None; }
    // SAFETY: MSRC001_0073 is architectural on AMD family 17h+ (PPR
    // Core::X86::Msr::CStateBaseAddr), gated on vendor, family and bare
    // metal; reading it has no side effect.
    let lo = unsafe { msr::read(0xC001_0073) } as u32;
    let base = (lo & 0xFFFF) as u16;
    if base == 0 || base > 0xFFF8 { None } else { Some(base) }
}

/// The worker has reached its loop. An AP that never started must not count
/// as the least loaded core: it never takes anything, so work would stall.
static IN_LOOP: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
/// TSC stamped by a core first thing after `hlt` (the `tsc_offset` probe).
static WOKE_TSC: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

pub fn note_woke(c: usize) {
    if c < 256 {
        WOKE_TSC[c].store(crate::interrupts::rdtsc(), Ordering::Release);
    }
}

/// Signed TSC offset of core `c` against the caller's TSC, in cycles, by a
/// wake-IPI round trip: the woken core stamps its own TSC first thing after
/// `hlt`; the caller's midpoint of send and receipt is the same instant ±
/// half the round trip. Returns (offset, round trip). None if `c` is not
/// halted or did not answer. Costs `c` one wake.
pub fn tsc_offset(c: usize) -> Option<(i64, u64)> {
    if c >= 256 || c == current_core_id() || !IDLE[c].load(Ordering::SeqCst) {
        return None;
    }
    let before = WOKE_TSC[c].load(Ordering::Acquire);
    let t_send = crate::interrupts::rdtsc();
    super::send_wake_ipi(CORE_APIC[c].load(Ordering::Relaxed));
    for _ in 0..10_000_000u32 {
        let w = WOKE_TSC[c].load(Ordering::Acquire);
        if w != before {
            let t_recv = crate::interrupts::rdtsc();
            let mid = t_send / 2 + t_recv / 2;
            return Some((w as i64 - mid as i64, t_recv.wrapping_sub(t_send)));
        }
        core::hint::spin_loop();
    }
    None
}

/// Worker is halted in its idle path and needs an IPI to see new work.
static IDLE: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];

/// New work is in the inbox: send the wake IPI to every idle worker. Each
/// one re-runs its loop; the placement rule (`least_loaded`) picks who takes
/// it, the others halt again. Spawns are rare, so waking all idle cores is
/// cheaper than guessing the right one and stranding the work on a miss.
pub fn wake_idle_workers() {
    let workers = super::scheduler::worker_count();
    for c in 1..=workers.min(255) {
        wake_core(c);
    }
}

/// Wake worker `c` if it is halted in its idle path. The caller has already
/// published what `c` should find (a queued fiber, inbox work) — `c` sets
/// IDLE before its last look, so either it sees the work or we see IDLE.
pub fn wake_core(c: usize) {
    if c >= 256 {
        return;
    }
    if IDLE[c].load(Ordering::SeqCst) && current_core_id() != c {
        super::send_wake_ipi(CORE_APIC[c].load(Ordering::Relaxed));
    }
}

/// Core 0's loop after boot: the same fiber scheduler as a worker, with the
/// shell (`intent::run_loop`) as a fiber. No periodic tick: the halt ends at
/// the earliest fiber deadline, on an interrupt, or by the wake IPI when
/// another core signals a fiber here. Core 0 takes no inbox work.
pub fn core0_loop() -> ! {
    // No periodic tick from here on: Core 0's LAPIC timer becomes
    // the one-shot deadline timer every worker has.
    crate::interrupts::make_core0_tickless();
    loop {
        super::fiber::run_core_fibers(0);
        // Self-throttled to 100 ms windows.
        update_core_freq(0);
        // IF=0 from the last look to the halt: an ISR here that signals a
        // fiber on this core sends no IPI (`wake_core`), so it must stay
        // pending and end the halt rather than run before it.
        let flags = crate::interrupts::irq_save();
        IDLE[0].store(true, Ordering::SeqCst);
        let now = crate::interrupts::rdtsc();
        match super::fiber::earliest_deadline(0) {
            Some(d) if d <= now => {}
            wake => crate::interrupts::halt_until(wake, WAKE_HLT_FALLBACK),
        }
        IDLE[0].store(false, Ordering::Relaxed);
        crate::interrupts::irq_restore(flags);
    }
}

/// An intent is running on this core (to completion, without yielding).
static NATIVE_BUSY: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
/// The native task each core runs, as a `&'static str` split in two words.
static NATIVE_NAME: [(AtomicUsize, AtomicUsize); 256] =
    [const { (AtomicUsize::new(0), AtomicUsize::new(0)) }; 256];

/// The native task core `c` is running right now, if any (`cores`).
pub fn native_task(c: usize) -> Option<&'static str> {
    if c >= 256 || !NATIVE_BUSY[c].load(Ordering::Acquire) {
        return None;
    }
    let (p, l) = (&NATIVE_NAME[c].0, &NATIVE_NAME[c].1);
    let (p, l) = (p.load(Ordering::Acquire), l.load(Ordering::Acquire));
    if p == 0 { return None; }
    // SAFETY: stored from a `&'static str` in the worker loop below; the
    // pointer and length belong together and outlive everything.
    Some(unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(p as *const u8, l)) })
}

/// A core's load for placement: resident fibers, plus one while an intent
/// occupies it.
fn core_load(c: usize) -> u64 {
    super::fiber::fiber_count(c) + NATIVE_BUSY[c].load(Ordering::Relaxed) as u64
}

/// Whether `cid` is among the least loaded workers that take work.
///
/// The NIC driver's core and the microVM core are excluded: they never take
/// anything, so the minimum could otherwise sit on a core that never takes
/// it. Ties are allowed; several empty cores may race for the inbox.
fn least_loaded(cid: usize) -> bool {
    let workers = super::scheduler::worker_count();
    if workers <= 1 {
        return true;
    }
    let nic = crate::netdev::wasm_nic_core();
    let vm = DEDICATED_VM_CORE.load(Ordering::Acquire) as usize;
    let mine = core_load(cid);
    for c in 1..=workers.min(255) {
        // A core inside an intent cannot take anything right now; counting
        // it would make new work wait for that intent to finish.
        if c == cid || Some(c) == nic || c == vm || !IN_LOOP[c].load(Ordering::Acquire)
            || NATIVE_BUSY[c].load(Ordering::Relaxed)
        {
            continue;
        }
        if core_load(c) < mine {
            return false;
        }
    }
    true
}

/// AP Rust entry — called by trampoline after long mode transition.
/// Interrupts are disabled (cli from trampoline). IDT is loaded.
#[unsafe(no_mangle)]
pub extern "C" fn smp_ap_entry(core_id: u32) -> ! {
    // Own GDT + TSS before anything can fault: the shared IDT sends a
    // double fault to IST1, which only exists once this core has a TSS.
    crate::tss::init_core();

    // Enable Local APIC (needed for IPI wakeup fallback)
    // Each core sees its own LAPIC.
    let lapic = crate::interrupts::lapic_from_msr();
    let svr = lapic.r32(crate::interrupts::LAPIC_SVR);
    lapic.w32(crate::interrupts::LAPIC_SVR, svr | (1 << 8) | crate::interrupts::SPURIOUS_VECTOR as u32);

    crate::cpu_errata::apply();

    // Enable HWP on this AP (per-core frequency scaling)
    enable_hwp();

    // Signal BSP: this AP is alive
    super::AP_STARTED.fetch_add(1, Ordering::Release);

    // Wait until scheduler is initialized
    while !SCHEDULER_READY.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    // This core's own LAPIC timer, one-shot to whatever deadline it waits
    // for: its idle HLT needs its own wake source. Plain HLT VM-exits, so
    // KVM frees the host core (MWAIT would need cpu-pm=on). New work wakes
    // it by IPI (`wake_idle_workers`).
    crate::interrupts::init_worker_timer();
    if (core_id as usize) < 256 {
        IN_LOOP[core_id as usize].store(true, Ordering::Release);
    }

    // Enter scheduler loop
    let cid = core_id as usize;

    loop {
        crate::mm::paging::sync_tlb();
        // Carved out of the pool for the microvm. `vm_core_serve` opens the
        // guest on this core when a launch is pending and runs it to exit,
        // blocking the core for the guest's lifetime. Cheap no-op when
        // nothing is pending; we halt and re-check within ~10 ms. The
        // default sentinel never matches a real cid.
        if DEDICATED_VM_CORE.load(Ordering::Acquire) == cid as u32 {
            CORE_ACTIVE[cid].store(true, Ordering::Relaxed);
            crate::microvm::vm_core_serve();
            CORE_ACTIVE[cid].store(false, Ordering::Relaxed);
            update_core_freq(cid);
            // Re-check for a pending launch every 10 ms.
            let d = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 100;
            crate::interrupts::halt_until(Some(d), WAKE_HLT_FALLBACK);
            continue;
        }

        // The NIC driver's core takes no new work while other workers exist.
        // It wakes every millisecond and would otherwise win nearly every
        // new intent, including the one consuming the driver's frames; an
        // intent runs to completion, so driver and consumer would alternate
        // instead of running side by side.
        let nic_core = crate::netdev::wasm_nic_core() == Some(cid)
            && super::scheduler::worker_count() > 1;
        // In general, only a core among the least loaded takes new work;
        // otherwise the core that wakes most often collects every fiber, and
        // cooperative fibers then starve each other. A core with a vCPU or VM
        // worker takes nothing new: next to a spinning vCPU it would barely
        // get to run.
        let take = !nic_core && !crate::microvm::cpu::is_vm_worker_core(cid)
            && least_loaded(cid);
        // Admit a freshly-spawned app as a fiber, or run a native intent.
        // New work arrives in the shared inbox (`scheduler::spawn`).
        if let Some(task) = if take { super::scheduler::next_task(cid) } else { None } {
            if task.is_fiber {
                // App: hand to this core's fiber scheduler. It runs on its
                // own stack and yields at npk_sleep so peers share the core.
                super::fiber::admit(cid, task.func, task.arg);
            } else {
                // Native run-to-completion task (intent) — run directly.
                CORE_ACTIVE[cid].store(true, Ordering::Relaxed);
                NATIVE_NAME[cid].0.store(task.name.as_ptr() as usize, Ordering::Relaxed);
                NATIVE_NAME[cid].1.store(task.name.len(), Ordering::Relaxed);
                NATIVE_BUSY[cid].store(true, Ordering::Release);
                start_work(cid);
                (task.func)(task.arg);
                flush_busy(cid);
                NATIVE_BUSY[cid].store(false, Ordering::Relaxed);
                CORE_ACTIVE[cid].store(false, Ordering::Relaxed);
            }
            continue;
        }

        // Run this core's fibers round-robin: resume any whose sleep
        // deadline passed, park the rest. Returns when all are parked /
        // none remain — then the core halts below.
        CORE_ACTIVE[cid].store(true, Ordering::Relaxed);
        start_work(cid);
        super::fiber::run_core_fibers(cid);
        flush_busy(cid);
        CORE_ACTIVE[cid].store(false, Ordering::Relaxed);

        // Before sleep: update usage stats (delta covers work + idle since last call)
        update_core_freq(cid);

        // Idle: halt until the earliest parked fiber is due, or until an
        // interrupt — a device IRQ for a fiber in `irq_wait`, a kick, or
        // the wake IPI for new work. No periodic tick.
        //
        // IDLE is published before the inbox is looked at again: a spawn
        // that pushed without seeing IDLE sent no IPI, so its work must be
        // seen by this re-check; one that saw IDLE sends the IPI, which
        // stays pending (IF=0) and ends the halt below at once. The same
        // holds for an ISR on this core that signals one of its fibers.
        let flags = crate::interrupts::irq_save();
        IDLE[cid].store(true, Ordering::SeqCst);
        let now = crate::interrupts::rdtsc();
        let mut wake = super::fiber::earliest_deadline(cid);
        if matches!(wake, Some(d) if d <= now) {
            IDLE[cid].store(false, Ordering::Relaxed);
            crate::interrupts::irq_restore(flags);
            continue; // became runnable — round again
        }
        if super::scheduler::has_work() {
            if !nic_core && least_loaded(cid) {
                IDLE[cid].store(false, Ordering::Relaxed);
                crate::interrupts::irq_restore(flags);
                continue;
            }
            // Work waits that this core declines — for another core. If
            // that one is busy inside its fibers it takes the work only
            // when it gets back to the top of its loop; look again in
            // 10 ms so the work cannot be stranded behind a changed load.
            let recheck = now + crate::interrupts::tsc_freq() / 100;
            wake = Some(wake.map_or(recheck, |d| d.min(recheck)));
        }
        crate::interrupts::halt_until(wake, WAKE_HLT_FALLBACK);
        IDLE[cid].store(false, Ordering::Relaxed);
        crate::interrupts::irq_restore(flags);
    }
}
