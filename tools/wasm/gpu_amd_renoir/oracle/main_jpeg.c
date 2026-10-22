/* The oracle for stage 6's JPEG block: Linux's jpeg_v2_0 and amdgpu_jpeg
 * functions, with the ring helpers they call, copied verbatim by
 * extract.py. It runs the block's life as the port's run has it: hw_init
 * (its ring test starts the block through begin_use), the idle work that
 * gates it, a use that starts it again, and hw_fini. Then
 * amdgpu_jpeg_dec_parse_cs judges every command stream the check hands
 * it: what a guest's JPEG IB may contain.
 *
 * Registers are a file in which a register never written reads SEED(reg)
 * by the rule argv[1] names (ones, zeros or hash). Every register write is
 * printed, and every doorbell with the ring dwords it hands over. Behind
 * them runs what the polls wait for: the power FSM reports the configured
 * state, the engine copies its internal pitch register out, the ring's
 * read pointer follows. The rings, the register bases, the block's state
 * and the parser's cases come on stdin from jpeg4_check.rs, which runs the
 * port on the same file and prints the same lines. The SMU's JPEG power
 * gating is the SMU oracle's. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>
#include <stddef.h>
#include <errno.h>
typedef int atomic_t;
#include "soc15_hw_ip.h"
#include "amd_shared.h"
#include "vcn/vcn_2_0_0_offset.h"
#include "vcn/vcn_2_0_0_sh_mask.h"
#include "nbio/nbio_7_0_offset.h"
#include "nbio/nbio_7_0_sh_mask.h"
#include "vcn/irqsrcs_vcn_2_0.h"
#include "gfx_types.h"
#include "soc15d.h"
#undef __BIG_ENDIAN

#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define lower_32_bits(n) ((u32)((n) & 0xffffffff))
#define upper_32_bits(n) ((u32)(((n) >> 16) >> 16))
#define WARN_ON(c) (c)
#define WARN_ON_ONCE(c) (c)
#define mb() ((void)0)
#define READ_ONCE(x) (x)
#define memset32(p, v, n) do { for (u32 i_ = 0; i_ < (n); i_++) (p)[i_] = (v); } while (0)
#define udelay(n) ((void)0)
#define container_of(p, t, m) ((t *)((char *)(p) - offsetof(t, m)))
#define atomic_inc(a) ((*(a))++)
#define atomic_dec(a) ((*(a))--)
#define atomic_read(a) (*(a))
#define mutex_lock(m) ((void)(m))
#define mutex_unlock(m) ((void)(m))
/* Messages are the port's log lines, not part of the comparison. */
#define DRM_ERROR(...) ((void)0)
#define DRM_DEBUG(...) ((void)0)
#define DRM_DEV_ERROR(d, ...) ((void)0)
#define DRM_DEV_DEBUG(d, ...) ((void)0)
#define dev_err(d, ...) ((void)0)
#define dev_warn(d, ...) ((void)0)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#define amdgpu_sriov_wreg(adev, reg, value, flag, hwip, inst) ((void)0)
#define IP_VERSION(mj, mn, rv) (((mj) << 16) | ((mn) << 8) | (rv))
#define AMDGPU_REGS_RLC (1 << 2)
#define AMDGPU_REGS_NO_KIQ (1 << 1)
#define JPEG_IDLE_TIMEOUT 1000

#include "jpeg_extracted.h"

struct amdgpu_device;
struct amdgpu_ring;
struct amdgpu_job;
struct amdgpu_ip_block_version;
struct amdgpu_ib {
	u32 *ptr;
	u32 length_dw;
};
struct amdgpu_cs_parser {
	struct amdgpu_device *adev;
};
#include "jpeg_v2_0.h"

