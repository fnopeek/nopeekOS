#!/usr/bin/env bash
# microvm-userspace/build.sh: build a Linux userspace bundle for the
# nopeekOS MicroVM.
#
# Output: release/apps/<name>/<name>-<ver>.{cpio.gz,sqfs,manifest} (signed
# by `./build.sh release` / `./build.sh release-large`).
#
# The bundle is not embedded in the regular kernel binary. `install
# <name>` fetches it OTA-style, verifies ECDSA P-384 and stores it in
# npkFS; `microvm <name>` loads it from there.
#
# Contents: Arch Linux packages from the pinned snapshot + our PID-1 as
# /init.
#
# Trust split:
#   - Kernel + microvm-init: compiled + signed by us.
#   - Arch bootstrap tarball: pinned sha256 + Arch's release signature.
#   - Packages: names here, versions from the pinned Arch Linux Archive
#     snapshot; pacman verifies each against Arch's keyring.
#
# The version is the main client's own plus the snapshot, e.g.
# firefox-159.0-arch20261029: what the bundle runs, and on which base.

set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$REPO_ROOT/microvm/linux/arch.sh"

# The bundle is named after its main client.
BUNDLE_NAME="${BUNDLE_NAME:-firefox}"

# Packages on top of the bootstrap's base.
#   firefox   - the browser (pulls gtk/nss/fontconfig/freetype/... as deps)
#   seatd     - libseat and its daemon (the guest runs no logind)
#   libinput  - wlroots evdev input
#   systemd   - systemd-udevd + udevadm only; PID-1 is ours. wlroots'
#               libinput and DRM/session backends discover devices via the
#               udev monitor; without udev they find no input devices and
#               never see the DRM connector hotplug raised on a tile
#               resize. The session runs udevd, `udevadm trigger` and
#               `settle` before npkwm.
#   mesa, vulkan-intel - GBM/EGL for wlroots and the GPU drivers; their
#               files are replaced by our build (microvm/linux/mesa).
# ttf-dejavu + noto-fonts: the base ships no fonts, so without them all GTK
# and web text renders as tofu. dejavu covers Latin UI/body; noto adds
# broad Unicode and a sans default.
# wayland, libdisplay-info: libraries npkwm links (it carries wlroots
# itself); checked against its NEEDED list below.
# alsa-lib: libasound for Firefox's cubeb ALSA backend (no PulseAudio /
# PipeWire in the bundle). cubeb opens the ALSA "default" PCM, which is
# the virtio-snd card (card 0) backed by the host audio path.
# mesa-utils: es2gears_wayland and friends, GL test clients the session
# runs instead of the browser with npkapp= on the guest command line.
# Video decodes through VA-API: libva, intel-gmmlib for our build of Intel's
# media driver (below), libva-utils for `vainfo`, ffmpeg for Firefox's
# decoder and `ffmpeg -hwaccel vaapi`.
# gdb stays while the GPU paths are new (`npkgdb`).
# busybox: the session scripts in PID-1 are written for its applets (wget,
# nc, su without PAM, halt -f); linked in /usr/libexec/npk/bb, which the
# session puts first in PATH.
COMMON_PACKAGES="busybox seatd libinput systemd mesa vulkan-intel vulkan-icd-loader wayland libdisplay-info ttf-dejavu noto-fonts alsa-lib mesa-utils ffmpeg gdb"
case "$BUNDLE_NAME" in
    firefox) DEFAULT_PACKAGES="firefox libva libva-utils intel-gmmlib $COMMON_PACKAGES" ;;
    *)       echo "unknown bundle: $BUNDLE_NAME" >&2; exit 1 ;;
esac
PACKAGES="${PACKAGES:-$DEFAULT_PACKAGES}"
PKG_LIST="$SCRIPT_DIR/packages-${BUNDLE_NAME}.txt"
# The OTA asset name, section microvm:<name>-sqfs.
ASSET_NAME="microvm-${BUNDLE_NAME}"
SNAPSHOT_TAG="arch${ARCH_SNAPSHOT//\//}"

OUT_DIR="$REPO_ROOT/release/apps/${BUNDLE_NAME}"

PID1_DIR="$REPO_ROOT/microvm/linux/init"
PID1_BIN="$PID1_DIR/target/x86_64-unknown-linux-gnu/release/microvm-init"

source "$REPO_ROOT/microvm/linux/mesa/mesa.env"
MESA_OVERLAY="${HOME}/.cache/nopeekos/mesa/mesa-${MESA_VERSION}-npk.tar.gz"
source "$REPO_ROOT/microvm/linux/ihd/ihd.env"
IHD_OVERLAY="${HOME}/.cache/nopeekos/ihd/ihd-${IHD_VERSION}-npk.tar.gz"

