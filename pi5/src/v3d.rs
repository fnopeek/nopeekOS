//! The Pi 5's V3D 7.1 as far as the kernel owns it, after Linux:
//! `drivers/pmdomain/bcm/bcm2835-power.c` (the GRAFX and V3D power
//! domains of `brcm,bcm2712-pm`), `drivers/gpu/drm/v3d/v3d_power.c` (the
//! SMS), `v3d_gem.c` (`v3d_init_core`, `v3d_reset_sms`), `v3d_drv.c` (the
//! identification) and `v3d_mmu.c` (the page table). Jobs, caches and
//! interrupts are the driver module's; the page table, and so what memory
//! V3D reaches, stays here.
//!
//! [`Va`] hands out V3D addresses: Linux's `drm_mm` over the 4 GiB the
//! MMU translates, page by page.

use crate::hw::{self, Block, Hw};
use alloc::vec::Vec;

// bcm2835-power.c
const PM_PASSWORD: u32 = 0x5a00_0000;
const PM_GRAFX: u64 = 0x10c;
const PM_GRAFX_2712: u64 = 0x304;
const PM_V3DRSTN: u32 = 1 << 6;
const PM_ISFUNC: u32 = 1 << 5;
const PM_MRDONE: u32 = 1 << 4;
const PM_MEMREP: u32 = 1 << 3;
const PM_ISPOW: u32 = 1 << 2;
const PM_POWOK: u32 = 1 << 1;
const PM_POWUP: u32 = 1 << 0;
const PM_INRUSH_SHIFT: u32 = 13;
const PM_INRUSH_MASK: u32 = 3 << PM_INRUSH_SHIFT;
const PM_INRUSH_3_5_MA: u32 = 0;
const PM_INRUSH_20_MA: u32 = 3;

// v3d_regs.h
const V3D_HUB_IDENT1: u64 = 0x0000c;
const V3D_HUB_IDENT3: u64 = 0x00014;
const V3D_CTL_IDENT1: u64 = 0x00004;
const V3D_CTL_L2TFLSTA: u64 = 0x00034;
const V3D_CTL_L2TFLEND: u64 = 0x00038;
const V3D_MMUC_CONTROL: u64 = 0x01000;
const V3D_MMUC_CONTROL_FLUSHING: u32 = 1 << 2;
const V3D_MMUC_CONTROL_FLUSH: u32 = 1 << 1;
const V3D_MMUC_CONTROL_ENABLE: u32 = 1 << 0;
const V3D_MMU_CTL: u64 = 0x01200;
const V3D_MMU_CTL_CAP_EXCEEDED_ABORT: u32 = 1 << 26;
const V3D_MMU_CTL_CAP_EXCEEDED_INT: u32 = 1 << 25;
const V3D_MMU_CTL_PT_INVALID_ABORT: u32 = 1 << 19;
const V3D_MMU_CTL_PT_INVALID_INT: u32 = 1 << 18;
const V3D_MMU_CTL_PT_INVALID_ENABLE: u32 = 1 << 16;
const V3D_MMU_CTL_WRITE_VIOLATION_ABORT: u32 = 1 << 11;
const V3D_MMU_CTL_WRITE_VIOLATION_INT: u32 = 1 << 10;
const V3D_MMU_CTL_TLB_CLEARING: u32 = 1 << 7;
const V3D_MMU_CTL_TLB_CLEAR: u32 = 1 << 2;
const V3D_MMU_CTL_ENABLE: u32 = 1 << 0;
const V3D_MMU_PT_PA_BASE: u64 = 0x01204;
const V3D_MMU_ILLEGAL_ADDR: u64 = 0x01230;
const V3D_MMU_ILLEGAL_ADDR_ENABLE: u32 = 1 << 31;
const V3D_MMU_DEBUG_INFO: u64 = 0x01238;
const V3D_MMU_VA_WIDTH_MASK: u32 = 0xf << 4;
const V3D_SMS_REE_CS: u64 = 0x00000;
const V3D_SMS_TEE_CS: u64 = 0x00400;
const V3D_SMS_CLEAR_POWER_OFF: u32 = 1 << 29;
const V3D_SMS_POWER_OFF: u32 = 1 << 30;
const V3D_GMP_STATUS: u64 = 0x00600;
const V3D_GMP_STATUS_WR_COUNT_MASK: u32 = 0x7f << 24;
const V3D_GMP_STATUS_RD_COUNT_MASK: u32 = 0x7f << 16;
const V3D_GMP_STATUS_CFG_BUSY: u32 = 1 << 3;
const V3D_GMP_CFG: u64 = 0x00604;
const V3D_GMP_CFG_STOP_REQ: u32 = 1 << 1;
const V3D_MMU_VIO_ID: u64 = 0x0122c;
const V3D_MMU_VIO_ADDR: u64 = 0x01234;
const V3D_SMS_STATE_MASK: u32 = 0xf;
// v3d_drv.h
const V3D_SMS_IDLE: u32 = 0x0;
const V3D_SMS_ISOLATING_FOR_RESET: u32 = 0xa;
const V3D_SMS_RESETTING: u32 = 0xb;
const V3D_SMS_POWER_OFF_STATE: u32 = 0xd;
// v3d_mmu.c
pub const V3D_MMU_PAGE_SHIFT: u32 = 12;
const V3D_PTE_SUPERPAGE: u32 = 1 << 31;
const V3D_PTE_BIGPAGE: u32 = 1 << 30;
const V3D_PTE_WRITEABLE: u32 = 1 << 29;
const V3D_PTE_VALID: u32 = 1 << 28;
const SZ_64K: u64 = 64 << 10;
const SZ_1M: u64 = 1 << 20;
/// `pt_size` in `v3d_gem_init`: one 4-byte entry per 4 KiB of 4 GiB.
pub const PT_ENTRIES: usize = 1 << 20;
pub const PT_BYTES: usize = PT_ENTRIES * 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// `Timeout waiting for %s power OK`.
    PowOk,
    /// `Timeout waiting for %s memory repair`.
    MemRepair,
    /// `Failed to power up SMS`.
    Sms,
    /// `Failed to power off SMS`.
    SmsOff,
    /// `MMUC flush wait idle failed` / `MMU TLB clear wait idle failed`.
    Flush,
    /// Not V3D 7.1.
    Version(u32),
}

