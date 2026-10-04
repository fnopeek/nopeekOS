//! `WebSocket` (RFC 6455): handshake, framing and the JS surface.
//!
//! The engine opens no connection. As with `fetch`, it queues a request
//! (`pending_sockets`), the host executes it and hands the bytes back via
//! `npk_tls_*`. Everything here works on byte buffers and is testable on
//! the host.
//!
//! Same origin only. A WebSocket is not protected by any CORS response
//! check; only the server decides in the handshake whether to accept a
//! foreign origin. `Origin` is still sent so a server that may accept one
//! later can decide.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;

use super::ws_crypto::{base64, sha1};

/// RFC 6455 §4.2.2: the fixed GUID the server appends.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Per-message limit, so a chatty peer cannot fill the module heap.
const MAX_MESSAGE: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State { Connecting = 0, Open = 1, Closing = 2, Closed = 3 }

/// What came out of the stream, for the caller.
#[derive(Debug, PartialEq)]
pub enum Event {
    Open,
    Text(String),
    Binary(Vec<u8>),
    /// Code and reason. `1006` means closed without a Close frame; it must
    /// never be sent, it only reports an end nobody announced (§7.1.5).
    Closed(u16, String),
    Error(String),
}

/// One slot: the connection state without the host side.
pub struct Socket {
    pub id: u32,
    pub url: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub origin: String,
    pub state: State,
    /// The key we sent, base64. The response is checked against it.
    key: String,
    /// Bytes not yet taken by the wire.
    out: Vec<u8>,
    /// Received bytes, not yet parsed.
    inbox: Vec<u8>,
    /// The response header block while it is incomplete.
    handshake_done: bool,
    /// Partial message from continuation frames (§5.4).
    frag: Vec<u8>,
    frag_text: bool,
    /// Whether we sent the Close frame.
    close_sent: bool,
}

impl Socket {
    /// Parse `wss://` or `ws://`. A missing port is 443 or 80, as for HTTP.
    /// `ws://` is only allowed from an unencrypted page; from an `https` page
    /// it is mixed content, which browsers block.
    pub fn new(id: u32, url: &str, page_origin: &str, page_secure: bool,
               nonce: [u8; 16]) -> Result<Socket, String> {
        let (secure, rest) = if let Some(r) = url.strip_prefix("wss://") { (true, r) }
            else if let Some(r) = url.strip_prefix("ws://") { (false, r) }
            else { return Err(String::from("WebSocket: the URL scheme must be ws or wss")) };
        if page_secure && !secure {
            return Err(String::from("WebSocket: an insecure ws:// from a secure page is mixed content"));
        }
        let (hostport, path) = match rest.find(['/', '?']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if hostport.is_empty() { return Err(String::from("WebSocket: empty host")) }
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => match p.parse::<u16>() {
                Ok(n) if !h.is_empty() => (h, n),
                _ => return Err(String::from("WebSocket: bad port")),
            },
            None => (hostport, if secure { 443 } else { 80 }),
        };
        Ok(Socket {
            id,
            url: String::from(url),
            host: String::from(host),
            port,
            path: String::from(path),
            origin: String::from(page_origin),
            state: State::Connecting,
            key: base64(&nonce),
            out: Vec::new(),
            inbox: Vec::new(),
            handshake_done: false,
            frag: Vec::new(),
            frag_text: false,
            close_sent: false,
        })
    }