cyan()  { printf '\033[0;36m[npk]\033[0m %s\n' "$1"; }
green() { printf '\033[0;32m[npk]\033[0m %s\n' "$1"; }
red()   { printf '\033[0;31m[npk]\033[0m %s\n' "$1" >&2; }

# ── Sanity ───────────────────────────────────────────────────────

for t in curl sha256sum gpg bsdtar gzip cargo unshare readelf mksquashfs; do
    command -v "$t" >/dev/null 2>&1 || { red "missing: $t"; exit 1; }
done
[ -f "$MESA_OVERLAY" ] || { red "missing $MESA_OVERLAY — run microvm/linux/mesa/build.sh"; exit 1; }
[ -f "$IHD_OVERLAY" ]  || { red "missing $IHD_OVERLAY — run microvm/linux/mesa/build.sh"; exit 1; }

arch_fetch
green "verified $ARCH_TARBALL, packages from snapshot $ARCH_SNAPSHOT"

# ── Build PID-1 (idempotent; `cargo build --release` no-ops if fresh)

cyan "building PID-1 (microvm-init)"
(cd "$PID1_DIR" && cargo build --release --locked >/dev/null 2>&1)
[ -f "$PID1_BIN" ] || { red "PID-1 build did not produce $PID1_BIN"; exit 1; }

# ── Stage rootfs and pack, all inside the namespace ──────────────
#
# The root's files belong to sub-ids on the host (microvm/linux/arch.sh),
# so everything that reads them, packing included, runs in `arch_run`.

WORK="$(mktemp -d)"
trap 'arch_rm "$WORK"' EXIT
mkdir -p "$OUT_DIR"

