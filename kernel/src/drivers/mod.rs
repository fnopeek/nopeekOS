//! Hardware drivers
//!
//! All hardware-facing code: PCI, block devices, network, USB, serial, GPU HAL.

pub mod pci;
pub mod nvme;
pub mod virtio_blk;
pub mod virtio_net;
pub mod intel_nic;
pub mod igc;
pub mod rtl8153;
pub mod xhci;
pub mod usb_desc;
pub mod usb_storage;
pub mod sdhci;
pub mod mmc_sd;
pub mod keyboard;
pub mod font8x16;
pub mod framebuffer;
pub mod serial;
pub mod uart16550;
pub use uart16550 as uart;
pub mod rtc;
pub mod blkdev;
pub mod netdev;
pub mod wifi;
pub mod report;
pub mod acpi;
pub mod acpi_processor;
pub mod vtd;
pub mod amd_vi;
#[allow(dead_code)]
pub mod amd_vi_regs;
pub mod iommu;
pub mod smbus;
pub mod battery;
pub mod ec;
pub mod sci;
