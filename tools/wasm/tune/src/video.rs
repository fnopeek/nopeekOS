//! The video half: MP4 samples in, one picture at the right moment out.
//!
//! Pictures come out of the decoder in DECODE order; a B-frame is decoded
//! before the frame it sits between. Which one to SHOW when is a question the
//! container already answers — every sample carries its presentation time —
//! so the reordering here is a small queue sorted by `pts`, not a second
//! calculation from picture order counts.

use alloc::vec::Vec;

use rusty_h264_common::YuvFrame;
use rusty_h264_decoder::Decoder;

use crate::mp4;

/// Pictures held before one is handed out. Four covers the reorder depth of
/// everything a camera or an encoder with default settings produces; a stream
/// that needs more shows a frame late rather than out of order, because the
/// queue always emits the smallest `pts` it holds.
const REORDER: usize = 4;

pub struct Video {
    data: &'static [u8],
    track: mp4::Track,
    dec: Decoder,
    /// Next sample to feed the decoder, in decode order.
    next: usize,
    /// Reused scratch for the Annex-B reframing, so a frame costs no
    /// allocation beyond the picture itself.
    au: Vec<u8>,
    /// Decoded but not yet shown, ascending by presentation time.
    queue: Vec<(i64, YuvFrame)>,
    /// The picture currently on screen, and when it starts.
    shown: Option<(i64, YuvFrame)>,
    pub width: u32,
    pub height: u32,
    pub rotation: u16,
    pub duration_ms: u64,
    /// Every sample has been fed. The queue may still hold pictures.
    fed_all: bool,
}

pub fn looks_like(d: &[u8]) -> bool {
    mp4::looks_like(d)
}

/// Extensions the folder listing accepts as video. Next to [`Video::open`]
/// so a new container is registered in one place, the same way `is_audio`
/// sits next to `source::open`.
pub fn is_video(name: &str) -> bool {
    let lower = {
        let mut s = alloc::string::String::with_capacity(name.len());
        for c in name.chars() { s.push(c.to_ascii_lowercase()); }
        s
    };
    [".mp4", ".m4v", ".mov"].iter().any(|e| lower.ends_with(e))
}

pub enum OpenError {
    /// Not an MP4, or the box tree did not parse.
    NotMp4,
    /// Fragmented: the sample tables live in the fragments, not in `moov`.
    Fragmented,
    /// No video track, or one we do not decode.
    NoVideo,
}

impl Video {
    pub fn open(data: &'static [u8]) -> Result<Video, OpenError> {
        let m = mp4::parse(data).ok_or(OpenError::NotMp4)?;
        if m.fragmented { return Err(OpenError::Fragmented); }
        let track = m.video.ok_or(OpenError::NoVideo)?;
        if !matches!(track.codec, mp4::Codec::Avc { .. }) || track.samples.is_empty() {
            return Err(OpenError::NoVideo);
        }
        let mut v = Video {
            width: track.width,
            height: track.height,
            rotation: track.rotation,
            duration_ms: track.duration_ms(),
            data,
            track,
            dec: Decoder::new(),
            next: 0,
            au: Vec::new(),
            queue: Vec::new(),
            shown: None,
            fed_all: false,
        };
        v.prime();
        Ok(v)
    }

    /// Hand the decoder its parameter sets. In MP4 they live in `avcC`, so a
    /// decoder fed only the samples never sees them and every picture fails.
    fn prime(&mut self) {
        self.au.clear();
        mp4::parameter_sets(&self.track.codec, &mut self.au);
        if !self.au.is_empty() {
            // No picture in a parameter set; a refusal here is not an error.
            let _ = self.dec.decode(&self.au);
        }
    }

    fn nal_len(&self) -> usize {
        match &self.track.codec { mp4::Codec::Avc { nal_len, .. } => *nal_len, _ => 4 }
    }

    /// Decode one more sample into the queue. False when there are none left.
    fn feed_one(&mut self) -> bool {
        if self.next >= self.track.samples.len() {
            self.fed_all = true;
            return false;
        }
        let s = self.track.samples[self.next];
        self.next += 1;
        let pts = self.track.pts_ms(&s);
        let Some(bytes) = self.data.get(s.offset..s.offset + s.size) else {
            self.fed_all = true;
            return false;
        };
        self.au.clear();
        mp4::to_annex_b(bytes, self.nal_len(), &mut self.au);
        // A sample the decoder refuses is one lost picture, not a lost film:
        // the next sync sample starts it again. Silence here would hide a
        // broken file, so the caller gets to log it.
        if let Ok(Some(f)) = self.dec.decode(&self.au) {
            // Insert sorted; the queue is four long, so this is cheaper than
            // keeping a heap and far cheaper than being wrong about order.
            let at = self.queue.partition_point(|(p, _)| *p <= pts);
            self.queue.insert(at, (pts, f));
        }
        true
    }

    /// The picture that should be on screen at `ms`, if it changed since the
    /// last call. `None` means "keep showing what you have".
    ///
    /// Decoding happens here and nowhere else, so the cost lands on the
    /// caller's clock and a slow frame shows up as a late frame rather than
    /// as a stalled event loop.
    pub fn frame_at(&mut self, ms: i64) -> Option<&YuvFrame> {
        // Keep the queue full enough that the next picture in PRESENTATION
        // order is really the smallest one we hold.
        while self.queue.len() < REORDER && self.feed_one() {}

        let mut advanced = false;
        while let Some(&(pts, _)) = self.queue.first() {
            if pts > ms { break; }
            // Only take it if the queue is deep enough to know it is the
            // earliest, or there is nothing left to come.
            if self.queue.len() < REORDER && !self.fed_all { break; }
            let (pts, f) = self.queue.remove(0);
            self.shown = Some((pts, f));
            advanced = true;
            while self.queue.len() < REORDER && self.feed_one() {}
        }
        if advanced { self.shown.as_ref().map(|(_, f)| f) } else { None }
    }

    /// Everything fed and nothing left to show.
    pub fn ended(&self) -> bool {
        self.fed_all && self.queue.is_empty()
    }

    /// Restart at the last sync sample at or before `ms`, and answer the time
    /// it really landed on. A decoder started anywhere else produces garbage
    /// until the next sync sample, so this is a jump to a key frame and the
    /// caller must believe the number it gets back.
    pub fn seek(&mut self, ms: i64) -> i64 {
        let i = self.track.sync_at_ms(ms.max(0) as u64);
        self.dec = Decoder::new();
        self.queue.clear();
        self.shown = None;
        self.next = i;
        self.fed_all = false;
        self.prime();
        self.track.pts_ms(&self.track.samples[i])
    }

    /// Row strides for `npk_canvas_commit_yuv`. The decoder's planes are
    /// tight, but the host call takes strides because a decoder is allowed
    /// not to be — asking here keeps that promise in one place.
    pub fn strides(f: &YuvFrame) -> (usize, usize) {
        (f.width, f.width.div_ceil(2))
    }
}
