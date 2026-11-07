//! External volumes: the `ext/` namespace.
//!
//! A drive's partition table and file systems are crafted input, so the
//! kernel parses none of it. For each disk it starts an `extfs` instance
//! with a block handle bounded to the disk; the instance reads the
//! partition table and reports the partitions. The kernel checks them and
//! starts one instance per partition, bounded to that range. A volume
//! instance mounts its file system, names itself, and serves the file
//! requests the kernel routes from `ext/<label>/...`. Every reply is
//! checked before it reaches the caller, and every request has a deadline.
//!
//! An instance holds no right: no store, no network, no other module. Its
//! block handle is bound to its capability, which only the kernel issues.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

use crate::kprintln;
use crate::security::capability::{self, CapId, Rights};
use crate::smp::fiber::{self, Waker, NO_WAKER, SIG_EXT};
use crate::storage::disk;

/// The namespace's root segment.
pub const ROOT: &str = "ext";
/// The module that parses partition tables and file systems.
pub const MODULE: &str = "extfs";

pub const ROLE_PROBE: u32 = 1;
pub const ROLE_VOLUME: u32 = 2;

pub const OP_LIST: u8 = 1;
pub const OP_STAT: u8 = 2;
pub const OP_READ: u8 = 3;
/// An empty file, or an existing one cut to nothing.
pub const OP_CREATE: u8 = 4;
/// Append the payload to a file whose size is the offset.
pub const OP_WRITE: u8 = 5;
pub const OP_MKDIR: u8 = 6;
/// A file or an empty folder.
pub const OP_DELETE: u8 = 7;
/// Rename to the path in the payload; offset 1 replaces a file there.
pub const OP_RENAME: u8 = 8;

/// Reply status: done, and no such entry.
pub const ST_OK: i32 = 0;
pub const ST_NOT_FOUND: i32 = 1;
pub const ST_EXISTS: i32 = 3;
pub const ST_NOT_EMPTY: i32 = 4;
pub const ST_NO_SPACE: i32 = 5;

/// Request header: seq u32, op u8, 3 bytes zero, offset u64, length u32,
/// path length u16, 2 bytes zero; the path follows, then `length` bytes of
/// payload for a write or a rename.
pub const REQ_HDR: usize = 24;

/// Largest block transfer per call and largest `OP_READ`.
pub const MAX_IO: usize = 1 << 20;
const MAX_PARTS: usize = 16;
const MAX_LABEL: usize = 32;
const MAX_LIST_BYTES: usize = 8 << 20;
const MAX_ENTRIES: usize = 1 << 16;
/// A FAT long name is 255 UTF-16 units, at most 765 bytes of UTF-8.
const MAX_NAME: usize = 1024;
const MAX_PATH: usize = 4096;
/// A request may need several device commands of 30 s each in the worst
/// case; past this the volume counts as hung.
const REQ_TIMEOUT_MS: u64 = 90_000;
/// The partition table and the mount.
const START_TIMEOUT_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No such volume, or it went away.
    Gone,
    /// The instance failed, did not answer in time, or answered garbage.
    Io,
    /// Not a valid path below `ext/`.
    BadPath,
    /// The volume or the drive is write-protected.
    ReadOnly,
    /// The two paths of a rename lie on different volumes.
    OtherVolume,
    /// The file system refused: the reply's status.
    Refused(i32),
}

pub struct Entry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    /// UTC seconds since the Unix epoch, 0 = unknown.
    pub mtime: u64,
}

#[derive(Clone, Copy)]
pub struct Stat {
    pub size: u64,
    pub is_dir: bool,
    pub mtime: u64,
}

pub struct VolumeInfo {
    pub label: String,
    pub disk: u32,
    pub total: u64,
    pub free: u64,
    pub read_only: bool,
}

struct Req {
    seq: u32,
    op: u8,
    path: String,
    off: u64,
    len: u32,
    data: Vec<u8>,
    waiter: Waker,
}

