# `kernel/src/drivers/igc.rs` @ 5e0102684

## L1-13 · `use core::sync::atomic::{AtomicU8, Ordering};`

```
//! Intel I225/I226 (igc) data path — ported from Linux
//! `drivers/net/ethernet/intel/igc/igc_main.c`.
//!
//! Replaces the igc branch of `intel_nic`, which polled a 32-descriptor ring
//! with no interrupt: at 1 Gbit/s that is ~0.4 ms of traffic, so any gap in
//! the polling dropped frames on the card. Here: 256-descriptor rings
//! (`IGC_DEFAULT_RXD/TXD`), one MSI-X vector for queue pair 0 with hardware
//! auto-mask (`igc_configure_msix`), the NAPI fiber (`net::napi`) drains on
//! the interrupt and re-arms it when the ring is empty (`igc_ring_irq_enable`).
//!
//! Link and PHY stay as the firmware left them (no `CTRL.RST`), as before:
//! the ports below are the parts of `igc_configure`/`igc_up` that set up the
//! queues and the interrupt, not the PHY bring-up.
```

## L22 · `const IGC_STATUS: u32 = 0x0008;`

```
// ── Registers (igc_regs.h) ──
```

## L50 · `const IGC_STATUS_LU: u32 = 1 << 1;`

```
// ── Bits (igc_defines.h / igc_base.h / igc.h) ──
```

## L58 · `const IGC_START_ITR: u32 = 648; // ~6000 ints/s`

```
// ~6000 ints/s
```

## L109 · `const IGC_RX_BUFFER_WRITE: u16 = 16;`

```
/// Return RX buffers in batches — "one at a time is too slow".
```

## L113-114 · `const QUEUE_MSIX_ENTRY: u16 = 1;`

```
/// MSI-X table entry for queue pair 0; entry 0 is "other causes" (link),
/// which we leave masked — link state is read from STATUS.
```

## L124 · `rx_cleaned: u16,`

```
/// Consumed and re-armed, not yet handed back through RDT.
```

## L126-127 · `rx_discarding: bool,`

```
/// Inside a frame that spans descriptors (LPE on, frame > 2 KiB): drop
/// through its EOP descriptor rather than deliver the pieces as frames.
```

## L131-133 · `tx_eop: [u16; IGC_DEFAULT_TXD],`

```
/// `next_to_watch`: for the first slot of each packet, the index of its
/// EOP descriptor — the only one the NIC writes DD back to (RS is set on
/// the last descriptor; a TSO context descriptor never gets one).
```

## L135 · `eims_value: u32,`

```
/// EIMS bit of the queue vector; 0 = no interrupt, polled.
```

## L143 · `unsafe { core::ptr::read_volatile((base + reg as u64) as *const u32) }`

```
// SAFETY: BAR0 mapped by `intel_nic::init`, identity-mapped, uncached.
```

## L148 · `unsafe { core::ptr::write_volatile((base + reg as u64) as *mut u32, val); }`

```
// SAFETY: as `rd32`.
```

## L157-158 · `pub fn init(dev: pci::PciAddr, mmio: u64) -> bool {`

```
/// Bring the queues up on a BAR0 `intel_nic` has mapped. Returns false if a
/// ring cannot be allocated.
```

## L160 · `wr32(mmio, IGC_IMC, 0xFFFF_FFFF);`

```
// Interrupts off and acknowledged while the queues are rebuilt.
```

## L170 · `wr32(mmio, IGC_CTRL_EXT, rd32(mmio, IGC_CTRL_EXT) | IGC_CTRL_EXT_DRV_LOAD);`

```
// igc_get_hw_control
```

## L173 · `wr32(mmio, IGC_TXDCTL0, 0);`

```
// igc_setup_tctl
```

## L181 · `let mut rctl = rd32(mmio, IGC_RCTL);`

```
// igc_setup_rctl (mc_filter_type 0; RSS off — one queue)
```

## L192 · `wr32(mmio, IGC_TXDCTL0, 0);`

```
// igc_configure_tx_ring
```

## L202 · `wr32(mmio, IGC_RXDCTL0, 0);`

```
// igc_configure_rx_ring
```

## L225 · `for i in 0..IGC_DEFAULT_RXD as u16 {`

```
// igc_alloc_rx_buffers(ring, igc_desc_unused) — all but one descriptor.
```

## L231 · `if let Some(vector) = crate::irq::register(dev, QUEUE_MSIX_ENTRY) {`

```
// igc_configure_msix + igc_irq_enable, queue vector only.
```

## L234 · `let msix = QUEUE_MSIX_ENTRY as u32;`

```
// igc_assign_vector: rx queue 0 at offset 0, tx queue 0 at offset 8.
```

