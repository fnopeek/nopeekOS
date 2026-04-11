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

/// Per-core active flag: true while executing a scheduler task
static CORE_ACTIVE: [AtomicBool; 256] = {
    const FALSE: AtomicBool = AtomicBool::new(false);
    [FALSE; 256]
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

/// Check if MONITOR/MWAIT is available
pub fn has_mwait() -> bool {
    HAS_MWAIT.load(Ordering::Relaxed)
}

/// Update per-core CPU usage and frequency.
///
/// Usage: explicit busy-TSC tracking (task execution time / total time).
///        Independent of MPERF behavior — works on all CPUs.
/// Freq:  MSR 0x198 (IA32_PERF_STATUS) bits [15:8] = current ratio * 100 MHz
pub fn update_core_freq(core_id: usize) {
    if core_id >= 256 { return; }

    let busy = CORE_BUSY_TSC[core_id].load(Ordering::Relaxed);
    let tsc = crate::interrupts::rdtsc();

    let prev_busy = LAST_BUSY[core_id].swap(busy, Ordering::Relaxed);
    let prev_tsc = LAST_TSC_CORE[core_id].swap(tsc, Ordering::Relaxed);

    if prev_tsc == 0 { return; } // First call

    let delta_busy = busy.wrapping_sub(prev_busy);
    let delta_tsc = tsc.wrapping_sub(prev_tsc);

    if delta_tsc > 0 {
        let usage = (delta_busy * 100).checked_div(delta_tsc).unwrap_or(0);
        CORE_USAGE[core_id].store(usage.min(100) as u32, Ordering::Relaxed);
    }

    // Frequency from PERF_STATUS: bits [15:8] = current ratio * 100 MHz bus
    let perf_lo: u32;
    // SAFETY: MSR 0x198 readable on all x86_64 Intel in ring 0
    unsafe { core::arch::asm!("rdmsr", in("ecx") 0x198u32, out("eax") perf_lo, out("edx") _); }
    let ratio = (perf_lo >> 8) & 0xFF;
    CORE_MHZ[core_id].store(ratio * 100, Ordering::Relaxed);
}

/// Record task execution time on a core (called from AP work loop).
pub fn add_busy_tsc(core_id: usize, cycles: u64) {
    if core_id >= 256 { return; }
    CORE_BUSY_TSC[core_id].fetch_add(cycles, Ordering::Relaxed);
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

/// Per-core CPU usage in percent (0-100).
/// For cores running long-lived tasks (WASM apps), returns 100% while active.
pub fn core_usage(core_id: usize) -> u32 {
    if core_id >= 256 { return 0; }
    // If core is currently executing a task, it's 100% busy
    if CORE_ACTIVE[core_id].load(Ordering::Relaxed) { return 100; }
    CORE_USAGE[core_id].load(Ordering::Relaxed)
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
    // [7:0]=Min, [15:8]=Max, [23:16]=Desired(0=auto), [31:24]=EPP(128=balanced)
    let hwp_req = (lowest as u32)
        | ((highest as u32) << 8)
        | (128u32 << 24); // EPP = balanced
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
        // Try to get work (own deque first, then steal)
        if let Some(task) = super::scheduler::next_task(cid) {
            CORE_ACTIVE[cid].store(true, Ordering::Relaxed);
            let t0 = crate::interrupts::rdtsc();
            (task.func)(task.arg);
            add_busy_tsc(cid, crate::interrupts::rdtsc() - t0);
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
                // Re-check after MONITOR (avoid missed-wakeup race)
                if super::scheduler::next_task(cid).is_none() {
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
