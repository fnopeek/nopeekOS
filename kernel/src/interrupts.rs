//! Interrupt Descriptor Table, exception handlers, timer and device IRQs.

use core::fmt::Write as _;
use crate::hw::{msr, Mmio, Port};
use crate::kprintln;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Timer interrupts taken on Core 0. Not a clock: only used to tell
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
/// The clock is the TSC on every machine. An interrupt count would stop while
/// Core 0 runs with IF=0 and could not survive the tick going away
/// (`docs/plan/CORES_AND_EVENTS.md` §3.2). The unit is 10 ms.
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
/// `ticks()` is 100 Hz, too coarse for a receive-side RTT. Linux' DRS
/// (`tcp_rcv_space_adjust`) compares an elapsed time against
/// `rcv_rtt_est.rtt_us`, in microseconds; this is that clock.
pub fn uptime_us() -> u64 {
    let freq = TSC_FREQ.load(Ordering::Relaxed);
    let per_us = (freq / 1_000_000).max(1);
    let boot = BOOT_TSC.load(Ordering::Relaxed);
    rdtsc().saturating_sub(boot) / per_us
}

/// TSC cycles converted to nanoseconds, for measurements below a microsecond.
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
            crate::kdebug!("[npk] TSC: {} MHz (CPUID 0x15: crystal={}Hz ratio={}/{})",
                freq / 1_000_000, ecx, ebx, eax);
            return;
        }
    }
    // CPUID 0x15 absent (always on AMD, it is an Intel-only leaf). Calibrate
    // against PIT channel 2 (vendor-independent; Linux's quick_pit_calibrate
    // approach). Every TSC-derived time depends on this, including delay_ms,
    // run_slice's SLICE_MS and the guest's `tsc_early_khz=` cmdline.
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

    let t0 = rdtsc();
    let mut spins: u64 = 0;
    while SYSTEM_CONTROL_B.inb() & 0x20 == 0 {
        spins += 1;
        if spins > 2_000_000_000 {
            SYSTEM_CONTROL_B.outb(p61 & 0xFC); // gate off, restore
            return None; // PIT not advancing
        }
    }
    let t1 = rdtsc();
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
// worse than one unused constant.
// SAFETY (all port constants in this file): the legacy PIT, system control
// port B (speaker gate, PIT channel 2 output) and the 8259 PIC pair; this
// module is their only driver.
#[allow(dead_code)]
const PIT_CHANNEL0: Port = unsafe { Port::new(0x40) };
const PIT_CHANNEL2: Port = unsafe { Port::new(0x42) };
const PIT_COMMAND: Port = unsafe { Port::new(0x43) };
const SYSTEM_CONTROL_B: Port = unsafe { Port::new(0x61) };
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

// SAFETY: see the PIT port constants.
const PIC1_CMD: Port = unsafe { Port::new(0x20) };
const PIC1_DATA: Port = unsafe { Port::new(0x21) };
const PIC2_CMD: Port = unsafe { Port::new(0xA0) };
const PIC2_DATA: Port = unsafe { Port::new(0xA1) };
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
        IDT[2].set_handler(nmi_handler as *const () as u64);
        IDT[3].set_handler(breakpoint_handler as *const () as u64);
        IDT[6].set_handler(invalid_opcode_handler as *const () as u64);
        IDT[8].set_handler(double_fault_handler as *const () as u64);
        IDT[13].set_handler(gp_fault_handler as *const () as u64);
        IDT[14].set_handler(crate::forge_rt::forge_pf_stub as *const () as u64);
        IDT[10].set_handler(invalid_tss_handler as *const () as u64);
        IDT[11].set_handler(segment_not_present_handler as *const () as u64);
        IDT[12].set_handler(stack_segment_handler as *const () as u64);
        IDT[16].set_handler(x87_fp_handler as *const () as u64);
        IDT[17].set_handler(alignment_check_handler as *const () as u64);
        IDT[18].set_handler(machine_check_handler as *const () as u64);
        IDT[19].set_handler(simd_fp_handler as *const () as u64);
        // Every SVR write names this vector. Without a gate a spurious
        // interrupt would raise #NP and end as a double fault.
        IDT[SPURIOUS_VECTOR as usize].set_handler(spurious_handler as *const () as u64);

        // Hardware interrupt handlers
        IDT[PIC_OFFSET_MASTER as usize].set_handler(timer_handler as *const () as u64);
        IDT[(PIC_OFFSET_MASTER + 1) as usize].set_handler(keyboard_handler as *const () as u64);
        // Per-core worker idle timer (one per AP) — pure EOI, see
        // `init_worker_timer`. Shared IDT; each worker raises it on its
        // own LAPIC so its idle HLT wakes without a host tick.
        IDT[WORKER_TIMER_VECTOR as usize]
            .set_handler(worker_timer_handler as *const () as u64);
        // Wake IPI for an idle worker (new work in the inbox) — pure EOI.
        IDT[WORKER_WAKE_VECTOR as usize]
            .set_handler(worker_wake_handler as *const () as u64);

        // Cross-vCPU kick IPI (guest SMP) — prompt inter-vCPU IPI delivery.
        IDT[VCPU_KICK_VECTOR as usize]
            .set_handler(vcpu_kick_handler as *const () as u64);
        // Input by interrupt: the i8042 through the I/O APIC,
        // the xHCI controllers through MSI-X — both to Core 0.
        IDT[PS2_VECTOR as usize].set_handler(ps2_irq_handler as *const () as u64);
        IDT[XHCI_VECTOR as usize].set_handler(xhci_irq_handler as *const () as u64);
        // Device-IRQ pool (MSI-X → LAPIC vector → fiber wake). See `crate::irq`.
        install_device_isrs();

        // Load IDT
        let idt_reg = IdtRegister {
            limit: (IDT_SIZE * core::mem::size_of::<IdtEntry>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        // SAFETY: IDT is fully initialized above
        core::arch::asm!("lidt [{}]", in(reg) &idt_reg);

        // Do not re-initialize the legacy 8259 PIC. Writing ICW1 (0x11) to
        // its command port (0x20/0xA0) traps via SMI on some UEFI firmware
        // (HP/Insyde virtualize the legacy PIC in SMM under APIC mode) and
        // resets the machine after ExitBootServices. UEFI hands over in APIC
        // mode, so the PIC is only masked via its data ports and ticks come
        // from the Local APIC timer (`init_apic_timer`). A masked, un-remapped
        // PIC delivers no IRQs, so the default 0x08-0x0F vectors never
        // collide with CPU exceptions.
        PIC1_DATA.outb(0xFF);
        PIC2_DATA.outb(0xFF);

        // SAFETY: IDT loaded, PIC fully masked, handlers set.
        core::arch::asm!("sti");
    }
}

