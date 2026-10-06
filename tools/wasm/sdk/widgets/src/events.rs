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

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn npk_event_poll(ptr: i32, max: i32) -> i32;
}

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
    // SAFETY: the host writes at most `buf.len()` bytes into `buf`.
    let n = unsafe { npk_event_poll(buf.as_mut_ptr() as i32, buf.len() as i32) };
    match n {
        0 => Poll::Empty,
        -1 => Poll::Gone,
        n if n > 0 => decode(&buf[..n as usize]),
        n => {
            let needed = (-(n as i64) - 2) as usize;
            let mut big = vec![0u8; needed];
            // SAFETY: as above, for `big`.
            let m = unsafe { npk_event_poll(big.as_mut_ptr() as i32, big.len() as i32) };
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
