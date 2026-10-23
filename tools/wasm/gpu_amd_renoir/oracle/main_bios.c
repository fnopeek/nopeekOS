/* The DC VBIOS parser oracle: Linux's bios_parser2_construct on the device's
 * VBIOS (argv[1]), as dc_construct runs it for DCN 2.1. Prints the parser
 * state, the bound command tables, the firmware info and the integrated
 * info. hostcheck/bios_check.rs prints the same for the port.
 *
 * The command-table executors and helper tables are named symbols here: the
 * oracle checks which one each entry binds, not what it runs. */
#include <linux/sched.h>
#include <linux/slab.h>
#include "atomfirmware.h"
#include "atom.h"
#include "ObjectID.h"
#include "bios_types_std.h"
#include "bios_types_grph.h"
#include "bios_types.h"

#define container_of(ptr, type, member) ((type *)((char *)(ptr) - offsetof(type, member)))
#define DC_LOG_BIOS(...) do { } while (0)
#define fallthrough __attribute__((fallthrough))
#define ASSERT(x) do { } while (0)
#define ASSERT_CRITICAL(x) do { } while (0)
#define BREAK_TO_DEBUGGER() do { } while (0)
#define dm_output_to_console(...) do { } while (0)
#define min_t(t, a, b) ((t)(a) < (t)(b) ? (t)(a) : (t)(b))
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define swap(a, b) do { typeof(a) __t = (a); (a) = (b); (b) = __t; } while (0)

struct amdgpu_device {
	struct amdgpu_mode_info { struct atom_context *atom_context; } mode_info;
};
struct dc;
struct dc_context {
	struct dc *dc;
	void *driver_context;
	enum dce_version dce_version;
};
struct dc {
	struct dc_context *ctx;
	struct { unsigned int force_bios_fixed_vs; } config;
};
struct dc_vbios_funcs { int unused; };
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
	struct dc_golden_table golden_table;
};
struct bios_parser;
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

#include "bios_protos.h"
#include "bios_funcs.c"

static u32 zero_read(struct card_info *info, u32 reg) { return 0; }
static void no_write(struct card_info *info, u32 reg, u32 val) {}

static const char *fn(void *p)
{
#define N(n) if (p == (void *)n) return #n;
	N(encoder_control_digx_v1_5) N(encoder_control_fallback) N(transmitter_control_v1_6)
	N(transmitter_control_v1_7) N(transmitter_control_fallback) N(set_pixel_clock_v7)
	N(set_pixel_clock_fallback) N(set_crtc_using_dtd_timing_v3) N(enable_crtc_v1)
	N(external_encoder_control_v3) N(enable_disp_power_gating_v2_1)
	N(enable_disp_power_gating_fallback) N(set_dce_clock_v2_1) N(get_smu_clock_info_v3_1)
	N(enable_lvtma_control)
	N(&helper_dce60) N(&helper_dce80) N(&helper_dce110) N(&helper_dce112_2)
	return "NULL";
}

static void gid(const char *n, struct graphics_object_id g)
{
	printf("%s id %u enum %u type %u\n", n, g.id, g.enum_id, g.type);
}

static void retimer(int dp, unsigned char slv, unsigned char num, struct i2c_reg_info *r,
		    unsigned char num6, struct i2c_reg_info *r6)
{
	printf("dp%d slv %u num %u num6 %u\n", dp, slv, num, num6);
	for (int i = 0; i < 9; i++)
		printf("dp%d reg %d %u %u\n", dp, i, r[i].i2c_reg_index, r[i].i2c_reg_val);
	for (int i = 0; i < 3; i++)
		printf("dp%d reg6 %d %u %u\n", dp, i, r6[i].i2c_reg_index, r6[i].i2c_reg_val);
}

