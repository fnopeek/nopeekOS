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

/// Most samples one `frame_at` call may decode.
///
/// Without a cap this loop catches up until it reaches the wall clock, and
/// that is unbounded: a second behind at 30 fps is thirty decodes in ONE
/// call, and at 1440p thirty decodes are a second and a half in which the
/// event loop does not poll. The window stops answering, and „too slow"
/// looks like „hung" — two very different bugs.
///
/// With the cap the picture simply falls behind and the clock says by how
/// much. That is the honest symptom, and it is the one you can measure.
const MAX_DECODES_PER_CALL: usize = 4;

/// How far ahead of the clock to decode, in ms of PICTURE time.
///
/// Four frames of reorder depth is enough to put pictures in order and not
/// enough to absorb anything. The cost of decoding follows the BITS, not the
/// pixels: a day-to-night crossfade leaves every macroblock with a large
/// residual and costs several times what a talking head costs. The machine
/// holds the average and misses the peak — unless the cheap stretches, where
/// it is otherwise idle, are used to build a lead. That is what the audio
/// sink has done from the start (`sink::TARGET_LEAD_MS`).
///
/// **1000 ms, und die Zahl ist gemessen.** Florians Geraetelauf, 1440p30,
/// Tag/Nacht-Ueberblendung: der Rueckstand stieg auf **647 ms** und ging
/// danach wieder zurueck (424 -> 647 -> 314). Ein Loch, kein Dauerzustand —
/// also genau das, was ein Vorlauf schluckt, wenn er tief genug ist. Harte
/// Schnitte in derselben Datei liefen ohne Rueckstand durch: EIN teures Bild
/// faengt schon die Reihenfolge-Warteschlange ab, eine lange Kette nicht.
const TARGET_LEAD_MS: i64 = 1000;

/// And the real limit is BYTES, not frames.
///
/// 500 ms at 30 fps is fifteen pictures — 47 MB at 1080p and 83 MB at
/// 1440p. A cap counted in frames means the memory it costs depends on the
/// file, which is the same mistake as sizing a buffer by item count anywhere
/// else. So the lead is whichever comes first.
///
/// 128 MB reicht bei 1440p fuer 23 Bilder (775 ms) und bei 1080p fuer mehr,
/// als die Zeitgrenze zulaesst. Es ist viel, und es ist der Preis dafuer,
/// dass ein Ueberblendung nicht sichtbar wird; belegt wird es nur, wenn die
/// billigen Szenen davor Zeit uebrig hatten.
const MAX_QUEUE_BYTES: usize = 128 * 1024 * 1024;

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

    /// The picture that should be on screen at `ms`, if it changed since the
    /// last call. `None` means "keep showing what you have".
    ///
    /// Decoding happens here and nowhere else, so the cost lands on the
    /// caller's clock and a slow frame shows up as a late frame rather than
    /// as a stalled event loop.
    ///
    /// Frames that are already late are decoded and NOT shown — only the
    /// newest one that has come due reaches the caller. A picture cannot be
    /// skipped without decoding it (the next one predicts from it), but it
    /// can be skipped on the way to the screen, and that is where the whole
    /// cost of a commit sits.
    pub fn frame_at(&mut self, ms: i64) -> Option<&YuvFrame> {
        let mut budget = MAX_DECODES_PER_CALL;
        self.fill(ms, &mut budget);

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
            self.fill(ms, &mut budget);
            if budget == 0 { break; }
        }
        if advanced { self.shown.as_ref().map(|(_, f)| f) } else { None }
    }

    /// Decode ahead: deep enough to order pictures, then as far ahead of the
    /// clock as the lead allows, and never past the byte cap.
    fn fill(&mut self, ms: i64, budget: &mut usize) {
        while *budget > 0 {
            let need_order = self.queue.len() < REORDER;
            let lead = self.queue.last().map(|(p, _)| *p - ms).unwrap_or(0);
            let want_lead = lead < TARGET_LEAD_MS && self.queue_bytes < MAX_QUEUE_BYTES;
            if !need_order && !want_lead { break; }
            *budget -= 1;
            if !self.feed_one() { break; }
        }
    }

    /// Picture time the queue reaches beyond `ms`. Zero means the decoder is
    /// hand to mouth, and the next expensive scene will be visible.
    pub fn lead_ms(&self, ms: i64) -> i64 {
        self.queue.last().map(|(p, _)| p - ms).unwrap_or(0).max(0)
    }

    /// Presentation time of the picture on screen. The gap to the caller's
    /// clock IS the lag, and it is the number that says whether the machine
    /// keeps up.
    pub fn shown_ms(&self) -> i64 {
        self.shown.as_ref().map(|(p, _)| *p).unwrap_or(0)
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
