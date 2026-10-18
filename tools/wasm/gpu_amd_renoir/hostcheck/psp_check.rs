//! The port's PSP command path on the host run: stage1::run to the end on
//! the fake card, whose PSP answers every frame; then the inputs the
//! oracle needs (buffer addresses, TMR, the ucode table, the TA sizes and
//! shared buffers) and every frame with its command, as
//! oracle/main_psp.c prints Linux's.

extern crate alloc;

include!("modules.rs");
#[path = "../src/stage1.rs"]
mod stage1;

use regs::*;

include!("fake_renoir.rs");

fn main() {
    let mut adev = fake_renoir();
    npk_sys::quiet(true);
    stage1::run(&mut adev, BDF, (0xfc00_0000, 2 << 20));
    npk_sys::quiet(false);

    let p = &adev.psp;
    let (size, pa) = match p.tmr_bo {
        Some(h) => {
            let gpu = amdgpu_object::amdgpu_bo_gpu_offset(&adev, h);
            (amdgpu_object::amdgpu_bo_size(&adev, h), amdgpu_gmc::amdgpu_gmc_vram_mc2pa(&adev, gpu))
        }
        None => (0, 0),
    };
    let ucodes: Vec<_> = adev.firmware.ucode.iter().enumerate().filter(|(_, u)| u.fw.is_some()).collect();
    println!(
        "inputs fence {:x} cmd {:x} fw_pri {:x} ring {:x} tmr {:x} {:x} {:x} psp_timeout {} ucodes {}",
        p.buf.fence_buf_mc_addr, p.buf.cmd_buf_mc_addr, p.buf.fw_pri_mc_addr, p.km_ring.ring_mem_mc_addr,
        p.tmr_mc_addr, size, pa, adev.psp_timeout, ucodes.len()
    );
    for (i, u) in ucodes {
        println!("ucode {} {} {:x} {}", i, u.ucode_id, u.mc_addr, u.ucode_size);
    }
    let ta = |name: &str, c: &amdgpu_psp::ta_context| {
        println!("{} {} {:x}", name, c.bin_desc.size_bytes, c.mem_context.shared_mc_addr);
    };
    // The TA sizes as early_init parsed them: a TA whose query fails has
    // its size cleared by the run ("don't try again").
    let mut parsed = amdgpu_device::amdgpu_device::new(0x164c, 0xc1, npk_sys::Mmio(0), 0);
    parsed.ip_versions = adev.ip_versions;
    parsed.apu_flags = adev.apu_flags;
    amdgpu_psp::psp_v12_0_init_microcode(&mut parsed);
    let q = &parsed.psp;
    for (name, c, early) in [
        ("asd", &p.asd_context, &q.asd_context),
        ("hdcp", &p.hdcp_context.context, &q.hdcp_context.context),
        ("dtm", &p.dtm_context.context, &q.dtm_context.context),
        ("securedisplay", &p.securedisplay_context.context, &q.securedisplay_context.context),
    ] {
        let mut c = *c;
        c.bin_desc.size_bytes = early.bin_desc.size_bytes;
        ta(name, &c);
    }

    println!("--- psp");
    // The first frame is the ring's: frames from the TMR load on.
    for (n, (frame, cmd)) in npk_sys::fake_psp_log().iter().enumerate() {
        print!("frame {}:", n);
        for d in frame {
            print!(" {:08x}", d);
        }
        print!("\ncmd {}:", n);
        for (i, d) in cmd.iter().enumerate() {
            if *d != 0 {
                print!(" {:x}={:08x}", i, d);
            }
        }
        println!();
    }
}
