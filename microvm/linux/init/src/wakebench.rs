//! `npkbench_wake` on the cmdline: two processes bounce one byte through a
//! pair of pipes, pinned to the same or to different CPUs. Each blocking read
//! puts its side to sleep, so one round trip costs two cross-CPU wakeups — the
//! path every IPC handoff in the guest takes. Runs before the session, on an
//! idle system; `tools/wakebench.c` is the same loop for a native host.

use super::{syscall0, syscall1, syscall2, syscall3, syscall4, say};

const SYS_PIPE: u64 = 22;
const SYS_WAIT4: u64 = 61;
const SYS_SCHED_SETAFFINITY: u64 = 203;
const SYS_CLOCK_GETTIME: u64 = 228;
const CLOCK_MONOTONIC: u64 = 1;

const ROUNDS: u64 = 20_000;
const WARMUP: u64 = 200;
const SLOW_NS: u64 = 100_000;

pub fn requested() -> bool {
    let mut buf = [0u8; 1024];
    // SAFETY: `read_file` writes at most `buf.len()` bytes into `buf`.
    let n = unsafe { super::control::read_file(b"/proc/cmdline\0", &mut buf) }.unwrap_or(0);
    let needle = b"npkbench_wake";
    buf[..n].windows(needle.len()).any(|w| w == needle)
}

fn now_ns() -> u64 {
    let mut ts = [0i64; 2];
    // SAFETY: `ts` is a valid `struct timespec` for the kernel to fill.
    unsafe { syscall2(SYS_CLOCK_GETTIME, CLOCK_MONOTONIC, ts.as_mut_ptr() as u64) };
    ts[0] as u64 * 1_000_000_000 + ts[1] as u64
}

fn pin(cpu: u32) -> bool {
    let mask: u64 = 1 << cpu;
    // SAFETY: an 8-byte CPU mask for the calling process.
    unsafe { syscall3(SYS_SCHED_SETAFFINITY, 0, 8, &mask as *const u64 as u64) == 0 }
}

/// The highest online CPU, from /sys/devices/system/cpu/online ("0-5").
fn last_online() -> u32 {
    let mut buf = [0u8; 64];
    // SAFETY: as in `requested`.
    let n = unsafe { super::control::read_file(b"/sys/devices/system/cpu/online\0", &mut buf) }
        .unwrap_or(0);
    let (mut last, mut cur) = (0u32, 0u32);
    for &b in &buf[..n] {
        match b {
            b'0'..=b'9' => cur = cur * 10 + (b - b'0') as u32,
            b'-' | b',' => cur = 0,
            _ => break,
        }
        last = cur;
    }
    last
}

/// Byte by byte: the init links without libc, so there is no `memcpy`.
fn put(out: &mut [u8], at: &mut usize, s: &[u8]) {
    for &c in s {
        if *at == out.len() { return; }
        out[*at] = c;
        *at += 1;
    }
}

