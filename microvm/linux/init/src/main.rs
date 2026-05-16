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
const SYS_LSEEK: u64 = 8;
const SYS_MKDIR: u64 = 83;
const SYS_CHDIR: u64 = 80;
const SYS_CHROOT: u64 = 161;
const SYS_MOUNT: u64 = 165;
const SYS_REBOOT: u64 = 169;

// mount(2) flags
const MS_RDONLY: u64 = 1;

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

    // If the read-only userspace bundle is present on /dev/vdb
    // (second virtio-blk, slot 5), switch into it. The big bundle
    // (Mesa/cage/LibreWolf) lives compressed on squashfs, decompressed
    // on read — RAM-efficient vs. an unpacked cpio initramfs. On
    // absence/failure we stay in the minimal initramfs (the device
    // comes up empty until the OTA bundle lands).
    if try_switch_to_sqfs(kmsg_fd) {
        // Re-establish /proc /sys /dev /tmp inside the new root. The
        // mountpoints exist in the Alpine image; squashfs is RO so the
        // mkdirs no-op harmlessly.
        mount_essentials();
        say(kmsg_fd, b"[microvm-init] switched to squashfs bundle root\n");
    }

    // 12.4 A4 — Surface-bridge validation WITHOUT a Wayland bundle.
    // Paint /dev/fb0 a solid colour so the host gets non-zero
    // virtio-gpu TRANSFER+FLUSH frames, then DON'T power off — park
    // in the pause loop so the guest stays alive and its window
    // persists (the bound VM no longer idle-exits — kernel side).
    // This is throwaway de-risk: proves clip/stride/pixel-format of
    // the render arm before the expensive LibreWolf bundle. Phase B
    // replaces it with cage + a real Wayland client.
    // Paints /dev/fb0, then reads /dev/input/event0 and toggles the
    // colour on each key press — visibly proves the host→guest
    // virtio-input pipe (Shade Surface branch → eventq). Returns only
    // if fb/event device is absent; then we park so the window still
    // persists.
    fb_react_loop(kmsg_fd);
    say(kmsg_fd, b"[microvm-init] fb/input unavailable; parking (window stays open)\n");
    loop {
        let _ = unsafe { syscall0(SYS_PAUSE) };
    }

    #[allow(unreachable_code)]
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

/// Fill the whole 1280x720 BGRX scanout (= 3 686 400 B = 900 * 4096)
/// with one colour. Seeks to 0 first so it can repaint in place.
/// Small stack buffer written 900× — PID-1 is no_std, no 3.6 MB heap.
/// DRM fbdev deferred-io flushes the dirtied region → RESOURCE_FLUSH.
fn fb_paint(fd: i64, b: u8, g: u8, r: u8) {
    let mut buf = [0u8; 4096];
    let mut i = 0;
    while i < 4096 {
        buf[i] = b;
        buf[i + 1] = g;
        buf[i + 2] = r;
        buf[i + 3] = 0x00; // X
        i += 4;
    }
    let _ = unsafe { syscall3(SYS_LSEEK, fd as u64, 0, 0 /* SEEK_SET */) };
    let mut chunks = 0;
    while chunks < 900 {
        let n = unsafe { syscall3(SYS_WRITE, fd as u64, buf.as_ptr() as u64, 4096) };
        if n <= 0 {
            break;
        }
        chunks += 1;
    }
}

