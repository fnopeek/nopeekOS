//! Virtual Memory Manager
//!
//! 4-level x86_64 paging: PML4 → PDPT → PDT → PT → 4KB page.
//! `init` replaces the firmware tables with our own 64 GB identity map.
#![allow(dead_code)]

use bitflags::bitflags;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::hw::msr;
use crate::memory;

const IA32_EFER: u32 = 0xC000_0080;
const IA32_PAT: u32 = 0x277;
const ENTRY_COUNT: usize = 512;
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

static PML4_PHYS: AtomicU64 = AtomicU64::new(0);

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PageFlags: u64 {
        const PRESENT       = 1 << 0;
        const WRITABLE      = 1 << 1;
        const USER          = 1 << 2;
        const WRITE_THROUGH = 1 << 3;  // PWT — also selects PAT index bit 0
        const NO_CACHE      = 1 << 4;  // PCD — also selects PAT index bit 1
        const ACCESSED      = 1 << 5;
        const DIRTY         = 1 << 6;
        const HUGE          = 1 << 7;  // PS bit (PDT/PDPT: huge page; PT: PAT index bit 2)
        const GLOBAL        = 1 << 8;
        const NO_EXECUTE    = 1 << 63;

        /// Write-Combining: PAT index 5 = PWT(1) + PCD(0) + PAT(1)
        /// PAT bit for 4KB PTEs is bit 7 (same position as HUGE, but used at PT level).
        /// Requires PAT MSR to have WC at index 5.
        const WRITE_COMBINE = (1 << 3) | (1 << 7);  // PWT + PAT
    }
}

#[derive(Debug)]
pub enum PagingError {
    NotAligned,
    AlreadyMapped,
    NotMapped,
    HugePageConflict,
    FrameAllocationFailed,
}

impl core::fmt::Display for PagingError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            PagingError::NotAligned => write!(f, "address not page-aligned"),
            PagingError::AlreadyMapped => write!(f, "page already mapped"),
            PagingError::NotMapped => write!(f, "page not mapped"),
            PagingError::HugePageConflict => write!(f, "conflicts with 2MB huge page"),
            PagingError::FrameAllocationFailed => write!(f, "frame allocation failed"),
        }
    }
}

// === Address decomposition ===

fn pml4_index(vaddr: u64) -> usize { ((vaddr >> 39) & 0x1FF) as usize }
fn pdpt_index(vaddr: u64) -> usize { ((vaddr >> 30) & 0x1FF) as usize }
fn pdt_index(vaddr: u64) -> usize  { ((vaddr >> 21) & 0x1FF) as usize }
fn pt_index(vaddr: u64) -> usize   { ((vaddr >> 12) & 0x1FF) as usize }

fn entry_addr(entry: u64) -> u64 { entry & ADDR_MASK }
fn entry_flags(entry: u64) -> PageFlags { PageFlags::from_bits_truncate(entry) }

// === CR3 / TLB ===

fn read_cr3() -> u64 {
    let cr3: u64;
    // SAFETY: Reading CR3 is side-effect-free
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3); }
    cr3 & ADDR_MASK
}

fn flush_tlb(vaddr: u64) {
    // SAFETY: invlpg only invalidates the single TLB entry
    unsafe { core::arch::asm!("invlpg [{}]", in(reg) vaddr); }
}

/// Full TLB flush by reloading CR3. Required after splitting huge pages
/// because invlpg on a single address doesn't cover the entire old huge page.
fn flush_tlb_all() {
    // SAFETY: Reloading CR3 with same value flushes all non-global TLB entries
    unsafe {
        let cr3: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3);
        core::arch::asm!("mov cr3, {}", in(reg) cr3);
    }
}

/// Bumped after every change to an entry that may already sit in another
/// core's TLB (huge-page split, flag change, unmap). All cores share one
/// PML4 and `invlpg` / a CR3 reload act only locally.
///
/// Not a synchronous shootdown: workers run with IF=0 outside their idle
/// halt, so an IPI would be answered only there. Instead every core flushes
/// at its next scheduling point (`sync_tlb`), before it starts the next fiber
/// or task. The core that made the change flushes at once; a fiber only ever
/// touches memory mapped for it, on the core it is pinned to.
static TLB_GEN: AtomicU64 = AtomicU64::new(0);
static TLB_SEEN: [AtomicU64; 256] =
    [const { AtomicU64::new(0) }; 256];