fn pic_eoi(irq: u8) {
    if irq >= 8 { PIC2_CMD.outb(PIC_EOI); }
    PIC1_CMD.outb(PIC_EOI);
}

// === Exception Handlers ===

// Reached from `forge_de_stub` when the fault did not come from compiled
// wasm. Named for the assembler, which cannot see through Rust's mangling.
#[unsafe(no_mangle)]
extern "x86-interrupt" fn divide_error_handler(frame: InterruptStackFrame) {
    crate::fatal::die(format_args!("DIVIDE ERROR (INT 0) !!!\n[npk] RIP: {:#018x}\n[npk] RSP: {:#018x}",
        frame.instruction_pointer, frame.stack_pointer));
}

/// NMIs other than the panic stop. They reach the host when a guest is
/// interrupted (SVM intercepts NMI) or from firmware. Counted, not printed —
/// an NMI may land while the console lock is held.
pub static NMI_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A panicking core stops the others with an NMI (`fatal::begin`).
extern "x86-interrupt" fn nmi_handler(_frame: InterruptStackFrame) {
    if crate::fatal::is_panicking() {
        crate::fatal::halt();
    }
    NMI_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    kprintln!();
    kprintln!("[npk] BREAKPOINT (INT 3) at {:#018x}", frame.instruction_pointer);
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    crate::fatal::die(format_args!("INVALID OPCODE (INT 6) !!!\n[npk] RIP: {:#018x}\n[npk] RSP: {:#018x}",
        frame.instruction_pointer, frame.stack_pointer));
}

/// Run the double-fault handler on the IST stack from `tss::init_core`.
/// Call once the boot core's TSS is in; every core that runs afterwards
/// must have installed its own (the IDT is shared).
pub fn use_double_fault_stack() {
    // SAFETY: a single byte of the static IDT, written on the boot core
    // before the APs start; the CPU reads it only when delivering #DF.
    unsafe {
        core::ptr::addr_of_mut!(IDT[8].ist).write(crate::tss::DOUBLE_FAULT_IST);
    }
}

/// LAPIC spurious-interrupt vector, as written to every core's SVR.
pub const SPURIOUS_VECTOR: u8 = 0xFF;

/// A spurious interrupt is not in service: no EOI (SDM 11.9).
extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {}

/// Fatal exceptions the kernel never raises on purpose: report and halt,
/// rather than fault on a missing gate and surface as a double fault.
macro_rules! fatal_exception {
    ($name:ident, $vec:literal, $what:literal) => {
        extern "x86-interrupt" fn $name(frame: InterruptStackFrame) {
            crate::fatal::die(format_args!(concat!($what, " (INT ", $vec, ") !!!\n[npk] RIP: {:#018x}\n[npk] RSP: {:#018x}"),
                frame.instruction_pointer, frame.stack_pointer));
        }
    };
    ($name:ident, $vec:literal, $what:literal, error_code) => {
        extern "x86-interrupt" fn $name(frame: InterruptStackFrame, error_code: u64) {
            crate::fatal::die(format_args!(concat!($what, " (INT ", $vec, ") !!!\n[npk] Error code: {:#x}\n[npk] RIP: {:#018x}\n[npk] RSP: {:#018x}"),
                error_code, frame.instruction_pointer, frame.stack_pointer));
        }
    };
}

