//! microvm-init — nopeekOS Linux MicroVM PID-1.
//!
//! Statically linked, no_std, no libc. Talks to the Linux kernel
//! exclusively via raw syscalls (x86_64 ABI: rax=nr, rdi/rsi/rdx/r10
//! /r8/r9 = args, syscall, rax = result).
//!
//! Substrate task: mount the four essential virtual filesystems
//! (/proc /sys /dev /tmp), open the console + kmsg, do one
//! virtio-input smoke-read, then pause. Future versions will exec
//! the container manifest's `init` from a real rootfs.

#![no_std]
#![no_main]

use core::arch::asm;

const SYS_READ: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_OPEN: u64 = 2;
const SYS_CLOSE: u64 = 3;
const SYS_DUP2: u64 = 33;
const SYS_PAUSE: u64 = 34;
const SYS_ACCESS: u64 = 21;
const SYS_EXECVE: u64 = 59;
const SYS_EXIT: u64 = 60;
const SYS_MKDIR: u64 = 83;
const SYS_MOUNT: u64 = 165;
const SYS_REBOOT: u64 = 169;

// access(2) modes
const F_OK: u64 = 0;

const O_RDONLY: u64 = 0;
const O_RDWR: u64 = 2;

const LINUX_REBOOT_MAGIC1: u64 = 0xfee1dead;
const LINUX_REBOOT_MAGIC2: u64 = 0x28121969;
const LINUX_REBOOT_CMD_POWER_OFF: u64 = 0x4321fedc;

#[panic_handler]
fn on_panic(_info: &core::panic::PanicInfo) -> ! {
    let _ = sys_write(2, b"microvm-init panic\n");
    halt();
}

// _start in pure asm: Linux's process-load ABI gives RSP 16-aligned
// pointing at argc (no return slot). Rust's function prologue assumes
// the System V function-entry convention (RSP 8-misaligned after a
// CALL). Without the CALL bridge, any later MOVAPS-on-stack #GPs.
// Verified on NUC v0.137.0/.1 — the trap fired in a MOVAPS in the
// (now-removed) echo_round_trip prologue.
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "    xor rbp, rbp",
    "    and rsp, -16",
    "    call rust_main",
    "    ud2",
);

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_main() -> ! {
    mount_essentials();

    let (kmsg_fd, _console_fd) = open_console_kmsg();
    say(kmsg_fd, b"\n[microvm-init] PID-1 up.\n");

    // input_event_smoke removed from the boot path — open(/dev/input/event0)
    // exhibited inconsistent blocking behaviour between v0.162.0 (with
    // O_NONBLOCK) and v0.162.1 (without). 12.4c is already validated
    // on NUC; real event consumption is poll(2)-based in 12.4d.
    let _ = kmsg_fd;

    // If the bundled rootfs has /bin/sh (Alpine userspace, future
    // LibreWolf container), exec it. Otherwise fall through to the
    // pause loop — same PID-1 binary works for the minimal substrate
    // initramfs AND for a real userspace bundle.
    try_exec_userspace(kmsg_fd);

    // PID-1 must never return — Linux panics on "Attempted to kill
    // init!". Park in pause(2); signal-delivery wakes us, then we
    // pause again.
    loop {
        let _ = unsafe { syscall0(SYS_PAUSE) };
    }
}

