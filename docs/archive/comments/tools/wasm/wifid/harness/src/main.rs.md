# `tools/wasm/wifid/harness/src/main.rs` @ 5e0102684

## L1-2 · `use wifid_core::{hmac_sha1, sha1, wpa2_pmk, wpa2_ptk};`

```
//! wifid dev-harness — validates wifid_core's WPA2 crypto against published
//! test vectors on the dev machine. `cargo run` → all vectors must PASS.
```

## L37 · `all &= check("sha1(abc)", &sha1(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");`

```
// FIPS 180 SHA-1.
```

## L45 · `all &= check(`

```
// RFC 2202 HMAC-SHA1 case 1.
```

## L52 · `all &= check(`

```
// IEEE 802.11i PMK vector: passphrase "password", SSID "IEEE".
```

## L59-60 · `let pmk = wpa2_pmk(b"password", b"IEEE");`

```
// PTK derivation smoke test (Jouni Malinen's well-known 4-way vector):
// PMK all-zero variant is deterministic; assert it runs and is stable.
```

## L68 · `all &= ptk.iter().any(|&b| b != 0); // sanity: non-trivial output`

```
// sanity: non-trivial output
```

## L70 · `let aes = wifid_core::aes::Aes128::new(&hex16("000102030405060708090a0b0c0d0e0f"));`

```
// AES-128 single block (FIPS-197 appendix B / C.1).
```

## L77 · `let kek = hex16("000102030405060708090a0b0c0d0e0f");`

```
// AES Key Unwrap — RFC 3394 §4.1 (128-bit KEK, 128-bit key).
```

## L88-91 · `all &= four_way_roundtrip();`

```
// ── 4-way handshake self-consistency (full state machine) ──────────────
// Build a synthetic AP side with the same independently-vector-tested
// primitives, run the supplicant through msg1→msg4, and check it derives the
// right PTK, accepts the MICs, and unwraps the exact GTK we wrapped.
```

## L94 · `all &= hardening();`

```
// ── Die Haerteregeln aus 0.12.0 ───────────────────────────────────────
```

## L108 · `let aa = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55]; // AP`

```
// AP
```

## L109 · `let sa = [0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb]; // us`

```
// us
```

## L116 · `let gtk = hexn("000102030405060708090a0b0c0d0e0f"); // 16-byte group key`

```
// 16-byte group key
```

## L121 · `let mut msg1 = vec![0u8; 99];`

```
// AP msg1: Pairwise|Ack, ANonce, no MIC.
```

## L128 · `msg1[9 + 7] = 1; // replay counter = 1`

```
// replay counter = 1
```

## L133 · `ok &= &out[17..49] == &snonce[..];`

```
// msg2 must carry SNonce, our RSN IE, and a valid MIC.
```

## L146 · `let mut kde = vec![0xdd, (6 + gtk.len()) as u8, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00];`

```
// AP msg3: Pairwise|Ack|MIC|Install|Secure|Encrypted, ANonce, enc{GTK KDE}.
```

## L148 · `kde.extend_from_slice(&gtk); // 24 bytes, multiple of 8`

```
// 24 bytes, multiple of 8
```

## L157 · `msg3[9 + 7] = 2; // replay counter = 2`

```
// replay counter = 2
```

## L161 · `let m3mic = hmac_sha1(&kck, &msg3)[..16].to_vec(); // MIC field already zero`

```
// MIC field already zero
```

## L173-178 · `let mut gok = true;`

```
// ── Group-key handshake (802.11-2020 §12.7.7) ──
//
// The AP renews the group key on its own schedule. Ignoring the message is
// not neutral — the AP retries and then deauthenticates — which is what a
// link that "dies after a while, at no sensible interval" looks like from
// the outside. Same shape as msg3 but with the pairwise bit CLEAR.
```

## L190 · `put_be16(&mut grp, 5, 0x0002 | (1 << 7) | (1 << 8) | (1 << 9) | (1 << 12));`

```
// Ack | MIC | Secure | Encrypted — and NO Pairwise bit.
```

## L193 · `grp[9 + 7] = 3; // replay counter = 3`

```
// replay counter = 3
```

## L201 · `gok &= sup.gtk().map(|(g, id)| g == &gtk2[..] && id == 2).unwrap_or(false);`

```
// The new GTK must be installed…
```

## L203-204 · `let ki = ((out[5] as u16) << 8) | out[6] as u16;`

```
// …and the answer must echo the replay counter, carry a valid MIC,
// and leave the pairwise bit clear so the AP knows which key it is.
```

## L220-225 · `fn hardening() -> bool {`

```
/// Die Haerteregeln aus wifid 0.12.0, jede gegen ihren eigenen Rahmen.
///
/// **Der Handschlag laeuft dabei ganz durch** — eine Regel, die das
/// Funktionierende bricht, ist keine Haertung, und genau das ist hier
/// das Risiko: ein zu strenger Wiedereinspielzaehler wirft eine
/// legitime Wiederholung weg und die Verbindung kommt nie zustande.
```

## L242 · `let mut sup = Supplicant::new(pmk, aa, sa, snonce, &rsn);`

```
// Einen Supplicant bis nach msg3 fahren.
```

## L283 · `let m3 = build_msg3(2, 0x0002);`

```
// (1) Das echte msg3 mit Zaehler 2 geht durch.
```

## L289-290 · `let rep_before = sup.replays_repeated;`

```
// (2) DASSELBE msg3 noch einmal — eine Wiederholung, und sie MUSS
//     durchgehen, sonst haben wir einen Handschlag ohne Wiederholung.
```

## L298 · `let m3_old = build_msg3(1, 0x0002);`

```
// (3) Ein ALTES msg3 (Zaehler 1) wird verworfen.
```

## L307 · `let m3_v3 = build_msg3(9, 0x0003);`

```
// (4) Key Descriptor Version 3 (AES-CMAC) koennen wir nicht rechnen.
```

## L315-316 · `let mut huge = vec![0u8; 600];`

```
// (5) Ein Rahmen laenger als der MIC-Puffer wird abgewiesen, nicht
//     abgeschnitten.
```

