//! NVMe Driver (NVM Express over PCIe)
//!
//! Memory-mapped I/O via BAR0. Admin + I/O queue pairs.
//! Exposes same block API as virtio_blk for npkFS compatibility.

use crate::{kprintln, pci, paging, memory};
use crate::paging::PageFlags;
use crate::virtio_blk::BlkError;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

pub const SECTOR_SIZE: usize = 512;
pub const BLOCK_SIZE: usize = 4096;

// NVMe class: Mass Storage (01h), NVMe (08h)
const NVME_CLASS: u8 = 0x01;
const NVME_SUBCLASS: u8 = 0x08;

// NVMe controller registers (offsets from BAR0)
const REG_CAP: usize = 0x00;       // Controller Capabilities (64-bit)
const REG_VS: usize = 0x08;        // Version
const REG_INTMS: usize = 0x0C;     // Interrupt Mask Set
const REG_CC: usize = 0x14;        // Controller Configuration
const REG_CSTS: usize = 0x1C;      // Controller Status
const REG_AQA: usize = 0x24;       // Admin Queue Attributes
const REG_ASQ: usize = 0x28;       // Admin Submission Queue Base (64-bit)
const REG_ACQ: usize = 0x30;       // Admin Completion Queue Base (64-bit)

// Controller Configuration bits
const CC_EN: u32 = 1 << 0;         // Enable
const CC_CSS_NVM: u32 = 0 << 4;    // NVM Command Set
const CC_MPS_4K: u32 = 0 << 7;     // Memory Page Size = 4K (2^(12+0))
const CC_IOSQES: u32 = 6 << 16;    // I/O SQ Entry Size = 2^6 = 64
const CC_IOCQES: u32 = 4 << 20;    // I/O CQ Entry Size = 2^4 = 16

// Controller Status bits
const CSTS_RDY: u32 = 1 << 0;      // Ready

// Admin opcodes
const ADM_IDENTIFY: u8 = 0x06;
const ADM_CREATE_IO_CQ: u8 = 0x05;
const ADM_CREATE_IO_SQ: u8 = 0x01;

// NVM opcodes
const NVM_READ: u8 = 0x02;
const NVM_WRITE: u8 = 0x01;
const NVM_DSM: u8 = 0x09;   // Dataset Management (TRIM/Deallocate)

// Queue sizes (entries)
const ADMIN_QUEUE_SIZE: u16 = 16;
// 256 entries is universally supported by NVMe controllers (CAP.MQES on
// any modern SSD is well above this). 1024 worked on the test rig but
// blocks at-rest decryption if an SSD reports a smaller MQES — keycheck
// fails to read and the user sees "wrong passphrase" with no clue why.
const IO_QUEUE_SIZE: u16 = 256;

// Submission Queue Entry (64 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
struct SqEntry {
    opcode: u8,
    flags: u8,
    command_id: u16,
    nsid: u32,
    _rsvd: [u32; 2],
    metadata: u64,
    prp1: u64,
    prp2: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
}

impl SqEntry {
    const fn zeroed() -> Self {
        SqEntry {
            opcode: 0, flags: 0, command_id: 0, nsid: 0,
            _rsvd: [0; 2], metadata: 0, prp1: 0, prp2: 0,
            cdw10: 0, cdw11: 0, cdw12: 0, cdw13: 0, cdw14: 0, cdw15: 0,
        }
    }
}

// Completion Queue Entry (16 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
struct CqEntry {
    dw0: u32,           // Command-specific
    _rsvd: u32,
    sq_head: u16,
    sq_id: u16,
    command_id: u16,
    status: u16,        // Bit 0 = Phase, bits 1-15 = status
}

impl CqEntry {
    #[allow(dead_code)]
    const fn zeroed() -> Self {
        CqEntry { dw0: 0, _rsvd: 0, sq_head: 0, sq_id: 0, command_id: 0, status: 0 }
    }
}

struct NvmeState {
    bar0: u64,                  // Virtual address of mapped BAR0
    doorbell_stride: u32,       // Bytes between doorbells
    // Admin queue
    admin_sq: u64,              // Physical address
    admin_cq: u64,              // Physical address
    admin_sq_tail: u16,
    admin_cq_head: u16,
    admin_phase: bool,
    // I/O queue (QID 1)
    io_sq: u64,
    io_cq: u64,
    io_sq_tail: u16,
    io_cq_head: u16,
    io_phase: bool,
    // Device info
    total_lbas: u64,            // Total logical blocks (512-byte sectors)
    model: [u8; 40],
    serial: [u8; 20],
    oncs: u16,                  // Optional NVM Command Support (from Identify Controller)
    command_id: u16,
}

static NVME: Mutex<Option<NvmeState>> = Mutex::new(None);
static AVAILABLE: AtomicBool = AtomicBool::new(false);

// DMA buffer for data transfers (one 4KB page, identity-mapped)
static DMA_BUF: Mutex<Option<u64>> = Mutex::new(None);

/// Pool of pre-allocated 4 KB DMA buffers for batched I/O. With a single
/// shared `DMA_BUF`, every block transfer is necessarily synchronous —
/// the source/dest pointer would race otherwise. The pool lets us submit
/// up to `DMA_POOL_SLOTS` commands in flight, ring the doorbell once,
/// then collect all completions in a single drain.
///
/// 128 slots × 4 KB = 512 KB. Must stay strictly below `IO_QUEUE_SIZE`
/// since a full batch submits N commands and the SQ needs at least one
/// empty slot (head == tail means empty).
const DMA_POOL_SLOTS: usize = 128;
static DMA_POOL_BASE: Mutex<Option<u64>> = Mutex::new(None);

