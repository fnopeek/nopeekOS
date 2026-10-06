//! VirtIO Block Device Driver
//!
//! Legacy (0.9.5) VirtIO PCI transport with split virtqueue.
//! Provides sector-level and block-level (4KB) I/O with TRIM/DISCARD.
//! SSD-friendly: negotiates DISCARD when available.

use core::sync::atomic::{fence, Ordering};
use spin::Mutex;
use crate::hw::{DmaRegion, PortRange};
use crate::{kprintln, memory, pci};

const VIRTIO_VENDOR: u16 = 0x1AF4;
const VIRTIO_BLK_DEV: u16 = 0x1001;

const REG_DEV_FEATURES: u16  = 0x00;
const REG_DRV_FEATURES: u16  = 0x04;
const REG_QUEUE_PFN: u16     = 0x08;
const REG_QUEUE_SIZE: u16    = 0x0C;
const REG_QUEUE_SEL: u16     = 0x0E;
const REG_QUEUE_NOTIFY: u16  = 0x10;
const REG_STATUS: u16        = 0x12;
const REG_ISR: u16           = 0x13;

const S_ACKNOWLEDGE: u8 = 1;
const S_DRIVER: u8      = 2;
const S_DRIVER_OK: u8   = 4;
const S_FAILED: u8      = 128;

const BLK_T_IN: u32      = 0;
const BLK_T_OUT: u32     = 1;
const BLK_T_DISCARD: u32 = 11;
const F_DISCARD: u32     = 1 << 13;

const DESC_F_NEXT: u16  = 1;
const DESC_F_WRITE: u16 = 2;

pub const SECTOR_SIZE: usize = 512;
pub const BLOCK_SIZE: usize = 4096;
pub const SECTORS_PER_BLOCK: u64 = (BLOCK_SIZE / SECTOR_SIZE) as u64;

/// Legacy I/O BAR: common header (0x14, 0x18 with MSI-X) plus the
/// 64-bit capacity field of the device config.
const IO_LEN: u16 = 0x20;

// virtio_blk_outhdr: type u32 @0, reserved u32 @4, sector u64 @8.
// virtio_blk_discard_write_zeroes: sector u64 @0, num_sectors u32 @8, flags u32 @12.
// vring_desc (16 bytes): addr u64 @0, len u32 @8, flags u16 @12, next u16 @14.
const DESC_SIZE: u64 = 16;

struct VirtioBlk {
    io: PortRange,
    queue_size: u16,
    num_free: u16,
    free_head: u16,
    avail_idx: u16,
    last_used_idx: u16,
    capacity_sectors: u64,
    has_discard: bool,

    /// Descriptor table at 0, avail ring at `avail_off`, used ring at `used_off`.
    queue: DmaRegion,
    avail_off: u64,
    used_off: u64,
    /// One 16-byte request header per descriptor index.
    req_hdrs: DmaRegion,
    /// One status byte per descriptor index.
    status_buf: DmaRegion,
}

static DEVICE: Mutex<Option<VirtioBlk>> = Mutex::new(None);

