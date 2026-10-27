//! Audio mailbox + software mixer: the kernel's generic PCM sink.
//!
//! Apps push PCM into per-stream ring buffers, each in its own format
//! ([`Format`], S16/48 kHz/stereo until set otherwise); the audio driver
//! (audio_hda.wasm) pulls one mixed, master-volume-scaled stream in the
//! device format it set with [`set_device_format`] via [`poll_mix`]. Any
//! number of streams mix; a module may hold [`SLOTS_PER_OWNER`] at once.
//! The kernel holds no hardware knowledge: this is a byte shuttle, a format
//! and rate converter and a sum-mix, equally usable by virtio-snd or a
//! USB-audio driver.

use core::sync::atomic::{AtomicU8, Ordering};
use spin::Mutex;

/// Sample encodings, as numbered in the ABI (`npk_audio_format`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    S16 = 0,
    S32 = 1,
    F32 = 2,
}

impl Encoding {
    pub fn from_abi(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::S16),
            1 => Some(Self::S32),
            2 => Some(Self::F32),
            _ => None,
        }
    }
    const fn bytes(self) -> usize {
        match self {
            Self::S16 => 2,
            Self::S32 | Self::F32 => 4,
        }
    }
}

/// Interleaved little-endian PCM.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Format {
    pub rate: u32,
    pub encoding: Encoding,
    pub channels: u8,
}

impl Format {
    pub const fn frame_bytes(&self) -> usize {
        self.encoding.bytes() * self.channels as usize
    }
    fn valid(&self, max_channels: u8) -> bool {
        (RATE_MIN..=RATE_MAX).contains(&self.rate) && (1..=max_channels).contains(&self.channels)
    }
}

/// What a stream and the device are until told otherwise.
pub const DEFAULT_FORMAT: Format = Format { rate: 48_000, encoding: Encoding::S16, channels: 2 };
pub const BYTES_PER_FRAME: usize = DEFAULT_FORMAT.frame_bytes();

/// HDA's rate range (`rate_bits`, 8 kHz .. 192 kHz) with room for 384 kHz.
const RATE_MIN: u32 = 8_000;
const RATE_MAX: u32 = 384_000;
/// Source streams are mono or stereo; a device takes up to eight channels
/// (7.1), of which a stereo source fills the first two.
const SOURCE_CHANNELS_MAX: u8 = 2;
const DEVICE_CHANNELS_MAX: u8 = 8;

/// Streams one module may hold at once; the table itself grows. A safety
/// cap against a module that opens without closing, logged when it bites.
pub const SLOTS_PER_OWNER: usize = 8;
// ~1.36 s ring per slot: it must absorb the clock drift between virtio-snd's
// wall-clock-paced fill (100 Hz ticks) and audio_hda's HDA-crystal-paced
// drain, plus driver poll jitter under load; a full ring makes `submit` take
// fewer bytes, which the submitter sees.
const SLOT_BYTES: usize = 262144;

/// The ring size for a stream of `fmt`: the same play time as `SLOT_BYTES`
/// holds of the default format, whole frames.
fn ring_bytes(fmt: &Format) -> usize {
    let per_sec = fmt.rate as usize * fmt.frame_bytes();
    let default_per_sec = DEFAULT_FORMAT.rate as usize * BYTES_PER_FRAME;
    let n = SLOT_BYTES * per_sec / default_per_sec;
    (n - n % fmt.frame_bytes()).max(fmt.frame_bytes())
}

/// One unit of the resampler's position: a whole source frame.
const ONE: u64 = 1 << 32;

struct Slot {
    active: bool,
    auto_close: bool, // free the slot once drained (one-shot sounds, e.g. `beep`)
    head: usize,      // read offset into `buf`
    len: usize,       // bytes currently buffered
    owner: Option<u32>, // pid of the opening module; None = kernel
    fmt: Format,
    /// Linear rate converter state (ALSA `pcm_rate_linear`): the output
    /// position between frame `prev` and frame `cur`, in units of `ONE`.
    pos: u64,
    prev: [i32; SOURCE_CHANNELS_MAX as usize],
    cur: [i32; SOURCE_CHANNELS_MAX as usize],
    /// The ring; empty while the slot is closed, so a closed slot holds no
    /// memory.
    buf: alloc::vec::Vec<u8>,
}

impl Slot {
    const fn new() -> Self {
        Self { active: false, auto_close: false, head: 0, len: 0, owner: None, fmt: DEFAULT_FORMAT,
               pos: 2 * ONE, prev: [0; 2], cur: [0; 2], buf: alloc::vec::Vec::new() }
    }

