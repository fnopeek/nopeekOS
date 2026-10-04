# `tools/wasm/wifi_ax200/src/host.rs` @ 5e0102684

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

## L46 · `fn npk_wifi_poll_cmd(buf_ptr: i32, max: i32) -> i32;`

```
// WiFi-class control channel (driver side — gated to bound drivers).
```

## L50 · `fn npk_netdev_submit_rx(buf_ptr: i32, len: i32) -> i32;`

```
// netdev data path (driver ↔ kernel IP stack).
```

## L58 · `pub fn print(s: &str) {`

```
// ── Safe wrappers ────────────────────────────────────────────────
```

## L96-97 · `pub fn mmio_r16(handle: i32, offset: u32) -> u16 {`

```
/// Read a 16-bit MMIO register (true 16-bit bus access, no RMW).
/// Offset must be 2-byte aligned.
```

## L102-105 · `pub fn mmio_w16(handle: i32, offset: u32, val: u16) {`

```
/// Write a 16-bit MMIO register (true 16-bit bus access, no RMW).
/// Required for split registers like RX/TX BD IDX where the upper 16 bits
/// are HW-owned and must not be clobbered by a 32-bit RMW.
/// Offset must be 2-byte aligned.
```

## L114 · `pub fn mmio_w64(handle: i32, offset: u32, val: u64) {`

```
/// Write a 64-bit MMIO register (iwl_write64). Offset must be 8-byte aligned.
```

## L119 · `pub fn mmio_set32(handle: i32, offset: u32, bits: u32) {`

```
/// Read-modify-write: set bits in a 32-bit MMIO register.
```

## L125 · `pub fn mmio_clr32(handle: i32, offset: u32, bits: u32) {`

```
/// Read-modify-write: clear bits in a 32-bit MMIO register.
```

## L131-132 · `pub fn mmio_w32_mask(handle: i32, offset: u32, mask: u32, val: u32) {`

```
/// Read-modify-write: write a field value into a masked region.
/// `val` is the unshifted value (shifted to mask position automatically).
```

## L141-143 · `pub fn mmio_w8(handle: i32, offset: u32, val: u8) {`

```
/// Write a single byte within a 32-bit MMIO register (iwl_write8 semantics).
/// No `npk_mmio_write8` host fn exists, so RMW the containing dword: this
/// preserves the other three bytes, matching a true 8-bit register write.
```

## L153 · `pub fn mmio_set8(handle: i32, offset: u32, bits: u8) {`

```
/// Read-modify-write: set bits in a byte within a 32-bit MMIO register.
```

## L162 · `pub fn mmio_clr8(handle: i32, offset: u32, bits: u8) {`

```
/// Read-modify-write: clear bits in a byte within a 32-bit MMIO register.
```

## L207 · `pub fn now_ms() -> u64 {`

```
/// Milliseconds since boot (kernel tick counter × 10).
```

## L213-215 · `pub fn now_us() -> u64 {`

```
/// Microseconds since boot. `now_ms` steps in 10 ms and cannot time one pass of
/// the poll loop — which is exactly the number that says whether this driver is
/// CPU-bound or waiting.
```

## L221 · `pub fn driver_report(text: &[u8]) {`

```
/// Publish a plain-text status snapshot for the `wlan` intent to print.
```

## L226-228 · `pub fn fetch(name: &str, buf: &mut [u8]) -> usize {`

```
/// Read an npkFS object into `buf`, returning the bytes read (0 = absent).
/// Used for the connect policy in `sys/config/*` — never for secrets: the
/// driver must not be able to see the PSK.
```

## L239-241 · `pub fn netdev_register(mac: &[u8; 6]) -> i32 {`

```
/// Register this driver as a network interface with the given MAC. The kernel
/// exposes it as `wlan` and routes the global IP stack to it when no wired NIC
/// is present. Returns 0 on success, -1 if already registered / on error.
```

## L246-247 · `pub fn wifi_poll_cmd(buf: &mut [u8]) -> i32 {`

```
/// Dequeue one control command from the manager (wifid). Returns its length
/// (0 if none / -1 on error). The driver side is gated to bound drivers.
```

## L252 · `pub fn wifi_send_event(msg: &[u8]) -> i32 {`

```
/// Send one event (uplink) to the manager. Returns 0 on success, -1 on error.
```

## L257 · `pub fn netdev_submit_rx(frame: &[u8]) {`

```
/// Hand a received Ethernet frame to the kernel IP stack.
```

## L262-264 · `pub fn netdev_rx_deliver(frame: &[u8]) {`

```
/// Deliver a received Ethernet frame STRAIGHT into the kernel IP stack from this
/// driver fiber's context (NAPI topology: drain → stack in one hop, off Core 0).
/// Falls back to the relay ring internally if Core 0 holds the drain guard.
```

## L269-270 · `pub fn netdev_poll_tx(buf: &mut [u8]) -> usize {`

```
/// Fetch the next Ethernet frame the kernel wants transmitted into `buf`.
/// Returns its length, or 0 when there is none.
```

## L276-279 · `pub fn netdev_set_link_state(carrier: bool, dormant: bool) {`

```
/// Report carrier state (associated + keyed → data path live).
/// RFC 2863 pair: `carrier` = the association exists, `dormant` = it exists
/// but is not usable yet (WPA not done). operstate is UP only when
/// carrier && !dormant.
```

## L289 · `const HEX: &[u8; 16] = b"0123456789abcdef";`

```
// ── Hex output helpers ───────────────────────────────────────────
```

## L326 · `pub fn print_dec(mut val: u32) {`

```
/// Print an unsigned decimal number (for channel / count / RSSI magnitude).
```

## L351-356 · `pub const DEBUG: bool = false;`

```
// ── Debug tracing ────────────────────────────────────────────────
// Verbose bring-up / per-frame traces. OFF in releases: the driver is run in a
// window, so every print also RENDERS → hundreds of lines visibly slow the
// connect. Flip DEBUG to true to get the full bring-up log back. Essential,
// user-facing lines (version, scan results, AUTHORIZED, failures) use the plain
// print* fns and are always shown.
```

