//! HTTP/HTTPS intents

use crate::{kprint, kprintln, capability};
use alloc::string::String;
use super::{parse_ip, resolve_path};

const HTTP_MAX_RESPONSE: usize = 128 * 1024; // 128 KB

/// Per-request chatter — the connect breakdown and the HTTP status lines.
///
/// On by default: for `https <host>` the transfer IS what you asked about,
/// and the `dns + arp + tcp + tls` split is the tool that names a slow leg
/// (see the netbench work). But an intent that makes several requests just to
/// answer a question of its own — `update` fetches three manifests before it
/// can even say what changed — drowns its own output in them. Such a caller
/// takes a `quiet()` guard for its duration; errors still speak.
static HTTP_QUIET: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn chatty() -> bool {
    !HTTP_QUIET.load(core::sync::atomic::Ordering::Relaxed)
}

/// Silence per-request chatter until the returned guard is dropped — so an
/// early `return` in the middle of a fetch cannot leave the shell mute.
pub struct QuietGuard(bool);

pub fn quiet() -> QuietGuard {
    QuietGuard(HTTP_QUIET.swap(true, core::sync::atomic::Ordering::Relaxed))
}

impl Drop for QuietGuard {
    fn drop(&mut self) {
        HTTP_QUIET.store(self.0, core::sync::atomic::Ordering::Relaxed);
    }
}

/// How we identify ourselves. Deliberately NOT a borrowed browser string.
///
/// This used to read `Mozilla/5.0 (X11; Linux x86_64) beak/0.1`, added in the
/// belief that the `Mozilla` prefix bought a friendlier rate-limit bucket at
/// Wikimedia. Measured on 2026-07-22, that was wrong twice over: the 429s came
/// from speaking HTTP/1.1, not from the name, and the same burst over h2 is
/// served in full with this honest string. Sending *no* User-Agent is not an
/// option either — that earns a 403, and Wikimedia's policy rightly asks for
/// an identifiable client.
///
/// One string covers OTA as well as page fetches, since both go through this
/// client. Splitting them would mean threading a parameter through every
/// layer for little gain -- which is also why the name here is the OS and not
/// `beak`: an OTA request does not come from the browser.
///
/// Two things the honest string still got wrong, both found on 2026-08-25 when
/// Wikimedia answered a test burst with 429 and a pointer to its robot policy:
///
/// * **It said `0.1` while beak stood at 0.35.3.** A version that is typed by
///   hand goes stale the moment nobody remembers it exists. `env!` cannot.
/// * **It named no contact.** Wikimedia's User-Agent policy asks for a way to
///   reach whoever is running the client, and an unidentifiable client is the
///   one that gets throttled. Adding that is the OPPOSITE of the masquerade
///   this comment argues against: it says MORE about who we are, not less.
pub(crate) const USER_AGENT: &str = concat!(
    "nopeekOS/", env!("CARGO_PKG_VERSION"),
    " (+https://github.com/fnopeek/nopeekOS)"
);

/// Flags parsed from HTTP/HTTPS arguments.
struct HttpFlags {
    headers_only: bool,  // -h: show only headers
    body_only: bool,     // -b: show only body
    silent: bool,        // -s: no status output
    discard: bool,       // -d: stream + count + report MB/s, DON'T store (RAM only)
}

/// Parse flags from anywhere in the args, return flags + cleaned args.
fn parse_http_args(args: &str) -> (HttpFlags, String) {
    let mut flags = HttpFlags { headers_only: false, body_only: false, silent: false, discard: false };
    let mut cleaned = String::new();

    for part in args.split_whitespace() {
        match part {
            "-h" => flags.headers_only = true,
            "-b" => flags.body_only = true,
            "-s" => flags.silent = true,
            "-d" => flags.discard = true,
            _ => {
                if !cleaned.is_empty() { cleaned.push(' '); }
                cleaned.push_str(part);
            }
        }
    }

    (flags, cleaned)
}

pub fn intent_http(args: &str) {
    do_http_request(args, false);
}

pub fn intent_https(args: &str) {
    do_http_request(args, true);
}

fn do_http_request(args: &str, use_tls: bool) {
    // Arm Ctrl+C cancellation for this download (cleared so a stale earlier
    // press doesn't abort us immediately).
    super::clear_cancel();
    let proto = if use_tls { "https" } else { "http" };
    let (flags, url) = parse_http_args(args);
    let url = url.as_str();

    if url.is_empty() {
        kprintln!("[npk] Usage: {} [-h|-b|-s|-d] <host> [path] [> name]", proto);
        kprintln!("[npk]   -h  Headers only");
        kprintln!("[npk]   -b  Body only (no headers)");
        kprintln!("[npk]   -s  Silent (no status messages)");
        kprintln!("[npk]   -d  Discard to RAM + report MB/s (speed test, no disk)");
        return;
    }

    // Step 1: peel off `> store-redirect` first. Has to happen
    // BEFORE the host/path split because for inputs like
    // `host/long/url/path > tmp/file` the first whitespace lives
    // before the `>`, so a naive whitespace split would assign
    // `host/long/url/path` to the host and break DNS.
    let (url_no_redirect, store_as) = if let Some(idx) = url.find('>') {
        let name = url[idx + 1..].trim();
        let left = url[..idx].trim_end();
        let redirect = if name.is_empty() { None } else { Some(String::from(name)) };
        (left, redirect)
    } else {
        (url, None)
    };

    // Step 2: split host from URL path on the first whitespace OR
    // first slash, on the redirect-free remainder.
    let (host, path) = if let Some(idx) = url_no_redirect.find(' ') {
        (&url_no_redirect[..idx], url_no_redirect[idx + 1..].trim())
    } else if let Some(idx) = url_no_redirect.find('/') {
        (&url_no_redirect[..idx], &url_no_redirect[idx..])
    } else {
        (url_no_redirect, "/")
    };
    let host = host.trim();
    let path = if path.is_empty() { "/" } else { path };

    // Streaming fast-path: HTTPS + `> name` writes the body straight
    // into npkFS via the ChunkedWriter so a multi-GB ISO / movie
    // download doesn't fill the heap. Peak RAM = one 16 MiB chunk
    // regardless of total size. Plain-HTTP storing stays on the
    // legacy buffered path (capped at HTTP_MAX_RESPONSE = 128 KB)
    // because we never want to encourage cleartext downloads of
    // anything large enough to need streaming.
    if flags.discard || store_as.is_some() {
        // Sink for the streamed body. With -d we DON'T open npkFS — bytes are
        // counted + thrown away, so this measures the pure net throughput
        // and rules the disk OUT as a bottleneck. Otherwise stream to npkFS.
        // Works for both https (TLS) and http (plain) — plain http sidesteps
        // our minimal TLS for arbitrary hosts and is a cleaner speed test.
        let mut writer: Option<(String, crate::npkfs::fs::StreamingWriter)> = if flags.discard {
            if !flags.silent {
                kprintln!("[npk] Streaming to RAM (-d, discard) — pure throughput, no disk");
                if crate::xhci::nic_attached() {
                    kprintln!("[npk] NIC USB link: {}", crate::xhci::nic_link_speed_str());
                    crate::drivers::rtl8153::log_link_diag();
                    crate::drivers::rtl8153::tally_reset(); // zero chip stats for this run
                }
            }
            None
        } else {
            let name = store_as.as_ref().unwrap();
            let store_path = match resolve_store_target(name, path) {
                Some(p) => p,
                None => {
                    kprintln!("[npk] '{}' is a directory and the URL has no filename — give an explicit name (> dir/name)", name);
                    return;
                }
            };
            if !flags.silent {
                kprintln!("[npk] Streaming to npkFS: {}", store_path);
            }
            match crate::npkfs::open_streaming_write(&store_path) {
                Ok(w) => Some((store_path, w)),
                Err(e) => { kprintln!("[npk] npkfs open failed: {:?}", e); return; }
            }
        };
        let mut total: usize = 0;
        let mut first = true;
        let start_tick = crate::interrupts::ticks();
        let mut last_tick = start_tick;
        let max_size = usize::MAX;
        let stream_result = {
            let mut sink = |chunk: &[u8]| -> Result<(), &'static str> {
                if first {
                    kprintln!("[npk]   first body bytes ({} B)", chunk.len());
                    first = false;
                }
                if let Some((_, w)) = writer.as_mut() {
                    if w.write(chunk).is_err() {
                        return Err("npkfs write failed");
                    }
                }
                total = total.saturating_add(chunk.len());
                let now = crate::interrupts::ticks();
                if now.wrapping_sub(last_tick) >= 200 {
                    let dt = now.wrapping_sub(start_tick).max(1);
                    let mbps = (total as u64 * 8 * 100) / dt / 1_000_000;
                    let (ooo_ahead, ooo_behind) = crate::net::tcp::take_ooo_stats();
                    let maxbuf = crate::net::tcp::take_max_rxbuf();
                    let txsegs = crate::net::tcp::take_tx_segs();
                    let dt2 = now.wrapping_sub(last_tick).max(1);
                    // Profiler: avg ns in poll_rx_only vs recv per loop iter.
                    use core::sync::atomic::Ordering::Relaxed;
                    let iters = PROF_ITERS.swap(0, Relaxed).max(1);
                    let poll_cyc = PROF_POLL_CYC.swap(0, Relaxed);
                    let recv_cyc = PROF_RECV_CYC.swap(0, Relaxed);
                    let ghz = (crate::interrupts::tsc_freq() / 1_000_000_000).max(1);
                    kprintln!("[npk]   rx {} KiB (~{} Mbit/s)  ooo: a={} d={}  rxbuf={}K  tx_acks={}/s  | iters={} poll={}ns recv={}ns",
                        total / 1024, mbps, ooo_ahead, ooo_behind, maxbuf / 1024,
                        txsegs as u64 * 100 / dt2,
                        iters, poll_cyc / iters / ghz, recv_cyc / iters / ghz);
                    let (nd, st, tf, rn, pk) = crate::net::take_poll_prof();
                    let pk = pk.max(1);
                    kprintln!("[npk]      poll-split per pkt: netdev={}ns stack={}ns  | per-poll: txflush={}ns render={}ns  pkts/poll={}",
                        nd / pk / ghz, st / pk / ghz, tf / iters / ghz, rn / iters / ghz, pk / iters);
                    // USB-transport layer: is the bulk RX itself the cap?
                    let (rxb, deliv, empty, armed, txc, txcyc) = crate::xhci::nic_take_stats();
                    let deliv1 = deliv.max(1);
                    let polls = deliv + empty;
                    let nic_mbit = rxb * 8 * 100 / dt2 / 1_000_000;
                    kprintln!("[npk]      NIC-usb: {} Mbit  avg_batch={}B  empty_polls={}%  ring_depth={}  | tx_acks={} tx_wait={}ns",
                        nic_mbit, rxb / deliv1, empty * 100 / polls.max(1),
                        armed / deliv1, txc, if txc > 0 { txcyc / txc / ghz } else { 0 });
                    // Do WE discard received bytes in the rx_desc walker?
                    let (frames, trunc, discard) = crate::drivers::rtl8153::take_rx_parse_stats();
                    kprintln!("[npk]      rx-parse: frames={}  truncated_batches={}  DISCARDED={} B",
                        frames, trunc, discard);
                    last_tick = now;
                }
                Ok(())
            };
            // Cross-scheme redirect-following download: lets `https cdimage…`
            // chase its 302 to a fast plain-http mirror (the user's gigabit
            // source) where strict https would dead-end.
            user_download_streaming(host, path, use_tls, max_size, &mut sink)
        };
        if let Err(e) = stream_result {
            if e == "cancelled" {
                kprintln!("[npk] ^C — download abgebrochen ({} KiB empfangen)", total / 1024);
            } else {
                kprintln!("[npk] download failed: {}", e);
            }
            // `writer` drops here → StreamingWriter::drop cleans up the partial.
            return;
        }
        // Final throughput summary (the headline number for a speed test).
        let dt = crate::interrupts::ticks().wrapping_sub(start_tick).max(1);
        let mbps = (total as u64 * 8 * 100) / dt / 1_000_000;
        let mbytes_s = (total as u64 * 100) / dt / 1_000_000;
        let secs10 = dt * 10 / 100;
        kprintln!("[npk] done: {} KiB in {}.{} s = ~{} Mbit/s (~{} MB/s)",
            total / 1024, secs10 / 10, secs10 % 10, mbps, mbytes_s);
        if crate::xhci::nic_attached() { crate::drivers::rtl8153::dump_tally("after"); }
        if let Some((store_path, w)) = writer {
            match w.finish() {
                Ok(written) => kprintln!("[npk] Stored '{}' ({} bytes)", store_path, written),
                Err(e) => kprintln!("[npk] publish failed: {:?}", e),
            }
        }
        return;
    }

    // Resolve hostname
    let ip = if let Some(ip) = parse_ip(host) {
        ip
    } else {
        match crate::net::dns::resolve(host) {
            Some(ip) => {
                if !flags.silent {
                    kprintln!("[npk] {} -> {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]);
                }
                ip
            }
            None => {
                kprintln!("[npk] Could not resolve '{}'", host);
                return;
            }
        }
    };

    // ARP resolve gateway
    let gw = crate::net::ipv4::gateway();
    let _ = crate::net::arp::resolve(gw, 100); // see open_tls: not a blind spin

    let port = if use_tls { 443u16 } else { 80 };
    if !flags.silent {
        kprintln!("[npk] Connecting to {}.{}.{}.{}:{}...", ip[0], ip[1], ip[2], ip[3], port);
    }

    let handle = match crate::net::tcp::connect(ip, port) {
        Ok(h) => h,
        Err(e) => { kprintln!("[npk] TCP error: {}", e); return; }
    };

    // TLS handshake (if HTTPS)
    let mut tls_session = if use_tls {
        if !flags.silent {
            kprintln!("[npk] TLS 1.3 handshake with '{}'...", host);
        }
        match crate::tls::tls_connect(handle, host) {
            Ok(s) => {
                if !flags.silent {
                    kprintln!("[npk] TLS established ({})", s.cipher_name());
                }
                Some(s)
            }
            Err(e) => {
                kprintln!("[npk] TLS error: {}", e);
                let _ = crate::net::tcp::close(handle);
                return;
            }
        }
    } else {
        None
    };

    // Send HTTP GET
    let http_ver = if use_tls { "1.1" } else { "1.0" };
    let request = alloc::format!(
        "GET {} HTTP/{}\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path, http_ver, host, USER_AGENT
    );

    let send_ok = if let Some(ref mut sess) = tls_session {
        crate::tls::tls_send(sess, request.as_bytes()).is_ok()
    } else {
        crate::net::tcp::send_blocking(handle, request.as_bytes(), 1000).is_ok()
    };
    if !send_ok {
        kprintln!("[npk] Send error");
        if let Some(ref mut sess) = tls_session { let _ = crate::tls::tls_close(sess); }
        else { let _ = crate::net::tcp::close(handle); }
        return;
    }

    // Receive response (buffer >= max TLS record to avoid data loss)
    let mut response = alloc::vec::Vec::new();
    let mut buf = [0u8; 17000];

    if let Some(ref mut sess) = tls_session {
        let mut empty_count = 0;
        loop {
            match crate::tls::tls_recv(sess, &mut buf) {
                Ok(0) => {
                    empty_count += 1;
                    // Response can arrive across multiple TCP segments. Poll the
                    // net stack and wait ~5ms between zero-reads instead of
                    // tight-looping in microseconds (starves slow links like
                    // QEMU user-mode NAT before the response arrives).
                    if empty_count > 40 && response.is_empty() { break; } // 200ms
                    if empty_count > 10 && !response.is_empty() { break; } // 50ms
                    crate::net::poll();
                    let end = crate::interrupts::rdtsc()
                        + crate::interrupts::tsc_freq() / 200; // 5ms
                    while crate::interrupts::rdtsc() < end { core::hint::spin_loop(); }
                }
                Ok(n) => { response.extend_from_slice(&buf[..n]); empty_count = 0; }
                Err(_) => break,
            }
            if response.len() > HTTP_MAX_RESPONSE { break; }
        }
        let _ = crate::tls::tls_close(sess);
    } else {
        loop {
            match crate::net::tcp::recv_blocking(handle, &mut buf, 500) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                // A timeout now arrives as an error rather than as Ok(0); for
                // this plain-HTTP reader both still mean "stop".
                Err(_) => break,
            }
            if response.len() > HTTP_MAX_RESPONSE { break; }
        }
        let _ = crate::net::tcp::close(handle);
    }

    if response.is_empty() {
        kprintln!("[npk] No response received");
        return;
    }

    // Find header/body boundary
    let header_end = response.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(response.len());
    let body_start = if header_end < response.len() { header_end + 4 } else { response.len() };

    if let Some(name) = store_as {
        // Guard: don't write a "successful" file from a redirect or
        // error response. The legacy non-TLS path doesn't follow
        // 3xx, so `http github.com/...` (which 301s to https) would
        // otherwise store a 0-byte file and print "Stored". Point
        // the user at `https` instead of silently succeeding.
        let status = core::str::from_utf8(&response[..header_end])
            .ok()
            .and_then(parse_status_code)
            .unwrap_or(0);
        let body = &response[body_start..];
        if !(200..300).contains(&status) {
            kprintln!("[npk] HTTP {} — not storing.", status);
            if (300..400).contains(&status) && !use_tls {
                kprintln!("[npk] (plain http doesn't follow redirects — use `https`)");
            }
            return;
        }
        if body.is_empty() {
            kprintln!("[npk] Empty body — not storing.");
            return;
        }
        let store_path = match resolve_store_target(&name, path) {
            Some(p) => p,
            None => {
                kprintln!("[npk] '{}' is a directory and the URL has no filename — give an explicit name (> dir/name)", name);
                return;
            }
        };
        match crate::npkfs::upsert(&store_path, body, capability::CAP_NULL) {
            Ok(hash) => {
                kprint!("[npk] Stored '{}' ({} bytes, hash: ", store_path, body.len());
                for b in &hash[..4] { kprint!("{:02x}", b); }
                kprintln!("...)");
            }
            Err(e) => kprintln!("[npk] Store error: {}", e),
        }
        return;
    }

    // Display based on flags
    if flags.headers_only {
        if let Ok(hdrs) = core::str::from_utf8(&response[..header_end]) {
            kprintln!("{}", hdrs);
        }
    } else if flags.body_only {
        print_response_data(&response[body_start..]);
    } else {
        // Full response: headers + body
        print_response_data(&response);
    }

    if response.len() >= HTTP_MAX_RESPONSE {
        kprintln!("\n[npk] (truncated at {} KB)", HTTP_MAX_RESPONSE / 1024);
    }
}

