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

// key_info bits.
const KI_PAIRWISE: u16 = 1 << 3;
const KI_INSTALL: u16 = 1 << 6;
const KI_ACK: u16 = 1 << 7;
const KI_MIC: u16 = 1 << 8;
const KI_SECURE: u16 = 1 << 9;
const KI_ENCRYPTED: u16 = 1 << 12;
const KEY_DESC_VER_2: u16 = 2; // AES / HMAC-SHA1-128

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

    /// Feed one received EAPOL-Key frame; build the reply into `out`.
    pub fn on_eapol(&mut self, frame: &[u8], out: &mut [u8]) -> Step {
        if frame.len() < O_KEY_DATA || frame[1] != 0x03 {
            return Step::Ignore; // not EAPOL-Key
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
            if !self.verify_mic(frame) {
                return Step::Fail;
            }
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
            if !self.verify_mic(frame) {
                return Step::Fail;
            }
            if ki & KI_ENCRYPTED != 0 && !self.extract_gtk(frame) {
                return Step::Fail;
            }
            let len = self.build_msg4(frame, out);
            Step::Done(len)
        }
    }

    // MIC = first 16 bytes of HMAC-SHA1(KCK, frame-with-MIC-field-zeroed).
    fn compute_mic(&self, frame: &[u8]) -> [u8; MIC_LEN] {
        let mut tmp = [0u8; 512];
        let n = frame.len().min(512);
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
        put_be16(out, O_KEY_INFO, KEY_DESC_VER_2 | KI_PAIRWISE | KI_MIC);
        // key_length echoes msg1; replay counter echoes msg1.
        out[7] = msg1[7];
        out[8] = msg1[8];
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
        put_be16(out, O_KEY_INFO, KEY_DESC_VER_2 | KI_MIC | KI_SECURE);
        // key_length and the replay counter are echoed, as in every reply.
        out[7] = req[7];
        out[8] = req[8];
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
        put_be16(out, O_KEY_INFO, KEY_DESC_VER_2 | KI_PAIRWISE | KI_MIC | KI_SECURE);
        out[7] = msg3[7];
        out[8] = msg3[8];
        out[O_REPLAY..O_REPLAY + 8].copy_from_slice(&msg3[O_REPLAY..O_REPLAY + 8]);
        put_be16(out, O_KEY_DATA_LEN, 0);
        let mic = self.compute_mic(&out[..total]);
        out[O_MIC..O_MIC + MIC_LEN].copy_from_slice(&mic);
        total
    }
}
