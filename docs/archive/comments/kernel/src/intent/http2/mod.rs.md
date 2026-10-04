# `kernel/src/intent/http2/mod.rs` @ 5e0102684

## L1-18 · `pub mod hpack;`

```
//! HTTP/2 client (RFC 9113).
//!
//! Why this exists at all: measured 2026-07-22, Wikimedia's front end
//! throttles HTTP/1.1 clients — four images, then `429 Too Many Requests`
//! for everything after, at a sustainable rate of about half a request per
//! second. Over HTTP/2 the identical burst, from the same address with the
//! same headers, is served in full. Backing off on `Retry-After` does not
//! help (waiting past the advertised second still returns 429) and opening
//! more HTTP/1.1 connections makes it worse, because the limit counts per
//! address rather than per connection. So h2 is not a nicety here; it is how
//! a page full of sub-resources loads at all.
//!
//! It also happens to be the concurrency story: one connection carrying many
//! interleaved streams is what browsers do, and it replaces the ~8 serial
//! round-trips a page currently spends before its first paint.
//!
//! Scope: client only, GET only, no server push (we disable it), no
//! prioritisation (advisory anyway, and RFC 9113 deprecated the scheme).
```

## L32 · `const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";`

```
// ── Wire constants (RFC 9113 §4, §6, §11) ───────────────────────────────────
```

## L57-59 · `const MAX_FRAME: usize = 16_384;`

```
/// Frame payload ceiling we accept. The protocol default and minimum legal
/// value for `SETTINGS_MAX_FRAME_SIZE`; we neither send nor accept larger,
/// which bounds every per-frame allocation.
```

## L62-64 · `const WINDOW: u32 = 4 * 1024 * 1024;`

```
/// Per-stream and connection receive window we advertise. Large enough that
/// a page's images stream without us becoming the bottleneck, small enough to
/// bound what an unsolicited sender can park in our memory.
```

## L67-68 · `const WINDOW_REFILL_AT: u32 = WINDOW / 2;`

```
/// Refill a window once this much of it has been consumed, rather than after
/// every frame — one `WINDOW_UPDATE` per megabyte instead of per 16 KiB.
```

## L71-73 · `const READ_BUF: usize = 17 * 1024;`

```
/// A TLS record can carry 16 KiB; `tls_recv` copies at most `buf.len()` and
/// **drops the rest of the record**, so the read buffer must exceed the
/// largest record or we silently lose bytes.
```

## L76-82 · `const FILL_BUDGET: u64 = 1500; // 15 s`

```
/// Wie lange EIN `fill_to` insgesamt auf seine Bytes wartet, in Ticks (100 Hz).
///
/// Die aeusserste Schranke des h2-Lesewegs — und damit die einzige Zahl in
/// dieser Kette, die wirklich bindet. Darunter liegen `QUIET_TRANSFER` x
/// `ATTEMPT_TICKS` (bis 60 s) und `ATTEMPT_TICKS_REUSED` (1 s); `fill_to`
/// stutzt beide auf das, was hiervon uebrig ist. Wer eine der inneren Zahlen
/// anhebt, hebt damit NICHT diese hier an — das ist der Sinn der Rangfolge.
```

## L83 · `const FILL_BUDGET: u64 = 1500; // 15 s`

```
// 15 s
```

## L85-86 · `const MAX_BODY: usize = 24 * 1024 * 1024;`

```
/// Cap on one response body. Larger than any page asset we fetch; a peer
/// cannot make us buffer beyond it.
```

## L89-93 · `const MAX_HEADER_BLOCK: usize = 64 * 1024;`

```
/// Cap on one header block, assembled across HEADERS + CONTINUATION. Each
/// frame is bounded by `MAX_FRAME`, but the number of CONTINUATIONs is not —
/// without this a peer can grow one Vec until the kernel is out of memory.
/// Generous: this is the HPACK-compressed size, and the HTTP/1.1 path stops
/// at 32 KiB of plain text.
```

## L96-98 · `#[allow(dead_code)]`

