//! gpu_intel_tgl: Intel Tiger Lake graphics, ported from Linux 6.18.55 i915.
//!
//! The port follows docs/plan/GPU_INTEL_LINUX_MAP.md and the call order in
//! docs/plan/GPU_INTEL_TGL_TREE.txt. This first stage only takes the card
//! over from the kernel and reads it: bind (which makes the kernel's own
//! driver hand over), map the register BAR, read two registers that need
//! no forcewake. Nothing is written to the card.

#![no_std]

mod regs;

use npk_sys::Mmio;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// The default rights plus HARDWARE, which binding the PCI device requires.
// A section replaces the default, so READ, EXECUTE and RENDER are listed.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x01 | 0x04 | 0x08 | 0x40]; // READ|EXEC|RENDER|HARDWARE

const DRIVER_VERSION: &str = env!("CARGO_PKG_VERSION");

const INTEL_VENDOR: u16 = 0x8086;
/// Tiger Lake-LP GT2 (INTEL_TGL_GT2_IDS), the NUC.
const TGL_GT2: u16 = 0x9a78;
/// GTTMMADR: registers in the lower half, the GGTT in the upper.
const BAR0_PAGES: i32 = 16 * 1024 * 1024 / 4096;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut l = Line::new();
    l.s("PANIC, driver stopped");
    if let Some(loc) = info.location() {
        l.s(" at ").s(loc.file()).s(":").hex(loc.line());
    }
    l.say();
    core::arch::wasm32::unreachable()
}

/// One output line, built whole so the kernel log keeps it on one line.
struct Line {
    buf: [u8; 160],
    len: usize,
}

impl Line {
    fn new() -> Self {
        let mut l = Line { buf: [0; 160], len: 0 };
        l.s("[gpu_intel_tgl] ");
        l
    }

    fn s(&mut self, s: &str) -> &mut Self {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        self
    }

    fn hex(&mut self, v: u32) -> &mut Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut b = *b"0x00000000";
        for i in 0..8 {
            b[9 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
        }
        self.s(core::str::from_utf8(&b).unwrap_or("?"))
    }

    /// To the terminal `driver` was started from, and to the kernel log, so
    /// the result can be read with `dmesg` from any session.
    fn say(&self) {
        let text = &self.buf[..self.len];
        npk_sys::print(text);
        npk_sys::print(b"\n");
        npk_sys::log(text);
    }
}

fn say(s: &str) {
    Line::new().s(s).say();
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    Line::new().s("v").s(DRIVER_VERSION).s(", stage 0: take over and read").say();

    match npk_sys::pci_bind(INTEL_VENDOR as i32, TGL_GT2 as i32) {
        0 => {}
        rc => {
            say(match rc {
                -1 => "no 8086:9a78 on the bus",
                -2 => "bind denied (needs HARDWARE)",
                -3 => "card owned by another driver",
                _ => "bind failed",
            });
            return;
        }
    }
    say("bound 8086:9a78, kernel driver handed over");

    let rev = npk_sys::pci_read_config(0x08) as u32 & 0xff;
    Line::new().s("revision ").hex(rev).say();

    let Some(regs) = Mmio::map_bar(0, BAR0_PAGES) else {
        say("BAR0 could not be mapped");
        return;
    };
    say("BAR0 mapped (16 MB)");

    Line::new().s("FORCEWAKE_ACK_GT_GEN9     ").hex(regs.r32(regs::FORCEWAKE_ACK_GT_GEN9)).say();
    Line::new().s("FORCEWAKE_ACK_RENDER_GEN9 ").hex(regs.r32(regs::FORCEWAKE_ACK_RENDER_GEN9)).say();
    say("stage 0 done, desktop stays on the CPU path");
}
