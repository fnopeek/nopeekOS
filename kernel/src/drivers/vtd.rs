//! Intel VT-d DMA remapping for one device (docs/plan/GPU_STACK.md stage 2,
//! docs/plan/GPU_IN_VM.md).
//!
//! A driver module programs its device with physical addresses. Without an
//! IOMMU nothing stops the device from reaching any page; the WASM sandbox
//! protects the kernel from the module's code, not from what the module
//! lets the card do. Here the device's remapping unit translates its
//! requests through a second-level table that maps exactly the pages the
//! kernel granted, one to one (IOVA = physical address), so the module
//! needs no change and every other address faults.
//!
//! Register sequences follow Linux `drivers/iommu/intel/` (iommu.c, dmar.c)
//! in legacy mode with register-based invalidation: `iommu_set_root_entry`,
//! `__iommu_flush_context`, `__iommu_flush_iotlb`,
//! `iommu_flush_write_buffer`, `iommu_disable_protect_mem_regions`,
//! `iommu_enable_translation`, `dmar_fault`. Differences: faults are polled,
//! not signalled; a unit is used only when its scope is this one device, so
//! enabling it touches no other device; translation is never disabled again
//! (Linux `quirk_igfx_skip_te_disable` for Gen12 graphics).
//!
//! Granted: the RMRR ranges of the device and, for a display controller,
//! its stolen memory (Linux maps the direct regions of a graphics device the
//! same way); the module's DMA buffers; the buffers gpudev registers.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

use crate::drivers::pci::{self, PciAddr};
use crate::hw::{DmaRegion, Mmio};

const DMAR_CAP_REG: u32 = 0x08;
const DMAR_ECAP_REG: u32 = 0x10;
const DMAR_GCMD_REG: u32 = 0x18;
const DMAR_GSTS_REG: u32 = 0x1c;
const DMAR_RTADDR_REG: u32 = 0x20;
const DMAR_CCMD_REG: u32 = 0x28;
const DMAR_FSTS_REG: u32 = 0x34;
const DMAR_PMEN_REG: u32 = 0x64;

const DMA_GCMD_TE: u32 = 1 << 31;
const DMA_GCMD_SRTP: u32 = 1 << 30;
const DMA_GCMD_WBF: u32 = 1 << 27;
const DMA_GCMD_IRE: u32 = 1 << 25;
const DMA_GSTS_TES: u32 = 1 << 31;
const DMA_GSTS_RTPS: u32 = 1 << 30;
const DMA_GSTS_WBFS: u32 = 1 << 27;
const DMA_GSTS_QIES: u32 = 1 << 26;
const DMA_GSTS_IRES: u32 = 1 << 25;

const DMA_CCMD_ICC: u64 = 1 << 63;
const DMA_CCMD_GLOBAL_INVL: u64 = 1 << 61;
const DMA_TLB_IVT: u64 = 1 << 63;
const DMA_TLB_GLOBAL_FLUSH: u64 = 1 << 60;
const DMA_TLB_DSI_FLUSH: u64 = 2 << 60;
const DMA_TLB_WRITE_DRAIN: u64 = 1 << 48;

const DMA_PMEN_EPM: u32 = 1 << 31;
const DMA_PMEN_PRS: u32 = 1 << 0;

const DMA_FSTS_PFO: u32 = 1 << 0;
const DMA_FSTS_PPF: u32 = 1 << 1;
const DMA_FSTS_PRO: u32 = 1 << 7;
const DMA_FRCD_F: u32 = 1 << 31;

const DMA_PTE_READ: u64 = 1 << 0;
const DMA_PTE_WRITE: u64 = 1 << 1;
const VTD_PAGE_MASK: u64 = !0xfff;
const PAGE: u64 = 4096;

/// The one domain id a unit uses: it serves one device.
const DID: u16 = 1;
/// How long a register operation may take before the unit counts as broken
/// (Linux `DMAR_OPERATION_TIMEOUT`).
const OPERATION_TIMEOUT_MS: u64 = 1000;
/// Faults kept for `gpu iommu`.
const FAULTS_KEPT: usize = 16;

