/* The DC clock manager oracle: Linux's bios_parser2_construct on the VBIOS
 * (argv[1]), then rn_clk_mgr_construct against a register file whose
 * unwritten registers read hash(reg), with the VBIOS SMC mailbox answering
 * GetSmuVersion with argv[2] and IsPeriodicRetrainingDisabled with argv[3].
 * argv[4] "dpm" lets pp_smu hand out a clock table, "nodpm" makes it
 * unsupported. Every register write is printed, then the clock manager.
 * hostcheck/clk_check.rs prints the same for the port. */
#include <linux/sched.h>
#define container_of(ptr, type, member) ((type *)((char *)(ptr) - offsetof(type, member)))
#define DC_LOG_BIOS(...) do { } while (0)
#define fallthrough __attribute__((fallthrough))
#define DC_LOG_SMU(...) do { } while (0)
#define ASSERT(x) do { } while (0)
#define ASSERT_CRITICAL(x) do { } while (0)
#define BREAK_TO_DEBUGGER() do { } while (0)
#define dm_output_to_console(...) do { } while (0)
#define min_t(t, a, b) ((t)(a) < (t)(b) ? (t)(a) : (t)(b))
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define swap(a, b) do { typeof(a) __t = (a); (a) = (b); (b) = __t; } while (0)
#define DC_FP_START() do { } while (0)
#define DC_FP_END() do { } while (0)
#define dc_assert_fp_enabled() do { } while (0)
#define msleep(ms) do { } while (0)
#define udelay(us) do { } while (0)
#define IS_SMU_TIMEOUT(result) (result == 0x0)
#define dm_helpers_smu_timeout(ctx, msg, param, us) do { } while (0)
#define snprintf_count(...) 0
#define GREEN_SARDINE_A0 0xA1
#define ASICREV_IS_GREEN_SARDINE(r) ((r >= GREEN_SARDINE_A0) && (r < 0xFF))
#define WM_A 0
#define WM_B 1
#define WM_C 2
#define WM_D 3
#define WM_SET_COUNT 4
#define smu_print(...) do { } while (0)

#include <linux/slab.h>
#include "atomfirmware.h"
#include "atom.h"
#include "ObjectID.h"
#include "renoir_ip_offset.h"
#include "clk/clk_10_0_2_offset.h"
#include "clk/clk_10_0_2_sh_mask.h"
#include "mp/mp_12_0_0_offset.h"
#include "mp/mp_12_0_0_sh_mask.h"
#include "fixed31_32.h"
#include "dm_pp_smu.h"
#include "bios_types.h"
#include "clk_types.h"

/* The register file. */
#define NREGS 4096
static u32 reg_addr[NREGS], reg_val[NREGS];
static int nregs;
static u32 smu_ver_answer, retrain_answer;

static int find(u32 reg)
{
	for (int i = 0; i < nregs; i++)
		if (reg_addr[i] == reg)
			return i;
	return -1;
}

static void set(u32 reg, u32 val)
{
	int i = find(reg);
	if (i < 0) {
		i = nregs++;
		reg_addr[i] = reg;
	}
	reg_val[i] = val;
}

static u32 rd(u32 reg)
{
	int i = find(reg);
	return i < 0 ? (reg * 0x9e3779b1u) ^ 0xa5a5a5a5u : reg_val[i];
}

#define MP1_REG(r) (MP0_BASE.instance[0].segment[mm##r##_BASE_IDX] + mm##r)

/* The VBIOS SMC: a message on C2PMSG_67 answers OK on C2PMSG_91 and its
 * value on C2PMSG_83. */
static void wr(u32 reg, u32 val)
{
	printf("write reg 0x%x = 0x%x\n", reg, val);
	set(reg, val);
	if (reg == MP1_REG(MP1_SMN_C2PMSG_67)) {
		if (val == 0x2)
			set(MP1_REG(MP1_SMN_C2PMSG_83), smu_ver_answer);
		else if (val == 0xF)
			set(MP1_REG(MP1_SMN_C2PMSG_83), retrain_answer);
		set(MP1_REG(MP1_SMN_C2PMSG_91), 1);
	}
}

#define REG_READ(reg) rd(REG(reg))
#define REG_WRITE(reg, v) wr(REG(reg), (v))
#define REG_GET(reg, f, v) (*(v) = (rd(REG(reg)) & reg##__##f##_MASK) >> reg##__##f##__SHIFT)

