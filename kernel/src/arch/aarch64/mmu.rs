//! The identity map, in two steps as Linux builds it.
//!
//! `early_mmu` runs with the MMU and the data cache off and maps only the
//! kernel image and the device tree, with single aligned stores: until the
//! MMU is on all memory is Device memory, where an unaligned or wide access
//! faults. `init` then builds the real map from the device tree, with the
//! caches on: the first 512 GiB of physical address space at themselves
//! (4 KiB granule, TTBR0, 48-bit VA), every 2 MiB block that holds RAM as
//! Normal write-back memory, everything else as Device-nGnRE and never
//! executable. Device windows need no mapping call before use, as on x86
//! with its identity map.

use crate::platform::dt::Fdt;

/// End of the identity map: frames above it are never handed out.
pub const IDENTITY_MAX: u64 = 512 << 30;

const GIB: u64 = 1 << 30;
const MIB2: u64 = 2 << 20;

/// RAM covered by second-level tables: the GiBs that hold RAM.
const L2_TABLES: usize = 64;

/// MAIR attribute indices.
const ATTR_DEVICE: u64 = 0; // Device-nGnRE
const ATTR_NORMAL: u64 = 1; // Normal, write-back, read/write-allocate
const MAIR: u64 = 0x04 | (0xff << 8) | (0x44 << 16); // attr2: Normal non-cacheable

const VALID: u64 = 1;
const TABLE: u64 = 1 << 1;
const AF: u64 = 1 << 10;
const SH_INNER: u64 = 3 << 8;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

const DEVICE_BLOCK: u64 = VALID | AF | (ATTR_DEVICE << 2) | PXN | UXN;
const NORMAL_BLOCK: u64 = VALID | AF | SH_INNER | (ATTR_NORMAL << 2) | UXN;

#[repr(C, align(4096))]
struct Table([u64; 512]);

#[repr(C, align(4096))]
struct Tables {
    l0: Table,
    l1: Table,
    l2: [Table; L2_TABLES],
}

static mut BOOT: Tables = Tables {
    l0: Table([0; 512]),
    l1: Table([0; 512]),
    l2: [const { Table([0; 512]) }; L2_TABLES],
};

static mut TABLES: Tables = Tables {
    l0: Table([0; 512]),
    l1: Table([0; 512]),
    l2: [const { Table([0; 512]) }; L2_TABLES],
};

/// RAM as the device tree's /memory nodes describe it.
pub const MAX_RAM: usize = 32;
static mut RAM: [(u64, u64); MAX_RAM] = [(0, 0); MAX_RAM];
static mut RAM_COUNT: usize = 0;

/// The RAM ranges found at boot.
pub fn ram() -> &'static [(u64, u64)] {
    // SAFETY: written once by `init` on the boot core before anything reads it.
    unsafe { &*core::ptr::slice_from_raw_parts(core::ptr::addr_of!(RAM) as *const (u64, u64), RAM_COUNT) }
}

fn holds_ram(lo: u64, hi: u64) -> bool {
    ram().iter().any(|&(b, s)| b < hi && lo < b + s)
}

unsafe extern "C" {
    static __image_base: u8;
    static __heap_start: u8;
}

/// Store one descriptor with a single aligned 8-byte write.
///
/// # Safety
/// `table` points at a 4 KiB-aligned table of 512 entries; `i < 512`.
unsafe fn put(table: *mut Table, i: u64, v: u64) {
    // SAFETY: the caller's contract.
    unsafe { core::ptr::write_volatile((table as *mut u64).add(i as usize), v) };
}

/// Map the 2 MiB blocks of `[lo, hi)` as Normal memory in the boot tables.
///
/// # Safety
/// Single-threaded boot with the MMU off; nothing else uses `BOOT`.
unsafe fn boot_map(lo: u64, hi: u64, next_l2: &mut usize) {
    let t = core::ptr::addr_of_mut!(BOOT);
    let mut a = lo & !(MIB2 - 1);
    while a < hi {
        let g = a / GIB;
        if g >= 512 { return; }
        // SAFETY: `t` is the boot table set; every index is below 512.
        unsafe {
            let l1 = core::ptr::addr_of_mut!((*t).l1);
            let e = core::ptr::read_volatile((l1 as *const u64).add(g as usize));
            let l2 = if e & VALID != 0 {
                (e & 0x0000_ffff_ffff_f000) as *mut Table
            } else {
                if *next_l2 == L2_TABLES { return; }
                let l2 = core::ptr::addr_of_mut!((*t).l2[*next_l2]);
                *next_l2 += 1;
                put(l1, g, l2 as u64 | TABLE | VALID);
                l2
            };
            put(l2, (a % GIB) / MIB2, a | NORMAL_BLOCK);
        }
        a += MIB2;
    }
}