struct Inst {
    cap: CapId,
    disk: u32,
    role: u32,
    /// The range of the disk the handle covers, in device blocks.
    start: u64,
    blocks: u64,
    block_size: u32,
    read_only: bool,
    /// The disk went away; the instance is told to end.
    gone: bool,
    /// The instance broke a deadline or a rule; nothing is routed to it.
    failed: bool,
    /// Its drive is being ejected: no new request, the running ones end.
    closing: bool,
    born: u64,
    /// The instance's fiber while it waits in `next`.
    waker: Waker,
    /// The volume's name in `ext/`, once mounted.
    label: Option<String>,
    total: u64,
    free: u64,
    queue: VecDeque<Req>,
    taken: Vec<Req>,
    done: Vec<(u32, Result<Answer, Error>)>,
}

enum Answer {
    Entries(Vec<Entry>),
    Stat(Option<Stat>),
    Bytes(Vec<u8>),
    NotFound,
    /// A change went through; the volume's free bytes are updated.
    Done,
}

static INSTS: Mutex<Vec<Inst>> = Mutex::new(Vec::new());
static NEXT_SEQ: AtomicU32 = AtomicU32::new(1);

fn ms_to_tsc(ms: u64) -> u64 {
    ms.saturating_mul(crate::time::counter_hz() / 1000)
}

fn now() -> u64 {
    crate::time::counter()
}

// === Instances ===

fn spawn(disk: u32, role: u32, start: u64, blocks: u64, block_size: u32, read_only: bool) -> bool {
    let bytes = match crate::npkfs::fetch(&alloc::format!("sys/wasm/{}", MODULE)) {
        Ok((b, _)) => b,
        Err(_) => {
            kprintln!("[npk] ext: disk {} not read, `install {}` first", disk, MODULE);
            return false;
        }
    };
    let Ok(cap) = capability::create_module_cap(MODULE, Rights::empty(), None) else { return false };
    INSTS.lock().push(Inst {
        cap, disk, role, start, blocks, block_size, read_only,
        gone: false, failed: false, closing: false, born: now(), waker: NO_WAKER,
        label: None, total: 0, free: 0,
        queue: VecDeque::new(), taken: Vec::new(), done: Vec::new(),
    });
    if !crate::wasm::spawn_background_widget_app(bytes, cap, MODULE) {
        INSTS.lock().retain(|i| i.cap != cap);
        return false;
    }
    true
}

/// Disks plugged in before the store was unlocked: the module lives in the
/// store, so they are read once it is.
static WAITING: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static READY: AtomicBool = AtomicBool::new(false);

/// A new disk: start the instance that reads its partition table.
pub fn disk_added(disk: u32) {
    if !READY.load(Ordering::Acquire) {
        WAITING.lock().push(disk);
        return;
    }
    let Some(d) = disk::info(disk) else { return };
    spawn(disk, ROLE_PROBE, 0, d.blocks, d.block_size, d.read_only);
}

/// The store is unlocked: read the disks that waited for it.
pub fn store_ready() {
    READY.store(true, Ordering::Release);
    let waiting: Vec<u32> = core::mem::take(&mut *WAITING.lock());
    for disk in waiting { disk_added(disk); }
}

/// A disk went away or is being ejected: its instances are told to end and
/// every request still waiting fails.
pub fn disk_gone(disk: u32) {
    let mut g = INSTS.lock();
    for i in g.iter_mut().filter(|i| i.disk == disk) {
        fail_all(i, Error::Gone);
        i.gone = true;
        fiber::signal(i.waker, SIG_EXT);
    }
}

fn fail_all(i: &mut Inst, e: Error) {
    for r in i.queue.drain(..).chain(i.taken.drain(..)) {
        i.done.push((r.seq, Err(e)));
        fiber::signal(r.waiter, SIG_EXT);
    }
}

/// The instance with this capability ended.
pub fn instance_ended(cap: &CapId) {
    let mut g = INSTS.lock();
    let Some(k) = g.iter().position(|i| i.cap == *cap) else { return };
    let mut i = g.remove(k);
    fail_all(&mut i, Error::Gone);
    if i.role == ROLE_VOLUME && !i.gone {
        match &i.label {
            Some(l) => kprintln!("[npk] ext: {} ended", l),
            None => kprintln!("[npk] ext: disk {} at block {}: no file system read", i.disk, i.start),
        }
    }
}

