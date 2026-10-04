# `kernel/src/boot_uefi.rs` @ 5e0102684

## L1-11 · `#![allow(dead_code)]`

```
//! UEFI boot stub — what `_start` (in boot.s) hands control to.
//!
//! Lives outside the rest of the kernel: this is the only code that
//! talks to UEFI Boot Services. Once `ExitBootServices` succeeds we
//! own the machine and the SystemTable's BootServices pointer becomes
//! invalid — at that point we synthesize a [`BootInfo`] and hand off
//! to `kernel_main` (Phase 3; Phase 2 just collects + halts).
//!
//! Reference: UEFI Specification 2.10 §4 (System Table), §7 (Services
//! — Boot Services), §11 (Protocols — Graphics Output Protocol), §12
//! (Protocols — Console), §15 (Memory Allocation Services).
```

## L19 · `pub type EfiStatus = usize;`

```
// ── Primitive UEFI types ───────────────────────────────────────────
```

## L41 · `pub const EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID: EfiGuid = EfiGuid {`

```
// GUIDs we care about — these are spec-defined byte patterns.
```

## L55 · `#[repr(C)]`

```
// ── System / Boot Services tables ──────────────────────────────────
```

## L73 · `}`

```
// rest of fn ptrs omitted — we only print
```

## L82-85 · `#[repr(C)]`

```
/// EFI_BOOT_SERVICES — UEFI spec §7. Only the function pointers we
/// actually call are typed; everything else is `usize` placeholder
/// to keep the layout right. Adding more is mechanical: count entries
/// from spec and bump placeholders.
```

## L90 · `pub raise_tpl: usize,`

```
// Task Priority Services
```

## L94 · `pub allocate_pages: usize,`

```
// Memory Services
```

## L111 · `pub create_event: usize,`

```
// Event & Timer Services
```

## L119 · `pub install_protocol_interface: usize,`

```
// Protocol Handler Services
```

## L130 · `pub load_image: usize,`

```
// Image Services
```

## L140 · `pub get_next_monotonic_count: usize,`

```
// Miscellaneous Services (partial — we don't use these in Phase 2)
```

## L145 · `pub connect_controller: usize,`

```
// DriverSupport Services
```

## L149 · `pub open_protocol: usize,`

```
// Open and Close Protocol Services
```

## L154 · `pub protocols_per_handle: usize,`

```
// Library Services
```

## L162 · `}`

```
// ... rest omitted
```

## L170 · `pub console_in_handle: EfiHandle,`

```
// Implicit 4-byte padding (alignment of next ptr).
```

## L183 · `#[repr(C)]`

```
// ── Memory map ─────────────────────────────────────────────────────
```

## L200 · `pub const EFI_LOADER_DATA_POOL: u32 = 2; // for AllocatePool type arg`

```
// for AllocatePool type arg
```

## L202 · `#[repr(C)]`

```
// ── Graphics Output Protocol ───────────────────────────────────────
```

## L250-252 · `use crate::boot_info::{BootInfo, MemoryRegion, MAX_MEMORY_REGIONS};`

```
// BootInfo / MemoryRegion live in `crate::boot_info` — same types the
// kernel proper sees. The UEFI stub populates them before
// ExitBootServices; everything downstream is firmware-independent.
```

## L255 · `#[inline(always)]`

```
// ── COM1 raw debug print (survives ExitBootServices) ───────────────
```

## L260-261 · `core::arch::asm!(`

```
// COM2 (0x2F8) → -serial file:target/serial.log. Unbuffered.
// Write FIRST so the byte hits disk even if COM1 stalls.
```

## L266-267 · `core::arch::asm!(`

```
// COM1 (0x3F8) → -serial mon:stdio (interactive). QEMU's UART
// has effectively unbounded buffer; skip the LSR-empty poll.
```

## L306-309 · `static mut BOOT_INFO: BootInfo = BootInfo::empty();`

```
// ── BootInfo storage ───────────────────────────────────────────────
//
// Allocated in .bss so it survives the transition from UEFI-managed
// memory to our own. Single instance, leaked into kernel_main.
```

## L313 · `unsafe fn find_acpi_rsdp(system_table: *mut EfiSystemTable) -> u64 {`

```
// ── ACPI RSDP discovery ────────────────────────────────────────────
```

## L328 · `if rsdp_v2 != 0 { rsdp_v2 } else { rsdp_v1 }`

