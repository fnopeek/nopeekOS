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
    $X "$AMD/amdgpu/amdgpu_ring.h" '#AMDGPU_MAX_RINGS'
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_init_lvl_id
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
    $X "$AMD/amdgpu/nbio_v7_0.c" '#smnNBIF_MGCG_CTRL_LCLK' nbio_7_0_read_syshub_ind_mmr nbio_7_0_write_syshub_ind_mmr \
        nbio_v7_0_enable_doorbell_selfring_aperture nbio_v7_0_ih_doorbell_range \
        nbio_v7_0_update_medium_grain_clock_gating nbio_v7_0_update_medium_grain_light_sleep nbio_v7_0_ih_control
    $X "$AMD/amdgpu/soc15.c" '#mmMP0_MISC_CGTT_CTRL0' '#mmMP0_MISC_CGTT_CTRL0_BASE_IDX' '#mmMP0_MISC_LIGHT_SLEEP_CTRL' \
        '#mmMP0_MISC_LIGHT_SLEEP_CTRL_BASE_IDX' soc15_update_drm_clock_gating soc15_update_drm_light_sleep \
        soc15_common_set_clockgating_state soc15_common_hw_fini
} > "$B/common_extracted.c"

{
    $X "$AMD/amdgpu/amdgpu_irq.h" '#AMDGPU_MAX_IRQ_SRC_ID' '#AMDGPU_IRQ_CLIENTID_MAX' amdgpu_irq_src amdgpu_irq_client
    $X "$AMD/amdgpu/amdgpu_vm.h" '#AMDGPU_VM_FAULT_STOP_ALWAYS' '#AMDGPU_MAX_VMHUBS' '#AMDGPU_GFXHUB_START' '#AMDGPU_MMHUB0_START' \
        '#AMDGPU_MMHUB1_START' '#AMDGPU_GFXHUB' '#AMDGPU_MMHUB0' '#AMDGPU_MMHUB1' amdgpu_vm_level
    $X "$AMD/amdgpu/amdgpu_gart.h" '#AMDGPU_GPU_PAGE_SHIFT'
    $X "$AMD/amdgpu/amdgpu_gmc.h" '#AMDGPU_GMC_HOLE_START' amdgpu_gart_placement amdgpu_vmhub
    $X "$AMD/amdgpu/amdgpu.h" '#AMDGPU_VBIOS_VGA_ALLOCATION'
    $X "$AMD/amdgpu/amdgpu_ih.h" amdgpu_ih_regs amdgpu_ih_ring
    $X "$AMD/amdgpu/amdgpu_psp.h" psp_reg_prog_id
    $X "$AMD/amdgpu/amdgpu_irq.h" amdgpu_interrupt_state amdgpu_irq_src_funcs
    $X "$AMD/amdgpu/amdgpu_ras.h" amdgpu_ras_block
} > "$B/gmc_types.h"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_ip_version
    $X "$AMD/amdgpu/amdgpu_ids.h" '#AMDGPU_NUM_VMID'
    $X "$AMD/amdgpu/gfxhub_v1_0.c" gfxhub_v1_0_get_mc_fb_offset gfxhub_v1_0_init gfxhub_v1_0_setup_vm_pt_regs \
        gfxhub_v1_0_init_gart_aperture_regs gfxhub_v1_0_init_system_aperture_regs gfxhub_v1_0_init_tlb_regs \
        gfxhub_v1_0_init_cache_regs gfxhub_v1_0_enable_system_domain gfxhub_v1_0_disable_identity_aperture \
        gfxhub_v1_0_setup_vmid_config gfxhub_v1_0_program_invalidation gfxhub_v1_0_gart_enable \
        gfxhub_v1_0_set_fault_enable_default gfxhub_v1_0_gart_disable
} > "$B/hub_gc_extracted.c"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_ip_version
    $X "$AMD/amdgpu/amdgpu_ids.h" '#AMDGPU_NUM_VMID'
    $X "$AMD/amdgpu/mmhub_v1_0.c" '#mmDAGB0_CNTL_MISC2_RV' '#mmDAGB0_CNTL_MISC2_RV_BASE_IDX' mmhub_v1_0_get_fb_location mmhub_v1_0_init mmhub_v1_0_setup_vm_pt_regs \
        mmhub_v1_0_init_gart_aperture_regs mmhub_v1_0_init_system_aperture_regs mmhub_v1_0_init_tlb_regs \
        mmhub_v1_0_init_cache_regs mmhub_v1_0_enable_system_domain mmhub_v1_0_disable_identity_aperture \
        mmhub_v1_0_init_saw mmhub_v1_0_setup_vmid_config mmhub_v1_0_program_invalidation \
        mmhub_v1_0_update_power_gating mmhub_v1_0_gart_enable mmhub_v1_0_set_fault_enable_default \
        mmhub_v1_0_gart_disable mmhub_v1_0_update_medium_grain_clock_gating \
        mmhub_v1_0_update_medium_grain_light_sleep mmhub_v1_0_set_clockgating
} > "$B/hub_mm_extracted.c"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_ip_version
    $X "$AMD/amdgpu/hdp_v4_0.c" '#mmHDP_MEM_POWER_CTRL' '#HDP_MEM_POWER_CTRL__IPH_MEM_POWER_CTRL_EN_MASK' \
        '#HDP_MEM_POWER_CTRL__IPH_MEM_POWER_LS_EN_MASK' '#HDP_MEM_POWER_CTRL__RC_MEM_POWER_CTRL_EN_MASK' \
        '#HDP_MEM_POWER_CTRL__RC_MEM_POWER_LS_EN_MASK' '#mmHDP_MEM_POWER_CTRL_BASE_IDX' \
        hdp_v4_0_init_registers hdp_v4_0_update_clock_gating
} > "$B/hub_hdp_extracted.c"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_ip_version
    $X "$AMD/amdgpu/vega10_ih.c" vega10_ih_init_register_offset vega10_ih_toggle_ring_interrupts \
        vega10_ih_toggle_interrupts vega10_ih_rb_cntl vega10_ih_doorbell_rptr vega10_ih_enable_ring \
        vega10_ih_irq_init vega10_ih_irq_disable vega10_ih_hw_init vega10_ih_hw_fini \
        vega10_ih_update_clockgating_state vega10_ih_set_clockgating_state
} > "$B/hub_ih_extracted.c"
{
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_ip_version
    $X "$AMD/amdgpu/athub_v1_0.c" athub_update_medium_grain_clock_gating athub_update_medium_grain_light_sleep \
        athub_v1_0_set_clockgating
} > "$B/hub_athub_extracted.c"
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
    $X "$AMD/amdgpu/amdgpu_irq.c" amdgpu_irq_update amdgpu_irq_enabled amdgpu_irq_get amdgpu_irq_put
    $X "$AMD/amdgpu/amdgpu_sdma.c" amdgpu_sdma_is_shared_inv_eng amdgpu_sdma_get_shared_ring
    $X "$AMD/amdgpu/amdgpu_gmc.c" '#AMDGPU_VMHUB_INV_ENG_BITMAP' amdgpu_gmc_allocate_vm_inv_eng amdgpu_gmc_ras_late_init
    $X "$AMD/amdgpu/amdgpu_ras.c" amdgpu_persistent_edc_harvesting_supported
    $X "$AMD/amdgpu/gmc_v9_0.c" gmc_v9_0_vm_fault_interrupt_state gmc_v9_0_late_init gmc_v9_0_set_clockgating_state \
        gmc_v9_0_gart_disable gmc_v9_0_hw_fini
} > "$B/gmc_extracted.c"

