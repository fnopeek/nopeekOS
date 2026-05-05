//! npkFS on-disk format (v3).
//!
//! Disk layout: block 0 reserved (MBR/GPT/UEFI), blocks 1–8 = SB slots,
//! blocks 9–264 = journal area, block 265+ = bitmap & data.
//!
//! v3 (this kernel) extends the v2 `TreeEntry` shape by adding an
//! `mtime` field (UTC seconds since the Unix epoch). Old v2 disks
//! cannot be read — the on-disk magic shifted from `npkFS\x02\0\0`
//! to `npkFS\x03\0\0` and the postcard wire shape of `Tree` payloads
//! changed. Mount-time guard halts with a reinstall message when v2
//! magic is detected.
//!
//! The B-tree node layout (block-level) is unchanged across v2 → v3
//! — only the typed Object payload (TreeEntry list) shifted shape. So
//! `BTREE_NODE_MAGIC` stays `"NPK2"` even at superblock version 3.

#![allow(dead_code)]

use super::types::{Extent, BLOCK_SIZE};

// ── Layout (block geometry, unchanged since v1) ───────────────────────
pub use super::types::{
    SUPERBLOCK_SLOTS,
    SUPERBLOCK_START,
    JOURNAL_START,
    JOURNAL_BLOCKS,
    META_END,
};

// ── Format identity ───────────────────────────────────────────────────

/// v3 superblock magic. Byte 5 carries the schema version (`0x01` v1,
/// `0x02` v2, `0x03` v3) so `dd | xxd` shows the generation directly
/// next to the ASCII tag.
pub const DISK_MAGIC: [u8; 8] = *b"npkFS\x03\0\0";

/// v2 superblock magic. Kept here so the mount-time guard can detect
/// pre-mtime disks and surface a clear reinstall message.
pub const DISK_MAGIC_V2: [u8; 8] = *b"npkFS\x02\0\0";

/// On-disk version field of the v3 superblock.
pub const DISK_VERSION: u32 = 3;

/// B-tree node magic. ASCII "NPK2" little-endian. Unchanged across
/// v2 → v3: the block-level node layout (header + leaf entries keyed
/// by 32-byte hash, internal entries with 32-byte key + 8-byte child
/// pointer) didn't shift; only the typed Object payload above the
/// storage layer changed shape.
pub const BTREE_NODE_MAGIC: u32 = 0x324B504E;

pub const BTREE_INTERNAL: u8 = 1;
pub const BTREE_LEAF: u8 = 2;

// ── Per-leaf entry ────────────────────────────────────────────────────

/// Direct extents stored inline in a leaf entry. More extents go through
/// the indirect chain (same format as v1).
pub const DIRECT_EXTENTS: usize = 3;

/// Extents per indirect block (see v1's identical layout).
pub const EXTENTS_PER_INDIRECT: usize = 255;

/// B-tree leaf entry. Keyed by `hash` (the BLAKE3 of the plaintext
/// payload — same value the caller passed to `put`). Stores location +
/// sizes of the on-disk (encrypted) bytes.
///
/// Layout chosen so 36 entries fit in a 4 KB leaf (after 16 B header +
/// 32 B checksum).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct BTreeEntryRaw {
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
    /// First DIRECT_EXTENTS extents stored inline.
    pub extents: [Extent; DIRECT_EXTENTS],
    /// Address of the first indirect block (0 = none).
    pub indirect_block: u64,
}

pub const LEAF_ENTRY_SIZE: usize = 112;
const _LE_SIZE: () = assert!(core::mem::size_of::<BTreeEntryRaw>() == LEAF_ENTRY_SIZE);

// ── Per-internal entry ────────────────────────────────────────────────

/// Internal node entry: 32-byte key + 8-byte child pointer.
pub const INTERNAL_ENTRY_SIZE: usize = 40;

// ── Node capacities ───────────────────────────────────────────────────

pub const NODE_HEADER_SIZE: usize = 16;
/// 32-byte checksum trailer, BLAKE3 over the rest of the block.
pub const CHECKSUM_SIZE: usize = 32;

pub const MAX_INTERNAL_KEYS: usize =
    (BLOCK_SIZE - NODE_HEADER_SIZE) / INTERNAL_ENTRY_SIZE; // 102
pub const MAX_LEAF_ENTRIES: usize =
    (BLOCK_SIZE - NODE_HEADER_SIZE - CHECKSUM_SIZE) / LEAF_ENTRY_SIZE; // 36

// ── Node header (same shape as v1 but distinct magic) ─────────────────

#[derive(Clone, Copy)]
#[repr(C)]
pub struct BTreeNodeHeader {
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
pub struct SuperblockRaw {
    pub magic: [u8; 8],
    pub version: u32,
    pub flags: u32,
    pub generation: u64,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub bitmap_start: u64,
    pub bitmap_count: u64,
    pub data_start: u64,
    /// B-tree root block address (Step 2 entry point: hash → BTreeEntryRaw).
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

const _SB_SIZE: () = assert!(core::mem::size_of::<SuperblockRaw>() == BLOCK_SIZE);

impl SuperblockRaw {
    pub fn compute_checksum(&self) -> [u8; 32] {
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, BLOCK_SIZE - 32)
        };
        *blake3::hash(bytes).as_bytes()
    }

    pub fn is_valid(&self) -> bool {
        self.magic == DISK_MAGIC
            && self.version == DISK_VERSION
            && self.checksum == self.compute_checksum()
    }

    pub fn set_checksum(&mut self) {
        self.checksum = self.compute_checksum();
    }
}
