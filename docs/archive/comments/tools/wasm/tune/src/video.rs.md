# `tools/wasm/tune/src/video.rs` @ 5e0102684

## L1-7 · `use alloc::vec::Vec;`

```
//! The video half: MP4 samples in, one picture at the right moment out.
//!
//! Pictures come out of the decoder in DECODE order; a B-frame is decoded
//! before the frame it sits between. Which one to SHOW when is a question the
//! container already answers — every sample carries its presentation time —
//! so the reordering here is a small queue sorted by `pts`, not a second
//! calculation from picture order counts.
```

## L16-19 · `const REORDER: usize = 4;`

```
/// Pictures held before one is handed out. Four covers the reorder depth of
/// everything a camera or an encoder with default settings produces; a stream
/// that needs more shows a frame late rather than out of order, because the
/// queue always emits the smallest `pts` it holds.
```

## L22-25 · `pub const FILL_DECODES: usize = 4;`

```
/// Decodes per call while FILLING UP — before the clock runs.
///
/// Only here may a call take several: nobody is watching yet, and the whole
/// point is to get a lead before the first picture moves.
```

## L28-40 · `pub const PLAY_DECODES: usize = 1;`

```
/// Decodes per call while PLAYING. One, and the reason is the whole bug.
///
/// A turn of the caller's loop shows at most ONE picture. So the display
/// rate is the TURN rate, not the decode rate — and every decode a turn does
/// makes that turn longer. Measured at the device, 1440p30: four decodes at
/// 22 ms are 88 ms, which is 2.6 frame periods in one turn, so two or three
/// pictures fall due unseen. The log said it plainly: „30x dekodiert" (the
/// machine keeps up) next to „4 verworfen" (a quarter never shown).
///
/// With one, a turn is one decode plus one commit, and it fits inside a
/// frame period with room to spare. When decoding is slower than that — the
/// expensive scene — the LEAD drains instead, which is exactly what a lead
/// is for, and the picture keeps its rate until the lead is gone.
```

## L43-69 · `const TARGET_LEAD_MS: i64 = 1500;`