fn cap_sagaw(c: u64) -> u64 { (c >> 8) & 0x1f }
fn cap_rwbf(c: u64) -> bool { (c >> 4) & 1 != 0 }
fn cap_plmr(c: u64) -> bool { (c >> 5) & 1 != 0 }
fn cap_phmr(c: u64) -> bool { (c >> 6) & 1 != 0 }
fn cap_caching_mode(c: u64) -> bool { (c >> 7) & 1 != 0 }
fn cap_fault_reg_offset(c: u64) -> u32 { (((c >> 24) & 0x3ff) * 16) as u32 }
fn cap_num_fault_regs(c: u64) -> u32 { (((c >> 40) & 0xff) + 1) as u32 }
fn cap_write_drain(c: u64) -> bool { (c >> 54) & 1 != 0 }
fn cap_esrtps(c: u64) -> bool { (c >> 63) & 1 != 0 }
fn ecap_coherent(e: u64) -> bool { e & 1 != 0 }
fn ecap_iotlb_offset(e: u64) -> u32 { (((e >> 8) & 0x3ff) * 16) as u32 }

/// A remapping unit (DRHD) and the devices in its scope.
#[derive(Clone)]
struct Unit {
    base: u64,
    /// Register window size in bytes.
    size: u64,
    include_all: bool,
    devices: Vec<PciAddr>,
}

/// A reserved memory region (RMRR) the firmware lets devices reach.
#[derive(Clone)]
struct Rmrr {
    base: u64,
    limit: u64,
    devices: Vec<PciAddr>,
}

/// A recorded DMA remapping fault.
#[derive(Clone, Copy)]
pub struct Fault {
    pub addr: u64,
    pub source: u16,
    pub reason: u8,
    pub read: bool,
}

/// The protected device's unit and its second-level table.
struct Domain {
    dev: PciAddr,
    regs: Mmio,
    cap: u64,
    ecap: u64,
    gcmd: u32,
    /// Second-level paging levels: 3 (39-bit) or 4 (48-bit).
    levels: u32,
    root: DmaRegion,
    table: DmaRegion,
    /// Grants per page; a page is mapped while its count is above 0.
    granted: BTreeMap<u64, u32>,
    faults: Vec<Fault>,
    faults_total: u64,
}

struct State {
    parsed: bool,
    units: Vec<Unit>,
    rmrrs: Vec<Rmrr>,
    domain: Option<Domain>,
}

static STATE: Mutex<State> = Mutex::new(State { parsed: false, units: Vec::new(), rmrrs: Vec::new(), domain: None });

/// Write back the cache lines of `len` bytes at `addr`: the unit reads its
/// tables without snooping unless ECAP.C says otherwise (Linux
/// `clflush_cache_range` in `__iommu_flush_cache`).
fn flush_cache(addr: u64, len: u64) {
    let mut line = addr & !63;
    while line < addr + len {
        // SAFETY: `line` lies in an identity-mapped table page the kernel
        // owns; clflush only writes the line back.
        unsafe { core::arch::asm!("clflush [{}]", in(reg) line, options(nostack, preserves_flags)) };
        line += 64;
    }
    // SAFETY: a fence, no memory operands.
    unsafe { core::arch::asm!("mfence", options(nostack, preserves_flags)) };
}

fn deadline() -> u64 {
    crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 1000 * OPERATION_TIMEOUT_MS
}

/// Wait until `done` holds on the register at `off` (Linux `IOMMU_WAIT_OP`).
fn wait32(regs: &Mmio, off: u32, done: impl Fn(u32) -> bool) -> Result<(), &'static str> {
    let dl = deadline();
    while !done(regs.r32(off)) {
        if crate::interrupts::rdtsc() > dl {
            return Err("remapping unit did not finish an operation");
        }
        core::hint::spin_loop();
    }
    Ok(())
}

fn wait64(regs: &Mmio, off: u32, done: impl Fn(u64) -> bool) -> Result<u64, &'static str> {
    let dl = deadline();
    loop {
        let v = regs.r64_lo_hi(off);
        if done(v) {
            return Ok(v);
        }
        if crate::interrupts::rdtsc() > dl {
            return Err("remapping unit did not finish an invalidation");
        }
        core::hint::spin_loop();
    }
}

/// The device at the end of a DMAR device-scope path: start bus, then one
/// (device, function) per hop, each hop but the last a bridge.
fn scope_device(start_bus: u8, path: &[(u8, u8)]) -> Option<PciAddr> {
    let mut bus = start_bus;
    for (i, &(d, f)) in path.iter().enumerate() {
        let a = PciAddr { bus, device: d, function: f };
        if i + 1 == path.len() {
            return Some(a);
        }
        bus = pci::read8(a, 0x19);
    }
    None
}

