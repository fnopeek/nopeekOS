//! The nopeekOS host ABI for WASM modules, made memory-safe.
//!
//! Every `npk_*` import a module shares with others is declared here once.
//! Each function takes slices where the import takes a pointer and a
//! length, and returns what the kernel returns, unchanged: a module keeps
//! its own reading of the result codes. The kernel checks every range
//! against the instance's linear memory, and no host call re-enters the
//! guest, so the only `unsafe` left is the FFI call itself, here.
//!
//! No allocator is needed; drivers and small tools use this directly, the
//! widget SDK builds on it.

#![no_std]

pub mod bump;
pub mod cell;

#[cfg(target_arch = "wasm32")]
mod calls;
#[cfg(target_arch = "wasm32")]
pub use calls::*;