fn print_response_data(data: &[u8]) {
    match core::str::from_utf8(data) {
        Ok(text) => kprintln!("{}", text),
        Err(_) => kprintln!("[npk] ({} bytes, binary)", data.len()),
    }
}

/// Resolve a `> dest` store target, wget-style. `url_path` is the
/// HTTP path of the request (everything after the host) and is used
/// to infer a filename when `dest` names a directory rather than a
/// file:
///   `.`        → URL basename, in CWD
///   `dir/`     → URL basename, in `dir`
///   `dir/.`    → URL basename, in `dir`
///   `dir/name` → exact (unchanged behavior)
///
/// Returns the CWD-resolved npkFS path, or `None` if a basename was
/// required but the URL has none (path ends in `/`, or is just `/`).
fn resolve_store_target(dest: &str, url_path: &str) -> Option<String> {
    let dest = dest.trim();
    let wants_basename =
        dest.is_empty() || dest == "." || dest.ends_with('/') || dest.ends_with("/.");

    if !wants_basename {
        return Some(resolve_path(dest));
    }

    let base = url_basename(url_path)?;
    // Strip the trailing dir markers ("." / "/") and join the
    // inferred basename onto whatever directory prefix remains.
    let dir = dest.trim_end_matches('.').trim_end_matches('/');
    let joined = if dir.is_empty() {
        base
    } else {
        alloc::format!("{}/{}", dir, base)
    };
    Some(resolve_path(&joined))
}

/// Last path segment of an HTTP URL path, minus any `?query` or
/// `#fragment`. `None` when there is no segment (e.g. `/` or
/// `/dir/`).
fn url_basename(url_path: &str) -> Option<String> {
    let p = url_path
        .split(['?', '#'])
        .next()
        .unwrap_or(url_path)
        .trim_end_matches('/');
    let base = p.rsplit('/').next().unwrap_or("");
    if base.is_empty() {
        None
    } else {
        Some(String::from(base))
    }
}

/// Status + Location of a single HTTPS round-trip. Body bytes are
/// not carried in this struct — `https_get_once` always pushes them
/// through the caller's sink closure as they arrive (`https_get`
/// installs a Vec-collecting sink, `https_get_streaming` passes the
/// caller's sink through directly).
struct HttpResponse {
    status: u16,
    location: Option<String>,
    /// Raw `Content-Type` value. A browser cannot decode a document without
    /// it: a page declaring `charset=ISO-8859-1` is not UTF-8, and guessing
    /// wrong costs the whole document.
    content_type: Option<String>,
    /// The response header block verbatim, minus the status line. A browser
    /// needs headers the body cannot carry — `Set-Cookie` (which repeats, so
    /// no single-value getter would do) and `Retry-After`. Capped, see
    /// `MAX_REPLY_HEADERS`.
    headers: String,
}

/// What to send. `GET`, no extra headers, no body — what every caller before
/// the browser meant, and what [`HttpRequest::default`] gives you.
pub struct HttpRequest<'a> {
    pub method: &'a str,
    /// Extra request headers as `Name: value`, without CRLF. The caller is
    /// responsible for validating them; [`header_line_is_safe`] is the check
    /// the WASM boundary uses.
    pub headers: &'a [String],
    pub body: &'a [u8],
    /// Ask for `gzip` and inflate the answer here, before the sink sees it.
    ///
    /// Off by default, and that is deliberate: OTA downloads are already
    /// compressed and signed, so for them this would be work without an
    /// answer. It is the BROWSER that pays for its absence — the same
    /// document arrives 4,1x to 9,9x larger without it, measured across the
    /// target corpus (`docs/plan/JS_SCOPE_CONTENT_WEB.md` §8).
    pub accept_gzip: bool,
    /// Diese Anfrage geht im KLARTEXT, ohne TLS.
    ///
    /// Aus by default, und das ist keine Vorsichtsmassnahme, sondern die
    /// Politik: `parse_url` setzt sie nur, wenn `net.allow_plain_http` an ist
    /// UND der Host eine literale private Adresse ist. Wer sie von Hand setzt,
    /// umgeht diese Pruefung — also nicht tun.
    pub plain: bool,
    /// Offer HTTP/2 for this request, falling back to HTTP/1.1 when the host
    /// does not speak it.
    ///
    /// Off by default, and that is a decision about blast radius, not about
    /// h2: OTA and module installs come down this same path, and an update
    /// that cannot download is the one failure this project cannot fix over
    /// the air. The browser — which is the caller Wikimedia throttles (§8.1)
    /// — asks for it. Flip the default once a release has h2 documents on
    /// hardware behind it.
    pub try_h2: bool,
}

impl Default for HttpRequest<'_> {
    fn default() -> Self {
        HttpRequest { method: "GET", headers: &[], body: &[], accept_gzip: false, try_h2: false,
                      plain: false }
    }
}

/// The response header block, minus the status line, capped and cleaned.
///
/// Drops any line starting with a colon. No HTTP field name may begin with
/// one, so nothing legitimate is lost — and it is what keeps a server from
/// writing its own `:hop https://yourbank.example` into its headers and
/// having the browser file the cookies that follow against a host it does
/// not own.
fn capture_headers(hdr_str: &str) -> String {
    let rest = match hdr_str.split_once("\r\n") {
        Some((_, r)) => r,
        None => return String::new(),
    };
    let mut out = String::new();
    for line in rest.split("\r\n") {
        if line.starts_with(':') || out.len() + line.len() + 2 > MAX_REPLY_HEADERS {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out
}

/// Append one hop's response headers under a `:hop <url>` marker.
///
/// A redirect chain can cross origins, and a cookie belongs to the response
/// that SENT it — filing a login cookie against the URL the chain happened to
/// end at scopes it to the wrong host. So each block says where it came from.
///
/// `:hop` cannot be forged by a server: a field name may not start with a
/// colon, and `capture_headers` drops any line that does.
fn push_hop(out: &mut String, host: &str, path: &str, headers: &str) {
    out.push_str(":hop https://");
    out.push_str(host);
    out.push_str(path);
    out.push_str("\r\n");
    out.push_str(headers);
    if !headers.ends_with('\n') {
        out.push_str("\r\n");
    }
}

/// Response headers we hand back to a guest, capped. Big enough for a page
/// that sets a dozen cookies, small enough that a hostile server cannot make
/// the kernel hold an unbounded string per request.
const MAX_REPLY_HEADERS: usize = 8 * 1024;

/// Headers a guest may NOT set, because we own them.
///
/// `Host` decides which virtual host answers, and letting a caller state one
/// that differs from the TLS SNI name is a request for the wrong origin's
/// content under the wrong certificate. The other three frame the message: if
/// a caller could state its own `Content-Length` or `Transfer-Encoding`, the
/// body it sends and the body we announce could disagree, which is exactly
/// the shape of a request-smuggling bug.
// `accept-encoding` is ours for the same reason the framing headers are:
// we decode the answer. An app that asks for an encoding we do not unpack
// gets bytes it cannot read, and one that asks for none while we inflate
// anyway would be lied to about what came back.
const RESERVED_HEADERS: &[&str] =
    &["host", "content-length", "transfer-encoding", "connection", "accept-encoding"];

/// Is this a header line a guest is allowed to send?
///
/// Rejects CR, LF and NUL anywhere — a newline in a header VALUE ends the
/// header block early and everything after it is read as another request, so
/// this single check is what stops a sandboxed app from smuggling one. Also
/// rejects a missing colon, an empty name, and the reserved names above.
pub fn header_line_is_safe(line: &str) -> bool {
    if line.is_empty() || line.len() > 2048 {
        return false;
    }
    // No control characters anywhere. CR and LF are the smuggling vector, but
    // the rest have no business in a field value either (RFC 9112 §5.5), and
    // a NUL or a stray 0x01 is how a parser downstream gets confused. Tab is
    // the one control character a value may legitimately hold.
    if line.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7F) {
        return false;
    }
    let Some((name, _)) = line.split_once(':') else { return false };
    // NOT trimmed, deliberately. A leading space makes the line an obsolete
    // folded continuation of the header before it, and a space before the
    // colon is a name a server may read differently than we do. Both are
    // ways to mean something other than what this line looks like, so both
    // are refused: `is_ascii_graphic` excludes space and tab.
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_graphic()) {
        return false;
    }
    !RESERVED_HEADERS.iter().any(|r| name.eq_ignore_ascii_case(r))
}

/// Is this a method a guest is allowed to send? A token of ASCII letters,
/// nothing else — the method sits at the very front of the request line, so
/// a space or a newline there rewrites the whole request.
pub fn method_is_safe(m: &str) -> bool {
    !m.is_empty() && m.len() <= 16 && m.bytes().all(|b| b.is_ascii_uppercase())
}

/// What the caller learns about a response besides its bytes.
///
/// Grouped rather than passed as more out-params, because every one of
/// these is "something the body alone cannot tell you" and the list grows.
#[derive(Default)]
pub struct FetchInfo {
    /// The URL the body actually came from, after redirects (RFC 3986
    /// §5.1.3 base URL). Empty if unknown.
    pub final_url: String,
    /// The response's `Content-Type`, verbatim. Empty if the server sent none.
    pub content_type: String,
    /// The final response's status. 0 if the exchange never got that far.
    pub status: u16,
    /// The final response's header block, minus the status line. Only filled
    /// by [`https_request_streaming`] — a GET caller has no use for it and
    /// would pay a copy per sub-resource.
    pub headers: String,
}