```
// The `&str` payloads reach the log through the derived `Debug` (see the h2
// fallback in intent/http.rs). rustc's dead-code lint does not count derived
// impls as a read, so it flags them regardless.
```

## L102 · `NotNegotiated,`

```
/// The peer did not select `h2` via ALPN.
```

## L105 · `Protocol(&'static str),`

```
/// The peer broke the protocol; the connection is not reusable.
```

## L107 · `Closed,`

```
/// The peer closed or reset before the response completed.
```

## L109 · `TooLarge,`

```
/// A response exceeded `MAX_BODY`, or a frame exceeded `MAX_FRAME`.
```

## L128 · `struct FrameHeader {`

```
// ── Frames ──────────────────────────────────────────────────────────────────
```

## L151 · `put_u32(out, stream & 0x7FFF_FFFF);`

```
// The reserved high bit of the stream identifier is always sent as 0.
```

## L156 · `struct Stream {`

```
// ── Per-stream state ────────────────────────────────────────────────────────
```

## L160-162 · `block: Vec<u8>,`

```
/// Header block bytes accumulated across HEADERS + CONTINUATION. HPACK is
/// stateful per connection, so a block must be decoded whole and in
/// order — that is also why CONTINUATION may not be interleaved.
```

## L166-167 · `consumed: u32,`

```
/// Bytes counted against this stream's receive window since the last
/// refill.
```

## L171-172 · `head_seen: bool,`

```
/// The final (non-1xx) header block has been handed to a `BodySink`.
/// Trailers arrive as a second HEADERS and must not report a second time.
```

## L176 · `pub trait BodySink {`

```
// ── Where a response body goes ──────────────────────────────────────────────
```

## L178-183 · `pub trait BodySink {`

```
/// Receiver for a streamed response.
///
/// `head` runs once, before the first body byte, because the caller cannot
/// decode what follows without the headers: `Content-Encoding: gzip` decides
/// whether an inflater belongs between the wire and the sink, and a 3xx body
/// is courtesy text nobody may see — the redirect is followed instead.
```

## L189-191 · `enum Dest<'a> {`

```
/// `get_all` fetches a batch and hands back Vecs, so it buffers. The document
/// path hands every byte straight on — a page is the largest thing we fetch
/// and the one thing we must not hold a second time.
```

## L197 · `pub struct Http2 {`

```
// ── Connection ──────────────────────────────────────────────────────────────
```

## L202 · `rx: Vec<u8>,`

```
/// Undecoded bytes left over from the last read.
```

## L204-205 · `answered: bool,`

```
/// Ob seit dem letzten Senden auf dieser Verbindung ueberhaupt ein Byte
/// kam. Entscheidet, wie lange auf Daten gewartet wird — siehe `fill_to`.
```

## L207-209 · `pub reused: bool,`

```
/// Ob diese Verbindung aus dem Pool kam. Auf einer wiederverwendeten ist
/// das Schweigen der Gegenstelle wahrscheinlicher und der Neuaufbau
/// billig, also wird frueher aufgegeben.
```

## L214 · `goaway: bool,`

```
/// Set when the peer sends GOAWAY: finish what is in flight, start nothing.
```

## L216-217 · `expect_continuation: Option<u32>,`

```
/// A header block is being assembled; only CONTINUATION for this stream
/// is legal until it ends (§6.10).
```

## L219-221 · `conn_send_window: u32,`

```
/// What the peer still lets us send on the connection before it grants
/// more (§6.9). Starts at the protocol default and grows with every
/// connection-level WINDOW_UPDATE.
```

## L223-225 · `peer_initial_window: u32,`

```
/// The per-stream send window the peer announces. `request` refuses a
/// body that does not fit it rather than waiting for credit, so no send
/// ever blocks on a WINDOW_UPDATE.
```

## L230 · `pub fn start(tls: TlsSession) -> Result<Self, Http2Error> {`

```
/// Take over an already-established TLS session that negotiated `h2`.
```

## L241 · `next_id: 1, // client streams are odd (§5.1.1)`

```
// client streams are odd (§5.1.1)
```

