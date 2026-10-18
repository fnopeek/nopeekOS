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
    $X "$AMD/amdgpu/amdgpu.h" amd_hw_ip_block_type '#HWIP_MAX_INSTANCE' '#HW_ID_MAX' '#IP_VERSION_FULL' '#IP_VERSION'
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

CF="-O0 -w -DLITTLEENDIAN_CPU -include linux/types.h -I $HERE/shim -I $B -I $AMD/include -I $AMD/amdgpu"
gcc $CF -c "$B/atom.c" -o "$B/atom.o"
gcc $CF "$HERE/main.c" "$B/atom.o" -o "$B/oracle_atom"
gcc $CF "$HERE/main_discovery.c" -o "$B/oracle_discovery"
