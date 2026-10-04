# `kernel/src/microvm/cpu/vmx/ept.rs` @ 5e0102684

## L1-30 · `use crate::mm::memory;`

```
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
```

## L34 · `const EPT_R: u64 = 1 << 0;`

```
// EPT entry permission + attribute bits.
```

## L39 · `const EPT_MEM_TYPE_WB: u64 = 6 << 3;`

```
// Memory type for leaf entries: 6 = WB (write-back).
```

## L41 · `const EPT_LEAF: u64 = 1 << 7;`

```
// Bit 7: leaf entry (page) vs pointer to next-level table.
```

## L44 · `const EPTP_MEM_TYPE_WB: u64 = 6;`

```
// EPTP fields VMWRITE'd into VMCS::EPT_POINTER.
```

## L46 · `const EPTP_WALK_LENGTH_4: u64 = 3 << 3; // 4 levels = walk length 3`

```
// 4 levels = walk length 3
```

## L50-53 · `const MAX_GUEST_BYTES: u64 = 3 * ONE_GB;`

```
/// Maximum guest RAM we can map: PDPT slots [0, 1, 2] each cover 1 GiB
/// of guest physical, PDPT[3] is reserved for the MMIO scratch range
/// (IOAPIC/HPET/LAPIC). 3 GiB hard cap; bumping past that needs either
/// rearranging the MMIO hole or growing the EPT to a second PDPT.
```

## L55-57 · `const GUEST_RAM_ALIGN_SLACK: usize = 511;`

```
// Canonical size lives in `guest_mem`; this is its EPT-window twin.
/// Slack frames `boot_frames_for` adds so `allocate_contiguous` can be
/// rounded up to a 2-MB boundary for the boot-window 2-MB leaves.
```

## L60 · `const EPT_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;`

```
/// Bits [51:12] of an EPT entry hold the next-level / page phys addr.
```

## L63-69 · `pub const BOOT_WINDOW_BYTES: u64 = 256 * 1024 * 1024;`

```
/// B3 hybrid split: `[0, BOOT_WINDOW_BYTES)` is one contiguous 2-MB-
/// leaf block (fast boot, no faults on the early path, covers the
/// kernel image + initramfs @ 0x0C00_0000 + boot_params); everything
/// above is demand-paged 4 KB on EPT-violation. 256 MiB is reliably
/// allocatable even on a fragmented host (the 1 GiB contiguous alloc
/// was the fragmentation problem); the lazy region is the page cache
/// / browser heap, the bulk on a ≥1 GiB guest.
```

## L72-73 · `pub fn round_up_to_2mb(raw_base: u64) -> u64 {`

```
/// Round a raw `allocate_contiguous` base up to the next 2-MB
/// boundary so it can be passed to `install_window`.
```

## L78-81 · `pub fn boot_window_bytes(guest_bytes: u64) -> u64 {`

```
/// Contiguous boot window size for a guest of `guest_bytes`
/// (= `min(BOOT_WINDOW_BYTES, guest_bytes)`, 2-MB-aligned). A guest
/// ≤ 256 MiB (small-host B2 policy) is fully contiguous → no demand
/// region, behaviour identical to B2.
```

## L86-87 · `guest_bytes`

```
// Demand off → whole guest contiguous (boot_leaves ==
// guest_leaves, no demand PTs): exactly B2/A2.
```

## L92-94 · `pub fn boot_frames_for(guest_bytes: u64) -> usize {`

```
/// 4 KB host frames to allocate **contiguously** for the boot window
/// (+ 2-MB-align slack). The demand region needs NO upfront
/// allocation — only touched 4 KB pages are committed on fault.
```

## L99-115 · `pub fn install_window(boot_base: u64, guest_bytes: u64) -> Result<(u64, u64), &'static str> {`

```
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
```

## L128-129 · `let num_pds = ((guest_bytes + ONE_GB - 1) / ONE_GB) as usize;`

```
// One PD covers 1 GiB (512 × 2-MB leaves). Allocate as many PDs
// as the guest needs, chained off PDPT[0..num_pds).
```

## L138-141 · `let lapic_trap = crate::microvm::cpu::GUEST_LAPIC && crate::microvm::cpu::VMX_GUEST_LAPIC;`

```
// Trap the guest LAPIC page (0xFEE00000) into the emulator (vmx::lapic,
// Intel parity #2) by leaving it EPT-not-present. Only when LAPIC
// emulation is on; otherwise keep the old aliased scratch (byte-
// identical rollback for the `nolapic` boot). Mirrors svm/npt.rs.
```

## L148 · `unsafe {`

```
// SAFETY: identity-mapped, freshly allocated, exclusive.
```

## L156 · `for p in 0..num_pds {`

```
// PDPT[0..num_pds] → guest-RAM PDs (each covers 1 GiB).
```

