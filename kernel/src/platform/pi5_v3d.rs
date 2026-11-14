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
    /// Where an access outside the page table lands (`ILLEGAL_ADDR`).
    scratch: u64,
    va: Va,
    ident: Ident,
    /// Powered and its state programmed (runtime-active, in Linux's terms).
    /// The page table may change while it is off; resuming programs it.
    powered: bool,
    /// The clock's lowest and highest rate (`raspberrypi_clk_register`).
    min_rate: u32,
    max_rate: u32,
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

/// `raspberrypi_fw_prepare` with `maximize`: the clock on, at its highest
/// rate. The firmware may park the caller: never under `STATE`.
fn clock_prepare(max: u32, t: &mut Phases) -> Option<()> {
    super::rpi_fw::property::<3>(pi5_core::rpi_fw::RPI_FIRMWARE_SET_CLOCK_STATE,
        [V3D_CLK, pi5_core::rpi_fw::RPI_FIRMWARE_STATE_ENABLE_BIT, 0])?;
    t.mark();
    super::rpi_fw::property::<3>(pi5_core::rpi_fw::RPI_FIRMWARE_SET_CLOCK_RATE, [V3D_CLK, max, 0])?;
    t.mark();
    Some(())
}

/// `raspberrypi_fw_unprepare`: the rate to its lowest (the firmware does
/// not stop the clock), then the clock off. Not under `STATE`.
fn clock_unprepare(min: u32, t: &mut Phases) {
    let _ = super::rpi_fw::property::<3>(pi5_core::rpi_fw::RPI_FIRMWARE_SET_CLOCK_RATE, [V3D_CLK, min, 0]);
    t.mark();
    let _ = super::rpi_fw::property::<3>(pi5_core::rpi_fw::RPI_FIRMWARE_SET_CLOCK_STATE, [V3D_CLK, 0, 0]);
    t.mark();
}

/// Power transitions reported in full; after them only the slow ones.
const POWER_REPORTED: u32 = 4;
/// A transition this slow (us) is reported always.
const POWER_SLOW_US: u64 = 2000;
static POWER_REPORTS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The steps of one power transition, timed.
struct Phases {
    t0: u64,
    marks: [u64; 6],
    n: usize,
}

impl Phases {
    fn start() -> Phases {
        Phases { t0: crate::time::counter(), marks: [0; 6], n: 0 }
    }

    fn mark(&mut self) {
        if self.n < self.marks.len() {
            self.marks[self.n] = crate::time::counter();
            self.n += 1;
        }
    }

    fn report(&self, what: &str, names: &[&str]) {
        let us = |t: u64| t * 1_000_000 / crate::time::counter_hz().max(1);
        let total = us(self.marks[self.n.saturating_sub(1)].saturating_sub(self.t0));
        let n = POWER_REPORTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n >= POWER_REPORTED && total < POWER_SLOW_US {
            return;
        }
        let mut line = alloc::format!("[npk] v3d: power {} in {} us:", what, total);
        let mut prev = self.t0;
        for (name, &m) in names.iter().zip(&self.marks[..self.n]) {
            line.push_str(&alloc::format!(" {} {}", name, us(m.saturating_sub(prev))));
            prev = m;
        }
        kprintln!("{}", line);
    }
}

/// The GRAFX domain, then GRAFX_V3D, on (genpd before the device's
/// resume).
fn domains_on(regs: &mut Regs) -> bool {
    if let Err(e) = v3d::grafx_power_on(regs) {
        kprintln!("[npk] v3d: GRAFX power: {:?}", e);
        return false;
    }
    v3d::v3d_domain_on(regs);
    true
}

/// `v3d_power_resume` after the clock is on: the SMS, the invariant state,
/// the page table. The interrupts are the module's.
fn resume_locked(s: &mut State) -> bool {
    if let Err(e) = v3d::resume_sms(&mut s.regs) {
        kprintln!("[npk] v3d: {:?}", e);
        return false;
    }
    v3d::init_hw_state(&mut s.regs);
    if let Err(e) = v3d::mmu_set_page_table(&mut s.regs, s.pt, s.scratch) {
        kprintln!("[npk] v3d: {:?}", e);
        return false;
    }
    s.powered = true;
    true
}

