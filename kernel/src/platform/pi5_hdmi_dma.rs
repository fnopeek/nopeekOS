//! The DMA channel that feeds an HDMI port's audio FIFO, as Linux's
//! `vc4_hdmi_audio_init` and the ASoC dmaengine PCM set it up: the
//! channel named by the port's `dmas` (`audio-rx`) on its
//! `brcm,bcm2712-dma` controller, a cyclic transfer from a ring of
//! periods to `HDMI_MAI_DATA` in the port's `hd` window. The driver is
//! `pi5_core::bcm2835_dma`.
//!
//! The kernel's own and never a module's: `dma40` sits behind no IOMMU, so
//! whoever writes its control blocks reaches all of memory. A module only
//! ever sees the ring's samples, never a bus address.

use super::dt::{Fdt, Node};
use crate::hw::Mmio;
use crate::kprintln;
use core::sync::atomic::{AtomicU64, Ordering};
use pi5_core::bcm2835_dma::{self, Chan, Cyclic, CB_SIZE};
use pi5_core::hw::{Block, Hw};
use spin::Mutex;

/// `HDMI_MAI_DATA` in the `hd` window (`vc6_hdmi_hdmi0_fields`,
/// `vc6_hdmi_hdmi1_fields`).
const MAI_DATA: [u64; 2] = [0x1c, 0x3c];
/// `HDMI_MAI_CTL` in the `hd` window, `HDMI_SCHEDULER_CONTROL` in `hdmi`.
const MAI_CTL: [u64; 2] = [0x10, 0x30];
const SCHEDULER_CONTROL: u64 = 0xe8;
const PAGE: usize = 4096;

/// The controller's registers from its own `reg` on; channel offsets count
/// from channel 0 (`bcm2835_dma_probe` rebases `base`).
struct DmaRegs {
    mmio: Mmio,
    /// `reg` start minus the rebased origin.
    skip: u64,
}

impl Hw for DmaRegs {
    fn read(&mut self, _: Block, off: u64, _: u8) -> u32 {
        self.mmio.r32(off - self.skip)
    }
    fn write(&mut self, _: Block, off: u64, _: u8, v: u32) {
        self.mmio.w32(off - self.skip, v)
    }
    fn delay_us(&mut self, us: u64) {
        crate::time::delay_us(us)
    }
}

struct State {
    regs: DmaRegs,
    chan: Chan,
    irq: u32,
    cbs: u64,
    cb_pages: usize,
    ring: u64,
    ring_pages: usize,
    d: Cyclic,
    running: bool,
    /// `HDMI_MAI_CTL` and `HDMI_SCHEDULER_CONTROL` of the port.
    mai_ctl: u64,
    scheduler: u64,
}

// SAFETY: the MMIO window and the DMA pages are plain addresses owned by
// this driver; every access goes through `STATE`'s lock.
unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Periods completed since `start`.
static PERIODS: AtomicU64 = AtomicU64::new(0);
/// Run on every completed period, in interrupt context.
static ON_PERIOD: Mutex<Option<fn()>> = Mutex::new(None);

/// The `dmas` of HDMI port `port`: its controller and the cell.
fn dmas(fdt: &Fdt, port: usize) -> Option<(Node, Node, u32)> {
    let compat: &[u8] = if port == 0 { b"brcm,bcm2712-hdmi0" } else { b"brcm,bcm2712-hdmi1" };
    let hdmi = fdt.find_compatible(compat).filter(|n| fdt.is_enabled(n))?;
    let idx = fdt.prop_str_index(&hdmi, b"dma-names", b"audio-rx").unwrap_or(0);
    let ctrl = fdt.find_phandle(fdt.prop_cell(&hdmi, b"dmas", idx * 2)?)?;
    if !fdt.prop_has_str(&ctrl, b"compatible", b"brcm,bcm2712-dma") || !fdt.is_enabled(&ctrl) {
        return None;
    }
    let cell = fdt.prop_cell(&hdmi, b"dmas", idx * 2 + 1)?;
    Some((hdmi, ctrl, cell))
}