# The DMUB region layout: dmub_srv.c's functions with the types they use.
{
    $X "$AMD/display/dmub/dmub_srv.h" dmub_window_id dmub_window_memory_type dmub_status dmub_asic dmub_region \
        dmub_fb dmub_srv_region_params dmub_srv_region_info dmub_srv_memory_params dmub_srv_fb_info \
        '#DMUB_TRACE_BUFFER_SIZE'
    echo '#pragma pack(push, 1)'
    $X "$AMD/display/dmub/inc/dmub_cmd.h" '#DMUB_FW_META_MAGIC' '#DMUB_FW_META_OFFSET' dmub_fw_meta_feature_bits \
        dmub_fw_meta_info dmub_fw_meta
    echo '#pragma pack(pop)'
    $X "$AMD/display/dmub/inc/dmub_cmd.h" dmub_shared_state_feature_id '#DMUB_FW_HEADER_SHARED_STATE_SIZE' \
        '#DMUB_RB_CMD_SIZE' '#DMUB_RB_MAX_ENTRY' '#DMUB_RB_SIZE'
    $X "$AMD/amdgpu/amdgpu_ucode.h" common_firmware_header dmcub_firmware_header_v1_0
    $X "$AMD/display/amdgpu_dm/amdgpu_dm.c" '#PSP_HEADER_BYTES' '#PSP_FOOTER_BYTES'
} > "$B/dmub_types.c"
{
    $X "$AMD/display/dmub/src/dmub_srv.c" '#DMUB_STACK_SIZE' '#DMUB_CONTEXT_SIZE' '#DMUB_MAILBOX_SIZE' \
        '#DMUB_FW_STATE_SIZE' '#DMUB_SCRATCH_MEM_SIZE' '#DMUB_IB_MEM_SIZE' '#DMUB_LSDMA_RB_SIZE' \
        '#DMUB_NUM_WINDOWS' dmub_align dmub_get_fw_meta_info_from_blob dmub_get_fw_meta_info \
        dmub_srv_calc_regions_for_memory_type dmub_srv_calc_region_info dmub_srv_calc_mem_info
} > "$B/dmub_funcs.c"

# The PSP's command path: amdgpu_psp.c's functions from the ring submit
# up to the TA loads, with the types amdgpu_psp.h gives them.
{
    $X "$AMD/amdgpu/amdgpu_psp.h" '#PSP_FENCE_BUFFER_SIZE' '#PSP_CMD_BUFFER_SIZE' '#PSP_1_MEG' '#PSP_TMR_SIZE' \
        '#PSP_TMR_ALIGNMENT' '#PSP_WAITREG_CHANGED' '#PSP_WAITREG_NOVERBOSE' '#psp_ring_get_wptr' '#psp_ring_set_wptr' \
        '#psp_smu_reload_quirk' psp_shared_mem_size ta_type_id psp_ring_type psp_ring psp_bin_desc ta_mem_context \
        ta_context ta_cp_context
    $X "$L/include/uapi/drm/amdgpu_drm.h" '#AMDGPU_GEM_DOMAIN_GTT' '#AMDGPU_GEM_DOMAIN_VRAM'
} > "$B/psp_types.c"
{
    $X "$AMD/amdgpu/amdgpu_securedisplay.c" psp_securedisplay_parse_resp_status psp_prep_securedisplay_cmd_buf
    $X "$AMD/amdgpu/amdgpu_psp.c" acquire_psp_cmd_buf release_psp_cmd_buf psp_err_warn psp_ring_cmd_submit \
        psp_cmd_submit_buf psp_prep_tmr_cmd_buf psp_skip_tmr psp_tmr_load psp_get_fw_type psp_prep_load_ip_fw_cmd_buf \
        psp_execute_ip_fw_load psp_load_smu_fw psp_load_p2s_table fw_load_skip_check psp_load_non_psp_fw \
        psp_copy_fw psp_prep_ta_load_cmd_buf psp_ta_load psp_asd_initialize psp_ta_init_shared_buf \
        psp_prep_ta_invoke_cmd_buf psp_ta_invoke psp_prep_ta_unload_cmd_buf psp_ta_unload psp_ta_free_shared_buf \
        psp_hdcp_initialize psp_dtm_initialize psp_securedisplay_invoke psp_securedisplay_terminate \
        psp_securedisplay_initialize psp_ras_terminate psp_rap_terminate psp_dtm_terminate psp_hdcp_terminate \
        psp_asd_terminate psp_prep_tmr_unload_cmd_buf psp_tmr_unload psp_tmr_terminate
} > "$B/psp_funcs.c"