fn note_shared_change() {
    TLB_GEN.fetch_add(1, Ordering::Release);
}

/// Flush this core's TLB if another core changed a live entry since it last
/// looked. One atomic load when nothing changed.
pub fn sync_tlb() {
    let cid = crate::smp::per_core::current_core_id();
    if cid >= TLB_SEEN.len() { return; }
    let g = TLB_GEN.load(Ordering::Acquire);
    if TLB_SEEN[cid].load(Ordering::Relaxed) != g {
        flush_tlb_all();
        TLB_SEEN[cid].store(g, Ordering::Relaxed);
    }
}

/// Split a 1GB huge page (PDPT entry) into 512 × 2MB huge pages (PDT).
/// Preserves the original identity mapping with the same base flags.
/// Returns the physical address of the new PDT.
/// SAFETY: pdpt must be valid, index must point to a 1GB huge page entry.
unsafe fn split_1gb_to_2mb(pdpt: u64, index: usize) -> Result<u64, PagingError> {
    unsafe {
        let entry = read_entry(pdpt, index);
        let huge_base = entry_addr(entry); // 1GB-aligned physical base
        let base_flags = (entry & 0xFF) | PageFlags::HUGE.bits();

        let pdt = memory::allocate_frame().ok_or(PagingError::FrameAllocationFailed)?;
        zero_frame(pdt);

        for i in 0..ENTRY_COUNT {
            let phys = huge_base + (i as u64) * (2 * 1024 * 1024);
            write_entry(pdt, i, phys | base_flags);
        }

        let new_entry = pdt | PageFlags::PRESENT.bits() | PageFlags::WRITABLE.bits();
        write_entry(pdpt, index, new_entry);

        Ok(pdt)
    }
}

/// Split a 2MB huge page (PDT entry) into 512 × 4KB pages (PT).
/// Preserves the original identity mapping with the same base flags.
/// Returns the physical address of the new PT.
/// SAFETY: pdt must be valid, index must point to a 2MB huge page entry.
unsafe fn split_2mb_to_4kb(pdt: u64, index: usize) -> Result<u64, PagingError> {
    unsafe {
        let entry = read_entry(pdt, index);
        let huge_base = entry_addr(entry);
        // bit 7 at PT level is PAT, not HUGE — strip it from base flags
        let base_flags = entry & 0x67;

        let pt = memory::allocate_frame().ok_or(PagingError::FrameAllocationFailed)?;
        zero_frame(pt);

        for i in 0..ENTRY_COUNT {
            let phys = huge_base + (i as u64) * 4096;
            write_entry(pt, i, phys | base_flags);
        }

        let new_entry = pt | PageFlags::PRESENT.bits() | PageFlags::WRITABLE.bits();
        write_entry(pdt, index, new_entry);

        Ok(pt)
    }
}

// === Table access (identity-mapped) ===

/// Read a page table entry. SAFETY: table_phys must be in identity-mapped range.
unsafe fn read_entry(table_phys: u64, index: usize) -> u64 {
    let ptr = table_phys as *const u64;
    unsafe { *ptr.add(index) }
}

/// Write a page table entry. SAFETY: table_phys must be in identity-mapped range.
unsafe fn write_entry(table_phys: u64, index: usize, value: u64) {
    let ptr = table_phys as *mut u64;
    unsafe { *ptr.add(index) = value; }
}

/// Zero a freshly allocated 4KB frame for use as a page table.
unsafe fn zero_frame(addr: u64) {
    unsafe { core::ptr::write_bytes(addr as *mut u8, 0, memory::PAGE_SIZE); }
}

/// Walk to the next table level. If not present, allocate a new table.
unsafe fn get_or_create(table_phys: u64, index: usize) -> Result<u64, PagingError> {
    unsafe {
        let entry = read_entry(table_phys, index);

        if entry & PageFlags::PRESENT.bits() != 0 {
            Ok(entry_addr(entry))
        } else {
            let frame = memory::allocate_frame()
                .ok_or(PagingError::FrameAllocationFailed)?;
            zero_frame(frame);
            write_entry(table_phys, index, frame | PageFlags::PRESENT.bits() | PageFlags::WRITABLE.bits());
            Ok(frame)
        }
    }
}

