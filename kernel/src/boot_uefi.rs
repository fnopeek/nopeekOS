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

#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]

use core::ffi::c_void;

// ── Primitive UEFI types ───────────────────────────────────────────

pub type EfiStatus = usize;
pub type EfiHandle = *mut c_void;
pub type Uintn = usize;
pub type EfiPhysicalAddress = u64;
pub type EfiVirtualAddress = u64;

pub const EFI_SUCCESS: EfiStatus = 0;
pub const EFI_BUFFER_TOO_SMALL: EfiStatus = 5 | (1usize << (usize::BITS - 1));
pub const EFI_INVALID_PARAMETER: EfiStatus = 2 | (1usize << (usize::BITS - 1));
pub const EFI_NOT_FOUND: EfiStatus = 14 | (1usize << (usize::BITS - 1));

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EfiGuid {
    pub a: u32,
    pub b: u16,
    pub c: u16,
    pub d: [u8; 8],
}

// GUIDs we care about — these are spec-defined byte patterns.
pub const EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID: EfiGuid = EfiGuid {
    a: 0x9042A9DE, b: 0x23DC, c: 0x4A38,
    d: [0x96, 0xFB, 0x7A, 0xDE, 0xD0, 0x80, 0x51, 0x6A],
};
pub const EFI_ACPI_20_TABLE_GUID: EfiGuid = EfiGuid {
    a: 0x8868E871, b: 0xE4F1, c: 0x11D3,
    d: [0xBC, 0x22, 0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81],
};
pub const EFI_ACPI_10_TABLE_GUID: EfiGuid = EfiGuid {
    a: 0xEB9D2D30, b: 0x2D88, c: 0x11D3,
    d: [0x9A, 0x16, 0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D],
};

// ── System / Boot Services tables ──────────────────────────────────

#[repr(C)]
pub struct EfiTableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct EfiSimpleTextOutputProtocol {
    pub reset: usize,
    pub output_string: unsafe extern "efiapi" fn(
        this: *mut EfiSimpleTextOutputProtocol,
        string: *const u16,
    ) -> EfiStatus,
    // rest of fn ptrs omitted — we only print
}

#[repr(C)]
pub struct EfiConfigurationTable {
    pub vendor_guid: EfiGuid,
    pub vendor_table: *mut c_void,
}

/// EFI_BOOT_SERVICES — UEFI spec §7. Only the function pointers we
/// actually call are typed; everything else is `usize` placeholder
/// to keep the layout right. Adding more is mechanical: count entries
/// from spec and bump placeholders.
#[repr(C)]
pub struct EfiBootServices {
    pub hdr: EfiTableHeader,

    // Task Priority Services
    pub raise_tpl: usize,
    pub restore_tpl: usize,

    // Memory Services
    pub allocate_pages: usize,
    pub free_pages: usize,
    pub get_memory_map: unsafe extern "efiapi" fn(
        memory_map_size: *mut Uintn,
        memory_map: *mut EfiMemoryDescriptor,
        map_key: *mut Uintn,
        descriptor_size: *mut Uintn,
        descriptor_version: *mut u32,
    ) -> EfiStatus,
    pub allocate_pool: unsafe extern "efiapi" fn(
        pool_type: u32,
        size: Uintn,
        buffer: *mut *mut c_void,
    ) -> EfiStatus,
    pub free_pool: unsafe extern "efiapi" fn(buffer: *mut c_void) -> EfiStatus,

    // Event & Timer Services
    pub create_event: usize,
    pub set_timer: usize,
    pub wait_for_event: usize,
    pub signal_event: usize,
    pub close_event: usize,
    pub check_event: usize,

    // Protocol Handler Services
    pub install_protocol_interface: usize,
    pub reinstall_protocol_interface: usize,
    pub uninstall_protocol_interface: usize,
    pub handle_protocol: usize,
    pub reserved: usize,
    pub register_protocol_notify: usize,
    pub locate_handle: usize,
    pub locate_device_path: usize,
    pub install_configuration_table: usize,

