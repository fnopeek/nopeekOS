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

pub mod hpack;
mod tables;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::kprintln;
use crate::tls::TlsSession;

pub use hpack::Header;

// ── Wire constants (RFC 9113 §4, §6, §11) ───────────────────────────────────

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

const FRAME_DATA: u8 = 0x0;
const FRAME_HEADERS: u8 = 0x1;
const FRAME_RST_STREAM: u8 = 0x3;
const FRAME_SETTINGS: u8 = 0x4;
const FRAME_PUSH_PROMISE: u8 = 0x5;
const FRAME_PING: u8 = 0x6;
const FRAME_GOAWAY: u8 = 0x7;
const FRAME_WINDOW_UPDATE: u8 = 0x8;
const FRAME_CONTINUATION: u8 = 0x9;

const FLAG_END_STREAM: u8 = 0x1;
const FLAG_ACK: u8 = 0x1;
const FLAG_END_HEADERS: u8 = 0x4;
const FLAG_PADDED: u8 = 0x8;
const FLAG_PRIORITY: u8 = 0x20;

const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
const SETTINGS_ENABLE_PUSH: u16 = 0x2;
const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;

/// Frame payload ceiling we accept. The protocol default and minimum legal
/// value for `SETTINGS_MAX_FRAME_SIZE`; we neither send nor accept larger,
/// which bounds every per-frame allocation.
const MAX_FRAME: usize = 16_384;

/// Per-stream and connection receive window we advertise. Large enough that
/// a page's images stream without us becoming the bottleneck, small enough to
/// bound what an unsolicited sender can park in our memory.
const WINDOW: u32 = 4 * 1024 * 1024;

/// Refill a window once this much of it has been consumed, rather than after
/// every frame — one `WINDOW_UPDATE` per megabyte instead of per 16 KiB.
const WINDOW_REFILL_AT: u32 = WINDOW / 2;

/// A TLS record can carry 16 KiB; `tls_recv` copies at most `buf.len()` and
/// **drops the rest of the record**, so the read buffer must exceed the
/// largest record or we silently lose bytes.
const READ_BUF: usize = 17 * 1024;

/// Cap on one response body. Larger than any page asset we fetch; a peer
/// cannot make us buffer beyond it.
const MAX_BODY: usize = 24 * 1024 * 1024;

/// Cap on one header block, assembled across HEADERS + CONTINUATION. Each
/// frame is bounded by `MAX_FRAME`, but the number of CONTINUATIONs is not —
/// without this a peer can grow one Vec until the kernel is out of memory.
/// Generous: this is the HPACK-compressed size, and the HTTP/1.1 path stops
/// at 32 KiB of plain text.
const MAX_HEADER_BLOCK: usize = 64 * 1024;

// The `&str` payloads reach the log through the derived `Debug` (see the h2
// fallback in intent/http.rs). rustc's dead-code lint does not count derived
// impls as a read, so it flags them regardless.
#[allow(dead_code)]
#[derive(Debug)]
pub enum Http2Error {
    /// The peer did not select `h2` via ALPN.
    NotNegotiated,
    Tls(&'static str),
    /// The peer broke the protocol; the connection is not reusable.
    Protocol(&'static str),
    /// The peer closed or reset before the response completed.
    Closed,
    /// A response exceeded `MAX_BODY`, or a frame exceeded `MAX_FRAME`.
    TooLarge,
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name == name)
            .map(|h| h.value.as_str())
    }
}

// ── Frames ──────────────────────────────────────────────────────────────────

struct FrameHeader {
    len: usize,
    kind: u8,
    flags: u8,
    stream: u32,
}

