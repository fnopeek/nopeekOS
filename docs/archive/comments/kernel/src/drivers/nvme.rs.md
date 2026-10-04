# `kernel/src/drivers/nvme.rs` @ 5e0102684

## L1-4 · `use crate::{kprintln, pci, paging, memory};`

```
//! NVMe Driver (NVM Express over PCIe)
//!
//! Memory-mapped I/O via BAR0. Admin + I/O queue pairs.
//! Exposes same block API as virtio_blk for npkFS compatibility.
```

## L15 · `const NVME_CLASS: u8 = 0x01;`

```
// NVMe class: Mass Storage (01h), NVMe (08h)
```

## L19 · `const REG_CAP: usize = 0x00;       // Controller Capabilities (64-bit)`

```
// NVMe controller registers (offsets from BAR0)
```

## L20 · `const REG_CAP: usize = 0x00;       // Controller Capabilities (64-bit)`

```
// Controller Capabilities (64-bit)
```

## L21 · `const REG_VS: usize = 0x08;        // Version`

```
// Version
```

## L22 · `const REG_INTMS: usize = 0x0C;     // Interrupt Mask Set`

```
// Interrupt Mask Set
```

## L23 · `const REG_INTMC: usize = 0x10;     // Interrupt Mask Clear`

```
// Interrupt Mask Clear
```

## L24 · `const REG_CC: usize = 0x14;        // Controller Configuration`

```
// Controller Configuration
```

## L25 · `const REG_CSTS: usize = 0x1C;      // Controller Status`

```
// Controller Status
```

## L26 · `const REG_AQA: usize = 0x24;       // Admin Queue Attributes`

```
// Admin Queue Attributes
```

## L27 · `const REG_ASQ: usize = 0x28;       // Admin Submission Queue Base (64-bit)`

```
// Admin Submission Queue Base (64-bit)
```

## L28 · `const REG_ACQ: usize = 0x30;       // Admin Completion Queue Base (64-bit)`

```
// Admin Completion Queue Base (64-bit)
```

## L30 · `const CC_EN: u32 = 1 << 0;         // Enable`

```
// Controller Configuration bits
```

## L31 · `const CC_EN: u32 = 1 << 0;         // Enable`

```
// Enable
```

## L32 · `const CC_CSS_NVM: u32 = 0 << 4;    // NVM Command Set`

```
// NVM Command Set
```

## L33 · `const CC_MPS_4K: u32 = 0 << 7;     // Memory Page Size = 4K (2^(12+0))`

```
// Memory Page Size = 4K (2^(12+0))
```

## L34 · `const CC_IOSQES: u32 = 6 << 16;    // I/O SQ Entry Size = 2^6 = 64`

```
// I/O SQ Entry Size = 2^6 = 64
```

## L35 · `const CC_IOCQES: u32 = 4 << 20;    // I/O CQ Entry Size = 2^4 = 16`

```
// I/O CQ Entry Size = 2^4 = 16
```

## L37 · `const CSTS_RDY: u32 = 1 << 0;      // Ready`

```
// Controller Status bits
```

## L38 · `const CSTS_RDY: u32 = 1 << 0;      // Ready`

```
// Ready
```

## L40 · `const ADM_IDENTIFY: u8 = 0x06;`

```
// Admin opcodes
```

## L46 · `const NVM_FLUSH: u8 = 0x00; // Flush volatile write cache to stable media`

```
// NVM opcodes
```

## L47 · `const NVM_FLUSH: u8 = 0x00; // Flush volatile write cache to stable media`

```
// Flush volatile write cache to stable media
```

## L50 · `const NVM_DSM: u8 = 0x09;   // Dataset Management (TRIM/Deallocate)`

```
// Dataset Management (TRIM/Deallocate)
```

## L52 · `const ADMIN_QUEUE_SIZE: u16 = 16;`

```
// Queue sizes (entries)
```

## L54-57 · `const IO_QUEUE_SIZE: u16 = 256;`

```
// 256 entries is universally supported by NVMe controllers (CAP.MQES on
// any modern SSD is well above this). 1024 worked on the test rig but
// blocks at-rest decryption if an SSD reports a smaller MQES — keycheck
// fails to read and the user sees "wrong passphrase" with no clue why.
```

## L60 · `#[repr(C)]`

```
// Submission Queue Entry (64 bytes)
```

## L90 · `#[repr(C)]`

```
// Completion Queue Entry (16 bytes)
```

## L94 · `dw0: u32,           // Command-specific`

```
// Command-specific
```

## L99 · `status: u16,        // Bit 0 = Phase, bits 1-15 = status`

