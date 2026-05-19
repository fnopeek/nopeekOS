//! Fetch guest instruction bytes for MMIO emulation.
//!
//! When SVM decode-assists is unavailable (notably nested SVM under
//! KVM doesn't populate the GUEST_INST_BYTES VMCB fields for #NPF),
//! we walk the guest's own page tables to read the faulting instruction.
//!
//! Same code works for VMX, where decode-assists doesn't exist at all.
//!
//! Assumptions (valid for any Linux ≥ 5.x in our MicroVM):
//! - 4-level long-mode paging (CR4.PAE=1, EFER.LME=1, CR0.PG=1)
//! - Guest page tables live in guest RAM, read via `GuestMem`. The
//!   walk is gpa-based; `GuestMem` owns gpa→host translation + bounds
//!   (B3: scattered/demand-paged — including the guest PT pages this
//!   walk itself reads).
//!
//! No support for 5-level paging (LA57) — Alpine's vmlinuz-virt 6.18
//! doesn't enable it; we'd need to detect CR4.LA57 and add a PML5 walk.

#![allow(dead_code)]

use super::guest_mem::GuestMem;

const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Walk guest page tables to translate `rip` (guest virtual) into a
/// guest physical address, then return up to 15 bytes from there.
/// `cr3` is the guest's PML4 base. `mem` owns gpa→host translation.
///
/// Returns None on any walk failure (page not present, instruction
/// crossing a page boundary, GPA outside the window).
pub fn fetch_inst(
    rip: u64,
    cr3: u64,
    mem: &GuestMem,
) -> Option<[u8; 15]> {
    let pml4_phys = cr3 & ADDR_MASK;

    let pml4_idx = ((rip >> 39) & 0x1FF) as u64;
    let pml4e = mem.read_u64(pml4_phys + pml4_idx * 8)?;
    if pml4e & 1 == 0 { return None; }

    let pdpt_phys = pml4e & ADDR_MASK;
    let pdpt_idx = ((rip >> 30) & 0x1FF) as u64;
    let pdpte = mem.read_u64(pdpt_phys + pdpt_idx * 8)?;
    if pdpte & 1 == 0 { return None; }

    // 1 GB page (PS bit at PDPT level)
    if pdpte & 0x80 != 0 {
        let phys = (pdpte & 0x000F_FFFF_C000_0000) | (rip & 0x3FFF_FFFF);
        return read_15_bytes(phys, mem);
    }

    let pd_phys = pdpte & ADDR_MASK;
    let pd_idx = ((rip >> 21) & 0x1FF) as u64;
    let pde = mem.read_u64(pd_phys + pd_idx * 8)?;
    if pde & 1 == 0 { return None; }

    // 2 MB page (PS bit at PD level)
    if pde & 0x80 != 0 {
        let phys = (pde & 0x000F_FFFF_FFE0_0000) | (rip & 0x001F_FFFF);
        return read_15_bytes(phys, mem);
    }

    let pt_phys = pde & ADDR_MASK;
    let pt_idx = ((rip >> 12) & 0x1FF) as u64;
    let pte = mem.read_u64(pt_phys + pt_idx * 8)?;
    if pte & 1 == 0 { return None; }

    let phys = (pte & ADDR_MASK) | (rip & 0xFFF);
    read_15_bytes(phys, mem)
}

fn read_15_bytes(guest_phys: u64, mem: &GuestMem) -> Option<[u8; 15]> {
    // x86 instructions are at most 15 bytes. If the instruction starts
    // late enough in a page that 15 bytes would cross a 4 KB boundary,
    // we'd need to do a second walk for the next page. Bail for now —
    // Linux's MMIO accessors don't generate such instructions in
    // practice (each is an aligned `mov`, ~3-7 bytes, well within a
    // single page). This single-page guarantee also keeps the
    // `GuestMem` read within one (B3) demand-paged frame.
    let page_off = guest_phys & 0xFFF;
    if page_off > 0xFF1 { return None; }

    let mut buf = [0u8; 15];
    if !mem.read_bytes(guest_phys, &mut buf) { return None; }
    Some(buf)
}
