//! Loading an arm64 Linux kernel, after
//! `Documentation/arch/arm64/booting.rst`: the `Image` header (offset,
//! size, flags, the "ARM\x64" magic), the image at `text_offset` from a
//! 2 MiB aligned base in RAM, the device tree and the initramfs behind it,
//! and on entry x0 = the device tree's address, MMU off, at EL1.

use crate::microvm::devices::guest_mem::GuestMem;

const MAGIC: u32 = 0x644d_5241;
const HEADER_LEN: usize = 64;
const TWO_MB: u64 = 2 << 20;
/// The image's base: 2 MiB, so a guest page 0 stays free as on QEMU.
const IMAGE_BASE: u64 = TWO_MB;
/// The device tree must not exceed 2 MiB (booting.rst §2).
const DTB_MAX: u64 = TWO_MB;

pub struct Loaded {
    pub entry: u64,
    pub dtb_gpa: u64,
}

/// Where the image, its working memory, the device tree and the initramfs go.
pub struct Placement {
    pub image: u64,
    pub image_end: u64,
    pub dtb: u64,
    pub initrd: u64,
}

/// Check the header and place everything; `initrd_len` 0 for none.
pub fn place(image: &[u8], initrd_len: usize, window: u64) -> Result<Placement, &'static str> {
    if image.len() < HEADER_LEN {
        return Err("arm64 Image: shorter than its header");
    }
    let u32_at = |o: usize| u32::from_le_bytes([image[o], image[o + 1], image[o + 2], image[o + 3]]);
    let u64_at = |o: usize| u64::from_le_bytes(image[o..o + 8].try_into().unwrap_or([0; 8]));
    if u32_at(0x38) != MAGIC {
        return Err("arm64 Image: no ARM\\x64 magic (a compressed Image.gz?)");
    }
    let text_offset = u64_at(0x08);
    let mut image_size = u64_at(0x10);
    let flags = u64_at(0x18);
    // image_size 0: a kernel older than 3.17, text_offset 0x80000 (booting.rst).
    let text_offset = if image_size == 0 { 0x8_0000 } else { text_offset };
    if image_size == 0 {
        image_size = image.len() as u64;
    }
    if flags & 1 != 0 {
        return Err("arm64 Image: big-endian kernel");
    }
    // Bits 2:1, the page size the kernel was built for: 4 KiB or unspecified.
    if (flags >> 1) & 3 > 1 {
        return Err("arm64 Image: needs a page size other than 4 KiB");
    }
    let start = IMAGE_BASE + text_offset;
    let image_end = start + image_size.max(image.len() as u64);
    let dtb = image_end.next_multiple_of(TWO_MB);
    let initrd = dtb + DTB_MAX;
    let end = initrd + initrd_len as u64;
    if end > window {
        return Err("arm64 Image: kernel + initramfs do not fit the boot window");
    }
    Ok(Placement { image: start, image_end, dtb, initrd })
}

/// Copy the image, `dtb` and `initrd` into guest RAM at `p`. Everything
/// written is cleaned to the point of coherency: the guest starts with its
/// MMU and caches off.
pub fn load(gm: &GuestMem, p: &Placement, image: &[u8], dtb: &[u8], initrd: Option<&[u8]>) -> Result<Loaded, &'static str> {
    if dtb.len() as u64 > DTB_MAX {
        return Err("device tree larger than 2 MiB");
    }
    if !gm.write_bytes(p.image, image) {
        return Err("arm64 Image outside guest RAM");
    }
    if !gm.write_bytes(p.dtb, dtb) {
        return Err("device tree outside guest RAM");
    }
    if let Some(rd) = initrd {
        if !gm.write_bytes(p.initrd, rd) {
            return Err("initramfs outside guest RAM");
        }
    }
    Ok(Loaded { entry: p.image, dtb_gpa: p.dtb })
}
