//! amdgpu/amdgpu_gart.c (Linux 6.18.55): the GART table in VRAM, the
//! dummy page, binding pages and flushing the hubs' TLBs.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_object::amdgpu_kptr;
use crate::linux::Line;
use crate::regs::*;

const PAGE_SIZE: u64 = 4096;

/// `struct amdgpu_gart`.
#[derive(Default)]
pub struct amdgpu_gart {
    pub bo: Option<usize>,
    /// `gart.ptr`: the table's kernel mapping.
    pub ptr: Option<amdgpu_kptr>,
    pub num_cpu_pages: u32,
    pub num_gpu_pages: u32,
    pub table_size: u64,
    pub gart_pte_flags: u64,
}

/// `amdgpu_gart_dummy_page_init`: TTM's zeroed dummy read page, here one
/// DMA page of the device's.
fn amdgpu_gart_dummy_page_init(adev: &mut amdgpu_device) -> i32 {
    if adev.dummy_page_addr != 0 {
        return 0;
    }
    let Some(d) = npk_sys::Dma::alloc(1) else {
        Line::new().s("Failed to DMA MAP the dummy page").say();
        return -12;
    };
    adev.dummy_page_addr = d.phys;
    0
}

/// `amdgpu_gart_init`.
pub fn amdgpu_gart_init(adev: &mut amdgpu_device) -> i32 {
    if adev.dummy_page_addr != 0 {
        return 0;
    }
    let r = amdgpu_gart_dummy_page_init(adev);
    if r != 0 {
        return r;
    }
    // Compute table size
    adev.gart.num_cpu_pages = (adev.gmc.gart_size / PAGE_SIZE) as u32;
    adev.gart.num_gpu_pages = (adev.gmc.gart_size / AMDGPU_GPU_PAGE_SIZE as u64) as u32;
    Line::new().s("GART: num cpu pages ").dec(adev.gart.num_cpu_pages as u64).s(", num gpu pages ")
        .dec(adev.gart.num_gpu_pages as u64).say();
    0
}

/// `amdgpu_gart_table_vram_alloc`.
pub fn amdgpu_gart_table_vram_alloc(adev: &mut amdgpu_device) -> i32 {
    if adev.gart.bo.is_some() {
        return 0;
    }
    let size = adev.gart.table_size;
    match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size, PAGE_SIZE, AMDGPU_GEM_DOMAIN_VRAM, true) {
        Ok(h) => adev.gart.bo = h,
        Err(r) => return r,
    }
    let h = adev.gart.bo.unwrap();
    adev.gart.ptr = crate::amdgpu_object::bo(adev, h).kptr;
    // memset_io(ptr, gart_pte_flags, table_size): every byte is the low
    // byte of the flags.
    let b = adev.gart.gart_pte_flags as u8 as u32;
    let fill = b * 0x0101_0101;
    let ptr = adev.gart.ptr.unwrap();
    kptr_fill32(adev, ptr, 0, size, fill);
    0
}

/// `amdgpu_gmc_set_pte_pde` (amdgpu_gmc.c): PTEs only, GART has no PDEs.
fn amdgpu_gmc_set_pte_pde(adev: &amdgpu_device, ptr: amdgpu_kptr, gpu_page_idx: u64, addr: u64, flags: u64) {
    let value = (addr & 0x0000_FFFF_FFFF_F000) | flags;
    kptr_writeq(adev, ptr, gpu_page_idx * 8, value);
}

/// `amdgpu_gart_map`.
pub fn amdgpu_gart_map(adev: &amdgpu_device, offset: u64, pages: u64, dma_addr: &[u64], flags: u64, dst: amdgpu_kptr) {
    let mut t = offset / AMDGPU_GPU_PAGE_SIZE as u64;
    for i in 0..pages as usize {
        let mut page_base = dma_addr[i];
        for _ in 0..AMDGPU_GPU_PAGES_IN_CPU_PAGE {
            amdgpu_gmc_set_pte_pde(adev, dst, t, page_base, flags);
            page_base += AMDGPU_GPU_PAGE_SIZE as u64;
            t += 1;
        }
    }
}

