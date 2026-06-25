//! virtio-net-pci device emulation.
//!
//! Modern virtio (1.0+) network device — vendor 0x1AF4, device 0x1041,
//! class 02_00_00 (Network controller / Ethernet). Two virtqueues:
//!   q0 = receive (driver fills with empty buffers, device writes packets)
//!   q1 = transmit (driver fills with packets, device reads + sends)
//!
//! Responsibility split: this file owns the wire-level virtio device
//! (PCI cap chain, Common Cfg, virtqueue walking, RX injection). The
//! synthetic gateway logic — ARP replies, DNS short-circuit, future
//! NAT — lives in `super::nat`. We hand each TX'd frame to
//! `nat::process_tx` and inject whatever replies it produces.
//!
//! Lots of code overlaps with virtio_blk_pci. When we add the third
//! modern-virtio device (virtio-gpu in 12.4) we'll factor the common
//! Common Cfg + cap-list machinery out into a shared
//! `virtio_modern.rs`. For two devices the duplication isn't worth a
//! refactor.

#![allow(dead_code)]

extern crate alloc;
use crate::kprintln;

const VIRTIO_VENDOR: u32 = 0x1AF4;
const VIRTIO_NET_DEVICE: u32 = 0x1041;

/// MMIO BAR0 — placed above virtio-blk's BAR0 (which spans
/// 0xFE000000..0xFE004000).
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

const COMMON_OFF:  u32 = 0x0000;
const COMMON_LEN:  u32 = 0x0100;
const NOTIFY_OFF:  u32 = 0x0100;
const NOTIFY_LEN:  u32 = 0x0100;
const ISR_OFF:     u32 = 0x0200;
const ISR_LEN:     u32 = 0x0100;
const DEVICE_OFF:  u32 = 0x0300;
const DEVICE_LEN:  u32 = 0x0100;

const NOTIFY_OFF_MULTIPLIER: u32 = 4;

// Common Cfg register offsets (same as virtio-blk — virtio 1.2 §4.1.4.3)
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

// virtio-net device-cfg layout (§5.1.4)
const DC_MAC_OFF:    u32 = 0x00;  // 6 bytes
const DC_STATUS_OFF: u32 = 0x06;  // u16

// virtio-net feature bits we advertise
const VIRTIO_NET_F_MAC: u32 = 5;
const VIRTIO_NET_F_STATUS: u32 = 16;
// Guest RX-offload features. Negotiating GUEST_TSO4 puts Linux's virtio-net in
// "big packets" mode (page-chain RX buffers) and lets it accept coalesced GSO
// frames → host-side GRO in nat.rs merges bulk TCP segments (throughput +
// loaded latency). GUEST_CSUM is the prerequisite for the TSO bits.
const VIRTIO_NET_F_GUEST_CSUM: u32 = 1;
const VIRTIO_NET_F_GUEST_TSO4: u32 = 7;
const VIRTIO_NET_F_GUEST_TSO6: u32 = 9;
/// Master switch for RX GRO feature advertisement (GUEST_CSUM/TSO4/TSO6 →
/// guest enters "big packets" mode).
///
/// 2026-06-24 ISOLATION RESULT (v0.225.4, GRO_COALESCE=false): advertising the
/// offloads alone — with NO coalescing, every RX frame a plain pass-through —
/// kills ALL guest networking (zero netstat traffic, no internet). So the
/// culprit is the offload-advertise / big-packets path itself, NOT the
/// coalesced GSO frame. DISABLED (false) again to keep main working. Next
/// (tomorrow): get the guest log to see whether virtio_net probes / eth0 comes
/// up in big-packets mode, or switch to the mergeable-rxbuf RX path (QEMU
/// default, best-tested) instead of big-packets. See project_microvm_rx_gro.
const NET_GRO_ENABLED: bool = true;

