# `tools/wasm/beak-engine/src/js/ws_crypto.rs` @ 5e0102684

## L1-8 · `use alloc::string::String;`

```
//! SHA-1 und base64 — die zwei Bausteine des WebSocket-Handschlags.
//!
//! **SHA-1 steht hier und nicht im Kryptomodul des Kernels, und das ist
//! Absicht.** Es ist gebrochen und darf nie wieder etwas sichern; RFC 6455
//! §4.1 benutzt es auch nicht dafuer, sondern als festen Rechenschritt gegen
//! einen Zwischenspeicher, der eine Aufruest-Anfrage fuer eine gewoehnliche
//! haelt. Wer es im Kernel neben AES und ECDSA ablegte, laedt den naechsten
//! Leser ein, es fuer eine Sicherheitsfunktion zu halten.
```

## L13 · `pub fn sha1(data: &[u8]) -> [u8; 20] {`

```
/// SHA-1 (FIPS 180-4) — 20 Bytes.
```

## L56 · `pub fn base64(data: &[u8]) -> String {`

```
/// base64 mit Fuellzeichen, wie RFC 6455 es verlangt.
```

## L74-76 · `#[test]`

```
/// Die drei Proben aus FIPS 180-4 und RFC 4648 — und die EINE aus
/// RFC 6455 §1.3, die zaehlt: sie prueft die ganze Kette, wie der Server
/// sie rechnet.
```

## L86 · `assert_eq!(hex(sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),`

```
// Ueber eine Blockgrenze hinaus (56 Bytes = genau die Fuellgrenze).
```

## L94 · `let key = "dGhlIHNhbXBsZSBub25jZQ==";`

```
// **RFC 6455 §1.3, das Beispiel des Standards selbst.**
```

