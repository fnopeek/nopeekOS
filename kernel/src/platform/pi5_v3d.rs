//! V3D on the Pi 5, the kernel's share (`pi5_core::v3d`): power, the SMS,
//! the invariant state and the MMU's page table, brought up when the
//! display module opens V3D. The module runs jobs through the windows of
//! the display set without the MMU registers; V3D reaches exactly the
//! pages mapped here.
//!
//! Only pages granted to the display device can be mapped
//! (`drivers::iommu` reports every grant and revoke here, whether or not
//! the HVS's IOMMU translates); a mapping goes with the last grant of any
//! of its pages, so a buffer the kernel takes back is never in reach.

use super::dt::Fdt;
use crate::hw::Mmio;
use crate::kprintln;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use pi5_core::hw::{Block, Hw};
use pi5_core::v3d::{self, Ident, Va, PT_ENTRIES};
use spin::Mutex;

/// `RPI_FIRMWARE_V3D_CLK_ID`.
const V3D_CLK: u32 = 5;
const PAGE: u64 = 4096;

struct Regs {
    pm: Mmio,
    hub: Mmio,
    core: Mmio,
    sms: Mmio,
}

impl Hw for Regs {
    fn read(&mut self, b: Block, off: u64, _: u8) -> u32 {
        self.block(b).r32(off as usize)
    }
    fn write(&mut self, b: Block, off: u64, _: u8, v: u32) {
        self.block(b).w32(off as usize, v)
    }
    fn delay_us(&mut self, us: u64) { crate::time::delay_us(us) }
}

impl Regs {
    fn block(&self, b: Block) -> &Mmio {
        match b {
            Block::Pm => &self.pm,
            Block::V3dHub => &self.hub,
            Block::V3dCore => &self.core,
            Block::V3dSms => &self.sms,
            _ => unreachable!("not a V3D block"),
        }
    }
}

struct Mapping {
    va: u32,
    pages: Vec<u64>,
}

struct State {
    regs: Regs,
    /// The page table: `PT_ENTRIES` words, uncached.
    pt: u64,
    va: Va,
    ident: Ident,
    /// Mappings by first V3D page.
    maps: BTreeMap<u32, Mapping>,
    /// Physical page to the mapping that holds it.
    by_page: BTreeMap<u64, u32>,
}

// SAFETY: the page table and windows are owned by this state, reached only
// under `STATE`.
unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Pages granted to the display device, with their grant count.
static GRANTED: Mutex<BTreeMap<u64, u32>> = Mutex::new(BTreeMap::new());

fn map_window(base: u64, len: u64) -> Option<Mmio> {
    // SAFETY: a register window of the V3D or power node of the boot tree.
    unsafe { Mmio::map(base, len) }.ok()
}

fn named(fdt: &Fdt, node: &super::dt::Node, name: &[u8]) -> Option<(u64, u64)> {
    let idx = fdt.prop_str_index(node, b"reg-names", name)?;
    let mut i = 0;
    let mut found = None;
    fdt.reg(node, |a, s| {
        if i == idx { found = Some((a, s)); }
        i += 1;
    });
    found
}

/// `v3d_platform_drm_probe` up to the first resume: the windows, the
/// clock (prepared and raised to its maximum, as clk-raspberrypi does for
/// it), the GRAFX and V3D power domains, the SMS, the invariant state and
/// the page table. Once; later calls return what the first found.
pub fn open() -> Option<Ident> {
    let mut g = STATE.lock();
    if let Some(s) = g.as_ref() {
        return Some(s.ident);
    }
    let fdt = super::dt::boot()?;
    let node = fdt.find_compatible(b"brcm,2712-v3d")?;
    let pm_node = fdt.find_phandle(fdt.prop_cell(&node, b"power-domains", 0)?)?;
    if !fdt.prop_has_str(&pm_node, b"compatible", b"brcm,bcm2712-pm") {
        return None;
    }
    let (pa, pl) = fdt.reg0(&pm_node)?;
    let (ha, hl) = named(&fdt, &node, b"hub")?;
    let (ca, cl) = named(&fdt, &node, b"core0")?;
    let (sa, sl) = named(&fdt, &node, b"sms")?;
    let mut regs = Regs {
        pm: map_window(pa, pl)?,
        hub: map_window(ha, hl)?,
        core: map_window(ca, cl)?,
        sms: map_window(sa, sl)?,
    };

    // raspberrypi_fw_prepare with `maximize`.
    let max = super::rpi_fw::clock_max_rate(V3D_CLK)?;
    super::rpi_fw::property::<3>(pi5_core::rpi_fw::RPI_FIRMWARE_SET_CLOCK_STATE,
        [V3D_CLK, pi5_core::rpi_fw::RPI_FIRMWARE_STATE_ENABLE_BIT, 0])?;
    super::rpi_fw::property::<3>(pi5_core::rpi_fw::RPI_FIRMWARE_SET_CLOCK_RATE, [V3D_CLK, max, 0])?;

    if let Err(e) = v3d::grafx_power_on(&mut regs) {
        kprintln!("[npk] v3d: GRAFX power: {:?}", e);
        return None;
    }
    v3d::v3d_domain_on(&mut regs);
    if let Err(e) = v3d::resume_sms(&mut regs) {
        kprintln!("[npk] v3d: {:?}", e);
        return None;
    }
    let ident = match v3d::ident(&mut regs) {
        Ok(i) => i,
        Err(e) => { kprintln!("[npk] v3d: {:?}", e); return None; }
    };
    v3d::init_hw_state(&mut regs);

    let pt_pages = (PT_ENTRIES * 4) / PAGE as usize;
    let pt = crate::arch::aarch64::dma::alloc(pt_pages, 1, 1 << 40)?;
    let Some(scratch) = crate::arch::aarch64::dma::alloc(1, 1, 1 << 40) else {
        crate::arch::aarch64::dma::free(pt, pt_pages);
        return None;
    };
    // SAFETY: fresh uncached pages owned by this state.
    unsafe {
        core::ptr::write_bytes(pt as *mut u8, 0, pt_pages * PAGE as usize);
        core::ptr::write_bytes(scratch as *mut u8, 0, PAGE as usize);
    }
    if let Err(e) = v3d::mmu_set_page_table(&mut regs, pt, scratch) {
        kprintln!("[npk] v3d: {:?}", e);
        return None;
    }
    kprintln!("[npk] v3d: V3D {}.{} rev {}, {} QPUs, {}-bit addresses",
        ident.ver / 10, ident.ver % 10, ident.rev, ident.qpus, ident.va_width);
    *g = Some(State {
        regs,
        pt,
        va: Va::new(1, PT_ENTRIES as u32),
        ident,
        maps: BTreeMap::new(),
        by_page: BTreeMap::new(),
    });
    Some(ident)
}