## L260 · `(SETTINGS_ENABLE_PUSH, 0), // we never want PUSH_PROMISE`

```
// we never want PUSH_PROMISE
```

## L269-270 · `let mut inc = Vec::new();`

```
// SETTINGS_INITIAL_WINDOW_SIZE covers streams only; the connection
// window starts at 65535 regardless and must be raised explicitly.
```

## L282-293 · `pub fn get_all(`

```
/// Fetch several paths concurrently over this one connection.
///
/// This is the whole point of the module: the requests all go out before
/// any response is read, so the round-trips overlap instead of stacking.
/// Results come back positionally, one per requested path.
/// `accept_gzip` asks for the transfer compressed. The caller unpacks —
/// see `intent::gzip`. Measured 4,1x-9,9x fewer bytes per page on the
/// browser's target corpus (`docs/plan/JS_SCOPE_CONTENT_WEB.md` §8).
/// `cookies` ist POSITIONELL zu `paths`: Eintrag `i` ist die
/// `Cookie`-Kopfzeile fuer `paths[i]`, oder leer. Positionell und nicht
/// „einer je Host", weil ein Keks einen `Path` haben darf — zwei
/// Ressourcen desselben Hosts bekommen dann verschiedene.
```

## L321-324 · `if let Some(c) = cookies.get(pi) {`

```
// **Der Keks gehoert auch an die Unterressource.** Ohne ihn kam
// das Dokument angemeldet und jedes Bild darin anonym zurueck —
// auf einer Seite hinter einer Anmeldung sah das aus wie ein
// Bildfehler und war keiner.
```

## L329-331 · `if block.len() > self.peer_max_frame {`

```
// A block longer than one frame would need CONTINUATION on send.
// Request headers are far below that, so treat it as a bug rather
// than growing an encoder path nothing exercises.
```

## L338 · `FLAG_END_HEADERS | FLAG_END_STREAM, // GET has no body`

```
// GET has no body
```

## L353-356 · `self.answered = false;`

```
// Neuer Austausch: bis zur ersten Antwort gilt die kurze Geduld. Eine
// Verbindung aus dem Pool HAT frueher geantwortet — ohne dieses
// Zuruecksetzen greift die Unterscheidung genau im Fall nicht, fuer
// den sie da ist.
```

## L379-389 · `pub fn request(`

```
/// One request over this connection, streamed.
///
/// The document path's shape, as opposed to `get_all`'s batch: one
/// stream, any method, a body if there is one, and DATA handed to `sink`
/// as it arrives. It exists because the document fetch was the last
/// caller still on HTTP/1.1 — and therefore the only one Wikimedia still
/// throttles (§8.1). Redirects are NOT followed here: the host may
/// change, so that decision stays one layer up, with the caller that
/// already owns the method switch and the per-hop headers.
///
/// Returns the response's header fields; the status is in `:status`.
```

## L404-408 · `let room = self.conn_send_window.min(self.peer_initial_window) as usize;`

```
// Send flow control, decided before a byte goes out: a body larger
// than the credit we already hold would have to wait for a
// WINDOW_UPDATE mid-send, and this connection has no way to read one
// while writing. Refuse it instead and let the caller use HTTP/1.1 —
// a form post is a few hundred bytes against a 64 KiB default.
```

## L414-415 · `let mut owned: Vec<(String, String)> = Vec::new();`

```
// Caller headers, lowercased (§8.2.1 — an uppercase name makes the
// message malformed).
```

## L420-424 · `if matches!(name.as_str(),`

```
// §8.2.2: connection-specific fields have no meaning in h2 and
// make the message malformed. Most are already refused at the
// sandbox boundary, but `keep-alive`, `upgrade`, `te` and
// `proxy-connection` are not on that list — they are dropped
// here, where the reason is the protocol rather than the guest.
```

## L446-447 · `content_length = alloc::format!("{}", body.len());`

```
// We state the length ourselves, never from a caller header —
// the same rule the HTTP/1.1 path keeps, for the same reason.
```

## L472 · `self.answered = false; // wie in get_all`

```
// wie in `get_all`
```

