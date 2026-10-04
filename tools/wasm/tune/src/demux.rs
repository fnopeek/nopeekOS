//! The container seam: one file, two streams.
//!
//! A film has an audio and a video stream, and both must come from the same
//! read of the container — two parsers on one file are two chances to
//! disagree.
//!
//! Audio alone takes no longer path: an MP3 is a demux with one stream, and
//! `open` passes it to `source::open` unchanged.

use alloc::boxed::Box;

use crate::aac::Aac;
use crate::mp4;
use crate::source::{self, Source};
use crate::video::{OpenError, Video};

pub struct Demux {
    pub audio: Option<Box<dyn Source>>,
    pub video: Option<Video>,
    /// Priming samples of the audio track, at its own rate — what precedes the
    /// first audible sample. Kept here rather than behind the `Source`
    /// contract because it is a property of the container, not the decoder:
    /// the same file has different values on video and audio.
    pub audio_priming: u64,
    /// Why there is no picture although the file would have one. `None`
    /// means the question never arose — an MP3 has no picture and owes no
    /// explanation.
    pub video_error: Option<OpenError>,
}

/// Decide by content, not by extension — the same rule `source::open`
/// uses to pick the audio decoder.
pub fn open(bytes: &'static [u8]) -> Demux {
    if !mp4::looks_like(bytes) {
        return Demux { audio: source::open(bytes), video: None, audio_priming: 0, video_error: None };
    }

    let Some(m) = mp4::parse(bytes) else {
        return Demux { audio: None, video: None, audio_priming: 0, video_error: Some(OpenError::NotMp4) };
    };
    if m.fragmented {
        // The sample tables live in the fragments, not in `moov`. That is a
        // different answer from "cannot play", and only one of the two is our
        // shortcoming.
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
