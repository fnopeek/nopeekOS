/* The oracle for gmc_v9_0_sw_init up to amdgpu_bo_init: Linux's functions,
 * copied verbatim by extract.py, on the register bases of the discovery
 * binary and the register file of main_common.c. A few registers hold
 * values a posted Renoir has (the presets below, set alike by the port's
 * check); the hubs' functions sit in hub_gc.c and hub_mm.c. Prints the
 * address space layout as gmc_check.rs prints it. With inputs it runs
 * gmc_v9_0_hw_init, and with `late` the IH, GMC and COMMON blocks' late
 * life (see late()), whose IH and ATHUB functions sit in hub_ih.c and
 * hub_athub.c. */
#include <limits.h>
#include "vram_types.h"
#define ORACLE_COMMON_LIBRARY
#include "main_common.c"
#include "ivsrcid/vmc/irqsrcs_vmc_1_0.h"
#include "dce/dce_12_0_offset.h"
#include "dce/dce_12_0_sh_mask.h"
#include "gc/gc_9_0_sh_mask.h"

u32 oracle_rreg32(struct amdgpu_device *adev, u32 reg) { return RREG32(reg); }
void oracle_wreg32(struct amdgpu_device *adev, u32 reg, u32 v) { WREG32(reg, v); }
void oracle_gc_presets(struct amdgpu_device *adev);
void oracle_mm_presets(struct amdgpu_device *adev);
extern const struct amdgpu_gfxhub_funcs oracle_gfxhub_v1_0_funcs;
extern const struct amdgpu_mmhub_funcs oracle_mmhub_v1_0_funcs;
extern const struct amdgpu_hdp_funcs oracle_hdp_v4_0_funcs;

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
/* gmc_v9_0_hw_init's world on bare metal before the rings: no KIQ, no
 * reset in flight, no PDB0, no VRAM check outside emulation. */
#define AMDGPU_REGS_NO_KIQ (1 << 1)
#define RREG32_NO_KIQ(r) RREG32(r)
#define amdgpu_sriov_runtime(adev) false
#define spin_lock(l) ((void)0)
#define spin_unlock(l) ((void)0)
#define udelay(n) ((void)0)
#define DRM_INFO(...) printf(__VA_ARGS__)
#undef DRM_ERROR
#define DRM_ERROR(...) printf(__VA_ARGS__)
#define BUG_ON(c) ((void)0)
#define for_each_set_bit(bit, addr, size) for ((bit) = 0; (bit) < (size); (bit)++) if (((addr)[0] >> (bit)) & 1)
#define amdgpu_gmc_fw_reg_write_reg_wait(adev, req, ack, inv, mask, inst) abort()
#define amdgpu_gmc_init_pdb0(adev) abort()
#define amdgpu_gmc_vram_checking(adev) (abort(), 0)
#define amdgpu_asic_flush_hdp(adev, ring) abort()
#define amdgpu_ring_emit_hdp_flush(ring) abort()
/* The GART's pages are not registers; gtt_mgr_recover writes only PTEs. */
#define amdgpu_gtt_mgr_recover(mgr) ((void)0)
#define amdgpu_bo_gpu_offset(bo) ((bo)->gpu_offset)
static int amdgpu_vm_fault_stop;
/* No RAS block objects on Renoir. */
#define amdgpu_ras_reset_error_count(adev, block) (-EOPNOTSUPP)
#define amdgpu_ras_is_supported(adev, block) 0
#define amdgpu_ras_is_rma(adev) 0
/* amdgpu_device_xcc_rreg/wreg on bare metal with one XCC: RREG32/WREG32. */
#define RREG32_XCC(reg, inst) RREG32(reg)
#define WREG32_XCC(reg, v, inst) WREG32(reg, v)
bool amdgpu_irq_enabled(struct amdgpu_device *adev, struct amdgpu_irq_src *src, unsigned int type);
int athub_v1_0_set_clockgating(struct amdgpu_device *adev, enum amd_clockgating_state state);
/* MMHUB 1.5.0 has no golden settings; the tables name MMHUB registers,
 * which cannot meet the GC headers in this unit. */