/// Map the kernel image and the device tree, then turn on MMU and caches.
/// Called once from `_start`.
#[unsafe(no_mangle)]
pub extern "C" fn early_mmu(dtb: usize) {
    // SAFETY: linker symbols; only their addresses are taken.
    let (lo, hi) = unsafe { (&__image_base as *const u8 as u64, &__heap_start as *const u8 as u64) };
    let mut next_l2 = 0;
    // SAFETY: single-threaded boot with the MMU off. The tree's size is
    // not read yet (its header lies in memory not mapped as RAM); 4 MiB
    // covers any board's tree.
    unsafe {
        boot_map(lo, hi, &mut next_l2);
        boot_map(dtb as u64, dtb as u64 + (4 << 20), &mut next_l2);
        let t = core::ptr::addr_of_mut!(BOOT);
        put(core::ptr::addr_of_mut!((*t).l0), 0, core::ptr::addr_of!((*t).l1) as u64 | TABLE | VALID);
        enable(t as u64, core::mem::size_of::<Tables>() as u64, true);
    }
}

/// Load `tables` and turn on MMU and caches (or switch to `tables` when
/// they are on already).
///
/// # Safety
/// `tables` maps the running code, its stack and its data at themselves.
unsafe fn enable(tables: u64, len: u64, first: bool) {
    let el2 = crate::arch::exception_level() == 2;
    let pa_range: u64;
    // SAFETY: reads the CPU's physical address size, no other effect.
    unsafe { core::arch::asm!("mrs {}, ID_AA64MMFR0_EL1", out(reg) pa_range, options(nomem, nostack)) };
    // T0SZ = 16 (48-bit), IRGN0/ORGN0 write-back, inner shareable, 4K
    // granule, EPD1 (no TTBR1 walks), IPS = what the CPU supports.
    let tcr: u64 = 16 | (1 << 8) | (1 << 10) | (3 << 12) | (1 << 23) | ((pa_range & 7) << 32);
    // SAFETY: the caller's contract. With the caches still off the walker
    // must not find stale lines of the table memory, so they are dropped;
    // with the caches on, the stores are made visible to the walker.
    unsafe {
        let mut a = tables & !63;
        while a < tables + len {
            if first {
                core::arch::asm!("dc ivac, {}", in(reg) a, options(nostack));
            } else {
                core::arch::asm!("dc cvac, {}", in(reg) a, options(nostack));
            }
            a += 64;
        }
        core::arch::asm!("dsb sy", options(nostack));
        core::arch::asm!(
            "msr MAIR_EL1, {mair}",
            "msr TCR_EL1, {tcr}",
            "msr TTBR0_EL1, {ttbr}",
            "isb",
            mair = in(reg) MAIR,
            tcr = in(reg) tcr,
            ttbr = in(reg) tables,
            options(nostack),
        );
        if el2 {
            core::arch::asm!("tlbi alle2", options(nostack));
        } else {
            core::arch::asm!("tlbi vmalle1", options(nostack));
        }
        core::arch::asm!("dsb sy", "isb", options(nostack));
        if first {
            core::arch::asm!(
                "ic iallu",
                "dsb sy",
                "isb",
                "mrs {s}, SCTLR_EL1",
                "orr {s}, {s}, #(1 << 0)",
                "orr {s}, {s}, #(1 << 2)",
                "orr {s}, {s}, #(1 << 12)",
                "bic {s}, {s}, #(1 << 1)",
                "bic {s}, {s}, #(1 << 19)",
                "msr SCTLR_EL1, {s}",
                "isb",
                s = out(reg) _,
                options(nostack),
            );
        }
    }
}

/// Build the full identity map from the device tree and switch to it.
pub fn init(fdt: &Fdt) {
    let mut n = 0;
    fdt.walk(|node| {
        if node.depth == 1 && (fdt.name_is(node, b"memory") || fdt.prop_has_str(node, b"device_type", b"memory")) {
            fdt.reg(node, |a, s| {
                if n < MAX_RAM && s > 0 {
                    // SAFETY: single-threaded boot, the only writer.
                    unsafe { (*core::ptr::addr_of_mut!(RAM))[n] = (a, s) };
                    n += 1;
                }
            });
        }
        true
    });
    // SAFETY: as above.
    unsafe { RAM_COUNT = n };

    // SAFETY: single-threaded boot; the walker does not use these tables yet.
    let t = unsafe { &mut *core::ptr::addr_of_mut!(TABLES) };
    let mut next_l2 = 0;
    for g in 0..512u64 {
        let lo = g * GIB;
        if !holds_ram(lo, lo + GIB) || next_l2 == L2_TABLES {
            t.l1.0[g as usize] = lo | DEVICE_BLOCK;
            continue;
        }
        let l2 = &mut t.l2[next_l2];
        next_l2 += 1;
        for b in 0..512u64 {
            let a = lo + b * MIB2;
            l2.0[b as usize] = a | if holds_ram(a, a + MIB2) { NORMAL_BLOCK } else { DEVICE_BLOCK };
        }
        t.l1.0[g as usize] = l2 as *const Table as u64 | TABLE | VALID;
    }
    t.l0.0[0] = &t.l1 as *const Table as u64 | TABLE | VALID;
    // SAFETY: the new tables map all RAM, so the kernel image, its stack
    // and the device tree, at themselves with the attributes the boot
    // tables gave them.
    unsafe { enable(t as *const Tables as u64, core::mem::size_of::<Tables>() as u64, false) };
}
