/* hdp_v4_0's init for the GMC oracle, with its own headers. */
#include "oracle_adev.h"
#include "soc15_common.h"
#include "hdp/hdp_4_0_offset.h"
#include "hdp/hdp_4_0_sh_mask.h"

u32 oracle_rreg32(struct amdgpu_device *adev, u32 reg);
void oracle_wreg32(struct amdgpu_device *adev, u32 reg, u32 v);
#define RREG32(r) oracle_rreg32(adev, r)
#define WREG32(r, v) oracle_wreg32(adev, r, v)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#define amdgpu_sriov_wreg(adev, reg, value, flag, hwip, inst) ((void)0)
#define REG_FIELD_SHIFT(reg, field) reg##__##field##__SHIFT
#define REG_FIELD_MASK(reg, field) reg##__##field##_MASK
#include "hub_hdp_extracted.c"

const struct amdgpu_hdp_funcs oracle_hdp_v4_0_funcs = { .init_registers = hdp_v4_0_init_registers };
