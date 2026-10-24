/* The DC bring-up oracle: Linux's amdgpu_dm_init up to dc_hardware_init on
 * the VBIOS (argv[1]), with all of display/dc compiled unchanged, then a
 * DMUB service attached and dc_hardware_init: the eDP presence test,
 * dcn10_init_hw, the D0 notification. Unwritten registers read
 * argv[2] = hash / ones / zeros / lit (all ones, but no pipe fused off:
 * every block reads as running) / boot (lit, as the VBIOS leaves it: not in
 * accelerated mode, golden-init clock divider, each OTG scanning 1920x1080
 * out of its own OPP with the pattern generator off, the DMCUB booted
 * without an optimized init); the VBIOS SMC answers
 * every message. The boot registers are taken from Linux's own register
 * tables of the built DC objects and printed as `preset` lines, which the
 * port reads back.
 * After it, as amdgpu_dm_init goes on: mmhub_read_system_context on a
 * Renoir-like GMC layout and dc_setup_system_context.
 * argv[3] = detect then runs amdgpu_dm_initialize_drm_device's link loop
 * (dc_link_detect_connection_type, dc_link_detect at boot) against the AUX
 * engines of dp_sink.c, an eDP panel on the first link, every other HPD
 * low; EDID and DPCD go through the kernel half (kdrm/), DRM's and DM's own
 * code, back into DC.
 * Every register write is printed; the commands DC hands the DMCUB are
 * recorded where it hands them to the DM, then printed with the state
 * init_hw leaves. hostcheck/dchw_check.rs prints the same for the port. */
#include <errno.h>
#include "dc.h"
#include "core_types.h"
#include "dc_dmub_srv.h"
#include "dc_state.h"
#include "dc_stream.h"
#include "dmub/dmub_srv.h"
#include "dmub/src/dmub_dcn20.h"
#include "dm_pp_smu.h"
#include "clk_mgr.h"
#include "dchubbub.h"
#include "atom.h"
#include "amdgpu.h"
#include "renoir_ip_offset.h"
#include "mp/mp_12_0_0_offset.h"
#include "dcn/dcn_2_1_0_offset.h"
#include "dcn/dcn_2_1_0_sh_mask.h"
#include "dcn10/dcn10_optc.h"
#include "dcn20/dcn20_opp.h"
#include "hw_sequencer_private.h"
#include "hwss/dce/dce_hwseq.h"
#include "dce/dce_aux.h"
#include "gpio/hw_gpio.h"
#include "gpio/hw_hpd.h"
#include <drm/display/drm_dp.h>

#define mutex_init(m) ((void)(m))
#define drm_dbg_driver(d, ...) do { } while (0)
#define dev_err(d, ...) do { } while (0)
#define amdgpu_ip_version(adev, ip, inst) ((adev)->ip_versions[ip][inst] & ~0xFFU)

struct dmi_strmatch { unsigned char slot; char substr[79]; };
struct dmi_system_id {
	int (*callback)(const struct dmi_system_id *);
	const char *ident;
	struct dmi_strmatch matches[4];
	void *driver_data;
};
enum { DMI_SYS_VENDOR = 4, DMI_PRODUCT_NAME = 5 };
#define DMI_MATCH(a, b) { .slot = a, .substr = b }
static int dmi_check_system(const struct dmi_system_id *list) { return 0; }

uint amdgpu_dc_debug_mask;
uint amdgpu_dc_visual_confirm;

/* The register file. */
#define NREGS 16384
static u32 reg_addr[NREGS], reg_val[NREGS];
static int nregs;

static int find(u32 reg)
{
	for (int i = 0; i < nregs; i++)
		if (reg_addr[i] == reg)
			return i;
	return -1;
}

static u32 hash_rule(u32 reg) { return (reg * 0x9e3779b1u) ^ 0xa5a5a5a5u; }
static u32 ones_rule(u32 reg) { return 0xffffffffu; }
static u32 zeros_rule(u32 reg) { return 0; }
static u32 lit_rule(u32 reg)
{
	return reg == DCN_BASE.instance[0].segment[mmCC_DC_PIPE_DIS_BASE_IDX] + mmCC_DC_PIPE_DIS ? 0 : 0xffffffffu;
}
static u32 (*unset_rule)(u32) = hash_rule;

