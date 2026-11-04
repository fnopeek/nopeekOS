#!/usr/bin/env bash
# Build portal-npk, the guest's xdg-desktop-portal backend, inside a
# throwaway root of the pinned Arch snapshot (microvm/linux/arch.sh, no root
# needed), against the GLib, PipeWire, GBM and libdrm the userspace bundle
# carries.
#
# Output: microvm/linux/portal/portal-npk and npk-capture-probe (committed;
# the userspace bundle carries them with npk.portal, portals.conf and the
# D-Bus service file).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../arch.sh"

arch_fetch
STAGE="$(mktemp -d)"
trap 'arch_rm "$STAGE"' EXIT
arch_run "$STAGE/root" "
    in_root pacman -Syu --noconfirm --needed base-devel glib2 pipewire mesa libdrm >/dev/null 2>&1
    mkdir -p \$R/src
    cp '$SCRIPT_DIR/portal-npk.c' '$SCRIPT_DIR/npk-capture-probe.c' '$SCRIPT_DIR/npkcap.h' \$R/src/
    in_root /bin/sh -c '
        set -e
        cd /src
        cc -std=gnu11 -O2 -Wall -Wextra -Werror -Wno-unused-parameter portal-npk.c -o portal-npk \
            \$(pkg-config --cflags --libs gio-2.0 gio-unix-2.0 libpipewire-0.3 gbm libdrm)
        cc -std=gnu11 -O2 -Wall -Wextra -Werror -Wno-unused-parameter npk-capture-probe.c -o npk-capture-probe \
            \$(pkg-config --cflags --libs gio-2.0 libpipewire-0.3 gbm libdrm)
        strip portal-npk npk-capture-probe
    '
    cp \$R/src/portal-npk \$R/src/npk-capture-probe '$STAGE/'
"
for f in portal-npk npk-capture-probe; do
    cp "$STAGE/$f" "$SCRIPT_DIR/$f"
    touch -d @0 "$SCRIPT_DIR/$f"
    echo "$f: $(stat -c%s "$SCRIPT_DIR/$f") bytes"
done
