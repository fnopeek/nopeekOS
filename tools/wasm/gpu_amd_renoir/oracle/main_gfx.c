/* The oracle for stage 4's GFX block: Linux's gfx_v9_0 functions, with the
 * ring, KIQ, RLC and IRQ helpers they call, copied verbatim by extract.py.
 * It runs the block's life as the port's run has it: hw_init (ring tests
 * included), late_init, clock and power gating on, both off again, and
 * hw_fini.
 *
 * Registers are a file in which a register never written reads SEED(reg)
 * by the rule argv[1] names (ones, zeros or hash). Every register write is
 * printed, and every doorbell with the ring dwords it hands over. Behind
 * the doorbells a CP runs just what the ring tests poll for: a
 * SET_UCONFIG_REG sets the register it names, unprinted. The rings, the
 * register bases and the block's state come on stdin from gfx4_check.rs,
 * which runs the port on the same file and prints the same lines. The SMU
 * mailbox writes of GFXOFF are not part of it: amdgpu_dpm_set_powergating_by_smu
 * is the SMU oracle's. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>
#include <errno.h>
typedef int atomic_t;
typedef unsigned long long atomic64_t;
#include "soc15_hw_ip.h"
#include "amd_shared.h"
#include "gc/gc_9_0_offset.h"
#include "gc/gc_9_0_sh_mask.h"
#include "gc/gc_9_0_default.h"
#include "vega10_enum.h"
#include "soc15d.h"
#include "v9_structs.h"
#include "clearstate_defs.h"
#include "clearstate_gfx9.h"
#include "gfx_types.h"
#define __maybe_unused __attribute__((unused))
#include "gfx_v9_0_cleaner_shader.h"
/* glibc defines it on every host: the kernel's little-endian build has
 * it undefined. */
#undef __BIG_ENDIAN

#define AMDGPU_MAX_RINGS 128
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define BIT(n) (1u << (n))
#define lower_32_bits(n) ((u32)((n) & 0xffffffff))
#define upper_32_bits(n) ((u32)(((n) >> 16) >> 16))
#define min(a, b) ((a) < (b) ? (a) : (b))
#define max(a, b) ((a) > (b) ? (a) : (b))
#define cpu_to_le32(x) (x)
#define le32_to_cpu(x) (x)
#define WARN_ON(c) (c)
#define WARN_ON_ONCE(c) (c)
#define unlikely(c) (c)
#define likely(c) (c)
#define mb() ((void)0)
#define mutex_lock(m) ((void)0)
#define mutex_unlock(m) ((void)0)
#define spin_lock(m) ((void)0)
#define spin_unlock(m) ((void)0)
#define spin_lock_irqsave(l, f) ((void)(f))
#define spin_unlock_irqrestore(l, f) ((void)(f))
#define atomic_read(a) (*(a))
#define atomic_inc_return(a) (++*(a))
#define atomic_dec_and_test(a) (--*(a) == 0)
#define atomic64_set(p, v) (*(p) = (v))
#define atomic64_read(p) (*(p))
#define READ_ONCE(x) (x)
#define memset32(p, v, n) do { for (u32 i_ = 0; i_ < (n); i_++) (p)[i_] = (v); } while (0)
#define test_bit(i, m) (((m)[(i) / 64] >> ((i) % 64)) & 1)
#define hweight32(x) __builtin_popcount(x)
#define order_base_2(n) ((n) > 1 ? 32 - __builtin_clz((u32)(n) - 1) : 0)
#define udelay(n) ((void)0)
#define mdelay(n) ((void)0)
/* Messages are the port's log lines, not part of the comparison. */
#define DRM_INFO(...) ((void)0)
#define DRM_ERROR(...) ((void)0)
#define DRM_DEBUG(...) ((void)0)
#define DRM_DEV_ERROR(d, ...) ((void)0)
#define DRM_DEV_DEBUG(d, ...) ((void)0)
#define dev_err(d, ...) ((void)0)
#define dev_info(d, ...) ((void)0)
#define dev_dbg(d, ...) ((void)0)
#define BUG() abort()
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#define amdgpu_sriov_wreg(adev, reg, value, flag, hwip, inst) ((void)0)
#define amdgpu_sriov_is_pp_one_vf(adev) 0
#define amdgpu_in_reset(adev) 0
#define amdgpu_ras_intr_triggered() 0
#define amdgpu_ras_is_rma(adev) 0
#define amdgpu_device_flush_hdp(adev, ring) ((void)0)
#define IP_VERSION(mj, mn, rv) (((mj) << 16) | ((mn) << 8) | (rv))
#define AMDGPU_REGS_RLC (1 << 2)
#define AMDGPU_REGS_NO_KIQ (1 << 1)
#define AMDGPU_XCP_NO_PARTITION (~0)
#define MAX_XCP 8