static void set(u32 reg, u32 val)
{
	int i = find(reg);
	if (i < 0) {
		i = nregs++;
		reg_addr[i] = reg;
	}
	reg_val[i] = val;
}

#include "dp_sink.c"

static u32 rd(u32 reg)
{
	u32 v;

	if (aux_engine_read(reg, &v))
		return v;
	int i = find(reg);
	return i < 0 ? unset_rule(reg) : reg_val[i];
}

#define MP1_REG(r) (MP0_BASE.instance[0].segment[mm##r##_BASE_IDX] + mm##r)

/* The VBIOS SMC: a message on C2PMSG_67 answers OK on C2PMSG_91, with the
 * SMU version on C2PMSG_83. */
static void wr(u32 reg, u32 val)
{
	printf("write reg 0x%x = 0x%x\n", reg, val);
	if (aux_engine_write(reg, val))
		return;
	set(reg, val);
	if (reg == MP1_REG(MP1_SMN_C2PMSG_67)) {
		set(MP1_REG(MP1_SMN_C2PMSG_83), 0x373700);
		set(MP1_REG(MP1_SMN_C2PMSG_91), 1);
	}
}

uint32_t dm_read_reg_func(const struct dc_context *ctx, uint32_t address, const char *func_name)
{
	return rd(address);
}

void dm_write_reg_func(const struct dc_context *ctx, uint32_t address, uint32_t value, const char *func_name)
{
	wr(address, value);
}

static uint32_t dmub_reg_read(void *ctx, uint32_t address) { return rd(address); }
static void dmub_reg_write(void *ctx, uint32_t address, uint32_t value) { wr(address, value); }

void dc_assert_fp_enabled(void) { }
void dm_perf_trace_timestamp(const char *func_name, unsigned int line, struct dc_context *ctx) { }
void dm_trace_smu_enter(uint32_t msg_id, uint32_t param_in, unsigned int delay, struct dc_context *ctx) { }
void dm_trace_smu_exit(bool success, uint32_t response, struct dc_context *ctx) { }

unsigned long long dm_get_elapse_time_in_ns(struct dc_context *ctx, unsigned long long current_time_stamp,
					    unsigned long long last_time_stamp)
{
	return current_time_stamp - last_time_stamp;
}

/* Where DC hands a command to the DMCUB: the oracle records it. */
static uint8_t dmub_cmds[256][DMUB_RB_CMD_SIZE];
static int n_dmub_cmds;

bool dm_execute_dmub_cmd_list(const struct dc_context *ctx, unsigned int count, union dmub_rb_cmd *cmd,
			      enum dm_dmub_wait_type wait_type)
{
	for (unsigned int i = 0; i < count; i++)
		memcpy(dmub_cmds[n_dmub_cmds++], &cmd[i], DMUB_RB_CMD_SIZE);
	return true;
}

bool dm_execute_dmub_cmd(const struct dc_context *ctx, union dmub_rb_cmd *cmd, enum dm_dmub_wait_type wait_type)
{
	return dm_execute_dmub_cmd_list(ctx, 1, cmd, wait_type);
}

/* The SMU behind pp_smu: power play is not up, so there is no clock
 * table. The watermark ranges DC hands it stay in clk_mgr->ranges. */
static enum pp_smu_status pp_rn_get_dpm_clock_table(struct pp_smu *pp, struct dpm_clocks *clock_table)
{
	return PP_SMU_RESULT_UNSUPPORTED;
}

static enum pp_smu_status pp_rn_set_wm_ranges(struct pp_smu *pp, struct pp_smu_wm_range_sets *ranges)
{
	return PP_SMU_RESULT_OK;
}

static void print_range(const char *n, unsigned int i, const struct pp_smu_wm_set_range *r)
{
	printf("  %s %u inst %u type %u fill %u %u drain %u %u\n", n, i, r->wm_inst, r->wm_type, r->min_fill_clk_mhz,
	       r->max_fill_clk_mhz, r->min_drain_clk_mhz, r->max_drain_clk_mhz);
}

void dm_pp_get_funcs(struct dc_context *ctx, struct pp_smu_funcs *funcs)
{
	if (ctx->dce_version != DCN_VERSION_2_1)
		abort();
	funcs->ctx.ver = PP_SMU_VER_RN;
	funcs->rn_funcs.pp_smu.dm = ctx;
	funcs->rn_funcs.set_wm_ranges = pp_rn_set_wm_ranges;
	funcs->rn_funcs.get_dpm_clock_table = pp_rn_get_dpm_clock_table;
}

