//! xhci_core: the register side of an xHCI controller's events, after
//! Linux `drivers/usb/host/xhci.c` and `xhci-ring.c`: starting the
//! controller with its interrupts on (`xhci_run_finished`), the interrupt
//! handler (`xhci_irq`, `xhci_handle_events`), the event ring and its
//! dequeue pointer (`inc_deq`, `xhci_update_erst_dequeue`), and halting
//! (`xhci_halt`). Registers go through [`Regs`] and event ring memory
//! through [`EventMem`], so the same code runs on a controller and against
//! a recorded Linux run.

#![cfg_attr(not(test), no_std)]

#[cfg(test)]
mod tests;

/// The controller's register window, offsets from the capability base.
pub trait Regs {
    fn r32(&mut self, off: u32) -> u32;
    fn w32(&mut self, off: u32, v: u32);
    /// Wait at least `us` microseconds.
    fn delay_us(&mut self, us: u64);
}

/// The event ring's memory, offsets in bytes from its start.
pub trait EventMem {
    fn r32(&self, off: usize) -> u32;
}

/// `lo_hi_readq`, as `xhci_read_64`.
fn r64<R: Regs + ?Sized>(r: &mut R, off: u32) -> u64 {
    let lo = r.r32(off) as u64;
    let hi = r.r32(off + 4) as u64;
    lo | hi << 32
}

/// `lo_hi_writeq`, as `xhci_write_64`.
fn w64<R: Regs + ?Sized>(r: &mut R, off: u32, v: u64) {
    r.w32(off, v as u32);
    r.w32(off + 4, (v >> 32) as u32);
}

// Operational registers.
pub const USBCMD: u32 = 0x00;
pub const USBSTS: u32 = 0x04;
pub const CMD_RUN: u32 = 1 << 0;
pub const CMD_EIE: u32 = 1 << 2;
pub const CMD_HSEIE: u32 = 1 << 3;
pub const CMD_EWE: u32 = 1 << 10;
/// `XHCI_IRQS`.
pub const XHCI_IRQS: u32 = CMD_EIE | CMD_HSEIE | CMD_EWE;
pub const STS_HALT: u32 = 1 << 0;
pub const STS_FATAL: u32 = 1 << 2;
pub const STS_EINT: u32 = 1 << 3;
pub const STS_HCE: u32 = 1 << 12;

pub const PAGESIZE: u32 = 0x08;
pub const DNCTRL: u32 = 0x14;
pub const CRCR: u32 = 0x18;
pub const DCBAAP: u32 = 0x30;
pub const CONFIG: u32 = 0x38;
pub const CMD_RESET: u32 = 1 << 1;
pub const STS_CNR: u32 = 1 << 11;
/// Capability registers.
pub const HCSPARAMS1: u32 = 0x04;
pub const DBOFF: u32 = 0x14;
const HCS_SLOTS_MASK: u32 = 0xff;
const DEV_NOTE_MASK: u32 = 0xffff;
const DEV_NOTE_FWAKE: u32 = 1 << 1;
const CMD_RING_CYCLE: u64 = 1 << 0;
const CMD_RING_PTR_MASK: u64 = !0x3f;
/// `XHCI_RESET_LONG_USEC`: the reset `xhci_gen_setup` waits for.
pub const XHCI_RESET_LONG_USEC: u64 = 10 * 1000 * 1000;
/// `XHCI_RESET_SHORT_USEC`: the reset `xhci_shutdown` waits for.
pub const XHCI_RESET_SHORT_USEC: u64 = 250 * 1000;
/// `imod_interval` of xhci-plat and xhci-pci: 40 us.
pub const IMOD_INTERVAL_NS: u32 = 40_000;

// Interrupter registers.
pub const IMAN: u32 = 0x00;
pub const IMOD: u32 = 0x04;
pub const ERSTSZ: u32 = 0x08;
pub const ERSTBA: u32 = 0x10;
pub const ERDP: u32 = 0x18;
const ERST_SIZE_MASK: u32 = 0xffff;
const ERST_BASE_ADDRESS_MASK: u64 = !0x3f;
const IMODI_MASK: u32 = 0xffff;
pub const IMAN_IP: u32 = 1 << 0;
pub const IMAN_IE: u32 = 1 << 1;
pub const ERST_DESI_MASK: u64 = 0x7;
pub const ERST_EHB: u64 = 1 << 3;
pub const ERST_PTR_MASK: u64 = !0xf;

