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
rustc $RF "$HERE/stage1_run.rs" -o "$B/stage1_run"
echo "--- stage 1 on the host (the log the card run prints):"
"$B/stage1_run" "$DISC" "$BIOS" "$FW"
exit $fail
