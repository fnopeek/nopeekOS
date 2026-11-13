//! printk and the errno values the ported code returns
//! (`include/uapi/asm-generic/errno*.h`).

use core::sync::atomic::{AtomicPtr, Ordering};

pub const ENXIO: i32 = 6;
pub const EINVAL: i32 = 22;
pub const EAGAIN: i32 = 11;
pub const EOPNOTSUPP: i32 = 95;

/// Where `printk` goes: the module's log, with its own prefix. Null until
/// the module sets it; lines before that are dropped.
static PRINTK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// The module's printk: one whole line, without a newline.
pub fn set_printk(f: fn(&[u8])) {
    PRINTK.store(f as *mut (), Ordering::Relaxed);
}

fn printk(line: &[u8]) {
    let p = PRINTK.load(Ordering::Relaxed);
    if p.is_null() {
        return;
    }
    // SAFETY: PRINTK only ever holds null or a `fn(&[u8])` stored by
    // `set_printk`; a function pointer and a data pointer have the same
    // size on every target the crate builds for.
    let f: fn(&[u8]) = unsafe { core::mem::transmute::<*mut (), fn(&[u8])>(p) };
    f(line);
}

/// One log line, built whole so the log keeps it on one line.
pub struct Line {
    buf: [u8; 192],
    len: usize,
}

impl Line {
    pub fn new() -> Self {
        Line { buf: [0; 192], len: 0 }
    }

    pub fn s(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    /// Raw bytes, as `%s` prints them.
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        let n = b.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
        self
    }

    /// `0x%08x`.
    pub fn hex(&mut self, v: u32) -> &mut Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut b = *b"0x00000000";
        for i in 0..8 {
            b[9 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
        }
        self.bytes(&b)
    }

    /// `%llu`.
    pub fn dec(&mut self, mut v: u64) -> &mut Self {
        let mut b = [0u8; 20];
        let mut i = b.len();
        loop {
            i -= 1;
            b[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.bytes(&b[i..])
    }

    /// `pr_notice` / `drm_notice`: to the module's log.
    pub fn say(&self) {
        printk(&self.buf[..self.len]);
    }
}

impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}
