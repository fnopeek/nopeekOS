//! Source of the bytes behind `crypto.getRandomValues`.
//!
//! The engine has no host functions of its own; the host lends one, as with
//! the clock (`Interp::epoch_ms`). In beak this is the kernel CSPRNG via
//! `npk_random_bytes`.
//!
//! Without a source there is no randomness at all. Falling back to
//! `Math.random` would be worse than a missing API: pages derive session
//! tokens from `getRandomValues`, and a predictable token is invisible.

/// The host's source. `None` means there is none and `crypto` is not exposed.
static mut SOURCE: Option<fn(&mut [u8]) -> bool> = None;

/// Install the host's source. Called once at startup.
pub fn set_source(f: fn(&mut [u8]) -> bool) {
    // SAFETY: the engine is single-threaded; SOURCE is written once at
    // startup before any reader runs.
    unsafe { core::ptr::addr_of_mut!(SOURCE).write(Some(f)) };
}

/// Whether a source exists; decides whether `crypto` is on the global
/// object, so that `if (window.crypto)` tells the truth.
pub fn available() -> bool {
    // SAFETY: single-threaded engine; plain read of a `Copy` static.
    unsafe { core::ptr::addr_of!(SOURCE).read().is_some() }
}

/// Fill the buffer. `false` if there is no source or the host refused; the
/// caller then throws instead of returning weak randomness.
pub fn fill(out: &mut [u8]) -> bool {
    // SAFETY: single-threaded engine; plain read of a `Copy` static.
    let Some(f) = (unsafe { core::ptr::addr_of!(SOURCE).read() }) else { return false };
    f(out)
}