// === Public API ===

pub fn init() {
    // Enable NXE (bit 11) in EFER MSR so NO_EXECUTE flag works in page tables.
    // SAFETY: IA32_EFER exists on every x86_64 CPU; setting NXE only makes
    // the NX bit in page tables meaningful, and no entry uses it yet.
    unsafe {
        let efer = msr::read(IA32_EFER);
        msr::write(IA32_EFER, efer | (1 << 11));
    }

    // Program PAT MSR (0x277) to add Write-Combining on index 5.
    // SAFETY: IA32_PAT exists on every x86_64 CPU. Against the reset value
    // only index 5 changes (WT to WC), and no mapping selects it yet.
    unsafe { msr::write(IA32_PAT, 0x0007_0106_0007_0406) };

    // Build our own page tables. UEFI handed us long mode running on
    // its own PML4 — those tables are in firmware-owned memory marked
    // read-only, so we can't add new entries to them later (e.g. for
    // MMIO regions or WASM module memory). We allocate fresh
    // tables from our frame allocator and identity-map 64 GB via
    // 1 GB huge pages.
    let pml4_phys = memory::allocate_frame()
        .expect("paging::init: failed to allocate PML4 frame");
    let pdpt_phys = memory::allocate_frame()
        .expect("paging::init: failed to allocate PDPT frame");
    unsafe {
        // Zero both tables.
        core::ptr::write_bytes(pml4_phys as *mut u8, 0, 4096);
        core::ptr::write_bytes(pdpt_phys as *mut u8, 0, 4096);

        // PML4[0] → PDPT, Present + Writable.
        let pml4 = pml4_phys as *mut u64;
        *pml4 = pdpt_phys | 0x03;

        // PDPT: 64 entries, each a 1 GB huge page identity mapping.
        // 0x83 = Present (bit 0) + Writable (bit 1) + Huge (bit 7).
        let pdpt = pdpt_phys as *mut u64;
        for i in 0..64u64 {
            *pdpt.add(i as usize) = (i << 30) | 0x83;
        }
    }
    PML4_PHYS.store(pml4_phys, Ordering::Relaxed);

    // Switch CR3 to our new tables. Any TLB entries from UEFI's
    // mapping get flushed. From here on, MMIO mappings can be added
    // freely (map_page will modify our own tables, which are writable).
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) pml4_phys);
    }

    crate::kdebug!("[npk] Paging: 64 GB identity-mapped, NX enabled (own PML4 @ {:#x})", pml4_phys);
}

/// End of the identity mapping (see `init`: 64 GB).
pub const IDENTITY_LIMIT: u64 = 64 * 1024 * 1024 * 1024;

/// Read one byte at a physical address (identity-mapped, so physical equals
/// virtual). Meant for firmware windows such as ACPI SystemMemory regions.
/// The caller must already have checked that the address is not RAM; this
/// only checks the mapping limit.
///
/// Reads go through the normal cached mapping, which is fine for firmware
/// status bytes. Not implemented: an uncached mapping for registers that
/// change on their own.
pub fn read_phys_u8(addr: u64) -> Option<u8> {
    if addr >= IDENTITY_LIMIT { return None; }
    // SAFETY: inside the identity mapping, a single byte, read-only.
    Some(unsafe { core::ptr::read_volatile(addr as *const u8) })
}

