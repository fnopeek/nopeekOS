/* gfxhub_v1_0's functions for the GMC oracle, in their own translation
 * unit: gc_9_0_offset.h names its registers as mmhub_1_0_offset.h does. */
#include "oracle_adev.h"
#include "soc15_common.h"
#include "gc/gc_9_0_offset.h"
#include "gc/gc_9_0_sh_mask.h"
#include "gc/gc_9_0_default.h"
#include "vega10_enum.h"

u32 oracle_rreg32(struct amdgpu_device *adev, u32 reg);
void oracle_wreg32(struct amdgpu_device *adev, u32 reg, u32 v);
#define RREG32(r) oracle_rreg32(adev, r)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
void oracle_wreg32(struct amdgpu_device *adev, u32 reg, u32 v);
#define WREG32(r, v) oracle_wreg32(adev, r, v)
#define amdgpu_sriov_wreg(adev, reg, value, flag, hwip, inst) ((void)0)
#define REG_FIELD_SHIFT(reg, field) reg##__##field##__SHIFT
#define REG_FIELD_MASK(reg, field) reg##__##field##_MASK
#define REG_SET_FIELD(orig_val, reg, field, field_val) \
	(((orig_val) & ~REG_FIELD_MASK(reg, field)) | (REG_FIELD_MASK(reg, field) & ((field_val) << REG_FIELD_SHIFT(reg, field))))
#define lower_32_bits(n) ((u32)((n) & 0xffffffff))
#define upper_32_bits(n) ((u32)(((n) >> 16) >> 16))
#define max(a, b) ((a) > (b) ? (a) : (b))
#define min(a, b) ((a) < (b) ? (a) : (b))
/* The GART table's page directory address, as the port computed it. */
#define amdgpu_gmc_pd_addr(bo) ((bo)->pd_addr)
u64 amdgpu_gmc_vram_mc2pa(struct amdgpu_device *adev, u64 mc_addr);
#define amdgpu_dpm_set_powergating_by_smu(adev, type, gate, inst) abort()
#include "hub_gc_extracted.c"

const struct amdgpu_gfxhub_funcs oracle_gfxhub_v1_0_funcs = {
	.get_mc_fb_offset = gfxhub_v1_0_get_mc_fb_offset,
	.gart_enable = gfxhub_v1_0_gart_enable,
	.set_fault_enable_default = gfxhub_v1_0_set_fault_enable_default,
	.init = gfxhub_v1_0_init,
};

/* The preset gmc_check.rs sets alike. */
void oracle_gc_presets(struct amdgpu_device *adev)
{
	oracle_wreg32(adev, SOC15_REG_OFFSET(GC, 0, mmMC_VM_FB_OFFSET), 0x3e0);
}
