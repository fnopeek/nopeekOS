/* The oracle for the discovery path: Linux's amdgpu_discovery functions,
 * copied verbatim by extract.py, from amdgpu_discovery_reg_base_init through
 * amdgpu_discovery_get_vcn_info, on an IP discovery binary. The binary
 * stands in for the top of VRAM: amdgpu_discovery_read_binary_from_mem is
 * the one function replaced, it copies the file.
 *
 * struct amdgpu_device carries only the fields those functions touch; the
 * device is 1002:164c rev 0xc1, CHIP_RENOIR, AMD_APU_IS_RENOIR. */
#include <linux/sched.h>
#include <linux/slab.h>
#include <drm/amd_asic_type.h>
#include "amd_shared.h"
#include "amdgpu_discovery.h"
#include "soc15_hw_ip.h"
#include "discovery.h"
#include "amdgpu_extracted.h"

#define dev_err(d, ...) printf("dev_err: " __VA_ARGS__)
#define dev_info(d, ...) printf("dev_info: " __VA_ARGS__)
#define dev_dbg(d, ...) ((void)0)
#define drm_dbg(d, ...) ((void)0)
#define DRM_ERROR(...) printf("DRM_ERROR: " __VA_ARGS__)
#define DRM_DEBUG(...) ((void)0)
#define struct_size(p, member, n) (sizeof(*(p)) + sizeof((p)->member[0]) * (n))
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define RREG32(r) 0u
#define amdgpu_ras_query_boot_status(adev, n) ((void)0)
#define amdgpu_discovery_read_binary_from_file(adev, bin, name) (-ENOENT)
static int amdgpu_discovery = -1;

struct amdgpu_vcn_inst { u32 vcn_config, vcn_codec_disable_mask; };
struct amdgpu_device {
	void *dev;
	struct { u16 device; u8 revision; } *pdev;
	enum amd_asic_type asic_type;
	u32 apu_flags;
	u32 harvest_ip_mask;
	struct { u32 discovery_tmr_size; u8 *discovery_bin; } mman;
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
	} gfx;
	struct { u32 num_umc; u64 mall_size; u32 m_half_use; } gmc;
	struct { u32 num_instances, sdma_mask; } sdma;
	struct { u32 node_inst_num, active_mask; } umc;
	struct { u32 num_vcn_inst, inst_mask, harvest_config; struct amdgpu_vcn_inst inst[AMDGPU_MAX_VCN_INSTANCES]; } vcn;
	struct { u32 inst_mask, harvest_config; } jpeg;
	struct { u32 num_instances; } vpe;
	struct { u32 harvest_config; } isp;
};

static const char *bin_path;

static int amdgpu_discovery_read_binary_from_mem(struct amdgpu_device *adev, uint8_t *binary)
{
	FILE *f = fopen(bin_path, "rb");
	if (!f)
		return -ENOENT;
	fread(binary, 1, adev->mman.discovery_tmr_size, f);
	fclose(f);
	return 0;
}

#include "discovery_extracted.c"

int main(int argc, char **argv)
{
	static struct { u16 device; u8 revision; } pdev = { 0x164c, 0xc1 };
	static struct amdgpu_device adev;
	bin_path = argv[1];
	adev.pdev = (void *)&pdev;
	adev.asic_type = CHIP_RENOIR;
	adev.apu_flags = AMD_APU_IS_RENOIR;

	/* amdgpu_discovery_set_ip_blocks, default branch */
	int r = amdgpu_discovery_reg_base_init(&adev);
	printf("reg_base_init %d\n", r);
	if (r)
		return 1;
	amdgpu_discovery_harvest_ip(&adev);
	printf("get_gfx_info %d\n", amdgpu_discovery_get_gfx_info(&adev));
	printf("get_mall_info %d\n", amdgpu_discovery_get_mall_info(&adev));
	printf("get_vcn_info %d\n", amdgpu_discovery_get_vcn_info(&adev));

	for (int ip = 0; ip < MAX_HWIP; ip++)
		for (int inst = 0; inst < HWIP_MAX_INSTANCE; inst++) {
			if (adev.ip_versions[ip][inst])
				printf("ip_versions[%d][%d] 0x%08x\n", ip, inst, adev.ip_versions[ip][inst]);
			if (adev.reg_offset[ip][inst]) {
				/* reg_offset points at ip->base_address in the binary */
				struct ip_v4 *e = (struct ip_v4 *)((u8 *)adev.reg_offset[ip][inst] -
								 offsetof(struct ip_v4, base_address));
				printf("reg_offset[%d][%d]", ip, inst);
				for (int k = 0; k < e->num_base_address; k++)
					printf(" 0x%x", adev.reg_offset[ip][inst][k]);
				printf("\n");
			}
		}
	printf("vcn num_vcn_inst %u inst_mask 0x%x harvest_config 0x%x vcn_config 0x%x codec_disable 0x%x\n",
	       adev.vcn.num_vcn_inst, adev.vcn.inst_mask, adev.vcn.harvest_config, adev.vcn.inst[0].vcn_config,
	       adev.vcn.inst[0].vcn_codec_disable_mask);
	printf("jpeg inst_mask 0x%x harvest_config 0x%x\n", adev.jpeg.inst_mask, adev.jpeg.harvest_config);
	printf("sdma num_instances %u sdma_mask 0x%x\n", adev.sdma.num_instances, adev.sdma.sdma_mask);
	printf("gfx xcc_mask 0x%x\n", adev.gfx.xcc_mask);
	printf("gmc num_umc %u mall_size %llu m_half_use %u\n", adev.gmc.num_umc,
	       (unsigned long long)adev.gmc.mall_size, adev.gmc.m_half_use);
	printf("umc node_inst_num %u active_mask 0x%x\n", adev.umc.node_inst_num, adev.umc.active_mask);
	printf("vpe num_instances %u harvest_ip_mask 0x%x isp harvest_config 0x%x\n", adev.vpe.num_instances,
	       adev.harvest_ip_mask, adev.isp.harvest_config);
#define C(f) printf("gfx.config." #f " %u\n", adev.gfx.config.f)
	C(max_shader_engines); C(max_cu_per_sh); C(max_sh_per_se); C(max_backends_per_se);
	C(max_texture_channel_caches); C(max_gprs); C(max_gs_threads); C(gs_vgt_table_depth);
	C(gs_prim_buffer_depth); C(double_offchip_lds_buf); C(num_sc_per_sh); C(num_packer_per_sc);
	C(gc_num_tcp_per_sa); C(gc_num_sdp_interface); C(gc_num_tcps); C(gc_num_tcp_per_wpg);
	C(gc_tcp_l1_size); C(gc_num_sqc_per_wgp); C(gc_l1_instruction_cache_size_per_sqc);
	C(gc_l1_data_cache_size_per_sqc); C(gc_gl1c_per_sa); C(gc_gl1c_size_per_instance);
	C(gc_gl2c_per_gpu); C(gc_tcp_size_per_cu); C(gc_tcp_cache_line_size);
	C(gc_instruction_cache_size_per_sqc); C(gc_instruction_cache_line_size);
	C(gc_scalar_data_cache_size_per_sqc); C(gc_scalar_data_cache_line_size);
	C(gc_tcc_size); C(gc_tcc_cache_line_size); C(gc_num_cu_per_sqc);
#define U(f) printf("gfx.cu_info." #f " %u\n", adev.gfx.cu_info.f)
	U(wave_front_size); U(max_waves_per_simd); U(max_scratch_slots_per_cu); U(lds_size);
	return 0;
}
