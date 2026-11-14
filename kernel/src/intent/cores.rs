//! `cores`: where the time goes, as Linux tells it in `/proc/stat`,
//! `/proc/<pid>/schedstat` and `/proc/interrupts`, over one window:
//!
//! - per core: busy from the halted-cycle counters (a spinning core never
//!   halts and shows busy), halts and what woke them;
//! - per fiber: time on the core, runs, and the time it was runnable but
//!   not running (`run_delay`), average and longest;
//! - per interrupt: hardirqs, and the time from the hardirq until the
//!   driver's fiber ran (average and longest).
//!
//! The x86-only parts (clock, microVM, guest network) stay in
//! `system::intent_cores`, which prints these tables too.

use crate::{kprint, kprintln};
use alloc::string::String;
use alloc::vec::Vec;

/// The counters of one moment.
pub struct Sample {
    at: u64,
    halt: Vec<(u64, u64)>,
    wake: Vec<[u64; crate::smp::per_core::WAKE_CAUSES]>,
    timer: Vec<(u64, u64)>,
    fibers: Vec<crate::smp::fiber::FiberStats>,
    irqs: Vec<crate::irq::IrqStats>,
}

/// The counters now. The first sample of a window restarts the maxima.
pub fn sample(first: bool) -> Sample {
    let cores = crate::smp::per_core::core_count();
    Sample {
        at: crate::time::counter(),
        halt: (0..cores).map(crate::smp::per_core::halt_snapshot).collect(),
        wake: (0..cores).map(crate::smp::per_core::wake_snapshot).collect(),
        timer: (0..cores).map(crate::smp::per_core::timer_snapshot).collect(),
        fibers: crate::smp::fiber::fiber_stats(first),
        irqs: crate::irq::irq_stats(first),
    }
}

/// Counter ticks as microseconds.
fn us(ticks: u64) -> u64 {
    ticks.saturating_mul(1_000_000) / crate::time::counter_hz().max(1)
}

/// A fiber's name: its own, its process's, or its waker.
fn fiber_name(f: &crate::smp::fiber::FiberStats) -> String {
    if let Some(n) = f.name {
        return String::from(n);
    }
    if f.pid != 0 {
        if let Some(n) = crate::process::name(f.pid) {
            return n;
        }
    }
    alloc::format!("fiber {}", f.waker)
}

