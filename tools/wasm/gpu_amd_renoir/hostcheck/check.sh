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
# The translated DML (and its dumps) must be what dmlgen.py writes today.
python3 "$MOD/dmlgen.py" --check
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
check bios oracle_bios bios_check "$BIOS"
check gpio oracle_gpio gpio_check "$BIOS"
check link oracle_link link_check "$BIOS"
# detect_edp_presence after create_links, with the registers never written
# reading all ones (panel on, HPD high), all zeros (panel off, HPD low:
# power-on through the DMCUB) or the hash.
for seed in ones zeros hash; do
    "$B/oracle_link" "$BIOS" edp $seed > "$B/edp_$seed.linux"
    "$B/link_check" "$BIOS" edp $seed > "$B/edp_$seed.port"
    if diff -u "$B/edp_$seed.linux" "$B/edp_$seed.port" | head -30; cmp -s "$B/edp_$seed.linux" "$B/edp_$seed.port"; then
        echo "edp presence ($seed): port = Linux ($(grep -c '^dmub cmd' "$B/edp_$seed.linux") DMUB commands)"
    else
        fail=1
    fi
done
# DC's clock manager on the VBIOS, with the VBIOS SMC answering two SMU
# versions (either side of 55.51.0 and 0x371500), retraining on/off, and
# the SMU's clock table handed out or not.
rustc $RF "$HERE/clk_check.rs" -o "$B/clk_check"
for a in "0x373700 0 dpm" "0x370000 1 nodpm" "0x371600 0 nodpm" "0x373300 1 dpm"; do
    "$B/oracle_clk" "$BIOS" $a > "$B/clk.linux"
    "$B/clk_check" "$BIOS" $a > "$B/clk.port"
    if diff -u "$B/clk.linux" "$B/clk.port" | head -30; cmp -s "$B/clk.linux" "$B/clk.port"; then
        echo "clk_mgr ($a): port = Linux ($(grep -c '^write' "$B/clk.linux") register writes)"
    else
        fail=1
    fi
done
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
# dm_dmub_hw_init on the DMCUB image and the VBIOS: a DMCUB fused off, one
# in reset that never answers (backdoor load), and a running one that
# answers STOP_FW and reports its boot status (PSP load). With "cmd" DC
# then sends the D0 power state: the running DMCUB consumes the ring, the
# cold one times out into the diagnostic dump.
rustc $RF "$HERE/dmubhw_check.rs" -o "$B/dmubhw_check"
for a in "off psp -" "cold direct cmd" "running psp cmd"; do
    set -- $a
    "$B/oracle_dmubhw" "$FW/amdgpu/renoir_dmcub.bin" "$BIOS" $1 $2 0xf400000000 0x80000000 0xf400100000 $3 > "$B/dmubhw.linux"
    "$B/dmubhw_check" "$FW/amdgpu/renoir_dmcub.bin" "$BIOS" $1 $2 0xf400000000 0x80000000 0xf400100000 $3 > "$B/dmubhw.port"
    if diff -u "$B/dmubhw.linux" "$B/dmubhw.port" | head -30; cmp -s "$B/dmubhw.linux" "$B/dmubhw.port"; then
        echo "dmub hw_init ($a): port = Linux ($(grep -c '^write' "$B/dmubhw.linux") register writes)"
    else
        fail=1
    fi
