//! WPA2-PSK 4-way handshake (IEEE 802.11i) — supplicant side.
//!
//! Drives the EAPOL-Key exchange that turns the PMK into installable keys:
//!
//! ```text
//!   AP → STA  msg1: ANonce                         (Pairwise, Ack)
//!   STA → AP  msg2: SNonce + RSN IE + MIC          (Pairwise, MIC)
//!   AP → STA  msg3: ANonce + enc{RSN IE, GTK} + MIC (Install, Ack, MIC, Secure, Enc)
//!   STA → AP  msg4: MIC                            (MIC, Secure)
//! ```
//!
//! After msg3 the supplicant has the PTK (KCK/KEK/TK) and the GTK. The vendor
//! driver installs TK (pairwise) and GTK (group) into the firmware via
//! ADD_STA_KEY. Key-descriptor version 2: MIC = HMAC-SHA1-128(KCK), key_data
//! wrapped with AES-Key-Wrap(KEK).

use crate::aes::aes_unwrap;
use crate::{hmac_sha1, wpa2_ptk};

// EAPOL frame field offsets (from protocol_version @0).
const O_BODY_LEN: usize = 2; // __be16
const O_KEY_INFO: usize = 5; // __be16
const O_REPLAY: usize = 9; // 8 bytes
const O_NONCE: usize = 17; // 32 bytes
const O_MIC: usize = 81; // 16 bytes
const O_KEY_DATA_LEN: usize = 97; // __be16
const O_KEY_DATA: usize = 99;
const MIC_LEN: usize = 16;
/// How long a frame `compute_mic` can hash. A longer one would be
/// truncated and every MIC would mismatch, so `on_eapol` turns it away
/// at the door and counts it instead.
const MIC_BUF: usize = 512;

// key_info bits.
const KI_PAIRWISE: u16 = 1 << 3;
const KI_INSTALL: u16 = 1 << 6;
const KI_ACK: u16 = 1 << 7;
const KI_MIC: u16 = 1 << 8;
const KI_SECURE: u16 = 1 << 9;
const KI_ENCRYPTED: u16 = 1 << 12;
/// Key Descriptor Version, bits 0-2 (`WPA_KEY_INFO_TYPE_MASK`, wpa_common.h:217).
/// wpa_supplicant echoes the REQUEST's version into every reply instead of
/// asserting one of its own.
const KI_TYPE_MASK: u16 = 0x0007;
/// Key Index, bits 4-5 (`WPA_KEY_INFO_KEY_INDEX_MASK`, wpa_common.h:224).
/// `wpa_supplicant_send_2_of_2` keeps these from the request; we dropped them.
const KI_KEY_INDEX_MASK: u16 = 0x0030;
/// Key Length offset (__be16). For RSN every reply carries ZERO here —
/// wpa_supplicant mirrors the request's value only for legacy WPA.
const O_KEY_LENGTH: usize = 7;

fn be16(b: &[u8], o: usize) -> u16 {
    ((b[o] as u16) << 8) | b[o + 1] as u16
}
fn put_be16(b: &mut [u8], o: usize, v: u16) {
    b[o] = (v >> 8) as u8;
    b[o + 1] = v as u8;
}

/// Pairwise Transient Key, split into its three sub-keys (CCMP: 16/16/16).
#[derive(Clone, Copy, Default)]
pub struct Ptk {
    pub kck: [u8; 16], // EAPOL MIC key
    pub kek: [u8; 16], // EAPOL key-encryption key (unwraps the GTK)
    pub tk: [u8; 16],  // pairwise temporal key (→ firmware)
}

/// Result of feeding one EAPOL frame to the supplicant.
pub enum Step {
    /// Not a 4-way frame we handle / ignored.
    Ignore,
    /// Reply `out[..len]` to the AP (msg2 or msg4).
    Reply(usize),
    /// msg3 processed: handshake complete. Reply msg4 = `out[..len]`, and the
    /// PTK + GTK are now available via `ptk()` / `gtk()`.
    Done(usize),
    /// A frame failed verification (bad MIC / unwrap) — abort the handshake.
    Fail,
    /// Group-key handshake done: the AP handed us a NEW GTK. Reply `out[..len]`
    /// and install the group key from `gtk()`; the pairwise key is untouched.
    Rekey(usize),
}

