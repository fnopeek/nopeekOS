//! Nested Page Tables — AMD's equivalent of Intel EPT.
//!
//! Reference: AMD64 APM Vol. 2 §15.25 "Nested Paging".
//!
//! NPT uses the standard 4-level x86_64 page-table format (PML4 →
//! PDPT → PD → PT), unlike EPT which has its own permission bit
//! layout. That makes NPT noticeably easier to bring up: the same
//! page-table walker the kernel uses for host paging would, in
//! principle, work for guest NPT too.
//!
//! Two modes are used by the SVM backend:
//!
//! * `allocate_identity_npt()` — guest 0..256 MB → host 0..256 MB.
//!   Used by the substrate test where the guest stub lives wherever
//!   the frame allocator hands it out. Only safe when the stub
//!   happens to fall below 256 MB.
//!
//! * `allocate_window_npt(host_base)` — guest 0..256 MB → host
//!   `host_base..host_base+256 MB`. Used by `run_linux` so the guest
//!   can write freely at GPA 0x10000/0x90000/0x100000/... without
//!   stomping on the host kernel image (which sits at host_phys 1 MB
//!   from Multiboot2). Mirrors `vmx::ept::install_window`.
//!
//! Both modes use 2 MB pages (3-frame NPT footprint: PML4+PDPT+PD).
//! 1 GB pages tested unstable on KVM nested SVM (exit-code 0); 2 MB
//! stays inside the well-shadowed path.
//!
//! `allocate_window_npt` additionally maps the high MMIO region
//! [0xFEC00000, 0xFF000000) (IOAPIC + HPET + LAPIC) to a single
//! aliased scratch page. With `nolapic noapic acpi=off` Linux barely
//! touches it — the mapping is defence-in-depth so an early MMIO
//! probe doesn't NPF before we've added a real exit handler.

use crate::mm::memory;

/// [B3-DIAG] count of 4-KB demand pages faulted in this boot. Used to
/// throttle the trace and to size-check OOM. Diagnostic only — removed
/// once the demand-corruption root cause is confirmed.
static DEMAND_FRAMES: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Slack frames `boot_frames_for` adds so `allocate_contiguous` can be
/// rounded up to a 2 MB boundary. Mirrors `ept`.
const GUEST_RAM_ALIGN_SLACK: usize = 511;

/// Bits [51:12] of an NPT entry hold the next-level / page phys addr.
const NPT_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

const TWO_MB: u64 = 2 * 1024 * 1024;
// Canonical size lives in `guest_mem`; this is its NPT-window twin.
const GUEST_WINDOW_BYTES: u64 = crate::microvm::devices::guest_mem::GUEST_RAM_BYTES;

// ── NPT page-table flags ───────────────────────────────────────────

/// Present.
const NPT_P: u64 = 1 << 0;
/// Writable.
const NPT_RW: u64 = 1 << 1;
/// User-mode accessible. Must be set in NPT entries — otherwise the
/// CPU treats the page as kernel-only, and any guest access NPT-
/// faults with a permission mismatch (APM §15.25.6).
const NPT_US: u64 = 1 << 2;
/// Page Size — leaf entries at PD level (2 MB pages). Cleared at
/// PML4 + PDPT (those point to the next level).
const NPT_PS: u64 = 1 << 7;

/// Contiguous boot window; mirrors `ept`. See `ept::BOOT_WINDOW_BYTES`.
pub const BOOT_WINDOW_BYTES: u64 = 256 * 1024 * 1024;

/// Round a raw `allocate_contiguous` base up to the next 2 MB
/// boundary so it can be passed to `allocate_window_npt`.
pub fn round_up_to_2mb(raw_base: u64) -> u64 {
    (raw_base + (TWO_MB - 1)) & !(TWO_MB - 1)
}

/// Contiguous boot window size for `guest_bytes`; mirrors `ept`.
pub fn boot_window_bytes(guest_bytes: u64) -> u64 {
    if crate::microvm::devices::guest_mem::DEMAND_ENABLED {
        guest_bytes.min(BOOT_WINDOW_BYTES)
    } else {
        // Demand off → whole guest contiguous: exactly B2/A2.
        guest_bytes
    }
}

/// 4 KB host frames to allocate **contiguously** for the boot window
/// (+ 2-MB-align slack). The demand region is faulted 4 KB at a time;
/// mirrors `ept::boot_frames_for`.
pub fn boot_frames_for(guest_bytes: u64) -> usize {
    (boot_window_bytes(guest_bytes) / 4096) as usize + GUEST_RAM_ALIGN_SLACK
}

/// Build a fresh NPT root that identity-maps `0..256 MB` of guest
/// physical to host physical via 2 MB pages. Returns the physical
/// address of the PML4 page, suitable for VMCB.NCR3.
///
/// Allocates 3 frames per call (PML4 + PDPT + PD). Frames are leaked
/// alongside the rest of the per-call substrate-test allocations.
pub fn allocate_identity_npt() -> Result<u64, &'static str> {
    // Substrate test: fixed 1 GiB, behaviour unchanged by B2.
    build_npt(0, GUEST_WINDOW_BYTES, /* with_mmio_scratch */ false)
}

