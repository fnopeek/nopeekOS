#!/bin/bash
# Gate before every driver release: the six checkers, a build, and
# tools/forge-gate.py, all in one place so none can be skipped.
#
# forge-gate runs last because it checks the compiled module and needs a
# fresh build; otherwise it would check a stale module.
set -u
cd "$(dirname "$0")"
fehler=0

for p in check_regs seqdiff txpwrcheck cfgcheck framecheck datapath; do
    printf '%-12s ' "$p"
    if out=$(python3 "$p.py" 2>&1); then
        echo "ok"
    else
        echo "NEIN"
        echo "$out" | tail -20 | sed 's/^/             /'
        fehler=1
    fi
done

printf '%-12s ' "cargo build"
if out=$(cargo build --release --target wasm32-unknown-unknown --locked 2>&1); then
    echo "ok"
else
    echo "NEIN"
    echo "$out" | tail -20 | sed 's/^/             /'
    fehler=1
fi

printf '%-12s ' "forge-gate"
wasm=target/wasm32-unknown-unknown/release/wifi_rtl8822ce.wasm
if [ ! -f "$wasm" ]; then
    echo "NEIN — kein Modul gebaut"
    fehler=1
elif out=$(python3 ../../forge-gate.py "$wasm" 2>&1); then
    echo "ok"
else
    echo "NEIN"
    echo "$out" | tail -20 | sed 's/^/             /'
    fehler=1
fi

echo
if [ $fehler -eq 0 ]; then
    echo "alle Tore GRUEN — freigeben"
else
    echo "NICHT freigeben"
fi
exit $fehler
