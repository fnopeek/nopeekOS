//! exFAT, read and write, after Linux fs/exfat: the boot region and its
//! checksum (super.c `exfat_read_boot_sector`, `exfat_verify_boot_region`),
//! the FAT and the allocation bitmap (fatent.c, balloc.c), the up-case
//! table (nls.c `exfat_load_upcase_table`), entry sets with their checksum
//! (dir.c), names compared through the up-case table, times in UTC with
//! the offset field (misc.c). New files and folders chain their clusters
//! through the FAT; files written elsewhere without a chain are read as
//! one contiguous run, as `ALLOC_NO_FAT_CHAIN` says.

use alloc::string::String;
use alloc::vec::Vec;

use crate::fat::Error;
use crate::{Dev, Volume};

const BOOT_SIGNATURE: u16 = 0xAA55;
const EXBOOT_SIGNATURE: u32 = 0xAA55_0000;
const VOLUME_DIRTY: u16 = 0x0002;
const EXFAT_EOF_CLUSTER: u32 = 0xFFFF_FFFF;
const EXFAT_BAD_CLUSTER: u32 = 0xFFFF_FFF7;
const EXFAT_FIRST_CLUSTER: u32 = 2;
const DENTRY: usize = 32;
const MAX_EXFAT_DENTRIES: u64 = 8_388_608;

const EXFAT_UNUSED: u8 = 0x00;
const EXFAT_BITMAP: u8 = 0x81;
const EXFAT_UPCASE: u8 = 0x82;
const EXFAT_VOLUME: u8 = 0x83;
const EXFAT_FILE: u8 = 0x85;
const EXFAT_STREAM: u8 = 0xC0;
const EXFAT_NAME: u8 = 0xC1;

const ATTR_SUBDIR: u16 = 0x0010;
const ATTR_ARCHIVE: u16 = 0x0020;
/// `ALLOC_POSSIBLE`; with bit 1 (`ALLOC_NO_FAT_CHAIN`) the clusters are
/// one contiguous run and the FAT is not read.
const ALLOC_POSSIBLE: u8 = 0x01;
const ALLOC_NO_FAT_CHAIN: u8 = 0x02;
const EXFAT_TZ_VALID: u8 = 0x80;
const EXFAT_FILE_NAME_LEN: usize = 15;
const EXFAT_MAX_FILE_LEN: usize = 255;
const MAX_READ: usize = 1 << 20;
const FAT_CACHE: usize = 32;

const CS_DIR_ENTRY: u8 = 0;
const CS_BOOT_SECTOR: u8 = 1;
const CS_DEFAULT: u8 = 2;

/// `exfat_calc_chksum16`.
fn chksum16(data: &[u8], mut sum: u16, kind: u8) -> u16 {
    for (i, &c) in data.iter().enumerate() {
        if kind == CS_DIR_ENTRY && (i == 2 || i == 3) { continue; }
        sum = ((sum << 15) | (sum >> 1)).wrapping_add(c as u16);
    }
    sum
}

/// `exfat_calc_chksum32`.
fn chksum32(data: &[u8], mut sum: u32, kind: u8) -> u32 {
    for (i, &c) in data.iter().enumerate() {
        if kind == CS_BOOT_SECTOR && (i == 106 || i == 107 || i == 112) { continue; }
        sum = ((sum << 31) | (sum >> 1)).wrapping_add(c as u32);
    }
    sum
}

fn le16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn le32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn le64(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8])) }

/// A folder as a run of clusters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirRef {
    first: u32,
    contig: bool,
    /// Bytes, for a folder that has an entry; the root ends with its chain.
    size: u64,
    root: bool,
}

/// Where a node's entry set is: its folder, the index of the file entry and
/// the number of entries in the set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Loc {
    dir: DirRef,
    index: u32,
    count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub first: u32,
    pub is_dir: bool,
    pub size: u64,
    /// UTC seconds since the Unix epoch, 0 = unknown.
    pub mtime: u64,
    valid: u64,
    contig: bool,
    attr: u16,
    root: bool,
    /// Modification time as stored: time, date, centiseconds, offset.
    times: (u16, u16, u8, u8),
    loc: Option<Loc>,
}

impl Node {
    fn as_dir(&self) -> DirRef {
        DirRef { first: self.first, contig: self.contig, size: self.size, root: self.root }
    }
}

pub struct Entry {
    pub name: String,
    pub node: Node,
}

#[derive(Clone, Copy)]
struct Cursor {
    first: u32,
    idx: u64,
    clus: u32,
}

pub struct Exfat<D: Dev> {
    dev: D,
    bps: u64,
    sector_size: u32,
    cluster_size: u32,
    fat_start: u64,
    data_start: u64,
    /// Clusters plus the two reserved numbers (`num_clusters`).
    num_clusters: u32,
    root_dir: u32,
    vol_flags: u16,
    upcase: Vec<u16>,
    bitmap: Vec<u8>,
    bitmap_first: u32,
    dirty_bitmap: Vec<u64>,
    free_clusters: u32,
    /// Where the search for a free cluster starts (`clu_srch_ptr`).
    srch: u32,
    label: String,
    cache: Vec<(u64, Vec<u8>)>,
    dirty_fat: Vec<u64>,
    cursor: Option<Cursor>,
    marked: bool,
    now: u64,
    /// The file being appended to, its set not yet rewritten.
    open: Option<(String, Node)>,
}

fn unix_from(time: u16, date: u16, cs: u8, tz: u8) -> u64 {
    if date == 0 { return 0; }
    let year = 1980 + (date >> 9) as i64;
    let month = ((date >> 5) & 0xF).clamp(1, 12) as i64;
    let day = (date & 0x1F).max(1) as i64;
    let mut secs = ((time & 0x1F) as i64) * 2 + ((time >> 5) & 0x3F) as i64 * 60 + (time >> 11) as i64 * 3600
        + (cs / 100) as i64;
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    secs += (era * 146_097 + doe - 719_468) * 86_400;
    // `exfat_adjust_tz`: the offset in 15-minute steps, signed 7 bits.
    if tz & EXFAT_TZ_VALID != 0 {
        let off = tz & 0x7F;
        let q = if off <= 0x3F { off as i64 } else { off as i64 - 0x80 };
        secs -= q * 15 * 60;
    }
    secs.max(0) as u64
}

