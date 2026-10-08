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
pub mod blk_image;
pub mod virtio_blk_pci;
pub mod virtio_console_pci;
pub mod virtio_gpu_pci;
pub mod virtio_input_pci;
pub mod virtio_input_keymap;
pub mod virtio_net_dev;
pub mod virtio_9p_pci;
pub mod virtio_snd_pci;
pub mod virtqueue;

pub use pci_bus::{handle_pci_io, PciBus, PCI_CONFIG_ADDR, PCI_CONFIG_DATA_END, PCI_CONFIG_DATA_START};

/// A virtio-pci device behind one MMIO BAR whose trapped accesses the vCPU
/// exit handlers decode and forward, and whose queue notifies they serve
/// on the spot (raising its line when a queue advanced).
pub trait MmioDevice {
    fn bar0_base(&self) -> u64;
    fn mmio_read(&mut self, off: u32, width: u8) -> u64;
    fn mmio_write(&mut self, off: u32, width: u8, value: u64);
    fn take_pending_kick(&mut self) -> Option<u16>;
    fn service_queues(&mut self, queue: u16, mem: &guest_mem::GuestMem) -> bool;
    fn irq_line(&self) -> u8;
}

impl MmioDevice for virtio_console_pci::VirtioConsole {
    fn bar0_base(&self) -> u64 { self.bar0_base() }
    fn mmio_read(&mut self, off: u32, width: u8) -> u64 { self.mmio_read(off, width) }
    fn mmio_write(&mut self, off: u32, width: u8, value: u64) { self.mmio_write(off, width, value) }
    fn take_pending_kick(&mut self) -> Option<u16> { self.take_pending_kick() }
    fn service_queues(&mut self, queue: u16, mem: &guest_mem::GuestMem) -> bool {
        self.service_queues(queue, mem)
    }
    fn irq_line(&self) -> u8 { self.irq_line() }
}
