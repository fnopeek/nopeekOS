//! VirtIO Network Device Driver
//!
//! Legacy (0.9.5) VirtIO PCI transport with RX/TX virtqueues.
//! Provides Ethernet frame send/receive for the TCP/IP stack.

use core::sync::atomic::{fence, AtomicBool, Ordering};

/// The host NIC RX used.idx field, published at init. Lets the off-vCPU data
/// plane busy-poll for RX arrival without taking the DEVICE lock every spin
/// iteration.
static RX_USED_IDX: spin::Once<DmaRegion> = spin::Once::new();
/// The device segments and checksums TCPv4 GSO frames (`send_tso`).
static TSO: AtomicBool = AtomicBool::new(false);

pub fn tso_capable() -> bool { TSO.load(Ordering::Acquire) }

/// Current host NIC RX used.idx (lock-free volatile read). The data-plane worker
/// caches the value it last drained to; a change means a frame arrived.
#[inline]
pub fn rx_used_idx() -> u16 {
    RX_USED_IDX.get().map_or(0, |r| r.r16(0u64))
}
use spin::Mutex;
use crate::hw::{DmaRegion, PortRange};
use crate::{kprintln, memory, pci};

const VIRTIO_VENDOR: u16 = 0x1AF4;
const VIRTIO_NET_DEV: u16 = 0x1000;

const REG_DEV_FEATURES: u16  = 0x00;
const REG_DRV_FEATURES: u16  = 0x04;
const REG_QUEUE_PFN: u16     = 0x08;
const REG_QUEUE_SIZE: u16    = 0x0C;
const REG_QUEUE_SEL: u16     = 0x0E;
const REG_QUEUE_NOTIFY: u16  = 0x10;
const REG_STATUS: u16        = 0x12;
const REG_ISR: u16           = 0x13;
// Legacy-virtio MSI-X vector registers — present only when MSI-X is enabled
// (which also shifts the device config from offset 20 to 24).
const REG_CONFIG_MSIX_VEC: u16 = 0x14;
const REG_QUEUE_MSIX_VEC: u16  = 0x16;
const MSIX_NO_VECTOR: u16      = 0xFFFF;

const S_ACKNOWLEDGE: u8 = 1;
const S_DRIVER: u8      = 2;
const S_DRIVER_OK: u8   = 4;
const S_FAILED: u8      = 128;

const F_MAC: u32       = 1 << 5;
const F_CSUM: u32      = 1 << 0;   // VIRTIO_NET_F_CSUM      — device checksums TX
const F_GSO: u32       = 1 << 6;   // VIRTIO_NET_F_GSO (legacy) — device handles
                                   // ANY GSO type + checksum. QEMU's transitional
                                   // virtio-net offers this (not the modern
                                   // per-type HOST_TSO4) to a legacy driver.
const F_HOST_TSO4: u32 = 1 << 11;  // VIRTIO_NET_F_HOST_TSO4 — device segments TSOv4
/// virtio_net_hdr.flags / gso_type for offloaded TX (host does csum + TCP-GSO).
#[allow(dead_code)]
const F_STATUS: u32    = 1 << 16;

const DESC_F_NEXT: u16  = 1;
const DESC_F_WRITE: u16 = 2;

const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;
// RX ring depth. If `net::poll()` cannot drain in time, a full ring makes
// QEMU drop inbound frames, the sender retransmits and its cwnd collapses.
// 1024 buffers give ~12 ms of cushion at 1 Gbit. Needs QEMU
// `rx_queue_size=1024` (else capped via `RX_BUFFERS.min(rx_qs)`).
const RX_BUFFERS: usize = 1024;

// On a full TX ring, spin-reclaim this many times before dropping the
// frame. QEMU/slirp drains the ring on its own host thread, so a bounded
// busy-wait lets in-flight descriptors complete instead of dropping the
// guest's packet (→ TCP retransmit → upload throughput collapse). Bounded
// so a genuinely wedged device can't hang the sender.
const TX_RECLAIM_SPINS: u32 = 4096;
pub const MTU: usize = 1514; // Ethernet max frame

