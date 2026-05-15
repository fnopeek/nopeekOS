//! HTTP/HTTPS intents

use crate::{kprint, kprintln, capability};
use alloc::string::String;
use super::{parse_ip, resolve_path};

const HTTP_MAX_RESPONSE: usize = 128 * 1024; // 128 KB

/// Streaming-download progress heartbeat interval. A large download
/// on the synchronous path blocks the shell until it finishes; a
/// line every 8 MiB is the "still alive, not crashed" signal so the
/// freeze is legible rather than alarming.
const PROGRESS_STEP: usize = 8 * 1024 * 1024;

/// Flags parsed from HTTP/HTTPS arguments.
struct HttpFlags {
    headers_only: bool,  // -h: show only headers
    body_only: bool,     // -b: show only body
    silent: bool,        // -s: no status output
}

/// Parse flags from anywhere in the args, return flags + cleaned args.
fn parse_http_args(args: &str) -> (HttpFlags, String) {
    let mut flags = HttpFlags { headers_only: false, body_only: false, silent: false };
    let mut cleaned = String::new();

    for part in args.split_whitespace() {
        match part {
            "-h" => flags.headers_only = true,
            "-b" => flags.body_only = true,
            "-s" => flags.silent = true,
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
    let proto = if use_tls { "https" } else { "http" };
    let (flags, url) = parse_http_args(args);
    let url = url.as_str();

    if url.is_empty() {
        kprintln!("[npk] Usage: {} [-h|-b|-s] <host> [path] [> name]", proto);
        kprintln!("[npk]   -h  Headers only");
        kprintln!("[npk]   -b  Body only (no headers)");
        kprintln!("[npk]   -s  Silent (no status messages)");
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
    if use_tls {
        if let Some(name) = &store_as {
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
            let mut writer = match crate::npkfs::open_streaming_write(&store_path) {
                Ok(w) => w,
                Err(e) => { kprintln!("[npk] npkfs open failed: {:?}", e); return; }
            };
            let mut total: usize = 0;
            let mut first = true;
            // Time-based heartbeat (~2 s @ 100 Hz ticks). Distinguishes
            // "stuck" (byte count frozen) from "slow" (count creeps up)
            // regardless of throughput — a byte-threshold heartbeat
            // can't tell those apart on a slow link.
            let mut last_tick = crate::interrupts::ticks();
            // No max_size cap for user-initiated downloads — the
            // ceiling is whatever fits on disk. We still defend the
            // kernel via the per-chunk allocator (each 16 MiB chunk
            // is freed after flush) so a 1 TB download just uses
            // 16 MiB of heap throughout.
            let max_size = usize::MAX;
            let stream_result = https_get_streaming(
                host, path, max_size,
                &mut |chunk: &[u8]| -> Result<(), &'static str> {
                    if first {
                        kprintln!("[npk]   first body bytes ({} B)", chunk.len());
                        first = false;
                    }
                    if writer.write(chunk).is_err() {
                        return Err("npkfs write failed");
                    }
                    total = total.saturating_add(chunk.len());
                    let now = crate::interrupts::ticks();
                    if now.wrapping_sub(last_tick) >= 200 {
                        kprintln!("[npk]   rx {} KiB", total / 1024);
                        last_tick = now;
                    }
                    Ok(())
                },
            );
            match stream_result {
                Ok(_) => {}
                Err(e) => { kprintln!("[npk] download failed: {}", e); return; }
            }
            match writer.finish() {
                Ok(written) => {
                    kprintln!("[npk] Stored '{}' ({} bytes)", store_path, written);
                }
                Err(e) => kprintln!("[npk] publish failed: {:?}", e),
            }
            let _ = total;
            return;
        }
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
    crate::net::arp::request(gw);
    for _ in 0..50_000 { crate::net::poll(); core::hint::spin_loop(); }

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
        "GET {} HTTP/{}\r\nHost: {}\r\nUser-Agent: nopeekOS/0.1\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path, http_ver, host
    );

    let send_ok = if let Some(ref mut sess) = tls_session {
        crate::tls::tls_send(sess, request.as_bytes()).is_ok()
    } else {
        crate::net::tcp::send(handle, request.as_bytes()).is_ok()
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
        let store_path = match resolve_store_target(&name, path) {
            Some(p) => p,
            None => {
                kprintln!("[npk] '{}' is a directory and the URL has no filename — give an explicit name (> dir/name)", name);
                return;
            }
        };
        let body = &response[body_start..];
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
    let mut cur_host = String::from(host);
    let mut cur_path = String::from(path);
    for _ in 0..4 {
        // Vec-mode: accumulate into out, sink just extends it.
        let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        let resp = https_get_once(
            &cur_host,
            &cur_path,
            max_size,
            &mut |chunk: &[u8]| -> Result<(), &'static str> {
                if out.len().saturating_add(chunk.len()) > max_size {
                    out.extend_from_slice(&chunk[..max_size.saturating_sub(out.len())]);
                    Ok(())
                } else {
                    out.extend_from_slice(chunk);
                    Ok(())
                }
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
    let mut cur_host = String::from(host);
    let mut cur_path = String::from(path);
    for _ in 0..4 {
        let mut total: usize = 0;
        let resp = https_get_once(
            &cur_host,
            &cur_path,
            max_size,
            &mut |chunk: &[u8]| -> Result<(), &'static str> {
                on_chunk(chunk)?;
                total = total.saturating_add(chunk.len());
                Ok(())
            },
        )?;
        match resp.status {
            200..=299 => {
                if total == 0 {
                    return Err("empty body");
                }
                return Ok(total);
            }
            301 | 302 | 303 | 307 | 308 => {
                // On 3xx the inner once-fn returns early without
                // calling the sink, so the consumer never sees any
                // bytes from the redirect response. Safe to retry
                // against the Location target with a fresh TLS
                // session.
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

/// One HTTPS round-trip — no redirect following. Body bytes are
/// pushed through `on_chunk` as they arrive; the returned
/// `HttpResponse.body` is always empty (the sink owns the bytes).
fn https_get_once(
    host: &str,
    path: &str,
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<HttpResponse, &'static str> {
    // Resolve hostname
    kprintln!("[npk]   resolving {} ...", host);
    let ip = if let Some(ip) = parse_ip(host) {
        ip
    } else {
        crate::net::dns::resolve(host).ok_or("DNS resolution failed")?
    };
    kprintln!("[npk]   {} -> {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]);

    // ARP resolve gateway (use actual gateway from DHCP, not hardcoded)
    let gw = crate::net::ipv4::gateway();
    crate::net::arp::request(gw);
    for _ in 0..50_000 { crate::net::poll(); core::hint::spin_loop(); }

    kprintln!("[npk]   TCP connect {}:443 ...", host);
    let handle = crate::net::tcp::connect(ip, 443).map_err(|_| "TCP connect failed")?;

    kprintln!("[npk]   TLS handshake ...");
    let mut tls = match crate::tls::tls_connect(handle, host) {
        Ok(s) => s,
        Err(_) => {
            let _ = crate::net::tcp::close(handle);
            return Err("TLS handshake failed");
        }
    };
    kprintln!("[npk]   TLS up, GET {}", path);

    // Send HTTP/1.1 GET
    let request = alloc::format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: nopeekOS/0.1\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path, host
    );
    if crate::tls::tls_send(&mut tls, request.as_bytes()).is_err() {
        let _ = crate::tls::tls_close(&mut tls);
        return Err("HTTP send failed");
    }

    // ── Phase 1: Receive HTTP headers ──────────────────────────
    // Read TLS records until we have the full header block (\r\n\r\n).
    let mut raw = alloc::vec::Vec::new();
    let mut buf = [0u8; 17000]; // >= max TLS record (16KB)
    let mut header_end = None;

    loop {
        match tls_recv_poll(&mut tls, &mut buf) {
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
            let _ = crate::tls::tls_close(&mut tls);
            return Err("headers too large");
        }
    }

    let hdr_end = match header_end {
        Some(pos) => pos,
        None => {
            let _ = crate::tls::tls_close(&mut tls);
            return Err("no HTTP headers received");
        }
    };
    let body_start = hdr_end + 4;

    // ── Phase 2: Parse HTTP status + headers ───────────────────
    let hdr_str = core::str::from_utf8(&raw[..hdr_end]).map_err(|_| "invalid header encoding")?;

    let status = parse_status_code(hdr_str).unwrap_or(0);
    let location = parse_header_value(hdr_str, "location").map(String::from);

    // On redirect we still drain the body (some servers send a short HTML
    // courtesy page) but skip the work of streaming a multi-MB asset.
    if (300..400).contains(&status) {
        match &location {
            Some(l) => kprintln!("[npk]   HTTP {} → {}", status, l),
            None => kprintln!("[npk]   HTTP {} (redirect, no Location)", status),
        }
        let _ = crate::tls::tls_close(&mut tls);
        return Ok(HttpResponse { status, location });
    }
    kprintln!("[npk]   HTTP {} — receiving body", status);

    let content_length = parse_header_value(hdr_str, "content-length")
        .and_then(|v| v.trim().parse::<usize>().ok());
    let chunked = parse_header_value(hdr_str, "transfer-encoding")
        .map(|v| v.contains("chunked"))
        .unwrap_or(false);

    // ── Phase 3: Receive body, push through sink ───────────────
    // Bytes after the header terminator (`\r\n\r\n`) that arrived in
    // the same TLS record are the first body bytes.
    let leading = &raw[body_start..];
    let mut delivered: usize = 0;

    if let Some(cl) = content_length {
        // Content-Length path: deliver exactly `cl` bytes (clipped to
        // `max_size`), then close.
        let cap = core::cmp::min(cl, max_size);
        let n_leading = core::cmp::min(leading.len(), cap);
        if n_leading > 0 {
            on_chunk(&leading[..n_leading])?;
            delivered += n_leading;
        }
        while delivered < cap {
            match tls_recv_poll(&mut tls, &mut buf) {
                Ok(0) => continue,
                Ok(n) => {
                    let take = core::cmp::min(n, cap - delivered);
                    on_chunk(&buf[..take])?;
                    delivered += take;
                }
                Err(_) => break,
            }
        }
    } else if chunked {
        // Transfer-Encoding: chunked — true streaming decoder.
        // GitHub's codeload serves dynamically-generated tarballs
        // (git archive on the fly) chunked + binary, so the body
        // is both huge and impossible to buffer-then-scan: a gzip
        // stream contains the byte sequence "0\r\n" by chance
        // almost immediately, which would false-trip any
        // end-of-body heuristic. A proper chunk-size state machine
        // is the only correct option.
        match stream_chunked_body(leading, &mut tls, &mut buf, max_size, on_chunk) {
            Ok(n) => delivered += n,
            Err(e) => {
                // A chunked stream that ends before the 0-size chunk
                // is a genuine truncation (dropped connection, sink
                // write failure). Propagate so the caller does NOT
                // commit a partial file — for OTA the SHA-384 check
                // would catch it anyway, but `https <url> > path`
                // has no hash and would otherwise silently store
                // corrupt data.
                let _ = crate::tls::tls_close(&mut tls);
                return Err(e);
            }
        }
    } else {
        // Connection: close — push all bytes until peer closes.
        if !leading.is_empty() {
            let take = core::cmp::min(leading.len(), max_size);
            on_chunk(&leading[..take])?;
            delivered += take;
        }
        while delivered < max_size {
            match tls_recv_poll(&mut tls, &mut buf) {
                Ok(0) => continue,
                Ok(n) => {
                    let take = core::cmp::min(n, max_size - delivered);
                    on_chunk(&buf[..take])?;
                    delivered += take;
                }
                Err(_) => break,
            }
        }
    }

    let _ = crate::tls::tls_close(&mut tls);
    Ok(HttpResponse { status, location })
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

/// TLS recv with network polling. Retries on Ok(0) up to a hard timeout.
fn tls_recv_poll(tls: &mut crate::tls::TlsSession, buf: &mut [u8]) -> Result<usize, &'static str> {
    let start = crate::interrupts::ticks();
    loop {
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
        crate::net::poll();
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

/// True streaming chunked-transfer decoder (RFC 7230 §4.1).
///
/// Parses the chunk-size framing exactly and pushes only decoded
/// payload bytes through `on_chunk` as they arrive — never buffering
/// the whole body and never scanning for an end-of-body magic
/// sequence (which is unsound on binary payloads, where
/// `30 0D 0A` = "0\r\n" occurs by chance). A small `carry` buffer
/// holds bytes received but not yet consumed; it stays around one
/// TLS-record in size because payload is drained to the sink and
/// chunk-size lines are tiny.
///
/// Returns the number of payload bytes delivered.
fn stream_chunked_body(
    leading: &[u8],
    tls: &mut crate::tls::TlsSession,
    buf: &mut [u8],
    max_size: usize,
    on_chunk: &mut dyn FnMut(&[u8]) -> Result<(), &'static str>,
) -> Result<usize, &'static str> {
    // Decoder state: reading a chunk-size line, copying N data
    // bytes, swallowing the CRLF after a chunk's data, or done.
    enum St { Size, Data(usize), AfterData, Done }

    let mut carry: alloc::vec::Vec<u8> = leading.to_vec();
    let mut state = St::Size;
    let mut delivered: usize = 0;
    // One-shot diagnostics: did the decoder ever parse a chunk-size
    // line, and did the wire ever yield body bytes?
    let mut logged_first_size = false;
    let mut logged_first_recv = false;
    crate::kprintln!("[npk]   chunked decoder: leading {} B", carry.len());

    loop {
        // Drain as much as possible from `carry` before asking the
        // network for more.
        let progressed = match state {
            St::Size => {
                if let Some(p) = carry.windows(2).position(|w| w == b"\r\n") {
                    let line = &carry[..p];
                    // Chunk extensions (";name=val") are ignored.
                    let hex_end = line.iter().position(|&b| b == b';').unwrap_or(line.len());
                    let hex = core::str::from_utf8(&line[..hex_end])
                        .map_err(|_| "chunk: bad size line")?
                        .trim();
                    let sz = usize::from_str_radix(hex, 16)
                        .map_err(|_| "chunk: bad size hex")?;
                    if !logged_first_size {
                        crate::kprintln!("[npk]   first chunk size = {} (0x{})", sz, hex);
                        logged_first_size = true;
                    }
                    carry.drain(..p + 2);
                    state = if sz == 0 { St::Done } else { St::Data(sz) };
                    true
                } else {
                    false
                }
            }
            St::Data(remaining) => {
                if carry.is_empty() {
                    false
                } else {
                    let take = remaining.min(carry.len());
                    on_chunk(&carry[..take])?;
                    delivered = delivered.saturating_add(take);
                    if delivered > max_size {
                        return Err("chunk: body exceeds max_size");
                    }
                    carry.drain(..take);
                    state = if remaining - take == 0 {
                        St::AfterData
                    } else {
                        St::Data(remaining - take)
                    };
                    true
                }
            }
            St::AfterData => {
                // Consume the CRLF that terminates a chunk's data.
                if carry.len() >= 2 {
                    carry.drain(..2);
                    state = St::Size;
                    true
                } else {
                    false
                }
            }
            St::Done => return Ok(delivered),
        };

        if progressed {
            continue;
        }

        // Need more bytes from the wire.
        match tls_recv_poll(tls, buf) {
            Ok(0) => continue, // transient; tls_recv_poll caps the wait
            Ok(n) => {
                if !logged_first_recv {
                    crate::kprintln!("[npk]   first wire recv = {} B", n);
                    logged_first_recv = true;
                }
                carry.extend_from_slice(&buf[..n]);
            }
            Err(e) => {
                crate::kprintln!("[npk]   chunked recv error (delivered {} B)", delivered);
                return Err(e);
            }
        }
    }
}
