//! The write side of Linux fs/fat: FAT entries (fatent.c `fat_ent_write`,
//! `fat_alloc_clusters`, `fat_free_clusters`), directory entries (dir.c
//! `fat_add_entries`, `fat_remove_entries`, `fat_alloc_new_dir`), names
//! (namei_vfat.c `vfat_create_shortname`, `vfat_build_slots`) and the
//! state kept on the medium (`fat_set_state`, `fat_clusters_flush`).
//!
//! Every operation writes its FAT sectors through to all FAT copies before
//! it returns; `sync` then writes FSINFO, clears the dirty bit and flushes
//! the write cache, so a volume at rest is consistent.

use alloc::vec::Vec;

use super::{
    fat_checksum, le32, same_name, Error, Fat, Loc, Node, ATTR_DIR, ATTR_EXT, DELETED_FLAG,
    FAT_MAX_DIR_ENTRIES, FAT_START_ENT, MAX_READ, MSDOS_DPS_ENTRY,
};
use crate::{cp437, Dev};

const ATTR_ARCH: u8 = 0x20;
const FAT_STATE_DIRTY: u8 = 1;
const FSINFO_SIG1: u32 = 0x4161_5252;
const FSINFO_SIG2: u32 = 0x6141_7272;
/// `FAT_LFN_LEN`.
const FAT_LFN_LEN: usize = 255;
/// Largest file FAT can describe.
const FAT_MAX_SIZE: u64 = 0xFFFF_FFFF;

/// `struct shortname_info`.
#[derive(Clone, Copy)]
struct ShortInfo {
    lower: bool,
    upper: bool,
    valid: bool,
}

const INIT_INFO: ShortInfo = ShortInfo { lower: true, upper: true, valid: true };

fn vfat_bad_char(w: u16) -> bool {
    w < 0x20 || matches!(w, 0x2A | 0x3F | 0x3C | 0x3E | 0x7C | 0x22 | 0x3A | 0x2F | 0x5C)
}

fn vfat_replace_char(w: u16) -> bool {
    matches!(w, 0x5B | 0x5D | 0x3B | 0x2C | 0x2B | 0x3D)
}

fn vfat_skip_char(w: u16) -> bool {
    w == 0x2E || w == 0x20
}

/// `uni2char` of codepage 437.
fn cp437_byte(w: u16) -> Option<u8> {
    if w < 0x80 { return Some(w as u8); }
    cp437::HIGH.iter().position(|&u| u == w).map(|i| 0x80 + i as u8)
}

/// `nls_toupper` of codepage 437.
fn cp437_upper(b: u8) -> u8 {
    if b < 0x80 { return b.to_ascii_uppercase(); }
    let c = cp437::to_char(b);
    let mut up = c.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) if (u as u32) < 0x10000 => cp437_byte(u as u32 as u16).unwrap_or(b),
        _ => b,
    }
}

/// `to_shortname_char`: the code page byte for one UTF-16 unit, `None`
/// for a unit the short name skips.
fn to_shortname_char(w: u16, info: &mut ShortInfo) -> Option<u8> {
    if vfat_skip_char(w) {
        info.valid = false;
        return None;
    }
    if vfat_replace_char(w) {
        info.valid = false;
        return Some(b'_');
    }
    let Some(prev) = cp437_byte(w) else {
        info.valid = false;
        return Some(b'_');
    };
    if prev >= 0x7F {
        info.lower = false;
        info.upper = false;
    }
    let b = cp437_upper(prev);
    if b.is_ascii_alphabetic() {
        if b == prev { info.lower = false; } else { info.upper = false; }
    }
    Some(b)
}

/// `vfat_valid_longname` plus `vfat_is_used_badchars`. Trailing dots are
/// refused rather than cut, so a name never changes on the way in.
fn valid_longname(u: &[u16]) -> Result<(), Error> {
    if u.is_empty() || u.len() > FAT_LFN_LEN { return Err(Error::BadName); }
    if u.iter().any(|&w| vfat_bad_char(w)) { return Err(Error::BadName); }
    let last = u[u.len() - 1];
    if last == 0x20 || last == 0x2E { return Err(Error::BadName); }
    if u == [0x2E] || u == [0x2E, 0x2E] { return Err(Error::BadName); }
    Ok(())
}