static void gmc_v9_0_init_golden_registers(struct amdgpu_device *adev)
{
	u32 v = amdgpu_ip_version(adev, MMHUB_HWIP, 0);

	if (v == IP_VERSION(9, 0, 0) || v == IP_VERSION(9, 4, 0) || v == IP_VERSION(9, 1, 0) ||
	    v == IP_VERSION(9, 2, 0))
		abort();
}
#include "gmc_extracted.c"

static const struct amdgpu_irq_src_funcs *const oracle_funcs = (void *)1;
static struct amdgpu_nbio_funcs nbio_gmc_funcs;
static const struct amdgpu_gmc_funcs oracle_gmc_funcs = { .get_vbios_fb_size = gmc_v9_0_get_vbios_fb_size };

void oracle_ih_init_register_offset(struct amdgpu_device *adev);
int oracle_ih_hw_init(struct amdgpu_ip_block *b);
int oracle_ih_hw_fini(struct amdgpu_ip_block *b);
int oracle_ih_set_clockgating_state(struct amdgpu_ip_block *b, enum amd_clockgating_state s);

/* The late mode: on the run's state up to the end of sw_init, the IH
 * block's hw_init, GMC's late_init, the clock gating of COMMON, GMC and IH
 * on and off, and the hw_fini of IH, GMC and COMMON. The port's register
 * file and its IH rings and rings come on stdin from soc4_check.rs; a
 * register never written reads SEED(reg) by the rule `rule` names. */
