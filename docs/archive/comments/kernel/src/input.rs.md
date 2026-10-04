# `kernel/src/input.rs` @ 5e0102684

## L1-5 · `#[derive(Clone, Copy, Debug)]`

```
//! Unified input events — KeyEvent replaces raw u8 bytes throughout the system.
//!
//! All keyboard input (PS/2, USB/xHCI) is converted to KeyEvent at the driver
//! level. Consumers (intent loop, shade compositor, WASM apps) work with
//! KeyEvent instead of raw scancodes or ANSI escape sequences.
```

## L7 · `#[derive(Clone, Copy, Debug)]`

```
/// A single keyboard event with full context.
```

## L10 · `pub key: KeyCode,`

```
/// Logical key code (always set).
```

## L12 · `pub modifiers: Modifiers,`

```
/// Modifier state at time of keypress.
```

## L16-20 · `#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`

```
/// Logical key codes — hardware-independent.
///
/// Wire-stable: variant order and field shape are part of the widget ABI
/// (Phase 10, `shade::widgets::abi::Event::Key`). Append-only; never
/// reorder. Mirrored in the SDK at `nopeek_widgets::abi::KeyCode`.
```

## L23 · `Char(u8),`

```
/// Printable ASCII character (already layout-converted).
```

## L42 · `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]`

```
/// Modifier key state at time of keypress.
```

## L52 · `#[allow(dead_code)]`

```
/// Empty event for array initialization.
```

## L54-70 · `pub struct Utf8Tail {`

```
/// Der Rest einer UTF-8-Folge, die byteweise durch die Tastaturleitung muss.
///
/// **Ein Tastendruck traegt ein BYTE, und `ü` sind zwei.** Die ganze Kette
/// (`push_key` → `KeyCode::Char(u8)` → jede App) auf Zeichen umzustellen
/// hiesse, das ABI zu aendern und jede ausgelieferte App zu einem
/// Bindefehler zu machen. Also bleibt sie byteweise: das erste Byte geht
/// sofort, der Rest liegt hier und wird VOR dem naechsten Tastendruck
/// abgeholt. Fuer ASCII ist der Puffer immer leer und der Weg genau der von
/// vorher.
///
/// **Ein TYP mit zwei Instanzen, keine zwei Kopien.** Es gibt zwei
/// Tastaturtreiber — PS/2 (`drivers::keyboard`) und USB-HID
/// (`drivers::xhci`) —, und genau daran ist die Umlaut-Umstellung beim ersten
/// Anlauf gescheitert: die PS/2-Tabelle war repariert, die Maschine hing an
/// USB, und die Zeichen kamen weiter als `;` `[` `'`. Zwei Instanzen, weil
/// die beiden unabhaengige Erzeuger sind; eine Rechnung, damit sie nicht
/// auseinanderlaufen ([[feedback_a_copy_is_a_second_semantics_waiting]]).
```

## L79 · `pub fn split(&mut self, c: char) -> u8 {`

```
/// Ein Zeichen einreichen und sein ERSTES Byte bekommen; der Rest wartet.
```

## L83 · `return c as u8;      // ASCII: genau der Weg von frueher`

```
// ASCII: genau der Weg von frueher
```

## L93-95 · `pub fn take(&mut self) -> Option<u8> {`

```
/// Das naechste wartende Byte. **Muss vor dem naechsten Tastendruck
/// gerufen werden** — sonst geht die zweite Haelfte eines Zeichens
/// dahinter verloren.
```

## L108 · `pub const fn char(c: u8, modifiers: Modifiers) -> Self {`

```
/// Create a KeyEvent for a printable character.
```

## L113 · `pub const fn special(key: KeyCode, modifiers: Modifiers) -> Self {`

```
/// Create a KeyEvent for a special key.
```

## L118-119 · `pub fn to_ascii(&self) -> Option<u8> {`

```
/// Convert to ASCII byte for backwards compatibility (WASM apps, serial).
/// Returns None for non-printable keys (arrows, F-keys, etc.).
```

## L132 · `pub fn is_printable(&self) -> bool {`

```
/// True if this is a printable character (not a special key).
```

## L134-135 · `matches!(self.key, KeyCode::Char(c) if c >= 0x20 && c != 0x7F)`

```
// Ab 0x80 ist es ein Stueck einer UTF-8-Folge — also Text, auch
// wenn es allein kein Zeichen ist. 0x7F (DEL) ist keiner.
```

## L139 · `pub fn has_mod(&self) -> bool {`

```
/// True if Mod (Super) key is held.
```

## L150 · `pub fn current() -> Self {`

```
/// Read current modifier state from keyboard driver atomics.
```

## L155 · `alt: false, // no separate Alt tracking yet (AltGr covers Right Alt)`

```
// no separate Alt tracking yet (AltGr covers Right Alt)
```

## L156 · `alt_gr: crate::keyboard::is_alt_held(), // is_alt_held actually tracks AltGr`

```
// is_alt_held actually tracks AltGr
```

