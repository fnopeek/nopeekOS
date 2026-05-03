//! Virtual Memory Manager
//!
//! 4-level x86_64 paging: PML4 → PDPT → PDT → PT → 4KB page.
//! Works alongside boot.s 2MB identity mapping.
//! Preparation for WASM sandbox memory isolation.
#![allow(dead_code)]

use bitflags::bitflags;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::memory;
use crate::kprintln;

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
    // SAFETY: Existing entries have bit 63 = 0, so enabling NXE changes nothing
    // for current mappings. Required for future NO_EXECUTE support.
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdmsr", in("ecx") 0xC000_0080u32, out("eax") lo, out("edx") hi);
        let efer = ((hi as u64) << 32) | (lo as u64);
        let efer = efer | (1 << 11);
        core::arch::asm!("wrmsr",
            in("ecx") 0xC000_0080u32,
            in("eax") efer as u32,
            in("edx") (efer >> 32) as u32);
    }

    // Program PAT MSR (0x277) to add Write-Combining on index 5.
    // Default PAT: 0=WB, 1=WT, 2=UC-, 3=UC, 4=WB, 5=WT, 6=UC-, 7=UC
    // We change: index 5 from WT(0x04) to WC(0x01)
    // PAT MSR layout: each index is 8 bits, indices 0-3 in low dword, 4-7 in high dword.
    // SAFETY: Only changes PAT entry 5 which is unused (no existing page uses PWT+PAT).
    unsafe {
        let pat_lo: u32 = 0x00070406; // [3]=UC(0x00) [2]=UC-(0x07) [1]=WT(0x04) [0]=WB(0x06)
        let pat_hi: u32 = 0x00070106; // [7]=UC(0x00) [6]=UC-(0x07) [5]=WC(0x01) [4]=WB(0x06)
        core::arch::asm!("wrmsr",
            in("ecx") 0x277u32,
            in("eax") pat_lo,
            in("edx") pat_hi);
    }

    let pml4 = read_cr3();
    PML4_PHYS.store(pml4, Ordering::Relaxed);

    let (huge_pages, small_pages) = count_mappings(pml4);
    let mapped_mb = huge_pages * 2 + (small_pages * 4) / 1024;

    let mapped_gb = mapped_mb / 1024;
    if mapped_gb > 0 {
        kprintln!("[npk] Paging: {} GB identity-mapped, NX enabled", mapped_gb);
    } else {
        kprintln!("[npk] Paging: {} MB mapped, NX enabled", mapped_mb);
    }
}

/// Map a 4KB virtual page to a physical frame.
/// Automatically splits 1GB and 2MB huge pages when a different mapping
/// (e.g. NO_CACHE for MMIO) is needed at 4KB granularity.
pub fn map_page(vaddr: u64, paddr: u64, flags: PageFlags) -> Result<(), PagingError> {
    if vaddr & 0xFFF != 0 || paddr & 0xFFF != 0 {
        return Err(PagingError::NotAligned);
    }

    let pml4 = PML4_PHYS.load(Ordering::Relaxed);

    // SAFETY: All table accesses are within identity-mapped range
    unsafe {
        let pdpt = get_or_create(pml4, pml4_index(vaddr))?;
        let pdpt_entry = read_entry(pdpt, pdpt_index(vaddr));

        // 1GB huge page covers this address — split into 512 × 2MB pages
        if pdpt_entry & PageFlags::PRESENT.bits() != 0 && pdpt_entry & PageFlags::HUGE.bits() != 0 {
            split_1gb_to_2mb(pdpt, pdpt_index(vaddr))?;
            flush_tlb_all(); // full TLB flush after huge page split
        }

        let pdt = get_or_create(pdpt, pdpt_index(vaddr))?;
        let pdt_entry = read_entry(pdt, pdt_index(vaddr));

        // 2MB huge page covers this address — split into 512 × 4KB pages
        if pdt_entry & PageFlags::PRESENT.bits() != 0 && pdt_entry & PageFlags::HUGE.bits() != 0 {
            split_2mb_to_4kb(pdt, pdt_index(vaddr))?;
            flush_tlb_all(); // full TLB flush after huge page split
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
