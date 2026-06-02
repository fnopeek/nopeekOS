//! Interrupt Descriptor Table + PIC 8259
//!
//! Exception handlers + timer IRQ for hlt wakeup.
//! Phase 2+: keyboard IRQ, serial IRQ, TSS with IST for double fault

use crate::serial::{outb, inb};
use crate::kprintln;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Monotonic tick counter, incremented by timer IRQ at 100 Hz.
/// Used on hardware with working PIT (e.g. QEMU).
static TICKS: AtomicU64 = AtomicU64::new(0);

/// TSC value at boot — for deriving ticks on hardware without PIT.
static BOOT_TSC: AtomicU64 = AtomicU64::new(0);

/// Call once at boot after calibrate_tsc().
pub fn init_tsc_ticks() {
    BOOT_TSC.store(rdtsc(), Ordering::Relaxed);
}

/// Monotonic 100 Hz tick counter. Works on all hardware:
/// uses PIT timer IRQ if available, falls back to TSC.
pub fn ticks() -> u64 {
    let pit = TICKS.load(Ordering::Relaxed);
    if pit > 0 { return pit; }
    // PIT not working (NUC, UEFI-only, no legacy timer) — derive from TSC
    let freq = TSC_FREQ.load(Ordering::Relaxed);
    let period = freq / 100; // TSC cycles per 10ms tick
    if period == 0 { return 0; }
    let boot = BOOT_TSC.load(Ordering::Relaxed);
    (rdtsc() - boot) / period
}

/// Seconds since boot (approximate)
pub fn uptime_secs() -> u64 {
    ticks() / 100
}

/// Read CPU Time Stamp Counter (works on all x86_64, no PIC needed).
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi); }
    ((hi as u64) << 32) | lo as u64
}

/// Estimate TSC frequency by calibrating against PIT (called once at boot).
static TSC_FREQ: AtomicU64 = AtomicU64::new(2_000_000_000); // default 2GHz

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

pub fn calibrate_tsc() {
    // CPUID leaf 0x15: TSC frequency = ECX * EBX / EAX
    // EAX = denominator, EBX = numerator, ECX = crystal clock (Hz)
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
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
            kprintln!("[npk] TSC: {} MHz (CPUID 0x15: crystal={}Hz ratio={}/{})",
                freq / 1_000_000, ecx, ebx, eax);
            return;
        }
    }
    // CPUID 0x15 absent (always on AMD — Intel-only leaf). Calibrate
    // against PIT channel 2 (vendor-independent; Linux's
    // quick_pit_calibrate approach). MUST work or every TSC-derived
    // time is wrong: host `top` per-app time, delay_ms, run_slice's
    // SLICE_MS, AND the guest's `tsc_early_khz=` cmdline (→ guest
    // clock 2-2.3× fast → librewolf timed-wait crash). The old
    // hardcoded 2 GHz was that bug.
    if let Some(freq) = pit_calibrate_tsc() {
        TSC_FREQ.store(freq, Ordering::Relaxed);
        kprintln!("[npk] TSC: {} MHz (PIT ch2 calibration)", freq / 1_000_000);
        return;
    }

    // Last resort: 2 GHz default (PIT also unavailable).
    TSC_FREQ.store(2_000_000_000, Ordering::Relaxed);
    kprintln!("[npk] TSC: 2000 MHz (default — CPUID 0x15 + PIT both unavailable)");
}

#[inline]
unsafe fn cal_inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", out("al") v, in("dx") port,
                     options(nomem, nostack, preserves_flags));
    v
}

#[inline]
unsafe fn cal_outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("al") val, in("dx") port,
                     options(nomem, nostack, preserves_flags));
}

