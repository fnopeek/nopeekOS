//! Hardware drivers
//!
//! All hardware-facing code: PCI, block devices, network, USB, serial, GPU HAL.

pub mod pci;
pub mod nvme;
pub mod virtio_blk;
pub mod virtio_net;
pub mod intel_nic;
pub mod xhci;
pub mod keyboard;
pub mod framebuffer;
pub mod serial;
pub mod rtc;
pub mod blkdev;
pub mod netdev;
pub mod acpi;