/// Reusable HTTPS GET — returns the response body as Vec<u8>.
///
/// Suitable for small responses (manifests, signatures, JSON, < ~32 MB
/// configs). For large downloads use [`https_get_streaming`] instead —
/// this function buffers the entire body in heap and will OOM the
/// kernel on multi-GB inputs.
///
/// Follows up to 3 redirects (301/302/303/307/308). Absolute and
/// relative Location values are both honored; absolute redirects to
/// a different host re-handshake TLS against that host (this is how
/// GitHub Releases work — `github.com/.../releases/download/...`
/// always 302s to a signed `objects.githubusercontent.com` URL).
pub fn https_get(host: &str, path: &str, max_size: usize) -> Result<alloc::vec::Vec<u8>, &'static str> {
    // OTA and module installs land here: signed, already-compressed payloads.
    https_get_ex(host, path, max_size, false)
}

/// As [`https_get`], but the caller says whether it wants the transfer
/// compressed. The browser does; nothing else in the tree does.
pub fn https_get_ex(host: &str, path: &str, max_size: usize, accept_gzip: bool)
    -> Result<alloc::vec::Vec<u8>, &'static str> {
    https_get_req(host, path, max_size,
        &HttpRequest { accept_gzip, ..HttpRequest::default() })
}

/// As [`https_get_ex`], but the caller states the whole request — which is
/// how the browser's sub-resource fallback asks for HTTP/2 and an OTA
/// download does not.
fn https_get_req(host: &str, path: &str, max_size: usize, req: &HttpRequest)
    -> Result<alloc::vec::Vec<u8>, &'static str> {
    let mut cur_host = String::from(host);
    let mut cur_path = String::from(path);
    for _ in 0..4 {
        // Vec-mode: accumulate into out, sink just extends it.
        let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        // Progress heartbeat. The streaming asset path reports every 8 MiB —
        // but a kernel is ~4 MB and a module ~1.4 MB, so NEITHER ever crossed
        // that threshold and both downloaded in complete silence. Over a slow
        // WiFi link that is indistinguishable from a hang, and it is the path
        // every update takes. Step from the expected size so any real download
        // reports about eight times; manifests and signatures never reach
        // 256 KiB and stay quiet.
        let step = core::cmp::max(max_size / 8, 256 * 1024);
        let mut next_report = step;
        let resp = https_get_once(
            &cur_host,
            &cur_path,
            req,
            max_size,
            &mut |chunk: &[u8]| -> Result<(), &'static str> {
                if out.len().saturating_add(chunk.len()) > max_size {
                    out.extend_from_slice(&chunk[..max_size.saturating_sub(out.len())]);
                } else {
                    out.extend_from_slice(chunk);
                }
                if out.len() >= next_report && max_size > 0 {
                    crate::kprintln!("[npk]     {} / {} KiB ({}%)",
                        out.len() / 1024, max_size / 1024,
                        out.len() * 100 / max_size);
                    next_report = out.len() + step;
                }
                Ok(())
            },
        )?;
        match resp.status {
            200..=299 => {
                if out.is_empty() {
                    return Err("empty body");
                }
                return Ok(out);
            }
            301 | 302 | 303 | 307 | 308 => {
                let loc = resp.location.ok_or("redirect without Location")?;
                let (next_host, next_path) = parse_https_url(&loc, &cur_host)?;
                cur_host = next_host;
                cur_path = next_path;
            }
            _ => return Err("HTTP non-2xx response"),
        }
    }
    Err("too many redirects")
}

/// Streaming HTTPS GET — drives body bytes through `on_chunk` as they
/// arrive, never buffering the full payload in memory. The caller
/// chooses where the bytes go (typical: `npkfs::open_streaming_write`
/// then `writer.write(chunk)`).
///
/// Returns the total number of body bytes pushed to the sink. Follows
/// up to 3 redirects, same rules as [`https_get`].
///
/// On non-2xx (other than a 3xx that's followed), the sink is NOT
/// called and an error is returned — so a half-failed download
/// never feeds garbage into the consumer.
pub fn https_get_streaming(
    host: &str,
    path: &str,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<usize, &'static str> {
    // OTA: the payload is already compressed and signed — asking for gzip
    // would be a round of work with nothing at the end of it.
    https_get_streaming_ex(host, path, max_size, on_chunk, None, &HttpRequest::default())
}

/// As [`https_get_streaming`], but also reports what the caller needs to
/// interpret the bytes — see [`FetchInfo`].
///
/// A browser resolves a document's relative URLs against the *final*
/// address (the document base URL, RFC 3986 §5.1.3). Without it every
/// relative sub-resource is requested against the pre-redirect address and
/// pays a second round-trip through the same redirect — which is what drove
/// beak into Wikimedia's rate limit. It equally cannot decode the bytes
/// without the Content-Type.
pub fn https_get_streaming_ex(
    host: &str,
    path: &str,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
    info: Option<&mut FetchInfo>,
    req: &HttpRequest,
) -> Result<usize, &'static str> {
    let n = https_request_streaming(
        host, path, req, max_size, on_chunk, info, false)?;
    if n == 0 {
        return Err("empty body");
    }
    Ok(n)
}

/// The general exchange: any method, any (validated) headers, any body,
/// following redirects.
///
/// `want_headers` fills `FetchInfo::headers` — off for sub-resource GETs,
/// which have no use for it and would pay a copy each.
///
/// `keep_status` decides what a non-2xx means. An OTA download wants an
/// error; a BROWSER wants the bytes, because a 404 page and a 403 explaining
/// why are documents a person needs to read. With it set, the status is
/// reported in `FetchInfo` and the body is delivered whatever it says.
pub fn https_request_streaming(
    host: &str,
    path: &str,
    req: &HttpRequest,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
    mut info: Option<&mut FetchInfo>,
    keep_status: bool,
) -> Result<usize, &'static str> {
    let mut cur_host = String::from(host);
    let mut cur_path = String::from(path);
    // Welches Schema der AKTUELLE Sprung faehrt. Eine Umleitung darf
    // hinaufgehen (Klartext -> TLS), aber niemals hinunter — das ist die
    // Regel, die `parse_https_url` seit jeher durchsetzt, und sie bleibt.
    let mut cur_tls = !req.plain;
    // A redirect can change the method, so the request travels by value from
    // here on (RFC 9110 §15.4).
    let mut method = String::from(req.method);
    let mut body: &[u8] = req.body;
    // Caller headers stop at the first hop that leaves the origin they were
    // written for. The caller computed its `Cookie:` (and anything else
    // sensitive) for THIS host; replaying it to whatever a redirect names
    // hands one site's session to another. We follow redirects on the
    // caller's behalf, so this rule is ours to keep — the same reason the
    // redirect path already refuses an http downgrade.
    let mut carry_headers = true;
    // Every hop's response headers, each under a `:hop <url>` marker, so the
    // caller can scope what it finds to the response that actually sent it.
    // A cookie set by the 303 of a login POST lives HERE and nowhere else:
    // reporting only the final response's headers drops it, and the login
    // silently does not take.
    let mut hops = String::new();
    for _ in 0..4 {
        let mut total: usize = 0;
        let no_headers: [String; 0] = [];
        let hop = HttpRequest {
            method: &method,
            headers: if carry_headers { req.headers } else { &no_headers },
            body,
            accept_gzip: req.accept_gzip && cur_tls,
            try_h2: req.try_h2 && cur_tls,
            plain: !cur_tls,
        };
        let resp = https_get_once(
            &cur_host,
            &cur_path,
            &hop,
            max_size,
            &mut |chunk: &[u8]| -> Result<(), &'static str> {
                on_chunk(chunk)?;
                total = total.saturating_add(chunk.len());
                Ok(())
            },
        )?;
        let done = match resp.status {
            200..=299 => true,
            301 | 302 | 303 | 307 | 308 => false,
            // Any other status is a document too, once the caller asked for
            // it. Without `keep_status` this stays the old hard error.
            _ if keep_status => true,
            _ => return Err("HTTP non-2xx response"),
        };
        if done {
            if let Some(out) = info.as_deref_mut() {
                out.final_url.clear();
                out.final_url.push_str("https://");
                out.final_url.push_str(&cur_host);
                out.final_url.push_str(&cur_path);
                out.content_type.clear();
                if let Some(ct) = &resp.content_type {
                    out.content_type.push_str(ct);
                }
                out.status = resp.status;
                // The header blocks are copied only for the caller that asked
                // to see statuses — the browser. A page load fans out to ~20
                // sub-resource GETs, and none of them has any use for a
                // second copy of their headers.
                out.headers.clear();
                if keep_status {
                    out.headers.push_str(&hops);
                    push_hop(&mut out.headers, &cur_host, &cur_path, &resp.headers);
                }
            }
            return Ok(total);
        }
        // On 3xx the inner once-fn returns early without calling the sink, so
        // the consumer never sees any bytes from the redirect response. Safe
        // to retry against the Location target with a fresh TLS session.
        let loc = resp.location.ok_or("redirect without Location")?;
        // A redirect's OWN headers matter: a login POST is answered with a
        // 303 that carries `Set-Cookie: session=…`, and the page it points at
        // is only reachable because of it. Keeping just the last response's
        // headers threw the session away and the login quietly did not take.
        if keep_status {
            push_hop(&mut hops, &cur_host, &cur_path, &resp.headers);
        }
        // Auf TLS gilt weiter die strenge Fassung: sie verweigert jedes
        // `http://` im `Location`. Faehrt der Lauf schon im Klartext, darf das
        // Ziel auch `https://` sein — hinauf ist erlaubt.
        let (next_host, next_path, next_tls) = if cur_tls {
            let (h, p) = parse_https_url(&loc, &cur_host)?;
            (h, p, true)
        } else {
            parse_any_url(&loc, &cur_host, false)?
        };
        if next_host != cur_host {
            carry_headers = false;
        }
        cur_host = next_host;
        cur_path = next_path;
        cur_tls = next_tls;
        // 303 says so outright, and 301/302 after a POST is the behaviour
        // every browser settled on (RFC 9110 §15.4.3 note): the redirect
        // points at a RESULT page, and re-POSTing the form to it would
        // submit twice. 307/308 exist precisely to keep method and body.
        if matches!(resp.status, 301 | 302 | 303) && method != "GET" && method != "HEAD" {
            method = String::from("GET");
            body = &[];
        }
    }
    Err("too many redirects")
}

// ── HTTPS keep-alive connection pool ───────────────────────────────
// A page load in beak fans out to ~20 sub-resources (CSS, images), most
// from one or two hosts. Without reuse each pays a full fresh DNS + TCP
// + TLS handshake, serially — the "Zeitlupe" page load Florian saw on
// the serial log. Holding the TLS session open and sending
// `Connection: keep-alive` collapses those ~20 handshakes to ~1 per host.
//
// A session is returned to the pool ONLY when its response was fully
// framed (Content-Length or chunked, and we read the whole body off the
// wire) and the peer did not signal `Connection: close`. Otherwise the
// message boundary on the wire is unknown and reuse would desync the
// byte stream.
struct PooledConn {
    host: String,
    tls: crate::tls::TlsSession,
    /// When it went idle. A server keeps a connection open for 5-75 s and
    /// then closes it; `is_healthy` sees only the LOCAL TCP state, so a peer
    /// that hung up quietly still looks alive here. The age is what catches
    /// that — one wasted round-trip per stale socket, and the delayed-ACK-
    /// shaped 230 ms header times all sat on pooled connections.
    idle_since: u64,
}

/// How long a pooled connection may sit unused. Under every common server
/// keep-alive (nginx 75 s, most CDNs 5-10 s), above any page load's own
/// fan-out — the reuse we actually want is measured in the same second.
const POOL_MAX_IDLE_TICKS: u64 = 500; // 5 s at 100 Hz
const CONN_POOL_SIZE: usize = 8;
static CONN_POOL: spin::Mutex<[Option<PooledConn>; CONN_POOL_SIZE]> =
    spin::Mutex::new([const { None }; CONN_POOL_SIZE]);

/// Take a *live* pooled session for `host`, if any. A session the server
/// has since closed (idle-timeout FIN → state left `Established`) is
/// closed and skipped here, so the caller never sends on a dead socket.
/// Vor jedem Griff in einen Pool: den NIC-Ring leeren.
///
/// `conn_healthy` liest den VERBINDUNGSZUSTAND, und der aendert sich nur,
/// wenn ein Paket verarbeitet wurde. Zwischen Einlegen und Herausnehmen
/// pollt niemand — das FIN des Servers liegt also unbearbeitet im Ring, der
/// Zustand sagt weiter `Established`, und die Pruefung nickt eine tote
/// Verbindung durch. Gemessen: `0/4 over one connection` auf
/// thumb.wikimedia.org, reproduzierbar, weil der Server nach jeder bedienten
/// Runde zumacht.
///
/// Ein Aufruf, und die Pruefung sieht, was schon angekommen ist.
fn drain_before_reuse() {
    crate::net::poll_rx_only();
}

fn pool_take(host: &str) -> Option<crate::tls::TlsSession> {
    drain_before_reuse();
    let mut pool = CONN_POOL.lock();
    let now = crate::interrupts::ticks();
    for slot in pool.iter_mut() {
        if matches!(slot, Some(c) if c.host == host) {
            let c = slot.take().unwrap();
            let fresh = now.wrapping_sub(c.idle_since) < POOL_MAX_IDLE_TICKS;
            let mut tls = c.tls;
            if fresh && tls.is_healthy() {
                return Some(tls);
            }
            // Sagen, dass es gegriffen hat — sonst ist "kein Haenger mehr"
            // nicht von "der Fall trat nicht ein" zu unterscheiden.
            kprintln!("[npk]   pool {}: Verbindung verworfen ({})", host,
                if fresh { "Gegenstelle hat zugemacht" } else { "zu lange ungenutzt" });
            let _ = crate::tls::tls_close(&mut tls);
            return None;
        }
    }
    None
}