fatal_exception!(invalid_tss_handler, 10, "INVALID TSS", error_code);
fatal_exception!(segment_not_present_handler, 11, "SEGMENT NOT PRESENT", error_code);
fatal_exception!(stack_segment_handler, 12, "STACK-SEGMENT FAULT", error_code);
fatal_exception!(x87_fp_handler, 16, "x87 FLOATING-POINT ERROR");
fatal_exception!(alignment_check_handler, 17, "ALIGNMENT CHECK", error_code);
fatal_exception!(simd_fp_handler, 19, "SIMD FLOATING-POINT EXCEPTION");

extern "x86-interrupt" fn machine_check_handler(frame: InterruptStackFrame) -> ! {
    crate::fatal::die(format_args!("MACHINE CHECK (INT 18) !!!\n[npk] RIP: {:#018x}", frame.instruction_pointer));
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, error_code: u64) -> ! {
    let Some(mut w) = crate::fatal::begin() else { crate::fatal::halt() };
    let cr2: u64;
    // SAFETY: reads CR2, which holds the address of the last page fault.
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags)) };
    let _ = writeln!(w, "DOUBLE FAULT (INT 8) !!!");
    if crate::mm::stack::is_guard(cr2) {
        let _ = writeln!(w, "[npk] kernel stack overflow: guard page {:#018x} hit", cr2);
    }
    let _ = writeln!(w, "[npk] Error code: {:#x}\n[npk] RIP: {:#018x}\n[npk] RSP: {:#018x}",
        error_code, frame.instruction_pointer, frame.stack_pointer);
    crate::fatal::halt()
}

extern "x86-interrupt" fn gp_fault_handler(frame: InterruptStackFrame, error_code: u64) {
    crate::fatal::die(format_args!("GENERAL PROTECTION FAULT (INT 13) !!!\n[npk] Error code: {:#x}\n[npk] RIP: {:#018x}\n[npk] RSP: {:#018x}",
        error_code, frame.instruction_pointer, frame.stack_pointer));
}

#[unsafe(no_mangle)]
extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, error_code: u64) {
    let cr2: u64;
    // SAFETY: Reading CR2 is side-effect-free
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2); }

    let Some(mut w) = crate::fatal::begin() else { crate::fatal::halt() };
    let _ = writeln!(w, "PAGE FAULT (INT 14) !!!\n[npk] Faulting address: {:#018x}\n[npk] Error code: {:#x}\n[npk] RIP: {:#018x}\n[npk] RSP: {:#018x}",
        cr2, error_code, frame.instruction_pointer, frame.stack_pointer);

    // Best-effort backtrace: scan the stack for words that look like return
    // addresses into kernel code (link base 0x1000_0000 .. ~+8 MB). The PIE
    // kernel runs at its link base, so these map directly via
    // `addr2line -e target/.../nopeekos-kernel <addr>`. Only scanned when RSP
    // is inside the identity-mapped range so the scan itself can't fault.
    let rsp = frame.stack_pointer;
    if rsp >= 0x10_0000 && rsp < 0x10_0000_0000 {
        let _ = writeln!(w, "[npk] stack trace (return addrs in kernel code):");
        let mut p = rsp;
        let mut printed = 0;
        let mut scanned = 0;
        while scanned < 1024 && printed < 24 {
            // SAFETY: rsp is within the identity-mapped range (checked above),
            // so this read cannot page-fault (re-entry would triple-fault).
            let v = unsafe { core::ptr::read_volatile(p as *const u64) };
            if v >= 0x1000_0000 && v < 0x1080_0000 {
                let _ = writeln!(w, "[npk]   {:#018x}", v);
                printed += 1;
            }
            p += 8;
            scanned += 1;
        }
    }
    crate::fatal::halt()
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
    // No busy time is accounted here: Core 0's usage comes from
    // `record_halt` at the HLT sites, like every other core.
    // Update BSP frequency once per second (for top display)
    if tick % 100 == 0 {
        crate::smp::per_core::update_core_freq(0);
    }
    pic_eoi(0);
}

extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    // Wake attribution: IRQ1 fires on the BSP (Core 0).
    crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_KEYBOARD);
    crate::keyboard::irq_handler();
    pic_eoi(1);
}

