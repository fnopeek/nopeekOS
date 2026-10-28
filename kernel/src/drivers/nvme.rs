//! NVMe Driver (NVM Express over PCIe)
//!
//! Memory-mapped I/O via BAR0. Admin + I/O queue pairs.
//! Exposes same block API as virtio_blk for npkFS compatibility.

use alloc::vec::Vec;
use crate::{kprintln, pci, memory};
use crate::hw::{DmaRegion, Mmio};
use crate::virtio_blk::BlkError;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
const REG_INTMC: usize = 0x10;     // Interrupt Mask Clear
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
const ADM_SET_FEATURES: u8 = 0x09;

// NVM opcodes
const NVM_FLUSH: u8 = 0x00; // Flush volatile write cache to stable media
const NVM_READ: u8 = 0x02;
const NVM_WRITE: u8 = 0x01;
const NVM_DSM: u8 = 0x09;   // Dataset Management (TRIM/Deallocate)

// Queue sizes (entries)
const ADMIN_QUEUE_SIZE: u16 = 16;
/// I/O queue depth asked for; the controller's CAP.MQES may lower it, as in
/// Linux `nvme_pci_enable` (`io_queue_depth`).
const IO_QUEUE_DEPTH: u32 = 1024;
/// Spec minimum for an I/O queue (CAP.MQES >= 1).
const IO_QUEUE_MIN: u32 = 2;

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
    regs: Mmio,                 // BAR0
    doorbell_stride: u32,       // Bytes between doorbells
    // Admin queue
    admin_sq: DmaRegion,
    admin_cq: DmaRegion,
    admin_sq_tail: u16,
    admin_cq_head: u16,
    admin_phase: bool,
    // I/O queue (QID 1)
    io_sq: DmaRegion,
    io_cq: DmaRegion,
    io_sq_tail: u16,
    io_cq_head: u16,
    io_phase: bool,
    io_q_depth: u16,
    // Device info
    total_lbas: u64,            // Total logical blocks (512-byte sectors)
    model: [u8; 40],
    serial: [u8; 20],
    oncs: u16,                  // Optional NVM Command Support (from Identify Controller)
    command_id: u16,
    /// I/O commands submitted and not yet completed. Every I/O path leaves
    /// it at zero: a later command must never take an earlier one's
    /// completion as its own.
    io_inflight: u16,
    /// Free I/O command ids. An id indexes `io_busy` and `prp`, so a
    /// completion finds its command's resources, as blk-mq tags do.
    io_free: Vec<u16>,
    io_busy: Vec<bool>,
    /// One PRP list page per command id.
    prp: Vec<DmaRegion>,
    /// A completion did not arrive in time. Its command may still run and
    /// DMA at any moment, so the queue is closed for good: refusing I/O is
    /// safe, guessing which completion belongs to whom is not.
    io_dead: bool,
    msix_vector: crate::irq::Irq, // interrupt of the I/O CQ completion MSI-X (0 = none/poll)
    /// The controller uses host memory (`setup_host_mem`).
    host_mem: bool,
    pci_addr: pci::PciAddr,     // for live MSI-X read-back (disk diagnostic)
}

static NVME: Mutex<Option<NvmeState>> = Mutex::new(None);
static AVAILABLE: AtomicBool = AtomicBool::new(false);

// The Dataset Management range of `discard_blocks` (one 4 KB page).
static DMA_BUF: Mutex<Option<DmaRegion>> = Mutex::new(None);

/// Pages one PRP list addresses (NVMe base spec 4.1.1: a 4 KB list holds
/// 512 entries). A command never needs a second list page because a
/// transfer is cut at `MAX_BLOCKS_PER_CMD`, which is at most this.
const PRP_LIST_ENTRIES: u32 = 512;

/// Largest single READ/WRITE in 4 KB blocks: MDTS, bounded by what one
/// PRP list page addresses. Set at init.
static MAX_BLOCKS_PER_CMD: AtomicU32 = AtomicU32::new(64);

/// Write submission entry `idx` of `sq` as one volatile store.
fn sq_write(sq: &DmaRegion, idx: u16, cmd: SqEntry) {
    sq.write_obj(idx as u64 * 64, cmd);
}

/// Read completion entry `idx` of `cq` as one volatile load.
fn cq_read(cq: &DmaRegion, idx: u16) -> CqEntry {
    cq.read_obj(idx as u64 * 16)
}

/// Ring doorbell for a submission queue.
fn ring_sq_doorbell(state: &NvmeState, qid: u16, tail: u16) {
    let offset = 0x1000 + (2 * qid as usize) * state.doorbell_stride as usize;
    state.regs.w32(offset, tail as u32);
}

/// Ring doorbell for a completion queue.
fn ring_cq_doorbell(state: &NvmeState, qid: u16, head: u16) {
    let offset = 0x1000 + (2 * qid as usize + 1) * state.doorbell_stride as usize;
    state.regs.w32(offset, head as u32);
}

