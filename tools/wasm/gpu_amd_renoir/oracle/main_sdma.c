/* The oracle for stage 4's SDMA block: Linux's sdma_v4_0 functions, with
 * the ring helpers they call, copied verbatim by extract.py. It runs the
 * block's life as the port's run has it: hw_init (ring test included),
 * late_init, clock and power gating on, both off again, and hw_fini.
 *
 * Registers are a file in which a register never written reads SEED(reg)
 * by the rule argv[1] names (ones, zeros or hash). Every register write is
 * printed, and every doorbell with the ring dwords it hands over. Behind
 * the doorbell an engine runs just what the ring test polls for: a
 * WRITE_LINEAR into the writeback slots. The ring, the register bases and
 * the block's state come on stdin from sdma4_check.rs, which runs the port
 * on the same file and prints the same lines. The SMU's SDMA power-up and
 * power-down are the SMU oracle's. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>
#include <errno.h>
typedef int atomic_t;
#include "soc15_hw_ip.h"
#include "amd_shared.h"
#include "gc/gc_9_0_offset.h"
#include "gc/gc_9_0_sh_mask.h"
#include "sdma0/sdma0_4_2_offset.h"
#include "sdma0/sdma0_4_2_sh_mask.h"
#include "sdma1/sdma1_4_2_offset.h"
#include "sdma1/sdma1_4_2_sh_mask.h"
#include "sdma0/sdma0_4_1_default.h"
#include "vega10_sdma_pkt_open.h"
#include "gfx_types.h"
/* glibc defines it on every host: the kernel's little-endian build has
 * it undefined. */
#undef __BIG_ENDIAN

#define AMDGPU_MAX_SDMA_INSTANCES 16
#define AMDGPU_MAX_WB 1024
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define lower_32_bits(n) ((u32)((n) & 0xffffffff))
#define upper_32_bits(n) ((u32)(((n) >> 16) >> 16))
#define cpu_to_le32(x) (x)
#define le32_to_cpu(x) (x)
#define WARN_ON_ONCE(c) (c)
#define WARN_ONCE(c, ...) (c)
#define mb() ((void)0)
#define spin_lock_irqsave(l, f) ((void)(f))
#define spin_unlock_irqrestore(l, f) ((void)(f))
#define WRITE_ONCE(x, v) ((x) = (v))
#define memset32(p, v, n) do { for (u32 i_ = 0; i_ < (n); i_++) (p)[i_] = (v); } while (0)
#define find_first_zero_bit(m, n) ffz_(m, n)
#define __set_bit(i, m) ((m)[(i) / 64] |= 1ull << ((i) % 64))
#define __clear_bit(i, m) ((m)[(i) / 64] &= ~(1ull << ((i) % 64)))
#define order_base_2(n) ((n) > 1 ? 32 - __builtin_clz((u32)(n) - 1) : 0)
#define udelay(n) ((void)0)
/* Messages are the port's log lines, not part of the comparison. */
#define DRM_ERROR(...) ((void)0)
#define DRM_DEBUG(...) ((void)0)
#define DRM_DEV_ERROR(d, ...) ((void)0)
#define DRM_DEV_DEBUG(d, ...) ((void)0)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#define amdgpu_sriov_wreg(adev, reg, value, flag, hwip, inst) ((void)0)
#define IP_VERSION(mj, mn, rv) (((mj) << 16) | ((mn) << 8) | (rv))
#define AMDGPU_REGS_RLC (1 << 2)
#define AMDGPU_REGS_NO_KIQ (1 << 1)
#define AMDGPU_FW_LOAD_PSP 2

static unsigned long ffz_(const unsigned long long *m, unsigned long n)
{
	for (unsigned long i = 0; i < n; i++)
		if (!((m[i / 64] >> (i % 64)) & 1))
			return i;
	return n;
}

struct amdgpu_device;
struct amdgpu_ring;
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
	u64 gpu_addr, rptr_gpu_addr, wptr_gpu_addr;
	u64 *wptr_cpu_addr;
	struct { bool ready; } sched;
	char name[16];
	/* The wptr the last doorbell handed over, for the dump. */
	u64 rung;
};
struct amdgpu_sdma_instance {
	u32 fw_version;
	struct amdgpu_ring ring, page;
	bool burst_nop;
};
struct amdgpu_irq_src { int dummy; };
struct amdgpu_device {
	void *dev;
	struct { u32 device; } *pdev;
	u32 flags, apu_flags, cg_flags, pg_flags, usec_timeout;
	u32 ip_versions[MAX_HWIP][HWIP_MAX_INSTANCE];
	u32 *reg_offset[MAX_HWIP][HWIP_MAX_INSTANCE];
	struct {
		u32 num_instances;
		bool has_page_queue;
		struct amdgpu_sdma_instance instance[AMDGPU_MAX_SDMA_INSTANCES];
		struct amdgpu_irq_src ecc_irq;
	} sdma;
	struct {
		u32 *wb;
		u64 gpu_addr;
		u32 num_wb;
		unsigned long long used[AMDGPU_MAX_WB / 64];
	} wb;
	struct { u32 load_type; } firmware;
	/* What soc15_common.h's RLC accessors ask, SR-IOV only. */
	struct { struct { void *funcs; bool rlcg_reg_access_supported; } rlc; } gfx;
};
struct amdgpu_ip_block { struct amdgpu_device *adev; };
#define amdgpu_ip_version(adev, ip, inst) ((adev)->ip_versions[ip][inst] >> 8)

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
#include "soc15_common.h"

