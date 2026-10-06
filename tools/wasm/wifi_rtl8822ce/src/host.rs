//! nopeekOS WASM driver ABI: bindings for the RTL8822CE driver.
//!
//! The same ABI as `wifi_ax200` and `wifi` (RTL8852BE), plus the two 8-bit
//! accessors rtw88 needs: the power sequence interpreter in `mac.c` is
//! nothing but `rtw_read8`/`rtw_write8`, and a 32-bit RMW is no substitute.

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ── Waiting for events ───────────────────────────────────────────

/// `npk_wait`-Bits (Kernel `host_core::npk_wait`).
pub const WAIT_IRQ: i32 = 2;
pub const WAIT_NET_TX: i32 = 4;
pub const WAIT_WIFI_CMD: i32 = 8;

/// Register the MSI of the bound device. Returns the vector, or `-1` if
/// there is none; the driver then stays in polling mode.
pub fn irq_register() -> i32 {
    npk_sys::irq_register(0)
}

/// Park until one of the events in `mask` occurs or `timeout_ms` passes.
/// Returns the bits that occurred, 0 on timeout.
pub fn wait(mask: i32, timeout_ms: u32) -> i32 {
    npk_sys::wait(mask, timeout_ms as i32)
}

// ── Control channel, npkFS and data path ─────────────────────────

/// Fetch an object from npkFS. Returns its length, or `-1` if it does not
/// exist, which is an ordinary case and not an error.
pub fn fetch(name: &str, buf: &mut [u8]) -> i32 {
    npk_sys::fetch(name.as_bytes(), buf)
}

/// Next command from `wifid`, `-1` = none.
pub fn wifi_poll_cmd(buf: &mut [u8]) -> i32 {
    npk_sys::wifi_poll_cmd(buf)
}

/// Event to `wifid`.
pub fn wifi_send_event(msg: &[u8]) -> i32 {
    npk_sys::wifi_send_event(msg)
}

pub fn netdev_register(mac: &[u8; 6]) -> i32 {
    npk_sys::netdev_register(mac)
}

/// Hand a received Ethernet frame to the IP stack.
pub fn netdev_submit_rx(frame: &[u8]) -> i32 {
    npk_sys::netdev_submit_rx(frame)
}

/// Fetch an Ethernet frame to send, `-1` = none.
pub fn netdev_poll_tx(buf: &mut [u8]) -> i32 {
    npk_sys::netdev_poll_tx(buf)
}

pub fn netdev_set_link(up: bool) {
    npk_sys::netdev_set_link(up as i32);
}

// ── Output: verbose or quiet ─────────────────────────────────────
//
// Quiet is the default at autostart; the bring-up stages print a lot and
// would make the console useless for everything else. `debug: 1` in
// `sys/config/wifi` turns them back on.
//
// Quiet does not mean mute: anything wrong always goes out, via `say` for
// a string or `loud_begin`/`loud_end` around a composed line. Because
// `print_dec` and friends call `print`, which honours the bracket, no
// second set of number formatters is needed.
static VERBOSE: AtomicBool = AtomicBool::new(false);
static LOUD: AtomicU32 = AtomicU32::new(0);

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// Everything up to `loud_end` goes out even without `debug: 1`. Counted,
/// not toggled, so one caller cannot turn off another.
pub fn loud_begin() {
    LOUD.fetch_add(1, Ordering::Relaxed);
}

pub fn loud_end() {
    // Saturating: an unmatched close must not underflow and thereby enable
    // all output.
    let v = LOUD.load(Ordering::Relaxed);
    LOUD.store(v.saturating_sub(1), Ordering::Relaxed);
}

fn emit(s: &str) {
    npk_sys::print(s.as_bytes());
}

/// Stage output: only with `debug: 1` or inside `loud_*`.
pub fn print(s: &str) {
    if verbose() || LOUD.load(Ordering::Relaxed) > 0 {
        emit(s);
    }
}

/// Always printed: errors, failed gates, link state.
pub fn say(s: &str) {
    emit(s);
}

pub fn log(s: &str) {
    npk_sys::log(s.as_bytes());
}

// ── PCI ──────────────────────────────────────────────────────────