export MESA_VERSION MESA_OVERLAY IHD_OVERLAY PKG_LIST PID1_BIN REPO_ROOT OUT_DIR
export BUNDLE_NAME SNAPSHOT_TAG ARCH_SNAPSHOT WORK
arch_run "$WORK/root" "
    cyan()  { printf '\033[0;36m[npk]\033[0m %s\n' \"\$1\"; }
    red()   { printf '\033[0;31m[npk]\033[0m %s\n' \"\$1\" >&2; exit 1; }
    cyan 'pacman: $PACKAGES'
    in_root pacman -Syu --noconfirm --needed $PACKAGES > \$WORK/pacman.log 2>&1 \
        || { tail -20 \$WORK/pacman.log; red 'pacman failed'; }
    in_root pacman -Q | LC_ALL=C sort > \$PKG_LIST
    FIREFOX_VERSION=\$(in_root pacman -Q $BUNDLE_NAME | awk '{print \$2}' | sed 's/^[0-9]*://; s/-[0-9.]*\$//')
    echo \"\$FIREFOX_VERSION\" > \$WORK/client-version

    # Mesa with nopeekOS's native-context patches (microvm/linux/mesa)
    # replaces the files of Arch's mesa and vulkan-intel. Same release only:
    # libEGL_mesa, libgbm and libgallium share an internal ABI.
    arch_mesa=\$(in_root pacman -Q mesa | awk '{print \$2}' | sed 's/^[0-9]*://; s/-[0-9.]*\$//')
    [ \"\$arch_mesa\" = \"\$MESA_VERSION\" ] || red \"Arch mesa is \$arch_mesa, mesa.env pins \$MESA_VERSION\"
    in_root pacman -Qlq mesa vulkan-intel | grep -v '/\$' | while read -r f; do rm -f \"\$R\$f\"; done
    tar -xzf \"\$MESA_OVERLAY\" -C \$R
    # Intel's media driver built against that Mesa's libnpk_intel_virtio;
    # Arch's intel-media-driver is not installed.
    tar -xzf \"\$IHD_OVERLAY\" -C \$R
    for so in \$(tar -tzf \"\$MESA_OVERLAY\"; tar -tzf \"\$IHD_OVERLAY\"); do
        [ -L \"\$R/\$so\" ] || [ ! -f \"\$R/\$so\" ] && continue
        [ \"\$(head -c 4 \"\$R/\$so\")\" = \"\$(printf '\177ELF')\" ] || continue
        for lib in \$(readelf -d \"\$R/\$so\" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'); do
            [ -e \"\$R/usr/lib/\$lib\" ] || red \"\$so needs \$lib, not in the image\"
        done
    done
    cyan \"mesa \$MESA_VERSION and iHD with intel-virtio over Arch's packages\"

    mkdir -p \$R/usr/libexec/npk/bb
    in_root /usr/bin/busybox --install -s /usr/libexec/npk/bb

    # Noto as the browser needs it: Sans, Serif and Mono for Latin, Greek
    # and Cyrillic, plus math and symbols. Arch packages every script in
    # one package; the others are left out.
    find \$R/usr/share/fonts/noto -type f ! \( -name 'NotoSans-*' -o -name 'NotoSerif-*' \
        -o -name 'NotoSansMono-*' -o -name 'NotoSansMath-*' -o -name 'NotoSansSymbols-*' \
        -o -name 'NotoSansSymbols2-*' \) -delete

    # Pre-build the fontconfig cache inside the image. A cache is valid only
    # while its recorded directory mtime matches, and the image is packed
    # with every mtime at the epoch (see mksquashfs below): the font dirs
    # are set to the epoch first, or every app start rescans all fonts into
    # a tmpfs cache.
    find \$R/usr/share/fonts -type d -exec touch -h -d @0 {} +
    in_root fc-cache -f > /dev/null 2>&1 || true

    # Firefox's autoconfig, which the session extends with npkstats and
    # npksupport.
    if [ -x \$R/usr/lib/firefox/firefox ]; then
        mkdir -p \$R/usr/lib/firefox/defaults/pref
        printf '%s\n' 'pref(\"general.config.filename\", \"firefox.cfg\");' 'pref(\"general.config.obscure_value\", 0);' \
            > \$R/usr/lib/firefox/defaults/pref/autoconfig.js
        printf '%s\n' '// nopeekOS: the session appends its probes here.' > \$R/usr/lib/firefox/firefox.cfg
    fi

    # The compositor for a guest with a GPU (microvm/linux/npkwm/build.sh
    # gles2); the init runs it instead of PID-1's npkwm when the host offers
    # native contexts.
    install -D -m 0755 \$REPO_ROOT/microvm/linux/npkwm/npkwm-gles2 \$R/usr/libexec/npk/npkwm-gles2
    # npkwm is embedded in PID-1 and loads its libraries from this image, as
    # does npkwm-gles2: every library they need must be here, or the session
    # never starts.
    for lib in \$(readelf -d \$REPO_ROOT/microvm/linux/npkwm/npkwm \$REPO_ROOT/microvm/linux/npkwm/npkwm-gles2 | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | sort -u); do
        [ -e \$R/usr/lib/\$lib ] || red \"npkwm needs \$lib, not in the image\"
    done

    # Only what runs: no package manager state, keys, headers, static
    # libraries, documentation, introspection data or translations (the
    # session is English; Firefox carries its own strings), and no locale
    # sources (glibc's built-in C.UTF-8 is used).
    rm -rf \$R/etc/pacman.d/gnupg \$R/var/lib/pacman/sync \$R/var/log/* \
           \$R/usr/include \$R/usr/share/man \$R/usr/share/doc \$R/usr/share/info \$R/usr/share/gtk-doc \
           \$R/usr/lib/pkgconfig \$R/usr/share/pkgconfig \$R/usr/share/gir-1.0 \
           \$R/usr/share/locale \$R/usr/share/i18n
    find \$R/usr/lib -maxdepth 1 -name '*.a' -delete
    : > \$R/etc/resolv.conf

    # No set-uid or set-gid binaries: the app's processes are the attacker
    # in this guest, and nothing it runs needs to gain privileges.
    find \$R -xdev -type f -perm /6000 -exec chmod ug-s {} +

    # PID-1 as /init: Linux execs /init when an initramfs is in use.
    install -m 0755 \$PID1_BIN \$R/init

    printf '%s\n' '' '   nopeekOS MicroVM: Arch Linux userspace' '' \
        '   You are inside a hardware-isolated Linux microvm.' \
        '   Substrate: VMX/SVM + EPT/NPT, virtio-blk/net/gpu/input.' '' > \$R/etc/motd

    # /etc/machine-id: the browser's a11y stack spawns a DBus session bus,
    # which refuses to start without one and costs a startup timeout. dbus
    # also checks /var/lib/dbus/machine-id. A fixed value: with a single VM
    # a constant id is correct, and the image stays byte-reproducible.
    printf '%s\n' 2d8b9f1c4e6a7d0b3f5c8e1a9d4b7c0e > \$R/etc/machine-id
    mkdir -p \$R/var/lib/dbus
    ln -sfn /etc/machine-id \$R/var/lib/dbus/machine-id

    # Detached before packing, or the image would carry the host's /dev.
    for d in dev proc tmp; do umount -l \$R/\$d; done
    umount \$R/var/cache/pacman/pkg
    for d in dev proc tmp; do [ -z \"\$(ls -A \$R/\$d)\" ] || { ls -A \$R/\$d | head -5; red \"\$d is still mounted\"; }; done
    rm -rf \$R/var/cache/pacman
    VERSION=\"\$(cat \$WORK/client-version)-\$SNAPSHOT_TAG\"
    OUT=\$OUT_DIR/\$BUNDLE_NAME-\$VERSION

    cyan \"packing \$BUNDLE_NAME \$VERSION (\$(find \$R -type f | wc -l) files)\"
    # newc is what Linux's initramfs unpacker expects; owners and mtimes
    # zeroed so the bytes, which the signature covers, are reproducible.
    (cd \$R && find . -mindepth 1 | LC_ALL=C sort | \
        bsdtar --format newc --uid 0 --gid 0 --mtime '1970-01-01 00:00:00 UTC' -cf - -T -) \
        | gzip -9n > \$OUT.cpio.gz.tmp
    mv \$OUT.cpio.gz.tmp \$OUT.cpio.gz

    # The squashfs is what PID-1 loads: try_switch_to_sqfs mounts it
    # read-only from /dev/vdb and chroots into it. lz4 (high compression):
    # the guest inflates every block on first touch, and that is most of an
    # app's start. -all-time/-mkfs-time 0: the guest has no RTC, and real
    # mtimes look in the future to it, which makes fontconfig refuse to scan
    # the font dirs; the epoch is in the past for any guest clock.
    mksquashfs \$R \$OUT.sqfs.tmp -comp lz4 -Xhc -all-root -noappend -no-xattrs \
        -all-time 0 -mkfs-time 0 -quiet > /dev/null
    mv \$OUT.sqfs.tmp \$OUT.sqfs
    echo \"\$VERSION\" > \$WORK/version
"

VERSION="$(cat "$WORK/version")"
CLIENT_VERSION="$(cat "$WORK/client-version")"
OUT="$OUT_DIR/${BUNDLE_NAME}-${VERSION}"
SIZE=$(stat -c%s "$OUT.cpio.gz")
SHA=$(sha256sum "$OUT.cpio.gz" | awk '{print $1}')
SQFS_SIZE=$(stat -c%s "$OUT.sqfs")
SQFS_SHA=$(sha256sum "$OUT.sqfs" | awk '{print $1}')

# ── Manifest ─────────────────────────────────────────────────────
#
# Stored alongside the bundle so `install` knows what version it's
# fetching and `microvm <name>` knows the expected format. ECDSA sig
# happens later in `build.sh release`.
cat > "$OUT.manifest" <<MANIFEST
name      = "${BUNDLE_NAME}"
version   = "${VERSION}"
client    = "${BUNDLE_NAME} ${CLIENT_VERSION}"
base      = "Arch Linux ${ARCH_SNAPSHOT}"
format    = "cpio.gz"
size      = ${SIZE}
sha256    = "${SHA}"
MANIFEST

# 'current' pointers: the latest version for OTA.
ln -sfn "${BUNDLE_NAME}-${VERSION}.cpio.gz"  "$OUT_DIR/current.cpio.gz"
ln -sfn "${BUNDLE_NAME}-${VERSION}.manifest" "$OUT_DIR/current.manifest"
ln -sfn "${BUNDLE_NAME}-${VERSION}.sqfs"     "$OUT_DIR/current.sqfs"

green "built ${BUNDLE_NAME} ${VERSION}: cpio ${SIZE} bytes, squashfs ${SQFS_SIZE} bytes"
cyan  "       sha256 (sqfs): ${SQFS_SHA}"

# ── Stage as OTA assets ──────────────────────────────────────────
#
# Two distribution lanes, depending on size:
#   * < ~30 MB → release/assets/ (raw.githubusercontent.com via main,
#                signed by `./build.sh release`).
#   * ≥ ~30 MB → release/assets/large/ (GitHub Releases via
#                `./build.sh release-large guest/<name>-<version>`).
# The kernel asset manifest carries a `url=` line for the large lane;
# `https_get_streaming` follows the 302 redirect chain.
for ext in cpio.gz sqfs; do
    f="$OUT.$ext"
    if [ "$(stat -c%s "$f")" -lt $((30 * 1024 * 1024)) ]; then
        cp "$f" "$REPO_ROOT/release/assets/${ASSET_NAME}.$ext"
        cyan "       staged ${ASSET_NAME}.$ext (small lane): sign with ./build.sh release"
    else
        mkdir -p "$REPO_ROOT/release/assets/large"
        cp "$f" "$REPO_ROOT/release/assets/large/${ASSET_NAME}.$ext"
        cyan "       staged ${ASSET_NAME}.$ext (large lane)"
    fi
done
cyan "       upload with: ./build.sh release-large guest/${BUNDLE_NAME}-${VERSION} ${ASSET_NAME}.sqfs"
