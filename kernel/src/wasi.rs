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
}

impl WasiCtx {
    pub fn new(args: Vec<String>, env: Vec<String>) -> Self {
        let mut fds = BTreeMap::new();
        fds.insert(0, Handle::Stdin);
        fds.insert(1, Handle::Stdout);
        fds.insert(2, Handle::Stderr);
        WasiCtx { fds, next_fd: 3, args, env }
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
macro_rules! state {
    ($caller:expr) => {{
        let mem = match mem_of(&mut $caller) { Ok(m) => m, Err(e) => return e };
        mem.data_and_store_mut(&mut $caller)
    }};
}
macro_rules! wasi_of {
    ($state:expr) => {
        match $state.wasi.as_mut() {
            Some(w) => &mut **w,
            None => return ENOTCAPABLE,
        }
    };
}

type HS = crate::wasm::HostState;

pub fn link(linker: &mut Linker<HS>) -> Result<(), wasmi::Error> {
    const NS: &str = "wasi_snapshot_preview1";

    // ── process ───────────────────────────────────────────────────────
    linker.func_wrap(NS, "proc_exit",
        |_caller: Caller<'_, HS>, code: i32| -> Result<(), wasmi::Error> {
            Err(wasmi::Error::i32_exit(code))
        })?;
    linker.func_wrap(NS, "sched_yield", |_c: Caller<'_, HS>| -> i32 { SUCCESS })?;

    // ── argv / environ ────────────────────────────────────────────────
    linker.func_wrap(NS, "args_sizes_get", |mut c: Caller<'_, HS>, n: i32, sz: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let count = w.args.len() as u32;
        let bytes: u32 = w.args.iter().map(|a| a.len() as u32 + 1).sum();
        if let Err(e) = w32(mem, n, count) { return e; }
        if let Err(e) = w32(mem, sz, bytes) { return e; }
        SUCCESS
    })?;
    linker.func_wrap(NS, "args_get", |mut c: Caller<'_, HS>, argv: i32, buf: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let items = w.args.clone();
        write_string_vec(mem, argv, buf, &items)
    })?;
    linker.func_wrap(NS, "environ_sizes_get", |mut c: Caller<'_, HS>, n: i32, sz: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let count = w.env.len() as u32;
        let bytes: u32 = w.env.iter().map(|a| a.len() as u32 + 1).sum();
        if let Err(e) = w32(mem, n, count) { return e; }
        if let Err(e) = w32(mem, sz, bytes) { return e; }
        SUCCESS
    })?;
    linker.func_wrap(NS, "environ_get", |mut c: Caller<'_, HS>, ep: i32, buf: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let items = w.env.clone();
        write_string_vec(mem, ep, buf, &items)
    })?;

    // ── clocks + randomness ───────────────────────────────────────────
    linker.func_wrap(NS, "clock_res_get", |mut c: Caller<'_, HS>, _id: i32, out: i32| -> i32 {
        let (mem, st) = state!(c);
        let _ = wasi_of!(st);
        // The tick is 100 Hz, and saying so beats claiming nanoseconds
        // we cannot deliver.
        match w64(mem, out, 10_000_000) { Ok(()) => SUCCESS, Err(e) => e }
    })?;
    linker.func_wrap(NS, "clock_time_get", |mut c: Caller<'_, HS>, id: i32, _p: i64, out: i32| -> i32 {
        // 0 = realtime, everything else treated as monotonic.
        let now = if id == 0 {
            (crate::rtc::read_unix_time().unwrap_or(0) as u64).saturating_mul(1_000_000_000)
        } else {
            crate::interrupts::ticks().saturating_mul(10_000_000)
        };
        let (mem, st) = state!(c);
        let _ = wasi_of!(st);
        match w64(mem, out, now) { Ok(()) => SUCCESS, Err(e) => e }
    })?;
    linker.func_wrap(NS, "random_get", |mut c: Caller<'_, HS>, buf: i32, len: i32| -> i32 {
        // Straight from the kernel CSPRNG. CPython seeds its hash
        // randomisation here, so a predictable stream is a real
        // weakness, not a placeholder detail.
        let l = match usize::try_from(len) { Ok(v) => v, Err(_) => return EFAULT };
        let mut tmp = Vec::with_capacity(l);
        while tmp.len() < l {
            let block = crate::security::csprng::random_256();
            let take = (l - tmp.len()).min(block.len());
            tmp.extend_from_slice(&block[..take]);
        }
        let (mem, st) = state!(c);
        let _ = wasi_of!(st);
        match put_bytes(mem, buf, &tmp) { Ok(()) => SUCCESS, Err(e) => e }
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
        let (mem, st) = state!(c);
        if st.wasi.is_none() { return ENOTCAPABLE; }
        let data = match gather(mem, iovs, n) { Ok(d) => d, Err(e) => return e };
        let written = data.len() as u32;
        // stdout/stderr leave through the same door as npk_print, so a
        // wasi program lands in the terminal the user is looking at.
        let is_std = matches!(
            st.wasi.as_ref().and_then(|w| w.fds.get(&fd)),
            Some(Handle::Stdout) | Some(Handle::Stderr)
        );
        if is_std {
            let s = alloc::string::String::from_utf8_lossy(&data).into_owned();
            crate::wasm::emit_output(st, &s);
            return match w32(mem, out, written) { Ok(()) => SUCCESS, Err(e) => e };
        }
        let w = wasi_of!(st);
        match w.fds.get_mut(&fd) {
            Some(Handle::File { data: buf, pos, dirty, writable, .. }) => {
                if !*writable { return EPERM; }
                if *pos > buf.len() { return EINVAL; }
                let end = pos.saturating_add(data.len());
                if end > buf.len() { buf.resize(end, 0); }
                buf[*pos..end].copy_from_slice(&data);
                *pos = end;
                *dirty = true;
            }
            Some(Handle::Stdin) | Some(Handle::Dir { .. }) => return EBADF,
            Some(_) => return EBADF,
            None => return EBADF,
        }
        match w32(mem, out, written) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "fd_read", |mut c: Caller<'_, HS>, fd: i32, iovs: i32, n: i32, out: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let mut total = 0u32;
        for i in 0..n {
            let base = match iovs.checked_add(i * 8) { Some(v) => v, None => return EFAULT };
            let ptr = match r32(mem, base) { Ok(v) => v as i32, Err(e) => return e };
            let len = match r32(mem, base + 4) { Ok(v) => v as usize, Err(e) => return e };
            if len == 0 { continue; }
            let chunk: Vec<u8> = match w.fds.get_mut(&fd) {
                // Nothing types into a wasi program yet: it gets EOF, not
                // a hang. A REPL will need a real stdin here.
                Some(Handle::Stdin) => Vec::new(),
                Some(Handle::File { data, pos, .. }) => {
                    let end = (*pos + len).min(data.len());
                    let s = data[(*pos).min(data.len())..end].to_vec();
                    *pos = end;
                    s
                }
                Some(Handle::Dir { .. }) => return EISDIR,
                Some(_) => return EBADF,
                None => return EBADF,
            };
            if let Err(e) = put_bytes(mem, ptr, &chunk) { return e; }
            total += chunk.len() as u32;
            if chunk.len() < len { break; }
        }
        match w32(mem, out, total) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "fd_pread", |mut c: Caller<'_, HS>, fd: i32, iovs: i32, n: i32, off: i64, out: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let mut total = 0u32;
        let mut pos = match usize::try_from(off) { Ok(v) => v, Err(_) => return EINVAL };
        for i in 0..n {
            let base = match iovs.checked_add(i * 8) { Some(v) => v, None => return EFAULT };
            let ptr = match r32(mem, base) { Ok(v) => v as i32, Err(e) => return e };
            let len = match r32(mem, base + 4) { Ok(v) => v as usize, Err(e) => return e };
            if len == 0 { continue; }
            // pread must not move the cursor — with the bytes already in
            // hand that is just not touching `pos`.
            let chunk: Vec<u8> = match w.fds.get(&fd) {
                Some(Handle::File { data, .. }) => {
                    let start = pos.min(data.len());
                    let end = (pos + len).min(data.len());
                    data[start..end].to_vec()
                }
                Some(_) => return ESPIPE,
                None => return EBADF,
            };
            if let Err(e) = put_bytes(mem, ptr, &chunk) { return e; }
            total += chunk.len() as u32;
            pos += chunk.len();
            if chunk.len() < len { break; }
        }
        match w32(mem, out, total) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "fd_pwrite", |mut c: Caller<'_, HS>, fd: i32, iovs: i32, n: i32, off: i64, out: i32| -> i32 {
        let (mem, st) = state!(c);
        if st.wasi.is_none() { return ENOTCAPABLE; }
        let data = match gather(mem, iovs, n) { Ok(d) => d, Err(e) => return e };
        let at = match usize::try_from(off) { Ok(v) => v, Err(_) => return EINVAL };
        let w = wasi_of!(st);
        match w.fds.get_mut(&fd) {
            Some(Handle::File { data: buf, dirty, writable, .. }) => {
                if !*writable { return EPERM; }
                let end = at.saturating_add(data.len());
                if end > buf.len() { buf.resize(end, 0); }
                buf[at..end].copy_from_slice(&data);
                *dirty = true;
            }
            Some(_) => return ESPIPE,
            None => return EBADF,
        }
        match w32(mem, out, data.len() as u32) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "fd_seek", |mut c: Caller<'_, HS>, fd: i32, off: i64, whence: i32, out: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let pos = match w.fds.get_mut(&fd) {
            Some(Handle::File { data, pos, .. }) => {
                let base = match whence {
                    0 => 0i64,
                    1 => *pos as i64,
                    2 => data.len() as i64,
                    _ => return EINVAL,
                };
                let np = match base.checked_add(off) { Some(v) => v, None => return EINVAL };
                if np < 0 { return EINVAL; }
                *pos = np as usize;
                *pos as u64
            }
            Some(_) => return ESPIPE,
            None => return EBADF,
        };
        match w64(mem, out, pos) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "fd_tell", |mut c: Caller<'_, HS>, fd: i32, out: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let pos = match w.fds.get(&fd) {
            Some(Handle::File { pos, .. }) => *pos as u64,
            Some(_) => return ESPIPE,
            None => return EBADF,
        };
        match w64(mem, out, pos) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "fd_close", |mut c: Caller<'_, HS>, fd: i32| -> i32 {
        let st = c.data_mut();
        let w = wasi_of!(st);
        match w.fds.remove(&fd) {
            // Whole-file store: the write-back happens here, once, not
            // on every fd_write. A close that fails must say so — a
            // silently dropped buffer is a lost file.
            Some(Handle::File { path, data, dirty: true, .. }) => {
                match fs::write(&path, &data) { Ok(()) => SUCCESS, Err(e) => fs_errno(&e) }
            }
            Some(_) => SUCCESS,
            None => EBADF,
        }
    })?;
    linker.func_wrap(NS, "fd_sync", |mut c: Caller<'_, HS>, _fd: i32| -> i32 {
        let st = c.data_mut(); let _ = wasi_of!(st); SUCCESS
    })?;
    linker.func_wrap(NS, "fd_datasync", |mut c: Caller<'_, HS>, _fd: i32| -> i32 {
        let st = c.data_mut(); let _ = wasi_of!(st); SUCCESS
    })?;
    linker.func_wrap(NS, "fd_advise", |mut c: Caller<'_, HS>, _f: i32, _o: i64, _l: i64, _a: i32| -> i32 {
        let st = c.data_mut(); let _ = wasi_of!(st); SUCCESS
    })?;
    linker.func_wrap(NS, "fd_fdstat_set_flags", |mut c: Caller<'_, HS>, _f: i32, _fl: i32| -> i32 {
        let st = c.data_mut(); let _ = wasi_of!(st); SUCCESS
    })?;

    linker.func_wrap(NS, "fd_fdstat_get", |mut c: Caller<'_, HS>, fd: i32, out: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let (ft, flags) = match w.fds.get(&fd) {
            Some(Handle::Stdin) | Some(Handle::Stdout) | Some(Handle::Stderr) => (FT_CHR, 0u16),
            Some(Handle::Dir { .. }) => (FT_DIR, 0),
            Some(Handle::File { .. }) => (FT_REG, 0),
            None => return EBADF,
        };
        let mut b = [0u8; 24];
        b[0] = ft;
        b[2..4].copy_from_slice(&flags.to_le_bytes());
        b[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        b[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        match put_bytes(mem, out, &b) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "fd_filestat_get", |mut c: Caller<'_, HS>, fd: i32, out: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let b = match w.fds.get(&fd) {
            Some(Handle::File { data, .. }) => filestat_bytes(EntryKind::File, data.len() as u64, 0),
            Some(Handle::Dir { path, .. }) => match fs::stat(path) {
                Ok(Some(s)) => filestat_bytes(s.kind, s.size, s.mtime),
                Ok(None) => return ENOENT,
                Err(e) => return fs_errno(&e),
            },
            Some(_) => filestat_bytes(EntryKind::File, 0, 0),
            None => return EBADF,
        };
        let b = if matches!(w.fds.get(&fd), Some(Handle::Stdin) | Some(Handle::Stdout) | Some(Handle::Stderr)) {
            let mut t = b; t[16] = FT_CHR; t
        } else { b };
        match put_bytes(mem, out, &b) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "fd_prestat_get", |mut c: Caller<'_, HS>, fd: i32, out: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        // wasi-libc walks fds upward until one answers EBADF — that is
        // how it learns where the grant ends. Only preopens answer.
        let name_len = match w.fds.get(&fd) {
            Some(Handle::Dir { guest, preopen: true, .. }) => guest.len() as u32,
            _ => return EBADF,
        };
        let mut b = [0u8; 8];
        b[0] = 0; // dir
        b[4..8].copy_from_slice(&name_len.to_le_bytes());
        match put_bytes(mem, out, &b) { Ok(()) => SUCCESS, Err(e) => e }
    })?;
    linker.func_wrap(NS, "fd_prestat_dir_name", |mut c: Caller<'_, HS>, fd: i32, ptr: i32, len: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let name = match w.fds.get(&fd) {
            Some(Handle::Dir { guest, preopen: true, .. }) => guest.clone(),
            _ => return EBADF,
        };
        let n = (len as usize).min(name.len());
        match put_bytes(mem, ptr, &name.as_bytes()[..n]) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "fd_readdir", |mut c: Caller<'_, HS>, fd: i32, buf: i32, buf_len: i32, cookie: i64, out: i32| -> i32 {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let dir = match w.fds.get(&fd) {
            Some(Handle::Dir { path, .. }) => path.clone(),
            Some(_) => return ENOTDIR,
            None => return EBADF,
        };
        let entries = match fs::list(&dir) {
            Ok(Some(v)) => v,
            Ok(None) => return ENOENT,
            Err(e) => return fs_errno(&e),
        };
        let mut names: Vec<(String, u8)> =
            vec![(".".to_string(), FT_DIR), ("..".to_string(), FT_DIR)];
        for e in entries {
            let ft = match e.kind { EntryKind::Dir => FT_DIR, _ => FT_REG };
            names.push((e.name, ft));
        }
        let cap = match usize::try_from(buf_len) { Ok(v) => v, Err(_) => return EFAULT };
        let skip = match usize::try_from(cookie) { Ok(v) => v, Err(_) => return EINVAL };
        let mut written = 0usize;
        for (i, (name, ft)) in names.iter().enumerate().skip(skip) {
            let mut rec = Vec::with_capacity(24 + name.len());
            rec.extend_from_slice(&((i as u64) + 1).to_le_bytes()); // d_next
            rec.extend_from_slice(&((i as u64) + 1).to_le_bytes()); // d_ino
            rec.extend_from_slice(&(name.len() as u32).to_le_bytes());
            rec.push(*ft);
            rec.extend_from_slice(&[0, 0, 0]);
            rec.extend_from_slice(name.as_bytes());
            // A truncated last record is not an error: the reader sees
            // bufused == buf_len and comes back with a bigger buffer.
            let take = rec.len().min(cap - written);
            let at = match i32::try_from(written).ok().and_then(|o| buf.checked_add(o)) {
                Some(v) => v, None => return EFAULT,
            };
            if let Err(e) = put_bytes(mem, at, &rec[..take]) { return e; }
            written += take;
            if written >= cap { break; }
        }
        match w32(mem, out, written as u32) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "poll_oneoff", |mut c: Caller<'_, HS>, _i: i32, _o: i32, _n: i32, nev: i32| -> i32 {
        let (mem, st) = state!(c);
        let _ = wasi_of!(st);
        // Nothing is ever ready. CPython polls here for stdin and for
        // its signal machinery; neither exists for us yet.
        match w32(mem, nev, 0) { Ok(()) => SUCCESS, Err(e) => e }
    })?;
    Ok(())
}

