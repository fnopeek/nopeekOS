//! The Raspberry Pi 5's Ethernet: RP1's GEM (Cadence, `macb` in Linux)
//! with its BCM54213PE PHY, as the network device `netdev` dispatches to.
//! The register work is `pi5_core`'s port of Linux; this module binds it
//! to the device tree, gives it DMA memory, and runs it on one fiber that
//! is the interrupt thread, NAPI and the PHY state machine's work in one:
//! woken by RP1's line or by the next PHY poll or LPI timer.

use super::dt::{Fdt, Node};
use super::pi5::Board;
use crate::hw::DmaRegion;
use crate::kprintln;
use crate::netdev::{NetError, MTU};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use pi5_core::eth::{self, Eth};
use pi5_core::gem::{self, Ring, Rings};
use pi5_core::phy::Event;
use spin::Mutex;

/// Transmit buffers, each one frame with padding and FCS.
const TX_BUFS: usize = 128;
const TX_BUF_BYTES: usize = 2048;
/// Frames received and not yet taken by the stack; more are dropped, as
/// a full backlog drops in Linux (`netdev_max_backlog`).
const RX_BACKLOG: usize = 1000;
/// NAPI's weight for one poll.
const NAPI_WEIGHT: usize = 64;
/// `PHY_STATE_TIME`.
const PHY_POLL_US: u64 = 1_000_000;
/// When the first link-up reports what arrived: proof that RP1's line
/// reaches the fiber.
const REPORT_AFTER_US: u64 = 5_000_000;
/// The flag cell of a `gpios` specifier.
const GPIO_ACTIVE_LOW: u32 = 1;

/// Descriptor memory and buffers, as the GEM reaches them.
struct Bufs {
    /// TX ring, RX ring and the tie-off descriptor in one region, so their
    /// bus addresses share the upper 32 bits.
    rings: DmaRegion,
    rx: DmaRegion,
    tx: DmaRegion,
    tx_free: Vec<usize>,
}

const TX_RING_OFF: usize = 0;
const RX_RING_OFF: usize = gem::RING_BYTES;
const TIEOFF_OFF: usize = 2 * gem::RING_BYTES;

impl Bufs {
    fn alloc(bus_off: u64) -> Option<Bufs> {
        let ring_pages = (TIEOFF_OFF + gem::DESC_SIZE).div_ceil(4096);
        let rings = DmaRegion::alloc_zeroed_aligned_below(ring_pages, ring_pages.next_power_of_two(), u64::MAX)?
            .with_bus_offset(bus_off);
        let rx = DmaRegion::alloc_zeroed((gem::RING_SIZE * gem::RX_BUFFER_SIZE).div_ceil(4096))?
            .with_bus_offset(bus_off);
        let tx = DmaRegion::alloc_zeroed((TX_BUFS * TX_BUF_BYTES).div_ceil(4096))?.with_bus_offset(bus_off);
        Some(Bufs { rings, rx, tx, tx_free: (0..TX_BUFS).rev().collect() })
    }

    fn rings(&self) -> Rings {
        let b = self.rings.bus();
        Rings { tx: b + TX_RING_OFF as u64, rx: b + RX_RING_OFF as u64, tieoff: b + TIEOFF_OFF as u64 }
    }

    fn desc(ring: Ring, index: usize, word: usize) -> usize {
        let base = match ring {
            Ring::Tx => TX_RING_OFF,
            Ring::Rx => RX_RING_OFF,
            Ring::TieOff => TIEOFF_OFF,
        };
        base + index * gem::DESC_SIZE + word * 4
    }
}

/// The CPU side of the deliveries `rx_poll` makes.
static RX: Mutex<VecDeque<(Vec<u8>, bool)>> = Mutex::new(VecDeque::new());

impl gem::Dma for Bufs {
    fn read(&mut self, ring: Ring, index: usize, word: usize) -> u32 {
        self.rings.r32(Self::desc(ring, index, word))
    }
    fn write(&mut self, ring: Ring, index: usize, word: usize, v: u32) {
        self.rings.w32(Self::desc(ring, index, word), v)
    }
    fn barrier(&mut self) {
        // SAFETY: a barrier; no memory is accessed. `dsb sy` covers
        // `dma_wmb`, `wmb` and `rmb` alike.
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }
    fn rx_alloc(&mut self, slot: usize) -> Option<u64> {
        (slot < gem::RING_SIZE).then(|| self.rx.bus() + (slot * gem::RX_BUFFER_SIZE) as u64)
    }
    fn rx_deliver(&mut self, _slot: usize, bus: u64, len: usize, csum_ok: bool) {
        let Some(off) = bus.checked_sub(self.rx.bus()) else { return };
        if len > MTU || len > gem::RX_BUFFER_SIZE || off as usize + len > gem::RING_SIZE * gem::RX_BUFFER_SIZE {
            return;
        }
        let mut frame = alloc::vec![0u8; len];
        self.rx.copy_out(off, &mut frame);
        let mut q = RX.lock();
        if q.len() < RX_BACKLOG {
            q.push_back((frame, csum_ok));
            crate::netdev::note_rx_available(1);
        }
    }
    fn rx_free(&mut self, _slot: usize, _bus: u64) {}
    fn tx_done(&mut self, bus: u64, _len: usize) {
        if let Some(off) = bus.checked_sub(self.tx.bus()) {
            let i = off as usize / TX_BUF_BYTES;
            if i < TX_BUFS && !self.tx_free.contains(&i) {
                self.tx_free.push(i);
            }
        }
    }
}

