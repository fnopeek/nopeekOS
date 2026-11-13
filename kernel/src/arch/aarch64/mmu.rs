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
const ATTR_NC: u64 = 2; // Normal non-cacheable
const MAIR: u64 = 0x04 | (0xff << 8) | (0x44 << 16);

const VALID: u64 = 1;
const TABLE: u64 = 1 << 1;
const AF: u64 = 1 << 10;
const SH_INNER: u64 = 3 << 8;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

const DEVICE_BLOCK: u64 = VALID | AF | (ATTR_DEVICE << 2) | PXN | UXN;
/// RAM: never executable (Linux maps the linear map PXN, `CONFIG_STRICT_KERNEL_RWX`);
/// the kernel's text is mapped apart by `map_image`.
const NORMAL_BLOCK: u64 = VALID | AF | SH_INNER | (ATTR_NORMAL << 2) | PXN | UXN;
/// A 4 KiB page of the kernel image; AP[2] (`RO`) makes it read-only.
const IMAGE_PAGE: u64 = VALID | TABLE | AF | SH_INNER | (ATTR_NORMAL << 2) | UXN;
const RO: u64 = 1 << 7;
/// Third-level tables for the 2 MiB blocks the kernel image covers.
const L3_TABLES: usize = 8;
const NC_BLOCK: u64 = VALID | AF | SH_INNER | (ATTR_NC << 2) | PXN | UXN;

#[repr(C, align(4096))]
struct Table([u64; 512]);

#[repr(C, align(4096))]
struct Tables {
    l0: Table,
    l1: Table,
    l2: [Table; L2_TABLES],
    l3: [Table; L3_TABLES],
}

static mut BOOT: Tables = Tables {
    l0: Table([0; 512]),
    l1: Table([0; 512]),
    l2: [const { Table([0; 512]) }; L2_TABLES],
    l3: [const { Table([0; 512]) }; L3_TABLES],
};

/// Second-level tables of `TABLES` in use.
static mut NEXT_L2: usize = 0;

