# `tools/wasm/beak-engine/src/webp/transform.rs` @ 5e0102684

## L1-2 · `static CONST1: i64 = 20091;`

```
//! Ported verbatim from `image-webp` 0.1.3 (image-rs, MIT OR Apache-2.0):
//! `src/transform.rs` has no imports at all, so nothing had to change.
```

## L8 · `fn fetch(block: &[i32], idx: usize) -> i64 {`

```
// The intermediate results may overflow the types, so we stretch the type.
```

## L13 · `assert!(block.len() >= 16);`

```
// Perform one lenght check up front to avoid subsequent bounds checks in this function
```

## L53 · `pub(crate) fn iwht4x4(block: &mut [i32]) {`

```
// 14.3
```

## L55 · `assert!(block.len() >= 16);`

```
// Perform one length check up front to avoid subsequent bounds checks in this function
```

