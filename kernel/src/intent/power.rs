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
    let tsc_hz = crate::interrupts::tsc_freq();
    let tsc_mhz = tsc_hz / 1_000_000;
    let rapl_ok = has_rapl();

    // RAPL energy unit + initial counter
    let (energy_bits, e0) = if rapl_ok {
        let u = rdmsr(MSR_RAPL_POWER_UNIT);
        let e = rdmsr(MSR_PKG_ENERGY_STATUS) & 0xFFFF_FFFF;
        (((u >> 8) & 0x1F) as u32, e)
    } else {
        (0, 0)
    };

    // HWP state (Core 0)
    let hwp = rdmsr(MSR_HWP_REQUEST);
    let hwp_min = (hwp & 0xFF) as u32;
    let hwp_max = ((hwp >> 8) & 0xFF) as u32;
    let hwp_des = ((hwp >> 16) & 0xFF) as u32;
    let hwp_epp = ((hwp >> 24) & 0xFF) as u32;

    let tsc0 = crate::interrupts::rdtsc();

    // Sample period — ~1 second via hlt (timer IRQ wakes every 10ms).
    // We WANT the CPU to idle during this window.
    let deadline = tsc0 + tsc_hz;
    while crate::interrupts::rdtsc() < deadline {
        // SAFETY: ring-0, interrupts enabled, APIC timer will wake us.
        unsafe { core::arch::asm!("hlt"); }
    }

    let tsc1 = crate::interrupts::rdtsc();
    let tsc_delta = tsc1.wrapping_sub(tsc0);

    let mw = if rapl_ok && tsc_delta > 0 {
        let e1 = rdmsr(MSR_PKG_ENERGY_STATUS) & 0xFFFF_FFFF;
        let e_delta = e1.wrapping_sub(e0) & 0xFFFF_FFFF;
        // Energy in microjoules = e_delta * 1_000_000 / 2^energy_bits
        let uj = (e_delta as u128) * 1_000_000u128 >> energy_bits;
        let us = (tsc_delta as u128) / (tsc_mhz as u128).max(1);
        if us > 0 { (uj * 1000) / us } else { 0 }
    } else {
        0
    };

    kprintln!();
    kprintln!("  power — 1s sample");
    kprintln!("  ────────────────────────────────────────");
    if rapl_ok {
        kprintln!("  Package:       {}.{:03} W",
            (mw / 1000) as u32, (mw % 1000) as u32);
    } else {
        kprintln!("  Package:       RAPL unsupported on this CPU");
    }
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
