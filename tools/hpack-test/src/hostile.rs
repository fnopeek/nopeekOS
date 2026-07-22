//! The decoder parses bytes straight off the network inside the kernel, where
//! a panic is a halt. These assert the only property that really matters
//! there: it may reject anything, but it must never panic.
use crate::hpack::{encode, Decoder};

fn hx(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap()).collect()
}

/// Every prefix of a valid block is a truncation a peer could actually send.
#[test]
fn truncations_never_panic() {
    let valid = [
        "828684be5808 6e6f2d6361636865".replace(' ', ""),
        "400a637573746f6d2d6b65790d637573746f6d2d686561646572".into(),
        "828487408825a849e95ba97d7f8925a849e95bb8e8b4bf".into(),
    ];
    for v in valid {
        let bytes = hx(&v);
        for n in 0..=bytes.len() {
            let mut d = Decoder::new();
            let _ = d.decode(&bytes[..n]); // must return, panic-free
        }
    }
}

/// A cheap deterministic sweep of the instruction space: every first byte,
/// followed by assorted tails. Catches indexing panics in the prefix decode.
#[test]
fn arbitrary_bytes_never_panic() {
    let tails: [&[u8]; 6] = [
        &[],
        &[0x00],
        &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        &[0x7f, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00],
        &[0x8f, 0xff, 0xff, 0xff],
        &[0x0a, b'x', b'y'],
    ];
    for first in 0u16..=255 {
        for tail in tails {
            let mut buf = vec![first as u8];
            buf.extend_from_slice(tail);
            let mut d = Decoder::new();
            let _ = d.decode(&buf);
        }
    }
}

/// A length prefix far larger than the block must be refused, not trusted
/// into a huge allocation or an out-of-range slice.
#[test]
fn absurd_string_length_is_refused() {
    // literal, new name, length 0x7fffffff, no payload
    let mut d = Decoder::new();
    assert!(d.decode(&hx("00ff8080808007")).is_err());
}

/// EOS must not decode as a symbol, and padding must be an EOS prefix (§5.2).
#[test]
fn bad_huffman_padding_is_refused() {
    let mut d = Decoder::new();
    // literal name, Huffman string whose padding bits are zeros, not ones
    assert!(d.decode(&hx("0082 0000".replace(' ', "").as_str())).is_err());
}

/// A dynamic index nobody ever inserted.
#[test]
fn out_of_range_index_is_refused() {
    let mut d = Decoder::new();
    assert!(d.decode(&hx("be")).is_err()); // idx 62, table empty
    let mut d = Decoder::new();
    assert!(d.decode(&hx("80")).is_err()); // idx 0 is forbidden
}

/// What we encode, we must be able to decode back.
#[test]
fn encoder_round_trips() {
    let headers = [
        (":method", "GET"),
        (":scheme", "https"),
        (":authority", "de.wikipedia.org"),
        (":path", "/wiki/Wikipedia:Hauptseite"),
        ("user-agent", "beak/0.1 (nopeekOS)"),
        ("accept", "*/*"),
    ];
    let block = encode(&headers);
    let mut d = Decoder::new();
    let got = d.decode(&block).expect("round trip");
    let pairs: Vec<(String, String)> =
        got.into_iter().map(|h| (h.name, h.value)).collect();
    let want: Vec<(String, String)> =
        headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
    assert_eq!(pairs, want);
}
