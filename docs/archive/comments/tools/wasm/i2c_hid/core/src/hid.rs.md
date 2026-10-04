# `tools/wasm/i2c_hid/core/src/hid.rs` @ 5e0102684

## L1-5 · `use crate::dw_i2c::{self, Bus, Error, Msg};`

```
//! HID over I2C — das Protokoll auf dem Bus.
//!
//! Portiert aus Linux 6.18.26 `drivers/hid/i2c-hid/i2c-hid-core.c`. Die
//! Reihenfolge ist die des Originals, und die Wartezeiten sind seine:
//! sie stehen dort nicht in der Spezifikation, sondern sind gemessen.
```

## L10 · `#[derive(Clone, Copy, Debug, Default)]`

```
/// `struct i2c_hid_desc` (i2c-hid-core.c) — 30 Bytes, little-endian.
```

## L52-56 · `pub fn valid(&self) -> Result<(), String> {`

```
/// `i2c_hid_fetch_hid_descriptor` prueft genau diese zwei Dinge.
///
/// **Beides ist eine echte Probe, keine Formalitaet:** ein Geraet, das
/// gar nicht antwortet, liefert lauter Nullen oder lauter Einsen — und
/// beides faellt hier durch.
```

## L79-84 · `pub fn probe_address(bus: &mut dyn Bus, dw: &dw_i2c::Dw, addr: u16) -> Result<(), Error> {`

```
/// `i2c_hid_probe_address` — ein Byte lesen, und bei Fehlschlag nach
/// 400 µs noch einmal.
///
/// Manche STM- und Weida-Geraete brauchen nach einer steigenden Taktflanke
/// diese Zeit, um aus dem Tiefschlaf zu kommen; der erste Versuch schlaegt
/// dann fehl. Steht im Original mit genau dieser Begruendung.
```

## L98-99 · `pub fn read_register(`

```
/// `i2c_hid_read_register` — zwei Bytes Registeradresse schreiben, dann
/// `len` Bytes lesen. Ein einziger Transfer mit Restart dazwischen.
```

## L108 · `pub fn fetch_descriptor(`

```
/// `i2c_hid_fetch_hid_descriptor`.
```

## L120 · `pub const OPCODE_RESET: u8 = 0x01;`

```
/// Opcodes aus dem HID-over-I2C-Protokoll (i2c-hid-core.c).
```

## L128 · `fn encode_command(buf: &mut Vec<u8>, opcode: u8, report_type: u8, report_id: u8) {`

```
/// `i2c_hid_encode_command`.
```

## L140-146 · `pub fn set_power(`

```
/// `i2c_hid_set_power`.
///
/// **Die 60 ms danach stehen nicht in der Spezifikation.** Der Kommentar
/// im Original sagt es woertlich: nach PWR_ON soll das GERAET den Takt
/// dehnen, aber Windows wartet 1 ms, Goodix-Geraete brauchen 60 — und
/// mehrere Geraete arbeiten ohne diese Pause nicht richtig. Gemessen, nicht
/// hergeleitet, und deshalb uebernommen.
```

## L161 · `bus.udelay(500);`

```
// Dieselbe 400-µs-Geschichte wie bei `probe_address`.
```

## L172-182 · `pub fn set_report(`

```
/// `i2c_hid_set_or_send_report` mit `do_set = true` — ein FEATURE-Bericht
/// an das Geraet.
///
/// Gebraucht fuer genau eine Sache, aber eine wichtige: den „Device Mode"
/// eines Praezisions-Touchpads auf 3 zu stellen. Ohne diesen Schalter
/// meldet es sich wie eine Maus und liefert gar keine Mehrfingerdaten —
/// Zweifinger-Scrollen ist dann nicht schwer, sondern unmoeglich.
///
/// Die Form ist die des Originals: Befehlsregister, SET_REPORT mit Typ
/// und Berichtsnummer, dann die Adresse des DATENregisters, dann der
/// Bericht mit seiner Laenge davor (`i2c_hid_format_report`).
```

## L195-197 · `let mut body: Vec<u8> = vec![0, 0];`

```
// `i2c_hid_format_report`: Laenge zuerst, dann — wenn es eine gibt —
// die Berichtsnummer, dann die Daten. Die Laenge zaehlt sich SELBST
// mit.
```

## L210-214 · `pub fn reset(`

```
/// `i2c_hid_start_hwreset` + `i2c_hid_finish_hwreset`.
///
/// Der Reset meldet sich zurueck, indem das Geraet ein Eingaberegister mit
/// LAENGE NULL bereitstellt. Linux wartet darauf ueber den Interrupt; wir
/// lesen das Register, bis die Null kommt oder die Zeit ablaeuft.
```

## L231 · `let deadline = bus.now_us() + 1_000_000;`

```
// Auf die Null-Laenge warten (Linux: 1 s).
```

## L241 · `bus.note("i2c-hid: device did not ack reset within 1000 ms");`

```
// Linux warnt hier nur und macht weiter.
```

## L248 · `set_power(bus, dw, addr, d, PWR_ON).map_err(|e| format!("power on after reset: {e:?}"))?;`

```
// „At least some SIS devices need this after reset."
```

## L253-257 · `pub fn get_input<'a>(`

```
/// `i2c_hid_get_input` — ein Eingabebericht, wenn einer anliegt.
///
/// Gelesen wird die in `wMaxInputLength` angesagte Groesse; die ersten
/// zwei Bytes sind die tatsaechliche Laenge. **Null heisst „nichts da"**
/// (oder: ein Reset ist fertig), und das ist der Normalfall beim Pollen.
```

## L269 · `if n == 0xFFFF || n > want || n < 2 { return Ok(None); }`

```
// 0xFFFF ist ein bekannter Muellwert (I2C_HID_QUIRK_BOGUS_IRQ).
```

## L278-279 · `#[test]`

```
/// Ein Deskriptor, wie ein Elan-Touchpad ihn liefert — und die beiden
/// Proben, die Linux daran macht.
```

## L283 · `raw[0] = 30; // wHIDDescLength`

```
// wHIDDescLength
```

## L284 · `raw[2] = 0x00; raw[3] = 0x01; // bcdVersion 1.00`

```
// bcdVersion 1.00
```

## L285 · `raw[8] = 0x03; // wInputRegister`

```
// wInputRegister
```

## L290 · `assert!(HidDesc::parse(&[0u8; 30]).valid().is_err());`

```
// Ein Geraet, das nicht antwortet, liefert lauter Nullen …
```

## L292 · `assert!(HidDesc::parse(&[0xFFu8; 30]).valid().is_err());`

```
// … oder lauter Einsen.
```

## L296 · `#[test]`

```
/// `i2c_hid_encode_command`: ab Report-ID 15 wird sie ein drittes Byte.
```

