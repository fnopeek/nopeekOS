//! Deciding what encoding a document is in, and getting it to UTF-8.
//!
//! The engine takes `&str`, so bytes that are not valid UTF-8 have to become
//! valid UTF-8 somewhere. Doing that with `from_utf8().unwrap_or("")` is a
//! cliff: ONE bad byte discards the entire document, and the reader gets a
//! blank page. google.ch hit exactly that — it serves ISO-8859-1, so every
//! umlaut in it was an invalid byte.
//!
//! Only two encodings are handled: UTF-8, and windows-1252 for everything
//! legacy-Latin. That covers the Western web; a page in Shift_JIS or GBK
//! still comes out wrong, but it comes out *readable-ish* rather than empty,
//! which is the property that matters here.

/// The encoding a document's bytes are in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    Utf8,
    /// windows-1252. Also what `ISO-8859-1` means in practice: the HTML
    /// standard requires that label to be decoded as windows-1252, because
    /// pages tagged Latin-1 have always used the C1 range for curly quotes.
    Windows1252,
}

/// Pick the encoding for a document.
///
/// Order follows the HTML standard's precedence: the transport header wins,
/// then the document's own `<meta>`, then a sniff. The sniff is the part
/// that actually saves pages — plenty of servers send no charset at all.
pub fn detect(content_type: Option<&str>, body: &[u8]) -> Encoding {
    if let Some(enc) = content_type.and_then(charset_param).and_then(label_to_encoding) {
        return enc;
    }
    if let Some(enc) = meta_encoding(body) {
        return enc;
    }
    // Nothing declared. Valid UTF-8 is overwhelmingly likely to BE UTF-8;
    // anything else is legacy-Latin far more often than not. Note this also
    // makes pure ASCII come out as UTF-8, which is correct and free.
    if core::str::from_utf8(body).is_ok() {
        Encoding::Utf8
    } else {
        Encoding::Windows1252
    }
}

/// Extract the `charset=` parameter from a Content-Type value.
fn charset_param(content_type: &str) -> Option<&str> {
    let lower_pos = find_ci(content_type, "charset")?;
    let rest = content_type[lower_pos + "charset".len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    // The value may be quoted, and may be followed by another `;` parameter.
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let end = rest
        .find(|c: char| c == '"' || c == ';' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Sniff a `<meta>` charset declaration out of the head of the document.
///
/// Bounded to the first 1024 bytes, as the HTML standard specifies: the
/// declaration has to come early to be usable at all, and scanning a whole
/// 3 MB document for it would cost more than it saves.
///
/// Returns the decoded `Encoding` rather than the label, so nothing borrows
/// from the scratch buffer below.
fn meta_encoding(body: &[u8]) -> Option<Encoding> {
    // The document is not valid UTF-8 — that is the whole reason we are
    // here — so it cannot simply be viewed as `&str`. Every byte above ASCII
    // becomes a placeholder: markup and charset labels are ASCII, so nothing
    // that matters is lost, and a non-ASCII byte sitting in a comment or a
    // title BEFORE the declaration no longer cuts the scan short.
    const HEAD: usize = 1024;
    let n = body.len().min(HEAD);
    let mut ascii = [0u8; HEAD];
    for (i, &b) in body[..n].iter().enumerate() {
        ascii[i] = if b < 0x80 { b } else { b'?' };
    }
    let text = core::str::from_utf8(&ascii[..n]).ok()?;

    let mut rest = text;
    loop {
        let i = find_ci(rest, "<meta")?;
        rest = &rest[i + 5..];
        let tag_end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..tag_end];

        // <meta charset="utf-8">
        if let Some(enc) = attr_value(tag, "charset").and_then(label_to_encoding) {
            return Some(enc);
        }
        // <meta http-equiv="Content-Type" content="text/html; charset=…">
        if let Some(enc) = attr_value(tag, "content")
            .and_then(charset_param)
            .and_then(label_to_encoding)
        {
            return Some(enc);
        }
        rest = &rest[tag_end..];
    }
}

/// Read `name="value"` (or unquoted) out of a tag's attribute text.
fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    loop {
        let i = find_ci(rest, name)?;
        // Must be a whole attribute name, not the tail of another one
        // (`data-charset` must not answer for `charset`).
        let before_ok = i == 0
            || !rest.as_bytes()[i - 1].is_ascii_alphanumeric()
                && rest.as_bytes()[i - 1] != b'-'
                && rest.as_bytes()[i - 1] != b'_';
        let after = rest[i + name.len()..].trim_start();
        if before_ok {
            if let Some(v) = after.strip_prefix('=') {
                let v = v.trim_start();
                let (quote, v) = match v.strip_prefix('"') {
                    Some(v) => (Some('"'), v),
                    None => match v.strip_prefix('\'') {
                        Some(v) => (Some('\''), v),
                        None => (None, v),
                    },
                };
                let end = match quote {
                    Some(q) => v.find(q).unwrap_or(v.len()),
                    None => v
                        .find(|c: char| c.is_whitespace() || c == '>')
                        .unwrap_or(v.len()),
                };
                return Some(&v[..end]);
            }
        }
        rest = &rest[i + name.len()..];
    }
}

/// Map a charset label to what we will actually decode it as.
fn label_to_encoding(label: &str) -> Option<Encoding> {
    let l = label.trim();
    let eq = |a: &str| l.len() == a.len() && l.bytes().zip(a.bytes())
        .all(|(x, y)| x.to_ascii_lowercase() == y);
    if eq("utf-8") || eq("utf8") {
        return Some(Encoding::Utf8);
    }
    // Every one of these is decoded as windows-1252 — see `Encoding`.
    for a in ["iso-8859-1", "iso8859-1", "latin1", "latin-1", "windows-1252",
              "cp1252", "us-ascii", "ascii", "iso-8859-15"] {
        if eq(a) {
            return Some(Encoding::Windows1252);
        }
    }
    // A label we don't know (Shift_JIS, GBK, …). Returning None lets the
    // sniff decide, which at least keeps the page from vanishing.
    None
}

/// Case-insensitive substring search, ASCII only.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| {
        h[i..i + n.len()]
            .iter()
            .zip(n)
            .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
    })
}

