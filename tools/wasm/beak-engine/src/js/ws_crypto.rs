//! SHA-1 und base64 — die zwei Bausteine des WebSocket-Handschlags.
//!
//! **SHA-1 steht hier und nicht im Kryptomodul des Kernels, und das ist
//! Absicht.** Es ist gebrochen und darf nie wieder etwas sichern; RFC 6455
//! §4.1 benutzt es auch nicht dafuer, sondern als festen Rechenschritt gegen
//! einen Zwischenspeicher, der eine Aufruest-Anfrage fuer eine gewoehnliche
//! haelt. Wer es im Kernel neben AES und ECDSA ablegte, laedt den naechsten
//! Leser ein, es fuer eine Sicherheitsfunktion zu halten.

use alloc::string::String;
use alloc::vec::Vec;

/// SHA-1 (FIPS 180-4) — 20 Bytes.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bits = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0) }
    msg.extend_from_slice(&bits.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5)
                .wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            e = d; d = c; c = b.rotate_left(30); b = a; a = t;
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// base64 mit Fuellzeichen, wie RFC 6455 es verlangt.
pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die drei Proben aus FIPS 180-4 und RFC 4648 — und die EINE aus
    /// RFC 6455 §1.3, die zaehlt: sie prueft die ganze Kette, wie der Server
    /// sie rechnet.
    #[test]
    fn sha1_und_base64_treffen_die_bekannten_werte() {
        let hex = |d: [u8; 20]| {
            let mut s = String::new();
            for b in d { s.push_str(&alloc::format!("{b:02x}")) }
            s
        };
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        // Ueber eine Blockgrenze hinaus (56 Bytes = genau die Fuellgrenze).
        assert_eq!(hex(sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
                   "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // **RFC 6455 §1.3, das Beispiel des Standards selbst.**
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let mut buf = String::from(key);
        buf.push_str("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        assert_eq!(base64(&sha1(buf.as_bytes())), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}