## L240 · `wr32(mmio, igc_eitr(msix), (IGC_START_ITR & IGC_QVECTOR_MASK) | IGC_EITR_CNT_IGNR);`

```
// igc_write_itr
```

## L263 · `unsafe { core::ptr::write_bytes(a as *mut u8, 0, pages * 4096); }`

```
// SAFETY: freshly allocated, identity-mapped, exclusively ours.
```

## L268-271 · `fn arm_rx_desc(d: &Igc, i: u16) {`

```
/// Read format for descriptor `i`: buffer address, no header buffer. The
/// `hdr_addr` word overlaps the write-back `status_error`/`length`, so zeroing
/// it also clears the length the clean loop tests (Linux clears
/// `wb.upper.length` of the next descriptor for the same reason).
```

## L275 · `unsafe {`

```
// SAFETY: `desc` is inside the ring we allocated.
```

## L282-283 · `fn rx_refill(d: &mut Igc) {`

```
/// Hand the re-armed descriptors back: RDT one behind next_to_clean, so one
/// descriptor always stays unused (`igc_desc_unused`).
```

## L288 · `core::sync::atomic::fence(Ordering::Release); // wmb()`

```
// wmb()
```

## L293-296 · `pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {`

```
/// One frame of `igc_clean_rx_irq`. At the empty ring it returns the batch
/// and re-enables the queue interrupt (`napi_complete_done` +
/// `igc_ring_irq_enable`); a frame that lands meanwhile is left pending in
/// EICR and fires when EIMS unmasks it.
```

## L304 · `let (status, len) = unsafe {`

```
// SAFETY: descriptor inside our ring; the NIC writes it back by DMA.
```

## L316 · `core::sync::atomic::fence(Ordering::Acquire); // dma_rmb()`

```
// dma_rmb()
```

## L324 · `unsafe { core::ptr::copy_nonoverlapping(src as *const u8, buf.as_mut_ptr(), n); }`

```
// SAFETY: the NIC finished writing this buffer (length != 0 + rmb).
```

## L330 · `if good { return Some(n); }`

```
// A frame spanning buffers or flagged bad is dropped.
```

## L335 · `fn tx_clean(d: &mut Igc) {`

```
/// `igc_clean_tx_irq`: retire whole packets whose EOP descriptor is done.
```

## L341 · `let status = unsafe { core::ptr::read_volatile((desc + 12) as *const u32) };`

```
// SAFETY: descriptor inside our ring; `wb.status` is the olinfo word.
```

## L348 · `fn tx_unused(d: &Igc) -> u16 {`

```
/// Descriptors free for a new packet (one always stays unused).
```

## L354-355 · `fn tx_reserve(d: &mut Igc, need: u16) -> Result<(), NetError> {`

```
/// Wait until `need` descriptors are free: at most ~200 µs for completions,
/// then refuse (`NETDEV_TX_BUSY`) instead of spinning the caller for seconds.
```

## L371 · `unsafe {`

```
// SAFETY: slot `i` was retired by `tx_clean`; buffer and descriptor are ours.
```

## L382 · `core::sync::atomic::fence(Ordering::SeqCst); // wmb() before the tail`

```
// wmb() before the tail
```

## L386 · `pub fn send(frame: &[u8]) -> Result<(), NetError> {`

```
/// `igc_xmit_frame_ring` for a single-buffer frame.
```

## L402-404 · `pub fn send_tso(frame: &[u8], mss: u16, l4_off: usize, hdr_len: usize) -> Result<(), NetError> {`

```
/// TCPv4 segmentation offload — `igc_tso` + `igc_tx_ctxtdesc` + `igc_tx_map`.
/// `frame` is a whole Ethernet frame (IPv4, no options, TCP) whose TCP check
/// holds the Linux `CHECKSUM_PARTIAL` seed (pseudo-header incl. length).
```

## L411-412 · `let mut hdr = [0u8; 128];`

```
// igc_tso: IP tot_len 0 (HW fills per segment), IP check 0 (IXSM), and
// the payload length taken back out of the TCP seed.
```

## L426 · `let ctx = tx_desc(d, first);`

```
// Context descriptor.
```

## L429 · `unsafe {`

```
// SAFETY: slot `first` is free (reserved above).
```

## L440 · `let cmd = IGC_ADVTXD_DTYP_DATA | IGC_ADVTXD_DCMD_DEXT | IGC_ADVTXD_DCMD_IFCS`

```
// Data descriptors; PAYLEN + checksum insertion on the first only.
```

## L452 · `if c == 0 { chunk[..hdr_len].copy_from_slice(&hdr[..hdr_len]); }`

```
// The rewritten headers replace the head of the first chunk.
```

## L472 · `pub fn rx_irq_vector() -> u8 { RX_VECTOR.load(Ordering::Acquire) }`

```
/// LAPIC vector of the RX queue interrupt, 0 when polled.
```