struct amdgpu_ring_funcs {
	enum amdgpu_ring_type type;
	u32 align_mask;
	u32 nop;
	u64 (*get_rptr)(struct amdgpu_ring *ring);
	u64 (*get_wptr)(struct amdgpu_ring *ring);
	void (*set_wptr)(struct amdgpu_ring *ring);
	void (*insert_nop)(struct amdgpu_ring *ring, uint32_t count);
	int (*test_ring)(struct amdgpu_ring *ring);
	void (*begin_use)(struct amdgpu_ring *ring);
	void (*end_use)(struct amdgpu_ring *ring);
};
struct amdgpu_ring {
	struct amdgpu_device *adev;
	const struct amdgpu_ring_funcs *funcs;
	u32 *ring;
	u64 wptr, wptr_old;
	int count_dw;
	u32 buf_mask, ring_size, max_dw;
	u64 ptr_mask;
	u32 me, pipe;
	bool use_doorbell;
	u32 doorbell_index;
	u64 gpu_addr;
	u64 *wptr_cpu_addr;
	struct { bool ready; } sched;
	struct { u32 last_seq, sync_seq; } fence_drv;
	char name[16];
	/* The wptr the last doorbell handed over, for the dump. */
	u64 rung;
};
struct work_struct { int dummy; };
struct delayed_work { struct work_struct work; };
struct amdgpu_jpeg_inst {
	struct amdgpu_ring ring_dec[AMDGPU_MAX_JPEG_RINGS];
	struct amdgpu_jpeg_reg external;
};
struct amdgpu_nbio_funcs {
	void (*vcn_doorbell_range)(struct amdgpu_device *adev, bool use_doorbell, int doorbell_index, int instance);
};
struct amdgpu_device {
	void *dev;
	struct {
		int (*logical_to_dev_inst)(struct amdgpu_device *adev, int ip, int inst);
		u32 (*logical_to_dev_mask)(struct amdgpu_device *adev, int ip, u32 mask);
	} ip_map;
	u32 cg_flags, pg_flags, usec_timeout;
	u32 *reg_offset[MAX_HWIP][HWIP_MAX_INSTANCE];
	struct {
		u8 num_jpeg_inst, num_jpeg_rings;
		struct amdgpu_jpeg_inst inst[1];
		struct amdgpu_jpeg_reg internal;
		unsigned harvest_config;
		struct delayed_work idle_work;
		enum amd_powergating_state cur_state;
		int jpeg_pg_lock;
		atomic_t total_submission_cnt;
	} jpeg;
	struct { struct { u32 vcn_ring0_1; } vcn; } doorbell_index;
	struct { struct { u32 gb_addr_config; } config; struct { void *funcs; bool rlcg_reg_access_supported; } rlc; } gfx;
	struct { bool dpm_enabled; } pm;
	struct { const struct amdgpu_nbio_funcs *funcs; } nbio;
};
struct amdgpu_ip_block { struct amdgpu_device *adev; };

static char seed_rule;
#define SEED(reg) (seed_rule == 'o' ? 0xffffffffu : seed_rule == 'z' ? 0u : \
		   (((u32)(reg) * 0x9e3779b1u) ^ 0xa5a5a5a5u))
static struct { u32 reg, v; } file[8192];
static int nfile;

static u32 file_get(u32 reg)
{
	for (int i = 0; i < nfile; i++)
		if (file[i].reg == reg)
			return file[i].v;
	return SEED(reg);
}

static void file_set(u32 reg, u32 v)
{
	int i;

	for (i = 0; i < nfile; i++)
		if (file[i].reg == reg)
			break;
	if (i == nfile) {
		if (nfile == 8192)
			abort();
		nfile++;
	}
	file[i].reg = reg;
	file[i].v = v;
}

static struct amdgpu_device a;
static u32 uvd_bases[8];

static u32 uvd(u32 idx, u32 reg) { return uvd_bases[idx] + reg; }

/* The power FSM's status: per two-bit tile field, a request to power on
 * (1) reads on (0), one to power off (2) reads off. */
static void jpeg_write(u32 reg, u32 v)
{
	if (reg == uvd(mmUVD_PGFSM_CONFIG_BASE_IDX, mmUVD_PGFSM_CONFIG)) {
		u32 st = 0;

		for (int f = 0; f < 16; f++)
			if (((v >> (2 * f)) & 3) == 2)
				st |= 2u << (2 * f);
		file_set(uvd(mmUVD_PGFSM_STATUS_BASE_IDX, mmUVD_PGFSM_STATUS), st);
	} else if (reg == uvd(mmUVD_JRBC_RB_RPTR_BASE_IDX, mmUVD_JRBC_RB_RPTR)) {
		a.jpeg.inst[0].ring_dec[0].rung = v;
	}
}

