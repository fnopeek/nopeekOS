//! The BCM2712 IOMMU in front of a display device-tree device (the HVS's
//! iommu4), behind `drivers::iommu`: `pi5_core::iommu` (Linux
//! bcm2712-iommu.c) on the node's registers, with the contract VT-d and
//! AMD-Vi keep. Configured as Linux with `aperture-base = 0`: no
//! pass-through, so the device reaches only what is granted.
//!
//! A page is granted at its own physical address as the IOVA: the
//! aperture covers 0..4 GiB, so pages above it cannot be granted. Grants
//! are counted per page and mapped with 4 KiB entries, so revoking one
//! page never leaves a larger entry covering it.

use alloc::collections::BTreeMap;
use pi5_core::hw::{Block, Hw};
use pi5_core::iommu::{self as core_iommu, Config, Iommu, Pages, IOMMU_PAGE_SIZE};
use spin::Mutex;

use crate::hw::{DmaRegion, Mmio};
use crate::kprintln;
use crate::platform::dt::{Fdt, Node};

const PAGE: u64 = IOMMU_PAGE_SIZE;

struct Regs {
    mmu: Mmio,
    cache: Option<Mmio>,
}

impl Hw for Regs {
    fn read(&mut self, b: Block, off: u64, _: u8) -> u32 {
        match (b, &self.cache) {
            (Block::IommuCache, Some(c)) => c.r32(off),
            (Block::IommuCache, None) => 0,
            _ => self.mmu.r32(off),
        }
    }
    fn write(&mut self, b: Block, off: u64, _: u8, v: u32) {
        match (b, &self.cache) {
            (Block::IommuCache, Some(c)) => c.w32(off, v),
            (Block::IommuCache, None) => {}
            _ => self.mmu.w32(off, v),
        }
    }
    fn delay_us(&mut self, us: u64) { crate::time::delay_us(us) }
}

/// Table pages from the uncached DMA zone: the IOMMU reads them without
/// the CPU's caches, so handing a page over needs no cache maintenance.
struct Tables(BTreeMap<u64, DmaRegion>);

impl Pages for Tables {
    fn alloc(&mut self) -> Option<u64> {
        let r = DmaRegion::alloc_zeroed(1)?;
        let p = r.phys();
        self.0.insert(p, r);
        Some(p)
    }
    fn free(&mut self, phys: u64) {
        if let Some(r) = self.0.remove(&phys) {
            r.free();
        }
    }
    fn read(&self, phys: u64, i: usize) -> u32 { self.0[&phys].r32(i * 4) }
    fn write(&mut self, phys: u64, i: usize, v: u32) {
        if let Some(r) = self.0.get(&phys) {
            r.w32(i * 4, v);
        }
    }
    fn sync_for_cpu(&mut self, _: u64) {}
    fn sync_for_device(&mut self, _: u64) {}
}

struct Domain {
    /// The device: the base of its first `reg` window.
    dev: u64,
    regs: Regs,
    tables: Tables,
    mmu: Iommu,
    /// Grants per page; a page is mapped while its count is above 0.
    granted: BTreeMap<u64, u32>,
}

static STATE: Mutex<Option<Domain>> = Mutex::new(None);

/// The device-tree node whose first `reg` window starts at `base`.
fn node_at(fdt: &Fdt, base: u64) -> Option<Node> {
    let mut found = None;
    fdt.walk(|n| {
        if found.is_none() && fdt.is_enabled(n) && fdt.reg0(n).is_some_and(|(a, _)| a == base) {
            found = Some(*n);
        }
        found.is_none()
    });
    found
}

fn map_node(fdt: &Fdt, n: &Node) -> Option<Mmio> {
    let (base, len) = fdt.reg0(n)?;
    // SAFETY: the register window of an IOMMU node, kernel-owned.
    unsafe { Mmio::map(base, len) }.ok()
}

/// Put device `dev` behind the IOMMU its node names (`iommus`). It keeps
/// reaching the firmware's framebuffer, which the display may be scanning
/// out (as VT-d keeps a device's reserved ranges).
pub fn protect(dev: u64) -> Result<(), &'static str> {
    let mut st = STATE.lock();
    if st.as_ref().is_some_and(|d| d.dev == dev) {
        return Ok(());
    }
    if st.is_some() {
        return Err("another device is protected already");
    }
    let fdt = crate::platform::dt::boot().ok_or("no device tree")?;
    let node = node_at(&fdt, dev).ok_or("no device-tree node at the device's address")?;
    let mmu_node = fdt.prop_cell(&node, b"iommus", 0).and_then(|ph| fdt.find_phandle(ph))
        .ok_or("the device names no IOMMU")?;
    if !fdt.prop_has_str(&mmu_node, b"compatible", b"brcm,bcm2712-iommu") {
        return Err("the device's IOMMU is not a bcm2712-iommu");
    }
    let mmu_regs = map_node(&fdt, &mmu_node).ok_or("the IOMMU's registers are not mappable")?;
    let cache = fdt.find_compatible(b"brcm,bcm2712-iommuc").filter(|n| fdt.is_enabled(n)).and_then(|n| map_node(&fdt, &n));
    let mut regs = Regs { mmu: mmu_regs, cache };
    let mut tables = Tables(BTreeMap::new());
    let has_cache = regs.cache.is_some();
    let mmu = Iommu::init(&mut regs, &mut tables, Config::NO_BYPASS, has_cache).map_err(|e| match e {
        core_iommu::Error::NoMemory => "no memory for the IOMMU's tables",
        core_iommu::Error::Unsupported(_) => "IOMMU version not supported",
        core_iommu::Error::Range => "aperture",
    })?;
    let mut d = Domain { dev, regs, tables, mmu, granted: BTreeMap::new() };
    if let Some(Ok(fb)) = crate::platform::simplefb::find(&fdt) {
        d.grant(fb.addr, fb.size.div_ceil(PAGE) as usize);
    }
    kprintln!("[npk] iommu: device @{:x} behind bcm2712-iommu, IOVA 0..{} GiB, no pass-through",
        dev, d.mmu.aperture_end >> 30);
    *st = Some(d);
    Ok(())
}