/// True for an `extfs` instance the kernel started: such a module has no
/// private area and no settings file.
pub fn is_instance(cap: &CapId) -> bool {
    INSTS.lock().iter().any(|i| i.cap == *cap)
}

fn with_inst<R>(cap: &CapId, f: impl FnOnce(&mut Inst) -> R) -> Option<R> {
    INSTS.lock().iter_mut().find(|i| i.cap == *cap).map(f)
}

/// Instances that did not get as far as a mount in time are failed, so a
/// stuck partition table does not hold its disk forever.
fn reap_stuck() {
    let limit = ms_to_tsc(START_TIMEOUT_MS);
    let t = now();
    for i in INSTS.lock().iter_mut() {
        if i.label.is_none() && !i.failed && t.saturating_sub(i.born) > limit {
            i.failed = true;
            i.gone = true;
            fiber::signal(i.waker, SIG_EXT);
        }
    }
}

// === The block handle (`npk_blk_*`) ===

/// Role, block size, block count and write protection of the caller's
/// handle.
pub fn blk_info(cap: &CapId) -> Option<(u32, u32, u64, bool)> {
    with_inst(cap, |i| (i.role, i.block_size, i.blocks, i.read_only || i.role != ROLE_VOLUME))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlkError {
    /// Outside the handle, or not a whole number of blocks.
    Range,
    Gone,
    Io,
    ReadOnly,
}

fn blk_target(cap: &CapId, lba: u64, len: usize, write: bool) -> Result<(u32, u64), BlkError> {
    let (disk, start, blocks, bs, ro, role, gone) = with_inst(cap, |i|
        (i.disk, i.start, i.blocks, i.block_size, i.read_only, i.role, i.gone || i.failed))
        .ok_or(BlkError::Gone)?;
    if gone { return Err(BlkError::Gone); }
    if write && (ro || role != ROLE_VOLUME) { return Err(BlkError::ReadOnly); }
    if len == 0 || len > MAX_IO || len % bs as usize != 0 { return Err(BlkError::Range); }
    let count = (len / bs as usize) as u64;
    match lba.checked_add(count) {
        Some(end) if end <= blocks => {}
        _ => return Err(BlkError::Range),
    }
    let abs = start.checked_add(lba).ok_or(BlkError::Range)?;
    Ok((disk, abs))
}

fn blk_err(e: disk::Error) -> BlkError {
    match e {
        disk::Error::Gone => BlkError::Gone,
        disk::Error::ReadOnly => BlkError::ReadOnly,
        disk::Error::Range => BlkError::Range,
        _ => BlkError::Io,
    }
}

pub fn blk_read(cap: &CapId, lba: u64, out: &mut [u8]) -> Result<(), BlkError> {
    let (disk, abs) = blk_target(cap, lba, out.len(), false)?;
    disk::read(disk, abs, out).map_err(blk_err)
}

pub fn blk_write(cap: &CapId, lba: u64, data: &[u8]) -> Result<(), BlkError> {
    let (disk, abs) = blk_target(cap, lba, data.len(), true)?;
    disk::write(disk, abs, data).map_err(blk_err)
}

pub fn blk_flush(cap: &CapId) -> Result<(), BlkError> {
    let (disk, ro) = with_inst(cap, |i| (i.disk, i.read_only || i.role != ROLE_VOLUME))
        .ok_or(BlkError::Gone)?;
    if ro { return Ok(()); }
    disk::flush(disk).map_err(blk_err)
}

// === Bring-up (`npk_ext_parts`, `npk_ext_mount`) ===

/// The partitions a probe instance read: `(start, count)` in device blocks.
/// Each must lie inside the disk and not overlap another; then one volume
/// instance starts per partition. Returns how many started.
pub fn report_parts(cap: &CapId, parts: &[(u64, u64)]) -> Option<usize> {
    let (disk, blocks, bs, ro) = {
        let mut g = INSTS.lock();
        let i = g.iter_mut().find(|i| i.cap == *cap && i.role == ROLE_PROBE && !i.gone)?;
        // One report per probe.
        i.gone = true;
        (i.disk, i.blocks, i.block_size, i.read_only)
    };
    if parts.is_empty() || parts.len() > MAX_PARTS { return None; }
    let mut ok: Vec<(u64, u64)> = Vec::new();
    for &(s, n) in parts {
        let end = s.checked_add(n)?;
        if n == 0 || end > blocks { return None; }
        if ok.iter().any(|&(a, m)| s < a + m && a < end) { return None; }
        ok.push((s, n));
    }
    let mut started = 0;
    for (s, n) in ok {
        if spawn(disk, ROLE_VOLUME, s, n, bs, ro) { started += 1; }
    }
    Some(started)
}

/// A volume instance mounted its file system. The label is cleaned to a
/// path segment and made unique.
pub fn mount(cap: &CapId, label: &str, total: u64, free: u64) -> Option<String> {
    let mut base: String = label.chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .take(MAX_LABEL)
        .collect();
    base = String::from(base.trim());
    if base.is_empty() || base == "." || base == ".." { base = String::from("DISK"); }
    let mut g = INSTS.lock();
    let taken = |n: &str, g: &Vec<Inst>| g.iter().any(|i| i.label.as_deref() == Some(n));
    let mut name = base.clone();
    let mut k = 2;
    while taken(&name, &g) {
        name = alloc::format!("{}-{}", base, k);
        k += 1;
    }
    let i = g.iter_mut().find(|i| i.cap == *cap && i.role == ROLE_VOLUME && !i.gone && i.label.is_none())?;
    i.label = Some(name.clone());
    i.total = total;
    i.free = free.min(total);
    kprintln!("[npk] ext: disk {} mounted as {}/{} ({} MB{})", i.disk, ROOT, name,
        total / 1_000_000, if i.read_only { ", read-only" } else { "" });
    Some(name)
}

// === Serving (`npk_ext_next`, `npk_ext_reply`) ===

pub enum Next {
    /// A request: its header and path, written to the instance.
    Req(Vec<u8>),
    /// Nothing within the wait.
    Idle,
    /// The volume is gone; the instance should end.
    Gone,
}

/// The next request for the instance, waiting up to `timeout_ms`.
pub fn next(cap: &CapId, timeout_ms: u64, max: usize) -> Next {
    let deadline = now() + ms_to_tsc(timeout_ms);
    loop {
        let w = fiber::current_waker();
        let got = with_inst(cap, |i| {
            if let Some(w) = w { i.waker = w; }
            if i.gone || i.failed { return Some(Next::Gone); }
            let r = i.queue.front()?;
            if REQ_HDR + r.path.len() + r.data.len() > max { return Some(Next::Gone); }
            let r = i.queue.pop_front()?;
            let mut b = Vec::with_capacity(REQ_HDR + r.path.len() + r.data.len());
            b.extend_from_slice(&r.seq.to_le_bytes());
            b.extend_from_slice(&[r.op, 0, 0, 0]);
            b.extend_from_slice(&r.off.to_le_bytes());
            let len = if r.data.is_empty() { r.len } else { r.data.len() as u32 };
            b.extend_from_slice(&len.to_le_bytes());
            b.extend_from_slice(&(r.path.len() as u16).to_le_bytes());
            b.extend_from_slice(&[0, 0]);
            b.extend_from_slice(r.path.as_bytes());
            b.extend_from_slice(&r.data);
            i.taken.push(r);
            Some(Next::Req(b))
        });
        match got {
            None => return Next::Gone,
            Some(Some(n)) => return n,
            Some(None) => {}
        }
        if now() >= deadline { return Next::Idle; }
        if w.is_none() || fiber::wait(SIG_EXT, deadline).is_none() {
            fiber::spin_or_yield();
        }
    }
}

/// The instance's answer to request `seq`. Checked against what was asked
/// before it is handed on; an answer that breaks the format fails the
/// request and the volume.
pub fn reply(cap: &CapId, seq: u32, status: i32, data: &[u8]) -> bool {
    let mut g = INSTS.lock();
    let Some(i) = g.iter_mut().find(|i| i.cap == *cap) else { return false };
    let Some(k) = i.taken.iter().position(|r| r.seq == seq) else { return false };
    let r = i.taken.remove(k);
    let changes = matches!(r.op, OP_CREATE | OP_WRITE | OP_MKDIR | OP_DELETE | OP_RENAME);
    let ans = if status == ST_NOT_FOUND {
        Ok(Answer::NotFound)
    } else if status != ST_OK {
        Err(if changes { Error::Refused(status) } else { Error::Io })
    } else {
        match r.op {
            OP_LIST => parse_entries(data).map(Answer::Entries),
            OP_STAT => parse_stat(data).map(|s| Answer::Stat(Some(s))),
            OP_READ if data.len() <= r.len as usize => Some(Answer::Bytes(Vec::from(data))),
            _ if changes && data.len() == 8 => {
                let free = u64::from_le_bytes(data.try_into().unwrap_or([0; 8]));
                i.free = free.min(i.total);
                Some(Answer::Done)
            }
            _ => None,
        }.ok_or(Error::Io)
    };
    let broke = status == ST_OK && ans.is_err();
    i.done.push((seq, ans));
    fiber::signal(r.waiter, SIG_EXT);
    if broke {
        kprintln!("[npk] ext: {} answered out of format, volume stopped",
            i.label.as_deref().unwrap_or("?"));
        i.failed = true;
        fail_all(i, Error::Io);
        fiber::signal(i.waker, SIG_EXT);
    }
    true
}

/// A listing: per entry name length u16, is_dir u8, size u64, mtime u64,
/// then the name. Names are one path segment of valid UTF-8.
fn parse_entries(mut b: &[u8]) -> Option<Vec<Entry>> {
    if b.len() > MAX_LIST_BYTES { return None; }
    let mut out = Vec::new();
    while !b.is_empty() {
        if b.len() < 19 || out.len() >= MAX_ENTRIES { return None; }
        let n = u16::from_le_bytes([b[0], b[1]]) as usize;
        let is_dir = match b[2] { 0 => false, 1 => true, _ => return None };
        let size = u64::from_le_bytes(b[3..11].try_into().ok()?);
        let mtime = u64::from_le_bytes(b[11..19].try_into().ok()?);
        let name = core::str::from_utf8(b.get(19..19 + n)?).ok()?;
        if !segment_ok(name) { return None; }
        out.push(Entry { name: String::from(name), size: if is_dir { 0 } else { size }, is_dir, mtime });
        b = &b[19 + n..];
    }
    Some(out)
}

fn parse_stat(b: &[u8]) -> Option<Stat> {
    if b.len() != 17 { return None; }
    let is_dir = match b[0] { 0 => false, 1 => true, _ => return None };
    Some(Stat {
        is_dir,
        size: if is_dir { 0 } else { u64::from_le_bytes(b[1..9].try_into().ok()?) },
        mtime: u64::from_le_bytes(b[9..17].try_into().ok()?),
    })
}

fn segment_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_NAME && s != "." && s != ".."
        && !s.chars().any(|c| c == '/' || c == '\0' || c.is_control())
}

