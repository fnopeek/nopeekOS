//! RSA PKCS#1 v1.5 Signature Verification
//!
//! Supports 2048-bit and 4096-bit RSA public keys.
//! Verification only — no signing, no encryption.
//! Uses big-integer modular exponentiation with Montgomery multiplication.

use alloc::vec::Vec;
use super::sha256::sha256;

/// Maximum RSA key size in 32-bit words (4096 bits)
#[allow(dead_code)]
const MAX_WORDS: usize = 128;

/// Big unsigned integer (little-endian u32 words)
struct BigUint {
    words: Vec<u32>,
}

impl BigUint {
    fn from_be_bytes(bytes: &[u8]) -> Self {
        // Skip leading zeros
        let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
        let bytes = &bytes[start..];

        let mut words = Vec::with_capacity((bytes.len() + 3) / 4);
        // Process from least significant to most significant
        let mut i = bytes.len();
        while i > 0 {
            let end = i;
            let start = if i >= 4 { i - 4 } else { 0 };
            let mut word = 0u32;
            for j in start..end {
                word = (word << 8) | bytes[j] as u32;
            }
            words.push(word);
            i = start;
        }
        if words.is_empty() { words.push(0); }
        BigUint { words }
    }

    fn to_be_bytes(&self, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        for &w in self.words.iter().rev() {
            out.extend_from_slice(&w.to_be_bytes());
        }
        // Strip leading zeros then pad to desired length
        let start = out.iter().position(|&b| b != 0).unwrap_or(out.len().saturating_sub(1));
        let stripped = &out[start..];
        if stripped.len() >= len {
            stripped[stripped.len() - len..].to_vec()
        } else {
            let mut padded = alloc::vec![0u8; len - stripped.len()];
            padded.extend_from_slice(stripped);
            padded
        }
    }

    fn num_words(&self) -> usize {
        self.words.len()
    }

    fn bit_len(&self) -> usize {
        if self.words.is_empty() { return 0; }
        let top = self.words.len() - 1;
        let top_bits = 32 - self.words[top].leading_zeros() as usize;
        top * 32 + top_bits
    }
}

/// Modular exponentiation: base^exp mod modulus
/// Uses square-and-multiply algorithm.
fn mod_exp(base: &BigUint, exp: &BigUint, modulus: &BigUint) -> BigUint {
    let n = modulus.num_words();
    let mut result = BigUint { words: alloc::vec![0u32; n] };
    result.words[0] = 1;

    let mut b = mod_reduce(base, modulus);

    let exp_bits = exp.bit_len();
    for i in 0..exp_bits {
        let word_idx = i / 32;
        let bit_idx = i % 32;
        if word_idx < exp.words.len() && (exp.words[word_idx] >> bit_idx) & 1 == 1 {
            result = mod_mul(&result, &b, modulus);
        }
        b = mod_mul(&b, &b, modulus);
    }
    result
}

/// Modular multiplication: (a * b) mod m
fn mod_mul(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
    let n = a.words.len().max(b.words.len());
    // Schoolbook multiply
    let mut product = alloc::vec![0u64; n * 2 + 1];
    for i in 0..a.words.len() {
        let mut carry = 0u64;
        for j in 0..b.words.len() {
            let p = a.words[i] as u64 * b.words[j] as u64 + product[i + j] + carry;
            product[i + j] = p & 0xFFFFFFFF;
            carry = p >> 32;
        }
        product[i + b.words.len()] += carry;
    }

    let mut prod = BigUint {
        words: product.iter().map(|&w| w as u32).collect(),
    };
    // Trim trailing zeros
    while prod.words.len() > 1 && *prod.words.last().unwrap() == 0 {
        prod.words.pop();
    }

    mod_reduce(&prod, m)
}

/// Modular reduction: a mod m (using repeated subtraction for simplicity)
fn mod_reduce(a: &BigUint, m: &BigUint) -> BigUint {
    if cmp(a, m) < 0 {
        return BigUint { words: a.words.clone() };
    }

    // Long division approach
    let mut remainder = BigUint { words: a.words.clone() };
    let m_bits = m.bit_len();

    loop {
        let r_bits = remainder.bit_len();
        if r_bits < m_bits || cmp(&remainder, m) < 0 {
            break;
        }

        let shift = r_bits - m_bits;
        let shifted = shl(m, shift);

        if cmp(&remainder, &shifted) >= 0 {
            remainder = sub(&remainder, &shifted);
        } else if shift > 0 {
            let shifted = shl(m, shift - 1);
            if cmp(&remainder, &shifted) >= 0 {
                remainder = sub(&remainder, &shifted);
            }
        }
    }
    remainder
}

