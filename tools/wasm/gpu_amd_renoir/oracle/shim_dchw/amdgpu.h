/* The members of struct amdgpu_device that amdgpu_dm_init and DC's
 * command tables reach (driver_context is the device). One definition for
 * every DC object of the oracle and its main. */
#pragma once
#include "amdgpu_extracted.h"
struct atom_context;
struct amdgpu_bo;
struct pci_dev { u16 vendor, device, subsystem_vendor, subsystem_device; u8 revision; };
struct amdgpu_display_manager {
	void *ddev;
	struct amdgpu_device *adev;
	struct mutex dpia_aux_lock, dc_lock, audio_lock;
	struct dc *dc;
	void *cgs_device;
	uint32_t dmcub_fw_version;
	bool aux_hpd_discon_quirk;
	bool edp0_on_dp1_quirk;
	void *bb_from_dmub;
};
struct amdgpu_device {
	struct amdgpu_display_manager dm;
	struct pci_dev *pdev;
	void *dev;
	uint32_t family, external_rev_id, asic_type, apu_flags, flags;
	struct {
		uint32_t vram_width;
		u64 fb_start, fb_end, agp_start, agp_end, gart_start, gart_end;
	} gmc;
	struct { struct amdgpu_bo *bo; } gart;
	struct { u64 vram_base_offset; } vm_manager;
	struct { struct atom_context *atom_context; bool gpu_vm_support; } mode_info;
	struct { bool keep_stolen_vga_memory; } mman;
	uint32_t ip_versions[MAX_HWIP][HWIP_MAX_INSTANCE];
	uint32_t *reg_offset[MAX_HWIP][HWIP_MAX_INSTANCE];
};
int amdgpu_atom_execute_table(struct atom_context *ctx, int index, uint32_t *params, int params_size);
bool amdgpu_atom_parse_cmd_header(struct atom_context *ctx, int index, uint8_t *frev, uint8_t *crev);
