//! The BCM2712 IOMMU in front of the HVS (iommu4) and the TLB cache it
//! shares (iommuc), after Linux `drivers/iommu/bcm2712-iommu.c` and
//! `bcm2712-iommu-cache.c`, with a 4 KiB `PAGE_SIZE` (Linux allows 4 KiB
//! and larger).
//!
//! Two-level tables of 4-byte entries, 1024 per 4 KiB table: one top-level
//! page of L2 page numbers, the L2 pages of PTEs. Table memory belongs to
//! the caller ([`Pages`]); the IOMMU does not snoop the CPU's caches, so
//! every change is handed over with `sync_for_device`.

use crate::hw::{field_get, Block, Hw};

pub const MMMU_CTRL_OFFSET: u64 = 0x00;
pub const MMMU_CTRL_CAP_EXCEEDED: u32 = 1 << 27;
pub const MMMU_CTRL_CAP_EXCEEDED_ABORT_EN: u32 = 1 << 26;
pub const MMMU_CTRL_PT_INVALID: u32 = 1 << 20;
pub const MMMU_CTRL_PT_INVALID_ABORT_EN: u32 = 1 << 19;
pub const MMMU_CTRL_PT_INVALID_EN: u32 = 1 << 16;
pub const MMMU_CTRL_WRITE_VIOLATION: u32 = 1 << 12;
pub const MMMU_CTRL_WRITE_VIOLATION_ABORT_EN: u32 = 1 << 11;
pub const MMMU_CTRL_TLB_CLEARING: u32 = 1 << 7;
pub const MMMU_CTRL_STATS_CLEAR: u32 = 1 << 3;
pub const MMMU_CTRL_TLB_CLEAR: u32 = 1 << 2;
pub const MMMU_CTRL_STATS_ENABLE: u32 = 1 << 1;
pub const MMMU_CTRL_ENABLE: u32 = 1 << 0;
pub const MMMU_PT_PA_BASE_OFFSET: u64 = 0x04;
pub const MMMU_ADDR_CAP_OFFSET: u64 = 0x14;
pub const MMMU_ADDR_CAP_ENABLE: u32 = 1 << 31;
const ADDR_CAP_SHIFT: u32 = 28;
pub const MMMU_SHOOT_DOWN_OFFSET: u64 = 0x18;
pub const MMMU_SHOOT_DOWN_SHOOTING: u32 = 1 << 31;
pub const MMMU_SHOOT_DOWN_SHOOT: u32 = 1 << 30;
pub const MMMU_BYPASS_START_OFFSET: u64 = 0x1C;
pub const MMMU_BYPASS_END_OFFSET: u64 = 0x20;
pub const MMMU_BYPASS_END_ENABLE: u32 = 1 << 31;
pub const MMMU_MISC_OFFSET: u64 = 0x24;
pub const MMMU_MISC_SINGLE_TABLE: u32 = 1 << 31;
pub const MMMU_ILLEGAL_ADR_OFFSET: u64 = 0x30;
pub const MMMU_ILLEGAL_ADR_ENABLE: u32 = 1 << 31;
pub const MMMU_VIO_ADDR_OFFSET: u64 = 0x34;
pub const MMMU_DEBUG_INFO_OFFSET: u64 = 0x38;
pub const MMMU_DEBUG_INFO_VERSION_MASK: u32 = 0x0000_000F;
pub const MMMU_DEBUG_INFO_VA_WIDTH_MASK: u32 = 0x0000_00F0;
pub const MMMU_DEBUG_INFO_PA_WIDTH_MASK: u32 = 0x0000_0F00;
pub const MMMU_DEBUG_INFO_BIGPAGE_WIDTH_MASK: u32 = 0x000F_F000;
pub const MMMU_DEBUG_INFO_SUPERPAGE_WIDTH_MASK: u32 = 0x0FF0_0000;
pub const MMMU_DEBUG_INFO_BYPASS_4M: u32 = 1 << 28;
pub const MMMU_DEBUG_INFO_BYPASS: u32 = 1 << 29;
pub const MMMU_PTE_PAGESIZE_MASK: u32 = 0xC000_0000;
pub const MMMU_PTE_WRITEABLE: u32 = 1 << 29;
pub const MMMU_PTE_VALID: u32 = 1 << 28;