// === Callers ===

/// True if `path` lies in the `ext/` namespace (`ext` itself included).
pub fn is_ext(path: &str) -> bool {
    let p = path.trim_matches('/');
    p == ROOT || p.strip_prefix(ROOT).is_some_and(|r| r.starts_with('/'))
}

/// `ext/<label>/<rest>` split into label and the path inside the volume;
/// `None` for `ext` itself.
fn split(path: &str) -> Result<Option<(String, String)>, Error> {
    let p = path.trim_matches('/');
    if p == ROOT { return Ok(None); }
    let rest = p.strip_prefix(ROOT).and_then(|r| r.strip_prefix('/')).ok_or(Error::BadPath)?;
    if rest.len() > MAX_PATH { return Err(Error::BadPath); }
    let mut segs = rest.split('/');
    let label = segs.next().filter(|s| segment_ok(s)).ok_or(Error::BadPath)?;
    let mut inner = String::new();
    for s in segs {
        if !segment_ok(s) { return Err(Error::BadPath); }
        if !inner.is_empty() { inner.push('/'); }
        inner.push_str(s);
    }
    Ok(Some((String::from(label), inner)))
}

/// The mounted volumes.
pub fn volumes() -> Vec<VolumeInfo> {
    reap_stuck();
    INSTS.lock().iter()
        .filter(|i| !i.gone && !i.failed && !i.closing)
        .filter_map(|i| i.label.as_ref().map(|l| VolumeInfo {
            label: l.clone(), disk: i.disk, total: i.total, free: i.free, read_only: i.read_only,
        }))
        .collect()
}

