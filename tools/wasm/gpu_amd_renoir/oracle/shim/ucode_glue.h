#pragma once
/* What the copied microcode functions call around them. */
static inline uint32_t amdgpu_ip_version(const struct amdgpu_device *adev, uint8_t ip, uint8_t inst)
{
	return adev->ip_versions[ip][inst] & ~0xFFU;
}
