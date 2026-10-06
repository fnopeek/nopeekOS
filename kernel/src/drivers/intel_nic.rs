//! Intel Ethernet Driver (e1000 / e1000e / I219 family), and the front door
//! for I225/I226: those are handed to `igc` after BAR0 is mapped.
//!
//! MMIO via BAR0. Legacy RX/TX descriptor rings with DMA, polled.
//! Exposes same API as virtio_net.

use core::sync::atomic::{AtomicBool, Ordering};
use super::igc;
use spin::Mutex;
use crate::{kprintln, pci};
use crate::hw::{DmaRegion, Mmio};
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

// Status register bits
const STATUS_LU: u32 = 1 << 1;  // Link Up

// I225/I226 device IDs — driven by `igc`
const IGC_IDS: &[u16] = &[0x15F3, 0x15F2, 0x125C, 0x125B];

// CTRL bits
const CTRL_RST: u32     = 1 << 26;
const CTRL_SLU: u32     = 1 << 6;   // Set Link Up
const CTRL_ASDE: u32    = 1 << 5;   // Auto-Speed Detection Enable

// RCTL bits
const RCTL_EN: u32      = 1 << 1;   // Receiver Enable
#[allow(dead_code)]
const RCTL_SBP: u32     = 1 << 2;   // Store Bad Packets
#[allow(dead_code)]
const RCTL_UPE: u32     = 1 << 3;   // Unicast Promiscuous
#[allow(dead_code)]
const RCTL_MPE: u32     = 1 << 4;   // Multicast Promiscuous
const RCTL_BAM: u32     = 1 << 15;  // Broadcast Accept Mode
#[allow(dead_code)]
const RCTL_BSIZE_4K: u32 = (3 << 16) | (1 << 25); // Buffer size 4096
#[allow(dead_code)]
const RCTL_BSIZE_2K: u32 = 0;       // Buffer size 2048 (default)
const RCTL_SECRC: u32   = 1 << 26;  // Strip Ethernet CRC

// TCTL bits
const TCTL_EN: u32      = 1 << 1;   // Transmit Enable
const TCTL_PSP: u32     = 1 << 3;   // Pad Short Packets
const TCTL_CT_SHIFT: u32 = 4;       // Collision Threshold
const TCTL_COLD_SHIFT: u32 = 12;    // Collision Distance

// RX descriptor status bits
const RXD_STAT_DD: u8   = 1 << 0;   // Descriptor Done
#[allow(dead_code)]
const RXD_STAT_EOP: u8  = 1 << 1;   // End of Packet

// TX descriptor command bits
const TXD_CMD_EOP: u8   = 1 << 0;   // End of Packet
const TXD_CMD_IFCS: u8  = 1 << 1;   // Insert FCS/CRC
const TXD_CMD_RS: u8    = 1 << 3;   // Report Status
const TXD_STAT_DD: u8   = 1 << 0;   // Descriptor Done

// Ring depth. The host download path busy-spins the drain and needs no more;
// a consumer that drains slowly should poll faster rather than deepen this
// shared ring.
const NUM_RX_DESC: usize = 32;
const NUM_TX_DESC: usize = 32;
const RX_BUF_SIZE: usize = 2048;

// Legacy RX descriptor (16 bytes): addr u64, length u16, checksum u16,
// status u8, errors u8, special u16.
const RXD_ADDR: u64 = 0;
const RXD_LENGTH: u64 = 8;
const RXD_STATUS: u64 = 12;
const RXD_ERRORS: u64 = 13;

// Legacy TX descriptor (16 bytes): addr u64, length u16, cso u8, cmd u8,
// status u8, css u8, special u16.
const TXD_ADDR: u64 = 0;
const TXD_LENGTH: u64 = 8;
const TXD_CSO: u64 = 10;
const TXD_CMD: u64 = 11;
const TXD_STATUS: u64 = 12;
const TXD_CSS: u64 = 13;
const TXD_SPECIAL: u64 = 14;

struct QueueRegs {
    rdbal: u32, rdbah: u32, rdlen: u32, rdh: u32, rdt: u32,
    tdbal: u32, tdbah: u32, tdlen: u32, tdh: u32, tdt: u32,
}