/// PRP-list scratch pool. NVMe spec: a transfer covering ≥3 4 KB pages
/// uses `prp1` for the first page and a "PRP List" — a 4 KB block
/// holding up to 512 page-pointers — addressed by `prp2`. Each cmd we
/// submit with PRP-List uses one slot; a 4-slot pool is enough for the
/// current synchronous one-cmd-at-a-time pattern, with headroom for
/// short pipelining bursts.
const PRP_LIST_POOL_SLOTS: usize = 4;
static PRP_LIST_POOL_BASE: Mutex<Option<u64>> = Mutex::new(None);

/// Max data blocks per single NVMe READ/WRITE command. Computed from
/// Identify Controller's MDTS field (offset 77) at init time; capped to
/// `DMA_POOL_SLOTS` so a single cmd fits in our DMA pool. Treated as a
/// hard upper bound by `read_extent` / `write_extent`; larger transfers
/// are split into multiple cmds.
static MAX_BLOCKS_PER_CMD: AtomicU32 = AtomicU32::new(64);

fn mmio_read32(base: u64, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + offset as u64) as *const u32) }
}

fn mmio_write32(base: u64, offset: usize, val: u32) {
    unsafe { core::ptr::write_volatile((base + offset as u64) as *mut u32, val); }
}

fn mmio_read64(base: u64, offset: usize) -> u64 {
    // NVMe spec: 64-bit registers may need two 32-bit reads
    let lo = mmio_read32(base, offset) as u64;
    let hi = mmio_read32(base, offset + 4) as u64;
    (hi << 32) | lo
}

fn mmio_write64(base: u64, offset: usize, val: u64) {
    mmio_write32(base, offset, val as u32);
    mmio_write32(base, offset + 4, (val >> 32) as u32);
}

/// Ring doorbell for a submission queue.
fn ring_sq_doorbell(state: &NvmeState, qid: u16, tail: u16) {
    let offset = 0x1000 + (2 * qid as usize) * state.doorbell_stride as usize;
    mmio_write32(state.bar0, offset, tail as u32);
}

/// Ring doorbell for a completion queue.
fn ring_cq_doorbell(state: &NvmeState, qid: u16, head: u16) {
    let offset = 0x1000 + (2 * qid as usize + 1) * state.doorbell_stride as usize;
    mmio_write32(state.bar0, offset, head as u32);
}

/// Submit a command to the admin queue and wait for completion.
fn admin_command(state: &mut NvmeState, mut cmd: SqEntry) -> Result<CqEntry, BlkError> {
    cmd.command_id = state.command_id;
    state.command_id = state.command_id.wrapping_add(1);

    // Write to admin SQ
    let sq_ptr = state.admin_sq as *mut SqEntry;
    unsafe { core::ptr::write_volatile(sq_ptr.add(state.admin_sq_tail as usize), cmd); }
    state.admin_sq_tail = (state.admin_sq_tail + 1) % ADMIN_QUEUE_SIZE;
    ring_sq_doorbell(state, 0, state.admin_sq_tail);

    // Poll completion
    let cq_ptr = state.admin_cq as *const CqEntry;
    for _ in 0..5_000_000u32 {
        let entry = unsafe { core::ptr::read_volatile(cq_ptr.add(state.admin_cq_head as usize)) };
        let phase = (entry.status & 1) != 0;
        if phase == state.admin_phase {
            // Advance CQ head
            state.admin_cq_head = (state.admin_cq_head + 1) % ADMIN_QUEUE_SIZE;
            if state.admin_cq_head == 0 { state.admin_phase = !state.admin_phase; }
            ring_cq_doorbell(state, 0, state.admin_cq_head);

            let status_code = (entry.status >> 1) & 0x7FF;
            if status_code != 0 {
                return Err(BlkError::IoError);
            }
            return Ok(entry);
        }
        core::hint::spin_loop();
    }
    Err(BlkError::Timeout)
}

/// Submit a command to I/O queue 1 and wait for completion.
fn io_command(state: &mut NvmeState, mut cmd: SqEntry) -> Result<CqEntry, BlkError> {
    cmd.command_id = state.command_id;
    state.command_id = state.command_id.wrapping_add(1);

    let sq_ptr = state.io_sq as *mut SqEntry;
    unsafe { core::ptr::write_volatile(sq_ptr.add(state.io_sq_tail as usize), cmd); }
    state.io_sq_tail = (state.io_sq_tail + 1) % IO_QUEUE_SIZE;
    ring_sq_doorbell(state, 1, state.io_sq_tail);

    let cq_ptr = state.io_cq as *const CqEntry;
    for _ in 0..5_000_000u32 {
        let entry = unsafe { core::ptr::read_volatile(cq_ptr.add(state.io_cq_head as usize)) };
        let phase = (entry.status & 1) != 0;
        if phase == state.io_phase {
            state.io_cq_head = (state.io_cq_head + 1) % IO_QUEUE_SIZE;
            if state.io_cq_head == 0 { state.io_phase = !state.io_phase; }
            ring_cq_doorbell(state, 1, state.io_cq_head);

            let status_code = (entry.status >> 1) & 0x7FF;
            if status_code != 0 {
                return Err(BlkError::IoError);
            }
            return Ok(entry);
        }
        core::hint::spin_loop();
    }
    Err(BlkError::Timeout)
}

