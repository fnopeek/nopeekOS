#!/usr/bin/env bash
# Build portal-npk, the guest's xdg-desktop-portal backend, inside a
# throwaway root of the pinned Arch snapshot (microvm/linux/arch.sh, no root
# needed), against the GLib the userspace bundle carries.
#
# Output: microvm/linux/portal/portal-npk (committed; the userspace bundle
# carries it with npk.portal, portals.conf and its D-Bus service file).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../arch.sh"
OUT="$SCRIPT_DIR/portal-npk"

arch_fetch
STAGE="$(mktemp -d)"
trap 'arch_rm "$STAGE"' EXIT
arch_run "$STAGE/root" "
    in_root pacman -Syu --noconfirm --needed base-devel glib2 >/dev/null 2>&1
    mkdir -p \$R/src
    cp '$SCRIPT_DIR/portal-npk.c' \$R/src/
    in_root /bin/sh -c '
        set -e
        cd /src
        cc -std=gnu11 -O2 -Wall -Wextra -Werror -Wno-unused-parameter portal-npk.c -o portal-npk \
            \$(pkg-config --cflags --libs gio-2.0)
        strip portal-npk
    '
    cp \$R/src/portal-npk '$STAGE/portal-npk'
"
cp "$STAGE/portal-npk" "$OUT"
touch -d @0 "$OUT"
echo "portal-npk: $(stat -c%s "$OUT") bytes"
