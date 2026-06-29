//! VirtIO Network Device Driver
//!
//! Legacy (0.9.5) VirtIO PCI transport with RX/TX virtqueues.
//! Provides Ethernet frame send/receive for the TCP/IP stack.

use core::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};

/// Host TX offload (VIRTIO_NET_F_CSUM + _HOST_TSO4) negotiated with QEMU. When
/// set, the masquerade TX path forwards the guest's GSO super-frame to the host
/// NIC AS-IS (device does checksum + TCP segmentation) — the symmetric twin of RX
/// (`inject_rx`): rewrite the address, carry the virtio_net_hdr, forward. No
/// per-segment SW re-segmentation/checksum. Falls back to `emit_tcp_out` if unset.
static HOST_OFFLOAD: AtomicBool = AtomicBool::new(false);

/// Master switch for the TX offload path. DISABLED: first HW test hung the
/// download after the TCP handshake — the offloaded data frame is malformed for
/// QEMU's legacy F_GSO (SYN went through, then the connection stalled). The path
/// (negotiation + send_offload + l3_outbound_offload) stays in the tree for
/// debugging; flip true to retest. With it false, TX falls back to the working
/// SW path (`emit_tcp_out`).
const TX_OFFLOAD_ENABLED: bool = false;

/// True iff the host NIC accepts offloaded (CSUM+TSO) TX frames via `send_offload`.
pub fn host_offload_ok() -> bool {
    TX_OFFLOAD_ENABLED && HOST_OFFLOAD.load(Ordering::Relaxed)
}

/// Lock-free pointer to the host NIC RX used.idx (`rx_used_base + 2`), published
/// at init. Lets the off-vCPU data-plane busy-poll for RX arrival WITHOUT taking
/// the DEVICE lock every spin iteration (the lock-hammer that made the earlier
/// worker-spin net-negative). 0 = not yet up.
static RX_USED_IDX_PTR: AtomicU64 = AtomicU64::new(0);

/// Current host NIC RX used.idx (lock-free volatile read). The data-plane worker
/// caches the value it last drained to; a change means a frame arrived.
#[inline]
pub fn rx_used_idx() -> u16 {
    let p = RX_USED_IDX_PTR.load(Ordering::Relaxed);
    if p == 0 { return 0; }
    // SAFETY: p is the device's used-ring idx field, identity-mapped, set once
    // at init and stable for the device's life; a torn 16-bit read at worst
    // costs one extra poll iteration.
    unsafe { core::ptr::read_volatile(p as *const u16) }
}
use spin::Mutex;
use crate::serial::{outb, outw, outl, inb, inw, inl};
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
const VNET_HDR_F_NEEDS_CSUM: u8 = 1;
const VNET_HDR_GSO_TCPV4: u8    = 1;
#[allow(dead_code)]
const F_STATUS: u32    = 1 << 16;

const DESC_F_NEXT: u16  = 1;
const DESC_F_WRITE: u16 = 2;

const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;
// RX ring depth. 32 buffers (~48 KB) under-runs at high throughput: if
// `net::poll()` can't drain in time (BSP vCPU pump spinning, Core 0
// compositing, or the POLLING guard contended), the ring fills and
// QEMU/slirp DROPS inbound frames → guest TCP sees loss → backs off →
// throughput collapse + latency spikes. 256 buffers (~387 KB ≈ 3 ms at 1 Gbit)
// was too shallow: a download burst that outran a brief drain gap overflowed the
// host NIC RX ring → QEMU/slirp DROPPED frames → the SERVER retransmitted →
// its cwnd collapsed (the measured per-connection download lottery: server
// total_retrans 6-14 on slow runs, 0 on fast). 1024 gives ~12 ms of cushion so
// a transient burst is absorbed instead of lost. Needs QEMU `rx_queue_size=1024`
// (else capped to the device's actual queue size via `RX_BUFFERS.min(rx_qs)`).
const RX_BUFFERS: usize = 1024;