#define RREG32(r) file_get(r)
#define WREG32(r, v) (printf("write reg 0x%x = 0x%x\n", (u32)(r), (u32)(v)), file_set(r, v), jpeg_write(r, v))
#define REG_FIELD_SHIFT(reg, field) reg##__##field##__SHIFT
#define REG_FIELD_MASK(reg, field) reg##__##field##_MASK
#define REG_SET_FIELD(orig_val, reg, field, field_val) \
	(((orig_val) & ~REG_FIELD_MASK(reg, field)) | (REG_FIELD_MASK(reg, field) & ((field_val) << REG_FIELD_SHIFT(reg, field))))
#define WREG32_P(reg, val, mask) do { u32 tmp_ = RREG32(reg); tmp_ &= (mask); tmp_ |= ((val) & ~(mask)); WREG32(reg, tmp_); } while (0)
#include "soc15_common.h"

/* The engine behind the doorbell: its internal pitch register shows
 * outside, the read pointer follows. */
static void jpeg_run(struct amdgpu_ring *ring, u64 from, u64 to)
{
	for (u64 at = from; at != to; at += 2) {
		u32 hdr = ring->ring[at & ring->buf_mask];
		u32 v = ring->ring[(at + 1) & ring->buf_mask];
		if ((hdr >> 28) == PACKETJ_TYPE0 && (hdr & 0x3ffff) == mmUVD_JPEG_PITCH_INTERNAL_OFFSET)
			file_set(uvd(mmUVD_JPEG_PITCH_BASE_IDX, mmUVD_JPEG_PITCH), v);
	}
	file_set(uvd(mmUVD_JRBC_RB_WPTR_BASE_IDX, mmUVD_JRBC_RB_WPTR), to);
	file_set(uvd(mmUVD_JRBC_RB_RPTR_BASE_IDX, mmUVD_JRBC_RB_RPTR), to);
}

/* The doorbell takes the wptr in dwords. */
static void amdgpu_mm_wdoorbell(struct amdgpu_device *adev, u32 index, u32 v)
{
	struct amdgpu_ring *ring = &a.jpeg.inst[0].ring_dec[0];

	if (!ring->use_doorbell || ring->doorbell_index != index) {
		printf("doorbell 0x%x = 0x%x: no ring\n", index, v);
		return;
	}
	printf("doorbell 0x%x = 0x%x:", index, v);
	for (u64 at = ring->rung; at != v; at++)
		printf(" %08x", ring->ring[at & ring->buf_mask]);
	printf("\n");
	jpeg_run(ring, ring->rung, v);
	ring->rung = v;
}
#define WDOORBELL32(index, v) amdgpu_mm_wdoorbell(adev, (index), (v))

static int amdgpu_ring_test_ring(struct amdgpu_ring *ring) { return ring->funcs->test_ring(ring); }
static void amdgpu_ring_set_wptr(struct amdgpu_ring *ring) { ring->funcs->set_wptr(ring); }

/* The SMU's JPEG power gating is the SMU oracle's. */
static void amdgpu_dpm_enable_jpeg(struct amdgpu_device *adev, bool enable) {}
#define cancel_delayed_work_sync(w) ((void)(w))
#define schedule_delayed_work(w, t) ((void)(w))
static struct amdgpu_ip_block blk;
static int jpeg_v2_0_set_powergating_state(struct amdgpu_ip_block *ip_block, enum amd_powergating_state state);
/* JPEG is the one block of its type. */
static int amdgpu_device_ip_set_powergating_state(struct amdgpu_device *adev, int type,
						  enum amd_powergating_state state)
{
	return jpeg_v2_0_set_powergating_state(&blk, state);
}

#include "jpeg_protos.h"
#include "jpeg_extracted.c"

static const struct amdgpu_ring_funcs funcs_dec = {
	.type = AMDGPU_RING_TYPE_VCN_JPEG, .align_mask = 0xf, .nop = 0,
	.get_rptr = jpeg_v2_0_dec_ring_get_rptr, .get_wptr = jpeg_v2_0_dec_ring_get_wptr,
	.set_wptr = jpeg_v2_0_dec_ring_set_wptr, .insert_nop = jpeg_v2_0_dec_ring_nop,
	.test_ring = amdgpu_jpeg_dec_ring_test_ring,
	.begin_use = amdgpu_jpeg_ring_begin_use, .end_use = amdgpu_jpeg_ring_end_use,
};
static const struct amdgpu_nbio_funcs nbio_funcs = { .vcn_doorbell_range = nbio_v7_0_vcn_doorbell_range };

