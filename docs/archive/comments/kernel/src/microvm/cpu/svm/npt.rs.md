# `kernel/src/microvm/cpu/svm/npt.rs` @ 5e0102684

## L1-32 · `use crate::mm::memory;`

```
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
```

## L36-37 · `const GUEST_RAM_ALIGN_SLACK: usize = 511;`

```
/// Slack frames `boot_frames_for` adds so `allocate_contiguous` can be
/// rounded up to a 2 MB boundary. Mirrors `ept`.
```

## L40 · `const NPT_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;`

```
/// Bits [51:12] of an NPT entry hold the next-level / page phys addr.
```

## L45-47 · `const MAX_GUEST_BYTES: u64 = 3 * ONE_GB;`

```
/// Maximum guest RAM we can map: PDPT slots [0, 1, 2] hold guest-RAM
/// PDs (1 GiB each), PDPT[3] is reserved for the MMIO scratch range.
/// Mirrors `ept::MAX_GUEST_BYTES`.
```

## L49 · `const GUEST_WINDOW_BYTES: u64 = crate::microvm::devices::guest_mem::GUEST_RAM_BYTES;`

```
// Canonical size lives in `guest_mem`; this is its NPT-window twin.
```

## L52 · `const NPT_P: u64 = 1 << 0;`

```
// ── NPT page-table flags ───────────────────────────────────────────
```

## L54 · `const NPT_P: u64 = 1 << 0;`

```
/// Present.
```

## L56 · `const NPT_RW: u64 = 1 << 1;`

```
/// Writable.
```

## L58-60 · `const NPT_US: u64 = 1 << 2;`

```
/// User-mode accessible. Must be set in NPT entries — otherwise the
/// CPU treats the page as kernel-only, and any guest access NPT-
/// faults with a permission mismatch (APM §15.25.6).
```

## L62-63 · `const NPT_PS: u64 = 1 << 7;`

```
/// Page Size — leaf entries at PD level (2 MB pages). Cleared at
/// PML4 + PDPT (those point to the next level).
```

## L66 · `pub const BOOT_WINDOW_BYTES: u64 = 256 * 1024 * 1024;`

```
/// Contiguous boot window; mirrors `ept`. See `ept::BOOT_WINDOW_BYTES`.
```

## L69-70 · `pub fn round_up_to_2mb(raw_base: u64) -> u64 {`

```
/// Round a raw `allocate_contiguous` base up to the next 2 MB
/// boundary so it can be passed to `allocate_window_npt`.
```

## L75 · `pub fn boot_window_bytes(guest_bytes: u64) -> u64 {`

```
/// Contiguous boot window size for `guest_bytes`; mirrors `ept`.
```

## L80 · `guest_bytes`

```
// Demand off → whole guest contiguous: exactly B2/A2.
```

## L85-87 · `pub fn boot_frames_for(guest_bytes: u64) -> usize {`

```
/// 4 KB host frames to allocate **contiguously** for the boot window
/// (+ 2-MB-align slack). The demand region is faulted 4 KB at a time;
/// mirrors `ept::boot_frames_for`.
```

## L92-97 · `pub fn allocate_identity_npt() -> Result<u64, &'static str> {`

```
/// Build a fresh NPT root that identity-maps `0..256 MB` of guest
/// physical to host physical via 2 MB pages. Returns the physical
/// address of the PML4 page, suitable for VMCB.NCR3.
///
/// Allocates 3 frames per call (PML4 + PDPT + PD). Frames are leaked
/// alongside the rest of the per-call substrate-test allocations.
```

## L99 · `build_npt(0, GUEST_WINDOW_BYTES, /* with_mmio_scratch */ false)`

```
// Substrate test: fixed 1 GiB, behaviour unchanged by B2.
```

## L100 · `build_npt(0, GUEST_WINDOW_BYTES, /* with_mmio_scratch */ false)`

```
/* with_mmio_scratch */
```

## L103-109 · `pub fn allocate_window_npt(host_base: u64, guest_bytes: u64) -> Result<u64, &'static str> {`

```
/// Build a fresh NPT root that maps `0..256 MB` of guest physical to
/// host physical `host_base..host_base+256 MB` via 2 MB pages, plus a
/// scratch alias for [0xFEC00000, 0xFF000000) (IOAPIC + HPET +
/// LAPIC). Returns NCR3.
///
/// `host_base` must be 2 MB aligned. Allocates 6 frames (PML4 + PDPT
/// + PD + PD_HIGH + PT_DUMMY + dummy_page). Frames are leaked.
```

## L114 · `build_npt(host_base, guest_bytes, /* with_mmio_scratch */ true)`

```
/* with_mmio_scratch */
```

## L117-119 · `fn build_npt(host_base: u64, guest_bytes: u64, with_mmio_scratch: bool) -> Result<u64, &'static str> {`

```
/// Inner builder. `host_base = 0` gives identity mapping; non-zero
/// shifts the leaf addresses by `host_base`. Maps exactly
/// `guest_bytes` (single PD, ≤ 1 GiB; B4 adds multi-PD).
```

