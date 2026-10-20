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
rustc --edition 2024 -O --crate-type rlib --crate-name gpu_scene "$MOD/../gpu_scene/src/lib.rs" -o "$B/libgpu_scene.rlib"
RF="--edition 2024 -O --cap-lints allow -A dead_code -A non_camel_case_types -A non_snake_case --extern npk_sys=$B/libnpk_sys.rlib --extern gpu_scene=$B/libgpu_scene.rlib"
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
# The ATOM interpreter: the clock tables smu_v12_0 runs, every register
# access compared. Linux prefixes DRM_ERROR, the port's log does not.
rustc $RF "$HERE/atomexec_check.rs" -o "$B/atomexec_check"
for seed in ones zeros hash; do
    "$B/oracle_atomexec" "$BIOS" $seed | sed 's/^DRM_ERROR: //' > "$B/atomexec_$seed.linux"
    "$B/atomexec_check" "$BIOS" $seed > "$B/atomexec_$seed.port"
    if diff -u "$B/atomexec_$seed.linux" "$B/atomexec_$seed.port" | head -30; cmp -s "$B/atomexec_$seed.linux" "$B/atomexec_$seed.port"; then
        echo "atom exec ($seed): port = Linux ($(grep -c '^[RW] ' "$B/atomexec_$seed.linux") register accesses)"
    else
        fail=1
    fi
done
# Every command table of the image alone; a table that polls stops at the
# read cap on both sides.
n=0; acc=0
for seed in ones zeros hash; do
    for t in $(grep -o '^cmd\[[0-9]*\]' "$B/atom.linux" | tr -dc '0-9\n'); do
        "$B/oracle_atomexec" "$BIOS" $seed $t | sed 's/^DRM_ERROR: //' > "$B/table.linux"
        "$B/atomexec_check" "$BIOS" $seed $t > "$B/table.port"
        if ! cmp -s "$B/table.linux" "$B/table.port"; then
            echo "atom table $t ($seed): port differs"
            diff -u "$B/table.linux" "$B/table.port" | head -10
            fail=1
        fi
        n=$((n + 1)); acc=$((acc + $(grep -c '^[RW] ' "$B/table.linux" || true)))
    done
done
echo "atom tables: port = Linux ($n runs, $acc register accesses)"
# dm_dmub_sw_init's region layout on the DMCUB image, rebased on a
# framebuffer address.
rustc $RF "$HERE/dmub_check.rs" -o "$B/dmub_check"
VBIOS_SIZE=$(stat -c %s "$BIOS")
"$B/oracle_dmub" "$FW/amdgpu/renoir_dmcub.bin" $VBIOS_SIZE 0xf400100000 > "$B/dmub.linux"
"$B/dmub_check" "$FW/amdgpu/renoir_dmcub.bin" $VBIOS_SIZE 0xf400100000 > "$B/dmub.port"
if diff -u "$B/dmub.linux" "$B/dmub.port"; then
    echo "dmub regions: port = Linux ($(grep -c '^fb ' "$B/dmub.linux") windows)"
else
    fail=1
fi
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
# psp_hw_init: the port's run on the fake card, whose PSP signals every
# frame; Linux's command path builds its frames and commands from the
# run's inputs (buffers, TMR, ucode table, TAs). Every frame and every
# command dword is compared.
rustc $RF "$HERE/psp_check.rs" -o "$B/psp_check"
"$B/psp_check" "$DISC" "$BIOS" "$FW" > "$B/psp.out"
sed -n '/^--- psp/,$p' "$B/psp.out" > "$B/psp.port"
sed -n '1,/^--- psp/p' "$B/psp.out" | grep -v '^--- psp' | "$B/oracle_psp" 2>/dev/null | sed '/^ret /d' > "$B/psp.linux"
if diff -u "$B/psp.linux" "$B/psp.port" | head -30; cmp -s "$B/psp.linux" "$B/psp.port"; then
    echo "psp: port = Linux ($(grep -c '^cmd ' "$B/psp.linux") commands)"
else
    fail=1
fi
# Stage 4, block by block: the port runs to the block's start on the fake
# card and prints the oracle's inputs, then the block's life with every
# register write and doorbell; Linux runs the same life on the same inputs.
# soc is GMC's oracle in its late mode: IH hw_init, GMC late_init, the
# clock gating of COMMON, GMC and IH, their hw_fini.
for blk in gfx sdma smu soc; do
    rustc $RF "$HERE/${blk}4_check.rs" -o "$B/${blk}4_check"
    for seed in ones zeros hash; do
        "$B/${blk}4_check" "$DISC" "$BIOS" "$FW" $seed > "$B/$blk.$seed.out"
        sed -n "/^--- $blk /,\$p" "$B/$blk.$seed.out" > "$B/$blk.$seed.port"
        sed -n "1,/^--- $blk /p" "$B/$blk.$seed.out" | grep -v '^---' > "$B/$blk.$seed.in"
        if [ $blk = soc ]; then
            "$B/oracle_gmc" "$DISC" late $seed < "$B/$blk.$seed.in" | sed -n "/^--- $blk /,\$p" |
                grep -v ' uses VM inv eng ' > "$B/$blk.$seed.linux"
        else
            "$B/oracle_$blk" $seed < "$B/$blk.$seed.in" > "$B/$blk.$seed.linux"
        fi
        if diff -u "$B/$blk.$seed.linux" "$B/$blk.$seed.port" | head -30; cmp -s "$B/$blk.$seed.linux" "$B/$blk.$seed.port"; then
            echo "$blk ($seed): port = Linux ($(grep -c '^write' "$B/$blk.$seed.linux") register writes, $(grep -c '^doorbell' "$B/$blk.$seed.linux" || true) doorbells)"
        else
            fail=1
        fi
    done
done
# A guest frame's import against radeonsi's steps through Mesa's common
# code, under the drm-shim; then the bounds the module adds on top.
python3 -I "$MOD/shaders/gen_shaders.py" --imgdesc "$B/imgdesc"
rustc $RF "$HERE/import_check.rs" -o "$B/import_check"
SHIM=$HOME/.cache/nopeekos/gpu-shaders/build-amd/src/amd/drm-shim/libamdgpu_noop_drm_shim.so
word1=$(LD_PRELOAD=$SHIM AMDGPU_GPU_ID=renoir "$B/imgdesc" < /dev/null | sed -n 's/^device word1 \([0-9]*\).*/\1/p')
python3 -I "$HERE/import_cases.py" $word1 > "$B/import.cases"
LD_PRELOAD=$SHIM AMDGPU_GPU_ID=renoir "$B/imgdesc" < "$B/import.cases" 2>/dev/null | grep -v '^device' > "$B/import.linux"
"$B/import_check" $(printf '%x' $((word1 & 0xffff))) < "$B/import.cases" > "$B/import.port"
if diff -u "$B/import.linux" "$B/import.port" | head -30; cmp -s "$B/import.linux" "$B/import.port"; then
    echo "import: port = Mesa ($(grep -c '^case desc' "$B/import.linux") descriptors)"
else
    fail=1
fi
"$B/import_check" $(printf '%x' $((word1 & 0xffff))) bounds || fail=1
# The scene stream against RADV's recording, word for word.
rustc $RF "$HERE/stream_check.rs" -o "$B/stream_check"
"$B/stream_check" || fail=1
rustc $RF "$HERE/stage1_run.rs" -o "$B/stage1_run"
timeout 300 "$B/stage1_run" "$DISC" "$BIOS" "$FW" > "$B/run.log"
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