/// Parse the DMAR table once: DRHD units and RMRR regions with their scopes.
fn parse(st: &mut State) {
    if st.parsed {
        return;
    }
    st.parsed = true;
    let Some(dmar) = crate::drivers::acpi::find_table(b"DMAR") else { return };
    crate::drivers::acpi::ensure_mapped_pub(dmar, 4096);
    // SAFETY: the DMAR header lies in the page mapped above.
    let len = unsafe { crate::hw::PhysView::new(dmar as u64, 4096) }.u32(4u32).unwrap_or(0) as usize;
    crate::drivers::acpi::ensure_mapped_pub(dmar, len.max(48));
    // SAFETY: the whole table is mapped now; firmware tables do not change.
    let t = unsafe { crate::hw::PhysView::new(dmar as u64, len as u64) };
    let scopes = |from: usize, to: usize| -> Vec<PciAddr> {
        let mut out = Vec::new();
        let mut o = from;
        while o + 6 <= to {
            let (ty, slen) = (t.u8(o).unwrap_or(0), t.u8(o + 1).unwrap_or(0) as usize);
            if slen < 6 {
                break;
            }
            // Type 1: PCI endpoint, type 2: bridge with its subtree.
            if ty == 1 || ty == 2 {
                let bus = t.u8(o + 5).unwrap_or(0);
                let path: Vec<(u8, u8)> = (o + 6..o + slen).step_by(2)
                    .map(|p| (t.u8(p).unwrap_or(0), t.u8(p + 1).unwrap_or(0))).collect();
                if let Some(a) = scope_device(bus, &path) {
                    out.push(a);
                }
            }
            o += slen;
        }
        out
    };
    let mut off = 48usize;
    while off + 4 <= len {
        let (ty, slen) = (t.u16(off).unwrap_or(0), t.u16(off + 2).unwrap_or(0) as usize);
        if slen < 4 || off + slen > len {
            break;
        }
        match ty {
            // DRHD: flags @4, size @5 (2^n pages), segment @6, base @8.
            0 if slen >= 16 => st.units.push(Unit {
                base: t.u64(off + 8).unwrap_or(0),
                size: PAGE << (t.u8(off + 5).unwrap_or(0) & 0xf),
                include_all: t.u8(off + 4).unwrap_or(0) & 1 != 0,
                devices: scopes(off + 16, off + slen),
            }),
            // RMRR: segment @6, base @8, limit @16 (inclusive).
            1 if slen >= 24 => st.rmrrs.push(Rmrr {
                base: t.u64(off + 8).unwrap_or(0),
                limit: t.u64(off + 16).unwrap_or(0),
                devices: scopes(off + 24, off + slen),
            }),
            _ => {}
        }
        off += slen;
    }
}

impl Domain {
    /// The table entry for `iova` at `level` (1 = leaf), creating the
    /// tables above it.
    fn entry(&mut self, iova: u64, level: u32) -> Option<u64> {
        let coherent = ecap_coherent(self.ecap);
        let mut table = self.table.phys();
        let mut l = self.levels;
        loop {
            let idx = (iova >> (12 + 9 * (l - 1))) & 0x1ff;
            let at = table + idx * 8;
            if l == level {
                return Some(at);
            }
            // SAFETY: `at` is an entry of an identity-mapped table page this
            // domain allocated.
            let e = unsafe { core::ptr::read_volatile(at as *const u64) };
            table = if e & (DMA_PTE_READ | DMA_PTE_WRITE) != 0 {
                e & VTD_PAGE_MASK & ((1 << 52) - 1)
            } else {
                let next = DmaRegion::alloc_zeroed(1)?;
                if !coherent {
                    flush_cache(next.phys(), PAGE);
                }
                // SAFETY: as above.
                unsafe { core::ptr::write_volatile(at as *mut u64, next.phys() | DMA_PTE_READ | DMA_PTE_WRITE) };
                if !coherent {
                    flush_cache(at, 8);
                }
                next.phys()
            };
            l -= 1;
        }
    }

    /// Point the leaf for `page` at itself, or clear it.
    fn set_leaf(&mut self, page: u64, present: bool) -> bool {
        let Some(at) = self.entry(page, 1) else { return false };
        let v = if present { page | DMA_PTE_READ | DMA_PTE_WRITE } else { 0 };
        // SAFETY: `at` is a leaf entry of an identity-mapped table page.
        unsafe { core::ptr::write_volatile(at as *mut u64, v) };
        if !ecap_coherent(self.ecap) {
            flush_cache(at, 8);
        }
        true
    }

