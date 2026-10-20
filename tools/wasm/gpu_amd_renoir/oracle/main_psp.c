/* The PSP oracle: Linux's command path of psp_hw_init, from the TMR setup
 * to the TA loads, against a PSP that signals every fence at once. The
 * buffers' GPU addresses, the TMR and the ucode table come in on stdin as
 * hostcheck/psp_check.rs prints them from the port's run; every KM ring
 * frame is printed with the command it names, as the port's fake PSP
 * prints them.
 *
 * struct amdgpu_device and struct psp_context carry only the fields those
 * functions touch; the device is bare metal, CHIP_RENOIR, MP0 11.0.3,
 * with display hardware, not in reset, suspend or runtime PM. */
#include <linux/sched.h>
#include <linux/slab.h>
#include <drm/amd_asic_type.h>
#include "amd_shared.h"
#include "amdgpu_extracted.h"
struct amdgpu_device;
#include "amdgpu_ucode.h"
#include "psp_gfx_if.h"
#include "ta_secureDisplay_if.h"
#include "soc15_hw_ip.h"

#define dev_err(d, ...) fprintf(stderr, __VA_ARGS__)
#define dev_warn(d, ...) fprintf(stderr, __VA_ARGS__)
#define dev_info(d, ...) fprintf(stderr, __VA_ARGS__)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_in_reset(adev) 0
#define amdgpu_ras_intr_triggered() 0
#define usleep_range(a, b) ((void)0)
#define amdgpu_device_invalidate_hdp(adev, ring) ((void)0)
#define amdgpu_device_flush_hdp(adev, ring) ((void)0)
#define amdgpu_virt_xgmi_migrate_enabled(adev) 0
#define amdgpu_virt_fw_load_skip_check(adev, id) 0
#define amdgpu_ucode_name(id) "ucode"
#define psp_gfx_cmd_name(id) "cmd"
#define drm_dev_enter(d, idx) 1
#define drm_dev_exit(idx) ((void)0)
#define adev_to_drm(adev) 0
#define mutex_init(m) ((void)0)
#define mutex_destroy(m) ((void)0)
#define dev_dbg(d, ...) ((void)0)
#define atomic_inc_return(a) (++(a)->counter)
#define atomic_dec(a) (--(a)->counter)
#define lower_32_bits(n) ((u32)(n))
#define upper_32_bits(n) ((u32)((n) >> 32))
#define IP_VERSION(mj, mn, rv) (((mj) << 16) | ((mn) << 8) | (rv))
#define AMDGPU_RUNPM_BACO 1
#define AMDGPU_RUNPM_BAMACO 3
#define PP_MP1_STATE_UNLOAD 0
#define PAGE_SIZE 4096
#define amdgpu_bo_fb_aper_addr(bo) 0
typedef struct { int counter; } atomic_t;
#include "psp_types.c"

