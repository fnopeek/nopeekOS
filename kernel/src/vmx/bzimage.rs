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

/// Setup-header offset within a bzImage (also within boot_params).
pub const SETUP_HEADER_OFFSET: usize = 0x1F1;

/// "HdrS" — required magic at SetupHeader::header.
pub const HDR_MAGIC: u32 = 0x53726448;

/// 0xAA55 — boot-sector magic at byte offset 0x1FE inside bzImage.
pub const BOOT_FLAG: u16 = 0xAA55;

/// Linux Boot Protocol setup-header. Fields up through
/// `handover_offset` cover protocol 2.10+; later fields exist on
/// 2.12+ but we don't read them. Layout is `#[repr(C, packed)]`
/// matching Linux's `struct setup_header`.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SetupHeader {
    pub setup_sects:        u8,    // # of setup sectors (each 512 B)
    pub root_flags:         u16,
    pub syssize:            u32,   // size/16 of protected-mode part
    pub ram_size:           u16,
    pub vid_mode:           u16,
    pub root_dev:           u16,
    pub boot_flag:          u16,   // = 0xAA55
    pub jump:               u16,
    pub header:             u32,   // = "HdrS" = 0x53726448
    pub version:            u16,   // protocol, e.g. 0x020F = 2.15
    pub realmode_swtch:     u32,
    pub start_sys_seg:      u16,
    pub kernel_version:     u16,
    pub type_of_loader:     u8,
    pub loadflags:          u8,
    pub setup_move_size:    u16,
    pub code32_start:       u32,   // 32-bit entry point (default 0x100000)
    pub ramdisk_image:      u32,
    pub ramdisk_size:       u32,
    pub bootsect_kludge:    u32,
    pub heap_end_ptr:       u16,
    pub ext_loader_ver:     u8,
    pub ext_loader_type:    u8,
    pub cmd_line_ptr:       u32,
    pub initrd_addr_max:    u32,
    pub kernel_alignment:   u32,
    pub relocatable_kernel: u8,
    pub min_alignment:      u8,
    pub xloadflags:         u16,
    pub cmdline_size:       u32,
    pub hardware_subarch:   u32,
    pub hardware_subarch_data: u64,
    pub payload_offset:     u32,
    pub payload_length:     u32,
    pub setup_data:         u64,
    pub pref_address:       u64,
    pub init_size:          u32,   // bytes of contiguous RAM the kernel needs
    pub handover_offset:    u32,
}

/// Parse the setup-header at `bzimage[0x1F1..]`. Validates both magic
/// fields. Returns the parsed header by value; the input slice must
/// remain valid for the caller's use, but the parsed struct is
/// independently owned.
pub fn parse_header(bzimage: &[u8]) -> Result<SetupHeader, &'static str> {
    let header_size = core::mem::size_of::<SetupHeader>();
    if bzimage.len() < SETUP_HEADER_OFFSET + header_size {
        return Err("bzImage too short for setup-header");
    }

    // Check boot_flag at byte offset 0x1FE (little-endian u16).
    let boot_flag = u16::from_le_bytes([bzimage[0x1FE], bzimage[0x1FF]]);
    if boot_flag != BOOT_FLAG {
        return Err("bzImage boot_flag mismatch (not 0xAA55)");
    }

    // SAFETY: bounds checked above; SetupHeader is repr(C, packed)
    // so a byte-for-byte read from the source slice is well-defined.
    let header: SetupHeader = unsafe {
        core::ptr::read_unaligned(
            bzimage.as_ptr().add(SETUP_HEADER_OFFSET) as *const SetupHeader,
        )
    };

    if header.header != HDR_MAGIC {
        return Err("bzImage HdrS magic mismatch");
    }

    Ok(header)
}

/// Setup-section size in bytes including the bootsector (i.e., the
/// portion that gets loaded at guest-phys 0x10000 in 16-bit boot).
/// `setup_sects = 0` is interpreted as 4 per legacy convention.
pub fn setup_section_size(header: &SetupHeader) -> usize {
    let sects = if header.setup_sects == 0 { 4 } else { header.setup_sects };
    (sects as usize + 1) * 512
}

/// Protected-mode kernel image size. From the syssize field
/// (paragraphs of 16 bytes).
pub fn protected_kernel_size(header: &SetupHeader) -> usize {
    (header.syssize as usize) * 16
}
