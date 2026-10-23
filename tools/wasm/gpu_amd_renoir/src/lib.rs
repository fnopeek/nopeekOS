//! gpu_amd_renoir: AMD Renoir/Lucienne graphics, ported from Linux 6.18.55
//! amdgpu.
//!
//! The port follows docs/plan/GPU_AMD_LINUX_MAP.md. This is stage 2
//! (`stage1.rs`): `amdgpu_device_init` with the reset on init, then
//! `amdgpu_device_ip_init` through the COMMON block, then the card is given
//! back. hostcheck/check.sh holds the port against Linux's own code on the
//! binaries read off the device.

#![no_std]
// Linux names are kept as they are (`amdgpu_device`, `mmMM_INDEX`), so a
// function can be found in both trees by name.
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
#![allow(dead_code)]

extern crate alloc;

mod native_gen;
mod amdgpu_kms;
mod amdgpu_virtio;
mod amdgpu_atombios;
mod athub_v1_0;
mod amdgpu_atomfirmware;
mod amdgpu_bios;
mod amdgpu_device;
mod hdp_v4_0;
mod amdgpu_gart;
mod amdgpu_ttm;
mod amdgpu_object;
mod amdgpu_ring;
mod amdgpu_vram_mgr;
mod drm_mm;
mod dmub_srv;
mod ttm;
mod gpu_buddy;
mod mmhub_v1_0;
mod gfxhub_v1_0;
mod amdgpu_vm;
mod amdgpu_irq;
mod amdgpu_gmc;
mod amdgpu_discovery;
mod amdgpu_dm;
mod dc_helper;
mod dc_types;
mod dc;
mod dc_resource;
mod dccg;
mod dce_clock_source;
mod dcn10_ipp;
mod dcn21_hubp;
mod dcn21_resource;
mod dml_structs;
mod dcn20_fpu;
mod amdgpu_fence;
mod amdgpu_ib;
mod dma_fence;
mod dma_resv;
mod workqueue;
mod drm_sched;
mod amdgpu_sync;
mod amdgpu_job;
mod amdgpu_ids;
mod amdgpu_vm_pt;
mod amdgpu_vm_sdma;
mod amdgpu_res_cursor;
mod amdgpu_ctx;
mod amdgpu_cs;
mod shaders_gen;
mod radv_stream;
mod compose;
mod amdgpu_psp;
mod amdgpu_ras;
mod amdgpu_smu;
mod amdgpu_ucode;
mod atom;
mod atom_exec;
mod gfx_v9_0;
mod gmc_v9_0;
mod gpudev;
mod import;
mod ip_blocks;
mod jpeg_v2_0;
mod nbio_v7_0;
mod linux;
mod regs;
mod sdma_v4_0;
mod soc15;
mod stage1;
mod tables;
mod vcn_v2_0;
mod vega10_ih;

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
    Line::new().s("v").s(DRIVER_VERSION).s(", stage 5: the GPU service, copies on SDMA").say();

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
    adev.subsystem_vendor = subsys as u16;
    adev.subsystem_device = (subsys >> 16) as u16;
    // BAR2, the doorbells: a 64-bit memory BAR (BAR2/BAR3).
    let bar2_lo = npk_sys::pci_read_config(0x18) as u32;
    let bar2_hi = if bar2_lo & 0x6 == 0x4 { npk_sys::pci_read_config(0x1c) as u32 } else { 0 };
    let bar2 = (((bar2_hi as u64) << 32) | (bar2_lo & !0xf) as u64, npk_sys::pci_bar_size(2).max(0) as u64);
    // BAR0, the VRAM aperture: a 64-bit memory BAR (BAR0/BAR1).
    let bar0_lo = npk_sys::pci_read_config(0x10) as u32;
    let bar0_hi = if bar0_lo & 0x6 == 0x4 { npk_sys::pci_read_config(0x14) as u32 } else { 0 };
    adev.bar0 = (((bar0_hi as u64) << 32) | (bar0_lo & !0xf) as u64, npk_sys::pci_bar_size(0).max(0) as u64);
    stage1::run(&mut adev, bdf, bar2);
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