/// A GSO super-frame's data buffer: eth + a full 64 KB IP packet. One per
/// in-flight super-frame, so it is one data descriptor instead of one per MTU
/// chunk; otherwise the ring fills under upload and `send_tso` spins holding
/// `DEVICE`, the lock the RX side needs to take ACKs in.
const TSO_SLOT_SIZE: usize = 66 * 1024;
const TSO_SLOTS: usize = 32;
const NO_TSO_SLOT: u8 = u8::MAX;

/// virtio_net_hdr prepended to every packet (no mergeable buffers): flags u8,
/// gso_type u8, hdr_len u16, gso_size u16, csum_start u16, csum_offset u16.
const NET_HDR_SIZE: usize = 10;
const RX_BUF_SIZE: usize = NET_HDR_SIZE + MTU;

/// Legacy I/O BAR: common header (0x14, 0x18 with MSI-X) plus the MAC field
/// of the device config.
const IO_LEN: u16 = 0x20;

// vring_desc (16 bytes): addr u64 @0, len u32 @8, flags u16 @12, next u16 @14.
const DESC_SIZE: u64 = 16;

fn set_desc(q: &DmaRegion, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
    let o = idx as u64 * DESC_SIZE;
    q.w64(o, addr);
    q.w32(o + 8, len);
    q.w16(o + 12, flags);
    q.w16(o + 14, next);
}

#[allow(dead_code)]
struct VirtioNet {
    io: PortRange,
    mac: [u8; 6],

    // RX queue: descriptor table at 0, avail ring at `rx_avail`, used ring at `rx_used`
    rx_q: DmaRegion,
    rx_avail: u64,
    rx_used: u64,
    rx_queue_size: u16,
    rx_last_used: u16,
    rx_buffers: DmaRegion, // contiguous RX buffer region
    rx_repost_pending: u16, // RX buffers reposted but not yet notified (batch the doorbell)
    pci_addr: pci::PciAddr, // for MSI-X dest re-routing (net-RX IRQ)
    rx_msix_vector: u8,     // LAPIC vector for RX-queue MSI-X (0 = none, polling)

    // TX queue, laid out as the RX queue
    tx_q: DmaRegion,
    tx_avail: u64,
    tx_used: u64,
    tx_queue_size: u16,
    tx_avail_idx: u16,
    tx_last_used: u16,
    tx_num_free: u16,
    tx_notify_pending: bool, // TX frames queued but doorbell not yet rung (batch it)
    tx_free_head: u16,
    tx_hdrs: DmaRegion, // pre-allocated net headers for TX
    /// Pre-allocated DMA-stable TX data pool — one MTU-sized slot per
    /// descriptor. `send` copies the caller's frame here so the device
    /// reads from memory that outlives the caller's stack-local `Vec`
    /// (which may be freed and reused before QEMU's slirp loop does the DMA).
    /// Modelled on `intel_nic::send`'s `tx_bufs`.
    tx_data: DmaRegion,
    /// `TSO_SLOTS` super-frame buffers, `TSO_SLOT_SIZE` each.
    tso_data: DmaRegion,
    /// Bit per free TSO slot.
    tso_free: u32,
    /// Chain head descriptor → the TSO slot it holds, freed on completion.
    tso_slot_of: alloc::vec::Vec<u8>,
}

static DEVICE: Mutex<Option<VirtioNet>> = Mutex::new(None);