// On a full TX ring, spin-reclaim this many times before dropping the
// frame. QEMU/slirp drains the ring on its own host thread, so a bounded
// busy-wait lets in-flight descriptors complete instead of dropping the
// guest's packet (→ TCP retransmit → upload throughput collapse). Bounded
// so a genuinely wedged device can't hang the sender.
const TX_RECLAIM_SPINS: u32 = 4096;
pub const MTU: usize = 1514; // Ethernet max frame

/// VirtIO net header prepended to every packet (10 bytes, no mergeable buffers)
#[repr(C)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
}

const NET_HDR_SIZE: usize = core::mem::size_of::<VirtioNetHdr>(); // 10
const RX_BUF_SIZE: usize = NET_HDR_SIZE + MTU;

#[repr(C)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[allow(dead_code)]
struct VirtioNet {
    io_base: u16,
    mac: [u8; 6],

    // RX queue
    rx_desc_base: u64,
    rx_avail_base: u64,
    rx_used_base: u64,
    rx_queue_size: u16,
    rx_last_used: u16,
    rx_buffers: u64, // contiguous RX buffer region
    rx_repost_pending: u16, // RX buffers reposted but not yet notified (batch the doorbell)
    pci_addr: pci::PciAddr, // for MSI-X dest re-routing (net-RX IRQ)
    rx_msix_vector: u8,     // LAPIC vector for RX-queue MSI-X (0 = none, polling)

    // TX queue
    tx_desc_base: u64,
    tx_avail_base: u64,
    tx_used_base: u64,
    tx_queue_size: u16,
    tx_avail_idx: u16,
    tx_last_used: u16,
    tx_num_free: u16,
    tx_notify_pending: bool, // TX frames queued but doorbell not yet rung (batch it)
    tx_free_head: u16,
    tx_hdrs: u64,   // pre-allocated net headers for TX
    /// Pre-allocated DMA-stable TX data pool — one MTU-sized slot per
    /// descriptor. `send` copies the caller's frame here so the device
    /// reads from memory that outlives the caller's stack-local `Vec`
    /// (which otherwise would be dropped + heap-recycled before QEMU's
    /// slirp main loop wakes up to do the DMA, on real HW this race
    /// happens to lose less often because the NIC engine reads
    /// synchronously). Modelled on `intel_nic::send`'s `tx_bufs`.
    tx_data: u64,
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
    let io = (dev.bar0 & 0xFFFC) as u16;
    pci::enable_bus_master(dev.addr);
    // Try to enable MSI-X for the RX queue → the device raises an interrupt on
    // RX so delivery is event-driven (a drain/wake) instead of pump-cadence
    // polling. `register` enables the MSI-X PCI cap (which shifts the legacy
    // config layout: device config moves 20 → 24). Falls back to polling if
    // the device has no usable MSI-X.
    //
    // net-RX wired end-to-end: (1) here — enable RX MSI-X; (2) nat::pump routes
    // the IRQ to the vCPU/BSP-pump core; (3) the IRQ wakes that core out of HLT
    // → the run-loop pump delivers RX promptly (event-driven, not pump-cadence-
    // bound); (4) recv() does NAPI-style RX-IRQ suppression during the drain so
    // the device doesn't interrupt per packet.
    const NET_RX_IRQ_ENABLED: bool = true;
    let rx_vec = if NET_RX_IRQ_ENABLED {
        crate::irq::register(dev.addr, 0).unwrap_or(0)
    } else {
        0
    };
    let cfg_off: u16 = if rx_vec != 0 || pci::msix_enabled(dev.addr) { 24 } else { 20 };

