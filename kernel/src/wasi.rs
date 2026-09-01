//! `wasi_snapshot_preview1` — the second ABI.
//!
//! Everything else in this kernel talks `npk_*`: 123 functions that were
//! designed here, for a system with capabilities instead of permissions
//! and content addresses instead of a path tree. This module is the one
//! place that speaks somebody else's language, and it exists because the
//! programs worth borrowing — CPython, lua, sqlite — were all written
//! against POSIX and compiled through wasi-libc.
//!
//! It is deliberately NOT a Python feature. A guest that gets this
//! namespace gets a filesystem-shaped view of ONE npkFS subtree it was
//! handed, and nothing else. That framing is what makes the POSIX dent
//! in the architecture worth its price: every wasi binary lands the same
//! way, under the same grant, with the same ceiling.
//!
//! ## The shape of the grant
//!
//! `path_open` and friends resolve only under a preopened directory the
//! caller passed in. `resolve` refuses absolute escapes and any `..`
//! that would climb above the root. There is no way to name a path
//! outside the grant, so "which files can this program see" is answered
//! once, at spawn, by whoever built the `WasiCtx` — not by the program.
//!
//! ## Whole-file storage
//!
//! npkFS reads and writes whole objects; there is no seek at the storage
//! layer. So an opened file is fetched once into the fd entry and served
//! from there, and a written file is flushed on close. That is a real
//! memory cost — CPython holds an 8.5 MB stdlib zip open for its whole
//! run — and it is the honest shape for a content-addressed store: a
//! blob has a hash, and half a blob does not.

#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use wasmi::{Caller, Extern, Linker, Memory};

use crate::capability::{self, Rights};
use crate::storage::npkfs::fs;
use crate::storage::npkfs::object::EntryKind;

// ── errno (preview1 numbering — do not renumber) ──────────────────────
pub const SUCCESS: i32 = 0;
const EBADF: i32 = 8;
const EEXIST: i32 = 20;
const EFAULT: i32 = 21;
const EINVAL: i32 = 28;
const EIO: i32 = 29;
const EISDIR: i32 = 31;
const ENOENT: i32 = 44;
const ENOSYS: i32 = 52;
const ENOTDIR: i32 = 54;
const ENOTEMPTY: i32 = 55;
const EPERM: i32 = 63;
const ESPIPE: i32 = 70;
const ENOTCAPABLE: i32 = 76;

// ── filetype ──────────────────────────────────────────────────────────
const FT_CHR: u8 = 2;
const FT_DIR: u8 = 3;
const FT_REG: u8 = 4;

// ── oflags / fdflags / rights ─────────────────────────────────────────
const O_CREAT: i32 = 1;
const O_DIRECTORY: i32 = 2;
const O_EXCL: i32 = 4;
const O_TRUNC: i32 = 8;
const FD_APPEND: i32 = 1;
const RIGHT_FD_WRITE: i64 = 1 << 6;

/// An open descriptor.
///
/// `File` carries the bytes because npkFS has no seek — see the module
/// header. `dirty` decides whether closing has to write back.
pub enum Handle {
    Stdin,
    Stdout,
    Stderr,
    File {
        path: String,
        data: Vec<u8>,
        pos: usize,
        dirty: bool,
        writable: bool,
    },
    Dir {
        /// npkFS path.
        path: String,
        /// What the guest calls it. Only meaningful for preopens.
        guest: String,
        preopen: bool,
        /// The grant this handle descends from. `..` may not climb past
        /// it, and every subdirectory opened through it inherits it —
        /// which is what lets several preopens coexist without one
        /// becoming a door into another.
        root: String,
        writable: bool,
    },
}

pub struct WasiCtx {
    fds: BTreeMap<i32, Handle>,
    next_fd: i32,
    args: Vec<String>,
    env: Vec<String>,
    /// Womit sich das Programm verabschiedet hat. Der Trap-Code sagt nur DASS
    /// es sich beendet hat; der Status gehoert hierher, weil ihn beide Motoren
    /// auf demselben Weg hinterlegen.
    exit_status: Option<i32>,
}