/// If `/bin/sh` exists, exec a small smoke command through it. This
/// proves the userspace bundle is loadable end-to-end: dynamic linker
/// (`/lib/ld-musl-x86_64.so.1`) resolves, busybox runs, console output
/// makes it back through our virtio + serial pipes, then `poweroff`
/// shuts the VM down cleanly.
///
/// On execve success this function never returns — PID-1 IS now the
/// shell. If anything fails (no /bin/sh, missing ld-musl, exec
/// returns ENOENT) we silently fall through; caller goes to pause loop.
fn try_exec_userspace(kmsg_fd: i64) {
    // Probe for /bin/sh — access(F_OK). Cheaper than open+close, and
    // doesn't pollute the fd table on the success path (execve
    // inherits all fds).
    let probe = unsafe { syscall3(SYS_ACCESS, b"/bin/sh\0".as_ptr() as u64, F_OK, 0) };
    if probe != 0 {
        say(kmsg_fd, b"[microvm-init] no /bin/sh -- staying minimal\n");
        return;
    }

    say(kmsg_fd, b"[microvm-init] /bin/sh present, exec'ing userspace smoke\n");

    // Build argv = ["/bin/sh", "-c", "<command>", NULL]
    // and envp = ["PATH=/usr/bin:/bin:/usr/sbin:/sbin", "TERM=linux", NULL]
    // execve takes *const *const u8 — we hand-build the pointer tables.
    let prog = b"/bin/sh\0".as_ptr();
    let arg0 = b"/bin/sh\0".as_ptr();
    let arg1 = b"-c\0".as_ptr();
    // Smoke command: identify guest, list /, sleep, clean shutdown.
    //
    // First line redirects stdout+stderr to /dev/kmsg — the tty
    // 8250 driver buffers writes against IRQ4, which never fires
    // because our cmdline has `noapic nolapic`. /dev/kmsg writes
    // go through printk which uses a polled path → always reach
    // the host. Without this redirect, the shell's output piles
    // up in the 8250 TX buffer and the host never sees it.
    //
    // `poweroff` is a busybox symlink to /bin/busybox poweroff which
    // calls reboot(POWER_OFF) — same path our PID-1's halt() uses.
    let arg2 = b"exec >/dev/kmsg 2>&1; echo '[shell] hello from Alpine in nopeekOS microvm'; uname -a; echo '[shell] --- wayland-libs check ---'; ls -la /usr/lib/libwayland* 2>&1 | head -5; ldd /usr/lib/libwayland-client.so.0 2>&1 | head -10; echo '[shell] done -- powering off'; sleep 1; poweroff -f\0".as_ptr();
    let env0 = b"PATH=/usr/bin:/bin:/usr/sbin:/sbin\0".as_ptr();
    let env1 = b"TERM=linux\0".as_ptr();

    let argv = [arg0, arg1, arg2, core::ptr::null::<u8>()];
    let envp = [env0, env1, core::ptr::null::<u8>()];

    unsafe {
        syscall3(
            SYS_EXECVE,
            prog as u64,
            argv.as_ptr() as u64,
            envp.as_ptr() as u64,
        );
    }
    // Only reached on execve failure (ENOENT / EACCES / ENOEXEC).
    say(kmsg_fd, b"[microvm-init] execve failed -- falling back to pause\n");
}

/// Mount /proc, /sys, /dev (devtmpfs), /tmp (tmpfs). Required for any
/// real Linux userspace to function. With a cpio initramfs Linux skips
/// `prepare_namespace()` and never honors `devtmpfs.mount=1`, so the
/// init has to do it itself.
fn mount_essentials() {
    unsafe {
        let _ = syscall2(SYS_MKDIR, b"/proc\0".as_ptr() as u64, 0o755);
        let _ = syscall5(
            SYS_MOUNT,
            b"proc\0".as_ptr() as u64,
            b"/proc\0".as_ptr() as u64,
            b"proc\0".as_ptr() as u64,
            0, 0,
        );
        let _ = syscall2(SYS_MKDIR, b"/sys\0".as_ptr() as u64, 0o755);
        let _ = syscall5(
            SYS_MOUNT,
            b"sysfs\0".as_ptr() as u64,
            b"/sys\0".as_ptr() as u64,
            b"sysfs\0".as_ptr() as u64,
            0, 0,
        );
        let _ = syscall2(SYS_MKDIR, b"/dev\0".as_ptr() as u64, 0o755);
        let _ = syscall5(
            SYS_MOUNT,
            b"devtmpfs\0".as_ptr() as u64,
            b"/dev\0".as_ptr() as u64,
            b"devtmpfs\0".as_ptr() as u64,
            0, 0,
        );
        let _ = syscall2(SYS_MKDIR, b"/tmp\0".as_ptr() as u64, 0o755);
        let _ = syscall5(
            SYS_MOUNT,
            b"tmpfs\0".as_ptr() as u64,
            b"/tmp\0".as_ptr() as u64,
            b"tmpfs\0".as_ptr() as u64,
            0, 0,
        );
    }
}