static mut TABLES: Tables = Tables {
    l0: Table([0; 512]),
    l1: Table([0; 512]),
    l2: [const { Table([0; 512]) }; L2_TABLES],
    l3: [const { Table([0; 512]) }; L3_TABLES],
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
    static __text_end: u8;
    static __rodata_end: u8;
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
unsafe fn boot_map(lo: u64, hi: u64, kind: u64, next_l2: &mut usize) {
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
            put(l2, (a % GIB) / MIB2, a | kind);
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
        // The tree first: where both share a 2 MiB block, the image's
        // executable mapping must win until `init` maps it page by page.
        boot_map(dtb as u64, dtb as u64 + (4 << 20), NORMAL_BLOCK, &mut next_l2);
        boot_map(lo, hi, NORMAL_BLOCK & !PXN, &mut next_l2);
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
    map_image(t);
    // SAFETY: single-threaded boot, the only writer.
    unsafe { NEXT_L2 = next_l2 };
    // SAFETY: the new tables map all RAM, so the kernel image, its stack
    // and the device tree, at themselves with the attributes the boot
    // tables gave them.
    unsafe { enable(t as *const Tables as u64, core::mem::size_of::<Tables>() as u64, false) };
}

/// The kernel image in 4 KiB pages with its own permissions (Linux
/// `map_kernel_segment`): text read-only and executable, rodata read-only,
/// data, bss and the boot stack writable; neither of the last two
/// executable. Runs on the tables `init` builds, before they are live.
fn map_image(t: &mut Tables) {
    // SAFETY: linker symbols; only their addresses are taken.
    let (base, text_end, ro_end, end) = unsafe {
        (
            &__image_base as *const u8 as u64,
            &__text_end as *const u8 as u64,
            &__rodata_end as *const u8 as u64,
            &__heap_start as *const u8 as u64,
        )
    };
    let (lo, hi) = (base & !(MIB2 - 1), end.next_multiple_of(MIB2));
    if ((hi - lo) / MIB2) as usize > L3_TABLES {
        crate::kprintln!("[npk] mmu: kernel image {:#x}..{:#x} beyond its page tables; left writable", lo, hi);
        return;
    }
    let mut block = lo;
    let mut n = 0;
    while block < hi {
        let l1 = t.l1.0[(block / GIB) as usize];
        if l1 & TABLE == 0 {
            crate::kprintln!("[npk] mmu: kernel image at {:#x} not in a 2 MiB table", block);
            return;
        }
        let l2 = (l1 & 0x0000_ffff_ffff_f000) as *mut u64;
        let l3 = &mut t.l3[n];
        n += 1;
        for i in 0..512u64 {
            let a = block + i * 4096;
            l3.0[i as usize] = a | if a < base || a >= end {
                IMAGE_PAGE | PXN
            } else if a < text_end {
                IMAGE_PAGE | RO
            } else if a < ro_end {
                IMAGE_PAGE | RO | PXN
            } else {
                IMAGE_PAGE | PXN
            };
        }
        // SAFETY: `l2` is one of `t.l2`, identity-mapped static memory not
        // yet walked by the MMU.
        unsafe { core::ptr::write_volatile(l2.add(((block % GIB) / MIB2) as usize), l3 as *const Table as u64 | TABLE | VALID) };
        block += MIB2;
    }
}

/// Drop stale translations on every core (inner shareable broadcast).
fn flush_tlb() {
    // SAFETY: TLB maintenance for the running regime, no memory effect.
    unsafe {
        core::arch::asm!("dsb ishst", options(nostack));
        if crate::arch::exception_level() == 2 {
            core::arch::asm!("tlbi alle2is", options(nostack));
        } else {
            core::arch::asm!("tlbi vmalle1is", options(nostack));
        }
        core::arch::asm!("dsb ish", "isb", options(nostack));
    }
}

/// Write back and invalidate the cache lines of `[lo, hi)`.
fn clean_inval(lo: u64, hi: u64) {
    let mut a = lo & !63;
    while a < hi {
        // SAFETY: the range is mapped (identity map), cache maintenance only.
        unsafe { core::arch::asm!("dc civac, {}", in(reg) a, options(nostack)) };
        a += 64;
    }
    // SAFETY: a barrier, no memory effect of its own.
    unsafe { core::arch::asm!("dsb sy", options(nostack)) };
}

/// Map the 2 MiB blocks covering `[lo, hi)` as Normal non-cacheable, for a
/// framebuffer the display reads behind the caches: CPU writes reach memory
/// without a cache clean, and may merge, as write-combining does on x86.
/// The blocks must hold nothing but that buffer's memory, the running code
/// and stack never. False when the range lies above the identity map or no
/// table is left to split a 1 GiB block.
pub fn map_uncached(lo: u64, hi: u64) -> bool {
    set_blocks(lo, hi, NC_BLOCK)
}

/// Map RAM blocks `map_uncached` took back as Normal write-back memory.
/// Nothing may use them while this runs.
pub fn map_cached(lo: u64, hi: u64) -> bool {
    set_blocks(lo, hi, NORMAL_BLOCK)
}

/// Serializes changes to the live map; the cores' walks need no lock.
static REMAP: spin::Mutex<()> = spin::Mutex::new(());

fn set_blocks(lo: u64, hi: u64, kind: u64) -> bool {
    if hi > IDENTITY_MAX || hi <= lo { return false; }
    let _g = REMAP.lock();
    let (lo, hi) = (lo & !(MIB2 - 1), hi.next_multiple_of(MIB2));
    // Write back and drop cached lines while the range is still cacheable:
    // once uncached the CPU no longer looks there, and once cached again it
    // must not find lines from before.
    clean_inval(lo, hi);
    // SAFETY: TABLES is the live map, changed only here under REMAP and in
    // `init` on the boot core; every descriptor change goes through
    // break-before-make.
    unsafe {
        let t = &mut *core::ptr::addr_of_mut!(TABLES);
        let next = &mut *core::ptr::addr_of_mut!(NEXT_L2);
        let mut g = lo / GIB;
        while g * GIB < hi {
            let e = core::ptr::read_volatile(&t.l1.0[g as usize]);
            if e & TABLE == 0 {
                // A 1 GiB block: split it into 2 MiB blocks of the same kind.
                if *next == L2_TABLES { return false; }
                let l2 = &mut t.l2[*next];
                *next += 1;
                let attrs = e & !(0x0000_ffff_ffff_f000) & !(TABLE);
                for b in 0..512u64 {
                    l2.0[b as usize] = (g * GIB + b * MIB2) | attrs;
                }
                core::ptr::write_volatile(&mut t.l1.0[g as usize], 0);
                flush_tlb();
                core::ptr::write_volatile(&mut t.l1.0[g as usize], l2 as *const Table as u64 | TABLE | VALID);
                flush_tlb();
            }
            g += 1;
        }
        let mut a = lo;
        while a < hi {
            let l1 = t.l1.0[(a / GIB) as usize];
            let l2 = (l1 & 0x0000_ffff_ffff_f000) as *mut u64;
            core::ptr::write_volatile(l2.add(((a % GIB) / MIB2) as usize), 0);
            a += MIB2;
        }
        flush_tlb();
        let mut a = lo;
        while a < hi {
            let l1 = t.l1.0[(a / GIB) as usize];
            let l2 = (l1 & 0x0000_ffff_ffff_f000) as *mut u64;
            core::ptr::write_volatile(l2.add(((a % GIB) / MIB2) as usize), a | kind);
            a += MIB2;
        }
        flush_tlb();
    }
    if kind == NORMAL_BLOCK {
        clean_inval(lo, hi);
    }
    true
}

/// The 4 KiB granule above the identity map (`paging`): L1, L2 and L3
/// tables allocated as needed below the live root, page descriptors with
/// the block attributes' meaning.
pub mod page {
    use super::*;

    /// Page descriptor bits besides the address: what `paging::PageFlags`
    /// asks for.
    pub const PAGE: u64 = VALID | TABLE | AF | SH_INNER;
    pub const ATTR_NORMAL_IDX: u64 = ATTR_NORMAL << 2;
    pub const ATTR_DEVICE_IDX: u64 = ATTR_DEVICE << 2;
    pub const ATTR_NC_IDX: u64 = ATTR_NC << 2;
    /// AP[2]: read-only.
    pub const RO: u64 = 1 << 7;
    /// AP[1]: EL0 may access.
    pub const EL0: u64 = 1 << 6;
    pub const PXN_BIT: u64 = PXN;
    pub const UXN_BIT: u64 = UXN;
    const ADDR: u64 = 0x0000_ffff_ffff_f000;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Error {
        /// Inside the identity map, or past the 48-bit range.
        OutOfRange,
        NoMemory,
        AlreadyMapped,
        NotMapped,
    }

    /// The next-level table under `*e`, made when `make`.
    ///
    /// # Safety
    /// `e` points at a descriptor of the live map; REMAP is held.
    unsafe fn next(e: *mut u64, make: bool) -> Result<*mut u64, Error> {
        // SAFETY: the caller's contract; tables are identity-mapped frames.
        unsafe {
            let d = core::ptr::read_volatile(e);
            if d & VALID != 0 {
                if d & TABLE == 0 { return Err(Error::OutOfRange); }
                return Ok((d & ADDR) as *mut u64);
            }
            if !make { return Err(Error::NotMapped); }
            let f = crate::memory::allocate_frame().ok_or(Error::NoMemory)?;
            core::ptr::write_bytes(f as *mut u8, 0, 4096);
            // The zeroed table is visible to the walker before it is linked.
            core::arch::asm!("dsb ishst", options(nostack));
            core::ptr::write_volatile(e, f | TABLE | VALID);
            Ok(f as *mut u64)
        }
    }

    /// The L3 entry of `va`, its tables made when `make`.
    fn walk(va: u64, make: bool) -> Result<*mut u64, Error> {
        if va < IDENTITY_MAX || va >= 1 << 48 { return Err(Error::OutOfRange); }
        let idx = |shift: u32| ((va >> shift) & 511) as usize;
        // SAFETY: TABLES.l0 is the live root; entries above 0 belong to this
        // module alone, changed under REMAP, which the callers hold.
        unsafe {
            let l0 = core::ptr::addr_of_mut!(TABLES.l0.0) as *mut u64;
            let l1 = next(l0.add(idx(39)), make)?;
            let l2 = next(l1.add(idx(30)), make)?;
            let l3 = next(l2.add(idx(21)), make)?;
            Ok(l3.add(idx(12)))
        }
    }

    /// Map the page at `va` to the frame `pa` with `attrs`.
    pub fn map(va: u64, pa: u64, attrs: u64) -> Result<(), Error> {
        let _g = REMAP.lock();
        let e = walk(va, true)?;
        // SAFETY: an L3 entry of the live map, under REMAP.
        unsafe {
            if core::ptr::read_volatile(e) & VALID != 0 { return Err(Error::AlreadyMapped); }
            core::ptr::write_volatile(e, (pa & ADDR) | attrs);
            core::arch::asm!("dsb ishst", "isb", options(nostack));
        }
        Ok(())
    }

    /// Unmap the page at `va` and drop it from every core's TLB (Linux
    /// `flush_tlb_kernel_range`); returns the frame it mapped.
    pub fn unmap(va: u64) -> Result<u64, Error> {
        let _g = REMAP.lock();
        let e = walk(va, false)?;
        // SAFETY: as in `map`; the TLBI is broadcast to the inner
        // shareable domain, all cores.
        unsafe {
            let d = core::ptr::read_volatile(e);
            if d & VALID == 0 { return Err(Error::NotMapped); }
            core::ptr::write_volatile(e, 0);
            core::arch::asm!("dsb ishst", options(nostack));
            if crate::arch::exception_level() == 2 {
                core::arch::asm!("tlbi vae2is, {}", in(reg) (va >> 12) & 0xfff_ffff_ffff, options(nostack));
            } else {
                core::arch::asm!("tlbi vaae1is, {}", in(reg) (va >> 12) & 0xfff_ffff_ffff, options(nostack));
            }
            core::arch::asm!("dsb ish", "isb", options(nostack));
            Ok(d & ADDR)
        }
    }

    /// The frame and offset `va` maps to.
    pub fn translate(va: u64) -> Option<u64> {
        let _g = REMAP.lock();
        let e = walk(va, false).ok()?;
        // SAFETY: as in `map`.
        let d = unsafe { core::ptr::read_volatile(e) };
        (d & VALID != 0).then_some((d & ADDR) | (va & 0xfff))
    }
}

/// Why a device window cannot be used.
#[derive(Clone, Copy, Debug)]
pub enum MapError {
    /// Above the identity map.
    OutOfRange,
    /// The range holds RAM, which is no device window.
    Ram,
}

impl core::fmt::Display for MapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MapError::OutOfRange => f.write_str("above the identity map"),
            MapError::Ram => f.write_str("RAM, not a device window"),
        }
    }
}

/// Is `[phys, phys + len)` a device window the identity map covers as
/// Device memory?
pub fn device_window(phys: u64, len: u64) -> Result<(), MapError> {
    let end = phys.checked_add(len).ok_or(MapError::OutOfRange)?;
    if end > IDENTITY_MAX { return Err(MapError::OutOfRange); }
    if holds_ram(phys, end) { return Err(MapError::Ram); }
    Ok(())
}