static int amdgpu_dm_irq_init(struct amdgpu_device *adev) { return 0; }
static int dm_dmub_hw_init(struct amdgpu_device *adev) { return 0; }
static void amdgpu_dm_fini(struct amdgpu_device *adev) { }

#include "dminit_funcs.c"

/* The GMC layout of the system context: 512 MB of VRAM, the GART table in
 * its last MB, 1 GB of GART, AGP above the VRAM. */
#define VRAM_START 0xf400000000ull
#define VRAM_BASE_OFFSET 0x60000000ull
#define GART_TABLE_PAGE 0x1ff00ull
/* amdgpu_gmc_pd_addr for a VRAM BO at GART_TABLE_PAGE: its MC address
 * turned physical, with AMDGPU_PTE_VALID. */
static u64 amdgpu_gmc_pd_addr(struct amdgpu_bo *bo)
{
	return ((GART_TABLE_PAGE << 12) + VRAM_BASE_OFFSET) | 1;
}
#include "dchw_dm_funcs.c"

void kdrm_connector_create(struct dc_link *link);


/* The context as the commit path builds it: the stream, and per pipe the
 * blocks it holds (pool instances), its scaler data and pixel clock. */
static void dump_context(const char *tag, struct dc_state *ctx)
{
	printf("context %s streams %d\n", tag, ctx->stream_count);
	for (int s = 0; s < ctx->stream_count; s++) {
		struct dc_stream_state *st = ctx->streams[s];
		struct dc_stream_status *ss = &ctx->stream_status[s];

		printf("  stream %d id %u signal %d phy_pix_clk %d link %d planes %d otg %d\n", s, st->stream_id,
		       st->signal, st->phy_pix_clk, st->link->link_index, ss->plane_count, ss->primary_otg_inst);
	}
	for (int i = 0; i < MAX_PIPES; i++) {
		struct pipe_ctx *p = &ctx->res_ctx.pipe_ctx[i];
		struct scaler_data *d = &p->plane_res.scl_data;
		struct pixel_clk_params *c = &p->stream_res.pix_clk_params;

		if (!p->stream)
			continue;
		printf("  pipe %d idx %d plane %d tg %d opp %d hubp %d dpp %d mpcc %d senc %d clk %d top %d bottom %d"
		       " odm %d %d\n", i, p->pipe_idx, p->plane_state != NULL,
		       p->stream_res.tg ? p->stream_res.tg->inst : -1, p->stream_res.opp ? p->stream_res.opp->inst : -1,
		       p->plane_res.hubp ? p->plane_res.hubp->inst : -1, p->plane_res.dpp ? p->plane_res.dpp->inst : -1,
		       p->plane_res.mpcc_inst, p->stream_res.stream_enc ? (int)p->stream_res.stream_enc->id : -1,
		       p->clock_source ? (int)p->clock_source->id : -1, p->top_pipe ? p->top_pipe->pipe_idx : -1,
		       p->bottom_pipe ? p->bottom_pipe->pipe_idx : -1, p->prev_odm_pipe ? p->prev_odm_pipe->pipe_idx : -1,
		       p->next_odm_pipe ? p->next_odm_pipe->pipe_idx : -1);
		printf("    link lanes %d rate %d spread %d\n", p->link_config.dp_link_settings.lane_count,
		       p->link_config.dp_link_settings.link_rate, p->link_config.dp_link_settings.link_spread);
		printf("    pixclk req %u sym %u ref %u signal %d ctrl %d depth %d enc %d flags 0x%x pll actual %u ref %u"
		       " fb %u.%u post %u\n", c->requested_pix_clk_100hz, c->requested_sym_clk, c->dp_ref_clk,
		       c->signal_type, c->controller_id, c->color_depth, c->pixel_encoding, *(uint32_t *)&c->flags,
		       p->pll_settings.actual_pix_clk_100hz, p->pll_settings.reference_divider,
		       p->pll_settings.feedback_divider, p->pll_settings.fract_feedback_divider,
		       p->pll_settings.pix_clk_post_divider);
		printf("    scl active %d %d vp %d %d %d %d vpc %d %d %d %d recout %d %d %d %d\n", d->h_active, d->v_active,
		       d->viewport.x, d->viewport.y, d->viewport.width, d->viewport.height, d->viewport_c.x,
		       d->viewport_c.y, d->viewport_c.width, d->viewport_c.height, d->recout.x, d->recout.y,
		       d->recout.width, d->recout.height);
		printf("    scl ratios %lld %lld %lld %lld inits %lld %lld %lld %lld taps %d %d %d %d format %d lb %d\n",
		       d->ratios.horz.value, d->ratios.vert.value, d->ratios.horz_c.value, d->ratios.vert_c.value,
		       d->inits.h.value, d->inits.h_c.value, d->inits.v.value, d->inits.v_c.value, d->taps.h_taps,
		       d->taps.v_taps, d->taps.h_taps_c, d->taps.v_taps_c, d->format, d->lb_params.depth);
	}
}