    /// Start empty in `fmt` with a ring of at least `min_bytes`.
    fn start(&mut self, fmt: Format, min_bytes: usize) {
        let want = ring_bytes(&fmt).max(min_bytes);
        if self.buf.len() != want {
            self.buf = alloc::vec![0u8; want];
        }
        self.fmt = fmt;
        self.head = 0;
        self.len = 0;
        self.rewind();
    }

    /// The converter starts over: the next output frame is the next source
    /// frame.
    fn rewind(&mut self) {
        self.pos = 2 * ONE;
        self.prev = [0; 2];
        self.cur = [0; 2];
    }

    fn stop(&mut self) {
        self.active = false;
        self.auto_close = false;
        self.len = 0;
        self.buf = alloc::vec::Vec::new();
    }

    /// The next source frame as full-scale i32 samples, or `None` when the
    /// ring has no whole frame.
    fn read_frame(&mut self) -> Option<[i32; SOURCE_CHANNELS_MAX as usize]> {
        let fb = self.fmt.frame_bytes();
        if self.len < fb {
            return None;
        }
        let cap = self.buf.len();
        let mut raw = [0u8; 8];
        for (i, b) in raw[..fb].iter_mut().enumerate() {
            *b = self.buf[(self.head + i) % cap];
        }
        self.head = (self.head + fb) % cap;
        self.len -= fb;
        let w = self.fmt.encoding.bytes();
        let mut f = [0i32; SOURCE_CHANNELS_MAX as usize];
        for (c, s) in f.iter_mut().enumerate().take(self.fmt.channels as usize) {
            *s = decode(self.fmt.encoding, &raw[c * w..c * w + w]);
        }
        if self.fmt.channels == 1 {
            f[1] = f[0];
        }
        Some(f)
    }

    /// One output frame at the device rate, linear interpolation between
    /// the two source frames around the output position; `None` on
    /// underrun.
    fn next_frame(&mut self, step: u64) -> Option<[i32; SOURCE_CHANNELS_MAX as usize]> {
        while self.pos >= ONE {
            let f = self.read_frame()?;
            self.prev = self.cur;
            self.cur = f;
            self.pos -= ONE;
        }
        let frac = self.pos as i64;
        let mut out = [0i32; SOURCE_CHANNELS_MAX as usize];
        for c in 0..out.len() {
            let a = self.prev[c] as i64;
            let b = self.cur[c] as i64;
            out[c] = (a + (((b - a) * frac) >> 32)) as i32;
        }
        self.pos += step;
        Some(out)
    }
}