    // Image Services
    pub load_image: usize,
    pub start_image: usize,
    pub exit: usize,
    pub unload_image: usize,
    pub exit_boot_services: unsafe extern "efiapi" fn(
        image_handle: EfiHandle,
        map_key: Uintn,
    ) -> EfiStatus,

    // Miscellaneous Services (partial — we don't use these in Phase 2)
    pub get_next_monotonic_count: usize,
    pub stall: usize,
    pub set_watchdog_timer: usize,

    // DriverSupport Services
    pub connect_controller: usize,
    pub disconnect_controller: usize,

    // Open and Close Protocol Services
    pub open_protocol: usize,
    pub close_protocol: usize,
    pub open_protocol_information: usize,

    // Library Services
    pub protocols_per_handle: usize,
    pub locate_handle_buffer: usize,
    pub locate_protocol: unsafe extern "efiapi" fn(
        protocol: *const EfiGuid,
        registration: *mut c_void,
        interface: *mut *mut c_void,
    ) -> EfiStatus,
    // ... rest omitted
}

#[repr(C)]
pub struct EfiSystemTable {
    pub hdr: EfiTableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    // Implicit 4-byte padding (alignment of next ptr).
    pub console_in_handle: EfiHandle,
    pub con_in: *mut c_void,
    pub console_out_handle: EfiHandle,
    pub con_out: *mut EfiSimpleTextOutputProtocol,
    pub standard_error_handle: EfiHandle,
    pub std_err: *mut EfiSimpleTextOutputProtocol,
    pub runtime_services: *mut c_void,
    pub boot_services: *mut EfiBootServices,
    pub number_of_table_entries: Uintn,
    pub configuration_table: *mut EfiConfigurationTable,
}

// ── Memory map ─────────────────────────────────────────────────────

#[repr(C)]
pub struct EfiMemoryDescriptor {
    pub r#type: u32,
    pub pad: u32,
    pub physical_start: EfiPhysicalAddress,
    pub virtual_start: EfiVirtualAddress,
    pub number_of_pages: u64,
    pub attribute: u64,
}

pub const EFI_LOADER_CODE: u32 = 1;
pub const EFI_LOADER_DATA: u32 = 2;
pub const EFI_BOOT_SERVICES_CODE: u32 = 3;
pub const EFI_BOOT_SERVICES_DATA: u32 = 4;
pub const EFI_CONVENTIONAL_MEMORY: u32 = 7;
pub const EFI_LOADER_DATA_POOL: u32 = 2; // for AllocatePool type arg

// ── Graphics Output Protocol ───────────────────────────────────────

#[repr(C)]
pub struct EfiPixelBitmask {
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub reserved_mask: u32,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum EfiGraphicsPixelFormat {
    PixelRedGreenBlueReserved8BitPerColor = 0,
    PixelBlueGreenRedReserved8BitPerColor = 1,
    PixelBitMask = 2,
    PixelBltOnly = 3,
    PixelFormatMax = 4,
}

#[repr(C)]
pub struct EfiGraphicsOutputModeInformation {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    pub pixel_format: EfiGraphicsPixelFormat,
    pub pixel_information: EfiPixelBitmask,
    pub pixels_per_scan_line: u32,
}

#[repr(C)]
pub struct EfiGraphicsOutputProtocolMode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *mut EfiGraphicsOutputModeInformation,
    pub size_of_info: Uintn,
    pub frame_buffer_base: EfiPhysicalAddress,
    pub frame_buffer_size: Uintn,
}

#[repr(C)]
pub struct EfiGraphicsOutputProtocol {
    pub query_mode: usize,
    pub set_mode: usize,
    pub blt: usize,
    pub mode: *mut EfiGraphicsOutputProtocolMode,
}

// BootInfo / MemoryRegion live in `crate::boot_info` — same types the
// kernel proper sees. The UEFI stub populates them before
// ExitBootServices; everything downstream is firmware-independent.
use crate::boot_info::{BootInfo, MemoryRegion, MAX_MEMORY_REGIONS};

// ── COM1 raw debug print (survives ExitBootServices) ───────────────

