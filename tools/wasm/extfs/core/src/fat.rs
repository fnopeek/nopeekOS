//! FAT12/16/32 with VFAT long names, read side, after Linux fs/fat:
//! `fat_read_bpb` and `fat_fill_super` (inode.c), the FAT entry access of
//! fatent.c, `fat_parse_long`/`fat_parse_short`/`fat_search_long` (dir.c)
//! and `fat_time_fat2unix` (misc.c). Options as a vfat mount without any:
//! `codepage=437`, `shortname=mixed`, names compared without case, times
//! taken as UTC.

use alloc::string::String;
use alloc::vec::Vec;

use crate::part::fat_valid_media;
use crate::{cp437, Dev, IoError};

mod write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Io,
    /// Not a FAT file system.
    NotFat,
    /// An on-disk structure does not add up.
    Corrupt,
    NotFound,
    Exists,
    /// A folder to remove still holds entries.
    NotEmpty,
    NoSpace,
    /// Not a name FAT can store (`vfat_valid_longname`).
    BadName,
    /// A folder where a file was meant, or the other way round.
    WrongKind,
}

impl From<IoError> for Error {
    fn from(_: IoError) -> Self { Error::Io }
}

const MSDOS_DPS_ENTRY: usize = 32;
const ATTR_RO: u8 = 1;
const ATTR_HIDDEN: u8 = 2;
const ATTR_SYS: u8 = 4;
const ATTR_VOLUME: u8 = 8;
const ATTR_DIR: u8 = 16;
/// `ATTR_EXT`: a long-name slot.
const ATTR_EXT: u8 = ATTR_RO | ATTR_HIDDEN | ATTR_SYS | ATTR_VOLUME;
const DELETED_FLAG: u8 = 0xE5;
const CASE_LOWER_BASE: u8 = 8;
const CASE_LOWER_EXT: u8 = 16;
const FAT_START_ENT: u32 = 2;
const MAX_FAT12: u32 = 0xFF4;
const MAX_FAT16: u32 = 0xFFF4;
const MAX_FAT32: u32 = 0x0FFF_FFF6;
/// `FAT_MAX_DIR_ENTRIES`: no directory is longer.
const FAT_MAX_DIR_ENTRIES: usize = 65536;
/// Largest device read per call.
const MAX_READ: usize = 1 << 20;
const FAT_CACHE: usize = 32;

/// A file or directory: where it starts and what its entry says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    /// First cluster; 0 for an empty file and for the FAT12/16 root.
    pub first: u32,
    pub is_dir: bool,
    pub size: u64,
    /// UTC seconds since the Unix epoch, 0 = unknown.
    pub mtime: u64,
    root: bool,
    attr: u8,
    wtime: u16,
    wdate: u16,
    loc: Option<Loc>,
}

/// Where a node's entries are: its folder, the index of its 8.3 entry and
/// of the first long-name slot before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Loc {
    dir_first: u32,
    dir_root: bool,
    index: u32,
    first_slot: u32,
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    /// The 8.3 alias, as `fat_parse_short` shows it.
    pub short: String,
    /// The alias as stored.
    pub raw: [u8; 11],
    pub node: Node,
}

/// Where the last read stopped: the cluster of index `idx` in the chain
/// that starts at `first`, so a sequential read does not walk the chain
/// from the start again.
#[derive(Clone, Copy)]
struct Cursor {
    first: u32,
    idx: u64,
    clus: u32,
}

