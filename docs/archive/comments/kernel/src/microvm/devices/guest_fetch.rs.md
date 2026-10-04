# `kernel/src/microvm/devices/guest_fetch.rs` @ 5e0102684

## L1-17 · `#![allow(dead_code)]`

```
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
```

## L25-30 · `pub fn fetch_inst(`

```
/// Walk guest page tables to translate `rip` (guest virtual) into a
/// guest physical address, then return up to 15 bytes from there.
/// `cr3` is the guest's PML4 base. `mem` owns gpa→host translation.
///
/// Returns None on any walk failure (page not present, instruction
/// crossing a page boundary, GPA outside the window).
```

## L47 · `if pdpte & 0x80 != 0 {`

```
// 1 GB page (PS bit at PDPT level)
```

## L58 · `if pde & 0x80 != 0 {`

```
// 2 MB page (PS bit at PD level)
```

## L74-80 · `let page_off = guest_phys & 0xFFF;`

```
// x86 instructions are at most 15 bytes. If the instruction starts
// late enough in a page that 15 bytes would cross a 4 KB boundary,
// we'd need to do a second walk for the next page. Bail for now —
// Linux's MMIO accessors don't generate such instructions in
// practice (each is an aligned `mov`, ~3-7 bytes, well within a
// single page). This single-page guarantee also keeps the
// `GuestMem` read within one (B3) demand-paged frame.
```

