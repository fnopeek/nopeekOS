/* The oracle for stage 1b: Linux's microcode functions, copied verbatim by
 * extract.py, requesting and reading the firmware as the early_init of
 * psp_v12_0, dm, gfx_v9_0, sdma_v4_0 and vcn_v2_0 do on Renoir, from a
 * directory laid out as sys/firmware/ (amdgpu/<name>). The output is what hostcheck/ucode_check.rs
 * must print for the same files.
 *
 * struct amdgpu_device carries only the fields those functions touch; the
 * device is 1002:164c rev 0xc1, subsystem 17aa:3806, CHIP_RENOIR,
 * AMD_APU_IS_RENOIR, bare metal, PSP loading, with the IP versions the
 * card reports. */
#include <linux/sched.h>
#include <linux/slab.h>
#include <stdarg.h>
#include <drm/amd_asic_type.h>
#include "amd_shared.h"
#include "amdgpu_extracted.h"
struct amdgpu_device;
#include "amdgpu_ucode.h"
#include "dal_asic_id.h"

#define dev_err(d, ...) printf("dev_err: " __VA_ARGS__)
#define dev_warn(d, ...) printf("dev_warn: " __VA_ARGS__)
#define dev_dbg(d, ...) ((void)0)
#define drm_info(d, ...) printf("drm_info: " __VA_ARGS__)
#define DRM_DEBUG(...) ((void)0)
#define DRM_WARN_ONCE(...) printf("DRM_WARN_ONCE: " __VA_ARGS__ "\n")
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define PAGE_SIZE 4096
#define ALIGN(x, a) (((x) + (a) - 1) & ~((a) - 1))
#define amdgpu_sriov_vf(adev) 0
#define BUG() abort()
#define kmalloc(n, f) malloc(n)
#define fallthrough
#define AMD_APU_IS_RENOIR 0x00000008UL
#define AMD_APU_IS_RAVEN2 0x00000001UL
#define AMD_APU_IS_PICASSO 0x00000002UL
#define AMDGPU_MAX_RLC_INSTANCES 8
#define AMDGPU_MAX_GC_INSTANCES 8
static int amdgpu_num_kcq = -1;

struct pci_dev { u16 vendor, device, subsystem_vendor, subsystem_device; u8 revision; };
struct psp_bin_desc { uint32_t fw_version, feature_version, size_bytes; uint8_t *start_addr; };
struct ta_context { struct psp_bin_desc bin_desc; };
struct psp_ta { struct ta_context context; };
struct psp_context {
	struct amdgpu_device *adev;
	const struct firmware *asd_fw, *ta_fw;
	struct ta_context asd_context;
	struct psp_ta xgmi_context, ras_context, hdcp_context, dtm_context, rap_context, securedisplay_context;
	uint32_t ta_fw_version;
};
struct amdgpu_rlcg_reg_access_ctrl { u32 scratch_reg0; };
struct amdgpu_rlc {
	bool is_rlc_v2_1;
	u32 save_and_restore_offset, clear_state_descriptor_offset, avail_scratch_ram_locations;
	u32 reg_restore_list_size, reg_list_format_start, reg_list_format_separate_start, starting_offsets_start;
	u32 reg_list_format_size_bytes, reg_list_size_bytes;
	u32 *register_list_format, *register_restore;
	u32 save_restore_list_cntl_size_bytes, save_restore_list_gpm_size_bytes, save_restore_list_srm_size_bytes;
	u8 *save_restore_list_cntl, *save_restore_list_gpm, *save_restore_list_srm;
	u32 reg_list_format_direct_reg_list_length;
	u32 rlc_iram_ucode_size_bytes, rlc_dram_ucode_size_bytes;
	u8 *rlc_iram_ucode, *rlc_dram_ucode;
	u32 rlcp_ucode_size_bytes, rlcv_ucode_size_bytes;
	u8 *rlcp_ucode, *rlcv_ucode;
	u32 global_tap_delays_ucode_size_bytes, se0_tap_delays_ucode_size_bytes, se1_tap_delays_ucode_size_bytes;
	u32 se2_tap_delays_ucode_size_bytes, se3_tap_delays_ucode_size_bytes;
	u8 *global_tap_delays_ucode, *se0_tap_delays_ucode, *se1_tap_delays_ucode, *se2_tap_delays_ucode;
	u8 *se3_tap_delays_ucode;
};
struct amdgpu_gfx {
	const struct firmware *pfp_fw, *me_fw, *ce_fw, *rlc_fw, *mec_fw, *mec2_fw;
	u32 pfp_fw_version, pfp_feature_version, me_fw_version, me_feature_version, ce_fw_version, ce_feature_version;
	u32 rlc_fw_version, rlc_feature_version, mec_fw_version, mec_feature_version, mec2_fw_version, mec2_feature_version;
	u32 rlc_srlc_fw_version, rlc_srlc_feature_version, rlc_srlg_fw_version, rlc_srlg_feature_version;
	u32 rlc_srls_fw_version, rlc_srls_feature_version, rlcp_ucode_version, rlcp_ucode_feature_version;
	u32 rlcv_ucode_version, rlcv_ucode_feature_version;
	bool me_fw_write_wait, mec_fw_write_wait;
	unsigned num_gfx_rings;
	struct amdgpu_rlc rlc;
};
struct amdgpu_sdma_instance { const struct firmware *fw; uint32_t fw_version, feature_version; bool burst_nop; };
struct amdgpu_vcn_inst { struct amdgpu_device *adev; int inst; const struct firmware *fw; };
struct amdgpu_device {
	void *dev;
	int ddev;
	struct pci_dev *pdev;
	enum amd_asic_type asic_type;
	u32 apu_flags, flags, pg_flags, external_rev_id;
	u32 ip_versions[MAX_HWIP][HWIP_MAX_INSTANCE];
	struct amdgpu_firmware firmware;
	struct { u32 pp_feature; } pm;
	struct psp_context psp;
	struct amdgpu_gfx gfx;
	struct { int num_instances, num_inst_per_aid; bool has_page_queue;
		 struct amdgpu_sdma_instance instance[AMDGPU_MAX_SDMA_INSTANCES]; } sdma;
	struct { bool per_inst_fw; struct amdgpu_vcn_inst inst[AMDGPU_MAX_VCN_INSTANCES]; } vcn;
	struct { const struct firmware *dmub_fw; } dm;
};
#include "ucode_glue.h"

