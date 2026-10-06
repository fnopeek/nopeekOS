//! The wasi ABI for both engines, from one list (see `wasm::forge_glue`).
//!
//! `proc_exit` is hand-written: it must not return, and that is exactly where
//! the engines differ.

use super::calls;

/// The one adapter a generator cannot write.
///
/// A wasi program always leaves through here, clean exit included. The
/// interpreter turns it into an `Err` and unwinds; generated code takes the
/// module's trap routine, which restores `rsp`/`rbp` and jumps back to the
/// entry. The status is recorded first because the trap path does not carry
/// it.
extern "C" fn f_proc_exit(vm: *const u64, code: i32) -> i32 {
    crate::wasm::forge_glue::with_ctx(vm, |st| super::record_exit(st, code));
    // SAFETY: same vmctx, and the caller holds nothing that needs cleanup.
    // Does not return.
    unsafe { crate::forge_rt::host_trap(vm, forge_core::trap::EXIT) }
}

/// Address and signature of the routine for a wasi import, or `None`.
pub(crate) fn resolve(module: &str, name: &str) -> Option<(u64, crate::forge_rt::HostSig)> {
    if module == "wasi_snapshot_preview1" && name == "proc_exit" {
        let sig = crate::forge_rt::HostSig { params: &[crate::forge_rt::Wt::I32], result: None };
        return Some((f_proc_exit as *const () as u64, sig));
    }
    resolve_table(module, name)
}

crate::wasm::forge_glue::host_imports! {
    ns: "wasi_snapshot_preview1", host: calls, no_memory: super::EFAULT;

    mem fn sched_yield() -> i32;

    // ── argv / environ ────────────────────────────────────────────────
    mem fn args_sizes_get(n: i32, sz: i32) -> i32;
    mem fn args_get(argv: i32, buf: i32) -> i32;
    mem fn environ_sizes_get(n: i32, sz: i32) -> i32;
    mem fn environ_get(ep: i32, buf: i32) -> i32;

    // ── clocks + randomness ───────────────────────────────────────────
    mem fn clock_res_get(_id: i32, out: i32) -> i32;
    mem fn clock_time_get(id: i32, _p: i64, out: i32) -> i32;
    mem fn random_get(buf: i32, len: i32) -> i32;

    // ── fd: the read/write path ───────────────────────────────────────────
    mem fn fd_write(fd: i32, iovs: i32, n: i32, out: i32) -> i32;
    mem fn fd_read(fd: i32, iovs: i32, n: i32, out: i32) -> i32;
    mem fn fd_pread(fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32;
    mem fn fd_pwrite(fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32;
    mem fn fd_seek(fd: i32, off: i64, whence: i32, out: i32) -> i32;
    mem fn fd_tell(fd: i32, out: i32) -> i32;
    mem fn fd_close(fd: i32) -> i32;
    mem fn fd_sync(_fd: i32) -> i32;
    mem fn fd_datasync(_fd: i32) -> i32;
    mem fn fd_advise(_f: i32, _o: i64, _l: i64, _a: i32) -> i32;
    mem fn fd_fdstat_set_flags(_f: i32, _fl: i32) -> i32;
    mem fn fd_fdstat_get(fd: i32, out: i32) -> i32;
    mem fn fd_filestat_get(fd: i32, out: i32) -> i32;
    mem fn fd_prestat_get(fd: i32, out: i32) -> i32;
    mem fn fd_prestat_dir_name(fd: i32, ptr: i32, len: i32) -> i32;
    mem fn fd_readdir(fd: i32, buf: i32, buf_len: i32, cookie: i64, out: i32) -> i32;
    mem fn poll_oneoff(_i: i32, _o: i32, _n: i32, nev: i32) -> i32;

    // ── path-based calls ──────────────────────────────────────────────────
    mem fn path_open(dirfd: i32, _dirflags: i32, path: i32, path_len: i32, oflags: i32, rights: i64, _rights_inh: i64, _fdflags: i32, out: i32) -> i32;
    mem fn path_filestat_get(dirfd: i32, _flags: i32, path: i32, path_len: i32, out: i32) -> i32;
    mem fn path_create_directory(dirfd: i32, p: i32, pl: i32) -> i32;
    mem fn path_remove_directory(dirfd: i32, p: i32, pl: i32) -> i32;
    mem fn path_unlink_file(dirfd: i32, p: i32, pl: i32) -> i32;
    mem fn path_rename(ofd: i32, op: i32, ol: i32, nfd: i32, np: i32, nl: i32) -> i32;

    // ── the ten that answer "no" ──────────────────────────────────────────
    //
    // Not laziness: npkFS has no links, no symlinks and no settable
    // timestamps, and there are no sockets behind this ABI. ENOSYS is the
    // true answer, and CPython handles it — `os.symlink` raises
    // OSError, which is what it should do on a filesystem without symlinks.
    mem fn fd_filestat_set_size(_a0: i32, _a1: i64) -> i32;
    mem fn fd_filestat_set_times(_a0: i32, _a1: i64, _a2: i64, _a3: i32) -> i32;
    mem fn path_filestat_set_times(_a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i64, _a5: i64, _a6: i32) -> i32;
    mem fn path_link(_a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32) -> i32;
    mem fn path_symlink(_a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32) -> i32;
    mem fn path_readlink(_a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32;
    mem fn fd_renumber(_a0: i32, _a1: i32) -> i32;
    mem fn sock_accept(_a0: i32, _a1: i32, _a2: i32) -> i32;
    mem fn sock_recv(_a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32;
    mem fn sock_send(_a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32) -> i32;
    mem fn sock_shutdown(_a0: i32, _a1: i32) -> i32;
}