struct amdgpu_bo { u64 size, pa; };
struct psp_context;
struct psp_funcs {
	uint32_t (*ring_get_wptr)(struct psp_context *psp);
	void (*ring_set_wptr)(struct psp_context *psp, uint32_t value);
	bool (*smu_reload_quirk)(struct psp_context *psp);
	int (*rlc_autoload_start)(struct psp_context *psp);
};
struct psp_ras_context { struct ta_context context; struct amdgpu_ras *ras; struct mutex mutex; };
struct psp_context {
	struct amdgpu_device *adev;
	struct psp_ring km_ring;
	struct psp_gfx_cmd_resp *cmd;
	const struct psp_funcs *funcs;
	struct amdgpu_bo *fw_pri_bo;
	uint64_t fw_pri_mc_addr;
	void *fw_pri_buf;
	struct psp_bin_desc sos, toc;
	struct amdgpu_bo *tmr_bo;
	uint64_t tmr_mc_addr;
	uint64_t fence_buf_mc_addr;
	void *fence_buf;
	uint64_t cmd_buf_mc_addr;
	struct psp_gfx_cmd_resp *cmd_buf_mem;
	atomic_t fence_value;
	bool autoload_supported, boot_time_tmr, pmfw_centralized_cstate_management;
	struct ta_context asd_context;
	struct psp_ras_context ras_context;
	struct ta_cp_context hdcp_context, dtm_context, rap_context, securedisplay_context;
	struct mutex mutex;
	const struct firmware *ta_fw;
};
struct amdgpu_device {
	enum amd_asic_type asic_type;
	u32 flags, harvest_ip_mask;
	bool no_hw_access, in_runpm;
	int psp_timeout, ras_enabled;
	struct { int rpm_mode; } pm;
	struct amdgpu_firmware firmware;
	struct psp_context psp;
	struct { u32 autoload_ucode_id; } virt;
};
static u32 amdgpu_ip_version(struct amdgpu_device *adev, int ip, int inst)
{
	return ip == MP0_HWIP ? IP_VERSION(11, 0, 3) : IP_VERSION(2, 1, 0);
}
static int amdgpu_dpm_set_mp1_state(struct amdgpu_device *adev, int state) { return 0; }
static bool amdgpu_device_has_display_hardware(struct amdgpu_device *adev) { return true; }
static u64 amdgpu_bo_size(struct amdgpu_bo *bo) { return bo->size; }
static u64 amdgpu_gmc_vram_pa(struct amdgpu_device *adev, struct amdgpu_bo *bo) { return bo->pa; }
/* A debug print of the header; nothing on the ring. */
static void psp_print_fw_hdr(struct psp_context *psp, struct amdgpu_firmware_info *ucode) { }
static int psp_rlc_autoload_start(struct psp_context *psp) { return 0; }

/* The TA shared buffers, in the order the TAs are initialized. */
static u64 shared_mc[8];
static int shared_next;
static int amdgpu_bo_create_kernel(struct amdgpu_device *adev, unsigned long size, int align, u32 domain,
				   struct amdgpu_bo **bo, u64 *gpu, void **cpu)
{
	static struct amdgpu_bo bos[8];
	*bo = &bos[shared_next];
	*gpu = shared_mc[shared_next++];
	if (cpu)
		*cpu = calloc(1, size);
	return 0;
}
static void amdgpu_bo_free_kernel(struct amdgpu_bo **bo, u64 *gpu, void **cpu) { *bo = NULL; }

static u32 wptr;
static uint32_t get_wptr(struct psp_context *psp) { return wptr; }
/* The PSP: the frame before the new write pointer, the command it names,
 * and its fence signalled. */
static void set_wptr(struct psp_context *psp, uint32_t value)
{
	static int n;
	u32 size_dw = psp->km_ring.ring_size / 4;
	u32 *f = (u32 *)psp->km_ring.ring_mem + (value + size_dw - 16) % size_dw;
	u32 *c = (u32 *)psp->cmd_buf_mem;
	printf("frame %d:", n);
	for (int i = 0; i < 16; i++)
		printf(" %08x", f[i]);
	printf("\ncmd %d:", n++);
	for (int i = 0; i < 0x100; i++)
		if (c[i])
			printf(" %x=%08x", i, c[i]);
	printf("\n");
	*(u32 *)psp->fence_buf = ((struct psp_gfx_rb_frame *)f)->fence_value;
	wptr = value;
}
static const struct psp_funcs funcs = { .ring_get_wptr = get_wptr, .ring_set_wptr = set_wptr };

#include "psp_funcs.c"

static void ta(struct ta_context *c, const char *name)
{
	char w[32];
	unsigned size;
	unsigned long long mc;
	if (scanf("%31s %u %llx", w, &size, &mc) != 3 || strcmp(w, name))
		abort();
	c->bin_desc.size_bytes = size;
	c->bin_desc.start_addr = size ? calloc(1, size) : NULL;
	if (size && strcmp(name, "asd"))
		shared_mc[shared_next++] = mc;
}

