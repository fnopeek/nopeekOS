//! virtio-console-pci with two named ports: the control channel between the
//! host and the guest's PID 1, and a port reserved for the window protocol.
//!
//! Modern virtio (1.0+), vendor 0x1AF4, device 0x1043, with
//! VIRTIO_CONSOLE_F_MULTIPORT, so the guest sees plain character devices
//! (`/dev/vportNpM`, named through sysfs) rather than ttys. Queues, per the
//! virtio spec §5.3.2: 0/1 port 0 receive/transmit, 2/3 control
//! receive/transmit, 4/5 port 1 receive/transmit.
//!
//! Port 0 ("npk.control") carries frames `u16 len | u8 type | payload`,
//! little-endian, at most `MAX_FRAME` bytes after the length. Unknown types
//! are dropped; a frame that cannot be one (zero or oversized length) ends
//! the VM, since only a misbehaving guest sends it. Port 1 ("npk.windows")
//! is announced and opened; what the guest writes there is consumed and
//! dropped until the window protocol exists.

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::guest_mem::GuestMem;

const VIRTIO_VENDOR: u32 = 0x1AF4;
const VIRTIO_CONSOLE_DEVICE: u32 = 0x1043;

pub const BAR0_BASE: u64 = 0xFE01_C000;
pub const BAR0_SIZE: u64 = 0x4000;
/// 8259 line: the legacy secondary-IDE line, free here.
pub const IRQ_LINE: u8 = 15;
const BAR0_SIZE_MASK_LO: u32 = !((BAR0_SIZE as u32) - 1) | 0b0100;

const CAP_COMMON_OFF: u8 = 0x40;
const CAP_NOTIFY_OFF: u8 = 0x54;
const CAP_ISR_OFF:    u8 = 0x68;
const CAP_DEVICE_OFF: u8 = 0x78;

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG:    u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

const COMMON_OFF:  u32 = 0x0000;
const COMMON_LEN:  u32 = 0x0100;
const NOTIFY_OFF:  u32 = 0x0100;
const NOTIFY_LEN:  u32 = 0x0100;
const ISR_OFF:     u32 = 0x0200;
const ISR_LEN:     u32 = 0x0100;
const DEVICE_OFF:  u32 = 0x0300;
const DEVICE_LEN:  u32 = 0x0100;

const NOTIFY_OFF_MULTIPLIER: u32 = 4;

const CC_DEVICE_FEATURE_SELECT:  u32 = 0x00;
const CC_DEVICE_FEATURE:         u32 = 0x04;
const CC_DRIVER_FEATURE_SELECT:  u32 = 0x08;
const CC_DRIVER_FEATURE:         u32 = 0x0C;
const CC_MSIX_CONFIG:            u32 = 0x10;
const CC_NUM_QUEUES:             u32 = 0x12;
const CC_DEVICE_STATUS:          u32 = 0x14;
const CC_CONFIG_GENERATION:      u32 = 0x15;
const CC_QUEUE_SELECT:           u32 = 0x16;
const CC_QUEUE_SIZE:             u32 = 0x18;
const CC_QUEUE_MSIX_VECTOR:      u32 = 0x1A;
const CC_QUEUE_ENABLE:           u32 = 0x1C;
const CC_QUEUE_NOTIFY_OFF:       u32 = 0x1E;
const CC_QUEUE_DESC_LO:          u32 = 0x20;
const CC_QUEUE_DESC_HI:          u32 = 0x24;
const CC_QUEUE_DRIVER_LO:        u32 = 0x28;
const CC_QUEUE_DRIVER_HI:        u32 = 0x2C;
const CC_QUEUE_DEVICE_LO:        u32 = 0x30;
const CC_QUEUE_DEVICE_HI:        u32 = 0x34;

const VIRTIO_CONSOLE_F_MULTIPORT: u32 = 1 << 1;

const NUM_PORTS: usize = 2;
const NUM_QUEUES: u16 = 2 + 2 * NUM_PORTS as u16;
const MAX_QUEUE_SIZE: u16 = 64;

const Q_CTRL_RX: u16 = 2;
const Q_CTRL_TX: u16 = 3;

// virtio_console_control.event (virtio spec §5.3.6.2).
const VIRTIO_CONSOLE_DEVICE_READY: u16 = 0;
const VIRTIO_CONSOLE_DEVICE_ADD:   u16 = 1;
const VIRTIO_CONSOLE_PORT_READY:   u16 = 3;
const VIRTIO_CONSOLE_PORT_OPEN:    u16 = 6;
const VIRTIO_CONSOLE_PORT_NAME:    u16 = 7;

