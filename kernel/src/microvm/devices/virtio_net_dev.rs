//! virtio-net device (modern, virtio 1.2) — clean port of the QEMU/Linux
//! reference, replacing the hand-rolled `virtio_net_pci.rs`.
//!
//! What the old device got wrong (and this one does right, 1:1 from
//! `drivers/net/virtio_net.c` + `drivers/virtio/virtio_ring.c`):
//!
//!   * **Mergeable RX buffers** (`VIRTIO_NET_F_MRG_RXBUF`, the QEMU default).
//!     The guest posts single-descriptor buffers (`virtqueue_add_inbuf ...
//!     num_sg=1`); a frame is delivered across N of them, `num_buffers=N` in the
//!     first buffer's header. The old device used `GUEST_TSO4` → "big packets"
//!     mode → each RX buffer a ~19-descriptor page chain → only ~queue_size/19
//!     buffers fit (shallow ring, burst starvation) AND a 19-descriptor walk +
//!     19-page skb reconstruction per frame. Mergeable = deep ring (one buffer
//!     per descriptor) + cheap inject + cheap guest skb.
//!
//!   * **EVENT_IDX** (`VIRTIO_RING_F_EVENT_IDX`). The guest publishes
//!     `used_event` (interrupt me only when used.idx crosses this) in
//!     `avail->ring[size]`; the device publishes `avail_event` (kick me only
//!     when avail.idx crosses this) in `used->ring[size]`. This replaces the
//!     binary NO_INTERRUPT/NO_NOTIFY flags with precise suppression →
//!     dramatically fewer guest IRQs (the EOI storm) and, on RX, zero repost
//!     doorbells (the device polls, so it sets avail_event far ahead).
//!
//!   * **Batched used-ring updates.** A frame's N used entries are written, then
//!     `used.idx` is published once (one release fence), then ONE interrupt
//!     decision — not a fence+ISR per descriptor.
//!
//! The synthetic gateway (ARP/DNS/NAT/GRO/TX-GSO) still lives in `super::nat`;
//! this file owns only the wire-level device. TX (incl. TX-GSO segmentation via
//! `nat::tap_outbound`/`emit_tcp_out`) is unchanged in spirit, ported here.

#![allow(dead_code)]

extern crate alloc;
use crate::kprintln;
use core::sync::atomic::{fence, Ordering};
use super::guest_mem::GuestMem;
use super::nat::{GUEST_MAC, NetCaps};

// ── PCI identity / BAR / capability layout (unchanged — proven enumeration) ──
const VIRTIO_VENDOR:     u32 = 0x1AF4;
const VIRTIO_NET_DEVICE: u32 = 0x1041;

pub const BAR0_BASE: u64 = 0xFE00_4000;
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

// ── Feature bits ──
const VIRTIO_NET_F_CSUM:        u32 = 0;   // device handles TX checksum
const VIRTIO_NET_F_GUEST_CSUM:  u32 = 1;   // guest handles RX checksum
const VIRTIO_NET_F_MAC:         u32 = 5;
const VIRTIO_NET_F_GUEST_TSO4:  u32 = 7;   // guest accepts GSO/TSO4 RX
const VIRTIO_NET_F_GUEST_TSO6:  u32 = 9;
const VIRTIO_NET_F_HOST_TSO4:   u32 = 11;  // guest may send TSO4 (we segment)
const VIRTIO_NET_F_MRG_RXBUF:   u32 = 15;  // mergeable RX buffers (THE big one)
const VIRTIO_NET_F_STATUS:      u32 = 16;
const VIRTIO_F_VERSION_1:       u32 = 32;  // bit 0 of feature word 1
const VIRTIO_RING_F_EVENT_IDX:  u32 = 29;  // used_event / avail_event

