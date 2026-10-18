/* The interpreter oracle: Linux's atom.c runs the command tables
 * smu_v12_0_get_vbios_bootup_values runs, on the device's VBIOS, against a
 * register file whose unwritten registers follow argv[2] (ones, zeros or
 * hash). Every register access is printed, then the clock each call returns.
 * With argv[3], command table argv[3] runs alone instead, on 256 zero
 * parameters, to cover the opcodes the clock tables do not use.
 * hostcheck/atomexec_check.rs prints the same for the port. */
#include <linux/sched.h>
#include <linux/slab.h>
#include "atomfirmware.h"
#include "atom.h"
#include <drm/amd_asic_type.h>
#define get_index_into_master_table(master_table, table_name) (offsetof(struct master_table, table_name) / sizeof(uint16_t))
#define amdgpu_sriov_vf(adev) 0
#define BUG() abort()
struct amdgpu_pll {
	u32 reference_freq, reference_div, min_post_div, max_post_div, min_ref_div, max_ref_div;
	u32 min_feedback_div, max_feedback_div, best_vco;
};
struct amdgpu_device {
	enum amd_asic_type asic_type;
	u32 flags;
	struct amdgpu_mode_info { struct atom_context *atom_context; u32 firmware_flags; } mode_info;
	u32 bios_scratch_reg_offset;
	struct { u64 fw_vram_usage_start_offset, fw_vram_usage_size, drv_vram_usage_start_offset, drv_vram_usage_size; } mman;
	struct { u32 default_sclk, default_mclk; struct amdgpu_pll spll, mpll; } clock;
	struct { u32 current_sclk, current_mclk; } pm;
};
#include "amd_shared.h"
#include "vram_types.h"
#define RREG32(r) 0u
#define amdgpu_ip_version(adev, ip, inst) 0u
#include "fw_extracted.c"

#define NREGS 8192
#define MAX_READS 200000
static u32 reg_addr[NREGS], reg_val[NREGS];
static int nregs, nreads;
static int rule;

static u32 unset(u32 reg)
{
	if (rule == 0)
		return 0xffffffffu;
	if (rule == 1)
		return 0;
	return reg * 0x9e3779b1u ^ (reg >> 7) * 0x85ebca6bu;
}

static int find(u32 reg)
{
	for (int i = 0; i < nregs; i++)
		if (reg_addr[i] == reg)
			return i;
	return -1;
}

static u32 rd(struct card_info *info, u32 reg)
{
	int i = find(reg);
	u32 v = i < 0 ? unset(reg) : reg_val[i];
	printf("R %05x %08x\n", reg, v);
	if (++nreads >= MAX_READS) {
		printf("read cap\n");
		exit(0);
	}
	return v;
}

static void wr(struct card_info *info, u32 reg, u32 val)
{
	int i = find(reg);
	printf("W %05x %08x\n", reg, val);
	if (i < 0) {
		if (nregs == NREGS)
			abort();
		i = nregs++;
		reg_addr[i] = reg;
	}
	reg_val[i] = val;
}

static u32 zero_read(struct card_info *info, u32 reg) { return 0; }
static void no_write(struct card_info *info, u32 reg, u32 val) {}

static void clockinfo(struct atom_context *ctx, const char *n, u8 clk_id, u8 syspll_id)
{
	struct atom_get_smu_clock_info_parameters_v3_1 input = {0};
	struct atom_get_smu_clock_info_output_parameters_v3_1 *output;
	static u32 ps[sizeof(input)];
	int index = get_index_into_master_table(atom_master_list_of_command_functions_v2_1, getsmuclockinfo);

	input.clk_id = clk_id;
	input.syspll_id = syspll_id;
	input.command = GET_SMU_CLOCK_INFO_V3_1_GET_CLOCK_FREQ;
	/* The tables may read PS up to params_size; the port zeroes past the
	 * struct, so the oracle does too. */
	memset(ps, 0, sizeof(ps));
	memcpy(ps, &input, sizeof(input));
	printf("--- %s\n", n);
	int ret = amdgpu_atom_execute_table(ctx, index, ps, sizeof(input));
	output = (struct atom_get_smu_clock_info_output_parameters_v3_1 *)ps;
	printf("%s ret %d freq %u divmul %08x %08x\n", n, ret,
	       ret ? 0 : le32_to_cpu(output->atom_smu_outputclkfreq.smu_clock_freq_hz) / 10000,
	       ctx->divmul[0], ctx->divmul[1]);
}

int main(int argc, char **argv)
{
	FILE *f = fopen(argv[1], "rb");
	static unsigned char bios[1 << 20];
	fread(bios, 1, sizeof(bios), f);
	fclose(f);
	rule = !strcmp(argv[2], "ones") ? 0 : !strcmp(argv[2], "zeros") ? 1 : 2;
	static struct card_info card = { .reg_read = rd, .reg_write = wr, .mc_read = zero_read,
		.mc_write = no_write, .pll_read = zero_read, .pll_write = no_write };
	static struct amdgpu_device adev = { .asic_type = CHIP_RENOIR, .flags = CHIP_RENOIR | AMD_IS_APU };
	struct atom_context *ctx = amdgpu_atom_parse(&card, bios);
	adev.mode_info.atom_context = ctx;
	mutex_init(&ctx->mutex);
	amdgpu_atomfirmware_scratch_regs_init(&adev);
	amdgpu_atomfirmware_allocate_fb_scratch(&adev);

	if (argc > 3) {
		static u32 ps[256]; /* params_size bounds PS in bytes */
		int r = amdgpu_atom_execute_table(ctx, atoi(argv[3]), ps, 256);
		printf("table %s ret %d divmul %08x %08x cs %d %d\n", argv[3], r, ctx->divmul[0], ctx->divmul[1],
		       ctx->cs_equal, ctx->cs_above);
		for (int i = 0; i < 256; i++)
			if (ps[i])
				printf("ps[%d] %08x\n", i, ps[i]);
		return 0;
	}
	clockinfo(ctx, "socclk", SMU12_SYSPLL0_SOCCLK_ID, SMU12_SYSPLL0_ID);
	clockinfo(ctx, "dcefclk", SMU12_SYSPLL1_DCFCLK_ID, SMU12_SYSPLL1_ID);
	clockinfo(ctx, "vclk", SMU12_SYSPLL0_VCLK_ID, SMU12_SYSPLL0_ID);
	clockinfo(ctx, "dclk", SMU12_SYSPLL0_DCLK_ID, SMU12_SYSPLL0_ID);
	clockinfo(ctx, "fclk", SMU12_SYSPLL3_0_FCLK_ID, SMU12_SYSPLL3_0_ID);
	return 0;
}
