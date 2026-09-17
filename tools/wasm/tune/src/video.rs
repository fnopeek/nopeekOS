//! The video half: MP4 samples in, one picture at the right moment out.
//!
//! Pictures come out of the decoder in DECODE order; a B-frame is decoded
//! before the frame it sits between. Which one to SHOW when is a question the
//! container already answers — every sample carries its presentation time —
//! so the reordering here is a small queue sorted by `pts`, not a second
//! calculation from picture order counts.

use alloc::vec::Vec;

use rusty_h264_common::YuvFrame;
use rusty_h264_decoder::Decoder;

use crate::mp4;

/// Pictures held before one is handed out. Four covers the reorder depth of
/// everything a camera or an encoder with default settings produces; a stream
/// that needs more shows a frame late rather than out of order, because the
/// queue always emits the smallest `pts` it holds.
const REORDER: usize = 4;

/// Decodes per call while FILLING UP — before the clock runs.
///
/// Only here may a call take several: nobody is watching yet, and the whole
/// point is to get a lead before the first picture moves.
pub const FILL_DECODES: usize = 4;

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
pub const PLAY_DECODES: usize = 1;

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
/// ```text
///   30 faellig, 21 dekodiert  ->  9 fehlen
///   30 faellig, 18 dekodiert  -> 12 fehlen
///   30 faellig, 30 dekodiert  ->  0
///                     Summe     21 Bilder = 700 ms
/// ```
///
/// Der Vorlauf ging dabei von 880 auf **40 ms** — er hat 840 ms hergegeben
/// und war damit knapp zu flach. Ein Puffer, der bis auf 40 ms leerlaeuft,
/// hat die Szene nicht gedeckt, er hat sie ueberlebt.
///
/// Das Defizit einer Szene ist die Zahl, die den Vorlauf setzt, und nicht
/// die Bildrate oder das Bauchgefuehl. 2000 ms geben dem gemessenen Loch von
/// 700 ms gut den doppelten Spielraum; bei 1440p greift ohnehin der Bytedeckel.
const TARGET_LEAD_MS: i64 = 1500;

/// And the real limit is BYTES, not frames.
///
/// 500 ms at 30 fps is fifteen pictures — 47 MB at 1080p and 83 MB at
/// 1440p. A cap counted in frames means the memory it costs depends on the
/// file, which is the same mistake as sizing a buffer by item count anywhere
/// else. So the lead is whichever comes first.
///
/// **Und es war der Deckel, der wirklich griff.** Bei 1440p sind 128 MB nur
/// 23 Bilder, also 775 ms — weniger als das gemessene Loch von 700 ms plus
/// Reserve. Die Zeitgrenze stand auf 1000 und kam nie zum Zug.
///
/// 192 MB sind bei 1440p rund 34 Bilder (1,15 s) und bei 1080p mehr, als die
/// Zeitgrenze zulaesst. Das ist viel Speicher, und es ist der Preis dafuer,
/// dass eine Ueberblendung nicht sichtbar wird; belegt wird er nur, wenn die
/// billigen Szenen davor Zeit uebrig hatten, und er schrumpft mit der
/// Aufloesung von selbst.
const MAX_QUEUE_BYTES: usize = 192 * 1024 * 1024;

/// Ab wieviel Vorrat das ZEIGEN vor dem Dekodieren kommt.
///
/// Gemessen an Florians Lauf, 1440p30, Tag/Nacht-Ueberblendung: **109 Bilder
/// dekodiert, 75 gezeigt** — 45 wurden bezahlt und nie gesehen. Der Grund
/// ist die Reihenfolge in der Runde: wer erst 52 ms dekodiert und dann EIN
/// Bild zeigt, laesst in der Zwischenzeit zwei faellig werden und wirft
/// eines davon weg.
///
/// Ein faelliges Bild zu zeigen kostet 3 ms. Liegt genug Vorrat da, gehoert
/// es also VOR das naechste Dekodieren — dann laeuft die Anzeige weiter mit
/// voller Rate, und der Vorrat bezahlt dafuer. Genau dafuer ist er da.
///
/// Der Boden ist nicht null: unter der Reihenfolge-Tiefe weiss die Schlange
/// nicht mehr, welches Bild das naechste ist, und dann muss dekodiert
/// werden, egal was faellig ist.
const SHOW_FIRST_FLOOR_MS: i64 = 150;

