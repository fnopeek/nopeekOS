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

/// Find first PCI device matching class + subclass
pub fn find_by_class(class: u8, subclass: u8) -> Option<PciDevice> {
    for bus in 0u16..=255 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let addr = PciAddr { bus: bus as u8, device: dev, function: func };
                let id = read32(addr, 0x00);
                if id == 0xFFFF_FFFF || id == 0 {
                    if func == 0 { break; }
                    continue;
                }

                let class_reg = read32(addr, 0x08);
                let cls = ((class_reg >> 24) & 0xFF) as u8;
                let sub = ((class_reg >> 16) & 0xFF) as u8;

                if cls == class && sub == subclass {
                    let vid = (id & 0xFFFF) as u16;
                    let did = ((id >> 16) & 0xFFFF) as u16;
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

/// Read 64-bit BAR (BAR0 + BAR1 for 64-bit MMIO devices like NVMe)
pub fn read_bar64(addr: PciAddr, bar_offset: u8) -> u64 {
    let low = read32(addr, bar_offset) as u64;
    let high = read32(addr, bar_offset + 4) as u64;
    // Clear type/prefetch bits from low word
    (high << 32) | (low & 0xFFFF_FFF0)
}

/// Enable PCI bus mastering (required for DMA)
pub fn enable_bus_master(addr: PciAddr) {
    let cmd = read32(addr, 0x04);
    write32(addr, 0x04, cmd | 0x04);
}

/// Assign an MMIO address to an unassigned BAR and configure the parent bridge.
/// `bar_offset` is the config space offset of the BAR (e.g. 0x18 for BAR2).
/// Returns the assigned physical address, or 0 on failure.
pub fn assign_bar_mmio(dev: PciAddr, bar_offset: u8) -> u64 {
    // ── Step 1: Probe BAR size ──────────────────────────────────
    let cmd = read32(dev, 0x04);
    write32(dev, 0x04, cmd & !0x06); // disable memory + bus master

    let saved_lo = read32(dev, bar_offset);
    let saved_hi = read32(dev, bar_offset + 4);
    let is_64bit = saved_lo & 0x04 != 0;

    write32(dev, bar_offset, 0xFFFF_FFFF);
    if is_64bit { write32(dev, bar_offset + 4, 0xFFFF_FFFF); }

    let size_lo = read32(dev, bar_offset);
    let size_hi = if is_64bit { read32(dev, bar_offset + 4) } else { 0 };

    let size_mask = if is_64bit {
        ((size_hi as u64) << 32) | ((size_lo as u64) & !0xF)
    } else {
        (size_lo as u64) & !0xF
    };
    let bar_size = (!size_mask).wrapping_add(1);

    if bar_size == 0 || bar_size > 0x0100_0000 {
        // Restore and bail
        write32(dev, bar_offset, saved_lo);
        if is_64bit { write32(dev, bar_offset + 4, saved_hi); }
        write32(dev, 0x04, cmd);
        return 0;
    }

    // ── Step 2: Pick address (simple bump allocator) ────────────
    // Use 0xFD000000 region — above typical TOLUD, below APIC/ECAM
    static NEXT_ADDR: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0xFD00_0000);

    let align = if bar_size < 0x1000 { 0x1000 } else { bar_size as u32 };
    let mut addr = NEXT_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    addr = (addr + align - 1) & !(align - 1); // align up
    NEXT_ADDR.store(addr + bar_size as u32, core::sync::atomic::Ordering::Relaxed);

    // ── Step 3: Write BAR ───────────────────────────────────────
    write32(dev, bar_offset, addr);
    if is_64bit { write32(dev, bar_offset + 4, 0); }

    // Re-enable memory space + bus master
    write32(dev, 0x04, cmd | 0x06);

    kprintln!("[npk] PCI BAR assigned: {:02x}:{:02x}.{} offset 0x{:02x} → {:#x} (size {:#x})",
        dev.bus, dev.device, dev.function, bar_offset, addr, bar_size);

    // ── Step 4: Configure parent bridge memory window ───────────
    if dev.bus > 0 {
        configure_bridge_window(dev.bus, addr, bar_size as u32);
    }

    addr as u64
}

/// Find the PCIe root port (bridge on bus 0) that forwards to `target_bus`
/// and set its Memory Base/Limit to include `addr..addr+size`.
fn configure_bridge_window(target_bus: u8, addr: u32, size: u32) {
    for dev in 0u8..32 {
        for func in 0u8..8 {
            let bridge = PciAddr { bus: 0, device: dev, function: func };
            let id = read32(bridge, 0x00);
            if id == 0xFFFF_FFFF || id == 0 {
                if func == 0 { break; }
                continue;
            }

            // Check: is this a PCI-PCI bridge? (header type = 0x01)
            let hdr = read8(bridge, 0x0E) & 0x7F;
            if hdr != 0x01 {
                if func == 0 && read8(bridge, 0x0E) & 0x80 == 0 { break; }
                continue;
            }

            // Check: does this bridge's secondary bus match?
            let sec_bus = read8(bridge, 0x19);
            if sec_bus != target_bus {
                if func == 0 && read8(bridge, 0x0E) & 0x80 == 0 { break; }
                continue;
            }

            // Found the bridge! Set Memory Base/Limit (offset 0x20)
            // Format: bits [15:4] = address [31:20], granularity = 1MB
            let base_val = ((addr >> 16) & 0xFFF0) as u16;
            let end = addr + size - 1;
            let limit_val = ((end >> 16) & 0xFFF0) as u16;
            let mem_window = ((limit_val as u32) << 16) | (base_val as u32);
            write32(bridge, 0x20, mem_window);

            // Enable memory forwarding on the bridge
            let bcmd = read32(bridge, 0x04);
            write32(bridge, 0x04, bcmd | 0x06);

            kprintln!("[npk] Bridge {:02x}:{:02x}.{} memory window: {:#x}-{:#x}",
                0, dev, func, addr, end);
            return;
        }
    }
    kprintln!("[npk] WARNING: no bridge found for bus {}", target_bus);
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