```
// Prefer ACPI 2.0+ if present; fall back to legacy.
```

## L332 · `unsafe fn locate_gop(bs: *mut EfiBootServices) -> Option<&'static EfiGraphicsOutputProtocol> {`

```
// ── Graphics Output ────────────────────────────────────────────────
```

## L347-353 · `#[repr(C, align(8))]`

```
// ── Memory map collection ──────────────────────────────────────────
//
// UEFI GetMemoryMap is "tell me how big a buffer to give you" + "now
// fill it". The buffer must come from AllocatePool because the map
// can change between calls. After ExitBootServices we have no
// allocator — but we copy regions into the static BootInfo.regions
// array before exiting, so it's fine.
```

## L359-363 · `unsafe fn collect_memory_map(bs: *mut EfiBootServices) -> Uintn {`

```
/// Fill `BOOT_INFO.regions` from the UEFI memory map. Also returns
/// the `map_key` that must be passed to `ExitBootServices`. The map
/// changes when any allocator call runs between this and ExitBootServices,
/// so the caller must invoke them in the right order (we don't do any
/// allocation after this).
```

## L389-391 · `let n = map_size / desc_size;`

```
// Walk descriptors. Note `desc_size` may be larger than
// sizeof(EfiMemoryDescriptor) — vendor extensions are allowed.
// We stride by desc_size in bytes, casting each.
```

## L412-413 · `unsafe fn print_mem_summary() {`

```
/// Diagnostic dump of usable / total memory from the populated
/// BOOT_INFO.regions[].
```

## L439 · `#[unsafe(no_mangle)]`

```
// ── Entry from boot.s ──────────────────────────────────────────────
```

## L453 · `let rsdp = unsafe { find_acpi_rsdp(system_table) };`

```
// 1. ACPI RSDP via ConfigurationTable walk.
```

## L463 · `if let Some(gop) = unsafe { locate_gop(bs) } {`

```
// 2. Graphics Output Protocol → framebuffer.
```

## L490-491 · `let map_key = unsafe { collect_memory_map(bs) };`

```
// 3. Memory map — this is also the last UEFI call before
// ExitBootServices; map_key must come from this snapshot.
```

## L496-498 · `let status = unsafe { ((*bs).exit_boot_services)(image_handle, map_key) };`

```
// 4. ExitBootServices. After this call UEFI Boot Services are
// gone — no more LocateProtocol, no more printk through ConOut.
// We're on our own from here.
```

## L501-504 · `unsafe { core::arch::asm!("cli", options(nomem, nostack)); }`

```
// Now that UEFI's IDT is no longer the active one (we still
// share it but boot services are dismantled), gate IRQs off
// until our own IDT loads in interrupts::init. boot.s left IF=1
// because Boot Services need timer IRQs to make progress.
```

## L511-513 · `com1_print(b"[uefi] retrying with fresh memory map...\n");`

```
// Retry once with a fresh map — common when boot services
// allocated/freed pages behind our back between GetMemoryMap
// and ExitBootServices.
```

## L529-532 · `unsafe { install_kernel_gdt() };`

```
// Install our own GDT now. Doing this BEFORE ExitBootServices hangs
// the firmware mid-call — Boot Services internally relies on
// UEFI's selector layout (TR/LDT/code segs). After ExitBootServices
// the firmware is dismantled and we're free to install ours.
```

## L539-541 · `unsafe { crate::kernel_main(&*(&raw const BOOT_INFO)) };`

```
// Hand off to the kernel proper. BOOT_INFO lives in .bss (the
// kernel image), so the &'static reference outlives anything
// UEFI gave us. kernel_main never returns.
```

## L545 · `unsafe extern "C" {`

```
// ── GDT install (post-ExitBootServices) ────────────────────────────
```

## L558-564 · `#[unsafe(no_mangle)]`

```
/// Load our kernel GDT and reload all segment selectors. Safe to call
/// only after ExitBootServices — the firmware's Boot Services run code
/// that assumes its own selector layout; replacing the GDT before that
/// hangs the call.
///
/// CS=0x08 (code), DS/ES/FS/GS/SS=0x10 (data). Same layout the IDT
/// hardcodes and what VMX host-state validation expects later.
```

## L575 · `"mov ax, 0x10",`

```
// Reload data segments to 0x10
```

## L582 · `"lea rax, [rip + 2f]",`

```
// Far-return to reload CS = 0x08
```

