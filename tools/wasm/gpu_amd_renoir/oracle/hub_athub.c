/* athub_v1_0's clock gating for the GMC oracle's late mode, with the ATHUB
 * headers in a unit of their own. */
#include "oracle_adev.h"
#include "soc15_common.h"
#include "athub/athub_1_0_offset.h"
#include "athub/athub_1_0_sh_mask.h"

u32 oracle_rreg32(struct amdgpu_device *adev, u32 reg);
void oracle_wreg32(struct amdgpu_device *adev, u32 reg, u32 v);
#define RREG32(r) oracle_rreg32(adev, r)
#define WREG32(r, v) oracle_wreg32(adev, r, v)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#define amdgpu_sriov_wreg(adev, reg, value, flag, hwip, inst) ((void)0)
#include "hub_athub_extracted.c"
