//! gpu_intel_tgl: Intel Tiger Lake graphics, ported from Linux 6.18.55 i915.
//!
//! The port follows docs/plan/GPU_INTEL_LINUX_MAP.md and the call order in
//! docs/plan/GPU_INTEL_TGL_TREE.txt. This stage runs `i915_driver_probe`
//! through `i915_driver_mmio_probe` (device and display info, the PCH,
//! forcewake, the GT's clock, slices, L3 banks and engines from the fuses,
//! the display fuses and raw clock, the full GT reset of `sanitize_gpu`)
//! and the non-display part of `i915_driver_hw_probe` (GGTT probe, fences,
//! stolen memory, bus mastering, PCODE, DRAM). It reports what it found
//! and gives the card back.

#![no_std]
// Linux names are kept as they are (`intel_uncore`, `FORCEWAKE_MT`,
// `__gen12_fw_ranges`), so a function can be found in both trees by name.
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]
#![allow(dead_code)]

mod i915_drv;
mod intel_bios;
mod intel_display_device;
mod intel_dram;
mod intel_engine;
mod intel_ggtt;
mod intel_gt;
mod intel_opregion;
mod intel_pch;
mod intel_pcode;
mod intel_reset;
mod intel_uncore;
mod linux;
mod regs;
mod tables;

use i915_drv::intel_step_info;
use intel_bios::{bdb_block, gmbus_pin};
use intel_gt::engine_info;
use intel_uncore::{i915_range, intel_forcewake_range};
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
        l.s(" at ").s(loc.file()).s(":").dec(loc.line() as u64);
    }
    l.say();
    core::arch::wasm32::unreachable()
}

use linux::{say, Line};