#[inline(always)]
unsafe fn com1_putc(b: u8) {
    unsafe {
        // COM2 (0x2F8) → -serial file:target/serial.log. Unbuffered.
        // Write FIRST so the byte hits disk even if COM1 stalls.
        core::arch::asm!(
            "out dx, al", in("dx") 0x2F8u16, in("al") b,
            options(nomem, nostack)
        );
        // COM1 (0x3F8) → -serial mon:stdio (interactive). QEMU's UART
        // has effectively unbounded buffer; skip the LSR-empty poll.
        core::arch::asm!(
            "out dx, al", in("dx") 0x3F8u16, in("al") b,
            options(nomem, nostack)
        );
    }
}

unsafe fn com1_print(s: &[u8]) {
    for &b in s {
        if b == b'\n' { unsafe { com1_putc(b'\r') }; }
        unsafe { com1_putc(b) };
    }
}

unsafe fn com1_hex64(v: u64) {
    for i in (0..16).rev() {
        let nibble = ((v >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + (nibble - 10) };
        unsafe { com1_putc(c) };
    }
}

unsafe fn com1_dec(v: u64) {
    if v == 0 { unsafe { com1_putc(b'0') }; return; }
    let mut buf = [0u8; 20];
    let mut n = v;
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        unsafe { com1_putc(buf[i]) };
    }
}

// ── BootInfo storage ───────────────────────────────────────────────
//
// Allocated in .bss so it survives the transition from UEFI-managed
// memory to our own. Single instance, leaked into kernel_main.

static mut BOOT_INFO: BootInfo = BootInfo::empty();

// ── ACPI RSDP discovery ────────────────────────────────────────────

unsafe fn find_acpi_rsdp(system_table: *mut EfiSystemTable) -> u64 {
    let count = unsafe { (*system_table).number_of_table_entries };
    let table = unsafe { (*system_table).configuration_table };
    let mut rsdp_v2: u64 = 0;
    let mut rsdp_v1: u64 = 0;
    for i in 0..count {
        let entry = unsafe { &*table.add(i) };
        if entry.vendor_guid == EFI_ACPI_20_TABLE_GUID {
            rsdp_v2 = entry.vendor_table as u64;
        } else if entry.vendor_guid == EFI_ACPI_10_TABLE_GUID {
            rsdp_v1 = entry.vendor_table as u64;
        }
    }
    // Prefer ACPI 2.0+ if present; fall back to legacy.
    if rsdp_v2 != 0 { rsdp_v2 } else { rsdp_v1 }
}

// ── Graphics Output ────────────────────────────────────────────────

unsafe fn locate_gop(bs: *mut EfiBootServices) -> Option<&'static EfiGraphicsOutputProtocol> {
    let mut interface: *mut c_void = core::ptr::null_mut();
    let status = unsafe {
        ((*bs).locate_protocol)(
            &EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID,
            core::ptr::null_mut(),
            &mut interface,
        )
    };
    if status != EFI_SUCCESS || interface.is_null() { return None; }
    Some(unsafe { &*(interface as *const EfiGraphicsOutputProtocol) })
}

// ── Memory map collection ──────────────────────────────────────────
//
// UEFI GetMemoryMap is "tell me how big a buffer to give you" + "now
// fill it". The buffer must come from AllocatePool because the map
// can change between calls. After ExitBootServices we have no
// allocator — but we copy regions into the static BootInfo.regions
// array before exiting, so it's fine.

#[repr(C, align(8))]
struct MmapBuf([u8; 16 * 1024]);
static mut MMAP_SCRATCH: MmapBuf = MmapBuf([0; 16 * 1024]);