    /// The upgrade request (§4.1). `Host` carries the port when it is not the
    /// default, otherwise a server with several names may reject it.
    pub fn handshake(&self) -> Vec<u8> {
        let hostline = if (self.port == 443) || (self.port == 80) {
            self.host.clone()
        } else {
            alloc::format!("{}:{}", self.host, self.port)
        };
        let req = alloc::format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\nOrigin: {}\r\n\
             User-Agent: Mozilla/5.0 (nopeekOS) beak\r\n\r\n",
            self.path, hostline, self.key, self.origin);
        req.into_bytes()
    }

    /// The `Sec-WebSocket-Accept` value the server must return (§4.1, item 4).
    fn expected_accept(&self) -> String {
        let mut s = self.key.clone();
        s.push_str(GUID);
        base64(&sha1(s.as_bytes()))
    }

    /// Feed bytes from the wire. Returns what they produced.
    pub fn feed(&mut self, bytes: &[u8], rnd: &mut dyn FnMut() -> [u8; 4]) -> Vec<Event> {
        let mut out = Vec::new();
        if self.state == State::Closed { return out }
        self.inbox.extend_from_slice(bytes);
        if !self.handshake_done {
            match self.try_handshake() {
                Ok(false) => return out,          // header not complete yet
                Ok(true) => { self.state = State::Open; out.push(Event::Open); }
                Err(e) => {
                    self.state = State::Closed;
                    out.push(Event::Error(e));
                    out.push(Event::Closed(1006, String::new()));
                    return out;
                }
            }
        }
        loop {
            match self.next_frame() {
                Ok(None) => break,
                Ok(Some((fin, op, payload))) => {
                    if let Some(ev) = self.handle_frame(fin, op, payload, rnd) { out.push(ev) }
                    if self.state == State::Closed { break }
                }
                Err(e) => {
                    self.state = State::Closed;
                    out.push(Event::Error(e));
                    out.push(Event::Closed(1006, String::new()));
                    break;
                }
            }
        }
        out
    }

    /// Parse the response header. `Ok(false)` = not complete yet.
    fn try_handshake(&mut self) -> Result<bool, String> {
        let Some(end) = find(&self.inbox, b"\r\n\r\n") else {
            // A header that never ends is not a header.
            if self.inbox.len() > 16 * 1024 {
                return Err(String::from("WebSocket: the handshake reply has no end"));
            }
            return Ok(false);
        };
        let head = String::from_utf8_lossy(&self.inbox[..end]).into_owned();
        self.inbox.drain(..end + 4);
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap_or("");
        // "HTTP/1.1 101 …"; anything else is a refusal, and the status goes into
        // the message.
        if !status.contains(" 101") {
            return Err(alloc::format!("WebSocket: the server answered {status:.64} instead of 101"));
        }
        let (mut upgrade, mut connection, mut accept) = (false, false, String::new());
        for l in lines {
            let Some((k, v)) = l.split_once(':') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            match k.as_str() {
                "upgrade" => upgrade = v.eq_ignore_ascii_case("websocket"),
                "connection" => connection = v.to_ascii_lowercase().contains("upgrade"),
                "sec-websocket-accept" => accept = String::from(v),
                _ => {}
            }
        }
        if !upgrade || !connection {
            return Err(String::from("WebSocket: the reply is not an upgrade"));
        }
        // This check is the point of the key. It does not prove the peer is
        // trustworthy (TLS does that); it proves a WebSocket server answered and
        // not a cache replaying an old response (§1.3).
        if accept != self.expected_accept() {
            return Err(String::from("WebSocket: Sec-WebSocket-Accept does not match the key"));
        }
        self.handshake_done = true;
        Ok(true)
    }

    /// Take one frame (§5.2). `Ok(None)` = not complete yet.
    #[allow(clippy::type_complexity)]
    fn next_frame(&mut self) -> Result<Option<(bool, u8, Vec<u8>)>, String> {
        let b = &self.inbox;
        if b.len() < 2 { return Ok(None) }
        let fin = b[0] & 0x80 != 0;
        if b[0] & 0x70 != 0 { return Err(String::from("WebSocket: reserved bits are set")) }
        let op = b[0] & 0x0F;
        let masked = b[1] & 0x80 != 0;
        // A server never masks (§5.1). If it does, the connection must be
        // failed, not unmasked.
        if masked { return Err(String::from("WebSocket: the server masked a frame")) }
        let len7 = (b[1] & 0x7F) as usize;
        let (len, hdr) = match len7 {
            126 => {
                if b.len() < 4 { return Ok(None) }
                (((b[2] as usize) << 8) | b[3] as usize, 4)
            }
            127 => {
                if b.len() < 10 { return Ok(None) }
                let mut n: u64 = 0;
                for &x in &b[2..10] { n = (n << 8) | x as u64 }
                if n > MAX_MESSAGE as u64 {
                    return Err(String::from("WebSocket: the frame is larger than the cap"));
                }
                (n as usize, 10)
            }
            n => (n, 2),
        };
        if len > MAX_MESSAGE { return Err(String::from("WebSocket: the frame is larger than the cap")) }
        if b.len() < hdr + len { return Ok(None) }
        let payload = b[hdr..hdr + len].to_vec();
        self.inbox.drain(..hdr + len);
        Ok(Some((fin, op, payload)))
    }

    fn handle_frame(&mut self, fin: bool, op: u8, payload: Vec<u8>,
                    rnd: &mut dyn FnMut() -> [u8; 4]) -> Option<Event> {
        match op {
            // Continuation
            0x0 => {
                if self.frag.is_empty() && !self.frag_text {
                    return Some(Event::Error(String::from("WebSocket: a continuation without a start")));
                }
                if self.frag.len() + payload.len() > MAX_MESSAGE {
                    self.state = State::Closed;
                    return Some(Event::Error(String::from("WebSocket: the message is larger than the cap")));
                }
                self.frag.extend_from_slice(&payload);
                if !fin { return None }
                let done = core::mem::take(&mut self.frag);
                let text = self.frag_text;
                self.frag_text = false;
                Some(if text { Event::Text(String::from_utf8_lossy(&done).into_owned()) }
                     else { Event::Binary(done) })
            }
            // Text / binary
            0x1 | 0x2 => {
                let text = op == 0x1;
                if !fin { self.frag = payload; self.frag_text = text; return None }
                Some(if text { Event::Text(String::from_utf8_lossy(&payload).into_owned()) }
                     else { Event::Binary(payload) })
            }
            // Close (§5.5.1)
            0x8 => {
                let code = if payload.len() >= 2 {
                    ((payload[0] as u16) << 8) | payload[1] as u16
                } else { 1005 };
                let reason = if payload.len() > 2 {
                    String::from_utf8_lossy(&payload[2..]).into_owned()
                } else { String::new() };
                // The reply is the same frame echoed back, once.
                if !self.close_sent {
                    self.close_sent = true;
                    let echo = if payload.len() >= 2 { payload[..2].to_vec() } else { Vec::new() };
                    self.push_frame(0x8, &echo, rnd);
                }
                self.state = State::Closed;
                Some(Event::Closed(code, reason))
            }
            // Ping -> Pong with the same payload (§5.5.2)
            0x9 => { self.push_frame(0xA, &payload, rnd); None }
            // Pong: nothing to do, but not an error.
            0xA => None,
            other => {
                self.state = State::Closed;
                Some(Event::Error(alloc::format!("WebSocket: unknown opcode {other}")))
            }
        }
    }

    /// Every client frame is masked (§5.3) with four bytes of real randomness
    /// from the host. This protects intermediaries from reading the stream as
    /// HTTP, not the content (TLS does that).
    fn push_frame(&mut self, op: u8, payload: &[u8], rnd: &mut dyn FnMut() -> [u8; 4]) {
        let mut f = Vec::with_capacity(payload.len() + 14);
        f.push(0x80 | op);
        let n = payload.len();
        if n < 126 {
            f.push(0x80 | n as u8);
        } else if n <= 0xFFFF {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            f.push(0x80 | 127);
            f.extend_from_slice(&(n as u64).to_be_bytes());
        }
        let mask = rnd();
        f.extend_from_slice(&mask);
        for (i, b) in payload.iter().enumerate() { f.push(b ^ mask[i % 4]) }
        self.out.extend_from_slice(&f);
    }

    /// `ws.send(text)`; `false` if the connection is not open.
    pub fn send_text(&mut self, s: &str, rnd: &mut dyn FnMut() -> [u8; 4]) -> bool {
        if self.state != State::Open { return false }
        self.push_frame(0x1, s.as_bytes(), rnd);
        true
    }

    pub fn send_binary(&mut self, d: &[u8], rnd: &mut dyn FnMut() -> [u8; 4]) -> bool {
        if self.state != State::Open { return false }
        self.push_frame(0x2, d, rnd);
        true
    }

    /// `ws.close(code, reason)`. The Close frame goes out and the connection
    /// stays `Closing` until the peer answers; only then is it cleanly closed
    /// (§7.1.2).
    pub fn close(&mut self, code: u16, reason: &str, rnd: &mut dyn FnMut() -> [u8; 4]) {
        if self.close_sent || matches!(self.state, State::Closed) { return }
        self.close_sent = true;
        let mut p = Vec::with_capacity(2 + reason.len());
        p.extend_from_slice(&code.to_be_bytes());
        p.extend_from_slice(reason.as_bytes());
        self.push_frame(0x8, &p, rnd);
        self.state = State::Closing;
    }

    /// Bytes to put on the wire. The caller takes them; sending twice would
    /// be a second frame.
    pub fn take_out(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.out)
    }

    pub fn has_out(&self) -> bool { !self.out.is_empty() }

    /// The peer is gone without a Close frame; `1006` exists for exactly this.
    pub fn hung_up(&mut self) -> Option<Event> {
        if self.state == State::Closed { return None }
        self.state = State::Closed;
        Some(Event::Closed(1006, String::new()))
    }
}