pub struct Fat<D: Dev> {
    dev: D,
    /// Device blocks per logical sector.
    bps: u64,
    sector_size: u32,
    sec_per_clus: u32,
    cluster_size: u32,
    fat_bits: u8,
    fat_start: u64,
    root_cluster: u32,
    dir_start: u64,
    dir_entries: u32,
    data_start: u64,
    max_cluster: u32,
    free_clusters: u32,
    label: String,
    dirty: bool,
    cache: Vec<(u64, Vec<u8>)>,
    cursor: Option<Cursor>,
    fats: u64,
    fat_length: u64,
    /// The FAT that is read; with mirroring (the rule) all are written.
    active_fat: u64,
    mirror: bool,
    fsinfo_sector: u64,
    /// Where the search for a free cluster starts (`prev_free`).
    prev_free: u32,
    /// Offset of the state byte in the boot sector.
    state_off: usize,
    /// FAT sectors (index within one FAT) changed in the cache.
    dirty_fat: Vec<u64>,
    /// The dirty bit is set on the medium.
    marked: bool,
    /// The time new and changed entries get, UTC seconds.
    now: u64,
    /// The file being appended to, with the start and size its entry does
    /// not show yet: written with the FAT when another operation comes.
    open: Option<(String, Node)>,
}

fn le16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }

impl<D: Dev> Fat<D> {
    /// `fat_fill_super` for vfat: read and check the BPB, size the FAT,
    /// read the label and count the free clusters.
    pub fn mount(mut dev: D) -> Result<Self, Error> {
        let bs = dev.block_size();
        if bs < 512 || dev.blocks() == 0 { return Err(Error::NotFat); }
        let mut b = alloc::vec![0u8; bs as usize];
        dev.read(0, &mut b)?;

        // `fat_read_bpb`.
        let sector_size = le16(&b, 11) as u32;
        let sec_per_clus = b[13] as u32;
        let reserved = le16(&b, 14) as u64;
        let fats = b[16] as u64;
        let dir_entries = le16(&b, 17) as u32;
        let sectors16 = le16(&b, 19) as u64;
        let media = b[21];
        let fat_length16 = le16(&b, 22) as u64;
        let total_sect = le32(&b, 32) as u64;
        let fat16_state = b[37];
        let fat32_length = le32(&b, 36) as u64;
        let fat32_root_cluster = le32(&b, 44);
        let fat32_ext_flags = le16(&b, 40);
        let fat32_info_sector = le16(&b, 48) as u64;
        let fat32_state = b[65];
        if reserved == 0 || fats == 0 || !fat_valid_media(media) { return Err(Error::NotFat); }
        if !sector_size.is_power_of_two() || !(512..=4096).contains(&sector_size) { return Err(Error::NotFat); }
        if !sec_per_clus.is_power_of_two() { return Err(Error::NotFat); }
        if fat_length16 == 0 && fat32_length == 0 { return Err(Error::NotFat); }
        // A logical sector smaller than the device block cannot be read.
        if sector_size < bs { return Err(Error::NotFat); }
        let bps = (sector_size / bs) as u64;

        let cluster_size = sector_size.checked_mul(sec_per_clus).ok_or(Error::NotFat)?;
        let mut fat_bits = 0u8;
        let mut fat_length = fat_length16;
        let mut root_cluster = 0;
        let mut fsinfo_sector = 0;
        let (mut mirror, mut active_fat) = (true, 0u64);
        if fat_length == 0 && fat32_length != 0 {
            fat_bits = 32;
            fat_length = fat32_length;
            root_cluster = fat32_root_cluster;
            fsinfo_sector = if fat32_info_sector == 0 { 1 } else { fat32_info_sector };
            // `FAT_EXT_FLAGS`: bit 7 turns mirroring off, bits 0-3 name
            // the one FAT in use.
            if fat32_ext_flags & 0x80 != 0 {
                mirror = false;
                active_fat = (fat32_ext_flags & 0xF) as u64;
                if active_fat >= fats { return Err(Error::NotFat); }
            }
        }
        let fat_start = reserved;
        let dir_per_block = sector_size / MSDOS_DPS_ENTRY as u32;
        if dir_entries & (dir_per_block - 1) != 0 { return Err(Error::NotFat); }
        let dir_start = fat_start.checked_add(fats.checked_mul(fat_length).ok_or(Error::NotFat)?).ok_or(Error::NotFat)?;
        let rootdir_sectors = dir_entries as u64 * MSDOS_DPS_ENTRY as u64 / sector_size as u64;
        let data_start = dir_start + rootdir_sectors;
        let total_sectors = if sectors16 == 0 { total_sect } else { sectors16 };
        if total_sectors < data_start { return Err(Error::NotFat); }
        let mut total_clusters = (total_sectors - data_start) / sec_per_clus as u64;
        if fat_bits != 32 {
            fat_bits = if total_clusters > MAX_FAT12 as u64 { 16 } else { 12 };
        }
        let dirty = (if fat_bits == 32 { fat32_state } else { fat16_state }) & 1 != 0;
        // `calc_fat_clusters`: the FAT must not overflow.
        let fat_clusters = fat_length * sector_size as u64 * 8 / fat_bits as u64;
        total_clusters = total_clusters.min(fat_clusters.saturating_sub(FAT_START_ENT as u64));
        let max_fat = match fat_bits { 32 => MAX_FAT32, 16 => MAX_FAT16, _ => MAX_FAT12 };
        if total_clusters > max_fat as u64 { return Err(Error::NotFat); }
        let max_cluster = total_clusters as u32 + FAT_START_ENT;
        if fat_bits == 32 && !(FAT_START_ENT..max_cluster).contains(&root_cluster) { return Err(Error::NotFat); }

        let mut fs = Fat {
            dev, bps, sector_size, sec_per_clus, cluster_size, fat_bits, fat_start,
            root_cluster, dir_start, dir_entries, data_start, max_cluster,
            free_clusters: 0, label: String::new(), dirty, cache: Vec::new(), cursor: None,
            fats, fat_length, active_fat, mirror, fsinfo_sector, prev_free: FAT_START_ENT - 1,
            state_off: if fat_bits == 32 { 65 } else { 37 },
            dirty_fat: Vec::new(), marked: false, now: 0, open: None,
        };
        fs.free_clusters = fs.count_free()?;
        fs.label = fs.read_label(&b)?;
        Ok(fs)
    }

