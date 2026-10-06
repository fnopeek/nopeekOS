//! The mailbox side: source frames in, 48 kHz S16 stereo out.
//!
//! The kernel mixer takes one format and only one (48 kHz, S16LE, stereo),
//! so every source rate meets a resampler here. It also holds the play
//! clock: the mailbox is a ring the driver drains on the HDA crystal, and
//! the app cannot read its fill level — so what has actually been *heard*
//! is tracked from the wall clock and corrected whenever the ring pushes
//! back. `submitted - played` is the buffered lead, and that number decides
//! both how far ahead we decode and how quickly a pause takes effect.

use crate::host;
use crate::resample::Resampler;

pub use crate::resample::MIX_RATE;
/// How far ahead of the speaker we keep the ring. Long enough to survive a
/// browser-sized hiccup on the worker core, short enough that pause and
/// seek feel immediate — the kernel ring itself holds 1.36 s.
pub const TARGET_LEAD_MS: u64 = 600;

/// Worst case one source block can become: 1152 frames at 8 kHz resampled
/// to 48 kHz. Sized so `push` never has to stop halfway through a block.
const OUT_FRAMES: usize = 7168;

pub struct Sink {
    slot:      i32,
    rs:        Resampler,
    out:       [i16; OUT_FRAMES * 2],
    /// Bytes written by `push`, and how many of them the kernel has taken.
    /// Bytes, not frames: `npk_audio_submit` reports what it accepted in
    /// bytes and is free to accept a partial frame. Counting in frames
    /// would round that away and shift the stream by one sample — which
    /// swaps left and right for the rest of the track.
    out_bytes: usize,
    out_sent:  usize,
    /// 48 kHz frames handed to the mailbox since the last resync.
    submitted: u64,
    /// Wall-clock time of the last ring read — the anchor from which
    /// `played_frames_at` interpolates.
    anchor_ms: i64,
    /// Position at the last `restart`. The clock must not fall below it:
    /// right after a seek the ring is empty, and `submitted - unheard` would
    /// lie before the seek target.
    base: u64,
    /// 48 kHz frames the driver has drained, from the wall clock.
    played:    u64,
    last_ms:   i64,
}

impl Sink {
    pub fn open() -> Option<Sink> {
        let slot = host::audio_open();
        if slot < 0 { return None; }
        let mut s = Sink::empty();
        s.slot = slot;
        Some(s)
    }

    fn empty() -> Sink {
        Sink {
            slot: -1,
            rs: Resampler::new(),
            out: [0; OUT_FRAMES * 2],
            out_bytes: 0,
            out_sent: 0,
            submitted: 0,
            base: 0,
            anchor_ms: 0,
            played: 0,
            last_ms: 0,
        }
    }

    /// Point the resampler at a new source rate and drop everything still
    /// buffered — used on track change and on seek. Closing and reopening
    /// the slot is the flush: the kernel offers no other way to discard a
    /// ring, and without it a seek would keep playing the old position for
    /// as long as the lead lasts.
    pub fn restart(&mut self, rate: u32, now_ms: i64, at_frame_48k: u64) {
        if self.slot >= 0 { host::audio_close(self.slot); }
        self.slot = host::audio_open();
        self.rs.restart(rate);
        self.out_bytes = 0;
        self.out_sent = 0;
        self.submitted = at_frame_48k;
        self.base = at_frame_48k;
        self.played = at_frame_48k;
        self.anchor_ms = now_ms;
        self.last_ms = now_ms;
    }

    /// A player with no slot — all four were taken. It renders, it just
    /// never makes a sound, and `load` says so instead of pretending.
    pub fn dead() -> Sink {
        let mut s = Sink::empty();
        s.slot = -1;
        s
    }

    pub fn ok(&self) -> bool { self.slot >= 0 }

    /// Bytes still waiting between mailbox and speaker.
    ///
    /// `audio_hda` keeps its own ring of 2 x 2048 frames = 16384 bytes, which
    /// sits behind what `npk_audio_buffered` reports. The kernel adds the same
    /// number for the microVM guest (`HDA_RING_BYTES` in `virtio_snd_pci.rs`);
    /// leaving it out runs the picture 85 ms ahead of the sound.
    const HDA_RING_FRAMES: u64 = 16_384 / 4;