/// APIC timer handler, for hardware without a PIT (UEFI-only machines).
/// Same function as PIT timer: tick counter + USB event drain.
extern "x86-interrupt" fn apic_timer_handler(_frame: InterruptStackFrame) {
    let tick = TICKS.fetch_add(1, Ordering::Relaxed);
    // Wake attribution: the APIC timer is armed only on the BSP (Core 0).
    crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_TIMER);
    crate::xhci::poll_events_irq();
    crate::keyboard::poll_ps2_irq();
    // No busy time is accounted here: Core 0's usage comes from
    // `record_halt` at the HLT sites, like every other core.
    // Update BSP frequency once per second (for top display)
    if tick % 100 == 0 {
        crate::smp::per_core::update_core_freq(0);
    }
    if let Some(lapic) = lapic_cached() {
        lapic.w32(LAPIC_EOI, 0);
    }
}

/// APIC base address (from MSR 0x1B).
static APIC_BASE: AtomicU64 = AtomicU64::new(0);
const APIC_TIMER_VECTOR: u8 = 48;

/// Get cached APIC base (for per-core identification via LAPIC ID register).
pub fn apic_base() -> u64 { APIC_BASE.load(Ordering::Relaxed) }

const IA32_APIC_BASE: u32 = 0x1B;
const LAPIC_LEN: u64 = 0x1000;
// xAPIC register offsets.
pub const LAPIC_ID: u32 = 0x20;
const LAPIC_EOI: u32 = 0xB0;
pub const LAPIC_SVR: u32 = 0xF0;
pub const LAPIC_ICR_LO: u32 = 0x300;
pub const LAPIC_ICR_HI: u32 = 0x310;
const LAPIC_LVT_TIMER: u32 = 0x320;
const LAPIC_TIMER_INIT: u32 = 0x380;
const LAPIC_TIMER_CUR: u32 = 0x390;
const LAPIC_TIMER_DIV: u32 = 0x3E0;

/// The xAPIC registers named by IA32_APIC_BASE. Every core reaches its own
/// LAPIC through the same physical page.
pub fn lapic_from_msr() -> Mmio {
    // SAFETY: IA32_APIC_BASE is architectural on x86_64; reading it has no
    // side effect.
    let base = unsafe { msr::read(IA32_APIC_BASE) } & 0xFFFF_FFFF_F000;
    // SAFETY: the page IA32_APIC_BASE names is the xAPIC register window. It
    // lies inside the 64 GB identity map, which `smp::init` and
    // `init_apic_timer` remap uncached, and it never moves.
    unsafe { Mmio::from_mapped(base, LAPIC_LEN) }
}

/// The xAPIC registers at the base cached in `APIC_BASE`, once set.
fn lapic_cached() -> Option<Mmio> {
    let b = APIC_BASE.load(Ordering::Relaxed);
    // SAFETY: `APIC_BASE` holds only bases from `lapic_from_msr`.
    (b != 0).then(|| unsafe { Mmio::from_mapped(b, LAPIC_LEN) })
}

/// The xAPIC registers, from the cached base or else from the MSR.
pub fn lapic() -> Mmio {
    lapic_cached().unwrap_or_else(lapic_from_msr)
}

/// The xAPIC registers at the base cached in `WORKER_APIC_BASE`, once set.
fn worker_lapic() -> Option<Mmio> {
    let b = WORKER_APIC_BASE.load(Ordering::Relaxed);
    // SAFETY: `WORKER_APIC_BASE` holds only bases from `lapic_from_msr`.
    (b != 0).then(|| unsafe { Mmio::from_mapped(b, LAPIC_LEN) })
}

// ── Per-core worker timer: one-shot to the next deadline ────────────
//
// See `docs/plan/CORES_AND_EVENTS.md`. The timer is one-shot, armed to the
// exact TSC deadline the core waits for or not at all, so an idle core does
// not wake periodically. New work wakes an idle core with an IPI
// (`WORKER_WAKE_VECTOR`).
//
// TSC-deadline mode (CPUID.1:ECX[24]) when the CPU has it: the LAPIC
// compares against the TSC itself, no conversion. Otherwise one-shot count
// mode, converted with the LAPIC rate calibrated against the TSC.
//
// A core running a guest vCPU uses the same one-shot: before every VM entry
// it is armed to the guest's next timer deadline (`arm_vcpu_timer`) — KVM's
// hrtimer on the vCPU's core — so the guest's clock events land on time and
// not on a host tick.
//
// Both vectors are pure EOI: the interrupt returning the core from HLT is
// the effect.
const WORKER_TIMER_VECTOR: u8 = 50;
/// IPI to an idle worker: new work is in the inbox.
pub const WORKER_WAKE_VECTOR: u8 = 52;
static WORKER_APIC_BASE: AtomicU64 = AtomicU64::new(0);
/// LAPIC counts per 10 ms at divide 16 — for one-shot count mode.
static WORKER_TIMER_INITIAL: AtomicU32 = AtomicU32::new(0);
/// Does this CPU have the LAPIC TSC-deadline mode (CPUID.1:ECX[24])?
static HAS_TSC_DEADLINE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
const MSR_TSC_DEADLINE: u32 = 0x6E0;