/// The per-core table. `mhz`, where the architecture measures the clock,
/// gives one column per core.
pub fn print_cores(s0: &Sample, s1: &Sample, mhz: Option<&[String]>) {
    let window = s1.at.saturating_sub(s0.at).max(1);
    let window_ms = us(window) / 1000;
    let per_s = |d: u64| d.saturating_mul(crate::time::counter_hz()) / window;
    let tick_per_us = (crate::time::counter_hz() / 1_000_000).max(1);
    let vmcore = crate::smp::per_core::dedicated_vm_core();

    kprintln!();
    kprintln!("  Per-core CPU (idle-measured, {} ms window; every core wakes on deadline/IRQ/IPI)", window_ms);
    kprintln!("  ─────────────────────────────────────────────────────");
    if mhz.is_some() {
        kprintln!("  CORE   BUSY%    MHz   HALTS/s   AVG-RESIDENCY   QUEUE  ROLE");
    } else {
        kprintln!("  CORE   BUSY%   HALTS/s   AVG-RESIDENCY   QUEUE  ROLE");
    }
    for c in 0..s1.halt.len().min(s0.halt.len()) {
        let dhalt = s1.halt[c].0.saturating_sub(s0.halt[c].0);
        let dcount = s1.halt[c].1.saturating_sub(s0.halt[c].1);
        let halt_pct = ((dhalt as u128) * 100 / (window as u128)) as u64;
        let busy = 100u64.saturating_sub(halt_pct.min(100));
        let halts_per_s = per_s(dcount);
        let avg_us = if dcount > 0 { (dhalt / dcount) / tick_per_us } else { 0 };
        let qlen = crate::smp::scheduler::queue_len(c);

        // Classified from the measured signals, not a hardcoded guess.
        let role: String = if c == 0 {
            String::from("core0 (kernel/irq/shell)")
        } else if Some(c) == vmcore {
            String::from("microvm (dedicated)")
        } else if let Some(t) = crate::smp::per_core::native_task(c) {
            alloc::format!("native task '{}'", t)
        } else if busy >= 90 && halts_per_s < 5 {
            String::from("SPINNING (never halts!)")
        } else if halts_per_s > 200 && avg_us < 50 {
            String::from("spin? (spurious wakes)")
        } else if crate::smp::per_core::is_active(c) {
            String::from("running task")
        } else if busy < 5 && crate::smp::fiber::fiber_count(c) > 0 {
            alloc::format!("{} fiber(s), asleep", crate::smp::fiber::fiber_count(c))
        } else if busy < 5 {
            String::from("idle (asleep)")
        } else {
            String::from("worker")
        };

        let res = if dcount > 0 { alloc::format!("{:>10} us", avg_us) } else { String::from("—") };
        match mhz.and_then(|m| m.get(c)) {
            Some(m) => kprintln!("  {:>4}   {:>4}%  {:>5}   {:>7}   {:>13}   {:>5}  {}",
                c, busy, m, halts_per_s, res, qlen, role),
            None => kprintln!("  {:>4}   {:>4}%   {:>7}   {:>13}   {:>5}  {}",
                c, busy, halts_per_s, res, qlen, role),
        }

        // Which cause returned each halt this window. UNATTR = HALTS −
        // Σcauses: a halt that returned without a recorded cause.
        let labels = crate::smp::per_core::WAKE_LABELS;
        let mut attributed = 0u64;
        kprint!("        wakes:");
        let mut any = false;
        for (i, label) in labels.iter().enumerate() {
            let d = s1.wake[c][i].saturating_sub(s0.wake[c][i]);
            attributed = attributed.saturating_add(d);
            if d > 0 {
                kprint!(" {}={}/s", label, per_s(d));
                any = true;
            }
        }
        if !any { kprint!(" (none)"); }
        kprintln!("  | UNATTR={}/s", per_s(dcount.saturating_sub(attributed)));
        kprintln!("        timer: fires={}/s late_max={}us",
            per_s(s1.timer[c].0.saturating_sub(s0.timer[c].0)),
            s1.timer[c].1 / tick_per_us);
        let fibers = crate::smp::fiber::fiber_count(c);
        if c > 0 && fibers > 0 {
            let (names, n) = crate::process::wasm_names_on_core(c);
            kprint!("        fibers: {}", if n > 0 { names.as_str() } else { "-" });
            let kernel = fibers.saturating_sub(n);
            if kernel > 0 { kprint!(" (+{} kernel)", kernel); }
            if crate::smp::per_core::driver_cores() & (1 << c) != 0 { kprint!(" [driver]"); }
            if crate::smp::per_core::app_core() == Some(c) { kprint!(" [apps]"); }
            kprintln!();
        }
    }
}

