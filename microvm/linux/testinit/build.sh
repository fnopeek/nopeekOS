#!/usr/bin/env bash
# Build the arm64 smoke-test initramfs: this crate's static binary as /init.
#
# Output: release/assets/microvm-test-arm64.cpio.gz
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUT="${OUT:-$REPO_ROOT/release/assets/microvm-test-arm64.cpio.gz}"

(cd "$SCRIPT_DIR" && cargo build --release)
BIN="$SCRIPT_DIR/target/aarch64-unknown-linux-musl/release/testinit"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp "$BIN" "$STAGE/init"
chmod 0755 "$STAGE/init"
touch -d @0 "$STAGE/init"
(cd "$STAGE" && echo init | cpio -o -H newc --reproducible --owner 0:0 2>/dev/null | gzip -n9 > "$OUT")
echo "microvm-test-arm64.cpio.gz: $(stat -c%s "$OUT") bytes"
