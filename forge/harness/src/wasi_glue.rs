//! Two adapters over ONE implementation. Everything the guest can observe
//! lives in `wasi_core`; all that differs here is how each engine hands over
//! guest memory and the context. Any other arrangement would compare host
//! layers instead of compilers.
#![allow(dead_code)]

use crate::wasi_core::{self, WasiCtx};
use wasmi::{Caller, Extern, Linker, Memory};

/// The context of the single instance a run has. One run, one instance, one
/// thread — so a pointer parked here is enough, and it saves threading a
/// closure environment through generated code that has none.
static mut WASI: *mut WasiCtx = std::ptr::null_mut();

pub fn install(ctx: &mut WasiCtx) {
    // SAFETY: the harness runs one instance at a time on one thread.
    unsafe { WASI = ctx as *mut WasiCtx };
}

/// Guest memory and context, from the instance context. Both are re-read on
/// every call because `memory.grow` may have moved the end since the last one.
unsafe fn parts<'a>(vm: *const u64) -> (&'a mut [u8], &'a mut WasiCtx) {
    use forge_core::vmctx as v;
    unsafe {
        let base = *vm.add(v::MEM_BASE as usize / 8) as *mut u8;
        let size = *vm.add(v::MEM_SIZE as usize / 8) as usize;
        (std::slice::from_raw_parts_mut(base, size), &mut *WASI)
    }
}

/// The same pair, the way the interpreter offers it.
fn pair<'a>(caller: &'a mut Caller<'_, WasiCtx>) -> (&'a mut [u8], &'a mut WasiCtx) {
    let mem = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => panic!("module without a memory"),
    };
    let m: Memory = mem;
    m.data_and_store_mut(caller)
}

extern "C" fn f_proc_exit(vm: *const u64, code: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::proc_exit(mem, ctx, code)
}

extern "C" fn f_sched_yield(vm: *const u64) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::sched_yield(mem, ctx)
}

extern "C" fn f_args_sizes_get(vm: *const u64, n: i32, sz: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::args_sizes_get(mem, ctx, n, sz)
}

extern "C" fn f_args_get(vm: *const u64, argv: i32, buf: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::args_get(mem, ctx, argv, buf)
}

extern "C" fn f_environ_sizes_get(vm: *const u64, n: i32, sz: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::environ_sizes_get(mem, ctx, n, sz)
}

extern "C" fn f_environ_get(vm: *const u64, ep: i32, buf: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::environ_get(mem, ctx, ep, buf)
}

extern "C" fn f_clock_res_get(vm: *const u64, _id: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::clock_res_get(mem, ctx, _id, out)
}

extern "C" fn f_clock_time_get(vm: *const u64, _id: i32, _prec: i64, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::clock_time_get(mem, ctx, _id, _prec, out)
}

extern "C" fn f_random_get(vm: *const u64, buf: i32, len: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::random_get(mem, ctx, buf, len)
}

extern "C" fn f_fd_fdstat_get(vm: *const u64, fd: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_fdstat_get(mem, ctx, fd, out)
}

extern "C" fn f_fd_fdstat_set_flags(vm: *const u64, _fd: i32, _f: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_fdstat_set_flags(mem, ctx, _fd, _f)
}

extern "C" fn f_fd_filestat_get(vm: *const u64, fd: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_filestat_get(mem, ctx, fd, out)
}

extern "C" fn f_fd_prestat_get(vm: *const u64, fd: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_prestat_get(mem, ctx, fd, out)
}

extern "C" fn f_fd_prestat_dir_name(vm: *const u64, fd: i32, ptr: i32, len: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_prestat_dir_name(mem, ctx, fd, ptr, len)
}

extern "C" fn f_fd_write(vm: *const u64, fd: i32, iovs: i32, n: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_write(mem, ctx, fd, iovs, n, out)
}

extern "C" fn f_fd_read(vm: *const u64, fd: i32, iovs: i32, n: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_read(mem, ctx, fd, iovs, n, out)
}

extern "C" fn f_fd_pread(vm: *const u64, fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_pread(mem, ctx, fd, iovs, n, off, out)
}

