# `tools/wasm/beak-engine/src/charset.rs` @ 5e0102684

## L1-12 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
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
```

## L14 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// The encoding a document's bytes are in.
```

## L18-20 · `Windows1252,`

```
/// windows-1252. Also what `ISO-8859-1` means in practice: the HTML
/// standard requires that label to be decoded as windows-1252, because
/// pages tagged Latin-1 have always used the C1 range for curly quotes.
```

## L24-28 · `pub fn detect(content_type: Option<&str>, body: &[u8]) -> Encoding {`

```
/// Pick the encoding for a document.
///
/// Order follows the HTML standard's precedence: the transport header wins,
/// then the document's own `<meta>`, then a sniff. The sniff is the part
/// that actually saves pages — plenty of servers send no charset at all.
```

## L36-38 · `if core::str::from_utf8(body).is_ok() {`

```
// Nothing declared. Valid UTF-8 is overwhelmingly likely to BE UTF-8;
// anything else is legacy-Latin far more often than not. Note this also
// makes pure ASCII come out as UTF-8, which is correct and free.
```

## L46 · `fn charset_param(content_type: &str) -> Option<&str> {`

```
/// Extract the `charset=` parameter from a Content-Type value.
```

## L51 · `let rest = rest.strip_prefix('"').unwrap_or(rest);`

```
// The value may be quoted, and may be followed by another `;` parameter.
```

## L59-66 · `fn meta_encoding(body: &[u8]) -> Option<Encoding> {`

```
/// Sniff a `<meta>` charset declaration out of the head of the document.
///
/// Bounded to the first 1024 bytes, as the HTML standard specifies: the
/// declaration has to come early to be usable at all, and scanning a whole
/// 3 MB document for it would cost more than it saves.
///
/// Returns the decoded `Encoding` rather than the label, so nothing borrows
/// from the scratch buffer below.
```

## L68-72 · `const HEAD: usize = 1024;`

```
// The document is not valid UTF-8 — that is the whole reason we are
// here — so it cannot simply be viewed as `&str`. Every byte above ASCII
// becomes a placeholder: markup and charset labels are ASCII, so nothing
// that matters is lost, and a non-ASCII byte sitting in a comment or a
// title BEFORE the declaration no longer cuts the scan short.
```

## L88 · `if let Some(enc) = attr_value(tag, "charset").and_then(label_to_encoding) {`

```
// <meta charset="utf-8">
```

## L92 · `if let Some(enc) = attr_value(tag, "content")`

```
// <meta http-equiv="Content-Type" content="text/html; charset=…">
```

## L103 · `fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {`

```
/// Read `name="value"` (or unquoted) out of a tag's attribute text.
```

## L108-109 · `let before_ok = i == 0`

```
// Must be a whole attribute name, not the tail of another one
// (`data-charset` must not answer for `charset`).
```

## L138 · `fn label_to_encoding(label: &str) -> Option<Encoding> {`

```
/// Map a charset label to what we will actually decode it as.
```

## L146 · `for a in ["iso-8859-1", "iso8859-1", "latin1", "latin-1", "windows-1252",`

```
// Every one of these is decoded as windows-1252 — see `Encoding`.
```

## L153-154 · `None`

```
// A label we don't know (Shift_JIS, GBK, …). Returning None lets the
// sniff decide, which at least keeps the page from vanishing.
```

## L158 · `fn find_ci(haystack: &str, needle: &str) -> Option<usize> {`

```
/// Case-insensitive substring search, ASCII only.
```

## L172-177 · `fn cp1252_to_utf8(b: u8) -> ([u8; 3], usize) {`

```
/// UTF-8 for a windows-1252 byte, and how many bytes that takes.
///
/// 0x00–0x7F is ASCII. 0xA0–0xFF is Latin-1, one code point up in the
/// U+0080 block. 0x80–0x9F is where windows-1252 differs from Latin-1:
/// it puts printable punctuation there, which is why pages tagged
/// ISO-8859-1 must still be decoded this way.
```