/// `TRBS_PER_SEGMENT`.
pub const TRBS_PER_SEGMENT: usize = 256;
/// `ERST_DEFAULT_SEGS`: event ring segments asked for, at most what
/// HCSPARAMS2's ERST Max allows (`xhci_alloc_interrupter`).
pub const ERST_DEFAULT_SEGS: usize = 8;
/// `XHCI_MAX_HALT_USEC`.
pub const XHCI_MAX_HALT_USEC: u64 = 32 * 1000;
const TRB_CYCLE: u32 = 1 << 0;

/// Event ring segments for a controller with HCSPARAMS2 `hcs_params2`.
pub fn event_ring_segments(hcs_params2: u32) -> usize {
    ERST_DEFAULT_SEGS.min(1 << ((hcs_params2 >> 4) & 0xf))
}

/// Where the operational registers and interrupter 0 sit in the window.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// CAPLENGTH.
    pub op: u32,
    /// RTSOFF + 0x20: interrupter 0's register set.
    pub ir0: u32,
}

/// The event ring: one block of `segs` segments of `TRBS_PER_SEGMENT`
/// TRBs at bus address `bus`, and the software dequeue cursor.
#[derive(Clone, Copy, Debug)]
pub struct EventRing {
    pub bus: u64,
    pub segs: usize,
    /// TRB index of the dequeue pointer, segment 0 first.
    pub dequeue: usize,
    pub cycle: u32,
}

/// An event TRB, its four words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event(pub [u32; 4]);

impl Event {
    pub fn param(&self) -> u64 { self.0[0] as u64 | (self.0[1] as u64) << 32 }
    pub fn status(&self) -> u32 { self.0[2] }
    pub fn control(&self) -> u32 { self.0[3] }
    /// TRB type field, still in place (bits 15:10).
    pub fn trb_type(&self) -> u32 { self.0[3] & (0x3f << 10) }
    pub fn cc(&self) -> u32 { (self.0[2] >> 24) & 0xff }
    pub fn residual(&self) -> u32 { self.0[2] & 0x00ff_ffff }
}

impl EventRing {
    pub fn new(bus: u64, segs: usize) -> EventRing {
        EventRing { bus, segs, dequeue: 0, cycle: 1 }
    }

    pub fn trbs(&self) -> usize { self.segs * TRBS_PER_SEGMENT }

    /// The segment the dequeue pointer is in (`deq_seg->num`).
    pub fn segment(&self) -> usize { self.dequeue / TRBS_PER_SEGMENT }

    /// The dequeue pointer's bus address.
    pub fn dequeue_bus(&self) -> u64 { self.bus + (self.dequeue * 16) as u64 }

