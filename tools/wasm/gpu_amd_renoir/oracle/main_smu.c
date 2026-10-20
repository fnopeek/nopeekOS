/* The oracle for stage 4's SMU block: Linux's smu_cmn.c, smu_v12_0.c and
 * renoir_ppt.c whole, and the block's life out of amdgpu_smu.c, copied
 * verbatim by extract.py. early_init and sw_init run silently, as the
 * port runs them before the comparison starts; then hw_init, the power
 * gating the GFX and SDMA blocks ask for, late_init and hw_fini run with
 * every register write printed.
 *
 * Registers are a file in which a register never written reads SEED(reg)
 * by the rule argv[1] names (ones, zeros or hash), and so does an address
 * behind the PCIe index/data pair. The SMU answers a message written to its
 * message register with SMU_RESP_OK, unprinted. The register bases, the
 * presets of the fake card, the buffers sw_init gets and the boot values
 * the port read from the VBIOS come on stdin from smu4_check.rs, which runs
 * the port on the same file and prints the same lines. */
#define SWSMU_CODE_LAYER_L1
#include "amdgpu.h"
#include "amdgpu_smu.h"
#include "smu_internal.h"
#include "renoir_ppt.h"
#include "amd_pcie.h"
#include "hdp/hdp_4_0_offset.h"

static char seed_rule;
#define SEED(reg) (seed_rule == 'o' ? 0xffffffffu : seed_rule == 'z' ? 0u : \
		   (((u32)(reg) * 0x9e3779b1u) ^ 0xa5a5a5a5u))
struct cell { u32 reg, v; };
static struct cell file[16384], pcie[1024];
static int nfile, npcie;
static bool echo;
static u32 pcie_index, pcie_data;
static struct smu_context *the_smu;

static u32 cell_get(struct cell *c, int n, u32 reg)
{
	for (int i = 0; i < n; i++)
		if (c[i].reg == reg)
			return c[i].v;
	return SEED(reg);
}

static void cell_set(struct cell *c, int *n, int cap, u32 reg, u32 v)
{
	int i;

	for (i = 0; i < *n; i++)
		if (c[i].reg == reg)
			break;
	if (i == *n) {
		if (*n == cap)
			abort();
		(*n)++;
	}
	c[i].reg = reg;
	c[i].v = v;
}

u32 oracle_rreg(u32 reg)
{
	return cell_get(file, nfile, reg);
}

void oracle_wreg(u32 reg, u32 v)
{
	if (echo)
		printf("write reg 0x%x = 0x%x\n", reg, v);
	cell_set(file, &nfile, 16384, reg, v);
	if (the_smu && reg == the_smu->msg_reg)
		cell_set(file, &nfile, 16384, the_smu->resp_reg, 0x1);
}

/* amdgpu_device_indirect_rreg: the address into the index register, the
 * value out of the data register. */
u32 oracle_rreg_pcie(u32 addr)
{
	if (echo)
		printf("write reg 0x%x = 0x%x\n", pcie_index, addr);
	cell_set(file, &nfile, 16384, pcie_index, addr);
	return cell_get(pcie, npcie, addr);
}

/* sw_init's buffers, as the port got them: size, alignment and domain
 * must be what Linux asks for. */
static struct { u64 size, gpu; u32 align, domain; } bos[8];
static int nbos, bo_at;

int amdgpu_bo_create_kernel(struct amdgpu_device *adev, unsigned long size, int align, u32 domain,
			    struct amdgpu_bo **bo_ptr, u64 *gpu_addr, void **cpu_addr)
{
	if (bo_at == nbos || bos[bo_at].size != size || bos[bo_at].align != align || bos[bo_at].domain != domain) {
		printf("bo %lu %d %u is not the port's\n", size, align, domain);
		return -ENOMEM;
	}
	*bo_ptr = (struct amdgpu_bo *)&bos[bo_at];
	*gpu_addr = bos[bo_at].gpu;
	*cpu_addr = calloc(1, size);
	bo_at++;
	return 0;
}
#define amdgpu_bo_free_kernel(bo, gpu, cpu) abort()

/* smu_v12_0_get_vbios_bootup_values parses the VBIOS through the atom
 * interpreter; the values are the port's, read from the same VBIOS. */
static struct smu_bios_boot_up_values boot_values;
static int oracle_boot_values(struct smu_context *smu)
{
	smu->smu_table.boot_values = boot_values;
	return 0;
}
#undef smu_get_vbios_bootup_values
#define smu_get_vbios_bootup_values(smu) oracle_boot_values(smu)

