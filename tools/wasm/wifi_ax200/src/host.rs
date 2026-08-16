//! Host function bindings for nopeekOS WASM Driver ABI

// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
#[link(wasm_import_module = "env")]
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
    fn npk_mmio_read16(handle: i32, offset: i32) -> i32;
    fn npk_mmio_write16(handle: i32, offset: i32, value: i32) -> i32;
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
    fn npk_ticks() -> i64;
    fn npk_now_us() -> i64;
    fn npk_driver_report(buf_ptr: i32, len: i32) -> i32;
    fn npk_fetch(name_ptr: i32, name_len: i32, buf_ptr: i32, buf_max: i32) -> i32;
    fn npk_netdev_register(mac_ptr: i32) -> i32;
    fn npk_input_wait(timeout_ms: i32) -> i32;

    // WiFi-class control channel (driver side — gated to bound drivers).
    fn npk_wifi_poll_cmd(buf_ptr: i32, max: i32) -> i32;
    fn npk_wifi_send_event(buf_ptr: i32, len: i32) -> i32;

    // netdev data path (driver ↔ kernel IP stack).
    fn npk_netdev_submit_rx(buf_ptr: i32, len: i32) -> i32;
    fn npk_netdev_rx_deliver(buf_ptr: i32, len: i32) -> i32;
    fn npk_netdev_poll_tx(buf_ptr: i32, max: i32) -> i32;
    fn npk_netdev_set_link(up: i32) -> i32;
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