    fn flush_write_buffer(&mut self) -> Result<(), &'static str> {
        if !cap_rwbf(self.cap) {
            return Ok(());
        }
        self.regs.w32(DMAR_GCMD_REG, self.gcmd | DMA_GCMD_WBF);
        wait32(&self.regs, DMAR_GSTS_REG, |s| s & DMA_GSTS_WBFS == 0)
    }

    fn flush_context_global(&mut self) -> Result<(), &'static str> {
        self.regs.w64_lo_hi(DMAR_CCMD_REG, DMA_CCMD_GLOBAL_INVL | DMA_CCMD_ICC);
        wait64(&self.regs, DMAR_CCMD_REG, |v| v & DMA_CCMD_ICC == 0).map(|_| ())
    }

    fn flush_iotlb(&mut self, ty: u64) -> Result<(), &'static str> {
        let off = ecap_iotlb_offset(self.ecap);
        let mut val = ty | DMA_TLB_IVT;
        if ty == DMA_TLB_DSI_FLUSH {
            val |= (DID as u64) << 32;
        }
        if cap_write_drain(self.cap) {
            val |= DMA_TLB_WRITE_DRAIN;
        }
        self.regs.w64_lo_hi(off + 8, val);
        let v = wait64(&self.regs, off + 8, |v| v & DMA_TLB_IVT == 0)?;
        if (v >> 57) & 3 == 0 {
            return Err("IOTLB invalidation failed");
        }
        Ok(())
    }

    /// Collect pending fault records (Linux `dmar_fault`).
    fn poll_faults(&mut self) {
        let fsts = self.regs.r32(DMAR_FSTS_REG);
        if fsts & DMA_FSTS_PPF != 0 {
            let reg = cap_fault_reg_offset(self.cap);
            let n = cap_num_fault_regs(self.cap);
            let mut idx = (fsts >> 8) & 0xff;
            for _ in 0..n {
                let at = reg + idx * 16;
                let data = self.regs.r32(at + 12);
                if data & DMA_FRCD_F == 0 {
                    break;
                }
                let f = Fault {
                    addr: self.regs.r64_lo_hi(at) & VTD_PAGE_MASK,
                    source: (self.regs.r32(at + 8) & 0xffff) as u16,
                    reason: (data & 0xff) as u8,
                    read: (data >> 30) & 1 != 0,
                };
                self.regs.w32(at + 12, DMA_FRCD_F);
                self.faults_total += 1;
                if self.faults_total <= FAULTS_KEPT as u64 {
                    crate::kprintln!("[npk] iommu: {} fault at {:#x} from {:02x}:{:02x}.{}, reason {:#x}",
                        if f.read { "read" } else { "write" }, f.addr, f.source >> 8, (f.source >> 3) & 0x1f,
                        f.source & 7, f.reason);
                }
                if self.faults.len() >= FAULTS_KEPT {
                    self.faults.remove(0);
                }
                self.faults.push(f);
                idx = (idx + 1) % n;
            }
        }
        if fsts & (DMA_FSTS_PFO | DMA_FSTS_PPF | DMA_FSTS_PRO) != 0 {
            self.regs.w32(DMAR_FSTS_REG, DMA_FSTS_PFO | DMA_FSTS_PPF | DMA_FSTS_PRO);
        }
    }
}

/// The stolen memory of an Intel display controller from Gen11 on: the
/// GTT stolen memory directly below the data stolen memory (BDSM at
/// 0xC0/0xC4, sizes in GGC at 0x50; Linux `intel_early_quirks`).
fn intel_stolen(dev: PciAddr) -> Option<(u64, u64)> {
    if pci::read16(dev, 0x00) != 0x8086 || pci::read8(dev, 0x0B) != 0x03 {
        return None;
    }
    let bdsm = ((pci::read32(dev, 0xC4) as u64) << 32 | pci::read32(dev, 0xC0) as u64) & !0xfffff;
    let ggc = pci::read16(dev, 0x50);
    let gms = (ggc >> 8) as u64;
    let dsm = if gms < 0xf0 { gms * 32 << 20 } else { (gms - 0xf0 + 1) * 4 << 20 };
    let ggms = ((ggc >> 6) & 3) as u64;
    let gsm = if ggms != 0 { 1u64 << (20 + ggms) } else { 0 };
    if bdsm == 0 || dsm == 0 || bdsm < gsm {
        return None;
    }
    Some((bdsm - gsm, bdsm + dsm))
}

