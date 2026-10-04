# `tools/wasm/tune/src/lib.rs` @ 5e0102684

## L1-17 · `#![no_std]`

```
//! tune — the audio player for nopeekOS.
//!
//! Layout (top → bottom):
//!   body — the picture (video), or title + folder playlist (audio)
//!   seek — `Widget::Slider`, edge to edge; seeks on release
//!   bar  — play/pause · −10 s · +10 s · "24:10 / 46:02" · spacer · volume
//!          (volume opens a popover with its own slider)
//!
//! The player itself knows no formats. It pulls f32 frames out of a
//! [`source::Source`], hands them to [`sink::Sink`] (resample → 48 kHz S16
//! stereo → kernel audio mailbox), and the HDA driver takes it from there.
//! Adding FLAC or Opus later means adding a `Source`, not touching this
//! file — see `src/source.rs`.
//!
//! Decoding costs about 6 % of one core on the device (measured under the
//! kernel's wasmi, 44.1 kHz / 128 kbps), so the loop below decodes ahead in
//! small steps between event polls rather than in one burst.
```

## L35-36 · `mod mp4;`

```
// Der Container. Noch nicht verdrahtet — der Spieler kommt als eigener
// Schnitt; geprüft ist er host-seitig gegen ffmpeg (<tools>/mediabench).
```

## L51-52 · `#[unsafe(link_section = ".npk.caps")]`

```
// Read files + draw. The audio mailbox is ungated — playback is not a
// security boundary, and the kernel holds no format knowledge to protect.
```

## L59 · `const EVENT_BUF_SIZE: usize = 4 * 1024;`

```
// ── Buffers ───────────────────────────────────────────────────────────
```

## L72 · `static mut BLOCK: [f32; MAX_BLOCK_SAMPLES] = [0.0; MAX_BLOCK_SAMPLES];`

```
/// One decoded block, interleaved f32 at the source rate.
```

## L102-112 · `#[global_allocator]`

```
// ── Bump allocator ────────────────────────────────────────────────────
//
// Everything the player keeps is small: the playlist, one decoder state,
// and a scene tree rebuilt from scratch each frame. The file bytes do NOT
// live here — they are claimed with `memory.grow` (see `file_arena`), so a
// four-minute song never has to fit in a fixed heap.
// Eine Halde, die FREIGIBT (`nopeek_widgets::heap`, seit widgets 0.28.0).
// Der Bump-Allokator davor reichte, solange tune nur Tonbloecke dekodierte:
// ein paar Kilobyte je Runde, und die Szene wurde mit einer Marke
// zurueckgedreht. Ein Videobild ist 0,5 MB, und je Sekunde kommen sechzehn
// davon — ein Allokator ohne Freigabe waere nach wenigen Sekunden voll.
```

## L119-125 · `const WASM_PAGE: usize = 64 * 1024;`

```
// ── File arena ────────────────────────────────────────────────────────
//
// A song is a few megabytes and the next one is a different few. A static
// array would have to be sized for the longest file anyone owns and would
// be paid for at launch, by everyone. Instead the arena is claimed with
// `memory.grow` at the size the folder listing says this file has, and
// reused for every track after that.
```

## L138-142 · `let fresh = (prev * WASM_PAGE) as *mut u8;`

```
// Seit der Umstellung auf `nopeek_widgets::heap` sind wir NICHT
// mehr der einzige Rufer von `memory.grow` — die Halde waechst
// ebenso. Das macht nichts: `memory_grow` gibt die vorige
// Seitenzahl zurueck, die frischen Seiten dahinter gehoeren uns
// allein, und die alte Belegung wird schlicht vergessen.
```

## L150-151 · `fn fetch_file(path: &str, size: usize) -> Option<&'static [u8]> {`

```
/// Read a whole file into the arena. `size` comes from the folder listing;
/// a wrong guess only costs a second claim.
```

## L160 · `struct Track { name: String, size: u64 }`

```
// ── State ─────────────────────────────────────────────────────────────
```

## L172 · `drained: bool,`

```
/// End of the decoded stream reached; the mailbox may still be draining.
```

## L175 · `opened_with_file: bool,`

```
/// Launched with a file to open, as opposed to launched bare.
```

