//! Extended Page Tables (EPT).
//!
//! Maps guest-physical RAM onto host-physical memory at an offset
//! (non-identity), so the guest can place Linux at guest-phys 0x10000 /
//! 0x100000 without touching the host kernel image at the same host
//! addresses. The low boot window is one contiguous block of 2-MB leaves;
//! the rest of guest RAM is demand-paged on EPT violation, a 2-MB leaf per
//! first-touched block where the host has an aligned 2-MB block free and
//! 4-KB pages otherwise (as `svm::npt`). The high
//! MMIO range [3 GiB, 4 GiB) has its own PD (scratch page, trapped
//! LAPIC/IOAPIC pages).
//!
//! Reference: Intel SDM Vol. 3C §28.2 (EPT Mechanism), Vol. 3D
//! Appendix A.10 (VPID and EPT Capabilities).

use crate::mm::memory;

// EPT entry permission + attribute bits.
const EPT_R: u64 = 1 << 0;
const EPT_W: u64 = 1 << 1;
const EPT_X: u64 = 1 << 2;
const EPT_RWX: u64 = EPT_R | EPT_W | EPT_X;
// Memory type for leaf entries: 6 = WB (write-back).
const EPT_MEM_TYPE_WB: u64 = 6 << 3;
// Bit 7: leaf entry (page) vs pointer to next-level table.
const EPT_LEAF: u64 = 1 << 7;

// EPTP fields VMWRITE'd into VMCS::EPT_POINTER.
const EPTP_MEM_TYPE_WB: u64 = 6;
const EPTP_WALK_LENGTH_4: u64 = 3 << 3; // 4 levels = walk length 3

const TWO_MB: u64 = 2 * 1024 * 1024;
const ONE_GB: u64 = 1024 * 1024 * 1024;
/// Maximum guest RAM we can map: PDPT slots [0, 1, 2] each cover 1 GiB
/// of guest physical, PDPT[3] is reserved for the MMIO scratch range
/// (IOAPIC/HPET/LAPIC). 3 GiB hard cap; bumping past that needs either
/// rearranging the MMIO hole or growing the EPT to a second PDPT.
const MAX_GUEST_BYTES: u64 = 3 * ONE_GB;
// Canonical size lives in `guest_mem`; this is its EPT-window twin.
/// Slack frames `boot_frames_for` adds so `allocate_contiguous` can be
/// rounded up to a 2-MB boundary for the boot-window 2-MB leaves.
const GUEST_RAM_ALIGN_SLACK: usize = 511;

/// Bits [51:12] of an EPT entry hold the next-level / page phys addr.
const EPT_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
/// Bits [51:21] of a 2-MB leaf.
const EPT_2M_ADDR_MASK: u64 = 0x000F_FFFF_FFE0_0000;
const FRAMES_PER_2M: usize = (TWO_MB / 4096) as usize;

/// Demand blocks mapped as one 2-MB leaf, and as a PT of 4-KB pages.
pub static DEMAND_2M: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static DEMAND_4K_BLOCKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Hybrid split: `[0, BOOT_WINDOW_BYTES)` is one contiguous 2-MB-leaf
/// block (fast boot, no faults on the early path, covers the kernel image
/// + initramfs @ 0x0C00_0000 + boot_params); everything above is
/// demand-paged 4 KB on EPT violation. 256 MiB stays allocatable on a
/// fragmented host, where a 1 GiB contiguous block may not; the lazy
/// region is the page cache / browser heap, the bulk on a ≥1 GiB guest.
pub const BOOT_WINDOW_BYTES: u64 = 256 * 1024 * 1024;

/// Round a raw `allocate_contiguous` base up to the next 2-MB
/// boundary so it can be passed to `install_window`.
pub fn round_up_to_2mb(raw_base: u64) -> u64 {
    (raw_base + (TWO_MB - 1)) & !(TWO_MB - 1)
}

