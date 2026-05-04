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

// ── Loader (Phase 12.1.1c-3b3b2) ───────────────────────────────────

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
const SETUP_GUEST_PHYS: u64 = 0x10000;
const CMDLINE_GUEST_PHYS: u64 = 0x20000;
const BOOT_PARAMS_GUEST_PHYS: u64 = 0x90000;
const KERNEL_GUEST_PHYS: u64 = 0x100000;
const INITRAMFS_GUEST_PHYS: u64 = 0xC000000;

/// e820 memory map types.
const E820_TYPE_RAM: u32 = 1;
const E820_TYPE_RESERVED: u32 = 2;

/// Standard PC layout for the e820 we present to Linux:
///   [0x000000, 0x09F000) RAM (640 KB lower memory)
///   [0x09F000, 0x100000) RESERVED (BIOS area + EBDA)
///   [0x100000, GUEST_RAM_TOTAL) RAM ("extended memory")
///
/// Linux's early-boot direct-map setup walks the e820 and builds
/// the kernel's identity/direct mappings. A single contiguous
/// `[0, RAM_TOTAL) RAM` entry omits the BIOS hole, which on some
/// kernel paths trips memory-layout assumptions and leaves the
/// direct-map L4 entry empty for low-RAM regions. Splitting per
/// PC convention works around it.
///
/// Must equal `ept::GUEST_WINDOW_BYTES` — the EPT window backs the
/// e820 RAM. 64 MB OOM-panicked Alpine virt during first kthread
/// fork ("Memory: 20420K available"); 256 MB gives Linux ~230 MB
/// usable, enough for initcalls + initramfs + a small distro.
const GUEST_RAM_TOTAL: u64 = 256 * 1024 * 1024;

/// Linux loadflags bits we need.
const LOADFLAG_LOADED_HIGH: u8 = 1 << 0;
const LOADFLAG_KEEP_SEGMENTS: u8 = 1 << 6;

/// Linux Boot Protocol bootloader-id we put in type_of_loader.
/// 0xFF = "undefined / generic" (any third-party loader).
const TYPE_OF_LOADER: u8 = 0xFF;

/// Boot-params zero-page offsets (subset we touch).
const OFF_E820_ENTRIES: usize = 0x1E8;
const OFF_SENTINEL: usize     = 0x1EF; // must be 0
const OFF_HDR: usize          = 0x1F1; // setup-header copy
const OFF_E820_TABLE: usize   = 0x2D0;

/// One e820_entry as Linux expects (struct boot_e820_entry).
#[repr(C, packed)]
struct E820Entry {
    addr: u64,
    size: u64,
    typ:  u32,
}

/// Where we placed the kernel + boot_params, returned from
/// `load_into_guest_ram`.
pub struct LoadInfo {
    /// Linear (= EPT-mapped guest-physical) entry point —
    /// `header.code32_start`, typically 0x100000.
    pub entry_rip: u64,
    /// Guest-physical address of the boot_params zero-page —
    /// must end up in ESI before VM-entry per Linux 32-bit boot
    /// protocol.
    pub boot_params_phys: u64,
}

