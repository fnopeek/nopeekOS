//! Cryptography engine
//!
//! AEAD encryption, TLS 1.3, signing keys.

pub mod aead;
pub mod aead_hw;
pub mod tls;
pub mod update_key;

// Re-export aead contents at crypto:: level for backward compat
// (callers use crate::crypto::derive_master_key, etc.)
pub use aead::*;
pub use aead_hw::{aead_encrypt_aes_hw, aead_decrypt_aes_hw_in_place};
