#!/bin/sh
# The ATOM path of the port against Linux's own code, on a VBIOS image:
#   hostcheck/check.sh [vbios.bin]
# Builds the oracle (oracle/main.c with Linux's atom.c and the
# amdgpu_atomfirmware functions, taken from the call-graph tree at build
# time, so no Linux source is kept here) and the port's files with rustc on
# the host, and diffs their output.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
MOD=$(dirname "$HERE")
BIOS=${1:-$HOME/.cache/nopeekos/amd-device/vbios.bin}
L=$HOME/.cache/nopeekos/amdgpu-callgraph/linux-6.18.55
AMD=$L/drivers/gpu/drm/amd
B=$HOME/.cache/nopeekos/amd-device/hostcheck-build
mkdir -p "$B"
cp "$AMD/amdgpu/atom.c" "$AMD/amdgpu/atom.h" "$B/"
echo '#pragma once' > "$B/amdgpu.h"
F=$AMD/amdgpu/amdgpu_atomfirmware.c
python3 -I "$MOD/oracle/extract.py" "$F" firmware_info amdgpu_atomfirmware_query_firmware_capability \
    amdgpu_atomfirmware_scratch_regs_init amdgpu_atomfirmware_allocate_fb_v2_1 \
    amdgpu_atomfirmware_allocate_fb_v2_2 amdgpu_atomfirmware_allocate_fb_scratch smu_info umc_info \
    gfx_info amdgpu_atomfirmware_get_clock_info > "$B/fw_extracted.c"
CF="-O0 -w -include linux/types.h -I $MOD/oracle/shim -I $B -I $AMD/include -I $AMD/amdgpu -I $L/include"
gcc $CF -c "$B/atom.c" -o "$B/atom.o"
gcc $CF "$MOD/oracle/main.c" "$B/atom.o" -o "$B/oracle"
rustc --edition 2024 -O --crate-type rlib --crate-name npk_sys "$HERE/npk_sys.rs" -o "$B/libnpk_sys.rlib"
rustc --edition 2024 -O --cap-lints allow -A dead_code -A non_camel_case_types -A non_snake_case \
    --extern npk_sys="$B/libnpk_sys.rlib" "$HERE/atom_check.rs" -o "$B/atom_check"
"$B/oracle" "$BIOS" > "$B/linux.txt"
"$B/atom_check" "$BIOS" > "$B/port.txt"
if diff -u "$B/linux.txt" "$B/port.txt"; then
    echo "port = Linux ($(wc -l < "$B/linux.txt") lines)"
fi