## L178 · `vol_open: bool,`

```
/// Volume popover open.
```

## L180-181 · `scrub:   Option<u16>,`

```
/// Where the seek slider is being dragged to, while it is. The time
/// readout follows it; the seek itself waits for the release.
```

## L183 · `scrub_resume: bool,`

```
/// Was playing when the seek drag started — resume after the seek.
```

## L185-186 · `slide_drawn_at: i64,`

```
/// Last scene committed for a drag step (ms). The compositor moves the
/// thumb itself; what the app redraws mid-drag is only the readout.
```

## L188 · `motion_at: i64,`

```
/// Last pointer movement over the window (ms), for hiding the bar.
```

## L190 · `over_bar: bool,`

```
/// The last movement was over the bar itself: keep it up.
```

## L192 · `controls_shown: bool,`

```
/// Whether the scene on screen shows the bar.
```

## L194 · `shown_s: i64,`

```
/// Seconds last drawn, so the loop only re-commits when the clock moves.
```

## L196 · `told_underruns: u32,`

```
/// Underruns already reported, so a stutter logs once and not per tick.
```

## L198-199 · `video: Option<video::Video>,`

```
/// The video half, when the file has one. `None` is an ordinary audio
/// track and every line below behaves exactly as it did before.
```

## L201-204 · `video_t0: i64,`

```
/// Wall-clock tick at which the current video started, so the picture
/// time is `now - started`. The audio sink's clock is the better one and
/// takes over the moment tune plays a film WITH sound; for a silent file
/// there is nothing to synchronise to.
```

## L206 · `video_ms: i64,`

```
/// Video time of the last frame handed to the compositor, for the UI.
```

## L208 · `told_lag_s: i64,`

```
/// Second of playback the lag was last reported for.
```

## L210-213 · `decode_est_ms: i64,`

```
/// Was eine Dekodierung gerade kostet, gleitend gemittelt. Die Zahl
/// entscheidet, ob eine Runde noch dekodieren darf, ohne das naechste
/// Bild zu verpassen — und sie aendert sich mit der Szene um das
/// Dreifache, taugt also nicht als Konstante.
```

## L215 · `audio_priming_ms: i64,`

```
/// Vorlauf der Tonspur in ms — was vor dem ersten hoerbaren Sample liegt.
```

## L217 · `video_buffering: bool,`

```
/// Still filling the queue before the clock starts.
```

## L219-221 · `sec_decode_ms: i64,`

```
/// Je Sekunde gesammelt: Zeit im Dekoder, Zeit im Commit, Bilder.
/// Die Uhr hat 10-ms-Koernung, also taugt nur die SUMME ueber eine
/// Sekunde — ein einzelnes Bild zu messen waere Rauschen.
```

## L237 · `const CONTROLS_HIDE_MS: i64 = 2500;`

```
/// How long the bar stays over a playing video after the pointer stops.
```

## L239 · `const SLIDE_REDRAW_MS: i64 = 100;`

```
/// At most this often a new scene while a slider is being dragged.
```

## L242 · `const JUMP_MS:      u64 = 10_000;`

```
/// What the two jump buttons move, in ms.
```

## L244 · `const NODE_VOL:     u32 = 1;`

```
/// Anchor of the volume popover.
```

## L246-249 · `const LIST_WINDOW:  usize = 200;`

```
/// Playlist rows drawn at once. The scene is rebuilt every second while
/// playing, so a folder of two thousand files would re-encode and re-lay-out
/// two thousand rows per second for a clock that moved by one digit. The
/// window follows the current track; skipping still walks the whole folder.
```

## L251 · `const VIDEO_CANVAS: i32 = 0;`

```
/// Die eine Leinwand. Eine App darf mehrere haben; ein Spieler zeigt ein Bild.
```

## L300 · `let home = read_home_dir();`

```
// No argument: the music folder, if the user has one.
```

## L328-331 · `fn load(&mut self, play: bool) {`

```
/// Open the current track. `play` decides whether it starts: opening a
/// file (loft double-click, `run tune <file>`) means "play this", while
/// launching the player bare means "here is the folder" — a window that
/// starts making noise because it was opened is a rude window.
```

