#!/bin/sh
# Build the oracles into $1 from the call-graph tree: Linux's atom.c whole,
# and the amdgpu functions extract.py copies verbatim at build time, so no
# Linux source is kept in this repository.
set -e
B=$1
HERE=$(cd "$(dirname "$0")" && pwd)
L=$HOME/.cache/nopeekos/amdgpu-callgraph/linux-6.18.55
AMD=$L/drivers/gpu/drm/amd
X="python3 -I $HERE/extract.py"
mkdir -p "$B/drm"
cp "$AMD/amdgpu/atom.c" "$AMD/amdgpu/atom.h" "$B/"
cp "$L/include/drm/amd_asic_type.h" "$B/drm/"
echo '#pragma once' > "$B/amdgpu.h"
{
    $X "$L/include/uapi/drm/amdgpu_drm.h" '#AMDGPU_VRAM_TYPE_UNKNOWN' '#AMDGPU_VRAM_TYPE_DDR2' \
        '#AMDGPU_VRAM_TYPE_GDDR5' '#AMDGPU_VRAM_TYPE_HBM' '#AMDGPU_VRAM_TYPE_DDR3' '#AMDGPU_VRAM_TYPE_DDR4' \
        '#AMDGPU_VRAM_TYPE_GDDR6' '#AMDGPU_VRAM_TYPE_DDR5' '#AMDGPU_VRAM_TYPE_LPDDR4' '#AMDGPU_VRAM_TYPE_LPDDR5' \
        '#AMDGPU_VRAM_TYPE_HBM3E'
    $X "$AMD/amdgpu/amdgpu.h" '#IP_VERSION_FULL' '#IP_VERSION'
} > "$B/vram_types.h"

$X "$AMD/amdgpu/amdgpu_atomfirmware.c" firmware_info amdgpu_atomfirmware_query_firmware_capability \
    amdgpu_atomfirmware_scratch_regs_init amdgpu_atomfirmware_allocate_fb_v2_1 \
    amdgpu_atomfirmware_allocate_fb_v2_2 amdgpu_atomfirmware_allocate_fb_scratch smu_info umc_info \
    gfx_info amdgpu_atomfirmware_get_clock_info igp_info umc_info vram_info vram_module \
    convert_atom_mem_type_to_vram_type amdgpu_atomfirmware_get_vram_info > "$B/fw_extracted.c"

{
    $X "$AMD/amdgpu/amdgpu.h" amd_hw_ip_block_type '#HWIP_MAX_INSTANCE' '#HW_ID_MAX' '#IP_VERSION_FULL' '#IP_VERSION' \
        '#IP_VERSION_MAJ' '#IP_VERSION_MIN' '#IP_VERSION_REV'
    $X "$AMD/amdgpu/amdgpu_vcn.h" '#AMDGPU_MAX_VCN_INSTANCES' '#AMDGPU_VCN_HARVEST_VCN0' '#AMDGPU_VCN_HARVEST_VCN1'
    $X "$AMD/amdgpu/amdgpu_sdma.h" '#AMDGPU_MAX_SDMA_INSTANCES'
    $X "$AMD/amdgpu/amdgpu_vpe.h" '#AMDGPU_MAX_VPE_INSTANCES'
} > "$B/amdgpu_extracted.h"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_ip_version
    $X "$AMD/amdgpu/amdgpu_discovery.c" '#mmIP_DISCOVERY_VERSION' '#mmRCC_CONFIG_MEMSIZE' hw_id_map \
        amdgpu_discovery_calculate_checksum amdgpu_discovery_verify_checksum \
        amdgpu_discovery_verify_binary_signature amdgpu_discovery_get_fw_name amdgpu_discovery_init \
        amdgpu_discovery_validate_ip amdgpu_discovery_read_harvest_bit_per_ip \
        amdgpu_discovery_read_from_harvest_table amdgpu_discovery_reg_base_init \
        amdgpu_discovery_harvest_config_quirk amdgpu_discovery_harvest_ip gc_info \
        amdgpu_discovery_get_gfx_info mall_info amdgpu_discovery_get_mall_info vcn_info \
        amdgpu_discovery_get_vcn_info
} > "$B/discovery_extracted.c"