fn put_u24(out: &mut Vec<u8>, v: usize) {
    out.push((v >> 16) as u8);
    out.push((v >> 8) as u8);
    out.push(v as u8);
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn frame(out: &mut Vec<u8>, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
    put_u24(out, payload.len());
    out.push(kind);
    out.push(flags);
    // The reserved high bit of the stream identifier is always sent as 0.
    put_u32(out, stream & 0x7FFF_FFFF);
    out.extend_from_slice(payload);
}

// ── Per-stream state ────────────────────────────────────────────────────────

struct Stream {
    id: u32,
    /// Header block bytes accumulated across HEADERS + CONTINUATION. HPACK is
    /// stateful per connection, so a block must be decoded whole and in
    /// order — that is also why CONTINUATION may not be interleaved.
    block: Vec<u8>,
    headers: Vec<Header>,
    body: Vec<u8>,
    /// Bytes counted against this stream's receive window since the last
    /// refill.
    consumed: u32,
    done: bool,
    failed: Option<&'static str>,
    /// The final (non-1xx) header block has been handed to a `BodySink`.
    /// Trailers arrive as a second HEADERS and must not report a second time.
    head_seen: bool,
}

// ── Where a response body goes ──────────────────────────────────────────────

/// Receiver for a streamed response.
///
/// `head` runs once, before the first body byte, because the caller cannot
/// decode what follows without the headers: `Content-Encoding: gzip` decides
/// whether an inflater belongs between the wire and the sink, and a 3xx body
/// is courtesy text nobody may see — the redirect is followed instead.
pub trait BodySink {
    fn head(&mut self, status: u16, headers: &[Header]) -> Result<(), &'static str>;
    fn data(&mut self, chunk: &[u8]) -> Result<(), &'static str>;
}

/// `get_all` fetches a batch and hands back Vecs, so it buffers. The document
/// path hands every byte straight on — a page is the largest thing we fetch
/// and the one thing we must not hold a second time.
enum Dest<'a> {
    Buffer,
    Sink(&'a mut dyn BodySink),
}

// ── Connection ──────────────────────────────────────────────────────────────

pub struct Http2 {
    tls: TlsSession,
    dec: hpack::Decoder,
    /// Undecoded bytes left over from the last read.
    rx: Vec<u8>,
    /// Ob seit dem letzten Senden auf dieser Verbindung ueberhaupt ein Byte
    /// kam. Entscheidet, wie lange auf Daten gewartet wird — siehe `fill_to`.
    answered: bool,
    /// Ob diese Verbindung aus dem Pool kam. Auf einer wiederverwendeten ist
    /// das Schweigen der Gegenstelle wahrscheinlicher und der Neuaufbau
    /// billig, also wird frueher aufgegeben.
    pub reused: bool,
    next_id: u32,
    peer_max_frame: usize,
    conn_consumed: u32,
    /// Set when the peer sends GOAWAY: finish what is in flight, start nothing.
    goaway: bool,
    /// A header block is being assembled; only CONTINUATION for this stream
    /// is legal until it ends (§6.10).
    expect_continuation: Option<u32>,
    /// What the peer still lets us send on the connection before it grants
    /// more (§6.9). Starts at the protocol default and grows with every
    /// connection-level WINDOW_UPDATE.
    conn_send_window: u32,
    /// The per-stream send window the peer announces. `request` refuses a
    /// body that does not fit it rather than waiting for credit, so no send
    /// ever blocks on a WINDOW_UPDATE.
    peer_initial_window: u32,
}

impl Http2 {
    /// Take over an already-established TLS session that negotiated `h2`.
    pub fn start(tls: TlsSession) -> Result<Self, Http2Error> {
        if tls.alpn() != Some("h2") {
            return Err(Http2Error::NotNegotiated);
        }
        let mut c = Self {
            tls,
            dec: hpack::Decoder::new(),
            rx: Vec::new(),
            answered: false,
            reused: false,
            next_id: 1, // client streams are odd (§5.1.1)
            peer_max_frame: MAX_FRAME,
            conn_consumed: 0,
            goaway: false,
            expect_continuation: None,
            conn_send_window: 65_535,
            peer_initial_window: 65_535,
        };
        c.send_preface()?;
        Ok(c)
    }

    fn send_preface(&mut self) -> Result<(), Http2Error> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(PREFACE);

        let mut settings = Vec::new();
        for (id, value) in [
            (SETTINGS_HEADER_TABLE_SIZE, hpack::MAX_TABLE_SIZE as u32),
            (SETTINGS_ENABLE_PUSH, 0), // we never want PUSH_PROMISE
            (SETTINGS_INITIAL_WINDOW_SIZE, WINDOW),
            (SETTINGS_MAX_FRAME_SIZE, MAX_FRAME as u32),
        ] {
            settings.extend_from_slice(&id.to_be_bytes());
            put_u32(&mut settings, value);
        }
        frame(&mut out, FRAME_SETTINGS, 0, 0, &settings);

        // SETTINGS_INITIAL_WINDOW_SIZE covers streams only; the connection
        // window starts at 65535 regardless and must be raised explicitly.
        let mut inc = Vec::new();
        put_u32(&mut inc, WINDOW - 65_535);
        frame(&mut out, FRAME_WINDOW_UPDATE, 0, 0, &inc);

        self.write(&out)
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Http2Error> {
        crate::tls::tls_send(&mut self.tls, bytes).map_err(|_| Http2Error::Tls("send failed"))
    }

    /// Fetch several paths concurrently over this one connection.
    ///
    /// This is the whole point of the module: the requests all go out before
    /// any response is read, so the round-trips overlap instead of stacking.
    /// Results come back positionally, one per requested path.
    /// `accept_gzip` asks for the transfer compressed. The caller unpacks —
    /// see `intent::gzip`. Measured 4,1x-9,9x fewer bytes per page on the
    /// browser's target corpus (`docs/plan/JS_SCOPE_CONTENT_WEB.md` §8).
    pub fn get_all(
        &mut self,
        authority: &str,
        paths: &[&str],
        user_agent: &str,
        accept_gzip: bool,
    ) -> Result<Vec<Result<Response, Http2Error>>, Http2Error> {
        if self.goaway {
            return Err(Http2Error::Closed);
        }
        let mut streams: Vec<Stream> = Vec::with_capacity(paths.len());
        let mut out = Vec::new();
        for path in paths {
            let id = self.next_id;
            self.next_id += 2;
            let mut fields: Vec<(&str, &str)> = alloc::vec![
                (":method", "GET"),
                (":scheme", "https"),
                (":authority", authority),
                (":path", path),
                ("user-agent", user_agent),
                ("accept", "*/*"),
            ];
            if accept_gzip {
                fields.push(("accept-encoding", "gzip"));
            }
            let block = hpack::encode(&fields);
            // A block longer than one frame would need CONTINUATION on send.
            // Request headers are far below that, so treat it as a bug rather
            // than growing an encoder path nothing exercises.
            if block.len() > self.peer_max_frame {
                return Err(Http2Error::TooLarge);
            }
            frame(
                &mut out,
                FRAME_HEADERS,
                FLAG_END_HEADERS | FLAG_END_STREAM, // GET has no body
                id,
                &block,
            );
            streams.push(Stream {
                id,
                block: Vec::new(),
                headers: Vec::new(),
                body: Vec::new(),
                consumed: 0,
                done: false,
                failed: None,
                head_seen: false,
            });
        }
        // Neuer Austausch: bis zur ersten Antwort gilt die kurze Geduld. Eine
        // Verbindung aus dem Pool HAT frueher geantwortet — ohne dieses
        // Zuruecksetzen greift die Unterscheidung genau im Fall nicht, fuer
        // den sie da ist.
        self.answered = false;
        self.write(&out)?;
        self.pump(&mut streams, &mut Dest::Buffer)?;

        Ok(streams
            .into_iter()
            .map(|s| match s.failed {
                Some(e) => Err(Http2Error::Protocol(e)),
                None if !s.done => Err(Http2Error::Closed),
                None => {
                    let status = s
                        .headers
                        .iter()
                        .find(|h| h.name == ":status")
                        .and_then(|h| h.value.parse::<u16>().ok())
                        .unwrap_or(0);
                    Ok(Response { status, headers: s.headers, body: s.body })
                }
            })
            .collect())
    }

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
    pub fn request(
        &mut self,
        authority: &str,
        method: &str,
        path: &str,
        extra: &[String],
        body: &[u8],
        user_agent: &str,
        accept_gzip: bool,
        sink: &mut dyn BodySink,
    ) -> Result<Vec<Header>, Http2Error> {
        if self.goaway {
            return Err(Http2Error::Closed);
        }
        // Send flow control, decided before a byte goes out: a body larger
        // than the credit we already hold would have to wait for a
        // WINDOW_UPDATE mid-send, and this connection has no way to read one
        // while writing. Refuse it instead and let the caller use HTTP/1.1 —
        // a form post is a few hundred bytes against a 64 KiB default.
        let room = self.conn_send_window.min(self.peer_initial_window) as usize;
        if body.len() > room {
            return Err(Http2Error::TooLarge);
        }

        // Caller headers, lowercased (§8.2.1 — an uppercase name makes the
        // message malformed).
        let mut owned: Vec<(String, String)> = Vec::new();
        for line in extra {
            let Some((name, value)) = line.split_once(':') else { continue };
            let name = name.trim().to_ascii_lowercase();
            // §8.2.2: connection-specific fields have no meaning in h2 and
            // make the message malformed. Most are already refused at the
            // sandbox boundary, but `keep-alive`, `upgrade`, `te` and
            // `proxy-connection` are not on that list — they are dropped
            // here, where the reason is the protocol rather than the guest.
            if matches!(name.as_str(),
                "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding"
                | "upgrade" | "te" | "host" | "content-length" | "accept-encoding") {
                continue;
            }
            owned.push((name, String::from(value.trim())));
        }

        let content_length;
        let mut fields: Vec<(&str, &str)> = alloc::vec![
            (":method", method),
            (":scheme", "https"),
            (":authority", authority),
            (":path", path),
            ("user-agent", user_agent),
            ("accept", "*/*"),
        ];
        if accept_gzip {
            fields.push(("accept-encoding", "gzip"));
        }
        if !body.is_empty() {
            // We state the length ourselves, never from a caller header —
            // the same rule the HTTP/1.1 path keeps, for the same reason.
            content_length = alloc::format!("{}", body.len());
            fields.push(("content-length", &content_length));
        }
        for (name, value) in &owned {
            fields.push((name.as_str(), value.as_str()));
        }

        let block = hpack::encode(&fields);
        if block.len() > self.peer_max_frame {
            return Err(Http2Error::TooLarge);
        }
        let id = self.next_id;
        self.next_id += 2;

        let mut out = Vec::new();
        let end = if body.is_empty() { FLAG_END_STREAM } else { 0 };
        frame(&mut out, FRAME_HEADERS, FLAG_END_HEADERS | end, id, &block);
        let mut sent = 0usize;
        for chunk in body.chunks(self.peer_max_frame) {
            sent += chunk.len();
            let last = if sent == body.len() { FLAG_END_STREAM } else { 0 };
            frame(&mut out, FRAME_DATA, last, id, chunk);
        }
        self.conn_send_window -= body.len() as u32;
        self.answered = false; // wie in `get_all`
        self.write(&out)?;

        let mut streams = alloc::vec![Stream {
            id,
            block: Vec::new(),
            headers: Vec::new(),
            body: Vec::new(),
            consumed: 0,
            done: false,
            failed: None,
            head_seen: false,
        }];
        self.pump(&mut streams, &mut Dest::Sink(sink))?;
        let s = streams.pop().expect("one stream in, one stream out");
        match s.failed {
            Some(e) => Err(Http2Error::Protocol(e)),
            None if !s.done => Err(Http2Error::Closed),
            None => Ok(s.headers),
        }
    }

    /// Read frames until every stream has ended.
    fn pump(&mut self, streams: &mut [Stream], dest: &mut Dest<'_>) -> Result<(), Http2Error> {
        while streams.iter().any(|s| !s.done && s.failed.is_none()) {
            let hdr = match self.next_frame_header()? {
                Some(h) => h,
                None => {
                    // Peer hung up with streams still open.
                    for s in streams.iter_mut().filter(|s| !s.done) {
                        s.failed.get_or_insert("connection closed mid-response");
                    }
                    return Ok(());
                }
            };
            if hdr.len > MAX_FRAME {
                return Err(Http2Error::Protocol("frame exceeds our max size"));
            }
            let payload = self.read_exact(hdr.len)?;
            self.dispatch(&hdr, &payload, streams, dest)?;
            if self.goaway && streams.iter().all(|s| s.done || s.failed.is_some()) {
                break;
            }
        }
        Ok(())
    }

    fn dispatch(
        &mut self,
        hdr: &FrameHeader,
        payload: &[u8],
        streams: &mut [Stream],
        dest: &mut Dest<'_>,
    ) -> Result<(), Http2Error> {
        // §6.10: once a header block starts, nothing but its CONTINUATION may
        // appear. Enforcing it keeps the HPACK stream in step.
        if let Some(expect) = self.expect_continuation {
            if hdr.kind != FRAME_CONTINUATION || hdr.stream != expect {
                return Err(Http2Error::Protocol("interleaved header block"));
            }
        }

        match hdr.kind {
            FRAME_SETTINGS => {
                if hdr.flags & FLAG_ACK == 0 {
                    self.apply_settings(payload)?;
                    let mut ack = Vec::new();
                    frame(&mut ack, FRAME_SETTINGS, FLAG_ACK, 0, &[]);
                    self.write(&ack)?;
                }
            }
            FRAME_PING => {
                if hdr.flags & FLAG_ACK == 0 {
                    let mut pong = Vec::new();
                    frame(&mut pong, FRAME_PING, FLAG_ACK, 0, payload);
                    self.write(&pong)?;
                }
            }
            FRAME_GOAWAY => {
                self.goaway = true;
                // Streams above the peer's last-processed id were never acted
                // on; the rest may still complete.
                let last = payload.get(..4).map(be32).unwrap_or(0) & 0x7FFF_FFFF;
                for s in streams.iter_mut().filter(|s| s.id > last && !s.done) {
                    s.failed.get_or_insert("refused by GOAWAY");
                }
            }
            FRAME_WINDOW_UPDATE => {
                // Credit for OUR send direction. Only the connection-level
                // grant is banked: `request` refuses a body that does not fit
                // the window it already holds, so a per-stream grant always
                // arrives too late to change a decision.
                if hdr.stream == 0 {
                    let inc = payload.get(..4).map(be32).unwrap_or(0) & 0x7FFF_FFFF;
                    self.conn_send_window = self.conn_send_window.saturating_add(inc);
                }
            }
            FRAME_RST_STREAM => {
                if let Some(s) = find(streams, hdr.stream) {
                    s.failed.get_or_insert("stream reset by peer");
                }
            }
            FRAME_PUSH_PROMISE => {
                // We set ENABLE_PUSH = 0, so this is a protocol error (§8.4).
                return Err(Http2Error::Protocol("PUSH_PROMISE despite ENABLE_PUSH=0"));
            }
            FRAME_HEADERS | FRAME_CONTINUATION => {
                self.on_headers(hdr, payload, streams, dest)?;
            }
            FRAME_DATA => {
                self.on_data(hdr, payload, streams, dest)?;
            }
            _ => {} // unknown frame types must be ignored (§4.1)
        }
        Ok(())
    }

    fn on_headers(
        &mut self,
        hdr: &FrameHeader,
        payload: &[u8],
        streams: &mut [Stream],
        dest: &mut Dest<'_>,
    ) -> Result<(), Http2Error> {
        let body = if hdr.kind == FRAME_HEADERS {
            strip_headers_padding(hdr.flags, payload)
                .ok_or(Http2Error::Protocol("bad HEADERS padding"))?
        } else {
            payload
        };

        // Decode even for an unknown stream: HPACK is connection-stateful, so
        // skipping a block would desynchronise every later one.
        let end_headers = hdr.flags & FLAG_END_HEADERS != 0;
        let end_stream = hdr.flags & FLAG_END_STREAM != 0;

        let (assembled, target) = {
            match find(streams, hdr.stream) {
                Some(s) => {
                    s.block.extend_from_slice(body);
                    if s.block.len() > MAX_HEADER_BLOCK {
                        return Err(Http2Error::TooLarge);
                    }
                    if !end_headers {
                        self.expect_continuation = Some(hdr.stream);
                        return Ok(());
                    }
                    (core::mem::take(&mut s.block), Some(hdr.stream))
                }
                None => {
                    if !end_headers {
                        self.expect_continuation = Some(hdr.stream);
                        // Still must decode later; stash on no stream is not
                        // possible, so treat an unknown multi-frame block as
                        // fatal rather than desynchronise HPACK.
                        return Err(Http2Error::Protocol("header block for unknown stream"));
                    }
                    (body.to_vec(), None)
                }
            }
        };
        self.expect_continuation = None;

        let decoded = self
            .dec
            .decode(&assembled)
            .map_err(|_| Http2Error::Protocol("HPACK decode failed"))?;

        if let Some(id) = target {
            // This block's OWN status, not the first one on the stream: a 1xx
            // is informational and the response we are here for is still
            // coming, so the sink must not be told it has arrived.
            let status = decoded
                .iter()
                .find(|h| h.name == ":status")
                .and_then(|h| h.value.parse::<u16>().ok())
                .unwrap_or(0);
            if let Some(s) = find(streams, id) {
                // A 1xx is informational and the answer is still coming. Drop
                // it: keeping its fields would leave TWO `:status` on the
                // stream, and a reader that takes the first one reads 103
                // Early Hints as the response — which is what a CDN sends
                // before the document, over h2 far more often than over
                // HTTP/1.1. A client that does not use the hints is required
                // to be able to ignore them (RFC 9110 §15.2).
                if (100..200).contains(&status) {
                    return Ok(());
                }
                // A second HEADERS after that is trailers; later fields
                // append.
                s.headers.extend(decoded);
                if end_stream {
                    s.done = true;
                }
                if let Dest::Sink(sink) = dest {
                    if status >= 200 && !s.head_seen {
                        s.head_seen = true;
                        if let Err(e) = sink.head(status, &s.headers) {
                            s.failed.get_or_insert(e);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn on_data(
        &mut self,
        hdr: &FrameHeader,
        payload: &[u8],
        streams: &mut [Stream],
        dest: &mut Dest<'_>,
    ) -> Result<(), Http2Error> {
        // The whole padded length counts against flow control, even the part
        // we discard (§6.1).
        let counted = payload.len() as u32;
        let data = strip_data_padding(hdr.flags, payload)
            .ok_or(Http2Error::Protocol("bad DATA padding"))?;

        let end_stream = hdr.flags & FLAG_END_STREAM != 0;
        let mut refill_stream = None;
        if let Some(s) = find(streams, hdr.stream) {
            match dest {
                Dest::Buffer => {
                    if s.body.len() + data.len() > MAX_BODY {
                        s.failed.get_or_insert("response body too large");
                    } else {
                        s.body.extend_from_slice(data);
                    }
                }
                // Straight through. MAX_BODY is the buffered path's rule
                // because it is the buffered path that holds the bytes; a
                // sink states its own cap and clips there.
                Dest::Sink(sink) => {
                    if let Err(e) = sink.data(data) {
                        s.failed.get_or_insert(e);
                    }
                }
            }
            s.consumed += counted;
            if s.consumed >= WINDOW_REFILL_AT && !end_stream {
                refill_stream = Some((s.id, core::mem::take(&mut s.consumed)));
            }
            if end_stream {
                s.done = true;
            }
        }

        self.conn_consumed += counted;
        let refill_conn = if self.conn_consumed >= WINDOW_REFILL_AT {
            Some(core::mem::take(&mut self.conn_consumed))
        } else {
            None
        };

        // Hand the credit back, or the peer stops sending once the window
        // is spent — a stall that looks exactly like a hung server.
        let mut out = Vec::new();
        if let Some(n) = refill_conn {
            let mut inc = Vec::new();
            put_u32(&mut inc, n);
            frame(&mut out, FRAME_WINDOW_UPDATE, 0, 0, &inc);
        }
        if let Some((id, n)) = refill_stream {
            let mut inc = Vec::new();
            put_u32(&mut inc, n);
            frame(&mut out, FRAME_WINDOW_UPDATE, 0, id, &inc);
        }
        if !out.is_empty() {
            self.write(&out)?;
        }
        Ok(())
    }

    fn apply_settings(&mut self, payload: &[u8]) -> Result<(), Http2Error> {
        if payload.len() % 6 != 0 {
            return Err(Http2Error::Protocol("SETTINGS length not a multiple of 6"));
        }
        for chunk in payload.chunks_exact(6) {
            let id = u16::from_be_bytes([chunk[0], chunk[1]]);
            let value = be32(&chunk[2..6]);
            if id == SETTINGS_INITIAL_WINDOW_SIZE {
                // §6.5.2: above 2^31-1 is a connection error. It bounds what
                // we may send on a stream, which used to be nothing at all.
                if value > 0x7FFF_FFFF {
                    return Err(Http2Error::Protocol("illegal INITIAL_WINDOW_SIZE"));
                }
                self.peer_initial_window = value;
            }
            if id == SETTINGS_MAX_FRAME_SIZE {
                if !(16_384..=16_777_215).contains(&value) {
                    return Err(Http2Error::Protocol("illegal MAX_FRAME_SIZE"));
                }
                // We never send a frame bigger than the default anyway; the
                // cap matters only so our own HEADERS check is honest.
                self.peer_max_frame = (value as usize).min(MAX_FRAME);
            }
        }
        Ok(())
    }

    // ── Byte plumbing ───────────────────────────────────────────────────────

    fn next_frame_header(&mut self) -> Result<Option<FrameHeader>, Http2Error> {
        if !self.fill_to(9)? {
            return Ok(None);
        }
        let h = &self.rx[..9];
        let hdr = FrameHeader {
            len: ((h[0] as usize) << 16) | ((h[1] as usize) << 8) | h[2] as usize,
            kind: h[3],
            flags: h[4],
            stream: be32(&h[5..9]) & 0x7FFF_FFFF,
        };
        self.rx.drain(..9);
        Ok(Some(hdr))
    }

    fn read_exact(&mut self, n: usize) -> Result<Vec<u8>, Http2Error> {
        if !self.fill_to(n)? {
            return Err(Http2Error::Closed);
        }
        let out = self.rx[..n].to_vec();
        self.rx.drain(..n);
        Ok(out)
    }

    /// Read until `self.rx` holds at least `n` bytes. False means the peer
    /// closed first.
    ///
    /// `poll_rx_only`, NOT `poll`: the full poll also runs a shade render
    /// pass, and calling that once per idle turn of a receive loop costs far
    /// more than the receive itself — the HTTP/1.1 path learned this the hard
    /// way (see the note on `tls_recv_poll`). Timeout is measured in ticks
    /// rather than iterations so it means 15 seconds on any machine.
    fn fill_to(&mut self, n: usize) -> Result<bool, Http2Error> {
        let mut buf = vec![0u8; READ_BUF];
        let start = crate::interrupts::ticks();
        while self.rx.len() < n {
            crate::net::poll_rx_only();
            // Solange auf DIESER Verbindung seit dem Senden noch nichts kam,
            // ist die kurze Geduld richtig: eine aus dem Pool genommene
            // Verbindung, die der Server inzwischen geschlossen hat, sieht
            // lokal lebendig aus (siehe `PooledConn` in `intent/http.rs`) und
            // hat einen Bildabruf 60 s gekostet. Sobald das erste Byte da ist,
            // gilt wieder die volle Nachsicht fuer stockende Uebertragungen.
            let (patience, attempt) = if self.answered {
                (crate::tls::QUIET_TRANSFER, crate::tls::ATTEMPT_TICKS)
            } else if self.reused {
                (crate::tls::QUIET_FIRST_BYTE, crate::tls::ATTEMPT_TICKS_REUSED)
            } else {
                (crate::tls::QUIET_FIRST_BYTE, crate::tls::ATTEMPT_TICKS)
            };
            match crate::tls::tls_recv_patient(&mut self.tls, &mut buf, patience, attempt) {
                Ok(0) => {
                    // Either a record carrying no application data (session
                    // tickets arrive this way) or nothing ready yet.
                    if crate::interrupts::ticks().wrapping_sub(start) > 1500 {
                        return Err(Http2Error::Tls("timed out waiting for data"));
                    }
                    core::hint::spin_loop();
                }
                Ok(got) => {
                    self.answered = true;
                    self.rx.extend_from_slice(&buf[..got]);
                }
                Err(_) => return Ok(false),
            }
        }
        Ok(true)
    }

    pub fn close(&mut self) {
        let _ = crate::tls::tls_close(&mut self.tls);
    }

    pub fn is_healthy(&self) -> bool {
        !self.goaway && self.tls.is_healthy()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn find<'a>(streams: &'a mut [Stream], id: u32) -> Option<&'a mut Stream> {
    streams.iter_mut().find(|s| s.id == id)
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Strip DATA padding (§6.1). `None` if the pad length overruns the payload.
fn strip_data_padding(flags: u8, payload: &[u8]) -> Option<&[u8]> {
    if flags & FLAG_PADDED == 0 {
        return Some(payload);
    }
    let pad = *payload.first()? as usize;
    let body = payload.get(1..)?;
    body.len().checked_sub(pad).map(|end| &body[..end])
}

/// Strip HEADERS padding and the deprecated priority block (§6.2).
fn strip_headers_padding(flags: u8, payload: &[u8]) -> Option<&[u8]> {
    let mut body = payload;
    let mut pad = 0usize;
    if flags & FLAG_PADDED != 0 {
        pad = *body.first()? as usize;
        body = body.get(1..)?;
    }
    if flags & FLAG_PRIORITY != 0 {
        body = body.get(5..)?; // 4-byte dependency + 1-byte weight
    }
    body.len().checked_sub(pad).map(|end| &body[..end])
}

/// Open a TLS session offering h2, then start a connection on it. Falls back
/// to `Err(NotNegotiated)` — never silently to HTTP/1.1 — so the caller can
/// decide explicitly.
pub fn connect(host: &str, ip: [u8; 4], port: u16) -> Result<Http2, Http2Error> {
    if super::http::chatty() {
        kprintln!(
            "[npk]   h2 connect {} -> {}.{}.{}.{}:{}",
            host, ip[0], ip[1], ip[2], ip[3], port
        );
    }
    let t_tcp = crate::interrupts::ticks();
    let handle = crate::net::tcp::connect(ip, port).map_err(|_| Http2Error::Tls("TCP connect failed"))?;
    let t_tls = crate::interrupts::ticks();
    let tls = match crate::tls::tls_connect_alpn(handle, host, &["h2", "http/1.1"]) {
        Ok(s) => s,
        Err(_) => {
            let _ = crate::net::tcp::close(handle);
            return Err(Http2Error::Tls("TLS handshake failed"));
        }
    };
    // Which leg is slow? The connect swings between ~200 ms and ~2100 ms
    // across runs; 2 s is about a retransmission timeout, so name the leg.
    // tcp+tls came to 90 ms while the caller measured 2100 for the same
    // connect, so the preface — the first application write after the
    // handshake — is timed too rather than left as the unnamed remainder.
    let t_start = crate::interrupts::ticks();
    let legs = |t_start: u64| {
        kprintln!("[npk]   h2 tcp {} ms + tls {} ms + preface {} ms",
            t_tls.wrapping_sub(t_tcp) * 10,
            t_start.wrapping_sub(t_tls) * 10,
            crate::interrupts::ticks().wrapping_sub(t_start) * 10);
    };
    match tls.alpn() {
        Some("h2") => {
            let c = Http2::start(tls);
            legs(t_start);
            c
        }
        other => {
            legs(t_start);
            kprintln!("[npk]   h2 not offered by {} (alpn={:?})", host, other);
            let mut tls = tls;
            let _ = crate::tls::tls_close(&mut tls);
            Err(Http2Error::NotNegotiated)
        }
    }
}

/// Unused today but part of the response contract; keeps `String` imported
/// where the body decoder will need it.
#[allow(dead_code)]
fn _body_as_string(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}