## L345-347 · `let d = demux::open(bytes);`

```
// EINE Lesung des Containers, zwei Stroeme. Nach INHALT
// entschieden, nicht nach Endung — dieselbe Regel, nach der
// `source::open` seit je den Tondekoder waehlt.
```

## L361-363 · `let rate = src.info().rate.max(1);`

```
// Der Vorlauf der TONspur, in ihrer eigenen Rate. Die
// Videospur derselben Datei hat oft einen anderen; beide roh
// zu spielen ist der einfachste Weg zu 48 ms Versatz.
```

## L372-374 · `self.error = Some(match d.video_error {`

```
// Der Grund gehoert auf den Schirm. Ein fragmentiertes MP4 ist
// eine andere Auskunft als ein Codec, den wir nicht bauen, und
// nur eine davon ist unser Fehler.
```

## L388-389 · `fn log_format(&self) {`

```
/// What was opened, once per file. The player shows no format line, so
/// the serial log is where a device run says what it is measuring.
```

## L411-418 · `fn position_ms(&self) -> u64 {`

```
/// Play position in ms — what the speaker has reached, not what the
/// decoder has read.
/// Die Wiedergabestelle, wie sie der Betrachter sieht.
///
/// Mit Ton ist der TON die Uhr, auch wenn ein Bild dazu laeuft: er
/// zaehlt, was wirklich gehoert wurde. Ein STUMMER Film hat nichts, woran
/// er sich haengen koennte — dort ist die Wanduhr die Uhr, und das ist
/// keine Notloesung, sondern die einzige verfuegbare Zeit.
```

## L425-430 · `fn audio_ms(&self) -> i64 {`

```
/// Gehoerte Zeit auf der PRAESENTATIONS-Achse: was der Ring hergibt,
/// minus den Vorlauf, der vor dem ersten hoerbaren Sample liegt.
///
/// Die Videospur rechnet ihre `pts` schon gegen ihr eigenes
/// `edit_start`; beide landen damit auf derselben Null, und genau das
/// ist Lippensynchronitaet.
```

## L441-443 · `fn video_tick(&mut self) {`

```
/// Advance the picture and hand it to the compositor. Called once per
/// turn of the loop, like `pump` — decoding a frame costs milliseconds,
/// and it belongs where the event loop can see it.
```

## L449-453 · `if self.video_buffering {`

```
// Vor dem Start erst Vorrat anlegen. Ohne Ton wird die Wanduhr
// MITGEZOGEN statt angehalten — sonst zaehlt die Pufferzeit als
// Rueckstand, und die erste Meldung des Laufs waere eine ueber ein
// Problem, das gerade behoben wird. Mit Ton haelt der Sink ohnehin
// an, solange nichts eingespeist wurde.
```

## L460-462 · `if let Some(f) = v.take_due(at) {`

```
// Das erste Bild wird dabei schon gezeigt. Ein schwarzer Kasten,
// waehrend im Hintergrund gepuffert wird, sieht aus wie ein
// Fehler; das stehende erste Bild sieht aus wie das, was es ist.
```

## L472 · `let ms = self.clock_ms(has_audio, now);`

```
// ── Uhr lesen ─────────────────────────────────────────────────────
```

## L477-488 · `let mut showed = false;`

```
// ZEIGEN ZUERST, dann dekodieren — und beides in derselben Runde.
//
// Bis 0.4.5 stand hier eine Wahl: war ein Bild faellig, bekam die
// Runde Budget 0. Gemessen am echten Film nahm das **27 % aller
// Runden** das Dekodieren weg, obwohl ein Commit nur 3 ms von 33
// kostet — und der Vorlauf wurde zum Saegezahn (143 bis 1563 ms)
// statt auf seinen 1500 zu stehen. Mit halbem Puffer traf er die
// Ueberblendung, und DAS war das Ruckeln: 106 Bilder mehr als eine
// Periode zu spaet, ueber vier Sekunden verteilt.
//
// Umgedreht ist die Frage beantwortet, statt gestellt: das faellige
// Bild ist schon auf dem Schirm, wenn die Dekodierung beginnt.
```

## L495-498 · `host::canvas_commit_yuv(VIDEO_CANVAS, &f.y, &f.u, &f.v, ys, cs,`