/// Lead to build before the clock starts. Less than the target, because the
/// rest can be built while playing and nobody wants to wait a second for a
/// three-second clip.
const PREROLL_MS: i64 = 400;

pub struct Video {
    data: &'static [u8],
    track: mp4::Track,
    dec: Decoder,
    /// Next sample to feed the decoder, in decode order.
    next: usize,
    /// Samples decoded since the counter was last read.
    decoded: u32,
    /// Reused scratch for the Annex-B reframing, so a frame costs no
    /// allocation beyond the picture itself.
    au: Vec<u8>,
    /// Decoded but not yet shown, ascending by presentation time.
    queue: Vec<(i64, YuvFrame)>,
    /// Pictures decoded and thrown away because a newer one was already due.
    /// Not a failure — it is what keeps the picture on the clock — but it is
    /// what the eye SEES when the machine is short, so it gets counted.
    pub dropped: u32,
    /// Pictures handed to the screen.
    pub shown_count: u32,
    /// Bytes the queue holds, tracked rather than recomputed: the planes do
    /// not change size, and walking them per call to add up three `len()`s
    /// would be work that grows with the lead we are trying to build.
    queue_bytes: usize,
    /// The picture currently on screen, and when it starts.
    shown: Option<(i64, YuvFrame)>,
    pub width: u32,
    pub height: u32,
    pub rotation: u16,
    pub duration_ms: u64,
    /// Flags for `npk_canvas_commit_yuv`: bit 0 = Rec. 709, bit 1 = full range.
    pub colour_flags: i32,
    /// Every sample has been fed. The queue may still hold pictures.
    fed_all: bool,
}

pub fn looks_like(d: &[u8]) -> bool {
    mp4::looks_like(d)
}

/// Extensions the folder listing accepts as video. Next to [`Video::open`]
/// so a new container is registered in one place, the same way `is_audio`
/// sits next to `source::open`.
pub fn is_video(name: &str) -> bool {
    let lower = {
        let mut s = alloc::string::String::with_capacity(name.len());
        for c in name.chars() { s.push(c.to_ascii_lowercase()); }
        s
    };
    [".mp4", ".m4v", ".mov"].iter().any(|e| lower.ends_with(e))
}

pub enum OpenError {
    /// Not an MP4, or the box tree did not parse.
    NotMp4,
    /// Fragmented: the sample tables live in the fragments, not in `moov`.
    Fragmented,
    /// No video track, or one we do not decode.
    NoVideo,
}

impl Video {
    pub fn open(data: &'static [u8]) -> Result<Video, OpenError> {
        let m = mp4::parse(data).ok_or(OpenError::NotMp4)?;
        if m.fragmented { return Err(OpenError::Fragmented); }
        let track = m.video.ok_or(OpenError::NoVideo)?;
        if !matches!(track.codec, mp4::Codec::Avc { .. }) || track.samples.is_empty() {
            return Err(OpenError::NoVideo);
        }
        let mut v = Video {
            width: track.width,
            height: track.height,
            rotation: track.rotation,
            duration_ms: track.duration_ms(),
            colour_flags: (track.colour.bt709 as i32) | ((track.colour.full_range as i32) << 1),
            data,
            track,
            dec: Decoder::new(),
            next: 0,
            au: Vec::new(),
            decoded: 0,
            queue: Vec::new(),
            queue_bytes: 0,
            dropped: 0,
            shown_count: 0,
            shown: None,
            fed_all: false,
        };
        v.prime();
        Ok(v)
    }

