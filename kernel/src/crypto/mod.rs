//! Cryptography engine
//!
//! AEAD encryption, TLS 1.3, signing keys.

pub mod aead;
/// AES-GCM by hand on AES-NI and PCLMULQDQ; elsewhere the RustCrypto path
/// in `aead` uses the CPU's crypto instructions itself.
#[cfg(target_arch = "x86_64")]
pub mod aead_hw;
#[cfg(target_arch = "x86_64")]
pub mod aead_hw_ghash;
pub mod keyslot;
pub mod tls;
pub mod update_key;

// Re-export aead contents at crypto:: level for backward compat
// (callers use crate::crypto::set_disk_key, etc.)
pub use aead::*;
#[cfg(target_arch = "x86_64")]
pub use aead_hw::{aead_encrypt_aes_hw, aead_decrypt_aes_hw_in_place};
