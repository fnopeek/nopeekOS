/* The DMCUB bring-up oracle: Linux's dm_dmub_sw_init and dm_dmub_hw_init
 * with dmub/src compiled unchanged, on the DMCUB image (argv[1]) and the
 * VBIOS (argv[2]). argv[3] picks the DMCUB the card holds: "off" (fused
 * off in CC_DC_PIPE_DIS), "cold" (in soft reset, never answers) or
 * "running" (running; acknowledges GPINT STOP_FW and reports dal_fw and
 * mailbox_rdy once released). argv[4] is "psp" or "direct" for the load
 * type; argv[5..7] are fb_start, vram_base_offset and the GPU address of
 * the DMUB buffer. Unwritten registers read hash(reg). Every register
 * write is printed, then the DMUB state and a checksum of each window.
 * hostcheck/dmubhw_check.rs prints the same for the port. */
#include <errno.h>
#include "dmub_srv.h"
#include "src/dmub_dcn21.h"
#include "dal_asic_id.h"
#include "dmubhw_types.h"

void udelay(unsigned long us) { }

#define PAGE_SIZE 4096
#define ALIGN(x, a) (((x) + (a) - 1) / (a) * (a))
#define IP_VERSION(mj, mn, rv) (((mj) << 16) | ((mn) << 8) | (rv))
#define DCE_HWIP 0
#define amdgpu_ip_version(adev, ip, inst) IP_VERSION(2, 1, 0)
#define adev_to_drm(adev) NULL
#define drm_err(d, ...) do { } while (0)
#define drm_warn(d, ...) do { } while (0)
#define drm_info(d, ...) do { } while (0)
/* Renoir takes the default case, which returns NULL; the DCN 4.0.1 case
 * needs dml2. */
#define dm_dmub_get_vbios_bounding_box(adev) NULL

/* The register file. */
#define NREGS 4096
static u32 reg_addr[NREGS], reg_val[NREGS];
static int nregs;
static int running;

static int find(u32 reg)
{
	for (int i = 0; i < nregs; i++)
		if (reg_addr[i] == reg)
			return i;
	return -1;
}