```
// Bit 0 = Phase, bits 1-15 = status
```

## L110 · `bar0: u64,                  // Virtual address of mapped BAR0`

```
// Virtual address of mapped BAR0
```

## L111 · `doorbell_stride: u32,       // Bytes between doorbells`

```
// Bytes between doorbells
```

## L112 · `admin_sq: u64,              // Physical address`

```
// Admin queue
```

## L113 · `admin_sq: u64,              // Physical address`

```
// Physical address
```

## L114 · `admin_cq: u64,              // Physical address`

```
// Physical address
```

## L118 · `io_sq: u64,`

```
// I/O queue (QID 1)
```

## L124 · `total_lbas: u64,            // Total logical blocks (512-byte sectors)`

```
// Device info
```

## L125 · `total_lbas: u64,            // Total logical blocks (512-byte sectors)`

```
// Total logical blocks (512-byte sectors)
```

## L128 · `oncs: u16,                  // Optional NVM Command Support (from Identify Controller)`

```
// Optional NVM Command Support (from Identify Controller)
```

## L130 · `msix_vector: u8,            // LAPIC vector for I/O CQ completion MSI-X (0 = none/poll)`

```
// LAPIC vector for I/O CQ completion MSI-X (0 = none/poll)
```

## L131 · `pci_addr: pci::PciAddr,     // for live MSI-X read-back (disk diagnostic)`

```
// for live MSI-X read-back (disk diagnostic)
```

## L137 · `static DMA_BUF: Mutex<Option<u64>> = Mutex::new(None);`

```
// DMA buffer for data transfers (one 4KB page, identity-mapped)
```

## L140-148 · `const DMA_POOL_SLOTS: usize = 256;`

```
/// Pool of pre-allocated 4 KB DMA buffers for batched I/O. With a single
/// shared `DMA_BUF`, every block transfer is necessarily synchronous —
/// the source/dest pointer would race otherwise. The pool lets us submit
/// up to `DMA_POOL_SLOTS` commands in flight, ring the doorbell once,
/// then collect all completions in a single drain.
///
/// 256 slots × 4 KB = 1 MB. Sized to hold `MAX_INFLIGHT` cmds × 128
/// blocks/cmd = 256 slots, so a 1 MB read can keep two cmds in flight
/// simultaneously. Must stay strictly below `IO_QUEUE_SIZE` (256).
```

## L152-155 · `const MAX_INFLIGHT: usize = 4;`

```
/// Number of NVMe commands `read_extent` / `write_extent` keep in
/// flight before draining. Bounded by both the PRP-list pool and the
/// DMA pool — each in-flight cmd consumes one PRP-list slot and up to
/// `MAX_BLOCKS_PER_CMD` data slots. 4 = 1 MB at MDTS=128.
```

## L158-162 · `const MAX_INFLIGHT_MULTI: usize = 32;`

```
/// Max in-flight cmds for the multi-extent read path. Sized for the
/// fragmented-blob case: a 1 MB blob split into 257 single-block
/// extents wants as many concurrent cmds as the SSD can pipeline.
/// 32 cmds × 1 block = 32 pool slots — plenty of headroom in the
/// 256-slot pool.
```

## L165-168 · `const PRP_LIST_POOL_SLOTS: usize = MAX_INFLIGHT_MULTI;`

```
/// PRP-list scratch pool. NVMe spec: a transfer covering ≥3 4 KB pages
/// uses `prp1` for the first page and a "PRP List" — a 4 KB block
/// holding up to 512 page-pointers — addressed by `prp2`. Sized for
/// `MAX_INFLIGHT_MULTI` so the multi-extent path can fully parallelise.
```

## L172-176 · `static MAX_BLOCKS_PER_CMD: AtomicU32 = AtomicU32::new(64);`

```
/// Max data blocks per single NVMe READ/WRITE command. Computed from
/// Identify Controller's MDTS field (offset 77) at init time; capped to
/// `DMA_POOL_SLOTS` so a single cmd fits in our DMA pool. Treated as a
/// hard upper bound by `read_extent` / `write_extent`; larger transfers
/// are split into multiple cmds.
```

## L188 · `let lo = mmio_read32(base, offset) as u64;`

```
// NVMe spec: 64-bit registers may need two 32-bit reads
```

## L199 · `fn ring_sq_doorbell(state: &NvmeState, qid: u16, tail: u16) {`

```
/// Ring doorbell for a submission queue.
```

## L205 · `fn ring_cq_doorbell(state: &NvmeState, qid: u16, head: u16) {`

```
/// Ring doorbell for a completion queue.
```