pub fn init() -> bool {
    let dev = match pci::find_device(VIRTIO_VENDOR, VIRTIO_NET_DEV) {
        Some(d) => d,
        None => {
            kprintln!("[npk] virtio-net: no device found");
            return false;
        }
    };

    if dev.bar0 & 1 == 0 {
        kprintln!("[npk] virtio-net: BAR0 is MMIO — legacy I/O required");
        return false;
    }
    // SAFETY: BAR0 is this virtio-net device's legacy I/O BAR, which this
    // driver owns; IO_LEN does not exceed the header plus the MAC field.
    let io = unsafe { PortRange::new((dev.bar0 & 0xFFFC) as u16, IO_LEN) };
    pci::enable_bus_master(dev.addr);
    // Try to enable MSI-X for the RX queue → the device raises an interrupt on
    // RX so delivery is event-driven (a drain/wake) instead of pump-cadence
    // polling. `register` enables the MSI-X PCI cap (which shifts the legacy
    // config layout: device config moves 20 → 24). Falls back to polling if
    // the device has no usable MSI-X.
    //
    // Path: enable RX MSI-X here; nat::pump routes the IRQ to the vCPU/BSP-pump
    // core, waking it out of HLT; recv() suppresses RX IRQs NAPI-style during a
    // drain so the device does not interrupt per packet.
    const NET_RX_IRQ_ENABLED: bool = true;
    let rx_vec = if NET_RX_IRQ_ENABLED {
        crate::irq::register(dev.addr, 0).unwrap_or(0)
    } else {
        0
    };
    let cfg_off: u16 = if rx_vec != 0 || pci::msix_enabled(dev.addr) { 24 } else { 20 };

    io.outb(REG_STATUS, 0);
    io.outb(REG_STATUS, S_ACKNOWLEDGE);
    io.outb(REG_STATUS, S_ACKNOWLEDGE | S_DRIVER);

    let features = io.inl(REG_DEV_FEATURES);

    // Accept MAC, plus CSUM+TSO4 if BOTH are offered. These are DEVICE
    // capabilities (it can segment and checksum for us); we do not currently
    // hand it a GSO frame, and accepting them costs nothing. Only when both
    // are present, so we never promise a frame the device can't segment.
    let mut accepted = features & F_MAC;
    // Prefer the modern per-type bits; fall back to the legacy combined F_GSO
    // (what QEMU's transitional device offers a legacy driver). Either lets us
    // forward the guest's GSO super-frame AS-IS (device segments + checksums).
    // Linux `virtnet_probe`: every TSO feature sits inside the F_CSUM
    // branch. A device that cannot checksum cannot segment; QEMU with a
    // slirp backend still lists the legacy F_GSO bit without backing it.
    let modern = (features & (F_CSUM | F_HOST_TSO4)) == (F_CSUM | F_HOST_TSO4);
    let legacy_gso = features & (F_CSUM | F_GSO) == (F_CSUM | F_GSO);
    let offload = modern || legacy_gso;
    crate::kdebug!("[npk] virtio-net: dev features {:#010x} (csum={} gso={} host_tso4={} → offload={})",
              features, features & F_CSUM != 0, legacy_gso, features & F_HOST_TSO4 != 0, offload);
    if modern {
        accepted |= F_CSUM | F_HOST_TSO4;
    } else if legacy_gso {
        accepted |= F_CSUM | F_GSO;
    }
    if offload {
        TSO.store(true, Ordering::Release);
        kprintln!("[npk] virtio-net: TX offload negotiated ({})",
                  if modern { "CSUM+HOST_TSO4" } else { "legacy GSO" });
    }
    io.outl(REG_DRV_FEATURES, accepted);

    // Read MAC address
    let mut mac = [0u8; 6];
    if features & F_MAC != 0 {
        for i in 0..6 {
            mac[i] = io.inb(cfg_off + i as u16);
        }
    }

    // Setup RX queue (queue 0)
    io.outw(REG_QUEUE_SEL, RX_QUEUE);
    let rx_qs = io.inw(REG_QUEUE_SIZE);
    if rx_qs == 0 {
        kprintln!("[npk] virtio-net: RX queue size 0");
        io.outb(REG_STATUS, S_FAILED);
        return false;
    }

    let (rx_q, rx_avail, rx_used) = match setup_queue(&io, rx_qs) {
        Some(v) => v,
        None => {
            io.outb(REG_STATUS, S_FAILED);
            return false;
        }
    };

    // Setup TX queue (queue 1)
    io.outw(REG_QUEUE_SEL, TX_QUEUE);
    let tx_qs = io.inw(REG_QUEUE_SIZE);
    if tx_qs == 0 {
        kprintln!("[npk] virtio-net: TX queue size 0");
        io.outb(REG_STATUS, S_FAILED);
        return false;
    }

    let (tx_q, tx_avail, tx_used) = match setup_queue(&io, tx_qs) {
        Some(v) => v,
        None => {
            io.outb(REG_STATUS, S_FAILED);
            return false;
        }
    };

    // Build TX descriptor free chain
    for i in 0..tx_qs as usize {
        tx_q.w16(i as u64 * DESC_SIZE + 14, if i + 1 < tx_qs as usize { (i + 1) as u16 } else { 0 });
    }

    // Allocate TX net headers (one per descriptor)
    let tx_hdrs = match DmaRegion::alloc_zeroed(
        (tx_qs as usize * NET_HDR_SIZE + 4095) / 4096
    ) {
        Some(r) => r,
        None => {
            io.outb(REG_STATUS, S_FAILED);
            return false;
        }
    };

    // Allocate TX data pool — one MTU slot per descriptor. The
    // sender copies the frame here so the descriptor points at
    // memory that outlives the caller's stack frame; otherwise QEMU/slirp
    // can DMA-read recycled heap memory.
    let tx_data_pages = (tx_qs as usize * MTU + 4095) / 4096;
    let tx_data = match DmaRegion::alloc_zeroed(tx_data_pages) {
        Some(r) => r,
        None => {
            io.outb(REG_STATUS, S_FAILED);
            return false;
        }
    };

    let tso_pages = (TSO_SLOTS * TSO_SLOT_SIZE).div_ceil(4096);
    let tso_data = match memory::allocate_contiguous(tso_pages) {
        // SAFETY: fresh contiguous frames from the allocator, identity-mapped
        // RAM this driver keeps for the device's lifetime. Not zeroed: every
        // slot is written before a descriptor points at it.
        Some(a) => unsafe { DmaRegion::from_raw(a, tso_pages as u64 * 4096) },
        None => {
            io.outb(REG_STATUS, S_FAILED);
            return false;
        }
    };

    // Allocate RX buffers (contiguous, one per RX descriptor)
    let rx_buf_count = RX_BUFFERS.min(rx_qs as usize);
    let rx_buf_pages = (rx_buf_count * RX_BUF_SIZE + 4095) / 4096;
    let rx_buffers = match DmaRegion::alloc_zeroed(rx_buf_pages) {
        Some(r) => r,
        None => {
            io.outb(REG_STATUS, S_FAILED);
            return false;
        }
    };

    // Post RX buffers to the RX queue
    for i in 0..rx_buf_count {
        let buf_addr = rx_buffers.phys() + (i * RX_BUF_SIZE) as u64;
        set_desc(&rx_q, i as u16, buf_addr, RX_BUF_SIZE as u32, DESC_F_WRITE, 0);

        // Add to available ring
        rx_q.w16(rx_avail + 4 + (i as u64) * 2, i as u16);
    }

    // Set available ring idx
    fence(Ordering::SeqCst);
    rx_q.w16(rx_avail + 2, rx_buf_count as u16);

    // Suppress TX interrupts
    tx_q.w16(tx_avail, 1);

    // Bind the RX queue to MSI-X table entry 0 so the device fires our
    // vector on RX. config_msix_vector = NO_VECTOR (we don't want a
    // config-change IRQ). Read-back guards a device that rejects it →
    // rx_msix_vector stays 0 and we keep polling (the recv path is
    // unchanged either way).
    let mut rx_msix_vector = 0u8;
    if rx_vec != 0 {
        io.outw(REG_CONFIG_MSIX_VEC, MSIX_NO_VECTOR);
        io.outw(REG_QUEUE_SEL, RX_QUEUE);
        io.outw(REG_QUEUE_MSIX_VEC, 0);
        if io.inw(REG_QUEUE_MSIX_VEC) == 0 {
            rx_msix_vector = rx_vec;
            crate::kdebug!("[npk] virtio-net: RX MSI-X on vector {:#04x}", rx_vec);
        } else {
            kprintln!("[npk] virtio-net: RX MSI-X vector rejected — polling");
        }
    }

    // Go live
    io.outb(REG_STATUS, S_ACKNOWLEDGE | S_DRIVER | S_DRIVER_OK);
    if io.inb(REG_STATUS) & S_FAILED != 0 {
        kprintln!("[npk] virtio-net: device rejected initialization");
        return false;
    }

    // Notify RX queue that buffers are available
    io.outw(REG_QUEUE_NOTIFY, RX_QUEUE);

    kprintln!("[npk] virtio-net: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    *DEVICE.lock() = Some(VirtioNet {
        io,
        mac,
        rx_q,
        rx_avail,
        rx_used: { RX_USED_IDX.call_once(|| rx_q.sub(rx_used + 2, 2)); rx_used },
        rx_queue_size: rx_qs,
        rx_last_used: 0,
        rx_repost_pending: 0,
        rx_buffers,
        pci_addr: dev.addr,
        rx_msix_vector,
        tx_q,
        tx_avail,
        tx_used,
        tx_queue_size: tx_qs,
        tx_avail_idx: 0,
        tx_last_used: 0,
        tx_num_free: tx_qs,
        tx_notify_pending: false,
        tx_free_head: 0,
        tx_hdrs,
        tx_data,
        tso_data,
        tso_free: if TSO_SLOTS == 32 { u32::MAX } else { (1u32 << TSO_SLOTS) - 1 },
        tso_slot_of: alloc::vec![NO_TSO_SLOT; tx_qs as usize],
    });

    kprintln!("[npk] virtio-net: online");
    true
}

