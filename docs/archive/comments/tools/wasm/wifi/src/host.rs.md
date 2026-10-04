# `tools/wasm/wifi/src/host.rs` @ 5e0102684

## L1 · `#[link(wasm_import_module = "env")]`

```
//! Host function bindings for nopeekOS WASM Driver ABI
```

## L3-5 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L8 · `fn npk_print(ptr: i32, len: i32);`

```
// Output
```

## L12 · `fn npk_pci_bind(vendor: i32, device: i32) -> i32;`

```
// PCI
```

## L19 · `fn npk_mmio_map_bar(bar_idx: i32, pages: i32) -> i32;`

```
// MMIO
```

## L28 · `fn npk_dma_alloc(pages: i32) -> i32;`

```
// DMA
```

## L36 · `fn npk_memory_fence() -> i32;`

```
// Misc
```

## L44 · `pub fn print(s: &str) {`

```
// ── Safe wrappers ────────────────────────────────────────────────
```

## L82-83 · `pub fn mmio_r16(handle: i32, offset: u32) -> u16 {`

```
/// Read a 16-bit MMIO register (true 16-bit bus access, no RMW).
/// Offset must be 2-byte aligned.
```

## L88-91 · `pub fn mmio_w16(handle: i32, offset: u32, val: u16) {`

```
/// Write a 16-bit MMIO register (true 16-bit bus access, no RMW).
/// Required for split registers like RX/TX BD IDX where the upper 16 bits
/// are HW-owned and must not be clobbered by a 32-bit RMW.
/// Offset must be 2-byte aligned.
```

## L100 · `pub fn mmio_set32(handle: i32, offset: u32, bits: u32) {`

```
/// Read-modify-write: set bits in a 32-bit MMIO register.
```

## L106 · `pub fn mmio_clr32(handle: i32, offset: u32, bits: u32) {`

```
/// Read-modify-write: clear bits in a 32-bit MMIO register.
```

## L112-113 · `pub fn mmio_w32_mask(handle: i32, offset: u32, mask: u32, val: u32) {`

```
/// Read-modify-write: write a field value into a masked region.
/// `val` is the unshifted value (shifted to mask position automatically).
```

## L122 · `pub fn mmio_set8(handle: i32, offset: u32, bits: u8) {`

```
/// Read-modify-write: set bits in a byte within a 32-bit MMIO register.
```

## L131 · `pub fn mmio_clr8(handle: i32, offset: u32, bits: u8) {`

```
/// Read-modify-write: clear bits in a byte within a 32-bit MMIO register.
```

## L176 · `const HEX: &[u8; 16] = b"0123456789abcdef";`

```
// ── Hex output helpers ───────────────────────────────────────────
```