struct amdgpu_device;
struct amdgpu_ring;
struct amdgpu_irq_src;
struct amdgpu_iv_entry;
enum amdgpu_irq_src_ids { AMDGPU_IRQ_SRC_DUMMY };
struct amdgpu_irq_src_funcs {
	int (*set)(struct amdgpu_device *adev, struct amdgpu_irq_src *source, unsigned type,
		   enum amdgpu_interrupt_state state);
};
struct amdgpu_irq_src {
	unsigned num_types;
	atomic_t *enabled_types;
	const struct amdgpu_irq_src_funcs *funcs;
};
struct amdgpu_ring_funcs {
	enum amdgpu_ring_type type;
	u32 align_mask;
	u32 nop;
	void (*set_wptr)(struct amdgpu_ring *ring);
	void (*insert_nop)(struct amdgpu_ring *ring, uint32_t count);
	int (*test_ring)(struct amdgpu_ring *ring);
	void (*begin_use)(struct amdgpu_ring *ring);
	void (*end_use)(struct amdgpu_ring *ring);
};
struct amdgpu_bo { u64 gpu_offset; };
#define amdgpu_bo_gpu_offset(bo) ((bo)->gpu_offset)
struct amdgpu_ring {
	struct amdgpu_device *adev;
	const struct amdgpu_ring_funcs *funcs;
	u32 *ring;
	u64 wptr, wptr_old;
	int count_dw;
	u32 buf_mask, ring_size, max_dw;
	u64 ptr_mask;
	u32 me, pipe, queue, xcc_id, xcp_id;
	bool use_doorbell;
	u32 doorbell_index;
	u64 gpu_addr, rptr_gpu_addr, wptr_gpu_addr, eop_gpu_addr, mqd_gpu_addr;
	u64 *wptr_cpu_addr;
	u32 *rptr_cpu_addr;
	void *mqd_ptr;
	struct amdgpu_bo *mqd_obj;
	struct { bool ready; } sched;
	char name[16];
	int idx;
	/* The wptr the last doorbell handed over, for the dump. */
	u64 rung;
};
struct kiq_pm4_funcs {
	void (*kiq_set_resources)(struct amdgpu_ring *kiq_ring, uint64_t queue_mask);
	void (*kiq_map_queues)(struct amdgpu_ring *kiq_ring, struct amdgpu_ring *ring);
	void (*kiq_unmap_queues)(struct amdgpu_ring *kiq_ring, struct amdgpu_ring *ring,
				 enum amdgpu_unmap_queues_action action, u64 gpu_addr, u64 seq);
	int set_resources_size;
	int map_queues_size;
	int unmap_queues_size;
};
struct amdgpu_rlc_funcs {
	bool (*is_rlc_enabled)(struct amdgpu_device *adev);
	void (*set_safe_mode)(struct amdgpu_device *adev, int xcc_id);
	void (*unset_safe_mode)(struct amdgpu_device *adev, int xcc_id);
	void (*get_csb_buffer)(struct amdgpu_device *adev, u32 *buffer);
	int (*resume)(struct amdgpu_device *adev);
	void (*stop)(struct amdgpu_device *adev);
	void (*start)(struct amdgpu_device *adev);
};
struct amdgpu_kiq {
	struct amdgpu_ring ring;
	const struct kiq_pm4_funcs *pmf;
	void *mqd_backup;
	int ring_lock;
};
struct amdgpu_cu_info {
	u32 number, ao_cu_mask, simd_per_cu;
	u32 bitmap[1][4][4];
	u32 ao_cu_bitmap[4][4];
};
struct amdgpu_device {
	void *dev;
	u32 flags, apu_flags, cg_flags, pg_flags, usec_timeout;
	u32 ip_versions[MAX_HWIP][HWIP_MAX_INSTANCE];
	u32 *reg_offset[MAX_HWIP][HWIP_MAX_INSTANCE];
	bool in_s0ix, in_suspend, in_s3;
	struct { u32 pp_feature; } pm;
	struct {
		struct {
			u32 max_shader_engines, max_sh_per_se, max_backends_per_se, max_cu_per_sh, max_hw_contexts;
			u32 backend_enable_mask, num_rbs, db_debug2;
		} config;
		struct amdgpu_cu_info cu_info;
		struct {
			const struct amdgpu_rlc_funcs *funcs;
			bool rlcg_reg_access_supported, is_rlc_v2_1;
			bool in_safe_mode[1];
			const struct cs_section_def *cs_data;
			u32 *cs_ptr;
			u64 clear_state_gpu_addr, cp_table_gpu_addr;
			u32 clear_state_size;
		} rlc;
		struct { u32 num_mec, num_pipe_per_mec, num_queue_per_pipe; void *mqd_backup[AMDGPU_MAX_COMPUTE_RINGS]; } mec;
		struct { unsigned long queue_bitmap[AMDGPU_MAX_COMPUTE_QUEUES / 64]; } mec_bitmap[1];
		struct amdgpu_kiq kiq[1];
		struct amdgpu_ring gfx_ring[1];
		struct amdgpu_ring compute_ring[AMDGPU_MAX_COMPUTE_RINGS];
		u32 num_gfx_rings, num_compute_rings;
		u32 me_fw_version, me_feature_version;
		struct amdgpu_irq_src priv_reg_irq, priv_inst_irq, bad_op_irq;
		bool enable_cleaner_shader, mcbp;
		u64 cleaner_shader_gpu_addr;
		void *cleaner_shader_cpu_ptr, *cleaner_shader_ptr;
		u32 cleaner_shader_size;
		const struct { int (*init_spm_golden)(struct amdgpu_device *adev); } *funcs;
		int gfx_off_delay_work;
		struct { void *funcs; } *ras;
		u32 gfx_off_req_count;
		bool gfx_off_state;
	} gfx;
	struct { u32 noretry; u64 private_aperture_start, shared_aperture_start; struct { bool connected_to_cpu; } xgmi; } gmc;
	struct { u32 num_ids; } id_mgr_gfx;
	struct { u32 first_kfd_vmid; struct { u32 num_ids; } id_mgr[1]; } vm_manager;
	struct { struct { bool hash_64k, hash_2m, hash_1g; } hash_status; } df;
	struct { u32 gds_size, gds_compute_max_wave_id; } gds;
	struct { u32 kiq, userqueue_end; } doorbell_index;
	struct { u32 size; } doorbell;
	struct { bool installed; } irq;
	struct { bool enable_legacy_queue_map; } mes;
	bool barrier_has_auto_waitcnt, enable_mes, enable_mes_kiq;
	struct { u32 load_type; } firmware;
};
struct amdgpu_ip_block { struct amdgpu_device *adev; };
#define amdgpu_ip_version(adev, ip, inst) ((adev)->ip_versions[ip][inst] >> 8)
#define AMDGPU_GFXHUB(x) (x)

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

