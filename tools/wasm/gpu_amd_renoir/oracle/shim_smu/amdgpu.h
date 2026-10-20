/* The part of amdgpu.h the SMU oracle's translation units use: the kernel
 * helpers they call and the fields of struct amdgpu_device they reach. The
 * register file lives in main_smu.c. */
#pragma once
#include <linux/types.h>
#include <linux/slab.h>
#include <errno.h>
#include <limits.h>
#include "soc15_hw_ip.h"
#include "amd_shared.h"

typedef uint8_t __u8; typedef uint16_t __u16; typedef uint32_t __u32; typedef uint64_t __u64;
typedef int atomic_t; typedef long long atomic64_t; typedef int spinlock_t;
struct mutex { int m; };
struct work_struct { int w; };
struct delayed_work { int w; };
struct notifier_block { void *notifier_call; };
struct amdgpu_irq_src { int dummy; };
#include "smu_oracle_types.h"
struct list_head { struct list_head *next, *prev; };
struct kref { int r; };
struct device;
struct pci_bus { int number; };
struct pci_dev { u16 device, revision; struct pci_bus *bus; };
#define pci_get_domain_bus_and_slot(d, b, f) ((struct pci_dev *)NULL)
#define pci_domain_nr(b) 0
#define pci_dev_put(p) ((void)0)
#define BITS_PER_LONG 64
#define BITS_TO_LONGS(n) (((n) + BITS_PER_LONG - 1) / BITS_PER_LONG)
#define DECLARE_BITMAP(name, bits) unsigned long name[BITS_TO_LONGS(bits)]
#define BIT(n) (1ul << (n))
#define BIT_ULL(n) (1ull << (n))
#define container_of(p, t, m) ((t *)((char *)(p) - offsetof(t, m)))
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define lower_32_bits(n) ((u32)((n) & 0xffffffff))
#define upper_32_bits(n) ((u32)(((n) >> 16) >> 16))
#define min(a, b) ((a) < (b) ? (a) : (b))
#define max(a, b) ((a) > (b) ? (a) : (b))
#define min_t(t, a, b) ((t)(a) < (t)(b) ? (t)(a) : (t)(b))
#define max_t(t, a, b) ((t)(a) > (t)(b) ? (t)(a) : (t)(b))
#define MAX(a, b) max(a, b)
#define U64_MAX UINT64_MAX
#define U32_MAX UINT32_MAX
#define PAGE_SIZE 4096
#define offset_in_page(p) ((unsigned long)(p) & (PAGE_SIZE - 1))
#define WARN_ON(c) (c)
#define WARN_ON_ONCE(c) (c)
#define WARN_ONCE(c, ...) (c)
#define unlikely(c) (c)
#define likely(c) (c)
#define fallthrough __attribute__((fallthrough))
#define typecheck(t, x) 1
#define atomic_read(a) (*(a))
#define atomic_set(a, v) (*(a) = (v))
#define atomic64_set(a, v) (*(a) = (v))
#define INIT_WORK(w, f) ((void)0)
#define INIT_DELAYED_WORK(w, f) ((void)0)
#define mutex_init(m) ((void)0)
#define mutex_lock(m) ((void)0)
#define mutex_unlock(m) ((void)0)
#define cancel_work_sync(w) ((void)0)
#define cancel_delayed_work_sync(w) ((void)0)
#define schedule_delayed_work(w, d) abort()
#define jiffies 0ul
#define time_after(a, b) 0
#define msecs_to_jiffies(m) (m)
#define ktime_get_boottime_ns() 0ull
#define udelay(n) ((void)0)
#define mdelay(n) ((void)0)
#define msleep(n) ((void)0)
#define usleep_range(a, b) ((void)0)
#define fls(x) ((x) ? 32 - __builtin_clz(x) : 0)
#define test_bit(i, m) ((((const unsigned long *)(m))[(i) / BITS_PER_LONG] >> ((i) % BITS_PER_LONG)) & 1)
#define set_bit(i, m) (((unsigned long *)(m))[(i) / BITS_PER_LONG] |= 1ul << ((i) % BITS_PER_LONG))
#define clear_bit(i, m) (((unsigned long *)(m))[(i) / BITS_PER_LONG] &= ~(1ul << ((i) % BITS_PER_LONG)))
#define bitmap_zero(m, n) memset((m), 0, BITS_TO_LONGS(n) * sizeof(long))
#define bitmap_copy(d, s, n) memcpy((d), (s), BITS_TO_LONGS(n) * sizeof(long))
#define bitmap_fill(m, n) memset((m), 0xff, BITS_TO_LONGS(n) * sizeof(long))
#define bitmap_or(d, a, b, n) do { for (int i_ = 0; i_ < BITS_TO_LONGS(n); i_++) (d)[i_] = (a)[i_] | (b)[i_]; } while (0)
#define hweight32(x) __builtin_popcount(x)
#define for_each_set_bit(b, m, n) for ((b) = 0; (b) < (n); (b)++) if (test_bit((b), (m)))
#define sysfs_emit_at(buf, at, ...) sprintf((buf) + (at), __VA_ARGS__)
#define power_supply_is_system_supplied() 1
/* Messages are the port's log lines, not part of the comparison. */
#define dev_err(d, ...) ((void)0)
#define dev_err_once(d, ...) ((void)0)
#define dev_err_ratelimited(d, ...) ((void)0)
#define dev_warn(d, ...) ((void)0)
#define dev_WARN(d, ...) ((void)0)
#define dev_info(d, ...) ((void)0)
#define dev_dbg(d, ...) ((void)0)
#define dev_dbg_once(d, ...) ((void)0)
#define DRM_ERROR(...) ((void)0)
#define DRM_INFO(...) ((void)0)
#define DRM_DEBUG(...) ((void)0)
#define pr_info(...) ((void)0)

