//! Wire format — postcard with a leading `WIRE_VERSION` byte.
//!
//! Every `npk_scene_commit` payload is framed as:
//!
//! ```text
//! [ version: u8 ][ postcard-serialized Widget tree ]
//! ```
//!
//! The compositor reads the version byte first; unknown versions are
//! rejected before deserialization is attempted. This lets us bump the
//! wire shape later without the kernel having to speculatively parse
//! unknown payloads.

use alloc::vec::Vec;

use crate::abi::Widget;

/// Wire protocol version byte. Must match
/// `kernel/src/shade/widgets/abi.rs::WIRE_VERSION`.
pub const WIRE_VERSION: u8 = 0x01;

/// Errors that may arise when encoding/decoding a wire payload.
#[derive(Debug, PartialEq, Eq)]
pub enum WireError {
    /// Payload too short to even contain the version byte.
    Empty,
    /// Version byte did not match [`WIRE_VERSION`].
    VersionMismatch { got: u8, want: u8 },
    /// Postcard deserialization failed (truncated or malformed).
    Postcard,
    /// Serialization failed (allocation or internal).
    Serialize,
}

/// Encode a widget tree into a wire buffer: version byte + postcard
/// body. Returns the buffer ready for `npk_scene_commit`.
pub fn encode(tree: &Widget) -> Result<Vec<u8>, WireError> {
    let body = postcard::to_allocvec(tree).map_err(|_| WireError::Serialize)?;
    let mut out = Vec::with_capacity(body.len() + 1);
    out.push(WIRE_VERSION);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a wire buffer back into a widget tree. Verifies the version
/// byte first. Exposed mainly for unit-test round-tripping; the kernel
/// has its own deserializer (P10.2).
pub fn decode(bytes: &[u8]) -> Result<Widget, WireError> {
    let (&ver, body) = bytes.split_first().ok_or(WireError::Empty)?;
    if ver != WIRE_VERSION {
        return Err(WireError::VersionMismatch { got: ver, want: WIRE_VERSION });
    }
    postcard::from_bytes(body).map_err(|_| WireError::Postcard)
}