impl WasiCtx {
    pub fn new(args: Vec<String>, env: Vec<String>) -> Self {
        let mut fds = BTreeMap::new();
        fds.insert(0, Handle::Stdin);
        fds.insert(1, Handle::Stdout);
        fds.insert(2, Handle::Stderr);
        WasiCtx { fds, next_fd: 3, args, env, exit_status: None }
    }

    /// Hand the guest one npkFS directory under the name `guest`.
    ///
    /// Call order matters only in that preopen fds must be contiguous
    /// from 3 — wasi-libc discovers them by walking upward until one
    /// answers EBADF, and a hole would hide everything after it.
    pub fn preopen(&mut self, npkfs_path: &str, guest: &str, writable: bool) {
        let path = npkfs_path.trim_matches('/').to_string();
        let h = Handle::Dir {
            root: path.clone(),
            path,
            guest: guest.to_string(),
            preopen: true,
            writable,
        };
        self.insert(h);
    }

    fn insert(&mut self, h: Handle) -> i32 {
        let fd = self.next_fd;
        self.next_fd += 1;
        self.fds.insert(fd, h);
        fd
    }

    /// Resolve a guest path under `dir_fd` to an npkFS path.
    ///
    /// This is the whole security boundary, so it stays boring: split
    /// into components, refuse `..` at the floor, rebuild. A leading `/`
    /// contributes an empty component and is skipped — preview1 paths
    /// are always relative to the directory fd, so an absolute-looking
    /// path still lands inside the grant rather than beside it.
    /// Returns `(npkfs_path, grant_root, writable)`.
    fn resolve(&self, dir_fd: i32, rel: &str) -> Result<(String, String, bool), i32> {
        let (base, root, writable) = match self.fds.get(&dir_fd) {
            Some(Handle::Dir { path, root, writable, .. }) => (path.clone(), root.clone(), *writable),
            Some(_) => return Err(ENOTDIR),
            None => return Err(EBADF),
        };
        match resolve_under(&base, &root, rel) {
            Ok(p) => Ok((p, root, writable)),
            Err(Reject::Escape) => Err(ENOTCAPABLE),
            Err(Reject::Invalid) => Err(EINVAL),
        }
    }
}

include!("wasi_resolve.rs");

/// `fs::Error` is npkFS's `PathError` under an alias — see
/// `storage/npkfs/fs.rs`.
fn fs_errno(e: &fs::Error) -> i32 {
    match e {
        fs::Error::NotFound       => ENOENT,
        fs::Error::NotADirectory  => ENOTDIR,
        fs::Error::AlreadyExists  => EEXIST,
        fs::Error::NotEmpty       => ENOTEMPTY,
        fs::Error::InvalidPath    => EINVAL,
        fs::Error::Corrupt        => EIO,
        fs::Error::Storage(_)     => EIO,
    }
}

// ── guest memory pokes ────────────────────────────────────────────────

