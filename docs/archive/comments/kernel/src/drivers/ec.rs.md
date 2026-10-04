# `kernel/src/drivers/ec.rs` @ 5e0102684

## L1-14 · `use crate::serial::{inb, outb};`

```
//! ACPI Embedded Controller (EC) — polling reads of EC RAM.
//!
//! Ported from Linux `drivers/acpi/ec.c` (polling path). The EC is the
//! microcontroller that owns battery / thermal / lid state on laptops; its
//! 256-byte RAM is read one byte at a time via the RD_EC command over the
//! ISA ports 0x62 (data) / 0x66 (status+command). On HP (and most x86) the
//! battery lives here as plain EC-RAM fields (remaining/full capacity,
//! status) that the DSDT's `_BST`/`_BIF` read.
//!
//! Reads race-tolerantly share the EC with SMM/firmware. [`write`] is
//! firmware-directed only: the AML interpreter runs the DSDT's own
//! `_BST`/`_BIF`, which writes the battery-select field (BSEL) before reading
//! the multiplexed capacity registers — exactly as ACPICA drives the EC. We
//! never poke arbitrary offsets ourselves.
```

## L18 · `const EC_DATA: u16 = 0x62;`

```
// Default ISA EC ports (FADT EC_BLK; 0x62/0x66 is universal on x86 laptops).
```

## L20 · `const EC_SC: u16 = 0x66; // status (read) / command (write)`

```
// status (read) / command (write)
```

## L22 · `const EC_FLAG_OBF: u8 = 0x01; // output buffer full → data ready to read`

```
// Status register bits (ec.c).
```

## L23 · `const EC_FLAG_OBF: u8 = 0x01; // output buffer full → data ready to read`

```
// output buffer full → data ready to read
```

## L24 · `const EC_FLAG_IBF: u8 = 0x02; // input buffer full → controller still busy`

```
// input buffer full → controller still busy
```

## L26 · `const CMD_READ: u8 = 0x80; // RD_EC`

```
// RD_EC
```

## L27 · `const CMD_WRITE: u8 = 0x81; // WR_EC`

```
// WR_EC
```

## L28 · `const CMD_QUERY: u8 = 0x84; // QR_EC`

```
// QR_EC
```

## L30 · `const EC_FLAG_SCI: u8 = 0x20;`

```
/// Statusbit: der EC hat ein EREIGNIS zu melden (ec.c: `ACPI_EC_FLAG_SCI`).
```

## L43-44 · `fn wait_ibf_clear() -> bool {`

```
/// Wait until the input buffer is clear (controller ready for the next
/// byte). ~10 ms cap. Returns false on timeout.
```

## L53 · `fn wait_obf_set() -> bool {`

```
/// Wait until the output buffer is full (a read result is available).
```

## L62-63 · `pub fn read(addr: u8) -> Option<u8> {`

```
/// Read one byte from EC RAM at `addr`. None on timeout. Polling RD_EC:
/// IBF-clear → cmd 0x80 → IBF-clear → addr → OBF-set → data.
```

## L73-88 · `pub fn query() -> Option<u8> {`

```
/// Eine anstehende EC-ABFRAGE abholen (`QR_EC`), oder `None`.
///
/// Der EC meldet Ereignisse — Akku rein/raus, Netzteil, Deckel, Tasten —
/// indem er Bit 5 seines Statusregisters setzt. Das Betriebssystem holt
/// daraufhin mit `QR_EC` (0x84) eine Ereignisnummer ab und ruft im AML
/// `_Q<nr>`. Erst DORT traegt die Firmware ihren Zustand nach; eine DSDT
/// mit 56 solchen Behandlern (gemessen auf einem Lenovo IdeaPad) haengt
/// ihre halbe Geraeteverwaltung daran.
///
/// Wir haben das nie getan, und deshalb stand dort auf einem Notebook mit
/// vollem Akku `_STA = 0x0F` — Geraet da, Bit4 frei, also "kein Akku
/// eingelegt". Die Firmware war nie gefragt worden.
///
/// Weg aus Linux `drivers/acpi/acpica`/`ec.c`, Abfragepfad: Statusbit
/// pruefen, Kommando schreiben, EIN Byte lesen. Null heisst "nichts
/// anliegend" (ec.c behandelt 0 ausdruecklich als leere Abfrage).
```

## L90 · `if unsafe { inb(EC_SC) } & EC_FLAG_SCI == 0 { return None; }`

```
// SAFETY: ring-0 ISA-Portzugriff auf den EC, wie im ganzen Modul.
```

## L99-100 · `pub fn query_raw() -> Result<u8, &'static str> {`

```
/// `query` without folding the outcomes together, for `ec watch take`:
/// Ok(0) = the EC answered "nothing pending", Err = which step timed out.
```

## L108 · `pub fn read_u16(addr: u8) -> Option<u16> {`

```
/// Read a little-endian 16-bit word from EC RAM (addr = low byte).
```

## L115-117 · `pub fn write(addr: u8, val: u8) -> bool {`

```
/// Write one byte to EC RAM at `addr`. Polling WR_EC:
/// IBF-clear → cmd 0x81 → IBF-clear → addr → IBF-clear → data. Returns false
/// on timeout. See the module note on firmware-directed-only writes.
```