    /// Hand the decoder its parameter sets. In MP4 they live in `avcC`, so a
    /// decoder fed only the samples never sees them and every picture fails.
    fn prime(&mut self) {
        self.au.clear();
        mp4::parameter_sets(&self.track.codec, &mut self.au);
        if !self.au.is_empty() {
            // No picture in a parameter set; a refusal here is not an error.
            let _ = self.dec.decode(&self.au);
        }
    }

    fn nal_len(&self) -> usize {
        match &self.track.codec { mp4::Codec::Avc { nal_len, .. } => *nal_len, _ => 4 }
    }

    /// Decode one more sample into the queue. False when there are none left.
    fn feed_one(&mut self) -> bool {
        if self.next >= self.track.samples.len() {
            self.fed_all = true;
            return false;
        }
        let s = self.track.samples[self.next];
        self.next += 1;
        let pts = self.track.pts_ms(&s);
        let Some(bytes) = self.data.get(s.offset..s.offset + s.size) else {
            self.fed_all = true;
            return false;
        };
        self.au.clear();
        mp4::to_annex_b(bytes, self.nal_len(), &mut self.au);
        // A sample the decoder refuses is one lost picture, not a lost film:
        // the next sync sample starts it again. Silence here would hide a
        // broken file, so the caller gets to log it.
        self.decoded += 1;
        if let Ok(Some(f)) = self.dec.decode(&self.au) {
            // Insert sorted; the queue is four long, so this is cheaper than
            // keeping a heap and far cheaper than being wrong about order.
            let at = self.queue.partition_point(|(p, _)| *p <= pts);
            self.queue_bytes += f.y.len() + f.u.len() + f.v.len();
            self.queue.insert(at, (pts, f));
        }
        true
    }

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
    pub fn decode_step(&mut self, ms: i64, max_decodes: usize) {
        let mut budget = max_decodes;
        self.fill(ms, &mut budget);
    }

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
    pub fn take_due(&mut self, ms: i64) -> Option<&YuvFrame> {
        let mut advanced = false;
        while let Some(&(pts, _)) = self.queue.first() {
            if pts > ms { break; }
            // Only take it if the queue is deep enough to know it is the
            // earliest, or there is nothing left to come.
            if self.queue.len() < REORDER && !self.fed_all { break; }
            let (pts, f) = self.queue.remove(0);
            self.queue_bytes = self.queue_bytes
                .saturating_sub(f.y.len() + f.u.len() + f.v.len());
            if advanced { self.dropped += 1; }   // der vorige kam nie hin
            self.shown = Some((pts, f));
            advanced = true;
        }
        if advanced {
            self.shown_count += 1;
            self.shown.as_ref().map(|(_, f)| f)
        } else {
            None
        }
    }

    /// Decode ahead: deep enough to order pictures, then as far ahead of the
    /// clock as the lead allows, and never past the byte cap.
    /// Samples decoded since the last `take_decoded`. Der Rufer misst die
    /// ZEIT selbst; hier wird nur gezaehlt, was sie verursacht hat.
    pub fn take_decoded(&mut self) -> u32 {
        core::mem::take(&mut self.decoded)
    }

    fn fill(&mut self, ms: i64, budget: &mut usize) {
        // Die Reihenfolge-Tiefe ist eine Pflicht, kein Vorrat: ohne sie
        // weiss die Schlange nicht, welches Bild das naechste ist. Sie geht
        // deshalb VOR dem Budget und nicht aus ihm.
        while self.queue.len() < REORDER {
            if !self.feed_one() { break; }
        }
        while *budget > 0 {
            let lead = self.queue.last().map(|(p, _)| *p - ms).unwrap_or(0);
            if lead >= TARGET_LEAD_MS || self.queue_bytes >= MAX_QUEUE_BYTES { break; }
            *budget -= 1;
            if !self.feed_one() { break; }
        }
    }