/// One-shot guards for the RX-GRO bring-up diagnostic logs.
static GRO_DIAG_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static TX_DIAG_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
/// RX-inject diagnostic: log the first few inject_rx attempts (q0 state +
/// outcome) so we can tell "guest posts no RX buffers in big-packets mode"
/// (no-buf bails) from "guest drops our frames" (delivered but ignored).
static RX_DIAG_COUNT: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// Feature-negotiation trace: bounded count of device-feature reads / driver-
/// feature + status writes, so we can see EXACTLY what we offer and what the
/// guest commits (the v0.225.7 DRIVER_OK showed a collapsed lo=0x2/hi=0 set).
static NEG_TRACE_COUNT: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

#[inline]
fn neg_trace_allowed() -> bool {
    NEG_TRACE_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40
}

// virtio device-status bit (driver finished feature negotiation + setup).
const VIRTIO_STATUS_DRIVER_OK: u8 = 4;

// Status bits
const VIRTIO_NET_S_LINK_UP: u16 = 1;

const NUM_QUEUES: u16 = 2;
// 4096 (up from 1024): in big-packets mode (GRO on) the guest posts each RX
// buffer as a ~19-descriptor page chain, so a 1024-deep virtqueue only holds
// ~53 buffers — which the HW trace showed exhausting under download bursts
// (rxring min 0, injfalse in the thousands/s). 4096 descriptors → ~215 RX
// buffers, ~4× the burst headroom → far fewer "guest ring full" bails and the
// frames-waiting-in-INBOUND_Q latency that comes with them. The host NIC ring
// (drivers/virtio_net.rs) stays QEMU's queue size.
const MAX_QUEUE_SIZE: u16 = 4096;

use super::guest_mem::GuestMem;
use super::nat::{GUEST_MAC, NetCaps};

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

/// Light parser + log for outbound ethernet frames. virtio-net header
/// is 12 bytes (modern, no F_MRG_RXBUF / F_HASH_REPORT). After that
/// comes an ethernet frame. Logs first ~5 packets per VM run.
fn tx_log(_payload: &[u8]) {
    // Per-frame bring-up trace, stripped — the net path is proven
    // end-to-end. NAT layer keeps its own throttled L3 inbound log
    // for diagnosis when we need it.
}

pub struct VirtioNet {
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

    notify_log_count: u32,

    /// Per-VM net policy. Set by the microvm session when the device is
    /// brought up; defaults to DNS-only so a misconfigured caller can't
    /// accidentally exfiltrate anything.
    caps: NetCaps,
}