/// Initialize the NVMe controller.
pub fn init() -> bool {
    // Find NVMe device by class code (01h:08h)
    let dev = match pci::find_by_class(NVME_CLASS, NVME_SUBCLASS) {
        Some(d) => d,
        None => return false,
    };

    kprintln!("[npk] nvme: PCI {:02x}:{:02x}.{} [{:04x}:{:04x}] IRQ {}",
        dev.addr.bus, dev.addr.device, dev.addr.function,
        dev.vendor_id, dev.device_id, dev.irq_line);

    // Enable bus mastering for DMA
    pci::enable_bus_master(dev.addr);

    // Read 64-bit BAR0
    let bar0_phys = pci::read_bar64(dev.addr, 0x10);
    if bar0_phys == 0 {
        kprintln!("[npk] nvme: BAR0 is zero, cannot initialize");
        return false;
    }

    // Map BAR0 pages (NVMe registers are typically 16KB-64KB)
    // Map 64KB to be safe (covers registers + doorbells)
    let map_size = 64 * 1024u64;
    let bar0_virt = bar0_phys; // Identity-mapped (64GB range covers all PCIe BARs)
    for offset in (0..map_size).step_by(4096) {
        let paddr = bar0_phys + offset;
        let vaddr = bar0_virt + offset;
        match paging::map_page(vaddr, paddr,
            PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_CACHE) {
            Ok(()) => {}
            Err(paging::PagingError::AlreadyMapped) => {} // Identity-mapped region
            Err(e) => {
                kprintln!("[npk] nvme: failed to map BAR0 at {:#x}: {:?}", paddr, e);
                return false;
            }
        }
    }

    // Read capabilities
    let cap = mmio_read64(bar0_virt, REG_CAP);
    let doorbell_stride = 4u32 << ((cap >> 32) & 0xF) as u32; // DSTRD field
    let max_queue_entries = (cap & 0xFFFF) as u16 + 1;
    let version = mmio_read32(bar0_virt, REG_VS);

    kprintln!("[npk] nvme: version {}.{}.{}, max queue {}",
        version >> 16, (version >> 8) & 0xFF, version & 0xFF,
        max_queue_entries);

    if (IO_QUEUE_SIZE as u16) > max_queue_entries {
        kprintln!("[npk] nvme: IO_QUEUE_SIZE={} exceeds CAP.MQES+1={}, refusing init",
            IO_QUEUE_SIZE, max_queue_entries);
        return false;
    }

    // Disable controller
    let cc = mmio_read32(bar0_virt, REG_CC);
    if cc & CC_EN != 0 {
        mmio_write32(bar0_virt, REG_CC, cc & !CC_EN);
        // Wait for not ready
        for _ in 0..1_000_000u32 {
            if mmio_read32(bar0_virt, REG_CSTS) & CSTS_RDY == 0 { break; }
            core::hint::spin_loop();
        }
    }

    // Allocate Admin Submission Queue (ADMIN_QUEUE_SIZE * 64 bytes)
    let admin_sq_pages = ((ADMIN_QUEUE_SIZE as usize * 64) + 4095) / 4096;
    let admin_sq = match memory::allocate_contiguous(admin_sq_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] nvme: failed to allocate admin SQ"); return false; }
    };
    // Zero the queue
    unsafe { core::ptr::write_bytes(admin_sq as *mut u8, 0, admin_sq_pages * 4096); }

    // Allocate Admin Completion Queue (ADMIN_QUEUE_SIZE * 16 bytes)
    let admin_cq_pages = ((ADMIN_QUEUE_SIZE as usize * 16) + 4095) / 4096;
    let admin_cq = match memory::allocate_contiguous(admin_cq_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] nvme: failed to allocate admin CQ"); return false; }
    };
    unsafe { core::ptr::write_bytes(admin_cq as *mut u8, 0, admin_cq_pages * 4096); }

    // Configure admin queues
    let aqa = ((ADMIN_QUEUE_SIZE as u32 - 1) << 16) | (ADMIN_QUEUE_SIZE as u32 - 1);
    mmio_write32(bar0_virt, REG_AQA, aqa);
    mmio_write64(bar0_virt, REG_ASQ, admin_sq);
    mmio_write64(bar0_virt, REG_ACQ, admin_cq);

    // Mask all interrupts (we poll)
    mmio_write32(bar0_virt, REG_INTMS, 0xFFFF_FFFF);

    // Enable controller
    let cc_val = CC_EN | CC_CSS_NVM | CC_MPS_4K | CC_IOSQES | CC_IOCQES;
    mmio_write32(bar0_virt, REG_CC, cc_val);

    // Wait for ready
    for _ in 0..5_000_000u32 {
        if mmio_read32(bar0_virt, REG_CSTS) & CSTS_RDY != 0 { break; }
        core::hint::spin_loop();
    }
    if mmio_read32(bar0_virt, REG_CSTS) & CSTS_RDY == 0 {
        kprintln!("[npk] nvme: controller did not become ready");
        return false;
    }

    let mut state = NvmeState {
        bar0: bar0_virt,
        doorbell_stride: doorbell_stride,
        admin_sq, admin_cq,
        admin_sq_tail: 0, admin_cq_head: 0, admin_phase: true,
        io_sq: 0, io_cq: 0,
        io_sq_tail: 0, io_cq_head: 0, io_phase: true,
        total_lbas: 0,
        model: [0; 40],
        serial: [0; 20],
        oncs: 0,
        command_id: 1,
    };

    // Allocate DMA buffer for Identify data (4KB)
    let identify_buf = match memory::allocate_contiguous(1) {
        Some(a) => a,
        None => { kprintln!("[npk] nvme: failed to allocate identify buf"); return false; }
    };
    unsafe { core::ptr::write_bytes(identify_buf as *mut u8, 0, 4096); }

    // Identify Controller (CNS=1)
    let mut cmd = SqEntry::zeroed();
    cmd.opcode = ADM_IDENTIFY;
    cmd.nsid = 0;
    cmd.prp1 = identify_buf;
    cmd.cdw10 = 1; // CNS=1: Identify Controller
    if admin_command(&mut state, cmd).is_err() {
        kprintln!("[npk] nvme: Identify Controller failed");
        return false;
    }

    // Parse controller info
    let buf = identify_buf as *const u8;
    unsafe {
        // Serial number: bytes 4-23
        core::ptr::copy_nonoverlapping(buf.add(4), state.serial.as_mut_ptr(), 20);
        // Model number: bytes 24-63
        core::ptr::copy_nonoverlapping(buf.add(24), state.model.as_mut_ptr(), 40);
    }

    // ONCS (Optional NVM Command Support) at offset 256 (2 bytes)
    // Bit 2 = Dataset Management (TRIM/Deallocate)
    unsafe {
        state.oncs = core::ptr::read_volatile(buf.add(256) as *const u16);
    }

    // MDTS (Maximum Data Transfer Size) at offset 77, 1 byte. Units of
    // 2^(12 + CAP.MPSMIN) bytes; for MPSMIN=0 (~all NVMe SSDs) that's
    // pages of 4 KB. 0 means "no limit". Cap at DMA_POOL_SLOTS so a
    // single cmd always fits in our staging pool.
    let mdts: u8 = unsafe { core::ptr::read_volatile(buf.add(77) as *const u8) };
    let mpsmin = ((cap >> 48) & 0xF) as u32;
    let pool_cap = DMA_POOL_SLOTS as u32;
    let hw_max_blocks = if mdts == 0 {
        pool_cap
    } else {
        let pages = 1u32 << (mdts as u32);
        let bytes_per_page = 1u32 << (12 + mpsmin);
        let blocks = pages.saturating_mul(bytes_per_page) / BLOCK_SIZE as u32;
        blocks.max(1)
    };
    let max_blocks_per_cmd = hw_max_blocks.min(pool_cap);
    MAX_BLOCKS_PER_CMD.store(max_blocks_per_cmd, Ordering::Relaxed);

    let model_str = core::str::from_utf8(&state.model).unwrap_or("?").trim();
    let serial_str = core::str::from_utf8(&state.serial).unwrap_or("?").trim();
    let has_trim = state.oncs & (1 << 2) != 0;
    kprintln!("[npk] nvme: {} (SN: {}), TRIM={}, MDTS={} ({} KB/cmd)",
        model_str, serial_str,
        if has_trim { "yes" } else { "no" },
        mdts, max_blocks_per_cmd * 4);

    // Identify Namespace 1 (CNS=0, NSID=1)
    unsafe { core::ptr::write_bytes(identify_buf as *mut u8, 0, 4096); }
    let mut cmd = SqEntry::zeroed();
    cmd.opcode = ADM_IDENTIFY;
    cmd.nsid = 1;
    cmd.prp1 = identify_buf;
    cmd.cdw10 = 0; // CNS=0: Identify Namespace
    if admin_command(&mut state, cmd).is_err() {
        kprintln!("[npk] nvme: Identify Namespace failed");
        return false;
    }

    // NSZE (Namespace Size) at offset 0 (8 bytes, little-endian)
    let nsze = unsafe { core::ptr::read_volatile(identify_buf as *const u64) };
    state.total_lbas = nsze;

    let size_mb = (nsze * 512) / (1024 * 1024);
    let size_gb = size_mb / 1024;
    if size_gb > 0 {
        kprintln!("[npk] nvme: {} GB ({} sectors)", size_gb, nsze);
    } else {
        kprintln!("[npk] nvme: {} MB ({} sectors)", size_mb, nsze);
    }

    // Create I/O Completion Queue (QID=1)
    let io_cq_pages = ((IO_QUEUE_SIZE as usize * 16) + 4095) / 4096;
    let io_cq = match memory::allocate_contiguous(io_cq_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] nvme: failed to allocate I/O CQ"); return false; }
    };
    unsafe { core::ptr::write_bytes(io_cq as *mut u8, 0, io_cq_pages * 4096); }

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = ADM_CREATE_IO_CQ;
    cmd.prp1 = io_cq;
    cmd.cdw10 = ((IO_QUEUE_SIZE as u32 - 1) << 16) | 1; // QID=1, size
    cmd.cdw11 = 1; // Physically contiguous
    if admin_command(&mut state, cmd).is_err() {
        kprintln!("[npk] nvme: Create I/O CQ failed");
        return false;
    }

    // Create I/O Submission Queue (QID=1, CQ=1)
    let io_sq_pages = ((IO_QUEUE_SIZE as usize * 64) + 4095) / 4096;
    let io_sq = match memory::allocate_contiguous(io_sq_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] nvme: failed to allocate I/O SQ"); return false; }
    };
    unsafe { core::ptr::write_bytes(io_sq as *mut u8, 0, io_sq_pages * 4096); }

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = ADM_CREATE_IO_SQ;
    cmd.prp1 = io_sq;
    cmd.cdw10 = ((IO_QUEUE_SIZE as u32 - 1) << 16) | 1; // QID=1, size
    cmd.cdw11 = (1 << 16) | 1; // CQID=1, Physically contiguous
    if admin_command(&mut state, cmd).is_err() {
        kprintln!("[npk] nvme: Create I/O SQ failed");
        return false;
    }

    state.io_sq = io_sq;
    state.io_cq = io_cq;

    // Allocate DMA buffer for block I/O
    let dma = match memory::allocate_contiguous(1) {
        Some(a) => a,
        None => { kprintln!("[npk] nvme: failed to allocate DMA buf"); return false; }
    };
    *DMA_BUF.lock() = Some(dma);

    // Allocate the batched-I/O DMA pool. Failure here just means
    // batched paths fall back to the single-buffer slow path; the
    // device still works.
    if let Some(pool) = memory::allocate_contiguous(DMA_POOL_SLOTS) {
        *DMA_POOL_BASE.lock() = Some(pool);
    } else {
        kprintln!("[npk] nvme: WARN — could not allocate {}-page DMA pool, batched flush disabled",
            DMA_POOL_SLOTS);
    }

    // Allocate the PRP-list scratch pool. Each multi-page extent cmd
    // takes one slot. Without this we can still service single-block
    // reads/writes via `read_block` / `write_block`, so failure is
    // recoverable but disables the fast path.
    if let Some(pool) = memory::allocate_contiguous(PRP_LIST_POOL_SLOTS) {
        *PRP_LIST_POOL_BASE.lock() = Some(pool);
    } else {
        kprintln!("[npk] nvme: WARN — could not allocate {}-page PRP-list pool, extent path disabled",
            PRP_LIST_POOL_SLOTS);
    }

    // Free identify buffer
    memory::deallocate_frame(identify_buf);

    kprintln!("[npk] nvme: online");
    AVAILABLE.store(true, Ordering::Relaxed);
    *NVME.lock() = Some(state);
    true
}

