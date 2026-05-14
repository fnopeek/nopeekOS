#!/usr/bin/env bash
# microvm-userspace/build.sh — build a Linux userspace bundle for the
# nopeekOS MicroVM.
#
# Output: release/apps/<name>/<name>-<ver>.cpio.gz (signed in
# release/apps/<name>/<name>-<ver>.cpio.gz.sig by `./build.sh release`).
#
# The bundle is **not** embedded in the kernel binary. Distribution
# path: `install <name>` fetches it OTA-style via GitHub raw, verifies
# ECDSA P-384, stores in npkFS. `microvm <name>` then loads it from
# npkFS as a second initramfs (or future: read-only virtio-blk sqfs).
#
# Iteration 1: just Alpine minirootfs + our PID-1 as /init. Gives an
# interactive busybox-ash shell inside the MicroVM.
# Iteration 2: + Mesa + Wayland-libs (apk add).
# Iteration 3: + LibreWolf (built from Alpine APKBUILD on top).
#
# Trust split (see PHASE12_MICROVM.md):
#   - Kernel + microvm-init: compiled + signed by us, fully audited.
#   - Alpine minirootfs: pinned tarball + pinned sha256.
#   - Future apks: pinned version + pinned blake3 per package
#     from a pinned Alpine snapshot date.

set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────

# Alpine release tracking. Bump these together, document the reason
# in the commit message. Current selection: 3.23 stable, ~Q3 2025
# release line. Older still-supported lines if needed (security
# context): 3.20.10, 3.21.7, 3.22.4. Each major gets 2 years upstream.
ALPINE_BRANCH="${ALPINE_BRANCH:-v3.23}"
ALPINE_VERSION="${ALPINE_VERSION:-3.23.4}"
ALPINE_MINIROOTFS_SHA256="${ALPINE_MINIROOTFS_SHA256:-85498865362aa7ebececa0d725a2f2e4db7ac4e4b2850b8df21645afa0d03ee3}"

# Output naming. `alpine-base` for iteration 1 (no apks added).
# Iteration 3 will rename to `librewolf` once LibreWolf is on top.
BUNDLE_NAME="${BUNDLE_NAME:-alpine-base}"
BUNDLE_VERSION="${BUNDLE_VERSION:-${ALPINE_VERSION}}"

# ── Paths ────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CACHE="${HOME}/.cache/nopeekos/alpine"
OUT_DIR="$REPO_ROOT/release/apps/${BUNDLE_NAME}"
OUT="$OUT_DIR/${BUNDLE_NAME}-${BUNDLE_VERSION}.cpio.gz"

PID1_DIR="$REPO_ROOT/microvm/linux/init"
PID1_BIN="$PID1_DIR/target/x86_64-unknown-linux-gnu/release/microvm-init"

cyan()  { printf '\033[0;36m[npk]\033[0m %s\n' "$1"; }
green() { printf '\033[0;32m[npk]\033[0m %s\n' "$1"; }
red()   { printf '\033[0;31m[npk]\033[0m %s\n' "$1" >&2; }

# ── Sanity ───────────────────────────────────────────────────────

for t in curl sha256sum tar bsdtar gzip cargo; do
    command -v "$t" >/dev/null 2>&1 || { red "missing: $t"; exit 1; }
done

# ── Download Alpine minirootfs (cached, integrity-checked) ───────

mkdir -p "$CACHE"
TARBALL="alpine-minirootfs-${ALPINE_VERSION}-x86_64.tar.gz"

if [ ! -f "$CACHE/$TARBALL" ]; then
    cyan "downloading $TARBALL"
    curl -L --fail --max-time 60 \
        -o "$CACHE/$TARBALL.part" \
        "https://dl-cdn.alpinelinux.org/alpine/${ALPINE_BRANCH}/releases/x86_64/${TARBALL}"
    mv "$CACHE/$TARBALL.part" "$CACHE/$TARBALL"
fi

# Verify against pinned hash. Mismatch = either upstream tampering or
# our pin drifted past available retention — bail loudly either way.
echo "${ALPINE_MINIROOTFS_SHA256}  $CACHE/$TARBALL" | sha256sum -c - >/dev/null \
    || { red "sha256 mismatch on $TARBALL"; exit 1; }
