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
const SYS_READ: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_OPEN: u64 = 2;
const SYS_CLOSE: u64 = 3;
const SYS_IOCTL: u64 = 16;
const SYS_DUP2: u64 = 33;
const SYS_PAUSE: u64 = 34;
const SYS_SOCKET: u64 = 41;
const SYS_SENDTO: u64 = 44;
const SYS_EXIT: u64 = 60;
const SYS_MKDIR: u64 = 83;
const SYS_MOUNT: u64 = 165;
const SYS_REBOOT: u64 = 169;
const SYS_IOPERM: u64 = 173;

// AF_INET socket
const AF_INET: u64 = 2;
const SOCK_DGRAM: u64 = 2;

// ioctl(2) commands for net interface
const SIOCSIFADDR:  u64 = 0x8916;
const SIOCSIFFLAGS: u64 = 0x8914;

// IFF flags
const IFF_UP: u16 = 0x0001;

// open(2) flags.
const O_RDONLY: u64 = 0;
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

// _start is the ELF entry point. Linux's process-load ABI sets RSP to
// a 16-byte-aligned value pointing at argc, with no return address —
// different from the System V function-call ABI, which expects RSP to
// be 8-misaligned at function entry (because the caller's CALL has
// pushed an 8-byte return address). If we declare _start as a normal
// Rust function the compiler's prologue assumes the function-entry
// alignment, all subsequent stack math is off by 8, and any later
// MOVAPS-on-stack (e.g. compiler-emitted SIMD zero-init of a [u8; 64]
// local) #GPs on misalignment. Verified on NUC v0.137.0/.1: the trap
// fired in echo_round_trip's prologue at `movaps [rsp+0x30], xmm0`,
// not at any inb — both ioperm and inb were red herrings.
//
// Workaround: write _start in pure asm and have it CALL into a normal
// Rust function. The CALL pushes 8 bytes, which exactly establishes
// the alignment Rust's prologue expects. The `and rsp, -16` first is
// defensive — Linux's ABI guarantees 16-aligned at entry, but if a
// future kernel or auxv-extension shifted that we'd silently corrupt.
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "    xor rbp, rbp",       // clear frame pointer (unwinder convention)
    "    and rsp, -16",        // belt-and-braces 16-byte align
    "    call rust_main",      // pushes 8-byte return → 8-misaligned RSP per ABI
    "    ud2",                 // rust_main is `-> !`; if it returns, fault
);

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_main() -> ! {
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

    // Phase 12.2 capstone: read first 32 bytes of /dev/vda (virtio-blk
    // backed by host's 4 MB in-RAM buffer). Magic pattern set by host
    // virtio-blk emulator — round-trip proof from host backing →
    // virtqueue READ → DMA into our buffer → kmsg log.
    blk_read_test(kmsg_fd);

    // Phase 12.3.1: bring eth0 up with the static IP that the host's
    // virtio-net is set up to bridge for, then send one UDP datagram
    // toward the gateway — that forces Linux to ARP-resolve 10.99.0.1
    // and the resulting ARP-Request hits our virtio-net TX path.
    eth0_up(kmsg_fd);
    udp_poke(kmsg_fd);

    // Phase 12.1.4 — inject_console round-trip. Grant ourselves I/O
    // port access on COM1 (0x3F8-0x3FF) via ioperm(2). Modern Linux
    // (≥5.5) made iopl(3) into an emulated stub: the syscall succeeds
    // and even sets EFLAGS.IOPL=3 in the saved frame, but the kernel
    // no longer maps that to ring-0 port access for arbitrary ports —
    // each port range still has to be enabled through the per-task
    // ioperm bitmap. Calling iopl alone passes silently and then the
    // first real `inb` #GPs (verified on NUC v0.137.0). ioperm both
    // checks CAP_SYS_RAWIO and sets up the bitmap; a 0 return
    // actually grants the access. With `nolapic noapic` on the
    // cmdline the regular 8250 driver gets no IRQ4 and reads from
    // /dev/console block forever, so we bypass the tty layer.
    // Echo mode triggers when the host pre-injected bytes
    // (LSR.DR=1 at this point); otherwise we fall through to the
    // idle pause loop (12.1.3a behavior).
    let ioperm_rc = unsafe { syscall3(SYS_IOPERM, 0x3F8, 8, 1) };
    if ioperm_rc < 0 {
        say(kmsg_fd, b"[microvm-init] ioperm(0x3F8, 8, 1) failed; skipping echo loop.\n");
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

/// Open /dev/vda, read 32 bytes from sector 0, dump as ASCII + hex
/// through kmsg. Validates the full virtio-blk path:
///   guest open → blk-mq request → virtqueue notify → host service →
///   used-ring update → IRQ inject → guest read returns.
fn blk_read_test(kmsg_fd: i64) {
    let fd = unsafe { syscall2(SYS_OPEN, b"/dev/vda\0".as_ptr() as u64, O_RDONLY) };
    if fd < 0 {
        say(kmsg_fd, b"[microvm-init] /dev/vda open failed\n");
        return;
    }

    let mut buf = [0u8; 32];
    let n = unsafe { syscall3(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) };
    let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };

    if n < 0 {
        say(kmsg_fd, b"[microvm-init] /dev/vda read failed\n");
        return;
    }
    if n == 0 {
        say(kmsg_fd, b"[microvm-init] /dev/vda EOF on first read\n");
        return;
    }

    // Format "  vda[0..N]: <ascii> | <hex hex hex ...>"
    let mut out: [u8; 256] = [0; 256];
    let mut p: usize = 0;
    let prefix = b"[microvm-init] vda[0..";
    for &b in prefix { if p < out.len() { out[p] = b; p += 1; } }
    p = push_dec(&mut out, p, n as u32);
    let tail1 = b"] ascii=\"";
    for &b in tail1 { if p < out.len() { out[p] = b; p += 1; } }
    let nu = (n as usize).min(buf.len());
    for i in 0..nu {
        let c = buf[i];
        let printable = c >= 0x20 && c < 0x7F;
        if p < out.len() { out[p] = if printable { c } else { b'.' }; p += 1; }
    }
    let tail2 = b"\" hex=";
    for &b in tail2 { if p < out.len() { out[p] = b; p += 1; } }
    for i in 0..nu {
        if p + 3 > out.len() { break; }
        out[p] = hex_nib(buf[i] >> 4); p += 1;
        out[p] = hex_nib(buf[i] & 0xF); p += 1;
        out[p] = b' '; p += 1;
    }
    if p < out.len() { out[p] = b'\n'; p += 1; }
    say(kmsg_fd, &out[..p]);
}

fn push_dec(out: &mut [u8; 256], mut p: usize, mut n: u32) -> usize {
    if n == 0 {
        if p < out.len() { out[p] = b'0'; p += 1; }
        return p;
    }
    let mut tmp = [0u8; 10];
    let mut i = 0;
    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        if p < out.len() { out[p] = tmp[i]; p += 1; }
    }
    p
}