pub fn is_available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

pub fn capacity() -> Option<u64> {
    NVME.lock().as_ref().map(|s| s.total_lbas)
}

pub fn block_count() -> Option<u64> {
    capacity().map(|sectors| sectors / (BLOCK_SIZE / SECTOR_SIZE) as u64)
}

/// Read a 512-byte sector.
pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;
    if sector >= state.total_lbas { return Err(BlkError::OutOfRange); }

    let dma = DMA_BUF.lock().ok_or(BlkError::NotInitialized)?;

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = NVM_READ;
    cmd.nsid = 1;
    cmd.prp1 = dma;
    cmd.cdw10 = sector as u32;         // Starting LBA (low)
    cmd.cdw11 = (sector >> 32) as u32; // Starting LBA (high)
    cmd.cdw12 = 0;                     // Number of LBs - 1 (0 = 1 sector)

    io_command(state, cmd)?;

    unsafe { core::ptr::copy_nonoverlapping(dma as *const u8, buf.as_mut_ptr(), SECTOR_SIZE); }
    Ok(())
}

/// Write a 512-byte sector.
pub fn write_sector(sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;
    if sector >= state.total_lbas { return Err(BlkError::OutOfRange); }

    let dma = DMA_BUF.lock().ok_or(BlkError::NotInitialized)?;
    unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), dma as *mut u8, SECTOR_SIZE); }

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = NVM_WRITE;
    cmd.nsid = 1;
    cmd.prp1 = dma;
    cmd.cdw10 = sector as u32;
    cmd.cdw11 = (sector >> 32) as u32;
    cmd.cdw12 = 0; // 1 sector

    io_command(state, cmd)?;
    Ok(())
}

