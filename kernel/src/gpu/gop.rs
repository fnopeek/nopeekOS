//! GOP (Graphics Output Protocol) Fallback Driver
//!
//! Uses the framebuffer provided by the bootloader via Multiboot2.
//! No modesetting — resolution is fixed at boot by GRUB/UEFI.

use super::{FramebufferInfo, ModeInfo};

pub struct GopDriver {
    fb: FramebufferInfo,
}

impl GopDriver {
    /// Parse Multiboot2 boot info to find framebuffer tag (type 8).
    pub fn from_multiboot2(mb_info_addr: u32) -> Option<Self> {
        let base = mb_info_addr as usize;
        let total_size = unsafe { *(base as *const u32) } as usize;

        let mut offset = 8;
        while offset + 8 <= total_size {
            let tag_type = unsafe { *((base + offset) as *const u32) };
            let tag_size = unsafe { *((base + offset + 4) as *const u32) } as usize;

            if tag_size == 0 { break; }

            if tag_type == 8 && tag_size >= 32 {
                let addr = unsafe { *((base + offset + 8) as *const u64) };
                let pitch = unsafe { *((base + offset + 16) as *const u32) };
                let width = unsafe { *((base + offset + 20) as *const u32) };
                let height = unsafe { *((base + offset + 24) as *const u32) };
                let bpp = unsafe { *((base + offset + 28) as *const u8) };
                let fb_type = unsafe { *((base + offset + 29) as *const u8) };

                // Type 0 or 1 = pixel framebuffer, need at least 24bpp
                if (fb_type == 1 || fb_type == 0) && bpp >= 24 {
                    // Map framebuffer MMIO pages
                    let fb_size = pitch as u64 * height as u64;
                    for page_off in (0..fb_size).step_by(4096) {
                        let pa = addr + page_off;
                        let _ = crate::paging::map_page(
                            pa, pa,
                            crate::paging::PageFlags::PRESENT
                                | crate::paging::PageFlags::WRITABLE,
                        );
                    }

                    return Some(GopDriver {
                        fb: FramebufferInfo { addr, pitch, width, height, bpp },
                    });
                }
            }

            if tag_type == 0 { break; }
            offset += (tag_size + 7) & !7;
        }

        None
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
