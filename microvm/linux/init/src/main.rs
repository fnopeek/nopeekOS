//! microvm-init — nopeekOS Linux MicroVM PID-1.
//!
//! Statically linked, no_std, no libc. Talks to the Linux kernel
//! exclusively via raw syscalls (x86_64 ABI: rax=nr, rdi/rsi/rdx/r10
//! /r8/r9 = args, syscall, rax = result).
//!
//! Phase 12.1.3a target: print a hello banner on /dev/console, then
//! sit in an idle loop. Phase 12.1.4 will replace the banner with a
//! virtio-console reader/writer round-trip; later phases load a
//! real shell from the read-only sqfs container.

#![no_std]
#![no_main]

use core::arch::asm;

// x86_64 Linux syscall numbers — copy of asm-generic/unistd.h.
const SYS_WRITE: u64 = 1;
const SYS_OPEN: u64 = 2;
const SYS_DUP2: u64 = 33;
const SYS_PAUSE: u64 = 34;
const SYS_EXIT: u64 = 60;
const SYS_REBOOT: u64 = 169;

// open(2) flags.
const O_WRONLY: u64 = 1;
const O_RDWR: u64 = 2;

// reboot(2) magic — see linux/reboot.h. POWER_OFF cleanly halts the
// VM via Linux's machine_power_off path; matches the `panic=1` flow.
const LINUX_REBOOT_MAGIC1: u64 = 0xfee1dead;
const LINUX_REBOOT_MAGIC2: u64 = 0x28121969;
const LINUX_REBOOT_CMD_POWER_OFF: u64 = 0x4321fedc;

#[panic_handler]
fn on_panic(_info: &core::panic::PanicInfo) -> ! {
    let _ = sys_write(2, b"microvm-init panic\n");
    halt();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    // Linux kernel exec's PID-1 with whatever fds it set up via
    // `console_on_rootfs()` — but only if /dev/console exists at the
    // time. With cpio initramfs, /dev is empty unless cmdline has
    // `devtmpfs.mount=1` (which we now set). Defensive fallback:
    // explicitly open /dev/console O_RDWR and dup2 it over 0/1/2 so
    // print works regardless of how the kernel set us up.
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

    // /dev/kmsg always works regardless of tty buffering / IRQ state:
    // writes to it go through Linux's `printk` subsystem, which uses
    // the same polled-write path as kernel log. With our `noapic
    // nolapic` cmdline the tty layer can't drain because there's no
    // IRQ4 — but printk doesn't need one. So even if /dev/console
    // writes get stuck in the tty TX buffer, /dev/kmsg writes always
    // reach our [guest] capture. Best-effort fallback channel; -1
    // here is non-fatal, we still try /dev/console.
    let kmsg_fd = unsafe {
        syscall2(SYS_OPEN, b"/dev/kmsg\0".as_ptr() as u64, O_WRONLY)
    };

    say(kmsg_fd, b"\n[microvm-init] Hello from nopeekOS PID-1.\n");
    say(kmsg_fd, b"[microvm-init] kernel boot reached userspace.\n");
    say(kmsg_fd, b"[microvm-init] entering idle loop (12.1.3a milestone).\n");

    // PID-1 must never return — the kernel panics on
    // "Attempted to kill init!" otherwise. Park in pause(2) so we
    // don't spin the CPU. Linux signal-delivery wakes us, then we
    // pause again.
    loop {
        let _ = unsafe { syscall0(SYS_PAUSE) };
    }
}

/// Write a message to both /dev/kmsg (printk-direct, polled, always
/// reaches the host capture) and stdout (/dev/console via tty layer,
/// may be buffered behind IRQ4 which doesn't fire on our hypervisor).
/// Either reaching the host is enough.
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
        // Fall back to exit if reboot returns (it shouldn't for PID 1).
        let _ = syscall1(SYS_EXIT, 0);
    }
    loop {}
}

unsafe fn syscall0(nr: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") nr as i64 => r,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    r
}

unsafe fn syscall2(nr: u64, a: u64, b: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") nr as i64 => r,
            in("rdi") a,
            in("rsi") b,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    r
}

unsafe fn syscall1(nr: u64, a: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") nr as i64 => r,
            in("rdi") a,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    r
}

unsafe fn syscall3(nr: u64, a: u64, b: u64, c: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") nr as i64 => r,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    r
}

unsafe fn syscall4(nr: u64, a: u64, b: u64, c: u64, d: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") nr as i64 => r,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            in("r10") d,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    r
}
