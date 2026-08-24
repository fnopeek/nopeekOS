//! MP3 — MPEG-1/2/2.5 Layer III, decoded by `nanomp3` (minimp3).
//!
//! This file is everything around the decoder that the decoder refuses to
//! care about: ID3 tags at both ends, the Xing/Info header that turns a VBR
//! file into a duration and a seek table, and the byte-level seek itself.

use alloc::string::{String, ToString};
use nanomp3::{Decoder, MAX_SAMPLES_PER_FRAME};

use crate::source::{Info, Source};

pub struct Mp3 {
    /// Audio bytes only — ID3v2 at the front and ID3v1 at the back removed,
    /// so byte fractions map to time fractions.
    data:  &'static [u8],
    pos:   usize,
    dec:   Decoder,
    info:  Info,
    frame: u64,
    /// Byte offset of every `stride`-th frame, built by walking the frame
    /// headers once at open. Costs one pass over ~9000 headers and gives
    /// what no tag does: an exact duration and a seek that lands on the
    /// frame asked for. The Xing table it replaces is quantised to 1/256
    /// of the file — measured 832 ms off on a four-minute VBR track.
    index:  alloc::vec::Vec<u32>,
    stride: u32,
    /// Samples per frame, constant within a stream.
    spf:    u32,
}

pub fn looks_like(b: &[u8]) -> bool {
    if b.len() >= 3 && &b[..3] == b"ID3" { return true; }
    // A bare frame sync: 11 set bits, layer III, a defined bitrate.
    b.len() >= 4 && b[0] == 0xFF && (b[1] & 0xE6) == 0xE2
}

/// Fields of an MPEG audio frame header, or `None` if these four bytes are
/// not one. Reserved values (bitrate 0/15, rate index 3) count as not-one:
/// treating them as a frame is how a resync lands in the middle of audio.
struct Header {
    rate:          u32,
    bitrate_kbps:  u32,
    channels:      u8,
    /// 1152 for MPEG-1, 576 for MPEG-2/2.5.
    spf:           u32,
    /// Bytes between the header and a Xing tag, if there is one.
    side_info:     usize,
}

