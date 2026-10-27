#!/usr/bin/env bash
# Build a WASM app and stage it into release/modules/ so the next
# installer/OTA build picks it up. `build.sh release` only signs what is
# already there; it never compiles WASM.
#
# A module with a firmware.list brings firmware: each line is a path under
# /lib/firmware (a .zst next to it is unpacked), or `<path> <source>` for a
# file from elsewhere (a licence). It is staged to release/modules/<mod>.fw/
# and is installed at sys/firmware/<path>.
#
#   tools/stage-module.sh loft spell deck
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

for mod in "$@"; do
    src="$ROOT/tools/wasm/$mod"
    # Modules with their own core/harness (acpi, i2c_hid) keep the crate
    # one level deeper.
    if [ ! -f "$src/Cargo.toml" ] && [ -f "$src/wasm/Cargo.toml" ]; then
        src="$src/wasm"
    fi
    [ -f "$src/Cargo.toml" ] || { echo "no such module: $mod" >&2; exit 1; }

    ( cd "$src" && cargo build --release --target wasm32-unknown-unknown )

    wasm="$src/target/wasm32-unknown-unknown/release/${mod}.wasm"
    [ -f "$wasm" ] || { echo "not built: $wasm" >&2; exit 1; }

    ver=$(grep -m1 '^version' "$src/Cargo.toml" | cut -d'"' -f2)
    cp "$wasm" "$ROOT/release/modules/${mod}.wasm"
    echo "$ver" > "$ROOT/release/modules/${mod}.version"
    echo "staged $mod $ver ($(du -h "$wasm" | cut -f1))"

    fw="$ROOT/release/modules/${mod}.fw"
    rm -rf "$fw"
    if [ -f "$src/firmware.list" ]; then
        while read -r path from; do
            case "$path" in ''|'#'*) continue ;; esac
            mkdir -p "$fw/$(dirname "$path")"
            if [ -n "$from" ]; then
                cp "$from" "$fw/$path"
            elif [ -f "/lib/firmware/$path" ]; then
                cp -L "/lib/firmware/$path" "$fw/$path"
            elif [ -f "/lib/firmware/$path.zst" ]; then
                zstd -q -dc < "/lib/firmware/$path.zst" > "$fw/$path"
            else
                echo "no firmware: $path" >&2; exit 1
            fi
        done < "$src/firmware.list"
        echo "staged $mod firmware: $(find "$fw" -type f | wc -l) file(s), $(du -sh "$fw" | cut -f1)"
    fi
done
