//! MP4 / ISO base media file format — the container, not a codec.
//!
//! Written rather than pulled in: this is the first code to touch bytes a
//! stranger wrote, so it is the piece we most want to own. Every read goes
//! through `Cur`, which returns `None` past the end instead of panicking —
//! a panic here is a kernel halt.
//!
//! What it does: walk the box tree, find each track's sample table, and turn
//! it into a flat `Vec<Sample>` of (offset, size, dts, pts, sync). That is
//! everything a player needs; the rest of the format is metadata we skip.
//!
//! What it does NOT do, and says so instead of guessing: fragmented MP4
//! (`moof`), where the sample table lives in the fragments rather than in
//! `moov`.

use alloc::vec::Vec;

/// A bounds-checked cursor. Every getter answers `None` past the end, so a
/// truncated or hostile file ends as a refusal and never as a panic.
struct Cur<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn new(d: &'a [u8]) -> Cur<'a> { Cur { d, p: 0 } }
    fn at(d: &'a [u8], p: usize) -> Cur<'a> { Cur { d, p } }
    fn left(&self) -> usize { self.d.len().saturating_sub(self.p) }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.p = self.p.checked_add(n)?;
        if self.p > self.d.len() { None } else { Some(()) }
    }
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let b = self.d.get(self.p..self.p + 2)?;
        self.p += 2;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let b = self.d.get(self.p..self.p + 4)?;
        self.p += 4;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let hi = self.u32()? as u64;
        let lo = self.u32()? as u64;
        Some(hi << 32 | lo)
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.p.checked_add(n)?;
        let s = self.d.get(self.p..end)?;
        self.p = end;
        Some(s)
    }
    /// Header of the next box: its four-character type and its payload.
    /// `size == 1` means the real size follows as 64 bits; `size == 0` means
    /// "to the end of the enclosing box", which is legal for the last one.
    fn next_box(&mut self) -> Option<([u8; 4], &'a [u8])> {
        if self.left() < 8 { return None; }
        let size = self.u32()? as u64;
        let ty = *<&[u8; 4]>::try_from(self.take(4)?).ok()?;
        let (hdr, size) = if size == 1 {
            (16u64, self.u64()?)
        } else if size == 0 {
            (8u64, self.left() as u64 + 8)
        } else {
            (8u64, size)
        };
        let body = size.checked_sub(hdr)? as usize;
        Some((ty, self.take(body)?))
    }
    /// Version + flags of a full box, as one word with the version on top.
    fn full(&mut self) -> Option<(u8, u32)> {
        let v = self.u32()?;
        Some(((v >> 24) as u8, v & 0x00ff_ffff))
    }
}

fn children(d: &[u8]) -> Cur<'_> { Cur::new(d) }

/// One decodable unit, already located in the file.
#[derive(Clone, Copy)]
pub struct Sample {
    pub offset: usize,
    pub size: usize,
    /// Decode and presentation time, in the track's timescale.
    pub dts: u64,
    pub pts: u64,
    /// A sample the decoder may start at. Seeking lands on one of these.
    pub sync: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Video, Audio }

#[derive(Clone)]
pub enum Codec {
    /// H.264. `sps`/`pps` come from `avcC` and have to be prepended to the
    /// stream, because in MP4 they live in the header and not in the data.
    /// `nal_len` is how many bytes each NAL unit's length prefix uses.
    Avc { sps: Vec<Vec<u8>>, pps: Vec<Vec<u8>>, nal_len: usize },
    /// AAC, with its AudioSpecificConfig from `esds`.
    Aac { config: Vec<u8> },
    /// Present, understood well enough to skip. The four-character code is
    /// kept so a log line can name what we declined.
    Other([u8; 4]),
}

