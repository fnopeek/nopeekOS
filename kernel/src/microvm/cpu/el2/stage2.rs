//! Stage-2 translation of the guest, after Linux
//! `arch/arm64/kvm/hyp/pgtable.c` and `arch/arm64/kvm/mmu.c`.
//!
//! 4 KiB granule and a 40-bit IPA (KVM's default), so the walk starts at
//! level 1 with two concatenated tables (Arm ARM D8.2.6). Guest RAM is laid
//! out as on x86 (`guest_mem::RamLayout`): a contiguous boot window mapped
//! with 2 MiB blocks at open, the rest faulted in on first touch as a 2 MiB
//! block, or as 4 KiB pages when no aligned block is free. Every frame the
//! guest is given is cleaned to the point of coherency first: a guest with
//! its MMU off reads memory past the caches (`clean_dcache_guest_page`).
//! Demand memory is mapped not executable; the first instruction fetch
//! invalidates the instruction cache and lifts the XN bit, as KVM does on
//! an exec fault (`__invalidate_icache_guest_page`).
//!
//! Removing an entry is followed by a broadcast TLBI of the IPA under the
//! VM's VMID (`__kvm_tlb_flush_vmid_ipa`), so no core keeps the old
//! translation once the call returns.

use crate::mm::memory;
use crate::microvm::devices::guest_mem::RamLayout;
use core::sync::atomic::{AtomicU64, Ordering};

pub const IPA_BITS: u32 = 40;
/// The VM's VMID (one VM at a time).
pub const VMID: u64 = 1;

const TWO_MB: u64 = 2 << 20;
const ONE_GB: u64 = 1 << 30;
const FRAMES_PER_2M: usize = 512;
/// Level-1 entries: two concatenated 4 KiB tables cover 40 bits.
const ROOT_ENTRIES: usize = 1024;
const ROOT_FRAMES: usize = 2;

const VALID: u64 = 1 << 0;
/// A table at levels 1 and 2, a page at level 3.
const TABLE: u64 = 1 << 1;
const MEMATTR_SHIFT: u64 = 2;
/// Stage-2 MemAttr[3:0]: Normal outer/inner write-back.
pub const MEM_NORMAL_WB: u64 = 0xf << MEMATTR_SHIFT;
/// Normal outer/inner non-cacheable.
pub const MEM_NORMAL_NC: u64 = 0x5 << MEMATTR_SHIFT;
/// Device-nGnRE.
pub const MEM_DEVICE: u64 = 0x1 << MEMATTR_SHIFT;
const S2AP_RW: u64 = 3 << 6;
const SH_INNER: u64 = 3 << 8;
const AF: u64 = 1 << 10;
/// Stage-2 XN[1]: no instruction fetch at EL1 and EL0.
const XN: u64 = 1 << 54;
const ADDR_MASK: u64 = 0x0000_ffff_ffff_f000;
const BLOCK_ADDR_MASK: u64 = 0x0000_ffff_ffe0_0000;

const RAM_LEAF: u64 = VALID | AF | SH_INNER | S2AP_RW | MEM_NORMAL_WB;

pub static DEMAND_2M: AtomicU64 = AtomicU64::new(0);
pub static DEMAND_4K_BLOCKS: AtomicU64 = AtomicU64::new(0);

/// Contiguous boot window, as `npt`/`ept`.
pub const BOOT_WINDOW_BYTES: u64 = 256 << 20;
const GUEST_RAM_ALIGN_SLACK: usize = 511;

pub fn round_up_to_2mb(raw_base: u64) -> u64 {
    (raw_base + (TWO_MB - 1)) & !(TWO_MB - 1)
}

pub fn boot_window_bytes(guest_bytes: u64) -> u64 {
    if crate::microvm::devices::guest_mem::DEMAND_ENABLED {
        guest_bytes.min(BOOT_WINDOW_BYTES)
    } else {
        guest_bytes
    }
}

pub fn boot_frames_for(guest_bytes: u64) -> usize {
    (boot_window_bytes(guest_bytes) / 4096) as usize + GUEST_RAM_ALIGN_SLACK
}

/// VTCR_EL2 for this layout (kvm_get_vtcr): T0SZ for 40 bits, start at
/// level 1, write-back inner shareable walks, 4 KiB granule, the CPU's
/// physical address size, 8-bit VMID. None if the CPU has fewer than 40
/// physical address bits.
pub fn vtcr() -> Option<u64> {
    let mmfr0: u64;
    // SAFETY: reads an ID register, no other effect.
    unsafe { core::arch::asm!("mrs {}, ID_AA64MMFR0_EL1", out(reg) mmfr0, options(nomem, nostack, preserves_flags)) };
    let parange = (mmfr0 & 0xf).min(5);
    if parange < 2 {
        return None;
    }
    const RES1: u64 = 1 << 31;
    Some(RES1 | (parange << 16) | (3 << 12) | (1 << 10) | (1 << 8) | (1 << 6) | (64 - IPA_BITS as u64))
}