extern "C" fn f_fd_pwrite(vm: *const u64, fd: i32, iovs: i32, n: i32, off: i64, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_pwrite(mem, ctx, fd, iovs, n, off, out)
}

extern "C" fn f_fd_seek(vm: *const u64, fd: i32, off: i64, whence: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_seek(mem, ctx, fd, off, whence, out)
}

extern "C" fn f_fd_tell(vm: *const u64, fd: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_tell(mem, ctx, fd, out)
}

extern "C" fn f_fd_close(vm: *const u64, fd: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_close(mem, ctx, fd)
}

extern "C" fn f_fd_sync(vm: *const u64, _fd: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_sync(mem, ctx, _fd)
}

extern "C" fn f_fd_datasync(vm: *const u64, _fd: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_datasync(mem, ctx, _fd)
}

extern "C" fn f_fd_advise(vm: *const u64, _fd: i32, _o: i64, _l: i64, _a: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_advise(mem, ctx, _fd, _o, _l, _a)
}

extern "C" fn f_fd_readdir(vm: *const u64, fd: i32, buf: i32, buf_len: i32, cookie: i64, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_readdir(mem, ctx, fd, buf, buf_len, cookie, out)
}

extern "C" fn f_poll_oneoff(vm: *const u64, _in: i32, _out: i32, _n: i32, nev: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::poll_oneoff(mem, ctx, _in, _out, _n, nev)
}

extern "C" fn f_path_open(vm: *const u64, dirfd: i32, _dirflags: i32, path: i32, path_len: i32, oflags: i32, rights: i64, _rights_inh: i64, fdflags: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_open(mem, ctx, dirfd, _dirflags, path, path_len, oflags, rights, _rights_inh, fdflags, out)
}

extern "C" fn f_path_filestat_get(vm: *const u64, dirfd: i32, _flags: i32, path: i32, path_len: i32, out: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_filestat_get(mem, ctx, dirfd, _flags, path, path_len, out)
}

extern "C" fn f_path_create_directory(vm: *const u64, dirfd: i32, path: i32, path_len: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_create_directory(mem, ctx, dirfd, path, path_len)
}

extern "C" fn f_path_remove_directory(vm: *const u64, dirfd: i32, path: i32, path_len: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_remove_directory(mem, ctx, dirfd, path, path_len)
}

extern "C" fn f_path_unlink_file(vm: *const u64, dirfd: i32, path: i32, path_len: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_unlink_file(mem, ctx, dirfd, path, path_len)
}

extern "C" fn f_path_rename(vm: *const u64, ofd: i32, op: i32, ol: i32, nfd: i32, np: i32, nl: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_rename(mem, ctx, ofd, op, ol, nfd, np, nl)
}

extern "C" fn f_fd_filestat_set_size(vm: *const u64, _a1: i32, _a2: i64) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_filestat_set_size(mem, ctx, _a1, _a2)
}

extern "C" fn f_fd_filestat_set_times(vm: *const u64, _a1: i32, _a2: i64, _a3: i64, _a4: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::fd_filestat_set_times(mem, ctx, _a1, _a2, _a3, _a4)
}

extern "C" fn f_path_filestat_set_times(vm: *const u64, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i64, _a6: i64, _a7: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_filestat_set_times(mem, ctx, _a1, _a2, _a3, _a4, _a5, _a6, _a7)
}

extern "C" fn f_path_link(vm: *const u64, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32, _a7: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_link(mem, ctx, _a1, _a2, _a3, _a4, _a5, _a6, _a7)
}

extern "C" fn f_path_symlink(vm: *const u64, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_symlink(mem, ctx, _a1, _a2, _a3, _a4, _a5)
}

extern "C" fn f_path_readlink(vm: *const u64, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::path_readlink(mem, ctx, _a1, _a2, _a3, _a4, _a5, _a6)
}

extern "C" fn f_sock_accept(vm: *const u64, _a1: i32, _a2: i32, _a3: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::sock_accept(mem, ctx, _a1, _a2, _a3)
}

extern "C" fn f_sock_recv(vm: *const u64, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::sock_recv(mem, ctx, _a1, _a2, _a3, _a4, _a5, _a6)
}

extern "C" fn f_sock_send(vm: *const u64, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::sock_send(mem, ctx, _a1, _a2, _a3, _a4, _a5)
}

