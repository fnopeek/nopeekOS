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

    // Hand the framebuffer to the Wayland stack: cage (wlroots kiosk
    // compositor) running LibreWolf, rendered through the pixman
    // software renderer + wlroots DRM backend → /dev/dri/card0 →
    // virtio-gpu → our Shade Surface tile. On success PID-1 becomes
    // the supervising shell and never returns. It returns only if the
    // bundle has no cage (degraded/minimal initramfs) — then just
    // park, so the window still persists (Linux panics if PID-1
    // exits).
    launch_wayland(kmsg_fd);

    say(kmsg_fd, b"[microvm-init] no cage in bundle; parking\n");
    loop {
        let _ = unsafe { syscall0(SYS_PAUSE) };
    }
}

/// Phase B — hand the framebuffer to a real Wayland stack. If the
/// bundle ships `/usr/bin/cage`, exec a shell that sets up the
/// runtime env and runs `cage -- librewolf`:
///   - cage: wlroots kiosk compositor, one fullscreen client — the
///     browser, exactly the one-surface-one-tile target topology.
///   - librewolf: the actual end-goal client (Firefox fork). Needs
///     MOZ_ENABLE_WAYLAND=1 or it tries X11 (there is no X).
///     weston-simple-shm proved the path; this is the real thing.
/// Renderer = pixman (software, no GL/Mesa driver). Backend = wlroots
/// DRM on /dev/dri/card0 (virtio-gpu KMS) → our Surface tile. Seat =
/// the seatd daemon (Alpine's libseat has no builtin backend), its
/// socket on a tmpfs over /run (RO sqfs root). XDG_RUNTIME_DIR on
/// tmpfs for the same reason. Output → /dev/kmsg (8250 TX is never
/// flushed: cmdline is `noapic nolapic`, no IRQ4). On success PID-1
/// becomes the supervising shell and never returns; on absence we
/// return so the caller falls back to the fb_react_loop test.
fn launch_wayland(kmsg_fd: i64) {
    let probe = unsafe { syscall3(SYS_ACCESS, b"/usr/bin/cage\0".as_ptr() as u64, F_OK, 0) };
    if probe != 0 {
        say(kmsg_fd, b"[microvm-init] no /usr/bin/cage -- not a Wayland bundle\n");
        return;
    }
    say(kmsg_fd, b"[microvm-init] cage present, starting Wayland session\n");

    let prog = b"/bin/sh\0".as_ptr();
    let arg0 = b"/bin/sh\0".as_ptr();
    let arg1 = b"-c\0".as_ptr();
    // Clean launch (Phase B validated — colored circles rendered).
    // Earned fixes, all kept: XDG_RUNTIME_DIR + seatd socket on
    // tmpfs (sqfs root is RO); seatd daemon because Alpine libseat
    // has no builtin backend; pixman renderer (no GL); WLR DRM
    // backend on virtio-gpu KMS. seatd.log kept (cheap, the one
    // thing worth seeing if seat ever breaks); per-frame cage debug
    // and the bring-up snapshot loop removed — they cost real guest
    // CPU. cage runs in the foreground; PID-1 parks if it exits so
    // the window/VM stay alive.
    // udevd + `udevadm trigger`/`settle` MUST run before cage:
    // wlroots' libinput backend discovers input devices, and its
    // DRM/session backend discovers the GPU + receives connector
    // HOTPLUG, exclusively through the udev monitor. With no udev
    // wlroots finds zero input devices (keyboard/mouse never reach
    // the browser) and never reacts to the DRM hotplug we raise on a
    // tile resize (no live reflow). `trigger` replays uevents for
    // already-present devices (event0, card0); `settle` waits for
    // /run/udev/data to be populated before cage enumerates. Degrades
    // with a WARN if eudev is absent (older bundle) — cage still
    // starts, just input/hotplug-blind.
    let arg2 = b"exec >/dev/kmsg 2>&1; \
                 NPT=$(sed -n 's/.*nopeektime=\\([0-9][0-9]*\\).*/\\1/p' /proc/cmdline); \
                 if [ -n \"$NPT\" ]; then date -s @\"$NPT\" >/dev/null 2>&1 \
                   && echo \"[wl] clock set from host: $(date -u)\" \
                   || echo '[wl] WARN: date -s failed'; \
                 else echo '[wl] WARN: no nopeektime= on cmdline (TLS will fail)'; fi; \
                 echo 0 > /proc/sys/kernel/print-fatal-signals 2>/dev/null; \
                 echo 1 > /proc/sys/kernel/printk 2>/dev/null; \
                 echo '[wl] print-fatal-signals=0, printk=1 (serial flood was the wedge)'; \
                 (hostname nopeek 2>/dev/null \
                   || echo nopeek > /proc/sys/kernel/hostname 2>/dev/null) \
                   && echo '[wl] hostname=nopeek (silences (none) self-lookup)' \
                   || echo '[wl] WARN: could not set hostname'; \
                 mkdir -p /tmp/xrt; chmod 0700 /tmp/xrt; \
                 mount -t tmpfs -o mode=0755 tmpfs /run \
                   || echo '[wl] WARN: /run tmpfs mount failed'; \
                 mkdir -p /run/udev; \
                 udevd --daemon 2>/dev/null \
                   || echo '[wl] WARN: udevd failed (input + resize-hotplug degraded)'; \
                 udevadm trigger --type=devices --action=add 2>/dev/null; \
                 udevadm settle --timeout=10 2>/dev/null; \
                 echo \"[wl] udev up; input: $(ls /dev/input 2>/dev/null | tr '\\n' ' ')\"; \
                 IFACE=$(for d in /sys/class/net/*; do n=${d##*/}; [ \"$n\" = lo ] || { echo $n; break; }; done); \
                 ip link set \"$IFACE\" up 2>/dev/null \
                   || ifconfig \"$IFACE\" up 2>/dev/null; \
                 ip addr add 10.99.0.2/24 dev \"$IFACE\" 2>/dev/null \
                   || ifconfig \"$IFACE\" 10.99.0.2 netmask 255.255.255.0 2>/dev/null; \
                 ip route add default via 10.99.0.1 2>/dev/null \
                   || route add default gw 10.99.0.1 2>/dev/null; \
                 echo 'nameserver 10.99.0.1' > /tmp/resolv.conf; \
                 mount --bind /tmp/resolv.conf /etc/resolv.conf 2>/dev/null \
                   || echo '[wl] WARN: resolv.conf bind failed'; \
                 echo \"[wl] net: iface=$IFACE; $(ip route 2>/dev/null | tr '\\n' ';')\"; \
                 mkdir -p /tmp/moz; : > /tmp/moz/user.js; \
                 for p in \
                   'browser.tabs.remote.autostart|false' \
                   'fission.autostart|false' \
                   'network.process.enabled|false' \
                   'layers.gpu-process.enabled|false' \
                   'gfx.webrender.software|true' \
                   'security.sandbox.content.level|0' \
                   'extensions.autoDisableScopes|15' \
                   'extensions.startupScanScopes|0' \
                   'toolkit.startup.max_resumed_crashes|-1' \
                   'browser.shell.checkDefaultBrowser|false' \
                   'browser.sessionstore.resume_from_crash|false' \
                   'security.OCSP.enabled|0' \
                   'security.OCSP.require|false' \
                   'network.dns.echconfig.enabled|false' \
                   'gfx.webrender.partial|false' \
                   'gfx.webrender.compositor.force-enabled|false' \
                   'widget.dmabuf.force-enabled|false' \
                   'gfx.canvas.accelerated|false'; \
                 do k=${p%|*}; v=${p#*|}; \
                   echo \"user_pref(\\\"$k\\\", $v);\" >> /tmp/moz/user.js; \
                 done; \
                 export XDG_RUNTIME_DIR=/tmp/xrt XDG_SEAT=seat0 \
                 WLR_RENDERER=pixman WLR_BACKENDS=libinput,drm \
                 LIBSEAT_BACKEND=seatd \
                 XDG_CONFIG_HOME=/tmp HOME=/tmp \
                 MOZ_ENABLE_WAYLAND=1 MOZ_DISABLE_RDD_SANDBOX=1 \
                 MOZ_DISABLE_CONTENT_SANDBOX=1 MOZ_DISABLE_GMP_SANDBOX=1; \
                 seatd -g root > /tmp/seatd.log 2>&1 & \
                 sleep 1; \
                 echo '[wl] launching cage -- librewolf https://example.com'; \
                 cage -- librewolf --no-remote --profile /tmp/moz https://example.com \
                   >/dev/null 2>&1; \
                 echo \"[wl] cage exited rc=$?\"; \
                 while true; do sleep 3600; done\0".as_ptr();
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
    say(kmsg_fd, b"[microvm-init] cage execve failed -- falling back\n");
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

/// Mount /proc, /sys, /dev (devtmpfs), /tmp + /dev/shm (tmpfs).
/// Required for any real Linux userspace to function (and Firefox
/// hard-requires /dev/shm). With a cpio initramfs Linux skips
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
        // /dev/shm — Firefox/Gecko hard-requires POSIX shared memory
        // for content-process IPC. Without it shm_open() fails and a
        // content process null-derefs in libxul (observed: "Privileged
        // Cont segfault at 0 in libxul.so"). /dev is devtmpfs; mount a
        // tmpfs on the /dev/shm subdir, mode 1777 like a real system.
        let _ = syscall2(SYS_MKDIR, b"/dev/shm\0".as_ptr() as u64, 0o1777);
        let _ = syscall5(
            SYS_MOUNT,
            b"tmpfs\0".as_ptr() as u64,
            b"/dev/shm\0".as_ptr() as u64,
            b"tmpfs\0".as_ptr() as u64,
            0,
            b"mode=1777\0".as_ptr() as u64,
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