## L211 · `fn admin_command(state: &mut NvmeState, mut cmd: SqEntry) -> Result<CqEntry, BlkError> {`

```
/// Submit a command to the admin queue and wait for completion.
```

## L216 · `let sq_ptr = state.admin_sq as *mut SqEntry;`

```
// Write to admin SQ
```

## L222 · `let cq_ptr = state.admin_cq as *const CqEntry;`

```
// Poll completion
```

## L228 · `state.admin_cq_head = (state.admin_cq_head + 1) % ADMIN_QUEUE_SIZE;`

```
// Advance CQ head
```

## L244 · `fn io_command(state: &mut NvmeState, mut cmd: SqEntry) -> Result<CqEntry, BlkError> {`

```
/// Submit a command to I/O queue 1 and wait for completion.
```

## L267-269 · `nvme_msix_confirm(state.msix_vector);`

```
// One-shot HW validation (B-1): the poll completed this command;
// if the MSI-X ISR also fired (count advanced), the device-IRQ
// path works end-to-end. Logged once, in non-ISR context.
```

## L278-281 · `static MSIX_CONFIRMED: AtomicBool = AtomicBool::new(false);`

```
/// One-shot confirmation that NVMe completions raise our MSI-X ISR. Validates
/// the `host-device-irq` foundation on real hardware without changing the
/// (poll-based) completion logic. Removed once B-2 makes `io_command` actually
/// wait on the IRQ.
```

## L294 · `pub fn init() -> bool {`

```
/// Initialize the NVMe controller.
```

## L296-299 · `let dev = match pci::find_by_class(NVME_CLASS, NVME_SUBCLASS) {`

```
// Find NVMe device by class code (01h:08h)
// Die Absage war STUMM, und damit sah "keine NVMe da" genauso aus wie
// "Treiber nicht gerufen". Eine Zeile kostet nichts und ist auf einem
// Geraet ohne Platte die einzige Auskunft, die es gibt.
```

## L312 · `pci::enable_bus_master(dev.addr);`

```
// Enable bus mastering for DMA
```

## L315 · `let bar0_phys = pci::read_bar64(dev.addr, 0x10);`

```
// Read 64-bit BAR0
```

## L322-323 · `let map_size = 64 * 1024u64;`

```
// Map BAR0 pages (NVMe registers are typically 16KB-64KB)
// Map 64KB to be safe (covers registers + doorbells)
```

## L325 · `let bar0_virt = bar0_phys; // Identity-mapped (64GB range covers all PCIe BARs)`

```
// Identity-mapped (64GB range covers all PCIe BARs)
```

## L332 · `Err(paging::PagingError::AlreadyMapped) => {} // Identity-mapped region`

```
// Identity-mapped region
```

## L340 · `let cap = mmio_read64(bar0_virt, REG_CAP);`

```
// Read capabilities
```

## L342 · `let doorbell_stride = 4u32 << ((cap >> 32) & 0xF) as u32; // DSTRD field`

```
// DSTRD field
```

## L343-351 · `let max_queue_entries = (cap & 0xFFFF) as u32 + 1;`

```
// CAP.MQES ist NULLBASIERT: der groesste zulaessige Wert 0xFFFF heisst
// 65536 Eintraege. `as u16 + 1` lief dabei ueber und ergab 0 — und mit
// 0 lehnt die Pruefung darunter JEDE Queue-Groesse ab. Ausgerechnet der
// Controller, der am meisten kann, kam damit nicht hoch.
//
// Gemeldet an einer KIOXIA [1e0f:000c] in einem Lenovo IdeaPad Flex 5
// 14ALC7: "version 1.4.0, max queue 0" — die Version las sich sauber,
// der Controller antwortete also; nur die Rechnung daneben war falsch.
// In u32, damit der groesste zulaessige Wert auch der groesste bleibt.
```

## L355-357 · `kprintln!("[npk] nvme: version {}.{}.{}, CAP={:#018x}, max queue {}",`

```
// CAP roh mit ins Log: eine 0 kann aus einem ueberlaufenen Plus kommen
// ODER aus einem Register, das gar nicht antwortet, und die zwei Faelle
// sehen in der gerechneten Zahl gleich aus.
```

## L368 · `let cc = mmio_read32(bar0_virt, REG_CC);`

```
// Disable controller
```

## L372 · `for _ in 0..1_000_000u32 {`

```
// Wait for not ready
```

## L379 · `let admin_sq_pages = ((ADMIN_QUEUE_SIZE as usize * 64) + 4095) / 4096;`

