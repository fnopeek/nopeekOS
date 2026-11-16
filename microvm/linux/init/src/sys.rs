//! Raw Linux syscalls for x86_64 and aarch64, and the process entry point.
//!
//! Callers use the x86_64 syscall set by name. aarch64 has no `open`,
//! `access`, `mkdir`, `dup2`, `pause`, `fork`, `pipe` or `poll`; there those
//! names carry numbers no kernel assigns, and the wrappers issue the `*at`,
//! `dup3`, `ppoll`, `clone` and `pipe2` equivalents instead.

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const SYS_READ: u64 = 0;
    pub const SYS_WRITE: u64 = 1;
    pub const SYS_OPEN: u64 = 2;
    pub const SYS_CLOSE: u64 = 3;
    pub const SYS_POLL: u64 = 7;
    #[allow(dead_code)] // part of the syscall table; a map with holes is worse
    pub const SYS_LSEEK: u64 = 8;
    pub const SYS_IOCTL: u64 = 16;
    pub const SYS_PREAD64: u64 = 17;
    pub const SYS_ACCESS: u64 = 21;
    pub const SYS_PIPE: u64 = 22;
    pub const SYS_DUP2: u64 = 33;
    pub const SYS_PAUSE: u64 = 34;
    pub const SYS_NANOSLEEP: u64 = 35;
    pub const SYS_FORK: u64 = 57;
    pub const SYS_EXECVE: u64 = 59;
    pub const SYS_EXIT: u64 = 60;
    pub const SYS_WAIT4: u64 = 61;
    pub const SYS_KILL: u64 = 62;
    pub const SYS_CHDIR: u64 = 80;
    pub const SYS_MKDIR: u64 = 83;
    pub const SYS_STATFS: u64 = 137;
    pub const SYS_SCHED_SETSCHEDULER: u64 = 144;
    pub const SYS_CHROOT: u64 = 161;
    pub const SYS_MOUNT: u64 = 165;
    pub const SYS_REBOOT: u64 = 169;
    pub const SYS_SCHED_SETAFFINITY: u64 = 203;
    pub const SYS_GETDENTS64: u64 = 217;
    pub const SYS_CLOCK_GETTIME: u64 = 228;

    pub const O_DIRECTORY: u64 = 0o200000;
}

#[cfg(target_arch = "aarch64")]
mod nr {
    // asm-generic/unistd.h
    pub const SYS_MKDIRAT: u64 = 34;
    pub const SYS_DUP3: u64 = 24;
    pub const SYS_FCNTL: u64 = 25;
    pub const SYS_IOCTL: u64 = 29;
    pub const SYS_MOUNT: u64 = 40;
    pub const SYS_STATFS: u64 = 43;
    pub const SYS_FACCESSAT: u64 = 48;
    pub const SYS_CHDIR: u64 = 49;
    pub const SYS_CHROOT: u64 = 51;
    pub const SYS_OPENAT: u64 = 56;
    pub const SYS_CLOSE: u64 = 57;
    pub const SYS_PIPE2: u64 = 59;
    pub const SYS_GETDENTS64: u64 = 61;
    #[allow(dead_code)] // part of the syscall table; a map with holes is worse
    pub const SYS_LSEEK: u64 = 62;
    pub const SYS_READ: u64 = 63;
    pub const SYS_WRITE: u64 = 64;
    pub const SYS_PREAD64: u64 = 67;
    pub const SYS_PPOLL: u64 = 73;
    pub const SYS_EXIT: u64 = 93;
    pub const SYS_NANOSLEEP: u64 = 101;
    pub const SYS_CLOCK_GETTIME: u64 = 113;
    pub const SYS_SCHED_SETSCHEDULER: u64 = 119;
    pub const SYS_SCHED_SETAFFINITY: u64 = 122;
    pub const SYS_KILL: u64 = 129;
    pub const SYS_REBOOT: u64 = 142;
    pub const SYS_CLONE: u64 = 220;
    pub const SYS_EXECVE: u64 = 221;
    pub const SYS_WAIT4: u64 = 260;

    // Not in the aarch64 table; `dispatch` maps them.
    const EMULATED: u64 = 1 << 32;
    pub const SYS_OPEN: u64 = EMULATED;
    pub const SYS_ACCESS: u64 = EMULATED + 1;
    pub const SYS_MKDIR: u64 = EMULATED + 2;
    pub const SYS_DUP2: u64 = EMULATED + 3;
    pub const SYS_PAUSE: u64 = EMULATED + 4;
    pub const SYS_FORK: u64 = EMULATED + 5;
    pub const SYS_PIPE: u64 = EMULATED + 6;
    pub const SYS_POLL: u64 = EMULATED + 7;

