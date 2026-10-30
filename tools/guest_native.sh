#!/usr/bin/env bash
# Run the microVM's Firefox natively on this Linux host: the same Arch
# userspace bundle, no VM, no guest kernel, on the host's
# Wayland session and network. Separates what the guest userspace costs from
# what virtualization costs.
#
#   tools/guest_native.sh                      # https://www.speedtest.net
#   tools/guest_native.sh https://example.org
#   PRELOAD=/usr/lib/libjemalloc.so.2 tools/guest_native.sh   # path inside the bundle
#   SQFS=path/to/other.sqfs DIR=/tmp/other tools/guest_native.sh
#   PROFILE=1 tools/guest_native.sh            # Gecko profile, as npkprofile in the guest
#
# With PROFILE=1 the profile is written on exit as app-prof.json in the bundle's
# home directory ($DIR/tmp/home),
# with the same settings the guest's `npkprofile` uses, and the browser's
# memory map after 20 s as app-maps.txt beside it: the profile's own library
# list can be overwritten by a child process writing the same file.
#
# The bundle is unpacked once to $DIR (default /tmp/npk-guest-native) and is
# writable there.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SQFS="${SQFS:-$ROOT/release/assets/large/microvm-firefox.sqfs}"
DIR="${DIR:-/tmp/npk-guest-native}"
WL="${WAYLAND_DISPLAY:-wayland-0}"
: "${XDG_RUNTIME_DIR:?needs a Wayland session}"

if [ ! -x "$DIR/usr/bin/firefox" ]; then
    rm -rf "$DIR"
    unsquashfs -q -d "$DIR" "$SQFS" > /dev/null 2>&1 || true
    [ -x "$DIR/usr/bin/firefox" ] || { echo "unpacking $SQFS failed" >&2; exit 1; }
fi
cp /etc/resolv.conf "$DIR/etc/resolv.conf"
mkdir -p "$DIR/tmp/xrt" "$DIR/tmp/home"

URL="${1:-https://www.speedtest.net}"
PROF_ENV=
if [ -n "${PROFILE:-}" ]; then
    rm -f "$DIR/tmp/home/app-prof.json"
    PROF_ENV="MOZ_PROFILER_STARTUP=1 MOZ_PROFILER_STARTUP_INTERVAL=1 MOZ_PROFILER_STARTUP_ENTRIES=16777216 MOZ_PROFILER_STARTUP_FILTERS=GeckoMain,Socket,Worker,Timer,StreamTrans,IPC,RemoteLzy,File,BgIO,BackgroundThreadPool,Renderer MOZ_PROFILER_STARTUP_FEATURES=js,stackwalk,cpu,fileio MOZ_PROFILER_SHUTDOWN=/tmp/home/app-prof.json"
fi
exec unshare --map-root-user --mount sh -c '
    D=$0; R=$1; W=$2; U=$3; P=$4; PE=$5
    mount --rbind /dev "$D/dev" && mount --rbind /proc "$D/proc" && mount --rbind /sys "$D/sys"
    mount --rbind "$R" "$D/tmp/xrt" || exit 1
    [ -n "$PE" ] && ( sleep 20; cat /proc/$$/maps > "$D/tmp/home/app-maps.txt" ) &
    exec chroot "$D" /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME=/tmp/home \
        XDG_RUNTIME_DIR=/tmp/xrt WAYLAND_DISPLAY="$W" MOZ_ENABLE_WAYLAND=1 \
        ${P:+LD_PRELOAD=$P} $PE \
        /usr/bin/firefox --no-remote --profile /tmp/home "$U"' \
    "$DIR" "$XDG_RUNTIME_DIR" "$WL" "$URL" "${PRELOAD:-}" "$PROF_ENV"