```
// Allocate Admin Submission Queue (ADMIN_QUEUE_SIZE * 64 bytes)
```

## L385 · `unsafe { core::ptr::write_bytes(admin_sq as *mut u8, 0, admin_sq_pages * 4096); }`

```
// Zero the queue
```

## L388 · `let admin_cq_pages = ((ADMIN_QUEUE_SIZE as usize * 16) + 4095) / 4096;`

```
// Allocate Admin Completion Queue (ADMIN_QUEUE_SIZE * 16 bytes)
```

## L396 · `let aqa = ((ADMIN_QUEUE_SIZE as u32 - 1) << 16) | (ADMIN_QUEUE_SIZE as u32 - 1);`

```
// Configure admin queues
```

## L402 · `mmio_write32(bar0_virt, REG_INTMS, 0xFFFF_FFFF);`

```
// Mask all interrupts (we poll)
```

## L405 · `let cc_val = CC_EN | CC_CSS_NVM | CC_MPS_4K | CC_IOSQES | CC_IOCQES;`

```
// Enable controller
```

## L409 · `for _ in 0..5_000_000u32 {`

```
// Wait for ready
```

## L435 · `let identify_buf = match memory::allocate_contiguous(1) {`

```
// Allocate DMA buffer for Identify data (4KB)
```

## L442 · `let mut cmd = SqEntry::zeroed();`

```
// Identify Controller (CNS=1)
```

## L447 · `cmd.cdw10 = 1; // CNS=1: Identify Controller`

```
// CNS=1: Identify Controller
```

## L453 · `let buf = identify_buf as *const u8;`

```
// Parse controller info
```

## L456 · `core::ptr::copy_nonoverlapping(buf.add(4), state.serial.as_mut_ptr(), 20);`

```
// Serial number: bytes 4-23
```

## L458 · `core::ptr::copy_nonoverlapping(buf.add(24), state.model.as_mut_ptr(), 40);`

```
// Model number: bytes 24-63
```

## L462-465 · `unsafe {`

```
// ONCS (Optional NVM Command Support) at byte 520 (2 bytes); bit 2 =
// Dataset Management (TRIM/Deallocate). The old code read byte 256, which
// is OACS — Optional *Admin* Command Support — so TRIM detection keyed off
// the firmware-download bit by accident.
```

## L469-472 · `let vwc_present = unsafe { core::ptr::read_volatile(buf.add(525) as *const u8) } & 1 != 0;`

```
// VWC (Volatile Write Cache) at byte 525; bit 0 = a volatile write cache is
// present. When set, completed writes may sit in controller DRAM until an
// NVM Flush — which is exactly why the npkFS commit issues a flush barrier
// before the superblock. Logged so the property is visible on real HW.
```

## L475-478 · `let mdts: u8 = unsafe { core::ptr::read_volatile(buf.add(77) as *const u8) };`

```
// MDTS (Maximum Data Transfer Size) at offset 77, 1 byte. Units of
// 2^(12 + CAP.MPSMIN) bytes; for MPSMIN=0 (~all NVMe SSDs) that's
// pages of 4 KB. 0 means "no limit". Cap at DMA_POOL_SLOTS so a
// single cmd always fits in our staging pool.
```

## L502 · `unsafe { core::ptr::write_bytes(identify_buf as *mut u8, 0, 4096); }`

```
// Identify Namespace 1 (CNS=0, NSID=1)
```

## L508 · `cmd.cdw10 = 0; // CNS=0: Identify Namespace`

```
// CNS=0: Identify Namespace
```

## L514 · `let nsze = unsafe { core::ptr::read_volatile(identify_buf as *const u64) };`

```
// NSZE (Namespace Size) at offset 0 (8 bytes, little-endian)
```

## L526 · `let io_cq_pages = ((IO_QUEUE_SIZE as usize * 16) + 4095) / 4096;`

```
// Create I/O Completion Queue (QID=1)
```

## L534-539 · `state.msix_vector = crate::irq::register(dev.addr, 0).unwrap_or(0);`

```
// Set up MSI-X completion interrupts for the I/O CQ (foundation:
// host-device-irq). Allocate a LAPIC vector + program the device's MSI-X
// table entry 0 to deliver to this (BSP) core. MSI-X writes go straight
// to the LAPIC (no PIC/IOAPIC), so the masked-PIC setup is irrelevant. If
// the device has no usable MSI-X, the vector stays 0 and the CQ is created
// without interrupts (IEN=0) → pure poll, exactly the old behavior.
```

## L541 · `let ien = if state.msix_vector != 0 { 1u32 << 1 } else { 0 }; // cdw11 bit1 = IEN`

