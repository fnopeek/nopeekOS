//! WASI preview1, lifted out of `../tools/pywasi` so BOTH engines can use the
//! SAME implementation. Comparing two compilers through two different host
//! layers would measure the host layers.
//!
//! Every function here works on `(guest memory, context)` and nothing else —
//! that was already true in pywasi, which is why the bodies transfer
//! unchanged. What differs per engine is only how those two are obtained.
#![allow(dead_code)]

// `wasi_snapshot_preview1` on wasmi — host-side prototype of the shim
// nopeekOS would need in `kernel/src/wasm.rs`.
//
// Everything that touches the outside world goes through `Fs` at the
// bottom of this file. On the host that is `std::fs`; in the kernel it
// becomes npkFS, and nothing above it changes.
//
// Capability shape, kept deliberately: the guest can only reach paths
// below a preopened directory it was handed. There is no absolute-path
// escape hatch and no `..` climb above the root — a WASI program does
// not get the machine, it gets the subtree it was granted.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};


/// Where a finishing run reports how long it took. A wasi program leaves
/// through `proc_exit`, so the time has to be taken on the way out.
thread_local! {
    static REPORT: std::cell::Cell<(i32, Option<std::time::Instant>)> =
        const { std::cell::Cell::new((-1, None)) };
}

/// Arm the report before entering the module.
pub fn arm_report(fd: i32, start: std::time::Instant) {
    REPORT.with(|r| r.set((fd, Some(start))));
}

/// Send the run's time, its exit status and everything it wrote to stdout back
/// to the parent. Both engines leave through here, so both are measured the
/// same way and their output can be compared byte for byte.
pub fn report(ctx: &WasiCtx, code: i32) {
    let (fd, start) = REPORT.with(|r| r.get());
    if fd < 0 {
        return;
    }
    let ms = start.map(|t| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0);
    let mut buf = Vec::with_capacity(16 + ctx.stdout.len());
    buf.extend_from_slice(&ms.to_le_bytes());
    buf.extend_from_slice(&code.to_le_bytes());
    buf.extend_from_slice(&(ctx.stdout.len() as u32).to_le_bytes());
    buf.extend_from_slice(&ctx.stdout);
    // SAFETY: `fd` is a pipe this process opened; `buf` is a live allocation.
    unsafe {
        libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len());
    }
}

// ── errno ─────────────────────────────────────────────────────────────
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
const ESPIPE: i32 = 70;
const ENOTCAPABLE: i32 = 76;

// ── filetype ──────────────────────────────────────────────────────────
const FT_DIR: u8 = 3;
const FT_REG: u8 = 4;
const FT_CHR: u8 = 2;

// ── oflags / fdflags / rights ─────────────────────────────────────────
const O_CREAT: i32 = 1;
const O_DIRECTORY: i32 = 2;
const O_EXCL: i32 = 4;
const O_TRUNC: i32 = 8;
const FD_APPEND: i32 = 1;
const RIGHT_FD_WRITE: i64 = 1 << 6;

pub enum Handle {
    Stdin,
    Stdout,
    Stderr,
    File { f: std::fs::File, path: PathBuf, append: bool },
    /// `preopen` is set for the fds announced through `fd_prestat_get`.
    Dir { path: PathBuf, guest: String, preopen: bool },
}

pub struct WasiCtx {
    fds: BTreeMap<i32, Handle>,
    next_fd: i32,
    /// Host directory the guest sees as its root.
    root: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<String>,
    /// Bytes written to fd 1 and 2, so a measurement run can stay quiet
    /// and still prove the program produced the right output.
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Counts every host call, per function — this is the number that
    /// tells us which preview1 calls the kernel shim must be fast at.
    pub calls: BTreeMap<&'static str, u64>,
    pub echo: bool,
}