# Stage 4, GFX: gfx_v9_0_hw_init and late_init, the GFX gating, hw_fini,
# with the ring, KIQ, RLC and IRQ helpers they call.
{
    $X "$L/include/uapi/drm/amdgpu_drm.h" '#AMDGPU_HW_IP_GFX' '#AMDGPU_HW_IP_COMPUTE' '#AMDGPU_HW_IP_DMA' '#AMDGPU_HW_IP_UVD' \
        '#AMDGPU_HW_IP_VCE' '#AMDGPU_HW_IP_UVD_ENC' '#AMDGPU_HW_IP_VCN_DEC' '#AMDGPU_HW_IP_VCN_ENC' '#AMDGPU_HW_IP_VCN_JPEG' \
        '#AMDGPU_HW_IP_VPE'
    $X "$AMD/amdgpu/amdgpu.h" amd_hw_ip_block_type '#HWIP_MAX_INSTANCE'
    $X "$AMD/amdgpu/amdgpu_ring.h" '#AMDGPU_MAX_COMPUTE_RINGS' amdgpu_ring_type amdgpu_ring_priority_level
    $X "$AMD/amdgpu/amdgpu_gfx.h" '#AMDGPU_MAX_QUEUES' '#AMDGPU_MAX_GFX_QUEUES' '#AMDGPU_MAX_COMPUTE_QUEUES' amdgpu_gfx_pipe_priority \
        '#AMDGPU_GFX_QUEUE_PRIORITY_MINIMUM' '#AMDGPU_GFX_QUEUE_PRIORITY_MAXIMUM' amdgpu_unmap_queues_action
    $X "$AMD/amdgpu/amdgpu_irq.h" amdgpu_interrupt_state
    $X "$AMD/amdgpu/amdgpu.h" amdgpu_cp_irq '#AMDGPU_MAX_USEC_TIMEOUT'
    $X "$AMD/amdgpu/amdgpu_ids.h" '#AMDGPU_NUM_VMID'
    $X "$AMD/amdgpu/amdgpu_gart.h" '#AMDGPU_GPU_PAGE_SIZE'
    $X "$AMD/amdgpu/soc15.h" soc15_reg_golden '#SOC15_REG_ENTRY' '#SOC15_REG_GOLDEN_VALUE'
    $X "$AMD/amdgpu/gfx_v9_0.c" '#GFX9_NUM_GFX_RINGS' '#GFX9_MEC_HPD_SIZE' '#mmGCEA_PROBE_MAP' '#mmGCEA_PROBE_MAP_BASE_IDX' \
        '#DEFAULT_SH_MEM_BASES'
} > "$B/gfx_types.h"
{
    $X "$AMD/amdgpu/soc15.c" soc15_grbm_select soc15_program_register_sequence
    $X "$AMD/amdgpu/amdgpu_ring.c" amdgpu_ring_alloc amdgpu_ring_insert_nop amdgpu_ring_commit amdgpu_ring_undo \
        amdgpu_ring_test_helper
    $X "$AMD/amdgpu/amdgpu_ring.h" amdgpu_ring_clear_ring amdgpu_ring_write
    $X "$AMD/amdgpu/amdgpu_rlc.c" amdgpu_gfx_rlc_enter_safe_mode amdgpu_gfx_rlc_exit_safe_mode
    $X "$AMD/amdgpu/amdgpu_irq.c" amdgpu_irq_update amdgpu_irq_enabled amdgpu_irq_get amdgpu_irq_put
    $X "$AMD/amdgpu/amdgpu_gfx.h" amdgpu_gfx_create_bitmask
    $X "$AMD/amdgpu/amdgpu_gfx.c" amdgpu_queue_mask_bit_to_mec_queue amdgpu_gfx_parse_disable_cu \
        amdgpu_gfx_is_high_priority_compute_queue amdgpu_queue_mask_bit_to_set_resource_bit amdgpu_gfx_enable_kcq \
        amdgpu_gfx_disable_kcq amdgpu_gfx_do_off_ctrl amdgpu_gfx_off_ctrl_immediate amdgpu_gfx_cleaner_shader_init \
        amdgpu_gfx_csb_preamble_start amdgpu_gfx_csb_data_parser amdgpu_gfx_csb_preamble_end
    $X "$AMD/amdgpu/gfx_v9_0.c" golden_settings_gc_9_1_rn golden_settings_gc_9_x_common \
        gfx_v9_0_kiq_set_resources gfx_v9_0_kiq_map_queues gfx_v9_0_kiq_unmap_queues \
        gfx_v9_0_init_golden_registers gfx_v9_0_ring_test_ring check_if_enlarge_doorbell_range \
        gfx_v9_0_get_csb_size gfx_v9_0_get_csb_buffer gfx_v9_0_tiling_mode_table_init gfx_v9_0_select_se_sh \
        gfx_v9_0_get_rb_active_bitmap gfx_v9_0_setup_rb gfx_v9_0_debug_trap_config_init gfx_v9_0_init_compute_vmid \
        gfx_v9_0_init_gds_vmid gfx_v9_0_init_sq_config gfx_v9_0_constants_init gfx_v9_0_wait_for_rlc_serdes \
        gfx_v9_0_enable_gui_idle_interrupt gfx_v9_0_init_csb gfx_v9_0_enable_save_restore_machine \
        gfx_v9_0_init_gfx_power_gating gfx_v9_0_enable_sck_slow_down_on_power_up \
        gfx_v9_0_enable_sck_slow_down_on_power_down gfx_v9_0_enable_cp_power_gating \
        gfx_v9_0_enable_gfx_cg_power_gating gfx_v9_0_enable_gfx_pipeline_powergating \
        gfx_v9_0_enable_gfx_static_mg_power_gating gfx_v9_0_enable_gfx_dynamic_mg_power_gating gfx_v9_0_init_pg \
        gfx_v9_0_rlc_stop gfx_v9_0_rlc_start gfx_v9_0_rlc_resume gfx_v9_0_cp_gfx_enable gfx_v9_0_cp_gfx_start \
        gfx_v9_0_cp_gfx_resume gfx_v9_0_cp_compute_enable gfx_v9_0_kiq_setting gfx_v9_0_mqd_set_priority \
        gfx_v9_0_mqd_init gfx_v9_0_kiq_init_register gfx_v9_0_kiq_fini_register gfx_v9_0_kiq_init_queue \
        gfx_v9_0_kcq_init_queue gfx_v9_0_kiq_resume gfx_v9_0_kcq_resume gfx_v9_0_cp_resume gfx_v9_0_cp_enable \
        gfx_v9_0_init_tcp_config gfx_v9_0_hw_init gfx_v9_0_hw_fini gfx_v9_0_late_init \
        gfx_v9_0_is_rlc_enabled gfx_v9_0_set_safe_mode gfx_v9_0_unset_safe_mode gfx_v9_0_update_gfx_cg_power_gating \
        gfx_v9_0_update_gfx_mg_power_gating gfx_v9_0_update_medium_grain_clock_gating \
        gfx_v9_0_update_3d_clock_gating gfx_v9_0_update_coarse_grain_clock_gating gfx_v9_0_update_gfx_clock_gating \
        gfx_v9_0_update_spm_vmid_internal gfx_v9_0_set_powergating_state gfx_v9_0_set_clockgating_state \
        gfx_v9_0_ring_set_wptr_gfx gfx_v9_0_ring_set_wptr_compute gfx_v9_ring_insert_nop \
        gfx_v9_0_set_gfx_eop_interrupt_state gfx_v9_0_set_compute_eop_interrupt_state gfx_v9_0_get_cpc_int_cntl \
        gfx_v9_0_set_priv_reg_fault_state gfx_v9_0_set_bad_op_fault_state gfx_v9_0_set_priv_inst_fault_state \
        gfx_v9_0_set_eop_interrupt_state gfx_v9_0_set_user_cu_inactive_bitmap gfx_v9_0_get_cu_active_bitmap \
        gfx_v9_0_get_cu_info
} > "$B/gfx_extracted.c"