const PORT_NAMES: [&[u8]; NUM_PORTS] = [b"npk.control", b"npk.windows"];
const PORT_CONTROL: usize = 0;

/// Largest frame (type byte + payload) either side may send.
pub const MAX_FRAME: usize = 4096;
/// Bytes buffered per direction and port; a guest that never reads or a
/// stream of garbage cannot grow the host past this.
const MAX_BUFFERED: usize = 4 * MAX_FRAME;
/// Control messages waiting for the driver. A guest repeating DEVICE_READY
/// or PORT_READY gets answers only up to this many; the rest are dropped.
const MAX_CTRL_OUT: usize = 16;

// Control-port message types.
/// Host -> guest: end the app cleanly, sync, power off.
pub const MSG_QUIT: u8 = 0x01;
/// Guest -> host: PID 1's control process is listening.
pub const MSG_READY: u8 = 0x81;

/// The guest's control process has said READY on this launch.
static CONTROL_READY: AtomicBool = AtomicBool::new(false);
/// The guest broke the frame format; the run loop ends the VM.
static VIOLATION: AtomicBool = AtomicBool::new(false);
/// Frames for the control port, queued by any core (`send`), delivered by
/// `pump` on the vCPU that owns the device.
static OUTBOX: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());

/// Whether a guest is listening on the control port.
pub fn control_ready() -> bool { CONTROL_READY.load(Ordering::Acquire) }

/// Whether the guest has violated the frame format (the VM must end).
pub fn violated() -> bool { VIOLATION.load(Ordering::Acquire) }

/// Queue one control frame for the guest. False if the channel is not up or
/// the payload is too large.
pub fn send(msg_type: u8, payload: &[u8]) -> bool {
    if !control_ready() || payload.len() + 1 > MAX_FRAME {
        return false;
    }
    let len = (payload.len() + 1) as u16;
    {
        let mut out = OUTBOX.lock();
        if out.len() + 2 + len as usize > MAX_BUFFERED {
            return false;
        }
        out.extend(len.to_le_bytes());
        out.push_back(msg_type);
        out.extend(payload.iter().copied());
    }
    crate::microvm::cpu::kick_bsp_net_irq();
    true
}

/// Forget the previous launch's channel state.
pub fn reset() {
    CONTROL_READY.store(false, Ordering::Release);
    VIOLATION.store(false, Ordering::Release);
    OUTBOX.lock().clear();
}

#[derive(Default, Clone, Copy)]
struct VirtQueue {
    size: u16,
    msix_vec: u16,
    enable: u16,
    desc_lo: u32, desc_hi: u32,
    driver_lo: u32, driver_hi: u32,
    device_lo: u32, device_hi: u32,
    last_avail_idx: u16,
    used_idx: u16,
}

impl VirtQueue {
    const fn fresh() -> Self {
        VirtQueue {
            size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
            desc_lo: 0, desc_hi: 0, driver_lo: 0, driver_hi: 0,
            device_lo: 0, device_hi: 0, last_avail_idx: 0, used_idx: 0,
        }
    }
    fn desc_gpa(&self)   -> u64 { ((self.desc_hi   as u64) << 32) | self.desc_lo   as u64 }
    fn driver_gpa(&self) -> u64 { ((self.driver_hi as u64) << 32) | self.driver_lo as u64 }
    fn device_gpa(&self) -> u64 { ((self.device_hi as u64) << 32) | self.device_lo as u64 }
}

pub struct VirtioConsole {
    bar0_lo: u32,
    bar0_hi: u32,
    bar0_lo_sized: bool,
    bar0_hi_sized: bool,

    device_feature_select: u32,
    driver_feature_select: u32,
    driver_features:       [u32; 2],
    msix_config:           u16,
    device_status:         u8,
    config_generation:     u8,
    queue_select:          u16,

    queues: [VirtQueue; NUM_QUEUES as usize],

    isr: u8,
    pending_kick_queue: Option<u16>,

    /// Control messages for the guest's driver, one per receive buffer.
    ctrl_out: VecDeque<Vec<u8>>,
    /// Bytes the guest wrote on the control port, not yet a whole frame.
    control_in: Vec<u8>,
}

