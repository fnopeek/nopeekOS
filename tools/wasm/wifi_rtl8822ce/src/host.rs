//! nopeekOS WASM Driver ABI — Bindings fuer den RTL8822CE-Treiber.
//!
//! Dieselbe ABI, die `wifi_ax200` und `wifi` (RTL8852BE) fahren, plus die
//! zwei 8-Bit-Zugriffe, die es fuer rtw88 geben MUSS: der
//! Power-Sequenz-Interpreter in `mac.c` besteht aus nichts anderem als
//! `rtw_read8`/`rtw_write8`, und ein 32-Bit-RMW ist dafuer kein Ersatz.

#![allow(dead_code)]

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_print(ptr: i32, len: i32);
    fn npk_log(ptr: i32, len: i32);

    fn npk_pci_bind(vendor: i32, device: i32) -> i32;
    fn npk_pci_enable_bus_master() -> i32;
    fn npk_pci_read_config(offset: i32) -> i32;
    fn npk_pci_write_config(offset: i32, value: i32) -> i32;

    fn npk_mmio_map_bar(bar_idx: i32, pages: i32) -> i32;
    fn npk_mmio_read8(handle: i32, offset: i32) -> i32;
    fn npk_mmio_write8(handle: i32, offset: i32, value: i32) -> i32;
    fn npk_mmio_read16(handle: i32, offset: i32) -> i32;
    fn npk_mmio_write16(handle: i32, offset: i32, value: i32) -> i32;
    fn npk_mmio_read32(handle: i32, offset: i32) -> i32;
    fn npk_mmio_write32(handle: i32, offset: i32, value: i32) -> i32;

    fn npk_dma_alloc(pages: i32) -> i32;
    fn npk_dma_alloc_below(pages: i32, limit_mb: i32) -> i32;
    fn npk_dma_phys_addr(handle: i32) -> i64;
    fn npk_dma_read(handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32;
    fn npk_dma_write(handle: i32, dma_off: i32, wasm_ptr: i32, len: i32) -> i32;
    fn npk_dma_read32(handle: i32, offset: i32) -> i32;
    fn npk_dma_write32(handle: i32, offset: i32, value: i32) -> i32;

    fn npk_memory_fence() -> i32;
    fn npk_sleep(ms: i32) -> i32;
    fn npk_ticks() -> i64;
    fn npk_now_us() -> i64;
    fn npk_driver_report(buf_ptr: i32, len: i32) -> i32;
}

// ── Ausgabe ──────────────────────────────────────────────────────

pub fn print(s: &str) {
    unsafe { npk_print(s.as_ptr() as i32, s.len() as i32) };
}

pub fn log(s: &str) {
    unsafe { npk_log(s.as_ptr() as i32, s.len() as i32) };
}

// ── PCI ──────────────────────────────────────────────────────────

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
    unsafe { npk_pci_write_config(offset as i32, value as i32) };
}

// ── MMIO ─────────────────────────────────────────────────────────

pub fn mmio_map_bar(bar: u8, pages: u16) -> i32 {
    unsafe { npk_mmio_map_bar(bar as i32, pages as i32) }
}

/// `rtw_read8`. Echter 8-Bit-Buszugriff, kein RMW.
pub fn r8(h: i32, off: u32) -> u8 {
    unsafe { npk_mmio_read8(h, off as i32) as u8 }
}

/// `rtw_write8`. Echter 8-Bit-Buszugriff — die drei Nachbarbytes bleiben
/// unberuehrt, und genau das ist der Unterschied zu einem 32-Bit-RMW.
pub fn w8(h: i32, off: u32, val: u8) {
    unsafe { npk_mmio_write8(h, off as i32, val as i32) };
}

pub fn r16(h: i32, off: u32) -> u16 {
    unsafe { npk_mmio_read16(h, off as i32) as u16 }
}

pub fn w16(h: i32, off: u32, val: u16) {
    unsafe { npk_mmio_write16(h, off as i32, val as i32) };
}

pub fn r32(h: i32, off: u32) -> u32 {
    unsafe { npk_mmio_read32(h, off as i32) as u32 }
}

pub fn w32(h: i32, off: u32, val: u32) {
    unsafe { npk_mmio_write32(h, off as i32, val as i32) };
}

