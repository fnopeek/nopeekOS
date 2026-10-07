#!/usr/bin/env bash
# microvm-userspace/build.sh: build a Linux userspace bundle for the
# nopeekOS MicroVM.
#
# Output: release/apps/<name>/<name>-<ver>.cpio.gz (signed in
# release/apps/<name>/<name>-<ver>.cpio.gz.sig by `./build.sh release`).
#
# The bundle is not embedded in the regular kernel binary. `install
# <name>` fetches it OTA-style, verifies ECDSA P-384 and stores it in
# npkFS; `microvm <name>` loads it from there.
#
# Contents: Alpine minirootfs + our PID-1 as /init + the apks below.
#
# Trust split (see docs/archive/PHASE12_MICROVM.md):
#   - Kernel + microvm-init: compiled + signed by us.
#   - Alpine minirootfs: pinned tarball + pinned sha256.
#   - apks: package names here, versions from the pinned Alpine branch;
#     apk verifies them against the signed index.

set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────

# Alpine release tracking. Bump these together and give the reason in
# the commit message. A stable branch does not follow the browser:
# rebuilding on the same branch only brings library patches. Check
# `librewolf` in the next branch's APKINDEX before assuming a rebuild
# updates it.
ALPINE_BRANCH="${ALPINE_BRANCH:-v3.24}"
ALPINE_VERSION="${ALPINE_VERSION:-3.24.2}"
ALPINE_MINIROOTFS_SHA256="${ALPINE_MINIROOTFS_SHA256:-c5ca053cfe1d85c5b96dff8b9bc57045f7f184a30ffb6b65776409ca90388677}"

# Output naming; the bundle is named after its main client.
BUNDLE_NAME="${BUNDLE_NAME:-librewolf}"
BUNDLE_VERSION="${BUNDLE_VERSION:-${ALPINE_VERSION}}"

# Apks to add on top of minirootfs. Package names only; versions come
# from the pinned Alpine branch (patch level via ALPINE_VERSION).
#   cage      - kiosk Wayland compositor (one client = the browser)
#   librewolf - the browser (Firefox fork; pulls gtk/nss/fontconfig/
#               freetype/... via apk deps)
#   seatd     - libseat daemon (Alpine libseat has no builtin backend)
#   libinput  - wlroots evdev input
#   eudev     - udevd + udevadm + libudev. wlroots' libinput and
#               DRM/session backends discover devices via the udev
#               monitor; without udev they find no input devices and
#               never see the DRM connector hotplug raised on a tile
#               resize. launch_wayland runs udevd, `udevadm trigger`
#               and `settle` before cage.
#   mesa-gbm  - wlroots DRM backend buffer alloc (pixman SW renderer,
#               no GL driver)
# font-dejavu + font-noto: the minirootfs ships no fonts, so without them
# all GTK and web text renders as tofu. dejavu covers Latin UI/body;
# noto adds broad Unicode and a sans default.
# alsa-lib: libasound for LibreWolf's cubeb ALSA backend (no PulseAudio /
# PipeWire in the bundle). cubeb opens the ALSA "default" PCM, which is
# the virtio-snd card (card 0) backed by the host audio path.
APK_PACKAGES="${APK_PACKAGES:-cage librewolf seatd libinput eudev mesa-gbm font-dejavu font-noto alsa-lib}"

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

for t in curl sha256sum tar bsdtar gzip cargo unshare chroot; do
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
# our pin drifted past available retention; bail either way.
echo "${ALPINE_MINIROOTFS_SHA256}  $CACHE/$TARBALL" | sha256sum -c - >/dev/null \
    || { red "sha256 mismatch on $TARBALL"; exit 1; }
green "verified $TARBALL ($(stat -c%s "$CACHE/$TARBALL") bytes)"

# ── Build PID-1 (idempotent; `cargo build --release` no-ops if fresh)

cyan "building PID-1 (microvm-init)"
(cd "$PID1_DIR" && cargo build --release --locked >/dev/null 2>&1)
[ -f "$PID1_BIN" ] || { red "PID-1 build did not produce $PID1_BIN"; exit 1; }

# ── Stage rootfs ─────────────────────────────────────────────────

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cyan "staging Alpine minirootfs"
tar -xzf "$CACHE/$TARBALL" -C "$STAGE"

# Resolv.conf for apk's CDN lookups inside the chroot. Without this
# `apk add` fails at DNS resolution (no resolver inside the bare
# rootfs).
echo "nameserver 1.1.1.1" > "$STAGE/etc/resolv.conf"

