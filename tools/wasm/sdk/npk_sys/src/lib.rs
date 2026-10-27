//! The nopeekOS host ABI for WASM modules, made memory-safe.
//!
//! Every `npk_*` import a module shares with others is declared here once.
//! Each function takes slices where the import takes a pointer and a
//! length, and returns what the kernel returns, unchanged: a module keeps
//! its own reading of the result codes. The kernel checks every range
//! against the instance's linear memory, and no host call re-enters the
//! guest, so the only `unsafe` left is the FFI call itself, here.
//!
//! A call that writes one complete object (a file, a list, a string)
//! answers a buffer too small for it with `-(needed + 2)` and writes
//! nothing; -1 is an error or a refusal. [`fill`] reads such an answer,
//! and [`read_all`] (feature `alloc`) grows the buffer and asks again.
//!
//! No allocator is needed; drivers and small tools use this directly, the
//! widget SDK builds on it.

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod bump;
pub mod cell;

#[cfg(target_arch = "wasm32")]
mod calls;
#[cfg(target_arch = "wasm32")]
pub use calls::*;

/// The answer of a host call that writes one complete object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Written, this many bytes.
    Len(usize),
    /// The buffer is too small; the object needs this many bytes. Nothing
    /// was written, and a queued item stays queued.
    Need(usize),
    /// Refused or failed.
    Fail,
}

/// Reads a host call's return as a [`Fill`].
pub fn fill(ret: i32) -> Fill {
    match ret {
        n if n >= 0 => Fill::Len(n as usize),
        -1 => Fill::Fail,
        n => Fill::Need((-(n as i64) - 2) as usize),
    }
}

/// First buffer [`read_all`] offers: most answers fit, a larger one costs a
/// second call.
#[cfg(feature = "alloc")]
const FIRST_READ: usize = 1024;

/// Calls `call` with a buffer and, while the host answers that it is too
/// small, again with one of the needed size. The bytes, or `None` when the
/// host refused or failed.
#[cfg(feature = "alloc")]
pub fn read_all(call: impl FnMut(&mut [u8]) -> i32) -> Option<alloc::vec::Vec<u8>> {
    read_all_from(FIRST_READ, call)
}

/// [`read_all`] starting with a buffer of `first` bytes, for a caller that
/// knows the size (a file from its listing).
#[cfg(feature = "alloc")]
pub fn read_all_from(first: usize, mut call: impl FnMut(&mut [u8]) -> i32) -> Option<alloc::vec::Vec<u8>> {
    let mut buf = alloc::vec![0u8; first];
    loop {
        match fill(call(&mut buf)) {
            Fill::Len(n) => {
                buf.truncate(n);
                return Some(buf);
            }
            // The object may have grown between the two calls; then the
            // host names the new size and the loop asks once more.
            Fill::Need(n) if n > buf.len() => buf.resize(n, 0),
            Fill::Need(_) | Fill::Fail => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers() {
        assert_eq!(fill(0), Fill::Len(0));
        assert_eq!(fill(17), Fill::Len(17));
        assert_eq!(fill(-1), Fill::Fail);
        assert_eq!(fill(-2), Fill::Need(0));
        assert_eq!(fill(-(4096 + 2)), Fill::Need(4096));
        assert_eq!(fill(i32::MIN + 1), Fill::Need(i32::MAX as usize - 2));
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn grows_to_the_needed_size() {
        let obj = [7u8; 5000];
        let mut calls = 0;
        let got = read_all(|buf| {
            calls += 1;
            if buf.len() < obj.len() { return -(obj.len() as i32 + 2); }
            buf[..obj.len()].copy_from_slice(&obj);
            obj.len() as i32
        });
        assert_eq!(got.as_deref(), Some(&obj[..]));
        assert_eq!(calls, 2);
        assert_eq!(read_all(|_| -1), None);
        // A host that names a size the buffer already has is broken; no loop.
        assert_eq!(read_all(|_| -(10 + 2)), None);
    }
}
