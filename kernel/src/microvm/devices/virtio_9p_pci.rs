//! virtio-9p-pci device — host↔guest shared filesystem (9P2000.L).
//!
//! Modern virtio (1.0+): vendor 0x1AF4, device 0x1049 (= 0x1040 +
//! virtio device-type 9). The guest's built-in `9pnet_virtio` +
//! `9p` (v9fs) drivers attach and mount via:
//!
//! ```text
//!   mount -t 9p -o trans=virtio,version=9p2000.L,access=any \
//!         npkhome /some/mountpoint
//! ```
//!
//! The mount tag (`npkhome`) is advertised in device-cfg. One request
//! virtqueue carries 9P messages: the driver-readable descriptors hold
//! the outbound T-message, the driver-writable descriptors receive the
//! inbound R-message.
//!
//! The 9P server maps operations onto npkFS, rooted (and CONFINED) at
//! `home/<user>/` so the guest can see/read/write the user's files and
//! they show up live in loft — never `sys/`, never other apps' images.
//!
//! STATUS — step 1 (this commit): device skeleton + virtqueue request/
//! response plumbing + `Tversion`. Everything else returns `Rlerror`,
//! so the guest binds the device but a `mount` fails cleanly (no crash).
//! The real ops (attach/walk/readdir/getattr/lopen/read → then write)
//! land next, gated behind the guest never mounting 9p yet (PID-1
//! doesn't issue the mount until the server is ready).

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use crate::kprintln;
use super::guest_mem::GuestMem;
use super::virtqueue::{read_desc, avail_idx, avail_ring, used_push, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE};

const VIRTIO_VENDOR: u32 = 0x1AF4;
/// Modern virtio device id = 0x1040 + device-type. 9p = type 9.
const VIRTIO_9P_DEVICE: u32 = 0x1049;

/// BAR0 — next free 16 KB window above the sqfs blk device (0xFE01_0000).
pub const BAR0_BASE: u64 = 0xFE01_4000;
pub const BAR0_SIZE: u64 = 0x4000;
pub const BAR0_END: u64 = BAR0_BASE + BAR0_SIZE;
const BAR0_SIZE_MASK_LO: u32 = !((BAR0_SIZE as u32) - 1) | 0b0100; // 64-bit MMIO

/// 8259 line. 0,5,9,10,11,12 are taken (timer/sqfs/gpu/net/blk/input);
/// 6 is free.
const IRQ_LINE: u8 = 6;

// PCI capability list anchors (identical scheme to virtio-blk).
const CAP_COMMON_OFF: u8 = 0x40;
const CAP_NOTIFY_OFF: u8 = 0x54;
const CAP_ISR_OFF:    u8 = 0x68;
const CAP_DEVICE_OFF: u8 = 0x78;

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG:    u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

const COMMON_OFF: u32 = 0x0000; const COMMON_LEN: u32 = 0x0100;
const NOTIFY_OFF: u32 = 0x0100; const NOTIFY_LEN: u32 = 0x0100;
const ISR_OFF:    u32 = 0x0200; const ISR_LEN:    u32 = 0x0100;
const DEVICE_OFF: u32 = 0x0300; const DEVICE_LEN: u32 = 0x0100;
const NOTIFY_OFF_MULTIPLIER: u32 = 4;

// Common Cfg register offsets (virtio 1.2 §4.1.4.3).
const CC_DEVICE_FEATURE_SELECT: u32 = 0x00;
const CC_DEVICE_FEATURE:        u32 = 0x04;
const CC_DRIVER_FEATURE_SELECT: u32 = 0x08;
const CC_DRIVER_FEATURE:        u32 = 0x0C;
const CC_MSIX_CONFIG:           u32 = 0x10;
const CC_NUM_QUEUES:            u32 = 0x12;
const CC_DEVICE_STATUS:         u32 = 0x14;
const CC_CONFIG_GENERATION:     u32 = 0x15;
const CC_QUEUE_SELECT:          u32 = 0x16;
const CC_QUEUE_SIZE:            u32 = 0x18;
const CC_QUEUE_MSIX_VECTOR:     u32 = 0x1A;
const CC_QUEUE_ENABLE:          u32 = 0x1C;
const CC_QUEUE_NOTIFY_OFF:      u32 = 0x1E;
const CC_QUEUE_DESC_LO:         u32 = 0x20;
const CC_QUEUE_DESC_HI:         u32 = 0x24;
const CC_QUEUE_DRIVER_LO:       u32 = 0x28;
const CC_QUEUE_DRIVER_HI:       u32 = 0x2C;
const CC_QUEUE_DEVICE_LO:       u32 = 0x30;
const CC_QUEUE_DEVICE_HI:       u32 = 0x34;

