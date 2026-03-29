//! VirtIO Block Device Driver
//!
//! Legacy (0.9.5) VirtIO PCI transport with split virtqueue.
//! Provides sector-level read/write via polling.
//! Identity-mapped memory = physical addresses for DMA.

use core::sync::atomic::{fence, Ordering};
use spin::Mutex;
use crate::serial::{outb, outw, outl, inb, inw, inl};
use crate::{kprintln, memory, pci};

const VIRTIO_VENDOR: u16 = 0x1AF4;
const VIRTIO_BLK_DEV: u16 = 0x1001;

// Legacy VirtIO PCI register offsets (from I/O base)
const REG_DEV_FEATURES: u16  = 0x00;
const REG_DRV_FEATURES: u16  = 0x04;
const REG_QUEUE_PFN: u16     = 0x08;
const REG_QUEUE_SIZE: u16    = 0x0C;
const REG_QUEUE_SEL: u16     = 0x0E;
const REG_QUEUE_NOTIFY: u16  = 0x10;
const REG_STATUS: u16        = 0x12;
const REG_ISR: u16           = 0x13;

// VirtIO device status bits
const S_ACKNOWLEDGE: u8 = 1;
const S_DRIVER: u8      = 2;
const S_DRIVER_OK: u8   = 4;
const S_FAILED: u8      = 128;

// Block request types
const BLK_T_IN: u32  = 0;
const BLK_T_OUT: u32 = 1;

// Virtqueue descriptor flags
const DESC_F_NEXT: u16  = 1;
const DESC_F_WRITE: u16 = 2;

pub const SECTOR_SIZE: usize = 512;

#[repr(C)]
struct BlkReqHeader {
    req_type: u32,
    _reserved: u32,
    sector: u64,
}

#[repr(C)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

struct VirtioBlk {
    io_base: u16,
    queue_size: u16,
    num_free: u16,
    free_head: u16,
    avail_idx: u16,
    last_used_idx: u16,
    capacity: u64,

    desc_base: u64,
    avail_base: u64,
    used_base: u64,
    req_hdrs: u64,
    status_buf: u64,
}

static DEVICE: Mutex<Option<VirtioBlk>> = Mutex::new(None);

