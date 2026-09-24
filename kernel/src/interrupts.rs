//! Interrupt Descriptor Table + PIC 8259
//!
//! Exception handlers + timer IRQ for hlt wakeup.
//! Phase 2+: keyboard IRQ, serial IRQ, TSS with IST for double fault

use crate::serial::outb;
use crate::kprintln;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Timer interrupts taken on Core 0. NOT a clock: only used to tell
/// whether the PIT is running (`init_apic_timer`) and to pace the once-a-
/// second statistics in the tick handler.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// TSC value at boot — the zero point of every clock below.
static BOOT_TSC: AtomicU64 = AtomicU64::new(0);

/// Call once at boot after calibrate_tsc().
pub fn init_tsc_ticks() {
    BOOT_TSC.store(rdtsc(), Ordering::Relaxed);
}

/// 100 Hz ticks since boot, derived from the TSC.
///
/// **The clock is the TSC, on every machine.** This used to return the PIT
/// interrupt count whenever the PIT was running: a clock that only advances
/// when Core 0 takes an interrupt stops while Core 0 runs with IF=0, and
/// could not survive the tick going away (`docs/plan/CORES_AND_EVENTS.md`
/// §3.2). The unit stays 10 ms so the 223 callers read the same numbers.
pub fn ticks() -> u64 {
    let freq = TSC_FREQ.load(Ordering::Relaxed);
    let period = freq / 100; // TSC cycles per 10ms tick
    let boot = BOOT_TSC.load(Ordering::Relaxed);
    if period == 0 || boot == 0 { return 0; }
    rdtsc().saturating_sub(boot) / period
}

/// Seconds since boot (approximate)
pub fn uptime_secs() -> u64 {
    ticks() / 100
}

/// Microseconds since boot, from the TSC.
///
/// **`ticks()` is 100 Hz, and that is too coarse for a receive-side RTT.**
/// On this link an RTT of 5 ms measures as "0 ticks" and one of 20 ms as
/// one or two — so any window derived from it is a step function with
/// 10 ms steps. Linux' DRS (`tcp_rcv_space_adjust`) compares an elapsed
/// time against `rcv_rtt_est.rtt_us`, in microseconds; this is that clock.
pub fn uptime_us() -> u64 {
    let freq = TSC_FREQ.load(Ordering::Relaxed);
    let per_us = (freq / 1_000_000).max(1);
    let boot = BOOT_TSC.load(Ordering::Relaxed);
    rdtsc().saturating_sub(boot) / per_us
}

/// TSC-Takte in Nanosekunden. Fuer Messungen, die kleiner sind als eine
/// Mikrosekunde — die Kosten EINES TCP-Segments zum Beispiel.
pub fn tsc_to_ns(cycles: u64) -> u64 {
    let freq = TSC_FREQ.load(Ordering::Relaxed).max(1);
    cycles.saturating_mul(1_000_000_000) / freq
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
    // SAFETY: port I/O read; the caller picks the port (PIT calibration only).
    unsafe {
        core::arch::asm!("in al, dx", out("al") v, in("dx") port,
                         options(nomem, nostack, preserves_flags));
    }
    v
}

