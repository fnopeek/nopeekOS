/* The oracle for stage 6's VCN block: Linux's vcn_v2_0 and amdgpu_vcn
 * functions, with the ring helpers they call, copied verbatim by
 * extract.py. It runs the block's life as the port's run has it: hw_init
 * (its ring tests start the block through begin_use), the idle work that
 * gates it, a use that starts it again, and hw_fini.
 *
 * Registers are a file in which a register never written reads SEED(reg)
 * by the rule argv[1] names (ones, zeros or hash). Every register write is
 * printed, and every doorbell with the ring dwords it hands over. Behind
 * the doorbell and the registers runs what the block's polls wait for:
 * the power FSM reports the configured state, a DPG pause is acknowledged,
 * the decoder copies its internal scratch register out, a ring's read
 * pointer follows its write pointer. At the end the DPG SRAM the PSP
 * would load and the firmware's queue modes are printed. The rings, the
 * register bases and the block's state come on stdin from vcn4_check.rs,
 * which runs the port on the same file and prints the same lines. The
 * SMU's VCN power-up and power-down and the PSP command are the other
 * oracles'. */
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

#define AMDGPU_MAX_VCN_INSTANCES 4
#define AMDGPU_MAX_VCN_ENC_RINGS 3
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define lower_32_bits(n) ((u32)((n) & 0xffffffff))
#define upper_32_bits(n) ((u32)(((n) >> 16) >> 16))
#define WARN_ON(c) (c)
#define WARN_ON_ONCE(c) (c)
#define mb() ((void)0)
#define unlikely(x) (x)
#define READ_ONCE(x) (x)
#define memset32(p, v, n) do { for (u32 i_ = 0; i_ < (n); i_++) (p)[i_] = (v); } while (0)
#define order_base_2(n) ((n) > 1 ? 32 - __builtin_clz((u32)(n) - 1) : 0)
#define udelay(n) ((void)0)
#define mdelay(n) ((void)0)
#define AMDGPU_GPU_PAGE_SIZE 4096
#define AMDGPU_GPU_PAGE_ALIGN(a) (((a) + AMDGPU_GPU_PAGE_SIZE - 1) & ~(AMDGPU_GPU_PAGE_SIZE - 1))
#define AMDGPU_UVD_FIRMWARE_OFFSET 256
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
#define __maybe_unused
#define IP_VERSION(mj, mn, rv) (((mj) << 16) | ((mn) << 8) | (rv))
#define AMDGPU_REGS_RLC (1 << 2)
#define AMDGPU_REGS_NO_KIQ (1 << 1)
#define AMDGPU_FW_LOAD_PSP 2
#define AMDGPU_FENCE_FLAG_64BIT (1 << 0)
#define PP_SMC_POWER_PROFILE_VIDEO 3
#define VCN_IDLE_TIMEOUT 1000

#include "vcn_extracted.h"