struct amdgpu_device {
	struct amdgpu_mode_info { struct atom_context *atom_context; } mode_info;
};
struct dc;
struct dc_context {
	struct dc *dc;
	void *driver_context;
	enum dce_version dce_version;
	struct dc_bios *dc_bios;
	struct { uint32_t hw_internal_rev; } asic_id;
	void *logger;
};
struct dc_debug_options {
	int min_disp_clk_khz;
	bool disable_48mhz_pwrdwn;
};
struct dc {
	struct dc_context *ctx;
	struct {
		unsigned int force_bios_fixed_vs;
		bool is_single_rank_dimm;
		bool is_asymmetric_memory;
		bool ignore_dpref_ss;
	} config;
	struct dc_debug_options debug;
};
struct bios_parser;
struct dc_bios;
struct dc_vbios_funcs {
	uint32_t (*get_ss_entry_number)(struct dc_bios *bios, enum as_signal_type signal);
	enum bp_result (*get_spread_spectrum_info)(struct dc_bios *bios, enum as_signal_type signal,
						   uint32_t index, struct spread_spectrum_info *ss_info);
};
struct dc_bios {
	const struct dc_vbios_funcs *funcs;
	uint8_t *bios;
	uint32_t bios_size;
	uint8_t *bios_local_image;
	struct dc_context *ctx;
	struct integrated_info *integrated_info;
	struct dc_firmware_info fw_info;
	bool fw_info_valid;
	struct dc_vram_info vram_info;
	struct bp_soc_bb_info bb_info;
};
struct cmd_tbl {
	void *dig_encoder_control, *transmitter_control, *set_pixel_clock, *set_crtc_timing,
	     *enable_crtc, *external_encoder_control, *enable_disp_power_gating, *set_dce_clock,
	     *enable_lvtma_control;
	unsigned int (*get_smu_clock_info)(struct bios_parser *bp, uint8_t id);
};
struct command_table_helper { int unused; };
struct bios_parser {
	struct dc_bios base;
	struct object_info_table object_info_tbl;
	uint32_t object_info_tbl_offset;
	struct atom_master_data_table_v2_1 *master_data_tbl;
	const struct command_table_helper *cmd_helper;
	struct cmd_tbl cmd_tbl;
	bool remap_device_tags;
};
static const struct dc_vbios_funcs vbios_funcs;

#define EXEC(n) static char n[1];
EXEC(encoder_control_digx_v1_5) EXEC(encoder_control_fallback) EXEC(transmitter_control_v1_6)
EXEC(transmitter_control_v1_7) EXEC(transmitter_control_fallback) EXEC(set_pixel_clock_v7)
EXEC(set_pixel_clock_fallback) EXEC(set_crtc_using_dtd_timing_v3) EXEC(enable_crtc_v1)
EXEC(external_encoder_control_v3) EXEC(enable_disp_power_gating_v2_1)
EXEC(enable_disp_power_gating_fallback) EXEC(set_dce_clock_v2_1) EXEC(enable_lvtma_control)
static unsigned int get_smu_clock_info_v3_1(struct bios_parser *bp, uint8_t id)
{
	printf("get_smu_clock_info_v3_1 %u\n", id);
	return 0;
}
static const struct command_table_helper helper_dce60, helper_dce80, helper_dce110, helper_dce112_2;
static const struct command_table_helper *dal_cmd_tbl_helper_dce60_get_table(void) { return &helper_dce60; }
static const struct command_table_helper *dal_cmd_tbl_helper_dce80_get_table(void) { return &helper_dce80; }
static const struct command_table_helper *dal_cmd_tbl_helper_dce110_get_table(void) { return &helper_dce110; }
static const struct command_table_helper *dal_cmd_tbl_helper_dce112_get_table2(void) { return &helper_dce112_2; }

struct clk_mgr_funcs { int unused; };
struct clk_mgr {
	struct dc_context *ctx;
	struct clk_mgr_funcs *funcs;
	bool force_smu_not_present;
	int dprefclk_khz;
	int dentist_vco_freq_khz;
	struct clk_state_registers_and_bypass boot_snapshot;
	struct clk_bw_params *bw_params;
};
struct dccg { int unused; };
struct clk_mgr_internal {
	struct clk_mgr base;
	int smu_ver;
	struct pp_smu_funcs *pp_smu;
	struct dccg *dccg;
	uint32_t dfs_ref_freq_khz;
	int dfs_bypass_disp_clk;
	bool ss_on_dprefclk;
	int dprefclk_ss_percentage;
	int dprefclk_ss_divider;
	bool periodic_retraining_disabled;
};
#define TO_CLK_MGR_INTERNAL(clk_mgr) container_of(clk_mgr, struct clk_mgr_internal, base)
#define CTX clk_mgr->base.ctx
static struct clk_mgr_funcs dcn21_funcs;

#include "bios_protos.h"
#include "bios_funcs.c"
static const struct dc_vbios_funcs vbios_funcs = {
	.get_ss_entry_number = bios_parser_get_ss_entry_number,
	.get_spread_spectrum_info = bios_parser_get_spread_spectrum_info,
};
#include "clk_protos.h"
#include "clk_funcs.c"

static int dpm_supported;

/* The clock table the SMU would hand out: FCLK highest first, every level
 * on a voltage the DCFCLK and SOCCLK tables carry. */