static const char *fw_dir;
int request_firmware(const struct firmware **fw, const char *name, void *dev)
{
	char path[512];
	snprintf(path, sizeof(path), "%s/%s", fw_dir, name);
	FILE *f = fopen(path, "rb");
	if (!f) {
		printf("request_firmware: %s missing\n", name);
		*fw = NULL;
		return -ENOENT;
	}
	struct firmware *w = calloc(1, sizeof(*w));
	fseek(f, 0, SEEK_END);
	w->size = ftell(f);
	fseek(f, 0, SEEK_SET);
	u8 *d = malloc(w->size);
	fread(d, 1, w->size, f);
	fclose(f);
	w->data = d;
	*fw = w;
	return 0;
}
#define firmware_request_nowarn request_firmware
void release_firmware(const struct firmware *fw) { }
static bool is_raven_kicker(struct amdgpu_device *adev) { return false; }
int amdgpu_pm_load_smu_firmware(struct amdgpu_device *adev, uint32_t *smu_version) { return -1; }

#include "ucode_extracted.c"

static long off(const struct firmware *fw, const void *p) { return p ? (long)((const u8 *)p - fw->data) : -1; }

int main(int argc, char **argv)
{
	static struct pci_dev pdev = { 0x1002, 0x164c, 0x17aa, 0x3806, 0xc1 };
	static struct amdgpu_device a;
	struct amdgpu_device *adev = &a;
	fw_dir = argv[1];
	a.pdev = &pdev;
	a.asic_type = CHIP_RENOIR;
	a.apu_flags = AMD_APU_IS_RENOIR;
	a.flags = AMD_IS_APU;
	a.external_rev_id = 0x91;
	a.firmware.load_type = AMDGPU_FW_LOAD_PSP;
	a.pm.pp_feature = 0xfff7bfff;
	a.pg_flags = 0x0002c100;
	a.ip_versions[GC_HWIP][0] = IP_VERSION(9, 3, 0);
	a.ip_versions[SDMA0_HWIP][0] = IP_VERSION(4, 1, 2);
	a.ip_versions[MP0_HWIP][0] = IP_VERSION(11, 0, 3);
	a.ip_versions[UVD_HWIP][0] = IP_VERSION(2, 2, 0);
	a.ip_versions[DCE_HWIP][0] = IP_VERSION(2, 1, 0);
	a.sdma.num_instances = 1;
	a.gfx.num_gfx_rings = 1;
	a.psp.adev = adev;

	/* in amdgpu_device_ip_early_init's block order */
	printf("psp_v12_0_init_microcode %d\n", psp_v12_0_init_microcode(&a.psp));
	printf("dm_init_microcode %d\n", dm_init_microcode(adev));
	printf("gfx_v9_0_init_microcode %d\n", gfx_v9_0_init_microcode(adev));
	printf("sdma_v4_0_init_microcode %d\n", sdma_v4_0_init_microcode(adev));
	if (!(amdgpu_ip_version(adev, SDMA0_HWIP, 0) == IP_VERSION(4, 0, 0) && amdgpu_sriov_vf(adev)) &&
	    sdma_v4_0_fw_support_paging_queue(adev))
		a.sdma.has_page_queue = true;
	printf("amdgpu_vcn_early_init %d\n", amdgpu_vcn_early_init(adev, 0));

	struct psp_context *p = &a.psp;
#define D(n, c, img) printf(n " fw_version 0x%x feature 0x%x size %u at %ld\n", (c).fw_version, (c).feature_version, \
	(c).size_bytes, off(img, (c).start_addr))
	D("asd", p->asd_context.bin_desc, p->asd_fw);
	D("xgmi", p->xgmi_context.context.bin_desc, p->ta_fw);
	D("ras", p->ras_context.context.bin_desc, p->ta_fw);
	D("hdcp", p->hdcp_context.context.bin_desc, p->ta_fw);
	D("dtm", p->dtm_context.context.bin_desc, p->ta_fw);
	D("rap", p->rap_context.context.bin_desc, p->ta_fw);
	D("securedisplay", p->securedisplay_context.context.bin_desc, p->ta_fw);
	printf("ta_fw_version 0x%x\n", p->ta_fw_version);
	struct amdgpu_gfx *g = &a.gfx;
	printf("pfp 0x%x/%u me 0x%x/%u ce 0x%x/%u mec 0x%x/%u mec2 0x%x/%u rlc 0x%x/%u\n", g->pfp_fw_version,
	       g->pfp_feature_version, g->me_fw_version, g->me_feature_version, g->ce_fw_version, g->ce_feature_version,
	       g->mec_fw_version, g->mec_feature_version, g->mec2_fw_version, g->mec2_feature_version,
	       g->rlc_fw_version, g->rlc_feature_version);
	printf("srlc 0x%x/%u srlg 0x%x/%u srls 0x%x/%u rlcp 0x%x/%u rlcv 0x%x/%u\n", g->rlc_srlc_fw_version,
	       g->rlc_srlc_feature_version, g->rlc_srlg_fw_version, g->rlc_srlg_feature_version, g->rlc_srls_fw_version,
	       g->rlc_srls_feature_version, g->rlcp_ucode_version, g->rlcp_ucode_feature_version, g->rlcv_ucode_version,
	       g->rlcv_ucode_feature_version);
	struct amdgpu_rlc *r = &g->rlc;
	printf("rlc v2_1 %d sr_offset 0x%x csd 0x%x scratch %u restore_size %u fmt_start %u fmt_sep %u starts %u\n",
	       r->is_rlc_v2_1, r->save_and_restore_offset, r->clear_state_descriptor_offset,
	       r->avail_scratch_ram_locations, r->reg_restore_list_size, r->reg_list_format_start,
	       r->reg_list_format_separate_start, r->starting_offsets_start);
	unsigned n = (r->reg_list_format_size_bytes + r->reg_list_size_bytes) / 4;
	unsigned long sum = 0;
	for (unsigned i = 0; i < n; i++)
		sum = sum * 31 + r->register_list_format[i];
	printf("rlc lists %u+%u words, restore at %ld, hash 0x%lx\n", r->reg_list_format_size_bytes / 4,
	       r->reg_list_size_bytes / 4, (long)(r->register_restore - r->register_list_format), sum);
