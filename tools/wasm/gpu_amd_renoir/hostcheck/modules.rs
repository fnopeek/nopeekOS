// The module's sources the host checks compile, as `mod` items of the
// check's crate root (include!d there), so `crate::…` paths resolve as in
// the module.
#[path = "../src/amdgpu_atombios.rs"]
mod amdgpu_atombios;
#[path = "../src/amdgpu_atomfirmware.rs"]
mod amdgpu_atomfirmware;
#[path = "../src/amdgpu_bios.rs"]
mod amdgpu_bios;
#[path = "../src/amdgpu_device.rs"]
mod amdgpu_device;
#[path = "../src/amdgpu_discovery.rs"]
mod amdgpu_discovery;
#[path = "../src/atom.rs"]
mod atom;
#[path = "../src/atom_exec.rs"]
mod atom_exec;
#[path = "../src/ip_blocks.rs"]
mod ip_blocks;
#[path = "../src/linux.rs"]
mod linux;
#[path = "../src/nbio_v7_0.rs"]
mod nbio_v7_0;
#[path = "../src/regs.rs"]
#[allow(dead_code, non_upper_case_globals)]
mod regs;
#[path = "../src/soc15.rs"]
mod soc15;
#[path = "../src/tables.rs"]
#[allow(dead_code, non_upper_case_globals)]
mod tables;
#[path = "../src/amdgpu_dm.rs"]
mod amdgpu_dm;
#[path = "../src/dmub_srv.rs"]
mod dmub_srv;
#[path = "../src/dmub_dcn20.rs"]
mod dmub_dcn20;
#[path = "../src/dc_dmub_srv.rs"]
mod dc_dmub_srv;
#[path = "../src/hw_sequencer.rs"]
mod hw_sequencer;
#[path = "../src/link_detection.rs"]
mod link_detection;
#[path = "../src/link_dpms.rs"]
mod link_dpms;
#[path = "../src/dcn10_hwseq.rs"]
mod dcn10_hwseq;
#[path = "../src/dcn20_hwseq.rs"]
mod dcn20_hwseq;
#[path = "../src/dcn21_hwseq.rs"]
mod dcn21_hwseq;
#[path = "../src/dmub_abm.rs"]
mod dmub_abm;
#[path = "../src/amdgpu_psp.rs"]
mod amdgpu_psp;
#[path = "../src/amdgpu_ras.rs"]
mod amdgpu_ras;
#[path = "../src/amdgpu_smu.rs"]
mod amdgpu_smu;
#[path = "../src/amdgpu_ucode.rs"]
mod amdgpu_ucode;
#[path = "../src/gfx_v9_0.rs"]
mod gfx_v9_0;
#[path = "../src/gmc_v9_0.rs"]
mod gmc_v9_0;
#[path = "../src/gpudev.rs"]
mod gpudev;
#[path = "../src/import.rs"]
mod import;
#[path = "../src/jpeg_v2_0.rs"]
mod jpeg_v2_0;
#[path = "../src/sdma_v4_0.rs"]
mod sdma_v4_0;
#[path = "../src/vcn_v2_0.rs"]
mod vcn_v2_0;
#[path = "../src/vega10_ih.rs"]
mod vega10_ih;
#[path = "../src/amdgpu_gmc.rs"]
mod amdgpu_gmc;
#[path = "../src/amdgpu_irq.rs"]
mod amdgpu_irq;
#[path = "../src/amdgpu_vm.rs"]
mod amdgpu_vm;
#[path = "../src/gfxhub_v1_0.rs"]
mod gfxhub_v1_0;
#[path = "../src/mmhub_v1_0.rs"]
mod mmhub_v1_0;
#[path = "../src/gpu_buddy.rs"]
mod gpu_buddy;
#[path = "../src/ttm.rs"]
mod ttm;
#[path = "../src/drm_mm.rs"]
mod drm_mm;
#[path = "../src/amdgpu_vram_mgr.rs"]
mod amdgpu_vram_mgr;
#[path = "../src/amdgpu_object.rs"]
mod amdgpu_object;
#[path = "../src/amdgpu_ttm.rs"]
mod amdgpu_ttm;
#[path = "../src/amdgpu_gart.rs"]
mod amdgpu_gart;
#[path = "../src/hdp_v4_0.rs"]
mod hdp_v4_0;
#[path = "../src/amdgpu_ring.rs"]
mod amdgpu_ring;
#[path = "../src/amdgpu_fence.rs"]
mod amdgpu_fence;
#[path = "../src/amdgpu_ib.rs"]
mod amdgpu_ib;
#[path = "../src/dma_fence.rs"]
mod dma_fence;
#[path = "../src/dma_resv.rs"]
mod dma_resv;
#[path = "../src/workqueue.rs"]
mod workqueue;
#[path = "../src/drm_sched.rs"]
mod drm_sched;
#[path = "../src/amdgpu_sync.rs"]
mod amdgpu_sync;
#[path = "../src/amdgpu_job.rs"]
mod amdgpu_job;
#[path = "../src/amdgpu_ids.rs"]
mod amdgpu_ids;
#[path = "../src/amdgpu_vm_pt.rs"]
mod amdgpu_vm_pt;
#[path = "../src/amdgpu_vm_sdma.rs"]
mod amdgpu_vm_sdma;
#[path = "../src/amdgpu_res_cursor.rs"]
mod amdgpu_res_cursor;
#[path = "../src/amdgpu_ctx.rs"]
mod amdgpu_ctx;
#[path = "../src/amdgpu_cs.rs"]
mod amdgpu_cs;
#[path = "../src/shaders_gen.rs"]
mod shaders_gen;
#[path = "../src/radv_stream.rs"]
mod radv_stream;
#[path = "../src/compose.rs"]
mod compose;
#[path = "../src/athub_v1_0.rs"]
mod athub_v1_0;
#[path = "../src/native_gen.rs"]
mod native_gen;
#[path = "../src/amdgpu_kms.rs"]
mod amdgpu_kms;
#[path = "../src/amdgpu_virtio.rs"]
mod amdgpu_virtio;
#[path = "../src/bios_parser2.rs"]
mod bios_parser2;
#[path = "../src/command_table2.rs"]
mod command_table2;
#[path = "../src/dc_types.rs"]
mod dc_types;
#[path = "../src/clk_mgr.rs"]
mod clk_mgr;
#[path = "../src/dc_helper.rs"]
mod dc_helper;
#[path = "../src/dm_pp_smu.rs"]
mod dm_pp_smu;
#[path = "../src/fixpt31_32.rs"]
mod fixpt31_32;
#[path = "../src/rn_clk_mgr.rs"]
mod rn_clk_mgr;
#[path = "../src/gpio_service.rs"]
mod gpio_service;
#[path = "../src/dc.rs"]
mod dc;
#[path = "../src/dc_resource.rs"]
mod dc_resource;
#[path = "../src/dccg.rs"]
mod dccg;
#[path = "../src/dce_audio.rs"]
mod dce_audio;
#[path = "../src/dce_aux.rs"]
mod dce_aux;
#[path = "../src/dce_hwseq.rs"]
mod dce_hwseq;
#[path = "../src/dce_clock_source.rs"]
mod dce_clock_source;
#[path = "../src/dce_i2c_hw.rs"]
mod dce_i2c_hw;
#[path = "../src/dcn10_ipp.rs"]
mod dcn10_ipp;
#[path = "../src/dcn20_dpp.rs"]
mod dcn20_dpp;
#[path = "../src/dcn20_dsc.rs"]
mod dcn20_dsc;
#[path = "../src/dcn20_dwb.rs"]
mod dcn20_dwb;
#[path = "../src/dcn20_hubbub.rs"]
mod dcn20_hubbub;
#[path = "../src/dcn20_mmhubbub.rs"]
mod dcn20_mmhubbub;
#[path = "../src/dcn20_mpc.rs"]
mod dcn20_mpc;
#[path = "../src/dcn20_opp.rs"]
mod dcn20_opp;
#[path = "../src/dcn20_optc.rs"]
mod dcn20_optc;
#[path = "../src/dcn20_stream_encoder.rs"]
mod dcn20_stream_encoder;
#[path = "../src/dcn21_hubp.rs"]
mod dcn21_hubp;
#[path = "../src/dcn21_resource.rs"]
mod dcn21_resource;
#[path = "../src/virtual_stream_encoder.rs"]
mod virtual_stream_encoder;
#[path = "../src/dml_structs.rs"]
mod dml_structs;
#[path = "../src/dcn20_fpu.rs"]
mod dcn20_fpu;
#[path = "../src/gpio_base.rs"]
mod gpio_base;
#[path = "../src/link_ddc.rs"]
mod link_ddc;
#[path = "../src/link_hpd.rs"]
mod link_hpd;
#[path = "../src/dce_panel_cntl.rs"]
mod dce_panel_cntl;
#[path = "../src/dcn21_link_encoder.rs"]
mod dcn21_link_encoder;
#[path = "../src/link_factory.rs"]
mod link_factory;
#[path = "../src/dc_state.rs"]
mod dc_state;
#[path = "../src/dc_hw_types.rs"]
mod dc_hw_types;
#[path = "../src/dc_stream.rs"]
mod dc_stream;
#[path = "../src/dc_dsc.rs"]
mod dc_dsc;
#[path = "../src/dcn20_resource.rs"]
mod dcn20_resource;
#[path = "../src/dc_ddc_types.rs"]
mod dc_ddc_types;
#[path = "../src/link_dpcd.rs"]
mod link_dpcd;
#[path = "../src/drm_dp_helper.rs"]
mod drm_dp_helper;
#[path = "../src/i2c.rs"]
mod i2c;
#[path = "../src/drm_connector.rs"]
mod drm_connector;
#[path = "../src/drm_edid.rs"]
mod drm_edid;
#[path = "../src/amdgpu_dm_helpers.rs"]
mod amdgpu_dm_helpers;
#[path = "../src/dc_sink.rs"]
mod dc_sink;
#[path = "../src/dc_dp_types.rs"]
mod dc_dp_types;
#[path = "../src/link_dp_capability.rs"]
mod link_dp_capability;
#[path = "../src/hdcp_msg.rs"]
mod hdcp_msg;