/// VTTBR_EL2 for the root `root`.
pub fn vttbr(root: u64) -> u64 {
    root | (VMID << 48)
}

fn zero(frame: u64, len: usize) {
    // SAFETY: a frame just taken from the allocator, identity-mapped and owned here.
    unsafe { core::ptr::write_bytes(frame as *mut u8, 0, len) };
}

/// Zero and clean to the point of coherency: what the guest may read with
/// its caches off is what is in memory.
fn zero_clean(frame: u64, len: usize) {
    zero(frame, len);
    crate::mm::clflush_range(frame, len as u64);
}

fn table_frame() -> Option<u64> {
    let f = memory::allocate_frame()?;
    zero(f, 4096);
    Some(f)
}

/// Every stale instruction for any address goes (`ic ialluis`).
pub fn invalidate_icache() {
    // SAFETY: cache maintenance, no data changes.
    unsafe { core::arch::asm!("dsb ish", "ic ialluis", "dsb ish", "isb", options(nostack, preserves_flags)) };
}

/// A table walk reads what was written (the walker is coherent, FEAT
/// coherent walks; the barrier orders the store before the next walk).
fn publish() {
    // SAFETY: a barrier only.
    unsafe { core::arch::asm!("dsb ishst", options(nostack, preserves_flags)) };
}

/// `__tlb_switch_to_guest` + `__kvm_tlb_flush_vmid_ipa` + switch back:
/// drop the translation of `ipa` (and the combined stage 1+2 entries) on
/// every core. With HCR_EL2.TGE set TLB maintenance targets EL2&0, so TGE
/// is cleared around it, with this VM's stage 2 loaded.
fn flush_ipa(root: u64, ipa: Option<u64>) {
    flush(root, ipa, false);
}

/// `__kvm_tlb_flush_vmid_ipa_nsh`: drop the translation of `ipa` on this
/// core only, after a permission was relaxed (`kvm_pgtable_stage2_relax_perms`).
fn flush_ipa_local(root: u64, ipa: u64) {
    flush(root, Some(ipa), true);
}

fn flush(root: u64, ipa: Option<u64>, local: bool) {
    let irq = crate::arch::irq_save();
    // SAFETY: the calling core's own HCR_EL2/VTTBR_EL2; interrupts are
    // masked while TGE is clear, so no host code runs in that regime.
    unsafe {
        let hcr: u64;
        let old_vttbr: u64;
        core::arch::asm!("mrs {}, HCR_EL2", out(reg) hcr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, VTTBR_EL2", out(reg) old_vttbr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr VTTBR_EL2, {}", "isb", in(reg) vttbr(root), options(nostack, preserves_flags));
        core::arch::asm!("msr HCR_EL2, {}", "isb", in(reg) hcr & !(1u64 << 27), options(nostack, preserves_flags));
        match ipa {
            Some(a) if local => core::arch::asm!(
                "dsb nshst", "tlbi ipas2e1, {}", "dsb nsh", "tlbi vmalle1", "dsb nsh", "isb",
                in(reg) (a >> 12) & 0xf_ffff_ffff, options(nostack, preserves_flags)),
            Some(a) => core::arch::asm!(
                "dsb ishst", "tlbi ipas2e1is, {}", "dsb ish", "tlbi vmalle1is", "dsb ish", "isb",
                in(reg) (a >> 12) & 0xf_ffff_ffff, options(nostack, preserves_flags)),
            None => core::arch::asm!("dsb ishst", "tlbi vmalls12e1is", "dsb ish", "isb",
                options(nostack, preserves_flags)),
        }
        core::arch::asm!("msr HCR_EL2, {}", "isb", in(reg) hcr, options(nostack, preserves_flags));
        core::arch::asm!("msr VTTBR_EL2, {}", "isb", in(reg) old_vttbr, options(nostack, preserves_flags));
    }
    crate::arch::irq_restore(irq);
}

/// Drop every translation of the VM (a fresh VM reuses the VMID).
pub fn flush_vm(root: u64) {
    flush_ipa(root, None);
}