// ── Requests to the host ─────────────────────────────────────────────────
//
// Same pattern as `fetch`: the engine queues, the host executes.

/// A connection the host still has to open.
pub struct PendingSocket {
    pub id: u32,
    pub host: String,
    pub port: u16,
    /// The complete upgrade request; the host sends it once TLS is up.
    pub hello: Vec<u8>,
    /// `false` for `ws://`: no TLS. The host currently refuses it; the field
    /// keeps the request complete so the host does not have to guess.
    pub secure: bool,
}

use super::interp::{Interp, C};
use super::value::{Gc, Prop, Value, new_obj};

/// Hidden fields on the JS object, as for XHR (`\0!` is not a name script
/// can write).
const W_ID: &str = "\0!ws.id";
const W_LISTEN: &str = "\0!ws.listen";

fn sock_of<'a>(i: &'a mut Interp, t: &Value) -> Option<u32> {
    let v = i.get(t, W_ID).ok()?;
    match v { Value::Num(n) => Some(n as u32), _ => None }
}

fn find_sock(i: &mut Interp, id: u32) -> Option<usize> {
    i.sockets.iter().position(|s| s.id == id)
}

/// Four bytes from the host's random source. RFC 6455 §5.3 requires a
/// mask, and a predictable one would be worse than no connection.
fn mask_source() -> impl FnMut() -> [u8; 4] {
    || {
        let mut b = [0u8; 4];
        if !super::random::fill(&mut b) {
            // The mask is not a secret but a scrambler against caches reading the
            // stream as HTTP. Without a source it stays zero, but then `WebSocket`
            // is not exposed at all.
            b = [0; 4];
        }
        b
    }
}