/// Serialises every change to the page tables.
///
/// All cores share one set of tables, and forge maps from whichever worker
/// runs the module (`memory.grow`, `Code::map`). `get_or_create` reads an
/// empty slot, allocates a table and writes it — two cores doing that on the
/// same slot each install their own table, and the mappings made through the
/// loser vanish. Code of different modules shares page tables because the
/// code region grows linearly.
///
/// Never taken from interrupt context; nothing under it maps again.
static PT_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// Map a 4KB virtual page to a physical frame.
/// Automatically splits 1GB and 2MB huge pages when a different mapping
/// (e.g. NO_CACHE for MMIO) is needed at 4KB granularity.
pub fn map_page(vaddr: u64, paddr: u64, flags: PageFlags) -> Result<(), PagingError> {
    if vaddr & 0xFFF != 0 || paddr & 0xFFF != 0 {
        return Err(PagingError::NotAligned);
    }
    let _pt = PT_LOCK.lock();

    let pml4 = PML4_PHYS.load(Ordering::Relaxed);

    // SAFETY: All table accesses are within identity-mapped range
    unsafe {
        let pdpt = get_or_create(pml4, pml4_index(vaddr))?;
        let pdpt_entry = read_entry(pdpt, pdpt_index(vaddr));

        // 1GB huge page covers this address — split into 512 × 2MB pages
        if pdpt_entry & PageFlags::PRESENT.bits() != 0 && pdpt_entry & PageFlags::HUGE.bits() != 0 {
            split_1gb_to_2mb(pdpt, pdpt_index(vaddr))?;
            flush_tlb_all(); // full TLB flush after huge page split
            note_shared_change();
        }

        let pdt = get_or_create(pdpt, pdpt_index(vaddr))?;
        let pdt_entry = read_entry(pdt, pdt_index(vaddr));

        // 2MB huge page covers this address — split into 512 × 4KB pages
        if pdt_entry & PageFlags::PRESENT.bits() != 0 && pdt_entry & PageFlags::HUGE.bits() != 0 {
            split_2mb_to_4kb(pdt, pdt_index(vaddr))?;
            flush_tlb_all(); // full TLB flush after huge page split
            note_shared_change();
        }

        let pt = get_or_create(pdt, pdt_index(vaddr))?;
        let pt_entry = read_entry(pt, pt_index(vaddr));

        if pt_entry & PageFlags::PRESENT.bits() != 0 {
            // Page already mapped at 4KB level — update flags if different
            let old_paddr = entry_addr(pt_entry);
            if old_paddr == paddr {
                // Same physical address — just update flags (e.g. add NO_CACHE)
                write_entry(pt, pt_index(vaddr), paddr | flags.bits());
                flush_tlb(vaddr);
                note_shared_change();
                return Ok(());
            }
            return Err(PagingError::AlreadyMapped);
        }

        write_entry(pt, pt_index(vaddr), paddr | flags.bits());
    }

    flush_tlb(vaddr);
    Ok(())
}

/// Unmap a 4KB page. Returns the physical address that was mapped.
pub fn unmap_page(vaddr: u64) -> Result<u64, PagingError> {
    if vaddr & 0xFFF != 0 {
        return Err(PagingError::NotAligned);
    }
    let _pt = PT_LOCK.lock();

    let pml4 = PML4_PHYS.load(Ordering::Relaxed);

    // SAFETY: All table accesses within identity-mapped range
    unsafe {
        let pml4_entry = read_entry(pml4, pml4_index(vaddr));
        if pml4_entry & PageFlags::PRESENT.bits() == 0 { return Err(PagingError::NotMapped); }

        let pdpt = entry_addr(pml4_entry);
        let pdpt_entry = read_entry(pdpt, pdpt_index(vaddr));
        if pdpt_entry & PageFlags::PRESENT.bits() == 0 { return Err(PagingError::NotMapped); }
        if pdpt_entry & PageFlags::HUGE.bits() != 0 { return Err(PagingError::HugePageConflict); }

        let pdt = entry_addr(pdpt_entry);
        let pdt_entry = read_entry(pdt, pdt_index(vaddr));
        if pdt_entry & PageFlags::PRESENT.bits() == 0 { return Err(PagingError::NotMapped); }
        if pdt_entry & PageFlags::HUGE.bits() != 0 { return Err(PagingError::HugePageConflict); }

        let pt = entry_addr(pdt_entry);
        let pt_entry = read_entry(pt, pt_index(vaddr));
        if pt_entry & PageFlags::PRESENT.bits() == 0 { return Err(PagingError::NotMapped); }

        let paddr = entry_addr(pt_entry);
        write_entry(pt, pt_index(vaddr), 0);
        flush_tlb(vaddr);
        note_shared_change();
        Ok(paddr)
    }
}

