/* mmhub_v1_0's functions for the GMC oracle, in their own translation
 * unit: mmhub_1_0_offset.h names its registers as gc_9_0_offset.h does. */
#include "oracle_adev.h"
#include "soc15_common.h"
#include "mmhub/mmhub_1_0_offset.h"
#include "mmhub/mmhub_1_0_sh_mask.h"

u32 oracle_rreg32(struct amdgpu_device *adev, u32 reg);
void oracle_wreg32(struct amdgpu_device *adev, u32 reg, u32 v);
#define RREG32(r) oracle_rreg32(adev, r)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#include "hub_mm_extracted.c"

const struct amdgpu_mmhub_funcs oracle_mmhub_v1_0_funcs = {
	.get_fb_location = mmhub_v1_0_get_fb_location,
	.init = mmhub_v1_0_init,
};

/* The presets gmc_check.rs sets alike. */
void oracle_mm_presets(struct amdgpu_device *adev)
{
	oracle_wreg32(adev, SOC15_REG_OFFSET(MMHUB, 0, mmMC_VM_FB_LOCATION_BASE), 0xf400);
	oracle_wreg32(adev, SOC15_REG_OFFSET(MMHUB, 0, mmMC_VM_FB_LOCATION_TOP), 0xf41f);
}