    pub fn label(&self) -> &str { &self.label }
    /// The device, given back.
    pub fn into_dev(self) -> D { self.dev }
    pub fn dirty(&self) -> bool { self.dirty }
    pub fn fat_bits(&self) -> u8 { self.fat_bits }
    pub fn total_bytes(&self) -> u64 { (self.max_cluster - FAT_START_ENT) as u64 * self.cluster_size as u64 }
    pub fn free_bytes(&self) -> u64 { self.free_clusters as u64 * self.cluster_size as u64 }

    pub fn root(&self) -> Node {
        Node {
            first: if self.fat_bits == 32 { self.root_cluster } else { 0 }, is_dir: true, size: 0, mtime: 0,
            root: true, attr: ATTR_DIR, wtime: 0, wdate: 0, loc: None,
        }
    }

    // === Sectors and the FAT (fatent.c) ===

    fn read_sectors(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), Error> {
        let lba = sector.checked_mul(self.bps).ok_or(Error::Corrupt)?;
        let bs = self.dev.block_size() as usize;
        let mut at = 0;
        while at < buf.len() {
            let n = (buf.len() - at).min(MAX_READ);
            self.dev.read(lba + (at / bs) as u64, &mut buf[at..at + n])?;
            at += n;
        }
        Ok(())
    }

    /// Sector `k` of the FAT (counted within one FAT) through a small
    /// cache. A changed sector leaving the cache is written first.
    fn fat_sector(&mut self, k: u64) -> Result<&mut Vec<u8>, Error> {
        if let Some(i) = self.cache.iter().position(|(s, _)| *s == k) {
            let e = self.cache.remove(i);
            self.cache.push(e);
        } else {
            if self.cache.len() == FAT_CACHE {
                let old = self.cache[0].0;
                if self.dirty_fat.contains(&old) { self.fat_flush()?; }
                self.cache.remove(0);
            }
            let mut b = alloc::vec![0u8; self.sector_size as usize];
            self.read_sectors(self.fat_start + self.active_fat * self.fat_length + k, &mut b)?;
            self.cache.push((k, b));
        }
        Ok(&mut self.cache.last_mut().ok_or(Error::Io)?.1)
    }