#define RREG32(r) file_get(r)
#define WREG32(r, v) (printf("write reg 0x%x = 0x%x\n", (u32)(r), (u32)(v)), file_set(r, v))
#define REG_FIELD_SHIFT(reg, field) reg##__##field##__SHIFT
#define REG_FIELD_MASK(reg, field) reg##__##field##_MASK
#define REG_SET_FIELD(orig_val, reg, field, field_val) \
	(((orig_val) & ~REG_FIELD_MASK(reg, field)) | (REG_FIELD_MASK(reg, field) & ((field_val) << REG_FIELD_SHIFT(reg, field))))
#define REG_GET_FIELD(value, reg, field) (((value) & REG_FIELD_MASK(reg, field)) >> REG_FIELD_SHIFT(reg, field))
#include "soc15_common.h"

/* The CP behind the doorbells: the ring tests' SET_UCONFIG_REG. */
static void cp_run(struct amdgpu_ring *ring, u64 from, u64 to)
{
	for (u64 at = from; at != to;) {
		u32 hdr = ring->ring[at & ring->buf_mask];
		u32 op = (hdr >> 8) & 0xff, count = (hdr >> 16) & 0x3fff;
		u32 len = (op == PACKET3_NOP && count == 0x3fff) ? 1 : count + 2;

		if (op == PACKET3_SET_UCONFIG_REG)
			for (u32 i = 0; i < count; i++)
				file_set((ring->ring[(at + 1) & ring->buf_mask] & 0xffff) + PACKET3_SET_UCONFIG_REG_START + i,
					 ring->ring[(at + 2 + i) & ring->buf_mask]);
		at += len;
	}
}