impl VirtioConsole {
    pub const fn new() -> Self {
        Self {
            bar0_lo: BAR0_BASE as u32,
            bar0_hi: (BAR0_BASE >> 32) as u32,
            bar0_lo_sized: false,
            bar0_hi_sized: false,
            device_feature_select: 0,
            driver_feature_select: 0,
            driver_features: [0; 2],
            msix_config: 0xFFFF,
            device_status: 0,
            config_generation: 0,
            queue_select: 0,
            queues: [VirtQueue::fresh(); NUM_QUEUES as usize],
            isr: 0,
            pending_kick_queue: None,
            ctrl_out: VecDeque::new(),
            control_in: Vec::new(),
        }
    }

    pub fn bar0_base(&self) -> u64 {
        ((self.bar0_hi as u64) << 32) | (self.bar0_lo as u64 & !0x0Fu64)
    }

    pub fn bar0_in_range(&self, gpa: u64) -> bool {
        let base = self.bar0_base();
        gpa >= base && gpa < base + BAR0_SIZE
    }

    pub fn irq_line(&self) -> u8 { IRQ_LINE }

    pub fn take_pending_kick(&mut self) -> Option<u16> {
        self.pending_kick_queue.take()
    }

    /// A queue notify: consume what the guest sent on a transmit queue, and
    /// deliver whatever is pending into receive buffers it just posted.
    pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {
        let mut any = false;
        match queue_idx {
            Q_CTRL_TX => any |= self.consume_ctrl(mem),
            1 => any |= self.consume_port(PORT_CONTROL, mem),
            q if q >= 4 && q % 2 == 1 => any |= self.consume_port(((q - 2) / 2) as usize, mem),
            _ => {}
        }
        any |= self.pump(mem);
        any
    }

    /// Deliver pending control messages and control-port frames into the
    /// guest's receive buffers. True if anything was delivered (the caller
    /// raises the IRQ). Called on notify and on the timer tick.
    pub fn pump(&mut self, mem: &GuestMem) -> bool {
        let mut any = false;
        while !self.ctrl_out.is_empty() {
            let msg = self.ctrl_out.front().cloned().unwrap_or_default();
            if !self.deliver(Q_CTRL_RX, &msg, mem) {
                break;
            }
            self.ctrl_out.pop_front();
            any = true;
        }
        loop {
            let chunk: Vec<u8> = {
                let out = OUTBOX.lock();
                if out.is_empty() { break; }
                out.iter().take(MAX_FRAME + 2).copied().collect()
            };
            let Some(n) = self.deliver_partial(0, &chunk, mem) else { break };
            let mut out = OUTBOX.lock();
            for _ in 0..n { out.pop_front(); }
            any = true;
        }
        if any { self.isr |= 1; }
        any
    }

    /// Write all of `data` into one receive buffer of queue `qi`. False if no
    /// buffer is posted or it is too small.
    fn deliver(&mut self, qi: u16, data: &[u8], mem: &GuestMem) -> bool {
        matches!(self.deliver_partial(qi, data, mem), Some(n) if n == data.len())
    }

    /// Write as much of `data` as fits into one receive buffer of queue `qi`
    /// (one descriptor chain); the number of bytes written, or None if no
    /// buffer is posted.
    fn deliver_partial(&mut self, qi: u16, data: &[u8], mem: &GuestMem) -> Option<usize> {
        use super::virtqueue::{avail_idx, avail_ring, read_desc, chain_next, used_push, VRING_DESC_F_WRITE};
        let q = self.queues.get_mut(qi as usize)?;
        if q.enable == 0 || q.size == 0 { return None; }
        let top = avail_idx(mem, q.driver_gpa())?;
        if top == q.last_avail_idx { return None; }
        let head = avail_ring(mem, q.driver_gpa(), q.size, q.last_avail_idx)?;
        let mut idx = head;
        let mut hops = 0u16;
        let mut written = 0usize;
        loop {
            let Some(d) = read_desc(mem, q.desc_gpa(), idx, q.size) else { break };
            if d.flags & VRING_DESC_F_WRITE != 0 && written < data.len() {
                let n = (d.len as usize).min(data.len() - written);
                if !mem.write_bytes(d.addr, &data[written..written + n]) { break; }
                written += n;
            }
            match chain_next(&d, &mut hops, q.size) { Some(n) => idx = n, None => break }
        }
        used_push(mem, q.device_gpa(), q.size, &mut q.used_idx, head, written as u32);
        q.last_avail_idx = q.last_avail_idx.wrapping_add(1);
        Some(written)
    }

