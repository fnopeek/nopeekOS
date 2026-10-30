#!/usr/bin/env bash
# Build the guest's Mesa with nopeekOS's native-context patches, inside a
# throwaway root of the pinned Arch snapshot (microvm/linux/arch.sh, no
# root needed).
#
# Arch's mesa already enables the Intel and AMD virtio backends; this
# builds the same release from the same tarball with Arch's layout (glvnd,
# one Vulkan manifest) plus microvm/linux/mesa/patches: the backend as a
# library for drivers outside Mesa (libnpk_intel_virtio) and anv fixes.
# The drivers are limited to what a virtio-gpu guest can reach: iris, anv
# (Vulkan, and with it Vulkan Video) and radeonsi through native context,
# virgl as the name the kernel driver reports, llvmpipe as the fallback.
# The userspace bundle replaces the files of Arch's mesa and vulkan-intel
# with this output, and only if the versions match: libEGL_mesa, libgbm
# and libgallium share an internal ABI and must come from one build.
#
# With it radeonsi's VA-API driver (radeonsi_drv_video.so), through which
# Firefox decodes video on an AMD card over the native context.
#
# Also builds npk-gpuprobe (gpuprobe.c) against this Mesa: one log line per
# session with the driver the guest got; and installs npk-vkprobe
# (vkprobe.sh), which checks Vulkan Video decoding per codec.
#
# Then, in the same root, Intel's media driver (iHD, microvm/linux/ihd)
# against this Mesa's libnpk_intel_virtio: the patch sends its i915 ioctls
# through the same virtio backend iris and anv use.
#
# Output: ~/.cache/nopeekos/mesa/mesa-<ver>-npk.tar.gz and
# ~/.cache/nopeekos/ihd/ihd-<ver>-npk.tar.gz (paths below usr/).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
source "$SCRIPT_DIR/../arch.sh"
source "$SCRIPT_DIR/mesa.env"
IHD_DIR="$SCRIPT_DIR/../ihd"
source "$IHD_DIR/ihd.env"

CACHE="${HOME}/.cache/nopeekos/mesa"
MESA_TARBALL="mesa-${MESA_VERSION}.tar.xz"
OUT="$CACHE/mesa-${MESA_VERSION}-npk.tar.gz"
IHD_CACHE="${HOME}/.cache/nopeekos/ihd"
IHD_TARBALL="intel-media-driver-${IHD_VERSION}.tar.gz"
IHD_OUT="$IHD_CACHE/ihd-${IHD_VERSION}-npk.tar.gz"

# Arch's makedepends for these drivers, without rust (rusticl is off).
# clang, libclc and spirv-llvm-translator build mesa-clc, which iris needs
# for its precompiled internal shaders.
BUILD_PACKAGES="base-devel meson cmake bison flex python-mako python-packaging python-ply python-yaml python-pycparser
    libdrm libglvnd libxdamage libxshmfence libx11 libxext libxfixes libxrandr libxxf86vm libxcb xcb-util-keysyms xorgproto
    libelf systemd-libs expat libdisplay-info libxml2 zlib zstd libpng
    wayland wayland-protocols glslang spirv-tools libva vulkan-icd-loader
    llvm clang libclc spirv-llvm-translator intel-gmmlib libpciaccess mesa vulkan-intel"

# Arch's options (mesa PKGBUILD build()), except: the driver lists, Vulkan
# only for Intel, no rusticl, no layers.
MESON_OPTS="--optimization 2 -Db_ndebug=true
    -Dgallium-drivers=iris,radeonsi,virgl,llvmpipe
    -Dvulkan-drivers=intel -Dvulkan-layers=[] -Dintel-rt=disabled
    -Damdgpu-virtio=true -Dintel-virtio-experimental=true
    -Dgallium-extra-hud=true -Dgallium-rusticl=false -Dgles1=disabled
    -Dlibunwind=disabled -Dvalgrind=disabled -Dandroid-libbacktrace=disabled
    -Dmicrosoft-clc=disabled -Dhtml-docs=disabled
    -Dgallium-mediafoundation=disabled
    -Dvideo-codecs=all -Dvulkan-manifest-per-architecture=false"

fetch() {
    [ -f "$2" ] && return
    curl -L --fail --retry 5 --retry-all-errors --max-time 900 -o "$2.part" "$1"
    mv "$2.part" "$2"
}

mkdir -p "$CACHE" "$IHD_CACHE"
arch_fetch
fetch "https://archive.mesa3d.org/${MESA_TARBALL}" "$CACHE/$MESA_TARBALL"
fetch "https://github.com/intel/media-driver/archive/intel-media-${IHD_VERSION}.tar.gz" "$IHD_CACHE/$IHD_TARBALL"
echo "${MESA_SHA512}  $CACHE/$MESA_TARBALL" | sha512sum -c - >/dev/null \
    || { echo "sha512 mismatch on $MESA_TARBALL" >&2; exit 1; }
