//! The TSC as the kernel's counter: read, and its frequency calibrated
//! once at boot.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::hw::Port;
use crate::kprintln;

// Kept next to the ports we do drive: a register map with holes in it is
// worse than one unused constant.
// SAFETY: the legacy PIT channel 0; nothing here drives it.
#[allow(dead_code)]
const PIT_CHANNEL0: Port = unsafe { Port::new(0x40) };
// SAFETY: PIT channel 2; the calibration below is its only user.
const PIT_CHANNEL2: Port = unsafe { Port::new(0x42) };
// SAFETY: the PIT mode register; the calibration below is its only user.
const PIT_COMMAND: Port = unsafe { Port::new(0x43) };
// SAFETY: system control port B (speaker gate, PIT channel 2 output); the
// calibration below is its only user.
const SYSTEM_CONTROL_B: Port = unsafe { Port::new(0x61) };
const PIT_BASE_FREQ: u32 = 1_193_182;

/// The counter: the CPU's Time Stamp Counter.
#[inline]
pub fn counter() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: rdtsc is allowed in ring 0 and only writes eax and edx.
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi); }
    ((hi as u64) << 32) | lo as u64
}

/// TSC frequency, set by `calibrate_counter`.
static TSC_FREQ: AtomicU64 = AtomicU64::new(2_000_000_000);

/// Counter frequency in Hz.
#[inline]
pub fn counter_hz() -> u64 {
    TSC_FREQ.load(Ordering::Relaxed)
}

/// Raw CPUID 0x15 values for diagnostics (stored at boot)
static CPUID15_EAX: AtomicU64 = AtomicU64::new(0);
static CPUID15_EBX: AtomicU64 = AtomicU64::new(0);
static CPUID15_ECX: AtomicU64 = AtomicU64::new(0);

/// Get raw CPUID 0x15 values: (eax, ebx, ecx)
pub fn cpuid15() -> (u32, u32, u32) {
    (CPUID15_EAX.load(Ordering::Relaxed) as u32,
     CPUID15_EBX.load(Ordering::Relaxed) as u32,
     CPUID15_ECX.load(Ordering::Relaxed) as u32)
}

/// Find the TSC frequency, once at boot.
pub fn calibrate_counter() {
    // CPUID leaf 0x15: TSC frequency = ECX * EBX / EAX
    // EAX = denominator, EBX = numerator, ECX = crystal clock (Hz)
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    // SAFETY: CPUID exists on every x86_64 CPU and only writes registers; rbx,
    // which LLVM reserves, is saved and restored around it.
    unsafe {
        // rbx is reserved by LLVM, so save/restore manually
        let ebx_out: u64;
        let ecx_out: u64;
        core::arch::asm!(
            "push rbx",
            "mov eax, 0x15",
            "xor ecx, ecx",
            "cpuid",
            "mov {0}, rbx",
            "mov {1}, rcx",
            "pop rbx",
            out(reg) ebx_out,
            out(reg) ecx_out,
            out("eax") eax,
            out("edx") _,
        );
        ebx = ebx_out as u32;
        ecx = ecx_out as u32;
    }
    CPUID15_EAX.store(eax as u64, Ordering::Relaxed);
    CPUID15_EBX.store(ebx as u64, Ordering::Relaxed);
    CPUID15_ECX.store(ecx as u64, Ordering::Relaxed);

    if eax > 0 && ebx > 0 && ecx > 0 {
        let freq = (ecx as u64 * ebx as u64) / eax as u64;
        if freq > 100_000_000 {
            TSC_FREQ.store(freq, Ordering::Relaxed);
            crate::kdebug!("[npk] TSC: {} MHz (CPUID 0x15: crystal={}Hz ratio={}/{})",
                freq / 1_000_000, ecx, ebx, eax);
            return;
        }
    }
    // CPUID 0x15 absent (always on AMD, it is an Intel-only leaf). Calibrate
    // against PIT channel 2 (vendor-independent; Linux's quick_pit_calibrate
    // approach). Every counter-derived time depends on this, including
    // delay_ms, run_slice's SLICE_MS and the guest's `tsc_early_khz=` cmdline.
    if let Some(freq) = pit_calibrate_tsc() {
        TSC_FREQ.store(freq, Ordering::Relaxed);
        crate::kdebug!("[npk] TSC: {} MHz (PIT ch2 calibration)", freq / 1_000_000);
        return;
    }

    // Last resort: 2 GHz default (PIT also unavailable).
    TSC_FREQ.store(2_000_000_000, Ordering::Relaxed);
    kprintln!("[npk] TSC: 2000 MHz (default — CPUID 0x15 + PIT both unavailable)");
}

/// Measure TSC over a fixed PIT channel-2 window (no IRQs needed —
/// polled via the timer-2 output bit in port 0x61). Channel 2 is the
/// "speaker" timer; gating it via 0x61 starts a one-shot countdown we
/// can poll, so this works before any IRQ/timer setup and on both
/// bare-metal AMD and QEMU/KVM. Returns the TSC frequency in Hz, or
/// None if the PIT isn't counting / the result is implausible.
fn pit_calibrate_tsc() -> Option<u64> {
    // Single-threaded boot context.
    // Gate2 on, speaker off (preserve the other bits to restore).
    let p61 = SYSTEM_CONTROL_B.inb();
    SYSTEM_CONTROL_B.outb((p61 & 0xFC) | 0x01);
    // Channel 2, access lobyte+hibyte, mode 0 (terminal count),
    // binary. Mode 0: OUT (0x61 bit 5) is low while counting,
    // goes high at terminal count.
    PIT_COMMAND.outb(0xB0);
    // Initial count 0xFFFF → 0x10000 input clocks @ 1.193182 MHz
    // ≈ 54.9 ms calibration window.
    PIT_CHANNEL2.outb(0xFF);
    PIT_CHANNEL2.outb(0xFF);

    let t0 = counter();
    let mut spins: u64 = 0;
    while SYSTEM_CONTROL_B.inb() & 0x20 == 0 {
        spins += 1;
        if spins > 2_000_000_000 {
            SYSTEM_CONTROL_B.outb(p61 & 0xFC); // gate off, restore
            return None; // PIT not advancing
        }
    }
    let t1 = counter();
    SYSTEM_CONTROL_B.outb(p61 & 0xFC); // gate off, restore original bits

    let cycles = t1.wrapping_sub(t0);
    // 0x10000 PIT clocks at PIT_BASE_FREQ Hz.
    let freq = cycles.saturating_mul(PIT_BASE_FREQ as u64) / 0x10000;
    if (500_000_000..=10_000_000_000).contains(&freq) {
        Some(freq)
    } else {
        None
    }
}