pub struct Supplicant {
    pmk: [u8; 32],
    aa: [u8; 6], // AP (authenticator) MAC
    sa: [u8; 6], // our (supplicant) MAC
    snonce: [u8; 32],
    rsn_ie: [u8; 64],
    rsn_len: usize,
    ptk: Ptk,
    have_ptk: bool,
    gtk: [u8; 32],
    gtk_len: usize,
    gtk_id: u8,
    /// 802.11-2020 §12.7.2: the Key Replay Counter of the last frame we
    /// accepted from the Authenticator.
    rx_replay: [u8; 8],
    rx_replay_set: bool,
    /// Frames dropped as replays, and frames seen with the SAME counter
    /// as the last one.
    pub replays_dropped: u32,
    pub replays_repeated: u32,
    /// Frames whose Key Descriptor Version we cannot compute a MIC for.
    pub bad_key_version: u32,
    /// Frames too long for `compute_mic`'s buffer.
    pub too_long: u32,
}

impl Supplicant {
    /// `rsn_ie` is the exact RSN element we put in our (re)assoc request, echoed
    /// in msg2's key_data. `snonce` must be 32 random bytes from the caller.
    pub fn new(pmk: [u8; 32], aa: [u8; 6], sa: [u8; 6], snonce: [u8; 32], rsn_ie: &[u8]) -> Self {
        let mut s = Supplicant {
            pmk,
            aa,
            sa,
            snonce,
            rsn_ie: [0; 64],
            rsn_len: rsn_ie.len().min(64),
            ptk: Ptk::default(),
            have_ptk: false,
            gtk: [0; 32],
            gtk_len: 0,
            gtk_id: 0,
            rx_replay: [0; 8],
            rx_replay_set: false,
            replays_dropped: 0,
            replays_repeated: 0,
            bad_key_version: 0,
            too_long: 0,
        };
        s.rsn_ie[..s.rsn_len].copy_from_slice(&rsn_ie[..s.rsn_len]);
        s
    }

    pub fn ptk(&self) -> Option<&Ptk> {
        if self.have_ptk { Some(&self.ptk) } else { None }
    }
    pub fn gtk(&self) -> Option<(&[u8], u8)> {
        if self.gtk_len > 0 { Some((&self.gtk[..self.gtk_len], self.gtk_id)) } else { None }
    }

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
    fn replay_ok(&mut self, frame: &[u8]) -> bool {
        let got = &frame[O_REPLAY..O_REPLAY + 8];
        if !self.rx_replay_set {
            return true;
        }
        match got.cmp(&self.rx_replay[..]) {
            core::cmp::Ordering::Less => {
                self.replays_dropped += 1;
                false
            }
            core::cmp::Ordering::Equal => {
                self.replays_repeated += 1;
                true
            }
            core::cmp::Ordering::Greater => true,
        }
    }

    fn remember_replay(&mut self, frame: &[u8]) {
        self.rx_replay.copy_from_slice(&frame[O_REPLAY..O_REPLAY + 8]);
        self.rx_replay_set = true;
    }

    /// Key Descriptor Version 2 is HMAC-SHA1-128 over the frame; that is
    /// the only one `compute_mic` can do. Version 3 (AES-128-CMAC, used
    /// with PMF and the SHA256 AKMs) would need a different MIC, and we
    /// never offer those AKMs — so a version we cannot compute means the
    /// AP chose something we did not ask for. Say so instead of failing
    /// on a MIC that was never going to match.
    fn key_version_ok(&mut self, ki: u16) -> bool {
        if ki & KI_TYPE_MASK == 2 {
            return true;
        }
        self.bad_key_version += 1;
        false
    }