/// Deep idle via an ACPI SystemIO C-state port; 0 = off (plain `hlt`, C1).
/// Set by `set_deep_idle` (`power cstate`).
static DEEP_IDLE_PORT: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
/// A halt shorter than this stays in C1: the exit latency of a deep state
/// would cost more than it saves (Linux's menu governor makes the same cut
/// against the state's target residency).
static DEEP_IDLE_MIN_TSC: AtomicU64 = AtomicU64::new(u64::MAX);
/// Deep entries per core (how often the deep path was actually taken).
pub static DEEP_IDLE_COUNT: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

/// Turn deep idle on (`port` != 0) or off (0). Refuses anything outside the
/// CPU's own C-state trap range and a CPU whose LAPIC timer would stop in a
/// deep state (no ARAT) — there a core sleeping on its deadline timer would
/// never wake.
pub fn set_deep_idle(port: u16, min_us: u64) -> Result<(), &'static str> {
    if port == 0 {
        DEEP_IDLE_PORT.store(0, Ordering::Relaxed);
        return Ok(());
    }
    if !has_arat() {
        return Err("no ARAT: the LAPIC timer stops in deep C-states");
    }
    let base = crate::smp::per_core::amd_cstate_base().ok_or("no AMD C-state base address")?;
    if port < base || port >= base.saturating_add(8) {
        return Err("port outside CStateBaseAddr..+8");
    }
    let tsc_per_us = (tsc_freq() / 1_000_000).max(1);
    DEEP_IDLE_MIN_TSC.store(min_us.saturating_mul(tsc_per_us), Ordering::Relaxed);
    DEEP_IDLE_PORT.store(port, Ordering::Relaxed);
    Ok(())
}

pub fn deep_idle_port() -> u16 { DEEP_IDLE_PORT.load(Ordering::Relaxed) }

/// CPUID.06H:EAX[2] — Always Running APIC Timer (keeps counting in deep
/// C-states).
pub fn has_arat() -> bool {
    let eax: u32;
    // SAFETY: CPUID leaf 6 exists on every CPU this kernel runs on; rbx is
    // reserved by LLVM.
    unsafe {
        core::arch::asm!("push rbx", "mov eax, 6", "xor ecx, ecx", "cpuid", "pop rbx",
            out("eax") eax, out("ecx") _, out("edx") _);
    }
    eax & (1 << 2) != 0
}

pub fn has_tsc_deadline() -> bool {
    let ecx: u32;
    // SAFETY: CPUID leaf 1 exists on every x86_64; rbx is reserved by LLVM.
    unsafe {
        core::arch::asm!("push rbx", "mov eax, 1", "cpuid", "pop rbx",
            out("ecx") ecx, out("eax") _, out("edx") _);
    }
    ecx & (1 << 24) != 0
}
/// Core 0 runs on the one-shot timer too (`make_core0_tickless`).
static CORE0_TICKLESS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Give Core 0 the worker's one-shot timer and stop its periodic 100 Hz
/// tick (the LVT is rewritten from periodic vector 48 to one-shot vector
/// 50). From here Core 0 wakes only when something is due or something
/// happens. The tick's work is done elsewhere: the wall clock is the TSC,
/// input comes by interrupt or is drained by the shell when no interrupt
/// exists, and the frequency statistics run in `core0_loop`.
pub fn make_core0_tickless() {
    init_worker_timer();
    CORE0_TICKLESS.store(true, Ordering::Release);
    crate::kdebug!("[npk] core 0: periodic tick off — deadline timer from here ({})",
        if HAS_TSC_DEADLINE.load(Ordering::Relaxed) { "TSC-deadline" } else { "one-shot" });
}

/// TSC value the core's one-shot is armed to; 0 = not armed (fired or
/// disarmed). Lets a vCPU skip re-arming the same deadline on every entry.
static ARMED: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];
/// Wake period of `worker_idle_hlt` in TSC cycles; 0 = 10 ms.
static POLL_PERIOD: [AtomicU64; 256] = [const { AtomicU64::new(0) }; 256];

/// Wake IPI for an idle worker — pure EOI; the interrupt ending the HLT is
/// the effect.
extern "x86-interrupt" fn worker_wake_handler(_frame: InterruptStackFrame) {
    if CORE0_TICKLESS.load(Ordering::Relaxed) && crate::smp::per_core::current_core_id() == 0 {
        crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_TIMER);
    }
    if let Some(lapic) = worker_lapic() {
        lapic.w32(LAPIC_EOI, 0); // each core EOIs its own LAPIC
    }
}

extern "x86-interrupt" fn worker_timer_handler(_frame: InterruptStackFrame) {
    let cid = crate::smp::per_core::current_core_id();
    if cid < 256 {
        ARMED[cid].store(0, Ordering::Relaxed);
    }
    crate::smp::per_core::note_timer_fire(cid);
    // Core 0 attributes its own wakes (halt_until skips it): a timer or a
    // wake IPI on this vector counts as `timer` in `cores`.
    if CORE0_TICKLESS.load(Ordering::Relaxed) && crate::smp::per_core::current_core_id() == 0 {
        crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_TIMER);
    }
    if let Some(lapic) = worker_lapic() {
        lapic.w32(LAPIC_EOI, 0); // each core EOIs its own LAPIC
    }
}