# Stage 4, SDMA: sdma_v4_0_hw_init with its ring test, late_init, gating,
# hw_fini.
{
    $X "$AMD/amdgpu/soc15.c" soc15_program_register_sequence
    $X "$AMD/amdgpu/amdgpu_ring.c" amdgpu_ring_alloc amdgpu_ring_commit amdgpu_ring_test_helper
    $X "$AMD/amdgpu/amdgpu_ring.h" amdgpu_ring_write
    $X "$AMD/amdgpu/amdgpu_sdma.c" amdgpu_sdma_get_instance_from_ring
    $X "$AMD/amdgpu/amdgpu_device.c" amdgpu_device_wb_get amdgpu_device_wb_free
    $X "$AMD/amdgpu/sdma_v4_0.c" '#WREG32_SDMA' '#RREG32_SDMA' golden_settings_sdma_4_3 sdma_v4_0_get_reg_offset \
        sdma_v4_0_init_golden_registers sdma_v4_0_setup_ulv sdma_v4_0_ring_set_wptr sdma_v4_0_ring_insert_nop \
        sdma_v4_0_gfx_enable sdma_v4_0_rlc_stop sdma_v4_0_page_stop sdma_v4_0_ctx_switch_enable sdma_v4_0_enable \
        sdma_v4_0_rb_cntl sdma_v4_0_gfx_resume sdma_v4_1_update_power_gating sdma_v4_1_init_power_gating \
        sdma_v4_0_init_pg sdma_v4_0_rlc_resume sdma_v4_0_start sdma_v4_0_ring_test_ring sdma_v4_0_late_init \
        sdma_v4_0_hw_init sdma_v4_0_hw_fini sdma_v4_0_update_medium_grain_clock_gating \
        sdma_v4_0_update_medium_grain_light_sleep sdma_v4_0_set_clockgating_state sdma_v4_0_set_powergating_state
} > "$B/sdma_extracted.c"

# Stage 6, VCN: vcn_v2_0's hw_init with its ring tests, the idle work that
# gates it, a use that starts it again, hw_fini; DPG mode with the SRAM
# the PSP loads.
{
    $X "$AMD/amdgpu/amdgpu_ucode.h" AMDGPU_UCODE_ID
    $X "$AMD/amdgpu/amdgpu_vcn.h" '#VCN_DEC_KMD_CMD' '#VCN_DEC_CMD_FENCE' '#VCN_DEC_CMD_TRAP' \
        '#VCN_DEC_CMD_WRITE_REG' '#VCN_DEC_CMD_REG_READ_COND_WAIT' '#VCN_DEC_CMD_PACKET_START' \
        '#VCN_DEC_CMD_PACKET_END' '#VCN_ENC_CMD_NO_OP' '#VCN_ENC_CMD_END' '#VCN_ENC_CMD_IB' '#VCN_ENC_CMD_FENCE' \
        '#VCN_ENC_CMD_TRAP' '#VCN_ENC_CMD_REG_WRITE' '#VCN_ENC_CMD_REG_WAIT' '#AMDGPU_VCN_STACK_SIZE' \
        '#AMDGPU_VCN_CONTEXT_SIZE' '#VCN_AON_SOC_ADDRESS_2_0' '#VCN_VID_IP_ADDRESS_2_0' '#VCN_AON_IP_ADDRESS_2_0' \
        '#SOC15_DPG_MODE_OFFSET' '#WREG32_SOC15_DPG_MODE' '#mmUVD_RBC_XX_IB_REG_CHECK' \
        '#mmUVD_RBC_XX_IB_REG_CHECK_BASE_IDX' '#mmUVD_REG_XX_MASK' '#mmUVD_REG_XX_MASK_BASE_IDX' fw_queue_mode engine_status_constants \
        internal_dpg_state dpg_pause_state amdgpu_vcn_reg amdgpu_fw_shared_rb_ptrs_struct \
        amdgpu_fw_shared_multi_queue amdgpu_fw_shared_sw_ring amdgpu_fw_shared_fw_logging \
        amdgpu_fw_shared_smu_interface_info amdgpu_fw_shared
    $X "$AMD/amdgpu/vcn_v2_0.c" '#VCN_VID_SOC_ADDRESS_2_0' '#VCN1_VID_SOC_ADDRESS_3_0' '#VCN1_AON_SOC_ADDRESS_3_0' \
        '#mmUVD_CONTEXT_ID_INTERNAL_OFFSET' '#mmUVD_GPCOM_VCPU_CMD_INTERNAL_OFFSET' \
        '#mmUVD_GPCOM_VCPU_DATA0_INTERNAL_OFFSET' '#mmUVD_GPCOM_VCPU_DATA1_INTERNAL_OFFSET' \
        '#mmUVD_NO_OP_INTERNAL_OFFSET' '#mmUVD_GP_SCRATCH8_INTERNAL_OFFSET' '#mmUVD_SCRATCH9_INTERNAL_OFFSET' \
        '#mmUVD_LMI_RBC_IB_VMID_INTERNAL_OFFSET' '#mmUVD_LMI_RBC_IB_64BIT_BAR_HIGH_INTERNAL_OFFSET' \
        '#mmUVD_LMI_RBC_IB_64BIT_BAR_LOW_INTERNAL_OFFSET' '#mmUVD_RBC_IB_SIZE_INTERNAL_OFFSET'
} > "$B/vcn_extracted.h"
{
    $X "$AMD/amdgpu/amdgpu_ring.c" amdgpu_ring_alloc amdgpu_ring_insert_nop amdgpu_ring_commit amdgpu_ring_test_helper
    $X "$AMD/amdgpu/amdgpu_ring.h" amdgpu_ring_write
    $X "$AMD/amdgpu/amdgpu_fence.c" amdgpu_fence_count_emitted
    $X "$AMD/amdgpu/amdgpu_device.c" amdgpu_device_wait_on_rreg
    $X "$AMD/amdgpu/nbio_v7_0.c" nbio_v7_0_vcn_doorbell_range
    $X "$AMD/amdgpu/amdgpu_vcn.c" amdgpu_vcn_get_profile amdgpu_vcn_put_profile amdgpu_vcn_idle_work_handler \
        amdgpu_vcn_ring_begin_use amdgpu_vcn_ring_end_use amdgpu_vcn_enc_ring_test_ring amdgpu_vcn_psp_update_sram
    $X "$AMD/amdgpu/vcn_v2_0.c" vcn_v2_0_hw_init vcn_v2_0_hw_fini vcn_v2_0_mc_resume vcn_v2_0_mc_resume_dpg_mode \
        vcn_v2_0_disable_clock_gating vcn_v2_0_clock_gating_dpg_mode vcn_v2_0_enable_clock_gating \
        vcn_v2_0_disable_static_power_gating vcn_v2_0_enable_static_power_gating vcn_v2_0_start_dpg_mode \
        vcn_v2_0_start vcn_v2_0_stop_dpg_mode vcn_v2_0_stop vcn_v2_0_pause_dpg_mode vcn_v2_0_dec_ring_get_rptr \
        vcn_v2_0_dec_ring_get_wptr vcn_v2_0_dec_ring_set_wptr vcn_v2_0_dec_ring_insert_nop \
        vcn_v2_0_enc_ring_get_rptr vcn_v2_0_enc_ring_get_wptr vcn_v2_0_enc_ring_set_wptr \
        vcn_v2_0_dec_ring_test_ring vcn_v2_0_set_pg_state
} > "$B/vcn_extracted.c"
python3 -I "$HERE/protos.py" "$B/vcn_extracted.c" > "$B/vcn_protos.h"

