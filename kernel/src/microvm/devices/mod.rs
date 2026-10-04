//! MicroVM virtual devices.
//!
//! Pure-software emulation of devices the Linux guest expects to see.
//! Vendor-neutral — both VMX and SVM exit handlers thread the same
//! state through.
//!
//! PCI config space, BAR sizing, the modern virtio capability chain, MMIO
//! BAR emulation and a minimal x86 MOV decoder for SVM-side MMIO traps.

pub mod guest_fetch;
pub mod guest_mem;
pub mod ioapic;
pub mod insn_decoder;
pub mod nat;
pub mod net_backend;
pub mod gpu_backend;
pub mod net_dataplane;
pub mod p9_async;
pub mod pci_bus;
pub mod pic8259;
pub mod pit8253;
pub mod virtio_blk_pci;
pub mod virtio_gpu_pci;
pub mod virtio_input_pci;
pub mod virtio_input_keymap;
pub mod virtio_net_dev;
pub mod virtio_9p_pci;
pub mod virtio_snd_pci;
pub mod virtqueue;

pub use pci_bus::{handle_pci_io, PciBus, PCI_CONFIG_ADDR, PCI_CONFIG_DATA_END, PCI_CONFIG_DATA_START};