    /// `unhandled_event_trb`: the TRB at the dequeue pointer is the
    /// controller's next event; its cycle bit is read first, and only then
    /// the rest (`xhci_handle_event_trb` after its `rmb()`).
    pub fn peek(&self, mem: &impl EventMem) -> Option<Event> {
        let off = self.dequeue * 16;
        let control = mem.r32(off + 12);
        if control & TRB_CYCLE != self.cycle {
            return None;
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        Some(Event([mem.r32(off), mem.r32(off + 4), mem.r32(off + 8), control]))
    }

    /// `inc_deq` for an event ring: the next TRB; after a segment's last
    /// one the next segment, after the ring's last one segment 0 with the
    /// cycle state toggled.
    pub fn inc_deq(&mut self) {
        self.dequeue += 1;
        if self.dequeue >= self.trbs() {
            self.dequeue = 0;
            self.cycle ^= 1;
        }
    }

    /// The next event, the cursor moved past it: for a caller that waits
    /// for one event of its own.
    pub fn take(&mut self, mem: &impl EventMem) -> Option<Event> {
        let e = self.peek(mem)?;
        self.inc_deq();
        Some(e)
    }
}

/// What the driver keeps about the controller's interrupt side.
#[derive(Clone, Copy, Debug, Default)]
pub struct Intr {
    /// IMAN.IP clears itself when the interrupt is sent (`ip_autoclear`:
    /// MSI and MSI-X).
    pub ip_autoclear: bool,
    /// `XHCI_STATE_HALTED` or `XHCI_STATE_DYING`: nothing but the event
    /// handler busy flag is touched any more.
    pub halted: bool,
}

/// `xhci_handshake`: read `off` every microsecond until `(v & mask) ==
/// done` or the register reads all ones (gone), at most `timeout_us`.
pub fn handshake<R: Regs + ?Sized>(r: &mut R, off: u32, mask: u32, done: u32, timeout_us: u64) -> Result<(), Error> {
    let mut waited = 0;
    loop {
        let v = r.r32(off);
        if v == u32::MAX {
            return Err(Error::Gone);
        }
        if v & mask == done {
            return Ok(());
        }
        if waited >= timeout_us {
            return Err(Error::Timeout);
        }
        r.delay_us(1);
        waited += 1;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Timeout,
    /// The controller reads all ones.
    Gone,
}

/// `xhci_update_erst_dequeue`: the dequeue pointer with its segment index,
/// and with `clear_ehb` Event Handler Busy cleared. Without `clear_ehb` a
/// pointer the controller already has is not written (xHCI 4.9.4: writes
/// always advance it).
pub fn update_erst_dequeue<R: Regs + ?Sized>(r: &mut R, l: &Layout, ring: &EventRing, clear_ehb: bool) {
    let cur = r64(r, l.ir0 + ERDP);
    let deq = ring.dequeue_bus();
    if cur & ERST_PTR_MASK == deq & ERST_PTR_MASK && !clear_ehb {
        return;
    }
    let mut v = (ring.segment() as u64 & ERST_DESI_MASK) | (deq & ERST_PTR_MASK);
    if clear_ehb {
        v |= ERST_EHB;
    }
    w64(r, l.ir0 + ERDP, v);
}

/// `xhci_enable_interrupter`: IE set, IP not written, the read flushing.
pub fn enable_interrupter<R: Regs + ?Sized>(r: &mut R, l: &Layout) {
    let iman = (r.r32(l.ir0 + IMAN) & !IMAN_IP) | IMAN_IE;
    r.w32(l.ir0 + IMAN, iman);
    r.r32(l.ir0 + IMAN);
}

/// `xhci_start`: the run bit, then HCHalted must clear.
pub fn start<R: Regs + ?Sized>(r: &mut R, l: &Layout, st: &mut Intr) -> Result<(), Error> {
    let cmd = r.r32(l.op + USBCMD) | CMD_RUN;
    r.w32(l.op + USBCMD, cmd);
    let ret = handshake(r, l.op + USBSTS, STS_HALT, 0, XHCI_MAX_HALT_USEC);
    if ret.is_ok() {
        st.halted = false;
    }
    ret
}

/// `xhci_run_finished`: interrupts on before the controller starts (xHCI
/// 4.2 and 5.5.2), then `xhci_start`. A controller without an interrupt
/// (`by_irq` false) is polled: its interrupter is still enabled, as the
/// event ring needs, but USBCMD.EIE stays off.
pub fn run_finished<R: Regs + ?Sized>(r: &mut R, l: &Layout, st: &mut Intr, by_irq: bool) -> Result<(), Error> {
    if by_irq {
        let cmd = r.r32(l.op + USBCMD) | CMD_EIE;
        r.w32(l.op + USBCMD, cmd);
    }
    enable_interrupter(r, l);
    match start(r, l, st) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = halt(r, l, st);
            Err(e)
        }
    }
}

/// `xhci_quiesce`: interrupts off and, unless halted already, the run bit.
pub fn quiesce<R: Regs + ?Sized>(r: &mut R, l: &Layout) {
    let mut mask = !XHCI_IRQS;
    let halted = r.r32(l.op + USBSTS) & STS_HALT;
    if halted == 0 {
        mask &= !CMD_RUN;
    }
    let cmd = r.r32(l.op + USBCMD) & mask;
    r.w32(l.op + USBCMD, cmd);
}

/// `xhci_halt`: quiesce, wait for HCHalted, the controller is halted.
pub fn halt<R: Regs + ?Sized>(r: &mut R, l: &Layout, st: &mut Intr) -> Result<(), Error> {
    quiesce(r, l);
    let ret = handshake(r, l.op + USBSTS, STS_HALT, STS_HALT, XHCI_MAX_HALT_USEC);
    if ret.is_ok() {
        st.halted = true;
    }
    ret
}

