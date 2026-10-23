/* The link oracle: Linux's create_links -> link_create -> construct_phy on
 * the VBIOS (argv[1]), with the gpio/ sources compiled unchanged, the DCN 2.1
 * link encoder and panel control constructors, program_hpd_filter and one
 * virtual link. `struct dc_link`, `struct dc` and `struct resource_pool`
 * carry the members this path touches. Unwritten registers read hash(reg);
 * every register write is printed, then every link.
 * hostcheck/link_check.rs prints the same for the port. */
#include <linux/sched.h>
#include "dm_services.h"
#include "atomfirmware.h"
#include "atom.h"
#include "ObjectID.h"
#include "include/grph_object_ctrl_defs.h"
#include "bios_types.h"
#include "include/gpio_service_interface.h"
#include "hw_gpio.h"
#include "hw_translate.h"
#include "hw_factory.h"
#include "gpio_service.h"
#include "dcn21/hw_translate_dcn21.h"
#include "dcn21/hw_factory_dcn21.h"

#define DC_LOG_BIOS(...) do { } while (0)
#define DC_LOG_DC(...) do { } while (0)
#define DC_LOGGER_INIT(x) do { } while (0)
#define min_t(t, a, b) ((t)(a) < (t)(b) ? (t)(a) : (t)(b))
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define swap(a, b) do { typeof(a) __t = (a); (a) = (b); (b) = __t; } while (0)
#define SMU9_SYSPLL0_ID 0

/* The register file. */
#define NREGS 4096
static u32 reg_addr[NREGS], reg_val[NREGS];
static int nregs;

static int find(u32 reg)
{
	for (int i = 0; i < nregs; i++)
		if (reg_addr[i] == reg)
			return i;
	return -1;
}

uint32_t dc_mmio_read(uint32_t reg)
{
	int i = find(reg);
	return i < 0 ? (reg * 0x9e3779b1u) ^ 0xa5a5a5a5u : reg_val[i];
}

void dc_mmio_write(uint32_t reg, uint32_t val)
{
	printf("write reg 0x%x = 0x%x\n", reg, val);
	int i = find(reg);
	if (i < 0) {
		i = nregs++;
		reg_addr[i] = reg;
	}
	reg_val[i] = val;
}