```
// Der Commit ist NICHT gratis: er kopiert die drei Ebenen
// ueber die Modulgrenze und laesst das Fenster neu rastern,
// und beides laeuft im Wirtsaufruf, also seriell zum
// Dekodieren. Deshalb getrennt gemessen.
```

## L506-507 · `let ms = self.clock_ms(has_audio, host::ticks());`

```
// Die Uhr NEU lesen: der Commit hat gedauert, und das Dekodieren
// richtet sich nach dem Vorlauf, der seither kleiner ist.
```

## L519-520 · `self.decode_est_ms = (self.decode_est_ms * 3 + took) / 4;`

```
// Gleitendes Mittel, 1:3. Es steuert nichts mehr — es steht im
// Bericht, und dort sagt es, warum eine Sekunde teuer war.
```

## L525-526 · `let ms = self.clock_ms(has_audio, host::ticks());`

```
// Und noch einmal die Uhr: die Dekodierung war lang, und der
// Rueckstand, den der Bericht gleich nennt, ist der von JETZT.
```

## L539-542 · `let (dropped, shown) = {`

```
// ERST HIER leeren. Beim Umbau auf die Tonuhr standen die beiden
// `take` eine Ebene zu weit aussen und liefen bei JEDEM Tick —
// die Meldung zaehlte dann einen Tick statt einer Sekunde und
// sagte „1 Bilder, 1 verworfen", egal was lief.
```

## L547-549 · `if dropped > 0 || lag > 150 {`

```
// Gemeldet wird, was ERKLAERT: ein verworfenes Bild ist das, was
// das Auge sieht, auch wenn die Uhr stimmt. Und die zwei Zeiten
// daneben sagen, WOHIN die Sekunde ging.
```

## L574-580 · `fn pump(&mut self) {`

```
/// Decode ahead until the mailbox holds `TARGET_LEAD_MS`. Runs between
/// event polls, so each visit does a little and returns.
///
/// Bei einem Film mit Ton ist das der Weg, der die UHR fuellt: der Bildweg
/// folgt ihr, also darf sie nicht leerlaufen. `drained` faellt deshalb
/// hier und im Bildweg unabhaengig — ein Film endet, wenn BEIDE fertig
/// sind, nicht wenn einer es ist.
```

## L583 · `if !self.sink.flush() { return; }   // ring still full from last time`

```
// ring still full from last time
```

## L600 · `fn clock_ms(&self, has_audio: bool, now: i64) -> i64 {`

```
/// Die Zeit, der das Bild folgt. Mit Ton ist es der Ton, sonst die Wand.
```

## L613-614 · `self.motion_at = host::ticks();`

```
// Die Leiste bleibt nach jedem Wechsel noch kurz stehen, sonst
// verschwindet sie unter dem Finger, der gerade Play gedrueckt hat.
```

## L617 · `if self.drained { self.seek_to_ms(0); }`

```
// Am Ende stehen geblieben: Play heisst von vorn.
```

## L620-621 · `self.sink.resume(host::ticks());`

```
// Without this the first tick after a pause charges the whole
// paused stretch to the speaker and reports a phantom underrun.
```

## L624-625 · `self.video_t0 = host::ticks() - self.video_ms;`

```
// Die Wanduhr lief weiter, also wird der Nullpunkt
// nachgezogen statt die Pause mitgezaehlt.
```

## L630-632 · `if self.src.is_some() {`

```
// Pausieren mit Ton heisst: den Vorlauf wegwerfen. Sonst spielt der
// Ring noch eine halbe Sekunde weiter, waehrend das Bild steht — und
// beim Fortsetzen waere er doppelt zu hoeren. Springen tut genau das.
```

## L639-643 · `fn seek_to_ms(&mut self, ms: u64) {`

```
/// Auf `ms` der PRAESENTATIONS-Achse springen — beide Stroeme.
///
/// Der Ton bestimmt, wo es wirklich hingeht (er kann auf jeden Rahmen),
/// und das Bild folgt auf sein naechstes Synchronbild davor. Andersherum
/// waere der Ton an einer Stelle, an der das Bild noch nichts zeigt.
```

## L649-650 · `let src_frame =`

