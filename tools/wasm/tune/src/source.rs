//! The format seam.
//!
//! The player knows exactly one thing about a file: it can be turned into
//! interleaved f32 frames at some sample rate. Everything format-specific —
//! containers, tags, seek tables — lives behind [`Source`]. Adding a format
//! means adding a `Source` and one line in [`open`]; the player, the
//! resampler and the UI stay untouched.

use alloc::boxed::Box;
use alloc::string::String;

/// Largest block a `Source` may return, in frames. One MPEG-1 Layer III
/// granule pair; every other format we add chops itself to fit.
pub const MAX_BLOCK_FRAMES: usize = 1152;
/// Interleaved sample capacity the caller must provide to `next_block`.
pub const MAX_BLOCK_SAMPLES: usize = MAX_BLOCK_FRAMES * 2;

pub struct Info {
    pub rate:         u32,
    pub channels:     u8,
    /// Total frames at `rate`, or 0 when the format won't say. A zero here
    /// means the UI shows elapsed time only and seeking is disabled — never
    /// a guessed duration.
    pub total_frames: u64,
    pub title:        Option<String>,
    pub artist:       Option<String>,
    /// Shown in the footer: "MP3", "WAV".
    pub kind:         &'static str,
    /// 0 when the format has no meaningful bitrate (uncompressed).
    pub bitrate_kbps: u32,
}

impl Info {
    pub fn duration_ms(&self) -> u64 {
        if self.rate == 0 { return 0; }
        self.total_frames * 1000 / self.rate as u64
    }
}

pub trait Source {
    fn info(&self) -> &Info;

    /// Decode the next block into `out` (interleaved, `channels` samples per
    /// frame). Returns frames written; 0 means end of stream.
    ///
    /// `out` is always at least [`MAX_BLOCK_SAMPLES`] long.
    fn next_block(&mut self, out: &mut [f32]) -> usize;

    /// Jump to `frame` (at `info().rate`). Best effort — returns the frame
    /// actually landed on, which is what the caller must believe afterwards.
    fn seek(&mut self, frame: u64) -> u64;
}

/// Pick a decoder by content, not by file name. A `.mp3` that is really a
/// RIFF file plays; a truncated one is refused here rather than three
/// layers down.
pub fn open(bytes: &'static [u8]) -> Option<Box<dyn Source>> {
    if crate::wav::looks_like(bytes) {
        return crate::wav::Wav::open(bytes).map(|s| Box::new(s) as Box<dyn Source>);
    }
    if crate::mp3::looks_like(bytes) {
        return crate::mp3::Mp3::open(bytes).map(|s| Box::new(s) as Box<dyn Source>);
    }
    None
}

/// Extensions the folder listing accepts. Kept next to [`open`] so a new
/// format is registered in one place.
pub fn is_audio(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".mp3", ".wav"].iter().any(|e| lower.ends_with(e))
}
