//! Host function bindings for the nopeekOS WASM driver ABI.

use npk_sys as sys;

pub fn print(s: &str) {
    sys::print(s.as_bytes());
}

/// Raw bytes to the terminal, unchecked for UTF-8.
pub fn print_bytes(b: &[u8]) {
    sys::print(b);
}

pub fn log(s: &str) {
    sys::log(s.as_bytes());
}

pub fn pci_bind(vendor: u16, device: u16) -> i32 {
    sys::pci_bind(vendor as i32, device as i32)
}

pub fn pci_enable_bus_master() -> i32 {
    sys::pci_enable_bus_master()
}

pub fn pci_read_config(offset: u8) -> u32 {
    sys::pci_read_config(offset as i32) as u32
}

pub fn pci_write_config(offset: u8, value: u32) {
    sys::pci_write_config(offset as i32, value as i32);
}

pub fn mmio_map_bar(bar: u8, pages: u16) -> i32 {
    sys::mmio_map_bar(bar as i32, pages as i32)
}

pub fn mmio_r32(handle: i32, offset: u32) -> u32 {
    sys::mmio_read32(handle, offset as i32) as u32
}

pub fn mmio_w32(handle: i32, offset: u32, val: u32) {
    sys::mmio_write32(handle, offset as i32, val as i32);
}

/// Read a 16-bit MMIO register (true 16-bit bus access, no RMW).
/// Offset must be 2-byte aligned.
pub fn mmio_r16(handle: i32, offset: u32) -> u16 {
    sys::mmio_read16(handle, offset as i32) as u16
}

/// Write a 16-bit MMIO register (true 16-bit bus access, no RMW).
/// Required for split registers like RX/TX BD IDX where the upper 16 bits
/// are HW-owned and must not be clobbered by a 32-bit RMW.
/// Offset must be 2-byte aligned.
pub fn mmio_w16(handle: i32, offset: u32, val: u16) {
    sys::mmio_write16(handle, offset as i32, val as i32);
}

pub fn mmio_r64(handle: i32, offset: u32) -> u64 {
    sys::mmio_read64(handle, offset as i32) as u64
}

/// Write a 64-bit MMIO register (iwl_write64). Offset must be 8-byte aligned.
pub fn mmio_w64(handle: i32, offset: u32, val: u64) {
    sys::mmio_write64(handle, offset as i32, val as i64);
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

/// Write a single byte within a 32-bit MMIO register (iwl_write8 semantics)
/// as a read-modify-write of the containing dword, which preserves the other
/// three bytes. Linux issues a real 8-bit write (`npk_sys::mmio_write8`);
/// differs only on registers with read side effects.
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
    sys::dma_alloc(pages as i32)
}

pub fn dma_phys(handle: i32) -> u64 {
    sys::dma_phys_addr(handle) as u64
}

pub fn dma_write_buf(handle: i32, offset: u32, data: &[u8]) -> i32 {
    sys::dma_write(handle, offset as i32, data)
}

pub fn dma_read_buf(handle: i32, offset: u32, buf: &mut [u8]) -> i32 {
    sys::dma_read(handle, offset as i32, buf)
}

pub fn dma_r32(handle: i32, offset: u32) -> u32 {
    sys::dma_read32(handle, offset as i32) as u32
}

pub fn dma_w32(handle: i32, offset: u32, val: u32) {
    sys::dma_write32(handle, offset as i32, val as i32);
}

pub fn fence() {
    sys::memory_fence();
}

pub fn sleep_ms(ms: u32) {
    sys::sleep(ms as i32);
}

pub fn input_wait(timeout_ms: u32) -> i32 {
    sys::input_wait(timeout_ms as i32)
}

/// Milliseconds since boot (kernel tick counter × 10).
pub fn now_ms() -> u64 {
    let t = sys::ticks();
    if t < 0 { 0 } else { t as u64 }
}

/// Microseconds since boot. `now_ms` steps in 10 ms and cannot time one pass of
/// the poll loop — which is exactly the number that says whether this driver is
/// CPU-bound or waiting.
pub fn now_us() -> u64 {
    let t = sys::now_us();
    if t < 0 { 0 } else { t as u64 }
}

/// Publish a plain-text status snapshot for the `wlan` intent to print.
pub fn driver_report(text: &[u8]) {
    sys::driver_report(text);
}

/// Read an npkFS object into `buf`, returning the bytes read (0 = absent).
/// Used for the connect policy in `sys/config/*` — never for secrets: the
/// driver must not be able to see the PSK.
pub fn fetch(name: &str, buf: &mut [u8]) -> usize {
    let n = sys::fetch(name.as_bytes(), buf);
    if n > 0 { (n as usize).min(buf.len()) } else { 0 }
}