static struct amdgpu_device a;

/* The engine behind the doorbell: the ring test's WRITE_LINEAR into the
 * writeback slots. */
static void sdma_run(struct amdgpu_ring *ring, u64 from, u64 to)
{
	for (u64 at = from; at != to;) {
		u32 hdr = ring->ring[at & ring->buf_mask];
		u32 op = hdr & 0xff, sub = (hdr >> 8) & 0xff;
		u32 len = 1;

		if (op == SDMA_OP_NOP) {
			len = 1 + ((hdr >> 16) & 0x3fff);
		} else if (op == SDMA_OP_WRITE && sub == SDMA_SUBOP_WRITE_LINEAR) {
			u64 dst = ring->ring[(at + 1) & ring->buf_mask] |
				  (u64)ring->ring[(at + 2) & ring->buf_mask] << 32;
			u32 n = (ring->ring[(at + 3) & ring->buf_mask] & 0xfffff) + 1;
			for (u32 i = 0; i < n; i++)
				a.wb.wb[(dst - a.wb.gpu_addr) / 4 + i] = ring->ring[(at + 4 + i) & ring->buf_mask];
			len = 4 + n;
		} else {
			abort();
		}
		at += len;
	}
}

static struct amdgpu_ring *rings[AMDGPU_MAX_SDMA_INSTANCES];
static int nrings;

/* The doorbell takes the wptr in bytes. */
static void amdgpu_mm_wdoorbell64(struct amdgpu_device *adev, u32 index, u64 v)
{
	for (int r = 0; r < nrings; r++) {
		struct amdgpu_ring *ring = rings[r];

		if (!ring->use_doorbell || ring->doorbell_index != index)
			continue;
		printf("doorbell 0x%x = 0x%llx:", index, (unsigned long long)v);
		for (u64 at = ring->rung; at != v / 4; at++)
			printf(" %08x", ring->ring[at & ring->buf_mask]);
		printf("\n");
		sdma_run(ring, ring->rung, v / 4);
		ring->rung = v / 4;
		return;
	}
	printf("doorbell 0x%x = 0x%llx: no ring\n", index, (unsigned long long)v);
}
#define WDOORBELL64(index, v) amdgpu_mm_wdoorbell64(adev, (index), (v))

static int amdgpu_ring_test_ring(struct amdgpu_ring *ring) { return ring->funcs->test_ring(ring); }
static void amdgpu_ring_set_wptr(struct amdgpu_ring *ring) { ring->funcs->set_wptr(ring); }

/* amdgpu_dpm_set_powergating_by_smu: SDMA's power-up and power-down go to
 * the SMU, whose messages the SMU oracle checks. */
#define AMD_IP_BLOCK_TYPE_SDMA 7
static int amdgpu_dpm_set_powergating_by_smu(struct amdgpu_device *adev, u32 block, bool gate, int inst)
{
	return 0;
}
/* No RAS block is registered on Renoir. */
#define amdgpu_ras_is_supported(adev, block) 0
#define amdgpu_persistent_edc_harvesting_supported(adev) 0
#define amdgpu_ras_reset_error_count(adev, block) (-EOPNOTSUPP)
#define amdgpu_irq_put(adev, src, type) abort()
#define sdma_v4_0_load_microcode(adev) (abort(), 0)
#define sdma_v4_0_page_resume(adev, i) abort()
static uint amdgpu_sdma_phase_quantum = 32;
/* The golden tables of other SDMA versions: their branches are not taken
 * on 4.1.2. */
static const struct soc15_reg_golden golden_settings_sdma_4[1], golden_settings_sdma_vg10[1],
	golden_settings_sdma_vg12[1], golden_settings_sdma0_4_2_init[1], golden_settings_sdma0_4_2[1],
	golden_settings_sdma1_4_2[1], golden_settings_sdma_arct[1], golden_settings_sdma_aldebaran[1],
	golden_settings_sdma_4_1[1], golden_settings_sdma_rv1[1], golden_settings_sdma_rv2[1];
