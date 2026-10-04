# `kernel/src/intent/http2/hpack.rs` @ 5e0102684

## L1-13 · `use alloc::collections::VecDeque;`

```
//! HPACK — header compression for HTTP/2 (RFC 7541).
//!
//! Full client-side decoder including the dynamic table. An earlier plan was
//! to advertise `SETTINGS_HEADER_TABLE_SIZE = 0` and skip the dynamic table
//! entirely; that was dropped for two reasons. It would have cost us the
//! RFC's own worked examples (Appendix C) as a test oracle — most of them
//! reference dynamic indices — and it left a real failure mode, since a peer
//! that indexes dynamically anyway would break every page rather than one
//! header.
//!
//! Our *encoder* stays deliberately dumb: literals without indexing, never
//! Huffman-coded, so our own dynamic table stays empty. Request headers are a
//! handful of short strings; the bytes saved would not pay for an encoder.
```

## L23 · `Truncated,`

```
/// Ran off the end of the block mid-instruction.
```

## L25 · `BadIndex(usize),`

```
/// An index naming neither a static nor a dynamic entry.
```

## L27 · `BadHuffman,`

```
/// A Huffman bit sequence that decodes to no symbol, or bad EOS padding.
```

## L29 · `IntegerOverflow,`

```
/// An integer whose continuation bytes exceed what a length can hold.
```

## L31 · `TooLarge,`

```
/// A header, or the table, beyond what we are willing to buffer.
```

## L35-37 · `const MAX_STRING: usize = 16 * 1024;`

```
/// Cap on a single decoded header name or value. Real response headers are
/// well under this; the point is that a hostile peer cannot make us allocate
/// unboundedly from a few bytes of Huffman.
```

## L40-42 · `pub const MAX_TABLE_SIZE: usize = 4096;`

```
/// What we advertise as `SETTINGS_HEADER_TABLE_SIZE`, and the ceiling a
/// dynamic table size update may set (RFC 7541 §6.3). 4096 is the protocol
/// default; bounding it bounds our memory.
```

## L45 · `fn decode_int(buf: &[u8], pos: &mut usize, n: u32) -> Result<usize, HpackError> {`

```
// ── Integers (RFC 7541 §5.1) ────────────────────────────────────────────────
```

## L47-48 · `fn decode_int(buf: &[u8], pos: &mut usize, n: u32) -> Result<usize, HpackError> {`

```
/// Decode an integer with an `n`-bit prefix. `pos` points at the prefix byte
/// and is advanced past the whole integer.
```

## L57 · `let mut shift = 0u32;`

```
// Continuation octets, 7 bits each, low bits first.
```

## L75-76 · `fn encode_int(out: &mut Vec<u8>, mut value: usize, n: u32, prefix_bits: u8) {`

```
/// Encode an integer with an `n`-bit prefix. `prefix_bits` supplies the flag
/// bits above the prefix and must not intrude into the low `n` bits.
```

## L92 · `fn huffman_decode(src: &[u8]) -> Result<Vec<u8>, HpackError> {`

```
// ── Huffman (RFC 7541 §5.2 + Appendix B) ────────────────────────────────────
```

## L94-101 · `fn huffman_decode(src: &[u8]) -> Result<Vec<u8>, HpackError> {`

```
/// Canonical-Huffman decode. Walks the input bit by bit, growing a candidate
/// code; at each bit length it checks whether the accumulated value falls in
/// that length's consecutive code range, which is what "canonical" buys us —
/// no trie to build, no per-symbol scan.
///
/// Padding: the encoder pads the final byte with the high bits of EOS (all
/// ones). Fewer than 8 such bits are required, and a longer or non-ones pad
/// is malformed (§5.2).
```

## L115 · `return Err(HpackError::BadHuffman); // EOS is not a symbol`

```
// EOS is not a symbol
```

## L126 · `if len >= 8 || cur != (1u32 << len) - 1 {`

```
// Whatever is left must be EOS-prefix padding: all ones, under 8 bits.
```

## L133 · `fn lookup(code: u32, len: u8) -> Option<u16> {`

```
/// The symbol whose code is exactly `code` in `len` bits, if any.
```

