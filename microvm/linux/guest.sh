# Sourced by the guest component builds: picks the build root for the guest
# architecture in $ARCH (x86_64, the default, or aarch64).
#
#   x86_64:  Arch Linux on the pinned snapshot (arch.sh)
#   aarch64: Arch Linux ARM, run through qemu-user (alarm.sh)
#
# guest_fetch, guest_run ROOT SCRIPT and guest_rm DIR stand for the
# arch_*/alarm_* functions; $GUEST_PACMAN is the pacman command to use inside
# SCRIPT; $GUEST_SUFFIX ("" or "-aarch64") goes on every output name.

GUEST_SH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
case "${ARCH:-x86_64}" in
    x86_64|x86) GUEST_ARCH=x86_64 ;;
    aarch64|arm64) GUEST_ARCH=aarch64 ;;
    *) echo "unknown ARCH=${ARCH} (x86_64 or aarch64)" >&2; exit 1 ;;
esac

if [ "$GUEST_ARCH" = aarch64 ]; then
    source "$GUEST_SH_DIR/alarm.sh"
    guest_fetch() { alarm_fetch; }
    guest_run() { alarm_run "$@"; }
    GUEST_PACMAN="alarm_pacman"
    GUEST_SUFFIX="-aarch64"
else
    source "$GUEST_SH_DIR/arch.sh"
    guest_fetch() { arch_fetch; }
    guest_run() { arch_run "$@"; }
    GUEST_PACMAN="in_root pacman"
    GUEST_SUFFIX=""
fi
guest_rm() { arch_rm "$@"; }