static void amdgpu_mm_wdoorbell64(struct amdgpu_device *adev, u32 index, u64 v);
#define WDOORBELL64(index, v) amdgpu_mm_wdoorbell64(adev, (index), (v))
static struct amdgpu_ring *rings[AMDGPU_MAX_RINGS];
static int nrings;

static void amdgpu_mm_wdoorbell64(struct amdgpu_device *adev, u32 index, u64 v)
{
	for (int r = 0; r < nrings; r++) {
		struct amdgpu_ring *ring = rings[r];

		if (!ring->use_doorbell || ring->doorbell_index != index)
			continue;
		printf("doorbell 0x%x = 0x%llx:", index, (unsigned long long)v);
		for (u64 at = ring->rung; at != v; at++)
			printf(" %08x", ring->ring[at & ring->buf_mask]);
		printf("\n");
		cp_run(ring, ring->rung, v);
		ring->rung = v;
		return;
	}
	printf("doorbell 0x%x = 0x%llx: no ring\n", index, (unsigned long long)v);
}

int amdgpu_irq_update(struct amdgpu_device *adev, struct amdgpu_irq_src *src, unsigned int type);
bool amdgpu_irq_enabled(struct amdgpu_device *adev, struct amdgpu_irq_src *src, unsigned int type);
static int amdgpu_ring_test_ring(struct amdgpu_ring *ring) { return ring->funcs->test_ring(ring); }
static void amdgpu_ring_set_wptr(struct amdgpu_ring *ring) { ring->funcs->set_wptr(ring); }
static void amdgpu_ring_emit_wreg(struct amdgpu_ring *ring, u32 reg, u32 v) { abort(); }
static void amdgpu_ring_emit_rreg(struct amdgpu_ring *ring, u32 reg, u32 v) { abort(); }

/* amdgpu_dpm_set_powergating_by_smu: GFXOFF goes to the SMU; the SMU
 * oracle checks its messages, and the GFX block sees it succeed. */
