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
    say("[gpu_intel_tgl] PANIC, driver stopped");
    if let Some(l) = info.location() {
        say(" at ");
        say(l.file());
        say(":");
        say_hex(l.line());
    }
    say("\n");
    core::arch::wasm32::unreachable()
}

fn say(s: &str) {
    npk_sys::print(s.as_bytes());
}

fn say_hex(v: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut b = *b"0x00000000";
    for i in 0..8 {
        b[9 - i] = HEX[((v >> (i * 4)) & 0xf) as usize];
    }
    say(core::str::from_utf8(&b).unwrap_or("?"));
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    say("[gpu_intel_tgl] v");
    say(DRIVER_VERSION);
    say(", stage 0: take over and read\n");

    match npk_sys::pci_bind(INTEL_VENDOR as i32, TGL_GT2 as i32) {
        0 => {}
        rc => {
            say(match rc {
                -1 => "[gpu_intel_tgl] no 8086:9a78 on the bus\n",
                -2 => "[gpu_intel_tgl] bind denied (needs HARDWARE)\n",
                -3 => "[gpu_intel_tgl] card owned by another driver\n",
                _ => "[gpu_intel_tgl] bind failed\n",
            });
            return;
        }
    }
    say("[gpu_intel_tgl] bound 8086:9a78, kernel driver handed over\n");

    let rev = npk_sys::pci_read_config(0x08) as u32 & 0xff;
    say("[gpu_intel_tgl] revision ");
    say_hex(rev);
    say("\n");

    let Some(regs) = Mmio::map_bar(0, BAR0_PAGES) else {
        say("[gpu_intel_tgl] BAR0 could not be mapped\n");
        return;
    };
    say("[gpu_intel_tgl] BAR0 mapped (16 MB)\n");

    say("[gpu_intel_tgl] FORCEWAKE_ACK_GT_GEN9     ");
    say_hex(regs.r32(regs::FORCEWAKE_ACK_GT_GEN9));
    say("\n[gpu_intel_tgl] FORCEWAKE_ACK_RENDER_GEN9 ");
    say_hex(regs.r32(regs::FORCEWAKE_ACK_RENDER_GEN9));
    say("\n[gpu_intel_tgl] stage 0 done, desktop stays on the CPU path\n");
}
