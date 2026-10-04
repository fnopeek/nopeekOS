# `tools/wasm/tune/src/aac.rs` @ 5e0102684

## L1-8 · `use crate::mp4;`

```
//! AAC aus einer MP4 — die Tonhaelfte des Containers.
//!
//! Der Dekoder liegt in `<repo>/tools/wasm/vendor/rusty_aac` (siehe dort
//! VENDOR.md); hier steht nur, was er nicht wissen will: wo die Rahmen in der
//! Datei liegen, wie lang der Strom ist, und wohin ein Sprung fuehrt.
//!
//! Ein AAC-Rahmen traegt 1024 Samples je Kanal und passt damit in
//! [`MAX_BLOCK_FRAMES`](crate::source::MAX_BLOCK_FRAMES) (1152).
```

## L19 · `next: usize,`

```
/// Naechster Rahmen, den `next_block` dekodiert.
```

## L21 · `frame: u64,`

```
/// Ausgabeposition in Frames der Quellrate.
```

## L26 · `pub fn from_track(data: &'static [u8], track: mp4::Track) -> Option<Aac> {`

```
/// `track` muss die Tonspur eines bereits geparsten MP4 sein.
```

## L35-37 · `let total = track.duration as u128 * cfg.sample_rate as u128`

```
// Die Dauer kommt aus der Sampletabelle und nicht aus einer Schaetzung
// — und sie zaehlt in der QUELLrate, weil der Rufer danach mit
// `info().rate` weiterrechnet.
```

## L41 · `let bytes: u64 = track.samples.iter().map(|s| s.size as u64).sum();`

```
// Bitrate aus den wirklichen Bytes: Summe der Rahmen durch die Dauer.
```

## L64-72 · `pub fn priming(&self) -> u64 {`

```
/// Wieviele Samples der Strom vor seinem ersten HOERBAREN ueberspringt.
///
/// Ein AAC-Encoder beginnt mit Vorlaufsamples, die nicht zum Ton
/// gehoeren, und die Edit-List des Containers sagt, wieviele. Gemessen an
/// einem Handyvideo: ffmpeg schneidet **genau** `edit_start` = 2112
/// Samples weg, und unser Dekoder stimmt danach bis auf 1 LSB.
///
/// Das ist keine Kosmetik: die Videospur derselben Datei hatte 0. Wer
/// beide Zeitachsen roh spielt, hat Ton und Bild 48 ms auseinander.
```

## L87-88 · `let Ok(d) = self.dec.decode(au, None) else { continue };`

```
// Ein abgelehnter Rahmen ist ein Loch und kein Ende: der naechste
// faengt sich wieder, weil jeder AAC-Rahmen fuer sich steht.
```

## L100-102 · `let rate = self.info.rate.max(1) as u128;`

```
// Rahmen suchen, dessen Zeitspanne `frame` enthaelt. Der Dekoder
// faengt bei AAC-LC an jedem Rahmen an; der erste danach klingt
// weich, weil ihm die Ueberlappung des Vorgaengers fehlt.
```

