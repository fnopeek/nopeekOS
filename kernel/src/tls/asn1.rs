//! Minimal ASN.1 DER Parser (zero-copy)
//!
//! Only parses what X.509 certificates need for TLS.

#[derive(Debug, Clone, Copy)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub value: &'a [u8],
}

/// Parse one TLV (Tag-Length-Value) from DER-encoded data.
/// Returns (Tlv, remaining bytes).
pub fn parse_tlv(data: &[u8]) -> Option<(Tlv<'_>, &[u8])> {
    if data.is_empty() { return None; }

    let tag = data[0];
    let (length, header_len) = parse_length(&data[1..])?;
    let total_header = 1 + header_len;

    if data.len() < total_header + length {
        return None;
    }

    let value = &data[total_header..total_header + length];
    let rest = &data[total_header + length..];
    Some((Tlv { tag, value }, rest))
}

/// Parse all TLVs inside a SEQUENCE.
pub fn parse_sequence_contents(data: &[u8]) -> SequenceIter<'_> {
    SequenceIter { data }
}

pub struct SequenceIter<'a> {
    data: &'a [u8],
}

impl<'a> Iterator for SequenceIter<'a> {
    type Item = Tlv<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() { return None; }
        let (tlv, rest) = parse_tlv(self.data)?;
        self.data = rest;
        Some(tlv)
    }
}

fn parse_length(data: &[u8]) -> Option<(usize, usize)> {
    if data.is_empty() { return None; }

    let first = data[0];
    if first < 0x80 {
        // Short form
        Some((first as usize, 1))
    } else if first == 0x80 {
        None // Indefinite length not supported
    } else {
        let num_bytes = (first & 0x7F) as usize;
        if num_bytes > 4 || data.len() < 1 + num_bytes { return None; }
        let mut length = 0usize;
        for i in 0..num_bytes {
            length = (length << 8) | data[1 + i] as usize;
        }
        Some((length, 1 + num_bytes))
    }
}

// ASN.1 tag constants
pub const TAG_INTEGER: u8 = 0x02;
pub const TAG_BIT_STRING: u8 = 0x03;
pub const TAG_OCTET_STRING: u8 = 0x04;
#[allow(dead_code)]
pub const TAG_NULL: u8 = 0x05;
pub const TAG_OID: u8 = 0x06;
pub const TAG_UTF8_STRING: u8 = 0x0C;
pub const TAG_PRINTABLE_STRING: u8 = 0x13;
pub const TAG_IA5_STRING: u8 = 0x16;
pub const TAG_UTC_TIME: u8 = 0x17;
pub const TAG_GENERALIZED_TIME: u8 = 0x18;
pub const TAG_SEQUENCE: u8 = 0x30;
pub const TAG_SET: u8 = 0x31;

// Context-specific tags (used in X.509)
pub const TAG_CONTEXT_0: u8 = 0xA0;
pub const TAG_CONTEXT_3: u8 = 0xA3;

/// Compare an OID value against a known OID byte sequence
pub fn oid_matches(tlv: &Tlv<'_>, expected: &[u8]) -> bool {
    tlv.tag == TAG_OID && tlv.value == expected
}
