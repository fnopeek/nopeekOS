//! The Linux kernel services i915 calls, on top of the nopeekOS driver ABI.
//!
//! The module is single-threaded: every i915 spinlock and mutex guards
//! nothing here and is left out. Timers (`hrtimer`) are deadlines checked
//! by the driver loop and by every wait, so they fire no earlier than in
//! Linux and at most one poll later.

/// Microseconds since boot.
pub fn now_us() -> u64 {
    let t = npk_sys::now_us();
    if t < 0 { 0 } else { t as u64 }
}

/// `udelay`: spins on the clock.
pub fn udelay(us: u64) {
    let t0 = now_us();
    while now_us() - t0 < us {}
}

/// `wait_for_atomic(COND, ms)` / `_wait_for_atomic(COND, us, 0)`: busy-polls
/// `cond` until it holds or `timeout_us` passes. `Ok` when it held. The
/// condition is checked once more after the deadline, as Linux does.
pub fn wait_for_atomic_us(timeout_us: u64, mut cond: impl FnMut() -> bool) -> Result<(), ()> {
    let t0 = now_us();
    loop {
        let expired = now_us() - t0 > timeout_us;
        if cond() {
            return Ok(());
        }
        if expired {
            return Err(());
        }
    }
}

/// `wait_for(COND, ms)`: polls with sleeps that grow from 10 us to 1 ms.
pub fn wait_for_ms(timeout_ms: u64, cond: impl FnMut() -> bool) -> Result<(), ()> {
    _wait_for(timeout_ms * 1000, 10, 1000, cond)
}

/// `_wait_for(COND, US, Wmin, Wmax)`: polls with sleeps that double from
/// `wmin` up to `wmax` microseconds.
pub fn _wait_for(timeout_us: u64, wmin: u64, wmax: u64, mut cond: impl FnMut() -> bool) -> Result<(), ()> {
    let t0 = now_us();
    let mut wait_us = wmin;
    loop {
        let expired = now_us() - t0 > timeout_us;
        if cond() {
            return Ok(());
        }
        if expired {
            return Err(());
        }
        if wait_us >= 1000 {
            npk_sys::sleep((wait_us / 1000) as i32);
        } else {
            udelay(wait_us);
        }
        if wait_us < wmax {
            wait_us <<= 1;
        }
    }
}

/// A `struct hrtimer` reduced to its deadline.
#[derive(Clone, Copy, Default)]
pub struct HrTimer {
    pub expires_us: Option<u64>,
}

impl HrTimer {
    /// `hrtimer_start_range_ns(timer, ns, ...)`, relative.
    pub fn start(&mut self, delay_us: u64) {
        self.expires_us = Some(now_us() + delay_us);
    }

    /// `hrtimer_cancel`: true when the timer was queued.
    pub fn cancel(&mut self) -> bool {
        self.expires_us.take().is_some()
    }

    /// `hrtimer_active`.
    pub fn active(&self) -> bool {
        self.expires_us.is_some()
    }

    /// Due now: the deadline has passed.
    pub fn due(&self, now: u64) -> bool {
        self.expires_us.is_some_and(|t| now >= t)
    }
}

/// One log line, built whole so the kernel log keeps it on one line.
pub struct Line {
    buf: [u8; 192],
    len: usize,
}

impl Line {
    pub fn new() -> Self {
        let mut l = Line { buf: [0; 192], len: 0 };
        l.s("[gpu_intel_tgl] ");
        l
    }

    pub fn s(&mut self, s: &str) -> &mut Self {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        self
    }

    pub fn hex(&mut self, v: u32) -> &mut Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut b = *b"0x00000000";
        for i in 0..8 {
            b[9 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
        }
        self.s(core::str::from_utf8(&b).unwrap_or("?"))
    }

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
        self.s(core::str::from_utf8(&b[i..]).unwrap_or("?"))
    }

    /// To the kernel log: `dmesg`, the terminal's log stream and `debug`
    /// all show it, once.
    pub fn say(&self) {
        npk_sys::log(&self.buf[..self.len]);
    }
}

pub fn say(s: &str) {
    Line::new().s(s).say();
}

/// `drm_err`, `drm_WARN` with a true condition, `drm_info`.
pub fn drm_err(s: &str) {
    Line::new().s("error: ").s(s).say();
}