/// `wait_for(cond, ms)`: poll every 10 us until `cond` or `ms` passed.
fn wait_for(hw: &mut impl Hw, ms: u64, mut cond: impl FnMut(&mut dyn FnMut(Block, u64) -> u32) -> bool) -> bool {
    let mut waited = 0;
    loop {
        if cond(&mut |b, off| hw.r32(b, off)) {
            return true;
        }
        if waited >= ms * 1000 {
            return cond(&mut |b, off| hw.r32(b, off));
        }
        hw.delay_us(10);
        waited += 10;
    }
}

fn pm_read(hw: &mut impl Hw, reg: u64) -> u32 { hw.r32(Block::Pm, reg) }
fn pm_write(hw: &mut impl Hw, reg: u64, v: u32) { hw.w32(Block::Pm, reg, PM_PASSWORD | v) }

/// `bcm2835_power_power_on` for the GRAFX domain (`PM_GRAFX`), which the
/// V3D domain sits in: left alone when the firmware powered it.
pub fn grafx_power_on(hw: &mut impl Hw) -> Result<(), Error> {
    let reg = PM_GRAFX;
    if pm_read(hw, reg) & PM_POWUP != 0 {
        return Ok(());
    }
    // Enable power, ramping the inrush current up until POWOK.
    let mut powok = false;
    for inrush in PM_INRUSH_3_5_MA..=PM_INRUSH_20_MA {
        let v = (pm_read(hw, reg) & !PM_INRUSH_MASK) | (inrush << PM_INRUSH_SHIFT) | PM_POWUP;
        pm_write(hw, reg, v);
        let mut waited = 0;
        loop {
            powok = pm_read(hw, reg) & PM_POWOK != 0;
            if powok || waited >= 3 {
                break;
            }
            hw.delay_us(1);
            waited += 1;
        }
    }
    if !powok {
        let v = pm_read(hw, reg) & !(PM_POWUP | PM_INRUSH_MASK);
        pm_write(hw, reg, v);
        return Err(Error::PowOk);
    }
    // Disable electrical isolation, repair memory, disable functional
    // isolation.
    let v = pm_read(hw, reg) | PM_ISPOW;
    pm_write(hw, reg, v);
    let v = pm_read(hw, reg) | PM_MEMREP;
    pm_write(hw, reg, v);
    if !hw::poll(hw, Block::Pm, reg, 1, 1, |v| v & PM_MRDONE != 0).0 {
        let v = pm_read(hw, reg) & !PM_ISPOW;
        pm_write(hw, reg, v);
        let v = pm_read(hw, reg) & !(PM_POWUP | PM_INRUSH_MASK);
        pm_write(hw, reg, v);
        return Err(Error::MemRepair);
    }
    let v = pm_read(hw, reg) | PM_ISFUNC;
    pm_write(hw, reg, v);
    Ok(())
}

