//! Extended Page Tables (EPT) — Phase 12.1.1a/c-1/c-3.
//!
//! Maps a 64-MB guest-physical window [0, 64 MB) onto a contiguous
//! 64-MB host-physical region using 2-MB EPT large pages (32 leaf
//! entries in a single PD). The host backing region is allocated
//! once via `memory::allocate_contiguous(GUEST_RAM_FRAMES + slack)`;
//! the caller rounds the result up to a 2-MB boundary and passes
//! that base in.
//!
//! Why non-identity (12.1.1c-1 vs the v0.97 1-GB identity map):
//! the guest will copy Linux's bzImage into its address space at
//! guest-phys 0x10000 (setup) and 0x100000 (protected-mode kernel)
//! — but host_phys 0x100000 is the kernel.bin's own load address
//! (Multiboot2 puts us at 1 MB). A non-identity EPT separates the
//! two so the guest can write freely without corrupting host code.
//!
//! Why 64 MB: Alpine 6.18 linux-virt's `init_size` field reports
//! 0x25ff000 ≈ 38 MB — that's how much memory Linux's early boot
//! needs for decompression buffers + brk + page tables before
//! it sees its own memory map. 16 MB (12.1.1c-1) was enough for
//! the real-mode HLT-test substrate but cannot host real Linux.
//! 64 MB rounded up gives Linux some headroom and stays in a
//! single PD's range (32 × 2-MB leaves; one PML4 + one PDPT + one
//! PD covers everything). Larger windows would need a second PD.
//!
//! Tables are leaked (same lifecycle as VMXON / VMCS regions in
//! `vmx/enable.rs`).
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
// 1 GB guest RAM — Firefox manifest spec target. Exactly fills one
// PDPT entry's worth of 2 MB pages (512 leaves). Bumping past 1 GB
// needs multi-PD support (split across PDPT entries). Canonical size
// lives in `guest_mem`; this is its EPT-window twin.
const GUEST_WINDOW_BYTES: u64 = crate::microvm::devices::guest_mem::GUEST_RAM_BYTES;

/// Slack frames `boot_frames_for` adds so `allocate_contiguous` can be
/// rounded up to a 2-MB boundary for the boot-window 2-MB leaves.
const GUEST_RAM_ALIGN_SLACK: usize = 511;

/// Bits [51:12] of an EPT entry hold the next-level / page phys addr.
const EPT_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// B3 hybrid split: `[0, BOOT_WINDOW_BYTES)` is one contiguous 2-MB-
/// leaf block (fast boot, no faults on the early path, covers the
/// kernel image + initramfs @ 0x0C00_0000 + boot_params); everything
/// above is demand-paged 4 KB on EPT-violation. 256 MiB is reliably
/// allocatable even on a fragmented host (the 1 GiB contiguous alloc
/// was the fragmentation problem); the lazy region is the page cache
/// / browser heap, the bulk on a ≥1 GiB guest.
pub const BOOT_WINDOW_BYTES: u64 = 256 * 1024 * 1024;

/// Round a raw `allocate_contiguous` base up to the next 2-MB
/// boundary so it can be passed to `install_window`.
pub fn round_up_to_2mb(raw_base: u64) -> u64 {
    (raw_base + (TWO_MB - 1)) & !(TWO_MB - 1)
}

/// Contiguous boot window size for a guest of `guest_bytes`
/// (= `min(BOOT_WINDOW_BYTES, guest_bytes)`, 2-MB-aligned). A guest
/// ≤ 256 MiB (small-host B2 policy) is fully contiguous → no demand
/// region, behaviour identical to B2.
pub fn boot_window_bytes(guest_bytes: u64) -> u64 {
    guest_bytes.min(BOOT_WINDOW_BYTES)
}

/// 4 KB host frames to allocate **contiguously** for the boot window
/// (+ 2-MB-align slack). The demand region needs NO upfront
/// allocation — only touched 4 KB pages are committed on fault.
pub fn boot_frames_for(guest_bytes: u64) -> usize {
    (boot_window_bytes(guest_bytes) / 4096) as usize + GUEST_RAM_ALIGN_SLACK
}

