# `kernel/src/storage/npkfs/object.rs` @ 5e0102684

## L1-14 · `#![allow(dead_code)]`

```
//! npkFS object format — content-addressed Blob/Tree objects.
//!
//! Every object is encoded with postcard, hashed with BLAKE3 over the
//! encoded bytes, and stored under that 32-byte hash in the B-tree.
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
```

## L21-22 · `pub const MAX_NAME_LEN: usize = 255;`

```
/// Maximum bytes for a single name component (one path segment).
/// Names are UTF-8, may not contain `/` or NUL, and may not be empty.
```

## L25-28 · `#[repr(u8)]`

```
/// What a `TreeEntry` references.
///
/// Append-only. Reserve slots in tag order; the postcard tag is the
/// variant index.
```

## L35-36 · `}`

```
// Symlink = 2  (reserved)
// Device  = 3  (reserved)
```

## L39-60 · `#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]`

```
/// One row in a directory listing.
///
/// `size` semantics:
///   - `File`: byte size of the referenced Blob
///   - `Dir` : recursive size of the subtree (sum of File sizes)
///
/// `mtime` is UTC seconds since the Unix epoch, captured at write
/// time from the host RTC. Zero means "unknown" — the RTC was not
/// readable when this entry was created (early boot, hardware
/// quirk, …). Field added with the mtime schema; older trees are not
/// readable by this kernel (mount halts with a reinstall message).
///
/// `flags` is a u8 bitmap of per-entry metadata. For `File` entries the
/// low two bits record the object shape so GC can decide reachability
/// WITHOUT reading the object (see `FLAG_BLOB` / `FLAG_CHUNKED`):
///   - `FLAG_BLOB`    (0x02): single `Object::Blob` — a leaf, GC skips it.
///   - `FLAG_CHUNKED` (0x01): `Object::Chunked` manifest — GC reads it to
///                            reach the chunk blobs.
/// `flags == 0` on a `File` is the legacy/unknown case (entries written
/// before the flag existed): GC falls back to reading the object to
/// classify it, so a pre-flag chunked file's chunks are never mistaken
/// for orphans. `Dir` entries leave `flags` unused (GC keys on `kind`).
```

## L71-89 · `#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]`

```
/// Top-level addressable object.
///
/// Postcard-encodes as a u32 varint discriminant followed by the
/// variant payload, so `Blob(b"")` and `Tree(vec![])` hash to
/// different values even though both are "empty".
///
/// Variant order is **append-only**. The postcard tag is the
/// variant index, so reordering or removing variants breaks every
/// on-disk object.
///
/// `Chunked` references a sequence of regular `Blob` chunk objects
/// by their content addresses. Used when a file is too large to
/// materialize in a single `Vec<u8>` during write — the path layer
/// can stream the source into N fixed-size chunks (default 16 MB),
/// store each as its own `Blob`, then emit a `Chunked` manifest
/// pointing at all of them. On read, `fs::read` stitches the
/// chunks back into one `Vec<u8>` for consumer transparency; for
/// the multi-GB case a future streaming reader can iterate
/// chunk-by-chunk against the same on-disk layout.
```

## L102-103 · `Encode,`

```
/// postcard refused to serialize (unreachable for our types in alloc mode,
/// kept as a defense-in-depth boundary).
```

## L105 · `Decode,`

```
/// Wire bytes were truncated, malformed, or contained an unknown tag.
```

## L107 · `InvalidName,`

```
/// A `TreeEntry::name` violated the naming rules.
```

## L112-113 · `pub const FLAG_CHUNKED: u8 = 0x01;`

```
/// `flags` bit: this `File` references an `Object::Chunked` manifest.
/// GC must read it to reach the chunk blobs.
```

## L115-116 · `pub const FLAG_BLOB: u8 = 0x02;`

```
/// `flags` bit: this `File` references a single `Object::Blob` (a
/// leaf). GC marks it reachable without reading the body.
```

## L119-120 · `pub fn validate_name(&self) -> Result<(), ObjectError> {`