# ── Add apks via unshared-namespace chroot ───────────────────────
#
# Run Alpine's own apk binary inside a chroot via `unshare -r` so
# we don't need real root on the host. apk handles trust-chain
# verification (signed indexes + signed apks against /etc/apk/keys
# which the minirootfs already populated).
#
# `--pid --fork --mount-proc` gives apk a fresh /proc inside the
# chroot (some post-install scripts need it). `--mount` isolates
# mount changes from the host namespace.
if [ -n "$APK_PACKAGES" ]; then
    cyan "apk add: $APK_PACKAGES"
    APK_CACHE_DIR="$CACHE/apks"
    mkdir -p "$APK_CACHE_DIR"
    # Bind the host cache dir into the chroot so apk reuses
    # downloaded .apks across builds.
    mkdir -p "$STAGE/var/cache/apk"

    unshare --user --map-root-user --mount --pid --fork --mount-proc \
        sh -c "
            set -e
            mount --bind '$APK_CACHE_DIR' '$STAGE/var/cache/apk'
            chroot '$STAGE' /sbin/apk update 2>&1 | tail -3
            chroot '$STAGE' /sbin/apk add --no-progress $APK_PACKAGES 2>&1 | tail -20
            # Pre-build the fontconfig cache inside the image. The
            # guest has no RTC, so font dirs can look newer than its
            # clock ('mtime in the future', fonts not scanned, tofu).
            # A prebuilt cache serves fonts without a runtime scan.
            chroot '$STAGE' /usr/bin/fc-cache -f 2>&1 | tail -3 || true
        "

    # Apk leaves /var/cache/apk populated with symlinks/copies; wipe
    # it so the cpio doesn't carry duplicate-of-host-cache bytes.
    rm -rf "$STAGE/var/cache/apk"/*

    # Drop the temporary resolver; the guest sets its own.
    : > "$STAGE/etc/resolv.conf"
fi

# Install our PID-1 as /init: Linux execs /init as PID-1 when an
# initramfs is in use. Overrides anything Alpine ships (openrc lives
# at /sbin/init).
cp "$PID1_BIN" "$STAGE/init"
chmod +x "$STAGE/init"

# /etc/motd so an interactive shell shows it is inside the microvm.
cat > "$STAGE/etc/motd" <<'MOTD'

   nopeekOS MicroVM — Alpine userspace iteration 1

   You are inside a hardware-isolated Linux microvm.
   Substrate: VMX/SVM + EPT/NPT, 1 GB RAM, virtio-blk/net/gpu/input.

MOTD

# /etc/machine-id: LibreWolf's a11y stack spawns a DBus session bus,
# which refuses to start without one ("Cannot spawn a message bus
# without a machine-id") and costs a startup timeout. dbus also checks
# /var/lib/dbus/machine-id. A fixed 32-hex value (not dbus-uuidgen):
# the squashfs is signed over its bytes, so the image must be
# byte-reproducible, and with a single VM a constant id is correct.
MACHINE_ID="2d8b9f1c4e6a7d0b3f5c8e1a9d4b7c0e"
printf '%s\n' "$MACHINE_ID" > "$STAGE/etc/machine-id"
mkdir -p "$STAGE/var/lib/dbus"
ln -sfn /etc/machine-id "$STAGE/var/lib/dbus/machine-id"

# ── Pack as cpio.gz (newc format = what Linux's initramfs unpacker
#    expects). Strip uid/gid noise to keep the cpio reproducible
#    across rebuilds; the sig is over the bytes.

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

# ── 'current' pointer: symlink to the latest version for OTA.
ln -sfn "${BUNDLE_NAME}-${BUNDLE_VERSION}.cpio.gz"      "$OUT_DIR/current.cpio.gz"
ln -sfn "${BUNDLE_NAME}-${BUNDLE_VERSION}.manifest"     "$OUT_DIR/current.manifest"

green "built ${BUNDLE_NAME} ${BUNDLE_VERSION}: ${SIZE} bytes → ${OUT}"
cyan  "       sha256: ${SHA}"

# Stage as OTA asset. The AssetSpec section `microvm:userspace`
# (kernel/src/intent/update.rs) maps to `sys/microvm/userspace.cpio.gz`
# in npkFS. `microvm linux` picks it up on next launch.
#
# Two distribution lanes, depending on size:
#   * < ~30 MB → release/assets/microvm-userspace.cpio.gz
#                (raw.githubusercontent.com via main, signed by `./build.sh release`).
#   * ≥ ~30 MB → release/assets/large/microvm-userspace.cpio.gz
#                (GitHub Releases via `./build.sh release-large <tag>`).
#
# The kernel asset manifest carries a `url=` line for the large
# lane; `https_get_streaming` follows the 302 redirect chain to
# objects.githubusercontent.com transparently. This script stages to
# the small lane; if the bundle is too large for raw, move the file
# into `large/` and use `release-large`.
SIZE_BYTES=$(stat -c%s "$OUT")
if [ "$SIZE_BYTES" -lt $((30 * 1024 * 1024)) ]; then
    OTA_SLOT="$REPO_ROOT/release/assets/microvm-userspace.cpio.gz"
    cp "$OUT" "$OTA_SLOT"
    cyan  "       staged to OTA (small lane): ${OTA_SLOT}"
    cyan  "       sign with: ./build.sh release"
else
    LARGE_DIR="$REPO_ROOT/release/assets/large"
    mkdir -p "$LARGE_DIR"
    OTA_SLOT="$LARGE_DIR/microvm-userspace.cpio.gz"
    cp "$OUT" "$OTA_SLOT"
    cyan  "       staged to OTA (large lane, ${SIZE_BYTES} bytes): ${OTA_SLOT}"
    cyan  "       upload with: ./build.sh release-large assets/<your-tag>"
fi

# ── Squashfs form: the path PID-1 actually loads ─────────────────
#
# PID-1's `try_switch_to_sqfs` mounts the bundle read-only from
# /dev/vdb (slot-5 RO virtio-blk) and chroots into it; decompress-on-read
# is cheaper in RAM than unpacking the cpio into a tmpfs.
#
# zstd: the guest inflates every block on first touch, and that is most of
# an app's start. Measured with LibreWolf, start to first full window:
# gzip 4.5 s, zstd 3.2 s, uncompressed 2.8-3.1 s at almost three times the
# bytes to read and hold.
SQFS_OUT="$OUT_DIR/${BUNDLE_NAME}-${BUNDLE_VERSION}.sqfs"
cyan "building squashfs (zstd) — PID-1's load path"
# -all-root: everything root-owned inside the guest. -noappend:
# fresh image. -no-xattrs: our minimal guest doesn't carry them.
# -all-time/-mkfs-time 0: epoch (1970) timestamps. The guest has
# no RTC; real mtimes look "in the future" to it, which makes
# fontconfig refuse to scan the font dirs ("mtime in the future. New
# fonts may not be detected", tofu). Epoch is in the past for any
# guest clock, and it makes the image reproducible (the .sig is over
# the bytes).
mksquashfs "$STAGE" "$SQFS_OUT.tmp" \
    -comp zstd -all-root -noappend -no-xattrs \
    -all-time 0 -mkfs-time 0 -quiet \
    >/dev/null
mv "$SQFS_OUT.tmp" "$SQFS_OUT"

SQFS_SIZE=$(stat -c%s "$SQFS_OUT")
SQFS_SHA=$(sha256sum "$SQFS_OUT" | awk '{print $1}')
ln -sfn "${BUNDLE_NAME}-${BUNDLE_VERSION}.sqfs" "$OUT_DIR/current.sqfs"
green "built squashfs: ${SQFS_SIZE} bytes → ${SQFS_OUT}"
cyan  "       sha256: ${SQFS_SHA}"

# Same two-lane staging as the cpio. `microvm-userspace.sqfs` maps to
# AssetSpec [microvm:userspace-sqfs] (build.sh release signs it).
if [ "$SQFS_SIZE" -lt $((30 * 1024 * 1024)) ]; then
    SQFS_SLOT="$REPO_ROOT/release/assets/microvm-userspace.sqfs"
    cp "$SQFS_OUT" "$SQFS_SLOT"
    cyan  "       staged to OTA (small lane): ${SQFS_SLOT}"
    cyan  "       sign with: ./build.sh release"
else
    LARGE_DIR="$REPO_ROOT/release/assets/large"
    mkdir -p "$LARGE_DIR"
    SQFS_SLOT="$LARGE_DIR/microvm-userspace.sqfs"
    cp "$SQFS_OUT" "$SQFS_SLOT"
    cyan  "       staged to OTA (large lane, ${SQFS_SIZE} bytes): ${SQFS_SLOT}"
    cyan  "       upload with: ./build.sh release-large assets/<your-tag>"
fi
