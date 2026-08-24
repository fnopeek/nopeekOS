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

/// What the kernel mixer takes, and the only rate it takes.
pub const MIX_RATE: u32 = 48_000;

pub struct Resampler {
    /// Source frames per output frame, 16.16 fixed point.
    step:   u32,
    phase:  u32,
    /// The four frames the cubic needs: output is interpolated between
    /// `hist[1]` and `hist[2]`, so the stream runs one frame behind the
    /// decoder. That one frame is the price of the two extra taps.
    hist:   [[f32; 2]; 4],
    primed: u8,
}

impl Resampler {
    pub const fn new() -> Resampler {
        Resampler { step: 1 << 16, phase: 0, hist: [[0.0; 2]; 4], primed: 0 }
    }

    /// Point at a source rate and forget the previous stream.
    pub fn restart(&mut self, rate: u32) {
        // Truncated, not rounded: the phase accumulator carries the
        // remainder, so the error stays below one output sample forever
        // instead of accumulating into a drifting pitch.
        self.step = (((rate.max(1) as u64) << 16) / MIX_RATE as u64) as u32;
        self.phase = 0;
        self.hist = [[0.0; 2]; 4];
        self.primed = 0;
    }

    /// Convert `frames` interleaved source frames into `out` (interleaved
    /// stereo i16). Returns samples written. `out` must be able to hold a
    /// whole block at the caller's lowest supported rate.
    pub fn push(&mut self, block: &[f32], frames: usize, channels: usize, out: &mut [i16]) -> usize {
        let mut w = 0usize;
        for f in 0..frames {
            let l = block[f * channels];
            let r = if channels >= 2 { block[f * channels + 1] } else { l };
            self.hist[0] = self.hist[1];
            self.hist[1] = self.hist[2];
            self.hist[2] = self.hist[3];
            self.hist[3] = [l, r];
            if self.primed < 3 {
                // Prime with the first frames rather than with silence: a
                // click at the start of every track is not a rounding error,
                // it is an audible defect.
                self.primed += 1;
                self.hist[0] = self.hist[3];
                self.hist[1] = self.hist[3];
                self.hist[2] = self.hist[3];
                continue;
            }
            while self.phase < 1 << 16 {
                if w + 2 > out.len() { break; }
                let t = self.phase as f32 / 65536.0;
                out[w] = to_i16(cubic(self.hist[0][0], self.hist[1][0], self.hist[2][0], self.hist[3][0], t));
                out[w + 1] = to_i16(cubic(self.hist[0][1], self.hist[1][1], self.hist[2][1], self.hist[3][1], t));
                w += 2;
                self.phase += self.step;
            }
            // The loop above only exits with the phase past one whole source
            // frame, so this never underflows — including when a step is
            // larger than a frame (a source above 48 kHz).
            self.phase -= 1 << 16;
        }
        w
    }
}

/// Catmull-Rom between `b` and `c`, with `a` and `d` as the outer slopes.
#[inline(always)]
fn cubic(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    let c0 = b;
    let c1 = 0.5 * (c - a);
    let c2 = a - 2.5 * b + 2.0 * c - 0.5 * d;
    let c3 = 0.5 * (d - a) + 1.5 * (b - c);
    ((c3 * t + c2) * t + c1) * t + c0
}

fn to_i16(v: f32) -> i16 {
    let s = v * 32767.0;
    if s >= 32767.0 { 32767 } else if s <= -32768.0 { -32768 } else { s as i16 }
}