/// `amdgpu_gart_bind`.
pub fn amdgpu_gart_bind(adev: &amdgpu_device, offset: u64, pages: u64, dma_addr: &[u64], flags: u64) {
    let Some(ptr) = adev.gart.ptr else { return };
    amdgpu_gart_map(adev, offset, pages, dma_addr, flags, ptr);
}

/// `amdgpu_gart_invalidate_tlb`: an APU needs no HDP flush
/// (`amdgpu_device_flush_hdp`).
pub fn amdgpu_gart_invalidate_tlb(adev: &mut amdgpu_device) {
    if adev.gart.ptr.is_none() {
        return;
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    for i in 0..AMDGPU_MAX_VMHUBS {
        if adev.vmhubs_mask & (1 << i) != 0 {
            // amdgpu_gmc_flush_gpu_tlb: no SDMA invalidation workaround on
            // GC 9.3, no extra flush types.
            crate::gmc_v9_0::gmc_v9_0_flush_gpu_tlb(adev, 0, i, 0);
        }
    }
}

/// A 64-bit store through a kernel mapping (`writeq`).
pub fn kptr_writeq(adev: &amdgpu_device, k: amdgpu_kptr, off: u64, v: u64) {
    match k {
        amdgpu_kptr::vram(base) => adev.mman.aper_base_kaddr.unwrap().writeq((base + off) as u32, v),
        amdgpu_kptr::gtt(h) => {
            let d = npk_sys::Dma { handle: h, phys: 0 };
            d.w32(off as u32, v as u32);
            d.w32(off as u32 + 4, (v >> 32) as u32);
        }
        amdgpu_kptr::doorbell(base) => adev.doorbell_mmio.unwrap().writeq((base + off) as u32, v),
    }
}

/// A 32-bit store through a kernel mapping.
pub fn kptr_w32(adev: &amdgpu_device, k: amdgpu_kptr, off: u64, v: u32) {
    match k {
        amdgpu_kptr::vram(base) => adev.mman.aper_base_kaddr.unwrap().w32((base + off) as u32, v),
        amdgpu_kptr::gtt(h) => npk_sys::Dma { handle: h, phys: 0 }.w32(off as u32, v),
        amdgpu_kptr::doorbell(base) => adev.doorbell_mmio.unwrap().w32((base + off) as u32, v),
    }
}

/// A 32-bit load through a kernel mapping.
pub fn kptr_r32(adev: &amdgpu_device, k: amdgpu_kptr, off: u64) -> u32 {
    match k {
        amdgpu_kptr::vram(base) => adev.mman.aper_base_kaddr.unwrap().r32((base + off) as u32),
        amdgpu_kptr::gtt(h) => npk_sys::Dma { handle: h, phys: 0 }.r32(off as u32),
        amdgpu_kptr::doorbell(base) => adev.doorbell_mmio.unwrap().r32((base + off) as u32),
    }
}

/// `memset_io` / `memset32` over `len` bytes at `off`.
pub fn kptr_fill32(adev: &amdgpu_device, k: amdgpu_kptr, off: u64, len: u64, v: u32) {
    let mut o = 0;
    while o < len {
        kptr_w32(adev, k, off + o, v);
        o += 4;
    }
}

/// `memcpy_toio` / `memcpy` of `data` to `off`.
pub fn kptr_write(adev: &amdgpu_device, k: amdgpu_kptr, off: u64, data: &[u8]) {
    match k {
        amdgpu_kptr::gtt(h) => {
            if !(npk_sys::Dma { handle: h, phys: 0 }).write(off as u32, data) {
                crate::linux::Line::new().s("kptr_write: DMA write refused").say();
            }
        }
        _ => {
            for (i, c) in data.chunks(4).enumerate() {
                let mut w = [0u8; 4];
                w[..c.len()].copy_from_slice(c);
                if c.len() < 4 {
                    let old = kptr_r32(adev, k, off + i as u64 * 4).to_le_bytes();
                    w[c.len()..].copy_from_slice(&old[c.len()..]);
                }
                kptr_w32(adev, k, off + i as u64 * 4, u32::from_le_bytes(w));
            }
        }
    }
}