/// `rtw_write8_set` — Bits im BYTE setzen, gelesen und geschrieben in 8 Bit.
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

/// Zusammenhaengende Seiten unter 4 GB (der Kernel garantiert beides; der
/// TX-/RX-Deskriptor hat nur ein 32-Bit-Adressfeld).
pub fn dma_alloc(pages: u16) -> i32 {
    unsafe { npk_dma_alloc(pages as i32) }
}

/// Wie `dma_alloc`, aber unter einer selbst genannten Obergrenze.
/// `limit_mb = 0` heisst 4 GB, also dasselbe wie `dma_alloc`.
pub fn dma_alloc_below(pages: u16, limit_mb: u32) -> i32 {
    unsafe { npk_dma_alloc_below(pages as i32, limit_mb as i32) }
}

pub fn dma_phys(handle: i32) -> u64 {
    let v = unsafe { npk_dma_phys_addr(handle) };
    if v < 0 { 0 } else { v as u64 }
}

/// Bytes aus dem Linearspeicher in den DMA-Puffer. 4096 Bytes ueber 1024
/// Einzelschreibungen waeren dieselbe Wirkung zum vielfachen Preis.
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
    unsafe { npk_dma_write32(handle, offset as i32, val as i32) };
}

// ── Zeit und Bericht ─────────────────────────────────────────────

pub fn fence() {
    unsafe { npk_memory_fence() };
}

pub fn sleep_ms(ms: u32) {
    unsafe { npk_sleep(ms as i32) };
}

pub fn now_ms() -> u64 {
    let t = unsafe { npk_ticks() };
    if t < 0 { 0 } else { t as u64 }
}

pub fn now_us() -> u64 {
    let t = unsafe { npk_now_us() };
    if t < 0 { 0 } else { t as u64 }
}

pub fn driver_report(text: &[u8]) {
    unsafe { npk_driver_report(text.as_ptr() as i32, text.len() as i32) };
}

// ── Hex/Dez ohne alloc ───────────────────────────────────────────

const HEX: &[u8; 16] = b"0123456789abcdef";

pub fn print_hex8(v: u8) {
    let b = [HEX[(v >> 4) as usize], HEX[(v & 0xf) as usize]];
    print(unsafe { core::str::from_utf8_unchecked(&b) });
}

pub fn print_hex16(v: u16) {
    let mut b = [0u8; 4];
    for i in 0..4 {
        b[3 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
    }
    print(unsafe { core::str::from_utf8_unchecked(&b) });
}

pub fn print_hex32(v: u32) {
    let mut b = [0u8; 8];
    for i in 0..8 {
        b[7 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
    }
    print(unsafe { core::str::from_utf8_unchecked(&b) });
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
    print(unsafe { core::str::from_utf8_unchecked(&b[i..]) });
}

/// Ein Register mit Namen und Wert — die Zeile, aus der spaeter jede
/// Diagnose kommt.
pub fn log_reg32(name: &str, val: u32) {
    print("  ");
    print(name);
    print(" = 0x");
    print_hex32(val);
    print("\n");
}

/// hci.h:241-253 `rtw_write32_mask` — Feld an seiner Schiebestelle setzen.
/// `data` ist der Wert des FELDES, nicht das fertige Bitmuster.
pub fn w32_mask(h: i32, off: u32, mask: u32, data: u32) {
    let shift = mask.trailing_zeros();
    let orig = r32(h, off);
    w32(h, off, (orig & !mask) | ((data << shift) & mask));
}

/// hci.h:202-212 `rtw_read32_mask`
pub fn r32_mask(h: i32, off: u32, mask: u32) -> u32 {
    (r32(h, off) & mask) >> mask.trailing_zeros()
}

/// `udelay(n)`. Unter einer Millisekunde kann `npk_sleep` nichts, also wird
/// auf der Uhr gedreht. Der Preis ist ehrlich: ein `npk_now_us` kostet selbst
/// etwa so viel wie die kuerzeste Pause, die hier verlangt wird (1 us nach
/// jedem RF-Schreibzugriff), und laenger zu warten als noetig ist harmlos —
/// kuerzer waere es nicht.
pub fn delay_us(us: u64) {
    let t0 = now_us();
    while now_us() - t0 < us {}
}