// Low feature word (bits 0..31) we advertise. Mergeable + GSO both directions +
// EVENT_IDX. NOT HOST_TSO6 (NAT is IPv4-only) and NOT INDIRECT_DESC (TX walker
// doesn't handle indirect tables yet).
const FEAT_LO: u32 =
      (1 << VIRTIO_NET_F_CSUM)
    | (1 << VIRTIO_NET_F_GUEST_CSUM)
    | (1 << VIRTIO_NET_F_MAC)
    | (1 << VIRTIO_NET_F_GUEST_TSO4)
    | (1 << VIRTIO_NET_F_GUEST_TSO6)
    | (1 << VIRTIO_NET_F_HOST_TSO4)
    | (1 << VIRTIO_NET_F_MRG_RXBUF)
    | (1 << VIRTIO_NET_F_STATUS)
    | (1 << VIRTIO_RING_F_EVENT_IDX);
// High feature word (bits 32..63): only VERSION_1 (bit 32 = bit 0 here).
const FEAT_HI: u32 = 1; // VIRTIO_F_VERSION_1

const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
const VIRTIO_NET_S_LINK_UP:    u16 = 1;

const NUM_QUEUES: u16 = 2;           // q0 = RX, q1 = TX
const MAX_QUEUE_SIZE: u16 = 1024;    // QEMU default depth; mergeable = 1 buf/desc

// virtio-net header is 12 bytes under VERSION_1 (virtio_net_hdr_v1, incl.
// num_buffers @ offset 10). `nat` builds frames with this 12-byte prefix.
const VNET_HDR_LEN: usize = 12;
const NUM_BUFFERS_OFF: u64 = 10;

// Split-ring descriptor flags (virtio 1.2 §2.7.5).
const VRING_DESC_F_NEXT:  u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;
// Ring flags (used when EVENT_IDX is NOT negotiated).
const VRING_AVAIL_F_NO_INTERRUPT: u16 = 1;
const VRING_USED_F_NO_NOTIFY:     u16 = 1;

// ── Split virtqueue ───────────────────────────────────────────────────────
#[derive(Default, Clone, Copy)]
struct VirtQueue {
    size: u16,
    msix_vec: u16,
    enable: u16,
    desc_lo: u32, desc_hi: u32,
    driver_lo: u32, driver_hi: u32,   // avail ring
    device_lo: u32, device_hi: u32,   // used ring
    last_avail_idx: u16,              // next avail entry the device will consume
    used_idx: u16,                    // device's running used.idx
    /// `used.idx` value at the last interrupt we raised — the `old_idx` for
    /// `vring_need_event`. Lets us interrupt only when used.idx crosses the
    /// guest's `used_event` since we last told it.
    last_irq_used_idx: u16,
    /// QEMU's `signalled_used_valid`: false until the first notify decision on
    /// this queue. Without it there is no guaranteed first interrupt after a
    /// reset — the first crossing depends on the guest's zeroed `used_event`
    /// happening to land inside the window.
    signalled_used_valid: bool,
}

impl VirtQueue {
    fn desc_gpa(&self)   -> u64 { ((self.desc_hi   as u64) << 32) | self.desc_lo   as u64 }
    fn avail_gpa(&self)  -> u64 { ((self.driver_hi as u64) << 32) | self.driver_lo as u64 }
    fn used_gpa(&self)   -> u64 { ((self.device_hi as u64) << 32) | self.device_lo as u64 }
    fn ready(&self) -> bool { self.enable != 0 && self.size != 0 }
}

/// One split-ring descriptor.
struct Desc { addr: u64, len: u32, flags: u16, next: u16 }

#[inline]
fn read_desc(mem: &GuestMem, desc_gpa: u64, idx: u16, size: u16) -> Option<Desc> {
    let i = (idx % size) as u64;
    let base = desc_gpa + i * 16;
    Some(Desc {
        addr:  mem.read_u64(base)?,
        len:   mem.read_u32(base + 8)?,
        flags: mem.read_u16(base + 12)?,
        next:  mem.read_u16(base + 14)?,
    })
}

#[inline]
fn avail_idx(mem: &GuestMem, avail_gpa: u64) -> Option<u16> {
    mem.read_u16(avail_gpa + 2)   // avail: flags(2) idx(2) ring[]
}
#[inline]
fn avail_ring(mem: &GuestMem, avail_gpa: u64, size: u16, slot: u16) -> Option<u16> {
    mem.read_u16(avail_gpa + 4 + (slot % size) as u64 * 2)
}
#[inline]
fn avail_flags(mem: &GuestMem, avail_gpa: u64) -> u16 {
    mem.read_u16(avail_gpa).unwrap_or(0)
}
/// `used_event` (guest's interrupt threshold) lives at `avail->ring[size]`.
#[inline]
fn used_event(mem: &GuestMem, avail_gpa: u64, size: u16) -> u16 {
    mem.read_u16(avail_gpa + 4 + size as u64 * 2).unwrap_or(0)
}

