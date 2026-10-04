# `kernel/src/intent/gzip.rs` @ 5e0102684

## L1-24 · `use alloc::boxed::Box;`

```
//! gzip on the receive path (RFC 1952), streaming and capped.
//!
//! Measured across beak's target corpus: the same document arrives **4,1x to
//! 9,9x** smaller with `Accept-Encoding: gzip`
//! (`docs/plan/JS_SCOPE_CONTENT_WEB.md` §8). Until now the HTTP path neither
//! sent the header nor could inflate, so every page came uncompressed.
//!
//! Two things here are deliberate:
//!
//! * **Streaming, not buffer-then-unpack.** The receive path hands fragments
//!   to a sink; a staging buffer would hold the whole response a second time.
//! * **The cap is the same cap as without gzip.** At most as many bytes are
//!   inflated as the caller already named in `max_size`. A zip bomb is then no
//!   more dangerous than an uncompressed body of the size the caller said it
//!   could take, and it is clipped in the same place. That is the answer to
//!   the security checkpoint: we unpack foreign bytes, but into a pot whose
//!   size the far side does not decide.
//!
//! miniz_oxide knows zlib and raw DEFLATE, not the gzip framing — so the
//! header is read here and the rest is fed as `Raw`.
//!
//! The trailer's CRC32 is NOT verified, and that is a decision: the bytes came
//! through TLS, which already vouches for their integrity. A stream damaged
//! here means "the server is broken", not "someone turned it on the way".
```

## L31-33 · `const MAX_HEADER: usize = 4096;`

```
/// A gzip header is variable-length (file name, comment). No real server
/// needs more than this, and without a bound the far side could keep us busy
/// with an endless FNAME field.
```

## L36-37 · `const CHUNK: usize = 16 * 1024;`

```
/// Inflate buffer per round. Big enough that a 16 KB TLS record clears in a
/// few rounds, small enough for the kernel heap.
```

## L42 · `hdr: Vec<u8>,`

```
/// Header bytes, while the header is still incomplete.
```

## L46-48 · `fed: usize,`

```
/// Fed in raw and handed out inflated — for the trace only. Without these
/// two numbers a successful gzip run on the device looks exactly like no
/// run at all (`feedback_the_fast_path_must_say_it_ran`).
```

## L52 · `clipped: bool,`

```
/// Report once, not per fragment.
```

## L57 · `pub fn new(budget: usize) -> Self {`

```
/// `budget` = how many INFLATED bytes may pass through the sink at most.
```

## L71 · `pub fn ratio(&self) -> (usize, usize) {`

```
/// Raw bytes in, inflated bytes out — the two numbers for the trace.
```

## L87-90 · `let n = match header_len(&self.hdr)? {`

```
// The bound is for a header that does NOT END — not for the first
// delivery. Checking it before the parse rejected every response
// that arrived in one piece, which is nearly all of them: the
// bytes AFTER the header were counted too.
```

## L96 · `return Ok(()); // header still incomplete`

```
// header still incomplete
```

## L117-121 · `while !input.is_empty() {`

```
// Only while there is input. A call with EMPTY input reports
// `MZError::Buf` — "no progress possible", not "damaged". The first
// version read that as damage and dropped every response that arrived
// in more than one piece; in one piece it worked, which is the only
// reason it looked correct.
```

## L132-133 · `if !self.clipped {`

```
// Cap reached — exactly as an uncompressed body over
// `max_size` gets clipped.
```

## L151 · `Err(MZError::Buf) => return Ok(()), // needs more input`

```
// needs more input
```

## L155-156 · `if r.bytes_consumed == 0 && r.bytes_written == 0 {`

```
// No progress and nothing more to give: the next delivery
// continues. Without this guard the loop spins.
```

## L165 · `fn header_len(b: &[u8]) -> Result<Option<usize>, &'static str> {`

```
/// Length of the gzip header, or `None` while it is still incomplete.
```

## L179 · `if b.len() < i + 2 {`

```
// FEXTRA
```

## L187 · `if flg & bit != 0 {`

```
// FNAME, FCOMMENT — one NUL-terminated string each
```

## L202 · `i += 2; // FHCRC`

```
// FHCRC
```

## L210-213 · `pub fn inflate_all(body: &[u8], budget: usize) -> Result<Vec<u8>, &'static str> {`

```
/// Inflate a gzip body that is fully in hand.
///
/// The h2 path collects a stream's DATA frames into one Vec anyway — there is
/// nothing to stream there, and the same cap applies.
```

