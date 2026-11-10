//! Per-core state and the worker loop: usage accounting, placement of
//! new work, waking idle cores. What a core is and how it is started,
//! woken and halted comes from the architecture's half (`super::hw`).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use super::hw;

pub use hw::current_core_id;
#[cfg(target_arch = "x86_64")]
pub use hw::{
    amd_cstate_base, aperf_mperf_now, core_apic, halted_aperf_mperf, has_mwait, has_rapl,
    intel_model, max_turbo_mhz, min_eff_mhz, pkg_cstate_limit, rapl_core_is_all, rapl_core_raw,
    rapl_mw, rapl_nj_per_unit, rapl_pkg_raw, residency, sample_aperf_mperf, RESIDENCY_NAMES,
};

static CORE_COUNT: AtomicUsize = AtomicUsize::new(1);

/// Set to true once scheduler is initialized and APs should start working
static SCHEDULER_READY: AtomicBool = AtomicBool::new(false);

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
        hw::sample_aperf_mperf(core_id);
        HALT_SINCE[core_id].store(t0, Ordering::Relaxed);
    }
}

/// Cumulative halted TSC for `core_id`, including a halt still in progress.
fn halted_tsc(core_id: usize) -> u64 {
    let acc = CORE_HALT_TSC[core_id].load(Ordering::Relaxed);
    let since = HALT_SINCE[core_id].load(Ordering::Relaxed);
    if since == 0 {
        acc
    } else {
        acc + crate::time::counter().saturating_sub(since)
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
    // A driver stays with its interrupt and its registration.
    if core_id == current_core_id() {
        super::fiber::pin_current();
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

/// Total cores (BSP + online APs)
pub fn core_count() -> usize {
    CORE_COUNT.load(Ordering::Acquire)
}

/// Record how many cores run, the boot core included.
pub fn set_core_count(n: usize) {
    CORE_COUNT.store(n, Ordering::Release);
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

/// The dedicated microvm core id, or `None` if cooperative Core-0.
pub fn dedicated_vm_core() -> Option<usize> {
    match DEDICATED_VM_CORE.load(Ordering::Acquire) {
        NO_DEDICATED_CORE => None,
        c => Some(c as usize),
    }
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
        && crate::time::counter().wrapping_sub(prev) < crate::time::counter_hz() / 10
    {
        return;
    }

    // (0, 0) where the core has no such counters.
    let (aperf, mperf) = hw::aperf_mperf_of(core_id);
    let tsc = crate::time::counter();

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
        let nominal_mhz = (crate::time::counter_hz() / 1_000_000) as u64;
        // Guard against overflow: cap aperf delta ratio implicitly via u128.
        let eff_mhz = ((delta_aperf as u128) * (nominal_mhz as u128)
                       / (delta_mperf as u128)) as u64;
        CORE_MHZ[core_id].store(eff_mhz.min(u32::MAX as u64) as u32, Ordering::Relaxed);
    }
}

/// Start tracking work time for a core (called when task begins or resumes).
pub fn start_work(core_id: usize) {
    if core_id >= 256 { return; }
    WORK_START_TSC[core_id].store(crate::time::counter(), Ordering::Relaxed);
}

/// Flush accumulated work time since last start_work (called before wait/idle).
/// Returns the flushed cycles count.
pub fn flush_busy(core_id: usize) -> u64 {
    if core_id >= 256 { return 0; }
    let start = WORK_START_TSC[core_id].swap(0, Ordering::Relaxed);
    if start == 0 { return 0; }
    let elapsed = crate::time::counter().saturating_sub(start);
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

/// Per-core CPU usage in percent (0-100): 100 − halted% over the last
/// sampling window. Measured, not self-reported.
pub fn core_usage(core_id: usize) -> u32 {
    if core_id >= 256 { return 0; }
    CORE_USAGE[core_id].load(Ordering::Relaxed)
}

/// The worker has reached its loop. An AP that never started must not count
/// as the least loaded core: it never takes anything, so work would stall.
static IN_LOOP: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
/// TSC stamped by a core first thing after `hlt` (the `tsc_offset` probe).
static WOKE_TSC: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

pub fn note_woke(c: usize) {
    if c < 256 {
        WOKE_TSC[c].store(crate::time::counter(), Ordering::Release);
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
    let t_send = crate::time::counter();
    hw::send_wake(c);
    for _ in 0..10_000_000u32 {
        let w = WOKE_TSC[c].load(Ordering::Acquire);
        if w != before {
            let t_recv = crate::time::counter();
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
        hw::send_wake(c);
    }
}

/// A fiber on core `c` became runnable while `c` may be running a vCPU.
/// `run_slice` reads and clears it after every exit and gives the core back
/// to the fiber scheduler, so the fiber runs within one exit instead of at
/// the end of the vCPU's slice.
static PEER_WAKE: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];

/// Wake core `c` for a fiber that lives there and was just made runnable
/// (`fiber::signal`, `fiber::admit`). Like `wake_core`, and in addition: a
/// vCPU running there is taken out of the guest at once, the way KVM's
/// reschedule IPI preempts a vCPU thread. The IPI's arrival is the exit.
pub fn wake_fiber_core(c: usize) {
    if c >= 256 {
        return;
    }
    PEER_WAKE[c].store(true, Ordering::SeqCst);
    if current_core_id() == c {
        return;
    }
    // A vCPU core only while the vCPU may be in the guest: between exit and
    // entry it sees `PEER_WAKE` in its last look (`entry_irqs_off`).
    if IDLE[c].load(Ordering::SeqCst) || hw::in_guest(c) {
        hw::send_wake(c);
    }
}

/// Did a fiber on core `c` become runnable since the last look? Clears it.
pub fn take_peer_wake(c: usize) -> bool {
    c < 256 && PEER_WAKE[c].swap(false, Ordering::AcqRel)
}

/// Whether a fiber on core `c` became runnable, without clearing it — for
/// the last look before a guest entry.
pub fn peer_wake_pending(c: usize) -> bool {
    c < 256 && PEER_WAKE[c].load(Ordering::SeqCst)
}

/// Core 0's loop after boot: the same fiber scheduler as a worker, with the
/// shell (`intent::run_loop`) as a fiber. No periodic tick: the halt ends at
/// the earliest fiber deadline, on an interrupt, or by the wake IPI when
/// another core signals a fiber here. Core 0 takes no inbox work.
pub fn core0_loop() -> ! {
    // No periodic tick from here on: Core 0's LAPIC timer becomes
    // the one-shot deadline timer every worker has.
    hw::make_core0_tickless();
    loop {
        super::fiber::run_core_fibers(0);
        // Self-throttled to 100 ms windows.
        update_core_freq(0);
        // IF=0 from the last look to the halt: an ISR here that signals a
        // fiber on this core sends no IPI (`wake_core`), so it must stay
        // pending and end the halt rather than run before it.
        let flags = crate::arch::irq_save();
        IDLE[0].store(true, Ordering::SeqCst);
        let now = crate::time::counter();
        match super::fiber::earliest_deadline(0) {
            Some(d) if d <= now => {}
            wake => hw::halt_until(wake, WAKE_HLT_FALLBACK),
        }
        IDLE[0].store(false, Ordering::Relaxed);
        crate::arch::irq_restore(flags);
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
/// The NIC driver's core, the microVM core and every core running a vCPU are
/// excluded: they never take anything, so the minimum could otherwise sit on
/// a core that never takes it. Ties are allowed; several
/// empty cores may race for the inbox.
fn least_loaded(cid: usize) -> bool {
    let workers = super::scheduler::worker_count();
    if workers <= 1 {
        return true;
    }
    let nic = hw::nic_core();
    let vm = DEDICATED_VM_CORE.load(Ordering::Acquire) as usize;
    let mine = core_load(cid);
    for c in 1..=workers.min(255) {
        // A core inside an intent cannot take anything right now; counting
        // it would make new work wait for that intent to finish.
        if c == cid || Some(c) == nic || c == vm || !IN_LOOP[c].load(Ordering::Acquire)
            || NATIVE_BUSY[c].load(Ordering::Relaxed)
            || hw::is_vcpu_core(c)
        {
            continue;
        }
        if core_load(c) < mine {
            return false;
        }
    }
    true
}

/// The core the background apps share (autostart: deck); 0 = none yet.
///
/// Each of them sleeps on events almost all the time, yet on a core of its
/// own it makes that core unusable for a vCPU (`microvm::cpu::protected_cores`).
/// Together they cost one core. Only background apps are packed: an
/// interactive app may compute for seconds without yielding, and a peer
/// on its core would wait that long.
static APP_CORE: AtomicUsize = AtomicUsize::new(0);

/// The shared app core, if one has been chosen.
pub fn app_core() -> Option<usize> {
    let c = APP_CORE.load(Ordering::Acquire);
    (c != 0).then_some(c)
}

/// `c` may keep collecting background apps: not a driver's core (those stay
/// to themselves), not the NIC driver's or the microVM's.
fn app_core_usable(c: usize) -> bool {
    c != 0 && c < 64
        && driver_cores() & (1 << c) == 0
        && hw::nic_core() != Some(c)
        && DEDICATED_VM_CORE.load(Ordering::Acquire) as usize != c
        && !hw::is_vcpu_core(c)
}

/// Whether core `cid` takes new work from the inbox at all.
///
/// The NIC driver's core takes none while other workers exist: it wakes every
/// millisecond and would otherwise win nearly every new intent, including the
/// one consuming the driver's frames; an intent runs to completion, so driver
/// and consumer would alternate instead of running side by side. A core with
/// a vCPU takes nothing new either: fibers are cooperative, and the vCPU
/// would wait for a computing app to yield. A core with a microVM worker does
/// take work — the workers park or yield between passes. Among the rest,
/// `accepts` prefers the least loaded, so the core that wakes most often does
/// not collect every fiber.
fn takes_work(cid: usize) -> bool {
    let nic_core = hw::nic_core() == Some(cid)
        && super::scheduler::worker_count() > 1;
    !nic_core && !hw::is_vcpu_core(cid)
}

/// Hand an app task to this core's fiber scheduler. It runs on its own stack
/// and yields at its waits, so peers share the core.
fn admit_fiber_task(cid: usize, task: super::scheduler::Task) {
    if task.shared && !app_core_usable(APP_CORE.load(Ordering::Acquire)) {
        APP_CORE.store(cid, Ordering::Release);
    }
    if !super::fiber::admit_movable(cid, task.func, task.arg) {
        if let Some(dropped) = task.dropped {
            dropped(task.arg);
        }
    }
}

/// Whether core `c` may take over fibers from busy cores when it has nothing
/// to run. Not Core 0 (the shell and the compositor want it responsive), and
/// not a driver's core: it looks idle whenever its driver parks, and a taken
/// app's long kernel call would keep the driver off it (as in `accepts`). A
/// core holding a vCPU takes only preemptible fibers (`steal_for`).
fn may_steal(c: usize) -> bool {
    c != 0 && c < 256
        && !holds_driver(c)
        && IN_LOOP[c].load(Ordering::Acquire)
        && DEDICATED_VM_CORE.load(Ordering::Acquire) as usize != c
}

/// Last time an idle core was asked to look at core `c`'s queue.
static INVITED_TSC: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

/// Core `victim` holds a movable fiber that could run now but waits behind
/// another: wake one idle core that may take it (`fiber::steal_for`). At most
/// once per 50 us per victim — the woken core takes one fiber per look and
/// looks again before it halts.
pub fn invite_stealer(victim: usize) {
    if victim >= 256 {
        return;
    }
    let now = crate::time::counter();
    let last = INVITED_TSC[victim].load(Ordering::Relaxed);
    if now.wrapping_sub(last) < crate::time::counter_hz() / 20_000 {
        return;
    }
    INVITED_TSC[victim].store(now, Ordering::Relaxed);
    let workers = super::scheduler::worker_count().min(255);
    let me = current_core_id();
    // A core without a vCPU first: it can take any movable fiber.
    let idle = |c: usize| c != victim && c != me && IDLE[c].load(Ordering::SeqCst) && may_steal(c);
    if let Some(c) = (1..=workers)
        .find(|&c| idle(c) && !hw::is_vcpu_core(c))
        .or_else(|| (1..=workers).find(|&c| idle(c)))
    {
        hw::send_wake(c);
    }
}

/// Between two fibers of core `cid` (`fiber::run_core_fibers`): take a new
/// app if it is waiting and this core would take it. Without this a core
/// looks at the inbox only once all its fibers park, which a busy-polling
/// worker never does. Native tasks still wait for the top of the loop or for
/// another core: run here they would stop every fiber of the core until they
/// return. Core 0 takes no inbox work.
pub fn admit_between_fibers(cid: usize) {
    if cid == 0 || !takes_work(cid) {
        return;
    }
    if let Some(task) = super::scheduler::next_fiber_task(|fiber, shared, waited| accepts(cid, fiber, shared, waited)) {
        admit_fiber_task(cid, task);
    }
}

/// Whether `cid` takes the oldest waiting task. A background app goes to the
/// app core while that can take it; everything else, and a background app
/// with no usable app core, goes to a least loaded core.
fn accepts(cid: usize, fiber: bool, shared: bool, waited: u64) -> bool {
    // A core a driver lives on gives its pinned fibers the core only between
    // other work: a native task holds it until it returns, and an app's long
    // kernel call until that returns. While another core without a driver
    // can take work, a native task never goes there, and an app other than
    // the background apps of the shared app core only after waiting as long
    // as any task may.
    let driver_core = holds_driver(cid) && (1..=super::scheduler::worker_count().min(63))
        .any(|c| c != cid && !holds_driver(c) && can_take(c));
    if driver_core && !fiber {
        return false;
    }
    // Placement is a preference, not a reason to strand work: a core whose
    // fibers never all park (a busy-polling worker) never gets back to the
    // inbox, yet still counts as less loaded.
    if waited >= crate::time::counter_hz() / 1000 * INBOX_STARVE_MS {
        return true;
    }
    if shared {
        let a = APP_CORE.load(Ordering::Acquire);
        if app_core_usable(a) && IN_LOOP[a].load(Ordering::Acquire)
            && !NATIVE_BUSY[a].load(Ordering::Relaxed)
        {
            return a == cid;
        }
    }
    if driver_core {
        return false;
    }
    least_loaded(cid)
}

/// Whether a driver's pinned fiber lives on `c` (`mark_driver_core`).
fn holds_driver(c: usize) -> bool {
    c < 64 && DRIVER_CORES.load(Ordering::Relaxed) & (1 << c) != 0
}

/// Whether worker `c` takes inbox work at all.
fn can_take(c: usize) -> bool {
    IN_LOOP[c].load(Ordering::Acquire)
        && takes_work(c)
        && DEDICATED_VM_CORE.load(Ordering::Acquire) as usize != c
}

/// How long a task may wait for its preferred core before any core that
/// takes work takes it. Twice the idle re-check (10 ms), so every declining
/// core has looked again at least once.
const INBOX_STARVE_MS: u64 = 20;

/// A started core's scheduler loop, entered from the architecture's AP
/// entry once the core can take interrupts, with them masked. Waits for
/// `start_scheduler`, then never returns.
pub fn worker_loop(core_id: u32) -> ! {
    while !SCHEDULER_READY.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    // This core's own one-shot timer, armed to whatever deadline it waits
    // for: its idle halt needs its own wake source. New work wakes it by
    // IPI (`wake_idle_workers`).
    hw::init_worker_timer();
    if (core_id as usize) < 256 {
        IN_LOOP[core_id as usize].store(true, Ordering::Release);
    }

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
            hw::vm_core_serve();
            CORE_ACTIVE[cid].store(false, Ordering::Relaxed);
            update_core_freq(cid);
            // Re-check for a pending launch every 10 ms.
            let d = crate::time::counter() + crate::time::counter_hz() / 100;
            hw::halt_until(Some(d), WAKE_HLT_FALLBACK);
            continue;
        }

        let take = takes_work(cid);
        // Admit a freshly-spawned app as a fiber, or run a native intent.
        // New work arrives in the shared inbox (`scheduler::spawn`).
        let next = if take { super::scheduler::next_task(|fiber, shared, waited| accepts(cid, fiber, shared, waited)) } else { None };
        if let Some(task) = next {
            if task.is_fiber {
                admit_fiber_task(cid, task);
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

        // Nothing of its own to run: take over a fiber that waits, runnable,
        // behind a busy core. Its scheduler runs it on the next pass.
        if may_steal(cid)
            && super::fiber::steal_for(cid, hw::is_vcpu_core(cid))
        {
            continue;
        }

        // Idle: halt until the earliest parked fiber is due, or until an
        // interrupt — a device IRQ for a fiber in `irq_wait`, a kick, or
        // the wake IPI for new work. No periodic tick.
        //
        // IDLE is published before the inbox is looked at again: a spawn
        // that pushed without seeing IDLE sent no IPI, so its work must be
        // seen by this re-check; one that saw IDLE sends the IPI, which
        // stays pending (IF=0) and ends the halt below at once. The same
        // holds for an ISR on this core that signals one of its fibers.
        let flags = crate::arch::irq_save();
        IDLE[cid].store(true, Ordering::SeqCst);
        let now = crate::time::counter();
        let mut wake = super::fiber::earliest_deadline(cid);
        if matches!(wake, Some(d) if d <= now) {
            IDLE[cid].store(false, Ordering::Relaxed);
            crate::arch::irq_restore(flags);
            continue; // became runnable — round again
        }
        if super::scheduler::has_work() {
            if take && super::scheduler::head_acceptable(|fiber, shared, waited| accepts(cid, fiber, shared, waited)) {
                IDLE[cid].store(false, Ordering::Relaxed);
                crate::arch::irq_restore(flags);
                continue;
            }
            // Work waits that this core declines — for another core. If
            // that one is busy inside its fibers it takes the work only
            // when it gets back to the top of its loop; look again in
            // 10 ms so the work cannot be stranded behind a changed load.
            let recheck = now + crate::time::counter_hz() / 100;
            wake = Some(wake.map_or(recheck, |d| d.min(recheck)));
        }
        hw::halt_until(wake, WAKE_HLT_FALLBACK);
        IDLE[cid].store(false, Ordering::Relaxed);
        crate::arch::irq_restore(flags);
    }
}