fn pt(s: &mut State) -> &mut [u32] {
    // SAFETY: `s.pt` is the uncached page table of `PT_ENTRIES` words this
    // state allocated; `STATE` is held.
    unsafe { core::slice::from_raw_parts_mut(s.pt as *mut u32, PT_ENTRIES) }
}

/// Map the 4 KiB pages `pages` (physical, in order) at a new V3D address;
/// every one must be granted to the display device now. The address in
/// bytes, or None.
pub fn map(pages: &[u64]) -> Option<u32> {
    if pages.is_empty() || pages.iter().any(|p| p % PAGE != 0) {
        return None;
    }
    {
        let g = GRANTED.lock();
        if pages.iter().any(|p| !g.contains_key(p)) {
            return None;
        }
    }
    let mut guard = STATE.lock();
    let s = guard.as_mut()?;
    let n = pages.len() as u32;
    let start = s.va.alloc(n)?;
    // The pages as contiguous runs, as a scatterlist hands them over.
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for &p in pages {
        match runs.last_mut() {
            Some((a, l)) if *a + *l == p => *l += PAGE,
            _ => runs.push((p, PAGE)),
        }
    }
    if !v3d::insert_ptes(pt(s), start, &runs) {
        s.va.free(start, n);
        return None;
    }
    let _ = v3d::mmu_flush_all(&mut s.regs);
    for &p in pages {
        s.by_page.insert(p, start);
    }
    s.maps.insert(start, Mapping { va: start, pages: pages.to_vec() });
    Some(start << v3d::V3D_MMU_PAGE_SHIFT)
}

fn unmap_locked(s: &mut State, start: u32) {
    let Some(m) = s.maps.remove(&start) else { return };
    v3d::remove_ptes(pt(s), m.va, m.pages.len() as u32);
    let _ = v3d::mmu_flush_all(&mut s.regs);
    for p in &m.pages {
        s.by_page.remove(p);
    }
    s.va.free(m.va, m.pages.len() as u32);
}

/// Take the mapping at V3D address `va` (bytes) away.
pub fn unmap(va: u32) -> bool {
    let mut guard = STATE.lock();
    let Some(s) = guard.as_mut() else { return false };
    let start = va >> v3d::V3D_MMU_PAGE_SHIFT;
    if !s.maps.contains_key(&start) {
        return false;
    }
    unmap_locked(s, start);
    true
}

/// `drivers::iommu`: `pages` pages from `phys` granted to the display
/// device (+1) or revoked (-1). A page's last revoke takes every mapping
/// holding it away.
pub fn note(phys: u64, pages: usize, grant: bool) {
    let mut gone = Vec::new();
    {
        let mut g = GRANTED.lock();
        for i in 0..pages as u64 {
            let p = (phys & !(PAGE - 1)) + i * PAGE;
            if grant {
                *g.entry(p).or_insert(0) += 1;
            } else if let Some(c) = g.get_mut(&p) {
                *c -= 1;
                if *c == 0 {
                    g.remove(&p);
                    gone.push(p);
                }
            }
        }
    }
    if gone.is_empty() {
        return;
    }
    let mut guard = STATE.lock();
    let Some(s) = guard.as_mut() else { return };
    let starts: Vec<u32> = gone.iter().filter_map(|p| s.by_page.get(p).copied()).collect();
    for start in starts {
        unmap_locked(s, start);
    }
}