pub struct Track {
    pub kind: Kind,
    pub codec: Codec,
    /// Ticks per second for this track's `dts`/`pts`.
    pub timescale: u32,
    pub duration: u64,
    pub width: u32,
    pub height: u32,
    /// Clockwise display rotation in degrees (0, 90, 180, 270) from the
    /// `tkhd` matrix. A phone films landscape and writes the matrix; the
    /// coded picture is NOT the picture the viewer expects. Dropping this
    /// is how a player shows every holiday video on its side.
    pub rotation: u16,
    pub rate: u32,
    pub channels: u8,
    /// First media time the edit list asks for, in this track's timescale.
    ///
    /// Not cosmetic: the two tracks of one file carry DIFFERENT values. A
    /// phone recording here has 0 on the video and 2112 on the audio — the
    /// AAC encoder's priming samples, 48 ms. Playing the media timeline
    /// straight puts sound and picture that far apart, and it is the single
    /// easiest way to ship a player that is subtly out of sync.
    pub edit_start: u64,
    pub samples: Vec<Sample>,
}

impl Track {
    pub fn duration_ms(&self) -> u64 {
        if self.timescale == 0 { return 0; }
        self.duration * 1000 / self.timescale as u64
    }
    /// Presentation time of a sample on the shared clock, in ms.
    ///
    /// Signed on purpose: samples before `edit_start` are decoded but not
    /// shown, and a player has to be able to tell that from "show now".
    /// Every consumer asks through here so the edit list is applied once.
    pub fn pts_ms(&self, s: &Sample) -> i64 {
        if self.timescale == 0 { return 0; }
        (s.pts as i64 - self.edit_start as i64) * 1000 / self.timescale as i64
    }

    pub fn dts_ms(&self, s: &Sample) -> i64 {
        if self.timescale == 0 { return 0; }
        (s.dts as i64 - self.edit_start as i64) * 1000 / self.timescale as i64
    }

    /// Index of the last sync sample at or before `ms`. A decoder started
    /// anywhere else produces garbage until the next one.
    pub fn sync_at_ms(&self, ms: u64) -> usize {
        let t = ms as i64 * self.timescale as i64 / 1000 + self.edit_start as i64;
        let mut best = 0usize;
        for (i, s) in self.samples.iter().enumerate() {
            if s.dts as i64 > t { break; }
            if s.sync { best = i; }
        }
        best
    }
}

pub struct Mp4 {
    pub video: Option<Track>,
    pub audio: Option<Track>,
    /// The file said it is fragmented. The sample tables in `moov` are then
    /// empty by design and the real ones live in each `moof`, which we do
    /// not read — so this is reported rather than played as an empty file.
    pub fragmented: bool,
}

pub fn looks_like(d: &[u8]) -> bool {
    // `ftyp` at offset 4 is the normal case. Some muxers put `styp`, `moov`
    // or `free` first, so accept any known top-level box rather than only
    // the tidy one.
    let mut c = Cur::new(d);
    match c.next_box() {
        Some((t, _)) => matches!(&t, b"ftyp" | b"styp" | b"moov" | b"free" | b"skip" | b"mdat" | b"wide"),
        None => false,
    }
}

pub fn parse(d: &[u8]) -> Option<Mp4> {
    let mut top = Cur::new(d);
    let mut moov: Option<&[u8]> = None;
    let mut fragmented = false;
    while let Some((ty, body)) = top.next_box() {
        match &ty {
            b"moov" => moov = Some(body),
            b"moof" => fragmented = true,
            _ => {}
        }
    }
    let moov = moov?;

    let mut out = Mp4 { video: None, audio: None, fragmented };
    let mut c = children(moov);
    while let Some((ty, body)) = c.next_box() {
        if &ty == b"mvex" {
            // Movie-extends: the header promising fragments. Present even
            // when the first `moof` sits past whatever we have read.
            out.fragmented = true;
        }
        if &ty != b"trak" { continue; }
        if let Some(t) = parse_trak(body, d) {
            match t.kind {
                Kind::Video if out.video.is_none() => out.video = Some(t),
                Kind::Audio if out.audio.is_none() => out.audio = Some(t),
                _ => {}
            }
        }
    }
    Some(out)
}