struct amdgpu_device {
	struct amdgpu_mode_info { struct atom_context *atom_context; } mode_info;
};
#include "link_types.h"
#define DC_LOG_WARNING(...) do { } while (0)
#define LINK_INFO(...) do { } while (0)
#define DC_LOGGER_INIT(x) do { } while (0)
struct dc_vbios_funcs {
	enum bp_result (*get_hpd_info)(struct dc_bios *dcb, struct graphics_object_id id,
				       struct graphics_object_hpd_info *info);
	enum bp_result (*get_gpio_pin_info)(struct dc_bios *dcb, uint32_t gpio_id, struct gpio_pin_info *info);
	enum bp_result (*get_i2c_info)(struct dc_bios *dcb, struct graphics_object_id id,
				       struct graphics_object_i2c_info *info);
	uint8_t (*get_connectors_number)(struct dc_bios *dcb);
	struct graphics_object_id (*get_connector_id)(struct dc_bios *dcb, uint8_t connector_index);
	enum bp_result (*get_src_obj)(struct dc_bios *bios, struct graphics_object_id object_id, uint32_t index,
				      struct graphics_object_id *src_object_id);
	enum bp_result (*get_device_tag)(struct dc_bios *bios, struct graphics_object_id connector_object_id,
					 uint32_t device_tag_index, struct connector_device_tag_info *info);
	bool (*is_device_id_supported)(struct dc_bios *bios, struct device_id id);
	enum bp_result (*get_encoder_cap_info)(struct dc_bios *bios, struct graphics_object_id object_id,
					       struct bp_encoder_cap_info *info);
	enum bp_result (*get_disp_connector_caps_info)(struct dc_bios *bios, struct graphics_object_id object_id,
						       struct bp_disp_connector_caps_info *info);
	enum bp_result (*get_atom_dc_golden_table)(struct dc_bios *dcb);
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
static unsigned int get_smu_clock_info_v3_1(struct bios_parser *bp, uint8_t id) { return 0; }
static const struct command_table_helper helper_dce60, helper_dce80, helper_dce110, helper_dce112_2;
static const struct command_table_helper *dal_cmd_tbl_helper_dce60_get_table(void) { return &helper_dce60; }
static const struct command_table_helper *dal_cmd_tbl_helper_dce80_get_table(void) { return &helper_dce80; }
static const struct command_table_helper *dal_cmd_tbl_helper_dce110_get_table(void) { return &helper_dce110; }
static const struct command_table_helper *dal_cmd_tbl_helper_dce112_get_table2(void) { return &helper_dce112_2; }

/* The members of struct dc_link, struct dc and struct resource_pool the
 * construct path touches. */
struct dc_link_training_overrides { int unused; };
struct dc_link_settings { int unused; };
struct dc_link {
	struct dc *dc;
	struct dc_context *ctx;
	uint32_t link_index;
	struct graphics_object_id link_id;
	enum display_endpoint_type ep_type;
	bool is_internal_display;
	enum dc_irq_source irq_source_hpd, irq_source_hpd_rx, irq_source_read_request;
	struct { void *dpcd_caps; } link_status;
	int dpcd_caps;
	struct dc_link_training_overrides preferred_training_settings;
	struct dc_link_settings preferred_link_setting;
	struct gpio *hpd_gpio;
	enum signal_type connector_signal;
	struct ddc_service *ddc;
	uint32_t ddc_hw_inst;
	enum hpd_source_id hpd_src;
	struct link_encoder *link_enc;
	enum engine_id eng_id;
	uint32_t link_enc_hw_inst;
	struct panel_cntl *panel_cntl;
	struct connector_device_tag_info device_tag;
	union ddi_channel_mapping ddi_channel_mapping;
	uint16_t chip_caps;
	struct { uint8_t VOLTAGE_SWING, PRE_EMPHASIS; } bios_forced_drive_settings;
	struct { bool psr_vtotal_control_support; enum dc_psr_version psr_version; } psr_settings;
	bool is_hpd_filter_disabled;
};
struct resource_funcs {
	void (*link_init)(struct dc_link *link);
	struct link_encoder *(*link_enc_create)(struct dc_context *ctx, const struct encoder_init_data *init);
	struct panel_cntl *(*panel_cntl_create)(const struct panel_cntl_init_data *panel_cntl_init_data);
};
struct resource_caps { int num_usb4_dpia, num_dig_link_enc; };
struct resource_pool {
	const struct resource_funcs *funcs;
	const struct resource_caps *res_cap;
	struct link_encoder *link_encoders[MAX_DIG_LINK_ENCODERS];
	unsigned int dig_link_enc_count;
	unsigned int usb4_dpia_count;
};
struct link_service {
	struct dc_link *(*create_link)(const struct link_init_data *init_params);
};
struct dc {
	struct dc_context *ctx;
	struct resource_pool *res_pool;
	const struct link_service *link_srv;
	struct {
		unsigned int force_bios_fixed_vs;
		int smart_mux_version;
		uint32_t allow_edp_hotplug_detection;
	} config;
	struct { bool is_apu; uint32_t num_of_internal_disp; } caps;
	struct { bool hdmi20_disable; } debug;
	struct dc_link *links[MAX_LINKS];
	uint8_t link_count;
	uint8_t lowest_dpia_link_index;
};
#include "bios_protos.h"
#include "bios_funcs.c"
static const struct dc_vbios_funcs vbios_funcs = {
	.get_hpd_info = bios_parser_get_hpd_info,
	.get_gpio_pin_info = bios_parser_get_gpio_pin_info,
	.get_i2c_info = bios_parser_get_i2c_info,
	.get_connectors_number = bios_parser_get_connectors_number,
	.get_connector_id = bios_parser_get_connector_id,
	.get_src_obj = bios_parser_get_src_obj,
	.get_device_tag = bios_parser_get_device_tag,
	.is_device_id_supported = bios_parser_is_device_id_supported,
	.get_encoder_cap_info = bios_parser_get_encoder_cap_info,
	.get_disp_connector_caps_info = bios_parser_get_disp_connector_caps_info,
	.get_atom_dc_golden_table = bios_get_atom_dc_golden_table,
};
#include "gpio_link.c"

struct link_encoder_funcs { void (*destroy)(struct link_encoder **enc); };
struct panel_cntl_funcs { void (*destroy)(struct panel_cntl **panel_cntl); };
static const struct link_encoder_funcs dcn21_link_enc_funcs, virtual_lnk_enc_funcs;
static const struct panel_cntl_funcs dce_link_panel_cntl_funcs;
/* The register tables are pointed at, not read, on this path. */
struct dcn10_link_enc_registers { int unused; };
struct dcn10_link_enc_aux_registers { int unused; };
struct dcn10_link_enc_hpd_registers { int unused; };
struct dcn10_link_enc_shift { int unused; };
struct dcn10_link_enc_mask { int unused; };
struct dce_panel_cntl_registers { int unused; };
struct dce_panel_cntl_shift { int unused; };
struct dce_panel_cntl_mask { int unused; };
static const struct dcn10_link_enc_registers link_enc_regs[5];
static const struct dcn10_link_enc_aux_registers link_enc_aux_regs[5];
static const struct dcn10_link_enc_hpd_registers link_enc_hpd_regs[5];
static const struct dcn10_link_enc_shift le_shift;
static const struct dcn10_link_enc_mask le_mask;
static const struct dce_panel_cntl_registers panel_cntl_regs[1];
static const struct dce_panel_cntl_shift panel_cntl_shift;
static const struct dce_panel_cntl_mask panel_cntl_mask;
struct dcn21_link_encoder {
	struct dcn10_link_encoder enc10;
	char phy_seq_cfg[256];
};
static bool construct_phy(struct dc_link *link, const struct link_init_data *init_params);
static bool link_construct(struct dc_link *link, const struct link_init_data *init_params)
{
	return construct_phy(link, init_params);
}
#include "link_protos.h"
#include "link_funcs.c"

/* dal_hw_translate_init / dal_hw_factory_init (hw_translate.c,
 * hw_factory.c): their DCN 2.1 case. */
bool dal_hw_translate_init(struct hw_translate *translate, enum dce_version dce_version,
			   enum dce_environment dce_environment)
{
	if (dce_version != DCN_VERSION_2_1)
		return false;
	dal_hw_translate_dcn21_init(translate);
	return true;
}

bool dal_hw_factory_init(struct hw_factory *factory, enum dce_version dce_version,
			 enum dce_environment dce_environment)
{
	if (dce_version != DCN_VERSION_2_1)
		return false;
	dal_hw_factory_dcn21_init(factory);
	return true;
}

static u32 zero_read(struct card_info *info, u32 reg) { return 0; }
static void no_write(struct card_info *info, u32 reg, u32 val) {}

static void gid(const char *n, struct graphics_object_id g)
{
	printf("%s id %u enum %u type %u\n", n, g.id, g.enum_id, g.type);
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

	static struct dc dc = { .caps = { .is_apu = true } };
	static struct dc_context ctx = { .dc = &dc, .driver_context = &adev, .dce_version = DCN_VERSION_2_1 };
	dc.ctx = &ctx;
	struct bp_init_data init = { .ctx = &ctx, .bios = bios };
	struct bios_parser *bp = calloc(1, sizeof(*bp));
	if (!bios_parser2_construct(bp, &init, DCN_VERSION_2_1))
		return 1;
	ctx.dc_bios = &bp->base;
	ctx.gpio_service = dal_gpio_service_create(DCN_VERSION_2_1, DCE_ENV_PRODUCTION_DRV, &ctx);

	static const struct resource_funcs funcs = {
		.link_enc_create = dcn21_link_encoder_create,
		.panel_cntl_create = dcn21_panel_cntl_create,
	};
	static const struct resource_caps cap = { 0 };
	static struct resource_pool pool = { .funcs = &funcs, .res_cap = &cap };
	static const struct link_service srv = { .create_link = link_create };
	dc.res_pool = &pool;
	dc.link_srv = &srv;

	bool ok = create_links(&dc, 1);
	printf("create_links %d link_count %u internal %u dig %u edp %u lowest_dpia %u\n", ok, dc.link_count,
	       dc.caps.num_of_internal_disp, pool.dig_link_enc_count, ctx.dc_edp_id_count, dc.lowest_dpia_link_index);
	for (int i = 0; i < MAX_DIG_LINK_ENCODERS; i++) {
		int owner = -1;
		for (int l = 0; l < dc.link_count; l++)
			if (pool.link_encoders[i] && dc.links[l]->link_enc == pool.link_encoders[i])
				owner = l;
		printf("link_encoders %d %d\n", i, owner);
	}
	for (int l = 0; l < dc.link_count; l++) {
		struct dc_link *k = dc.links[l];
		printf("link %d index %u ep %d internal %d signal 0x%x", l, k->link_index, k->ep_type,
		       k->is_internal_display, k->connector_signal);
		gid("", k->link_id);
		printf("  irq %d %d %d hpd_gpio %d", k->irq_source_hpd, k->irq_source_hpd_rx, k->irq_source_read_request,
		       k->hpd_gpio != NULL);
		if (k->hpd_gpio)
			printf(" id %d en %u mode %d", k->hpd_gpio->id, k->hpd_gpio->en, k->hpd_gpio->mode);
		printf("\n  ddc %d hw_inst %u hpd_src %d eng %d enc_hw_inst %u panel %d", k->ddc != NULL, k->ddc_hw_inst,
		       k->hpd_src, k->eng_id, k->link_enc_hw_inst, k->panel_cntl != NULL);
		if (k->panel_cntl)
			printf(" inst %u pwrseq %u", k->panel_cntl->inst, k->panel_cntl->pwrseq_inst);
		printf("\n  tag acpi %u type %d enum %u raw %u map %u caps %u drive %u %u psr %d %d\n",
		       k->device_tag.acpi_device, k->device_tag.dev_id.device_type, k->device_tag.dev_id.enum_id,
		       k->device_tag.dev_id.raw_device_tag, k->ddi_channel_mapping.raw, k->chip_caps,
		       k->bios_forced_drive_settings.VOLTAGE_SWING, k->bios_forced_drive_settings.PRE_EMPHASIS,
		       k->psr_settings.psr_vtotal_control_support, k->psr_settings.psr_version);
		struct link_encoder *e = k->link_enc;
		if (e) {
			printf("  enc output 0x%x engine %d transmitter %d hpd %d flags 0x%x color %d clock %u 420 %d %d fec %d",
			       e->output_signals, e->preferred_engine, e->transmitter, e->hpd_source, e->features.flags.raw,
			       e->features.max_hdmi_deep_color, e->features.max_hdmi_pixel_clock,
			       e->features.hdmi_ycbcr420_supported, e->features.dp_ycbcr420_supported,
			       e->features.fec_supported);
			gid(" id", e->id);
			gid("  connector", e->connector);
		}
	}
	return 0;
}