/// Call a page handler (`onX` or a listener) and report an exception from
/// it to the console, as browsers do. Delivery continues: each handler
/// stands alone, one that throws does not stop the next.
fn run_handler(i: &mut Interp, f: &Value, obj: &Value, ev: &Value, kind: &str) {
    if !i.is_callable(f) { return }
    if let Err(super::interp::Abrupt::Throw(v)) = i.call(f, obj.clone(), &[ev.clone()]) {
        let text = i.get(&v, "message").ok()
            .and_then(|m| i.to_string(&m).ok())
            .filter(|m| !m.is_empty())
            .or_else(|| i.to_string(&v).ok())
            .map(|s| String::from(&*s))
            .unwrap_or_else(|| String::from("?"));
        i.console_push(alloc::format!("WebSocket: der {kind}-Behandler warf: {text}"));
    }
}

fn fire(i: &mut Interp, obj: &Value, kind: &str, fill: &dyn Fn(&mut Interp, &Gc)) -> C<()> {
    let ev = new_obj(Some(i.realm.object_proto.clone()));
    ev.borrow_mut().define("type", Prop::builtin(Value::str(kind)));
    ev.borrow_mut().define("target", Prop::builtin(obj.clone()));
    fill(i, &ev);
    let ev = Value::Obj(ev);
    let on = i.get(obj, &alloc::format!("on{kind}"))?;
    run_handler(i, &on, obj, &ev, kind);
    let list = i.get(obj, W_LISTEN)?;
    if let Value::Obj(arr) = &list {
        let n = match i.get(&list, "length") { Ok(Value::Num(n)) => n as usize, _ => 0 };
        for k in 0..n {
            let pair = match arr.borrow().get_own(&alloc::format!("{k}")) {
                Some(p) => p.value.clone().unwrap_or(Value::Undefined),
                None => continue,
            };
            let kk = i.get(&pair, "0")?;
            let g = i.get(&pair, "1")?;
            if i.to_string(&kk)?.as_ref() == kind {
                run_handler(i, &g, obj, &ev, kind);
            }
        }
    }
    Ok(())
}

/// The host hands over what arrived on the wire. `None` means disconnected.
pub fn host_bytes(i: &mut Interp, id: u32, bytes: Option<&[u8]>) {
    let Some(idx) = find_sock(i, id) else { return };
    let mut rnd = mask_source();
    let events = match bytes {
        Some(b) => i.sockets[idx].feed(b, &mut rnd),
        None => i.sockets[idx].hung_up().into_iter().collect(),
    };
    if events.is_empty() { return }
    let Some(obj) = i.socket_objs.get(&id).cloned() else { return };
    let obj = Value::Obj(obj);
    for ev in events {
        match ev {
            Event::Open => {
                let _ = i.set(&obj, "readyState", Value::Num(1.0), false);
                let _ = fire(i, &obj, "open", &|_, _| {});
            }
            Event::Text(t) => {
                let _ = fire(i, &obj, "message", &move |_, e| {
                    e.borrow_mut().define("data", Prop::builtin(Value::str(&t)));
                });
            }
            Event::Binary(d) => {
                // Binary messages arrive as `ArrayBuffer`; page code checks
                // `typeof e.data`, and a string there would be a wrong answer.
                let len = d.len();
                let buf = i.new_buffer(d.len());
                if let Value::Obj(o) = &buf {
                    if let super::value::ObjKind::Buffer(b) = &o.borrow().kind {
                        b.bytes.borrow_mut().copy_from_slice(&d);
                    }
                }
                let _ = fire(i, &obj, "message", &move |_, e| {
                    e.borrow_mut().define("data", Prop::builtin(buf.clone()));
                    e.borrow_mut().define("byteLength", Prop::builtin(Value::Num(len as f64)));
                });
            }
            Event::Closed(code, reason) => {
                let _ = i.set(&obj, "readyState", Value::Num(3.0), false);
                let _ = fire(i, &obj, "close", &move |_, e| {
                    e.borrow_mut().define("code", Prop::builtin(Value::Num(code as f64)));
                    e.borrow_mut().define("reason", Prop::builtin(Value::str(&reason)));
                    // `wasClean` is false for 1006; that is what the code is for.
                    e.borrow_mut().define("wasClean", Prop::builtin(Value::Bool(code != 1006)));
                });
            }
            Event::Error(msg) => {
                i.console_push(alloc::format!("error: {msg}"));
                let _ = fire(i, &obj, "error", &|_, _| {});
            }
        }
    }
    // Closed means removed; otherwise the table keeps every connection
    // ever opened until navigation.
    if let Some(idx) = find_sock(i, id) {
        if i.sockets[idx].state == State::Closed {
            i.sockets.remove(idx);
            i.socket_objs.remove(&id);
        }
    }
}

