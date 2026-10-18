/* struct amdgpu_device as the oracles see it: only the fields the Linux
 * functions they run touch, shared by every translation unit (the GC and
 * MMHUB register headers cannot meet in one). */
#pragma once
#include <linux/sched.h>
#include <linux/slab.h>
#include <drm/amd_asic_type.h>
#include "amd_shared.h"
#include "amdgpu_discovery.h"
#include "soc15_hw_ip.h"
#include "discovery.h"
#include "amdgpu_extracted.h"
typedef u64 resource_size_t;
typedef struct { int counter; } atomic_t;
#include "amdgpu_doorbell.h"
#include "soc15_ih_clientid.h"
#include "gmc_types.h"

struct amdgpu_device;
/* The members of the hub and GMC funcs tables (amdgpu_gfxhub.h,
 * amdgpu_mmhub.h, amdgpu_gmc.h) the GMC functions call. */
struct amdgpu_gfxhub_funcs {
	u64 (*get_mc_fb_offset)(struct amdgpu_device *adev);
	int (*gart_enable)(struct amdgpu_device *adev);
	void (*set_fault_enable_default)(struct amdgpu_device *adev, bool value);
	void (*init)(struct amdgpu_device *adev);
};
struct amdgpu_mmhub_funcs {
	u64 (*get_fb_location)(struct amdgpu_device *adev);
	void (*init)(struct amdgpu_device *adev);
	int (*gart_enable)(struct amdgpu_device *adev);
	void (*set_fault_enable_default)(struct amdgpu_device *adev, bool value);
	void (*update_power_gating)(struct amdgpu_device *adev, bool enable);
};
struct amdgpu_gmc_funcs {
	unsigned int (*get_vbios_fb_size)(struct amdgpu_device *adev);
};
struct amdgpu_hdp_funcs {
	void (*init_registers)(struct amdgpu_device *adev);
};
/* What gmc_v9_0_hw_init reads of a buffer object: the GART table's page
 * directory address and GPU offset, given by the port's run. */
struct amdgpu_bo { u64 pd_addr, gpu_offset; };

struct amdgpu_device;
/* The members of struct amdgpu_nbio_funcs (amdgpu_nbio.h) soc15_common_hw_init calls. */
struct amdgpu_nbio_funcs {
	void (*init_registers)(struct amdgpu_device *adev);
	void (*remap_hdp_registers)(struct amdgpu_device *adev);
	void (*enable_doorbell_aperture)(struct amdgpu_device *adev, bool enable);
	void (*sdma_doorbell_range)(struct amdgpu_device *adev, int instance, bool use_doorbell,
				    int doorbell_index, int doorbell_size);
	void (*program_aspm)(struct amdgpu_device *adev);
	u32 (*get_pcie_index_offset)(struct amdgpu_device *adev);
	u32 (*get_pcie_data_offset)(struct amdgpu_device *adev);
	u32 (*get_memsize)(struct amdgpu_device *adev);
};