fn call(label: &str, op: u8, path: &str, off: u64, len: u32) -> Result<Answer, Error> {
    call_with(label, op, path, off, len, Vec::new())
}

fn call_with(label: &str, op: u8, path: &str, off: u64, len: u32, data: Vec<u8>) -> Result<Answer, Error> {
    let seq = NEXT_SEQ.fetch_add(1, Ordering::Relaxed);
    let waiter = fiber::current_waker().unwrap_or(NO_WAKER);
    let cap = {
        let mut g = INSTS.lock();
        let i = g.iter_mut()
            .find(|i| i.label.as_deref() == Some(label) && !i.gone && !i.failed && !i.closing)
            .ok_or(Error::Gone)?;
        if (op == OP_CREATE || op == OP_WRITE || op == OP_MKDIR || op == OP_DELETE || op == OP_RENAME)
            && i.read_only
        {
            return Err(Error::ReadOnly);
        }
        i.queue.push_back(Req { seq, op, path: String::from(path), off, len, data, waiter });
        fiber::signal(i.waker, SIG_EXT);
        i.cap
    };
    let deadline = now() + ms_to_tsc(REQ_TIMEOUT_MS);
    loop {
        {
            let mut g = INSTS.lock();
            let Some(i) = g.iter_mut().find(|i| i.cap == cap) else { return Err(Error::Gone) };
            if let Some(k) = i.done.iter().position(|(s, _)| *s == seq) {
                return i.done.swap_remove(k).1;
            }
            if now() >= deadline {
                kprintln!("[npk] ext: {} did not answer, volume stopped", label);
                i.failed = true;
                fail_all(i, Error::Io);
                i.done.retain(|(s, _)| *s != seq);
                fiber::signal(i.waker, SIG_EXT);
                return Err(Error::Io);
            }
        }
        if waiter == NO_WAKER || fiber::wait(SIG_EXT, deadline).is_none() {
            fiber::spin_or_yield();
        }
    }
}