/// Return a reusable session to the pool. Evicts (and closes) the first
/// slot when the pool is full.
fn pool_put(host: &str, tls: crate::tls::TlsSession) {
    let mut pool = CONN_POOL.lock();
    let conn = PooledConn {
        host: String::from(host),
        tls,
        idle_since: crate::interrupts::ticks(),
    };
    for slot in pool.iter_mut() {
        if slot.is_none() {
            *slot = Some(conn);
            return;
        }
    }
    if let Some(mut old) = pool[0].replace(conn) {
        let _ = crate::tls::tls_close(&mut old.tls);
    }
}

/// True if `value` (an HTTP list header) contains `token` as a
/// comma-separated element, case-insensitively.
fn header_has_token(value: &str, token: &str) -> bool {
    value.split(',').any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// Either pool `tls` for reuse or close it — exactly one, never both.
fn finish_conn(host: &str, tls: crate::tls::TlsSession, reusable: bool) {
    if reusable {
        pool_put(host, tls);
    } else {
        let mut t = tls;
        let _ = crate::tls::tls_close(&mut t);
    }
}

/// Classify a fetch failure into a stable token.
///
/// The message itself is written for a human reading the serial log and is
/// free to be reworded; this token is the contract a client branches on.
/// Keeping them apart means improving a message never silently changes
/// which error page the browser shows.
pub fn error_kind(msg: &str) -> &'static str {
    match msg {
        "certificate: untrusted root CA" => "cert.untrusted",
        "certificate: expired" => "cert.expired",
        "certificate: not yet valid" => "cert.not_yet_valid",
        "certificate: hostname mismatch" => "cert.hostname",
        "DNS resolution failed" => "dns.failed",
        "TCP connect failed" | "connection failed" => "net.connect",
        "recv timeout" => "net.timeout",
        "recv error" | "connection reset after handshake" => "net.reset",
        "empty body" => "http.empty",
        "HTTP non-2xx response" => "http.status",
        "cancelled" => "cancelled",
        m if m.starts_with("certificate:") => "cert.invalid",
        // Matched against the constants, not copies of the text — see
        // `tls::reasons`. The peer aborting the handshake is the common
        // shape here (a server that dislikes our ClientHello), and it is
        // NOT a certificate problem, so it must not read like one.
        crate::tls::reasons::HANDSHAKE_REJECTED
        | crate::tls::reasons::VERSION_UNSUPPORTED
        | crate::tls::reasons::INSUFFICIENT_SECURITY
        | crate::tls::reasons::SERVER_ALERT => "tls.handshake",
        m if m.starts_with("TLS ") => "tls.protocol",
        m if m.starts_with("redirect") => "http.redirect",
        "too many redirects" | "refusing http downgrade" => "http.redirect",
        m if m.starts_with("empty url") || m.starts_with("invalid url") => "url.invalid",
        _ => "unknown",
    }
}

/// Fresh DNS + ARP + TCP + TLS to `host:443`. Only paid on a pool miss.
fn open_tls(host: &str) -> Result<crate::tls::TlsSession, &'static str> {
    // Der nackte Name fuer DNS und fuer die TLS-Kennung (SNI); der Port, wenn
    // einer dasteht, sonst 443.
    let (bare, port) = split_host_port(host);
    let port = port.unwrap_or(443);
    let t_dns = crate::interrupts::ticks();
    let ip = if let Some(ip) = parse_ip(bare) {
        ip
    } else {
        crate::net::dns::resolve(bare).ok_or("DNS resolution failed")?
    };
    let t_arp = crate::interrupts::ticks();
    // Make sure the gateway's MAC is known before we SYN.
    //
    // This used to fire an ARP request and then spin 50_000 times over the
    // FULL `net::poll()` — unconditionally, even when the MAC was already
    // cached, and `net::poll()` also runs a shade render pass. So every fresh
    // connect paid 50_000 render passes, and the price grew with whatever the
    // browser had on screen: handshakes measured 350 ms against an empty page
    // and 2250 ms once a real article was painted. `arp::resolve` returns
    // immediately on a cache hit and otherwise polls only until the reply
    // lands.
    let gw = crate::net::ipv4::gateway();
    let _ = crate::net::arp::resolve(gw, 100); // 1 s at 100 Hz
    let t_tcp = crate::interrupts::ticks();

    let handle = crate::net::tcp::connect(ip, port).map_err(|_| "TCP connect failed")?;
    let t_tls = crate::interrupts::ticks();
    let out = match crate::tls::tls_connect(handle, bare) {
        Ok(s) => Ok(s),
        Err(e) => {
            // Keep the reason. Collapsing every handshake failure into one
            // message is what left a browser with nothing to say but a blank
            // page — "untrusted root CA" and "expired" are the two things the
            // person in front of the screen actually needs to be told apart.
            kprintln!("[npk] TLS error: {}", e);
            let _ = crate::net::tcp::close(handle);
            Err(e.reason())
        }
    };
    // Split so a slow connect names its own culprit. The whole thing swings
    // between ~200 ms and ~2100 ms across runs, and 2 s is suspiciously
    // exactly a retransmission timeout — this says which leg waits.
    let done = crate::interrupts::ticks();
    if chatty() {
        kprintln!("[npk]   connect {} -> {}.{}.{}.{}:443 (dns {} + arp {} + tcp {} + tls {} ms)",
            host, ip[0], ip[1], ip[2], ip[3],
            t_arp.wrapping_sub(t_dns) * 10, t_tcp.wrapping_sub(t_arp) * 10,
            t_tls.wrapping_sub(t_tcp) * 10, done.wrapping_sub(t_tls) * 10);
    }
    out
}

// ── HTTP/2 ──────────────────────────────────────────────────────────────────
//
// Two callers: the browser's batch fetch for sub-resources (`get_all`, whole
// bodies buffered) and the document fetch (`request`, streamed into the
// caller's sink). NOT OTA — see `HttpRequest::try_h2` for why that is a
// decision about blast radius rather than about the protocol.

use super::http2::{self, Http2};

const H2_POOL_SIZE: usize = 4;
static H2_POOL: spin::Mutex<[Option<(String, Http2, u64)>; H2_POOL_SIZE]> =
    spin::Mutex::new([const { None }; H2_POOL_SIZE]);

/// Hosts that turned out not to speak h2. Without this, every batch pays a
/// fresh TLS handshake to re-learn the same answer.
static H2_REFUSED: spin::Mutex<[Option<String>; H2_POOL_SIZE]> =
    spin::Mutex::new([const { None }; H2_POOL_SIZE]);

fn h2_refused(host: &str) -> bool {
    H2_REFUSED.lock().iter().any(|s| s.as_deref() == Some(host))
}

fn mark_h2_refused(host: &str) {
    let mut list = H2_REFUSED.lock();
    if list.iter().any(|s| s.as_deref() == Some(host)) {
        return;
    }
    for slot in list.iter_mut() {
        if slot.is_none() {
            *slot = Some(String::from(host));
            return;
        }
    }
    list[0] = Some(String::from(host));
}

fn h2_take(host: &str) -> Option<Http2> {
    drain_before_reuse();
    let mut pool = H2_POOL.lock();
    let now = crate::interrupts::ticks();
    for slot in pool.iter_mut() {
        if matches!(slot, Some((h, _, _)) if h == host) {
            let (_, mut conn, idle_since) = slot.take().unwrap();
            // Same rule as the HTTP/1.1 pool: a GOAWAY we have not read yet
            // is indistinguishable from a quiet connection.
            let fresh = now.wrapping_sub(idle_since) < POOL_MAX_IDLE_TICKS;
            if fresh && conn.is_healthy() {
                // Der Nehmer soll wissen, dass er wettet: eine Gegenstelle,
                // die zwischen zwei Benutzungen still weggeht, sendet weder
                // FIN noch RST — vorhersagen laesst sich das nicht, nur
                // schneller merken.
                conn.reused = true;
                return Some(conn);
            }
            kprintln!("[npk]   h2 pool {}: Verbindung verworfen ({})", host,
                if fresh { "Gegenstelle hat zugemacht" } else { "zu lange ungenutzt" });
            conn.close();
            return None;
        }
    }
    None
}

fn h2_put(host: &str, conn: Http2) {
    let mut pool = H2_POOL.lock();
    let entry = (String::from(host), conn, crate::interrupts::ticks());
    for slot in pool.iter_mut() {
        if slot.is_none() {
            *slot = Some(entry);
            return;
        }
    }
    if let Some((_, mut old, _)) = pool[0].replace(entry) {
        old.close();
    }
}

/// A connection for `host`: the pooled one if it is still fresh, otherwise a
/// new one. The batch fetch's entry point.
fn h2_open(host: &str) -> Option<Http2> {
    h2_take(host).or_else(|| h2_connect(host))
}

/// A NEW connection — no pool. Split out because the document path needs to
/// be able to say "not that one": a pooled connection can carry a GOAWAY we
/// have not read yet, and giving up on h2 for that would put the document
/// back under the HTTP/1.1 rate limit this whole path exists to leave.
fn h2_connect(host: &str) -> Option<Http2> {
    // Everything before `t_dns` used to be the one unmeasured stretch of the
    // connect, and a 2026-08-14 device log showed 2100 ms connect with every
    // named leg at 90 ms. Do not "explain" that gap from a single clean run
    // again — measure it.
    let t_enter = crate::interrupts::ticks();
    if h2_refused(host) {
        return None;
    }
    let t_dns = crate::interrupts::ticks();
    let (bare, port) = split_host_port(host);
    let port = port.unwrap_or(443);
    let ip = parse_ip(bare).or_else(|| crate::net::dns::resolve(bare))?;
    let t_arp = crate::interrupts::ticks();
    // Gateway MAC, same as the h1 path (see `open_tls` for why this is a
    // resolve and not a spin).
    let gw = crate::net::ipv4::gateway();
    let _ = crate::net::arp::resolve(gw, 100);
    let t_conn = crate::interrupts::ticks();
    if chatty() {
        kprintln!("[npk]   h2 pre-connect {} (refused-check {} + dns {} + arp {} ms)",
            host, t_dns.wrapping_sub(t_enter) * 10,
            t_arp.wrapping_sub(t_dns) * 10, t_conn.wrapping_sub(t_arp) * 10);
    }
    // The serial write above is itself unmeasured otherwise, and it sits
    // INSIDE the span the caller reports as "connect".
    let t_pre = crate::interrupts::ticks();
    let r = http2::connect(bare, ip, port);
    if chatty() {
        let after = crate::interrupts::ticks();
        kprintln!("[npk]   h2 connect() span {} ms (log {} ms before it)",
            after.wrapping_sub(t_pre) * 10, t_pre.wrapping_sub(t_conn) * 10);
    }
    match r {
        Ok(c) => Some(c),
        Err(http2::Http2Error::NotNegotiated) => {
            mark_h2_refused(host);
            None
        }
        // Every other h2 failure falls back to HTTP/1.1 silently, which is the
        // right behaviour but a terrible way to debug: the variants carry the
        // reason, so say it when asked.
        Err(e) => {
            if chatty() {
                kprintln!("[npk]   h2 {} failed ({:?}) — falling back to HTTP/1.1", host, e);
            }
            None
        }
    }
}

