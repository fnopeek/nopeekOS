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
const SYS_MKDIR: u64 = 83;
const SYS_MOUNT: u64 = 165;
const SYS_IOPL: u64 = 172;
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
    // With a cpio initramfs Linux skips `prepare_namespace()` entirely
    // and exec's /init directly out of the unpacked rootfs — so the
    // `devtmpfs.mount=1` cmdline parameter is never honored, /dev is
    // empty, and `console_on_rootfs()` already failed before we got
    // here. We have to mount devtmpfs ourselves before any open() on
    // /dev/* will work.
    unsafe {
        let _ = syscall2(SYS_MKDIR, b"/dev\0".as_ptr() as u64, 0o755);
        let _ = syscall5(
            SYS_MOUNT,
            b"devtmpfs\0".as_ptr() as u64,
            b"/dev\0".as_ptr() as u64,
            b"devtmpfs\0".as_ptr() as u64,
            0,
            0,
        );
    }

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

    // Phase 12.1.4 — inject_console round-trip. Grant ourselves
    // IOPL=3 so we can do raw inb/outb on COM1 (port 0x3F8). With
    // `nolapic noapic` on the cmdline the regular 8250 driver gets
    // no IRQ4 and reads from /dev/console block forever, so we
    // bypass the tty layer entirely. Echo mode triggers when the
    // host pre-injected bytes (LSR.DR=1 at this point); otherwise
    // we fall through to the idle pause loop (12.1.3a behavior).
    let iopl_rc = unsafe { syscall1(SYS_IOPL, 3) };
    if iopl_rc < 0 {
        say(kmsg_fd, b"[microvm-init] iopl(3) failed; skipping echo loop.\n");
    } else if (unsafe { inb(0x3FD) } & 0x01) != 0 {
        say(kmsg_fd, b"[microvm-init] echo round-trip (12.1.4 milestone).\n");
        echo_round_trip();
        say(kmsg_fd, b"[microvm-init] echo done -- powering off.\n");
        halt();
    } else {
        say(kmsg_fd, b"[microvm-init] no input pending; entering idle loop.\n");
    }

    // PID-1 must never return — the kernel panics on
    // "Attempted to kill init!" otherwise. Park in pause(2) so we
    // don't spin the CPU. Linux signal-delivery wakes us, then we
    // pause again.
    loop {
        let _ = unsafe { syscall0(SYS_PAUSE) };
    }
}

/// Drain bytes from COM1 RBR until a newline (or buffer cap), then
/// echo the captured line back through COM1 THR prefixed with
/// `[init] echo: `. Both sides go through port 0x3F8, which is
/// trapped by the host hypervisor — input drains the RX FIFO
/// pre-loaded by `microvm shell`, output flows out as standard
/// `[guest] <line>` capture.
fn echo_round_trip() {
    let mut line: [u8; 64] = [0; 64];
    let mut n: usize = 0;

    loop {
        // Spin on LSR.DR. Each inb is a VM-exit; if the host stops
        // injecting we'd spin forever, so cap at a generous count
        // and bail. In practice the host pre-loads a complete line
        // so we drain it without ever waiting.
        let mut spins: u32 = 0;
        while unsafe { inb(0x3FD) } & 0x01 == 0 {
            spins += 1;
            if spins > 1_000_000 { return; }
        }
        let c = unsafe { inb(0x3F8) };
        if c == b'\n' || c == b'\r' || n == line.len() {
            tx_str(b"[init] echo: ");
            for i in 0..n { tx_putc(line[i]); }
            tx_putc(b'\n');
            return;
        }
        line[n] = c;
        n += 1;
    }
}

fn tx_str(s: &[u8]) {
    for &b in s { tx_putc(b); }
}

fn tx_putc(b: u8) {
    // Wait for THRE before writing. Host's emulated LSR has
    // bit 5 always set so this never spins, but we follow the
    // 8250 protocol so the loop stays correct against any future
    // host backend (real virtio-console, hardware passthrough).
    while unsafe { inb(0x3FD) } & 0x20 == 0 {}
    unsafe { outb(0x3F8, b) };
}

unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe {
        asm!(
            "in al, dx",
            out("al") v,
            in("dx") port,
            options(nostack, preserves_flags),
        );
    }
    v
}

unsafe fn outb(port: u16, val: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("al") val,
            in("dx") port,
            options(nostack, preserves_flags),
        );
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

unsafe fn syscall5(nr: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
    let r: i64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") nr as i64 => r,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            in("r10") d,
            in("r8") e,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    r
}
