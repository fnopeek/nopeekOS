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

$X "$AMD/amdgpu/amdgpu_atomfirmware.c" firmware_info amdgpu_atomfirmware_query_firmware_capability \
    amdgpu_atomfirmware_scratch_regs_init amdgpu_atomfirmware_allocate_fb_v2_1 \
    amdgpu_atomfirmware_allocate_fb_v2_2 amdgpu_atomfirmware_allocate_fb_scratch smu_info umc_info \
    gfx_info amdgpu_atomfirmware_get_clock_info > "$B/fw_extracted.c"

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

CF="-O0 -w -DLITTLEENDIAN_CPU -include linux/types.h -I $HERE/shim -I $B -I $AMD/include -I $AMD/amdgpu"
gcc $CF -c "$B/atom.c" -o "$B/atom.o"
gcc $CF "$HERE/main.c" "$B/atom.o" -o "$B/oracle_atom"
gcc $CF "$HERE/main_discovery.c" -o "$B/oracle_discovery"
gcc $CF -I "$AMD/display/include" "$HERE/main_ucode.c" -o "$B/oracle_ucode"
gcc $CF -I "$AMD/include/asic_reg" "$HERE/main_common.c" -o "$B/oracle_common"