/// Put `dev` behind its remapping unit. Only a unit whose scope is this
/// device alone is used. Afterwards the device reaches its RMRR ranges, its
/// stolen memory and what `grant` adds. Translation stays on until reboot.
pub fn protect(dev: PciAddr) -> Result<(), &'static str> {
    let mut st = STATE.lock();
    if st.domain.as_ref().is_some_and(|d| d.dev == dev) {
        return Ok(());
    }
    if st.domain.is_some() {
        return Err("another device is protected already");
    }
    parse(&mut st);
    let unit = st.units.iter().find(|u| u.devices.contains(&dev)).cloned()
        .ok_or("no remapping unit names this device")?;
    if unit.include_all || unit.devices.iter().any(|d| *d != dev) {
        return Err("the device shares its remapping unit with other devices");
    }
    let mut ranges: Vec<(u64, u64)> = st.rmrrs.iter().filter(|r| r.devices.contains(&dev))
        .map(|r| (r.base & VTD_PAGE_MASK, (r.limit | 0xfff) + 1)).collect();
    if let Some(r) = intel_stolen(dev) {
        ranges.push(r);
    }

    let page = unit.base & VTD_PAGE_MASK;
    // SAFETY: the DRHD register window is remapping-hardware MMIO, owned by
    // this driver from here on.
    let regs = unsafe { Mmio::map(page, unit.size.max(PAGE)) }.map_err(|_| "cannot map the unit's registers")?;
    let cap = regs.r64_lo_hi(DMAR_CAP_REG);
    let ecap = regs.r64_lo_hi(DMAR_ECAP_REG);
    let gsts = regs.r32(DMAR_GSTS_REG);
    if gsts & DMA_GSTS_QIES != 0 {
        return Err("queued invalidation is on; register invalidation is not allowed");
    }
    let levels = if cap_sagaw(cap) & 0b100 != 0 {
        4
    } else if cap_sagaw(cap) & 0b10 != 0 {
        3
    } else {
        return Err("the unit supports neither 39- nor 48-bit tables");
    };
    let mut gcmd = 0;
    if gsts & DMA_GSTS_IRES != 0 {
        gcmd |= DMA_GCMD_IRE;
    }
    if gsts & DMA_GSTS_TES != 0 {
        gcmd |= DMA_GCMD_TE;
    }
    let coherent = ecap_coherent(ecap);

    // Root table → context table for the device's bus → context entry.
    let root = DmaRegion::alloc_zeroed(1).ok_or("no memory for the root table")?;
    let ctx = DmaRegion::alloc_zeroed(1).ok_or("no memory for a context table")?;
    let table = DmaRegion::alloc_zeroed(1).ok_or("no memory for the page table")?;
    root.w64(dev.bus as u64 * 16, ctx.phys() | 1);
    let devfn = (dev.device as u64) << 3 | dev.function as u64;
    // Context entry (Linux `context_set_*`, legacy mode): hi first, present
    // last. Translation type 0 (multi-level), fault processing enabled.
    ctx.w64(devfn * 16 + 8, (levels as u64 - 2) | (DID as u64) << 8);
    ctx.w64(devfn * 16, (table.phys() & VTD_PAGE_MASK) | 1);
    if !coherent {
        flush_cache(root.phys(), PAGE);
        flush_cache(ctx.phys(), PAGE);
    }
    let mut d = Domain {
        dev, regs, cap, ecap, gcmd, levels, root, table,
        granted: BTreeMap::new(), faults: Vec::new(), faults_total: 0,
    };
    let mut pages = 0u64;
    for (start, end) in ranges {
        let mut p = start;
        while p < end {
            *d.granted.entry(p).or_insert(0) += 1;
            if !d.set_leaf(p, true) {
                return Err("no memory for the page table");
            }
            p += PAGE;
            pages += 1;
        }
    }

    // Protected memory regions off (Linux
    // `iommu_disable_protect_mem_regions`).
    if cap_plmr(cap) || cap_phmr(cap) {
        let pmen = d.regs.r32(DMAR_PMEN_REG);
        d.regs.w32(DMAR_PMEN_REG, pmen & !DMA_PMEN_EPM);
        wait32(&d.regs, DMAR_PMEN_REG, |v| v & DMA_PMEN_PRS == 0)?;
    }
    // Stale records from before.
    d.poll_faults();
    d.faults.clear();
    // Linux `iommu_set_root_entry`.
    d.regs.w64_lo_hi(DMAR_RTADDR_REG, d.root.phys());
    d.regs.w32(DMAR_GCMD_REG, d.gcmd | DMA_GCMD_SRTP);
    wait32(&d.regs, DMAR_GSTS_REG, |s| s & DMA_GSTS_RTPS != 0)?;
    if !cap_esrtps(cap) {
        d.flush_context_global()?;
        d.flush_iotlb(DMA_TLB_GLOBAL_FLUSH)?;
    }
    d.flush_write_buffer()?;
    // Linux `iommu_enable_translation`.
    d.gcmd |= DMA_GCMD_TE;
    d.regs.w32(DMAR_GCMD_REG, d.gcmd);
    wait32(&d.regs, DMAR_GSTS_REG, |s| s & DMA_GSTS_TES != 0)?;
    crate::kprintln!("[npk] iommu: {:02x}:{:02x}.{} behind VT-d @ {:#x} ({}-level, {}coherent), {} pages direct-mapped",
        dev.bus, dev.device, dev.function, unit.base, levels, if coherent { "" } else { "not " }, pages);
    st.domain = Some(d);
    Ok(())
}

