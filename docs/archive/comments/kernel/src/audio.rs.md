# `kernel/src/audio.rs` @ 5e0102684

## L1-7 · `use core::sync::atomic::{AtomicU8, Ordering};`

```
//! Audio mailbox + software mixer — the kernel's generic PCM sink.
//!
//! Apps push S16LE / 48 kHz / stereo PCM into per-slot ring buffers; the HDA
//! driver (audio_hda.wasm) pulls a mixed, master-volume-scaled stream via
//! [`poll_mix`] and feeds it to the controller. The kernel holds NO hardware
//! knowledge — this is a dumb byte shuttle + sum-mix, equally usable by a
//! future virtio-snd or USB-audio driver. See `memory/project_audio_hda.md`.
```

## L13 · `pub const BYTES_PER_FRAME: usize = CHANNELS * 2; // S16`

```
// S16
```

## L16-21 · `const SLOT_BYTES: usize = 262144;`

```
// ~1.36 s ring per slot. The old 32 KiB (~170 ms) was too small to absorb the
// clock drift between virtio-snd's wall-clock-paced fill (100 Hz ticks) and
// audio_hda's HDA-crystal-paced drain, plus the resident-driver poll jitter
// under heavy browser load — `mbox-free` pegged at 0 the whole session, so
// `submit` dropped real PCM (the "choppy / cuts out" symptom). Zero-init →
// .bss, so 4 × 256 KiB = 1 MiB costs no binary size.
```

## L26 · `auto_close: bool, // free the slot once drained (one-shot sounds, e.g. beep)`

```
// free the slot once drained (one-shot sounds, e.g. `beep`)
```

## L27 · `head: usize,      // read offset into buf`

```
// read offset into `buf`
```

## L28 · `len: usize,       // bytes currently buffered`

```
// bytes currently buffered
```

## L29 · `drained: u64,     // total bytes pulled by poll_mix since open (real 48 kHz clock)`

```
// total bytes pulled by `poll_mix` since `open` (real 48 kHz clock)
```

## L39-40 · `static SLOTS: Mutex<[Slot; NUM_SLOTS]> =`

```
// All-zero initial value -> lands in .bss (not the kernel binary). Volume is a
// separate atomic so its non-zero default doesn't drag the slots into .data.
```

## L43 · `static VOLUME: AtomicU8 = AtomicU8::new(80); // master, 0..=100 %`

```
// master, 0..=100 %
```

## L45 · `pub fn open() -> i32 {`

```
/// Allocate a streaming slot. Returns the slot index or -1 if all are in use.
```

## L61 · `pub fn close(slot: usize) {`

```
/// Release a streaming slot (discards anything still buffered).
```

## L69-71 · `pub fn reset(slot: usize) {`

```
/// Empty a slot's ring without releasing it. Used when a still-held stream is
/// re-prepared (e.g. cubeb re-initialising after an underrun without a clean
/// RELEASE): reuse the slot + drop stale PCM instead of leaking a fresh one.
```

## L79-80 · `pub fn submit(slot: usize, data: &[u8]) -> usize {`

```
/// Append PCM to a slot's ring. Returns the number of bytes accepted (fewer
/// than `data.len()` when the ring is full — the submitter must retry/pace).
```

## L97-98 · `pub fn play_oneshot(data: &[u8]) -> bool {`

```
/// Play a self-contained sound: grab a free slot, fill it, and auto-free it
/// once the driver has drained it. For short one-shots (must fit in a slot).
```

## L115-118 · `pub fn poll_mix(out: &mut [u8]) -> usize {`

```
/// Driver side: mix every active slot into `out` (S16 add + clamp), scale by
/// master volume, and advance each slot. Always fills `out` fully (silence
/// where slots are empty), so the driver always has a complete buffer.
/// Returns the number of bytes written (a whole number of frames).
```

## L133 · `s.drained += BYTES_PER_FRAME as u64; // real-clock pacing for virtio-snd`

```
// real-clock pacing for virtio-snd
```

## L151-154 · `pub fn free_space(slot: usize) -> usize {`

```
/// Free bytes in a slot's ring (0 if the slot is closed). The virtio-snd
/// bridge uses this to pace the guest: it only completes a PCM period once
/// the period fits, so the mailbox drain rate (= real playback at 48 kHz)
/// becomes the guest's audio clock — no separate timer needed.
```

## L162-165 · `pub fn buffered(slot: usize) -> usize {`

```
/// Bytes currently buffered in a slot's ring (0 if closed). The virtio-snd
/// bridge reports this (plus the HDA ring) as the PCM-received-but-not-yet-played
/// latency in each tx completion, so the guest's A/V sync accounts for our
/// hidden host buffering.
```

## L173 · `pub fn set_volume(pct: u8) {`

```
/// Set master volume (0..=100 %).
```

## L181 · `pub fn get_volume() -> u8 {`

```
/// Get master volume (0..=100 %).
```