/// Contiguous boot window size for a guest of `guest_bytes`
/// (= `min(BOOT_WINDOW_BYTES, guest_bytes)`, 2-MB-aligned). A guest
/// ≤ 256 MiB is fully contiguous → no demand region.
pub fn boot_window_bytes(guest_bytes: u64) -> u64 {
    if crate::microvm::devices::guest_mem::DEMAND_ENABLED {
        guest_bytes.min(BOOT_WINDOW_BYTES)
    } else {
        // Demand off → whole guest contiguous (boot_leaves ==
        // guest_leaves, no demand PTs).
        guest_bytes
    }
}

/// 4 KB host frames to allocate contiguously for the boot window
/// (+ 2-MB-align slack). The demand region needs no upfront
/// allocation — only touched 4 KB pages are committed on fault.
pub fn boot_frames_for(guest_bytes: u64) -> usize {
    (boot_window_bytes(guest_bytes) / 4096) as usize + GUEST_RAM_ALIGN_SLACK
}

/// Build the EPT. Maps:
///   - guest-physical [0, boot window) → host-physical
///     [boot_base, +boot window) via 2-MB leaf entries;
///   - [boot window, guest_bytes) not present, faulted in on demand
///     (`demand_fault_in`);
///   - guest-physical [0xFEC00000, 0xFF000000) (IOAPIC, HPET, LAPIC) →
///     a 4 KB scratch page via PT-level mapping, except the trapped
///     IOAPIC page and (with LAPIC emulation) the LAPIC page, which are
///     left not-present so accesses EPT-fault into the emulators.
/// Returns `(eptp, pml4_phys)` — the EPTP to VMWRITE into
/// VMCS::EPT_POINTER, and the PML4 phys for `demand_fault_in` /
/// `release`.
pub fn install_window(boot_base: u64, guest_bytes: u64) -> Result<(u64, u64), &'static str> {
    if boot_base & (TWO_MB - 1) != 0 {
        return Err("EPT: boot_base must be 2-MB aligned for 2-MB EPT pages");
    }
    if guest_bytes == 0 || guest_bytes & (TWO_MB - 1) != 0 {
        return Err("EPT: guest_bytes must be a non-zero multiple of 2 MB");
    }
    if guest_bytes > MAX_GUEST_BYTES {
        return Err("EPT: guest_bytes > 3 GiB (PDPT[3] reserved for MMIO)");
    }
    let boot_leaves = boot_window_bytes(guest_bytes) / TWO_MB;
    // One PD covers 1 GiB (512 × 2-MB leaves). Allocate as many PDs
    // as the guest needs, chained off PDPT[0..num_pds).
    let num_pds = ((guest_bytes + ONE_GB - 1) / ONE_GB) as usize;

    // Trap the guest LAPIC page (0xFEE00000) into the emulator (vmx::lapic)
    // by leaving it EPT-not-present. Only when LAPIC emulation is on;
    // otherwise PD_HIGH[503] aliases the scratch PT for the `nolapic` boot
    // and no PT_LAPIC exists. Mirrors svm/npt.rs.
    let lapic_trap = crate::microvm::cpu::GUEST_LAPIC && crate::microvm::cpu::VMX_GUEST_LAPIC;

    // Fixed tables: PML4, PDPT, PD_HIGH, PT_DUMMY, the scratch page, the
    // guest-RAM PDs and (trapping) PT_LAPIC. All or none.
    let nfixed = 5 + num_pds + lapic_trap as usize;
    let mut fixed = [0u64; 9];
    for i in 0..nfixed {
        match memory::allocate_frame() {
            Some(f) => fixed[i] = f,
            None => {
                for &f in &fixed[..i] { memory::deallocate_frame(f); }
                return Err("OOM: EPT tables");
            }
        }
    }
    let (pml4_phys, pdpt_phys, pd_high_phys, pt_dummy_phys, dummy_page_phys) =
        (fixed[0], fixed[1], fixed[2], fixed[3], fixed[4]);
    let pd_physs = &fixed[5..5 + num_pds];
    let pt_lapic_phys = if lapic_trap { fixed[5 + num_pds] } else { pt_dummy_phys };

    // SAFETY: identity-mapped, freshly allocated, exclusive.
    unsafe {
        for &f in &fixed[..nfixed] {
            core::ptr::write_bytes(f as *mut u8, 0, 4096);
        }
        let pml4 = pml4_phys as *mut u64;
        pml4.add(0).write_volatile(pdpt_phys | EPT_RWX);

        let pdpt = pdpt_phys as *mut u64;
        // PDPT[0..num_pds] → guest-RAM PDs (each covers 1 GiB).
        for p in 0..num_pds {
            pdpt.add(p).write_volatile(pd_physs[p] | EPT_RWX);
        }
        // PDPT[3] → PD_HIGH (MMIO scratch lives in [3 GiB, 4 GiB)).
        pdpt.add(3).write_volatile(pd_high_phys | EPT_RWX);

        // PD_HIGH[502] → PT_DUMMY (covers [0xFEC00000, 0xFEE00000):
        // IOAPIC + HPET, scratch). PD_HIGH[503] covers [0xFEE00000,
        // 0xFF000000) = the LAPIC: when emulating, point it at a separate
        // PT_LAPIC whose first page (0xFEE00000) is left not-present so the
        // guest's LAPIC MMIO EPT-faults into vmx::lapic; the rest stays
        // scratch. When not emulating, alias it to PT_DUMMY.
        let pd_high = pd_high_phys as *mut u64;
        pd_high.add(502).write_volatile(pt_dummy_phys | EPT_RWX);
        pd_high.add(503).write_volatile(pt_lapic_phys | EPT_RWX);

        let pt_dummy = pt_dummy_phys as *mut u64;
        for i in 0..512usize {
            pt_dummy
                .add(i)
                .write_volatile(dummy_page_phys | EPT_RWX | EPT_MEM_TYPE_WB);
        }
        // [0]: the I/O APIC page (0xFEC00000) not present → trap and emulate
        // (`devices::ioapic`).
        pt_dummy.add(0).write_volatile(0);

        // PT_LAPIC: entry [0] = the LAPIC MMIO page (0xFEE00000) left
        // not present → guest LAPIC accesses EPT-violate → trap-and-emulate
        // (vmx::lapic). The rest of the 2 MB → dummy scratch (harmless if
        // ever touched).
        if lapic_trap {
            let pt_lapic = pt_lapic_phys as *mut u64;
            for i in 1..512usize {
                pt_lapic
                    .add(i)
                    .write_volatile(dummy_page_phys | EPT_RWX | EPT_MEM_TYPE_WB);
            }
        }

        // Populate each PD: the boot window as 2-MB leaves. The demand region
        // stays not present (the PDs are zeroed); its first touch installs
        // either a 2-MB leaf or a PT (`demand_fault_in`). Either is a
        // not-present -> present change, which needs no INVEPT, so a present
        // entry is never replaced.
        const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512
        for p in 0..num_pds {
            let pd = pd_physs[p] as *mut u64;
            let base_leaf = (p as u64) * LEAVES_PER_PD;
            let end_leaf = ((p as u64 + 1) * LEAVES_PER_PD).min(boot_leaves);
            for leaf in base_leaf..end_leaf {
                let local = (leaf - base_leaf) as usize;
                let host_target = boot_base + leaf * TWO_MB;
                pd.add(local).write_volatile(
                    host_target | EPT_RWX | EPT_MEM_TYPE_WB | EPT_LEAF);
            }
        }
    }

    Ok((pml4_phys | EPTP_WALK_LENGTH_4 | EPTP_MEM_TYPE_WB, pml4_phys))
}