```
/// Validate the name component. Names must be 1..=MAX_NAME_LEN bytes,
/// UTF-8 (guaranteed by `String`), and contain neither `/` nor NUL.
```

## L137-140 · `pub fn tree_sorted(mut entries: Vec<TreeEntry>) -> Result<Self, ObjectError> {`

```
/// Construct a `Tree` from entries, sorted lexicographically by name.
/// Sorting makes the resulting hash independent of input order.
/// Caller is responsible for not passing duplicate names — duplicates
/// pass through as-is and will hash deterministically by sorted order.
```

## L149 · `pub fn encode(&self) -> Result<Vec<u8>, ObjectError> {`

```
/// Encode to postcard wire bytes.
```

## L154 · `pub fn decode(bytes: &[u8]) -> Result<Self, ObjectError> {`

```
/// Decode from postcard wire bytes.
```

## L159 · `pub fn hash(&self) -> Result<[u8; 32], ObjectError> {`

```
/// BLAKE3-hash the encoded form. This is the object's content address.
```

## L165-166 · `pub fn encode_and_hash(&self) -> Result<(Vec<u8>, [u8; 32]), ObjectError> {`

```
/// Encode + hash in one shot. Returns (encoded_bytes, hash). Avoids
/// re-encoding when the caller wants both (typical write path).
```

## L174-185 · `pub fn blob_content_hash(data: &[u8]) -> [u8; 32] {`

```
/// Compute the content-address (BLAKE3 hash) that `Object::Blob(data)`
/// would produce, **without** allocating the encoded form. Streams
/// the postcard wire bytes into a `blake3::Hasher`:
///
///   `update([variant_tag = 0])`
///   `update(varint(data.len()))`
///   `update(data)`
///
/// Used by the path layer to skip `data.to_vec() + encode_and_hash()`
/// (~1 ms / MB on the test rig) when the blob already exists in
/// storage. If the hash misses, callers fall back to the full encode
/// path; if it hits, we go straight to tree-rebuild.
```

## L188 · `hasher.update(&[0u8]); // postcard variant tag for Object::Blob`

```
// postcard variant tag for `Object::Blob`
```

## L197-198 · `let mut i = 0;`

```
// postcard uses LEB128: 7 bits payload per byte, high bit set
// means "more bytes follow".
```

## L209-229 · `pub fn decode_blob_inplace(mut bytes: Vec<u8>) -> Result<Vec<u8>, ObjectError> {`

```
/// Zero-copy-ish Blob decoder. Takes ownership of the encoded `Object`
/// bytes (typical: AES-GCM-decrypted plaintext from `storage::get`)
/// and returns the inner blob `Vec<u8>` by shifting the postcard
/// prefix off the front in-place.
///
/// `Object::decode` allocates a fresh `Vec<u8>` and copies — for a
/// 1 MB blob that's ~1 ms wasted (measured 920 µs in the testdisk
/// profile). `drain(0..prefix)` is a single memmove of the same
/// payload by ~6 bytes, ~10× faster (~0.1 ms / MB). The wire format
/// is unchanged: postcard for `Object::Blob(Vec<u8>)` lays down
///
///   [variant_tag = 0u8 (1 byte)]
///   [length      = u32 varint (1–5 bytes)]
///   [content     = N bytes]
///
/// We parse just enough to find `prefix = 1 + varint_size`, drain
/// it, truncate to the declared length, return what's left.
///
/// Errors: returns `Decode` if the buffer is empty, the variant tag
/// isn't 0 (Blob), the varint is malformed, or the declared length
/// exceeds the buffer.
```

## L232-233 · `if bytes[0] != 0 { return Err(ObjectError::Decode); }`

```
// postcard u32 varint: variant index for `Object::Blob` is 0,
// which encodes as a single 0x00 byte.
```

## L236-238 · `let mut len: u64 = 0;`

```
// Varint length of the inner Vec<u8>. postcard uses LEB128:
// each byte carries 7 bits of payload, MSB=1 means "more bytes
// follow". u32 fits in 5 bytes max.
```