/// Fetch many URLs at once, multiplexed over one HTTP/2 connection per host
/// where the peer offers h2, and falling back to the existing sequential
/// HTTP/1.1 path where it does not.
///
/// Results are positional: entry `i` corresponds to `urls[i]`, and is `None`
/// if that resource could not be fetched. A response with a 4xx/5xx status
/// counts as a failure rather than returning the error page's body — the
/// caller is loading images and stylesheets, and decoding an HTML error page
/// as a PNG helps nobody.
pub fn https_get_many(urls: &[String], max_size: usize) -> alloc::vec::Vec<Option<alloc::vec::Vec<u8>>> {
    let mut out: alloc::vec::Vec<Option<alloc::vec::Vec<u8>>> = urls.iter().map(|_| None).collect();

    // Split into per-host groups, keeping the original positions.
    // Das Schema bleibt DABEI: ein Klartext-Host kann kein h2 (h2c sprechen
    // wir nicht) und muss den einfachen Weg nehmen. Es wegzuwerfen hiesse,
    // jede Unterressource einer lokalen Vorlage gegen :443 zu versuchen.
    let mut parsed: alloc::vec::Vec<Option<(String, String, bool)>> = alloc::vec::Vec::new();
    for u in urls {
        parsed.push(parse_url(u).ok());
    }
    let mut hosts: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    for p in parsed.iter().flatten() {
        if !hosts.contains(&p.0) {
            hosts.push(p.0.clone());
        }
    }

    for host in &hosts {
        let idxs: alloc::vec::Vec<usize> = parsed
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p, Some((h, _, _)) if h == host))
            .map(|(i, _)| i)
            .collect();
        let plain = matches!(parsed.iter().flatten().find(|p| &p.0 == host), Some((_, _, false)));

        let mut served = false;
        let t0 = crate::interrupts::ticks();
        if let Some(mut conn) = (if plain { None } else { h2_open(host) }) {
            let t_conn = crate::interrupts::ticks();
            let paths: alloc::vec::Vec<&str> =
                idxs.iter().map(|&i| parsed[i].as_ref().unwrap().1.as_str()).collect();
            match conn.get_all(host, &paths, USER_AGENT, true) {
                Ok(results) => {
                    let mut ok = 0usize;
                    let (mut gz_raw, mut gz_out) = (0usize, 0usize);
                    for (slot, res) in idxs.iter().zip(results) {
                        match res {
                            Ok(r) if (200..300).contains(&r.status) => {
                                // h2 buffers a stream's DATA frames into one
                                // Vec, so there is nothing to stream here —
                                // but the same cap applies.
                                let gz = r.header("content-encoding")
                                    .map(|v| v.trim().eq_ignore_ascii_case("gzip"))
                                    .unwrap_or(false);
                                let body = if gz {
                                    match crate::intent::gzip::inflate_all(&r.body, max_size) {
                                        Ok(b) => {
                                            gz_raw += r.body.len();
                                            gz_out += b.len();
                                            Some(b)
                                        }
                                        Err(e) => {
                                            kprintln!("[npk]   h2 gzip: {}", e);
                                            None
                                        }
                                    }
                                } else {
                                    Some(r.body)
                                };
                                // A damaged stream leaves the slot unset, so
                                // the h1 fallback below picks the URL up again
                                // — the same door a redirect goes through.
                                if let Some(b) = body {
                                    ok += 1;
                                    out[*slot] = Some(b);
                                }
                            }
                            // A redirect needs the h1 path's follow logic (it
                            // may cross hosts); leave it unset and let the
                            // per-URL fallback below pick it up.
                            Ok(r) if (300..400).contains(&r.status) => {
                                kprintln!("[npk]   h2 {} -> {}", r.status,
                                    r.header("location").unwrap_or("(no location)"));
                            }
                            Ok(r) => kprintln!("[npk]   h2 HTTP {}", r.status),
                            Err(e) => kprintln!("[npk]   h2 stream failed: {:?}", e),
                        }
                    }
                    // Split the time so a slow batch says WHERE it was slow:
                    // a fresh TLS handshake, or the transfer itself.
                    let now = crate::interrupts::ticks();
                    kprintln!("[npk]   h2 {}: {}/{} over one connection ({} ms connect + {} ms transfer)",
                        host, ok, idxs.len(),
                        t_conn.wrapping_sub(t0) * 10, now.wrapping_sub(t_conn) * 10);
                    if gz_raw > 0 {
                        let r10 = gz_out * 10 / gz_raw;
                        kprintln!("[npk]   h2 {}: gzip {} -> {} B ({}.{}x)",
                            host, gz_raw, gz_out, r10 / 10, r10 % 10);
                    }
                    served = true;
                    if conn.is_healthy() {
                        h2_put(host, conn);
                    } else {
                        conn.close();
                    }
                }
                Err(e) => {
                    kprintln!("[npk]   h2 {} failed ({:?}) — falling back to HTTP/1.1", host, e);
                    conn.close();
                }
            }
        }

        // Anything h2 did not deliver — no h2, a protocol error, or a
        // redirect — falls back to the ordinary sequential fetch.
        for &i in &idxs {
            if out[i].is_some() {
                continue;
            }
            let (h, p, tls) = parsed[i].as_ref().unwrap();
            // Still h2 where the host offers it — this door is the one a
            // redirect goes through, and a redirected sub-resource is no
            // less throttled than a direct one.
            let req = HttpRequest { accept_gzip: *tls, try_h2: *tls, plain: !*tls,
                                    ..HttpRequest::default() };
            if let Ok(body) = https_get_req(h, p, max_size, &req) {
                out[i] = Some(body);
            }
        }
        let _ = served;
    }
    out
}

/// Exchange failure mode. `Retry` = failed in the send/header phase,
/// before any body byte reached the sink → safe to retry on a fresh
/// connection (this is how a stale pooled socket surfaces, and how an h2
/// attempt hands the request to HTTP/1.1). `Fatal` = failed mid-body or a
/// protocol error → propagate, because a retry would deliver twice.
enum ExchangeErr {
    Retry,
    Fatal(&'static str),
}

// ── The document fetch over HTTP/2 ──────────────────────────────────────────

/// Adapter between an h2 stream and the sink the caller handed us. It exists
/// so that nothing above `https_get_once` has to know which protocol carried
/// the bytes: same clipping, same gzip, same "a 3xx never reaches the sink".
struct H2Sink<'a> {
    max_size: usize,
    delivered: usize,
    /// A 3xx body is courtesy text. `https_request_streaming` follows the
    /// redirect instead and counts on the sink having stayed untouched.
    ///
    /// Starts TRUE and is decided in `head`: DATA before HEADERS is a broken
    /// (or hostile) peer, and the safe reading of "we do not know what this
    /// response is yet" is that the caller may not have it.
    discard: bool,
    gunzip: Option<crate::intent::gzip::GzipInflate>,
    on_chunk: &'a mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
    /// Set once the body has started. After that, falling back to HTTP/1.1
    /// would deliver the document twice.
    touched: bool,
    /// When the response headers landed, so the trace splits the wait from
    /// the transfer the way the HTTP/1.1 path does.
    t_head: u64,
}

impl http2::BodySink for H2Sink<'_> {
    fn head(&mut self, status: u16, headers: &[http2::Header]) -> Result<(), &'static str> {
        self.t_head = crate::interrupts::ticks();
        self.discard = (300..400).contains(&status);
        // Same link in the chain as HTTP/1.1 puts it: between the transport
        // and the sink, streaming, with the caller's own cap as the budget —
        // a zip bomb is then no more dangerous than a body of the size the
        // caller already said it could take.
        if headers.iter().any(|h| {
            h.name == "content-encoding" && h.value.trim().eq_ignore_ascii_case("gzip")
        }) {
            self.gunzip = Some(crate::intent::gzip::GzipInflate::new(self.max_size));
        }
        Ok(())
    }

    fn data(&mut self, chunk: &[u8]) -> Result<(), &'static str> {
        if self.discard || chunk.is_empty() {
            return Ok(());
        }
        self.touched = true;
        let Self { max_size, delivered, gunzip, on_chunk, .. } = self;
        let mut clip = |c: &[u8]| -> Result<(), &'static str> {
            if *delivered >= *max_size {
                return Ok(());
            }
            let take = c.len().min(*max_size - *delivered);
            *delivered += take;
            on_chunk(&c[..take])
        };
        match gunzip.as_mut() {
            Some(g) => g.feed(chunk, &mut clip),
            None => clip(chunk),
        }
    }
}

/// The response header block in the shape every caller above already reads:
/// one `Name: value` line per field, capped.
///
/// Pseudo-headers are dropped for the same reason `capture_headers` drops
/// colon-prefixed lines — no HTTP field name may start with a colon, so
/// nothing legitimate is lost, and it is what stops a server from writing
/// its own `:hop` marker into the block.
fn h2_header_block(headers: &[http2::Header]) -> String {
    let mut out = String::new();
    for h in headers {
        if h.name.starts_with(':')
            || out.len() + h.name.len() + h.value.len() + 4 > MAX_REPLY_HEADERS
        {
            continue;
        }
        out.push_str(&h.name);
        out.push_str(": ");
        out.push_str(&h.value);
        out.push_str("\r\n");
    }
    out
}

/// Field lookup. h2 field names are lowercase on the wire (§8.2.1), so this
/// is an exact match and not a case-insensitive one.
fn h2_value<'a>(headers: &'a [http2::Header], name: &str) -> Option<&'a str> {
    headers.iter().find(|h| h.name == name).map(|h| h.value.as_str())
}

