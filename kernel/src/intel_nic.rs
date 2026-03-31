//! Intel Ethernet Driver (I225-V / I226-V / e1000 family)
//!
//! MMIO via BAR0. RX/TX descriptor rings with DMA.
//! Polling model (no interrupts). Exposes same API as virtio_net.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use crate::{kprintln, pci, paging, memory};
use crate::paging::PageFlags;
use crate::virtio_net::NetError;

pub const MTU: usize = 1514;

const INTEL_VENDOR: u16 = 0x8086;

// Known Intel NIC device IDs
const KNOWN_IDS: &[u16] = &[
    0x15F3, // I225-V
    0x15F2, // I225-LM
    0x125C, // I226-V
    0x125B, // I226-LM
    0x15BC, // I219-V (Cannon Lake)
    0x15BD, // I219-LM
    0x15BE, // I219-V
    0x0D4F, // I219-LM (Comet Lake)
    0x0D4E, // I219-V (Comet Lake)
    0x1A1E, // I219-LM (Alder Lake)
    0x1A1F, // I219-V (Alder Lake)
    0x550A, // I219-V (Raptor Lake)
    0x550B, // I219-LM (Raptor Lake)
    // Classic e1000/e1000e
    0x100E, // 82540EM (QEMU default e1000)
    0x100F, // 82545EM
    0x10D3, // 82574L
    0x153A, // I217-LM
    0x153B, // I217-V
];

// Register offsets
const CTRL: u32     = 0x0000;  // Device Control
const STATUS: u32   = 0x0008;  // Device Status
const EERD: u32     = 0x0014;  // EEPROM Read
const ICR: u32      = 0x00C0;  // Interrupt Cause Read (clear on read)
const IMS: u32      = 0x00D0;  // Interrupt Mask Set
const IMC: u32      = 0x00D8;  // Interrupt Mask Clear
const RCTL: u32     = 0x0100;  // Receive Control
const TCTL: u32     = 0x0400;  // Transmit Control
const TIPG: u32     = 0x0410;  // Transmit IPG
const RDBAL: u32    = 0x2800;  // RX Descriptor Base Low
const RDBAH: u32    = 0x2804;  // RX Descriptor Base High
const RDLEN: u32    = 0x2808;  // RX Descriptor Length
const RDH: u32      = 0x2810;  // RX Descriptor Head
const RDT: u32      = 0x2818;  // RX Descriptor Tail
const TDBAL: u32    = 0x3800;  // TX Descriptor Base Low
const TDBAH: u32    = 0x3804;  // TX Descriptor Base High
const TDLEN: u32    = 0x3808;  // TX Descriptor Length
const TDH: u32      = 0x3810;  // TX Descriptor Head
const TDT: u32      = 0x3818;  // TX Descriptor Tail
const RAL: u32      = 0x5400;  // Receive Address Low
const RAH: u32      = 0x5404;  // Receive Address High
const MTA: u32      = 0x5200;  // Multicast Table Array (128 entries)

// CTRL bits
const CTRL_RST: u32     = 1 << 26;
const CTRL_SLU: u32     = 1 << 6;   // Set Link Up
const CTRL_ASDE: u32    = 1 << 5;   // Auto-Speed Detection Enable

// RCTL bits
const RCTL_EN: u32      = 1 << 1;   // Receiver Enable
const RCTL_SBP: u32     = 1 << 2;   // Store Bad Packets
const RCTL_UPE: u32     = 1 << 3;   // Unicast Promiscuous
const RCTL_MPE: u32     = 1 << 4;   // Multicast Promiscuous
const RCTL_BAM: u32     = 1 << 15;  // Broadcast Accept Mode
const RCTL_BSIZE_4K: u32 = (3 << 16) | (1 << 25); // Buffer size 4096
const RCTL_BSIZE_2K: u32 = 0;       // Buffer size 2048 (default)
const RCTL_SECRC: u32   = 1 << 26;  // Strip Ethernet CRC