/// LAPIC vector the host NIC raises on RX (0 = none / polling). The microvm
/// routes this IRQ to its vCPU core so RX arrival wakes the vCPU to pump.
pub fn rx_irq_vector() -> u8 {
    DEVICE.lock().as_ref().map_or(0, |d| d.rx_msix_vector)
}

/// Send an Ethernet frame. `frame` must be a complete Ethernet frame (dst + src + type + payload).
pub fn send(frame: &[u8]) -> Result<(), NetError> {
    if frame.len() > MTU { return Err(NetError::FrameTooLarge); }

    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(NetError::NotInitialized)?;

    // Reclaim completed TX descriptors. Under burst upload the ring fills
    // faster than QEMU/slirp drains it; rather than dropping the frame
    // (→ guest TCP retransmit → upload collapse), spin-reclaim briefly so
    // in-flight descriptors free up. Backpressure, not loss. Bounded.
    dev.reclaim_tx();
    if dev.tx_num_free < 2 {
        let mut spins = 0;
        while dev.tx_num_free < 2 && spins < TX_RECLAIM_SPINS {
            core::hint::spin_loop();
            dev.reclaim_tx();
            spins += 1;
        }
        if dev.tx_num_free < 2 { return Err(NetError::QueueFull); }
    }

    let d0 = dev.alloc_tx_desc().ok_or(NetError::QueueFull)?;
    let d1 = dev.alloc_tx_desc().ok_or(NetError::QueueFull)?;

    let hdr_off = d0 as u64 * NET_HDR_SIZE as u64;

    // Zero net header (no offload)
    dev.tx_hdrs.fill(hdr_off, 0, NET_HDR_SIZE as u64);

    // Descriptor 0: net header
    set_desc(&dev.tx_q, d0, dev.tx_hdrs.phys() + hdr_off, NET_HDR_SIZE as u32, DESC_F_NEXT, d1);

    // Descriptor 1: frame data. Copy into the DMA-stable pool
    // before publishing the descriptor — the caller's `frame: &[u8]`
    // is a stack-local Vec that gets dropped before QEMU/slirp's
    // event loop wakes up to walk our virtqueue.
    let data_off = d1 as u64 * MTU as u64;
    dev.tx_data.copy_in(data_off, frame);

    set_desc(&dev.tx_q, d1, dev.tx_data.phys() + data_off, frame.len() as u32, 0, 0);

    // Add to available ring
    let avail_ring = dev.tx_avail + 4;
    dev.tx_q.w16(avail_ring + (dev.tx_avail_idx % dev.tx_queue_size) as u64 * 2, d0);

    fence(Ordering::SeqCst);
    dev.tx_avail_idx = dev.tx_avail_idx.wrapping_add(1);
    dev.tx_q.w16(dev.tx_avail + 2, dev.tx_avail_idx);

    fence(Ordering::SeqCst);

    // Batch the TX doorbell: an outw() notify is a VM-exit, and notifying per
    // frame would be a per-packet exit on the upload path.
    // Mark pending; net::poll() flushes it once per cycle (tx_flush). Flush
    // immediately only if the ring is filling, so QEMU drains before send() has
    // to spin-reclaim.
    dev.tx_notify_pending = true;
    if dev.tx_num_free < 16 {
        dev.tx_kick();
    }

    Ok(())
}