```
/// How far ahead of the clock to decode, in ms of PICTURE time.
///
/// Four frames of reorder depth is enough to put pictures in order and not
/// enough to absorb anything. The cost of decoding follows the BITS, not the
/// pixels: a day-to-night crossfade leaves every macroblock with a large
/// residual and costs several times what a talking head costs. The machine
/// holds the average and misses the peak — unless the cheap stretches, where
/// it is otherwise idle, are used to build a lead. That is what the audio
/// sink has done from the start (`sink::TARGET_LEAD_MS`).
///
/// **1500 ms, und die Zahl ist ausgerechnet.** Florians Lauf mit 0.2.9,
/// 1440p30, Tag/Nacht-Ueberblendung, Sekunde fuer Sekunde:
///
/// `​``text
///   30 faellig, 21 dekodiert  ->  9 fehlen
///   30 faellig, 18 dekodiert  -> 12 fehlen
///   30 faellig, 30 dekodiert  ->  0
///                     Summe     21 Bilder = 700 ms
/// `​``
///
/// Der Vorlauf ging dabei von 880 auf **40 ms** — er hat 840 ms hergegeben
/// und war damit knapp zu flach. Ein Puffer, der bis auf 40 ms leerlaeuft,
/// hat die Szene nicht gedeckt, er hat sie ueberlebt.
///
/// Das Defizit einer Szene ist die Zahl, die den Vorlauf setzt, und nicht
/// die Bildrate oder das Bauchgefuehl. 2000 ms geben dem gemessenen Loch von
/// 700 ms gut den doppelten Spielraum; bei 1440p greift ohnehin der Bytedeckel.
```

## L72-100 · `const MAX_QUEUE_BYTES: usize = 256 * 1024 * 1024;`

```
/// And the real limit is BYTES, not frames.
///
/// 500 ms at 30 fps is fifteen pictures — 47 MB at 1080p and 83 MB at
/// 1440p. A cap counted in frames means the memory it costs depends on the
/// file, which is the same mistake as sizing a buffer by item count anywhere
/// else. So the lead is whichever comes first.
///
/// **Und es ist der Deckel, der wirklich greift** — zweimal in Folge zu
/// knapp, und beide Male nachgerechnet statt geraten.
///
/// Das Defizit der Tag/Nacht-Ueberblendung, Sekunde fuer Sekunde aus der
/// host-seitigen Messung (3,8 ms je Bild normal, 11,2 in der Szene) mal dem
/// Geraetefaktor 5,8, den Florians Log gegen dieselbe Datei hergibt:
///
/// `​``text
///   Sekunde 36:  30 Bilder x 64,8 ms = 1945 ms fuer 1000 ms Inhalt -> 945 ms fehlen
///   Sekunde 37:  30 Bilder x 43,4 ms = 1303 ms                     -> 303 ms fehlen
///                                                            Summe   1248 ms
/// `​``
///
/// Ein 1440p-Bild in I420 sind 5,27 MB. 192 MB reichten damit fuer 36 Bilder
/// = **1200 ms** — achtundvierzig Millisekunden zu wenig, und die Zeitgrenze
/// von 1500 kam nie zum Zug. 256 MB sind 48 Bilder = 1600 ms; ab da bindet
/// die Zeit und nicht der Speicher, und das ist die richtige Reihenfolge.
///
/// Es ist viel Speicher, und es ist der Preis dafuer, dass eine Ueberblendung
/// nicht sichtbar wird. Belegt wird er nur, wenn die billigen Szenen davor
/// Zeit uebrig hatten, und er schrumpft mit der Aufloesung von selbst: bei
/// 1080p sind dieselben 256 MB mehr, als die Zeitgrenze zulaesst.
```

## L103-113 · `const PREROLL_MS: i64 = 400;`

```
// Die Runde ZEIGT zuerst und dekodiert danach — und beides, nicht eines
// von beidem.
//
// Das war bis 0.4.5 zwei Konstanten und zwei Praedikate
// (`SHOW_FIRST_FLOOR_MS`, `SHOW_COST_MS`, `overloaded`,
// `show_before_decode`). Sie beantworteten die Frage „zeigen ODER
// dekodieren", und die Frage war falsch gestellt: ein Commit kostet 3 ms
// von 33, die anderen 30 gehoeren dem Dekodieren. Gemessen nahm die Regel
// **27 % aller Runden** ihr Budget, und der Vorlauf wurde zum Saegezahn
// zwischen 143 und 1563 ms statt auf 1500 zu stehen — womit er die teure
// Szene mit halbem Puffer traf.
```

## L115-117 · `const PREROLL_MS: i64 = 400;`

```
/// Lead to build before the clock starts. Less than the target, because the
/// rest can be built while playing and nobody wants to wait a second for a
/// three-second clip.
```

## L124 · `next: usize,`

```
/// Next sample to feed the decoder, in decode order.
```

## L126 · `decoded: u32,`

```
/// Samples decoded since the counter was last read.
```

## L128-129 · `au: Vec<u8>,`

```
/// Reused scratch for the Annex-B reframing, so a frame costs no
/// allocation beyond the picture itself.
```

## L131 · `queue: Vec<(i64, YuvFrame)>,`

```
/// Decoded but not yet shown, ascending by presentation time.
```

## L133-135 · `pub dropped: u32,`

```
/// Pictures decoded and thrown away because a newer one was already due.
/// Not a failure — it is what keeps the picture on the clock — but it is
/// what the eye SEES when the machine is short, so it gets counted.
```

## L137 · `pub shown_count: u32,`

```
/// Pictures handed to the screen.
```

## L139-141 · `queue_bytes: usize,`

```
/// Bytes the queue holds, tracked rather than recomputed: the planes do
/// not change size, and walking them per call to add up three `len()`s
/// would be work that grows with the lead we are trying to build.
```

## L143 · `shown: Option<(i64, YuvFrame)>,`

```
/// The picture currently on screen, and when it starts.
```

## L149 · `pub colour_flags: i32,`

```
/// Flags for `npk_canvas_commit_yuv`: bit 0 = Rec. 709, bit 1 = full range.
```

## L151 · `fed_all: bool,`

```
/// Every sample has been fed. The queue may still hold pictures.
```

## L155-157 · `pub fn is_video(name: &str) -> bool {`

```
/// Extensions the folder listing accepts as video. Next to [`Video::open`]
/// so a new container is registered in one place, the same way `is_audio`
/// sits next to `source::open`.
```

## L168 · `NotMp4,`

```
/// Not an MP4, or the box tree did not parse.
```

## L170 · `Fragmented,`

```
/// Fragmented: the sample tables live in the fragments, not in `moov`.
```

## L172 · `NoVideo,`

```
/// No video track, or one we do not decode.
```

## L177-181 · `pub fn from_track(data: &'static [u8], track: mp4::Track) -> Result<Video, OpenError> {`

```
/// `track` muss die Videospur eines bereits geparsten MP4 sein.
///
/// Der Container wird EINMAL geparst (siehe `demux`) — vorher tat es
/// jede Haelfte fuer sich, und zwei Parser auf derselben Datei sind zwei
/// Gelegenheiten, verschiedener Meinung zu sein.
```

## L209-210 · `fn prime(&mut self) {`

```
/// Hand the decoder its parameter sets. In MP4 they live in `avcC`, so a
/// decoder fed only the samples never sees them and every picture fails.
```

## L215 · `let _ = self.dec.decode(&self.au);`

```
// No picture in a parameter set; a refusal here is not an error.
```

## L224 · `fn feed_one(&mut self) -> bool {`

```
/// Decode one more sample into the queue. False when there are none left.
```

## L239-241 · `self.decoded += 1;`

```
// A sample the decoder refuses is one lost picture, not a lost film:
// the next sync sample starts it again. Silence here would hide a
// broken file, so the caller gets to log it.
```

## L244-245 · `let at = self.queue.partition_point(|(p, _)| *p <= pts);`

```
// Insert sorted; the queue is four long, so this is cheaper than
// keeping a heap and far cheaper than being wrong about order.
```

## L253-273 · `pub fn decode_step(&mut self, ms: i64, max_decodes: usize) {`

```
/// The picture that should be on screen at `ms`, if it changed since the
/// last call. `None` means "keep showing what you have".
///
/// Decoding happens here and nowhere else, so the cost lands on the
/// caller's clock and a slow frame shows up as a late frame rather than
/// as a stalled event loop.
///
/// Frames that are already late are decoded and NOT shown — only the
/// newest one that has come due reaches the caller. A picture cannot be
/// skipped without decoding it (the next one predicts from it), but it
/// can be skipped on the way to the screen, and that is where the whole
/// cost of a commit sits.
/// Decode ahead. Getrennt vom Zeigen, und das ist der Punkt: dazwischen
/// muss der Rufer die UHR NEU LESEN.
///
/// Vorher stand beides in einem Aufruf und gab gegen dasselbe `ms` aus,
/// das VOR dem Dekodieren gelesen wurde. Nach 41 ms Dekodieren ist die
/// Uhr aber 41 ms weiter, und die Bilder, die inzwischen faellig wurden,
/// sah die Ausgabeschleife nicht — die Dekodier-Runde zeigte also gar
/// nichts. Gemessen waren das 22 Bilder je Sekunde in der teuren Szene,
/// obwohl fertige danebenlagen.
```

## L279-288 · `pub fn take_due(&mut self, ms: i64) -> Option<&YuvFrame> {`

```
/// Das Bild, das jetzt dran ist — und alles davor faellt weg.
///
/// Frames that are already late are dropped rather than shown: a picture
/// cannot be skipped without decoding it (the next one predicts from
/// it), but it can be skipped on the way to the screen, and that is
/// where the whole cost of a commit sits.
///
/// Kostet KEIN Budget: ein spaetes Bild wegzuwerfen ist ein `remove` und
/// kein Dekodieren. Stand hier ein Budgetabbruch, gab ein Aufruf genau
/// EIN Bild heraus, waehrend die Uhr um die Aufrufdauer weiterlief.
```

## L293-294 · `if self.queue.len() < REORDER && !self.fed_all { break; }`

```
// Only take it if the queue is deep enough to know it is the
// earliest, or there is nothing left to come.
```

## L299 · `if advanced { self.dropped += 1; }   // der vorige kam nie hin`

```
// der vorige kam nie hin
```

## L311-314 · `pub fn take_decoded(&mut self) -> u32 {`

```
/// Decode ahead: deep enough to order pictures, then as far ahead of the
/// clock as the lead allows, and never past the byte cap.
/// Samples decoded since the last `take_decoded`. Der Rufer misst die
/// ZEIT selbst; hier wird nur gezaehlt, was sie verursacht hat.
```

## L320-322 · `while self.queue.len() < REORDER {`

```
// Die Reihenfolge-Tiefe ist eine Pflicht, kein Vorrat: ohne sie
// weiss die Schlange nicht, welches Bild das naechste ist. Sie geht
// deshalb VOR dem Budget und nicht aus ihm.
```

## L334-365 · `pub fn may_wait(&self, ms: i64) -> bool {`

```
/// Darf der Rufer schlafen, statt die Runde ans Dekodieren zu geben?
///
/// **Nur wenn der Vorrat voll ist, und das ist die ganze Regel.**
///
/// Bis 0.4.5 stand hier eine zweite Bedingung — „ueberlastet, aber es
/// ist noch Vorrat da, also hat das Zeigen Vorrang" —, dazu ein
/// `show_before_decode`, das der Runde ihr Dekodierbudget nahm, sobald
/// ein Bild faellig war. Die Beobachtung dahinter stimmte (eine Runde,
/// die 57 ms dekodiert, laesst zwei Bilder faellig werden und kann nur
/// eines zeigen); der Schluss war falsch, und er kostete **27 % aller
/// Runden**, die nicht dekodierten, obwohl sie es gekonnt haetten.
///
/// Nachgerechnet am echten Film — 1625 Bilder, Kosten je Bild aus
/// `<tools>/mediabench`, echte pts aus dem Container:
///
/// `​``text
///   Politik                 gezeigt  verworfen  SPAET  max Rueckstand
///   immer dekodieren           1542         83      0           33 ms
///   zwei Runden auslassen      1570         55     62          487 ms
///   Zeigen hat Vorrang         1580         45    106          916 ms
/// `​``
///
/// **Wer das Dekodieren auslaesst, tauscht gezeigte gegen SPAETE Bilder**
/// — und ein spaetes Bild ist genau das, was man sieht: das Bild bleibt
/// stehen und springt. Ein verworfenes sieht man nicht. Die Bildrate
/// faellt in der Ueberblendung so oder so; die Frage ist nur, ob sie
/// glatt faellt.
///
/// Die andere Haelfte der Antwort steht beim Rufer: die Runde ZEIGT
/// zuerst und dekodiert danach. Damit ist die Frage, die
/// `show_before_decode` stellen wollte, schon beantwortet, wenn sie
/// gestellt wuerde.
```

## L371-378 · `pub fn stocked(&self, ms: i64) -> bool {`

```
/// Ist der Vorrat voll? Nur dann darf der Rufer schlafen.
///
/// Die freie Kapazitaet steckt genau hier: dekodieren kostet am Geraet
/// 22 ms je Bild und eine Bildperiode ist 33 ms, also LIESSEN sich 45
/// Bilder je Sekunde dekodieren, wo 30 gebraucht werden. Wer in dieser
/// Luecke schlaeft, verschenkt sie — und steht in der teuren Szene ohne
/// Puffer da. Gemessen: mit festem Schlaf fiel der Vorlauf von 870 auf
/// 150 ms und der Rueckstand stieg auf 734.
```

## L383-385 · `pub fn next_due_in(&self, ms: i64) -> i64 {`

```
/// Wie lange bis zum naechsten faelligen Bild. Der Rufer schlaeft danach
/// — ein fester Schlaf schiebt eine Runde ueber die Bildperiode, und
/// dann faellt genau dort ein Bild aus.
```

## L390-395 · `pub fn primed(&self, ms: i64) -> bool {`

```
/// Enough decoded to start without stumbling.
///
/// Playback used to begin the moment the file opened, with nothing in
/// the queue — measured at the device: 207 ms behind with 26 ms of lead.
/// The first second is the one stretch where the decoder has no head
/// start at all, so it is the one stretch where waiting is free.
```

## L400-401 · `pub fn lead_ms(&self, ms: i64) -> i64 {`

```
/// Picture time the queue reaches beyond `ms`. Zero means the decoder is
/// hand to mouth, and the next expensive scene will be visible.
```

## L406-408 · `pub fn shown_ms(&self) -> i64 {`

```
/// Presentation time of the picture on screen. The gap to the caller's
/// clock IS the lag, and it is the number that says whether the machine
/// keeps up.
```

## L413 · `pub fn ended(&self) -> bool {`

```
/// Everything fed and nothing left to show.
```

## L418-421 · `pub fn seek(&mut self, ms: i64) -> i64 {`

```
/// Restart at the last sync sample at or before `ms`, and answer the time
/// it really landed on. A decoder started anywhere else produces garbage
/// until the next sync sample, so this is a jump to a key frame and the
/// caller must believe the number it gets back.
```

## L434-436 · `pub fn strides(f: &YuvFrame) -> (usize, usize) {`

```
/// Row strides for `npk_canvas_commit_yuv`. The decoder's planes are
/// tight, but the host call takes strides because a decoder is allowed
/// not to be — asking here keeps that promise in one place.
```

