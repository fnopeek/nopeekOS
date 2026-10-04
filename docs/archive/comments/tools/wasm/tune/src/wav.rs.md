# `tools/wasm/tune/src/wav.rs` @ 5e0102684

## L1-5 · `use crate::source::{Info, Source, MAX_BLOCK_FRAMES};`

```
//! WAV — RIFF/PCM.
//!
//! Here to keep [`crate::source`] honest: the seam is only real once a
//! second format goes through it. Also the format a recording tool or a
//! decoder test drops on disk, so it costs little and earns its place.
```

## L10 · `data:   &'static [u8],  // the data chunk, nothing else`

```
// the data chunk, nothing else
```

## L20 · `block_align: usize,`

```
/// Bytes per frame across all channels — how the file itself steps.
```

## L46 · `let tag = if tag == 0xFFFE && size >= 26 { le16(&f[24..]) } else { tag };`

```
// WAVE_FORMAT_EXTENSIBLE hides the real tag in its GUID.
```

## L79 · `p = body + size + (size & 1);`

```
// Chunks are word-aligned; an odd size carries a pad byte.
```