#define IP_VERSION(mj, mn, rv) (((mj) << 16) | ((mn) << 8) | (rv))
#define IP_VERSION_MAJ(ver) ((ver) >> 16)
#define AMDGPU_FW_LOAD_PSP 2
#define SMU_DEBUG_POOL_USE_VRAM BIT(1)
#define AMDGPU_MAX_VCN_INSTANCES 4
#define AMDGPU_GEM_DOMAIN_VRAM 0x4
#define AMDGPU_GEM_DOMAIN_GTT 0x2
#define AMDGPU_REGS_NO_KIQ (1 << 1)
#define AMDGPU_REGS_RLC (1 << 2)
#define ENOTSUPP 524
#define SMU_DEBUG_HALT_ON_ERROR BIT(0)

struct amdgpu_device;
struct amdgpu_ring;
struct amdgpu_hdp_funcs {
	void (*flush_hdp)(struct amdgpu_device *adev, struct amdgpu_ring *ring);
	void (*invalidate_hdp)(struct amdgpu_device *adev, struct amdgpu_ring *ring);
};
struct amdgpu_asic_funcs {
	void (*flush_hdp)(struct amdgpu_device *adev, struct amdgpu_ring *ring);
	void (*invalidate_hdp)(struct amdgpu_device *adev, struct amdgpu_ring *ring);
};
struct amd_pm_funcs;