/// Fibers that ran or waited for a core in the window, most CPU first.
pub fn print_fibers(s0: &Sample, s1: &Sample) {
    let window = s1.at.saturating_sub(s0.at).max(1);
    let per_s = |d: u64| d.saturating_mul(crate::time::counter_hz()) / window;
    struct Row { core: u32, name: String, cpu: u64, runs: u64, delay: u64, delay_max: u64 }
    let mut rows: Vec<Row> = Vec::new();
    let mut quiet = 0usize;
    for f in &s1.fibers {
        let before = s0.fibers.iter().find(|b| b.waker == f.waker && b.runs <= f.runs);
        let (cpu0, runs0, delay0) = before.map_or((0, 0, 0), |b| (b.cpu, b.runs, b.delay_sum));
        let cpu = f.cpu.saturating_sub(cpu0);
        let runs = f.runs.saturating_sub(runs0);
        if runs == 0 && cpu == 0 {
            quiet += 1;
            continue;
        }
        rows.push(Row {
            core: f.core,
            name: fiber_name(f),
            cpu,
            runs,
            delay: f.delay_sum.saturating_sub(delay0),
            delay_max: f.delay_max,
        });
    }
    rows.sort_by(|a, b| b.cpu.cmp(&a.cpu));
    kprintln!();
    kprintln!("  Fibers (schedstat): time on the core, and runnable but waiting for it");
    kprintln!("  ─────────────────────────────────────────────────────");
    kprintln!("  CORE  CPU%   RUNS/s   WAIT avg     WAIT max   NAME");
    for r in &rows {
        let pct = ((r.cpu as u128) * 1000 / (window as u128)) as u64;
        let avg = if r.runs > 0 { us(r.delay) / r.runs } else { 0 };
        kprintln!("  {:>4}  {:>3}.{}  {:>6}   {:>6} us  {:>8} us   {}",
            r.core, pct / 10, pct % 10, per_s(r.runs), avg, us(r.delay_max), r.name);
    }
    if quiet > 0 {
        kprintln!("        (+{} fibers that did not run)", quiet);
    }
}

/// Interrupts in the window: hardirqs and the way to the driver's fiber.
pub fn print_irqs(s0: &Sample, s1: &Sample) {
    let window = s1.at.saturating_sub(s0.at).max(1);
    let per_s = |d: u64| d.saturating_mul(crate::time::counter_hz()) / window;
    kprintln!();
    kprintln!("  Interrupts: hardirqs, and hardirq until the driver's fiber ran");
    kprintln!("  ─────────────────────────────────────────────────────");
    kprintln!("   IRQ   HWIRQ    FIRED/s   TOTAL       RUN avg     RUN max   FIBER");
    for i in &s1.irqs {
        let before = s0.irqs.iter().find(|b| b.irq == i.irq);
        let (f0, ls0, ln0) = before.map_or((0, 0, 0), |b| (b.fired, b.lat_sum, b.lat_n));
        let dn = i.lat_n.saturating_sub(ln0);
        let avg = if dn > 0 { us(i.lat_sum.saturating_sub(ls0)) / dn } else { 0 };
        let fiber = s1.fibers.iter().find(|f| f.waker == i.waiter).map(fiber_name)
            .unwrap_or_else(|| String::from("-"));
        kprintln!("  {:>4}  {:>6}  {:>9}  {:>6}  {:>8} us  {:>8} us   {}",
            i.irq, i.hwirq, per_s(i.fired.saturating_sub(f0)), i.fired, avg, us(i.lat_max), fiber);
    }
}

/// Park the shell's fiber until `deadline` in frame-sized steps, composing
/// between them: the shell's loop is what composites.
pub fn idle_until(deadline: u64) {
    let step = crate::time::counter_hz() / 60;
    loop {
        let now = crate::time::counter();
        if now >= deadline {
            return;
        }
        let d = deadline.min(now + step);
        if crate::smp::fiber::wait(0, d).is_none() {
            crate::smp::per_core::halt_until(Some(d), crate::smp::per_core::WAKE_HLT_FALLBACK);
        }
        crate::shade::poll_render();
    }
}

/// `cores` where the architecture has nothing of its own to add.
#[cfg(not(target_arch = "x86_64"))]
pub fn intent_cores() {
    let window_ms: u64 = 1000;
    let s0 = sample(true);
    idle_until(s0.at + window_ms * (crate::time::counter_hz() / 1000));
    let s1 = sample(false);
    print_cores(&s0, &s1, None);
    print_fibers(&s0, &s1);
    print_irqs(&s0, &s1);
}