{
    $X "$AMD/amdgpu/amdgpu_ucode.c" '#AMDGPU_UCODE_NAME_MAX' kicker_device_list amdgpu_ucode_validate amdgpu_ucode_legacy_naming \
        amdgpu_is_kicker_fw amdgpu_ucode_ip_version_decode amdgpu_ucode_request amdgpu_ucode_release
    $X "$AMD/amdgpu/amdgpu_psp.c" psp_init_asd_microcode is_ta_fw_applicable parse_ta_bin_descriptor \
        parse_ta_v1_microcode parse_ta_v2_microcode psp_init_ta_microcode
    $X "$AMD/amdgpu/psp_v12_0.c" psp_v12_0_init_microcode
    $X "$AMD/amdgpu/amdgpu_gfx.c" amdgpu_gfx_cp_init_microcode
    $X "$AMD/amdgpu/amdgpu_rlc.c" amdgpu_gfx_rlc_init_microcode_v2_0 amdgpu_gfx_rlc_init_microcode_v2_1 \
        amdgpu_gfx_rlc_init_microcode_v2_2 amdgpu_gfx_rlc_init_microcode_v2_3 amdgpu_gfx_rlc_init_microcode_v2_4 \
        amdgpu_gfx_rlc_init_microcode
    $X "$AMD/amdgpu/gfx_v9_0.c" amdgpu_gfxoff_quirk amdgpu_gfxoff_quirk_list gfx_v9_0_init_cp_gfx_microcode gfx_v9_0_init_rlc_microcode \
        gfx_v9_0_load_mec2_fw_bin_support gfx_v9_0_should_disable_gfxoff gfx_v9_0_check_if_need_gfxoff \
        gfx_v9_0_check_fw_write_wait gfx_v9_0_init_cp_compute_microcode gfx_v9_0_init_microcode
    $X "$AMD/amdgpu/amdgpu_sdma.c" amdgpu_sdma_init_inst_ctx amdgpu_sdma_destroy_inst_ctx amdgpu_sdma_init_microcode
    $X "$AMD/amdgpu/sdma_v4_0.c" sdma_v4_0_init_microcode sdma_v4_0_fw_support_paging_queue
    $X "$AMD/amdgpu/amdgpu_vcn.c" amdgpu_vcn_early_init
    grep -E '^#define FIRMWARE_[A-Z0-9_]+ +"' "$AMD/display/amdgpu_dm/amdgpu_dm.c"
    $X "$AMD/display/amdgpu_dm/amdgpu_dm.c" dm_init_microcode
} > "$B/ucode_extracted.c"

{
    $X "$L/include/uapi/linux/kfd_ioctl.h" kfd_mmio_remap
    $X "$AMD/amdgpu/amdgpu.h" '#REG_FIELD_SHIFT' '#REG_FIELD_MASK' '#REG_SET_FIELD'
    $X "$AMD/amdgpu/nbio_v7_0.c" '#regRCC_DEV0_EPF6_STRAP4' '#regRCC_DEV0_EPF6_STRAP4_BASE_IDX' '#MMIO_REG_HOLE_OFFSET' \
        nbio_v7_0_set_reg_remap nbio_v7_0_init_registers nbio_v7_0_remap_hdp_registers \
        nbio_v7_0_enable_doorbell_aperture nbio_v7_0_sdma_doorbell_range \
        nbio_v7_0_get_pcie_index_offset nbio_v7_0_get_pcie_data_offset
    $X "$AMD/amdgpu/amdgpu_device.c" amdgpu_device_should_use_aspm amdgpu_device_indirect_rreg \
        amdgpu_device_indirect_wreg
    $X "$AMD/amdgpu/vega10_reg_init.c" vega10_doorbell_index_init
    $X "$AMD/amdgpu/soc15.c" soc15_program_aspm soc15_sdma_doorbell_range_init soc15_common_hw_init
} > "$B/common_extracted.c"