    /// How often the ring is read. It corrects drift, and ten reads per second
    /// suffice: the card clock and the wall clock differ by ppm, i.e.
    /// microseconds per 100 ms.
    ///
    /// `npk_audio_buffered` takes `SLOTS.lock()`, the same lock the driver
    /// holds on another core while mixing 2048 frames, so reads are not free.
    const ANCHOR_EVERY_MS: i64 = 100;

    /// Advance the play clock. `playing` false freezes it, which is what
    /// makes a pause hold its position.
    ///
    /// Two sources with distinct jobs: the wall clock advances between reads,
    /// the ring re-anchors it regularly. Neither works alone — the wall clock
    /// drifts over a film, and each ring read takes a contended lock.
    pub fn tick(&mut self, now_ms: i64, playing: bool) {
        let dt = (now_ms - self.last_ms).max(0) as u64;
        self.last_ms = now_ms;
        if !playing { return; }

        // Between two anchors: advance.
        self.played = (self.played + dt * MIX_RATE as u64 / 1000).min(self.submitted);

        if now_ms - self.anchor_ms < Self::ANCHOR_EVERY_MS { return; }
        let ring = host::audio_buffered(self.slot);
        if ring < 0 {
            // No slot: only the wall clock remains.
            return;
        }
        self.anchor_ms = now_ms;
        let unheard = ring as u64 / 4 + Self::HDA_RING_FRAMES;
        self.played = self.submitted.saturating_sub(unheard).max(self.base);
    }

    /// Frames heard at `now_ms`, without a host call.
    ///
    /// The ring is the anchor and the wall clock interpolates between reads:
    /// `npk_audio_buffered` takes the lock the driver holds while mixing on
    /// another core, so reading it every tick would stall decoding. The wall
    /// clock drifts against the card clock, which is harmless as long as the
    /// next anchor catches it.
    pub fn played_frames_at(&self, now_ms: i64) -> u64 {
        let dt = (now_ms - self.last_ms).max(0) as u64;
        (self.played + dt * MIX_RATE as u64 / 1000).min(self.submitted)
    }

    /// 48 kHz frames buffered ahead of the speaker.
    pub fn lead_frames(&self) -> u64 { self.submitted.saturating_sub(self.played) }
    pub fn lead_ms(&self) -> u64 { self.lead_frames() * 1000 / MIX_RATE as u64 }
    /// 48 kHz frames the speaker has actually reached.
    pub fn played_frames(&self) -> u64 { self.played }

    /// Restart the play clock without touching the buffer — the resume side
    /// of a pause. Without it the first tick after a pause charges the whole
    /// paused stretch to the speaker and reports a phantom underrun.
    pub fn resume(&mut self, now_ms: i64) { self.last_ms = now_ms; }

    /// Resample + convert one source block. Only legal when nothing is
    /// pending; `out` is sized so a whole block always fits.
    pub fn push(&mut self, block: &[f32], frames: usize, channels: usize) {
        self.out_sent = 0;
        self.out_bytes = self.rs.push(block, frames, channels, &mut self.out) * 2;
    }

    /// Hand whatever is pending to the mailbox. Returns true when the
    /// buffer emptied; false means the ring is full and the caller must
    /// stop decoding until the next tick.
    pub fn flush(&mut self) -> bool {
        while self.out_sent < self.out_bytes {
            let left = self.out_bytes - self.out_sent;
            let n = host::audio_submit(self.slot, &pcm_bytes(&self.out)[self.out_sent..self.out_bytes]);
            if n <= 0 { return false; }
            let accepted = (n as usize).min(left);
            self.out_sent += accepted;
            self.submitted += (accepted / 4) as u64;
            if accepted < left { return false; }
        }
        self.out_bytes = 0;
        self.out_sent = 0;
        true
    }

    pub fn close(&mut self) {
        if self.slot >= 0 { host::audio_close(self.slot); }
        self.slot = -1;
    }
}

/// The samples as the little-endian bytes the mailbox takes.
fn pcm_bytes(s: &[i16]) -> &[u8] {
    // SAFETY: i16 has no padding and every byte pattern is a valid u8; the
    // byte slice covers exactly the samples and borrows them. wasm32 is
    // little-endian.
    unsafe { core::slice::from_raw_parts(s.as_ptr() as *const u8, s.len() * 2) }
}
