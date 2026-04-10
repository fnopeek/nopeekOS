//! Per-Core State
//!
//! Tracks CPU cores discovered at boot. Dynamically sized — no hardcoded limit.
//! Scales from 1 (BSP only) to 1024+ cores.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

static CORES: Mutex<Vec<CoreInfo>> = Mutex::new(Vec::new());
static CORE_COUNT: AtomicUsize = AtomicUsize::new(1);

/// Set to true once scheduler is initialized and APs should start working
static SCHEDULER_READY: AtomicBool = AtomicBool::new(false);

/// True if CPU supports MONITOR/MWAIT (detected at boot)
static HAS_MWAIT: AtomicBool = AtomicBool::new(false);

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
            (task.func)(task.arg);
            continue;
        }

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
                        in("eax") 0u32,  // C0 — lightest sleep, fastest wake
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
