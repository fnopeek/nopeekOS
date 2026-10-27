//! The container seam: one file, two streams.
//!
//! A film has an audio and a video stream, and both must come from the same
//! read of the container — two parsers on one file are two chances to
//! disagree.
//!
//! A film is read range by range ([`Media`]) and never held whole. Audio
//! alone takes no longer path: an MP3 is a demux with one stream, and
//! `open` passes its bytes to `source::open` unchanged.

use alloc::boxed::Box;

use crate::aac::Aac;
use crate::media::Media;
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

/// An audio file held whole.
pub fn open(bytes: &'static [u8]) -> Demux {
    Demux { audio: source::open(bytes), video: None, audio_priming: 0, video_error: None }
}

/// The MP4 at `path`, read range by range; `None` if the file is no MP4 (or
/// cannot be opened), so the caller reads it whole as audio. Decided by
/// content, not by extension — the same rule `source::open` uses.
pub fn open_mp4(path: &str) -> Option<Demux> {
    let mut m = Media::open(path)?;
    let mut head = [0u8; 8];
    if !m.read_exact_at(0, &mut head) || !mp4::looks_like(&head) {
        return None;
    }

    let Some(parsed) = mp4::parse(&mut m) else {
        return Some(Demux { audio: None, video: None, audio_priming: 0, video_error: Some(OpenError::NotMp4) });
    };
    if parsed.fragmented {
        // The sample tables live in the fragments, not in `moov`. That is a
        // different answer from "cannot play", and only one of the two is our
        // shortcoming.
        return Some(Demux { audio: None, video: None, audio_priming: 0, video_error: Some(OpenError::Fragmented) });
    }

    // The video stream keeps the handle the container was read through; the
    // audio stream reads through its own.
    let video = match parsed.video {
        Some(t) => Video::from_track(m, t),
        None => Err(OpenError::NoVideo),
    };
    let a = parsed.audio.and_then(|t| Aac::from_track(Media::open(path)?, t));
    let audio_priming = a.as_ref().map(|a| a.priming()).unwrap_or(0);
    let audio = a.map(|a| Box::new(a) as Box<dyn Source>);

    Some(match video {
        Ok(v) => Demux { audio, video: Some(v), audio_priming, video_error: None },
        Err(e) => Demux { audio, video: None, audio_priming, video_error: Some(e) },
    })
}