pub fn init() -> bool {
    let dev = match pci::find_device(VIRTIO_VENDOR, VIRTIO_BLK_DEV) {
        Some(d) => d,
        None => {
            kprintln!("[npk] virtio-blk: no device found");
            return false;
        }
    };

    kprintln!("[npk] virtio-blk: PCI {:02x}:{:02x}.{} IRQ {}",
        dev.addr.bus, dev.addr.device, dev.addr.function, dev.irq_line);

    if dev.bar0 & 1 == 0 {
        kprintln!("[npk] virtio-blk: BAR0 is MMIO — legacy I/O required");
        return false;
    }
    let io = (dev.bar0 & 0xFFFC) as u16;

    pci::enable_bus_master(dev.addr);

    // MSI-X shifts device config by 4 bytes only when enabled
    let cfg_off: u16 = if pci::msix_enabled(dev.addr) { 24 } else { 20 };

    // SAFETY: All port I/O below targets the VirtIO device's I/O BAR
    unsafe {
        // Reset
        outb(io + REG_STATUS, 0);
        outb(io + REG_STATUS, S_ACKNOWLEDGE);
        outb(io + REG_STATUS, S_ACKNOWLEDGE | S_DRIVER);

        let features = inl(io + REG_DEV_FEATURES);
        kprintln!("[npk] virtio-blk: features {:#010x}", features);
        outl(io + REG_DRV_FEATURES, 0);

        let cap_lo = inl(io + cfg_off) as u64;
        let cap_hi = inl(io + cfg_off + 4) as u64;
        let capacity = cap_lo | (cap_hi << 32);
        let mb = (capacity * SECTOR_SIZE as u64) / (1024 * 1024);
        kprintln!("[npk] virtio-blk: {} sectors ({} MB)", capacity, mb);

        // Setup virtqueue 0
        outw(io + REG_QUEUE_SEL, 0);
        let qs = inw(io + REG_QUEUE_SIZE);
        if qs == 0 || qs > 1024 {
            kprintln!("[npk] virtio-blk: invalid queue size {}", qs);
            outb(io + REG_STATUS, S_FAILED);
            return false;
        }

        let q = qs as usize;
        let part1 = align_up(16 * q + 6 + 2 * q, 4096);
        let part2 = align_up(6 + 8 * q, 4096);
        let total_bytes = part1 + part2;
        let pages = (total_bytes + 4095) / 4096;

        let qmem = match memory::allocate_contiguous(pages) {
            Some(a) => a,
            None => {
                kprintln!("[npk] virtio-blk: queue alloc failed ({} pages)", pages);
                outb(io + REG_STATUS, S_FAILED);
                return false;
            }
        };
        core::ptr::write_bytes(qmem as *mut u8, 0, pages * 4096);

        let desc_base = qmem;
        let avail_base = qmem + (16 * q) as u64;
        let used_base = qmem + part1 as u64;

        // Build descriptor free chain
        for i in 0..q {
            let d = (desc_base + (i * 16) as u64) as *mut VringDesc;
            (*d).next = if i + 1 < q { (i + 1) as u16 } else { 0 };
        }

        // Suppress device interrupts (we poll)
        *(avail_base as *mut u16) = 1;

        // Tell device the queue PFN
        outl(io + REG_QUEUE_PFN, (qmem >> 12) as u32);

        // Allocate request header + status buffers
        let req_hdrs = match memory::allocate_contiguous((q * 16 + 4095) / 4096) {
            Some(a) => a,
            None => {
                kprintln!("[npk] virtio-blk: header alloc failed");
                outb(io + REG_STATUS, S_FAILED);
                return false;
            }
        };
        core::ptr::write_bytes(req_hdrs as *mut u8, 0, (q * 16 + 4095) / 4096 * 4096);

        let status_buf = match memory::allocate_frame() {
            Some(a) => a,
            None => {
                kprintln!("[npk] virtio-blk: status alloc failed");
                outb(io + REG_STATUS, S_FAILED);
                return false;
            }
        };
        core::ptr::write_bytes(status_buf as *mut u8, 0, 4096);

        // Go live
        outb(io + REG_STATUS, S_ACKNOWLEDGE | S_DRIVER | S_DRIVER_OK);

        if inb(io + REG_STATUS) & S_FAILED != 0 {
            kprintln!("[npk] virtio-blk: device rejected initialization");
            return false;
        }

        *DEVICE.lock() = Some(VirtioBlk {
            io_base: io,
            queue_size: qs,
            num_free: qs,
            free_head: 0,
            avail_idx: 0,
            last_used_idx: 0,
            capacity,
            desc_base,
            avail_base,
            used_base,
            req_hdrs,
            status_buf,
        });
    }

    kprintln!("[npk] virtio-blk: online");
    true
}

/// Read one 512-byte sector
pub fn read_sector(sector: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(BlkError::NotInitialized)?;
    if sector >= dev.capacity { return Err(BlkError::OutOfRange); }

    let d0 = dev.alloc_desc().ok_or(BlkError::QueueFull)?;
    let d1 = dev.alloc_desc().ok_or(BlkError::QueueFull)?;
    let d2 = dev.alloc_desc().ok_or(BlkError::QueueFull)?;

    // Request header
    let hdr_addr = dev.req_hdrs + d0 as u64 * 16;
    let stat_addr = dev.status_buf + d0 as u64;
    let buf_addr = buf.as_mut_ptr() as u64;

    // SAFETY: All addresses in identity-mapped range, device does DMA
    unsafe {
        let hdr = hdr_addr as *mut BlkReqHeader;
        (*hdr).req_type = BLK_T_IN;
        (*hdr)._reserved = 0;
        (*hdr).sector = sector;
        *(stat_addr as *mut u8) = 0xFF;

        // Descriptor chain: header → data(W) → status(W)
        let desc0 = (dev.desc_base + d0 as u64 * 16) as *mut VringDesc;
        (*desc0).addr = hdr_addr;
        (*desc0).len = 16;
        (*desc0).flags = DESC_F_NEXT;
        (*desc0).next = d1;

        let desc1 = (dev.desc_base + d1 as u64 * 16) as *mut VringDesc;
        (*desc1).addr = buf_addr;
        (*desc1).len = SECTOR_SIZE as u32;
        (*desc1).flags = DESC_F_WRITE | DESC_F_NEXT;
        (*desc1).next = d2;

        let desc2 = (dev.desc_base + d2 as u64 * 16) as *mut VringDesc;
        (*desc2).addr = stat_addr;
        (*desc2).len = 1;
        (*desc2).flags = DESC_F_WRITE;
        (*desc2).next = 0;
    }

    let result = dev.submit_and_poll(d0);
    let status = unsafe { *(stat_addr as *const u8) };

    dev.free_desc(d2);
    dev.free_desc(d1);
    dev.free_desc(d0);

    result?;
    status_to_result(status)
}