pub const MMUC_CONTROL_ENABLE: u32 = 1;
pub const MMUC_CONTROL_FLUSH: u32 = 2;
pub const MMUC_CONTROL_FLUSHING: u32 = 4;

pub const IOMMU_PAGE_SHIFT: u32 = 12;
pub const IOMMU_PAGE_SIZE: u64 = 1 << IOMMU_PAGE_SHIFT;
/// Linux `PAGE_SHIFT` of this port.
pub const PAGE_SHIFT: u32 = 12;
const LX_PAGEWORDS_SHIFT: u32 = PAGE_SHIFT - 2;
const LX_PAGEWORDS_MASK: u32 = (1 << LX_PAGEWORDS_SHIFT) - 1;
const IOMMU_PAGEWORDS_SHIFT: u32 = IOMMU_PAGE_SHIFT - 2;
const IOMMU_HUGEPAGE_SHIFT: u32 = IOMMU_PAGE_SHIFT + IOMMU_PAGEWORDS_SHIFT;
const L1_AP_BASE_SHIFT: u32 = IOMMU_PAGE_SHIFT + 2 * IOMMU_PAGEWORDS_SHIFT;
const IOVA_TO_LXPAGE_SHIFT: u32 = IOMMU_PAGE_SHIFT + LX_PAGEWORDS_SHIFT;
/// The largest IOVA space: one top-level page of L2 page numbers
/// (`PAGE_SIZE << 20`).
pub const MAX_APERTURE_SIZE: u64 = 1 << (PAGE_SHIFT + 20);

/// Table memory: whole pages the IOMMU reads, never cached by it.
pub trait Pages {
    /// A zeroed page's physical address (`iommu_alloc_pages_sz` +
    /// `dma_map_single` to the device), or None.
    fn alloc(&mut self) -> Option<u64>;
    fn free(&mut self, phys: u64);
    fn read(&self, phys: u64, i: usize) -> u32;
    fn write(&mut self, phys: u64, i: usize, v: u32);
    /// `dma_sync_single_for_cpu` / `_for_device` of the page.
    fn sync_for_cpu(&mut self, phys: u64);
    fn sync_for_device(&mut self, phys: u64);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NoMemory,
    /// Outside the aperture.
    Range,
    /// `DEBUG_INFO` below what the driver supports (version 4, 2-level).
    Unsupported(u32),
}

/// The DT's view (`aperture-base`, `aperture-size`, `dma-iova-offset`).
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub aperture_base: u64,
    pub aperture_size: u64,
    pub dma_iova_offset: u64,
}

impl Config {
    /// Linux's default: the aperture at 40 GiB, 2 GiB, everything below
    /// passed through to SDRAM.
    pub const LINUX_DEFAULT: Config = Config { aperture_base: 40 << 30, aperture_size: 2 << 30, dma_iova_offset: 0 };
    /// `aperture-base = 0`, the largest size: pass-through disabled, so
    /// the device reaches nothing but what is mapped.
    pub const NO_BYPASS: Config = Config { aperture_base: 0, aperture_size: MAX_APERTURE_SIZE, dma_iova_offset: 0 };
}

pub struct Iommu {
    pub aperture_base: u64,
    pub aperture_end: u64,
    dma_iova_offset: u64,
    /// Per Linux page of L2 tables, its physical address; 0 = none.
    tables: alloc::vec::Vec<u64>,
    top_table: u64,
    default_page: u64,
    bigpage_mask: u32,
    superpage_mask: u32,
    nmapped_pages: u32,
    /// The shared TLB cache is present.
    cache: bool,
}

/// `bcm2712_iommu_cache_flush`.
pub fn cache_flush(hw: &mut impl Hw) {
    hw.w32(Block::IommuCache, 0, MMUC_CONTROL_ENABLE | MMUC_CONTROL_FLUSH);
    for _ in 0..1024 {
        if hw.r32(Block::IommuCache, 0) & MMUC_CONTROL_FLUSHING == 0 {
            break;
        }
    }
}