#define AMD_IP_BLOCK_TYPE_GFX 6
static int amdgpu_dpm_set_powergating_by_smu(struct amdgpu_device *adev, u32 block, bool gate, int inst)
{
	return 0;
}
#define amdgpu_gfx_init_spm_golden(adev) abort()
#define AMD_RESET_METHOD_MODE2 3
#define amdgpu_asic_reset_method(adev) 0
#define RREG32_NO_KIQ(r) RREG32(r)
#define WREG32_NO_KIQ(r, v) WREG32(r, v)
#define memcpy_toio(d, s, n) memcpy(d, s, n)
static int gfx_v9_0_get_cu_info(struct amdgpu_device *adev, struct amdgpu_cu_info *cu_info);
static void gfx_v9_0_update_spm_vmid_internal(struct amdgpu_device *adev, unsigned int vmid);
/* The golden tables of other GC versions: their branches are not taken
 * on 9.3.0. */
static const struct soc15_reg_golden golden_settings_gc_9_0[1], golden_settings_gc_9_0_vg10[1],
	golden_settings_gc_9_2_1[1], golden_settings_gc_9_2_1_vg12[1], golden_settings_gc_9_0_vg20[1],
	golden_settings_gc_9_4_1_arct[1], golden_settings_gc_9_1[1], golden_settings_gc_9_1_rv2[1],
	golden_settings_gc_9_1_rv1[1];
#define cancel_delayed_work_sync(w) ((void)0)
#define schedule_delayed_work(w, d) abort()
#define GFX_OFF_DELAY_ENABLE 0
/* No RAS on Renoir: both EDC workarounds return at their RAS check, and
 * there is no gfx.ras. */
static int gfx_v9_0_ecc_late_init(struct amdgpu_ip_block *ip_block) { return 0; }
#define gfx_v9_4_2_debug_trap_config_init(adev, a, b) abort()
#define gfx_v9_4_2_set_power_brake_sequence(adev) abort()
#define gfx_v9_4_2_init_golden_registers(adev, die) abort()
#define gfx_v9_4_2_init_sq(adev) abort()
#define gfx_v9_1_init_rlc_save_restore_list(adev) abort()
#define gfx_v9_0_init_lbpw(adev) abort()
#define gfx_v9_4_init_lbpw(adev) abort()
#define gfx_v9_0_enable_lbpw(adev, e) abort()
#define gfx_v9_0_cp_gfx_load_microcode(adev) (abort(), 0)
#define gfx_v9_0_cp_compute_load_microcode(adev) (abort(), 0)
#define gfx_v9_0_rlc_load_microcode(adev) (abort(), 0)
#define gfx_v9_0_deactivate_kcq_hqd(adev) abort()
#define pwr_10_0_gfxip_control_over_cgpg(adev, e) abort()
#define pm_resume_via_firmware() false
#define amdgpu_mes_map_legacy_queue(adev, ring) (abort(), 0)
#define amdgpu_gfx_mes_enable_kcq(adev, xcc) (abort(), 0)
#define amdgpu_mes_unmap_legacy_queue(adev, ring, a, b, c) abort()
#define amdgpu_amdkfd_is_kfd_vmid(adev, v) false
#define amdgpu_gfx_select_se_sh(adev, se, sh, inst, xcc) gfx_v9_0_select_se_sh(adev, se, sh, inst, xcc)
#define amdgpu_gfx_select_me_pipe_q(adev, me, pipe, q, vmid, xcc) soc15_grbm_select(adev, me, pipe, q, vmid, xcc)
static const char *amdgpu_disable_cu;
static int amdgpu_lbpw;
#define AMDGPU_FW_LOAD_PSP 2
static void gfx_v9_0_select_se_sh(struct amdgpu_device *adev, u32 se_num, u32 sh_num, u32 instance, int xcc_id);
static void soc15_grbm_select(struct amdgpu_device *adev, u32 me, u32 pipe, u32 queue, u32 vmid, int xcc_id);

#include "gfx_extracted.c"