/// Copy the bzImage parts into guest RAM and build a minimal
/// boot_params zero-page so the kernel can boot via the 32-bit
/// boot protocol.
///
/// `host_base` is the host-physical address of the EPT-mapped
/// guest-RAM window (caller already allocated it). `bzimage` is
/// the raw bzImage byte slice. `cmdline` is the kernel command
/// line as ASCII bytes (no NUL — the loader appends one).
/// `initramfs` is an optional cpio.gz that becomes the rootfs at
/// /; Linux's standard logic execs `/init` from it as PID-1.
pub fn load_into_guest_ram(
    host_base: u64,
    bzimage: &[u8],
    cmdline: &[u8],
    initramfs: Option<&[u8]>,
) -> Result<LoadInfo, &'static str> {
    let header = parse_header(bzimage)?;
    let setup_size = setup_section_size(&header);
    let prot_size  = protected_kernel_size(&header);

    if cmdline.len() >= 4096 {
        return Err("cmdline too large (max 4 KB)");
    }
    if setup_size + prot_size > bzimage.len() {
        return Err("bzImage truncated (setup + kernel exceeds bytes)");
    }
    if KERNEL_GUEST_PHYS + (prot_size as u64) > GUEST_RAM_TOTAL {
        return Err("kernel image overflows guest RAM window");
    }
    if let Some(ir) = initramfs {
        if INITRAMFS_GUEST_PHYS + ir.len() as u64 > GUEST_RAM_TOTAL {
            return Err("initramfs overflows guest RAM window");
        }
    }

    // Copy setup-section to guest-phys 0x10000.
    // SAFETY: host_base is 2-MB-aligned and pre-allocated; the
    // [host_base, host_base + 256 MB) window is exclusively ours.
    unsafe {
        copy_to_guest(host_base, SETUP_GUEST_PHYS, &bzimage[..setup_size]);
        copy_to_guest(
            host_base,
            KERNEL_GUEST_PHYS,
            &bzimage[setup_size..setup_size + prot_size],
        );

        // Cmdline at guest-phys 0x20000, NUL-terminated.
        copy_to_guest(host_base, CMDLINE_GUEST_PHYS, cmdline);
        write_byte_to_guest(host_base, CMDLINE_GUEST_PHYS + cmdline.len() as u64, 0);

        if let Some(ir) = initramfs {
            copy_to_guest(host_base, INITRAMFS_GUEST_PHYS, ir);
        }
    }

    // Build boot_params: zero 4 KB, copy setup-header from bzImage
    // at offset 0x1F1 into boot_params at the same offset, override
    // the fields we care about.
    let mut bp: [u8; 4096] = [0; 4096];
    let bp_hdr_end = OFF_HDR + core::mem::size_of::<SetupHeader>();
    bp[OFF_HDR..bp_hdr_end].copy_from_slice(
        &bzimage[OFF_HDR..bp_hdr_end],
    );

    // Sentinel byte must be 0 to allow boot.
    bp[OFF_SENTINEL] = 0;

    // Override setup-header fields per Boot Protocol.
    // type_of_loader is at OFF_HDR + offsetof(SetupHeader, type_of_loader)
    // = 0x1F1 + 0x1F = 0x210.
    bp[0x210] = TYPE_OF_LOADER;
    // loadflags is at 0x211.
    bp[0x211] = LOADFLAG_LOADED_HIGH | LOADFLAG_KEEP_SEGMENTS;
    // cmd_line_ptr (u32) is at 0x228.
    let cmd_line_ptr = CMDLINE_GUEST_PHYS as u32;
    bp[0x228..0x22C].copy_from_slice(&cmd_line_ptr.to_le_bytes());
    // ramdisk_image (u32) at 0x218, ramdisk_size (u32) at 0x21C.
    let (ramdisk_image, ramdisk_size) = match initramfs {
        Some(ir) => (INITRAMFS_GUEST_PHYS as u32, ir.len() as u32),
        None => (0, 0),
    };
    bp[0x218..0x21C].copy_from_slice(&ramdisk_image.to_le_bytes());
    bp[0x21C..0x220].copy_from_slice(&ramdisk_size.to_le_bytes());

    // Three e820 entries — standard PC layout.
    bp[OFF_E820_ENTRIES] = 3;
    let entries = [
        E820Entry { addr: 0,         size: 0x09_F000,                 typ: E820_TYPE_RAM },
        E820Entry { addr: 0x09_F000, size: 0x10_0000 - 0x09_F000,     typ: E820_TYPE_RESERVED },
        E820Entry { addr: 0x10_0000, size: GUEST_RAM_TOTAL - 0x10_0000, typ: E820_TYPE_RAM },
    ];
    let entry_size = core::mem::size_of::<E820Entry>();
    for (i, entry) in entries.iter().enumerate() {
        let start = OFF_E820_TABLE + i * entry_size;
        let entry_bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(
                entry as *const _ as *const u8,
                entry_size,
            )
        };
        bp[start..start + entry_size].copy_from_slice(entry_bytes);
    }

    // Copy boot_params into guest RAM at 0x90000.
    // SAFETY: as above, exclusive window.
    unsafe { copy_to_guest(host_base, BOOT_PARAMS_GUEST_PHYS, &bp); }

    Ok(LoadInfo {
        entry_rip: header.code32_start as u64,
        boot_params_phys: BOOT_PARAMS_GUEST_PHYS,
    })
}

/// SAFETY: caller guarantees the [host_base + guest_phys,
/// host_base + guest_phys + bytes.len()) range is exclusively ours
/// and within the EPT window.
unsafe fn copy_to_guest(host_base: u64, guest_phys: u64, bytes: &[u8]) {
    let dst = (host_base + guest_phys) as *mut u8;
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
    }
}

/// SAFETY: caller guarantees the byte at host_base + guest_phys is
/// exclusively ours.
unsafe fn write_byte_to_guest(host_base: u64, guest_phys: u64, val: u8) {
    let dst = (host_base + guest_phys) as *mut u8;
    unsafe { dst.write_volatile(val); }
}