/// Submit a command to the admin queue and wait for completion.
fn admin_command(state: &mut NvmeState, mut cmd: SqEntry) -> Result<CqEntry, BlkError> {
    cmd.command_id = state.command_id;
    state.command_id = state.command_id.wrapping_add(1);

    // Write to admin SQ
    sq_write(&state.admin_sq, state.admin_sq_tail, cmd);
    state.admin_sq_tail = (state.admin_sq_tail + 1) % ADMIN_QUEUE_SIZE;
    ring_sq_doorbell(state, 0, state.admin_sq_tail);

    // Poll completion
    for _ in 0..5_000_000u32 {
        let entry = cq_read(&state.admin_cq, state.admin_cq_head);
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

/// How long one I/O completion may take before the queue is given up,
/// as Linux's default `nvme_io_timeout`.
const IO_TIMEOUT_SECS: u64 = 30;
/// Longest single halt while waiting for a completion; short enough that
/// the core stays out of deep C-states, whose exit would delay the I/O.
const HALT_STEP_US: u64 = 100;

impl NvmeState {
    /// Queue `cmd` under the free id `cid` without ringing the doorbell.
    fn io_push(&mut self, cid: u16, mut cmd: SqEntry) {
        cmd.command_id = cid;
        self.io_busy[cid as usize] = true;
        sq_write(&self.io_sq, self.io_sq_tail, cmd);
        self.io_sq_tail = (self.io_sq_tail + 1) % self.io_q_depth;
        self.io_inflight += 1;
    }

    /// Hand everything queued so far to the controller.
    fn io_ring(&self) {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        ring_sq_doorbell(self, 1, self.io_sq_tail);
    }

    /// Take the next completion: `(command id, status code)`, its id free
    /// again. A timeout, or a completion for an id not in flight, closes
    /// the queue (see `io_dead`).
    fn io_reap(&mut self) -> Result<(u16, u16), BlkError> {
        let t0 = crate::interrupts::rdtsc();
        let deadline = t0.saturating_add(crate::interrupts::tsc_freq().saturating_mul(IO_TIMEOUT_SECS));
        let halt = self.msix_vector != 0 && MSIX_CONFIRMED.load(Ordering::Relaxed);
        if halt {
            // The completion's MSI-X must wake this core.
            crate::irq::route_to_current(self.msix_vector);
        }
        loop {
            if halt {
                self.halt_for_completion(deadline);
            }
            let entry = cq_read(&self.io_cq, self.io_cq_head);
            if ((entry.status & 1) != 0) == self.io_phase {
                self.io_cq_head = (self.io_cq_head + 1) % self.io_q_depth;
                if self.io_cq_head == 0 { self.io_phase = !self.io_phase; }
                ring_cq_doorbell(self, 1, self.io_cq_head);
                record_drain(t0, self.msix_vector);
                let cid = entry.command_id;
                if !self.io_busy.get(cid as usize).copied().unwrap_or(false) {
                    kprintln!("[npk] nvme: completion for id {}, which is not in flight", cid);
                    self.io_close();
                    return Err(BlkError::IoError);
                }
                self.io_busy[cid as usize] = false;
                self.io_free.push(cid);
                self.io_inflight -= 1;
                return Ok((cid, (entry.status >> 1) & 0x7FF));
            }
            if crate::interrupts::rdtsc() > deadline {
                kprintln!("[npk] nvme: no completion within {} s, I/O queue closed until reboot",
                    IO_TIMEOUT_SECS);
                self.io_close();
                return Err(BlkError::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    /// Give up the I/O queue. Commands still in flight may DMA into the
    /// callers' buffers at any time, and those buffers are freed once the
    /// call returns: bus mastering goes off first, so the controller
    /// reaches no memory any more.
    fn io_close(&mut self) {
        let cmd = pci::read32(self.pci_addr, 0x04);
        pci::write32(self.pci_addr, 0x04, cmd & !0x04);
        self.io_dead = true;
    }

    /// Halt this core until an interrupt (the completion's MSI-X), at most
    /// `HALT_STEP_US`, unless the next completion is already posted. The
    /// core sleeps instead of spinning and runs no other fiber meanwhile, so
    /// the caller keeps holding the controller's lock safely. With
    /// interrupts off it returns at once and the caller spins.
    fn halt_for_completion(&self, deadline: u64) {
        let rflags: u64;
        // SAFETY: reads RFLAGS, no other effect.
        unsafe { core::arch::asm!("pushfq; pop {}", out(reg) rflags, options(preserves_flags)) };
        if rflags & (1 << 9) == 0 {
            return;
        }
        // The look and the halt with IF off: a completion after the look
        // leaves its interrupt pending, and that ends the halt.
        crate::interrupts::without_interrupts(|| {
            let entry = cq_read(&self.io_cq, self.io_cq_head);
            if ((entry.status & 1) != 0) != self.io_phase {
                let step = crate::interrupts::rdtsc()
                    + crate::interrupts::tsc_freq() / (1_000_000 / HALT_STEP_US);
                crate::interrupts::halt_until(Some(deadline.min(step)), crate::smp::per_core::WAKE_HLT_FALLBACK);
            }
        });
    }

    /// Entry check of every I/O path: the queue is usable and empty.
    fn io_ready(&mut self) -> Result<(), BlkError> {
        if self.io_dead { return Err(BlkError::IoError); }
        while self.io_inflight > 0 {
            self.io_reap()?;
        }
        Ok(())
    }

    /// The PRP entries of a transfer of `len` bytes at virtual `addr`
    /// (dword aligned, at most `PRP_LIST_ENTRIES` pages past the first):
    /// `prp1`, and `prp2` as the second page or command `cid`'s list
    /// (NVMe base spec 4.1.1, Linux `nvme_pci_setup_prps`).
    fn map_prps(&self, cid: u16, addr: u64, len: usize) -> Result<(u64, u64), BlkError> {
        let first = 4096 - (addr & 0xFFF) as usize;
        let prp1 = phys(addr)?;
        if len <= first { return Ok((prp1, 0)); }
        let pages = (len - first).div_ceil(4096);
        let mut page = (addr & !0xFFF) + 4096;
        if pages == 1 { return Ok((prp1, phys(page)?)); }
        let list = &self.prp[cid as usize];
        for j in 0..pages {
            list.w64(j as u64 * 8, phys(page)?);
            page += 4096;
        }
        Ok((prp1, list.phys()))
    }

    /// Run `segs` as `opcode` commands with `flags` in CDW12, straight
    /// to and from the callers' memory. As many commands are outstanding
    /// as the queue holds, and each completion lets the next one in. After
    /// an error nothing new is submitted; the call returns once every
    /// submitted command has completed, with the first error.
    fn transfer(&mut self, opcode: u8, flags: u32, segs: &[Seg]) -> Result<(), BlkError> {
        self.io_ready()?;
        for s in segs {
            if s.addr & 3 != 0 || s.len % SECTOR_SIZE != 0 || s.lba.checked_add((s.len / SECTOR_SIZE) as u64)
                .is_none_or(|end| end > self.total_lbas)
            {
                kprintln!("[npk] nvme: refused segment lba {} at {:#x}, {} B", s.lba, s.addr, s.len);
                return Err(BlkError::OutOfRange);
            }
        }
        let max = MAX_BLOCKS_PER_CMD.load(Ordering::Relaxed) as usize * BLOCK_SIZE;
        let (mut si, mut done) = (0usize, 0usize);
        let mut err = None;
        loop {
            let mut queued = false;
            while si < segs.len() && err.is_none() {
                let Some(cid) = self.io_free.pop() else { break };
                let s = segs[si];
                let n = (s.len - done).min(max);
                let (prp1, prp2) = match self.map_prps(cid, s.addr + done as u64, n) {
                    Ok(p) => p,
                    Err(e) => {
                        kprintln!("[npk] nvme: no physical address for {:#x} (+{} B)", s.addr + done as u64, n);
                        self.io_free.push(cid);
                        err = Some(e);
                        break;
                    }
                };
                let lba = s.lba + (done / SECTOR_SIZE) as u64;
                let mut cmd = SqEntry::zeroed();
                cmd.opcode = opcode;
                cmd.nsid = 1;
                cmd.prp1 = prp1;
                cmd.prp2 = prp2;
                cmd.cdw10 = lba as u32;
                cmd.cdw11 = (lba >> 32) as u32;
                cmd.cdw12 = (n / SECTOR_SIZE - 1) as u32 | flags;
                self.io_push(cid, cmd);
                queued = true;
                done += n;
                if done == s.len { si += 1; done = 0; }
            }
            if queued { self.io_ring(); }
            if self.io_inflight == 0 { break; }
            let (cid, status) = self.io_reap()?;
            if status != 0 && err.is_none() {
                kprintln!("[npk] nvme: opcode {:#04x} (id {}) failed, status {:#05x}", opcode, cid, status);
                err = Some(BlkError::IoError);
            }
        }
        // The controller's writes into the buffers before the caller's reads.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        nvme_msix_confirm(self.msix_vector);
        err.map_or(Ok(()), Err)
    }
}

/// One stretch of a transfer: `len` bytes at virtual `addr`, from LBA `lba`.
#[derive(Clone, Copy)]
struct Seg {
    lba: u64,
    addr: u64,
    len: usize,
}

/// The physical address of kernel virtual `addr`: itself in the identity
/// map, where the heap lives (Linux `virt_to_phys`), else from the page
/// tables (the kernel stacks).
fn phys(addr: u64) -> Result<u64, BlkError> {
    if addr < crate::mm::paging::IDENTITY_END { return Ok(addr); }
    crate::mm::paging::translate(addr).ok_or(BlkError::OutOfRange)
}

/// Submit a data-less command (flush, or one whose data is in `DMA_BUF`)
/// and wait for it.
fn io_command(state: &mut NvmeState, cmd: SqEntry) -> Result<(), BlkError> {
    state.io_ready()?;
    let cid = state.io_free.pop().ok_or(BlkError::IoError)?;
    state.io_push(cid, cmd);
    state.io_ring();
    let (_, status) = state.io_reap()?;
    if status != 0 { return Err(BlkError::IoError); }
    nvme_msix_confirm(state.msix_vector);
    Ok(())
}

/// One-shot confirmation that NVMe completions raise our MSI-X ISR, without
/// changing the poll-based completion logic.
static MSIX_CONFIRMED: AtomicBool = AtomicBool::new(false);
fn nvme_msix_confirm(vector: crate::irq::Irq) {
    if vector != 0 && !MSIX_CONFIRMED.load(Ordering::Relaxed) {
        let n = crate::irq::fired_count(vector);
        if n > 0 {
            MSIX_CONFIRMED.store(true, Ordering::Relaxed);
            crate::kdebug!("[npk] nvme: MSI-X completion IRQ CONFIRMED (vector {:#04x}, {} fires) — host-device-irq works",
                vector, n);
        }
    }
}

/// Initialize the NVMe controller.
pub fn init() -> bool {
    // Find NVMe device by class code (01h:08h). The absence is logged: on a
    // machine without a disk it is the only clue there is.
    let dev = match pci::find_by_class(NVME_CLASS, NVME_SUBCLASS) {
        Some(d) => d,
        None => {
            kprintln!("[npk] nvme: no device of class 01:08 on the PCI bus");
            return false;
        }
    };

    crate::kdebug!("[npk] nvme: PCI {:02x}:{:02x}.{} [{:04x}:{:04x}] IRQ {}",
        dev.addr.bus, dev.addr.device, dev.addr.function,
        dev.vendor_id, dev.device_id, dev.irq_line);
    // npkFS lives on this controller; no module may bind it.
    let _ = pci::claim(dev.addr, pci::Owner::Kernel("nvme"));

    // Enable bus mastering for DMA
    pci::enable_bus_master(dev.addr);

    // Read 64-bit BAR0
    let bar0_phys = pci::read_bar64(dev.addr, 0x10);
    if bar0_phys == 0 {
        kprintln!("[npk] nvme: BAR0 is zero, cannot initialize");
        return false;
    }

    // NVMe registers are typically 16KB-64KB; 64KB covers registers + doorbells.
    let map_size = 64 * 1024u64;
    // SAFETY: BAR0 of the NVMe controller this driver owns: its register
    // and doorbell window, not RAM.
    let regs = match unsafe { Mmio::map(bar0_phys, map_size) } {
        Ok(m) => m,
        Err(e) => {
            kprintln!("[npk] nvme: failed to map BAR0 at {:#x}: {:?}", bar0_phys, e);
            return false;
        }
    };

    // Read capabilities
    let cap = regs.r64_lo_hi(REG_CAP);
    let doorbell_stride = 4u32 << ((cap >> 32) & 0xF) as u32; // DSTRD field
    // CAP.MQES is zero-based: the maximum 0xFFFF means 65536 entries, which
    // overflows u16 (e.g. KIOXIA [1e0f:000c] reports it). Hence u32.
    let max_queue_entries = (cap & 0xFFFF) as u32 + 1;
    let version = regs.r32(REG_VS);

    // Log raw CAP too: a computed 0 could come from an overflow or from a
    // register that does not answer.
    crate::kdebug!("[npk] nvme: version {}.{}.{}, CAP={:#018x}, max queue {}",
        version >> 16, (version >> 8) & 0xFF, version & 0xFF,
        cap, max_queue_entries);

    if max_queue_entries < IO_QUEUE_MIN {
        kprintln!("[npk] nvme: CAP.MQES+1={} below the spec minimum, refusing init",
            max_queue_entries);
        return false;
    }
    let io_q_depth = max_queue_entries.min(IO_QUEUE_DEPTH) as u16;

    // Disable controller
    let cc = regs.r32(REG_CC);
    if cc & CC_EN != 0 {
        regs.w32(REG_CC, cc & !CC_EN);
        // Wait for not ready
        for _ in 0..1_000_000u32 {
            if regs.r32(REG_CSTS) & CSTS_RDY == 0 { break; }
            core::hint::spin_loop();
        }
    }

    // Allocate Admin Submission Queue (ADMIN_QUEUE_SIZE * 64 bytes)
    let admin_sq_pages = ((ADMIN_QUEUE_SIZE as usize * 64) + 4095) / 4096;
    let admin_sq = match DmaRegion::alloc_zeroed(admin_sq_pages) {
        Some(r) => r,
        None => { kprintln!("[npk] nvme: failed to allocate admin SQ"); return false; }
    };

    // Allocate Admin Completion Queue (ADMIN_QUEUE_SIZE * 16 bytes)
    let admin_cq_pages = ((ADMIN_QUEUE_SIZE as usize * 16) + 4095) / 4096;
    let admin_cq = match DmaRegion::alloc_zeroed(admin_cq_pages) {
        Some(r) => r,
        None => { kprintln!("[npk] nvme: failed to allocate admin CQ"); return false; }
    };

    // Configure admin queues
    let aqa = ((ADMIN_QUEUE_SIZE as u32 - 1) << 16) | (ADMIN_QUEUE_SIZE as u32 - 1);
    regs.w32(REG_AQA, aqa);
    regs.w64_lo_hi(REG_ASQ, admin_sq.phys());
    regs.w64_lo_hi(REG_ACQ, admin_cq.phys());

    // Mask all interrupts (we poll)
    regs.w32(REG_INTMS, 0xFFFF_FFFF);

    // Enable controller
    let cc_val = CC_EN | CC_CSS_NVM | CC_MPS_4K | CC_IOSQES | CC_IOCQES;
    regs.w32(REG_CC, cc_val);

    // Wait for ready
    for _ in 0..5_000_000u32 {
        if regs.r32(REG_CSTS) & CSTS_RDY != 0 { break; }
        core::hint::spin_loop();
    }
    if regs.r32(REG_CSTS) & CSTS_RDY == 0 {
        kprintln!("[npk] nvme: controller did not become ready");
        return false;
    }

    let mut state = NvmeState {
        regs,
        doorbell_stride: doorbell_stride,
        admin_sq, admin_cq,
        admin_sq_tail: 0, admin_cq_head: 0, admin_phase: true,
        io_sq: DmaRegion::empty(), io_cq: DmaRegion::empty(),
        io_sq_tail: 0, io_cq_head: 0, io_phase: true, io_q_depth,
        total_lbas: 0,
        model: [0; 40],
        serial: [0; 20],
        oncs: 0,
        command_id: 1,
        io_inflight: 0,
        io_free: Vec::new(),
        io_busy: Vec::new(),
        prp: Vec::new(),
        io_dead: false,
        msix_vector: 0,
        host_mem: false,
        pci_addr: dev.addr,
    };

    // Allocate DMA buffer for Identify data (4KB)
    let identify_buf = match DmaRegion::alloc_zeroed(1) {
        Some(r) => r,
        None => { kprintln!("[npk] nvme: failed to allocate identify buf"); return false; }
    };

    // Identify Controller (CNS=1)
    let mut cmd = SqEntry::zeroed();
    cmd.opcode = ADM_IDENTIFY;
    cmd.nsid = 0;
    cmd.prp1 = identify_buf.phys();
    cmd.cdw10 = 1; // CNS=1: Identify Controller
    if admin_command(&mut state, cmd).is_err() {
        kprintln!("[npk] nvme: Identify Controller failed");
        return false;
    }

    // Parse controller info
    // Serial number: bytes 4-23
    identify_buf.copy_out(4u64, &mut state.serial);
    // Model number: bytes 24-63
    identify_buf.copy_out(24u64, &mut state.model);

    // ONCS (Optional NVM Command Support) at byte 520 (2 bytes); bit 2 =
    // Dataset Management (TRIM/Deallocate). Not byte 256: that is OACS
    // (Optional Admin Command Support).
    state.oncs = identify_buf.r16(520u64);
    // Host memory buffer wishes (HMPRE, HMMIN, HMMINDS in 4 KiB units,
    // HMMAXD), for `setup_host_mem` once the namespace is known.
    let hmb = HostMemAsk {
        preferred: identify_buf.r32(272u64) as u64 * 4096,
        min: identify_buf.r32(276u64) as u64 * 4096,
        min_chunk: identify_buf.r32(332u64) as u64 * 4096,
        max_descs: identify_buf.r16(336u64) as usize,
    };
    // VWC (Volatile Write Cache) at byte 525; bit 0 = a volatile write cache is
    // present. When set, completed writes may sit in controller DRAM until an
    // NVM Flush — which is exactly why the npkFS commit issues a flush barrier
    // before the superblock. Logged so the property is visible.
    let vwc_present = identify_buf.r8(525u64) & 1 != 0;

    // MDTS (Maximum Data Transfer Size) at offset 77, 1 byte. Units of
    // 2^(12 + CAP.MPSMIN) bytes; for MPSMIN=0 (~all NVMe SSDs) that's
    // pages of 4 KB. 0 means "no limit"; one PRP list page bounds it then.
    let mdts: u8 = identify_buf.r8(77u64);
    let mpsmin = ((cap >> 48) & 0xF) as u32;
    let hw_max_blocks = if mdts == 0 {
        PRP_LIST_ENTRIES
    } else {
        let pages = 1u32.checked_shl(mdts as u32).unwrap_or(u32::MAX);
        let bytes_per_page = 1u32 << (12 + mpsmin);
        let blocks = pages.saturating_mul(bytes_per_page) / BLOCK_SIZE as u32;
        blocks.max(1)
    };
    let max_blocks_per_cmd = hw_max_blocks.min(PRP_LIST_ENTRIES);
    MAX_BLOCKS_PER_CMD.store(max_blocks_per_cmd, Ordering::Relaxed);

    let model_str = core::str::from_utf8(&state.model).unwrap_or("?").trim();
    let serial_str = core::str::from_utf8(&state.serial).unwrap_or("?").trim();
    let has_trim = state.oncs & (1 << 2) != 0;
    kprintln!("[npk] nvme: {} (SN: {}), TRIM={}, VWC={}, MDTS={} ({} KB/cmd)",
        model_str, serial_str,
        if has_trim { "yes" } else { "no" },
        if vwc_present { "yes" } else { "no" },
        mdts, max_blocks_per_cmd * 4);

    // Identify Namespace 1 (CNS=0, NSID=1)
    identify_buf.zero();
    let mut cmd = SqEntry::zeroed();
    cmd.opcode = ADM_IDENTIFY;
    cmd.nsid = 1;
    cmd.prp1 = identify_buf.phys();
    cmd.cdw10 = 0; // CNS=0: Identify Namespace
    if admin_command(&mut state, cmd).is_err() {
        kprintln!("[npk] nvme: Identify Namespace failed");
        return false;
    }

    // NSZE (Namespace Size) at offset 0 (8 bytes, little-endian)
    let nsze = identify_buf.r64(0u64);
    state.total_lbas = nsze;

    // The LBA format in use: FLBAS (byte 26) indexes LBAF[] at byte 128,
    // four bytes each, metadata size in bits 15:0 and LBADS (log2 of the
    // data size) in bits 23:16. Linux `nvme_update_ns_info`. Every command
    // here counts 512-byte LBAs; with any other format a 4 KB block would
    // be eight LBAs of 4 KB, DMA'd into a 4 KB buffer.
    let flbas = identify_buf.r8(26u64);
    let idx = ((flbas & 0x0F) | ((flbas >> 1) & 0x30)) as usize;
    let lbaf = identify_buf.r32(128 + idx * 4);
    let lbads = (lbaf >> 16) & 0xFF;
    let metadata = lbaf & 0xFFFF;
    if lbads != 9 || metadata != 0 {
        kprintln!("[npk] nvme: namespace format {} has {}-byte LBAs with {} bytes of metadata; only 512 without metadata is supported",
            flbas & 0x0F, 1u64 << lbads.min(63), metadata);
        return false;
    }

    let size_mb = (nsze * 512) / (1024 * 1024);
    let size_gb = size_mb / 1024;
    if size_gb > 0 {
        kprintln!("[npk] nvme: {} GB ({} sectors)", size_gb, nsze);
    } else {
        kprintln!("[npk] nvme: {} MB ({} sectors)", size_mb, nsze);
    }

    setup_host_mem(&mut state, hmb);

    // Create I/O Completion Queue (QID=1)
    let io_cq_pages = ((io_q_depth as usize * 16) + 4095) / 4096;
    let io_cq = match DmaRegion::alloc_zeroed(io_cq_pages) {
        Some(r) => r,
        None => { kprintln!("[npk] nvme: failed to allocate I/O CQ"); return false; }
    };

    // Set up MSI-X completion interrupts for the I/O CQ. Allocate a LAPIC
    // vector + program the device's MSI-X table entry 0 to deliver to this
    // (BSP) core. MSI-X writes go straight to the LAPIC (no PIC/IOAPIC), so
    // the masked-PIC setup is irrelevant. If the device has no usable MSI-X,
    // the vector stays 0 and the CQ is created without interrupts (IEN=0),
    // i.e. pure poll.
    state.msix_vector = crate::irq::register(dev.addr, 0).unwrap_or(0);
    let ien = if state.msix_vector != 0 { 1u32 << 1 } else { 0 }; // cdw11 bit1 = IEN
    if state.msix_vector != 0 {
        crate::kdebug!("[npk] nvme: MSI-X I/O-completion IRQ → LAPIC vector {:#04x} (IV=0)",
            state.msix_vector);
    } else {
        kprintln!("[npk] nvme: no usable MSI-X — polling completions");
    }

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = ADM_CREATE_IO_CQ;
    cmd.prp1 = io_cq.phys();
    cmd.cdw10 = ((io_q_depth as u32 - 1) << 16) | 1; // QID=1, size
    cmd.cdw11 = 1 | ien; // bit0 = Physically Contiguous, bit1 = IEN; IV=0
    if admin_command(&mut state, cmd).is_err() {
        kprintln!("[npk] nvme: Create I/O CQ failed");
        return false;
    }

    // Create I/O Submission Queue (QID=1, CQ=1)
    let io_sq_pages = ((io_q_depth as usize * 64) + 4095) / 4096;
    let io_sq = match DmaRegion::alloc_zeroed(io_sq_pages) {
        Some(r) => r,
        None => { kprintln!("[npk] nvme: failed to allocate I/O SQ"); return false; }
    };

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = ADM_CREATE_IO_SQ;
    cmd.prp1 = io_sq.phys();
    cmd.cdw10 = ((io_q_depth as u32 - 1) << 16) | 1; // QID=1, size
    cmd.cdw11 = (1 << 16) | 1; // CQID=1, Physically contiguous
    if admin_command(&mut state, cmd).is_err() {
        kprintln!("[npk] nvme: Create I/O SQ failed");
        return false;
    }

    // Disable interrupt coalescing (Set Features FID 0x08, value 0) so the
    // controller raises an MSI-X per completion instead of aggregating by
    // count/time. Our io_command poll-drains the CQ immediately; if the
    // controller defaults to time-based coalescing, the held interrupt
    // condition clears before the timer fires → no MSI ever. (Linux waits on
    // the IRQ rather than poll-draining, so it never hits this.) Best-effort —
    // ignore errors; controllers that lack the feature still work via poll.
    if state.msix_vector != 0 {
        let mut cmd = SqEntry::zeroed();
        cmd.opcode = ADM_SET_FEATURES;
        cmd.cdw10 = 0x08; // FID = Interrupt Coalescing
        cmd.cdw11 = 0; // aggregation threshold = 0, time = 0 → no coalescing
        match admin_command(&mut state, cmd) {
            Ok(_) => kprintln!("[npk] nvme: interrupt coalescing disabled"),
            Err(_) => crate::kdebug!("[npk] nvme: set-features (coalescing) not supported"),
        }
        // INTMS was set to 0xFFFFFFFF above ("mask all, we poll"). The spec
        // says MSI-X uses the per-vector table mask and ignores INTMS, but
        // some Intel controllers honor it anyway and gate the MSI. Clear it
        // (INTMC) now that MSI-X is configured + unmasked. Harmless on a
        // spec-compliant controller (write ignored under MSI-X).
        state.regs.w32(REG_INTMC, 0xFFFF_FFFF);
        crate::kdebug!("[npk] nvme: INTMS cleared (INTMC) for MSI-X");
    }

    state.io_sq = io_sq;
    state.io_cq = io_cq;

    // The DSM range page.
    let dma = match DmaRegion::alloc_zeroed(1) {
        Some(r) => r,
        None => { kprintln!("[npk] nvme: failed to allocate DMA buf"); return false; }
    };
    *DMA_BUF.lock() = Some(dma);

    // One command id per entry the queue can hold outstanding, each with
    // its PRP list page.
    let ids = io_q_depth - 1;
    for cid in 0..ids {
        let Some(page) = DmaRegion::alloc_zeroed(1) else {
            kprintln!("[npk] nvme: no memory for the PRP list of command {}", cid);
            return false;
        };
        state.prp.push(page);
    }
    state.io_busy = alloc::vec![false; ids as usize];
    state.io_free = (0..ids).rev().collect();

    // Free identify buffer
    memory::deallocate_frame(identify_buf.phys());

    kprintln!("[npk] nvme: online");
    AVAILABLE.store(true, Ordering::Relaxed);
    *NVME.lock() = Some(state);
    true
}

/// Linux `max_host_mem_size_mb`: the most host memory a controller gets.
const MAX_HOST_MEM: u64 = 128 << 20;
/// Largest piece of the host memory buffer tried first (Linux: the largest
/// page-allocator block, 4 MiB on x86).
const HOST_MEM_CHUNK: u64 = 4 << 20;
const ADM_FEAT_HOST_MEM: u32 = 0x0d;

/// What Identify Controller asks for a host memory buffer.
struct HostMemAsk {
    preferred: u64,
    min: u64,
    min_chunk: u64,
    max_descs: usize,
}

/// Hand the controller the host memory it asks for (Linux
/// `nvme_setup_host_mem`): DRAM-less SSDs keep their mapping tables there
/// and are slow without it. Pieces of `HOST_MEM_CHUNK` down to the
/// controller's smallest, listed in a descriptor table, enabled with Set
/// Features 0x0d. The memory stays the controller's until `shutdown`.
fn setup_host_mem(state: &mut NvmeState, ask: HostMemAsk) {
    if ask.preferred == 0 { return; }
    let preferred = ask.preferred.min(MAX_HOST_MEM);
    if ask.min > preferred {
        kprintln!("[npk] nvme: host memory buffer needs {} MB, more than {} MB; not set up",
            ask.min >> 20, MAX_HOST_MEM >> 20);
        return;
    }
    let smallest = ask.min_chunk.max(2 * 4096);
    let mut chunk = HOST_MEM_CHUNK.min(preferred);
    while chunk >= smallest {
        let mut entries = preferred.div_ceil(chunk) as usize;
        if ask.max_descs != 0 { entries = entries.min(ask.max_descs); }
        let Some(descs) = DmaRegion::alloc_zeroed((entries * 16).div_ceil(4096)) else { return };
        let mut pieces: Vec<(u64, u64)> = Vec::new();
        let mut size = 0u64;
        while size < preferred && pieces.len() < entries {
            let len = chunk.min(preferred - size);
            let Some(piece) = DmaRegion::alloc_zeroed((len / 4096) as usize) else { break };
            descs.w64(pieces.len() as u64 * 16, piece.phys());
            descs.w32(pieces.len() as u64 * 16 + 8, (len / 4096) as u32);
            pieces.push((piece.phys(), len / 4096));
            size += len;
        }
        if size > 0 && size >= ask.min {
            let mut cmd = SqEntry::zeroed();
            cmd.opcode = ADM_SET_FEATURES;
            cmd.cdw10 = ADM_FEAT_HOST_MEM;
            cmd.cdw11 = 1; // EHM: enable host memory
            cmd.cdw12 = (size / 4096) as u32;
            cmd.cdw13 = descs.phys() as u32;
            cmd.cdw14 = (descs.phys() >> 32) as u32;
            cmd.cdw15 = pieces.len() as u32;
            if admin_command(state, cmd).is_ok() {
                state.host_mem = true;
                kprintln!("[npk] nvme: host memory buffer {} MB in {} piece(s)", size >> 20, pieces.len());
                return;
            }
            kprintln!("[npk] nvme: the controller refused its host memory buffer");
        }
        for (phys, pages) in pieces {
            memory::deallocate_contiguous(phys, pages as usize);
        }
        memory::deallocate_contiguous(descs.phys(), (entries * 16).div_ceil(4096));
        if size > 0 && size >= ask.min { return; }
        chunk /= 2;
    }
}

/// Before power-off or reset (Linux `nvme_dev_disable` for shutdown): the
/// controller gives the host memory back, then shuts down normally
/// (CC.SHN), which puts its volatile write cache on media. No I/O after.
pub fn shutdown() {
    let mut nvme = NVME.lock();
    let Some(state) = nvme.as_mut() else { return };
    let _ = state.io_ready();
    AVAILABLE.store(false, Ordering::Relaxed);
    if state.host_mem {
        let mut cmd = SqEntry::zeroed();
        cmd.opcode = ADM_SET_FEATURES;
        cmd.cdw10 = ADM_FEAT_HOST_MEM;
        let _ = admin_command(state, cmd);
        state.host_mem = false;
    }
    let cc = state.regs.r32(REG_CC);
    state.regs.w32(REG_CC, (cc & !CC_SHN_MASK) | CC_SHN_NORMAL);
    let deadline = crate::interrupts::rdtsc()
        .saturating_add(crate::interrupts::tsc_freq().saturating_mul(SHUTDOWN_TIMEOUT_SECS));
    while state.regs.r32(REG_CSTS) & CSTS_SHST_MASK != CSTS_SHST_DONE {
        if crate::interrupts::rdtsc() > deadline {
            kprintln!("[npk] nvme: shutdown not complete within {} s", SHUTDOWN_TIMEOUT_SECS);
            return;
        }
        core::hint::spin_loop();
    }
}

/// CC.SHN (bits 15:14) and CSTS.SHST (bits 3:2).
const CC_SHN_MASK: u32 = 3 << 14;
const CC_SHN_NORMAL: u32 = 1 << 14;
const CSTS_SHST_MASK: u32 = 3 << 2;
const CSTS_SHST_DONE: u32 = 2 << 2;
/// Linux `shutdown_timeout`.
const SHUTDOWN_TIMEOUT_SECS: u64 = 5;

pub fn is_available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

pub fn capacity() -> Option<u64> {
    NVME.lock().as_ref().map(|s| s.total_lbas)
}

pub fn block_count() -> Option<u64> {
    capacity().map(|sectors| sectors / (BLOCK_SIZE / SECTOR_SIZE) as u64)
}

const SECTORS_PER_BLOCK: u64 = (BLOCK_SIZE / SECTOR_SIZE) as u64;

/// The first LBA of 4 KB block `block`.
fn block_lba(block: u64) -> Result<u64, BlkError> {
    block.checked_mul(SECTORS_PER_BLOCK).ok_or(BlkError::OutOfRange)
}

/// `stretches` (first LBA, bytes) laid end to end in a buffer at `base`,
/// neighbours on the disk merged into one segment.
fn segs_at(stretches: &[(u64, usize)], base: u64) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::with_capacity(stretches.len());
    let mut addr = base;
    for &(lba, len) in stretches {
        match segs.last_mut() {
            Some(s) if s.lba + (s.len / SECTOR_SIZE) as u64 == lba => s.len += len,
            _ => segs.push(Seg { lba, addr, len }),
        }
        addr += len as u64;
    }
    segs
}

/// A dword-aligned scratch buffer of `len` bytes, for callers whose own
/// buffer the controller cannot address.
fn with_aligned<R>(len: usize, f: impl FnOnce(&mut [u8]) -> R) -> R {
    let mut tmp = alloc::vec![0u8; len + 3];
    let off = tmp.as_ptr().align_offset(4);
    f(&mut tmp[off..off + len])
}

fn total(stretches: &[(u64, usize)]) -> usize {
    stretches.iter().map(|s| s.1).sum()
}

/// Read `stretches` (first LBA, bytes) into `out`, one after the other.
fn read_lbas(stretches: &[(u64, usize)], out: &mut [u8]) -> Result<(), BlkError> {
    if total(stretches) != out.len() { return Err(BlkError::OutOfRange); }
    if out.as_ptr() as usize & 3 != 0 {
        return with_aligned(out.len(), |tmp| {
            read_lbas(stretches, tmp)?;
            out.copy_from_slice(tmp);
            Ok(())
        });
    }
    let segs = segs_at(stretches, out.as_mut_ptr() as u64);
    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;
    state.transfer(NVM_READ, 0, &segs)
}

/// Write `input` to `stretches` (first LBA, bytes), one after the other.
fn write_lbas(stretches: &[(u64, usize)], input: &[u8], flags: u32) -> Result<(), BlkError> {
    if total(stretches) != input.len() { return Err(BlkError::OutOfRange); }
    if input.as_ptr() as usize & 3 != 0 {
        return with_aligned(input.len(), |tmp| {
            tmp.copy_from_slice(input);
            write_lbas(stretches, tmp, flags)
        });
    }
    let segs = segs_at(stretches, input.as_ptr() as u64);
    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;
    state.transfer(NVM_WRITE, flags, &segs)
}

/// Force Unit Access in CDW12: on stable media before the completion.
const CDW12_FUA: u32 = 1 << 30;

/// Read a 512-byte sector.
pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    read_lbas(&[(sector, SECTOR_SIZE)], buf)
}

/// Write a 512-byte sector.
pub fn write_sector(sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    write_lbas(&[(sector, SECTOR_SIZE)], buf, 0)
}

/// Read a 4KB block (8 sectors).
pub fn read_block(block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    read_lbas(&[(block_lba(block)?, BLOCK_SIZE)], buf)
}

/// Write a 4KB block (8 sectors).
pub fn write_block(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    write_lbas(&[(block_lba(block)?, BLOCK_SIZE)], buf, 0)
}

/// Like `write_block` but with Force Unit Access: the controller must place the
/// data on stable media before completing, regardless of its volatile write
/// cache. Used for the npkFS superblock so it is durable the instant the commit
/// returns, without a separate full-cache FLUSH after it.
pub fn write_block_fua(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    write_lbas(&[(block_lba(block)?, BLOCK_SIZE)], buf, CDW12_FUA)
}

/// Write each `(block, data)`, all of them in flight together. Ok only if
/// every write completed without error.
pub fn write_blocks_batch(items: &[(u64, &[u8; BLOCK_SIZE])]) -> Result<(), BlkError> {
    if items.is_empty() { return Ok(()); }
    if items.iter().all(|(_, b)| b.as_ptr() as usize & 3 == 0) {
        let mut segs: Vec<Seg> = Vec::with_capacity(items.len());
        for &(block, buf) in items {
            let (lba, addr) = (block_lba(block)?, buf.as_ptr() as u64);
            match segs.last_mut() {
                Some(s) if s.lba + (s.len / SECTOR_SIZE) as u64 == lba && s.addr + s.len as u64 == addr =>
                    s.len += BLOCK_SIZE,
                _ => segs.push(Seg { lba, addr, len: BLOCK_SIZE }),
            }
        }
        let mut nvme = NVME.lock();
        let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;
        return state.transfer(NVM_WRITE, 0, &segs);
    }
    let stretches = items.iter().map(|&(b, _)| Ok((block_lba(b)?, BLOCK_SIZE)))
        .collect::<Result<Vec<_>, BlkError>>()?;
    with_aligned(items.len() * BLOCK_SIZE, |tmp| {
        for (i, (_, buf)) in items.iter().enumerate() {
            tmp[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE].copy_from_slice(&buf[..]);
        }
        write_lbas(&stretches, tmp, 0)
    })
}

/// Read `blocks` into `output`, block i at `output[i * BLOCK_SIZE..]`, all
/// of them in flight together.
pub fn read_blocks_batch(blocks: &[u64], output: &mut [u8]) -> Result<(), BlkError> {
    let stretches = blocks.iter().map(|&b| Ok((block_lba(b)?, BLOCK_SIZE)))
        .collect::<Result<Vec<_>, BlkError>>()?;
    read_lbas(&stretches, output)
}

/// Read `count` 4 KB blocks starting at `start_block` into `output`
/// (`count * BLOCK_SIZE` bytes).
pub fn read_extent(start_block: u64, count: u64, output: &mut [u8]) -> Result<(), BlkError> {
    if count == 0 { return Ok(()); }
    let len = usize::try_from(count).ok().and_then(|c| c.checked_mul(BLOCK_SIZE)).ok_or(BlkError::OutOfRange)?;
    read_lbas(&[(block_lba(start_block)?, len)], output)
}

/// Write `count` 4 KB blocks of `input` starting at `start_block`.
pub fn write_extent(start_block: u64, count: u64, input: &[u8]) -> Result<(), BlkError> {
    if count == 0 { return Ok(()); }
    let len = usize::try_from(count).ok().and_then(|c| c.checked_mul(BLOCK_SIZE)).ok_or(BlkError::OutOfRange)?;
    write_lbas(&[(block_lba(start_block)?, len)], input, 0)
}

/// Read a list of (start_block, count_blocks) extents into `output`, one
/// after the other, all of them in flight together.
pub fn read_multi_extent(extents: &[(u64, u64)], output: &mut [u8]) -> Result<(), BlkError> {
    let stretches = extents.iter().map(|&(start, count)| {
        let len = usize::try_from(count).ok().and_then(|c| c.checked_mul(BLOCK_SIZE)).ok_or(BlkError::OutOfRange)?;
        Ok((block_lba(start)?, len))
    }).collect::<Result<Vec<_>, BlkError>>()?;
    read_lbas(&stretches, output)
}

// [nvme-spin] rolling spin-duration stats over the shared completion drain.
static NVME_DRAIN_COUNT: AtomicU64 = AtomicU64::new(0);
static NVME_DRAIN_TSC_SUM: AtomicU64 = AtomicU64::new(0);
static NVME_DRAIN_TSC_MAX: AtomicU64 = AtomicU64::new(0);
static NVME_DRAIN_SLOW: AtomicU64 = AtomicU64::new(0);
const NVME_DRAIN_LOG_EVERY: u64 = 512;
const NVME_SLOW_US: u64 = 200; // a completion that spun > this is "slow"

fn record_drain(t0: u64, vector: crate::irq::Irq) {
    let dt = crate::interrupts::rdtsc().saturating_sub(t0);
    let mhz = (crate::interrupts::tsc_freq() / 1_000_000).max(1);
    let us = dt / mhz;
    NVME_DRAIN_TSC_SUM.fetch_add(dt, Ordering::Relaxed);
    NVME_DRAIN_TSC_MAX.fetch_max(dt, Ordering::Relaxed);
    if us > NVME_SLOW_US { NVME_DRAIN_SLOW.fetch_add(1, Ordering::Relaxed); }
    let n = NVME_DRAIN_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if n % NVME_DRAIN_LOG_EVERY == 0 {
        let sum = NVME_DRAIN_TSC_SUM.swap(0, Ordering::Relaxed);
        let max = NVME_DRAIN_TSC_MAX.swap(0, Ordering::Relaxed);
        let slow = NVME_DRAIN_SLOW.swap(0, Ordering::Relaxed);
        let avg_us = (sum / NVME_DRAIN_LOG_EVERY) / mhz;
        let max_us = max / mhz;
        let fires = crate::irq::fired_count(vector);
        if crate::shade::PERF_LOG {
        kprintln!(
            "[nvme-spin] {} drains: avg {}us max {}us slow(>{}us) {} | msix vec {:#04x} fires {}",
            NVME_DRAIN_LOG_EVERY, avg_us, max_us, NVME_SLOW_US, slow, vector, fires,
        );
        }
    }
}

/// Force the controller's volatile write cache to stable media (NVM Flush,
/// opcode 0x00, namespace-wide). Without this, completed writes may linger in
/// controller DRAM and be lost or reordered on power-loss — which would break
/// the npkFS commit ordering (superblock durable only after the data it points
/// at). This is the durability barrier the 4-phase commit relies on.
pub fn flush() -> Result<(), BlkError> {
    let mut nvme = NVME.lock();
    let state = nvme.as_mut().ok_or(BlkError::NotInitialized)?;
    let mut cmd = SqEntry::zeroed();
    cmd.opcode = NVM_FLUSH;
    cmd.nsid = 1;
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

    dma.fill(0u64, 0, 16);
    // Length in LBAs at offset 4
    dma.copy_in(4u64, &(lba_count as u32).to_le_bytes());
    // Starting LBA at offset 8
    dma.copy_in(8u64, &start_lba.to_le_bytes());

    let mut cmd = SqEntry::zeroed();
    cmd.opcode = NVM_DSM;
    cmd.nsid = 1;
    cmd.prp1 = dma.phys();
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

/// Return serial number for display.
pub fn serial_number() -> Option<alloc::string::String> {
    let nvme = NVME.lock();
    let state = nvme.as_ref()?;
    let s = core::str::from_utf8(&state.serial).unwrap_or("?").trim();
    Some(alloc::string::String::from(s))
}

/// Maximum data per single READ/WRITE cmd in 4 KB blocks: MDTS, at most
/// what one PRP list page addresses.
pub fn max_blocks_per_cmd() -> u32 {
    MAX_BLOCKS_PER_CMD.load(Ordering::Relaxed)
}

/// (interrupt number, total completion-IRQ fires) for the I/O-CQ MSI-X, or None
/// if MSI-X isn't set up (poll-only). For the `disk` diagnostic — `fires > 0`
/// proves the device-IRQ path works.
pub fn msix_status() -> Option<(crate::irq::Irq, u64)> {
    let guard = NVME.lock();
    let v = guard.as_ref()?.msix_vector;
    if v == 0 {
        return None;
    }
    Some((v, crate::irq::fired_count(v)))
}

/// Live MSI-X capability + table-entry read-back for the `disk` diagnostic.
/// Shows whether our programming stuck (entry unmasked, MSI-X enabled, addr/
/// data correct), to tell "programmed but no IRQ" apart without the boot log.
pub fn msix_debug() -> Option<crate::pci::MsixDebug> {
    let dev = NVME.lock().as_ref().map(|s| s.pci_addr)?;
    crate::pci::msix_debug(dev, 0)
}

#[allow(dead_code)]
/// Return capacity in GB.
pub fn capacity_gb() -> Option<u64> {
    capacity().map(|sectors| (sectors * 512) / (1024 * 1024 * 1024))
}