/// One HTTPS round-trip over HTTP/2, when the host speaks it.
///
/// `Retry` means nothing was delivered — no h2 here, no connection, or a
/// failure before the first body byte — so the caller may run the same
/// request over HTTP/1.1. `Fatal` means the sink has already seen bytes.
///
/// Redirects come back exactly as the HTTP/1.1 path hands them back: status
/// plus Location, body dropped. Following them stays one layer up, in
/// `https_request_streaming`, which is where the method switch, the
/// origin-crossing header rule and the per-hop `Set-Cookie` already live —
/// and every login runs through all three.
fn h2_once(
    host: &str,
    path: &str,
    req: &HttpRequest,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<HttpResponse, ExchangeErr> {
    // Attempt 1: the pooled connection. Same ladder as HTTP/1.1 below, for
    // the same reason — the peer may have hung up while it sat idle.
    if let Some(conn) = h2_take(host) {
        match h2_exchange(host, path, req, max_size, on_chunk, conn) {
            Ok(r) => return Ok(r),
            Err(ExchangeErr::Fatal(e)) => return Err(ExchangeErr::Fatal(e)),
            Err(ExchangeErr::Retry) => {}
        }
    }
    // Attempt 2: a fresh one. `None` here means this host does not speak h2.
    let Some(conn) = h2_connect(host) else {
        return Err(ExchangeErr::Retry);
    };
    h2_exchange(host, path, req, max_size, on_chunk, conn)
}

/// The exchange itself, over a connection the caller already holds.
fn h2_exchange(
    host: &str,
    path: &str,
    req: &HttpRequest,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
    mut conn: Http2,
) -> Result<HttpResponse, ExchangeErr> {
    let t_send = crate::interrupts::ticks();
    let mut sink = H2Sink {
        max_size,
        delivered: 0,
        discard: true,
        gunzip: None,
        on_chunk,
        touched: false,
        t_head: t_send,
    };
    let result = conn.request(
        host, req.method, path, req.headers, req.body,
        USER_AGENT, req.accept_gzip, &mut sink,
    );
    let (touched, t_head) = (sink.touched, sink.t_head);
    let gz = sink.gunzip.as_ref().map(|g| g.ratio());

    match result {
        Ok(headers) => {
            if conn.is_healthy() {
                h2_put(host, conn);
            } else {
                conn.close();
            }
            let status = h2_value(&headers, ":status")
                .and_then(|v| v.parse::<u16>().ok())
                .unwrap_or(0);
            let location = h2_value(&headers, "location").map(String::from);
            let content_type = h2_value(&headers, "content-type").map(String::from);
            let redirect = (300..400).contains(&status);
            if chatty() {
                let now = crate::interrupts::ticks();
                let hdr_ms = t_head.wrapping_sub(t_send) * 10;
                match (&location, redirect) {
                    (Some(l), true) =>
                        kprintln!("[npk]   h2 HTTP {} (headers {} ms) -> {}", status, hdr_ms, l),
                    _ => kprintln!("[npk]   h2 HTTP {} {} — headers {} ms + body {} ms",
                        status, req.method, hdr_ms, now.wrapping_sub(t_head) * 10),
                }
                // Say that gzip ran. Silence is ambiguous three ways — never
                // asked, server answered identity, or the path lost it — and
                // the whole point of asking is a number.
                match (gz, req.accept_gzip, redirect) {
                    (Some((raw, out)), _, _) => {
                        let r10 = out * 10 / core::cmp::max(raw, 1);
                        kprintln!("[npk]   gzip {} -> {} B ({}.{}x)", raw, out, r10 / 10, r10 % 10);
                    }
                    (None, true, false) => kprintln!("[npk]   gzip asked, server answered identity"),
                    _ => {}
                }
            }
            Ok(HttpResponse { status, location, content_type, headers: h2_header_block(&headers) })
        }
        Err(e) => {
            conn.close();
            if chatty() {
                kprintln!("[npk]   h2 {}{} failed ({:?})", host, path, e);
            }
            if touched {
                Err(ExchangeErr::Fatal("h2 stream failed mid-body"))
            } else {
                Err(ExchangeErr::Retry)
            }
        }
    }
}

/// Discard a redirect's (small) body so the socket is left at a clean
/// message boundary and can be reused. Returns true iff the whole body
/// was consumed.
fn drain_body(
    tls: &mut crate::tls::TlsSession,
    leading: &[u8],
    content_length: Option<usize>,
    chunked: bool,
    buf: &mut [u8],
) -> bool {
    if let Some(cl) = content_length {
        let mut got = core::cmp::min(leading.len(), cl);
        while got < cl {
            match tls_recv_poll(tls, buf) {
                Ok(0) => continue,
                Ok(n) => got += core::cmp::min(n, cl - got),
                Err(_) => return false,
            }
        }
        true
    } else if chunked {
        let mut sink = |_: &[u8]| -> Result<(), &'static str> { Ok(()) };
        stream_chunked_body(leading, tls, buf, usize::MAX, &mut sink).is_ok()
    } else {
        // A redirect with no framing → boundary unknown; not reusable.
        false
    }
}

/// One HTTPS round-trip over an OWNED TLS session (`Connection:
/// keep-alive`). Reads + parses the response, streams the body through
/// `on_chunk`, and hands the session back to the pool (if cleanly
/// reusable) or closes it. No redirect following — the returned
/// `HttpResponse` carries status + Location for the caller to follow.
fn https_exchange(
    host: &str,
    path: &str,
    req: &HttpRequest,
    max_size: usize,
    mut tls: crate::tls::TlsSession,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<HttpResponse, ExchangeErr> {
    let mut head = alloc::format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nConnection: keep-alive\r\n",
        req.method, path, host, USER_AGENT
    );
    if req.accept_gzip {
        head.push_str("Accept-Encoding: gzip\r\n");
    }
    for line in req.headers {
        head.push_str(line);
        head.push_str("\r\n");
    }
    // We state the length ourselves — always, when there is a body, and never
    // from a caller-supplied header (`RESERVED_HEADERS`). Announcing a length
    // that disagrees with the bytes we then send is how a request gets split
    // in two on the far side.
    if !req.body.is_empty() {
        head.push_str(&alloc::format!("Content-Length: {}\r\n", req.body.len()));
    }
    head.push_str("\r\n");
    let mut request = head.into_bytes();
    request.extend_from_slice(req.body);
    // h2 reports connect/transfer separately; h1 reported NOTHING between the
    // handshake and "receiving body", which is where a 6.5 s document fetch
    // hid on 2026-08-14 with every measured leg at 90 ms.
    let t_send = crate::interrupts::ticks();
    if crate::tls::tls_send(&mut tls, &request).is_err() {
        // Stale pooled socket (or a send error) — nothing delivered, retry fresh.
        let _ = crate::tls::tls_close(&mut tls);
        return Err(ExchangeErr::Retry);
    }

    // ── Phase 1: read the header block (up to \r\n\r\n) ──
    let mut raw = alloc::vec::Vec::new();
    let mut buf = [0u8; 17000]; // >= max TLS record (16KB)
    // The loop leaves only two ways: it breaks WITH the header offset, or it
    // returns. Yielding the offset out of `break` says that in the types, so
    // there is no "we got here without a header" case left to handle.
    let t_hdr0 = crate::interrupts::ticks();
    let hdr_end = loop {
        match tls_recv_poll(&mut tls, &mut buf) {
            Ok(0) => continue,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break pos;
                }
            }
            Err(_) => {
                // A live server always answers — no header means the socket
                // was dead/stale. No body delivered yet → safe to retry fresh.
                let _ = crate::tls::tls_close(&mut tls);
                return Err(ExchangeErr::Retry);
            }
        }
        if raw.len() > 32_768 {
            let _ = crate::tls::tls_close(&mut tls);
            return Err(ExchangeErr::Fatal("headers too large"));
        }
    };
    let body_start = hdr_end + 4;

    // ── Phase 2: parse status + framing ──
    let hdr_str = match core::str::from_utf8(&raw[..hdr_end]) {
        Ok(s) => s,
        Err(_) => {
            let _ = crate::tls::tls_close(&mut tls);
            return Err(ExchangeErr::Fatal("invalid header encoding"));
        }
    };
    let status = parse_status_code(hdr_str).unwrap_or(0);
    // Everything after the status line, capped. `Set-Cookie` repeats, so
    // handing back parsed single values could never carry it.
    let reply_headers = capture_headers(hdr_str);
    let location = parse_header_value(hdr_str, "location").map(String::from);
    let content_type = parse_header_value(hdr_str, "content-type").map(String::from);
    let content_length = parse_header_value(hdr_str, "content-length")
        .and_then(|v| v.trim().parse::<usize>().ok());
    let chunked = parse_header_value(hdr_str, "transfer-encoding")
        .map(|v| v.contains("chunked"))
        .unwrap_or(false);
    let conn_hdr = parse_header_value(hdr_str, "connection");
    let explicit_close = conn_hdr.map(|v| header_has_token(v, "close")).unwrap_or(false);
    let explicit_keepalive = conn_hdr.map(|v| header_has_token(v, "keep-alive")).unwrap_or(false);
    // HTTP/1.1 is persistent by default; HTTP/1.0 is not.
    let http10 = hdr_str.starts_with("HTTP/1.0");
    let persistent = if explicit_close { false }
        else if explicit_keepalive { true }
        else { !http10 };

    let leading = &raw[body_start..];

    // ── Phase 3: consume the body ──
    // Redirect: drain the courtesy body so the socket is clean, then hand
    // the Location back to the caller (no bytes go to the sink).
    if (300..400).contains(&status) {
        if chatty() {
            let (send_ms, hdr_ms) = (t_hdr0.wrapping_sub(t_send) * 10,
                                     crate::interrupts::ticks().wrapping_sub(t_hdr0) * 10);
            match &location {
                Some(l) => kprintln!("[npk]   HTTP {} (send {} + headers {} ms) -> {}", status, send_ms, hdr_ms, l),
                None => kprintln!("[npk]   HTTP {} (redirect, no Location)", status),
            }
        }
        let drained = drain_body(&mut tls, leading, content_length, chunked, &mut buf);
        finish_conn(host, tls, persistent && drained);
        return Ok(HttpResponse { status, location, content_type, headers: reply_headers });
    }
    let t_body0 = crate::interrupts::ticks();
    if chatty() {
        kprintln!("[npk]   HTTP {} — send {} ms + headers {} ms, receiving body",
            status,
            t_hdr0.wrapping_sub(t_send) * 10,
            t_body0.wrapping_sub(t_hdr0) * 10);
    }

    // `Content-Encoding: gzip` — inflate between the transport and the sink,
    // streaming, so the compressed body is never held a second time. The cap
    // handed to the inflater is the caller's own `max_size`: a zip bomb is
    // then no more dangerous than an uncompressed body of the size the caller
    // already said it could take, and it is clipped in the same place.
    let mut gunzip = match parse_header_value(hdr_str, "content-encoding") {
        Some(v) if v.trim().eq_ignore_ascii_case("gzip") =>
            Some(crate::intent::gzip::GzipInflate::new(max_size)),
        _ => None,
    };
    let mut sink = |chunk: &[u8]| -> Result<(), &'static str> {
        match gunzip.as_mut() {
            Some(g) => g.feed(chunk, on_chunk),
            None => on_chunk(chunk),
        }
    };

    // 2xx / other: stream the body. The headers already proved the socket
    // live, so a failure HERE is a genuine mid-body drop (partial bytes are
    // already in the sink) → Fatal, never a retry.
    let fully_drained;
    if let Some(cl) = content_length {
        let cap = core::cmp::min(cl, max_size);
        let n_leading = core::cmp::min(leading.len(), cap);
        let mut delivered = 0usize;
        if n_leading > 0 {
            if sink(&leading[..n_leading]).is_err() {
                finish_conn(host, tls, false);
                return Err(ExchangeErr::Fatal("sink write failed"));
            }
            delivered += n_leading;
        }
        while delivered < cap {
            match tls_recv_poll(&mut tls, &mut buf) {
                Ok(0) => continue,
                Ok(n) => {
                    let take = core::cmp::min(n, cap - delivered);
                    if sink(&buf[..take]).is_err() {
                        finish_conn(host, tls, false);
                        return Err(ExchangeErr::Fatal("sink write failed"));
                    }
                    delivered += take;
                }
                Err(_) => break,
            }
        }
        // Reusable only if we consumed the ENTIRE body — a max_size-clipped
        // (truncated) read leaves unread bytes on the wire.
        fully_drained = delivered == cl;
    } else if chunked {
        // True streaming chunked decoder (RFC 7230 §4.1) — GitHub codeload
        // serves dynamically-generated tarballs chunked + binary, so the
        // body can't be buffer-then-scanned.
        match stream_chunked_body(leading, &mut tls, &mut buf, max_size, &mut sink) {
            Ok(_) => fully_drained = true,
            Err(e) => {
                finish_conn(host, tls, false);
                return Err(ExchangeErr::Fatal(e));
            }
        }
    } else {
        // Neither Content-Length nor chunked → close-delimited body: read
        // until the peer closes. Never reusable (no boundary to stop at).
        let mut delivered = 0usize;
        if !leading.is_empty() {
            let take = core::cmp::min(leading.len(), max_size);
            if sink(&leading[..take]).is_err() {
                finish_conn(host, tls, false);
                return Err(ExchangeErr::Fatal("sink write failed"));
            }
            delivered += take;
        }
        while delivered < max_size {
            match tls_recv_poll(&mut tls, &mut buf) {
                Ok(0) => continue,
                Ok(n) => {
                    let take = core::cmp::min(n, max_size - delivered);
                    if sink(&buf[..take]).is_err() {
                        finish_conn(host, tls, false);
                        return Err(ExchangeErr::Fatal("sink write failed"));
                    }
                    delivered += take;
                }
                Err(_) => break,
            }
        }
        fully_drained = false;
    }

    if chatty() {
        kprintln!("[npk]   HTTP body {} ms", crate::interrupts::ticks().wrapping_sub(t_body0) * 10);
        // Say that it ran. A gzip that quietly did not happen looks exactly
        // like one that did, and the whole point of this path is a number.
        // Silence used to be ambiguous three ways — old kernel, we never
        // asked, or the server answered identity. Each now says which.
        match (gunzip.as_ref(), req.accept_gzip) {
            (Some(g), _) => {
                let (raw, out) = g.ratio();
                let r10 = out * 10 / core::cmp::max(raw, 1);
                kprintln!("[npk]   gzip {} -> {} B ({}.{}x)", raw, out, r10 / 10, r10 % 10);
            }
            (None, true) => kprintln!("[npk]   gzip asked, server answered identity"),
            (None, false) => {}
        }
    }
    finish_conn(host, tls, persistent && fully_drained);
    Ok(HttpResponse { status, location, content_type, headers: reply_headers })
}

/// One HTTPS round-trip — no redirect following. Tries HTTP/2 when the
/// caller asked for it, then a pooled HTTP/1.1 keep-alive session for `host`,
/// then a fresh one. Body bytes are pushed through `on_chunk` as they arrive.
///
/// This is the layer h2 belongs in, and the reason is the redirect: a
/// redirect may change the host, and everything that follows from that —
/// the method switch, dropping the caller's headers at an origin boundary,
/// keeping each hop's `Set-Cookie` — lives ABOVE here, in
/// `https_request_streaming`. Lifting the document onto h2 anywhere higher
/// would have traded a throttling problem for a redirect problem, and every
/// login is a redirect chain.
fn https_get_once(
    host: &str,
    path: &str,
    req: &HttpRequest,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<HttpResponse, &'static str> {
    // Timer-NAPI, for the handshake AND the body. `http_get_once` has done this
    // since it was written; the TLS path never did — so every OTA download
    // polled its socket at 100 Hz and slept up to 10 ms between looks, while
    // `netbench` over plain HTTP got 10 kHz and a 100 µs floor. A factor of a
    // hundred on the receive path, and exactly the asymmetry we kept blaming on
    // TLS itself: "update crawls, netbench flies". The guard restores 100 Hz on
    // every exit path, including the `?` returns below.
    crate::interrupts::set_worker_poll_hz(10_000);
    struct PollHzGuard;
    impl Drop for PollHzGuard {
        fn drop(&mut self) { crate::interrupts::set_worker_poll_hz(100); }
    }
    let _hz = PollHzGuard;

    // Klartext geht seinen eigenen Weg: kein h2 (h2c sprechen wir nicht),
    // kein Sitzungsspeicher, kein TLS. `http_get_once` gibt es seit langem,
    // es fehlte nur der Weg dorthin.
    //
    // Nur GET. Ein POST oder eigene Koepfe muessten durch `https_exchange`,
    // und das setzt eine TLS-Sitzung voraus — den zweiten Rumpf dafuer zu
    // bauen, bevor ihn jemand braucht, waere Arbeit auf Verdacht. Wer es
    // versucht, bekommt eine Absage und keinen stillen Fehlschlag.
    if req.plain {
        if req.method != "GET" && !req.method.is_empty() {
            return Err("plain http: only GET");
        }
        return http_get_once(host, path, max_size, on_chunk);
    }

    // Attempt 0: HTTP/2. Wikimedia throttles HTTP/1.1 to ~0.5 requests/s and
    // exempts h2 (§8.1, measured) — and one page load is FOUR document
    // requests inside two seconds, so this path was the only one still
    // paying. `Retry` means nothing was delivered; HTTP/1.1 runs below.
    if req.try_h2 {
        match h2_once(host, path, req, max_size, on_chunk) {
            Ok(r) => return Ok(r),
            Err(ExchangeErr::Fatal(e)) => return Err(e),
            Err(ExchangeErr::Retry) => {}
        }
    }

    // Attempt 1: reuse a pooled session (no DNS/TCP/TLS handshake).
    if let Some(tls) = pool_take(host) {
        match https_exchange(host, path, req, max_size, tls, on_chunk) {
            Ok(r) => return Ok(r),
            Err(ExchangeErr::Retry) => {} // stale — reconnect below
            Err(ExchangeErr::Fatal(e)) => return Err(e),
        }
    }
    // Attempt 2: fresh connection.
    let tls = open_tls(host)?;
    match https_exchange(host, path, req, max_size, tls, on_chunk) {
        Ok(r) => Ok(r),
        Err(ExchangeErr::Retry) => Err("connection reset after handshake"),
        Err(ExchangeErr::Fatal(e)) => Err(e),
    }
}

/// Parse a Location header value into (host, path-with-query).
///
/// Accepts:
///   * absolute `https://host/path?query` → (host, "/path?query")
///   * absolute `https://host` → (host, "/")
///   * absolute-path `/path?query` → (current_host, "/path?query")
///
/// Rejects `http://...` (we never downgrade) and any other scheme.
fn parse_https_url(loc: &str, current_host: &str) -> Result<(String, String), &'static str> {
    let loc = loc.trim();
    if let Some(rest) = loc.strip_prefix("https://") {
        let (h, p) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if h.is_empty() { return Err("redirect: empty host"); }
        Ok((String::from(h), String::from(p)))
    } else if loc.starts_with('/') {
        Ok((String::from(current_host), String::from(loc)))
    } else if loc.starts_with("http://") {
        Err("redirect: refusing http downgrade")
    } else {
        Err("redirect: unsupported Location")
    }
}

/// Parse a user-facing URL (from a WASM app, e.g. beak) into (host, path).
/// Accepts `https://host/path`, `host/path`, or a bare `host` (path
/// defaults to `/`). Scheme-less input is treated as https; plain `http://`
/// is refused (no downgrade). Reuses `parse_https_url`'s rules.
pub(crate) fn parse_url(url: &str) -> Result<(String, String, bool), &'static str> {
    let url = url.trim();
    if url.is_empty() {
        return Err("empty url");
    }
    if url.starts_with("https://") {
        parse_https_url(url, "").map(|(h, p)| (h, p, true))
    } else if let Some(rest) = url.strip_prefix("http://") {
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if host.is_empty() {
            return Err("empty host");
        }
        if !plain_http_allowed(host) {
            return Err("refusing http downgrade");
        }
        // JEDER Klartext-Abruf sagt es. Ein stiller Downgrade ist genau das,
        // was die Regel verhindern soll — und wer den Schalter vergessen hat
        // umzulegen, sieht es hier statt in einem Paketmitschnitt.
        kprintln!("[npk]   http (KLARTEXT, kein TLS) -> {}", host);
        Ok((String::from(host), String::from(path), false))
    } else {
        let mut full = String::from("https://");
        full.push_str(url);
        parse_https_url(&full, "").map(|(h, p)| (h, p, true))
    }
}