const CTRL_ON: u32 = MMMU_CTRL_CAP_EXCEEDED_ABORT_EN
    | MMMU_CTRL_PT_INVALID_ABORT_EN
    | MMMU_CTRL_PT_INVALID_EN
    | MMMU_CTRL_WRITE_VIOLATION_ABORT_EN
    | MMMU_CTRL_STATS_ENABLE
    | MMMU_CTRL_ENABLE;

impl Iommu {
    /// The probe's aperture arithmetic, then `bcm2712_iommu_init`.
    pub fn init(hw: &mut impl Hw, pages: &mut impl Pages, cfg: Config, cache: bool) -> Result<Iommu, Error> {
        let base = (cfg.aperture_base & !((1u64 << L1_AP_BASE_SHIFT) - 1)) + cfg.dma_iova_offset;
        let size = (cfg.aperture_size & !((1u64 << ADDR_CAP_SHIFT) - 1)).clamp(1 << ADDR_CAP_SHIFT, MAX_APERTURE_SIZE);
        let num_pages = (size >> IOVA_TO_LXPAGE_SHIFT) as usize;
        let mut mmu = Iommu {
            aperture_base: base,
            aperture_end: base + size,
            dma_iova_offset: cfg.dma_iova_offset,
            tables: alloc::vec![0; num_pages],
            top_table: 0,
            default_page: 0,
            bigpage_mask: 0,
            superpage_mask: 0,
            nmapped_pages: 0,
            cache,
        };

        let u = hw.r32(Block::Iommu, MMMU_DEBUG_INFO_OFFSET);
        if field_get(u, MMMU_DEBUG_INFO_VERSION_MASK) < 4
            || field_get(u, MMMU_DEBUG_INFO_VA_WIDTH_MASK) < 6
            || field_get(u, MMMU_DEBUG_INFO_PA_WIDTH_MASK) < 6
            || u & MMMU_DEBUG_INFO_BYPASS == 0
        {
            return Err(Error::Unsupported(u));
        }
        mmu.bigpage_mask = ((1u32 << field_get(u, MMMU_DEBUG_INFO_BIGPAGE_WIDTH_MASK)) - 1) << IOMMU_PAGE_SHIFT;
        mmu.superpage_mask = ((1u32 << field_get(u, MMMU_DEBUG_INFO_SUPERPAGE_WIDTH_MASK)) - 1) << IOMMU_PAGE_SHIFT;

        hw.w32(Block::Iommu, MMMU_CTRL_OFFSET,
            MMMU_CTRL_CAP_EXCEEDED | MMMU_CTRL_PT_INVALID | MMMU_CTRL_WRITE_VIOLATION
                | MMMU_CTRL_STATS_CLEAR | MMMU_CTRL_TLB_CLEAR);

        let misc = hw.r32(Block::Iommu, MMMU_MISC_OFFSET);
        hw.w32(Block::Iommu, MMMU_MISC_OFFSET, misc & !MMMU_MISC_SINGLE_TABLE);
        hw.w32(Block::Iommu, MMMU_ADDR_CAP_OFFSET,
            MMMU_ADDR_CAP_ENABLE + ((mmu.aperture_end - mmu.dma_iova_offset) >> ADDR_CAP_SHIFT) as u32 - 1);
        hw.w32(Block::Iommu, MMMU_BYPASS_START_OFFSET, 0);
        if mmu.aperture_base > mmu.dma_iova_offset {
            let byp_shift = if u & MMMU_DEBUG_INFO_BYPASS_4M != 0 { IOMMU_HUGEPAGE_SHIFT } else { ADDR_CAP_SHIFT };
            hw.w32(Block::Iommu, MMMU_BYPASS_END_OFFSET,
                MMMU_BYPASS_END_ENABLE + ((mmu.aperture_base - mmu.dma_iova_offset) >> byp_shift) as u32);
        } else {
            hw.w32(Block::Iommu, MMMU_BYPASS_END_OFFSET, 0);
        }

        mmu.top_table = pages.alloc().ok_or(Error::NoMemory)?;
        let u = (mmu.top_table >> IOMMU_PAGE_SHIFT) as u32;
        hw.w32(Block::Iommu, MMMU_PT_PA_BASE_OFFSET,
            u.wrapping_sub(((mmu.aperture_base - mmu.dma_iova_offset) >> L1_AP_BASE_SHIFT) as u32));
        let Some(dp) = pages.alloc() else {
            pages.free(mmu.top_table);
            return Err(Error::NoMemory);
        };
        mmu.default_page = dp;
        hw.w32(Block::Iommu, MMMU_ILLEGAL_ADR_OFFSET, MMMU_ILLEGAL_ADR_ENABLE + (dp >> IOMMU_PAGE_SHIFT) as u32);
        mmu.nmapped_pages = 0;

        if mmu.cache {
            cache_flush(hw);
        }
        hw.w32(Block::Iommu, MMMU_CTRL_OFFSET, CTRL_ON);
        Ok(mmu)
    }

