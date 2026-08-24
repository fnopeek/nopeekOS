//! WAV — RIFF/PCM.
//!
//! Here to keep [`crate::source`] honest: the seam is only real once a
//! second format goes through it. Also the format a recording tool or a
//! decoder test drops on disk, so it costs little and earns its place.

use crate::source::{Info, Source, MAX_BLOCK_FRAMES};

pub struct Wav {
    data:   &'static [u8],  // the data chunk, nothing else
    fmt:    Fmt,
    info:   Info,
    frame:  u64,
    frames: u64,
}

#[derive(Clone, Copy)]
struct Fmt {
    channels:    u8,
    /// Bytes per frame across all channels — how the file itself steps.
    block_align: usize,
    bits:        u16,
    float:       bool,
}

pub fn looks_like(b: &[u8]) -> bool {
    b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WAVE"
}

fn le16(b: &[u8]) -> u16 { u16::from_le_bytes([b[0], b[1]]) }
fn le32(b: &[u8]) -> u32 { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) }

impl Wav {
    pub fn open(bytes: &'static [u8]) -> Option<Wav> {
        if !looks_like(bytes) { return None; }
        let mut p = 12;
        let mut parsed: Option<(Fmt, u32)> = None;
        while p + 8 <= bytes.len() {
            let id = &bytes[p..p + 4];
            let size = le32(&bytes[p + 4..]) as usize;
            let body = p + 8;
            if body + size > bytes.len() { break; }
            if id == b"fmt " && size >= 16 {
                let f = &bytes[body..];
                let tag = le16(f);
                // WAVE_FORMAT_EXTENSIBLE hides the real tag in its GUID.
                let tag = if tag == 0xFFFE && size >= 26 { le16(&f[24..]) } else { tag };
                if tag != 1 && tag != 3 { return None; }
                parsed = Some((
                    Fmt {
                        channels: le16(&f[2..]).clamp(1, 2) as u8,
                        block_align: le16(&f[12..]) as usize,
                        bits: le16(&f[14..]),
                        float: tag == 3,
                    },
                    le32(&f[4..]),
                ));
            }
            if id == b"data" {
                let (fmt, rate) = parsed?;
                if fmt.block_align == 0 || rate == 0 { return None; }
                let frames = (size / fmt.block_align) as u64;
                return Some(Wav {
                    data: &bytes[body..body + size],
                    fmt,
                    info: Info {
                        rate,
                        channels: fmt.channels,
                        total_frames: frames,
                        title: None,
                        artist: None,
                        kind: "WAV",
                        bitrate_kbps: 0,
                    },
                    frame: 0,
                    frames,
                });
            }
            // Chunks are word-aligned; an odd size carries a pad byte.
            p = body + size + (size & 1);
        }
        None
    }

    fn sample(&self, at: usize) -> f32 {
        let b = match self.data.get(at..) { Some(b) => b, None => return 0.0 };
        match (self.fmt.bits, self.fmt.float) {
            (8, _) if !b.is_empty()             => (b[0] as f32 - 128.0) / 128.0,
            (16, false) if b.len() >= 2         => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            (24, false) if b.len() >= 3         => {
                let v = ((b[2] as i32) << 24 | (b[1] as i32) << 16 | (b[0] as i32) << 8) >> 8;
                v as f32 / 8_388_608.0
            }
            (32, true) if b.len() >= 4          => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            (32, false) if b.len() >= 4         => le32(b) as i32 as f32 / 2_147_483_648.0,
            _ => 0.0,
        }
    }
}

impl Source for Wav {
    fn info(&self) -> &Info { &self.info }

    fn next_block(&mut self, out: &mut [f32]) -> usize {
        let ch = self.fmt.channels as usize;
        let step = self.fmt.bits as usize / 8;
        let left = self.frames.saturating_sub(self.frame) as usize;
        let want = MAX_BLOCK_FRAMES.min(left);
        for f in 0..want {
            let base = (self.frame as usize + f) * self.fmt.block_align;
            for c in 0..ch {
                out[f * ch + c] = self.sample(base + c * step);
            }
        }
        self.frame += want as u64;
        want
    }

    fn seek(&mut self, frame: u64) -> u64 {
        self.frame = frame.min(self.frames);
        self.frame
    }
}