```
// cdw11 bit1 = IEN
```

## L552 · `cmd.cdw10 = ((IO_QUEUE_SIZE as u32 - 1) << 16) | 1; // QID=1, size`

```
// QID=1, size
```

## L553 · `cmd.cdw11 = 1 | ien; // bit0 = Physically Contiguous, bit1 = IEN; IV=0`

```
// bit0 = Physically Contiguous, bit1 = IEN; IV=0
```

## L559 · `let io_sq_pages = ((IO_QUEUE_SIZE as usize * 64) + 4095) / 4096;`

```
// Create I/O Submission Queue (QID=1, CQ=1)
```

## L570 · `cmd.cdw10 = ((IO_QUEUE_SIZE as u32 - 1) << 16) | 1; // QID=1, size`

```
// QID=1, size
```

## L571 · `cmd.cdw11 = (1 << 16) | 1; // CQID=1, Physically contiguous`

```
// CQID=1, Physically contiguous
```

## L577-583 · `if state.msix_vector != 0 {`

```
// Disable interrupt coalescing (Set Features FID 0x08, value 0) so the
// controller raises an MSI-X per completion instead of aggregating by
// count/time. Our io_command poll-drains the CQ immediately; if the
// controller defaults to time-based coalescing, the held interrupt
// condition clears before the timer fires → no MSI ever. (Linux waits on
// the IRQ rather than poll-draining, so it never hits this.) Best-effort —
// ignore errors; controllers that lack the feature still work via poll.
```

## L587 · `cmd.cdw10 = 0x08; // FID = Interrupt Coalescing`

```
// FID = Interrupt Coalescing
```

## L588 · `cmd.cdw11 = 0; // aggregation threshold = 0, time = 0 → no coalescing`

```
// aggregation threshold = 0, time = 0 → no coalescing
```

## L593-598 · `mmio_write32(bar0_virt, REG_INTMC, 0xFFFF_FFFF);`

```
// We set INTMS=0xFFFFFFFF early ("mask all, we poll") and never cleared
// it. The spec says MSI-X uses the per-vector table mask and ignores
// INTMS — but some Intel controllers honor it anyway and gate the MSI.
// Clear it (INTMC) now that MSI-X is configured + unmasked, so it can't
// suppress our completion interrupt. Harmless if the controller is
// spec-compliant (write ignored under MSI-X).
```

## L606 · `let dma = match memory::allocate_contiguous(1) {`

```
// Allocate DMA buffer for block I/O
```

## L613-615 · `if let Some(pool) = memory::allocate_contiguous(DMA_POOL_SLOTS) {`

```
// Allocate the batched-I/O DMA pool. Failure here just means
// batched paths fall back to the single-buffer slow path; the
// device still works.
```

## L623-626 · `if let Some(pool) = memory::allocate_contiguous(PRP_LIST_POOL_SLOTS) {`

```
// Allocate the PRP-list scratch pool. Each multi-page extent cmd
// takes one slot. Without this we can still service single-block
// reads/writes via `read_block` / `write_block`, so failure is
// recoverable but disables the fast path.
```

## L634 · `memory::deallocate_frame(identify_buf);`

```
// Free identify buffer
```

## L655 · `pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {`

```
/// Read a 512-byte sector.
```

## L667 · `cmd.cdw10 = sector as u32;         // Starting LBA (low)`

```
// Starting LBA (low)
```

## L668 · `cmd.cdw11 = (sector >> 32) as u32; // Starting LBA (high)`

```
// Starting LBA (high)
```

## L669 · `cmd.cdw12 = 0;                     // Number of LBs - 1 (0 = 1 sector)`

```
// Number of LBs - 1 (0 = 1 sector)
```

## L677 · `pub fn write_sector(sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlkError> {`

```
/// Write a 512-byte sector.
```

## L692 · `cmd.cdw12 = 0; // 1 sector`

```
// 1 sector
```

## L698 · `pub fn read_block(block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), BlkError> {`

```
/// Read a 4KB block (8 sectors).
```

## L713 · `cmd.cdw12 = 7; // 8 sectors - 1`

```
// 8 sectors - 1
```

## L721-728 · `pub fn write_blocks_batch(items: &[(u64, &[u8; BLOCK_SIZE])]) -> Result<(), BlkError> {`

```
/// Submit up to `DMA_POOL_SLOTS` write-block commands in parallel and
/// wait for them all. Returns Ok only if every command completed
/// without error. Falls back to per-block sequential writes when the
/// batch is too large or the pool isn't available.
///
/// This is the path `cache::flush` takes once it has more than one
/// dirty block — replaces N sequential synchronous writes (N × disk
/// latency) with one batch (~1 × disk latency, modulo NVMe ordering).
```

