# `tools/wasm/sdk/widgets/tests/roundtrip.rs` @ 5e0102684

## L1-6 · `extern crate alloc;`

```
//! Wire round-trip tests for nopeek_widgets.
//!
//! These run on the host with std (via `cargo test` in this crate dir)
//! and verify that a tree encoded on the SDK side decodes back to the
//! same tree with the same byte layout we'll eventually deserialize in
//! the kernel.
```

## L94 · `let truncated = &bytes[..3]; // version byte + 2 bytes of postcard`

```
// version byte + 2 bytes of postcard
```

## L100-101 · `let t0 = Widget::Popover {`

```
// Reserved widget slots must still round-trip — only the compositor
// logs + rejects them, the wire format accepts them.
```

## L140-141 · `assert_eq!(Token::Surface as u8, 0);`

```
// Belt-and-braces — check_abi const-asserts should already guarantee
// this, but surface any drift in a legible test failure too.
```

## L155-156 · `let tree = Widget::Column {`

```
// All vocab-v2 modifiers in a single tree — proves wire indices are
// stable and nested Vec<Modifier> serializes correctly.
```

## L169 · `Modifier::Scale(264), // 1.03×`

```
// 1.03×
```

## L208 · `assert_eq!(`

```
// Motion is SDK-only sugar — wire form must remain Transition::Linear.
```