    /// `bcm2712_iommu_map` of `bytes` at `iova` to `pa`.
    pub fn map(&mut self, pages: &mut impl Pages, iova: u64, pa: u64, bytes: u64, write: bool) -> Result<(), Error> {
        let mut entry = MMMU_PTE_VALID | (pa >> IOMMU_PAGE_SHIFT) as u32;
        let align = (iova | pa | bytes) as u32;
        if iova < self.aperture_base || iova + bytes > self.aperture_end {
            return Err(Error::Range);
        }
        let iova = iova - self.aperture_base;
        let mut p = (iova >> IOMMU_PAGE_SHIFT) as u32;
        let p_last = ((iova + bytes - 1) >> IOMMU_PAGE_SHIFT) as u32;

        let mut dirty_top = false;
        let mut lxp = p >> LX_PAGEWORDS_SHIFT;
        while lxp <= p_last >> LX_PAGEWORDS_SHIFT {
            if self.tables[lxp as usize] == 0 {
                let Some(t) = pages.alloc() else { break };
                self.tables[lxp as usize] = t;
                let mut u = (t >> IOMMU_PAGE_SHIFT) as u32 | MMMU_PTE_VALID;
                if !dirty_top {
                    pages.sync_for_cpu(self.top_table);
                    dirty_top = true;
                }
                for i in lxp << (PAGE_SHIFT - IOMMU_PAGE_SHIFT)..(lxp + 1) << (PAGE_SHIFT - IOMMU_PAGE_SHIFT) {
                    pages.write(self.top_table, i as usize, u);
                    u += 1;
                }
            }
            lxp += 1;
        }
        if dirty_top {
            pages.sync_for_device(self.top_table);
        }
        if lxp <= p_last >> LX_PAGEWORDS_SHIFT {
            return Err(Error::NoMemory);
        }

        if align & ((1 << IOMMU_HUGEPAGE_SHIFT) - 1) == 0 {
            entry |= 3 << 30;
        } else if self.superpage_mask != 0 && align & self.superpage_mask == 0 {
            entry |= 2 << 30;
        } else if self.bigpage_mask != 0 && align & self.bigpage_mask == 0 {
            entry |= 1 << 30;
        }
        if write {
            entry |= MMMU_PTE_WRITEABLE;
        }

        while p <= p_last {
            let tbl = self.tables[(p >> LX_PAGEWORDS_SHIFT) as usize];
            pages.sync_for_cpu(tbl);
            while p <= p_last {
                let i = (p & LX_PAGEWORDS_MASK) as usize;
                self.nmapped_pages += (pages.read(tbl, i) == 0) as u32;
                pages.write(tbl, i, entry);
                entry = entry.wrapping_add(1);
                p += 1;
                if p % (1 << LX_PAGEWORDS_SHIFT) == 0 {
                    break;
                }
            }
            pages.sync_for_device(tbl);
        }
        Ok(())
    }

