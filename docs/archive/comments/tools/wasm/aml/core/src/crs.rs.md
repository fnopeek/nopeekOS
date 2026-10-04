# `tools/wasm/aml/core/src/crs.rs` @ 5e0102684

## L1-14 · `use alloc::{string::String, vec::Vec};`

```
//! `_CRS`/`_PRS` resource templates — the binary form, decoded.
//!
//! Layouts 1:1 aus ACPICA `drivers/acpi/acpica/amlresrc.h` (Linux 6.18.26);
//! die Feldnamen sind dort nachzulesen. Nur die vier Deskriptoren, die ein
//! Treiber hier braucht, werden ausgepackt — alles andere wird UEBERSPRUNGEN
//! und nicht geraten, damit ein unbekannter Eintrag nicht den Rest der
//! Vorlage verschiebt.
//!
//! Ein Template ist eine Folge von Deskriptoren, klein oder gross:
//!
//! * **klein** — Byte 0 Bit 7 = 0, Laenge in Bit 2..0, gesamt `1 + len`
//! * **gross** — Byte 0 Bit 7 = 1, danach `u16` Laenge, gesamt `3 + len`
//!
//! Ende ist das kleine Tag `0x79`.
```

## L18 · `pub const TAG_MEMORY32_FIXED: u8 = 0x86;`

```
/// Descriptor-Type-Bytes (ACPI 6.5 §6.4.3), so wie sie im Puffer stehen.
```

## L25 · `pub const SERIAL_TYPE_I2C: u8 = 1;`

```
/// Serial-bus `type`-Feld (AML_RESOURCE_SERIAL_COMMON.type).
```

## L28 · `pub const GPIO_CONN_INTERRUPT: u8 = 0;`

```
/// GPIO `connection_type`.
```

## L34 · `Memory32Fixed { address: u32, length: u32 },`

```
/// `Memory32Fixed` — wie ein FCH-I2C seinen Registerblock ansagt.
```

## L36-37 · `ExtendedIrq { flags: u8, interrupts: Vec<u32> },`

```
/// `Interrupt (...)` — die GSI(s). Wir routen sie heute nicht (kein
/// IOAPIC), aber sie gehoeren in den Bericht.
```

## L39-40 · `I2cSerialBus {`

```
/// `I2cSerialBus (...)` — Slave-Adresse, Busfrequenz und der ACPI-PFAD
/// des Controllers (`resource_source`).
```

## L44 · `flags: u8,`

```
/// Bit 0 der `flags`: 1 = der Verbraucher spricht, 0 = Producer.
```

## L49 · `Gpio {`

```
/// `GpioInt` / `GpioIo` — Pin-Liste und der Pfad des GPIO-Controllers.
```

## L57 · `Other { tag: u8, len: usize },`

```
/// Alles andere: nur das Tag, damit ein Bericht vollstaendig bleibt.
```

## L78-81 · `fn source_str(b: &[u8], off: usize, end: usize) -> String {`

```
/// Eine NUL-begrenzte Zeichenkette ab `off`, hoechstens bis `end`.
///
/// ACPICA liest `resource_source` genauso: der Rest des Deskriptors hinter
/// den festen Feldern, bis zur Null. Fehlt die Null, gilt der Rest.
```

## L97-100 · `pub fn parse(buf: &[u8]) -> Vec<Resource> {`

```
/// Ein Ressourcen-Template in seine Deskriptoren zerlegen.
///
/// Bricht ab beim End-Tag, bei einer Laenge von null (sonst laeuft die
/// Schleife ewig) und am Pufferende.
```

## L107 · `let len = (tag & 0x07) as usize;`

```
// Kleiner Deskriptor: Laenge in den unteren drei Bits.
```

## L110 · `break; // End tag: kleiner Typ 0x0F`

```
// End tag: kleiner Typ 0x0F
```

## L116 · `if i + 3 > buf.len() {`

```
// Grosser Deskriptor.
```

## L145 · `let connection_type = if d.len() > 4 { d[4] } else { 0 };`

```
// amlresrc.h struct aml_resource_gpio
```

## L152-153 · `let pin_end = if res_source_offset > pin_table_offset {`

```
// Die Pin-Liste laeuft bis zur Quellzeichenkette (oder, wenn
// die fehlt, bis zu den Herstellerdaten).
```

## L181 · `let bus_type = if d.len() > 5 { d[5] } else { 0 };`

```
// AML_RESOURCE_SERIAL_COMMON ab Offset 3.
```

## L187-189 · `let src_off = 12 + type_data_length;`

```
// aml_resource_i2c_serialbus: connection_speed @12, slave @16.
// Die Quellzeichenkette folgt hinter den typspezifischen
// Daten — deren Laenge steht in `type_data_length`.
```

## L214-216 · `#[test]`

```
/// Ein von Hand gebautes Template: Memory32Fixed + Extended IRQ + Ende.
/// Die Zahlen stammen aus der Struktur, nicht aus einer Messung — der
/// Test prueft den DEKODIERER, nicht die Firmware.
```

## L220 · `0x86, 0x09, 0x00, // Memory32Fixed, len 9`

```
// Memory32Fixed, len 9
```

## L221 · `0x01, // flags (writable)`

```
// flags (writable)
```

## L222 · `0x00, 0x20, 0xDC, 0xFE, // address 0xFEDC2000`

```
// address 0xFEDC2000
```

## L223 · `0x00, 0x10, 0x00, 0x00, // length 0x1000`

```
// length 0x1000
```

## L226 · `0x89, 0x06, 0x00, // Extended IRQ, len 6`

```
// Extended IRQ, len 6
```

## L227 · `0x03, // flags`

```
// flags
```

## L228 · `0x01, // count`

```
// count
```

## L229 · `0x0A, 0x00, 0x00, 0x00, // GSI 10`

```
// GSI 10
```

## L248-249 · `#[test]`

```
/// I2cSerialBus mit Quellzeichenkette — der Fall, an dem die
/// Controller-Zuordnung haengt.
```

## L253 · `let type_data_length = 6usize; // connection_speed + slave_address`

```
// connection_speed + slave_address
```

## L254 · `let body_len = 9 + type_data_length + src.len(); // ab revision_id`

```
// ab revision_id
```

## L258 · `b.push(0x01); // revision_id`

```
// revision_id
```

## L259 · `b.push(0x00); // res_source_index`

```
// res_source_index
```

## L261 · `b.push(0x00); // flags`

```
// flags
```

## L262 · `b.extend_from_slice(&[0x00, 0x00]); // type_specific_flags`

```
// type_specific_flags
```

## L263 · `b.push(0x01); // type_revision_id`

```
// type_revision_id
```

## L282 · `#[test]`

```
/// Ein unbekannter grosser Deskriptor darf den Rest nicht verschieben.
```

