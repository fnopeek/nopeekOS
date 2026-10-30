#!/usr/bin/env bash
# Build npkwm, the guest's Wayland compositor, inside a throwaway root of
# the pinned Arch snapshot (microvm/linux/arch.sh, no root needed).
#
# wlroots is built here from source and linked in statically, with only what
# the guest uses: the DRM and libinput backends and the pixman renderer.
# A distribution's libwlroots carries the GLES2 and Vulkan renderers, and
# through them Mesa and LLVM, which the dynamic loader has to map and
# relocate before main(). The libraries npkwm still
# links (libdrm, libinput, libudev, libseat, pixman, xkbcommon, wayland,
# libdisplay-info) are in the userspace bundle.
#
# `build.sh gles2` builds the variant for a guest with a GPU: the same
# source with wlroots' GLES2 renderer and GBM allocator, so clients get
# linux-dmabuf with a render node and npkwm composes on the GPU. It links
# Mesa (libEGL, libGLESv2, libgbm) and pays that load time; the guest's init
# runs it only when the host offers native contexts. Without a usable GPU
# wlroots' autocreate refuses a software GL device and falls back to pixman.
#
# Output: microvm/linux/npkwm/npkwm (committed; PID 1 embeds it), or
# microvm/linux/npkwm/npkwm-gles2 (committed; the userspace bundle carries it).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
source "$SCRIPT_DIR/../arch.sh"

CACHE="${HOME}/.cache/nopeekos/wlroots"
VARIANT="${1:-}"
case "$VARIANT" in
    "")    NAME=npkwm;       WLR_RENDER="-Drenderers=[] -Dallocators=[]"; EXTRA_PACKAGES="" ;;
    gles2) NAME=npkwm-gles2; WLR_RENDER="-Drenderers=gles2 -Dallocators=gbm"; EXTRA_PACKAGES="mesa" ;;
    *) echo "usage: $0 [gles2]" >&2; exit 1 ;;
esac
OUT="$SCRIPT_DIR/$NAME"

# wlroots from its release tarball, pinned by sha512.
WLR_VERSION=0.20.2
WLR_TARBALL="wlroots-${WLR_VERSION}.tar.gz"
WLR_URL="https://gitlab.freedesktop.org/wlroots/wlroots/-/archive/${WLR_VERSION}/${WLR_TARBALL}"
WLR_SHA512=e96523a2b215e7d1f36bbe5e7fa3e65d15cb87e2f0d9c304d3c2fdc81b0ac0c7202cd955635c01072b34e70f08c6c00fa2cde8c277bd73fcabdcb19f7b579a12

BUILD_PACKAGES="base-devel meson wayland wayland-protocols libxkbcommon libdrm pixman libinput systemd-libs seatd hwdata libdisplay-info $EXTRA_PACKAGES"

fetch() {
    [ -f "$CACHE/$2" ] && return
    curl -L --fail --max-time 120 -o "$CACHE/$2.part" "$1"
    mv "$CACHE/$2.part" "$CACHE/$2"
}

mkdir -p "$CACHE"
arch_fetch
fetch "$WLR_URL" "$WLR_TARBALL"
echo "${WLR_SHA512}  $CACHE/$WLR_TARBALL" | sha512sum -c - >/dev/null \
    || { echo "sha512 mismatch on $WLR_TARBALL" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'arch_rm "$STAGE"' EXIT
arch_run "$STAGE/root" "
    in_root pacman -Syu --noconfirm --needed $BUILD_PACKAGES >/dev/null
    mkdir -p \$R/src
    tar -xzf '$CACHE/$WLR_TARBALL' -C \$R/src
    cp '$SCRIPT_DIR/npkwm.c' \$R/src/
    in_root /bin/sh -c '
        set -e
        cd /src/wlroots-${WLR_VERSION}
        meson setup build --prefix=/opt/wlr --buildtype=release \
            -Ddefault_library=static -Dexamples=false \
            $WLR_RENDER -Dbackends=drm,libinput \
            -Dsession=enabled -Dxwayland=disabled -Dxcb-errors=disabled \
            -Dcolor-management=disabled -Dlibliftoff=disabled >/dev/null
        meson compile -C build >/dev/null
        meson install -C build >/dev/null
        cd /src
        P=\$(pkg-config --variable=pkgdatadir wayland-protocols)
        wayland-scanner server-header \$P/stable/xdg-shell/xdg-shell.xml xdg-shell-protocol.h
        export PKG_CONFIG_PATH=/opt/wlr/lib/pkgconfig
        cc -std=c11 -O2 -Wall -Wextra -Werror -Wno-unused-parameter \
            -DWLR_USE_UNSTABLE -I. npkwm.c -o $NAME -Wl,--as-needed \
            \$(pkg-config --static --cflags --libs wlroots-0.20 wayland-server xkbcommon)
        strip $NAME
    '
    cp \$R/src/$NAME '$STAGE/$NAME'
"
cp "$STAGE/$NAME" "$OUT"
touch -d @0 "$OUT"
echo "$NAME: $(stat -c%s "$OUT") bytes"