    // arm64 asm/fcntl.h moves O_DIRECTORY and O_NOFOLLOW down.
    pub const O_DIRECTORY: u64 = 0o40000;
}

pub use nr::*;

// _start in pure asm: Linux's process-load ABI gives RSP 16-aligned
// pointing at argc (no return slot). Rust's function prologue assumes
// the System V function-entry convention (RSP 8-misaligned after a
// CALL). Without the CALL bridge, any later MOVAPS on the stack #GPs.
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "    xor rbp, rbp",
    "    and rsp, -16",
    "    call rust_main",
    "    ud2",
);

// AAPCS64 needs only a 16-byte aligned SP, which the kernel hands over;
// zeroed fp and lr end the frame chain.
#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "    mov x29, xzr",
    "    mov x30, xzr",
    "    bl rust_main",
    "    brk #0",
);

#[cfg(target_arch = "x86_64")]
mod raw {
    use core::arch::asm;

    pub unsafe fn syscall0(nr: u64) -> i64 {
        let r: i64;
        // SAFETY: the caller upholds the syscall's contract; `syscall`
        // clobbers only rcx and r11 besides rax.
        unsafe {
            asm!("syscall",
                 inlateout("rax") nr as i64 => r,
                 out("rcx") _, out("r11") _,
                 options(nostack));
        }
        r
    }

    pub unsafe fn syscall1(nr: u64, a: u64) -> i64 {
        let r: i64;
        // SAFETY: as in `syscall0`.
        unsafe {
            asm!("syscall",
                 inlateout("rax") nr as i64 => r,
                 in("rdi") a,
                 out("rcx") _, out("r11") _,
                 options(nostack));
        }
        r
    }

    pub unsafe fn syscall2(nr: u64, a: u64, b: u64) -> i64 {
        let r: i64;
        // SAFETY: as in `syscall0`.
        unsafe {
            asm!("syscall",
                 inlateout("rax") nr as i64 => r,
                 in("rdi") a, in("rsi") b,
                 out("rcx") _, out("r11") _,
                 options(nostack));
        }
        r
    }

    pub unsafe fn syscall3(nr: u64, a: u64, b: u64, c: u64) -> i64 {
        let r: i64;
        // SAFETY: as in `syscall0`.
        unsafe {
            asm!("syscall",
                 inlateout("rax") nr as i64 => r,
                 in("rdi") a, in("rsi") b, in("rdx") c,
                 out("rcx") _, out("r11") _,
                 options(nostack));
        }
        r
    }

    pub unsafe fn syscall4(nr: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
        let r: i64;
        // SAFETY: as in `syscall0`.
        unsafe {
            asm!("syscall",
                 inlateout("rax") nr as i64 => r,
                 in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d,
                 out("rcx") _, out("r11") _,
                 options(nostack));
        }
        r
    }

    pub unsafe fn syscall5(nr: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
        let r: i64;
        // SAFETY: as in `syscall0`.
        unsafe {
            asm!("syscall",
                 inlateout("rax") nr as i64 => r,
                 in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, in("r8") e,
                 out("rcx") _, out("r11") _,
                 options(nostack));
        }
        r
    }
}

#[cfg(target_arch = "aarch64")]
mod raw {
    use super::nr::*;
    use core::arch::asm;

    const AT_FDCWD: u64 = -100i64 as u64;
    const SIGCHLD: u64 = 17;
    const F_GETFD: u64 = 1;

    unsafe fn svc(nr: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
        let r: i64;
        // SAFETY: the caller upholds the syscall's contract; `svc 0`
        // returns in x0 and clobbers nothing else.
        unsafe {
            asm!("svc 0",
                 in("x8") nr,
                 inlateout("x0") a as i64 => r,
                 in("x1") b, in("x2") c, in("x3") d, in("x4") e,
                 options(nostack));
        }
        r
    }

