/* The GPIO oracle: Linux's gpio/ sources (compiled unchanged against
 * shim_dc) on the VBIOS (argv[1]) as construct_phy reaches them. Per
 * connector: link_get_hpd_gpio, opening it in interrupt mode and unlocking
 * it, its IRQ sources, the DDC service; then every translate entry, a
 * second open of a busy pin, and closing the HPD pins. Unwritten registers
 * read hash(reg); every register write is printed. hostcheck/gpio_check.rs
 * prints the same for the port. */
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
struct dc {
	struct dc_context *ctx;
	struct { unsigned int force_bios_fixed_vs; } config;
};
struct dc_link { int unused; };
struct dc_vbios_funcs {
	enum bp_result (*get_hpd_info)(struct dc_bios *dcb, struct graphics_object_id id,
				       struct graphics_object_hpd_info *info);
	enum bp_result (*get_gpio_pin_info)(struct dc_bios *dcb, uint32_t gpio_id, struct gpio_pin_info *info);
	enum bp_result (*get_i2c_info)(struct dc_bios *dcb, struct graphics_object_id id,
				       struct graphics_object_i2c_info *info);
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

#include "bios_protos.h"
#include "bios_funcs.c"
static const struct dc_vbios_funcs vbios_funcs = {
	.get_hpd_info = bios_parser_get_hpd_info,
	.get_gpio_pin_info = bios_parser_get_gpio_pin_info,
	.get_i2c_info = bios_parser_get_i2c_info,
};
#include "gpio_link.c"

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
	if (!bios_parser2_construct(bp, &init, DCN_VERSION_2_1))
		return 1;
	struct dc_bios *dcb = &bp->base;
	ctx.dc_bios = dcb;
	ctx.gpio_service = dal_gpio_service_create(DCN_VERSION_2_1, DCE_ENV_PRODUCTION_DRV, &ctx);
	struct gpio_service *gs = ctx.gpio_service;

	struct gpio *hpd[8] = { 0 };
	static struct dc_link link;
	for (int i = 0; i < 8; i++) {
		struct graphics_object_id c = bios_parser_get_connector_id(dcb, i);
		if (c.id == 0)
			continue;
		printf("connector %d id %u\n", i, c.id);
		hpd[i] = link_get_hpd_gpio(dcb, c, gs);
		if (hpd[i]) {
			printf("  hpd id %d en %u\n", dal_gpio_get_id(hpd[i]), dal_gpio_get_enum(hpd[i]));
			enum gpio_result r = dal_gpio_open(hpd[i], GPIO_MODE_INTERRUPT);
			printf("  open %d\n", r);
			printf("  unlock %d\n", dal_gpio_unlock_pin(hpd[i]));
			printf("  irq %d rx %d rr %d\n", dal_irq_get_source(hpd[i]), dal_irq_get_rx_source(hpd[i]),
			       dal_irq_get_read_request(hpd[i]));
		} else {
			printf("  hpd none\n");
		}
		struct ddc_service_init_data di = { .ctx = &ctx, .id = c, .link = &link };
		struct ddc_service *ddc = link_create_ddc_service(&di);
		printf("  ddc pin %d", ddc->ddc_pin != NULL);
		if (ddc->ddc_pin)
			printf(" line %u clock %u hw %d channel %u", dal_gpio_get_enum(ddc->ddc_pin->pin_data),
			       dal_gpio_get_enum(ddc->ddc_pin->pin_clock), ddc->ddc_pin->hw_info.hw_supported,
			       ddc->ddc_pin->hw_info.ddc_channel);
		printf(" internal %d\n", ddc->flags.IS_INTERNAL_DISPLAY);
	}

	for (int id = GPIO_ID_DDC_DATA; id <= GPIO_ID_GSL; id++)
		for (int en = 0; en < 8; en++) {
			struct gpio_pin_info p = dal_gpio_get_generic_pin_info(gs, id, en);
			printf("pin %d %d: %u %u %u %u %u %u %u %u\n", id, en, p.offset, p.offset_y, p.offset_en,
			       p.offset_mask, p.mask, p.mask_y, p.mask_en, p.mask_mask);
		}

	/* A busy pin refuses a second open; locking and closing free it. */
	for (int i = 0; i < 8; i++) {
		if (!hpd[i])
			continue;
		struct gpio *again = dal_gpio_create_irq(gs, GPIO_ID_HPD, dal_gpio_get_enum(hpd[i]));
		printf("lock %d\n", dal_gpio_lock_pin(hpd[i]));
		printf("busy open %d\n", dal_gpio_open(again, GPIO_MODE_INPUT));
		dal_gpio_close(hpd[i]);
		enum gpio_result r = dal_gpio_open(again, GPIO_MODE_INPUT);
		printf("free open %d mode %d\n", r, dal_gpio_get_mode(again));
		dal_gpio_close(again);
	}
	return 0;
}