static enum pp_smu_status get_dpm_clock_table(struct pp_smu *pp, struct dpm_clocks *t)
{
	if (!dpm_supported)
		return PP_SMU_RESULT_UNSUPPORTED;
	for (int i = 0; i < 8; i++) {
		t->DcfClocks[i].Freq = 400 + 100 * i;
		t->DcfClocks[i].Vol = 3000 + 50 * i;
		t->SocClocks[i].Freq = 200 + 60 * i;
		t->SocClocks[i].Vol = 3000 + 50 * i;
		t->VClocks[i].Freq = 300 + 10 * i;
		t->VClocks[i].Vol = 3000 + 50 * i;
		t->DClocks[i].Freq = 250 + 10 * i;
		t->DClocks[i].Vol = 3000 + 50 * i;
	}
	for (int i = 0; i < 4; i++) {
		t->FClocks[i].Freq = 1600 - 400 * i;
		t->FClocks[i].Vol = 3000 + 50 * (3 - i);
		t->MemClocks[i].Freq = 2133 - 400 * i;
		t->MemClocks[i].Vol = 3000 + 50 * (3 - i);
	}
	return PP_SMU_RESULT_OK;
}

static u32 zero_read(struct card_info *info, u32 reg) { return 0; }
static void no_write(struct card_info *info, u32 reg, u32 val) {}

int main(int argc, char **argv)
{
	FILE *f = fopen(argv[1], "rb");
	static unsigned char bios[1 << 20];
	fread(bios, 1, sizeof(bios), f);
	fclose(f);
	smu_ver_answer = strtoul(argv[2], NULL, 0);
	retrain_answer = strtoul(argv[3], NULL, 0);
	dpm_supported = !strcmp(argv[4], "dpm");
	static struct card_info card = { .reg_read = zero_read, .reg_write = no_write, .mc_read = zero_read,
		.mc_write = no_write, .pll_read = zero_read, .pll_write = no_write };
	static struct amdgpu_device adev;
	adev.mode_info.atom_context = amdgpu_atom_parse(&card, bios);

	static struct dc dc = { .debug = { .min_disp_clk_khz = 100000, .disable_48mhz_pwrdwn = false } };
	static struct dc_context ctx = { .dc = &dc, .driver_context = &adev, .dce_version = DCN_VERSION_2_1 };
	dc.ctx = &ctx;
	struct bp_init_data init = { .ctx = &ctx, .bios = bios };
	struct bios_parser *bp = calloc(1, sizeof(*bp));
	if (!bios_parser2_construct(bp, &init, DCN_VERSION_2_1))
		return 1;
	ctx.dc_bios = &bp->base;

	static struct pp_smu_funcs pp_smu;
	pp_smu.ctx.ver = PP_SMU_VER_RN;
	pp_smu.rn_funcs.get_dpm_clock_table = get_dpm_clock_table;
	static struct dccg dccg;
	struct clk_mgr_internal *clk_mgr = calloc(1, sizeof(*clk_mgr));
	rn_clk_mgr_construct(&ctx, clk_mgr, &pp_smu, &dccg);

	printf("smu_ver 0x%x retraining_disabled %d min_disp_clk_khz %d\n", clk_mgr->smu_ver,
	       clk_mgr->periodic_retraining_disabled, dc.debug.min_disp_clk_khz);
	printf("dfs_ref_freq_khz %u dfs_bypass_disp_clk %d dentist_vco_freq_khz %d dprefclk_khz %d\n",
	       clk_mgr->dfs_ref_freq_khz, clk_mgr->dfs_bypass_disp_clk, clk_mgr->base.dentist_vco_freq_khz,
	       clk_mgr->base.dprefclk_khz);
	printf("ss_on_dprefclk %d dprefclk_ss_percentage %d dprefclk_ss_divider %d\n", clk_mgr->ss_on_dprefclk,
	       clk_mgr->dprefclk_ss_percentage, clk_mgr->dprefclk_ss_divider);
	struct clk_state_registers_and_bypass *s = &clk_mgr->base.boot_snapshot;
	printf("boot dcfclk %u ds %u %u dprefclk %u dispclk %u dppclk %u bypass %u %u %u %u\n", s->dcfclk,
	       s->dcf_deep_sleep_divider, s->dcf_deep_sleep_allow, s->dprefclk, s->dispclk, s->dppclk,
	       s->dppclk_bypass, s->dcfclk_bypass, s->dprefclk_bypass, s->dispclk_bypass);
	struct clk_bw_params *b = clk_mgr->base.bw_params;
	printf("bw vram_type %u num_channels %u num_entries %u\n", b->vram_type, b->num_channels,
	       b->clk_table.num_entries);
	for (int i = 0; i < MAX_NUM_DPM_LVL; i++) {
		struct clk_limit_table_entry *e = &b->clk_table.entries[i];
		printf("clk %d voltage %u dcfclk %u fclk %u memclk %u socclk %u\n", i, e->voltage, e->dcfclk_mhz,
		       e->fclk_mhz, e->memclk_mhz, e->socclk_mhz);
	}
	for (int i = 0; i < WM_SET_COUNT; i++) {
		struct wm_range_table_entry *w = &b->wm_table.entries[i];
		printf("wm %d inst %u type %u pstate %.6f sr_exit %.6f sr_enter_exit %.6f valid %d\n", i, w->wm_inst,
		       w->wm_type, w->pstate_latency_us, w->sr_exit_time_us, w->sr_enter_plus_exit_time_us, w->valid);
	}
	return 0;
}
