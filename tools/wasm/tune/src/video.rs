//! The video half: MP4 samples in, one picture at the right moment out.
//!
//! Pictures come out of the decoder in decode order; a B-frame is decoded
//! before the frame it sits between. Which one to show when is a question the
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

/// Decodes per call while filling up, before the clock runs.
///
/// Only here may a call take several: nobody is watching yet, and the
/// point is to get a lead before the first picture moves.
pub const FILL_DECODES: usize = 4;

/// Decodes per call while playing.
///
/// A turn of the caller's loop shows at most one picture, so the display
/// rate is the turn rate, and every extra decode makes the turn longer and
/// lets pictures fall due unseen. With one, a turn is one decode plus one
/// commit and fits inside a frame period. When decoding is slower than that
/// (an expensive scene), the lead drains instead — which is what a lead is
/// for — and the picture keeps its rate until the lead is gone.
pub const PLAY_DECODES: usize = 1;

/// How far ahead of the clock to decode, in ms of picture time.
///
/// Four frames of reorder depth is enough to put pictures in order and not
/// enough to absorb anything. The cost of decoding follows the bits, not the
/// pixels: a crossfade leaves every macroblock with a large residual and
/// costs several times what a talking head costs. The machine holds the
/// average and misses the peak — unless the cheap stretches, where it is
/// otherwise idle, are used to build a lead. The audio sink does the same
/// (`sink::TARGET_LEAD_MS`).
///
/// The value is sized from the frame deficit of an expensive scene, with
/// about twice its margin; at high resolutions the byte cap binds first.
const TARGET_LEAD_MS: i64 = 1500;

/// The real limit is bytes, not frames.
///
/// A cap counted in frames makes its memory cost depend on the file: 500 ms
/// at 30 fps is 47 MB at 1080p and 83 MB at 1440p. So the lead is whichever
/// limit comes first.
///
/// A 1440p I420 picture is 5.27 MB, so 256 MB hold 48 pictures = 1600 ms —
/// just above the deficit of an expensive crossfade, so the time limit
/// binds rather than memory. The memory is only filled when the cheaper
/// scenes before had time to spare, and shrinks with resolution by itself.
const MAX_QUEUE_BYTES: usize = 256 * 1024 * 1024;