/// Cross-vCPU kick IPI (guest SMP). When a guest vCPU sends an inter-processor
/// interrupt to another vCPU (reschedule / call-function / TLB-shootdown), the
/// sender's host core fires this vector at the target vCPU's host core, forcing
/// its VMRUN to #VMEXIT(INTR) so it injects the guest IPI within microseconds
/// instead of waiting up to a host-timer tick; otherwise `smp_call_function`'s
/// `csd_lock_wait` spins in the guest. Pure EOI: receipt is the effect.
pub const VCPU_KICK_VECTOR: u8 = 51;

extern "x86-interrupt" fn vcpu_kick_handler(_frame: InterruptStackFrame) {
    if let Some(lapic) = worker_lapic() {
        lapic.w32(LAPIC_EOI, 0); // each core EOIs its own LAPIC
    }
}

/// Set up the calling worker's LAPIC timer: one-shot (or TSC-deadline)
/// mode on `WORKER_TIMER_VECTOR`, nothing armed. Each AP calls it once at
/// boot.
pub fn init_worker_timer() {
    let lapic = lapic_from_msr();
    let base = lapic.base();
    WORKER_APIC_BASE.store(base, Ordering::Relaxed);
    // Also publish the (core-invariant) xAPIC base for the device-IRQ ISR EOI
    // path, in case Core 0 runs on the PIT and never set APIC_BASE itself.
    let _ = APIC_BASE.compare_exchange(0, base, Ordering::Relaxed, Ordering::Relaxed);

    let deadline = has_tsc_deadline();
    HAS_TSC_DEADLINE.store(deadline, Ordering::Relaxed);

    let svr = lapic.r32(LAPIC_SVR);
    lapic.w32(LAPIC_SVR, svr | (1 << 8) | SPURIOUS_VECTOR as u32);
    // Divide = 16.
    lapic.w32(LAPIC_TIMER_DIV, 0x03);
    if WORKER_TIMER_INITIAL.load(Ordering::Relaxed) == 0 {
        // Calibrate counts/10ms against the TSC, once — the LAPIC timer
        // clock is the same on every core. Periodic mode for the
        // measurement only; the counter just has to run down.
        lapic.w32(LAPIC_LVT_TIMER, (1 << 16) | WORKER_TIMER_VECTOR as u32);
        let freq = TSC_FREQ.load(Ordering::Relaxed);
        lapic.w32(LAPIC_TIMER_INIT, 0xFFFF_FFFF);
        let start = rdtsc();
        let tsc_10ms = if freq > 0 { freq / 100 } else { 20_000_000 };
        while rdtsc() - start < tsc_10ms { core::hint::spin_loop(); }
        let n = 0xFFFF_FFFFu32
            .wrapping_sub(lapic.r32(LAPIC_TIMER_CUR))
            .max(1);
        WORKER_TIMER_INITIAL.store(n, Ordering::Relaxed);
    }
    // Stop whatever ran before, then set the mode:
    // TSC-deadline = LVT bits 18:17 = 10b, one-shot = 00b.
    lapic.w32(LAPIC_TIMER_INIT, 0);
    let mode = if deadline { 0b10 << 17 } else { 0 };
    lapic.w32(LAPIC_LVT_TIMER, mode | WORKER_TIMER_VECTOR as u32);
    if deadline {
        set_tsc_deadline(0);
    }
    let cid = crate::smp::per_core::current_core_id();
    if cid < 256 {
        ARMED[cid].store(0, Ordering::Relaxed);
    }
}

/// Write IA32_TSC_DEADLINE; 0 disarms. A no-op on a CPU without
/// TSC-deadline mode.
fn set_tsc_deadline(tsc: u64) {
    if !HAS_TSC_DEADLINE.load(Ordering::Relaxed) { return; }
    // SAFETY: the fences only order: the SDM asks for the LVT write to be
    // ordered before the MSR write, and Linux fences the same way
    // (`weak_wrmsr_fence`). IA32_TSC_DEADLINE exists when CPUID.1:ECX[24]
    // is set (checked above); a write only arms or disarms this core's
    // LAPIC timer.
    unsafe {
        core::arch::asm!("mfence; lfence", options(nostack, preserves_flags));
        msr::write(MSR_TSC_DEADLINE, tsc);
    }
}