#define S(name) printf("rlc " #name " size %u at %ld\n", r->name##_size_bytes, off(g->rlc_fw, r->name))
	S(save_restore_list_cntl); S(save_restore_list_gpm); S(save_restore_list_srm);
	S(rlc_iram_ucode); S(rlc_dram_ucode); S(rlcp_ucode); S(rlcv_ucode);
	S(global_tap_delays_ucode); S(se0_tap_delays_ucode); S(se1_tap_delays_ucode); S(se2_tap_delays_ucode);
	S(se3_tap_delays_ucode);
	printf("rlc direct_reg_list_length %u\n", r->reg_list_format_direct_reg_list_length);
	printf("write_wait me %d mec %d pg_flags 0x%x pp_feature 0x%x\n", g->me_fw_write_wait, g->mec_fw_write_wait,
	       a.pg_flags, a.pm.pp_feature);
	for (int i = 0; i < a.sdma.num_instances; i++)
		printf("sdma%d fw_version 0x%x feature %u burst_nop %d\n", i, a.sdma.instance[i].fw_version,
		       a.sdma.instance[i].feature_version, a.sdma.instance[i].burst_nop);
	printf("sdma has_page_queue %d\n", a.sdma.has_page_queue);
	printf("vcn fw %zu dmub fw %zu\n", a.vcn.inst[0].fw ? a.vcn.inst[0].fw->size : 0,
	       a.dm.dmub_fw ? a.dm.dmub_fw->size : 0);
	for (int i = 0; i < AMDGPU_UCODE_ID_MAXIMUM; i++) {
		struct amdgpu_firmware_info *u = &a.firmware.ucode[i];
		if (!u->fw)
			continue;
		const char *src = u->fw == g->pfp_fw ? "pfp" : u->fw == g->me_fw ? "me" : u->fw == g->ce_fw ? "ce" :
			u->fw == g->mec_fw ? "mec" : u->fw == g->rlc_fw ? "rlc" : u->fw == a.sdma.instance[0].fw ? "sdma0" : "?";
		printf("ucode[%d] id %d from %s\n", i, u->ucode_id, src);
	}
	printf("fw_size %u\n", a.firmware.fw_size);
	return 0;
}