/// `bcm2835_power_power_off` for the GRAFX domain: functional, then
/// electrical isolation, then the power switches opened.
pub fn grafx_power_off(hw: &mut impl Hw) {
    let reg = PM_GRAFX;
    let v = pm_read(hw, reg) & !PM_ISFUNC;
    pm_write(hw, reg, v);
    let v = pm_read(hw, reg) & !PM_ISPOW;
    pm_write(hw, reg, v);
    let v = pm_read(hw, reg) & !PM_POWUP;
    pm_write(hw, reg, v);
}

/// `bcm2835_asb_power_off` for GRAFX_V3D on BCM2712 (no async bridges, no
/// domain clock): the V3D reset asserted.
pub fn v3d_domain_off(hw: &mut impl Hw) {
    let v = pm_read(hw, PM_GRAFX_2712) & !PM_V3DRSTN;
    pm_write(hw, PM_GRAFX_2712, v);
}

/// `bcm2835_reset_reset` for `BCM2835_RESET_V3D`: the V3D domain off and
/// on again.
pub fn reset_v3d_domain(hw: &mut impl Hw) {
    v3d_domain_off(hw);
    v3d_domain_on(hw);
}

/// `bcm2835_asb_power_on` for GRAFX_V3D on BCM2712 (`PM_GRAFX_2712`, no
/// async bridges, no domain clock): 1 us for the reset to propagate, then
/// the V3D reset deasserted.
pub fn v3d_domain_on(hw: &mut impl Hw) {
    hw.delay_us(1);
    let v = pm_read(hw, PM_GRAFX_2712) | PM_V3DRSTN;
    pm_write(hw, PM_GRAFX_2712, v);
}

fn sms_state(v: u32) -> u32 { v & V3D_SMS_STATE_MASK }

/// `v3d_resume_sms`, with `v3d_reset_sms`.
pub fn resume_sms(hw: &mut impl Hw) -> Result<(), Error> {
    hw.w32(Block::V3dSms, V3D_SMS_TEE_CS, V3D_SMS_CLEAR_POWER_OFF);
    if !wait_for(hw, 100, |r| sms_state(r(Block::V3dSms, V3D_SMS_TEE_CS)) == V3D_SMS_IDLE) {
        return Err(Error::Sms);
    }
    reset_sms(hw);
    Ok(())
}

/// `v3d_suspend_sms`.
pub fn suspend_sms(hw: &mut impl Hw) -> Result<(), Error> {
    hw.w32(Block::V3dSms, V3D_SMS_TEE_CS, V3D_SMS_POWER_OFF);
    if !wait_for(hw, 100, |r| sms_state(r(Block::V3dSms, V3D_SMS_TEE_CS)) == V3D_SMS_POWER_OFF_STATE) {
        return Err(Error::SmsOff);
    }
    Ok(())
}

/// `v3d_idle_axi` on V3D 7.1: the GMP asked to stop, then waited for until
/// no read or write is outstanding. False on a timeout (Linux reports it
/// and goes on).
pub fn idle_axi(hw: &mut impl Hw) -> bool {
    hw.w32(Block::V3dHub, V3D_GMP_CFG, V3D_GMP_CFG_STOP_REQ);
    wait_for(hw, 100, |r| r(Block::V3dHub, V3D_GMP_STATUS)
        & (V3D_GMP_STATUS_RD_COUNT_MASK | V3D_GMP_STATUS_WR_COUNT_MASK | V3D_GMP_STATUS_CFG_BUSY) == 0)
}

/// `v3d_power_suspend` staying active after a failed SMS power-off: the
/// GMP stop request of `idle_axi` taken back.
pub fn gmp_resume(hw: &mut impl Hw) {
    let v = hw.r32(Block::V3dHub, V3D_GMP_CFG) & !V3D_GMP_CFG_STOP_REQ;
    hw.w32(Block::V3dHub, V3D_GMP_CFG, v);
}

/// What `v3d_hub_irq` reads after an MMU fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MmuFault {
    /// `V3D_MMU_VIO_ID`, the AXI ID of the client.
    pub axi_id: u32,
    /// `V3D_MMU_VIO_ADDR` shifted by `va_width - 32`.
    pub addr: u64,
}

/// `v3d_hub_irq`'s MMU part: the violating client and address read, the
/// error cleared by writing `V3D_MMU_CTL` back.
pub fn mmu_fault(hw: &mut impl Hw, va_width: u32) -> MmuFault {
    let axi_id = hw.r32(Block::V3dHub, V3D_MMU_VIO_ID);
    let addr = (hw.r32(Block::V3dHub, V3D_MMU_VIO_ADDR) as u64) << va_width.saturating_sub(32);
    let ctl = hw.r32(Block::V3dHub, V3D_MMU_CTL);
    hw.w32(Block::V3dHub, V3D_MMU_CTL, ctl);
    MmuFault { axi_id, addr }
}