/// Register this driver as a network interface with the given MAC. The kernel
/// exposes it as `wlan` and routes the global IP stack to it when no wired NIC
/// is present. Returns 0 on success, -1 if already registered / on error.
pub fn netdev_register(mac: &[u8; 6]) -> i32 {
    sys::netdev_register(mac)
}

/// Dequeue one control command from the manager (wifid). Returns its length
/// (0 if none / -1 on error). The driver side is gated to bound drivers.
pub fn wifi_poll_cmd(buf: &mut [u8]) -> i32 {
    sys::wifi_poll_cmd(buf)
}

/// Send one event (uplink) to the manager. Returns 0 on success, -1 on error.
pub fn wifi_send_event(msg: &[u8]) -> i32 {
    sys::wifi_send_event(msg)
}

/// Hand a received Ethernet frame to the kernel IP stack.
pub fn netdev_submit_rx(frame: &[u8]) {
    sys::netdev_submit_rx(frame);
}

/// Deliver a received Ethernet frame straight into the kernel IP stack from this
/// driver fiber's context (NAPI topology: drain → stack in one hop, off Core 0).
/// Falls back to the relay ring internally if Core 0 holds the drain guard.
pub fn netdev_rx_deliver(frame: &[u8]) {
    sys::netdev_rx_deliver(frame);
}

/// Fetch the next Ethernet frame the kernel wants transmitted into `buf`.
/// Returns its length, or 0 when there is none.
pub fn netdev_poll_tx(buf: &mut [u8]) -> usize {
    let n = sys::netdev_poll_tx(buf);
    if n > 0 { n as usize } else { 0 }
}

/// Report carrier state (associated + keyed → data path live).
/// RFC 2863 pair: `carrier` = the association exists, `dormant` = it exists
/// but is not usable yet (WPA not done). operstate is UP only when
/// carrier && !dormant.
pub fn netdev_set_link_state(carrier: bool, dormant: bool) {
    sys::netdev_set_link_state(if carrier { 1 } else { 0 },
                               if dormant { 1 } else { 0 });
}

pub fn netdev_set_link(up: bool) {
    sys::netdev_set_link(if up { 1 } else { 0 });
}

// ── Hex output helpers ───────────────────────────────────────────

const HEX: &[u8; 16] = b"0123456789abcdef";

pub fn print_hex32(val: u32) {
    let mut buf = [0u8; 8];
    for i in 0..8 {
        buf[7 - i] = HEX[((val >> (i * 4)) & 0xF) as usize];
    }
    print_bytes(&buf);
}

pub fn print_hex8(val: u8) {
    let buf = [HEX[(val >> 4) as usize], HEX[(val & 0xF) as usize]];
    print_bytes(&buf);
}

pub fn print_hex16(val: u16) {
    let mut buf = [0u8; 4];
    for i in 0..4 {
        buf[3 - i] = HEX[((val >> (i * 4)) & 0xF) as usize];
    }
    print_bytes(&buf);
}

pub fn print_hex64(val: u64) {
    let mut buf = [0u8; 16];
    for i in 0..16 {
        buf[15 - i] = HEX[((val >> (i * 4)) & 0xF) as usize];
    }
    print_bytes(&buf);
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
    print_bytes(&buf[i..]);
}

pub fn log_reg(name: &str, val: u32) {
    print("  ");
    print(name);
    print(": 0x");
    print_hex32(val);
    print("\n");
}

// ── Debug tracing ────────────────────────────────────────────────
// Verbose bring-up and per-frame traces, off by default: the driver runs in a
// window, so every print also renders, and hundreds of lines slow the connect.
// Set DEBUG to true for the full bring-up log.
// Essential user-facing lines (version, scan results, AUTHORIZED, failures) use
// the plain print* fns and are always shown.
pub const DEBUG: bool = false;

#[inline] pub fn dprint(s: &str) { if DEBUG { print(s); } }
#[inline] pub fn dprint_bytes(b: &[u8]) { if DEBUG { print_bytes(b); } }
#[inline] pub fn dprint_dec(val: u32) { if DEBUG { print_dec(val); } }
#[inline] pub fn dprint_hex8(val: u8) { if DEBUG { print_hex8(val); } }
#[inline] pub fn dprint_hex16(val: u16) { if DEBUG { print_hex16(val); } }
#[inline] pub fn dprint_hex32(val: u32) { if DEBUG { print_hex32(val); } }
#[inline] pub fn dprint_hex64(val: u64) { if DEBUG { print_hex64(val); } }