/// Arm this core's worker timer to fire at TSC `deadline`. A deadline in
/// the past fires at once.
fn arm_at(deadline: u64) {
    let cid = crate::smp::per_core::current_core_id();
    if cid < 256 {
        ARMED[cid].store(deadline.max(1), Ordering::Relaxed);
    }
    if HAS_TSC_DEADLINE.load(Ordering::Relaxed) {
        // Never 0, which disarms.
        set_tsc_deadline(deadline.max(1));
        return;
    }
    let lapic = worker_lapic();
    let per_10ms = WORKER_TIMER_INITIAL.load(Ordering::Relaxed) as u64;
    let tsc_10ms = (TSC_FREQ.load(Ordering::Relaxed) / 100).max(1);
    let Some(lapic) = lapic else { return };
    if per_10ms == 0 { return; }
    let delta = deadline.saturating_sub(rdtsc());
    let count = ((delta as u128 * per_10ms as u128) / tsc_10ms as u128)
        .clamp(1, u32::MAX as u128) as u32;
    // Writing the initial count starts the one-shot countdown.
    lapic.w32(LAPIC_TIMER_INIT, count);
}

fn disarm() {
    let cid = crate::smp::per_core::current_core_id();
    if cid < 256 {
        ARMED[cid].store(0, Ordering::Relaxed);
    }
    if HAS_TSC_DEADLINE.load(Ordering::Relaxed) {
        set_tsc_deadline(0);
        return;
    }
    if let Some(lapic) = worker_lapic() {
        lapic.w32(LAPIC_TIMER_INIT, 0); // 0 stops it
    }
}

/// Arm this core's one-shot to `deadline` before a VM entry, unless it is
/// armed there already. The fire is a host interrupt → #VMEXIT → the run
/// loop delivers the guest's due timer (KVM: the APIC/PIT hrtimer kicks the
/// vCPU). Any other exit, a park or a fire resets it; the loop re-arms.
pub fn arm_vcpu_timer(deadline: u64) {
    let cid = crate::smp::per_core::current_core_id();
    if cid < 256 && ARMED[cid].load(Ordering::Relaxed) == deadline.max(1) {
        return;
    }
    arm_at(deadline);
}

/// The idle primitive of a worker: halt until an interrupt, and, if
/// `deadline` is given — no later than that TSC value. IF is restored as
/// found. `cause` is the wake-attribution slot for `cores`.
///
/// Arming and halting happen with IF=0 and the halt is entered as
/// `sti; hlt`: an interrupt landing between the arm and the halt stays
/// pending and ends the halt, instead of being consumed before it and
/// leaving the core asleep with nothing armed.
///
/// On Core 0 before it went tickless the timer is not touched; the halt
/// ends at the next tick there.
pub fn halt_until(deadline: Option<u64>, cause: usize) {
    let cid = crate::smp::per_core::current_core_id();
    let own_timer = cid < 256
        && (cid != 0 || CORE0_TICKLESS.load(Ordering::Relaxed));
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
    let port = DEEP_IDLE_PORT.load(Ordering::Relaxed);
    let deep = port != 0 && match deadline {
        None => true,
        Some(d) => d.saturating_sub(t0) >= DEEP_IDLE_MIN_TSC.load(Ordering::Relaxed),
    };
    if deep {
        // ACPI SystemIO C-state (Linux `io_idle`): a read of the port the
        // core traps as its C-state entry request. Entered with IF=0, like
        // Linux's cpuidle: a pending interrupt is a break event regardless
        // of IF, so there is no window between arming and sleeping. The
        // `sti; nop; cli` afterwards takes that interrupt here, as the hlt
        // path does. No dummy PM-timer read: Linux restricts it to old Intel.
        // SAFETY: `port` was accepted by `set_deep_idle` only inside the
        // CPU's own C-state trap range (CStateBaseAddr..+8); reading it has
        // no effect but the idle entry.
        unsafe {
            core::arch::asm!("in al, dx", in("dx") port, out("al") _,
                options(nomem, nostack, preserves_flags));
            core::arch::asm!("sti; nop; cli");
        }
        DEEP_IDLE_COUNT[cid.min(255)].fetch_add(1, Ordering::Relaxed);
    } else {
        // SAFETY: sti-shadow arms the HLT before any pending IRQ is taken.
        unsafe { core::arch::asm!("sti; hlt; cli") };
    }
    crate::smp::per_core::note_woke(cid);
    let t1 = rdtsc();
    crate::smp::per_core::record_halt(cid, t1.saturating_sub(t0));
    if let Some(d) = deadline {
        crate::smp::per_core::note_deadline_late(cid, t1.saturating_sub(d));
    }
    // Core 0's wakes are attributed by its ISRs (timer, input).
    if cid != 0 {
        crate::smp::per_core::record_wake(cid, cause);
    }
    // Woken by something else: the one-shot is still pending — drop it.
    if own_timer && deadline.is_some() { disarm(); }
    if rflags & (1 << 9) != 0 {
        // SAFETY: IF was set on entry.
        unsafe { core::arch::asm!("sti") };
    }
}

/// How often `worker_idle_hlt` wakes on the calling worker: `hz` per second.
/// A native network loop (download) raises it to poll its NIC ring without a
/// device IRQ; pass 100 to go back to 10 ms.
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