const BITRATES_V1_L3: [u32; 16] =
    [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
const BITRATES_V2_L3: [u32; 16] =
    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
const RATES: [[u32; 3]; 3] = [
    [44100, 48000, 32000], // MPEG-1
    [22050, 24000, 16000], // MPEG-2
    [11025, 12000, 8000],  // MPEG-2.5
];

fn parse_header(b: &[u8]) -> Option<Header> {
    if b.len() < 4 || b[0] != 0xFF || (b[1] & 0xE0) != 0xE0 { return None; }
    let version = (b[1] >> 3) & 3;      // 0=2.5, 1=reserved, 2=MPEG-2, 3=MPEG-1
    let layer = (b[1] >> 1) & 3;        // 1 = Layer III
    if version == 1 || layer != 1 { return None; }
    let bitrate_idx = (b[2] >> 4) as usize;
    let rate_idx = ((b[2] >> 2) & 3) as usize;
    if rate_idx == 3 || bitrate_idx == 0 || bitrate_idx == 15 { return None; }
    let mpeg1 = version == 3;
    let row = match version { 3 => 0, 2 => 1, _ => 2 };
    let mono = (b[3] >> 6) & 3 == 3;
    Some(Header {
        rate: RATES[row][rate_idx],
        bitrate_kbps: if mpeg1 { BITRATES_V1_L3[bitrate_idx] } else { BITRATES_V2_L3[bitrate_idx] },
        channels: if mono { 1 } else { 2 },
        spf: if mpeg1 { 1152 } else { 576 },
        side_info: match (mpeg1, mono) {
            (true, true)   => 17,
            (true, false)  => 32,
            (false, true)  => 9,
            (false, false) => 17,
        },
    })
}

/// First frame header at or after `from`, scanning at most `limit` bytes.
fn find_header(data: &[u8], from: usize, limit: usize) -> Option<usize> {
    let end = (from + limit).min(data.len().saturating_sub(4));
    let mut i = from;
    while i < end {
        if data[i] == 0xFF && parse_header(&data[i..]).is_some() { return Some(i); }
        i += 1;
    }
    None
}

impl Mp3 {
    pub fn open(bytes: &'static [u8]) -> Option<Mp3> {
        let (title, artist, start) = read_id3v2(bytes);
        let mut end = bytes.len();
        // ID3v1 is 128 trailing bytes that are not audio; leaving them in
        // makes every byte-fraction seek land slightly late and adds a
        // burst of noise at the end of the file.
        if end >= start + 128 && &bytes[end - 128..end - 125] == b"TAG" {
            end -= 128;
        }
        let (t1, a1) = if title.is_none() || artist.is_none() {
            read_id3v1(&bytes[end..])
        } else {
            (None, None)
        };
        let data = &bytes[start..end];

        let first = find_header(data, 0, 128 * 1024)?;
        let h = parse_header(&data[first..])?;

        // Xing/Info sits inside the first frame, right after the side info.
        // It is the only place a VBR file states its length.
        //
        // The two names are not synonyms: "Xing" means the stream is VBR,
        // "Info" means it is CBR. That distinction decides how to seek —
        // see below.
        let mut audio_start = first;
        let tag_at = first + 4 + h.side_info;
        if data.len() >= tag_at + 8 {
            let tag = &data[tag_at..tag_at + 4];
            if tag == b"Xing" || tag == b"Info" {
                // Neither the frame count nor the seek table is read: the
                // scan below knows both exactly. What the tag is needed for
                // is that its own frame is silence — leaving it in prepends
                // a frame of nothing and shifts every position by one frame.
                audio_start = first + frame_len(&h, &data[first..]);
            }
        }
        // From here on `data` is audio and nothing else, so a byte fraction
        // IS a time fraction. Anything left in front of it — tags, the Xing
        // frame, junk before the first sync — would otherwise shift every
        // seek by its own size.
        let data = &data[audio_start.min(data.len())..];
        let (count, index, stride) = scan(data);
        let total_frames = count as u64 * h.spf as u64;
        // Average, not the first frame's nominal rate: on a VBR file the
        // first frame is often the quietest and reads as "64 kbps" for a
        // track that averages 130.
        let bitrate_kbps = match total_frames {
            0 => h.bitrate_kbps,
            _ => (data.len() as u64 * 8 * h.rate as u64 / (total_frames * 1000)) as u32,
        };

        Some(Mp3 {
            data,
            pos: 0,
            dec: Decoder::new(),
            info: Info {
                rate: h.rate,
                channels: h.channels,
                total_frames,
                title: title.or(t1),
                artist: artist.or(a1),
                kind: "MP3",
                bitrate_kbps,
            },
            frame: 0,
            index,
            stride,
            spf: h.spf,
        })
    }
}

/// Encoded length of the frame whose header starts at `b`.
fn frame_len(h: &Header, b: &[u8]) -> usize {
    let padding = ((b[2] >> 1) & 1) as usize;
    let spf_bytes = h.spf as usize / 8; // 144 for MPEG-1, 72 for MPEG-2
    spf_bytes * h.bitrate_kbps as usize * 1000 / h.rate as usize + padding
}

/// Walk every frame header once. Returns (frame count, sparse byte index,
/// frames per index entry). Roughly nine thousand iterations of pointer
/// arithmetic for a four-minute track — three orders of magnitude below
/// what decoding one second costs.
fn scan(data: &[u8]) -> (u32, alloc::vec::Vec<u32>, u32) {
    const MAX_ENTRIES: usize = 4096;
    let mut index: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    let mut stride: u32 = 16;
    let mut at = 0usize;
    let mut n: u32 = 0;
    while at + 4 <= data.len() {
        let h = match parse_header(&data[at..]) {
            Some(h) => h,
            // Garbage between frames happens (embedded tags, splice points).
            // Resync rather than declaring the file over.
            None => match find_header(data, at + 1, 64 * 1024) {
                Some(next) => { at = next; continue; }
                None => break,
            },
        };
        if n % stride == 0 {
            if index.len() == MAX_ENTRIES {
                // Halve the resolution instead of growing without bound:
                // a two-hour podcast keeps the same 16 KB of index a
                // four-minute song has.
                let mut i = 0;
                index.retain(|_| { i += 1; i % 2 == 1 });
                stride *= 2;
            }
            index.push(at as u32);
        }
        let len = frame_len(&h, &data[at..]);
        if len == 0 { break; }
        at += len;
        n += 1;
    }
    (n, index, stride)
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

impl Source for Mp3 {
    fn info(&self) -> &Info { &self.info }

    fn next_block(&mut self, out: &mut [f32]) -> usize {
        debug_assert!(out.len() >= MAX_SAMPLES_PER_FRAME);
        loop {
            if self.pos >= self.data.len() { return 0; }
            let (used, info) = self.dec.decode(&self.data[self.pos..], out);
            if used == 0 { self.pos = self.data.len(); return 0; }
            self.pos += used.min(self.data.len() - self.pos);
            // A frame the decoder skipped as garbage returns bytes but no
            // samples — keep going rather than reporting end of stream.
            if let Some(i) = info {
                self.frame += i.samples_produced as u64;
                return i.samples_produced;
            }
        }
    }

    fn seek(&mut self, frame: u64) -> u64 {
        let total = self.info.total_frames;
        if total == 0 || self.index.is_empty() { return self.frame; }
        let want = frame.min(total) / self.spf as u64;          // frame number
        let entry = (want / self.stride as u64) as usize;
        let entry = entry.min(self.index.len() - 1);
        let mut at = self.index[entry] as usize;
        let mut n = entry as u64 * self.stride as u64;
        // Walk the remaining `stride` headers by hand: the index is coarse
        // on purpose, and this makes the landing exact.
        while n < want && at < self.data.len() {
            match parse_header(&self.data[at..]) {
                Some(h) => {
                    let len = frame_len(&h, &self.data[at..]);
                    if len == 0 { break; }
                    at += len;
                    n += 1;
                }
                None => break,
            }
        }
        self.pos = at.min(self.data.len());
        // The bit reservoir means the first frame after a seek may refer to
        // data we skipped. Starting from a clean decoder keeps that to one
        // frame of quiet instead of a burst from the previous position.
        self.dec = Decoder::new();
        self.frame = n * self.spf as u64;
        self.frame
    }
}

// ── ID3 ───────────────────────────────────────────────────────────────

/// Returns (title, artist, offset of the first audio byte).
fn read_id3v2(b: &[u8]) -> (Option<String>, Option<String>, usize) {
    if b.len() < 10 || &b[..3] != b"ID3" { return (None, None, 0); }
    let major = b[3];
    let flags = b[5];
    let size = syncsafe(&b[6..10]) as usize;
    let mut end = 10 + size;
    if flags & 0x10 != 0 { end += 10; } // footer
    let end = end.min(b.len());
    if major < 3 {
        // v2.2 uses 3-byte frame ids. Rare enough that skipping the tag and
        // showing the file name beats a second parser.
        return (None, None, end);
    }

    let mut title = None;
    let mut artist = None;
    let mut p = 10;
    // An extended header, if present, is announced by its own length.
    if flags & 0x40 != 0 && p + 4 <= end {
        let ext = if major >= 4 { syncsafe(&b[p..p + 4]) as usize } else { be32(&b[p..]) as usize + 4 };
        p += ext;
    }
    while p + 10 <= end {
        let id = &b[p..p + 4];
        if id == [0, 0, 0, 0] { break; } // padding
        let fsize = if major >= 4 { syncsafe(&b[p + 4..p + 8]) as usize } else { be32(&b[p + 4..]) as usize };
        let body = p + 10;
        if fsize == 0 || body + fsize > end { break; }
        if id == b"TIT2" { title = decode_text(&b[body..body + fsize]); }
        if id == b"TPE1" { artist = decode_text(&b[body..body + fsize]); }
        p = body + fsize;
        if title.is_some() && artist.is_some() { break; }
    }
    (title, artist, end)
}

fn read_id3v1(tag: &[u8]) -> (Option<String>, Option<String>) {
    if tag.len() < 128 || &tag[..3] != b"TAG" { return (None, None); }
    let field = |s: &[u8]| -> Option<String> {
        let t: String = s.iter().take_while(|&&c| c != 0)
            .map(|&c| c as char).collect();
        let t = t.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    };
    (field(&tag[3..33]), field(&tag[33..63]))
}

/// Syncsafe integer — seven bits per byte, so a tag length can never
/// contain a byte that looks like a frame sync.
fn syncsafe(b: &[u8]) -> u32 {
    ((b[0] as u32 & 0x7F) << 21) | ((b[1] as u32 & 0x7F) << 14)
        | ((b[2] as u32 & 0x7F) << 7) | (b[3] as u32 & 0x7F)
}

/// ID3 text frame: one encoding byte, then the text. All four encodings
/// appear in the wild; a title is worth 30 lines.
fn decode_text(f: &[u8]) -> Option<String> {
    let (enc, body) = f.split_first()?;
    let s: String = match enc {
        0 => body.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect(),
        3 => {
            let cut = body.iter().position(|&c| c == 0).unwrap_or(body.len());
            core::str::from_utf8(&body[..cut]).ok()?.to_string()
        }
        1 | 2 => {
            let (be, start) = match body {
                [0xFF, 0xFE, ..] => (false, 2),
                [0xFE, 0xFF, ..] => (true, 2),
                _ => (*enc == 2, 0),
            };
            let mut out = String::new();
            let mut i = start;
            while i + 1 < body.len() {
                let u = if be { u16::from_be_bytes([body[i], body[i + 1]]) }
                        else { u16::from_le_bytes([body[i], body[i + 1]]) };
                if u == 0 { break; }
                // Surrogate pairs would need the next unit; music metadata
                // outside the BMP is rare enough to render as a placeholder.
                out.push(char::from_u32(u as u32).unwrap_or('?'));
                i += 2;
            }
            out
        }
        _ => return None,
    };
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