impl WasiCtx {
    pub fn new(root: PathBuf, guest_root: &str, args: Vec<String>, env: Vec<String>) -> Self {
        let mut fds = BTreeMap::new();
        fds.insert(0, Handle::Stdin);
        fds.insert(1, Handle::Stdout);
        fds.insert(2, Handle::Stderr);
        fds.insert(3, Handle::Dir {
            path: root.clone(),
            guest: guest_root.to_string(),
            preopen: true,
        });
        WasiCtx {
            fds, next_fd: 4, root, args, env,
            stdout: Vec::new(), stderr: Vec::new(),
            calls: BTreeMap::new(), echo: false,
        }
    }

    fn tick(&mut self, name: &'static str) {
        *self.calls.entry(name).or_insert(0) += 1;
    }

    fn insert(&mut self, h: Handle) -> i32 {
        let fd = self.next_fd;
        self.next_fd += 1;
        self.fds.insert(fd, h);
        fd
    }

    /// Resolve a guest-relative path under `dir_fd`, refusing anything
    /// that would leave the granted subtree. This is the whole security
    /// boundary of the shim — it has to be the boring, obvious code.
    fn resolve(&self, dir_fd: i32, rel: &str) -> Result<PathBuf, i32> {
        let base = match self.fds.get(&dir_fd) {
            Some(Handle::Dir { path, .. }) => path.clone(),
            Some(_) => return Err(ENOTDIR),
            None => return Err(EBADF),
        };
        let mut out = base;
        for c in Path::new(rel).components() {
            match c {
                Component::Normal(p) => out.push(p),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !out.pop() { return Err(ENOTCAPABLE); }
                }
                // An absolute path or a prefix would step outside the
                // grant by construction.
                Component::RootDir | Component::Prefix(_) => return Err(ENOTCAPABLE),
            }
        }
        if !out.starts_with(&self.root) { return Err(ENOTCAPABLE); }
        Ok(out)
    }
}

fn errno_of(e: &std::io::Error) -> i32 {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => ENOENT,
        AlreadyExists => EEXIST,
        PermissionDenied => ENOTCAPABLE,
        InvalidInput => EINVAL,
        IsADirectory => EISDIR,
        NotADirectory => ENOTDIR,
        _ => EIO,
    }
}

// ── little-endian pokes into guest memory ─────────────────────────────
fn w32(m: &mut [u8], at: i32, v: u32) -> Result<(), i32> {
    let a = at as usize;
    let s = m.get_mut(a..a + 4).ok_or(EFAULT)?;
    s.copy_from_slice(&v.to_le_bytes());
    Ok(())
}
fn w64(m: &mut [u8], at: i32, v: u64) -> Result<(), i32> {
    let a = at as usize;
    let s = m.get_mut(a..a + 8).ok_or(EFAULT)?;
    s.copy_from_slice(&v.to_le_bytes());
    Ok(())
}
fn r32(m: &[u8], at: i32) -> Result<u32, i32> {
    let a = at as usize;
    let s = m.get(a..a + 4).ok_or(EFAULT)?;
    Ok(u32::from_le_bytes(s.try_into().unwrap()))
}
fn slice<'a>(m: &'a [u8], ptr: i32, len: i32) -> Result<&'a [u8], i32> {
    let (a, l) = (ptr as usize, len as usize);
    m.get(a..a + l).ok_or(EFAULT)
}
fn guest_str(m: &[u8], ptr: i32, len: i32) -> Result<String, i32> {
    let b = slice(m, ptr, len)?;
    String::from_utf8(b.to_vec()).map_err(|_| EINVAL)
}

fn filestat_bytes(md: &std::fs::Metadata) -> [u8; 64] {
    let mut b = [0u8; 64];
    let ft = if md.is_dir() { FT_DIR } else { FT_REG };
    b[16] = ft;
    b[24..32].copy_from_slice(&1u64.to_le_bytes());          // nlink
    b[32..40].copy_from_slice(&md.len().to_le_bytes());
    let ns = |t: std::io::Result<SystemTime>| -> u64 {
        t.ok().and_then(|x| x.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64).unwrap_or(0)
    };
    b[40..48].copy_from_slice(&ns(md.accessed()).to_le_bytes());
    b[48..56].copy_from_slice(&ns(md.modified()).to_le_bytes());
    b[56..64].copy_from_slice(&ns(md.modified()).to_le_bytes());
    b
}

