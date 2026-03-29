//! Interrupt Descriptor Table + PIC 8259
//!
//! Exception handlers + timer IRQ for hlt wakeup.
//! Phase 2+: keyboard IRQ, serial IRQ, TSS with IST for double fault

use crate::serial::{outb, inb};
use crate::kprintln;
use core::sync::atomic::{AtomicU64, Ordering};

/// Monotonic tick counter, incremented by timer IRQ at 100 Hz.
/// 1 tick = 10ms. Wraps after ~5.8 billion years.
static TICKS: AtomicU64 = AtomicU64::new(0);

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Seconds since boot (approximate)
pub fn uptime_secs() -> u64 {
    TICKS.load(Ordering::Relaxed) / 100
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

        // Load IDT
        let idt_reg = IdtRegister {
            limit: (IDT_SIZE * core::mem::size_of::<IdtEntry>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        // SAFETY: IDT is fully initialized above
        core::arch::asm!("lidt [{}]", in(reg) &idt_reg);

        pic_remap();

        // Only unmask IRQ0 (timer) — needed for hlt wakeup
        outb(PIC1_DATA, 0xFE);
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
    TICKS.fetch_add(1, Ordering::Relaxed);
    // No lock acquisition — deadlock-free
    unsafe { pic_eoi(0); }
}

fn halt_loop() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}