struct amdgpu_device;
struct amdgpu_ring;
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
	u32 me;
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
struct amdgpu_irq_src { int dummy; };
struct amdgpu_vcn_inst {
	struct amdgpu_device *adev;
	int inst;
	u64 gpu_addr;
	const struct firmware *fw;
	struct amdgpu_ring ring_dec;
	struct amdgpu_ring ring_enc[AMDGPU_MAX_VCN_ENC_RINGS];
	atomic_t sched_score;
	struct amdgpu_irq_src irq;
	struct amdgpu_vcn_reg external;
	struct { u64 gpu_addr; void *cpu_addr; } dpg_sram;
	void *dpg_sram_bo;
	u32 *dpg_sram_cpu_addr;
	u64 dpg_sram_gpu_addr;
	u32 *dpg_sram_curr_addr;
	atomic_t dpg_enc_submission_cnt;
	struct { void *cpu_addr; u64 gpu_addr; u32 mem_size; } fw_shared;
	u8 num_enc_rings;
	enum amd_powergating_state cur_state;
	struct delayed_work idle_work;
	bool indirect_sram;
	struct amdgpu_vcn_reg internal;
	int vcn_pg_lock;
	atomic_t total_submission_cnt;
	bool using_unified_queue;
	struct dpg_pause_state pause_state;
	int (*pause_dpg_mode)(struct amdgpu_vcn_inst *vinst, struct dpg_pause_state *new_state);
	int (*set_pg_state)(struct amdgpu_vcn_inst *vinst, enum amd_powergating_state state);
};
struct amdgpu_firmware_info {
	enum AMDGPU_UCODE_ID ucode_id;
	u64 mc_addr;
	u32 ucode_size;
	u32 tmr_mc_addr_lo, tmr_mc_addr_hi;
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
		u32 num_vcn_inst, harvest_config;
		struct amdgpu_vcn_inst inst[AMDGPU_MAX_VCN_INSTANCES];
		bool workload_profile_active;
		int workload_profile_mutex;
	} vcn;
	struct { u32 load_type; struct amdgpu_firmware_info ucode[AMDGPU_UCODE_ID_MAXIMUM]; } firmware;
	struct { struct { u32 gb_addr_config; } config; struct { void *funcs; bool rlcg_reg_access_supported; } rlc; } gfx;
	struct { bool dpm_enabled; } pm;
	struct { const struct amdgpu_nbio_funcs *funcs; } nbio;
	struct { int dummy; } psp;
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

/* What the block's own state machines do with a write. */
static void vcn_write(u32 reg, u32 v)
{
	if (reg == uvd(mmUVD_PGFSM_CONFIG_BASE_IDX, mmUVD_PGFSM_CONFIG)) {
		u32 st = 0;

		/* Per two-bit tile field: power on (1) reads on (0), off (2) off. */
		for (int f = 0; f < 16; f++)
			if (((v >> (2 * f)) & 3) == 2)
				st |= 2u << (2 * f);
		file_set(uvd(mmUVD_PGFSM_STATUS_BASE_IDX, mmUVD_PGFSM_STATUS), st);
	} else if (reg == uvd(mmUVD_DPG_PAUSE_BASE_IDX, mmUVD_DPG_PAUSE)) {
		u32 ps = uvd(mmUVD_POWER_STATUS_BASE_IDX, mmUVD_POWER_STATUS);
		u32 st = file_get(ps) & ~UVD_POWER_STATUS__UVD_POWER_STATUS_MASK;
		if (v & UVD_DPG_PAUSE__NJ_PAUSE_DPG_REQ_MASK) {
			file_set(reg, v | UVD_DPG_PAUSE__NJ_PAUSE_DPG_ACK_MASK);
			file_set(ps, st | UVD_PGFSM_CONFIG__UVDM_UVDU_PWR_ON);
		} else {
			file_set(reg, v & ~UVD_DPG_PAUSE__NJ_PAUSE_DPG_ACK_MASK);
			file_set(ps, st | UVD_POWER_STATUS__UVD_POWER_STATUS_TILES_OFF);
		}
	}
}

#define RREG32(r) file_get(r)
#define WREG32(r, v) (printf("write reg 0x%x = 0x%x\n", (u32)(r), (u32)(v)), file_set(r, v), vcn_write(r, v))
#define REG_FIELD_SHIFT(reg, field) reg##__##field##__SHIFT
#define REG_FIELD_MASK(reg, field) reg##__##field##_MASK
#define REG_SET_FIELD(orig_val, reg, field, field_val) \
	(((orig_val) & ~REG_FIELD_MASK(reg, field)) | (REG_FIELD_MASK(reg, field) & ((field_val) << REG_FIELD_SHIFT(reg, field))))
#define WREG32_P(reg, val, mask) do { u32 tmp_ = RREG32(reg); tmp_ &= (mask); tmp_ |= ((val) & ~(mask)); WREG32(reg, tmp_); } while (0)
#include "soc15_common.h"