# Stage 6, JPEG: jpeg_v2_0's hw_init with its ring test, the idle work, a
# use, hw_fini; and amdgpu_jpeg_dec_parse_cs over the cases the check
# hands it.
{
    $X "$AMD/amdgpu/amdgpu_jpeg.h" '#AMDGPU_MAX_JPEG_RINGS' '#JPEG_REG_RANGE_START' '#JPEG_REG_RANGE_END' \
        '#JPEG_ATOMIC_RANGE_START' '#JPEG_ATOMIC_RANGE_END' amdgpu_jpeg_reg
    $X "$AMD/amdgpu/amdgpu_vcn.h" engine_status_constants
} > "$B/jpeg_extracted.h"
{
    $X "$AMD/amdgpu/amdgpu_ring.c" amdgpu_ring_alloc amdgpu_ring_commit amdgpu_ring_test_helper
    $X "$AMD/amdgpu/amdgpu_ring.h" amdgpu_ring_write
    $X "$AMD/amdgpu/amdgpu_fence.c" amdgpu_fence_count_emitted
    $X "$AMD/amdgpu/amdgpu_device.c" amdgpu_device_wait_on_rreg
    $X "$AMD/amdgpu/nbio_v7_0.c" nbio_v7_0_vcn_doorbell_range
    $X "$AMD/amdgpu/amdgpu_jpeg.c" amdgpu_jpeg_idle_work_handler amdgpu_jpeg_ring_begin_use amdgpu_jpeg_ring_end_use \
        amdgpu_jpeg_dec_ring_test_ring amdgpu_jpeg_reg_valid amdgpu_jpeg_dec_parse_cs
    $X "$AMD/amdgpu/jpeg_v2_0.c" jpeg_v2_0_hw_init jpeg_v2_0_hw_fini jpeg_v2_0_disable_power_gating \
        jpeg_v2_0_enable_power_gating jpeg_v2_0_disable_clock_gating jpeg_v2_0_enable_clock_gating jpeg_v2_0_start \
        jpeg_v2_0_stop jpeg_v2_0_dec_ring_get_rptr jpeg_v2_0_dec_ring_get_wptr jpeg_v2_0_dec_ring_set_wptr \
        jpeg_v2_0_dec_ring_nop jpeg_v2_0_set_powergating_state
} > "$B/jpeg_extracted.c"
python3 -I "$HERE/protos.py" "$B/jpeg_extracted.c" > "$B/jpeg_protos.h"

# Stage 4, SMU: smu_cmn.c, smu_v12_0.c and renoir_ppt.c are compiled whole
# against shim_smu/amdgpu.h; of amdgpu_smu.c the IP block's life is
# copied, with amdgpu_dpm.c's way into it and the HDP invalidate its table
# reads use.
{
    $X "$AMD/amdgpu/amdgpu.h" amd_hw_ip_block_type '#HWIP_MAX_INSTANCE' amd_reset_method
    $X "$AMD/pm/inc/amdgpu_dpm.h" ip_power_state amdgpu_dpm_thermal config_table_setting
} > "$B/smu_oracle_types.h"
{
    $X "$AMD/pm/swsmu/amdgpu_smu.c" smu_set_funcs smu_early_init is_vcn_enabled smu_dpm_set_vcn_enable \
        smu_dpm_set_jpeg_enable smu_dpm_set_vpe_enable smu_dpm_set_isp_enable smu_dpm_set_umsch_mm_enable \
        smu_set_mall_enable smu_set_gfx_imu_enable smu_set_gfx_power_up_by_imu smu_set_gfx_cgpg \
        smu_power_profile_mode_get smu_power_profile_mode_put smu_init_power_profile smu_feature_cap_init \
        smu_set_user_clk_dependencies smu_wbrf_event_handler \
        smu_init_fb_allocations smu_alloc_memory_pool smu_alloc_dummy_read_table smu_smc_table_sw_init \
        smu_sw_init smu_start_smc_engine smu_wbrf_support_check smu_wbrf_init smu_wbrf_fini \
        smu_get_driver_allowed_feature_mask smu_get_thermal_temperature_range smu_init_xgmi_plpd_mode \
        smu_get_pm_policy smu_update_gpu_addresses smu_set_default_dpm_table smu_smc_hw_setup smu_hw_init \
        smu_convert_to_smuclk smu_force_smuclk_levels smu_bump_power_profile_mode smu_adjust_power_state_dynamic \
        smu_handle_task smu_set_ac_dc smu_apply_default_config_table_settings smu_set_power_limit \
        smu_set_fan_control_mode smu_set_fan_speed_pwm smu_set_fan_speed_rpm smu_od_edit_dpm_table \
        smu_restore_dpm_user_profile smu_late_init smu_disable_dpms smu_smc_hw_cleanup smu_set_mp1_state \
        smu_reset_mp1_state smu_hw_fini smu_dpm_set_power_gate
    $X "$AMD/pm/amdgpu_dpm.c" amdgpu_dpm_set_powergating_by_smu
    $X "$AMD/amdgpu/hdp_v4_0.c" hdp_v4_0_invalidate_hdp
} > "$B/smu_extracted.c"
python3 -I "$HERE/protos.py" "$B/smu_extracted.c" > "$B/smu_protos.h"

