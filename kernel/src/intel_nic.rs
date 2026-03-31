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

// Common registers
const CTRL: u32     = 0x0000;  // Device Control
const STATUS: u32   = 0x0008;  // Device Status
const EERD: u32     = 0x0014;  // EEPROM Read
const ICR: u32      = 0x00C0;  // Interrupt Cause Read (clear on read)
const IMC: u32      = 0x00D8;  // Interrupt Mask Clear
const RCTL: u32     = 0x0100;  // Receive Control
const TCTL: u32     = 0x0400;  // Transmit Control
const TIPG: u32     = 0x0410;  // Transmit IPG
const RAL: u32      = 0x5400;  // Receive Address Low
const RAH: u32      = 0x5404;  // Receive Address High
const MTA: u32      = 0x5200;  // Multicast Table Array (128 entries)

// e1000 classic queue registers
const E1000_RDBAL: u32 = 0x2800;
const E1000_RDBAH: u32 = 0x2804;
const E1000_RDLEN: u32 = 0x2808;
const E1000_RDH: u32   = 0x2810;
const E1000_RDT: u32   = 0x2818;
const E1000_TDBAL: u32 = 0x3800;
const E1000_TDBAH: u32 = 0x3804;
const E1000_TDLEN: u32 = 0x3808;
const E1000_TDH: u32   = 0x3810;
const E1000_TDT: u32   = 0x3818;

// I225/I226 (igc) queue registers
const IGC_RDBAL: u32 = 0xC000;
const IGC_RDBAH: u32 = 0xC004;
const IGC_RDLEN: u32 = 0xC008;
const IGC_RDH: u32   = 0xC010;
const IGC_RDT: u32   = 0xC018;
const IGC_TDBAL: u32 = 0xE000;
const IGC_TDBAH: u32 = 0xE004;
const IGC_TDLEN: u32 = 0xE008;
const IGC_TDH: u32   = 0xE010;
const IGC_TDT: u32   = 0xE018;

// I225/I226 descriptor control registers (queue enable)
const IGC_RXDCTL: u32 = 0xC028;  // RX Descriptor Control queue 0
const IGC_TXDCTL: u32 = 0xE028;  // TX Descriptor Control queue 0
const IGC_SRRCTL: u32 = 0xC00C;  // Split Receive Control queue 0
const DCTL_ENABLE: u32 = 1 << 25;

// Status register bits
const STATUS_LU: u32 = 1 << 1;  // Link Up

// I225/I226 device IDs (use igc register offsets)
const IGC_IDS: &[u16] = &[0x15F3, 0x15F2, 0x125C, 0x125B];

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

/// Legacy RX Descriptor (16 bytes, for e1000)
#[repr(C)]
#[derive(Clone, Copy)]
struct LegacyRxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

/// Legacy TX Descriptor (16 bytes, for e1000)
#[repr(C)]
#[derive(Clone, Copy)]
struct LegacyTxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

/// Advanced RX Descriptor — read format (16 bytes, for I225/I226)
#[repr(C)]
#[derive(Clone, Copy)]
struct AdvRxDesc {
    pkt_addr: u64,     // Buffer physical address
    hdr_addr: u64,     // Header buffer (0 for single-buffer)
}

/// Advanced RX Descriptor — writeback format
#[repr(C)]
#[derive(Clone, Copy)]
struct AdvRxWB {
    lo: u32,
    hi: u32,
    status_error: u32,
    length_vlan: u32,  // bits [15:0] = length, [31:16] = vlan
}

/// Advanced TX Descriptor (16 bytes, for I225/I226)
#[repr(C)]
#[derive(Clone, Copy)]
struct AdvTxDesc {
    buffer_addr: u64,
    cmd_type_len: u32,
    olinfo_status: u32,
}