#[inline]
unsafe fn cal_outb(port: u16, val: u8) {
    // SAFETY: port I/O write; the caller picks the port (PIT calibration only).
    unsafe {
        core::arch::asm!("out dx, al", in("al") val, in("dx") port,
                         options(nomem, nostack, preserves_flags));
    }
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

// Kept next to the ports we do drive: a register map with holes in it is
// worse than one unused constant (see feedback_verify_reg_addrs).
#[allow(dead_code)]
const PIT_CHANNEL0: u16 = 0x40;
#[allow(dead_code)]
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
#[allow(dead_code)] // the master's twin; the map stays complete
const PIC_OFFSET_SLAVE: u8 = 40;  // IRQ8-15 → vectors 40-47

pub fn init() {
    unsafe {
        // Exception handlers
        // Both go through a stub that first asks whether the fault came from
        // compiled wasm; if it did, it is that module's trap and not the
        // kernel's. Anything else lands on the handler that was here before.
        IDT[0].set_handler(crate::forge_rt::forge_de_stub as *const () as u64);
        IDT[3].set_handler(breakpoint_handler as *const () as u64);
        IDT[6].set_handler(invalid_opcode_handler as *const () as u64);
        IDT[8].set_handler(double_fault_handler as *const () as u64);
        IDT[13].set_handler(gp_fault_handler as *const () as u64);
        IDT[14].set_handler(crate::forge_rt::forge_pf_stub as *const () as u64);

        // Hardware interrupt handlers
        IDT[PIC_OFFSET_MASTER as usize].set_handler(timer_handler as *const () as u64);
        IDT[(PIC_OFFSET_MASTER + 1) as usize].set_handler(keyboard_handler as *const () as u64);
        // Dedicated-microvm-core LAPIC timer (substrate rework A2).
        // Global IDT is shared by all cores; only the dedicated core
        // ever raises this vector (its own LAPIC timer) — a pure EOI.
        IDT[DEDICATED_VM_TIMER_VECTOR as usize]
            .set_handler(dedicated_vm_timer_handler as *const () as u64);
        // Per-core worker idle timer (one per AP) — pure EOI, see
        // `init_worker_timer`. Shared IDT; each worker raises it on its
        // own LAPIC so its idle HLT wakes without a host tick.
        IDT[WORKER_TIMER_VECTOR as usize]
            .set_handler(worker_timer_handler as *const () as u64);
        // Wake IPI for an idle worker (new work in the inbox) — pure EOI.
        IDT[WORKER_WAKE_VECTOR as usize]
            .set_handler(worker_timer_handler as *const () as u64);

        // Cross-vCPU kick IPI (guest SMP) — prompt inter-vCPU IPI delivery.
        IDT[VCPU_KICK_VECTOR as usize]
            .set_handler(vcpu_kick_handler as *const () as u64);
        // Device-IRQ pool (MSI-X → LAPIC vector → fiber wake). See `crate::irq`.
        install_device_isrs();

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

// Reached from `forge_de_stub` when the fault did not come from compiled
// wasm. Named for the assembler, which cannot see through Rust's mangling.
#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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
    // NO busy-TSC fabrication here.
    //
    // This used to add `freq / 100` — one whole tick of cycles — to Core 0
    // on every tick, with the note "Core 0 runs event loop". That is the
    // wall clock itself, so Core 0's usage was pinned at 99-100 % by
    // construction and said nothing about the machine. The shell loop has
    // executed `hlt` when idle for a long time; the accounting never
    // followed. Usage now comes from `record_halt` at the HLT sites, like
    // every other core (`update_core_freq`).
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
    // NO busy-TSC fabrication here.
    //
    // This used to add `freq / 100` — one whole tick of cycles — to Core 0
    // on every tick, with the note "Core 0 runs event loop". That is the
    // wall clock itself, so Core 0's usage was pinned at 99-100 % by
    // construction and said nothing about the machine. The shell loop has
    // executed `hlt` when idle for a long time; the accounting never
    // followed. Usage now comes from `record_halt` at the HLT sites, like
    // every other core (`update_core_freq`).
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

/// The xAPIC base, read from MSR 0x1B if no timer path cached it yet.
/// Physical address is the same on every core.
pub fn apic_base_any() -> u64 {
    let b = APIC_BASE.load(Ordering::Relaxed);
    if b != 0 { return b; }
    let (lo, hi): (u32, u32);
    // SAFETY: MSR 0x1B (APIC base) is always readable in ring 0.
    unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi) };
    ((hi as u64) << 32 | lo as u64) & 0xFFFF_FFFF_F000
}

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
    // (A Stage-1 experiment that woke the parked vCPU 1 kHz here was REVERTED: the
    // guest is tickless (NO_HZ) and only asks for ~356 ticks/s — it does NOT want
    // 1000, so forcing wakes didn't raise its effective HZ and just span a core.
    // The effective HZ tracks load; it is a SYMPTOM, not the cause — as the
    // original diagnosis already found.)
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
    // The periodic VM timer now owns this core's LAPIC timer: the worker
    // one-shot must not be armed over it (`halt_until`), and it wakes the
    // core's idle HLT on its own. `init_worker_timer` takes it back.
    let cid = crate::smp::per_core::current_core_id();
    if cid < 256 {
        VM_TIMER_ON[cid].store(true, Ordering::Relaxed);
    }
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
        // Count mode (masked) BEFORE measuring: the worker timer may have
        // left the LVT in TSC-deadline mode, where the initial-count
        // register is ignored and the measurement would read garbage.
        core::ptr::write_volatile(b.add(0x320) as *mut u32,
            (1 << 16) | DEDICATED_VM_TIMER_VECTOR as u32);
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

// ── Per-core worker timer: one-shot to the next deadline ────────────
//
// Stage 1 of `docs/plan/CORES_AND_EVENTS.md`. A worker used to arm a
// PERIODIC 100 Hz LAPIC timer and wake on every tick to look for work: 100
// wakes a second per idle core, and every sleep under 10 ms needed the
// period shortened (`arm_worker_wake_in`). Now the timer is one-shot —
// armed to the exact TSC deadline the core waits for, or not at all — and
// new work wakes an idle core with an IPI (`WORKER_WAKE_VECTOR`), not a tick.
//
// TSC-deadline mode (CPUID.1:ECX[24]) when the CPU has it: the LAPIC
// compares against the TSC itself, no conversion. Otherwise one-shot count
// mode, converted with the LAPIC rate calibrated against the TSC.
//
// Core 0 keeps its periodic tick until stage 3: its loop still drains PS/2,
// runs TCP timers and animations on it. A core running a guest vCPU keeps the
// 1 kHz periodic VM timer (`arm_dedicated_vm_timer`), which then also wakes
// its idle HLT.
//
// Both vectors are pure EOI: the interrupt returning the core from HLT IS
// the effect.
const WORKER_TIMER_VECTOR: u8 = 50;
/// IPI to an idle worker: new work is in the inbox.
pub const WORKER_WAKE_VECTOR: u8 = 52;
static WORKER_APIC_BASE: AtomicU64 = AtomicU64::new(0);
/// LAPIC counts per 10 ms at divide 16 — for one-shot count mode.
static WORKER_TIMER_INITIAL: AtomicU32 = AtomicU32::new(0);
static HAS_TSC_DEADLINE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
const MSR_TSC_DEADLINE: u32 = 0x6E0;

/// Does this CPU have the LAPIC TSC-deadline mode (CPUID.1:ECX[24])?
pub fn has_tsc_deadline() -> bool {
    let ecx: u32;
    // SAFETY: CPUID leaf 1 exists on every x86_64; rbx is reserved by LLVM.
    unsafe {
        core::arch::asm!("push rbx", "mov eax, 1", "cpuid", "pop rbx",
            out("ecx") ecx, out("eax") _, out("edx") _);
    }
    ecx & (1 << 24) != 0
}
/// The 1 kHz VM timer owns this core's LAPIC timer; leave it alone.
static VM_TIMER_ON: [core::sync::atomic::AtomicBool; 256] =
    [const { core::sync::atomic::AtomicBool::new(false) }; 256];
/// Wake period of `worker_idle_hlt` in TSC cycles; 0 = 10 ms.
static POLL_PERIOD: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

extern "x86-interrupt" fn worker_timer_handler(_frame: InterruptStackFrame) {
    let base = WORKER_APIC_BASE.load(Ordering::Relaxed);
    if base != 0 {
        // SAFETY: LAPIC MMIO, identity-mapped; each core EOIs its own LAPIC.
        unsafe { core::ptr::write_volatile((base + 0xB0) as *mut u32, 0); }
    }
}

/// Cross-vCPU kick IPI (guest SMP). When a guest vCPU sends an inter-processor
/// interrupt to another vCPU (reschedule / call-function / TLB-shootdown), the
/// sender's host core fires this vector at the TARGET vCPU's host core, forcing
/// its VMRUN to #VMEXIT(INTR) so it injects the guest IPI within microseconds
/// instead of waiting up to a host-timer tick (~10 ms). That ~10 ms latency is
/// what made `smp_call_function`'s `csd_lock_wait` spin burn ~50% of guest CPU
/// across all vCPUs (measured via rip_sample). Pure EOI — receipt IS the effect.
pub const VCPU_KICK_VECTOR: u8 = 51;

extern "x86-interrupt" fn vcpu_kick_handler(_frame: InterruptStackFrame) {
    let base = WORKER_APIC_BASE.load(Ordering::Relaxed);
    if base != 0 {
        // SAFETY: LAPIC MMIO, identity-mapped; each core EOIs its own LAPIC.
        unsafe { core::ptr::write_volatile((base + 0xB0) as *mut u32, 0); }
    }
}

/// Set up the calling worker's LAPIC timer: one-shot (or TSC-deadline)
/// mode on `WORKER_TIMER_VECTOR`, nothing armed. Each AP calls it once at
/// boot; a core that ran a guest calls it again after the VM exits, to take
/// the timer back from the 1 kHz periodic VM timer.
pub fn init_worker_timer() {
    let (lo, hi): (u32, u32);
    // SAFETY: MSR 0x1B (APIC base) is always readable on x86_64 ring 0.
    unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }
    let base = ((hi as u64) << 32 | lo as u64) & 0xFFFF_FFFF_F000;
    WORKER_APIC_BASE.store(base, Ordering::Relaxed);
    // Also publish the (core-invariant) xAPIC base for the device-IRQ ISR EOI
    // path, in case Core 0 runs on the PIT and never set APIC_BASE itself.
    let _ = APIC_BASE.compare_exchange(0, base, Ordering::Relaxed, Ordering::Relaxed);

    let deadline = has_tsc_deadline();
    HAS_TSC_DEADLINE.store(deadline, Ordering::Relaxed);

    // SAFETY: LAPIC MMIO regs, identity-mapped; this core's own LAPIC.
    unsafe {
        let b = base as *mut u8;
        let svr = core::ptr::read_volatile(b.add(0xF0) as *const u32);
        core::ptr::write_volatile(b.add(0xF0) as *mut u32, svr | (1 << 8) | 0xFF);
        // Divide = 16.
        core::ptr::write_volatile(b.add(0x3E0) as *mut u32, 0x03);
        if WORKER_TIMER_INITIAL.load(Ordering::Relaxed) == 0 {
            // Calibrate counts/10ms against the TSC, once — the LAPIC timer
            // clock is the same on every core. Periodic mode for the
            // measurement only; the counter just has to run down.
            core::ptr::write_volatile(b.add(0x320) as *mut u32, (1 << 16) | WORKER_TIMER_VECTOR as u32);
            let freq = TSC_FREQ.load(Ordering::Relaxed);
            core::ptr::write_volatile(b.add(0x380) as *mut u32, 0xFFFF_FFFF);
            let start = rdtsc();
            let tsc_10ms = if freq > 0 { freq / 100 } else { 20_000_000 };
            while rdtsc() - start < tsc_10ms { core::hint::spin_loop(); }
            let n = 0xFFFF_FFFFu32
                .wrapping_sub(core::ptr::read_volatile(b.add(0x390) as *const u32))
                .max(1);
            WORKER_TIMER_INITIAL.store(n, Ordering::Relaxed);
        }
        // Stop whatever ran before (a periodic VM timer), then set the mode:
        // TSC-deadline = LVT bits 18:17 = 10b, one-shot = 00b.
        core::ptr::write_volatile(b.add(0x380) as *mut u32, 0);
        let mode = if deadline { 0b10 << 17 } else { 0 };
        core::ptr::write_volatile(b.add(0x320) as *mut u32, mode | WORKER_TIMER_VECTOR as u32);
    }
    if deadline {
        // SAFETY: TSC-deadline mode is supported (checked above); 0 disarms.
        unsafe { wrmsr_deadline(0) };
    }
    let cid = crate::smp::per_core::current_core_id();
    if cid < 256 {
        VM_TIMER_ON[cid].store(false, Ordering::Relaxed);
    }
}

