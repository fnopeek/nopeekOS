//! HTTP/HTTPS intents

use crate::{kprint, kprintln, capability};
use alloc::string::String;
use super::{parse_ip, resolve_path};

const HTTP_MAX_RESPONSE: usize = 128 * 1024; // 128 KB

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

    // Parse "host path" or "host/path"
    let (host, path) = if let Some(idx) = url.find(' ') {
        (&url[..idx], url[idx + 1..].trim())
    } else if let Some(idx) = url.find('/') {
        (&url[..idx], &url[idx..])
    } else {
        (url, "/")
    };
    let host = host.trim();

    // Check for "> name" store redirect
    let store_as = if let Some(idx) = path.find('>') {
        let name = path[idx + 1..].trim();
        if name.is_empty() { None } else { Some(String::from(name)) }
    } else {
        None
    };
    let path = if let Some(idx) = path.find('>') { path[..idx].trim() } else { path };
    let path = if path.is_empty() { "/" } else { path };

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
        let store_path = resolve_path(&name);
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

/// Reusable HTTPS GET — returns the response body as Vec<u8>.
///
/// Proper HTTP/1.1 implementation (RFC 7230):
///   Phase 1: Receive headers (read until \r\n\r\n)
///   Phase 2: Parse status + Content-Length / Transfer-Encoding
///   Phase 3: Receive body (exactly Content-Length bytes, or read-until-close)
pub fn https_get(host: &str, path: &str, max_size: usize) -> Result<alloc::vec::Vec<u8>, &'static str> {
    // Resolve hostname
    let ip = if let Some(ip) = parse_ip(host) {
        ip
    } else {
        crate::net::dns::resolve(host).ok_or("DNS resolution failed")?
    };

    // ARP resolve gateway (use actual gateway from DHCP, not hardcoded)
    let gw = crate::net::ipv4::gateway();
    crate::net::arp::request(gw);
    for _ in 0..50_000 { crate::net::poll(); core::hint::spin_loop(); }

    let handle = crate::net::tcp::connect(ip, 443).map_err(|_| "TCP connect failed")?;

    let mut tls = match crate::tls::tls_connect(handle, host) {
        Ok(s) => s,
        Err(_) => {
            let _ = crate::net::tcp::close(handle);
            return Err("TLS handshake failed");
        }
    };

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
        if raw.len() > 32_768 { return close_err(&mut tls, "headers too large"); }
    }

    let hdr_end = match header_end {
        Some(pos) => pos,
        None => return close_err(&mut tls, "no HTTP headers received"),
    };
    let body_start = hdr_end + 4;

    // ── Phase 2: Parse HTTP status + headers ───────────────────
    let hdr_str = core::str::from_utf8(&raw[..hdr_end]).map_err(|_| "invalid header encoding")?;

    // Status code (first line: "HTTP/1.1 200 OK")
    let status = parse_status_code(hdr_str).unwrap_or(0);
    if status < 200 || status >= 300 {
        let _ = crate::tls::tls_close(&mut tls);
        return Err("HTTP non-2xx response");
    }

    let content_length = parse_header_value(hdr_str, "content-length")
        .and_then(|v| v.trim().parse::<usize>().ok());
    let chunked = parse_header_value(hdr_str, "transfer-encoding")
        .map(|v| v.contains("chunked"))
        .unwrap_or(false);

    // ── Phase 3: Receive body ──────────────────────────────────
    // Body bytes we already have from phase 1 (may be partial or complete).
    let mut body = raw[body_start..].to_vec();

    if let Some(cl) = content_length {
        // Content-Length: read exactly `cl` bytes
        while body.len() < cl && body.len() < max_size {
            match tls_recv_poll(&mut tls, &mut buf) {
                Ok(0) => continue,
                Ok(n) => body.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        body.truncate(cl); // trim any excess (shouldn't happen with well-behaved servers)
    } else if chunked {
        // Transfer-Encoding: chunked — decode chunks
        let chunked_raw = body;
        body = decode_chunked(&chunked_raw, &mut tls, &mut buf, max_size);
    } else {
        // Connection: close — read until server closes (fallback per RFC 7230 §3.3.3)
        loop {
            match tls_recv_poll(&mut tls, &mut buf) {
                Ok(0) => continue,
                Ok(n) => body.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
            if body.len() > max_size { break; }
        }
    }

    let _ = crate::tls::tls_close(&mut tls);
    if body.is_empty() && content_length != Some(0) {
        return Err("empty body");
    }
    Ok(body)
}

/// TLS recv with network polling. Retries on Ok(0) up to a hard timeout.
fn tls_recv_poll(tls: &mut crate::tls::TlsSession, buf: &mut [u8]) -> Result<usize, &'static str> {
    let start = crate::interrupts::ticks();
    loop {
        for _ in 0..2000 { crate::net::poll(); core::hint::spin_loop(); }
        match crate::tls::tls_recv(tls, buf) {
            Ok(0) => {
                // TLS returned no app data (NewSessionTicket, CCS, etc.) — retry
                if crate::interrupts::ticks().wrapping_sub(start) > 1500 {
                    return Err("recv timeout"); // 15 seconds hard timeout
                }
            }
            Ok(n) => return Ok(n),
            Err(_) => return Err("recv error"),
        }
    }
}

fn close_err(tls: &mut crate::tls::TlsSession, msg: &'static str) -> Result<alloc::vec::Vec<u8>, &'static str> {
    let _ = crate::tls::tls_close(tls);
    Err(msg)
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

/// Decode chunked transfer encoding (RFC 7230 §4.1).
fn decode_chunked(
    initial: &[u8],
    tls: &mut crate::tls::TlsSession,
    buf: &mut [u8],
    max_size: usize,
) -> alloc::vec::Vec<u8> {
    // Accumulate all chunked data, then decode
    let mut raw = initial.to_vec();

    // Read until we see "0\r\n" (final chunk)
    let mut attempts = 0;
    while !has_final_chunk(&raw) && raw.len() < max_size {
        match tls_recv_poll(tls, buf) {
            Ok(0) => { attempts += 1; if attempts > 50 { break; } }
            Ok(n) => { raw.extend_from_slice(&buf[..n]); attempts = 0; }
            Err(_) => break,
        }
    }

    // Decode: each chunk is "SIZE\r\n DATA \r\n", final "0\r\n\r\n"
    let mut body = alloc::vec::Vec::new();
    let mut pos = 0;
    loop {
        // Find chunk size line
        let line_end = match raw[pos..].windows(2).position(|w| w == b"\r\n") {
            Some(p) => pos + p,
            None => break,
        };
        let size_str = match core::str::from_utf8(&raw[pos..line_end]) {
            Ok(s) => s.trim(),
            Err(_) => break,
        };
        // Chunk size may have extensions after ';' — ignore them
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let chunk_size = match usize::from_str_radix(size_hex, 16) {
            Ok(s) => s,
            Err(_) => break,
        };
        if chunk_size == 0 { break; } // final chunk

        let data_start = line_end + 2;
        let data_end = data_start + chunk_size;
        if data_end > raw.len() { break; } // incomplete
        body.extend_from_slice(&raw[data_start..data_end]);
        pos = data_end + 2; // skip trailing \r\n
        if body.len() > max_size { break; }
    }
    body
}

fn has_final_chunk(data: &[u8]) -> bool {
    // Look for "0\r\n\r\n" anywhere (final chunk marker)
    data.windows(5).any(|w| w == b"0\r\n\r\n")
        || data.windows(3).any(|w| w == b"0\r\n") // might not have trailing \r\n yet
}
