# `tools/hpack-test/src/hostile.rs` @ 5e0102684

## L1-3 · `use crate::hpack::{encode, Decoder};`

```
//! The decoder parses bytes straight off the network inside the kernel, where
//! a panic is a halt. These assert the only property that really matters
//! there: it may reject anything, but it must never panic.
```

## L10 · `#[test]`

```
/// Every prefix of a valid block is a truncation a peer could actually send.
```

## L22 · `let _ = d.decode(&bytes[..n]); // must return, panic-free`

```
// must return, panic-free
```

## L27-28 · `#[test]`

```
/// A cheap deterministic sweep of the instruction space: every first byte,
/// followed by assorted tails. Catches indexing panics in the prefix decode.
```

## L49-50 · `#[test]`

```
/// A length prefix far larger than the block must be refused, not trusted
/// into a huge allocation or an out-of-range slice.
```

## L53 · `let mut d = Decoder::new();`

```
// literal, new name, length 0x7fffffff, no payload
```

## L58 · `#[test]`

```
/// EOS must not decode as a symbol, and padding must be an EOS prefix (§5.2).
```

## L62 · `assert!(d.decode(&hx("0082 0000".replace(' ', "").as_str())).is_err());`

```
// literal name, Huffman string whose padding bits are zeros, not ones
```

## L66 · `#[test]`

```
/// A dynamic index nobody ever inserted.
```

## L70 · `assert!(d.decode(&hx("be")).is_err()); // idx 62, table empty`

```
// idx 62, table empty
```

## L72 · `assert!(d.decode(&hx("80")).is_err()); // idx 0 is forbidden`

```
// idx 0 is forbidden
```

## L75 · `#[test]`

```
/// What we encode, we must be able to decode back.
```

