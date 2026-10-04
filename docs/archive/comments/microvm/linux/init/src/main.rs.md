# `microvm/linux/init/src/main.rs` @ 5e0102684

## L1-10 · `#![no_std]`

```
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
```

## L26 · `#[allow(dead_code)] // part of the syscall table; a map with holes is worse`

```
// part of the syscall table; a map with holes is worse
```

## L38 · `const O_DIRECTORY: u64 = 0o200000;`

```
// O_DIRECTORY for opendir-style reads, SCHED_RR for the RT promoter.
```

## L42 · `const MS_RDONLY: u64 = 1;`

```
// mount(2) flags
```

## L45 · `const F_OK: u64 = 0;`

```
// access(2) modes
```

## L61-66 · `core::arch::global_asm!(`

```
// _start in pure asm: Linux's process-load ABI gives RSP 16-aligned
// pointing at argc (no return slot). Rust's function prologue assumes
// the System V function-entry convention (RSP 8-misaligned after a
// CALL). Without the CALL bridge, any later MOVAPS-on-stack #GPs.
// Verified on NUC v0.137.0/.1 — the trap fired in a MOVAPS in the
// (now-removed) echo_round_trip prologue.
```

## L83-88 · `if try_switch_to_sqfs(kmsg_fd) {`

```
// If the read-only userspace bundle is present on /dev/vdb
// (second virtio-blk, slot 5), switch into it. The big bundle
// (Mesa/cage/LibreWolf) lives compressed on squashfs, decompressed
// on read — RAM-efficient vs. an unpacked cpio initramfs. On
// absence/failure we stay in the minimal initramfs (the device
// comes up empty until the OTA bundle lands).
```

## L90-92 · `mount_essentials();`

```
// Re-establish /proc /sys /dev /tmp inside the new root. The
// mountpoints exist in the Alpine image; squashfs is RO so the
// mkdirs no-op harmlessly.
```

## L97-100 · `if bench_requested() {`

```
// Diagnostic: if the host passed `nopeekbench=` on the cmdline, run a pure
// busybox download through the nat bridge (no cage/GPU/browser) so the
// BRIDGE can be measured in isolation, then halt. Bisects bridge vs
// browser-userspace for the loaded-latency hunt.
```

## L105-112 · `launch_wayland(kmsg_fd);`

```
// Hand the framebuffer to the Wayland stack: cage (wlroots kiosk
// compositor) running LibreWolf, rendered through the pixman
// software renderer + wlroots DRM backend → /dev/dri/card0 →
// virtio-gpu → our Shade Surface tile. On success PID-1 becomes
// the supervising shell and never returns. It returns only if the
// bundle has no cage (degraded/minimal initramfs) — then just
// park, so the window still persists (Linux panics if PID-1
// exits).
```

## L121-136 · `fn launch_wayland(kmsg_fd: i64) {`

```
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
```

## L148-175 · `let arg2 = b"exec >/dev/kmsg 2>&1; \`

```
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
```

## L274-276 · `unsafe { spawn_rt_watcher(kmsg_fd); }`

```
// Fork the RT-promoter before becoming the shell. The child loops on
// /proc promoting cubeb/AudioIPC threads to SCHED_RR from outside the
// sandbox; the parent execs cage+librewolf as usual.
```

## L279-281 · `if gdiag_requested() {`

```
// Fork the guest-side diagnostic probe (only if the host asked via
// `nopeekgdiag` on the cmdline). It dumps the guest's INTERNAL view every
// second — the inside angle we never had while chasing the download latency.
```

## L297-298 · `fn bench_requested() -> bool {`

```
/// True if the kernel cmdline contains `nopeekbench` — the host asked for a
/// pure-bridge throughput run instead of the browser.
```

## L301 · `syscall3(SYS_OPEN, b"/proc/cmdline\0".as_ptr() as u64, 0 /*O_RDONLY*/, 0)`

```
/*O_RDONLY*/
```

## L315-318 · `fn launch_bench(kmsg_fd: i64) {`

```
/// Pure-bridge throughput run: bring up eth0, wget a big file three times from
/// the local server (10.0.2.2 via slirp) through our nat bridge — no cage, no
/// GPU, no browser. The SERVER reports the authoritative rate; guest /proc/
/// uptime gives a cross-check. Then halt. `MB` read from the cmdline.
```

## L324-327 · `let arg2 = b"exec >/dev/kmsg 2>&1; \`

```
// Measure what ARRIVED, not what was asked for. The old script divided a
// hardcoded 150 MB by the elapsed time and threw wget's exit status away, so
// a connection that failed in 50 ms printed "24000 Mbit" -- a measuring tool
// that reports success for a failure is worse than none.
```

## L382-384 · `if gdiag_requested() {`

```
// Fork the guest-side probe so [gdiag] runs alongside the bench — the clean
// measurement (raw wget/nc, no browser compute). The decisive question for the
// lottery: during a SLOW GET is the guest CPU-bound, or IDLE and waiting on us?
```

## L399-407 · `fn try_switch_to_sqfs(kmsg_fd: i64) -> bool {`