/// Write one 512-byte sector
pub fn write_sector(sector: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlkError> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(BlkError::NotInitialized)?;
    if sector >= dev.capacity { return Err(BlkError::OutOfRange); }

    let d0 = dev.alloc_desc().ok_or(BlkError::QueueFull)?;
    let d1 = dev.alloc_desc().ok_or(BlkError::QueueFull)?;
    let d2 = dev.alloc_desc().ok_or(BlkError::QueueFull)?;

    let hdr_addr = dev.req_hdrs + d0 as u64 * 16;
    let stat_addr = dev.status_buf + d0 as u64;
    let buf_addr = buf.as_ptr() as u64;

    unsafe {
        let hdr = hdr_addr as *mut BlkReqHeader;
        (*hdr).req_type = BLK_T_OUT;
        (*hdr)._reserved = 0;
        (*hdr).sector = sector;
        *(stat_addr as *mut u8) = 0xFF;

        let desc0 = (dev.desc_base + d0 as u64 * 16) as *mut VringDesc;
        (*desc0).addr = hdr_addr;
        (*desc0).len = 16;
        (*desc0).flags = DESC_F_NEXT;
        (*desc0).next = d1;

        // Data buffer: device-readable (no WRITE flag)
        let desc1 = (dev.desc_base + d1 as u64 * 16) as *mut VringDesc;
        (*desc1).addr = buf_addr;
        (*desc1).len = SECTOR_SIZE as u32;
        (*desc1).flags = DESC_F_NEXT;
        (*desc1).next = d2;

        let desc2 = (dev.desc_base + d2 as u64 * 16) as *mut VringDesc;
        (*desc2).addr = stat_addr;
        (*desc2).len = 1;
        (*desc2).flags = DESC_F_WRITE;
        (*desc2).next = 0;
    }

    let result = dev.submit_and_poll(d0);
    let status = unsafe { *(stat_addr as *const u8) };

    dev.free_desc(d2);
    dev.free_desc(d1);
    dev.free_desc(d0);

    result?;
    status_to_result(status)
}

pub fn capacity() -> Option<u64> {
    DEVICE.lock().as_ref().map(|d| d.capacity)
}

pub fn is_available() -> bool {
    DEVICE.lock().is_some()
}

impl VirtioBlk {
    fn alloc_desc(&mut self) -> Option<u16> {
        if self.num_free == 0 { return None; }
        let idx = self.free_head;
        // SAFETY: Descriptor is in allocated queue memory
        unsafe {
            let d = (self.desc_base + idx as u64 * 16) as *const VringDesc;
            self.free_head = (*d).next;
        }
        self.num_free -= 1;
        Some(idx)
    }

    fn free_desc(&mut self, idx: u16) {
        // SAFETY: Returning descriptor to free list
        unsafe {
            let d = (self.desc_base + idx as u64 * 16) as *mut VringDesc;
            (*d).flags = 0;
            (*d).next = self.free_head;
        }
        self.free_head = idx;
        self.num_free += 1;
    }

    fn submit_and_poll(&mut self, head: u16) -> Result<(), BlkError> {
        let avail_ring = self.avail_base + 4;
        let used_idx_ptr = (self.used_base + 2) as *const u16;

        // SAFETY: Writing to available ring, reading used ring (identity-mapped DMA memory)
        unsafe {
            let slot = (avail_ring + (self.avail_idx % self.queue_size) as u64 * 2) as *mut u16;
            core::ptr::write_volatile(slot, head);

            fence(Ordering::SeqCst);

            let avail_idx_ptr = (self.avail_base + 2) as *mut u16;
            self.avail_idx = self.avail_idx.wrapping_add(1);
            core::ptr::write_volatile(avail_idx_ptr, self.avail_idx);

            fence(Ordering::SeqCst);

            outw(self.io_base + REG_QUEUE_NOTIFY, 0);

            // Poll used ring
            for _ in 0..2_000_000u32 {
                let idx = core::ptr::read_volatile(used_idx_ptr);
                if idx != self.last_used_idx {
                    self.last_used_idx = idx;
                    // Acknowledge any pending ISR
                    inb(self.io_base + REG_ISR);
                    return Ok(());
                }
                core::hint::spin_loop();
            }
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