const NUM_QUEUES: u16 = 1;
const MAX_QUEUE_SIZE: u16 = 256;

/// virtio-9p feature: a mount tag is present in device-cfg (§5.9.3).
const VIRTIO_9P_F_MOUNT_TAG: u32 = 1 << 0;

/// The mount tag the guest passes to `mount -t 9p ... <tag> ...`.
const MOUNT_TAG: &[u8] = b"npkhome";

/// Largest 9P message we negotiate. Bounds the per-request buffers.
const MAX_MSIZE: u32 = 128 * 1024;

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

pub struct Virtio9p {
    bar0_lo: u32,
    bar0_hi: u32,
    bar0_lo_sized: bool,
    bar0_hi_sized: bool,

    device_feature_select: u32,
    driver_feature_select: u32,
    driver_features: [u32; 2],
    msix_config: u16,
    device_status: u8,
    config_generation: u8,
    queue_select: u16,

    queues: [VirtQueue; NUM_QUEUES as usize],

    isr: u8,
    bar0_base_init: u64,
    irq_line: u8,
    pending_kick_queue: Option<u16>,

    /// Negotiated msize (after Tversion).
    msize: u32,
    log_count: u32,
}

impl Virtio9p {
    pub fn new() -> Self {
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
            queues: [VirtQueue { size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
                desc_lo: 0, desc_hi: 0, driver_lo: 0, driver_hi: 0,
                device_lo: 0, device_hi: 0, last_avail_idx: 0, used_idx: 0 };
                NUM_QUEUES as usize],
            isr: 0,
            bar0_base_init: BAR0_BASE,
            irq_line: IRQ_LINE,
            pending_kick_queue: None,
            msize: 8192,
            log_count: 0,
        }
    }

    pub fn irq_line(&self) -> u8 { self.irq_line }
    pub fn take_pending_kick(&mut self) -> Option<u16> { self.pending_kick_queue.take() }

    pub fn bar0_base(&self) -> u64 {
        ((self.bar0_hi as u64) << 32) | (self.bar0_lo as u64 & !0x0Fu64)
    }
    pub fn bar0_in_range(&self, gpa: u64) -> bool {
        let base = self.bar0_base();
        gpa >= base && gpa < base + BAR0_SIZE
    }

    // ── PCI config-space dword reads ────────────────────────────────
    pub fn pci_read_dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (VIRTIO_9P_DEVICE << 16) | VIRTIO_VENDOR,
            0x04 => (0x0010 << 16) | 0x0007,        // status: cap-list | command: mem+busmaster+io
            0x08 => (0x00_00_00 << 8) | 0x01,       // class: unclassified | revision 1
            0x0C => 0,
            0x10 => if self.bar0_lo_sized { BAR0_SIZE_MASK_LO } else { self.bar0_lo },
            0x14 => if self.bar0_hi_sized { 0xFFFF_FFFF } else { self.bar0_hi },
            0x18..=0x24 => 0,
            0x28 => 0,
            0x2C => (0x0009 << 16) | 0x1AF4,        // subsystem id (9 = 9p)
            0x30 => 0,
            0x34 => CAP_COMMON_OFF as u32,
            0x38 => 0,
            0x3C => (0x01 << 8) | self.irq_line as u32,

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
                else { self.bar0_lo = value & !0x0F | (BAR0_BASE as u32 & 0x0F); self.bar0_lo_sized = false; }
            }
            0x14 => {
                if value == 0xFFFF_FFFF { self.bar0_hi_sized = true; }
                else { self.bar0_hi = value; self.bar0_hi_sized = false; }
            }
            _ => {}
        }
    }

    // ── BAR0 MMIO ───────────────────────────────────────────────────
    pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {
        if off >= COMMON_OFF && off < COMMON_OFF + COMMON_LEN {
            self.common_read(off - COMMON_OFF, width)
        } else if off >= ISR_OFF && off < ISR_OFF + ISR_LEN {
            let v = self.isr as u64; self.isr = 0; v & width_mask(width)
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
            let _ = (value, width);
            self.pending_kick_queue = Some(queue);
        }
        // ISR read-to-clear; device-cfg read-only.
    }

    fn common_read(&self, off: u32, width: u8) -> u64 {
        let mask = width_mask(width);
        let v: u64 = match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select as u64,
            CC_DEVICE_FEATURE => {
                if self.device_feature_select == 1 {
                    1 // bit 32 = VIRTIO_F_VERSION_1
                } else {
                    VIRTIO_9P_F_MOUNT_TAG as u64 // bit 0
                }
            }
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
        v & mask
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
                    for q in self.queues.iter_mut() {
                        *q = VirtQueue { size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
                            desc_lo: 0, desc_hi: 0, driver_lo: 0, driver_hi: 0,
                            device_lo: 0, device_hi: 0, last_avail_idx: 0, used_idx: 0 };
                    }
                    self.driver_features = [0; 2];
                    self.driver_feature_select = 0;
                    self.device_feature_select = 0;
                    self.queue_select = 0;
                    self.msize = 8192;
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

    /// Device-cfg = virtio-9p config: tag_len (u16) @ 0, tag bytes @ 2.
    fn device_read(&self, off: u32, width: u8) -> u64 {
        let mask = width_mask(width);
        let v: u64 = if off == 0 {
            MOUNT_TAG.len() as u64
        } else {
            // tag bytes start at offset 2; assemble `width` bytes LE.
            let mut acc: u64 = 0;
            for i in 0..(width as u32) {
                let tag_idx = (off + i).wrapping_sub(2) as usize;
                let byte = MOUNT_TAG.get(tag_idx).copied().unwrap_or(0);
                acc |= (byte as u64) << (i * 8);
            }
            acc
        };
        v & mask
    }

    fn q(&self) -> &VirtQueue { &self.queues[self.queue_select as usize % self.queues.len()] }
    fn q_mut(&mut self) -> &mut VirtQueue {
        let idx = self.queue_select as usize % self.queues.len();
        &mut self.queues[idx]
    }

    // ── Virtqueue servicing — 9P request/response ───────────────────

    /// Walk the request virtqueue: for each chain, gather the readable
    /// descriptors (T-message), process it, scatter the R-message into
    /// the writable descriptors. Returns true if any chain completed
    /// (caller injects the IRQ).
    pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {
        let (desc, avail, used, size) = {
            let q = match self.queues.get(queue_idx as usize) {
                Some(q) if q.enable != 0 && q.size != 0 => q,
                _ => return false,
            };
            (q.desc_gpa(), q.driver_gpa(), q.device_gpa(), q.size)
        };

        let head_avail = match avail_idx(mem, avail) { Some(v) => v, None => return false };
        let mut last = self.queues[queue_idx as usize].last_avail_idx;
        if head_avail == last { return false; }

        let mut used_idx = self.queues[queue_idx as usize].used_idx;
        let mut serviced = false;

        while last != head_avail {
            let head = match avail_ring(mem, avail, size, last) { Some(v) => v, None => break };

            // Gather readable bytes + collect writable (addr,len) targets.
            let mut req: Vec<u8> = Vec::new();
            let mut wtargets: Vec<(u64, u32)> = Vec::new();
            let mut idx = head;
            let mut guard = 0u32;
            loop {
                let d = match read_desc(mem, desc, idx, size) { Some(d) => d, None => break };
                if d.flags & VRING_DESC_F_WRITE != 0 {
                    wtargets.push((d.addr, d.len));
                } else if (req.len() as u32) < MAX_MSIZE {
                    let take = (d.len as usize).min((MAX_MSIZE as usize).saturating_sub(req.len()));
                    let mut buf = alloc::vec![0u8; take];
                    if mem.read_bytes(d.addr, &mut buf) { req.extend_from_slice(&buf); }
                }
                if d.flags & VRING_DESC_F_NEXT == 0 { break; }
                idx = d.next;
                guard += 1;
                if guard > size as u32 { break; } // malformed loop guard
            }

            let resp = self.process_message(&req);

            // Scatter the response across the writable descriptors.
            let mut off = 0usize;
            for (addr, len) in &wtargets {
                if off >= resp.len() { break; }
                let n = ((*len as usize)).min(resp.len() - off);
                mem.write_bytes(*addr, &resp[off..off + n]);
                off += n;
            }

            used_push(mem, used, size, &mut used_idx, head, resp.len() as u32);
            last = last.wrapping_add(1);
            serviced = true;
        }

        if serviced {
            let q = &mut self.queues[queue_idx as usize];
            q.last_avail_idx = last;
            q.used_idx = used_idx;
            self.isr |= 1;
        }
        serviced
    }

    // ── 9P2000.L protocol ───────────────────────────────────────────

    /// Process one T-message, return the R-message bytes. STEP 1:
    /// only `Tversion` is real; everything else → `Rlerror(ENOSYS)`.
    fn process_message(&mut self, req: &[u8]) -> Vec<u8> {
        // Header: size[4] type[1] tag[2]
        if req.len() < 7 {
            return rlerror(0xFFFF, EINVAL);
        }
        let mtype = req[4];
        let tag = u16::from_le_bytes([req[5], req[6]]);
        let body = &req[7..];
        match mtype {
            TVERSION => self.t_version(tag, body),
            _ => {
                if self.log_count < 16 {
                    kprintln!("[9p] unhandled T-type {} (tag {}) → Rlerror(ENOSYS)", mtype, tag);
                    self.log_count += 1;
                }
                rlerror(tag, ENOSYS)
            }
        }
    }

    /// Tversion: msize[4] version[s]. Negotiate msize + reply 9P2000.L.
    fn t_version(&mut self, tag: u16, body: &[u8]) -> Vec<u8> {
        if body.len() < 6 {
            return rlerror(tag, EINVAL);
        }
        let cli_msize = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
        self.msize = cli_msize.min(MAX_MSIZE).max(4096);
        // We only speak 9P2000.L. (If the client asked for plain 9P2000
        // we'd reply "9P2000" — but v9fs with version=9p2000.L always
        // requests .L, so reply it.)
        let ver: &[u8] = b"9P2000.L";
        let mut b = Vec::with_capacity(4 + 2 + ver.len());
        b.extend_from_slice(&self.msize.to_le_bytes());
        b.extend_from_slice(&(ver.len() as u16).to_le_bytes());
        b.extend_from_slice(ver);
        if self.log_count < 16 {
            kprintln!("[9p] Tversion msize={} → Rversion msize={} 9P2000.L", cli_msize, self.msize);
            self.log_count += 1;
        }
        msg(RVERSION, tag, &b)
    }
}

// ── 9P2000.L message type codes (subset; full set lands incrementally) ──
const RLERROR:  u8 = 7;
const TVERSION: u8 = 100;
const RVERSION: u8 = 101;

// Linux errno values used in Rlerror.
const EINVAL: u32 = 22;
const ENOSYS: u32 = 38;

/// Build a 9P message: size[4] (incl. header) | type[1] | tag[2] | body.
fn msg(mtype: u8, tag: u16, body: &[u8]) -> Vec<u8> {
    let size = (7 + body.len()) as u32;
    let mut v = Vec::with_capacity(size as usize);
    v.extend_from_slice(&size.to_le_bytes());
    v.push(mtype);
    v.extend_from_slice(&tag.to_le_bytes());
    v.extend_from_slice(body);
    v
}

/// Rlerror(ecode): the 9P2000.L error reply (errno in `ecode`).
fn rlerror(tag: u16, ecode: u32) -> Vec<u8> {
    msg(RLERROR, tag, &ecode.to_le_bytes())
}

fn width_mask(width: u8) -> u64 {
    match width { 1 => 0xFF, 2 => 0xFFFF, 4 => 0xFFFF_FFFF, _ => 0xFFFF_FFFF_FFFF_FFFF }
}