/// Back the 4 KB guest page at `gpa` (demand region) and return its host
/// address. The first touch of a 2 MB block maps the whole block as one
/// 2-MB leaf from a 2-MB-aligned host block (one allocation, one TLB entry
/// for the guest, like a THP-backed KVM memslot); without such a block it
/// falls back to a PT of 4 KB pages. Called from the EPT-violation handler
/// (guest fault) and `GuestMem` (host DMA to an untouched page) —
/// idempotent. `None` outside the demand region or without host memory to
/// spare. Mirrors `svm::npt::demand_fault_in`.
pub fn demand_fault_in(pml4_phys: u64, gpa: u64, guest_bytes: u64) -> Option<u64> {
    // The demand PDEs and those past the end of guest RAM are both empty:
    // only the bounds tell them apart.
    if gpa < boot_window_bytes(guest_bytes) || gpa >= guest_bytes {
        return None;
    }
    // SAFETY: our own freshly-built EPT, identity-mapped tables.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt = (pml4.read_volatile() & EPT_ADDR_MASK) as *const u64;
        // PDPT[0..3] holds the guest-RAM PDs (1 GiB each); pick by gpa.
        let pdpt_idx = (gpa / ONE_GB) as usize;
        if pdpt_idx >= 3 {
            return None; // PDPT[3] is the MMIO hole, never demand-faulted
        }
        let pdpt_entry = pdpt.add(pdpt_idx).read_volatile();
        if pdpt_entry == 0 {
            return None; // PD not present (gpa beyond advertised guest_bytes)
        }
        let pd = (pdpt_entry & EPT_ADDR_MASK) as *mut u64;
        let pd_idx = ((gpa % ONE_GB) / TWO_MB) as usize;
        let in_block = gpa % TWO_MB & !0xFFF;
        let pde = pd.add(pd_idx).read_volatile();
        if pde & EPT_R != 0 && pde & EPT_LEAF != 0 {
            return Some((pde & EPT_2M_ADDR_MASK) + in_block); // a 2-MB leaf
        }
        let pt = if pde & EPT_R != 0 {
            (pde & EPT_ADDR_MASK) as *mut u64
        } else {
            let spare = crate::microvm::devices::guest_mem::frames_spare();
            if spare > FRAMES_PER_2M {
                if let Some(block) = memory::allocate_contiguous_aligned(FRAMES_PER_2M, FRAMES_PER_2M) {
                    core::ptr::write_bytes(block as *mut u8, 0, TWO_MB as usize);
                    pd.add(pd_idx).write_volatile(block | EPT_RWX | EPT_MEM_TYPE_WB | EPT_LEAF);
                    DEMAND_2M.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    return Some(block + in_block);
                }
            }
            if spare < 2 {
                return None;
            }
            let pt = memory::allocate_frame()?;
            core::ptr::write_bytes(pt as *mut u8, 0, 4096);
            pd.add(pd_idx).write_volatile(pt | EPT_RWX);
            DEMAND_4K_BLOCKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            pt as *mut u64
        };
        let pt_idx = (in_block / 4096) as usize;
        let pte = pt.add(pt_idx).read_volatile();
        if pte & EPT_R != 0 {
            return Some(pte & EPT_ADDR_MASK); // already faulted in
        }
        let spare = crate::microvm::devices::guest_mem::frames_spare();
        if spare == 0 {
            return None;
        }
        let frame = memory::allocate_frame()?;
        core::ptr::write_bytes(frame as *mut u8, 0, 4096);
        pt.add(pt_idx)
            .write_volatile(frame | EPT_RWX | EPT_MEM_TYPE_WB);
        // Fault-around: back the rest of this 2 MB block now, so a guest
        // walking fresh memory takes one exit per 2 MB, not one per 4 KB.
        // Stops at the host reserve; the remaining pages fault in singly.
        let mut left = spare - 1;
        for j in 0..512usize {
            if left == 0 { break; }
            if j == pt_idx || pt.add(j).read_volatile() & EPT_R != 0 { continue; }
            left -= 1;
            let Some(f) = memory::allocate_frame() else { break };
            core::ptr::write_bytes(f as *mut u8, 0, 4096);
            pt.add(j).write_volatile(f | EPT_RWX | EPT_MEM_TYPE_WB);
        }
        Some(frame)
    }
}

