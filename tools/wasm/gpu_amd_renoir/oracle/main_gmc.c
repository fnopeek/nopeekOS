/* The oracle for gmc_v9_0_sw_init up to amdgpu_bo_init: Linux's functions,
 * copied verbatim by extract.py, on the register bases of the discovery
 * binary and the register file of main_common.c. A few registers hold
 * values a posted Renoir has (the presets below, set alike by the port's
 * check); the hubs' functions sit in hub_gc.c and hub_mm.c. Prints the
 * address space layout as gmc_check.rs prints it. */
#include <limits.h>
#include "vram_types.h"
#define ORACLE_COMMON_LIBRARY
#include "main_common.c"
#include "ivsrcid/vmc/irqsrcs_vmc_1_0.h"
#include "dce/dce_12_0_offset.h"
#include "dce/dce_12_0_sh_mask.h"

u32 oracle_rreg32(struct amdgpu_device *adev, u32 reg) { return RREG32(reg); }
void oracle_wreg32(struct amdgpu_device *adev, u32 reg, u32 v) { WREG32(reg, v); }
void oracle_gc_presets(struct amdgpu_device *adev);
void oracle_mm_presets(struct amdgpu_device *adev);
extern const struct amdgpu_gfxhub_funcs oracle_gfxhub_v1_0_funcs;
extern const struct amdgpu_mmhub_funcs oracle_mmhub_v1_0_funcs;

#undef dev_info
#define dev_info(d, ...) printf(__VA_ARGS__)
#define dev_warn(d, ...) printf(__VA_ARGS__)
#define pci_resource_start(p, n) ((u64)0xd0000000)
#define pci_resource_len(p, n) ((u64)256 << 20)
#define amdgpu_passthrough(adev) false
#define amdgpu_device_resize_fb_bar(adev) 0
#define amdgpu_virt_xgmi_migrate_enabled(adev) false
#define hypervisor_is_type(t) false
#define ALIGN(x, a) (((x) + (a) - 1) & ~((u64)(a) - 1))
#define max(a, b) ((a) > (b) ? (a) : (b))
#define min(a, b) ((a) < (b) ? (a) : (b))
#define min_t(t, a, b) ((t)(a) < (t)(b) ? (t)(a) : (t)(b))
#define clamp(v, lo, hi) min(max(v, lo), hi)
#define DIV_ROUND_UP(n, d) (((n) + (d) - 1) / (d))
static int fls64(u64 x) { return x ? 64 - __builtin_clzll(x) : 0; }
static u64 roundup_pow_of_two(u64 x) { return 1ULL << fls64(x - 1); }
#define ilog2(x) (63 - __builtin_clzll(x))
struct sysinfo { unsigned long totalram; unsigned int mem_unit; };
/* Any RAM below 87 TiB clamps vm_size to its minimum, so none will do. */
static void si_meminfo(struct sysinfo *si) { si->totalram = 0; si->mem_unit = 1; }
#define kcalloc(n, s, f) calloc(n, s)
#define spin_lock_init(l) ((void)0)
#define set_bit(b, m) ((m)[0] |= 1UL << (b))
#define dma_set_mask_and_coherent(d, m) 0
#define DMA_BIT_MASK(n) (((n) == 64) ? ~0ULL : ((1ULL << (n)) - 1))
#define drm_need_swiotlb(bits) false
/* Checked by the ATOM oracle on the VBIOS (get_vram_info). */
#define amdgpu_atomfirmware_get_vram_info(adev, w, t, v) 0
/* gmc_check.rs stops where the memory manager begins. */
#define amdgpu_bo_init(adev) (-1000)
static int amdgpu_emu_mode, amdgpu_gart_size = -1, amdgpu_vm_size = -1, amdgpu_vm_block_size = -1;
static int amdgpu_vm_fragment_size = -1, amdgpu_vis_vram_limit, amdgpu_vram_limit = INT_MAX, amdgpu_agp = -1;
/* Multi-AID and NPS paths: not taken on Renoir. */
#define NUM_XCC(m) 0
#define bitmap_set(m, s, n) ((void)0)
#define bitmap_or(d, a, b, n) ((void)0)
#define amdgpu_gmc_init_mem_ranges(adev) 0
/* Past amdgpu_bo_init, which the stub above ends the function at. */
static int gmc_v9_0_gart_init(struct amdgpu_device *adev) { abort(); }
static void gmc_v9_0_init_nps_details(struct amdgpu_device *adev) { abort(); }
static void amdgpu_vm_manager_init(struct amdgpu_device *adev) { abort(); }
static void gmc_v9_0_save_registers(struct amdgpu_device *adev) { abort(); }
static int amdgpu_gmc_ras_sw_init(struct amdgpu_device *adev) { abort(); }
static void amdgpu_gmc_sysfs_init(struct amdgpu_device *adev) { abort(); }
#include "gmc_extracted.c"

static const struct amdgpu_irq_src_funcs *const oracle_funcs = (void *)1;
static struct amdgpu_nbio_funcs nbio_gmc_funcs;
static const struct amdgpu_gmc_funcs oracle_gmc_funcs = { .get_vbios_fb_size = gmc_v9_0_get_vbios_fb_size };