struct Device {
    eth: Eth,
    bufs: Bufs,
    regs: Board,
}

static DEV: Mutex<Option<Device>> = Mutex::new(None);
static PRESENT: AtomicBool = AtomicBool::new(false);
static LINK: AtomicBool = AtomicBool::new(false);

/// `eth_hw_addr_random`: random, unicast, locally administered.
fn random_mac() -> [u8; 6] {
    let r = crate::security::csprng::random_u64().to_le_bytes();
    let mut m = [r[0], r[1], r[2], r[3], r[4], r[5]];
    m[0] = (m[0] & 0xfe) | 0x02;
    m
}

/// The DT's `ethernet@100000`: the PHY behind `phy-handle`, the reset
/// line of `phy-reset-gpios` and the firmware's `local-mac-address`.
fn config(fdt: &Fdt, node: &Node) -> eth::Config {
    let mut cfg = eth::PI5;
    if let Some(phy) = fdt.prop_u32(node, b"phy-handle").and_then(|ph| fdt.find_phandle(ph)) {
        if let Some(addr) = fdt.prop_u32(&phy, b"reg") {
            cfg.phy_addr = addr;
        }
    }
    if let Some(pin) = fdt.prop_cell(node, b"phy-reset-gpios", 1) {
        cfg.reset_gpio = pin;
        cfg.reset_active_low = fdt.prop_cell(node, b"phy-reset-gpios", 2).unwrap_or(0) & GPIO_ACTIVE_LOW != 0;
    }
    if let Some(ms) = fdt.prop_u32(node, b"phy-reset-duration") {
        cfg.reset_ms = ms as u64;
    }
    if let Some(ms) = fdt.prop_u32(node, b"phy-reset-post-delay") {
        cfg.reset_post_ms = ms as u64;
    }
    let mut mac = [0u8; 6];
    cfg.dt_mac = fdt.prop_bytes(node, b"local-mac-address", &mut mac).map(|_| mac);
    if let Some(line) = fdt.prop_cell(node, b"interrupts", 0) {
        cfg.irq = line;
    }
    cfg
}

/// Probe and open the GEM, then start its fiber. Needs RP1 up
/// (`pi5::init`) and worker cores. True when the device is open.
pub fn init(fdt: &Fdt) -> bool {
    let Some(node) = super::pi5::rp1_child(fdt, b"raspberrypi,rp1-gem") else {
        kprintln!("[npk] eth: no RP1 GEM in the device tree");
        return false;
    };
    let Some((rp1, board, dma_off)) = super::pi5::rp1() else {
        kprintln!("[npk] eth: RP1 is not up");
        return false;
    };
    if crate::smp::scheduler::worker_count() == 0 {
        kprintln!("[npk] eth: no worker core for its fiber");
        return false;
    }
    let cfg = config(fdt, &node);
    let mut regs = board;
    if let Err(e) = pi5_core::clk_rp1::eth_clocks(&mut regs) {
        kprintln!("[npk] eth: clocks: {:?}", e);
        return false;
    }
    let Some((irq, line, intid, _)) = super::pi5::rp1_irq_map(fdt, &node) else {
        kprintln!("[npk] eth: no interrupt");
        return false;
    };
    let mut eth = match eth::probe(&mut regs, &rp1, &cfg, random_mac()) {
        Ok(e) => e,
        Err(e) => {
            kprintln!("[npk] eth: probe: {:?}", e);
            crate::irq::release(irq);
            return false;
        }
    };
    let Some(mut bufs) = Bufs::alloc(dma_off) else {
        kprintln!("[npk] eth: no DMA memory");
        crate::irq::release(irq);
        return false;
    };
    let now_ns = crate::time::wall::now_ns().unwrap_or(0);
    let rings = bufs.rings();
    if let Err(e) = eth.open(&mut regs, &mut bufs, rings, now_ns / 1_000_000_000, (now_ns % 1_000_000_000) as u32) {
        kprintln!("[npk] eth: open: {:?}", e);
        crate::irq::release(irq);
        return false;
    }
    let m = eth.gem.mac;
    kprintln!("[npk] eth: GEM {:#x}, PHY {:#x} at {}, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, line {} -> INTID {} (irq {})",
        eth.gem.caps.mid, pi5_core::phy::PHY_ID_BCM54213PE, cfg.phy_addr,
        m[0], m[1], m[2], m[3], m[4], m[5], line, intid, irq);
    *DEV.lock() = Some(Device { eth, bufs, regs });
    PRESENT.store(true, Ordering::Release);
    crate::smp::scheduler::spawn_fiber(eth_fiber, irq as u64);
    true
}