const VNET_HDR_F_NEEDS_CSUM: u8 = 1;
const VNET_HDR_GSO_TCPV4: u8 = 1;

/// Hand the device a TCPv4 GSO super-frame to segment and checksum — Linux
/// virtio_net `xmit_skb` with `CHECKSUM_PARTIAL` + `SKB_GSO_TCPV4`. The TCP
/// check must hold the pseudo-header seed. Only for real GSO frames: QEMU's
/// legacy F_GSO gets NEEDS_CSUM wrong on small frames, so anything that fits
/// one MSS uses the plain `send` with a full checksum.
pub fn send_tso(frame: &[u8], mss: u16, l4_off: usize, hdr_len: usize) -> Result<(), NetError> {
    if frame.len() > TSO_SLOT_SIZE { return Err(NetError::FrameTooLarge); }
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(NetError::NotInitialized)?;

    // Two descriptors (header, data) and a free super-frame buffer.
    dev.reclaim_tx();
    if dev.tx_num_free < 2 || dev.tso_free == 0 {
        dev.tx_kick();
        let mut spins = 0;
        while (dev.tx_num_free < 2 || dev.tso_free == 0) && spins < TX_RECLAIM_SPINS {
            core::hint::spin_loop();
            dev.reclaim_tx();
            spins += 1;
        }
        if dev.tx_num_free < 2 || dev.tso_free == 0 { return Err(NetError::QueueFull); }
    }
    let slot = dev.tso_free.trailing_zeros() as usize;
    dev.tso_free &= !(1u32 << slot);

    let d0 = dev.alloc_tx_desc().ok_or(NetError::QueueFull)?;
    let d1 = dev.alloc_tx_desc().ok_or(NetError::QueueFull)?;
    dev.tso_slot_of[d0 as usize] = slot as u8;
    let hdr_off = d0 as u64 * NET_HDR_SIZE as u64;
    let data_off = (slot * TSO_SLOT_SIZE) as u64;
    let mut h = [0u8; NET_HDR_SIZE];
    h[0] = VNET_HDR_F_NEEDS_CSUM;
    h[1] = VNET_HDR_GSO_TCPV4;
    h[2..4].copy_from_slice(&(hdr_len as u16).to_le_bytes());
    h[4..6].copy_from_slice(&mss.to_le_bytes());
    h[6..8].copy_from_slice(&(l4_off as u16).to_le_bytes()); // csum_start
    h[8..10].copy_from_slice(&16u16.to_le_bytes());          // csum_offset: tcphdr.check
    // The slot is ours until its chain completes.
    dev.tx_hdrs.copy_in(hdr_off, &h);
    dev.tso_data.copy_in(data_off, frame);
    set_desc(&dev.tx_q, d0, dev.tx_hdrs.phys() + hdr_off, NET_HDR_SIZE as u32, DESC_F_NEXT, d1);
    set_desc(&dev.tx_q, d1, dev.tso_data.phys() + data_off, frame.len() as u32, 0, 0);

    // Publish the chain head (d0) on the avail ring.
    let avail_ring = dev.tx_avail + 4;
    dev.tx_q.w16(avail_ring + (dev.tx_avail_idx % dev.tx_queue_size) as u64 * 2, d0);
    fence(Ordering::SeqCst);
    dev.tx_avail_idx = dev.tx_avail_idx.wrapping_add(1);
    dev.tx_q.w16(dev.tx_avail + 2, dev.tx_avail_idx);
    fence(Ordering::SeqCst);
    dev.tx_notify_pending = true;
    if dev.tx_num_free < 16 || dev.tso_free.count_ones() < 4 { dev.tx_kick(); }
    Ok(())
}

