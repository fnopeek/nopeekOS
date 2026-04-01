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
    crate::net::arp::request([10, 0, 2, 2]);
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

    // Receive response
    let mut response = alloc::vec::Vec::new();
    let mut buf = [0u8; 4096];

    if let Some(ref mut sess) = tls_session {
        let mut empty_count = 0;
        loop {
            match crate::tls::tls_recv(sess, &mut buf) {
                Ok(0) => {
                    empty_count += 1;
                    if empty_count > 5 && response.is_empty() { break; }
                    if empty_count > 2 && !response.is_empty() { break; }
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
/// Supports large downloads (up to max_size bytes, default 4 MB).
pub fn https_get(host: &str, path: &str, max_size: usize) -> Result<alloc::vec::Vec<u8>, &'static str> {
    // Resolve hostname
    let ip = if let Some(ip) = parse_ip(host) {
        ip
    } else {
        crate::net::dns::resolve(host).ok_or("DNS resolution failed")?
    };

    // ARP resolve gateway
    crate::net::arp::request([10, 0, 2, 2]);
    for _ in 0..50_000 { crate::net::poll(); core::hint::spin_loop(); }

    let handle = crate::net::tcp::connect(ip, 443).map_err(|_| "TCP connect failed")?;

    let mut tls_session = match crate::tls::tls_connect(handle, host) {
        Ok(s) => s,
        Err(_) => {
            let _ = crate::net::tcp::close(handle);
            return Err("TLS handshake failed");
        }
    };

    // Send HTTP GET
    let request = alloc::format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: nopeekOS/0.1\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path, host
    );
    if crate::tls::tls_send(&mut tls_session, request.as_bytes()).is_err() {
        let _ = crate::tls::tls_close(&mut tls_session);
        return Err("HTTP send failed");
    }

    // Receive response (up to max_size)
    // Large downloads need real time-based patience, not just retry counts
    let mut response = alloc::vec::Vec::new();
    let mut buf = [0u8; 4096];
    let mut last_data_tick = crate::interrupts::ticks();
    loop {
        match crate::tls::tls_recv(&mut tls_session, &mut buf) {
            Ok(0) => {
                // No data — poll network and check timeout
                for _ in 0..5000 { crate::net::poll(); core::hint::spin_loop(); }

                let idle = crate::interrupts::ticks().wrapping_sub(last_data_tick);
                // Timeout: 3s if we have data, 1s if empty
                let timeout = if response.is_empty() { 100 } else { 300 };
                if idle > timeout { break; }
            }
            Ok(n) => {
                response.extend_from_slice(&buf[..n]);
                last_data_tick = crate::interrupts::ticks();
            }
            Err(_) => break,
        }
        if response.len() > max_size { break; }
    }
    let _ = crate::tls::tls_close(&mut tls_session);

    if response.is_empty() { return Err("empty response"); }

    // Extract body (skip HTTP headers)
    let header_end = response.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(response.len());
    let body_start = if header_end < response.len() { header_end + 4 } else { response.len() };

    // Check HTTP status
    if let Ok(hdr) = core::str::from_utf8(&response[..header_end.min(64)]) {
        if !hdr.contains("200") {
            return Err("HTTP non-200 response");
        }
    }

    Ok(response[body_start..].to_vec())
}