/* The engine behind a doorbell: the decoder's internal scratch register
 * shows outside, and each ring's read pointer follows. */
static void vcn_run(struct amdgpu_ring *ring, u64 from, u64 to)
{
	if (ring == &a.vcn.inst[0].ring_dec) {
		for (u64 at = from; at != to; at += 2) {
			u32 hdr = ring->ring[at & ring->buf_mask];
			u32 v = ring->ring[(at + 1) & ring->buf_mask];
			if ((hdr & 0xffff) == mmUVD_SCRATCH9_INTERNAL_OFFSET)
				file_set(uvd(mmUVD_SCRATCH9_BASE_IDX, mmUVD_SCRATCH9), v);
		}
		file_set(uvd(mmUVD_RBC_RB_WPTR_BASE_IDX, mmUVD_RBC_RB_WPTR), to);
		file_set(uvd(mmUVD_RBC_RB_RPTR_BASE_IDX, mmUVD_RBC_RB_RPTR), to);
	} else if (ring == &a.vcn.inst[0].ring_enc[0]) {
		file_set(uvd(mmUVD_RB_WPTR_BASE_IDX, mmUVD_RB_WPTR), to);
		file_set(uvd(mmUVD_RB_RPTR_BASE_IDX, mmUVD_RB_RPTR), to);
	} else {
		file_set(uvd(mmUVD_RB_WPTR2_BASE_IDX, mmUVD_RB_WPTR2), to);
		file_set(uvd(mmUVD_RB_RPTR2_BASE_IDX, mmUVD_RB_RPTR2), to);
	}
}

static struct amdgpu_ring *rings[4];
static int nrings;

/* The doorbell takes the wptr in dwords. */
static void amdgpu_mm_wdoorbell(struct amdgpu_device *adev, u32 index, u32 v)
{
	for (int r = 0; r < nrings; r++) {
		struct amdgpu_ring *ring = rings[r];

		if (!ring->use_doorbell || ring->doorbell_index != index)
			continue;
		printf("doorbell 0x%x = 0x%x:", index, v);
		for (u64 at = ring->rung; at != v; at++)
			printf(" %08x", ring->ring[at & ring->buf_mask]);
		printf("\n");
		vcn_run(ring, ring->rung, v);
		ring->rung = v;
		return;
	}
	printf("doorbell 0x%x = 0x%x: no ring\n", index, v);
}
#define WDOORBELL32(index, v) amdgpu_mm_wdoorbell(adev, (index), (v))

static int amdgpu_ring_test_ring(struct amdgpu_ring *ring) { return ring->funcs->test_ring(ring); }
static void amdgpu_ring_set_wptr(struct amdgpu_ring *ring) { ring->funcs->set_wptr(ring); }
static u64 amdgpu_ring_get_rptr(struct amdgpu_ring *ring) { return ring->funcs->get_rptr(ring); }

/* The SMU's VCN power gating and its workload profile are the SMU
 * oracle's; the PSP command is the PSP oracle's. What the PSP loads is
 * printed at the end. */
static void amdgpu_dpm_enable_vcn(struct amdgpu_device *adev, bool enable, int inst) {}
static int amdgpu_dpm_switch_power_profile(struct amdgpu_device *adev, int type, bool en) { return 0; }
static u32 sram_loaded;
static int psp_execute_ip_fw_load(void *psp, struct amdgpu_firmware_info *ucode)
{
	sram_loaded = ucode->ucode_size;
	return 0;
}
/* Bare metal. */
static int vcn_v2_0_start_sriov(struct amdgpu_device *adev) { abort(); }
#define cancel_delayed_work_sync(w) ((void)(w))
#define schedule_delayed_work(w, t) ((void)(w))

#include "vcn_protos.h"
#include "vcn_extracted.c"