/// The level-2 table under root entry `i`, made when `create`.
///
/// # Safety
/// `root` is this VM's live root; the caller serializes changes.
unsafe fn l2(root: u64, ipa: u64, create: bool) -> Option<*mut u64> {
    let i = (ipa / ONE_GB) as usize;
    if i >= ROOT_ENTRIES {
        return None;
    }
    // SAFETY: caller contract; identity-mapped tables.
    unsafe {
        let slot = (root as *mut u64).add(i);
        let e = slot.read_volatile();
        if e & VALID != 0 {
            return Some((e & ADDR_MASK) as *mut u64);
        }
        if !create {
            return None;
        }
        let t = table_frame()?;
        slot.write_volatile(t | VALID | TABLE);
        publish();
        Some(t as *mut u64)
    }
}

/// Build the stage-2 root of a guest of `guest_bytes`: the boot window at
/// host `host_base` (2 MiB aligned) as 2 MiB blocks, executable.
pub fn allocate_window(host_base: u64, guest_bytes: u64) -> Result<u64, &'static str> {
    if host_base & (TWO_MB - 1) != 0 || guest_bytes == 0 || guest_bytes & (TWO_MB - 1) != 0 {
        return Err("stage 2: unaligned guest RAM");
    }
    let layout = RamLayout::new(guest_bytes);
    if crate::microvm::devices::hostmem::gpa() < layout.top() {
        return Err("stage 2: guest RAM overlaps the host-visible window");
    }
    let root = memory::allocate_contiguous_aligned_below(ROOT_FRAMES, ROOT_FRAMES, u64::MAX)
        .ok_or("OOM: stage-2 root")?;
    zero(root, ROOT_FRAMES * 4096);
    let boot = boot_window_bytes(guest_bytes);
    let mut ipa = 0;
    while ipa < boot {
        // SAFETY: the root was just built here and is not live yet.
        let t = unsafe { l2(root, ipa, true) }.ok_or("OOM: stage-2 tables")?;
        // SAFETY: as above.
        unsafe { t.add(((ipa % ONE_GB) / TWO_MB) as usize).write_volatile((host_base + ipa) | RAM_LEAF) };
        ipa += TWO_MB;
    }
    publish();
    Ok(root)
}

/// Back the 4 KiB guest page at `gpa` in the demand region and return its
/// host address (`npt::demand_fault_in`). None outside the demand region or
/// without memory to spare.
pub fn demand_fault_in(root: u64, gpa: u64, guest_bytes: u64) -> Option<u64> {
    if gpa < boot_window_bytes(guest_bytes) || !RamLayout::new(guest_bytes).contains(gpa) {
        return None;
    }
    let in_block = gpa % TWO_MB & !0xfff;
    // SAFETY: this VM's root; the caller holds the fault-in lock.
    unsafe {
        let l2 = l2(root, gpa, true)?;
        let slot = l2.add(((gpa % ONE_GB) / TWO_MB) as usize);
        let e = slot.read_volatile();
        if e & VALID != 0 && e & TABLE == 0 {
            return Some((e & BLOCK_ADDR_MASK) + in_block);
        }
        let l3 = if e & VALID != 0 {
            (e & ADDR_MASK) as *mut u64
        } else {
            let spare = crate::microvm::devices::guest_mem::frames_spare();
            if spare > FRAMES_PER_2M {
                if let Some(block) = memory::allocate_contiguous_aligned(FRAMES_PER_2M, FRAMES_PER_2M) {
                    zero_clean(block, TWO_MB as usize);
                    slot.write_volatile(block | RAM_LEAF | XN);
                    publish();
                    DEMAND_2M.fetch_add(1, Ordering::Relaxed);
                    return Some(block + in_block);
                }
            }
            if spare < 2 {
                return None;
            }
            let t = table_frame()?;
            slot.write_volatile(t | VALID | TABLE);
            publish();
            DEMAND_4K_BLOCKS.fetch_add(1, Ordering::Relaxed);
            t as *mut u64
        };
        let idx = (in_block / 4096) as usize;
        let pte = l3.add(idx).read_volatile();
        if pte & VALID != 0 {
            return Some(pte & ADDR_MASK);
        }
        let spare = crate::microvm::devices::guest_mem::frames_spare();
        if spare == 0 {
            return None;
        }
        let frame = memory::allocate_frame()?;
        zero_clean(frame, 4096);
        l3.add(idx).write_volatile(frame | RAM_LEAF | TABLE | XN);
        // Fault-around, as `npt`: the rest of the 2 MiB block now.
        let mut left = spare - 1;
        for j in 0..512usize {
            if left == 0 { break; }
            if j == idx || l3.add(j).read_volatile() & VALID != 0 { continue; }
            left -= 1;
            let Some(f) = memory::allocate_frame() else { break };
            zero_clean(f, 4096);
            l3.add(j).write_volatile(f | RAM_LEAF | TABLE | XN);
        }
        publish();
        Some(frame)
    }
}