fn cmp(a: &BigUint, b: &BigUint) -> i32 {
    let alen = a.words.len();
    let blen = b.words.len();
    // Compare effective lengths (ignore trailing zeros)
    let a_eff = a.words.iter().rposition(|&w| w != 0).map(|i| i + 1).unwrap_or(0);
    let b_eff = b.words.iter().rposition(|&w| w != 0).map(|i| i + 1).unwrap_or(0);
    if a_eff != b_eff {
        return if a_eff > b_eff { 1 } else { -1 };
    }
    for i in (0..a_eff).rev() {
        let aw = if i < alen { a.words[i] } else { 0 };
        let bw = if i < blen { b.words[i] } else { 0 };
        if aw != bw {
            return if aw > bw { 1 } else { -1 };
        }
    }
    0
}

fn sub(a: &BigUint, b: &BigUint) -> BigUint {
    let n = a.words.len().max(b.words.len());
    let mut result = alloc::vec![0u32; n];
    let mut borrow: i64 = 0;
    for i in 0..n {
        let aw = if i < a.words.len() { a.words[i] as i64 } else { 0 };
        let bw = if i < b.words.len() { b.words[i] as i64 } else { 0 };
        let diff = aw - bw - borrow;
        if diff < 0 {
            result[i] = (diff + 0x100000000) as u32;
            borrow = 1;
        } else {
            result[i] = diff as u32;
            borrow = 0;
        }
    }
    while result.len() > 1 && *result.last().unwrap() == 0 {
        result.pop();
    }
    BigUint { words: result }
}

fn shl(a: &BigUint, bits: usize) -> BigUint {
    let word_shift = bits / 32;
    let bit_shift = bits % 32;

    let mut result = alloc::vec![0u32; a.words.len() + word_shift + 1];
    for i in 0..a.words.len() {
        result[i + word_shift] |= a.words[i] << bit_shift;
        if bit_shift > 0 && i + word_shift + 1 < result.len() {
            result[i + word_shift + 1] |= a.words[i] >> (32 - bit_shift);
        }
    }
    while result.len() > 1 && *result.last().unwrap() == 0 {
        result.pop();
    }
    BigUint { words: result }
}

/// Verify an RSA PKCS#1 v1.5 signature with SHA-256.
/// Returns true if signature is valid.
pub fn rsa_verify_pkcs1_sha256(
    modulus: &[u8],
    exponent: &[u8],
    message: &[u8],
    signature: &[u8],
) -> bool {
    let n = BigUint::from_be_bytes(modulus);
    let e = BigUint::from_be_bytes(exponent);
    let s = BigUint::from_be_bytes(signature);

    // s^e mod n
    let m = mod_exp(&s, &e, &n);
    let em = m.to_be_bytes(modulus.len());

    // PKCS#1 v1.5 encoding: 0x00 0x01 [0xFF padding] 0x00 [DigestInfo] [Hash]
    // DigestInfo for SHA-256:
    let digest_info_prefix: &[u8] = &[
        0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86,
        0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
        0x00, 0x04, 0x20,
    ];

    let hash = sha256(message);

    // Expected: 0x00 0x01 [FF..FF] 0x00 [prefix] [hash]
    let suffix_len = digest_info_prefix.len() + 32; // prefix + SHA-256 hash
    if em.len() < suffix_len + 11 { return false; } // Min 8 bytes of 0xFF padding

    if em[0] != 0x00 || em[1] != 0x01 { return false; }

    let pad_end = em.len() - suffix_len - 1; // position of 0x00 separator
    for i in 2..pad_end {
        if em[i] != 0xFF { return false; }
    }
    if em[pad_end] != 0x00 { return false; }

    let di_start = pad_end + 1;
    if &em[di_start..di_start + digest_info_prefix.len()] != digest_info_prefix { return false; }

    let hash_start = di_start + digest_info_prefix.len();
    em[hash_start..hash_start + 32] == hash
}