/// Read a 4KB block (8 sectors).
pub fn read_block(block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;
    let sector = block * (BLOCK_SIZE / SECTOR_SIZE) as u64;
    if sector + 7 >= state.total_lbas { return Err(BlkError::OutOfRange); }

    let dma = DMA_BUF.lock().ok_or(BlkError::NotInitialized)?;

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = NVM_READ;
    cmd.nsid = 1;
    cmd.prp1 = dma;
    cmd.cdw10 = sector as u32;
    cmd.cdw11 = (sector >> 32) as u32;
    cmd.cdw12 = 7; // 8 sectors - 1

    io_command(state, cmd)?;

    unsafe { core::ptr::copy_nonoverlapping(dma as *const u8, buf.as_mut_ptr(), BLOCK_SIZE); }
    Ok(())
}

/// Submit up to `DMA_POOL_SLOTS` write-block commands in parallel and
/// wait for them all. Returns Ok only if every command completed
/// without error. Falls back to per-block sequential writes when the
/// batch is too large or the pool isn't available.
///
/// This is the path `cache::flush` takes once it has more than one
/// dirty block — replaces N sequential synchronous writes (N × disk
/// latency) with one batch (~1 × disk latency, modulo NVMe ordering).
pub fn write_blocks_batch(items: &[(u64, &[u8; BLOCK_SIZE])]) -> Result<(), BlkError> {
    if items.is_empty() { return Ok(()); }

    // Batch larger than the pool → fall back to sequential. We could
    // chunk this internally but `cache::flush` won't ever exceed the
    // cache slot count (64 in practice, our pool holds 32 — chunk
    // boundary handled here).
    if items.len() > DMA_POOL_SLOTS {
        for &(block, buf) in items {
            write_block(block, buf)?;
        }
        return Ok(());
    }

    let pool_base = match *DMA_POOL_BASE.lock() {
        Some(addr) => addr,
        None => {
            // Pool wasn't allocated — fall back.
            for &(block, buf) in items {
                write_block(block, buf)?;
            }
            return Ok(());
        }
    };

    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;

    // Stage every payload into its own DMA pool slot + push every SQ
    // entry, then ring the doorbell exactly once. The SQ has 64 slots;
    // `items.len() ≤ DMA_POOL_SLOTS = 32`, so we can't wrap into our
    // own un-acked head.
    for (i, &(block, buf)) in items.iter().enumerate() {
        let sector = block * (BLOCK_SIZE / SECTOR_SIZE) as u64;
        if sector + 7 >= state.total_lbas { return Err(BlkError::OutOfRange); }

        let dma = pool_base + (i as u64) * BLOCK_SIZE as u64;
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), dma as *mut u8, BLOCK_SIZE); }

        let mut cmd = SqEntry::zeroed();
        cmd.opcode = NVM_WRITE;
        cmd.command_id = state.command_id;
        cmd.nsid = 1;
        cmd.prp1 = dma;
        cmd.cdw10 = sector as u32;
        cmd.cdw11 = (sector >> 32) as u32;
        cmd.cdw12 = 7;
        state.command_id = state.command_id.wrapping_add(1);

        let sq_ptr = state.io_sq as *mut SqEntry;
        unsafe { core::ptr::write_volatile(sq_ptr.add(state.io_sq_tail as usize), cmd); }
        state.io_sq_tail = (state.io_sq_tail + 1) % IO_QUEUE_SIZE;
    }

    // Memory fence between SQ entry stores and the doorbell write.
    // x86 stores are normally ordered, but during the deadlock chase
    // adding kprintlns between submit + drain made the issue go away —
    // those acted as MMIO-serializing barriers. An explicit SeqCst
    // fence pins the ordering deterministically without the serial
    // overhead, and matches what real NVMe drivers do.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    ring_sq_doorbell(state, 1, state.io_sq_tail);

    let mut completed = 0usize;
    let mut overall_err: Option<BlkError> = None;
    let mut spin_budget = 5_000_000u32 * items.len() as u32;
    while completed < items.len() {
        if spin_budget == 0 {
            return Err(BlkError::Timeout);
        }
        spin_budget -= 1;

        let cq_ptr = state.io_cq as *const CqEntry;
        let entry = unsafe { core::ptr::read_volatile(cq_ptr.add(state.io_cq_head as usize)) };
        let phase = (entry.status & 1) != 0;
        if phase != state.io_phase {
            core::hint::spin_loop();
            continue;
        }

        state.io_cq_head = (state.io_cq_head + 1) % IO_QUEUE_SIZE;
        if state.io_cq_head == 0 { state.io_phase = !state.io_phase; }

        let status_code = (entry.status >> 1) & 0x7FF;
        if status_code != 0 && overall_err.is_none() {
            overall_err = Some(BlkError::IoError);
        }
        completed += 1;
    }
    ring_cq_doorbell(state, 1, state.io_cq_head);
    overall_err.map_or(Ok(()), Err)
}

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
pub fn read_blocks_batch(blocks: &[u64], output: &mut [u8]) -> Result<(), BlkError> {
    if blocks.is_empty() { return Ok(()); }
    if output.len() != blocks.len() * BLOCK_SIZE {
        return Err(BlkError::OutOfRange);
    }

    let pool_base = match *DMA_POOL_BASE.lock() {
        Some(addr) => addr,
        None => {
            for (i, &block) in blocks.iter().enumerate() {
                let dst = &mut output[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE];
                let dst_arr: &mut [u8; BLOCK_SIZE] = dst.try_into().unwrap();
                read_block(block, dst_arr)?;
            }
            return Ok(());
        }
    };

    // Chunk into pool-sized groups so a 256-block fetch (1 MB blob)
    // runs as 8 × 32-block parallel batches instead of 256 serial.
    if blocks.len() > DMA_POOL_SLOTS {
        let mut offset = 0;
        while offset < blocks.len() {
            let take = (blocks.len() - offset).min(DMA_POOL_SLOTS);
            let chunk_blocks = &blocks[offset..offset + take];
            let chunk_out = &mut output[offset * BLOCK_SIZE..(offset + take) * BLOCK_SIZE];
            read_blocks_batch_inner(chunk_blocks, chunk_out, pool_base)?;
            offset += take;
        }
        return Ok(());
    }

    read_blocks_batch_inner(blocks, output, pool_base)
}