## L732-735 · `if items.len() > DMA_POOL_SLOTS {`

```
// Batch larger than the pool → fall back to sequential. We could
// chunk this internally but `cache::flush` won't ever exceed the
// cache slot count (64 in practice, our pool holds 32 — chunk
// boundary handled here).
```

## L746 · `for &(block, buf) in items {`

```
// Pool wasn't allocated — fall back.
```

## L757-760 · `for (i, &(block, buf)) in items.iter().enumerate() {`

```
// Stage every payload into its own DMA pool slot + push every SQ
// entry, then ring the doorbell exactly once. The SQ has 64 slots;
// `items.len() ≤ DMA_POOL_SLOTS = 32`, so we can't wrap into our
// own un-acked head.
```

## L783-788 · `core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);`

```
// Memory fence between SQ entry stores and the doorbell write.
// x86 stores are normally ordered, but during the deadlock chase
// adding kprintlns between submit + drain made the issue go away —
// those acted as MMIO-serializing barriers. An explicit SeqCst
// fence pins the ordering deterministically without the serial
// overhead, and matches what real NVMe drivers do.
```

## L822-831 · `pub fn read_blocks_batch(blocks: &[u64], output: &mut [u8]) -> Result<(), BlkError> {`

```
/// Submit up to `DMA_POOL_SLOTS` read-block commands in parallel and
/// wait for them all. `output` is the destination buffer, sized
/// `blocks.len() * BLOCK_SIZE` — block i lands at `output[i*B..i*B+B]`.
/// Falls back to sequential `read_block` when the batch exceeds the
/// pool (or the pool isn't available).
///
/// Read-side analog of `write_blocks_batch`. Same SQ submit + drain
/// pattern, with one key extra: a memory fence after the drain so the
/// CPU's copy from pool slots doesn't observe stale data ahead of the
/// DMA writes.
```

## L850-851 · `if blocks.len() > DMA_POOL_SLOTS {`

```
// Chunk into pool-sized groups so a 256-block fetch (1 MB blob)
// runs as 8 × 32-block parallel batches instead of 256 serial.
```

## L867 · `fn read_blocks_batch_inner(blocks: &[u64], output: &mut [u8], pool_base: u64) -> Result<(), BlkError> {`

```
/// Single ≤ DMA_POOL_SLOTS read batch. Pool-based, queue-depth ≈ N.
```

## L922 · `core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);`

```
// Ensure DMA writes are visible before we read from the pool.
```

## L938-948 · `fn build_prp_list(list_addr: u64, dma_base: u64, count: u64) {`

```
// ── Single-command extent transfers (PRP list path) ───────────────────
//
// A naive `read_blocks_batch` issues one NVMe cmd per 4 KB page; a 1 MB
// read becomes 256 round-trips through SQ/CQ. NVMe's PRP list mechanism
// (spec 4.3) lets a single cmd address up to 512 pages via a "PRP List"
// block — `prp1` is the first page, `prp2` points at the list, and the
// list holds entries for the remaining pages. With our contiguous DMA
// pool that costs one extra block-size scratch buffer per cmd in
// flight; in exchange a 1 MB read drops to ~2 cmds (one per
// `MAX_BLOCKS_PER_CMD` chunk) and the SSD's internal pipelining gets to
// do its job instead of draining a SQ on every page.
```

## L950-953 · `fn build_prp_list(list_addr: u64, dma_base: u64, count: u64) {`

```
/// Build a PRP-list scratch block in `list_addr`. Page 0 of the
/// transfer goes into `cmd.prp1` directly; this list addresses pages
/// `1..count` (inclusive of the second page, exclusive of `count`).
/// Caller must ensure `count >= 3` and `count - 1 <= 512`.
```

## L965-966 · `fn drain_one_completion(state: &mut NvmeState) -> Result<(), BlkError> {`

```
/// Drain one I/O completion from the IO queue. Caller must have
/// submitted a single cmd and rung the doorbell.
```

## L985-987 · `record_drain(t0, state.msix_vector);`

```
// [nvme-spin] measure how long this completion busy-spun. Tells us
// whether the poll wastes real CPU (→ worth the lock-restructure to
// an IRQ-park) or completes in µs (→ leave it). Pure measurement.
```

## L996 · `static NVME_DRAIN_COUNT: AtomicU64 = AtomicU64::new(0);`

```
// [nvme-spin] rolling spin-duration stats over the shared completion drain.
```