# The DC VBIOS parser: bios_parser2.c's construct path with the types it uses.
DB=$AMD/display/dc/bios
{
    $X "$AMD/display/include/dal_types.h" dce_version
    $X "$AMD/display/include/gpio_types.h" gpio_pin_info
} > "$B/bios_types_std.h"
{
    $X "$AMD/display/include/grph_object_id.h" object_type object_enum_id generic_id encoder_id connector_id \
        graphics_object_id dal_graphics_object_id_init dal_graphics_object_id_get_connector_id
    $X "$AMD/display/include/grph_object_ctrl_defs.h" ddi_channel_mapping '#NUMBER_OF_UCHAR_FOR_GUID' \
        '#MAX_NUMBER_OF_EXT_DISPLAY_PATH' '#NUMBER_OF_CSR_M3_ARB' '#NUMBER_OF_DISP_CLK_VOLTAGE' \
        '#NUMBER_OF_AVAILABLE_SCLK' i2c_reg_info edp_info integrated_info dc_firmware_info dc_vram_info \
        step_and_delay_info spread_spectrum_info dal_device_type device_id graphics_object_i2c_info \
        graphics_object_hpd_info connector_device_tag_info
} > "$B/bios_types_grph.h"
{
    $X "$AMD/display/include/bios_parser_types.h" bp_result bp_soc_bb_info as_signal_type bp_disp_connector_caps_info \
        bp_encoder_cap_info
    $X "$AMD/display/include/bios_parser_interface.h" bp_init_data
    $X "$DB/bios_parser_types_internal2.h" atom_data_revision object_info_table '#BP_FROM_DCB'
    $X "$DB/bios_parser_helper.h" '#GET_IMAGE' '#BIOS_MAX_NUM_RECORD'
    $X "$AMD/display/dc/dc_types.h" dc_golden_table
} > "$B/bios_types.h"
{
    $X "$DB/bios_parser_helper.c" bios_get_image
    $X "$DB/bios_parser_common.c" object_type_from_bios_object_id enum_id_from_bios_object_id \
        gpu_id_from_bios_object_id encoder_id_from_bios_object_id connector_id_from_bios_object_id \
        generic_id_from_bios_object_id id_from_bios_object_id object_id_from_bios_object_id
    $X "$DB/bios_parser2.c" '#LAST_RECORD_TYPE' '#SMU9_SYSPLL0_ID' '#BIOS_IMAGE_SIZE_OFFSET' '#BIOS_IMAGE_SIZE_UNIT' '#DATA_TABLES' \
        get_atom_data_table_revision bios_parser_get_firmware_info get_firmware_info_v3_1 \
        get_firmware_info_v3_2 get_firmware_info_v3_4 get_firmware_info_v3_5 get_integrated_info_v11 \
        get_integrated_info_v2_1 get_integrated_info_v2_2 construct_integrated_info get_vram_info_v23 \
        get_vram_info_v24 get_vram_info_v25 get_vram_info_v30 get_vram_info_from_umc_info_v40 \
        bios_parser_get_vram_info get_soc_bb_info_v4_4 get_soc_bb_info_v4_5 bios_parser_get_soc_bb_info \
        bios_parser_create_integrated_info bios_parser2_construct get_ss_info_v4_1 get_ss_info_v4_2 \
        get_ss_info_v4_5 bios_parser_get_spread_spectrum_info bios_parser_get_ss_entry_number \
        bios_parser_get_connectors_number bios_parser_get_connector_id bios_parser_get_src_obj get_bios_object \
        get_bios_object_from_path_v3 bios_parser_get_i2c_info get_gpio_i2c_info get_hpd_record_for_path_v3 \
        bios_parser_get_hpd_info get_hpd_record bios_parser_get_gpio_pin_info device_type_from_device_id \
        bios_parser_get_device_tag get_support_mask_for_device_id bios_parser_is_device_id_supported \
        get_disp_connector_caps_record get_connector_caps_record bios_parser_get_disp_connector_caps_info \
        get_encoder_cap_record bios_parser_get_encoder_cap_info bios_get_golden_table bios_get_atom_dc_golden_table
    $X "$DB/command_table2.c" '#GET_INDEX_INTO_MASTER_TABLE' '#BIOS_CMD_TABLE_REVISION' \
        '#BIOS_CMD_TABLE_PARA_REVISION' bios_cmd_table_para_revision init_dig_encoder_control \
        init_transmitter_control init_set_pixel_clock init_set_crtc_timing init_enable_crtc \
        init_external_encoder_control init_enable_disp_power_gating init_set_dce_clock \
        init_get_smu_clock_info init_enable_lvtma_control dal_firmware_parser_init_cmd_tbl
    $X "$DB/command_table_helper2.c" dal_bios_parser_init_cmd_tbl_helper2
} > "$B/bios_funcs.c"
python3 -I "$HERE/protos.py" "$B/bios_funcs.c" > "$B/bios_protos.h"

# DC's clock manager: rn_clk_mgr_construct and what it calls, on top of the
# VBIOS parser above.
DCLK=$AMD/display/dc/clk_mgr
{
    $X "$AMD/display/dc/inc/hw/clk_mgr.h" '#MAX_NUM_DPM_LVL' clk_limit_table_entry clk_limit_num_entries \
        clk_limit_table wm_range_table_entry nv_wm_range_entry wm_table dummy_pstate_entry clk_bw_params \
        clk_state_registers_and_bypass clk_log_info rn_clk_internal
} > "$B/clk_types.h"
{
    $X "$AMD/display/dc/basics/fixpt31_32.c" '#FRACTIONAL_PART_MASK' '#GET_INTEGER_PART' '#GET_FRACTIONAL_PART' \
        dc_fixpt_mul
    $X "$DCLK/dce100/dce_clk_mgr.c" dce_clock_read_ss_info
    $X "$AMD/display/dc/dml/dcn20/dcn20_fpu.c" '#LPDDR_MEM_RETRAIN_LATENCY' ddr4_wm_table_gs lpddr4_wm_table_gs \
        lpddr4_wm_table_with_disabled_ppt ddr4_wm_table_rn ddr4_1R_wm_table_rn lpddr4_wm_table_rn \
        dcn21_clk_mgr_set_bw_params_wm_table dcn2_1_ip dcn2_1_soc construct_low_pstate_lvl \
        dcn21_update_bw_bounding_box
    $X "$DCLK/dcn21/rn_clk_mgr_vbios_smu.c" '#REG' '#FN' '#VBIOSSMC_MSG_GetSmuVersion' \
        '#VBIOSSMC_MSG_EnableTmdp48MHzRefclkPwrDown' '#VBIOSSMC_MSG_IsPeriodicRetrainingDisabled' \
        '#VBIOSSMC_Status_BUSY' '#VBIOSSMC_Result_OK' rn_smu_wait_for_response rn_vbios_smu_send_msg_with_param \
        rn_vbios_smu_get_smu_version rn_vbios_smu_enable_48mhz_tmdp_refclk_pwrdwn \
        rn_vbios_smu_is_periodic_retraining_disabled
    echo '#undef REG'
    echo '#undef FN'
    $X "$DCLK/dcn21/rn_clk_mgr.c" '#SMU_VER_55_51_0' '#REG' get_vco_frequency_from_reg \
        rn_dump_clk_registers_internal rn_dump_clk_registers rn_bw_params find_socclk_for_voltage \
        find_dcfclk_for_voltage rn_clk_mgr_helper_populate_bw_params rn_clk_mgr_construct
} > "$B/clk_funcs.c"
python3 -I "$HERE/protos.py" "$B/clk_funcs.c" > "$B/clk_protos.h"

