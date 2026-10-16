//! virtio-balloon-pci with free page reporting: the guest hands the pages
//! it has freed back to the host.
//!
//! Modern virtio (1.0+), vendor 0x1AF4, device 0x1045 (virtio-spec device
//! id 5). Features: VERSION_1 and VIRTIO_BALLOON_F_REPORTING; the device
//! never asks the guest to inflate (`num_pages` stays 0). Queues, as the
//! Linux driver numbers them without STATS_VQ and FREE_PAGE_HINT: 0
//! inflateq, 1 deflateq, 2 reporting_vq.
//!
//! On a report (QEMU `virtio_balloon_handle_report`) every device-writable
//! buffer of the chain names guest memory the guest no longer uses; its
//! demand pages go back to the host (`GuestMem::reclaim`) before the
//! buffer is returned, since the guest reuses the pages from then on. A
//! later touch faults in a zeroed page, as after `MADV_DONTNEED` under KVM.
//! A range outside guest RAM, the boot window or a pinned frame is left as
//! it is: the guest only ever gives back frames of its own demand region.

#![allow(dead_code)]

use super::guest_mem::GuestMem;

const VIRTIO_VENDOR: u32 = 0x1AF4;
const VIRTIO_BALLOON_DEVICE: u32 = 0x1045;

pub const BAR0_BASE: u64 = 0xFE02_0000;
pub const BAR0_SIZE: u64 = 0x4000;
/// 8259 line: the legacy parallel-port line, free here.
pub const IRQ_LINE: u8 = 7;
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

/// VIRTIO_BALLOON_F_REPORTING (virtio 1.2 §5.5.3).
const F_REPORTING: u32 = 1 << 5;

const INFLATEQ: u16 = 0;
const DEFLATEQ: u16 = 1;
const REPORTQ: u16 = 2;
const NUM_QUEUES: u16 = 3;
const MAX_QUEUE_SIZE: u16 = 256;

/// The guest's page size for inflate PFNs (virtio 1.2 §5.5.6).
const BALLOON_PAGE: u64 = 4096;

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
    const RESET: VirtQueue = VirtQueue {
        size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
        desc_lo: 0, desc_hi: 0, driver_lo: 0, driver_hi: 0, device_lo: 0, device_hi: 0,
        last_avail_idx: 0, used_idx: 0,
    };

    fn desc_gpa(&self)   -> u64 { ((self.desc_hi   as u64) << 32) | self.desc_lo   as u64 }
    fn driver_gpa(&self) -> u64 { ((self.driver_hi as u64) << 32) | self.driver_lo as u64 }
    fn device_gpa(&self) -> u64 { ((self.device_hi as u64) << 32) | self.device_lo as u64 }
}

pub struct VirtioBalloon {
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

    /// `virtio_balloon_config.actual`, written by the driver.
    actual: u32,
    /// Reports served and pages they gave back, for the end-of-VM line.
    reports: u64,
    given_back: u64,
}

impl VirtioBalloon {
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
            queues: [VirtQueue::RESET; NUM_QUEUES as usize],
            isr: 0,
            pending_kick_queue: None,
            actual: 0,
            reports: 0,
            given_back: 0,
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

    pub fn irq_line(&self) -> u8 {
        IRQ_LINE
    }

    /// Reports served and the pages they gave back.
    pub fn stats(&self) -> (u64, u64) {
        (self.reports, self.given_back)
    }

    /// A queue notify: every buffer the driver made available is served and
    /// returned. True if any was (the caller raises the line).
    pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {
        use super::virtqueue::{avail_idx, avail_pending, avail_ring, chain_next, read_desc, used_push,
                               VRING_DESC_F_WRITE};
        let Some(q) = self.queues.get(queue_idx as usize).copied() else { return false };
        if q.enable == 0 || q.size == 0 {
            return false;
        }
        let Some(top) = avail_idx(mem, q.driver_gpa()) else { return false };
        let n = avail_pending(q.last_avail_idx, top, q.size);
        let (mut last, mut used) = (q.last_avail_idx, q.used_idx);
        for _ in 0..n {
            let Some(head) = avail_ring(mem, q.driver_gpa(), q.size, last) else { break };
            let mut idx = head;
            let mut hops = 0u16;
            while let Some(d) = read_desc(mem, q.desc_gpa(), idx, q.size) {
                match queue_idx {
                    REPORTQ if d.flags & VRING_DESC_F_WRITE != 0 => {
                        self.given_back += mem.reclaim(d.addr, d.len as u64);
                    }
                    // A guest that inflates on its own gives these pages up
                    // as well; a deflate needs nothing, a touch faults in.
                    INFLATEQ if d.flags & VRING_DESC_F_WRITE == 0 => {
                        let mut off = 0u64;
                        while off + 4 <= d.len as u64 {
                            if let Some(pfn) = mem.read_u32(d.addr + off) {
                                self.given_back += mem.reclaim(pfn as u64 * BALLOON_PAGE, BALLOON_PAGE);
                            }
                            off += 4;
                        }
                    }
                    _ => {}
                }
                match chain_next(&d, &mut hops, q.size) {
                    Some(next) => idx = next,
                    None => break,
                }
            }
            if queue_idx == REPORTQ {
                self.reports += 1;
            }
            used_push(mem, q.device_gpa(), q.size, &mut used, head, 0);
            last = last.wrapping_add(1);
        }
        let qm = &mut self.queues[queue_idx as usize];
        qm.last_avail_idx = last;
        qm.used_idx = used;
        if n != 0 {
            self.isr |= 1;
        }
        n != 0
    }