static int late(struct amdgpu_device *adev, struct amdgpu_ip_block *gmc_block, const char *rule)
{
	static const struct amdgpu_irq_src_funcs vm_fault_funcs = { .set = gmc_v9_0_vm_fault_interrupt_state };
	static struct amdgpu_nbio_funcs nbio_funcs;
	static struct { int level; } init_level = { AMDGPU_INIT_LEVEL_DEFAULT };
	static struct amdgpu_ring rings[AMDGPU_MAX_RINGS];
	static struct amdgpu_ip_block common, ih;
	struct amdgpu_ih_ring *ihs[] = { &adev->irq.ih, &adev->irq.ih1, &adev->irq.ih2, &adev->irq.ih_soft };
	unsigned long long cg, gpu, wptr, dummy;
	u32 msi, k, p, q, x, y, z;
	char kw[16], name[16];

	if (scanf(" inputs cg %llx pg %x msi %u dummy %llx", &cg, &adev->pg_flags, &msi, &dummy) != 4)
		return 2;
	adev->cg_flags = cg;
	adev->irq.msi_enabled = msi;
	adev->dummy_page_addr = dummy;
	ndirect = nindirect = 0;
	while (scanf(" %15s", kw) == 1 && strcmp(kw, "end")) {
		if (!strcmp(kw, "ih") && scanf(" %u %u %llx %llx %u %u %u", &k, &p, &gpu, &wptr, &x, &y, &z) == 7) {
			ihs[k]->ring_size = p;
			ihs[k]->gpu_addr = gpu;
			ihs[k]->wptr_addr = wptr;
			ihs[k]->use_bus_addr = x;
			ihs[k]->use_doorbell = y;
			ihs[k]->doorbell_index = z;
		} else if (!strcmp(kw, "ring") && scanf(" %15s %u %u", name, &p, &q) == 3) {
			struct amdgpu_ring *r = &rings[adev->num_rings];
			strcpy(r->name, name);
			r->vm_hub = p;
			r->me = q;
			adev->rings[adev->num_rings++] = r;
		} else if (!strcmp(kw, "preset") && scanf(" %x %x", &p, &q) == 2) {
			file_set(direct, &ndirect, p, q);
		} else if (!strcmp(kw, "pcie") && scanf(" %x %x", &p, &q) == 2) {
			file_set(indirect, &nindirect, p, q);
		} else {
			return 3;
		}
	}
	seed_rule = rule[0];
	/* What the blocks' early and sw_init set up for these paths. */
	nbio_funcs = *adev->nbio.funcs;
	nbio_funcs.enable_doorbell_selfring_aperture = nbio_v7_0_enable_doorbell_selfring_aperture;
	nbio_funcs.ih_doorbell_range = nbio_v7_0_ih_doorbell_range;
	nbio_funcs.update_medium_grain_clock_gating = nbio_v7_0_update_medium_grain_clock_gating;
	nbio_funcs.update_medium_grain_light_sleep = nbio_v7_0_update_medium_grain_light_sleep;
	nbio_funcs.ih_control = nbio_v7_0_ih_control;
	adev->nbio.funcs = &nbio_funcs;
	adev->hdp.funcs = &oracle_hdp_v4_0_funcs;
	adev->gmc.vm_fault.funcs = &vm_fault_funcs;
	adev->init_lvl = (void *)&init_level;
	adev->irq.installed = true;
	adev->usec_timeout = 100000;
	oracle_ih_init_register_offset(adev);
	common.adev = ih.adev = adev;

	printf("--- soc ih hw_init\n");
	printf("vega10_ih_hw_init %d\n", oracle_ih_hw_init(&ih));
	printf("--- soc gmc late_init\n");
	printf("gmc_v9_0_late_init %d\n", gmc_v9_0_late_init(gmc_block));
	for (u32 i = 0; i < adev->num_rings; i++)
		printf("inv %s %u\n", adev->rings[i]->name, adev->rings[i]->vm_inv_eng);
	printf("--- soc gating\n");
	soc15_common_set_clockgating_state(&common, AMD_CG_STATE_GATE);
	gmc_v9_0_set_clockgating_state(gmc_block, AMD_CG_STATE_GATE);
	oracle_ih_set_clockgating_state(&ih, AMD_CG_STATE_GATE);
	oracle_ih_set_clockgating_state(&ih, AMD_CG_STATE_UNGATE);
	gmc_v9_0_set_clockgating_state(gmc_block, AMD_CG_STATE_UNGATE);
	soc15_common_set_clockgating_state(&common, AMD_CG_STATE_UNGATE);
	printf("--- soc hw_fini\n");
	printf("vega10_ih_hw_fini %d\n", oracle_ih_hw_fini(&ih));
	printf("gmc_v9_0_hw_fini %d\n", gmc_v9_0_hw_fini(gmc_block));
	printf("soc15_common_hw_fini %d\n", soc15_common_hw_fini(&common));
	return 0;
}

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
	if (argc > 3 && !strcmp(argv[2], "late"))
		return late(adev, &gmc_block, argv[3]);
	if (argc > 2) {
		/* hw mode: the inputs as gmc_hw_check.rs printed them. */
		static struct amdgpu_bo gart_bo;

		gart_bo.pd_addr = strtoull(argv[2], NULL, 0);
		gart_bo.gpu_offset = strtoull(argv[3], NULL, 0);
		a.gart.bo = &gart_bo;
		a.mem_scratch.gpu_addr = strtoull(argv[4], NULL, 0);
		a.dummy_page_addr = strtoull(argv[5], NULL, 0);
		a.hdp.funcs = &oracle_hdp_v4_0_funcs;
		a.mode_info.num_crtc = 4;
		a.usec_timeout = 100000;
		a.pg_flags = strtoul(argv[6], NULL, 0);
		a.gmc.noretry = strtoul(argv[7], NULL, 0);
		for (int h = 0; h < AMDGPU_MAX_VMHUBS; h++) {
			struct amdgpu_vmhub *v = &a.vmhub[h];

			if (!v->ctx0_ptb_addr_lo32)
				continue;
			WREG32(v->vm_inv_eng0_sem + v->eng_distance * 17, 1);
			WREG32(v->vm_inv_eng0_ack + v->eng_distance * 17, 0xffffffff);
		}
		printf("--- hw_init\n");
		printf("gmc_v9_0_hw_init %d\n", gmc_v9_0_hw_init(&gmc_block));
		return 0;
	}
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
