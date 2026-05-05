#!/usr/bin/env bash
# microvm-linux/build.sh — build the Linux kernel for the nopeekOS MicroVM.
#
# Downloads kernel.org source (cached in ~/.cache/nopeekos/linux-src/),
# applies our config overlay on top of x86_64_defconfig, builds bzImage,
# copies the result into release/assets/linux-virt.bzImage where the
# installer + OTA pipeline pick it up.
set -euo pipefail

LINUX_VERSION="${LINUX_VERSION:-6.18.26}"
SRC_CACHE="${SRC_CACHE:-$HOME/.cache/nopeekos/linux-src}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CONFIG_FRAGMENT="$SCRIPT_DIR/nopeek-virt.config"
OUT="${OUT:-$REPO_ROOT/release/assets/linux-virt.bzImage}"

JOBS="${JOBS:-$(nproc 2>/dev/null || echo 4)}"

cyan()  { printf '\033[0;36m[npk]\033[0m %s\n' "$1"; }
green() { printf '\033[0;32m[npk]\033[0m %s\n' "$1"; }
red()   { printf '\033[0;31m[npk]\033[0m %s\n' "$1" >&2; }

# ── Sanity ────────────────────────────────────────────────────────
for t in gcc make flex bison openssl curl xz tar pkg-config; do
    command -v "$t" > /dev/null 2>&1 || { red "missing: $t"; exit 1; }
done

# ── Source ────────────────────────────────────────────────────────
mkdir -p "$SRC_CACHE"
cd "$SRC_CACHE"

TARBALL="linux-${LINUX_VERSION}.tar.xz"
SRCDIR="linux-${LINUX_VERSION}"

if [ ! -f "$TARBALL" ]; then
    cyan "downloading linux-${LINUX_VERSION}.tar.xz"
    curl -L --fail -o "$TARBALL.part" \
        "https://cdn.kernel.org/pub/linux/kernel/v6.x/${TARBALL}"
    mv "$TARBALL.part" "$TARBALL"
fi

if [ ! -d "$SRCDIR" ]; then
    cyan "extracting (~1.5 GB)"
    tar xf "$TARBALL"
fi

cd "$SRCDIR"

# ── Configure ─────────────────────────────────────────────────────
# Detect whether the config has changed since last build — if not,
# skip the merge step (which would mark every dependent as dirty).
NEEDS_RECONFIG=1
if [ -f .config ] && [ -f .nopeek-config-stamp ]; then
    if cmp -s "$CONFIG_FRAGMENT" .nopeek-config-stamp; then
        NEEDS_RECONFIG=0
    fi
fi

if [ "$NEEDS_RECONFIG" = "1" ]; then
    cyan "configuring (defconfig + nopeek overlay)"
    make x86_64_defconfig > /dev/null
    KCONFIG_CONFIG=.config ./scripts/kconfig/merge_config.sh \
        -m -O . .config "$CONFIG_FRAGMENT" > /dev/null
    make olddefconfig > /dev/null
    cp "$CONFIG_FRAGMENT" .nopeek-config-stamp
fi

# ── Build ─────────────────────────────────────────────────────────
cyan "compiling bzImage with -j$JOBS (this takes a few minutes)"
make -j"$JOBS" bzImage

# ── Install ───────────────────────────────────────────────────────
mkdir -p "$(dirname "$OUT")"
cp arch/x86/boot/bzImage "$OUT"
SIZE=$(stat -c%s "$OUT")
green "Built linux-${LINUX_VERSION}-nopeek: ${SIZE} bytes → ${OUT}"
