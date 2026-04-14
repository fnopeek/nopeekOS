//! Host function bindings for nopeekOS WASM Driver ABI

unsafe extern "C" {
    // Output
    fn npk_print(ptr: i32, len: i32);
    fn npk_log(ptr: i32, len: i32);

    // PCI
    fn npk_pci_bind(vendor: i32, device: i32) -> i32;
    fn npk_pci_bind_class(class: i32, subclass: i32) -> i32;
    fn npk_pci_enable_bus_master() -> i32;
    fn npk_pci_read_config(offset: i32) -> i32;
    fn npk_pci_write_config(offset: i32, value: i32) -> i32;

    // MMIO
    fn npk_mmio_map_bar(bar_idx: i32, pages: i32) -> i32;
    fn npk_mmio_read32(handle: i32, offset: i32) -> i32;
    fn npk_mmio_write32(handle: i32, offset: i32, value: i32) -> i32;
    fn npk_mmio_read64(handle: i32, offset: i32) -> i64;
    fn npk_mmio_write64(handle: i32, offset: i32, value: i64) -> i32;

    // DMA
    fn npk_dma_alloc(pages: i32) -> i32;
    fn npk_dma_phys_addr(handle: i32) -> i64;
    fn npk_dma_read(handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32;
    fn npk_dma_write(handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32;
    fn npk_dma_read32(handle: i32, offset: i32) -> i32;
    fn npk_dma_write32(handle: i32, offset: i32, value: i32) -> i32;

    // Misc
    fn npk_memory_fence() -> i32;
    fn npk_sleep(ms: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_netdev_register(mac_ptr: i32) -> i32;
    fn npk_input_wait(timeout_ms: i32) -> i32;
}

// ── Safe wrappers ────────────────────────────────────────────────

pub fn print(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32); }
}

pub fn log(s: &str) {
    unsafe { npk_log(s.as_ptr() as i32, s.len() as i32); }
}

pub fn pci_bind(vendor: u16, device: u16) -> i32 {
    unsafe { npk_pci_bind(vendor as i32, device as i32) }
}

pub fn pci_enable_bus_master() -> i32 {
    unsafe { npk_pci_enable_bus_master() }
}

pub fn pci_read_config(offset: u8) -> u32 {
    unsafe { npk_pci_read_config(offset as i32) as u32 }
}

pub fn mmio_map_bar(bar: u8, pages: u16) -> i32 {
    unsafe { npk_mmio_map_bar(bar as i32, pages as i32) }
}

pub fn mmio_r32(handle: i32, offset: u32) -> u32 {
    unsafe { npk_mmio_read32(handle, offset as i32) as u32 }
}

pub fn mmio_w32(handle: i32, offset: u32, val: u32) {
    unsafe { npk_mmio_write32(handle, offset as i32, val as i32); }
}

/// Write a 16-bit value to an MMIO register using aligned read-modify-write.
/// Offset must be 2-byte aligned (but not necessarily 4-byte aligned).
pub fn mmio_w16(handle: i32, offset: u32, val: u16) {
    let aligned = offset & !0x3;
    let shift = (offset & 0x2) * 8; // 0 or 16
    let mut word = mmio_r32(handle, aligned);
    word &= !(0xFFFF << shift);
    word |= (val as u32) << shift;
    mmio_w32(handle, aligned, word);
}

pub fn mmio_r64(handle: i32, offset: u32) -> u64 {
    unsafe { npk_mmio_read64(handle, offset as i32) as u64 }
}

pub fn dma_alloc(pages: u16) -> i32 {
    unsafe { npk_dma_alloc(pages as i32) }
}

pub fn dma_phys(handle: i32) -> u64 {
    unsafe { npk_dma_phys_addr(handle) as u64 }
}

pub fn dma_write_buf(handle: i32, offset: u32, data: &[u8]) -> i32 {
    unsafe { npk_dma_write(handle, offset as i32, data.as_ptr() as i32, data.len() as i32) }
}

pub fn dma_read_buf(handle: i32, offset: u32, buf: &mut [u8]) -> i32 {
    unsafe { npk_dma_read(handle, offset as i32, buf.as_mut_ptr() as i32, buf.len() as i32) }
}

pub fn dma_r32(handle: i32, offset: u32) -> u32 {
    unsafe { npk_dma_read32(handle, offset as i32) as u32 }
}

pub fn dma_w32(handle: i32, offset: u32, val: u32) {
    unsafe { npk_dma_write32(handle, offset as i32, val as i32); }
}

pub fn fence() {
    unsafe { npk_memory_fence(); }
}

pub fn sleep_ms(ms: u32) {
    unsafe { npk_sleep(ms as i32); }
}

pub fn input_wait(timeout_ms: u32) -> i32 {
    unsafe { npk_input_wait(timeout_ms as i32) }
}

// ── Hex output helpers ───────────────────────────────────────────

const HEX: &[u8; 16] = b"0123456789abcdef";

pub fn print_hex32(val: u32) {
    let mut buf = [0u8; 8];
    for i in 0..8 {
        buf[7 - i] = HEX[((val >> (i * 4)) & 0xF) as usize];
    }
    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    print(s);
}

pub fn print_hex16(val: u16) {
    let mut buf = [0u8; 4];
    for i in 0..4 {
        buf[3 - i] = HEX[((val >> (i * 4)) & 0xF) as usize];
    }
    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    print(s);
}

pub fn log_reg(name: &str, val: u32) {
    print("  ");
    print(name);
    print(": 0x");
    print_hex32(val);
    print("\n");
}