{
    $X "$AMD/amdgpu/amdgpu_irq.h" '#AMDGPU_MAX_IRQ_SRC_ID' '#AMDGPU_IRQ_CLIENTID_MAX' amdgpu_irq_src amdgpu_irq_client
    $X "$AMD/amdgpu/amdgpu_vm.h" '#AMDGPU_VM_FAULT_STOP_ALWAYS' '#AMDGPU_MAX_VMHUBS' '#AMDGPU_GFXHUB_START' '#AMDGPU_MMHUB0_START' \
        '#AMDGPU_MMHUB1_START' '#AMDGPU_GFXHUB' '#AMDGPU_MMHUB0' '#AMDGPU_MMHUB1' amdgpu_vm_level
    $X "$AMD/amdgpu/amdgpu_gart.h" '#AMDGPU_GPU_PAGE_SHIFT'
    $X "$AMD/amdgpu/amdgpu_gmc.h" '#AMDGPU_GMC_HOLE_START' amdgpu_gart_placement amdgpu_vmhub
    $X "$AMD/amdgpu/amdgpu.h" '#AMDGPU_VBIOS_VGA_ALLOCATION'
} > "$B/gmc_types.h"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_ip_version
    $X "$AMD/amdgpu/gfxhub_v1_0.c" gfxhub_v1_0_get_mc_fb_offset gfxhub_v1_0_init gfxhub_v1_0_setup_vm_pt_regs \
        gfxhub_v1_0_init_gart_aperture_regs gfxhub_v1_0_init_system_aperture_regs gfxhub_v1_0_init_tlb_regs \
        gfxhub_v1_0_init_cache_regs gfxhub_v1_0_enable_system_domain gfxhub_v1_0_disable_identity_aperture \
        gfxhub_v1_0_setup_vmid_config gfxhub_v1_0_program_invalidation gfxhub_v1_0_gart_enable \
        gfxhub_v1_0_set_fault_enable_default
} > "$B/hub_gc_extracted.c"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_ip_version
    $X "$AMD/amdgpu/mmhub_v1_0.c" mmhub_v1_0_get_fb_location mmhub_v1_0_init mmhub_v1_0_setup_vm_pt_regs \
        mmhub_v1_0_init_gart_aperture_regs mmhub_v1_0_init_system_aperture_regs mmhub_v1_0_init_tlb_regs \
        mmhub_v1_0_init_cache_regs mmhub_v1_0_enable_system_domain mmhub_v1_0_disable_identity_aperture \
        mmhub_v1_0_init_saw mmhub_v1_0_setup_vmid_config mmhub_v1_0_program_invalidation \
        mmhub_v1_0_update_power_gating mmhub_v1_0_gart_enable mmhub_v1_0_set_fault_enable_default
} > "$B/hub_mm_extracted.c"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_ip_version
    $X "$AMD/amdgpu/hdp_v4_0.c" hdp_v4_0_init_registers
} > "$B/hub_hdp_extracted.c"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_is_multi_aid '#REG_GET_FIELD'
    $X "$AMD/amdgpu/amdgpu_gmc.h" '#amdgpu_gmc_get_vbios_fb_size'
    $X "$AMD/amdgpu/nbio_v7_0.c" nbio_v7_0_get_memsize
    $X "$AMD/amdgpu/amdgpu_irq.c" amdgpu_irq_add_id
    $X "$AMD/amdgpu/amdgpu_vm.c" amdgpu_vm_get_block_size amdgpu_vm_adjust_size
    $X "$AMD/amdgpu/amdgpu_device.c" amdgpu_device_has_display_hardware
    $X "$AMD/amdgpu/amdgpu_gmc.c" four_gb amdgpu_gmc_is_pdb0_enabled amdgpu_gmc_set_agp_default \
        amdgpu_gmc_sysvm_location amdgpu_gmc_vram_location amdgpu_gmc_gart_location amdgpu_gmc_agp_location \
        amdgpu_gmc_get_vbios_allocations
    $X "$AMD/amdgpu/amdgpu_gmc.c" amdgpu_gmc_vram_mc2pa
    $X "$AMD/amdgpu/amdgpu_device.c" amdgpu_device_flush_hdp
    $X "$AMD/amdgpu/gmc_v9_0.c" '#mmHUBP0_DCSURF_PRI_VIEWPORT_DIMENSION' \
        '#mmHUBP0_DCSURF_PRI_VIEWPORT_DIMENSION_BASE_IDX' \
        '#HUBP0_DCSURF_PRI_VIEWPORT_DIMENSION__PRI_VIEWPORT_WIDTH__SHIFT' \
        '#HUBP0_DCSURF_PRI_VIEWPORT_DIMENSION__PRI_VIEWPORT_HEIGHT__SHIFT' \
        '#HUBP0_DCSURF_PRI_VIEWPORT_DIMENSION__PRI_VIEWPORT_WIDTH_MASK' \
        '#HUBP0_DCSURF_PRI_VIEWPORT_DIMENSION__PRI_VIEWPORT_HEIGHT_MASK' \
        '#mmHUBP0_DCSURF_PRI_VIEWPORT_DIMENSION_DCN2' '#mmHUBP0_DCSURF_PRI_VIEWPORT_DIMENSION_DCN2_BASE_IDX' \
        gmc_v9_0_get_vbios_fb_size gmc_v9_0_vram_gtt_location gmc_v9_0_mc_init gmc_v9_4_3_init_vram_info \
        gmc_v9_0_sw_init gmc_v9_0_use_invalidate_semaphore gmc_v9_0_get_invalidate_req gmc_v9_0_flush_gpu_tlb \
        gmc_v9_0_gart_enable gmc_v9_0_hw_init
} > "$B/gmc_extracted.c"

