# `tools/wasm/wifid/core/src/eapol.rs` @ 5e0102684

## L1-15 · `use crate::aes::aes_unwrap;`

```
//! WPA2-PSK 4-way handshake (IEEE 802.11i) — supplicant side.
//!
//! Drives the EAPOL-Key exchange that turns the PMK into installable keys:
//!
//! `​``text
//!   AP → STA  msg1: ANonce                         (Pairwise, Ack)
//!   STA → AP  msg2: SNonce + RSN IE + MIC          (Pairwise, MIC)
//!   AP → STA  msg3: ANonce + enc{RSN IE, GTK} + MIC (Install, Ack, MIC, Secure, Enc)
//!   STA → AP  msg4: MIC                            (MIC, Secure)
//! `​``
//!
//! After msg3 the supplicant has the PTK (KCK/KEK/TK) and the GTK. The vendor
//! driver installs TK (pairwise) and GTK (group) into the firmware via
//! ADD_STA_KEY. Key-descriptor version 2: MIC = HMAC-SHA1-128(KCK), key_data
//! wrapped with AES-Key-Wrap(KEK).
```

## L20 · `const O_BODY_LEN: usize = 2; // __be16`

```
// EAPOL frame field offsets (from protocol_version @0).
```

## L21 · `const O_BODY_LEN: usize = 2; // __be16`

```
// __be16
```

## L22 · `const O_KEY_INFO: usize = 5; // __be16`

```
// __be16
```

## L23 · `const O_REPLAY: usize = 9; // 8 bytes`

```
// 8 bytes
```

## L24 · `const O_NONCE: usize = 17; // 32 bytes`

```
// 32 bytes
```

## L25 · `const O_MIC: usize = 81; // 16 bytes`

```
// 16 bytes
```

## L26 · `const O_KEY_DATA_LEN: usize = 97; // __be16`

```
// __be16
```

## L29-31 · `const MIC_BUF: usize = 512;`

```
/// How long a frame `compute_mic` can hash. A longer one would be
/// truncated and every MIC would mismatch, so `on_eapol` turns it away
/// at the door and counts it instead.
```

## L34 · `const KI_PAIRWISE: u16 = 1 << 3;`

```
// key_info bits.
```

## L41-43 · `const KI_TYPE_MASK: u16 = 0x0007;`

```
/// Key Descriptor Version, bits 0-2 (`WPA_KEY_INFO_TYPE_MASK`, wpa_common.h:217).
/// wpa_supplicant echoes the REQUEST's version into every reply instead of
/// asserting one of its own.
```

## L45-46 · `const KI_KEY_INDEX_MASK: u16 = 0x0030;`

```
/// Key Index, bits 4-5 (`WPA_KEY_INFO_KEY_INDEX_MASK`, wpa_common.h:224).
/// `wpa_supplicant_send_2_of_2` keeps these from the request; we dropped them.
```

## L48-49 · `const O_KEY_LENGTH: usize = 7;`

```
/// Key Length offset (__be16). For RSN every reply carries ZERO here —
/// wpa_supplicant mirrors the request's value only for legacy WPA.
```

## L60 · `#[derive(Clone, Copy, Default)]`

```
/// Pairwise Transient Key, split into its three sub-keys (CCMP: 16/16/16).
```

## L63 · `pub kck: [u8; 16], // EAPOL MIC key`

```
// EAPOL MIC key
```

## L64 · `pub kek: [u8; 16], // EAPOL key-encryption key (unwraps the GTK)`

```
// EAPOL key-encryption key (unwraps the GTK)
```

## L65 · `pub tk: [u8; 16],  // pairwise temporal key (→ firmware)`

```
// pairwise temporal key (→ firmware)
```

## L68 · `pub enum Step {`

```
/// Result of feeding one EAPOL frame to the supplicant.
```

## L70 · `Ignore,`

```
/// Not a 4-way frame we handle / ignored.
```

## L72 · `Reply(usize),`

```
/// Reply `out[..len]` to the AP (msg2 or msg4).
```

## L74-75 · `Done(usize),`

```
/// msg3 processed: handshake complete. Reply msg4 = `out[..len]`, and the
/// PTK + GTK are now available via `ptk()` / `gtk()`.
```

## L77 · `Fail,`

```
/// A frame failed verification (bad MIC / unwrap) — abort the handshake.
```

## L79-80 · `Rekey(usize),`

```
/// Group-key handshake done: the AP handed us a NEW GTK. Reply `out[..len]`
/// and install the group key from `gtk()`; the pairwise key is untouched.
```

## L86 · `aa: [u8; 6], // AP (authenticator) MAC`

```
// AP (authenticator) MAC
```

## L87 · `sa: [u8; 6], // our (supplicant) MAC`

```
// our (supplicant) MAC
```

## L96-97 · `rx_replay: [u8; 8],`

```
/// 802.11-2020 §12.7.2: the Key Replay Counter of the last frame we
/// accepted from the Authenticator.
```

## L100-101 · `pub replays_dropped: u32,`

```
/// Frames dropped as replays, and frames seen with the SAME counter
/// as the last one.
```

## L104 · `pub bad_key_version: u32,`

```
/// Frames whose Key Descriptor Version we cannot compute a MIC for.
```

## L106 · `pub too_long: u32,`

```
/// Frames too long for `compute_mic`'s buffer.
```

## L111-112 · `pub fn new(pmk: [u8; 32], aa: [u8; 6], sa: [u8; 6], snonce: [u8; 32], rsn_ie: &[u8]) -> Self {`