/// Single ≤ DMA_POOL_SLOTS read batch. Pool-based, queue-depth ≈ N.
fn read_blocks_batch_inner(blocks: &[u64], output: &mut [u8], pool_base: u64) -> Result<(), BlkError> {

    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;

    for (i, &block) in blocks.iter().enumerate() {
        let sector = block * (BLOCK_SIZE / SECTOR_SIZE) as u64;
        if sector + 7 >= state.total_lbas { return Err(BlkError::OutOfRange); }

        let dma = pool_base + (i as u64) * BLOCK_SIZE as u64;

        let mut cmd = SqEntry::zeroed();
        cmd.opcode = NVM_READ;
        cmd.command_id = state.command_id;
        cmd.nsid = 1;
        cmd.prp1 = dma;
        cmd.cdw10 = sector as u32;
        cmd.cdw11 = (sector >> 32) as u32;
        cmd.cdw12 = 7;
        state.command_id = state.command_id.wrapping_add(1);

        let sq_ptr = state.io_sq as *mut SqEntry;
        unsafe { core::ptr::write_volatile(sq_ptr.add(state.io_sq_tail as usize), cmd); }
        state.io_sq_tail = (state.io_sq_tail + 1) % IO_QUEUE_SIZE;
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    ring_sq_doorbell(state, 1, state.io_sq_tail);

    let mut completed = 0usize;
    let mut overall_err: Option<BlkError> = None;
    let mut spin_budget = 5_000_000u32 * blocks.len() as u32;
    while completed < blocks.len() {
        if spin_budget == 0 { return Err(BlkError::Timeout); }
        spin_budget -= 1;

        let cq_ptr = state.io_cq as *const CqEntry;
        let entry = unsafe { core::ptr::read_volatile(cq_ptr.add(state.io_cq_head as usize)) };
        let phase = (entry.status & 1) != 0;
        if phase != state.io_phase {
            core::hint::spin_loop();
            continue;
        }

        state.io_cq_head = (state.io_cq_head + 1) % IO_QUEUE_SIZE;
        if state.io_cq_head == 0 { state.io_phase = !state.io_phase; }

        let status_code = (entry.status >> 1) & 0x7FF;
        if status_code != 0 && overall_err.is_none() {
            overall_err = Some(BlkError::IoError);
        }
        completed += 1;
    }
    ring_cq_doorbell(state, 1, state.io_cq_head);

    // Ensure DMA writes are visible before we read from the pool.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    if overall_err.is_none() {
        for i in 0..blocks.len() {
            let dma = pool_base + (i as u64) * BLOCK_SIZE as u64;
            let dst = &mut output[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE];
            unsafe {
                core::ptr::copy_nonoverlapping(dma as *const u8, dst.as_mut_ptr(), BLOCK_SIZE);
            }
        }
    }

    overall_err.map_or(Ok(()), Err)
}

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

