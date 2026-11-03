//! Audio capture: the microphone side of the audio mailbox.
//!
//! The driver of the active audio input (`devices`, kind audio in) pushes
//! what its hardware recorded, in the format it set; every open capture
//! stream gets its own copy, converted to the stream's format (encoding,
//! channels, rate by linear interpolation as `audio` does for playback).
//! A driver runs its hardware only while `wanted` says someone listens, so
//! the microphone is off unless a stream is open, and the kernel's `mic`
//! status item shows every open stream, whoever opened it.

use crate::audio::{Encoding, Format, DEFAULT_FORMAT};
use alloc::vec::Vec;
use spin::Mutex;

/// Streams one module may hold at once, against one that opens without
/// closing.
pub const STREAMS_PER_OWNER: usize = 4;
/// About 1.4 s of the stream's format: room for a reader that polls in
/// bursts. A full ring drops its oldest bytes.
const RING_MS: usize = 1365;

/// One unit of the converter's position: a whole device frame.
const ONE: u64 = 1 << 32;

struct Stream {
    owner: Option<u32>,
    fmt: Format,
    buf: Vec<u8>,
    head: usize,
    len: usize,
    /// Frames lost to a full ring since the last read.
    dropped: u64,
    /// Linear converter state: position between device frames `prev`
    /// and `cur`, in units of `ONE`.
    pos: u64,
    prev: [i32; 2],
    cur: [i32; 2],
}

impl Stream {
    fn new(owner: Option<u32>, fmt: Format) -> Self {
        let mut s = Self { owner, fmt, buf: Vec::new(), head: 0, len: 0, dropped: 0, pos: 0,
                           prev: [0; 2], cur: [0; 2] };
        s.start(fmt);
        s
    }

    fn start(&mut self, fmt: Format) {
        let n = fmt.rate as usize * fmt.frame_bytes() * RING_MS / 1000;
        self.buf = alloc::vec![0u8; (n - n % fmt.frame_bytes()).max(fmt.frame_bytes())];
        self.fmt = fmt;
        self.head = 0;
        self.len = 0;
        self.pos = 0;
        self.prev = [0; 2];
        self.cur = [0; 2];
    }

    /// One frame in the stream's format; the oldest goes when full.
    fn put(&mut self, f: [i32; 2]) {
        let fb = self.fmt.frame_bytes();
        let cap = self.buf.len();
        if self.len + fb > cap {
            self.head = (self.head + fb) % cap;
            self.len -= fb;
            self.dropped += 1;
        }
        let w = self.fmt.encoding_bytes();
        let mut raw = [0u8; 8];
        if self.fmt.channels == 1 {
            encode(self.fmt.encoding, ((f[0] as i64 + f[1] as i64) / 2) as i32, &mut raw[..w]);
        } else {
            encode(self.fmt.encoding, f[0], &mut raw[..w]);
            encode(self.fmt.encoding, f[1], &mut raw[w..2 * w]);
        }
        let at = (self.head + self.len) % cap;
        for (i, b) in raw[..fb].iter().enumerate() {
            self.buf[(at + i) % cap] = *b;
        }
        self.len += fb;
    }

    /// Device frame `f` arrives at `dev_rate`: the stream frames that fall
    /// between the previous one and this one.
    fn feed(&mut self, f: [i32; 2], dev_rate: u32) {
        self.prev = self.cur;
        self.cur = f;
        let step = ((dev_rate as u64) << 32) / self.fmt.rate as u64;
        while self.pos < ONE {
            let frac = self.pos as i64;
            let mut out = [0i32; 2];
            for c in 0..2 {
                let (a, b) = (self.prev[c] as i64, self.cur[c] as i64);
                out[c] = (a + (((b - a) * frac) >> 32)) as i32;
            }
            self.put(out);
            self.pos += step;
        }
        self.pos -= ONE;
    }

    fn read(&mut self, out: &mut [u8]) -> usize {
        let fb = self.fmt.frame_bytes();
        let n = self.len.min(out.len() - out.len() % fb);
        let cap = self.buf.len();
        for (i, b) in out[..n].iter_mut().enumerate() {
            *b = self.buf[(self.head + i) % cap];
        }
        self.head = (self.head + n) % cap;
        self.len -= n;
        n
    }
}

trait EncodingBytes {
    fn encoding_bytes(&self) -> usize;
}

impl EncodingBytes for Format {
    fn encoding_bytes(&self) -> usize {
        self.frame_bytes() / self.channels as usize
    }
}

fn decode(enc: Encoding, b: &[u8]) -> i32 {
    match enc {
        Encoding::S16 => (i16::from_le_bytes([b[0], b[1]]) as i32) << 16,
        Encoding::S32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        Encoding::F32 => {
            let f = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            (f.clamp(-1.0, 1.0) as f64 * i32::MAX as f64) as i32
        }
    }
}

fn encode(enc: Encoding, v: i32, out: &mut [u8]) {
    match enc {
        Encoding::S16 => out[..2].copy_from_slice(&((v >> 16) as i16).to_le_bytes()),
        Encoding::S32 => out[..4].copy_from_slice(&v.to_le_bytes()),
        Encoding::F32 => out[..4].copy_from_slice(&((v as f64 / i32::MAX as f64) as f32).to_le_bytes()),
    }
}