/// Darf `http://<host>` ohne TLS geholt werden?
///
/// **Zwei Bedingungen, und beide muessen halten.**
///
/// 1. `net.allow_plain_http` steht auf `1`. Vorgabe ist AUS: ein Geraet, das
///    den Schluessel nie setzt, verhaelt sich wie vorher, und die Angriffs-
///    flaeche entsteht erst, wenn jemand sie einschaltet.
/// 2. Der Host ist eine LITERALE private Adresse — 10/8, 172.16/12,
///    192.168/16, 127/8, 169.254/16.
///
/// Der zweite Punkt ist nicht Bequemlichkeit, sondern der Kern: ein NAME
/// waere hier eine Luecke, weil sein DNS-Eintrag jederzeit auf eine oeffent-
/// liche Adresse zeigen kann (und beim zweiten Auflosen auf eine andere als
/// beim ersten). Eine Adresse, die im URL selbst steht, kann sich nicht
/// verwandeln.
///
/// ⚠ Was auch mit dem Schalter AN bestehen bleibt: eine fremde Seite kann
/// beak dazu bringen, Unterressourcen von `http://192.168.x.y` zu holen und
/// so das eigene Netz abzuklopfen. Deshalb ist der Schalter fuer die Dauer
/// einer Messung gedacht und nicht fuer den Dauerbetrieb.
fn plain_http_allowed(host: &str) -> bool {
    if crate::config::get("net.allow_plain_http").as_deref() != Some("1") {
        return false;
    }
    let bare = split_host_port(host).0;
    match parse_ip(bare) {
        Some([10, ..]) => true,
        Some([172, b, ..]) if (16..=31).contains(&b) => true,
        Some([192, 168, ..]) => true,
        Some([127, ..]) => true,
        Some([169, 254, ..]) => true,
        _ => false,
    }
}

/// `host` oder `host:port` in beides zerlegen. Der Port ist `None`, wenn
/// keiner dasteht — dann entscheidet das Schema.
///
/// Bis hierher hat NICHTS im Kernel einen Port aus einer Adresse gelesen:
/// `connect(ip, 443)` stand fest, und `http://10.0.2.2:8080/x` waere auf :443
/// gelandet. Der Aufruf mit dem vollen `host:port` bleibt fuer den
/// `Host:`-Kopf richtig (RFC 9110 nennt den Port, wenn er nicht der
/// Vorgabeport ist); DNS, `parse_ip` und die TLS-Kennung brauchen den nackten
/// Namen.
pub(crate) fn split_host_port(host: &str) -> (&str, Option<u16>) {
    match host.rsplit_once(':') {
        // Ein Doppelpunkt im Namen ist auch eine IPv6-Adresse — die kann
        // dieser Stapel nicht, aber sie darf hier nicht als Port gelesen
        // werden.
        Some((h, p)) if !h.contains(':') => match p.parse::<u16>() {
            Ok(n) => (h, Some(n)),
            Err(_) => (host, None),
        },
        _ => (host, None),
    }
}

/// TLS recv with network polling. Retries on Ok(0) up to a hard timeout.
fn tls_recv_poll(tls: &mut crate::tls::TlsSession, buf: &mut [u8]) -> Result<usize, &'static str> {
    let start = crate::interrupts::ticks();
    loop {
        if super::cancel_requested() { return Err("cancelled"); }
        // ONE poll per attempt. `net::poll()` already drains the
        // entire NIC ring (`while let Some = netdev::recv`), ticks
        // TCP, and runs a shade render pass — so a single call
        // pulls everything currently available. The old code did
        // this 2000× before every `tls_recv`, which on emulated
        // NICs (each MMIO read traps to the hypervisor) cost ~0.5 s
        // of pure overhead per 16 KiB TLS record → ~31 KiB/s
        // ceiling regardless of link speed. Polling once and
        // returning the instant a record is ready makes throughput
        // bound by the actual network, not a fixed busy-wait tax.
        crate::net::poll_rx_only();
        match crate::tls::tls_recv(tls, buf) {
            Ok(0) => {
                // No app data yet (partial record, or a control
                // message like NewSessionTicket/CCS). Keep polling
                // until the hard timeout.
                if crate::interrupts::ticks().wrapping_sub(start) > 1500 {
                    return Err("recv timeout"); // 15 seconds hard timeout
                }
                core::hint::spin_loop();
            }
            Ok(n) => return Ok(n),
            Err(_) => return Err("recv error"),
        }
    }
}

/// Plain-TCP recv with polling (no TLS). Analog of `tls_recv_poll`.
// TEMP profiler: where does the recv loop's time go? TSC cycles + iteration
// count, read+reset in the http heartbeat. Pinpoints the ~13 µs/packet.
static PROF_POLL_CYC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static PROF_RECV_CYC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static PROF_ITERS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn tcp_recv_poll(handle: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    use core::sync::atomic::Ordering::Relaxed;
    let start = crate::interrupts::ticks();
    loop {
        if super::cancel_requested() { return Err("cancelled"); }
        let t0 = crate::interrupts::rdtsc();
        crate::net::poll_rx_only();
        let t1 = crate::interrupts::rdtsc();
        let r = crate::net::tcp::recv(handle, buf);
        let t2 = crate::interrupts::rdtsc();
        PROF_POLL_CYC.fetch_add(t1.wrapping_sub(t0), Relaxed);
        PROF_RECV_CYC.fetch_add(t2.wrapping_sub(t1), Relaxed);
        PROF_ITERS.fetch_add(1, Relaxed);
        match r {
            Ok(0) => {
                if crate::interrupts::ticks().wrapping_sub(start) > 1500 {
                    return Err("recv timeout");
                }
                // BUSY-SPIN, do NOT HLT. The USB NIC has no IRQ — its RX ring is
                // re-armed ONLY by poll_rx_only() above. worker_idle_hlt() parks
                // this core until the next 100 Hz worker tick (up to 10 ms);
                // nothing re-arms the ring in that gap, so the chip exhausts all
                // buffers in a few ms and then drops every frame → the ~24 Mbit
                // cap + massive TCP reorder on rtl8153. (virtio/QEMU is immune:
                // it delivers RX from a fiber, not this polled loop.) tcp_recv_poll
                // only runs during an active download, so spinning is correct —
                // and matches tls_recv_poll, which never had the HLT.
                core::hint::spin_loop();
            }
            Ok(n) => return Ok(n),
            Err(_) => return Err("recv error"),
        }
    }
}

/// Parse a Location into (host, path) for the PLAIN-HTTP path: accepts
/// `http://…` and absolute-path; rejects https upgrade (our TLS is minimal).
fn parse_http_url(loc: &str, current_host: &str) -> Result<(String, String), &'static str> {
    let loc = loc.trim();
    if let Some(rest) = loc.strip_prefix("http://") {
        let (h, p) = match rest.find('/') { Some(i) => (&rest[..i], &rest[i..]), None => (rest, "/") };
        if h.is_empty() { return Err("redirect: empty host"); }
        Ok((String::from(h), String::from(p)))
    } else if loc.starts_with('/') {
        Ok((String::from(current_host), String::from(loc)))
    } else if loc.starts_with("https://") {
        Err("redirect to https — our TLS is minimal; use a direct-http mirror")
    } else {
        Err("redirect: unsupported Location")
    }
}

/// Plain-HTTP streaming GET (no TLS) — for throughput tests + plain-http
/// mirrors (our TLS only handshakes with a couple of CAs, so arbitrary HTTPS
/// fails; plain HTTP sidesteps it AND is a cleaner pure-net speed test).
/// Follows up to 4 http→http (or absolute-path) redirects. Requires
/// Content-Length (true for static file mirrors); rejects chunked.
pub fn http_get_streaming(
    host: &str,
    path: &str,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<usize, &'static str> {
    let mut cur_host = String::from(host);
    let mut cur_path = String::from(path);
    for _ in 0..5 {
        let mut total: usize = 0;
        let resp = http_get_once(&cur_host, &cur_path, max_size,
            &mut |chunk: &[u8]| -> Result<(), &'static str> {
                on_chunk(chunk)?;
                total = total.saturating_add(chunk.len());
                Ok(())
            })?;
        match resp.status {
            200..=299 => {
                if total == 0 { return Err("empty body"); }
                return Ok(total);
            }
            301 | 302 | 303 | 307 | 308 => {
                let loc = resp.location.ok_or("redirect without Location")?;
                let (h, p) = parse_http_url(&loc, &cur_host)?;
                cur_host = h;
                cur_path = p;
            }
            _ => return Err("HTTP non-2xx response"),
        }
    }
    Err("too many redirects")
}

/// Network throughput benchmark against a plain-HTTP server — isolates our net
/// stack from the WAN so we can see where the real bottleneck is. Reaches a
/// local server (e.g. `10.0.2.2:80` = QEMU slirp host alias).
///   netbench get <host> <path>        download into a counting sink (we time)
///   netbench put <host> <path> [MB]    upload a RAM buffer (the SERVER times it
///                                      and returns the rate — our send side has
///                                      no congestion control, so it can't)
pub fn intent_netbench(args: &str) {
    let mut it = args.split_whitespace();
    match it.next().unwrap_or("") {
        "get" => {
            let host = match it.next() { Some(h) => h, None => { kprintln!("usage: netbench get <host> <path>"); return; } };
            let path = it.next().unwrap_or("/");
            bench_get(host, path);
        }
        "put" => {
            let host = match it.next() { Some(h) => h, None => { kprintln!("usage: netbench put <host> <path> [MB]"); return; } };
            let path = it.next().unwrap_or("/upload");
            let mb: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(50);
            bench_put(host, path, mb);
        }
        _ => kprintln!("usage: netbench get <host> <path> | netbench put <host> <path> [MB]"),
    }
}

/// Print bytes/elapsed as MB, ms, Mbit/s, MB/s using integer math (no float).
fn bench_report(label: &str, bytes: usize, cyc: u64) {
    let freq = crate::interrupts::tsc_freq().max(1) as u128;
    let us = (cyc as u128 * 1_000_000 / freq).max(1);
    let ms = (us / 1000) as u64;
    let mbit_s = (bytes as u128 * 8 / us) as u64; // bits/us = Mbit/s
    let mbyte_s = (bytes as u128 / us) as u64;    // bytes/us = MB/s
    let mb = bytes / (1024 * 1024);
    kprintln!("[netbench] {}: {} MB in {} ms = {} Mbit/s ({} MB/s)", label, mb, ms, mbit_s, mbyte_s);
}

fn bench_get(host: &str, path: &str) {
    kprintln!("[netbench] GET http://{}{}", host, path);
    let mut bytes: usize = 0;
    let mut sink = |c: &[u8]| -> Result<(), &'static str> { bytes += c.len(); Ok(()) };
    let t0 = crate::interrupts::rdtsc();
    let res = http_get_streaming(host, path, usize::MAX, &mut sink);
    let t1 = crate::interrupts::rdtsc();
    match res {
        Ok(_) => bench_report("GET", bytes, t1.wrapping_sub(t0)),
        Err(e) => kprintln!("[netbench] GET failed after {} bytes: {}", bytes, e),
    }
}

fn bench_put(host: &str, path: &str, mb: usize) {
    let total = mb * 1024 * 1024;
    kprintln!("[netbench] PUT http://{}{} ({} MB)", host, path, mb);
    let t0 = crate::interrupts::rdtsc();
    let res = http_post_zeros(host, path, total);
    let t1 = crate::interrupts::rdtsc();
    match res {
        Ok(reply) => {
            bench_report("PUT(local)", total, t1.wrapping_sub(t0));
            // The server measures the true received rate (our send side has no
            // congestion control); echo whatever it reported.
            let r = reply.trim();
            if !r.is_empty() { kprintln!("[netbench] PUT(server): {}", r); }
        }
        Err(e) => kprintln!("[netbench] PUT failed: {}", e),
    }
}

/// POST `total` zero-bytes to <host><path>; returns the server's response body
/// (which is expected to report the server-measured throughput).
fn http_post_zeros(host: &str, path: &str, total: usize) -> Result<String, &'static str> {
    let ip = parse_ip(host).or_else(|| crate::net::dns::resolve(host)).ok_or("DNS/IP failed")?;
    let gw = crate::net::ipv4::gateway();
    let _ = crate::net::arp::resolve(gw, 100); // see open_tls: not a blind spin
    // Name the failure. ConnectionRefused means the peer answered with a RST —
    // nothing is listening there, go look at the server. Timeout means nobody
    // answered at all — go look at ARP, routing, the air. Collapsing both into
    // "TCP connect failed" sent us hunting the radio while a Python process on
    // the other end had simply exited.
    let handle = crate::net::tcp::connect(ip, 80).map_err(|e| {
        kprintln!("[netbench] connect to {}.{}.{}.{}:80 failed: {}",
                  ip[0], ip[1], ip[2], ip[3], e);
        "TCP connect failed"
    })?;
    crate::interrupts::set_worker_poll_hz(10_000);
    struct PollHzGuard;
    impl Drop for PollHzGuard {
        fn drop(&mut self) { crate::interrupts::set_worker_poll_hz(100); }
    }
    let _poll_hz_guard = PollHzGuard;

    let req = alloc::format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        path, host, total
    );
    if crate::net::tcp::send_blocking(handle, req.as_bytes(), 1000).is_err() {
        let _ = crate::net::tcp::close(handle);
        return Err("send header failed");
    }

    // 64 KiB, not 256. `tcp::send` refuses when `send_buf.len() + data.len()`
    // exceeds MAX_UNACKED, which is itself 256 KiB — so a 256 KiB chunk could
    // only ever be accepted with the send buffer at EXACTLY zero. That is
    // stop-and-wait dressed up as a stream: every chunk had to be fully
    // acknowledged before the next one could be queued, and one delayed ACK
    // inside the 10 s window failed the whole transfer. A quarter of the cap
    // leaves three chunks in flight.
    let chunk = alloc::vec![0u8; 64 * 1024];
    let mut sent = 0;
    while sent < total {
        let n = core::cmp::min(chunk.len(), total - sent);
        // Say WHICH failure it was. "send body failed" covers a peer that
        // closed the connection and a send window that never opened, and those
        // want opposite investigations.
        if let Err(e) = crate::net::tcp::send_blocking(handle, &chunk[..n], 1000) {
            kprintln!("[netbench] PUT stalled after {} of {} bytes: {}", sent, total, e);
            let _ = crate::net::tcp::close(handle);
            return Err("send body failed");
        }
        sent += n;
        // Drive the stack so ACKs come in and the retransmit buffer is trimmed
        // (send() has no flow control, so without this the send_buf grows).
        crate::net::poll();
    }

    // Read the server's response (it measured the receive rate).
    let mut raw = alloc::vec::Vec::new();
    let mut buf = alloc::vec![0u8; 4096];
    for _ in 0..200_000 {
        match tcp_recv_poll(handle, &mut buf) {
            Ok(0) => { core::hint::spin_loop(); }
            Ok(n) => { raw.extend_from_slice(&buf[..n]); if raw.len() > 8192 { break; } }
            Err(_) => break,
        }
        if raw.windows(4).any(|w| w == b"\r\n\r\n") && raw.len() > 16 { break; }
    }
    let _ = crate::net::tcp::close(handle);
    // Return the body after the header (best-effort).
    let body = match raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(p) => String::from_utf8_lossy(&raw[p + 4..]).into_owned(),
        None => String::new(),
    };
    Ok(body)
}