/// The entries of a folder, `None` if it does not exist.
pub fn list(path: &str) -> Result<Option<Vec<Entry>>, Error> {
    let Some((label, inner)) = split(path)? else {
        return Ok(Some(volumes().into_iter()
            .map(|v| Entry { name: v.label, size: 0, is_dir: true, mtime: 0 })
            .collect()));
    };
    if !volumes().iter().any(|v| v.label == label) { return Ok(None); }
    match call(&label, OP_LIST, &inner, 0, 0)? {
        // A file still being written is not shown under its partial name.
        Answer::Entries(mut e) => {
            e.retain(|x| !(x.name.starts_with('.') && x.name.ends_with(".npk-part")));
            Ok(Some(e))
        }
        Answer::NotFound => Ok(None),
        _ => Err(Error::Io),
    }
}

pub fn stat(path: &str) -> Result<Option<Stat>, Error> {
    let Some((label, inner)) = split(path)? else {
        return Ok(Some(Stat { size: 0, is_dir: true, mtime: 0 }));
    };
    let known = volumes().iter().any(|v| v.label == label);
    if !known { return Ok(None); }
    if inner.is_empty() { return Ok(Some(Stat { size: 0, is_dir: true, mtime: 0 })); }
    match call(&label, OP_STAT, &inner, 0, 0)? {
        Answer::Stat(s) => Ok(s),
        Answer::NotFound => Ok(None),
        _ => Err(Error::Io),
    }
}