static const struct amdgpu_ring_funcs funcs_dec = {
	.type = AMDGPU_RING_TYPE_VCN_DEC, .align_mask = 0xf, .nop = 0,
	.get_rptr = vcn_v2_0_dec_ring_get_rptr, .get_wptr = vcn_v2_0_dec_ring_get_wptr,
	.set_wptr = vcn_v2_0_dec_ring_set_wptr, .insert_nop = vcn_v2_0_dec_ring_insert_nop,
	.test_ring = vcn_v2_0_dec_ring_test_ring,
	.begin_use = amdgpu_vcn_ring_begin_use, .end_use = amdgpu_vcn_ring_end_use,
};
static const struct amdgpu_ring_funcs funcs_enc = {
	.type = AMDGPU_RING_TYPE_VCN_ENC, .align_mask = 0x3f, .nop = VCN_ENC_CMD_NO_OP,
	.get_rptr = vcn_v2_0_enc_ring_get_rptr, .get_wptr = vcn_v2_0_enc_ring_get_wptr,
	.set_wptr = vcn_v2_0_enc_ring_set_wptr, .insert_nop = amdgpu_ring_insert_nop,
	.test_ring = amdgpu_vcn_enc_ring_test_ring,
	.begin_use = amdgpu_vcn_ring_begin_use, .end_use = amdgpu_vcn_ring_end_use,
};
static const struct amdgpu_nbio_funcs nbio_funcs = { .vcn_doorbell_range = nbio_v7_0_vcn_doorbell_range };

static u32 bases[MAX_HWIP][8];
static u64 wptr_shadow[4];
static u8 shared[4096];
static u32 sram[4096];
static struct firmware fw;

static void print_state(void)
{
	struct amdgpu_vcn_inst *v = &a.vcn.inst[0];
	struct amdgpu_fw_shared *s = v->fw_shared.cpu_addr;

	for (int r = 0; r < nrings; r++)
		printf("ready %s %d wptr 0x%llx\n", rings[r]->name, rings[r]->sched.ready,
		       (unsigned long long)rings[r]->wptr);
	printf("state %u pause %u queues %u %u %u\n", v->cur_state, v->pause_state.fw_based,
	       s->multi_queue.decode_queue_mode, s->multi_queue.encode_generalpurpose_queue_mode,
	       s->multi_queue.encode_lowlatency_queue_mode);
}