static u32 bases[MAX_HWIP][8];
static u64 wptr_shadow;

static void print_state(void)
{
	struct amdgpu_ring *ring = &a.jpeg.inst[0].ring_dec[0];

	printf("ready %s %d wptr 0x%llx\n", ring->name, ring->sched.ready, (unsigned long long)ring->wptr);
	printf("state %u submissions %d\n", a.jpeg.cur_state, a.jpeg.total_submission_cnt);
}

int main(int argc, char **argv)
{
	struct amdgpu_device *adev = &a;
	struct amdgpu_ring *ring = &a.jpeg.inst[0].ring_dec[0];
	seed_rule = argv[1][0];
	blk.adev = adev;

	u32 nb;
	if (scanf(" inputs bases %u", &nb) != 1)
		return 1;
	for (u32 k = 0; k < nb; k++) {
		u32 hw, seg, x;
		if (scanf(" %u %u %x", &hw, &seg, &x) != 3)
			return 1;
		bases[hw][seg] = x;
		a.reg_offset[hw][0] = bases[hw];
	}
	for (int k = 0; k < 8; k++)
		uvd_bases[k] = bases[UVD_HWIP][k];
	u32 dpm;
	unsigned long long gpu;
	if (scanf(" cg %x pg %x usec %u dpm %u gb %x vcn_ring0_1 %x", &a.cg_flags, &a.pg_flags, &a.usec_timeout, &dpm,
		  &a.gfx.config.gb_addr_config, &a.doorbell_index.vcn.vcn_ring0_1) != 6)
		return 2;
	if (scanf(" ring %15s %u %u %u %llx", ring->name, &ring->doorbell_index, &ring->ring_size, &ring->max_dw,
		  &gpu) != 5)
		return 3;
	a.pm.dpm_enabled = dpm;
	a.nbio.funcs = &nbio_funcs;
	a.jpeg.num_jpeg_inst = 1;
	a.jpeg.num_jpeg_rings = 1;
	a.jpeg.cur_state = AMD_PG_STATE_GATE;
	a.jpeg.internal.jpeg_pitch[0] = mmUVD_JPEG_PITCH_INTERNAL_OFFSET;
	a.jpeg.inst[0].external.jpeg_pitch[0] = SOC15_REG_OFFSET(JPEG, 0, mmUVD_JPEG_PITCH);
	ring->adev = adev;
	ring->funcs = &funcs_dec;
	ring->use_doorbell = true;
	ring->buf_mask = ring->ring_size / 4 - 1;
	ring->ptr_mask = ~0ULL;
	ring->ring = calloc(1, ring->ring_size);
	memset32(ring->ring, ring->funcs->nop, ring->ring_size / 4);
	ring->gpu_addr = gpu;
	ring->wptr_cpu_addr = &wptr_shadow;

	printf("--- jpeg hw_init\n");
	printf("jpeg_v2_0_hw_init %d\n", jpeg_v2_0_hw_init(&blk));
	print_state();
	printf("--- jpeg idle\n");
	amdgpu_jpeg_idle_work_handler(&a.jpeg.idle_work.work);
	print_state();
	printf("--- jpeg use\n");
	amdgpu_jpeg_ring_begin_use(ring);
	amdgpu_jpeg_ring_end_use(ring);
	print_state();
	printf("--- jpeg hw_fini\n");
	printf("jpeg_v2_0_hw_fini %d\n", jpeg_v2_0_hw_fini(&blk));
	print_state();

	printf("--- jpeg parse_cs\n");
	u32 ncases;
	struct amdgpu_cs_parser parser = { .adev = adev };
	if (scanf(" cases %u", &ncases) != 1)
		return 4;
	for (u32 c = 0; c < ncases; c++) {
		static u32 words[64];
		struct amdgpu_ib ib = { .ptr = words };
		if (scanf(" case %u", &ib.length_dw) != 1 || ib.length_dw > 64)
			return 5;
		for (u32 i = 0; i < ib.length_dw; i++)
			if (scanf(" %x", &words[i]) != 1)
				return 5;
		printf("case %u: %d\n", c, amdgpu_jpeg_dec_parse_cs(&parser, NULL, &ib));
	}
	return 0;
}