/// Receive an Ethernet frame. Returns frame data (without virtio net header).
/// Returns None if no packet available.
pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut()?;

    let used_idx_off = dev.rx_used + 2;
    let used_idx = dev.rx_q.r16(used_idx_off);

    if used_idx == dev.rx_last_used {
        // Ring appears drained. NAPI: if RX IRQs are enabled (MSI-X on) and we
        // had suppressed them during a drain (avail.flags==1), this is the
        // drain end → re-enable + re-check once (a frame may have landed in the
        // window) before parking, so no wakeup is lost. If already enabled
        // (flags==0), the next RX will interrupt — nothing to do.
        if dev.rx_msix_vector != 0 {
            let flags = dev.rx_q.r16(dev.rx_avail);
            if flags != 0 {
                dev.rx_q.w16(dev.rx_avail, 0);
                fence(Ordering::SeqCst);
                let used2 = dev.rx_q.r16(used_idx_off);
                if used2 == dev.rx_last_used {
                    dev.rx_kick();
                    return None; // truly empty, IRQ re-armed for the next frame
                }
                // A frame arrived in the window — re-suppress + fall through.
                dev.rx_q.w16(dev.rx_avail, 1);
            } else {
                dev.rx_kick();
                return None;
            }
        } else {
            // Ring drained — ring the doorbell once for everything reposted
            // during this drain (batched notify, not one VM-exit per packet).
            dev.rx_kick();
            return None; // no new packets
        }
    } else if dev.rx_msix_vector != 0 {
        // Frames present → we're draining; suppress further RX IRQs until the
        // ring empties (NAPI), so the device doesn't interrupt per packet.
        dev.rx_q.w16(dev.rx_avail, 1);
    }

    // Read the used ring entry
    let used_entry_off = dev.rx_used + 4 + (dev.rx_last_used % dev.rx_queue_size) as u64 * 8;
    let used_id = dev.rx_q.r32(used_entry_off);
    let used_len = dev.rx_q.r32(used_entry_off + 4) as usize;

    dev.rx_last_used = dev.rx_last_used.wrapping_add(1);

    // The buffer contains: [VirtioNetHdr (10 bytes)][Ethernet frame]
    if used_len <= NET_HDR_SIZE {
        // Repost buffer
        dev.repost_rx(used_id as usize);
        return None;
    }

    let frame_len = used_len - NET_HDR_SIZE;
    let frame_len = frame_len.min(MTU);

    let buf_off = (used_id as usize * RX_BUF_SIZE) as u64;
    dev.rx_buffers.copy_out(buf_off + NET_HDR_SIZE as u64, &mut buf[..frame_len]);

    // Repost buffer for next receive
    dev.repost_rx(used_id as usize);

    Some(frame_len)
}