```
// Der Vorlauf gehoert dazugerechnet: die Praesentationsachse
// faengt NACH ihm an, die Rahmen der Spur davor.
```

## L666-668 · `self.video_buffering = true;`

```
// Nach einem Sprung ist die Schlange leer — erst wieder Vorrat
// anlegen, sonst ruckelt genau die Stelle, auf die man gezeigt
// hat.
```

## L682-684 · `fn controls_visible(&self, now: i64) -> bool {`

```
/// The bar is always there for audio. Over a video it gives way to the
/// picture while it plays, and comes back when paused, when the pointer
/// moves, or while something on it is in use.
```

## L696 · `fn fmt_time(ms: u64) -> String {`

```
// ── Rendering ─────────────────────────────────────────────────────────
```

## L721-722 · `if t.files.is_empty() && t.src.is_none() && t.video.is_none() {`

```
// Nothing in the folder and nothing opened: no player to show, only
// the way to a file.
```

## L728 · `Some(_) => Widget::Canvas {`

```
// Das BILD bekommt die ganze Flaeche.
```

## L731-736 · `width: 320,`

```
// NICHT die Videogroesse. `measure_intrinsic` nimmt diese Zahlen
// als UNTERGRENZE der Spalte; ein 2560 breites Bild haette auf
// einem 1920er Schirm das Layout getrieben statt sich einzufuegen.
// Die wirkliche Groesse entsteht aus `Flex(1)` und dem
// contain-fit des Compositors, der das gespeicherte Bild
// unabhaengig davon einpasst.
```

## L744-745 · `let shown = match t.scrub {`

```
// While dragging, the readout shows where the thumb is, not where the
// decoder is — that is the number the hand is looking for.
```

## L803-804 · `Some(_) => {`

```
// Die Leiste liegt UEBER dem Bild, damit es beim Ein- und
// Ausblenden nicht die Groesse wechselt.
```

## L806-809 · `let mut over = alloc::vec![Widget::Column {`

```
// Ein Klick aufs Bild ist Play/Pause. Die Klickflaeche liegt in
// der oberen Ebene UEBER der Leiste und nicht am Bild selbst:
// der Treffertest nimmt das erste Kind, das trifft, und das Bild
// deckt die ganze Flaeche — die Knoepfe waeren darunter tot.
```

## L886-887 · `fn audio_body(t: &Tune, title: &str, artist: &str) -> Widget {`

```
/// Without a picture the area belongs to the folder: what is playing on
/// top, every track below it.
```

## L943 · `fn open_dialog(t: &Tune) {`

```
/// The system file dialog, starting in the folder tune is looking at.
```

## L949-950 · `fn slide_redraw(t: &mut Tune) -> Outcome {`

```
/// A mid-drag redraw, throttled: every scene is a full layout and raster
/// in the compositor, and a drag sends a step with every mouse packet.
```

## L958 · `enum Outcome { Idle, Render, Exit }`

```
// ── Events ────────────────────────────────────────────────────────────
```

## L978 · `if path.is_empty() { return Outcome::Idle; }`

```
// Leer = abgebrochen.
```

## L991-992 · `t.point_at(payload);`

```
// Already running and asked to open another file (loft
// double-click): switch tracks rather than spawning a twin.
```

## L997-998 · `Event::Slide { action: ActionId(A_SEEK), value, done } => {`

```
// Anfassen haelt an, Loslassen springt und spielt weiter, wenn es
// vorher lief — waehrend der Hand laeuft nichts unter ihr weg.
```

## L1015-1016 · `Event::Slide { action: ActionId(A_VOL), value, done } => {`

```
// Volume follows the hand: a level is cheap to set and the ear is
// the feedback.
```

## L1029-1030 · `if id == A_MOTION || id == A_MOTION_BAR {`

```
// Bewegung allein zeichnet nichts neu; die Schleife entscheidet,
// ob die Leiste dadurch kommt oder bleibt.
```

## L1054 · `fn read_home_dir() -> String {`

```
// ── npkFS helpers ─────────────────────────────────────────────────────
```

## L1064-1065 · `fn is_media(name: &str) -> bool {`

