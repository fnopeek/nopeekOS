/* gfxhub_v1_0's functions for the GMC oracle, in their own translation
 * unit: gc_9_0_offset.h names its registers as mmhub_1_0_offset.h does. */
#include "oracle_adev.h"
#include "soc15_common.h"
#include "gc/gc_9_0_offset.h"
#include "gc/gc_9_0_sh_mask.h"

u32 oracle_rreg32(struct amdgpu_device *adev, u32 reg);
void oracle_wreg32(struct amdgpu_device *adev, u32 reg, u32 v);
#define RREG32(r) oracle_rreg32(adev, r)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#include "hub_gc_extracted.c"

const struct amdgpu_gfxhub_funcs oracle_gfxhub_v1_0_funcs = {
	.get_mc_fb_offset = gfxhub_v1_0_get_mc_fb_offset,
	.init = gfxhub_v1_0_init,
};

/* The preset gmc_check.rs sets alike. */
void oracle_gc_presets(struct amdgpu_device *adev)
{
	oracle_wreg32(adev, SOC15_REG_OFFSET(GC, 0, mmMC_VM_FB_OFFSET), 0x3e0);
}