fn parse_trak(trak: &[u8], file: &[u8]) -> Option<Track> {
    let mut width = 0u32;
    let mut height = 0u32;
    let mut rotation = 0u16;
    let mut mdia: Option<&[u8]> = None;
    let mut edit_start = 0u64;
    let mut c = children(trak);
    while let Some((ty, body)) = c.next_box() {
        match &ty {
            b"tkhd" => {
                let mut t = Cur::new(body);
                let (ver, _) = t.full()?;
                // creation, modification, track_id, reserved, duration
                t.skip(if ver == 1 { 32 } else { 20 })?;
                // reserved(8) layer(2) altgroup(2) volume(2) reserved(2)
                // matrix(36), then width/height as 16.16 fixed point.
                // reserved(8) layer(2) altgroup(2) volume(2) reserved(2),
                // then the 3x3 display matrix, then width/height as 16.16.
                t.skip(16)?;
                rotation = matrix_rotation(&mut t)?;
                width = t.u32()? >> 16;
                height = t.u32()? >> 16;
            }
            b"edts" => edit_start = parse_elst(body).unwrap_or(0),
            b"mdia" => mdia = Some(body),
            _ => {}
        }
    }
    let mdia = mdia?;

    let mut timescale = 0u32;
    let mut duration = 0u64;
    let mut kind: Option<Kind> = None;
    let mut minf: Option<&[u8]> = None;
    let mut c = children(mdia);
    while let Some((ty, body)) = c.next_box() {
        match &ty {
            b"mdhd" => {
                let mut t = Cur::new(body);
                let (ver, _) = t.full()?;
                if ver == 1 {
                    t.skip(16)?;
                    timescale = t.u32()?;
                    duration = t.u64()?;
                } else {
                    t.skip(8)?;
                    timescale = t.u32()?;
                    duration = t.u32()? as u64;
                }
            }
            b"hdlr" => {
                let mut t = Cur::new(body);
                t.full()?;
                t.skip(4)?; // pre_defined
                kind = match t.take(4)? {
                    b"vide" => Some(Kind::Video),
                    b"soun" => Some(Kind::Audio),
                    _ => None,
                };
            }
            b"minf" => minf = Some(body),
            _ => {}
        }
    }
    let kind = kind?;
    let stbl = find_box(minf?, b"stbl")?;
    let tables = parse_stbl(stbl)?;

    // `stsd` carries the real picture size for video; `tkhd`'s is the
    // DISPLAY size and may differ (anamorphic, or a track matrix). The
    // decoder outputs coded pixels, so the coded size wins where we have it.
    let (w, h) = match tables.visual {
        Some((vw, vh)) if vw > 0 && vh > 0 => (vw as u32, vh as u32),
        _ => (width, height),
    };

    let samples = build_samples(&tables, file);
    Some(Track {
        kind,
        codec: tables.codec,
        timescale,
        duration,
        width: w,
        height: h,
        rotation,
        rate: tables.rate,
        channels: tables.channels,
        edit_start,
        samples,
    })
}

/// Read the 3x3 display matrix and answer the rotation it expresses.
///
/// Only the four right-angle cases are recognised, because they are the only
/// ones a camera writes and the only ones a nearest-neighbour blit can carry
/// out exactly. A matrix that means anything else (a shear, a flip, a free
/// angle) answers 0 rather than a wrong quarter turn.
fn matrix_rotation(c: &mut Cur) -> Option<u16> {
    let a = c.u32()? as i32;
    let b = c.u32()? as i32;
    c.skip(4)?; // u
    let cc = c.u32()? as i32;
    let d = c.u32()? as i32;
    c.skip(4 + 4 + 4 + 4)?; // v, x, y, w
    const ONE: i32 = 0x0001_0000;
    Some(match (a, b, cc, d) {
        (0, ONE, m, 0) if m == -ONE => 90,
        (m, 0, 0, n) if m == -ONE && n == -ONE => 180,
        (0, m, ONE, 0) if m == -ONE => 270,
        _ => 0,
    })
}