    fn fat_byte(&mut self, off: u64) -> Result<u8, Error> {
        let ss = self.sector_size as u64;
        Ok(self.fat_sector(off / ss)?[(off % ss) as usize])
    }

    /// `fat_ent_read`: the FAT entry of cluster `entry`.
    fn fat_ent(&mut self, entry: u32) -> Result<u32, Error> {
        if entry < FAT_START_ENT || entry >= self.max_cluster { return Err(Error::Corrupt); }
        let e = entry as u64;
        Ok(match self.fat_bits {
            12 => {
                let off = e + e / 2;
                let v = self.fat_byte(off)? as u32 | (self.fat_byte(off + 1)? as u32) << 8;
                if entry & 1 != 0 { v >> 4 } else { v & 0xFFF }
            }
            16 => {
                let off = e * 2;
                self.fat_byte(off)? as u32 | (self.fat_byte(off + 1)? as u32) << 8
            }
            _ => {
                let off = e * 4;
                let mut v = 0u32;
                for k in 0..4 { v |= (self.fat_byte(off + k)? as u32) << (8 * k); }
                v & 0x0FFF_FFFF
            }
        })
    }

    /// The cluster after `c`, `None` at the end of the chain. A free or
    /// bad entry inside a chain is corruption (`fat_get_cluster`).
    fn next(&mut self, c: u32) -> Result<Option<u32>, Error> {
        let v = self.fat_ent(c)?;
        let eof = match self.fat_bits { 12 => 0xFF8, 16 => 0xFFF8, _ => 0x0FFF_FFF8 };
        if v >= eof { return Ok(None); }
        if v < FAT_START_ENT || v >= self.max_cluster { return Err(Error::Corrupt); }
        Ok(Some(v))
    }

    /// `fat_count_free_clusters`: the whole FAT, in large reads.
    fn count_free(&mut self) -> Result<u32, Error> {
        let bytes_per = match self.fat_bits { 12 => 0, 16 => 2, _ => 4 } as u64;
        let mut free = 0u32;
        if bytes_per == 0 {
            for c in FAT_START_ENT..self.max_cluster {
                if self.fat_ent(c)? == 0 { free += 1; }
            }
            return Ok(free);
        }
        let end = self.max_cluster as u64 * bytes_per;
        let ss = self.sector_size as u64;
        let chunk = (MAX_READ as u64 / ss) * ss;
        let mut off = (FAT_START_ENT as u64 * bytes_per / ss) * ss;
        let mut buf = alloc::vec![0u8; chunk as usize];
        while off < end {
            let n = (end - off).div_ceil(ss) * ss;
            let n = n.min(chunk) as usize;
            self.read_sectors(self.fat_start + self.active_fat * self.fat_length + off / ss, &mut buf[..n])?;
            let mut i = 0;
            while i + bytes_per as usize <= n {
                let ent = (off + i as u64) / bytes_per;
                if ent >= FAT_START_ENT as u64 && ent < self.max_cluster as u64 {
                    let v = if bytes_per == 2 { le16(&buf, i) as u32 } else { le32(&buf, i) & 0x0FFF_FFFF };
                    if v == 0 { free += 1; }
                }
                i += bytes_per as usize;
            }
            off += n as u64;
        }
        Ok(free)
    }

    fn cluster_sector(&self, c: u32) -> u64 {
        self.data_start + (c - FAT_START_ENT) as u64 * self.sec_per_clus as u64
    }

    // === Directories (dir.c) ===