/// Write one used-ring element at slot `(used_base_idx) % size` WITHOUT
/// advancing used.idx (the publish is batched). used: flags(2) idx(2) ring[].
#[inline]
fn used_fill(mem: &GuestMem, used_gpa: u64, size: u16, slot_idx: u16, id: u16, len: u32) {
    let elem = used_gpa + 4 + (slot_idx % size) as u64 * 8;
    mem.write_u32(elem, id as u32);
    mem.write_u32(elem + 4, len);
}
/// Publish a new used.idx (release fence so the element writes settle first).
#[inline]
fn used_publish(mem: &GuestMem, used_gpa: u64, new_idx: u16) {
    fence(Ordering::Release);
    mem.write_u16(used_gpa + 2, new_idx);
}
/// `avail_event` (device's kick threshold) lives at `used->ring[size]`.
#[inline]
fn set_avail_event(mem: &GuestMem, used_gpa: u64, size: u16, val: u16) {
    mem.write_u16(used_gpa + 4 + size as u64 * 8, val);
}

/// vring_need_event (virtio_ring.h): true iff `new_idx` has reached/passed the
/// guest's `event_idx` threshold since `old_idx`. Wrapping u16 arithmetic.
#[inline]
fn need_event(event_idx: u16, new_idx: u16, old_idx: u16) -> bool {
    new_idx.wrapping_sub(event_idx).wrapping_sub(1) < new_idx.wrapping_sub(old_idx)
}

// ── Device ────────────────────────────────────────────────────────────────
pub struct VirtioNet {
    bar0_lo: u32, bar0_hi: u32,
    bar0_lo_sized: bool, bar0_hi_sized: bool,

    device_feature_select: u32,
    driver_feature_select: u32,
    driver_features: [u32; 2],
    msix_config: u16,
    device_status: u8,
    config_generation: u8,
    queue_select: u16,

    queues: [VirtQueue; NUM_QUEUES as usize],

    isr: u8,
    pending_kick_queue: Option<u16>,

    /// Reusable TX read buffer (grown once; a GSO super-frame is ≤64 KiB).
    tx_scratch: alloc::vec::Vec<u8>,

    caps: NetCaps,
}

impl VirtioNet {
    pub const fn new() -> Self {
        Self {
            bar0_lo: BAR0_BASE as u32,
            bar0_hi: (BAR0_BASE >> 32) as u32,
            bar0_lo_sized: false, bar0_hi_sized: false,
            device_feature_select: 0,
            driver_feature_select: 0,
            driver_features: [0; 2],
            msix_config: 0xFFFF,
            device_status: 0,
            config_generation: 0,
            queue_select: 0,
            queues: [VirtQueue {
                size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
                desc_lo: 0, desc_hi: 0, driver_lo: 0, driver_hi: 0,
                device_lo: 0, device_hi: 0,
                last_avail_idx: 0, used_idx: 0, last_irq_used_idx: 0,
                signalled_used_valid: false,
            }; NUM_QUEUES as usize],
            isr: 0,
            pending_kick_queue: None,
            tx_scratch: alloc::vec::Vec::new(),
            caps: NetCaps::dns_tcp(),
        }
    }

    pub fn set_caps(&mut self, caps: NetCaps) { self.caps = caps; }

    pub fn bar0_base(&self) -> u64 {
        ((self.bar0_hi as u64) << 32) | (self.bar0_lo as u64 & !0x0Fu64)
    }
    pub fn bar0_in_range(&self, gpa: u64) -> bool {
        let base = self.bar0_base();
        gpa >= base && gpa < base + BAR0_SIZE
    }