pub fn pci_write_config(offset: u8, value: u32) {
    unsafe { npk_pci_write_config(offset as i32, value as i32); }
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

/// Read a 16-bit MMIO register (true 16-bit bus access, no RMW).
/// Offset must be 2-byte aligned.
pub fn mmio_r16(handle: i32, offset: u32) -> u16 {
    unsafe { npk_mmio_read16(handle, offset as i32) as u16 }
}

/// Write a 16-bit MMIO register (true 16-bit bus access, no RMW).
/// Required for split registers like RX/TX BD IDX where the upper 16 bits
/// are HW-owned and must not be clobbered by a 32-bit RMW.
/// Offset must be 2-byte aligned.
pub fn mmio_w16(handle: i32, offset: u32, val: u16) {
    unsafe { npk_mmio_write16(handle, offset as i32, val as i32); }
}

pub fn mmio_r64(handle: i32, offset: u32) -> u64 {
    unsafe { npk_mmio_read64(handle, offset as i32) as u64 }
}

/// Write a 64-bit MMIO register (iwl_write64). Offset must be 8-byte aligned.
pub fn mmio_w64(handle: i32, offset: u32, val: u64) {
    unsafe { npk_mmio_write64(handle, offset as i32, val as i64); }
}

/// Read-modify-write: set bits in a 32-bit MMIO register.
pub fn mmio_set32(handle: i32, offset: u32, bits: u32) {
    let val = mmio_r32(handle, offset);
    mmio_w32(handle, offset, val | bits);
}

/// Read-modify-write: clear bits in a 32-bit MMIO register.
pub fn mmio_clr32(handle: i32, offset: u32, bits: u32) {
    let val = mmio_r32(handle, offset);
    mmio_w32(handle, offset, val & !bits);
}

/// Read-modify-write: write a field value into a masked region.
/// `val` is the unshifted value (shifted to mask position automatically).
pub fn mmio_w32_mask(handle: i32, offset: u32, mask: u32, val: u32) {
    let shift = mask.trailing_zeros();
    let mut word = mmio_r32(handle, offset);
    word &= !mask;
    word |= (val << shift) & mask;
    mmio_w32(handle, offset, word);
}

/// Write a single byte within a 32-bit MMIO register (iwl_write8 semantics).
/// No `npk_mmio_write8` host fn exists, so RMW the containing dword: this
/// preserves the other three bytes, matching a true 8-bit register write.
pub fn mmio_w8(handle: i32, offset: u32, val: u8) {
    let aligned = offset & !0x3;
    let shift = (offset & 0x3) * 8;
    let mut word = mmio_r32(handle, aligned);
    word &= !(0xFFu32 << shift);
    word |= (val as u32) << shift;
    mmio_w32(handle, aligned, word);
}

/// Read-modify-write: set bits in a byte within a 32-bit MMIO register.
pub fn mmio_set8(handle: i32, offset: u32, bits: u8) {
    let aligned = offset & !0x3;
    let shift = (offset & 0x3) * 8;
    let mut word = mmio_r32(handle, aligned);
    word |= (bits as u32) << shift;
    mmio_w32(handle, aligned, word);
}

/// Read-modify-write: clear bits in a byte within a 32-bit MMIO register.
pub fn mmio_clr8(handle: i32, offset: u32, bits: u8) {
    let aligned = offset & !0x3;
    let shift = (offset & 0x3) * 8;
    let mut word = mmio_r32(handle, aligned);
    word &= !((bits as u32) << shift);
    mmio_w32(handle, aligned, word);
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

/// Milliseconds since boot (kernel tick counter × 10).
pub fn now_ms() -> u64 {
    let t = unsafe { npk_ticks() };
    if t < 0 { 0 } else { t as u64 }
}

/// Microseconds since boot. `now_ms` steps in 10 ms and cannot time one pass of
/// the poll loop — which is exactly the number that says whether this driver is
/// CPU-bound or waiting.
pub fn now_us() -> u64 {
    let t = unsafe { npk_now_us() };
    if t < 0 { 0 } else { t as u64 }
}

/// Publish a plain-text status snapshot for the `wlan` intent to print.
pub fn driver_report(text: &[u8]) {
    unsafe { npk_driver_report(text.as_ptr() as i32, text.len() as i32) };
}

/// Read an npkFS object into `buf`, returning the bytes read (0 = absent).
/// Used for the connect policy in `sys/config/*` — never for secrets: the
/// driver must not be able to see the PSK.
pub fn fetch(name: &str, buf: &mut [u8]) -> usize {
    let n = unsafe {
        npk_fetch(
            name.as_ptr() as i32, name.len() as i32,
            buf.as_mut_ptr() as i32, buf.len() as i32,
        )
    };
    if n > 0 { (n as usize).min(buf.len()) } else { 0 }
}

/// Register this driver as a network interface with the given MAC. The kernel
/// exposes it as `wlan` and routes the global IP stack to it when no wired NIC
/// is present. Returns 0 on success, -1 if already registered / on error.
pub fn netdev_register(mac: &[u8; 6]) -> i32 {
    unsafe { npk_netdev_register(mac.as_ptr() as i32) }
}

/// Dequeue one control command from the manager (wifid). Returns its length
/// (0 if none / -1 on error). The driver side is gated to bound drivers.
pub fn wifi_poll_cmd(buf: &mut [u8]) -> i32 {
    unsafe { npk_wifi_poll_cmd(buf.as_mut_ptr() as i32, buf.len() as i32) }
}

/// Send one event (uplink) to the manager. Returns 0 on success, -1 on error.
pub fn wifi_send_event(msg: &[u8]) -> i32 {
    unsafe { npk_wifi_send_event(msg.as_ptr() as i32, msg.len() as i32) }
}

/// Hand a received Ethernet frame to the kernel IP stack.
pub fn netdev_submit_rx(frame: &[u8]) {
    unsafe { npk_netdev_submit_rx(frame.as_ptr() as i32, frame.len() as i32) };
}

/// Deliver a received Ethernet frame STRAIGHT into the kernel IP stack from this
/// driver fiber's context (NAPI topology: drain → stack in one hop, off Core 0).
/// Falls back to the relay ring internally if Core 0 holds the drain guard.
pub fn netdev_rx_deliver(frame: &[u8]) {
    unsafe { npk_netdev_rx_deliver(frame.as_ptr() as i32, frame.len() as i32) };
}

/// Fetch the next Ethernet frame the kernel wants transmitted into `buf`.
/// Returns its length, or 0 when there is none.
pub fn netdev_poll_tx(buf: &mut [u8]) -> usize {
    let n = unsafe { npk_netdev_poll_tx(buf.as_mut_ptr() as i32, buf.len() as i32) };
    if n > 0 { n as usize } else { 0 }
}

/// Report carrier state (associated + keyed → data path live).
pub fn netdev_set_link(up: bool) {
    unsafe { npk_netdev_set_link(if up { 1 } else { 0 }) };
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

pub fn print_hex8(val: u8) {
    let buf = [HEX[(val >> 4) as usize], HEX[(val & 0xF) as usize]];
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

pub fn print_hex64(val: u64) {
    let mut buf = [0u8; 16];
    for i in 0..16 {
        buf[15 - i] = HEX[((val >> (i * 4)) & 0xF) as usize];
    }
    let s = unsafe { core::str::from_utf8_unchecked(&buf) };
    print(s);
}

/// Print an unsigned decimal number (for channel / count / RSSI magnitude).
pub fn print_dec(mut val: u32) {
    if val == 0 {
        print("0");
        return;
    }
    let mut buf = [0u8; 10];
    let mut i = 10;
    while val > 0 {
        i -= 1;
        buf[i] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    let s = unsafe { core::str::from_utf8_unchecked(&buf[i..]) };
    print(s);
}

pub fn log_reg(name: &str, val: u32) {
    print("  ");
    print(name);
    print(": 0x");
    print_hex32(val);
    print("\n");
}

// ── Debug tracing ────────────────────────────────────────────────
// Verbose bring-up / per-frame traces. OFF in releases: the driver is run in a
// window, so every print also RENDERS → hundreds of lines visibly slow the
// connect. Flip DEBUG to true to get the full bring-up log back. Essential,
// user-facing lines (version, scan results, AUTHORIZED, failures) use the plain
// print* fns and are always shown.
pub const DEBUG: bool = false;

#[inline] pub fn dprint(s: &str) { if DEBUG { print(s); } }
#[inline] pub fn dprint_dec(val: u32) { if DEBUG { print_dec(val); } }
#[inline] pub fn dprint_hex8(val: u8) { if DEBUG { print_hex8(val); } }
#[inline] pub fn dprint_hex16(val: u16) { if DEBUG { print_hex16(val); } }
#[inline] pub fn dprint_hex32(val: u32) { if DEBUG { print_hex32(val); } }
#[inline] pub fn dprint_hex64(val: u64) { if DEBUG { print_hex64(val); } }