static void sdma_v4_0_ring_set_wptr(struct amdgpu_ring *ring);

#include "sdma_extracted.c"

static const struct amdgpu_ring_funcs funcs_sdma = {
	.type = AMDGPU_RING_TYPE_SDMA, .align_mask = 0xff, .nop = SDMA_PKT_NOP_HEADER_OP(SDMA_OP_NOP),
	.set_wptr = sdma_v4_0_ring_set_wptr, .insert_nop = sdma_v4_0_ring_insert_nop,
	.test_ring = sdma_v4_0_ring_test_ring,
};

static u32 bases[MAX_HWIP][8];
static u64 wptr_shadow[AMDGPU_MAX_SDMA_INSTANCES];

int main(int argc, char **argv)
{
	struct amdgpu_device *adev = &a;
	static struct amdgpu_ip_block blk;
	static struct { u32 device; } pdev;
	seed_rule = argv[1][0];
	blk.adev = adev;
	a.pdev = (void *)&pdev;

	u32 nb;
	if (scanf(" inputs bases %u", &nb) != 1)
		return 1;
	for (u32 k = 0; k < nb; k++) {
		u32 hw, seg, v;
		if (scanf(" %u %u %x", &hw, &seg, &v) != 3)
			return 1;
		bases[hw][seg] = v;
		a.reg_offset[hw][0] = bases[hw];
	}
	unsigned long long wb_gpu;
	u32 page;
	if (scanf(" versions %x flags %x apu %x cg %x pg %x device %x usec %u instances %u page %u"
		  " wb %llx %u used",
		  &a.ip_versions[SDMA0_HWIP][0], &a.flags, &a.apu_flags, &a.cg_flags, &a.pg_flags, &pdev.device,
		  &a.usec_timeout, &a.sdma.num_instances, &page, &wb_gpu, &a.wb.num_wb) != 11)
		return 2;
	for (u32 k = 0; k < AMDGPU_MAX_WB / 64; k++)
		if (scanf(" %llx", &a.wb.used[k]) != 1)
			return 2;
	a.sdma.has_page_queue = page;
	a.wb.gpu_addr = wb_gpu;
	a.wb.wb = calloc(a.wb.num_wb * 8, 4);
	a.firmware.load_type = AMDGPU_FW_LOAD_PSP;
	for (u32 i = 0; i < a.sdma.num_instances; i++) {
		struct amdgpu_sdma_instance *s = &a.sdma.instance[i];
		struct amdgpu_ring *ring = &s->ring;
		unsigned long long gpu, rptr, wptr;
		u32 burst;
		if (scanf(" ring %15s %u %u %u %u %u %u %llx %llx %llx", ring->name, &s->fw_version, &burst, &ring->me,
			  &ring->doorbell_index, &ring->ring_size, &ring->max_dw, &gpu, &rptr, &wptr) != 10)
			return 3;
		s->burst_nop = burst;
		ring->adev = adev;
		ring->funcs = &funcs_sdma;
		ring->use_doorbell = true;
		ring->buf_mask = ring->ring_size / 4 - 1;
		ring->ptr_mask = ~0ULL;
		ring->ring = calloc(1, ring->ring_size);
		memset32(ring->ring, ring->funcs->nop, ring->ring_size / 4);
		ring->gpu_addr = gpu;
		ring->rptr_gpu_addr = rptr;
		ring->wptr_gpu_addr = wptr;
		ring->wptr_cpu_addr = &wptr_shadow[i];
		rings[nrings++] = ring;
	}

	printf("--- sdma hw_init\n");
	printf("sdma_v4_0_hw_init %d\n", sdma_v4_0_hw_init(&blk));
	for (int r = 0; r < nrings; r++)
		printf("ready %s %d wptr 0x%llx\n", rings[r]->name, rings[r]->sched.ready,
		       (unsigned long long)*rings[r]->wptr_cpu_addr);
	printf("wb used");
	for (u32 k = 0; k < AMDGPU_MAX_WB / 64; k++)
		printf(" %llx", a.wb.used[k]);
	printf("\n");
	printf("--- sdma late_init\n");
	printf("sdma_v4_0_late_init %d\n", sdma_v4_0_late_init(&blk));
	printf("--- sdma gating\n");
	sdma_v4_0_set_clockgating_state(&blk, AMD_CG_STATE_GATE);
	sdma_v4_0_set_powergating_state(&blk, AMD_PG_STATE_GATE);
	sdma_v4_0_set_powergating_state(&blk, AMD_PG_STATE_UNGATE);
	sdma_v4_0_set_clockgating_state(&blk, AMD_CG_STATE_UNGATE);
	printf("--- sdma hw_fini\n");
	printf("sdma_v4_0_hw_fini %d\n", sdma_v4_0_hw_fini(&blk));
	return 0;
}