    /// Feed one received EAPOL-Key frame; build the reply into `out`.
    pub fn on_eapol(&mut self, frame: &[u8], out: &mut [u8]) -> Step {
        if frame.len() < O_KEY_DATA || frame[1] != 0x03 {
            return Step::Ignore; // not EAPOL-Key
        }
        if frame.len() > MIC_BUF {
            // `compute_mic` would hash a truncated frame and every MIC
            // would mismatch. A named limit beats a silent wrong answer.
            self.too_long += 1;
            return Step::Ignore;
        }
        let ki = be16(frame, O_KEY_INFO);
        if ki & KI_ACK == 0 {
            return Step::Ignore; // not a message the AP expects an answer to
        }
        if ki & KI_PAIRWISE == 0 {
            // ── Group-key handshake (802.11-2020 §12.7.7.2) ──
            //
            // The AP renews the group key on its own schedule and expects an
            // answer. Ignoring it is not neutral: the AP retries a few times and
            // then DEAUTHENTICATES the station. That is the "connection dies
            // after a while, and the interval makes no sense" fault — measured
            // on the device as eapol in 10 / out 6 with deauth 3, all four
            // unanswered frames being this message.
            if !self.have_ptk {
                return Step::Ignore; // no KCK yet — nothing we could verify with
            }
            if !self.key_version_ok(ki) || !self.replay_ok(frame) {
                return Step::Ignore;
            }
            if !self.verify_mic(frame) {
                return Step::Fail;
            }
            self.remember_replay(frame);
            if !self.extract_gtk(frame) {
                return Step::Fail;
            }
            return Step::Rekey(self.build_group_msg2(frame, out));
        }

        if ki & KI_MIC == 0 {
            // ── msg1: ANonce, no MIC → derive PTK, send msg2. ──
            let mut anonce = [0u8; 32];
            anonce.copy_from_slice(&frame[O_NONCE..O_NONCE + 32]);
            let ptk48 = wpa2_ptk(&self.pmk, &self.aa, &self.sa, &anonce, &self.snonce);
            self.ptk.kck.copy_from_slice(&ptk48[0..16]);
            self.ptk.kek.copy_from_slice(&ptk48[16..32]);
            self.ptk.tk.copy_from_slice(&ptk48[32..48]);
            self.have_ptk = true;
            let len = self.build_msg2(frame, out);
            Step::Reply(len)
        } else {
            // ── msg3: MIC + Install + Encrypted → verify, unwrap GTK, send msg4. ──
            if !self.have_ptk {
                return Step::Fail;
            }
            if !self.key_version_ok(ki) || !self.replay_ok(frame) {
                return Step::Ignore;
            }
            if !self.verify_mic(frame) {
                return Step::Fail;
            }
            self.remember_replay(frame);
            if ki & KI_ENCRYPTED != 0 && !self.extract_gtk(frame) {
                return Step::Fail;
            }
            let len = self.build_msg4(frame, out);
            Step::Done(len)
        }
    }

    // MIC = first 16 bytes of HMAC-SHA1(KCK, frame-with-MIC-field-zeroed).
    fn compute_mic(&self, frame: &[u8]) -> [u8; MIC_LEN] {
        let mut tmp = [0u8; MIC_BUF];
        let n = frame.len().min(MIC_BUF);
        tmp[..n].copy_from_slice(&frame[..n]);
        for b in &mut tmp[O_MIC..O_MIC + MIC_LEN] {
            *b = 0;
        }
        let full = hmac_sha1(&self.ptk.kck, &tmp[..n]);
        let mut mic = [0u8; MIC_LEN];
        mic.copy_from_slice(&full[..MIC_LEN]);
        mic
    }

    fn verify_mic(&self, frame: &[u8]) -> bool {
        let want = self.compute_mic(frame);
        frame[O_MIC..O_MIC + MIC_LEN] == want
    }

    // Unwrap msg3's key_data with the KEK and pull the GTK out of its KDE list.
    fn extract_gtk(&mut self, frame: &[u8]) -> bool {
        let kdl = be16(frame, O_KEY_DATA_LEN) as usize;
        if frame.len() < O_KEY_DATA + kdl || kdl < 16 || kdl % 8 != 0 {
            return false;
        }
        let mut plain = [0u8; 256];
        if !aes_unwrap(&self.ptk.kek, &frame[O_KEY_DATA..O_KEY_DATA + kdl], &mut plain) {
            return false;
        }
        let plen = kdl - 8;
        // Walk KDEs: 0xDD <len> <OUI 3> <type 1> <data...>. GTK KDE = 00-0F-AC,1.
        let mut p = 0;
        while p + 2 <= plen {
            let id = plain[p];
            let len = plain[p + 1] as usize;
            if id == 0x00 || p + 2 + len > plen {
                break; // padding / overrun
            }
            if id == 0xDD
                && len >= 6
                && plain[p + 2] == 0x00
                && plain[p + 3] == 0x0f
                && plain[p + 4] == 0xac
                && plain[p + 5] == 0x01
            {
                // GTK KDE: OUI(3) type(1) keyid+flags(2) GTK(len-6).
                self.gtk_id = plain[p + 6] & 0x03;
                let g = len - 6;
                self.gtk_len = g.min(32);
                self.gtk[..self.gtk_len].copy_from_slice(&plain[p + 8..p + 8 + self.gtk_len]);
                return true;
            }
            p += 2 + len;
        }
        false
    }

