//! npkFS v2 on-disk format.
//!
//! Reuses v1's disk-layout constants (block 0 reserved, blocks 1–8 = SB
//! slots, blocks 9–264 = journal area, block 265+ = bitmap & data) but
//! with distinct magic numbers so v1 and v2 disks can be told apart at
//! boot and never silently confused.

#![allow(dead_code)]

use super::types::{Extent, BLOCK_SIZE};

// ── Layout shared with v1 (same physical regions) ─────────────────────
pub use super::types::{
    SUPERBLOCK_SLOTS,
    SUPERBLOCK_START,
    JOURNAL_START,
    JOURNAL_BLOCKS,
    META_END,
};

// ── v2-specific magic ─────────────────────────────────────────────────

/// v2 superblock magic. v1 has byte 5 == 0x01; v2 has byte 5 == 0x02.
/// Exact byte-position so a `dd | xxd` shows it next to the v1 byte.
pub const V2_MAGIC: [u8; 8] = *b"npkFS\x02\0\0";

/// On-disk version field of the v2 superblock.
pub const V2_VERSION: u32 = 2;

/// v2 B-tree node magic. Bytes "NPK2" on disk (LE u32).
/// Different from v1's "NPKB" so a v2 walker reading a leftover v1 node
/// errors as Corrupt rather than parsing it as v2 leaf entries.
pub const V2_BTREE_MAGIC: u32 = 0x324B504E;

pub const V2_BTREE_INTERNAL: u8 = 1;
pub const V2_BTREE_LEAF: u8 = 2;

// ── Per-leaf entry ────────────────────────────────────────────────────

/// Direct extents stored inline in a leaf entry. More extents go through
/// the indirect chain (same format as v1).
pub const V2_DIRECT_EXTENTS: usize = 3;

/// Extents per indirect block (see v1's identical layout).
pub const V2_EXTENTS_PER_INDIRECT: usize = 255;

/// B-tree leaf entry. Keyed by `hash` (the BLAKE3 of the plaintext
/// payload — same value the caller passed to `put`). Stores location +
/// sizes of the on-disk (encrypted) bytes.
///
/// Layout chosen so 36 entries fit in a 4 KB leaf (after 16 B header +
/// 32 B checksum).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct V2EntryRaw {
    /// Primary key. Equals BLAKE3(plaintext). Verified on read.
    pub hash: [u8; 32],
    /// Caller's payload size (decrypted).
    pub plaintext_size: u64,
    /// Bytes actually stored across `extents` + indirect chain. Equals
    /// `plaintext_size` if the FS was formatted without a master key,
    /// `plaintext_size + 16` if the AEAD tag is appended.
    pub disk_size: u64,
    /// Total extents (direct + indirect).
    pub extent_count: u32,
    pub _pad: u32,
    /// First V2_DIRECT_EXTENTS extents stored inline.
    pub extents: [Extent; V2_DIRECT_EXTENTS],
    /// Address of the first indirect block (0 = none).
    pub indirect_block: u64,
}

pub const V2_LEAF_ENTRY_SIZE: usize = 112;
const _LE_SIZE: () = assert!(core::mem::size_of::<V2EntryRaw>() == V2_LEAF_ENTRY_SIZE);

// ── Per-internal entry ────────────────────────────────────────────────

/// Internal node entry: 32-byte key + 8-byte child pointer.
pub const V2_INTERNAL_ENTRY_SIZE: usize = 40;

// ── Node capacities ───────────────────────────────────────────────────

pub const V2_NODE_HEADER_SIZE: usize = 16;
/// 32-byte checksum trailer, BLAKE3 over the rest of the block.
pub const V2_CHECKSUM_SIZE: usize = 32;

pub const V2_MAX_INTERNAL_KEYS: usize =
    (BLOCK_SIZE - V2_NODE_HEADER_SIZE) / V2_INTERNAL_ENTRY_SIZE; // 102
pub const V2_MAX_LEAF_ENTRIES: usize =
    (BLOCK_SIZE - V2_NODE_HEADER_SIZE - V2_CHECKSUM_SIZE) / V2_LEAF_ENTRY_SIZE; // 36

// ── Node header (same shape as v1 but distinct magic) ─────────────────

#[derive(Clone, Copy)]
#[repr(C)]
pub struct V2NodeHeader {
    pub magic: u32,
    pub node_type: u8,
    pub _pad: u8,
    pub num_entries: u16,
    /// Internal nodes: rightmost child block. Leaf nodes: reserved (0).
    pub right_child: u64,
}

// ── Superblock ────────────────────────────────────────────────────────

/// 4096-byte v2 superblock. Adds `root_tree_hash` for Step 4 (path
/// walker / mutations) — populated 0 in Step 2 since no path layer exists.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct V2SuperblockRaw {
    pub magic: [u8; 8],
    pub version: u32,
    pub flags: u32,
    pub generation: u64,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub bitmap_start: u64,
    pub bitmap_count: u64,
    pub data_start: u64,
    /// B-tree root block address (Step 2 entry point: hash → V2EntryRaw).
    pub btree_root: u64,
    /// Hash of the root Tree object (Step 4+). Zero in Step 2.
    pub root_tree_hash: [u8; 32],
    pub object_count: u64,
    pub journal_head: u64,
    pub journal_seq: u64,
    pub install_salt: [u8; 16],
    pub _reserved: [u8; 3920],
    pub checksum: [u8; 32],
}

const _SB_SIZE: () = assert!(core::mem::size_of::<V2SuperblockRaw>() == BLOCK_SIZE);

impl V2SuperblockRaw {
    pub fn compute_checksum(&self) -> [u8; 32] {
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, BLOCK_SIZE - 32)
        };
        *blake3::hash(bytes).as_bytes()
    }

    pub fn is_valid(&self) -> bool {
        self.magic == V2_MAGIC
            && self.version == V2_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn set_checksum(&mut self) {
        self.checksum = self.compute_checksum();
    }
}
