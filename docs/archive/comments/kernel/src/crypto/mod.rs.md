# `kernel/src/crypto/mod.rs` @ 5e0102684

## L1-3 · `pub mod aead;`

```
//! Cryptography engine
//!
//! AEAD encryption, TLS 1.3, signing keys.
```

## L11-12 · `pub use aead::*;`

```
// Re-export aead contents at crypto:: level for backward compat
// (callers use crate::crypto::derive_master_key, etc.)
```