fn mem_of(caller: &mut Caller<'_, crate::wasm::HostState>) -> Result<Memory, i32> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Ok(m),
        _ => Err(EFAULT),
    }
}
fn w32(m: &mut [u8], at: i32, v: u32) -> Result<(), i32> {
    let a = usize::try_from(at).map_err(|_| EFAULT)?;
    let s = m.get_mut(a..a.checked_add(4).ok_or(EFAULT)?).ok_or(EFAULT)?;
    s.copy_from_slice(&v.to_le_bytes());
    Ok(())
}
fn w64(m: &mut [u8], at: i32, v: u64) -> Result<(), i32> {
    let a = usize::try_from(at).map_err(|_| EFAULT)?;
    let s = m.get_mut(a..a.checked_add(8).ok_or(EFAULT)?).ok_or(EFAULT)?;
    s.copy_from_slice(&v.to_le_bytes());
    Ok(())
}
fn r32(m: &[u8], at: i32) -> Result<u32, i32> {
    let a = usize::try_from(at).map_err(|_| EFAULT)?;
    let s = m.get(a..a.checked_add(4).ok_or(EFAULT)?).ok_or(EFAULT)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
/// Copy `len` bytes out of guest memory at `ptr`.
///
/// `try_from` on every offset, not `as usize`: a negative i32 from a
/// buggy or hostile guest becomes a huge usize under `as`, and that is
/// the shape of the overflow already logged against ~25 `npk_*`
/// functions. Not repeating it here.
fn bytes_at(m: &[u8], ptr: i32, len: i32) -> Result<&[u8], i32> {
    let a = usize::try_from(ptr).map_err(|_| EFAULT)?;
    let l = usize::try_from(len).map_err(|_| EFAULT)?;
    m.get(a..a.checked_add(l).ok_or(EFAULT)?).ok_or(EFAULT)
}
fn put_bytes(m: &mut [u8], ptr: i32, src: &[u8]) -> Result<(), i32> {
    let a = usize::try_from(ptr).map_err(|_| EFAULT)?;
    let s = m.get_mut(a..a.checked_add(src.len()).ok_or(EFAULT)?).ok_or(EFAULT)?;
    s.copy_from_slice(src);
    Ok(())
}
fn guest_str(m: &[u8], ptr: i32, len: i32) -> Result<String, i32> {
    let b = bytes_at(m, ptr, len)?;
    core::str::from_utf8(b).map(|s| s.to_string()).map_err(|_| EINVAL)
}

fn filestat_bytes(kind: EntryKind, size: u64, mtime_secs: u64) -> [u8; 64] {
    let mut b = [0u8; 64];
    b[16] = match kind { EntryKind::Dir => FT_DIR, _ => FT_REG };
    b[24..32].copy_from_slice(&1u64.to_le_bytes());
    b[32..40].copy_from_slice(&size.to_le_bytes());
    let ns = mtime_secs.saturating_mul(1_000_000_000);
    b[40..48].copy_from_slice(&ns.to_le_bytes());
    b[48..56].copy_from_slice(&ns.to_le_bytes());
    b[56..64].copy_from_slice(&ns.to_le_bytes());
    b
}

// ── the 42 imports ────────────────────────────────────────────────────
//
// python.wasm imports exactly these. Ten answer ENOSYS on purpose —
// sockets, symlinks, hard links, the *_set_times family. CPython treats
// those failures as "this filesystem cannot do that", which is the
// truth: npkFS has no links and no per-file timestamps to set.

/// Grab guest memory and the host state together. Every function starts
/// here, and a run that was never granted a `WasiCtx` bounces on the
/// second step — the namespace is always linked, but it is inert unless
/// someone deliberately built a grant.
macro_rules! wasi_of {
    ($state:expr) => {
        match $state.wasi.as_mut() {
            Some(w) => &mut **w,
            None => return ENOTCAPABLE,
        }
    };
}

type HS = crate::wasm::HostState;

pub(crate) mod calls;
pub(crate) mod forge_glue;

/// Was `proc_exit` hinterlaesst — von beiden Motoren gleich geschrieben.
pub(crate) fn record_exit(st: &mut HS, code: i32) {
    if let Some(w) = st.wasi.as_mut() {
        w.exit_status = Some(code);
    }
}

/// Und wieder heraus. `None` heisst: das Programm ist normal aus `_start`
/// zurueckgekehrt, ohne sich zu verabschieden.
pub fn exit_status(st: &HS) -> Option<i32> {
    st.wasi.as_ref().and_then(|w| w.exit_status)
}

pub fn link(linker: &mut Linker<HS>) -> Result<(), wasmi::Error> {
    const NS: &str = "wasi_snapshot_preview1";

    // ── process ───────────────────────────────────────────────────────
    // Das EINZIGE, was die Motoren wirklich unterscheidet. Der Effekt ist
    // gemeinsam — den Status hinterlegen —, das Verlassen nicht: der
    // Interpreter macht daraus ein `Err` und rollt ab, erzeugter Code nimmt
    // `forge_rt::host_trap`. Beides endet im selben Zustand.
    linker.func_wrap(NS, "proc_exit",
        |mut c: Caller<'_, HS>, code: i32| -> Result<(), wasmi::Error> {
            record_exit(c.data_mut(), code);
            Err(wasmi::Error::i32_exit(code))
        })?;
    linker.func_wrap(NS, "sched_yield", |mut c: Caller<'_, HS>| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::sched_yield(mem, st)
    })?;

    // ── argv / environ ────────────────────────────────────────────────
    linker.func_wrap(NS, "args_sizes_get", |mut c: Caller<'_, HS>, n: i32, sz: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::args_sizes_get(mem, st, n, sz)
    })?;
    linker.func_wrap(NS, "args_get", |mut c: Caller<'_, HS>, argv: i32, buf: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::args_get(mem, st, argv, buf)
    })?;
    linker.func_wrap(NS, "environ_sizes_get", |mut c: Caller<'_, HS>, n: i32, sz: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::environ_sizes_get(mem, st, n, sz)
    })?;
    linker.func_wrap(NS, "environ_get", |mut c: Caller<'_, HS>, ep: i32, buf: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::environ_get(mem, st, ep, buf)
    })?;

    // ── clocks + randomness ───────────────────────────────────────────
    linker.func_wrap(NS, "clock_res_get", |mut c: Caller<'_, HS>, _id: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::clock_res_get(mem, st, _id, out)
    })?;
    linker.func_wrap(NS, "clock_time_get", |mut c: Caller<'_, HS>, id: i32, _p: i64, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::clock_time_get(mem, st, id, _p, out)
    })?;
    linker.func_wrap(NS, "random_get", |mut c: Caller<'_, HS>, buf: i32, len: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::random_get(mem, st, buf, len)
    })?;

    link_fd(linker)?;
    link_path(linker)?;
    link_stubs(linker)?;
    Ok(())
}