const E1000_REGS: QueueRegs = QueueRegs {
    rdbal: E1000_RDBAL, rdbah: E1000_RDBAH, rdlen: E1000_RDLEN, rdh: E1000_RDH, rdt: E1000_RDT,
    tdbal: E1000_TDBAL, tdbah: E1000_TDBAH, tdlen: E1000_TDLEN, tdh: E1000_TDH, tdt: E1000_TDT,
};

struct IntelNic {
    mmio: Mmio,
    mac_addr: [u8; 6],
    regs: &'static QueueRegs,
    rx_descs: DmaRegion,
    tx_descs: DmaRegion,
    rx_bufs: DmaRegion,
    tx_bufs: DmaRegion,
    rx_cur: usize,
    tx_cur: usize,
}

static DEVICE: Mutex<Option<IntelNic>> = Mutex::new(None);
static AVAILABLE: AtomicBool = AtomicBool::new(false);
/// The card is an I225/I226 and `igc` owns its rings.
static IS_IGC: AtomicBool = AtomicBool::new(false);

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

    let is_igc = IGC_IDS.contains(&dev.device_id);
    let qregs: &'static QueueRegs = &E1000_REGS;
    let variant = if is_igc { "igc" } else { "e1000" };

    kprintln!("[npk] intel-nic: variant={}, BAR0 = {:#x}", variant, bar0);

    // Map BAR0 (128KB for modern Intel NICs)
    let map_size = 128 * 1024u64;
    // SAFETY: BAR0 of the Intel NIC this driver binds: its register window.
    let mmio = match unsafe { Mmio::map(bar0, map_size) } {
        Ok(m) => m,
        Err(e) => {
            kprintln!("[npk] intel-nic: map failed at {:#x}: {:?}", bar0, e);
            return false;
        }
    };

    // Don't full-reset — UEFI firmware already configured the PHY.
    // Just disable interrupts (we poll).
    mmio.w32(IMC, 0xFFFF_FFFF);
    let _ = mmio.r32(ICR); // Clear pending

    // Preserve UEFI link config, ensure link-up + auto-speed
    let ctrl = mmio.r32(CTRL);
    mmio.w32(CTRL, (ctrl | CTRL_SLU | CTRL_ASDE) & !(CTRL_RST));

    // Read MAC from RAL/RAH
    let ral = mmio.r32(RAL);
    let rah = mmio.r32(RAH);
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
        read_mac_eeprom(&mmio)
    } else {
        mac
    };

    kprintln!("[npk] intel-nic: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    // Clear multicast table
    for i in 0..128 {
        mmio.w32(MTA + i * 4, 0);
    }

    if is_igc {
        if !igc::init(dev.addr, mmio) {
            kprintln!("[npk] intel-nic: igc ring setup failed");
            return false;
        }
        IS_IGC.store(true, Ordering::Release);
        wait_for_link(&mmio);
        kprintln!("[npk] intel-nic: online (igc)");
        AVAILABLE.store(true, Ordering::Relaxed);
        *DEVICE.lock() = Some(IntelNic {
            mmio, mac_addr: mac, regs: qregs,
            rx_descs: DmaRegion::empty(), tx_descs: DmaRegion::empty(),
            rx_bufs: DmaRegion::empty(), tx_bufs: DmaRegion::empty(),
            rx_cur: 0, tx_cur: 0,
        });
        return true;
    }

    // === Setup RX ===
    let rx_ring_size = NUM_RX_DESC * 16; // 16 bytes per desc
    let rx_ring_pages = (rx_ring_size + 4095) / 4096;
    let rx_descs = match DmaRegion::alloc_zeroed(rx_ring_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: RX ring alloc failed"); return false; }
    };

    // Allocate RX buffers (NUM_RX_DESC * RX_BUF_SIZE)
    let rx_buf_pages = (NUM_RX_DESC * RX_BUF_SIZE + 4095) / 4096;
    let rx_bufs = match DmaRegion::alloc_zeroed(rx_buf_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: RX buf alloc failed"); return false; }
    };

    let rctl = mmio.r32(RCTL);
    mmio.w32(RCTL, (rctl & !(3 << 12)) | RCTL_EN | RCTL_BAM | RCTL_SECRC);

    {
        // Legacy e1000 RX init
        for i in 0..NUM_RX_DESC {
            let desc = (i * 16) as u64;
            rx_descs.w64(desc + RXD_ADDR, rx_bufs.phys() + (i * RX_BUF_SIZE) as u64);
            rx_descs.w8(desc + RXD_STATUS, 0);
        }

        mmio.w32(qregs.rdbal, rx_descs.phys() as u32);
        mmio.w32(qregs.rdbah, (rx_descs.phys() >> 32) as u32);
        mmio.w32(qregs.rdlen, rx_ring_size as u32);
        mmio.w32(qregs.rdh, 0);
        mmio.w32(qregs.rdt, (NUM_RX_DESC - 1) as u32);
    }

    // === Setup TX ===
    let tx_ring_size = NUM_TX_DESC * 16;
    let tx_ring_pages = (tx_ring_size + 4095) / 4096;
    let tx_descs = match DmaRegion::alloc_zeroed(tx_ring_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: TX ring alloc failed"); return false; }
    };

    // Allocate TX buffers (2KB aligned per buffer, not MTU)
    let tx_buf_pages = (NUM_TX_DESC * RX_BUF_SIZE + 4095) / 4096;
    let tx_bufs = match DmaRegion::alloc_zeroed(tx_buf_pages) {
        Some(a) => a,
        None => { kprintln!("[npk] intel-nic: TX buf alloc failed"); return false; }
    };

    {
        mmio.w32(qregs.tdbal, tx_descs.phys() as u32);
        mmio.w32(qregs.tdbah, (tx_descs.phys() >> 32) as u32);
        mmio.w32(qregs.tdlen, tx_ring_size as u32);
        mmio.w32(qregs.tdh, 0);
        mmio.w32(qregs.tdt, 0);
        mmio.w32(TIPG, 10 | (10 << 10) | (10 << 20));
        mmio.w32(TCTL, TCTL_EN | TCTL_PSP | (15 << TCTL_CT_SHIFT) | (64 << TCTL_COLD_SHIFT));
    }

    wait_for_link(&mmio);

    // Debug: show buffer addresses
    kprintln!("[npk] intel-nic: RX descs={:#x} bufs={:#x}", rx_descs.phys(), rx_bufs.phys());
    kprintln!("[npk] intel-nic: TX descs={:#x} bufs={:#x}", tx_descs.phys(), tx_bufs.phys());
    kprintln!("[npk] intel-nic: online");
    AVAILABLE.store(true, Ordering::Relaxed);
    *DEVICE.lock() = Some(IntelNic {
        mmio, mac_addr: mac, regs: qregs,
        rx_descs, tx_descs,
        rx_bufs, tx_bufs,
        rx_cur: 0, tx_cur: 0,
    });
    true
}