/// What has to go on the wire for this connection.
pub fn take_out_for(i: &mut Interp, id: u32) -> Option<Vec<u8>> {
    let idx = find_sock(i, id)?;
    if !i.sockets[idx].has_out() { return None }
    Some(i.sockets[idx].take_out())
}

/// `WebSocket` on the global object.
///
/// Exposed only when real randomness exists, the same rule as `crypto`:
/// without it there is no mask (RFC 6455 §5.3), and since pages test
/// `if (window.WebSocket)`, a missing interface is better than a
/// predictable mask.
pub fn install(i: &mut Interp) {
    if !super::random::available() { return }
    let fp = i.realm.function_proto.clone();
    let proto = new_obj(Some(i.realm.object_proto.clone()));

    let meth = |o: &Gc, name: &str, f: super::value::NativeFn, len: usize| {
        let g = super::value::native(Some(fp.clone()), f, name, len, false);
        o.borrow_mut().define(name, Prop::builtin(Value::Obj(g)));
    };
    meth(&proto, "send", |i, t, a| {
        let Some(id) = sock_of(i, &t) else { return i.type_err("not a WebSocket") };
        let Some(idx) = find_sock(i, id) else {
            return i.type_err("WebSocket is already closed");
        };
        let arg = a.first().cloned().unwrap_or(Value::Undefined);
        let mut rnd = mask_source();
        // An `ArrayBuffer` or a view on one goes out as a binary frame,
        // everything else as text.
        let bytes = bytes_of(i, &arg);
        let ok = match bytes {
            Some(b) => i.sockets[idx].send_binary(&b, &mut rnd),
            None => {
                let s = i.to_string(&arg)?;
                i.sockets[idx].send_text(&s, &mut rnd)
            }
        };
        if !ok {
            return i.type_err("WebSocket is not open");
        }
        Ok(Value::Undefined)
    }, 1);
    meth(&proto, "close", |i, t, a| {
        let Some(id) = sock_of(i, &t) else { return i.type_err("not a WebSocket") };
        let Some(idx) = find_sock(i, id) else { return Ok(Value::Undefined) };
        let code = match a.first() {
            Some(v) if !matches!(v, Value::Undefined) => i.to_number(v)? as u16,
            _ => 1000,
        };
        let reason = match a.get(1) {
            Some(v) if !matches!(v, Value::Undefined) => String::from(&*i.to_string(v)?),
            _ => String::new(),
        };
        let mut rnd = mask_source();
        i.sockets[idx].close(code, &reason, &mut rnd);
        let _ = i.set(&t, "readyState", Value::Num(2.0), false);
        Ok(Value::Undefined)
    }, 2);
    meth(&proto, "addEventListener", |i, t, a| {
        let k = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let f = a.get(1).cloned().unwrap_or(Value::Undefined);
        let list = i.get(&t, W_LISTEN)?;
        let pair = i.new_array(alloc::vec![Value::str(&k), f]);
        let push = i.get(&list, "push")?;
        i.call(&push, list, &[pair])?;
        Ok(Value::Undefined)
    }, 2);
    meth(&proto, "removeEventListener", |_, _, _| Ok(Value::Undefined), 2);

    // The four constants, on the prototype and on the constructor.
    for (n, v) in [("CONNECTING", 0.0), ("OPEN", 1.0), ("CLOSING", 2.0), ("CLOSED", 3.0)] {
        proto.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
    }
    proto.borrow_mut().define(super::value::SYM_TO_STRING_TAG,
        Prop::frozen(Value::str("WebSocket")));

    let ctor = super::value::native(Some(fp.clone()), |i, _, a| {
        let url = i.to_string(a.first().unwrap_or(&Value::Undefined))?;
        let page = super::url::parse_abs(&i.loc_href);
        let origin = page.as_ref().map(|p| p.origin()).unwrap_or_default();
        let secure = page.as_ref().is_some_and(|p| p.scheme == "https");
        let mut nonce = [0u8; 16];
        if !super::random::fill(&mut nonce) {
            return i.type_err("WebSocket: no entropy source");
        }
        let id = i.next_socket_id;
        i.next_socket_id += 1;
        let sock = match Socket::new(id, &url, &origin, secure, nonce) {
            Ok(s) => s,
            // The spec names `SyntaxError` for an invalid URL.
            Err(e) => return Err(i.throw_kind("SyntaxError", &e)),
        };
        // Same origin only (see the module header); `Origin` is sent anyway.
        // The host is compared, not the scheme: `wss://` pairs with `https://`
        // as `ws://` with `http://`.
        let same = page.as_ref().is_some_and(|p| p.host.eq_ignore_ascii_case(&sock.host));
        if !same {
            // Page code checks `e.name === 'SecurityError'`, not the message.
            // `throw_kind` only knows the language error types, so the name is set
            // here, as in `crypto.getRandomValues`.
            let e = i.throw_kind("Error", &alloc::format!(
                "WebSocket: {} is a different origin — only the page's own origin is allowed",
                sock.host));
            if let super::interp::Abrupt::Throw(Value::Obj(x)) = &e {
                x.borrow_mut().define("name", Prop::builtin(Value::str("SecurityError")));
            }
            return Err(e);
        }
        let obj = new_obj(Some(i.realm.websocket_proto.clone()));
        obj.borrow_mut().define(W_ID, hidden(Value::Num(id as f64)));
        obj.borrow_mut().define("url", Prop::builtin(Value::str(&url)));
        obj.borrow_mut().define("readyState", Prop::data(Value::Num(0.0)));
        obj.borrow_mut().define("bufferedAmount", Prop::data(Value::Num(0.0)));
        obj.borrow_mut().define("protocol", Prop::builtin(Value::str("")));
        obj.borrow_mut().define("extensions", Prop::builtin(Value::str("")));
        obj.borrow_mut().define("binaryType", Prop::data(Value::str("arraybuffer")));
        let list = i.new_array(Vec::new());
        obj.borrow_mut().define(W_LISTEN, hidden(list));
        i.pending_sockets.push(PendingSocket {
            id, host: sock.host.clone(), port: sock.port,
            hello: sock.handshake(), secure: sock.port == 443 || url.starts_with("wss://"),
        });
        i.sockets.push(sock);
        i.socket_objs.insert(id, obj.clone());
        Ok(Value::Obj(obj))
    }, "WebSocket", 1, true);
    ctor.borrow_mut().define("prototype", Prop {
        value: Some(Value::Obj(proto.clone())), get: None, set: None,
        writable: false, enumerable: false, configurable: false });
    for (n, v) in [("CONNECTING", 0.0), ("OPEN", 1.0), ("CLOSING", 2.0), ("CLOSED", 3.0)] {
        ctor.borrow_mut().define(n, Prop::frozen(Value::Num(v)));
    }
    proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(ctor.clone())));
    i.realm.websocket_proto = proto;
    let g = i.realm.global.clone();
    g.borrow_mut().define("WebSocket", Prop::builtin(Value::Obj(ctor)));
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    if n.is_empty() || h.len() < n.len() { return None }
    (0..=h.len() - n.len()).find(|&i| &h[i..i + n.len()] == n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feste_maske() -> impl FnMut() -> [u8; 4] { || [0x11, 0x22, 0x33, 0x44] }

    fn offen(s: &mut Socket) {
        let accept = s.expected_accept();
        let reply = alloc::format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n");
        let ev = s.feed(reply.as_bytes(), &mut feste_maske());
        assert_eq!(ev, vec![Event::Open], "der Handschlag muss aufgehen");
    }

    fn sock(url: &str) -> Socket {
        Socket::new(1, url, "https://example.test", true, [7u8; 16]).expect("URL")
    }

    #[test]
    fn die_url_wird_zerlegt_und_mischinhalt_abgelehnt() {
        let s = sock("wss://a.test/x?y=1");
        assert_eq!((s.host.as_str(), s.port, s.path.as_str()), ("a.test", 443, "/x?y=1"));
        let s = sock("wss://a.test:8443/ws");
        assert_eq!((s.host.as_str(), s.port), ("a.test", 8443));
        let s = sock("wss://a.test");
        assert_eq!(s.path, "/", "ohne Pfad ist es die Wurzel");
        // `ws://` from a secure page is mixed content.
        assert!(Socket::new(1, "ws://a.test/", "https://e.test", true, [0; 16]).is_err());
        assert!(Socket::new(1, "ws://a.test/", "http://e.test", false, [0; 16]).is_ok());
        assert!(Socket::new(1, "https://a.test/", "https://e.test", true, [0; 16]).is_err());
    }

    #[test]
    fn der_handschlag_prueft_den_schluessel() {
        let mut s = sock("wss://a.test/ws");
        let req = String::from_utf8(s.handshake()).unwrap();
        assert!(req.starts_with("GET /ws HTTP/1.1\r\n"));
        assert!(req.contains("\r\nUpgrade: websocket\r\n"));
        assert!(req.contains("\r\nSec-WebSocket-Version: 13\r\n"));
        // `Origin` is sent: a server allowed to accept a foreign origin can
        // only do so if it sees it.
        assert!(req.contains("\r\nOrigin: https://example.test\r\n"));
        assert!(req.contains("\r\nHost: a.test\r\n"), "ohne Port bei 443");
        // A response with the wrong Accept is rejected.
        let bad = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                   Connection: Upgrade\r\nSec-WebSocket-Accept: falsch\r\n\r\n";
        let ev = s.feed(bad.as_bytes(), &mut feste_maske());
        assert!(matches!(ev.first(), Some(Event::Error(_))), "falsches Accept: {ev:?}");
        assert_eq!(s.state, State::Closed);
    }

    #[test]
    fn der_port_steht_im_host_kopf_wenn_er_nicht_der_vorgegebene_ist() {
        let s = sock("wss://a.test:8443/ws");
        let req = String::from_utf8(s.handshake()).unwrap();
        assert!(req.contains("\r\nHost: a.test:8443\r\n"), "{req}");
    }

    /// An engine with `WebSocket` and a page as origin.
    fn motor(js: &str) -> Interp {
        fn zufall(out: &mut [u8]) -> bool { out.fill(7); true }
        crate::js::random::set_source(zufall);
        let mut i = Interp::new();
        i.loc_href = String::from("https://a.test/seite");
        let prog = crate::js::parse(js, false).expect("parst");
        assert!(i.run_program(&prog).is_ok(), "das Skript muss laufen");
        i
    }

    /// Answer the handshake and follow with a text frame, as a real server
    /// does.
    fn server_spricht(i: &mut Interp, text: &str) {
        let id = i.sockets[0].id;
        let accept = i.sockets[0].expected_accept();
        let hallo = alloc::format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n");
        host_bytes(i, id, Some(hallo.as_bytes()));
        let mut rahmen = alloc::vec![0x81u8, text.len() as u8];
        rahmen.extend_from_slice(text.as_bytes());
        host_bytes(i, id, Some(&rahmen));
    }

    fn global(i: &mut Interp, name: &str) -> String {
        let g = Value::Obj(i.realm.global.clone());
        let Ok(v) = i.get(&g, name) else { return String::from("<fehlt>") };
        match i.to_string(&v) {
            Ok(s) => String::from(&*s),
            Err(_) => String::from("<unlesbar>"),
        }
    }

    /// What the page sees: `onopen` fires, `e.data` is a string and
    /// `JSON.parse` accepts it.
    #[test]
    fn die_seite_bekommt_die_nachricht_als_zeichenkette() {
        let mut i = motor(
            "var offen = false; var gesehen = 'nichts'; var zustand = -1;
             var ws = new WebSocket('wss://a.test/ws');
             ws.onopen = function () { offen = true; };
             ws.onmessage = function (e) { gesehen = JSON.parse(e.data).id;
                                           zustand = ws.readyState; };");
        assert_eq!(i.sockets.len(), 1, "ein Socket steht an");
        assert_eq!(i.pending_sockets.len(), 1, "und der Wirt hat einen Auftrag");
        server_spricht(&mut i, r#"{"id":1,"result":{}}"#);
        assert_eq!(global(&mut i, "offen"), "true", "onopen muss fallen");
        assert_eq!(global(&mut i, "gesehen"), "1", "e.data muss durch JSON.parse gehen");
        assert_eq!(global(&mut i, "zustand"), "1", "waehrend der Nachricht ist der Stand OFFEN");
    }

    /// A throwing handler is reported, and the second handler still runs.
    #[test]
    fn ein_werfender_behandler_wird_gemeldet_und_haelt_den_naechsten_nicht_auf() {
        let mut i = motor(
            "var zweiter = false;
             var ws = new WebSocket('wss://a.test/ws');
             ws.onmessage = function () { null.x; };
             ws.addEventListener('message', function () { zweiter = true; });");
        server_spricht(&mut i, "{}");
        assert_eq!(global(&mut i, "zweiter"), "true", "der zweite Behandler laeuft trotzdem");
        let konsole = i.take_console();
        assert!(konsole.iter().any(|l| l.contains("message-Behandler warf")),
                "der Wurf gehoert gemeldet, gesehen: {konsole:?}");
    }

    #[test]
    fn rahmen_kommen_auch_in_stuecken_an() {
        let mut s = sock("wss://a.test/ws");
        offen(&mut s);
        // An unmasked text frame "hi", in three pieces.
        let frame = [0x81u8, 0x02, b'h', b'i'];
        assert!(s.feed(&frame[..1], &mut feste_maske()).is_empty());
        assert!(s.feed(&frame[1..3], &mut feste_maske()).is_empty());
        let ev = s.feed(&frame[3..], &mut feste_maske());
        assert_eq!(ev, vec![Event::Text(String::from("hi"))]);
    }

    #[test]
    fn fortsetzungsrahmen_werden_zusammengesetzt() {
        let mut s = sock("wss://a.test/ws");
        offen(&mut s);
        let mut b = vec![0x01u8, 0x02, b'a', b'b'];   // text, FIN clear
        b.extend_from_slice(&[0x00, 0x01, b'c']);      // continuation
        b.extend_from_slice(&[0x80, 0x01, b'd']);      // continuation, FIN set
        assert_eq!(s.feed(&b, &mut feste_maske()), vec![Event::Text(String::from("abcd"))]);
    }

    #[test]
    fn ping_wird_mit_derselben_nutzlast_beantwortet() {
        let mut s = sock("wss://a.test/ws");
        offen(&mut s);
        let ev = s.feed(&[0x89, 0x03, 1, 2, 3], &mut feste_maske());
        assert!(ev.is_empty(), "ein Ping ist kein Ereignis fuer die Seite");
        let out = s.take_out();
        assert_eq!(out[0], 0x8A, "Pong");
        assert_eq!(out[1] & 0x80, 0x80, "der Client MASKIERT immer");
        assert_eq!(out[1] & 0x7F, 3);
        let mask = &out[2..6];
        let payload: Vec<u8> = out[6..].iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
        assert_eq!(payload, vec![1, 2, 3], "dieselbe Nutzlast zurueck");
    }

    #[test]
    fn senden_maskiert_und_haelt_die_laengenformen_ein() {
        let mut s = sock("wss://a.test/ws");
        offen(&mut s);
        let mut r = feste_maske();
        assert!(s.send_text("hi", &mut r));
        let out = s.take_out();
        assert_eq!(&out[..2], &[0x81, 0x82], "Text, FIN, maskiert, Laenge 2");
        assert_eq!(&out[2..6], &[0x11, 0x22, 0x33, 0x44]);
        assert_eq!(&out[6..], &[b'h' ^ 0x11, b'i' ^ 0x22]);
        // 126 bytes -> the 16-bit form.
        let mittel = alloc::vec![b'x'; 200];
        s.send_binary(&mittel, &mut r);
        let out = s.take_out();
        assert_eq!(out[0], 0x82);
        assert_eq!(out[1], 0x80 | 126);
        assert_eq!(u16::from_be_bytes([out[2], out[3]]), 200);
        // Above 65535 the 64-bit form.
        let gross = alloc::vec![b'y'; 70_000];
        s.send_binary(&gross, &mut r);
        let out = s.take_out();
        assert_eq!(out[1], 0x80 | 127);
        assert_eq!(u64::from_be_bytes(out[2..10].try_into().unwrap()), 70_000);
    }

    #[test]
    fn ein_maskierter_server_rahmen_beendet_die_verbindung() {
        let mut s = sock("wss://a.test/ws");
        offen(&mut s);
        // §5.1: a server never masks.
        let ev = s.feed(&[0x81, 0x82, 1, 2, 3, 4, b'h', b'i'], &mut feste_maske());
        assert!(matches!(ev.first(), Some(Event::Error(_))), "{ev:?}");
        assert_eq!(s.state, State::Closed);
    }

    #[test]
    fn close_wird_beantwortet_und_ein_abriss_ist_1006() {
        let mut s = sock("wss://a.test/ws");
        offen(&mut s);
        let ev = s.feed(&[0x88, 0x02, 0x03, 0xE8], &mut feste_maske());  // 1000
        assert_eq!(ev, vec![Event::Closed(1000, String::new())]);
        assert!(!s.take_out().is_empty(), "der Close wird zurueckgeschickt");
        // A drop without a Close frame is 1006, which nobody ever sends.
        let mut s2 = sock("wss://a.test/ws");
        offen(&mut s2);
        assert_eq!(s2.hung_up(), Some(Event::Closed(1006, String::new())));
        assert_eq!(s2.hung_up(), None, "nur einmal");
    }
}

/// A hidden property, same pattern as in `fetch.rs`.
fn hidden(v: Value) -> Prop {
    Prop { value: Some(v), get: None, set: None,
           writable: true, enumerable: false, configurable: false }
}

/// The bytes behind an `ArrayBuffer` or a view on one, else `None`.
fn bytes_of(i: &mut Interp, v: &Value) -> Option<Vec<u8>> {
    let Value::Obj(o) = v else { return None };
    if let super::value::ObjKind::Buffer(b) = &o.borrow().kind {
        return Some(b.bytes.borrow().clone());
    }
    let t = super::interp::ta_of(o)?;
    let b = t.buf.borrow();
    let super::value::ObjKind::Buffer(buf) = &b.kind else { return None };
    let bytes = buf.bytes.borrow();
    let start = t.offset.min(bytes.len());
    let end = (start + t.len * t.kind.size()).min(bytes.len());
    let _ = i;
    Some(bytes[start..end].to_vec())
}