/// Build a PRP-list scratch block in `list_addr`. Page 0 of the
/// transfer goes into `cmd.prp1` directly; this list addresses pages
/// `1..count` (inclusive of the second page, exclusive of `count`).
/// Caller must ensure `count >= 3` and `count - 1 <= 512`.
fn build_prp_list(list_addr: u64, dma_base: u64, count: u64) {
    debug_assert!(count >= 3 && count - 1 <= 512);
    unsafe {
        let entries = list_addr as *mut u64;
        for j in 0..(count - 1) {
            let page_addr = dma_base + (j + 1) * BLOCK_SIZE as u64;
            core::ptr::write_volatile(entries.add(j as usize), page_addr);
        }
    }
}

/// Drain one I/O completion from the IO queue. Caller must have
/// submitted a single cmd and rung the doorbell.
fn drain_one_completion(state: &mut NvmeState) -> Result<(), BlkError> {
    let mut budget = 50_000_000u32;
    loop {
        if budget == 0 { return Err(BlkError::Timeout); }
        budget -= 1;

        let cq_ptr = state.io_cq as *const CqEntry;
        let entry = unsafe { core::ptr::read_volatile(cq_ptr.add(state.io_cq_head as usize)) };
        let phase = (entry.status & 1) != 0;
        if phase != state.io_phase {
            core::hint::spin_loop();
            continue;
        }
        state.io_cq_head = (state.io_cq_head + 1) % IO_QUEUE_SIZE;
        if state.io_cq_head == 0 { state.io_phase = !state.io_phase; }
        ring_cq_doorbell(state, 1, state.io_cq_head);

        let status_code = (entry.status >> 1) & 0x7FF;
        if status_code != 0 { return Err(BlkError::IoError); }
        return Ok(());
    }
}

/// Submit a single READ command spanning `chunk_blocks` consecutive
/// 4 KB blocks starting at `start_sector`. `chunk_blocks` must be ≤
/// `DMA_POOL_SLOTS` and ≤ `MAX_BLOCKS_PER_CMD`. Caller drains.
fn submit_extent_cmd(
    state: &mut NvmeState,
    opcode: u8,
    start_sector: u64,
    chunk_blocks: u64,
    dma_base: u64,
    prp_list_addr: u64,
) {
    let prp1 = dma_base;
    let prp2 = if chunk_blocks == 1 {
        0
    } else if chunk_blocks == 2 {
        dma_base + BLOCK_SIZE as u64
    } else {
        build_prp_list(prp_list_addr, dma_base, chunk_blocks);
        prp_list_addr
    };

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = opcode;
    cmd.command_id = state.command_id;
    cmd.nsid = 1;
    cmd.prp1 = prp1;
    cmd.prp2 = prp2;
    cmd.cdw10 = start_sector as u32;
    cmd.cdw11 = (start_sector >> 32) as u32;
    cmd.cdw12 = (chunk_blocks as u32 * 8 - 1) & 0xFFFF;
    state.command_id = state.command_id.wrapping_add(1);

    let sq_ptr = state.io_sq as *mut SqEntry;
    unsafe { core::ptr::write_volatile(sq_ptr.add(state.io_sq_tail as usize), cmd); }
    state.io_sq_tail = (state.io_sq_tail + 1) % IO_QUEUE_SIZE;

    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    ring_sq_doorbell(state, 1, state.io_sq_tail);
}

/// Read `count` 4 KB blocks starting at `start_block` into `output`.
/// Single NVMe cmd per `MAX_BLOCKS_PER_CMD` chunk via the PRP-list
/// path, instead of one cmd per block. `output.len()` must equal
/// `count * BLOCK_SIZE`. Falls back to per-block `read_block` when the
/// DMA pool / PRP list pool aren't initialised.
pub fn read_extent(start_block: u64, count: u64, output: &mut [u8]) -> Result<(), BlkError> {
    if count == 0 { return Ok(()); }
    if output.len() != (count as usize) * BLOCK_SIZE {
        return Err(BlkError::OutOfRange);
    }

    let pool_base = match *DMA_POOL_BASE.lock() {
        Some(b) => b,
        None => {
            for i in 0..count {
                let dst: &mut [u8; BLOCK_SIZE] = (&mut output
                    [(i as usize) * BLOCK_SIZE..((i as usize) + 1) * BLOCK_SIZE])
                    .try_into().unwrap();
                read_block(start_block + i, dst)?;
            }
            return Ok(());
        }
    };
    let prp_pool_base = (*PRP_LIST_POOL_BASE.lock()).ok_or(BlkError::NotInitialized)?;

    let max_per_cmd = (MAX_BLOCKS_PER_CMD.load(Ordering::Relaxed) as u64)
        .min(DMA_POOL_SLOTS as u64);

    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;

    let mut offset = 0u64;
    let mut prp_slot = 0usize;
    while offset < count {
        let chunk = (count - offset).min(max_per_cmd);
        let start_sector = (start_block + offset) * (BLOCK_SIZE / SECTOR_SIZE) as u64;
        if start_sector + chunk * 8 > state.total_lbas {
            return Err(BlkError::OutOfRange);
        }

        let prp_list_addr = prp_pool_base + (prp_slot as u64) * BLOCK_SIZE as u64;
        prp_slot = (prp_slot + 1) % PRP_LIST_POOL_SLOTS;

        submit_extent_cmd(state, NVM_READ, start_sector, chunk, pool_base, prp_list_addr);
        drain_one_completion(state)?;

        // DMA visibility — the SSD's writes to the pool must be flushed
        // through the CPU's view before we copy out.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        unsafe {
            core::ptr::copy_nonoverlapping(
                pool_base as *const u8,
                output[(offset as usize) * BLOCK_SIZE..].as_mut_ptr(),
                (chunk as usize) * BLOCK_SIZE,
            );
        }

        offset += chunk;
    }

    Ok(())
}

