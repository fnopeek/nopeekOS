#!/bin/bash
# Das Tor vor JEDER Freigabe des Treibers.
#
# Es gab die sechs Pruefer und daneben `tools/forge-gate.py`, und genau
# das war der Fehler: vor 0.58.1, 0.58.2 und 0.59.0 lief das Forge-Tor
# nicht mit, weil es an einer anderen Stelle steht und man es vergessen
# KANN. Ein Tor, an das man sich erinnern muss, ist keins.
#
# forge-gate steht bewusst ZULETZT: es prueft das uebersetzte Modul,
# also braucht es einen frischen Build. Wer die Quelle aendert und nur
# das Tor faehrt, prueft das Modul von gestern.
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