pub fn mac() -> Option<[u8; 6]> {
    DEVICE.lock().as_ref().map(|d| d.mac)
}

/// Ring the deferred TX doorbell, if any frames were queued since the last kick.
/// Called once per net::poll() cycle so per-frame send() avoids a VM-exit each.
pub fn tx_flush() {
    if let Some(dev) = DEVICE.lock().as_mut() {
        dev.tx_kick();
    }
}

#[allow(dead_code)]
pub fn is_available() -> bool {
    DEVICE.lock().is_some()
}

// === Internal ===

impl VirtioNet {
    fn alloc_tx_desc(&mut self) -> Option<u16> {
        if self.tx_num_free == 0 { return None; }
        let idx = self.tx_free_head;
        self.tx_free_head = self.tx_q.r16(idx as u64 * DESC_SIZE + 14);
        self.tx_num_free -= 1;
        Some(idx)
    }

    fn free_tx_desc(&mut self, idx: u16) {
        let o = idx as u64 * DESC_SIZE;
        self.tx_q.w16(o + 12, 0);
        self.tx_q.w16(o + 14, self.tx_free_head);
        self.tx_free_head = idx;
        self.tx_num_free += 1;
    }

    fn reclaim_tx(&mut self) {
        let used_idx_off = self.tx_used + 2;
        loop {
            let used_idx = self.tx_q.r16(used_idx_off);
            if used_idx == self.tx_last_used { break; }

            let entry_off = self.tx_used + 4 + (self.tx_last_used % self.tx_queue_size) as u64 * 8;
            let id = self.tx_q.r32(entry_off) as u16;
            if let Some(slot) = self.tso_slot_of.get_mut(id as usize) {
                if *slot != NO_TSO_SLOT {
                    self.tso_free |= 1u32 << *slot;
                    *slot = NO_TSO_SLOT;
                }
            }

            // Free the WHOLE descriptor chain. A plain frame is hdr→data (2); an
            // offloaded GSO super-frame spans hdr→data→data→… (many). Walk
            // DESC_F_NEXT, reading (flags,next) BEFORE freeing (free_tx_desc
            // overwrites next with the free-list head).
            let mut cur = id;
            loop {
                let o = cur as u64 * DESC_SIZE;
                let (flags, next) = (self.tx_q.r16(o + 12), self.tx_q.r16(o + 14));
                self.free_tx_desc(cur);
                if flags & DESC_F_NEXT == 0 { break; }
                cur = next;
            }

            self.tx_last_used = self.tx_last_used.wrapping_add(1);
        }
        // Reading ISR deasserts the legacy INTx line. Under MSI-X there is
        // no line to deassert, and on QEMU the port read is an exit — once
        // per frame sent, since every send reclaims first.
        if self.rx_msix_vector == 0 {
            self.io.inb(REG_ISR);
        }
    }

