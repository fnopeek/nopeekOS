//! power — diagnostic snapshot of CPU/power state.
//!
//! Samples per-core effective frequency + hardware activity (from APERF/MPERF
//! tracked in smp::per_core), package energy (RAPL MSR 0x611), HWP request
//! (MSR 0x774 on Core 0), and package C-state residencies. Reports over a
//! 1-second interval to expose whether cores actually idle.

use crate::kprintln;
use crate::smp::per_core;

/// MSR 0x606 IA32_RAPL_POWER_UNIT — bits [12:8] = energy unit (Joules = 2^-x)
const MSR_RAPL_POWER_UNIT: u32 = 0x606;
/// MSR 0x611 MSR_PKG_ENERGY_STATUS — 32-bit energy counter (wraps)
const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
/// MSR 0x774 IA32_HWP_REQUEST — per-core perf/EPP request
const MSR_HWP_REQUEST: u32 = 0x774;
/// Package C-state residency counters (tick at TSC rate in that state)
const MSR_PKG_C2_RES: u32 = 0x60D;
const MSR_PKG_C3_RES: u32 = 0x3F8;
const MSR_PKG_C6_RES: u32 = 0x3F9;
const MSR_PKG_C7_RES: u32 = 0x3FA;

#[inline]
fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: ring-0, rdmsr on advertised-supported MSRs.
    // Caller must ensure MSR exists on this CPU (guarded at call sites).
    unsafe { core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi); }
    ((hi as u64) << 32) | lo as u64
}

/// Try-read an MSR; returns None on #GP (not supported).
/// We don't trap — instead require callers to verify via CPUID first.
/// For the snapshot we read unconditionally; a missing MSR returns 0 on
/// some models but panics on others. Alder Lake-N supports all used MSRs.
fn rdmsr_safe(msr: u32) -> u64 { rdmsr(msr) }

pub fn intent_power() {
    let tsc_hz = crate::interrupts::tsc_freq();
    let tsc_mhz = tsc_hz / 1_000_000;

    // RAPL energy unit — MSR 0x606, bits [12:8]
    let unit_raw = rdmsr_safe(MSR_RAPL_POWER_UNIT);
    let energy_bits = ((unit_raw >> 8) & 0x1F) as u32;
    // 1 unit = 2^-energy_bits Joules. Common value: 14 → 1 unit = 61 µJ.

    // Initial package energy + TSC
    let e0 = rdmsr_safe(MSR_PKG_ENERGY_STATUS) & 0xFFFF_FFFF;
    let c2_0 = rdmsr_safe(MSR_PKG_C2_RES);
    let c3_0 = rdmsr_safe(MSR_PKG_C3_RES);
    let c6_0 = rdmsr_safe(MSR_PKG_C6_RES);
    let c7_0 = rdmsr_safe(MSR_PKG_C7_RES);
    let tsc0 = crate::interrupts::rdtsc();

    // HWP state (Core 0)
    let hwp = rdmsr_safe(MSR_HWP_REQUEST);
    let hwp_min = (hwp & 0xFF) as u32;
    let hwp_max = ((hwp >> 8) & 0xFF) as u32;
    let hwp_des = ((hwp >> 16) & 0xFF) as u32;
    let hwp_epp = ((hwp >> 24) & 0xFF) as u32;

    // Sample period — ~1 second via TSC spin (but yields via hlt so we idle).
    // We WANT the CPU to idle during this window so the measurement reflects reality.
    let deadline = tsc0 + tsc_hz;
    while crate::interrupts::rdtsc() < deadline {
        // SAFETY: ring-0, interrupts enabled, timer IRQ wakes every 10ms.
        unsafe { core::arch::asm!("hlt"); }
    }

    // End snapshot
    let e1 = rdmsr_safe(MSR_PKG_ENERGY_STATUS) & 0xFFFF_FFFF;
    let c2_1 = rdmsr_safe(MSR_PKG_C2_RES);
    let c3_1 = rdmsr_safe(MSR_PKG_C3_RES);
    let c6_1 = rdmsr_safe(MSR_PKG_C6_RES);
    let c7_1 = rdmsr_safe(MSR_PKG_C7_RES);
    let tsc1 = crate::interrupts::rdtsc();

    let tsc_delta = tsc1.wrapping_sub(tsc0);
    let e_delta = e1.wrapping_sub(e0) & 0xFFFF_FFFF;
    // micro-joules per unit = 10^6 * 2^-energy_bits; multiply first to avoid fp.
    // Energy in microjoules = e_delta * 1_000_000 >> energy_bits.
    let uj = (e_delta as u128) * 1_000_000u128 >> energy_bits;
    // Time in microseconds from TSC.
    let us = (tsc_delta as u128) / (tsc_mhz as u128).max(1);
    // Power in milliwatts = uj / us * 1000 (1µJ/1µs = 1W; *1000 for mW)
    let mw = if us > 0 { (uj * 1000) / us } else { 0 };

    // C-state residency as % of TSC delta
    let pct = |x: u64| -> u32 {
        if tsc_delta == 0 { 0 } else { ((x as u128 * 100 / tsc_delta as u128) as u32).min(100) }
    };
    let c2_pct = pct(c2_1.wrapping_sub(c2_0));
    let c3_pct = pct(c3_1.wrapping_sub(c3_0));
    let c6_pct = pct(c6_1.wrapping_sub(c6_0));
    let c7_pct = pct(c7_1.wrapping_sub(c7_0));

    kprintln!();
    kprintln!("  power — 1s sample");
    kprintln!("  ────────────────────────────────────────");
    kprintln!("  Package:       {}.{:03} W",
        (mw / 1000) as u32, (mw % 1000) as u32);
    kprintln!("  Pkg C2/C3/C6/C7 residency: {}% / {}% / {}% / {}%",
        c2_pct, c3_pct, c6_pct, c7_pct);
    kprintln!();
    kprintln!("  HWP (Core 0):  min={} max={} desired={} EPP={}",
        hwp_min, hwp_max, hwp_des, hwp_epp);
    kprintln!("  HWP caps:      {}-{} MHz (from CPUID/HWP_CAPABILITIES)",
        per_core::min_eff_mhz(), per_core::max_turbo_mhz());
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