/// SAFETY: only with TSC-deadline mode supported and selected in the LVT.
unsafe fn wrmsr_deadline(tsc: u64) {
    // SAFETY: the SDM asks for the LVT write to be ordered before the MSR
    // write; Linux fences the same way (`weak_wrmsr_fence`).
    unsafe {
        core::arch::asm!("mfence; lfence", options(nostack, preserves_flags));
        core::arch::asm!("wrmsr", in("ecx") MSR_TSC_DEADLINE,
            in("eax") tsc as u32, in("edx") (tsc >> 32) as u32,
            options(nostack, preserves_flags));
    }
}

/// Arm this core's worker timer to fire at TSC `deadline`. A deadline in
/// the past fires at once.
fn arm_at(deadline: u64) {
    if HAS_TSC_DEADLINE.load(Ordering::Relaxed) {
        // SAFETY: mode selected by `init_worker_timer`; never 0 (= disarm).
        unsafe { wrmsr_deadline(deadline.max(1)) };
        return;
    }
    let base = WORKER_APIC_BASE.load(Ordering::Relaxed);
    let per_10ms = WORKER_TIMER_INITIAL.load(Ordering::Relaxed) as u64;
    let tsc_10ms = (TSC_FREQ.load(Ordering::Relaxed) / 100).max(1);
    if base == 0 || per_10ms == 0 { return; }
    let delta = deadline.saturating_sub(rdtsc());
    let count = ((delta as u128 * per_10ms as u128) / tsc_10ms as u128)
        .clamp(1, u32::MAX as u128) as u32;
    // SAFETY: this core's own LAPIC initial-count register; a write starts
    // the one-shot countdown.
    unsafe { core::ptr::write_volatile((base + 0x380) as *mut u32, count) };
}

