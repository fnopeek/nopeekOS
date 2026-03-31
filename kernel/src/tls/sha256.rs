//! SHA-256 — thin wrapper around the `sha2` crate (FIPS 180-4, audited)

use sha2::Digest;

#[derive(Clone)]
pub struct Sha256(sha2::Sha256);

impl Sha256 {
    pub fn new() -> Self {
        Sha256(sha2::Sha256::new())
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finalize(self) -> [u8; 32] {
        let result = self.0.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&result);
        out
    }
}

/// One-shot hash
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize()
}

/// One-shot SHA-384 hash
pub fn sha384(data: &[u8]) -> [u8; 48] {
    use sha2::Sha384 as Sha384Inner;
    let mut h = Sha384Inner::new();
    h.update(data);
    let result = h.finalize();
    let mut out = [0u8; 48];
    out.copy_from_slice(&result);
    out
}