/// `v3d_platform_drm_probe` up to the first resume: the windows, the GRAFX
/// and V3D power domains, the clock (prepared and raised to its maximum, as
/// clk-raspberrypi does for it), the SMS, the identification, the page
/// table. Once; later calls return what the first found, powering V3D up
/// again if it was suspended.
pub fn open() -> Option<Ident> {
    if STATE.lock().is_some() {
        return power(true).then(|| STATE.lock().as_ref().map(|s| s.ident)).flatten();
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
    let max_rate = super::rpi_fw::clock_max_rate(V3D_CLK)?;
    let min_rate = super::rpi_fw::clock_min_rate(V3D_CLK)?;
    if !domains_on(&mut regs) {
        return None;
    }
    clock_prepare(max_rate, &mut Phases::start())?;

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
    if let Err(e) = v3d::resume_sms(&mut regs) {
        kprintln!("[npk] v3d: {:?}", e);
        return None;
    }
    let ident = match v3d::ident(&mut regs) {
        Ok(i) => i,
        Err(e) => { kprintln!("[npk] v3d: {:?}", e); return None; }
    };
    let mut st = State {
        regs,
        pt,
        scratch,
        va: Va::new(1, PT_ENTRIES as u32),
        ident,
        powered: false,
        min_rate,
        max_rate,
        maps: BTreeMap::new(),
        by_page: BTreeMap::new(),
    };
    v3d::init_hw_state(&mut st.regs);
    if let Err(e) = v3d::mmu_set_page_table(&mut st.regs, st.pt, st.scratch) {
        kprintln!("[npk] v3d: {:?}", e);
        return None;
    }
    st.powered = true;
    kprintln!("[npk] v3d: V3D {}.{} rev {}, {} QPUs, {}-bit addresses, clock {}..{} MHz",
        ident.ver / 10, ident.ver % 10, ident.rev, ident.qpus, ident.va_width,
        min_rate / 1_000_000, max_rate / 1_000_000);
    *STATE.lock() = Some(st);
    Some(ident)
}

/// Runtime PM of V3D (`v3d_power_resume` / `v3d_power_suspend` with the
/// genpd domains and the firmware clock around them). The module has
/// masked V3D's interrupts and cleaned its caches before a suspend
/// (`v3d_irq_disable`, `v3d_clean_caches`) and enables them after a
/// resume. False when the transition failed; a failed suspend leaves V3D
/// powered, as Linux stays active.
pub fn power(on: bool) -> bool {
    let (min, max) = match STATE.lock().as_ref() {
        Some(s) if s.powered == on => return true,
        Some(s) => (s.min_rate, s.max_rate),
        None => return false,
    };
    let mut t = Phases::start();
    if on {
        {
            let mut g = STATE.lock();
            let Some(s) = g.as_mut() else { return false };
            if !domains_on(&mut s.regs) {
                return false;
            }
        }
        t.mark();
        if clock_prepare(max, &mut t).is_none() {
            return false;
        }
        let mut g = STATE.lock();
        let Some(s) = g.as_mut() else { return false };
        let ok = resume_locked(s);
        drop(g);
        t.mark();
        t.report("on", &["domains", "clock on", "clock rate", "resume"]);
        ok
    } else {
        {
            let mut g = STATE.lock();
            let Some(s) = g.as_mut() else { return false };
            if !v3d::idle_axi(&mut s.regs) {
                kprintln!("[npk] v3d: Failed to wait for safe GMP shutdown");
            }
            t.mark();
            if let Err(e) = v3d::suspend_sms(&mut s.regs) {
                kprintln!("[npk] v3d: {:?}", e);
                v3d::gmp_resume(&mut s.regs);
                return false;
            }
            s.powered = false;
        }
        t.mark();
        clock_unprepare(min, &mut t);
        let mut g = STATE.lock();
        let Some(s) = g.as_mut() else { return false };
        v3d::v3d_domain_off(&mut s.regs);
        v3d::grafx_power_off(&mut s.regs);
        drop(g);
        t.mark();
        t.report("off", &["axi", "sms", "clock rate", "clock off", "domains"]);
        true
    }
}

/// The kernel's share of `v3d_reset` after a hang: the SMS reset, V3D
/// reset through its power domain (`reset_control_reset`) with the
/// invariant state again, then the page table. The module has disabled
/// V3D's interrupts and resets them after (`v3d_irq_disable`,
/// `v3d_irq_reset`).
pub fn reset() -> bool {
    let mut g = STATE.lock();
    let Some(s) = g.as_mut() else { return false };
    if !s.powered {
        return false;
    }
    if !v3d::reset_sms(&mut s.regs) {
        kprintln!("[npk] v3d: Failed to wait for SMS reset");
    }
    v3d::reset_v3d_domain(&mut s.regs);
    v3d::init_hw_state(&mut s.regs);
    if let Err(e) = v3d::mmu_set_page_table(&mut s.regs, s.pt, s.scratch) {
        kprintln!("[npk] v3d: {:?}", e);
        return false;
    }
    true
}

/// `v3d_hub_irq` after an MMU interrupt: the client and address, the error
/// cleared. None when V3D is not powered.
pub fn mmu_fault() -> Option<v3d::MmuFault> {
    let mut g = STATE.lock();
    let s = g.as_mut()?;
    if !s.powered {
        return None;
    }
    let w = s.ident.va_width;
    Some(v3d::mmu_fault(&mut s.regs, w))
}

/// `v3d_mmu_flush_all`: only while V3D is powered; a resume programs the
/// page table and flushes.
fn flush(s: &mut State) {
    if s.powered {
        let _ = v3d::mmu_flush_all(&mut s.regs);
    }
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
    flush(s);
    for &p in pages {
        s.by_page.insert(p, start);
    }
    s.maps.insert(start, Mapping { va: start, pages: pages.to_vec() });
    Some(start << v3d::V3D_MMU_PAGE_SHIFT)
}

fn unmap_locked(s: &mut State, start: u32) {
    let Some(m) = s.maps.remove(&start) else { return };
    v3d::remove_ptes(pt(s), m.va, m.pages.len() as u32);
    flush(s);
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