## L204 · `pub fn decoded_len(src: &[u8]) -> usize {`

```
/// How many bytes `src` becomes as UTF-8.
```

## L209-218 · `pub fn transcode_in_place(buf: &mut [u8], len: usize) -> Option<usize> {`

```
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
```

## L234-239 · `pub fn sanitize_utf8(buf: &mut [u8]) {`

```
/// Replace every byte that is not part of a valid UTF-8 sequence with `?`.
///
/// Length-preserving, so it needs no room. For a document that IS UTF-8 but
/// carries a few broken bytes, this is the right repair — transcoding the
/// whole thing as windows-1252 instead would double-encode every correct
/// accent in it.
```

## L247-248 · `let n = e.error_len().unwrap_or(buf.len() - bad);`

```
// `error_len() == None` means the input ends mid-sequence —
// everything from here on is unusable.
```

## L259 · `pub const KEPT: &str = "utf-8";`

```
/// What [`to_utf8_in_place`] did, for the log.
```

## L265-269 · `pub fn to_utf8_in_place(`

```
/// Bring `buf[..len]` to valid UTF-8 in place, and say what it took.
///
/// This is the whole policy in one place, because the failure it replaces —
/// `from_utf8().unwrap_or("")` — was a blank page, and the one thing every
/// branch here must guarantee is that a document never becomes nothing.
```

## L280 · `sanitize_utf8(&mut buf[..len]);`

```
// Declared (or sniffed as) UTF-8 and mostly is, but not entirely.
```

## L286-288 · `None => {`

```
// Growing it would overflow the buffer. Repairing in place keeps
// the page at the cost of its accented characters, which beats
// handing back a document that cannot be shown at all.
```

## L302-304 · `#[test]`

```
/// A document that IS UTF-8 but carries a stray bad byte must keep its
/// correct accents — transcoding the whole thing would double-encode
/// every one of them.
```

## L308 · `let src = "grün".as_bytes();          // valid UTF-8, 5 bytes`

```
// valid UTF-8, 5 bytes
```

## L310 · `buf[src.len()] = 0xFF;                // one impossible byte`

```
// one impossible byte
```

## L317 · `#[test]`

```
/// The google.ch shape end to end: Latin-1 bytes, header says so.
```

## L328-329 · `#[test]`

```
/// The property that matters most: whatever the input, the result is
/// valid UTF-8 and never empty. That is the blank page, gone.
```

## L334 · `&[0xC3],                       // truncated sequence`

```
// truncated sequence
```

## L335 · `&[0xE2, 0x82],                 // truncated 3-byte`

```
// truncated 3-byte
```

## L352 · `#[test]`

```
/// A buffer with no headroom must still come back showable.
```

## L378-379 · `#[test]`

```
/// `data-charset` must not answer for `charset` — the check that a
/// substring match alone would get wrong.
```

## L392-393 · `#[test]`

```
/// An encoding we cannot decode must not blank the page: fall through to
/// the sniff rather than pretending we know.
```

## L403 · `buf[..3].copy_from_slice(&[b'f', 0xFC, b'r']); // "für" in Latin-1`

```
// "für" in Latin-1
```

## L408-409 · `#[test]`

```
/// The C1 range is where windows-1252 and Latin-1 part ways, and it is
/// the case that grows one byte into three.
```

## L413 · `buf[..3].copy_from_slice(&[b'a', 0x92, b'b']); // right single quote`

```
// right single quote
```

## L427-428 · `#[test]`

```
/// Refusing is the point: a buffer that cannot hold the result must not
/// come back half-transcoded.
```

## L432 · `buf[..4].copy_from_slice(&[0xFC, 0xFC, 0xFC, 0xFC]); // needs 8`

```
// needs 8
```

## L437-438 · `#[test]`

```
/// Every byte must survive a round trip — the property that decides
/// whether a page reads correctly or is quietly mangled.
```