pub fn protected(dev: PciAddr) -> bool {
    STATE.lock().domain.as_ref().is_some_and(|d| d.dev == dev)
}

/// Let `dev` reach `pages` pages from `phys`. No-op for an unprotected device.
pub fn grant(dev: PciAddr, phys: u64, pages: usize) {
    let mut st = STATE.lock();
    let Some(d) = st.domain.as_mut().filter(|d| d.dev == dev) else { return };
    let mut fresh = false;
    for i in 0..pages as u64 {
        let p = (phys & VTD_PAGE_MASK) + i * PAGE;
        let n = d.granted.entry(p).or_insert(0);
        *n += 1;
        if *n == 1 {
            fresh = true;
            if !d.set_leaf(p, true) {
                crate::kprintln!("[npk] iommu: no memory for the page table, {:#x} stays unreachable", p);
            }
        }
    }
    // Non-present to present needs no invalidation unless the unit caches
    // non-present entries (caching mode).
    if fresh {
        let r = if cap_caching_mode(d.cap) { d.flush_iotlb(DMA_TLB_DSI_FLUSH) } else { d.flush_write_buffer() };
        if let Err(e) = r {
            crate::kprintln!("[npk] iommu: {}", e);
        }
    }
}

/// Take back one grant of `pages` pages from `phys`. When it returns, the
/// device no longer reaches the pages whose last grant this was.
pub fn revoke(dev: PciAddr, phys: u64, pages: usize) {
    let mut st = STATE.lock();
    let Some(d) = st.domain.as_mut().filter(|d| d.dev == dev) else { return };
    let mut gone = false;
    for i in 0..pages as u64 {
        let p = (phys & VTD_PAGE_MASK) + i * PAGE;
        let Some(n) = d.granted.get_mut(&p) else { continue };
        *n -= 1;
        if *n == 0 {
            d.granted.remove(&p);
            d.set_leaf(p, false);
            gone = true;
        }
    }
    if gone {
        if let Err(e) = d.flush_iotlb(DMA_TLB_DSI_FLUSH) {
            crate::kprintln!("[npk] iommu: {}", e);
        }
    }
}

/// Look for new fault records of `dev` (logged as they come).
pub fn poll(dev: PciAddr) {
    let mut st = STATE.lock();
    if let Some(d) = st.domain.as_mut().filter(|d| d.dev == dev) {
        d.poll_faults();
    }
}

/// New fault records of the protected device since the last call.
pub fn take_faults(dev: PciAddr) -> Vec<Fault> {
    let mut st = STATE.lock();
    let Some(d) = st.domain.as_mut().filter(|d| d.dev == dev) else { return Vec::new() };
    d.poll_faults();
    core::mem::take(&mut d.faults)
}

/// What `gpu iommu` shows.
pub struct Status {
    pub dev: PciAddr,
    pub pages: usize,
    pub faults_total: u64,
}

pub fn status() -> Option<Status> {
    let mut st = STATE.lock();
    let d = st.domain.as_mut()?;
    d.poll_faults();
    Some(Status { dev: d.dev, pages: d.granted.len(), faults_total: d.faults_total })
}