#include "dml_dump.h"

static void dump_wm(const char *n, const struct dcn_watermarks *w)
{
	printf("  wm %s urgent %u pte_meta %u frac_nom %u frac_flip %u urg_lat %u cstate_exit %u cstate_enter_exit %u"
	       " pstate %u usr %u\n", n, w->urgent_ns, w->pte_meta_urgent_ns, w->frac_urg_bw_nom, w->frac_urg_bw_flip,
	       w->urgent_latency_ns, w->cstate_pstate.cstate_exit_ns, w->cstate_pstate.cstate_enter_plus_exit_ns,
	       w->cstate_pstate.pstate_change_ns, w->usr_retraining_ns);
}

/* What bandwidth validation leaves in the context: the clocks, the four
 * watermark sets, per pipe the DLG/TTU/RQ programming DML computed, and
 * DML's whole working state. */
static void dump_bw(const char *tag, struct dc_state *ctx)
{
	struct dc_clocks *c = &ctx->bw_ctx.bw.dcn.clk;

	printf("bw %s dispclk %d dppclk %d dcfclk %d socclk %d deep_sleep %d fclk %d dramclk %d pstate %d zstate %d"
	       " dtbclk %d bw_dpp %d bw_disp %d max_dpp %d max_disp %d compbuf %d\n", tag, c->dispclk_khz, c->dppclk_khz,
	       c->dcfclk_khz, c->socclk_khz, c->dcfclk_deep_sleep_khz, c->fclk_khz, c->dramclk_khz,
	       c->p_state_change_support, c->zstate_support, c->dtbclk_en, c->bw_dppclk_khz, c->bw_dispclk_khz,
	       c->max_supported_dppclk_khz, c->max_supported_dispclk_khz, ctx->bw_ctx.bw.dcn.compbuf_size_kb);
	dump_wm("a", &ctx->bw_ctx.bw.dcn.watermarks.a);
	dump_wm("b", &ctx->bw_ctx.bw.dcn.watermarks.b);
	dump_wm("c", &ctx->bw_ctx.bw.dcn.watermarks.c);
	dump_wm("d", &ctx->bw_ctx.bw.dcn.watermarks.d);
	for (int i = 0; i < MAX_PIPES; i++) {
		struct pipe_ctx *p = &ctx->res_ctx.pipe_ctx[i];
		char q[32];

		if (!p->stream)
			continue;
		printf("  pipe %d det %d unbounded %d dppclk %d\n", i, p->det_buffer_size_kb, p->unbounded_req,
		       p->plane_res.bw.dppclk_khz);
		snprintf(q, sizeof(q), "pipe%d.dest", i);
		dmldump__vcs_dpi_display_pipe_dest_params_st(q, &p->pipe_dlg_param);
		snprintf(q, sizeof(q), "pipe%d.dlg", i);
		dmldump__vcs_dpi_display_dlg_regs_st(q, &p->dlg_regs);
		snprintf(q, sizeof(q), "pipe%d.ttu", i);
		dmldump__vcs_dpi_display_ttu_regs_st(q, &p->ttu_regs);
		snprintf(q, sizeof(q), "pipe%d.rq", i);
		dmldump__vcs_dpi_display_rq_regs_st(q, &p->rq_regs);
	}
	dmldump_vba_vars_st("vba", &ctx->bw_ctx.dml.vba);
}

/* udelay for the DMUB service's polls (dmub_srv.c): time does not pass
 * in the register file. */
void (udelay)(unsigned long usecs)
{
}

/* dml2_destroy (dml2_wrapper.c), which dc_state_release calls with the
 * state's DML2 context; DCN 2.1 has none (dml2 is not built). */