    #[inline]
    fn event_idx_on(&self) -> bool {
        self.driver_features[0] & (1 << VIRTIO_RING_F_EVENT_IDX) != 0
    }
    #[inline]
    fn guest_gso_ok(&self) -> bool {
        self.driver_features[0] & (1 << VIRTIO_NET_F_GUEST_TSO4) != 0
    }

    pub fn take_pending_kick(&mut self) -> Option<u16> { self.pending_kick_queue.take() }

    // ── PCI config space (identical layout to the proven device) ──
    pub fn pci_read_dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (VIRTIO_NET_DEVICE << 16) | VIRTIO_VENDOR,
            0x04 => (0x0010 << 16) | 0x0007,
            0x08 => (0x02_00_00 << 8) | 0x01,
            0x0C => 0,
            0x10 => if self.bar0_lo_sized { BAR0_SIZE_MASK_LO } else { self.bar0_lo },
            0x14 => if self.bar0_hi_sized { 0xFFFF_FFFF } else { self.bar0_hi },
            0x2C => (0x0001 << 16) | 0x1AF4,
            0x34 => CAP_COMMON_OFF as u32,
            0x3C => 0x0000_010A,   // IRQ line 10, INTA

            0x40 => 0x09 | ((CAP_NOTIFY_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_COMMON_CFG as u32) << 24),
            0x48 => COMMON_OFF, 0x4C => COMMON_LEN,

            0x54 => 0x09 | ((CAP_ISR_OFF as u32) << 8) | (20 << 16) | ((VIRTIO_PCI_CAP_NOTIFY_CFG as u32) << 24),
            0x5C => NOTIFY_OFF, 0x60 => NOTIFY_LEN, 0x64 => NOTIFY_OFF_MULTIPLIER,

            0x68 => 0x09 | ((CAP_DEVICE_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_ISR_CFG as u32) << 24),
            0x70 => ISR_OFF, 0x74 => ISR_LEN,

