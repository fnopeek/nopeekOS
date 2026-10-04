# `kernel/src/drivers/serial.rs` @ 5e0102684

## L1-4 · `use core::fmt;`

```
//! Serial Console Driver (COM1)
//!
//! Minimal human interface via serial port.
//! QEMU: -serial stdio
```

## L11-15 · `static WRITE_GEN: AtomicU64 = AtomicU64::new(0);`

```
/// Bumped once per console `write_str`. The line editor snapshots
/// this before its async poll batch and reprints the prompt + input
/// if it changed — so background output (guest logs, net, worker
/// intents) never leaves a corrupted, prompt-less input line.
/// Edge-triggered + general (any output source), not microvm-specific.
```

## L18-19 · `pub fn write_gen() -> u64 {`

```
/// Snapshot of the console-write generation. A change between two
/// reads means something wrote to the console in between.
```

## L28 · `static CAPTURE: Mutex<Option<String>> = Mutex::new(None);`

```
// Output capture for npk-shell: when active, all kprint output is also buffered
```

## L31 · `pub fn start_capture() {`

```
/// Start capturing serial output into a buffer.
```

## L36 · `pub fn stop_capture() -> String {`

```
/// Stop capturing and return the captured output.
```

## L41-42 · `pub fn capture_snapshot() -> String {`

```
/// Copy what has been captured so far, leaving capture running. Used to
/// persist the boot log without blinding `dmesg` for the rest of the session.
```

## L50 · `fn capture_bytes(s: &str) {`

```
/// Append to capture buffer if active (called from write_str).
```

## L67 · `pub fn init(&mut self) {`

```
/// Initialize COM1: 115200 baud, 8N1
```

## L70 · `if inb(self.base + 5) == 0xFF {`

```
// Check if serial port exists (0xFF = no hardware)
```

## L75 · `outb(self.base + 1, 0x00);       // Disable interrupts`

```
// Disable interrupts
```

## L76 · `outb(self.base + 3, 0x80);       // Enable DLAB`

```
// Enable DLAB
```

## L77 · `outb(self.base + 0, 0x01);       // Divisor low: 115200 baud`

```
// Divisor low: 115200 baud
```

## L78 · `outb(self.base + 1, 0x00);       // Divisor high`

```
// Divisor high
```

## L79 · `outb(self.base + 3, 0x03);       // 8 bits, no parity, one stop`

```
// 8 bits, no parity, one stop
```

## L80 · `outb(self.base + 2, 0xC7);       // Enable FIFO, 14-byte threshold`

```
// Enable FIFO, 14-byte threshold
```

## L81 · `outb(self.base + 4, 0x0B);       // IRQs off, RTS/DSR set`

```
// IRQs off, RTS/DSR set
```

## L82 · `outb(self.base + 4, 0x1E);       // Loopback test`

```
// Loopback test
```

## L86 · `return; // Port defective`

```
// Port defective
```

## L88 · `outb(self.base + 4, 0x0F);       // Normal operation`

```
// Normal operation
```

## L96-100 · `outb(0x2F8, byte);`

```
// Mirror to COM2 (0x2F8) — host pipes this to
// target/serial.log via -serial file: in build.sh. Write
// first/unconditionally so the log captures every byte
// even if COM1's THR-empty poll stalls. Real HW just
// discards writes to an absent COM2 port.
```

## L107 · `fn echo_byte(&self, byte: u8) {`

```
/// Write byte with framebuffer echo (for read_line echo on headless systems)
```

## L113 · `pub fn read_byte(&self) -> u8 {`

```
/// Blocking read. Polls both serial port and USB keyboard.
```

## L116 · `if let Some(key) = crate::keyboard::read_key() {`

```
// Check keyboard first (USB/xHCI — primary on bare metal)
```

## L120 · `if self.port_exists && self.has_data() {`

```
// Check serial (only if port exists — 0xFF means no hardware)
```

## L132-136 · `pub fn read_serial_raw(&self) -> u8 {`

```
/// Read a single byte directly from the UART receive register.
/// Unlike read_byte(), this does NOT fall back to the USB keyboard.
/// Caller must check has_data() first. Use from paths that already
/// poll the USB keyboard separately (e.g. the intent loop) so the
/// two input sources don't steal each other's keystrokes.
```

## L141 · `pub fn read_line_masked(&self, buf: &mut [u8]) -> usize {`

```
/// Read a line with masked echo (shows '*'), returns length.
```

## L172 · `pub fn read_line(&self, buf: &mut [u8]) -> usize {`

```
/// Read a line with echo, returns length
```