/// Measure TSC over a fixed PIT channel-2 window (no IRQs needed —
/// polled via the timer-2 output bit in port 0x61). Channel 2 is the
/// "speaker" timer; gating it via 0x61 starts a one-shot countdown we
/// can poll, so this works before any IRQ/timer setup and on both
/// bare-metal AMD and QEMU/KVM. Returns the TSC frequency in Hz, or
/// None if the PIT isn't counting / the result is implausible.
fn pit_calibrate_tsc() -> Option<u64> {
    // SAFETY: legacy PIT/0x61 ports, single-threaded boot context.
    unsafe {
        // Gate2 on, speaker off (preserve the other bits to restore).
        let p61 = cal_inb(0x61);
        cal_outb(0x61, (p61 & 0xFC) | 0x01);
        // Channel 2, access lobyte+hibyte, mode 0 (terminal count),
        // binary. Mode 0: OUT (0x61 bit 5) is low while counting,
        // goes high at terminal count.
        cal_outb(0x43, 0xB0);
        // Initial count 0xFFFF → 0x10000 input clocks @ 1.193182 MHz
        // ≈ 54.9 ms calibration window.
        cal_outb(0x42, 0xFF);
        cal_outb(0x42, 0xFF);

        let t0 = rdtsc();
        let mut spins: u64 = 0;
        while cal_inb(0x61) & 0x20 == 0 {
            spins += 1;
            if spins > 2_000_000_000 {
                cal_outb(0x61, p61 & 0xFC); // gate off, restore
                return None; // PIT not advancing
            }
        }
        let t1 = rdtsc();
        cal_outb(0x61, p61 & 0xFC); // gate off, restore original bits

        let cycles = t1.wrapping_sub(t0);
        // 0x10000 PIT clocks at PIT_BASE_FREQ Hz.
        let freq = cycles.saturating_mul(PIT_BASE_FREQ as u64) / 0x10000;
        if (500_000_000..=10_000_000_000).contains(&freq) {
            Some(freq)
        } else {
            None
        }
    }
}

/// Get TSC frequency in Hz.
pub fn tsc_freq() -> u64 {
    TSC_FREQ.load(Ordering::Relaxed)
}

/// Busy-wait for approximately `ms` milliseconds using TSC.
pub fn delay_ms(ms: u64) {
    let ticks_per_ms = TSC_FREQ.load(Ordering::Relaxed) / 1000;
    let target = rdtsc() + ms * ticks_per_ms;
    while rdtsc() < target {
        core::hint::spin_loop();
    }
}

const PIT_CHANNEL0: u16 = 0x40;
const PIT_COMMAND: u16 = 0x43;
const PIT_BASE_FREQ: u32 = 1_193_182;
const TARGET_FREQ: u32 = 100; // 100 Hz = 10ms per tick

// IDT entry: 64-bit interrupt gate descriptor (16 bytes)
#[derive(Clone, Copy)]
#[repr(C, packed)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    _reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        IdtEntry {
            offset_low: 0, selector: 0, ist: 0, type_attr: 0,
            offset_mid: 0, offset_high: 0, _reserved: 0,
        }
    }

    fn set_handler(&mut self, handler: u64) {
        self.offset_low = handler as u16;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.selector = 0x08; // GDT code segment from boot.s
        self.ist = 0;
        // 0x8E = Present | DPL=0 | 64-bit Interrupt Gate
        // DPL=0: no ring-3 software interrupt injection possible
        self.type_attr = 0x8E;
        self._reserved = 0;
    }
}

#[repr(C, packed)]
struct IdtRegister {
    limit: u16,
    base: u64,
}

#[repr(C)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

const IDT_SIZE: usize = 256;

// SAFETY: Written exactly once in init() before sti, then only read by CPU
static mut IDT: [IdtEntry; IDT_SIZE] = [IdtEntry::missing(); IDT_SIZE];

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;
const PIC_EOI: u8 = 0x20;
const PIC_OFFSET_MASTER: u8 = 32; // IRQ0-7 → vectors 32-39
const PIC_OFFSET_SLAVE: u8 = 40;  // IRQ8-15 → vectors 40-47

