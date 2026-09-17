//! AAC aus einer MP4 — die Tonhaelfte des Containers.
//!
//! Der Dekoder liegt in `<repo>/tools/wasm/vendor/rusty_aac` (siehe dort
//! VENDOR.md); hier steht nur, was er nicht wissen will: wo die Rahmen in der
//! Datei liegen, wie lang der Strom ist, und wohin ein Sprung fuehrt.
//!
//! Ein AAC-Rahmen traegt 1024 Samples je Kanal und passt damit in
//! [`MAX_BLOCK_FRAMES`](crate::source::MAX_BLOCK_FRAMES) (1152).


use crate::mp4;
use crate::source::{Info, Source};

pub struct Aac {
    data: &'static [u8],
    track: mp4::Track,
    dec: rusty_aac::decode::Decoder,
    info: Info,
    /// Naechster Rahmen, den `next_block` dekodiert.
    next: usize,
    /// Ausgabeposition in Frames der Quellrate.
    frame: u64,
}

impl Aac {
    /// `track` muss die Tonspur eines bereits geparsten MP4 sein.
    pub fn from_track(data: &'static [u8], track: mp4::Track) -> Option<Aac> {
        let cfg_bytes = match &track.codec {
            mp4::Codec::Aac { config } => config.clone(),
            _ => return None,
        };
        let cfg = rusty_aac::parse_audio_specific_config(&cfg_bytes).ok()?;
        if cfg.sample_rate == 0 || cfg.channels == 0 { return None; }

        // Die Dauer kommt aus der Sampletabelle und nicht aus einer Schaetzung
        // — und sie zaehlt in der QUELLrate, weil der Rufer danach mit
        // `info().rate` weiterrechnet.
        let total = track.duration as u128 * cfg.sample_rate as u128
            / track.timescale.max(1) as u128;

        // Bitrate aus den wirklichen Bytes: Summe der Rahmen durch die Dauer.
        let bytes: u64 = track.samples.iter().map(|s| s.size as u64).sum();
        let ms = track.duration_ms().max(1);
        let kbps = (bytes * 8 / ms) as u32;

        Some(Aac {
            info: Info {
                rate: cfg.sample_rate,
                channels: cfg.channels.min(255) as u8,
                total_frames: total as u64,
                title: None,
                artist: None,
                kind: "AAC",
                bitrate_kbps: kbps,
            },
            dec: rusty_aac::decode::Decoder::new(cfg.sample_rate),
            data,
            track,
            next: 0,
            frame: 0,
        })
    }

    /// Wieviele Samples der Strom vor seinem ersten HOERBAREN ueberspringt.
    ///
    /// Ein AAC-Encoder beginnt mit Vorlaufsamples, die nicht zum Ton
    /// gehoeren, und die Edit-List des Containers sagt, wieviele. Gemessen an
    /// einem Handyvideo: ffmpeg schneidet **genau** `edit_start` = 2112
    /// Samples weg, und unser Dekoder stimmt danach bis auf 1 LSB.
    ///
    /// Das ist keine Kosmetik: die Videospur derselben Datei hatte 0. Wer
    /// beide Zeitachsen roh spielt, hat Ton und Bild 48 ms auseinander.
    pub fn priming(&self) -> u64 {
        self.track.edit_start
    }
}

impl Source for Aac {
    fn info(&self) -> &Info { &self.info }

    fn next_block(&mut self, out: &mut [f32]) -> usize {
        let ch = self.info.channels.max(1) as usize;
        while self.next < self.track.samples.len() {
            let s = self.track.samples[self.next];
            self.next += 1;
            let Some(au) = self.data.get(s.offset..s.offset + s.size) else { break };
            // Ein abgelehnter Rahmen ist ein Loch und kein Ende: der naechste
            // faengt sich wieder, weil jeder AAC-Rahmen fuer sich steht.
            let Ok(d) = self.dec.decode(au, None) else { continue };
            let frames = d.frames().min(out.len() / ch);
            if frames == 0 { continue; }
            out[..frames * ch].copy_from_slice(&d.samples[..frames * ch]);
            self.frame += frames as u64;
            return frames;
        }
        0
    }

    fn seek(&mut self, frame: u64) -> u64 {
        // Rahmen suchen, dessen Zeitspanne `frame` enthaelt. Der Dekoder
        // faengt bei AAC-LC an jedem Rahmen an; der erste danach klingt
        // weich, weil ihm die Ueberlappung des Vorgaengers fehlt.
        let rate = self.info.rate.max(1) as u128;
        let ts = self.track.timescale.max(1) as u128;
        let want = frame as u128 * ts / rate;
        let mut idx = 0usize;
        for (i, s) in self.track.samples.iter().enumerate() {
            if s.dts as u128 > want { break; }
            idx = i;
        }
        self.dec = rusty_aac::decode::Decoder::new(self.info.rate);
        self.next = idx;
        let landed = self.track.samples.get(idx).map(|s| s.dts).unwrap_or(0) as u128;
        self.frame = (landed * rate / ts) as u64;
        self.frame
    }
}
