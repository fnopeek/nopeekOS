//! Der Containerschnitt: EINE Datei, zwei Stroeme.
//!
//! Bis hierher kannte tune nur [`Source`](crate::source::Source) — einen
//! Strom von Tonbloecken. Ein Film hat zwei, und sie muessen aus DERSELBEN
//! Lesung des Containers kommen: zwei Parser auf einer Datei sind zwei
//! Gelegenheiten, verschiedener Meinung zu sein.
//!
//! **Der Weg fuer Ton allein wird dadurch nicht laenger.** Eine MP3 ist ein
//! Demux mit einem Strom; `open` gibt sie unveraendert an `source::open`
//! weiter, und kein Byte geht durch neuen Code.

use alloc::boxed::Box;

use crate::aac::Aac;
use crate::mp4;
use crate::source::{self, Source};
use crate::video::{OpenError, Video};

pub struct Demux {
    pub audio: Option<Box<dyn Source>>,
    pub video: Option<Video>,
    /// Vorlaufsamples der TONspur, in deren eigener Rate — was vor dem
    /// ersten hoerbaren Sample liegt. Steht hier und nicht hinter dem
    /// `Source`-Vertrag, weil es eine Eigenschaft des CONTAINERS ist und
    /// nicht des Dekoders: dieselbe Datei hat auf Video und Ton
    /// verschiedene Werte (gemessen: Video 0, Ton 2112).
    pub audio_priming: u64,
    /// Warum kein Bild da ist, wenn die Datei eigentlich eines haette.
    /// `None` heisst „war nie eine Frage" — eine MP3 hat kein Bild und
    /// schuldet dafuer keine Erklaerung.
    pub video_error: Option<OpenError>,
}

/// Nach INHALT entscheiden, nicht nach Endung — dieselbe Regel, nach der
/// `source::open` seit je den Tondekoder waehlt.
pub fn open(bytes: &'static [u8]) -> Demux {
    if !mp4::looks_like(bytes) {
        return Demux { audio: source::open(bytes), video: None, audio_priming: 0, video_error: None };
    }

    let Some(m) = mp4::parse(bytes) else {
        return Demux { audio: None, video: None, audio_priming: 0, video_error: Some(OpenError::NotMp4) };
    };
    if m.fragmented {
        // Die Sampletabellen stehen in den Fragmenten, nicht im `moov`. Das
        // zu sagen ist eine andere Auskunft als „geht nicht", und nur eine
        // davon ist unser Fehler.
        return Demux { audio: None, video: None, audio_priming: 0, video_error: Some(OpenError::Fragmented) };
    }

    let video = match m.video {
        Some(t) => match Video::from_track(bytes, t) {
            Ok(v) => Ok(v),
            Err(e) => Err(e),
        },
        None => Err(OpenError::NoVideo),
    };
    let a = m.audio.and_then(|t| Aac::from_track(bytes, t));
    let audio_priming = a.as_ref().map(|a| a.priming()).unwrap_or(0);
    let audio = a.map(|a| Box::new(a) as Box<dyn Source>);

    match video {
        Ok(v) => Demux { audio, video: Some(v), audio_priming, video_error: None },
        Err(e) => Demux { audio, video: None, audio_priming, video_error: Some(e) },
    }
}
