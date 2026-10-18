//! gpu_amd_renoir: AMD Renoir/Lucienne graphics, ported from Linux 6.18.55
//! amdgpu.
//!
//! The port follows docs/plan/GPU_AMD_LINUX_MAP.md. This is stage 1a
//! (`stage1a.rs`): `amdgpu_device_init` through `amdgpu_device_ip_early_init`
//! up to the `early_init` of the first block after COMMON, then the card is
//! given back. Nothing is written to the card but the MM_INDEX window VRAM
//! is read through. hostcheck/check.sh holds the port against Linux's own
//! code on the binaries read off the device.

#![no_std]
// Linux names are kept as they are (`amdgpu_device`, `mmMM_INDEX`), so a
// function can be found in both trees by name.
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
#![allow(dead_code)]

extern crate alloc;

mod amdgpu_atombios;
mod amdgpu_atomfirmware;
mod amdgpu_bios;
mod amdgpu_device;
mod amdgpu_discovery;
mod atom;
mod ip_blocks;
mod nbio_v7_0;
mod linux;
mod regs;
mod soc15;
mod stage1a;
mod tables;

use linux::{say, Line};

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

const AMD_VENDOR: u16 = 0x1002;
/// Lucienne (amdgpu_pciidlist: CHIP_RENOIR | AMD_IS_APU).
const LUCIENNE: u16 = 0x164c;
/// `adev->rmmio_base = pci_resource_start(adev->pdev, 5)` from CHIP_BONAIRE on.
const RMMIO_BAR: i32 = 5;

#[global_allocator]
static ALLOCATOR: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut l = Line::new();
    l.s("PANIC, driver stopped");
    if let Some(loc) = info.location() {
        l.s(" at ").s(loc.file()).s(":").dec(loc.line() as u64);
    }
    l.say();
    core::arch::wasm32::unreachable()
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    Line::new().s("v").s(DRIVER_VERSION).s(", stage 1a: amdgpu_device_init through the COMMON block and the VBIOS").say();

    match npk_sys::pci_bind(AMD_VENDOR as i32, LUCIENNE as i32) {
        0 => {}
        rc => {
            say(match rc {
                -1 => "no 1002:164c on the bus",
                -2 => "bind denied (needs HARDWARE)",
                -3 => "card owned by another driver",
                _ => "bind failed",
            });
            return;
        }
    }
    let revision = npk_sys::pci_read_config(0x08) as u32 as u8;
    let subsys = npk_sys::pci_read_config(0x2c) as u32;
    let mut l = Line::new();
    l.s("1002:164c rev ").hex(revision as u32).s(", subsystem ").hex(subsys).s(", BARs:");
    for bar in 0..6 {
        let size = npk_sys::pci_bar_size(bar);
        if size > 0 {
            l.s(" ").dec(bar as u64).s("=").dec((size as u64) >> 10).s("K");
        }
    }
    l.say();

    // amdgpu_device_init: the register BAR.
    let rmmio_size = npk_sys::pci_bar_size(RMMIO_BAR);
    if rmmio_size <= 0 {
        say("BAR5 (registers) has no size");
        return;
    }
    let pages = ((rmmio_size as u64).div_ceil(4096)) as i32;
    let Some(rmmio) = npk_sys::Mmio::map_bar(RMMIO_BAR, pages) else {
        say("BAR5 could not be mapped");
        return;
    };
    let mut adev = amdgpu_device::amdgpu_device::new(LUCIENNE, revision, rmmio, rmmio_size as u32);
    adev.rmmio_base = (npk_sys::pci_read_config(0x10 + 4 * RMMIO_BAR) as u32 & !0xf) as u64;
    let Some(bdf) = find_self() else {
        say("1002:164c not found by its bus address");
        return;
    };
    stage1a::run(&mut adev, bdf);
    say("giving the card back");
}

/// The bus, slot and function of this card: the only 1002:164c.
fn find_self() -> Option<(u8, u8, u8)> {
    for bus in 0..=255u32 {
        for slot in 0..32u32 {
            let bdf = (bus << 8 | slot << 3) as i32;
            let id = npk_sys::pci_read_config_at(bdf, 0) as u32;
            if id == (LUCIENNE as u32) << 16 | AMD_VENDOR as u32 {
                return Some((bus as u8, slot as u8, 0));
            }
        }
    }
    None
}