fn hex_nib(n: u8) -> u8 {
    if n < 10 { b'0' + n } else { b'a' + (n - 10) }
}

/// Bring eth0 up with hardcoded IP 10.99.0.2/24 via SIOCSIFADDR +
/// SIOCSIFFLAGS. Builds an ifreq buffer (40 bytes) by hand.
///   off  0..16 = ifr_name "eth0\0..."
///   off 16..40 = ifr_ifru union — for SIOCSIFADDR a sockaddr_in
///                (family u16, port u16, ip[4], pad[8]); for
///                SIOCSIFFLAGS just ifr_flags (i16) at off 16.
fn eth0_up(kmsg_fd: i64) {
    let fd = unsafe { syscall3(SYS_SOCKET, AF_INET, SOCK_DGRAM, 0) };
    if fd < 0 {
        say(kmsg_fd, b"[microvm-init] socket() failed\n");
        return;
    }

    // ifreq for SIOCSIFADDR: name "eth0\0" + sockaddr_in @ off 16
    let mut ifr = [0u8; 40];
    ifr[0] = b'e'; ifr[1] = b't'; ifr[2] = b'h'; ifr[3] = b'0';
    ifr[16] = AF_INET as u8;          // family LE: low byte
    ifr[17] = (AF_INET >> 8) as u8;   // high byte (=0)
    // ip 10.99.0.2 in network byte order — high byte first
    ifr[20] = 10; ifr[21] = 99; ifr[22] = 0; ifr[23] = 2;

    let r = unsafe { syscall3(SYS_IOCTL, fd as u64, SIOCSIFADDR, ifr.as_ptr() as u64) };
    if r < 0 {
        say(kmsg_fd, b"[microvm-init] SIOCSIFADDR failed\n");
        let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
        return;
    }

    // ifreq for SIOCSIFFLAGS: name + ifr_flags (i16 LE) @ off 16
    let mut ifr_flags = [0u8; 40];
    ifr_flags[0] = b'e'; ifr_flags[1] = b't'; ifr_flags[2] = b'h'; ifr_flags[3] = b'0';
    ifr_flags[16] = IFF_UP as u8;
    ifr_flags[17] = (IFF_UP >> 8) as u8;

    let r = unsafe { syscall3(SYS_IOCTL, fd as u64, SIOCSIFFLAGS, ifr_flags.as_ptr() as u64) };
    let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
    if r < 0 {
        say(kmsg_fd, b"[microvm-init] SIOCSIFFLAGS failed\n");
        return;
    }
    say(kmsg_fd, b"[microvm-init] eth0 up @ 10.99.0.2/24\n");
}

/// Send one UDP datagram to the gateway. Linux's IP stack does
/// route lookup → on-link → ARP-resolve 10.99.0.1 → ARP-Request on
/// the wire. The send itself probably fails (no answer), but the
/// ARP-Request fires our virtio-net TX path so we see it logged.
fn udp_poke(kmsg_fd: i64) {
    let fd = unsafe { syscall3(SYS_SOCKET, AF_INET, SOCK_DGRAM, 0) };
    if fd < 0 {
        say(kmsg_fd, b"[microvm-init] udp socket failed\n");
        return;
    }

    // sockaddr_in for 10.99.0.1:53
    let mut sa = [0u8; 16];
    sa[0] = AF_INET as u8;
    sa[1] = (AF_INET >> 8) as u8;
    sa[2] = 0; sa[3] = 53;                            // port 53 (NBO: hi,lo)
    sa[4] = 10; sa[5] = 99; sa[6] = 0; sa[7] = 1;     // ip (NBO)

    let payload = b"npk-poke";
    let n = unsafe {
        syscall6(
            SYS_SENDTO,
            fd as u64,
            payload.as_ptr() as u64,
            payload.len() as u64,
            0,
            sa.as_ptr() as u64,
            sa.len() as u64,
        )
    };
    let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };

    if n < 0 {
        say(kmsg_fd, b"[microvm-init] udp poke sent (errno; ARP queued)\n");
    } else {
        say(kmsg_fd, b"[microvm-init] udp poke sent\n");
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

unsafe fn syscall6(nr: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> i64 {
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
            in("r9") f,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    r
}
