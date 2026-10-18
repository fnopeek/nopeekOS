#!/bin/sh
# The port against Linux's own code, on the binaries read off the device:
#   hostcheck/check.sh [vbios.bin [ip_discovery.bin [firmware dir]]]
# oracle/build.sh builds the oracles from the call-graph tree; the port's
# files are compiled with rustc on the host. Each pair of outputs is diffed.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
MOD=$(dirname "$HERE")
DEV=$HOME/.cache/nopeekos/amd-device
BIOS=${1:-$DEV/vbios.bin}
DISC=${2:-$DEV/ip_discovery.bin}
# The firmware as stage-module.sh stages it for the release.
FW=${3:-$(dirname "$(dirname "$(dirname "$MOD")")")/release/modules/gpu_amd_renoir.fw}
B=$DEV/hostcheck-build
"$MOD/oracle/build.sh" "$B"
rustc --edition 2024 -O --crate-type rlib --crate-name npk_sys "$HERE/npk_sys.rs" -o "$B/libnpk_sys.rlib"
RF="--edition 2024 -O --cap-lints allow -A dead_code -A non_camel_case_types -A non_snake_case --extern npk_sys=$B/libnpk_sys.rlib"
fail=0
check() {
    name=$1; oracle=$2; port=$3; input=$4
    rustc $RF "$HERE/$port.rs" -o "$B/$port"
    "$B/$oracle" "$input" > "$B/$name.linux"
    "$B/$port" "$input" > "$B/$name.port"
    if diff -u "$B/$name.linux" "$B/$name.port"; then
        echo "$name: port = Linux ($(wc -l < "$B/$name.linux") lines)"
    else
        fail=1
    fi
}
check atom oracle_atom atom_check "$BIOS"
check discovery oracle_discovery discovery_check "$DISC"
check ucode oracle_ucode ucode_check "$FW"
# Stage 2: the oracle reads the discovery binary, the port runs the whole
# amdgpu_device_init on the fake card first.
# Registers never written read all ones (a bit cleared shows), all zeros
# (a field set shows) and a hash (mixed).
rustc $RF "$HERE/common_check.rs" -o "$B/common_check"
for seed in ones zeros hash; do
    "$B/oracle_common" "$DISC" $seed > "$B/common_$seed.linux"
    "$B/common_check" "$DISC" "$BIOS" "$FW" $seed > "$B/common_$seed.port"
    if diff -u "$B/common_$seed.linux" "$B/common_$seed.port"; then
        echo "common ($seed): port = Linux ($(wc -l < "$B/common_$seed.linux") lines)"
    else
        fail=1
    fi
done
rustc $RF "$HERE/gmc_check.rs" -o "$B/gmc_check"
"$B/oracle_gmc" "$DISC" > "$B/gmc.linux"
"$B/gmc_check" "$DISC" "$BIOS" "$FW" > "$B/gmc.port"
if diff -u "$B/gmc.linux" "$B/gmc.port"; then
    echo "gmc: port = Linux ($(wc -l < "$B/gmc.linux") lines)"
else
    fail=1
fi
# The VRAM manager: the same requests through Linux's buddy.c and
# amdgpu_vram_mgr_new and through the port. A failed allocation takes no
# id on either side, so the frees stay matched.
# gmc_v9_0_hw_init: the port runs as on the card and names the inputs that
# hang on memory (GART table, scratch, dummy page); Linux's side programs
# the hubs with the same inputs. Every register write is compared.
rustc $RF "$HERE/gmc_hw_check.rs" -o "$B/gmc_hw_check"
"$B/gmc_hw_check" "$DISC" "$BIOS" "$FW" > "$B/gmc_hw.out"
inputs=$(grep '^inputs ' "$B/gmc_hw.out" | cut -d' ' -f2-)
sed -n '/^--- hw_init/,$p' "$B/gmc_hw.out" > "$B/gmc_hw.port"
"$B/oracle_gmc" "$DISC" $inputs | sed -n '/^--- hw_init/,$p' > "$B/gmc_hw.linux"
if diff -u "$B/gmc_hw.linux" "$B/gmc_hw.port" | head -30; cmp -s "$B/gmc_hw.linux" "$B/gmc_hw.port"; then
    echo "gmc hw_init: port = Linux ($(grep -c '^write' "$B/gmc_hw.linux") register writes)"
else
    fail=1
fi
rustc $RF "$HERE/vram_check.rs" -o "$B/vram_check"
python3 -I "$HERE/vram_ops.py" 0x20000000 400 > "$B/vram.ops"
"$B/oracle_vram" 0x20000000 0x20000000 < "$B/vram.ops" > "$B/vram.linux"
"$B/vram_check" 0x20000000 0x20000000 < "$B/vram.ops" > "$B/vram.port"
if diff -u "$B/vram.linux" "$B/vram.port" | head -20; [ "$(cmp -s "$B/vram.linux" "$B/vram.port"; echo $?)" = 0 ]; then
    echo "vram: port = Linux ($(wc -l < "$B/vram.linux") lines)"
else
    fail=1
fi
rustc $RF "$HERE/stage1_run.rs" -o "$B/stage1_run"
"$B/stage1_run" "$DISC" "$BIOS" "$FW" > "$B/run.log"
# The VRAM requests of the run itself, through Linux's VRAM manager.
grep -E '^(vram |  block )' "$B/run.log" > "$B/run_vram.port" || true
grep -E '^vram ' "$B/run.log" | "$B/oracle_vram" 0x20000000 0x20000000 > "$B/run_vram.linux"
if diff -u "$B/run_vram.linux" "$B/run_vram.port"; then
    echo "run vram: port = Linux ($(grep -c '^vram' "$B/run_vram.linux") requests)"
else
    fail=1
fi
echo "--- the run on the host (the log the card run prints; a fake SMU answers OK):"
grep -vE '^(vram |  block )' "$B/run.log"
exit $fail