/// The media time an `edts`/`elst` asks playback to start at.
///
/// Only the shape that every muxer writes is honoured: ONE entry, rate 1.0,
/// a non-negative media time. Multiple segments, an empty edit (`-1`) or a
/// changed rate describe an edit we do not perform, and shifting the clock
/// by a number taken out of such a list would be worse than leaving it —
/// so those answer 0 and the media timeline stands.
fn parse_elst(edts: &[u8]) -> Option<u64> {
    let mut c = Cur::new(find_box(edts, b"elst")?);
    let (ver, _) = c.full()?;
    if c.u32()? != 1 { return None; }
    let media_time = if ver == 1 {
        c.skip(8)?;
        c.u64()? as i64
    } else {
        c.skip(4)?;
        c.u32()? as i32 as i64
    };
    let rate = c.u32()?;
    if rate != 0x0001_0000 || media_time < 0 { return None; }
    Some(media_time as u64)
}

fn find_box<'a>(d: &'a [u8], want: &[u8; 4]) -> Option<&'a [u8]> {
    let mut c = children(d);
    while let Some((ty, body)) = c.next_box() {
        if &ty == want { return Some(body); }
    }
    None
}

struct Tables {
    codec: Codec,
    visual: Option<(u16, u16)>,
    rate: u32,
    channels: u8,
    /// (count, delta) pairs from `stts`.
    stts: Vec<(u32, u32)>,
    /// (count, offset) pairs from `ctts`; offsets are signed in version 1.
    ctts: Vec<(u32, i32)>,
    /// 1-based sample numbers that are sync points. Empty = every sample is.
    stss: Vec<u32>,
    /// (first_chunk, samples_per_chunk) from `stsc`.
    stsc: Vec<(u32, u32)>,
    sizes: Vec<u32>,
    /// A single size for all samples, when `stsz` says so.
    uniform_size: u32,
    sample_count: u32,
    chunks: Vec<u64>,
}

fn parse_stbl(stbl: &[u8]) -> Option<Tables> {
    let mut t = Tables {
        codec: Codec::Other(*b"none"),
        visual: None,
        rate: 0,
        channels: 0,
        stts: Vec::new(),
        ctts: Vec::new(),
        stss: Vec::new(),
        stsc: Vec::new(),
        sizes: Vec::new(),
        uniform_size: 0,
        sample_count: 0,
        chunks: Vec::new(),
    };
    let mut c = children(stbl);
    while let Some((ty, body)) = c.next_box() {
        let mut b = Cur::new(body);
        match &ty {
            b"stsd" => {
                b.full()?;
                let n = b.u32()?;
                if n > 0 {
                    if let Some((fmt, entry)) = b.next_box() {
                        parse_sample_entry(&fmt, entry, &mut t);
                    }
                }
            }
            b"stts" => {
                b.full()?;
                let n = b.u32()?.min(b.left() as u32 / 8);
                for _ in 0..n { t.stts.push((b.u32()?, b.u32()?)); }
            }
            b"ctts" => {
                let (ver, _) = b.full()?;
                let n = b.u32()?.min(b.left() as u32 / 8);
                for _ in 0..n {
                    let cnt = b.u32()?;
                    let off = b.u32()?;
                    // Version 0 says unsigned, but files in the wild carry
                    // negative offsets there anyway; version 1 says signed.
                    t.ctts.push((cnt, if ver == 0 { off as i32 } else { off as i32 }));
                }
            }
            b"stss" => {
                b.full()?;
                let n = b.u32()?.min(b.left() as u32 / 4);
                for _ in 0..n { t.stss.push(b.u32()?); }
            }
            b"stsc" => {
                b.full()?;
                let n = b.u32()?.min(b.left() as u32 / 12);
                for _ in 0..n {
                    let first = b.u32()?;
                    let per = b.u32()?;
                    b.u32()?; // sample_description_index
                    t.stsc.push((first, per));
                }
            }
            b"stsz" => {
                b.full()?;
                t.uniform_size = b.u32()?;
                t.sample_count = b.u32()?;
                if t.uniform_size == 0 {
                    let n = t.sample_count.min(b.left() as u32 / 4);
                    for _ in 0..n { t.sizes.push(b.u32()?); }
                }
            }
            b"stco" => {
                b.full()?;
                let n = b.u32()?.min(b.left() as u32 / 4);
                for _ in 0..n { t.chunks.push(b.u32()? as u64); }
            }
            b"co64" => {
                b.full()?;
                let n = b.u32()?.min(b.left() as u32 / 8);
                for _ in 0..n { t.chunks.push(b.u64()?); }
            }
            _ => {}
        }
    }
    Some(t)
}

