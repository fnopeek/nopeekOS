//! Host-function bindings for the nopeekOS WASM Driver ABI.
//! Subset needed by the HDA driver: serial log, PCI, MMIO, DMA, sleep, fence.

use core::sync::atomic::{AtomicBool, Ordering};

pub fn audio_poll_mix(buf: &mut [u8]) -> usize {
    let n = npk_sys::audio_poll_mix(buf);
    if n > 0 { n as usize } else { 0 }
}

/// Diagnostic log line (`dmesg`; on screen only with `bootlog verbose`).
/// The host adds the newline.
pub fn log(s: &str) {
    let s = s.strip_suffix('\n').unwrap_or(s);
    npk_sys::log_serial(s.as_bytes());
}

// ── Diagnostic lines: built in, silent in normal operation ───────────
//
// The codec topology and the per-second reports (LPIB/wpos/SDCTL) say
// whether the DMA runs at all. Queried once at start;
// `set log.drivers 1` turns them on.
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Once at start: does the user want the diagnostic log?
pub fn log_init() {
    VERBOSE.store(npk_sys::sys_info(50) == 1, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}


pub fn pci_bind_class(class: u8, subclass: u8) -> i32 {
    npk_sys::pci_bind_class(class as i32, subclass as i32)
}
/// Bind the `index`-th controller of this class. A machine usually has two
/// HD Audio controllers (GPU HDMI and chipset), in arbitrary PCI order.
pub fn pci_bind_class_n(class: u8, subclass: u8, index: u32) -> i32 {
    npk_sys::pci_bind_class_n(class as i32, subclass as i32, index as i32)
}
pub fn pci_enable_bus_master() -> i32 {
    npk_sys::pci_enable_bus_master()
}
pub fn pci_read_config(offset: u8) -> u32 {
    npk_sys::pci_read_config(offset as i32) as u32
}
pub fn pci_write_config(offset: u8, value: u32) {
    npk_sys::pci_write_config(offset as i32, value as i32);
}

pub fn mmio_map_bar(bar: u8, pages: u16) -> i32 {
    npk_sys::mmio_map_bar(bar as i32, pages as i32)
}
pub fn mmio_r16(h: i32, off: u32) -> u16 {
    npk_sys::mmio_read16(h, off as i32) as u16
}
pub fn mmio_w8(h: i32, off: u32, val: u8) {
    npk_sys::mmio_write8(h, off as i32, val as i32);
}
pub fn mmio_w16(h: i32, off: u32, val: u16) {
    npk_sys::mmio_write16(h, off as i32, val as i32);
}
pub fn mmio_r32(h: i32, off: u32) -> u32 {
    npk_sys::mmio_read32(h, off as i32) as u32
}
pub fn mmio_w32(h: i32, off: u32, val: u32) {
    npk_sys::mmio_write32(h, off as i32, val as i32);
}

pub fn dma_alloc(pages: u16) -> i32 {
    npk_sys::dma_alloc(pages as i32)
}
pub fn dma_phys(h: i32) -> u64 {
    npk_sys::dma_phys_addr(h) as u64
}
pub fn dma_write(h: i32, off: u32, data: &[u8]) -> i32 {
    npk_sys::dma_write(h, off as i32, data)
}

pub fn fence() {
    npk_sys::memory_fence();
}
pub fn sleep_ms(ms: u32) {
    npk_sys::sleep(ms as i32);
}

/// `npk_wait` bit: the bound device's IRQ fired.
pub const WAIT_IRQ: i32 = 2;

/// Register the bound controller's MSI; returns the vector or -1.
pub fn irq_register() -> i32 {
    npk_sys::irq_register(0)
}

/// Park until the IRQ fires or `timeout_ms` passes.
pub fn wait_irq(timeout_ms: u32) -> i32 {
    npk_sys::wait(WAIT_IRQ, timeout_ms as i32)
}

/// Read a dword back from the DMA buffer.
///
/// Diagnostic: checks that `dma_write` lands in the memory the device
/// reads.
pub fn dma_read32(h: i32, off: u32) -> u32 {
    npk_sys::dma_read32(h, off as i32) as u32
}