pub fn init() {
    unsafe {
        // Exception handlers
        IDT[0].set_handler(divide_error_handler as *const () as u64);
        IDT[3].set_handler(breakpoint_handler as *const () as u64);
        IDT[6].set_handler(invalid_opcode_handler as *const () as u64);
        IDT[8].set_handler(double_fault_handler as *const () as u64);
        IDT[13].set_handler(gp_fault_handler as *const () as u64);
        IDT[14].set_handler(page_fault_handler as *const () as u64);

        // Hardware interrupt handlers
        IDT[PIC_OFFSET_MASTER as usize].set_handler(timer_handler as *const () as u64);
        IDT[(PIC_OFFSET_MASTER + 1) as usize].set_handler(keyboard_handler as *const () as u64);
        // Dedicated-microvm-core LAPIC timer (substrate rework A2).
        // Global IDT is shared by all cores; only the dedicated core
        // ever raises this vector (its own LAPIC timer) — a pure EOI.
        IDT[DEDICATED_VM_TIMER_VECTOR as usize]
            .set_handler(dedicated_vm_timer_handler as *const () as u64);
        // Per-core worker idle timer (one per AP) — pure EOI, see
        // `arm_worker_timer`. Shared IDT; each worker raises it on its
        // own LAPIC so its idle HLT wakes without a host tick.
        IDT[WORKER_TIMER_VECTOR as usize]
            .set_handler(worker_timer_handler as *const () as u64);

        // Load IDT
        let idt_reg = IdtRegister {
            limit: (IDT_SIZE * core::mem::size_of::<IdtEntry>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        // SAFETY: IDT is fully initialized above
        core::arch::asm!("lidt [{}]", in(reg) &idt_reg);

        // Do NOT re-initialize the legacy 8259 PIC. Writing ICW1 (0x11)
        // to its command port (0x20/0xA0) traps via SMI on some UEFI
        // firmware (HP/Insyde virtualize the legacy PIC in SMM under APIC
        // mode) and resets the machine post-ExitBootServices — confirmed
        // by colour-bisecting a serial-less HP notebook. UEFI hands control
        // over in APIC mode, so we just MASK the PIC fully via its data
        // ports (plain mask registers — safe) and drive ticks from the
        // Local APIC timer (interrupts::init_apic_timer). A masked,
        // un-remapped PIC delivers no IRQs, so the default 0x08-0x0F vector
        // collision with CPU exceptions never happens.
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);

        // SAFETY: IDT loaded, PIC fully masked, handlers set.
        core::arch::asm!("sti");
    }
}

unsafe fn pic_eoi(irq: u8) {
    unsafe {
        if irq >= 8 { outb(PIC2_CMD, PIC_EOI); }
        outb(PIC1_CMD, PIC_EOI);
    }
}

// === Exception Handlers ===

extern "x86-interrupt" fn divide_error_handler(frame: InterruptStackFrame) {
    kprintln!();
    kprintln!("[npk] !!! DIVIDE ERROR (INT 0) !!!");
    kprintln!("[npk] RIP: {:#018x}", frame.instruction_pointer);
    kprintln!("[npk] RSP: {:#018x}", frame.stack_pointer);
    halt_loop();
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    kprintln!();
    kprintln!("[npk] BREAKPOINT (INT 3) at {:#018x}", frame.instruction_pointer);
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    kprintln!();
    kprintln!("[npk] !!! INVALID OPCODE (INT 6) !!!");
    kprintln!("[npk] RIP: {:#018x}", frame.instruction_pointer);
    kprintln!("[npk] RSP: {:#018x}", frame.stack_pointer);
    halt_loop();
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, error_code: u64) -> ! {
    kprintln!();
    kprintln!("[npk] !!! DOUBLE FAULT (INT 8) !!!");
    kprintln!("[npk] Error code: {:#x}", error_code);
    kprintln!("[npk] RIP: {:#018x}", frame.instruction_pointer);
    kprintln!("[npk] RSP: {:#018x}", frame.stack_pointer);
    halt_loop();
}

extern "x86-interrupt" fn gp_fault_handler(frame: InterruptStackFrame, error_code: u64) {
    kprintln!();
    kprintln!("[npk] !!! GENERAL PROTECTION FAULT (INT 13) !!!");
    kprintln!("[npk] Error code: {:#x}", error_code);
    kprintln!("[npk] RIP: {:#018x}", frame.instruction_pointer);
    kprintln!("[npk] RSP: {:#018x}", frame.stack_pointer);
    halt_loop();
}

extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, error_code: u64) {
    let cr2: u64;
    // SAFETY: Reading CR2 is side-effect-free
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2); }

    kprintln!();
    kprintln!("[npk] !!! PAGE FAULT (INT 14) !!!");
    kprintln!("[npk] Faulting address: {:#018x}", cr2);
    kprintln!("[npk] Error code: {:#x}", error_code);
    kprintln!("[npk] RIP: {:#018x}", frame.instruction_pointer);
    kprintln!("[npk] RSP: {:#018x}", frame.stack_pointer);

    // Best-effort backtrace: scan the stack for words that look like return
    // addresses into kernel code (link base 0x1000_0000 .. ~+8 MB). On this HW
    // the PIE kernel runs at its link base, so these map directly via
    // `addr2line -e target/.../nopeekos-kernel <addr>`. Only scanned when RSP
    // is inside the identity-mapped range so the scan itself can't fault.
    let rsp = frame.stack_pointer;
    if rsp >= 0x10_0000 && rsp < 0x10_0000_0000 {
        kprintln!("[npk] stack trace (return addrs in kernel code):");
        let mut p = rsp;
        let mut printed = 0;
        let mut scanned = 0;
        while scanned < 1024 && printed < 24 {
            // SAFETY: rsp is within the identity-mapped range (checked above),
            // so this read cannot page-fault (re-entry would triple-fault).
            let v = unsafe { core::ptr::read_volatile(p as *const u64) };
            if v >= 0x1000_0000 && v < 0x1080_0000 {
                kprintln!("[npk]   {:#018x}", v);
                printed += 1;
            }
            p += 8;
            scanned += 1;
        }
    }
    halt_loop();
}