fn disarm() {
    if HAS_TSC_DEADLINE.load(Ordering::Relaxed) {
        // SAFETY: mode selected by `init_worker_timer`; 0 disarms.
        unsafe { wrmsr_deadline(0) };
        return;
    }
    let base = WORKER_APIC_BASE.load(Ordering::Relaxed);
    if base != 0 {
        // SAFETY: this core's own LAPIC initial-count register; 0 stops it.
        unsafe { core::ptr::write_volatile((base + 0x380) as *mut u32, 0) };
    }
}

/// THE idle primitive of a worker: halt until an interrupt, and — if
/// `deadline` is given — no later than that TSC value. IF is restored as
/// found. `cause` is the wake-attribution slot for `cores`.
///
/// Arming and halting happen with IF=0 and the halt is entered as
/// `sti; hlt`: an interrupt landing between the arm and the halt stays
/// pending and ends the halt, instead of being consumed before it and
/// leaving the core asleep with nothing armed.
///
/// On Core 0 (periodic tick) and on a core running a guest (1 kHz VM timer)
/// the timer is not touched; the halt ends at the next tick there.
pub fn halt_until(deadline: Option<u64>, cause: usize) {
    let cid = crate::smp::per_core::current_core_id();
    let own_timer = cid != 0 && cid < 256 && !VM_TIMER_ON[cid].load(Ordering::Relaxed);
    let rflags: u64;
    // SAFETY: save RFLAGS and clear IF; restored below exactly as found.
    unsafe { core::arch::asm!("pushfq; pop {}; cli", out(reg) rflags) };
    if let Some(d) = deadline {
        if rdtsc() >= d {
            if rflags & (1 << 9) != 0 {
                // SAFETY: IF was set on entry.
                unsafe { core::arch::asm!("sti") };
            }
            return;
        }
        if own_timer { arm_at(d); }
    }
    let t0 = rdtsc();
    crate::smp::per_core::halt_begin(cid, t0);
    // SAFETY: sti-shadow arms the HLT before any pending IRQ is taken.
    unsafe { core::arch::asm!("sti; hlt; cli") };
    crate::smp::per_core::record_halt(cid, rdtsc().saturating_sub(t0));
    crate::smp::per_core::record_wake(cid, cause);
    // Woken by something else: the one-shot is still pending — drop it.
    if own_timer && deadline.is_some() { disarm(); }
    if rflags & (1 << 9) != 0 {
        // SAFETY: IF was set on entry.
        unsafe { core::arch::asm!("sti") };
    }
}

