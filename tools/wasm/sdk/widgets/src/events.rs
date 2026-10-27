//! Polling widget events (`npk_event_poll`).
//!
//! The host answers with the event's length, 0 for an empty queue, -1 when
//! the window is gone, and `-(needed + 2)` when the buffer is too small; the
//! event then stays queued. An app passes its usual buffer; only an event
//! that does not fit gets a buffer of its own size, for this one read. That
//! works with a bump heap that is reset every frame as well as with a
//! growing one.

use crate::abi::Event;
use npk_sys::{fill, Fill};

/// What one poll found.
pub enum Poll {
    Event(Event),
    Empty,
    /// The window was closed; the app should leave its loop.
    Gone,
}

/// Take the next event, decoding it from `buf` or, if it is larger, from a
/// buffer of its size.
pub fn poll(buf: &mut [u8]) -> Poll {
    match fill(npk_sys::event_poll(buf)) {
        Fill::Len(0) => Poll::Empty,
        Fill::Len(n) => decode(&buf[..n]),
        Fill::Fail => Poll::Gone,
        Fill::Need(n) => match npk_sys::read_all_from(n, npk_sys::event_poll) {
            Some(big) if big.is_empty() => Poll::Empty,
            Some(big) => decode(&big),
            None => Poll::Gone,
        },
    }
}

fn decode(bytes: &[u8]) -> Poll {
    match postcard::from_bytes::<Event>(bytes) {
        Ok(ev) => Poll::Event(ev),
        Err(_) => Poll::Empty,
    }
}