/// Fill `BOOT_INFO.regions` from the UEFI memory map. Also returns
/// the `map_key` that must be passed to `ExitBootServices`. The map
/// changes when any allocator call runs between this and ExitBootServices,
/// so the caller must invoke them in the right order (we don't do any
/// allocation after this).
unsafe fn collect_memory_map(bs: *mut EfiBootServices) -> Uintn {
    let buf_ptr = unsafe { &raw mut MMAP_SCRATCH.0 } as *mut EfiMemoryDescriptor;
    let mut map_size: Uintn = 16 * 1024;
    let mut map_key: Uintn = 0;
    let mut desc_size: Uintn = 0;
    let mut desc_version: u32 = 0;

    let status = unsafe {
        ((*bs).get_memory_map)(
            &mut map_size,
            buf_ptr,
            &mut map_key,
            &mut desc_size,
            &mut desc_version,
        )
    };
    if status != EFI_SUCCESS {
        unsafe {
            com1_print(b"[uefi] GetMemoryMap failed: ");
            com1_hex64(status as u64);
            com1_print(b"\n");
        }
        return 0;
    }

    // Walk descriptors. Note `desc_size` may be larger than
    // sizeof(EfiMemoryDescriptor) — vendor extensions are allowed.
    // We stride by desc_size in bytes, casting each.
    let n = map_size / desc_size;
    let buf_bytes = buf_ptr as *const u8;
    let mut stored = 0u32;
    for i in 0..n {
        let d = unsafe { &*(buf_bytes.add(i * desc_size) as *const EfiMemoryDescriptor) };
        if (stored as usize) < MAX_MEMORY_REGIONS {
            unsafe {
                BOOT_INFO.regions[stored as usize] = MemoryRegion {
                    physical_start: d.physical_start,
                    page_count: d.number_of_pages,
                    uefi_type: d.r#type,
                };
            }
            stored += 1;
        }
    }
    unsafe { BOOT_INFO.region_count = stored };
    map_key
}

/// Diagnostic dump of usable / total memory from the populated
/// BOOT_INFO.regions[].
unsafe fn print_mem_summary() {
    let mut usable_pages: u64 = 0;
    let mut total_pages: u64 = 0;
    for i in 0..unsafe { BOOT_INFO.region_count } {
        let r = unsafe { &BOOT_INFO.regions[i as usize] };
        total_pages += r.page_count;
        match r.uefi_type {
            EFI_CONVENTIONAL_MEMORY | EFI_LOADER_CODE | EFI_LOADER_DATA
            | EFI_BOOT_SERVICES_CODE | EFI_BOOT_SERVICES_DATA => {
                usable_pages += r.page_count;
            }
            _ => {}
        }
    }
    unsafe {
        com1_print(b"[uefi] memory regions: ");
        com1_dec(BOOT_INFO.region_count as u64);
        com1_print(b" (");
        com1_dec((usable_pages * 4) / 1024);
        com1_print(b" MB usable / ");
        com1_dec((total_pages * 4) / 1024);
        com1_print(b" MB total)\n");
    }
}