/// `exfat_set_entry_time` for UTC: time, date, centiseconds, offset.
fn to_entry_time(t: u64) -> (u16, u16, u8, u8) {
    let days = (t / 86_400) as i64;
    let secs = t % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (yoe + era * 400 + if month <= 2 { 1 } else { 0 }).clamp(1980, 2107);
    let s = secs % 60;
    let time = ((secs / 3600) << 11) | (((secs / 60) % 60) << 5) | (s / 2);
    let date = (((year - 1980) as u64) << 9) | ((month as u64) << 5) | day as u64;
    (time as u16, date as u16, ((s & 1) * 100) as u8, EXFAT_TZ_VALID)
}

/// `bad_uni_chars` and control characters; trailing dots and spaces are
/// refused rather than cut.
fn valid_name(u: &[u16]) -> Result<(), Error> {
    if u.is_empty() || u.len() > EXFAT_MAX_FILE_LEN { return Err(Error::BadName); }
    if u.iter().any(|&w| w < 0x20 || matches!(w, 0x22 | 0x2A | 0x2F | 0x3A | 0x3C | 0x3E | 0x3F | 0x5C | 0x7C)) {
        return Err(Error::BadName);
    }
    if matches!(u[u.len() - 1], 0x20 | 0x2E) { return Err(Error::BadName); }
    Ok(())
}

fn split(path: &str) -> (&str, &str) {
    let p = path.trim_matches('/');
    match p.rfind('/') {
        Some(k) => (&p[..k], &p[k + 1..]),
        None => ("", p),
    }
}

impl<D: Dev> Exfat<D> {
    pub fn mount(mut dev: D) -> Result<Self, Error> {
        let bs = dev.block_size();
        if bs < 512 || dev.blocks() == 0 { return Err(Error::NotFat); }
        let mut b = alloc::vec![0u8; bs as usize];
        dev.read(0, &mut b)?;
        // `exfat_read_boot_sector`.
        if le16(&b, 510) != BOOT_SIGNATURE || &b[3..11] != b"EXFAT   " { return Err(Error::NotFat); }
        if b[11..64].iter().any(|&x| x != 0) { return Err(Error::NotFat); }
        let num_fats = b[110];
        if num_fats != 1 && num_fats != 2 { return Err(Error::NotFat); }
        let sect_size_bits = b[108];
        if !(9..=12).contains(&sect_size_bits) { return Err(Error::NotFat); }
        let spc_bits = b[109];
        if spc_bits > 25 - sect_size_bits { return Err(Error::NotFat); }
        let sector_size = 1u32 << sect_size_bits;
        if sector_size < bs { return Err(Error::NotFat); }
        let bps = (sector_size / bs) as u64;
        let cluster_size = 1u32 << (spc_bits + sect_size_bits);
        let fat_length = le32(&b, 84) as u64;
        let fat_start = le32(&b, 80) as u64;
        let data_start = le32(&b, 88) as u64;
        let clu_count = le32(&b, 92);
        let num_clusters = clu_count.checked_add(2).ok_or(Error::NotFat)?;
        let root_dir = le32(&b, 96);
        let vol_flags = le16(&b, 106);
        if (fat_length << sect_size_bits) < num_clusters as u64 * 4 { return Err(Error::NotFat); }
        if data_start < fat_start + fat_length * num_fats as u64 { return Err(Error::NotFat); }
        if !(EXFAT_FIRST_CLUSTER..num_clusters).contains(&root_dir) { return Err(Error::NotFat); }

        let mut fs = Exfat {
            dev, bps, sector_size, cluster_size, fat_start, data_start, num_clusters, root_dir, vol_flags,
            upcase: Vec::new(), bitmap: Vec::new(), bitmap_first: 0, dirty_bitmap: Vec::new(),
            free_clusters: 0, srch: EXFAT_FIRST_CLUSTER, label: String::new(), cache: Vec::new(),
            dirty_fat: Vec::new(), cursor: None, marked: false, now: 0, open: None,
        };
        fs.verify_boot_region()?;
        fs.load_root_entries()?;
        Ok(fs)
    }