/// `fat_time_unix2fat` with the time zone taken as UTC: (time, date).
fn unix2fat(t: u64) -> (u16, u16) {
    let days = (t / 86_400) as i64;
    let secs = t % 86_400;
    // `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    if year < 1980 { return (0, (1 << 5) | 1); }
    if year > 2107 { return ((23 << 11) | (59 << 5) | 29, (127 << 9) | (12 << 5) | 31); }
    let time = ((secs / 3600) << 11) | (((secs / 60) % 60) << 5) | ((secs % 60) / 2);
    let date = (((year - 1980) as u64) << 9) | ((month as u64) << 5) | day as u64;
    (time as u16, date as u16)
}

fn split(path: &str) -> (&str, &str) {
    let p = path.trim_matches('/');
    match p.rfind('/') {
        Some(k) => (&p[..k], &p[k + 1..]),
        None => ("", p),
    }
}

impl<D: Dev> Fat<D> {
    /// The time new and changed entries carry.
    pub fn set_now(&mut self, unix: u64) {
        self.now = unix;
    }

    fn eof(&self) -> u32 {
        match self.fat_bits { 12 => 0xFFF, 16 => 0xFFFF, _ => 0x0FFF_FFFF }
    }

    // === Sectors ===

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

    /// FAT sector `k` from the cache to every FAT (`fat_mirror_bhs`), or to
    /// the active one when mirroring is off.
    pub(super) fn write_fat_sector(&mut self, k: u64) -> Result<(), Error> {
        let Some(buf) = self.cache.iter().find(|(s, _)| *s == k).map(|(_, b)| b.clone()) else {
            return Err(Error::Io);
        };
        for f in 0..self.fats {
            if !self.mirror && f != self.active_fat { continue; }
            self.write_sectors(self.fat_start + f * self.fat_length + k, &buf)?;
        }
        self.dirty_fat.retain(|&d| d != k);
        Ok(())
    }

    fn fat_flush(&mut self) -> Result<(), Error> {
        while let Some(&k) = self.dirty_fat.first() {
            self.write_fat_sector(k)?;
        }
        Ok(())
    }

    fn set_fat_byte(&mut self, off: u64, f: impl FnOnce(u8) -> u8) -> Result<(), Error> {
        let ss = self.sector_size as u64;
        let k = off / ss;
        let sec = self.fat_sector(k)?;
        let i = (off % ss) as usize;
        sec[i] = f(sec[i]);
        if !self.dirty_fat.contains(&k) { self.dirty_fat.push(k); }
        Ok(())
    }