int main(int argc, char **argv)
{
	struct amdgpu_device *adev = &a;
	static struct amdgpu_ip_block blk;
	struct amdgpu_vcn_inst *v = &a.vcn.inst[0];
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
	unsigned long long vcpu, shared_gpu, sram_gpu;
	u32 indirect, enc_rings, tmr_lo, tmr_hi, fw_size, dpm;
	if (scanf(" cg %x pg %x usec %u dpm %u gb %x fw %u tmr %x %x vcpu %llx shared %llx sram %llx indirect %u enc %u",
		  &a.cg_flags, &a.pg_flags, &a.usec_timeout, &dpm, &a.gfx.config.gb_addr_config, &fw_size, &tmr_lo,
		  &tmr_hi, &vcpu, &shared_gpu, &sram_gpu, &indirect, &enc_rings) != 13)
		return 2;
	a.pm.dpm_enabled = dpm;
	a.nbio.funcs = &nbio_funcs;
	a.vcn.num_vcn_inst = 1;
	a.firmware.load_type = AMDGPU_FW_LOAD_PSP;
	a.firmware.ucode[AMDGPU_UCODE_ID_VCN].tmr_mc_addr_lo = tmr_lo;
	a.firmware.ucode[AMDGPU_UCODE_ID_VCN].tmr_mc_addr_hi = tmr_hi;
	fw.size = fw_size;
	v->adev = adev;
	v->fw = &fw;
	v->gpu_addr = vcpu;
	v->fw_shared.cpu_addr = shared;
	v->fw_shared.gpu_addr = shared_gpu;
	v->dpg_sram_cpu_addr = sram;
	v->dpg_sram_gpu_addr = sram_gpu;
	v->indirect_sram = indirect;
	v->num_enc_rings = enc_rings;
	v->cur_state = AMD_PG_STATE_GATE;
	v->pause_dpg_mode = vcn_v2_0_pause_dpg_mode;
	v->set_pg_state = vcn_v2_0_set_pg_state;
	v->internal.context_id = mmUVD_CONTEXT_ID_INTERNAL_OFFSET;
	v->internal.ib_vmid = mmUVD_LMI_RBC_IB_VMID_INTERNAL_OFFSET;
	v->internal.ib_bar_low = mmUVD_LMI_RBC_IB_64BIT_BAR_LOW_INTERNAL_OFFSET;
	v->internal.ib_bar_high = mmUVD_LMI_RBC_IB_64BIT_BAR_HIGH_INTERNAL_OFFSET;
	v->internal.ib_size = mmUVD_RBC_IB_SIZE_INTERNAL_OFFSET;
	v->internal.gp_scratch8 = mmUVD_GP_SCRATCH8_INTERNAL_OFFSET;
	v->internal.scratch9 = mmUVD_SCRATCH9_INTERNAL_OFFSET;
	v->internal.data0 = mmUVD_GPCOM_VCPU_DATA0_INTERNAL_OFFSET;
	v->internal.data1 = mmUVD_GPCOM_VCPU_DATA1_INTERNAL_OFFSET;
	v->internal.cmd = mmUVD_GPCOM_VCPU_CMD_INTERNAL_OFFSET;
	v->internal.nop = mmUVD_NO_OP_INTERNAL_OFFSET;
	v->external.scratch9 = SOC15_REG_OFFSET(VCN, 0, mmUVD_SCRATCH9);
	v->external.data0 = SOC15_REG_OFFSET(VCN, 0, mmUVD_GPCOM_VCPU_DATA0);
	v->external.data1 = SOC15_REG_OFFSET(VCN, 0, mmUVD_GPCOM_VCPU_DATA1);
	v->external.cmd = SOC15_REG_OFFSET(VCN, 0, mmUVD_GPCOM_VCPU_CMD);
	v->external.nop = SOC15_REG_OFFSET(VCN, 0, mmUVD_NO_OP);
	for (u32 i = 0; i < 1 + enc_rings; i++) {
		struct amdgpu_ring *ring = i ? &v->ring_enc[i - 1] : &v->ring_dec;
		unsigned long long gpu;
		if (scanf(" ring %15s %u %u %u %llx", ring->name, &ring->doorbell_index, &ring->ring_size, &ring->max_dw,
			  &gpu) != 5)
			return 3;
		ring->adev = adev;
		ring->funcs = i ? &funcs_enc : &funcs_dec;
		ring->use_doorbell = true;
		ring->buf_mask = ring->ring_size / 4 - 1;
		ring->ptr_mask = ~0ULL;
		ring->ring = calloc(1, ring->ring_size);
		memset32(ring->ring, ring->funcs->nop, ring->ring_size / 4);
		ring->gpu_addr = gpu;
		ring->wptr_cpu_addr = &wptr_shadow[i];
		rings[nrings++] = ring;
	}

	printf("--- vcn hw_init\n");
	printf("vcn_v2_0_hw_init %d\n", vcn_v2_0_hw_init(&blk));
	print_state();
	printf("--- vcn idle\n");
	amdgpu_vcn_idle_work_handler(&v->idle_work.work);
	print_state();
	printf("--- vcn use\n");
	amdgpu_vcn_ring_begin_use(&v->ring_dec);
	amdgpu_vcn_ring_end_use(&v->ring_dec);
	print_state();
	printf("--- vcn hw_fini\n");
	printf("vcn_v2_0_hw_fini %d\n", vcn_v2_0_hw_fini(&blk));
	print_state();
	printf("sram %u bytes:", sram_loaded);
	for (u32 i = 0; i < sram_loaded / 4; i++)
		printf(" %08x", sram[i]);
	printf("\n");
	return 0;
}