// TCTL bits
const TCTL_EN: u32      = 1 << 1;   // Transmit Enable
const TCTL_PSP: u32     = 1 << 3;   // Pad Short Packets
const TCTL_CT_SHIFT: u32 = 4;       // Collision Threshold
const TCTL_COLD_SHIFT: u32 = 12;    // Collision Distance

// RX descriptor status bits
const RXD_STAT_DD: u8   = 1 << 0;   // Descriptor Done
const RXD_STAT_EOP: u8  = 1 << 1;   // End of Packet

// TX descriptor command bits
const TXD_CMD_EOP: u8   = 1 << 0;   // End of Packet
const TXD_CMD_IFCS: u8  = 1 << 1;   // Insert FCS/CRC
const TXD_CMD_RS: u8    = 1 << 3;   // Report Status
const TXD_STAT_DD: u8   = 1 << 0;   // Descriptor Done

const NUM_RX_DESC: usize = 32;
const NUM_TX_DESC: usize = 32;
const RX_BUF_SIZE: usize = 2048;

/// RX Descriptor (legacy format, 16 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
struct RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

/// TX Descriptor (legacy format, 16 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
struct TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

struct IntelNic {
    mmio: u64,
    mac_addr: [u8; 6],
    rx_descs: u64,      // Physical addr of RX descriptor ring
    tx_descs: u64,      // Physical addr of TX descriptor ring
    rx_bufs: u64,       // Physical addr of RX buffer pool
    tx_bufs: u64,       // Physical addr of TX buffer pool
    rx_cur: usize,
    tx_cur: usize,
}

static DEVICE: Mutex<Option<IntelNic>> = Mutex::new(None);
static AVAILABLE: AtomicBool = AtomicBool::new(false);

fn r32(base: u64, reg: u32) -> u32 {
    unsafe { core::ptr::read_volatile((base + reg as u64) as *const u32) }
}

fn w32(base: u64, reg: u32, val: u32) {
    unsafe { core::ptr::write_volatile((base + reg as u64) as *mut u32, val); }
}