static u32 rd(u32 reg)
{
	int i = find(reg);
	return i < 0 ? (reg * 0x9e3779b1u) ^ 0xa5a5a5a5u : reg_val[i];
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

#define R (&dmub_srv_dcn21_regs)

static void wr(u32 reg, u32 val)
{
	printf("write reg 0x%x = 0x%x\n", reg, val);
	set(reg, val);
	if (!running)
		return;
	if (reg == R->offset.DMCUB_GPINT_DATAIN1 && (val >> 28)) {
		set(reg, val & 0x0fffffff);
		if (((val >> 16) & 0xfff) == DMUB_GPINT__STOP_FW)
			set(R->offset.DMCUB_SCRATCH7, DMUB_GPINT__STOP_FW_RESPONSE);
	}
	if (reg == R->offset.DMCUB_CNTL && (val & R->mask.DMCUB_CNTL__DMCUB_ENABLE) &&
	    !(val & R->mask.DMCUB_CNTL__DMCUB_SOFT_RESET))
		set(R->offset.DMCUB_SCRATCH0, 3);
}

#define dm_read_reg(ctx, address) rd(address)
#define dm_write_reg(ctx, address, value) wr(address, value)

struct dmcu;
struct dmcu_funcs {
	void (*dmcu_init)(struct dmcu *dmcu);
	bool (*is_dmcu_initialized)(struct dmcu *dmcu);
};
struct dmcu { const struct dmcu_funcs *funcs; uint32_t psp_version; };
struct abm { bool dmcu_is_running; };
struct resource_pool { struct dmcu *dmcu; struct abm *abm; };
struct dc {
	struct dc_context *ctx;
	struct resource_pool *res_pool;
	struct {
		bool sanity_checks;
		union { struct { uint32_t disable_dpia : 1; } bits; uint32_t raw; } dpia_debug;
	} debug;
};
struct amdgpu_bo;
struct amdgpu_firmware_info { enum AMDGPU_UCODE_ID ucode_id; const struct firmware *fw; };
struct amdgpu_device {
	struct {
		struct dmub_srv *dmub_srv;
		struct dmub_srv_fb_info *dmub_fb_info;
		const struct firmware *dmub_fw;
		struct dc *dc;
		uint32_t dmcub_fw_version;
		struct amdgpu_bo *dmub_bo;
		uint64_t dmub_bo_gpu_addr;
		void *dmub_bo_cpu_addr;
		void *bb_from_dmub;
	} dm;
	struct {
		enum amdgpu_firmware_load_type load_type;
		struct amdgpu_firmware_info ucode[AMDGPU_UCODE_ID_MAXIMUM];
		uint32_t fw_size;
	} firmware;
	uint8_t *bios;
	uint32_t bios_size;
	struct { uint64_t fb_start; } gmc;
	struct { uint64_t vram_base_offset; } vm_manager;
	uint32_t external_rev_id;
};

static uint64_t bo_gpu;
static uint8_t *bo_cpu;
static int amdgpu_bo_create_kernel(struct amdgpu_device *adev, unsigned long size, int align, u32 domain,
				   struct amdgpu_bo **bo, u64 *gpu, void **cpu)
{
	bo_cpu = calloc(1, size);
	*bo = (void *)bo_cpu;
	*gpu = bo_gpu;
	*cpu = bo_cpu;
	return 0;
}

#include "dmubhw_funcs.c"

static uint8_t *load(const char *path, long *size)
{
	FILE *f = fopen(path, "rb");
	fseek(f, 0, SEEK_END);
	*size = ftell(f);
	fseek(f, 0, SEEK_SET);
	uint8_t *d = malloc(*size);
	fread(d, 1, *size, f);
	fclose(f);
	return d;
}

static uint32_t fnv(const uint8_t *p, uint32_t n)
{
	uint32_t h = 0x811c9dc5u;
	for (uint32_t i = 0; i < n; i++)
		h = (h ^ p[i]) * 0x01000193u;
	return h;
}

static unsigned long off(const void *p)
{
	return p ? (unsigned long)((const uint8_t *)p - bo_cpu) : 0;
}

static void rb(const char *n, const struct dmub_rb *r)
{
	printf("%s base 0x%lx capacity 0x%x rptr %u wrpt %u\n", n, off(r->base_address), r->capacity, r->rptr,
	       r->wrpt);
}

int main(int argc, char **argv)
{
	long fw_size, bios_size;
	static struct firmware fw;
	static struct amdgpu_device adev;
	static struct dc_context ctx;
	static struct resource_pool pool;
	static struct dc dc = { .ctx = &ctx, .res_pool = &pool };

	fw.data = load(argv[1], &fw_size);
	fw.size = fw_size;
	adev.bios = load(argv[2], &bios_size);
	adev.bios_size = bios_size;
	adev.dm.dmub_fw = &fw;
	adev.firmware.load_type = strcmp(argv[4], "psp") ? AMDGPU_FW_LOAD_DIRECT : AMDGPU_FW_LOAD_PSP;
	adev.gmc.fb_start = strtoull(argv[5], NULL, 0);
	adev.vm_manager.vram_base_offset = strtoull(argv[6], NULL, 0);
	bo_gpu = strtoull(argv[7], NULL, 0);

	if (!strcmp(argv[3], "off")) {
		set(R->offset.CC_DC_PIPE_DIS, 0);
	} else {
		set(R->offset.CC_DC_PIPE_DIS, R->mask.CC_DC_PIPE_DIS__DC_DMCUB_ENABLE);
		if (!strcmp(argv[3], "running")) {
			set(R->offset.DMCUB_CNTL, R->mask.DMCUB_CNTL__DMCUB_ENABLE);
			running = 1;
		} else {
			set(R->offset.DMCUB_CNTL, R->mask.DMCUB_CNTL__DMCUB_SOFT_RESET);
		}
	}

	int r = dm_dmub_sw_init(&adev);
	printf("sw_init %d dmcub_fw_version 0x%08x fw_version 0x%08x\n", r, adev.dm.dmcub_fw_version,
	       adev.dm.dmub_srv->fw_version);

	adev.dm.dc = &dc;
	r = dm_dmub_hw_init(&adev);
	struct dmub_srv *d = adev.dm.dmub_srv;
	printf("hw_init %d hw_init %d dc_dmub_srv %d\n", r, d->hw_init, ctx.dmub_srv != NULL);
	printf("fb_base 0x%llx fb_offset 0x%llx psp_version %u power_state %d inbox1_last_wptr %u\n",
	       (unsigned long long)d->fb_base, (unsigned long long)d->fb_offset, d->psp_version, d->power_state,
	       d->inbox1_last_wptr);
	rb("inbox1", &d->inbox1.rb);
	rb("outbox1", &d->outbox1_rb);
	rb("outbox0", &d->outbox0_rb);
	printf("fw_state 0x%lx shared_state 0x%lx\n", off((const void *)d->fw_state), off((const void *)d->shared_state));
	printf("scratch_mem gpu 0x%llx size 0x%x ib_mem gpu 0x%llx size 0x%x\n",
	       (unsigned long long)d->scratch_mem_fb.gpu_addr, d->scratch_mem_fb.size,
	       (unsigned long long)d->ib_mem_gart.gpu_addr, d->ib_mem_gart.size);
	struct dmub_srv_fb_info *fb = adev.dm.dmub_fb_info;
	for (int i = 0; i < DMUB_WINDOW_TOTAL; i++)
		printf("window %d gpu 0x%llx size 0x%x sum 0x%08x\n", i, (unsigned long long)fb->fb[i].gpu_addr,
		       fb->fb[i].size, fnv(fb->fb[i].cpu_addr, fb->fb[i].size));
	return 0;
}
