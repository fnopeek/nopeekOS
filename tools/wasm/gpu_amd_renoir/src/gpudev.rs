//! The GPU service (docs/plan/GPU_STACK.md §3.3): the kernel's framebuffer
//! and its two shadow buffers made reachable for the SDMA engine, a copy
//! self-test, then the compositor's copies on SDMA0 until the module ends.
//!
//! The framebuffer is the firmware's, in VRAM: its bus address in one of
//! the windows onto VRAM gives its MC address. The shadows are kernel
//! pages, imported as a dma-buf would be (`amdgpu_dma_buf_create_obj`),
//! pinned in GTT and bound into the GART of VMID 0, where the kernel's
//! copies run in Linux too (`amdgpu_copy_buffer`). A rectangle is copied
//! row by row with `sdma_v4_0_emit_copy_buffer`, a full-width one in one
//! span.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_object::{self as obj, amdgpu_kptr};
use crate::linux::Line;
use crate::regs::*;
use alloc::vec::Vec;

const WAIT_IRQ: i32 = 2;
const WAIT_GPU_CMD: i32 = 64;
const VERSION: i32 = 2;
const OP_COPY_RECT: u32 = 1;
const CAP_COPY: i32 = 1;
const JOB_BYTES: usize = 32;
const BO_INFO_BYTES: usize = 40;
const NUM_BOS: usize = 3;
const EINVAL: i32 = 22;
const ETIME: i32 = 62;
/// The kernel gives a job 100 ms; the module answers before that.
const JOB_TIMEOUT_US: i64 = 80_000;
const PAGE: u64 = 4096;
const TEST_W: u32 = 64;
const TEST_H: u32 = 64;
const TEST_PITCH: u32 = TEST_W * 4;

/// A buffer of the kernel's as `npk_gpudev_bo_info` describes it, and
/// where SDMA reaches its first pixel.
#[derive(Clone, Copy, Default)]
struct bo {
    phys: u64,
    size: u64,
    pitch: u32,
    width: u32,
    height: u32,
    offset: u32,
    pages: u32,
    gpu: u64,
    /// The imported buffer object of a shadow.
    handle: Option<usize>,
}

/// `npk_gpudev_bo_info`, parsed.
fn bo_info(id: u32) -> Option<bo> {
    let mut out = [0u8; BO_INFO_BYTES];
    if npk_sys::gpudev_bo_info(id as i32, &mut out) != 0 {
        return None;
    }
    let q = |o: usize| u64::from_le_bytes(out[o..o + 8].try_into().unwrap());
    let d = |o: usize| u32::from_le_bytes(out[o..o + 4].try_into().unwrap());
    Some(bo { phys: q(0), size: q(8), pitch: d(16), width: d(20), height: d(24), offset: d(32), pages: d(36),
              ..Default::default() })
}

/// The framebuffer by its bus address: its MC address, if it lies in
/// visible VRAM. The firmware's scanout may be named through either window
/// onto VRAM: the APU's direct one (`gmc.aper_base`, the carve-out, as
/// `gmc_v9_0_mc_init` sets it) or PCI BAR0, which the GOP uses.
fn bind_framebuffer(adev: &amdgpu_device, b: &mut bo) -> bool {
    let gmc = &adev.gmc;
    let windows = [("aperture", gmc.aper_base, gmc.visible_vram_size), ("BAR0", adev.bar0.0, adev.bar0.1)];
    let Some((name, off)) = windows.iter().find_map(|&(name, base, size)| {
        let off = b.phys.checked_sub(base)?;
        (off + b.offset as u64 + b.size <= size.min(gmc.visible_vram_size)).then_some((name, off))
    }) else {
        return false;
    };
    b.gpu = crate::amdgpu_ttm::amdgpu_ttm_domain_start(adev, TTM_PL_VRAM) + off + b.offset as u64;
    let mut l = Line::new();
    l.s("gpudev: framebuffer at VRAM 0x").hexw((off >> 32) as u32, 8).hexw(off as u32, 8).s(" through ").s(name);
    if off + b.size > adev.mman.stolen_vga_size + adev.mman.stolen_extended_size {
        l.s(", beyond the reserved firmware range");
    }
    l.say();
    true
}

