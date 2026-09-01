//! Die forge-Seite der wasi-ABI.
//!
//! Ein Adapter je Funktion, und jeder tut dasselbe wie sein wasmi-Zwilling in
//! `wasi.rs`: `mem` und `st` beschaffen und `calls::` rufen. Der Helfer dafuer
//! ist derselbe wie auf der npk-Seite — zwei Fassungen wuerden auseinander
//! laufen.
//!
//! `proc_exit` ist von Hand geschrieben und steht unten: es darf nicht
//! zurueckkehren, und genau darin unterscheiden sich die Motoren.
//!
//! DIESE DATEI IST ERZEUGT (bis auf `proc_exit` und `resolve`).
#![allow(clippy::too_many_arguments)]

use super::calls;
use crate::wasm::forge_glue::parts;

extern "C" fn f_sched_yield(vm: *const u64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::sched_yield(mem, st)
}

extern "C" fn f_args_sizes_get(vm: *const u64, n: i32, sz: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::args_sizes_get(mem, st, n, sz)
}

extern "C" fn f_args_get(vm: *const u64, argv: i32, buf: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::args_get(mem, st, argv, buf)
}

extern "C" fn f_environ_sizes_get(vm: *const u64, n: i32, sz: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::environ_sizes_get(mem, st, n, sz)
}

extern "C" fn f_environ_get(vm: *const u64, ep: i32, buf: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::environ_get(mem, st, ep, buf)
}

extern "C" fn f_clock_res_get(vm: *const u64, _id: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::clock_res_get(mem, st, _id, out)
}

extern "C" fn f_clock_time_get(vm: *const u64, id: i32, _p: i64, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::clock_time_get(mem, st, id, _p, out)
}

extern "C" fn f_random_get(vm: *const u64, buf: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::random_get(mem, st, buf, len)
}

extern "C" fn f_fd_write(vm: *const u64, fd: i32, iovs: i32, n: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_write(mem, st, fd, iovs, n, out)
}

extern "C" fn f_fd_read(vm: *const u64, fd: i32, iovs: i32, n: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_read(mem, st, fd, iovs, n, out)
}

extern "C" fn f_fd_pread(vm: *const u64, fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_pread(mem, st, fd, iovs, n, off, out)
}

extern "C" fn f_fd_pwrite(vm: *const u64, fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_pwrite(mem, st, fd, iovs, n, off, out)
}

extern "C" fn f_fd_seek(vm: *const u64, fd: i32, off: i64, whence: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_seek(mem, st, fd, off, whence, out)
}

extern "C" fn f_fd_tell(vm: *const u64, fd: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_tell(mem, st, fd, out)
}

extern "C" fn f_fd_close(vm: *const u64, fd: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_close(mem, st, fd)
}

extern "C" fn f_fd_sync(vm: *const u64, _fd: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_sync(mem, st, _fd)
}

extern "C" fn f_fd_datasync(vm: *const u64, _fd: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_datasync(mem, st, _fd)
}

extern "C" fn f_fd_advise(vm: *const u64, _f: i32, _o: i64, _l: i64, _a: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_advise(mem, st, _f, _o, _l, _a)
}

extern "C" fn f_fd_fdstat_set_flags(vm: *const u64, _f: i32, _fl: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_fdstat_set_flags(mem, st, _f, _fl)
}

extern "C" fn f_fd_fdstat_get(vm: *const u64, fd: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_fdstat_get(mem, st, fd, out)
}

extern "C" fn f_fd_filestat_get(vm: *const u64, fd: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_filestat_get(mem, st, fd, out)
}

extern "C" fn f_fd_prestat_get(vm: *const u64, fd: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_prestat_get(mem, st, fd, out)
}

extern "C" fn f_fd_prestat_dir_name(vm: *const u64, fd: i32, ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_prestat_dir_name(mem, st, fd, ptr, len)
}