    // SAFETY: All port I/O targets the VirtIO device's I/O BAR
    unsafe {
        outb(io + REG_STATUS, 0);
        outb(io + REG_STATUS, S_ACKNOWLEDGE);
        outb(io + REG_STATUS, S_ACKNOWLEDGE | S_DRIVER);

        let features = inl(io + REG_DEV_FEATURES);

        // Accept MAC, plus CSUM+TSO4 offload if BOTH are offered (needed to
        // forward the guest's GSO super-frames AS-IS — symmetric TX, see
        // `send_offload`). Only enable offload when both are present so we never
        // promise a GSO frame the device can't segment.
        let mut accepted = features & F_MAC;
        // Prefer the modern per-type bits; fall back to the legacy combined F_GSO
        // (what QEMU's transitional device offers a legacy driver). Either lets us
        // forward the guest's GSO super-frame AS-IS (device segments + checksums).
        let modern = (features & (F_CSUM | F_HOST_TSO4)) == (F_CSUM | F_HOST_TSO4);
        let legacy_gso = features & F_GSO != 0;
        let offload = modern || legacy_gso;
        kprintln!("[npk] virtio-net: dev features {:#010x} (csum={} gso={} host_tso4={} → offload={})",
                  features, features & F_CSUM != 0, legacy_gso, features & F_HOST_TSO4 != 0, offload);
        if modern {
            accepted |= F_CSUM | F_HOST_TSO4;
        } else if legacy_gso {
            // F_GSO bundles checksum; accept F_CSUM too if the device lists it.
            accepted |= F_GSO | (features & F_CSUM);
        }
        if offload {
            kprintln!("[npk] virtio-net: TX offload negotiated ({})",
                      if modern { "CSUM+HOST_TSO4" } else { "legacy GSO" });
        }
        HOST_OFFLOAD.store(offload, Ordering::Relaxed);
        outl(io + REG_DRV_FEATURES, accepted);

        // Read MAC address
        let mut mac = [0u8; 6];
        if features & F_MAC != 0 {
            for i in 0..6 {
                mac[i] = inb(io + cfg_off + i as u16);
            }
        }

        // Setup RX queue (queue 0)
        outw(io + REG_QUEUE_SEL, RX_QUEUE);
        let rx_qs = inw(io + REG_QUEUE_SIZE);
        if rx_qs == 0 {
            kprintln!("[npk] virtio-net: RX queue size 0");
            outb(io + REG_STATUS, S_FAILED);
            return false;
        }

        let (rx_desc, rx_avail, rx_used, _rx_mem) = match setup_queue(io, rx_qs) {
            Some(v) => v,
            None => {
                outb(io + REG_STATUS, S_FAILED);
                return false;
            }
        };

        // Setup TX queue (queue 1)
        outw(io + REG_QUEUE_SEL, TX_QUEUE);
        let tx_qs = inw(io + REG_QUEUE_SIZE);
        if tx_qs == 0 {
            kprintln!("[npk] virtio-net: TX queue size 0");
            outb(io + REG_STATUS, S_FAILED);
            return false;
        }

        let (tx_desc, tx_avail, tx_used, _tx_mem) = match setup_queue(io, tx_qs) {
            Some(v) => v,
            None => {
                outb(io + REG_STATUS, S_FAILED);
                return false;
            }
        };

        // Build TX descriptor free chain
        for i in 0..tx_qs as usize {
            let d = (tx_desc + (i * 16) as u64) as *mut VringDesc;
            (*d).next = if i + 1 < tx_qs as usize { (i + 1) as u16 } else { 0 };
        }

        // Allocate TX net headers (one per descriptor)
        let tx_hdrs = match memory::allocate_contiguous(
            (tx_qs as usize * NET_HDR_SIZE + 4095) / 4096
        ) {
            Some(a) => a,
            None => {
                outb(io + REG_STATUS, S_FAILED);
                return false;
            }
        };
        core::ptr::write_bytes(tx_hdrs as *mut u8, 0,
            (tx_qs as usize * NET_HDR_SIZE + 4095) / 4096 * 4096);

        // Allocate TX data pool — one MTU slot per descriptor. The
        // sender copies the frame here so the descriptor points at
        // memory that outlives the caller's stack frame. Without this
        // QEMU + slirp races the heap allocator and DMA-reads recycled
        // garbage (intermittent under real HW, deterministic under
        // QEMU on AMD-host where slirp's event loop wakes up later).
        let tx_data_pages = (tx_qs as usize * MTU + 4095) / 4096;
        let tx_data = match memory::allocate_contiguous(tx_data_pages) {
            Some(a) => a,
            None => {
                outb(io + REG_STATUS, S_FAILED);
                return false;
            }
        };
        core::ptr::write_bytes(tx_data as *mut u8, 0, tx_data_pages * 4096);

        // Allocate RX buffers (contiguous, one per RX descriptor)
        let rx_buf_count = RX_BUFFERS.min(rx_qs as usize);
        let rx_buf_pages = (rx_buf_count * RX_BUF_SIZE + 4095) / 4096;
        let rx_buffers = match memory::allocate_contiguous(rx_buf_pages) {
            Some(a) => a,
            None => {
                outb(io + REG_STATUS, S_FAILED);
                return false;
            }
        };
        core::ptr::write_bytes(rx_buffers as *mut u8, 0, rx_buf_pages * 4096);

        // Post RX buffers to the RX queue
        for i in 0..rx_buf_count {
            let buf_addr = rx_buffers + (i * RX_BUF_SIZE) as u64;
            let d = (rx_desc + (i * 16) as u64) as *mut VringDesc;
            (*d).addr = buf_addr;
            (*d).len = RX_BUF_SIZE as u32;
            (*d).flags = DESC_F_WRITE;
            (*d).next = 0;

            // Add to available ring
            let avail_ring = rx_avail + 4;
            let slot = (avail_ring + (i as u64) * 2) as *mut u16;
            *slot = i as u16;
        }

        // Set available ring idx
        fence(Ordering::SeqCst);
        let rx_avail_idx_ptr = (rx_avail + 2) as *mut u16;
        core::ptr::write_volatile(rx_avail_idx_ptr, rx_buf_count as u16);

        // Suppress TX interrupts
        *(tx_avail as *mut u16) = 1;

        // Bind the RX queue to MSI-X table entry 0 so the device fires our
        // vector on RX. config_msix_vector = NO_VECTOR (we don't want a
        // config-change IRQ). Read-back guards a device that rejects it →
        // rx_msix_vector stays 0 and we keep polling (the recv path is
        // unchanged either way).
        let mut rx_msix_vector = 0u8;
        if rx_vec != 0 {
            outw(io + REG_CONFIG_MSIX_VEC, MSIX_NO_VECTOR);
            outw(io + REG_QUEUE_SEL, RX_QUEUE);
            outw(io + REG_QUEUE_MSIX_VEC, 0);
            if inw(io + REG_QUEUE_MSIX_VEC) == 0 {
                rx_msix_vector = rx_vec;
                kprintln!("[npk] virtio-net: RX MSI-X on vector {:#04x}", rx_vec);
            } else {
                kprintln!("[npk] virtio-net: RX MSI-X vector rejected — polling");
            }
        }

        // Go live
        outb(io + REG_STATUS, S_ACKNOWLEDGE | S_DRIVER | S_DRIVER_OK);
        if inb(io + REG_STATUS) & S_FAILED != 0 {
            kprintln!("[npk] virtio-net: device rejected initialization");
            return false;
        }

        // Notify RX queue that buffers are available
        outw(io + REG_QUEUE_NOTIFY, RX_QUEUE);

        kprintln!("[npk] virtio-net: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

        *DEVICE.lock() = Some(VirtioNet {
            io_base: io,
            mac,
            rx_desc_base: rx_desc,
            rx_avail_base: rx_avail,
            rx_used_base: { RX_USED_IDX_PTR.store(rx_used + 2, Ordering::Relaxed); rx_used },
            rx_queue_size: rx_qs,
            rx_last_used: 0,
            rx_repost_pending: 0,
            rx_buffers,
            pci_addr: dev.addr,
            rx_msix_vector,
            tx_desc_base: tx_desc,
            tx_avail_base: tx_avail,
            tx_used_base: tx_used,
            tx_queue_size: tx_qs,
            tx_avail_idx: 0,
            tx_last_used: 0,
            tx_num_free: tx_qs,
            tx_notify_pending: false,
            tx_free_head: 0,
            tx_hdrs,
            tx_data,
        });
    }

    kprintln!("[npk] virtio-net: online");
    true
}

/// LAPIC vector the host NIC raises on RX (0 = none / polling). The microvm
/// routes this IRQ to its vCPU core (step 2) so RX arrival wakes the vCPU to
/// pump. Returns 0 until net-RX is enabled end-to-end.
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