/// `xhci_reset`, on a halted controller: the reset bit, until it clears,
/// then until Controller Not Ready clears. A controller still running is
/// not reset (Linux warns and returns 0).
pub fn reset<R: Regs + ?Sized>(r: &mut R, l: &Layout, timeout_us: u64) -> Result<(), Error> {
    let state = r.r32(l.op + USBSTS);
    if state == u32::MAX {
        return Err(Error::Gone);
    }
    if state & STS_HALT == 0 {
        return Ok(());
    }
    let cmd = r.r32(l.op + USBCMD) | CMD_RESET;
    r.w32(l.op + USBCMD, cmd);
    handshake(r, l.op + USBCMD, CMD_RESET, 0, timeout_us)?;
    handshake(r, l.op + USBSTS, STS_CNR, 0, timeout_us)
}

/// What `xhci_init` programs, from the driver's structures.
#[derive(Clone, Copy, Debug)]
pub struct Init {
    pub max_slots: u32,
    /// Command ring dequeue pointer and cycle state.
    pub cmd_ring: u64,
    pub cmd_cycle: u32,
    pub dcbaa: u64,
    /// The event ring segment table and its entries.
    pub erst: u64,
    pub erst_entries: u32,
    pub event_ring: EventRing,
    /// `XHCI_WRITE_64_HI_LO`: ERSTBA high dword first.
    pub write_64_hi_lo: bool,
}

/// The register part of `xhci_init`: `xhci_hcd_page_size`, then after
/// `xhci_mem_init` `xhci_enable_max_dev_slots`, `xhci_set_cmd_ring_deq`,
/// DCBAAP, `xhci_set_doorbell_ptr`, `xhci_set_dev_notifications` and
/// `xhci_add_interrupter`. Returns PAGESIZE and the doorbell offset.
pub fn init<R: Regs + ?Sized>(r: &mut R, l: &Layout, i: &Init) -> (u32, u32) {
    let page_size = r.r32(l.op + PAGESIZE) & 0xffff;
    let config = (r.r32(l.op + CONFIG) & !HCS_SLOTS_MASK) | i.max_slots;
    r.w32(l.op + CONFIG, config);
    let mut crcr = r64(r, l.op + CRCR) & !CMD_RING_PTR_MASK;
    crcr |= i.cmd_ring & CMD_RING_PTR_MASK;
    crcr = (crcr & !CMD_RING_CYCLE) | i.cmd_cycle as u64;
    w64(r, l.op + CRCR, crcr);
    w64(r, l.op + DCBAAP, i.dcbaa);
    let dboff = r.r32(DBOFF) & !0x3;
    let dn = (r.r32(l.op + DNCTRL) & !DEV_NOTE_MASK) | DEV_NOTE_FWAKE;
    r.w32(l.op + DNCTRL, dn);
    add_interrupter(r, l, i);
    (page_size, dboff)
}

/// `xhci_add_interrupter` for interrupter 0: ERSTSZ, ERSTBA (high dword
/// first with `XHCI_WRITE_64_HI_LO`), then `xhci_set_hc_event_deq`.
pub fn add_interrupter<R: Regs + ?Sized>(r: &mut R, l: &Layout, i: &Init) {
    let size = (r.r32(l.ir0 + ERSTSZ) & !ERST_SIZE_MASK) | i.erst_entries;
    r.w32(l.ir0 + ERSTSZ, size);
    let mut base = r64(r, l.ir0 + ERSTBA) & !ERST_BASE_ADDRESS_MASK;
    base |= i.erst & ERST_BASE_ADDRESS_MASK;
    if i.write_64_hi_lo {
        r.w32(l.ir0 + ERSTBA + 4, (base >> 32) as u32);
        r.w32(l.ir0 + ERSTBA, base as u32);
    } else {
        w64(r, l.ir0 + ERSTBA, base);
    }
    set_hc_event_deq(r, l, &i.event_ring);
}

/// `xhci_set_hc_event_deq`: the dequeue pointer, EHB left alone (not
/// written as 1).
pub fn set_hc_event_deq<R: Regs + ?Sized>(r: &mut R, l: &Layout, ring: &EventRing) {
    w64(r, l.ir0 + ERDP, ring.dequeue_bus() & ERST_PTR_MASK);
}

