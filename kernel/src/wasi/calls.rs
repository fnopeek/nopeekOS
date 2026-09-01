//! wasi preview1, motorneutral.
//!
//! Jede Funktion hier arbeitet auf `(&mut [u8], &mut HS)` und sonst nichts —
//! genau das Paar, das das `state!`-Makro im Interpreterpfad schon lieferte.
//! Deshalb sind die Rumpfe unveraendert herueber gekommen; was sich je Motor
//! unterscheidet, ist nur, WIE die beiden beschafft werden.
//!
//! `proc_exit` steht bewusst NICHT hier: es darf nicht zurueckkehren, und wie
//! man ein Modul verlaesst, ist das eine, was die Motoren wirklich
//! unterscheidet. Siehe `wasi.rs`.
//!
//! DIESE DATEI IST ERZEUGT.
#![allow(clippy::too_many_arguments)]

use super::*;

pub(crate) fn sched_yield(_mem: &mut [u8], _st: &mut HS) -> i32 {
 SUCCESS 
}

pub(crate) fn args_sizes_get(mem: &mut [u8], st: &mut HS, n: i32, sz: i32) -> i32 {
let w = wasi_of!(st);
let count = w.args.len() as u32;
let bytes: u32 = w.args.iter().map(|a| a.len() as u32 + 1).sum();
if let Err(e) = w32(mem, n, count) { return e; }
if let Err(e) = w32(mem, sz, bytes) { return e; }
SUCCESS
}

pub(crate) fn args_get(mem: &mut [u8], st: &mut HS, argv: i32, buf: i32) -> i32 {
let w = wasi_of!(st);
let items = w.args.clone();
write_string_vec(mem, argv, buf, &items)
}

pub(crate) fn environ_sizes_get(mem: &mut [u8], st: &mut HS, n: i32, sz: i32) -> i32 {
let w = wasi_of!(st);
let count = w.env.len() as u32;
let bytes: u32 = w.env.iter().map(|a| a.len() as u32 + 1).sum();
if let Err(e) = w32(mem, n, count) { return e; }
if let Err(e) = w32(mem, sz, bytes) { return e; }
SUCCESS
}

pub(crate) fn environ_get(mem: &mut [u8], st: &mut HS, ep: i32, buf: i32) -> i32 {
let w = wasi_of!(st);
let items = w.env.clone();
write_string_vec(mem, ep, buf, &items)
}

pub(crate) fn clock_res_get(mem: &mut [u8], st: &mut HS, _id: i32, out: i32) -> i32 {
let _ = wasi_of!(st);
// The tick is 100 Hz, and saying so beats claiming nanoseconds
// we cannot deliver.
match w64(mem, out, 10_000_000) { Ok(()) => SUCCESS, Err(e) => e }
}

pub(crate) fn clock_time_get(mem: &mut [u8], st: &mut HS, id: i32, _p: i64, out: i32) -> i32 {
// 0 = realtime, everything else treated as monotonic.
let now = if id == 0 {
    (crate::rtc::read_unix_time().unwrap_or(0) as u64).saturating_mul(1_000_000_000)
} else {
    crate::interrupts::ticks().saturating_mul(10_000_000)
};
let _ = wasi_of!(st);
match w64(mem, out, now) { Ok(()) => SUCCESS, Err(e) => e }
}

pub(crate) fn random_get(mem: &mut [u8], st: &mut HS, buf: i32, len: i32) -> i32 {
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
let _ = wasi_of!(st);
match put_bytes(mem, buf, &tmp) { Ok(()) => SUCCESS, Err(e) => e }
}

pub(crate) fn fd_write(mem: &mut [u8], st: &mut HS, fd: i32, iovs: i32, n: i32, out: i32) -> i32 {
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
}

pub(crate) fn fd_read(mem: &mut [u8], st: &mut HS, fd: i32, iovs: i32, n: i32, out: i32) -> i32 {
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
}