impl Domain {
    fn grant(&mut self, phys: u64, pages: usize) {
        if self.grant_pages(phys, pages) {
            let first = phys & !(PAGE - 1);
            // `iotlb_sync_map`: the TLB may hold the invalid entries.
            self.mmu.sync_range(&mut self.regs, first, pages as u64 * PAGE);
        }
    }

    /// Count one grant of each page, mapping those that had none; true when
    /// one was mapped.
    fn grant_pages(&mut self, phys: u64, pages: usize) -> bool {
        let first = phys & !(PAGE - 1);
        let mut fresh = false;
        for i in 0..pages as u64 {
            let p = first + i * PAGE;
            if p + PAGE > self.mmu.aperture_end {
                kprintln!("[npk] iommu: {:#x} above the IOVA aperture, stays unreachable", p);
                continue;
            }
            let n = self.granted.entry(p).or_insert(0);
            *n += 1;
            if *n == 1 {
                if self.mmu.map(&mut self.tables, p, p, PAGE, true).is_err() {
                    kprintln!("[npk] iommu: no memory for the page table, {:#x} stays unreachable", p);
                    self.granted.remove(&p);
                    continue;
                }
                fresh = true;
            }
        }
        fresh
    }

    fn revoke(&mut self, phys: u64, pages: usize) {
        if self.revoke_pages(phys, pages) {
            let first = phys & !(PAGE - 1);
            // `iotlb_sync`: when this returns, the device reaches none of them.
            self.mmu.sync_range(&mut self.regs, first, pages as u64 * PAGE);
        }
    }

    /// Take one grant of each page, unmapping those whose last it was; true
    /// when one was unmapped.
    fn revoke_pages(&mut self, phys: u64, pages: usize) -> bool {
        let first = phys & !(PAGE - 1);
        let mut gone = false;
        for i in 0..pages as u64 {
            let p = first + i * PAGE;
            let Some(n) = self.granted.get_mut(&p) else { continue };
            *n -= 1;
            if *n == 0 {
                self.granted.remove(&p);
                self.mmu.unmap(&mut self.tables, p, PAGE);
                gone = true;
            }
        }
        gone
    }

    /// One TLB sync over the span of `list`'s pages.
    fn sync_list(&mut self, list: &[u64]) {
        let lo = list.iter().map(|p| p & !(PAGE - 1)).min().unwrap_or(0);
        let hi = list.iter().map(|p| (p & !(PAGE - 1)) + PAGE).max().unwrap_or(0);
        if hi > lo {
            self.mmu.sync_range(&mut self.regs, lo, hi - lo);
        }
    }
}

pub fn protected(dev: u64) -> bool {
    STATE.lock().as_ref().is_some_and(|d| d.dev == dev)
}

pub fn grant(dev: u64, phys: u64, pages: usize) {
    if let Some(d) = STATE.lock().as_mut().filter(|d| d.dev == dev) {
        d.grant(phys, pages);
    }
}

pub fn grant_list(dev: u64, list: &[u64]) {
    if let Some(d) = STATE.lock().as_mut().filter(|d| d.dev == dev) {
        let mut fresh = false;
        for &p in list {
            fresh |= d.grant_pages(p, 1);
        }
        if fresh {
            d.sync_list(list);
        }
    }
}

/// When it returns, the device no longer reaches the pages whose last
/// grant this was.
pub fn revoke(dev: u64, phys: u64, pages: usize) {
    if let Some(d) = STATE.lock().as_mut().filter(|d| d.dev == dev) {
        d.revoke(phys, pages);
    }
}

pub fn revoke_list(dev: u64, list: &[u64]) {
    if let Some(d) = STATE.lock().as_mut().filter(|d| d.dev == dev) {
        let mut gone = false;
        for &p in list {
            gone |= d.revoke_pages(p, 1);
        }
        if gone {
            d.sync_list(list);
        }
    }
}

/// Pages the protected device reaches now.
pub fn granted_pages() -> Option<(u64, usize)> {
    STATE.lock().as_ref().map(|d| (d.dev, d.granted.len()))
}

/// What `gpu iommu` shows: the control word with its sticky fault bits,
/// the address of the last violation and the TLB statistics.
pub struct Stats {
    pub ctrl: u32,
    pub vio_addr: u32,
    pub hit: u32,
    pub miss: u32,
    pub stall: u32,
}

pub fn stats() -> Option<Stats> {
    let mut st = STATE.lock();
    let d = st.as_mut()?;
    let r = |d: &mut Domain, off| d.regs.read(Block::Iommu, off, 4);
    Some(Stats {
        ctrl: r(d, core_iommu::MMMU_CTRL_OFFSET),
        vio_addr: r(d, core_iommu::MMMU_VIO_ADDR_OFFSET),
        hit: r(d, 0x08),
        miss: r(d, 0x0C),
        stall: r(d, 0x10),
    })
}