```
/// Was die Ordnerliste annimmt. Ton UND Bewegtbild, an einer Stelle, damit
/// ein neues Format nicht an zwei Orten nachgetragen werden muss.
```

## L1067-1068 · `let lower = {`

```
// `.m4a` ist MP4 mit Tonspur und ohne Bild — derselbe Demuxer, also
// gehoert es hierher und nicht in eine dritte Liste.
```

## L1115 · `const TICK_MS: i32 = 10;`

```
// ── Main loop ─────────────────────────────────────────────────────────
```

## L1117-1119 · `const TICK_MS: i32 = 10;`

```
/// Poll cadence while playing. The tick is 10 ms, so anything smaller is a
/// lie (see the kernel's sleep granularity); anything larger eats into the
/// 600 ms lead the mailbox is holding.
```

## L1124-1128 · `log(concat!("[tune] version ", env!("CARGO_PKG_VERSION")));`

```
// Die Version ZUERST, vor allem anderen. Ohne sie sagt ein Log nicht,
// welchen Bau er misst — und eine Messung aus dem falschen Bau ist
// schlimmer als keine. Konkret passiert 2026-09-17: ein Lauf, der
// Zeile fuer Zeile mit dem vorigen identisch war, und keine Zeile im
// Log, die das haette entscheiden koennen.
```

## L1131 · `commit_scene(&mut t);      // window appears before the first fetch`

```
// window appears before the first fetch
```

## L1137-1140 · `let now = host::ticks();`

```
// Clock and pump run on EVERY turn, not only when the poll came up
// empty: a stream of mouse-move events would otherwise starve the
// decoder for as long as the hand keeps moving, and the mailbox
// holds 600 ms.
```

## L1142-1147 · `t.sink.tick(now, t.playing && t.src.is_some());`

```
// `playing` heisst „der Spieler laeuft", der Sink braucht aber „es
// klingt gerade". Bei einem stummen Film sind das zwei verschiedene
// Dinge: `submitted` bleibt 0, die Wanduhr schiebt `played` vor, und
// die Aussetzer-Bedingung ist bei JEDEM Takt wahr — hundertmal je
// Sekunde. Ein Zaehler, dessen Voraussetzung nicht geprueft wird,
// meldet ununterbrochen und sagt damit nichts mehr.
```

## L1152-1155 · `t.told_underruns = t.sink.underruns;`

```
// A stutter that only shows as a click is a measurement thrown
// away — the serial mirror gets the count. Checked here and not
// in the redraw below, because a hard stall freezes the clock
// and the redraw with it.
```

## L1173-1174 · `if t.drained && t.playing && (t.video.is_some() || t.sink.lead_frames() == 0) {`

```
// The track ends when the mailbox has drained, not when the
// decoder ran out — otherwise the last second is cut off.
```

## L1177-1178 · `if t.video.is_none() {`

```
// Ein Film bleibt am Ende stehen; Musik laeuft durch den
// Ordner weiter.
```

## L1185-1186 · `let secs = (t.position_ms() / 1000) as i64;`

```
// Redraw once a second while playing: the clock and the
// progress bar are the only things that move.
```

## L1194-1209 · `let nap = match (t.playing, t.video.as_ref()) {`

```
// Paused, there is nothing to keep up with — poll a quarter
// as often and leave the core alone.
// Nicht fest schlafen, sondern bis zum naechsten faelligen
// Bild — hoechstens aber die uebliche Runde. Zehn feste
// Millisekunden schieben eine Runde ueber die Bildperiode,
// und weil eine Runde nur EIN Bild zeigt, faellt dort dann
// genau eines aus.
// Geschlafen wird erst, wenn der Vorrat VOLL ist — und dann
// bis zum naechsten faelligen Bild. Solange er es nicht ist,
// ist jede geschlafene Millisekunde eine, die in der teuren
// Szene fehlt; das Dekodieren laeuft schneller als Echtzeit
// (22 ms je Bild bei 33 ms Periode), und genau diese Luecke
// ist der Puffer.
// Ueber der Vorrats-Marke wird bis zum naechsten faelligen
// Bild gewartet — die Runde gehoert dann dem Zeigen. Darunter
// zaehlt jede Millisekunde fuers Dekodieren.
```