fn write_string_vec(mem: &mut [u8], ptr_arr: i32, buf: i32, items: &[String]) -> i32 {
    let mut p = buf;
    for (i, s) in items.iter().enumerate() {
        let slot = match i32::try_from(i * 4) { Ok(v) => v, Err(_) => return EFAULT };
        let slot = match ptr_arr.checked_add(slot) { Some(v) => v, None => return EFAULT };
        let pu = match u32::try_from(p) { Ok(v) => v, Err(_) => return EFAULT };
        if let Err(e) = w32(mem, slot, pu) { return e; }
        if let Err(e) = put_bytes(mem, p, s.as_bytes()) { return e; }
        let after = match p.checked_add(s.len() as i32) { Some(v) => v, None => return EFAULT };
        if let Err(e) = put_bytes(mem, after, &[0u8]) { return e; }
        p = match after.checked_add(1) { Some(v) => v, None => return EFAULT };
    }
    SUCCESS
}

// ── fd: the read/write path ───────────────────────────────────────────

/// Gather the `iovec` array into one buffer. Copying first keeps the
/// borrow of guest memory apart from the borrow of the fd table.
fn gather(mem: &[u8], iovs: i32, iovs_len: i32) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    for i in 0..iovs_len {
        let base = iovs.checked_add(i.checked_mul(8).ok_or(EFAULT)?).ok_or(EFAULT)?;
        let ptr = i32::try_from(r32(mem, base)?).map_err(|_| EFAULT)?;
        let len = i32::try_from(r32(mem, base + 4)?).map_err(|_| EFAULT)?;
        out.extend_from_slice(bytes_at(mem, ptr, len)?);
    }
    Ok(out)
}