/// Write `count` 4 KB blocks of `input` starting at `start_block`.
/// Single NVMe cmd per `MAX_BLOCKS_PER_CMD` chunk via PRP-list.
/// `input.len()` must equal `count * BLOCK_SIZE`. Falls back to
/// per-block `write_block` when DMA / PRP pools aren't initialised.
pub fn write_extent(start_block: u64, count: u64, input: &[u8]) -> Result<(), BlkError> {
    if count == 0 { return Ok(()); }
    if input.len() != (count as usize) * BLOCK_SIZE {
        return Err(BlkError::OutOfRange);
    }

    let pool_base = match *DMA_POOL_BASE.lock() {
        Some(b) => b,
        None => {
            for i in 0..count {
                let src: &[u8; BLOCK_SIZE] = input
                    [(i as usize) * BLOCK_SIZE..((i as usize) + 1) * BLOCK_SIZE]
                    .try_into().unwrap();
                write_block(start_block + i, src)?;
            }
            return Ok(());
        }
    };
    let prp_pool_base = (*PRP_LIST_POOL_BASE.lock()).ok_or(BlkError::NotInitialized)?;

    let max_per_cmd = (MAX_BLOCKS_PER_CMD.load(Ordering::Relaxed) as u64)
        .min(DMA_POOL_SLOTS as u64);

    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;

    let mut offset = 0u64;
    let mut prp_slot = 0usize;
    while offset < count {
        let chunk = (count - offset).min(max_per_cmd);
        let start_sector = (start_block + offset) * (BLOCK_SIZE / SECTOR_SIZE) as u64;
        if start_sector + chunk * 8 > state.total_lbas {
            return Err(BlkError::OutOfRange);
        }

        unsafe {
            core::ptr::copy_nonoverlapping(
                input[(offset as usize) * BLOCK_SIZE..].as_ptr(),
                pool_base as *mut u8,
                (chunk as usize) * BLOCK_SIZE,
            );
        }
        // Make CPU stores to the pool visible before the SSD reads them.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        let prp_list_addr = prp_pool_base + (prp_slot as u64) * BLOCK_SIZE as u64;
        prp_slot = (prp_slot + 1) % PRP_LIST_POOL_SLOTS;

        submit_extent_cmd(state, NVM_WRITE, start_sector, chunk, pool_base, prp_list_addr);
        drain_one_completion(state)?;

        offset += chunk;
    }

    Ok(())
}

/// Write a 4KB block (8 sectors).
pub fn write_block(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;
    let sector = block * (BLOCK_SIZE / SECTOR_SIZE) as u64;
    if sector + 7 >= state.total_lbas { return Err(BlkError::OutOfRange); }

    let dma = DMA_BUF.lock().ok_or(BlkError::NotInitialized)?;
    unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), dma as *mut u8, BLOCK_SIZE); }

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = NVM_WRITE;
    cmd.nsid = 1;
    cmd.prp1 = dma;
    cmd.cdw10 = sector as u32;
    cmd.cdw11 = (sector >> 32) as u32;
    cmd.cdw12 = 7; // 8 sectors - 1

    io_command(state, cmd)?;
    Ok(())
}

pub fn has_discard() -> bool {
    match NVME.lock().as_ref() {
        Some(state) => state.oncs & (1 << 2) != 0, // ONCS bit 2 = Dataset Management
        None => false,
    }
}

/// TRIM/Deallocate blocks via NVMe Dataset Management command.
/// start = block number (4KB blocks), count = number of blocks.
pub fn discard_blocks(start: u64, count: u64) -> Result<(), BlkError> {
    if count == 0 { return Ok(()); }

    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;

    if state.oncs & (1 << 2) == 0 {
        return Ok(()); // No DSM support, silent no-op (same as virtio)
    }

    let dma = DMA_BUF.lock().ok_or(BlkError::NotInitialized)?;

    // Build Dataset Management Range entry (16 bytes)
    // Offset 0: Context Attributes (4 bytes) — unused for deallocate
    // Offset 4: Length in LBAs (4 bytes)
    // Offset 8: Starting LBA (8 bytes)
    let start_lba = start * (BLOCK_SIZE / SECTOR_SIZE) as u64;
    let lba_count = count * (BLOCK_SIZE / SECTOR_SIZE) as u64;

    // SAFETY: DMA buffer is a valid, identity-mapped 4KB page
    unsafe {
        let range = dma as *mut u8;
        core::ptr::write_bytes(range, 0, 16);
        // Length in LBAs at offset 4
        core::ptr::copy_nonoverlapping(
            &(lba_count as u32).to_le_bytes() as *const u8,
            range.add(4), 4);
        // Starting LBA at offset 8
        core::ptr::copy_nonoverlapping(
            &start_lba.to_le_bytes() as *const u8,
            range.add(8), 8);
    }

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = NVM_DSM;
    cmd.nsid = 1;
    cmd.prp1 = dma;
    cmd.cdw10 = 0;         // Number of ranges - 1 (0 = 1 range)
    cmd.cdw11 = 1 << 2;   // AD (Attribute Deallocate) bit

    io_command(state, cmd)?;
    Ok(())
}

/// Return model name for display.
pub fn model_name() -> Option<alloc::string::String> {
    let nvme = NVME.lock();
    let state = nvme.as_ref()?;
    let s = core::str::from_utf8(&state.model).unwrap_or("?").trim();
    Some(alloc::string::String::from(s))
}

#[allow(dead_code)]
/// Return capacity in GB.
pub fn capacity_gb() -> Option<u64> {
    capacity().map(|sectors| (sectors * 512) / (1024 * 1024 * 1024))
}