    pub fn pci_read_dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (VIRTIO_BALLOON_DEVICE << 16) | VIRTIO_VENDOR,
            // status (cap-list bit) | command (mem + bus-master + io)
            0x04 => (0x0010 << 16) | 0x0007,
            // class FF_00_00 (unassigned, as QEMU's virtio-balloon-pci) | revision 0x01
            0x08 => (0xFF_00_00 << 8) | 0x01,
            0x0C => 0,
            0x10 => if self.bar0_lo_sized { BAR0_SIZE_MASK_LO } else { self.bar0_lo },
            0x14 => if self.bar0_hi_sized { 0xFFFF_FFFF }       else { self.bar0_hi },
            0x18..=0x24 => 0,
            0x28 => 0,
            // Subsystem vendor 0x1AF4 / Subsystem device 0x0005 (balloon)
            0x2C => (0x0005 << 16) | 0x1AF4,
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
            let queue = ((off - NOTIFY_OFF) / NOTIFY_OFF_MULTIPLIER) as u16;
            self.pending_kick_queue = Some(queue);
        } else if off >= DEVICE_OFF && off < DEVICE_OFF + DEVICE_LEN {
            self.device_write(off - DEVICE_OFF, width, value);
        }
    }

    fn common_read(&self, off: u32, width: u8) -> u64 {
        let v: u64 = match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select as u64,
            CC_DEVICE_FEATURE => match self.device_feature_select {
                0 => F_REPORTING as u64,
                1 => 1, // VIRTIO_F_VERSION_1 (bit 32)
                _ => 0,
            },
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select as u64,
            CC_DRIVER_FEATURE => {
                let sel = self.driver_feature_select as usize;
                if sel < 2 { self.driver_features[sel] as u64 } else { 0 }
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
        v & width_mask(width)
    }

    fn common_write(&mut self, off: u32, width: u8, raw: u64) {
        let val = raw & width_mask(width);
        match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select = val as u32,
            CC_DRIVER_FEATURE_SELECT => self.driver_feature_select = val as u32,
            CC_DRIVER_FEATURE => {
                let sel = self.driver_feature_select as usize;
                if sel < 2 { self.driver_features[sel] = val as u32; }
            }
            CC_MSIX_CONFIG => self.msix_config = val as u16,
            CC_DEVICE_STATUS => {
                self.device_status = val as u8;
                if self.device_status == 0 {
                    self.queues = [VirtQueue::RESET; NUM_QUEUES as usize];
                    self.driver_features = [0; 2];
                    self.driver_feature_select = 0;
                    self.device_feature_select = 0;
                    self.queue_select = 0;
                    self.actual = 0;
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

    /// `virtio_balloon_config`: `num_pages` (0, never inflate) at 0,
    /// `actual` at 4.
    fn device_read(&self, off: u32, width: u8) -> u64 {
        let cfg = (self.actual as u64) << 32;
        let mut v = 0u64;
        for i in 0..width as u32 {
            let p = off + i;
            let byte = if p < 8 { (cfg >> (p * 8)) as u8 } else { 0 };
            v |= (byte as u64) << (i * 8);
        }
        v & width_mask(width)
    }

    fn device_write(&mut self, off: u32, width: u8, raw: u64) {
        let val = raw & width_mask(width);
        for i in 0..width as u32 {
            let p = off + i;
            if (4..8).contains(&p) {
                let shift = (p - 4) * 8;
                self.actual = (self.actual & !(0xFF << shift)) | (((val >> (i * 8)) as u32 & 0xFF) << shift);
            }
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