/// Wait for link up (max 3 seconds).
fn wait_for_link(mmio: &Mmio) {
    kprintln!("[npk] intel-nic: waiting for link...");
    for _ in 0..3_000_000u32 {
        if mmio.r32(STATUS) & STATUS_LU != 0 { break; }
        core::hint::spin_loop();
    }
    if mmio.r32(STATUS) & STATUS_LU != 0 {
        kprintln!("[npk] intel-nic: link up");
    } else {
        kprintln!("[npk] intel-nic: WARNING: no link");
    }
}

/// TCPv4 segmentation offload — I225/I226 only.
pub fn tso_capable() -> bool { IS_IGC.load(Ordering::Acquire) }

pub fn send_tso(frame: &[u8], mss: u16, l4_off: usize, hdr_len: usize) -> Result<(), NetError> {
    if !tso_capable() { return Err(NetError::NotInitialized); }
    igc::send_tso(frame, mss, l4_off, hdr_len)
}

/// LAPIC vector of the RX interrupt; 0 for the polled e1000 path.
pub fn rx_irq_vector() -> u8 {
    if IS_IGC.load(Ordering::Acquire) { igc::rx_irq_vector() } else { 0 }
}

pub fn is_available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

/// Live link (carrier) state: reads STATUS.LU right now, so a pulled/plugged
/// cable is reflected immediately. Cheap MMIO read — but call from Core 0 (it
/// takes the DEVICE lock). False if the NIC isn't present.
pub fn link_up() -> bool {
    if !AVAILABLE.load(Ordering::Relaxed) { return false; }
    if IS_IGC.load(Ordering::Acquire) { return igc::link_up(); }
    match DEVICE.lock().as_ref() {
        Some(d) => d.mmio.r32(STATUS) & STATUS_LU != 0,
        None => false,
    }
}