pub fn init() -> bool {
    let dev = match pci::find_device(VIRTIO_VENDOR, VIRTIO_BLK_DEV) {
        Some(d) => d,
        None => {
            crate::kdebug!("[npk] virtio-blk: no device found");
            return false;
        }
    };

    kprintln!("[npk] virtio-blk: PCI {:02x}:{:02x}.{} IRQ {}",
        dev.addr.bus, dev.addr.device, dev.addr.function, dev.irq_line);

    if dev.bar0 & 1 == 0 {
        kprintln!("[npk] virtio-blk: BAR0 is MMIO — legacy I/O required");
        return false;
    }
    // SAFETY: BAR0 is this virtio-blk device's legacy I/O BAR, which this
    // driver owns; IO_LEN does not exceed the header plus capacity field.
    let io = unsafe { PortRange::new((dev.bar0 & 0xFFFC) as u16, IO_LEN) };

    pci::enable_bus_master(dev.addr);
    let cfg_off: u16 = if pci::msix_enabled(dev.addr) { 24 } else { 20 };

    io.outb(REG_STATUS, 0);
    io.outb(REG_STATUS, S_ACKNOWLEDGE);
    io.outb(REG_STATUS, S_ACKNOWLEDGE | S_DRIVER);

    let features = io.inl(REG_DEV_FEATURES);
    let has_discard = features & F_DISCARD != 0;
    io.outl(REG_DRV_FEATURES, if has_discard { F_DISCARD } else { 0 });

    let cap_lo = io.inl(cfg_off) as u64;
    let cap_hi = io.inl(cfg_off + 4) as u64;
    let capacity_sectors = cap_lo | (cap_hi << 32);
    let mb = (capacity_sectors * SECTOR_SIZE as u64) / (1024 * 1024);
    kprintln!("[npk] virtio-blk: {} sectors ({} MB), TRIM={}",
        capacity_sectors, mb, if has_discard { "yes" } else { "no" });

    io.outw(REG_QUEUE_SEL, 0);
    let qs = io.inw(REG_QUEUE_SIZE);
    if qs == 0 || qs > 1024 {
        kprintln!("[npk] virtio-blk: invalid queue size {}", qs);
        io.outb(REG_STATUS, S_FAILED);
        return false;
    }

    let q = qs as usize;
    let part1 = align_up(16 * q + 6 + 2 * q, 4096);
    let part2 = align_up(6 + 8 * q, 4096);
    let pages = (part1 + part2 + 4095) / 4096;

    let queue = match DmaRegion::alloc_zeroed(pages) {
        Some(r) => r,
        None => {
            kprintln!("[npk] virtio-blk: queue alloc failed");
            io.outb(REG_STATUS, S_FAILED);
            return false;
        }
    };

    let avail_off = (16 * q) as u64;
    let used_off = part1 as u64;

    for i in 0..q {
        queue.w16(i as u64 * DESC_SIZE + 14, if i + 1 < q { (i + 1) as u16 } else { 0 });
    }
    queue.w16(avail_off, 1);
    io.outl(REG_QUEUE_PFN, (queue.phys() >> 12) as u32);

    let req_hdrs = match DmaRegion::alloc_zeroed((q * 16 + 4095) / 4096) {
        Some(r) => r,
        None => {
            kprintln!("[npk] virtio-blk: header alloc failed");
            io.outb(REG_STATUS, S_FAILED);
            return false;
        }
    };

    let status_buf = match memory::allocate_frame() {
        // SAFETY: a fresh frame from the allocator, identity-mapped RAM that
        // this driver keeps for the device's lifetime.
        Some(a) => unsafe { DmaRegion::from_raw(a, 4096) },
        None => {
            kprintln!("[npk] virtio-blk: status alloc failed");
            io.outb(REG_STATUS, S_FAILED);
            return false;
        }
    };
    status_buf.zero();

    io.outb(REG_STATUS, S_ACKNOWLEDGE | S_DRIVER | S_DRIVER_OK);
    if io.inb(REG_STATUS) & S_FAILED != 0 {
        kprintln!("[npk] virtio-blk: device rejected initialization");
        return false;
    }

    *DEVICE.lock() = Some(VirtioBlk {
        io, queue_size: qs, num_free: qs,
        free_head: 0, avail_idx: 0, last_used_idx: 0,
        capacity_sectors, has_discard,
        queue, avail_off, used_off, req_hdrs, status_buf,
    });

    kprintln!("[npk] virtio-blk: online");
    true
}

// === Sector-level API ===

pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(BlkError::NotInitialized)?;
    if sector >= dev.capacity_sectors { return Err(BlkError::OutOfRange); }
    dev.do_rw(BLK_T_IN, sector, buf.as_mut_ptr() as u64, SECTOR_SIZE as u32, true)
}

pub fn write_sector(sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(BlkError::NotInitialized)?;
    if sector >= dev.capacity_sectors { return Err(BlkError::OutOfRange); }
    dev.do_rw(BLK_T_OUT, sector, buf.as_ptr() as u64, SECTOR_SIZE as u32, false)
}

// === Block-level API (4KB, for npkFS) ===

