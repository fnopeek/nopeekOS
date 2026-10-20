//! The run end to end on the host: stage1::run as _start calls it, on a
//! fake card with the device's IP discovery binary at the top of VRAM and
//! an ACPI VFCT holding the device's VBIOS, built to the layouts
//! amdgpu_acpi_vfct_bios reads, and the staged firmware as sys/firmware/.
//! Prints the log the card run prints.

extern crate alloc;

include!("modules.rs");
#[path = "../src/stage1.rs"]
mod stage1;

use regs::*;

include!("fake_renoir.rs");
include!("fake_gpu.rs");

/// The GPU service's framebuffer on the fake card: at VRAM 0, the bottom
/// of the aperture, 64x32.
const FB: (u64, u32, u32, u32) = (0, 64, 32, 256);

fn shade(shadow: u32, x: u32, y: u32) -> u32 {
    shadow << 28 | y << 8 | x
}

fn main() {
    let mut adev = fake_renoir();
    fake_gpu();
    // An inner rectangle from shadow 1, then the lower half from shadow 2,
    // which is one span.
    npk_sys::fake_gpudev(FB, vec![[1, 0, 3, 2, 20, 10], [2, 0, 0, 16, 64, 16]]);
    for s in 1..=2 {
        let d = npk_sys::fake_gpudev_shadow(s as usize).unwrap();
        for y in 0..FB.2 {
            for x in 0..FB.1 {
                d.w32(y * FB.3 + x * 4, shade(s, x, y));
            }
        }
    }
    stage1::run(&mut adev, BDF, (0xfc00_0000, 2 << 20));
    let mut wrong = 0;
    for y in 0..FB.2 {
        for x in 0..FB.1 {
            let want = if y >= 16 {
                shade(2, x, y)
            } else if (3..23).contains(&x) && (2..12).contains(&y) {
                shade(1, x, y)
            } else {
                0
            };
            if npk_sys::vram_r32(FB.0 + (y * FB.3 + x * 4) as u64) != want {
                wrong += 1;
            }
        }
    }
    println!("gpudev framebuffer after the copies: {} of {} pixels wrong", wrong, FB.1 * FB.2);
    PACKETS.with(|p| for l in p.borrow().iter() { println!("gpu {}", l); });
    // Every write but the MM_INDEX window VRAM is read through.
    for (reg, v) in npk_sys::fake_card_writes() {
        if reg != mmMM_INDEX && reg != mmMM_INDEX_HI {
            println!("write reg {:#x} = {:#x}", reg, v);
        }
    }
}