static const struct amdgpu_rlc_funcs rlc_funcs = {
	.is_rlc_enabled = gfx_v9_0_is_rlc_enabled,
	.set_safe_mode = gfx_v9_0_set_safe_mode,
	.unset_safe_mode = gfx_v9_0_unset_safe_mode,
	.get_csb_buffer = gfx_v9_0_get_csb_buffer,
	.resume = gfx_v9_0_rlc_resume,
	.stop = gfx_v9_0_rlc_stop,
	.start = gfx_v9_0_rlc_start,
};
static const struct kiq_pm4_funcs kiq_pm4_funcs = {
	.kiq_set_resources = gfx_v9_0_kiq_set_resources,
	.kiq_map_queues = gfx_v9_0_kiq_map_queues,
	.kiq_unmap_queues = gfx_v9_0_kiq_unmap_queues,
	.set_resources_size = 8,
	.map_queues_size = 7,
	.unmap_queues_size = 6,
};
static const struct amdgpu_ring_funcs funcs_gfx = {
	.type = AMDGPU_RING_TYPE_GFX, .align_mask = 0xff, .nop = PACKET3(PACKET3_NOP, 0x3FFF),
	.set_wptr = gfx_v9_0_ring_set_wptr_gfx, .insert_nop = gfx_v9_ring_insert_nop,
	.test_ring = gfx_v9_0_ring_test_ring,
};
static const struct amdgpu_ring_funcs funcs_compute = {
	.type = AMDGPU_RING_TYPE_COMPUTE, .align_mask = 0xff, .nop = PACKET3(PACKET3_NOP, 0x3FFF),
	.set_wptr = gfx_v9_0_ring_set_wptr_compute, .insert_nop = gfx_v9_ring_insert_nop,
	.test_ring = gfx_v9_0_ring_test_ring,
};
static const struct amdgpu_ring_funcs funcs_kiq = {
	.type = AMDGPU_RING_TYPE_KIQ, .align_mask = 0xff, .nop = PACKET3(PACKET3_NOP, 0x3FFF),
	.set_wptr = gfx_v9_0_ring_set_wptr_compute, .insert_nop = amdgpu_ring_insert_nop,
	.test_ring = gfx_v9_0_ring_test_ring,
};
static const struct amdgpu_irq_src_funcs priv_reg_funcs = { .set = gfx_v9_0_set_priv_reg_fault_state };
static const struct amdgpu_irq_src_funcs priv_inst_funcs = { .set = gfx_v9_0_set_priv_inst_fault_state };
static const struct amdgpu_irq_src_funcs bad_op_funcs = { .set = gfx_v9_0_set_bad_op_fault_state };

static u32 bases[MAX_HWIP][8];
static u64 wptr_shadow[AMDGPU_MAX_RINGS];

/* One ring as gfx4_check.rs describes it on a `ring` line. */
static void read_ring(struct amdgpu_device *adev, struct amdgpu_ring *ring)
{
	char kind[16];
	unsigned long long gpu, rptr, wptr, eop, mqd;
	u32 size, me, pipe, queue, db, max_dw;
	if (scanf(" ring %15s %15s %u %u %u %u %u %u %llx %llx %llx %llx %llx", ring->name, kind, &me, &pipe, &queue,
		  &db, &size, &max_dw, &gpu, &rptr, &wptr, &eop, &mqd) != 13)
		abort();
	ring->adev = adev;
	ring->funcs = !strcmp(kind, "gfx") ? &funcs_gfx : !strcmp(kind, "compute") ? &funcs_compute : &funcs_kiq;
	ring->me = me;
	ring->pipe = pipe;
	ring->queue = queue;
	ring->use_doorbell = true;
	ring->doorbell_index = db;
	ring->ring_size = size;
	ring->buf_mask = size / 4 - 1;
	ring->ptr_mask = ~0ULL;
	ring->max_dw = max_dw;
	ring->ring = calloc(1, size);
	memset32(ring->ring, ring->funcs->nop, size / 4);
	ring->gpu_addr = gpu;
	ring->rptr_gpu_addr = rptr;
	ring->wptr_gpu_addr = wptr;
	ring->eop_gpu_addr = eop;
	ring->mqd_gpu_addr = mqd;
	ring->mqd_ptr = calloc(1, sizeof(struct v9_mqd_allocation));
	ring->mqd_obj = calloc(1, sizeof(struct amdgpu_bo));
	ring->mqd_obj->gpu_offset = mqd;
	ring->wptr_cpu_addr = &wptr_shadow[nrings];
	ring->idx = nrings;
	rings[nrings++] = ring;
}

