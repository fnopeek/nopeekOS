//! virtio-blk PCI device — config-space descriptor (Phase 12.2 step 1).
//!
//! Modern virtio (transitional 1.0+): vendor 0x1AF4, device 0x1042.
//! Class 0x01_80_00 = mass storage / other / PI 0.
//!
//! BAR layout (will become real once MMIO BAR traps land):
//!   BAR0 = 64-bit MMIO @ 0xFE00_0000, 16 KB. Holds the modern virtio
//!   capability layout (Common Cfg / Notify / ISR / Device Cfg).
//!
//! For this commit Linux only sees the device exists — it will probe
//! BAR0, fail to talk to it (no MMIO trap yet), and shelve the device.
//! That's enough to validate the PCI bus emulation. Real driver bring-
//! up follows in 12.2.2 (MMIO traps + virtqueue).

const VIRTIO_VENDOR: u32 = 0x1AF4;
const VIRTIO_BLK_DEVICE: u32 = 0x1042;

/// MMIO BAR0 — chosen above the standard MMIO holes and aligned 16 KB.
/// Will become the address Linux talks to once we trap it.
pub const VIRTIO_BLK_BAR0: u64 = 0xFE00_0000;
#[allow(dead_code)] // wired in 12.2.2 when MMIO BAR traps land
pub const VIRTIO_BLK_BAR0_SIZE: u64 = 0x4000;

/// Read a config-space dword. `reg` is the dword-aligned offset.
pub fn config_dword(reg: u8) -> u32 {
    match reg {
        // Vendor + Device IDs
        0x00 => (VIRTIO_BLK_DEVICE << 16) | VIRTIO_VENDOR,
        // Status | Command. Command: I/O + Memory + bus-master enabled.
        // Status: capabilities-list bit (0x10).
        0x04 => (0x0010 << 16) | 0x0007,
        // Class code 01_80_00 (mass storage, other) | revision 0x01.
        0x08 => (0x01_80_00 << 8) | 0x01,
        // BIST | Header type 0x00 | Latency 0 | Cache line 0
        0x0C => 0x0000_0000,
        // BAR0 low — 64-bit MMIO, prefetchable=0, type=10 (64-bit). Bit 0=0 (mem).
        0x10 => (VIRTIO_BLK_BAR0 as u32) | 0b0100,
        // BAR0 high — upper 32 bits of 64-bit BAR.
        0x14 => (VIRTIO_BLK_BAR0 >> 32) as u32,
        // BAR2..BAR5 — unused. Returning zero tells Linux they're absent.
        0x18..=0x24 => 0,
        // Cardbus CIS pointer
        0x28 => 0,
        // Subsystem vendor (0x2C low) + Subsystem ID (0x2C high).
        // Subsys vendor 0x1AF4 (Red Hat / virtio), subsys device 0x0002 (block).
        0x2C => (0x0002 << 16) | 0x1AF4,
        // Expansion ROM base — none.
        0x30 => 0,
        // Capabilities pointer (low byte of 0x34). We point at 0x40 where
        // the modern virtio capability list begins; full caps come in 12.2.2.
        0x34 => 0x40,
        // Reserved.
        0x38 => 0,
        // Interrupt: line 11 (legacy INTA), pin 0x01 (INTA), min/max grant 0.
        // Linux records INTA but no controller is wired yet — IRQ delivery
        // comes in 12.2.2.
        0x3C => (0x00 << 24) | (0x00 << 16) | (0x01 << 8) | 0x0B,
        // Capability list anchor (placeholder — 12.2.2 fills with the
        // four virtio-modern capabilities).
        0x40 => 0,
        _ => 0,
    }
}