/// How often `worker_idle_hlt` wakes on the calling worker: `hz` per second.
/// A native network loop (download) raises it to poll its NIC ring without a
/// device IRQ; pass 100 to go back to 10 ms. Goes away with NAPI (stage 2).
pub fn set_worker_poll_hz(hz: u32) {
    let cid = crate::smp::per_core::current_core_id();
    if cid == 0 || cid >= 256 { return; }
    let freq = TSC_FREQ.load(Ordering::Relaxed);
    let period = if hz <= 100 { 0 } else { freq / hz as u64 };
    POLL_PERIOD[cid].store(period, Ordering::Relaxed);
}

/// Idle step of a worker-core wait loop that polls (the native network
/// receive loops: `tcp_recv_poll`, `recv_blocking`): halt for one poll
/// period — 10 ms, or what `set_worker_poll_hz` asked for — or until an
/// interrupt. Spin on Core 0, which has its own idle path.
pub fn worker_idle_hlt() {
    let cid = crate::smp::per_core::current_core_id();
    if cid == 0 || cid >= 256 { core::hint::spin_loop(); return; }
    let freq = TSC_FREQ.load(Ordering::Relaxed);
    let p = match POLL_PERIOD[cid].load(Ordering::Relaxed) {
        0 => freq / 100,
        p => p,
    };
    halt_until(Some(rdtsc() + p), crate::smp::per_core::WAKE_HLT_FALLBACK);
}

