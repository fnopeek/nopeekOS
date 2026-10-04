# `tools/wasm/sdk/widgets/src/wire.rs` @ 5e0102684

## L1-12 · `use alloc::vec::Vec;`

```
//! Wire format — postcard with a leading `WIRE_VERSION` byte.
//!
//! Every `npk_scene_commit` payload is framed as:
//!
//! `​``text
//! [ version: u8 ][ postcard-serialized Widget tree ]
//! `​``
//!
//! The compositor reads the version byte first; unknown versions are
//! rejected before deserialization is attempted. This lets us bump the
//! wire shape later without the kernel having to speculatively parse
//! unknown payloads.
```

## L18-19 · `pub const WIRE_VERSION: u8 = 0x01;`

```
/// Wire protocol version byte. Must match
/// `kernel/src/shade/widgets/abi.rs::WIRE_VERSION`.
```

## L22 · `#[derive(Debug, PartialEq, Eq)]`

```
/// Errors that may arise when encoding/decoding a wire payload.
```

## L25 · `Empty,`

```
/// Payload too short to even contain the version byte.
```

## L27 · `VersionMismatch { got: u8, want: u8 },`

```
/// Version byte did not match [`WIRE_VERSION`].
```

## L29 · `Postcard,`

```
/// Postcard deserialization failed (truncated or malformed).
```

## L31 · `Serialize,`

```
/// Serialization failed (allocation or internal).
```

## L35-36 · `pub fn encode(tree: &Widget) -> Result<Vec<u8>, WireError> {`

```
/// Encode a widget tree into a wire buffer: version byte + postcard
/// body. Returns the buffer ready for `npk_scene_commit`.
```

## L45-47 · `pub fn decode(bytes: &[u8]) -> Result<Widget, WireError> {`

```
/// Decode a wire buffer back into a widget tree. Verifies the version
/// byte first. Exposed mainly for unit-test round-tripping; the kernel
/// has its own deserializer (P10.2).
```

