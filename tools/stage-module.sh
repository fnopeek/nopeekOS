#!/usr/bin/env bash
# Build a WASM app and stage it into release/modules/ so the next
# installer/OTA build picks it up. `build.sh release` only SIGNS what is
# already there — it never compiles WASM.
#
#   tools/stage-module.sh loft spell dock bar
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

for mod in "$@"; do
    src="$ROOT/tools/wasm/$mod"
    [ -d "$src" ] || { echo "no such module: $mod" >&2; exit 1; }

    ( cd "$src" && cargo build --release --target wasm32-unknown-unknown )

    wasm="$src/target/wasm32-unknown-unknown/release/${mod}.wasm"
    [ -f "$wasm" ] || { echo "not built: $wasm" >&2; exit 1; }

    ver=$(grep -m1 '^version' "$src/Cargo.toml" | cut -d'"' -f2)
    cp "$wasm" "$ROOT/release/modules/${mod}.wasm"
    echo "$ver" > "$ROOT/release/modules/${mod}.version"
    echo "staged $mod $ver ($(du -h "$wasm" | cut -f1))"
done