pub fn read_block(block: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(BlkError::NotInitialized)?;
    let sector = block * SECTORS_PER_BLOCK;
    if sector + SECTORS_PER_BLOCK > dev.capacity_sectors { return Err(BlkError::OutOfRange); }
    dev.do_rw(BLK_T_IN, sector, buf.as_mut_ptr() as u64, BLOCK_SIZE as u32, true)
}

pub fn write_block(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(BlkError::NotInitialized)?;
    let sector = block * SECTORS_PER_BLOCK;
    if sector + SECTORS_PER_BLOCK > dev.capacity_sectors { return Err(BlkError::OutOfRange); }
    dev.do_rw(BLK_T_OUT, sector, buf.as_ptr() as u64, BLOCK_SIZE as u32, false)
}

/// Durability barrier. We negotiate write-through (no VIRTIO_BLK_F_FLUSH in
/// `init`), so per the virtio spec the device keeps no volatile write cache and
/// there is nothing to flush — an explicit no-op so `blkdev::flush()` has a
/// backend on the QEMU path.
pub fn flush() -> Result<(), BlkError> {
    Ok(())
}

/// No separate FUA path under write-through negotiation — a completed
/// `write_block` is already as durable as the device gets. Mirrors
/// `write_block` so `blkdev` has a backend.
pub fn write_block_fua(block: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), BlkError> {
    write_block(block, buf)
}

/// TRIM/DISCARD blocks. Silent no-op if device doesn't support DISCARD.
pub fn discard_blocks(start: u64, count: u64) -> Result<(), BlkError> {
    if count == 0 { return Ok(()); }
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(BlkError::NotInitialized)?;
    if !dev.has_discard { return Ok(()); }

    let start_sector = start * SECTORS_PER_BLOCK;
    let num_sectors = count * SECTORS_PER_BLOCK;
    if start_sector + num_sectors > dev.capacity_sectors { return Err(BlkError::OutOfRange); }

    let d0 = dev.alloc_desc().ok_or(BlkError::QueueFull)?;
    let d1 = dev.alloc_desc().ok_or(BlkError::QueueFull)?;
    let d2 = dev.alloc_desc().ok_or(BlkError::QueueFull)?;

    let hdr_off = d0 as u64 * 16;
    let seg_off = d1 as u64 * 16; // reuse header slot for discard segment
    let stat_off = d0 as u64;

    dev.req_hdrs.w32(hdr_off, BLK_T_DISCARD);
    dev.req_hdrs.w32(hdr_off + 4, 0);
    dev.req_hdrs.w64(hdr_off + 8, 0);

    dev.req_hdrs.w64(seg_off, start_sector);
    dev.req_hdrs.w32(seg_off + 8, num_sectors as u32);
    dev.req_hdrs.w32(seg_off + 12, 0);

    dev.status_buf.w8(stat_off, 0xFF);

    dev.set_desc(d0, dev.req_hdrs.phys() + hdr_off, 16, DESC_F_NEXT, d1);
    dev.set_desc(d1, dev.req_hdrs.phys() + seg_off, 16, DESC_F_NEXT, d2);
    dev.set_desc(d2, dev.status_buf.phys() + stat_off, 1, DESC_F_WRITE, 0);

    let result = dev.submit_and_poll(d0);
    let status = dev.status_buf.r8(stat_off);

    dev.free_desc(d2);
    dev.free_desc(d1);
    dev.free_desc(d0);

    result?;
    match status {
        0 | 2 => Ok(()), // UNSUPPORTED treated as OK (graceful degradation)
        1 => Err(BlkError::IoError),
        0xFF => Err(BlkError::Timeout),
        s => Err(BlkError::Unknown(s)),
    }
}

/// Total 4KB blocks on device
pub fn block_count() -> Option<u64> {
    DEVICE.lock().as_ref().map(|d| d.capacity_sectors / SECTORS_PER_BLOCK)
}

/// Total 512-byte sectors
pub fn capacity() -> Option<u64> {
    DEVICE.lock().as_ref().map(|d| d.capacity_sectors)
}

pub fn has_discard() -> bool {
    DEVICE.lock().as_ref().map_or(false, |d| d.has_discard)
}

pub fn is_available() -> bool {
    DEVICE.lock().is_some()
}

// === Internal ===

