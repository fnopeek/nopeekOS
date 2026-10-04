# `kernel/src/drivers/virtio_net.rs` @ 5e0102684

## L1-4 · `use core::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};`

```
//! VirtIO Network Device Driver
//!
//! Legacy (0.9.5) VirtIO PCI transport with RX/TX virtqueues.
//! Provides Ethernet frame send/receive for the TCP/IP stack.
```

## L8-11 · `static RX_USED_IDX_PTR: AtomicU64 = AtomicU64::new(0);`

```
/// Lock-free pointer to the host NIC RX used.idx (`rx_used_base + 2`), published
/// at init. Lets the off-vCPU data-plane busy-poll for RX arrival WITHOUT taking
/// the DEVICE lock every spin iteration (the lock-hammer that made the earlier
/// worker-spin net-negative). 0 = not yet up.
```

## L13 · `static TSO: AtomicBool = AtomicBool::new(false);`

```
/// The device segments and checksums TCPv4 GSO frames (`send_tso`).
```

## L18-19 · `#[inline]`

```
/// Current host NIC RX used.idx (lock-free volatile read). The data-plane worker
/// caches the value it last drained to; a change means a frame arrived.
```

## L24-26 · `unsafe { core::ptr::read_volatile(p as *const u16) }`

```
// SAFETY: p is the device's used-ring idx field, identity-mapped, set once
// at init and stable for the device's life; a torn 16-bit read at worst
// costs one extra poll iteration.
```

## L44-45 · `const REG_CONFIG_MSIX_VEC: u16 = 0x14;`

```
// Legacy-virtio MSI-X vector registers — present only when MSI-X is enabled
// (which also shifts the device config from offset 20 to 24).
```

## L56 · `const F_CSUM: u32      = 1 << 0;   // VIRTIO_NET_F_CSUM      — device checksums TX`

```
// VIRTIO_NET_F_CSUM      — device checksums TX
```

## L57 · `const F_GSO: u32       = 1 << 6;   // VIRTIO_NET_F_GSO (legacy) — device handles`

```
// VIRTIO_NET_F_GSO (legacy) — device handles
```

## L58-60 · `const F_HOST_TSO4: u32 = 1 << 11;  // VIRTIO_NET_F_HOST_TSO4 — device segments TSOv4`

```
// ANY GSO type + checksum. QEMU's transitional
// virtio-net offers this (not the modern
// per-type HOST_TSO4) to a legacy driver.
```

## L61 · `const F_HOST_TSO4: u32 = 1 << 11;  // VIRTIO_NET_F_HOST_TSO4 — device segments TSOv4`

```
// VIRTIO_NET_F_HOST_TSO4 — device segments TSOv4
```

## L62 · `#[allow(dead_code)]`

```
/// virtio_net_hdr.flags / gso_type for offloaded TX (host does csum + TCP-GSO).
```

## L71-81 · `const RX_BUFFERS: usize = 1024;`

```
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
```

## L84-88 · `const TX_RECLAIM_SPINS: u32 = 4096;`

```
// On a full TX ring, spin-reclaim this many times before dropping the
// frame. QEMU/slirp drains the ring on its own host thread, so a bounded
// busy-wait lets in-flight descriptors complete instead of dropping the
// guest's packet (→ TCP retransmit → upload throughput collapse). Bounded
// so a genuinely wedged device can't hang the sender.
```

## L90 · `pub const MTU: usize = 1514; // Ethernet max frame`

```
// Ethernet max frame
```

## L92-96 · `const TSO_SLOT_SIZE: usize = 66 * 1024;`

```
/// A GSO super-frame's data buffer: eth + a full 64 KB IP packet. One per
/// in-flight super-frame, so it is ONE data descriptor instead of one per
/// MTU chunk (46): a 256-entry ring held five super-frames, filled at once
/// under upload, and `send_tso` spun on it holding `DEVICE` — the lock the
/// RX side needs to take the ACKs in.
```

## L101 · `#[repr(C)]`

```
/// VirtIO net header prepended to every packet (10 bytes, no mergeable buffers)
```

## L112 · `const NET_HDR_SIZE: usize = core::mem::size_of::<VirtioNetHdr>(); // 10`

```
// 10
```

## L128 · `rx_desc_base: u64,`

```
// RX queue
```

