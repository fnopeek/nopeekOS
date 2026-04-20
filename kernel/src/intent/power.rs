//! power — diagnostic snapshot of CPU/power state.
//!
//! Samples per-core effective frequency + hardware activity (from APERF/MPERF
//! tracked in smp::per_core), package energy (RAPL MSR 0x611), HWP request
//! (MSR 0x774 on Core 0). Reports over a 1-second interval to expose whether
//! cores actually idle.
//!
//! Package C-state residency MSRs (0x3F8/3F9/3FA/60D) are omitted — their
//! presence varies across Intel families and we have no #GP recovery.
//! Revisit when an IDT-level safe_rdmsr lands.

use crate::kprintln;
use crate::smp::per_core;

/// MSR 0x606 IA32_RAPL_POWER_UNIT — bits [12:8] = energy unit (Joules = 2^-x)
const MSR_RAPL_POWER_UNIT: u32 = 0x606;
/// MSR 0x611 MSR_PKG_ENERGY_STATUS — 32-bit energy counter (wraps)
const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
/// MSR 0x774 IA32_HWP_REQUEST — per-core perf/EPP request
const MSR_HWP_REQUEST: u32 = 0x774;

#[inline]
fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: ring-0, caller guarantees MSR is supported on this CPU.
    unsafe { core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi); }
    ((hi as u64) << 32) | lo as u64
}

/// CPUID.06H:EAX bit 14 — RAPL Package thermal and power interface support.
fn has_rapl() -> bool {
    let eax: u32;
    // SAFETY: CPUID leaf 6 exists on all x86_64.
    unsafe {
        core::arch::asm!(
            "push rbx", "mov eax, 6", "cpuid", "mov {0:e}, eax", "pop rbx",
            out(reg) eax, out("ecx") _, out("edx") _,
        );
    }
    eax & (1 << 14) != 0
}

pub fn intent_power() {
    kprintln!();
    kprintln!("  power — diagnostic snapshot");
    kprintln!("  ────────────────────────────────────────");

    // Step 1: CPUID — cannot trap
    let rapl_ok = has_rapl();
    kprintln!("  [1] RAPL supported (CPUID.06H:EAX[14]): {}", rapl_ok);

    // Step 2: HWP_REQUEST — HWP was enabled at boot, so MSR 0x774 must exist
    let hwp = rdmsr(MSR_HWP_REQUEST);
    let hwp_min = (hwp & 0xFF) as u32;
    let hwp_max = ((hwp >> 8) & 0xFF) as u32;
    let hwp_des = ((hwp >> 16) & 0xFF) as u32;
    let hwp_epp = ((hwp >> 24) & 0xFF) as u32;
    kprintln!("  [2] HWP req    min={} max={} desired={} EPP={}",
        hwp_min, hwp_max, hwp_des, hwp_epp);
    kprintln!("      HWP caps   {}-{} MHz",
        per_core::min_eff_mhz(), per_core::max_turbo_mhz());

    // Step 3: RAPL — only if CPUID signalled support
    let (energy_bits, e0) = if rapl_ok {
        let u = rdmsr(MSR_RAPL_POWER_UNIT);
        let e = rdmsr(MSR_PKG_ENERGY_STATUS) & 0xFFFF_FFFF;
        let bits = ((u >> 8) & 0x1F) as u32;
        kprintln!("  [3] RAPL unit  2^-{} J/unit, initial counter={:#x}", bits, e);
        (bits, e)
    } else {
        kprintln!("  [3] RAPL skipped (not supported)");
        (0, 0)
    };

    // Step 4: 1-second wait driven by the 100Hz tick counter (timer IRQ).
    // Using ticks() instead of raw rdtsc spin avoids any issue where
    // rdtsc returns a value that makes the deadline unreachable.
    let t0_ticks = crate::interrupts::ticks();
    kprintln!("  [4] sampling for 100 ticks (~1s) via hlt...");

    while crate::interrupts::ticks().wrapping_sub(t0_ticks) < 100 {
        // SAFETY: Core 0 event-dispatcher context — IF=1 (keyboard IRQ works).
        // Timer IRQ increments ticks every 10ms, guaranteeing progress.
        unsafe { core::arch::asm!("hlt"); }
    }

    let elapsed_ticks = crate::interrupts::ticks().wrapping_sub(t0_ticks);
    kprintln!("  [5] sample done ({} ticks elapsed)", elapsed_ticks);

    // Step 6: final RAPL read + package power calculation
    if rapl_ok {
        let e1 = rdmsr(MSR_PKG_ENERGY_STATUS) & 0xFFFF_FFFF;
        let e_delta = e1.wrapping_sub(e0) & 0xFFFF_FFFF;
        // ticks are 100Hz → elapsed_ticks/100 seconds
        // Energy J = e_delta / 2^energy_bits
        // Power W = Energy / seconds = (e_delta / 2^bits) * 100 / elapsed_ticks
        // In milliwatts: (e_delta * 1000 * 100 / 2^bits) / elapsed_ticks
        let mw = if elapsed_ticks > 0 {
            let num = (e_delta as u128) * 100_000u128 >> energy_bits;
            num / elapsed_ticks as u128
        } else {
            0
        };
        kprintln!("  [6] e_delta={}  ->  Package: {}.{:03} W",
            e_delta, (mw / 1000) as u32, (mw % 1000) as u32);
    }
    kprintln!();

    // Per-core view. CORE_MHZ / CORE_MPERF_PCT updated by each core's own
    // scheduler path (APs) or timer IRQ (Core 0) — values are recent.
    let n = per_core::core_count();
    kprintln!("  Core │ freq MHz │ HW busy │ task busy");
    kprintln!("  ─────┼──────────┼─────────┼──────────");
    for c in 0..n {
        kprintln!("  {:>3}  │ {:>8} │ {:>5}%  │ {:>5}%",
            c,
            per_core::core_freq_mhz(c),
            per_core::core_mperf_pct(c),
            per_core::core_usage(c));
    }
    kprintln!();
}