/// Translate a virtual address to physical (handles both 2MB and 4KB pages)
pub fn translate(vaddr: u64) -> Option<u64> {
    let pml4 = PML4_PHYS.load(Ordering::Relaxed);

    // SAFETY: Read-only table walk within identity-mapped range
    unsafe {
        let pml4_e = read_entry(pml4, pml4_index(vaddr));
        if pml4_e & PageFlags::PRESENT.bits() == 0 { return None; }

        let pdpt_e = read_entry(entry_addr(pml4_e), pdpt_index(vaddr));
        if pdpt_e & PageFlags::PRESENT.bits() == 0 { return None; }
        if pdpt_e & PageFlags::HUGE.bits() != 0 {
            return Some(entry_addr(pdpt_e) + (vaddr & 0x3FFF_FFFF)); // 1GB page
        }

        let pdt_e = read_entry(entry_addr(pdpt_e), pdt_index(vaddr));
        if pdt_e & PageFlags::PRESENT.bits() == 0 { return None; }
        if pdt_e & PageFlags::HUGE.bits() != 0 {
            return Some(entry_addr(pdt_e) + (vaddr & 0x1F_FFFF)); // 2MB page
        }

        let pt_e = read_entry(entry_addr(pdt_e), pt_index(vaddr));
        if pt_e & PageFlags::PRESENT.bits() == 0 { return None; }
        Some(entry_addr(pt_e) + (vaddr & 0xFFF)) // 4KB page
    }
}

/// Return the raw leaf paging entry (with flag bits) mapping `vaddr`, plus
/// the page level: 1 = 1 GB, 2 = 2 MB, 3 = 4 KB. `None` if not present.
/// Read-only — lets a diagnostic decode the PAT/PCD/PWT memory-type bits.
pub fn leaf_entry(vaddr: u64) -> Option<(u64, u8)> {
    let pml4 = PML4_PHYS.load(Ordering::Relaxed);
    // SAFETY: read-only table walk within identity-mapped range.
    unsafe {
        let pml4_e = read_entry(pml4, pml4_index(vaddr));
        if pml4_e & PageFlags::PRESENT.bits() == 0 { return None; }
        let pdpt_e = read_entry(entry_addr(pml4_e), pdpt_index(vaddr));
        if pdpt_e & PageFlags::PRESENT.bits() == 0 { return None; }
        if pdpt_e & PageFlags::HUGE.bits() != 0 { return Some((pdpt_e, 1)); }
        let pdt_e = read_entry(entry_addr(pdpt_e), pdt_index(vaddr));
        if pdt_e & PageFlags::PRESENT.bits() == 0 { return None; }
        if pdt_e & PageFlags::HUGE.bits() != 0 { return Some((pdt_e, 2)); }
        let pt_e = read_entry(entry_addr(pdt_e), pt_index(vaddr));
        if pt_e & PageFlags::PRESENT.bits() == 0 { return None; }
        Some((pt_e, 3))
    }
}

/// Count mapped pages: (huge_2mb, small_4kb)
fn count_mappings(pml4: u64) -> (usize, usize) {
    let mut huge = 0;
    let mut small = 0;

    for i in 0..ENTRY_COUNT {
        // SAFETY: PML4 is in identity-mapped range
        let pml4_e = unsafe { read_entry(pml4, i) };
        if pml4_e & PageFlags::PRESENT.bits() == 0 { continue; }

        let pdpt = entry_addr(pml4_e);
        for j in 0..ENTRY_COUNT {
            let pdpt_e = unsafe { read_entry(pdpt, j) };
            if pdpt_e & PageFlags::PRESENT.bits() == 0 { continue; }
            if pdpt_e & PageFlags::HUGE.bits() != 0 { huge += 512; continue; } // 1GB = 512 x 2MB

            let pdt = entry_addr(pdpt_e);
            for k in 0..ENTRY_COUNT {
                let pdt_e = unsafe { read_entry(pdt, k) };
                if pdt_e & PageFlags::PRESENT.bits() == 0 { continue; }
                if pdt_e & PageFlags::HUGE.bits() != 0 { huge += 1; continue; }

                let pt = entry_addr(pdt_e);
                for l in 0..ENTRY_COUNT {
                    let pt_e = unsafe { read_entry(pt, l) };
                    if pt_e & PageFlags::PRESENT.bits() != 0 { small += 1; }
                }
            }
        }
    }

    (huge, small)
}

/// Stats for status intent: (huge_2mb_count, small_4kb_count)
pub fn stats() -> (usize, usize) {
    let pml4 = PML4_PHYS.load(Ordering::Relaxed);
    if pml4 == 0 { return (0, 0); }
    count_mappings(pml4)
}
