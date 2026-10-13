#!/usr/bin/env bash
# Build vmbench statically against musl, inside a throwaway chroot of the
# pinned Alpine release (no root needed: `unshare --map-root-user`). The
# binary runs unchanged in the guest and on a host.
#
# Output: microvm/linux/vmbench/vmbench (committed; PID 1 embeds it).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
source "$REPO_ROOT/microvm-userspace/alpine.env"

CACHE="${HOME}/.cache/nopeekos/alpine"
TARBALL="alpine-minirootfs-${ALPINE_VERSION}-x86_64.tar.gz"
OUT="$SCRIPT_DIR/vmbench"

mkdir -p "$CACHE/apks"
[ -f "$CACHE/$TARBALL" ] || curl -L --fail --max-time 120 -o "$CACHE/$TARBALL" \
    "https://dl-cdn.alpinelinux.org/alpine/${ALPINE_BRANCH}/releases/x86_64/${TARBALL}"
echo "${ALPINE_MINIROOTFS_SHA256}  $CACHE/$TARBALL" | sha256sum -c - >/dev/null \
    || { echo "sha256 mismatch on $TARBALL" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
tar -xzf "$CACHE/$TARBALL" -C "$STAGE"
echo "nameserver 1.1.1.1" > "$STAGE/etc/resolv.conf"
mkdir -p "$STAGE/src" "$STAGE/var/cache/apk"
cp "$SCRIPT_DIR/vmbench.c" "$STAGE/src/"

unshare --user --map-root-user --mount --pid --fork --mount-proc sh -c "
    set -e
    mount --bind '$CACHE/apks' '$STAGE/var/cache/apk'
    chroot '$STAGE' /sbin/apk update >/dev/null
    chroot '$STAGE' /sbin/apk add --no-progress build-base linux-headers >/dev/null
    chroot '$STAGE' /bin/sh -c '
        set -e
        cd /src
        cc -std=c11 -O2 -Wall -Wextra -Werror -static -pthread vmbench.c -o vmbench
        strip vmbench
    '
"
cp "$STAGE/src/vmbench" "$OUT"
touch -d @0 "$OUT"
echo "vmbench: $(stat -c%s "$OUT") bytes"
