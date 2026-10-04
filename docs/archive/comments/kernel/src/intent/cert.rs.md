# `kernel/src/intent/cert.rs` @ 5e0102684

## L1-10 · `use alloc::string::String;`

```
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
```

## L46-47 · `fn list() {`

```
/// One line per anchor, provenance first — the point of the command is
/// answering "who can vouch for a server on this machine".
```

## L64-66 · `if let Some(n) = name {`

```
// The filename is what `cert remove` takes, so it has to be visible
// somewhere — indented under its own entry rather than in a column
// that would push the subject out of the terminal.
```

## L92-94 · `if !info.is_ca {`

```
// A leaf certificate in the trust store would never anchor anything —
// it cannot sign. Saying so beats a store entry that silently does
// nothing and sends the next hour into the TLS layer.
```

## L138-139 · `kprintln!("[npk]   ! no stored anchor '{}'", name);`

```
// Built-ins show up in `cert list` but live in the kernel binary, so
// this is the likely mistake — name the reason rather than "missing".
```

## L166 · `fn expiry(info: &certstore::CertInfo) -> String {`

```
/// "EXPIRED" beats a date the reader has to compare against today's.
```

## L176-177 · `let s = crate::net::ntp::format_time(unix);`

```
// Date only — the time of day of a CA expiry has never mattered to
// anyone reading this list.
```

## L191 · `fn read_cert_file(path: &str) -> Option<Vec<u8>> {`

```
/// Read a certificate from npkFS, accepting PEM or DER.
```

## L200-202 · `match pem_to_der(&data) {`

```
// PEM is what `openssl` hands people, so accepting only DER would mean
// every self-signed certificate needs a conversion on another machine
// before it can be trusted here.
```

## L209-210 · `fn pem_to_der(data: &[u8]) -> Option<Vec<u8>> {`

```
/// Extract the first CERTIFICATE block from a PEM file. `None` if the input
/// is not PEM (then it is treated as raw DER).
```

## L251 · `fn file_stem(path: &str) -> String {`

```
/// Filename component of a path, used as the anchor's name in the store.
```