    fn build_msg2(&self, msg1: &[u8], out: &mut [u8]) -> usize {
        let total = O_KEY_DATA + self.rsn_len;
        for b in out[..total].iter_mut() {
            *b = 0;
        }
        out[0] = msg1[0]; // protocol version (echo)
        out[1] = 0x03; // EAPOL-Key
        put_be16(out, O_BODY_LEN, (total - 4) as u16);
        out[4] = msg1[4]; // descriptor type (echo)
        // `wpa_supplicant_send_2_of_4`: version echoed, key_length zero for RSN.
        let ki_req = be16(msg1, O_KEY_INFO);
        put_be16(out, O_KEY_INFO, (ki_req & KI_TYPE_MASK) | KI_PAIRWISE | KI_MIC);
        put_be16(out, O_KEY_LENGTH, 0);
        out[O_REPLAY..O_REPLAY + 8].copy_from_slice(&msg1[O_REPLAY..O_REPLAY + 8]);
        out[O_NONCE..O_NONCE + 32].copy_from_slice(&self.snonce);
        put_be16(out, O_KEY_DATA_LEN, self.rsn_len as u16);
        out[O_KEY_DATA..total].copy_from_slice(&self.rsn_ie[..self.rsn_len]);
        let mic = self.compute_mic(&out[..total]);
        out[O_MIC..O_MIC + MIC_LEN].copy_from_slice(&mic);
        total
    }

    /// The group handshake's answer: same shape as msg4 but with the pairwise
    /// bit clear, so the AP knows which key it acknowledges.
    fn build_group_msg2(&self, req: &[u8], out: &mut [u8]) -> usize {
        let total = O_KEY_DATA; // empty key_data
        for b in out[..total].iter_mut() {
            *b = 0;
        }
        out[0] = req[0];
        out[1] = 0x03;
        put_be16(out, O_BODY_LEN, (total - 4) as u16);
        out[4] = req[4];
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
        let ki_req = be16(req, O_KEY_INFO);
        put_be16(out, O_KEY_INFO,
            (ki_req & KI_KEY_INDEX_MASK) | (ki_req & KI_TYPE_MASK) | KI_MIC | KI_SECURE);
        put_be16(out, O_KEY_LENGTH, 0);
        out[O_REPLAY..O_REPLAY + 8].copy_from_slice(&req[O_REPLAY..O_REPLAY + 8]);
        put_be16(out, O_KEY_DATA_LEN, 0);
        let mic = self.compute_mic(&out[..total]);
        out[O_MIC..O_MIC + MIC_LEN].copy_from_slice(&mic);
        total
    }

    fn build_msg4(&self, msg3: &[u8], out: &mut [u8]) -> usize {
        let total = O_KEY_DATA; // empty key_data
        for b in out[..total].iter_mut() {
            *b = 0;
        }
        out[0] = msg3[0];
        out[1] = 0x03;
        put_be16(out, O_BODY_LEN, (total - 4) as u16);
        out[4] = msg3[4];
        // `wpa_supplicant_send_4_of_4`: Secure carried over from msg3, version
        // echoed, key_length zero for RSN. This path already worked — the
        // change is conformance, not a fix.
        let ki_req = be16(msg3, O_KEY_INFO);
        put_be16(out, O_KEY_INFO,
            (ki_req & KI_SECURE) | (ki_req & KI_TYPE_MASK) | KI_PAIRWISE | KI_MIC);
        put_be16(out, O_KEY_LENGTH, 0);
        out[O_REPLAY..O_REPLAY + 8].copy_from_slice(&msg3[O_REPLAY..O_REPLAY + 8]);
        put_be16(out, O_KEY_DATA_LEN, 0);
        let mic = self.compute_mic(&out[..total]);
        out[O_MIC..O_MIC + MIC_LEN].copy_from_slice(&mic);
        total
    }
}
