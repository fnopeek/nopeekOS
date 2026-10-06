//! npkFS on-disk format (v4).
//!
//! Disk layout: block 0 reserved (MBR/GPT/UEFI), blocks 1–8 = SB slots,
//! blocks 9–264 = journal area, block 265+ = bitmap & data.
//!
//! v4 keeps v3's object shapes and changes the keys: the superblock holds
//! keyslots that wrap a random data key (`crypto::keyslot`), every object is
//! encrypted (trees included), and an object's address is a keyed BLAKE3 of
//! its bytes instead of a plain hash. v2 and v3 disks cannot be read; the
//! mount-time guard halts with a reinstall message.
//!
//! The block-level B-tree node layout is unchanged since v2, so
//! `BTREE_NODE_MAGIC` stays `"NPK2"`.

#![allow(dead_code)]

use super::types::{Extent, BLOCK_SIZE};

// ── Layout (block geometry) ───────────────────────────────────────────
pub use super::types::{
    SUPERBLOCK_SLOTS,
    SUPERBLOCK_START,
    JOURNAL_START,
    JOURNAL_BLOCKS,
    META_END,
};

// ── Format identity ───────────────────────────────────────────────────

/// v4 superblock magic. Byte 5 carries the schema version so `dd | xxd`
/// shows the generation directly next to the ASCII tag.
pub const DISK_MAGIC: [u8; 8] = *b"npkFS\x04\0\0";

/// Older superblock magics, kept so the mount-time guard can name the
/// version and ask for a reinstall.
pub const DISK_MAGIC_V2: [u8; 8] = *b"npkFS\x02\0\0";
pub const DISK_MAGIC_V3: [u8; 8] = *b"npkFS\x03\0\0";

/// On-disk format version field of the superblock.
pub const DISK_VERSION: u32 = 4;

/// Keyslots in the superblock (see `crypto::keyslot`).
pub const KEYSLOTS: usize = 2;

/// B-tree node magic. ASCII "NPK2" little-endian. The same in v2 and v3:
/// the block-level node layout (header + leaf entries keyed by 32-byte
/// hash, internal entries with 32-byte key + 8-byte child pointer) did
/// not change; only the typed Object payload above the storage layer did.
pub const BTREE_NODE_MAGIC: u32 = 0x324B504E;

pub const BTREE_INTERNAL: u8 = 1;
pub const BTREE_LEAF: u8 = 2;

// ── Per-leaf entry ────────────────────────────────────────────────────

/// Direct extents stored inline in a leaf entry. More extents go through
/// the indirect chain.
pub const DIRECT_EXTENTS: usize = 3;

/// Extents per indirect block.
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
    /// Bytes actually stored across `extents` + indirect chain:
    /// `plaintext_size + 16`, the AEAD tag appended.
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

// Reserve the 32-byte BLAKE3 checksum trailer (at BLOCK_SIZE-32), same as
// MAX_LEAF_ENTRIES does. Entry 101 would sit at offset 16+101*40 =
// 4056..4096 and overlap the checksum at 4064; writing the checksum would
// clobber its child pointer, and read_node's checksum verify would not
// notice because the clobbered bytes are the checksum. 101 keeps the last
// entry below 4064.
pub const MAX_INTERNAL_KEYS: usize =
    (BLOCK_SIZE - NODE_HEADER_SIZE - CHECKSUM_SIZE) / INTERNAL_ENTRY_SIZE; // 101
pub const MAX_LEAF_ENTRIES: usize =
    (BLOCK_SIZE - NODE_HEADER_SIZE - CHECKSUM_SIZE) / LEAF_ENTRY_SIZE; // 36

// ── Node header ───────────────────────────────────────────────────────

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

/// 4096-byte superblock.
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
/// B-tree root block address (entry point: hash → BTreeEntryRaw).
    pub btree_root: u64,
/// Hash of the root Tree object. Zero while no root Tree exists.
    pub root_tree_hash: [u8; 32],
    pub object_count: u64,
    pub journal_head: u64,
    pub journal_seq: u64,
    pub install_salt: [u8; 16],
    /// Slot 0: passphrase. Slot 1: free for a second way in (recovery key).
    pub keyslots: [[u8; crate::crypto::keyslot::SLOT_BYTES]; KEYSLOTS],
    pub _reserved: [u8; 3664],
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