fn parse_sample_entry(fmt: &[u8; 4], entry: &[u8], t: &mut Tables) {
    let mut e = Cur::new(entry);
    // SampleEntry: reserved(6) data_reference_index(2)
    if e.skip(8).is_none() { return; }
    match fmt {
        b"avc1" | b"avc3" => {
            // VisualSampleEntry up to the sub-boxes.
            if e.skip(16).is_none() { return; }
            let w = e.u16().unwrap_or(0);
            let h = e.u16().unwrap_or(0);
            t.visual = Some((w, h));
            if e.skip(4 + 4 + 4 + 2 + 32 + 2 + 2).is_none() { return; }
            while let Some((bt, body)) = e.next_box() {
                if &bt == b"avcC" {
                    if let Some(c) = parse_avcc(body) { t.codec = c; }
                    return;
                }
            }
            t.codec = Codec::Other(*fmt);
        }
        b"mp4a" => {
            if e.skip(8).is_none() { return; }
            t.channels = e.u16().unwrap_or(2) as u8;
            if e.skip(2 + 2 + 2).is_none() { return; }
            t.rate = (e.u32().unwrap_or(0) >> 16) as u32;
            while let Some((bt, body)) = e.next_box() {
                if &bt == b"esds" {
                    if let Some(cfg) = parse_esds(body) {
                        t.codec = Codec::Aac { config: cfg };
                        return;
                    }
                }
            }
            t.codec = Codec::Other(*fmt);
        }
        _ => t.codec = Codec::Other(*fmt),
    }
}

fn parse_avcc(d: &[u8]) -> Option<Codec> {
    let mut c = Cur::new(d);
    c.skip(4)?; // version, profile, compat, level
    let nal_len = (c.u8()? & 0x03) as usize + 1;
    let n_sps = (c.u8()? & 0x1f) as usize;
    let mut sps = Vec::new();
    for _ in 0..n_sps {
        let l = c.u16()? as usize;
        sps.push(c.take(l)?.to_vec());
    }
    let n_pps = c.u8()? as usize;
    let mut pps = Vec::new();
    for _ in 0..n_pps {
        let l = c.u16()? as usize;
        pps.push(c.take(l)?.to_vec());
    }
    Some(Codec::Avc { sps, pps, nal_len })
}

/// `esds` is an MPEG-4 descriptor blob; we want exactly one field out of it,
/// the DecoderSpecificInfo (tag 0x05) that AAC needs to configure itself.
fn parse_esds(d: &[u8]) -> Option<Vec<u8>> {
    let mut c = Cur::new(d);
    c.full()?;
    // Descriptors nest: ES(0x03) -> DecoderConfig(0x04) -> DecSpecific(0x05).
    // Each is tag, then a length in 1..4 bytes with a continuation bit.
    fn len_of(c: &mut Cur) -> Option<usize> {
        let mut n = 0usize;
        for _ in 0..4 {
            let b = c.u8()?;
            n = (n << 7) | (b & 0x7f) as usize;
            if b & 0x80 == 0 { break; }
        }
        Some(n)
    }
    loop {
        let tag = c.u8()?;
        let len = len_of(&mut c)?;
        match tag {
            0x03 => { c.skip(2)?; let flags = c.u8()?;
                      if flags & 0x80 != 0 { c.skip(2)?; }
                      if flags & 0x40 != 0 { let l = c.u8()? as usize; c.skip(l)?; }
                      if flags & 0x20 != 0 { c.skip(2)?; } }
            0x04 => { c.skip(13)?; }
            0x05 => return Some(c.take(len)?.to_vec()),
            _ => { c.skip(len)?; }
        }
    }
}