/// `intel_gmch_bridge_setup` + `intel_gmch_bar_setup`: the host bridge at
/// 00:00.0 must have MCHBAR enabled. Linux enables it when the firmware
/// did not; a module may only read another function's config, so a
/// disabled MCHBAR is reported, not fixed.
fn intel_gmch_bar_setup() {
    let mchbar = npk_sys::pci_read_config_at(0, regs::MCHBAR_I965 as i32) as u32;
    if mchbar == 0xffff_ffff {
        linux::drm_err("bridge device not found");
    } else if mchbar & 1 == 0 {
        linux::drm_err("MCHBAR is disabled on the host bridge and cannot be enabled from a module");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    Line::new().s("v").s(DRIVER_VERSION).s(", stage 2b: i915_driver_probe up to the hardware probe and the VBT").say();

    // pci_enable_device: bind (the kernel's own driver hands the card over).
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
    let revision = npk_sys::pci_read_config(0x08) as u32 as u8;
    let Some(regs) = Mmio::map_bar(0, BAR0_PAGES) else {
        say("BAR0 could not be mapped");
        return;
    };

    // i915_driver_create
    let mut i915 = i915_drv::drm_i915_private {
        revision,
        runtime: i915_drv::intel_device_info_driver_create(TGL_GT2),
        display: i915_drv::intel_display_device_probe(revision),
        uncore: intel_uncore::intel_uncore::new(regs),
    };

    // i915_driver_early_probe
    i915_drv::intel_device_info_runtime_init_early(&mut i915.runtime);
    i915_drv::intel_step_init(&mut i915);
    let mut gt = intel_gt::intel_gt::new();
    let pch_type = intel_pch::intel_pch_detect(i915.display.has_display);
    i915_drv::intel_detect_preproduction_hw(&i915);
    Line::new().s("graphics ").dec(i915.runtime.graphics_ip.ver as u64).s(".")
        .dec(i915.runtime.graphics_ip.rel as u64).s(" stepping ")
        .s(i915_drv::intel_step_name(i915.runtime.step.graphics_step))
        .s(", display stepping ").s(i915_drv::intel_step_name(i915.display.step))
        .s(", PCH type ").dec(pch_type as u64).say();

    // i915_driver_mmio_probe
    let has_rcs = intel_engine::has_engine(gt.info.engine_mask, regs::RCS0);
    // HAS_FPGA_DBG_UNCLAIMED(display): the display info flag.
    let fpga_dbg = regs::TGL_DISPLAY_HAS_FPGA_DBG != 0;
    if i915.uncore.intel_uncore_init_mmio(gt.info.engine_mask, has_rcs, fpga_dbg).is_err() {
        say("intel_uncore_init_mmio failed, giving the card back");
        return;
    }
    intel_gmch_bar_setup();
    let mut drt = intel_display_device::tgl_runtime_defaults();
    intel_display_device::intel_display_device_info_runtime_init(&mut i915, &mut drt, pch_type);
    intel_gt::intel_gt_init_mmio(&mut i915, &mut gt);
    // sanitize_gpu
    if !intel_reset::intel_gt_gpu_reset_clobbers_display() {
        if intel_reset::intel_gt_reset_all_engines(&mut i915, &gt).is_err() {
            say("GT reset failed");
        }
    }

    report(&i915, &gt, &drt);

    // i915_driver_hw_probe, without the display parts (OpRegion, bandwidth)
    // and MSI, which comes with the interrupt handler.
    intel_dram::intel_dram_edram_detect(&mut i915);
    // i915_set_dma_info: TGL_DMA_MASK_SIZE bits; DMA buffers come from
    // below 4 GB, inside the mask.
    let mut ggtt = match intel_ggtt::i915_ggtt_probe_hw(regs) {
        Ok(g) => g,
        Err(()) => {
            say("i915_ggtt_probe_hw failed");
            return;
        }
    };
    intel_ggtt::i915_ggtt_init_hw(&mut i915, &mut ggtt);
    let dsm = intel_ggtt::i915_gem_init_stolen(&mut i915);
    // i915_ggtt_enable_hw: GRAPHICS_VER >= 6 has nothing to enable.
    if npk_sys::pci_enable_bus_master() != 0 {
        say("bus mastering could not be enabled");
    }
    // intel_opregion_setup (display), then i915_pcode_init.
    let opregion = intel_opregion::intel_opregion_setup();
    let mut display = intel_bios::intel_display::new(i915.display.ip.ver, pch_type, drt.pipe_mask, drt.port_mask);
    intel_bios::intel_bios_init(&mut display, opregion.as_ref());
    let _ = intel_pcode::intel_pcode_init();
    let dram = intel_dram::intel_dram_detect(&mut i915);

    Line::new().s("GGTT ").dec(ggtt.total >> 20).s(" MB, aperture ").hex((ggtt.gmadr_start >> 32) as u32)
        .s("_").hex(ggtt.gmadr_start as u32).s(" + ").dec(ggtt.mappable_end >> 20).s(" MB, ")
        .dec(ggtt.num_fences as u64).s(" fences").say();
    Line::new().s("GGTT[0] -> ").hex((ggtt.gen8_ggtt_read_entry(0) >> 32) as u32).s("_")
        .hex(ggtt.gen8_ggtt_read_entry(0) as u32).say();
    match dsm {
        Some(d) => {
            Line::new().s("stolen ").hex((d.stolen_start >> 32) as u32).s("_").hex(d.stolen_start as u32)
                .s(" + ").dec((d.stolen_end - d.stolen_start + 1) >> 20).s(" MB, reserved ")
                .hex(d.reserved_start as u32).s(" + ").dec(d.reserved_size >> 20).s(" MB, usable ")
                .dec(d.usable_size >> 20).s(" MB").say();
        }
        None => say("no stolen memory"),
    }
    match &opregion {
        Some(op) if op.vbt_len != 0 => {
            let (ver, bdb_size) = intel_opregion::vbt(op.vbt_len, |v| {
                let le16 = |o: usize| u16::from_le_bytes([v[o], v[o + 1]]);
                let o = regs::VBT_HEADER_BDB_OFFSET as usize;
                let bdb = u32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]]) as usize;
                (le16(bdb + regs::BDB_HEADER_VERSION as usize), le16(bdb + regs::BDB_HEADER_BDB_SIZE as usize))
            });
            Line::new().s("VBT from ").s(op.vbt_source).s(": ").dec(op.vbt_len as u64)
                .s(" bytes, BDB version ").dec(ver as u64).s(", BDB size ").dec(bdb_size as u64).say();
        }
        Some(_) => say("OpRegion without a valid VBT"),
        None => say("no OpRegion"),
    }
    Line::new().s("DRAM ").s(dram.type_name).s(", ").dec(dram.num_channels as u64).s(" channels, ")
        .dec(dram.num_qgv_points as u64).s(" QGV points, ").dec(dram.num_psf_gv_points as u64)
        .s(" PSF GV points").say();

    // Forcewake taken for register access is released by its timer; wait
    // for it, so the GT can enter RC6 once the module is gone.
    let t0 = linux::now_us();
    while i915.uncore.fw_domains_active != 0 && linux::now_us() - t0 < 100_000 {
        npk_sys::sleep(1);
        i915.uncore.run_timers();
    }
    Line::new().s("forcewake held at exit: ").hex(i915.uncore.fw_domains_active).say();
}

fn report(i915: &i915_drv::drm_i915_private, gt: &intel_gt::intel_gt,
          drt: &intel_display_device::intel_display_runtime_info) {
    Line::new().s("forcewake domains ").hex(i915.uncore.fw_domains).say();
    Line::new().s("GT clock ").dec(gt.clock_frequency as u64).s(" Hz, period ")
        .dec(gt.clock_period_ns as u64).s(" ns").say();
    let s = &gt.info.sseu;
    Line::new().s("slices ").hex(s.slice_mask).s(", subslices ").hex(s.subslice_mask[0])
        .s(", EUs ").dec(s.eu_total as u64).s(" (").dec(s.eu_per_subslice as u64)
        .s(" per subslice), L3 banks ").hex(gt.info.l3bank_mask).say();
    let mut l = Line::new();
    l.s("engines ").hex(gt.info.engine_mask as u32).s(":");
    for e in gt.engine.iter().flatten() {
        l.s(" ").s(intel_gt::intel_engine_class_repr(e.class)).dec(e.instance as u64)
            .s("@").hex(e.mmio_base);
    }
    l.say();
    Line::new().s("display pipes ").hex(drt.pipe_mask).s(", dmc ").dec(drt.has_dmc as u64)
        .s(", rawclk ").dec(drt.rawclk_freq as u64).s(" kHz").say();
}