/* The VBIOS readers smu_v12_0.c links against; the boot values above stand
 * in for the one path that calls them. */
int amdgpu_atom_execute_table(struct atom_context *ctx, int index, uint32_t *params, int params_size)
{
	abort();
}
int amdgpu_atombios_get_data_table(struct amdgpu_device *adev, uint32_t table, uint16_t *size, uint8_t *frev,
				   uint8_t *crev, uint16_t *data_start)
{
	abort();
}
/* The names renoir_get_power_profile_mode prints, a sysfs read. */
const char * const amdgpu_pp_profile_name[PP_SMC_POWER_PROFILE_COUNT];

/* amdgpu_dpm's module parameters at their defaults. */
static int amdgpu_dpm = -1;
static int amdgpu_smu_pptable_id = -1;
static int amdgpu_wbrf = -1;
#define WREG32_NO_KIQ(r, v) WREG32(r, v)
#define RREG32_NO_KIQ(r) RREG32(r)
struct amdgpu_ring { const struct { void *emit_wreg; } *funcs; };
#define amdgpu_ring_emit_wreg(ring, reg, v) abort()

/* The other ASICs' ppt tables: smu_set_funcs takes renoir's for MP1 12.0.0. */
#define navi10_set_ppt_funcs(smu) abort()
#define sienna_cichlid_set_ppt_funcs(smu) abort()
#define arcturus_set_ppt_funcs(smu) abort()
#define vangogh_set_ppt_funcs(smu) abort()
#define aldebaran_set_ppt_funcs(smu) abort()
#define yellow_carp_set_ppt_funcs(smu) abort()
#define cyan_skillfish_set_ppt_funcs(smu) abort()
#define smu_v13_0_0_set_ppt_funcs(smu) abort()
#define smu_v13_0_4_set_ppt_funcs(smu) abort()
#define smu_v13_0_5_set_ppt_funcs(smu) abort()
#define smu_v13_0_6_set_ppt_funcs(smu) abort()
#define smu_v13_0_7_set_ppt_funcs(smu) abort()
#define smu_v14_0_0_set_ppt_funcs(smu) abort()
#define smu_v14_0_2_set_ppt_funcs(smu) abort()

#include "smu_protos.h"
static const struct amd_pm_funcs swsmu_pm_funcs = {
	.set_powergating_by_smu = smu_dpm_set_power_gate,
};
#include "smu_extracted.c"

static const struct amdgpu_hdp_funcs hdp_funcs = { .invalidate_hdp = hdp_v4_0_invalidate_hdp };
static const struct amdgpu_asic_funcs asic_funcs;
static u32 bases[MAX_HWIP][8];

static void dump(struct amdgpu_device *adev, struct smu_context *smu)
{
	/* Not smu_feature.allowed: renoir has no get_allowed_feature_mask, and
	 * smu_get_driver_allowed_feature_mask ORs in a stack array nothing
	 * wrote. */
	printf("smu fw %x if %x drv_if %x pm_fw %x dpm %d feature %lx workload %x\n",
	       smu->smc_fw_version, smu->smc_fw_if_version, smu->smc_driver_if_version, adev->pm.fw_version,
	       adev->pm.dpm_enabled, smu->smu_feature.supported[0], smu->workload_mask);
	printf("gfx freq %u %u %u %u\n", smu->gfx_default_hard_min_freq, smu->gfx_default_soft_max_freq,
	       smu->gfx_actual_hard_min_freq, smu->gfx_actual_soft_max_freq);
	printf("gated vcn %d jpeg %d pwr_state", atomic_read(&smu->smu_power.power_gate.vcn_gated[0]),
	       atomic_read(&smu->smu_power.power_gate.jpeg_gated));
	for (int i = 0; i < AMD_IP_BLOCK_TYPE_NUM; i++)
		printf(" %d", atomic_read(&adev->pm.pwr_state[i]));
	printf("\n");
}