int main(int argc, char **argv)
{
	static struct { u16 device; u8 revision; } pdev = { 0x164c, 0xc1 };
	static struct amdgpu_device a;
	struct amdgpu_device *adev = &a;
	bin_path = argv[1];
	seed_rule = 'z';
	a.pdev = (void *)&pdev;
	a.asic_type = CHIP_RENOIR;
	a.apu_flags = AMD_APU_IS_RENOIR;
	a.flags = CHIP_RENOIR | AMD_IS_APU;
	a.rmmio_size = 512 << 10;
	a.bios = &a;
	if (amdgpu_discovery_reg_base_init(adev))
		return 1;
	nbio_gmc_funcs = nbio_v7_0_funcs;
	nbio_gmc_funcs.get_memsize = nbio_v7_0_get_memsize;
	a.nbio.funcs = &nbio_gmc_funcs;
	pcie_index = nbio_v7_0_get_pcie_index_offset(adev);
	pcie_data = nbio_v7_0_get_pcie_data_offset(adev);
	/* The presets gmc_check.rs sets alike. */
	WREG32(SOC15_REG_OFFSET(NBIO, 0, mmRCC_CONFIG_MEMSIZE), 512);
	oracle_mm_presets(adev);
	oracle_gc_presets(adev);
	WREG32(SOC15_REG_OFFSET(DCE, 0, mmHUBP0_DCSURF_PRI_VIEWPORT_DIMENSION_DCN2), 1080 << 16 | 1920);
	printf("--- presets set\n");
	/* gmc_v9_0_early_init */
	a.gmc.gmc_funcs = &oracle_gmc_funcs;
	a.gmc.vm_fault.num_types = 1;
	a.gmc.vm_fault.funcs = (void *)oracle_funcs;
	a.gmc.ecc_irq.num_types = 1;
	a.gmc.ecc_irq.funcs = (void *)oracle_funcs;
	a.gfxhub.funcs = &oracle_gfxhub_v1_0_funcs;
	a.mmhub.funcs = &oracle_mmhub_v1_0_funcs;
	static struct amdgpu_ip_block gmc_block;
	gmc_block.adev = adev;
	int r = gmc_v9_0_sw_init(&gmc_block);
	printf("gmc_v9_0_sw_init up to amdgpu_bo_init: %d\n", r == -1000 ? 0 : r);
	struct amdgpu_gmc *mc = &a.gmc;
	printf("mc_mask 0x%llx mc_vram_size 0x%llx real 0x%llx visible 0x%llx aper 0x%llx+0x%llx\n",
	       (unsigned long long)mc->mc_mask, (unsigned long long)mc->mc_vram_size,
	       (unsigned long long)mc->real_vram_size, (unsigned long long)mc->visible_vram_size,
	       (unsigned long long)mc->aper_base, (unsigned long long)mc->aper_size);
	printf("fb 0x%llx-0x%llx vram 0x%llx-0x%llx gart 0x%llx 0x%llx-0x%llx agp 0x%llx-0x%llx+0x%llx\n",
	       (unsigned long long)mc->fb_start, (unsigned long long)mc->fb_end, (unsigned long long)mc->vram_start,
	       (unsigned long long)mc->vram_end, (unsigned long long)mc->gart_size, (unsigned long long)mc->gart_start,
	       (unsigned long long)mc->gart_end, (unsigned long long)mc->agp_start, (unsigned long long)mc->agp_end,
	       (unsigned long long)mc->agp_size);
	printf("vm max_pfn 0x%llx num_level %u root_level %u block_size %u fragment_size %u vram_base_offset 0x%llx\n",
	       (unsigned long long)a.vm_manager.max_pfn, a.vm_manager.num_level, a.vm_manager.root_level,
	       a.vm_manager.block_size, a.vm_manager.fragment_size, (unsigned long long)a.vm_manager.vram_base_offset);
	printf("stolen vga 0x%llx extended 0x%llx reserved 0x%llx+0x%llx keep %d vmhubs_mask 0x%lx\n",
	       (unsigned long long)a.mman.stolen_vga_size, (unsigned long long)a.mman.stolen_extended_size,
	       (unsigned long long)a.mman.stolen_reserved_offset, (unsigned long long)a.mman.stolen_reserved_size,
	       a.mman.keep_stolen_vga_memory, a.vmhubs_mask[0]);
	for (int h = 0; h < AMDGPU_MAX_VMHUBS; h++) {
		struct amdgpu_vmhub *v = &a.vmhub[h];
		if (!v->ctx0_ptb_addr_lo32)
			continue;
		printf("vmhub[%d] ptb 0x%x 0x%x inv sem 0x%x req 0x%x ack 0x%x ctx0_cntl 0x%x fault status 0x%x cntl 0x%x"
		       " dist %u %u %u %u\n", h, v->ctx0_ptb_addr_lo32, v->ctx0_ptb_addr_hi32, v->vm_inv_eng0_sem,
		       v->vm_inv_eng0_req, v->vm_inv_eng0_ack, v->vm_context0_cntl, v->vm_l2_pro_fault_status,
		       v->vm_l2_pro_fault_cntl, v->ctx_distance, v->ctx_addr_distance, v->eng_distance,
		       v->eng_addr_distance);
	}
	for (int c = 0; c < AMDGPU_IRQ_CLIENTID_MAX; c++)
		for (int i = 0; a.irq.client[c].sources && i < AMDGPU_MAX_IRQ_SRC_ID; i++)
			if (a.irq.client[c].sources[i])
				printf("irq client %d src %d %s\n", c, i,
				       a.irq.client[c].sources[i] == &a.gmc.vm_fault ? "gmc.vm_fault" : "gmc.ecc_irq");
	return 0;
}