// === IRQ Handlers ===

extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    let tick = TICKS.fetch_add(1, Ordering::Relaxed);
    // Wake attribution: IRQ0 fires on the BSP (Core 0).
    crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_TIMER);
    // Drain USB events from interrupt context (try_lock, never blocks)
    crate::xhci::poll_events_irq();
    // Drain the polled PS/2 mouse/keyboard here too (no-op unless a PS/2
    // mouse is up) — same model as the USB drain → smooth cursor regardless
    // of the run loop's HLT/spin.
    crate::keyboard::poll_ps2_irq();
    // Core 0 busy tracking: add one tick worth of busy TSC (Core 0 runs event loop)
    let freq = TSC_FREQ.load(Ordering::Relaxed);
    if freq > 0 {
        crate::smp::per_core::add_busy_tsc(0, freq / 100);
    }
    // Update BSP frequency once per second (for top display)
    if tick % 100 == 0 {
        crate::smp::per_core::update_core_freq(0);
    }
    unsafe { pic_eoi(0); }
}

extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    // Wake attribution: IRQ1 fires on the BSP (Core 0).
    crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_KEYBOARD);
    crate::keyboard::irq_handler();
    unsafe { pic_eoi(1); }
}

/// APIC timer handler — fires on hardware without PIT (NUC, UEFI-only).
/// Same function as PIT timer: tick counter + USB event drain.
extern "x86-interrupt" fn apic_timer_handler(_frame: InterruptStackFrame) {
    let tick = TICKS.fetch_add(1, Ordering::Relaxed);
    // Wake attribution: the APIC timer is armed only on the BSP (Core 0).
    crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_TIMER);
    crate::xhci::poll_events_irq();
    crate::keyboard::poll_ps2_irq();
    // Core 0 busy tracking: add one tick worth of busy TSC (Core 0 runs event loop)
    let freq = TSC_FREQ.load(Ordering::Relaxed);
    if freq > 0 {
        crate::smp::per_core::add_busy_tsc(0, freq / 100);
    }
    // Update BSP frequency once per second (for top display)
    if tick % 100 == 0 {
        crate::smp::per_core::update_core_freq(0);
    }
    // APIC EOI: write 0 to End-of-Interrupt register
    let apic_base = APIC_BASE.load(Ordering::Relaxed);
    if apic_base != 0 {
        unsafe { core::ptr::write_volatile((apic_base + 0xB0) as *mut u32, 0); }
    }
}