```
/// Mount the read-only squashfs userspace bundle from `/dev/vdb` and
/// chroot into it. Returns `true` if we are now running inside the
/// bundle root, `false` if the device is absent or not a valid
/// squashfs (→ caller stays in the minimal initramfs).
///
/// chroot (not pivot_root/MS_MOVE): with squashfs the initramfs holds
/// only our ~1 KB PID-1, so there is no initramfs RAM worth reclaiming
/// — chroot is the lower-risk switch. Open fds (kmsg/console) survive
/// it, so logging keeps working across the boundary.
```

## L409-410 · `let probe = unsafe { syscall3(SYS_ACCESS, b"/dev/vdb\0".as_ptr() as u64, F_OK, 0) };`

```
// /dev/vdb only exists once devtmpfs is mounted (done by the
// initramfs-side mount_essentials before we get here).
```

## L443-447 · `fn mount_essentials() {`

```
/// Mount /proc, /sys, /dev (devtmpfs), /tmp + /dev/shm (tmpfs).
/// Required for any real Linux userspace to function (and Firefox
/// hard-requires /dev/shm). With a cpio initramfs Linux skips
/// `prepare_namespace()` and never honors `devtmpfs.mount=1`, so the
/// init has to do it itself.
```

## L482-486 · `let _ = syscall2(SYS_MKDIR, b"/dev/shm\0".as_ptr() as u64, 0o1777);`

```
// /dev/shm — Firefox/Gecko hard-requires POSIX shared memory
// for content-process IPC. Without it shm_open() fails and a
// content process null-derefs in libxul (observed: "Privileged
// Cont segfault at 0 in libxul.so"). /dev is devtmpfs; mount a
// tmpfs on the /dev/shm subdir, mode 1777 like a real system.
```

## L499-504 · `fn open_console_kmsg() -> (i64, i64) {`

```
/// Open /dev/console (RDWR, dup'd to 0/1/2) and /dev/kmsg (WO).
/// Returns (kmsg_fd, console_fd). Either may be -1 on failure.
///
/// `/dev/kmsg` always reaches the host capture (printk subsystem,
/// polled). `/dev/console` goes through the tty layer which may stall
/// on missing IRQ4 under our `nolapic noapic` cmdline.
```

## L518 · `syscall2(SYS_OPEN, b"/dev/kmsg\0".as_ptr() as u64, 1 /* O_WRONLY */)`

```
/* O_WRONLY */
```

## L523-524 · `fn say(kmsg_fd: i64, msg: &[u8]) {`

```
/// Write a message to both /dev/kmsg (printk-direct, polled, always
/// reaches the host capture) and stdout. Either reaching is enough.
```

## L550-559 · `static mut RT_SEEN: [i32; 96] = [0; 96];`

```
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
```

## L566 · `let base = (&raw mut RT_SEEN) as *mut i32;`

```
// Raw-pointer access: edition 2024 denies references to `static mut`.
```

## L587-588 · `while i + needle.len() <= hay.len() {`

```
// Manual byte compare — slice `==` would emit a bcmp/memcmp call that
// this freestanding (no libc) binary can't link.
```

## L598 · `unsafe fn spawn_rt_watcher(kmsg_fd: i64) {`

```
/// Fork the RT watcher. Parent returns immediately; the child never returns.
```

## L606-608 · `fn gdiag_requested() -> bool {`

```
/// True if the kernel cmdline contains `nopeekgdiag` — the host asked for the
/// guest-side diagnostic probe (per-vCPU busy/softirq %, download-socket TCP
/// state, softnet drops/squeeze every second).
```

## L625-635 · `unsafe fn spawn_gdiag(kmsg_fd: i64) {`

```
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
```

## L639 · `return; // parent continues to cage`

```
// parent continues to cage
```

## L666-667 · `say(kmsg_fd, b"[gdiag] /bin/sh execve failed -- probe off\n");`

```
// execve failed — park forever so the child NEVER falls back into the
// parent's cage launch (a double cage exec).
```

## L677-680 · `let ts: [i64; 2] = [0, 50_000_000];`

```
// ~50 ms between sweeps: catch a freshly-spawned cubeb/AudioIPC
// thread and give it SCHED_RR before it underruns its first periods
// (the 500 ms sweep raced — audio worked only when promotion beat the
// first buffering gap). Scan is cheap + we have idle headroom now.
```

## L695 · `while off + 19 <= n {`

```
// linux_dirent64: d_ino(8) d_off(8) d_reclen@16(2) d_type@18(1) d_name@19
```

## L710 · `let mut path = [0u8; 64];`

```
// "/proc/<pid>/task\0"
```

## L738 · `let mut path = [0u8; 80];`

```
// "/proc/<pid>/task/<tid>/comm\0" + parse tid as int.
```

## L762 · `if unsafe { rt_already(tidnum) } { return; }`

```
// Promote each matching thread once (idempotent; skip if already done).
```

## L764 · `let param: [i32; 1] = [2]; // sched_priority = 2`

```
// sched_priority = 2
```

## L768 · `unsafe fn syscall0(nr: u64) -> i64 {`

```
// ── Raw syscall wrappers ───────────────────────────────────────────
```

