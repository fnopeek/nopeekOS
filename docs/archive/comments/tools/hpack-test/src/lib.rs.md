# `tools/hpack-test/src/lib.rs` @ 5e0102684

## L1-13 · `extern crate alloc;`

```
//! Host-side oracle for the kernel's HPACK codec (RFC 7541).
//!
//! The kernel is a `no_std` binary, so `cargo test` cannot run inside it —
//! a host build hits a duplicate `_start` at link time. This crate pulls the
//! REAL kernel sources in via `#[path]` (std supplies `alloc`), so the tested
//! code and the shipped code are the same bytes and cannot drift.
//!
//! Run: `cargo test --manifest-path tools/hpack-test/Cargo.toml`
//!
//! `rfc_vectors.rs` is generated from RFC 7541 Appendix C — all 16 worked
//! examples, including the Huffman ones and the size-256 response sequence
//! that exercises dynamic-table eviction. `hostile.rs` asserts the property
//! that matters in a kernel: malformed input may be rejected, never panic.
```

## L32-34 · `#[test]`

```
/// The decoder assumes the code is canonical. If a future edit to the
/// generated table broke that, decoding would go subtly wrong rather than
/// fail loudly — so assert the property directly.
```

## L44 · `let mut prev: Option<(u32, u32, usize)> = None; // (first code, count, len)`

```
// (first code, count, len)
```