```
/// `rsn_ie` is the exact RSN element we put in our (re)assoc request, echoed
/// in msg2's key_data. `snonce` must be 32 random bytes from the caller.
```

## L144-153 · `fn replay_ok(&mut self, frame: &[u8]) -> bool {`

```
/// 802.11-2020 §12.7.2 — the Key Replay Counter.
///
/// **We drop a frame whose counter is STRICTLY LOWER than the last
/// one we accepted, and only count one that repeats it.** The strict
/// reading ("already used → discard") would also drop a legitimate
/// retransmission, and a handshake that cannot be retried is worse
/// than a replay window on a network we already trust with the PSK.
/// `replays_repeated` says whether tightening it would be safe here;
/// until that number has been seen on the device, guessing would put
/// a working path at risk.
```

## L177-182 · `fn key_version_ok(&mut self, ki: u16) -> bool {`

```
/// Key Descriptor Version 2 is HMAC-SHA1-128 over the frame; that is
/// the only one `compute_mic` can do. Version 3 (AES-128-CMAC, used
/// with PMF and the SHA256 AKMs) would need a different MIC, and we
/// never offer those AKMs — so a version we cannot compute means the
/// AP chose something we did not ask for. Say so instead of failing
/// on a MIC that was never going to match.
```

## L191 · `pub fn on_eapol(&mut self, frame: &[u8], out: &mut [u8]) -> Step {`

```
/// Feed one received EAPOL-Key frame; build the reply into `out`.
```

## L194 · `return Step::Ignore; // not EAPOL-Key`

```
// not EAPOL-Key
```

## L197-198 · `self.too_long += 1;`

```
// `compute_mic` would hash a truncated frame and every MIC
// would mismatch. A named limit beats a silent wrong answer.
```

## L204 · `return Step::Ignore; // not a message the AP expects an answer to`

```
// not a message the AP expects an answer to
```

## L207-214 · `if !self.have_ptk {`

```
// ── Group-key handshake (802.11-2020 §12.7.7.2) ──
//
// The AP renews the group key on its own schedule and expects an
// answer. Ignoring it is not neutral: the AP retries a few times and
// then DEAUTHENTICATES the station. That is the "connection dies
// after a while, and the interval makes no sense" fault — measured
// on the device as eapol in 10 / out 6 with deauth 3, all four
// unanswered frames being this message.
```

## L216 · `return Step::Ignore; // no KCK yet — nothing we could verify with`

```
// no KCK yet — nothing we could verify with
```

## L232 · `let mut anonce = [0u8; 32];`

```
// ── msg1: ANonce, no MIC → derive PTK, send msg2. ──
```

## L243 · `if !self.have_ptk {`

```
// ── msg3: MIC + Install + Encrypted → verify, unwrap GTK, send msg4. ──
```

## L262 · `fn compute_mic(&self, frame: &[u8]) -> [u8; MIC_LEN] {`

```
// MIC = first 16 bytes of HMAC-SHA1(KCK, frame-with-MIC-field-zeroed).
```

## L281 · `fn extract_gtk(&mut self, frame: &[u8]) -> bool {`

```
// Unwrap msg3's key_data with the KEK and pull the GTK out of its KDE list.
```

## L292 · `let mut p = 0;`

```
// Walk KDEs: 0xDD <len> <OUI 3> <type 1> <data...>. GTK KDE = 00-0F-AC,1.
```

## L298 · `break; // padding / overrun`

```
// padding / overrun
```

## L307 · `self.gtk_id = plain[p + 6] & 0x03;`

```
// GTK KDE: OUI(3) type(1) keyid+flags(2) GTK(len-6).
```

## L324 · `out[0] = msg1[0]; // protocol version (echo)`

```
// protocol version (echo)
```

## L325 · `out[1] = 0x03; // EAPOL-Key`

```
// EAPOL-Key
```

## L327 · `out[4] = msg1[4]; // descriptor type (echo)`

```
// descriptor type (echo)
```

## L328 · `let ki_req = be16(msg1, O_KEY_INFO);`

```
// `wpa_supplicant_send_2_of_4`: version echoed, key_length zero for RSN.
```

## L341-342 · `fn build_group_msg2(&self, req: &[u8], out: &mut [u8]) -> usize {`

```
/// The group handshake's answer: same shape as msg4 but with the pairwise
/// bit clear, so the AP knows which key it acknowledges.
```

## L344 · `let total = O_KEY_DATA; // empty key_data`

```
// empty key_data
```

## L352-361 · `let ki_req = be16(req, O_KEY_INFO);`

```
// `wpa_supplicant_send_2_of_2` (rsn_supp/wpa.c), field for field:
//
//     key_info &= WPA_KEY_INFO_KEY_INDEX_MASK;
//     key_info |= ver | WPA_KEY_INFO_SECURE | WPA_KEY_INFO_MIC;
//     if (proto == RSN) WPA_PUT_BE16(reply->key_length, 0);
//
// We dropped the Key Index bits AND mirrored key_length. Measured on
// the device: the AP repeated the same rekey four times over, always
// `key id 1`, never alternating — that is a RETRY, not a schedule. It
// was rejecting our answer, and afterwards it stops talking to us.
```

## L374 · `let total = O_KEY_DATA; // empty key_data`

```
// empty key_data
```

## L382-384 · `let ki_req = be16(msg3, O_KEY_INFO);`

```
// `wpa_supplicant_send_4_of_4`: Secure carried over from msg3, version
// echoed, key_length zero for RSN. This path already worked — the
// change is conformance, not a fix.
```

