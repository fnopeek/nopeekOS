//! gpu_bcm2712: the Raspberry Pi 5 display module. It binds the display
//! set of the device tree (HVS, pixel valves, HDMI, DDC and their
//! interrupt controllers), takes the screen over from the firmware the
//! way Linux's vc4 does when it binds, and shows the framebuffer the
//! kernel draws into on the first connected HDMI output: in a mode of
//! the framebuffer's size when the monitor has one, else in the mode
//! Linux's fbdev client would pick, with the HVS scaling.
//!
//! The compositor keeps drawing on the CPU into that framebuffer
//! (`gpudev_ready` without the copy capability): the module only scans it
//! out. The driver itself is `gpu_bcm2712_core`, replayed against a Linux
//! run on the host.

#![no_std]

extern crate alloc;

mod hw;

use drm_core::drm_fourcc::drm_format_info;
use drm_core::drm_plane::drm_framebuffer;
use drm_core::linux::Line;
use gpu_bcm2712_core::vc4_drv::Vc4;
use hw::PiHw;

// The default rights plus HARDWARE, which binding the display set
// requires. A section replaces the default, so READ, EXECUTE and RENDER
// are listed; RENDER also makes `run` start the module as a process of its
// own (the set is never bound to the shell's pid 0).
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x01 | 0x04 | 0x08 | 0x40]; // READ|EXEC|RENDER|HARDWARE

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

/// `kernel/src/gpu/gpudev.rs`: the protocol version, a buffer's
/// description and one job.
const GPUDEV_VERSION: i32 = 2;
const JOB_BYTES: usize = 32;

/// The mask bits of `npk_sys::wait`.
const WAIT_IRQ: i32 = 2;
const WAIT_GPU_CMD: i32 = 64;

fn printk(b: &[u8]) {
    let mut v = alloc::vec::Vec::with_capacity(b.len() + 16);
    v.extend_from_slice(b"[gpu_bcm2712] ");
    v.extend_from_slice(b);
    v.push(b'\n');
    npk_sys::log(&v);
}

/// The kernel's framebuffer, gpudev buffer 0.
struct KernelFb {
    phys: u64,
    pitch: u32,
    width: u32,
    height: u32,
    format: u32,
}

fn kernel_fb() -> Option<KernelFb> {
    let mut o = [0u8; 40];
    if npk_sys::gpudev_bo_info(0, &mut o) < 0 {
        return None;
    }
    let u32_at = |i: usize| u32::from_le_bytes([o[i], o[i + 1], o[i + 2], o[i + 3]]);
    Some(KernelFb {
        phys: u64::from_le_bytes([o[0], o[1], o[2], o[3], o[4], o[5], o[6], o[7]]),
        pitch: u32_at(16),
        width: u32_at(20),
        height: u32_at(24),
        format: u32_at(28),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    drm_core::linux::set_printk(printk);
    Line::new().s("v").s(env!("CARGO_PKG_VERSION")).s(" starting").say();

    if npk_sys::dt_bind(b"brcm,bcm2712-hvs") != 0 {
        Line::new().s("no display set to bind (needs the Pi 5, HARDWARE and a process of its own)").say();
        return;
    }
    let Some(mut hw) = PiHw::map() else {
        Line::new().s("a register window or interrupt of the display set is missing").say();
        return;
    };

    if npk_sys::gpudev_register(GPUDEV_VERSION) < 0 {
        Line::new().s("the GPU service is taken, not binding").say();
        return;
    }
    let Some(kfb) = kernel_fb() else {
        Line::new().s("no framebuffer from the kernel").say();
        return;
    };
    let Some(format) = drm_format_info(kfb.format) else {
        Line::new().s("framebuffer format not known: ").hex(kfb.format).say();
        return;
    };

    // From here on the firmware no longer drives the display.
    let mut vc4 = match Vc4::bind(&mut hw) {
        Ok(v) => v,
        Err(e) => {
            Line::new().s("bind failed: ").dec(e.unsigned_abs() as u64).say();
            return;
        }
    };

    let client = vc4.fbdev_probe(&mut hw);
    let modesets = vc4.takeover_modesets(&client, kfb.width, kfb.height);
    let fb = drm_framebuffer {
        format,
        width: kfb.width,
        height: kfb.height,
        pitches: [kfb.pitch, 0, 0, 0],
        offsets: [0; 4],
        modifier: drm_core::regs::DRM_FORMAT_MOD_LINEAR as u64,
        dma_addr: [kfb.phys, 0, 0, 0],
    };

    match modesets.modesets.iter().find(|m| m.mode.is_some()) {
        None => Line::new().s("no monitor connected").say(),
        Some(ms) => {
            let m = ms.mode.as_ref().unwrap();
            let conn = &vc4.connectors[ms.connectors[0]].name.clone();
            let ret = vc4.takeover_commit(&mut hw, &modesets, &fb);
            let mut l = Line::new();
            l.s(conn).s(" ").dec(m.hdisplay as u64).s("x").dec(m.vdisplay as u64).s("@")
                .dec(vc4.vrefresh(ms.crtc as usize) as u64).s(", framebuffer ")
                .dec(kfb.width as u64).s("x").dec(kfb.height as u64).s(" at ").hex(kfb.phys as u32);
            if ret != 0 {
                l.s(": commit failed ").dec(ret.unsigned_abs() as u64);
            }
            l.say();
        }
    }

    // The compositor stays on the CPU, drawing into the framebuffer the
    // HVS shows now.
    npk_sys::gpudev_ready(0);

    serve(&mut vc4, &mut hw);
}

/// The interrupts and the timed work of a running display, and the GPU
/// service's queue, which stays empty without the copy capability.
fn serve(vc4: &mut Vc4, hw: &mut PiHw) {
    let mut jobs = [0u8; JOB_BYTES * 16];
    let mut seen = hw.arm_all();
    loop {
        let now = npk_sys::now_us() as u64;
        let mut timeout_ms = 1000u64;
        for h in vc4.hdmi.iter() {
            if let Some(due) = h.scrambling_work {
                timeout_ms = timeout_ms.min(due.saturating_sub(now) / 1000);
            }
        }

        npk_sys::wait(WAIT_IRQ | WAIT_GPU_CMD, timeout_ms.max(1) as i32);

        let fired = hw.fired(&mut seen);
        for pv in 0..vc4.crtc.len() {
            if fired.pv[pv] {
                vc4.crtc_irq_handler(hw, pv);
            }
        }
        if fired.disp_intr {
            vc4.disp_intr_handler(hw);
        }
        if fired.aon_intr {
            let ports = vc4.aon_intr_handler(hw);
            if ports != 0 {
                Line::new().s("hotplug on HDMI port mask ").dec(ports as u64).s(" (not followed yet)").say();
            }
        }
        seen = hw.arm_all();

        let now = npk_sys::now_us() as u64;
        for i in 0..vc4.hdmi.len() {
            if vc4.hdmi[i].scrambling_work.is_some_and(|due| now >= due) {
                let h = &mut vc4.hdmi[i];
                h.scrambling_wq(hw, &mut vc4.bsc);
            }
        }

        let n = npk_sys::gpudev_poll(&mut jobs);
        if n < 0 {
            Line::new().s("no longer the GPU service, stopping").say();
            return;
        }
        for j in 0..n as usize {
            let seq = i32::from_le_bytes([jobs[j * JOB_BYTES], jobs[j * JOB_BYTES + 1],
                jobs[j * JOB_BYTES + 2], jobs[j * JOB_BYTES + 3]]);
            npk_sys::gpudev_complete(seq, -1);
        }
    }
}