/// UTF-8 for a windows-1252 byte, and how many bytes that takes.
///
/// 0x00–0x7F is ASCII. 0xA0–0xFF is Latin-1, one code point up in the
/// U+0080 block. 0x80–0x9F is where windows-1252 differs from Latin-1:
/// it puts printable punctuation there, which is why pages tagged
/// ISO-8859-1 must still be decoded this way.
fn cp1252_to_utf8(b: u8) -> ([u8; 3], usize) {
    const C1: [u16; 32] = [
        0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
        0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F,
        0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
        0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    ];
    let cp: u32 = match b {
        0x00..=0x7F => return ([b, 0, 0], 1),
        0x80..=0x9F => C1[(b - 0x80) as usize] as u32,
        _ => b as u32,
    };
    if cp < 0x800 {
        ([0xC0 | (cp >> 6) as u8, 0x80 | (cp & 0x3F) as u8, 0], 2)
    } else {
        (
            [
                0xE0 | (cp >> 12) as u8,
                0x80 | ((cp >> 6) & 0x3F) as u8,
                0x80 | (cp & 0x3F) as u8,
            ],
            3,
        )
    }
}

/// How many bytes `src` becomes as UTF-8.
pub fn decoded_len(src: &[u8]) -> usize {
    src.iter().map(|&b| cp1252_to_utf8(b).1).sum()
}

/// Transcode windows-1252 → UTF-8 **inside** `buf`, in place.
///
/// `len` is the current byte count; the result is longer, so the work goes
/// back to front: the last source byte is read before anything has been
/// written over it, and every write lands at or after where its source sat.
/// Reversing the direction here would corrupt the document instead of
/// growing it.
///
/// Returns the new length, or `None` if the result would not fit — the
/// caller must not silently produce a half-transcoded buffer.
pub fn transcode_in_place(buf: &mut [u8], len: usize) -> Option<usize> {
    let out_len = decoded_len(&buf[..len]);
    if out_len > buf.len() {
        return None;
    }
    let mut w = out_len;
    for r in (0..len).rev() {
        let (bytes, n) = cp1252_to_utf8(buf[r]);
        w -= n;
        buf[w..w + n].copy_from_slice(&bytes[..n]);
    }
    debug_assert_eq!(w, 0);
    Some(out_len)
}

/// Replace every byte that is not part of a valid UTF-8 sequence with `?`.
///
/// Length-preserving, so it needs no room. For a document that IS UTF-8 but
/// carries a few broken bytes, this is the right repair — transcoding the
/// whole thing as windows-1252 instead would double-encode every correct
/// accent in it.
pub fn sanitize_utf8(buf: &mut [u8]) {
    let mut start = 0;
    while start < buf.len() {
        match core::str::from_utf8(&buf[start..]) {
            Ok(_) => return,
            Err(e) => {
                let bad = start + e.valid_up_to();
                // `error_len() == None` means the input ends mid-sequence —
                // everything from here on is unusable.
                let n = e.error_len().unwrap_or(buf.len() - bad);
                for b in &mut buf[bad..bad + n] {
                    *b = b'?';
                }
                start = bad + n;
            }
        }
    }
}