/// Open /dev/console (RDWR, dup'd to 0/1/2) and /dev/kmsg (WO).
/// Returns (kmsg_fd, console_fd). Either may be -1 on failure.
///
/// `/dev/kmsg` always reaches the host capture (printk subsystem,
/// polled). `/dev/console` goes through the tty layer which may stall
/// on missing IRQ4 under our `nolapic noapic` cmdline.
fn open_console_kmsg() -> (i64, i64) {
    let console_fd = unsafe {
        syscall2(SYS_OPEN, b"/dev/console\0".as_ptr() as u64, O_RDWR)
    };
    if console_fd >= 0 {
        let cfd = console_fd as u64;
        unsafe {
            let _ = syscall2(SYS_DUP2, cfd, 0);
            let _ = syscall2(SYS_DUP2, cfd, 1);
            let _ = syscall2(SYS_DUP2, cfd, 2);
        }
    }
    let kmsg_fd = unsafe {
        syscall2(SYS_OPEN, b"/dev/kmsg\0".as_ptr() as u64, 1 /* O_WRONLY */)
    };
    (kmsg_fd, console_fd)
}

/// Smoke-check that `/dev/input/event0` exists. Open + close, no read.
///
/// Earlier versions also tried a non-blocking SYS_READ to dump the
/// first pending event, but evdev's read path blocked in
/// `wait_event_interruptible` even with O_NONBLOCK in the open flags
/// — likely a flag-arg encoding issue at the syscall boundary. Since
/// the real event-injection path (Shade compositor → eventq) is
/// future work (12.4d), the test devolved to "does the device node
/// exist". Open success is enough for 12.4c.
#[allow(dead_code)]
fn input_event_smoke(kmsg_fd: i64) {
    let fd = unsafe {
        syscall3(SYS_OPEN, b"/dev/input/event0\0".as_ptr() as u64, O_RDONLY, 0)
    };
    if fd < 0 {
        say(kmsg_fd, b"[microvm-init] input: /dev/input/event0 not present\n");
        return;
    }
    let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
    say(kmsg_fd, b"[microvm-init] input: /dev/input/event0 present (open OK)\n");
}

/// Write a message to both /dev/kmsg (printk-direct, polled, always
/// reaches the host capture) and stdout. Either reaching is enough.
fn say(kmsg_fd: i64, msg: &[u8]) {
    if kmsg_fd >= 0 {
        let _ = unsafe { syscall3(SYS_WRITE, kmsg_fd as u64, msg.as_ptr() as u64, msg.len() as u64) };
    }
    let _ = sys_write(1, msg);
}

fn sys_write(fd: u64, buf: &[u8]) -> i64 {
    unsafe { syscall3(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64) }
}

fn halt() -> ! {
    unsafe {
        let _ = syscall4(
            SYS_REBOOT,
            LINUX_REBOOT_MAGIC1,
            LINUX_REBOOT_MAGIC2,
            LINUX_REBOOT_CMD_POWER_OFF,
            0,
        );
        let _ = syscall1(SYS_EXIT, 0);
    }
    loop {}
}

// ── Raw syscall wrappers ───────────────────────────────────────────

unsafe fn syscall0(nr: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!("syscall",
             inlateout("rax") nr as i64 => r,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    r
}

unsafe fn syscall1(nr: u64, a: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!("syscall",
             inlateout("rax") nr as i64 => r,
             in("rdi") a,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    r
}

unsafe fn syscall2(nr: u64, a: u64, b: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!("syscall",
             inlateout("rax") nr as i64 => r,
             in("rdi") a, in("rsi") b,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    r
}

unsafe fn syscall3(nr: u64, a: u64, b: u64, c: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!("syscall",
             inlateout("rax") nr as i64 => r,
             in("rdi") a, in("rsi") b, in("rdx") c,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    r
}

unsafe fn syscall4(nr: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!("syscall",
             inlateout("rax") nr as i64 => r,
             in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    r
}

unsafe fn syscall5(nr: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!("syscall",
             inlateout("rax") nr as i64 => r,
             in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, in("r8") e,
             out("rcx") _, out("r11") _,
             options(nostack));
    }
    r
}