pub(crate) fn fd_pread(mem: &mut [u8], st: &mut HS, fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32 {
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
}

pub(crate) fn fd_pwrite(mem: &mut [u8], st: &mut HS, fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32 {
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
}

pub(crate) fn fd_seek(mem: &mut [u8], st: &mut HS, fd: i32, off: i64, whence: i32, out: i32) -> i32 {
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
}

pub(crate) fn fd_tell(mem: &mut [u8], st: &mut HS, fd: i32, out: i32) -> i32 {
let w = wasi_of!(st);
let pos = match w.fds.get(&fd) {
    Some(Handle::File { pos, .. }) => *pos as u64,
    Some(_) => return ESPIPE,
    None => return EBADF,
};
match w64(mem, out, pos) { Ok(()) => SUCCESS, Err(e) => e }
}

pub(crate) fn fd_close(_mem: &mut [u8], st: &mut HS, fd: i32) -> i32 {
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
}

pub(crate) fn fd_sync(_mem: &mut [u8], st: &mut HS, _fd: i32) -> i32 {
let _ = wasi_of!(st); SUCCESS
}

pub(crate) fn fd_datasync(_mem: &mut [u8], st: &mut HS, _fd: i32) -> i32 {
let _ = wasi_of!(st); SUCCESS
}

pub(crate) fn fd_advise(_mem: &mut [u8], st: &mut HS, _f: i32, _o: i64, _l: i64, _a: i32) -> i32 {
let _ = wasi_of!(st); SUCCESS
}

pub(crate) fn fd_fdstat_set_flags(_mem: &mut [u8], st: &mut HS, _f: i32, _fl: i32) -> i32 {
let _ = wasi_of!(st); SUCCESS
}

pub(crate) fn fd_fdstat_get(mem: &mut [u8], st: &mut HS, fd: i32, out: i32) -> i32 {
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
}

pub(crate) fn fd_filestat_get(mem: &mut [u8], st: &mut HS, fd: i32, out: i32) -> i32 {
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
}

pub(crate) fn fd_prestat_get(mem: &mut [u8], st: &mut HS, fd: i32, out: i32) -> i32 {
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
}

pub(crate) fn fd_prestat_dir_name(mem: &mut [u8], st: &mut HS, fd: i32, ptr: i32, len: i32) -> i32 {
let w = wasi_of!(st);
let name = match w.fds.get(&fd) {
    Some(Handle::Dir { guest, preopen: true, .. }) => guest.clone(),
    _ => return EBADF,
};
let n = (len as usize).min(name.len());
match put_bytes(mem, ptr, &name.as_bytes()[..n]) { Ok(()) => SUCCESS, Err(e) => e }
}

pub(crate) fn fd_readdir(mem: &mut [u8], st: &mut HS, fd: i32, buf: i32, buf_len: i32, cookie: i64, out: i32) -> i32 {
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
}

pub(crate) fn poll_oneoff(mem: &mut [u8], st: &mut HS, _i: i32, _o: i32, _n: i32, nev: i32) -> i32 {
let _ = wasi_of!(st);
// Nothing is ever ready. CPython polls here for stdin and for
// its signal machinery; neither exists for us yet.
match w32(mem, nev, 0) { Ok(()) => SUCCESS, Err(e) => e }
}

pub(crate) fn path_open(mem: &mut [u8], st: &mut HS, dirfd: i32, _dirflags: i32, path: i32, path_len: i32, oflags: i32, rights: i64, _rights_inh: i64, _fdflags: i32, out: i32) -> i32 {
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
}

pub(crate) fn path_filestat_get(mem: &mut [u8], st: &mut HS, dirfd: i32, _flags: i32, path: i32, path_len: i32, out: i32) -> i32 {
let w = wasi_of!(st);
let rel = match guest_str(mem, path, path_len) { Ok(s) => s, Err(e) => return e };
let (full, _, _) = match w.resolve(dirfd, &rel) { Ok(v) => v, Err(e) => return e };
let b = match fs::stat(&full) {
    Ok(Some(s)) => filestat_bytes(s.kind, s.size, s.mtime),
    Ok(None) => return ENOENT,
    Err(e) => return fs_errno(&e),
};
match put_bytes(mem, out, &b) { Ok(()) => SUCCESS, Err(e) => e }
}

pub(crate) fn path_create_directory(mem: &mut [u8], st: &mut HS, dirfd: i32, p: i32, pl: i32) -> i32 {
let cap = st.cap_id;
let w = wasi_of!(st);
let rel = match guest_str(mem, p, pl) { Ok(s) => s, Err(e) => return e };
let (full, _, writable) = match w.resolve(dirfd, &rel) { Ok(v) => v, Err(e) => return e };
if !writable || capability::check_global(&cap, Rights::WRITE).is_err() { return EPERM; }
match fs::mkdir(&full) { Ok(()) => SUCCESS, Err(e) => fs_errno(&e) }
}

pub(crate) fn path_remove_directory(mem: &mut [u8], st: &mut HS, dirfd: i32, p: i32, pl: i32) -> i32 {
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
}

pub(crate) fn path_unlink_file(mem: &mut [u8], st: &mut HS, dirfd: i32, p: i32, pl: i32) -> i32 {
let cap = st.cap_id;
let w = wasi_of!(st);
let rel = match guest_str(mem, p, pl) { Ok(s) => s, Err(e) => return e };
let (full, _, writable) = match w.resolve(dirfd, &rel) { Ok(v) => v, Err(e) => return e };
if !writable || capability::check_global(&cap, Rights::WRITE).is_err() { return EPERM; }
match fs::delete(&full) { Ok(()) => SUCCESS, Err(e) => fs_errno(&e) }
}

pub(crate) fn path_rename(mem: &mut [u8], st: &mut HS, ofd: i32, op: i32, ol: i32, nfd: i32, np: i32, nl: i32) -> i32 {
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
}

pub(crate) fn fd_filestat_set_size(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i64) -> i32 {
 ENOSYS 
}

pub(crate) fn fd_filestat_set_times(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i64, _a2: i64, _a3: i32) -> i32 {
 ENOSYS 
}

pub(crate) fn path_filestat_set_times(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i64, _a5: i64, _a6: i32) -> i32 {
 ENOSYS 
}

pub(crate) fn path_link(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32) -> i32 {
 ENOSYS 
}

pub(crate) fn path_symlink(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32) -> i32 {
 ENOSYS 
}

pub(crate) fn path_readlink(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32 {
 ENOSYS 
}

pub(crate) fn fd_renumber(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i32) -> i32 {
 ENOSYS 
}

pub(crate) fn sock_accept(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i32, _a2: i32) -> i32 {
 ENOSYS 
}

pub(crate) fn sock_recv(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32 {
 ENOSYS 
}

pub(crate) fn sock_send(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32) -> i32 {
 ENOSYS 
}

pub(crate) fn sock_shutdown(_mem: &mut [u8], _st: &mut HS, _a0: i32, _a1: i32) -> i32 {
 ENOSYS 
}
