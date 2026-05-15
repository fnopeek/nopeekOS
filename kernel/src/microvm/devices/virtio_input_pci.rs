//! virtio-input-pci device emulation (Phase 12.4c).
//!
//! Modern virtio (1.0+) input device — vendor 0x1AF4, device 0x1052
//! (= 0x1040 + 18, virtio-spec device-id 18). Two virtqueues:
//!   q0 = eventq  (host fills with input_event structs, driver reads)
//!   q1 = statusq (driver writes LED state / etc., host acks)
//!
//! Device-cfg layout (virtio 1.2 §5.8.5):
//!
//! ```text
//!   off  0  u8   select       (write-only, picks a property)
//!   off  1  u8   subsel       (write-only, sub-select within property)
//!   off  2  u8   size         (read-only, length of u valid for this select)
//!   off  3  u8[5] reserved
//!   off  8  u8[128] u         (read-only, content depends on select)
//! ```
//!
//! Smoke-test scope for 12.4c: device appears in PCI scan, Linux probes
//! it, /dev/input/event0 falls out — but we never inject any events.
//! Real event injection (Shade compositor → guest) comes in 12.4d.

#![allow(dead_code)]

extern crate alloc;
use crate::kprintln;

const VIRTIO_VENDOR: u32 = 0x1AF4;
const VIRTIO_INPUT_DEVICE: u32 = 0x1052;

pub const BAR0_BASE: u64 = 0xFE00_C000;
pub const BAR0_SIZE: u64 = 0x4000;

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

// Common Cfg register offsets (virtio 1.2 §4.1.4.3)
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

// virtio-input device-cfg selectors (virtio 1.2 §5.8.4)
const VIRTIO_INPUT_CFG_UNSET:     u8 = 0x00;
const VIRTIO_INPUT_CFG_ID_NAME:   u8 = 0x01;
const VIRTIO_INPUT_CFG_ID_SERIAL: u8 = 0x02;
const VIRTIO_INPUT_CFG_ID_DEVIDS: u8 = 0x03;
const VIRTIO_INPUT_CFG_PROP_BITS: u8 = 0x10;
const VIRTIO_INPUT_CFG_EV_BITS:   u8 = 0x11;
const VIRTIO_INPUT_CFG_ABS_INFO:  u8 = 0x12;

// Linux input event-type codes we care about (linux/input-event-codes.h)
const EV_SYN: u8 = 0x00;
const EV_KEY: u8 = 0x01;
// const EV_REL: u8 = 0x02;  // mouse: future
// const EV_ABS: u8 = 0x03;
const EV_MSC: u8 = 0x04;
const EV_REP: u8 = 0x14;

const KEY_ESC: u16 = 1;
const KEY_A:   u16 = 30;
const SYN_REPORT: u16 = 0;

const NUM_QUEUES: u16 = 2;
const MAX_QUEUE_SIZE: u16 = 256;

/// Pending host→guest input events (type, code, value), pushed by the
/// Shade Surface-window branch (Core 0) and drained into the eventq
/// by `drain_injected` on the timer tick. Bounded — drop oldest on
/// overflow (a wedged guest must not OOM the host). Single global:
/// one focused Surface VM at a time (forward-compat #2 — keyed
/// generalisation later, with the registry).
static INPUT_Q: spin::Mutex<alloc::collections::VecDeque<(u16, u16, u32)>> =
    spin::Mutex::new(alloc::collections::VecDeque::new());

const INPUT_Q_CAP: usize = 256;

/// Queue one evdev event for delivery to the guest's virtio-input
/// eventq. Caller (Shade) pushes EV_KEY then EV_SYN(SYN_REPORT) so
/// the guest's evdev dispatches it.
pub fn push_input_event(etype: u16, code: u16, value: u32) {
    let mut q = INPUT_Q.lock();
    if q.len() >= INPUT_Q_CAP {
        q.pop_front();
    }
    q.push_back((etype, code, value));
    let n = PUSH_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n < 8 {
        kprintln!("[virtio-input] push #{} type={} code={} val={} (qlen={})",
                  n + 1, etype, code, value, q.len());
    }
}

static PUSH_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static DRAIN_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

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
    fn desc_gpa(&self)   -> u64 { ((self.desc_hi   as u64) << 32) | self.desc_lo   as u64 }
    fn driver_gpa(&self) -> u64 { ((self.driver_hi as u64) << 32) | self.driver_lo as u64 }
    fn device_gpa(&self) -> u64 { ((self.device_hi as u64) << 32) | self.device_lo as u64 }
}

pub struct VirtioInput {
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

