/* The oracle: Linux's own amdgpu_atom_parse (atom.c, compiled whole) and
 * amdgpu_atomfirmware functions (copied verbatim by extract.py) on a VBIOS
 * image, in the order amdgpu_device_init calls them. The output is what
 * hostcheck/atom_check.rs must print for the same image.
 *
 * struct amdgpu_device carries only the fields those functions touch; the
 * device is bare metal (amdgpu_sriov_vf 0) and CHIP_RENOIR. */
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
/* Only the dGPU branches of get_vram_info read these; Renoir is an APU. */
#define RREG32(r) 0u
#define amdgpu_ip_version(adev, ip, inst) 0u
#include "fw_extracted.c"

static void pll(const char *n, struct amdgpu_pll *p)
{
	printf("%s reference_freq %u reference_div %u post_div %u-%u ref_div %u-%u feedback_div %u-%u best_vco %u\n", n,
	       p->reference_freq, p->reference_div, p->min_post_div, p->max_post_div, p->min_ref_div, p->max_ref_div,
	       p->min_feedback_div, p->max_feedback_div, p->best_vco);
}

static void field(const char *n, const uint8_t *s, int len)
{
	printf("%s '", n);
	for (int i = 0; i < len && s[i]; i++)
		putchar(s[i]);
	printf("'\n");
}

int main(int argc, char **argv)
{
	FILE *f = fopen(argv[1], "rb");
	static unsigned char bios[1 << 20];
	size_t n = fread(bios, 1, sizeof(bios), f);
	fclose(f);
	static struct card_info card;
	static struct amdgpu_device adev = { .asic_type = CHIP_RENOIR, .flags = CHIP_RENOIR | AMD_IS_APU };

	/* amdgpu_atombios_init, is_atom_fw */
	struct atom_context *ctx = amdgpu_atom_parse(&card, bios);
	if (!ctx) {
		printf("parse failed\n");
		return 1;
	}
	adev.mode_info.atom_context = ctx;
	printf("size %zu\n", n);
	printf("cmd_table 0x%x data_table 0x%x version 0x%08x\n", ctx->cmd_table, ctx->data_table, ctx->version);
	field("name", ctx->name, STRLEN_LONG);
	field("vbios_pn", ctx->vbios_pn, STRLEN_LONG);
	field("date", ctx->date, STRLEN_NORMAL);
	field("vbios_ver_str", ctx->vbios_ver_str, STRLEN_NORMAL);
	field("build_num", ctx->build_num, STRLEN_NORMAL);
	for (int i = 0; i < 256; i++)
		if (ctx->iio[i])
			printf("iio[%d] 0x%x\n", i, ctx->iio[i]);
	unsigned nd = (*(uint16_t *)(bios + ctx->data_table) - 4) / 2;
	for (unsigned i = 0; i < nd; i++) {
		uint16_t size = 0, start = 0;
		uint8_t frev = 0, crev = 0;
		if (amdgpu_atom_parse_data_header(ctx, i, &size, &frev, &crev, &start))
			printf("data[%u] at 0x%x size %u rev %u.%u\n", i, start, size, frev, crev);
	}
	unsigned nc = (*(uint16_t *)(bios + ctx->cmd_table) - 4) / 2;
	for (unsigned i = 0; i < nc; i++) {
		uint8_t frev = 0, crev = 0;
		if (amdgpu_atom_parse_cmd_header(ctx, i, &frev, &crev))
			printf("cmd[%u] rev %u.%u\n", i, frev, crev);
	}

	amdgpu_atomfirmware_scratch_regs_init(&adev);
	int r = amdgpu_atomfirmware_allocate_fb_scratch(&adev);
	adev.mode_info.firmware_flags = amdgpu_atomfirmware_query_firmware_capability(&adev);
	printf("bios_scratch_reg_offset 0x%x\n", adev.bios_scratch_reg_offset);
	printf("allocate_fb_scratch %d scratch_size_bytes %d\n", r, ctx->scratch_size_bytes);
	printf("fw_vram_usage start 0x%llx size 0x%llx drv_vram_usage start 0x%llx size 0x%llx\n",
	       (unsigned long long)adev.mman.fw_vram_usage_start_offset, (unsigned long long)adev.mman.fw_vram_usage_size,
	       (unsigned long long)adev.mman.drv_vram_usage_start_offset, (unsigned long long)adev.mman.drv_vram_usage_size);
	printf("firmware_flags 0x%08x\n", adev.mode_info.firmware_flags);

	/* amdgpu_device_init, after amdgpu_device_need_post */
	r = amdgpu_atomfirmware_get_clock_info(&adev);
	printf("get_clock_info %d default_sclk %u default_mclk %u current_sclk %u current_mclk %u\n", r,
	       adev.clock.default_sclk, adev.clock.default_mclk, adev.pm.current_sclk, adev.pm.current_mclk);
	pll("spll", &adev.clock.spll);
	pll("mpll", &adev.clock.mpll);

	/* gmc_v9_0_sw_init */
	int vram_width = 0, vram_type = 0, vram_vendor = 0;
	r = amdgpu_atomfirmware_get_vram_info(&adev, &vram_width, &vram_type, &vram_vendor);
	printf("get_vram_info %d width %d type %d vendor %d\n", r, vram_width, vram_type, vram_vendor);

	/* atom_check.rs runs amdgpu_atombios_init once more, whole */
	amdgpu_atom_parse(&card, bios);
	return 0;
}
