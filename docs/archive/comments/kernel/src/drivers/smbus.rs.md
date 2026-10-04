# `kernel/src/drivers/smbus.rs` @ 5e0102684

## L1-8 · `use spin::Mutex;`

```
//! Intel ICH/PCH SMBus host controller (i801) — polling word-data reads.
//!
//! Ported 1:1 from Linux `drivers/i2c/busses/i2c-i801.c` (the no-IRQ
//! `i801_wait_intr` / `i801_simple_transaction` path). Vendor-neutral: the
//! controller is found by PCI class 0x0C05 (serial-bus / SMBus), so any
//! Intel PCH exposing the standard i801 register block works without an ID
//! table. Only the SMBus READ_WORD transaction is implemented — that is all
//! the Smart-Battery client (`battery.rs`) needs.
```

## L15 · `const SMBHSTSTS: u16 = 0;`

```
// SMBus I/O register offsets from the SMBA base (i2c-i801.c).
```

## L23 · `const SMBHSTCFG: u8 = 0x40;`

```
// PCI config: host configuration register + I/O BAR (BAR4).
```

## L26 · `const SMBBAR_OFFSET: u8 = 0x20; // BAR4 = 0x10 + 4*4`

```
// BAR4 = 0x10 + 4*4
```

## L28 · `const I801_WORD_DATA: u8 = 0x0C;`

```
// SMBHSTCNT transaction protocols / control bits.
```

## L33 · `const STS_HOST_BUSY: u8 = 1 << 0;`

```
// SMBHSTSTS status bits.
```

## L43 · `const SMBUS_READ: u8 = 1;`

```
// SMBus transfer direction in the address byte.
```

## L48-49 · `pub fn init() {`

```
/// Find the i801 SMBus controller, enable it, and cache its I/O base.
/// Logs + returns silently if absent — SMBus is a nice-to-have (battery).
```

## L57 · `let base = (pci::read32(addr, SMBBAR_OFFSET) & 0xFFFF_FFFC) as u16;`

```
// I/O BAR (BAR4): bit0 = I/O space marker, bit1 reserved — mask both off.
```

## L64 · `let cfg = pci::read32(addr, SMBHSTCFG);`

```
// Enable the host controller (HST_EN), preserving the rest of the dword.
```

## L74-76 · `pub fn base() -> Option<u16> {`

```
/// The cached SMBus I/O base, or None if no controller was found at boot.
/// Used by the `akku` diagnostic intent to tell "no controller" apart from
/// "controller present but no battery on the bus".
```

## L91-97 · `fn wait_intr(base: u16) -> Option<u8> {`

```
/// Poll SMBHSTSTS until the controller is idle with a result — mirrors
/// `i801_wait_intr`. Returns the latched error flags (0 = success) or None
/// on timeout. A healthy word read completes in a handful of 250 µs polls;
/// we cap at ~30 ms (vs Linux's 200 ms) deliberately — this runs on the
/// bar's fiber, so a wedged bus must not stall the clock for a fifth of a
/// second every poll. A best-effort battery read that times out just leaves
/// the segment stale until the next tick.
```

## L111-113 · `pub fn read_word(addr: u8, cmd: u8) -> Option<u16> {`

```
/// Read a 16-bit word from an SMBus device (SMBus READ_WORD protocol).
/// `addr` is the 7-bit address, `cmd` the register/command byte. Returns
/// None if the controller is absent, busy-stuck, or the device NAKs.
```

## L117-118 · `let pre = unsafe { inb(base + SMBHSTSTS) };`

```
// Pre-transaction: bail if the bus is wedged busy, then clear any
// lingering status flags (i801_check_pre).
```

## L133 · `unsafe {`

```
// Set up the word-data read and kick it off.
```

## L142 · `unsafe { outb(base + SMBHSTSTS, err); }`

```
// Clear the error so the next transaction starts clean.
```

## L150 · `let done = unsafe { inb(base + SMBHSTSTS) } & STATUS_FLAGS;`

```
// Clear the completion flags.
```