// ── Entry from boot.s ──────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "efiapi" fn efi_main(
    image_handle: EfiHandle,
    system_table: *mut EfiSystemTable,
) -> ! {
    unsafe {
        com1_print(b"\n=== nopeekOS UEFI entry ===\n");
        com1_print(b"Phase 2: collecting BootInfo from firmware\n");
    }

    let bs = unsafe { (*system_table).boot_services };

    // 1. ACPI RSDP via ConfigurationTable walk.
    let rsdp = unsafe { find_acpi_rsdp(system_table) };
    unsafe {
        BOOT_INFO.acpi_rsdp = rsdp;
        com1_print(b"[uefi] ACPI RSDP @ 0x");
        com1_hex64(rsdp);
        com1_print(b"\n");
    }

    unsafe { com1_print(b"[uefi] >> locate_gop\n"); }
    // 2. Graphics Output Protocol → framebuffer.
    if let Some(gop) = unsafe { locate_gop(bs) } {
        unsafe { com1_print(b"[uefi] gop OK, reading mode...\n"); }
        let mode = unsafe { &*gop.mode };
        let info = unsafe { &*mode.info };
        unsafe {
            BOOT_INFO.fb_base = mode.frame_buffer_base;
            BOOT_INFO.fb_width = info.horizontal_resolution;
            BOOT_INFO.fb_height = info.vertical_resolution;
            BOOT_INFO.fb_pitch_bytes = info.pixels_per_scan_line * 4;
            BOOT_INFO.fb_bpp = 32;
            com1_print(b"[uefi] FB @ 0x");
            com1_hex64(mode.frame_buffer_base);
            com1_print(b" ");
            com1_dec(info.horizontal_resolution as u64);
            com1_print(b"x");
            com1_dec(info.vertical_resolution as u64);
            com1_print(b" pitch=");
            com1_dec(info.pixels_per_scan_line as u64 * 4);
            com1_print(b"B\n");
        }
    } else {
        unsafe {
            com1_print(b"[uefi] GOP not found (no framebuffer info)\n");
        }
    }

    // 3. Memory map — this is also the last UEFI call before
    // ExitBootServices; map_key must come from this snapshot.
    let map_key = unsafe { collect_memory_map(bs) };
    unsafe { print_mem_summary() };

    unsafe { com1_print(b"[uefi] >> ExitBootServices\n"); }
    // 4. ExitBootServices. After this call UEFI Boot Services are
    // gone — no more LocateProtocol, no more printk through ConOut.
    // We're on our own from here.
    let status = unsafe { ((*bs).exit_boot_services)(image_handle, map_key) };

    // Now that UEFI's IDT is no longer the active one (we still
    // share it but boot services are dismantled), gate IRQs off
    // until our own IDT loads in interrupts::init. boot.s left IF=1
    // because Boot Services need timer IRQs to make progress.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)); }
    if status != EFI_SUCCESS {
        unsafe {
            com1_print(b"[uefi] ExitBootServices failed: 0x");
            com1_hex64(status as u64);
            com1_print(b"\n");
            // Retry once with a fresh map — common when boot services
            // allocated/freed pages behind our back between GetMemoryMap
            // and ExitBootServices.
            com1_print(b"[uefi] retrying with fresh memory map...\n");
            let map_key2 = collect_memory_map(bs);
            let status2 = ((*bs).exit_boot_services)(image_handle, map_key2);
            if status2 != EFI_SUCCESS {
                com1_print(b"[uefi] ExitBootServices retry failed: 0x");
                com1_hex64(status2 as u64);
                com1_print(b"\n[uefi] giving up, halting.\n");
                loop { core::arch::asm!("cli; hlt") }
            }
        }
    }
    unsafe {
        com1_print(b"[uefi] ExitBootServices OK\n");
    }

    // Install our own GDT now. Doing this BEFORE ExitBootServices hangs
    // the firmware mid-call — Boot Services internally relies on
    // UEFI's selector layout (TR/LDT/code segs). After ExitBootServices
    // the firmware is dismantled and we're free to install ours.
    unsafe { install_kernel_gdt() };

    unsafe {
        com1_print(b"[uefi] GDT installed -- calling kernel_main\n");
    }

    // Hand off to the kernel proper. BOOT_INFO lives in .bss (the
    // kernel image), so the &'static reference outlives anything
    // UEFI gave us. kernel_main never returns.
    unsafe { crate::kernel_main(&*(&raw const BOOT_INFO)) };
}

// ── GDT install (post-ExitBootServices) ────────────────────────────

unsafe extern "C" {
    static gdt64: u8;
    static gdt64_end: u8;
}

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

/// Load our kernel GDT and reload all segment selectors. Safe to call
/// only after ExitBootServices — the firmware's Boot Services run code
/// that assumes its own selector layout; replacing the GDT before that
/// hangs the call.
///
/// CS=0x08 (code), DS/ES/FS/GS/SS=0x10 (data). Same layout the IDT
/// hardcodes and what VMX host-state validation expects later.
#[unsafe(no_mangle)]
unsafe fn install_kernel_gdt() {
    unsafe {
        let base = &gdt64 as *const u8 as u64;
        let end = &gdt64_end as *const u8 as u64;
        let limit = (end - base - 1) as u16;
        let ptr = GdtPointer { limit, base };

        core::arch::asm!(
            "lgdt [{ptr}]",
            // Reload data segments to 0x10
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ss, ax",
            // Far-return to reload CS = 0x08
            "lea rax, [rip + 2f]",
            "push 0x08",
            "push rax",
            "retfq",
            "2:",
            ptr = in(reg) &ptr,
            out("rax") _,
            options(nostack)
        );
    }
}