    /// `bcm2712_iommu_unmap`; returns the bytes unmapped. The TLB still
    /// holds the old entries until `sync_range` over the same range.
    pub fn unmap(&mut self, pages: &mut impl Pages, iova: u64, bytes: u64) -> u64 {
        if iova < self.aperture_base || iova + bytes > self.aperture_end {
            return 0;
        }
        let iova = iova - self.aperture_base;
        let mut p = (iova >> IOMMU_PAGE_SHIFT) as u32;
        let p_last = ((iova + bytes - 1) >> IOMMU_PAGE_SHIFT) as u32;
        while p <= p_last {
            let tbl = self.tables[(p >> LX_PAGEWORDS_SHIFT) as usize];
            if tbl != 0 {
                pages.sync_for_cpu(tbl);
                while p <= p_last {
                    let i = (p & LX_PAGEWORDS_MASK) as usize;
                    self.nmapped_pages -= (pages.read(tbl, i) != 0) as u32;
                    pages.write(tbl, i, 0);
                    p += 1;
                    if p % (1 << LX_PAGEWORDS_SHIFT) == 0 {
                        break;
                    }
                }
                pages.sync_for_device(tbl);
            } else {
                // Linux does not advance here and would loop for ever; a
                // range whose table was never allocated has nothing to clear.
                p = (p | LX_PAGEWORDS_MASK) + 1;
            }
        }
        bytes
    }

    /// `bcm2712_iommu_sync_range`: the TLB forgets `[iova, iova + size)`.
    pub fn sync_range(&mut self, hw: &mut impl Hw, iova: u64, size: u64) {
        let iova_end = self.aperture_end.min(iova + size);
        let iova = self.aperture_base.max(iova);
        if iova_end <= iova {
            return;
        }
        if self.cache {
            cache_flush(hw);
        }
        if self.nmapped_pages == 0 || iova_end >= iova + (1 << 24) {
            hw.w32(Block::Iommu, MMMU_CTRL_OFFSET, CTRL_ON | MMMU_CTRL_TLB_CLEAR);
            for _ in 0..1024 {
                if hw.r32(Block::Iommu, MMMU_CTRL_OFFSET) & MMMU_CTRL_TLB_CLEARING == 0 {
                    break;
                }
            }
        } else {
            let iova = iova - self.dma_iova_offset;
            let iova_end = iova_end - (self.dma_iova_offset + 1);
            for p4 in (iova >> (IOMMU_PAGE_SHIFT + 2))..=(iova_end >> (IOMMU_PAGE_SHIFT + 2)) {
                hw.w32(Block::Iommu, MMMU_SHOOT_DOWN_OFFSET, MMMU_SHOOT_DOWN_SHOOT + ((p4 as u32) << 2));
                for _ in 0..1024 {
                    if hw.r32(Block::Iommu, MMMU_SHOOT_DOWN_OFFSET) & MMMU_SHOOT_DOWN_SHOOTING == 0 {
                        break;
                    }
                }
            }
        }
    }

    /// `bcm2712_iommu_sync_all`.
    pub fn sync_all(&mut self, hw: &mut impl Hw) {
        let (base, end) = (self.aperture_base, self.aperture_end);
        self.sync_range(hw, base, end - base);
    }

    /// `bcm2712_iommu_iova_to_phys`; 0 for an address not mapped.
    pub fn iova_to_phys(&self, pages: &impl Pages, iova: u64) -> u64 {
        if iova < self.aperture_base || iova >= self.aperture_end {
            return 0;
        }
        let p = ((iova - self.aperture_base) >> IOMMU_PAGE_SHIFT) as u32;
        let tbl = self.tables[(p >> LX_PAGEWORDS_SHIFT) as usize];
        if tbl == 0 {
            return 0;
        }
        let e = pages.read(tbl, (p & LX_PAGEWORDS_MASK) as usize) & 0x0FFF_FFFF;
        ((e as u64) << IOMMU_PAGE_SHIFT) + (iova & (IOMMU_PAGE_SIZE - 1))
    }

    /// Pages mapped now.
    pub fn mapped_pages(&self) -> u32 {
        self.nmapped_pages
    }
}