## L160 · `pdpt.add(3).write_volatile(pd_high_phys | EPT_RWX);`

```
// PDPT[3] → PD_HIGH (MMIO scratch lives in [3 GiB, 4 GiB)).
```

## L163 · `const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512`

```
// Populate each PD: entries [0..512) cover [p*1GiB, (p+1)*1GiB).
```

## L164 · `const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512`

```
// 512
```

## L184-190 · `let pd_high = pd_high_phys as *mut u64;`

```
// PD_HIGH[502] → PT_DUMMY (covers [0xFEC00000, 0xFEE00000):
// IOAPIC + HPET, scratch). PD_HIGH[503] covers [0xFEE00000,
// 0xFF000000) = the LAPIC: when emulating, point it at a SEPARATE
// PT_LAPIC whose first page (0xFEE00000) is left not-present so the
// guest's LAPIC MMIO EPT-faults into vmx::lapic; the rest stays
// scratch. When not emulating, alias it to PT_DUMMY like before
// (byte-identical scratch for the `nolapic` boot).
```

## L205-206 · `pt_dummy.add(0).write_volatile(0);`

```
// [0]: the I/O APIC page (0xFEC00000) NOT-PRESENT → trap and emulate
// (`devices::ioapic`).
```

## L209-213 · `let pt_lapic = pt_lapic_phys as *mut u64;`

```
// PT_LAPIC: entry [0] = the LAPIC MMIO page (0xFEE00000) left
// NOT-PRESENT → guest LAPIC accesses EPT-violate → trap-and-emulate
// (vmx::lapic). The rest of the 2 MB → dummy scratch (harmless if
// ever touched). Only consulted when `lapic_trap` (PD_HIGH[503]
// points here); built unconditionally — a leaked frame otherwise.
```

## L216 · `pt_lapic.add(0).write_volatile(0); // LAPIC page: trap on access`

```
// LAPIC page: trap on access
```

## L227-234 · `pub fn demand_fault_in(pml4_phys: u64, gpa: u64) -> Option<u64> {`

```
/// Walk PML4[0]→PDPT[0]→PD→PT for a demand-region `gpa` and return
/// the host phys of its 4 KB page, allocating + mapping a zeroed
/// frame on first touch. Called from the EPT-violation handler
/// (guest fault) AND `GuestMem` (host DMA to an untouched page) —
/// idempotent: an already-present PTE returns its existing frame.
/// `gpa` MUST be ≥ the boot window (caller guarantees; the boot
/// region is 2-MB leaves, never PT-walked). No INVEPT needed: the
/// entry was not-present so there is no stale TLB for it.
```

## L236 · `unsafe {`

```
// SAFETY: our own freshly-built EPT, identity-mapped tables.
```

## L240 · `let pdpt_idx = (gpa / ONE_GB) as usize;`

```
// B4: PDPT[0..3] holds the guest-RAM PDs (1 GiB each); pick by gpa.
```

## L243 · `return None; // PDPT[3] is the MMIO hole, never demand-faulted`

```
// PDPT[3] is the MMIO hole, never demand-faulted
```

## L247 · `return None; // PD not present (gpa beyond advertised guest_bytes)`

```
// PD not present (gpa beyond advertised guest_bytes)
```

## L253 · `return None; // outside demand region / unexpected 2-MB leaf`

```
// outside demand region / unexpected 2-MB leaf
```

## L259 · `return Some(pte & EPT_ADDR_MASK); // already faulted in`

```
// already faulted in
```

## L265-268 · `for j in 0..512usize {`

```
// Fault-around: back the rest of this 2 MB block now, so a guest
// walking fresh memory takes one exit per 2 MB, not one per 4 KB
// (KVM gets the same from a THP-backed memslot). Stops quietly when
// the allocator runs dry; the remaining pages fault in singly.
```

## L279-282 · `pub fn release(pml4_phys: u64, guest_bytes: u64) {`

```
/// Free everything `install_window` allocated for `guest_bytes`: every
/// demand-faulted 4 KB frame, the demand PT pages, and the four fixed
/// tables (PML4/PDPT/PD/PD_HIGH/PT_DUMMY/dummy). The contiguous boot
/// block is freed separately by the caller (it owns `boot_raw_base`).
```

## L287 · `const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512`

```
// 512
```

## L288-289 · `unsafe {`

```
// SAFETY: our own EPT; nothing else references these frames after
// VMXOFF + window teardown.
```

## L294-295 · `for p in 0..num_pds {`

```
// Walk PDPT[0..num_pds] and free each guest-RAM PD + its demand
// PTs + demand frames.
```

## L325 · `let dummy = (pt_dummy_phys as *const u64).add(1).read_volatile() & EPT_ADDR_MASK;`

```
// [0] is the trapped I/O APIC page; [1] maps the scratch page.
```

