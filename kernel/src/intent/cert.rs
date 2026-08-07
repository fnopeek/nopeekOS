//! `cert` — inspect and manage the trust store.
//!
//! Trusting a root CA means every certificate it signs is accepted by the
//! whole system. That is a decision, not a file copy, so `cert add` shows
//! what is being trusted — subject, validity, fingerprint — and asks
//! before it takes effect.
//!
//! Anchors compiled into the kernel are shown but cannot be removed here:
//! they are the floor that keeps the machine able to reach its own
//! updates. See `crypto/tls/certstore.rs`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::kprintln;
use crate::tls::certstore::{self, STORE_DIR};

pub fn intent_cert(args: &str) {
    let mut it = args.split_whitespace();
    let sub = it.next().unwrap_or("list");
    let rest = it.next();

    match sub {
        "list" | "ls" => list(),
        "add" | "trust" => match rest {
            Some(path) => add(path),
            None => kprintln!("[npk] usage: cert add <file>"),
        },
        "remove" | "rm" | "untrust" => match rest {
            Some(name) => remove(name),
            None => kprintln!("[npk] usage: cert remove <name>"),
        },
        "show" | "info" => match rest {
            Some(path) => show(path),
            None => kprintln!("[npk] usage: cert show <file>"),
        },
        _ => {
            kprintln!("[npk] cert list             trusted root CAs");
            kprintln!("[npk] cert show <file>      inspect a certificate without trusting it");
            kprintln!("[npk] cert add <file>       trust a root CA (asks first)");
            kprintln!("[npk] cert remove <name>    stop trusting a stored root CA");
        }
    }
}

/// One line per anchor, provenance first — the point of the command is
/// answering "who can vouch for a server on this machine".
fn list() {
    kprintln!("[npk]");
    kprintln!("[npk]   {:<10} {:<44} {}", "ORIGIN", "SUBJECT", "EXPIRES");

    let mut builtin = 0usize;
    let mut stored = 0usize;
    certstore::for_each_anchor(|name, der| {
        let info = match certstore::describe(der) {
            Some(i) => i,
            None => return,
        };
        let origin = match name {
            None => { builtin += 1; String::from("built-in") }
            Some(_) => { stored += 1; String::from("stored") }
        };
        kprintln!("[npk]   {:<10} {:<44} {}", origin, truncate(&info.subject, 44), expiry(&info));
        // The filename is what `cert remove` takes, so it has to be visible
        // somewhere — indented under its own entry rather than in a column
        // that would push the subject out of the terminal.
        if let Some(n) = name {
            kprintln!("[npk]   {:<10} {}", "", n);
        }
    });

    kprintln!("[npk]");
    kprintln!("[npk]   {} built-in (not removable), {} stored in {}", builtin, stored, STORE_DIR);
}

fn show(path: &str) {
    let Some(der) = read_cert_file(path) else { return };
    let Some(info) = certstore::describe(&der) else {
        kprintln!("[npk]   ! {} is not a certificate", path);
        return;
    };
    print_cert(&info);
}

fn add(path: &str) {
    let Some(der) = read_cert_file(path) else { return };
    let Some(info) = certstore::describe(&der) else {
        kprintln!("[npk]   ! {} is not a certificate", path);
        return;
    };

    // A leaf certificate in the trust store would never anchor anything —
    // it cannot sign. Saying so beats a store entry that silently does
    // nothing and sends the next hour into the TLS layer.
    if !info.is_ca {
        kprintln!("[npk]   ! this is not a CA certificate (no BasicConstraints CA)");
        kprintln!("[npk]     Only a CA can anchor a chain — trusting this would have no effect.");
        return;
    }

    let name = file_stem(path);
    if !safe_name(&name) {
        kprintln!("[npk]   ! '{}' is not a usable anchor name (A-Z a-z 0-9 . - _)", name);
        return;
    }
    let dest = alloc::format!("{}/{}", STORE_DIR, name);
    if crate::npkfs::exists(&dest) {
        kprintln!("[npk]   ! {} already exists — remove it first", dest);
        return;
    }

    print_cert(&info);
    kprintln!("[npk]");
    kprintln!("[npk]   Trusting this CA means every certificate it signs is accepted");
    kprintln!("[npk]   by this system. Compare the fingerprint against the issuer's");
    kprintln!("[npk]   own published value before continuing.");
    kprintln!("[npk]");
    if !super::confirm("Trust this root CA?") {
        kprintln!("[npk]   . nothing changed");
        return;
    }

    if let Err(e) = crate::npkfs::store(&dest, &der, [0u8; 32]) {
        kprintln!("[npk]   ! could not write {}: {:?}", dest, e);
        return;
    }
    let n = certstore::load_store();
    kprintln!("[npk]   * trusted — {} stored anchor(s)", n);
}