struct Capture {
    /// Open streams by handle; a closed one is `None`, so handles stay.
    streams: Vec<Option<Stream>>,
    /// What each driver pushes.
    devices: Vec<(u32, Format)>,
}

static CAPTURE: Mutex<Capture> = Mutex::new(Capture { streams: Vec::new(), devices: Vec::new() });

fn changed() {
    crate::notify::notify(crate::notify::TOPIC_STATUS);
}

/// Open streams, for the kernel's `mic` status item.
pub fn open_count() -> usize {
    CAPTURE.lock().streams.iter().flatten().count()
}

/// Open a stream for `owner` (None = the kernel) in [`DEFAULT_FORMAT`].
pub fn open(owner: Option<u32>) -> Option<usize> {
    let i = {
        let mut c = CAPTURE.lock();
        if owner.is_some() && c.streams.iter().flatten().filter(|s| s.owner == owner).count() >= STREAMS_PER_OWNER {
            return None;
        }
        let s = Stream::new(owner, DEFAULT_FORMAT);
        match c.streams.iter().position(|s| s.is_none()) {
            Some(i) => { c.streams[i] = Some(s); i }
            None => { c.streams.push(Some(s)); c.streams.len() - 1 }
        }
    };
    changed();
    Some(i)
}

fn with_stream<R>(i: usize, owner: Option<u32>, f: impl FnOnce(&mut Stream) -> R) -> Option<R> {
    let mut c = CAPTURE.lock();
    let s = c.streams.get_mut(i)?.as_mut().filter(|s| s.owner == owner)?;
    Some(f(s))
}

/// The stream's format: encoding, mono or stereo, 8..384 kHz. Empties it.
pub fn set_format(i: usize, owner: Option<u32>, fmt: Format) -> bool {
    if !(8_000..=384_000).contains(&fmt.rate) || !(1..=2).contains(&fmt.channels) {
        return false;
    }
    with_stream(i, owner, |s| s.start(fmt)).is_some()
}

/// What the stream has, whole frames, into `out`.
pub fn read(i: usize, owner: Option<u32>, out: &mut [u8]) -> Option<usize> {
    with_stream(i, owner, |s| s.read(out))
}

/// Frames the stream lost to a full ring since the last call.
pub fn take_dropped(i: usize, owner: Option<u32>) -> Option<u64> {
    with_stream(i, owner, |s| core::mem::take(&mut s.dropped))
}

pub fn close(i: usize, owner: Option<u32>) -> bool {
    let closed = {
        let mut c = CAPTURE.lock();
        match c.streams.get_mut(i) {
            Some(slot) if slot.as_ref().is_some_and(|s| s.owner == owner) => { *slot = None; true }
            _ => false,
        }
    };
    if closed {
        let mut c = CAPTURE.lock();
        while c.streams.last().is_some_and(|s| s.is_none()) {
            c.streams.pop();
        }
        drop(c);
        changed();
    }
    closed
}

/// Module `pid` ended: its streams close and its device format goes.
pub fn release_owner(pid: u32) {
    let n = {
        let mut c = CAPTURE.lock();
        c.devices.retain(|d| d.0 != pid);
        let mut n = 0;
        for slot in c.streams.iter_mut() {
            if slot.as_ref().is_some_and(|s| s.owner == Some(pid)) {
                *slot = None;
                n += 1;
            }
        }
        n
    };
    if n > 0 {
        changed();
    }
}

/// Whether driver `pid`'s input is the active one.
fn serves(pid: u32) -> bool {
    use crate::devices::{self, Kind, Owner};
    let (ins, active) = devices::list(Kind::AudioIn);
    active.is_some_and(|a| ins.iter().any(|e| e.handle == a && e.owner == Owner::Module(pid)))
}

/// Driver side: what driver `pid` records, stereo or mono.
pub fn set_device_format(pid: u32, fmt: Format) -> bool {
    if !(8_000..=384_000).contains(&fmt.rate) || !(1..=2).contains(&fmt.channels) {
        return false;
    }
    let mut c = CAPTURE.lock();
    match c.devices.iter_mut().find(|d| d.0 == pid) {
        Some(d) => d.1 = fmt,
        None => c.devices.push((pid, fmt)),
    }
    true
}

/// Driver side: how many streams want what driver `pid` records; 0 when
/// none or its input is not the active one (the hardware may sleep).
pub fn wanted(pid: u32) -> usize {
    if !serves(pid) {
        return 0;
    }
    open_count()
}

/// Driver side: recorded bytes in the driver's format, whole frames, to
/// every open stream. The bytes taken; 0 while its input is not active.
pub fn push(pid: u32, data: &[u8]) -> usize {
    if !serves(pid) {
        return 0;
    }
    let mut c = CAPTURE.lock();
    let Some(dev) = c.devices.iter().find(|d| d.0 == pid).map(|d| d.1) else { return 0 };
    let fb = dev.frame_bytes();
    let w = dev.encoding_bytes();
    let n = data.len() - data.len() % fb;
    for frame in data[..n].chunks_exact(fb) {
        let l = decode(dev.encoding, &frame[..w]);
        let r = if dev.channels == 2 { decode(dev.encoding, &frame[w..2 * w]) } else { l };
        for s in c.streams.iter_mut().flatten() {
            s.feed([l, r], dev.rate);
        }
    }
    n
}