int main(void)
{
	static struct amdgpu_device a = { .asic_type = CHIP_RENOIR, .flags = AMD_IS_APU };
	static struct amdgpu_bo tmr;
	struct psp_context *psp = &a.psp;
	unsigned long long fence, cmd, fw_pri, ring, tmr_mc, tmr_size, tmr_pa;
	int timeout, n;

	if (scanf(" inputs fence %llx cmd %llx fw_pri %llx ring %llx tmr %llx %llx %llx psp_timeout %d ucodes %d",
		  &fence, &cmd, &fw_pri, &ring, &tmr_mc, &tmr_size, &tmr_pa, &timeout, &n) != 9)
		return 1;
	a.psp_timeout = timeout;
	a.firmware.load_type = AMDGPU_FW_LOAD_PSP;
	a.firmware.max_ucodes = AMDGPU_UCODE_ID_MAXIMUM;
	for (int k = 0; k < n; k++) {
		unsigned i, id, size;
		unsigned long long mc;
		if (scanf(" ucode %u %u %llx %u", &i, &id, &mc, &size) != 4)
			return 1;
		a.firmware.ucode[i].ucode_id = id;
		a.firmware.ucode[i].fw = (const struct firmware *)&a;
		a.firmware.ucode[i].mc_addr = mc;
		a.firmware.ucode[i].ucode_size = size;
	}
	psp->adev = &a;
	psp->funcs = &funcs;
	psp->cmd = calloc(1, sizeof(struct psp_gfx_cmd_resp));
	psp->fence_buf_mc_addr = fence;
	psp->fence_buf = calloc(1, PSP_FENCE_BUFFER_SIZE);
	psp->cmd_buf_mc_addr = cmd;
	psp->cmd_buf_mem = calloc(1, PSP_CMD_BUFFER_SIZE);
	psp->fw_pri_mc_addr = fw_pri;
	psp->fw_pri_buf = calloc(1, PSP_1_MEG);
	psp->km_ring.ring_type = PSP_RING_TYPE__KM;
	psp->km_ring.ring_size = 0x1000;
	psp->km_ring.ring_mem_mc_addr = ring;
	psp->km_ring.ring_mem = calloc(1, 0x1000);
	tmr.size = tmr_size;
	tmr.pa = tmr_pa;
	psp->tmr_bo = &tmr;
	psp->tmr_mc_addr = tmr_mc;
	shared_next = 0;
	ta(&psp->asd_context, "asd");
	ta(&psp->hdcp_context.context, "hdcp");
	ta(&psp->dtm_context.context, "dtm");
	ta(&psp->securedisplay_context.context, "securedisplay");
	shared_next = 0;

	/* psp_load_fw after psp_ring_init: psp_hw_start's TMR load, the IP
	 * firmware, ASD, and the TAs Renoir's TA image carries. */
	printf("--- psp\n");
	int r = psp_tmr_load(psp);
	if (!r)
		r = psp_load_non_psp_fw(psp);
	if (!r)
		r = psp_asd_initialize(psp);
	if (!r) {
		psp_hdcp_initialize(psp);
		psp_dtm_initialize(psp);
		psp_securedisplay_initialize(psp);
	}
	printf("ret %d\n", r);

	/* psp_hw_fini: the TAs Renoir has, ASD, the TMR. psp_ring_destroy
	 * writes the ring stop to C2PMSG_64, not a ring command. */
	psp->ta_fw = (const struct firmware *)&a;
	psp_ras_terminate(psp);
	psp_securedisplay_terminate(psp);
	psp_rap_terminate(psp);
	psp_dtm_terminate(psp);
	psp_hdcp_terminate(psp);
	psp_asd_terminate(psp);
	psp_tmr_terminate(psp);
	return 0;
}