struct amdgpu_vcn_inst { u32 vcn_config, vcn_codec_disable_mask; };
struct amdgpu_device {
	void *dev;
	struct { u16 device; u8 revision; } *pdev;
	enum amd_asic_type asic_type;
	u32 apu_flags;
	u32 harvest_ip_mask;
	struct { u32 discovery_tmr_size; u8 *discovery_bin;
		 u64 stolen_reserved_offset, stolen_reserved_size, stolen_vga_size, stolen_extended_size;
		 bool keep_stolen_vga_memory; } mman;
	u32 ip_versions[MAX_HWIP][HWIP_MAX_INSTANCE];
	u32 *reg_offset[MAX_HWIP][HWIP_MAX_INSTANCE];
	struct {
		u32 xcc_mask;
		struct {
			u32 max_shader_engines, max_cu_per_sh, max_sh_per_se, max_backends_per_se;
			u32 max_texture_channel_caches, max_gprs, max_gs_threads, gs_vgt_table_depth;
			u32 gs_prim_buffer_depth, double_offchip_lds_buf, num_sc_per_sh, num_packer_per_sc;
			u32 gc_num_tcp_per_sa, gc_num_sdp_interface, gc_num_tcps, gc_num_tcp_per_wpg;
			u32 gc_tcp_l1_size, gc_num_sqc_per_wgp, gc_l1_instruction_cache_size_per_sqc;
			u32 gc_l1_data_cache_size_per_sqc, gc_gl1c_per_sa, gc_gl1c_size_per_instance;
			u32 gc_gl2c_per_gpu, gc_tcp_size_per_cu, gc_tcp_cache_line_size;
			u32 gc_instruction_cache_size_per_sqc, gc_instruction_cache_line_size;
			u32 gc_scalar_data_cache_size_per_sqc, gc_scalar_data_cache_line_size;
			u32 gc_tcc_size, gc_tcc_cache_line_size, gc_num_cu_per_sqc;
		} config;
		struct { u32 wave_front_size, max_waves_per_simd, max_scratch_slots_per_cu, lds_size; } cu_info;
		struct { void *funcs; int rlcg_reg_access_supported; } rlc;
		struct { struct { struct { bool ready; } sched; } ring; } kiq[1];
	} gfx;
	struct amdgpu_gmc {
		u32 num_umc; u64 mall_size; u32 m_half_use;
		bool is_app_apu;
		struct { bool supported, connected_to_cpu; u32 num_physical_nodes, physical_node_id;
			 u64 node_segment_size; } xgmi;
		const struct amdgpu_gmc_funcs *gmc_funcs;
		struct amdgpu_irq_src vm_fault, ecc_irq;
		int vram_width, vram_type, vram_vendor;
		bool translate_further;
		u64 mc_mask, mc_vram_size, real_vram_size, visible_vram_size, aper_base, aper_size;
		u64 fb_start, fb_end, vram_start, vram_end, gart_size, gart_start, gart_end;
		u64 agp_start, agp_end, agp_size;
		u32 vmid0_page_table_depth, vmid0_page_table_block_size;
		int noretry;
		bool flush_pasid_uses_kiq, flush_tlb_needs_extra_type_2;
		struct amdgpu_bo *pdb0_bo;
		int invalidate_lock;
	} gmc;
	struct { u32 smu_prv_buffer_size; } pm;
	struct { const struct amdgpu_gfxhub_funcs *funcs; } gfxhub;
	struct { const struct amdgpu_mmhub_funcs *funcs; } mmhub;
	struct amdgpu_vmhub vmhub[AMDGPU_MAX_VMHUBS];
	unsigned long vmhubs_mask[1];
	struct { u64 max_pfn; u32 num_level, root_level, block_size, fragment_size; u64 vram_base_offset;
		 u32 first_kfd_vmid; } vm_manager;
	struct { struct amdgpu_irq_client client[AMDGPU_IRQ_CLIENTID_MAX]; } irq;
	void *bios;
	struct { struct amdgpu_bo *bo; } gart;
	struct { u64 gpu_addr; } mem_scratch;
	u64 dummy_page_addr;
	u32 pg_flags, usec_timeout;
	bool in_s0ix;
	struct { const struct amdgpu_hdp_funcs *funcs; } hdp;
	struct { int num_crtc; } mode_info;
	struct { void *logical_to_dev_inst, *logical_to_dev_mask; } ip_map;
	u32 aid_mask;
	u32 rev_id;
	struct { const struct { int (*get_hbm_channel_number)(struct amdgpu_device *adev); } *funcs; } df;
	bool need_swiotlb;
	struct { u32 num_instances, sdma_mask; } sdma;
	struct { u32 node_inst_num, active_mask; const struct { void (*init_registers)(struct amdgpu_device *); } *funcs; } umc;
	struct { u32 num_vcn_inst, inst_mask, harvest_config; struct amdgpu_vcn_inst inst[AMDGPU_MAX_VCN_INSTANCES]; } vcn;
	struct { u32 inst_mask, harvest_config; } jpeg;
	struct { u32 num_instances; } vpe;
	struct { u32 harvest_config; } isp;
	u32 flags;
	struct { const struct amdgpu_nbio_funcs *funcs; } nbio;
	struct { u32 reg_offset; u64 bus_addr; } rmmio_remap;
	u64 rmmio_base;
	void *rmmio;
	u32 rmmio_size;
	int pcie_idx_lock;
	struct amdgpu_doorbell_index doorbell_index;
};