void dml2_destroy(struct dml2_context *dml2)
{
	if (dml2)
		abort();
}

static void preset(u32 reg, u32 val)
{
	printf("preset reg 0x%x = 0x%x\n", reg, val);
	set(reg, val);
}

/* The boot image over the lit rule. */
static void boot_preset(struct dc *dc)
{
	struct resource_pool *pool = dc->res_pool;

	preset(dc->ctx->dc_bios->regs->BIOS_SCRATCH_6, 0);
	preset(dc->hwseq->regs->MICROSECOND_TIME_BASE_DIV, 0x00120464);
	for (unsigned int i = 0; i < pool->timing_generator_count; i++) {
		struct optc *optc1 = DCN10TG_FROM_TG(pool->timing_generators[i]);
		const struct dcn_optc_shift *s = optc1->tg_shift;

		preset(optc1->tg_regs->OPTC_DATA_SOURCE_SELECT, i << s->OPTC_SEG0_SRC_SEL | 0xf << s->OPTC_SEG1_SRC_SEL);
		preset(optc1->tg_regs->OPTC_DATA_FORMAT_CONTROL, 0);
		preset(optc1->tg_regs->OTG_H_BLANK_START_END, 2000 << s->OTG_H_BLANK_START | 80 << s->OTG_H_BLANK_END);
		preset(optc1->tg_regs->OTG_V_BLANK_START_END, 1110 << s->OTG_V_BLANK_START | 30 << s->OTG_V_BLANK_END);
	}
	for (unsigned int i = 0; i < pool->pipe_count; i++) {
		struct dcn20_opp *oppn20 = TO_DCN20_OPP(pool->opps[i]);

		preset(oppn20->regs->DPG_CONTROL, 0);
		preset(oppn20->regs->DPG_STATUS, 0);
	}
}

static u32 zero_read(struct card_info *info, u32 reg) { return 0; }
static void no_write(struct card_info *info, u32 reg, u32 val) {}