## L142-143 · `const LENGTH_INDEX: [(u32, u32, u32); 31] = build_length_index();`

```
/// Per bit length: (first code, index into `SORTED_SYMS`, how many codes).
/// Built at compile time from `HUFFMAN`, so it cannot drift from the table.
```

## L145 · `const SORTED_SYMS: [u16; 257] = build_sorted_syms();`

```
/// Symbols ordered by (code length, code) — i.e. canonical order.
```

## L153-154 · `let mut sym = 0usize;`

```
// Codes of one length are consecutive and ordered by symbol, so
// ascending symbol order is ascending code order.
```

## L194 · `fn decode_string(buf: &[u8], pos: &mut usize) -> Result<Vec<u8>, HpackError> {`

```
// ── Strings (RFC 7541 §5.2) ─────────────────────────────────────────────────
```

## L213 · `encode_int(out, s.len(), 7, 0x00); // H = 0, we never Huffman-encode`

```
// H = 0, we never Huffman-encode
```

## L217 · `#[derive(Clone, Debug, PartialEq)]`

```
// ── Header blocks ───────────────────────────────────────────────────────────
```

## L219-220 · `#[derive(Clone, Debug, PartialEq)]`

```
/// One decoded header. Names arrive lowercase per HTTP/2 §8.2.1; we do not
/// re-case them.
```

## L227-229 · `pub struct Decoder {`

```
/// Decoder state that must persist across every header block on a connection
/// — the dynamic table is connection-scoped, so one `Decoder` per connection
/// and never a fresh one per response.
```

## L231 · `table: VecDeque<Header>,`

```
/// Newest entry at the front, which is index 62 (RFC 7541 §2.3.3).
```

## L248-250 · `pub const fn with_max(max: usize) -> Self {`

```
/// A decoder for a connection that advertised a table size other than the
/// 4096 default. `max` above `MAX_TABLE_SIZE` is clamped, since we must
/// never let a peer size our table past what we advertised.
```

## L256-258 · `fn entry_size(h: &Header) -> usize {`

```
/// Entry cost per §4.1: the octets of name and value plus 32 for
/// bookkeeping. Deliberately *not* our real allocation size — the number
/// has to match the peer's accounting or the tables desynchronise.
```

## L265-266 · `if need > self.max_size {`

```
// §4.4: an entry larger than the whole table empties it and is not
// added. That is not an error.
```

## L283 · `if max > MAX_TABLE_SIZE {`

```
// §6.3: an update may not exceed what we advertised in SETTINGS.
```

## L311-315 · `pub fn decode(&mut self, block: &[u8]) -> Result<Vec<Header>, HpackError> {`

```
/// Decode one complete header block.
///
/// Byte sequences that are not UTF-8 are kept lossily rather than
/// rejected: a header value is defined over octets, and a broken
/// `server:` line should not fail a page load.
```

## L322 · `let idx = decode_int(block, &mut pos, 7)?;`

```
// §6.1 Indexed Header Field — name and value from the table.
```

## L326 · `let h = self.literal(block, &mut pos, 6)?;`

```
// §6.2.1 Literal with Incremental Indexing — also stored.
```

## L331 · `let size = decode_int(block, &mut pos, 5)?;`

```
// §6.3 Dynamic Table Size Update.
```

## L335 · `out.push(self.literal(block, &mut pos, 4)?);`

```
// §6.2.2 / §6.2.3 Literal without / never indexed.
```

## L362 · `Err(e) => e.as_bytes().iter().map(|&b| b as char).collect(),`

```
// Latin-1 the remainder rather than dropping the header entirely.
```

## L367-369 · `pub fn encode(headers: &[(&str, &str)]) -> Vec<u8> {`

```
/// Encode a request header block. Names must already be lowercase and the
/// pseudo-headers must come first (HTTP/2 §8.1.2.1) — the caller owns that
/// ordering.
```

## L373-374 · `match static_name_index(name) {`

```
// Prefer a static index for the name: one byte instead of the whole
// string, for a short scan over 61 entries.
```

## L378 · `out.push(0x00); // literal without indexing, new name`

```
// literal without indexing, new name
```

