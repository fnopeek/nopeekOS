#!/usr/bin/env bash
# Build the guest's Mesa with the Intel native-context backend, inside a
# throwaway chroot of the pinned Alpine release (no root needed:
# `unshare --map-root-user`).
#
# Alpine's mesa is built without `-Dintel-virtio-experimental`, so iris in
# the guest never asks virtio-gpu for the DRM capset and the guest stays on
# llvmpipe. This builds the same release from the same tarball with
# Alpine's meson options (main/mesa/APKBUILD) plus that option, limited to
# the drivers a virtio-gpu guest can reach: iris, anv (Vulkan, and with it
# Vulkan Video) and radeonsi through native context, virgl as the name the
# kernel driver reports, llvmpipe as the fallback. The userspace bundle replaces the files of Alpine's mesa
# packages with this output, and only if the versions match: libEGL, libgbm
# and libgallium share an internal ABI and must come from one build.
#
# Also builds npk-gpuprobe (gpuprobe.c) against this Mesa: one log line per
# session with the driver the guest got; and installs npk-vkprobe
# (vkprobe.sh), which checks Vulkan Video decoding per codec.
#
# Then, in the same chroot, Intel's media driver (iHD, microvm/linux/ihd)
# against this Mesa's libnpk_intel_virtio: the patch sends its i915 ioctls
# through the same virtio backend iris and anv use.
#
# Output: ~/.cache/nopeekos/mesa/mesa-<ver>-npk.tar.gz and
# ~/.cache/nopeekos/ihd/ihd-<ver>-npk.tar.gz (paths below usr/).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
source "$REPO_ROOT/microvm-userspace/alpine.env"
source "$SCRIPT_DIR/mesa.env"
IHD_DIR="$SCRIPT_DIR/../ihd"
source "$IHD_DIR/ihd.env"

ALPINE_CACHE="${HOME}/.cache/nopeekos/alpine"
CACHE="${HOME}/.cache/nopeekos/mesa"
TARBALL="alpine-minirootfs-${ALPINE_VERSION}-x86_64.tar.gz"
MESA_TARBALL="mesa-${MESA_VERSION}.tar.xz"
OUT="$CACHE/mesa-${MESA_VERSION}-npk.tar.gz"
IHD_CACHE="${HOME}/.cache/nopeekos/ihd"
IHD_TARBALL="intel-media-driver-${IHD_VERSION}.tar.gz"
IHD_OUT="$IHD_CACHE/ihd-${IHD_VERSION}-npk.tar.gz"

# Alpine's makedepends without rust (rusticl is off) and the Vulkan loader.
# clang, libclc and spirv-llvm-translator build mesa-clc, which iris needs
# for its precompiled internal shaders.
BUILD_PACKAGES="build-base pkgconf meson bison flex python3 py3-mako py3-packaging py3-ply py3-yaml py3-cparser
    libdrm-dev libxdamage-dev libxshmfence-dev libx11-dev libxfixes-dev libxrandr-dev libxxf86vm-dev xorgproto
    elfutils-dev eudev-dev expat-dev libdisplay-info-dev libxml2-dev zlib-dev zstd-dev
    wayland-dev wayland-protocols glslang-dev libva-dev
    llvm${MESA_LLVM}-dev clang${MESA_LLVM}-dev libclc-dev spirv-llvm-translator-dev
    cmake samurai intel-gmmlib-dev libpciaccess-dev"

# Alpine's options (APKBUILD build()), except: the driver lists, Vulkan only
# for Intel, no rusticl, no VA-API driver, and the Intel virtio backend.
MESON_OPTS="--prefix=/usr --libdir=lib --sysconfdir=/etc --localstatedir=/var
    --buildtype=plain --wrap-mode=nodownload
    -Db_ndebug=true -Db_lto=false
    -Dallow-kcmp=enabled -Dexpat=enabled -Dshader-cache=enabled
    -Dxlib-lease=enabled -Dxmlconfig=enabled -Dzstd=enabled
    -Dbackend_max_links=2 -Dbuild-tests=false
    -Ddri-drivers-path=/usr/lib/dri
    -Dgallium-drivers=iris,radeonsi,virgl,llvmpipe
    -Dvulkan-drivers=intel -Dvulkan-layers=[] -Dintel-rt=disabled
    -Damdgpu-virtio=true -Dintel-virtio-experimental=true
    -Dplatforms=x11,wayland
    -Dllvm=enabled -Dshared-llvm=enabled
    -Dgbm=enabled -Dglx=dri -Dopengl=true -Dgles1=enabled -Dgles2=enabled -Degl=enabled
    -Dgallium-extra-hud=true -Dgallium-rusticl=false -Dgallium-va=disabled
    -Dvideo-codecs=all"