    let hdr_addr = dev.tx_hdrs + d0 as u64 * NET_HDR_SIZE as u64;

    // SAFETY: Writing to pre-allocated DMA buffers
    unsafe {
        // Zero net header (no offload)
        core::ptr::write_bytes(hdr_addr as *mut u8, 0, NET_HDR_SIZE);

        // Descriptor 0: net header
        let desc0 = (dev.tx_desc_base + d0 as u64 * 16) as *mut VringDesc;
        (*desc0).addr = hdr_addr;
        (*desc0).len = NET_HDR_SIZE as u32;
        (*desc0).flags = DESC_F_NEXT;
        (*desc0).next = d1;

        // Descriptor 1: frame data. Copy into the DMA-stable pool
        // before publishing the descriptor — the caller's `frame: &[u8]`
        // is a stack-local Vec that gets dropped before QEMU/slirp's
        // event loop wakes up to walk our virtqueue.
        let data_addr = dev.tx_data + d1 as u64 * MTU as u64;
        core::ptr::copy_nonoverlapping(frame.as_ptr(), data_addr as *mut u8, frame.len());

        let desc1 = (dev.tx_desc_base + d1 as u64 * 16) as *mut VringDesc;
        (*desc1).addr = data_addr;
        (*desc1).len = frame.len() as u32;
        (*desc1).flags = 0;
        (*desc1).next = 0;

        // Add to available ring
        let avail_ring = dev.tx_avail_base + 4;
        let slot = (avail_ring + (dev.tx_avail_idx % dev.tx_queue_size) as u64 * 2) as *mut u16;
        core::ptr::write_volatile(slot, d0);

        fence(Ordering::SeqCst);
        let avail_idx_ptr = (dev.tx_avail_base + 2) as *mut u16;
        dev.tx_avail_idx = dev.tx_avail_idx.wrapping_add(1);
        core::ptr::write_volatile(avail_idx_ptr, dev.tx_avail_idx);

        fence(Ordering::SeqCst);
    }