    /// Driver writes `select` here; we recompute (size, data) so any
    /// subsequent read of the size/data fields returns a coherent
    /// snapshot. virtio 1.2 §5.8.5 explicitly requires this latching.
    cfg_select: u8,
    cfg_subsel: u8,
    /// Pre-computed response for the current (select, subsel).
    /// `cfg_data[0..cfg_size]` is the meaningful prefix.
    cfg_size: u8,
    cfg_data: [u8; 128],
}

impl VirtioInput {
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
            queues: [VirtQueue {
                size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
                desc_lo: 0, desc_hi: 0,
                driver_lo: 0, driver_hi: 0,
                device_lo: 0, device_hi: 0,
                last_avail_idx: 0, used_idx: 0,
            }; NUM_QUEUES as usize],
            isr: 0,
            pending_kick_queue: None,
            cfg_select: VIRTIO_INPUT_CFG_UNSET,
            cfg_subsel: 0,
            cfg_size: 0,
            cfg_data: [0; 128],
        }
    }

    pub fn bar0_base(&self) -> u64 {
        ((self.bar0_hi as u64) << 32) | (self.bar0_lo as u64 & !0x0Fu64)
    }

    pub fn bar0_in_range(&self, gpa: u64) -> bool {
        let base = self.bar0_base();
        gpa >= base && gpa < base + BAR0_SIZE
    }

    pub fn take_pending_kick(&mut self) -> Option<u16> {
        self.pending_kick_queue.take()
    }

    /// Process queue notify. q0 = eventq (driver fills with empty
    /// buffers, host will populate when events arrive — for 12.4c we
    /// have no events to inject, just leave buffers queued).
    /// q1 = statusq (driver writes LED state, we ack via used-ring).
    pub fn service_queues(&mut self, queue_idx: u16, host_base: u64) -> bool {
        match queue_idx {
            1 => self.service_statusq(host_base),
            _ => false,
        }
    }

    /// Drain pending host events into the eventq (q0). The guest's
    /// virtio-input driver keeps the eventq stocked with empty
    /// writable buffers; we write one `virtio_input_event`
    /// {u16 type; u16 code; u32 value} (8 bytes, LE) per buffer and
    /// publish it on the used-ring. Returns true if anything was
    /// delivered (caller injects the input IRQ). Called on the timer
    /// tick (mirrors the NAT pump).
    pub fn drain_injected(&mut self, host_base: u64) -> bool {
        use super::virtqueue::{avail_idx, avail_ring, read_desc, used_push};

        let mut pending = INPUT_Q.lock();
        if pending.is_empty() {
            return false;
        }
        let dn = DRAIN_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let q0_en = self.queues.first().map(|q| q.enable).unwrap_or(0);
        let q0_sz = self.queues.first().map(|q| q.size).unwrap_or(0);
        let q = match self.queues.get_mut(0) {
            Some(q) if q.enable != 0 && q.size != 0 => q,
            _ => {
                if dn < 8 {
                    kprintln!("[virtio-input] drain#{}: q0 not ready (enable={} size={}, {} pending)",
                              dn + 1, q0_en, q0_sz, pending.len());
                }
                return false;
            }
        };
        let avail_top = match avail_idx(host_base, q.driver_gpa()) {
            Some(v) => v,
            None => {
                if dn < 8 {
                    kprintln!("[virtio-input] drain#{}: avail_idx read failed (driver_gpa={:#x})",
                              dn + 1, q.driver_gpa());
                }
                return false;
            }
        };
        if dn < 8 {
            kprintln!("[virtio-input] drain#{}: pending={} avail_top={} last_avail={} size={}",
                      dn + 1, pending.len(), avail_top, q.last_avail_idx, q.size);
        }

        let mut any = false;
        while q.last_avail_idx != avail_top {
            let (etype, code, value) = match pending.pop_front() {
                Some(e) => e,
                None => break, // no more events; leave buffers queued
            };
            let head = match avail_ring(host_base, q.driver_gpa(), q.size, q.last_avail_idx) {
                Some(v) => v,
                None => break,
            };
            if let Some(d) = read_desc(host_base, q.desc_gpa(), head, q.size) {
                if d.len >= 8 {
                    super::guest_mem::write_u16(host_base, d.addr, etype);
                    super::guest_mem::write_u16(host_base, d.addr + 2, code);
                    super::guest_mem::write_u32(host_base, d.addr + 4, value);
                }
            }
            used_push(host_base, q.device_gpa(), q.size, &mut q.used_idx, head, 8);
            q.last_avail_idx = q.last_avail_idx.wrapping_add(1);
            any = true;
        }

        if any {
            self.isr |= 1;
        }
        if dn < 8 {
            kprintln!("[virtio-input] drain#{}: delivered={} new_last_avail={} used_idx={}",
                      dn + 1, any, self.queues[0].last_avail_idx, self.queues[0].used_idx);
        }
        any
    }

    /// Drain the statusq: every buffer the driver wrote (LED state,
    /// repeat-rate, etc.) gets immediately pushed onto the used-ring.
    /// We don't actually do anything with the state.
    fn service_statusq(&mut self, host_base: u64) -> bool {
        use super::virtqueue::{avail_idx, avail_ring, used_push};

        let q = match self.queues.get_mut(1) {
            Some(q) if q.enable != 0 => q,
            _ => return false,
        };
        if q.size == 0 { return false; }

        let avail_top = match avail_idx(host_base, q.driver_gpa()) {
            Some(v) => v, None => return false,
        };
        if avail_top == q.last_avail_idx { return false; }

        let mut any = false;
        while q.last_avail_idx != avail_top {
            let head = match avail_ring(host_base, q.driver_gpa(), q.size, q.last_avail_idx) {
                Some(v) => v, None => break,
            };
            used_push(host_base, q.device_gpa(), q.size, &mut q.used_idx, head, 0);
            q.last_avail_idx = q.last_avail_idx.wrapping_add(1);
            any = true;
        }

        if any { self.isr |= 1; }
        any
    }

    pub fn pci_read_dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (VIRTIO_INPUT_DEVICE << 16) | VIRTIO_VENDOR,
            // status (cap-list bit) | command (mem + bus-master + io)
            0x04 => (0x0010 << 16) | 0x0007,
            // class 09_00_00 (Input device controller) | revision 0x01
            0x08 => (0x09_00_00 << 8) | 0x01,
            0x0C => 0,
            0x10 => if self.bar0_lo_sized { BAR0_SIZE_MASK_LO } else { self.bar0_lo },
            0x14 => if self.bar0_hi_sized { 0xFFFF_FFFF }       else { self.bar0_hi },
            0x18..=0x24 => 0,
            0x28 => 0,
            // Subsystem vendor 0x1AF4 / Subsystem device 0x0012 (input)
            0x2C => (0x0012 << 16) | 0x1AF4,
            0x30 => 0,
            0x34 => CAP_COMMON_OFF as u32,
            0x38 => 0,
            // Interrupt: line=12, pin=INTA. Conventional PS/2-mouse IRQ
            // line, free in our microvm (no real PS/2 controller).
            0x3C => 0x0000_010C,

            // Modern virtio capability list — same shape as the rest.
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
            let queue = ((off - NOTIFY_OFF) / NOTIFY_OFF_MULTIPLIER) as u16;
            let _ = value; let _ = width;
            self.pending_kick_queue = Some(queue);
        } else if off >= DEVICE_OFF && off < DEVICE_OFF + DEVICE_LEN {
            self.device_write(off - DEVICE_OFF, width, value);
        }
    }

    fn common_read(&self, off: u32, width: u8) -> u64 {
        let mask = width_mask(width);
        let v: u64 = match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select as u64,
            CC_DEVICE_FEATURE => {
                if self.device_feature_select == 1 {
                    1 // VIRTIO_F_VERSION_1 (bit 32)
                } else {
                    0 // no virtio-input feature bits in <32 range
                }
            }
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select as u64,
            CC_DRIVER_FEATURE => {
                let half = (self.driver_feature_select & 1) as usize;
                self.driver_features[half] as u64
            }
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
        v & mask
    }

    fn common_write(&mut self, off: u32, width: u8, raw: u64) {
        let val = raw & width_mask(width);
        match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select = val as u32,
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select = val as u32,
            CC_DRIVER_FEATURE => {
                let half = (self.driver_feature_select & 1) as usize;
                self.driver_features[half] = val as u32;
            }
            CC_MSIX_CONFIG => self.msix_config = val as u16,
            CC_DEVICE_STATUS => {
                let prev = self.device_status;
                self.device_status = val as u8;
                kprintln!("[virtio-input] device_status {:#04x} -> {:#04x}",
                          prev, self.device_status);
                if self.device_status == 0 {
                    for q in self.queues.iter_mut() {
                        *q = VirtQueue {
                            size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
                            desc_lo: 0, desc_hi: 0,
                            driver_lo: 0, driver_hi: 0,
                            device_lo: 0, device_hi: 0,
                            last_avail_idx: 0, used_idx: 0,
                        };
                    }
                    self.driver_features = [0; 2];
                    self.driver_feature_select = 0;
                    self.device_feature_select = 0;
                    self.queue_select = 0;
                    self.cfg_select = VIRTIO_INPUT_CFG_UNSET;
                    self.cfg_subsel = 0;
                    self.cfg_size = 0;
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

    fn device_read(&self, off: u32, width: u8) -> u64 {
        // Device-cfg map: 0=select, 1=subsel, 2=size, 3..7=reserved,
        //                 8..136=u (128 bytes).
        let mask = width_mask(width);
        let mut v: u64 = 0;
        for i in 0..(width as u32) {
            let p = off + i;
            let byte: u8 = match p {
                0 => self.cfg_select,
                1 => self.cfg_subsel,
                2 => self.cfg_size,
                3..=7 => 0,
                8..=135 => self.cfg_data[(p - 8) as usize],
                _ => 0,
            };
            v |= (byte as u64) << (i * 8);
        }
        v & mask
    }

    fn device_write(&mut self, off: u32, width: u8, raw: u64) {
        let val = raw & width_mask(width);
        for i in 0..(width as u32) {
            let byte = (val >> (i * 8)) as u8;
            let p = off + i;
            match p {
                0 => self.cfg_select = byte,
                1 => self.cfg_subsel = byte,
                _ => {} // size + reserved + u are RO from driver POV
            }
        }
        self.recompute_cfg();
    }

    /// Latch the (size, u) response for the current (select, subsel).
    /// virtio 1.2 §5.8.5: the device MUST guarantee that once `select`
    /// is set, `size` and `u` reflect that selection coherently — i.e.
    /// we re-derive on every selector write, not lazily on read.
    fn recompute_cfg(&mut self) {
        self.cfg_data = [0; 128];
        self.cfg_size = match self.cfg_select {
            VIRTIO_INPUT_CFG_ID_NAME => {
                let name = b"nopeek-keyboard";
                self.cfg_data[..name.len()].copy_from_slice(name);
                name.len() as u8
            }
            VIRTIO_INPUT_CFG_ID_SERIAL => 0,
            VIRTIO_INPUT_CFG_ID_DEVIDS => {
                // virtio_input_devids { bustype, vendor, product, version } LE u16 × 4
                self.cfg_data[0] = 0x06; self.cfg_data[1] = 0x00;     // bustype VIRTUAL
                self.cfg_data[2] = 0xF4; self.cfg_data[3] = 0x1A;     // vendor 0x1AF4
                self.cfg_data[4] = 0x01; self.cfg_data[5] = 0x00;     // product 1
                self.cfg_data[6] = 0x01; self.cfg_data[7] = 0x00;     // version 1
                8
            }
            VIRTIO_INPUT_CFG_PROP_BITS => 0, // no input device properties
            VIRTIO_INPUT_CFG_EV_BITS => self.fill_ev_bits(),
            VIRTIO_INPUT_CFG_ABS_INFO => 0,  // no ABS axes
            _ => 0,
        };
    }

    /// Bitmap of supported codes for the requested EV_TYPE in
    /// `cfg_subsel`. Linux iterates subsel=0..EV_CNT; a non-zero size
    /// signals "this type supported".
    fn fill_ev_bits(&mut self) -> u8 {
        match self.cfg_subsel {
            // EV_SYN — bit 0 (SYN_REPORT)
            x if x == EV_SYN => {
                self.set_bit(SYN_REPORT as usize);
                1
            }
            // EV_KEY — advertise the full 0..=255 keycode range (32
            // bytes). Linux's input core drops any EV_KEY whose code
            // isn't set in dev->keybit (test_bit in input_handle_event)
            // — so declaring only KEY_ESC/KEY_A meant every other
            // injected key (e.g. KEY_SPACE=57) was silently filtered
            // before evdev, even though the eventq/IRQ path worked.
            // Covering the standard keyboard range fixes injection now
            // and is what Phase B's real per-scancode mapping needs.
            x if x == EV_KEY => {
                let _ = (KEY_ESC, KEY_A); // (kept as named refs)
                for code in 0..=255usize {
                    self.set_bit(code);
                }
                32
            }
            // EV_MSC / EV_REP — declare unsupported (size=0).
            x if x == EV_MSC => 0,
            x if x == EV_REP => 0,
            _ => 0,
        }
    }

    fn set_bit(&mut self, bit: usize) {
        let (byte, mask) = (bit / 8, 1u8 << (bit % 8));
        if byte < self.cfg_data.len() {
            self.cfg_data[byte] |= mask;
        }
    }

    fn q(&self) -> &VirtQueue {
        &self.queues[self.queue_select as usize % self.queues.len()]
    }

    fn q_mut(&mut self) -> &mut VirtQueue {
        let idx = self.queue_select as usize % self.queues.len();
        &mut self.queues[idx]
    }
}

const fn width_mask(width: u8) -> u64 {
    match width {
        1 => 0xFF,
        2 => 0xFFFF,
        4 => 0xFFFF_FFFF,
        _ => 0xFFFF_FFFF_FFFF_FFFF,
    }
}