    /// `fat_ent_write`.
    fn fat_set(&mut self, entry: u32, value: u32) -> Result<(), Error> {
        if entry < FAT_START_ENT || entry >= self.max_cluster { return Err(Error::Corrupt); }
        let e = entry as u64;
        match self.fat_bits {
            12 => {
                let off = e + e / 2;
                if entry & 1 != 0 {
                    self.set_fat_byte(off, |b| (b & 0x0F) | ((value << 4) as u8 & 0xF0))?;
                    self.set_fat_byte(off + 1, |_| (value >> 4) as u8)?;
                } else {
                    self.set_fat_byte(off, |_| value as u8)?;
                    self.set_fat_byte(off + 1, |b| (b & 0xF0) | ((value >> 8) as u8 & 0x0F))?;
                }
            }
            16 => {
                self.set_fat_byte(e * 2, |_| value as u8)?;
                self.set_fat_byte(e * 2 + 1, |_| (value >> 8) as u8)?;
            }
            _ => {
                // The top four bits are reserved and kept.
                let v = value & 0x0FFF_FFFF;
                for k in 0..4u64 {
                    let byte = (v >> (8 * k)) as u8;
                    if k == 3 {
                        self.set_fat_byte(e * 4 + 3, |b| (b & 0xF0) | (byte & 0x0F))?;
                    } else {
                        self.set_fat_byte(e * 4 + k, |_| byte)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `fat_alloc_clusters`: `n` free clusters from the last allocation on,
    /// chained, the last one ending the chain, the first linked after
    /// `prev` (0: a new chain).
    fn alloc_clusters(&mut self, n: usize, prev: u32) -> Result<Vec<u32>, Error> {
        if n == 0 { return Ok(Vec::new()); }
        if n as u64 > self.free_clusters as u64 { return Err(Error::NoSpace); }
        let total = self.max_cluster - FAT_START_ENT;
        let eof = self.eof();
        let mut out = Vec::with_capacity(n);
        let mut c = self.prev_free;
        let mut scanned = 0u32;
        while out.len() < n {
            c += 1;
            if c >= self.max_cluster { c = FAT_START_ENT; }
            scanned += 1;
            if scanned > total {
                for &k in &out { self.fat_set(k, 0)?; }
                return Err(Error::NoSpace);
            }
            if self.fat_ent(c)? == 0 {
                self.fat_set(c, eof)?;
                out.push(c);
            }
        }
        for w in out.windows(2) { self.fat_set(w[0], w[1])?; }
        if prev != 0 { self.fat_set(prev, out[0])?; }
        self.prev_free = *out.last().unwrap_or(&self.prev_free);
        self.free_clusters -= n as u32;
        Ok(out)
    }

    /// `fat_free_clusters`: the chain from `first` back to free.
    fn free_chain(&mut self, first: u32) -> Result<(), Error> {
        let mut c = first;
        let mut steps = 0u32;
        loop {
            if c < FAT_START_ENT || c >= self.max_cluster { return Err(Error::Corrupt); }
            let next = self.next(c)?;
            self.fat_set(c, 0)?;
            self.free_clusters += 1;
            steps += 1;
            match next {
                Some(n) if steps < self.max_cluster => c = n,
                Some(_) => return Err(Error::Corrupt),
                None => return Ok(()),
            }
        }
    }

    // === State on the medium ===

    fn boot_state(&mut self, dirty: bool) -> Result<(), Error> {
        let mut b = alloc::vec![0u8; self.sector_size as usize];
        self.read_sectors(0, &mut b)?;
        if dirty { b[self.state_off] |= FAT_STATE_DIRTY; } else { b[self.state_off] &= !FAT_STATE_DIRTY; }
        self.write_sectors(0, &b)
    }

    /// `fat_set_state(sb, 1, 0)` before the first change: a volume pulled
    /// mid-write is known as such. One that came dirty is left as it is.
    fn mark(&mut self) -> Result<(), Error> {
        if self.marked || self.dirty { return Ok(()); }
        self.boot_state(true)?;
        self.marked = true;
        Ok(())
    }

    /// The end of an operation: FAT through, FSINFO (`fat_clusters_flush`),
    /// the dirty bit cleared, the device's cache flushed.
    pub fn sync(&mut self) -> Result<(), Error> {
        self.fat_flush()?;
        if self.fat_bits == 32 {
            let mut b = alloc::vec![0u8; self.sector_size as usize];
            self.read_sectors(self.fsinfo_sector, &mut b)?;
            if le32(&b, 0) == FSINFO_SIG1 && le32(&b, 484) == FSINFO_SIG2 {
                b[488..492].copy_from_slice(&self.free_clusters.to_le_bytes());
                b[492..496].copy_from_slice(&self.prev_free.to_le_bytes());
                self.write_sectors(self.fsinfo_sector, &b)?;
            }
        }
        if self.marked {
            self.boot_state(false)?;
            self.marked = false;
        }
        self.dev.flush()?;
        Ok(())
    }

    // === Directory entries ===

    /// The sector holding entry `index` of a folder, and the entry's offset.
    fn entry_sector(&mut self, dir_first: u32, dir_root: bool, index: u32) -> Result<(u64, usize), Error> {
        let byte = index as u64 * MSDOS_DPS_ENTRY as u64;
        let ss = self.sector_size as u64;
        if dir_root && self.fat_bits != 32 {
            if index >= self.dir_entries { return Err(Error::Corrupt); }
            return Ok((self.dir_start + byte / ss, (byte % ss) as usize));
        }
        let cs = self.cluster_size as u64;
        let mut c = dir_first;
        for _ in 0..byte / cs { c = self.next(c)?.ok_or(Error::Corrupt)?; }
        if c < FAT_START_ENT || c >= self.max_cluster { return Err(Error::Corrupt); }
        Ok((self.cluster_sector(c) + (byte % cs) / ss, (byte % ss) as usize))
    }

    fn with_entry(&mut self, dir_first: u32, dir_root: bool, index: u32, f: impl FnOnce(&mut [u8])) -> Result<(), Error> {
        let (sector, off) = self.entry_sector(dir_first, dir_root, index)?;
        let mut b = alloc::vec![0u8; self.sector_size as usize];
        self.read_sectors(sector, &mut b)?;
        f(&mut b[off..off + MSDOS_DPS_ENTRY]);
        self.write_sectors(sector, &b)
    }

    /// `fat_remove_entries`: the long-name slots and the 8.3 entry deleted.
    fn remove_entries(&mut self, loc: Loc) -> Result<(), Error> {
        for k in loc.first_slot..=loc.index {
            self.with_entry(loc.dir_first, loc.dir_root, k, |d| d[0] = DELETED_FLAG)?;
        }
        Ok(())
    }

    /// Start cluster, size and modification time of an 8.3 entry.
    fn update_entry(&mut self, loc: Loc, first: u32, size: u64, time: (u16, u16)) -> Result<(), Error> {
        self.with_entry(loc.dir_first, loc.dir_root, loc.index, |d| {
            d[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
            d[26..28].copy_from_slice(&(first as u16).to_le_bytes());
            d[28..32].copy_from_slice(&(size as u32).to_le_bytes());
            d[22..24].copy_from_slice(&time.0.to_le_bytes());
            d[24..26].copy_from_slice(&time.1.to_le_bytes());
            d[18..20].copy_from_slice(&time.1.to_le_bytes());
        })
    }

    /// `vfat_create_shortname` with `shortname=mixed` (create as Win95):
    /// the 8.3 alias and whether long-name slots must go with it.
    fn create_shortname(&self, taken: &[[u8; 11]], uname: &[u16]) -> Result<([u8; 11], bool), Error> {
        let exists = |n: &[u8; 11]| taken.iter().any(|t| t == n);
        let ulen = uname.len();
        let mut base_info = INIT_INFO;
        let mut ext_info = INIT_INFO;

        // The extension starts after the last dot, unless only dots and
        // spaces come before it or the dot ends the name.
        let mut sz = ulen;
        let mut ext_start: Option<usize> = None;
        if let Some(dot) = uname.iter().rposition(|&w| w == 0x2E) {
            if dot != ulen - 1 && uname[..dot].iter().any(|&w| !vfat_skip_char(w)) {
                sz = dot;
                ext_start = Some(dot + 1);
            }
        }

        let mut numtail_baselen = 6;
        let mut numtail2_baselen = 2;
        let mut base: Vec<u8> = Vec::new();
        let mut is_shortname = true;
        for (i, &w) in uname[..sz].iter().enumerate() {
            let Some(b) = to_shortname_char(w, &mut base_info) else { continue };
            let baselen = base.len();
            if baselen < 2 && baselen + 1 > 2 { numtail2_baselen = baselen; }
            if baselen < 6 && baselen + 1 > 6 { numtail_baselen = baselen; }
            base.push(b);
            if base.len() >= 8 {
                if i + 1 < sz { is_shortname = false; }
                break;
            }
        }
        if base.is_empty() { return Err(Error::BadName); }

        let mut ext: Vec<u8> = Vec::new();
        if let Some(es) = ext_start {
            for (k, &w) in uname[es..].iter().enumerate() {
                let Some(b) = to_shortname_char(w, &mut ext_info) else { continue };
                ext.push(b);
                if ext.len() >= 3 {
                    if es + k + 1 != ulen { is_shortname = false; }
                    break;
                }
            }
        }
        if base[0] == DELETED_FLAG { base[0] = 0x05; }

        let mut name = [b' '; 11];
        name[..base.len()].copy_from_slice(&base);
        name[8..8 + ext.len()].copy_from_slice(&ext);
        if is_shortname && base_info.valid && ext_info.valid {
            if exists(&name) { return Err(Error::Exists); }
            return Ok((name, !(base_info.upper && ext_info.upper)));
        }

        let mut baselen = base.len();
        if baselen > 6 {
            baselen = numtail_baselen;
            name[7] = b' ';
        }
        name[baselen] = b'~';
        for i in 1..10u8 {
            name[baselen + 1] = b'0' + i;
            if !exists(&name) { return Ok((name, true)); }
        }
        // `~n` with a hex hash before it once ~1..~9 are taken.
        let mut i = (self.now as u32).wrapping_mul(2_654_435_761);
        let sz = ((i >> 16) & 7) as u8;
        if baselen > 2 {
            baselen = numtail2_baselen;
            name[7] = b' ';
        }
        name[baselen + 4] = b'~';
        name[baselen + 5] = b'1' + sz;
        for _ in 0..0x10000 {
            let hex = alloc::format!("{:04X}", i & 0xFFFF);
            name[baselen..baselen + 4].copy_from_slice(hex.as_bytes());
            if !exists(&name) { return Ok((name, true)); }
            i = i.wrapping_sub(11);
        }
        Err(Error::NoSpace)
    }

    /// `fat_add_entries`: room for `need` entries in a row, the folder
    /// grown by a zeroed cluster when it has none (the fixed FAT12/16 root
    /// cannot grow). Returns the index of the first.
    fn find_slots(&mut self, dir: &Node, need: usize) -> Result<u32, Error> {
        let raw = self.dir_bytes(dir)?;
        let n = raw.len() / MSDOS_DPS_ENTRY;
        let mut run = 0usize;
        for i in 0..n {
            let b0 = raw[i * MSDOS_DPS_ENTRY];
            if b0 == 0 || b0 == DELETED_FLAG {
                run += 1;
                if run == need { return Ok((i + 1 - need) as u32); }
            } else {
                run = 0;
            }
        }
        if dir.root && self.fat_bits != 32 { return Err(Error::NoSpace); }
        let per = self.cluster_size as usize / MSDOS_DPS_ENTRY;
        let add = (need - run).div_ceil(per);
        if n + add * per > FAT_MAX_DIR_ENTRIES { return Err(Error::NoSpace); }
        let mut last = dir.first;
        let mut steps = 0u32;
        while let Some(c) = self.next(last)? {
            last = c;
            steps += 1;
            if steps >= self.max_cluster { return Err(Error::Corrupt); }
        }
        let new = self.alloc_clusters(add, last)?;
        let zero = alloc::vec![0u8; self.cluster_size as usize];
        for c in new {
            let s = self.cluster_sector(c);
            self.write_sectors(s, &zero)?;
        }
        Ok((n - run) as u32)
    }

    /// `vfat_add_entry`: the long-name slots and the 8.3 entry for `name`
    /// in `dir`. `skip` is an entry the name may match (a rename that
    /// changes only the case).
    fn add_entry(&mut self, dir: &Node, name: &str, attr: u8, first: u32, size: u64,
                 time: (u16, u16), skip: Option<Loc>) -> Result<Loc, Error> {
        let uname: Vec<u16> = name.encode_utf16().collect();
        valid_longname(&uname)?;
        let entries = self.read_dir(dir)?;
        let mut taken = Vec::with_capacity(entries.len());
        for e in &entries {
            if e.node.loc == skip && skip.is_some() { continue; }
            if same_name(&e.name, name) || same_name(&e.short, name) { return Err(Error::Exists); }
            taken.push(e.raw);
        }
        let (short, lfn) = self.create_shortname(&taken, &uname)?;
        let slots = if lfn { uname.len().div_ceil(13) } else { 0 };
        let start = self.find_slots(dir, slots + 1)?;

        // `vfat_build_slots`: the name, a terminator, 0xFFFF padding.
        let mut padded = uname.clone();
        if padded.len() % 13 != 0 { padded.push(0); }
        while padded.len() < slots * 13 { padded.push(0xFFFF); }
        let cksum = fat_checksum(&short);
        for s in 0..slots {
            let slot = slots - s;
            let off = (slot - 1) * 13;
            let part = &padded[off..off + 13];
            self.with_entry(dir.first, dir.root, start + s as u32, |d| {
                d.fill(0);
                d[0] = slot as u8 | if s == 0 { 0x40 } else { 0 };
                for k in 0..5 { d[1 + 2 * k..3 + 2 * k].copy_from_slice(&part[k].to_le_bytes()); }
                d[11] = ATTR_EXT;
                d[13] = cksum;
                for k in 0..6 { d[14 + 2 * k..16 + 2 * k].copy_from_slice(&part[5 + k].to_le_bytes()); }
                for k in 0..2 { d[28 + 2 * k..30 + 2 * k].copy_from_slice(&part[11 + k].to_le_bytes()); }
            })?;
        }
        let index = start + slots as u32;
        let (ct, cd) = unix2fat(self.now);
        self.with_entry(dir.first, dir.root, index, |d| {
            d.fill(0);
            d[0..11].copy_from_slice(&short);
            d[11] = attr;
            d[14..16].copy_from_slice(&ct.to_le_bytes());
            d[16..18].copy_from_slice(&cd.to_le_bytes());
            d[18..20].copy_from_slice(&time.1.to_le_bytes());
            d[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
            d[22..24].copy_from_slice(&time.0.to_le_bytes());
            d[24..26].copy_from_slice(&time.1.to_le_bytes());
            d[26..28].copy_from_slice(&(first as u16).to_le_bytes());
            d[28..32].copy_from_slice(&(size as u32).to_le_bytes());
        })?;
        Ok(Loc { dir_first: dir.first, dir_root: dir.root, index, first_slot: start })
    }

    fn parent(&mut self, path: &str) -> Result<(Node, alloc::string::String), Error> {
        let (dir, name) = split(path);
        if name.is_empty() { return Err(Error::BadName); }
        let node = self.lookup(dir)?.ok_or(Error::NotFound)?;
        if !node.is_dir { return Err(Error::WrongKind); }
        Ok((node, alloc::string::String::from(name)))
    }

    // === Operations ===

    /// An empty file at `path`; an existing file is cut to nothing.
    pub fn create(&mut self, path: &str) -> Result<(), Error> {
        self.cursor = None;
        self.mark()?;
        let now = unix2fat(self.now);
        match self.lookup(path)? {
            Some(n) if n.is_dir => return Err(Error::WrongKind),
            Some(n) => {
                let loc = n.loc.ok_or(Error::Corrupt)?;
                if n.first != 0 { self.free_chain(n.first)?; }
                self.update_entry(loc, 0, 0, now)?;
            }
            None => {
                let (dir, name) = self.parent(path)?;
                self.add_entry(&dir, &name, ATTR_ARCH, 0, 0, now, None)?;
            }
        }
        self.fat_flush()
    }

    /// Append `data` to the file at `path`, whose size must be `off`.
    pub fn append(&mut self, path: &str, off: u64, data: &[u8]) -> Result<(), Error> {
        let n = self.lookup(path)?.ok_or(Error::NotFound)?;
        if n.is_dir { return Err(Error::WrongKind); }
        if off != n.size { return Err(Error::Corrupt); }
        if data.is_empty() { return Ok(()); }
        let loc = n.loc.ok_or(Error::Corrupt)?;
        let new_size = n.size + data.len() as u64;
        if new_size > FAT_MAX_SIZE { return Err(Error::NoSpace); }
        self.mark()?;
        let cs = self.cluster_size as u64;
        let ss = self.sector_size as u64;
        let have = if n.first == 0 { 0 } else { n.size.div_ceil(cs).max(1) };
        let need = new_size.div_ceil(cs);
        // The chain's last cluster, from the cursor when it is there.
        let mut last = 0u32;
        if have > 0 {
            let (mut idx, mut c) = match self.cursor {
                Some(k) if k.first == n.first && k.idx < have => (k.idx, k.clus),
                _ => (0, n.first),
            };
            while idx + 1 < have {
                c = self.next(c)?.ok_or(Error::Corrupt)?;
                idx += 1;
            }
            last = c;
        }
        let new = self.alloc_clusters((need - have) as usize, last)?;
        let first = if n.first == 0 { new[0] } else { n.first };
        // Room left in the last cluster comes first.
        let mut clusters: Vec<u32> = Vec::new();
        let mut in_clus = 0;
        if have > 0 && have * cs > n.size {
            clusters.push(last);
            in_clus = n.size - (have - 1) * cs;
        }
        clusters.extend_from_slice(&new);

        // Runs of contiguous clusters, each in one write; a partial first
        // sector is read first, the tail of the last one is zero.
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
            let n_bytes = (room.min((data.len() - done) as u64)) as usize;
            let first_sec = in_clus / ss;
            let end = (in_clus + n_bytes as u64).div_ceil(ss);
            let mut buf = alloc::vec![0u8; ((end - first_sec) * ss) as usize];
            let head = (in_clus % ss) as usize;
            let base = self.cluster_sector(start) + first_sec;
            if head != 0 { self.read_sectors(base, &mut buf[..ss as usize])?; }
            buf[head..head + n_bytes].copy_from_slice(&data[done..done + n_bytes]);
            self.write_sectors(base, &buf)?;
            done += n_bytes;
            i += run;
            in_clus = 0;
        }
        let tail = *clusters.last().unwrap_or(&last);
        self.cursor = Some(super::Cursor { first, idx: need - 1, clus: tail });
        self.update_entry(loc, first, new_size, unix2fat(self.now))?;
        self.fat_flush()
    }

    /// `vfat_mkdir`: a folder with its `.` and `..` (`fat_alloc_new_dir`).
    pub fn mkdir(&mut self, path: &str) -> Result<(), Error> {
        self.cursor = None;
        if self.lookup(path)?.is_some() { return Err(Error::Exists); }
        let (dir, name) = self.parent(path)?;
        self.mark()?;
        let c = self.alloc_clusters(1, 0)?[0];
        let now = unix2fat(self.now);
        let mut b = alloc::vec![0u8; self.cluster_size as usize];
        let parent_first = if dir.root { 0 } else { dir.first };
        for (k, (nm, cl)) in [(b".          ", c), (b"..         ", parent_first)].into_iter().enumerate() {
            let d = &mut b[k * 32..k * 32 + 32];
            d[0..11].copy_from_slice(nm);
            d[11] = ATTR_DIR;
            d[14..16].copy_from_slice(&now.0.to_le_bytes());
            d[16..18].copy_from_slice(&now.1.to_le_bytes());
            d[18..20].copy_from_slice(&now.1.to_le_bytes());
            d[20..22].copy_from_slice(&((cl >> 16) as u16).to_le_bytes());
            d[22..24].copy_from_slice(&now.0.to_le_bytes());
            d[24..26].copy_from_slice(&now.1.to_le_bytes());
            d[26..28].copy_from_slice(&(cl as u16).to_le_bytes());
        }
        let s = self.cluster_sector(c);
        self.write_sectors(s, &b)?;
        if let Err(e) = self.add_entry(&dir, &name, ATTR_DIR, c, 0, now, None) {
            self.free_chain(c)?;
            self.fat_flush()?;
            return Err(e);
        }
        self.fat_flush()
    }

    /// A file, or an empty folder (`vfat_unlink`, `vfat_rmdir`).
    pub fn remove(&mut self, path: &str) -> Result<(), Error> {
        self.cursor = None;
        let n = self.lookup(path)?.ok_or(Error::NotFound)?;
        let loc = n.loc.ok_or(Error::WrongKind)?;
        if n.is_dir && !self.read_dir(&n)?.is_empty() { return Err(Error::NotEmpty); }
        self.mark()?;
        if n.first != 0 { self.free_chain(n.first)?; }
        self.remove_entries(loc)?;
        self.fat_flush()
    }

    /// `vfat_rename`: `from` gets the name `to`, in the same or another
    /// folder. A file at `to` is replaced when `replace` is set; a folder
    /// there never is.
    pub fn rename(&mut self, from: &str, to: &str, replace: bool) -> Result<(), Error> {
        self.cursor = None;
        let src = self.lookup(from)?.ok_or(Error::NotFound)?;
        let src_loc = src.loc.ok_or(Error::WrongKind)?;
        let (f, t) = (from.trim_matches('/'), to.trim_matches('/'));
        if src.is_dir && t.len() > f.len() && t.starts_with(f) && t.as_bytes()[f.len()] == b'/' {
            return Err(Error::BadName);
        }
        let (dir, name) = self.parent(to)?;
        self.mark()?;
        let mut skip = None;
        if let Some(dst) = self.lookup(to)? {
            if dst.loc == Some(src_loc) {
                skip = Some(src_loc);
            } else if !replace || dst.is_dir || src.is_dir {
                return Err(Error::Exists);
            } else {
                if dst.first != 0 { self.free_chain(dst.first)?; }
                self.remove_entries(dst.loc.ok_or(Error::Corrupt)?)?;
            }
        }
        self.add_entry(&dir, &name, src.attr, src.first, src.size, (src.wtime, src.wdate), skip)?;
        self.remove_entries(src_loc)?;
        let moved = src_loc.dir_first != dir.first || src_loc.dir_root != dir.root;
        if src.is_dir && moved && src.first != 0 {
            let parent_first = if dir.root { 0 } else { dir.first };
            self.with_entry(src.first, false, 1, |d| {
                d[20..22].copy_from_slice(&((parent_first >> 16) as u16).to_le_bytes());
                d[26..28].copy_from_slice(&(parent_first as u16).to_le_bytes());
            })?;
        }
        self.fat_flush()
    }
}
