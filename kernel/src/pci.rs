//! PCI Configuration Space Access
//!
//! Standard x86 PCI bus via I/O ports 0xCF8 (address) / 0xCFC (data).
//! Scans for VirtIO and other PCI devices.

use crate::serial::{outl, inl};
use crate::kprintln;

const CONFIG_ADDR: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

#[derive(Debug, Clone, Copy)]
pub struct PciAddr {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl PciAddr {
    fn address(self, offset: u8) -> u32 {
        0x8000_0000
            | ((self.bus as u32) << 16)
            | ((self.device as u32) << 11)
            | ((self.function as u32) << 8)
            | ((offset & 0xFC) as u32)
    }
}

pub fn read32(addr: PciAddr, offset: u8) -> u32 {
    // SAFETY: PCI config space port I/O, standard x86 mechanism
    unsafe {
        outl(CONFIG_ADDR, addr.address(offset));
        inl(CONFIG_DATA)
    }
}

pub fn write32(addr: PciAddr, offset: u8, value: u32) {
    // SAFETY: PCI config space port I/O
    unsafe {
        outl(CONFIG_ADDR, addr.address(offset));
        outl(CONFIG_DATA, value);
    }
}

pub fn read16(addr: PciAddr, offset: u8) -> u16 {
    let val = read32(addr, offset);
    ((val >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

pub fn read8(addr: PciAddr, offset: u8) -> u8 {
    let val = read32(addr, offset);
    ((val >> ((offset & 3) * 8)) & 0xFF) as u8
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct PciDevice {
    pub addr: PciAddr,
    pub vendor_id: u16,
    pub device_id: u16,
    pub bar0: u32,
    pub irq_line: u8,
}

/// Find first PCI device matching vendor + device ID
pub fn find_device(vendor: u16, device: u16) -> Option<PciDevice> {
    for bus in 0u16..=255 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let addr = PciAddr { bus: bus as u8, device: dev, function: func };
                let id = read32(addr, 0x00);
                if id == 0xFFFF_FFFF || id == 0 {
                    if func == 0 { break; }
                    continue;
                }

                let vid = (id & 0xFFFF) as u16;
                let did = ((id >> 16) & 0xFFFF) as u16;

                if vid == vendor && did == device {
                    return Some(PciDevice {
                        addr,
                        vendor_id: vid,
                        device_id: did,
                        bar0: read32(addr, 0x10),
                        irq_line: read8(addr, 0x3C),
                    });
                }

                if func == 0 && read8(addr, 0x0E) & 0x80 == 0 {
                    break;
                }
            }
        }
    }
    None
}

/// Enable PCI bus mastering (required for DMA)
pub fn enable_bus_master(addr: PciAddr) {
    let cmd = read32(addr, 0x04);
    write32(addr, 0x04, cmd | 0x04);
}

/// Check if MSI-X is enabled (not just present) — affects legacy VirtIO config offset
pub fn msix_enabled(addr: PciAddr) -> bool {
    let status = read16(addr, 0x06);
    if status & (1 << 4) == 0 { return false; }
    let mut ptr = read8(addr, 0x34) & 0xFC;
    while ptr != 0 {
        if read8(addr, ptr) == 0x11 {
            let msg_ctrl = read16(addr, ptr + 2);
            return msg_ctrl & (1 << 15) != 0;
        }
        ptr = read8(addr, ptr + 1) & 0xFC;
    }
    false
}

/// Scan PCI bus, print all devices, return count
pub fn scan() -> u16 {
    let mut count = 0u16;
    for bus in 0u16..=255 {
        for dev in 0u8..32 {
            let addr = PciAddr { bus: bus as u8, device: dev, function: 0 };
            let id = read32(addr, 0x00);
            if id == 0xFFFF_FFFF || id == 0 { continue; }

            let vid = (id & 0xFFFF) as u16;
            let did = ((id >> 16) & 0xFFFF) as u16;
            let class = read32(addr, 0x08);
            let cls = ((class >> 24) & 0xFF) as u8;
            let sub = ((class >> 16) & 0xFF) as u8;

            kprintln!("[npk]   {:02x}:{:02x}.0  {:04x}:{:04x}  class {:02x}.{:02x}",
                bus, dev, vid, did, cls, sub);
            count += 1;
        }
    }
    count
}