/// `xhci_set_interrupter_moderation`: IMODI in 250 ns steps.
pub fn set_interrupter_moderation<R: Regs + ?Sized>(r: &mut R, l: &Layout, interval_ns: u32) {
    let interval = (interval_ns / 250).min(IMODI_MASK);
    let imod = (r.r32(l.ir0 + IMOD) & !IMODI_MASK) | interval;
    r.w32(l.ir0 + IMOD, imod);
}

/// The register part of `xhci_run` before `xhci_run_finished`: the event
/// ring dequeue pointer read for the trace, then the interrupt moderation.
pub fn run<R: Regs + ?Sized>(r: &mut R, l: &Layout) {
    let _ = r64(r, l.ir0 + ERDP);
    set_interrupter_moderation(r, l, IMOD_INTERVAL_NS);
}

/// `xhci_clear_interrupt_pending`, the read flushing the write.
pub fn clear_interrupt_pending<R: Regs + ?Sized>(r: &mut R, l: &Layout, st: &Intr) {
    if !st.ip_autoclear {
        let iman = r.r32(l.ir0 + IMAN) | IMAN_IP;
        r.w32(l.ir0 + IMAN, iman);
        r.r32(l.ir0 + IMAN);
    }
}

/// `xhci_handle_events`: every event the controller has posted, each
/// handed to `on_event` before the cursor moves past it; the dequeue
/// pointer is written after every half segment, and at the end with Event
/// Handler Busy cleared. A halted controller has only EHB cleared.
pub fn handle_events<R: Regs + ?Sized, M: EventMem>(r: &mut R, l: &Layout, st: &Intr, ring: &mut EventRing,
    mem: &M, on_event: impl FnMut(&Event))
{
    clear_interrupt_pending(r, l, st);
    if st.halted {
        let v = r64(r, l.ir0 + ERDP);
        w64(r, l.ir0 + ERDP, v | ERST_EHB);
        return;
    }
    take_events(r, l, ring, mem, on_event);
}

/// The loop of `xhci_handle_events`: every posted event to `on_event`, the
/// dequeue pointer written after every half segment and at the end with
/// Event Handler Busy cleared. Also for a caller that drains the ring
/// outside the interrupt (polling).
pub fn take_events<R: Regs + ?Sized, M: EventMem>(r: &mut R, l: &Layout, ring: &mut EventRing,
    mem: &M, mut on_event: impl FnMut(&Event))
{
    let mut event_loop = 0;
    while let Some(e) = ring.peek(mem) {
        on_event(&e);
        if event_loop > TRBS_PER_SEGMENT / 2 {
            update_erst_dequeue(r, l, ring, false);
            event_loop = 0;
        } else {
            event_loop += 1;
        }
        ring.inc_deq();
    }
    update_erst_dequeue(r, l, ring, true);
}

/// What `irq` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Irq {
    /// The controller did not raise it (`IRQ_NONE`).
    None,
    Handled,
    /// The controller reads all ones (`xhci_hc_died`).
    Died,
    /// Host Controller Error: halted.
    HostControllerError,
    /// Host System Error: halted.
    HostSystemError,
}

/// `xhci_irq` for interrupter 0: whether the controller raised the
/// interrupt, its errors, the status written back (write 1 to clear), then
/// `handle_events`.
pub fn irq<R: Regs + ?Sized, M: EventMem>(r: &mut R, l: &Layout, st: &mut Intr, ring: &mut EventRing,
    mem: &M, on_event: impl FnMut(&Event)) -> Irq
{
    let status = r.r32(l.op + USBSTS);
    if status == u32::MAX {
        st.halted = true;
        return Irq::Died;
    }
    if status & STS_EINT == 0 {
        return Irq::None;
    }
    if status & STS_HCE != 0 {
        let _ = halt(r, l, st);
        return Irq::HostControllerError;
    }
    if status & STS_FATAL != 0 {
        let _ = halt(r, l, st);
        return Irq::HostSystemError;
    }
    r.w32(l.op + USBSTS, status | STS_EINT);
    handle_events(r, l, st, ring, mem, on_event);
    Irq::Handled
}