    /// Issue `nr` with the x86_64 argument order and meaning.
    unsafe fn dispatch(nr: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
        // SAFETY (all arms): each replacement takes the same pointers with
        // the same meaning as the call it stands for.
        unsafe {
            match nr {
                SYS_OPEN => svc(SYS_OPENAT, AT_FDCWD, a, b, c, 0),
                SYS_ACCESS => svc(SYS_FACCESSAT, AT_FDCWD, a, b, 0, 0),
                SYS_MKDIR => svc(SYS_MKDIRAT, AT_FDCWD, a, b, 0, 0),
                // dup3 refuses equal fds, where dup2 checks `a` and returns it.
                SYS_DUP2 if a == b => {
                    let r = svc(SYS_FCNTL, a, F_GETFD, 0, 0, 0);
                    if r < 0 { r } else { b as i64 }
                }
                SYS_DUP2 => svc(SYS_DUP3, a, b, 0, 0, 0),
                SYS_PAUSE => svc(SYS_PPOLL, 0, 0, 0, 0, 0),
                SYS_FORK => svc(SYS_CLONE, SIGCHLD, 0, 0, 0, 0),
                SYS_PIPE => svc(SYS_PIPE2, a, 0, 0, 0, 0),
                SYS_POLL => {
                    let ms = c as i64;
                    let ts = [ms / 1000, ms % 1000 * 1_000_000];
                    let tp = if ms < 0 { 0 } else { ts.as_ptr() as u64 };
                    svc(SYS_PPOLL, a, b, tp, 0, 0)
                }
                _ => svc(nr, a, b, c, d, e),
            }
        }
    }

    pub unsafe fn syscall0(nr: u64) -> i64 {
        // SAFETY: forwarded contract.
        unsafe { dispatch(nr, 0, 0, 0, 0, 0) }
    }

    pub unsafe fn syscall1(nr: u64, a: u64) -> i64 {
        // SAFETY: forwarded contract.
        unsafe { dispatch(nr, a, 0, 0, 0, 0) }
    }

    pub unsafe fn syscall2(nr: u64, a: u64, b: u64) -> i64 {
        // SAFETY: forwarded contract.
        unsafe { dispatch(nr, a, b, 0, 0, 0) }
    }

    pub unsafe fn syscall3(nr: u64, a: u64, b: u64, c: u64) -> i64 {
        // SAFETY: forwarded contract.
        unsafe { dispatch(nr, a, b, c, 0, 0) }
    }

    pub unsafe fn syscall4(nr: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
        // SAFETY: forwarded contract.
        unsafe { dispatch(nr, a, b, c, d, 0) }
    }

    pub unsafe fn syscall5(nr: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
        // SAFETY: forwarded contract.
        unsafe { dispatch(nr, a, b, c, d, e) }
    }
}

pub use raw::*;

// x86_64 lowers block fills and copies inline; aarch64 calls the C library's
// symbols, which a binary without libc has to bring. Volatile accesses keep
// the loops from being recognised as the very calls they implement.
#[cfg(target_arch = "aarch64")]
mod mem {
    use core::ffi::{c_int, c_void};

    #[unsafe(no_mangle)]
    unsafe extern "C" fn memset(dst: *mut c_void, c: c_int, n: usize) -> *mut c_void {
        let d = dst as *mut u8;
        for i in 0..n {
            // SAFETY: the caller guarantees `dst..dst+n` is writable.
            unsafe { d.add(i).write_volatile(c as u8) };
        }
        dst
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
        let (d, s) = (dst as *mut u8, src as *const u8);
        for i in 0..n {
            // SAFETY: the caller guarantees both ranges are valid and disjoint.
            unsafe { d.add(i).write_volatile(s.add(i).read_volatile()) };
        }
        dst
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn memmove(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
        let (d, s) = (dst as *mut u8, src as *const u8);
        if (d as usize) <= (s as usize) {
            for i in 0..n {
                // SAFETY: the caller guarantees both ranges are valid; copying
                // upwards never overwrites a byte before it is read.
                unsafe { d.add(i).write_volatile(s.add(i).read_volatile()) };
            }
        } else {
            for i in (0..n).rev() {
                // SAFETY: as above, copying downwards.
                unsafe { d.add(i).write_volatile(s.add(i).read_volatile()) };
            }
        }
        dst
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int {
        let (a, b) = (a as *const u8, b as *const u8);
        for i in 0..n {
            // SAFETY: the caller guarantees both ranges are readable.
            let (x, y) = unsafe { (a.add(i).read_volatile(), b.add(i).read_volatile()) };
            if x != y {
                return x as c_int - y as c_int;
            }
        }
        0
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn bcmp(a: *const c_void, b: *const c_void, n: usize) -> c_int {
        // SAFETY: forwarded contract.
        unsafe { memcmp(a, b, n) }
    }
}
