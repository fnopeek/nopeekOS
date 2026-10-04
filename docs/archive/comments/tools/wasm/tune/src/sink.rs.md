# `tools/wasm/tune/src/sink.rs` @ 5e0102684

## L1-9 · `use crate::host;`

```
//! The mailbox side: source frames in, 48 kHz S16 stereo out.
//!
//! The kernel mixer takes one format and only one (48 kHz, S16LE, stereo),
//! so every source rate meets a resampler here. It also holds the play
//! clock: the mailbox is a ring the driver drains on the HDA crystal, and
//! the app cannot read its fill level — so what has actually been *heard*
//! is tracked from the wall clock and corrected whenever the ring pushes
//! back. `submitted - played` is the buffered lead, and that number decides
//! both how far ahead we decode and how quickly a pause takes effect.
```

## L15-17 · `pub const TARGET_LEAD_MS: u64 = 600;`

```
/// How far ahead of the speaker we keep the ring. Long enough to survive a
/// browser-sized hiccup on the worker core, short enough that pause and
/// seek feel immediate — the kernel ring itself holds 1.36 s.
```

## L20-21 · `const OUT_FRAMES: usize = 7168;`

```
/// Worst case one source block can become: 1152 frames at 8 kHz resampled
/// to 48 kHz. Sized so `push` never has to stop halfway through a block.
```

## L28-32 · `out_bytes: usize,`

```
/// Bytes written by `push`, and how many of them the kernel has taken.
/// Bytes, not frames: `npk_audio_submit` reports what it accepted in
/// bytes and is free to accept a partial frame. Counting in frames
/// would round that away and shift the stream by one sample — which
/// swaps left and right for the rest of the track.
```

## L35 · `submitted: u64,`

```
/// 48 kHz frames handed to the mailbox since the last resync.
```

## L37-38 · `anchor_ms: i64,`

```
/// Wanduhrzeit der letzten Ring-Lesung — der Anker, von dem aus
/// `played_frames_at` interpoliert.
```

## L40-42 · `base: u64,`

```
/// Stand beim letzten `restart`. Die Uhr darf nicht darunter fallen:
/// direkt nach einem Sprung ist der Ring leer, und `submitted - unheard`
/// laege 85 ms VOR der Stelle, auf die gesprungen wurde.
```

## L44 · `played:    u64,`

```
/// 48 kHz frames the driver has drained, from the wall clock.
```

## L75-79 · `pub fn restart(&mut self, rate: u32, now_ms: i64, at_frame_48k: u64) {`

```
/// Point the resampler at a new source rate and drop everything still
/// buffered — used on track change and on seek. Closing and reopening
/// the slot is the flush: the kernel offers no other way to discard a
/// ring, and without it a seek would keep playing the old position for
/// as long as the lead lasts.
```

## L93-94 · `pub fn dead() -> Sink {`

```
/// A player with no slot — all four were taken. It renders, it just
/// never makes a sound, and `load` says so instead of pretending.
```

## L103-109 · `const HDA_RING_FRAMES: u64 = 16_384 / 4;`

```
/// Bytes, die zwischen Mailbox und Lautsprecher noch warten.
///
/// `audio_hda` haelt einen eigenen Ring von 2 x 2048 Rahmen = 16384
/// Bytes, und der steht HINTER dem, was `npk_audio_buffered` meldet.
/// Der Kernel rechnet dieselbe Zahl fuer den microVM-Gast dazu
/// (`HDA_RING_BYTES` in `virtio_snd_pci.rs`) — wer sie weglaesst, laesst
/// das Bild 85 ms vor dem Ton laufen.
```

## L112-118 · `const ANCHOR_EVERY_MS: i64 = 100;`

```
/// Wie oft der Ring gefragt wird. Er korrigiert die Drift, und dafuer
/// reichen zehn Lesungen je Sekunde: der Takt der Karte und die Wanduhr
/// laufen um ppm auseinander, also um Mikrosekunden in 100 ms.
///
/// Die Zahl ist nicht Sparsamkeit. `npk_audio_buffered` nimmt
/// `SLOTS.lock()`, und dieselbe Sperre haelt der Treiber auf einem
/// anderen Kern, waehrend er 2048 Rahmen mischt.
```

## L121-127 · `pub fn tick(&mut self, now_ms: i64, playing: bool) {`

```
/// Advance the play clock. `playing` false freezes it, which is what
/// makes a pause hold its position.
///
/// Zwei Quellen mit klarer Aufgabenteilung: die **Wanduhr** schiebt
/// zwischen den Lesungen, der **Ring** faengt sie regelmaessig wieder
/// ein. Allein taugt keine — die Wanduhr driftet ueber einen Film, und
/// der Ring kostet bei jeder Lesung eine fremde Sperre.
```

## L133 · `self.played = (self.played + dt * MIX_RATE as u64 / 1000).min(self.submitted);`

```
// Zwischen zwei Ankern: schieben.
```

## L139-140 · `if self.played >= self.submitted && self.submitted > 0 {`

```
// Kein Schlitz — dann bleibt nur die Wanduhr, und ein Ueberholen
// des Eingespeisten IST der Aussetzer.
```

## L148-149 · `if ring == 0 && self.submitted > self.base {`

```
// Ein LEERER Ring, obwohl wir spielen, ist ein Aussetzer — und genau
// das, was die Wanduhr frueher nur raten konnte.
```

## L156-167 · `pub fn played_frames_at(&self, now_ms: i64) -> u64 {`

```
/// Gehoerte Rahmen zum Zeitpunkt `now_ms` — **ohne Wirtsaufruf.**
///
/// Der Ring ist der ANKER, die Wanduhr interpoliert dazwischen. Das ist
/// nicht Bequemlichkeit, sondern gemessen: `npk_audio_buffered` nimmt
/// `SLOTS.lock()`, und dieselbe Sperre haelt der Treiber auf einem
/// anderen Kern, waehrend er 2048 Rahmen mischt. Drei Lesungen je Tick
/// haben den Vorlauf des Bildwegs von 850 auf 150 ms gedrueckt — die
/// Kapazitaet ging ins Warten.
///
/// Die Wanduhr driftet gegen den Takt der Karte; das ist egal, solange
/// der naechste Anker sie wieder einfaengt. Genau deshalb ist sie
/// zwischen zwei Ankern richtig und ueber einen Film falsch.
```

## L173 · `pub fn lead_frames(&self) -> u64 { self.submitted.saturating_sub(self.played) }`

```
/// 48 kHz frames buffered ahead of the speaker.
```

## L176 · `pub fn played_frames(&self) -> u64 { self.played }`

```
/// 48 kHz frames the speaker has actually reached.
```

## L179-181 · `pub fn resume(&mut self, now_ms: i64) { self.last_ms = now_ms; }`

```
/// Restart the play clock without touching the buffer — the resume side
/// of a pause. Without it the first tick after a pause charges the whole
/// paused stretch to the speaker and reports a phantom underrun.
```

## L184-185 · `pub fn push(&mut self, block: &[f32], frames: usize, channels: usize) {`

```
/// Resample + convert one source block. Only legal when nothing is
/// pending; `out` is sized so a whole block always fits.
```

## L191-193 · `pub fn flush(&mut self) -> bool {`

```
/// Hand whatever is pending to the mailbox. Returns true when the
/// buffer emptied; false means the ring is full and the caller must
/// stop decoding until the next tick.
```