extern "C" fn f_sock_shutdown(vm: *const u64, _a1: i32, _a2: i32) -> i32 {
    // SAFETY: `vm` is the instance context of the module doing the call.
    let (mem, ctx) = unsafe { parts(vm) };
    wasi_core::sock_shutdown(mem, ctx, _a1, _a2)
}

/// Name to address, for the instance's host-function array.
pub fn forge_table() -> &'static [(&'static str, u64)] {
    // Built once; the addresses are of `extern "C"` items and never move.
    static_table()
}

fn static_table() -> &'static [(&'static str, u64)] {
    use std::sync::OnceLock;
    static T: OnceLock<Vec<(&'static str, u64)>> = OnceLock::new();
    T.get_or_init(|| vec![
        ("proc_exit", f_proc_exit as usize as u64),
        ("sched_yield", f_sched_yield as usize as u64),
        ("args_sizes_get", f_args_sizes_get as usize as u64),
        ("args_get", f_args_get as usize as u64),
        ("environ_sizes_get", f_environ_sizes_get as usize as u64),
        ("environ_get", f_environ_get as usize as u64),
        ("clock_res_get", f_clock_res_get as usize as u64),
        ("clock_time_get", f_clock_time_get as usize as u64),
        ("random_get", f_random_get as usize as u64),
        ("fd_fdstat_get", f_fd_fdstat_get as usize as u64),
        ("fd_fdstat_set_flags", f_fd_fdstat_set_flags as usize as u64),
        ("fd_filestat_get", f_fd_filestat_get as usize as u64),
        ("fd_prestat_get", f_fd_prestat_get as usize as u64),
        ("fd_prestat_dir_name", f_fd_prestat_dir_name as usize as u64),
        ("fd_write", f_fd_write as usize as u64),
        ("fd_read", f_fd_read as usize as u64),
        ("fd_pread", f_fd_pread as usize as u64),
        ("fd_pwrite", f_fd_pwrite as usize as u64),
        ("fd_seek", f_fd_seek as usize as u64),
        ("fd_tell", f_fd_tell as usize as u64),
        ("fd_close", f_fd_close as usize as u64),
        ("fd_sync", f_fd_sync as usize as u64),
        ("fd_datasync", f_fd_datasync as usize as u64),
        ("fd_advise", f_fd_advise as usize as u64),
        ("fd_readdir", f_fd_readdir as usize as u64),
        ("poll_oneoff", f_poll_oneoff as usize as u64),
        ("path_open", f_path_open as usize as u64),
        ("path_filestat_get", f_path_filestat_get as usize as u64),
        ("path_create_directory", f_path_create_directory as usize as u64),
        ("path_remove_directory", f_path_remove_directory as usize as u64),
        ("path_unlink_file", f_path_unlink_file as usize as u64),
        ("path_rename", f_path_rename as usize as u64),
        ("fd_filestat_set_size", f_fd_filestat_set_size as usize as u64),
        ("fd_filestat_set_times", f_fd_filestat_set_times as usize as u64),
        ("path_filestat_set_times", f_path_filestat_set_times as usize as u64),
        ("path_link", f_path_link as usize as u64),
        ("path_symlink", f_path_symlink as usize as u64),
        ("path_readlink", f_path_readlink as usize as u64),
        ("sock_accept", f_sock_accept as usize as u64),
        ("sock_recv", f_sock_recv as usize as u64),
        ("sock_send", f_sock_send as usize as u64),
        ("sock_shutdown", f_sock_shutdown as usize as u64),
    ])
}

pub fn link_wasmi(linker: &mut Linker<WasiCtx>) -> Result<(), wasmi::Error> {
    const NS: &str = "wasi_snapshot_preview1";
    linker.func_wrap(NS, "proc_exit", |mut caller: Caller<'_, WasiCtx>, code: i32| {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::proc_exit(mem, ctx, code);
    })?;
    linker.func_wrap(NS, "sched_yield", |mut caller: Caller<'_, WasiCtx>| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::sched_yield(mem, ctx)
    })?;
    linker.func_wrap(NS, "args_sizes_get", |mut caller: Caller<'_, WasiCtx>, n: i32, sz: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::args_sizes_get(mem, ctx, n, sz)
    })?;
    linker.func_wrap(NS, "args_get", |mut caller: Caller<'_, WasiCtx>, argv: i32, buf: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::args_get(mem, ctx, argv, buf)
    })?;
    linker.func_wrap(NS, "environ_sizes_get", |mut caller: Caller<'_, WasiCtx>, n: i32, sz: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::environ_sizes_get(mem, ctx, n, sz)
    })?;
    linker.func_wrap(NS, "environ_get", |mut caller: Caller<'_, WasiCtx>, ep: i32, buf: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::environ_get(mem, ctx, ep, buf)
    })?;
    linker.func_wrap(NS, "clock_res_get", |mut caller: Caller<'_, WasiCtx>, _id: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::clock_res_get(mem, ctx, _id, out)
    })?;
    linker.func_wrap(NS, "clock_time_get", |mut caller: Caller<'_, WasiCtx>, _id: i32, _prec: i64, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::clock_time_get(mem, ctx, _id, _prec, out)
    })?;
    linker.func_wrap(NS, "random_get", |mut caller: Caller<'_, WasiCtx>, buf: i32, len: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::random_get(mem, ctx, buf, len)
    })?;
    linker.func_wrap(NS, "fd_fdstat_get", |mut caller: Caller<'_, WasiCtx>, fd: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_fdstat_get(mem, ctx, fd, out)
    })?;
    linker.func_wrap(NS, "fd_fdstat_set_flags", |mut caller: Caller<'_, WasiCtx>, _fd: i32, _f: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_fdstat_set_flags(mem, ctx, _fd, _f)
    })?;
    linker.func_wrap(NS, "fd_filestat_get", |mut caller: Caller<'_, WasiCtx>, fd: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_filestat_get(mem, ctx, fd, out)
    })?;
    linker.func_wrap(NS, "fd_prestat_get", |mut caller: Caller<'_, WasiCtx>, fd: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_prestat_get(mem, ctx, fd, out)
    })?;
    linker.func_wrap(NS, "fd_prestat_dir_name", |mut caller: Caller<'_, WasiCtx>, fd: i32, ptr: i32, len: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_prestat_dir_name(mem, ctx, fd, ptr, len)
    })?;
    linker.func_wrap(NS, "fd_write", |mut caller: Caller<'_, WasiCtx>, fd: i32, iovs: i32, n: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_write(mem, ctx, fd, iovs, n, out)
    })?;
    linker.func_wrap(NS, "fd_read", |mut caller: Caller<'_, WasiCtx>, fd: i32, iovs: i32, n: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_read(mem, ctx, fd, iovs, n, out)
    })?;
    linker.func_wrap(NS, "fd_pread", |mut caller: Caller<'_, WasiCtx>, fd: i32, iovs: i32, n: i32, off: i64, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_pread(mem, ctx, fd, iovs, n, off, out)
    })?;
    linker.func_wrap(NS, "fd_pwrite", |mut caller: Caller<'_, WasiCtx>, fd: i32, iovs: i32, n: i32, off: i64, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_pwrite(mem, ctx, fd, iovs, n, off, out)
    })?;
    linker.func_wrap(NS, "fd_seek", |mut caller: Caller<'_, WasiCtx>, fd: i32, off: i64, whence: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_seek(mem, ctx, fd, off, whence, out)
    })?;
    linker.func_wrap(NS, "fd_tell", |mut caller: Caller<'_, WasiCtx>, fd: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_tell(mem, ctx, fd, out)
    })?;
    linker.func_wrap(NS, "fd_close", |mut caller: Caller<'_, WasiCtx>, fd: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_close(mem, ctx, fd)
    })?;
    linker.func_wrap(NS, "fd_sync", |mut caller: Caller<'_, WasiCtx>, _fd: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_sync(mem, ctx, _fd)
    })?;
    linker.func_wrap(NS, "fd_datasync", |mut caller: Caller<'_, WasiCtx>, _fd: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_datasync(mem, ctx, _fd)
    })?;
    linker.func_wrap(NS, "fd_advise", |mut caller: Caller<'_, WasiCtx>, _fd: i32, _o: i64, _l: i64, _a: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_advise(mem, ctx, _fd, _o, _l, _a)
    })?;
    linker.func_wrap(NS, "fd_readdir", |mut caller: Caller<'_, WasiCtx>, fd: i32, buf: i32, buf_len: i32, cookie: i64, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_readdir(mem, ctx, fd, buf, buf_len, cookie, out)
    })?;
    linker.func_wrap(NS, "poll_oneoff", |mut caller: Caller<'_, WasiCtx>, _in: i32, _out: i32, _n: i32, nev: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::poll_oneoff(mem, ctx, _in, _out, _n, nev)
    })?;
    linker.func_wrap(NS, "path_open", |mut caller: Caller<'_, WasiCtx>, dirfd: i32, _dirflags: i32, path: i32, path_len: i32, oflags: i32, rights: i64, _rights_inh: i64, fdflags: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_open(mem, ctx, dirfd, _dirflags, path, path_len, oflags, rights, _rights_inh, fdflags, out)
    })?;
    linker.func_wrap(NS, "path_filestat_get", |mut caller: Caller<'_, WasiCtx>, dirfd: i32, _flags: i32, path: i32, path_len: i32, out: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_filestat_get(mem, ctx, dirfd, _flags, path, path_len, out)
    })?;
    linker.func_wrap(NS, "path_create_directory", |mut caller: Caller<'_, WasiCtx>, dirfd: i32, path: i32, path_len: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_create_directory(mem, ctx, dirfd, path, path_len)
    })?;
    linker.func_wrap(NS, "path_remove_directory", |mut caller: Caller<'_, WasiCtx>, dirfd: i32, path: i32, path_len: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_remove_directory(mem, ctx, dirfd, path, path_len)
    })?;
    linker.func_wrap(NS, "path_unlink_file", |mut caller: Caller<'_, WasiCtx>, dirfd: i32, path: i32, path_len: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_unlink_file(mem, ctx, dirfd, path, path_len)
    })?;
    linker.func_wrap(NS, "path_rename", |mut caller: Caller<'_, WasiCtx>, ofd: i32, op: i32, ol: i32, nfd: i32, np: i32, nl: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_rename(mem, ctx, ofd, op, ol, nfd, np, nl)
    })?;
    linker.func_wrap(NS, "fd_filestat_set_size", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i64| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_filestat_set_size(mem, ctx, _a1, _a2)
    })?;
    linker.func_wrap(NS, "fd_filestat_set_times", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i64, _a3: i64, _a4: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::fd_filestat_set_times(mem, ctx, _a1, _a2, _a3, _a4)
    })?;
    linker.func_wrap(NS, "path_filestat_set_times", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i64, _a6: i64, _a7: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_filestat_set_times(mem, ctx, _a1, _a2, _a3, _a4, _a5, _a6, _a7)
    })?;
    linker.func_wrap(NS, "path_link", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32, _a7: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_link(mem, ctx, _a1, _a2, _a3, _a4, _a5, _a6, _a7)
    })?;
    linker.func_wrap(NS, "path_symlink", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_symlink(mem, ctx, _a1, _a2, _a3, _a4, _a5)
    })?;
    linker.func_wrap(NS, "path_readlink", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::path_readlink(mem, ctx, _a1, _a2, _a3, _a4, _a5, _a6)
    })?;
    linker.func_wrap(NS, "sock_accept", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i32, _a3: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::sock_accept(mem, ctx, _a1, _a2, _a3)
    })?;
    linker.func_wrap(NS, "sock_recv", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32, _a6: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::sock_recv(mem, ctx, _a1, _a2, _a3, _a4, _a5, _a6)
    })?;
    linker.func_wrap(NS, "sock_send", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i32, _a3: i32, _a4: i32, _a5: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::sock_send(mem, ctx, _a1, _a2, _a3, _a4, _a5)
    })?;
    linker.func_wrap(NS, "sock_shutdown", |mut caller: Caller<'_, WasiCtx>, _a1: i32, _a2: i32| -> i32 {
        let (mem, ctx) = pair(&mut caller);
        wasi_core::sock_shutdown(mem, ctx, _a1, _a2)
    })?;
    Ok(())
}