/// APIC base address (from MSR 0x1B).
static APIC_BASE: AtomicU64 = AtomicU64::new(0);
const APIC_TIMER_VECTOR: u8 = 48;

/// Get cached APIC base (for per-core identification via LAPIC ID register).
pub fn apic_base() -> u64 { APIC_BASE.load(Ordering::Relaxed) }

// ── Dedicated-microvm-core LAPIC timer (substrate rework A2) ───────
//
// The core dedicated to the microvm (see `smp::per_core`) runs the
// guest in a continuous VMRESUME/VMRUN loop. The guest's only time
// source is an IRQ0 the run loop injects — which it can only do when
// VMRUN *returns*. An idle guest (MWAIT) only yields VMRUN on a
// *physical* interrupt (SVM INTERCEPT_INTR / VMX ext-int exiting).
// The host 100 Hz timer fires on Core 0, NOT here, so without a
// per-core source this core's guest would freeze the moment it idles.
// So while a VM runs we arm THIS core's own LAPIC timer (periodic);
// every fire → #VMEXIT(INTR) → the run loop regains control and does
// its TICKS-paced injection. Disarmed when no VM runs, so a parked
// dedicated core still truly idles. The IRQ only EOIs — it must NOT
// touch the global TICKS (Core 0 owns the canonical wall clock).
const DEDICATED_VM_TIMER_VECTOR: u8 = 49;

/// LAPIC MMIO base for the dedicated-core EOI (xAPIC base is the same
/// physical address on every core; each core's access hits its own
/// LAPIC). Set by `arm_dedicated_vm_timer` because `APIC_BASE` is only
/// populated on the APIC-timer path (Core 0 may be on PIT).
static DEDICATED_APIC_BASE: AtomicU64 = AtomicU64::new(0);

extern "x86-interrupt" fn dedicated_vm_timer_handler(_frame: InterruptStackFrame) {
    // EOI only — purpose is purely to make VMRUN return periodically.
    let base = DEDICATED_APIC_BASE.load(Ordering::Relaxed);
    if base != 0 {
        // SAFETY: LAPIC MMIO, identity-mapped, per-core EOI register.
        unsafe { core::ptr::write_volatile((base + 0xB0) as *mut u32, 0); }
    }
}

/// Arm the calling core's LAPIC timer ~1 kHz periodic on
/// `DEDICATED_VM_TIMER_VECTOR`. Called by the dedicated core right
/// before its VM run loop. ~1 kHz (not 100 Hz): finer net/input
/// servicing + slice cadence; the guest IRQ0 stays 100 Hz (TICKS-
/// paced) regardless.
pub fn arm_dedicated_vm_timer() {
    let (lo, hi): (u32, u32);
    // SAFETY: MSR 0x1B (APIC base) is always readable on x86_64.
    unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }
    let base = ((hi as u64) << 32 | lo as u64) & 0xFFFF_FFFF_F000;
    DEDICATED_APIC_BASE.store(base, Ordering::Relaxed);
    let freq = TSC_FREQ.load(Ordering::Relaxed);

    // SAFETY: LAPIC MMIO regs, identity-mapped; this core's own LAPIC.
    unsafe {
        let b = base as *mut u8;
        // Ensure LAPIC enabled (smp_ap_entry already did, defensive).
        let svr = core::ptr::read_volatile(b.add(0xF0) as *const u32);
        core::ptr::write_volatile(b.add(0xF0) as *mut u32, svr | (1 << 8) | 0xFF);
        // Divide = 16.
        core::ptr::write_volatile(b.add(0x3E0) as *mut u32, 0x03);
        // Calibrate ticks/10ms against the TSC.
        core::ptr::write_volatile(b.add(0x380) as *mut u32, 0xFFFF_FFFF);
        let start = rdtsc();
        let tsc_10ms = if freq > 0 { freq / 100 } else { 20_000_000 };
        while rdtsc() - start < tsc_10ms { core::hint::spin_loop(); }
        let per_10ms = 0xFFFF_FFFFu32
            .wrapping_sub(core::ptr::read_volatile(b.add(0x390) as *const u32));
        let initial = (per_10ms / 10).max(1); // /10 → ~1 kHz
        // LVT timer: periodic (bit 17) | vector.
        core::ptr::write_volatile(b.add(0x320) as *mut u32,
            (1 << 17) | DEDICATED_VM_TIMER_VECTOR as u32);
        core::ptr::write_volatile(b.add(0x380) as *mut u32, initial);
    }
}