# DC sources compiled unchanged: a dm_services.h of the oracle's own
# (shim_dc/dm_services_base.h + the field helpers of Linux's dm_services.h)
# and dc_helper.c's register helpers.
DCS=$AMD/display/dc
mkdir -p "$B/dc_shim"
{
    echo '#pragma once'
    echo '#include "dm_services_base.h"'
    $X "$DCS/dm_services.h" get_reg_field_value_ex set_reg_field_value_ex
    $X "$DCS/dc_types.h" dce_environment
    $X "$DCS/irq_types.h" dc_irq_source
    $X "$DCS/dc_ddc_types.h" gpio_ddc_hw_info ddc
} > "$B/dc_shim/dm_services.h"
{
    echo '#include "dm_services.h"'
    echo 'static uint32_t dmub_reg_value_pack(const struct dc_context *c, uint32_t a, void *m) { abort(); }'
    echo 'static bool dmub_reg_value_burst_set_pack(const struct dc_context *c, uint32_t a, uint32_t v) { abort(); }'
    sed '/note:  va version of this/,$d' "$DCS/dc_helper.c" > "$B/dc_helper_head.c"
    $X "$B/dc_helper_head.c" dc_reg_value_masks set_reg_field_value_masks set_reg_field_values generic_reg_update_ex \
        generic_reg_set_ex generic_reg_get generic_reg_get2 generic_reg_get3 generic_reg_get4 generic_reg_get5 \
        generic_reg_get6 generic_reg_get7 generic_reg_get8
} > "$B/dc_helper_extracted.c"

# The GPIO pins of a link: Linux's gpio/ sources unchanged, with
# link_get_hpd_gpio and the DDC service around them.
{
    $X "$DCS/dc_ddc_types.h" ddc_flags ddc_transaction_type display_dongle_type ddc_wa '#DC_MAX_EDID_BUFFER_SIZE' ddc_service
    $X "$DCS/inc/link_service.h" ddc_service_init_data
    $X "$DCS/link/protocols/link_hpd.c" link_get_hpd_gpio
    $X "$DCS/link/protocols/link_ddc.c" ddc_service_construct link_create_ddc_service
} > "$B/gpio_link.c"
GPIO_CF="-I $HERE/shim_dc -I $B/dc_shim -I $AMD/display -I $DCS -I $DCS/inc -I $DCS/gpio -I $AMD/display/include -I $AMD/include/asic_reg"

# The links: construct_phy / link_create / create_links with the link
# encoder and panel control constructors, on top of the GPIO pins above.
{
    $X "$DCS/inc/hw/hw_shared.h" '#MAX_DPIA' '#MAX_CONNECTOR' '#MAX_VIRTUAL_LINKS' '#MAX_LINKS' '#MAX_DIG_LINK_ENCODERS'
    $X "$DCS/dc_hw_types.h" dc_color_depth
    $X "$DCS/dc_types.h" dc_psr_version display_endpoint_type dc_hpd_enable_select
    $X "$DCS/inc/hw/link_encoder.h" encoder_init_data encoder_feature_support link_encoder
    $X "$DCS/dio/dcn10/dcn10_link_encoder.h" dcn10_link_encoder
    $X "$DCS/inc/hw/panel_cntl.h" panel_cntl_backlight_registers panel_cntl_init_data panel_cntl
    $X "$DCS/dce/dce_panel_cntl.h" dce_panel_cntl
    $X "$DCS/inc/link_service.h" link_init_data
} > "$B/link_types.h"
{
    $X "$DCS/dce/dce_panel_cntl.c" dce_panel_cntl_construct
    $X "$DCS/dcn21/dcn21_link_encoder.c" dcn21_link_encoder_construct
    $X "$DCS/virtual/virtual_link_encoder.c" virtual_link_encoder_construct
    $X "$DCS/resource/dcn21/dcn21_resource.c" link_enc_feature map_transmitter_id_to_phy_instance \
        dcn21_link_encoder_create dcn21_panel_cntl_create
    $X "$DCS/link/protocols/link_ddc.c" ddc_service_destruct link_destroy_ddc_service get_ddc_pin
    $X "$DCS/link/protocols/link_hpd.c" get_hpd_line program_hpd_filter
    $X "$DCS/link/protocols/link_edp_panel_control.c" is_smartmux_suported
    $X "$DCS/link/link_factory.c" translate_encoder_to_transmitter get_ddc_line construct_phy link_create
    $X "$DCS/core/dc.c" get_num_of_internal_disp create_links
} > "$B/link_funcs.c"
python3 -I "$HERE/protos.py" "$B/link_funcs.c" > "$B/link_protos.h"

# The DMCUB bring-up: Linux's dmub/src compiled unchanged (shim_dmub stands
# in for os_types.h), with dm_dmub_sw_init / dm_dmub_hw_init and the DC DMUB
# handle around them.
{
    $X "$L/include/uapi/drm/amdgpu_drm.h" '#AMDGPU_GEM_DOMAIN_GTT' '#AMDGPU_GEM_DOMAIN_VRAM'
    $X "$AMD/amdgpu/amdgpu_ucode.h" common_firmware_header dmcub_firmware_header_v1_0 amdgpu_firmware_load_type \
        AMDGPU_UCODE_ID
    $X "$AMD/display/dc/dc_dmub_srv.h" dc_reg_helper_state dc_dmub_srv
    $X "$AMD/display/dc/dm_services_types.h" dm_dmub_wait_type
} > "$B/dmubhw_types.h"
{
    $X "$AMD/display/dc/dc_dmub_srv.c" dc_dmub_srv_construct dc_dmub_srv_create \
        dc_dmub_srv_reg_cmd_list_queue_execute dc_dmub_srv_fb_cmd_list_queue_execute \
        dc_dmub_srv_cmd_list_queue_execute dc_dmub_srv_wait_for_idle dc_dmub_srv_cmd_run dc_dmub_srv_cmd_run_list \
        dc_dmub_srv_optimized_init_done dc_dmub_srv_get_diagnostic_data dc_dmub_srv_log_diagnostic_data \
        dc_wake_and_execute_dmub_cmd dc_wake_and_execute_dmub_cmd_list dc_dmub_srv_notify_fw_dc_power_state
    $X "$AMD/display/amdgpu_dm/amdgpu_dm.c" '#PSP_HEADER_BYTES' '#PSP_FOOTER_BYTES' amdgpu_dm_dmub_reg_read \
        amdgpu_dm_dmub_reg_write dm_dmub_sw_init dm_dmub_hw_init dm_execute_dmub_cmd dm_execute_dmub_cmd_list
} > "$B/dmubhw_funcs.c"
python3 -I "$HERE/protos.py" "$B/dmubhw_funcs.c" > "$B/dmubhw_protos.h"
# amdgpu_dm_init up to dc_hardware_init (stage D cuts it there), with its
# quirk tables and the module parameters it reads.
{
    $X "$AMD/amdgpu/amdgpu_drv.c" amdgpu_pp_feature_mask amdgpu_dc_feature_mask
    grep -h '^int amdgpu_sg_display = \|^int amdgpu_seamless = ' "$AMD/amdgpu/amdgpu_drv.c"
    $X "$AMD/display/amdgpu_dm/amdgpu_dm_quirks.c" amdgpu_dm_quirks quirk_entries edp0_on_dp1_callback \
        aux_hpd_discon_callback dmi_quirk_table retrieve_dmi_info
    $X "$AMD/amdgpu/amdgpu_device.c" amdgpu_device_seamless_boot_supported
    $X "$AMD/display/amdgpu_dm/amdgpu_dm.c" amdgpu_stutter_quirk amdgpu_stutter_quirk_list dm_should_disable_stutter \
        dm_get_default_ips_mode
    $X "$AMD/display/amdgpu_dm/amdgpu_dm.c" amdgpu_dm_init | python3 -I -c '
import sys
t = sys.stdin.read()
cut = "\tdc_hardware_init(adev->dm.dc);\n"
assert t.count(cut) == 1
print(t[:t.index(cut)] + "\treturn 0;\nerror:\n\tamdgpu_dm_fini(adev);\n\n\treturn -EINVAL;\n}")'
} > "$B/dminit_funcs.c"
DMUB_CF="-I $HERE/shim_dmub -D_OS_TYPES_H_ -include $HERE/shim_dmub/dmub_os.h -I $AMD/display -I $AMD/display/dc \
    -I $AMD/display/include -I $AMD/include/asic_reg -I $AMD/display/dmub/inc -I $AMD/display/dmub"