// ── the 42 imports ────────────────────────────────────────────────────
//
// python.wasm imports exactly these, all from one namespace. Sixteen of
// them are stubs that return ENOSYS and CPython copes — sockets,
// symlinks, hard links. The ones that carry the startup are path_open,
// fd_read, fd_seek, fd_readdir and fd_filestat_get.


#[allow(unused_variables, unused_mut)]
pub fn proc_exit(mem: &mut [u8], ctx: &mut WasiCtx, code: i32) -> i32 {
    ctx.tick("proc_exit");
    // A wasi program finishes by trapping out of here, clean run included.
    // The interpreter can turn that into an `Err` and unwind; generated code
    // has no trap path yet, so the run reports its own time on the way out and
    // leaves. Both engines take this same exit, which is what keeps the two
    // measurements comparable.
    report(ctx, code);
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: ends this process without unwinding, after the buffers are out.
    unsafe { libc::_exit(code) }
}

#[allow(unused_variables, unused_mut)]
pub fn sched_yield(mem: &mut [u8], ctx: &mut WasiCtx) -> i32 {
        ctx.tick("sched_yield");
        SUCCESS
    }

#[allow(unused_variables, unused_mut)]
pub fn args_sizes_get(mem: &mut [u8], ctx: &mut WasiCtx, n: i32, sz: i32) -> i32 {
        ctx.tick("args_sizes_get");
        
        let count = ctx.args.len() as u32;
        let bytes: u32 = ctx.args.iter().map(|a| a.len() as u32 + 1).sum();
        if let Err(e) = w32(mem, n, count) { return e; }
        if let Err(e) = w32(mem, sz, bytes) { return e; }
        SUCCESS
    }

#[allow(unused_variables, unused_mut)]
pub fn args_get(mem: &mut [u8], ctx: &mut WasiCtx, argv: i32, buf: i32) -> i32 {
        ctx.tick("args_get");
        
        write_string_vec(mem, argv, buf, &ctx.args)
    }

#[allow(unused_variables, unused_mut)]
pub fn environ_sizes_get(mem: &mut [u8], ctx: &mut WasiCtx, n: i32, sz: i32) -> i32 {
        ctx.tick("environ_sizes_get");
        
        let count = ctx.env.len() as u32;
        let bytes: u32 = ctx.env.iter().map(|a| a.len() as u32 + 1).sum();
        if let Err(e) = w32(mem, n, count) { return e; }
        if let Err(e) = w32(mem, sz, bytes) { return e; }
        SUCCESS
    }

#[allow(unused_variables, unused_mut)]
pub fn environ_get(mem: &mut [u8], ctx: &mut WasiCtx, ep: i32, buf: i32) -> i32 {
        ctx.tick("environ_get");
        
        write_string_vec(mem, ep, buf, &ctx.env)
    }