int main(int argc, char **argv)
{
	static struct amdgpu_device a;
	struct amdgpu_device *adev = &a;
	static struct amdgpu_ip_block blk;
	static struct pci_dev pdev;
	seed_rule = argv[1][0];
	blk.adev = adev;
	a.pdev = &pdev;
	a.hdp.funcs = &hdp_funcs;
	a.asic_funcs = &asic_funcs;

	u32 nb;
	if (scanf(" inputs bases %u", &nb) != 1)
		return 1;
	for (u32 k = 0; k < nb; k++) {
		u32 hw, seg, v;
		if (scanf(" %u %u %x", &hw, &seg, &v) != 3)
			return 1;
		bases[hw][seg] = v;
		a.reg_offset[hw][0] = bases[hw];
	}
	unsigned long long cg;
	if (scanf(" versions %x %x %x flags %x apu %x cg %llx pg %x pp %x vcn %d gen %x mlw %x usec %u pool %u"
		  " pcie %x %x",
		  &a.ip_versions[MP1_HWIP][0], &a.ip_versions[HDP_HWIP][0], &a.ip_versions[GC_HWIP][0], &a.flags,
		  &a.apu_flags, &cg, &a.pg_flags, &a.pm.pp_feature, &a.vcn.num_vcn_inst, &a.pm.pcie_gen_mask,
		  &a.pm.pcie_mlw_mask, &a.usec_timeout, &a.pm.smu_prv_buffer_size, &pcie_index, &pcie_data) != 15)
		return 2;
	a.cg_flags = cg;
	struct smu_bios_boot_up_values *b = &boot_values;
	u32 vddc, vddci, mvddc, vdd_gfx, cooling;
	if (scanf(" boot %u %u %u %u %u %u %u %u %u %u %u %u %u %u %u %u %u %u %u", &b->revision, &b->gfxclk, &b->uclk,
		  &b->socclk, &b->dcefclk, &b->eclk, &b->vclk, &b->dclk, &vddc, &vddci, &mvddc, &vdd_gfx, &cooling,
		  &b->pp_table_id, &b->format_revision, &b->content_revision, &b->fclk, &b->lclk,
		  &b->firmware_caps) != 19)
		return 3;
	b->vddc = vddc;
	b->vddci = vddci;
	b->mvddc = mvddc;
	b->vdd_gfx = vdd_gfx;
	b->cooling_id = cooling;
	char kw[16];
	while (scanf(" %15s", kw) == 1 && strcmp(kw, "end")) {
		unsigned long long x, y;
		u32 p, q;
		static struct amdgpu_ip_block_version versions[AMD_IP_BLOCK_TYPE_NUM];
		if (!strcmp(kw, "block") && scanf(" %u %u", &p, &q) == 2) {
			versions[a.num_ip_blocks].type = p;
			a.ip_blocks[a.num_ip_blocks].adev = adev;
			a.ip_blocks[a.num_ip_blocks].version = &versions[a.num_ip_blocks];
			a.ip_blocks[a.num_ip_blocks++].status.valid = q;
		} else if (!strcmp(kw, "bo") && scanf(" %llu %u %u %llx", &x, &p, &q, &y) == 4)
			bos[nbos++] = (typeof(bos[0])){ x, y, p, q };
		else if (!strcmp(kw, "preset") && scanf(" %x %x", &p, &q) == 2)
			cell_set(file, &nfile, 16384, p, q);
		else if (!strcmp(kw, "pcie") && scanf(" %x %x", &p, &q) == 2)
			cell_set(pcie, &npcie, 1024, p, q);
		else
			return 4;
	}

	if (smu_early_init(&blk) || smu_sw_init(&blk))
		return 5;
	struct smu_context *smu = a.powerplay.pp_handle;
	the_smu = smu;
	if (bo_at != nbos)
		printf("the port got %d buffers, Linux %d\n", nbos, bo_at);

	echo = true;
	printf("--- smu hw_init\n");
	printf("smu_hw_init %d\n", smu_hw_init(&blk));
	dump(adev, smu);
	printf("--- smu powergating\n");
	printf("sdma ungate %d\n", amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_SDMA, false, 0));
	printf("gfx gate %d\n", amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_GFX, true, 0));
	printf("gfx ungate %d\n", amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_GFX, false, 0));
	printf("--- smu late_init\n");
	printf("smu_late_init %d\n", smu_late_init(&blk));
	dump(adev, smu);
	printf("--- smu hw_fini\n");
	printf("sdma gate %d\n", amdgpu_dpm_set_powergating_by_smu(adev, AMD_IP_BLOCK_TYPE_SDMA, true, 0));
	printf("smu_hw_fini %d\n", smu_hw_fini(&blk));
	dump(adev, smu);
	return 0;
}