echo "${IHD_SHA512}  $IHD_CACHE/$IHD_TARBALL" | sha512sum -c - >/dev/null \
    || { echo "sha512 mismatch on $IHD_TARBALL" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'arch_rm "$STAGE"' EXIT
mkdir -p "$STAGE/src" "$STAGE/out/usr/bin"
tar -xJf "$CACHE/$MESA_TARBALL" -C "$STAGE/src"
cp "$SCRIPT_DIR/gpuprobe.c" "$STAGE/src/"
install -m 0755 "$SCRIPT_DIR/vkprobe.sh" "$STAGE/out/usr/bin/npk-vkprobe"
for p in "$SCRIPT_DIR"/patches/*.patch; do
    patch -d "$STAGE/src/mesa-${MESA_VERSION}" -p1 -s < "$p"
done
tar -xzf "$IHD_CACHE/$IHD_TARBALL" -C "$STAGE/src"
IHD_SRC="media-driver-intel-media-${IHD_VERSION}"
for p in "$IHD_DIR"/patches/*.patch; do
    patch -d "$STAGE/src/$IHD_SRC" -p1 -s < "$p"
done

JOBS="$(nproc)"
arch_run "$STAGE/root" "
    in_root pacman -Syu --noconfirm --needed $(echo $BUILD_PACKAGES) >/dev/null 2>&1
    arch_mesa=\$(in_root pacman -Q mesa | awk '{print \$2}')
    case \"\$arch_mesa\" in
        ${MESA_VERSION}-*|*:${MESA_VERSION}-*) ;;
        *) echo \"Arch has mesa \$arch_mesa, mesa.env pins ${MESA_VERSION}\" >&2; exit 1 ;;
    esac
    mkdir -p \$R/src \$R/out \$R/ihd
    mount --bind '$STAGE/src' \$R/src
    mount --bind '$STAGE/out' \$R/out
    in_root /bin/bash -c '
        set -e
        source /etc/makepkg.conf
        export CFLAGS CXXFLAGS LDFLAGS
        cd /src/mesa-${MESA_VERSION}
        arch-meson . output $(echo $MESON_OPTS) > /tmp/setup.log 2>&1 || { grep -E \"ERROR|error\" -A3 /tmp/setup.log | head -20; exit 1; }
        meson compile -C output -j $JOBS > /tmp/build.log 2>&1 || { grep -B2 -A12 -E \"error|Error\" /tmp/build.log | head -80; exit 1; }
        DESTDIR=/out meson install --no-rebuild -C output >/dev/null
        cc -std=c11 -O2 -Wall -Wextra -Werror -I/out/usr/include \$(pkg-config --cflags libdrm) \
            /src/gpuprobe.c -o /out/usr/bin/npk-gpuprobe -L/out/usr/lib -Wl,-rpath-link,/out/usr/lib -lEGL -lGLESv2 -lgbm -ldrm

        # The options of Arch (intel-media-driver PKGBUILD); the virtio
        # backend from the Mesa just built.
        cd /src/${IHD_SRC}
        export CFLAGS=\"\$CFLAGS -I/out/usr/include\" CXXFLAGS=\"\$CXXFLAGS -I/out/usr/include\"
        export LDFLAGS=\"\$LDFLAGS -L/out/usr/lib -Wl,-rpath-link,/out/usr/lib\"
        cmake -B build -G Ninja -Wno-dev -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \\
            -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_INSTALL_LIBDIR=lib \\
            -DINSTALL_DRIVER_SYSCONF=OFF -DMEDIA_BUILD_FATAL_WARNINGS=OFF >/dev/null
        cmake --build build -j $JOBS > /tmp/build.log 2>&1 || { grep -B2 -A12 -E \"error|Error\" /tmp/build.log | head -80; exit 1; }
        DESTDIR=/out/ihd cmake --install build >/dev/null
    '
"

# Exactly the runtime files of Arch's mesa and vulkan-intel that this build
# makes, plus the probes; headers, pkg-config files and dev symlinks stay
# out.
(
    cd "$STAGE/out"
    find usr/lib/libgallium-*.so usr/lib/gbm/dri_gbm.so usr/share/drirc.d \
         usr/lib/libEGL_mesa.so.0* usr/lib/libGLX_mesa.so.0* usr/share/glvnd/egl_vendor.d/50_mesa.json \
         usr/lib/libgbm.so.1* usr/bin/npk-gpuprobe usr/bin/npk-vkprobe \
         usr/lib/libvulkan_intel.so usr/share/vulkan/icd.d/intel_icd.json \
         usr/lib/libnpk_intel_virtio.so.* usr/lib/dri/radeonsi_drv_video.so \
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

# The runtime files of intel-media-driver: the VA-API driver and the
# C-for-Media runtime library.
(
    cd "$STAGE/out/ihd"
    find usr/lib/dri/iHD_drv_video.so usr/lib/libigfxcmrt.so.* \( -type f -o -type l \) -print \
        | LC_ALL=C sort > "$STAGE/ihd-files"
    rm -rf "$IHD_CACHE/symbols" && mkdir -p "$IHD_CACHE/symbols"
    cp usr/lib/dri/iHD_drv_video.so "$IHD_CACHE/symbols/"
    find usr/lib -name '*.so*' -type f -exec strip --strip-unneeded {} +
    tar --owner=0 --group=0 --mtime=@0 -czf "$IHD_OUT.tmp" -T "$STAGE/ihd-files"
)
mv "$IHD_OUT.tmp" "$IHD_OUT"
echo "ihd ${IHD_VERSION} (intel-virtio): $(stat -c%s "$IHD_OUT") bytes -> $IHD_OUT"