#[allow(unused_variables, unused_mut)]
pub fn clock_res_get(mem: &mut [u8], ctx: &mut WasiCtx, _id: i32, out: i32) -> i32 {
        ctx.tick("clock_res_get");
        match w64(mem, out, 1_000) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn clock_time_get(mem: &mut [u8], ctx: &mut WasiCtx, _id: i32, _prec: i64, out: i32) -> i32 {
        ctx.tick("clock_time_get");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
        match w64(mem, out, now) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn random_get(mem: &mut [u8], ctx: &mut WasiCtx, buf: i32, len: i32) -> i32 {
        ctx.tick("random_get");
        let (a, l) = (buf as usize, len as usize);
        let dst = match mem.get_mut(a..a + l) { Some(d) => d, None => return EFAULT };
        // Measurement harness: a counter, not entropy. The kernel shim
        // must wire this to csprng — CPython seeds its hash randomisation
        // from here, and a predictable seed is a real weakness.
        let mut x: u64 = 0x9E3779B97F4A7C15;
        for b in dst.iter_mut() {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            *b = (x >> 24) as u8;
        }
        SUCCESS
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_fdstat_get(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, out: i32) -> i32 {
        ctx.tick("fd_fdstat_get");
        
        let (ft, flags) = match ctx.fds.get(&fd) {
            Some(Handle::Stdin) | Some(Handle::Stdout) | Some(Handle::Stderr) => (FT_CHR, 0u16),
            Some(Handle::Dir { .. }) => (FT_DIR, 0),
            Some(Handle::File { append, .. }) => (FT_REG, if *append { 1 } else { 0 }),
            None => return EBADF,
        };
        let mut b = [0u8; 24];
        b[0] = ft;
        b[2..4].copy_from_slice(&flags.to_le_bytes());
        b[8..16].copy_from_slice(&u64::MAX.to_le_bytes());   // rights_base
        b[16..24].copy_from_slice(&u64::MAX.to_le_bytes());  // rights_inheriting
        let a = out as usize;
        match mem.get_mut(a..a + 24) {
            Some(d) => { d.copy_from_slice(&b); SUCCESS }
            None => EFAULT,
        }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_fdstat_set_flags(mem: &mut [u8], ctx: &mut WasiCtx, _fd: i32, _f: i32) -> i32 {
        ctx.tick("fd_fdstat_set_flags");
        SUCCESS
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_filestat_get(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, out: i32) -> i32 {
        ctx.tick("fd_filestat_get");
        
        let md = match ctx.fds.get(&fd) {
            Some(Handle::File { f, .. }) => f.metadata(),
            Some(Handle::Dir { path, .. }) => std::fs::metadata(path),
            Some(_) => {
                let mut b = [0u8; 64];
                b[16] = FT_CHR;
                let a = out as usize;
                return match mem.get_mut(a..a + 64) {
                    Some(d) => { d.copy_from_slice(&b); SUCCESS }
                    None => EFAULT,
                };
            }
            None => return EBADF,
        };
        let md = match md { Ok(m) => m, Err(e) => return errno_of(&e) };
        let b = filestat_bytes(&md);
        let a = out as usize;
        match mem.get_mut(a..a + 64) {
            Some(d) => { d.copy_from_slice(&b); SUCCESS }
            None => EFAULT,
        }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_prestat_get(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, out: i32) -> i32 {
        ctx.tick("fd_prestat_get");
        
        // Only preopened dirs answer. The libc walks fds upward until it
        // gets EBADF — that is how it learns where the grant ends.
        let name_len = match ctx.fds.get(&fd) {
            Some(Handle::Dir { guest, preopen: true, .. }) => guest.len() as u32,
            _ => return EBADF,
        };
        let mut b = [0u8; 8];
        b[0] = 0; // dir
        b[4..8].copy_from_slice(&name_len.to_le_bytes());
        let a = out as usize;
        match mem.get_mut(a..a + 8) {
            Some(d) => { d.copy_from_slice(&b); SUCCESS }
            None => EFAULT,
        }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_prestat_dir_name(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, ptr: i32, len: i32) -> i32 {
        ctx.tick("fd_prestat_dir_name");
        
        let name = match ctx.fds.get(&fd) {
            Some(Handle::Dir { guest, preopen: true, .. }) => guest.clone(),
            _ => return EBADF,
        };
        let n = (len as usize).min(name.len());
        let a = ptr as usize;
        match mem.get_mut(a..a + n) {
            Some(d) => { d.copy_from_slice(&name.as_bytes()[..n]); SUCCESS }
            None => EFAULT,
        }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_write(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, iovs: i32, n: i32, out: i32) -> i32 {
        ctx.tick("fd_write");
        
        let data = match gather(mem, iovs, n) { Ok(d) => d, Err(e) => return e };
        let written = data.len() as u32;
        match ctx.fds.get_mut(&fd) {
            Some(Handle::Stdout) => {
                if ctx.echo { let _ = std::io::stdout().write_all(&data); }
                ctx.stdout.extend_from_slice(&data);
            }
            Some(Handle::Stderr) => {
                if ctx.echo { let _ = std::io::stderr().write_all(&data); }
                ctx.stderr.extend_from_slice(&data);
            }
            Some(Handle::File { f, .. }) => {
                if let Err(e) = f.write_all(&data) { return errno_of(&e); }
            }
            Some(_) => return EBADF,
            None => return EBADF,
        }
        match w32(mem, out, written) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_read(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, iovs: i32, n: i32, out: i32) -> i32 {
        ctx.tick("fd_read");
        
        // Read into a staging buffer, then scatter — reading straight into
        // guest memory would need two mutable borrows of `mem` at once.
        let mut total = 0u32;
        for i in 0..n {
            let base = iovs + i * 8;
            let ptr = match r32(mem, base) { Ok(v) => v as i32, Err(e) => return e };
            let len = match r32(mem, base + 4) { Ok(v) => v as usize, Err(e) => return e };
            if len == 0 { continue; }
            let mut tmp = vec![0u8; len];
            let got = match ctx.fds.get_mut(&fd) {
                Some(Handle::Stdin) => 0usize,
                Some(Handle::File { f, .. }) => match f.read(&mut tmp) {
                    Ok(g) => g,
                    Err(e) => return errno_of(&e),
                },
                Some(Handle::Dir { .. }) => return EISDIR,
                Some(_) => return EBADF,
                None => return EBADF,
            };
            let a = ptr as usize;
            match mem.get_mut(a..a + got) {
                Some(d) => d.copy_from_slice(&tmp[..got]),
                None => return EFAULT,
            }
            total += got as u32;
            if got < len { break; }
        }
        match w32(mem, out, total) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_pread(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32 {
        ctx.tick("fd_pread");
        
        let mut total = 0u32;
        let mut pos = off as u64;
        for i in 0..n {
            let base = iovs + i * 8;
            let ptr = match r32(mem, base) { Ok(v) => v as i32, Err(e) => return e };
            let len = match r32(mem, base + 4) { Ok(v) => v as usize, Err(e) => return e };
            if len == 0 { continue; }
            let mut tmp = vec![0u8; len];
            let got = match ctx.fds.get_mut(&fd) {
                Some(Handle::File { f, .. }) => {
                    // Save and restore: pread must not move the cursor.
                    let save = match f.stream_position() { Ok(p) => p, Err(e) => return errno_of(&e) };
                    if let Err(e) = f.seek(SeekFrom::Start(pos)) { return errno_of(&e); }
                    let g = match f.read(&mut tmp) { Ok(g) => g, Err(e) => return errno_of(&e) };
                    if let Err(e) = f.seek(SeekFrom::Start(save)) { return errno_of(&e); }
                    g
                }
                Some(_) => return ESPIPE,
                None => return EBADF,
            };
            let a = ptr as usize;
            match mem.get_mut(a..a + got) {
                Some(d) => d.copy_from_slice(&tmp[..got]),
                None => return EFAULT,
            }
            total += got as u32;
            pos += got as u64;
            if got < len { break; }
        }
        match w32(mem, out, total) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_pwrite(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32 {
        ctx.tick("fd_pwrite");
        
        let data = match gather(mem, iovs, n) { Ok(d) => d, Err(e) => return e };
        match ctx.fds.get_mut(&fd) {
            Some(Handle::File { f, .. }) => {
                let save = match f.stream_position() { Ok(p) => p, Err(e) => return errno_of(&e) };
                if let Err(e) = f.seek(SeekFrom::Start(off as u64)) { return errno_of(&e); }
                if let Err(e) = f.write_all(&data) { return errno_of(&e); }
                if let Err(e) = f.seek(SeekFrom::Start(save)) { return errno_of(&e); }
            }
            Some(_) => return ESPIPE,
            None => return EBADF,
        }
        match w32(mem, out, data.len() as u32) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_seek(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, off: i64, whence: i32, out: i32) -> i32 {
        ctx.tick("fd_seek");
        
        let pos = match ctx.fds.get_mut(&fd) {
            Some(Handle::File { f, .. }) => {
                let sf = match whence {
                    0 => SeekFrom::Start(off as u64),
                    1 => SeekFrom::Current(off),
                    2 => SeekFrom::End(off),
                    _ => return EINVAL,
                };
                match f.seek(sf) { Ok(p) => p, Err(e) => return errno_of(&e) }
            }
            Some(_) => return ESPIPE,
            None => return EBADF,
        };
        match w64(mem, out, pos) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_tell(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, out: i32) -> i32 {
        ctx.tick("fd_tell");
        
        let pos = match ctx.fds.get_mut(&fd) {
            Some(Handle::File { f, .. }) => match f.stream_position() {
                Ok(p) => p,
                Err(e) => return errno_of(&e),
            },
            Some(_) => return ESPIPE,
            None => return EBADF,
        };
        match w64(mem, out, pos) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_close(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32) -> i32 {
        ctx.tick("fd_close");
        match ctx.fds.remove(&fd) { Some(_) => SUCCESS, None => EBADF }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_sync(mem: &mut [u8], ctx: &mut WasiCtx, _fd: i32) -> i32 {
        ctx.tick("fd_sync"); SUCCESS
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_datasync(mem: &mut [u8], ctx: &mut WasiCtx, _fd: i32) -> i32 {
        ctx.tick("fd_datasync"); SUCCESS
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_advise(mem: &mut [u8], ctx: &mut WasiCtx, _fd: i32, _o: i64, _l: i64, _a: i32) -> i32 {
        ctx.tick("fd_advise"); SUCCESS
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_readdir(mem: &mut [u8], ctx: &mut WasiCtx, fd: i32, buf: i32, buf_len: i32, cookie: i64, out: i32) -> i32 {
        ctx.tick("fd_readdir");
        
        let dir = match ctx.fds.get(&fd) {
            Some(Handle::Dir { path, .. }) => path.clone(),
            Some(_) => return ENOTDIR,
            None => return EBADF,
        };
        // `.` and `..` come first so the cookie space matches what a
        // POSIX reader expects to walk.
        let mut names: Vec<(String, u8)> = vec![(".".into(), FT_DIR), ("..".into(), FT_DIR)];
        let rd = match std::fs::read_dir(&dir) { Ok(r) => r, Err(e) => return errno_of(&e) };
        for ent in rd.flatten() {
            let ft = match ent.file_type() {
                Ok(t) if t.is_dir() => FT_DIR,
                Ok(_) => FT_REG,
                Err(_) => continue,
            };
            names.push((ent.file_name().to_string_lossy().into_owned(), ft));
        }
        let mut written = 0usize;
        let cap = buf_len as usize;
        for (i, (name, ft)) in names.iter().enumerate().skip(cookie as usize) {
            let mut rec = Vec::with_capacity(24 + name.len());
            rec.extend_from_slice(&((i as u64) + 1).to_le_bytes());   // d_next
            rec.extend_from_slice(&((i as u64) + 1).to_le_bytes());   // d_ino
            rec.extend_from_slice(&(name.len() as u32).to_le_bytes());
            rec.push(*ft);
            rec.extend_from_slice(&[0, 0, 0]);
            rec.extend_from_slice(name.as_bytes());
            // A truncated final record is not an error: the reader sees
            // bufused == buf_len and comes back with a bigger buffer.
            let take = rec.len().min(cap - written);
            let a = buf as usize + written;
            match mem.get_mut(a..a + take) {
                Some(d) => d.copy_from_slice(&rec[..take]),
                None => return EFAULT,
            }
            written += take;
            if written >= cap { break; }
        }
        match w32(mem, out, written as u32) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn poll_oneoff(mem: &mut [u8], ctx: &mut WasiCtx, _in: i32, _out: i32, _n: i32, nev: i32) -> i32 {
        ctx.tick("poll_oneoff");
        // Nothing is ever ready. CPython only polls here for stdin and
        // for its signal machinery, neither of which exists for us.
        match w32(mem, nev, 0) { Ok(()) => SUCCESS, Err(e) => e }
    }

#[allow(unused_variables, unused_mut)]
pub fn path_open(mem: &mut [u8], ctx: &mut WasiCtx, dirfd: i32, _dirflags: i32, path: i32, path_len: i32, oflags: i32, rights: i64, _rights_inh: i64, fdflags: i32, out: i32) -> i32 {
        ctx.tick("path_open");
        
        let rel = match guest_str(mem, path, path_len) { Ok(s) => s, Err(e) => return e };
        let full = match ctx.resolve(dirfd, &rel) { Ok(p) => p, Err(e) => return e };

        if oflags & O_DIRECTORY != 0 {
            return match std::fs::metadata(&full) {
                Ok(md) if md.is_dir() => {
                    let fd = ctx.insert(Handle::Dir { path: full, guest: rel, preopen: false });
                    match w32(mem, out, fd as u32) { Ok(()) => SUCCESS, Err(e) => e }
                }
                Ok(_) => ENOTDIR,
                Err(e) => errno_of(&e),
            };
        }
        // A plain open of a directory still has to succeed — CPython
        // stats through opened dir handles during import.
        if let Ok(md) = std::fs::metadata(&full) {
            if md.is_dir() {
                let fd = ctx.insert(Handle::Dir { path: full, guest: rel, preopen: false });
                return match w32(mem, out, fd as u32) { Ok(()) => SUCCESS, Err(e) => e };
            }
        }
        let want_write = rights & RIGHT_FD_WRITE != 0;
        let append = fdflags & FD_APPEND != 0;
        let mut o = std::fs::OpenOptions::new();
        o.read(true);
        if want_write {
            o.write(true);
            if oflags & O_CREAT != 0 { o.create(true); }
            if oflags & O_EXCL != 0 { o.create_new(true); }
            if oflags & O_TRUNC != 0 { o.truncate(true); }
            if append { o.append(true); }
        }
        match o.open(&full) {
            Ok(f) => {
                let fd = ctx.insert(Handle::File { f, path: full, append });
                match w32(mem, out, fd as u32) { Ok(()) => SUCCESS, Err(e) => e }
            }
            Err(e) => errno_of(&e),
        }
    }

#[allow(unused_variables, unused_mut)]
pub fn path_filestat_get(mem: &mut [u8], ctx: &mut WasiCtx, dirfd: i32, _flags: i32, path: i32, path_len: i32, out: i32) -> i32 {
        ctx.tick("path_filestat_get");
        
        let rel = match guest_str(mem, path, path_len) { Ok(s) => s, Err(e) => return e };
        let full = match ctx.resolve(dirfd, &rel) { Ok(p) => p, Err(e) => return e };
        let md = match std::fs::metadata(&full) { Ok(m) => m, Err(e) => return errno_of(&e) };
        let b = filestat_bytes(&md);
        let a = out as usize;
        match mem.get_mut(a..a + 64) {
            Some(d) => { d.copy_from_slice(&b); SUCCESS }
            None => EFAULT,
        }
    }

#[allow(unused_variables, unused_mut)]
pub fn path_create_directory(mem: &mut [u8], ctx: &mut WasiCtx, dirfd: i32, path: i32, path_len: i32) -> i32 {
        ctx.tick("path_create_directory");
        
        let rel = match guest_str(mem, path, path_len) { Ok(s) => s, Err(e) => return e };
        let full = match ctx.resolve(dirfd, &rel) { Ok(p) => p, Err(e) => return e };
        match std::fs::create_dir(&full) { Ok(()) => SUCCESS, Err(e) => errno_of(&e) }
    }

#[allow(unused_variables, unused_mut)]
pub fn path_remove_directory(mem: &mut [u8], ctx: &mut WasiCtx, dirfd: i32, path: i32, path_len: i32) -> i32 {
        ctx.tick("path_remove_directory");
        
        let rel = match guest_str(mem, path, path_len) { Ok(s) => s, Err(e) => return e };
        let full = match ctx.resolve(dirfd, &rel) { Ok(p) => p, Err(e) => return e };
        match std::fs::remove_dir(&full) { Ok(()) => SUCCESS, Err(e) => errno_of(&e) }
    }

#[allow(unused_variables, unused_mut)]
pub fn path_unlink_file(mem: &mut [u8], ctx: &mut WasiCtx, dirfd: i32, path: i32, path_len: i32) -> i32 {
        ctx.tick("path_unlink_file");
        
        let rel = match guest_str(mem, path, path_len) { Ok(s) => s, Err(e) => return e };
        let full = match ctx.resolve(dirfd, &rel) { Ok(p) => p, Err(e) => return e };
        match std::fs::remove_file(&full) { Ok(()) => SUCCESS, Err(e) => errno_of(&e) }
    }

#[allow(unused_variables, unused_mut)]
pub fn path_rename(mem: &mut [u8], ctx: &mut WasiCtx, ofd: i32, op: i32, ol: i32, nfd: i32, np: i32, nl: i32) -> i32 {
        ctx.tick("path_rename");
        
        let a = match guest_str(mem, op, ol) { Ok(s) => s, Err(e) => return e };
        let b = match guest_str(mem, np, nl) { Ok(s) => s, Err(e) => return e };
        let from = match ctx.resolve(ofd, &a) { Ok(p) => p, Err(e) => return e };
        let to = match ctx.resolve(nfd, &b) { Ok(p) => p, Err(e) => return e };
        match std::fs::rename(from, to) { Ok(()) => SUCCESS, Err(e) => errno_of(&e) }
    }

#[allow(unused_variables, unused_mut)]
pub fn fd_filestat_set_size(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i64) -> i32 {
        ctx.tick("fd_filestat_set_size"); ENOSYS }

#[allow(unused_variables, unused_mut)]
pub fn fd_filestat_set_times(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i64, _a3: i64, _a4: i32) -> i32 {
        ctx.tick("fd_filestat_set_times"); ENOSYS }

#[allow(unused_variables, unused_mut)]
pub fn path_filestat_set_times(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i64, _a6: i64, _a7: i32) -> i32 {
        ctx.tick("path_filestat_set_times"); ENOSYS }

#[allow(unused_variables, unused_mut)]
pub fn path_link(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32, _a7: i32) -> i32 {
        ctx.tick("path_link"); ENOSYS }

#[allow(unused_variables, unused_mut)]
pub fn path_symlink(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32 {
        ctx.tick("path_symlink"); ENOSYS }

#[allow(unused_variables, unused_mut)]
pub fn path_readlink(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32) -> i32 {
        ctx.tick("path_readlink"); ENOSYS }

#[allow(unused_variables, unused_mut)]
pub fn sock_accept(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i32, _a3: i32) -> i32 {
        ctx.tick("sock_accept"); ENOSYS }

#[allow(unused_variables, unused_mut)]
pub fn sock_recv(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32) -> i32 {
        ctx.tick("sock_recv"); ENOSYS }

#[allow(unused_variables, unused_mut)]
pub fn sock_send(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32 {
        ctx.tick("sock_send"); ENOSYS }

#[allow(unused_variables, unused_mut)]
pub fn sock_shutdown(mem: &mut [u8], ctx: &mut WasiCtx, _a1: i32, _a2: i32) -> i32 {
        ctx.tick("sock_shutdown"); ENOSYS }

fn write_string_vec(mem: &mut [u8], ptr_arr: i32, buf: i32, items: &[String]) -> i32 {
    let mut p = buf as u32;
    for (i, s) in items.iter().enumerate() {
        if let Err(e) = w32(mem, ptr_arr + (i as i32) * 4, p) { return e; }
        let a = p as usize;
        let end = a + s.len() + 1;
        match mem.get_mut(a..end) {
            Some(d) => {
                d[..s.len()].copy_from_slice(s.as_bytes());
                d[s.len()] = 0;
            }
            None => return EFAULT,
        }
        p = end as u32;
    }
    SUCCESS
}

fn gather(mem: &[u8], iovs: i32, iovs_len: i32) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    for i in 0..iovs_len {
        let base = iovs + i * 8;
        let ptr = r32(mem, base)? as i32;
        let len = r32(mem, base + 4)? as i32;
        out.extend_from_slice(slice(mem, ptr, len)?);
    }
    Ok(out)
}
