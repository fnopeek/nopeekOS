# `kernel/src/crypto/tls/asn1.rs` @ 5e0102684

## L1-3 · `#[derive(Debug, Clone, Copy)]`

```
//! Minimal ASN.1 DER Parser (zero-copy)
//!
//! Only parses what X.509 certificates need for TLS.
```

## L11-12 · `pub fn parse_tlv(data: &[u8]) -> Option<(Tlv<'_>, &[u8])> {`

```
/// Parse one TLV (Tag-Length-Value) from DER-encoded data.
/// Returns (Tlv, remaining bytes).
```

## L29 · `pub fn parse_sequence_contents(data: &[u8]) -> SequenceIter<'_> {`

```
/// Parse all TLVs inside a SEQUENCE.
```

## L54 · `Some((first as usize, 1))`

```
// Short form
```

## L57 · `None // Indefinite length not supported`

```
// Indefinite length not supported
```

## L69 · `pub const TAG_INTEGER: u8 = 0x02;`

```
// ASN.1 tag constants
```

## L84 · `pub const TAG_CONTEXT_0: u8 = 0xA0;`

```
// Context-specific tags (used in X.509)
```

## L88 · `pub fn oid_matches(tlv: &Tlv<'_>, expected: &[u8]) -> bool {`

```
/// Compare an OID value against a known OID byte sequence
```