/// What [`to_utf8_in_place`] did, for the log.
pub const KEPT: &str = "utf-8";
pub const TRANSCODED: &str = "windows-1252 -> utf-8";
pub const REPAIRED: &str = "utf-8 (repaired)";
pub const TRUNCATED: &str = "windows-1252 (no room, repaired)";

/// Bring `buf[..len]` to valid UTF-8 in place, and say what it took.
///
/// This is the whole policy in one place, because the failure it replaces —
/// `from_utf8().unwrap_or("")` — was a blank page, and the one thing every
/// branch here must guarantee is that a document never becomes nothing.
pub fn to_utf8_in_place(
    buf: &mut [u8],
    len: usize,
    content_type: Option<&str>,
) -> (usize, &'static str) {
    match detect(content_type, &buf[..len]) {
        Encoding::Utf8 => {
            if core::str::from_utf8(&buf[..len]).is_ok() {
                return (len, KEPT);
            }
            // Declared (or sniffed as) UTF-8 and mostly is, but not entirely.
            sanitize_utf8(&mut buf[..len]);
            (len, REPAIRED)
        }
        Encoding::Windows1252 => match transcode_in_place(buf, len) {
            Some(n) => (n, TRANSCODED),
            // Growing it would overflow the buffer. Repairing in place keeps
            // the page at the cost of its accented characters, which beats
            // handing back a document that cannot be shown at all.
            None => {
                sanitize_utf8(&mut buf[..len]);
                (len, TRUNCATED)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    /// A document that IS UTF-8 but carries a stray bad byte must keep its
    /// correct accents — transcoding the whole thing would double-encode
    /// every one of them.
    #[test]
    fn a_stray_bad_byte_is_repaired_not_transcoded() {
        let mut buf = [0u8; 64];
        let src = "grün".as_bytes();          // valid UTF-8, 5 bytes
        buf[..src.len()].copy_from_slice(src);
        buf[src.len()] = 0xFF;                // one impossible byte
        let len = src.len() + 1;
        let (n, how) = to_utf8_in_place(&mut buf, len, Some("text/html; charset=utf-8"));
        assert_eq!(how, REPAIRED);
        assert_eq!(core::str::from_utf8(&buf[..n]).unwrap(), "grün?");
    }

    /// The google.ch shape end to end: Latin-1 bytes, header says so.
    #[test]
    fn a_latin1_document_becomes_readable_utf8() {
        let mut buf = [0u8; 64];
        let src = [b'f', 0xFC, b'r', b' ', b'j', 0xE4, b'h'];
        buf[..src.len()].copy_from_slice(&src);
        let (n, how) = to_utf8_in_place(&mut buf, src.len(), Some("text/html; charset=ISO-8859-1"));
        assert_eq!(how, TRANSCODED);
        assert_eq!(core::str::from_utf8(&buf[..n]).unwrap(), "für jäh");
    }

    /// The property that matters most: whatever the input, the result is
    /// valid UTF-8 and never empty. That is the blank page, gone.
    #[test]
    fn no_input_can_produce_an_empty_or_invalid_document() {
        let patterns: [&[u8]; 6] = [
            &[0xFF, 0xFE, 0xFD],
            &[0xC3],                       // truncated sequence
            &[0xE2, 0x82],                 // truncated 3-byte
            &[0x41, 0x80, 0x42],
            "ok".as_bytes(),
            &[0xFC; 40],
        ];
        for (i, p) in patterns.iter().enumerate() {
            for ct in [None, Some("text/html; charset=utf-8"), Some("text/html; charset=latin1")] {
                let mut buf = [0u8; 128];
                buf[..p.len()].copy_from_slice(p);
                let (n, _) = to_utf8_in_place(&mut buf, p.len(), ct);
                assert!(n > 0, "pattern {i} with {ct:?} produced an empty document");
                core::str::from_utf8(&buf[..n])
                    .unwrap_or_else(|e| panic!("pattern {i} with {ct:?}: invalid UTF-8: {e}"));
            }
        }
    }

    /// A buffer with no headroom must still come back showable.
    #[test]
    fn a_full_buffer_degrades_instead_of_failing() {
        let mut buf = [0xFCu8; 8];
        let (n, how) = to_utf8_in_place(&mut buf, 8, Some("text/html; charset=latin1"));
        assert_eq!(how, TRUNCATED);
        assert_eq!(core::str::from_utf8(&buf[..n]).unwrap(), "????????");
    }

    #[test]
    fn header_charset_wins() {
        assert_eq!(detect(Some("text/html; charset=ISO-8859-1"), b"plain"), Encoding::Windows1252);
        assert_eq!(detect(Some("text/html;charset=\"utf-8\""), b"plain"), Encoding::Utf8);
        assert_eq!(detect(Some("text/html; charset=utf-8; boundary=x"), b""), Encoding::Utf8);
    }

    #[test]
    fn meta_is_the_fallback() {
        assert_eq!(detect(None, b"<html><head><meta charset='windows-1252'>"), Encoding::Windows1252);
        assert_eq!(detect(None, b"<meta charset=utf-8>"), Encoding::Utf8);
        assert_eq!(
            detect(None, b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-8859-1\">"),
            Encoding::Windows1252
        );
    }

    /// `data-charset` must not answer for `charset` — the check that a
    /// substring match alone would get wrong.
    #[test]
    fn a_similarly_named_attribute_does_not_count() {
        assert_eq!(detect(None, b"<meta data-charset='iso-8859-1'>"), Encoding::Utf8);
    }

    #[test]
    fn undeclared_falls_back_to_sniffing() {
        assert_eq!(detect(None, b"plain ascii"), Encoding::Utf8);
        assert_eq!(detect(None, "wörter".as_bytes()), Encoding::Utf8, "valid UTF-8 stays UTF-8");
        assert_eq!(detect(None, &[b'w', 0xF6, b'r']), Encoding::Windows1252, "a lone 0xF6 is Latin-1");
    }

    /// An encoding we cannot decode must not blank the page: fall through to
    /// the sniff rather than pretending we know.
    #[test]
    fn an_unknown_label_defers_to_the_sniff() {
        assert_eq!(detect(Some("text/html; charset=Shift_JIS"), b"ascii"), Encoding::Utf8);
        assert_eq!(detect(Some("text/html; charset=Shift_JIS"), &[0xF6]), Encoding::Windows1252);
    }

    #[test]
    fn transcoding_grows_the_text_in_place() {
        let mut buf = [0u8; 32];
        buf[..3].copy_from_slice(&[b'f', 0xFC, b'r']); // "für" in Latin-1
        let n = transcode_in_place(&mut buf, 3).unwrap();
        assert_eq!(core::str::from_utf8(&buf[..n]).unwrap(), "für");
    }

    /// The C1 range is where windows-1252 and Latin-1 part ways, and it is
    /// the case that grows one byte into three.
    #[test]
    fn the_c1_range_decodes_as_windows_1252() {
        let mut buf = [0u8; 32];
        buf[..3].copy_from_slice(&[b'a', 0x92, b'b']); // right single quote
        let n = transcode_in_place(&mut buf, 3).unwrap();
        assert_eq!(core::str::from_utf8(&buf[..n]).unwrap(), "a\u{2019}b");
        assert_eq!(n, 5);
    }

    #[test]
    fn ascii_is_unchanged_and_costs_nothing() {
        let mut buf = [0u8; 16];
        buf[..5].copy_from_slice(b"hello");
        assert_eq!(transcode_in_place(&mut buf, 5), Some(5));
        assert_eq!(&buf[..5], b"hello");
    }

    /// Refusing is the point: a buffer that cannot hold the result must not
    /// come back half-transcoded.
    #[test]
    fn it_refuses_rather_than_overflow() {
        let mut buf = [0u8; 4];
        buf[..4].copy_from_slice(&[0xFC, 0xFC, 0xFC, 0xFC]); // needs 8
        assert_eq!(transcode_in_place(&mut buf, 4), None);
        assert_eq!(&buf[..4], &[0xFC, 0xFC, 0xFC, 0xFC], "left untouched");
    }

    /// Every byte must survive a round trip — the property that decides
    /// whether a page reads correctly or is quietly mangled.
    #[test]
    fn every_byte_decodes_to_something_valid() {
        for b in 0u8..=255 {
            let mut buf = [0u8; 8];
            buf[0] = b;
            let n = transcode_in_place(&mut buf, 1).unwrap();
            let s = core::str::from_utf8(&buf[..n])
                .unwrap_or_else(|_| panic!("byte {b:#04x} produced invalid UTF-8"));
            assert_eq!(s.chars().count(), 1, "byte {b:#04x} must be exactly one char");
        }
    }
}
