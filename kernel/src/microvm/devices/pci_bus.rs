//! PCI bus emulation for the Linux MicroVM guest.
//!
//! Type-1 config-space access via legacy PIO ports 0xCF8 (address) and
//! 0xCFC..0xCFF (data window with byte-lane select). No ACPI, no MMCFG —
//! the guest boots with `acpi=off` so it falls back to the legacy path.
//!
//! Layout we present:
//!
//! ```text
//!   bus 0, slot 0, func 0  →  Intel i440FX-style host bridge (8086:1237)
//!   bus 0, slot 1, func 0  →  virtio-blk-pci device (1AF4:1042)
//!   everything else        →  vendor=0xFFFF (no device)
//! ```
//!
//! Writes are accepted but mostly ignored — BARs are returned as fixed
//! values (which Linux's PCI enumerator will accept since it treats
//! them as already-assigned). When we wire up MMIO BAR traps, the BAR
//! values become the GPA range we trap.

use super::virtio_blk_pci;

pub const PCI_CONFIG_ADDR: u16 = 0xCF8;
pub const PCI_CONFIG_DATA_START: u16 = 0xCFC;
pub const PCI_CONFIG_DATA_END: u16 = 0xCFF;

const NO_DEVICE: u32 = 0xFFFF_FFFF;

/// Per-VM PCI bus emulation state. Holds the latched 0xCF8 address.
pub struct PciBus {
    config_addr: u32,
}

impl PciBus {
    pub const fn new() -> Self {
        Self { config_addr: 0 }
    }
}

/// Dispatch a guest PIO access targeted at a PCI config-space port.
/// Returns `Some(value)` for IN reads, `None` for OUT writes (caller
/// leaves guest RAX alone in that case).
///
/// Caller must already have decided this port is in our PCI range
/// (`PCI_CONFIG_ADDR` or `PCI_CONFIG_DATA_START..=PCI_CONFIG_DATA_END`).
pub fn handle_pci_io(
    bus: &mut PciBus,
    port: u16,
    dir_in: bool,
    size: u8,
    val_out: u32,
) -> Option<u64> {
    // 0xCF8 — config address latch. 32-bit per spec; OS writes always
    // size 4. Smaller accesses are technically defined but Linux
    // doesn't issue them.
    if port == PCI_CONFIG_ADDR {
        if !dir_in {
            if size == 4 {
                bus.config_addr = val_out;
            }
            return None;
        }
        return Some(bus.config_addr as u64);
    }

    // 0xCFC..0xCFF — data window. The low two bits of the port select
    // which byte lane within the latched dword the access targets.
    let lane_off = (port - PCI_CONFIG_DATA_START) as u8; // 0..=3

    let enable = bus.config_addr & 0x8000_0000 != 0;
    if !enable {
        // 0xCF8 wasn't enabled — guest is doing a probe loop or
        // legacy nonsense. Real chipsets return all-ones here.
        return if dir_in { Some(NO_DEVICE as u64) } else { None };
    }

    let bus_num = ((bus.config_addr >> 16) & 0xFF) as u8;
    let slot = ((bus.config_addr >> 11) & 0x1F) as u8;
    let func = ((bus.config_addr >> 8) & 0x07) as u8;
    let reg_dword = (bus.config_addr & 0xFC) as u8; // dword-aligned offset

    if dir_in {
        let dword = read_pci_dword(bus_num, slot, func, reg_dword);
        let shift = lane_off * 8;
        let mask: u64 = match size {
            1 => 0xFF,
            2 => 0xFFFF,
            4 => 0xFFFF_FFFF,
            _ => 0xFF,
        };
        Some(((dword >> shift) as u64) & mask)
    } else {
        // OUT writes — Linux's PCI enumerator writes BARs (to size them)
        // and command/status registers. We don't honour BAR resizing yet;
        // BARs return fixed values from `read_pci_dword`. Drop silently.
        let _ = (bus_num, slot, func, reg_dword, lane_off, val_out);
        None
    }
}

fn read_pci_dword(bus: u8, slot: u8, func: u8, reg: u8) -> u32 {
    if bus != 0 || func != 0 {
        return NO_DEVICE;
    }
    match slot {
        0 => host_bridge_config(reg),
        1 => virtio_blk_pci::config_dword(reg),
        _ => NO_DEVICE,
    }
}

/// Intel i440FX host bridge — minimum descriptor that satisfies the
/// Linux PCI enumerator. Class 06_00_00 = host bridge.
fn host_bridge_config(reg: u8) -> u32 {
    match reg {
        // Vendor + Device: Intel 8086, i440FX 1237
        0x00 => (0x1237 << 16) | 0x8086,
        // Status (upper 16) | Command (lower 16). Command bus-master + IO.
        0x04 => (0x0000 << 16) | 0x0007,
        // Class code (24 bits) | revision (8 bits)
        0x08 => (0x06_00_00 << 8) | 0x02,
        // BIST | Header type | Latency | Cache line. Header type 0x00 = std.
        0x0C => 0x0000_0000,
        // BARs all zero (host bridge has no BARs).
        0x10..=0x24 => 0,
        // Subsystem vendor + ID, capabilities pointer, etc — zero is fine.
        _ => 0,
    }
}
