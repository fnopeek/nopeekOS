//! Content-Addressable Store
//!
//! No filesystem. No paths. No directory tree.
//! Every object is addressed by its content hash.
//!
//! Phase 1: Data structures only (no heap was available)
//! Phase 5: In-memory store with BLAKE3

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentHash {
    pub bytes: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeTag {
    Raw,
    WasmModule,
    Text,
    Structured,
    AuditEntry,
    IntentDef,
}

pub struct StoreObject {
    pub hash: ContentHash,
    pub type_tag: TypeTag,
    pub size: usize,
}