## L494 · `fn pump(&mut self, streams: &mut [Stream], dest: &mut Dest<'_>) -> Result<(), Http2Error> {`

```
/// Read frames until every stream has ended.
```

## L500 · `for s in streams.iter_mut().filter(|s| !s.done) {`

```
// Peer hung up with streams still open.
```

## L526-527 · `if let Some(expect) = self.expect_continuation {`

```
// §6.10: once a header block starts, nothing but its CONTINUATION may
// appear. Enforcing it keeps the HPACK stream in step.
```

## L552-553 · `let last = payload.get(..4).map(be32).unwrap_or(0) & 0x7FFF_FFFF;`

```
// Streams above the peer's last-processed id were never acted
// on; the rest may still complete.
```

## L560-563 · `if hdr.stream == 0 {`

```
// Credit for OUR send direction. Only the connection-level
// grant is banked: `request` refuses a body that does not fit
// the window it already holds, so a per-stream grant always
// arrives too late to change a decision.
```

## L575 · `return Err(Http2Error::Protocol("PUSH_PROMISE despite ENABLE_PUSH=0"));`

```
// We set ENABLE_PUSH = 0, so this is a protocol error (§8.4).
```

## L584 · `_ => {} // unknown frame types must be ignored (§4.1)`

```
// unknown frame types must be ignored (§4.1)
```

## L603-604 · `let end_headers = hdr.flags & FLAG_END_HEADERS != 0;`

```
// Decode even for an unknown stream: HPACK is connection-stateful, so
// skipping a block would desynchronise every later one.
```

## L624-626 · `return Err(Http2Error::Protocol("header block for unknown stream"));`

```
// Still must decode later; stash on no stream is not
// possible, so treat an unknown multi-frame block as
// fatal rather than desynchronise HPACK.
```

## L641-643 · `let status = decoded`

```
// This block's OWN status, not the first one on the stream: a 1xx
// is informational and the response we are here for is still
// coming, so the sink must not be told it has arrived.
```

## L650-656 · `if (100..200).contains(&status) {`

```
// A 1xx is informational and the answer is still coming. Drop
// it: keeping its fields would leave TWO `:status` on the
// stream, and a reader that takes the first one reads 103
// Early Hints as the response — which is what a CDN sends
// before the document, over h2 far more often than over
// HTTP/1.1. A client that does not use the hints is required
// to be able to ignore them (RFC 9110 §15.2).
```

## L660-661 · `s.headers.extend(decoded);`

```
// A second HEADERS after that is trailers; later fields
// append.
```

## L686-687 · `let counted = payload.len() as u32;`

```
// The whole padded length counts against flow control, even the part
// we discard (§6.1).
```

## L703-705 · `Dest::Sink(sink) => {`

```
// Straight through. MAX_BODY is the buffered path's rule
// because it is the buffered path that holds the bytes; a
// sink states its own cap and clips there.
```

## L728-729 · `let mut out = Vec::new();`

```
// Hand the credit back, or the peer stops sending once the window
// is spent — a stall that looks exactly like a hung server.
```

## L755-756 · `if value > 0x7FFF_FFFF {`

```
// §6.5.2: above 2^31-1 is a connection error. It bounds what
// we may send on a stream, which used to be nothing at all.
```

## L766-767 · `self.peer_max_frame = (value as usize).min(MAX_FRAME);`

```
// We never send a frame bigger than the default anyway; the
// cap matters only so our own HEADERS check is honest.
```

## L774 · `fn next_frame_header(&mut self) -> Result<Option<FrameHeader>, Http2Error> {`

```
// ── Byte plumbing ───────────────────────────────────────────────────────
```

## L800-807 · `fn fill_to(&mut self, n: usize) -> Result<bool, Http2Error> {`

```
/// Read until `self.rx` holds at least `n` bytes. False means the peer
/// closed first.
///
/// `poll_rx_only`, NOT `poll`: the full poll also runs a shade render
/// pass, and calling that once per idle turn of a receive loop costs far
/// more than the receive itself — the HTTP/1.1 path learned this the hard
/// way (see the note on `tls_recv_poll`). Timeout is measured in ticks
/// rather than iterations so it means 15 seconds on any machine.
```

