//! Per-core GDT + Task State Segment.
//!
//! Every core gets its own GDT (the boot GDT's code and data descriptors
//! plus a TSS descriptor; the busy bit `ltr` sets means cores cannot share
//! one) and its own TSS. The TSS carries IST1, the stack the double-fault
//! handler runs on: a fault while pushing an exception frame — a kernel
//! stack running into its guard page — can only be reported from a stack
//! that is known to be good. VMX needs a valid TR as well
//! (HOST_TR_SELECTOR must not be 0, SDM Vol. 3C §26.2.3); it reads TR and
//! GDTR back with `str`/`sgdt`, so nothing here is VMX-specific.
//!
//! Reference: Intel SDM Vol. 3A §3.4.5.1, §7.7, §6.14.5 (IST).

use alloc::boxed::Box;

/// Long-mode TSS layout (104 bytes, no I/O bitmap).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Tss {
    _reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    _reserved1: u64,
    ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

const TSS_LIMIT: u16 = (core::mem::size_of::<Tss>() - 1) as u16;

/// IST slot of the double-fault stack (1-based, as the IDT entry names it).
pub const DOUBLE_FAULT_IST: u8 = 1;

/// Size of each core's double-fault stack. The handler only prints and
/// halts, but printing walks the console path.
const DF_STACK_BYTES: usize = 32 * 1024;

/// 10-byte pseudo-descriptor consumed by `lgdt`.
#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

/// Everything one core's `lgdt`/`ltr` points at. Leaked on purpose: the CPU
/// keeps using it for the life of the core.
#[repr(C, align(16))]
struct CoreTables {
    gdt: [u64; 5],
    gdtr: GdtPointer,
    tss: Tss,
}

/// TSS selector for `ltr`: index 3, TI=0, RPL=0.
const TSS_SELECTOR: u16 = 3 << 3;

/// Build the 16-byte long-mode TSS descriptor (two GDT slots) for a TSS
/// at `tss_base`. Returns `(lo, hi)`. SDM Vol. 3A §7.2.3.
fn tss_descriptor(tss_base: u64) -> (u64, u64) {
    let limit_lo = TSS_LIMIT as u64;
    let base_lo15 = tss_base & 0xFFFF;
    let base_lo23 = (tss_base >> 16) & 0xFF;
    let base_lo31 = (tss_base >> 24) & 0xFF;
    let access: u64 = 0x89; // P=1, DPL=0, S=0, type=9 (available 64-bit TSS)
    let desc_lo = limit_lo
        | (base_lo15 << 16)
        | (base_lo23 << 32)
        | (access << 40)
        | (base_lo31 << 56);
    let desc_hi = (tss_base >> 32) & 0xFFFF_FFFF;
    (desc_lo, desc_hi)
}

/// Install this core's GDT and TSS and `ltr` it. Call once per core, as
/// early as the heap allows: the BSP before the APs start, each AP first
/// thing in its entry. Until then a double fault on that core has no
/// usable IST stack.
pub fn init_core() {
    let df_stack: &'static mut [u128] =
        Box::leak(alloc::vec![0u128; DF_STACK_BYTES / 16].into_boxed_slice());
    let df_top = df_stack.as_ptr() as u64 + DF_STACK_BYTES as u64; // 16-aligned

    let mut ist = [0u64; 7];
    ist[DOUBLE_FAULT_IST as usize - 1] = df_top;
    let tables: &'static mut CoreTables = Box::leak(Box::new(CoreTables {
        gdt: [
            0,                     // null
            0x00AF_9A00_0000_FFFF, // code (0x08, ring0, L=1, P=1, type=0xA) — as boot.s
            0x00CF_9200_0000_FFFF, // data (0x10, ring0, P=1, type=0x2) — as boot.s
            0,                     // TSS descriptor, low half
            0,                     // TSS descriptor, high half
        ],
        gdtr: GdtPointer { limit: 0, base: 0 },
        tss: Tss {
            _reserved0: 0,
            rsp0: 0, rsp1: 0, rsp2: 0,
            _reserved1: 0,
            ist,
            _reserved2: 0, _reserved3: 0,
            iomap_base: TSS_LIMIT + 1, // no I/O bitmap
        },
    }));

    let (lo, hi) = tss_descriptor(core::ptr::addr_of!(tables.tss) as u64);
    tables.gdt[3] = lo;
    tables.gdt[4] = hi;
    tables.gdtr = GdtPointer {
        limit: (core::mem::size_of::<[u64; 5]>() - 1) as u16,
        base: tables.gdt.as_ptr() as u64,
    };

    // SAFETY: `tables` is leaked, so the GDT and TSS outlive the core. Slots
    // 1 and 2 match the boot GDT byte for byte, so CS/SS/DS stay valid
    // without reloading the segment registers. The TSS descriptor is fresh
    // (not busy), which `ltr` requires.
    unsafe {
        core::arch::asm!(
            "lgdt [{ptr}]",
            "ltr {sel:x}",
            ptr = in(reg) core::ptr::addr_of!(tables.gdtr),
            sel = in(reg) TSS_SELECTOR,
            options(nostack, preserves_flags),
        );
    }
}
