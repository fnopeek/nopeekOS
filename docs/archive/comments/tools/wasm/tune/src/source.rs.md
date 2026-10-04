# `tools/wasm/tune/src/source.rs` @ 5e0102684

## L1-7 · `use alloc::boxed::Box;`

```
//! The format seam.
//!
//! The player knows exactly one thing about a file: it can be turned into
//! interleaved f32 frames at some sample rate. Everything format-specific —
//! containers, tags, seek tables — lives behind [`Source`]. Adding a format
//! means adding a `Source` and one line in [`open`]; the player, the
//! resampler and the UI stay untouched.
```

## L12-13 · `pub const MAX_BLOCK_FRAMES: usize = 1152;`

```
/// Largest block a `Source` may return, in frames. One MPEG-1 Layer III
/// granule pair; every other format we add chops itself to fit.
```

## L15 · `pub const MAX_BLOCK_SAMPLES: usize = MAX_BLOCK_FRAMES * 2;`

```
/// Interleaved sample capacity the caller must provide to `next_block`.
```

## L21-23 · `pub total_frames: u64,`

```
/// Total frames at `rate`, or 0 when the format won't say. A zero here
/// means the UI shows elapsed time only and seeking is disabled — never
/// a guessed duration.
```

## L27 · `pub kind:         &'static str,`

```
/// Shown in the footer: "MP3", "WAV".
```

## L29 · `pub bitrate_kbps: u32,`

```
/// 0 when the format has no meaningful bitrate (uncompressed).
```

## L43-46 · `fn next_block(&mut self, out: &mut [f32]) -> usize;`

```
/// Decode the next block into `out` (interleaved, `channels` samples per
/// frame). Returns frames written; 0 means end of stream.
///
/// `out` is always at least [`MAX_BLOCK_SAMPLES`] long.
```

## L49-50 · `fn seek(&mut self, frame: u64) -> u64;`

```
/// Jump to `frame` (at `info().rate`). Best effort — returns the frame
/// actually landed on, which is what the caller must believe afterwards.
```

## L54-56 · `pub fn open(bytes: &'static [u8]) -> Option<Box<dyn Source>> {`

```
/// Pick a decoder by content, not by file name. A `.mp3` that is really a
/// RIFF file plays; a truncated one is refused here rather than three
/// layers down.
```

## L67-68 · `pub fn is_audio(name: &str) -> bool {`

```
/// Extensions the folder listing accepts. Kept next to [`open`] so a new
/// format is registered in one place.
```

