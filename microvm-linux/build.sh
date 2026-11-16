#!/usr/bin/env bash
# microvm-linux/build.sh — build the Linux kernel for the nopeekOS MicroVM.
#
# Downloads kernel.org source (cached in ~/.cache/nopeekos/linux-src/),
# applies our patches (patches/*.patch) and our config overlay on top of
# the architecture's defconfig and copies the result into release/assets/
# where the installer + OTA pipeline pick it up.
#
#   ARCH=x86_64 (default): x86_64_defconfig + nopeek-virt.config, bzImage
#                          -> release/assets/linux-virt.bzImage
#   ARCH=arm64:            defconfig + nopeek-virt.config (x86 lines dropped)
#                          + nopeek-virt-arm64.config, built with LLVM=1,
#                          uncompressed Image
#                          -> release/assets/linux-virt-arm64.Image
set -euo pipefail

ARCH="${ARCH:-x86_64}"
case "$ARCH" in
    x86_64|x86) ARCH=x86_64 ;;
    arm64|aarch64) ARCH=arm64 ;;
    *) printf 'unknown ARCH=%s (x86_64 or arm64)\n' "$ARCH" >&2; exit 1 ;;
esac

# Bump both together. The sha256 comes from
# https://cdn.kernel.org/pub/linux/kernel/v6.x/sha256sums.asc
LINUX_VERSION="${LINUX_VERSION:-6.18.60}"
LINUX_SHA256="${LINUX_SHA256:-3c2f44ef6acc021a7ad444d9a2e6ec1ef488ff09368793d0004a6080f69d621e}"
SRC_CACHE="${SRC_CACHE:-$HOME/.cache/nopeekos/linux-src}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONFIG_FRAGMENT="$SCRIPT_DIR/nopeek-virt.config"
if [ "$ARCH" = "arm64" ]; then
    OUT="${OUT:-$REPO_ROOT/release/assets/linux-virt-arm64.Image}"
    ARCH_FRAGMENT="$SCRIPT_DIR/nopeek-virt-arm64.config"
    DEFCONFIG=defconfig
    TARGET=Image
    BUILT=arch/arm64/boot/Image
    # No GNU aarch64 cross toolchain on the build host; clang + lld cover it.
    MAKE_ARGS=(ARCH=arm64 LLVM=1)
    TREE_SUFFIX="-arm64"
else
    OUT="${OUT:-$REPO_ROOT/release/assets/linux-virt.bzImage}"
    ARCH_FRAGMENT=""
    DEFCONFIG=x86_64_defconfig
    TARGET=bzImage
    BUILT=arch/x86/boot/bzImage
    MAKE_ARGS=()
    TREE_SUFFIX=""
fi

JOBS="${JOBS:-$(nproc 2>/dev/null || echo 4)}"

cyan()  { printf '\033[0;36m[npk]\033[0m %s\n' "$1"; }
green() { printf '\033[0;32m[npk]\033[0m %s\n' "$1"; }
red()   { printf '\033[0;31m[npk]\033[0m %s\n' "$1" >&2; }

# ── Sanity ────────────────────────────────────────────────────────
TOOLS=(make flex bison openssl curl xz tar pkg-config sha256sum)
if [ "$ARCH" = "arm64" ]; then
    TOOLS+=(clang ld.lld llvm-objcopy llvm-ar llvm-nm)
else
    TOOLS+=(gcc)
fi
for t in "${TOOLS[@]}"; do
    command -v "$t" > /dev/null 2>&1 || { red "missing: $t"; exit 1; }
done

# ── Source ────────────────────────────────────────────────────────
mkdir -p "$SRC_CACHE"
cd "$SRC_CACHE"

TARBALL="linux-${LINUX_VERSION}.tar.xz"
# Each architecture builds in its own tree: a configured source tree
# cannot also serve an O= build.
SRCDIR="linux-${LINUX_VERSION}${TREE_SUFFIX}"

if [ ! -f "$TARBALL" ]; then
    cyan "downloading linux-${LINUX_VERSION}.tar.xz"
    curl -L --fail -o "$TARBALL.part" \
        "https://cdn.kernel.org/pub/linux/kernel/v6.x/${TARBALL}"
    mv "$TARBALL.part" "$TARBALL"