/// A sample as full-scale i32.
fn decode(enc: Encoding, b: &[u8]) -> i32 {
    match enc {
        Encoding::S16 => (i16::from_le_bytes([b[0], b[1]]) as i32) << 16,
        Encoding::S32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        Encoding::F32 => {
            let f = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            // NaN maps to 0 through `as`.
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

struct Mixer {
    /// Streams by index; a closed slot is reused, so indices stay put.
    slots: alloc::vec::Vec<Slot>,
    /// What `poll_mix` produces.
    device: Format,
}

static MIXER: Mutex<Mixer> =
    Mutex::new(Mixer { slots: alloc::vec::Vec::new(), device: DEFAULT_FORMAT });

/// A free slot, reused or new. `None` when `owner` already holds its share.
fn free_slot(slots: &mut alloc::vec::Vec<Slot>, owner: Option<u32>) -> Option<usize> {
    if owner.is_some() && slots.iter().filter(|s| s.active && s.owner == owner).count() >= SLOTS_PER_OWNER {
        return None;
    }
    match slots.iter().position(|s| !s.active) {
        Some(i) => Some(i),
        None => { slots.push(Slot::new()); Some(slots.len() - 1) }
    }
}

/// Drop closed slots at the end of the table; indices of open ones stay.
fn shrink(slots: &mut alloc::vec::Vec<Slot>) {
    while slots.last().is_some_and(|s| !s.active) {
        slots.pop();
    }
}

static VOLUME: AtomicU8 = AtomicU8::new(80); // master, 0..=100 %

/// Allocate a streaming slot for the kernel, in [`DEFAULT_FORMAT`].
pub fn open() -> i32 {
    open_owned(None)
}

/// Allocate a streaming slot owned by module `pid`, in [`DEFAULT_FORMAT`];
/// only that module may submit to, query or close it (`*_for`). -1 when the
/// module already holds [`SLOTS_PER_OWNER`] streams.
pub fn open_for(pid: u32) -> i32 {
    open_owned(Some(pid))
}

fn open_owned(owner: Option<u32>) -> i32 {
    let mut m = MIXER.lock();
    let Some(i) = free_slot(&mut m.slots, owner) else {
        drop(m);
        crate::kdebug!("audio: pid {} holds {} streams, open refused", owner.unwrap_or(0), SLOTS_PER_OWNER);
        return -1;
    };
    let s = &mut m.slots[i];
    s.active = true;
    s.auto_close = false;
    s.owner = owner;
    s.start(DEFAULT_FORMAT, 0);
    i as i32
}

fn owned(s: &Slot, pid: u32) -> bool {
    s.active && s.owner == Some(pid)
}

/// Set the format of module `pid`'s stream; empties its ring. False for a
/// foreign or closed slot or a format the mixer does not take.
pub fn set_format_for(slot: usize, pid: u32, fmt: Format) -> bool {
    if !fmt.valid(SOURCE_CHANNELS_MAX) {
        return false;
    }
    let mut m = MIXER.lock();
    match m.slots.get_mut(slot) {
        Some(s) if owned(s, pid) => {
            s.start(fmt, 0);
            true
        }
        _ => false,
    }
}

/// The audio driver says what it plays: `poll_mix` produces this from now
/// on. False for a format the mixer cannot produce.
pub fn set_device_format(fmt: Format) -> bool {
    if !fmt.valid(DEVICE_CHANNELS_MAX) {
        return false;
    }
    MIXER.lock().device = fmt;
    true
}

/// `close` for a module: only its own slot. Returns false otherwise.
pub fn close_for(slot: usize, pid: u32) -> bool {
    let mut m = MIXER.lock();
    if !m.slots.get(slot).is_some_and(|s| owned(s, pid)) { return false; }
    m.slots[slot].stop();
    shrink(&mut m.slots);
    true
}

/// `submit` for a module: None unless the slot belongs to `pid`.
pub fn submit_for(slot: usize, pid: u32, data: &[u8]) -> Option<usize> {
    let mut m = MIXER.lock();
    let s = m.slots.get_mut(slot).filter(|s| owned(s, pid))?;
    Some(push(s, data))
}

/// `buffered` for a module: None unless the slot belongs to `pid`.
pub fn buffered_for(slot: usize, pid: u32) -> Option<usize> {
    let m = MIXER.lock();
    m.slots.get(slot).filter(|s| owned(s, pid)).map(|s| s.len)
}

/// Close every slot module `pid` still holds (it has ended).
pub fn release_owner(pid: u32) {
    let mut m = MIXER.lock();
    for s in m.slots.iter_mut().filter(|s| s.active && s.owner == Some(pid)) {
        s.stop();
    }
    shrink(&mut m.slots);
}

/// Release a streaming slot (discards anything still buffered).
pub fn close(slot: usize) {
    let mut m = MIXER.lock();
    let Some(s) = m.slots.get_mut(slot) else { return };
    s.stop();
    shrink(&mut m.slots);
}

/// Empty a slot's ring without releasing it. Used when a still-held stream is
/// re-prepared (e.g. cubeb re-initialising after an underrun without a clean
/// RELEASE): reuse the slot + drop stale PCM instead of leaking a fresh one.
pub fn reset(slot: usize) {
    let mut m = MIXER.lock();
    let Some(s) = m.slots.get_mut(slot) else { return };
    s.head = 0;
    s.len = 0;
    s.rewind();
}

/// Append PCM to a slot's ring. Returns the number of bytes accepted (fewer
/// than `data.len()` when the ring is full; the submitter must retry/pace).
pub fn submit(slot: usize, data: &[u8]) -> usize {
    let mut m = MIXER.lock();
    match m.slots.get_mut(slot) {
        Some(s) if s.active => push(s, data),
        _ => 0,
    }
}

fn push(s: &mut Slot, data: &[u8]) -> usize {
    let cap = s.buf.len();
    let n = data.len().min(cap - s.len);
    let w = (s.head + s.len) % cap;
    let first = n.min(cap - w);
    s.buf[w..w + first].copy_from_slice(&data[..first]);
    s.buf[..n - first].copy_from_slice(&data[first..n]);
    s.len += n;
    n
}

/// Play a self-contained sound in [`DEFAULT_FORMAT`]: grab a free slot, fill
/// it, and free it once the driver has drained it. The ring grows to hold
/// the whole sound.
pub fn play_oneshot(data: &[u8]) -> bool {
    let mut m = MIXER.lock();
    let Some(i) = free_slot(&mut m.slots, None) else { return false };
    let s = &mut m.slots[i];
    s.active = true;
    s.auto_close = true;
    s.owner = None;
    s.start(DEFAULT_FORMAT, data.len());
    push(s, data);
    true
}

/// Driver side: mix every active slot into `out` in the device format,
/// converting each stream's encoding and rate (linear interpolation) and
/// mapping mono to both front channels, then scale by master volume and
/// clamp. Always fills `out` fully (silence where slots are empty), so the
/// driver always has a complete buffer. Returns the number of bytes written
/// (a whole number of frames).
pub fn poll_mix(out: &mut [u8]) -> usize {
    let vol = VOLUME.load(Ordering::Relaxed) as i64;
    let mut m = MIXER.lock();
    let dev = m.device;
    let fb = dev.frame_bytes();
    let sb = dev.encoding.bytes();
    let chans = dev.channels as usize;
    let frames = out.len() / fb;
    for f in 0..frames {
        let mut acc = [0i64; DEVICE_CHANNELS_MAX as usize];
        for s in m.slots.iter_mut() {
            if !s.active { continue; }
            let step = ((s.fmt.rate as u64) << 32) / dev.rate as u64;
            let Some(x) = s.next_frame(step) else {
                if s.auto_close { s.stop(); }
                continue;
            };
            if chans == 1 {
                acc[0] += (x[0] as i64 + x[1] as i64) / 2;
            } else {
                acc[0] += x[0] as i64;
                acc[1] += x[1] as i64;
            }
        }
        let o = f * fb;
        for (c, a) in acc.iter().enumerate().take(chans) {
            let v = (a * vol / 100).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            encode(dev.encoding, v, &mut out[o + c * sb..o + c * sb + sb]);
        }
    }
    shrink(&mut m.slots);
    frames * fb
}

/// Bytes currently buffered in a slot's ring (0 if closed). The virtio-snd
/// bridge reports this (plus the HDA ring) as the PCM-received-but-not-yet-played
/// latency in each tx completion, so the guest's A/V sync accounts for our
/// hidden host buffering.
pub fn buffered(slot: usize) -> usize {
    let m = MIXER.lock();
    m.slots.get(slot).filter(|s| s.active).map_or(0, |s| s.len)
}

/// Set master volume (0..=100 %).
pub fn set_volume(pct: u8) {
    let v = pct.min(100);
    if VOLUME.swap(v, Ordering::Relaxed) != v {
        crate::notify::notify(crate::notify::TOPIC_VOLUME);
    }
}

/// Get master volume (0..=100 %).
pub fn get_volume() -> u8 {
    VOLUME.load(Ordering::Relaxed)
}

/// Display ports the display side can report audio for; i915's port index
/// (A = 0) is the index.
pub const DISPLAY_AUDIO_PORTS: usize = 9;
/// `MAX_ELD_BYTES`.
pub const ELD_MAX: usize = 128;

/// What Linux' audio component carries from i915 to snd-hda: the ELD each
/// port's sink has (`get_eld`), and a count that moves whenever one changes
/// (`pin_eld_notify`).
struct DisplayAudio {
    generation: u32,
    eld: [([u8; ELD_MAX], usize); DISPLAY_AUDIO_PORTS],
}

static DISPLAY_AUDIO: Mutex<DisplayAudio> =
    Mutex::new(DisplayAudio { generation: 0, eld: [([0; ELD_MAX], 0); DISPLAY_AUDIO_PORTS] });

/// The display driver says what sink `port` has: its ELD, empty for none.
pub fn display_audio_set(port: usize, eld: &[u8]) -> bool {
    if port >= DISPLAY_AUDIO_PORTS || eld.len() > ELD_MAX {
        return false;
    }
    let mut d = DISPLAY_AUDIO.lock();
    let slot = &mut d.eld[port];
    slot.0[..eld.len()].copy_from_slice(eld);
    slot.0[eld.len()..].fill(0);
    slot.1 = eld.len();
    d.generation = d.generation.wrapping_add(1).max(1);
    true
}

/// The ELD of `port` into `out`; its length, 0 when there is no sink.
pub fn display_audio_get(port: usize, out: &mut [u8]) -> Option<usize> {
    if port >= DISPLAY_AUDIO_PORTS {
        return None;
    }
    let d = DISPLAY_AUDIO.lock();
    let (eld, len) = &d.eld[port];
    let n = (*len).min(out.len());
    out[..n].copy_from_slice(&eld[..n]);
    Some(*len)
}

/// 0 until the display side reported once, then a count that moves with
/// every report.
pub fn display_audio_generation() -> u32 {
    DISPLAY_AUDIO.lock().generation
}

/// The display driver is gone: no port has a sink any more.
pub fn display_audio_clear() {
    let mut d = DISPLAY_AUDIO.lock();
    for slot in d.eld.iter_mut() {
        *slot = ([0; ELD_MAX], 0);
    }
    d.generation = d.generation.wrapping_add(1).max(1);
}
