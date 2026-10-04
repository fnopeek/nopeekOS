//! AAC from an MP4 — the audio half of the container.
//!
//! The decoder lives in `tools/wasm/vendor/rusty_aac` (see its VENDOR.md);
//! this file holds only what it does not want to know: where the frames sit
//! in the file, how long the stream is, and where a seek lands.
//!
//! An AAC frame carries 1024 samples per channel and so fits in
//! [`MAX_BLOCK_FRAMES`](crate::source::MAX_BLOCK_FRAMES) (1152).


use crate::mp4;
use crate::source::{Info, Source};

pub struct Aac {
    data: &'static [u8],
    track: mp4::Track,
    dec: rusty_aac::decode::Decoder,
    info: Info,
    /// Next frame `next_block` decodes.
    next: usize,
    /// Output position in frames at the source rate.
    frame: u64,
}

impl Aac {
    /// `track` must be the audio track of an already parsed MP4.
    pub fn from_track(data: &'static [u8], track: mp4::Track) -> Option<Aac> {
        let cfg_bytes = match &track.codec {
            mp4::Codec::Aac { config } => config.clone(),
            _ => return None,
        };
        let cfg = rusty_aac::parse_audio_specific_config(&cfg_bytes).ok()?;
        if cfg.sample_rate == 0 || cfg.channels == 0 { return None; }

        // The duration comes from the sample table, not an estimate, and
        // counts at the source rate because the caller continues with
        // `info().rate`.
        let total = track.duration as u128 * cfg.sample_rate as u128
            / track.timescale.max(1) as u128;

        // Bitrate from the actual bytes: sum of frame sizes over the duration.
        let bytes: u64 = track.samples.iter().map(|s| s.size as u64).sum();
        let ms = track.duration_ms().max(1);
        let kbps = (bytes * 8 / ms) as u32;

        Some(Aac {
            info: Info {
                rate: cfg.sample_rate,
                channels: cfg.channels.min(255) as u8,
                total_frames: total as u64,
                title: None,
                artist: None,
                kind: "AAC",
                bitrate_kbps: kbps,
            },
            dec: rusty_aac::decode::Decoder::new(cfg.sample_rate),
            data,
            track,
            next: 0,
            frame: 0,
        })
    }

    /// How many samples the stream skips before its first audible one.
    ///
    /// An AAC encoder starts with priming samples that are not part of the
    /// sound, and the container's edit list says how many; ffmpeg trims exactly
    /// `edit_start` of them. The video track of the same file usually has 0, so
    /// playing both timelines raw puts sound and picture apart (48 ms for 2112
    /// samples at 44.1 kHz).
    pub fn priming(&self) -> u64 {
        self.track.edit_start
    }
}

impl Source for Aac {
    fn info(&self) -> &Info { &self.info }

    fn next_block(&mut self, out: &mut [f32]) -> usize {
        let ch = self.info.channels.max(1) as usize;
        while self.next < self.track.samples.len() {
            let s = self.track.samples[self.next];
            self.next += 1;
            let Some(au) = self.data.get(s.offset..s.offset + s.size) else { break };
            // A rejected frame is a gap, not the end: the next one recovers,
            // because every AAC frame stands alone.
            let Ok(d) = self.dec.decode(au, None) else { continue };
            let frames = d.frames().min(out.len() / ch);
            if frames == 0 { continue; }
            out[..frames * ch].copy_from_slice(&d.samples[..frames * ch]);
            self.frame += frames as u64;
            return frames;
        }
        0
    }

    fn seek(&mut self, frame: u64) -> u64 {
        // Find the frame whose span contains `frame`. AAC-LC can start at
        // any frame; the first one after a seek sounds soft because it lacks
        // the overlap of its predecessor.
        let rate = self.info.rate.max(1) as u128;
        let ts = self.track.timescale.max(1) as u128;
        let want = frame as u128 * ts / rate;
        let mut idx = 0usize;
        for (i, s) in self.track.samples.iter().enumerate() {
            if s.dts as u128 > want { break; }
            idx = i;
        }
        self.dec = rusty_aac::decode::Decoder::new(self.info.rate);
        self.next = idx;
        let landed = self.track.samples.get(idx).map(|s| s.dts).unwrap_or(0) as u128;
        self.frame = (landed * rate / ts) as u64;
        self.frame
    }
}