/// `v3d_reset_sms`: a timeout is only reported by Linux.
pub fn reset_sms(hw: &mut impl Hw) -> bool {
    hw.w32(Block::V3dSms, V3D_SMS_REE_CS, 0x4);
    wait_for(hw, 100, |r| {
        let s = sms_state(r(Block::V3dSms, V3D_SMS_REE_CS));
        s != V3D_SMS_ISOLATING_FOR_RESET && s != V3D_SMS_RESETTING
    })
}

/// `v3d_init_core` on core 0 for V3D 4.1 and later: every L2T flush flushes
/// all of it.
pub fn init_hw_state(hw: &mut impl Hw) {
    hw.w32(Block::V3dCore, V3D_CTL_L2TFLSTA, 0);
    hw.w32(Block::V3dCore, V3D_CTL_L2TFLEND, !0);
}

/// What `v3d_platform_drm_probe` reads of the hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ident {
    /// `tver * 10 + rev`: 71.
    pub ver: u32,
    pub rev: u32,
    pub cores: u32,
    /// Slices times QPUs per slice (`V3D_CTL_IDENT1`), what v3dv's
    /// `qpu_count` is.
    pub qpus: u32,
    /// 30 + the MMU's VA width field.
    pub va_width: u32,
}

pub fn ident(hw: &mut impl Hw) -> Result<Ident, Error> {
    let ident1 = hw.r32(Block::V3dHub, V3D_HUB_IDENT1);
    let ver = (ident1 & 0xf) * 10 + ((ident1 >> 4) & 0xf);
    let cores = (ident1 >> 8) & 0xf;
    let rev = (hw.r32(Block::V3dHub, V3D_HUB_IDENT3) >> 8) & 0xff;
    let core1 = hw.r32(Block::V3dCore, V3D_CTL_IDENT1);
    let qpus = ((core1 >> 4) & 0xf) * ((core1 >> 8) & 0xf);
    let va_width = 30 + hw::field_get(hw.r32(Block::V3dHub, V3D_MMU_DEBUG_INFO), V3D_MMU_VA_WIDTH_MASK);
    if ver != 71 {
        return Err(Error::Version(ver));
    }
    Ok(Ident { ver, rev, cores, qpus, va_width })
}

/// `v3d_mmu_flush_all_locked`.
pub fn mmu_flush_all(hw: &mut impl Hw) -> Result<(), Error> {
    hw.w32(Block::V3dHub, V3D_MMUC_CONTROL, V3D_MMUC_CONTROL_FLUSH | V3D_MMUC_CONTROL_ENABLE);
    if !wait_for(hw, 100, |r| r(Block::V3dHub, V3D_MMUC_CONTROL) & V3D_MMUC_CONTROL_FLUSHING == 0) {
        return Err(Error::Flush);
    }
    let v = hw.r32(Block::V3dHub, V3D_MMU_CTL) | V3D_MMU_CTL_TLB_CLEAR;
    hw.w32(Block::V3dHub, V3D_MMU_CTL, v);
    if !wait_for(hw, 100, |r| r(Block::V3dHub, V3D_MMU_CTL) & V3D_MMU_CTL_TLB_CLEARING == 0) {
        return Err(Error::Flush);
    }
    Ok(())
}

/// `v3d_mmu_set_page_table`: the table at `pt_pa`, an access outside it
/// aborts and lands on the page at `scratch_pa`.
pub fn mmu_set_page_table(hw: &mut impl Hw, pt_pa: u64, scratch_pa: u64) -> Result<(), Error> {
    hw.w32(Block::V3dHub, V3D_MMU_PT_PA_BASE, (pt_pa >> V3D_MMU_PAGE_SHIFT) as u32);
    hw.w32(Block::V3dHub, V3D_MMU_CTL,
        V3D_MMU_CTL_ENABLE
            | V3D_MMU_CTL_PT_INVALID_ENABLE
            | V3D_MMU_CTL_PT_INVALID_ABORT
            | V3D_MMU_CTL_PT_INVALID_INT
            | V3D_MMU_CTL_WRITE_VIOLATION_ABORT
            | V3D_MMU_CTL_WRITE_VIOLATION_INT
            | V3D_MMU_CTL_CAP_EXCEEDED_ABORT
            | V3D_MMU_CTL_CAP_EXCEEDED_INT);
    hw.w32(Block::V3dHub, V3D_MMU_ILLEGAL_ADDR,
        (scratch_pa >> V3D_MMU_PAGE_SHIFT) as u32 | V3D_MMU_ILLEGAL_ADDR_ENABLE);
    hw.w32(Block::V3dHub, V3D_MMUC_CONTROL, V3D_MMUC_CONTROL_ENABLE);
    mmu_flush_all(hw)
}

