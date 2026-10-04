# `tools/wasm/beak-engine/src/webp/loop_filter.rs` @ 5e0102684

## L1-2 · `#[inline]`

```
//! Ported verbatim from `image-webp` 0.1.3 (image-rs, MIT OR Apache-2.0):
//! `src/loop_filter.rs` has no imports at all, so nothing had to change.
```

## L4 · `#[inline]`

```
//! Does loop filtering on webp lossy images
```

## L11 · `#[inline]`

```
//unsigned to signed
```

## L17 · `#[inline]`

```
//signed to unsigned
```

## L32 · `fn common_adjust(use_outer_taps: bool, pixels: &mut [u8], point: usize, stride: usize) -> i32 {`

```
//15.2
```

## L39 · `let outer = if use_outer_taps { c(p1 - q1) } else { 0 };`

```
//value for the outer 2 pixels
```

## L81-82 · `pub(crate) fn simple_segment(edge_limit: u8, pixels: &mut [u8], point: usize, stride: usize) {`

```
//simple filter
//effects 4 pixels on an edge(2 each side)
```

## L89-90 · `pub(crate) fn subblock_filter(`

```
//normal filter
//works on the 8 pixels on the edges between subblocks inside a macroblock
```

## L111-112 · `pub(crate) fn macroblock_filter(`

```
//normal filter
//works on the 8 pixels on the edges between macroblocks
```

