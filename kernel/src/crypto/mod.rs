//! Cryptography engine
//!
//! AEAD encryption, TLS 1.3, signing keys.

pub mod aead;
pub mod tls;
pub mod update_key;

// Re-export aead contents at crypto:: level for backward compat
// (callers use crate::crypto::derive_master_key, etc.)
pub use aead::*;