    /// `exfat_verify_boot_region`: the checksum of sectors 0..11 repeated
    /// through sector 11.
    fn verify_boot_region(&mut self) -> Result<(), Error> {
        let ss = self.sector_size as usize;
        let mut sum = 0u32;
        let mut b = alloc::vec![0u8; ss];
        for sn in 0..11u64 {
            self.read_sectors(sn, &mut b)?;
            if sn != 0 && sn <= 8 && le32(&b, ss - 4) != EXBOOT_SIGNATURE {
                // Linux only warns about a missing signature.
            }
            sum = chksum32(&b, sum, if sn == 0 { CS_BOOT_SECTOR } else { CS_DEFAULT });
        }
        self.read_sectors(11, &mut b)?;
        if b.chunks_exact(4).any(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) != sum) {
            return Err(Error::NotFat);
        }
        Ok(())
    }

    /// The bitmap, the up-case table and the label from the root
    /// (`exfat_load_bitmap`, `exfat_create_upcase_table`).
    fn load_root_entries(&mut self) -> Result<(), Error> {
        let root = self.root();
        let raw = self.dir_bytes(&root.as_dir())?;
        let mut bitmap = None;
        let mut upcase = None;
        for d in raw.chunks_exact(DENTRY) {
            match d[0] {
                EXFAT_UNUSED => break,
                // The first bitmap; a second one belongs to TexFAT.
                EXFAT_BITMAP if bitmap.is_none() && d[1] & 1 == 0 => bitmap = Some((le32(d, 20), le64(d, 24))),
                EXFAT_UPCASE if upcase.is_none() => upcase = Some((le32(d, 20), le64(d, 24), le32(d, 4))),
                EXFAT_VOLUME => {
                    let n = (d[1] as usize).min(11);
                    let u: Vec<u16> = (0..n).map(|k| le16(d, 2 + 2 * k)).collect();
                    self.label = char::decode_utf16(u).map(|r| r.unwrap_or('\u{FFFD}')).collect();
                }
                _ => {}
            }
        }
        let (bfirst, bsize) = bitmap.ok_or(Error::NotFat)?;
        let need = (self.num_clusters as u64 - 2).div_ceil(8);
        if bsize < need || bsize > need + self.cluster_size as u64 * 2 { return Err(Error::NotFat); }
        let bits = self.read_chain_bytes(bfirst, false, need)?;
        self.bitmap = bits;
        self.bitmap_first = bfirst;
        self.free_clusters = (0..self.num_clusters - 2).filter(|&i| !self.bit(i + 2)).count() as u32;

        self.upcase = alloc::vec![0u16; 0x10000];
        let mut ok = false;
        if let Some((ufirst, usize_, ucsum)) = upcase {
            if usize_ > 0 && usize_ <= 0x20000 {
                let bytes = self.read_chain_bytes(ufirst, false, usize_)?;
                ok = self.decode_upcase(&bytes, ucsum);
            }
        }
        if !ok {
            // `exfat_load_default_upcase_table`: ASCII and Latin-1 at least.
            for c in 0..0x10000u32 {
                let ch = char::from_u32(c);
                let up = ch.and_then(|ch| {
                    let mut u = ch.to_uppercase();
                    match (u.next(), u.next()) { (Some(x), None) if (x as u32) < 0x10000 => Some(x as u32 as u16), _ => None }
                });
                if let Some(u) = up { if u as u32 != c { self.upcase[c as usize] = u; } }
            }
        }
        Ok(())
    }

    /// `exfat_load_upcase_table`: runs of identity are compressed as
    /// 0xFFFF and a count.
    fn decode_upcase(&mut self, bytes: &[u8], csum: u32) -> bool {
        let mut index = 0u32;
        let mut skip = false;
        for c in bytes.chunks_exact(2) {
            if index > 0xFFFF { break; }
            let uni = u16::from_le_bytes([c[0], c[1]]);
            if skip {
                index += uni as u32;
                skip = false;
            } else if uni as u32 == index {
                index += 1;
            } else if uni == 0xFFFF {
                skip = true;
            } else {
                self.upcase[index as usize] = uni;
                index += 1;
            }
        }
        index >= 0xFFFF && chksum32(bytes, 0, CS_DEFAULT) == csum
    }

    fn up(&self, c: u16) -> u16 {
        match self.upcase[c as usize] { 0 => c, u => u }
    }

    fn same_name(&self, a: &[u16], b: &[u16]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| self.up(x) == self.up(y))
    }

    /// `exfat_calc_chksum16` over the up-cased name: the stream's hash.
    fn name_hash(&self, u: &[u16]) -> u16 {
        let mut bytes = Vec::with_capacity(u.len() * 2);
        for &c in u { bytes.extend_from_slice(&self.up(c).to_le_bytes()); }
        chksum16(&bytes, 0, CS_DEFAULT)
    }

    pub fn root(&self) -> Node {
        Node {
            first: self.root_dir, is_dir: true, size: 0, mtime: 0, valid: 0, contig: false,
            attr: ATTR_SUBDIR, root: true, times: (0, 0, 0, 0), loc: None,
        }
    }

    // === Sectors, the FAT and the bitmap ===

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

    fn write_sectors(&mut self, sector: u64, buf: &[u8]) -> Result<(), Error> {
        let lba = sector.checked_mul(self.bps).ok_or(Error::Corrupt)?;
        let bs = self.dev.block_size() as usize;
        let mut at = 0;
        while at < buf.len() {
            let n = (buf.len() - at).min(MAX_READ);
            self.dev.write(lba + (at / bs) as u64, &buf[at..at + n])?;
            at += n;
        }
        Ok(())
    }

    fn valid_cluster(&self, c: u32) -> bool {
        (EXFAT_FIRST_CLUSTER..self.num_clusters).contains(&c)
    }

    fn cluster_sector(&self, c: u32) -> u64 {
        self.data_start + (c - EXFAT_FIRST_CLUSTER) as u64 * (self.cluster_size / self.sector_size) as u64
    }

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
            self.read_sectors(self.fat_start + k, &mut b)?;
            self.cache.push((k, b));
        }
        Ok(&mut self.cache.last_mut().ok_or(Error::Io)?.1)
    }

    fn fat_raw(&mut self, c: u32) -> Result<u32, Error> {
        let ss = self.sector_size as u64;
        let off = c as u64 * 4;
        let sec = self.fat_sector(off / ss)?;
        let i = (off % ss) as usize;
        Ok(le32(sec, i))
    }

    /// `exfat_ent_get`: the next cluster, `None` at the end.
    fn next(&mut self, c: u32) -> Result<Option<u32>, Error> {
        if !self.valid_cluster(c) { return Err(Error::Corrupt); }
        let mut v = self.fat_raw(c)?;
        if v > EXFAT_BAD_CLUSTER { v = EXFAT_EOF_CLUSTER; }
        if v == EXFAT_EOF_CLUSTER { return Ok(None); }
        if v == 0 || v == EXFAT_BAD_CLUSTER || !self.valid_cluster(v) { return Err(Error::Corrupt); }
        Ok(Some(v))
    }

    fn fat_set(&mut self, c: u32, v: u32) -> Result<(), Error> {
        if !self.valid_cluster(c) { return Err(Error::Corrupt); }
        let ss = self.sector_size as u64;
        let off = c as u64 * 4;
        let k = off / ss;
        let sec = self.fat_sector(k)?;
        let i = (off % ss) as usize;
        sec[i..i + 4].copy_from_slice(&v.to_le_bytes());
        if !self.dirty_fat.contains(&k) { self.dirty_fat.push(k); }
        Ok(())
    }

    /// The FAT sectors changed, runs of neighbours in one write. Linux
    /// writes the first FAT only.
    fn fat_flush(&mut self) -> Result<(), Error> {
        let mut dirty = core::mem::take(&mut self.dirty_fat);
        dirty.sort_unstable();
        dirty.dedup();
        let ss = self.sector_size as usize;
        let mut i = 0;
        while i < dirty.len() {
            let mut j = i + 1;
            while j < dirty.len() && dirty[j] == dirty[j - 1] + 1 && (j - i + 1) * ss <= MAX_READ { j += 1; }
            let mut buf = alloc::vec![0u8; (j - i) * ss];
            for (n, k) in dirty[i..j].iter().enumerate() {
                let Some((_, b)) = self.cache.iter().find(|(s, _)| s == k) else { return Err(Error::Io) };
                buf[n * ss..(n + 1) * ss].copy_from_slice(b);
            }
            self.write_sectors(self.fat_start + dirty[i], &buf)?;
            i = j;
        }
        Ok(())
    }

    fn bit(&self, c: u32) -> bool {
        let i = (c - EXFAT_FIRST_CLUSTER) as usize;
        self.bitmap.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0)
    }

    fn set_bit(&mut self, c: u32, on: bool) {
        let i = (c - EXFAT_FIRST_CLUSTER) as usize;
        let Some(b) = self.bitmap.get_mut(i / 8) else { return };
        if on { *b |= 1 << (i % 8); } else { *b &= !(1 << (i % 8)); }
        let sec = (i / 8 / self.sector_size as usize) as u64;
        if !self.dirty_bitmap.contains(&sec) { self.dirty_bitmap.push(sec); }
    }

    /// The bitmap's changed sectors to its clusters (`exfat_set_bitmap`
    /// writes through its buffer heads).
    fn bitmap_flush(&mut self) -> Result<(), Error> {
        let mut dirty = core::mem::take(&mut self.dirty_bitmap);
        dirty.sort_unstable();
        dirty.dedup();
        let ss = self.sector_size as usize;
        let per = (self.cluster_size / self.sector_size) as u64;
        for k in dirty {
            let byte = k as usize * ss;
            let mut buf = alloc::vec![0u8; ss];
            let n = self.bitmap.len().saturating_sub(byte).min(ss);
            buf[..n].copy_from_slice(&self.bitmap[byte..byte + n]);
            let mut c = self.bitmap_first;
            for _ in 0..k / per { c = self.next(c)?.ok_or(Error::Corrupt)?; }
            let sector = self.cluster_sector(c) + k % per;
            // Bytes past the bitmap's end keep what they had.
            if n < ss {
                let mut old = alloc::vec![0u8; ss];
                self.read_sectors(sector, &mut old)?;
                buf[n..].copy_from_slice(&old[n..]);
            }
            self.write_sectors(sector, &buf)?;
        }
        Ok(())
    }

    /// `len` bytes of a chain (or a contiguous run) from `first`.
    fn read_chain_bytes(&mut self, first: u32, contig: bool, len: u64) -> Result<Vec<u8>, Error> {
        let cs = self.cluster_size as u64;
        let n = len.div_ceil(cs);
        let mut out = alloc::vec![0u8; (n * cs) as usize];
        let mut c = first;
        for k in 0..n {
            if !self.valid_cluster(c) { return Err(Error::Corrupt); }
            let s = self.cluster_sector(c);
            self.read_sectors(s, &mut out[(k * cs) as usize..((k + 1) * cs) as usize])?;
            if k + 1 < n {
                c = if contig { c + 1 } else { self.next(c)?.ok_or(Error::Corrupt)? };
            }
        }
        out.truncate(len as usize);
        Ok(out)
    }

    // === Folders (dir.c) ===

    /// A folder's raw entries: the root's whole chain, another folder up to
    /// its size.
    fn dir_bytes(&mut self, dir: &DirRef) -> Result<Vec<u8>, Error> {
        let cs = self.cluster_size as u64;
        let max = MAX_EXFAT_DENTRIES * DENTRY as u64;
        if !dir.root {
            if dir.size == 0 { return Ok(Vec::new()); }
            if dir.size > max || dir.size % cs != 0 { return Err(Error::Corrupt); }
            return self.read_chain_bytes(dir.first, dir.contig, dir.size);
        }
        let mut out = Vec::new();
        let mut c = dir.first;
        let mut steps = 0u32;
        loop {
            if !self.valid_cluster(c) { return Err(Error::Corrupt); }
            let at = out.len();
            out.resize(at + cs as usize, 0);
            let s = self.cluster_sector(c);
            self.read_sectors(s, &mut out[at..])?;
            steps += 1;
            if out.len() as u64 >= max || steps >= self.num_clusters { return Err(Error::Corrupt); }
            match self.next(c)? {
                Some(n) => c = n,
                None => return Ok(out),
            }
        }
    }

    /// The entry sets of a folder (`exfat_readdir`, `exfat_get_dentry_set`
    /// with its checks). A set whose checksum fails is left out.
    fn parse_dir(&mut self, dir: &DirRef) -> Result<Vec<(Vec<u16>, Node)>, Error> {
        let raw = self.dir_bytes(dir)?;
        let n = raw.len() / DENTRY;
        let de = |i: usize| &raw[i * DENTRY..(i + 1) * DENTRY];
        let mut out = Vec::new();
        let mut i = 0;
        while i < n {
            let d = de(i);
            if d[0] == EXFAT_UNUSED { break; }
            if d[0] != EXFAT_FILE { i += 1; continue; }
            let count = d[1] as usize;
            if count < 2 || i + count >= n {
                i += 1;
                continue;
            }
            let s = de(i + 1);
            if s[0] != EXFAT_STREAM { i += 1; continue; }
            let name_len = s[3] as usize;
            let names = name_len.div_ceil(EXFAT_FILE_NAME_LEN);
            if name_len == 0 || names + 1 > count || (0..names).any(|k| de(i + 2 + k)[0] != EXFAT_NAME) {
                i += 1;
                continue;
            }
            let mut sum = chksum16(d, 0, CS_DIR_ENTRY);
            for k in 1..=count { sum = chksum16(de(i + k), sum, CS_DEFAULT); }
            if sum != le16(d, 2) {
                i += count + 1;
                continue;
            }
            let mut name: Vec<u16> = Vec::with_capacity(name_len);
            for k in 0..names {
                let e = de(i + 2 + k);
                for j in 0..EXFAT_FILE_NAME_LEN {
                    if name.len() < name_len { name.push(le16(e, 2 + 2 * j)); }
                }
            }
            let attr = le16(d, 4);
            let is_dir = attr & ATTR_SUBDIR != 0;
            let flags = s[1];
            let times = (le16(d, 12), le16(d, 14), d[21], d[23]);
            out.push((name, Node {
                first: le32(s, 20),
                is_dir,
                size: le64(s, 24),
                mtime: unix_from(times.0, times.1, times.2, times.3),
                valid: le64(s, 8),
                contig: flags & ALLOC_NO_FAT_CHAIN != 0,
                attr,
                root: false,
                times,
                loc: Some(Loc { dir: *dir, index: i as u32, count: (count + 1) as u32 }),
            }));
            i += count + 1;
        }
        Ok(out)
    }

    pub fn read_dir(&mut self, node: &Node) -> Result<Vec<Entry>, Error> {
        if !node.is_dir { return Err(Error::WrongKind); }
        self.settle()?;
        Ok(self.parse_dir(&node.as_dir())?.into_iter().map(|(u, node)| Entry {
            name: char::decode_utf16(u).map(|r| r.unwrap_or('\u{FFFD}')).collect(),
            node,
        }).collect())
    }

    /// The node at `path`, names compared through the up-case table.
    pub fn lookup(&mut self, path: &str) -> Result<Option<Node>, Error> {
        if let Some((p, n)) = &self.open {
            if p == path.trim_matches('/') { return Ok(Some(*n)); }
        }
        let mut node = self.root();
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            if !node.is_dir { return Ok(None); }
            let want: Vec<u16> = seg.encode_utf16().collect();
            let found = self.parse_dir(&node.as_dir())?.into_iter().find(|(u, _)| self.same_name(u, &want));
            match found {
                Some((_, n)) => node = n,
                None => return Ok(None),
            }
        }
        Ok(Some(node))
    }

    /// Up to `out.len()` bytes from `off`; past the valid size the file
    /// reads as zero.
    pub fn read(&mut self, node: &Node, off: u64, out: &mut [u8]) -> Result<usize, Error> {
        if node.is_dir { return Err(Error::WrongKind); }
        if off >= node.size || out.is_empty() { return Ok(0); }
        let len = (out.len() as u64).min(node.size - off) as usize;
        let cs = self.cluster_size as u64;
        let mut idx = off / cs;
        let mut c = if node.contig {
            node.first.checked_add(idx as u32).ok_or(Error::Corrupt)?
        } else {
            let (mut k, mut c) = match self.cursor {
                Some(cur) if cur.first == node.first && cur.idx <= idx => (cur.idx, cur.clus),
                _ => (0, node.first),
            };
            while k < idx { c = self.next(c)?.ok_or(Error::Corrupt)?; k += 1; }
            c
        };
        let mut done = 0usize;
        let mut in_clus = (off % cs) as usize;
        while done < len {
            if !self.valid_cluster(c) { return Err(Error::Corrupt); }
            let want = len - done + in_clus;
            let mut run = 1u64;
            let mut last = c;
            while run * cs < want as u64 && run * cs < MAX_READ as u64 {
                let n = if node.contig { Some(last + 1) } else { self.next(last)? };
                match n {
                    Some(n) if n == last + 1 && self.valid_cluster(n) => { last = n; run += 1; }
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
            if !node.contig { self.cursor = Some(Cursor { first: node.first, idx: idx - 1, clus: last }); }
            if done < len {
                c = if node.contig { last + 1 } else { self.next(last)?.ok_or(Error::Corrupt)? };
            }
        }
        // `valid_size`: what lies past it was never written.
        let valid_end = node.valid.saturating_sub(off).min(len as u64) as usize;
        out[valid_end..len].fill(0);
        Ok(done)
    }

    // === Writing ===

    pub fn set_now(&mut self, unix: u64) { self.now = unix; }

    /// `exfat_set_volume_dirty` before the first change; a volume that came
    /// dirty is left as it is.
    fn mark(&mut self) -> Result<(), Error> {
        if self.marked || self.vol_flags & VOLUME_DIRTY != 0 { return Ok(()); }
        self.boot_flags(self.vol_flags | VOLUME_DIRTY)?;
        self.marked = true;
        Ok(())
    }

    /// The volume flags live in bytes the boot checksum skips.
    fn boot_flags(&mut self, flags: u16) -> Result<(), Error> {
        let mut b = alloc::vec![0u8; self.sector_size as usize];
        self.read_sectors(0, &mut b)?;
        b[106..108].copy_from_slice(&flags.to_le_bytes());
        self.write_sectors(0, &b)
    }

    /// `exfat_alloc_cluster` with a FAT chain: `n` free clusters after the
    /// last search, linked behind `prev` (0: a new chain).
    fn alloc(&mut self, n: usize, prev: u32) -> Result<Vec<u32>, Error> {
        if n == 0 { return Ok(Vec::new()); }
        if n as u64 > self.free_clusters as u64 { return Err(Error::NoSpace); }
        let mut out = Vec::with_capacity(n);
        let mut c = self.srch;
        let total = self.num_clusters - 2;
        let mut scanned = 0u32;
        while out.len() < n {
            if c >= self.num_clusters { c = EXFAT_FIRST_CLUSTER; }
            scanned += 1;
            if scanned > total {
                for &k in &out { self.set_bit(k, false); }
                return Err(Error::NoSpace);
            }
            if !self.bit(c) {
                self.set_bit(c, true);
                out.push(c);
            }
            c += 1;
        }
        for w in out.windows(2) { self.fat_set(w[0], w[1])?; }
        let last = *out.last().unwrap_or(&prev);
        self.fat_set(last, EXFAT_EOF_CLUSTER)?;
        if prev != 0 { self.fat_set(prev, out[0])?; }
        self.srch = last + 1;
        self.free_clusters -= n as u32;
        Ok(out)
    }

    /// `exfat_free_cluster`: the clusters back to the bitmap.
    fn free(&mut self, first: u32, contig: bool, size: u64) -> Result<(), Error> {
        if first == 0 { return Ok(()); }
        let cs = self.cluster_size as u64;
        if contig {
            let n = size.div_ceil(cs).max(1) as u32;
            for k in 0..n {
                let c = first.checked_add(k).ok_or(Error::Corrupt)?;
                if !self.valid_cluster(c) { return Err(Error::Corrupt); }
                if self.bit(c) { self.set_bit(c, false); self.free_clusters += 1; }
            }
            return Ok(());
        }
        let mut c = first;
        let mut steps = 0u32;
        loop {
            if !self.valid_cluster(c) { return Err(Error::Corrupt); }
            let next = self.next(c)?;
            if self.bit(c) { self.set_bit(c, false); self.free_clusters += 1; }
            steps += 1;
            match next {
                Some(n) if steps < self.num_clusters => c = n,
                Some(_) => return Err(Error::Corrupt),
                None => return Ok(()),
            }
        }
    }

    fn entry_sector(&mut self, dir: &DirRef, index: u32) -> Result<(u64, usize), Error> {
        let byte = index as u64 * DENTRY as u64;
        let cs = self.cluster_size as u64;
        let ss = self.sector_size as u64;
        let k = byte / cs;
        let c = if dir.contig {
            dir.first.checked_add(k as u32).ok_or(Error::Corrupt)?
        } else {
            let mut c = dir.first;
            for _ in 0..k { c = self.next(c)?.ok_or(Error::Corrupt)?; }
            c
        };
        if !self.valid_cluster(c) { return Err(Error::Corrupt); }
        Ok((self.cluster_sector(c) + (byte % cs) / ss, (byte % ss) as usize))
    }

    fn with_entry(&mut self, dir: &DirRef, index: u32, f: impl FnOnce(&mut [u8])) -> Result<(), Error> {
        let (sector, off) = self.entry_sector(dir, index)?;
        let mut b = alloc::vec![0u8; self.sector_size as usize];
        self.read_sectors(sector, &mut b)?;
        f(&mut b[off..off + DENTRY]);
        self.write_sectors(sector, &b)
    }

    /// Mark a set deleted: the in-use bit of every entry cleared.
    fn delete_set(&mut self, loc: Loc) -> Result<(), Error> {
        for k in 0..loc.count {
            self.with_entry(&loc.dir, loc.index + k, |d| d[0] &= 0x7F)?;
        }
        Ok(())
    }

    /// A folder written without a FAT chain gets one before it grows
    /// (`exfat_chain_cont_cluster`), and its stream says so.
    fn chain_dir(&mut self, dir: &mut Node) -> Result<(), Error> {
        if !dir.contig { return Ok(()); }
        let n = dir.size.div_ceil(self.cluster_size as u64) as u32;
        for k in 0..n {
            let c = dir.first + k;
            self.fat_set(c, if k + 1 == n { EXFAT_EOF_CLUSTER } else { c + 1 })?;
        }
        dir.contig = false;
        if let Some(loc) = dir.loc {
            self.rewrite_set(loc, |set| set[1][1] &= !ALLOC_NO_FAT_CHAIN)?;
        }
        Ok(())
    }

    /// Read a whole set, change it, put its checksum back and write it.
    fn rewrite_set(&mut self, loc: Loc, f: impl FnOnce(&mut Vec<[u8; DENTRY]>)) -> Result<(), Error> {
        let mut set: Vec<[u8; DENTRY]> = Vec::with_capacity(loc.count as usize);
        for k in 0..loc.count {
            let (sector, off) = self.entry_sector(&loc.dir, loc.index + k)?;
            let mut b = alloc::vec![0u8; self.sector_size as usize];
            self.read_sectors(sector, &mut b)?;
            let mut e = [0u8; DENTRY];
            e.copy_from_slice(&b[off..off + DENTRY]);
            set.push(e);
        }
        f(&mut set);
        let sum = set_checksum(&set);
        set[0][2..4].copy_from_slice(&sum.to_le_bytes());
        for (k, e) in set.iter().enumerate() {
            self.with_entry(&loc.dir, loc.index + k as u32, |d| d.copy_from_slice(e))?;
        }
        Ok(())
    }

    /// Room for `need` entries in a row; the folder grows by a zeroed
    /// cluster when it has none, and its size in its parent follows.
    fn find_slots(&mut self, dir: &mut Node, need: usize) -> Result<u32, Error> {
        let raw = self.dir_bytes(&dir.as_dir())?;
        let n = raw.len() / DENTRY;
        let mut run = 0usize;
        for i in 0..n {
            let t = raw[i * DENTRY];
            // Unused (the end) or deleted (in-use bit clear).
            if t == EXFAT_UNUSED || t < 0x80 {
                run += 1;
                if run == need { return Ok((i + 1 - need) as u32); }
            } else {
                run = 0;
            }
        }
        let per = self.cluster_size as usize / DENTRY;
        let add = (need - run).div_ceil(per);
        if (n + add * per) as u64 > MAX_EXFAT_DENTRIES { return Err(Error::NoSpace); }
        self.chain_dir(dir)?;
        let mut last = dir.first;
        let mut steps = 0u32;
        while let Some(c) = self.next(last)? {
            last = c;
            steps += 1;
            if steps >= self.num_clusters { return Err(Error::Corrupt); }
        }
        let new = self.alloc(add, last)?;
        let zero = alloc::vec![0u8; self.cluster_size as usize];
        for c in new {
            let s = self.cluster_sector(c);
            self.write_sectors(s, &zero)?;
        }
        if !dir.root {
            dir.size += (add * self.cluster_size as usize) as u64;
            dir.valid = dir.size;
            let size = dir.size;
            if let Some(loc) = dir.loc {
                self.rewrite_set(loc, |set| {
                    set[1][8..16].copy_from_slice(&size.to_le_bytes());
                    set[1][24..32].copy_from_slice(&size.to_le_bytes());
                })?;
            }
        }
        Ok((n - run) as u32)
    }

    /// `exfat_add_entry`: a file entry, its stream and its name entries.
    fn add_set(&mut self, dir: &mut Node, name: &str, attr: u16, first: u32, size: u64,
               times: (u16, u16, u8, u8), skip: Option<Loc>) -> Result<Loc, Error> {
        let u: Vec<u16> = name.encode_utf16().collect();
        valid_name(&u)?;
        for (other, n) in self.parse_dir(&dir.as_dir())? {
            if skip.is_some() && n.loc == skip { continue; }
            if self.same_name(&other, &u) { return Err(Error::Exists); }
        }
        let names = u.len().div_ceil(EXFAT_FILE_NAME_LEN);
        let count = 2 + names;
        let index = self.find_slots(dir, count)?;
        let mut set: Vec<[u8; DENTRY]> = alloc::vec![[0u8; DENTRY]; count];
        let now = to_entry_time(self.now);
        set[0][0] = EXFAT_FILE;
        set[0][1] = (count - 1) as u8;
        set[0][4..6].copy_from_slice(&attr.to_le_bytes());
        set[0][8..10].copy_from_slice(&now.0.to_le_bytes());
        set[0][10..12].copy_from_slice(&now.1.to_le_bytes());
        set[0][12..14].copy_from_slice(&times.0.to_le_bytes());
        set[0][14..16].copy_from_slice(&times.1.to_le_bytes());
        set[0][16..18].copy_from_slice(&now.0.to_le_bytes());
        set[0][18..20].copy_from_slice(&now.1.to_le_bytes());
        set[0][20] = now.2;
        set[0][21] = times.2;
        set[0][22] = now.3;
        set[0][23] = times.3;
        set[0][24] = now.3;
        set[1][0] = EXFAT_STREAM;
        set[1][1] = ALLOC_POSSIBLE;
        set[1][3] = u.len() as u8;
        set[1][4..6].copy_from_slice(&self.name_hash(&u).to_le_bytes());
        set[1][8..16].copy_from_slice(&size.to_le_bytes());
        set[1][20..24].copy_from_slice(&first.to_le_bytes());
        set[1][24..32].copy_from_slice(&size.to_le_bytes());
        for k in 0..names {
            set[2 + k][0] = EXFAT_NAME;
            for j in 0..EXFAT_FILE_NAME_LEN {
                let c = u.get(k * EXFAT_FILE_NAME_LEN + j).copied().unwrap_or(0);
                set[2 + k][2 + 2 * j..4 + 2 * j].copy_from_slice(&c.to_le_bytes());
            }
        }
        let sum = set_checksum(&set);
        set[0][2..4].copy_from_slice(&sum.to_le_bytes());
        let dref = dir.as_dir();
        for (k, e) in set.iter().enumerate() {
            self.with_entry(&dref, index + k as u32, |d| d.copy_from_slice(e))?;
        }
        Ok(Loc { dir: dref, index, count: count as u32 })
    }

    fn parent(&mut self, path: &str) -> Result<(Node, String), Error> {
        let (dir, name) = split(path);
        if name.is_empty() { return Err(Error::BadName); }
        let node = self.lookup(dir)?.ok_or(Error::NotFound)?;
        if !node.is_dir { return Err(Error::WrongKind); }
        Ok((node, String::from(name)))
    }

    /// The file being appended to gets its set rewritten.
    pub fn settle(&mut self) -> Result<(), Error> {
        let Some((_, n)) = self.open.take() else { return Ok(()) };
        let loc = n.loc.ok_or(Error::Corrupt)?;
        let t = to_entry_time(self.now);
        self.rewrite_set(loc, |set| {
            set[0][12..14].copy_from_slice(&t.0.to_le_bytes());
            set[0][14..16].copy_from_slice(&t.1.to_le_bytes());
            set[0][21] = t.2;
            set[0][23] = t.3;
            set[1][1] = ALLOC_POSSIBLE;
            set[1][8..16].copy_from_slice(&n.size.to_le_bytes());
            set[1][20..24].copy_from_slice(&n.first.to_le_bytes());
            set[1][24..32].copy_from_slice(&n.size.to_le_bytes());
        })?;
        self.fat_flush()?;
        self.bitmap_flush()
    }

    pub fn sync(&mut self) -> Result<(), Error> {
        self.settle()?;
        self.fat_flush()?;
        self.bitmap_flush()?;
        if self.marked {
            self.boot_flags(self.vol_flags & !VOLUME_DIRTY)?;
            self.marked = false;
        }
        self.dev.flush()?;
        Ok(())
    }

    pub fn create(&mut self, path: &str) -> Result<(), Error> {
        self.cursor = None;
        self.settle()?;
        self.mark()?;
        match self.lookup(path)? {
            Some(n) if n.is_dir => return Err(Error::WrongKind),
            Some(n) => {
                let loc = n.loc.ok_or(Error::Corrupt)?;
                self.free(n.first, n.contig, n.size)?;
                let t = to_entry_time(self.now);
                self.rewrite_set(loc, |set| {
                    set[0][12..14].copy_from_slice(&t.0.to_le_bytes());
                    set[0][14..16].copy_from_slice(&t.1.to_le_bytes());
                    set[1][1] = ALLOC_POSSIBLE;
                    set[1][8..16].fill(0);
                    set[1][20..24].fill(0);
                    set[1][24..32].fill(0);
                })?;
            }
            None => {
                let (mut dir, name) = self.parent(path)?;
                let t = to_entry_time(self.now);
                self.add_set(&mut dir, &name, ATTR_ARCHIVE, 0, 0, t, None)?;
            }
        }
        self.fat_flush()?;
        self.bitmap_flush()
    }

    pub fn append(&mut self, path: &str, off: u64, data: &[u8]) -> Result<(), Error> {
        let key = path.trim_matches('/');
        if self.open.as_ref().is_some_and(|(p, _)| p != key) { self.settle()?; }
        let n = self.lookup(path)?.ok_or(Error::NotFound)?;
        if n.is_dir { return Err(Error::WrongKind); }
        if off != n.size { return Err(Error::Corrupt); }
        if data.is_empty() { return Ok(()); }
        self.mark()?;
        // A file without a chain gets one before it grows.
        let mut n = n;
        if n.contig && n.first != 0 {
            let cnt = n.size.div_ceil(self.cluster_size as u64) as u32;
            for k in 0..cnt {
                let c = n.first + k;
                self.fat_set(c, if k + 1 == cnt { EXFAT_EOF_CLUSTER } else { c + 1 })?;
            }
            n.contig = false;
        }
        let cs = self.cluster_size as u64;
        let ss = self.sector_size as u64;
        let new_size = n.size + data.len() as u64;
        let have = if n.first == 0 { 0 } else { n.size.div_ceil(cs).max(1) };
        let need = new_size.div_ceil(cs);
        let mut last = 0u32;
        if have > 0 {
            let (mut idx, mut c) = match self.cursor {
                Some(k) if k.first == n.first && k.idx < have => (k.idx, k.clus),
                _ => (0, n.first),
            };
            while idx + 1 < have { c = self.next(c)?.ok_or(Error::Corrupt)?; idx += 1; }
            last = c;
        }
        let new = self.alloc((need - have) as usize, last)?;
        let first = if n.first == 0 { new[0] } else { n.first };
        let mut clusters: Vec<u32> = Vec::new();
        let mut in_clus = 0;
        if have > 0 && have * cs > n.size {
            clusters.push(last);
            in_clus = n.size - (have - 1) * cs;
        }
        clusters.extend_from_slice(&new);
        let mut done = 0usize;
        let mut i = 0usize;
        while done < data.len() {
            let start = clusters[i];
            let mut run = 1usize;
            while i + run < clusters.len() && clusters[i + run] == start + run as u32
                && (run as u64 + 1) * cs <= MAX_READ as u64
            {
                run += 1;
            }
            let room = run as u64 * cs - in_clus;
            let nb = room.min((data.len() - done) as u64) as usize;
            let first_sec = in_clus / ss;
            let end = (in_clus + nb as u64).div_ceil(ss);
            let mut buf = alloc::vec![0u8; ((end - first_sec) * ss) as usize];
            let head = (in_clus % ss) as usize;
            let base = self.cluster_sector(start) + first_sec;
            if head != 0 { self.read_sectors(base, &mut buf[..ss as usize])?; }
            buf[head..head + nb].copy_from_slice(&data[done..done + nb]);
            self.write_sectors(base, &buf)?;
            done += nb;
            i += run;
            in_clus = 0;
        }
        let tail = *clusters.last().unwrap_or(&last);
        self.cursor = Some(Cursor { first, idx: need - 1, clus: tail });
        self.open = Some((String::from(key), Node { first, size: new_size, valid: new_size, contig: false, ..n }));
        Ok(())
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), Error> {
        self.cursor = None;
        self.settle()?;
        if self.lookup(path)?.is_some() { return Err(Error::Exists); }
        let (mut dir, name) = self.parent(path)?;
        self.mark()?;
        let c = self.alloc(1, 0)?[0];
        let zero = alloc::vec![0u8; self.cluster_size as usize];
        let s = self.cluster_sector(c);
        self.write_sectors(s, &zero)?;
        let t = to_entry_time(self.now);
        let size = self.cluster_size as u64;
        if let Err(e) = self.add_set(&mut dir, &name, ATTR_SUBDIR, c, size, t, None) {
            self.free(c, false, size)?;
            self.fat_flush()?;
            self.bitmap_flush()?;
            return Err(e);
        }
        self.fat_flush()?;
        self.bitmap_flush()
    }

    pub fn remove(&mut self, path: &str) -> Result<(), Error> {
        self.cursor = None;
        self.settle()?;
        let n = self.lookup(path)?.ok_or(Error::NotFound)?;
        let loc = n.loc.ok_or(Error::WrongKind)?;
        if n.is_dir && !self.parse_dir(&n.as_dir())?.is_empty() { return Err(Error::NotEmpty); }
        self.mark()?;
        self.free(n.first, n.contig, n.size)?;
        self.delete_set(loc)?;
        self.fat_flush()?;
        self.bitmap_flush()
    }

    pub fn rename(&mut self, from: &str, to: &str, replace: bool) -> Result<(), Error> {
        self.cursor = None;
        self.settle()?;
        let src = self.lookup(from)?.ok_or(Error::NotFound)?;
        let src_loc = src.loc.ok_or(Error::WrongKind)?;
        let (f, t) = (from.trim_matches('/'), to.trim_matches('/'));
        if src.is_dir && t.len() > f.len() && t.starts_with(f) && t.as_bytes()[f.len()] == b'/' {
            return Err(Error::BadName);
        }
        let (mut dir, name) = self.parent(to)?;
        self.mark()?;
        let mut skip = None;
        if let Some(dst) = self.lookup(to)? {
            if dst.loc == Some(src_loc) {
                skip = Some(src_loc);
            } else if !replace || dst.is_dir || src.is_dir {
                return Err(Error::Exists);
            } else {
                self.free(dst.first, dst.contig, dst.size)?;
                self.delete_set(dst.loc.ok_or(Error::Corrupt)?)?;
            }
        }
        let loc = self.add_set(&mut dir, &name, src.attr, src.first, src.size, src.times, skip)?;
        // The stream as it was: valid size and chain flag.
        let (valid, contig) = (src.valid, src.contig);
        self.rewrite_set(loc, |set| {
            set[1][1] = ALLOC_POSSIBLE | if contig { ALLOC_NO_FAT_CHAIN } else { 0 };
            set[1][8..16].copy_from_slice(&valid.to_le_bytes());
        })?;
        // The old set moves only after the new one is written; the entry
        // indexes of a folder do not change when it grows at its end.
        self.delete_set(src_loc)?;
        self.fat_flush()?;
        self.bitmap_flush()
    }

    pub fn label(&self) -> &str { &self.label }
    pub fn dirty(&self) -> bool { self.vol_flags & VOLUME_DIRTY != 0 }
    pub fn total_bytes(&self) -> u64 { (self.num_clusters - 2) as u64 * self.cluster_size as u64 }
    pub fn free_bytes(&self) -> u64 { self.free_clusters as u64 * self.cluster_size as u64 }
    pub fn into_dev(self) -> D { self.dev }
}

/// The checksum over a whole set, its own field skipped.
fn set_checksum(set: &[[u8; DENTRY]]) -> u16 {
    let mut sum = chksum16(&set[0], 0, CS_DIR_ENTRY);
    for e in &set[1..] { sum = chksum16(e, sum, CS_DEFAULT); }
    sum
}

impl<D: Dev> Volume for Exfat<D> {
    type Node = Node;
    fn label(&self) -> &str { &self.label }
    fn dirty(&self) -> bool { Exfat::dirty(self) }
    fn total_bytes(&self) -> u64 { Exfat::total_bytes(self) }
    fn free_bytes(&self) -> u64 { Exfat::free_bytes(self) }
    fn info(n: &Node) -> (bool, u64, u64) { (n.is_dir, n.size, n.mtime) }
    fn lookup(&mut self, path: &str) -> Result<Option<Node>, Error> { Exfat::lookup(self, path) }
    fn list(&mut self, n: &Node) -> Result<Vec<(String, Node)>, Error> {
        Ok(self.read_dir(n)?.into_iter().map(|e| (e.name, e.node)).collect())
    }
    fn read(&mut self, n: &Node, off: u64, out: &mut [u8]) -> Result<usize, Error> { Exfat::read(self, n, off, out) }
    fn set_now(&mut self, t: u64) { Exfat::set_now(self, t) }
    fn create(&mut self, p: &str) -> Result<(), Error> { Exfat::create(self, p) }
    fn append(&mut self, p: &str, off: u64, d: &[u8]) -> Result<(), Error> { Exfat::append(self, p, off, d) }
    fn mkdir(&mut self, p: &str) -> Result<(), Error> { Exfat::mkdir(self, p) }
    fn remove(&mut self, p: &str) -> Result<(), Error> { Exfat::remove(self, p) }
    fn rename(&mut self, a: &str, b: &str, r: bool) -> Result<(), Error> { Exfat::rename(self, a, b, r) }
    fn sync(&mut self) -> Result<(), Error> { Exfat::sync(self) }
}
