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
//! - Guest physical RAM is identity-mapped through our NPT/EPT, so
//!   `host_virt = npt_host_base + guest_phys` for any guest physical
//!   inside the RAM window.
//!
//! No support for 5-level paging (LA57) — Alpine's vmlinuz-virt 6.18
//! doesn't enable it; we'd need to detect CR4.LA57 and add a PML5 walk.
//!
//! `GUEST_RAM_BYTES` MUST equal the EPT/NPT-mapped window — the twin
//! of `guest_mem::GUEST_RAM_BYTES`. This was the 5th of five places
//! the guest-RAM size is encoded and (with guest_mem) one the
//! 256 MB→1 GB bump missed: a stale 256 MB here made the MMIO
//! instruction-fetch page-walk reject any guest page-table / rip
//! resolving above 256 MB (`read_phys_u64`/`read_15_bytes` → None →
//! "insn fetch failed" → NPF). Harmless until a guest with > 256 MB
//! of page tables in use (LibreWolf, cr3 ~600 MB) hit it. Keep in
//! sync with: `guest_mem::GUEST_RAM_BYTES`,
//! `vmx::ept::GUEST_WINDOW_BYTES`, svm npt window,
//! `bzimage::GUEST_RAM_TOTAL`.

#![allow(dead_code)]

/// 1 GiB — see the module-doc sync invariant.
const GUEST_RAM_BYTES: u64 = 1024 * 1024 * 1024;
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Walk guest page tables to translate `rip` (guest virtual) into a
/// guest physical address, then return up to 15 bytes from there.
/// `cr3` is the guest's PML4 base. `npt_host_base` is the start of
/// the host-virtual window into the guest's 256 MB RAM.
///
/// Returns None on any walk failure (page not present, instruction
/// crossing a page boundary, GPA outside our window).
pub fn fetch_inst(
    rip: u64,
    cr3: u64,
    npt_host_base: u64,
) -> Option<[u8; 15]> {
    let pml4_phys = cr3 & ADDR_MASK;

    let pml4_idx = ((rip >> 39) & 0x1FF) as u64;
    let pml4e = read_phys_u64(pml4_phys + pml4_idx * 8, npt_host_base)?;
    if pml4e & 1 == 0 { return None; }

    let pdpt_phys = pml4e & ADDR_MASK;
    let pdpt_idx = ((rip >> 30) & 0x1FF) as u64;
    let pdpte = read_phys_u64(pdpt_phys + pdpt_idx * 8, npt_host_base)?;
    if pdpte & 1 == 0 { return None; }

    // 1 GB page (PS bit at PDPT level)
    if pdpte & 0x80 != 0 {
        let phys = (pdpte & 0x000F_FFFF_C000_0000) | (rip & 0x3FFF_FFFF);
        return read_15_bytes(phys, npt_host_base);
    }

    let pd_phys = pdpte & ADDR_MASK;
    let pd_idx = ((rip >> 21) & 0x1FF) as u64;
    let pde = read_phys_u64(pd_phys + pd_idx * 8, npt_host_base)?;
    if pde & 1 == 0 { return None; }

    // 2 MB page (PS bit at PD level)
    if pde & 0x80 != 0 {
        let phys = (pde & 0x000F_FFFF_FFE0_0000) | (rip & 0x001F_FFFF);
        return read_15_bytes(phys, npt_host_base);
    }

    let pt_phys = pde & ADDR_MASK;
    let pt_idx = ((rip >> 12) & 0x1FF) as u64;
    let pte = read_phys_u64(pt_phys + pt_idx * 8, npt_host_base)?;
    if pte & 1 == 0 { return None; }

    let phys = (pte & ADDR_MASK) | (rip & 0xFFF);
    read_15_bytes(phys, npt_host_base)
}

fn read_phys_u64(guest_phys: u64, npt_host_base: u64) -> Option<u64> {
    if guest_phys + 8 > GUEST_RAM_BYTES { return None; }
    let host_addr = npt_host_base + guest_phys;
    // SAFETY: host kernel identity-maps its 64 GB physical RAM and the
    // MicroVM guest-RAM window lives inside it. The address is valid as
    // long as the guest's CR3 hasn't pointed outside RAM (we bound-
    // check above).
    unsafe { Some(core::ptr::read_volatile(host_addr as *const u64)) }
}

fn read_15_bytes(guest_phys: u64, npt_host_base: u64) -> Option<[u8; 15]> {
    // x86 instructions are at most 15 bytes. If the instruction starts
    // late enough in a page that 15 bytes would cross a 4 KB boundary,
    // we'd need to do a second walk for the next page. Bail for now —
    // Linux's MMIO accessors don't generate such instructions in
    // practice (each is an aligned `mov`, ~3-7 bytes, well within a
    // single page).
    let page_off = guest_phys & 0xFFF;
    if page_off > 0xFF1 { return None; }

    if guest_phys + 15 > GUEST_RAM_BYTES { return None; }
    let host_addr = npt_host_base + guest_phys;
    let mut buf = [0u8; 15];
    // SAFETY: bounds checked.
    unsafe {
        core::ptr::copy_nonoverlapping(
            host_addr as *const u8,
            buf.as_mut_ptr(),
            15,
        );
    }
    Some(buf)
}