fetch() {
    [ -f "$2" ] && return
    curl -L --fail --retry 5 --retry-all-errors --max-time 900 -o "$2.part" "$1"
    mv "$2.part" "$2"
}

mkdir -p "$ALPINE_CACHE/apks" "$CACHE" "$IHD_CACHE"
fetch "https://dl-cdn.alpinelinux.org/alpine/${ALPINE_BRANCH}/releases/x86_64/${TARBALL}" "$ALPINE_CACHE/$TARBALL"
fetch "https://archive.mesa3d.org/${MESA_TARBALL}" "$CACHE/$MESA_TARBALL"
fetch "https://github.com/intel/media-driver/archive/intel-media-${IHD_VERSION}.tar.gz" "$IHD_CACHE/$IHD_TARBALL"
echo "${ALPINE_MINIROOTFS_SHA256}  $ALPINE_CACHE/$TARBALL" | sha256sum -c - >/dev/null \
    || { echo "sha256 mismatch on $TARBALL" >&2; exit 1; }
echo "${MESA_SHA512}  $CACHE/$MESA_TARBALL" | sha512sum -c - >/dev/null \
    || { echo "sha512 mismatch on $MESA_TARBALL" >&2; exit 1; }
echo "${IHD_SHA512}  $IHD_CACHE/$IHD_TARBALL" | sha512sum -c - >/dev/null \
    || { echo "sha512 mismatch on $IHD_TARBALL" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
tar -xzf "$ALPINE_CACHE/$TARBALL" -C "$STAGE"
echo "nameserver 1.1.1.1" > "$STAGE/etc/resolv.conf"
mkdir -p "$STAGE/src" "$STAGE/var/cache/apk" "$STAGE/out"
tar -xJf "$CACHE/$MESA_TARBALL" -C "$STAGE/src"
cp "$SCRIPT_DIR/gpuprobe.c" "$STAGE/src/"
install -D -m 0755 "$SCRIPT_DIR/vkprobe.sh" "$STAGE/out/usr/bin/npk-vkprobe"
for p in "$SCRIPT_DIR"/patches/*.patch; do
    patch -d "$STAGE/src/mesa-${MESA_VERSION}" -p1 -s < "$p"
done
tar -xzf "$IHD_CACHE/$IHD_TARBALL" -C "$STAGE/src"
IHD_SRC="media-driver-intel-media-${IHD_VERSION}"
for p in "$IHD_DIR"/patches/*.patch; do
    patch -d "$STAGE/src/$IHD_SRC" -p1 -s < "$p"
done

JOBS="$(nproc)"
mkdir -p "$STAGE/ihd"
unshare --user --map-root-user --mount --pid --fork --mount-proc sh -c "
    set -e
    mount --bind '$ALPINE_CACHE/apks' '$STAGE/var/cache/apk'
    chroot '$STAGE' /sbin/apk update >/dev/null
    chroot '$STAGE' /sbin/apk upgrade --no-progress --available >/dev/null
    chroot '$STAGE' /sbin/apk add --no-progress $(echo $BUILD_PACKAGES) >/dev/null
    chroot '$STAGE' /bin/sh -c '
        set -e
        alpine_mesa=\$(apk search -x mesa 2>/dev/null | head -1)
        case \"\$alpine_mesa\" in
            mesa-${MESA_VERSION}-r*) ;;
            *) echo \"Alpine has \$alpine_mesa, mesa.env pins ${MESA_VERSION}\" >&2; exit 1 ;;
        esac
        cd /src/mesa-${MESA_VERSION}
        export CFLAGS=\"-O2 -fstack-clash-protection -Wformat -Werror=format-security\"
        export CXXFLAGS=\"\$CFLAGS\"
        PATH=\"\$PATH:/usr/lib/llvm${MESA_LLVM}/bin\" meson setup output $(echo $MESON_OPTS) >/dev/null
        meson compile -C output -j $JOBS > /tmp/build.log 2>&1 || { grep -B2 -A12 -E \"error|Error\" /tmp/build.log | head -80; exit 1; }
        DESTDIR=/out meson install --no-rebuild -C output >/dev/null
        mkdir -p /out/usr/bin
        cc -std=c11 -O2 -Wall -Wextra -Werror -I/out/usr/include \$(pkg-config --cflags libdrm) \
            /src/gpuprobe.c -o /out/usr/bin/npk-gpuprobe -L/out/usr/lib -Wl,-rpath-link,/out/usr/lib -lEGL -lGLESv2 -lgbm -ldrm

        alpine_ihd=\$(apk search -x intel-media-driver 2>/dev/null | head -1)
        case \"\$alpine_ihd\" in
            intel-media-driver-${IHD_VERSION}-r*) ;;
            *) echo \"Alpine has \$alpine_ihd, ihd.env pins ${IHD_VERSION}\" >&2; exit 1 ;;
        esac
        # The options of Alpine (community/intel-media-driver APKBUILD); the
        # virtio backend from the Mesa just built.
        cd /src/${IHD_SRC}
        export CFLAGS=\"\$CFLAGS -I/out/usr/include\" CXXFLAGS=\"\$CXXFLAGS -I/out/usr/include\"
        export LDFLAGS=\"-L/out/usr/lib -Wl,-rpath-link,/out/usr/lib\"
        cmake3.5 -B build -G Ninja -Wno-dev \\
            -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_INSTALL_LIBDIR=lib \\
            -DBUILD_SHARED_LIBS=True -DINSTALL_DRIVER_SYSCONF=OFF -DMEDIA_BUILD_FATAL_WARNINGS=OFF >/dev/null
        cmake3.5 --build build -j $JOBS > /tmp/build.log 2>&1 || { grep -B2 -A12 -E \"error|Error\" /tmp/build.log | head -80; exit 1; }
        DESTDIR=/ihd cmake3.5 --install build >/dev/null
    '
"

# Exactly the runtime files of Alpine's mesa, mesa-egl, mesa-gl, mesa-gles,
# mesa-gbm and mesa-vulkan-intel, plus the probe; headers, pkg-config files
# and dev symlinks stay out.
(
    cd "$STAGE/out"
    find usr/lib/libgallium-*.so usr/lib/gbm/dri_gbm.so usr/share/drirc.d \
         usr/lib/libEGL.so.1* usr/lib/libGL.so.1* usr/lib/libGLESv1_CM.so.1* \
         usr/lib/libGLESv2.so.2* usr/lib/libgbm.so.1* usr/bin/npk-gpuprobe usr/bin/npk-vkprobe \
         usr/lib/libvulkan_intel.so usr/share/vulkan/icd.d/intel_icd.x86_64.json \
         usr/lib/libnpk_intel_virtio.so.* \
         \( -type f -o -type l \) -print | LC_ALL=C sort > "$STAGE/files"
    # Unstripped copies stay in the cache, to name the frames of a backtrace
    # (`npkgdb`) from the guest's stripped libraries.
    rm -rf "$CACHE/symbols" && mkdir -p "$CACHE/symbols"
    cp usr/lib/libvulkan_intel.so usr/lib/libgallium-*.so "$CACHE/symbols/"
    find usr/lib -name '*.so*' -type f -exec strip --strip-unneeded {} +
    strip usr/bin/npk-gpuprobe
    tar --owner=0 --group=0 --mtime=@0 -czf "$OUT.tmp" -T "$STAGE/files"
)
mv "$OUT.tmp" "$OUT"
echo "mesa ${MESA_VERSION} (intel-virtio): $(stat -c%s "$OUT") bytes -> $OUT"

# The runtime files of Alpine's intel-media-driver: the VA-API driver and
# the C-for-Media runtime library.
(
    cd "$STAGE/ihd"
    find usr/lib/dri/iHD_drv_video.so usr/lib/libigfxcmrt.so.* \( -type f -o -type l \) -print \
        | LC_ALL=C sort > "$STAGE/ihd-files"
    rm -rf "$IHD_CACHE/symbols" && mkdir -p "$IHD_CACHE/symbols"
    cp usr/lib/dri/iHD_drv_video.so "$IHD_CACHE/symbols/"
    find usr/lib -name '*.so*' -type f -exec strip --strip-unneeded {} +
    tar --owner=0 --group=0 --mtime=@0 -czf "$IHD_OUT.tmp" -T "$STAGE/ihd-files"
)
mv "$IHD_OUT.tmp" "$IHD_OUT"
echo "ihd ${IHD_VERSION} (intel-virtio): $(stat -c%s "$IHD_OUT") bytes -> $IHD_OUT"