/// Mask the calling core's LAPIC timer so a parked dedicated core
/// takes no further IRQs. Called after the VM run loop ends.
pub fn disarm_dedicated_vm_timer() {
    let base = DEDICATED_APIC_BASE.load(Ordering::Relaxed);
    if base == 0 { return; }
    // SAFETY: LAPIC MMIO; mask bit 16 + stop the counter.
    unsafe {
        let b = base as *mut u8;
        core::ptr::write_volatile(b.add(0x320) as *mut u32,
            (1 << 16) | DEDICATED_VM_TIMER_VECTOR as u32);
        core::ptr::write_volatile(b.add(0x380) as *mut u32, 0);
    }
}

// ── Per-core worker idle timer ──────────────────────────────────────
//
// Worker APs have no wake source of their own: hardware IRQs route to
// the BSP, and the only other signal was the WORK_AVAILABLE MONITOR
// cacheline, which needs real-HW MWAIT (`cpu-pm=on` passthrough). That
// passthrough is exactly what pinned the host at idle: a passthrough
// HLT/MWAIT never VMEXITs, so KVM never deschedules the idle vCPU
// thread and the host core stays at 100%/turbo. So every worker arms
// its OWN periodic LAPIC timer (100 Hz) and idles with plain HLT: HLT
// VMEXITs → KVM blocks the vCPU thread → the host core actually idles,
// and the timer guarantees the worker wakes to re-check its run-queue /
// sleep deadline on bare metal too (where no host tick exists). Pure
// EOI handler — must NOT touch TICKS (Core 0 owns the wall clock).
const WORKER_TIMER_VECTOR: u8 = 50;
static WORKER_APIC_BASE: AtomicU64 = AtomicU64::new(0);
static WORKER_TIMER_INITIAL: AtomicU32 = AtomicU32::new(0);

extern "x86-interrupt" fn worker_timer_handler(_frame: InterruptStackFrame) {
    let base = WORKER_APIC_BASE.load(Ordering::Relaxed);
    if base != 0 {
        // SAFETY: LAPIC MMIO, identity-mapped; each core EOIs its own LAPIC.
        unsafe { core::ptr::write_volatile((base + 0xB0) as *mut u32, 0); }
    }
}

/// Arm the calling core's LAPIC timer at 100 Hz periodic on
/// `WORKER_TIMER_VECTOR`. Each worker AP calls this once at boot, and
/// the dedicated core re-arms it after a VM exits. Calibrated once
/// against the TSC; later callers reuse the cached count (the LAPIC
/// timer clock is uniform across cores).
pub fn arm_worker_timer() {
    let (lo, hi): (u32, u32);
    // SAFETY: MSR 0x1B (APIC base) is always readable on x86_64 ring 0.
    unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }
    let base = ((hi as u64) << 32 | lo as u64) & 0xFFFF_FFFF_F000;
    WORKER_APIC_BASE.store(base, Ordering::Relaxed);

    // SAFETY: LAPIC MMIO regs, identity-mapped; this core's own LAPIC.
    unsafe {
        let b = base as *mut u8;
        // Ensure LAPIC enabled (smp_ap_entry already did, defensive).
        let svr = core::ptr::read_volatile(b.add(0xF0) as *const u32);
        core::ptr::write_volatile(b.add(0xF0) as *mut u32, svr | (1 << 8) | 0xFF);
        // Divide = 16.
        core::ptr::write_volatile(b.add(0x3E0) as *mut u32, 0x03);
        let mut initial = WORKER_TIMER_INITIAL.load(Ordering::Relaxed);
        if initial == 0 {
            // Calibrate ticks/10ms against the TSC (10 ms = 100 Hz).
            let freq = TSC_FREQ.load(Ordering::Relaxed);
            core::ptr::write_volatile(b.add(0x380) as *mut u32, 0xFFFF_FFFF);
            let start = rdtsc();
            let tsc_10ms = if freq > 0 { freq / 100 } else { 20_000_000 };
            while rdtsc() - start < tsc_10ms { core::hint::spin_loop(); }
            initial = 0xFFFF_FFFFu32
                .wrapping_sub(core::ptr::read_volatile(b.add(0x390) as *const u32))
                .max(1);
            WORKER_TIMER_INITIAL.store(initial, Ordering::Relaxed);
        }
        // LVT timer: periodic (bit 17) | vector.
        core::ptr::write_volatile(b.add(0x320) as *mut u32,
            (1 << 17) | WORKER_TIMER_VECTOR as u32);
        core::ptr::write_volatile(b.add(0x380) as *mut u32, initial);
    }
}

