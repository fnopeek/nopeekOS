//! The port's discovery path on an IP discovery binary, printed line for
//! line as oracle/main_discovery.c prints Linux's. The binary sits at the
//! top of a fake card's VRAM, where amdgpu_discovery_read_binary_from_mem
//! finds it, so the port's own read path runs as it does on a card.

extern crate alloc;

include!("modules.rs");

use amdgpu_device::{amdgpu_device as Adev, HWIP_MAX_INSTANCE, MAX_HWIP};
use regs::*;

const VRAM_MB: u32 = 512;

fn main() {
    let path = std::env::args().nth(1).expect("ip_discovery.bin");
    let bin = std::fs::read(path).expect("read");
    let vram_size = (VRAM_MB as u64) << 20;
    let mut regs = std::collections::HashMap::new();
    regs.insert(mmRCC_CONFIG_MEMSIZE, VRAM_MB);
    regs.insert(mmMP0_SMN_C2PMSG_33, 0x8000_0000);
    let mut vram = bin;
    vram.resize(DISCOVERY_TMR_SIZE as usize, 0);
    npk_sys::fake_card(npk_sys::FakeCard {
        regs,
        mm_index: mmMM_INDEX,
        mm_index_hi: mmMM_INDEX_HI,
        mm_data: mmMM_DATA,
        vram_base: vram_size - DISCOVERY_TMR_OFFSET as u64,
        vram,
        on_write: Vec::new(),
        writes: Vec::new(),
    });

    let mut adev = Adev::new(0x164c, 0xc1, npk_sys::Mmio(0), 512 << 10);
    amdgpu_device::amdgpu_device_init_apu_flags(&mut adev);

    // amdgpu_discovery_set_ip_blocks, default branch
    let r = match amdgpu_discovery::amdgpu_discovery_reg_base_init(&mut adev) {
        Ok(()) => 0,
        Err(e) => e,
    };
    println!("reg_base_init {}", r);
    if r != 0 {
        std::process::exit(1);
    }
    amdgpu_discovery::amdgpu_discovery_harvest_ip(&mut adev);
    println!("get_gfx_info {}", amdgpu_discovery::amdgpu_discovery_get_gfx_info(&mut adev));
    println!("get_mall_info {}", amdgpu_discovery::amdgpu_discovery_get_mall_info(&mut adev));
    println!("get_vcn_info {}", amdgpu_discovery::amdgpu_discovery_get_vcn_info(&mut adev));

    for ip in 0..MAX_HWIP {
        for inst in 0..HWIP_MAX_INSTANCE {
            if adev.ip_versions[ip][inst] != 0 {
                println!("ip_versions[{}][{}] 0x{:08x}", ip, inst, adev.ip_versions[ip][inst]);
            }
            let r = &adev.reg_offset[ip][inst];
            if !r.is_empty() {
                print!("reg_offset[{}][{}]", ip, inst);
                for b in r {
                    print!(" {:#x}", b);
                }
                println!();
            }
        }
    }
    let v = &adev.vcn;
    println!(
        "vcn num_vcn_inst {} inst_mask {:#x} harvest_config {:#x} vcn_config {:#x} codec_disable {:#x}",
        v.num_vcn_inst, v.inst_mask, v.harvest_config, v.inst[0].vcn_config, v.inst[0].vcn_codec_disable_mask
    );
    println!("jpeg inst_mask {:#x} harvest_config {:#x}", adev.jpeg.inst_mask, adev.jpeg.harvest_config);
    println!("sdma num_instances {} sdma_mask {:#x}", adev.sdma.num_instances, adev.sdma.sdma_mask);
    println!("gfx xcc_mask {:#x}", adev.gfx.xcc_mask);
    println!("gmc num_umc {} mall_size {} m_half_use {}", adev.gmc.num_umc, adev.gmc.mall_size, adev.gmc.m_half_use);
    println!("umc node_inst_num {} active_mask {:#x}", adev.umc.node_inst_num, adev.umc.active_mask);
    println!("vpe num_instances {} harvest_ip_mask {:#x} isp harvest_config 0x0", adev.vpe.num_instances, adev.harvest_ip_mask);
    let c = &adev.gfx.config;
    macro_rules! C { ($($f:ident),*) => { $(println!("gfx.config.{} {}", stringify!($f), c.$f);)* } }
    C!(max_shader_engines, max_cu_per_sh, max_sh_per_se, max_backends_per_se, max_texture_channel_caches,
       max_gprs, max_gs_threads, gs_vgt_table_depth, gs_prim_buffer_depth, double_offchip_lds_buf,
       num_sc_per_sh, num_packer_per_sc, gc_num_tcp_per_sa, gc_num_sdp_interface, gc_num_tcps,
       gc_num_tcp_per_wpg, gc_tcp_l1_size, gc_num_sqc_per_wgp, gc_l1_instruction_cache_size_per_sqc,
       gc_l1_data_cache_size_per_sqc, gc_gl1c_per_sa, gc_gl1c_size_per_instance, gc_gl2c_per_gpu,
       gc_tcp_size_per_cu, gc_tcp_cache_line_size, gc_instruction_cache_size_per_sqc,
       gc_instruction_cache_line_size, gc_scalar_data_cache_size_per_sqc, gc_scalar_data_cache_line_size,
       gc_tcc_size, gc_tcc_cache_line_size, gc_num_cu_per_sqc);
    let u = &adev.gfx.cu_info;
    macro_rules! U { ($($f:ident),*) => { $(println!("gfx.cu_info.{} {}", stringify!($f), u.$f);)* } }
    U!(wave_front_size, max_waves_per_simd, max_scratch_slots_per_cu, lds_size);
}
