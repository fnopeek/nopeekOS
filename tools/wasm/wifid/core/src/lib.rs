//! wifid_core — vendor-independent WPA2 supplicant logic.
//!
//! `no_std` (+`alloc` not needed here) so the same code runs in `wifid.wasm`
//! and in the std dev-harness against published test vectors. This first slice
//! is the crypto foundation of WPA2-PSK:
//!
//! - SHA-1 + HMAC-SHA1 (FIPS 180/198)
//! - PBKDF2-HMAC-SHA1 → the 256-bit PMK from passphrase + SSID (IEEE 802.11i)
//! - PRF-SHA1 → the PTK from the PMK + nonces + MACs (the 4-way handshake KDF)
//!
//! The EAPOL 4-way state machine (MIC verify, GTK unwrap, key install) builds
//! on these and lands in the next slice.

#![no_std]

// ── SHA-1 ────────────────────────────────────────────────────────────────
// FIPS 180-4. 64-byte block, 20-byte digest.
pub struct Sha1 {
    h: [u32; 5],
    block: [u8; 64],
    len: usize,    // bytes buffered in `block`
    total: u64,    // total message bytes
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub fn new() -> Self {
        Sha1 {
            h: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0],
            block: [0; 64],
            len: 0,
            total: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        while !data.is_empty() {
            let n = core::cmp::min(64 - self.len, data.len());
            self.block[self.len..self.len + n].copy_from_slice(&data[..n]);
            self.len += n;
            data = &data[n..];
            if self.len == 64 {
                self.compress();
                self.len = 0;
            }
        }
    }

    fn compress(&mut self) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                self.block[i * 4],
                self.block[i * 4 + 1],
                self.block[i * 4 + 2],
                self.block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
    }

    pub fn finish(mut self) -> [u8; 20] {
        let bits = self.total * 8;
        // append 0x80, pad to 56 mod 64, then 64-bit big-endian length.
        self.update(&[0x80]);
        self.total -= 1; // the 0x80 isn't message data for length purposes
        while self.len != 56 {
            self.update(&[0]);
            self.total -= 1;
        }
        self.update(&bits.to_be_bytes());
        self.total -= 8;
        let mut out = [0u8; 20];
        for (i, hv) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&hv.to_be_bytes());
        }
        out
    }
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finish()
}

// ── HMAC-SHA1 (RFC 2104) ──────────────────────────────────────────────────
pub fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha1::new();
    inner.update(&ipad);
    inner.update(msg);
    let inner = inner.finish();
    let mut outer = Sha1::new();
    outer.update(&opad);
    outer.update(&inner);
    outer.finish()
}

// ── PBKDF2-HMAC-SHA1 (RFC 2898) → PMK ─────────────────────────────────────
// WPA2: PMK = PBKDF2(passphrase, SSID, 4096, 32). Two 20-byte blocks → 40
// bytes, truncated to 32.
pub fn pbkdf2_sha1(passphrase: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    let mut block_index: u32 = 1;
    let mut pos = 0;
    while pos < out.len() {
        // U1 = HMAC(P, salt || INT(block_index))
        let mut msg = [0u8; 64];
        let sl = salt.len();
        msg[..sl].copy_from_slice(salt);
        msg[sl..sl + 4].copy_from_slice(&block_index.to_be_bytes());
        let mut u = hmac_sha1(passphrase, &msg[..sl + 4]);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac_sha1(passphrase, &u);
            for i in 0..20 {
                t[i] ^= u[i];
            }
        }
        let n = core::cmp::min(20, out.len() - pos);
        out[pos..pos + n].copy_from_slice(&t[..n]);
        pos += n;
        block_index += 1;
    }
}

/// WPA2-PSK PMK: 32 bytes from passphrase (8..63 ASCII) + SSID.
pub fn wpa2_pmk(passphrase: &[u8], ssid: &[u8]) -> [u8; 32] {
    let mut pmk = [0u8; 32];
    pbkdf2_sha1(passphrase, ssid, 4096, &mut pmk);
    pmk
}

// ── PRF-SHA1 (IEEE 802.11i) → PTK ─────────────────────────────────────────
// PTK = PRF(PMK, "Pairwise key expansion",
//          min(AA,SA) || max(AA,SA) || min(ANonce,SNonce) || max(ANonce,SNonce))
// CCMP PTK is 48 bytes (KCK16 || KEK16 || TK16).
pub fn prf_sha1(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {
    let mut i: u8 = 0;
    let mut pos = 0;
    while pos < out.len() {
        // HMAC-SHA1(key, label || 0x00 || data || i)
        let mut msg = [0u8; 128];
        let mut n = 0;
        msg[n..n + label.len()].copy_from_slice(label);
        n += label.len();
        msg[n] = 0;
        n += 1;
        msg[n..n + data.len()].copy_from_slice(data);
        n += data.len();
        msg[n] = i;
        n += 1;
        let digest = hmac_sha1(key, &msg[..n]);
        let c = core::cmp::min(20, out.len() - pos);
        out[pos..pos + c].copy_from_slice(&digest[..c]);
        pos += c;
        i += 1;
    }
}

/// Derive the 48-byte CCMP PTK from the PMK, the two MACs and the two nonces.
/// `aa` = authenticator (AP) MAC, `sa` = supplicant (our) MAC.
pub fn wpa2_ptk(
    pmk: &[u8; 32],
    aa: &[u8; 6],
    sa: &[u8; 6],
    anonce: &[u8; 32],
    snonce: &[u8; 32],
) -> [u8; 48] {
    let mut data = [0u8; 76];
    // min(AA,SA) || max(AA,SA)
    if aa[..] < sa[..] {
        data[0..6].copy_from_slice(aa);
        data[6..12].copy_from_slice(sa);
    } else {
        data[0..6].copy_from_slice(sa);
        data[6..12].copy_from_slice(aa);
    }
    // min(ANonce,SNonce) || max(ANonce,SNonce)
    if anonce[..] < snonce[..] {
        data[12..44].copy_from_slice(anonce);
        data[44..76].copy_from_slice(snonce);
    } else {
        data[12..44].copy_from_slice(snonce);
        data[44..76].copy_from_slice(anonce);
    }
    let mut ptk = [0u8; 48];
    prf_sha1(pmk, b"Pairwise key expansion", &data, &mut ptk);
    ptk
}
