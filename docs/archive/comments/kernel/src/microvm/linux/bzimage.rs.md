# `kernel/src/microvm/linux/bzimage.rs` @ 5e0102684

## L1-16 · `pub const SETUP_HEADER_OFFSET: usize = 0x1F1;`

```
//! Linux Boot Protocol — bzImage setup-header parser.
//!
//! Phase 12.1.1c-3b1: read-only parsing only. The full loader
//! (copy parts into guest RAM, build boot_params + e820 + cmdline,
//! VMLAUNCH at code32_start) lands in 12.1.1c-3b2.
//!
//! Linux ships its kernel image as a "bzImage" — a concatenation of
//! a legacy real-mode bootsector + a multi-sector setup section +
//! the (gzip-compressed) protected-mode kernel. The setup-header
//! struct lives at byte offset 0x1F1 inside the bzImage and tells
//! a bootloader where everything is.
//!
//! Reference: Linux's `Documentation/x86/boot.rst` (kernel.org).
//! The struct layout matches `arch/x86/include/uapi/asm/bootparam.h`
//! `struct setup_header`, which is ABI-stable across kernel versions
//! via the `boot_protocol_version` field.
```

## L18 · `pub const SETUP_HEADER_OFFSET: usize = 0x1F1;`

```
/// Setup-header offset within a bzImage (also within boot_params).
```

## L21 · `pub const HDR_MAGIC: u32 = 0x53726448;`

```
/// "HdrS" — required magic at SetupHeader::header.
```

## L24 · `pub const BOOT_FLAG: u16 = 0xAA55;`

```
/// 0xAA55 — boot-sector magic at byte offset 0x1FE inside bzImage.
```

## L27-30 · `#[repr(C, packed)]`

```
/// Linux Boot Protocol setup-header. Fields up through
/// `handover_offset` cover protocol 2.10+; later fields exist on
/// 2.12+ but we don't read them. Layout is `#[repr(C, packed)]`
/// matching Linux's `struct setup_header`.
```

## L34 · `pub setup_sects:        u8,    // # of setup sectors (each 512 B)`

```
// # of setup sectors (each 512 B)
```

## L36 · `pub syssize:            u32,   // size/16 of protected-mode part`

```
// size/16 of protected-mode part
```

## L40 · `pub boot_flag:          u16,   // = 0xAA55`

```
// = 0xAA55
```

## L42 · `pub header:             u32,   // = "HdrS" = 0x53726448`

```
// = "HdrS" = 0x53726448
```

## L43 · `pub version:            u16,   // protocol, e.g. 0x020F = 2.15`

```
// protocol, e.g. 0x020F = 2.15
```

## L50 · `pub code32_start:       u32,   // 32-bit entry point (default 0x100000)`

```
// 32-bit entry point (default 0x100000)
```

## L70 · `pub init_size:          u32,   // bytes of contiguous RAM the kernel needs`

```
// bytes of contiguous RAM the kernel needs
```

## L74-77 · `pub fn parse_header(bzimage: &[u8]) -> Result<SetupHeader, &'static str> {`

```
/// Parse the setup-header at `bzimage[0x1F1..]`. Validates both magic
/// fields. Returns the parsed header by value; the input slice must
/// remain valid for the caller's use, but the parsed struct is
/// independently owned.
```

## L84 · `let boot_flag = u16::from_le_bytes([bzimage[0x1FE], bzimage[0x1FF]]);`

```
// Check boot_flag at byte offset 0x1FE (little-endian u16).
```

## L90-91 · `let header: SetupHeader = unsafe {`

```
// SAFETY: bounds checked above; SetupHeader is repr(C, packed)
// so a byte-for-byte read from the source slice is well-defined.
```

## L105-107 · `pub fn setup_section_size(header: &SetupHeader) -> usize {`

```
/// Setup-section size in bytes including the bootsector (i.e., the
/// portion that gets loaded at guest-phys 0x10000 in 16-bit boot).
/// `setup_sects = 0` is interpreted as 4 per legacy convention.
```

## L113-114 · `pub fn protected_kernel_size(header: &SetupHeader) -> usize {`

```
/// Protected-mode kernel image size. From the syssize field
/// (paragraphs of 16 bytes).
```

## L119 · `const SETUP_GUEST_PHYS: u64 = 0x10000;`

```
// ── Loader (Phase 12.1.1c-3b3b2) ───────────────────────────────────
```

## L121-131 · `const SETUP_GUEST_PHYS: u64 = 0x10000;`

```
/// Boot-params guest-physical layout (under our 256-MB EPT window):
///   0x10000   setup-section (boot sector + setup_sects sectors,
///             ~16 KB) — needed for legacy compatibility / EFI even
///             when entry is 32-bit.
///   0x20000   kernel command line (NUL-terminated, max ~256 B)
///   0x90000   boot_params struct (4 KB zero-page, includes a copy
///             of the setup-header at offset 0x1F1).
///   0x100000  protected-mode kernel image (= bzImage[setup_section..])
///   0xC000000 initramfs (= 192 MB, well above kernel's `init_size`
///             which is ~38 MB for Alpine virt 6.18). Linux frees
///             this region after unpacking the cpio into rootfs.
```

## L138 · `const E820_TYPE_RAM: u32 = 1;`