/// Window `name` of `node` by `reg-names`.
fn named(fdt: &Fdt, node: &Node, name: &[u8]) -> Option<(u64, u64)> {
    let idx = fdt.prop_str_index(node, b"reg-names", name)?;
    let mut regs = alloc::vec::Vec::new();
    fdt.reg(node, |a, s| regs.push((a, s)));
    regs.get(idx).copied()
}

/// The GIC INTID of the controller's interrupt `dma<ch>`: a GIC SPI,
/// three cells, level-high.
fn chan_irq(fdt: &Fdt, ctrl: &Node, ch: u32) -> Option<u32> {
    if ch > 99 { return None; }
    let mut name = *b"dma\0\0";
    let n = if ch < 10 {
        name[3] = b'0' + ch as u8;
        4
    } else {
        name[3] = b'0' + (ch / 10) as u8;
        name[4] = b'0' + (ch % 10) as u8;
        5
    };
    let idx = fdt.prop_str_index(ctrl, b"interrupt-names", &name[..n])?;
    let kind = fdt.prop_cell(ctrl, b"interrupts", idx * 3)?;
    let line = fdt.prop_cell(ctrl, b"interrupts", idx * 3 + 1)?;
    (kind == 0).then_some(line + 32)
}

fn on_irq(_: u32) {
    let mut g = STATE.lock();
    let Some(s) = g.as_mut() else { return };
    match s.chan.irq(&mut s.regs) {
        Ok(true) => {
            PERIODS.fetch_add(1, Ordering::AcqRel);
            drop(g);
            if let Some(f) = *ON_PERIOD.lock() {
                f();
            }
        }
        Ok(false) => {}
        Err((cs, dbg)) => {
            kprintln!("[npk] hdmi audio: DMA error CS {:#x} DEBUG {:#x}", cs, dbg);
            let _ = s.chan.abort(&mut s.regs);
            s.running = false;
        }
    }
}

/// Set up HDMI port `port`'s audio DMA for a ring of `periods` periods of
/// `period_bytes` each (a multiple of 32: the FIFO takes 128-bit beats).
/// Returns the ring's CPU address; it is uncached and zeroed. `on_period`
/// runs in interrupt context after each period.
pub fn open(port: usize, periods: usize, period_bytes: usize, on_period: fn()) -> Option<u64> {
    if port > 1 || periods < 2 || period_bytes == 0 || period_bytes % 32 != 0 {
        return None;
    }
    close();
    let fdt = super::dt::boot()?;
    let Some((hdmi, ctrl, cell)) = dmas(&fdt, port) else {
        kprintln!("[npk] hdmi audio: HDMI{} has no audio DMA in the device tree", port);
        return None;
    };
    let (hd, _) = named(&fdt, &hdmi, b"hd")?;
    let (hdmi_win, _) = named(&fdt, &hdmi, b"hdmi")?;
    let (reg_base, reg_size) = fdt.reg0(&ctrl)?;
    let mask = fdt.prop_u32(&ctrl, b"brcm,dma-channel-mask")?;
    let (origin, mut chans) = bcm2835_dma::channels(reg_base, reg_size, mask);
    let ch = chans.next()?;
    let irq = chan_irq(&fdt, &ctrl, ch)?;
    // SAFETY: the `brcm,bcm2712-dma` node's own register window.
    let mmio = unsafe { Mmio::map(reg_base, reg_size) }.ok()?;
    let mut regs = DmaRegs { mmio, skip: reg_base - origin };

    let cb_pages = (periods * CB_SIZE).div_ceil(PAGE);
    let ring_bytes = periods * period_bytes;
    let ring_pages = ring_bytes.div_ceil(PAGE);
    let cbs = crate::arch::aarch64::dma::alloc(cb_pages, 1, 1 << 40)?;
    let Some(ring) = crate::arch::aarch64::dma::alloc(ring_pages, 1, 1 << 40) else {
        crate::arch::aarch64::dma::free(cbs, cb_pages);
        return None;
    };
    // SAFETY: both allocations are this driver's, uncached and mapped.
    unsafe {
        core::ptr::write_bytes(ring as *mut u8, 0, ring_pages * PAGE);
        core::ptr::write_bytes(cbs as *mut u8, 0, cb_pages * PAGE);
    }
    let chan = Chan::new(ch, cell);
    // SAFETY: `cbs` holds `periods` 32-byte blocks, page-aligned, uncached.
    let blocks = unsafe { core::slice::from_raw_parts_mut(cbs as *mut [u32; 8], periods) };
    let Some(d) = chan.prep_cyclic(blocks, cbs, ring, ring_bytes, period_bytes, hd + MAI_DATA[port]) else {
        crate::arch::aarch64::dma::free(ring, ring_pages);
        crate::arch::aarch64::dma::free(cbs, cb_pages);
        return None;
    };
    // A channel the firmware left running is stopped first.
    let _ = chan.abort(&mut regs);
    *ON_PERIOD.lock() = Some(on_period);
    *STATE.lock() = Some(State { regs, chan, irq, cbs, cb_pages, ring, ring_pages, d, running: false,
        mai_ctl: hd + MAI_CTL[port], scheduler: hdmi_win + SCHEDULER_CONTROL });
    crate::arch::aarch64::gic::enable(irq, on_irq);
    kprintln!("[npk] hdmi audio: HDMI{} on DMA channel {} (DREQ {}, IRQ {}), {} x {} bytes",
        port, ch, cell & 0x1f, irq, periods, period_bytes);
    Some(ring)
}