    /// Liegt ein Bild bereit UND genug Vorrat, um es zu zeigen, ohne
    /// vorher zu dekodieren?
    pub fn show_before_decode(&self, ms: i64) -> bool {
        self.next_due_in(ms) == 0
            && self.lead_ms(ms) > SHOW_FIRST_FLOOR_MS
            && self.queue.len() > REORDER
    }

    /// Ist der Vorrat voll? Nur dann darf der Rufer schlafen.
    ///
    /// Die freie Kapazitaet steckt genau hier: dekodieren kostet am Geraet
    /// 22 ms je Bild und eine Bildperiode ist 33 ms, also LIESSEN sich 45
    /// Bilder je Sekunde dekodieren, wo 30 gebraucht werden. Wer in dieser
    /// Luecke schlaeft, verschenkt sie — und steht in der teuren Szene ohne
    /// Puffer da. Gemessen: mit festem Schlaf fiel der Vorlauf von 870 auf
    /// 150 ms und der Rueckstand stieg auf 734.
    pub fn stocked(&self, ms: i64) -> bool {
        self.lead_ms(ms) >= TARGET_LEAD_MS || self.queue_bytes >= MAX_QUEUE_BYTES
    }

    /// Wie lange bis zum naechsten faelligen Bild. Der Rufer schlaeft danach
    /// — ein fester Schlaf schiebt eine Runde ueber die Bildperiode, und
    /// dann faellt genau dort ein Bild aus.
    pub fn next_due_in(&self, ms: i64) -> i64 {
        self.queue.first().map(|(p, _)| p - ms).unwrap_or(0).max(0)
    }

    /// Enough decoded to start without stumbling.
    ///
    /// Playback used to begin the moment the file opened, with nothing in
    /// the queue — measured at the device: 207 ms behind with 26 ms of lead.
    /// The first second is the one stretch where the decoder has no head
    /// start at all, so it is the one stretch where waiting is free.
    pub fn primed(&self, ms: i64) -> bool {
        self.lead_ms(ms) >= PREROLL_MS || self.fed_all
    }

    /// Picture time the queue reaches beyond `ms`. Zero means the decoder is
    /// hand to mouth, and the next expensive scene will be visible.
    pub fn lead_ms(&self, ms: i64) -> i64 {
        self.queue.last().map(|(p, _)| p - ms).unwrap_or(0).max(0)
    }

    /// Das Bild auf dem Schirm, unabhaengig davon, ob es eben gewechselt
    /// hat — `frame_at` gibt den Borrow zurueck, und der Rufer braucht ihn
    /// noch einmal, nachdem er die Zeit dazwischen gemessen hat.
    pub fn current_frame(&self) -> Option<&YuvFrame> {
        self.shown.as_ref().map(|(_, f)| f)
    }

    /// Presentation time of the picture on screen. The gap to the caller's
    /// clock IS the lag, and it is the number that says whether the machine
    /// keeps up.
    pub fn shown_ms(&self) -> i64 {
        self.shown.as_ref().map(|(p, _)| *p).unwrap_or(0)
    }

    /// Everything fed and nothing left to show.
    pub fn ended(&self) -> bool {
        self.fed_all && self.queue.is_empty()
    }

    /// Restart at the last sync sample at or before `ms`, and answer the time
    /// it really landed on. A decoder started anywhere else produces garbage
    /// until the next sync sample, so this is a jump to a key frame and the
    /// caller must believe the number it gets back.
    pub fn seek(&mut self, ms: i64) -> i64 {
        let i = self.track.sync_at_ms(ms.max(0) as u64);
        self.dec = Decoder::new();
        self.queue.clear();
        self.queue_bytes = 0;
        self.shown = None;
        self.next = i;
        self.fed_all = false;
        self.prime();
        self.track.pts_ms(&self.track.samples[i])
    }

    /// Row strides for `npk_canvas_commit_yuv`. The decoder's planes are
    /// tight, but the host call takes strides because a decoder is allowed
    /// not to be — asking here keeps that promise in one place.
    pub fn strides(f: &YuvFrame) -> (usize, usize) {
        (f.width, f.width.div_ceil(2))
    }
}