CF="-O0 -w -DCONFIG_X86 -DCONFIG_X86_64 -DLITTLEENDIAN_CPU -include linux/types.h -I $HERE/shim -I $B -I $AMD/include -I $AMD/amdgpu"
gcc $CF -c "$B/atom.c" -o "$B/atom.o"
gcc $CF "$HERE/main.c" "$B/atom.o" -o "$B/oracle_atom"
gcc $CF "$HERE/main_atomexec.c" "$B/atom.o" -o "$B/oracle_atomexec"
gcc $CF "$HERE/main_dmub.c" -o "$B/oracle_dmub"
DMUB_O=""
for f in "$AMD"/display/dmub/src/*.c; do
    n=$(basename "$f" .c)
    gcc $CF $DMUB_CF -c "$f" -o "$B/dmub_$n.o"
    DMUB_O="$DMUB_O $B/dmub_$n.o"
done
gcc $CF $DMUB_CF "$HERE/main_dmubhw.c" $DMUB_O -o "$B/oracle_dmubhw"
gcc $CF $DMUB_CF "$HERE/main_dminit.c" -o "$B/oracle_dminit"
gcc $CF "$HERE/main_bios.c" "$B/atom.o" -o "$B/oracle_bios"
GPIO_O=""
for f in gpio_base gpio_service hw_gpio hw_hpd hw_ddc hw_generic dcn21/hw_factory_dcn21 dcn21/hw_translate_dcn21; do
    gcc $CF $GPIO_CF -c "$DCS/gpio/$f.c" -o "$B/gpio_${f##*/}.o"
    GPIO_O="$GPIO_O $B/gpio_${f##*/}.o"
done
gcc $CF $GPIO_CF -c "$B/dc_helper_extracted.c" -o "$B/dc_helper_extracted.o"
gcc $CF $GPIO_CF "$HERE/main_gpio.c" $GPIO_O "$B/dc_helper_extracted.o" "$B/atom.o" -o "$B/oracle_gpio"
gcc $CF $GPIO_CF "$HERE/main_link.c" $GPIO_O "$B/dc_helper_extracted.o" "$B/atom.o" -o "$B/oracle_link"
gcc $CF -I "$AMD/include/asic_reg" -I "$AMD/display/dc" -I "$AMD/display/include" "$HERE/main_clk.c" "$B/atom.o" -o "$B/oracle_clk"
gcc $CF "$HERE/main_psp.c" -o "$B/oracle_psp"
gcc $CF "$HERE/main_discovery.c" -o "$B/oracle_discovery"
gcc $CF -I "$AMD/display/include" "$HERE/main_ucode.c" -o "$B/oracle_ucode"
gcc $CF -I "$AMD/include/asic_reg" "$HERE/main_common.c" -o "$B/oracle_common"
gcc $CF -I "$AMD/include/asic_reg" -c "$HERE/hub_gc.c" -o "$B/hub_gc.o"
gcc $CF -I "$AMD/include/asic_reg" -c "$HERE/hub_mm.c" -o "$B/hub_mm.o"
gcc $CF -I "$AMD/include/asic_reg" -c "$HERE/hub_hdp.c" -o "$B/hub_hdp.o"
gcc $CF -I "$AMD/include/asic_reg" -c "$HERE/hub_ih.c" -o "$B/hub_ih.o"
gcc $CF -I "$AMD/include/asic_reg" -c "$HERE/hub_athub.c" -o "$B/hub_athub.o"
gcc $CF -I "$AMD/include/asic_reg" "$HERE/main_gmc.c" "$B/hub_gc.o" "$B/hub_mm.o" "$B/hub_hdp.o" "$B/hub_ih.o" \
    "$B/hub_athub.o" -o "$B/oracle_gmc"
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
gcc $CF -I "$AMD/include/asic_reg" "$HERE/main_gfx.c" -o "$B/oracle_gfx"
gcc $CF -I "$AMD/include/asic_reg" "$HERE/main_sdma.c" -o "$B/oracle_sdma"
gcc $CF -I "$AMD/include/asic_reg" -I "$AMD/include/ivsrcid" "$HERE/main_vcn.c" -o "$B/oracle_vcn"
gcc $CF -I "$AMD/include/asic_reg" -I "$AMD/include/ivsrcid" "$HERE/main_jpeg.c" -o "$B/oracle_jpeg"

SMUF="-O0 -w -DCONFIG_X86 -DCONFIG_X86_64 -include $HERE/shim/linux/types.h -I $HERE/shim_smu -I $HERE/shim -I $B \
    -I $AMD/pm/swsmu/inc -I $AMD/pm/swsmu -I $AMD/pm/swsmu/inc/pmfw_if -I $AMD/pm/swsmu/smu12 -I $AMD/include \
    -I $AMD/include/asic_reg -I $AMD/pm/inc -I $AMD/amdgpu -I $AMD/display/dc"
gcc $SMUF -c "$AMD/pm/swsmu/smu_cmn.c" -o "$B/smu_cmn.o"
gcc $SMUF -c "$AMD/pm/swsmu/smu12/smu_v12_0.c" -o "$B/smu_v12_0.o"
gcc $SMUF -c "$AMD/pm/swsmu/smu12/renoir_ppt.c" -o "$B/renoir_ppt.o"
gcc $SMUF "$HERE/main_smu.c" "$B/smu_cmn.o" "$B/smu_v12_0.o" "$B/renoir_ppt.o" -o "$B/oracle_smu"
