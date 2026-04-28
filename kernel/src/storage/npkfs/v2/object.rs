//! npkFS v2 object format — content-addressed Blob/Tree objects.
//!
//! Every object is encoded with postcard, hashed with BLAKE3 over the
//! encoded bytes, and stored under that 32-byte hash in the v2 B-tree.
//! The hash IS the address; equal content produces equal addresses by
//! construction.
//!
//! Wire stability:
//!   - Enum variants append-only, never reordered, never removed without
//!     a wire-version bump
//!   - Reserved variants hold their slot via #[allow(dead_code)]
//!   - postcard variant tags are u32 varints — order matters
//!
//! Step-1 scope: in-memory only. No disk, no path walker, no B-tree.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;

/// Maximum bytes for a single name component (one path segment).
/// Names are UTF-8, may not contain `/` or NUL, and may not be empty.
pub const MAX_NAME_LEN: usize = 255;

/// What a `TreeEntry` references.
///
/// Append-only. Reserve slots in tag order; the postcard tag is the
/// variant index.
#[repr(u8)]
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EntryKind {
    File = 0,
    Dir  = 1,
    // Symlink = 2  (reserved)
    // Device  = 3  (reserved)
}

/// One row in a directory listing.
///
/// `size` semantics:
///   - `File`: byte size of the referenced Blob
///   - `Dir` : recursive size of the subtree (sum of File sizes)
///
/// `flags` is reserved for future per-entry metadata (timestamps,
/// permission bits). Zero in v2.0.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TreeEntry {
    pub name:  String,
    pub hash:  [u8; 32],
    pub kind:  EntryKind,
    pub size:  u64,
    pub flags: u8,
}

/// Top-level addressable object.
///
/// Postcard-encodes as a u32 varint discriminant followed by the
/// variant payload, so `Blob(b"")` and `Tree(vec![])` hash to
/// different values even though both are "empty".
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Object {
    Blob(Vec<u8>),
    Tree(Vec<TreeEntry>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectError {
    /// postcard refused to serialize (unreachable for our types in alloc mode,
    /// kept as a defense-in-depth boundary).
    Encode,
    /// Wire bytes were truncated, malformed, or contained an unknown tag.
    Decode,
    /// A `TreeEntry::name` violated the naming rules.
    InvalidName,
}

impl TreeEntry {
    /// Validate the name component. Names must be 1..=MAX_NAME_LEN bytes,
    /// UTF-8 (guaranteed by `String`), and contain neither `/` nor NUL.
    pub fn validate_name(&self) -> Result<(), ObjectError> {
        validate_name(&self.name)
    }
}

fn validate_name(name: &str) -> Result<(), ObjectError> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(ObjectError::InvalidName);
    }
    if name.bytes().any(|b| b == 0 || b == b'/') {
        return Err(ObjectError::InvalidName);
    }
    Ok(())
}

impl Object {
    /// Construct a `Tree` from entries, sorted lexicographically by name.
    /// Sorting makes the resulting hash independent of input order.
    /// Caller is responsible for not passing duplicate names — duplicates
    /// pass through as-is and will hash deterministically by sorted order.
    pub fn tree_sorted(mut entries: Vec<TreeEntry>) -> Result<Self, ObjectError> {
        for e in &entries {
            e.validate_name()?;
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Object::Tree(entries))
    }

    /// Encode to postcard wire bytes.
    pub fn encode(&self) -> Result<Vec<u8>, ObjectError> {
        postcard::to_allocvec(self).map_err(|_| ObjectError::Encode)
    }

    /// Decode from postcard wire bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, ObjectError> {
        postcard::from_bytes(bytes).map_err(|_| ObjectError::Decode)
    }

    /// BLAKE3-hash the encoded form. This is the object's content address.
    pub fn hash(&self) -> Result<[u8; 32], ObjectError> {
        let bytes = self.encode()?;
        Ok(*blake3::hash(&bytes).as_bytes())
    }

    /// Encode + hash in one shot. Returns (encoded_bytes, hash). Avoids
    /// re-encoding when the caller wants both (typical write path).
    pub fn encode_and_hash(&self) -> Result<(Vec<u8>, [u8; 32]), ObjectError> {
        let bytes = self.encode()?;
        let h = *blake3::hash(&bytes).as_bytes();
        Ok((bytes, h))
    }
}