    // Batch the TX doorbell: an outw() notify is a VM-exit, and notifying per
    // frame was a per-packet exit on the upload path (the asymmetry vs download).
    // Mark pending; net::poll() flushes it once per cycle (tx_flush). Flush
    // immediately only if the ring is filling, so QEMU drains before send() has
    // to spin-reclaim.
    dev.tx_notify_pending = true;
    if dev.tx_num_free < 16 {
        dev.tx_kick();
    }

    Ok(())
}

/// Send an Ethernet frame to the host NIC WITH a 10-byte virtio_net_hdr (CSUM/TSO
/// offload) — the symmetric TX twin of RX `inject_rx`: forward the frame as-is and
/// let the DEVICE do the checksum + TCP segmentation, instead of re-segmenting +
/// SW-checksumming each MSS in our hot path. `frame` (ethernet) may be a GSO
/// super-frame up to ~64 KB; it is split across CHAINED MTU-sized descriptors
/// (hdr → data → data → …), so no large contiguous DMA buffer is needed. Only
/// valid when `host_offload_ok()`; the caller falls back to `send()` otherwise.
///
/// `gso_size` = TCP MSS (0 ⇒ checksum-only, no segmentation). `csum_start` =
/// byte offset of the TCP header from the ethernet frame start (14 + IHL);
/// `csum_offset` = 16 (TCP checksum field); `hdr_len` = csum_start + TCP header
/// length. The frame's TCP checksum field must already hold the pseudo-header
/// partial (NEEDS_CSUM); the device completes it.
pub fn send_offload(frame: &[u8], gso_size: u16, csum_start: u16,
                    csum_offset: u16, hdr_len: u16) -> Result<(), NetError> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(NetError::NotInitialized)?;

    let nchunks = frame.len().div_ceil(MTU).max(1);
    let need = (nchunks + 1) as u16; // 1 hdr descriptor + one per data chunk

    dev.reclaim_tx();
    if dev.tx_num_free < need {
        let mut spins = 0;
        while dev.tx_num_free < need && spins < TX_RECLAIM_SPINS {
            core::hint::spin_loop();
            dev.reclaim_tx();
            spins += 1;
        }
        if dev.tx_num_free < need { return Err(NetError::QueueFull); }
    }

    // hdr descriptor (d0): the 10-byte virtio_net_hdr (CSUM + optional TCP-GSO).
    let d0 = dev.alloc_tx_desc().ok_or(NetError::QueueFull)?;
    let hdr_addr = dev.tx_hdrs + d0 as u64 * NET_HDR_SIZE as u64;
    let mut h = [0u8; NET_HDR_SIZE];
    // GSO frames NEED the device to checksum (the per-segment checksums don't
    // exist yet). Non-GSO frames carry a complete SW checksum already (this
    // device's legacy F_GSO NEEDS_CSUM proved unreliable for small frames), so
    // they go out as a plain frame — no offload flag, csum fields zero.
    if gso_size > 0 {
        h[0] = VNET_HDR_F_NEEDS_CSUM;
        h[1] = VNET_HDR_GSO_TCPV4;
        h[2..4].copy_from_slice(&hdr_len.to_le_bytes());
        h[4..6].copy_from_slice(&gso_size.to_le_bytes());
        h[6..8].copy_from_slice(&csum_start.to_le_bytes());
        h[8..10].copy_from_slice(&csum_offset.to_le_bytes());
    }
    // SAFETY: pre-allocated DMA buffers + this core's own descriptor table.
    unsafe {
        core::ptr::copy_nonoverlapping(h.as_ptr(), hdr_addr as *mut u8, NET_HDR_SIZE);
        let dd0 = (dev.tx_desc_base + d0 as u64 * 16) as *mut VringDesc;
        (*dd0).addr = hdr_addr;
        (*dd0).len = NET_HDR_SIZE as u32;
        (*dd0).flags = 0; // linked below
        (*dd0).next = 0;
    }

    // Data descriptors (one MTU chunk each), chained off d0.
    let mut prev = d0;
    let mut off = 0usize;
    for _ in 0..nchunks {
        let di = dev.alloc_tx_desc().ok_or(NetError::QueueFull)?;
        let take = (frame.len() - off).min(MTU);
        let data_addr = dev.tx_data + di as u64 * MTU as u64;
        // SAFETY: as above; `take` ≤ MTU = the slot size.
        unsafe {
            core::ptr::copy_nonoverlapping(frame[off..off + take].as_ptr(),
                                           data_addr as *mut u8, take);
            let dd = (dev.tx_desc_base + di as u64 * 16) as *mut VringDesc;
            (*dd).addr = data_addr;
            (*dd).len = take as u32;
            (*dd).flags = 0;
            (*dd).next = 0;
            // Link prev → di (prev keeps its addr/len).
            let dp = (dev.tx_desc_base + prev as u64 * 16) as *mut VringDesc;
            (*dp).flags |= DESC_F_NEXT;
            (*dp).next = di;
        }
        prev = di;
        off += take;
    }

    // Publish the chain head (d0) on the avail ring.
    // SAFETY: this core's own virtqueue memory; release-fenced.
    unsafe {
        let avail_ring = dev.tx_avail_base + 4;
        let slot = (avail_ring + (dev.tx_avail_idx % dev.tx_queue_size) as u64 * 2) as *mut u16;
        core::ptr::write_volatile(slot, d0);
        fence(Ordering::SeqCst);
        let avail_idx_ptr = (dev.tx_avail_base + 2) as *mut u16;
        dev.tx_avail_idx = dev.tx_avail_idx.wrapping_add(1);
        core::ptr::write_volatile(avail_idx_ptr, dev.tx_avail_idx);
        fence(Ordering::SeqCst);
    }
    dev.tx_notify_pending = true;
    if dev.tx_num_free < 16 { dev.tx_kick(); }
    Ok(())
}

