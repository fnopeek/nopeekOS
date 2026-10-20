//! The GPU service (docs/plan/GPU_STACK.md §3.3): the kernel's framebuffer
//! and its two shadow buffers made reachable for the SDMA engine and for
//! the compositor's own client, the copy and dispatch self-tests, then the
//! compositor's copies on SDMA0 and its scenes on the compute ring until
//! the module ends.
//!
//! The framebuffer is the firmware's, in VRAM: its bus address in one of
//! the windows onto VRAM gives its MC address, and the reserved buffers
//! that keep the firmware's framebuffer from TTM (`stolen_vga_memory`,
//! `stolen_extended_memory`) map it into the compositor's VM. The shadows
//! and the scene sources are kernel pages, imported as a dma-buf would be
//! (`amdgpu_dma_buf_create_obj`) and pinned in GTT; the shadows are bound
//! into the GART of VMID 0 too, where the kernel's copies run in Linux
//! (`amdgpu_copy_buffer`). A rectangle is copied row by row with
//! `sdma_v4_0_emit_copy_buffer`, a full-width one in one span.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_object::{self as obj, amdgpu_kptr};
use crate::compose::{self, composer};
use crate::drm_mm::drm_mm_node;
use crate::linux::Line;
use crate::regs::*;
use alloc::vec::Vec;

const WAIT_IRQ: i32 = 2;
const WAIT_GPU_CMD: i32 = 64;
const VERSION: i32 = 2;
const OP_COPY_RECT: u32 = 1;
const OP_BO_ADD: u32 = 2;
const OP_BO_REMOVE: u32 = 3;
const OP_SCENE: u32 = 4;
/// `a0` client, `a1` 1 = open, 0 = close.
const OP_CLIENT: u32 = 5;
/// `a0` client, `a1` length; the payload is the guest's ccmd.
const OP_CCMD: u32 = 7;
/// `a0` client, `a1` blob id, `a2` resource id: the object handed to the
/// kernel (`get_blob`).
const OP_BLOB: u32 = 8;
/// `a0` client, `a1` resource id: the guest let go of the resource.
const OP_BLOB_RELEASE: u32 = 9;
const OP_ATTACH: u32 = 10;
/// The largest ccmd a guest sends, and the kernel's largest reply.
const CCMD_MAX: usize = 256 * 1024;
const REPLY_MAX: usize = 64 * 1024;
const CAP_COPY: i32 = 1;
const CAP_SCENE: i32 = 2;
const CAP_NATIVE: i32 = 4;
const JOB_BYTES: usize = 32;
const BO_INFO_BYTES: usize = 40;
const NUM_BOS: usize = 3;
const MAX_DYN: usize = 256;
const EINVAL: i32 = 22;
const ENOSPC: i32 = 28;
const ENOMEM: i32 = 12;
const ETIME: i32 = 62;
/// The kernel gives a job 100 ms; the module answers before that.
const JOB_TIMEOUT_US: i64 = 80_000;
const PAGE: u64 = 4096;
const TEST_W: u32 = 64;
const TEST_H: u32 = 64;
const TEST_PITCH: u32 = TEST_W * 4;
/// A job left open: the scene completes it when its fence signals.
const JOB_OPEN: i32 = i32::MIN;
const SCENES_LOGGED: u32 = 3;
/// The pieces a framebuffer's mapping may take.
const FB_PIECES: usize = 4;

/// A buffer of the kernel's as `npk_gpudev_bo_info` describes it, where
/// SDMA reaches its first pixel, and where the compositor's VM has it.
#[derive(Clone, Copy, Default)]
struct bo {
    id: u32,
    phys: u64,
    size: u64,
    pitch: u32,
    width: u32,
    height: u32,
    offset: u32,
    pages: u32,
    gpu: u64,
    /// The imported buffer object of a shadow or scene source.
    handle: Option<usize>,
    /// The framebuffer's VRAM offset, from its page.
    vram: u64,
    /// The compositor's view: the address of the first pixel, its VA
    /// range, and the buffer objects mapped there in order (true when made
    /// for it), with the bytes of each.
    va: u64,
    node: Option<drm_mm_node>,
    pieces: [Option<(usize, bool, u64)>; FB_PIECES],
    /// A tiled guest frame: the linear buffer the compositor's view is,
    /// and the import that fills it from the guest's object.
    linear: Option<compose::gem>,
    import: Option<compose::frame_import>,
}

