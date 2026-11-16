#!/usr/bin/env bash
# Build PID-1 and pack it as the microVM's initramfs (/init alone).
#
#   ARCH=x86_64 (default) -> release/assets/microvm-initramfs.cpio.gz
#   ARCH=aarch64          -> release/assets/microvm-initramfs-arm64.cpio.gz
#
# The aarch64 build embeds microvm/linux/npkwm/npkwm-aarch64
# (`ARCH=aarch64 microvm/linux/npkwm/build.sh`). The packing matches
# build_microvm_initramfs in the top-level build.sh: reproducible newc,
# mtime and inode zeroed, gzip without a timestamp.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
case "${ARCH:-x86_64}" in
    x86_64|x86)    TARGET=x86_64-unknown-linux-gnu;  NAME=microvm-initramfs.cpio.gz ;;
    aarch64|arm64) TARGET=aarch64-unknown-linux-musl; NAME=microvm-initramfs-arm64.cpio.gz ;;
    *) echo "unknown ARCH=${ARCH} (x86_64 or aarch64)" >&2; exit 1 ;;
esac
OUT="${OUT:-$REPO_ROOT/release/assets/$NAME}"

# The embedded npkwm carries mtime 0, which cargo's freshness check never
# sees as newer than the last build: always rebuild.
(cd "$SCRIPT_DIR" && cargo clean --release --target "$TARGET" -p microvm-init -q \
    && cargo build --release --locked --target "$TARGET")
BIN="$SCRIPT_DIR/target/$TARGET/release/microvm-init"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp "$BIN" "$STAGE/init"
chmod +x "$STAGE/init"
touch -d @0 "$STAGE/init"
(cd "$STAGE" && echo init | cpio -o -H newc --reproducible 2>/dev/null | gzip -n9 > "$OUT")
echo "$NAME: $(stat -c%s "$OUT") bytes"