```
/// e820 memory map types.
```

## L144-155 · `const LOADFLAG_LOADED_HIGH: u8 = 1 << 0;`

```
// Standard PC layout for the e820 we present to Linux:
//   [0x000000, 0x09F000) RAM (640 KB lower memory)
//   [0x09F000, 0x100000) RESERVED (BIOS area + EBDA)
//   [0x100000, mem.len()) RAM ("extended memory")
//
// Linux's early-boot direct-map setup walks the e820 and builds the
// kernel's identity/direct mappings. A single contiguous
// `[0, RAM_TOTAL) RAM` entry omits the BIOS hole, which on some kernel
// paths trips memory-layout assumptions and leaves the direct-map L4
// entry empty for low-RAM regions. Splitting per PC convention works
// around it. The extended-memory size is `mem.len()` — the canonical
// guest-RAM size (`guest_mem::GUEST_RAM_BYTES`, B2: runtime).
```

## L157 · `const LOADFLAG_LOADED_HIGH: u8 = 1 << 0;`

```
/// Linux loadflags bits we need.
```

## L161-162 · `const TYPE_OF_LOADER: u8 = 0xFF;`

```
/// Linux Boot Protocol bootloader-id we put in type_of_loader.
/// 0xFF = "undefined / generic" (any third-party loader).
```

## L165 · `const OFF_E820_ENTRIES: usize = 0x1E8;`

```
/// Boot-params zero-page offsets (subset we touch).
```

## L167 · `const OFF_SENTINEL: usize     = 0x1EF; // must be 0`

```
// must be 0
```

## L168 · `const OFF_HDR: usize          = 0x1F1; // setup-header copy`

```
// setup-header copy
```

## L171 · `#[repr(C, packed)]`

```
/// One e820_entry as Linux expects (struct boot_e820_entry).
```

## L179-180 · `pub struct LoadInfo {`

```
/// Where we placed the kernel + boot_params, returned from
/// `load_into_guest_ram`.
```

## L182-183 · `pub entry_rip: u64,`

```
/// Linear (= EPT-mapped guest-physical) entry point —
/// `header.code32_start`, typically 0x100000.
```

## L185-187 · `pub boot_params_phys: u64,`

```
/// Guest-physical address of the boot_params zero-page —
/// must end up in ESI before VM-entry per Linux 32-bit boot
/// protocol.
```

## L191-200 · `pub fn load_into_guest_ram(`

```
/// Copy the bzImage parts into guest RAM and build a minimal
/// boot_params zero-page so the kernel can boot via the 32-bit
/// boot protocol.
///
/// `mem` translates guest-physical addresses into the (caller-
/// allocated) guest-RAM window. `bzimage` is the raw bzImage byte
/// slice. `cmdline` is the kernel command line as ASCII bytes (no
/// NUL — the loader appends one). `initramfs` is an optional cpio.gz
/// that becomes the rootfs at /; Linux's standard logic execs
/// `/init` from it as PID-1.
```

## L226-229 · `mem.write_bytes(SETUP_GUEST_PHYS, &bzimage[..setup_size]);`

```
// Place the guest image. `GuestMem` bounds-checks every write
// against the window; the explicit checks above give a precise
// error before we start. B1: window is contiguous, so these are
// plain copies; B3: `GuestMem` faults pages in / splits per page.
```

## L236 · `mem.write_bytes(CMDLINE_GUEST_PHYS, cmdline);`

```
// Cmdline at guest-phys 0x20000, NUL-terminated.
```

## L244-246 · `let mut bp: [u8; 4096] = [0; 4096];`

```
// Build boot_params: zero 4 KB, copy setup-header from bzImage
// at offset 0x1F1 into boot_params at the same offset, override
// the fields we care about.
```

## L253 · `bp[OFF_SENTINEL] = 0;`

```
// Sentinel byte must be 0 to allow boot.
```

## L256-258 · `bp[0x210] = TYPE_OF_LOADER;`

```
// Override setup-header fields per Boot Protocol.
// type_of_loader is at OFF_HDR + offsetof(SetupHeader, type_of_loader)
// = 0x1F1 + 0x1F = 0x210.
```

## L260 · `bp[0x211] = LOADFLAG_LOADED_HIGH | LOADFLAG_KEEP_SEGMENTS;`

```
// loadflags is at 0x211.
```

## L262 · `let cmd_line_ptr = CMDLINE_GUEST_PHYS as u32;`

```
// cmd_line_ptr (u32) is at 0x228.
```

## L265 · `let (ramdisk_image, ramdisk_size) = match initramfs {`

```
// ramdisk_image (u32) at 0x218, ramdisk_size (u32) at 0x21C.
```

## L273 · `bp[OFF_E820_ENTRIES] = 3;`

```
// Three e820 entries — standard PC layout.
```

## L292 · `mem.write_bytes(BOOT_PARAMS_GUEST_PHYS, &bp);`

```
// Copy boot_params into guest RAM at 0x90000.
```

## L295-297 · `if crate::microvm::cpu::GUEST_SMP {`

```
// Guest-SMP Stage 2: enumerate the vCPUs via an MP-table in the
// BIOS window (0xF0000, RESERVED above in our e820). Linux scans
// for it when booted `acpi=off` and counts GUEST_VCPUS CPUs.
```

