# `kernel/src/tss.rs` @ 5e0102684

## L1-14 · `use core::sync::atomic::{AtomicBool, Ordering};`

```
//! Task State Segment install — Phase 12.1.0d-2a prerequisite for VMLAUNCH.
//!
//! The boot GDT (`boot.s :: gdt64`) is three entries — null, 64-bit
//! code, 64-bit data — and the kernel never executed `ltr`, so TR=0.
//! VMX host-state validation rejects HOST_TR_SELECTOR=0 at VMLAUNCH
//! (SDM Vol. 3C §26.2.3). This module clones the boot GDT into BSS,
//! appends a 16-byte long-mode TSS descriptor, `lgdt`s the new GDT,
//! and `ltr`s the new TSS selector. Single-CPU (BSP-only) — APs keep
//! the boot GDT, which is fine since VMX runs only on Core 0.
//!
//! Reference: Intel SDM Vol. 3A §3.4.5.1 (Code- and Data-Segment
//! Descriptor Types), §7.7 (Task Management in 64-bit Mode);
//! Vol. 3C §26.2.3 (Checks on Host Segment and Descriptor-Table
//! Registers).
```

## L18-21 · `pub const MAX_CORES: usize = 16;`

```
/// Maximum cores we install a per-core TSS for (`ensure_core`). The test
/// boxes are ≤8 logical CPUs; 16 leaves headroom without bloating .data.
/// A core id ≥ this is a no-op (it just won't run a VMX guest — only the
/// fiber-mode worker cores need their own TR).
```

## L24-26 · `#[repr(C, packed)]`

```
/// Long-mode TSS layout (104 bytes minimum, no I/O bitmap). RSP0/1/2
/// and IST1..IST7 stay zero — we don't use ring transitions or IST
/// stacks today. I/O map base = 104 means "no I/O bitmap follows".
```

## L60-61 · `fn tss_descriptor(tss_base: u64) -> (u64, u64) {`

```
/// Build the 16-byte long-mode TSS descriptor (two GDT slots) for a TSS
/// at `tss_base`. Returns `(lo, hi)`. SDM Vol. 3A §7.2.3.
```

## L67 · `let access: u64 = 0x89; // P=1, DPL=0, S=0, type=9 (available 64-bit TSS)`

```
// P=1, DPL=0, S=0, type=9 (available 64-bit TSS)
```

## L68 · `let granularity: u64 = 0; // G=0, AVL=0, limit[19:16]=0`

```
// G=0, AVL=0, limit[19:16]=0
```

## L80 · `0,                       // null`

```
// null
```

## L81 · `0x00AF_9A00_0000_FFFF,   // code (0x08, ring0, L=1, P=1, type=0xA)`

```
// code (0x08, ring0, L=1, P=1, type=0xA)
```

## L82 · `0x00CF_9200_0000_FFFF,   // data (0x10, ring0, P=1, type=0x2)`

```
// data (0x10, ring0, P=1, type=0x2)
```

## L83 · `0,                       // TSS desc lo  — filled at install`

```
// TSS desc lo  — filled at install
```

## L84 · `0,                       // TSS desc hi  — filled at install`

```
// TSS desc hi  — filled at install
```

## L89-99 · `#[unsafe(link_section = ".data")]`

```
// ── Per-core (AP) TSS+GDT for fiber-mode VMX ────────────────────────
//
// VMX rejects HOST_TR_SELECTOR=0 at VM-entry (SDM §26.2.3). The BSP gets
// its TR from `init()` at boot; AP/worker cores keep the boot GDT with
// TR=0. That was fine while VMX ran only on Core 0, but fiber-mode runs
// the guest's VMRESUME loop on a worker core — so that worker needs its
// OWN valid TSS + TR before `write_host_state`. Each core gets a private
// TSS + 5-slot GDT (the busy bit `ltr` sets means cores cannot share one
// TSS descriptor). SVM (vmsave) has no such host-state check, so the AMD
// path never needed this — and never calls `ensure_core` (it opens via
// `svm::vm_open`, not `vmx::vm_open`).
```

## L110-115 · `pub fn ensure_core(core_id: usize) {`

```
/// Install a private TSS + GDT on the CURRENT core and `ltr` it, so VMX
/// host-state has a valid HOST_TR when the guest runs as a fiber on this
/// worker core. Idempotent per core. No-op for core 0 (the BSP already
/// `ltr`'d via `init()` at boot — must not clobber its GDT) and for ids
/// past `MAX_CORES`. Mirrors `init()` but per-core. Call on the worker
/// core that is about to `vm_open` a VMX guest.
```

## L123-126 · `unsafe {`

```
// SAFETY: each core writes only its own slot (disjoint), this core is
// the only one that ever `ltr`s AP_GDT[core_id]. lgdt keeps CS/SS/DS
// valid because slots 1+2 match the boot GDT byte-for-byte; we never
// reload the segment registers.
```

## L148-151 · `#[unsafe(link_section = ".data")]`

```
/// 5-slot GDT: null (0), code (1, 0x08), data (2, 0x10), TSS-lo (3,
/// 0x18), TSS-hi (4). Initial values for slots 1+2 mirror `boot.s ::
/// gdt64_code/_data`; slots 3+4 are filled at runtime once we know
/// the TSS virtual address.
```

## L154 · `0,                       // null`

```
// null
```

## L155 · `0x00AF_9A00_0000_FFFF,   // code (0x08, ring0, L=1, P=1, type=0xA)`

```
// code (0x08, ring0, L=1, P=1, type=0xA)
```

## L156 · `0x00CF_9200_0000_FFFF,   // data (0x10, ring0, P=1, type=0x2)`

```
// data (0x10, ring0, P=1, type=0x2)
```

## L157 · `0,                       // TSS desc lo  — filled in init()`

```
// TSS desc lo  — filled in init()
```

## L158 · `0,                       // TSS desc hi  — filled in init()`

```
// TSS desc hi  — filled in init()
```

## L161 · `#[repr(C, packed)]`

```
/// 10-byte pseudo-descriptor consumed by `lgdt`. Filled at init time.
```

## L171 · `const TSS_SELECTOR: u16 = 3 << 3;`

```
/// TSS selector for `ltr`: index 3, TI=0, RPL=0.
```

## L176-177 · `pub fn init() {`

```
/// Install the TSS and switch to the cloned GDT. Idempotent; calling
/// twice is a no-op so accidental double-invocation is harmless.
```

## L186-188 · `unsafe {`

```
// SAFETY: BSP boot path, single-threaded relative to GDT/TSS
// statics (APs are already running but never touch these symbols).
// Writes are confined to BSS-resident memory we own exclusively.
```

## L198-200 · `core::arch::asm!(`

```
// lgdt loads from a memory operand. Then ltr loads the TSS.
// The CS/SS/DS/ES/FS/GS selectors stay valid because slots 1
// and 2 of the new GDT match the boot GDT byte-for-byte.
```

