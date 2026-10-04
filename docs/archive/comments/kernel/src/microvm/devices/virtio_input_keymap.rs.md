# `kernel/src/microvm/devices/virtio_input_keymap.rs` @ 5e0102684

## L1-16 · `use crate::input::{KeyCode, KeyEvent};`

```
//! KeyEvent → Linux evdev key sequence for the guest's virtio-input.
//!
//! Our `KeyEvent` is *logical*: `Char(u8)` is already layout-converted
//! ASCII (Shift baked in by the host keyboard driver), specials arrive
//! as `KeyCode::Enter/Tab/Up/...`, and `Ctrl+letter` arrives as the
//! control byte (0x01..0x1A) with `modifiers.ctrl`. The guest runs the
//! default **US** xkb map, so we translate the *desired character* to
//! the US evdev keycode (+ whether Shift must be held) that produces
//! it, then wrap Ctrl/Alt from the modifier snapshot.
//!
//! Emission follows the proven virtio-input discipline (v0.169.3):
//! every state change is its own `EV_KEY` + `SYN_REPORT` frame, and
//! every press has a matching release (the input core de-dupes a
//! press of an already-down key). Modifiers are pressed before / and
//! released after the main key, each in its own frame, so xkb sees
//! the modifier down when it processes the key.
```

## L25 · `const KEY_ESC: u16 = 1;`

```
// Linux input-event-codes.h — only the subset we emit.
```

## L43 · `const KEY_F1: u16 = 59;  // F1..F10 = 59..68`

```
// F1..F10 = 59..68
```

## L47 · `fn frame(code: u16, down: bool) {`

```
/// One state change = one `EV_KEY` frame + its `SYN_REPORT`.
```

## L53-55 · `fn ascii_to_key(c: u8) -> Option<(u16, bool)> {`

```
/// ASCII (0x20..=0x7E) → (US evdev keycode, needs Shift). The Shift
/// here comes from the *character* (US layout), not the host's
/// physical Shift state (already folded into the char upstream).
```

## L57-58 · `let pair = match c {`

```
// KEY_* values inline — this table is the spec, naming each would
// just add 50 one-use consts.
```

## L126-129 · `pub fn forward_key(ev: &KeyEvent) -> bool {`

```
/// Translate one host `KeyEvent` into the guest evdev frames. Returns
/// false if the key has no mapping (caller drops it). Sends a full
/// press+release for the key (we only get press events from the
/// driver), wrapped by Ctrl/Alt/Shift held/released around it.
```

## L133-134 · `KeyCode::Char(c) if (0x20..0x7F).contains(&c) => {`

```
// Printable: char already encodes Shift; honor only Ctrl/Alt
// from the snapshot (e.g. Ctrl++ zoom, Alt+d address bar).
```

## L142 · `KeyCode::Char(c) if (1..=26).contains(&c) => {`

```
// Ctrl+letter arrives as the control byte (^A=1..^Z=26).
```

