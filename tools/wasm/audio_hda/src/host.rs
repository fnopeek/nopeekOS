//! Host-function bindings for the nopeekOS WASM Driver ABI.
//! Subset needed by the HDA driver: serial log, PCI, MMIO, DMA, sleep, fence.

unsafe extern "C" {
    fn npk_print(ptr: i32, len: i32);

    // PCI
    fn npk_pci_bind(vendor: i32, device: i32) -> i32;
    fn npk_pci_bind_class(class: i32, subclass: i32) -> i32;
    fn npk_pci_enable_bus_master() -> i32;
    fn npk_pci_read_config(offset: i32) -> i32;
    fn npk_pci_write_config(offset: i32, value: i32) -> i32;

    // MMIO (handle-based; no pointer deref)
    fn npk_mmio_map_bar(bar_idx: i32, pages: i32) -> i32;
    fn npk_mmio_read16(handle: i32, offset: i32) -> i32;
    fn npk_mmio_write16(handle: i32, offset: i32, value: i32) -> i32;
    fn npk_mmio_read32(handle: i32, offset: i32) -> i32;
    fn npk_mmio_write32(handle: i32, offset: i32, value: i32) -> i32;

    // DMA
    fn npk_dma_alloc(pages: i32) -> i32;
    fn npk_dma_phys_addr(handle: i32) -> i64;
    fn npk_dma_write(handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32;

    fn npk_memory_fence() -> i32;
    fn npk_sleep(ms: i32) -> i32;
}

pub fn log(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32) };
}

pub fn pci_bind(vendor: u16, device: u16) -> i32 {
    unsafe { npk_pci_bind(vendor as i32, device as i32) }
}
pub fn pci_bind_class(class: u8, subclass: u8) -> i32 {
    unsafe { npk_pci_bind_class(class as i32, subclass as i32) }
}
pub fn pci_enable_bus_master() -> i32 {
    unsafe { npk_pci_enable_bus_master() }
}
pub fn pci_read_config(offset: u8) -> u32 {
    unsafe { npk_pci_read_config(offset as i32) as u32 }
}
pub fn pci_write_config(offset: u8, value: u32) {
    unsafe { npk_pci_write_config(offset as i32, value as i32) };
}

pub fn mmio_map_bar(bar: u8, pages: u16) -> i32 {
    unsafe { npk_mmio_map_bar(bar as i32, pages as i32) }
}
pub fn mmio_r16(h: i32, off: u32) -> u16 {
    unsafe { npk_mmio_read16(h, off as i32) as u16 }
}
pub fn mmio_w16(h: i32, off: u32, val: u16) {
    unsafe { npk_mmio_write16(h, off as i32, val as i32) };
}
pub fn mmio_r32(h: i32, off: u32) -> u32 {
    unsafe { npk_mmio_read32(h, off as i32) as u32 }
}
pub fn mmio_w32(h: i32, off: u32, val: u32) {
    unsafe { npk_mmio_write32(h, off as i32, val as i32) };
}

pub fn dma_alloc(pages: u16) -> i32 {
    unsafe { npk_dma_alloc(pages as i32) }
}
pub fn dma_phys(h: i32) -> u64 {
    unsafe { npk_dma_phys_addr(h) as u64 }
}
pub fn dma_write(h: i32, off: u32, data: &[u8]) -> i32 {
    unsafe { npk_dma_write(h, off as i32, data.as_ptr() as i32, data.len() as i32) }
}

pub fn fence() {
    unsafe { npk_memory_fence() };
}
pub fn sleep_ms(ms: u32) {
    unsafe { npk_sleep(ms as i32) };
}
