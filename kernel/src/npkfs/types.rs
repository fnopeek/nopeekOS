//! npkFS on-disk and in-memory types

pub const BLOCK_SIZE: usize = 4096;

/// Aligned 4KB buffer for safe casting to on-disk structs
#[repr(C, align(16))]
pub struct AlignedBlock(pub [u8; BLOCK_SIZE]);

impl AlignedBlock {
    pub const fn zeroed() -> Self { AlignedBlock([0u8; BLOCK_SIZE]) }
}
pub const MAGIC: [u8; 8] = *b"npkFS\x01\0\0";
pub const JOURNAL_MAGIC: [u8; 8] = *b"npkJRNL\0";
pub const BTREE_MAGIC: u32 = 0x4E504B42; // "NPKB"
pub const VERSION: u32 = 1;

pub const SUPERBLOCK_SLOTS: u64 = 8;
pub const SUPERBLOCK_START: u64 = 1; // Block 0 reserved (MBR/GPT/UEFI)
pub const JOURNAL_START: u64 = SUPERBLOCK_START + SUPERBLOCK_SLOTS; // 9
pub const JOURNAL_BLOCKS: u64 = 256;
pub const META_END: u64 = JOURNAL_START + JOURNAL_BLOCKS; // 265

pub const MAX_NAME_LEN: usize = 63;
pub const MAX_EXTENTS: usize = 4;
pub const BTREE_INTERNAL: u8 = 1;
pub const BTREE_LEAF: u8 = 2;

// Per-node capacities
pub const NODE_HEADER_SIZE: usize = 16;
pub const INTERNAL_ENTRY_SIZE: usize = 72; // 64 name + 8 child ptr
pub const MAX_INTERNAL_KEYS: usize = (BLOCK_SIZE - NODE_HEADER_SIZE) / INTERNAL_ENTRY_SIZE; // 56
pub const LEAF_ENTRY_SIZE: usize = 216; // with 256-bit CapId
pub const MAX_LEAF_ENTRIES: usize = (BLOCK_SIZE - NODE_HEADER_SIZE) / LEAF_ENTRY_SIZE; // 18

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Extent {
    pub start_block: u64,
    pub block_count: u64,
}

impl Extent {
    pub const ZERO: Self = Extent { start_block: 0, block_count: 0 };
}

/// On-disk superblock (exactly 4096 bytes)
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
    pub btree_root: u64,
    pub object_count: u64,
    pub journal_head: u64,
    pub journal_seq: u64,
    pub install_salt: [u8; 16],
    pub _reserved: [u8; 3952],
    pub checksum: [u8; 32],
}

const _SB_SIZE_CHECK: () = assert!(core::mem::size_of::<SuperblockRaw>() == BLOCK_SIZE);

impl SuperblockRaw {
    pub fn compute_checksum(&self) -> [u8; 32] {
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, BLOCK_SIZE - 32)
        };
        *blake3::hash(bytes).as_bytes()
    }

    #[allow(dead_code)]
    pub fn is_valid(&self) -> bool {
        self.magic == MAGIC && self.version == VERSION && self.checksum == self.compute_checksum()
    }

    pub fn set_checksum(&mut self) {
        self.checksum = self.compute_checksum();
    }
}

/// B-tree leaf entry (216 bytes)
#[derive(Clone, Copy)]
#[repr(C)]
pub struct ObjectEntry {
    pub name: [u8; 64],
    pub content_hash: [u8; 32],
    pub size: u64,
    pub cap_id: [u8; 32],
    pub created_tick: u64,
    pub extent_count: u32,
    pub extents: [Extent; MAX_EXTENTS],
}

const _OE_SIZE_CHECK: () = assert!(core::mem::size_of::<ObjectEntry>() == LEAF_ENTRY_SIZE);

impl ObjectEntry {
    pub fn name_str(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(64);
        core::str::from_utf8(&self.name[..len]).unwrap_or("<invalid>")
    }
}

/// B-tree node header (16 bytes, start of every node block)
#[derive(Clone, Copy)]
#[repr(C)]
pub struct BTreeNodeHeader {
    pub magic: u32,
    pub node_type: u8,
    pub _pad: u8,
    pub num_entries: u16,
    pub next_leaf: u64, // leaf: next sibling, internal: rightmost child
}

/// On-disk journal header (at JOURNAL_START)
#[derive(Clone, Copy)]
#[repr(C)]
#[allow(dead_code)]
pub struct JournalHeader {
    pub magic: [u8; 8],
    pub seq: u64,
    pub entry_count: u32,
    pub committed: u32, // 1 = committed, 0 = in-progress
    // Followed by entry_count * JournalFreeEntry
}

#[derive(Clone, Copy)]
#[repr(C)]
#[allow(dead_code)]
pub struct JournalFreeEntry {
    pub start_block: u64,
    pub count: u64,
}

pub const MAX_JOURNAL_ENTRIES: usize = (BLOCK_SIZE - 24) / 16; // 254

#[derive(Debug)]
pub enum FsError {
    Disk(crate::virtio_blk::BlkError),
    NotFormatted,
    NotMounted,
    Corrupt,
    ObjectNotFound,
    ObjectExists,
    NameTooLong,
    InvalidName,
    DiskFull,
    #[allow(dead_code)]
    TooManyExtents,
}

impl From<crate::virtio_blk::BlkError> for FsError {
    fn from(e: crate::virtio_blk::BlkError) -> Self { FsError::Disk(e) }
}

impl core::fmt::Display for FsError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            FsError::Disk(e) => write!(f, "disk: {}", e),
            FsError::NotFormatted => write!(f, "disk not formatted"),
            FsError::NotMounted => write!(f, "filesystem not mounted"),
            FsError::Corrupt => write!(f, "filesystem corrupt"),
            FsError::ObjectNotFound => write!(f, "object not found"),
            FsError::ObjectExists => write!(f, "object already exists"),
            FsError::NameTooLong => write!(f, "name too long (max 63)"),
            FsError::InvalidName => write!(f, "invalid name"),
            FsError::DiskFull => write!(f, "disk full"),
            FsError::TooManyExtents => write!(f, "too many extents"),
        }
    }
}