/// Detect and initialize Intel NIC.
pub fn init() -> bool {
    // Find by specific device IDs
    let dev = KNOWN_IDS.iter().find_map(|&did| pci::find_device(INTEL_VENDOR, did));

    // Fallback: find by class (02:00 = Ethernet, vendor Intel)
    let dev = dev.or_else(|| {
        pci::find_by_class(0x02, 0x00).filter(|d| d.vendor_id == INTEL_VENDOR)
    });

    let dev = match dev {
        Some(d) => d,
        None => return false,
    };

    kprintln!("[npk] intel-nic: PCI {:02x}:{:02x}.{} [{:04x}:{:04x}]",
        dev.addr.bus, dev.addr.device, dev.addr.function,
        dev.vendor_id, dev.device_id);

    pci::enable_bus_master(dev.addr);

    // Also enable memory space access
    let cmd = pci::read16(dev.addr, 0x04);
    pci::write32(dev.addr, 0x04, (cmd | 0x06) as u32); // Bus Master + Memory Space

    // BAR0 (MMIO) — check if 32-bit or 64-bit
    let bar0_raw = pci::read32(dev.addr, 0x10);
    let bar0 = if bar0_raw & 0x04 != 0 {
        // 64-bit BAR
        pci::read_bar64(dev.addr, 0x10)
    } else {
        // 32-bit BAR
        (bar0_raw & 0xFFFF_FFF0) as u64
    };

    if bar0 == 0 {
        kprintln!("[npk] intel-nic: BAR0 is zero");
        return false;
    }

    kprintln!("[npk] intel-nic: BAR0 = {:#x}", bar0);

    // Map BAR0 (128KB for modern Intel NICs)
    let map_size = 128 * 1024u64;
    for offset in (0..map_size).step_by(4096) {
        match paging::map_page(bar0 + offset, bar0 + offset,
            PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_CACHE) {
            Ok(()) | Err(paging::PagingError::AlreadyMapped) => {}
            Err(e) => {
                kprintln!("[npk] intel-nic: map failed at {:#x}: {:?}", bar0 + offset, e);
                return false;
            }
        }
    }

    let mmio = bar0;

    // Reset
    w32(mmio, IMC, 0xFFFF_FFFF); // Disable all interrupts
    w32(mmio, CTRL, r32(mmio, CTRL) | CTRL_RST);
    for _ in 0..100_000 { core::hint::spin_loop(); } // Brief delay
    w32(mmio, IMC, 0xFFFF_FFFF); // Disable interrupts again after reset
    let _ = r32(mmio, ICR); // Clear pending interrupts

    // Set link up
    w32(mmio, CTRL, r32(mmio, CTRL) | CTRL_SLU | CTRL_ASDE);

    // Read MAC from RAL/RAH
    let ral = r32(mmio, RAL);
    let rah = r32(mmio, RAH);
    let mac = [
        (ral & 0xFF) as u8,
        ((ral >> 8) & 0xFF) as u8,
        ((ral >> 16) & 0xFF) as u8,
        ((ral >> 24) & 0xFF) as u8,
        (rah & 0xFF) as u8,
        ((rah >> 8) & 0xFF) as u8,
    ];

    // If MAC is all zeros, try EEPROM
    let mac = if mac == [0; 6] {
        read_mac_eeprom(mmio)
    } else {
        mac
    };

    kprintln!("[npk] intel-nic: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    // Clear multicast table
    for i in 0..128 {
        w32(mmio, MTA + i * 4, 0);
    }

    // === Setup RX ===
    let rx_ring_size = NUM_RX_DESC * 16; // 16 bytes per desc
    let rx_ring_pages = (rx_ring_size + 4095) / 4096;
    let rx_descs = match memory::allocate_contiguous(rx_ring_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: RX ring alloc failed"); return false; }
    };
    unsafe { core::ptr::write_bytes(rx_descs as *mut u8, 0, rx_ring_pages * 4096); }

    // Allocate RX buffers (NUM_RX_DESC * RX_BUF_SIZE)
    let rx_buf_pages = (NUM_RX_DESC * RX_BUF_SIZE + 4095) / 4096;
    let rx_bufs = match memory::allocate_contiguous(rx_buf_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: RX buf alloc failed"); return false; }
    };
    unsafe { core::ptr::write_bytes(rx_bufs as *mut u8, 0, rx_buf_pages * 4096); }

    // Initialize RX descriptors
    for i in 0..NUM_RX_DESC {
        let desc = (rx_descs + (i * 16) as u64) as *mut RxDesc;
        unsafe {
            (*desc).addr = rx_bufs + (i * RX_BUF_SIZE) as u64;
            (*desc).status = 0;
        }
    }

    w32(mmio, RDBAL, rx_descs as u32);
    w32(mmio, RDBAH, (rx_descs >> 32) as u32);
    w32(mmio, RDLEN, rx_ring_size as u32);
    w32(mmio, RDH, 0);
    w32(mmio, RDT, (NUM_RX_DESC - 1) as u32);

    // Enable receiver
    w32(mmio, RCTL, RCTL_EN | RCTL_BAM | RCTL_BSIZE_2K | RCTL_SECRC);

    // === Setup TX ===
    let tx_ring_size = NUM_TX_DESC * 16;
    let tx_ring_pages = (tx_ring_size + 4095) / 4096;
    let tx_descs = match memory::allocate_contiguous(tx_ring_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: TX ring alloc failed"); return false; }
    };
    unsafe { core::ptr::write_bytes(tx_descs as *mut u8, 0, tx_ring_pages * 4096); }

    // Allocate TX buffers
    let tx_buf_pages = (NUM_TX_DESC * MTU + 4095) / 4096;
    let tx_bufs = match memory::allocate_contiguous(tx_buf_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: TX buf alloc failed"); return false; }
    };

    w32(mmio, TDBAL, tx_descs as u32);
    w32(mmio, TDBAH, (tx_descs >> 32) as u32);
    w32(mmio, TDLEN, tx_ring_size as u32);
    w32(mmio, TDH, 0);
    w32(mmio, TDT, 0);

    // Transmit IPG values (IEEE 802.3)
    w32(mmio, TIPG, 10 | (10 << 10) | (10 << 20));

    // Enable transmitter
    w32(mmio, TCTL, TCTL_EN | TCTL_PSP | (15 << TCTL_CT_SHIFT) | (64 << TCTL_COLD_SHIFT));

    kprintln!("[npk] intel-nic: online");
    AVAILABLE.store(true, Ordering::Relaxed);
    *DEVICE.lock() = Some(IntelNic {
        mmio, mac_addr: mac,
        rx_descs, tx_descs,
        rx_bufs, tx_bufs,
        rx_cur: 0, tx_cur: 0,
    });
    true
}

pub fn is_available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

pub fn mac() -> Option<[u8; 6]> {
    DEVICE.lock().as_ref().map(|d| d.mac_addr)
}

pub fn send(frame: &[u8]) -> Result<(), NetError> {
    if frame.len() > MTU { return Err(NetError::FrameTooLarge); }

    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(NetError::NotInitialized)?;

    let i = dev.tx_cur;

    // Wait for previous descriptor to complete (if reused)
    let desc = (dev.tx_descs + (i * 16) as u64) as *mut TxDesc;
    for _ in 0..1_000_000u32 {
        if unsafe { (*desc).status } & TXD_STAT_DD != 0 || unsafe { (*desc).cmd } == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Copy frame to TX buffer
    let buf_addr = dev.tx_bufs + (i * MTU) as u64;
    unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(), buf_addr as *mut u8, frame.len()); }

    // Setup descriptor
    unsafe {
        (*desc).addr = buf_addr;
        (*desc).length = frame.len() as u16;
        (*desc).cmd = TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS;
        (*desc).status = 0;
        (*desc).cso = 0;
        (*desc).css = 0;
        (*desc).special = 0;
    }

    // Advance tail
    dev.tx_cur = (i + 1) % NUM_TX_DESC;
    w32(dev.mmio, TDT, dev.tx_cur as u32);

    Ok(())
}

pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut()?;

    let i = dev.rx_cur;
    let desc = (dev.rx_descs + (i * 16) as u64) as *mut RxDesc;

    let status = unsafe { core::ptr::read_volatile(&(*desc).status) };
    if status & RXD_STAT_DD == 0 {
        return None; // No packet
    }

    let len = unsafe { core::ptr::read_volatile(&(*desc).length) } as usize;
    let len = len.min(MTU);

    // Copy from RX buffer
    let buf_addr = dev.rx_bufs + (i * RX_BUF_SIZE) as u64;
    unsafe { core::ptr::copy_nonoverlapping(buf_addr as *const u8, buf.as_mut_ptr(), len); }

    // Reset descriptor for reuse
    unsafe {
        (*desc).status = 0;
        (*desc).length = 0;
        (*desc).errors = 0;
    }

    // Advance and update tail
    let old_cur = dev.rx_cur;
    dev.rx_cur = (i + 1) % NUM_RX_DESC;
    w32(dev.mmio, RDT, old_cur as u32);

    Some(len)
}

/// Read MAC address from EEPROM (for NICs that don't expose it via RAL/RAH).
fn read_mac_eeprom(mmio: u64) -> [u8; 6] {
    let mut mac = [0u8; 6];
    for i in 0..3u32 {
        w32(mmio, EERD, (i << 8) | 1); // Start read at address i
        // Wait for done
        for _ in 0..10_000u32 {
            let val = r32(mmio, EERD);
            if val & (1 << 4) != 0 { // Done bit
                let data = (val >> 16) as u16;
                mac[(i * 2) as usize] = data as u8;
                mac[(i * 2 + 1) as usize] = (data >> 8) as u8;
                break;
            }
            core::hint::spin_loop();
        }
    }
    mac
}