fn is_aligned(page: u32, page_address: u32, alignment: u64) -> bool {
    let a = (alignment >> V3D_MMU_PAGE_SHIFT) as u32;
    page % a == 0 && page_address % a == 0
}

/// `v3d_mmu_insert_ptes` for a buffer at V3D page `start` made of the
/// physically contiguous runs `runs` (address, bytes; whole pages), the way
/// `for_each_sgtable_dma_sg` hands them over: 1 MiB and 64 KiB PTEs where
/// address, page and length allow. False when a frame lies beyond the
/// 24-bit PFN, nothing written then.
pub fn insert_ptes(pt: &mut [u32], start: u32, runs: &[(u64, u64)]) -> bool {
    if runs.iter().any(|&(a, l)| ((a + l) >> V3D_MMU_PAGE_SHIFT) >= 1 << 24) {
        return false;
    }
    let mut page = start;
    for &(dma_addr, len) in runs {
        let mut pfn = (dma_addr >> V3D_MMU_PAGE_SHIFT) as u32;
        let mut len = len;
        while len > 0 {
            let page_prot = V3D_PTE_WRITEABLE | V3D_PTE_VALID;
            let mut page_address = page_prot | pfn;
            let page_size = if len >= SZ_1M && is_aligned(page, page_address, SZ_1M) {
                page_address |= V3D_PTE_SUPERPAGE;
                SZ_1M
            } else if len >= SZ_64K && is_aligned(page, page_address, SZ_64K) {
                page_address |= V3D_PTE_BIGPAGE;
                SZ_64K
            } else {
                4096
            };
            for i in 0..(page_size >> V3D_MMU_PAGE_SHIFT) as u32 {
                pt[page as usize] = page_address + i;
                page += 1;
                pfn += 1;
            }
            len -= page_size;
        }
    }
    true
}

/// `v3d_mmu_remove_ptes`.
pub fn remove_ptes(pt: &mut [u32], start: u32, pages: u32) {
    for p in start..start + pages {
        pt[p as usize] = 0;
    }
}

/// Free V3D pages, first fit (`drm_mm_insert_node_generic` with
/// `GMP_GRANULARITY >> PAGE_SHIFT` alignment of 128 KiB, as `v3d_bo`
/// allocates).
pub struct Va {
    /// Sorted, disjoint (first page, pages).
    free: Vec<(u32, u32)>,
}

/// `GMP_GRANULARITY`: 128 KiB, the alignment of every V3D buffer.
pub const GMP_GRANULARITY_PAGES: u32 = (128 << 10) >> V3D_MMU_PAGE_SHIFT;

impl Va {
    /// Pages `first..end` of the address space; page 0 stays unused, so 0
    /// is never a valid V3D address.
    pub fn new(first: u32, end: u32) -> Va {
        Va { free: alloc::vec![(first, end - first)] }
    }

    /// `pages` pages at a 128 KiB-aligned start.
    pub fn alloc(&mut self, pages: u32) -> Option<u32> {
        let a = GMP_GRANULARITY_PAGES;
        for i in 0..self.free.len() {
            let (s, n) = self.free[i];
            let start = s.div_ceil(a) * a;
            let pad = start - s;
            if n < pad || n - pad < pages {
                continue;
            }
            let tail = (start + pages, n - pad - pages);
            self.free.remove(i);
            if tail.1 > 0 {
                self.free.insert(i, tail);
            }
            if pad > 0 {
                self.free.insert(i, (s, pad));
            }
            return Some(start);
        }
        None
    }

    /// Pages `start..start + pages` free again, merged with neighbours.
    pub fn free(&mut self, start: u32, pages: u32) {
        let i = self.free.partition_point(|&(s, _)| s < start);
        self.free.insert(i, (start, pages));
        if i + 1 < self.free.len() && self.free[i].0 + self.free[i].1 == self.free[i + 1].0 {
            self.free[i].1 += self.free[i + 1].1;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].0 + self.free[i - 1].1 == self.free[i].0 {
            self.free[i - 1].1 += self.free[i].1;
            self.free.remove(i);
        }
    }
}