// Advanced TX command bits
const ADVTXD_DTYP_DATA: u32  = 0x00300000; // Data descriptor type
const ADVTXD_DCMD_DEXT: u32  = 0x20000000; // Descriptor extension
const ADVTXD_DCMD_EOP: u32   = 0x01000000; // End of packet
const ADVTXD_DCMD_IFCS: u32  = 0x02000000; // Insert FCS
const ADVTXD_DCMD_RS: u32    = 0x08000000; // Report status

// Advanced TX status bits (writeback)
const ADVTXD_STAT_DD: u32    = 1 << 0;

struct QueueRegs {
    rdbal: u32, rdbah: u32, rdlen: u32, rdh: u32, rdt: u32,
    tdbal: u32, tdbah: u32, tdlen: u32, tdh: u32, tdt: u32,
}

const E1000_REGS: QueueRegs = QueueRegs {
    rdbal: E1000_RDBAL, rdbah: E1000_RDBAH, rdlen: E1000_RDLEN, rdh: E1000_RDH, rdt: E1000_RDT,
    tdbal: E1000_TDBAL, tdbah: E1000_TDBAH, tdlen: E1000_TDLEN, tdh: E1000_TDH, tdt: E1000_TDT,
};

const IGC_REGS: QueueRegs = QueueRegs {
    rdbal: IGC_RDBAL, rdbah: IGC_RDBAH, rdlen: IGC_RDLEN, rdh: IGC_RDH, rdt: IGC_RDT,
    tdbal: IGC_TDBAL, tdbah: IGC_TDBAH, tdlen: IGC_TDLEN, tdh: IGC_TDH, tdt: IGC_TDT,
};

struct IntelNic {
    mmio: u64,
    mac_addr: [u8; 6],
    is_igc: bool,
    regs: &'static QueueRegs,
    rx_descs: u64,
    tx_descs: u64,
    rx_bufs: u64,
    tx_bufs: u64,
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

    // Enable bus mastering on PCIe bridge if device is behind one
    if dev.addr.bus > 0 {
        for d in 0u8..32 {
            for f in 0u8..8 {
                let ba = pci::PciAddr { bus: 0, device: d, function: f };
                let bid = pci::read32(ba, 0x00);
                if bid == 0xFFFF_FFFF || bid == 0 {
                    if f == 0 { break; }
                    continue;
                }
                // Header type 1 = PCI-PCI bridge
                if pci::read8(ba, 0x0E) & 0x7F == 1 {
                    let sec = pci::read8(ba, 0x19);
                    let sub = pci::read8(ba, 0x1A);
                    if dev.addr.bus >= sec && dev.addr.bus <= sub {
                        pci::enable_bus_master(ba);
                    }
                }
                if f == 0 && pci::read8(ba, 0x0E) & 0x80 == 0 { break; }
            }
        }
    }

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

    // Select register set based on device ID
    let is_igc = IGC_IDS.contains(&dev.device_id);
    let qregs: &'static QueueRegs = if is_igc { &IGC_REGS } else { &E1000_REGS };
    let variant = if is_igc { "igc" } else { "e1000" };

    kprintln!("[npk] intel-nic: variant={}, BAR0 = {:#x}", variant, bar0);

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

    // Don't full-reset — UEFI firmware already configured the PHY.
    // Just disable interrupts (we poll).
    w32(mmio, IMC, 0xFFFF_FFFF);
    let _ = r32(mmio, ICR); // Clear pending

    // Preserve UEFI link config, ensure link-up + auto-speed
    let ctrl = r32(mmio, CTRL);
    w32(mmio, CTRL, (ctrl | CTRL_SLU | CTRL_ASDE) & !(CTRL_RST));

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

    // Enable receiver BEFORE per-queue setup (matches Linux igc driver order
    // and our own TX path where TCTL_EN is set before TXDCTL enable).
    // Read-modify-write to preserve UEFI firmware bits.
    let rctl = r32(mmio, RCTL);
    w32(mmio, RCTL, (rctl & !(3 << 12)) | RCTL_EN | RCTL_BAM | RCTL_SECRC);