/// Free everything `install_window` allocated for `guest_bytes`: every
/// demand 2-MB block and 4 KB frame, the demand PT pages, and the fixed tables
/// (PML4/PDPT/PDs/PD_HIGH/PT_DUMMY/PT_LAPIC/scratch page). Also unwinds a
/// partially built tree: absent entries are skipped. The contiguous boot
/// block is freed separately by the caller (it owns `boot_raw_base`).
pub fn release(pml4_phys: u64, guest_bytes: u64) {
    let boot_leaves = boot_window_bytes(guest_bytes) / TWO_MB;
    let guest_leaves = guest_bytes / TWO_MB;
    let num_pds = ((guest_bytes + ONE_GB - 1) / ONE_GB) as usize;
    const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512
    // SAFETY: our own EPT; nothing else references these frames after
    // VMXOFF + window teardown.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt_phys = pml4.read_volatile() & EPT_ADDR_MASK;
        let pdpt = pdpt_phys as *const u64;
        // Walk PDPT[0..num_pds] and free each guest-RAM PD + its demand
        // PTs + demand frames.
        for p in 0..num_pds {
            let pdpt_entry = pdpt.add(p).read_volatile();
            if pdpt_entry == 0 { continue; }
            let pd_phys = pdpt_entry & EPT_ADDR_MASK;
            let pd = pd_phys as *const u64;
            let base_leaf = (p as u64) * LEAVES_PER_PD;
            let end_leaf = ((p as u64 + 1) * LEAVES_PER_PD).min(guest_leaves);
            for leaf in base_leaf..end_leaf {
                if leaf < boot_leaves { continue; }
                let local = (leaf - base_leaf) as usize;
                let pde = pd.add(local).read_volatile();
                if pde & EPT_R == 0 { continue; }
                if pde & EPT_LEAF != 0 {
                    memory::deallocate_contiguous(pde & EPT_2M_ADDR_MASK, FRAMES_PER_2M);
                    continue;
                }
                let pt_phys = pde & EPT_ADDR_MASK;
                let pt = pt_phys as *const u64;
                for j in 0..512usize {
                    let pte = pt.add(j).read_volatile();
                    if pte & EPT_R != 0 {
                        memory::deallocate_frame(pte & EPT_ADDR_MASK);
                    }
                }
                memory::deallocate_frame(pt_phys);
            }
            memory::deallocate_frame(pd_phys);
        }
        let pd_high_phys = pdpt.add(3).read_volatile() & EPT_ADDR_MASK;
        if pd_high_phys != 0 {
            let pd_high = pd_high_phys as *const u64;
            let pt_dummy_phys = pd_high.add(502).read_volatile() & EPT_ADDR_MASK;
            let pt_lapic_phys = pd_high.add(503).read_volatile() & EPT_ADDR_MASK;
            if pt_lapic_phys != 0 && pt_lapic_phys != pt_dummy_phys {
                memory::deallocate_frame(pt_lapic_phys);
            }
            if pt_dummy_phys != 0 {
                // [0] is the trapped I/O APIC page; [1] maps the scratch page.
                let dummy = (pt_dummy_phys as *const u64).add(1).read_volatile() & EPT_ADDR_MASK;
                if dummy != 0 {
                    memory::deallocate_frame(dummy);
                }
                memory::deallocate_frame(pt_dummy_phys);
            }
            memory::deallocate_frame(pd_high_phys);
        }
        memory::deallocate_frame(pdpt_phys);
        memory::deallocate_frame(pml4_phys);
    }
}
