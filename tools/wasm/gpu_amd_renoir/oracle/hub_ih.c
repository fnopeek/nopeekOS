/* vega10_ih's functions for the GMC oracle's late mode, with the OSSSYS
 * headers in a unit of their own. */
#include "oracle_adev.h"
#include "soc15_common.h"
#include "oss/osssys_4_0_offset.h"
#include "oss/osssys_4_0_sh_mask.h"

u32 oracle_rreg32(struct amdgpu_device *adev, u32 reg);
void oracle_wreg32(struct amdgpu_device *adev, u32 reg, u32 v);
#define RREG32(r) oracle_rreg32(adev, r)
#define WREG32(r, v) oracle_wreg32(adev, r, v)
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#define amdgpu_sriov_wreg(adev, reg, value, flag, hwip, inst) ((void)0)
#define REG_FIELD_SHIFT(reg, field) reg##__##field##__SHIFT
#define REG_FIELD_MASK(reg, field) reg##__##field##_MASK
#define REG_SET_FIELD(orig_val, reg, field, field_val) \
	(((orig_val) & ~REG_FIELD_MASK(reg, field)) | (REG_FIELD_MASK(reg, field) & ((field_val) << REG_FIELD_SHIFT(reg, field))))
#define lower_32_bits(n) ((u32)((n) & 0xffffffff))
#define upper_32_bits(n) ((u32)(((n) >> 16) >> 16))
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define order_base_2(n) ((n) > 1 ? 32 - __builtin_clz((u32)(n) - 1) : 0)
#define mdelay(n) ((void)0)
/* Bus mastering is a PCI config bit, not a register of the card. */
#define pci_set_master(pdev) ((void)0)
#define psp_reg_program(psp, id, v) (abort(), 0)
#define dev_err(d, ...) ((void)0)
#include "hub_ih_extracted.c"

void oracle_ih_init_register_offset(struct amdgpu_device *adev) { vega10_ih_init_register_offset(adev); }
int oracle_ih_hw_init(struct amdgpu_ip_block *b) { return vega10_ih_hw_init(b); }
int oracle_ih_hw_fini(struct amdgpu_ip_block *b) { return vega10_ih_hw_fini(b); }
int oracle_ih_set_clockgating_state(struct amdgpu_ip_block *b, enum amd_clockgating_state s)
{
	return vega10_ih_set_clockgating_state(b, s);
}