impl VirtioNet {
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
            notify_log_count: 0,
            caps: NetCaps::dns_tcp(),
        }
    }

    /// Replace the per-VM net policy. Called by microvm session setup
    /// (currently only used by tests / future per-app caps).
    pub fn set_caps(&mut self, caps: NetCaps) { self.caps = caps; }

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

    /// Process queue notify. q0 = RX (driver buffers, device fills,
    /// 12.3.2). q1 = TX (driver sends, device drains).
    pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {
        if queue_idx == 1 {
            self.service_tx(mem)
        } else {
            // RX notify — driver added more empty buffers to receive
            // into. We don't have any pending RX packets yet.
            false
        }
    }

    /// Drain TX queue: walk avail-ring, parse each frame, log + (later)
    /// hand to the host network stack. For 12.3.1 we just decode the
    /// ethernet header so the parsing path is exercised end-to-end.
    fn service_tx(&mut self, mem: &GuestMem) -> bool {
        use super::virtqueue::{avail_idx, avail_ring, read_desc, used_push, VRING_DESC_F_NEXT};

        let self_caps = self.caps;
        let advanced;
        let new_used_idx;
        let mut pending_rx: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
        // Per-frame scratch on the stack — NO per-packet heap alloc (the old
        // Vec::with_capacity + per-descriptor vec![] + extend was 2 mallocs +
        // 2 copies for every uploaded packet; ~18k mallocs/s at 100 Mbit).
        let mut frame = [0u8; 2048];
        {
            let q_idx = 1usize;
            let q = match self.queues.get_mut(q_idx) {
                Some(q) if q.enable != 0 => q,
                _ => return false,
            };
            if q.size == 0 { return false; }

            let avail_top = match avail_idx(mem, q.driver_gpa()) {
                Some(v) => v, None => return false,
            };
            if avail_top == q.last_avail_idx {
                return false;
            }

            let mut any = false;
            while q.last_avail_idx != avail_top {
                let head = match avail_ring(mem, q.driver_gpa(), q.size, q.last_avail_idx) {
                    Some(v) => v, None => break,
                };

                // Walk the descriptor chain. virtio-net frame layout
                // (modern, no merged buffers): one or more driver-readable
                // descriptors. The first 12 bytes of the chain are the
                // virtio_net_hdr; the rest is the ethernet frame.
                let mut total_len: u32 = 0;
                let mut off: usize = 0;
                let mut idx = head;
                loop {
                    let d = match read_desc(mem, q.desc_gpa(), idx, q.size) {
                        Some(d) => d, None => break,
                    };
                    let n = d.len as usize;
                    // Read straight from guest memory into the scratch frame at
                    // the running offset — no per-descriptor alloc/copy.
                    let take = n.min(frame.len().saturating_sub(off));
                    if take > 0 {
                        mem.read_bytes(d.addr, &mut frame[off..off + take]);
                        off += take;
                    }
                    total_len = total_len.saturating_add(n as u32);
                    if d.flags & VRING_DESC_F_NEXT == 0 { break; }
                    idx = d.next;
                }
                let payload = &frame[..off];

                // DIAG (RX-GRO bring-up): first TX frame from the guest proves
                // eth0 came up and is sending — i.e. probe/negotiation succeeded
                // even with the offloads advertised. Its absence (with GRO on)
                // pins the break to probe/negotiation, not RX delivery. One-shot.
                if !TX_DIAG_LOGGED.swap(true, core::sync::atomic::Ordering::Relaxed) {
                    kprintln!("[net-gro] first guest TX frame, {} bytes — eth0 is up", off);
                }

                tx_log(payload);

                used_push(mem, q.device_gpa(), q.size, &mut q.used_idx, head, total_len);
                q.last_avail_idx = q.last_avail_idx.wrapping_add(1);
                any = true;

                // Stash any replies (ARP, DNS, …) we want to inject
                // back via RX. We can't inject inline here — q is
                // borrowed mutably as queue 1; injecting needs queue 0.
                // Defer until after the TX-walk loop ends.
                for rep in super::nat::process_tx(payload, &self_caps) {
                    pending_rx.push(rep);
                }
            }
            advanced = any;
            new_used_idx = q.used_idx;
        }

        // Inject any pending RX replies (ARP-Replies for the gateway).
        let mut rx_advanced = false;
        for reply in &pending_rx {
            if self.inject_rx(mem, reply) {
                rx_advanced = true;
            }
        }

        if advanced { self.isr |= 1; }
        if rx_advanced { self.isr |= 1; }
        let _ = new_used_idx;
        let _ = pending_rx;
        advanced || rx_advanced
    }

    pub fn pci_read_dword(&self, reg: u8) -> u32 {
        match reg {
            0x00 => (VIRTIO_NET_DEVICE << 16) | VIRTIO_VENDOR,
            // status (cap-list bit) | command (mem + bus-master + io)
            0x04 => (0x0010 << 16) | 0x0007,
            // class 02_00_00 (Network controller / Ethernet) | revision 0x01
            0x08 => (0x02_00_00 << 8) | 0x01,
            0x0C => 0,
            0x10 => if self.bar0_lo_sized { BAR0_SIZE_MASK_LO } else { self.bar0_lo },
            0x14 => if self.bar0_hi_sized { 0xFFFF_FFFF }       else { self.bar0_hi },
            0x18..=0x24 => 0,
            0x28 => 0,
            // Subsystem vendor 0x1AF4 / Subsystem device 0x0001 (network)
            0x2C => (0x0001 << 16) | 0x1AF4,
            0x30 => 0,
            0x34 => CAP_COMMON_OFF as u32,
            0x38 => 0,
            // Interrupt: line=10, pin=INTA. Different IRQ from virtio-blk
            // so the 8259 can route them independently.
            0x3C => 0x0000_010A,

            // Modern virtio capability list — same shape as virtio-blk.
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
        }
    }

    fn common_read(&self, off: u32, width: u8) -> u64 {
        let mask = width_mask(width);
        let v: u64 = match off {
            CC_DEVICE_FEATURE_SELECT => self.device_feature_select as u64,
            CC_DEVICE_FEATURE => {
                // Feature words: 0 = device/transport low (bits 0..31),
                // 1 = transport high (bit 32 = VERSION_1). Words >= 2 carry
                // NOTHING — and MUST read back 0. Linux 6.18 reads the
                // extended feature range (selects 2/3, bits 64..127); if we
                // mirror the low word there, the guest negotiates a phantom
                // feature it happens to support in that range and the wire
                // protocol diverges (this was the "no internet" regression).
                let f = match self.device_feature_select {
                    0 => {
                        // bit 5 = VIRTIO_NET_F_MAC, bit 16 = VIRTIO_NET_F_STATUS
                        let mut f = (1u64 << VIRTIO_NET_F_MAC) | (1u64 << VIRTIO_NET_F_STATUS);
                        if NET_GRO_ENABLED {
                            f |= (1u64 << VIRTIO_NET_F_GUEST_CSUM)
                               | (1u64 << VIRTIO_NET_F_GUEST_TSO4)
                               | (1u64 << VIRTIO_NET_F_GUEST_TSO6);
                        }
                        f
                    }
                    1 => 1, // VIRTIO_F_VERSION_1 (bit 32)
                    _ => 0,
                };
                if neg_trace_allowed() {
                    kprintln!("[net-gro] OFFER dev_feat[sel={}] = 0x{:08x} (w{})",
                        self.device_feature_select, f, width);
                }
                f
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
                // Only words 0/1 carry features we offer. Ignore the guest's
                // writes for words >= 2 (it writes 0 there now) — masking them
                // onto [0]/[1] with `& 1` was what corrupted the negotiated set
                // (sel=2 clobbered word 0, sel=3 clobbered word 1).
                let sel = self.driver_feature_select as usize;
                if sel < 2 {
                    self.driver_features[sel] = val as u32;
                }
                if neg_trace_allowed() {
                    kprintln!("[net-gro] COMMIT drv_feat[sel={}] = 0x{:08x} (w{})",
                        self.driver_feature_select, val as u32, width);
                }
            }
            CC_MSIX_CONFIG => self.msix_config = val as u16,
            CC_DEVICE_STATUS => {
                let prev = self.device_status;
                self.device_status = val as u8;
                let _ = prev;
                if neg_trace_allowed() {
                    kprintln!("[net-gro] STATUS 0x{:02x} -> 0x{:02x}", prev, self.device_status);
                }
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
                    self.config_generation = self.config_generation.wrapping_add(1);
                    super::nat::set_gro_enabled(false);
                    // Fresh diagnostic on each device reset / VM relaunch.
                    GRO_DIAG_LOGGED.store(false, core::sync::atomic::Ordering::Relaxed);
                    TX_DIAG_LOGGED.store(false, core::sync::atomic::Ordering::Relaxed);
                    RX_DIAG_COUNT.store(0, core::sync::atomic::Ordering::Relaxed);
                    NEG_TRACE_COUNT.store(0, core::sync::atomic::Ordering::Relaxed);
                } else if self.device_status & VIRTIO_STATUS_DRIVER_OK != 0 {
                    // Feature negotiation is done by DRIVER_OK — latch whether
                    // the guest will accept coalesced GSO RX frames.
                    super::nat::set_gro_enabled(self.guest_gso_ok());
                    // DIAG (RX-GRO bring-up): print the exact feature words the
                    // guest accepted, once. Tells us whether GUEST_CSUM(1)/
                    // TSO4(7)/TSO6(9) were really negotiated (→ big-packets) or
                    // the guest renegotiated them away — separates a probe/
                    // negotiation break from an RX-delivery break. One-shot.
                    if !GRO_DIAG_LOGGED.swap(true, core::sync::atomic::Ordering::Relaxed) {
                        kprintln!(
                            "[net-gro] DRIVER_OK feat lo=0x{:08x} hi=0x{:08x} gso_ok={}",
                            self.driver_features[0], self.driver_features[1],
                            self.guest_gso_ok(),
                        );
                    }
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
        let mask = width_mask(width);
        // Build the device-cfg as a small array, then return the slice
        // starting at `off` masked to `width`. Keeps multi-byte reads
        // (e.g. Linux reads mac as u8[6] often via 6 byte-loads, but
        // status as u16) trivially correct.
        let mut buf = [0u8; 8];
        buf[..6].copy_from_slice(&GUEST_MAC);
        buf[6] = (VIRTIO_NET_S_LINK_UP & 0xFF) as u8;
        buf[7] = ((VIRTIO_NET_S_LINK_UP >> 8) & 0xFF) as u8;

        if off as usize >= buf.len() { return 0; }
        let mut v: u64 = 0;
        let n = (width as usize).min(buf.len() - off as usize);
        for i in 0..n {
            v |= (buf[off as usize + i] as u64) << (i * 8);
        }
        v & mask
    }

    fn q(&self) -> &VirtQueue {
        &self.queues[self.queue_select as usize % self.queues.len()]
    }

    fn q_mut(&mut self) -> &mut VirtQueue {
        let idx = self.queue_select as usize % self.queues.len();
        &mut self.queues[idx]
    }

    /// Push one packet (full payload incl. 12-byte virtio-net header)
    /// into the RX queue. Walks the next driver-provided buffer chain,
    /// writes the payload across descriptors honouring NEXT/WRITE
    /// flags, marks one used-ring entry, advances last_avail_idx.
    /// Returns false if the driver hasn't provided a buffer (RX queue
    /// drained) — caller can drop or retry.
    ///
    /// `pub(super)` so `nat::pump` can inject host-bound TCP response
    /// segments from the timer-tick path.
    /// True if the guest wants an RX interrupt right now — i.e. it has NOT set
    /// VIRTQ_AVAIL_F_NO_INTERRUPT on the RX queue. Linux's virtio-net NAPI sets
    /// that flag while polling the ring; firing IRQ10 anyway preempts the drain,
    /// so the guest reposts buffers slower → RX ring exhausts → INBOUND_Q
    /// overflows → TCP sawtooth. The pump checks this before injecting IRQ10.
    /// True once the driver accepted GUEST_TSO4 — i.e. it will accept the
    /// coalesced GSO RX frames the host-side GRO engine produces.
    fn guest_gso_ok(&self) -> bool {
        NET_GRO_ENABLED && self.driver_features[0] & (1u32 << VIRTIO_NET_F_GUEST_TSO4) != 0
    }

    /// Number of RX buffers the guest currently has posted (avail - consumed).
    /// In big-packets mode each "buffer" is a ~19-descriptor chain, so the count
    /// caps around queue_size/19 — a shallow ring that can starve under bursts.
    pub(super) fn rx_avail_count(&self, mem: &GuestMem) -> u64 {
        match self.queues.get(0) {
            Some(q) if q.enable != 0 && q.size != 0 => {
                match super::virtqueue::avail_idx(mem, q.driver_gpa()) {
                    Some(top) => top.wrapping_sub(q.last_avail_idx) as u64,
                    None => 0,
                }
            }
            _ => 0,
        }
    }

    pub(super) fn rx_wants_irq(&self, mem: &GuestMem) -> bool {
        match self.queues.get(0) {
            Some(q) if q.enable != 0 && q.size != 0 =>
                !super::virtqueue::avail_no_interrupt(mem, q.driver_gpa()),
            _ => true,
        }
    }

    pub(super) fn inject_rx(&mut self, mem: &GuestMem, payload: &[u8]) -> bool {
        use super::virtqueue::{avail_idx, avail_ring, read_desc, used_push, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE};

        // DIAG (RX-GRO): trace the first few inject attempts. `diag` is the
        // 0-based call index; once it exceeds the cap we stop logging.
        let diag = RX_DIAG_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let diag = if diag < 8 { Some(diag) } else { None };

        let q = match self.queues.get_mut(0) {  // RX = q0
            Some(q) if q.enable != 0 => q,
            _ => {
                if let Some(i) = diag { kprintln!("[net-gro] inject#{} q0 not enabled", i); }
                return false;
            }
        };
        if q.size == 0 {
            if let Some(i) = diag { kprintln!("[net-gro] inject#{} q0 size=0", i); }
            return false;
        }

        let avail_top = match avail_idx(mem, q.driver_gpa()) {
            Some(v) => v, None => {
                if let Some(i) = diag { kprintln!("[net-gro] inject#{} avail_idx unreadable", i); }
                return false;
            }
        };
        if avail_top == q.last_avail_idx {
            if let Some(i) = diag {
                kprintln!("[net-gro] inject#{} NO RX BUFFERS (avail={} last={} size={})",
                    i, avail_top, q.last_avail_idx, q.size);
            }
            return false;  // no buffers
        }

        let head = match avail_ring(mem, q.driver_gpa(), q.size, q.last_avail_idx) {
            Some(v) => v, None => return false,
        };

        // Walk the chain, write payload byte-by-byte across descriptors.
        let mut idx = head;
        let mut written: u32 = 0;
        let mut off: usize = 0;
        let mut desc_count: u32 = 0;
        let mut first_desc_len: u32 = 0;
        loop {
            let d = match read_desc(mem, q.desc_gpa(), idx, q.size) {
                Some(v) => v, None => return false,
            };
            if desc_count == 0 { first_desc_len = d.len; }
            desc_count += 1;
            if d.flags & VRING_DESC_F_WRITE == 0 {
                // Driver gave us a non-writable buffer — malformed.
                if let Some(i) = diag {
                    kprintln!("[net-gro] inject#{} desc#{} NOT writable (flags={:#x})",
                        i, desc_count - 1, d.flags);
                }
                return false;
            }
            if off < payload.len() {
                let n = (d.len as usize).min(payload.len() - off);
                if n > 0 {
                    mem.write_bytes(d.addr, &payload[off..off + n]);
                    off += n;
                    written = written.saturating_add(n as u32);
                }
            }
            if off >= payload.len() || d.flags & VRING_DESC_F_NEXT == 0 {
                break;
            }
            idx = d.next;
        }

        if let Some(i) = diag {
            kprintln!("[net-gro] inject#{} OK delivered={}B payload={}B descs={} sg0_len={}",
                i, written, payload.len(), desc_count, first_desc_len);
        }

        used_push(mem, q.device_gpa(), q.size, &mut q.used_idx, head, written);
        q.last_avail_idx = q.last_avail_idx.wrapping_add(1);
        // Set ISR bit 0 so Linux's IRQ handler, when it reads the ISR
        // register (write-1-to-clear), sees there's queue work to do.
        // Without this the pump's IRQ-10 inject looks spurious and
        // Linux returns from the handler without draining the queue.
        self.isr |= 1;
        true
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