/// Lead to build before the clock starts. Less than the target, because the
/// rest can be built while playing and nobody wants to wait a second for a
/// three-second clip.
const PREROLL_MS: i64 = 400;

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
    /// Bytes the queue holds, tracked rather than recomputed: the planes do
    /// not change size, and walking them per call to add up three `len()`s
    /// would be work that grows with the lead we are trying to build.
    queue_bytes: usize,
    /// The picture currently on screen, and when it starts.
    shown: Option<(i64, YuvFrame)>,
    pub width: u32,
    pub height: u32,
    pub rotation: u16,
    pub duration_ms: u64,
    /// Flags for `npk_canvas_commit_yuv`: bit 0 = Rec. 709, bit 1 = full range.
    pub colour_flags: i32,
    /// Every sample has been fed. The queue may still hold pictures.
    fed_all: bool,
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
    /// `track` must be the video track of an already parsed MP4. The container
    /// is parsed once (see `demux`), so audio and video cannot disagree about it.
    pub fn from_track(data: &'static [u8], track: mp4::Track) -> Result<Video, OpenError> {
        if !matches!(track.codec, mp4::Codec::Avc { .. }) || track.samples.is_empty() {
            return Err(OpenError::NoVideo);
        }
        let mut v = Video {
            width: track.width,
            height: track.height,
            rotation: track.rotation,
            duration_ms: track.duration_ms(),
            colour_flags: (track.colour.bt709 as i32) | ((track.colour.full_range as i32) << 1),
            data,
            track,
            dec: Decoder::new(),
            next: 0,
            au: Vec::new(),
            queue: Vec::new(),
            queue_bytes: 0,
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
            self.queue_bytes += f.y.len() + f.u.len() + f.v.len();
            self.queue.insert(at, (pts, f));
        }
        true
    }

    /// Decode ahead. Separate from showing: between the two, the caller must
    /// re-read the clock, otherwise pictures that came due during decoding are
    /// judged against a stale time.
    pub fn decode_step(&mut self, ms: i64, max_decodes: usize) {
        let mut budget = max_decodes;
        self.fill(ms, &mut budget);
    }

    /// The picture that is due now; everything before it is dropped.
    ///
    /// Frames that are already late are dropped rather than shown: a picture
    /// cannot be skipped without decoding it (the next one predicts from
    /// it), but it can be skipped on the way to the screen, and that is
    /// where the whole cost of a commit sits.
    ///
    /// Costs no decode budget: dropping a late picture is a `remove`, so all
    /// due pictures are consumed in one call.
    pub fn take_due(&mut self, ms: i64) -> Option<&YuvFrame> {
        let mut advanced = false;
        while let Some(&(pts, _)) = self.queue.first() {
            if pts > ms { break; }
            // Only take it if the queue is deep enough to know it is the
            // earliest, or there is nothing left to come.
            if self.queue.len() < REORDER && !self.fed_all { break; }
            let (pts, f) = self.queue.remove(0);
            self.queue_bytes = self.queue_bytes
                .saturating_sub(f.y.len() + f.u.len() + f.v.len());
            self.shown = Some((pts, f));
            advanced = true;
        }
        if advanced {
            self.shown.as_ref().map(|(_, f)| f)
        } else {
            None
        }
    }

    fn fill(&mut self, ms: i64, budget: &mut usize) {
        // Reorder depth is an obligation, not a reserve: without it the
        // queue does not know which picture comes next. So it comes before
        // the budget, not out of it.
        while self.queue.len() < REORDER {
            if !self.feed_one() { break; }
        }
        while *budget > 0 {
            let lead = self.queue.last().map(|(p, _)| *p - ms).unwrap_or(0);
            if lead >= TARGET_LEAD_MS || self.queue_bytes >= MAX_QUEUE_BYTES { break; }
            *budget -= 1;
            if !self.feed_one() { break; }
        }
    }

    /// May the caller sleep instead of giving the turn to decoding?
    ///
    /// Only when the reserve is full. Skipping decodes to favour showing trades
    /// dropped pictures for late ones, and a late picture is what the eye sees:
    /// the picture stalls and jumps. A dropped one is invisible. The caller
    /// shows first and decodes after, so both happen in every turn.
    pub fn may_wait(&self, ms: i64) -> bool {
        self.stocked(ms)
    }


    /// Is the reserve full? Only then may the caller sleep.
    ///
    /// Decoding runs faster than real time on average; sleeping in that gap
    /// throws away the headroom that should become lead for the next expensive
    /// scene.
    pub fn stocked(&self, ms: i64) -> bool {
        self.lead_ms(ms) >= TARGET_LEAD_MS || self.queue_bytes >= MAX_QUEUE_BYTES
    }

    /// Time until the next due picture. The caller sleeps until then — a fixed
    /// sleep can push a turn past the frame period and drop exactly that frame.
    pub fn next_due_in(&self, ms: i64) -> i64 {
        self.queue.first().map(|(p, _)| p - ms).unwrap_or(0).max(0)
    }

    /// Enough decoded to start without stumbling.
    ///
    /// The first second is the one stretch where the decoder has no head
    /// start at all, so it is the one stretch where waiting is free.
    pub fn primed(&self, ms: i64) -> bool {
        self.lead_ms(ms) >= PREROLL_MS || self.fed_all
    }

    /// Picture time the queue reaches beyond `ms`. Zero means the decoder is
    /// hand to mouth, and the next expensive scene will be visible.
    pub fn lead_ms(&self, ms: i64) -> i64 {
        self.queue.last().map(|(p, _)| p - ms).unwrap_or(0).max(0)
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
        self.queue_bytes = 0;
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
