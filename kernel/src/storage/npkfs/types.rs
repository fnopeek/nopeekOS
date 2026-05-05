//! npkFS on-disk and in-memory types — shared scaffolding (block
//! geometry, journal layout, FsError) used by every other npkfs
//! submodule. Per-format constants (superblock magic, BTree node
//! magic + sizes) live in `format.rs` so they can move together
//! when the schema bumps.

pub const BLOCK_SIZE: usize = 4096;

/// Aligned 4KB buffer for safe casting to on-disk structs
#[repr(C, align(16))]
pub struct AlignedBlock(pub [u8; BLOCK_SIZE]);

impl AlignedBlock {
    pub const fn zeroed() -> Self { AlignedBlock([0u8; BLOCK_SIZE]) }
}

pub const JOURNAL_MAGIC: [u8; 8] = *b"npkJRNL\0";

pub const SUPERBLOCK_SLOTS: u64 = 8;
pub const SUPERBLOCK_START: u64 = 1; // Block 0 reserved (MBR/GPT/UEFI)
pub const JOURNAL_START: u64 = SUPERBLOCK_START + SUPERBLOCK_SLOTS; // 9
pub const JOURNAL_BLOCKS: u64 = 256;
pub const META_END: u64 = JOURNAL_START + JOURNAL_BLOCKS; // 265

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Extent {
    pub start_block: u64,
    pub block_count: u64,
}

impl Extent {
    pub const ZERO: Self = Extent { start_block: 0, block_count: 0 };
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
    ReservedName,
    DiskFull,
    TreeTooDeep,
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
            FsError::ReservedName => write!(f, "reserved name"),
            FsError::DiskFull => write!(f, "disk full"),
            FsError::TreeTooDeep => write!(f, "B-tree too deep"),
            FsError::TooManyExtents => write!(f, "too many extents"),
        }
    }
}