done
# amdgpu_dm_init up to dc_hardware_init: what reaches dc_create and the debug
# overrides, on Renoir and Raven, APU or not, DMCUB versions either side of
# the disable_dmcu list, the stutter-quirk device, and DCE 2.0.3 / 3.0 / 3.5.
rustc $RF "$HERE/dminit_check.rs" -o "$B/dminit_check"
for a in "24 0x20000 0 0x0101002b 0x02010000 0x164c 0xc1 0x17aa 0x3f1a" \
         "24 0x20000 0 0 0x02010000 0x1636 0xd1 0x1043 0x16c2" \
         "22 0x20000 1 0x01000000 0x01000000 0x15dd 0xc8 0x1002 0x15dd" \
         "22 0x20000 0 0x1 0x01000000 0x15dd 0xc1 0x1002 0x15dd" \
         "24 0x0 0 0x0101002b 0x02000300 0x164c 0xc1 0x17aa 0x3f1a" \
         "24 0x20000 0 0x0101002b 0x03050000 0x164c 0xc1 0x17aa 0x3f1a" \
         "24 0x20000 0 0x0101002b 0x03000000 0x164c 0xc1 0x17aa 0x3f1a"; do
    "$B/oracle_dminit" $a > "$B/dminit.linux"
    "$B/dminit_check" "$BIOS" $a > "$B/dminit.port"
    if diff -u "$B/dminit.linux" "$B/dminit.port" | head -30; cmp -s "$B/dminit.linux" "$B/dminit.port"; then
        echo "amdgpu_dm_init ($a): port = Linux ($(wc -l < "$B/dminit.linux") lines)"
    else
        fail=1
    fi
done
# The DC bring-up: Linux's amdgpu_dm_init (all of display/dc unchanged) and
# the port's, then dc_hardware_init with a DMUB service attached. Registers
# never written read all ones, all zeros, the hash, lit (ones with no pipe
# fused off) or boot (lit as the VBIOS leaves it; the oracle names those
# registers from Linux's own tables and the port reads them off its output).
rustc $RF "$HERE/dchw_check.rs" -o "$B/dchw_check"
for seed in ones zeros hash lit boot; do
    "$B/oracle_dchw" "$BIOS" $seed | grep -v '^ATOM BIOS:' > "$B/dchw_$seed.linux"
    "$B/dchw_check" "$BIOS" $seed "$B/dchw_$seed.linux" > "$B/dchw_$seed.port"
    if diff -u "$B/dchw_$seed.linux" "$B/dchw_$seed.port" | head -30; cmp -s "$B/dchw_$seed.linux" "$B/dchw_$seed.port"; then
        echo "dc_hardware_init ($seed): port = Linux ($(grep -c '^write' "$B/dchw_$seed.linux") register writes," \
            "$(grep -c '^dmub cmd' "$B/dchw_$seed.linux") DMUB commands)"
    else
        fail=1
    fi
done
# Link detection after it (amdgpu_dm_initialize_drm_device's link loop):
# DP AUX engines with an eDP panel behind the first link, every other HPD
# low; EDID and DPCD through DRM's and DM's own code in the oracle.
for seed in ones zeros hash lit boot; do
    "$B/oracle_dchw" "$BIOS" $seed detect | grep -v '^ATOM BIOS:' > "$B/detect_$seed.linux"
    "$B/dchw_check" "$BIOS" $seed "$B/detect_$seed.linux" detect > "$B/detect_$seed.port"
    if diff -u "$B/detect_$seed.linux" "$B/detect_$seed.port" | head -30; cmp -s "$B/detect_$seed.linux" "$B/detect_$seed.port"; then
        echo "link detection ($seed): port = Linux ($(grep -c '^write' "$B/detect_$seed.linux") register writes," \
            "$(grep -c '^aux action' "$B/detect_$seed.linux") AUX transactions)"
    else
        fail=1
    fi