/// Run `f` with this core's interrupts masked, restoring the previous IF.
///
/// For a spin lock that an ISR on the SAME core may also take: held with
/// IF=1, the timer IRQ lands inside the critical section and spins on the
/// lock its own core holds — the core is gone. Cheap (pushfq/cli/popfq), so
/// keep `f` short; it cannot be interrupted.
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let rflags: u64;
    // SAFETY: save RFLAGS and clear IF; restored below exactly as found.
    unsafe { core::arch::asm!("pushfq; pop {}; cli", out(reg) rflags) };
    let r = f();
    if rflags & (1 << 9) != 0 {
        // SAFETY: IF was set on entry — set it again.
        unsafe { core::arch::asm!("sti") };
    }
    r
}

// ── Device interrupts (MSI-X → LAPIC vector → fiber wake) ───────────
//
// Real-hardware device IRQs route via MSI-X to a vector in this pool (the
// PIC is masked, no IOAPIC). Each vector has a tiny ISR that bumps a
// per-vector fired-count (`irq::note_fired`) + EOIs the LAPIC; a driver
// fiber parks on it via `irq::wait`. See `crate::irq`. Vectors 0x70..0x80
// sit above the timer vectors (32/48/49/50) and the masked PIC range.
pub const DEVICE_IRQ_VEC_BASE: u8 = 0x70;
pub const DEVICE_IRQ_VEC_COUNT: usize = 16;

/// Shared body of every device-IRQ ISR: note the fire (wakes the parked
/// driver fiber via its count) + EOI this core's LAPIC. Minimal by design —
/// no fiber-state touch in interrupt context (the scheduler owns that; the
/// atomic bump is race-free across cores).
#[inline]
fn device_irq_common(vector: u8) {
    crate::irq::isr(vector);
    // LAPIC EOI: write 0 to offset 0xB0. The xAPIC base is the same physical
    // address on every core; this write hits the LAPIC of the core that took
    // the interrupt (the MSI-X destination = the driver fiber's core).
    let base = APIC_BASE.load(Ordering::Relaxed);
    if base != 0 {
        // SAFETY: LAPIC MMIO EOI register, identity-mapped.
        unsafe { core::ptr::write_volatile((base + 0xB0) as *mut u32, 0); }
    }
}

// Generate one `extern "x86-interrupt"` stub per pool vector (each hardcodes
// its vector, since the CPU passes none) + an installer that wires them into
// the IDT. Macro keeps the 16 stubs + 16 IDT writes in lockstep.
macro_rules! device_isrs {
    ($($name:ident = $idx:expr),+ $(,)?) => {
        $(
            extern "x86-interrupt" fn $name(_f: InterruptStackFrame) {
                device_irq_common(DEVICE_IRQ_VEC_BASE + $idx);
            }
        )+
        /// Install the device-IRQ ISRs into the (global) IDT. Called from
        /// `init()` on the BSP before APs boot, so every core that loads this
        /// IDT sees them.
        unsafe fn install_device_isrs() {
            // SAFETY: writing IDT entries before `sti`/AP-bringup; vectors are
            // in the dedicated device pool, distinct from timer/exception ones.
            unsafe {
                $( IDT[(DEVICE_IRQ_VEC_BASE as usize) + $idx]
                    .set_handler($name as *const () as u64); )+
            }
        }
    };
}
device_isrs!(
    di0 = 0, di1 = 1, di2 = 2, di3 = 3, di4 = 4, di5 = 5, di6 = 6, di7 = 7,
    di8 = 8, di9 = 9, di10 = 10, di11 = 11, di12 = 12, di13 = 13, di14 = 14,
    di15 = 15,
);

/// Hardware APIC ID of the calling core (xAPIC ID register, bits 31:24).
/// Used to set an MSI-X message's destination so the IRQ wakes this core.
/// Reads MSR 0x1B directly if `APIC_BASE` isn't cached yet.
pub fn current_apic_id() -> u32 {
    let base = {
        let b = APIC_BASE.load(Ordering::Relaxed);
        if b != 0 {
            b
        } else {
            let (lo, hi): (u32, u32);
            // SAFETY: MSR 0x1B (APIC base) is always readable in ring 0.
            unsafe { core::arch::asm!("rdmsr", in("ecx") 0x1Bu32, out("eax") lo, out("edx") hi); }
            ((hi as u64) << 32 | lo as u64) & 0xFFFF_FFFF_F000
        }
    };
    // SAFETY: LAPIC ID register at MMIO offset 0x20, identity-mapped.
    unsafe { core::ptr::read_volatile((base + 0x20) as *const u32) >> 24 }
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