/// Build a fresh NPT root that maps `0..256 MB` of guest physical to
/// host physical `host_base..host_base+256 MB` via 2 MB pages, plus a
/// scratch alias for [0xFEC00000, 0xFF000000) (IOAPIC + HPET +
/// LAPIC). Returns NCR3.
///
/// `host_base` must be 2 MB aligned. Allocates 6 frames (PML4 + PDPT
/// + PD + PD_HIGH + PT_DUMMY + dummy_page). Frames are leaked.
pub fn allocate_window_npt(host_base: u64, guest_bytes: u64) -> Result<u64, &'static str> {
    if host_base & (TWO_MB - 1) != 0 {
        return Err("NPT: host_base must be 2 MB aligned");
    }
    build_npt(host_base, guest_bytes, /* with_mmio_scratch */ true)
}

/// Inner builder. `host_base = 0` gives identity mapping; non-zero
/// shifts the leaf addresses by `host_base`. Maps exactly
/// `guest_bytes` (single PD, ≤ 1 GiB; B4 adds multi-PD).
fn build_npt(host_base: u64, guest_bytes: u64, with_mmio_scratch: bool) -> Result<u64, &'static str> {
    if guest_bytes == 0 || guest_bytes & (TWO_MB - 1) != 0 {
        return Err("NPT: guest_bytes must be a non-zero multiple of 2 MB");
    }
    if guest_bytes > GUEST_WINDOW_BYTES {
        return Err("NPT: guest_bytes exceeds single-PD window (B4: multi-PD)");
    }
    let guest_leaves = (guest_bytes / TWO_MB) as usize;
    // Identity substrate test (host_base = 0) stays all-2-MB; the real
    // window path is hybrid (contiguous boot + 4-KB demand region).
    let boot_leaves = if host_base == 0 {
        guest_leaves
    } else {
        (boot_window_bytes(guest_bytes) / TWO_MB) as usize
    };
    let pml4_phys = memory::allocate_frame()
        .ok_or("OOM allocating NPT PML4")?;
    let pdpt_phys = memory::allocate_frame()
        .ok_or("OOM allocating NPT PDPT")?;
    let pd_phys = memory::allocate_frame()
        .ok_or("OOM allocating NPT PD")?;

    // SAFETY: freshly allocated, identity-mapped (host paging),
    // exclusive. All pages are 4 KB aligned (frame allocator
    // guarantee).
    unsafe {
        core::ptr::write_bytes(pml4_phys as *mut u8, 0, 4096);
        core::ptr::write_bytes(pdpt_phys as *mut u8, 0, 4096);
        core::ptr::write_bytes(pd_phys as *mut u8, 0, 4096);

        // PML4[0] → PDPT (non-leaf, no PS bit).
        let pml4 = pml4_phys as *mut u64;
        pml4.write_volatile(pdpt_phys | NPT_P | NPT_RW | NPT_US);

        // PDPT[0] → PD (non-leaf, covers [0, 1 GB)).
        let pdpt = pdpt_phys as *mut u64;
        pdpt.write_volatile(pd_phys | NPT_P | NPT_RW | NPT_US);

        let pd = pd_phys as *mut u64;
        // PD[0..boot_leaves] = 2-MB leaves → contiguous boot block
        // (or the whole window for the identity substrate test).
        for i in 0..boot_leaves {
            let host_target = host_base + (i as u64) * TWO_MB;
            let entry = host_target | NPT_P | NPT_RW | NPT_US | NPT_PS;
            pd.add(i).write_volatile(entry);
        }
        // PD[boot_leaves..guest_leaves] → fresh PT sub-tables, all 512
        // 4-KB PTEs absent; faulted in by `demand_fault_in`.
        for i in boot_leaves..guest_leaves {
            let pt = memory::allocate_frame().ok_or("OOM allocating NPT demand PT")?;
            core::ptr::write_bytes(pt as *mut u8, 0, 4096);
            pd.add(i).write_volatile(pt | NPT_P | NPT_RW | NPT_US);
        }

        if with_mmio_scratch {
            // PDPT[3] → PD_HIGH (covers [3 GB, 4 GB), MMIO range).
            let pd_high_phys = memory::allocate_frame()
                .ok_or("OOM allocating NPT PD_HIGH")?;
            let pt_dummy_phys = memory::allocate_frame()
                .ok_or("OOM allocating NPT PT_DUMMY")?;
            let dummy_page_phys = memory::allocate_frame()
                .ok_or("OOM allocating NPT dummy page")?;

            core::ptr::write_bytes(pd_high_phys as *mut u8, 0, 4096);
            core::ptr::write_bytes(pt_dummy_phys as *mut u8, 0, 4096);
            core::ptr::write_bytes(dummy_page_phys as *mut u8, 0, 4096);

            pdpt.add(3).write_volatile(pd_high_phys | NPT_P | NPT_RW | NPT_US);

            // PD_HIGH[502] + [503] → same PT_DUMMY (covers
            // [0xFEC00000, 0xFF000000)). Two PD entries aliased to
            // one PT, so 4 MB of guest-phys all walk through the
            // same 512-entry PT.
            let pd_high = pd_high_phys as *mut u64;
            pd_high.add(502).write_volatile(pt_dummy_phys | NPT_P | NPT_RW | NPT_US);
            pd_high.add(503).write_volatile(pt_dummy_phys | NPT_P | NPT_RW | NPT_US);

            // PT_DUMMY[0..512] all → dummy_page. 4 MB → 4 KB scratch.
            let pt_dummy = pt_dummy_phys as *mut u64;
            for i in 0..512usize {
                pt_dummy.add(i).write_volatile(dummy_page_phys | NPT_P | NPT_RW | NPT_US);
            }
        }
    }

    Ok(pml4_phys)
}