/// Receive an Ethernet frame. Returns frame data (without virtio net header).
/// Returns None if no packet available.
pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut()?;

    let used_idx_ptr = (dev.rx_used_base + 2) as *const u16;
    let used_idx = unsafe { core::ptr::read_volatile(used_idx_ptr) };

    if used_idx == dev.rx_last_used {
        // Ring appears drained. NAPI: if RX IRQs are enabled (MSI-X on) and we
        // had suppressed them during a drain (avail.flags==1), this is the
        // drain end → re-enable + re-check once (a frame may have landed in the
        // window) before parking, so no wakeup is lost. If already enabled
        // (flags==0), the next RX will interrupt — nothing to do.
        if dev.rx_msix_vector != 0 {
            let flags = unsafe { core::ptr::read_volatile(dev.rx_avail_base as *const u16) };
            if flags != 0 {
                unsafe { core::ptr::write_volatile(dev.rx_avail_base as *mut u16, 0); }
                fence(Ordering::SeqCst);
                let used2 = unsafe { core::ptr::read_volatile(used_idx_ptr) };
                if used2 == dev.rx_last_used {
                    dev.rx_kick();
                    return None; // truly empty, IRQ re-armed for the next frame
                }
                // A frame arrived in the window — re-suppress + fall through.
                unsafe { core::ptr::write_volatile(dev.rx_avail_base as *mut u16, 1); }
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
        unsafe { core::ptr::write_volatile(dev.rx_avail_base as *mut u16, 1); }
    }

    // Read the used ring entry
    let used_entry_off = 4 + (dev.rx_last_used % dev.rx_queue_size) as u64 * 8;
    let used_id = unsafe {
        *((dev.rx_used_base + used_entry_off) as *const u32)
    };
    let used_len = unsafe {
        *((dev.rx_used_base + used_entry_off + 4) as *const u32)
    } as usize;

    dev.rx_last_used = dev.rx_last_used.wrapping_add(1);

    // The buffer contains: [VirtioNetHdr (10 bytes)][Ethernet frame]
    if used_len <= NET_HDR_SIZE {
        // Repost buffer
        dev.repost_rx(used_id as usize);
        return None;
    }

    let frame_len = used_len - NET_HDR_SIZE;
    let frame_len = frame_len.min(MTU);

    let buf_addr = dev.rx_buffers + (used_id as usize * RX_BUF_SIZE) as u64;
    // SAFETY: Reading from DMA buffer in identity-mapped range
    unsafe {
        core::ptr::copy_nonoverlapping(
            (buf_addr + NET_HDR_SIZE as u64) as *const u8,
            buf.as_mut_ptr(),
            frame_len,
        );
    }

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
        unsafe {
            let d = (self.tx_desc_base + idx as u64 * 16) as *const VringDesc;
            self.tx_free_head = (*d).next;
        }
        self.tx_num_free -= 1;
        Some(idx)
    }

    fn free_tx_desc(&mut self, idx: u16) {
        unsafe {
            let d = (self.tx_desc_base + idx as u64 * 16) as *mut VringDesc;
            (*d).flags = 0;
            (*d).next = self.tx_free_head;
        }
        self.tx_free_head = idx;
        self.tx_num_free += 1;
    }

    fn reclaim_tx(&mut self) {
        let used_idx_ptr = (self.tx_used_base + 2) as *const u16;
        loop {
            let used_idx = unsafe { core::ptr::read_volatile(used_idx_ptr) };
            if used_idx == self.tx_last_used { break; }

            let entry_off = 4 + (self.tx_last_used % self.tx_queue_size) as u64 * 8;
            let id = unsafe { *((self.tx_used_base + entry_off) as *const u32) } as u16;

            // Free the WHOLE descriptor chain. A plain frame is hdr→data (2); an
            // offloaded GSO super-frame spans hdr→data→data→… (many). Walk
            // DESC_F_NEXT, reading (flags,next) BEFORE freeing (free_tx_desc
            // overwrites next with the free-list head).
            let mut cur = id;
            loop {
                let (flags, next) = unsafe {
                    let d = (self.tx_desc_base + cur as u64 * 16) as *const VringDesc;
                    ((*d).flags, (*d).next)
                };
                self.free_tx_desc(cur);
                if flags & DESC_F_NEXT == 0 { break; }
                cur = next;
            }

            self.tx_last_used = self.tx_last_used.wrapping_add(1);
        }
        unsafe { inb(self.io_base + REG_ISR); }
    }

    fn repost_rx(&mut self, desc_idx: usize) {
        // Re-add this buffer to the RX available ring — but DON'T ring the
        // doorbell per packet. An `outw` to the notify port is a VM-exit; doing
        // it once per received packet costs a VM-exit per packet (~100k/s under
        // load) and was a major throughput/latency tax. We publish the buffer to
        // the avail ring now and batch the notify (see recv): one doorbell per
        // drain instead of per packet. A mid-burst safety notify keeps the device
        // from running dry if a caller doesn't drain to empty.
        let avail_ring = self.rx_avail_base + 4;
        let avail_idx_ptr = (self.rx_avail_base + 2) as *mut u16;

        unsafe {
            let idx = core::ptr::read_volatile(avail_idx_ptr);
            let slot = (avail_ring + (idx % self.rx_queue_size) as u64 * 2) as *mut u16;
            core::ptr::write_volatile(slot, desc_idx as u16);
            fence(Ordering::SeqCst);
            core::ptr::write_volatile(avail_idx_ptr, idx.wrapping_add(1));
        }
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
        unsafe { outw(self.io_base + REG_QUEUE_NOTIFY, TX_QUEUE); }
        self.tx_notify_pending = false;
    }

    // Ring the RX doorbell once for all buffers reposted since the last kick.
    fn rx_kick(&mut self) {
        if self.rx_repost_pending == 0 {
            return;
        }
        fence(Ordering::SeqCst);
        unsafe { outw(self.io_base + REG_QUEUE_NOTIFY, RX_QUEUE); }
        self.rx_repost_pending = 0;
    }
}

unsafe fn setup_queue(io: u16, qs: u16) -> Option<(u64, u64, u64, u64)> {
    let q = qs as usize;
    let part1 = align_up(16 * q + 6 + 2 * q, 4096);
    let part2 = align_up(6 + 8 * q, 4096);
    let pages = (part1 + part2 + 4095) / 4096;

    let qmem = memory::allocate_contiguous(pages)?;
    unsafe { core::ptr::write_bytes(qmem as *mut u8, 0, pages * 4096); }

    let desc_base = qmem;
    let avail_base = qmem + (16 * q) as u64;
    let used_base = qmem + part1 as u64;

    unsafe { outl(io + REG_QUEUE_PFN, (qmem >> 12) as u32); }

    Some((desc_base, avail_base, used_base, qmem))
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
