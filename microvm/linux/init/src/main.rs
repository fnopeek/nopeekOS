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
const SYS_EXIT: u64 = 60;
const SYS_PAUSE: u64 = 34;
const SYS_REBOOT: u64 = 169;

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
    let _ = sys_write(1, b"\n[microvm-init] Hello from nopeekOS PID-1.\n");
    let _ = sys_write(1, b"[microvm-init] kernel boot reached userspace.\n");
    let _ = sys_write(1, b"[microvm-init] entering idle loop (12.1.3a milestone).\n");

    // PID-1 must never return — the kernel panics on
    // "Attempted to kill init!" otherwise. Park in pause(2) so we
    // don't spin the CPU. Linux signal-delivery wakes us, then we
    // pause again.
    loop {
        let _ = unsafe { syscall0(SYS_PAUSE) };
    }
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