extern "C" fn f_fd_readdir(vm: *const u64, fd: i32, buf: i32, buf_len: i32, cookie: i64, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_readdir(mem, st, fd, buf, buf_len, cookie, out)
}

extern "C" fn f_poll_oneoff(vm: *const u64, _i: i32, _o: i32, _n: i32, nev: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::poll_oneoff(mem, st, _i, _o, _n, nev)
}

extern "C" fn f_path_open(vm: *const u64, dirfd: i32, _dirflags: i32, path: i32, path_len: i32, oflags: i32, rights: i64, _rights_inh: i64, _fdflags: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_open(mem, st, dirfd, _dirflags, path, path_len, oflags, rights, _rights_inh, _fdflags, out)
}

extern "C" fn f_path_filestat_get(vm: *const u64, dirfd: i32, _flags: i32, path: i32, path_len: i32, out: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_filestat_get(mem, st, dirfd, _flags, path, path_len, out)
}

extern "C" fn f_path_create_directory(vm: *const u64, dirfd: i32, p: i32, pl: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_create_directory(mem, st, dirfd, p, pl)
}

extern "C" fn f_path_remove_directory(vm: *const u64, dirfd: i32, p: i32, pl: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_remove_directory(mem, st, dirfd, p, pl)
}

extern "C" fn f_path_unlink_file(vm: *const u64, dirfd: i32, p: i32, pl: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_unlink_file(mem, st, dirfd, p, pl)
}

extern "C" fn f_path_rename(vm: *const u64, ofd: i32, op: i32, ol: i32, nfd: i32, np: i32, nl: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_rename(mem, st, ofd, op, ol, nfd, np, nl)
}

extern "C" fn f_fd_filestat_set_size(vm: *const u64, _a0: i32, _a1: i64) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_filestat_set_size(mem, st, _a0, _a1)
}

extern "C" fn f_fd_filestat_set_times(vm: *const u64, _a0: i32, _a1: i64, _a2: i64, _a3: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_filestat_set_times(mem, st, _a0, _a1, _a2, _a3)
}

extern "C" fn f_path_filestat_set_times(vm: *const u64, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i64, _a5: i64, _a6: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_filestat_set_times(mem, st, _a0, _a1, _a2, _a3, _a4, _a5, _a6)
}

extern "C" fn f_path_link(vm: *const u64, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_link(mem, st, _a0, _a1, _a2, _a3, _a4, _a5, _a6)
}

extern "C" fn f_path_symlink(vm: *const u64, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_symlink(mem, st, _a0, _a1, _a2, _a3, _a4)
}

extern "C" fn f_path_readlink(vm: *const u64, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::path_readlink(mem, st, _a0, _a1, _a2, _a3, _a4, _a5)
}

extern "C" fn f_fd_renumber(vm: *const u64, _a0: i32, _a1: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::fd_renumber(mem, st, _a0, _a1)
}

extern "C" fn f_sock_accept(vm: *const u64, _a0: i32, _a1: i32, _a2: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::sock_accept(mem, st, _a0, _a1, _a2)
}

extern "C" fn f_sock_recv(vm: *const u64, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::sock_recv(mem, st, _a0, _a1, _a2, _a3, _a4, _a5)
}

extern "C" fn f_sock_send(vm: *const u64, _a0: i32, _a1: i32, _a2: i32, _a3: i32, _a4: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::sock_send(mem, st, _a0, _a1, _a2, _a3, _a4)
}

extern "C" fn f_sock_shutdown(vm: *const u64, _a0: i32, _a1: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (mem, st) = unsafe { parts(vm) };
    calls::sock_shutdown(mem, st, _a0, _a1)
}

