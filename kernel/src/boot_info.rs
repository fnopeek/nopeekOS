//! Boot info handed off from the UEFI stub (`boot_uefi`) to
//! `kernel_main`. Decoupled from `boot_uefi` so the kernel proper
//! doesn't pull in UEFI ABI types; it just sees opaque memory regions
//! tagged with their original UEFI type id.
//!
//! The UEFI stub populates this in static storage before
//! `ExitBootServices`, then passes a `&'static BootInfo` to
//! `kernel_main`. After ExitBootServices the entire UEFI ABI surface
//! is gone; only this struct's contents survive.

#![allow(dead_code)]

pub const MAX_MEMORY_REGIONS: usize = 256;

// UEFI memory type constants (UEFI Spec §7.2). Mirror them here so
// modules outside the boot stub (memory.rs, framebuffer.rs, acpi.rs)
// don't have to import from boot_uefi.
pub const UEFI_RESERVED: u32          = 0;
pub const UEFI_LOADER_CODE: u32       = 1;
pub const UEFI_LOADER_DATA: u32       = 2;
pub const UEFI_BOOT_SERVICES_CODE: u32 = 3;
pub const UEFI_BOOT_SERVICES_DATA: u32 = 4;
pub const UEFI_RUNTIME_SERVICES_CODE: u32 = 5;
pub const UEFI_RUNTIME_SERVICES_DATA: u32 = 6;
pub const UEFI_CONVENTIONAL_MEMORY: u32 = 7;
pub const UEFI_UNUSABLE_MEMORY: u32    = 8;
pub const UEFI_ACPI_RECLAIM: u32       = 9;
pub const UEFI_ACPI_NVS: u32           = 10;
pub const UEFI_MMIO: u32               = 11;
pub const UEFI_MMIO_PORT_SPACE: u32    = 12;
pub const UEFI_PAL_CODE: u32           = 13;
pub const UEFI_PERSISTENT_MEMORY: u32  = 14;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MemoryRegion {
    pub physical_start: u64,
    pub page_count: u64,
    pub uefi_type: u32,
}

impl MemoryRegion {
    pub const ZERO: MemoryRegion = MemoryRegion {
        physical_start: 0, page_count: 0, uefi_type: 0,
    };

    /// Whether this region is general-purpose RAM the kernel can hand
    /// out via its frame allocator. Boot-services memory is freed for
    /// us by ExitBootServices, so it counts too.
    pub fn is_usable(&self) -> bool {
        matches!(
            self.uefi_type,
            UEFI_CONVENTIONAL_MEMORY
                | UEFI_BOOT_SERVICES_CODE
                | UEFI_BOOT_SERVICES_DATA
        )
    }
}

#[repr(C)]
pub struct BootInfo {
    /// Physical address of the ACPI 2.0 RSDP (RSDT/XSDT entrypoint).
    /// `0` if firmware exposes no ACPI tables.
    pub acpi_rsdp: u64,

    // Framebuffer (direct linear, identity-mapped by UEFI in low memory).
    pub fb_base: u64,
    pub fb_width: u32,
    pub fb_height: u32,
    pub fb_pitch_bytes: u32,
    pub fb_bpp: u8,
    pub _fb_pad: [u8; 3],

    pub region_count: u32,
    pub regions: [MemoryRegion; MAX_MEMORY_REGIONS],
}

impl BootInfo {
    pub const fn empty() -> Self {
        Self {
            acpi_rsdp: 0,
            fb_base: 0,
            fb_width: 0,
            fb_height: 0,
            fb_pitch_bytes: 0,
            fb_bpp: 0,
            _fb_pad: [0; 3],
            region_count: 0,
            regions: [MemoryRegion::ZERO; MAX_MEMORY_REGIONS],
        }
    }

    pub fn usable_regions(&self) -> impl Iterator<Item = &MemoryRegion> {
        self.regions[..self.region_count as usize]
            .iter()
            .filter(|r| r.is_usable() && r.page_count > 0)
    }
}
