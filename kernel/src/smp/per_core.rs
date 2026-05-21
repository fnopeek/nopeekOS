//! Per-Core State
//!
//! Tracks CPU cores discovered at boot. Dynamically sized — no hardcoded limit.
//! Scales from 1 (BSP only) to 1024+ cores.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

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

/// Snapshots for delta computation
static LAST_BUSY: [AtomicU64; 256] = {
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

/// Platform frequency limits (set once by enable_hwp)
static MAX_TURBO_MHZ: AtomicU32 = AtomicU32::new(0);
static MIN_EFF_MHZ: AtomicU32 = AtomicU32::new(0);

pub fn register_bsp(apic_id: u32) {
    CORES.lock().push(CoreInfo { id: 0, apic_id, state: CoreState::Bsp });
    // Detect MONITOR/MWAIT support via CPUID.01H:ECX bit 3
    let ecx: u32;
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
}

pub fn register_ap(apic_id: u32, core_id: u32) {
    let mut cores = CORES.lock();
    cores.push(CoreInfo { id: core_id, apic_id, state: CoreState::Online });
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

// ── Dedicated microvm core (substrate rework A1) ───────────────────

/// Sentinel: no core is dedicated → microvm stays cooperatively
/// time-sliced on Core 0 (the validated path on ≤2-core hosts).
pub const NO_DEDICATED_CORE: u32 = u32::MAX;

/// Worker core exclusively reserved for the microvm. It is carved out
/// of the work-stealing pool: it never calls `next_task`, so no task
/// is ever assigned to it (`spawn` only pushes to BSP's deque; nobody
/// `spawn_local`s onto it) and the scheduler needs no other change.
/// A1 just parks this core (proves the carve-out + stats); A2 runs the
/// guest VMRESUME/VMRUN loop here so the guest no longer fights Shade
/// + the shell for Core 0.
pub static DEDICATED_VM_CORE: AtomicU32 = AtomicU32::new(NO_DEDICATED_CORE);

/// A2 master switch (mirrors `guest_mem::DEMAND_ENABLED` for B3).
/// `false` → the microvm ALWAYS stays cooperatively time-sliced on
/// Core 0, regardless of core count; the A1/A2 dedicated-core code
/// stays in-tree but dormant. `true` → the core-count gate below
/// decides.
///
/// Gated OFF after the 2026-05-19 QEMU_SMP bisection: with A2 the
/// guest dies in process-agnostic musl/near-null corruption at ≈6 s
/// before any page loads; with the cooperative Core-0 path the
/// browser surfs real multi-site traffic (DuckDuckGo/Google,
/// ~74 s+). So A2 (dedicated core + per-core LAPIC timer + the new
/// inject path) is a correctness REGRESSION, not the validated step
/// the plan assumed. The cooperative path is the working baseline;
/// A2's cadence benefit is deferred until its guest-corruption root
/// cause is found and fixed. Flip to `true` only for A2 debugging.
pub const A2_DEDICATED_CORE_ENABLED: bool = true;

/// Minimum logical cores (incl. BSP) before dedicating one to the
/// microvm — below this, dedicating would starve the rest, so keep
/// cooperative Core-0 slicing. TODO: make a `sys/config/
/// microvm_dedicated_core` tunable (same pattern as the planned
/// `hwp_epp` knob in `enable_hwp`).
const MIN_CORES_FOR_DEDICATED: usize = 3;

/// Decide the dedicated VM core from the live core count. Called once
/// right after `scheduler::init` (so WORKER_COUNT is known). Picks the
/// highest worker id (= `worker_count`, since worker ids are
/// `1..=worker_count`) so the latency-sensitive low cores keep
/// stealing; Core 0 stays the IRQ/compositor/intent core regardless.
pub fn init_dedicated_vm_core(worker_count: usize) {
    let total = worker_count + 1; // + BSP

    // A2 dedicated-core path was hardened on QEMU/AMD-SVM through four
    // SVM-specific correctness fixes (v0.172.34/35/36/38: complete
    // host+guest vmsave/vmload bracket, host-restore before STGI,
    // EXITINTINFO type-gate, EVENTINJ-clear post-VMRUN). The Intel
    // VMX equivalents (VMCS-managed host-state, MSR-bitmap save/load,
    // VM-entry/exit interrupt info handling) were never audited under
    // the dedicated-core lifecycle. Bare-metal NUC reports the guest
    // VMLAUNCH succeeds (`[microvm] guest running` lands) but Linux
    // never produces an earlycon byte — classic symptom of the
    // dedicated-core VMX state lifecycle being subtly wrong (host
    // state corrupted on first VM-exit, etc.). Until VMX-A2 has its
    // own correctness pass, gate by vendor: AMD keeps the dedicated
    // path (validated), Intel falls back to cooperative Core-0
    // slicing (the byte-identical pre-A2 model). Browser still works,
    // just shares Core 0's cadence — that's the v0.172.11..v0.172.20
    // path we already know surfs google.ch + multi-site for ~74 s.
    // Vendor-detect inline via CPUID — runs BEFORE microvm::cpu::init
    // sets the static VENDOR (smp::init → tss::init → microvm::init
    // ordering in main.rs is fixed), so we can't use current_vendor()
    // here yet. CPUID leaf 0 vendor string (EBX/EDX/ECX in that order
    // per SDM Vol 2A §3.3) gives "AuthenticAMD" or "GenuineIntel".
    let a2_for_vendor = unsafe {
        let ebx: u32; let ecx: u32; let edx: u32;
        let ebx_out: u64; let ecx_out: u64;
        core::arch::asm!(
            "push rbx",
            "mov eax, 0",
            "cpuid",
            "mov {0}, rbx",
            "mov {1}, rcx",
            "pop rbx",
            out(reg) ebx_out,
            out(reg) ecx_out,
            out("eax") _,
            out("edx") edx,
        );
        ebx = ebx_out as u32;
        ecx = ecx_out as u32;
        let v = [
            (ebx & 0xFF) as u8, ((ebx >> 8) & 0xFF) as u8, ((ebx >> 16) & 0xFF) as u8, ((ebx >> 24) & 0xFF) as u8,
            (edx & 0xFF) as u8, ((edx >> 8) & 0xFF) as u8, ((edx >> 16) & 0xFF) as u8, ((edx >> 24) & 0xFF) as u8,
            (ecx & 0xFF) as u8, ((ecx >> 8) & 0xFF) as u8, ((ecx >> 16) & 0xFF) as u8, ((ecx >> 24) & 0xFF) as u8,
        ];
        match &v {
            b"AuthenticAMD" => A2_DEDICATED_CORE_ENABLED,
            _               => false,  // Intel & unknown: cooperative until VMX-A2 audited
        }
    };

    if a2_for_vendor && worker_count >= 1 && total >= MIN_CORES_FOR_DEDICATED {
        DEDICATED_VM_CORE.store(worker_count as u32, Ordering::Release);
        crate::kprintln!(
            "[npk] smp: core {} dedicated to microvm ({} cores, carved out of work-stealing)",
            worker_count, total
        );
    } else if !a2_for_vendor && total >= MIN_CORES_FOR_DEDICATED {
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
/// Must be called ON the core being measured (rdmsr is core-local).
pub fn update_core_freq(core_id: usize) {
    if core_id >= 256 { return; }

    // APERF/MPERF are gated by CPUID.06H:ECX[0] — absent on AMD and on KVM
    // guests with `-cpu qemu64`. Reading them unconditionally raises #GP.
    let has_apmp = HAS_APERFMPERF.load(Ordering::Relaxed);

    let (aperf, mperf): (u64, u64) = if has_apmp {
        // SAFETY: MSRs 0xE7/0xE8 confirmed by CPUID gate above.
        unsafe {
            let (a_lo, a_hi, m_lo, m_hi): (u32, u32, u32, u32);
            core::arch::asm!("rdmsr", in("ecx") 0xE8u32, out("eax") a_lo, out("edx") a_hi);
            core::arch::asm!("rdmsr", in("ecx") 0xE7u32, out("eax") m_lo, out("edx") m_hi);
            (((a_hi as u64) << 32) | a_lo as u64,
             ((m_hi as u64) << 32) | m_lo as u64)
        }
    } else {
        (0, 0)
    };
    let tsc = crate::interrupts::rdtsc();

    let busy = CORE_BUSY_TSC[core_id].load(Ordering::Relaxed);
    let prev_busy = LAST_BUSY[core_id].swap(busy, Ordering::Relaxed);
    let prev_tsc = LAST_TSC_CORE[core_id].swap(tsc, Ordering::Relaxed);
    let prev_aperf = LAST_APERF[core_id].swap(aperf, Ordering::Relaxed);
    let prev_mperf = LAST_MPERF[core_id].swap(mperf, Ordering::Relaxed);

    if prev_tsc == 0 { return; } // First call — seed only

    let delta_tsc = tsc.wrapping_sub(prev_tsc);
    let delta_aperf = aperf.wrapping_sub(prev_aperf);
    let delta_mperf = mperf.wrapping_sub(prev_mperf);
    let delta_busy = busy.wrapping_sub(prev_busy);

    if delta_tsc == 0 { return; }

    // Scheduler-tracked usage (task execution time / wall clock).
    let sched_pct = (delta_busy * 100 / delta_tsc).min(100) as u32;
    CORE_USAGE[core_id].store(sched_pct, Ordering::Relaxed);

    // Effective running frequency: APERF/MPERF * nominal_TSC_freq.
    // TSC is calibrated to nominal base; MPERF ticks at that same rate.
    if delta_mperf > 0 {
        let nominal_mhz = (crate::interrupts::tsc_freq() / 1_000_000) as u64;
        // Guard against overflow: cap aperf delta ratio implicitly via u128.
        let eff_mhz = ((delta_aperf as u128) * (nominal_mhz as u128)
                       / (delta_mperf as u128)) as u64;
        CORE_MHZ[core_id].store(eff_mhz.min(u32::MAX as u64) as u32, Ordering::Relaxed);
    }
}

/// Record task execution time on a core (called from AP work loop).
pub fn add_busy_tsc(core_id: usize, cycles: u64) {
    if core_id >= 256 { return; }
    CORE_BUSY_TSC[core_id].fetch_add(cycles, Ordering::Relaxed);
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

/// Per-core CPU usage in percent (0-100), based on delta busy/total TSC.
pub fn core_usage(core_id: usize) -> u32 {
    if core_id >= 256 { return 0; }
    CORE_USAGE[core_id].load(Ordering::Relaxed)
}

/// Read current core's sequential ID via LAPIC.
pub fn current_core_id() -> usize {
    let (lo, hi): (u32, u32);
    // SAFETY: MSR 0x1B (APIC base) always readable on x86_64 ring 0
    unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }
    let apic_base = ((hi as u64) << 32 | lo as u64) & 0xFFFF_FFFF_F000;
    // SAFETY: APIC MMIO is identity-mapped
    let apic_id = unsafe { core::ptr::read_volatile((apic_base + 0x20) as *const u32) } >> 24;
    let cores = CORES.lock();
    cores.iter().position(|c| c.apic_id == apic_id).unwrap_or(0)
}

/// Enable Hardware P-states (HWP / Speed Shift) on the current core.
/// CPU automatically scales frequency: idle → min, load → turbo.
/// Returns true if HWP was enabled successfully.
pub fn enable_hwp() -> bool {
    // Check HWP support: CPUID.06H:EAX bit 7
    let eax: u32;
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
    // SAFETY: HWP supported (checked above), ring 0
    unsafe { core::arch::asm!("wrmsr", in("ecx") 0x770u32, in("eax") 1u32, in("edx") 0u32); }

    // Read HWP capabilities: MSR 0x771 (IA32_HWP_CAPABILITIES)
    // [7:0]=Highest, [15:8]=Guaranteed, [23:16]=Efficient, [31:24]=Lowest
    let cap_lo: u32;
    // SAFETY: MSR 0x771 exists when HWP is supported
    unsafe { core::arch::asm!("rdmsr", in("ecx") 0x771u32, out("eax") cap_lo, out("edx") _); }
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
    // EPP=0 (max perf): CPU ramps to turbo on the first instruction of a
    // burst. Right for desktop / mains-powered targets like the N100 NUC.
    // Bursty workloads (crypto, I/O batches) live in the ~10-100 ms range
    // — too short for the firmware's default heuristics with EPP=192 to
    // notice and ramp; the CPU sat at base clock and software ChaCha20
    // throughput was capped accordingly. Mobile targets that care about
    // battery would set this to 128 or 192 — make it a `sys/config/hwp_epp`
    // tunable when the notebook hardware lands.
    let hwp_req = (lowest as u32)
        | ((highest as u32) << 8)
        | (0u32 << 24);
    // SAFETY: MSR 0x774 exists when HWP is enabled
    unsafe { core::arch::asm!("wrmsr", in("ecx") 0x774u32, in("eax") hwp_req, in("edx") 0u32); }

    true
}

/// AP Rust entry — called by trampoline after long mode transition.
/// Interrupts are disabled (cli from trampoline). IDT is loaded.
#[unsafe(no_mangle)]
pub extern "C" fn smp_ap_entry(core_id: u32) -> ! {
    // Enable Local APIC (needed for IPI wakeup fallback)
    let apic_base = super::read_apic_base();
    // SAFETY: APIC MMIO is identity-mapped, each core sees its own LAPIC
    unsafe {
        let svr = core::ptr::read_volatile((apic_base + 0xF0) as *const u32);
        core::ptr::write_volatile((apic_base + 0xF0) as *mut u32, svr | (1 << 8) | 0xFF);
    }

    // Enable HWP on this AP (per-core frequency scaling)
    enable_hwp();

    // Signal BSP: this AP is alive
    super::AP_STARTED.fetch_add(1, Ordering::Release);

    // Wait until scheduler is initialized
    while !SCHEDULER_READY.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    // Enter scheduler loop
    let cid = core_id as usize;
    let use_mwait = HAS_MWAIT.load(Ordering::Relaxed);

    loop {
        // Carved out of the work-stealing pool for the microvm
        // (substrate rework A2). `vm_core_serve` opens the guest ON
        // THIS CORE when a launch is pending and runs it continuously
        // to exit (blocking the core for the guest's whole lifetime —
        // that is the point: it no longer fights Shade + the shell for
        // Core 0). Cheap no-op when nothing is pending → we idle on
        // the 100 Hz timer and re-check within ~10 ms. Default
        // sentinel never matches a real cid, so ≤2-core / no-AP hosts
        // keep the exact old work-stealing loop.
        if DEDICATED_VM_CORE.load(Ordering::Acquire) == cid as u32 {
            CORE_ACTIVE[cid].store(true, Ordering::Relaxed);
            crate::microvm::vm_core_serve();
            CORE_ACTIVE[cid].store(false, Ordering::Relaxed);
            update_core_freq(cid);
            // SAFETY: ring-0 idle; the 100 Hz timer IRQ (≥) wakes us
            // to re-check for a pending launch.
            unsafe { core::arch::asm!("sti; hlt; cli"); }
            continue;
        }

        // Try to get work (own deque first, then steal)
        if let Some(task) = super::scheduler::next_task(cid) {
            CORE_ACTIVE[cid].store(true, Ordering::Relaxed);
            start_work(cid);
            (task.func)(task.arg);
            flush_busy(cid);
            CORE_ACTIVE[cid].store(false, Ordering::Relaxed);
            continue;
        }

        // Before sleep: update usage stats (delta covers work + idle since last call)
        update_core_freq(cid);

        // No work — sleep efficiently
        if use_mwait {
            // MONITOR/MWAIT: all APs watch the GLOBAL WORK_AVAILABLE flag.
            // When BSP (or any core) spawns work, it writes 1 → hardware wakes us.
            let flag_ptr = super::scheduler::wake_flag_ptr();
            super::scheduler::clear_wake();

            // SAFETY: MONITOR/MWAIT are safe ring-0 instructions.
            unsafe {
                core::arch::asm!(
                    "monitor",
                    in("rax") flag_ptr,
                    in("ecx") 0u32,
                    in("edx") 0u32,
                );
                // Re-check after MONITOR (avoid missed-wakeup race).
                // If a task arrived between clear_wake and MONITOR, execute it
                // instead of sleeping (next_task extracts from queue — must not drop!).
                if let Some(task) = super::scheduler::next_task(cid) {
                    CORE_ACTIVE[cid].store(true, Ordering::Relaxed);
                    start_work(cid);
                    (task.func)(task.arg);
                    flush_busy(cid);
                    CORE_ACTIVE[cid].store(false, Ordering::Relaxed);
                } else {
                    core::arch::asm!(
                        "mwait",
                        in("eax") 0x01u32, // C1E — clock gated, frequency drops
                        in("ecx") 0u32,
                    );
                }
            }
        } else {
            // Fallback: HLT with interrupts enabled (wakes on any interrupt)
            unsafe { core::arch::asm!("sti; hlt; cli"); }
        }
    }
}
