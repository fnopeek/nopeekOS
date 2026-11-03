//! The camera feed: one producer, any number of readers, the newest frame.
//! Contract: `docs/spec/CAMERA.md`.
//!
//! The camera's driver module (`uvc`) owns the device and pushes each
//! frame it decodes from the stream as it came (MJPEG or YUYV). Readers
//! (the `cam` viewer, the microVM's camera) take the newest frame they
//! have not seen; nothing queues, so a slow reader skips frames instead of
//! lagging. The driver streams only while `wanted` says someone reads,
//! and at the largest size a reader asked for.

use alloc::vec::Vec;
use spin::Mutex;

/// `npk_canvas_frame`'s numbers: packed YUYV (BT.601 limited) and MJPEG.
pub const FORMAT_YUYV: u8 = 1;
pub const FORMAT_MJPEG: u8 = 3;

/// A frame larger than this is refused: 1080p YUYV with room.
pub const FRAME_MAX: usize = 8 << 20;
/// Readers one module may hold.
const READERS_PER_OWNER: usize = 4;

#[derive(Clone, Copy, Default)]
pub struct Info {
    pub width: u16,
    pub height: u16,
    pub format: u8,
    /// Counts the producer's frames; a reader sees each once.
    pub seq: u64,
}

struct Reader {
    owner: Option<u32>,
    want: (u16, u16),
    last: u64,
}

struct Feed {
    readers: Vec<Option<Reader>>,
    frame: Vec<u8>,
    info: Info,
}

static FEED: Mutex<Feed> = Mutex::new(Feed { readers: Vec::new(), frame: Vec::new(), info: Info {
    width: 0, height: 0, format: 0, seq: 0,
} });

/// Open a reader wanting `width` x `height` (the producer picks the largest
/// asked for). None when `owner` holds its share.
pub fn open(owner: Option<u32>, width: u16, height: u16) -> Option<usize> {
    let mut f = FEED.lock();
    if owner.is_some() && f.readers.iter().flatten().filter(|r| r.owner == owner).count() >= READERS_PER_OWNER {
        return None;
    }
    let last = f.info.seq;
    let r = Reader { owner, want: (width, height), last };
    let i = match f.readers.iter().position(|r| r.is_none()) {
        Some(i) => { f.readers[i] = Some(r); i }
        None => { f.readers.push(Some(r)); f.readers.len() - 1 }
    };
    Some(i)
}

/// The newest frame `id` has not seen, into `out`: its length and info, or
/// `Some((0, _))` when there is none. A frame longer than `out` is skipped
/// and its length returned with nothing copied.
pub fn read(id: usize, owner: Option<u32>, out: &mut [u8]) -> Option<(usize, Info)> {
    let mut f = FEED.lock();
    let info = f.info;
    let r = f.readers.get_mut(id)?.as_mut().filter(|r| r.owner == owner)?;
    if info.seq == r.last || info.seq == 0 {
        return Some((0, info));
    }
    r.last = info.seq;
    let n = f.frame.len();
    if n <= out.len() {
        out[..n].copy_from_slice(&f.frame);
    }
    Some((n, info))
}

/// The size of the newest frame `id` has not seen, 0 for none.
pub fn pending(id: usize, owner: Option<u32>) -> Option<usize> {
    let f = FEED.lock();
    let r = f.readers.get(id)?.as_ref().filter(|r| r.owner == owner)?;
    Some(if f.info.seq != r.last && f.info.seq != 0 { f.frame.len() } else { 0 })
}

pub fn close(id: usize, owner: Option<u32>) -> bool {
    let mut f = FEED.lock();
    match f.readers.get_mut(id) {
        Some(slot) if slot.as_ref().is_some_and(|r| r.owner == owner) => {
            *slot = None;
            while f.readers.last().is_some_and(|r| r.is_none()) {
                f.readers.pop();
            }
            true
        }
        _ => false,
    }
}

/// Module `pid` ended: its readers go.
pub fn release_owner(pid: u32) {
    let mut f = FEED.lock();
    for slot in f.readers.iter_mut() {
        if slot.as_ref().is_some_and(|r| r.owner == Some(pid)) {
            *slot = None;
        }
    }
    while f.readers.last().is_some_and(|r| r.is_none()) {
        f.readers.pop();
    }
}

/// Producer side: how many read, and the largest size one asked for.
pub fn wanted() -> (usize, u16, u16) {
    let f = FEED.lock();
    let mut n = 0;
    let (mut w, mut h) = (0u16, 0u16);
    for r in f.readers.iter().flatten() {
        n += 1;
        if r.want.0 as u32 * r.want.1 as u32 > w as u32 * h as u32 {
            (w, h) = r.want;
        }
    }
    (n, w, h)
}

/// Producer side: a frame as the camera sent it. False for a size or
/// format the feed does not take.
pub fn push(data: &[u8], width: u16, height: u16, format: u8) -> bool {
    if data.is_empty() || data.len() > FRAME_MAX || width == 0 || height == 0
        || !matches!(format, FORMAT_YUYV | FORMAT_MJPEG)
        || (format == FORMAT_YUYV && data.len() < width as usize * height as usize * 2)
    {
        return false;
    }
    let mut f = FEED.lock();
    f.frame.clear();
    f.frame.extend_from_slice(data);
    let seq = f.info.seq + 1;
    f.info = Info { width, height, format, seq };
    true
}

/// The producer stopped: no frame stays to be read as new.
pub fn clear() {
    let mut f = FEED.lock();
    f.frame = Vec::new();
    let seq = f.info.seq;
    for r in f.readers.iter_mut().flatten() {
        r.last = seq;
    }
}
