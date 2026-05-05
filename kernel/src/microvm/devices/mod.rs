//! MicroVM virtual devices.
//!
//! Pure-software emulation of devices the Linux guest expects to see.
//! Vendor-neutral — both VMX and SVM exit handlers thread the same
//! state through.
//!
//! Phase 12.2 starts here with PCI config-space + virtio-blk discovery.
//! Real I/O paths (virtqueue parsing, MMIO BAR traps, IRQ injection)
//! land in follow-up commits.

pub mod pci_bus;
pub mod virtio_blk_pci;

pub use pci_bus::{handle_pci_io, PciBus, PCI_CONFIG_ADDR, PCI_CONFIG_DATA_END, PCI_CONFIG_DATA_START};