    /// Gather every pending buffer of transmit queue `qi`; each buffer is
    /// passed to `f` and acknowledged. True if any was consumed.
    fn consume<F: FnMut(&mut Self, &[u8])>(&mut self, qi: u16, mem: &GuestMem, mut f: F) -> bool {
        use super::virtqueue::{avail_idx, avail_pending, avail_ring, read_desc, chain_next, used_push, VRING_DESC_F_WRITE};
        let mut any = false;
        loop {
            let (head, data) = {
                let Some(q) = self.queues.get_mut(qi as usize) else { return any };
                if q.enable == 0 || q.size == 0 { return any; }
                let Some(top) = avail_idx(mem, q.driver_gpa()) else { return any };
                if avail_pending(q.last_avail_idx, top, q.size) == 0 { return any; }
                let Some(head) = avail_ring(mem, q.driver_gpa(), q.size, q.last_avail_idx) else { return any };
                let mut data = Vec::new();
                let mut idx = head;
                let mut hops = 0u16;
                loop {
                    let Some(d) = read_desc(mem, q.desc_gpa(), idx, q.size) else { break };
                    if d.flags & VRING_DESC_F_WRITE == 0 {
                        let n = (d.len as usize).min(MAX_BUFFERED.saturating_sub(data.len()));
                        let at = data.len();
                        data.resize(at + n, 0);
                        if !mem.read_bytes(d.addr, &mut data[at..]) { data.truncate(at); break; }
                    }
                    match chain_next(&d, &mut hops, q.size) { Some(n) => idx = n, None => break }
                }
                used_push(mem, q.device_gpa(), q.size, &mut q.used_idx, head, 0);
                q.last_avail_idx = q.last_avail_idx.wrapping_add(1);
                (head, data)
            };
            let _ = head;
            f(self, &data);
            any = true;
        }
    }

    fn queue_ctrl(&mut self, msg: Vec<u8>) {
        if self.ctrl_out.len() < MAX_CTRL_OUT {
            self.ctrl_out.push_back(msg);
        }
    }

