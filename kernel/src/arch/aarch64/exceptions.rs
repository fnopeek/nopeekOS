//! Exceptions: the vector table (`vectors.s`) and the report it ends in.

core::arch::global_asm!(include_str!("vectors.s"));

unsafe extern "C" {
    static exception_vectors: u8;
}

/// Where `report` runs, one per core (TPIDR_EL1 index): never the stack
/// that faulted, which may have just hit its guard page.
#[repr(C, align(16))]
struct FaultStacks([[u8; 16 * 1024]; super::cores::MAX_CORES]);

#[unsafe(export_name = "fault_stacks")]
static mut FAULT_STACKS: FaultStacks = FaultStacks([[0; 16 * 1024]; super::cores::MAX_CORES]);

/// Point VBAR at the vector table. With VHE this is VBAR_EL2.
pub fn init() {
    // SAFETY: the table is 2 KiB aligned code in the kernel image; taking
    // an exception through it reports and halts.
    unsafe {
        let v = &exception_vectors as *const u8 as u64;
        core::arch::asm!("msr VBAR_EL1, {}", "isb", in(reg) v, options(nostack));
    }
}

const KIND: [&str; 4] = ["synchronous", "IRQ", "FIQ", "SError"];
const FROM: [&str; 4] = ["current EL, SP0", "current EL", "lower EL (AArch64)", "lower EL (AArch32)"];

/// The exception class of ESR_ELx (Arm ARM D24.2.40), for the cases a
/// kernel fault produces.
fn class(ec: u64) -> &'static str {
    match ec {
        0x00 => "unknown reason",
        0x01 => "WFI/WFE trapped",
        0x07 => "FP/SIMD access trapped",
        0x0e => "illegal execution state",
        0x15 => "SVC",
        0x16 => "HVC",
        0x17 => "SMC",
        0x18 => "MSR/MRS trapped",
        0x20 | 0x21 => "instruction abort",
        0x22 => "PC alignment fault",
        0x24 | 0x25 => "data abort",
        0x26 => "SP alignment fault",
        0x2f => "SError",
        0x3c => "BRK",
        _ => "other",
    }
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_exception(kind: u64, esr: u64, elr: u64, far: u64, spsr: u64, sp: u64, lr: u64) -> ! {
    let ec = (esr >> 26) & 0x3f;
    let guard = matches!(ec, 0x24 | 0x25) && crate::mm::stack::is_guard(far);
    crate::fatal::die(format_args!(
        "{} exception from {}: {} (EC {:#x}){}\n[npk] ESR {:#x}  ELR {:#x}  FAR {:#x}\n[npk] SPSR {:#x}  SP {:#x}  LR {:#x}",
        KIND[(kind & 3) as usize], FROM[((kind >> 2) & 3) as usize], class(ec), ec,
        if guard { ", stack overflow into the guard page" } else { "" },
        esr, elr, far, spsr, sp, lr))
}
