# `tools/wasm/tune/src/resample.rs` @ 5e0102684

## L1-15 · `pub const MIX_RATE: u32 = 48_000;`

```
//! Source rate → 48 kHz S16 stereo, cubic (Catmull-Rom) interpolation.
//!
//! Split out of [`crate::sink`] so it touches no host function and can be
//! run and measured off the device: the harness in `tools/wasm/tune/tests`
//! compiles THIS file, not a second copy of the same arithmetic.
//!
//! Why cubic and not linear: measured against ffmpeg's polyphase resampler
//! on 30 s of 44.1 kHz material, linear interpolation lost 2.1 dB across
//! 10–15 kHz and added 3.9 dB of imaging above 15 kHz. Four taps instead of
//! two cost about 1 % of a core on the device and take the error back into
//! the tenths of a dB.
//!
//! Known limit: a source ABOVE 48 kHz (only WAV can be) is decimated with
//! no low-pass, so anything it carries above 24 kHz folds back. Nothing in
//! MP3 can reach that case.
```

## L17 · `pub const MIX_RATE: u32 = 48_000;`

```
/// What the kernel mixer takes, and the only rate it takes.
```

## L21 · `step:   u32,`

```
/// Source frames per output frame, 16.16 fixed point.
```

## L24-26 · `hist:   [[f32; 2]; 4],`

```
/// The four frames the cubic needs: output is interpolated between
/// `hist[1]` and `hist[2]`, so the stream runs one frame behind the
/// decoder. That one frame is the price of the two extra taps.
```

## L36 · `pub fn restart(&mut self, rate: u32) {`

```
/// Point at a source rate and forget the previous stream.
```

## L38-40 · `self.step = (((rate.max(1) as u64) << 16) / MIX_RATE as u64) as u32;`

```
// Truncated, not rounded: the phase accumulator carries the
// remainder, so the error stays below one output sample forever
// instead of accumulating into a drifting pitch.
```

## L47-49 · `pub fn push(&mut self, block: &[f32], frames: usize, channels: usize, out: &mut [i16]) -> usize {`

```
/// Convert `frames` interleaved source frames into `out` (interleaved
/// stereo i16). Returns samples written. `out` must be able to hold a
/// whole block at the caller's lowest supported rate.
```

## L60-62 · `self.primed += 1;`

```
// Prime with the first frames rather than with silence: a
// click at the start of every track is not a rounding error,
// it is an audible defect.
```

## L77-79 · `self.phase -= 1 << 16;`

```
// The loop above only exits with the phase past one whole source
// frame, so this never underflows — including when a step is
// larger than a frame (a source above 48 kHz).
```

## L86 · `#[inline(always)]`

```
/// Catmull-Rom between `b` and `c`, with `a` and `d` as the outer slopes.
```