fi

# Checked on every run, cached tarball included: the guest kernel is
# signed and shipped, so its source must be exactly what kernel.org
# published for this version.
if ! echo "${LINUX_SHA256}  $TARBALL" | sha256sum -c - >/dev/null 2>&1; then
    red "sha256 mismatch for $TARBALL"
    red "  expected: ${LINUX_SHA256}"
    red "  got:      $(sha256sum "$TARBALL" | awk '{print $1}')"
    red "  (version bump? take the new value from kernel.org sha256sums.asc)"
    exit 1
fi
cyan "sha256 ok: $TARBALL"

if [ ! -d "$SRCDIR" ]; then
    cyan "extracting (~1.5 GB)"
    if [ -n "$TREE_SUFFIX" ]; then
        mkdir -p "$SRCDIR"
        tar xf "$TARBALL" -C "$SRCDIR" --strip-components=1
    else
        tar xf "$TARBALL"
    fi
fi

cd "$SRCDIR"

# ── Patches ───────────────────────────────────────────────────────
# Applied once to the cached tree; one that reverses cleanly is in.
for p in "$SCRIPT_DIR"/patches/*.patch; do
    [ -f "$p" ] || continue
    if patch -p1 -R --dry-run -s -f < "$p" > /dev/null 2>&1; then
        continue
    fi
    cyan "patching: $(basename "$p")"
    patch -p1 -s -f < "$p"
done

# ── Configure ─────────────────────────────────────────────────────
# Detect whether the config has changed since last build — if not,
# skip the merge step (which would mark every dependent as dirty).
# The shared fragment carries x86 symbols; on arm64 they are dropped
# before the merge, then the arm64 fragment goes on top.
FRAGMENTS=("$CONFIG_FRAGMENT")
if [ "$ARCH" = "arm64" ]; then
    grep -Ev 'CONFIG_(X86_|IA32_|KERNEL_(GZIP|LZMA|XZ|LZO|LZ4|ZSTD)|LEGACY_VSYSCALL|MODIFY_LDT|AGP|DRM_I915|INTEL_MEI|SERIO_I8042|KEYBOARD_ATKBD)' \
        "$CONFIG_FRAGMENT" > .nopeek-shared.config
    FRAGMENTS=(.nopeek-shared.config "$ARCH_FRAGMENT")
fi
cat "${FRAGMENTS[@]}" > .nopeek-config-stamp.new

NEEDS_RECONFIG=1
if [ -f .config ] && [ -f .nopeek-config-stamp ]; then
    if cmp -s .nopeek-config-stamp.new .nopeek-config-stamp; then
        NEEDS_RECONFIG=0
    fi
fi

if [ "$NEEDS_RECONFIG" = "1" ]; then
    cyan "configuring ($DEFCONFIG + nopeek overlay)"
    make "${MAKE_ARGS[@]}" "$DEFCONFIG" > /dev/null
    if [ "$ARCH" = "arm64" ]; then
        # The guest has no module loader, and with MODULES off every =m
        # would be built in: arm64's defconfig modules are dropped instead.
        sed -i 's/^\(CONFIG_[A-Za-z0-9_]*\)=m$/# \1 is not set/' .config
    fi
    KCONFIG_CONFIG=.config ./scripts/kconfig/merge_config.sh \
        -m -O . .config "${FRAGMENTS[@]}" > /dev/null
    make "${MAKE_ARGS[@]}" olddefconfig > /dev/null
    cp .nopeek-config-stamp.new .nopeek-config-stamp
fi

# ── Build ─────────────────────────────────────────────────────────
cyan "compiling $TARGET with -j$JOBS (this takes a few minutes)"
make "${MAKE_ARGS[@]}" -j"$JOBS" "$TARGET"

# ── Install ───────────────────────────────────────────────────────
mkdir -p "$(dirname "$OUT")"
cp "$BUILT" "$OUT"
SIZE=$(stat -c%s "$OUT")
green "Built linux-${LINUX_VERSION}-nopeek: ${SIZE} bytes → ${OUT}"
