#!/usr/bin/env bash
# Build npk-camera, the guest's PipeWire camera, inside a throwaway root of
# the pinned Arch snapshot (microvm/linux/arch.sh, no root needed), against
# the libpipewire the userspace bundle carries.
#
# Output: microvm/linux/npkcam/npk-camera (committed; the userspace bundle
# carries it). `ARCH=aarch64` builds in an Arch Linux ARM root
# (microvm/linux/guest.sh) and writes npk-camera-aarch64.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../guest.sh"
OUT="$SCRIPT_DIR/npk-camera$GUEST_SUFFIX"

guest_fetch
STAGE="$(mktemp -d)"
trap 'guest_rm "$STAGE"' EXIT
guest_run "$STAGE/root" "
    $GUEST_PACMAN -Syu --noconfirm --needed base-devel pipewire libjpeg-turbo >/dev/null 2>&1
    mkdir -p \$R/src
    cp '$SCRIPT_DIR/npk-camera.c' \$R/src/
    in_root /bin/sh -c '
        set -e
        cd /src
        cc -std=gnu11 -O2 -Wall -Wextra -Werror -Wno-unused-parameter npk-camera.c -o npk-camera \
            \$(pkg-config --cflags --libs libpipewire-0.3 libjpeg)
        strip npk-camera
    '
    cp \$R/src/npk-camera '$STAGE/npk-camera'
"
cp "$STAGE/npk-camera" "$OUT"
touch -d @0 "$OUT"
echo "${OUT##*/}: $(stat -c%s "$OUT") bytes"