/// Build the EPT. Maps:
///   - guest-physical [0, 64 MB) → host-physical [host_base, +64 MB)
///     via 32 × 2-MB leaf entries (PD)
///   - guest-physical [0xFEC00000, 0xFF000000) → 4 KB dummy scratch
///     page (aliased): 4 MB of guest-phys → same single host page
///     via PT-level mapping. Covers IOAPIC (0xFEC00000), HPET
///     (0xFED00000), and LAPIC (0xFEE00000). Reads return scratch
///     contents (initially zero), writes land in scratch — not real
///     MMIO semantics, but enough to absorb Linux's early MMIO
///     probes without EPT-violating. With `nolapic noapic acpi=off
///     pci=off` cmdline, Linux barely touches this anyway; mapping
///     it is just defence in depth.
/// Returns `(eptp, pml4_phys)` — the EPTP to VMWRITE into
/// VMCS::EPT_POINTER, and the PML4 phys for `demand_fault_in` /
/// `release`. `boot_base` backs `[0, boot_window_bytes(guest_bytes))`
/// contiguously (2-MB leaves); `[boot, guest_bytes)` gets PD→PT
/// sub-tables with all-absent 4-KB PTEs, faulted in on demand.
pub fn install_window(boot_base: u64, guest_bytes: u64) -> Result<(u64, u64), &'static str> {
    if boot_base & (TWO_MB - 1) != 0 {
        return Err("EPT: boot_base must be 2-MB aligned for 2-MB EPT pages");
    }
    if guest_bytes == 0 || guest_bytes & (TWO_MB - 1) != 0 {
        return Err("EPT: guest_bytes must be a non-zero multiple of 2 MB");
    }
    // Single PD covers ≤ 1 GiB (512 × 2-MB region). B4 adds multi-PD.
    if guest_bytes > GUEST_WINDOW_BYTES {
        return Err("EPT: guest_bytes exceeds single-PD window (B4: multi-PD)");
    }
    let boot_leaves = boot_window_bytes(guest_bytes) / TWO_MB;
    let guest_leaves = guest_bytes / TWO_MB;

    let pml4_phys = memory::allocate_frame().ok_or("OOM: EPT PML4")?;
    let pdpt_phys = memory::allocate_frame().ok_or("OOM: EPT PDPT")?;
    let pd_phys = memory::allocate_frame().ok_or("OOM: EPT PD")?;
    let pd_high_phys = memory::allocate_frame().ok_or("OOM: EPT PD_HIGH")?;
    let pt_dummy_phys = memory::allocate_frame().ok_or("OOM: EPT PT_DUMMY")?;
    let dummy_page_phys = memory::allocate_frame().ok_or("OOM: EPT dummy page")?;

    // SAFETY: identity-mapped, freshly allocated, exclusive.
    unsafe {
        // PML4[0] → PDPT, R/W/X for sub-tree.
        let pml4 = pml4_phys as *mut u64;
        core::ptr::write_bytes(pml4 as *mut u8, 0, 4096);
        pml4.add(0).write_volatile(pdpt_phys | EPT_RWX);

        // PDPT[0] → PD (covers [0, 1 GB)).
        let pdpt = pdpt_phys as *mut u64;
        core::ptr::write_bytes(pdpt as *mut u8, 0, 4096);
        pdpt.add(0).write_volatile(pd_phys | EPT_RWX);
        // PDPT[3] → PD_HIGH (covers [3 GB, 4 GB), MMIO range lives here).
        pdpt.add(3).write_volatile(pd_high_phys | EPT_RWX);

        let pd = pd_phys as *mut u64;
        core::ptr::write_bytes(pd as *mut u8, 0, 4096);
        // PD[0..boot_leaves] = 2-MB leaves → contiguous boot block.
        for i in 0..boot_leaves {
            let host_target = boot_base + i * TWO_MB;
            pd.add(i as usize)
                .write_volatile(host_target | EPT_RWX | EPT_MEM_TYPE_WB | EPT_LEAF);
        }
        // PD[boot_leaves..guest_leaves] → fresh PT sub-tables, all 512
        // 4-KB PTEs absent. `demand_fault_in` populates one PTE per
        // EPT-violation; `release` walks these to free on teardown.
        for i in boot_leaves..guest_leaves {
            let pt = memory::allocate_frame().ok_or("OOM: EPT demand PT")?;
            core::ptr::write_bytes(pt as *mut u8, 0, 4096);
            pd.add(i as usize).write_volatile(pt | EPT_RWX);
        }

        // PD_HIGH[502] + [503] → same PT_DUMMY (covers [0xFEC00000,
        // 0xFF000000) = IOAPIC + HPET + LAPIC). Two PD entries
        // aliased to one PT, so 4 MB of guest-phys all walk through
        // the same 512-entry PT.
        let pd_high = pd_high_phys as *mut u64;
        core::ptr::write_bytes(pd_high as *mut u8, 0, 4096);
        pd_high.add(502).write_volatile(pt_dummy_phys | EPT_RWX);
        pd_high.add(503).write_volatile(pt_dummy_phys | EPT_RWX);

        // PT_DUMMY[0..512] all → dummy_page (4 KB). 4 MB of guest-phys
        // → 4 KB host scratch. PT entries are always 4-KB leaves;
        // no leaf-bit needed (bit 7 must be 0 in EPT PTEs).
        let pt_dummy = pt_dummy_phys as *mut u64;
        core::ptr::write_bytes(pt_dummy as *mut u8, 0, 4096);
        core::ptr::write_bytes(dummy_page_phys as *mut u8, 0, 4096);
        for i in 0..512usize {
            pt_dummy
                .add(i)
                .write_volatile(dummy_page_phys | EPT_RWX | EPT_MEM_TYPE_WB);
        }
    }

    Ok((pml4_phys | EPTP_WALK_LENGTH_4 | EPTP_MEM_TYPE_WB, pml4_phys))
}

