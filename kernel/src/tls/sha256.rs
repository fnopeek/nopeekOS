//! SHA-256 and SHA-384 — thin wrappers around the `sha2` crate (FIPS 180-4, audited)

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

/// One-shot SHA-256 hash
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize()
}

#[derive(Clone)]
pub struct Sha384(sha2::Sha384);

impl Sha384 {
    pub fn new() -> Self {
        Sha384(sha2::Sha384::new())
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finalize(self) -> [u8; 48] {
        let result = self.0.finalize();
        let mut out = [0u8; 48];
        out.copy_from_slice(&result);
        out
    }
}

/// One-shot SHA-384 hash
pub fn sha384(data: &[u8]) -> [u8; 48] {
    let mut h = Sha384::new();
    h.update(data);
    h.finalize()
}
