//! PCI bus emulation for the Linux MicroVM guest.
//!
//! Type-1 config-space access via legacy PIO ports 0xCF8 (address) and
//! 0xCFC..0xCFF (data window with byte-lane select). No ACPI, no MMCFG —
//! the guest boots with `acpi=off` so it falls back to the legacy path.
//!
//! Layout:
//!
//! ```text
//!   bus 0, slot 0, func 0  →  Intel i440FX-style host bridge (8086:1237)
//!   bus 0, slot 1, func 0  →  virtio-blk-pci device (1AF4:1042)
//!   everything else        →  vendor=0xFFFF (no device)
//! ```
//!
//! Slot 1's full state — including BAR sizing handshake and the modern
//! virtio capability list — lives in `VirtioBlk`. This module just
//! routes config-space dwords to/from there.

use super::virtio_blk_pci::VirtioBlk;

pub const PCI_CONFIG_ADDR: u16 = 0xCF8;
pub const PCI_CONFIG_DATA_START: u16 = 0xCFC;
pub const PCI_CONFIG_DATA_END: u16 = 0xCFF;

const NO_DEVICE: u32 = 0xFFFF_FFFF;

/// Per-VM PCI bus emulation state.
pub struct PciBus {
    config_addr: u32,
    pub virtio_blk: VirtioBlk,
}

impl PciBus {
    pub fn new() -> Self {
        Self {
            config_addr: 0,
            virtio_blk: VirtioBlk::new(),
        }
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
    if port == PCI_CONFIG_ADDR {
        if !dir_in {
            if size == 4 {
                bus.config_addr = val_out;
            }
            return None;
        }
        return Some(bus.config_addr as u64);
    }

    let lane_off = (port - PCI_CONFIG_DATA_START) as u8; // 0..=3

    let enable = bus.config_addr & 0x8000_0000 != 0;
    if !enable {
        return if dir_in { Some(NO_DEVICE as u64) } else { None };
    }

    let bus_num = ((bus.config_addr >> 16) & 0xFF) as u8;
    let slot = ((bus.config_addr >> 11) & 0x1F) as u8;
    let func = ((bus.config_addr >> 8) & 0x07) as u8;
    let reg_dword = (bus.config_addr & 0xFC) as u8;

    if dir_in {
        let dword = read_pci_dword(bus, bus_num, slot, func, reg_dword);
        let shift = lane_off * 8;
        let mask: u64 = match size {
            1 => 0xFF,
            2 => 0xFFFF,
            4 => 0xFFFF_FFFF,
            _ => 0xFF,
        };
        Some(((dword >> shift) as u64) & mask)
    } else {
        // Linux's PCI enumerator does dword-aligned 4-byte writes for
        // BAR sizing and command/status updates. Smaller writes fall
        // through unchanged.
        if size == 4 && lane_off == 0 {
            write_pci_dword(bus, bus_num, slot, func, reg_dword, val_out);
        }
        None
    }
}

fn read_pci_dword(bus: &PciBus, bus_num: u8, slot: u8, func: u8, reg: u8) -> u32 {
    if bus_num != 0 || func != 0 {
        return NO_DEVICE;
    }
    match slot {
        0 => host_bridge_config(reg),
        1 => bus.virtio_blk.pci_read_dword(reg),
        _ => NO_DEVICE,
    }
}

fn write_pci_dword(bus: &mut PciBus, bus_num: u8, slot: u8, func: u8, reg: u8, val: u32) {
    if bus_num != 0 || func != 0 {
        return;
    }
    if slot == 1 {
        bus.virtio_blk.pci_write_dword(reg, val);
    }
}

/// Intel i440FX host bridge — minimum descriptor that satisfies the
/// Linux PCI enumerator. Class 06_00_00 = host bridge.
fn host_bridge_config(reg: u8) -> u32 {
    match reg {
        0x00 => (0x1237 << 16) | 0x8086,
        0x04 => (0x0000 << 16) | 0x0007,
        0x08 => (0x06_00_00 << 8) | 0x02,
        0x0C => 0x0000_0000,
        0x10..=0x24 => 0,
        _ => 0,
    }
}