/// Walk PML4[0]→PDPT[0]→PD→PT for a demand-region `gpa`; allocate +
/// map a zeroed 4 KB frame on first touch, return its host phys.
/// Idempotent. Called from the #NPF handler AND `GuestMem` (host DMA
/// to an untouched page). `gpa` MUST be ≥ the boot window. No
/// INVLPGA needed: the entry was not-present (no stale TLB).
/// Mirrors `ept::demand_fault_in`.
pub fn demand_fault_in(pml4_phys: u64, gpa: u64) -> Option<u64> {
    // SAFETY: our own freshly-built NPT, identity-mapped tables.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt = (pml4.read_volatile() & NPT_ADDR_MASK) as *const u64;
        let pd = (pdpt.read_volatile() & NPT_ADDR_MASK) as *mut u64;
        let pde = pd.add((gpa / TWO_MB) as usize).read_volatile();
        if pde == 0 || pde & NPT_PS != 0 {
            return None; // outside demand region / unexpected 2-MB leaf
        }
        let pt = (pde & NPT_ADDR_MASK) as *mut u64;
        let pt_idx = ((gpa % TWO_MB) / 4096) as usize;
        let pte = pt.add(pt_idx).read_volatile();
        if pte & NPT_P != 0 {
            return Some(pte & NPT_ADDR_MASK); // already faulted in
        }
        let frame = match memory::allocate_frame() {
            Some(f) => f,
            None => {
                // [B3-DIAG] smoking gun: a demand page could not be
                // backed → page_host None → read/write_bytes silently
                // drops the DMA → guest RAM corruption. Un-throttled.
                crate::kprintln!(
                    "[B3-DIAG] demand_fault_in OOM gpa={:#x} (total faulted={})",
                    gpa,
                    DEMAND_FRAMES.load(core::sync::atomic::Ordering::Relaxed),
                );
                return None;
            }
        };
        core::ptr::write_bytes(frame as *mut u8, 0, 4096);
        pt.add(pt_idx)
            .write_volatile(frame | NPT_P | NPT_RW | NPT_US);
        let n = DEMAND_FRAMES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 8 || n % 4096 == 0 {
            crate::kprintln!(
                "[B3-DIAG] demand fault #{} gpa={:#x} -> frame={:#x}",
                n, gpa, frame,
            );
        }
        Some(frame)
    }
}

/// Free every demand-faulted 4 KB frame + the demand PT pages + the
/// fixed tables. The contiguous boot block is freed by the caller.
/// Mirrors `ept::release`.
pub fn release(pml4_phys: u64, guest_bytes: u64) {
    let boot_leaves = (boot_window_bytes(guest_bytes) / TWO_MB) as usize;
    let guest_leaves = (guest_bytes / TWO_MB) as usize;
    // SAFETY: our own NPT; nothing references these after teardown.
    unsafe {
        let pml4 = pml4_phys as *const u64;
        let pdpt_phys = pml4.read_volatile() & NPT_ADDR_MASK;
        let pdpt = pdpt_phys as *const u64;
        let pd_phys = pdpt.read_volatile() & NPT_ADDR_MASK;
        let pd = pd_phys as *const u64;
        for i in boot_leaves..guest_leaves {
            let pde = pd.add(i).read_volatile();
            if pde == 0 || pde & NPT_PS != 0 {
                continue;
            }
            let pt_phys = pde & NPT_ADDR_MASK;
            let pt = pt_phys as *const u64;
            for j in 0..512usize {
                let pte = pt.add(j).read_volatile();
                if pte & NPT_P != 0 {
                    memory::deallocate_frame(pte & NPT_ADDR_MASK);
                }
            }
            memory::deallocate_frame(pt_phys);
        }
        let pd_high_phys = pdpt.add(3).read_volatile() & NPT_ADDR_MASK;
        if pd_high_phys != 0 {
            let pd_high = pd_high_phys as *const u64;
            let pt_dummy_phys = pd_high.add(502).read_volatile() & NPT_ADDR_MASK;
            if pt_dummy_phys != 0 {
                let dummy = (pt_dummy_phys as *const u64).read_volatile() & NPT_ADDR_MASK;
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
