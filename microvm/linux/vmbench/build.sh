#!/usr/bin/env bash
# Build vmbench statically against musl, inside a throwaway root of the
# pinned Arch snapshot (microvm/linux/arch.sh, no root needed). The binary
# runs unchanged in the guest and on a host.
#
# Output: microvm/linux/vmbench/vmbench (committed; PID 1 embeds it).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/../arch.sh"
OUT="$SCRIPT_DIR/vmbench"

arch_fetch
STAGE="$(mktemp -d)"
trap 'arch_rm "$STAGE"' EXIT
arch_run "$STAGE/root" "
    in_root pacman -Syu --noconfirm --needed base-devel musl linux-api-headers >/dev/null 2>&1
    mkdir -p \$R/src
    cp '$SCRIPT_DIR/vmbench.c' \$R/src/
    in_root /bin/sh -c '
        set -e
        cd /src
        # musl-gcc leaves out /usr/include; the kernel headers vmbench
        # uses come in alone.
        mkdir -p kinc && ln -s /usr/include/linux /usr/include/asm /usr/include/asm-generic kinc/
        musl-gcc -isystem kinc -std=c11 -O2 -Wall -Wextra -Werror -static -pthread vmbench.c -o vmbench
        strip vmbench
    '
    cp \$R/src/vmbench '$STAGE/vmbench'
"
cp "$STAGE/vmbench" "$OUT"
echo "vmbench: $(stat -c%s "$OUT") bytes"