/// A shadow buffer: its pages imported, pinned in GTT and bound in the
/// GART.
fn bind_shadow(adev: &mut amdgpu_device, id: u32, b: &mut bo) -> Result<(), i32> {
    if b.pages == 0 || b.offset as u64 >= PAGE || b.size + b.offset as u64 > b.pages as u64 * PAGE {
        return Err(-EINVAL);
    }
    let mut pages = Vec::with_capacity(b.pages as usize);
    let mut list = [0u64; 64];
    while (pages.len() as u32) < b.pages {
        let want = ((b.pages - pages.len() as u32) as usize).min(list.len());
        let n = npk_sys::gpudev_bo_pages(id as i32, pages.len() as i32, &mut list[..want]);
        if n <= 0 || n as usize > want {
            return Err(-EINVAL);
        }
        pages.extend_from_slice(&list[..n as usize]);
    }
    let h = obj::amdgpu_dma_buf_create_obj(adev, pages)?;
    let r = obj::amdgpu_bo_pin(adev, h, AMDGPU_GEM_DOMAIN_GTT);
    if r != 0 {
        obj::amdgpu_bo_unref(adev, h);
        return Err(r);
    }
    let r = crate::amdgpu_ttm::amdgpu_ttm_alloc_gart(adev, h);
    if r != 0 {
        obj::amdgpu_bo_unpin(adev, h);
        obj::amdgpu_bo_unref(adev, h);
        return Err(r);
    }
    b.gpu = obj::amdgpu_bo_gpu_offset(adev, h) + b.offset as u64;
    b.handle = Some(h);
    Ok(())
}

/// The rectangle lies inside the buffer.
fn rect_fits(b: &bo, x: u32, y: u32, w: u32, h: u32) -> bool {
    let (Some(x2), Some(y2)) = (x.checked_add(w), y.checked_add(h)) else { return false };
    if w == 0 || h == 0 || x2 > b.width || y2 > b.height || x2 as u64 * 4 > b.pitch as u64 {
        return false;
    }
    (y2 as u64 - 1) * b.pitch as u64 + x2 as u64 * 4 <= b.size
}

/// One rectangle from `src` to `dst` at the same place on SDMA0, waited
/// for on its fence.
fn copy_rect(adev: &mut amdgpu_device, src: u64, src_pitch: u32, dst: u64, dst_pitch: u32, x: u32, y: u32, w: u32,
             h: u32) -> Result<(), i32> {
    let Some(r) = adev.sdma.instance[0].ring else { return Err(-EINVAL) };
    let row = w * 4;
    let s0 = src + y as u64 * src_pitch as u64 + x as u64 * 4;
    let d0 = dst + y as u64 * dst_pitch as u64 + x as u64 * 4;
    let spans: Vec<(u64, u64, u32)> = if row == src_pitch && row == dst_pitch {
        alloc::vec![(s0, d0, row * h)]
    } else {
        (0..h as u64).map(|i| (s0 + i * src_pitch as u64, d0 + i * dst_pitch as u64, row)).collect()
    };
    let f = crate::amdgpu_ttm::amdgpu_copy_spans(adev, r, &spans, false, false, 0)?;
    if crate::dma_fence::dma_fence_wait_timeout(adev, &f, JOB_TIMEOUT_US) == 0 {
        return Err(-ETIME);
    }
    Ok(())
}

/// The pattern of the self-test at a pixel.
fn pattern(x: u32, y: u32) -> u32 {
    0xa500_0000 | (y << 8) | x
}