pub fn pci_bind(vendor: u16, device: u16) -> i32 {
    npk_sys::pci_bind(vendor as i32, device as i32)
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

// ── MMIO ─────────────────────────────────────────────────────────

pub fn mmio_map_bar(bar: u8, pages: u16) -> i32 {
    npk_sys::mmio_map_bar(bar as i32, pages as i32)
}

/// `rtw_read8`. A real 8-bit bus access, not an RMW.
pub fn r8(h: i32, off: u32) -> u8 {
    npk_sys::mmio_read8(h, off as i32) as u8
}

/// `rtw_write8`. A real 8-bit bus access: the three neighbouring bytes are
/// untouched, which is the difference to a 32-bit RMW.
pub fn w8(h: i32, off: u32, val: u8) {
    npk_sys::mmio_write8(h, off as i32, val as i32);
}

pub fn r16(h: i32, off: u32) -> u16 {
    npk_sys::mmio_read16(h, off as i32) as u16
}

pub fn w16(h: i32, off: u32, val: u16) {
    npk_sys::mmio_write16(h, off as i32, val as i32);
}

pub fn r32(h: i32, off: u32) -> u32 {
    npk_sys::mmio_read32(h, off as i32) as u32
}

pub fn w32(h: i32, off: u32, val: u32) {
    npk_sys::mmio_write32(h, off as i32, val as i32);
}

/// `rtw_write8_set`: set bits in the byte, read and written as 8 bits.
pub fn set8(h: i32, off: u32, bits: u8) {
    w8(h, off, r8(h, off) | bits);
}

/// `rtw_write8_clr`
pub fn clr8(h: i32, off: u32, bits: u8) {
    w8(h, off, r8(h, off) & !bits);
}

/// `rtw_write32_set`
pub fn set32(h: i32, off: u32, bits: u32) {
    w32(h, off, r32(h, off) | bits);
}

/// `rtw_write32_clr`
pub fn clr32(h: i32, off: u32, bits: u32) {
    w32(h, off, r32(h, off) & !bits);
}

// ── DMA ──────────────────────────────────────────────────────────

/// Contiguous pages below 4 GB (the kernel guarantees both; the TX/RX
/// descriptor has only a 32-bit address field).
pub fn dma_alloc(pages: u16) -> i32 {
    npk_sys::dma_alloc(pages as i32)
}

/// Like `dma_alloc`, but below a caller-given limit. `limit_mb = 0` means
/// 4 GB, the same as `dma_alloc`.
pub fn dma_alloc_below(pages: u16, limit_mb: u32) -> i32 {
    npk_sys::dma_alloc_below(pages as i32, limit_mb as i32)
}

pub fn dma_phys(handle: i32) -> u64 {
    let v = npk_sys::dma_phys_addr(handle);
    if v < 0 { 0 } else { v as u64 }
}

/// Copy bytes from linear memory into the DMA buffer in one call instead
/// of one host call per word.
pub fn dma_write_buf(handle: i32, offset: u32, data: &[u8]) -> i32 {
    npk_sys::dma_write(handle, offset as i32, data)
}

pub fn dma_read_buf(handle: i32, offset: u32, buf: &mut [u8]) -> i32 {
    npk_sys::dma_read(handle, offset as i32, buf)
}

pub fn dma_r32(handle: i32, offset: u32) -> u32 {
    npk_sys::dma_read32(handle, offset as i32) as u32
}

pub fn dma_w32(handle: i32, offset: u32, val: u32) {
    npk_sys::dma_write32(handle, offset as i32, val as i32);
}

// ── Time and reporting ───────────────────────────────────────────

pub fn fence() {
    npk_sys::memory_fence();
}

pub fn sleep_ms(ms: u32) {
    npk_sys::sleep(ms as i32);
}

pub fn now_ms() -> u64 {
    let t = npk_sys::ticks();
    if t < 0 { 0 } else { t as u64 }
}

pub fn now_us() -> u64 {
    let t = npk_sys::now_us();
    if t < 0 { 0 } else { t as u64 }
}

pub fn driver_report(text: &[u8]) {
    npk_sys::driver_report(text);
}

// ── Hex/decimal without alloc ────────────────────────────────────

const HEX: &[u8; 16] = b"0123456789abcdef";

pub fn print_hex8(v: u8) {
    let b = [HEX[(v >> 4) as usize], HEX[(v & 0xf) as usize]];
    print(core::str::from_utf8(&b).unwrap_or("?"));
}

pub fn print_hex16(v: u16) {
    let mut b = [0u8; 4];
    for i in 0..4 {
        b[3 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
    }
    print(core::str::from_utf8(&b).unwrap_or("?"));
}

pub fn print_hex32(v: u32) {
    let mut b = [0u8; 8];
    for i in 0..8 {
        b[7 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
    }
    print(core::str::from_utf8(&b).unwrap_or("?"));
}

pub fn print_dec(mut v: u32) {
    if v == 0 {
        print("0");
        return;
    }
    let mut b = [0u8; 10];
    let mut i = 10;
    while v > 0 {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    print(core::str::from_utf8(&b[i..]).unwrap_or("?"));
}

/// Print a register with name and value.
pub fn log_reg32(name: &str, val: u32) {
    print("  ");
    print(name);
    print(" = 0x");
    print_hex32(val);
    print("\n");
}

/// hci.h:241-253 `rtw_write32_mask`: set a field at its shift position.
/// `data` is the field value, not the shifted bit pattern.
pub fn w32_mask(h: i32, off: u32, mask: u32, data: u32) {
    let shift = mask.trailing_zeros();
    let orig = r32(h, off);
    w32(h, off, (orig & !mask) | ((data << shift) & mask));
}

/// hci.h:202-212 `rtw_read32_mask`
pub fn r32_mask(h: i32, off: u32, mask: u32) -> u32 {
    (r32(h, off) & mask) >> mask.trailing_zeros()
}

/// `udelay(n)`. `npk_sleep` cannot wait less than a millisecond, so this
/// spins on the clock. A `npk_now_us` call itself costs about as much as
/// the shortest delay requested (1 us after each RF write); waiting longer
/// than needed is harmless, shorter would not be.
pub fn delay_us(us: u64) {
    let t0 = now_us();
    while now_us() - t0 < us {}
}

/// `rtw_write16_set`
pub fn set16(h: i32, off: u32, bits: u16) {
    w16(h, off, r16(h, off) | bits);
}

/// `rtw_write16_clr`
pub fn clr16(h: i32, off: u32, bits: u16) {
    w16(h, off, r16(h, off) & !bits);
}

/// hci.h:255-266 `rtw_write8_mask`: set a field in the byte. `mask` is
/// truncated to eight bits first, as in Linux.
pub fn w8_mask(h: i32, off: u32, mask: u32, data: u8) {
    let mask = (mask & 0xff) as u8;
    let shift = mask.trailing_zeros();
    let orig = r8(h, off);
    w8(h, off, (orig & !mask) | ((data << shift) & mask));
}

/// `rtw_write16_set` for a field: Linux has no `rtw_write16_mask`, but
/// `rtw_write16_set` with a mask.
pub fn r16_mask(h: i32, off: u32, mask: u16) -> u16 {
    (r16(h, off) & mask) >> mask.trailing_zeros()
}