/// Up to `out.len()` bytes of a file from `off`; 0 at the end.
pub fn read(path: &str, off: u64, out: &mut [u8]) -> Result<usize, Error> {
    let Some((label, inner)) = split(path)? else { return Err(Error::BadPath) };
    if inner.is_empty() { return Err(Error::BadPath); }
    let len = out.len().min(MAX_IO) as u32;
    match call(&label, OP_READ, &inner, off, len)? {
        Answer::Bytes(b) => {
            out[..b.len()].copy_from_slice(&b);
            Ok(b.len())
        }
        Answer::NotFound => Err(Error::Gone),
        _ => Err(Error::Io),
    }
}

/// A whole file, bounded by `max` bytes.
pub fn read_all(path: &str, max: usize) -> Result<Option<Vec<u8>>, Error> {
    let Some(st) = stat(path)? else { return Ok(None) };
    if st.is_dir || st.size > max as u64 { return Err(Error::Io); }
    let mut out = alloc::vec![0u8; st.size as usize];
    let mut at = 0usize;
    while at < out.len() {
        let n = read(path, at as u64, &mut out[at..])?;
        if n == 0 { out.truncate(at); break; }
        at += n;
    }
    Ok(Some(out))
}

// === Changes ===

fn change(path: &str, op: u8, off: u64, data: Vec<u8>) -> Result<(), Error> {
    let Some((label, inner)) = split(path)? else { return Err(Error::BadPath) };
    if inner.is_empty() { return Err(Error::BadPath); }
    match call_with(&label, op, &inner, off, 0, data)? {
        Answer::Done => Ok(()),
        Answer::NotFound => Err(Error::Refused(ST_NOT_FOUND)),
        _ => Err(Error::Io),
    }
}

/// True if `path` lies on a mounted volume that can be written.
pub fn writable(path: &str) -> bool {
    let Ok(Some((label, _))) = split(path) else { return false };
    volumes().iter().any(|v| v.label == label && !v.read_only)
}

/// An empty file at `path`, or the file there cut to nothing.
pub fn create(path: &str) -> Result<(), Error> {
    change(path, OP_CREATE, 0, Vec::new())
}

/// Append `data` (at most `MAX_IO` bytes) to the file at `path`, whose
/// size is `off`.
pub fn write(path: &str, off: u64, data: &[u8]) -> Result<(), Error> {
    if data.len() > MAX_IO { return Err(Error::BadPath); }
    change(path, OP_WRITE, off, Vec::from(data))
}

pub fn mkdir(path: &str) -> Result<(), Error> {
    change(path, OP_MKDIR, 0, Vec::new())
}

/// A file or an empty folder.
pub fn delete(path: &str) -> Result<(), Error> {
    change(path, OP_DELETE, 0, Vec::new())
}

/// `from` renamed to `to` on the same volume; with `replace` a file at
/// `to` gives way.
pub fn rename(from: &str, to: &str, replace: bool) -> Result<(), Error> {
    let (Some((la, _)), Some((lb, inner_to))) = (split(from)?, split(to)?) else { return Err(Error::BadPath) };
    if la != lb { return Err(Error::OtherVolume); }
    if inner_to.is_empty() { return Err(Error::BadPath); }
    change(from, OP_RENAME, replace as u64, Vec::from(inner_to.as_bytes()))
}

/// How long an eject waits for the requests already running.
const EJECT_WAIT_MS: u64 = 30_000;

/// Eject the drive volume `label` is on: its volumes take no new request,
/// the running ones finish, then the drive is flushed, stopped and gone
/// from the list (`disk::eject`).
pub fn eject(label: &str) -> Result<(), Error> {
    let disk = INSTS.lock().iter()
        .find(|i| i.label.as_deref() == Some(label) && !i.gone && !i.failed && !i.closing)
        .map(|i| i.disk)
        .ok_or(Error::Gone)?;
    for i in INSTS.lock().iter_mut().filter(|i| i.disk == disk) { i.closing = true; }
    let deadline = now() + ms_to_tsc(EJECT_WAIT_MS);
    loop {
        let busy = INSTS.lock().iter()
            .any(|i| i.disk == disk && (!i.queue.is_empty() || !i.taken.is_empty()));
        if !busy || now() >= deadline { break; }
        if !fiber::yield_sleep(10) { fiber::spin_or_yield(); }
    }
    disk::eject(disk).map_err(|_| Error::Io)
}