CF="-O0 -w -DCONFIG_X86 -DCONFIG_X86_64 -DLITTLEENDIAN_CPU -include linux/types.h -I $HERE/shim -I $B -I $AMD/include -I $AMD/amdgpu"
gcc $CF -c "$B/atom.c" -o "$B/atom.o"
gcc $CF "$HERE/main.c" "$B/atom.o" -o "$B/oracle_atom"
gcc $CF "$HERE/main_atomexec.c" "$B/atom.o" -o "$B/oracle_atomexec"
gcc $CF "$HERE/main_discovery.c" -o "$B/oracle_discovery"
gcc $CF -I "$AMD/display/include" "$HERE/main_ucode.c" -o "$B/oracle_ucode"
gcc $CF -I "$AMD/include/asic_reg" "$HERE/main_common.c" -o "$B/oracle_common"
gcc $CF -I "$AMD/include/asic_reg" -c "$HERE/hub_gc.c" -o "$B/hub_gc.o"
gcc $CF -I "$AMD/include/asic_reg" -c "$HERE/hub_mm.c" -o "$B/hub_mm.o"
gcc $CF -I "$AMD/include/asic_reg" -c "$HERE/hub_hdp.c" -o "$B/hub_hdp.o"
gcc $CF -I "$AMD/include/asic_reg" "$HERE/main_gmc.c" "$B/hub_gc.o" "$B/hub_mm.o" "$B/hub_hdp.o" -o "$B/oracle_gmc"
# The VRAM manager: buddy.c and tools/lib/rbtree.c whole, amdgpu_vram_mgr.c's functions.
mkdir -p "$B/buddy/linux"
cp "$L/include/linux/gpu_buddy.h" "$B/buddy/linux/"
cp "$L/drivers/gpu/buddy.c" "$B/buddy/"
$X "$L/include/linux/overflow.h" '#range_overflows' > "$B/buddy/range_overflows.h"
{
    $X "$L/include/drm/ttm/ttm_bo.h" ttm_bo_type
    $X "$AMD/amdgpu/amdgpu_vm.h" '#AMDGPU_VM_RESERVED_VRAM'
} > "$B/buddy/vram_mgr_types.h"
{
    $X "$AMD/amdgpu/amdgpu_vram_mgr.h" amdgpu_vram_mgr_block_start amdgpu_vram_mgr_block_size \
        to_amdgpu_vram_mgr_resource
    $X "$AMD/amdgpu/amdgpu_vram_mgr.c" to_vram_mgr to_amdgpu_device amdgpu_vram_mgr_first_block \
        amdgpu_is_vram_mgr_blocks_contiguous amdgpu_vram_mgr_blocks_size amdgpu_vram_mgr_vis_size \
        amdgpu_vram_mgr_do_reserve amdgpu_vram_mgr_del amdgpu_vram_mgr_new
} > "$B/buddy/vram_mgr_extracted.c"
gcc -O0 -w -include "$HERE/shim_buddy/buddy_kern.h" -I "$HERE/shim_buddy" -I "$B/buddy" -I "$L/tools/include" \
    -I "$AMD/include" -include "$L/include/uapi/drm/amdgpu_drm.h" -include "$L/include/drm/ttm/ttm_placement.h" \
    "$HERE/main_vram.c" "$L/tools/lib/rbtree.c" -o "$B/oracle_vram"
