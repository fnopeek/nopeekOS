# `kernel/src/boot_info.rs` @ 5e0102684

## L1-9 · `#![allow(dead_code)]`

```
//! Boot info handed off from the UEFI stub (`boot_uefi`) to
//! `kernel_main`. Decoupled from `boot_uefi` so the kernel proper
//! doesn't pull in UEFI ABI types; it just sees opaque memory regions
//! tagged with their original UEFI type id.
//!
//! The UEFI stub populates this in static storage before
//! `ExitBootServices`, then passes a `&'static BootInfo` to
//! `kernel_main`. After ExitBootServices the entire UEFI ABI surface
//! is gone; only this struct's contents survive.
```

## L15-17 · `pub const UEFI_RESERVED: u32          = 0;`

```
// UEFI memory type constants (UEFI Spec §7.2). Mirror them here so
// modules outside the boot stub (memory.rs, framebuffer.rs, acpi.rs)
// don't have to import from boot_uefi.
```

## L47-49 · `pub fn is_usable(&self) -> bool {`

```
/// Whether this region is general-purpose RAM the kernel can hand
/// out via its frame allocator. Boot-services memory is freed for
/// us by ExitBootServices, so it counts too.
```

## L62-63 · `pub acpi_rsdp: u64,`

```
/// Physical address of the ACPI 2.0 RSDP (RSDT/XSDT entrypoint).
/// `0` if firmware exposes no ACPI tables.
```

## L66 · `pub fb_base: u64,`

```
// Framebuffer (direct linear, identity-mapped by UEFI in low memory).
```