int main(int argc, char **argv)
{
	FILE *f = fopen(argv[1], "rb");
	static unsigned char bios[1 << 20];
	fread(bios, 1, sizeof(bios), f);
	fclose(f);
	static struct card_info card = { .reg_read = zero_read, .reg_write = no_write, .mc_read = zero_read,
		.mc_write = no_write, .pll_read = zero_read, .pll_write = no_write };
	static struct amdgpu_device adev;
	adev.mode_info.atom_context = amdgpu_atom_parse(&card, bios);

	static struct dc dc;
	static struct dc_context ctx = { .dc = &dc, .driver_context = &adev, .dce_version = DCN_VERSION_2_1 };
	dc.ctx = &ctx;
	struct bp_init_data init = { .ctx = &ctx, .bios = bios };
	struct bios_parser *bp = calloc(1, sizeof(*bp));
	bool ok = bios_parser2_construct(bp, &init, DCN_VERSION_2_1);

	printf("construct %d\n", ok);
	printf("bios_size %u\n", bp->base.bios_size);
	printf("master_data_tbl %lx\n", (long)((uint8_t *)bp->master_data_tbl - bios));
	printf("object_info_tbl_offset %x rev %u.%u v1_4 %lx v1_5 %lx\n", bp->object_info_tbl_offset,
	       bp->object_info_tbl.revision.major, bp->object_info_tbl.revision.minor,
	       bp->object_info_tbl.v1_4 ? (long)((uint8_t *)bp->object_info_tbl.v1_4 - bios) : 0L,
	       bp->object_info_tbl.v1_5 ? (long)((uint8_t *)bp->object_info_tbl.v1_5 - bios) : 0L);
	struct cmd_tbl *t = &bp->cmd_tbl;
	printf("dig_encoder_control %s\n", fn(t->dig_encoder_control));
	printf("transmitter_control %s\n", fn(t->transmitter_control));
	printf("set_pixel_clock %s\n", fn(t->set_pixel_clock));
	printf("set_crtc_timing %s\n", fn(t->set_crtc_timing));
	printf("enable_crtc %s\n", fn(t->enable_crtc));
	printf("external_encoder_control %s\n", fn(t->external_encoder_control));
	printf("enable_disp_power_gating %s\n", fn(t->enable_disp_power_gating));
	printf("set_dce_clock %s\n", fn(t->set_dce_clock));
	printf("get_smu_clock_info %s\n", fn((void *)t->get_smu_clock_info));
	printf("enable_lvtma_control %s\n", fn(t->enable_lvtma_control));
	printf("cmd_helper %s\n", fn((void *)bp->cmd_helper));

	struct dc_firmware_info *fw = &bp->base.fw_info;
	printf("fw_info_valid %d\n", bp->base.fw_info_valid);
	printf("pll %u %u %u %u %u\n", fw->pll_info.crystal_frequency, fw->pll_info.min_input_pxl_clk_pll_frequency,
	       fw->pll_info.max_input_pxl_clk_pll_frequency, fw->pll_info.min_output_pxl_clk_pll_frequency,
	       fw->pll_info.max_output_pxl_clk_pll_frequency);
	printf("feature %u %u\n", fw->feature.memory_clk_ss_percentage, fw->feature.engine_clk_ss_percentage);
	printf("fw %u %u %u %u %u %u %u %u %u %d %u\n", fw->default_display_engine_pll_frequency,
	       fw->external_clock_source_frequency_for_dp, fw->smu_gpu_pll_output_freq, fw->min_allowed_bl_level,
	       fw->remote_display_config, fw->default_memory_clk, fw->default_engine_clk, fw->dp_phy_ref_clk,
	       fw->i2c_engine_ref_clk, fw->oem_i2c_present, fw->oem_i2c_obj_id);
	printf("vram_info %u %u\n", bp->base.vram_info.num_chans, bp->base.vram_info.dram_channel_width_bytes);

	struct integrated_info *ii = bp->base.integrated_info;
	printf("integrated_info %d\n", ii != NULL);
	if (!ii)
		return 0;
	for (int i = 0; i < NUMBER_OF_DISP_CLK_VOLTAGE; i++)
		printf("disp_clk_voltage %d %u %u\n", i, ii->disp_clk_voltage[i].voltage_index,
		       ii->disp_clk_voltage[i].max_supported_clk);
	for (int i = 0; i < MAX_NUMBER_OF_EXT_DISPLAY_PATH; i++) {
		struct external_display_path *p = &ii->ext_disp_conn_info.path[i];
		printf("path %d tag %u acpi %u aux %u hpd %u map %u caps %u\n", i, p->device_tag, p->device_acpi_enum,
		       p->ext_aux_ddc_lut_index, p->ext_hpd_pin_lut_index, p->channel_mapping.raw, p->caps);
		gid("  connector", p->device_connector_id);
		gid("  encoder", p->ext_encoder_obj_id);
	}
	printf("guid");
	for (int i = 0; i < NUMBER_OF_UCHAR_FOR_GUID; i++)
		printf(" %u", ii->ext_disp_conn_info.gu_id[i]);
	printf("\nchecksum %u fixdpvoltageswing %u\n", ii->ext_disp_conn_info.checksum,
	       ii->ext_disp_conn_info.fixdpvoltageswing);
	printf("mem %u %u gpu_cap %u system_config %u cpu_cap %u\n", ii->memory_type, ii->ma_channel_number,
	       ii->gpu_cap_info, ii->system_config, ii->cpu_cap_info);
	printf("ss lvds %u %u hdmi %u %u dvi %u %u dp_ss_control %u lvds_misc %u\n", ii->lvds_ss_percentage,
	       ii->lvds_sspread_rate_in_10hz, ii->hdmi_ss_percentage, ii->hdmi_sspread_rate_in_10hz,
	       ii->dvi_ss_percentage, ii->dvi_sspread_rate_in_10_hz, ii->dp_ss_control, ii->lvds_misc);
	retimer(0, ii->dp0_ext_hdmi_slv_addr, ii->dp0_ext_hdmi_reg_num, ii->dp0_ext_hdmi_reg_settings,
		ii->dp0_ext_hdmi_6g_reg_num, ii->dp0_ext_hdmi_6g_reg_settings);
	retimer(1, ii->dp1_ext_hdmi_slv_addr, ii->dp1_ext_hdmi_reg_num, ii->dp1_ext_hdmi_reg_settings,
		ii->dp1_ext_hdmi_6g_reg_num, ii->dp1_ext_hdmi_6g_reg_settings);
	retimer(2, ii->dp2_ext_hdmi_slv_addr, ii->dp2_ext_hdmi_reg_num, ii->dp2_ext_hdmi_reg_settings,
		ii->dp2_ext_hdmi_6g_reg_num, ii->dp2_ext_hdmi_6g_reg_settings);
	retimer(3, ii->dp3_ext_hdmi_slv_addr, ii->dp3_ext_hdmi_reg_num, ii->dp3_ext_hdmi_reg_settings,
		ii->dp3_ext_hdmi_6g_reg_num, ii->dp3_ext_hdmi_6g_reg_settings);
	printf("edp1 %u %u %u %u %u %u %u %u\n", ii->edp1_info.edp_backlight_pwm_hz, ii->edp1_info.edp_ss_percentage,
	       ii->edp1_info.edp_ss_rate_10hz, ii->edp1_info.edp_pwr_on_off_delay, ii->edp1_info.edp_pwr_on_vary_bl_to_blon,
	       ii->edp1_info.edp_pwr_down_bloff_to_vary_bloff, ii->edp1_info.edp_panel_bpc, ii->edp1_info.edp_bootup_bl_level);
	printf("gpuclk_ss %u %u\n", ii->gpuclk_ss_percentage, ii->gpuclk_ss_type);

	/* The object table, as create_links and construct_phy ask it. */
	struct dc_bios *dcb = &bp->base;
	uint8_t n = bios_parser_get_connectors_number(dcb);
	printf("connectors %u\n", n);
	for (int i = 0; i < 8; i++) {
		struct graphics_object_id c = bios_parser_get_connector_id(dcb, i);
		printf("connector %d", i);
		gid("", c);
		struct graphics_object_id src = { 0 };
		enum bp_result r = bios_parser_get_src_obj(dcb, c, 0, &src);
		printf("  src %d", r);
		gid("", src);
		struct graphics_object_id gsrc = { 0 };
		r = bios_parser_get_src_obj(dcb, src, 0, &gsrc);
		printf("  encoder src %d", r);
		gid("", gsrc);
		struct graphics_object_i2c_info i2c = { 0 };
		r = bios_parser_get_i2c_info(dcb, c, &i2c);
		printf("  i2c %d hw_assist %d line %u engine %u slave %u clk_a %u shift %u\n", r, i2c.i2c_hw_assist,
		       i2c.i2c_line, i2c.i2c_engine_id, i2c.i2c_slave_address, i2c.gpio_info.clk_a_register_index,
		       i2c.gpio_info.clk_a_shift);
		struct graphics_object_hpd_info hpd = { 0 };
		r = bios_parser_get_hpd_info(dcb, c, &hpd);
		printf("  hpd %d uid %u active %u\n", r, hpd.hpd_int_gpio_uid, hpd.hpd_active);
		struct gpio_pin_info pin = { 0 };
		r = bios_parser_get_gpio_pin_info(dcb, hpd.hpd_int_gpio_uid, &pin);
		printf("  pin %d %u %u %u %u %u %u %u %u\n", r, pin.offset, pin.offset_y, pin.offset_en, pin.offset_mask,
		       pin.mask, pin.mask_y, pin.mask_en, pin.mask_mask);
		struct connector_device_tag_info tag = { 0 };
		r = bios_parser_get_device_tag(dcb, c, 0, &tag);
		printf("  tag %d acpi %u type %u enum %u raw %u supported %d\n", r, tag.acpi_device,
		       tag.dev_id.device_type, tag.dev_id.enum_id, tag.dev_id.raw_device_tag,
		       bios_parser_is_device_id_supported(dcb, tag.dev_id));
		struct bp_disp_connector_caps_info caps = { 0 };
		r = bios_parser_get_disp_connector_caps_info(dcb, c, &caps);
		printf("  caps %d internal %u bl %u\n", r, caps.INTERNAL_DISPLAY, caps.INTERNAL_DISPLAY_BL);
	}
	for (int g = 0; g < 256; g += 17) {
		struct graphics_object_id gen = dal_graphics_object_id_init(g & 0xff, ENUM_ID_1, OBJECT_TYPE_GENERIC);
		struct graphics_object_i2c_info i2c = { 0 };
		enum bp_result r = bios_parser_get_i2c_info(dcb, gen, &i2c);
		printf("generic i2c %d: %d line %u engine %u\n", g, r, i2c.i2c_line, i2c.i2c_engine_id);
	}
	return 0;
}