/// Initialize Local APIC timer (for hardware without PIT).
/// Call after init() — detects if PIT is working, sets up APIC timer if not.
pub fn init_apic_timer() {
    // Check if PIT timer is already working (TICKS > 0 after a short delay)
    let t0 = TICKS.load(Ordering::Relaxed);
    // Wait ~50ms using TSC
    let freq = TSC_FREQ.load(Ordering::Relaxed);
    let wait_cycles = freq / 20; // 50ms
    let start = rdtsc();
    while rdtsc() - start < wait_cycles {
        core::hint::spin_loop();
    }
    if TICKS.load(Ordering::Relaxed) > t0 {
        return; // PIT works — no APIC timer needed
    }

    // Read APIC base from MSR 0x1B
    let (lo, hi): (u32, u32);
    unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }
    let apic_base = ((hi as u64) << 32 | lo as u64) & 0xFFFF_FFFF_F000;
    APIC_BASE.store(apic_base, Ordering::Relaxed);

    // Map APIC page (identity-mapped, uncacheable)
    let _ = crate::paging::map_page(apic_base, apic_base,
        crate::paging::PageFlags::PRESENT | crate::paging::PageFlags::WRITABLE | crate::paging::PageFlags::NO_CACHE);

    unsafe {
        let base = apic_base as *mut u8;

        // Install APIC timer handler in IDT
        IDT[APIC_TIMER_VECTOR as usize].set_handler(apic_timer_handler as *const () as u64);

        // Enable APIC: set Spurious Interrupt Vector Register (offset 0xF0)
        // Bit 8 = APIC enable, bits 0-7 = spurious vector (use 0xFF)
        let svr = core::ptr::read_volatile(base.add(0xF0) as *const u32);
        core::ptr::write_volatile(base.add(0xF0) as *mut u32, svr | (1 << 8) | 0xFF);

        // Set timer divide = 16 (offset 0x3E0, value 0x03)
        core::ptr::write_volatile(base.add(0x3E0) as *mut u32, 0x03);

        // Calibrate: measure APIC ticks per 10ms using TSC
        core::ptr::write_volatile(base.add(0x380) as *mut u32, 0xFFFF_FFFF); // max initial count
        let tsc_start = rdtsc();
        let tsc_10ms = freq / 100;
        while rdtsc() - tsc_start < tsc_10ms { core::hint::spin_loop(); }
        let elapsed = 0xFFFF_FFFFu32 - core::ptr::read_volatile(base.add(0x390) as *const u32);

        // Set timer: periodic mode, vector APIC_TIMER_VECTOR
        // LVT Timer Register (offset 0x320): bit 17 = periodic, bits 0-7 = vector
        core::ptr::write_volatile(base.add(0x320) as *mut u32,
            (1 << 17) | APIC_TIMER_VECTOR as u32);

        // Set initial count (ticks per 10ms = 100Hz)
        core::ptr::write_volatile(base.add(0x380) as *mut u32, elapsed);

        kprintln!("[npk] APIC timer: {}Hz (base={:#x}, ticks/10ms={})",
            TARGET_FREQ, apic_base, elapsed);
    }
}

fn halt_loop() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}