/// 12.4 A4 + virtio-input mini-test. Paint the framebuffer cyan, then
/// block on /dev/input/event0 and toggle cyan↔magenta on every key
/// press. This visibly proves BOTH halves of the bridge without a
/// Wayland bundle: guest→host pixels (Surface tile) AND host→guest
/// input (Shade Surface branch → virtio-input eventq → evdev).
/// Throwaway de-risk; Phase B replaces it with cage + a real client.
///
/// input_event on x86_64: { u64 sec; u64 usec; u16 type; u16 code;
/// u32 value } = 24 bytes. type@16, value@20. EV_KEY=1, press=1.
fn fb_react_loop(kmsg_fd: i64) {
    let fb = unsafe { syscall3(SYS_OPEN, b"/dev/fb0\0".as_ptr() as u64, O_RDWR, 0) };
    if fb < 0 {
        say(kmsg_fd, b"[microvm-init] /dev/fb0 absent -- no fb test pattern\n");
        return;
    }
    // Cyan = B=0xFF G=0xFF R=0 (BGRX LE). If it shows red/blue-swapped
    // we learn the host pixel format cheaply here.
    fb_paint(fb, 0xFF, 0xFF, 0x00);
    say(kmsg_fd, b"[microvm-init] fb painted cyan; reading /dev/input/event0\n");

    let ev = unsafe { syscall3(SYS_OPEN, b"/dev/input/event0\0".as_ptr() as u64, O_RDONLY, 0) };
    if ev < 0 {
        say(kmsg_fd, b"[microvm-init] /dev/input/event0 absent -- static cyan\n");
        let _ = unsafe { syscall1(SYS_CLOSE, fb as u64) };
        return;
    }
    say(kmsg_fd, b"[microvm-init] press any key in the window: colour toggles\n");

    let mut magenta = false;
    let mut rec = [0u8; 24];
    // Throttle EV_ABS motion logs — without this a moving pointer
    // floods /dev/kmsg. First few are enough to prove motion on HW.
    let mut abs_logs: u32 = 0;
    loop {
        // Blocking read — wakes on a delivered virtio-input event.
        let n = unsafe { syscall3(SYS_READ, ev as u64, rec.as_mut_ptr() as u64, 24) };
        if n != 24 {
            // Short/again/error: keep the window alive, retry.
            continue;
        }
        // input_event x86_64: type@16, code@18, value@20.
        let etype = (rec[16] as u16) | ((rec[17] as u16) << 8);
        let code  = (rec[18] as u16) | ((rec[19] as u16) << 8);
        let value = (rec[20] as u32)
            | ((rec[21] as u32) << 8)
            | ((rec[22] as u32) << 16)
            | ((rec[23] as u32) << 24);
        // EV_ABS (3): throttled motion proof.
        if etype == 3 && abs_logs < 6 {
            abs_logs += 1;
            // ABS_X = 0, ABS_Y = 1.
            say(kmsg_fd, if code == 0 {
                b"[microvm-init] pointer abs X moved\n" as &[u8]
            } else {
                b"[microvm-init] pointer abs Y moved\n"
            });
        }
        // EV_KEY (1), press (value == 1). BTN_LEFT (0x110) is the
        // mouse — distinguish it in the log so a click vs a keystroke
        // is unambiguous on HW. Both toggle the colour.
        if etype == 1 && value == 1 {
            let is_click = code == 0x110;
            magenta = !magenta;
            if magenta {
                fb_paint(fb, 0xFF, 0x00, 0xFF); // magenta
                say(kmsg_fd, if is_click {
                    b"[microvm-init] click -> magenta\n" as &[u8]
                } else {
                    b"[microvm-init] key -> magenta\n"
                });
            } else {
                fb_paint(fb, 0xFF, 0xFF, 0x00); // cyan
                say(kmsg_fd, if is_click {
                    b"[microvm-init] click -> cyan\n" as &[u8]
                } else {
                    b"[microvm-init] key -> cyan\n"
                });
            }
        }
    }
}

/// Mount the read-only squashfs userspace bundle from `/dev/vdb` and
/// chroot into it. Returns `true` if we are now running inside the
/// bundle root, `false` if the device is absent or not a valid
/// squashfs (→ caller stays in the minimal initramfs).
///
/// chroot (not pivot_root/MS_MOVE): with squashfs the initramfs holds
/// only our ~1 KB PID-1, so there is no initramfs RAM worth reclaiming
/// — chroot is the lower-risk switch. Open fds (kmsg/console) survive
/// it, so logging keeps working across the boundary.
fn try_switch_to_sqfs(kmsg_fd: i64) -> bool {
    // /dev/vdb only exists once devtmpfs is mounted (done by the
    // initramfs-side mount_essentials before we get here).
    let probe = unsafe { syscall3(SYS_ACCESS, b"/dev/vdb\0".as_ptr() as u64, F_OK, 0) };
    if probe != 0 {
        say(kmsg_fd, b"[microvm-init] no /dev/vdb -- minimal initramfs\n");
        return false;
    }

    unsafe { let _ = syscall2(SYS_MKDIR, b"/newroot\0".as_ptr() as u64, 0o755); }

    let m = unsafe {
        syscall5(
            SYS_MOUNT,
            b"/dev/vdb\0".as_ptr() as u64,
            b"/newroot\0".as_ptr() as u64,
            b"squashfs\0".as_ptr() as u64,
            MS_RDONLY,
            0,
        )
    };
    if m != 0 {
        say(kmsg_fd, b"[microvm-init] /dev/vdb not squashfs -- minimal initramfs\n");
        return false;
    }

    let cr = unsafe { syscall1(SYS_CHROOT, b"/newroot\0".as_ptr() as u64) };
    if cr != 0 {
        say(kmsg_fd, b"[microvm-init] chroot(/newroot) failed\n");
        return false;
    }
    let _ = unsafe { syscall1(SYS_CHDIR, b"/\0".as_ptr() as u64) };
    true
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