## L134 · `rx_buffers: u64, // contiguous RX buffer region`

```
// contiguous RX buffer region
```

## L135 · `rx_repost_pending: u16, // RX buffers reposted but not yet notified (batch the doorbell)`

```
// RX buffers reposted but not yet notified (batch the doorbell)
```

## L136 · `pci_addr: pci::PciAddr, // for MSI-X dest re-routing (net-RX IRQ)`

```
// for MSI-X dest re-routing (net-RX IRQ)
```

## L137 · `rx_msix_vector: u8,     // LAPIC vector for RX-queue MSI-X (0 = none, polling)`

```
// LAPIC vector for RX-queue MSI-X (0 = none, polling)
```

## L139 · `tx_desc_base: u64,`

```
// TX queue
```

## L147 · `tx_notify_pending: bool, // TX frames queued but doorbell not yet rung (batch it)`

```
// TX frames queued but doorbell not yet rung (batch it)
```

## L149 · `tx_hdrs: u64,   // pre-allocated net headers for TX`

```
// pre-allocated net headers for TX
```

## L150-156 · `tx_data: u64,`

```
/// Pre-allocated DMA-stable TX data pool — one MTU-sized slot per
/// descriptor. `send` copies the caller's frame here so the device
/// reads from memory that outlives the caller's stack-local `Vec`
/// (which otherwise would be dropped + heap-recycled before QEMU's
/// slirp main loop wakes up to do the DMA, on real HW this race
/// happens to lose less often because the NIC engine reads
/// synchronously). Modelled on `intel_nic::send`'s `tx_bufs`.
```

## L158 · `tso_data: u64,`

```
/// `TSO_SLOTS` super-frame buffers, `TSO_SLOT_SIZE` each.
```

## L160 · `tso_free: u32,`

```
/// Bit per free TSO slot.
```

## L162 · `tso_slot_of: alloc::vec::Vec<u8>,`

```
/// Chain head descriptor → the TSO slot it holds, freed on completion.
```

## L183-193 · `const NET_RX_IRQ_ENABLED: bool = true;`

```
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
```

## L202 · `unsafe {`

```
// SAFETY: All port I/O targets the VirtIO device's I/O BAR
```

## L210-213 · `let mut accepted = features & F_MAC;`

```
// Accept MAC, plus CSUM+TSO4 if BOTH are offered. These are DEVICE
// capabilities (it can segment and checksum for us); we do not currently
// hand it a GSO frame, and accepting them costs nothing. Only when both
// are present, so we never promise a frame the device can't segment.
```

## L215-221 · `let modern = (features & (F_CSUM | F_HOST_TSO4)) == (F_CSUM | F_HOST_TSO4);`

```
// Prefer the modern per-type bits; fall back to the legacy combined F_GSO
// (what QEMU's transitional device offers a legacy driver). Either lets us
// forward the guest's GSO super-frame AS-IS (device segments + checksums).
// Linux `virtnet_probe`: every TSO feature sits INSIDE the F_CSUM
// branch. A device that cannot checksum cannot segment — QEMU with a
// slirp backend still lists the legacy F_GSO bit (no vnet header
// behind it), and trusting that bit alone cut the upload to 2 Mbit.
```

## L239 · `let mut mac = [0u8; 6];`

```
// Read MAC address
```

## L247 · `outw(io + REG_QUEUE_SEL, RX_QUEUE);`

```
// Setup RX queue (queue 0)
```

## L264 · `outw(io + REG_QUEUE_SEL, TX_QUEUE);`

```
// Setup TX queue (queue 1)
```

## L281 · `for i in 0..tx_qs as usize {`

```
// Build TX descriptor free chain
```

## L287 · `let tx_hdrs = match memory::allocate_contiguous(`

```
// Allocate TX net headers (one per descriptor)
```

## L300-305 · `let tx_data_pages = (tx_qs as usize * MTU + 4095) / 4096;`

```
// Allocate TX data pool — one MTU slot per descriptor. The
// sender copies the frame here so the descriptor points at
// memory that outlives the caller's stack frame. Without this
// QEMU + slirp races the heap allocator and DMA-reads recycled
// garbage (intermittent under real HW, deterministic under
// QEMU on AMD-host where slirp's event loop wakes up later).
```