int main(int argc, char **argv)
{
	setvbuf(stdout, NULL, _IOLBF, 0);
	FILE *f = fopen(argv[1], "rb");
	static unsigned char bios[1 << 20];
	fread(bios, 1, sizeof(bios), f);
	fclose(f);
	bool boot = !strcmp(argv[2], "boot");
	unset_rule = !strcmp(argv[2], "ones") ? ones_rule : !strcmp(argv[2], "zeros") ? zeros_rule :
		     !strcmp(argv[2], "lit") || boot ? lit_rule : hash_rule;

	static struct card_info card = { .reg_read = zero_read, .reg_write = no_write, .mc_read = zero_read,
		.mc_write = no_write, .pll_read = zero_read, .pll_write = no_write };
	static struct amdgpu_device adev;
	static struct pci_dev pdev = { .vendor = 0x1002, .device = 0x164c, .revision = 0xc1,
		.subsystem_vendor = 0x17aa, .subsystem_device = 0x3f1a };
	static uint32_t dcn[5] = { 0x12, 0xc0, 0x34c0, 0x9000, 0x2403c00 };
	static uint32_t nbio[5] = { 0, 0x14, 0xd20, 0x10400, 0x241b000 };
	static uint32_t clk[5] = { 0x16c00, 0x16e00, 0x17000, 0x17200, 0x1b000 };
	adev.asic_type = 24;
	adev.flags = 0x20000;
	adev.dm.dmcub_fw_version = 0x0101002b;
	adev.ip_versions[DCE_HWIP][0] = 0x02010000;
	adev.pdev = &pdev;
	adev.family = 142;
	adev.external_rev_id = 0x91;
	adev.gmc.vram_width = 64;
	adev.mode_info.atom_context = amdgpu_atom_parse(&card, bios);
	adev.dm.cgs_device = &adev;
	adev.reg_offset[DCE_HWIP][0] = dcn;
	adev.reg_offset[NBIO_HWIP][0] = nbio;
	adev.reg_offset[CLK_HWIP][0] = clk;
	adev.apu_flags = AMD_APU_IS_RENOIR;
	adev.gmc.fb_start = VRAM_START;
	adev.gmc.fb_end = VRAM_START + 0x20000000 - 1;
	adev.gmc.gart_start = 0;
	adev.gmc.gart_end = 0x40000000 - 1;
	adev.gmc.agp_start = 0xf800000000ull;
	adev.gmc.agp_end = 0xffffffffffffull;
	adev.vm_manager.vram_base_offset = VRAM_BASE_OFFSET;
	static char gart_bo;
	adev.gart.bo = (struct amdgpu_bo *)&gart_bo;

	printf("amdgpu_dm_init %d\n", amdgpu_dm_init(&adev));
	struct dc *dc = adev.dm.dc;
	if (boot)
		boot_preset(dc);
	bool commit = argc > 3 && !strcmp(argv[3], "commit");
	bool detect = commit || (argc > 3 && !strcmp(argv[3], "detect"));
	/* Every HPD but the panel's low, named from Linux's own pin tables. */
	if (detect)
		for (int l = 1; l < dc->link_count; l++)
			if (dc->links[l]->hpd_gpio && dc->links[l]->hpd_gpio->pin)
				preset(HW_HPD_FROM_BASE(dc->links[l]->hpd_gpio->pin)->regs->int_status, 0);

	static struct dmub_srv dmub;
	struct dmub_srv_create_params p = { .funcs = { .reg_read = dmub_reg_read, .reg_write = dmub_reg_write },
		.asic = DMUB_ASIC_DCN21 };
	dmub_srv_create(&dmub, &p);
	dmub.hw_init = true;
	dmub.power_state = DMUB_POWER_STATE_D0;
	dc->ctx->dmub_srv = dc_dmub_srv_create(dc, &dmub);
	/* DMUB_FW_BOOT_STATUS: dal_fw | mailbox_rdy. */
	if (boot)
		preset(dmub.regs->offset.DMCUB_SCRATCH0, 0x3);

	printf("dc_hardware_init\n");
	dc_hardware_init(dc);

	printf("dc_setup_system_context\n");
	if ((adev.flags & AMD_IS_APU) && (adev.asic_type >= CHIP_CARRIZO)) {
		struct dc_phy_addr_space_config pa_config;

		mmhub_read_system_context(&adev, &pa_config);
		struct dc_phy_addr_space_config *a = &pa_config;
		printf("pa_config aperture 0x%llx 0x%llx fb 0x%llx 0x%llx 0x%llx agp 0x%llx 0x%llx 0x%llx\n",
		       a->system_aperture.start_addr, a->system_aperture.end_addr, a->system_aperture.fb_base,
		       a->system_aperture.fb_top, a->system_aperture.fb_offset, a->system_aperture.agp_base,
		       a->system_aperture.agp_bot, a->system_aperture.agp_top);
		printf("pa_config gart 0x%llx 0x%llx 0x%llx hvm %d\n", a->gart_config.page_table_start_addr,
		       a->gart_config.page_table_end_addr, a->gart_config.page_table_base_addr, a->is_hvm_enabled);

		printf("num_vmids %d\n", dc_setup_system_context(adev.dm.dc, &pa_config));
		printf("riommu_active %d vm_pa_config valid %d\n", dc->res_pool->hubbub->riommu_active,
		       dc->vm_pa_config.valid);
	}

	/* argv[3] = detect: amdgpu_dm_initialize_drm_device's loop over the
	 * links, on the eDP panel with every other HPD low. */
	if (detect) {
		for (int i = 0; i < AUX_ENGINES && dc->res_pool->engines[i]; i++) {
			aux_engines[i].r = container_of(dc->res_pool->engines[i], struct aux_engine_dce110, base)->regs;
			n_aux_engines = i + 1;
		}
		panel_sink_init();
		aux_engines[dc->links[0]->ddc_hw_inst].sink = true;
		for (int l = 0; l < dc->link_count; l++) {
			struct dc_link *link = dc->links[l];
			enum dc_connection_type new_connection_type = dc_connection_none;

			if (link->connector_signal == SIGNAL_TYPE_VIRTUAL)
				continue;
			kdrm_connector_create(link);
			printf("detect link %d\n", l);
			if (!dc_link_detect_connection_type(link, &new_connection_type))
				printf("KMS: Failed to detect connector\n");
			printf("dc_link_detect %d\n", dc_link_detect(link, DETECT_REASON_BOOT));
		}
		for (int l = 0; l < dc->link_count; l++) {
			struct dc_link *link = dc->links[l];
			struct dc_sink *sink = link->local_sink;

			printf("link %d type %d signal %d", l, link->type, sink ? sink->sink_signal : -1);
			printf(" cap lanes %d rate %d spread %d verified lanes %d rate %d\n", link->reported_link_cap.lane_count,
			       link->reported_link_cap.link_rate, link->reported_link_cap.link_spread,
			       link->verified_link_cap.lane_count, link->verified_link_cap.link_rate);
			if (!sink)
				continue;
			struct dc_edid_caps *c = &sink->edid_caps;
			printf("  sink edid %u mfg 0x%x prod 0x%x serial 0x%x week %d year %d name '%s' hdmi %d audio %d"
			       " speakers 0x%x\n", sink->dc_edid.length, c->manufacturer_id, c->product_id,
			       c->serial_number, c->manufacture_week, c->manufacture_year, c->display_name, c->edid_hdmi,
			       c->audio_mode_count, c->speaker_flags);
			printf("  dpcd rev 0x%x lanes 0x%x spread 0x%x ext_caps 0x%x sink_count %d branch %d\n",
			       link->dpcd_caps.dpcd_rev.raw, link->dpcd_caps.max_ln_count.raw,
			       link->dpcd_caps.max_down_spread.raw, link->dpcd_sink_ext_caps.raw,
			       link->dpcd_caps.sink_count.raw, link->dpcd_caps.is_branch_dev);
		}
	}

	/* argv[3] = commit: after detection, a 1920x1080 stream for the panel
	 * from its EDID's first detailed timing, one ARGB8888 plane, then
	 * dc_validate_global_state, dc_commit_streams and
	 * dc_update_planes_and_stream as amdgpu_dm_atomic_commit_tail calls
	 * them. */
	if (commit && dc->links[0]->local_sink) {
		struct dc_stream_state *stream = dc_create_stream_for_sink(dc->links[0]->local_sink);
		struct dc_crtc_timing *t = &stream->timing;

		t->h_addressable = 1920;
		t->h_total = 1920 + 222;
		t->h_front_porch = 60;
		t->h_sync_width = 32;
		t->v_addressable = 1080;
		t->v_total = 1080 + 20;
		t->v_front_porch = 3;
		t->v_sync_width = 6;
		t->pix_clk_100hz = 1414000;
		t->display_color_depth = COLOR_DEPTH_888;
		t->pixel_encoding = PIXEL_ENCODING_RGB;
		t->scan_type = SCANNING_TYPE_NODATA;
		t->flags.HSYNC_POSITIVE_POLARITY = 1;
		t->flags.VSYNC_POSITIVE_POLARITY = 1;
		stream->src.width = stream->dst.width = 1920;
		stream->src.height = stream->dst.height = 1080;
		stream->output_color_space = COLOR_SPACE_SRGB;
		stream->out_transfer_func.type = TF_TYPE_PREDEFINED;
		stream->out_transfer_func.tf = TRANSFER_FUNCTION_SRGB;

		struct dc_plane_state *plane = dc_create_plane_state(dc);
		plane->format = SURFACE_PIXEL_FORMAT_GRPH_ARGB8888;
		plane->plane_size.surface_size.width = 1920;
		plane->plane_size.surface_size.height = 1080;
		plane->plane_size.surface_pitch = 1920;
		plane->src_rect = plane->dst_rect = plane->clip_rect = (struct rect){ 0, 0, 1920, 1080 };
		/* argv[4] = scaled: a 1280x720 surface shown at 1600x900, offset,
		 * so the scaler's ratios, taps, inits and viewport are not the
		 * identity. */
		if (argc > 4 && !strcmp(argv[4], "scaled")) {
			plane->plane_size.surface_size.width = 1280;
			plane->plane_size.surface_size.height = 720;
			plane->plane_size.surface_pitch = 1280;
			plane->src_rect = (struct rect){ 0, 0, 1280, 720 };
			plane->dst_rect = plane->clip_rect = (struct rect){ 100, 50, 1600, 900 };
		}
		plane->tiling_info.gfx9.swizzle = DC_SW_LINEAR;
		plane->rotation = ROTATION_ANGLE_0;
		plane->visible = true;
		plane->color_space = COLOR_SPACE_SRGB;
		plane->address.type = PLN_ADDR_TYPE_GRAPHICS;
		plane->address.grph.addr.quad_part = VRAM_START + 0x1000000;
		plane->global_alpha_value = 0xff;
		plane->in_transfer_func.type = TF_TYPE_PREDEFINED;
		plane->in_transfer_func.tf = TRANSFER_FUNCTION_SRGB;

		struct dc_state *context = dc_state_create_current_copy(dc);
		enum dc_status added = dc_state_add_stream(dc, context, stream);
		printf("dc_state_add_stream %d\n", added);
		printf("dc_state_add_plane %d\n", dc_state_add_plane(dc, stream, plane, context));
		dump_context("added", context);
		printf("dc_validate_global_state %d\n",
		       dc_validate_global_state(dc, context, DC_VALIDATE_MODE_AND_PROGRAMMING));
		dump_context("validated", context);
		dump_bw("validated", context);
		/* A stream the pool has no pipe for (every pipe fused off) is not
		 * committed: dc_commit_streams would reach a missing OTG. */
		if (added == DC_OK) {
			struct dc_commit_streams_params params = { .streams = &stream, .stream_count = 1 };
			printf("dc_commit_streams %d\n", dc_commit_streams(dc, &params));
			struct dc_surface_update srf = { .surface = plane };
			printf("dc_update_planes_and_stream %d\n",
			       dc_update_planes_and_stream(dc, &srf, 1, stream, NULL));
			/* Two page flips as amdgpu_dm_commit_planes builds them: the
			 * plane's unchanged scaling and plane info, a new address,
			 * and a stream update naming only the stream. */
			for (int f = 0; f < 2; f++) {
				struct dc_plane_info pi = {
					.plane_size = plane->plane_size, .tiling_info = plane->tiling_info,
					.dcc = plane->dcc, .format = plane->format, .rotation = plane->rotation,
					.stereo_format = plane->stereo_format, .color_space = plane->color_space,
					.horizontal_mirror = plane->horizontal_mirror, .visible = plane->visible,
					.per_pixel_alpha = plane->per_pixel_alpha,
					.pre_multiplied_alpha = plane->pre_multiplied_alpha,
					.global_alpha = plane->global_alpha,
					.global_alpha_value = plane->global_alpha_value,
					.input_csc_enabled = plane->input_csc_color_matrix.enable_adjustment,
					.layer_index = plane->layer_index, .cositing = plane->cositing,
				};
				struct dc_scaling_info si = {
					.src_rect = plane->src_rect, .dst_rect = plane->dst_rect,
					.clip_rect = plane->clip_rect, .scaling_quality = plane->scaling_quality,
				};
				struct dc_flip_addrs fa = { .address = plane->address };
				fa.address.grph.addr.quad_part = VRAM_START + 0x2000000 + f * 0x1000000;
				fa.flip_timestamp_in_us = 1000 + f * 16667;
				struct dc_surface_update fu = {
					.surface = plane, .flip_addr = &fa, .plane_info = &pi, .scaling_info = &si,
				};
				struct dc_stream_update su = { .stream = stream };
				printf("flip %d\n", dc_update_planes_and_stream(dc, &fu, 1, stream, &su));
			}
		}
	}

	for (int c = 0; c < n_dmub_cmds; c++) {
		printf("dmub cmd");
		for (int i = 0; i < DMUB_RB_CMD_SIZE; i++)
			printf(" %02x", dmub_cmds[c][i]);
		printf("\n");
	}
	struct pp_smu_wm_range_sets *ranges = &dc->clk_mgr->ranges;
	printf("wm_ranges reader %u writer %u\n", ranges->num_reader_wm_sets, ranges->num_writer_wm_sets);
	for (unsigned int i = 0; i < ranges->num_reader_wm_sets; i++)
		print_range("reader", i, &ranges->reader_wm_sets[i]);
	for (unsigned int i = 0; i < ranges->num_writer_wm_sets; i++)
		print_range("writer", i, &ranges->writer_wm_sets[i]);
	struct resource_pool *pool = dc->res_pool;
	printf("ref_clocks xtalin %u dccg %u dchub %u\n", pool->ref_clocks.xtalin_clock_inKhz,
	       pool->ref_clocks.dccg_ref_clock_inKhz, pool->ref_clocks.dchub_ref_clock_inKhz);
	for (int l = 0; l < dc->link_count; l++)
		printf("link %d sink_present %d active %d fec %d\n", l, dc->links[l]->edp_sink_present,
		       dc->links[l]->link_status.link_active, dc->links[l]->fec_state);
	return 0;
}