/// `bcm2835_dma_issue_pending`: start the cyclic transfer.
pub fn start() {
    let mut g = STATE.lock();
    let Some(s) = g.as_mut() else { return };
    if s.running { return; }
    PERIODS.store(0, Ordering::Release);
    let d = s.d;
    s.chan.start(&mut s.regs, &d);
    s.running = true;
}

/// `bcm2835_dma_terminate_all`.
pub fn stop() {
    let mut g = STATE.lock();
    let Some(s) = g.as_mut() else { return };
    if !s.chan.abort(&mut s.regs) {
        kprintln!("[npk] hdmi audio: the DMA channel did not pause in time");
    }
    s.running = false;
}

/// The byte offset in the ring the controller reads next.
pub fn position() -> Option<usize> {
    let mut g = STATE.lock();
    let s = g.as_mut()?;
    let d = s.d;
    Some(d.buf_len - s.chan.residue(&mut s.regs, &d))
}

/// `HDMI_MAI_CTL` and `HDMI_SCHEDULER_CONTROL` while the stream runs:
/// whether the MAI takes samples and the output is in HDMI mode.
pub fn mai_state() -> Option<(u32, u32)> {
    let g = STATE.lock();
    let s = g.as_ref().filter(|s| s.running)?;
    // SAFETY: registers of the running port's `hd` and `hdmi` windows from
    // the boot tree; the display module keeps the port powered while the
    // stream runs.
    unsafe {
        Some((core::ptr::read_volatile(s.mai_ctl as *const u32),
              core::ptr::read_volatile(s.scheduler as *const u32)))
    }
}

/// Periods completed since `start`.
pub fn periods() -> u64 {
    PERIODS.load(Ordering::Acquire)
}

/// Stop and give back the channel and its memory.
pub fn close() {
    let Some(mut s) = STATE.lock().take() else { return };
    let _ = s.chan.abort(&mut s.regs);
    crate::arch::aarch64::gic::mask(s.irq);
    crate::arch::aarch64::gic::clear_handler(s.irq);
    *ON_PERIOD.lock() = None;
    crate::arch::aarch64::dma::free(s.ring, s.ring_pages);
    crate::arch::aarch64::dma::free(s.cbs, s.cb_pages);
}
