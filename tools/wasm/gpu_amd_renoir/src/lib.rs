//! gpu_amd_renoir: AMD Renoir/Lucienne graphics, ported from Linux 6.18.55
//! amdgpu.
//!
//! The port follows docs/plan/GPU_AMD_LINUX_MAP.md. This stage runs the
//! start of `amdgpu_device_init` up to `amdgpu_discovery_reg_base_init`:
//! the register BAR, the APU flags, and the IP discovery binary from the top
//! of VRAM, checked and parsed into the version and register bases of every
//! IP. It reports what it found, keeps the binary in its private area and
//! gives the card back; nothing is written to the card but the MM_INDEX
//! window it reads VRAM through.

#![no_std]
// Linux names are kept as they are (`amdgpu_device`, `mmMM_INDEX`), so a
// function can be found in both trees by name.
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
#![allow(dead_code)]

extern crate alloc;

mod amdgpu_device;
mod amdgpu_discovery;
mod linux;
mod regs;
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
/// Lucienne (amdgpu_pciidlist: CHIP_RENOIR | AMD_IS_APU), the IdeaPad.
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
    Line::new().s("v").s(DRIVER_VERSION).s(", stage 0: amdgpu_device_init up to the IP discovery").say();

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
    amdgpu_device::amdgpu_device_renoir_apu_flags(&mut adev);
    Line::new().s("apu_flags ").hex(adev.apu_flags).s(if adev.apu_flags & regs::AMD_APU_IS_RENOIR != 0 {
        " (AMD_APU_IS_RENOIR: renoir_* firmware)"
    } else {
        ""
    }).say();

    // amdgpu_device_ip_early_init -> amdgpu_discovery_set_ip_blocks, default
    // branch -> amdgpu_discovery_reg_base_init.
    if let Err(e) = amdgpu_discovery::amdgpu_discovery_reg_base_init(&mut adev) {
        Line::new().s("amdgpu_discovery_reg_base_init failed (").dec((-e) as u64).s("), giving the card back").say();
        return;
    }
    report_ip_versions(&adev);

    // The binary as read, for the port to check against.
    let bin = &adev.mman.discovery_bin;
    let at = regs::BHDR_SIZE as usize;
    let used = (bin[at] as usize | (bin[at + 1] as usize) << 8).min(bin.len());
    let rc = npk_sys::store(b"priv/gpu_amd_renoir/ip_discovery.bin", &bin[..used]);
    Line::new().s("discovery binary (").dec(used as u64).s(" bytes) ").s(if rc >= 0 {
        "kept in priv/gpu_amd_renoir/ip_discovery.bin"
    } else {
        "could not be stored"
    }).say();
    say("stage 0 done, giving the card back");
}

/// `adev->ip_versions` by hardware IP, as the IP blocks will be chosen
/// from them.
fn report_ip_versions(adev: &amdgpu_device::amdgpu_device) {
    use regs::*;
    const IPS: &[(u32, &str)] = &[
        (GC_HWIP, "GC"), (HDP_HWIP, "HDP"), (SDMA0_HWIP, "SDMA0"), (SDMA1_HWIP, "SDMA1"), (MMHUB_HWIP, "MMHUB"),
        (ATHUB_HWIP, "ATHUB"), (NBIO_HWIP, "NBIO"), (MP0_HWIP, "MP0"), (MP1_HWIP, "MP1"), (UVD_HWIP, "UVD/VCN/JPEG"),
        (VCN1_HWIP, "VCN1"), (VCE_HWIP, "VCE"), (DF_HWIP, "DF"), (DCE_HWIP, "DCE"), (OSSSYS_HWIP, "OSSSYS"),
        (SMUIO_HWIP, "SMUIO"), (PWR_HWIP, "PWR"), (NBIF_HWIP, "NBIF"), (THM_HWIP, "THM"), (CLK_HWIP, "CLK"),
        (UMC_HWIP, "UMC"), (RSMU_HWIP, "RSMU"), (XGMI_HWIP, "XGMI"), (DCI_HWIP, "DCI"), (PCIE_HWIP, "PCIE"),
        (ISP_HWIP, "ISP"),
    ];
    for &(ip, name) in IPS {
        let v = adev.ip_versions[ip as usize][0];
        if v == 0 {
            continue;
        }
        Line::new().s("ip_versions[").s(name).s("][0] = IP_VERSION(").dec((v >> 24) as u64).s(", ")
            .dec(((v >> 16) & 0xff) as u64).s(", ").dec(((v >> 8) & 0xff) as u64).s(") variant ")
            .dec(((v >> 4) & 0xf) as u64).s(" subrev ").dec((v & 0xf) as u64).say();
    }
    Line::new().s("instances: sdma ").dec(adev.sdma_num_instances as u64).s(" (mask ").hex(adev.sdma_mask)
        .s("), vcn ").dec(adev.num_vcn_inst as u64).s(" (mask ").hex(adev.vcn_inst_mask).s("), gc xcc mask ")
        .hex(adev.gfx_xcc_mask).s(", umc ").dec(adev.num_umc as u64).say();
}