/// The guest fetched an instruction from `gpa`, which is mapped XN: make
/// the block or page executable after invalidating the instruction cache
/// (`user_mem_abort` with `exec_fault`). True if it was such a mapping.
pub fn exec_fault(root: u64, gpa: u64) -> bool {
    // SAFETY: this VM's root, read and then one entry rewritten; the
    // caller (the abort handler) serializes with fault-in.
    unsafe {
        let Some(l2) = l2(root, gpa, false) else { return false };
        let slot = l2.add(((gpa % ONE_GB) / TWO_MB) as usize);
        let e = slot.read_volatile();
        if e & VALID == 0 {
            return false;
        }
        let leaf = if e & TABLE == 0 {
            slot
        } else {
            (((e & ADDR_MASK) as *mut u64)).add(((gpa % TWO_MB) / 4096) as usize)
        };
        let v = leaf.read_volatile();
        if v & VALID == 0 {
            return false;
        }
        // Already executable: another vCPU relaxed it first and this core
        // still holds the old entry (`-EAGAIN`, flushed all the same).
        if v & XN != 0 {
            invalidate_icache();
            leaf.write_volatile(v & !XN);
            publish();
        }
        flush_ipa_local(root, gpa);
        true
    }
}

/// Free page reporting (`npt::demand_reclaim`): the demand pages of
/// `[gpa, gpa + len)` leave the guest, their frames and emptied tables go
/// to `freed`. Every removed entry is flushed before this returns.
pub fn demand_reclaim(root: u64, gpa: u64, len: u64, guest_bytes: u64, pinned: &dyn Fn(u64) -> bool,
                      freed: &mut alloc::vec::Vec<u64>) -> u64 {
    let layout = RamLayout::new(guest_bytes);
    let start = gpa.max(boot_window_bytes(guest_bytes)).next_multiple_of(4096);
    let end = gpa.saturating_add(len).min(layout.top()) & !0xfff;
    let mut taken = 0;
    let mut block = start & !(TWO_MB - 1);
    // SAFETY: this VM's root; the caller holds the fault-in lock.
    unsafe {
        while block < end {
            let (lo, hi) = (start.max(block), end.min(block + TWO_MB));
            let this = block;
            block += TWO_MB;
            if !layout.contains(this) { continue; }
            let Some(l2) = l2(root, this, false) else { continue };
            let slot = l2.add(((this % ONE_GB) / TWO_MB) as usize);
            let e = slot.read_volatile();
            if e & VALID == 0 { continue; }
            if e & TABLE == 0 {
                let host = e & BLOCK_ADDR_MASK;
                let whole = lo == this && hi == this + TWO_MB;
                if whole && !(0..512u64).any(|j| pinned(host + j * 4096)) {
                    slot.write_volatile(0);
                    flush_ipa(root, Some(this));
                    freed.extend((0..512u64).map(|j| host + j * 4096));
                    taken += 512;
                    DEMAND_2M.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                // Split the block into pages (break-before-make).
                let Some(t) = table_frame() else { continue };
                let tp = t as *mut u64;
                for j in 0..512u64 {
                    tp.add(j as usize).write_volatile((host + j * 4096) | RAM_LEAF | TABLE | (e & XN));
                }
                slot.write_volatile(0);
                flush_ipa(root, Some(this));
                slot.write_volatile(t | VALID | TABLE);
                publish();
                DEMAND_2M.fetch_sub(1, Ordering::Relaxed);
                DEMAND_4K_BLOCKS.fetch_add(1, Ordering::Relaxed);
            }
            let l3_phys = slot.read_volatile() & ADDR_MASK;
            let l3 = l3_phys as *mut u64;
            let mut p = lo;
            while p < hi {
                let ep = l3.add(((p % TWO_MB) / 4096) as usize);
                let pte = ep.read_volatile();
                if pte & VALID != 0 && !pinned(pte & ADDR_MASK) {
                    ep.write_volatile(0);
                    flush_ipa(root, Some(p));
                    freed.push(pte & ADDR_MASK);
                    taken += 1;
                }
                p += 4096;
            }
            if (0..512usize).all(|j| l3.add(j).read_volatile() & VALID == 0) {
                slot.write_volatile(0);
                flush_ipa(root, Some(this));
                freed.push(l3_phys);
                DEMAND_4K_BLOCKS.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
    taken
}

/// The level-3 entry of window page `gpa`, its tables made when `create`.
///
/// # Safety
/// `root` is this VM's live root; the caller serializes window changes.
unsafe fn window_leaf(root: u64, gpa: u64, create: bool) -> Option<*mut u64> {
    // SAFETY: caller contract; identity-mapped tables.
    unsafe {
        let l2 = l2(root, gpa, create)?;
        let slot = l2.add(((gpa % ONE_GB) / TWO_MB) as usize);
        let mut e = slot.read_volatile();
        if e & VALID == 0 {
            if !create { return None; }
            let t = table_frame()?;
            e = t | VALID | TABLE;
            slot.write_volatile(e);
            publish();
        }
        if e & TABLE == 0 {
            return None;
        }
        Some(((e & ADDR_MASK) as *mut u64).add(((gpa % TWO_MB) / 4096) as usize))
    }
}

/// Map host frame `host` at window page `gpa`, read/write, never
/// executable, with memory type `attr` (`MEM_*`).
pub fn window_map(root: u64, gpa: u64, host: u64, attr: u64) -> Result<(), &'static str> {
    // SAFETY: caller contract of `devices::hostmem`.
    unsafe {
        let leaf = window_leaf(root, gpa, true).ok_or("OOM: stage-2 window tables")?;
        if leaf.read_volatile() & VALID != 0 {
            return Err("window page already mapped");
        }
        leaf.write_volatile((host & ADDR_MASK) | VALID | TABLE | AF | SH_INNER | S2AP_RW | attr | XN);
        publish();
    }
    Ok(())
}

/// Clear window page `gpa` and flush it. True if it was mapped.
pub fn window_unmap(root: u64, gpa: u64) -> bool {
    // SAFETY: as in `window_map`.
    unsafe {
        let Some(leaf) = window_leaf(root, gpa, false) else { return false };
        let was = leaf.read_volatile() & VALID != 0;
        leaf.write_volatile(0);
        if was {
            flush_ipa(root, Some(gpa));
        }
        was
    }
}

pub fn window_mapped(root: u64, gpa: u64) -> bool {
    // SAFETY: as in `window_map`; read only.
    unsafe { window_leaf(root, gpa, false).is_some_and(|l| l.read_volatile() & VALID != 0) }
}

/// Map `len` bytes of device memory (the GIC's virtual CPU interface) at
/// `gpa`, page by page.
pub fn map_device(root: u64, gpa: u64, host: u64, len: u64) -> Result<(), &'static str> {
    let mut off = 0;
    while off < len {
        // SAFETY: this VM's root, before the guest runs.
        unsafe {
            let leaf = window_leaf(root, gpa + off, true).ok_or("OOM: stage-2 device tables")?;
            leaf.write_volatile(((host + off) & ADDR_MASK) | VALID | TABLE | AF | S2AP_RW | MEM_DEVICE | XN);
        }
        off += 4096;
    }
    publish();
    Ok(())
}

/// Free the demand frames, every table and the root; the boot window is
/// the caller's. Nothing runs the guest any more.
pub fn release(root: u64, guest_bytes: u64) {
    flush_vm(root);
    let layout = RamLayout::new(guest_bytes);
    let boot = boot_window_bytes(guest_bytes);
    let window = crate::microvm::devices::hostmem::gpa()..crate::microvm::devices::hostmem::gpa()
        + crate::microvm::devices::hostmem::size();
    // SAFETY: this VM's root; no vCPU and no device worker uses it.
    unsafe {
        for i in 0..ROOT_ENTRIES {
            let e = (root as *const u64).add(i).read_volatile();
            if e & VALID == 0 { continue; }
            let l2 = (e & ADDR_MASK) as *const u64;
            for j in 0..512usize {
                let ipa = i as u64 * ONE_GB + j as u64 * TWO_MB;
                let e2 = l2.add(j).read_volatile();
                if e2 & VALID == 0 { continue; }
                let ram = layout.contains(ipa) && ipa >= boot;
                if e2 & TABLE == 0 {
                    if ram {
                        memory::deallocate_contiguous(e2 & BLOCK_ADDR_MASK, FRAMES_PER_2M);
                    }
                    continue;
                }
                let l3 = (e2 & ADDR_MASK) as *const u64;
                // Window pages and the GIC belong to others; RAM pages are the guest's.
                if ram && !window.contains(&ipa) {
                    for k in 0..512usize {
                        let pte = l3.add(k).read_volatile();
                        if pte & VALID != 0 {
                            memory::deallocate_frame(pte & ADDR_MASK);
                        }
                    }
                }
                memory::deallocate_frame(e2 & ADDR_MASK);
            }
            memory::deallocate_frame(e & ADDR_MASK);
        }
    }
    memory::deallocate_contiguous(root, ROOT_FRAMES);
}