## L1002 · `const NVME_SLOW_US: u64 = 200; // a completion that spun > this is "slow"`

```
// a completion that spun > this is "slow"
```

## L1028-1030 · `fn submit_extent_cmd(`

```
/// Submit a single READ command spanning `chunk_blocks` consecutive
/// 4 KB blocks starting at `start_sector`. `chunk_blocks` must be ≤
/// `DMA_POOL_SLOTS` and ≤ `MAX_BLOCKS_PER_CMD`. Caller drains.
```

## L1068-1071 · `pub fn read_extent(start_block: u64, count: u64, output: &mut [u8]) -> Result<(), BlkError> {`

```
/// Read `count` 4 KB blocks starting at `start_block` into `output`.
/// Up to `MAX_INFLIGHT` PRP-list cmds are kept in flight at once: the
/// SSD's internal channels stay busy while we drain + memcpy the
/// completed batch. `output.len()` must equal `count * BLOCK_SIZE`.
```

## L1097-1100 · `let mut offset = 0u64;`

```
// Submit + drain in batches of MAX_INFLIGHT. Each in-flight cmd
// owns its own slice of the DMA pool + its own PRP-list slot, so
// they can complete out of order. We drain in submission order so
// the memcpy-out is straight-line.
```

## L1106 · `for slot in 0..MAX_INFLIGHT {`

```
// Submit phase: queue up to MAX_INFLIGHT cmds.
```

## L1123 · `for _ in 0..batch_len {`

```
// Drain phase: wait for every cmd in this batch to finish.
```

## L1127 · `core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);`

```
// Make all DMA writes visible before we copy out.
```

## L1130-1132 · `for i in 0..batch_len {`

```
// Memcpy phase: pool slot → caller buffer for each cmd we just
// drained. Order matches submission order, so output is
// contiguous block-by-block.
```

## L1148-1150 · `pub fn write_extent(start_block: u64, count: u64, input: &[u8]) -> Result<(), BlkError> {`

```
/// Write `count` 4 KB blocks of `input` starting at `start_block`.
/// Up to `MAX_INFLIGHT` cmds in flight at once. Symmetric to
/// `read_extent` — staged into the DMA pool first, then submitted.
```

## L1180 · `for slot in 0..MAX_INFLIGHT {`

```
// Stage + submit phase.
```

## L1197-1199 · `core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);`

```
// Per-cmd fence: the SSD must see this slot's payload
// before reading it. Combined fence after the loop would
// also work but per-cmd is clearer.
```

## L1208 · `for _ in 0..batch_len {`

```
// Drain phase.
```

## L1217-1225 · `pub fn read_multi_extent(extents: &[(u64, u64)], output: &mut [u8]) -> Result<(), BlkError> {`

```
/// Read a list of (start_block, count_blocks) extents into `output`.
/// All extents are concatenated in order: extent 0 → output[0..N₀ * 4096],
/// extent 1 → output[N₀ * 4096 .. (N₀+N₁) * 4096], etc.
///
/// Submit-phase batches up to `MAX_INFLIGHT_MULTI` cmds at once so the
/// SSD's internal channels can pipeline all of them. Critical for
/// fragmented blobs — a 1 MB blob split into 257 single-block extents
/// goes from 257 sequential round-trips (~8.5 ms) to ~9 batches × 32
/// cmds parallel (~1.5 ms expected).
```

## L1228 · `let total_blocks: u64 = extents.iter().map(|&(_, c)| c).sum();`

```
// Sanity-check size: total blocks must match output buffer.
```

## L1237-1238 · `let mut off = 0usize;`

```
// Fallback: per-extent calls (will use the per-block path
// when the pool is gone too). Slow but correct.
```

## L1252-1254 · `let mut work: alloc::vec::Vec<(u64, u64, usize)> =`

```
// Phase 1 — split any oversized extent into chunks ≤ max_per_cmd
// and compute the output offset for each chunk. Done up front so
// the submit loop is a single linear pass.
```

## L1273-1275 · `let mut wi = 0;`

```
// Phase 2 — process work in batches of `MAX_INFLIGHT_MULTI` cmds.
// Each batch: greedy-fill until we hit the inflight cap or pool
// capacity, submit all, drain all, memcpy all into `output`.
```

## L1278-1280 · `let mut batch: [(u64, usize, u64); MAX_INFLIGHT_MULTI] =`

```
// Per-batch state. We track each in-flight cmd's pool address
// and target output offset so the post-drain memcpy knows
// where each chunk lands.
```