done
# The commit after detection: a stream and a plane added to a copy of the
# current state, then dc_validate_global_state (pipe split, DML, watermarks,
# DLG/TTU/RQ, DML's whole state), identity and scaled plane, then
# dc_commit_streams (accelerated mode, clocks, watermarks, back end, link
# training, stream enable), dc_update_planes_and_stream (the plane's
# front end: HUBP, DPP scaler and colour, MPCC, ABM) and two page flips (a
# fast update on the current state); compared to the end, with every DMUB
# command.
for seed in ones zeros hash lit boot; do
    for plane in full scaled; do
        n=commit_$seed$plane
        "$B/oracle_dchw" "$BIOS" $seed commit $plane | grep -v '^ATOM BIOS:' > "$B/$n.linux"
        "$B/dchw_check" "$BIOS" $seed "$B/$n.linux" commit $plane > "$B/$n.port"
        for f in linux port; do
            awk '/^dc_state_add_stream/{p=1} p&&/^(dmub cmd|wm_ranges)/{exit} p{print}' "$B/$n.$f" > "$B/$n.$f.blk"
            n0=$(grep -n '^dc_state_add_stream' "$B/$n.$f" | cut -d: -f1)
            head -n $(( ${n0:-$(($(wc -l < "$B/$n.$f") + 1))} - 1 )) "$B/$n.$f" >> "$B/$n.$f.blk"
            grep '^dmub cmd' "$B/$n.$f" >> "$B/$n.$f.blk" || true
        done
        if diff -u "$B/$n.linux.blk" "$B/$n.port.blk" | head -30; cmp -s "$B/$n.linux.blk" "$B/$n.port.blk"; then
            echo "commit ($seed, $plane plane): port = Linux through two page flips" \
                "($(grep -c '^vba\.' "$B/$n.linux.blk") DML values," \
                "$(awk '/^dc_validate_global_state/{p=1} /^dc_commit_streams/{exit} p&&/^write/' "$B/$n.linux" | wc -l) commit writes," \
                "$(awk '/^dc_commit_streams/{p=1} /^dc_update_planes_and_stream/{exit} p&&/^write/' "$B/$n.linux" | wc -l) plane update writes," \
                "$(awk '/^dc_update_planes_and_stream/{p=1} p&&/^(dmub cmd|wm_ranges)/{exit} p&&/^write/' "$B/$n.linux" | wc -l) flip writes," \
                "$(grep -c '^dmub cmd' "$B/$n.linux") DMUB commands)"
        else
            fail=1
        fi
    done
done
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
for blk in gfx sdma smu soc vcn jpeg; do
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
# JPEG's ring runs IBs on VMID 0: a guest IB that fails
# amdgpu_jpeg_dec_parse_cs is refused, one that passes runs from the kernel
# copy (the 4-dword IB at a kernel address, not at the guest's VA).
if grep -q '^Unknown packet type 12 !' "$B/run.log" &&
   grep -E '^gpu jpeg_dec IB 0x[0-9a-f]+ 4 dw' "$B/run.log" | grep -vq '0x100001500 '; then
    echo "guest IB on jpeg_dec: checked in a kernel copy, refused or run from it"
else
    echo "guest IB on jpeg_dec: NOT checked in a kernel copy"
    fail=1
fi
# A guest encoder IB that hangs: the VCN engine is reset, and the next
# guest IB runs on the encoder.
if grep -q '^Ring vcn_enc0 reset succeeded' "$B/run.log" &&
   sed -n '/^Ring vcn_enc0 reset succeeded/,$p' "$B/run.log" | grep -q '^gpu vcn_enc0 IB 0x100001400 '; then
    echo "guest hang on vcn_enc0: ring reset, the next IB runs"
else
    echo "guest hang on vcn_enc0: NOT recovered by a ring reset"
    fail=1
fi
# The job timeout dumps the IP state first: the encoder hang shows the
# powered VCN's registers and the ring between rptr and wptr.
if sed -n '/^IP Type: 7 Ring Name: vcn_enc0/,/^Ring contents/p' "$B/run.log" |
   grep -q '^Active Instance:VCN0' &&
   sed -n '/^IP Type: 7 Ring Name: vcn_enc0/,/^Ring contents/p' "$B/run.log" | grep -q '^Rptr: 0x20 Wptr: 0x40 '; then
    echo "job timeout on vcn_enc0: IP state and ring dumped"
else
    echo "job timeout on vcn_enc0: NO core dump"
    fail=1
fi
# A guest job finished on a media ring is reported.
if grep -q '^native: client 100 ring [0-9]* (vcn_enc): 1 jobs done' "$B/run.log"; then
    echo "guest job on vcn_enc: reported done"
else
    echo "guest job on vcn_enc: NOT reported"
    fail=1
fi
echo "--- the run on the host (the log the card run prints; a fake SMU answers OK):"
grep -vE '^(vram |  block )' "$B/run.log"
exit $fail