fn link_fd(linker: &mut Linker<HS>) -> Result<(), wasmi::Error> {
    const NS: &str = "wasi_snapshot_preview1";

    linker.func_wrap(NS, "fd_write", |mut c: Caller<'_, HS>, fd: i32, iovs: i32, n: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_write(mem, st, fd, iovs, n, out)
    })?;

    linker.func_wrap(NS, "fd_read", |mut c: Caller<'_, HS>, fd: i32, iovs: i32, n: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_read(mem, st, fd, iovs, n, out)
    })?;

    linker.func_wrap(NS, "fd_pread", |mut c: Caller<'_, HS>, fd: i32, iovs: i32, n: i32, off: i64, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_pread(mem, st, fd, iovs, n, off, out)
    })?;

    linker.func_wrap(NS, "fd_pwrite", |mut c: Caller<'_, HS>, fd: i32, iovs: i32, n: i32, off: i64, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_pwrite(mem, st, fd, iovs, n, off, out)
    })?;

    linker.func_wrap(NS, "fd_seek", |mut c: Caller<'_, HS>, fd: i32, off: i64, whence: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_seek(mem, st, fd, off, whence, out)
    })?;

    linker.func_wrap(NS, "fd_tell", |mut c: Caller<'_, HS>, fd: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_tell(mem, st, fd, out)
    })?;

    linker.func_wrap(NS, "fd_close", |mut c: Caller<'_, HS>, fd: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_close(mem, st, fd)
    })?;
    linker.func_wrap(NS, "fd_sync", |mut c: Caller<'_, HS>, _fd: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_sync(mem, st, _fd)
    })?;
    linker.func_wrap(NS, "fd_datasync", |mut c: Caller<'_, HS>, _fd: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_datasync(mem, st, _fd)
    })?;
    linker.func_wrap(NS, "fd_advise", |mut c: Caller<'_, HS>, _f: i32, _o: i64, _l: i64, _a: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_advise(mem, st, _f, _o, _l, _a)
    })?;
    linker.func_wrap(NS, "fd_fdstat_set_flags", |mut c: Caller<'_, HS>, _f: i32, _fl: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_fdstat_set_flags(mem, st, _f, _fl)
    })?;

    linker.func_wrap(NS, "fd_fdstat_get", |mut c: Caller<'_, HS>, fd: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_fdstat_get(mem, st, fd, out)
    })?;

    linker.func_wrap(NS, "fd_filestat_get", |mut c: Caller<'_, HS>, fd: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_filestat_get(mem, st, fd, out)
    })?;

    linker.func_wrap(NS, "fd_prestat_get", |mut c: Caller<'_, HS>, fd: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_prestat_get(mem, st, fd, out)
    })?;
    linker.func_wrap(NS, "fd_prestat_dir_name", |mut c: Caller<'_, HS>, fd: i32, ptr: i32, len: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_prestat_dir_name(mem, st, fd, ptr, len)
    })?;

    linker.func_wrap(NS, "fd_readdir", |mut c: Caller<'_, HS>, fd: i32, buf: i32, buf_len: i32, cookie: i64, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_readdir(mem, st, fd, buf, buf_len, cookie, out)
    })?;

    linker.func_wrap(NS, "poll_oneoff", |mut c: Caller<'_, HS>, _i: i32, _o: i32, _n: i32, nev: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::poll_oneoff(mem, st, _i, _o, _n, nev)
    })?;
    Ok(())
}

// ── path-based calls ──────────────────────────────────────────────────

fn link_path(linker: &mut Linker<HS>) -> Result<(), wasmi::Error> {
    const NS: &str = "wasi_snapshot_preview1";

    linker.func_wrap(NS, "path_open", |mut c: Caller<'_, HS>, dirfd: i32, _dirflags: i32, path: i32, path_len: i32, oflags: i32, rights: i64, _rights_inh: i64, _fdflags: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_open(mem, st, dirfd, _dirflags, path, path_len, oflags, rights, _rights_inh, _fdflags, out)
    })?;

    linker.func_wrap(NS, "path_filestat_get", |mut c: Caller<'_, HS>, dirfd: i32, _flags: i32, path: i32, path_len: i32, out: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_filestat_get(mem, st, dirfd, _flags, path, path_len, out)
    })?;

    linker.func_wrap(NS, "path_create_directory", |mut c: Caller<'_, HS>, dirfd: i32, p: i32, pl: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_create_directory(mem, st, dirfd, p, pl)
    })?;

    linker.func_wrap(NS, "path_remove_directory", |mut c: Caller<'_, HS>, dirfd: i32, p: i32, pl: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_remove_directory(mem, st, dirfd, p, pl)
    })?;

    linker.func_wrap(NS, "path_unlink_file", |mut c: Caller<'_, HS>, dirfd: i32, p: i32, pl: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_unlink_file(mem, st, dirfd, p, pl)
    })?;

    linker.func_wrap(NS, "path_rename", |mut c: Caller<'_, HS>, ofd: i32, op: i32, ol: i32, nfd: i32, np: i32, nl: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_rename(mem, st, ofd, op, ol, nfd, np, nl)
    })?;
    Ok(())
}