// ── Input interrupts ────────────────────────────────────────────────
//
// The i8042 raises ISA IRQ 1 (and 12 for the aux port) through the I/O
// APIC, and each xHCI controller raises MSI-X; both land on Core 0 and
// drain right away. The timer tick also drains as a fallback: both paths
// run in interrupt context on Core 0, so they never interleave, and the
// xHCI path takes its controller lock with `try_lock`.

/// i8042 (keyboard IRQ 1, aux IRQ 12), routed by `keyboard::enable_irq`.
pub const PS2_VECTOR: u8 = 53;
/// Every xHCI controller's interrupter 0, programmed by `xhci::init`.
pub const XHCI_VECTOR: u8 = 54;

fn lapic_eoi() {
    lapic().w32(LAPIC_EOI, 0);
}

extern "x86-interrupt" fn ps2_irq_handler(_frame: InterruptStackFrame) {
    crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_KEYBOARD);
    crate::keyboard::poll_ps2_irq();
    crate::intent::wake_shell();
    lapic_eoi();
}

extern "x86-interrupt" fn xhci_irq_handler(_frame: InterruptStackFrame) {
    crate::smp::per_core::record_wake(0, crate::smp::per_core::WAKE_KEYBOARD);
    crate::xhci::msi_irq();
    crate::intent::wake_shell();
    lapic_eoi();
}

/// Clear IF and return RFLAGS as it was, for `irq_restore`. For a critical
/// section that does not fit a closure (`without_interrupts`).
pub fn irq_save() -> u64 {
    let rflags: u64;
    // SAFETY: save RFLAGS and clear IF; the caller restores via `irq_restore`.
    unsafe { core::arch::asm!("pushfq; pop {}; cli", out(reg) rflags) };
    rflags
}

/// Set IF again if it was set in `rflags` (from `irq_save`).
pub fn irq_restore(rflags: u64) {
    if rflags & (1 << 9) != 0 {
        // SAFETY: IF was set when `irq_save` ran.
        unsafe { core::arch::asm!("sti") };
    }
}

/// Run `f` with this core's interrupts masked, restoring the previous IF.
///
/// For a spin lock that an ISR on the same core may also take: held with
/// IF=1, the timer IRQ lands inside the critical section and spins on the
/// lock its own core holds, deadlocking the core. Cheap (pushfq/cli/popfq), so
/// keep `f` short; it cannot be interrupted.
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let rflags: u64;
    // SAFETY: save RFLAGS and clear IF; restored below exactly as found.
    unsafe { core::arch::asm!("pushfq; pop {}; cli", out(reg) rflags) };
    let r = f();
    if rflags & (1 << 9) != 0 {
        // SAFETY: IF was set on entry; set it again.
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
    if let Some(lapic) = lapic_cached() {
        lapic.w32(LAPIC_EOI, 0);
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
    lapic().r32(LAPIC_ID) >> 24
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

    let lapic = lapic_from_msr();
    let apic_base = lapic.base();
    APIC_BASE.store(apic_base, Ordering::Relaxed);

    // Map APIC page (identity-mapped, uncacheable)
    let _ = crate::paging::map_page(apic_base, apic_base,
        crate::paging::PageFlags::PRESENT | crate::paging::PageFlags::WRITABLE | crate::paging::PageFlags::NO_CACHE);

    // SAFETY: one IDT entry written on the BSP; the vector is not live
    // until the LVT below names it.
    unsafe {
        IDT[APIC_TIMER_VECTOR as usize].set_handler(apic_timer_handler as *const () as u64);
    }

    // Enable APIC: Spurious Interrupt Vector Register, bit 8 = APIC enable,
    // bits 0-7 = spurious vector.
    let svr = lapic.r32(LAPIC_SVR);
    lapic.w32(LAPIC_SVR, svr | (1 << 8) | SPURIOUS_VECTOR as u32);

    // Timer divide = 16.
    lapic.w32(LAPIC_TIMER_DIV, 0x03);

    // Calibrate: measure APIC ticks per 10ms using TSC
    lapic.w32(LAPIC_TIMER_INIT, 0xFFFF_FFFF); // max initial count
    let tsc_start = rdtsc();
    let tsc_10ms = freq / 100;
    while rdtsc() - tsc_start < tsc_10ms { core::hint::spin_loop(); }
    let elapsed = 0xFFFF_FFFFu32 - lapic.r32(LAPIC_TIMER_CUR);

    // LVT timer: bit 17 = periodic, bits 0-7 = vector.
    lapic.w32(LAPIC_LVT_TIMER, (1 << 17) | APIC_TIMER_VECTOR as u32);

    // Initial count: ticks per 10ms = 100Hz.
    lapic.w32(LAPIC_TIMER_INIT, elapsed);

    crate::kdebug!("[npk] APIC timer: {}Hz (base={:#x}, ticks/10ms={})",
        TARGET_FREQ, apic_base, elapsed);
}

