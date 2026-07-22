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

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use super::tables::{HUFFMAN, STATIC_TABLE};

#[derive(Debug, PartialEq)]
pub enum HpackError {
    /// Ran off the end of the block mid-instruction.
    Truncated,
    /// An index naming neither a static nor a dynamic entry.
    BadIndex(usize),
    /// A Huffman bit sequence that decodes to no symbol, or bad EOS padding.
    BadHuffman,
    /// An integer whose continuation bytes exceed what a length can hold.
    IntegerOverflow,
    /// A header, or the table, beyond what we are willing to buffer.
    TooLarge,
}

/// Cap on a single decoded header name or value. Real response headers are
/// well under this; the point is that a hostile peer cannot make us allocate
/// unboundedly from a few bytes of Huffman.
const MAX_STRING: usize = 16 * 1024;

/// What we advertise as `SETTINGS_HEADER_TABLE_SIZE`, and the ceiling a
/// dynamic table size update may set (RFC 7541 §6.3). 4096 is the protocol
/// default; bounding it bounds our memory.
pub const MAX_TABLE_SIZE: usize = 4096;

// ── Integers (RFC 7541 §5.1) ────────────────────────────────────────────────

/// Decode an integer with an `n`-bit prefix. `pos` points at the prefix byte
/// and is advanced past the whole integer.
fn decode_int(buf: &[u8], pos: &mut usize, n: u32) -> Result<usize, HpackError> {
    let first = *buf.get(*pos).ok_or(HpackError::Truncated)?;
    *pos += 1;
    let mask = ((1u32 << n) - 1) as usize;
    let mut value = (first as usize) & mask;
    if value < mask {
        return Ok(value);
    }
    // Continuation octets, 7 bits each, low bits first.
    let mut shift = 0u32;
    loop {
        let b = *buf.get(*pos).ok_or(HpackError::Truncated)?;
        *pos += 1;
        if shift >= usize::BITS - 7 {
            return Err(HpackError::IntegerOverflow);
        }
        value = value
            .checked_add(((b & 0x7F) as usize) << shift)
            .ok_or(HpackError::IntegerOverflow)?;
        shift += 7;
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
}

/// Encode an integer with an `n`-bit prefix. `prefix_bits` supplies the flag
/// bits above the prefix and must not intrude into the low `n` bits.
fn encode_int(out: &mut Vec<u8>, mut value: usize, n: u32, prefix_bits: u8) {
    let mask = ((1u32 << n) - 1) as usize;
    if value < mask {
        out.push(prefix_bits | value as u8);
        return;
    }
    out.push(prefix_bits | mask as u8);
    value -= mask;
    while value >= 0x80 {
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

// ── Huffman (RFC 7541 §5.2 + Appendix B) ────────────────────────────────────

/// Canonical-Huffman decode. Walks the input bit by bit, growing a candidate
/// code; at each bit length it checks whether the accumulated value falls in
/// that length's consecutive code range, which is what "canonical" buys us —
/// no trie to build, no per-symbol scan.
///
/// Padding: the encoder pads the final byte with the high bits of EOS (all
/// ones). Fewer than 8 such bits are required, and a longer or non-ones pad
/// is malformed (§5.2).
fn huffman_decode(src: &[u8]) -> Result<Vec<u8>, HpackError> {
    let mut out = Vec::new();
    let mut cur: u32 = 0;
    let mut len: u8 = 0;
    for byte in src {
        for bit in (0..8).rev() {
            cur = (cur << 1) | ((byte >> bit) & 1) as u32;
            len += 1;
            if len > 30 {
                return Err(HpackError::BadHuffman);
            }
            if let Some(sym) = lookup(cur, len) {
                if sym == 256 {
                    return Err(HpackError::BadHuffman); // EOS is not a symbol
                }
                if out.len() >= MAX_STRING {
                    return Err(HpackError::TooLarge);
                }
                out.push(sym as u8);
                cur = 0;
                len = 0;
            }
        }
    }
    // Whatever is left must be EOS-prefix padding: all ones, under 8 bits.
    if len >= 8 || cur != (1u32 << len) - 1 {
        return Err(HpackError::BadHuffman);
    }
    Ok(out)
}

/// The symbol whose code is exactly `code` in `len` bits, if any.
fn lookup(code: u32, len: u8) -> Option<u16> {
    let (first_code, first_sym_idx, count) = LENGTH_INDEX[len as usize];
    if count == 0 || code < first_code || code - first_code >= count {
        return None;
    }
    Some(SORTED_SYMS[(first_sym_idx + (code - first_code)) as usize])
}

/// Per bit length: (first code, index into `SORTED_SYMS`, how many codes).
/// Built at compile time from `HUFFMAN`, so it cannot drift from the table.
const LENGTH_INDEX: [(u32, u32, u32); 31] = build_length_index();
/// Symbols ordered by (code length, code) — i.e. canonical order.
const SORTED_SYMS: [u16; 257] = build_sorted_syms();

const fn build_sorted_syms() -> [u16; 257] {
    let mut out = [0u16; 257];
    let mut w = 0usize;
    let mut len = 1u8;
    while len <= 30 {
        // Codes of one length are consecutive and ordered by symbol, so
        // ascending symbol order is ascending code order.
        let mut sym = 0usize;
        while sym < 257 {
            if HUFFMAN[sym].1 == len {
                out[w] = sym as u16;
                w += 1;
            }
            sym += 1;
        }
        len += 1;
    }
    out
}

const fn build_length_index() -> [(u32, u32, u32); 31] {
    let mut out = [(0u32, 0u32, 0u32); 31];
    let mut w = 0u32;
    let mut len = 1u8;
    while len <= 30 {
        let mut count = 0u32;
        let mut first = 0u32;
        let mut seen = false;
        let mut sym = 0usize;
        while sym < 257 {
            if HUFFMAN[sym].1 == len {
                if !seen {
                    first = HUFFMAN[sym].0;
                    seen = true;
                }
                count += 1;
            }
            sym += 1;
        }
        out[len as usize] = (first, w, count);
        w += count;
        len += 1;
    }
    out
}

// ── Strings (RFC 7541 §5.2) ─────────────────────────────────────────────────

fn decode_string(buf: &[u8], pos: &mut usize) -> Result<Vec<u8>, HpackError> {
    let huffman = buf.get(*pos).ok_or(HpackError::Truncated)? & 0x80 != 0;
    let len = decode_int(buf, pos, 7)?;
    if len > MAX_STRING {
        return Err(HpackError::TooLarge);
    }
    let end = pos.checked_add(len).ok_or(HpackError::Truncated)?;
    let raw = buf.get(*pos..end).ok_or(HpackError::Truncated)?;
    *pos = end;
    if huffman {
        huffman_decode(raw)
    } else {
        Ok(raw.to_vec())
    }
}

fn encode_string(out: &mut Vec<u8>, s: &str) {
    encode_int(out, s.len(), 7, 0x00); // H = 0, we never Huffman-encode
    out.extend_from_slice(s.as_bytes());
}

// ── Header blocks ───────────────────────────────────────────────────────────

/// One decoded header. Names arrive lowercase per HTTP/2 §8.2.1; we do not
/// re-case them.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// Decoder state that must persist across every header block on a connection
/// — the dynamic table is connection-scoped, so one `Decoder` per connection
/// and never a fresh one per response.
pub struct Decoder {
    /// Newest entry at the front, which is index 62 (RFC 7541 §2.3.3).
    table: VecDeque<Header>,
    size: usize,
    max_size: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub const fn new() -> Self {
        Self::with_max(MAX_TABLE_SIZE)
    }

    /// A decoder for a connection that advertised a table size other than the
    /// 4096 default. `max` above `MAX_TABLE_SIZE` is clamped, since we must
    /// never let a peer size our table past what we advertised.
    pub const fn with_max(max: usize) -> Self {
        let max_size = if max > MAX_TABLE_SIZE { MAX_TABLE_SIZE } else { max };
        Self { table: VecDeque::new(), size: 0, max_size }
    }

    /// Entry cost per §4.1: the octets of name and value plus 32 for
    /// bookkeeping. Deliberately *not* our real allocation size — the number
    /// has to match the peer's accounting or the tables desynchronise.
    fn entry_size(h: &Header) -> usize {
        h.name.len() + h.value.len() + 32
    }

    fn insert(&mut self, h: Header) {
        let need = Self::entry_size(&h);
        // §4.4: an entry larger than the whole table empties it and is not
        // added. That is not an error.
        if need > self.max_size {
            self.table.clear();
            self.size = 0;
            return;
        }
        while self.size + need > self.max_size {
            match self.table.pop_back() {
                Some(old) => self.size -= Self::entry_size(&old),
                None => break,
            }
        }
        self.size += need;
        self.table.push_front(h);
    }

    fn resize(&mut self, max: usize) -> Result<(), HpackError> {
        // §6.3: an update may not exceed what we advertised in SETTINGS.
        if max > MAX_TABLE_SIZE {
            return Err(HpackError::TooLarge);
        }
        self.max_size = max;
        while self.size > self.max_size {
            match self.table.pop_back() {
                Some(old) => self.size -= Self::entry_size(&old),
                None => break,
            }
        }
        Ok(())
    }

    fn entry(&self, idx: usize) -> Result<Header, HpackError> {
        if idx == 0 {
            return Err(HpackError::BadIndex(idx));
        }
        if idx <= STATIC_TABLE.len() {
            let (n, v) = STATIC_TABLE[idx - 1];
            return Ok(Header { name: String::from(n), value: String::from(v) });
        }
        self.table
            .get(idx - STATIC_TABLE.len() - 1)
            .cloned()
            .ok_or(HpackError::BadIndex(idx))
    }

    /// Decode one complete header block.
    ///
    /// Byte sequences that are not UTF-8 are kept lossily rather than
    /// rejected: a header value is defined over octets, and a broken
    /// `server:` line should not fail a page load.
    pub fn decode(&mut self, block: &[u8]) -> Result<Vec<Header>, HpackError> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < block.len() {
            let b = block[pos];
            if b & 0x80 != 0 {
                // §6.1 Indexed Header Field — name and value from the table.
                let idx = decode_int(block, &mut pos, 7)?;
                out.push(self.entry(idx)?);
            } else if b & 0x40 != 0 {
                // §6.2.1 Literal with Incremental Indexing — also stored.
                let h = self.literal(block, &mut pos, 6)?;
                self.insert(h.clone());
                out.push(h);
            } else if b & 0x20 != 0 {
                // §6.3 Dynamic Table Size Update.
                let size = decode_int(block, &mut pos, 5)?;
                self.resize(size)?;
            } else {
                // §6.2.2 / §6.2.3 Literal without / never indexed.
                out.push(self.literal(block, &mut pos, 4)?);
            }
        }
        Ok(out)
    }

    fn literal(
        &self,
        block: &[u8],
        pos: &mut usize,
        prefix: u32,
    ) -> Result<Header, HpackError> {
        let idx = decode_int(block, pos, prefix)?;
        let name = if idx == 0 {
            lossy(decode_string(block, pos)?)
        } else {
            self.entry(idx)?.name
        };
        let value = lossy(decode_string(block, pos)?);
        Ok(Header { name, value })
    }
}

fn lossy(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        // Latin-1 the remainder rather than dropping the header entirely.
        Err(e) => e.as_bytes().iter().map(|&b| b as char).collect(),
    }
}

/// Encode a request header block. Names must already be lowercase and the
/// pseudo-headers must come first (HTTP/2 §8.1.2.1) — the caller owns that
/// ordering.
pub fn encode(headers: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in headers {
        // Prefer a static index for the name: one byte instead of the whole
        // string, for a short scan over 61 entries.
        match static_name_index(name) {
            Some(idx) => encode_int(&mut out, idx, 4, 0x00),
            None => {
                out.push(0x00); // literal without indexing, new name
                encode_string(&mut out, name);
            }
        }
        encode_string(&mut out, value);
    }
    out
}

fn static_name_index(name: &str) -> Option<usize> {
    let mut i = 0;
    while i < STATIC_TABLE.len() {
        if STATIC_TABLE[i].0 == name {
            return Some(i + 1);
        }
        i += 1;
    }
    None
}