    if is_igc {
        // 1. Disable RX queue and wait for it
        w32(mmio, IGC_RXDCTL, 0);
        for _ in 0..100_000 {
            if r32(mmio, IGC_RXDCTL) & DCTL_ENABLE == 0 { break; }
            core::hint::spin_loop();
        }

        // 2. Ring base address + length (before SRRCTL, per Linux igc)
        w32(mmio, qregs.rdbal, rx_descs as u32);
        w32(mmio, qregs.rdbah, (rx_descs >> 32) as u32);
        w32(mmio, qregs.rdlen, rx_ring_size as u32);

        // 3. SRRCTL: read-modify-write to preserve firmware bits (kernel patch)
        let srrctl = r32(mmio, IGC_SRRCTL);
        w32(mmio, IGC_SRRCTL, (srrctl & !((7 << 25) | 0x7F))
            | (1 << 25)   // DESCTYPE = advanced one-buffer
            | 2);         // BSIZEPACKET = 2KB

        // 4. Head/tail to zero
        w32(mmio, qregs.rdh, 0);
        w32(mmio, qregs.rdt, 0);

        // 5. Init advanced RX descriptors
        for i in 0..NUM_RX_DESC {
            let desc = (rx_descs + (i * 16) as u64) as *mut AdvRxDesc;
            unsafe {
                (*desc).pkt_addr = rx_bufs + (i * RX_BUF_SIZE) as u64;
                (*desc).hdr_addr = 0;
            }
        }

        // 6. Enable RX queue (PTHRESH=8, HTHRESH=8, WTHRESH=1 per Linux igc)
        w32(mmio, IGC_RXDCTL, 8 | (8 << 8) | (1 << 16) | DCTL_ENABLE);
        let mut rxdctl_ok = false;
        for _ in 0..100_000 {
            if r32(mmio, IGC_RXDCTL) & DCTL_ENABLE != 0 {
                rxdctl_ok = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !rxdctl_ok {
            kprintln!("[npk] intel-nic: WARNING: RXDCTL enable FAILED");
        }

        // 7. Make descriptors available to NIC
        w32(mmio, qregs.rdt, (NUM_RX_DESC - 1) as u32);

        // Debug readback
        kprintln!("[npk] intel-nic: RCTL={:#010x} SRRCTL={:#010x} RXDCTL={:#010x}",
            r32(mmio, RCTL), r32(mmio, IGC_SRRCTL), r32(mmio, IGC_RXDCTL));
        kprintln!("[npk] intel-nic: RDH={} RDT={} RDLEN={}",
            r32(mmio, qregs.rdh), r32(mmio, qregs.rdt), r32(mmio, qregs.rdlen));
    } else {
        // Legacy e1000 RX init
        for i in 0..NUM_RX_DESC {
            let desc = (rx_descs + (i * 16) as u64) as *mut LegacyRxDesc;
            unsafe {
                (*desc).addr = rx_bufs + (i * RX_BUF_SIZE) as u64;
                (*desc).status = 0;
            }
        }

        w32(mmio, qregs.rdbal, rx_descs as u32);
        w32(mmio, qregs.rdbah, (rx_descs >> 32) as u32);
        w32(mmio, qregs.rdlen, rx_ring_size as u32);
        w32(mmio, qregs.rdh, 0);
        w32(mmio, qregs.rdt, (NUM_RX_DESC - 1) as u32);
    }

    // === Setup TX ===
    let tx_ring_size = NUM_TX_DESC * 16;
    let tx_ring_pages = (tx_ring_size + 4095) / 4096;
    let tx_descs = match memory::allocate_contiguous(tx_ring_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: TX ring alloc failed"); return false; }
    };
    unsafe { core::ptr::write_bytes(tx_descs as *mut u8, 0, tx_ring_pages * 4096); }

    // Allocate TX buffers (2KB aligned per buffer, not MTU)
    let tx_buf_pages = (NUM_TX_DESC * RX_BUF_SIZE + 4095) / 4096;
    let tx_bufs = match memory::allocate_contiguous(tx_buf_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: TX buf alloc failed"); return false; }
    };

    if is_igc {
        // 1. Disable TX queue
        w32(mmio, IGC_TXDCTL, 0);
        // Flush + wait
        let _ = r32(mmio, STATUS);
        for _ in 0..100_000 { core::hint::spin_loop(); }

        // 2. Configure ring
        w32(mmio, qregs.tdlen, tx_ring_size as u32);
        w32(mmio, qregs.tdbal, tx_descs as u32);
        w32(mmio, qregs.tdbah, (tx_descs >> 32) as u32);
        w32(mmio, qregs.tdh, 0);
        w32(mmio, qregs.tdt, 0);

        // 3. TIPG
        w32(mmio, TIPG, 8 | (8 << 10) | (6 << 20)); // igc values from Linux

        // 4. Enable transmitter
        w32(mmio, TCTL, TCTL_EN | TCTL_PSP | (15 << TCTL_CT_SHIFT));

        // 5. Enable TX queue with thresholds (PTHRESH=8, HTHRESH=1, WTHRESH=16)
        w32(mmio, IGC_TXDCTL, 0x02100108);
        for _ in 0..100_000 {
            if r32(mmio, IGC_TXDCTL) & DCTL_ENABLE != 0 { break; }
            core::hint::spin_loop();
        }
    } else {
        w32(mmio, qregs.tdbal, tx_descs as u32);
        w32(mmio, qregs.tdbah, (tx_descs >> 32) as u32);
        w32(mmio, qregs.tdlen, tx_ring_size as u32);
        w32(mmio, qregs.tdh, 0);
        w32(mmio, qregs.tdt, 0);
        w32(mmio, TIPG, 10 | (10 << 10) | (10 << 20));
        w32(mmio, TCTL, TCTL_EN | TCTL_PSP | (15 << TCTL_CT_SHIFT) | (64 << TCTL_COLD_SHIFT));
    }

    // Wait for link up (max 3 seconds)
    kprintln!("[npk] intel-nic: waiting for link...");
    for _ in 0..3_000_000u32 {
        if r32(mmio, STATUS) & STATUS_LU != 0 { break; }
        core::hint::spin_loop();
    }
    if r32(mmio, STATUS) & STATUS_LU != 0 {
        kprintln!("[npk] intel-nic: link up");
    } else {
        kprintln!("[npk] intel-nic: WARNING: no link");
    }

    // Debug: show buffer addresses
    kprintln!("[npk] intel-nic: RX descs={:#x} bufs={:#x}", rx_descs, rx_bufs);
    kprintln!("[npk] intel-nic: TX descs={:#x} bufs={:#x}", tx_descs, tx_bufs);
    kprintln!("[npk] intel-nic: online");
    AVAILABLE.store(true, Ordering::Relaxed);
    *DEVICE.lock() = Some(IntelNic {
        mmio, mac_addr: mac, is_igc, regs: qregs,
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
    let buf_addr = dev.tx_bufs + (i * RX_BUF_SIZE) as u64; // 2KB aligned
    unsafe { core::ptr::copy_nonoverlapping(frame.as_ptr(), buf_addr as *mut u8, frame.len()); }

    if dev.is_igc {
        // Advanced TX descriptor
        let desc = (dev.tx_descs + (i * 16) as u64) as *mut AdvTxDesc;

        // Wait for previous descriptor done
        for _ in 0..1_000_000u32 {
            let wb_status = unsafe { core::ptr::read_volatile(&(*desc).olinfo_status) };
            if wb_status & ADVTXD_STAT_DD != 0 || unsafe { (*desc).cmd_type_len } == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        let len = frame.len() as u32;
        unsafe {
            core::ptr::write_volatile(&mut (*desc).buffer_addr, buf_addr);
            core::ptr::write_volatile(&mut (*desc).olinfo_status, len << 14);
            // Write cmd_type_len LAST (this triggers the NIC)
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            core::ptr::write_volatile(&mut (*desc).cmd_type_len,
                ADVTXD_DTYP_DATA | ADVTXD_DCMD_DEXT
                | ADVTXD_DCMD_EOP | ADVTXD_DCMD_IFCS | ADVTXD_DCMD_RS
                | len);
        }
    } else {
        // Legacy TX descriptor
        let desc = (dev.tx_descs + (i * 16) as u64) as *mut LegacyTxDesc;
        for _ in 0..1_000_000u32 {
            let status = unsafe { core::ptr::read_volatile(&(*desc).status) };
            let cmd = unsafe { core::ptr::read_volatile(&(*desc).cmd) };
            if status & TXD_STAT_DD != 0 || cmd == 0 { break; }
            core::hint::spin_loop();
        }
        unsafe {
            core::ptr::write_volatile(&mut (*desc).addr, buf_addr);
            core::ptr::write_volatile(&mut (*desc).length, frame.len() as u16);
            core::ptr::write_volatile(&mut (*desc).cso, 0);
            core::ptr::write_volatile(&mut (*desc).css, 0);
            core::ptr::write_volatile(&mut (*desc).special, 0);
            core::ptr::write_volatile(&mut (*desc).status, 0);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            core::ptr::write_volatile(&mut (*desc).cmd, TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS);
        }
    }

    dev.tx_cur = (i + 1) % NUM_TX_DESC;
    // Write barrier: ensure descriptor is visible to NIC before tail bump
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    w32(dev.mmio, dev.regs.tdt, dev.tx_cur as u32);

    TX_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    Ok(())
}

static TX_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static RX_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Debug: print TX/RX packet counts
pub fn debug_stats() {
    let tx = TX_COUNT.load(core::sync::atomic::Ordering::Relaxed);
    let rx = RX_COUNT.load(core::sync::atomic::Ordering::Relaxed);
    crate::kprintln!("[npk] intel-nic: TX={} RX={}", tx, rx);
}

pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut()?;

    let i = dev.rx_cur;
    let buf_addr = dev.rx_bufs + (i * RX_BUF_SIZE) as u64;

    if dev.is_igc {
        // Advanced RX: read writeback descriptor
        let desc = (dev.rx_descs + (i * 16) as u64) as *const AdvRxWB;
        let status = unsafe { core::ptr::read_volatile(&(*desc).status_error) };
        if status & RXD_STAT_DD as u32 == 0 { return None; }

        let length_vlan = unsafe { core::ptr::read_volatile(&(*desc).length_vlan) };
        let len = (length_vlan & 0xFFFF) as usize;
        let len = len.min(MTU);

        unsafe { core::ptr::copy_nonoverlapping(buf_addr as *const u8, buf.as_mut_ptr(), len); }

        // Reset descriptor to read format for reuse
        let desc_w = (dev.rx_descs + (i * 16) as u64) as *mut AdvRxDesc;
        unsafe {
            (*desc_w).pkt_addr = buf_addr;
            (*desc_w).hdr_addr = 0;
        }

        let old_cur = dev.rx_cur;
        dev.rx_cur = (i + 1) % NUM_RX_DESC;
        w32(dev.mmio, dev.regs.rdt, old_cur as u32);

        Some(len)
    } else {
        // Legacy RX
        let desc = (dev.rx_descs + (i * 16) as u64) as *mut LegacyRxDesc;
        let status = unsafe { core::ptr::read_volatile(&(*desc).status) };
        if status & RXD_STAT_DD == 0 { return None; }

        let len = unsafe { core::ptr::read_volatile(&(*desc).length) } as usize;
        let len = len.min(MTU);

        unsafe { core::ptr::copy_nonoverlapping(buf_addr as *const u8, buf.as_mut_ptr(), len); }

        unsafe {
            (*desc).status = 0;
            (*desc).length = 0;
            (*desc).errors = 0;
        }

        let old_cur = dev.rx_cur;
        dev.rx_cur = (i + 1) % NUM_RX_DESC;
        w32(dev.mmio, dev.regs.rdt, old_cur as u32);

        Some(len)
    }
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
