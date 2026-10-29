//! net_core — the network stack's protocol logic.
//!
//! Pure state machines over an [`tcp::Env`] the kernel provides (clock,
//! randomness, our address, the next hop, the IP send path), so the same
//! code runs in the kernel and under `cargo test` on a virtual clock.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod tcp;
