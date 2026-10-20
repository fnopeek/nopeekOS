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

/// The GPU service's framebuffer on the fake card: at VRAM 0, named
/// through PCI BAR0 as the GOP names it, 64x32.
const BAR0: (u64, u64) = (0xd000_0000, 256 << 20);
const FB: (u64, u32, u32, u32) = (BAR0.0, 64, 32, 256);

fn shade(shadow: u32, x: u32, y: u32) -> u32 {
    shadow << 28 | y << 8 | x
}

/// A scene for shadow 1: the whole screen damaged, one opaque fill.
const SCENE_BYTES: u32 = 64 + 16 + 64;

fn scene() -> Vec<u8> {
    let mut w = vec![0u32; SCENE_BYTES as usize / 4];
    w[..14].copy_from_slice(&[0x314E_4353, 1, 1, 0, 0, !0, !0, !0, 0, 0, 0, 0, FB.1, FB.2]);
    w[16..20].copy_from_slice(&[0, 0, FB.1, FB.2]);
    w[20..25].copy_from_slice(&[8, 0, 0, FB.1, FB.2]);
    w[25] = 0xff20_4060;
    w.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A guest ccmd: the `vdrm_ccmd_req` header, then `body`.
fn ccmd(cmd: u32, seqno: u32, body: &[u32]) -> Vec<u8> {
    let len = 16 + body.len() as u32 * 4;
    let mut w = vec![cmd, len, seqno, 0];
    w.extend_from_slice(body);
    w.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn main() {
    let mut adev = fake_renoir();
    fake_gpu();
    // A guest context: open, DEV_INFO and READ_MMR_REG as libdrm asks,
    // a context made and freed, a ccmd not served yet, closed.
    let dev_info = ccmd(1, 1, &[0, 0, SIZEOF_drm_amdgpu_info_device, AMDGPU_INFO_DEV_INFO, 0, 0, 0, 0]);
    let mmr = ccmd(1, 2, &[0, 0, 4, AMDGPU_INFO_READ_MMR_REG, 0x263e, 1, 0xffff_ffff, 0]);
    let ctx = ccmd(7, 3, &[0, 0]);
    let free = ccmd(7, 4, &[1, 1]);
    let gem = ccmd(2, 5, &[1, 0, 4096, 0, 4096, 0, 4, 0, 0, 0]);
    let len = |v: &Vec<u8>| v.len() as u32;
    // An inner rectangle from shadow 1, then the lower half from shadow 2,
    // which is one span.
    npk_sys::fake_gpudev(FB, vec![(1, [1, 0, 3, 2, 20, 10], vec![]), (1, [2, 0, 0, 16, 64, 16], vec![]),
                                  (2, [3, 0, 0, 0, 0, 0], vec![]), (4, [1, SCENE_BYTES, 0, 0, 0, 0], scene()),
                                  (3, [3, 0, 0, 0, 0, 0], vec![]),
                                  (5, [100, 1, 0, 0, 0, 0], vec![]),
                                  (7, [100, len(&dev_info), 0, 0, 0, 0], dev_info.clone()),
                                  (7, [100, len(&mmr), 0, 0, 0, 0], mmr.clone()),
                                  (7, [100, len(&ctx), 0, 0, 0, 0], ctx.clone()),
                                  (7, [100, len(&free), 0, 0, 0, 0], free.clone()),
                                  (7, [100, len(&gem), 0, 0, 0, 0], gem.clone()),
                                  (5, [100, 0, 0, 0, 0, 0], vec![])]);
    for s in 1..=2 {
        let d = npk_sys::fake_gpudev_shadow(s as usize).unwrap();
        for y in 0..FB.2 {
            for x in 0..FB.1 {
                d.w32(y * FB.3 + x * 4, shade(s, x, y));
            }
        }
    }
    adev.bar0 = BAR0;
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
            if npk_sys::vram_r32((y * FB.3 + x * 4) as u64) != want {
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