impl VirtioBlk {
    fn set_desc(&self, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
        let o = idx as u64 * DESC_SIZE;
        self.queue.w64(o, addr);
        self.queue.w32(o + 8, len);
        self.queue.w16(o + 12, flags);
        self.queue.w16(o + 14, next);
    }

    fn alloc_desc(&mut self) -> Option<u16> {
        if self.num_free == 0 { return None; }
        let idx = self.free_head;
        self.free_head = self.queue.r16(idx as u64 * DESC_SIZE + 14);
        self.num_free -= 1;
        Some(idx)
    }

    fn free_desc(&mut self, idx: u16) {
        let o = idx as u64 * DESC_SIZE;
        self.queue.w16(o + 12, 0);
        self.queue.w16(o + 14, self.free_head);
        self.free_head = idx;
        self.num_free += 1;
    }

    fn do_rw(&mut self, req_type: u32, sector: u64, buf_addr: u64, buf_len: u32, buf_writable: bool) -> Result<(), BlkError> {
        let d0 = self.alloc_desc().ok_or(BlkError::QueueFull)?;
        let d1 = self.alloc_desc().ok_or(BlkError::QueueFull)?;
        let d2 = self.alloc_desc().ok_or(BlkError::QueueFull)?;

        let hdr_off = d0 as u64 * 16;
        let stat_off = d0 as u64;

        self.req_hdrs.w32(hdr_off, req_type);
        self.req_hdrs.w32(hdr_off + 4, 0);
        self.req_hdrs.w64(hdr_off + 8, sector);
        self.status_buf.w8(stat_off, 0xFF);

        self.set_desc(d0, self.req_hdrs.phys() + hdr_off, 16, DESC_F_NEXT, d1);
        self.set_desc(d1, buf_addr, buf_len,
            if buf_writable { DESC_F_WRITE | DESC_F_NEXT } else { DESC_F_NEXT }, d2);
        self.set_desc(d2, self.status_buf.phys() + stat_off, 1, DESC_F_WRITE, 0);

        let result = self.submit_and_poll(d0);
        let status = self.status_buf.r8(stat_off);

        self.free_desc(d2);
        self.free_desc(d1);
        self.free_desc(d0);

        result?;
        status_to_result(status)
    }

    fn submit_and_poll(&mut self, head: u16) -> Result<(), BlkError> {
        let avail_ring = self.avail_off + 4;
        let used_idx = self.used_off + 2;

        self.queue.w16(avail_ring + (self.avail_idx % self.queue_size) as u64 * 2, head);
        fence(Ordering::SeqCst);

        self.avail_idx = self.avail_idx.wrapping_add(1);
        self.queue.w16(self.avail_off + 2, self.avail_idx);
        fence(Ordering::SeqCst);

        self.io.outw(REG_QUEUE_NOTIFY, 0);

        for _ in 0..2_000_000u32 {
            let idx = self.queue.r16(used_idx);
            if idx != self.last_used_idx {
                self.last_used_idx = idx;
                self.io.inb(REG_ISR);
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(BlkError::Timeout)
    }
}

fn status_to_result(status: u8) -> Result<(), BlkError> {
    match status {
        0 => Ok(()),
        1 => Err(BlkError::IoError),
        2 => Err(BlkError::Unsupported),
        0xFF => Err(BlkError::Timeout),
        s => Err(BlkError::Unknown(s)),
    }
}

#[derive(Debug)]
pub enum BlkError {
    NotInitialized,
    OutOfRange,
    QueueFull,
    IoError,
    Unsupported,
    Timeout,
    Unknown(u8),
}

impl core::fmt::Display for BlkError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            BlkError::NotInitialized => write!(f, "disk not initialized"),
            BlkError::OutOfRange => write!(f, "sector out of range"),
            BlkError::QueueFull => write!(f, "virtqueue full"),
            BlkError::IoError => write!(f, "I/O error"),
            BlkError::Unsupported => write!(f, "unsupported operation"),
            BlkError::Timeout => write!(f, "request timed out"),
            BlkError::Unknown(s) => write!(f, "unknown status: {}", s),
        }
    }
}

fn align_up(val: usize, align: usize) -> usize {
    (val + align - 1) & !(align - 1)
}
