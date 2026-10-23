/* The amdgpu_dm_init oracle: Linux's amdgpu_dm_init up to dc_hardware_init,
 * with its quirk tables, on a device the arguments describe:
 *   asic_type flags apu_flags dmcub_fw_version dce_ip_version
 *   pci_device pci_revision subsystem_vendor subsystem_device
 * dc_create is where the oracle looks: it prints the dc_init_data it is
 * handed and returns a zeroed dc. dm_dmub_hw_init has its own oracle
 * (main_dmubhw.c) and returns 0 here. No DMI strings: dmi_check_system
 * matches nothing, as for a module. Afterwards the dc->debug members
 * amdgpu_dm_init writes are printed.
 * hostcheck/dminit_check.rs prints the same for the port. */
#include <errno.h>
#include "dc.h"
#include "amdgpu_extracted.h"

#define mutex_init(m) ((void)(m))
#define adev_to_drm(adev) NULL
#define drm_err(d, ...) do { } while (0)
#define drm_info(d, ...) do { } while (0)
#define drm_dbg(d, ...) do { } while (0)
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

struct pci_dev { u16 vendor, device, subsystem_vendor, subsystem_device; u8 revision; };
struct mutex { int x; };
struct atom_context { uint8_t *bios; };
struct amdgpu_display_manager {
	void *ddev;
	struct amdgpu_device *adev;
	struct mutex dpia_aux_lock, dc_lock, audio_lock;
	struct dc *dc;
	void *cgs_device;
	uint32_t dmcub_fw_version;
	bool aux_hpd_discon_quirk;
	bool edp0_on_dp1_quirk;
	void *bb_from_dmub;
};
struct amdgpu_device {
	struct amdgpu_display_manager dm;
	struct pci_dev *pdev;
	void *dev;
	uint32_t family, external_rev_id, asic_type, apu_flags, flags;
	struct { uint32_t vram_width; } gmc;
	struct { struct atom_context *atom_context; bool gpu_vm_support; } mode_info;
	struct { bool keep_stolen_vga_memory; } mman;
	uint32_t ip_versions[MAX_HWIP][HWIP_MAX_INSTANCE];
	uint32_t *reg_offset[MAX_HWIP][HWIP_MAX_INSTANCE];
};

uint amdgpu_dc_debug_mask;
uint amdgpu_dc_visual_confirm;

static int amdgpu_dm_irq_init(struct amdgpu_device *adev) { return 0; }
static int dm_dmub_hw_init(struct amdgpu_device *adev) { return 0; }
static void amdgpu_dm_fini(struct amdgpu_device *adev) { }

static void offs(const char *n, const uint32_t *o)
{
	printf("%s", n);
	for (int i = 0; i < 5; i++)
		printf(" 0x%x", o ? o[i] : 0);
	printf("\n");
}

struct dc *dc_create(const struct dc_init_data *d)
{
	const struct dc_config *f = &d->flags;
	printf("asic_id family %u rev %u internal %u chip 0x%x vram_width %u\n", d->asic_id.chip_family,
	       d->asic_id.pci_revision_id, d->asic_id.hw_internal_rev, d->asic_id.chip_id, d->asic_id.vram_width);
	printf("env %d cgs %d virtual_links %d bb_from_dmub %d\n", d->dce_environment, d->cgs_device != NULL,
	       d->num_virtual_links, d->bb_from_dmub != NULL);
	printf("flags disable_dmcu %d gpu_vm %d fbc %d multi_mon %d frac_pwm %d edp_no_pwr %d lttpr %d %d\n",
	       f->disable_dmcu, f->gpu_vm_support, f->fbc_support, f->multi_mon_pp_mclk_switch,
	       f->disable_fractional_pwm, f->edp_no_power_sequencing, f->allow_lttpr_non_transparent_mode.bits.DP1_4A,
	       f->allow_lttpr_non_transparent_mode.bits.DP2_0);
	printf("flags seamless %d %d mipi %d ips %d ips_vpb %d dtb0 %d edp0_dp1 %d\n", f->seamless_boot_edp_requested,
	       f->allow_seamless_boot_optimization, f->enable_mipi_converter_optimization, f->disable_ips,
	       f->disable_ips_in_vpb, f->allow_0_dtb_clk, f->support_edp0_on_dp1);
	offs("dcn", d->dcn_reg_offsets);
	offs("nbio", d->nbio_reg_offsets);
	offs("clk", d->clk_reg_offsets);
	struct dc *dc = calloc(1, sizeof(*dc));
	dc->ctx = calloc(1, sizeof(*dc->ctx));
	return dc;
}

#include "dminit_funcs.c"

int main(int argc, char **argv)
{
	static struct amdgpu_device adev;
	static struct pci_dev pdev = { .vendor = 0x1002 };
	static uint8_t bios[1];
	static struct atom_context atom = { bios };
	static uint32_t dcn[5] = { 0x12, 0xc0, 0x34c0, 0x9000, 0x2403c00 };
	static uint32_t nbio[5] = { 0, 0x14, 0xd20, 0x10400, 0x241b000 };
	static uint32_t clk[5] = { 0x16c00, 0x16e00, 0x17000, 0x17200, 0x1b000 };
	int a = 1;

	adev.asic_type = strtoul(argv[a++], NULL, 0);
	adev.flags = strtoul(argv[a++], NULL, 0);
	adev.apu_flags = strtoul(argv[a++], NULL, 0);
	adev.dm.dmcub_fw_version = strtoul(argv[a++], NULL, 0);
	adev.ip_versions[DCE_HWIP][0] = strtoul(argv[a++], NULL, 0);
	pdev.device = strtoul(argv[a++], NULL, 0);
	pdev.revision = strtoul(argv[a++], NULL, 0);
	pdev.subsystem_vendor = strtoul(argv[a++], NULL, 0);
	pdev.subsystem_device = strtoul(argv[a++], NULL, 0);
	adev.pdev = &pdev;
	adev.family = 142;
	adev.external_rev_id = 0x91;
	adev.gmc.vram_width = 64;
	adev.mode_info.atom_context = &atom;
	adev.dm.cgs_device = &adev;
	adev.reg_offset[DCE_HWIP][0] = dcn;
	adev.reg_offset[NBIO_HWIP][0] = nbio;
	adev.reg_offset[CLK_HWIP][0] = clk;

	int r = amdgpu_dm_init(&adev);
	struct dc_debug_options *g = &adev.dm.dc->debug;
	printf("amdgpu_dm_init %d gpu_vm_support %d quirks %d %d\n", r, adev.mode_info.gpu_vm_support,
	       adev.dm.aux_hpd_discon_quirk, adev.dm.edp0_on_dp1_quirk);
	printf("debug split %d policy %d stutter %d dsc %d clock_gate %d subvp_mclk %d subvp_off %d dml2 %d %d\n",
	       g->force_single_disp_pipe_split, g->pipe_split_policy, g->disable_stutter, g->disable_dsc,
	       g->disable_clock_gate, g->force_subvp_mclk_switch, g->force_disable_subvp, g->using_dml2,
	       g->using_dml21);
	printf("debug hdcp %d %d skip_lt %d visual_confirm %d ignore_cable_id %d\n", g->hdcp_lc_force_fw_enable,
	       g->hdcp_lc_enable_sw_fallback, g->skip_detection_link_training, g->visual_confirm, g->ignore_cable_id);
	return 0;
}