    /// The raw 32-byte entries of a directory, at most
    /// `FAT_MAX_DIR_ENTRIES`.
    fn dir_bytes(&mut self, node: &Node) -> Result<Vec<u8>, Error> {
        if node.root && self.fat_bits != 32 {
            let mut b = alloc::vec![0u8; self.dir_entries as usize * MSDOS_DPS_ENTRY];
            if !b.is_empty() { self.read_sectors(self.dir_start, &mut b)?; }
            return Ok(b);
        }
        let mut out = Vec::new();
        let mut c = node.first;
        if c == 0 { return Ok(out); }
        let max = FAT_MAX_DIR_ENTRIES * MSDOS_DPS_ENTRY;
        let mut steps = 0u32;
        loop {
            if c < FAT_START_ENT || c >= self.max_cluster { return Err(Error::Corrupt); }
            let at = out.len();
            out.resize(at + self.cluster_size as usize, 0);
            let s = self.cluster_sector(c);
            self.read_sectors(s, &mut out[at..])?;
            if out.len() >= max { out.truncate(max); break; }
            steps += 1;
            if steps >= self.max_cluster { return Err(Error::Corrupt); }
            match self.next(c)? {
                Some(n) => c = n,
                None => break,
            }
        }
        Ok(out)
    }

    /// The entries of a directory, `.` and `..` and the volume label left
    /// out (`fat_readdir` / `__fat_readdir`).
    pub fn read_dir(&mut self, node: &Node) -> Result<Vec<DirEntry>, Error> {
        if !node.is_dir { return Err(Error::Corrupt); }
        self.settle()?;
        let raw = self.dir_bytes(node)?;
        let n = raw.len() / MSDOS_DPS_ENTRY;
        let de = |i: usize| &raw[i * MSDOS_DPS_ENTRY..(i + 1) * MSDOS_DPS_ENTRY];
        let mut out = Vec::new();
        let mut unicode = [0u16; 261];
        let mut i = 0;
        'next: while i < n {
            let mut d = de(i);
            // A free entry is skipped, not taken as the end: Linux reads
            // the directory to its last cluster.
            if d[11] != ATTR_EXT && (d[0] == 0 || d[0] == DELETED_FLAG || d[11] & ATTR_VOLUME != 0) {
                i += 1;
                continue;
            }
            let mut nr_slots = 0u8;
            let mut lfn_start = i;
            if d[11] == ATTR_EXT {
                // `fat_parse_long`.
                'parse_long: loop {
                    lfn_start = i;
                    let id = d[0];
                    if id & 0x40 == 0 { i += 1; continue 'next; }
                    let slots = id & !0x40;
                    if slots > 20 || slots == 0 { i += 1; continue 'next; }
                    nr_slots = slots;
                    let alias_checksum = d[13];
                    let mut slot = slots;
                    loop {
                        slot -= 1;
                        let offset = slot as usize * 13;
                        for k in 0..5 { unicode[offset + k] = le16(d, 1 + 2 * k); }
                        for k in 0..6 { unicode[offset + 5 + k] = le16(d, 14 + 2 * k); }
                        for k in 0..2 { unicode[offset + 11 + k] = le16(d, 28 + 2 * k); }
                        if d[0] & 0x40 != 0 { unicode[offset + 13] = 0; }
                        i += 1;
                        if i >= n { break 'next; }
                        d = de(i);
                        if slot == 0 { break; }
                        if d[11] != ATTR_EXT {
                            // Not a long name: take `d` as a plain entry.
                            nr_slots = 0;
                            break 'parse_long;
                        }
                        if (d[0] & !0x40) != slot || d[13] != alias_checksum { continue 'parse_long; }
                    }
                    if d[0] == DELETED_FLAG { i += 1; continue 'next; }
                    if d[11] == ATTR_EXT { continue 'parse_long; }
                    if d[0] == 0 || d[11] & ATTR_VOLUME != 0 { i += 1; continue 'next; }
                    if fat_checksum(&d[0..11]) != alias_checksum { nr_slots = 0; }
                    break;
                }
            }
            let index = i;
            i += 1;
            if d[0] == 0 || d[0] == DELETED_FLAG || d[11] & ATTR_VOLUME != 0 { continue; }
            if &d[0..11] == b".          " || &d[0..11] == b"..         " { continue; }
            let mut raw11 = [0u8; 11];
            raw11.copy_from_slice(&d[0..11]);
            let short = parse_short(d);
            let name = if nr_slots > 0 { utf16_name(&unicode) } else { short.clone() };
            if name.is_empty() { continue; }
            let is_dir = d[11] & ATTR_DIR != 0;
            let mut first = le16(d, 26) as u32;
            if self.fat_bits == 32 { first |= (le16(d, 20) as u32) << 16; }
            out.push(DirEntry {
                name,
                short,
                raw: raw11,
                node: Node {
                    first,
                    is_dir,
                    size: if is_dir { 0 } else { le32(d, 28) as u64 },
                    mtime: fat_time_fat2unix(le16(d, 22), le16(d, 24)),
                    root: false,
                    attr: d[11],
                    wtime: le16(d, 22),
                    wdate: le16(d, 24),
                    loc: Some(Loc {
                        dir_first: node.first,
                        dir_root: node.root,
                        index: index as u32,
                        first_slot: if nr_slots > 0 { lfn_start as u32 } else { index as u32 },
                    }),
                },
            });
        }
        Ok(out)
    }

    /// The node at `path` below the root (`/`-separated, empty = root),
    /// names matched as `vfat_find` does: long name or alias, any case.
    pub fn lookup(&mut self, path: &str) -> Result<Option<Node>, Error> {
        if let Some((p, n)) = &self.open {
            if p == path.trim_matches('/') { return Ok(Some(*n)); }
        }
        let mut node = self.root();
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            if !node.is_dir { return Ok(None); }
            let entries = self.read_dir(&node)?;
            match entries.iter().find(|e| same_name(&e.name, seg) || same_name(&e.short, seg)) {
                Some(e) => node = e.node,
                None => return Ok(None),
            }
        }
        Ok(Some(node))
    }

    /// The volume label: the root's volume entry, else the boot sector's,
    /// as blkid reads it.
    fn read_label(&mut self, boot: &[u8]) -> Result<String, Error> {
        let root = self.root();
        let raw = self.dir_bytes(&root)?;
        for d in raw.chunks_exact(MSDOS_DPS_ENTRY) {
            if d[0] == 0 || d[0] == DELETED_FLAG || d[11] == ATTR_EXT { continue; }
            if d[11] & ATTR_VOLUME != 0 && d[11] & ATTR_DIR == 0 {
                return Ok(label_from(&d[0..11]));
            }
        }
        let at = if self.fat_bits == 32 { 71 } else { 43 };
        let l = label_from(&boot[at..at + 11]);
        Ok(if l == "NO NAME" { String::new() } else { l })
    }

    // === File data ===

    /// Up to `out.len()` bytes of the file from `off`; 0 at the end.
    pub fn read(&mut self, node: &Node, off: u64, out: &mut [u8]) -> Result<usize, Error> {
        if node.is_dir { return Err(Error::Corrupt); }
        if off >= node.size || out.is_empty() { return Ok(0); }
        let len = (out.len() as u64).min(node.size - off) as usize;
        let cs = self.cluster_size as u64;
        let mut idx = off / cs;
        // The cluster of index `idx`, from the cursor when it is behind.
        let mut c = match self.cursor {
            Some(k) if k.first == node.first && k.idx <= idx => {
                let mut c = k.clus;
                for _ in k.idx..idx { c = self.next(c)?.ok_or(Error::Corrupt)?; }
                c
            }
            _ => {
                let mut c = node.first;
                if c == 0 { return Err(Error::Corrupt); }
                for _ in 0..idx { c = self.next(c)?.ok_or(Error::Corrupt)?; }
                c
            }
        };
        let mut done = 0usize;
        let mut in_clus = (off % cs) as usize;
        while done < len {
            if c < FAT_START_ENT || c >= self.max_cluster { return Err(Error::Corrupt); }
            // A run of contiguous clusters in one read.
            let want = len - done + in_clus;
            let mut run = 1u64;
            let mut last = c;
            while (run * cs) < want as u64 && run * cs < MAX_READ as u64 {
                match self.next(last)? {
                    Some(n) if n == last + 1 => { last = n; run += 1; }
                    _ => break,
                }
            }
            let mut buf = alloc::vec![0u8; (run * cs) as usize];
            let s = self.cluster_sector(c);
            self.read_sectors(s, &mut buf)?;
            let n = (buf.len() - in_clus).min(len - done);
            out[done..done + n].copy_from_slice(&buf[in_clus..in_clus + n]);
            done += n;
            in_clus = 0;
            idx += run;
            self.cursor = Some(Cursor { first: node.first, idx: idx - 1, clus: last });
            if done < len {
                c = self.next(last)?.ok_or(Error::Corrupt)?;
            }
        }
        Ok(done)
    }
}

/// `fat_checksum`.
fn fat_checksum(name: &[u8]) -> u8 {
    let mut s = name[0];
    for &b in &name[1..11] {
        s = (if s & 1 != 0 { 0x80u8 } else { 0 }).wrapping_add(s >> 1).wrapping_add(b);
    }
    s
}

/// `fat_parse_short` for vfat with `shortname=mixed`: the 8.3 name, each
/// half lower-cased when its flag in `lcase` says so.
fn parse_short(d: &[u8]) -> String {
    let mut work = [0u8; 11];
    work.copy_from_slice(&d[0..11]);
    if work[0] == 0x05 { work[0] = 0xE5; }
    let lcase = d[12];
    let half = |b: &[u8], lower: bool| -> String {
        let end = b.iter().rposition(|&c| c != b' ').map_or(0, |p| p + 1);
        b[..end].iter().map(|&c| {
            let ch = cp437::to_char(c);
            if lower { ch.to_lowercase().next().unwrap_or(ch) } else { ch }
        }).collect()
    };
    let base = half(&work[0..8], lcase & CASE_LOWER_BASE != 0);
    let ext = half(&work[8..11], lcase & CASE_LOWER_EXT != 0);
    if ext.is_empty() { base } else { alloc::format!("{}.{}", base, ext) }
}

fn label_from(b: &[u8]) -> String {
    let end = b.iter().rposition(|&c| c != b' ' && c != 0).map_or(0, |p| p + 1);
    b[..end].iter().map(|&c| cp437::to_char(c)).collect()
}

/// The long name up to its terminator, UTF-16 to UTF-8; an unpaired
/// surrogate becomes U+FFFD.
fn utf16_name(u: &[u16]) -> String {
    let end = u.iter().position(|&c| c == 0).unwrap_or(u.len());
    char::decode_utf16(u[..end].iter().copied())
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn same_name(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

/// `fat_time_fat2unix`, the time zone taken as UTC. A zero date is unknown.
pub fn fat_time_fat2unix(time: u16, date: u16) -> u64 {
    if date == 0 { return 0; }
    let year = 1980 + (date >> 9) as i64;
    let month = ((date >> 5) & 0xF).clamp(1, 12) as i64;
    let day = (date & 0x1F).max(1) as i64;
    let secs = ((time & 0x1F) as i64) * 2 + ((time >> 5) & 0x3F) as i64 * 60 + (time >> 11) as i64 * 3600;
    // Days from the civil date (Howard Hinnant's `days_from_civil`).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + secs).max(0) as u64
}