fn remove(name: &str) {
    if !safe_name(name) {
        kprintln!("[npk]   ! '{}' is not a valid anchor name", name);
        return;
    }
    let dest = alloc::format!("{}/{}", STORE_DIR, name);
    if !crate::npkfs::exists(&dest) {
        // Built-ins show up in `cert list` but live in the kernel binary, so
        // this is the likely mistake — name the reason rather than "missing".
        kprintln!("[npk]   ! no stored anchor '{}'", name);
        kprintln!("[npk]     Built-in anchors cannot be removed; they are what keeps");
        kprintln!("[npk]     this machine able to reach its own updates.");
        return;
    }

    if let Err(e) = crate::npkfs::delete(&dest) {
        kprintln!("[npk]   ! could not remove {}: {:?}", dest, e);
        return;
    }
    let n = certstore::load_store();
    kprintln!("[npk]   * removed — {} stored anchor(s)", n);
}

fn print_cert(info: &certstore::CertInfo) {
    kprintln!("[npk]");
    kprintln!("[npk]   Subject      {}", info.subject);
    kprintln!("[npk]   Issuer       {}{}", info.issuer,
        if info.self_signed { " (self-signed)" } else { "" });
    kprintln!("[npk]   Valid        {} .. {}",
        info.not_before.map(fmt_date).unwrap_or_else(|| String::from("?")),
        info.not_after.map(fmt_date).unwrap_or_else(|| String::from("?")));
    kprintln!("[npk]   CA           {}", if info.is_ca { "yes" } else { "no" });
    kprintln!("[npk]   SHA-256      {}", certstore::fingerprint_hex(&info.fingerprint));
}

/// "EXPIRED" beats a date the reader has to compare against today's.
fn expiry(info: &certstore::CertInfo) -> String {
    let Some(na) = info.not_after else { return String::from("?") };
    match crate::net::ntp::unix_time().or_else(crate::drivers::rtc::read_unix_time) {
        Some(now) if now > na => String::from("EXPIRED"),
        _ => fmt_date(na),
    }
}

fn fmt_date(unix: u64) -> String {
    // Date only — the time of day of a CA expiry has never mattered to
    // anyone reading this list.
    let s = crate::net::ntp::format_time(unix);
    s.split(' ').next().unwrap_or(&s).into()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return String::from(s);
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('~');
    out
}

/// Read a certificate from npkFS, accepting PEM or DER.
fn read_cert_file(path: &str) -> Option<Vec<u8>> {
    let data = match crate::npkfs::fetch(path) {
        Ok((d, _)) => d,
        Err(_) => {
            kprintln!("[npk]   ! cannot read {}", path);
            return None;
        }
    };
    // PEM is what `openssl` hands people, so accepting only DER would mean
    // every self-signed certificate needs a conversion on another machine
    // before it can be trusted here.
    match pem_to_der(&data) {
        Some(der) => Some(der),
        None => Some(data),
    }
}

/// Extract the first CERTIFICATE block from a PEM file. `None` if the input
/// is not PEM (then it is treated as raw DER).
fn pem_to_der(data: &[u8]) -> Option<Vec<u8>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let text = core::str::from_utf8(data).ok()?;
    let start = text.find(BEGIN)? + BEGIN.len();
    let end = text[start..].find(END)? + start;
    base64_decode(text[start..end].as_bytes())
}

fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &c in input {
        if c == b'\n' || c == b'\r' || c == b' ' || c == b'\t' {
            continue;
        }
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Filename component of a path, used as the anchor's name in the store.
fn file_stem(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    String::from(base)
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'_')
}