pub fn mac() -> Option<[u8; 6]> {
    DEVICE.lock().as_ref().map(|d| d.mac_addr)
}

pub fn send(frame: &[u8]) -> Result<(), NetError> {
    if frame.len() > MTU { return Err(NetError::FrameTooLarge); }
    if IS_IGC.load(Ordering::Acquire) {
        let r = igc::send(frame);
        if r.is_ok() { TX_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed); }
        return r;
    }

    let mut lock = DEVICE.lock();
    let dev = lock.as_mut().ok_or(NetError::NotInitialized)?;

    let i = dev.tx_cur;
    let buf_off = (i * RX_BUF_SIZE) as u64; // 2KB aligned
    dev.tx_bufs.copy_in(buf_off, frame);

    let txd = &dev.tx_descs;
    let desc = (i * 16) as u64;
    for _ in 0..1_000_000u32 {
        let status = txd.r8(desc + TXD_STATUS);
        let cmd = txd.r8(desc + TXD_CMD);
        if status & TXD_STAT_DD != 0 || cmd == 0 { break; }
        core::hint::spin_loop();
    }
    txd.w64(desc + TXD_ADDR, dev.tx_bufs.phys() + buf_off);
    txd.w16(desc + TXD_LENGTH, frame.len() as u16);
    txd.w8(desc + TXD_CSO, 0);
    txd.w8(desc + TXD_CSS, 0);
    txd.w16(desc + TXD_SPECIAL, 0);
    txd.w8(desc + TXD_STATUS, 0);
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    txd.w8(desc + TXD_CMD, TXD_CMD_EOP | TXD_CMD_IFCS | TXD_CMD_RS);

    dev.tx_cur = (i + 1) % NUM_TX_DESC;
    // Write barrier: ensure descriptor is visible to NIC before tail bump
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    dev.mmio.w32(dev.regs.tdt, dev.tx_cur as u32);

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
    if IS_IGC.load(Ordering::Acquire) { return igc::recv(buf); }
    let mut lock = DEVICE.lock();
    let dev = lock.as_mut()?;

    let i = dev.rx_cur;
    let buf_off = (i * RX_BUF_SIZE) as u64;

    let rxd = &dev.rx_descs;
    let desc = (i * 16) as u64;
    let status = rxd.r8(desc + RXD_STATUS);
    if status & RXD_STAT_DD == 0 { return None; }

    let len = rxd.r16(desc + RXD_LENGTH) as usize;
    let len = len.min(MTU);

    dev.rx_bufs.copy_out(buf_off, &mut buf[..len]);

    rxd.w8(desc + RXD_STATUS, 0);
    rxd.w16(desc + RXD_LENGTH, 0);
    rxd.w8(desc + RXD_ERRORS, 0);

    let old_cur = dev.rx_cur;
    dev.rx_cur = (i + 1) % NUM_RX_DESC;
    dev.mmio.w32(dev.regs.rdt, old_cur as u32);

    Some(len)
}

/// Read MAC address from EEPROM (for NICs that don't expose it via RAL/RAH).
fn read_mac_eeprom(mmio: &Mmio) -> [u8; 6] {
    let mut mac = [0u8; 6];
    for i in 0..3u32 {
        mmio.w32(EERD, (i << 8) | 1); // Start read at address i
        // Wait for done
        for _ in 0..10_000u32 {
            let val = mmio.r32(EERD);
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
