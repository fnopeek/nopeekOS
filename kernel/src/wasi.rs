//! `wasi_snapshot_preview1` — the second ABI.
//!
//! Everything else in this kernel talks `npk_*`, designed for capabilities
//! instead of permissions and content addresses instead of a path tree.
//! This module is the one place that speaks somebody else's language,
//! because programs worth borrowing (CPython, lua, sqlite) are written
//! against POSIX and compiled through wasi-libc.
//!
//! It is deliberately not a Python feature. A guest that gets this
//! namespace gets a filesystem-shaped view of one npkFS subtree it was
//! handed, and nothing else. Every wasi binary lands the same way, under
//! the same grant, with the same ceiling.
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

use wasmi::{Caller, Linker};

use crate::capability::{self, Rights};
use crate::storage::npkfs::fs;
use crate::storage::npkfs::object::EntryKind;

// ── errno (preview1 numbering — do not renumber) ──────────────────────
pub const SUCCESS: i32 = 0;
const EBADF: i32 = 8;
const EEXIST: i32 = 20;
const EFAULT: i32 = 21;
const EFBIG: i32 = 22;
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
    /// The program's exit status. The trap path only says that it exited;
    /// the status lives here because both engines record it the same way.
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
/// buggy or hostile guest becomes a huge usize under `as`.
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

/// What `proc_exit` leaves behind; written the same way by both engines.
pub(crate) fn record_exit(st: &mut HS, code: i32) {
    if let Some(w) = st.wasi.as_mut() {
        w.exit_status = Some(code);
    }
}

/// And read back. `None` means the program returned from `_start` without
/// calling `proc_exit`.
pub fn exit_status(st: &HS) -> Option<i32> {
    st.wasi.as_ref().and_then(|w| w.exit_status)
}

pub fn link(linker: &mut Linker<HS>) -> Result<(), wasmi::Error> {
    const NS: &str = "wasi_snapshot_preview1";

    // ── process ───────────────────────────────────────────────────────
    // The one thing that really differs between engines. Recording the
    // status is shared; leaving is not: the interpreter returns an `Err` and
    // unwinds, generated code takes `forge_rt::host_trap`. Both end in the
    // same state.
    linker.func_wrap(NS, "proc_exit",
        |mut c: Caller<'_, HS>, code: i32| -> Result<(), wasmi::Error> {
            record_exit(c.data_mut(), code);
            Err(wasmi::Error::i32_exit(code))
        })?;

    forge_glue::register_wasmi(linker)
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

/// Bytes one write call takes in. Every iovec may point at the same large
/// region, so without a ceiling a short list of them asks the kernel for
/// terabytes. A write may be partial: the caller is told how much went in
/// and writes the rest.
const MAX_WRITE_CALL: usize = 64 * 1024 * 1024;
/// Largest file a wasi program may grow by positioned writes; the whole file
/// is held in kernel memory until it is synced.
const MAX_WASI_FILE: usize = 256 * 1024 * 1024;

/// Gather the `iovec` array into one buffer. Copying first keeps the
/// borrow of guest memory apart from the borrow of the fd table.
fn gather(mem: &[u8], iovs: i32, iovs_len: i32) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    for i in 0..iovs_len {
        let base = iovs.checked_add(i.checked_mul(8).ok_or(EFAULT)?).ok_or(EFAULT)?;
        let ptr = i32::try_from(r32(mem, base)?).map_err(|_| EFAULT)?;
        let len = i32::try_from(r32(mem, base + 4)?).map_err(|_| EFAULT)?;
        let b = bytes_at(mem, ptr, len)?;
        let room = MAX_WRITE_CALL - out.len();
        out.extend_from_slice(&b[..b.len().min(room)]);
        if out.len() == MAX_WRITE_CALL { break; }
    }
    Ok(out)
}


/// Build a context. Callers add their preopens with `preopen` — there
/// is no default grant, because "what can this program see" should be a
/// decision somebody wrote down, not a fallback.
pub fn ctx(args: Vec<String>, env: Vec<String>) -> Box<WasiCtx> {
    Box::new(WasiCtx::new(args, env))
}