/// `npk_gpudev_bo_info`, parsed.
fn bo_info(id: u32) -> Option<bo> {
    let mut out = [0u8; BO_INFO_BYTES];
    if npk_sys::gpudev_bo_info(id as i32, &mut out) != 0 {
        return None;
    }
    let q = |o: usize| u64::from_le_bytes(out[o..o + 8].try_into().unwrap());
    let d = |o: usize| u32::from_le_bytes(out[o..o + 4].try_into().unwrap());
    Some(bo { id, phys: q(0), size: q(8), pitch: d(16), width: d(20), height: d(24), offset: d(32), pages: d(36),
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
    b.vram = off;
    b.gpu = crate::amdgpu_ttm::amdgpu_ttm_domain_start(adev, TTM_PL_VRAM) + off + b.offset as u64;
    let mut l = Line::new();
    l.s("gpudev: framebuffer at VRAM 0x").hexw((off >> 32) as u32, 8).hexw(off as u32, 8).s(" through ").s(name);
    if off + b.size > adev.mman.stolen_vga_size + adev.mman.stolen_extended_size {
        l.s(", beyond the reserved firmware range");
    }
    l.say();
    true
}

/// Buffer `id`'s pages imported and pinned in GTT.
fn import(adev: &mut amdgpu_device, id: u32, b: &bo) -> Result<usize, i32> {
    let pages = bo_page_list(id, b)?;
    import_pages(adev, pages)
}

/// The bus address of each page of buffer `id`.
fn bo_page_list(id: u32, b: &bo) -> Result<Vec<u64>, i32> {
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
    Ok(pages)
}

/// System pages imported and pinned in GTT.
fn import_pages(adev: &mut amdgpu_device, pages: Vec<u64>) -> Result<usize, i32> {
    let h = obj::amdgpu_dma_buf_create_obj(adev, pages)?;
    let r = obj::amdgpu_bo_pin(adev, h, AMDGPU_GEM_DOMAIN_GTT);
    if r != 0 {
        obj::amdgpu_bo_unref(adev, h);
        return Err(r);
    }
    Ok(h)
}

/// A shadow buffer: imported and bound in the GART.
fn bind_shadow(adev: &mut amdgpu_device, id: u32, b: &mut bo) -> Result<(), i32> {
    let h = import(adev, id, b)?;
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

/// The compositor's mapping of `b`: one piece per buffer object, at
/// consecutive addresses of one VA range.
fn map_vm(adev: &mut amdgpu_device, c: &mut composer, b: &mut bo, pieces: &[(usize, bool, u64, u64)], align: u64)
          -> Result<(), i32> {
    let total: u64 = pieces.iter().map(|p| p.3).sum();
    let node = c.mm.insert_node_in_range_best(total, align, 0, compose::DYN_START, compose::DYN_END)?;
    let mut at = node.start;
    let mut done = 0;
    let r = (|| {
        for &(h, _, off, len) in pieces {
            crate::amdgpu_cs::amdgpu_gem_object_open(adev, &c.fpriv, h);
            done += 1;
            crate::amdgpu_cs::amdgpu_gem_va_ioctl(adev, &c.fpriv, h, true, at, off, len,
                                                   AMDGPU_VM_PAGE_READABLE | AMDGPU_VM_PAGE_WRITEABLE)?;
            at += len;
        }
        Ok(())
    })();
    if let Err(e) = r {
        let mut at = node.start;
        for &(h, made, _, len) in &pieces[..done] {
            let _ = crate::amdgpu_cs::amdgpu_gem_va_ioctl(adev, &c.fpriv, h, false, at, 0, len, 0);
            crate::amdgpu_cs::amdgpu_gem_object_close(adev, &c.fpriv, h);
            if made {
                let mut k = Some(h);
                obj::amdgpu_bo_free_kernel(adev, &mut k);
            }
            at += len;
        }
        c.mm.remove_node(&node);
        return Err(e);
    }
    b.node = Some(node);
    for (i, p) in pieces.iter().enumerate() {
        b.pieces[i] = Some((p.0, p.1, p.3));
    }
    Ok(())
}

/// The compositor's mapping of `b` taken down.
fn unmap_vm(adev: &mut amdgpu_device, c: &mut composer, b: &mut bo) {
    let Some(node) = b.node.take() else { return };
    let mut at = node.start;
    for p in b.pieces.iter_mut() {
        let Some((h, made, len)) = p.take() else { continue };
        let _ = crate::amdgpu_cs::amdgpu_gem_va_ioctl(adev, &c.fpriv, h, false, at, 0, len, 0);
        crate::amdgpu_cs::amdgpu_gem_object_close(adev, &c.fpriv, h);
        if made {
            let mut k = Some(h);
            obj::amdgpu_bo_free_kernel(adev, &mut k);
        }
        at += len;
    }
    c.mm.remove_node(&node);
    b.va = 0;
}

/// The framebuffer in the compositor's VM: the reserved buffers that hold
/// the firmware's framebuffer, a kernel buffer at the offset for any part
/// they leave (`amdgpu_bo_create_kernel_at`).
fn map_framebuffer(adev: &mut amdgpu_device, c: &mut composer, b: &mut bo) -> Result<(), i32> {
    let start = b.vram;
    let end = (b.vram + b.offset as u64 + b.size).next_multiple_of(PAGE);
    let reserved: Vec<(usize, u64, u64)> = [adev.mman.stolen_vga_memory, adev.mman.stolen_extended_memory]
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let r = obj::bo(adev, h).tbo.resource.as_ref()?;
            (r.mem_type == TTM_PL_VRAM).then(|| (h, r.start * PAGE, obj::amdgpu_bo_size(adev, h)))
        })
        .collect();
    let mut pieces: Vec<(usize, bool, u64, u64)> = Vec::new();
    let mut p = start;
    while p < end {
        if pieces.len() == FB_PIECES {
            return Err(-ENOSPC);
        }
        if let Some(&(h, s, len)) = reserved.iter().find(|&&(_, s, len)| p >= s && p < s + len) {
            let n = (s + len).min(end) - p;
            pieces.push((h, false, p - s, n));
            p += n;
            continue;
        }
        let next = reserved.iter().map(|r| r.1).filter(|&s| s > p).min().unwrap_or(end).min(end);
        let h = obj::amdgpu_bo_create_kernel_at(adev, p, next - p, false)?.ok_or(-EINVAL)?;
        pieces.push((h, true, 0, next - p));
        p = next;
    }
    map_vm(adev, c, b, &pieces, PAGE)?;
    b.va = b.node.unwrap().start + b.offset as u64;
    Ok(())
}

/// An imported buffer in the compositor's VM.
fn map_import(adev: &mut amdgpu_device, c: &mut composer, b: &mut bo) -> Result<(), i32> {
    let h = b.handle.ok_or(-EINVAL)?;
    map_vm(adev, c, b, &[(h, false, 0, b.pages as u64 * PAGE)], PAGE)?;
    b.va = b.node.unwrap().start + b.offset as u64;
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

/// The service's state.
struct service {
    bos: [bo; NUM_BOS],
    dyn_bos: Vec<bo>,
    copy_ok: bool,
    composer: Option<composer>,
    /// The scene on the compute ring, its job still open: its job's seq and
    /// target buffer, and when the job began.
    scene: Option<(u32, u32, u64, compose::scene_inflight)>,
    scenes: u32,
    /// The guests' contexts, and their fence waits still open.
    native: crate::amdgpu_virtio::vclients,
    waits: Vec<crate::amdgpu_virtio::fence_wait>,
    /// The scene sources that are guest VRAM objects: id and the VRAM
    /// offset of each page, for the pixel log.
    guest_bos: Vec<(u32, Vec<u64>)>,
    /// The buffers the scene being checked names (`buffers::surface`).
    named: core::cell::RefCell<Vec<u32>>,
}

impl service {
    /// A buffer the compositor's VM has, by kernel id.
    fn bo(&self, id: u32) -> Option<&bo> {
        let b = if (id as usize) < NUM_BOS {
            Some(&self.bos[id as usize])
        } else {
            self.dyn_bos.iter().find(|b| b.id == id)
        };
        b.filter(|b| b.node.is_some())
    }

    /// The buffers the scene just checked names: the kernel's own, and of
    /// the others only those, so that its job syncs with and fences no
    /// guest buffer it does not read.
    fn scene_bos(&self) -> Vec<usize> {
        let named = self.named.borrow();
        self.bos.iter().chain(self.dyn_bos.iter().filter(|b| named.contains(&b.id)))
            .flat_map(|b| b.pieces.iter().flatten().map(|p| p.0).chain(b.linear.map(|g| g.bo)))
            .collect()
    }

    /// The tiled guest frames the scene just checked shows, imported
    /// before it is composed.
    fn imports(&self) -> Vec<compose::frame_import> {
        let named = self.named.borrow();
        self.dyn_bos.iter().filter(|b| named.contains(&b.id)).filter_map(|b| b.import).collect()
    }
}

impl gpu_scene::scene::buffers for service {
    fn surface(&self, id: u32) -> Option<gpu_scene::scene::surface> {
        let mut named = self.named.borrow_mut();
        if !named.contains(&id) {
            named.push(id);
        }
        drop(named);
        self.bo(id).map(|b| gpu_scene::scene::surface { va: b.va, pitch: b.pitch, width: b.width, height: b.height,
                                                        size: b.size })
    }
}

/// `OP_BO_ADD`: the buffer imported into the compositor's VM and its
/// address reported.
fn bo_add(adev: &mut amdgpu_device, svc: &mut service, id: u32) -> Result<(), i32> {
    if (id as usize) < NUM_BOS || svc.dyn_bos.iter().any(|b| b.id == id) {
        return Err(-EINVAL);
    }
    if svc.dyn_bos.len() == MAX_DYN {
        return Err(-ENOSPC);
    }
    if svc.composer.is_none() {
        return Err(-EINVAL);
    }
    let mut b = bo_info(id).ok_or(-EINVAL)?;
    let pages = bo_page_list(id, &b)?;
    // A guest's VRAM object comes back as itself (`drm_gem_prime_import` of
    // a dma-buf this device exported): its pages lie in the CPU's window
    // onto VRAM.
    let (base, size) = (adev.gmc.aper_base, adev.gmc.aper_size);
    let offsets: Option<Vec<u64>> =
        pages.iter().map(|&p| p.checked_sub(base).filter(|&o| o < size)).collect();
    let own = offsets.as_ref().and_then(|o| crate::amdgpu_virtio::guest_vram_object(adev, &svc.native, o));
    log_bo_add(adev, id, &b, &pages, offsets.as_deref(), own);
    let guest = own.and(offsets);
    let h = match own {
        Some(g) => {
            let h = obj::amdgpu_bo_ref(adev, g);
            let r = obj::amdgpu_bo_pin(adev, h, AMDGPU_GEM_DOMAIN_VRAM);
            if r != 0 {
                obj::amdgpu_bo_unref(adev, h);
                return Err(r);
            }
            h
        }
        None => import_pages(adev, pages)?,
    };
    b.handle = Some(h);
    let tiled = match own {
        Some(_) => guest_frame(adev, &b, h),
        None => None,
    };
    let c = svc.composer.as_mut().ok_or(-EINVAL)?;
    let mapped = match tiled {
        Some(surf) => map_tiled(adev, c, &mut b, &surf),
        None => map_import(adev, c, &mut b),
    };
    if let Err(e) = mapped {
        obj::amdgpu_bo_unpin(adev, h);
        obj::amdgpu_bo_unref(adev, h);
        return Err(e);
    }
    npk_sys::gpudev_bo_bound(id as i32, b.va as i64);
    if let Some(o) = guest {
        svc.guest_bos.push((id, o));
    }
    svc.dyn_bos.push(b);
    Ok(())
}

/// The pixels at the start and the centre of `b`, whose pages lie at VRAM
/// offsets `o`.
fn pixels(adev: &amdgpu_device, l: &mut Line, b: &bo, o: &[u64]) {
    let at = |byte: u64| -> Option<u32> {
        let page = (byte / PAGE) as usize;
        o.get(page).map(|&v| crate::amdgpu_gart::kptr_r32(adev, obj::amdgpu_kptr::vram(v), byte % PAGE))
    };
    let base = b.offset as u64;
    let centre = base + (b.height / 2) as u64 * b.pitch as u64 + (b.width / 2) as u64 * 4;
    for byte in [base, base + 4, base + 8, centre, centre + 4] {
        l.s(" ").hex(at(byte).unwrap_or(0));
    }
}

/// The pixels at the start and the centre of `b`'s linear buffer `g`, as
/// the import left them.
fn linear_pixels(adev: &amdgpu_device, l: &mut Line, b: &bo, g: &compose::gem) {
    let centre = (b.height / 2) as u64 * b.pitch as u64 + (b.width / 2) as u64 * 4;
    for byte in [0, 4, 8, centre, centre + 4] {
        l.s(" ").hex(crate::amdgpu_gart::kptr_r32(adev, g.k, byte));
    }
}

/// Every 600 scenes, the pixels of each guest frame being composed: of a
/// tiled frame those its import wrote.
fn log_guest_pixels(adev: &amdgpu_device, svc: &service) {
    for (id, o) in &svc.guest_bos {
        let Some(b) = svc.dyn_bos.iter().find(|b| b.id == *id) else { continue };
        let mut l = Line::new();
        l.s("gpudev: scene ").dec(svc.scenes as u64).s(", guest bo ").dec(*id as u64);
        match &b.linear {
            Some(g) => {
                l.s(" imported pixels");
                linear_pixels(adev, &mut l, b, g);
            }
            None => {
                l.s(" pixels");
                pixels(adev, &mut l, b, o);
            }
        }
        l.say();
    }
}

/// The layout of guest object `h` as radeonsi imports it, when it is tiled
/// and the texture unit may read it: the frame starts the object, and the
/// image and its DCC lie inside it. A linear frame, or one refused, is
/// read as it lies.
fn guest_frame(adev: &amdgpu_device, b: &bo, h: usize) -> Option<crate::import::radeon_surf> {
    let ob = obj::bo(adev, h);
    let (swizzle, _) = crate::import::ac_surface_apply_bo_metadata(ob.tiling_flags);
    if swizzle == crate::import::ADDR_SW_LINEAR {
        return None;
    }
    let md: Vec<u32> = ob.metadata.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
    let r = if b.offset != 0 {
        Err(crate::import::refusal::unmodelled("offset"))
    } else {
        crate::import::import(adev.device, b.width, b.height, ob.tiling_flags, &md)
            .and_then(|s| crate::import::check(&s, obj::amdgpu_bo_size(adev, h)).map(|_| s))
    };
    let mut l = Line::new();
    l.s("gpudev: bo ").dec(b.id as u64);
    match r {
        Ok(s) => {
            l.s(" tiled, swizzle ").dec(s.swizzle_mode as u64).s(", ").dec(s.surf_pitch as u64).s("x")
                .dec(s.surf_height as u64);
            if crate::import::dcc_enabled(&s) {
                l.s(", DCC at ").dec(s.meta_offset).s(" (").dec(s.meta_size as u64).s(" bytes, aligned ")
                    .dec(s.pipe_aligned as u64).s(")");
            }
            l.s(": through the texture unit into a linear buffer").say();
            Some(s)
        }
        Err(e) => {
            l.s(" tiled, swizzle ").dec(swizzle as u64).s(", read as it lies: ").s(match e {
                crate::import::refusal::unmodelled(w) => w,
                crate::import::refusal::surface(_) => "addrlib refuses the layout",
                crate::import::refusal::metadata => "metadata refused",
                crate::import::refusal::bounds(w) => w,
            });
            if let crate::import::refusal::bounds(_) = e {
                l.s(" past the object");
            }
            l.say();
            None
        }
    }
}

/// A tiled guest frame in the compositor's VM: its whole object, which the
/// descriptor reads from its start, at the alignment of its swizzle mode
/// and its DCC (the low bits of a descriptor's address are the pipe and
/// bank XOR, radeonsi's VA allocator aligns a BO to its surface), and the
/// linear buffer the scenes see.
fn map_tiled(adev: &mut amdgpu_device, c: &mut composer, b: &mut bo, surf: &crate::import::radeon_surf)
             -> Result<(), i32> {
    let h = b.handle.ok_or(-EINVAL)?;
    let size = obj::amdgpu_bo_size(adev, h);
    let align = crate::import::alignment(surf);
    if let Err(e) = map_vm(adev, c, b, &[(h, false, 0, size)], align) {
        Line::new().s("gpudev: bo ").dec(b.id as u64).s(": its object of ").dec(size).s(" bytes not mapped (")
            .dec((-e) as u64).s(")").say();
        return Err(e);
    }
    let pitch_px = b.width.next_multiple_of(64);
    let bytes = pitch_px as u64 * 4 * b.height as u64;
    let g = match compose::linear_new(adev, c, bytes) {
        Ok(g) => g,
        Err(e) => {
            Line::new().s("gpudev: bo ").dec(b.id as u64).s(": no linear buffer of ").dec(bytes).s(" bytes (")
                .dec((-e) as u64).s(")").say();
            unmap_vm(adev, c, b);
            return Err(e);
        }
    };
    let va = b.node.unwrap().start;
    let desc = crate::import::descriptor(surf, b.width, b.height, va);
    Line::new().s("gpudev: bo ").dec(b.id as u64).s(" object at 0x").hexw((va >> 32) as u32, 8).hexw(va as u32, 8)
        .s("+0x").hexu(size as u32).s(", linear at 0x").hexw((g.va >> 32) as u32, 8).hexw(g.va as u32, 8).s("+0x")
        .hexu(bytes as u32).say();
    b.import = Some(compose::frame_import { desc, dst: g.va, dst_pitch_px: pitch_px, width: b.width,
                                            height: b.height });
    b.linear = Some(g);
    b.va = g.va;
    b.pitch = pitch_px * 4;
    b.size = bytes;
    Ok(())
}

/// The lines for a buffer added: how it is reached and, for a guest object,
/// its tiling, the pixels at its start and centre, and its UMD metadata.
fn log_bo_add(adev: &amdgpu_device, id: u32, b: &bo, pages: &[u64], offsets: Option<&[u64]>, own: Option<usize>) {
    let mut l = Line::new();
    l.s("gpudev: bo ").dec(id as u64).s(" ").dec(b.width as u64).s("x").dec(b.height as u64).s(" pitch ")
        .dec(b.pitch as u64).s(" offset ").dec(b.offset as u64).s(", ").dec(pages.len() as u64).s(" pages");
    match (offsets, own) {
        (Some(o), Some(h)) => {
            let ob = obj::bo(adev, h);
            let t = ob.tiling_flags;
            let field = |shift: u32, mask: u32| (t >> shift) & mask as u64;
            l.s(": guest VRAM object of ").dec(obj::amdgpu_bo_size(adev, h) / PAGE).s(" pages").say();
            let mut l = Line::new();
            l.s("gpudev: bo ").dec(id as u64).s(" tiling 0x").hexw((t >> 32) as u32, 8).hexw(t as u32, 8).s(" (swizzle ")
                .dec(field(AMDGPU_TILING_SWIZZLE_MODE_SHIFT, AMDGPU_TILING_SWIZZLE_MODE_MASK)).s(", dcc offset ")
                .dec(field(AMDGPU_TILING_DCC_OFFSET_256B_SHIFT, AMDGPU_TILING_DCC_OFFSET_256B_MASK) * 256)
                .s(", dcc pitch ").dec(field(AMDGPU_TILING_DCC_PITCH_MAX_SHIFT, AMDGPU_TILING_DCC_PITCH_MAX_MASK))
                .s(", scanout ").dec(field(AMDGPU_TILING_SCANOUT_SHIFT, 1)).s("), metadata ")
                .dec(ob.metadata.len() as u64).s(" bytes").say();
            let mut l = Line::new();
            l.s("gpudev: bo ").dec(id as u64).s(" pixels");
            pixels(adev, &mut l, b, o);
            l.say();
            log_bo_metadata(id, &ob.metadata);
            return;
        }
        (Some(_), None) => {
            l.s(": in the VRAM window but no guest object matches, as system pages");
        }
        (None, _) => {
            l.s(": system pages from 0x").hexw((pages[0] >> 32) as u32, 8).hexw(pages[0] as u32, 8);
        }
    }
    l.say();
}

/// The guest object's UMD metadata, eight words per line from word `i`.
fn log_bo_metadata(id: u32, md: &[u8]) {
    for (row, words) in md.chunks(32).enumerate() {
        let mut l = Line::new();
        l.s("gpudev: bo ").dec(id as u64).s(" metadata[").dec(row as u64 * 8).s("]");
        for w in words.chunks(4) {
            let mut v = [0u8; 4];
            v[..w.len()].copy_from_slice(w);
            l.s(" ").hex(u32::from_le_bytes(v));
        }
        l.say();
    }
}

/// `OP_BO_REMOVE`: unmapped once no scene uses it, and released. A buffer
/// whose `OP_BO_ADD` failed is not known and needs nothing.
fn bo_remove(adev: &mut amdgpu_device, svc: &mut service, id: u32) -> Result<(), i32> {
    if (id as usize) < NUM_BOS {
        return Err(-EINVAL);
    }
    svc.guest_bos.retain(|g| g.0 != id);
    let Some(i) = svc.dyn_bos.iter().position(|b| b.id == id) else { return Ok(()) };
    drain_scene(adev, svc);
    let mut b = svc.dyn_bos.remove(i);
    if let Some(c) = svc.composer.as_mut() {
        unmap_vm(adev, c, &mut b);
        if let Some(g) = b.linear.take() {
            compose::linear_free(adev, c, g);
        }
    }
    if let Some(h) = b.handle.take() {
        obj::amdgpu_bo_unpin(adev, h);
        obj::amdgpu_bo_unref(adev, h);
    }
    Ok(())
}

/// One scene job: on the compute ring, its job left open until
/// `poll_scene` sees it done.
fn run_scene(adev: &mut amdgpu_device, svc: &mut service, seq: u32, target: u32, bytes: u32) -> i32 {
    drain_scene(adev, svc);
    svc.named.borrow_mut().clear();
    let Some(mut c) = svc.composer.take() else { return -EINVAL };
    let t0 = crate::linux::now_us();
    let r = compose::submit_scene(adev, &mut c, &*svc, &|| (svc.scene_bos(), svc.imports()), seq, target, bytes);
    svc.composer = Some(c);
    match r {
        Ok(s) => {
            svc.scene = Some((seq, target, t0, s));
            JOB_OPEN
        }
        Err(e) => {
            Line::new().s("gpudev: scene into bo ").dec(target as u64).s(" refused (").dec((-e) as u64).s(")").say();
            e
        }
    }
}

/// Complete the scene's job once its fence signaled. True when it was.
fn poll_scene(adev: &mut amdgpu_device, svc: &mut service) -> bool {
    let Some((_, _, _, s)) = svc.scene.as_ref() else { return false };
    let Some(r) = compose::poll_scene(adev, s) else { return false };
    let (seq, target, t0, _) = svc.scene.take().unwrap();
    svc.scenes += 1;
    if svc.scenes % 600 == 0 {
        log_guest_pixels(adev, svc);
    }
    match &r {
        Ok(rep) if svc.scenes <= SCENES_LOGGED && crate::linux::debug_enabled() => {
            let us = crate::linux::now_us() - t0;
            let mut sclk = 0;
            let clk = crate::amdgpu_smu::amdgpu_dpm_read_sensor_gfx_sclk(adev, &mut sclk);
            let mut l = Line::new();
            l.s("gpudev: scene ").dec(svc.scenes as u64).s(", ").dec(rep.items as u64).s(" items, ")
                .dec(rep.dispatches as u64).s(" dispatches into bo ").dec(target as u64).s(": ").dec(us)
                .s(" us, of which the GPU ").dec(rep.gpu_us).s(" us");
            if clk == 0 {
                l.s(" at ").dec((sclk / 100) as u64).s(" MHz");
            }
            l.say();
            rep.log.say();
        }
        Err(e) => {
            Line::new().s("gpudev: scene into bo ").dec(target as u64).s(" failed (").dec((-*e) as u64).s(")").say();
        }
        _ => {}
    }
    npk_sys::gpudev_complete(seq as i32, match r {
        Ok(_) => 0,
        Err(x) => x,
    });
    true
}

/// Wait for the scene on the compute ring: a job that touches the
/// desktop's buffers runs, and completes, after it.
fn drain_scene(adev: &mut amdgpu_device, svc: &mut service) {
    while svc.scene.is_some() && !poll_scene(adev, svc) {
        crate::amdgpu_irq::amdgpu_irq_wait(adev, 2);
        crate::workqueue::run_pending(adev);
    }
}

/// `OP_CCMD`: the guest's command from the job's payload, run, its reply
/// handed to the kernel before the job completes.
fn run_ccmd(adev: &mut amdgpu_device, svc: &mut service, seq: u32, client: u32, len: u32) -> i32 {
    let len = len as usize;
    if len > CCMD_MAX {
        return -EINVAL;
    }
    let mut req = Vec::new();
    if req.try_reserve_exact(len).is_err() {
        return -ENOMEM;
    }
    req.resize(len, 0);
    let mut at = 0usize;
    while at < len {
        let n = npk_sys::gpudev_job_data(seq as i32, at as i32, &mut req[at..len]);
        if n <= 0 {
            return -EINVAL;
        }
        at += n as usize;
    }
    match crate::amdgpu_virtio::ccmd(adev, &mut svc.native, client, &req) {
        Ok(crate::amdgpu_virtio::ccmd_result::reply(r)) if r.len() <= REPLY_MAX => {
            if npk_sys::gpudev_reply(seq as i32, &r) != 0 { -ENOMEM } else { 0 }
        }
        Ok(crate::amdgpu_virtio::ccmd_result::reply(_)) => -EINVAL,
        Ok(crate::amdgpu_virtio::ccmd_result::wait(mut w)) => {
            w.seq = seq;
            svc.waits.push(w);
            JOB_OPEN
        }
        Err(e) => e,
    }
}

/// Fence waits whose fence signaled or whose time is up: replied and
/// completed.
fn poll_waits(adev: &mut amdgpu_device, svc: &mut service) {
    let mut i = 0;
    while i < svc.waits.len() {
        match crate::amdgpu_virtio::fence_wait_reply(adev, &svc.native, &svc.waits[i]) {
            Some(r) => {
                let w = svc.waits.swap_remove(i);
                let st = if npk_sys::gpudev_reply(w.seq as i32, &r) != 0 { -ENOMEM } else { 0 };
                npk_sys::gpudev_complete(w.seq as i32, st);
            }
            None => i += 1,
        }
    }
}

/// One job from the kernel; its status, or `JOB_OPEN`.
fn run_job(adev: &mut amdgpu_device, svc: &mut service, job: &[u8]) -> i32 {
    let w = |i: usize| u32::from_le_bytes(job[i * 4..i * 4 + 4].try_into().unwrap());
    let seq = w(0);
    match w(1) {
        OP_COPY_RECT => {
            if !svc.copy_ok {
                return -EINVAL;
            }
            let (src, dst, x, y, cw, ch) = (w(2) as usize, w(3) as usize, w(4), w(5), w(6), w(7));
            if src >= NUM_BOS || dst >= NUM_BOS || src == dst || !rect_fits(&svc.bos[src], x, y, cw, ch)
                || !rect_fits(&svc.bos[dst], x, y, cw, ch) {
                return -EINVAL;
            }
            drain_scene(adev, svc);
            let (s, d) = (svc.bos[src], svc.bos[dst]);
            match copy_rect(adev, s.gpu, s.pitch, d.gpu, d.pitch, x, y, cw, ch) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        OP_BO_ADD => match bo_add(adev, svc, w(2)) {
            Ok(()) => 0,
            Err(e) => {
                Line::new().s("gpudev: bo ").dec(w(2) as u64).s(" not added (").dec((-e) as u64).s(")").say();
                e
            }
        },
        OP_BO_REMOVE => match bo_remove(adev, svc, w(2)) {
            Ok(()) => 0,
            Err(e) => e,
        },
        OP_SCENE if svc.composer.is_some() => run_scene(adev, svc, seq, w(2), w(3)),
        OP_CLIENT => {
            if w(3) != 0 {
                crate::amdgpu_virtio::client_open(adev, &mut svc.native, w(2))
            } else {
                crate::amdgpu_virtio::client_close(adev, &mut svc.native, w(2))
            }
        }
        OP_CCMD => run_ccmd(adev, svc, seq, w(2), w(3)),
        OP_BLOB => match crate::amdgpu_virtio::get_blob(adev, &mut svc.native, w(2), w(3), w(4)) {
            Ok(r) => {
                if npk_sys::gpudev_reply(seq as i32, &r) != 0 { -ENOMEM } else { 0 }
            }
            Err(e) => e,
        },
        OP_BLOB_RELEASE => crate::amdgpu_virtio::release_blob(adev, &mut svc.native, w(2), w(3)),
        OP_ATTACH if w(6) != 0 => crate::amdgpu_virtio::attach(adev, &mut svc.native, w(2), w(3), w(4), w(5)),
        // drm_context_detach_resource
        OP_ATTACH => crate::amdgpu_virtio::release_blob(adev, &mut svc.native, w(2), w(3)),
        _ => -EINVAL,
    }
}

/// Everything the service holds goes back.
fn unbind(adev: &mut amdgpu_device, svc: &mut service) {
    drain_scene(adev, svc);
    if let Some(mut c) = svc.composer.take() {
        for b in svc.bos.iter_mut().chain(svc.dyn_bos.iter_mut()) {
            unmap_vm(adev, &mut c, b);
        }
        compose::composer_fini(adev, c);
    }
    for b in svc.bos.iter_mut().chain(svc.dyn_bos.iter_mut()) {
        if let Some(h) = b.handle.take() {
            obj::amdgpu_bo_unpin(adev, h);
            obj::amdgpu_bo_unref(adev, h);
        }
    }
    svc.dyn_bos.clear();
    crate::amdgpu_gart::amdgpu_gart_invalidate_tlb(adev);
}

/// The compositor's client with the fixed buffers in its VM, proven by a
/// dispatch; None leaves the scenes on the CPU.
fn scenes_init(adev: &mut amdgpu_device, svc: &mut service) -> bool {
    let mut c = match compose::composer_init(adev) {
        Ok(c) => c,
        Err(e) => {
            Line::new().s("gpudev: no compositor client (").dec((-e) as u64).s("), scenes stay on the CPU").say();
            return false;
        }
    };
    let mut ok = map_framebuffer(adev, &mut c, &mut svc.bos[0]).is_ok();
    for id in 1..NUM_BOS {
        let mut b = svc.bos[id];
        ok = ok && map_import(adev, &mut c, &mut b).is_ok();
        svc.bos[id] = b;
    }
    ok = ok && compose::self_test(adev, &mut c);
    Line::new().s("gpudev: compute dispatch self-test ").s(if ok { "passed" } else { "FAILED" }).say();
    if !ok {
        for b in svc.bos.iter_mut() {
            unmap_vm(adev, &mut c, b);
        }
        compose::composer_fini(adev, c);
        return false;
    }
    svc.composer = Some(c);
    true
}

/// Register with the kernel, bind its buffers, prove the copy and the
/// dispatch, and serve until a copy times out.
pub fn serve(adev: &mut amdgpu_device) {
    let rc = npk_sys::gpudev_register(VERSION);
    if rc != 0 {
        Line::new().s("gpudev: register failed (").dec((-rc) as u64).s(")").say();
        return;
    }
    let mut svc = service { bos: [bo::default(); NUM_BOS], dyn_bos: Vec::new(), copy_ok: false, composer: None,
                            scene: None, scenes: 0, native: Default::default(), waits: Vec::new(),
                            guest_bos: Vec::new(), named: core::cell::RefCell::new(Vec::new()) };
    for id in 0..NUM_BOS {
        let Some(info) = bo_info(id as u32) else {
            Line::new().s("gpudev: no buffer ").dec(id as u64).say();
            npk_sys::gpudev_ready(0);
            return;
        };
        svc.bos[id] = info;
    }
    let fb = svc.bos[0];
    Line::new().s("gpudev: ").dec(fb.width as u64).s("x").dec(fb.height as u64).s(", pitch ")
        .dec(fb.pitch as u64).s(", framebuffer at bus 0x").hexw((fb.phys >> 32) as u32, 8)
        .hexw(fb.phys as u32, 8).say();
    let mut bound = bind_framebuffer(adev, &mut svc.bos[0]);
    if !bound {
        Line::new().s("gpudev: the framebuffer is not in visible VRAM").say();
    }
    for id in 1..NUM_BOS {
        if !bound {
            break;
        }
        let mut b = svc.bos[id];
        if let Err(e) = bind_shadow(adev, id as u32, &mut b) {
            Line::new().s("gpudev: shadow ").dec(id as u64).s(" could not be bound (").dec((-e) as u64).s(")")
                .say();
            bound = false;
        }
        svc.bos[id] = b;
    }
    if !bound {
        unbind(adev, &mut svc);
        npk_sys::gpudev_ready(0);
        return;
    }
    for (id, b) in svc.bos.iter().enumerate() {
        npk_sys::gpudev_bo_bound(id as i32, b.gpu as i64);
        Line::new().s("gpudev: bo ").dec(id as u64).s(" at 0x").hexw((b.gpu >> 32) as u32, 8).hexw(b.gpu as u32, 8)
            .dbg();
    }
    svc.copy_ok = self_test(adev);
    Line::new().s("gpudev: sdma0 self-test ").s(if svc.copy_ok { "passed" } else { "FAILED" }).say();
    let scene_ok = svc.copy_ok && scenes_init(adev, &mut svc);
    // An APU's VRAM is the carve-out the CPU reaches whole (`aper_base`,
    // as gmc_v9_0_mc_init sets it); BAR0 shows only its start.
    if adev.gmc.aper_base != adev.bar0.0 {
        npk_sys::gpudev_vram(adev.gmc.aper_base as i64, adev.gmc.aper_size as i64);
    }
    // The capset for the guests' native contexts, before ready. They need
    // the compute and SDMA submission the scene self-test has just proven.
    let capset = match crate::amdgpu_virtio::capset(adev) {
        Ok(c) => npk_sys::gpudev_capset(&c) == 0,
        Err(e) => {
            Line::new().s("gpudev: no capset for native contexts (").dec((-e) as u64).s(")").say();
            false
        }
    };
    let native_ok = scene_ok && capset;
    let caps = if svc.copy_ok { CAP_COPY } else { 0 } | if scene_ok { CAP_SCENE } else { 0 }
        | if native_ok { CAP_NATIVE } else { 0 };
    npk_sys::gpudev_ready(caps);
    if caps == 0 {
        unbind(adev, &mut svc);
        return;
    }
    Line::new().s("gpudev: serving copies").s(if scene_ok { " and scenes" } else { "" })
        .s(if native_ok { " and native contexts" } else { "" }).say();

    let mut jobs = [0u8; JOB_BYTES * 8];
    let (mut done, mut failed) = (0u64, 0u64);
    loop {
        let idle_ms = if svc.scene.is_some() { 2 } else { 100 };
        npk_sys::wait(WAIT_IRQ | WAIT_GPU_CMD, crate::amdgpu_virtio::fence_wait_ms(&svc.waits).min(idle_ms));
        crate::amdgpu_irq::amdgpu_ih_process(adev, false);
        crate::workqueue::run_pending(adev);
        poll_scene(adev, &mut svc);
        crate::amdgpu_virtio::poll_rings(adev, &mut svc.native);
        poll_waits(adev, &mut svc);
        let n = npk_sys::gpudev_poll(&mut jobs);
        // The kernel no longer has this module as its service.
        if n < 0 {
            Line::new().s("gpudev: no longer the service after ").dec(done).s(" jobs (").dec(failed)
                .s(" refused, ").dec(svc.scenes as u64).s(" scenes)").say();
            break;
        }
        let mut timed_out = false;
        for j in 0..n.max(0) as usize {
            let job = &jobs[j * JOB_BYTES..(j + 1) * JOB_BYTES];
            let seq = u32::from_le_bytes(job[0..4].try_into().unwrap());
            let status = run_job(adev, &mut svc, job);
            if status == JOB_OPEN {
                continue;
            }
            if status == 0 { done += 1 } else { failed += 1 }
            timed_out |= status == -ETIME;
            npk_sys::gpudev_complete(seq as i32, status);
        }
        // A fence wait with no time left (a poll) answers now.
        poll_waits(adev, &mut svc);
        if timed_out {
            Line::new().s("gpudev: a copy on sdma0 timed out after ").dec(done).s(" jobs (").dec(failed)
                .s(" refused), giving the card back").say();
            break;
        }
    }
    unbind(adev, &mut svc);
}