/// Streaming download for the USER `http`/`https` intents — follows redirects
/// across BOTH schemes, including https→http downgrade (which OTA's strict
/// `https_get` refuses on purpose). This lets `https cdimage.debian.org/…iso`
/// chase its 302 to a fast plain-http mirror — the user's confirmed gigabit
/// source — instead of dead-ending. `start_tls` = first hop's scheme.
/// NOT for OTA (that path stays strict, signatures aside).
fn user_download_streaming(
    host: &str,
    path: &str,
    start_tls: bool,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<usize, &'static str> {
    let mut cur_host = String::from(host);
    let mut cur_path = String::from(path);
    let mut use_tls = start_tls;
    for _ in 0..6 {
        let resp = if use_tls {
            https_get_once(&cur_host, &cur_path, &HttpRequest::default(), max_size, on_chunk)?
        } else {
            http_get_once(&cur_host, &cur_path, max_size, on_chunk)?
        };
        match resp.status {
            200..=299 => return Ok(0), // bytes already counted by the caller's sink
            301 | 302 | 303 | 307 | 308 => {
                let loc = resp.location.ok_or("redirect without Location")?;
                let (h, p, tls) = parse_any_url(&loc, &cur_host, use_tls)?;
                if !use_tls && tls {
                    return Err("redirect http→https — our TLS is minimal; use a direct mirror");
                }
                cur_host = h;
                cur_path = p;
                use_tls = tls;
            }
            _ => return Err("HTTP non-2xx response"),
        }
    }
    Err("too many redirects")
}

/// Permissive redirect-URL parser: accepts http://, https://, and absolute
/// paths. Returns (host, path, is_tls).
fn parse_any_url(loc: &str, current_host: &str, current_tls: bool) -> Result<(String, String, bool), &'static str> {
    let loc = loc.trim();
    let split = |rest: &str| -> (String, String) {
        match rest.find('/') {
            Some(i) => (String::from(&rest[..i]), String::from(&rest[i..])),
            None => (String::from(rest), String::from("/")),
        }
    };
    if let Some(rest) = loc.strip_prefix("https://") {
        let (h, p) = split(rest);
        if h.is_empty() { return Err("redirect: empty host"); }
        Ok((h, p, true))
    } else if let Some(rest) = loc.strip_prefix("http://") {
        let (h, p) = split(rest);
        if h.is_empty() { return Err("redirect: empty host"); }
        Ok((h, p, false))
    } else if loc.starts_with('/') {
        Ok((String::from(current_host), String::from(loc), current_tls))
    } else {
        Err("redirect: unsupported Location")
    }
}

/// One plain-HTTP round-trip (no TLS, no redirect follow). Mirrors
/// `https_get_once` but over raw TCP. Content-Length bodies only.
fn http_get_once(
    host: &str,
    path: &str,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<HttpResponse, &'static str> {
    let (bare, port) = split_host_port(host);
    let port = port.unwrap_or(80);
    let ip = match parse_ip(bare) {
        Some(ip) => ip,
        None => crate::net::dns::resolve(bare).ok_or("DNS resolution failed")?,
    };
    if chatty() { kprintln!("[npk]   {} -> {}.{}.{}.{}", bare, ip[0], ip[1], ip[2], ip[3]); }
    let gw = crate::net::ipv4::gateway();
    let _ = crate::net::arp::resolve(gw, 100); // see open_tls: not a blind spin

    if chatty() { kprintln!("[npk]   TCP connect {}:{} ...", bare, port); }
    let handle = crate::net::tcp::connect(ip, port).map_err(|_| "TCP connect failed")?;

    // Timer-NAPI: speed this worker core's idle timer to ~10 kHz for the whole
    // transfer so the recv loop's HLT wakes every ~100 µs (vs 10 ms at 100 Hz) —
    // low-latency polling without burning the core. The guard restores 100 Hz on
    // every exit path (success, error, redirect).
    crate::interrupts::set_worker_poll_hz(10_000);
    struct PollHzGuard;
    impl Drop for PollHzGuard {
        fn drop(&mut self) { crate::interrupts::set_worker_poll_hz(100); }
    }
    let _poll_hz_guard = PollHzGuard;

    let request = alloc::format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path, host, USER_AGENT
    );
    if crate::net::tcp::send_blocking(handle, request.as_bytes(), 1000).is_err() {
        let _ = crate::net::tcp::close(handle);
        return Err("HTTP send failed");
    }

    let mut raw = alloc::vec::Vec::new();
    // Large HEAP read buffer (a 512 KiB stack array would overflow the kernel
    // stack). recv() returns at most buf.len() per call; the old 17 KB cap made
    // the consumer drain far slower than poll_rx_only bulk-fills recv_buf (it
    // empties the whole NIC ring per call) → recv_buf climbed to the 8 MiB
    // window cap → window 0 → the sender stalled (measured rxbuf_max≈8191 KiB).
    // A big drain per call keeps recv_buf near-empty → window stays open.
    let mut buf = alloc::vec![0u8; 512 * 1024];
    let mut header_end = None;
    loop {
        match tcp_recv_poll(handle, &mut buf) {
            Ok(0) => continue,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    header_end = Some(pos);
                    break;
                }
            }
            Err(_) => break,
        }
        if raw.len() > 32_768 {
            let _ = crate::net::tcp::close(handle);
            return Err("headers too large");
        }
    }
    let hdr_end = match header_end {
        Some(p) => p,
        None => { let _ = crate::net::tcp::close(handle); return Err("no HTTP headers received"); }
    };
    let body_start = hdr_end + 4;
    let hdr_str = core::str::from_utf8(&raw[..hdr_end]).map_err(|_| "invalid header encoding")?;
    let status = parse_status_code(hdr_str).unwrap_or(0);
    // Everything after the status line, capped. `Set-Cookie` repeats, so
    // handing back parsed single values could never carry it.
    let reply_headers = capture_headers(hdr_str);
    let location = parse_header_value(hdr_str, "location").map(String::from);
    let content_type = parse_header_value(hdr_str, "content-type").map(String::from);

    if (300..400).contains(&status) {
        match &location {
            Some(l) if chatty() => kprintln!("[npk]   HTTP {} → {}", status, l),
            None if chatty() => kprintln!("[npk]   HTTP {} (redirect, no Location)", status),
            _ => {}
        }
        let _ = crate::net::tcp::close(handle);
        return Ok(HttpResponse { status, location, content_type, headers: reply_headers });
    }
    if chatty() { kprintln!("[npk]   HTTP {} — receiving body", status); }

    let cl = match parse_header_value(hdr_str, "content-length").and_then(|v| v.trim().parse::<usize>().ok()) {
        Some(c) => c,
        None => { let _ = crate::net::tcp::close(handle); return Err("plain http needs Content-Length (chunked unsupported)"); }
    };
    let cap = core::cmp::min(cl, max_size);
    let leading = &raw[body_start..];
    let mut delivered: usize = 0;
    let n_leading = core::cmp::min(leading.len(), cap);
    if n_leading > 0 {
        on_chunk(&leading[..n_leading])?;
        delivered += n_leading;
    }
    while delivered < cap {
        match tcp_recv_poll(handle, &mut buf) {
            Ok(0) => continue,
            Ok(n) => {
                let take = core::cmp::min(n, cap - delivered);
                on_chunk(&buf[..take])?;
                delivered += take;
            }
            Err(_) => break,
        }
    }
    let _ = crate::net::tcp::close(handle);
    Ok(HttpResponse { status, location, content_type, headers: reply_headers })
}

/// Parse HTTP status code from first header line.
fn parse_status_code(headers: &str) -> Option<u16> {
    // "HTTP/1.1 200 OK" → 200
    let first_line = headers.lines().next()?;
    let mut parts = first_line.split_whitespace();
    parts.next()?; // "HTTP/1.1"
    parts.next()?.parse().ok()
}

/// Find a header value by name (case-insensitive).
fn parse_header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    for line in headers.lines() {
        if let Some((key, val)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case(name) {
                return Some(val.trim());
            }
        }
    }
    None
}

/// Chunked-decoder state, carried across TLS-record boundaries.
enum ChunkSt {
    /// Accumulating the `<hex>[;ext]\r\n` size line.
    Size,
    /// Inside a chunk's payload; `usize` bytes still to copy.
    Data(usize),
    /// Expecting the `\r` of the CRLF that follows chunk data.
    AfterCr,
    /// Expecting the `\n` of that CRLF.
    AfterLf,
    /// Saw the 0-size chunk — body complete.
    Done,
}

/// Consume one input slice (header `leading` bytes, then each TLS
/// record), advancing the decoder and pushing payload slices to
/// `on_chunk`. Zero-copy: payload is handed out as sub-slices of
/// `input` — no intermediate buffer, no `Vec::drain`. Returns
/// `Ok(true)` once the terminal 0-chunk is seen.
fn chunked_feed(
    input: &[u8],
    state: &mut ChunkSt,
    size_line: &mut alloc::vec::Vec<u8>,
    delivered: &mut usize,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<bool, &'static str> {
    let mut i = 0;
    while i < input.len() {
        match *state {
            ChunkSt::Size => {
                // Accumulate up to (not including) the next '\n'.
                let mut j = i;
                while j < input.len() && input[j] != b'\n' {
                    j += 1;
                }
                if size_line.len() + (j - i) > 64 {
                    return Err("chunk: size line too long");
                }
                size_line.extend_from_slice(&input[i..j]);
                if j < input.len() {
                    // input[j] == '\n' — the size line is complete.
                    i = j + 1;
                    if size_line.last() == Some(&b'\r') {
                        size_line.pop();
                    }
                    // Chunk extensions (";name=val") are ignored.
                    let hex_end = size_line
                        .iter()
                        .position(|&b| b == b';')
                        .unwrap_or(size_line.len());
                    let hex = core::str::from_utf8(&size_line[..hex_end])
                        .map_err(|_| "chunk: bad size line")?
                        .trim();
                    let sz = usize::from_str_radix(hex, 16)
                        .map_err(|_| "chunk: bad size hex")?;
                    size_line.clear();
                    if sz == 0 {
                        *state = ChunkSt::Done;
                        return Ok(true);
                    }
                    *state = ChunkSt::Data(sz);
                } else {
                    // Ran out of input mid-line; resume next record.
                    i = j;
                }
            }
            ChunkSt::Data(remaining) => {
                let avail = input.len() - i;
                let take = remaining.min(avail);
                on_chunk(&input[i..i + take])?;
                *delivered = delivered.saturating_add(take);
                if *delivered > max_size {
                    return Err("chunk: body exceeds max_size");
                }
                i += take;
                let left = remaining - take;
                *state = if left == 0 {
                    ChunkSt::AfterCr
                } else {
                    ChunkSt::Data(left)
                };
            }
            ChunkSt::AfterCr => {
                // The byte should be '\r'; consume it if so. Either
                // way move on — a non-CR here on valid chunked never
                // happens, and tolerating it can't desync because
                // the size-line parser re-validates.
                if input[i] == b'\r' {
                    i += 1;
                }
                *state = ChunkSt::AfterLf;
            }
            ChunkSt::AfterLf => {
                if input[i] == b'\n' {
                    i += 1;
                }
                *state = ChunkSt::Size;
            }
            ChunkSt::Done => return Ok(true),
        }
    }
    Ok(false)
}

/// True streaming chunked-transfer decoder (RFC 7230 §4.1).
///
/// Parses the chunk-size framing exactly and pushes only decoded
/// payload bytes through `on_chunk` as they arrive. Linear and
/// zero-copy: each TLS record is walked in place and payload is
/// handed out as sub-slices — there is NO growing carry buffer and
/// NO `Vec::drain`. (The previous implementation drained from the
/// front of a `Vec` that it also extended at the back, which is
/// O(n²) over a transfer: fine for a 13 KB repo, ~31 KiB/s by
/// 500 KB, effectively dead at 250 MB. The Content-Length path was
/// always ~100× faster purely because it never did this.)
///
/// The only state kept across records is the decoder enum plus a
/// small `size_line` accumulator (a chunk-size line that straddles
/// a record boundary — capped at 64 bytes).
///
/// Returns the number of payload bytes delivered.
fn stream_chunked_body(
    leading: &[u8],
    tls: &mut crate::tls::TlsSession,
    buf: &mut [u8],
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<usize, &'static str> {
    let mut state = ChunkSt::Size;
    let mut size_line: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(32);
    let mut delivered: usize = 0;

    if !leading.is_empty()
        && chunked_feed(leading, &mut state, &mut size_line, &mut delivered, max_size, on_chunk)?
    {
        return Ok(delivered);
    }

    loop {
        match tls_recv_poll(tls, buf) {
            Ok(0) => continue, // transient; tls_recv_poll caps the wait
            Ok(n) => {
                if chunked_feed(
                    &buf[..n],
                    &mut state,
                    &mut size_line,
                    &mut delivered,
                    max_size,
                    on_chunk,
                )? {
                    return Ok(delivered);
                }
            }
            Err(e) => return Err(e),
        }
    }
}
