#!/usr/bin/env bash
# Build npkwm, the guest's Wayland compositor, inside a throwaway chroot of
# the pinned Alpine release (no root needed: `unshare --map-root-user`).
#
# wlroots is built here from source and linked in statically, with only what
# the guest uses: the DRM and libinput backends and the pixman renderer.
# Alpine's libwlroots carries the GLES2 and Vulkan renderers, and through
# them Mesa and LLVM; musl resolves every relocation at load time, so that
# alone cost about half a second before main(). The libraries npkwm still
# links (libdrm, libinput, libudev, libseat, pixman, xkbcommon, wayland,
# libdisplay-info) are in the userspace bundle.
#
# Output: microvm/linux/npkwm/npkwm (committed; PID 1 embeds it).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
source "$REPO_ROOT/microvm-userspace/alpine.env"

CACHE="${HOME}/.cache/nopeekos/alpine"
TARBALL="alpine-minirootfs-${ALPINE_VERSION}-x86_64.tar.gz"
OUT="$SCRIPT_DIR/npkwm"

# The wlroots of the pinned Alpine branch, from the URL and with the sha512
# of its APKBUILD (community/wlroots0.20).
WLR_VERSION=0.20.2
WLR_TARBALL="wlroots-${WLR_VERSION}.tar.gz"
WLR_URL="https://gitlab.freedesktop.org/wlroots/wlroots/-/archive/${WLR_VERSION}/${WLR_TARBALL}"
WLR_SHA512=e96523a2b215e7d1f36bbe5e7fa3e65d15cb87e2f0d9c304d3c2fdc81b0ac0c7202cd955635c01072b34e70f08c6c00fa2cde8c277bd73fcabdcb19f7b579a12

BUILD_PACKAGES="build-base pkgconf meson wayland-dev wayland-protocols libxkbcommon-dev libdrm-dev pixman-dev libinput-dev eudev-dev libseat-dev hwdata-dev libdisplay-info-dev"

fetch() {
    [ -f "$CACHE/$2" ] && return
    curl -L --fail --max-time 120 -o "$CACHE/$2.part" "$1"
    mv "$CACHE/$2.part" "$CACHE/$2"
}

mkdir -p "$CACHE/apks"
fetch "https://dl-cdn.alpinelinux.org/alpine/${ALPINE_BRANCH}/releases/x86_64/${TARBALL}" "$TARBALL"
fetch "$WLR_URL" "$WLR_TARBALL"
echo "${ALPINE_MINIROOTFS_SHA256}  $CACHE/$TARBALL" | sha256sum -c - >/dev/null \
    || { echo "sha256 mismatch on $TARBALL" >&2; exit 1; }
echo "${WLR_SHA512}  $CACHE/$WLR_TARBALL" | sha512sum -c - >/dev/null \
    || { echo "sha512 mismatch on $WLR_TARBALL" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
tar -xzf "$CACHE/$TARBALL" -C "$STAGE"
echo "nameserver 1.1.1.1" > "$STAGE/etc/resolv.conf"
mkdir -p "$STAGE/src" "$STAGE/var/cache/apk"
tar -xzf "$CACHE/$WLR_TARBALL" -C "$STAGE/src"
cp "$SCRIPT_DIR/npkwm.c" "$STAGE/src/"

unshare --user --map-root-user --mount --pid --fork --mount-proc sh -c "
    set -e
    mount --bind '$CACHE/apks' '$STAGE/var/cache/apk'
    chroot '$STAGE' /sbin/apk update >/dev/null
    chroot '$STAGE' /sbin/apk add --no-progress $BUILD_PACKAGES >/dev/null
    chroot '$STAGE' /bin/sh -c '
        set -e
        cd /src/wlroots-${WLR_VERSION}
        meson setup build --prefix=/opt/wlr --buildtype=release \
            -Ddefault_library=static -Dexamples=false \
            -Drenderers=[] -Dallocators=[] -Dbackends=drm,libinput \
            -Dsession=enabled -Dxwayland=disabled -Dxcb-errors=disabled \
            -Dcolor-management=disabled -Dlibliftoff=disabled >/dev/null
        meson compile -C build >/dev/null
        meson install -C build >/dev/null
        cd /src
        P=\$(pkg-config --variable=pkgdatadir wayland-protocols)
        wayland-scanner server-header \$P/stable/xdg-shell/xdg-shell.xml xdg-shell-protocol.h
        export PKG_CONFIG_PATH=/opt/wlr/lib/pkgconfig
        cc -std=c11 -O2 -Wall -Wextra -Werror -Wno-unused-parameter \
            -DWLR_USE_UNSTABLE -I. npkwm.c -o npkwm -Wl,--as-needed \
            \$(pkg-config --static --cflags --libs wlroots-0.20 wayland-server xkbcommon)
        strip npkwm
    '
"
cp "$STAGE/src/npkwm" "$OUT"
touch -d @0 "$OUT"
echo "npkwm: $(stat -c%s "$OUT") bytes"
