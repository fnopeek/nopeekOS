#!/usr/bin/env bash
# Build Mesa's v3d driver for the aarch64 guest with the V3D native
# context (patches/v3d-virtio-npk.patch, docs/spec/V3D_NATIVE_CONTEXT.md),
# in a throwaway Arch Linux ARM root run through qemu-user
# (microvm/linux/alarm.sh).
#
# Only the v3d Gallium driver, OpenGL ES, EGL for Wayland and GBM (the
# guest's GLES2 compositor allocates through it), no LLVM. It
# installs below /opt/npk-mesa with its own RUNPATH, beside Arch Linux
# ARM's mesa, which stays for llvmpipe: libgallium carries the release in
# its name, so the two would collide in one directory. The guest's init
# points EGL at this one (its glvnd vendor file) only when the host
# offers the v3d native context.
#
# Output: ~/.cache/nopeekos/mesa/mesa-v3d-<ver>-npk-aarch64.tar.gz
# (paths below opt/).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ARCH=aarch64
source "$SCRIPT_DIR/../guest.sh"
source "$SCRIPT_DIR/mesa.env"

CACHE="${HOME}/.cache/nopeekos/mesa"
TARBALL="mesa-${MESA_V3D_VERSION}.tar.xz"
OUT="$CACHE/mesa-v3d-${MESA_V3D_VERSION}-npk-aarch64.tar.gz"
PREFIX=/opt/npk-mesa
PATCH="$SCRIPT_DIR/patches/v3d-virtio-npk.patch"

BUILD_PACKAGES="base-devel meson python-mako python-packaging python-ply python-yaml python-pycparser"
BUILD_PACKAGES="$BUILD_PACKAGES libdrm libglvnd wayland wayland-protocols expat zlib zstd libelf systemd-libs"

MESON_OPTS="--prefix=$PREFIX --buildtype=release -Db_ndebug=true
    -Dc_link_args=-Wl,-rpath,$PREFIX/lib -Dcpp_link_args=-Wl,-rpath,$PREFIX/lib
    -Dgallium-drivers=v3d -Dvulkan-drivers=[] -Dplatforms=wayland
    -Dglx=disabled -Dglvnd=enabled -Degl=enabled -Dgles1=disabled -Dgles2=enabled -Dopengl=true
    -Dgbm=enabled -Dllvm=disabled -Dvideo-codecs=[] -Dgallium-va=disabled
    -Dlibunwind=disabled -Dvalgrind=disabled -Dlmsensors=disabled -Dzstd=enabled"

mkdir -p "$CACHE"
if [ ! -f "$CACHE/$TARBALL" ]; then
    curl -L --fail -o "$CACHE/$TARBALL.part" "https://archive.mesa3d.org/$TARBALL"
    mv "$CACHE/$TARBALL.part" "$CACHE/$TARBALL"
fi
echo "$MESA_V3D_SHA512  $CACHE/$TARBALL" | sha512sum -c --quiet - || { echo "$TARBALL: sha512 mismatch" >&2; exit 1; }

guest_fetch
STAGE="$(mktemp -d)"
trap 'guest_rm "$STAGE"' EXIT
cat > "$STAGE/inner.sh" <<INNER
set -e
cd /src/mesa-${MESA_V3D_VERSION}
fail() { tail -40 "\$1"; exit 1; }
patch -p1 < /src/${PATCH##*/} > /src/patch.log 2>&1 || fail /src/patch.log
meson setup build $(echo $MESON_OPTS) > /src/setup.log 2>&1 || fail /src/setup.log
meson compile -C build > /src/compile.log 2>&1 || fail /src/compile.log
DESTDIR=/out meson install -C build > /src/install.log 2>&1 || fail /src/install.log
# glvnd resolves a bare library name through the default path, which
# holds the other mesa: name this one by its path.
J=/out$PREFIX/share/glvnd/egl_vendor.d/50_mesa.json
sed -i 's|"libEGL_mesa.so.0"|"$PREFIX/lib/libEGL_mesa.so.0"|' \$J
grep -q '$PREFIX/lib/libEGL_mesa' \$J
rm -rf /out$PREFIX/include /out$PREFIX/lib/pkgconfig
find /out$PREFIX -name '*.so*' -type f -exec strip --strip-unneeded {} +
INNER
guest_run "$STAGE/root" "
    $GUEST_PACMAN -Syu --noconfirm --needed $BUILD_PACKAGES >/dev/null
    mkdir -p \$R/src
    tar -xJf '$CACHE/$TARBALL' -C \$R/src
    cp '$PATCH' '$STAGE/inner.sh' \$R/src/
    in_root /bin/sh /src/inner.sh
    tar -C \$R/out -czf '$STAGE/out.tar.gz' opt
"
cp "$STAGE/out.tar.gz" "$OUT"
echo "$OUT: $(stat -c%s "$OUT") bytes"
tar -tzf "$OUT" | grep -E "libEGL_mesa|libgallium|libgbm|dri_gbm|50_mesa"