// ── path-based calls ──────────────────────────────────────────────────

fn link_path(linker: &mut Linker<HS>) -> Result<(), wasmi::Error> {
    const NS: &str = "wasi_snapshot_preview1";

    linker.func_wrap(NS, "path_open", |mut c: Caller<'_, HS>,
        dirfd: i32, _dirflags: i32, path: i32, path_len: i32,
        oflags: i32, rights: i64, _rights_inh: i64, _fdflags: i32, out: i32| -> i32
    {
        let (mem, st) = state!(c);
        let cap = st.cap_id;
        let w = wasi_of!(st);
        let rel = match guest_str(mem, path, path_len) { Ok(s) => s, Err(e) => return e };
        let (full, root, grant_writable) = match w.resolve(dirfd, &rel) {
            Ok(v) => v, Err(e) => return e,
        };

        let want_write = rights & RIGHT_FD_WRITE != 0 || oflags & (O_CREAT | O_TRUNC) != 0;
        if want_write {
            // Two gates, both required: the grant this path came through
            // has to allow writing, and the capability the run carries
            // has to include WRITE. Either alone would be a hole.
            if !grant_writable { return EPERM; }
            if capability::check_global(&cap, Rights::WRITE).is_err() { return EPERM; }
        }

        let existing = match fs::stat(&full) { Ok(v) => v, Err(e) => return fs_errno(&e) };
        if let Some(st_) = &existing {
            if st_.kind == EntryKind::Dir {
                // A subdirectory inherits the grant it was reached
                // through — that is what keeps two preopens from
                // becoming a path into one another.
                let fd = w.insert(Handle::Dir {
                    path: full, guest: rel, preopen: false, root, writable: grant_writable,
                });
                return match w32(mem, out, fd as u32) { Ok(()) => SUCCESS, Err(e) => e };
            }
            if oflags & O_EXCL != 0 { return EEXIST; }
        } else {
            if oflags & O_DIRECTORY != 0 { return ENOENT; }
            if oflags & O_CREAT == 0 { return ENOENT; }
        }

        let data = if existing.is_some() && oflags & O_TRUNC == 0 {
            match fs::read(&full) {
                Ok(Some(d)) => d,
                Ok(None) => Vec::new(),
                Err(e) => return fs_errno(&e),
            }
        } else {
            Vec::new()
        };
        // A file created here does not exist in npkFS until close writes
        // it back, so it starts dirty — otherwise `open(w); close()`
        // would leave nothing behind.
        let fresh = existing.is_none() || oflags & O_TRUNC != 0;
        let fd = w.insert(Handle::File {
            path: full, data, pos: 0, dirty: fresh && want_write, writable: want_write,
        });
        match w32(mem, out, fd as u32) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "path_filestat_get", |mut c: Caller<'_, HS>,
        dirfd: i32, _flags: i32, path: i32, path_len: i32, out: i32| -> i32
    {
        let (mem, st) = state!(c);
        let w = wasi_of!(st);
        let rel = match guest_str(mem, path, path_len) { Ok(s) => s, Err(e) => return e };
        let (full, _, _) = match w.resolve(dirfd, &rel) { Ok(v) => v, Err(e) => return e };
        let b = match fs::stat(&full) {
            Ok(Some(s)) => filestat_bytes(s.kind, s.size, s.mtime),
            Ok(None) => return ENOENT,
            Err(e) => return fs_errno(&e),
        };
        match put_bytes(mem, out, &b) { Ok(()) => SUCCESS, Err(e) => e }
    })?;

    linker.func_wrap(NS, "path_create_directory", |mut c: Caller<'_, HS>, dirfd: i32, p: i32, pl: i32| -> i32 {
        let (mem, st) = state!(c);
        let cap = st.cap_id;
        let w = wasi_of!(st);
        let rel = match guest_str(mem, p, pl) { Ok(s) => s, Err(e) => return e };
        let (full, _, writable) = match w.resolve(dirfd, &rel) { Ok(v) => v, Err(e) => return e };
        if !writable || capability::check_global(&cap, Rights::WRITE).is_err() { return EPERM; }
        match fs::mkdir(&full) { Ok(()) => SUCCESS, Err(e) => fs_errno(&e) }
    })?;

    linker.func_wrap(NS, "path_remove_directory", |mut c: Caller<'_, HS>, dirfd: i32, p: i32, pl: i32| -> i32 {
        let (mem, st) = state!(c);
        let cap = st.cap_id;
        let w = wasi_of!(st);
        let rel = match guest_str(mem, p, pl) { Ok(s) => s, Err(e) => return e };
        let (full, _, writable) = match w.resolve(dirfd, &rel) { Ok(v) => v, Err(e) => return e };
        if !writable || capability::check_global(&cap, Rights::WRITE).is_err() { return EPERM; }
        match fs::list(&full) {
            Ok(Some(v)) if !v.is_empty() => return ENOTEMPTY,
            Ok(Some(_)) => {}
            Ok(None) => return ENOENT,
            Err(e) => return fs_errno(&e),
        }
        match fs::delete(&full) { Ok(()) => SUCCESS, Err(e) => fs_errno(&e) }
    })?;

    linker.func_wrap(NS, "path_unlink_file", |mut c: Caller<'_, HS>, dirfd: i32, p: i32, pl: i32| -> i32 {
        let (mem, st) = state!(c);
        let cap = st.cap_id;
        let w = wasi_of!(st);
        let rel = match guest_str(mem, p, pl) { Ok(s) => s, Err(e) => return e };
        let (full, _, writable) = match w.resolve(dirfd, &rel) { Ok(v) => v, Err(e) => return e };
        if !writable || capability::check_global(&cap, Rights::WRITE).is_err() { return EPERM; }
        match fs::delete(&full) { Ok(()) => SUCCESS, Err(e) => fs_errno(&e) }
    })?;

    linker.func_wrap(NS, "path_rename", |mut c: Caller<'_, HS>,
        ofd: i32, op: i32, ol: i32, nfd: i32, np: i32, nl: i32| -> i32
    {
        let (mem, st) = state!(c);
        let cap = st.cap_id;
        let w = wasi_of!(st);
        let a = match guest_str(mem, op, ol) { Ok(s) => s, Err(e) => return e };
        let b = match guest_str(mem, np, nl) { Ok(s) => s, Err(e) => return e };
        let (from, _, wa) = match w.resolve(ofd, &a) { Ok(v) => v, Err(e) => return e };
        let (to, _, wb) = match w.resolve(nfd, &b) { Ok(v) => v, Err(e) => return e };
        // Both ends must be writable: renaming out of a read-only grant
        // is a delete, renaming into one is a write.
        if !wa || !wb || capability::check_global(&cap, Rights::WRITE).is_err() { return EPERM; }
        match fs::rename(&from, &to) { Ok(()) => SUCCESS, Err(e) => fs_errno(&e) }
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
    linker.func_wrap(NS, "fd_filestat_set_size", |_c: Caller<'_, HS>, _: i32, _: i64| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "fd_filestat_set_times", |_c: Caller<'_, HS>, _: i32, _: i64, _: i64, _: i32| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "path_filestat_set_times", |_c: Caller<'_, HS>, _: i32, _: i32, _: i32, _: i32, _: i64, _: i64, _: i32| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "path_link", |_c: Caller<'_, HS>, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "path_symlink", |_c: Caller<'_, HS>, _: i32, _: i32, _: i32, _: i32, _: i32| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "path_readlink", |_c: Caller<'_, HS>, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "fd_renumber", |_c: Caller<'_, HS>, _: i32, _: i32| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "sock_accept", |_c: Caller<'_, HS>, _: i32, _: i32, _: i32| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "sock_recv", |_c: Caller<'_, HS>, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "sock_send", |_c: Caller<'_, HS>, _: i32, _: i32, _: i32, _: i32, _: i32| -> i32 { ENOSYS })?;
    linker.func_wrap(NS, "sock_shutdown", |_c: Caller<'_, HS>, _: i32, _: i32| -> i32 { ENOSYS })?;
    Ok(())
}

/// Build a context. Callers add their preopens with `preopen` — there
/// is no default grant, because "what can this program see" should be a
/// decision somebody wrote down, not a fallback.
pub fn ctx(args: Vec<String>, env: Vec<String>) -> Box<WasiCtx> {
    Box::new(WasiCtx::new(args, env))
}