/// The read-back self-test: a pattern in GTT, copied by SDMA0 into a GTT
/// buffer and into a VRAM buffer, a full-width block and an inner
/// rectangle, must arrive and nothing around it may change.
fn self_test(adev: &mut amdgpu_device) -> bool {
    let size = (TEST_PITCH * TEST_H) as u64;
    let mut bos: [Option<usize>; 3] = [None; 3];
    let ok = (|| {
        for (i, domain) in [AMDGPU_GEM_DOMAIN_GTT, AMDGPU_GEM_DOMAIN_GTT, AMDGPU_GEM_DOMAIN_VRAM].iter().enumerate() {
            bos[i] = obj::amdgpu_bo_create_kernel(adev, size, PAGE, *domain, true).ok().flatten();
            bos[i]?;
        }
        let k = |adev: &amdgpu_device, h: usize| -> Option<amdgpu_kptr> { obj::bo(adev, h).kptr };
        let (src, dsts) = (bos[0]?, [bos[1]?, bos[2]?]);
        let ks = k(adev, src)?;
        let mut row = [0u8; TEST_PITCH as usize];
        for y in 0..TEST_H {
            for x in 0..TEST_W {
                row[(x * 4) as usize..(x * 4 + 4) as usize].copy_from_slice(&pattern(x, y).to_le_bytes());
            }
            crate::amdgpu_gart::kptr_write(adev, ks, (y * TEST_PITCH) as u64, &row);
        }
        let s = obj::amdgpu_bo_gpu_offset(adev, src);
        for &dh in dsts.iter() {
            let kd = k(adev, dh)?;
            let d = obj::amdgpu_bo_gpu_offset(adev, dh);
            for (x, y, w, h) in [(0, 0, TEST_W, TEST_H / 2), (5, TEST_H / 2 + 3, TEST_W - 13, TEST_H / 2 - 7)] {
                crate::amdgpu_gart::kptr_fill32(adev, kd, 0, size, 0);
                if let Err(e) = copy_rect(adev, s, TEST_PITCH, d, TEST_PITCH, x, y, w, h) {
                    Line::new().s("gpudev: self-test copy failed (").dec((-e) as u64).s(")").say();
                    return None;
                }
                for py in 0..TEST_H {
                    crate::amdgpu_gart::kptr_read(adev, kd, (py * TEST_PITCH) as u64, &mut row);
                    for px in 0..TEST_W {
                        let inside = px >= x && px < x + w && py >= y && py < y + h;
                        let want = if inside { pattern(px, py) } else { 0 };
                        let v = u32::from_le_bytes(row[(px * 4) as usize..(px * 4 + 4) as usize].try_into().unwrap());
                        if v != want {
                            Line::new().s("gpudev: self-test read back 0x").hexw(v, 8).s(" at ").dec(px as u64)
                                .s(",").dec(py as u64).s(", expected 0x").hexw(want, 8).say();
                            return None;
                        }
                    }
                }
            }
        }
        Some(())
    })()
    .is_some();
    for mut h in bos {
        obj::amdgpu_bo_free_kernel(adev, &mut h);
    }
    ok
}