## L128-129 · `let boot_leaves = if host_base == 0 {`

```
// Identity substrate test (host_base = 0) stays all-2-MB; the real
// window path is hybrid (contiguous boot + 4-KB demand region).
```

## L144-146 · `unsafe {`

```
// SAFETY: freshly allocated, identity-mapped (host paging),
// exclusive. All pages are 4 KB aligned (frame allocator
// guarantee).
```

## L154 · `let pml4 = pml4_phys as *mut u64;`

```
// PML4[0] → PDPT (non-leaf, no PS bit).
```

## L158 · `let pdpt = pdpt_phys as *mut u64;`

```
// PDPT[0..num_pds] → guest-RAM PDs (each covers 1 GiB).
```

## L164 · `const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512`

```
// Populate each PD: [base_leaf, end_leaf) in this PD's window.
```

## L165 · `const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512`

```
// 512
```

## L186 · `let pd_high_phys = memory::allocate_frame()`

```
// PDPT[3] → PD_HIGH (covers [3 GB, 4 GB), MMIO range).
```

## L203-206 · `let pd_high = pd_high_phys as *mut u64;`

```
// PD_HIGH[502] → PT_DUMMY (covers [0xFEC00000, 0xFEE00000):
// IOAPIC/HPET). PD_HIGH[503] → PT_LAPIC (covers
// [0xFEE00000, 0xFF000000): the local APIC). Separate PTs so
// we can punch a single not-present page at the LAPIC base.
```

## L211 · `let pt_dummy = pt_dummy_phys as *mut u64;`

```
// PT_DUMMY[0..512] all → dummy_page. 2 MB → 4 KB scratch.
```

## L216-217 · `pt_dummy.add(0).write_volatile(0);`

```
// [0]: the I/O APIC page (0xFEC00000) NOT-PRESENT → trap and
// emulate (`devices::ioapic`), like the LAPIC page below.
```

## L219-222 · `let pt_lapic = pt_lapic_phys as *mut u64;`

```
// PT_LAPIC: entry [0] = the LAPIC MMIO page (0xFEE00000) left
// NOT-PRESENT → guest LAPIC accesses #NPF → trap-and-emulate
// (svm::lapic, guest-SMP Stage 1). The rest of the 2 MB →
// dummy scratch (harmless if ever touched).
```

## L224 · `pt_lapic.add(0).write_volatile(0); // LAPIC page: trap on access`

```
// LAPIC page: trap on access
```

## L234-239 · `pub fn demand_fault_in(pml4_phys: u64, gpa: u64) -> Option<u64> {`

```
/// Walk PML4[0]→PDPT[0]→PD→PT for a demand-region `gpa`; allocate +
/// map a zeroed 4 KB frame on first touch, return its host phys.
/// Idempotent. Called from the #NPF handler AND `GuestMem` (host DMA
/// to an untouched page). `gpa` MUST be ≥ the boot window. No
/// INVLPGA needed: the entry was not-present (no stale TLB).
/// Mirrors `ept::demand_fault_in`.
```

## L241 · `unsafe {`

```
// SAFETY: our own freshly-built NPT, identity-mapped tables.
```

## L245 · `let pdpt_idx = (gpa / ONE_GB) as usize;`

```
// B4: pick the right guest-RAM PD from PDPT[0..3] by gpa.
```

## L248 · `return None; // PDPT[3] is MMIO, never demand-faulted`

```
// PDPT[3] is MMIO, never demand-faulted
```

## L252 · `return None; // PD not present (gpa beyond advertised guest_bytes)`

```
// PD not present (gpa beyond advertised guest_bytes)
```

## L258 · `return None; // outside demand region / unexpected 2-MB leaf`

```
// outside demand region / unexpected 2-MB leaf
```

## L264 · `return Some(pte & NPT_ADDR_MASK); // already faulted in`

```
// already faulted in
```

## L270-273 · `for j in 0..512usize {`

```
// Fault-around: back the rest of this 2 MB block now, so a guest
// walking fresh memory takes one exit per 2 MB, not one per 4 KB
// (KVM gets the same from a THP-backed memslot). Stops quietly when
// the allocator runs dry; the remaining pages fault in singly.
```

## L284-286 · `pub fn release(pml4_phys: u64, guest_bytes: u64) {`

```
/// Free every demand-faulted 4 KB frame + the demand PT pages + the
/// fixed tables. The contiguous boot block is freed by the caller.
/// Mirrors `ept::release`.
```

## L291 · `const LEAVES_PER_PD: u64 = ONE_GB / TWO_MB; // 512`

```
// 512
```

## L292 · `unsafe {`

```
// SAFETY: our own NPT; nothing references these after teardown.
```

## L326 · `let dummy = (pt_dummy_phys as *const u64).add(1).read_volatile() & NPT_ADDR_MASK;`

```
// [0] is the trapped I/O APIC page; [1] maps the scratch page.
```