// ── the ten that answer "no" ──────────────────────────────────────────
//
// Not laziness: npkFS has no links, no symlinks and no settable
// timestamps, and there are no sockets behind this ABI. ENOSYS is the
// true answer, and CPython handles it — `os.symlink` raises
// OSError, which is what it should do on a filesystem without symlinks.

fn link_stubs(linker: &mut Linker<HS>) -> Result<(), wasmi::Error> {
    const NS: &str = "wasi_snapshot_preview1";
    linker.func_wrap(NS, "fd_filestat_set_size", |mut c: Caller<'_, HS>, a0: i32, a1: i64| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_filestat_set_size(mem, st, a0, a1)
    })?;
    linker.func_wrap(NS, "fd_filestat_set_times", |mut c: Caller<'_, HS>, a0: i32, a1: i64, a2: i64, a3: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_filestat_set_times(mem, st, a0, a1, a2, a3)
    })?;
    linker.func_wrap(NS, "path_filestat_set_times", |mut c: Caller<'_, HS>, a0: i32, a1: i32, a2: i32, a3: i32, a4: i64, a5: i64, a6: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_filestat_set_times(mem, st, a0, a1, a2, a3, a4, a5, a6)
    })?;
    linker.func_wrap(NS, "path_link", |mut c: Caller<'_, HS>, a0: i32, a1: i32, a2: i32, a3: i32, a4: i32, a5: i32, a6: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_link(mem, st, a0, a1, a2, a3, a4, a5, a6)
    })?;
    linker.func_wrap(NS, "path_symlink", |mut c: Caller<'_, HS>, a0: i32, a1: i32, a2: i32, a3: i32, a4: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_symlink(mem, st, a0, a1, a2, a3, a4)
    })?;
    linker.func_wrap(NS, "path_readlink", |mut c: Caller<'_, HS>, a0: i32, a1: i32, a2: i32, a3: i32, a4: i32, a5: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::path_readlink(mem, st, a0, a1, a2, a3, a4, a5)
    })?;
    linker.func_wrap(NS, "fd_renumber", |mut c: Caller<'_, HS>, a0: i32, a1: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::fd_renumber(mem, st, a0, a1)
    })?;
    linker.func_wrap(NS, "sock_accept", |mut c: Caller<'_, HS>, a0: i32, a1: i32, a2: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::sock_accept(mem, st, a0, a1, a2)
    })?;
    linker.func_wrap(NS, "sock_recv", |mut c: Caller<'_, HS>, a0: i32, a1: i32, a2: i32, a3: i32, a4: i32, a5: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::sock_recv(mem, st, a0, a1, a2, a3, a4, a5)
    })?;
    linker.func_wrap(NS, "sock_send", |mut c: Caller<'_, HS>, a0: i32, a1: i32, a2: i32, a3: i32, a4: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::sock_send(mem, st, a0, a1, a2, a3, a4)
    })?;
    linker.func_wrap(NS, "sock_shutdown", |mut c: Caller<'_, HS>, a0: i32, a1: i32| -> i32 {
        let mem = match mem_of(&mut c) { Ok(m) => m, Err(e) => return e };
        let (mem, st) = mem.data_and_store_mut(&mut c);
        calls::sock_shutdown(mem, st, a0, a1)
    })?;
    Ok(())
}

/// Build a context. Callers add their preopens with `preopen` — there
/// is no default grant, because "what can this program see" should be a
/// decision somebody wrote down, not a fallback.
pub fn ctx(args: Vec<String>, env: Vec<String>) -> Box<WasiCtx> {
    Box::new(WasiCtx::new(args, env))
}