## L325 · `let rx_buf_count = RX_BUFFERS.min(rx_qs as usize);`

```
// Allocate RX buffers (contiguous, one per RX descriptor)
```

## L337 · `for i in 0..rx_buf_count {`

```
// Post RX buffers to the RX queue
```

## L346 · `let avail_ring = rx_avail + 4;`

```
// Add to available ring
```

## L352 · `fence(Ordering::SeqCst);`

```
// Set available ring idx
```

## L357 · `let mut rx_msix_vector = 0u8;`

```
// Suppress TX interrupts
```

## L360-364 · `let mut rx_msix_vector = 0u8;`

```
// Bind the RX queue to MSI-X table entry 0 so the device fires our
// vector on RX. config_msix_vector = NO_VECTOR (we don't want a
// config-change IRQ). Read-back guards a device that rejects it →
// rx_msix_vector stays 0 and we keep polling (the recv path is
// unchanged either way).
```

## L378 · `outb(io + REG_STATUS, S_ACKNOWLEDGE | S_DRIVER | S_DRIVER_OK);`

```
// Go live
```

## L385 · `outw(io + REG_QUEUE_NOTIFY, RX_QUEUE);`

```
// Notify RX queue that buffers are available
```

## L424-426 · `pub fn rx_irq_vector() -> u8 {`

```
/// LAPIC vector the host NIC raises on RX (0 = none / polling). The microvm
/// routes this IRQ to its vCPU core (step 2) so RX arrival wakes the vCPU to
/// pump. Returns 0 until net-RX is enabled end-to-end.
```

## L431 · `pub fn send(frame: &[u8]) -> Result<(), NetError> {`

```
/// Send an Ethernet frame. `frame` must be a complete Ethernet frame (dst + src + type + payload).
```

## L438-441 · `dev.reclaim_tx();`

```
// Reclaim completed TX descriptors. Under burst upload the ring fills
// faster than QEMU/slirp drains it; rather than dropping the frame
// (→ guest TCP retransmit → upload collapse), spin-reclaim briefly so
// in-flight descriptors free up. Backpressure, not loss. Bounded.
```

## L458 · `unsafe {`

```
// SAFETY: Writing to pre-allocated DMA buffers
```

## L460 · `core::ptr::write_bytes(hdr_addr as *mut u8, 0, NET_HDR_SIZE);`

```
// Zero net header (no offload)
```

## L463 · `let desc0 = (dev.tx_desc_base + d0 as u64 * 16) as *mut VringDesc;`

```
// Descriptor 0: net header
```

## L470-473 · `let data_addr = dev.tx_data + d1 as u64 * MTU as u64;`

```
// Descriptor 1: frame data. Copy into the DMA-stable pool
// before publishing the descriptor — the caller's `frame: &[u8]`
// is a stack-local Vec that gets dropped before QEMU/slirp's
// event loop wakes up to walk our virtqueue.
```

## L483 · `let avail_ring = dev.tx_avail_base + 4;`

```
// Add to available ring
```

## L496-500 · `dev.tx_notify_pending = true;`

```
// Batch the TX doorbell: an outw() notify is a VM-exit, and notifying per
// frame was a per-packet exit on the upload path (the asymmetry vs download).
// Mark pending; net::poll() flushes it once per cycle (tx_flush). Flush
// immediately only if the ring is filling, so QEMU drains before send() has
// to spin-reclaim.
```

## L513-517 · `pub fn send_tso(frame: &[u8], mss: u16, l4_off: usize, hdr_len: usize) -> Result<(), NetError> {`

```
/// Hand the device a TCPv4 GSO super-frame to segment and checksum — Linux
/// virtio_net `xmit_skb` with `CHECKSUM_PARTIAL` + `SKB_GSO_TCPV4`. The TCP
/// check must hold the pseudo-header seed. Only for REAL GSO frames: this
/// QEMU's legacy F_GSO got NEEDS_CSUM wrong on small frames (0.226.60), so
/// everything that fits one MSS keeps the plain `send` with a full checksum.
```

## L523 · `dev.reclaim_tx();`

```
// Two descriptors (header, data) and a free super-frame buffer.
```

## L548 · `h[6..8].copy_from_slice(&(l4_off as u16).to_le_bytes()); // csum_start`

```
// csum_start
```

