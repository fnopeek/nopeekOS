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
#[allow(dead_code)] // part of the syscall table; a map with holes is worse
const SYS_LSEEK: u64 = 8;
const SYS_MKDIR: u64 = 83;
const SYS_CHDIR: u64 = 80;
const SYS_CHROOT: u64 = 161;
const SYS_MOUNT: u64 = 165;
const SYS_REBOOT: u64 = 169;
const SYS_FORK: u64 = 57;
const SYS_GETDENTS64: u64 = 217;
const SYS_NANOSLEEP: u64 = 35;
const SYS_SCHED_SETSCHEDULER: u64 = 144;

// O_DIRECTORY for opendir-style reads, SCHED_RR for the RT promoter.
const O_DIRECTORY: u64 = 0o200000;
const SCHED_RR: u64 = 2;

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

    // Diagnostic: if the host passed `nopeekbench=` on the cmdline, run a pure
    // busybox download through the nat bridge (no cage/GPU/browser) so the
    // BRIDGE can be measured in isolation, then halt. Bisects bridge vs
    // browser-userspace for the loaded-latency hunt.
    if bench_requested() {
        launch_bench(kmsg_fd);
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
    // Standard LibreWolf config — only the prefs our env actually
    // demands (no GPU / GL → software webrender; userChrome.css must
    // load to hide the titlebar buttons that crash the browser when
    // clicked under cage; dark mode the user asked for). Everything
    // else stays default: e10s, fission, content/RDD/GMP sandboxes,
    // OCSP, telemetry, addons — like a fresh install. The previous
    // crippled set masked real bugs; we'd rather debug LibreWolf with
    // a real LibreWolf.
    // Measure what ARRIVED, not what was asked for. The old script divided a
    // hardcoded 150 MB by the elapsed time and threw wget's exit status away, so
    // a connection that failed in 50 ms printed "24000 Mbit" — a measuring tool
    // that reports success for a failure is worse than none. It also ignored
    // `nopeekbench=<MB>` entirely and always asked for 150.
    let arg2 = b"exec >/dev/kmsg 2>&1; \
                 IFACE=$(for d in /sys/class/net/*; do n=${d##*/}; [ \"$n\" = lo ] || { echo $n; break; }; done); \
                 ip link set \"$IFACE\" up 2>/dev/null || ifconfig \"$IFACE\" up 2>/dev/null; \
                 ip addr add 10.99.0.2/24 dev \"$IFACE\" 2>/dev/null \
                   || ifconfig \"$IFACE\" 10.99.0.2 netmask 255.255.255.0 2>/dev/null; \
                 ip route add default via 10.99.0.1 2>/dev/null \
                   || route add default gw 10.99.0.1 2>/dev/null; \
                 H=$(tr ' ' '\\n' < /proc/cmdline | sed -n 's/^nopeekbenchhost=//p'); \
                 [ -z \"$H\" ] && H=10.0.2.2; \
                 MB=$(tr ' ' '\\n' < /proc/cmdline | sed -n 's/^nopeekbench=//p'); \
                 case \"$MB\" in ''|*[!0-9]*) MB=50 ;; esac; \
                 echo \"<0>[netbench-vm] iface=$IFACE server=$H size=${MB}MB\" > /dev/kmsg; \
                 echo \"<0>[netbench-vm] === idle RTT (ping x8) ===\" > /dev/kmsg; \
                 ping -c 8 \"$H\" 2>&1 | tail -3; \
                 echo 1 > /proc/sys/net/ipv4/tcp_no_metrics_save 2>/dev/null; \
                 echo \"<0>[netbench-vm] --- guest Tcp: BEFORE (compare InSegs with the host's host-to-guest count) ---\" > /dev/kmsg; \
                 grep '^Tcp:' /proc/net/snmp; \
                 echo \"<0>[netbench-vm] === GET x2 wget + x2 nc - BYTES COUNTED, not assumed ===\" > /dev/kmsg; \
                 for I in 1 2; do \
                   T0=$(cut -d' ' -f1 /proc/uptime); \
                   N=$(wget -q -O - \"http://$H/get?mb=$MB\" 2>/dev/null | wc -c); \
                   T1=$(cut -d' ' -f1 /proc/uptime); \
                   awk -v a=$T0 -v b=$T1 -v i=$I -v n=$N -v w=$MB 'BEGIN{d=b-a;if(d<=0)d=0.001; \
                     if(n<1){printf \"<0>[netbench-vm] GET wget#%s FAILED after %.2fs - 0 bytes (server unreachable, or the bridge dropped it)\\n\",i,d} \
                     else{printf \"<0>[netbench-vm] GET wget#%s %d of %d MB in %.2fs = %.0f Mbit\\n\",i,n/1048576,w,d,n*8/d/1000000}}' > /dev/kmsg; \
                 done; \
                 for I in 1 2; do \
                   T0=$(cut -d' ' -f1 /proc/uptime); \
                   N=$(printf 'GET /get?mb='\"$MB\"' HTTP/1.0\\r\\nHost: '\"$H\"'\\r\\n\\r\\n' | nc -w 20 \"$H\" 80 2>/dev/null | wc -c); \
                   T1=$(cut -d' ' -f1 /proc/uptime); \
                   awk -v a=$T0 -v b=$T1 -v i=$I -v n=$N -v w=$MB 'BEGIN{d=b-a;if(d<=0)d=0.001; \
                     if(n<1){printf \"<0>[netbench-vm] GET nc#%s FAILED after %.2fs - 0 bytes\\n\",i,d} \
                     else{printf \"<0>[netbench-vm] GET nc#%s %d of %d MB in %.2fs = %.0f Mbit\\n\",i,n/1048576,w,d,n*8/d/1000000}}' > /dev/kmsg; \
                 done; \
                 echo \"<0>[netbench-vm] === PUT ${MB} MB ===\" > /dev/kmsg; \
                 B=$((MB*1024*1024)); \
                 T0=$(cut -d' ' -f1 /proc/uptime); \
                 { printf 'POST /upload HTTP/1.1\\r\\nHost: '\"$H\"'\\r\\nContent-Length: %d\\r\\nConnection: close\\r\\n\\r\\n' \"$B\"; \
                   dd if=/dev/zero bs=1M count=$MB 2>/dev/null; } | nc -w 30 \"$H\" 80 > /dev/null 2>&1; \
                 T1=$(cut -d' ' -f1 /proc/uptime); \
                 awk -v a=$T0 -v b=$T1 -v w=$MB 'BEGIN{d=b-a;if(d<=0)d=0.001;printf \"<0>[netbench-vm] PUT %d MB in %.2fs = %.0f Mbit (check the server logged it)\\n\",w,d,w*8/d}' > /dev/kmsg; \
                 echo \"<0>[netbench-vm] --- guest Tcp: AFTER ---\" > /dev/kmsg; \
                 grep '^Tcp:' /proc/net/snmp; \
                 echo \"<0>[netbench-vm] --- named loss/reorder counters (netstat -s) ---\" > /dev/kmsg; \
                 netstat -s 2>/dev/null | grep -iE 'reorder|out.of.order|retrans|sack|lost|recover|prune|collaps|drop|fail|dupl|checksum'; \
                 echo \"<0>[netbench-vm] --- TcpExt + IpExt counters (value > 100) ---\" > /dev/kmsg; \
                 awk '/^(TcpExt|IpExt):/{if(!h[$1]){for(i=2;i<=NF;i++)n[$1,i]=$i;h[$1]=1;next}for(i=2;i<=NF;i++)if($i+0>100)print $1,n[$1,i],$i}' /proc/net/netstat; \
                 echo \"<0>[netbench-vm] done -- halting\" > /dev/kmsg; \
                 sync 2>/dev/null; halt -f 2>/dev/null; poweroff -f 2>/dev/null; \
                 while true; do sleep 3600; done\0".as_ptr();
    let env0 = b"PATH=/usr/bin:/bin:/usr/sbin:/sbin\0".as_ptr();
    let env1 = b"TERM=linux\0".as_ptr();

    let argv = [arg0, arg1, arg2, core::ptr::null::<u8>()];
    let envp = [env0, env1, core::ptr::null::<u8>()];

    // Fork the RT-promoter before becoming the shell. The child loops on
    // /proc promoting cubeb/AudioIPC threads to SCHED_RR from outside the
    // sandbox; the parent execs cage+librewolf as usual.
    unsafe { spawn_rt_watcher(kmsg_fd); }

    // Fork the guest-side diagnostic probe (only if the host asked via
    // `nopeekgdiag` on the cmdline). It dumps the guest's INTERNAL view every
    // second — the inside angle we never had while chasing the download latency.
    if gdiag_requested() {
        unsafe { spawn_gdiag(kmsg_fd); }
    }

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

/// True if the kernel cmdline contains `nopeekbench` — the host asked for a
/// pure-bridge throughput run instead of the browser.
fn bench_requested() -> bool {
    let fd = unsafe {
        syscall3(SYS_OPEN, b"/proc/cmdline\0".as_ptr() as u64, 0 /*O_RDONLY*/, 0)
    };
    if fd < 0 { return false; }
    let mut buf = [0u8; 1024];
    let n = unsafe {
        syscall3(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    };
    let _ = unsafe { syscall3(SYS_CLOSE, fd as u64, 0, 0) };
    if n <= 0 { return false; }
    let s = &buf[..n as usize];
    let needle = b"nopeekbench";
    s.windows(needle.len()).any(|w| w == needle)
}

/// Pure-bridge throughput run: bring up eth0, wget a big file three times from
/// the local server (10.0.2.2 via slirp) through our nat bridge — no cage, no
/// GPU, no browser. The SERVER reports the authoritative rate; guest /proc/
/// uptime gives a cross-check. Then halt. `MB` read from the cmdline.
fn launch_bench(kmsg_fd: i64) {
    say(kmsg_fd, b"[microvm-init] netbench mode: pure-bridge throughput run\n");
    let prog = b"/bin/sh\0".as_ptr();
    let arg0 = b"/bin/sh\0".as_ptr();
    let arg1 = b"-c\0".as_ptr();
    let arg2 = b"exec >/dev/kmsg 2>&1; \
                 IFACE=$(for d in /sys/class/net/*; do n=${d##*/}; [ \"$n\" = lo ] || { echo $n; break; }; done); \
                 ip link set \"$IFACE\" up 2>/dev/null || ifconfig \"$IFACE\" up 2>/dev/null; \
                 ip addr add 10.99.0.2/24 dev \"$IFACE\" 2>/dev/null \
                   || ifconfig \"$IFACE\" 10.99.0.2 netmask 255.255.255.0 2>/dev/null; \
                 ip route add default via 10.99.0.1 2>/dev/null \
                   || route add default gw 10.99.0.1 2>/dev/null; \
                 H=$(tr ' ' '\\n' < /proc/cmdline | sed -n 's/^nopeekbenchhost=//p'); \
                 [ -z \"$H\" ] && H=10.0.2.2; \
                 echo \"<0>[netbench-vm] iface=$IFACE server=$H -- SHORT bench (run 'cores' during a GET) ===\" > /dev/kmsg; \
                 echo \"<0>[netbench-vm] === idle RTT (ping x8) ===\" > /dev/kmsg; \
                 ping -c 8 \"$H\" 2>&1 | tail -3; \
                 echo 1 > /proc/sys/net/ipv4/tcp_no_metrics_save 2>/dev/null; \
                 echo 1 > /proc/sys/net/ipv4/tcp_no_ssthresh_metrics_save 2>/dev/null; \
                 NMS=$(cat /proc/sys/net/ipv4/tcp_no_metrics_save 2>/dev/null); \
                 echo \"<0>[netbench-vm] === GET 3x wget + 3x nc (is busybox wget the bottleneck?) ===\" > /dev/kmsg; \
                 for I in 1 2 3; do \
                   T0=$(cut -d' ' -f1 /proc/uptime); \
                   wget -q -O /dev/null \"http://$H/get?mb=150\" 2>/dev/null; \
                   T1=$(cut -d' ' -f1 /proc/uptime); \
                   awk -v a=$T0 -v b=$T1 -v i=$I 'BEGIN{d=b-a;if(d<=0)d=0.001;printf \"<0>[netbench-vm] GET wget#%s 150MB: %.2fs = %.0f Mbit\\n\",i,d,150*8/d}' > /dev/kmsg; \
                 done; \
                 for I in 1 2 3; do \
                   T0=$(cut -d' ' -f1 /proc/uptime); \
                   printf 'GET /get?mb=150 HTTP/1.0\\r\\nHost: '\"$H\"'\\r\\n\\r\\n' | nc -w 20 \"$H\" 80 > /dev/null 2>&1; \
                   T1=$(cut -d' ' -f1 /proc/uptime); \
                   awk -v a=$T0 -v b=$T1 -v i=$I 'BEGIN{d=b-a;if(d<=0)d=0.001;printf \"<0>[netbench-vm] GET nc#%s 150MB: %.2fs = %.0f Mbit\\n\",i,d,150*8/d}' > /dev/kmsg; \
                 done; \
                 echo \"<0>[netbench-vm] === PUT 300 (+ mid-flight upload-socket cwnd/rtt) ===\" > /dev/kmsg; \
                 B=$((300*1024*1024)); \
                 T0=$(cut -d' ' -f1 /proc/uptime); \
                 { printf 'POST /upload HTTP/1.1\\r\\nHost: '\"$H\"'\\r\\nContent-Length: %d\\r\\nConnection: close\\r\\n\\r\\n' \"$B\"; \
                   dd if=/dev/zero bs=1M count=300 2>/dev/null; } | nc \"$H\" 80 & \
                 NCJOB=$!; \
                 sleep 2; \
                 if command -v ss >/dev/null 2>&1; then \
                   echo \"<0>[netbench-vm] --- ss -tin upload socket (mid-flight: snd_cwnd/rtt) ---\" > /dev/kmsg; \
                   ss -tin 2>/dev/null | grep -A1 \"$H:80\"; \
                 else echo \"<0>[netbench-vm] ss absent (busybox) -- read host 'cores' net TX + worker BUSY% for b1/b2\" > /dev/kmsg; fi; \
                 wait $NCJOB; \
                 T1=$(cut -d' ' -f1 /proc/uptime); \
                 echo \"<0>[netbench-vm] PUT 300 MB: guest uptime $T0 -> $T1\" > /dev/kmsg; \
                 echo \"<0>[netbench-vm] === guest TCP counters (loss/reorder evidence) ===\" > /dev/kmsg; \
                 grep '^Tcp:' /proc/net/snmp; \
                 echo \"<0>[netbench-vm] --- named loss/reorder counters (netstat -s) ---\" > /dev/kmsg; \
                 netstat -s 2>/dev/null | grep -iE 'reorder|out.of.order|retrans|sack|lost|recover|prune|collaps|drop|fail|dupl'; \
                 echo \"<0>[netbench-vm] --- named TcpExt counters (value > 100) ---\" > /dev/kmsg; \
                 awk '/^TcpExt:/{if(!h){for(i=2;i<=NF;i++)n[i]=$i;h=1;next}for(i=2;i<=NF;i++)if($i+0>100)print n[i],$i}' /proc/net/netstat; \
                 echo \"<0>[netbench-vm] done -- halting\" > /dev/kmsg; \
                 sync 2>/dev/null; halt -f 2>/dev/null; poweroff -f 2>/dev/null; \
                 while true; do sleep 3600; done\0".as_ptr();
    let env0 = b"PATH=/usr/bin:/bin:/usr/sbin:/sbin\0".as_ptr();
    let env1 = b"TERM=linux\0".as_ptr();
    let argv = [arg0, arg1, arg2, core::ptr::null::<u8>()];
    let envp = [env0, env1, core::ptr::null::<u8>()];
    // Fork the guest-side probe so [gdiag] runs alongside the bench — the clean
    // measurement (raw wget/nc, no browser compute). The decisive question for the
    // lottery: during a SLOW GET is the guest CPU-bound, or IDLE and waiting on us?
    if gdiag_requested() {
        unsafe { spawn_gdiag(kmsg_fd); }
    }
    unsafe {
        syscall3(
            SYS_EXECVE,
            prog as u64,
            argv.as_ptr() as u64,
            envp.as_ptr() as u64,
        );
    }
    say(kmsg_fd, b"[microvm-init] netbench execve failed\n");
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

// ── Real-time audio-thread promoter ────────────────────────────────
//
// LibreWolf's audio thread needs SCHED_RR to avoid underruns under our
// software-everything CPU load, but inside the content sandbox it can't
// promote itself (seccomp blocks sched_setscheduler) and we have no
// rtkit/D-Bus broker. So PID-1 forks a tiny watcher that, from OUTSIDE
// the sandbox (as root), scans /proc for the cubeb/AudioIPC threads and
// sched_setscheduler's them to SCHED_RR. Root-from-outside is NOT
// blocked by the target's seccomp filter → the content sandbox stays on.
// Runs as a separate child: a bug here can't take down PID-1.

static mut RT_SEEN: [i32; 96] = [0; 96];
static mut RT_SEEN_N: usize = 0;

unsafe fn rt_already(tid: i32) -> bool {
    unsafe {
        // Raw-pointer access: edition 2024 denies references to `static mut`.
        let base = (&raw mut RT_SEEN) as *mut i32;
        let np = &raw mut RT_SEEN_N;
        let n = *np;
        let mut i = 0usize;
        while i < n {
            if *base.add(i) == tid { return true; }
            i += 1;
        }
        if n < 96 { *base.add(n) = tid; *np = n + 1; }
        false
    }
}

fn rt_match(comm: &[u8]) -> bool {
    byte_find(comm, b"cubeb") || byte_find(comm, b"AudioIPC")
}

fn byte_find(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() { return false; }
    let mut i = 0;
    // Manual byte compare — slice `==` would emit a bcmp/memcmp call that
    // this freestanding (no libc) binary can't link.
    while i + needle.len() <= hay.len() {
        let mut j = 0;
        while j < needle.len() && hay[i + j] == needle[j] { j += 1; }
        if j == needle.len() { return true; }
        i += 1;
    }
    false
}

/// Fork the RT watcher. Parent returns immediately; the child never returns.
unsafe fn spawn_rt_watcher(kmsg_fd: i64) {
    let pid = unsafe { syscall0(SYS_FORK) };
    if pid == 0 {
        unsafe { rt_watcher_loop(kmsg_fd); }
    }
}

/// True if the kernel cmdline contains `nopeekgdiag` — the host asked for the
/// guest-side diagnostic probe (per-vCPU busy/softirq %, download-socket TCP
/// state, softnet drops/squeeze every second).
fn gdiag_requested() -> bool {
    let fd = unsafe {
        syscall3(SYS_OPEN, b"/proc/cmdline\0".as_ptr() as u64, 0, 0)
    };
    if fd < 0 { return false; }
    let mut buf = [0u8; 1024];
    let n = unsafe {
        syscall3(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
    };
    let _ = unsafe { syscall3(SYS_CLOSE, fd as u64, 0, 0) };
    if n <= 0 { return false; }
    let hay = &buf[..n as usize];
    let needle = b"nopeekgdiag";
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Fork a busybox loop that dumps the GUEST's internal state to /dev/kmsg every
/// second — the inside view we never had. Per pass:
///   cpu(busy/sirq%)  — per-vCPU busy% and softirq% from /proc/stat deltas: is a
///                      vCPU pegged, and is it pegged on network softirq (= the
///                      guest is the RX bottleneck, not our bridge)?
///   sock             — every socket's cwnd/rtt/retrans (the bulk download flow's
///                      TCP state from inside the guest).
///   softnet          — /proc/net/softnet_stat: col2 = drops, col3 = times the
///                      NAPI poll ran out of budget (squeeze = guest can't drain).
/// The child execs /bin/sh; on exec failure it parks (never falls back into the
/// parent's cage launch).
unsafe fn spawn_gdiag(kmsg_fd: i64) {
    let pid = unsafe { syscall0(SYS_FORK) };
    if pid != 0 {
        return; // parent continues to cage
    }
    let prog = b"/bin/sh\0".as_ptr();
    let arg0 = b"/bin/sh\0".as_ptr();
    let arg1 = b"-c\0".as_ptr();
    let arg2 = b"exec >/dev/kmsg 2>&1; \
                 echo '<0>[gdiag] guest-side probe up (1s cadence)'; \
                 while true; do \
                   A=$(grep '^cpu[0-9]' /proc/stat); \
                   sleep 1; \
                   B=$(grep '^cpu[0-9]' /proc/stat); \
                   CPU=$( { echo \"$A\"; echo =; echo \"$B\"; } | awk '/^=$/{s=1;next} !s{for(i=2;i<=11;i++)p[$1,i]=$i;next} {t=0;for(i=2;i<=11;i++)t+=$i-p[$1,i];id=$5-p[$1,5];sq=$8-p[$1,8];if(t>0)printf \"%s=%d/%d \",$1,int(100*(t-id)/t+0.5),int(100*sq/t+0.5)}' ); \
                   echo \"<0>[gdiag] cpu(busy/sirq%): $CPU\"; \
                   echo \"<0>[gdiag] sock: $(ss -tin 2>/dev/null | grep -oE 'cwnd:[0-9]+|rtt:[0-9.]+|retrans:[0-9/]+|bytes_acked:[0-9]+' | tr '\\n' ' ')\"; \
                   echo \"<0>[gdiag] softnet(proc/drop/squeeze): $(tr '\\n' '|' < /proc/net/softnet_stat)\"; \
                 done\0".as_ptr();
    let env0 = b"PATH=/usr/bin:/bin:/usr/sbin:/sbin\0".as_ptr();
    let argv = [arg0, arg1, arg2, core::ptr::null::<u8>()];
    let envp = [env0, core::ptr::null::<u8>()];
    unsafe {
        syscall3(
            SYS_EXECVE,
            prog as u64,
            argv.as_ptr() as u64,
            envp.as_ptr() as u64,
        );
    }
    // execve failed — park forever so the child NEVER falls back into the
    // parent's cage launch (a double cage exec).
    say(kmsg_fd, b"[gdiag] /bin/sh execve failed -- probe off\n");
    loop {
        let _ = unsafe { syscall0(SYS_PAUSE) };
    }
}

unsafe fn rt_watcher_loop(kmsg_fd: i64) -> ! {
    loop {
        unsafe { rt_scan_proc(kmsg_fd); }
        // ~50 ms between sweeps: catch a freshly-spawned cubeb/AudioIPC
        // thread and give it SCHED_RR before it underruns its first periods
        // (the 500 ms sweep raced — audio worked only when promotion beat the
        // first buffering gap). Scan is cheap + we have idle headroom now.
        let ts: [i64; 2] = [0, 50_000_000];
        unsafe { let _ = syscall2(SYS_NANOSLEEP, ts.as_ptr() as u64, 0); }
    }
}

unsafe fn rt_scan_proc(kmsg_fd: i64) {
    let fd = unsafe { syscall2(SYS_OPEN, b"/proc\0".as_ptr() as u64, O_RDONLY | O_DIRECTORY) };
    if fd < 0 { return; }
    let mut buf = [0u8; 8192];
    loop {
        let n = unsafe { syscall3(SYS_GETDENTS64, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) };
        if n <= 0 { break; }
        let n = n as usize;
        let mut off = 0usize;
        // linux_dirent64: d_ino(8) d_off(8) d_reclen@16(2) d_type@18(1) d_name@19
        while off + 19 <= n {
            let reclen = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]) as usize;
            if reclen < 19 || off + reclen > n { break; }
            let name = &buf[off + 19..off + reclen];
            if name[0] >= b'0' && name[0] <= b'9' {
                unsafe { rt_scan_tasks(name, kmsg_fd); }
            }
            off += reclen;
        }
    }
    unsafe { let _ = syscall1(SYS_CLOSE, fd as u64); }
}

unsafe fn rt_scan_tasks(pid: &[u8], kmsg_fd: i64) {
    // "/proc/<pid>/task\0"
    let mut path = [0u8; 64];
    let mut p = 0usize;
    for &b in b"/proc/" { path[p] = b; p += 1; }
    for &b in pid { if b == 0 { break; } if p >= 56 { return; } path[p] = b; p += 1; }
    for &b in b"/task\0" { path[p] = b; p += 1; }
    let fd = unsafe { syscall2(SYS_OPEN, path.as_ptr() as u64, O_RDONLY | O_DIRECTORY) };
    if fd < 0 { return; }
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { syscall3(SYS_GETDENTS64, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) };
        if n <= 0 { break; }
        let n = n as usize;
        let mut off = 0usize;
        while off + 19 <= n {
            let reclen = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]) as usize;
            if reclen < 19 || off + reclen > n { break; }
            let tid = &buf[off + 19..off + reclen];
            if tid[0] >= b'0' && tid[0] <= b'9' {
                unsafe { rt_check_thread(pid, tid, kmsg_fd); }
            }
            off += reclen;
        }
    }
    unsafe { let _ = syscall1(SYS_CLOSE, fd as u64); }
}