struct amdgpu_device {
	struct device *dev;
	struct pci_dev *pdev;
	u32 flags, apu_flags, pg_flags, usec_timeout;
	u64 cg_flags;
	u32 ip_versions[MAX_HWIP][HWIP_MAX_INSTANCE];
	u32 *reg_offset[MAX_HWIP][HWIP_MAX_INSTANCE];
	bool in_suspend, in_runpm, in_s0ix, in_s3, in_s4, scpm_enabled, no_hw_access, enable_umsch_mm;
	int num_ip_blocks;
	struct amdgpu_ip_block {
		struct amdgpu_device *adev;
		const struct amdgpu_ip_block_version { int type; } *version;
		struct { bool valid; } status;
	} ip_blocks[AMD_IP_BLOCK_TYPE_NUM];
	struct { u32 load_type; } firmware;
	struct {
		bool dpm_enabled, ac_power;
		bool no_fan;
		atomic_t pwr_state[AMD_IP_BLOCK_TYPE_NUM];
		u32 pcie_gen_mask, pcie_mlw_mask, pp_feature, smu_debug_mask, fw_version, smu_prv_buffer_size;
		struct amd_pp_display_configuration { int dummy; } pm_display_cfg;
		struct { struct amdgpu_dpm_thermal thermal; } dpm;
		struct config_table_setting config_table;
		struct mutex mutex;
	} pm;
	struct {
		void *pp_handle;
		const struct amd_pm_funcs *pp_funcs;
	} powerplay;
	struct { int num_vcn_inst; struct { u32 cur_state; } inst[AMDGPU_MAX_VCN_INSTANCES]; } vcn;
	struct { u32 cur_state; } jpeg;
	struct { struct { bool connected_to_cpu; } xgmi; bool is_app_apu; } gmc;
	struct {
		struct { const struct { void (*stop)(struct amdgpu_device *adev); } *funcs; bool rlcg_reg_access_supported; } rlc;
		struct { void *funcs; } imu;
	} gfx;
	struct { const struct amdgpu_hdp_funcs *funcs; } hdp;
	const struct amdgpu_asic_funcs *asic_funcs;
	struct { struct atom_context *atom_context; } mode_info;
};

#define amdgpu_ip_version(adev, ip, inst) ((adev)->ip_versions[ip][inst] >> 8)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_multi_vf_mode(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#define amdgpu_sriov_wreg(adev, reg, value, flag, hwip, inst) ((void)0)
#define amdgpu_passthrough(adev) 0
#define amdgpu_in_reset(adev) 0

u32 oracle_rreg(u32 reg);
void oracle_wreg(u32 reg, u32 v);
u32 oracle_rreg_pcie(u32 addr);
#define RREG32(r) oracle_rreg(r)
#define WREG32(r, v) oracle_wreg((r), (v))
#define RREG32_PCIE(a) oracle_rreg_pcie(a)
#define amdgpu_asic_flush_hdp(adev, r) \
	((adev)->asic_funcs->flush_hdp ? (adev)->asic_funcs->flush_hdp((adev), (r)) : (adev)->hdp.funcs->flush_hdp((adev), (r)))
#define amdgpu_asic_invalidate_hdp(adev, r) \
	((adev)->asic_funcs->invalidate_hdp ? (adev)->asic_funcs->invalidate_hdp((adev), (r)) : \
	 ((adev)->hdp.funcs->invalidate_hdp ? (adev)->hdp.funcs->invalidate_hdp((adev), (r)) : (void)0))
#define REG_FIELD_SHIFT(reg, field) reg##__##field##__SHIFT
#define REG_FIELD_MASK(reg, field) reg##__##field##_MASK
#define REG_SET_FIELD(orig_val, reg, field, field_val) \
	(((orig_val) & ~REG_FIELD_MASK(reg, field)) | (REG_FIELD_MASK(reg, field) & ((field_val) << REG_FIELD_SHIFT(reg, field))))
#define REG_GET_FIELD(value, reg, field) (((value) & REG_FIELD_MASK(reg, field)) >> REG_FIELD_SHIFT(reg, field))
#include "soc15_common.h"

/* No PCI bus and no RAS on the host; the mailbox never times out here. */
#define amdgpu_device_bus_status_check(adev) 0
#define amdgpu_ras_get_fed_status(adev) 0
#define amdgpu_device_halt(adev) abort()
#define pci_is_enabled(p) 1
#define amdgpu_virt_xgmi_migrate_enabled(adev) 0
#define amdgpu_asic_reset_method(adev) (abort(), 0)
#define amdgpu_asic_supports_baco(adev) (abort(), 0)
#define amdgpu_device_has_display_hardware(adev) (abort(), 0)
#define amdgpu_bo_fb_aper_addr(bo) (abort(), 0ull)
#define amd_wbrf_register_notifier(n) (abort(), 0)
#define amd_wbrf_unregister_notifier(n) (abort(), 0)
