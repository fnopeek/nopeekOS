//! PID 1's side of the control channel: the virtio-console port named
//! "npk.control". Frames are `u16 len | u8 type | payload`, little-endian,
//! `len` counting the type byte and the payload. Runs as its own child, so a
//! fault here cannot take down PID 1.

use super::{say, syscall0, syscall1, syscall2, syscall3};
use super::{O_DIRECTORY, O_RDONLY, O_RDWR, SYS_CLOSE, SYS_FORK, SYS_GETDENTS64, SYS_NANOSLEEP,
            SYS_OPEN, SYS_READ, SYS_WRITE, SYS_EXIT};

const SYS_KILL: u64 = 62;
const SIGTERM: u64 = 15;

/// Host -> guest: end the app; the session then syncs and powers off.
const MSG_QUIT: u8 = 0x01;
/// Guest -> host: this process is listening.
const MSG_READY: u8 = 0x81;
const MAX_FRAME: usize = 4096;

const PORT_NAME: &[u8] = b"npk.control";
const PORTS_DIR: &[u8] = b"/sys/class/virtio-ports\0";
/// The process that gets SIGTERM on quit (its `comm`).
const APP_COMM: &[u8] = b"librewolf";

/// Fork the control process; the parent returns at once.
pub unsafe fn spawn(kmsg_fd: i64) {
    let pid = unsafe { syscall0(SYS_FORK) };
    if pid == 0 {
        unsafe { run(kmsg_fd) }
    }
}

unsafe fn run(kmsg_fd: i64) -> ! {
    // The port appears once the driver and the host have agreed on it;
    // wait up to 10 s, then give up quietly (an older host has none).
    let mut path = [0u8; 48];
    let mut found = false;
    for _ in 0..500 {
        if unsafe { find_port(&mut path) } {
            found = true;
            break;
        }
        sleep_ms(20);
    }
    if !found {
        say(kmsg_fd, b"[ctl] no control port\n");
        unsafe { exit() }
    }
    let fd = unsafe { syscall2(SYS_OPEN, path.as_ptr() as u64, O_RDWR) };
    if fd < 0 {
        say(kmsg_fd, b"[ctl] control port did not open\n");
        unsafe { exit() }
    }
    unsafe { write_all(fd, &[1, 0, MSG_READY]) };

    let mut buf = [0u8; MAX_FRAME + 2];
    let mut have = 0usize;
    loop {
        let n = unsafe {
            syscall3(SYS_READ, fd as u64, buf[have..].as_mut_ptr() as u64, (buf.len() - have) as u64)
        };
        if n <= 0 {
            // EOF while the host side is not connected; read blocks once it is.
            sleep_ms(100);
            continue;
        }
        have += n as usize;
        while have >= 2 {
            let len = u16::from_le_bytes([buf[0], buf[1]]) as usize;
            if len == 0 || len > MAX_FRAME {
                have = 0;
                break;
            }
            if have < 2 + len {
                break;
            }
            if buf[2] == MSG_QUIT {
                unsafe { quit_app(kmsg_fd) };
            }
            // By hand: `copy_within` would call a libc `memmove`.
            let mut i = 0;
            while i < have - (2 + len) {
                buf[i] = buf[2 + len + i];
                i += 1;
            }
            have -= 2 + len;
        }
    }
}

/// Send SIGTERM to every process whose `comm` is the app's.
unsafe fn quit_app(kmsg_fd: i64) {
    let mut sent = 0u32;
    let dir = unsafe { syscall2(SYS_OPEN, b"/proc\0".as_ptr() as u64, O_RDONLY | O_DIRECTORY) };
    if dir < 0 {
        return;
    }
    let mut ents = [0u8; 4096];
    loop {
        let n = unsafe { syscall3(SYS_GETDENTS64, dir as u64, ents.as_mut_ptr() as u64, ents.len() as u64) };
        if n <= 0 {
            break;
        }
        let mut off = 0usize;
        while off + 19 <= n as usize {
            let reclen = u16::from_ne_bytes([ents[off + 16], ents[off + 17]]) as usize;
            if reclen < 19 || off + reclen > n as usize {
                break;
            }
            let name = &ents[off + 19..off + reclen];
            if let Some(pid) = parse_pid(name) {
                let mut p = [0u8; 32];
                let len = join(&mut p, &[b"/proc/", digits(name), b"/comm\0"]);
                let mut comm = [0u8; 32];
                if len > 0 && unsafe { read_file(&p, &mut comm) }.is_some_and(|c| same(trim(&comm[..c]), APP_COMM)) {
                    let _ = unsafe { syscall2(SYS_KILL, pid, SIGTERM) };
                    sent += 1;
                }
            }
            off += reclen;
        }
    }
    let _ = unsafe { syscall1(SYS_CLOSE, dir as u64) };
    if sent > 0 {
        say(kmsg_fd, b"[ctl] quit: SIGTERM to the app\n");
    } else {
        say(kmsg_fd, b"[ctl] quit: no app running\n");
    }
}

