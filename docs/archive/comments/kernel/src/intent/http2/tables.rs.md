# `kernel/src/intent/http2/tables.rs` @ 5e0102684

## L1-12 · `pub(super) const HUFFMAN: [(u32, u8); 257] = [`

```
//! HPACK tables, transcribed from RFC 7541 Appendices A and B.
//!
//! Generated from the RFC text rather than typed by hand — 257 Huffman rows
//! is exactly the kind of transcription that silently corrupts one entry.
//! The extraction was checked three ways: all 257 symbols present, the code
//! set is prefix-free, and the Kraft sum is exactly 1.0 (so the code is
//! complete — a single wrong length would not sum to 1).
//!
//! The Huffman code is canonical: within one bit length the codes are
//! consecutive and ordered by symbol, and each length's first code is the
//! previous length's (first + count) shifted left. `hpack::huffman_decode`
//! relies on that, and `tests` re-checks it.
```

## L14 · `pub(super) const HUFFMAN: [(u32, u8); 257] = [`

```
/// (code, bit length) per symbol 0..=256; index 256 is EOS.
```

## L275 · `pub(super) const STATIC_TABLE: [(&str, &str); 61] = [`

```
/// RFC 7541 Appendix A. Index 0 is unused; entry N is at position N-1.
```