    fn repost_rx(&mut self, desc_idx: usize) {
        // Re-add this buffer to the RX available ring — but DON'T ring the
        // doorbell per packet. An `outw` to the notify port is a VM-exit; doing
        // it once per received packet costs a VM-exit per packet. We publish the buffer to
        // the avail ring now and batch the notify (see recv): one doorbell per
        // drain instead of per packet. A mid-burst safety notify keeps the device
        // from running dry if a caller doesn't drain to empty.
        let avail_ring = self.rx_avail + 4;
        let avail_idx_off = self.rx_avail + 2;

        let idx = self.rx_q.r16(avail_idx_off);
        self.rx_q.w16(avail_ring + (idx % self.rx_queue_size) as u64 * 2, desc_idx as u16);
        fence(Ordering::SeqCst);
        self.rx_q.w16(avail_idx_off, idx.wrapping_add(1));
        self.rx_repost_pending += 1;
        if self.rx_repost_pending >= 64 {
            self.rx_kick();
        }
    }

    // Ring the TX doorbell once for all frames queued since the last kick.
    fn tx_kick(&mut self) {
        if !self.tx_notify_pending {
            return;
        }
        fence(Ordering::SeqCst);
        self.io.outw(REG_QUEUE_NOTIFY, TX_QUEUE);
        self.tx_notify_pending = false;
    }

    // Ring the RX doorbell once for all buffers reposted since the last kick.
    fn rx_kick(&mut self) {
        if self.rx_repost_pending == 0 {
            return;
        }
        fence(Ordering::SeqCst);
        self.io.outw(REG_QUEUE_NOTIFY, RX_QUEUE);
        self.rx_repost_pending = 0;
    }
}

/// Allocate and register the selected queue: (memory, avail offset, used offset).
fn setup_queue(io: &PortRange, qs: u16) -> Option<(DmaRegion, u64, u64)> {
    let q = qs as usize;
    let part1 = align_up(16 * q + 6 + 2 * q, 4096);
    let part2 = align_up(6 + 8 * q, 4096);
    let pages = (part1 + part2 + 4095) / 4096;

    let qmem = DmaRegion::alloc_zeroed(pages)?;

    let avail = (16 * q) as u64;
    let used = part1 as u64;

    io.outl(REG_QUEUE_PFN, (qmem.phys() >> 12) as u32);

    Some((qmem, avail, used))
}

fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}

#[derive(Debug)]
pub enum NetError {
    NotInitialized,
    FrameTooLarge,
    QueueFull,
}

impl core::fmt::Display for NetError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            NetError::NotInitialized => write!(f, "network not initialized"),
            NetError::FrameTooLarge => write!(f, "frame too large"),
            NetError::QueueFull => write!(f, "TX queue full"),
        }
    }
}