/// Der eine Adapter, den kein Generator schreiben kann.
///
/// Ein wasi-Programm verlaesst sich IMMER hierueber, sauberes Ende
/// eingeschlossen. Der Interpreter macht daraus ein `Err` und rollt ab;
/// erzeugter Code nimmt die Trap-Routine des Moduls, die `rsp`/`rbp`
/// wiederherstellt und zum Eintritt zurueckspringt. Der Status wird VORHER
/// hinterlegt — im Trap-Code reist er nicht mit.
extern "C" fn f_proc_exit(vm: *const u64, code: i32) -> i32 {
    // SAFETY: `vm` ist der vmctx des rufenden Moduls.
    let (_mem, st) = unsafe { parts(vm) };
    super::record_exit(st, code);
    // SAFETY: derselbe vmctx, und der Aufrufer haelt nichts, was aufgeraeumt
    // werden muesste. Kehrt nicht zurueck.
    unsafe { crate::forge_rt::host_trap(vm, forge_core::trap::EXIT) }
}

/// Adresse der Routine fuer einen wasi-Import, oder nichts.
pub(crate) fn resolve(module: &str, name: &str) -> Option<u64> {
    if module != "wasi_snapshot_preview1" {
        return None;
    }
    Some(match name {
        "proc_exit" => f_proc_exit as *const () as u64,
        "sched_yield" => f_sched_yield as *const () as u64,
        "args_sizes_get" => f_args_sizes_get as *const () as u64,
        "args_get" => f_args_get as *const () as u64,
        "environ_sizes_get" => f_environ_sizes_get as *const () as u64,
        "environ_get" => f_environ_get as *const () as u64,
        "clock_res_get" => f_clock_res_get as *const () as u64,
        "clock_time_get" => f_clock_time_get as *const () as u64,
        "random_get" => f_random_get as *const () as u64,
        "fd_write" => f_fd_write as *const () as u64,
        "fd_read" => f_fd_read as *const () as u64,
        "fd_pread" => f_fd_pread as *const () as u64,
        "fd_pwrite" => f_fd_pwrite as *const () as u64,
        "fd_seek" => f_fd_seek as *const () as u64,
        "fd_tell" => f_fd_tell as *const () as u64,
        "fd_close" => f_fd_close as *const () as u64,
        "fd_sync" => f_fd_sync as *const () as u64,
        "fd_datasync" => f_fd_datasync as *const () as u64,
        "fd_advise" => f_fd_advise as *const () as u64,
        "fd_fdstat_set_flags" => f_fd_fdstat_set_flags as *const () as u64,
        "fd_fdstat_get" => f_fd_fdstat_get as *const () as u64,
        "fd_filestat_get" => f_fd_filestat_get as *const () as u64,
        "fd_prestat_get" => f_fd_prestat_get as *const () as u64,
        "fd_prestat_dir_name" => f_fd_prestat_dir_name as *const () as u64,
        "fd_readdir" => f_fd_readdir as *const () as u64,
        "poll_oneoff" => f_poll_oneoff as *const () as u64,
        "path_open" => f_path_open as *const () as u64,
        "path_filestat_get" => f_path_filestat_get as *const () as u64,
        "path_create_directory" => f_path_create_directory as *const () as u64,
        "path_remove_directory" => f_path_remove_directory as *const () as u64,
        "path_unlink_file" => f_path_unlink_file as *const () as u64,
        "path_rename" => f_path_rename as *const () as u64,
        "fd_filestat_set_size" => f_fd_filestat_set_size as *const () as u64,
        "fd_filestat_set_times" => f_fd_filestat_set_times as *const () as u64,
        "path_filestat_set_times" => f_path_filestat_set_times as *const () as u64,
        "path_link" => f_path_link as *const () as u64,
        "path_symlink" => f_path_symlink as *const () as u64,
        "path_readlink" => f_path_readlink as *const () as u64,
        "fd_renumber" => f_fd_renumber as *const () as u64,
        "sock_accept" => f_sock_accept as *const () as u64,
        "sock_recv" => f_sock_recv as *const () as u64,
        "sock_send" => f_sock_send as *const () as u64,
        "sock_shutdown" => f_sock_shutdown as *const () as u64,
        _ => return None,
    })
}