/// The interrupt thread with NAPI and phylib's poll: services the GEM when
/// its line fired, polls the PHY every `PHY_POLL_US`, runs the LPI work
/// when it is due, hands received frames to the stack.
fn eth_fiber(irq: u64) {
    let irq = irq as crate::irq::Irq;
    let mut fired = false;
    let mut next_poll = crate::time::uptime_us();
    let mut lpi_due: Option<u64> = None;
    let mut report_at: Option<u64> = None;
    let mut frames = 0u64;
    loop {
        let since = crate::irq::arm(irq);
        let now = crate::time::uptime_us();
        let mut rx = false;
        if let Some(d) = DEV.lock().as_mut() {
            let Device { eth, bufs, regs } = d;
            if fired {
                let i = eth.gem.interrupt(regs);
                if i.tx_error { eth.gem.tx_error_task(regs, bufs); }
                if i.hresp { eth.gem.hresp_error_task(regs, bufs); }
                if i.tx { while eth.gem.tx_poll(regs, bufs, NAPI_WEIGHT).1 {} }
                if i.rx {
                    loop {
                        let (n, again) = eth.gem.rx_poll(regs, bufs, NAPI_WEIGHT);
                        frames += n as u64;
                        if !again { break; }
                    }
                    rx = true;
                }
            }
            if now >= next_poll {
                next_poll = now + PHY_POLL_US;
                match eth.poll_link(regs, bufs) {
                    Ok(Some(Event::Up(l))) => {
                        LINK.store(true, Ordering::Release);
                        report_at = Some(now + REPORT_AFTER_US);
                        kprintln!("[npk] eth: link up, {} Mbit/s {}-duplex{}", l.speed,
                            if l.full_duplex { "full" } else { "half" }, if l.tx_lpi { ", EEE" } else { "" });
                    }
                    Ok(Some(Event::Down)) => {
                        LINK.store(false, Ordering::Release);
                        kprintln!("[npk] eth: link down");
                    }
                    Ok(None) => {}
                    Err(e) => kprintln!("[npk] eth: PHY: {:?}", e),
                }
            }
            if report_at.is_some_and(|t| now >= t) {
                report_at = None;
                kprintln!("[npk] eth: {} interrupts, {} frames received since open",
                    crate::irq::fired_count(irq), frames);
            }
            if lpi_due.is_some_and(|t| now >= t) {
                lpi_due = None;
                eth.gem.tx_lpi_work(regs);
            }
            while let Some(t) = eth.gem.take_lpi_timer() {
                lpi_due = match t {
                    gem::LpiTimer::Arm(us) => Some(now + us),
                    gem::LpiTimer::Cancel => None,
                };
            }
        }
        if rx {
            crate::net::poll_rx_only();
        }
        let due = [lpi_due, report_at].into_iter().flatten().fold(next_poll, u64::min);
        let ms = due.saturating_sub(crate::time::uptime_us()).div_ceil(1000).max(1);
        fired = crate::irq::wait(irq, since, ms);
    }
}

pub fn is_available() -> bool {
    PRESENT.load(Ordering::Acquire)
}

pub fn link_up() -> bool {
    LINK.load(Ordering::Acquire)
}

pub fn mac() -> Option<[u8; 6]> {
    DEV.lock().as_ref().map(|d| d.eth.gem.mac)
}

/// `macb_start_xmit` for one linear frame: padded, FCS appended
/// (`macb_pad_and_fcs`), into a TX buffer, onto the ring.
pub fn send(frame: &[u8]) -> Result<(), NetError> {
    if frame.len() > MTU {
        return Err(NetError::FrameTooLarge);
    }
    let mut g = DEV.lock();
    let Some(Device { eth, bufs, regs }) = g.as_mut() else { return Err(NetError::NotInitialized) };
    let Some(i) = bufs.tx_free.pop() else { return Err(NetError::QueueFull) };
    let mut buf = [0u8; TX_BUF_BYTES];
    buf[..frame.len()].copy_from_slice(frame);
    let Some(len) = gem::pad_and_fcs(&mut buf, frame.len()) else {
        bufs.tx_free.push(i);
        return Err(NetError::FrameTooLarge);
    };
    let off = i * TX_BUF_BYTES;
    bufs.tx.copy_in(off, &buf[..len]);
    let bus = bufs.tx.bus() + off as u64;
    match eth.gem.xmit(regs, bufs, bus, len) {
        Ok(_) => Ok(()),
        Err(gem::Busy) => {
            bufs.tx_free.push(i);
            Err(NetError::QueueFull)
        }
    }
}

/// A received frame and whether the GEM checked its checksum.
pub fn recv(buf: &mut [u8; MTU]) -> Option<(usize, bool)> {
    let (frame, csum_ok) = RX.lock().pop_front()?;
    let n = frame.len().min(MTU);
    buf[..n].copy_from_slice(&frame[..n]);
    Some((n, csum_ok))
}