## L813-821 · `let spent = crate::interrupts::ticks().wrapping_sub(start);`

```
// **Die aeussere Schranke ist die Autoritaet, und sie wird HIER
// geprueft, nicht nur im `Ok(0)`-Zweig.**
//
// Vorher stand sie dort unten und war Dekoration: ein einziger
// `tls_recv_patient(QUIET_TRANSFER=6, ATTEMPT_TICKS=1000)` wartet
// bis zu 60 s IN SICH, und die 15 s daueber konnten erst danach
// zuschlagen. Eine Schranke, die groesser ist als die Geduld, die
// sie begrenzen soll, begrenzt nichts
// ([[feedback-threshold-without-a-comparison]]).
```

## L827-832 · `let (patience, attempt) = if self.answered {`

```
// Solange auf DIESER Verbindung seit dem Senden noch nichts kam,
// ist die kurze Geduld richtig: eine aus dem Pool genommene
// Verbindung, die der Server inzwischen geschlossen hat, sieht
// lokal lebendig aus (siehe `PooledConn` in `intent/http.rs`) und
// hat einen Bildabruf 60 s gekostet. Sobald das erste Byte da ist,
// gilt wieder die volle Nachsicht fuer stockende Uebertragungen.
```

## L840-842 · `let attempt = attempt.min(left);`

```
// `tls_recv_patient` wartet bis zu `patience * attempt` Ticks. Beides
// wird auf das gestutzt, was vom Budget uebrig ist — sonst kaeme der
// Rueckweg erst, wenn die INNERE Geduld erschoepft ist.
```

## L847-848 · `core::hint::spin_loop();`

```
// Either a record carrying no application data (session
// tickets arrive this way) or nothing ready yet.
```

## L869-870 · `pub fn peer(&self) -> ([u8; 4], u16) {`

```
/// Adresse und Port der Gegenstelle — die erste Haelfte der
/// Coalescing-Bedingung (RFC 7540 §9.1.1).
```

## L875-877 · `pub fn covers(&self, host: &str) -> bool {`

```
/// Deckt das Zertifikat dieser Verbindung auch `host`? Die zweite Haelfte.
/// Ein GOAWAY schliesst sie aus: eine Verbindung, die keine neuen Streams
/// mehr annimmt, ist fuer einen zweiten Namen erst recht nichts.
```

## L883 · `fn find<'a>(streams: &'a mut [Stream], id: u32) -> Option<&'a mut Stream> {`

```
// ── Helpers ─────────────────────────────────────────────────────────────────
```

## L893 · `fn strip_data_padding(flags: u8, payload: &[u8]) -> Option<&[u8]> {`

```
/// Strip DATA padding (§6.1). `None` if the pad length overruns the payload.
```

## L903 · `fn strip_headers_padding(flags: u8, payload: &[u8]) -> Option<&[u8]> {`

```
/// Strip HEADERS padding and the deprecated priority block (§6.2).
```

## L912 · `body = body.get(5..)?; // 4-byte dependency + 1-byte weight`

```
// 4-byte dependency + 1-byte weight
```

## L917-919 · `pub fn connect(host: &str, ip: [u8; 4], port: u16) -> Result<Http2, Http2Error> {`

```
/// Open a TLS session offering h2, then start a connection on it. Falls back
/// to `Err(NotNegotiated)` — never silently to HTTP/1.1 — so the caller can
/// decide explicitly.
```

## L937-941 · `let t_start = crate::interrupts::ticks();`

```
// Which leg is slow? The connect swings between ~200 ms and ~2100 ms
// across runs; 2 s is about a retransmission timeout, so name the leg.
// tcp+tls came to 90 ms while the caller measured 2100 for the same
// connect, so the preface — the first application write after the
// handshake — is timed too rather than left as the unnamed remainder.
```

## L965-966 · `#[allow(dead_code)]`

```
/// Unused today but part of the response contract; keeps `String` imported
/// where the body decoder will need it.
```

