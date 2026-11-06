//! Last words: the panic and fatal-exception path.
//!
//! Nothing here takes a lock or allocates, because the fault may have
//! happened while holding the console, heap or capture lock. The first core
//! to get here stops the others with an NMI, then writes to the UART and
//! straight into the framebuffer; a nested fault on any core only halts.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

static PANICKING: AtomicBool = AtomicBool::new(false);

static FB_ADDR: AtomicU64 = AtomicU64::new(0);
static FB_PITCH: AtomicU32 = AtomicU32::new(0);
static FB_W: AtomicU32 = AtomicU32::new(0);
static FB_H: AtomicU32 = AtomicU32::new(0);

/// The visible framebuffer, recorded once it is mapped. 32 bpp only.
pub fn set_framebuffer(addr: u64, pitch: u32, width: u32, height: u32, bpp: u8) {
    if bpp != 32 { return; }
    FB_PITCH.store(pitch, Ordering::Relaxed);
    FB_W.store(width, Ordering::Relaxed);
    FB_H.store(height, Ordering::Relaxed);
    FB_ADDR.store(addr, Ordering::Release);
}

/// Has a core started dying? The NMI handler asks.
pub fn is_panicking() -> bool {
    PANICKING.load(Ordering::Acquire)
}

/// Stop this core for good.
pub fn halt() -> ! {
    crate::arch::halt_forever()
}

/// Start dying: interrupts off, other cores stopped. `None` if another
/// fault got here first; the caller then just halts.
pub fn begin() -> Option<Writer> {
    crate::arch::irq_disable();
    if PANICKING.swap(true, Ordering::AcqRel) {
        return None;
    }
    crate::arch::stop_other_cores();
    let mut w = Writer { col: 0, row: 0 };
    let _ = w.write_str("\n[npk] !!! ");
    Some(w)
}

/// Report `args` and halt: the whole path for a panic or a fatal exception.
pub fn die(args: fmt::Arguments) -> ! {
    if let Some(mut w) = begin() {
        let _ = w.write_fmt(args);
        let _ = writeln!(w, "\n[npk] core {}, all cores halted", crate::arch::current_core());
    }
    halt()
}

/// Writes to the UART and the top of the framebuffer at once.
pub struct Writer {
    col: u32,
    row: u32,
}

impl Writer {
    fn serial(b: u8) {
        crate::drivers::uart::panic_put(b);
    }

    fn screen(&mut self, b: u8) {
        let addr = FB_ADDR.load(Ordering::Acquire);
        if addr == 0 { return; }
        let (pitch, w, h) = (FB_PITCH.load(Ordering::Relaxed) as u64,
            FB_W.load(Ordering::Relaxed), FB_H.load(Ordering::Relaxed));
        let scale = (w / 1280).max(1);
        let (cw, ch) = (8 * scale, 16 * scale);
        let cols = (w / cw).max(1);
        if b == b'\n' {
            self.col = 0;
            self.row += 1;
            return;
        }
        if self.col >= cols {
            self.col = 0;
            self.row += 1;
        }
        let (x0, y0) = (self.col * cw, self.row * ch);
        if y0 + ch > h { return; }
        let glyph = if b < 128 { b } else { b'?' } as usize * 16;
        for gy in 0..ch {
            let bits = crate::drivers::font8x16::FONT[glyph + (gy / scale) as usize];
            for gx in 0..cw {
                let on = bits & (0x80 >> (gx / scale)) != 0;
                let px = if on { 0x00FF_FFFF } else { 0x0080_0000 };
                let off = (y0 + gy) as u64 * pitch + (x0 + gx) as u64 * 4;
                // SAFETY: inside the mapped framebuffer (x < w, y < h, 32 bpp);
                // the cores that could also draw are halted.
                unsafe { core::ptr::write_volatile((addr + off) as *mut u32, px) };
            }
        }
        self.col += 1;
    }
}

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' { Self::serial(b'\r'); }
            Self::serial(b);
            self.screen(b);
        }
        Ok(())
    }
}