unsafe fn rt_check_thread(pid: &[u8], tid: &[u8], kmsg_fd: i64) {
    // "/proc/<pid>/task/<tid>/comm\0" + parse tid as int.
    let mut path = [0u8; 80];
    let mut p = 0usize;
    for &b in b"/proc/" { path[p] = b; p += 1; }
    for &b in pid { if b == 0 { break; } if p >= 40 { return; } path[p] = b; p += 1; }
    for &b in b"/task/" { path[p] = b; p += 1; }
    let mut tidnum: i32 = 0;
    for &b in tid {
        if b == 0 { break; }
        if p >= 70 || b < b'0' || b > b'9' { return; }
        path[p] = b; p += 1;
        tidnum = tidnum.wrapping_mul(10).wrapping_add((b - b'0') as i32);
    }
    for &b in b"/comm\0" { path[p] = b; p += 1; }

    let fd = unsafe { syscall2(SYS_OPEN, path.as_ptr() as u64, O_RDONLY) };
    if fd < 0 { return; }
    let mut comm = [0u8; 32];
    let r = unsafe { syscall3(SYS_READ, fd as u64, comm.as_mut_ptr() as u64, comm.len() as u64) };
    unsafe { let _ = syscall1(SYS_CLOSE, fd as u64); }
    if r <= 0 { return; }
    let comm = &comm[..r as usize];
    if !rt_match(comm) { return; }
    let _ = kmsg_fd;
    // Promote each matching thread once (idempotent; skip if already done).
    if unsafe { rt_already(tidnum) } { return; }
    let param: [i32; 1] = [2]; // sched_priority = 2
    let _ = unsafe { syscall3(SYS_SCHED_SETSCHEDULER, tidnum as u64, SCHED_RR, param.as_ptr() as u64) };
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