    fn consume_ctrl(&mut self, mem: &GuestMem) -> bool {
        let any = self.consume(Q_CTRL_TX, mem, |dev, msg| {
            if msg.len() < 8 { return; }
            let id = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]);
            let event = u16::from_le_bytes([msg[4], msg[5]]);
            let value = u16::from_le_bytes([msg[6], msg[7]]);
            match event {
                VIRTIO_CONSOLE_DEVICE_READY if value == 1 => {
                    for port in 0..NUM_PORTS as u32 {
                        dev.queue_ctrl(ctrl_msg(port, VIRTIO_CONSOLE_DEVICE_ADD, 1, &[]));
                    }
                }
                VIRTIO_CONSOLE_PORT_READY if value == 1 && (id as usize) < NUM_PORTS => {
                    dev.queue_ctrl(ctrl_msg(id, VIRTIO_CONSOLE_PORT_NAME, 1, PORT_NAMES[id as usize]));
                    dev.queue_ctrl(ctrl_msg(id, VIRTIO_CONSOLE_PORT_OPEN, 1, &[]));
                }
                _ => {}
            }
        });
        if any { self.isr |= 1; }
        any
    }

    fn consume_port(&mut self, port: usize, mem: &GuestMem) -> bool {
        if port >= NUM_PORTS { return false; }
        let qi = (if port == 0 { 1 } else { 2 * port + 3 }) as u16;
        let any = self.consume(qi, mem, |dev, data| {
            if port == PORT_CONTROL {
                dev.control_in.extend_from_slice(data);
                dev.parse_control();
            }
        });
        if any { self.isr |= 1; }
        any
    }

    /// Take whole frames off the control-port input.
    fn parse_control(&mut self) {
        loop {
            if self.control_in.len() < 2 { return; }
            let len = u16::from_le_bytes([self.control_in[0], self.control_in[1]]) as usize;
            if len == 0 || len > MAX_FRAME || self.control_in.len() > MAX_BUFFERED {
                crate::kprintln!("[microvm] control channel: malformed frame (len {}) — ending the guest", len);
                VIOLATION.store(true, Ordering::Release);
                self.control_in.clear();
                return;
            }
            if self.control_in.len() < 2 + len { return; }
            let msg_type = self.control_in[2];
            match msg_type {
                MSG_READY => {
                    if !CONTROL_READY.swap(true, Ordering::AcqRel) {
                        crate::kprintln!("[microvm] control channel up");
                    }
                }
                _ => {}
            }
            self.control_in.drain(..2 + len);
        }
    }

    pub fn pci_read_dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (VIRTIO_CONSOLE_DEVICE << 16) | VIRTIO_VENDOR,
            0x04 => (0x0010 << 16) | 0x0007,
            // class 07_80_00 (communication controller, other) | revision 0x01
            0x08 => (0x07_80_00 << 8) | 0x01,
            0x0C => 0,
            0x10 => if self.bar0_lo_sized { BAR0_SIZE_MASK_LO } else { self.bar0_lo },
            0x14 => if self.bar0_hi_sized { 0xFFFF_FFFF } else { self.bar0_hi },
            0x18..=0x24 => 0,
            0x28 => 0,
            0x2C => (0x0003 << 16) | 0x1AF4,
            0x30 => 0,
            0x34 => CAP_COMMON_OFF as u32,
            0x38 => 0,
            0x3C => (0x01 << 8) | IRQ_LINE as u32, // INTA

            0x40 => 0x09 | ((CAP_NOTIFY_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_COMMON_CFG as u32) << 24),
            0x44 => 0,
            0x48 => COMMON_OFF,
            0x4C => COMMON_LEN,
            0x50 => 0,

            0x54 => 0x09 | ((CAP_ISR_OFF as u32) << 8) | (20 << 16) | ((VIRTIO_PCI_CAP_NOTIFY_CFG as u32) << 24),
            0x58 => 0,
            0x5C => NOTIFY_OFF,
            0x60 => NOTIFY_LEN,
            0x64 => NOTIFY_OFF_MULTIPLIER,

            0x68 => 0x09 | ((CAP_DEVICE_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_ISR_CFG as u32) << 24),
            0x6C => 0,
            0x70 => ISR_OFF,
            0x74 => ISR_LEN,

            0x78 => 0x09 | (0 << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_DEVICE_CFG as u32) << 24),
            0x7C => 0,
            0x80 => DEVICE_OFF,
            0x84 => DEVICE_LEN,

            _ => 0,
        }
    }

    pub fn pci_write_dword(&mut self, reg: u8, value: u32) {
        match reg {
            0x10 => {
                if value == 0xFFFF_FFFF { self.bar0_lo_sized = true; }
                else {
                    self.bar0_lo = value & !0x0F | (BAR0_BASE as u32 & 0x0F);
                    self.bar0_lo_sized = false;
                }
            }
            0x14 => {
                if value == 0xFFFF_FFFF { self.bar0_hi_sized = true; }
                else {
                    self.bar0_hi = value;
                    self.bar0_hi_sized = false;
                }
            }
            _ => {}
        }
    }

    pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {
        if off >= COMMON_OFF && off < COMMON_OFF + COMMON_LEN {
            self.common_read(off - COMMON_OFF, width)
        } else if off >= ISR_OFF && off < ISR_OFF + ISR_LEN {
            let v = self.isr as u64;
            self.isr = 0;
            v & width_mask(width)
        } else if off >= DEVICE_OFF && off < DEVICE_OFF + DEVICE_LEN {
            self.device_read(off - DEVICE_OFF, width)
        } else {
            0
        }
    }

    pub fn mmio_write(&mut self, off: u32, width: u8, value: u64) {
        if off >= COMMON_OFF && off < COMMON_OFF + COMMON_LEN {
            self.common_write(off - COMMON_OFF, width, value);
        } else if off >= NOTIFY_OFF && off < NOTIFY_OFF + NOTIFY_LEN {
            let _ = (value, width);
            self.pending_kick_queue = Some(((off - NOTIFY_OFF) / NOTIFY_OFF_MULTIPLIER) as u16);
        }
    }

    fn common_read(&self, off: u32, width: u8) -> u64 {
        let v: u64 = match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select as u64,
            CC_DEVICE_FEATURE => match self.device_feature_select {
                0 => VIRTIO_CONSOLE_F_MULTIPORT as u64,
                1 => 1, // VIRTIO_F_VERSION_1 (bit 32)
                _ => 0,
            },
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select as u64,
            CC_DRIVER_FEATURE => self.driver_features[(self.driver_feature_select & 1) as usize] as u64,
            CC_MSIX_CONFIG => self.msix_config as u64,
            CC_NUM_QUEUES => NUM_QUEUES as u64,
            CC_DEVICE_STATUS => self.device_status as u64,
            CC_CONFIG_GENERATION => self.config_generation as u64,
            CC_QUEUE_SELECT => self.queue_select as u64,
            CC_QUEUE_SIZE => self.q().size as u64,
            CC_QUEUE_MSIX_VECTOR => self.q().msix_vec as u64,
            CC_QUEUE_ENABLE => self.q().enable as u64,
            CC_QUEUE_NOTIFY_OFF => self.queue_select as u64,
            CC_QUEUE_DESC_LO => self.q().desc_lo as u64,
            CC_QUEUE_DESC_HI => self.q().desc_hi as u64,
            CC_QUEUE_DRIVER_LO => self.q().driver_lo as u64,
            CC_QUEUE_DRIVER_HI => self.q().driver_hi as u64,
            CC_QUEUE_DEVICE_LO => self.q().device_lo as u64,
            CC_QUEUE_DEVICE_HI => self.q().device_hi as u64,
            _ => 0,
        };
        v & width_mask(width)
    }

    fn common_write(&mut self, off: u32, width: u8, raw: u64) {
        let val = raw & width_mask(width);
        match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select = val as u32,
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select = val as u32,
            CC_DRIVER_FEATURE => self.driver_features[(self.driver_feature_select & 1) as usize] = val as u32,
            CC_MSIX_CONFIG => self.msix_config = val as u16,
            CC_DEVICE_STATUS => {
                self.device_status = val as u8;
                if self.device_status == 0 {
                    self.queues = [VirtQueue::fresh(); NUM_QUEUES as usize];
                    self.driver_features = [0; 2];
                    self.driver_feature_select = 0;
                    self.device_feature_select = 0;
                    self.queue_select = 0;
                    self.ctrl_out.clear();
                    self.control_in.clear();
                    self.config_generation = self.config_generation.wrapping_add(1);
                }
            }
            CC_QUEUE_SELECT => self.queue_select = val as u16,
            CC_QUEUE_SIZE => self.q_mut().size = (val as u16).min(MAX_QUEUE_SIZE),
            CC_QUEUE_MSIX_VECTOR => self.q_mut().msix_vec = val as u16,
            CC_QUEUE_ENABLE => self.q_mut().enable = val as u16,
            CC_QUEUE_DESC_LO => self.q_mut().desc_lo = val as u32,
            CC_QUEUE_DESC_HI => self.q_mut().desc_hi = val as u32,
            CC_QUEUE_DRIVER_LO => self.q_mut().driver_lo = val as u32,
            CC_QUEUE_DRIVER_HI => self.q_mut().driver_hi = val as u32,
            CC_QUEUE_DEVICE_LO => self.q_mut().device_lo = val as u32,
            CC_QUEUE_DEVICE_HI => self.q_mut().device_hi = val as u32,
            _ => {}
        }
    }

    /// virtio_console_config: cols u16 @0, rows u16 @2, max_nr_ports u32 @4,
    /// emerg_wr u32 @8.
    fn device_read(&self, off: u32, width: u8) -> u64 {
        let mut cfg = [0u8; 12];
        cfg[4..8].copy_from_slice(&(NUM_PORTS as u32).to_le_bytes());
        let mut v = 0u64;
        for i in 0..width as u32 {
            let p = (off + i) as usize;
            let byte = if p < cfg.len() { cfg[p] } else { 0 };
            v |= (byte as u64) << (i * 8);
        }
        v & width_mask(width)
    }

    fn q(&self) -> &VirtQueue {
        &self.queues[self.queue_select as usize % self.queues.len()]
    }

    fn q_mut(&mut self) -> &mut VirtQueue {
        let idx = self.queue_select as usize % self.queues.len();
        &mut self.queues[idx]
    }
}

/// One virtio_console_control message (+ optional trailing bytes, the name
/// for PORT_NAME).
fn ctrl_msg(id: u32, event: u16, value: u16, tail: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(8 + tail.len());
    m.extend_from_slice(&id.to_le_bytes());
    m.extend_from_slice(&event.to_le_bytes());
    m.extend_from_slice(&value.to_le_bytes());
    m.extend_from_slice(tail);
    m
}

const fn width_mask(width: u8) -> u64 {
    match width {
        1 => 0xFF,
        2 => 0xFFFF,
        4 => 0xFFFF_FFFF,
        _ => 0xFFFF_FFFF_FFFF_FFFF,
    }
}