## L549 · `h[8..10].copy_from_slice(&16u16.to_le_bytes());          // csum_offset: tcphdr.check`

```
// csum_offset: tcphdr.check
```

## L550-551 · `unsafe {`

```
// SAFETY: pre-allocated DMA buffers + this device's descriptor table;
// the slot is ours until its chain completes, `frame` fits it (checked).
```

## L567-568 · `unsafe {`

```
// Publish the chain head (d0) on the avail ring.
// SAFETY: this device's own virtqueue memory; fenced.
```

## L584-585 · `pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {`

```
/// Receive an Ethernet frame. Returns frame data (without virtio net header).
/// Returns None if no packet available.
```

## L594-598 · `if dev.rx_msix_vector != 0 {`

```
// Ring appears drained. NAPI: if RX IRQs are enabled (MSI-X on) and we
// had suppressed them during a drain (avail.flags==1), this is the
// drain end → re-enable + re-check once (a frame may have landed in the
// window) before parking, so no wakeup is lost. If already enabled
// (flags==0), the next RX will interrupt — nothing to do.
```

## L607 · `return None; // truly empty, IRQ re-armed for the next frame`

```
// truly empty, IRQ re-armed for the next frame
```

## L609 · `unsafe { core::ptr::write_volatile(dev.rx_avail_base as *mut u16, 1); }`

```
// A frame arrived in the window — re-suppress + fall through.
```

## L616-617 · `dev.rx_kick();`

```
// Ring drained — ring the doorbell once for everything reposted
// during this drain (batched notify, not one VM-exit per packet).
```

## L619 · `return None; // no new packets`

```
// no new packets
```

## L622-623 · `unsafe { core::ptr::write_volatile(dev.rx_avail_base as *mut u16, 1); }`

```
// Frames present → we're draining; suppress further RX IRQs until the
// ring empties (NAPI), so the device doesn't interrupt per packet.
```

## L627 · `let used_entry_off = 4 + (dev.rx_last_used % dev.rx_queue_size) as u64 * 8;`

```
// Read the used ring entry
```

## L638 · `if used_len <= NET_HDR_SIZE {`

```
// The buffer contains: [VirtioNetHdr (10 bytes)][Ethernet frame]
```

## L640 · `dev.repost_rx(used_id as usize);`

```
// Repost buffer
```

## L649 · `unsafe {`

```
// SAFETY: Reading from DMA buffer in identity-mapped range
```

## L658 · `dev.repost_rx(used_id as usize);`

```
// Repost buffer for next receive
```

## L668-669 · `pub fn tx_flush() {`

```
/// Ring the deferred TX doorbell, if any frames were queued since the last kick.
/// Called once per net::poll() cycle so per-frame send() avoids a VM-exit each.
```

## L681 · `impl VirtioNet {`

```
// === Internal ===
```

## L720-723 · `let mut cur = id;`

```
// Free the WHOLE descriptor chain. A plain frame is hdr→data (2); an
// offloaded GSO super-frame spans hdr→data→data→… (many). Walk
// DESC_F_NEXT, reading (flags,next) BEFORE freeing (free_tx_desc
// overwrites next with the free-list head).
```

## L737-739 · `if self.rx_msix_vector == 0 {`

```
// Reading ISR deasserts the legacy INTx line. Under MSI-X there is
// no line to deassert, and on QEMU the port read is an exit — once
// per frame sent, since every send reclaims first.
```

## L741 · `unsafe { inb(self.io_base + REG_ISR); }`

```
// SAFETY: this device's legacy I/O BAR.
```

## L747-753 · `let avail_ring = self.rx_avail_base + 4;`

```
// Re-add this buffer to the RX available ring — but DON'T ring the
// doorbell per packet. An `outw` to the notify port is a VM-exit; doing
// it once per received packet costs a VM-exit per packet (~100k/s under
// load) and was a major throughput/latency tax. We publish the buffer to
// the avail ring now and batch the notify (see recv): one doorbell per
// drain instead of per packet. A mid-burst safety notify keeps the device
// from running dry if a caller doesn't drain to empty.
```

## L770 · `fn tx_kick(&mut self) {`

```
// Ring the TX doorbell once for all frames queued since the last kick.
```

## L780 · `fn rx_kick(&mut self) {`

```
// Ring the RX doorbell once for all buffers reposted since the last kick.
```