static void dump_mqd(struct amdgpu_ring *ring)
{
	u32 *m = ring->mqd_ptr;

	printf("mqd %s:", ring->name);
	for (u32 i = 0; i < sizeof(struct v9_mqd_allocation) / 4; i++)
		if (m[i])
			printf(" %x=%08x", i, m[i]);
	printf("\n");
}

int main(int argc, char **argv)
{
	static struct amdgpu_device a;
	struct amdgpu_device *adev = &a;
	static struct amdgpu_ip_block blk;
	seed_rule = argv[1][0];
	blk.adev = adev;

	char ip[16];
	u32 n, nb;
	if (scanf(" inputs bases %u", &nb) != 1)
		return 1;
	for (u32 k = 0; k < nb; k++) {
		u32 hw, seg, v;
		if (scanf(" %u %u %x", &hw, &seg, &v) != 3)
			return 1;
		bases[hw][seg] = v;
		a.reg_offset[hw][0] = bases[hw];
	}
	unsigned long long csb, cpt, priv_ap, shared_ap, qmask, cleaner_gpu;
	u32 cleaner_on;
	if (scanf(" versions %x %x flags %x apu %x cg %x pg %x pp %x"
		  " config %u %u %u %u %u mec %u %u %u compute %u gfx %u me_fw %x %x"
		  " rlc %u %llx %u %llx doorbell %u %u %u vm %u %u gmc %u %llx %llx gds %u %u qmask %llx cleaner %u %llx",
		  &a.ip_versions[GC_HWIP][0], &a.ip_versions[SDMA0_HWIP][0], &a.flags, &a.apu_flags, &a.cg_flags,
		  &a.pg_flags, &a.pm.pp_feature, &a.gfx.config.max_shader_engines, &a.gfx.config.max_sh_per_se,
		  &a.gfx.config.max_backends_per_se, &a.gfx.config.max_cu_per_sh, &a.gfx.config.max_hw_contexts,
		  &a.gfx.mec.num_mec, &a.gfx.mec.num_pipe_per_mec, &a.gfx.mec.num_queue_per_pipe,
		  &a.gfx.num_compute_rings, &a.gfx.num_gfx_rings, &a.gfx.me_fw_version, &a.gfx.me_feature_version,
		  (u32 *)&a.gfx.rlc.is_rlc_v2_1, &csb, &a.gfx.rlc.clear_state_size, &cpt, &a.doorbell_index.kiq,
		  &a.doorbell_index.userqueue_end, &a.doorbell.size, &a.vm_manager.first_kfd_vmid,
		  &a.vm_manager.id_mgr[0].num_ids, &a.gmc.noretry, &priv_ap, &shared_ap, &a.gds.gds_size,
		  &a.gds.gds_compute_max_wave_id, &qmask, &cleaner_on, &cleaner_gpu) != 36)
		return 2;
	a.gfx.rlc.clear_state_gpu_addr = csb;
	a.gfx.rlc.cp_table_gpu_addr = cpt;
	a.gmc.private_aperture_start = priv_ap;
	a.gmc.shared_aperture_start = shared_ap;
	a.gfx.mec_bitmap[0].queue_bitmap[0] = qmask;
	/* gfx_v9_0_sw_init for GC 9.3.0, with the port's firmware versions
	 * deciding enable_cleaner_shader. */
	a.gfx.cleaner_shader_ptr = (void *)gfx_9_4_2_cleaner_shader_hex;
	a.gfx.cleaner_shader_size = sizeof(gfx_9_4_2_cleaner_shader_hex);
	a.gfx.enable_cleaner_shader = cleaner_on;
	a.gfx.cleaner_shader_gpu_addr = cleaner_gpu;
	a.gfx.cleaner_shader_cpu_ptr = calloc(1, a.gfx.cleaner_shader_size);
	a.usec_timeout = AMDGPU_MAX_USEC_TIMEOUT;
	a.firmware.load_type = AMDGPU_FW_LOAD_PSP;
	a.gfx.rlc.funcs = &rlc_funcs;
	a.gfx.rlc.cs_data = gfx9_cs_data;
	a.gfx.rlc.cs_ptr = calloc(1, a.gfx.rlc.clear_state_size * 4);
	a.gfx.kiq[0].pmf = &kiq_pm4_funcs;
	/* gfx_v9_0_gfx_funcs has no init_spm_golden. */
	static const struct { int (*init_spm_golden)(struct amdgpu_device *adev); } gfx_funcs;
	a.gfx.funcs = (void *)&gfx_funcs;
	a.gfx.gfx_off_req_count = 1;
	a.irq.installed = true;
	read_ring(adev, &a.gfx.gfx_ring[0]);
	for (u32 i = 0; i < a.gfx.num_compute_rings; i++) {
		read_ring(adev, &a.gfx.compute_ring[i]);
		a.gfx.mec.mqd_backup[i] = calloc(1, sizeof(struct v9_mqd_allocation));
	}
	read_ring(adev, &a.gfx.kiq[0].ring);
	a.gfx.kiq[0].mqd_backup = calloc(1, sizeof(struct v9_mqd_allocation));
	static atomic_t et[3];
	a.gfx.priv_reg_irq = (struct amdgpu_irq_src){ 1, &et[0], &priv_reg_funcs };
	a.gfx.priv_inst_irq = (struct amdgpu_irq_src){ 1, &et[1], &priv_inst_funcs };
	a.gfx.bad_op_irq = (struct amdgpu_irq_src){ 1, &et[2], &bad_op_funcs };

	printf("--- gfx hw_init\n");
	printf("gfx_v9_0_hw_init %d\n", gfx_v9_0_hw_init(&blk));
	printf("rbs %u mask 0x%x cu %u ao 0x%x db_debug2 0x%x\n", a.gfx.config.num_rbs,
	       a.gfx.config.backend_enable_mask, a.gfx.cu_info.number, a.gfx.cu_info.ao_cu_mask, a.gfx.config.db_debug2);
	printf("cleaner:");
	for (u32 i = 0; i < a.gfx.cleaner_shader_size / 4; i++)
		printf(" %08x", ((u32 *)a.gfx.cleaner_shader_cpu_ptr)[i]);
	printf("\n");
	printf("csb:");
	for (u32 i = 0; i < a.gfx.rlc.clear_state_size; i++)
		printf(" %08x", a.gfx.rlc.cs_ptr[i]);
	printf("\n");
	for (int r = 0; r < nrings; r++) {
		dump_mqd(rings[r]);
		printf("ready %s %d\n", rings[r]->name, rings[r]->sched.ready);
	}
	printf("--- gfx late_init\n");
	printf("gfx_v9_0_late_init %d\n", gfx_v9_0_late_init(&blk));
	printf("--- gfx gating\n");
	gfx_v9_0_set_clockgating_state(&blk, AMD_CG_STATE_GATE);
	gfx_v9_0_set_powergating_state(&blk, AMD_PG_STATE_GATE);
	printf("gfxoff %d count %u\n", a.gfx.gfx_off_state, a.gfx.gfx_off_req_count);
	gfx_v9_0_set_powergating_state(&blk, AMD_PG_STATE_UNGATE);
	gfx_v9_0_set_clockgating_state(&blk, AMD_CG_STATE_UNGATE);
	printf("gfxoff %d count %u\n", a.gfx.gfx_off_state, a.gfx.gfx_off_req_count);
	printf("--- gfx hw_fini\n");
	printf("gfx_v9_0_hw_fini %d\n", gfx_v9_0_hw_fini(&blk));
	return 0;
}
