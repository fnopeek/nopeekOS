//! Last words: the panic and fatal-exception path.
//!
//! Nothing here takes a lock or allocates, because the fault may have
//! happened while holding the console, heap or capture lock. The first core
//! to get here stops the others with an NMI, then writes to COM1 and
//! straight into the framebuffer; a nested fault on any core only halts.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::hw::Port;

static PANICKING: AtomicBool = AtomicBool::new(false);

static FB_ADDR: AtomicU64 = AtomicU64::new(0);
static FB_PITCH: AtomicU32 = AtomicU32::new(0);
static FB_W: AtomicU32 = AtomicU32::new(0);
static FB_H: AtomicU32 = AtomicU32::new(0);

// SAFETY: COM1, the kernel's own serial console.
const COM1_DATA: Port = unsafe { Port::new(0x3F8) };
// SAFETY: COM1's line status register; reading it has no side effect.
const COM1_LSR: Port = unsafe { Port::new(0x3FD) };

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
    loop {
        // SAFETY: interrupts off, then wait; nothing resumes this core.
        unsafe { core::arch::asm!("cli; hlt", options(nomem, nostack)) };
    }
}

/// Start dying: interrupts off, other cores stopped. `None` if another
/// fault got here first; the caller then just halts.
pub fn begin() -> Option<Writer> {
    // SAFETY: masking interrupts on the way down.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
    if PANICKING.swap(true, Ordering::AcqRel) {
        return None;
    }
    stop_other_cores();
    let mut w = Writer { col: 0, row: 0 };
    let _ = w.write_str("\n[npk] !!! ");
    Some(w)
}

/// Report `args` and halt: the whole path for a panic or a fatal exception.
pub fn die(args: fmt::Arguments) -> ! {
    if let Some(mut w) = begin() {
        let _ = w.write_fmt(args);
        let _ = writeln!(w, "\n[npk] core {}, all cores halted", crate::smp::per_core::current_core_id());
    }
    halt()
}

/// NMI to every other core; their NMI handler sees `PANICKING` and halts.
fn stop_other_cores() {
    use crate::interrupts::{LAPIC_ICR_HI, LAPIC_ICR_LO};
    let lapic = crate::interrupts::lapic();
    // Bounded: a wedged LAPIC must not keep the message from being written.
    for _ in 0..100_000 {
        if lapic.r32(LAPIC_ICR_LO) & (1 << 12) == 0 { break; }
        core::hint::spin_loop();
    }
    lapic.w32(LAPIC_ICR_HI, 0);
    // Shorthand all-excluding-self, delivery mode NMI, assert.
    lapic.w32(LAPIC_ICR_LO, (0b11 << 18) | (1 << 14) | (0b100 << 8));
}

/// Writes to COM1 and the top of the framebuffer at once.
pub struct Writer {
    col: u32,
    row: u32,
}

impl Writer {
    fn serial(b: u8) {
        for _ in 0..100_000 {
            if COM1_LSR.inb() & 0x20 != 0 { break; }
            core::hint::spin_loop();
        }
        COM1_DATA.outb(b);
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
            let bits = crate::framebuffer::FONT[glyph + (gy / scale) as usize];
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