/// Turn the five tables into one flat list. This is the whole point of the
/// container: `stsc` says how many samples sit in each chunk, `stco` says
/// where each chunk starts, `stsz` how long each sample is, and the offsets
/// simply accumulate inside a chunk.
fn build_samples(t: &Tables, file: &[u8]) -> Vec<Sample> {
    let count = if t.uniform_size == 0 { t.sizes.len() } else { t.sample_count as usize };
    let mut out: Vec<Sample> = Vec::with_capacity(count);
    if t.stsc.is_empty() || t.chunks.is_empty() { return out; }

    let size_of = |i: usize| -> usize {
        if t.uniform_size != 0 { t.uniform_size as usize }
        else { t.sizes.get(i).copied().unwrap_or(0) as usize }
    };

    let mut idx = 0usize;
    for (run, &(first, per)) in t.stsc.iter().enumerate() {
        // A run of chunks holding `per` samples each, until the next entry's
        // `first_chunk` (or the last chunk).
        let last = t.stsc.get(run + 1).map(|&(f, _)| f).unwrap_or(t.chunks.len() as u32 + 1);
        if first == 0 { continue; }
        for ch in first..last {
            let Some(&base) = t.chunks.get(ch as usize - 1) else { break };
            let mut off = base as usize;
            for _ in 0..per {
                if idx >= count { break; }
                let sz = size_of(idx);
                // A sample that does not lie inside the file is where a
                // damaged or hostile table shows itself. Stop; do not clamp.
                if off.checked_add(sz).map_or(true, |e| e > file.len()) {
                    return out;
                }
                out.push(Sample { offset: off, size: sz, dts: 0, pts: 0, sync: true });
                off += sz;
                idx += 1;
            }
        }
    }

    // Times: `stts` is a run-length list of deltas, `ctts` shifts
    // presentation against decode (that is what B-frames need).
    let mut dts = 0u64;
    let mut i = 0usize;
    for &(n, delta) in &t.stts {
        for _ in 0..n {
            if i >= out.len() { break; }
            out[i].dts = dts;
            out[i].pts = dts;
            dts += delta as u64;
            i += 1;
        }
    }
    let mut i = 0usize;
    for &(n, off) in &t.ctts {
        for _ in 0..n {
            if i >= out.len() { break; }
            out[i].pts = (out[i].dts as i64 + off as i64).max(0) as u64;
            i += 1;
        }
    }

    // `stss` present means only those samples are sync points. Absent means
    // every one is — which is true for audio and for all-intra video.
    if !t.stss.is_empty() {
        for s in out.iter_mut() { s.sync = false; }
        for &n in &t.stss {
            if n >= 1 {
                if let Some(s) = out.get_mut(n as usize - 1) { s.sync = true; }
            }
        }
    }
    out
}

/// Rewrite one MP4 sample into Annex-B, which is what a decoder reads.
///
/// In MP4 each NAL unit carries a length prefix; Annex-B separates them with
/// a start code instead. Same payload, different framing — so this is a
/// reframing, not a conversion.
pub fn to_annex_b(sample: &[u8], nal_len: usize, out: &mut Vec<u8>) {
    let mut p = 0usize;
    while p + nal_len <= sample.len() {
        let mut n = 0usize;
        for i in 0..nal_len { n = (n << 8) | sample[p + i] as usize; }
        p += nal_len;
        let end = match p.checked_add(n) {
            Some(e) if e <= sample.len() => e,
            // A length that runs past the sample is corruption; take what is
            // there and stop rather than reading a neighbour's bytes.
            _ => sample.len(),
        };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&sample[p..end]);
        p = end;
    }
}

/// SPS and PPS as an Annex-B preamble. In MP4 they sit in `avcC`, so a
/// decoder fed only the samples never sees them and cannot start.
pub fn parameter_sets(codec: &Codec, out: &mut Vec<u8>) {
    if let Codec::Avc { sps, pps, .. } = codec {
        for s in sps.iter().chain(pps.iter()) {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(s);
        }
    }
}
