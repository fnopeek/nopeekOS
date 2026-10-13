#!/usr/bin/env bash
# Run the microVM's LibreWolf natively on this Linux host: the same Alpine
# userspace bundle (musl, scudo), no VM, no guest kernel, on the host's
# Wayland session and network. Separates what the guest userspace costs from
# what virtualization costs.
#
#   tools/alpine_native.sh                      # https://www.speedtest.net
#   tools/alpine_native.sh https://example.org
#   PRELOAD=/usr/lib/libjemalloc.so.2 tools/alpine_native.sh   # path inside the bundle
#   SQFS=path/to/other.sqfs DIR=/tmp/other tools/alpine_native.sh
#   PROFILE=1 tools/alpine_native.sh            # Gecko profile, as npkprofile in the guest
#
# With PROFILE=1 the profile is written on exit as lw-prof.json in the bundle's
# home directory ($DIR/tmp/home),
# with the same settings the guest's `npkprofile` uses.
#
# The bundle is unpacked once to $DIR (default /tmp/npk-alpine-native) and is
# writable there, so `tools/alpine_native.sh --apk add jemalloc` installs into it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SQFS="${SQFS:-$ROOT/release/assets/large/microvm-userspace.sqfs}"
DIR="${DIR:-/tmp/npk-alpine-native}"
WL="${WAYLAND_DISPLAY:-wayland-0}"
: "${XDG_RUNTIME_DIR:?needs a Wayland session}"

if [ ! -x "$DIR/usr/bin/librewolf" ]; then
    rm -rf "$DIR"
    unsquashfs -q -d "$DIR" "$SQFS" > /dev/null 2>&1 || true
    [ -x "$DIR/usr/bin/librewolf" ] || { echo "unpacking $SQFS failed" >&2; exit 1; }
fi
cp /etc/resolv.conf "$DIR/etc/resolv.conf"
mkdir -p "$DIR/tmp/xrt" "$DIR/tmp/home"

if [ "${1:-}" = "--apk" ]; then
    shift
    exec unshare --map-root-user --mount sh -c '
        mount --rbind /dev "$0/dev" && mount --rbind /proc "$0/proc" && mount --rbind /sys "$0/sys"
        exec chroot "$0" /sbin/apk "$@"' "$DIR" "$@"
fi

URL="${1:-https://www.speedtest.net}"
PROF_ENV=
if [ -n "${PROFILE:-}" ]; then
    rm -f "$DIR/tmp/home/lw-prof.json"
    PROF_ENV="MOZ_PROFILER_STARTUP=1 MOZ_PROFILER_STARTUP_INTERVAL=1 MOZ_PROFILER_STARTUP_ENTRIES=16777216 MOZ_PROFILER_STARTUP_FILTERS=GeckoMain,Socket,Worker,Renderer,Compositor,Timer,StreamTrans MOZ_PROFILER_STARTUP_FEATURES=js,stackwalk,cpu,fileio MOZ_PROFILER_SHUTDOWN=/tmp/home/lw-prof.json"
fi
exec unshare --map-root-user --mount sh -c '
    D=$0; R=$1; W=$2; U=$3; P=$4; PE=$5
    mount --rbind /dev "$D/dev" && mount --rbind /proc "$D/proc" && mount --rbind /sys "$D/sys"
    mount --rbind "$R" "$D/tmp/xrt" || exit 1
    exec chroot "$D" /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME=/tmp/home \
        XDG_RUNTIME_DIR=/tmp/xrt WAYLAND_DISPLAY="$W" MOZ_ENABLE_WAYLAND=1 \
        ${P:+LD_PRELOAD=$P} $PE \
        /usr/bin/librewolf --no-remote --profile /tmp/home "$U"' \
    "$DIR" "$XDG_RUNTIME_DIR" "$WL" "$URL" "${PRELOAD:-}" "$PROF_ENV"