fn put_u64(out: &mut [u8], at: &mut usize, mut v: u64) {
    let mut d = [0u8; 20];
    let mut i = d.len();
    loop {
        i -= 1;
        d[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 { break; }
    }
    put(out, at, &d[i..]);
}

/// One pair: (average ns, max ns, rounds over `SLOW_NS`), or None on a
/// failed pipe, fork or pin.
fn pingpong(a: u32, b: u32) -> Option<(u64, u64, u64)> {
    let mut ping = [0i32; 2];
    let mut pong = [0i32; 2];
    // SAFETY: each array is the `int[2]` the kernel fills.
    if unsafe { syscall1(SYS_PIPE, ping.as_mut_ptr() as u64) } < 0
        || unsafe { syscall1(SYS_PIPE, pong.as_mut_ptr() as u64) } < 0
    {
        return None;
    }
    let mut byte = [0u8; 1];
    // SAFETY: fork has no preconditions; the child only uses its own copies.
    let pid = unsafe { syscall0(super::SYS_FORK) };
    if pid == 0 {
        pin(b);
        for _ in 0..ROUNDS + WARMUP {
            // SAFETY: fds from `pipe`, one-byte buffer.
            unsafe {
                if syscall3(super::SYS_READ, ping[0] as u64, byte.as_mut_ptr() as u64, 1) != 1 { break; }
                if syscall3(super::SYS_WRITE, pong[1] as u64, byte.as_ptr() as u64, 1) != 1 { break; }
            }
        }
        // SAFETY: the child ends here.
        unsafe { syscall1(super::SYS_EXIT, 0) };
    }
    if pid < 0 {
        return None;
    }
    let ok = pin(a);
    let (mut sum, mut max, mut slow) = (0u64, 0u64, 0u64);
    for i in 0..ROUNDS + WARMUP {
        let t0 = now_ns();
        // SAFETY: as in the child.
        unsafe {
            if syscall3(super::SYS_WRITE, ping[1] as u64, byte.as_ptr() as u64, 1) != 1 { break; }
            if syscall3(super::SYS_READ, pong[0] as u64, byte.as_mut_ptr() as u64, 1) != 1 { break; }
        }
        let dt = now_ns().saturating_sub(t0);
        if i >= WARMUP {
            sum += dt;
            max = max.max(dt);
            if dt > SLOW_NS { slow += 1; }
        }
    }
    let mut status = 0i32;
    // SAFETY: reap our own child; `status` is a valid int.
    unsafe { syscall4(SYS_WAIT4, pid as u64, &mut status as *mut i32 as u64, 0, 0) };
    for fd in [ping[0], ping[1], pong[0], pong[1]] {
        // SAFETY: closing fds this process opened.
        unsafe { syscall1(super::SYS_CLOSE, fd as u64) };
    }
    ok.then_some((sum / ROUNDS, max, slow))
}

/// Run in a child and wait for it, so PID 1 keeps its own CPU mask.
pub fn run(kmsg_fd: i64) {
    // SAFETY: as in `pingpong`.
    let pid = unsafe { syscall0(super::SYS_FORK) };
    if pid != 0 {
        if pid > 0 {
            let mut status = 0i32;
            // SAFETY: as in `pingpong`.
            unsafe { syscall4(SYS_WAIT4, pid as u64, &mut status as *mut i32 as u64, 0, 0) };
        }
        return;
    }
    // The other vCPUs come online beside PID 1; give them up to 5 s.
    let mut last = 0;
    for _ in 0..100 {
        last = last_online();
        if last > 0 { break; }
        let ts = [0i64, 50_000_000];
        // SAFETY: a valid `struct timespec`, no remainder wanted.
        unsafe { syscall2(super::SYS_NANOSLEEP, ts.as_ptr() as u64, 0) };
    }
    let mut prev_last = u32::MAX;
    while last != prev_last {
        prev_last = last;
        let ts = [0i64, 300_000_000];
        // SAFETY: as above.
        unsafe { syscall2(super::SYS_NANOSLEEP, ts.as_ptr() as u64, 0) };
        last = last_online();
    }
    for b in 0..=last {
        let mut line = [0u8; 160];
        let mut at = 0;
        put(&mut line, &mut at, b"[wakebench] cpu0<->cpu");
        put_u64(&mut line, &mut at, b as u64);
        match pingpong(0, b) {
            Some((avg, max, slow)) => {
                put(&mut line, &mut at, b": avg ");
                put_u64(&mut line, &mut at, avg / 1000);
                put(&mut line, &mut at, b".");
                put_u64(&mut line, &mut at, avg % 1000 / 100);
                put(&mut line, &mut at, b" us round trip, max ");
                put_u64(&mut line, &mut at, max / 1000);
                put(&mut line, &mut at, b" us, over 100 us: ");
                put_u64(&mut line, &mut at, slow);
                put(&mut line, &mut at, b"/");
                put_u64(&mut line, &mut at, ROUNDS);
            }
            None => put(&mut line, &mut at, b": failed"),
        }
        put(&mut line, &mut at, b"\n");
        say(kmsg_fd, &line[..at]);
    }
    // SAFETY: the benchmark child ends here.
    unsafe { syscall1(super::SYS_EXIT, 0) };
}
