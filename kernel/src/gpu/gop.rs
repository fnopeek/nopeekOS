//! GOP (Graphics Output Protocol) Fallback Driver
//!
//! Uses the framebuffer provided by the bootloader via Multiboot2.
//! No modesetting — resolution is fixed at boot by GRUB/UEFI.

use super::{FramebufferInfo, GpuError, GpuHal, ModeInfo};
use alloc::vec::Vec;

pub struct GopDriver {
    fb: FramebufferInfo,
}

impl GpuHal for GopDriver {
    fn name(&self) -> &'static str { "GOP" }

    fn set_mode(&mut self, _w: u32, _h: u32, _hz: u8) -> Result<FramebufferInfo, GpuError> {
        Err(GpuError::UnsupportedMode) // GOP can't switch modes
    }

    fn framebuffer(&self) -> FramebufferInfo { self.fb }

    fn supported_modes(&self) -> Vec<ModeInfo> {
        alloc::vec![ModeInfo { width: self.fb.width, height: self.fb.height, hz: 60 }]
    }

    fn current_hz(&self) -> u8 { 0 }
    fn is_native(&self) -> bool { false }
    fn flip(&mut self, _surface_addr: u64) { /* GOP: no hardware flip */ }
    fn wait_vblank(&self) { /* GOP: no vblank access */ }
    fn supports_flip(&self) -> bool { false }
}

impl GopDriver {
    /// Pull the framebuffer details from the BootInfo struct that
    /// `boot_uefi::efi_main` populated via the UEFI Graphics Output
    /// Protocol. Returns `None` if no framebuffer was discovered.
    pub fn from_boot_info(boot_info: &crate::boot_info::BootInfo) -> Option<Self> {
        if boot_info.fb_base == 0 || boot_info.fb_width == 0 {
            return None;
        }

        let addr = boot_info.fb_base;
        let pitch = boot_info.fb_pitch_bytes;
        let width = boot_info.fb_width;
        let height = boot_info.fb_height;
        let bpp = boot_info.fb_bpp;

        // Map framebuffer pages identity. UEFI already exposed them at
        // their physical address, but we install our own page tables
        // later and need the entries.
        let fb_size = pitch as u64 * height as u64;
        for page_off in (0..fb_size).step_by(4096) {
            let pa = addr + page_off;
            let _ = crate::paging::map_page(
                pa, pa,
                crate::paging::PageFlags::PRESENT
                    | crate::paging::PageFlags::WRITABLE,
            );
        }

        Some(GopDriver {
            fb: FramebufferInfo { addr, pitch, width, height, bpp },
        })
    }

    pub fn framebuffer(&self) -> FramebufferInfo {
        self.fb
    }

    pub fn supported_modes(&self) -> alloc::vec::Vec<ModeInfo> {
        // GOP only has the one mode the bootloader selected
        alloc::vec![ModeInfo {
            width: self.fb.width,
            height: self.fb.height,
            hz: 60,
        }]
    }
}