## L1282 · `[(0, 0, 0); MAX_INFLIGHT_MULTI]; // (dma_addr, output_off, blocks)`

```
// (dma_addr, output_off, blocks)
```

## L1288-1289 · `if pool_used + count > DMA_POOL_SLOTS as u64 { break; }`

```
// Pool capacity check — can't fit this chunk in the
// remaining slots; close the batch and process it.
```

## L1307 · `for _ in 0..batch_len {`

```
// Drain all in submission order.
```

## L1311 · `core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);`

```
// Make all DMA writes visible before the memcpy-out.
```

## L1314 · `for i in 0..batch_len {`

```
// Copy each completed chunk to its output offset.
```

## L1331 · `pub fn write_block(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {`

```
/// Write a 4KB block (8 sectors).
```

## L1336-1339 · `pub fn write_block_fua(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {`

```
/// Like `write_block` but with Force Unit Access: the controller must place the
/// data on stable media before completing, regardless of its volatile write
/// cache. Used for the npkFS superblock so it is durable the instant the commit
/// returns, without a separate full-cache FLUSH after it.
```

## L1359 · `cmd.cdw12 = 7 | if fua { 1 << 30 } else { 0 }; // 8 sectors - 1, + FUA bit`

```
// 8 sectors - 1, + FUA bit
```

## L1365-1369 · `pub fn flush() -> Result<(), BlkError> {`

```
/// Force the controller's volatile write cache to stable media (NVM Flush,
/// opcode 0x00, namespace-wide). Without this, completed writes may linger in
/// controller DRAM and be lost or reordered on power-loss — which would break
/// the npkFS commit ordering (superblock durable only after the data it points
/// at). This is the durability barrier the 4-phase commit relies on.
```

## L1382 · `Some(state) => state.oncs & (1 << 2) != 0, // ONCS bit 2 = Dataset Management`

```
// ONCS bit 2 = Dataset Management
```

## L1387-1388 · `pub fn discard_blocks(start: u64, count: u64) -> Result<(), BlkError> {`

```
/// TRIM/Deallocate blocks via NVMe Dataset Management command.
/// start = block number (4KB blocks), count = number of blocks.
```

## L1396 · `return Ok(()); // No DSM support, silent no-op (same as virtio)`

```
// No DSM support, silent no-op (same as virtio)
```

## L1401-1404 · `let start_lba = start * (BLOCK_SIZE / SECTOR_SIZE) as u64;`

```
// Build Dataset Management Range entry (16 bytes)
// Offset 0: Context Attributes (4 bytes) — unused for deallocate
// Offset 4: Length in LBAs (4 bytes)
// Offset 8: Starting LBA (8 bytes)
```

## L1408 · `unsafe {`

```
// SAFETY: DMA buffer is a valid, identity-mapped 4KB page
```

## L1412 · `core::ptr::copy_nonoverlapping(`

```
// Length in LBAs at offset 4
```

## L1416 · `core::ptr::copy_nonoverlapping(`

```
// Starting LBA at offset 8
```

## L1426 · `cmd.cdw10 = 0;         // Number of ranges - 1 (0 = 1 range)`

```
// Number of ranges - 1 (0 = 1 range)
```

## L1427 · `cmd.cdw11 = 1 << 2;   // AD (Attribute Deallocate) bit`

```
// AD (Attribute Deallocate) bit
```

## L1433 · `pub fn model_name() -> Option<alloc::string::String> {`

```
/// Return model name for display.
```

## L1441 · `pub fn serial_number() -> Option<alloc::string::String> {`

```
/// Return serial number for display.
```

## L1449-1450 · `pub fn max_blocks_per_cmd() -> u32 {`

```
/// Maximum data per single READ/WRITE cmd in 4 KB blocks (clamped to
/// our DMA pool). Computed from MDTS at init.
```

## L1455-1457 · `pub fn msix_status() -> Option<(u8, u64)> {`

```
/// (LAPIC vector, total completion-IRQ fires) for the I/O-CQ MSI-X, or None
/// if MSI-X isn't set up (poll-only). For the `disk` diagnostic — `fires > 0`
/// proves the host-device-irq path works on this hardware.
```

## L1467-1469 · `pub fn msix_debug() -> Option<crate::pci::MsixDebug> {`

```
/// Live MSI-X capability + table-entry read-back for the `disk` diagnostic.
/// Shows whether our programming stuck (entry unmasked, MSI-X enabled, addr/
/// data correct) — so we can debug "programmed but no IRQ" without the boot log.
```

## L1476 · `pub fn capacity_gb() -> Option<u64> {`

```
/// Return capacity in GB.
```