/// Walk PML4[0]→PDPT[0]→PD→PT for a demand-region `gpa` and return
/// the host phys of its 4 KB page, allocating + mapping a zeroed
/// frame on first touch. Called from the EPT-violation handler
/// (guest fault) AND `GuestMem` (host DMA to an untouched page) —
/// idempotent: an already-present PTE returns its existing frame.
/// `gpa` MUST be ≥ the boot window (caller guarantees; the boot
/// region is 2-MB leaves, never PT-walked). No INVEPT needed: the
/// entry was not-present so there is no stale TLB for it.
pub fn demand_fault_in(pml4_phys: u64, gpa: u64) -> Option<u64> {
    // SAFETY: our own freshly-built EPT, identity-mapped tables.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt = (pml4.read_volatile() & EPT_ADDR_MASK) as *const u64;
        let pd = (pdpt.read_volatile() & EPT_ADDR_MASK) as *mut u64;
        let pd_idx = (gpa / TWO_MB) as usize;
        let pde = pd.add(pd_idx).read_volatile();
        if pde == 0 || pde & EPT_LEAF != 0 {
            return None; // outside demand region / unexpected 2-MB leaf
        }
        let pt = (pde & EPT_ADDR_MASK) as *mut u64;
        let pt_idx = ((gpa % TWO_MB) / 4096) as usize;
        let pte = pt.add(pt_idx).read_volatile();
        if pte & EPT_R != 0 {
            return Some(pte & EPT_ADDR_MASK); // already faulted in
        }
        let frame = memory::allocate_frame()?;
        core::ptr::write_bytes(frame as *mut u8, 0, 4096);
        pt.add(pt_idx)
            .write_volatile(frame | EPT_RWX | EPT_MEM_TYPE_WB);
        Some(frame)
    }
}

/// Free everything `install_window` allocated for `guest_bytes`: every
/// demand-faulted 4 KB frame, the demand PT pages, and the four fixed
/// tables (PML4/PDPT/PD/PD_HIGH/PT_DUMMY/dummy). The contiguous boot
/// block is freed separately by the caller (it owns `boot_raw_base`).
pub fn release(pml4_phys: u64, guest_bytes: u64) {
    let boot_leaves = boot_window_bytes(guest_bytes) / TWO_MB;
    let guest_leaves = guest_bytes / TWO_MB;
    // SAFETY: our own EPT; nothing else references these frames after
    // VMXOFF + window teardown.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt_phys = pml4.read_volatile() & EPT_ADDR_MASK;
        let pdpt = pdpt_phys as *const u64;
        let pd_phys = pdpt.read_volatile() & EPT_ADDR_MASK;
        let pd = pd_phys as *const u64;
        for i in boot_leaves..guest_leaves {
            let pde = pd.add(i as usize).read_volatile();
            if pde == 0 || pde & EPT_LEAF != 0 {
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
        let pd_high_phys = pdpt.add(3).read_volatile() & EPT_ADDR_MASK;
        if pd_high_phys != 0 {
            let pd_high = pd_high_phys as *const u64;
            let pt_dummy_phys = pd_high.add(502).read_volatile() & EPT_ADDR_MASK;
            if pt_dummy_phys != 0 {
                let dummy = (pt_dummy_phys as *const u64).read_volatile() & EPT_ADDR_MASK;
                if dummy != 0 {
                    memory::deallocate_frame(dummy);
                }
                memory::deallocate_frame(pt_dummy_phys);
            }
            memory::deallocate_frame(pd_high_phys);
        }
        memory::deallocate_frame(pd_phys);
        memory::deallocate_frame(pdpt_phys);
        memory::deallocate_frame(pml4_phys);
    }
}