            0x78 => 0x09 | (0 << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_DEVICE_CFG as u32) << 24),
            0x80 => DEVICE_OFF, 0x84 => DEVICE_LEN,
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

    // ── MMIO dispatch (off is BAR0-relative, computed by the run loop) ──
    pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {
        if (COMMON_OFF..COMMON_OFF + COMMON_LEN).contains(&off) {
            self.common_read(off - COMMON_OFF, width)
        } else if (ISR_OFF..ISR_OFF + ISR_LEN).contains(&off) {
            // ISR status is read-to-clear.
            let v = self.isr as u64;
            self.isr = 0;
            v & width_mask(width)
        } else if (DEVICE_OFF..DEVICE_OFF + DEVICE_LEN).contains(&off) {
            self.device_read(off - DEVICE_OFF, width)
        } else {
            0
        }
    }

    pub fn mmio_write(&mut self, off: u32, width: u8, value: u64) {
        if (COMMON_OFF..COMMON_OFF + COMMON_LEN).contains(&off) {
            self.common_write(off - COMMON_OFF, width, value);
        } else if (NOTIFY_OFF..NOTIFY_OFF + NOTIFY_LEN).contains(&off) {
            let q = (off - NOTIFY_OFF) / NOTIFY_OFF_MULTIPLIER;
            self.pending_kick_queue = Some(q as u16);
        }
    }

    fn common_read(&self, off: u32, width: u8) -> u64 {
        let mask = width_mask(width);
        let v: u64 = match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select as u64,
            CC_DEVICE_FEATURE => match self.device_feature_select {
                0 => FEAT_LO as u64,
                1 => FEAT_HI as u64,
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
        v & mask
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
                    self.reset();
                } else if self.device_status & VIRTIO_STATUS_DRIVER_OK != 0 {
                    // GRO exists ONLY on this path. The AMD backend uses
                    // `l3_rewrite_inbound`, which is pure: no coalescing, no
                    // staging queue. So every super-frame we assemble here has
                    // never run anywhere but on Intel hardware, and it is the
                    // one place inbound where we take what Linux is about to
                    // receive, rebuild it, and put our own caps on it.
                    //
                    //     store sys/config/microvm_gro off
                    //
                    // forces it off for an A/B without a rebuild. `microvm
                    // benchvm` with and without answers it in two runs.
                    let gro_off = crate::config::get("microvm_gro")
                        .is_some_and(|v| v.trim().eq_ignore_ascii_case("off"));
                    super::nat::set_gro_enabled(self.guest_gso_ok() && !gro_off);
                    kprintln!(
                        "[net-dev] DRIVER_OK lo=0x{:08x} hi=0x{:08x} mrg={} eventidx={} gso={} gro={}",
                        self.driver_features[0], self.driver_features[1],
                        self.driver_features[0] & (1 << VIRTIO_NET_F_MRG_RXBUF) != 0,
                        self.event_idx_on(), self.guest_gso_ok(), !gro_off,
                    );
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

    fn reset(&mut self) {
        for q in self.queues.iter_mut() {
            *q = VirtQueue {
                size: MAX_QUEUE_SIZE, msix_vec: 0xFFFF, enable: 0,
                desc_lo: 0, desc_hi: 0, driver_lo: 0, driver_hi: 0,
                device_lo: 0, device_hi: 0,
                last_avail_idx: 0, used_idx: 0, last_irq_used_idx: 0,
                signalled_used_valid: false,
            };
        }
        self.driver_features = [0; 2];
        self.driver_feature_select = 0;
        self.device_feature_select = 0;
        self.queue_select = 0;
        self.config_generation = self.config_generation.wrapping_add(1);
        super::nat::set_gro_enabled(false);
    }

    fn device_read(&self, off: u32, width: u8) -> u64 {
        let mut buf = [0u8; 8];
        buf[..6].copy_from_slice(&GUEST_MAC);
        buf[6] = (VIRTIO_NET_S_LINK_UP & 0xFF) as u8;
        buf[7] = ((VIRTIO_NET_S_LINK_UP >> 8) & 0xFF) as u8;
        if off as usize >= buf.len() { return 0; }
        let mut v: u64 = 0;
        let n = (width as usize).min(buf.len() - off as usize);
        for i in 0..n { v |= (buf[off as usize + i] as u64) << (i * 8); }
        v & width_mask(width)
    }

    fn q(&self) -> &VirtQueue {
        &self.queues[self.queue_select as usize % self.queues.len()]
    }
    fn q_mut(&mut self) -> &mut VirtQueue {
        let idx = self.queue_select as usize % self.queues.len();
        &mut self.queues[idx]
    }

    // ── Queue servicing (called from the MMIO-notify VM-exit) ──
    pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {
        match queue_idx {
            1 => self.service_tx(mem),
            // RX kick: the guest added buffers. We poll RX (via inject_rx from the
            // pump), so we don't service here — but suppress further RX kicks by
            // pushing avail_event far ahead (EVENT_IDX). Nothing to deliver now.
            0 => { self.arm_rx_no_kick(mem); false }
            _ => false,
        }
    }

    /// With EVENT_IDX, tell the guest NOT to kick the RX queue (we poll it): set
    /// avail_event past the current avail.idx by ~the whole ring. Without
    /// EVENT_IDX, fall back to the used.flags NO_NOTIFY bit.
    fn arm_rx_no_kick(&mut self, mem: &GuestMem) {
        let q = self.queues[0];
        if !q.ready() { return; }
        if self.event_idx_on() {
            let top = avail_idx(mem, q.avail_gpa()).unwrap_or(0);
            // A target the guest's avail.idx won't reach for a full ring → no kick.
            set_avail_event(mem, q.used_gpa(), q.size, top.wrapping_add(q.size));
        } else {
            // used.flags |= NO_NOTIFY
            mem.write_u16(q.used_gpa(), VRING_USED_F_NO_NOTIFY);
        }
    }

    // ── RX: deliver one frame into the guest, mergeable buffers ──
    /// Inject one full frame (payload incl. 12-byte vnet header). Consumes as
    /// many single-descriptor RX buffers as the frame needs, writes one used
    /// element per buffer, sets `num_buffers=N` in the first buffer's header,
    /// and publishes used.idx once. Does NOT raise the interrupt — the caller
    /// batches that via `rx_should_interrupt`. Returns false (rolling back) if
    /// the guest hasn't posted enough buffers for the whole frame.
    pub fn inject_rx(&mut self, mem: &GuestMem, payload: &[u8]) -> bool {
        let q = &mut self.queues[0];
        if !q.ready() { return false; }

        let avail_top = match avail_idx(mem, q.avail_gpa()) { Some(v) => v, None => return false };
        let start_avail = q.last_avail_idx;
        if avail_top == start_avail { return false; }   // no buffers at all

        let total = payload.len();
        let mut written = 0usize;
        let mut nbuf: u16 = 0;
        let mut first_addr: u64 = 0;
        let start_used = q.used_idx;

        while written < total {
            if q.last_avail_idx == avail_top {
                // Out of buffers mid-frame → can't deliver a partial frame.
                // Roll back: used.idx was never published, so just un-consume the
                // avail entries. Caller requeues the frame and retries.
                q.last_avail_idx = start_avail;
                return false;
            }
            let head = match avail_ring(mem, q.avail_gpa(), q.size, q.last_avail_idx) {
                Some(v) => v, None => { q.last_avail_idx = start_avail; return false; }
            };
            let d = match read_desc(mem, q.desc_gpa(), head, q.size) {
                Some(d) => d, None => { q.last_avail_idx = start_avail; return false; }
            };
            if d.flags & VRING_DESC_F_WRITE == 0 {
                q.last_avail_idx = start_avail; return false;   // malformed (driver-readable)
            }
            let n = (d.len as usize).min(total - written);
            if n > 0 { mem.write_bytes(d.addr, &payload[written..written + n]); }
            if nbuf == 0 { first_addr = d.addr; }
            used_fill(mem, q.used_gpa(), q.size, start_used.wrapping_add(nbuf), head, n as u32);
            nbuf = nbuf.wrapping_add(1);
            written += n;
            q.last_avail_idx = q.last_avail_idx.wrapping_add(1);
        }

        // num_buffers (LE u16) in the first buffer's vnet header.
        mem.write_u16(first_addr + NUM_BUFFERS_OFF, nbuf);
        // Publish used.idx (+N) with a release fence.
        q.used_idx = start_used.wrapping_add(nbuf);
        used_publish(mem, q.used_gpa(), q.used_idx);
        self.isr |= 1;
        true
    }

    /// After a drain pass: should we raise IRQ10 for the RX queue? EVENT_IDX →
    /// only when used.idx has crossed the guest's used_event since our last IRQ;
    /// else honour the avail.flags NO_INTERRUPT bit. Records the threshold so the
    /// next decision uses the right `old_idx`.
    pub fn rx_should_interrupt(&mut self, mem: &GuestMem) -> bool {
        let q = &mut self.queues[0];
        if !q.ready() { return false; }
        // QEMU virtio_should_notify opens with `smp_mb()` and the comment "We
        // need to expose used array entries before checking used event." That
        // barrier was not ported, and on x86 it is the ONE reordering the
        // hardware allows: our store of used.idx may still sit in the store
        // buffer while the load of used_event below executes. The guest does its
        // half correctly (store used_event; mfence; load used.idx), so a missing
        // fence on our side is a textbook Dekker miss — both sides read stale and
        // neither wakes the other.
        //
        // Ordinarily a lost notification is a hiccup. Here it is terminal,
        // because `last_irq_used_idx` below only moves FORWARD: once it has
        // passed the guest's used_event, `need_event` is false for every future
        // check, and the guest can only move used_event from inside NAPI, which
        // only runs on an interrupt. Nothing in the device re-opens that loop —
        // and with the RX ring drained, nothing is injected either, so the
        // decision is never even re-evaluated. That is the absorbing state:
        // works, then one handshake is missed, then silence.
        fence(Ordering::SeqCst);
        if self.driver_features[0] & (1 << VIRTIO_RING_F_EVENT_IDX) != 0 {
            let ev = used_event(mem, q.avail_gpa(), q.size);
            // QEMU virtio_should_notify: `old = signalled_used; signalled_used =
            // used_idx; need_event(used_event, used_idx, old)`. signalled_used is
            // updated on EVERY check, NOT only when we fire — else last_irq runs
            // ahead of the guest's used_event and need_event stays false forever
            // (no IRQ → idle NAPI never wakes → network hangs after a while).
            let old = q.last_irq_used_idx;
            let valid = q.signalled_used_valid;
            q.signalled_used_valid = true;
            q.last_irq_used_idx = q.used_idx;
            // `!v || need_event(...)`, QEMU's exact expression. The first
            // decision after a reset has no meaningful `old`, so it must always
            // notify — otherwise the very first crossing can be swallowed and
            // there is no second chance.
            !valid || need_event(ev, q.used_idx, old)
        } else {
            avail_flags(mem, q.avail_gpa()) & VRING_AVAIL_F_NO_INTERRUPT == 0
        }
    }

    /// Back-compat name used by the pump: true iff the guest currently wants an
    /// RX interrupt. Delegates to the EVENT_IDX-aware decision.
    pub fn rx_wants_irq(&mut self, mem: &GuestMem) -> bool {
        self.rx_should_interrupt(mem)
    }

    /// RX buffers the guest has posted (avail - consumed). With mergeable each is
    /// one descriptor, so this is the true depth (no /19 big-packets penalty).
    pub fn rx_avail_count(&self, mem: &GuestMem) -> u64 {
        let q = self.queues[0];
        if !q.ready() { return 0; }
        match avail_idx(mem, q.avail_gpa()) {
            Some(top) => top.wrapping_sub(q.last_avail_idx) as u64,
            None => 0,
        }
    }

    // ── TX: read guest frames, segment GSO, forward; batched used + EVENT_IDX ──
    /// Combined TX service for the INLINE callers (Intel/VMX + AMD non-full):
    /// drain the ring (cheap, device-touching), emit the segments, finish. AMD
    /// full mode does NOT use this — the worker calls the three pieces below with
    /// the expensive `process_tx` emit run OUTSIDE the device mutex (the TX half
    /// of the v0.226.63 ACK-jitter fix; see net_dataplane::service_full).
    fn service_tx(&mut self, mem: &GuestMem) -> bool {
        super::nat::note_guest_kick();
        let self_caps = self.caps;
        let payloads = self.drain_tx_payloads(mem);
        let advanced = !payloads.is_empty();
        let mut pending_rx: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
        for p in &payloads {
            for rep in super::nat::tap_outbound(p, &self_caps) { pending_rx.push(rep); }
        }
        self.tx_finish(mem, advanced, &pending_rx)
    }

    /// Phase 2a (under the device mutex, CHEAP): walk the guest TX avail ring,
    /// copy each frame's bytes into an owned Vec, publish the used ring. No
    /// segmentation / checksum / host-NIC send here — those are the expensive
    /// part and run lock-free in the caller (`nat::tap_outbound`). Returns one
    /// owned payload per consumed frame (len == frames consumed = "advanced").
    pub fn drain_tx_payloads(&mut self, mem: &GuestMem) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {
        let mut payloads: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
        const TX_FRAME_MAX: usize = 65_536 + 128;
        let mut frame = core::mem::take(&mut self.tx_scratch);
        if frame.len() < TX_FRAME_MAX { frame.resize(TX_FRAME_MAX, 0); }

        let evidx = self.event_idx_on();
        {
            let q = &mut self.queues[1];
            if !q.ready() {
                super::nat::note_tx_ring_bad();
                self.tx_scratch = frame; return payloads;
            }
            let start_used = q.used_idx;
            let mut nused: u16 = 0;
            let mut avail_top = match avail_idx(mem, q.avail_gpa()) {
                Some(v) => v,
                None => {
                    super::nat::note_tx_ring_bad();
                    self.tx_scratch = frame; return payloads;
                }
            };

            // EVENT_IDX "process, then arm notification, then re-check" loop:
            // after draining we publish avail_event = last consumed avail.idx so
            // the guest kicks on the NEXT frame; we re-read avail.idx to catch a
            // frame that landed in the arming window (else TX stalls after one).
            loop {
                while q.last_avail_idx != avail_top {
                    let head = match avail_ring(mem, q.avail_gpa(), q.size, q.last_avail_idx) {
                        Some(v) => v, None => break,
                    };
                    let mut off = 0usize;
                    let mut idx = head;
                    loop {
                        // A broken chain pushes what we have so far as if it were
                        // a whole frame. Nothing downstream can tell a truncated
                        // packet from a short one, so say it here.
                        let d = match read_desc(mem, q.desc_gpa(), idx, q.size) {
                            Some(d) => d,
                            None => { super::nat::note_tx_truncated(); break }
                        };
                        let take = (d.len as usize).min(frame.len().saturating_sub(off));
                        if take > 0 { mem.read_bytes(d.addr, &mut frame[off..off + take]); off += take; }
                        if d.flags & VRING_DESC_F_NEXT == 0 { break; }
                        idx = d.next;
                    }
                    payloads.push(frame[..off].to_vec());   // own it; emit lock-free later

                    // TX buffers are device-read-only → used len = 0 (virtio spec).
                    used_fill(mem, q.used_gpa(), q.size, start_used.wrapping_add(nused), head, 0);
                    nused = nused.wrapping_add(1);
                    q.last_avail_idx = q.last_avail_idx.wrapping_add(1);
                }
                if !evidx { break; }
                // Arm: kick me when avail.idx passes what I've consumed.
                set_avail_event(mem, q.used_gpa(), q.size, q.last_avail_idx);
                fence(Ordering::SeqCst);
                let new_top = avail_idx(mem, q.avail_gpa()).unwrap_or(q.last_avail_idx);
                if new_top == q.last_avail_idx { break; }
                avail_top = new_top;
            }

            if nused > 0 {
                q.used_idx = start_used.wrapping_add(nused);
                used_publish(mem, q.used_gpa(), q.used_idx);
            }
        }
        self.tx_scratch = frame;
        payloads
    }

    /// Phase 2c (under the device mutex, CHEAP): set the TX ISR, inject any
    /// synthetic RX replies (ARP/DNS) the emit produced, and decide whether to
    /// raise IRQ10. `advanced` = at least one TX frame was consumed in the drain.
    pub fn tx_finish(&mut self, mem: &GuestMem, advanced: bool,
                     pending_rx: &[alloc::vec::Vec<u8>]) -> bool {
        // ISR reflects "queue work pending"; the RETURN value tells the run loop
        // whether to actually assert IRQ10, EVENT_IDX-gated (need_event).
        let mut raise = false;
        if advanced {
            self.isr |= 1;
            if self.tx_should_interrupt(mem) { raise = true; }  // NAPI-TX reap
        }

        // Inject any synthetic replies (ARP/DNS) into RX.
        let mut rx_advanced = false;
        for reply in pending_rx {
            if self.inject_rx(mem, reply) { rx_advanced = true; }
        }
        if rx_advanced {
            self.isr |= 1;
            if self.rx_should_interrupt(mem) { raise = true; }
        }
        raise
    }

    /// The negotiated capability mask (Copy) — the worker reads it once under the
    /// device lock, then drives `nat::tap_outbound` lock-free.
    pub fn caps(&self) -> NetCaps { self.caps }

    fn tx_should_interrupt(&mut self, mem: &GuestMem) -> bool {
        let q = &mut self.queues[1];
        if !q.ready() { return false; }
        fence(Ordering::SeqCst); // same barrier, same reason as the RX side
        if self.driver_features[0] & (1 << VIRTIO_RING_F_EVENT_IDX) != 0 {
            let ev = used_event(mem, q.avail_gpa(), q.size);
            let old = q.last_irq_used_idx;
            let valid = q.signalled_used_valid;
            q.signalled_used_valid = true;
            q.last_irq_used_idx = q.used_idx;   // signalled_used every check (QEMU)
            !valid || need_event(ev, q.used_idx, old)
        } else {
            avail_flags(mem, q.avail_gpa()) & VRING_AVAIL_F_NO_INTERRUPT == 0
        }
    }
}

const fn width_mask(width: u8) -> u64 {
    match width { 1 => 0xFF, 2 => 0xFFFF, 4 => 0xFFFF_FFFF, _ => 0xFFFF_FFFF_FFFF_FFFF }
}
