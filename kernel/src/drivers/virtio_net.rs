//! VirtIO Network Device Driver
//!
//! Legacy (0.9.5) VirtIO PCI transport with RX/TX virtqueues.
//! Provides Ethernet frame send/receive for the TCP/IP stack.

use core::sync::atomic::{fence, Ordering};
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

const S_ACKNOWLEDGE: u8 = 1;
const S_DRIVER: u8      = 2;
const S_DRIVER_OK: u8   = 4;
const S_FAILED: u8      = 128;

const F_MAC: u32       = 1 << 5;
#[allow(dead_code)]
const F_STATUS: u32    = 1 << 16;

const DESC_F_NEXT: u16  = 1;
const DESC_F_WRITE: u16 = 2;

const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;
const RX_BUFFERS: usize = 32;
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

    // TX queue
    tx_desc_base: u64,
    tx_avail_base: u64,
    tx_used_base: u64,
    tx_queue_size: u16,
    tx_avail_idx: u16,
    tx_last_used: u16,
    tx_num_free: u16,
    tx_free_head: u16,
    tx_hdrs: u64,   // pre-allocated net headers for TX
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
    let cfg_off: u16 = if pci::msix_enabled(dev.addr) { 24 } else { 20 };

    // SAFETY: All port I/O targets the VirtIO device's I/O BAR
    unsafe {
        outb(io + REG_STATUS, 0);
        outb(io + REG_STATUS, S_ACKNOWLEDGE);
        outb(io + REG_STATUS, S_ACKNOWLEDGE | S_DRIVER);

        let features = inl(io + REG_DEV_FEATURES);

        // Accept MAC feature only
        let accepted = features & F_MAC;
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
            rx_used_base: rx_used,
            rx_queue_size: rx_qs,
            rx_last_used: 0,
            rx_buffers,
            tx_desc_base: tx_desc,
            tx_avail_base: tx_avail,
            tx_used_base: tx_used,
            tx_queue_size: tx_qs,
            tx_avail_idx: 0,
            tx_last_used: 0,
            tx_num_free: tx_qs,
            tx_free_head: 0,
            tx_hdrs,
        });
    }

    kprintln!("[npk] virtio-net: online");
    true
}

/// Send an Ethernet frame. `frame` must be a complete Ethernet frame (dst + src + type + payload).
pub fn send(frame: &[u8]) -> Result<(), NetError> {
    if frame.len() > MTU { return Err(NetError::FrameTooLarge); }

    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(NetError::NotInitialized)?;

    // Reclaim completed TX descriptors
    dev.reclaim_tx();

    if dev.tx_num_free < 2 { return Err(NetError::QueueFull); }

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

        // Descriptor 1: frame data
        let desc1 = (dev.tx_desc_base + d1 as u64 * 16) as *mut VringDesc;
        (*desc1).addr = frame.as_ptr() as u64;
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
        outw(dev.io_base + REG_QUEUE_NOTIFY, TX_QUEUE);
    }

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
        return None; // no new packets
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

            // Free the descriptor chain (d0 → d1)
            let next = unsafe {
                let d = (self.tx_desc_base + id as u64 * 16) as *const VringDesc;
                (*d).next
            };
            self.free_tx_desc(next);
            self.free_tx_desc(id);

            self.tx_last_used = self.tx_last_used.wrapping_add(1);
        }
        unsafe { inb(self.io_base + REG_ISR); }
    }

    fn repost_rx(&mut self, desc_idx: usize) {
        // Re-add this buffer to the RX available ring
        let avail_ring = self.rx_avail_base + 4;
        let avail_idx_ptr = (self.rx_avail_base + 2) as *mut u16;

        unsafe {
            let idx = core::ptr::read_volatile(avail_idx_ptr);
            let slot = (avail_ring + (idx % self.rx_queue_size) as u64 * 2) as *mut u16;
            core::ptr::write_volatile(slot, desc_idx as u16);
            fence(Ordering::SeqCst);
            core::ptr::write_volatile(avail_idx_ptr, idx.wrapping_add(1));
            outw(self.io_base + REG_QUEUE_NOTIFY, RX_QUEUE);
        }
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
