# `kernel/src/crypto/update_key.rs` @ 5e0102684

## L1-5 · `pub const UPDATE_PUB_KEY: [u8; 97] = [`

```
//! Embedded ECDSA P-384 public key for OTA update verification.
//!
//! This key is used to verify kernel update signatures.
//! The corresponding private key is kept offline by the maintainer.
//! Private key: update.key (NEVER commit this!)
```

## L7 · `pub const UPDATE_PUB_KEY: [u8; 97] = [`

```
/// P-384 uncompressed public key (97 bytes: 0x04 || x || y).
```

## L9 · `0x04, // uncompressed point prefix`

```
// uncompressed point prefix
```

## L10 · `0x59, 0xb6, 0xfc, 0x08, 0x4f, 0xba, 0x28, 0x88,`

```
// x-coordinate (48 bytes)
```

## L17 · `0xd8, 0x76, 0xee, 0xf3, 0xbf, 0xfc, 0x60, 0xf6,`

```
// y-coordinate (48 bytes)
```