green "verified $TARBALL ($(stat -c%s "$CACHE/$TARBALL") bytes)"

# ── Build PID-1 (idempotent — `cargo build --release` no-ops if fresh)

cyan "building PID-1 (microvm-init)"
(cd "$PID1_DIR" && cargo build --release >/dev/null 2>&1)
[ -f "$PID1_BIN" ] || { red "PID-1 build did not produce $PID1_BIN"; exit 1; }

# ── Stage rootfs ─────────────────────────────────────────────────

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cyan "staging Alpine minirootfs"
tar -xzf "$CACHE/$TARBALL" -C "$STAGE"

# Drop our PID-1 as /init — Linux execs /init as PID-1 when an
# initramfs is in use. Overrides anything Alpine ships (Alpine's
# minirootfs typically has no /init, openrc lives at /sbin/init,
# but be safe).
cp "$PID1_BIN" "$STAGE/init"
chmod +x "$STAGE/init"

# Friendly /etc/motd so an interactive shell session greets us with
# clear "yes you're in the microvm" signal.
cat > "$STAGE/etc/motd" <<'MOTD'

   nopeekOS MicroVM — Alpine userspace iteration 1

   You are inside a hardware-isolated Linux microvm.
   Substrate: VMX/SVM + EPT/NPT, 1 GB RAM, virtio-blk/net/gpu/input.

MOTD

# ── Pack as cpio.gz (newc format = what Linux's initramfs unpacker
#    expects). Strip uid/gid noise to keep the cpio reproducible
#    across rebuilds — needed because the sig is over the bytes.

cyan "packing bundle ($(find "$STAGE" -type f | wc -l) files)"
mkdir -p "$OUT_DIR"

(
    cd "$STAGE"
    # `--uid 0 --gid 0` forces all entries to root-owned (we're not root
    # on the host, but inside the microvm everything wants root).
    # `--mtime` zeros timestamps for reproducibility.
    find . -mindepth 1 | LC_ALL=C sort | \
        bsdtar --format newc \
               --uid 0 --gid 0 \
               --mtime '1970-01-01 00:00:00 UTC' \
               -cf - -T -
) | gzip -9n > "$OUT.tmp"

mv "$OUT.tmp" "$OUT"

SIZE=$(stat -c%s "$OUT")
SHA=$(sha256sum "$OUT" | awk '{print $1}')

# ── Manifest ─────────────────────────────────────────────────────
#
# Stored alongside the bundle so `install` knows what version it's
# fetching and `microvm <name>` knows the expected format. ECDSA sig
# happens later in `build.sh release`.

cat > "$OUT_DIR/${BUNDLE_NAME}-${BUNDLE_VERSION}.manifest" <<MANIFEST
name      = "${BUNDLE_NAME}"
version   = "${BUNDLE_VERSION}"
format    = "cpio.gz"
size      = ${SIZE}
sha256    = "${SHA}"
alpine    = "${ALPINE_VERSION}"
MANIFEST

# ── 'current' pointer — symlink to latest version for OTA convenience.
ln -sfn "${BUNDLE_NAME}-${BUNDLE_VERSION}.cpio.gz"      "$OUT_DIR/current.cpio.gz"
ln -sfn "${BUNDLE_NAME}-${BUNDLE_VERSION}.manifest"     "$OUT_DIR/current.manifest"

green "built ${BUNDLE_NAME} ${BUNDLE_VERSION}: ${SIZE} bytes → ${OUT}"
cyan  "       sha256: ${SHA}"

# Stage as OTA asset — `microvm:rootfs` is what `intent::update` picks
# up and writes to `sys/microvm/rootfs.cpio.gz` in npkFS. Then
# `microvm linux` finds it on next launch. Cleaner architecture
# (per-app `install <name>` flow into release/apps/) comes later;
# release/assets/ piggybacks the existing OTA pipeline for iter-1.
OTA_SLOT="$REPO_ROOT/release/assets/microvm-rootfs.cpio.gz"
cp "$OUT" "$OTA_SLOT"
cyan  "       staged to OTA: ${OTA_SLOT}"
cyan  "       sign with: ./build.sh release"
