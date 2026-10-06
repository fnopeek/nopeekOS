//! Polling widget events (`npk_event_poll`).
//!
//! The host answers with the event's length, 0 for an empty queue, -1 when
//! the window is gone, and `-(needed + 2)` when the buffer is too small; the
//! event then stays queued. An app passes its usual buffer; only an event
//! that does not fit gets a buffer of its own size, for this one read. That
//! works with a bump heap that is reset every frame as well as with a
//! growing one.

use crate::abi::Event;
use alloc::vec;

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
    let n = npk_sys::event_poll(buf);
    match n {
        0 => Poll::Empty,
        -1 => Poll::Gone,
        n if n > 0 => decode(&buf[..n as usize]),
        n => {
            let needed = (-(n as i64) - 2) as usize;
            let mut big = vec![0u8; needed];
            let m = npk_sys::event_poll(&mut big);
            match m {
                m if m > 0 => decode(&big[..m as usize]),
                -1 => Poll::Gone,
                _ => Poll::Empty,
            }
        }
    }
}

fn decode(bytes: &[u8]) -> Poll {
    match postcard::from_bytes::<Event>(bytes) {
        Ok(ev) => Poll::Event(ev),
        Err(_) => Poll::Empty,
    }
}
