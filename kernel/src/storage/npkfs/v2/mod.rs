//! npkFS v2 — content-addressed Git-style trees.
//!
//! Replaces v1's path-as-key flat store with real directory objects.
//! See `NPKFS_V2.md` at the repo root for the design.
//!
//! Structure:
//!   `object`  — `Blob`/`Tree` wire format (postcard + BLAKE3)
//!   `format`  — on-disk superblock + B-tree node layout
//!   `sb_io`   — 8-slot rotating superblock read/write
//!   `btree`   — COW B-tree keyed by 32-byte hashes
//!   `storage` — mkfs/mount/put/get/has/remove + 4-phase commit
//!   `paths`   — slash-path walker + tree mutations on top of storage
//!   `fs`      — high-level API that flips SB.root_tree_hash atomically
//!               and exposes the GC.

pub mod object;

mod format;
mod sb_io;
mod btree;
pub mod storage;

pub mod paths;
pub mod fs;
