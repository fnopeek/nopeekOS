//! Interrupt Descriptor Table + PIC 8259
//!
//! Exception handlers + timer IRQ for hlt wakeup.
//! Phase 2+: keyboard IRQ, serial IRQ, TSS with IST for double fault

use crate::serial::{outb, inb};
use crate::kprintln;
use core::sync::atomic::{AtomicU64, Ordering};

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
    // Fallback: 2 GHz default
    TSC_FREQ.store(2_000_000_000, Ordering::Relaxed);
    kprintln!("[npk] TSC: 2000 MHz (default, CPUID 0x15 not available)");
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

        // Load IDT
        let idt_reg = IdtRegister {
            limit: (IDT_SIZE * core::mem::size_of::<IdtEntry>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        // SAFETY: IDT is fully initialized above
        core::arch::asm!("lidt [{}]", in(reg) &idt_reg);

        pic_remap();

        // Unmask IRQ0 (timer) + IRQ1 (keyboard)
        outb(PIC1_DATA, 0xFC);
        outb(PIC2_DATA, 0xFF);

        // Program PIT channel 0 to 100 Hz (10ms per tick)
        let divisor = (PIT_BASE_FREQ / TARGET_FREQ) as u16;
        outb(PIT_COMMAND, 0x36); // Channel 0, lobyte/hibyte, rate generator
        outb(PIT_CHANNEL0, divisor as u8);
        outb(PIT_CHANNEL0, (divisor >> 8) as u8);

        // SAFETY: IDT loaded, PIC configured, PIT programmed, handlers set
        core::arch::asm!("sti");
    }
}

/// Remap PIC: IRQ0-7 → 32-39, IRQ8-15 → 40-47
/// Without remapping, hardware IRQs collide with CPU exception vectors
unsafe fn pic_remap() {
    let mask1 = inb(PIC1_DATA);
    let mask2 = inb(PIC2_DATA);

    outb(PIC1_CMD, 0x11); io_wait();
    outb(PIC2_CMD, 0x11); io_wait();
    outb(PIC1_DATA, PIC_OFFSET_MASTER); io_wait();
    outb(PIC2_DATA, PIC_OFFSET_SLAVE); io_wait();
    outb(PIC1_DATA, 0x04); io_wait(); // Slave on IRQ2
    outb(PIC2_DATA, 0x02); io_wait();
    outb(PIC1_DATA, 0x01); io_wait(); // 8086 mode
    outb(PIC2_DATA, 0x01); io_wait();

    outb(PIC1_DATA, mask1);
    outb(PIC2_DATA, mask2);
}

unsafe fn pic_eoi(irq: u8) {
    if irq >= 8 { outb(PIC2_CMD, PIC_EOI); }
    outb(PIC1_CMD, PIC_EOI);
}

/// Port 0x80 write provides ~1µs bus delay for PIC timing
#[inline(always)]
unsafe fn io_wait() {
    outb(0x80, 0x00);
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
    halt_loop();
}

// === IRQ Handlers ===

extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    let tick = TICKS.fetch_add(1, Ordering::Relaxed);
    // Drain USB events from interrupt context (try_lock, never blocks)
    crate::xhci::poll_events_irq();
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
    crate::keyboard::irq_handler();
    unsafe { pic_eoi(1); }
}

/// APIC timer handler — fires on hardware without PIT (NUC, UEFI-only).
/// Same function as PIT timer: tick counter + USB event drain.
extern "x86-interrupt" fn apic_timer_handler(_frame: InterruptStackFrame) {
    let tick = TICKS.fetch_add(1, Ordering::Relaxed);
    crate::xhci::poll_events_irq();
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