/// Find the port named `PORT_NAME` under /sys/class/virtio-ports and put
/// its device path ("/dev/vportNpM", NUL-terminated) into `path`.
unsafe fn find_port(path: &mut [u8; 48]) -> bool {
    let dir = unsafe { syscall2(SYS_OPEN, PORTS_DIR.as_ptr() as u64, O_RDONLY | O_DIRECTORY) };
    if dir < 0 {
        return false;
    }
    let mut ents = [0u8; 2048];
    let mut found = false;
    'outer: loop {
        let n = unsafe { syscall3(SYS_GETDENTS64, dir as u64, ents.as_mut_ptr() as u64, ents.len() as u64) };
        if n <= 0 {
            break;
        }
        let mut off = 0usize;
        while off + 19 <= n as usize {
            let reclen = u16::from_ne_bytes([ents[off + 16], ents[off + 17]]) as usize;
            if reclen < 19 || off + reclen > n as usize {
                break;
            }
            let entry = cstr(&ents[off + 19..off + reclen]);
            if entry.len() >= 5 && same(&entry[..5], b"vport") {
                let mut p = [0u8; 80];
                let mut name = [0u8; 64];
                if join(&mut p, &[b"/sys/class/virtio-ports/", entry, b"/name\0"]) > 0
                    && unsafe { read_file(&p, &mut name) }.is_some_and(|c| same(trim(&name[..c]), PORT_NAME))
                    && join(path, &[b"/dev/", entry, b"\0"]) > 0
                {
                    found = true;
                    break 'outer;
                }
            }
            off += reclen;
        }
    }
    let _ = unsafe { syscall1(SYS_CLOSE, dir as u64) };
    found
}

/// Read up to `buf.len()` bytes of the NUL-terminated path `path`.
unsafe fn read_file(path: &[u8], buf: &mut [u8]) -> Option<usize> {
    let fd = unsafe { syscall2(SYS_OPEN, path.as_ptr() as u64, O_RDONLY) };
    if fd < 0 {
        return None;
    }
    let n = unsafe { syscall3(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) };
    let _ = unsafe { syscall1(SYS_CLOSE, fd as u64) };
    if n < 0 { None } else { Some(n as usize) }
}

unsafe fn write_all(fd: i64, mut data: &[u8]) {
    while !data.is_empty() {
        let n = unsafe { syscall3(SYS_WRITE, fd as u64, data.as_ptr() as u64, data.len() as u64) };
        if n <= 0 {
            return;
        }
        data = &data[n as usize..];
    }
}

unsafe fn exit() -> ! {
    loop {
        let _ = unsafe { syscall1(SYS_EXIT, 0) };
    }
}

fn sleep_ms(ms: i64) {
    let ts: [i64; 2] = [ms / 1000, (ms % 1000) * 1_000_000];
    let _ = unsafe { syscall2(SYS_NANOSLEEP, ts.as_ptr() as u64, 0) };
}

/// The bytes before the first NUL.
fn cstr(b: &[u8]) -> &[u8] {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    &b[..end]
}

/// Byte-wise equality; slice `==` would call into a libc `bcmp` that PID 1
/// does not link.
fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Without a trailing newline.
fn trim(b: &[u8]) -> &[u8] {
    if b.last() == Some(&b'\n') { &b[..b.len() - 1] } else { b }
}

fn digits(name: &[u8]) -> &[u8] {
    cstr(name)
}

fn parse_pid(name: &[u8]) -> Option<u64> {
    let d = cstr(name);
    if d.is_empty() || !d.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(d.iter().fold(0u64, |a, &c| a * 10 + (c - b'0') as u64))
}

/// Concatenate `parts` into `out`; 0 if it does not fit.
fn join(out: &mut [u8], parts: &[&[u8]]) -> usize {
    let mut n = 0usize;
    for part in parts {
        if n + part.len() > out.len() {
            return 0;
        }
        // By hand: `copy_from_slice` would call a libc `memcpy`.
        let mut i = 0;
        while i < part.len() {
            out[n + i] = part[i];
            i += 1;
        }
        n += part.len();
    }
    n
}