/// One job from the kernel; its status.
fn run_job(adev: &mut amdgpu_device, bos: &[bo; NUM_BOS], job: &[u8]) -> i32 {
    let w = |i: usize| u32::from_le_bytes(job[i * 4..i * 4 + 4].try_into().unwrap());
    match w(1) {
        OP_COPY_RECT => {
            let (src, dst, x, y, cw, ch) = (w(2) as usize, w(3) as usize, w(4), w(5), w(6), w(7));
            if src >= NUM_BOS || dst >= NUM_BOS || src == dst || !rect_fits(&bos[src], x, y, cw, ch)
                || !rect_fits(&bos[dst], x, y, cw, ch) {
                return -EINVAL;
            }
            let (s, d) = (bos[src], bos[dst]);
            match copy_rect(adev, s.gpu, s.pitch, d.gpu, d.pitch, x, y, cw, ch) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        _ => -EINVAL,
    }
}

/// The shadows' buffer objects go back.
fn unbind(adev: &mut amdgpu_device, bos: &mut [bo; NUM_BOS]) {
    for b in bos.iter_mut() {
        if let Some(h) = b.handle.take() {
            obj::amdgpu_bo_unpin(adev, h);
            obj::amdgpu_bo_unref(adev, h);
        }
    }
    crate::amdgpu_gart::amdgpu_gart_invalidate_tlb(adev);
}

/// Register with the kernel, bind its buffers, prove the copy and serve
/// until a copy times out.
pub fn serve(adev: &mut amdgpu_device) {
    let rc = npk_sys::gpudev_register(VERSION);
    if rc != 0 {
        Line::new().s("gpudev: register failed (").dec((-rc) as u64).s(")").say();
        return;
    }
    let mut bos = [bo::default(); NUM_BOS];
    for (id, b) in bos.iter_mut().enumerate() {
        let Some(info) = bo_info(id as u32) else {
            Line::new().s("gpudev: no buffer ").dec(id as u64).say();
            npk_sys::gpudev_ready(0);
            return;
        };
        *b = info;
    }
    Line::new().s("gpudev: ").dec(bos[0].width as u64).s("x").dec(bos[0].height as u64).s(", pitch ")
        .dec(bos[0].pitch as u64).s(", framebuffer at bus 0x").hexw((bos[0].phys >> 32) as u32, 8)
        .hexw(bos[0].phys as u32, 8).say();
    let mut bound = bind_framebuffer(adev, &mut bos[0]);
    if !bound {
        Line::new().s("gpudev: the framebuffer is not in visible VRAM").say();
    }
    for id in 1..NUM_BOS {
        if !bound {
            break;
        }
        if let Err(e) = bind_shadow(adev, id as u32, &mut bos[id]) {
            Line::new().s("gpudev: shadow ").dec(id as u64).s(" could not be bound (").dec((-e) as u64).s(")")
                .say();
            bound = false;
        }
    }
    if !bound {
        unbind(adev, &mut bos);
        npk_sys::gpudev_ready(0);
        return;
    }
    for (id, b) in bos.iter().enumerate() {
        npk_sys::gpudev_bo_bound(id as i32, b.gpu as i64);
        Line::new().s("gpudev: bo ").dec(id as u64).s(" at 0x").hexw((b.gpu >> 32) as u32, 8).hexw(b.gpu as u32, 8)
            .dbg();
    }
    let copy_ok = self_test(adev);
    Line::new().s("gpudev: sdma0 self-test ").s(if copy_ok { "passed" } else { "FAILED" }).say();
    npk_sys::gpudev_ready(if copy_ok { CAP_COPY } else { 0 });
    if !copy_ok {
        unbind(adev, &mut bos);
        return;
    }
    Line::new().s("gpudev: serving copies").say();

    let mut jobs = [0u8; JOB_BYTES * 8];
    let (mut done, mut failed) = (0u64, 0u64);
    loop {
        npk_sys::wait(WAIT_IRQ | WAIT_GPU_CMD, 100);
        crate::amdgpu_irq::amdgpu_ih_process(adev, false);
        crate::workqueue::run_pending(adev);
        let n = npk_sys::gpudev_poll(&mut jobs);
        // The kernel no longer has this module as its service.
        if n < 0 {
            Line::new().s("gpudev: no longer the service after ").dec(done).s(" copies (").dec(failed)
                .s(" refused)").say();
            break;
        }
        let mut timed_out = false;
        for j in 0..n.max(0) as usize {
            let job = &jobs[j * JOB_BYTES..(j + 1) * JOB_BYTES];
            let seq = u32::from_le_bytes(job[0..4].try_into().unwrap());
            let status = run_job(adev, &bos, job);
            if status == 0 { done += 1 } else { failed += 1 }
            timed_out |= status == -ETIME;
            npk_sys::gpudev_complete(seq as i32, status);
        }
        if timed_out {
            Line::new().s("gpudev: a copy on sdma0 timed out after ").dec(done).s(" copies (").dec(failed)
                .s(" refused), giving the card back").say();
            break;
        }
    }
    unbind(adev, &mut bos);
}
