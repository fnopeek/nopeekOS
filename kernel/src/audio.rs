//! Audio mailbox + software mixer — the kernel's generic PCM sink.
//!
//! Apps push S16LE / 48 kHz / stereo PCM into per-slot ring buffers; the HDA
//! driver (audio_hda.wasm) pulls a mixed, master-volume-scaled stream via
//! [`poll_mix`] and feeds it to the controller. The kernel holds NO hardware
//! knowledge — this is a dumb byte shuttle + sum-mix, equally usable by a
//! future virtio-snd or USB-audio driver. See `memory/project_audio_hda.md`.

use core::sync::atomic::{AtomicU8, Ordering};
use spin::Mutex;

/// Fixed mixing format every submitter must use.
pub const SAMPLE_RATE: usize = 48000;
pub const CHANNELS: usize = 2;
pub const BYTES_PER_FRAME: usize = CHANNELS * 2; // S16

const NUM_SLOTS: usize = 4;
// ~1.36 s ring per slot. The old 32 KiB (~170 ms) was too small to absorb the
// clock drift between virtio-snd's wall-clock-paced fill (100 Hz ticks) and
// audio_hda's HDA-crystal-paced drain, plus the resident-driver poll jitter
// under heavy browser load — `mbox-free` pegged at 0 the whole session, so
// `submit` dropped real PCM (the "choppy / cuts out" symptom). Zero-init →
// .bss, so 4 × 256 KiB = 1 MiB costs no binary size.
const SLOT_BYTES: usize = 262144;

struct Slot {
    active: bool,
    auto_close: bool, // free the slot once drained (one-shot sounds, e.g. `beep`)
    head: usize,      // read offset into `buf`
    len: usize,       // bytes currently buffered
    buf: [u8; SLOT_BYTES],
}

impl Slot {
    const fn new() -> Self {
        Self { active: false, auto_close: false, head: 0, len: 0, buf: [0u8; SLOT_BYTES] }
    }
}

// All-zero initial value -> lands in .bss (not the kernel binary). Volume is a
// separate atomic so its non-zero default doesn't drag the slots into .data.
static SLOTS: Mutex<[Slot; NUM_SLOTS]> =
    Mutex::new([Slot::new(), Slot::new(), Slot::new(), Slot::new()]);
static VOLUME: AtomicU8 = AtomicU8::new(80); // master, 0..=100 %

/// Allocate a streaming slot. Returns the slot index or -1 if all are in use.
pub fn open() -> i32 {
    let mut slots = SLOTS.lock();
    for (i, s) in slots.iter_mut().enumerate() {
        if !s.active {
            s.active = true;
            s.auto_close = false;
            s.head = 0;
            s.len = 0;
            return i as i32;
        }
    }
    -1
}

/// Release a streaming slot (discards anything still buffered).
pub fn close(slot: usize) {
    if slot >= NUM_SLOTS { return; }
    let mut slots = SLOTS.lock();
    slots[slot].active = false;
    slots[slot].len = 0;
}

/// Empty a slot's ring without releasing it. Used when a still-held stream is
/// re-prepared (e.g. cubeb re-initialising after an underrun without a clean
/// RELEASE): reuse the slot + drop stale PCM instead of leaking a fresh one.
pub fn reset(slot: usize) {
    if slot >= NUM_SLOTS { return; }
    let mut slots = SLOTS.lock();
    slots[slot].head = 0;
    slots[slot].len = 0;
}

/// Append PCM to a slot's ring. Returns the number of bytes accepted (fewer
/// than `data.len()` when the ring is full — the submitter must retry/pace).
pub fn submit(slot: usize, data: &[u8]) -> usize {
    if slot >= NUM_SLOTS { return 0; }
    let mut slots = SLOTS.lock();
    let s = &mut slots[slot];
    if !s.active { return 0; }
    let free = SLOT_BYTES - s.len;
    let n = data.len().min(free);
    let mut w = (s.head + s.len) % SLOT_BYTES;
    for &b in &data[..n] {
        s.buf[w] = b;
        w = (w + 1) % SLOT_BYTES;
    }
    s.len += n;
    n
}

/// Play a self-contained sound: grab a free slot, fill it, and auto-free it
/// once the driver has drained it. For short one-shots (must fit in a slot).
pub fn play_oneshot(data: &[u8]) -> bool {
    let mut slots = SLOTS.lock();
    for s in slots.iter_mut() {
        if !s.active {
            let n = data.len().min(SLOT_BYTES);
            s.active = true;
            s.auto_close = true;
            s.head = 0;
            s.len = n;
            s.buf[..n].copy_from_slice(&data[..n]);
            return true;
        }
    }
    false
}

/// Driver side: mix every active slot into `out` (S16 add + clamp), scale by
/// master volume, and advance each slot. Always fills `out` fully (silence
/// where slots are empty), so the driver always has a complete buffer.
/// Returns the number of bytes written (a whole number of frames).
pub fn poll_mix(out: &mut [u8]) -> usize {
    let vol = VOLUME.load(Ordering::Relaxed) as i32;
    let frames = out.len() / BYTES_PER_FRAME;
    let mut slots = SLOTS.lock();
    for f in 0..frames {
        let mut l: i32 = 0;
        let mut r: i32 = 0;
        for s in slots.iter_mut() {
            if !s.active || s.len < BYTES_PER_FRAME { continue; }
            let p = s.head;
            let lo = i16::from_le_bytes([s.buf[p], s.buf[(p + 1) % SLOT_BYTES]]);
            let ro = i16::from_le_bytes([s.buf[(p + 2) % SLOT_BYTES], s.buf[(p + 3) % SLOT_BYTES]]);
            s.head = (p + BYTES_PER_FRAME) % SLOT_BYTES;
            s.len -= BYTES_PER_FRAME;
            l += lo as i32;
            r += ro as i32;
            if s.len == 0 && s.auto_close { s.active = false; }
        }
        l = (l * vol / 100).clamp(-32768, 32767);
        r = (r * vol / 100).clamp(-32768, 32767);
        let lb = (l as i16).to_le_bytes();
        let rb = (r as i16).to_le_bytes();
        let o = f * BYTES_PER_FRAME;
        out[o] = lb[0];
        out[o + 1] = lb[1];
        out[o + 2] = rb[0];
        out[o + 3] = rb[1];
    }
    frames * BYTES_PER_FRAME
}

/// Free bytes in a slot's ring (0 if the slot is closed). The virtio-snd
/// bridge uses this to pace the guest: it only completes a PCM period once
/// the period fits, so the mailbox drain rate (= real playback at 48 kHz)
/// becomes the guest's audio clock — no separate timer needed.
pub fn free_space(slot: usize) -> usize {
    if slot >= NUM_SLOTS { return 0; }
    let slots = SLOTS.lock();
    if !slots[slot].active { return 0; }
    SLOT_BYTES - slots[slot].len
}

/// Set master volume (0..=100 %).
pub fn set_volume(pct: u8) {
    VOLUME.store(pct.min(100), Ordering::Relaxed);
}

/// Get master volume (0..=100 %).
pub fn get_volume() -> u8 {
    VOLUME.load(Ordering::Relaxed)
}
